# Контракт машины: physical memory, bus, bridge and translation probe

**Статус:** локально принят ограниченный срез; это не контракт полной VM и не
подтверждение MMU, устройств, загрузки Linux или JIT-интеграции.

Нормативная карта платы и будущие boot reservations остаются в
[VIRTUAL_BOARD.md](VIRTUAL_BOARD.md) и в принятом manifest S4-00. Целевой контракт
нескольких процессоров описан в [SMP_CONTRACT.md](SMP_CONTRACT.md); разделы ниже
фиксируют отдельные bounded-срезы машины и не означают, что SMP wiring реализован.

## Адреса и карта

- `GuestPhysicalAddress` — отдельный тип гостевого физического byte address; он
  не является host pointer, linear address или port address.
- `GuestPhysicalRange` — непустой полуоткрытый диапазон `[start, start + len)`.
  Внутренняя проверка endpoint выполняется в расширенном 65-битном домене, поэтому
  width 64 корректно представляет адрес `u64::MAX` с длиной 1.
- `MachineMemoryMap::new(width, regions)` сортирует regions, проверяет width
  1—64, checked bounds и запрещает overlap. Карта не меняется после создания.
- `MemoryRegionKind` различает `Ram`, `Rom`, `Reserved`, `Mmio` и `Framebuffer`.
  Backing создаётся для RAM, `Framebuffer` и (S7-03) `Rom`; reserved/MMIO
  классифицируются и отклоняют доступ до появления соответствующих владельцев.
  `Rom` читается, любая запись отклоняется `ReadOnlyRegion(Rom)`, наполняет её
  только машинный загрузчик (раздел «Firmware board v1 and handoff»).
  `Framebuffer` — память дисплейного устройства (раздел «Display device
  (S10-03)»): её backing не входит в `RamLimits`, карта принимает не больше одного
  такого региона, начало и длина кратны 4 KiB, длина не больше `0x0240_0000`
  (иначе `MultipleFramebufferRegions` / `InvalidFramebufferRegion` при построении
  карты, до allocation).
- Port I/O, E820, boot reservations, aliases и remapping пока не реализованы и
  не должны моделироваться перекрывающимися RAM regions.
- Карта firmware board v1 (окна ROM и NVRAM, boot reservations внутри одного
  `Ram`, handoff block, временные таблицы страниц) и семантика `Rom` для неё
  (чтение данных; запись — backend-ошибка; fetch — `#PF` по NX) описаны в
  [FIRMWARE_BOOT.md](FIRMWARE_BOOT.md) (S7-02, ожидает ревью); реализованная
  часть и её границы — раздел «Firmware board v1 and handoff» ниже (S7-03).

## RAM ownership и доступ

`PhysicalMemory` — единственный владелец RAM backing в `od-machine`. RAM
создаётся zeroed, с явным `RamLimits::max_total_bytes`, проверяемым до первой
allocation. Наружу не выдаются mutable slices, raw pointers или долгоживущие
handles.

`read_bytes` и `write_bytes` выполняют один checked access целиком. Access,
пересекающий две regions, overflow или hole, отклоняется до изменения RAM или
буфера назначения. Пересечение guest 4 KiB границы внутри одной region
допускается; host page size не участвует в семантике. Scalar helpers используют
little-endian encoding.

Ошибки карты, allocation и physical access являются host-side результатами
этого слоя. Они не превращаются автоматически в guest `#PF`; linear translation
и fault delivery появятся на S4-06—S4-09.

## Committed writes

Все byte/scalar writes используют один `write_bytes` path. Callback получает
ровно один `GuestPhysicalRange` только после полной записи в RAM. Ошибка,
включая отказ region, cross-region access или исчерпание page generation,
не вызывает callback и не меняет bytes или поколения. Для каждого RAM guest
4 KiB page `PhysicalMemory` хранит непереполняющийся version, изначально равный
1; успешная запись повышает только поколения затронутых страниц. Проверка
всех затронутых поколений выполняется до копирования. Запись в ROM/Reserved
сохраняет `ReadOnlyRegion` классификацию и завершается до generation lookup.

Это консервативная invalidation основа для code snapshots, а не заявление о
цене write path: на страницу RAM расходуется 8 байт, а каждая запись проверяет
и повышает версии затронутых страниц. Измерений пока нет; решение provisional.

Запись в `Framebuffer` (device-owned pixel memory, S10-03) фиксируется тем же
`write_bytes`, но обходит и callback, и поиск/повышение page generations:
для `Framebuffer` поколения не создаются, поэтому framebuffer-запись не может
привести к `AddressOverflow` или `CodeGenerationExhausted`, а от байтов
framebuffer не зависит ни один code-кеш. `code_page_generation` и
`MachineBus::tracked_code_snapshot` отвергают `Framebuffer` так же, как любой
не-RAM region (`UnreadableRegion`).

## Executable RAM snapshot и loader patch (S4-16a)

`MachineBus::tracked_code_snapshot` принимает ненулевой byte range внутри одной
guest 4 KiB страницы и `AccessOrigin::Explicit`; descriptor-table origin
отклоняется, поскольку walker иначе применяет supervisor page permissions
вместо предоставленного CPL. Он выполняет tracked `Fetch`
первой code byte с тем же privilege/mode context, поэтому Long64 walker
проверяет executable permissions и коммитит фактически нужные page-table
Accessed bits. После translation шина проверяет, что вся физическая
страница является RAM, и копирует запрошенный range из того же mapping. В
пределах одной guest page все байты используют один leaf translation и одну
permission set; crossing page отклоняется до walk. Snapshot содержит линейный
base, физический page base, owned bytes и generation именно этой физической
страницы. Snapshot и loader session сохраняют точный page offset диапазона;
два alias должны указывать на один physical byte range.

`patch_tracked_code_byte` сначала tracked-чтением проверяет, что virtual alias
указывает на ожидаемую физическую страницу, затем выполняет одну tracked byte
запись через этот alias. Отказ операции не коммитит сам patch byte. Оба walk
могут закоммитить Accessed effects; только write walk может закоммитить Dirty
effects до более позднего отказа. Если физическое хранение этих записей
перекрывает code backing, они могут изменить code bytes и увеличить backing
generation даже при неуспешном patch. Такое совпадение исключено из узкой
гарантии неизменности backing при отказе. Host перед каждым исполнением получает свежие bytes/generation и
инвалидирует выбранный alias cache до dispatch; старая версия поэтому не
исполняется повторно. Этот API намеренно покрывает один byte patch в
ограниченном loader сценарии. Полный
writer notification, guest-generated stores, remap/TLB, cross-page code,
DMA, MMIO и active-code retirement остаются вне S4-16a.

## Guest MOV committed-write observation (S4-16b)

`od-machine::step_paged_mov` retains its exact one-step Long64 MOV profile and
reports architectural page faults as a `RunResult`; committed physical write
notifications are delivered synchronously for actual RAM writes, including
page-walker A/D updates. `PagedCodeSession` supplies the fixed paging snapshot
and the caller's explicit CPL/origin context, while keeping CR3 as the adapter's
separate paging input. Because the CPU profile requires `state.cr3 == None`,
the host passes it a local state clone with CR3 cleared and restores the original
CR3 before copying a successful profile result back. Machine input errors leave
the caller's state unchanged; architectural faults retain the profile's ordinary
state-commit behavior. The host session must not lose committed notifications
when the guest step reports a fault or another error.

The S4-16b host fixture verifies that a Ring3 write walk can commit a PML4
Accessed update into the physical code page and then fault on a missing operand
mapping. The page-table write remains committed; the operand write and guest
retirement do not occur. This contract does not add cross-page store policy or
change the interpreter's exact admitted forms. The existing step can dispatch
MMIO operand accesses, but S4-16b validates RAM only; callback-based code
reconciliation observes committed physical RAM writes, not device-side writes.
See the
[S4-16b report](../reports/STAGE_4_S4-16B_2026-11-06.md).

## Physical RAM loader write observation (S4-16c)

`MachineBus::patch_physical_ram_with_callback` forwards `PhysicalMemory`'s
committed-write notification for a checked physical RAM byte-range write. The
callback receives the exact committed `GuestPhysicalRange`; rejected ranges,
ROM, and other non-RAM targets do not produce a commit notification. The
existing `patch_physical_ram` convenience method delegates with a no-op
callback, preserving its caller contract.

`PagedCodeSession::write_physical_ram` is a privileged, synchronous loader path
that can be called only through the session while native code is inactive. It
does not expose the bus, RAM references, or an arbitrary writer closure. The
session collects committed ranges and reconciles them before propagating either
success or error. An overlap with the captured physical code page reads its
current generation and invalidates both inactive alias entries. A write to a
different page preserves those entries; a rejected write has no callback and
preserves bytes, generation, and entries under PhysicalMemory's validate-before-
commit contract. Guest CPL permission checks are not part of this privileged
loader operation.

The S4-16c fixture checks a byte-range callback, loader replacement of a cached
instruction immediate and fresh execution through both aliases, conservative
invalidation for a same-page write outside the admitted instruction range,
generation isolation for a neighboring guest page, and failure at a RAM/ROM
boundary and within ROM. It does not establish arbitrary writers, remap/TLB,
active-block retirement, MMIO/DMA or SMP behavior. See the
[S4-16c report](../reports/STAGE_4_S4-16C_2026-11-06.md).

## Tracked code-walk committed writes (S4-16d)

`tracked_code_snapshot_with_callback` reports physical page-walker writes
committed while fetching the code snapshot. The callback fires synchronously
and in walker order; a later translation or physical-memory failure does not
erase earlier committed A-bit updates. `patch_tracked_code_byte_with_callback`
reports committed writes from its read preflight, write walk, and requested
operand byte in actual operation order. A failed patch reports only writes that
committed before the error; it never reports an uncommitted operand byte. The
existing callback-free methods delegate to these APIs with a no-op callback.

In `PagedCodeSession`, notifications from both tracked paths are reconciled
against the captured physical code page before returning either a report or an
error. If a committed A/D write overlaps that backing, both idle alias caches
are invalidated synchronously. This does not change page-table permissions,
translation/TLB semantics, or the ordinary fault classification. The host
patch-failure fixture observes the read preflight setting the alternate PDE A
bit at `0x7010` before the write walk faults. The snapshot-failure fixture
observes the root PML4 A bit at `0x7400` before the later missing-PDE fault. See the
[S4-16d report](../reports/STAGE_4_S4-16D_2026-11-06.md).

## Bounded tracked INVLPG execution (S4-16e)

`step_paged_invlpg` executes the existing exact `od-isa::step_invlpg` through
the RAM-only tracked fetch adapter. The callback reports each committed
page-walker A/D range in order. `Ok(PagedInvlpgStepReport)` carries the exact
CPU exit (including architectural fault or rejection), optional page-fault
record, and tracked-access error; fetch effects remain observable when the
instruction later raises an architectural fault. Rejected inputs are refused
before fetch. This adapter does not model a production TLB.

`PagedCodeSession::execute_invlpg` accepts the same fixed Long64 paging
controls as the captured code session. It reconciles fetch callbacks before
returning. Instruction faults and ISA rejections are carried in
`Ok(result).step.cpu.exit`; a tracked fetch backend failure is carried in
`Ok(result).step.access_error`. The `Err` path is reserved for adapter/session
configuration errors. After exactly one retired canonical INVLPG effect, the
bounded host session clears both idle native alias entries and releases its
backing/range/alias binding, permitting fresh admission after remap. Any
canonical effect conservatively resets both entries; noncanonical retirement
and rejected/faulting inputs do not reset the binding. Guest code must still
execute an INVLPG for each virtual alias.

The scope remains one vCPU, one code page, two inactive aliases, and a fixed
paging snapshot. Cross-page code, arbitrary writers, production TLB behavior,
MMIO/DMA, SMP, and active-block retirement are outside this slice. See the
[S4-16e report](../reports/STAGE_4_S4-16E_2026-11-06.md).

## Синхронная MMIO/PIO bus

`MachineBus` переиспользует одну `MachineMemoryMap` и один `PhysicalMemory`;
второй backing или независимая карта физических regions не создаются. MMIO
binding может ссылаться только на полный `MemoryRegionKind::Mmio`, а PIO имеет
отдельную карту `PortRange` в пространстве `0..65536` с checked endpoint.

`BusDevice` получает один `DeviceAccess` с `IoSpace`, offset и `AccessWidth`.
Доступы `Byte/Word/Dword/Qword` маскируют значение до ширины; PIO Qword в этом
срезе запрещён. Шина проверяет width, overflow и полное попадание в одно окно до
вызова устройства. Она не разбивает смешанный или cross-window access и не
повторяет device operation.

Ошибки построения карты, доступа шины и самого устройства разделены. Ошибка
устройства должна быть возвращена до guest-visible изменения или внешнего
эффекта; это проверено тестовым устройством. MMIO/PIO writes не вызывают RAM
committed-write callback. CPU `IN/OUT`, I/O privilege checks, guest exceptions
и преобразование linear address остаются в `od-isa` и CPU bridge.

## Bounded COM1 polling UART

`Uart16550PollingTx` is a bounded, synchronous byte-wide PIO profile bound by
the caller to COM1 ports `0x3F8–0x3FF`. It is not a claim of full 16550/16550A
compatibility. The public `Uart16550Handle` shares the device's
`Arc<Mutex<UartState>>`; cloning the device or handle aliases the same state.
The handle exposes explicit `transfer_holding_to_shift`,
`complete_transmission` and `reset` operations, so host call timing does not
advance transmission.

Only byte-wide PIO accesses are admitted. With DLAB clear, offset `0` is the
shared THR/RBR location: a write submits one TX holding byte and a read is an
RBR read. With DLAB set, offset `0` and `1` select DLL and DLM. The other
bounded registers are LCR, LSR and SCR. The accepted line format is 8N1 with
a nonzero divisor; FIFO is disabled. As of S4-24e IER bits 0-1 and MCR OUT2 are
accepted (see [S4-24e](#stage-4-uart-irq4-probe-s4-24e)); with IER zero IIR still
reports no interrupt. The UART interrupt line is connected to IOAPIC INTIN4 only in the
Stage 4 UART IRQ probe profile; on other boards (firmware board v1, flat probes) the line is
not wired, so an IER write there is accepted but produces no IRQ.

RX is one caller-injected byte. `LSR.DR` is bit `0` and is set exactly while
the receive register is occupied. Reading RBR at offset `0` returns the byte
and clears `LSR.DR`; reading an empty RBR returns `DeviceError::WriteOnly` and
does not create or replace a byte. A second injection while the receive
register is occupied returns `DeviceError::Busy`.

TX state consists of one holding byte and one shift byte. LSR reports THRE as
`0x20` when the holding register is empty and TEMT as `0x40` when both stages
are empty. A busy holding register or unsupported access is rejected without
changing device state. Completion returns the original raw byte; the model
writes no host output.

The register subset is based on TI [PC16550D datasheet, SNLS378C](https://media.digikey.com/pdf/Data%20Sheets/Texas%20Instruments%20PDFs/PC16550D.pdf),
including its register selection and THRE/TEMT status definitions. The model
does not claim baud-accurate timing, FIFO operation, IRQ4, guest Linux
serial-driver compatibility or boot support. Parent task S4-24 remains open
for complete UART behavior and interrupt integration.

## Bounded COM1 polling transmitter and CPU PIO step

`DiagnosticCpl0PioU8V1` connects the CPU to the existing synchronous PIO bus
through `od_machine::step_diagnostic_pio_u8`. It admits one bare `EC` (`IN
AL,DX`) or `EE` (`OUT DX,AL`) instruction per call, only in Long64 at CPL0 and
with CR0/CR4 both `None`. The machine adapter fetches from RAM using flat
identity addressing (`linear == physical`); the instruction performs exactly
one byte PIO callback. Success retires once and advances RIP by one; fetch or
PIO failure does not retire or change CPU state. The CPU contract defines the
register and definedness details.

When COM1 is bound to `Uart16550PollingTx`, `IN AL,DX` reads and clears its
single injected RX byte, while `OUT DX,AL` submits one byte to the bounded TX
holding stage. A second RX injection while occupied and writes to a busy TX
holding stage are rejected without replacing pending data. TX still advances
only through explicit handle operations. The slice has no TSS I/O bitmap or
IOPL matrix and does not claim word/dword/qword port instructions, immediate
ports, REP/string I/O, IRQ, APIC, Linux serial compatibility or Linux boot.
See the [S4-24b report](../reports/STAGE_4_S4-24B_2026-11-05.md) and its
[machine verification record](../testing/MACHINE.md#s4-24b-bounded-cpu-pio-and-com1-rx).

## Bounded paged CPU PIO step

`od_machine::step_paged_pio_u8` connects `PagedCpl0PioU8V1` to the existing
tracked RAM page walker and `MachineBusPortIo`. The machine entry accepts CR3
and EFER, validates them through the existing `Long64ProbeConfig` four-level,
4 KiB profile, and supplies the CPU entry with the real page-aware
`GuestMemory` adapter. The CPU entry validates only CPU-side admission and
CR0/CR4 requirements. Fetch is page-aware and explicitly marked
`AccessOrigin::Explicit`. Only bare Long64 `EC`/`EE` at CPL0 are admitted.

The integration evidence executes a non-identity `MOV RAX,[RBX]` and then
`OUT DX,AL` without host mutation between the two CPU steps, transferring the
first result's definedness. It checks that UART output matches the byte loaded
from guest RAM. If PIO fails after fetch, the CPU result and definedness remain
uncommitted, while page-table A/D writes and callbacks already performed by
fetch remain visible. The slice does not provide exception delivery or CR2
update. See the [S4-24c report](../reports/STAGE_4_S4-24C_2026-11-05.md),
[CPU contract](CPU_CONTRACT.md#bounded-paged-cpu-to-pio-profile-pagedcpl0piou8v1-s4-24c)
and [machine verification record](../testing/MACHINE.md#s4-24c-bounded-paged-cpu-pio).

## Stage 4 guest UART echo image (S4-24d)

`Stage4UartEchoProbeV1` is an explicit strict profile over the fixed Stage 4
board and page-table layout. Its code bytes are exactly `EC EE F4` (`IN AL,DX`,
`OUT DX,AL`, `HLT`). The profile loader checks this sequence; the legacy
`Stage4FlatProbeV1` loader continues to reject it. `PagedVcpuSession` retains
the selected profile and only resets with another image matching that profile.

The CLI injects at most one externally supplied RX byte by scheduling and
dispatching one bounded UART RX event before guest execution. `--expect-byte` is
an independent host expectation; it is not passed to the guest or used to
produce TX. Empty RBR retains the existing PIO error. Completion requires HLT,
three retired instructions, `RIP=0x400003`, RAX equal to the expected byte, and
actual UART TX exactly `[expected]`. The result reports `observed_rax`; the
memory-specific `observed_data_qword` remains null for this profile.

Budgets and deterministic cancellation expose the committed instruction prefix.
Cancellation at zero is handled before RX delivery; later cancellation counts
guest instruction-boundary polls. The profile does not add LSR polling loops,
baud timing, FIFO behavior, IRQ/APIC, HLT wakeup, host threads or a scheduler.
The builder verifies the assembled bytes/layout and artifact hashes. See the
[S4-24d report](../reports/STAGE_4_S4-24D_2026-11-07.md) and
[verification method](../testing/MACHINE.md#s4-24d-guest-uart-rxinout-echo).

## Stage 4 UART IRQ4 probe (S4-24e)

> Уточнено в S4-38L9y: IER хранит биты 0..3 (RDA, THRE, RLS, MS), биты 4..7 читаются как 0 и отбрасываются при записи; отказ `UnsupportedConfiguration` остался только для битов MCR, кроме OUT2 (см. [CPU_CONTRACT](CPU_CONTRACT.md), раздел S4-38L9y).

> S4-24f: IER бит 2 (RLS) имеет источник: LSR OE/PE/FE/BI (OE при доставке `deliver_received_byte` в полный RBR; PE/FE/BI задаёт host), IIR `0x06` с приоритетом над RDA и THRE, сброс чтением LSR. Modem status, loopback и FIFO не моделируются. См. [отчёт S4-24f](../reports/STAGE_4_S4-24F_2026-11-21.md).

**UART interrupt model.** `Uart16550PollingTx` gains IER bit 0 (received data
available, RDA) and bit 1 (transmitter holding register empty, THRE); any other
IER bit (line status, modem status) and any MCR bit except OUT2 are rejected with
`UnsupportedConfiguration` without changing state. IIR reports `0x04` (RDA) with
priority over `0x02` (THRE), else `0x01`; the FIFO bits stay zero. RDA is
level-like and is cleared only by reading RBR; reading IIR is repeatable and does
not clear it. THRE is raised when the guest enables it while the holding register
is empty and whenever the holding register is emptied by the (explicit) transfer
to the shift register; it is cleared by writing THR, by reading IIR when THRE is
the reported source, and by clearing the IER bit. Writing FCR (offset 2) other
than zero is still rejected: there is no FIFO.

**OUT2 decision.** OUT2 is modelled as a gate on the interrupt output only: the
line is `(IIR != 0x01) && MCR.OUT2`, and IIR itself does not depend on OUT2. On a
PC the 16550 interrupt pin is gated by OUT2 and the Linux 8250 driver sets OUT2
in `up->mcr` when it enables interrupts (`serial8250_do_set_mctrl` and the start-up
and IRQ-probe paths in `8250_port.c`). The offline sources were not available and a
Context7 query returned no usable text, so this rationale is from the author's
knowledge of the driver and is not machine-verified. Modelling the gate costs one
MCR bit; the gate is a modelling choice for this slice, not a claim about what a PC OS writes
(real drivers often write DTR|RTS|OUT2, which this model rejects). MCR reads
return the stored value. DTR/RTS/OUT1/loopback are not modelled.

**Route owner.** The UART output is a pure level (`Uart16550Handle::interrupt_line`,
`peek_iir`, `received_byte`; none have side effects). The only UART to IOAPIC
connection is `Stage4UartIrqProbe`: after every operation that can change UART
state (RX delivery, a guest PIO access) it samples the line and, on a level change,
calls `IoApicHandle::set_intin4` once. There is no callback from the UART, no
background scheduler and no thread. IOAPIC route and Local APIC semantics are
unchanged: a masked edge is ignored and unmasking does not replay it (S4-26a/b),
repeated high does not create an edge, and a vector already in service is not
delivered again until EOI (the second edge stays latched in the IRR).

**Profile `Stage4UartIrqProbeV1`.** A flat identity-mapped Long64/CPL0 guest:
RAM `0..0x7000`, raw image 0x6000 bytes at GPA `0x1000`, entry `0x4000`, `RSP=0x6ff0`
(the address of the IDTR pseudo-descriptor, so the interrupt frame grows below
it), COM1 `0x3F8..0x3FF`, IOAPIC `0xFEC00000`, Local APIC `0xFEE00000`, CS=8 (code64)
and SS=0x10, GDTR `{0x2000, 0x17}`, IDTR `{0, 0}` before the guest's `LIDT`, IF=0,
all integer flags defined. The loader validates the GDT, exactly one IDT gate
(vector `0x51`, selector 8, type `0x8E`, handler in the code page), the entry
`LIDT [RSP+0]`, the IDTR and that every other byte of the image is zero. The
guest executes `LIDT`, enables the Local APIC (`SVR=0x1FF`), unmasks INTIN4
(redirection entry `0x51`, edge, fixed) by IOAPIC MMIO writes, programs COM1 (DLAB
divisor, 8N1, `MCR=OUT2`, `IER=RDA`), runs `STI` and loops `HLT; JMP`. The handler
reads IIR, reads RBR, writes the byte to THR, writes the Local APIC EOI and `IRETQ`.
Paging is not used because maskable-interrupt entry, STI, HLT and same-CPL IRETQ
exist only in flat form and paged LIDT/PIO exist only for paged CR0; a flat
`step_flat_lidt` (shared body with `step_paged_lidt`, CR0/CR3/CR4 absent) was added
so the guest installs its own IDT. The session is not a replacement for the paged
board: Linux boot and paged IRQ entry are not claimed.

**Session semantics.** `Stage4UartIrqProbe::run(budget, stop)` takes one host step
per call to its loop: an instruction (selected by the opcode at RIP: `HLT`, `STI`,
`LIDT`, byte `IN`/`OUT`, `IRETQ`, otherwise the generic interpreter), an interrupt
entry, a halt-wait poll, or an idle advance. Due RX bytes are dispatched before every
step. When the guest is halted with nothing deliverable and a future RX deadline
exists, virtual time jumps to that deadline (trace `idle_advance`); with no future
event the run ends with `IdleHalt` (also when a due byte is stuck behind an unread
RBR). STOP is checked before the budget. TX completes at the end of each `OUT`
(no baud timing). The trace records RX delivery, UART line changes, IRR set, IRQ
entry (IRR to ISR), EOI (ISR cleared), UART port reads/writes, `LIDT`, `STI`, `HLT`,
`IRETQ` and idle advances, bounded to 512 events. The runner command
`--stage4-uart-irq-probe IMAGE [--budget N] [--rx BYTE@NS]... [--expect-echo B]...
[--expect-irqs N] [--cancel-after N]` prints guest state, counts, APIC/UART state,
budgets, the stop reason and the full trace; `guest_completed` requires an idle halt,
the exact echo bytes, equal counts of entries, EOIs and IRETQs, and a clear IRR and
ISR. Builder variants `--ier`, `--late-unmask`, `--omit-eoi` and `--omit-rbr-read`
exist only to prove that broken guests fail that check.

Not covered: FIFO, baud timing, line-status and modem-status interrupts, level
route, JIT, Linux 8250 compatibility, a production scheduler, IRQ from CPL3, paged
interrupt entry, reset of the session. Parent S4-24 stays open.
Since S4-27i5 the run loop of this probe is the `InterruptPump` of S4-27i1..i3
with a UART owner hook; the guest-visible behavior and these limits are
unchanged (see the S4-27i5 section). See the
[S4-24e report](../reports/STAGE_4_S4-24E_2026-11-08.md) and
[verification method](../testing/MACHINE.md#s4-24e-uart-irq4).

## Канал агента: COM2 и `UartPortSpec` (S11-21, S4-24f)

Предложено ADR-0007 (ждёт подтверждения владельца). Полный контракт: [AGENT_CHANNEL_DEVICE.md](AGENT_CHANNEL_DEVICE.md), host ABI — [AGENT_HOST_ABI_V1.md](AGENT_HOST_ABI_V1.md).

- **S4-24f, размещение UART.** `UartPortSpec { base_port, irq_line }` задаёт, где стоит UART: `COM1` = `0x3F8`, IRQ4; `COM2` = `0x2F8`, IRQ3. Регистровая модель не менялась и от порта не зависит; `Uart16550PollingTx::new` и `new_with_handle` по-прежнему строят COM1, `new_with_handle_at(spec)` — другое размещение. `UartPicIrqSource` берёт вход ведущего 8259 из spec handle (COM1 — IR4, как раньше); `UartIoapicIrqSource` ведёт COM1 на INTIN4 и, при `new_with_com2`, COM2 на INTIN3 (вход `COM2_PIN` в `apic_pair` с правилами INTIN4: active high, фронт или уровень, Remote IRR и повтор по EOI).
- **Канал агента.** `AgentChannel` оборачивает UART COM2 (профиль 16450, без FIFO) ограниченной очередью host → гость (питает `inject_received_byte`) и ограниченной очередью гость → host (питается из пути THR); по 65 536 байт (гипотеза до замера). Байт, не влезший в очередь, теряется и учитывается (`rx_overflow_bytes`, `tx_overflow_bytes`). Байты движутся по `AgentChannel::pump`, который Linux-цикл и цикл прошивки `firmware_board` вызывают вместе с COM1, а `holding_occupied` канала входит в предикат остановки кванта; многопоточные циклы `host_parallel` его не вызывают: `thread_parts` отказывает (`Config`) плате с каналом; поддержка — отдельная задача.
- **Плата.** `FirmwareBoardConfig::agent_channel` (по умолчанию `false`) привязывает устройство к `0x2F8..0x300` и снимает `AbsentPortRegion::Uart2` с Linux-профиля. COM3 и COM4 остаются absent (Linux-профиль) или ловушками. Handoff и манифест ACPI не меняются; IRQ3 идёт в IOAPIC INTIN3, когда включён `enable_uart_ioapic_irq4`.
- **Не сделано.** Запись COM2 в манифесте платы (handoff, DSDT; ISSUE-659); IRQ3 через 8259 на плате; pump в `host_parallel`. См. раздел 9 спецификации устройства.

## Display device (S10-03)

Нормативное guest-visible поведение — [DISPLAY_DEVICE.md](DISPLAY_DEVICE.md);
здесь записаны только реализованные границы `crates/od-machine/src/display_device.rs`.

- **Карта.** Регистры — `BusDevice` `DisplayDevice`, привязанный `MmioBinding` к
  региону `Mmio` длиной `0x1000` (`DISPLAY_REGISTERS_BASE = 0xFC00_0000`).
  Пиксели — регион `Framebuffer` (`DISPLAY_FRAMEBUFFER_BASE = 0xF800_0000`,
  `DISPLAY_FRAMEBUFFER_BYTES = 0x0240_0000`) внутри `PhysicalMemory`; у
  устройства байтов кадра нет. Один экран, один формат `BGRX8888`, stride
  `width × 4`, максимум 4096 × 2304.
- **Backing и доступ.** Память заполнена нулями; `read_physical`/`write_physical`/
  flat- и paged-доступ к данным работают как для RAM (любая ширина, невыровненный
  доступ внутри региона), без committed-write callback. Instruction fetch
  в `IdentityMemoryAdapter` и `step_diagnostic_pio_u8` — `MemoryError::Backend`
  (сообщение «unsupported»); в tracked paged-пути fetch — `TrackedAccessError::
  OperandPhysical { UnreadableRegion(Framebuffer) }`, а элементы таблиц страниц
  внутри региона (оба walker-а) — `UnreadableRegion(Framebuffer)` в
  `TableRead`. Везде это backend-ошибка, не `#PF`; A/D-биты в регион не пишутся.
  Доступ, пересекающий границу региона, и доступ в резерв
  `0xFA40_0000–0xFBFF_FFFF` отвергаются существующими правилами карты.
- **Регистры.** Допустимы только `IoSpace::Mmio`, `AccessWidth::Dword`, offset
  кратен 4 и внутри окна; иначе `DeviceError::{UnsupportedSpace,
  UnsupportedWidth, Misaligned, UnknownOffset}` до изменения состояния (на шине —
  `BusAccessError::Device`, в tracked paged-пути — `MmioBackend`). RO и
  зарезервированные offsets игнорируют запись; `MODE_COMMIT` читается как `0`;
  `FRAME_GENERATION` — RO + doorbell. Ошибки, исправимые гостем
  (`SIZE_NOT_SET`, `UNSUPPORTED_SIZE`, `UNSUPPORTED_FORMAT`, `INVALID_COMMAND`,
  `NO_ACTIVE_MODE`, `COUNTER_EXHAUSTED`), видны только в `STATUS`; порядок
  проверок `MODE_COMMIT` — раздел 5 контракта устройства, лимиты сравниваются до
  умножения.
- **Согласование при сборке.** `BusDevice::framebuffer_binding()` (по умолчанию
  `None`) возвращает `FB_BASE`/`FB_SIZE` устройства. `MachineBus::new` отвергает
  `BusBuildError::Display(..)`, если регион `Framebuffer` есть без устройства или
  наоборот, если начало/длина региона отличаются от регистров, если устройств
  больше одного, если оно не привязано к MMIO, привязано к нескольким MMIO-регионам или окно
  не равно `0x1000`.
- **Host API.** `DisplayDevice::new_with_handle()` возвращает устройство и
  типизированный `DisplayHandle`, делящий состояние с ним (по образцу
  `Uart16550Handle`, без downcast из `MachineBus`). Handle даёт
  `current_mode() -> DisplayMode` (все поля `0` = режима нет),
  `frame_generation()`, `mode_change_count()`, `has_current_frame()`, `reset()` и
  `read_frame_into(&MachineBus, &mut [u8]) -> Result<FrameInfo, DisplayReadError>`.
  Чтение копирует ровно `stride × height` байт с начала региона `Framebuffer`
  и сверяет счётчик смены режима до и после копирования (`ModeChanged`); отказы:
  `NoActiveMode`, `NoFrame`, `BufferLength`, `Memory`. Оно ничего не меняет:
  doorbell не звонит, регистры и `generation` не трогает.
- **Правило вызова.** Весь host API handle вызывается только между квантами
  исполнения гостя, владельцем `MachineBus` и тем же потоком, который исполняет
  квант; `read_frame_into` принимает именно ту шину, которой принадлежит
  устройство. Handle не потокобезопасен как протокол: `Mutex` внутри защищает
  состояние от поломки, но не упорядочивает чтение кадра относительно записей гостя.
  Готовность кадра host определяет только по `frame_generation()`; по таймеру
  память не сканируется.
- **Reset.** `DisplayHandle::reset()` — host-операция: режим и регистры
  возвращаются к значениям после reset, `mode_change_count` растёт на 1, если
  режим был активен; `generation` и память framebuffer сохраняются.
- **Не реализовано.** Курсор, несколько экранов, PCI/BAR, прерывания,
  dirty rects, snapshot/restore, другие форматы (список режимов и события —
  ревизия 2, следующий раздел). Dword MMIO-запись в
  tracked paged CPU-пути остаётся блокером гостевого probe S10-08
  (DISPLAY_DEVICE.md, 13.1): `PagedMovMemoryAdapter` пишет только qword, а
  qword-запись в регистр — backend-ошибка `UnsupportedWidth`.
- **Проверено.** Unit-тесты в `display_device.rs`, `physical_memory.rs`,
  `page_walk.rs` и интеграционные `tests/display_device.rs`: значения после reset,
  успешный commit, каждый отказ, чтение кадра после гостевых записей без
  callback, обнаружение смены режима между чтениями и во время копирования,
  запись в RO-регистр, ширины и offsets, fetch и page tables в framebuffer,
  согласование карты и устройства. На `feat/s9-stage` нет page generations и
  `tracked_code_snapshot` (они есть на `main`): пункты 7.4 (а)–(д) контракта
  устройства про них не сверены и не реализованы здесь.

## Display device, revision 2 (S10-13)

Нормативно: [DISPLAY_DEVICE.md](DISPLAY_DEVICE.md), раздел 14, и
[DISPLAY_DRIVER_V1.md](../../protocols/guest/DISPLAY_DRIVER_V1.md). Здесь записаны
только границы, реализованные в `crates/od-machine/src/display_device.rs`. Код
написан и ожидает ревью.

- **Ревизия.** `VERSION = 2`, `CAPS = 0x3` на момент S10-13 (бит 0 `MODE_LIST`,
  бит 1 `EVENTS`; биты 2–3 без своих блоков равны `0`). С блоком курсора
  (раздел «Display device, cursor (S10-14)» ниже) `CAPS = 0x7`, в конфигурации с двумя
  экранами (раздел «Display device, second screen (S10-15)» ниже) `CAPS = 0xF`. Один тип `DisplayDevice` обслуживает
  гостей обеих ревизий: регистры ревизии 1, их значения, порядок проверок,
  счётчики, backend-ошибки и host API раздела «Display device (S10-03)» не
  изменены; меняются только значения `VERSION` и `CAPS`. Новые регистры
  занимают бывший резерв `0x100–0x1FC` и `0x400–0x7FC`; `0x200–0x3FC`, `IRQ_*` и
  остальной резерв читаются как `0`, запись игнорируется.
- **Регистры.** 17 именованных (`DRIVER_PROTOCOLS`, `DRIVER_PROTOCOL`,
  `EVENT_STATUS`, `MODE_COUNT`, `EVENT_PENDING`, `EVENT_CAPACITY`, `EVENT_POP`,
  `EVENT_SEQ_LO/HI`, `EVENT_TYPE`, `EVENT_ARG0..3`, `RESPONSE_SEQ_LO/HI`,
  `REQUEST_RESPONSE`) и `MODE_TABLE`. Доступ — как в ревизии 1: только
  `Mmio`, `Dword`, выровненный; иначе `DeviceError` до изменения состояния.
  `DRIVER_PROTOCOL` — класс «RW с проверкой»: допустимое значение (`0` или `1`)
  применяется, недопустимое даёт `PROTOCOL_ERROR = UNSUPPORTED_PROTOCOL`.
- **Список режимов.** Константа устройства: `MODE_COUNT = 14`, записи по 16
  байт (`width`, `height`, `format = 1`, `0`), записи `i ≥ 14` читаются нулями.
  Чтение без побочных эффектов; reset список не меняет; `MODE_COMMIT` им не
  ограничен.
- **Очередь событий.** Ёмкость `DISPLAY_EVENT_CAPACITY = 16` записей
  (`seq: u64`, `type`, 4 аргумента). `EVENT_POP` берёт голову в защёлку; защёлку
  меняют только успешный `EVENT_POP` гостя, `DRIVER_PROTOCOL <- 0` и reset, host
  её не трогает. Пока `DRIVER_PROTOCOL = 0`, очередь пуста и `EVENT_PENDING = 0`.
  Ошибки `EVENT_STATUS` (`POP_ERROR`, `RESPONSE_ERROR`, `PROTOCOL_ERROR`) —
  `INVALID_COMMAND`, `NOT_ENABLED`, `QUEUE_EMPTY`, `STALE_REQUEST`,
  `MODE_MISMATCH`, `UNSUPPORTED_PROTOCOL` (`0x07–0x0B`); в `STATUS` они не
  попадают.
- **Host API** (`DisplayHandle`, без downcast): `driver_protocol()`,
  `post_mode_request(width, height, format) -> Result<u64, DisplayRequestError>`,
  `mode_request_state() -> Option<ModeRequestState>`. Проверки постановки по
  порядку: `NoDriver`, `InvalidMode` (проверки `MODE_COMMIT` 2–4),
  `SequenceExhausted`, `QueueFull`; при отказе состояние не меняется. Новый
  запрос удаляет непринятый `MODE_REQUEST` экрана 0 и вытесняет открытый;
  переполнение не сбрасывает старые события. Правило вызова — как у остального
  handle: только между квантами, владельцем `MachineBus`, в его потоке.
- **Ответ.** `REQUEST_RESPONSE` проверяет кодирование (`ACCEPT = 0x01`,
  `REJECT` с причиной `1..=3` в битах 15:8, биты 31:16 нулевые), затем
  `DRIVER_PROTOCOL`, затем открытый запрос с номером из `RESPONSE_SEQ_HI:LO`,
  затем для `ACCEPT` — равенство активного режима запрошенному (`MODE_MISMATCH`).
  Режим меняет только `MODE_COMMIT`.
- **Reset.** `DisplayHandle::reset()` дополнительно выключает протокол как
  `DRIVER_PROTOCOL <- 0` (открытый запрос → `Cancelled`), обнуляет
  `RESPONSE_SEQ_*` и `EVENT_STATUS`; `next_event_seq` и последний запрос
  сохраняются.
- **Не реализовано.** Прерывания, snapshot,
  путь запроса UI → VMHost → C ABI `od-host` (ABI в этом срезе не менялся;
  DISPLAY_DEVICE.md, 14.15.1), драйверы. Гостевой CPU-путь по-прежнему не имеет
  dword MMIO-записи (13.1), поэтому гостевой код не проверен: события и ответы
  проверены записями и чтениями через `MachineBus`.
- **Проверено.** Unit-тесты `display_device.rs` и интеграционные
  `tests/display_device.rs`: значения после reset регистров 14.5, неизменность
  при записи в RO и резерв, проба ревизии 1, таблица режимов (побайтное сравнение образа, собранного из dword-чтений; каждая
  запись проходит commit), `DRIVER_PROTOCOL`, порядок и защёлка очереди,
  объединение и вытеснение, ответы (`ACCEPT`/`REJECT`/`MODE_MISMATCH`/
  `STALE_REQUEST`, неверные кодирования, `seq` с разницей в `HI`), порядок
  отказов host API, `QueueFull`/`SequenceExhausted` через прямую подготовку
  состояния в тесте, reset, backend-ошибки на новых смещениях.

## Display device, cursor (S10-14)

Нормативно: [DISPLAY_DEVICE.md](DISPLAY_DEVICE.md), раздел 15. Здесь записаны
только границы, реализованные в `crates/od-machine/src/display_device.rs`. Код
устройства написан и ожидает ревью. Display host ABI ([DISPLAY_HOST_ABI_V1.md](DISPLAY_HOST_ABI_V1.md),
раздел 9), публикация в VMHost (frame transport v2) и наложение в UI реализованы в S10-14.

- **Блок.** Опциональный блок ревизии 2 по `CAPS` биту 2: `CAPS = 0x7`,
  `VERSION` остаётся `2`. Блок `0x200–0x2FC` существует только при этом бите
  (в коде проверка `CAPS & CAPS_CURSOR`); `0x22C` и `0x238–0x2FC` — резерв
  (чтение `0`, запись игнорируется), `0x300–0x3FC` по-прежнему резерв. Регистры
  версий 1 и 2, `MODE_COMMIT`, doorbell, очередь событий и `DRIVER_PROTOCOL`
  не изменены; запись вне блока состояния курсора не меняет (кроме reset).
- **Регистры** (13): `CURSOR_MAX_WIDTH/HEIGHT` (`64`), `CURSOR_FORMATS` (`0x4`),
  `CURSOR_STATUS` (RO; `DATA_ERROR` 7:0, `IMAGE_ERROR` 15:8, `POSITION_ERROR`
  23:16, `ENABLE_ERROR` 31:24), `CURSOR_WIDTH/HEIGHT/FORMAT/HOTSPOT` (RW, как
  записано, проверка при commit), `CURSOR_IMAGE_INDEX` (RW с автоинкрементом),
  `CURSOR_IMAGE_DATA`/`CURSOR_IMAGE_COMMIT` (WO), `CURSOR_POSITION` и
  `CURSOR_ENABLE` (RW с проверкой). Доступ — как у остального окна: только
  `Mmio`, `Dword`, выровненный; иначе `DeviceError` до изменения состояния, в
  том числе до автоинкремента индекса и записи в `staging`. Новые коды
  `0x0C–0x0F` (`INDEX_OUT_OF_RANGE`, `HOTSPOT_OUT_OF_IMAGE`,
  `POSITION_OUT_OF_FRAME`, `NO_CURSOR_IMAGE`); все ошибки исправимы гостем,
  в `STATUS`/`EVENT_STATUS` не попадают.
- **Образ.** От 1×1 до 64×64, формат `2` (`BGRA8888_PREMUL`, слово `0xAARRGGBB`,
  строки без паддинга). Буферы `staging` и `active` по 4096 слов (16 KiB)
  лежат в состоянии устройства: не в карте гостя, не во `Framebuffer`, запись не
  вызывает committed-write callback. `CURSOR_IMAGE_COMMIT` по проверкам 15.6.2
  копирует первые `W × H` слов `staging` в `active` одним действием под
  блокировкой состояния; `staging` после commit сохраняется. Пиксели не
  проверяются.
- **Счётчики** (только host): `cursor_seq` растёт на успешный commit, на
  изменившую значение запись `CURSOR_POSITION`/`CURSOR_ENABLE` и на reset, если
  было что показывать; `shape_seq ← cursor_seq` при commit. Не зависят от
  `generation` и `mode_change_count`; курсор их, `STATUS`, режим и память кадра
  не меняет, `MODE_COMMIT` и doorbell не меняют курсор. При `cursor_seq = 2^64 − 1`
  три команды дают `COUNTER_EXHAUSTED`; reset в этом состоянии скрывает курсор,
  но счётчик не растёт (допущение 15.9).
- **Host API** (`DisplayHandle`, без downcast, только между квантами, владельцем
  `MachineBus`): `cursor_state() -> CursorState` (`cursor_seq`, `enabled`, `x`,
  `y`, `shape: Option<CursorShapeInfo>` с `shape_seq`, `width`, `height`,
  `format`, `hot_x`, `hot_y`) и
  `read_cursor_image_into(&mut [u8]) -> Result<CursorShapeInfo, CursorReadError>`
  (`NoShape`, `BufferLength`; копирует ровно `W × H × 4` байт `active`, порядок
  B, G, R, A; параметры и байты берутся под одной блокировкой). Шина не нужна.
  Обе функции только читают.
- **Reset.** `DisplayHandle::reset()` возвращает регистры блока к значениям
  после reset, обнуляет `staging` и `active`, снимает форму (`shape_seq = 0`),
  скрывает курсор и ставит позицию `(0, 0)`; `cursor_seq` растёт на 1, если была
  форма, курсор был включён или позиция была ненулевой.
- **Не реализовано.** Test-only профиль `3` (отклонён, DISPLAY_HOST_ABI_V1.md,
  правило 0), snapshot, драйверные последовательности курсора
  (DISPLAY_DRIVER_V1.md), гостевой CPU-путь с dword MMIO (13.1): регистры
  проверены записями через `MachineBus`.
- **Проверено.** Unit-тесты `display_device.rs` и интеграционные
  `tests/display_device.rs`: значения после reset блока, неизменность при записи
  в RO и резерв, RW-регистры и WO-чтение `0`, автоинкремент и диапазон
  `IMAGE_INDEX`, каждая проверка commit по порядку и граничные размеры,
  атомарность commit и сохранение `staging`, позиция против активного кадра,
  `CURSOR_ENABLE`, неизменность кадра при 1 000 перемещений, reset,
  насыщение счётчика, backend-ошибки. Замер: загрузка 64×64 (4 102 записи через
  `MachineBus`) — 56–160 мкс в release и около 0,9 мс в debug; один прогон с eprintln, запуск `cargo test -p od-machine --release --offline -- --nocapture` (одна машина, без
  CPU-пути; в CPU-пути не измерено).

## Display device, second screen (S10-15)

Нормативно: [DISPLAY_DEVICE.md](DISPLAY_DEVICE.md), раздел 16; передача в UI —
[FRAME_TRANSPORT_V1.md](../../protocols/control/FRAME_TRANSPORT_V1.md), раздел 11;
display host ABI — [DISPLAY_HOST_ABI_V1.md](DISPLAY_HOST_ABI_V1.md), раздел 10;
последовательность драйвера — [DISPLAY_DRIVER_V1.md](../../protocols/guest/DISPLAY_DRIVER_V1.md),
раздел 10a. Здесь записаны только границы, реализованные в
`crates/od-machine/src/display_device.rs`, `physical_memory.rs` и `bus.rs`. Код
написан и ожидает ревью.

- **Конфигурация.** Число экранов задаёт конструктор при создании устройства и не
  меняется: `DisplayDevice::new_with_handle()` — один экран (`CAPS = 0x7`, блок
  `0x300–0x3FC` целиком резерв: чтение `0`, запись игнорируется),
  `DisplayDevice::new_two_screens_with_handle()` — два (`CAPS = 0xF`, бит 3
  `SECOND_SCREEN`). `VERSION` остаётся `2` (толкование 14.3 вынесено на ревью,
  DISPLAY_DEVICE.md 16.15 п. 1). Одно дисплейное устройство на шину:
  `DisplayBindingError::MultipleDevices` не менялась.
- **Блок экрана 1.** 17 именованных регистров (`S1_MAX_WIDTH/HEIGHT`, `S1_FORMATS`,
  `S1_FB_BASE_LO/HI`, `S1_FB_SIZE`, `S1_WIDTH/HEIGHT/FORMAT`, `S1_MODE_COMMIT`,
  `S1_ACTIVE_WIDTH/HEIGHT/FORMAT`, `S1_STRIDE`, `S1_STATUS`,
  `S1_FRAME_GENERATION`, `S1_FRAME_GENERATION_HI`) по смещению `0x300 +` смещение
  одноимённого регистра экрана 0 и 47 резервных слов; `ID`, `VERSION` и `CAPS` не
  зеркалируются. Доступ — как у остального окна: только `Mmio`, `Dword`, выровненный,
  иначе `DeviceError` до изменения состояния. `S1_MODE_COMMIT` принимает `1`
  (режим, проверки раздела 5 над регистрами экрана 1) и `2` (отключить экран,
  16.6); другое значение — `INVALID_COMMAND` в `S1_STATUS`. `S1_STATUS`: `ACTIVE`
  (бит 0), `FRAME_SINCE_COMMIT` (1), `DISABLED` (2), `COMMIT_ERROR` (15:8),
  `FRAME_ERROR` (23:16); новых кодов нет, ошибки исправимы гостем и не попадают в
  `STATUS`, `EVENT_STATUS`, `CURSOR_STATUS`. Экран 0 командой `2` не отключается.
- **Состояния и счётчики.** `NO_MODE`, `ACTIVE`, `DISABLED`; свои `generation`,
  `mode_change_count`, `frame_mode_epoch`. Отключение (в том числе из `NO_MODE`)
  растит `mode_change_count_1`; повтор отключения — успех без роста; doorbell на
  отключённом экране — `FRAME_ERROR = NO_ACTIVE_MODE`. При `2^64 − 1` команды
  `1` и `2` (кроме повторного отключения) дают `COUNTER_EXHAUSTED`, как и doorbell
  при `generation_1 = 2^64 − 1`. Запись в блок не меняет экран 0, его `STATUS`,
  очередь событий, `DRIVER_PROTOCOL` и курсор; запись в регистры экрана 0 не меняет
  экран 1.
- **Память.** Второй регион `Framebuffer` `[0xF400_0000, 0xF640_0000)` (36 MiB),
  резерв `[0xF640_0000, 0xF800_0000)` не отображается. Правила 7.1–7.4 те же, что у
  экрана 0: данные как RAM, без committed-write callback и page generations,
  instruction fetch и элементы таблиц страниц в регионе — backend-ошибки. Backing
  двух регионов (72 MiB) вне `RamLimits`. Курсор к региону отношения не имеет.
- **Карта и шина.** `MachineMemoryMap::new` принимает не больше двух регионов
  `Framebuffer` (`MAX_FRAMEBUFFER_REGIONS = 2`; третий —
  `MemoryAccessError::MultipleFramebufferRegions`; каждый регион проверяется как
  раньше, `InvalidFramebufferRegion`). Устройство отдаёт шине привязки через
  `BusDevice::framebuffer_binding()` (экран 0) и `secondary_framebuffer_binding()`
  (экран 1, `None` в конфигурации с одним экраном); множество регионов `Framebuffer`
  карты должно совпасть с ними по базе и длине: иначе
  `DisplayBindingError::FramebufferMismatch` (нет региона экрана 1, другая база или
  длина) или `UnboundFramebufferRegion` (лишний регион у устройства с одним экраном).
- **Reset.** `DisplayHandle::reset()` возвращает регистры блока к значениям после
  reset и состояние экрана 1 к `NO_MODE`; `mode_change_count_1` растёт на 1, только
  если был режим или `DISABLED` (при `2^64 − 1` не меняется); `generation_1`,
  `frame_mode_epoch_1` и память обоих экранов сохраняются.
- **Host API** (`DisplayHandle`, только между квантами, владельцем `MachineBus`, в его
  потоке; только читает): `second_screen_state() -> Option<SecondScreenState>`
  (`None` — один экран; число экранов `1 + u32::from(is_some())`; режим, `disabled`,
  `generation`, `mode_change_count`, `frame_mode_epoch`, `has_current_frame()`) и
  `read_second_frame_into(&bus, &mut [u8]) -> Result<FrameInfo, DisplayReadError>` с
  правилами `read_frame_into` и отказом `DisplayReadError::NoSuchScreen`, когда
  второго экрана нет (отдельно от `NoActiveMode`: экран есть, режима нет).
  Функции без номера экрана описывают экран 0.
- **Не реализовано.** Третий экран, курсор и события экрана 1, PCI-BAR (S10-12),
  snapshot; гостевой код: probe S10-08 рисует один экран (проверка 16.13 п. 10 с
  probe — S10-09), регистры экрана 1 проверены записями через `MachineBus`.
- **Проверено.** Unit-тесты `display_device.rs` (`cargo test -p od-machine --offline
  --lib display_device`, 65 тестов, из них 15 в `screen1_tests`): значения после
  reset 17 регистров и неизменность при записи `0xFFFF_FFFF` в RO-регистры и в
  каждое из 47 резервных слов (в конфигурации с одним экраном — во все 64 слова);
  `S1_MODE_COMMIT` и отключение по состояниям, независимость поколений и счётчиков
  двух экранов; `COUNTER_EXHAUSTED` (commit, disable, doorbell) и порядок проверок;
  reset двух экранов (счётчики, поколения, память); backend-ошибки на смещениях
  блока; правила 7.1–7.4 для региона экрана 1; построение карты и шины (72 MiB при
  `RamLimits` 1 MiB, третий регион, регион `0x0240_1000`, перекрытия с экраном 0,
  окном регистров и RAM, нет региона или другая база экрана 1,
  `UnboundFramebufferRegion`); `NoSuchScreen`. Не запускались: интеграционные тесты
  `tests/display_device.rs`, полный набор `od-machine`.

## Stage 4 flat probe image and direct handoff (S4-32a)

`Stage4FlatProbeV1` is a bounded, project-owned freestanding image contract.
The build fixture produces an exact `0x9000`-byte raw image from assembly and a
linker layout; the loader copies those bytes verbatim at GPA `0x1000` and does
not synthesize instructions, page tables or expected serial output. It accepts
only the fixed PML4/PDPT/PD and three 4 KiB page-table pages, code mapping at
GVA `0x400000` → GPA `0x7000`, payload mapping at GVA `0x800000` → GPA
`0x9000`, and NX stack mapping at GVA `0xa00000` → GPA `0xa000`.

The direct handoff is Long64/CPL0 with `CR0=0x80010033`, `CR4=0x20`,
`CR3=0x1000`, `EFER=0xd00`, `RIP=0x400000`, `RBX=0x800000`,
`RSP=0xa01000` and `DX=0x3f8`. The accepted fixture executes exactly
`MOV RAX,[RBX]`, `OUT DX,AL`, `HLT`; the runner invokes the existing bounded
paged quantum and reports the UART byte actually produced by the guest.
Budget exhaustion, loader rejection and I/O failures remain host-side result
categories; they are not converted into a success marker or guest exception.

The loader validates the complete present-entry set in the six table pages and
rejects aliases or unexpected mappings. The stack page is allocated as part of
the fixed RAM profile but is outside the raw image. This is a local interpreter
and runner acceptance slice, not a Linux boot protocol, general kernel loader,
exception-delivery path, IRQ/APIC path or physical-hardware oracle. See the
[S4-32a report](../reports/STAGE_4_S4-32A_2026-11-05.md) and
[verification record](../testing/MACHINE.md#s4-32a-reproducible-flat-probe).

## Stage 4 store/load probe image (S4-32b)

`Stage4StoreLoadProbeV1` preserves the S4-32a image size, page tables, code/data
placement, Long64/CPL0 mode and COM1 binding. Its code page contains exactly
`48 89 0B 48 8B 03 EE F4`: `MOV [RBX],RCX`, `MOV RAX,[RBX]`, `OUT DX,AL`,
`HLT`. The data page contains two artifact-owned qwords at GPA `0x9000` and
`0x9008`: the initial value and the value that the loader copies into `RCX` for
the documented handoff. The loader rejects equal qwords, validates the exact
page-table/code layout and does not generate the store value or serial output.

Successful execution retires four instructions, changes only the target data
qword among guest RAM data, leaves `RAX` equal to the handed-off store value,
emits its low byte through the bounded UART and enters `Halted` at
`RIP=0x400008`. The `rflags_defined` mask remains `0x0cd7`. Budgets below four
expose the committed prefix: the store is visible after one retirement, the
load result after two and UART output after three. These are bounded quantum
observations, not exception delivery or rollback semantics.

The profile is separate from `Stage4FlatProbeV1`; its runner subcommand is
`--stage4-store-load-probe`. It reuses `run_paged_probe` and the existing
machine loader boundary rather than dispatching guest instructions in the
runner. See the [S4-32b report](../reports/STAGE_4_S4-32B_2026-11-05.md) and
[verification record](../testing/MACHINE.md#s4-32b-freestanding-storeload-probe).

## Диагностический CPU memory bridge

`IdentityMemoryAdapter` реализует `od_isa::GuestMemory` поверх существующего
`MachineBus` в явно названном flat identity mode: `linear == physical`. Он
предназначен для проверки подключения интерпретатора к production RAM owner и
не заявляет paging, TLB, CR3, CR2, A/D effects или полную Long64 machine.

Adapter использует `AccessContext::page_privilege()`, сохраняет complete-or-no-
change для одного RAM access, отклоняет fetch из MMIO и неподдержанные atomic/
fence операции до эффектов. Ошибки machine/bus возвращаются как
non-architectural `MemoryError::Backend`; adapter не создаёт `#PF`, не доставляет
исключения и не хранит вторую карту памяти. Integration regression запускает
существующий `od_isa::execute::run` через этот adapter и проверяет успешные
fetch/load/store, faulting load, RIP/CPU state, retirement, RAM и callback.

## Следующие зависимости

## Диагностический uncached translation probe

`probe_long64_4k` читает вручную заданный snapshot `cr0/cr3/cr4/efer` и четыре
уровня page tables из `PhysicalMemory`. Поддержанный probe scope — canonical48,
physical36, four-level Long64 и 4 KiB pages без TLB, cache, huge pages, A/D
updates или чтения конечного операнда. Конфигурация отклоняется fail-closed,
включая `CR0.ET == 0`, неподдержанные control bits и недопустимый CR3.

Результат разделяет `Translated` и диагностические причины: noncanonical
address, unsupported page size/configuration, table read failure, nonpresent,
reserved-bit и protection outcomes. Trace содержит только реально прочитанные
entries; canonicality проверяется до чтения таблиц. PFEC — literal поле probe,
а не доставка vector 14, запись CR2 или выбор `#GP/#SS`. Эффективные US/RW/XD,
WP и NXE проверяются в заявленном профиле, но A/D bytes остаются неизменными.

Этот pure probe сохраняет диагностическую семантику: он не меняет A/D и не
подключён к CPU execution path. Для `GuestMemory` по-прежнему действует
отдельный linear-address contract; adapter S4-06 остаётся явно flat identity
mode.

## Bounded page-fault record construction

`od-isa` владеет `PageFaultRecord` и общей функцией `page_fault_error_code`.
Запись содержит vector 14, обязательные error code и linear address,
сохранённый RIP инструкции, `AccessKind` и полный `AccessContext`. Проверка
PFEC использует независимые literal cases; для descriptor-table origin CPL
сохраняется, а paging privilege остаётся supervisor.

`od-machine::probe_instruction_access` оборачивает существующий чистый probe:
перевод остаётся переводом, а только `ProbeOutcome::PageFault` становится
`PageFaultRecord`. Machine outcome сохраняет `PageFaultReason` и trace, включая
уровень таблицы и offending bits. Noncanonical, unsupported page size и ошибки
чтения таблиц остаются отдельными `ProbeError` и не создают запись.

`saved_rip` и `cr2_value()` имеют разные значения: второе — адрес, предназначенный
для будущей записи в CR2. Создание записи не доставляет исключение, не меняет
CR2 или `LongModeState`, не повторяет инструкцию и не меняет RAM/A-D bits.
Cross-page fetch, IDT и exception delivery остаются вне этого bounded среза.

## Tracked RAM access

`access_long64_4k_ram` выполняет одну операцию Fetch/Read/Write шириной 1, 2, 4
или 8 байт по текущему `Long64ProbeConfig` и полному `AccessContext`. Тип
`AccessKind` выводится из операции. До обхода отвергаются неканонический адрес и
операнд, пересекающий 4 KiB границу.

Page-table entries читаются напрямую из `PhysicalMemory`; bus/device dispatch,
TLB и конечные не-RAM области не используются. После проверки каждой present
entry на reserved/unsupported формат отсутствующий A устанавливается немедленно
через тот же `write_u64`/committed-write путь, прежде чем обход продолжится.
Поэтому уже совершённые A writes остаются после нижнего nonpresent/reserved
fault, protection fault или последующей ошибки чтения таблицы.

После всех четырёх уровней проверяются накопленные права через общий
`page_fault_error_code` helper. Весь конечный физический operand сначала
проверяется как RAM. Только для разрешённой Write операции устанавливается
отсутствующий leaf D, после чего записываются operand bytes. Ошибка не вызывает
rollback ранее committed A/D writes. `TrackedAccessReport` содержит trace
исходно прочитанных entries и журнал фактически committed A/D updates. Callback
получает notifications каждого committed write, включая конечный operand, в
порядке совершения; notifications operand не копируются автоматически в report.
Уже установленный A/D bit не создаёт лишнюю запись/callback.

Для ROM и reserved конечная Write возвращает `ReadOnlyRegion`; чтение/выборка
сохраняют `UnreadableRegion`. Конечный MMIO access не dispatch-ится этим
RAM-only API: Write возвращает `DeviceUnavailable`, а Read/Fetch сохраняют
`UnreadableRegion(Mmio)`. Во всех таких отказах уже committed A updates остаются,
D не устанавливается и callback не получает notification для operand.

Результат разделяет `PageFaultRecord`, ошибки чтения/записи таблицы и ошибки
физического операнда. Создание записи не меняет CPU, RIP или CR2. Этот bounded
API не реализует `GuestMemory`, cross-page операции, huge pages, TLB,
исключение-доставку или SMP/DMA coherence; pure probe выше остаётся без A/D
эффектов.

#### Injected table-write failure ordering (S4-09b)

The private tracked-walker test seam supplies a table-entry writer. Production
calls still delegate each A/D update to `PhysicalMemory::write_u64`; there is no
public or general fault-injection API. Injected A-write failures at PML4, PDPT,
PD and PT prove that only earlier successful writes remain committed. The
failing entry and all later entries stay unchanged, and the error names the
exact level, entry address and A/D bit. `TrackedAccessReport` includes only
committed `AdUpdate` values; callbacks contain only successfully committed
entry writes, in commit order.

A separate injected D failure after all four A updates preserves that A prefix,
leaves the leaf D bit and operand unchanged, and emits no D or operand callback.
The normal `PhysicalMemory` writer tests separately verify that an ordinary
rejected write leaves its complete range unchanged and invokes no callback.
Callback panic remains a host panic after the write has committed. The seam does
not add whole-operation rollback, cross-page or MMIO/guest-store behavior,
exception delivery, or cached A/D semantics; full S4-09 remains open.

<a id="tracked-permission-level-matrix-s4-09c"></a>

#### Tracked permission-level matrix (S4-09c)

The existing tracked Long64 four-level/4 KiB RAM path is covered at each PML4,
PDPT, PD and PT level with all other entries permissive. Tests exercise
non-present Read/Write/Fetch for CPL0 and CPL3 with literal PFEC values; a
faulting non-present/reserved entry and lower entries remain unchanged while
only eligible predecessor A updates and callbacks remain committed. Protection
checks occur after the four-entry walk, so A may already be set on the denied
entry; the full eligible A prefix remains committed. They compare the complete
`PageFaultRecord`, all five `ad_updates` slots, complete page-table entry values,
callback order and unchanged operand bytes.

The permission matrix clears U/S or R/W independently at every level. CPL3
Read/Write/Fetch faults on U/S denial; CPL0, CPL1 and CPL2 retain supervisor
access where allowed. Read and Fetch remain allowed through read-only mappings.
Supervisor Write through R/W=0 succeeds at WP=0 and sets A on the walk, then D,
then exactly the operand; WP=1 faults with PFEC 3. CPL3 Write faults with PFEC 7
at either WP setting. CPL1/CPL2 follow supervisor WP behavior. NX at each level
allows data Read/Write with NXE, denies Fetch for CPL0/CPL1/CPL2/CPL3 with the
expected PFEC, and is a reserved-bit fault when NXE is clear; in that case only
predecessor A updates occur because reserved-bit validation precedes A on the
faulting entry. Preset A/D entries generate no redundant table callbacks.

A separate pure-probe check covers Read/Write/Fetch for CPL0/CPL3 and confirms
that no page-table bytes change. This slice adds tests only: it does not change
walker or public CPU behavior, does not deliver #PF or update CR2, and does not
cover CPU execution integration, system acceptance, cross-page accesses,
additional paging modes, exception delivery or cached A/D behavior. See the
[S4-09c report](../reports/STAGE_4_S4-09C_2026-11-06.md).

<a id="ad-summary-contract-s4-09d"></a>

#### Сводный контракт A/D страниц (S4-09d, S4-09e)

Это единая таблица effects A/D-битов page-table entries по путям доступа и исходам.
Она сводит S4-09b/c и последующие срезы (2 MiB, межстраничные доступы, MMIO, LTR,
доставка `#PF`, кэш выборки) и ничего не меняет в коде; поведение определено кодом и
перечисленными тестами. Если ячейка не покрыта тестом, это записано явно.

**Решения, зафиксированные таблицей.**

- *A на denied entry.* Проверка прав идёт после обхода всех уровней, а отсутствующий A
  ставится сразу при прохождении entry (после проверки present/reserved). Поэтому при
  protection fault A стоит на всех уровнях обхода (четыре для 4 KiB, три для 2 MiB),
  включая entry, запретившую доступ; D не ставится. Это принятая семантика S4-09c, а не
  случайность реализации; соответствие конкретному CPU не заявляется. При nonpresent и
  reserved fault A ставится только у предшественников: сама entry и нижние уровни не
  меняются, потому что reserved-проверка предшествует записи A.
- *D только на листе и только для write.* D ставится на листе (PTE, либо PDE 2 MiB) после
  успешных прав и проверки физической цели, до записи операнда; read и fetch D не ставят.
  Уже установленный A/D не создаёт ни записи, ни callback.
- *Одно место коммита D (S4-09e).* D коммитится только helper-ом
  `commit_leaf_dirty` (`crates/od-machine/src/page_walk.rs`) через seam `table_writer`; им
  пользуются и tracked access, и транзакция LTR
  (`commit_paged_ltr_descriptor_with_writer`; до S4-09e LTR писал D прямым
  `PhysicalMemory::write_u64`). Записи A идут через тот же `table_writer` из
  `walk_long64_4k_with_writer`; других мест записи A/D в production-коде `od-machine` нет.
  Оговорка: это утверждение держится на ручном поиске и структурном grep-тесте
  `leaf_dirty_is_committed_only_by_the_helper`, который не ловит запись D в обход helper
  через `| 0x40` или новую константу без `AdBit::Dirty`; обход seam в LTR ловят
  поведенческие тесты (6.1, 6.5).
- *Отказ записи entry.* Ошибка `table_writer` возвращается как
  `TrackedAccessError::TableWrite { level, entry_address, bit, source }`; ранее
  committed A остаются, отказавший бит, операнд и дальнейшие callbacks отсутствуют.
- *Атомарность A/D не входит в S4-09; закрыта в S6-25.* До S6-25 каждая запись entry была одним 8-байтовым неатомарным `write_u64`. С S6-25 запись A и D — один compare-and-replace (раздел «S6-25» ниже), повтор обхода при изменении записи реализован; гонка с DMA и host-потоками остаётся за S6-30/S6-31. Целевое правило — в `MEMORY_MODEL.md`, строка «Page walk и A/D bits» (стр. 28). Страницы 1 GiB не заявляются (PDPTE.PS=1 даёт гостевой reserved
  `#PF`, S4-10c); полноценный 1 GiB остаётся за S4-10.
- Бит accessed у дескрипторов GDT/IDT/TSS (байт 5 дескриптора) — другой класс side
  effects и в таблицу не входит (`MEMORY_MODEL.md`, «Дескрипторные side effects»).
- Поиск по `AdBit::`, маскам `1 << 5`/`1 << 6` и `write_u64` в production-коде крейтов
  `crates/*/src` не нашёл других мест записи A/D page-table entries, кроме
  `walk_long64_4k_with_writer` (A), `commit_leaf_dirty` (D) и их writer-а
  `write_tracked_table_entry`.

**Обозначения.** «A: 4» — A установлен на PML4E, PDPTE, PDE и PTE; «A: 3» — на PML4E,
PDPTE и PDE 2 MiB; «A: предш.» — только на уровнях выше первой отсутствующей/reserved
entry. «cb» — callbacks `committed_write` в порядке коммита. Колонка «Тест» называет
существующий тест (`файл::имя`; без префикса — тест в том же файле, что и предыдущий;
`page_walk` = `crates/od-machine/src/page_walk.rs`, модуль `tracked_access_tests`).
Пометка «покрыто чтением» означает: effect следует из кода (ссылка в колонке «Примечание»),
но отдельного теста на биты таблицы нет. Пометка «частично» — тест проверяет только часть
ячейки (указано, какую).

| № | Путь | Исход | A по уровням | D листа | Операнд | cb | Тест |
|---|---|---|---|---|---|---|---|
| 1.1 | 4 KiB, tracked access | nonpresent | A: предш. | нет | не тронут | A предш. | `page_walk::tracked_nonpresent_matrix_covers_every_level_access_and_cpl`; `nonpresent_reserved_and_protection_faults_keep_prior_accessed_prefix`; покрыто системно: Сист-1, шаг s20 (PTE nonpresent под свежей PDE: A только на PDE, PTE не тронут, D нет) |
| 1.2 | 4 KiB | reserved | A: предш. | нет | не тронут | A предш. | `page_walk::tracked_nx_matrix_checks_fetch_faults_reserved_bits_and_data_access` (NXE=0); `nonpresent_reserved_and_protection_faults_keep_prior_accessed_prefix` |
| 1.3 | 4 KiB | protection (U/S, R/W+WP, NX) | A: 4, включая denied | нет | не тронут | 4 A | `page_walk::tracked_user_supervisor_matrix_accumulates_us_at_every_level`; `tracked_read_write_permissions_cover_every_level_and_wp_cpl_combination`; `tracked_nx_matrix_checks_fetch_faults_reserved_bits_and_data_access`; покрыто системно: Сист-2 (R/W=0 на PTE и на PDE при WP=1: A на запрещающей entry и ниже, D нет, операнд не записан) |
| 1.4 | 4 KiB | success read / fetch | A: 4 | нет | прочитан | 4 A | `page_walk::successful_read_and_fetch_commit_only_accessed_bits`; покрыто системно: Сист-1, шаги s1 (A на четырёх уровнях, D нет) и s26 (повтор после починки) |
| 1.5 | 4 KiB | success write | A: 4 | да, на PTE, до операнда | записан после D | 4 A, D, операнд | `page_walk::write_commits_accessed_dirty_then_operand_in_order`; `pre_set_accessed_and_dirty_bits_do_not_create_extra_commits`; `preset_accessed_and_dirty_bits_do_not_emit_redundant_table_callbacks`; покрыто системно: Сист-1, шаги s2 (D после чтения), s3 и s10 (первый доступ записью; s10 с A на всех четырёх уровнях) |
| 1.6 | 4 KiB | отказ записи entry (A либо D) | A-префикс до отказавшего остаётся | при отказе D не ставится | не тронут | только успешные | `page_walk::accessed_write_failures_preserve_committed_prefix_at_each_level`; `fetch_accessed_write_failures_are_tracked_before_physical_fetch`; `dirty_write_failure_preserves_accessed_prefix_without_operand_commit` |
| 1.7 | 4 KiB | ошибка физической цели (ROM, reserved, дыра) | A: 4 | нет | не тронут | 4 A | `page_walk::writes_to_rom_and_reserved_are_write_errors_after_a_without_dirty_or_operand_commit`; `table_read_and_operand_failures_are_physical_errors_with_committed_prefix` |
| 2.1 | 2 MiB (`access_long64_2m_ram`) | nonpresent | A: предш. | нет | не тронут | A предш. | частично: `linux_direct::tests::cpl0_set_board_byte_store_to_an_unmapped_page_is_the_exact_page_fault` (таблицы равны состоянию после одной выборки, D нет; A предш. уже стоят от выборки, поэтому их коммит самим тестом не различается); `crates/od-machine/tests/paged_mov_step.rs::two_mib_ram_profile_loads_stores_sets_pde_ad_and_faults_precisely` (nonpresent PDE: точная запись `#PF`, биты не проверяются) |
| 2.2 | 2 MiB | reserved (в т.ч. PDPTE.PS) | A: предш. | нет | не тронут | A предш. | `page_walk::large_leaf_reserved_bits_fault_keeps_ancestor_accessed_prefix`; `s4_10c_pdpte_ps_is_guest_rsvd_fault_with_a_only_on_pml4e` |
| 2.3 | 2 MiB | protection | A: 3, включая denied | нет | не тронут | 3 A | `page_walk::large_leaf_aggregates_user_write_and_nx_rights_from_ancestors` (`ad_update_count == 3`) |
| 2.4 | 2 MiB | success read | A: 3 | нет | прочитан | 3 A | `page_walk::large_leaf_offsets_dirty_and_legacy_admission`; `crates/od-machine/tests/paged_mov_step.rs::two_mib_ram_profile_loads_stores_sets_pde_ad_and_faults_precisely` («load sets A only»); покрыто системно: Сист-1, шаг s16 (A на PDE, D нет) и s28 (выборка из 2 MiB-страницы) |
| 2.5 | 2 MiB | success write | A: 3 | да, на PDE | записан после D | A, D, операнд | `page_walk::first_write_through_fresh_large_leaf_sets_a_then_d_before_operand`; `large_leaf_offsets_dirty_and_legacy_admission`; покрыто системно: Сист-1, шаг s14 (A на трёх уровнях, D на PDE, затем операнд) |
| 2.6 | 2 MiB | отказ записи PDE (A либо D) | A-префикс остаётся | не ставится | не тронут | A предш. | `page_walk::large_leaf_permissions_and_table_write_failures` |
| 3.1 | межстраничное чтение/выборка (две страницы, `access_cross_page_read`) | success | A: 4 (или 3) на обеих страницах, walk второй после первой | нет | собран из двух частей | A обеих страниц | `linux_direct::tests::cpl0_set_board_qword_and_dword_reads_across_a_page_boundary_for_every_split`; `paged_mov_step.rs::s4_11c_cross_page_alias_samples_after_both_walks_commit_accessed_bits` |
| 3.2 | межстраничное чтение | fault/отказ на второй странице | A первой страницы и предш. второй остаются | нет | буфер вызывающего не тронут | A до отказа | `linux_direct::tests::cpl0_set_board_cross_page_read_with_a_missing_second_page_is_a_precise_page_fault`; `paged_mov_step.rs::second_cross_page_walk_failures_preserve_cpu_and_keep_prior_a_updates`; `paged_memory_adapter::tests::failed_second_cross_page_walk_keeps_caller_buffer_unchanged` |
| 3.3 | межстраничный store (`write_crossing_page`): preflight A без D по обеим страницам, затем побайтные записи с D | success | A: полный обход обеих страниц (preflight) | да, на листе каждой страницы (при одном 2 MiB-листе — один раз на PDE) | записан по частям | A, A, затем D+байт по порядку записей | `linux_direct::tests::cpl0_set_board_qword_store_across_a_page_boundary_writes_both_pages_for_every_split`; `cpl0_set_board_word_and_dword_stores_across_a_page_boundary_on_the_split_tree`; `cpl0_set_board_store_to_a_non_executable_second_page_succeeds`; `cpl0_set_board_word_and_dword_stores_crossing_a_4k_boundary_store_both_pieces`; покрыто системно: Сист-1, шаг s8 (A, A, затем D первой страницы и её байты, D второй страницы и её байты) |
| 3.4 | межстраничный store | fault на второй странице (nonpresent или protection) | A первой страницы; у второй — предш. (nonpresent) либо все уровни (protection) | нет ни на одной странице | ни одного байта | A preflight | `linux_direct::tests::cpl0_set_board_store_with_a_missing_second_page_writes_no_byte_and_faults_precisely`; `cpl0_set_board_store_to_a_read_only_second_page_is_a_present_write_fault_and_no_byte`; `paged_memory_adapter::tests::cross_page_write_preflight_keeps_failed_walk_without_dirty_or_operand_write`; покрыто системно: Сист-3 (вторая страница nonpresent либо read-only при WP=1: A у первой страницы, нет D, нет байтов) |
| 3.5 | межстраничный store | fault на первой странице | A у предш./по протоколу 1.1-1.3 первой страницы; вторая не обходится | нет | ни одного байта | A первой | частично: `linux_direct::tests::cpl0_set_board_store_with_a_faulting_first_page_reports_the_store_address` (`#PF`, CPU, байты обеих страниц и PTE второй страницы не тронуты; тест не проверяет биты A/D первой страницы) |
| 3.6 | 4 KiB tracked API (`access_long64_4k_ram`), операнд через границу | `CrossesPage` | не ставится (отказ до обхода) | нет | не тронут | нет | `page_walk::cross_page_rejects_before_any_walk_or_commit_and_probe_stays_pure`; `crossing_read_write_width_matrix_is_an_exact_no_effect_rejection` |
| 4.1 | RMW (чтение, затем запись того же адреса через `GuestMemory`) | success | A при чтении; запись A повторно не пишет | да, при записи | записан | A, D, операнд | до S4-09f не покрыто тестом (покрыто чтением: `read` и `write` идут теми же tracked-путями 1.4 и 1.5; `linux_direct::tests::cpl0_set_board_dword_read_modify_write_stores_exactly_one_dword` проверяет только ширину записи данных, не биты таблицы); покрыто системно (биты таблицы): Сист-1, шаги s4 (4 KiB) и s18 (2 MiB): запись A при чтении, запись D при записи (две записи entry), затем операнд |
| 4.2 | RMW | fault на чтении или записи | как 1.1-1.3 | нет | не тронут | как 1.1-1.3 | не покрыто тестом (покрыто чтением, пути 1.1-1.3 и 1.5) |
| 4.3 | `LOCK`-формы (CMPXCHG, XADD, ADD/OR/AND/INC/DEC/BTS/BTR m) | success, в том числе неравное сравнение CMPXCHG (значение пишется обратно) | как 4.1 | как 4.1 | как 4.1 | как 4.1 | до S4-09f не покрыто тестом (покрыто чтением: `lock_*` тесты в `linux_direct` проверяют данные и флаги, не биты таблицы); атомарность `LOCK` относительно других vCPU и DMA не моделируется (S6-31); покрыто системно (биты таблицы): Сист-1, шаг s5 (`LOCK ADD m64, r64`): как 4.1 |
| 5.1 | MMIO served read | success | A: 4 | нет | чтение у устройства, RAM нет | A, затем dispatch read | `crates/od-machine/tests/paged_mov_step.rs::paged_mov_mmio_read_dispatches_once_and_commits_destination` |
| 5.2 | MMIO served write | success | A: 4 | да, до dispatch | запись у устройства | A, D, dispatch write | `paged_mov_step.rs::paged_mov_mmio_write_dispatches_once_after_a_and_d_updates`; `linux_direct::tests::an_ecam_store_sets_the_dirty_bit_first_and_commits_one_table_write`; покрыто системно: Сист-1, шаг s12 (dword store в окно ECAM через 4 KiB-лист: A, D, без записи операнда) |
| 5.3 | MMIO served write | ошибка backend устройства | A: 4 остаются | остаётся (ставится до dispatch, не откатывается) | устройство отказало, CPU не меняется, retire нет | A, D, затем dispatch | `paged_mov_step.rs::mmio_device_failure_is_backend_error_with_no_retirement_after_dirty_commit` |
| 5.4 | MMIO unserved (нет dispatch или вне окон набора) | `DeviceUnavailable` (write) / `UnreadableRegion(Mmio)` (read) | A остаётся | нет | устройство не вызывается | только A | `linux_direct::tests::the_set_loop_serves_ecam_and_no_other_mmio`; `a_split_access_never_reaches_the_ecam_device`; `page_walk::mmio_write_is_device_unavailable_without_dispatch_or_operand_notification` |
| 5.5 | MMIO | protection | A: 4, включая denied (в журнале теста A стоит и на запрещающей PTE) | нет | устройство не вызывается | A | `paged_mov_step.rs::mmio_permission_fault_never_calls_device` |
| 6.1 | LTR (`commit_paged_ltr_descriptor_with_writer`) | success | A: 4 (walk чтения; walk проверки записи ничего не добавляет) | да, на PTE через `commit_leaf_dirty`, до образа дескриптора | busy-образ дескриптора, 16 байт | A, D, образ | `crates/od-machine/tests/paged_ltr_step.rs::ltr_ax_loads_tr_marks_descriptor_busy_and_dirties_leaf_in_order`; `ltr_ax_with_dirty_leaf_writes_only_the_descriptor`; `page_walk::ltr_table_entry_writes_go_through_the_writer_seam` |
| 6.2 | LTR | nonpresent страницы GDT | A: предш. | нет | не тронут | A предш. | частично: `paged_ltr_step.rs::ltr_ax_gdt_page_faults_are_precise_and_do_not_load_tr` (точный `#PF`, TR и CPU не меняются; биты A не проверяются) |
| 6.3 | LTR | protection (GDT read-only при WP=1: walk чтения проходит, walk записи отказывает) | A: 4, включая denied (их ставит walk чтения) | нет | дескриптор не тронут | A walk чтения | частично: `paged_ltr_step.rs::ltr_ax_gdt_page_faults_are_precise_and_do_not_load_tr` (первая половина: код ошибки 3, CR2, дескриптор не тронут; биты A не проверяются) |
| 6.4 | LTR | дескриптор невалиден (`#GP`/`#NP`) | A: 4 (walk чтения) | нет | дескриптор не тронут | A | частично: `paged_ltr_step.rs::ltr_ax_descriptor_faults_preserve_cpu_and_memory` (семь случаев: D нет, дескриптор и CPU не тронуты; биты A не проверяются) |
| 6.5 | LTR | отказ записи D (`table_writer` возвращает ошибку) | A: 4 остаются | не ставится (`TableWrite { bit: Dirty }`) | образ не записан, TR не загружен | только A | `page_walk::ltr_dirty_write_failure_commits_no_d_and_no_descriptor_image` |
| 6.6 | LTR | дескриптор совпал со страницей таблицы (`TableAlias`) | A допускается и может попасть в байты дескриптора | нет | дескриптор не пишется | A | `paged_ltr_step.rs::ltr_ax_rejects_descriptor_aliasing_a_page_table_page`; `ltr_ax_alias_accessed_write_may_change_descriptor_bytes_but_nothing_else` |
| 7.1 | доставка `#PF`: кадр через `PagedExceptionMemory` (tracked RAM write на каждый слот) | success | A на уровнях страниц кадра, по правилам 1.3-1.5 | да, на PTE каждой страницы кадра, до слота | шесть qword-слотов стека | A, D, слот на каждый слот в порядке push | `tests/system/stage4_exception_delivery.rs::wp_write_fault_delivers_with_if_clear_and_executes_first_handler_mov` (`dirty_updates == [0x5810, 0x5808]` и порядок callbacks); через production-цикл набора: Сист-1, шаг s20 (A на странице IDT, шесть слотов кадра на странице стека с уже стоящими A и D) |
| 7.2 | доставка `#PF` | fault на странице стека/IDT/GDT посреди кадра | уже совершённые A и записи слотов остаются | D у отказавшей страницы нет | предыдущие слоты записаны | до отказа | `tests/system/stage4_exception_delivery.rs::double_fault_stack_page_fault_shuts_down_and_preserves_partial_writes` |
| 8.1 | peeks и диагностика: `probe_long64_4k`, `translate_long64_2m_ram_read_only*` (no-op writer), `read_virtual_ram_diagnostic`, снимок после IRET | любой | не пишется | нет | только чтение | нет | `page_walk::pure_probe_permission_cases_never_commit_accessed_or_dirty_bits`; `page_walk::tests::page_table_mutation_is_seen_next_call_and_a_d_bits_are_unchanged`; `bus::tests::diagnostic_ram_read_translates_each_virtual_page_fragment`; `paged_memory_adapter::tests::post_iret_snapshot_observes_handler_repaired_missing_leaf_without_a_d_updates` |
| 9.1 | hit кэша выборки (`FetchTranslationCache`) | success | ничего не пишет сверх того, что закоммитил первый реальный обход (A уже стоит); сброс по записи в таблицу и повторный A/D — как без кэша | нет | байты читаются из кадра | нет новых | `linux_direct::tests::cache_straight_line_code_takes_one_walk_and_changes_nothing`; `cache_a_pte_store_that_clears_accessed_sets_it_again_on_the_next_fetch`; `gate_cleared_accessed_bit_of_a_pde_is_set_again_on_the_next_fetch` (и `..._pdpte_`, `..._pml4e_`); `gate_dirty_commit_on_the_page_of_the_cached_code_matches_the_uncached_run`; `gate_dirty_commit_for_another_page_of_the_same_table_matches_the_uncached_run` |
| 9.2 | кэш выборки | fault | отказ не кэшируется: полный обход, те же A, что без кэша | нет | не тронут | A предш./4 как 1.1-1.3 | `linux_direct::tests::cache_a_pte_store_that_unmaps_the_page_is_the_same_page_fault` |

**Итог покрытия таблицы до S4-09f (историческая запись; актуальный итог ниже, после S4-09f).** Всего 38 ячеек (строк): 30 покрыто тестом полностью, 5 покрыто
частично (2.1, 3.5, 6.2, 6.3, 6.4: тест проверяет исход, но не часть битов A/D), 3 не покрыты
тестом и покрыты чтением (4.1, 4.2, 4.3: RMW и `LOCK`, нет теста на биты таблицы). Тест не
придуман ни для одной из них.

**Системная приёмка S4-09f («покрыто системно»).** Сквозные тесты через production-цикл набора
(`run_cpl0_set_paged_2m_ram`) над смешанным деревом (4 KiB и 2 MiB), все A/D стартуют с 0; после каждой
инструкции сверяются литералы изменившихся entries и порядок committed writes. Модуль
`crates/od-machine/src/linux_direct/tests/stage4_ad_acceptance.rs`:

- Сист-1: `a_d_acceptance_mixed_tree_sequence_through_the_set_loop_matches_the_literal_bits_and_write_order` (шаги s0..s28);
- Сист-2: `a_d_acceptance_write_protection_fault_after_earlier_accesses_keeps_a_on_every_level_and_no_d`;
- Сист-3: `a_d_acceptance_cross_page_store_faulting_on_the_second_page_leaves_no_d_and_no_byte`.

Ячейки 4.1 и 4.3 после S4-09f подтверждены тестом на биты таблицы (раньше «не покрыто тестом»);
4.2 (RMW с fault) остаётся не покрытой тестом. Итог после S4-09f: 38 ячеек, 32 покрыто тестом
полностью, 5 частично (2.1, 3.5, 6.2, 6.3, 6.4), 1 не покрыта (4.2). Отчёт: раздел «S4-09f» в
[STAGE_4_REPORT.md](../reports/STAGE_4_REPORT.md).

**Что таблица не утверждает.** Атомарность на уровне host (compare-exchange на общей RAM, S6-31; compare-and-replace S6-25 serial в режиме чередования), `LOCK` как
межпроцессорная операция (S6-31), TLB и инвалидация A/D в кэшах (кэша данных нет; кэш
выборки сбрасывается по записи в кадр таблицы и по смене CR3), страницы 1 GiB (S4-10),
и A/D для paging modes кроме Long64 4 KiB / 2 MiB.

## S4-06b CPU-connected paged MOV

The public `od_machine::step_paged_mov` entry uses its internal
`PagedMovMemoryAdapter` to connect the bounded `od-isa` entry to the existing
`MachineBus` and its single `PhysicalMemory`
owner. Each step builds `Long64ProbeConfig::from_cpu_state` and uses the local
`canonical48 / physical36 / four-level / 4 KiB / RAM-only` profile. The paging
tests use literal, non-identity linear-to-physical mappings for code and data;
the adapter does not decode instructions or create another RAM owner.

The adapter routes fetch and operand operations through the tracked walker.
Successful fetch/read/write and faults retain the walk's actual A/D commits and
the existing committed-write callback order. A page fault is returned with its
`PageFaultRecord`, PFEC, linear address and original instruction RIP. Backend,
unsupported-mode/page-size, cross-page and architectural fault outcomes remain
distinct. MMIO is not accessed through this RAM-only path.

The `GuestMemory` guarantees for this connection are deliberately scoped:

- A failed read leaves the caller's result buffer unchanged.
- A failed operand write does not partially commit the operand.
- Page-walk A/D metadata writes already committed before a later fault or
  operand failure remain visible and each produces its callback in execution
  order. There is no whole-RAM rollback guarantee.
- Page-table bytes may alias operand bytes. Such A/D writes can modify bytes
  physically addressed by a failing operand, and successful operations keep
  separate callbacks in their actual order, even for duplicate ranges.

This reconciles the tracked walk with the bounded `GuestMemory` contract only
for the two admitted MOV forms. It does not establish full paging or settle
rollback assumptions for other consumers. See the
[S4-06b report](../reports/STAGE_4_S4-06B_2026-11-05.md) and
[ISSUE-054](../KNOWN_ISSUES.md#issue-054-s4-11-cross-page-cpu-integration).

## S4-06c CPU-connected paged MOV to MMIO

The same two exact MOV forms may access a qword in a mapped MMIO range after
the existing uncached walker completes translation and permission checks.
RAM operands continue through the sole `PhysicalMemory` owner. An MMIO operand
must fit wholly in one declared MMIO region and resolve to one existing
`MachineBus` binding; the machine does not synthesize a device or split/replay
the access. Instruction fetch from MMIO remains unsupported.

For MMIO writes, the leaf dirty bit and its committed-write callback occur
after successful translation/permission admission and before the single device
call. For MMIO reads, the CPU destination changes only after the device read
succeeds. A permission/translation failure never calls the device. A device
failure is a backend error rather than `#PF`; the MOV does not retire, CPU
register state stays uncommitted, and A/D writes already committed by the walk
remain visible with their callbacks. The bus device contract requires a failed
operation to return before guest-visible device state or effects are committed.
There is no implicit retry, so a caller must not replay a failed device
operation as if it were a guest page fault.

The bounded evidence covers only these MOV forms, qword same-page operands,
the existing canonical48/physical36/four-level/4 KiB profile, and synchronous
bus devices. It does not establish cross-page MMIO, other operand widths,
instruction fetch from a device, exception delivery, TLB/invalidation, JIT,
interrupts or full CPU paging integration. See the
[S4-06c report](../reports/STAGE_4_S4-06C_2026-11-05.md).

## Huge-page и mode admission boundary

Текущий machine contract принимает только canonical48, physical36, четыре уровня
Long64 и страницы 4 KiB. Present PS в PD не означает fallback или
частичную поддержку huge page: `probe_long64_4k` и `access_long64_4k_ram`
возвращают `UnsupportedPageSize` с уровнем, адресом entry и исходным значением.
Pure probe не меняет RAM; tracked path сохраняет уже committed A updates
предков, но не меняет отвергнутую entry, не спускается к нижним таблицам и не
трогает operand. Present PS в PDPT больше не даёт `UnsupportedPageSize`: с S4-10c
это гостевой reserved-bit `#PF` (см. [S4-10c](#s4-10c-pdpteps--гостевой-pf-rsvd)).

Проверка `Present` выполняется раньше размера страницы: nonpresent entry с PS
остаётся `PageFault`. Для present PD PS admission refusal предшествует
проверке остальной large-page формы в этом диагностическом контракте; это не
заявление об архитектурном приоритете реального x86 fault. PML4 bit 7 и (с S4-10c)
PDPTE bit 7 остаются reserved-bit outcome, а PT bit 7 не выбирает huge page и не
мешает 4 KiB leaf.

В 4 KiB API (`probe_long64_4k`, `access_long64_4k_ram`) 2 MiB/1 GiB leaves по-прежнему
не поддержаны; LA57, PCID и другие paging modes не реализованы и не объявлены
optional до фиксации manifest S4-00. Их добавление потребует отдельной проверки
alignment, reserved bits, permissions и A/D effects, а также отрицательных
тестов; текущий admission boundary не заменяет эту работу. Единственное
явное исключение для 2 MiB — отдельный opt-in RAM-профиль ниже.

## S4-10b: отдельный RAM-профиль 2 MiB (вертикальный поднабор)

`access_long64_2m_ram` и `step_paged_mov_2m_ram` явно допускают PDE PS=1 в
четырёхуровневом canonical48/physical36 Long64. Для такого PDE биты 20:13
reserved (reserved-bit `#PF`: PFEC RSVD|P плюс W/I по типу доступа), бит 12 принимается как PAT (memory type не
моделируется), физический адрес leaf берётся с выравниванием на 2 MiB плюс
смещение `linear & 0x1fffff`. Права агрегируются по пройденным уровням, A commits
предков сохраняются при последующем отказе, A и D leaf относятся к PDE
(`PageTableLevel::Pd` в `AdUpdate` и `TableWrite`). Для 4 KiB leaf используется
прежний путь; PS в PDPT — гостевой reserved-bit `#PF` (S4-10c заменяет прежнюю
host-ошибку `UnsupportedPageSize` для PDPT PS).

`step_paged_mov_2m_ram` — это `step_paged_mov` с явным `cr3` и теми же точными
qword MOV; доступ только к RAM (MMIO не маршрутизируется), operand через
4 KiB границу отклоняется как `CrossesPage` до walk. Instruction fetch ограничен
4 KiB страницей RIP: байт fetch/decoder lookahead за её пределами отклоняется как
bounded `CrossesPage` memory error до walk следующей страницы (PTE следующей
страницы не получает A), это не гостевой `#PF`. Старые 4 KiB API и
`step_paged_mov` поведение не меняют: PDE PS=1 по-прежнему `UnsupportedPageSize`.

Не входит в профиль: MMIO, 1 GiB leaves, TLB/кэш трансляций, LA57, PCID/global,
JIT. `step_paged_mov_2m_ram` принимает CR3 явным аргументом; CPU-owned CR3 со смешанными
4 KiB/2 MiB картами — отдельный quantum [S4-12e](#cpu-owned-cr3-quantum-with-mixed-ram-pages-s4-12e). Полная матрица U/S,
R/W, WP, NX/NXE по уровням и физическая проверка остаются открытыми. См.
[отчёт](../reports/STAGE_4_S4-10B_2026-11-08.md).

## S4-10c: PDPTE.PS → гостевой #PF RSVD

1 GiB страницы не объявляются: диагностический CPUID (`DiagnosticV1`, лист
0x8000_0001) возвращает нули, то есть `CPUID.80000001H:EDX.Page1GB = 0`. При
Page1GB=0 PDPTE.PS по Intel SDM — reserved, и реальный CPU с таким CPUID дал бы
тот же RSVD `#PF`. На CPU с Page1GB=1 тот же бит был бы валидной 1 GiB страницей;
эта модель её не объявляет. В Long64/4 KiB профиле (`probe_long64_4k`,
`access_long64_4k_ram`) и в 2 MiB RAM-профиле (`access_long64_2m_ram`,
`step_paged_mov_2m_ram`) present PDPTE с PS (bit 7) = 1 трактуется так же, как
PML4E bit 7 из S4-10a: bit 7 reserved, результат — гостевой `#PF`
(`PageFaultReason::ReservedBits { level: Pdpt, offending_bits }`, `offending_bits`
содержит bit 7 и любые другие reserved-биты той же entry), а не host-ошибка.
Этот срез заменяет правило S4-10a «PDPT PS → `UnsupportedPageSize`».

- **PFEC:** P=1 и RSVD=1 всегда; плюс W/R=1 для записи, U/S=1 для CPL3, I/D=1 для
  fetch только при EFER.NXE=1. Литералы для CPL0: read `0x09`, write `0x0b`,
  fetch (NXE=1) `0x19`, fetch (NXE=0) `0x09`; для CPL3: `0x0d`, `0x0f`, `0x1d`,
  `0x0d`. Источник: Intel SDM vol. 3, «Page-Fault Error Code» (RSVD-бит ставится
  при reserved-bit нарушении в paging-structure entry) и принятое правило
  S4-10a для PML4 bit 7 (тот же вызов формирования PFEC для reserved-bit fault).
- **CR2 / saved RIP:** CR2 — линейный адрес доступа; saved RIP — RIP инструкции;
  без retirement и без записи данных операнда.
- **A/D:** A ставится на предков, пройденных успешно (PML4E), и не ставится на
  PDPTE; D не ставится нигде. Walk останавливается на PDPTE (trace из двух
  entries), к PD не спускается, отвергнутая PDPTE не меняется.
- **Не меняется:** PDE PS=1 — 2 MiB leaf в S4-10b и `UnsupportedPageSize` в 4 KiB
  API; PML4 bit 7 — reserved-bit `#PF`; PT bit 7 (PAT) допустим в 4 KiB leaf;
  nonpresent PDPTE с PS остаётся `#PF` без RSVD (PFEC P=0). Приоритет reserved-bit
  проверки PDPTE.PS относительно остальных reserved-битов той же entry не
  заявляется: все они попадают в один `offending_bits`.

Не входит: 1 GiB позитивный путь, MMIO, TLB-on, запись через такую PDPTE,
cross-page. Не закрывает S4-10 и S4-33. См.
[отчёт](../reports/STAGE_4_S4-10C_2026-11-09.md).

## Cross-page admission и separate-call progress

`access_long64_4k_ram` принимает только один operand внутри одной 4 KiB page.
Для Read/Write, у которых `(linear & 0xfff) + width > 4096`, результатом
является `TrackedAccessError::CrossesPage` с исходным адресом и шириной. Проверка
выполняется до page-table read: trace пуст, A/D journal пуст, callback не
вызывается, RAM и device state не изменяются. Этот результат является admission
boundary API, а не guest `#PF` и не заявлением об архитектурном fault priority.

Crossing refusal предшествует проверке canonicality и доступности таблиц для
этого bounded API; Fetch остаётся byte-only и сам по себе страницу не пересекает.
API не разбивает operand и не различает потенциальные RAM/RAM, RAM/hole или
RAM/MMIO destinations. Нет implicit retry, transaction или device replay.

Каждый отдельный same-page call имеет собственный report и commit boundary.
Успешный предыдущий call не откатывается, если следующий call получает
`CrossesPage` или другой fault. Adjacent byte fetches с одним `instruction_rip`
являются только диагностической последовательностью вызовов; CPU restart,
REP-iteration, stack recovery и RMW atomicity остаются вне этого контракта.

## Bounded cross-page qword read (S4-11b)

The opt-in CPU-connected S4-11b slice admits only the existing
`MOV RAX,[RBX]` form (`48 8B 03`) when its 8-byte operand crosses one 4 KiB
boundary. It splits the linear range into its first-page and second-page
segments in increasing linear-address order. Each segment is translated and
checked using the existing Long64/4 KiB access context; the corresponding
physical range must be wholly RAM. The two physical ranges may be discontiguous.
No MMIO device is dispatched by this cross-page path. Cross-page writes remain
rejected. Cross-page instruction fetch is admitted only by the separate bounded
S4-11d case below; all other fetch forms retain their existing boundaries.

Both translations, permissions and RAM-region admissions complete before
either operand segment is read. Accessed-bit writes commit as each walk
completes, and their callbacks retain actual walk order. If the second walk
fails, earlier A-bit writes remain committed; a read never sets D. Once both
segments are admitted, their bytes are read in increasing linear-address order
into a private 8-byte buffer. The caller's result buffer and CPU destination
change only after the complete read succeeds. A fault identifies the first
inaccessible linear segment, retains the original instruction RIP and reports
the original data-read context; no partial operand value is exposed.

When page-table entries alias bytes in the operand range, both walks and their
A-bit writes occur before operand bytes are sampled, so those committed writes
are visible to the final read. Page-table aliasing between the two walks also
observes earlier committed updates. This is the deterministic contract of this
bounded uncached adapter; it does not claim a universal hardware ordering for
page-walk observations or atomicity of an unaligned cross-page access. [Intel
SDM Rev. 090, Vol. 3A §§5.7–5.8](https://cdrdv2-public.intel.com/874240/325462-090-sdm-vol-1-2abcd-3abcd-4.pdf)
defines page faults and A/D effects, while §11.1.1 states that split accesses
across page boundaries are not generally guaranteed atomic. The emulator's
per-segment order and caller-buffer commit boundary are explicit local API
behavior.

This slice does not cover cross-page stores, RAM/MMIO or MMIO/MMIO splitting,
stack/REP/RMW restart behavior, exception delivery/CR2 commit, TLB/JIT, or the
full S4-11 acceptance. See the [S4-11b report](../reports/STAGE_4_S4-11B_2026-11-05.md).

## Bounded cross-page dword read (S4-11c)

The separate opt-in `PagedMovCrossPageDwordReadLong64V1` CPU-connected
profile admits only bare Long64 `MOV EAX,[RBX]` (`8B 03`), with no prefixes,
RBX as the sole address form, one vCPU, and the existing canonical48,
physical36, four-level 4 KiB uncached page-walk configuration. It does not
broaden `PagedMovLong64V1` or the S4-11b qword profile.

Same-page dword RAM read is the control. A crossing dword is accepted only at
linear offsets `0xffd`, `0xffe`, or `0xfff`. Both page walks, permissions and
physical RAM admissions complete before operand reads. A/D effects from each
completed walk persist in actual walk order; read access does not set D. After
both segments are admitted, bytes are read in increasing linear order into a
private four-byte buffer. Only a complete successful read commits that buffer
to CPU state as zero-extended EAX, advances RIP by two and retires once. Any
failure leaves full RAX, RIP and retirement unchanged. A second-page fault
identifies that segment's first inaccessible linear byte, vector 14, original
instruction RIP, and Read/Long64/Explicit access context.

Page-table aliases follow S4-11b's deterministic walk-then-sample behavior:
both walks and their A updates happen before sampling operand bytes, and an
earlier walk's committed writes can affect the later walk. This is a local
uncached adapter contract and makes no universal x86 ordering or atomicity
claim. Cross-page MMIO is not dispatched; stores and all other widths/forms
remain unsupported here. This does not implement general guest exception
delivery or complete S4-11. See the [S4-11c CPU contract](CPU_CONTRACT.md#exact-paged-dword-read-profile-pagedmovcrosspagedwordreadlong64v1-s4-11c).

## Bounded cross-page word read (S4-11e)

The separate opt-in `PagedMovCrossPageWordReadLong64V1` CPU-connected profile
admits only exact Long64 `MOV AX,[RBX]` (`66 8B 03`). It uses one vCPU,
synthetic Long64 state, canonical48/physical36, four-level 4 KiB paging, and
uncached translation. The profile covers CPL0 and CPL3 only when
`RFLAGS.AC=0`. The complete three-byte instruction resides on a code RAM page
separate from the operand RAM pages; this profile does not extend
cross-page instruction fetch. The shared CPU entry rejects CPL1/CPL2, AC=1 and
instruction starts at offsets `0xffe`/`0xfff` before a fetch or page walk.

A same-page two-byte RAM read is the control. The crossing case starts at
linear operand offset `0xfff`, with its two bytes mapped to non-contiguous
physical RAM pages. Both page walks, permission checks, and RAM-region
admissions complete before operand sampling. After both bytes have been read
into a private two-byte buffer, a complete successful read commits AX only,
preserving `RAX[63:16]`, all other registers, flags, and definedness; it then
advances RIP by three and retires once. The caller buffer and CPU state are
committed only after the complete read succeeds.

A second-page #PF identifies the first inaccessible byte on that page,
reports vector 14, the original instruction RIP, and Read/Long64/Explicit
context. For the admitted cases, a nonpresent second-page translation at CPL0
has PFEC `0`; a supervisor-only second-page translation at CPL3 has PFEC
`5`. Either fault preserves the complete CPU state and caller buffer and
retires zero instructions. Accessed-bit updates from completed walks persist
in walk order; reads do not set Dirty.

This profile is RAM-only: same-page MMIO and any crossing MMIO are rejected
without device callbacks. Aliased page-table effects follow S4-11c's
walk-then-sample contract; this local behavior makes no hardware atomicity
claim. Stores, other encodings or address forms, cross-page instruction fetch
in this profile, stack/REP/RMW behavior, exception delivery/CR2, JIT/TLB, and
Linux execution are outside this slice. See the
[S4-11e CPU contract](CPU_CONTRACT.md#exact-paged-word-read-profile-pagedmovcrosspagewordreadlong64v1-s4-11e)
for the encoding and normative references.

## Bounded word-read instruction fetch across a 4 KiB page (S4-11f)

The separate opt-in `PagedMovCrossPageWordFetchLong64V1` profile admits exact
`66 8B 03` fetch from offsets `0xffe` and `0xfff` across two non-contiguous RAM
code pages, with the two-byte operand wholly in a separate RAM page. It uses
one vCPU, CPL0, Long64, canonical48/physical36, four-level 4 KiB uncached
paging, NXE=1 and AC=0. The S4-11e `PagedMovCrossPageWordReadLong64V1` profile
retains its no-crossing-fetch gate. The fixture has no aliases between code,
operand and page-table mappings.

The same-page instruction at `0xffd` is the control and may execute with the
next code page absent. For crossing instructions, all required code bytes are
fetched before an operand translation/read. A second-page non-present or NX
fetch fault reports the second page's first required linear byte, original
instruction RIP and Fetch/Long64/Explicit context (PFEC `0x10` or `0x11`),
without CPU commit or retirement. Earlier A updates persist; D remains clear;
no operand page walk occurs. Hole and MMIO code-page outcomes do not dispatch a
device. A simultaneous unavailable operand does not outrank the fetch fault.
After complete decode, a word operand starting at data-page offset `0xfff` is
rejected before a data-page walk; the profile never splits its operand. Every
public entry requires the matching CR0/CR4 profile pair before fetching.

This slice adds no cross-page operand support, stores, exception delivery/CR2,
restart, JIT/TLB or Linux integration. See the [CPU contract](CPU_CONTRACT.md#exact-paged-word-read-and-cross-page-instruction-fetch-profile-pagedmovcrosspagewordfetchlong64v1-s4-11f)
and the [S4-11d fetch contract](MACHINE_CONTRACT.md#bounded-cross-page-instruction-fetch-s4-11d).

## Bounded cross-page instruction fetch (S4-11d)

This is a narrow fetch extension for the existing opt-in
`PagedMovCrossPageDwordReadLong64V1` profile and exact bare `MOV EAX,[RBX]`
(`8B 03`). It uses one vCPU, synthetic Long64, canonical48/physical36,
four-level 4 KiB uncached translation, CPL0 and NXE=1. The opcode byte `8B` is
at linear `RIP = page + 0xfff`; the ModRM byte `03` is on the next linear page.
The two code pages map to non-contiguous RAM pages, and the dword operand lies
entirely in a separate RAM page. This does not broaden the profile's opcode or
data operand allowlist.

Fetch proceeds byte by byte through the existing decoder and paged memory
adapter. Success fetches exactly the two instruction bytes, completes the
existing dword read, zero-extends EAX, advances RIP by two and retires once;
all other CPU state is preserved. A second-byte non-present fault records
PFEC `0x10`; an NX fault with NXE enabled records PFEC `0x11`. In either case
the record identifies the faulting linear byte, original RIP and
Fetch/Long64/Explicit context. CPU state and retirement remain unchanged,
already committed A effects remain visible, no D bit is set, and operand
translation/read is not reached.

Fetch from a hole and fetch from MMIO retain their distinct backend outcomes;
the device is never dispatched. As a decoder-level control only, a one-byte
instruction whose final byte is at the end of a mapped page succeeds with the
next page unmapped, proving the decoder does not speculatively fetch beyond the
instruction boundary. This control does not broaden the CPU-connected profile
allowlist. Code, operand and page-table backing do not alias in these cases.

This local profile adds no exception delivery, CR2 update, retry, stores,
JIT/TLB behavior or Linux claim. It does not establish general cross-page
instruction fetch support. See the [S4-11d CPU contract](CPU_CONTRACT.md#cross-page-fetch-for-pagedmovcrosspagedwordreadlong64v1-s4-11d).

## CPU-state to diagnostic translation snapshot

`Long64ProbeConfig::from_cpu_state` строит immutable diagnostic snapshot из
`LongModeState::cr0` и `LongModeState::cr4`; отсутствующий CR0 отвергается
раньше отсутствующего CR4 через `MissingCpuRegister`. CR3 и EFER остаются
явными аргументами adapter; если CPU state содержит CR3, он обязан совпасть
с переданным значением snapshot.

После извлечения raw values метод вызывает существующий `Long64ProbeConfig::new`.
Он не подставляет defaults, не маскирует unsupported bits, не меняет CPU/RAM/A/D
и не создаёт guest exception. Ошибки остаются configuration-admission results;
snapshot не меняется, если caller позже изменит CPU state или построит новый
snapshot с другим CR3/EFER.

Этот bridge позволяет диагностическому probe использовать CPU-owned CR0/CR4
значения и явный CR3/EFER context, включая WP/NXE effects уже существующего
validator. Он не реализует WRMSR/EFER, mode transition history, startup
sequencing, TLB invalidation или CPU-connected paging.

## CPU MOV CR3 execution boundary (S4-12b)

`MovCr3Long64V1` returns a typed CR3 context-change effect only after a
successful instruction commit. It retires exactly one instruction and stops
before any following fetch. The caller must consume the effect before resuming
execution. S4-12c integrates it into the opt-in paged CPU profile below. See the
[CPU contract](CPU_CONTRACT.md#exact-mov-cr3-execution-profile-movcr3long64v1-s4-12b)
and [S4-12b report](../reports/STAGE_4_S4-12B_2026-11-05.md).

## CPU-owned CR3 translation handoff (S4-12c)

`run_paged_cr3_probe` is the opt-in machine quantum for
`PagedCpl0Cr3ProbeV1`. It builds the active `Long64ProbeConfig` from CPU-owned
CR0/CR3/CR4 and fixed EFER before STOP polling. Each instruction gets a fresh
page-aware memory adapter holding that immutable snapshot.

Its instruction allowlist is exact `MOV CR3,RAX` and exact `MOV RAX,[RBX]`;
stores, other memory forms, stack operations, PIO and HLT are unsupported.

After a retired `MOV CR3`, the machine verifies that the effect's full old CR3
matches the active snapshot and that committed `LongModeState.cr3` equals the
effect's new CR3. It validates a candidate snapshot from that committed CPU
state and replaces the active value before the next STOP poll or instruction
fetch. The next fetch and operand access therefore walk the replacement root;
applying the effect itself performs no page walk or RAM/A-D update. The quantum
reports consumed context changes in retirement order.

Effect mismatch or candidate-validation failure is a terminal machine
integration error. The committed MOV and its retirement remain visible; the
machine does not resume using a stale snapshot. An unavailable or invalid page
table under the new root is observed only by the next real access with the
existing precise fault/backend classification. The old `run_paged_probe`
continues to reject CPU-owned CR3. This profile adds no TLB caching,
invalidation, PCID, global pages, exception delivery, or EFER transitions. See
the [CPU profile](CPU_CONTRACT.md#paged-cpu-owned-cr3-profile-pagedcpl0cr3probev1-s4-12c)
and [S4-12c report](../reports/STAGE_4_S4-12C_2026-11-05.md).

## CPU-owned CR0.WP translation handoff (S4-12d)

`run_paged_cr0_wp_probe` is the opt-in machine quantum for
`PagedCpl0Cr0WpProbeV1`. The source and target CR0 values are constrained to
`0x80010033` and `0x80000033`; CR3 is CPU-owned and fixed for the quantum,
CR4=`0x20`, EFER=`0xd00`, one vCPU and uncached translation. The allowlist is
exact `MOV CR0,RAX` and `MOV [RBX],RCX` only. The store operand must fit in one
4 KiB RAM page; a crossing operand is Unsupported before the data walk.
The exact LOCK-prefixed `MOV CR0` is #UD after full instruction fetch.

After `MOV CR0`, the machine checks the effect's full old value against the
active immutable snapshot and the new value against committed CPU state. It
validates a replacement `Long64ProbeConfig` and installs it before the next
STOP poll or memory access. Committing the effect does not walk page tables or
write A/D bits; the instruction fetch may already have made A updates. The next
supervisor store therefore observes WP from the new snapshot: WP=0 permits a
read-only mapping; WP=1 faults precisely without committing store data or D,
while prior A updates may remain. A successful store through an R/W=0 mapping
does not impose a D-bit assertion in this profile. Integration
mismatch/rejection is terminal and preserves the already retired CR0 write.
The S4-12c CR3 profile and older paged profiles retain their existing
instruction allowlists. Machine admission requires CPL0; the CPU instruction
entry separately classifies bare `MOV CR0` at nonzero CPL as #GP(0) after
fetch, without retirement/effect. It rejects the ordinary store at nonzero CPL
as `WrongPrivilege` after fetch without data access or retirement; it is not a
guest #GP.

The profile does not implement other CR0 bits, PE/PG transitions, EFER changes,
TLB invalidation, exception delivery or CR2. See the [CPU contract](CPU_CONTRACT.md#paged-cpu-owned-cr0wp-profile-pagedcpl0cr0wpprobev1-s4-12d)
for the exact CPU allowlist and values.

## CPU-owned CR3 quantum with mixed RAM pages (S4-12e)

`run_paged_cr3_probe_2m_ram` is an opt-in bounded quantum for the existing
`PagedCpl0Cr3ProbeV1` instruction profile (`MOV CR3,RAX` and `MOV RAX,[RBX]`; other forms keep
their `Unsupported` outcome). It takes the same arguments and returns the same report as
`run_paged_cr3_probe`, including the retirement-ordered `translation_context_changes`, and the
CR3 snapshot is replaced at the retirement boundary exactly as in S4-12c. The only difference is
the tracked RAM walk, which additionally accepts a PDE with PS=1 as a 2 MiB leaf
(`access_long64_2m_ram`, [S4-10b](#s4-10b-отдельный-ram-профиль-2-mib-вертикальный-поднабор)):
root A and root B may therefore map the same virtual regions through 4 KiB and 2 MiB leaves. A/D
bits go on the leaf used (the PDE for 2 MiB); a failing walk keeps the A/D updates and callbacks
already committed.

The CPU profile keeps instruction fetch inside the 4 KiB page of RIP (an RIP at or after offset
`0xff2` is `Unsupported` before any fetch, so the next page's PTE is never walked) and each operand
within one 4 KiB page (a qword crossing is a backend `CrossesPage` error, not a guest `#PF`). The
profile is RAM-only: no MMIO, no 1 GiB leaves, no TLB or JIT, and no 2 MiB variant of the CR0.WP
quantum. `run_paged_cr3_probe` keeps rejecting 2 MiB leaves (`UnsupportedPageSize`). See the
[S4-12e report](../reports/STAGE_4_S4-12E_2026-11-08.md).

## Fixed mixed-page CR3 artifact (S4-12f)

`load_stage4_cr3_mixed_probe` accepts exactly one raw image of `0x201000` bytes (any other length
or any differing byte is `InvalidImage`) and places it verbatim at GPA `0x1000` in a 4 MiB RAM
map. It is built from `tests/fixtures/guest/stage4/cr3_mixed_probe` by
`tools/build/build_stage4_cr3_mixed_probe.py` and runs on the [S4-12e](#cpu-owned-cr3-quantum-with-mixed-ram-pages-s4-12e)
quantum. Layout: root A `0x1000` (PML4 → PDPT `0x2000` → PD `0x3000`, PD index 2 → PT `0x4000`,
GVA `0x400000` → GPA `0xb000`, present only); root B `0x5000` (PML4 → PDPT `0x6000` → PD `0x7000`,
PD index 2 = 2 MiB leaf at GPA `0x200000`); code A at GPA `0xb000`, code B at GPA `0x200000`, and
the qword `0x1000` (root A) at GPA `0x201000`.

Initial state: CPL0 Long64, CR3 = `0x1000`, RAX = `0x5000`, RBX = `0x401000`, RIP = `0x400000`,
CR0 `0x80010033`, CR4 `0x20`, EFER `0xd00`, running, no STI shadow. The guest program is
`MOV CR3,RAX` (A → B), `MOV RAX,[RBX]` (loads root A from the 2 MiB page) and `MOV CR3,RAX`
(B → A). The CR3 profile does not admit `HLT`, so the image ends with a `UD2` sentinel at
GVA `0x400009`. Three instructions retire; with a budget of 3 the quantum ends by budget with
CR3 = `0x1000`, RAX = `0x1000`, RIP = `0x400009`, and with a larger budget the next instruction is
a precise `#UD` (vector 6, no retirement, repeated on every further run). Both CR3 writes appear
in `translation_context_changes` in retirement order. The host never touches CPU state or tables
between instructions; a split run (budget 1) matches the whole run in CPU, RAM, definedness and
callbacks, and after the first retired switch nothing has been walked under root B yet. NX or a non-present
2 MiB PDE after the switch raise a precise `#PF` (error code `0x11` / `0x10`) with CR3 left at B
and the walk's accessed-bit prefix committed. This is limited evidence (one vCPU, RAM only, no
TLB/JIT, no 2 MiB MMIO or Linux boot). See the
[S4-12f report](../reports/STAGE_4_S4-12F_2026-11-08.md).

## Candidate Linux machine inventory (S4-00c)

The versioned [candidate manifest](../reports/data/stage4/s4-00c-machine-candidate-v0.1.json)
consolidates one Ubuntu Generic amd64 `7.0.0-38.38` direct64 target, its evidence,
open values and ownership. It is an evidence inventory, not a separate normative
machine specification or a claim that the machine can boot Linux. CPU and board
policies remain in [CPU_CONTRACT](CPU_CONTRACT.md) and
[VIRTUAL_BOARD](VIRTUAL_BOARD.md). The [S4-00c report](../reports/STAGE_4_S4-00C_2026-11-08.md)
explains the status vocabulary and unresolved prerequisites. No entry snapshot or Linux
boot trace has been observed. S4-00, S4-01, full S4-10 and Stage 4 remain open.

### Selected Linux direct64 path

Under boot protocol 2.15 the loader enters the compressed image's 64-bit entry at `+0x200`
of the loaded protected-mode image (`setup_sects=39`; see the
[S4-00b report](../reports/STAGE_4_S4-00B_2026-11-06.md)). It must provide long mode with
paging, identity mappings for the image, zero page and command line, a flat GDT and segments,
interrupts disabled and RSI pointing to `boot_params`; these are handoff requirements, not an
observed snapshot. In the recovered source (hashes of the three S4-00a files in that report; `ident_map_64.c` and `misc.c` in the S4-00c report) the path continues as
follows; the trace is conditional on the proposed CPUID and initial CR4 and does not prove that
the binary executes it:

| Path / condition | Source evidence | Existing owner / API | Remaining prerequisite |
|---|---|---|---|
| Loader enters compressed direct64 | protocol 2.15; `arch/x86/boot/compressed/head_64.S` `startup_64` loads null selectors into DS/ES/SS/FS/GS, `lgdt gdt64` + `lretq` to `__KERNEL_CS`, `load_stage1_idt`, `sev_enable` (`CONFIG_AMD_MEM_ENCRYPT=y`, CPUID probing), then masks `%cr4` | S4-37 loader; S4-12f and S4-32 are separate synthetic handoffs; paged LGDT/LIDT (S4-18a/b) | Define and validate the exact entry state (control registers, GDT/segments, tables); null-selector loads (including SS), `LRETQ`, `sev_enable` CPUID probing and the CR4 read-modify-write are not covered (S4-18e excludes null selectors and SS) |
| `configure_5level_paging` | `head_64.S` calls it; `pgtable_64.c` requests LA57 only without `no5lvl` and with CPUID leaf 7 ECX[16] | CPU_CONTRACT reference CPUID (max basic leaf 1, so the request is false) | Confirm CPUID and `CR4.LA57=0` at entry; if `CR4.LA57` were set the source switches depth through a low-memory 32-bit trampoline |
| Compressed identity maps | `ident_map_64.c` rounds ranges to `PMD_SIZE`, supports large PMDs (`_PAGE_PSE`) and calls `write_cr3` | S4-10b RAM-only 2 MiB profile, S4-12e mixed CR3 quantum, S4-12f probe | Real instruction forms and CR4 effects, large-page requirement from a runtime trace (2 MiB MMIO is not implemented) |
| Decompress / relocate | `misc.c` (KASLR relocation, decompression, ELF parsing) | S4-37 loader; CPU instruction coverage per S4-01 | Run the decompressor under the selected CPU profile |
| Main kernel `startup_64` | `arch/x86/kernel/head_64.S`: `MSR_GS_BASE` writes, `verify_cpu`, `%cr3` loads; the `lgdt`, second `MSR_GS_BASE` write and EFER `rdmsr`/`wrmsr` are in the `common_startup_64` continuation | GS-base MSR slice, paged LGDT/LIDT (S4-18a/b), MOV CR3 (S4-12b/c) | General WRMSR/EFER policy, `verify_cpu` CPUID leaves, remaining startup instructions |

Direct64 therefore does not mean a direct jump to the uncompressed main kernel and does not
avoid the decompressor's page-table work. ISSUE-027 remains open for the absolute first EFER
access and actual startup state, and ISSUE-036 (opened for the FPU-mapping evidence) tracks the same
signature and source-mapping reproduction gap; both cite the earlier Stage 2 mapping, which this slice
does not reproduce.

## Experimental pure-translation cache boundary

`experimental-translation-probe-cache` — opt-in measurement prototype, а не
production TLB. One-entry cache владеет immutable borrow одного
`PhysicalMemory`, хранит только успешный 4 KiB translation и тегирует linear
page, полный `Long64ProbeConfig`, `AccessKind` и полный `AccessContext`.
Page-table/host pointers не сохраняются. Hit восстанавливает offset в сохранённом
physical page; miss вызывает существующий pure probe и различается в отчёте как
`Walk`. Fault, unsupported-size и другие неуспешные outcomes не кэшируются.

Прототип доказывает только корректность и стоимость synthetic pure-probe workload
при immutable mapping. Он не кэширует tracked A/D effects или конечный operand,
не реализует INVLPG/guest invalidation и не разрешает замену `PhysicalMemory` под
живым cache. `clear()` только удаляет resident value: он идемпотентен, сохраняет
привязку к memory и не выполняет walk, запись или A/D update. Для наблюдения
изменённой таблицы page-table writer должен завершить scope старого cache,
выполнить mutation через `PhysicalMemory`, затем создать новый cache. Fault и
unsupported replacement оставляют cache пустым; возвращённые `Translation4K`
являются диагностическими value snapshots и не являются access handles после
изменения mapping.

S4-14a проверяет этот lifecycle через remap, tighten/relax leaf write rights,
NX fetch rights и committed-write callback ranges. Это host-controlled
disposal/reconstruction boundary, а не guest invalidation protocol. Результаты
S4-13a не выбирают production capacity, replacement или TLB integration.

### Bounded INVLPG effect consumption (S4-14b)

The standalone CPU `step_invlpg` entry emits a typed
`TranslationInvalidation::Invlpg { linear_address }` only when the bare exact
instruction retires at CPL0 with a canonical operand. The experimental
`TranslationProbeCache` consumer is feature-gated and clears its one resident
translation before any subsequent lookup/continuation. Full clear applies even
when the cached tag names another linear page; empty and repeated consumption
are successful. The consumer reports the effect as consumed without walking
page tables or modifying RAM/A/D.

The CPU instruction fetch uses a diagnostic adapter over the same immutable
physical-memory snapshot and bypasses the resident probe-cache entry. The
composition demonstrates `Walk → Hit → INVLPG effect → consume → Walk → Hit`.
That fetch path uses pure translation and supplies no tracked A/D evidence.
Faulting LOCK/CPL/fetch cases and successful noncanonical no-op emit no effect
and leave the cache unchanged. This is not a cache-to-mutable-memory owner and
does not establish remap/permission-refresh, huge-page, global/PCID, CR3
invalidation, tracked A/D, JIT or production TLB behavior. The existing
S4-14a drop–mutate–recreate rule remains in force for page-table writes.
See the [CPU contract](CPU_CONTRACT.md#standalone-invlpg-profile-invlpglong64v1-s4-14b).

### Mutable diagnostic session (S4-14c)

The feature-gated `MutableTranslationProbeSession` is a separate bounded
experiment that exclusively borrows `&mut PhysicalMemory` for one vCPU and one
fixed S4-14b Long64/canonical48/physical36/four-level/4 KiB profile. It shares
the cache's private values-only one-entry state; the immutable
`TranslationProbeCache` keeps its existing borrow contract. No mutable memory
reference, cache entry reference or reusable access handle escapes the session.

`write_pte` accepts only 8-byte-aligned physical addresses whose full
`[address, address + 8)` range is inside physical36, then delegates to checked
`PhysicalMemory::write_u64`. Successful writes commit and notify once;
failed alignment/range/region writes leave RAM unchanged and do not notify.
Neither successful nor failed writes implicitly invalidate the entry; a
matching probe may continue to return the resident translation until the
caller consumes a retired INVLPG, MOV CR3 or MOV CR0.WP invalidation effect.
The standalone INVLPG
entry fetches instruction bytes through a pure walker over
the current memory, bypassing the resident cache and producing no A/D writes.
For this S4-14c path, only a retired CPL0 canonical invalidation effect is
consumed synchronously; it clears the full entry before control returns.
Faults, fetch failures and noncanonical no-ops preserve it. A CPU CR3 that
differs from the session snapshot is rejected before fetch. S4-14d adds the
separate retired MOV CR3 effect consumer below.

Acceptance covers a same-session `Walk → Hit → PTE remap → INVLPG → Walk → Hit`,
write-permission tighten/relax with exact page-fault metadata, failed-write
callbacks, and cache preservation on non-retiring/non-invalidating instruction
paths. Fetch-only lookup and invalidation remain diagnostic pure probes, not
tracked memory accesses. The S4-14c slice does not implement cached tracked A/D,
guest stores, CR3/reset transitions or generations. S4-14d adds only the
bounded MOV CR3 transition described below. Huge pages, PCID/global semantics,
DMA/SMP/JIT integration and production TLB behavior remain outside these
diagnostic slices. See the
[S4-14c report](../reports/STAGE_4_S4-14C_2026-11-05.md).

### CPU-owned CR3 invalidation in the mutable diagnostic session (S4-14d)

The feature-gated `MutableTranslationProbeSession::execute_mov_cr3` composes
the existing exact `MovCr3Long64V1` CPU step with the session's fixed
Long64/canonical48/physical36/four-level/4 KiB translation snapshot. Before
fetch it requires CPU CR0, CR3 and CR4 to match the active snapshot. Instruction
bytes are fetched through the pure walker, bypassing the resident translation
entry and producing no A/D writes.

Only a retired, single-instruction `Cr3Write` effect whose old value matches
the session and whose new value matches committed CPU state replaces the
snapshot. The session validates the replacement with `Long64ProbeConfig::new`
and clears its capacity-one translation entry synchronously before returning.
This applies to same-value CR3 writes too; after a PTE mutation they force the
next probe to walk current tables rather than hit the stale entry. The effect
consumer performs no table walk, A/D write or memory callback.

Rejected instructions, fetch faults and pre-state mismatches do not change the
snapshot or clear the resident entry. If CPU execution retires but the effect
cannot be reconciled or the replacement snapshot cannot be validated, the
session retains its old snapshot, clears its stale entry and becomes poisoned;
subsequent probe/write/instruction operations return `SessionPoisoned`. The
caller must stop using that session because CPU state may already have
advanced. This diagnostic integration does not add tracked A/D caching,
reset/EFER/CR0 transitions, PCID/global semantics, generations, JIT hooks or a
production TLB. Full S4-14 remains open. See the
[S4-14d report](../reports/STAGE_4_S4-14D_2026-11-06.md).

### CPU-owned CR0.WP invalidation in the mutable diagnostic session (S4-14e)

`MutableTranslationProbeSession` admits exactly CR0=`0x80010033` (WP=1) or
`0x80000033` (WP=0), with the same CR3/CR4/EFER, canonical48/physical36,
four-level/4 KiB profile. `execute_mov_cr0_wp` uses the separate exact
`MovCr0WpLong64V1` CPU entry; it does not reuse the broader S4-12d profile,
whose allowlist also includes a store. Fetch bypasses the resident cache and
uses the pure walker.

After a validated retired `Cr0Write`, the session installs a validated
replacement snapshot and clears its one-entry cache before returning. The
diagnostic policy also clears for same-value CR0 writes. A matching probe may
still return the old resident entry after a direct `write_pte` and before the
retired effect is consumed. The effect consumer itself performs no table walk,
A/D write or committed-write callback. Mismatched/unexpected retired effects
or invalid replacement snapshots clear the entry and poison the session, as in
S4-14d.

While CR0.WP=0, the original `execute_invlpg` and MOV CR3 entry points reject
the unsupported CR0 profile before fetch and leave CPU/session state unchanged;
returning to WP=1 makes those bounded entries available again. S4-14f adds a
separate `execute_invlpg_wp` path for either WP value without changing those
existing entries or the MOV CR3 CPU contract.

### INVLPG while CR0.WP is mutable in the diagnostic session (S4-14f)

`execute_invlpg_wp` validates CPU CR0/CR3/CR4 against the current session
snapshot before fetch, then uses the exact `InvlpgWpLong64V1` profile. Its
instruction fetch uses the pure page walker and bypasses the resident cache.
Canonical retired INVLPG consumes the typed effect and clears the one-entry
cache synchronously, including when WP=0. A noncanonical operand is the
architectural retired NOP and preserves the cache. Faults, unsupported forms
and state mismatches also preserve it; this path does not use MOV CR0's
retirement-poison rule. Page-table snapshots across fetch/effect show no A/D
writes, and the instruction has no target walk or data access.

Acceptance covers stale `Hit` after a PTE remap, then post-effect `Walk` and
`Hit` at the new mapping for both WP states; Ring1–3 #GP(0), LOCK #UD,
noncanonical NOP, fetch fault, unsupported encoding and CR0/CR3/CR4 mismatch.
The feature remains experimental and one-entry; no performance claim,
tracked A/D cache, reset, generations, PCID/global semantics, JIT integration
or production TLB behavior is added. See the
[S4-14f report](../reports/STAGE_4_S4-14F_2026-11-06.md).

### MOV CR3 while CR0.WP is mutable in the diagnostic session (S4-14g)

`execute_mov_cr3_wp` is a separate opt-in session entry for CR0=`0x80010033`
or `0x80000033`; it validates CPU CR0/CR3/CR4 against the current snapshot
before fetch. The exact bare MOV CR3 fetch uses the pure walker and bypasses
the resident cache. Only one retired `Cr3Write` whose old root matches the
snapshot and whose new root matches committed CPU state is consumed. A valid
replacement snapshot is installed and the capacity-one cache is cleared
synchronously, including same-value writes. CR0/CR4 must remain unchanged
across retirement. Malformed effects, retirement-count mismatches, invalid
replacement snapshots or unexpected CR0/CR4 changes clear the cache and poison
the session; non-retiring outcomes and fetch faults preserve cache and
snapshot. The original `execute_mov_cr3` remains WP=1-only.

The replacement root is not walked during effect consumption; an unusable root
commits and faults on the next probe. Fetch has no target/data access or A/D
writes. This remains a one-vCPU diagnostic experiment and does not add
production TLB behavior, tracked A/D caching, reset, PCID/global semantics,
JIT integration or a performance claim. Full S4-14 remains open. See the
[S4-14g report](../reports/STAGE_4_S4-14G_2026-11-06.md).

S4-14e also does not add tracked A/D caching, reset, generations, PCID/global
semantics, JIT integration or production TLB behavior. See the
[S4-14e report](../reports/STAGE_4_S4-14E_2026-11-06.md).

## Bounded diagnostic vCPU session boundary

S4-21a добавляет `DiagnosticVcpuSession` как синхронную caller-driven оболочку
над одним `LongModeState` и `MachineBus`. Сессия имеет состояния
`Runnable`, `Paused`, `Stopped` и terminal cause; исполнение разрешено только из
`Runnable`. Каждый вызов получает конечный instruction budget, сохраняет CPU
state и `rflags_defined`, а исчерпание бюджета возвращает `WorkLimit` без
автоматического продолжения.

`pause` и `resume` действуют только между вызовами и не меняют CPU/RAM. `stop`
идемпотентен; callback STOP передаётся существующим CPU poll points, поэтому
остановка до fetch и во время исполнения возвращает `Stopped` без дальнейшего
вызова. Fault, unsupported instruction, backend error и halted/stop-address
остаются разными terminal causes. Это диагностический synchronous boundary, не
полный vCPU lifecycle: HLT wakeup, reset, IRQ, MMU, JIT, VMHost/IPC и REP
continuation semantics остаются вне среза.

## Bounded paged probe quantum (S4-21b)

`od_machine::run_paged_probe` implements the opt-in `PagedCpl0ProbeV1`
machine quantum over page-aware RAM and the existing PIO bus. The CPU entry
`step_paged_probe` owns decode and semantics for the exact allowlist
`48 8B 03` (`MOV RAX,[RBX]`), `48 89 0B` (`MOV [RBX],RCX`), bare `EC`
(`IN AL,DX`), bare `EE` (`OUT DX,AL`) and bare `F4` (`HLT`). CPU admission
validates profile state and CR0/CR4; the machine validates CR3/EFER and
constructs the existing page-aware memory context.

One quantum receives a finite instruction budget, STOP predicate, definedness
and committed-write callback. At each boundary it applies this precedence:
STOP, exhausted budget, then the single CPU decode/fetch for that boundary.
The machine does not probe several instruction executors in sequence. It
returns distinct stopped, budget-exhausted, halted, unsupported, architectural
fault, memory-backend and PIO outcomes. A successful HLT retires once and
leaves the CPU halted; a repeated call on that state returns without fetching
or producing effects. HLT does not mean guest success.

Earlier committed instructions remain committed when a later instruction
faults or a STOP/budget boundary is reached. Page-table A/D updates and their
committed-write callbacks are retained. Architectural faults carry the exact
record, including PFEC, linear address, access kind/origin and saved RIP;
machine/backend and PIO errors remain distinct. The quantum does not deliver
exceptions, update CR2 or roll back prior effects.

Acceptance checks a single-quantum store→load→OUT→HLT probe and split
single-instruction quanta, with STOP/resume and exact-fault cases. The complete
vCPU lifecycle, S4-24 IRQ path, own-kernel boot, Linux, IRQ/APIC and exception
delivery remain outside this bounded slice. See the
[S4-21b report](../reports/STAGE_4_S4-21B_2026-11-05.md),
[JSON evidence](../reports/data/stage4/s4-21b-paged-probe.json) and
[verification method](../testing/MACHINE.md#s4-21b-bounded-paged-probe-quantum).

## Bounded fixed-image paged vCPU owner (S4-21c)

`PagedVcpuSession` constructs its CPU, bus, RAM and UART together using the
validated `Stage4FlatProbeV1` image loader. Its host state is independent of
guest activity. `pause` and `resume` preserve CPU, RAM, flags and device state;
HLT stays halted. Idempotent STOP is accepted after HLT or a terminal exit and
blocks further quantum and RX dispatch until reset. The owner runs no polling
loop. Quantum reports update the stored RFLAGS defined-mask. Guest faults and
other terminal quantum exits require reset; HLT is not guest completion.

`reset(image)` builds and checks a replacement, including no-fetch paging and
fixed CPU-profile admission, before touching the current session. It then
resets the existing RX pump with checked generation increment, clears payloads
and retargets it to the new UART; after that only infallible replacement steps
remain. Invalid image or queue reset failure leaves the session unchanged.
The original S4-21c API exposed scheduling and bounded dispatch without
generation tokens and accepted caller-supplied time at dispatch. S4-22b later
moved dispatch time into the session-owned `VirtualClock`; S4-23d exposes the
queue-local generation and requires it for RX scheduling/cancellation.
A fresh UART belongs to the replacement bus; stale RX and TX device state is
not inherited. The operation is a fixed-image re-entry, not x86 reset-vector
semantics. Runner completion/cancellation, IRQ wakeup and general scheduling
are outside this slice. Independent Sol/high review passed with no P0–P3
findings.

## Fixed-image runner completion and cancellation (S4-21d)

The owner exposes `drain_tx` to complete and consume committed polling-UART TX
without exposing the UART handle. The runner invokes one bounded owner quantum.
Only HLT with exactly three retired instructions, RAX matching the supplied
nonce and TX equal to its single low byte is `guest_completed`. HLT without an
expectation is `guest_halted`; a failed comparison is `completion_mismatch`.
`--cancel-after N` allows N instruction polls then requests STOP, producing
`host_cancelled` with the committed prefix; budget exhaustion is `work_limit`.
Guest faults, backend, port I/O, loader and input failures remain distinct.
The cancellation source is deterministic; VMHost and OS signal wiring is S4-41.
See [S4-21d report](../reports/STAGE_4_S4-21D_2026-11-07.md).

## Restart of the firmware board processors (S4-21e)

`LoadedFirmwareBoard::restart_vcpus` is the processor half of the machine reset
that S7-26 completes. It is legal between runs after any stop (guest reset or
S5, halt, budget, host STOP, guest shutdown, host fault). It returns every vCPU
to the power-on state through `reset_cpu_complex`: the boot processor takes
the entry state the loader gave it (recorded on the board; the firmware entry is
direct Long64, not the reset vector), every application processor waits for SIPI
with the real-mode reset state, the guest CR2/EFER/MSR cells, `REP` resume flag
and NMI blocking are cleared, and every Local APIC and the IOAPIC route drop
pending interrupts, NMI, INIT/SIPI, armed timer and Remote IRR. It then
acknowledges the PM block's stop latch (`take_stop_request`) and returns the
acknowledged `MachineStopReason`, so the next run is not stopped before its first
instruction. A board assembled for the Linux direct boot has no recorded entry
state and refuses the call without changing anything.
Not touched: guest RAM, ROM, devices (the PM registers too), NVRAM, TPM, the
virtual clock and the host STOP flag. RAM policy of cold and warm reset, device
and TPM volatile reset, and the runner loop are S7-26. The hardware-reset
values of the x87 state (FCW `0040H`, FTW `5555H`, SDM Vol. 3A Table 12-1)
are not modelled: vCPUs reset to the `FNINIT` values (S4-38L9d).
See [S4-21e report](../reports/STAGE_4_S4-21E_2026-11-21.md).

## Caller-driven virtual clock and RDTSC boundary

S4-22a добавляет `VirtualClock` с начальным `now_ns = 0` и фиксированной
ненулевой частотой `tsc_hz`. Время изменяется только явным
`advance_time_ns(delta)` между execution calls в состоянии `Runnable`;
`run`, instruction budget, polling и host delay его не продвигают. Значение TSC
равно `floor(now_ns * tsc_hz / 1_000_000_000)` с `u128` intermediate. Overflow
времени или результата отвергается до изменения состояния.

`DiagnosticVcpuSession::new_with_tsc` использует этот источник только для bare
Long64 `RDTSC`, сохраняя существующие TSD/CPL checks, `EDX:EAX`, RIP и
`rflags_defined`. Legacy constructor остаётся без clock/RDTSC source. RDTSCP,
MSR/CPUID frequency advertisement, host monotonic coupling, HLT progression,
IRQ/deadlines и reset не входят в срез; полный S4-22 остаётся открытым.

## Paged vCPU clock and UART RX deadlines (S4-22b)

`PagedVcpuSession` принимает явный ненулевой `tsc_hz` и владеет одним
`VirtualClock`. `schedule_rx` принимает абсолютные deadlines в наносекундном
домене этого clock; `dispatch_rx` считывает текущее время из владельца и не
принимает внешний `now_ns`. Run, polling и host call count не продвигают время.

`advance_time_ns` допустим только при host state `Active`; guest HLT сам по
себе ему не препятствует. Это сохраняет HLT и не dispatch-ит RX: гостевое
пробуждение не реализовано. Pause, Stop и Terminal отвергают advancement.
Reset очищает RX очередь и увеличивает её generation, но сохраняет clock и абсолютное время;
ошибка reset и переполнение времени/TSC не меняют состояние clock. Частота,
выбранная Stage 4 diagnostic runner, не является Linux-visible CPUID/MSR
объявлением. Host monotonic coupling, HLT wakeup, IRQ delivery, RDTSCP и полный
S4-21/S4-22 остаются вне этого среза. См. [S4-22b report](../reports/STAGE_4_S4-22B_2026-11-07.md).

## Bounded one-shot deadline queue boundary

S4-23a добавляет `EventQueue` с фиксированной логической ёмкостью и
абсолютными `u64` deadlines в том же наносекундном домене, что и
`VirtualClock`. Pending key уникален. `schedule`, `cancel`, `reschedule`,
`next_deadline` и `pop_due(now_ns)` имеют явные результаты; `pop_due` удаляет не
более одного события и считает `deadline <= now_ns` готовым, включая прошлые
deadlines.

Очередь упорядочивает события по `(deadline, insertion sequence)`; reschedule
получает новую sequence. Duplicate/full/not-found не меняют очередь. Очередь
сама не продвигает время, не вызывает callback, не исполняет CPU, не будит HLT
и не доставляет IRQ.

S4-23b добавляет непрозрачный `EventGeneration` конкретного экземпляра очереди,
начиная с `0`. Все изменяющие и извлекающие операции (`schedule`, `cancel`,
`reschedule`, `pop_due`, `reset`) требуют ожидаемое поколение; несовпадение
возвращает `StaleGeneration` до поиска ключа, проверки ёмкости или изменения
состояния. Уже извлечённое `ScheduledEvent` сохраняет своё поколение, и владелец
может проверить его через `validate_generation` перед использованием.

`reset` проверяет текущее поколение и выполняет checked increment. При успехе он
очищает ожидающие события и сбрасывает insertion sequence, сохраняет выделенную
ёмкость и публикует новое поколение. Reset не меняет `VirtualClock` и абсолютный
домен deadlines. При исчерпании поколения возвращается `GenerationExhausted`;
pending entries, sequence, capacity и clock остаются без изменений. Sequence
остаётся отдельной от поколения: её rebasing не инвалидирует поколение.

Порядок `(deadline, insertion sequence)`, новая позиция при reschedule,
готовность `deadline <= now_ns`, извлечение не более одного события и прежняя
атомарность duplicate/full/not-found сохранены. Проверка поколения и reset не
реализуют dispatch: periodic timers, HLT wakeup, IRQ, host wait, конкурентный
доступ и межмашинное владение остаются за пределами bounded generation/reset
среза S4-23b; HLT wakeup и host wait остаются открытыми требованиями полного
S4-23.

## Bounded UART RX event pump (S4-23c)

`UartRxEventPump` connects a bounded payload store to one UART RX endpoint. Its
payload capacity is fixed and bounded. `EventQueue` owns generations, absolute
deadlines, equal-deadline insertion order, cancellation, rescheduling and reset
invalidation; the pump does not maintain a second deadline/order structure.

Dispatch checks the expected generation first, then STOP, then the finite event
budget, then readiness of the earliest event at the current `VirtualClock`
time. Successful delivery injects the byte exactly once and removes its event
and payload. If the receiver is busy, dispatch returns `ReceiverBusy` and
preserves the earliest event, payload, deadline and ordering position; later
events do not bypass it. A later dispatch can retry that same event after the
guest consumes the pending RX byte.

Reset and stale-generation rejection follow `EventQueue` and invalidate pending
work. The pump does not change `VirtualClock` or CPU state, execute guest code,
or wake HLT. IRQ/APIC delivery, baud timing, HLT wakeup and host waiting remain
outside this bounded slice and open in S4-23/S4-24.

## Generation-aware RX control through PagedVcpuSession (S4-23d)

`PagedVcpuSession` exposes the current opaque queue-local `EventGeneration`
through `rx_generation()`, and a read-only `next_rx_deadline()` in the session
clock's absolute nanosecond domain. `schedule_rx`, `cancel_rx` and
`reschedule_rx` require the caller's expected generation. A generation from
before reset is rejected even if its key is reused in the replacement queue.

Control operations check staleness before lifecycle admission; a current
generation then requires host `Active` and guest not `Halted`, after which the
underlying queue returns duplicate/full/not-found results. A stale request thus
returns `StaleGeneration` even when the session is paused, stopped, terminal or
halted. Deadline inspection is side-effect free and available in every state.
Reset clears pending events and advances the generation while preserving the
clock; successful reset replaces a halted guest with a fresh image. RX control
does not wake HLT. Scheduling preserves `(deadline, insertion sequence)` order;
rescheduling assigns a new insertion position, including when the deadline is
unchanged. Deadline inspection and dispatch do not change queue ordering.

The generation is scoped to this queue instance; it does not identify a session
across owners or distinguish separate uses of a key within one generation.
Dispatch remains bounded and caller-driven. HLT wakeup, IRQ/APIC, periodic
timers, host wait and complete S4-21/S4-22/S4-23 acceptance are not claimed.
See [S4-23d report](../reports/STAGE_4_S4-23D_2026-11-07.md).

## Private one-slot deadline replacement (S4-23e)

`EventQueue::replace_single` is crate-private and accepts only a queue with
capacity one. It validates the caller's `EventGeneration`, checks that the
generation can advance, then replaces the pending deadline or cancels it and
returns the new generation. Each successful operation invalidates a previously
extracted event, including one from the same logical slot before rearm. A stale
generation, wrong capacity or generation exhaustion leaves the queue unchanged.
The operation uses the slot's fixed internal key and does not change the public
queue API. It is a prerequisite for the Local APIC timer owner; it does not
implement timer registers, clock ownership or event dispatch. See the
[S4-23e report](../reports/STAGE_4_S4-23E_2026-11-07.md).

## Fixed-edge Local APIC register core (S4-25a)

`LocalApic` is a fixed-profile xAPIC `BusDevice` (the description below is of one LAPIC; since S6-11 it is one of `vcpu_count` LAPICs, one CPU by default) at MMIO base
`0xFEE00000`, with APIC ID 0 (one LAPIC per vCPU on a board with several vCPUs:
see [LAPIC на каждый vCPU](#lapic-на-каждый-vcpu-s6-11)). It supports aligned 32-bit accesses on the
16-byte register stride for ID (`0x020`), TPR (`0x080`), PPR (`0x0A0`), EOI
(`0x0B0`), SVR (`0x0F0`), ISR (`0x100..0x170`), TMR (`0x180..0x1F0`) and IRR
(`0x200..0x270`). Fixed-edge requests for vectors 16 through 255 set the
corresponding IRR bit; repeated requests coalesce. TMR remains zero because
this profile accepts edge requests only.

Reset clears TPR, IRR and ISR and restores SVR to `0xFF` (software disabled).
Eligibility requires the SVR software-enable bit and an interrupt priority
class strictly above PPR. PPR uses the higher priority class from TPR and the
highest vector in ISR, preserving TPR's low nibble when its class wins or ties.
`pending` and `eligible` are read-only. `acknowledge` transfers one highest
eligible vector from IRR to ISR; it is a device-state operation and does not
deliver a CPU interrupt or execute a handler. EOI clears the highest ISR bit.

The xAPIC version register (`0x030`) is unsupported in this slice (superseded
by S4-25f below); no Version value is advertised because this slice has no
implemented LVT entries. LVT, APIC-base
MSR/relocation, x2APIC, IOAPIC/level routing, APIC timer, CPU interrupt
delivery, IDT/IF/shadow interaction, HLT wakeup, and default board wiring are
outside S4-25a. Non-supported offsets, widths, alignment and reserved writes
return a `DeviceError` without mutating APIC state; this is the local bus
contract, not guest exception delivery.

Behavior follows Intel's [64 and IA-32 SDM release 093, Volume 3A](https://cdrdv2.intel.com/v1/dl/getContent/671190), Chapter 13 (register map,
enable state, priority, interrupt request/service state and EOI). This bounded
profile is not full Local APIC compatibility. See the [S4-25a report](../reports/STAGE_4_S4-25A_2026-11-07.md) and [verification method](../testing/MACHINE.md#s4-25a-fixed-edge-local-apic-register-core).

## One-shot Local APIC timer (S4-28a1)

`LocalApic::with_timer(clock, timer_hz)` opts into one Local APIC timer with an
explicit nonzero frequency distinct from the clock's TSC frequency. The legacy
constructor has no timer registers. Timer MMIO uses aligned dword accesses at
LVT Timer (`0x320`), Initial Count (`0x380`), read-only Current Count (`0x390`)
and Divide Configuration (`0x3e0`). Only one-shot mode is supported; reserved
bits and other modes are rejected before state mutation. Divider encodings map
to 2, 4, 8, 16, 32, 64, 128 or 1; Divide Configuration can change only while
there is no active deadline. A zero initial count cancels; a nonzero count arms
or replaces the single deadline. Current Count is derived from elapsed virtual
nanoseconds and the configured timer frequency/divider, using checked wide
arithmetic and a rounded-up deadline.

The opt-in timer owns a caller-driven clock and one-entry event queue inside the
same APIC-pair mutex. Reprogram, cancellation and reset advance the queue
generation, so a previously extracted deadline is stale. `dispatch_timer`
processes at most one due event without entering the CPU. An unmasked expiry
requests its fixed-edge vector in IRR; a masked expiry consumes the deadline
without requesting an interrupt. The timer vector must be 16–255. An unmasked
timer LVT cannot match the configured IOAPIC level vector; a masked timer LVT
may use that vector, but unmasking the conflict is rejected. While the APIC is
software-disabled, the timer LVT is masked and attempts to unmask it are
rejected. Software disable while a deadline is active is also rejected without
changing state. A due timer must be dispatched before writable timer-register
accesses; they return `Busy` while a due event remains pending. The Current
Count register remains read-only.

This child slice does not connect the timer clock to a vCPU session, advance
time during guest execution, dispatch from a production scheduler, enter a CPU
handler, wake HLT, or implement periodic/TSC-deadline modes. It is not Linux
timer-profile evidence. The register and vector profile follows Intel's
[64 and IA-32 SDM release 089, Volumes 1–4](https://cdrdv2-public.intel.com/868137/325462-sdm-vol-1-2abcd-3abcd-4.pdf),
Volume 3A §§11.4.7.2, 12.5.2 and 12.5.4. See the [S4-28a1 report](../reports/STAGE_4_S4-28A1_2026-11-07.md).

## Shared vCPU clock for APIC timer (S4-28a2)

`VirtualClockHandle` is a cloneable reference to one `VirtualClock`. Its public
API creates a nonzero-frequency clock and reads snapshots; a non-cloneable
advancement capability can be claimed only once, for one vCPU session. The
claim remains consumed after session drop; create a new handle for a new
session. To connect an opt-in timer, construct
`LocalApic::with_shared_timer(clock.clone(), timer_hz)`, attach that device to
`MachineBus`, then pass `clock` to
`DiagnosticVcpuSession::new_with_clock_handle`. Session RDTSC and guest timer
MMIO (arm, Current Count and due checks) now observe the same nanosecond value.
The APIC handle's standalone `advance_timer_ns` rejects this shared mode.

The session advances only while Runnable. If a deadline is due while the
session is Paused, Stopped or Terminal, `dispatch_timer` returns `Busy` and
keeps the event pending; resume permits the caller to dispatch it. APIC state,
deadline queue and IRR remain protected by the existing APIC-pair lock; no APIC
lock is held while executing guest instructions. Arm derives deadline and
elapsed-count origin from one clock snapshot. Reset invalidates the timer
deadline but retains the shared session clock. This does not automatically dispatch an expiry,
deliver a CPU interrupt, progress time during HLT, or implement a scheduler.
The prior `with_timer(VirtualClock, hz)` standalone caller-driven behavior
remains available. See the [S4-28a2 report](../reports/STAGE_4_S4-28A2_2026-11-07.md).

### Explicit HLT timer progression (S4-28a3)

`DiagnosticVcpuSession::into_halt_wait` now accepts a configured shared clock
and matching RDTSC controls. The unique clock advancement capability moves
inside `DiagnosticHaltWait`. Its `advance_time_ns` succeeds only in `Waiting`;
overflow returns a typed clock error and leaves time/TSC unchanged. The caller
then explicitly dispatches the due APIC timer and polls `poll_apic`; a delivered
maskable vector commits the existing interrupt frame and wakes the wait. After
the caller executes the handler and IRETQ, RDTSC observes the advanced clock.

For a shared timer, due-event queue removal and IRR publication hold the APIC
pair lock and clock dispatch guard in that order. Before wake delivery, the wait
closes the dispatch gate and releases the clock lock before guest memory access.
If no eligible vector exists or admission returns `OutsideProfile`, it reopens
the gate and remains Waiting. Successful `Entered`, STOP, terminal failure and
advancer drop keep the gate closed; `into_running` opens it after committed
entry. APIC reset cancels its timer deadline but does not reset or advance the
session clock. No mutex is held across guest CPU execution or bus callbacks.

This remains caller-driven: the wait never polls, advances host time, dispatches
the timer, or executes the handler automatically. Periodic/TSC-deadline modes,
NMI/debug wake, production scheduling and Linux acceptance are not established.
See the [S4-28a3 report](../reports/STAGE_4_S4-28A3_2026-11-07.md).

### Periodic APIC timer (S4-28a4)

The same opt-in timer profile accepts LVT Timer bit 17 as the mode selector:
zero is one-shot and one is periodic. Bit 18 and other unsupported bits remain
rejected. Writing LVT selects the mode but does not arm the timer; a nonzero
Initial Count starts or replaces the countdown, and zero cancels it. While a
deadline is queued and is still in the future, the timer accepts a transition
from periodic mode to one-shot mode. The transition retains the deadline and
phase within the current period. Current Count reaches zero at that deadline.
The expiry is consumed once and signals the current LVT vector if unmasked.
Interrupts already pending or in service keep their state. A write repeating
the programmed divider is accepted before expiry and preserves the phase.
The timer accepts changes to the divider and transitions to periodic mode only
while no deadline is queued.
Vector and mask changes may be made before expiry without restarting the active
phase. The retained phase is the model's interpretation of the countdown and
reload rules. [Intel SDM rev. 090, vol. 3A, §13.5.4](https://cdrdv2-public.intel.com/874249/253668-090-sdm-vol-3a.pdf).

Periodic Current Count wraps through the programmed Initial Count as virtual
time advances. Dispatch processes no more than one due occurrence per call.
If caller-driven time passes several periods, occurrences coalesce into at
most one fixed-edge IRR request; the next deadline is the first periodic
boundary strictly after the current time, computed from the original arm time.
This preserves phase without catch-up bursts or dispatch-delay drift. A masked
expiry still advances/rearms the periodic timer but does not set IRR; unmasking
does not replay masked occurrences. The timer continues while its vector is in
ISR; EOI only clears ISR and does not control timer phase. Existing APIC
acknowledge and EOI behavior applies to each subsequently dispatched edge.

The capacity-one event queue, generation checks, APIC-pair→clock lock order and
shared-clock dispatch gate remain in force. Overflow while computing a next
deadline or queue-generation failure leaves timer and IRR state unchanged.
Writable timer registers still return `Busy` when a due event has not yet been
dispatched, including a zero-count write; dispatch remains an explicit caller
operation. Reset invalidates the queued event and clears periodic state while
preserving the associated clock. This is a bounded caller-driven profile, not
automatic scheduling or Linux timer-profile evidence. Register semantics are
based on Intel [SDM release 090, Volume 3A §13.5.4](https://cdrdv2-public.intel.com/874240/325462-090-sdm-vol-1-2abcd-3abcd-4.pdf).
See the [S4-28a4 report](../reports/STAGE_4_S4-28A4_2026-11-07.md).

### Guest periodic timer scenario (S4-28b)

S4-28b adds no device behavior; it fixes the observable contract of the
S4-28a1–a4 pieces as a caller-driven scenario on one shared clock. One step is:
the caller advances the session clock, calls `dispatch_timer`, then runs
`run_apic_quantum` (Runnable) or `poll_apic` (HLT wait); the guest handler
increments a RAM counter, writes the Local APIC EOI register (`0xFEE000B0`) and
executes IRETQ, after which the caller resumes the interrupted thread. N steps
yield exactly N handler runs and leave the interrupted thread's RIP, RAX, RSP
and RFLAGS as if no interrupt had occurred. `dispatch_timer` consumes at most
one due occurrence; a second call at the same virtual time returns `false`.

**Coalescing and ISR.** An expiry that arrives while the vector is in service
sets the (single) IRR bit and the timer keeps running; any number of expiries
before EOI collapse into one pending edge. The edge is eligible only after the
guest EOI clears the ISR bit (and only while IF and TPR permit). A large time
jump yields one edge, never a burst; the next boundary is the first period
boundary after the current time, counted from the original arm time.

**HLT gate paths.** `poll_apic` closes the dispatch gate before inspecting
IF/vector and reopens it on every non-committing result. IF=0, a vector blocked
by TPR, or `OutsideProfile` return/keep `Waiting` with the IRR edge retained and
the gate open (the next `advance_time_ns` and dispatch work, never `Busy`). A
masked LVT occurrence is consumed (periodic: rearmed) without IRR and without
wake; unmasking does not replay it. `DiagnosticHaltWait::stop` in `Waiting`
closes the gate for good: a due event stays queued, `dispatch_timer` returns
`Busy`, `poll_apic` returns `Stopped`, `advance_time_ns` returns `NotWaiting`.
Entered keeps the gate closed until `into_running`.

**Pause/resume.** A deadline that falls due before `pause` stays queued: while
Paused, `dispatch_timer` returns `Busy` and `run_apic_quantum` returns
`ProfileRejected` without changes; after `resume` the occurrence is dispatched
and delivered exactly once, and the next boundary is unchanged.

**Lock order.** `dispatch_timer` takes the APIC-pair lock, then the clock lock.
No clock operation (advance, gate toggle, snapshot, advancer drop) acquires the
pair lock, so the reverse order cannot occur; this is asserted by a test that
holds the pair lock while another thread performs every clock operation, and by
reading the code (the pair lock is never requested inside `with_dispatch_guard`
beyond the one already held).

**TSC-deadline decision.** TSC-deadline mode is not implemented and not
advertised: LVT Timer bit 18 is rejected (as are other unsupported bits),
`IA32_TSC_DEADLINE` (MSR `0x6E0`) has no handler, and the `DiagnosticV1` CPUID
profile keeps CPUID.01H:ECX[24] clear. The decision is revisited only by the
Linux timer-profile slices (S4-29, S4-37/S4-38) if the chosen kernel needs it.

The scenario is not an event pump or scheduler (owner undecided, S4-33),
does not connect the timer to the firmware board, and is not Linux
timer-profile evidence. The handler runs with IF=0 (interrupt gate); a handler
that re-enables interrupts is modelled in the tests by setting IF in CPU state,
because the STI profile requires a session without a clock. See the
[S4-28b report](../reports/STAGE_4_S4-28B_2026-11-09.md).

## CPU-owned IA32_APIC_BASE enable (S4-25b)

The opt-in `od_isa::msr::ApicBaseCpu` profile accepts only bare `RDMSR` and
`WRMSR` to index `0x1B` in Long64/CPL0. Its reset value is `0xFEE00900`: fixed
xAPIC base `0xFEE00000`, BSP bit set and global enable set. The supported
disabled value is `0xFEE00100`. `RDMSR` returns the value in EDX:EAX. A
successful `WRMSR` retires once and returns an `ApicBaseEffect`;
`step_apic_base_msr` synchronously applies it to the matching
`LocalApicHandle` before returning. The helper rejects mismatched CPU and
device enable state before executing. Same-value writes preserve device state.
A global enabled-to-disabled transition resets local APIC state and blocks
MMIO and acknowledge; re-enabling it leaves SVR software-disabled until the
guest explicitly enables SVR.

Reserved low bits and physical-address bits above this profile's 36-bit range
fault with `#GP(0)`. Unsupported fixed-profile values, including relocation,
BSP changes and x2APIC enable, fail closed without retirement. Active STI
shadow is rejected before fetch because this slice does not integrate
interrupt delivery. This caller-selected profile does not broaden existing
MSR profiles or automatically attach to `PagedVcpuSession`. Direct users of
`ApicBaseCpu::step` must apply a returned effect before the next guest or
device operation; the machine helper provides this ordering for the fixed
xAPIC device. CPU interrupt delivery, IDT/IF/handler semantics and HLT wakeup
remain outside the contract. See the [S4-25b report](../reports/STAGE_4_S4-25B_2026-11-07.md)
and [verification method](../testing/MACHINE.md#s4-25b-cpu-owned-ia32_apic_base-enable).

## xAPIC Fixed self-IPI through ICR (S4-25c)

The fixed xAPIC exposes the ICR low/high doublewords at offsets `0x300` and
`0x310`. `ICR_HIGH` stores only the destination byte; writing it alone does
not send. The supported command is an aligned Dword write to `ICR_LOW` with
Fixed delivery, Self destination shorthand, edge trigger, asserted level and
vector `16..=255`. Destination mode is ignored for Self. A valid low write
stores the command and sets the vector's IRR bit under the same device-state
lock. The internal one-CPU self-delivery completes synchronously, so Delivery
Status reads Idle after the operation; a written Delivery Status bit is
ignored because it is read-only. Idle means sending completed, not that the CPU
accepted the interrupt or executed a handler. Repeated commands are issued
again and outstanding equal vectors coalesce through IRR. Software disable
permits the request to enter IRR but blocks eligibility/acknowledge;
global APIC disable rejects ICR MMIO, and reset/global disable clears both ICR
halves.

This paragraph is the S4-25c state; since S6-12a the ICR accepts the wider set
of [Доставка IPI (S6-12a)](#доставка-ipi-s6-12a), of which Fixed Self with
`(Level, Trigger) = (1,0)` is one row. Everything else, and invalid widths or
alignment, fails without mutation through the local `DeviceError` contract,
not guest exception delivery. No ESR, CPU
interrupt acceptance, handler delivery or HLT wakeup is modeled. Behavior is
based on Intel [SDM release 093, Volume 3A](https://cdrdv2.intel.com/v1/dl/getContent/671190),
§§13.6.1–13.6.2.3, Table 13-3 and §§13.8.3–13.8.5. See the [S4-25c report](../reports/STAGE_4_S4-25C_2026-11-07.md)
and [verification method](../testing/MACHINE.md#s4-25c-xapic-fixed-self-ipi).

## Bounded CR8 ↔ xAPIC TPR (S4-25d)

The opt-in `Cr8Long64V1` ISA profile admits only `44 0F 20 C0`
(`MOV RAX, CR8`) and `44 0F 22 C0` (`MOV CR8, RAX`) in Long64 at CPL0.
The standalone machine adapter additionally requires a running CPU, clear
interrupt shadow, and enabled global and software Local APIC. Reads return
zero-extended `TPR[7:4]`; writes accept only values whose bits `63:4` are zero,
then set `TPR[7:4]` from `CR8[3:0]` and clear `TPR[3:0]`. TPR remains the sole
state source in `LocalApic`.

Privilege and reserved-bit violations produce `#GP(0)`; the exact prefixed
LOCK forms produce `#UD`. The profile checks each instruction-fetch address
for canonical48 before accessing guest memory. Unsupported encodings and
machine states stay closed. A successful write returns a typed effect, which
the machine helper applies before returning. Callers must serialize all APIC
operations across the full helper call, including fetch and effect application.
The instruction/helper makes no architectural serialization guarantee.

This profile does not implement general MOV CR8, interrupt/handler delivery,
IDT/IF, HLT wakeup, JIT/session integration, or default board wiring.

**S7-32 update (2026-11-26).** The CPL0 profile set has a separate member,
`Cpl0ProfileSet::with_cr8_tpr`, for `MOV r64, CR8` and `MOV CR8, r64` with any GPR (REX.R
forms): the set keeps CR8 in a cell, and the unified loop of the Windows guest
(`--cpu-loop unified --guest windows`) binds the cell to the boot vCPU's TPR (`Cr8TprLink`).
At every instruction boundary, before an interrupt is selected, a CR8 write becomes
`TPR = CR8 << 4` (`TPR[3:0]` cleared, also when the class is unchanged after an MMIO write
that set them), and after any device access (an MMIO TPR write) the cell is read back from
`TPR[7:4]`; a CR8 write retired as the last instruction of a run is applied after the run.
Interrupt delivery and HLT wakeup of that loop therefore see the CR8 value; the turbo engine
does not translate `MOV CR8` (it ends its blocks there). TPR remains the sole state source.
Other boards and loops do not have the member: a set with it must run with the binding (a
debug assertion). One vCPU only; INIT does not resynchronise the cell (no INIT on this loop). Behavior
follows Intel [SDM release 093, Volume 3A](https://cdrdv2.intel.com/v1/dl/getContent/671190),
§§13.8.6–13.8.6.1, and [Volume 2B](https://cdrdv2.intel.com/v1/dl/getContent/671241),
MOV to/from control registers. See the [S4-25d report](../reports/STAGE_4_S4-25D_2026-11-07.md)
and [verification method](../testing/MACHINE.md#s4-25d-bounded-cr8--xapic-tpr).

## Local APIC: spurious, LVT masks, reset matrix, приоритет и EOI (S4-25e)

Срез не закрывает родителя S4-25 (открыт до S4-25f) и не добавляет регистров, кроме двух публичных
операций на `LocalApicHandle`. Модель остаётся одним xAPIC с полем ID 0.

**Spurious.** `spurious_vector()` возвращает `SVR[7:0]` (или `None` при global
disable). `acknowledge_or_spurious()` решает на момент acknowledge, а не по
прежнему выбору: если есть eligible запрос, самый старший вектор IRR переходит
в ISR (`InterruptAcknowledge::Fixed`); иначе возвращается
`InterruptAcknowledge::Spurious(SVR[7:0])` без изменения IRR, ISR, TMR, TPR и
PPR (запрос снят reset, скрыт повышенным TPR/ISR или software disable). Spurious
вектор не попадает в ISR, поэтому гость не должен посылать за него EOI. При
гонке «после выбора пришёл более старший вектор» acknowledge вернёт новый
старший вектор, а `acknowledge_vector(выбранный)` откажет без изменений: CPU
путь (S4-27) по-прежнему заканчивает такой случай `InterruptSelectionChanged`.
Spurious разрешён при global enable независимо от SVR bit 8, потому что
software disable может снять запрос между выбором и acknowledge; биты 3:0
вектора не фиксируются. Источник: Intel SDM 093, Vol. 3A §13.9.

**LVT mask.** Реализован один LVT-элемент, Timer (`0x320`, только при opt-in
таймере). Bit 16 проверяется в момент expiry: masked expiry снимает deadline
(periodic перезаряжается), не изменяет IRR/ISR/TMR и не воспроизводится при
снятии маски; событие теряется по контракту, а не откладывается. Остальные LVT
(CMCI `0x2f0`, Thermal `0x330`, Perf `0x340`, LINT0/1 `0x350`/`0x360`, Error
`0x370`), Version (`0x30`) и ESR (`0x280`) не были реализованы (LINT0/1, Error,
Version и ESR добавлены в S4-25f, Perf — в S7-32, срез M; CMCI и Thermal остаются так): чтение и запись дают
`UnknownOffset` без изменения состояния. Без таймера отсутствуют и `0x320`,
`0x380`, `0x390`, `0x3e0`.

**Reset-матрица** (свежее устройство и `reset`, `reset_checked`, `reset_pair`,
global disable→enable; `reset` при Remote IRR отклоняется без изменений):

| Регистр | Значение после reset |
|---|---|
| ID `0x20`, TPR `0x80`, PPR `0xa0` | `0` |
| SVR `0xf0` | `0xff` (software disabled, spurious vector `0xff`) |
| ICR low/high `0x300`/`0x310` | `0` |
| ISR, TMR, IRR (`0x100..0x270`, 24 слова) | `0` |
| LVT Timer `0x320` | mask (`0x10000`) |
| Initial/Current/Divide `0x380`/`0x390`/`0x3e0` | `0`, deadline отменён |
| Version `0x30` | `0x00040014` (с S7-32, срез M; с S4-25f — `0x00030014`; до него `UnknownOffset`) |
| LVT PMC `0x340` | mask (`0x10000`) (с S7-32, срез M) |

**Приоритет и EOI.** Eligible только самый старший вектор IRR и только если его
класс (bits 7:4) строго выше класса PPR; PPR берёт TPR, если класс TPR не ниже
класса самого старшего ISR, иначе класс ISR. Равный класс блокирует. EOI снимает
самый старший ISR, не трогая IRR, TMR и TPR; EOI при пустом ISR принимается без
изменений.

**Ошибочные обращения** (прежний контракт S4-25a–d, теперь проверенный
перебором): неверное пространство, ширина не Dword, смещение не кратно 16,
неподдержанное смещение, reserved-биты записываемых регистров (TPR >`0xff`, SVR
>`0x1ff`, EOI ≠ 0, ICR/ICR high, Timer LVT/Initial/Divide), запись в read-only
регистры и обращения при global disable возвращают `DeviceError` без изменения
состояния. Значения шире 32 бит усекает шина по ширине обращения; само
устройство отклоняет их.

Не входят: x2APIC, multi-CPU startup, NMI и другие LVT-источники, IOAPIC
(S4-26c), CPU delivery, IDT/IF, HLT wakeup, JIT, Linux. Behavior: Intel
[SDM release 093, Volume 3A](https://cdrdv2.intel.com/v1/dl/getContent/671190),
Chapter 13. См. [отчёт S4-25e](../reports/STAGE_4_S4-25E_2026-11-08.md) и
[методику](../testing/MACHINE.md#s4-25e-local-apic-закрытие).

## Local APIC для Linux probe: Version, LINT0/LINT1, Error LVT, ESR, DFR/LDR (S4-25f)

Срез снимает `DeviceError` с регистров, которые читает и пишет Linux-подобный
probe APIC. Все новые регистры только хранят состояние: источников LINT-пинов
и ошибок нет, доставка с них не реализована. Остальные правила (Dword, шаг 16,
отказ при global disable, отсутствие мутации при отказе) прежние.

**Version `0x30`.** Значение `0x00030014`: версия `0x14` (integrated APIC,
`0x10..0xff`), Max LVT Entry = 3, то есть четыре LVT (Timer, LINT0, LINT1,
Error), bit 24 (EOI-broadcast suppression) = 0. Значение согласуется с SDM
Vol. 3A §13.4.8: Max LVT = число LVT минус 1 (Pentium: 4 LVT → 3). Версия `0x14`
типична для P6/P4; других расхождений с SDM нет. Запись возвращает `ReadOnly`.
Version доступен и без opt-in таймера, хотя Timer LVT без него отсутствует:
Version описывает профиль APIC, а таймер остаётся opt-in моделью (Linux читает
Max LVT до таймера и не сверяет его с наличием `0x320`).
*Обновление S7-32, срез M (2026-11-26):* Version = `0x00040014`, Max LVT Entry = 4 — пять LVT
(Timer, Performance Monitoring Counters, LINT0, LINT1, Error), раскладка P6 по SDM Vol. 3A §13.4.8
(P6: 5 LVT → 4). Thermal Sensor и CMCI в модели нет (абзац «Thermal `0x330`, CMCI `0x2f0`» ниже).

**LINT0 `0x350`, LINT1 `0x360`.** Reset `0x10000` (masked). Записываются vector
(7:0), delivery mode (10:8), polarity (13), trigger (15) и mask (16); допустимые
delivery mode: Fixed, SMI, NMI, INIT, ExtINT (`000/010/100/101/111`).
Delivery Status (12) и Remote IRR (14) read-only: при записи отбрасываются и
читаются нулём (как Delivery Status в ICR S4-25c; Timer LVT S4-28a отклоняет
запись этих битов, это различие осознанное: Linux записывает значения,
прочитанные из регистра). Bit 11, bits 31:17 и delivery mode `001/011/110`
вызывают `UnsupportedConfiguration` без изменения. Vector не валидируется (в
том числе < 16): LVT ничего не доставляет, проверка вектора принадлежит срезу с
доставкой.

**Error LVT `0x370`.** Reset `0x10000`. Записываются vector (7:0) и mask (16);
bit 12 отбрасывается; bits 10:8, 11, 13..15 и 31:17 зарезервированы.

**Mask и software disable.** Mask хранится и читается. Software disable SVR bit 8
= 0 устанавливает mask во всех хранимых LVT (LINT0, LINT1, Error; с S7-32, срез M, и PMC)
(SDM §13.4.7.2); снятие mask при
software disable отклоняется (`UnsupportedConfiguration`, без изменений), как
для Timer LVT. Запись masked значения разрешена. Mask не влияет на IRR/ISR/TMR:
ни одна запись в LINT/Error/PMC LVT не создаёт запрос.

**ESR `0x280`.** SDM §13.5.3: запись защёлкивает в ESR ошибки, накопленные с
прошлой записи, и очищает лог; записанное значение игнорируется (принимается
любой 32-битный; шире — `UnsupportedConfiguration`). Чтение возвращает
защёлкнутое значение и ничего не очищает. Reset обнуляет оба. Источников ошибок
в модели нет, поэтому лог всегда пуст, и ESR читается как 0. Выставление
ошибок (illegal vector, send/receive accept, redundant vector) относится к
срезу с доставкой. Запись ESR не меняет IRR, ISR, TMR и остальные регистры.

**DFR `0xe0` и LDR `0xd0`.** Не сверено с исходником Linux 7.0 (ссылка по памяти). Нужны: `setup_local_APIC` в Linux вызывает
`init_apic_ldr` (flat/cluster driver пишет DFR и LDR и читает LDR), поэтому без
них probe получит `DeviceError`. Хранение без logical destination routing.
DFR reset `0xffffffff` (flat, SDM §13.6.2.2): записывается только model
(31:28), допустимы `0xf` (flat) и `0x0` (cluster); bits 27:0 читаются единицами
и при записи игнорируются; другой model отклоняется. LDR reset `0`: записывается
только logical ID (31:24); ненулевые bits 23:0 отклоняются. Logical destination
по этим регистрам не вычисляется, ICR принимает прежний набор (Self shorthand).
*Обновление S7-32 (2026-11-26):* logical destination вычисляется для записей IOAPIC, MSI-X
и ICR (SDM Vol. 3A 13.6.2.2): flat — общий бит адреса и `LDR[31:24]`, cluster — тот же
кластер (биты 7:4) и общий бит члена (биты 3:0); другие модели не совпадают ни с чем.
Fixed по логическому адресу доставляется всем совпавшим vCPU, lowest priority — одному,
первому совпавшему (арбитраж между несколькими не моделируется). Физический
широковещательный адрес `0xFF` не моделируется.

**LVT Performance Monitoring Counters `0x340` (S7-32, срез M, 2026-11-26).** Ядро Windows
(`ntkrnlmp` + `0x569990`, инициализация локального блока HAL) пишет `0x340` = `0x100D8` безусловно,
Max LVT не читает ([ISSUE-700](../KNOWN_ISSUES.md#issue-700)); позже HAL программирует линию PMI
(`+0x33e19d`: Fixed, вектор `0xFE`). Регистр — хранение и маска, как LINT/Error. Reset `0x10000`
(masked) во всех путях сброса, в том числе INIT (SDM Vol. 3A §13.4.7.1, §13.4.7.3). Записываются
vector (7:0), delivery mode (10:8) и mask (16); Delivery Status (12) read-only — при записи
отбрасывается и читается нулём (Idle), как у LINT/Error (Linux пишет прочитанное значение
`v | MASKED`). Допустимые delivery mode: Fixed, SMI, NMI (`000/010/100`); `101` (INIT) и `111`
(ExtINT) для PMC не поддерживаются, `110` зарезервирован для всех LVT, `001/011` не кодируют режим
(§13.5.1); бит 11, polarity (13), Remote IRR (14), trigger (15) и биты 31:17 у PMC зарезервированы
(Figure 13-8). Такие записи — `UnsupportedConfiguration` без изменения, в том числе маскированные.
Vector не валидируется (в том числе < 16). Software disable ставит mask, снятие mask при нём
отклоняется (абзац «Mask и software disable»). Источника PMI нет: ни один CPUID-профиль не
объявляет PMU (максимальный базовый лист — 1 у Linux-профилей и 6 у Windows-профиля с S7-32, срез O; листа 0xA нет ни у одного), счётчиков нет, запись не создаёт
запросов; автоматическая установка mask при PMI (§13.5.1) не моделируется. Регистр PMC
существует независимо от объявления PMU: он появился в P6, до архитектурного PMU, и SDM называет
его implementation specific (§13.5.1, Table 13-1). Version одна для всех профилей платы: Windows
Max LVT не читает. Linux 7.0 при Max LVT > 3 пишет `ESR = 0` перед чтением ESR: дважды на каждом
CPU в `lapic_setup_esr` (`apic.c:1411`, `1422`) и при запуске каждого AP ещё пять раз — один в
`send_init_sequence` перед INIT (`smpboot.c:832`) и по два вокруг каждого из двух STARTUP в
`wakeup_secondary_cpu_via_init` (`smpboot.c:880`, `916`; результат `ESR & 0xEF` должен быть 0);
запись ESR модель поддерживает (S4-25f). `LVTPC` Linux маскирует лишь в `clear_local_APIC`
(выключение, hotplug).

**Thermal `0x330`, CMCI `0x2f0`.** Не реализованы: чтение и запись дают `UnknownOffset` без
мутации; Max LVT = 4 их не включает. Условие (закреплено тестом
`absent_thermal_and_cmci_lvts_match_the_cpu_identity`): ни один CPUID-профиль не объявляет ACPI
(лист 1 EDX 22), TM (EDX 29), TM2 (ECX 8), HWP с уведомлением и HFI (лист 6 EAX 7, 8, 19), а
`IA32_MCG_CAP.MCG_CMCI_P` (бит 10) = 0. При этом гости к ним не обращаются: Linux при ACPI и TM в CPUID
независимо от Max LVT читает `LVTTHMR` без проверки vendor (`therm_lvt_init`, вызов из
`setup.c:1263`, `therm_throt.c:718–719`), а пишет его только для vendor Intel (`intel_init_thermal`
из `init_intel`, `cpu/intel.c:625`), а в `clear_local_APIC` — при Max LVT ≥ 5;
`LVTCMCI` — при vendor Intel, Max LVT ≥ 6 и `MCG_CMCI_P` (`cmci_supported`) или в `clear_local_APIC`
при Max LVT ≥ 6; Windows пишет CMCI только при `MCG_CAP[10]` (`+0x569bb4`),
а Thermal — только когда `intelppm.sys` регистрирует тепловое прерывание
(`HalPrivateDispatchTable+0x3A8`), что он делает лишь при `CPUID.06H:EAX` с HWP и уведомлением или
HFI. При изменении любого из этих условий LVT добавляются вместе с Max LVT.

Не входят: доставка с LINT-пинов, NMI/SMI/INIT/ExtINT, доставка Error LVT,
выставление ESR, источник PMI и автоматическая маска PMC при PMI, x2APIC, multi-CPU,
IOAPIC (S4-26c), CPU delivery, CPUID, Linux acceptance (S4-38). См. [отчёт S4-25f](../reports/STAGE_4_S4-25F_2026-11-09.md)
и [методику](../testing/MACHINE.md#s4-25f-local-apic-для-linux-probe).

## Fixed-edge IOAPIC route (S4-26a)

The opt-in `IoApic` model occupies MMIO base `0xFEC00000` and supports one
route: INTIN4 to physical Local APIC ID 0. Dword accesses use IOREGSEL and
IOWIN at offsets `0x00` and `0x10`; redirection entry 4 is selected with
`0x18` (low) and `0x19` (high). The route accepts only Fixed delivery, physical
destination 0, active-high edge mode, mask and vectors `0x10..=0xFE`. Reset
leaves it masked. Other selectors/registers and routes are unsupported; no
Version value is exposed for the partial table.

The external line API records INTIN4 level. Only low-to-high transitions
through an unmasked route request a Local APIC fixed edge. A masked edge is
ignored; unmask and high-to-high do not replay it. Repeated accepted vectors
coalesce in the existing Local APIC IRR. If the destination rejects the
request, the call errors without consuming the line transition, so the owner
can retry. The owner must serialize operations on both APICs, including reset
and enable changes. The low redirection word's read-only Delivery Status and
Remote IRR bits read zero and are ignored when written.

This is not a full IOAPIC: other entries, level/Remote-IRR delivery, logical
destinations, APIC bus behavior, CPU acceptance/handler delivery, HLT wakeup,
default board wiring and Linux discovery are outside the contract. Device
errors do not model guest exception delivery. Register behavior follows Intel
[82093AA I/O APIC document 290566-001](https://intel-vintage-developer.eu5.org/DESIGN/PCISETS/DATASHTS/29056601.PDF),
preliminary release, §§3.1–3.2.4. See the [S4-26a report](../reports/STAGE_4_S4-26A_2026-11-07.md)
and [verification method](../testing/MACHINE.md#s4-26a-fixed-edge-ioapic-route).

## Fixed-level IOAPIC route and Local APIC EOI (S4-26b)

The same single INTIN4 → physical Local APIC ID 0 route may be configured for
active-high Fixed level delivery. On acceptance, the Local APIC sets the vector
in IRR and TMR and the IOAPIC sets Remote IRR. Acceptance is independent of
TPR eligibility. Acknowledge moves the vector to ISR without clearing TMR or
Remote IRR. A matching Local APIC EOI clears ISR and Remote IRR; if INTIN4 is
still asserted and the route is unmasked, the next request is accepted
atomically with that EOI. Deassertion does not cancel an already accepted
request. Masking after acceptance does not cancel it; EOI while masked clears
Remote IRR without retrigger, and unmasking an asserted line may accept a new
request. Acceptance requires both Local APIC global enable and
`SVR.SoftwareEnable`; an assertion that would immediately deliver an
unmasked level request, or unmasking an asserted level route, fails before the
line/configuration transition if acceptance is disabled. A masked assertion
may still record the high input level without delivery. TPR affects
eligibility only.

The configured level vector is reserved from the bounded edge/self-IPI path,
even while its route is masked or its line is inactive, so one vector's TMR/EOI
cannot ambiguously identify edge and level requests. This profile supports no
other level routes or vector sharing.

One shared owner state serializes route and Local APIC transitions under a
single mutex. There are no cross-device callbacks or nested locks. A level
route cannot change vector or trigger mode while its line is high, Remote IRR
is set, or either related vector is pending/in service. Edge→edge vector
reprogramming preserves the S4-26a behavior, including an already pending old
vector. Independent device resets preserve the other controller; they fail
without mutation while Remote IRR is outstanding. `reset_pair` is the explicit
coordinated operation that discards both controllers' state. The caller
serializes the full CPU helper and device operations; guest instruction fetch
does not run under the pair mutex. Global disable and SVR software disable are
also vetoed without mutation while Remote IRR is outstanding.

The APIC-base CPU helper executes on a candidate CPU state and publishes it
only after the corresponding Local APIC transition succeeds. A host-side
transition veto while Remote IRR is set leaves CPU retirement and device state
unchanged. This is a `DeviceError`, not a guest `#GP`; any fetch or page-walk
effects already committed before the veto are not rolled back.

TMR acceptance and EOI semantics follow Intel [SDM 253668-093US, September
2026, Volume 3A §§13.8.4–13.8.5](https://cdrdv2.intel.com/v1/dl/getContent/671190).
The bounded IOAPIC route follows [Intel 82093AA I/O APIC document 290566-001,
preliminary release, §3.2.4](https://intel-vintage-developer.eu5.org/DESIGN/PCISETS/DATASHTS/29056601.PDF).
See the [S4-26b report](../reports/STAGE_4_S4-26B_2026-11-07.md) and
[verification method](../testing/MACHINE.md#s4-26b-fixed-level-ioapic-route).

## IOAPIC ID/Version, окно регистров и недопустимые записи (S4-26c)

Окно `IoApic` (`0xFEC00000`, dword): IOREGSEL на смещении `0x00` (8 бит, значение
больше `0xFF` — `UnsupportedConfiguration` без изменений), IOWIN на `0x10`.
Селекторы IOREGSEL принимают любое значение 0..=0xFF; доступ через IOWIN к
неизвестному селектору (`0x03..=0x0F`, `0x40..=0xFF`) — `UnknownOffset` без
изменения состояния (на запись и чтение), как и в S4-26a/b. Селектор после
неизвестного значения сохраняется (он не часть конфигурации маршрутов).

- **ID (`0x00`).** Читается `(ID << 24)`, reset-значение ID = 1: это ID, который
  MADT (S4-31f, `IOAPIC_ID`) объявляет прошивкой; у модели нет стадии прошивки,
  поэтому значение MADT является reset-значением. Биты 27:24 — RW (ID — 4 бита, по
  документу 82093AA, как его принимает S4-26a); запись с ненулевыми reserved-битами
  (31:28, 23:0) — `UnsupportedConfiguration` без изменений. ID на маршрутизацию
  не влияет. Независимый `IoApicHandle::reset` и `reset_pair` возвращают ID = 1.
- **Version (`0x01`).** `0x00170020`: версия `0x20`, MRE (23:16) = 23, то есть
  24 входа. Read-only: запись любого 32-битного значения принимается и ничего не меняет
  (с S7-32, срез N; до него — `UnsupportedConfiguration`); значение шире 32 бит, переданное
  прямо устройству, — `UnsupportedConfiguration` без изменений (через dword-доступ шины оно не
  приходит). HAL Windows проверяет IOAPIC, дважды записывая 0 в Version и сравнивая прочитанные
  версию и MRE ([ISSUE-701](../KNOWN_ISSUES.md#issue-701)).
- **Arbitration ID (`0x02`).** Читается как `ID << 24` (82093AA загружает его из
  ID при записи ID; шины APIC в модели нет, арбитража тоже). Read-only: запись — как в Version,
  принимается и ничего не меняет (с S7-32, срез N). Version локального APIC при этом по-прежнему
  отвечает на запись `ReadOnly` (S4-25f): туда не пишут ни Linux 7.0, ни HAL Windows на пройденном
  пути.
- **Redirection entries.** Селекторы `0x10..=0x3F`: вход `n = (select - 0x10) / 2`,
  чётный селектор — low dword, нечётный — high; `n = 0..=23`. Вход 4 — прежний
  маршрут S4-26a/b (все проверки конфликтов и Remote IRR сохранены; Remote IRR
  читается в бите 14). Остальные 23 входа хранят low dword (после reset
  `0x00010000`, masked), у них нет входной линии (`set_intin4` относится только к
  INTIN4), поэтому они никогда ничего не доставляют, Remote IRR всегда 0.
  Запись low принимается при Fixed delivery (биты 10:8 = 0), physical destination
  mode (бит 11 = 0), active-high (бит 13 = 0), нулевых битах 31:17; исключение —
  входы без линии (все, кроме INTIN4): под маской разрешён бит 13 = 1 (active-low;
  MADT объявляет SCI на GSI 9 как level/active-low, flags `0x000F`), запись без маски
  с бит 13 = 1 — `UnsupportedConfiguration` без изменений, кроме одной формы (S4-38L8e):
  pin 9 с бит 15 = 1 (level) и бит 13 = 1 (active-low) принимается и без маски (вектор
  `0x10..=0xFE`, Fixed, physical), это ровно то, что описывает ISO MADT (IRQ 9 → GSI 9,
  flags `0x000F`); INTIN4 остаётся только
  active-high даже под маской (и INTIN3, когда плата подключила линию COM2 опцией `agent_channel`, S11-21: он получает правила INTIN4 — линия, уровень с Remote IRR, резерв вектора, active-high; без опции INTIN3 — вход без линии, как описано выше); биты 12 и 14
  (read-only) отбрасываются. Vector: для unmasked записи `0x10..=0xFE`; для masked
  записи допустим любой байт (reset-состояние уже masked с vector 0, а Linux
  маскирует входы с vector 0). Расширения относительно S4-26a: любой vector под маской и active-low под маской на входах без линии (SCI GSI 9):
  прежние отказы (vector 0/15/0xFF без маски) сохранены. High dword: принимается
  только 0 (physical destination 0, то есть Local APIC ID 0; reserved 23:0 = 0);
  любое другое значение — `UnsupportedConfiguration` без изменений. (С S6-11 принимается
  APIC ID любого существующего vCPU, при одном vCPU — по-прежнему только 0; см.
  [LAPIC на каждый vCPU](#lapic-на-каждый-vcpu-s6-11).)

**Недопустимые записи** (vector < 16 при unmasked, `0xFF` при unmasked, любой
delivery mode кроме Fixed — lowest priority, SMI, NMI, INIT, ExtINT и зарезервированные
`011`/`110`, polarity (кроме masked на входах без линии и формы level/active-low pin 9, S4-38L8e), reserved-биты, ненулевой high dword) всегда
`UnsupportedConfiguration` и не меняют ни один вход, ID, Remote IRR, IRR/ISR/TMR.
Тесты сравнивают полный снимок `PairState` до и после.
*Обновление S7-32 (2026-11-26):* delivery mode lowest priority (`001`) и logical destination
(бит 11) принимаются на любом входе (их программирует HAL Windows); SMI, NMI, INIT, ExtINT и
зарезервированные режимы по-прежнему отказ. Адресат хранится как записан (чтение high dword
возвращает его): пока запись маскирована, принимается любой байт адресата и ничего не
разрешается; запись low dword, снимающая маску, разрешает его по режиму из этой записи
(physical — APIC ID существующего vCPU, logical — по LDR/DFR); адресат, не называющий ни
одного vCPU, у немаскированной записи — отказ. Отказ любой из двух записей восстанавливает
весь снимок `PairState` (и маршрут, и запросы LAPIC). Разрешение выполняется в момент записи:
позднее изменение LDR/DFR на уже разрешённые записи не влияет.

**S4-38L8e: SCI на pin 9 (level, active-low).** Запись `0xa020`-формы (бит 15 и бит 13 = 1, без маски)
в low dword pin 9 принимается (раньше отказ, на нём остановился прогон L8d). Модель: `sci_asserted`
(физический уровень низкий, источник вызывает `IoApicHandle::set_sci_line(asserted)`) и
`sci_remote_irr`; семантика та же, что у уровневого INTIN4: доставка при снятой маске и поднятой линии
ставит IRR/TMR и Remote IRR, EOI адресата снимает Remote IRR и, пока линия поднята, а запись не
маскирована, доставляет повторно; маска блокирует доставку, повторная запись или поднятие линии при
Remote IRR второй запрос не ставят; вектор уровневого pin 9 резервируется против фронтов и уровневого
INTIN4 того же LAPIC; Remote IRR вето на сброс IOAPIC и отключение LAPIC. Pin 9 в других неотказных
формах (например, level/active-high) хранится без линии, `set_sci_line(true)` при такой записи без маски —
`UnsupportedConfiguration` без изменений. Источника SCI в плате пока нет (его линию никто не поднимает).
Резерв вектора уровневой формы pin 9 действует и под маской: маскированная SCI-форма с вектором V отвергается, если V уже занят (разблокированный entry 2, уровневый INTIN4, LVT Timer того же LAPIC), и наоборот: пока pin 9 в SCI-форме держит V, уровневый INTIN4, запись entry 2 и LVT Timer с V отвергаются, а фронт INTIN4 с V не доставляется (`edge` отказывает). Принятые пути не затронуты: UART probe пишет только INTIN4 = 0x51, pin 9 остаётся MASK; окна IOAPIC под stage4a закрыты. Для среза источника SCI: если линия поднята до настройки ОС (`sci_asserted`), переход записи pin 9 в SCI-форму или смена её вектора отвергается (`UnsupportedConfiguration`, как у INTIN4): ОС должна настроить запись до подъёма линии, либо источник поднимает её позже. Другие входы: active-low без маски по-прежнему отказ. Профиль stage4a окна IOAPIC не открывает (прежняя
типизированная ошибка шины).

**Ограничения S4-26c.** (а) [заменено S4-38L8e для pin 9: форма level/active-low без маски принимается;
прочие формы active-low без маски отвергаются.] (б) EOI-регистра IOAPIC по
смещению `0x40` нет (тот же `UnknownOffset` по общим
правилам окна), регистр `0x03` не моделируется (`UnknownOffset`); Version `0x20`
выбрана как у ICH/QEMU-style IOAPIC, не 82093AA. (в) Polarity в «Сделать» S4-26
покрыта только для active-high на INTIN4 и для маскированных входов без линии.
Родитель S4-26 закрывается только с этой оговоркой.

**Граница: logical destination.** Бит 11 = 1 и любой destination, кроме physical 0,
отклоняются; logical flat/cluster routing, lowest-priority и multiple IOAPIC не
моделируются. Риск: ядро Linux при logical flat mode (S4-25f «Открыто») запишет такие
записи в redirection entries — они будут отказаны, пока не появится logical routing
(S4-38 покажет, нужен ли он для одного CPU; physical mode с destination 0 принимается).
PCI MSI и дополнительные IOAPIC вне контракта.
*Обновление S7-32 (2026-11-26):* logical destination и lowest priority принимаются (см.
«Недопустимые записи» и DFR/LDR выше; один vCPU — первое совпадение). MSI-X контроллера NVMe
принимает в адресе RH (бит 3) и DM (бит 2, logical), в данных — delivery mode fixed или lowest
priority и фронт; остальное остаётся в PBA. Дополнительные IOAPIC и обычный MSI вне контракта.

**Согласование с MADT.** MADT прошивки (`firmware/uefi/src/acpi/madt.rs`, константы
`IOAPIC_ID = 1`, `IOAPIC_PIN_COUNT = 24`, блоб-фикстура
`tests/fixtures/guest/stage4/acpi_blob/stage4_acpi_blob.bin`) и модель связаны
тестом `io_apic::tests::model_agrees_with_the_firmware_madt`: ID читается из байтов
MADT фикстуры, число входов (не поле MADT) — из константы прошивки; модель обязана
отдавать тот же ID и MRE = `IOAPIC_PIN_COUNT - 1`. Изменение любой из сторон ломает тест.

Источники: Intel [82093AA I/O APIC document 290566-001](https://intel-vintage-developer.eu5.org/DESIGN/PCISETS/DATASHTS/29056601.PDF),
preliminary release, §§3.1–3.2.4 (регистры и redirection table). В S4-26c
текст документа и SDM заново не перечитывался: поля ID (RW 27:24), Version (RO,
MRE 23:16), Arbitration ID (RO, загружается из ID) и раскладка redirection entry
были взяты из принятых S4-26a/b и памяти исполнителя. *Обновление S7-32, срез N
(2026-11-26):* сверено с текстом 290566-001 (PDF SHA-256 `600188e3…49c6b6`): Table 2 — ID R/W,
Version и Arbitration ID RO; §3.1.2 — доступ через IOWIN определяет выбранный регистр; §3.2.1 —
ID в битах 27:24, 31:28 и 23:0 reserved, при записи ID тем же значением загружается
арбитражный ID; §3.2.2 — MRE 23:16 и версия 7:0, RO; §3.2.3 — атрибут «Read Only»; §3.2.4 —
раскладка записи, Delivery Status (12) и Remote IRR (14) RO. Осознанные отличия модели от документа:
(1) версия `0x20` вместо `0x11` (ограничение (б)); (2) ID после сброса — 1 (ID из MADT, у модели нет
стадии прошивки), а не `00h` §3.2.1; (3) Arbitration ID после сброса не `0`, а `ID << 24`, и не
вращается при арбитраже (§3.2.3: загружается при записи ID и меняется по итогам арбитража шины APIC;
шины и арбитража в модели нет). Сам документ в §3.2.3 противоречив: атрибут регистра — «Read Only»,
а поле 27:24 в таблице битов помечено «R/W»; модель следует Table 2 и атрибуту (RO). См. [отчёт S4-26c](../reports/STAGE_4_S4-26C_2026-11-09.md) и
[методику проверки](../testing/MACHINE.md#s4-26c-ioapic-idversion-и-окно).

## Caller-driven APIC interrupt entry (S4-27a)

`DiagnosticVcpuSession::deliver_interrupt_and_step` is a synchronous
diagnostic bridge over the already implemented Local APIC, IOAPIC and
`od-isa::deliver_maskable_interrupt`. At a caller-certified completed
instruction boundary it selects one APIC-eligible vector and invokes the
Long64/CPL0 interrupt-entry helper against a candidate CPU state and the
session's flat identity `MachineBus` memory adapter. It does not hold the APIC
mutex while reading descriptors or writing the stack. After successful entry,
the selected vector is revalidated and moved from IRR to ISR while the CPU
candidate is published under the APIC owner lock; then the session runs the
handler with an instruction budget of one. This attempts at most one handler
instruction and returns its ordinary `DiagnosticRunResult`. The instruction
may fault, be unsupported, halt, or fail on backend memory before retiring. In
those cases the successful entry and APIC acknowledgement remain committed,
and the session reports its resulting run/terminal state.

The caller must serialize the complete operation with APIC/IOAPIC activity and
certify a fresh instruction boundary, no pending synchronous exception,
debug/NMI event, or other interrupt inhibition, private synchronous RAM, and
disabled CET/VMX/SMM/shadow-stack mechanisms required by the underlying ISA
helper. Supported state is Long64/CPL0, Running, IF set, explicit clear STI
shadow, same-CPL present interrupt gate with IST=0, and the existing
descriptor/profile constraints. This method is caller-driven; it does not poll
or schedule IRQs automatically.

With IF clear, blocking STI shadow, or no APIC-eligible vector, the operation
returns without changing CPU/APIC state. TPR-blocked requests remain pending
in IRR. An ISA `OutsideProfile` result does not acknowledge the request. A
`DeliveryAborted` may leave the already committed stack-write prefix in RAM;
the session enters a terminal state, the APIC request remains pending, and the
caller must not retry that session. On successful entry, the same vector enters
ISR and remains there until a later EOI; level Remote IRR remains asserted
until that EOI. The one-instruction-budget handler run does not add IRET or a
guest EOI routine.

See the [S4-27a report](../reports/STAGE_4_S4-27A_2026-11-07.md),
[verification method](../testing/MACHINE.md#s4-27a-caller-driven-apic-interrupt-entry),
and [evidence manifest](../reports/data/stage4/s4-27a-apic-cpu-entry.json).

## S4-27b. Guest EOI, same-CPL IRETQ and resume

The S4-27a diagnostic handler can issue one guest Dword store of zero to the
Local APIC EOI register at `0xFEE000B0`. The existing flat identity adapter
routes data writes through `MachineBus`, which dispatches this aligned Dword to
the Local APIC; instruction fetch remains RAM-only. No host-side EOI substitute
is used by the fixture.

`DiagnosticVcpuSession::run_same_cpl0_iretq` executes the existing
`od-isa::run_with_iretq_profile` with `SameCpl0ValidFrameV1` and a one-instruction
budget. A retired IRETQ consumes the five-qword same-CPL frame, restores the
interrupted RIP/RSP/CS/SS/RFLAGS, and commits the candidate CPU. The caller then
uses the ordinary diagnostic run boundary to execute the resumed instruction.
The profile requires private synchronous RAM, a valid same-CPL ring-0 frame,
`rflags_defined = INTEGER_FLAGS`, and the disabled mechanisms and CPU state
required by the underlying IRETQ contract; see
[CPU_CONTRACT.md](CPU_CONTRACT.md#s2-27k--exact-long64-same-cpl0-iretq).

The EOI is an architectural device side effect and is not rolled back if a
subsequent IRETQ fetch/frame/CS/SS read aborts. Such a read abort leaves the
pre-IRETQ CPU state uncommitted, makes the session terminal, and prevents
automatic retry or repeated EOI. With INTIN4 deasserted before EOI, Remote IRR
clears and no new request appears. If it remains asserted, matching EOI accepts
one new level request atomically into IRR; CPU delivery of that request waits
for a later explicit caller boundary. The diagnostic path does not add automatic polling,
cross-privilege IRETQ, paged IRETQ, JIT, or scheduler integration.

See the [S4-27b report](../reports/STAGE_4_S4-27B_2026-11-07.md),
[verification method](../testing/MACHINE.md#s4-27b-guest-eoi-same-cpl-iretq-and-resume),
and [evidence manifest](../reports/data/stage4/s4-27b-guest-eoi-iretq.json).

## S4-27c. Caller-driven STI-profile APIC quantum

`DiagnosticVcpuSession::run_sti_apic_quantum` is an opt-in one-instruction-
budget diagnostic operation. At each caller-supplied boundary it checks one
vector with `LocalApicHandle::eligible()`. If IF is set, the STI shadow is
clear, and the vector is eligible, it performs the existing Long64/CPL0 entry,
acknowledges that exact vector, and attempts one handler instruction. Otherwise
it attempts one instruction through the existing `NonPviLong64V1` STI
execution profile. This operation does not schedule or poll in the background.

The STI profile starts admission anew on each call. Its admitted bare
instructions are limited: with IF clear and shadow clear, CLI or STI; with IF
set and shadow clear, STI; with IF set and a blocking shadow, NOP, STI or CLI.
Thus a request remains pending across `CLI; STI; NOP` quanta and can be selected
on the next call after NOP clears the shadow. This does not support arbitrary
intervening guest instructions. The first handler instruction is subject to
the same profile; a successful entry and APIC acknowledgement remain committed
if that attempt is unsupported or faults.

Admission rejects before APIC selection or guest-memory access unless the
session is Runnable Long64/CPL0 with explicit Running activity and STI shadow,
`rflags_defined = INTEGER_FLAGS`, no configured TSC/RDTSC, no CR0/CR3/CR4, and
a tracked completed instruction boundary. A blocking STI shadow with IF clear
is rejected. The caller serializes CPU/APIC/IOAPIC operations and certifies no
pending synchronous, debug, NMI or MOV-SS inhibition, one vCPU, private
synchronous RAM, and the remaining ISA entry preconditions. Admission rejection
does not change guest or host session state.

A partial REP iteration-budget exit invalidates the tracked instruction
boundary. The new quantum, S4-27a interrupt entry and same-CPL IRETQ reject
that state until an ordinary `run()` retires the instruction. A fault or
unsupported execution remains terminal under the existing session rules.
HLT wakeup, MOV SS/NMI scheduling, TSC/RDTSC, arbitrary instruction execution,
JIT integration and automatic EOI/IRET are outside this slice.

Note (S4-27i2): the "no configured TSC/RDTSC" admission condition above is
relaxed by S4-27i2 for the clock; an RDTSC inside the STI quantum is still
rejected, but only in the single form `0F 31` (optionally one REX prefix); the
profile itself forbids RDTSC (other forms are terminal `Unsupported`) and the
rejection is wider than the real behaviour. See "Ослабление gate S4-27c по clock (S4-27i2)" in the S4-27i2
section below.

See the [S4-27c report](../reports/STAGE_4_S4-27C_2026-11-07.md),
[verification method](../testing/MACHINE.md#s4-27c-caller-driven-sti-profile-apic-quantum),
and [evidence manifest](../reports/data/stage4/s4-27c-sti-apic-quantum.json).

## S4-27d. Explicit HLT wait and maskable APIC wake

`DiagnosticVcpuSession::into_halt_wait` consumes an eligible diagnostic
session and attempts exactly one bare HLT with the existing
`HaltProfile::Long64V1`. Profile rejection, non-HLT, guest fault or memory
error returns ownership of the original uncommitted session. Successful HLT
retires once, leaves the CPU Halted at post-HLT RIP, and consumes a permitted
blocking STI shadow according to the ISA contract.

`DiagnosticHaltWait` owns the halted session. `poll_apic` performs no
instruction execution. IF clear or no `LocalApicHandle::eligible()` vector
returns Waiting without guest-memory access; priority-blocked vectors remain
pending. When a vector is eligible, the existing maskable entry helper uses a
candidate CPU and the flat identity memory adapter. Only a successful frame
write followed by acknowledgement of the same vector publishes the CPU as
Running and allows `into_running` to return the session. The saved RIP is the
post-HLT address. Handler execution, guest EOI and IRETQ remain explicit caller
operations; after IRETQ, ordinary execution resumes at the post-HLT RIP.

STOP while Waiting prevents future polling. STOP after successful entry keeps
the committed CPU/frame/APIC acknowledgement but changes session status to
Stopped and prevents recovery for handler execution. A delivery abort can
leave an already written frame prefix; CPU remains Halted, the request remains
pending, the wait becomes terminal, and retry is disallowed. Repeated polling
after successful entry reports the committed vector without selecting another
request. The caller serializes CPU/APIC/IOAPIC activity and certifies one vCPU,
private synchronous RAM, no pending synchronous/debug/NMI/MOV-SS inhibition,
and all existing ISA entry prerequisites.

The profile is limited to Runnable Long64/CPL0 sessions with explicit Running
activity and STI shadow, `rflags_defined = INTEGER_FLAGS`, no TSC/RDTSC and no
CR0/CR3/CR4. It requires a tracked completed boundary; a partial REP
iteration-budget exit is rejected. It adds no asynchronous arrival, timer,
general scheduler, other wake source, JIT integration or automatic handler
execution.

See the [S4-27d report](../reports/STAGE_4_S4-27D_2026-11-07.md),
[verification method](../testing/MACHINE.md#s4-27d-explicit-hlt-wait-and-maskable-apic-wake),
and [evidence manifest](../reports/data/stage4/s4-27d-hlt-apic-wake.json).

## S4-27e. Ordinary APIC quantum and pending IRQ during handler

`DiagnosticVcpuSession::run_apic_quantum` arbitrates one eligible maskable
vector at a caller-certified completed boundary. STOP is checked before APIC
selection or guest-memory access. If IF is set and `LocalApicHandle::eligible()`
returns a vector, the existing Long64/CPL0 entry helper constructs a candidate
frame; acknowledgement of the exact vector publishes the CPU, after which the
ordinary session runner attempts one handler instruction. `OutsideProfile` or
delivery abort never falls through to an ordinary quantum. After successful
entry, a handler fault, unsupported instruction or stop does not roll back the
frame or APIC acknowledgement.

If IF is clear or no vector is eligible, the ordinary runner attempts one
instruction with budget 1. TPR/PPR-blocked requests remain pending. Existing
`rflags_defined` and an explicitly configured RDTSC source are preserved. This
API requires Runnable Long64/CPL0 with Running activity, `Some(Clear)` STI
shadow, a tracked completed boundary, and no CR0/CR3/CR4; caller-side
serialization, private synchronous RAM and absence of pending synchronous,
debug, NMI or MOV-SS inhibition remain prerequisites. A partial REP
iteration-budget exit blocks a subsequent APIC quantum until ordinary execution
retires the instruction.

The caller-driven scenario posts a second vector while the first handler is
active. IF=0 keeps it pending through the guest EOI. After explicit IRETQ, the
next quantum delivers the second vector before fetching the resumed instruction.
The second EOI/IRETQ then permits the interrupted stream to continue. This
does not add automatic polling inside longer interpreter runs, JIT block exits,
event-queue arbitration, HLT integration or general interruptibility handling.

STOP before delivery sets the existing session status to Stopped and preserves
the pending APIC request; later quantum calls cannot resume it. The caller must
serialize complete CPU/APIC/IOAPIC operations.

See the [S4-27e report](../reports/STAGE_4_S4-27E_2026-11-07.md),
[verification method](../testing/MACHINE.md#s4-27e-ordinary-apic-quantum),
and [evidence manifest](../reports/data/stage4/s4-27e-apic-quantum.json).

## S4-27f1. Bounded JIT event handoff

`DiagnosticVcpuSession::run_jit_nop` is a separate opt-in diagnostic entry. The
caller supplies a stable `code_start`, the complete expected code bytes, an
instruction budget, dispatch control, a controlled block executor and a
caller-owned event flag. The method builds the exact `S0_09NopAdmission`
internally from the supplied code. Admission rejects an empty range, address overflow, a range crossing a
guest 4 KiB page, any non-RAM backing, or a byte mismatch before invoking the
executor. Every call revalidates the entire range; the current RIP must be a
valid admission boundary in that range. Keeping the original base and bytes on
resume preserves backward branches and previously executed prefix bytes.

The session must be Runnable in Long64/CPL0 with Running activity, an explicit
Clear STI shadow, no configured CR0/CR3/CR4 snapshot, `INTEGER_FLAGS`
definedness, and a caller-certified completed instruction boundary. The caller
serializes machine access, provides private synchronous RAM, and keeps code
bytes stable for the duration of the call.

The dispatch path uses the event-aware controlled executor. A pending event
observed during publication yields before native entry. `Yield(EventPending)`
from an executor is returned as an event exit after higher-priority exits, with
no guest progress counters attributed. The API never clears the caller's event
flag. The owner decides when the event is consumed and cleared before a later
call. Successful CPU state is published only for a valid successful dispatch
result. Profile, range, byte and admission rejections occur before executor
entry and preserve the session. A dispatch error may occur after executor entry;
it leaves the original CPU state uncommitted, marks the session Stopped and
preserves the caller's event flag. RAM-only preflight does not invoke a device
callback.

This evidence covers a bounded NOP fallback plus a real ARM64
`PublishedBlockExecutor` block, publication-window yield, explicit event clear,
and continuation through the same full code range without repeating the
prefix. It does not interrupt native execution asynchronously, deliver the
event through APIC, or compose JIT exits with APIC handler/EOI/IRETQ. General
scheduling, paged code admission and Linux acceptance remain outside this
slice; parent S4-27 remains open. See the
[S4-27f1 report](../reports/STAGE_4_S4-27F1_2026-11-07.md),
[verification method](../testing/MACHINE.md#s4-27f1-bounded-jit-event-handoff),
and [evidence manifest](../reports/data/stage4/s4-27f1-jit-event-handoff.json).

## S4-27f2. JIT event, APIC entry and resume

The bounded host integration scenario composes the S4-27f1 JIT diagnostic
entry with the existing one-vector APIC quantum and guest EOI/IRETQ path. A
real ARM64 `PublishedBlockExecutor` runs the native prefix; the caller-owned
event causes `EventPending` at the completed boundary before the NOP fallback.
The caller posts one APIC request after that exit, confirms
`LocalApicHandle::eligible()`, and calls `run_apic_quantum`. The resulting
same-CPL interrupt frame stores the exact JIT boundary and the APIC acknowledges
the selected vector once.

The guest handler reads the saved RIP and ISR bit, writes Dword zero to LAPIC
EOI through the normal `MachineBus` MMIO path, confirms the ISR bit cleared,
then executes the explicit same-CPL `IRETQ`. The caller clears its event flag
only after handling the event, then resumes `run_jit_nop` with the original
complete code range and remaining budget. The checks prove that the prefix is
not repeated, NOP retires once through the interpreter, and the native suffix
executes as a separate block. JIT dispatch counters and handler instruction
counters are asserted independently.

This is one caller-driven vector at a completed boundary with a synchronous
single owner. The event flag is only a notification and is not an APIC
acknowledgement. The scenario does not asynchronously interrupt a running
native block; it does not add a scheduler, general event queue, automatic
polling, NMI/MOV-SS delivery, paged entry, recovery after a guest EOI or Linux
acceptance. Parent S4-27 remains open. See the
[S4-27f2 report](../reports/STAGE_4_S4-27F2_2026-11-07.md),
[verification method](../testing/MACHINE.md#s4-27f2-jitapiceoiiretq-resume),
and [evidence manifest](../reports/data/stage4/s4-27f2-jit-apic-resume.json).

## S4-27g. Queued IRQ and JIT/interpreter parity evidence

The host integration test schedules one due and one future `ScheduledEvent`
before guest execution. Its synchronous test caller consumes the due event at
the completed native block boundary, requests one fixed Local APIC vector, and
sets the existing caller-owned notification. The same deadline and routing
sequence is replayed with the interpreter for the prefix. Both paths then use
the same explicit APIC entry, guest EOI MMIO write, same-CPL IRETQ and resume
contracts.

Assertions cover the JIT `EventPending` boundary before NOP (RIP, remaining
budget and separate counters), exactly one queue extraction, eligible vector
and frame, ISR before and after EOI, pause/resume preservation of notification
and the future event, and equality of final `LongModeState` and observed action
order between JIT and interpreter.

The queue-to-APIC routing here is test-caller orchestration only. It does not
add a production event pump, queue-driven HLT wakeup, concurrent producers,
automatic scheduling, or MOV-SS/NMI arbitration (the pump of S4-27i1..i3 and
S4-27i5 later added the owner loop; see the S4-27i5 section). Parent S4-27 remains open. See
the [S4-27g report](../reports/STAGE_4_S4-27G_2026-11-07.md),
[verification method](../testing/MACHINE.md#s4-27g-queued-irq-and-jitinterpreter-parity),
and [evidence manifest](../reports/data/stage4/s4-27g-queued-irq-jit-parity.json).

## S4-32c: freestanding stack probe

`Stage4StackProbeV1` is a strict raw-image profile layered on the existing
S4-32b fixed board. Its entry bytes are exactly `51 58 EE F4`: `PUSH RCX`,
`POP RAX`, `OUT DX,AL`, `HLT`. The artifact owns one payload qword at raw
offset `0x8000` / GPA `0x9000`; the loader passes that qword to `RCX` and does
not synthesize guest code or stack contents.

The fixed handoff starts in Long64/CPL0 with `RAX=0`, `RSP=0xa01000`,
`RDX=0x3f8`, and the existing CR3/EFER and four-level 4 KiB page tables. The
stack page is GPA `0xa000..0xafff`. A successful four-instruction run writes
the payload qword at GPA `0xaff8`, restores `RSP=0xa01000`, places the payload
in `RAX`, emits its low byte through COM1, advances `RIP` to `0x400004`, and
enters `Halted`. The opt-in `run_paged_stack_probe` selects
`PagedCpl0StackProbeV1` and owns decoding, page walking, MMU access and UART
dispatch through the same internal quantum as `run_paged_probe`. The legacy
`PagedCpl0ProbeV1` allowlist remains unchanged and still rejects stack forms;
the profile loader and runner do not duplicate those semantics.

The bounded evidence covers deterministic artifacts, budgets 0…4, split
single-instruction calls, stack canaries, halted repeat no-op and strict
corrupt-image rejection. A stack access failure preserves the current
instruction's register commit boundary while already committed page-walk
effects follow the existing tracked-memory contract. Exception delivery/repair,
CR2 update, two-page stack transactions, IRQ/APIC, HLT wakeup, JIT, Linux,
hardware and parent S4-32 acceptance are outside this profile; ISSUE-054 stays
open.

## S4-33a: guest RAM aliases

`Stage4AliasProbeV1` is a separate strict raw-image profile. Its page tables
map data GVA `0x800000` and stack GVA `0xa00000` to the same GPA page
`0x9000..0xa000`; the stack alias is inside the raw image backing. The image
contains different initial and store qwords. Guest `MOV [RBX],RCX` changes the
shared page and `POP RAX` reads the changed qword through the other GVA before
polling UART and halting. Result acceptance checks the stored qword and RAX;
HLT alone does not pass. The older S4-32 loaders continue to require their
original non-alias layouts. This evidence covers uncached interpreter access,
not TLB/JIT invalidation, cross-page access, or system acceptance.

## S4-33b: CPL3 SYSCALL/SYSRETQ probe

`Stage4Cpl3SyscallProbeV1` is a strict raw-image profile (image GPA `0x1000`,
size `0x9000`, 128 KiB RAM, no devices). The loader validates every page-table
entry, the exact code bytes and zero tails, and that the initial and store
qwords differ. Page tables map user code GVA `0x400000` (GPA `0x7000`, user
RX), kernel code GVA `0x600000` (GPA `0x8000`, supervisor RX, the LSTAR target)
and one data page GPA `0x9000` twice: GVA `0x800000` supervisor RW and GVA
`0x801000` user RO. The loader supplies the CPL3 entry state: CS `0x23`, SS
`0x1b`, RIP `0x400000`, EFER `0xd01`, STAR `(0x10<<48)|(8<<32)`, FMASK
`0x200`, RDI = kernel data GVA, RBX = user data GVA, R10 = store qword, RAX a
sentinel, RSP `0x702027`, CR3 `0x1000`.

`LoadedStage4Cpl3SyscallProbe::run(budget, ..)` executes the S4-20c steps
`SYSCALL`, `MOV [RDI],R10`, `SYSRETQ`, `MOV RAX,[RBX]` through
`step_paged_syscall_profile` unchanged, stops at the first step that does not
retire, and records per-step RIP, CS, CPL (from CS RPL) and committed writes.
A successful run ends at RIP `0x400005` in CPL3 with RAX equal to the store
qword written by the kernel; RSP, CR3 and the kernel code mapping are
unchanged. The injections `KernelStoreMisdirected` and `SysretStaysCpl0` are
diagnostic regressions that alter only fixture registers; the runner exposes
them as `--inject-kernel-no-write` and `--inject-sysret-cpl0`. This profile
does not execute OUT/HLT, UART, timers, IRQ, exception delivery, TLB or JIT and
does not close S4-33. See the [S4-33b report](../reports/STAGE_4_S4-33B_2026-11-09.md).

## S4-33c: CR0.WP #PF repair/retry probe

`Stage4PfRepairProbeV1` is a strict raw-image profile (image GPA `0x1000`, size
`0xd000`, 128 KiB RAM, no devices). The loader compares the whole image with the
single accepted layout: page-table entries (reported per entry), exact main and
handler code with zero tails, the IDT gate for vector 14 (CS `0x08`, IST 0,
interrupt gate, target GVA `0x402000`), GDT (null, 64-bit CPL0 code, CPL0 data),
zero stack pages and the data page; initial, canary and store qwords must be
pairwise different. The data page (GPA `0xd000`, GVA `0x800000`) is supervisor
read-only (PTE `0xd001`, A clear); its page table (GPA `0x6000`) is also mapped
RW at GVA `0x403000`. The loader supplies the CPL0 entry state: RIP `0x401000`,
CR0 `0x80010033` (WP=1), CR3 `0x1000`, CR4 `0x20`, EFER `0xd00`, CS `0x08`, SS
`0x10`, GDTR/IDTR, RBX = data GVA, RCX = store qword, RDI = alias GVA, RSI =
`0xd023`, RSP `0x702027`. No guest LIDT/LGDT is executed.

`LoadedStage4PfRepairProbe::run(budget, ..)` runs the accepted primitives
unchanged: `step_paged_cr0_wp_mov` (faulting store), `deliver_paged_page_fault`,
`step_paged_cr0_wp_page_table_repair_mov`, `step_paged_invlpg`,
`step_paged_exception_stack_discard`, `step_paged_iretq`, then the retried store
and the load. It counts sequencer steps against the budget, stops at the first
step that does not behave as required (a page fault after the first one, a store
that retires without a fault, a failed delivery, a non-retiring step) and records
per step RIP, RSP, RFLAGS, CR2, PTE, data and canary qwords, the #PF record, A/D
updates and committed writes. The success path ends at RIP `0x401006` with PTE
`0xd063`, RSP `0x702027`, RF clear. Инъекции `pte_stays_readonly` и `pte_writable_at_load` меняют только
регистры fixture или PTE data. Инъекция `skip_lea` перезаписывает байты LEA в
handler и меняет план sequencer (`plan()` убирает шаг `Discard`: fault, delivery,
repair, INVLPG, IRETQ, retry, load). При `skip_lea` IRETQ стоит на `0x402006` и
читает кадр, сдвинутый на один слот: вершина стека (`iretq_frame_rip` = 3) -
это код ошибки, который IRETQ трактует как RIP. Примитив IRETQ возвращает
`Unsupported` (исход `unsupported`, 5 шагов, 2 retired); архитектурный
#GP(0x1000) (в слоте CS лежит `0x401000`) не моделируется. Uncached walker
означает, что TLB-on и необходимость INVLPG не проверяются. Права покрыты только
для записи в supervisor RO при CPL0 и WP=1 (#PF с P|W); в кейсе нет контрольного
прогона с CR0.WP=0 (что #PF вызван именно WP, подтверждают тесты примитивов
`paged_probe_quantum.rs` `cr0_wp_clear_*` и `page_walk.rs`), не покрыты U/S,
SMEP/SMAP, NX и CPL3. Исключён cross-page доступ к данным кейса; кадр доставки
исключения `0x701ff0..0x702020` сам пересекает две страницы стека (D ставится на
обоих PTE стека, `0x5810` и `0x5808`). IST/TSS, другие пары исключений и JIT вне
кейса. Открытые пункты: нет четвёртой инъекции для кода выхода 7 (loader кладёт
в RCX не store-qword, например canary; проверки после успешного sequencer
`retry_store_committed`, `load_observed_stored_value`, `canary_intact`,
`pte_accessed_dirty`, `final_rip`, `rsp_preserved` в текущих прогонах не падают;
не блокер); вопрос A-бита на PTE при отказе по правам унаследован от S4-19b (SDM
это допускает, но поведение не единственно возможное; помнить для S4-34).
It does not close S4-33. See the [S4-33c report](../reports/STAGE_4_S4-33C_2026-11-09.md).

## S4-33d: cross-page read and fetch probe

`Stage4CrossPageProbeV1` is a strict raw-image profile (image GPA `0x1000`, size
`0xd000`, 128 KiB RAM, polling UART at port `0x3f8`). The loader compares the
whole image with the single accepted layout: five page-table pages (reported per
entry), the instruction bytes, the data and the zero frames between them. All
leaf entries are supervisor entries with A and D clear. Two code pages (GVA
`0x400000` and `0x401000`) map the non-contiguous frames `0x7000` and `0x9000`
(PTE `0x7001`, `0x9001`, executable). Two data pages (GVA `0x800000` and
`0x801000`) map the non-contiguous frames `0xb000` and `0xd000` (PTE
`0xb003 | NX` and `0xd003 | NX`). A little-endian qword is split: its bytes 0..2
end data page 0 (GVA `0x800ffd..0x800fff`), its bytes 3..7 start data page 1 (GVA
`0x801000..0x801004`). A same-page copy of the value (GVA `0x800800`) serves as
the control; two decoy qwords (`0xa5` at the start of frame `0xb000`, `0x5a` at
the end of frame `0xd000`) make a swapped-frame loader visible. The value must
have a nonzero upper dword so that the qword and dword reads differ.

The loader supplies the entry state: CPL0 (the access context), RFLAGS `2`
(AC=0), CR0 `0x80010033`, CR3 `0x1000` (passed explicitly, `LongModeState.cr3`
is `None` as in the S4-32 quantum), CR4 `0x20`, EFER `0xd00` (NXE=1), RIP
`0x400ffe`, RAX `0xdeadbeefcafebabe`, RBX `0x800ffd`, RDX `0x3f8`.

`LoadedStage4CrossPageProbe::run(budget, ..)` runs six steps with the accepted
primitives unchanged: `step_paged_mov_word_read_cross_page_fetch` (`66 8B 03`
at `0x400ffe`: bytes `66 8B` end the first code frame, `03` starts the second;
its word operand stays on data page 0), `step_paged_mov` (`48 8B 03`, qword
across the data pages), `step_paged_mov_dword_read` (`8B 03`, dword across the
data pages), `step_paged_mov_word_read` (`66 8B 03`, word on data page 0),
`step_paged_pio_u8` (`EE`) and the existing `PagedCpl0ProbeV1` quantum
(`F4`, budget 1). The allowlist is not extended. With the single RBX the
operand sits at `0xffd` (the word stays in page 0, the qword and dword cross).
A word operand at page offset `0xfff` together with a crossing fetch is rejected
by the S4-11f profile in code (`crates/od-isa/src/execute.rs`, condition
`RBX & 0xfff > 0xffe`). For `8B 03` there is no such rejection in code: the
S4-11d dword fetch crossing is not executed because its contract covers only a
dword operand that "lies entirely in a separate RAM page", while any dword at RBX
`0x800ffd` crosses the data page boundary. Per step the evidence
holds RIP, RAX, CPL, CR2 of the CPU state, the page-fault record (vector, PFEC,
CR2 value, saved RIP, access), the backend classification, all four leaf PTEs
(A and D) and the committed writes. Expected success values: RAX after the
steps `(RAX0 & ~0xffff) | word`, the qword, the zero-extended dword and the
unchanged dword three times (steps 4-6); UART byte equals the low byte of the value; final RIP
`0x40100b`, halted; A on code 1 after step 1, on data 1 from step 2; D never.

Injections change only leaf PTEs: `second_data_pte_not_present` (PTE `0`),
`second_code_pte_nx` (`0x9001 | NX`), `data_frames_swapped` (the two data PTEs
exchange their frames), `second_data_page_hole` (`0x30003 | NX`, outside RAM).
Each has a different outcome: `page_fault` at step 2 with PFEC `0x00`, CR2
`0x801000`; `page_fault` at step 1 with PFEC `0x11`, CR2 `0x401000`, no data PTE
touched; `case_complete` with six retired steps and wrong RAX (exit code 7);
`backend_rejected` at step 2 without a page fault. `--same-page-control` moves
RBX to `0x800800`: same values, data page 1 never walked. The A-bit effect of a
rejected walk follows the accepted S4-11b/S4-11d contracts (the completed walk
keeps its A, D stays clear). The `cpl` field of the per-step evidence is a
constant, not CPU state; that NXE=1 takes effect is shown by other facts (S4-11f
rejects NXE=0 before any fetch, the NX injection gives PFEC `0x11`). CR2
`0x801000` for an access across the page boundary is a model contract choice that
the SDM does not define uniquely (open for S4-34). Word step 4 leaves RAX
unchanged and RAX cannot tell it apart from the preceding dword step; it is
confirmed by retirement, the RIP move and the budget prefix. Open: the loader and
builder do not reject values that make the injections invisible
(`0xa5a5a5a5a55a5a5a`, equal to the decoy, hides a frame swap; values whose low 16
bits are `0xbabe` make the step 1 RAX equal to the sentinel); a guard is left for a
later slice.

Not covered: cross-page store, RMW, stack, REP and RAM/MMIO splits (S4-11 stays
open); the S4-11d dword fetch crossing (outside the S4-11d contract for a
crossing data operand) and a word operand at offset `0xfff` together with a
crossing fetch (rejected by the S4-11f profile); TLB-on (S4-14 is not
accepted); JIT (the case is `not_supported` in the JIT matrix); CPL3, #PF
delivery and CR2 commit; huge pages. It does not close S4-33. See the
[S4-33d report](../reports/STAGE_4_S4-33D_2026-11-09.md).

## S4-33e: huge pages на собственном ядре (2 MiB позитив, 1 GiB негатив)

`Stage4HugePagesProbeV1` — строгий raw-image профиль (GPA `0x1000`, размер
`0x400000`, 6 MiB RAM, polling UART `0x3f8`). Loader сравнивает весь образ с
единственным допустимым: шесть страниц таблиц (отчёт по записи), код, decoy-кадр,
четыре qword 2 MiB-кадра, sentinel и нулевые страницы между ними (отчёт по базе
страницы). Все записи supervisor, A и D сброшены. Код на одной 4 KiB-странице
(GVA `0x400000` -> кадр `0x7000`, PTE `0x7001`). Данные — один 2 MiB leaf: PDE[4]
(GPA `0x3020`) = `0x8000000000200083` (P, R/W, PS=1, NX; адрес кадра `0x200000`
выровнен на 2 MiB, биты 20:13 и PAT нулевые), GVA `0x800000`. Значения: смещение 0
(GPA `0x200000`), `0x100000` (GPA `0x300000`), `0x1ffff8` (последний qword,
GPA `0x3ffff8`) — три разных qword; на смещении `0x1fff00` (GPA `0x3fff00`) лежит
decoy `0x5a`x8, который case перезаписывает и читает обратно. PDE[5] (GVA `0xa00000`)
отображает следующий 2 MiB-кадр `0x400000` с sentinel `0xfeedface0badf00d` — доступ
на байт за первой страницей попадает на другой физический адрес. Негативный маршрут
отдельный: PML4E[1] (`0x5003`) -> PDPT 1 (GPA `0x5000`), PDPTE[0] = `0x40000083`
(P, R/W, PS=1), GVA `0x8000000000`. Отдельный PML4E нужен, чтобы A на PML4E
негативного маршрута нельзя было получить от fetch (код идёт через PML4E[0]).

Loader задаёт entry state: CPL0, RFLAGS `2` (AC=0), CR0 `0x80010033`, CR3 `0x1000`
(явно; `LongModeState.cr3` = `None`, как в S4-32), CR4 `0x20`, EFER `0xd00` (NXE=1),
RIP `0x400000`, RAX `0xdeadbeefcafebabe`, RCX `0xc0dec0de123456ab` (значение store),
RDX `0x3f8`. RBX на каждом шаге ставит sequencer (в разрешённых allowlist нет
инструкции, меняющей RBX); гостевая инструкция на всех шагах чтения одна и та же
(`48 8B 03`), но три смещения выбирает sequencer, а не гость. Это ограничение
кейса.

`LoadedStage4HugePagesProbe::run(budget, ..)` без изменений использует принятые
примитивы: `step_paged_mov_2m_ram` (S4-10b, `48 8B 03` и `48 89 0B`) на шагах 1-5,
`step_paged_pio_u8` (`EE`) и существующий квант `PagedCpl0ProbeV1` (`F4`, budget 1).
Негатив идёт через `step_paged_mov` (4 KiB-walker, S4-10c): один шаг. Allowlist не
расширялся. Case `TwoMiB` (7 шагов): load смещение 0, load `0x100000`, load
`0x1ffff8`, store RCX по `0x1fff00`, load обратно, OUT, HLT.

Ожидаемые значения выведены из Intel SDM Vol. 3A гл. 4, не из вывода кода:
PDE 2 MiB leaf — формат PDE, отображающей 2-MByte страницу, §4.5 (P=bit 0, R/W=bit 1, A=bit 5, D=bit 6, PS=bit 7=1,
адрес бит 51:21, XD=bit 63); физический адрес = база кадра + биты 20:0 линейного
адреса (§4.5); A ставится на каждой используемой записи, D — только в записи,
которая отображает страницу, то есть в PDE (§4.8). После шага 1: PML4E `0x2023`,
PDPTE `0x3023`, PD[2] `0x4023`, code PTE `0x7021`, PDE `0x80000000002000a3`; D нет.
После store (шаг 4): PDE `0x80000000002000e3`; остальные записи без D. RAX по шагам:
start, middle, end, end (store RAX не меняет), `0xc0dec0de123456ab` (чтение обратно),
то же для OUT и HLT; UART-байт `0xab`; итоговый RIP `0x400011`, CPU halted.
Committed writes: шаг 1 — `0x1000, 0x2000, 0x3010, 0x4000, 0x3020`; шаг 4 —
`0x3020` (D на PDE) и `0x3fff00` (8 байт); остальные шаги без записей.

Негатив (`GibNegative`): `48 8B 03` по `0x8000000000` -> гостевой #PF, вектор 14,
PFEC `0x09` (SDM §4.7: P=1 — запись present, W/R=0 — чтение, U/S=0 — CPL0,
RSVD=1 — зарезервированный бит, I/D=0; PS=1 в PDPTE при Page1GB=0 зарезервирован,
контракт S4-10c), CR2-запись `0x8000000000`, saved RIP `0x400000`, steps 1, retired
0, RIP и RAX не меняются, CR2 состояния CPU остаётся `None` (доставка #PF в кейс не
входит). A: PML4E[1] `0x5023` (единственный committed write маршрута данных `0x1008`),
PDPTE[0] остаётся `0x40000083` (ни A, ни D), PDE данных не тронут. Остальные литералы
PFEC из S4-10c (CPL0 write `0x0b`, fetch NXE=1 `0x19`, CPL3 `0x0d/0x0f/0x1d`) этот
кейс не исполняет, они покрыты тестами S4-10c.

Инъекции меняют только записи страниц или RBX шага: `pde_ps_cleared` (PDE[4] ->
4 KiB-таблица `0x6003|NX`, её запись 0 -> decoy-кадр `0x8000`): load смещения 0
возвращает decoy `0xa5a5a5a5a5a5a5a5`, load `0x100000` — #PF PFEC `0x00`, CR2
`0x900000` (steps 2, retired 1, код 5); `offset_past_page` (RBX третьего чтения =
`0xa00000`): все 7 шагов, RAX шага 3 = sentinel, код 7; `pdpte_ps_host_error`
(только `GibNegative`: PDPTE[0] -> PD, RBX = `0x8000800000`, то есть маршрут к PDE с
PS=1 через 4 KiB-walker): host-ошибка `UnsupportedPageSize` без #PF (steps 1,
retired 0, код 5). Последняя — подстановка класса исхода: код walker не откатывался,
поэтому инъекция воспроизводит тот же исход (host-ошибка вместо #PF), который давал
PDPTE.PS до S4-10c, а не саму регрессию walker. Саму регрессию ловят unit-тесты
S4-10c (`s4_10c_*`). Различимы по (stop, steps, retired, PFEC, RAX по шагам).
`--negative-1gib` выбирает негативный case; инъекции чужого case отвергаются (код 3).

Не покрыто: 1 GiB позитивный путь (не объявлен, Page1GB=0); MMIO; TLB-on; JIT; CPL3;
доставка #PF и коммит CR2; cross-page; PAT; PDE PS=1 в 4 KiB-API остаётся
`UnsupportedPageSize` (инъекция `pdpte_ps_host_error` его и использует); три смещения
выбирает sequencer, не гость; A на PML4E негативного маршрута отделён от fetch
отдельным PML4E[1], на общем PML4E[0] разделить нельзя. Не закрывает S4-33 и S4-10.
См. [отчёт S4-33e](../reports/STAGE_4_S4-33E_2026-11-09.md).

## Paged `LIDT` adapter (S4-18a)

`step_paged_lidt` composes the exact `PagedLidtLong64V1` CPU entry with the
uncached tracked four-level walker. Instruction fetches and the operand walk
commit the actual page-table Accessed-bit writes and report each committed
eight-byte range; the descriptor read does not set Dirty. The operand is one
RAM-only ten-byte read contained in one 4 KiB guest page. A page-crossing
source is rejected before an operand walk, and a noncanonical full SS-based
range faults before operand access. Physical holes remain backend errors;
this adapter never dispatches MMIO/PIO. CPU state changes only after the full
descriptor read and canonical base check, and the loaded IDT base is not
translated. Tests compare exact callback ranges, relevant page-table entries,
fault metadata and full CPU state. S4-18a did not implement descriptor caching,
IDT gate checks, exception delivery, cross-page descriptor reads, or production
paging ownership. The bounded slice was independently
reviewed and accepted; its result is recorded in the S4-18a report.

## Paged `LGDT` adapter (S4-18b)

`step_paged_lgdt` composes the exact `PagedLgdtLong64V1` CPU entry with the
same uncached tracked four-level RAM walker. It accepts only bare
`0F 01 55 ib` in its fixed Long64/CPL0 profile, reads one ten-byte
pseudo-descriptor from a canonical source contained in one 4 KiB guest page,
and commits only GDTR and RIP after the complete read and loaded-base check.
Instruction decoding cannot fetch outside the guest code page; the four-byte
form at offset `0xffc` remains admissible, and a wrapped fetch at the top of
the address space is rejected before it can address page zero.
Actual fetch and operand Accessed-bit writes are reported in order; the
descriptor read sets no Dirty bit. Crossing sources are rejected before the
operand walk, physical holes stay backend errors, and descriptor access never
routes through MMIO/PIO. LGDT does not read the loaded GDT or modify segment
caches. A guest-loaded IDT and GDT are then exercised in the same CPU sequence
by the S4-18b end-to-end delivery case; no host register patch occurs between
LIDT/LGDT, the faulting store, and handler entry. This does not implement
TSS/IST, descriptor Accessed writes, `#DF`, shutdown, or fault recovery. The
bounded slice passed independent review and is accepted; its result is recorded
in the S4-18b report.

## Paged #PF target descriptor Accessed update (S4-18c)

`deliver_page_fault` accepts the already-validated GDT target code descriptor
with A=0 in the existing Long64/CPL0/same-CPL/IST=0 profile. The selected
emulator sequence is six tracked frame qword stores, one tracked write of the
descriptor access byte with only A set, then commit of the target CS cache,
RSP, RIP and live flags. This order is an explicit partial-effects contract of
the bounded profile; Intel SDM Vol. 1 §6.14.1 and Vol. 3A §3.4.5 describe the
frame/CS operations and the Accessed-bit effect, but do not guarantee this
fault-visible ordering for competing faults. [Vol. 1](https://cdrdv2-public.intel.com/874241/253665-090-sdm-vol-1.pdf),
[Vol. 3](https://cdrdv2-public.intel.com/782157/325384-sdm-vol-3abcd.pdf).

An A=1 target makes no descriptor write, so a read-only GDT remains usable. The
adapter routes the one-byte A update and all stack writes through tracked
RAM-only translation. Before any frame store, a preflight checks every planned
qword mapping and possible page-walker A/D write against the target descriptor
byte and the GDT page-table entries. It rejects physical aliases before frame
effects and preserves the descriptor's preflighted physical mapping for the
later A update. It also checks frame operands against page-table entries used
by reachable stack slots, preventing a frame store from remapping a later
slot; possible stack-walk A/D writes are checked against the descriptor and GDT
protection ranges. Preflight stops at the first stack access that would
fail; the tracked path then reports that original page-fault or physical
backend classification without considering unreachable later slots. No MMIO
dispatch is added.

A secondary #PF during original #PF delivery requires #DF; S4-18c returned a
terminal undelivered diagnostic result before the bounded #DF path was added.
See the [S4-18c report](../reports/STAGE_4_S4-18C_2026-11-06.md) and
[verification record](../testing/MACHINE.md#s4-18c-target-descriptor-accessed-update).

<a id="secondary-pf-df-delivery-and-shutdown-s4-19c"></a>

## Paged `LTR AX` (S4-18d)

`step_paged_ltr_ax` composes `prepare_paged_ltr_ax` / `prepare_paged_ltr_commit`
with the uncached tracked four-level RAM walker. It accepts only bare `0F 00 D8`
(`LTR AX`) in the same fixed Long64 profile as the bounded LGDT/LIDT entries:
CR0/CR4/EFER equal to the profile values, 36-bit CR3, a present GDTR, a running
CPU and no STI shadow. Instruction bytes are fetched one at a time and only inside
the page of RIP. A `LOCK` prefix is `#UD`; CPL != 0 is `#GP(0)` after the exact
fetch; other encodings are `Unsupported` with no descriptor access.

Selector checks precede any descriptor access: null → `#GP(0)`; TI=1 or a
descriptor outside the GDT limit → `#GP(selector)`; a descriptor address that
wraps or is non-canonical → `#GP(0)`. A descriptor whose 16 bytes would cross a
4 KiB page is `Unsupported` before access. The descriptor is read through a
supervisor (`DescriptorTable`) walk, one 16-byte RAM-only operand. The shared
validator `validate_tss_descriptor` then requires an available 64-bit TSS (type 9,
busy/other types `#GP(selector)`), present (`#NP(selector)`), a zero upper type
field (bits 12:8 of the upper dword; other upper bits are ignored; `#GP(selector)`)
and a canonical base (`#GP(0)`); limit honours the G bit. The `#GP(0)` code for a
non-canonical base and the check order are profile choices that are not yet
confirmed against the SDM text or hardware.

Only after validation a second, write-permission walk of the same address runs
(WP applies: a read-only GDT page raises `#PF` with W set, CR2 = descriptor
address, without CPU or descriptor change). Its translation must equal the read
walk; if the descriptor's 4 KiB page is also a page-table page used to reach it,
the step is rejected as `PagedLtrRejection::TableAlias` (a mismatch is
`TranslationChanged`, not reachable with the current walker since A bits do not change
translations), reported as `Unsupported`, with no D or busy-descriptor write. The A
bits set by the walks stay committed and, for an aliased descriptor, may change
its bytes.
On success the leaf D bit is written when clear, then the 16-byte descriptor
image with the busy bit set, and only then TR (selector, base, limit, access
byte with busy bit, G/AVL flags) and RIP are committed (RIP += 3). Fetch and A-bit
writes are reported in order through the callback, then the D write, then the
descriptor write. On any fault or rejection the CPU state is unchanged and page
table A bits (and their callbacks) from earlier walks stay committed.

The step needs exclusive RAM ownership for the duration of the call (no DMA,
other vCPUs or host writers); it is not system-wide locked-bus atomicity and has
no TLB, JIT or cross-page support. No TSS contents are read, no stack switching
or task switching is modelled, and `TaskRegisterCache` is rejected by the legacy
fragment adapter. CR3 and EFER are explicit arguments, like the other paged
descriptor steps. See the [S4-18d report](../reports/STAGE_4_S4-18D_2026-11-08.md).

## Paged `MOV DS/ES/FS/GS, AX` (S4-18e)

`step_paged_mov_ds`, `step_paged_mov_es`, `step_paged_mov_fs` and `step_paged_mov_gs`
compose the existing CPU GDT segment loaders with the uncached tracked four-level RAM
walker. Each accepts only the bare two-byte form `8E D8` / `8E C0` / `8E E0` / `8E E8`
(source AX) in the same fixed Long64 profile as the bounded LGDT/LIDT/LTR entries: CR0/CR4/EFER
equal to the profile values, 36-bit CR3, a present GDTR, the matching hidden descriptor
cache, CPL0 (otherwise `Unsupported` before any fetch), a running CPU and no STI shadow.
Only the two instruction bytes are fetched and only inside the page of RIP.

The selector is the low 16 bits of RAX. A null or TI=1 selector and a missing hidden cache are
`Unsupported` after the two-byte fetch (so the fetch walk's A bits and callbacks are already
committed); a selector outside the GDT limit is `#GP(selector)`, as is a descriptor address (or its
last byte) that is non-canonical or wraps, before any GDT walk. The 8-byte GDT descriptor is read through a
supervisor (`DescriptorTable`) walk as one RAM-only qword; it must be a present, readable,
non-system segment with `max(CPL, RPL) <= DPL` (`#GP(selector)` / `#NP(selector)`; the error code
has RPL cleared). When the descriptor's accessed bit is clear it is set with one one-byte
descriptor-table write after the leaf D bit (CR0.WP applies: a read-only GDT page raises `#PF`
with W set and CR2 = descriptor address + 5). The selector, hidden cache (and `fs_base`/`gs_base`
for FS/GS, 32-bit base) and RIP (+2) are committed only after every memory effect succeeded.
On any fault the CPU is unchanged; A bits (and the leaf D bit) written by earlier walks stay
committed. Fetch and A-bit writes are reported through the callback in order, then the D write,
then the accessed-bit byte.

A descriptor whose eight bytes would cross a 4 KiB page is `Unsupported` (the report carries
`TrackedAccessError::CrossesPage`); a physical hole, ROM, an MMIO region (no device dispatch) or
RAM ending inside the descriptor is a backend error. CR3 and EFER are explicit arguments. Not
covered: null-selector loads, LDT selectors, SS/CS, long-mode system descriptors and TLB/JIT. When
the descriptor shares a physical page with the page tables that translate it, the order is: A bits
(read walk), descriptor read, leaf D bit (write walk), accessed-bit byte; no A/D update is lost, and
the hidden cache holds the descriptor bytes read before the D write (as on hardware). See the
[S4-18e report](../reports/STAGE_4_S4-18E_2026-11-08.md).

## Secondary #PF, #DF delivery, and shutdown (S4-19c)

Within the existing Long64/CPL0/same-CPL/IST=0 exception profile, an actual
architectural #PF while delivering the original vector-14 frame constructs a
`DoubleFaultRecord` containing both page-fault records and makes exactly one
vector-8 delivery attempt. The #DF frame's error-code slot is zero. It keeps
the pre-entry RIP as a diagnostic value (not a restart address), does not set
RF in saved RFLAGS, and does not change CR2 itself. CR2 remains the nested #PF
linear address. Partial stack and paging effects from the failed #PF attempt
are retained and the #DF attempt uses the original uncommitted entry registers.

A real #PF during the #DF attempt sets `CpuActivity::Shutdown`; the adapter
does not attempt another delivery. Subsequent paged CPU execution reports
`PagedProbeExit::Shutdown` before instruction fetch or memory access. Backend
and profile errors remain undelivered diagnostics and are not reclassified as
architectural shutdown. Reports retain the initial #PF delivery result, its
secondary fault, the #DF record/result, any #DF delivery fault/backend error,
and cumulative ordered A/D and physical-write effects.

The end-to-end cases cover a read-only GDT target descriptor whose Accessed-bit
write causes nested #PF followed by a successfully executed #DF handler, and a
missing stack page that causes #PF during #DF delivery and terminal shutdown.
This does not implement other exception-pair classifications, TSS/IST or
privilege transitions, guest page-table repair, INVLPG, IRETQ, instruction
retry, or complete vCPU lifecycle. Legacy V2 snapshots explicitly reject
Shutdown because their activity schema contains only Running and Halted. See
the [S4-19c report](../reports/STAGE_4_S4-19C_2026-11-06.md) and
[verification record](../testing/MACHINE.md#s4-19c-secondary-pf-double-fault-delivery-and-shutdown).

## Guest PTE repair and INVLPG (S4-19d)

The bounded #PF handler profile adds one separate exact `MOV [RDI],RSI`
qword-store selector. It does not widen the established `MOV [RBX],RCX` / `MOV
RAX,[RBX]` allowlist. After the actual WP=1 write fault and successful vector-14
entry through IDTR/GDTR values initialized in the fixture, the handler writes
a repaired leaf PTE through a writable guest alias to the page-table page,
then executes the existing exact
`INVLPG [RBX]` profile for the original faulting linear address. RBX and RCX
remain available for the original store and are not host-patched after fault
entry. The instruction selector admits only the exact store form; the fixture
sets RDI to the alias and RSI to the repaired PTE value, and those fixture
preconditions preserve Present, Accessed, supervisor-only U/S=0, the physical
page address and the selected other bits. The qword operand must remain within one 4 KiB page; a
crossing address is rejected before operand access. Tracked writes and
page-table A/D changes remain visible in execution order.

This bounded stop point verifies the guest's PTE store and retired INVLPG;
data has not yet been modified. The tracked execution path is uncached, so this
does not prove a stale translation would otherwise be observed or that INVLPG
is necessary for visibility. Guest error-code discard, IRETQ, fault retry and
successful data commit remain for S4-19e. See the
[S4-19d report](../reports/STAGE_4_S4-19D_2026-11-06.md) and
[verification record](../testing/MACHINE.md#s4-19d-guest-pte-repair-and-invlpg).

## Guest error-code discard and paged IRETQ (S4-19e1)

After the guest has repaired the supervisor-only leaf PTE and retired INVLPG,
the bounded handler executes only exact `LEA RSP,[RSP+8]` to discard the
eight-byte #PF error-code slot, followed by exact `48 CF` IRETQ. The separate
paged IRETQ selector validates Long64 CPL0 state and the active CR0/CR3/CR4
and EFER profile. It reads the five-qword same-CPL frame and the real CS and
SS descriptors through tracked guest memory; CS is read and validated before
saved SS selector/bounds are checked. Descriptors must be valid and already
Accessed, with SS a writable CPL0 data segment.

Live RF=1 is outside this profile and is rejected before fetch. RF=1 in the
saved fault frame is accepted and restored only after the complete frame and
both descriptors pass validation. CPU return state is committed atomically;
page-walk A/D writes from reads already completed remain visible on later
failure. A tracked #PF is reported separately from backend errors and updates
CR2. The system fixture verifies restoration of the original faulting RIP,
unaligned RSP, RF and unchanged control registers while faulting data remains
untouched. It stops before retrying the original store.

Intel SDM Vol. 3A §6.14 describes the 64-bit exception frame and notes that
IRET does not pop the error code, so the handler removes it. Intel SDM Vol. 3B
§20.3.1.1 describes RF in fault frames, transfer by IRETQ and clearing at the
start of the next instruction. This bounded profile does not cover other IRETQ
forms or demonstrate stale-TLB invalidation because tracked execution is
uncached. See the [S4-19e1 report](../reports/STAGE_4_S4-19E1_2026-11-06.md)
and [verification record](../testing/MACHINE.md#s4-19e1-guest-error-code-discard-and-paged-iretq).

## Faulting MOV retry and RF clear (S4-19e2)

After successful guest paged IRETQ restores RF=1 and the original faulting RIP,
the bounded MOV-only CR0.WP profile starts the admitted exact qword
`MOV [RBX],RCX` or `MOV RAX,[RBX]` with RF set. After fetch/decode/profile
admission, it clears live RF before the architectural operand address check
and before tracked memory access. The clear remains if an operand #GP/#PF or
backend error occurs; fetch/profile exits and the bounded cross-page store
rejection leave RF as received. The defined-flags mask is unchanged.

The end-to-end case retries the original faulting store exactly once, observes
the tracked leaf PTE transition `0xe023 → 0xe063` (Dirty set; Accessed and
supervisor-only U/S=0 retained), then the qword data write, in that callback
order. The next guest MOV load reads the new value and the adjacent qword
canary remains unchanged. The page walk is tracked but uncached; this does not
establish stale-TLB eviction or INVLPG necessity. Intel SDM Vol. 3B
§20.3.1.1 revision 090US defines RF clearing at instruction start after the
specified preliminary checks. See the [S4-19e2 report](../reports/STAGE_4_S4-19E2_2026-11-06.md)
and [verification record](../testing/MACHINE.md#s4-19e2-faulting-mov-retry-and-data-commit).

## Synthetic paged IRETQ outer return to CPL3 (S4-20a)

`step_paged_iretq_to_cpl3(PagedCpl0ToCpl3V1, ...)` admits only the synthetic
four-level profile: `CpuidProfile::DiagnosticV1` (basic leaf limit 1, leaf 7
zero), Long64, CPU-owned `CR0=0x80010033`, `CR4=0x20` (`LA57=0`), a bounded
36-bit CR3, and `EFER=0xd00`. The current CS cache must be the flat CPL0 code
descriptor; DS/ES/FS/GS visible selectors must be null. This condition excludes
the five-level selection path for this CPU-only scenario. It does not establish
a Linux loader handoff or boot profile.

The guest executes exact `48 CF` from CPL0. The five-qword outer-return frame
and GDT user descriptors are read through the supplied `GuestMemory` interface.
This CPU-only slice does not connect that interface to the tracked page walker.
The supported
user code descriptor is flat `access=0xfb, flags=0xa`; the writable user stack
descriptor is flat `access=0xf3, flags=0xc`; both are already Accessed. Read
ordering follows the bounded Intel IRETQ path: RIP/CS/RFLAGS, validate/read CS
descriptor, read saved RSP/SS, check saved RIP canonicality, then read/validate
SS descriptor. CPU state is committed only after all admitted reads and checks.
The CPU profile does not roll back effects already committed by the supplied
memory backend. Its unit tests use a guest-memory test double and do not exercise
tracked translations or page-walk A/D updates.

Live RF must be clear at entry; RF may be restored from saved RFLAGS along with
the supported integer flags, IF and IOPL mask. Unsupported selector/descriptor,
address and flag shapes return `Unsupported` in this narrow profile. Tracked
guest paging faults remain `Fault`; backend access failures remain
`ReadAborted`. Architectural #GP/#SS classification for rejected return state
is not implemented here. The slice proves CPU-state transition only; deriving
machine `AccessContext` from committed CS and executing user memory operations
are deferred to S4-20b. See the [S4-20a report](../reports/STAGE_4_S4-20A_2026-11-06.md),
[plan](../plans/STAGE_4.md#s4-20-cpl3-и-системные-вызовывозвраты), and
[verification record](../testing/MACHINE.md#s4-20a-synthetic-paged-iretq-outer-return).

## Machine IRETQ followed by one CPL3 load (S4-20b)

`step_paged_iretq_and_user_load` performs the bounded S4-20a paged return and,
only after successful retirement and validation of the committed flat user CS,
executes one exact `48 8B 03` (`MOV RAX,[RBX]`). It derives the explicit CPL3
access context from committed CS selector RPL and cache, and reads CR3 from the
CPU state. The wrapper accepts neither an external access context nor a CR3
argument. IRETQ and load results are reported separately; a guest fault or
backend abort during the load leaves the already committed CPL3 transition in
place. A non-retired IRETQ produces no load result.

The load profile admits exactly the three bytes `48 8B 03`, with all bytes in
one guest page (RIP offset at most `0xffd`). Cross-page instruction fetch is
unsupported in this slice. Four-level tracked paging checks U/S at every level:
the tested user mapping loads the qword; supervisor-only instruction fetch and
data read fault with PFEC `0x15` and `0x05`, respectively, while retaining the
faulting RIP and setting CR2 to the faulting linear address. Successful and
faulting walks report their Accessed updates; a read-only load does not set
Dirty or write the operand. Backend physical-read failures remain distinct
from guest page faults and do not retire the instruction or commit its load
destination. Effects already committed by IRETQ and eligible page-walk A-bit
updates remain visible. After successful fetch and instruction admission, this
MOV profile clears RF before operand access; a subsequent data-access abort
leaves RF cleared. A fetch fault occurs before this RF update. See the
[S4-20b report](../reports/STAGE_4_S4-20B_2026-11-06.md),
[plan](../plans/STAGE_4.md#s4-20-cpl3-и-системные-вызовывозвраты), and
[verification record](../testing/MACHINE.md#s4-20b-machine-iretq-and-cpl3-load).

## Paged SYSCALL, kernel store, SYSRETQ and user load (S4-20c)

`step_paged_syscall_profile` executes one admitted instruction using the CPU's
owned syscall bundle and paging state. `run_paged_syscall_roundtrip` executes
the bounded sequence: CPL3 `0F 05` (`SYSCALL`), CPL0 `4C 89 17`
(`MOV [RDI],R10`), CPL0 `48 0F 07` (`SYSRETQ`), and CPL3 `48 8B 03`
(`MOV RAX,[RBX]`). The caller does not supply a parallel CPL, CR3, or EFER.
The machine derives privilege from committed CS and uses the current CPU CR3.

This profile is Long64 with four-level 4 KiB paging and canonical 48-bit
addresses. Every instruction fetch and qword operand must fit in one guest
page. It admits no cross-page fetch or operand. `SYSCALL` takes its target from
LSTAR, stores the next user RIP in RCX and saved flags in R11, applies FMASK,
and loads bounded fixed CS/SS caches derived from STAR without GDT reads or an
RSP switch. `SYSRETQ` validates CPL and canonical RCX before committing user
CS/SS caches and the supported RFLAGS subset. Its flags mask is `0x3c7fd7` with
bit 1 forced; definedness is validated alongside values. The handler store
uses the supervisor RW mapping and the final load uses a user RO alias to the
same physical qword in the roundtrip fixture. These are scenario mappings, not
API requirements; the machine checks the permissions relevant to each access.
The successful scenario uses EFER `0xd01`; the bounded API also admits `0xd00`
to verify #UD when SCE is disabled.

Each standalone step rejects live TF or RF before instruction fetch. SYSRETQ
also rejects TF in saved R11 after the existing SCE, CPL and canonical-address
checks; it leaves CPU state unchanged, while completed fetch page-walk effects
and their callbacks remain committed. Tracked page walks report exact A/D
changes and committed physical write ranges. Guest page faults retain PFEC,
CR2 and saved RIP; physical backend errors remain distinct. A later failure
does not undo an earlier retired privilege transition or kernel store.

There is no TSS/IST or stack switch, SWAPGS, asynchronous event delivery,
general WRMSR, compatibility SYSRET, exception delivery from CPL3, Linux entry
assembly, or Linux boot claim. The synthetic initial CPL3 CPU state is supplied
by the fixture. See the [S4-20c report](../reports/STAGE_4_S4-20C_2026-11-06.md),
[plan](../plans/STAGE_4.md#s4-20-cpl3-и-системные-вызовывозвраты), and
  [verification record](../testing/MACHINE.md#s4-20c-paged-syscall--kernel-store--sysretq--user-load).

<a id="paged-cpl3-mov-store-load-after-iretq-s4-20d"></a>

## Paged CPL3 MOV store/load after IRETQ (S4-20d)

`step_paged_cpl3_mov` is a separate opt-in entry for exact `48 89 0B`
(`MOV [RBX],RCX`) and `48 8B 03` (`MOV RAX,[RBX]`). It preserves the existing
load-only `step_paged_cpl3_load` contract. The entry derives CPL from the
committed CS selector/cache and CR3 from `LongModeState`; it takes no alternate
privilege or address-space input and does not require syscall MSRs.

The accepted sequence is paged IRETQ to CPL3, the existing S4-20b user load,
then the new store and reload. The machine uses the established Long64,
canonical48, physical36, four-level 4 KiB profile (`CR0=0x80010033`, `CR4=0x20`,
`EFER=0xd00`). Live TF, RF, and AC must be clear before fetch. Each exact
three-byte instruction and qword operand fits within one guest page. For both
MOV forms, after fetch/decode the profile checks qword page crossing before
canonicality; a combined crossing/noncanonical address returns
`Rejected(CrossPageOperand)` without operand access. Fetch already occurred
and may have committed page-walk A updates. This order applies only to the new
opt-in entry; the load-only entry and its page-aware adapter retain their
previous behavior.

The positive fixture stores RCX's nonce to user RW data and reads it back into
RAX. Protection cases distinguish user RO and supervisor-only store #PF
(`PFEC=0x07`), supervisor-only read (`0x05`), supervisor-only or NX fetch
(`0x15`), non-present user instruction fetch (`0x14`), and non-present user
read/store (`0x04`/`0x06`, NXE=1). The four-level matrices verify U/S and R/W
effects, CR2 and saved RIP. Exact A/D PTE values and callback order are checked
for the first user fetch-fault, non-present data, and store-fault paths;
supervisor-only read and late non-present fetch paths check the fault record
and prior committed effects. Protection faults do not write the operand or set
D. Backend physical-store failure remains
`OperandPhysical` and distinct from guest #PF. Earlier IRETQ, load, and store
commits remain visible after a later fault.

This slice does not add general CPL3 exception delivery, privileged-instruction
#GP/#UD behavior, invalid-selector IRETQ classification, cross-page accesses,
or Linux boot. See the [S4-20d report](../reports/STAGE_4_S4-20D_2026-11-06.md),
[plan](../plans/STAGE_4.md#s4-20-cpl3-и-системные-вызовывозвраты), and
[verification record](../testing/MACHINE.md#s4-20d-paged-cpl3-mov-store-load-after-iretq).

<a id="paged-cpl3-mov-cr0-after-iretq-s4-20e"></a>

## Paged CPL3 MOV CR0 after IRETQ (S4-20e)

`step_paged_cpl3_mov_cr0` is a separate opt-in machine entry for the bounded
privileged-instruction check after the accepted paged IRETQ to CPL3. It derives
CPL from the committed CS selector and cache and CR3 from CPU state. Its fixed
profile is Long64, canonical48, physical36, four-level 4 KiB paging,
`CR0=0x80010033`, `CR4=0x20`, and `EFER=0xd00`. A malformed user CS, unsupported
controls, live TF/RF/AC, or active STI shadow is rejected before instruction
fetch. The instruction RIP offset must be at most `0xff1`; `0xff2` and above
are rejected before fetch so the decoder's maximum instruction length stays
within one page.

The exact bare `0F 22 C0` (`MOV CR0,RAX`) fetches and decodes, then raises
`#GP(0)` at CPL3. This result is independent of RAX and IOPL and does not
retire or commit CR0 or other instruction CPU state. Exact `F0 0F 22 C0`
raises `#UD` only after full fetch/decode; an instruction-fetch fault takes
precedence until decoding completes. Fetch page walks may already have
committed A-bit updates and their callbacks before either execution fault.
The entry reports those effects separately from guest faults and backend
errors. It does not deliver the exception through an IDT, switch stacks, or
perform CPL3 exception handling.

Tracked fetch tests cover supervisor-only permissions at all four paging
levels, NX, and a non-present user leaf (`PFEC=0x14`) through this new entry.
They check CR2, faulting RIP, no retirement, preservation of CPU state except
the architecturally updated CR2, exact A-bit PTE values, and callback ranges.
The physical instruction-fetch failure is reported as `OperandPhysical` at GPA
`0x30000`, without retirement. A separate page-walk unit test injects failures
at the internal table-writer seam for fetch A-bit writes and checks
`TableWrite`, level/address/bit, and the committed update prefix; this checks
the shared walker path rather than injecting through the machine entry.
The IRETQ → user load → user store → `MOV CR0,RAX` case confirms that the
earlier store remains committed after `#GP(0)`.

This slice generates #GP/#UD results but does not deliver exceptions. Other
CR forms, general CPL3 exception delivery, IDT/TSS stack switching, and
cross-page instruction fetch remain outside its contract. See the
[S4-20e report](../reports/STAGE_4_S4-20E_2026-11-07.md),
[plan](../plans/STAGE_4.md#s4-20-cpl3-и-системные-вызовывозвраты), and
[verification record](../testing/MACHINE.md#s4-20e-paged-cpl3-mov-cr0-after-iretq).

## Scoped tracked memory for one Published V2 JIT load (S4-15a)

`with_paged_mov_memory` gives one synchronous callback a temporary, read-only
`GuestMemory` adapter over the caller's `MachineBus`. It reuses the tracked
Long64 four-level/4 KiB walker and the bus's sole physical RAM owner. The scoped
adapter admits only an eight-byte read contained in one guest page; it rejects
fetches, writes and crossing qwords. A read translated to MMIO returns the
tracked `OperandPhysical(UnreadableRegion(Mmio))` backend classification
without dispatching a device callback. Page-walk A updates already committed before a fault remain in the
scope result. The scope returns the callback result, precise `PageFaultRecord`
or tracked access error, committed A updates, and physical write ranges.
The temporary memory borrow cannot escape the synchronous callback.

`PublishedCode::execute_paged_mov_rax_rbx_v2` admits only the direct V2
published `48 8B 03` (`MOV RAX,[RBX]`) artifact and runs it with budget one.
Before opening the memory scope it checks source/site metadata, entry RIP,
Long64 mode, CPL0 or CPL3, non-halted activity, clear interrupt shadow, and the
supported paging snapshot. Unsupported artifacts and CPU context fail before
memory access. A missing operand mapping is returned as guest `#PF` with its
exact RIP, CR2 and PFEC. MMIO, cross-page and other backend refusals remain
host-side execution errors and do not retire. Successful loads commit the
destination and RIP once, set only architecturally reached A bits, and never
set D.

This slice does not add JIT stores, other load forms, cross-page splitting,
device accesses, instruction fetch through JIT memory, TLB caching, executable
alias invalidation or interpreter fallback orchestration. It is evidence for
one V2 load on the production machine RAM owner, not acceptance of parent
S4-15. See the [S4-15a report](../reports/STAGE_4_S4-15A_2026-11-06.md).

## Firmware board v1 and handoff (S7-03)

**Статус: реализовано исполнителем, ожидает ревью.** Контракт — [FIRMWARE_BOOT.md](FIRMWARE_BOOT.md);
код — `crates/od-machine/src/firmware_board.rs` (`load_firmware_image`,
`FirmwareBoardConfig`, `LoadedFirmwareBoard`), ROM backing — `physical_memory.rs`
и `bus.rs`. `od-machine` не знает UEFI, PE и ACPI: он проверяет плоский образ
`ODFWIMG1`, кладёт его в ROM и в RAM-shadow, строит временные таблицы и handoff
block, отдаёт `LongModeState`. Это не reset vector и не real mode.

**Реализовано.**

- **ROM.** Регион `Rom` имеет read backing (при создании нули). Данные читаются через
  `PhysicalMemory::read_bytes`, `MachineBus::read_physical` и `read_flat_bytes`.
  Любая запись гостя или обычного загрузчика (`write_bytes`, `write_physical`,
  `write_flat_bytes`) возвращает `ReadOnlyRegion(Rom)` и не вызывает callback;
  единственный писатель — `pub(crate)` `load_rom_bytes`, который вызывает только
  `load_firmware_image`. Page generations и code tracking для ROM не создаются
  (`code_page_generation` отклоняет `Rom`).
- **Плата.** Карта строится из `FirmwareBoardConfig` (`ram_bytes` кратно 2 MiB в
  `[128 MiB, 0xF_F400_0000]`, `second_screen`, `tsc_hz`, UUID, серийный номер) и
  таблицы устройств: RAM (нижняя `[0, min(R, 0xF400_0000))`, верхняя от 4 GiB),
  ROM `[0xFFC0_0000, 2^32)`, framebuffer экрана 0 (и экрана 1 при
  `second_screen`), `Mmio` регистров дисплея, IOAPIC и Local APIC, порт COM1
  `0x3F8` (8 портов). Одна и та же таблица из семи записей строит шину, окна карты
  и устройства handoff block. У NVRAM, TPM CRB и ECAM по умолчанию `PRESENT = 0`. Такие устройства не добавляются в карту или на шину.
- **TPM CRB (S7-22).** Опция `FirmwareBoardConfig::tpm` включает MMIO
  `[0xFED4_0000, 0xFED4_5000)`, строку устройства 6 и handoff v1.13 с ACPI.
  Обычная шина и firmware-путь работают с одним состоянием устройства. Синхронный
  START выполняет одну команду из внутреннего буфера через `od-tpm`.
  Регистровые и временные проверки завершаются до эффекта. Ошибка команды
  кодируется в ответе TPM. Firmware берёт абсолютное время инструкции из
  начала кванта и счётчика уже завершённых инструкций, обычная шина снимает
  текущее время платы. На хосте `tpm_state_snapshot()` возвращает секретный
  ODTPMSTA v6 с запрошенным выделением банков PCR и часами до текущей границы. Восстановление — при создании
  платы из проверенного снимка и свежей энтропии владельца. [CRB и снимок](VTPM.md#crb).
- **Образ.** Принимается только образ ровно `0x0040_0000` байт с заголовком v1
  (FIRMWARE_BOOT.md 3.2): magic, `format_version`, `header_size`, `image_size`,
  `link_base`, `entry_offset ∈ [0x40, image_size)`, `handoff_major = 1`,
  `flags = 0`, reserved нули, CRC-32. Любое несовпадение — `FirmwareBoardError`
  до создания шины и vCPU. Образ копируется в ROM, затем **читается из ROM** и
  записывается в shadow `[0x0020_0000, 0x0060_0000)` одной записью (page
  generations shadow растут обычным путём).
- **Таблицы страниц.** Identity, только 4 KiB, с `CR3 = 0x0060_0000`: листья RAM
  `P|RW`, ROM `P|NX`, `Mmio`/`Framebuffer` `P|RW|NX`, промежуточные уровни `P|RW`,
  `Reserved` не отображается. Размер `N` считается по формуле раздела 4 и сверен
  тестом с таблицей документа (110, 2097, 32780 страниц для трёх размеров RAM;
  при минимальной RAM текущей платы — 91). Порядок страниц: PML4, затем PDPT/PD/PT
  по первому использованию при обходе регионов по возрастанию адреса.
- **Handoff block v1.0** по GPA `0x0010_0000` строится одним кодом из той же карты и
  таблицы устройств: заголовок `0xA0`, `region_table_offset = 0xA0`,
  `device_table_offset = 0xA0 + 32·N`, 7 устройств по 48 байт, CRC-32, UUID и
  серийный номер из конфигурации (по умолчанию нули и пусто), `tsc_hz` из
  конфигурации (по умолчанию `1_000_000_000`). Типы регионов 1–4, 6–10 заполняются;
  тип 5 (`NVRAM`) не возникает. Тест сравнивает блок побайтно с эталоном,
  сгенерированным независимым скриптом по таблицам документа.
- **Состояние входа.** Long64/CPL0, `CR0 = 0x8001_0033`, `CR4 = 0x20`,
  `EFER = 0xD00`, `CR3 = 0x0060_0000`, `RIP = 0x0020_0000 + entry_offset`,
  `RSP = 0x0015_0000`, `RDI = 0x0010_0000`, остальные GPR 0, `RFLAGS = 2`.
  `CR4 = 0x20`, а не `0x620`: `0x620` — состояние вызова EFI-образа, которое
  ставит сама прошивка (FIRMWARE_BOOT.md, разделы 5 и 7), и paged-квант принимает
  только `CR4 = 0x20`.
- **Запуск.** `LoadedFirmwareBoard::run` — один квант существующего
  `run_paged_probe`. `run_boot(budget)` повторяет кванты: STOP-предикат —
  `Uart16550Handle::holding_occupied()` (квант завершается на записи THR, раздел 11
  FIRMWARE_BOOT.md); после каждого кванта holding → shift → транскрипт, затем
  `advance_time_ns(retired × 1 ns)` явным вызовом `VirtualClock`; останов по
  `HLT`, ошибке или бюджету (`Stopped` при исчерпанном бюджете отдаётся как
  `BudgetExhausted`).
- **COM1 при входе настроен** (`LCR = 0x03`, делитель 1), как у flat probe: иначе
  polling UART отклоняет запись THR (`UnsupportedConfiguration`). Это состояние
  входа перечислено в FIRMWARE_BOOT.md, раздел 5 (строка «COM1»); инициализация
  прошивки по разделу 11 записывает те же значения и остаётся допустимой.
- **Runner.** `od-runner boot-firmware --image <bin> --com1-out <file> --budget <steps>`
  печатает JSON-отчёт (шаги, кванты, причина останова, `rip`, SHA-256 образа и
  handoff block, виртуальное время, разобранная строка `OD-FW STOP`) и записывает
  транскрипт COM1. Строка `OD-FW STOP` разбирается только как ПОСЛЕДНЯЯ непустая
  строка транскрипта (FIRMWARE_BOOT.md, раздел 12): STOP, за которым идёт ещё
  текст, не считается STOP (код 0). Коды выхода: 0 — `HLT` без строки `OD-FW STOP`
  последней строкой; 3 — неверные аргументы или файл; 4 — отказ загрузчика
  (транскрипт не пишется); 5 — гость не остановился по `HLT` или ошибка прогона
  (транскрипт COM1 с уже записанными байтами пишется и в этом случае, даже пустой);
  6 — `HLT` после `OD-FW STOP` последней строкой.

**Границы (не реализовано и не доказано).**

- **Гостевой код ограничен allowlist paged-кванта.** `step_paged_probe` исполняет
  только `MOV RAX,[RBX]`, `MOV [RBX],RCX`, `IN AL,DX`, `OUT DX,AL`, `HLT` (без
  префиксов) и, по решению S7-03 ([CPU_CONTRACT.md](CPU_CONTRACT.md), «Расширение
  S7-03»), `MOV r8,imm8` (`B0+r ib`, без префиксов) и `MOV r16,imm16`
  (`66 B8+r iw`, ровно один `66`). Остальные формы `MOV imm` (`B8+r id`, REX,
  `C6`/`C7`) остаются `Unsupported`. Контракт входа обнуляет все GPR кроме `RSP` и
  `RDI`, поэтому образ `build_firmware.py --smoke` (`mov al,'X'; mov dx,0x3F8;
  out dx,al; hlt`) сам задаёт `AL` и `DX`: в машине он завершается `Halted`,
  `retired = 4`, COM1 содержит ровно `X` (тест
  `smoke_code_writes_x_to_com1_with_zero_gprs_at_entry`, runner —
  `smoke_code_prints_x_and_halts`). Исполнение настоящей прошивки (GDT/IDT, `LRETQ`,
  `MOV CR4`, `STI`, `CMP`, `Jcc`, `CALL`) по-прежнему требует расширения
  CPU-профиля — blocker S7-04, см. FIRMWARE_BOOT.md раздел 7.
- **Чтение ROM гостем через paged-кванты** по-прежнему отклоняется walker'ом как
  RAM-only (`UnreadableRegion`); прямое чтение ROM доступно на уровне шины и
  физической памяти. Для v1 это не требуется: образ исполняется из shadow.
- Fetch из ROM — `#PF`, error code `0x11`, адрес fetch (тест
  `instruction_fetch_from_rom_is_a_page_fault_by_nx`); backend-ошибка не
  возникает, пока листья ROM помечены NX.
- Повторное копирование shadow, handoff и таблиц страниц при сбросе платы выполняет
  `LoadedFirmwareBoard::platform_reset` (S7-26a, [FIRMWARE_BOOT.md](FIRMWARE_BOOT.md#121-сброс-платы-внутри-процесса-s7-26a)).
- Не реализованы: ACPI fixed hardware и reset register (S7-14, реализованы позже), TPM CRB (S7-22), ECAM (S4-30),
  доставка IRQ и APIC timer в paged-пути, SMP, выбор размеров RAM и
  UUID/серийного номера из командной строки runner (используются значения по
  умолчанию: 128 MiB, один экран, нулевой UUID, пустой серийный номер).
  Базовый firmware-профиль NVMe использует фиксированное MMIO. Опциональная
  PCI-привязка описана ниже в разделе «NVMe controller».
- Расширение handoff block (новые поля и устройства) повышает `version_minor`
  по правилам раздела 6.4 FIRMWARE_BOOT.md; машина выдаёт v1.0 и принимает образы
  с любым `handoff_minor_max`; v1.1 (запись NVMe) — только при `FirmwareBoardConfig::nvme = Some(..)`
  и образе с `handoff_minor_max ≥ 1`.

Проверки: `cargo test -p od-machine --offline` (модуль `firmware_board`, `physical_memory`),
`cargo test -p od-runner --offline --test firmware_boot_cli`,
`python3 -m unittest tools/build/test_build_firmware.py`; методика —
[FIRMWARE.md](../testing/FIRMWARE.md).

## Устройство NVRAM

`FirmwareBoardConfig::nvram` подключает окно `[0xFF00_0000, 0xFF04_1000)` как `Mmio`. В handoff v1.7 устройство 5 владеет регионом типа `NVRAM`. Плата хранит два банка в `NvramDevice`. Содержимое банка остаётся непрозрачным для машины. [Регистры, банки и Host API](NVRAM_DEVICE.md).

Квант прошивки передаёт простые чтения и записи через 4-КиБ страничный walker только для этого окна. Полный операнд должен помещаться в регион. Для прочих MMIO-окон действует прежний отказ. Выборка инструкции, чтение псевдодескриптора, read-for-write и locked-доступ через это окно не поддерживаются.

`NvramDevice` возвращает `UnsupportedSpace` для PIO, `UnsupportedWidth` при ширине регистра кроме dword, `Misaligned` для невыровненного регистра и `CrossesRegister` на границе данных и регистров. Неизвестное смещение и выход за диапазон передачи дают `UnknownOffset`. Чтение окна без READ даёт `Busy`, запись без WRITE — `ReadOnly`. Запись в `STATUS` и `ACTIVE_LEN` запрещена, чтение `CMD` даёт `WriteOnly`. Ошибка доступа сохраняет состояние устройства. Ошибки команд после корректной записи в `CMD` сообщаются через `STATUS`.

Хост получает снимок между квантами. Повторная загрузка может создать плату с тем же ROM и восстановленным снимком NVRAM. Общий сброс платы — `LoadedFirmwareBoard::platform_reset` (S7-26a): регистры устройства и запись в полёте теряются, закоммиченные банки остаются. Сохранение снимка в VM bundle — S9-26.

## NVMe controller (S7-16, срезы A и B)

**Статус.** Минимальный S7-16 принят. Реализованы управление и ввод-вывод
`Read`, `Write`, `Flush`. В opt-in PCI-профиле подтверждены чтение и MSI-X
в запущенном Linux. Контракт устройства и его границы описаны отдельно.
Код находится в `crates/od-machine/src/nvme/`. [NVME_DEVICE.md](NVME_DEVICE.md) · [Гостевой прогон](../reports/STAGE_7_REPORT.md#чтение-raw-и-msi-x-в-linux).

- **Шина.** В firmware-профиле контроллер занимает регион `[0xFC00_1000, 0xFC00_3000)`. В PCI-профиле `MachineBus` открывает BAR0/1 `[0xFC00_4000, 0xFC00_8000)` через MMIO, включая таблицу MSI-X и PBA.
  Ошибки доступа к окну — `DeviceError` по классам контракта шины:
  `UnsupportedWidth` (байт, слово, `Qword` на doorbell), `Misaligned`,
  `CrossesRegister` (`Qword` через границу регистров), `ReadOnly`, `WriteOnly`
  (чтение doorbell), `Busy` (запись `AQA/ASQ/ACQ` при `CC.EN = 1`), `UnknownOffset`
  (doorbell за пределами пяти очередей), `UnsupportedConfiguration` (doorbell при
  `CSTS.RDY = 0`). Ошибка возвращается до изменения состояния устройства.
- **DMA.** `BusDevice` не видит RAM, поэтому doorbell только запоминает указатели;
  команды исполняются в `NvmeHandle::service(&mut MachineBus, committed_write)`,
  который владелец машины вызывает между квантами. Доступ к памяти — только RAM
  (через `MachineBus::memory_kind` и `read_flat_bytes`/`write_flat_bytes`); записи
  проходят через `committed_write`. PRP в MMIO, ROM, framebuffer и вне карты —
  статус `Data Transfer Error` команды, не доступ и не паника. Обращение к очереди
  не в RAM — `CSTS.CFS`.
- **Прерывания.** PCI-профиль доставляет INTA через IOAPIC, а MSI-X — напрямую в IRR целевого Local APIC после записи CQE. Фиксированный MMIO-профиль использует опрос. [Контракт прерываний NVMe](NVME_DEVICE.md#31-прерывания-pci).
- **Плата.** `FirmwareBoardConfig::nvme: Option<NvmeConfig>` (по умолчанию `None`):
  при `Some` в таблицу устройств handoff block добавляется запись кода 8 (`version_minor = 1`),
  в карту — регион `Mmio`, на шину — устройство; `LoadedFirmwareBoard::nvme()` отдаёт handle;
  `run_boot` останавливает квант после doorbell-записи и вызывает `service` между квантами и один
  раз перед запуском. Без конфигурации окна нет (`Unmapped`), handoff остаётся v1.0, эталон
  `REFERENCE_HANDOFF_HEX` не изменился.
- **Ввод-вывод (срез B).** `Read`, `Write`, `Flush` исполняются в том же `service` через
  `trait NvmeBackend` (512-байтные секторы); `od-machine` реализует только `InMemoryBackend`
  (тесты, мини-образы), файловый/CoW backend подключает владелец машины над `od-storage` через
  `NvmeHandle::attach_backend` (только при `CC.EN = 0`; `NSZE` = размер backend). Без backend
  I/O-команда — `Namespace Not Ready`. Ошибки (NSID ≠ 1, LBA вне `NSZE`, `NLB` больше `MDTS`,
  `FUA`/`LR`/`DSM`/SGL, PRP, отказ backend) — статусы NVMe, не паника; файлового ввода-вывода
  в `od-machine` нет. Значения статусов `LBA Out of Range`/`Namespace Not Ready`/`Internal Error`
  не сверены с NVMe Base 2.4.
- **Не реализовано:** LBA 4 KiB, файловый backend, `Dataset Management`/`Write Zeroes`/`Compare`,
  `FUA`, прерывания (нет IRQ-маршрута), PCI-привязка (S4-30), SGL, Namespace Management, firmware
  download/commit, несколько NS, `Device Self-test`. Время исполнения команд не моделируется.

Проверки: `cargo test -p od-machine --offline` (модули `nvme`, `firmware_board`).

## PCI ECAM window and type-0 function model (S4-30a)

Реализация: `crates/od-machine/src/pci_ecam.rs`. Это ограниченный срез S4-30; плата по умолчанию ECAM не регистрирует, production-функций нет.

**Окно.** `PCI_ECAM_BASE = 0xFC10_0000`, сегмент 0, `start_bus = end_bus = 0`, длина `0x10_0000` (1 MiB), MMIO-регион `[0xFC10_0000, 0xFC20_0000)`. Не пересекает дисплей `[0xF400_0000, 0xFC00_1000)`, NVMe `[0xFC00_1000, 0xFC00_3000)` и APIC `0xFEC0_0000+`. Адрес: `base + (bus << 20) + (dev << 15) + (fn << 12) + reg`, dev 0..31, fn 0..7, reg 0..4095. Регистрировать можно только функцию 0 (`add_function` с fn ≠ 0 → `InvalidSlot`): type-0 header не задаёт multi-function bit, поэтому функции 1..7 не моделируются.

**Доступ.** Допустимы 1, 2, 4 байта, выровненные по ширине. Qword → `DeviceError::UnsupportedWidth`; невыровненный → `DeviceError::Misaligned`; PIO-пространство того же устройства обслуживает legacy conf1 (S4-30b, ниже); bus вне `[start, end]` → `DeviceError::UnknownOffset` (не absent). Ошибка возвращается до любых изменений состояния. Чтение absent-функции даёт все единицы своей ширины; запись в absent-функцию молча игнорируется.

**Type-0.** Vendor/Device ID, Revision/Class code, Header type 0x00 — RO. Class code — 24 бита; большее значение → `Type0ConfigError::InvalidClassCode`. Command (0x04): RW только bit0 (I/O), bit1 (memory), bit2 (bus master); Status RO 0. BAR0 (0x10): 32-bit memory, размер степень двойки ≥ 16, хранит биты `!(size-1) & 0xFFFF_FFF0`, биты 0..3 RO 0; запись 0xFFFF_FFFF читается как маска. BAR1..BAR5 RO 0. Interrupt line (0x3C) RW 8 бит, Interrupt pin (0x3D) RO 0, Capabilities pointer 0. Область 0x100–0xFFF: чтение 0, запись игнорируется.

**Не реализовано.** MCFG (S4-31); handoff код 7 остаётся `PRESENT = 0` без флага `pci_ecam` (S4-30c); BAR→MMIO mapping и влияние Command на decode (биты только хранятся); host bridge 00:00.0; capabilities, MSI/MSI-X. Тестовый endpoint существует только в `#[cfg(test)]`.

## Legacy conf1 (S4-30b)

Реализация: `crates/od-machine/src/pci_ecam.rs`. Ограниченный срез S4-30; плата по умолчанию conf1 не регистрирует, host bridge, MCFG, E820 и handoff код 7 не объявлены.

**Общее состояние.** Conf1 — второй front-end того же объекта `PciEcam`: одна таблица функций `(bus, device, function)`, один `device_index`, две привязки (`MmioBinding` на окно ECAM и `PortRegionBinding` на `PCI_CONF1_BASE_PORT = 0xCF8`, `PCI_CONF1_PORT_BYTES = 8`, т. е. `0xCF8..=0xCFF`). Различаются по `DeviceAccess::space` (`Pio` — conf1). Чтение/запись функции идёт через общие `config_read`/`config_write`, поэтому BAR, Command и Interrupt line видны через оба пути. Дублирования состояния нет. Единственное собственное состояние conf1 — address latch.

**Address latch (0xCF8, offset 0).** Меняется только dword-записью. Хранятся биты: 31 enable, 23:16 bus, 15:11 device, 10:8 function, 7:2 register (маска записи `0x80FF_FFFC`). Биты 30:24 и 1:0 не поддерживаются: при записи отбрасываются, читаются как 0. Dword-чтение возвращает сохранённое значение (нужно для Linux `pci_check_type1`: `outl(0x80000000)` → `inl` даёт `0x80000000`). Байтовые и словные записи в 0xCF8 игнорируются, байтовые и словные чтения 0xCF8 возвращают все единицы (`0xFF`/`0xFFFF`) независимо от латча: байты латча по узкому доступу не раскрываются.

**Data port (0xCFC..=0xCFF, offsets 4..7).** Ширины 1/2/4. Обращение шириной `w` по порту `0xCFC + n` идёт к регистру `(latch & 0xFC) + n` функции из латча; `n` выровнен по `w` (иначе `Misaligned`). Qword → `UnsupportedWidth` (шина отвергает его раньше устройства). Обращение, выходящее за 0xCFF (например dword на 0xCFE), шина отвергает как `CrossesRegion`. Если bit31 = 0, либо bus ≠ 0, либо функции нет: чтение — все единицы своей ширины, запись игнорируется (не ошибка). Состояние функции при ошибке ширины/выравнивания не меняется.

**0xCFA и 0xCFB.** Байтовые и словные записи игнорируются (Linux делает `outb(0x01, 0xCFB)` в `pci_check_type1` и `outb(0, 0xCFA)` в conf2-пробе), чтения возвращают все единицы своей ширины, состояния нет. Conf2 (0xCFA как forward register, окно 0xC000–0xCFFF) не реализован.

**0xCF9.** Reset control (PIIX) не реализован: байтовое чтение и запись → `DeviceError::UnknownOffset` («регистра нет, reset не реализован»), состояние не меняется. Это явная ошибка, а не тихий успех: тихий игнор скрыл бы неработающий reset (ядро первым делает `inb(0xCF9)`, а затем в любом случае переходит к `BOOT_TRIPLE`). `UnsupportedSpace` не используется: PIO-пространство поддерживается. Слово на 0xCF9 → `Misaligned`.

**Linux-пробы** (`linux-source-7.0.0`, `arch/x86/pci/direct.c`: `pci_check_type1` строки 223–241, `pci_check_type2` 243–261). `pci_check_type1` (`outb 0xCFB`, `inl 0xCF8`, `outl 0x80000000`, `inl 0xCF8`, `pci_sanity_check`, восстановление латча) проходит часть с чтением латча назад. `pci_check_type2` (`outb 0` на 0xCFB, 0xCF8, 0xCFA, затем `inb(0xCF8) == 0 && inb(0xCFA) == 0`) здесь не проходит при любом состоянии латча, потому что байтовые чтения 0xCF8/0xCFA дают 0xFF; ложного прохождения нет и чтения 0xC000–0xCFFF не начинаются.

**Задача для S4-31/S4-38: `pci_sanity_check`.** Без host bridge и без DMI-года BIOS ≥ 2001 conf1 отвергается `pci_sanity_check` (`direct.c:204–216`: нужен год BIOS ≥ 2001, либо любая функция с классом 0x0600/0x0300 на bus 0 (цикл по всем devfn 0..0xFF), либо vendor Intel/Compaq). В этом срезе на bus 0 нет ни одной подходящей функции (тестовый endpoint 00:03.0 с vendor 0xD1A6 и классом 0xFF0000 условию не удовлетворяет), поэтому Linux после этого пробует conf2, и он тоже не проходит — это ожидаемо, не дефект. Обход через `pci=nochecks` (`PCI_NO_CHECKS`, `direct.c:200`) в этом срезе не используется. Чтобы Linux принял conf1, S4-31/S4-38 должны дать DMI/SMBIOS с годом ≥ 2001 или объявить функцию класса host bridge/VGA на bus 0 (S4-30 целиком).

**Отличие bus > 0.** Conf1 может адресовать 256 шин. Bus ≠ 0 — обычный absent-доступ (все единицы, запись игнорируется). В ECAM тот же bus выходит за `[start_bus, end_bus]` и даёт `DeviceError::UnknownOffset`; окно покрывает только bus 0, поэтому по шине такие адреса до устройства не доходят.

**Не реализовано.** Host bridge 00:00.0, MCFG, E820, handoff код 7 (`PRESENT = 0` без флага `pci_ecam`, S4-30c), BAR→MMIO mapping и Command.MEM decode, conf2 (0xCFA forward register и окно 0xC000), reset control 0xCF9, регистрация в `firmware_board.rs` и manifest платы.

## ECAM и conf1 на плате (S4-30c)

Реализация: `crates/od-machine/src/firmware_board.rs` (`FirmwareBoardConfig::pci_ecam`, `board_devices`, `load_firmware_image`), дескриптор — `pci_ecam.rs` (`PCI_ECAM_DESCRIPTOR`). Ограниченный срез S4-30: родительский S4-30 остаётся открытым.

**Флаг.** `FirmwareBoardConfig::pci_ecam: bool`, по умолчанию `false`. Без флага плата, карта, шина и handoff не отличаются от S4-30b: handoff v1.0 байт в байт прежний (golden-тест), окно и порты `0xCF8..=0xCFF` не зарегистрированы (чтение — ошибка шины), код 7 — `PRESENT = 0`. Флаг не требует `handoff_minor_max ≥ 1`: код 7 — слот v1.0.

**Дескриптор.** `PciEcamDescriptor { segment, start_bus, end_bus, base, size }`, константа `PCI_ECAM_DESCRIPTOR = { 0, 0, 0, 0xFC10_0000, 0x10_0000 }`; compile-time проверка `size == (end_bus - start_bus + 1) << 20`. Это единственный источник записи кода 7 (`param0 = segment`, `param1 = handoff_param1() = start_bus | end_bus << 8`) и будущей аллокации MCFG (S4-31).

**Карта.** С флагом запись кода 7 `PRESENT` (MMIO) даёт в `MachineMemoryMap` регион `MemoryRegionKind::Mmio` `[0xFC10_0000, 0xFC20_0000)`: существующий вариант, не RAM, без backing-хранилища (`write_bytes`/`patch_physical_ram` → `ReadOnlyRegion(Mmio)`). Регион порождается общим циклом по MMIO-записям таблицы устройств (как LAPIC, IOAPIC, дисплей, NVMe), отдельного пути карты нет. Проверено: нет пересечений с RAM (в том числе максимальным `0xF_F400_0000`), ROM, обоими framebuffer, регистрами дисплея, NVMe, IOAPIC, LAPIC; окно ниже 4 GiB и внутри physical36. Boot page tables (4 KiB identity, `RW|NX`) покрывают окно тем же PT-блоком, что и регистры дисплея, `N` не меняется.

**Шина.** Одно устройство `PciEcam` добавляется в `bus_devices` после COM1, LAPIC, IOAPIC, дисплея и (если включён) NVMe; его индекс — `bus_devices.len()` в момент добавления. Две привязки с одним `device_index`: `MmioBinding` на `0xFC10_0000` и `PortRegionBinding` на `0xCF8` длиной 8. Состояние общее (S4-30a/S4-30b). Плата не вызывает `add_function`: bus 0 пустой, все 32 устройства (и функции 1..7) читаются `0xFFFF_FFFF`/`0xFFFF`/`0xFF`, записи молча игнорируются. Побочное следствие: с флагом `0xCF9` отвечает `DeviceError::UnknownOffset` (reset control не реализован), без флага — `UnmappedPort`.

**Diagnostic endpoint.** Только в `#[cfg(test)]`: тесты строят `PciEcam` с функцией 00:03.0 (`Type0Config`) и передают её приватному `load_firmware_image_with_pci`; публичный `load_firmware_image` всегда передаёт пустой `PciEcam`. Без флага переданное устройство не регистрируется (тест).

**Linux-пробы.** На экземпляре платы: `pci_check_type1` проходит часть с чтением латча назад; `pci_check_type2` отвергается. Как и в S4-30b, `pci_sanity_check` (`direct.c:204–216`) отвергает conf1, пока нет ни DMI-года BIOS ≥ 2001, ни функции на bus 0 с классом 0x0600 или 0x0300, ни vendor Intel/Compaq; обход — `pci=nochecks`. После отказа Linux пробует type2, и он тоже не проходит. Задача S4-31/S4-38 (SMBIOS-год или функция класса host bridge/VGA), не дефект среза.

**Не реализовано.** MCFG и PNP0C02/`_CRS` (S4-31); генерация E820 и loader (S4-37); host bridge и любые функции; BAR→MMIO mapping и PCI MMIO aperture; MSI/MSI-X; ACPI PM и reset register (сделано отдельным срезом S7-14a, см. раздел ниже); дефолтный handoff не менялся.

## ACPI fixed hardware (S7-14a)

Реализация: `crates/od-machine/src/acpi_pm.rs` (`AcpiPm`, `AcpiPmHandle`, `ACPI_PM_DESCRIPTOR`, `MachineStopReason`), подключение — `firmware_board.rs` (`FirmwareBoardConfig::acpi_pm`). Полный FADT, без `HW_REDUCED_ACPI`; сам FADT — S4-31, сброс как операция — S7-26.

**Флаг и handoff.** `FirmwareBoardConfig::acpi_pm: bool`, по умолчанию `false`. Без флага порты PM-блока не зарегистрированы (чтение — `UnmappedPort`). Handoff block не описывает PM-блок (у handoff v1.0/v1.1 нет кода устройства для него, новый код потребовал бы изменения контракта): с флагом `acpi_pm` и без него handoff байт в байт одинаков и равен эталону (golden-тест). Позднее S4-31c ввёл код 9 (handoff v1.2) под отдельным флагом `acpi_tables`, см. раздел «Расширение manifest: ACPI-таблицы, PM-блок и SCI (S4-31c)». Источник адресов для FADT — типизированный `ACPI_PM_DESCRIPTOR`, как `PCI_ECAM_DESCRIPTOR` для MCFG. Карта памяти не меняется: PM-блок живёт только в пространстве портов.

**Адреса (PIO, один блок `0x0600..0x0610`, одно устройство, одна `PortRegionBinding`).** Выбор: рядом нет ни одного устройства платы (COM1 `0x3F8..=0x3FF`, conf1 `0xCF8..=0xCFF`; NVMe, ECAM, LAPIC, IOAPIC, дисплей — MMIO и в пространстве портов не присутствуют); блок целиком ниже `0xCF8`; `0xCF9` остаётся `UnknownOffset` у устройства conf1 (reset control PCI не реализован). База `0x0600` — привычная для PM-блока PC-платформ; порты RTC/CMOS `0x70/0x71`, PIC, PIT и 8042 блок не пересекает.

| Поле FADT | Значение |
|---|---|
| `SMI_CMD` | `0` (нет SMI-обработчика; `ACPI_ENABLE/DISABLE` нет, `SCI_EN` не переключается) |
| `PM1a_EVT_BLK` / `PM1_EVT_LEN` | `0x0600` / 4 (`PM1_STS` 16 бит + `PM1_EN` 16 бит) |
| `PM1b_EVT_BLK`, `PM1b_CNT_BLK`, `PM2_CNT_BLK`, `GPE0_BLK`, `GPE1_BLK` | `0` (не реализованы) |
| `PM1a_CNT_BLK` / `PM1_CNT_LEN` | `0x0604` / 2 |
| `PM_TMR_BLK` / `PM_TMR_LEN` | `0x0608` / 4; флаг `TMR_VAL_EXT = 0` (24-битный счётчик) |
| `RESET_REG` | SystemIO (address space id 1), `register_bit_width = 8`, `bit_offset = 0`, access size byte, адрес `0x060C`; флаг `RESET_REG_SUP = 1` |
| `RESET_VALUE` | `0x06` |
| `X_PM1a_EVT_BLK`, `X_PM1a_CNT_BLK`, `X_PM_TMR_BLK` | те же блоки как SystemIO GAS (ширина 32/16/32 бита, смещение 0) с AccessSize 2 (слово), 2 (слово), 3 (двойное слово) — ширины целого регистра, которые принимает устройство (S7-32, срез L, [ISSUE-698](../KNOWN_ISSUES.md#issue-698); до среза — AccessSize 0) |
| `\_S5` `SLP_TYP` | `5` (`ACPI_SLP_TYP_S5`; пакет `\_S5` составит S4-31) |

**Регистры.** Доступ — только `IoSpace::Pio`, ширина 1/2/4 (Qword — `UnsupportedWidth`), естественное выравнивание (`Misaligned`), доступ не выходит за один регистр (`CrossesRegister`, в частности 32-битный доступ к `PM1_STS`+`PM1_EN` отвергается), промежутки блока (`0x606`, `0x607`, `0x60D..0x60F`) — `UnknownOffset`; все проверки до изменения состояния.
- `PM1_STS` (`+0`, 16 бит): write-one-to-clear; источников событий нет, поэтому после сброса `0` и не меняется (`TMR_STS`, `PWRBTN_STS` и прочие не взводятся).
- `PM1_EN` (`+2`, 16 бит): хранит биты `TMR_EN`, `GBL_EN`, `PWRBTN_EN`, `SLPBTN_EN`, `RTC_EN` (маска `0x0721`), остальные читаются `0`. SCI-маршрута нет, включённое событие ничего не вызывает.
- `PM1_CNT` (`+4`, 16 бит): `SCI_EN` (бит 0) равен `1` с сброса и записью не меняется; `BM_RLD` (бит 1) и `SLP_TYP` (биты 10–12) хранятся; `GBL_RLS` (бит 2) и `SLP_EN` (бит 13) только для записи и читаются `0`, `GBL_RLS` игнорируется. Запись `SLP_EN = 1` с `SLP_TYP = 5` (с учётом `SLP_TYP`, записанного в той же или предыдущей записи, в том числе побайтно) защёлкивает причину `Shutdown`. Запись `SLP_EN = 1` с другим `SLP_TYP` игнорируется: другие состояния сна не объявлены (`\_S1..\_S4` в DSDT нет), машина не засыпает, `SLP_EN` читается `0`.
- `PM_TMR` (`+8`, 32 бита): `floor(now_ns × 3 579 545 / 10^9)` по virtual clock платы, маска 24 бита (счётчик заворачивается каждые 4.687 с). Время не идёт само: оно двигается только `LoadedFirmwareBoard::advance_time_ns` и `run_boot`; само чтение устройством часы не двигает, Δ применяет `run_boot`. **Тик (S4-37a2, ISSUE-067):** каждое успешное чтение `PM_TMR` гостем (любой ширины) ставит в очередь устройства Δ = 1 мкс (внутренняя константа 1000 нс); значение чтения берётся до тика, устройство часы не двигает. `run_boot` сбрасывает очередь на старте (чтения хоста через `bus_mut()` часы не двигают), заканчивает квант после гостевого чтения `PM_TMR` и применяет тик к часам до следующей инструкции; на всех платах с `acpi_pm`, не только под Linux-профилем. Квант на каждое чтение — стоимость по производительности (S4-37a, риск г). Публичный `run()` (один квант) тики не применяет: гостевые чтения `PM_TMR` внутри него ставят Δ в очередь, которую `run_boot` сбрасывает на старте (решение 16а: тики только внутри своего запуска). Инъекция «без тика» на прямолинейном образце даёт AL=0 и Halted, а не BudgetExhausted: цикл ожидания по 0x608 требует CMP/Jcc, которых в paged-профиле нет (переносится в a6 или S4-38). Запись — `ReadOnly`.
- `RESET_REG` (`+0xC`, 1 байт): чтение — `WriteOnly`, запись шире байта — `CrossesRegister`. Запись `0x06` защёлкивает причину `Reset`; другое значение игнорируется.

**Остановка.** Причина защёлкивается на устройстве (`MachineStopReason::{Reset, Shutdown}`), первая причина остаётся: повторные команды её не меняют, снимает её только `LoadedFirmwareBoard::platform_reset` через `AcpiPmHandle::platform_reset` (S7-26a). `AcpiPmHandle::stop_request()` читает причину. С флагом `LoadedFirmwareBoard::run_boot` добавляет её опрос в предикат STOP после каждой инструкции: машина останавливается сразу после `OUT`, `FirmwareRunReport { exit: PagedProbeExit::Stopped, stop_reason: Some(..) }`; остановка по запросу не превращается в `BudgetExhausted`, повторный `run_boot` не исполняет ни одной инструкции. Без флага `stop_reason` всегда `None`. Сам сброс (состояние CPU и устройств, перезапуск) этим срезом не выполняется; он добавлен в S7-26a.

**Изменение API платы.** `LoadedFirmwareBoard::clock()` теперь возвращает снимок `VirtualClock` (значение), а не ссылку: часы платы лежат в `VirtualClockHandle`, который делят плата (единственный advancer) и таймер PM. `FirmwareRunReport` получил поле `stop_reason`.

**Передача S4-31 (поля FADT).** Нули в `PM1b_*`, `PM2_CNT_BLK`, `GPE0_BLK`, `GPE1_BLK` и их длинах, `SMI_CMD = 0`, `ACPI_ENABLE/DISABLE = 0`. `Flags`: `PWR_BUTTON = 1` и `SLP_BUTTON = 1` (источников кнопок нет; ноль означал бы «кнопка есть как fixed feature»), `FIX_RTC = 1` (`RTC_STS`/`RTC_EN` не поддержаны), `TMR_VAL_EXT = 0`, `RESET_REG_SUP = 1`, `HW_REDUCED_ACPI = 0`. Замечание: регистр `PM1_EN` (маска `0x0721`) хранит биты `PWRBTN_EN`/`SLPBTN_EN`/`RTC_EN`, но событий от этих источников нет — биты только запоминаются и не формируют `PM1_STS`. `SCI_INT` выбирает S4-31 (маршрут SCI пока не реализован). `RESET_REG`, `RESET_VALUE` и `\_S5` — из таблицы выше.

**Не реализовано.** `HW_REDUCED_ACPI`; сам FADT и таблицы MADT/MCFG (S4-31, S7-13); SCI и его маршрут к IOAPIC, события PM1, GPE0/GPE1, PM1b, PM2; `ACPI_ENABLE/DISABLE` через `SMI_CMD`; состояния сна S1–S4; сам сброс (S7-26); RTC/CMOS `0x70/0x71`, HPET, PIC/PIT/8042 (S7-14 и далее); `0xCF9` по-прежнему `UnknownOffset`. Проверка ACPI mode и SCI в гостевом Linux — S4-38.

## Расширение manifest: ACPI-таблицы, PM-блок и SCI (S4-31c)

Реализация: `crates/od-machine/src/firmware_board.rs` (`FirmwareBoardConfig::acpi_tables`, `board_devices`, `handoff_regions`, `handoff_minor`), SCI и упаковка `param0/param1` — `acpi_pm.rs` (`ACPI_SCI_GSI`, `ACPI_SCI_IRQ_FLAGS`, `AcpiPmDescriptor::handoff_param0/1`); разбор записи в прошивке — `firmware/uefi/src/acpi/manifest.rs` (`AcpiPmEntry::from_entry`). Нормативное описание полей — [FIRMWARE_BOOT.md](FIRMWARE_BOOT.md), раздел 6.3 (код 9).

**Флаг.** `FirmwareBoardConfig::acpi_tables: bool`, по умолчанию `false`; требует `acpi_pm` (иначе `FirmwareBoardError::Config`: запись описывала бы незарегистрированные порты). Без флага handoff не отличается от S4-30c/S7-14a: с `pci_ecam`, `acpi_pm` и NVMe по отдельности и вместе версия и число записей прежние, golden-тест `handoff_block_is_byte_identical_to_the_reference` не менялся. `acpi_pm` сам по себе handoff по-прежнему не меняет.

**Регион ACPI-таблиц (решение, не меняется этим срезом).** Область `[0x000E_0000, 0x000F_0000)`, 64 KiB, в единой карте платы — часть нижней RAM `[0, min(R, 0xF400_0000))` (нижняя RAM начинается с 0 и при `R ≥ 128 MiB` покрывает первый MiB), а не отдельный регион: карта (`MachineMemoryMap`), число boot page tables `N` и backing RAM не меняются, перекрытий в карте нет по построению. Подинтервалы описывает только таблица регионов handoff: `ACPI_TABLES` (код региона 11) `[0xE_0000, 0xE_F000)` и `ACPI_NVS` (код 12) `[0xE_F000, 0xF_0000)`; `[0xF_0000, 0x10_0000)` — нулевая `RAM`; handoff block стоит на `0x10_0000`, область ниже него. **Типы E820 как решение:** таблицы — ACPI reclaim, FACS (адрес `0xE_F000`) — NVS, RSDP — `0xE_0000`; сам E820 собирает S4-37, в этом срезе тип записан только в манифесте (код региона) и документах. Байты таблиц машина не пишет (область читается как нули).

**PM-блок и SCI.** Код устройства 9 (первый свободный код в диапазоне 8–15 после NVMe = 8; handoff v1.2, `version_minor = 2`, при флаге независимо от NVMe): `PRESENT | PIO`, `base = 0x0600`, `length = 0x10`, `param0 = 0x060C_0608_0604_0600` (EVT, CNT, TMR, RESET по 16 бит), `param1 = 0x0506` (`RESET_VALUE = 6`, `SLP_TYP` S5 = 5). SCI: `gsi = 9`, `irq_flags = 0b11` (level, active-low), через interrupt source override (запись MADT — S4-31f). Источник значений — `ACPI_PM_DESCRIPTOR` (S7-14a); у PM-устройства по-прежнему нет маршрута SCI в IOAPIC и событий PM1, это поля манифеста для FADT/MADT. Порядок записей: коды 1–7, затем 8 (NVMe, если есть), затем 9; `owner_device` регионов ECAM (6), NVMe (7), дисплея (3) не меняется.

**Версия и образ.** С флагом образ должен объявлять `handoff_minor_max ≥ 2` (иначе `ImageHeader { field: "handoff_minor_max" }`, как для NVMe). `region_count` растёт на 3, `device_count` на 1, `total_size` на 144 байта, CRC пересчитывается.

**Не делается.** Сериализаторы FADT, FACS, MADT, RSDP, XSDT и blob (S4-31d–g); E820 и loader (S4-37); изменения MCFG и DSDT; запись SCI в MADT (S4-31f); доставка SCI гостю и события PM1; разбор всего handoff-блока в прошивке (S7-04). `docs/plans/STAGE_4.md` не меняется.

**Linux-профиль платы (S4-37a1).** `FirmwareBoardConfig::linux_profile: bool`, по умолчанию `false`; требует `acpi_pm`, `acpi_tables` и `pci_ecam` (иначе `FirmwareBoardError::Config`). С флагом PIT `0x40..=0x43` и `0x61` работают на часах платы; кроме того привязываются пара 8259 `0x20..=0x21`/`0xA0..=0xA1` и «отсутствующие» порты (чтение `0xFF`, запись игнорируется) CMOS `0x70..=0x71`, `0xED` и `0x80` (sink-устройства часов не используют); `0x60/0x64` остаются ловушкой `UnmappedPort`. Handoff и manifest hash флаг не меняет. RDTSC — подсрез S4-37a6 (a6a/a6b), доставка IRQ — S4-38. **LAPIC timer (S4-37a4):** под флагом LAPIC платы строится `LocalApic::with_shared_timer` на тех же часах платы, что у PM_TMR и PIT (`FIRMWARE_APIC_TIMER_HZ` = 100 МГц; `has_timer() == true`, `LoadedFirmwareBoard::local_apic()`; частота — выбор платы (как в S4-27i2), не заявлена в handoff, ACPI или CPUID; Linux калибрует LAPIC-таймер сам (по PM_TMR, `lapic_timer_period = 0` без CPUID 0x15)), поэтому TMCCT идёт за `advance_time_ns` и порт-тиками `run_boot`, устройство часы не двигает; при masked LVT счёт идёт, masked expiry только снимает deadline. Без флага LAPIC прежний (`LocalApic::new()`, регистры `0x320/0x380/0x390/0x3e0` — `UnknownOffset`). Платой timer не диспетчеризуется (следствие: до диспетчеризации в S4-38 после истечения срока запись в `0x320`, `0x380`, `0x3e0` возвращает `DeviceError::Busy`, а TMCCT = 0): `dispatch_timer`, доставка IRQ и paged APIC timer — S4-38; гостевое MMIO-чтение TMCCT в paged-профиле пока невозможно (память paged-пробы — только RAM, qword `MOV RAX,[RBX]`), проверено чтением хоста. **Порт-тик PIT (S4-37a3, ISSUE-067):** под флагом PIT строится конструктором `Pit8254Devices::new_with_counter_tick`: чтение порта B `0x61` и чтение счётчика канала 2 `0x42` (живой или защёлкнутый байт) ставят по 1 мкс в общую очередь PIT; значение берётся до тика, устройство часы не двигает, отказанные чтения и чтения канала 0 не тикают. `LoadedFirmwareBoard::run_boot` сбрасывает очередь PIT на старте (чтения хоста через `bus_mut()` часы не двигают), заканчивает квант после гостевого чтения `0x61`/`0x42` и применяет накопленную сумму к часам вместе с retired-инструкциями (как для `PM_TMR`); публичный `run()` тики не применяет. Без флага порты не привязаны и поведение не меняется. Отклонение: цикл ожидания MSB по `0x42` (`quick_pit_calibrate`, `pit_calibrate_tsc`) требует CMP/Jcc/JMP, которых в paged-профиле платы нет, поэтому на плате проверены прямолинейные образцы (N чтений `0x42` двигают часы на N мкс; инъекция «без тика на `0x42`» оставляет счётчик на месте), а не зависание цикла до бюджета; полная калибровка — a6/S4-38.

**RDTSC на плате (S4-37a6b).** Под `linux_profile` `LoadedFirmwareBoard::run` исполняет голый `RDTSC` (`0F 31`) через `run_paged_probe_with_rdtsc`: временной порт, как `PM_TMR` и PIT. Значение — `clock.tsc()` (`floor(now_ns × tsc_hz / 10^9)`) на границе кванта: внутри кванта часы не движутся (отклонение). `RDTSC` завершает квант; `run_boot` применяет 1 мкс после PIT-тика (порядок: retired нс, PM, PIT, RDTSC), флаг ожидающего тика сбрасывается на старте; публичный `run()` тик не применяет. Без `linux_profile` `RDTSC` — `Unsupported{0x31}`.

**Отклонение S4-37a6b.** Полный MSB-цикл `calibrate` и UIP-цикл гостя на плате не выполнимы без `CMP`/`Jcc` в базовом профиле paged-пробы; они переносятся в S4-38.

## Boot contract собственного ядра (S4-32d)

Собственное маленькое ядро этапа 4 — отдельный профиль загрузки, а не протокол Linux и не общий ELF/segment loader: загрузчик принимает только raw image фиксированного размера. Entry state общий для всех probe-образов и зашит в код загрузчика (`stage4_flat_probe.rs`, значения раздела S4-32a, стр. 537–539): Long64/CPL0, CR3 = 0x1000, RIP = 0x400000, RSP = 0xA01000 (у Alias — 0xA00000), RDX = 0x3F8. Artifact не может его изменить. Профили различаются только значением RCX (0, store_value или payload) и CPU-профилем `run_stack` (разделы S4-32b и S4-32c). Собственные page tables лежат в image; загрузчик их не синтезирует, только проверяет. Загрузчик строит собственную маленькую карту (RAM и COM1), а не карту firmware board v1; «production memory» означает production RAM owner на этой карте. Неверный entry, битые code, page tables или stack mapping дают `loader_error` (exit 4) и ни одного UART-байта. Исполнение — только через interpreter. Не входит: размещение на board v1, ELF loader, exception delivery, IRQ, JIT.

Фиксированный direct Long64/CPL0 handoff достаточен для S4-32 и расширяется в S4-33 (CPL3, timer/IRQ, IDT/GDT, exception delivery). Подтверждение пользователя требуется только для фиксации: entry state задаёт код, а не artifact.

## Минимальная пара 8259 (S4-29a)

Реализация: `crates/od-machine/src/pic_8259.rs` (`Pic8259`, `Pic8259Pair`, `Pic8259Handle`, `Pic8259Snapshot`, `isa_irq_vector`). Устройство не подключено к плате (`FirmwareBoardConfig` не менялся, handoff и карта портов платы не затронуты): регистрация и порядок на шине — задача S4-29f/S4-37.

**Порты.** Master `0x20/0x21`, slave `0xA0/0xA1`; по одной `PortRegionBinding` длиной 2 на чип (устройство видит смещение относительно своей привязки, поэтому чипы — два устройства). Смещение 0 — команда (A0 = 0), 1 — данные. Пересечений с COM1 `0x3F8..=0x3FF`, conf1 `0xCF8..=0xCFF` и PM-блоком `0x600..0x610` нет (тест карты); NVMe, ECAM, LAPIC, IOAPIC — MMIO. Доступ: только `IoSpace::Pio` (иначе `UnsupportedSpace`), только байт (`UnsupportedWidth`), смещение 0..=1 (`UnknownOffset`); ошибка возвращается до изменения состояния.

**ISA_IRQ_VECTOR.** В проекте константы не было; взято из Linux x86-64: `ISA_IRQ_VECTOR(irq) = IRQ0_VECTOR + irq`, `IRQ0_VECTOR = FIRST_EXTERNAL_VECTOR (0x20) + 16 = 0x30`. Linux пишет ICW2 `0x30` (master) и `0x38` (slave) — это значения `Pic8259Role::icw2`. До S7-32 (срез O) модель принимала ровно их и отвергала остальные (в том числе PC-векторы `0x08/0x70`).

**ICW2 и вектор (S7-32, срез O, ISSUE-702).** HAL Windows (`ntkrnlmp` + `0x56a500`) пишет обоим кристаллам ICW2 `0xD8`. По даташиту Intel 8259A (231468-003, декабрь 1988; PDF SHA-256 `c15120c1…94674`, копия `pdos.csail.mit.edu/6.828/2005/readings/hardware/8259A.pdf`, полный документ, 24 страницы) в режиме 8086 «A15–A11 are inserted in the five most significant bits of the vectoring byte and the 8259A sets the three least significant bits according to the interrupt level. A10–A5 are ignored and ADI (Address interval) has no effect» (раздел «Initialization Command Words 1 and 2 (ICW1, ICW2)», с. 10; таблица «Content of Interrupt Vector Byte for 8086 System Mode», с. 9: IRn → T7–T3 и номер линии в битах 2:0). Поэтому принимается любой байт ICW2: он хранится как записан (`Pic8259Snapshot::icw2`), а вектор линии — `(ICW2 & 0xF8) | line` (`pending_vector`, `acknowledge`); для `0x30`/`0x38` Linux это прежние `ICW2 + line`, путь Linux не меняется. Пересечение базы с векторами исключений CPU (например, `0x08`) модель не проверяет: какой вектор выбрать — решение ОС, как на настоящем чипе. Остальные шаги автомата остаются строгими (ICW1, ICW3, ICW4 — ниже).

**Reset.** `IMR = 0xFF` (всё замаскировано), фаза `Uninitialized`, ICW2/ICW3 не заданы, флаг `aeoi` снят (S4-29a2), чтение команды-порта выбирает IRR. OCW1 принимается и в этой фазе (Linux пишет IMR до init и в `probe_8259A`).

**Автомат ICW.** Запись в команду-порт с битом 4 = 1 — ICW1. Принимается только cascade (SNGL = 0), edge (LTIM = 0), 8086 (IC4 = 1); биты 7:5 и бит 2 в режиме 8086 не используются и игнорируются (Linux пишет `0x11`). Принятая ICW1, как у микросхемы, очищает `IMR` в 0, выбирает IRR для чтения и сбрасывает ICW2/ICW3 и флаг `aeoi`; допустима посреди последовательности (рестарт). Затем запись в порт данных: ICW2 — любой байт (S7-32, срез O, абзац выше); ICW3 — master `0x04` (slave на IR2), slave `0x02` (cascade identity); ICW4 — `0x01` (8086, normal EOI) на обоих чипах; S4-29a2: на master дополнительно принимается ICW4 с AEOI (бит 1, `0x03`; бит 2 при BUF = 0 игнорируется) — флаг `aeoi` в `Pic8259Snapshot` хранится без эффекта (нет выхода INTR и ISR; OCW2/IRR/ISR/IMR не меняются; следующая ICW1 сбрасывает флаг), на slave AEOI отвергается (Linux `check_timer` → `init_8259A(1)` пишет `0x03` master и `0x01` slave); BUF (бит 3), SFNM (бит 4), reserved биты 7:5 и MCS-80 (бит 0 = 0) отвергаются. Любое недопустимое значение — `DeviceError::UnsupportedConfiguration` без изменения состояния (в том числе фазы), шаг можно повторить. После ICW4 фаза `Initialized`; запись в порт данных — снова OCW1. Чтение порта данных возвращает `IMR` в любой фазе.

**OCW** (поведение S4-29a; с S4-39g EOI и чтение IRR/ISR действуют — абзац «Доставка»). Запись в команду-порт с битами 4:3 = 00 — OCW2: принимается любое значение (EOI, specific EOI, rotate, set priority) без эффекта: нет линий и ISR. Биты 4:3 = 01 — OCW3: биты 1:0 (`RR`, `RIS`) выбирают чтение IRR (`0x0A`) или ISR (`0x0B`), `RR = 0` выбор не меняет; poll (бит 2), SMM/ESMM (биты 5:6) и бит 7 — `UnsupportedConfiguration`. Чтение команда-порта возвращает 0 для IRR и для ISR (линий и состояния обслуживания нет). OCW2/OCW3 между ICW1 и ICW4 — `DeviceError::Busy` (поведение микросхемы там не моделируется).

**Доставка.** В S4-29a — ни входных линий, ни выхода INTR; пара ничего не доставляла и не влияет на ExtINT/LINT0. С S4-39g у ведущего есть IRR, ISR и выход INTR ([CPU_CONTRACT, S4-39g](CPU_CONTRACT.md#8259-ir0-от-pit-eoi-inta-и-выход-из-hlt-под---user-mode-s4-39g)), EOI снимает биты ISR, а чтение порта команды возвращает IRR/ISR (абзац «OCW» выше описывает S4-29a); вектор — `(ICW2 & 0xF8) | line` (S7-32, срез O).

**Не входит.** ELCR `0x4D0/0x4D1`; ExtINT/LINT0; эффект AEOI (флаг только хранится, S4-29a2); IRQ-линии и маршрут IRQ0 (S4-29f); poll и special mask mode; PIT, CMOS/`0xED`, INTIN2; подключение к плате и handoff; MADT/FADT (`PCAT_COMPAT`). Отчёт: [S4-29a](../reports/STAGE_4_S4-29A_2026-11-09.md).

## Явно отсутствующие порты (S4-29b)

Реализация absent-поведения: `crates/od-machine/src/absent_ports.rs` (`AbsentPorts`, `AbsentPortRegion`). При `rtc_base_unix_seconds = None` RTC не подключён; регистрация absent-портов в Linux-профиле — S4-37/S7-14. При заданном timestamp на те же порты привязывается единственный `RtcCmos` из `crates/od-machine/src/rtc_cmos.rs`.

**Зачем.** Порт без привязки даёт `BusAccessError::UnmappedPort` (остановка), а не чтение `0xFF`. Stock Linux также обращается к `0xED` (`outb_p`/`io_delay`). Отсутствующее RTC поведение на `0x70/0x71` требуется только когда явная конфигурация платы не задаёт RTC.

**Источник и профиль.** Основной источник register semantics: Motorola Semiconductor Technical Data, *MC146818 Real-Time Clock Plus RAM (RTC)*, MC146818/D, ADI856R3 (1988), [datasheet PDF](https://pccomponents.com/datasheets/MOTO-MC146818.PDF). Модель покрывает выбранный register interface only; она не эмулирует oscillator/divider, SQW, DST, battery backup, alarm comparison or periodic/alarm/update IRQ. Alarm bytes are readable/writable storage only; status C is always zero. For reset-selected 32.768 kHz, UIP asserts 244 µs before the virtual-second boundary and stays high through the 1.984 ms update cycle from Table 6. The datasheet has a discrepancy: an earlier access-window paragraph says 1,948 µs, but the Update Cycle prose and Table 6 say 1,984 µs; this model follows the latter. Alternate divider/time-base writes do not retime UIP.

**Порты и поведение.** В absent-режиме `0x70` (индекс) и `0x71` (данные) занимают одну привязку длиной 2 (`0x72/0x73` не привязаны); чтение — `0xFF`, запись игнорируется. При настроенном RTC `RtcCmos` занимает ту же привязку и реализует CMOS index, бит NMI-disable (бит 7), календарные регистры и RAM через data-порт. Только байтовый PIO доступ допустим. Источник — явный `rtc_base_unix_seconds` из handoff; часы используют shared `VirtualClock`, BCD/24-hour — reset default, поддержаны binary/12-hour, SET freeze/rebase, century `0x32` и календарный rollover. Status C всегда 0; RTC periodic/alarm/update interrupts не подключены (ISSUE-509). `0xED` остаётся отдельным absent/sink портом длиной 1.

**Время RTC при загрузке прошивки.** Байтовый доступ к данным RTC на `0x71` получает время начала кванта плюс число завершённых до этой инструкции циклов vCPU. UIP, чтение календаря, SET и запись календаря используют одну такую отметку на доступ. Диагностический PIO вне исполнения гостя читает текущее значение общих часов. Продвижение общих часов по завершении кванта начисляет каждый цикл один раз. При переполнении времени байтовый доступ к RTC завершается с `DeviceError::ClockOverflow` до изменения регистра или завершения инструкции. [Контракт кванта](FIRMWARE_BOOT.md#8-решение-3-модель-таймеров-и-событий).

**Политика ширин.** Только байт. Слово, dword и qword на устройстве — `DeviceError::UnsupportedWidth` без эффекта; MMIO — `UnsupportedSpace`; смещение за привязкой — `UnknownOffset`. Основание: единая политика PIO-устройств модели (8259, UART, conf1 отказывают, а не выдумывают значение), Linux на этих портах использует только `inb`/`outb`; обращение шире байта означает, что гость делает то, что модель не аудировала, и остановка информативнее выдуманного `0xFFFF`. На шине: слово на `0x70` доходит до устройства (`Device{UnsupportedWidth}`), слово на `0x71`, dword на `0x70` и любое широкое обращение к `0xED` пересекают границу привязки — `BusAccessError::CrossesRegion` до устройства, qword — `UnsupportedWidth` на шине.

**Связь с FADT.** Manifest entry code 10 и `rtc_base_unix_seconds` совместно задают наличие RTC в handoff v1.4. Для настроенного RTC FADT builder очищает `IAPC_BOOT_ARCH` bit 5 (`CMOS_RTC_NOT_PRESENT`) и задаёт `Century = 0x32`; без RTC bit 5 установлен и century field равен 0. Валидация handoff/FADT проверяет эту согласованность. `LEGACY_DEVICES` (бит 0) и `8042` (бит 1) остаются 0. `0xED` в FADT не отражается.

**Остаются без привязки (решение открыто).** `0x60/0x64` (8042) не имеют устройства; обращение — `UnmappedPort` (остановка). Порт `0x80` (POST) — sink (`AbsentPortRegion::Post`) только под `linux_profile` (S4-37a1); без флага он `UnmappedPort`. Решение по 8042 остаётся открытым: это ловушка, а не решение, пока пользователь не выбрал (модель, sink, остановка); тест `open_decision_ports_stay_unmapped` фиксирует состояние и должен быть изменён осознанно. FADT при этом объявляет `8042 = 0` и `LEGACY_DEVICES = 0`.

**Граница S4-29b.** Историческое решение S4-29b об absent RTC применяется только когда нет явного handoff RTC config. S7-14 добавил опциональный `RtcCmos` и синхронное обновление FADT. В absent-режиме и при отсутствии Linux-профиля правила absent-привязки сохраняются. 8042 остаётся без привязки; `0x80` на плате — только под `linux_profile` (S4-37a1). Отчёт: [S4-29b](../reports/STAGE_4_S4-29B_2026-11-09.md).

## PIT 8254 и порт 0x61 (S4-29c)

Реализация: `crates/od-machine/src/pit_8254.rs` (`Pit8254Devices`, `Pit8254`, `PitPortB`, `Pit8254Handle`, `PitSnapshot`, `pit_ticks`). Устройства не подключены к плате (`FirmwareBoardConfig`, handoff, manifest, firmware, MADT, FADT не менялись); линий IRQ и событий нет (S4-29f). Модель только отвечает на обращения к портам и вычисляет счётчики из общего `VirtualClockHandle` в момент обращения.

**Порты.** Блок счётчиков `0x40..=0x43` — одна привязка длиной 4 (`0x40` канал 0, `0x41` канал 1, `0x42` канал 2, `0x43` управляющее слово); порт B `0x61` — отдельная привязка длиной 1 (устройству нужны смещения относительно своей привязки, поэтому два устройства с общим состоянием). Пересечений с PIC `0x20/0x21`, `0xA0/0xA1`, CMOS `0x70/0x71`, `0xED`, COM1, conf1 и PM-блоком нет; тест собирает одну `MachineBus` со всеми привязками. `0x60`, `0x62`, `0x44` остаются без привязки (`UnmappedPort`). Доступ: только `IoSpace::Pio` (`UnsupportedSpace`), только байт (`UnsupportedWidth`), смещение за привязкой — `UnknownOffset`; чтение `0x43` — `WriteOnly`; любое обращение к `0x41` (канал 1 не моделируется) — `UnsupportedConfiguration`. Ошибка возвращается до изменения состояния (операция выполняется на копии снимка).

**Формула счёта.** Вход 1 193 182 Гц, свободно идёт с виртуального времени 0. Индекс такта `T(t) = floor(t_ns * 1 193 182 / 10^9)` (одно деление `u128` от абсолютного времени, не сумма по шагам: три шага по 333 333 нс дают 1193 такта, а не 3 * 397 = 1191). Загрузка счёта завершается на такте `T0 = T(t_запись)`, прошедшее число тактов `e = T(now) - T0`. Выравнивание по общей сетке тактов, а не по моменту записи: генератор не перезапускается записью, поэтому запись в 0,9 мс и чтение в 1,1 мс дают 239 пересечённых фронтов (а не floor(0,2 мс * f) = 238). Эффективный reload `N` — записанное 16-битное значение, 0 означает 65 536.

**Режим 0 (interrupt on terminal count).** Значение счётчика `(N - e) mod 65 536` (за нулём считает дальше, 0xFFFF). OUT низкий после управляющего слова и после записи счёта, становится высоким при `e >= N` (счётчик = 0) и остаётся высоким до новой записи. Первый байт нового счёта (в режиме LSB+MSB) останавливает счёт и опускает OUT; счёт стартует по завершающему байту на текущем такте. Нет IRQ, нет события.

**Режим 2 (rate generator).** С `p = e mod N` значение счётчика `N - p` (`1..=N`, при `N = 65 536` и `p = 0` читается 0), OUT высокий, кроме одного такта, когда `p = N - 1` (счётчик = 1), то есть низкий на один такт перед перезагрузкой. Счёт 1 в режиме 2 недопустим: `UnsupportedConfiguration` на завершающем байте (при LSB+MSB первый байт уже принят, второй можно повторить). Счёт, записанный во время работы, вступает в силу на ближайшей границе периода (в снимке `PitRun.next`). Gate не останавливает и не перезапускает счёт; правила отказа по gate 2 введены в S4-29d (см. раздел ниже). У канала 0 нет входа gate.

**Управляющее слово `0x43`.** Биты 7:6 — канал (0 и 2; 1 и `11` read-back — `UnsupportedConfiguration`), 5:4 — доступ (01 LSB, 10 MSB, 11 LSB+MSB, 00 counter latch), 3:1 — режим (принимаются 0 и 2; 1, 3, 4, 5 и алиасы 6, 7 — `UnsupportedConfiguration`), бит 0 — BCD (`UnsupportedConfiguration`). Все отказы возвращаются до изменения снимка. Принятое слово сбрасывает канал: нет счёта, нет латча, триггеры чтения/записи в начале; другой канал не затрагивается. Запись данных или чтение счётчика до управляющего слова и до загрузки счёта — `UnsupportedConfiguration` (значение на микросхеме не определено).

**Counter latch (доступ 00, младший полубайт игнорируется).** Фиксирует текущее значение канала; чтение порта данных выдаёт латч по режиму доступа (LSB: один байт, MSB: старший байт, LSB+MSB: младший затем старший), после чего латч снимается — ровно одна выдача; следующие чтения возвращают живое значение. Второй латч до выдачи первого игнорируется. Латч без загруженного счёта — `UnsupportedConfiguration`. Живое чтение в LSB+MSB чередует байты (каждый — от текущего момента); управляющее слово сбрасывает триггер.

**Порт B `0x61`.** Бит 0 (gate 2) и бит 1 (speaker data) хранятся. Бит 5 — текущий OUT канала 2 (OUT канала 0 на него не влияет; не хранится, запись игнорируется). Бит 4 (refresh toggle) — решение модели: не хранится, а выводится из часов как `(T(now) / 18) & 1`, то есть переключается каждые 18 тактов (15,085 мкс), как выход канала 1, который аппаратура ставит в режим 2 со счётом 18; запись игнорируется. Чтение 0 константой отвергнуто: гость, который ждёт переключения бита 4, не завершил бы ожидание; при этом из-за ISSUE-067 ожидание завершится только если время продвигается при PIO-опросе (решение S4-29d). Биты 2, 3, 6, 7 (parity/IOCHK) читаются 0, запись в них и в биты 4/5 игнорируется (чтобы работала последовательность Linux «прочитать, изменить, записать обратно»). Gate не влияет на OUT2 и счёт; отказы по gate 2 и порт-тик — S4-29d.

**Reset** (`Pit8254Handle::reset`): снимок равен `PitSnapshot::default()` — счётчики не инициализированы, режим не задан, OUT обоих каналов низкий (до управляющего слова режим неизвестен; после него: режим 0 низкий, режим 2 высокий), gate 2 и speaker data равны 0.

**Не входит.** Линии IRQ и события `EventQueue` (S4-29f); канал 1 (его выход только эмулируется битом 4); BCD; read-back; режимы 1, 3, 4, 5; влияние gate на счёт и перезапуск (не моделируется и в S4-29d: только отказы); звук динамика (speaker); подключение к плате, manifest, handoff, MADT, FADT, firmware; гостевая калибровка и продвижение времени при PIO-опросе (сделано в S4-29d). Отчёт: [S4-29c](../reports/STAGE_4_S4-29C_2026-11-09.md).


**Отклонение от datasheet (одинаковое для всех режимов).** Модель видит счётный регистр в момент записи (e = 0 даёт N). Настоящий 8254 загружает счёт на следующем фронте CLK. Поэтому в режиме 0 OUT поднимается на N+1 такт позже у чипа, чем в модели. Ошибка — один такт (~84 ppm для калибровки на 11931), на Linux не влияет. Записано как отклонение, а не как выравнивание по сетке.

## Гостевая калибровка PIT и порт-тик (S4-29d)

Реализация: `crates/od-machine/src/pit_8254.rs` (правила gate 2, порт-тик: `PIT_PORT_B_READ_TICK_NS`, `Pit8254Handle::port_b_reads`, `take_port_tick_ns`), `crates/od-machine/src/stage4_pit_calibrate_probe.rs` (профиль `Stage4PitCalibrateProbeV1`, владелец часов), fixture `tests/fixtures/guest/stage4/pit_calibrate/`, builder `tools/build/build_stage4_pit_calibrate_probe.py`, runner `--stage4-pit-calibrate-probe IMAGE [--budget N] [--inject-no-port-tick]`. Решения консультации Opus (2026-11-09, п. 5 и 6) применены без пересмотра. Устройства по-прежнему не подключены к плате (`FirmwareBoardConfig`, `LoadedFirmwareBoard`, firmware, manifest, MADT, FADT не менялись).

**Порт-тик.** Каждое успешное чтение порта B `0x61` — чтение «временного порта»: устройство `PitPortB` считает его (`port_b_reads`) и ставит в очередь `PIT_PORT_B_READ_TICK_NS = 1000` нс виртуального времени (`take_port_tick_ns` забирает и обнуляет очередь). Значение, которое вернуло чтение, снято до тика. Устройство само часы не двигает: у `VirtualClock` один продвигающий — сессия vCPU (S4-22a/b, S4-28a2). Владелец часов забирает очередь после инструкции и продвигает часы на неё. Отказанное чтение (правила gate 2) тик не ставит. Записи в `0x61`, обращения к `0x40..0x43` и управляющие слова тик не ставят; чтение счётчика `0x42` и `PM_TMR` в этом срезе временными портами не объявлены (решение п. 5 перечисляет их, срез S4-29d по постановке — только `0x61`). Позже (S4-37a3) чтение `0x42` тоже ставит 1 мкс, но только если устройства построены `Pit8254Devices::new_with_counter_tick` (Linux-профиль платы); `Pit8254Devices::new` и профиль `Stage4PitCalibrateProbeV1` по-прежнему тикают только `0x61`.

**Что владелец применяет.** Профиль `Stage4PitCalibrateProbeV1` берёт очередь после каждой PIO-инструкции и вызывает `DiagnosticVcpuSession::advance_time_ns`. Время по числу выполненных инструкций в этом профиле не идёт (одна инструкция — 0 нс); единственный источник времени — порт-тик. Поэтому TSC (1 ГГц, один такт на наносекунду) и OUT2 двигаются только чтениями `0x61`. `LoadedFirmwareBoard::run_boot` применяет порт-тик PIT (`0x61` и `0x42`) под `linux_profile` с S4-37a3 (см. раздел «Linux-профиль платы»); `PagedVcpuSession` порт-тик не применяет.

**Ограничения (расхождение с решением п. 5 плана, по ревью Opus).** Тик на `0x42` и на `PM_TMR` в S4-29d не объявлен (на плате под `linux_profile` тик на `0x42` добавлен в S4-37a3, PM_TMR — в S4-37a2): решение п. 5 перечисляет их, срез по постановке — только `0x61`. Синхронизации часов по выполненным инструкциям нет: инструкция = 0 нс в этом профиле. Это записано как открытое; закрытие ISSUE-067 перенесено в S4-37a. Без `0x42` и `PM_TMR` Linux `quick_pit_calibrate` на этой модели время не продвигает.

**Gate 2 (решение п. 6): отказ без остановки счёта.** С `UnsupportedConfiguration` и без изменения состояния отвергаются: запись данных канала 2 (загрузка), пока gate 2 = 0; чтение счётчика канала 2, counter latch канала 2 и чтение порта B (бит 5 = OUT2) при загруженном канале 2 и gate 2 = 0; запись в порт B, опускающая gate 2, пока канал 2 загружен (в режиме 0 канал остаётся «загруженным» и после терминального счёта, до нового управляющего слова). Чтение порта B при незагруженном канале 2 допустимо при любом gate. Gate счёт не останавливает и не перезапускает. Состояние «загружен при gate 2 = 0» через порты недостижимо (загрузка требует gate 2 = 1, сброс отвергается); правило чтения проверяется тестом с принудительно выставленным состоянием.

**Профиль `Stage4PitCalibrateProbeV1`.** Плоский identity-образ Long64/CPL0: GPA `0x1000`, размер `0x4000`, RAM `0x7000`, GDT `0x2000`, код `0x4000`, стек `0x6ff0`; привязки PIO `0x40..0x43`, `0x61`, COM1 `0x3f8` (polling TX). RDTSC читает общий `VirtualClockHandle` (`RdtscControls { cr4_tsd: false }`, `tsc_hz = 1_000_000_000`). Гость повторяет `pit_calibrate_tsc` (gate high через `(inb(0x61) & ~2) | 1`, управляющее слово `0xb0`, счёт 11931, RDTSC, цикл по `0x61` до бита 5, `pitcnt >= 1000`, `tscmax <= 10 * tscmin`, частота = `delta / 10` кГц) и MSB-форму `quick_pit_calibrate` (счёт `0xffff`, на каждом проходе опрос `0x61`, латч канала 2 словом `0x80`, чтение LSB и MSB порта `0x42`; стадия 1 пока MSB = `0xff`, стадия 2 пока `0xfe`; порог: каждая стадия более 5 проходов; частота = `delta * 1193182 / 256000`). Результат — запись в COM1 (`A`, статус, 5 x u64; `B`, статус, 4 x u64; `Z`), затем HLT. HLT без пробуждения владелец воспринимает как завершение (`halted`): `run` диагностической сессии HLT вне halt wait не исполняет.

**Инъекции владельца.** `NoPortTick` — владелец отбрасывает очередь тика (поведение до S4-29d): время стоит, OUT2 не поднимается, гость крутится до исчерпания бюджета (`budget_exhausted`, код runner 5). `CoarsePortTick { ns_per_read }` — отрицательный контроль: 100 мкс на чтение; сработали собственные пороги гостя (`pitcnt < loopmin`, статус A = 1; MSB-стадии ≤ 5 проходов, статус B = 3). Вариант образа `--omit-gate` (гость не поднимает gate 2) отвергается моделью при загрузке канала 2.

**Ограничения.** Расхождение с буквой решения п. 5 (в этом профиле; на плате под `linux_profile` `0x42` тикает с S4-37a3): чтение `0x42` не тикает, поэтому MSB-форма использует `0x61` как драйвер времени, а `0x42` читается через latch (в Linux `pit_verify_msb` читает `0x42` дважды без латча). Порог `count > 5` и два этапа сохранены. PIT IRQ (INTIN2) — S4-29f; cmdline — S4-29g. Отчёт: [S4-29d](../reports/STAGE_4_S4-29D_2026-11-09.md).

## Вход IOAPIC INTIN2, только edge (S4-29e)

Реализация: `crates/od-machine/src/apic_pair.rs` (`Route::intin2_high`, `PairState::set_line2`, `PairState::set_low2`), `crates/od-machine/src/io_apic.rs` (`IoApicHandle::set_intin2`, `valid_low`). Зеркалит S4-26a для входа 2; INTIN4 не менялся (кроме одного симметричного отказа ниже). К плате, manifest, handoff, MADT/FADT и firmware не подключено; PIT к входу не привязан (S4-29f).

**Вход.** `set_intin2(level)` хранит уровень линии в `intin2_high`. Рост 0 -> 1 при `entry 2` без маски запрашивает вектор из `pins[2]` у Local APIC как fixed edge (тем же путём `PairState::edge`, что и INTIN4). Под маской фронт теряется, unmask событие не доставляет; high-to-high — не фронт. Если Local APIC отказал (выключен), фронт не потребляется и `intin2_high` не меняется. Вызывающий сериализует операции обоих контроллеров.

**Entry 2.** Low dword: вектор `0x10..=0xFE`, Fixed, physical destination, active-high, edge. Масочный entry может хранить любой вектор и (как остальные пины без INTIN4) active-low/LEVEL, но unmask с LEVEL, active-low, вектором `0x00..=0x0F`/`0xFF`, non-Fixed delivery или logical destination — `UnsupportedConfiguration` без изменения снимка. High dword (селектор `0x15`): по политике S4-26c принимается только 0 (physical APIC ID 0), читается 0; отдельного хранения нет. Remote IRR на entry 2 не существует: бит 14 читается 0, `route.remote_irr` принадлежит только INTIN4.

**Конфликт с INTIN4 (политика S4-26b).** Level-маршрут INTIN4 резервирует свой вектор против edge-запросов, в том числе под маской. Поэтому: (1) unmask entry 2 с вектором, равным вектору level INTIN4, — отказ; (2) запись INTIN4 с LEVEL (с маской или без) на вектор unmasked entry 2 — отказ (новый симметричный отказ в `PairState::set_low`, затрагивает только случай unmasked entry 2). Edge INTIN4 на тот же вектор допустим, запросы сливаются в IRR. Совпадение вектора с Timer LVT и PIC не проверяется.

**Reset.** `IoApicHandle::reset` и `reset_pair`: entry 2 masked, линия низкая, IRR не затрагивается IOAPIC-сбросом, Remote IRR 0.

**Не входит.** Level и Remote IRR для INTIN2, другие пины (кроме INTIN2 и INTIN4), PIT и IRQ0 (S4-29f), насос событий, EOI-регистр `0x40`, подключение к плате, MADT/FADT. Отчёт: [S4-29e](../reports/STAGE_4_S4-29E_2026-11-09.md).

## PIT канал 0 → INTIN2 (GSI 2) (S4-29f)

Реализация: `crates/od-machine/src/stage4_pit_irq0_probe.rs` (мост `PitIrq0Owner` — владелец-хук насоса `PumpHost`, профиль `Stage4PitIrq0ProbeV1`, загрузчик `load_stage4_pit_irq0_probe`, инъекции `Stage4PitIrq0Injection`), fixture `tests/fixtures/guest/stage4/pit_irq0/`, builder `tools/build/build_stage4_pit_irq0_probe.py`, runner `--stage4-pit-irq0-probe IMAGE [--budget N] [--inject-masked-entry2 | --inject-no-eoi | --inject-no-generation]`. Принятые примитивы (PIT S4-29c, PIC S4-29a, INTIN2 S4-29e, `InterruptPump` S4-27i) не менялись. К плате, manifest, handoff, MADT/FADT и firmware срез не подключён.

**Мост.** Очередь IRQ самого насоса (S4-27i3) хранит `(ключ, вектор LAPIC, срок)` и берёт due-событие через `request_fixed_edge` прямо на Local APIC; она обходит IOAPIC (entry 2, маску, вектор, выбранный гостем), поэтому OUT0 она нести не может. Мост — владелец-хук насоса (S4-27i5) со своим `EventQueue` с поколениями (тот же примитив и те же правила, что у очереди RX UART). Due-фронт OUT0 предъявляется IOAPIC импульсом на INTIN2: `set_intin2(true)`, затем `set_intin2(false)`; дальше действуют правила S4-29e (вектор берётся из `pins[2]`, фронт под маской теряется, unmask не доставляет). Событие снимается из очереди только после того, как импульс принят: отказ Local APIC (выключен) оставляет событие в очереди, насос завершается `Rejected(Host(..))`. OUT0 идёт только в INTIN2: пара 8259 не имеет входных линий (S4-29a), IR0 не получает ничего, вектор `0x30` ни разу не поднимается в IRR.

**События.** После каждой PIO-инструкции гостя и в начале каждого шага насоса мост сравнивает видимое гостю состояние канала 0 (режим и идущий счёт `PitRun`) с последним виденным. Любое изменение (управляющее слово, новый счёт, shutdown, сброс PIT) — reprogram: очередь начинает новое поколение (все ожидающие события старого поколения сняты, токен старого поколения устарел), и если у канала есть счёт, планируется следующий фронт. После доставки фронта следующий планируется, только если очередь пуста, так что в поколении живёт одна цепочка событий. Фронты OUT0 (формулы S4-29c, `T(t) = floor(t · 1 193 182 / 1e9)`): режим 2 с перезагрузкой `N`, загруженной в тик `T0`, — на тиках `T0 + k·N`, `k ≥ 1` (OUT на один тик низкий перед каждым; счёт, записанный во время работы, вступает в силу на границе периода, и граница сама — фронт); режим 0 — один фронт на тике `T0 + N`, дальше нет. Срок фронта на тике `E` — первая наносекунда `t` с `T(t) ≥ E`: `ceil(E · 1e9 / 1 193 182)`. Время инструкции 0 нс; часы двигает idle advance насоса до ближайшего срока моста (владелец-хук ждёт срока даже при IF=0). Shutdown — управляющее слово `0x30` (канал 0, режим 0, без счёта): `run = None`, события сняты. Сброс PIT (`Pit8254Handle::reset`, у пробы `reset_pit`) снимает события так же.

**Профиль образа `Stage4PitIrq0ProbeV1`.** Образ `0x4000` байт с GPA `0x1000`, RAM `0x7000` плюс MMIO IOAPIC и Local APIC, порты PIT `0x40..=0x43`, port B `0x61`, пара 8259; IDTR ставит загрузчик, в IDT один gate — вектор `0x60`. Гость: SVR `0x1FF`; мастер 8259 (ICW1..4, база `0x30`, IMR `0xFF`); entry 2 (`IOREGSEL 0x14`, `IOWIN` = `0x60`: edge, unmasked); PIT канал 0 дважды (`0x34` + счёт 1790, затем `0x34` + счёт 1193: второй reprogram заменяет первый); `STI; HLT; JMP`. Обработчик считает в `0x5000`, после третьего прерывания читает IRR мастера (OCW3 `0x0A`, `IN 0x20`) и пишет `0x30` в `0x43`, затем EOI и `IRETQ`. Ожидаемые значения builder выводит из формулы: N = 1193 — сроки 999 848, 1 999 695, 2 999 543 нс; три входа, счётчик 3, итоговое время 2 999 543 нс; сроки не кратны периоду (период 999 847,47 нс). Мост за прогон: 5 запланированных событий (первая программа, d1..d4), 3 доставлены, 5 смен поколения, 2 снятых события (первая программа и d4).

**Инъекции.** Загрузчик принимает `Stage4PitIrq0Injection`: `MaskedEntry2` (в образе у значения entry 2 выставляется бит маски 16: каждый фронт теряется, ни входа, ни счётчика; насос заканчивает бюджетом), `NoEoi` (EOI-запись обработчика уходит в TPR: вектор остаётся в ISR и блокирует второй вход), `NoGeneration` (мост не меняет поколение ни при reprogram, ни при shutdown, ни при сбросе: фронт замещённой программы на 1 500 191 нс доставляется, четвёртый вход после shutdown). Места правки образа (`0x4044` значение entry 2 `B8 60 00 00 00`, `0x40B1` EOI-запись `89 87 B0 00 00 00`) проверяются по байтам; несовпадение — `Layout`. Различимость по `(stop, wakeups, counter, time)`: принятый — `(idle_halt, 3, 3, 2 999 543)`; `MaskedEntry2` при бюджете 200 — `(budget, 0, 0, 156 976 053)`; `NoEoi` — `(budget, 1, 1, 143 978 036)`; `NoGeneration` — `(idle_halt, 4, 4, 2 999 543)` (вход 4 вместо 3).

**Модельные отказы и ограничения.** Канал 1, BCD, режимы 1/3..5, read-back и счёт 1 в режиме 2 отвергает ядро PIT (S4-29c); мост их не расширяет. Порт-тик порта B (`take_port_tick_ns`) мост не потребляет: образ порт `0x61` не читает. Очередь моста ёмкостью 16, в поколении одна цепочка; предел трассы насоса (512 событий) ограничивает прогон при бесконечной цепочке: инъекции `MaskedEntry2` и `NoEoi` при бюджете выше примерно 208 и 219 шагов заканчиваются `Rejected(TraceLimit)` (проверено запуском с бюджетами 300 и 4096), поэтому evidence снято при `--budget 200`. Flat-профиль (без paged); PIC IR0, ExtINT/LINT0, реальная калибровка TSC, cmdline (S4-29g) и подключение к плате не входят. Это не закрывает S4-29 целиком. Отчёт: [S4-29f](../reports/STAGE_4_S4-29F_2026-11-09.md).

## S4-27h: paged IRQ entry и IRETQ (разовый handler)

Реализация: `crates/od-isa/src/interrupt.rs` (`deliver_paged_maskable_interrupt`, общее приватное тело с flat-фронтом `deliver_maskable_interrupt`), `crates/od-isa/src/execute.rs` (`step_paged_mov_dword_store`, профиль `PagedMovDwordStoreLong64V1`), `crates/od-machine/src/paged_exception_delivery.rs` (`deliver_paged_external_interrupt`), `crates/od-machine/src/paged_memory_adapter.rs` (`step_paged_mov_dword_store`, `PagedDwordStoreStepReport`), `crates/od-machine/src/stage4_paged_irq_probe.rs` (профиль `Stage4PagedIrqProbeV1`, последовательность шагов, инъекции), fixture `tests/fixtures/guest/stage4/paged_irq/`, builder `tools/build/build_stage4_paged_irq_probe.py`, runner `--stage4-paged-irq-probe IMAGE [--budget N] [--inject-idt-gate-not-present | --inject-stack-readonly | --inject-handler-without-eoi]`. Решения консультации Opus (S4-27h, 2026-11-09) применены; отступления перечислены ниже. К плате (`FirmwareBoardConfig`, `LoadedFirmwareBoard`), firmware, manifest, MADT/FADT, IOAPIC, PIT и UART срез не подключён.

**Доставка внешнего вектора на paged-профиле.** До среза внешний вектор принимали только flat-пути (`DiagnosticVcpuSession::deliver_interrupt_and_step`, `run_apic_quantum`, `deliver_pending_irq` через `IdentityMemoryAdapter`): `deliver_maskable_interrupt` отвергал любое состояние с `cr0`, `cr3` или `cr4` как `OutsideProfile(UnsupportedCr0State/UnsupportedCr3State)`, а paged delivery (`deliver_paged_page_fault`) умел только вектор 14/8 с шестью qword кадра. `deliver_paged_maskable_interrupt(state, memory, vector, context, rflags_defined, efer)` — второй фронт того же тела: предусловия как у `step_paged_iretq` (CR0 и CR4 равны BASE, `cr3 < 2^36`, EFER = `0xD00`), иначе `OutsideProfile` без обращения к памяти. Остальная семантика общая: IF=1 и отсутствие STI shadow, CPL0 Long64, CS из кеша, gate типа `0xE` с P=1 и IST=0, дескриптор целевого CS (L=1, D=0, A=1 уже выставлен, база 0, лимит `0xFFFFFFFF` не проверяется для external), кадр из пяти qword (SS, RSP, RFLAGS, CS, RIP, без error code) под `aligned_rsp = RSP & ~15`, новый RSP = `aligned_rsp - 40`, сброс IF/TF/NT/RF/VM в живом RFLAGS, сохранённый RFLAGS сохраняет IF. **Расхождение с flat-фронтом (записано по решению консультации):** запись кадра в paged-фронте идёт как `AccessOrigin::ImplicitStack`, во flat — `Explicit`; flat-поведение и его тесты не менялись.

**Обёртка в машине.** `deliver_paged_external_interrupt(state, bus, cr3, efer, context, rflags_defined, vector, committed_write)` строит `PagedExceptionMemory` (RAM-only tracked walker, `descriptor_accessed_target: None`, без цепочки #DF) и возвращает `delivery`, `secondary_page_fault`, `access_error`, `ad_updates`, `committed_writes`. Вызывающий передаёт копию CPU-состояния (candidate) и публикует её только после подтверждения вектора: `LocalApicHandle::acknowledge_vector(vector, || state = candidate)` переносит IRR в ISR и публикует candidate в одном критическом участке. Запись A-бита дескриптора CS в этой модели не выполняется: бит должен быть выставлен.

**EOI-профиль `89 03`.** `step_paged_mov_dword_store(PagedMovDwordStoreLong64V1, ...)` исполняет ровно `MOV [RBX],EAX`: те же предусловия CR0/CR4/EFER/`cr3`, что у доставки; CPL0 Long64, активность Running, STI shadow Clear; оба байта кода в одной 4 KiB странице (`RIP & 0xFFF <= 0xFFE`); не-канонический RBX — #GP(0); dword не пересекает границу 4 KiB (иначе `Unsupported{0x89}`, раздельного обращения нет); порядок: fetch, затем запись операнда. Память — `PagedMovMemoryAdapter` с MMIO-диспатчем (`access_tracked`), единственная допустимая запись — один dword; qword-профили не наследуются. Флаги и RAX не меняются, RIP += 2. Тип памяти страницы APIC (PCD/PWT/PAT) **не моделируется**. Бит D PTE страницы APIC выставляется до диспатча MMIO-записи (политика `set_mmio_dirty_before_dispatch` шины). **Отступление от решения консультации:** профиль реализован как автономный точный шаг по образцу `step_paged_lidt`, а не как ещё одна запись в таблицы `Cr0Profile`/`Cr4Profile` интерпретатора, как `PagedMovCrossPageDwordReadLong64V1`: исполнитель не расширялся, семантика и память (адаптер с MMIO) те же.

**Профиль образа `Stage4PagedIrqProbeV1`.** Образ `0xA000` байт с GPA `0x1000`, RAM `0x20000` плюс MMIO Local APIC `0xFEE00000..0xFEE00FFF`; пять страниц таблиц (PML4 `0x1000`, PDPT `0x2000`, PD `0x3000`, таблица кода `0x4000` для 4 MiB, системная таблица `0x5000` для 6 MiB), только страницы 4 KiB. Отображения: main `0x401000` -> `0x6000`; handler `0x402000` -> `0x7000`; IDT `0x600000` -> `0x8000`; GDT `0x601000` -> `0x9000`; страница Local APIC `0x602000` -> `0xFEE00000`; стек `0x700000` -> `0xA000`. Листовые PTE: present, writable, A и D clear (`0x6003`, `0x7003`, `0x8003`, `0x9003`, `0xFEE00003`, `0xA003`); промежуточные уровни `0x...23` (A выставлен). IDT: вектор `0x41`, interrupt gate `0x8E`, CS `0x08`, IST 0, смещение `0x402000`; IDTR limit `0x41F`. GDT: null, CS `0x08` (`0x00AF9B000000FFFF`), SS `0x10`. Main — `INVLPG [RBX]` дважды (`0F 01 3B`): уже принятый точный paged-шаг без обращения к данным; MOV-профили не годятся, потому что RBX занят адресом EOI (`0x6020B0`) для handler. Handler — `89 03` (EOI, значение EAX = 0) и `48 CF`. Handoff загрузчика: CR0 `0x80010033`, CR3 `0x1000`, CR4 `0x20`, EFER `0xD00`, RFLAGS `0x202` (IF=1 из начального состояния, не STI), RAX 0, RBX `0x6020B0`, RSP `0x700FF8` (не кратен 16: выравнивание наблюдаемо), GDTR/IDTR, CS/SS как выше; загрузчик же включает software enable Local APIC (запись `0x1FF` в SVR) — без неё `eligible()` пуст. Образ принимается только в точности этих значений (`PageTableEntry`/`CodeBytes`).

**Последовательность шагов (успех).** (1) `Main`: `INVLPG`, RIP `0x401003`; (2) `Delivery`: `request_fixed_edge(0x41)`, `eligible() == Some(0x41)`, доставка на candidate, затем подтверждение IRR -> ISR вместе с публикацией; (3) `Eoi`: `89 03` по `0x6020B0` -> физический `0xFEE000B0`, ISR очищается (проверка IRR = ISR = 0 до IRETQ); (4) `Iretq`: `step_paged_iretq`, возврат в `0x401003`, RSP `0x700FF8`, RFLAGS `0x202`; (5) `Resume`: второй `INVLPG`, RIP `0x401006`. Итог: 5 шагов, 4 инструкции (доставка не инструкция).

**Ожидания (Intel SDM Vol. 3A 6.14.2 и 4.8, Vol. 3A 13.8–13.9; не вывод кода).** Новый RSP `0x700FC8` (`0x700FF0 - 40`); кадр снизу вверх: RIP `0x401003`, CS `0x08`, RFLAGS `0x202`, RSP `0x700FF8`, SS `0x10`; живой RFLAGS в handler `0x002` (IF сброшен); A выставлен на PTE main, handler, IDT, GDT; на PTE стека и страницы APIC — A и D (`0xA063`, `0xFEE00063`); CR2 не меняется.

**Инъекции (модель; различимы по stop, steps, retired, IRR/ISR, RSP, RIP).** `IdtGateNotPresent` (P=0 при типе `0xE`): `OutsideProfile(InvalidGate)` до commit, CPU не меняется (RIP `0x401003`, RSP `0x700FF8`), IRR сохранён, ISR пуст, handler не выполняется; A на PTE IDT выставлен чтением gate, GDT/стек/handler не тронуты; 2 шага, 1 инструкция. `StackReadOnly` (PTE стека `0xA001`, CR0.WP=1): `InterruptDeliveryAborted{StackWrite{OldSs}, Architectural #PF}` на первой записи кадра (линейный `0x700FE8`, код ошибки P|W = 3), terminal, вектор не подтверждён (IRR сохранён, ISR пуст), CR2 и CPU не меняются; A выставлен на PTE стека, D нет (`0xA021`); 2 шага, 1 инструкция. `HandlerWithoutEoi` (handler — только `48 CF`): ISR остаётся, после IRETQ и второго main повторный запрос того же вектора даёт IRR=1, ISR=1, `eligible() == None` (приоритетный класс `0x40` в обслуживании), `SecondDeliveryBlocked`; 5 шагов, 3 инструкции.

**Модельные расхождения с железом.** Отказы доставки терминальны: аппаратный исход — последовательная доставка #PF (стек) или #NP (gate P=0) по SDM Vol. 3A 6.15 и 6.7; вложенная доставка, #DF и shutdown — S4-33. Подтверждение IRR -> ISR «после входа» в модели расходится с INTA на железе (вектор выбирает APIC до записи кадра); решить в S4-33. Для S4-34 (KVM) инъекция read-only стека на железе даст shutdown/reset: результат model-only, не сравним.

**Снятие оговорки в S4-27.** Оговорка «только плоская identity» снята для одного профиля: одиночный внешний APIC-вектор, Long64/CPL0, 4 KiB (S4-32), CR0/CR4=BASE, EFER=0xD00, gate `0xE` с IST=0 без смены CPL, handler `89 03` + `48 CF` (S4-27h). STI/HLT/`run_apic_quantum`/JIT остаются flat-only до S4-27i; вложенные события при доставке — S4-33. Статус S4-27 в плане не менялся. Дальнейшее закрытие: paged STI/HLT и paged wake — S4-27i4a; точное состояние оговорок S4-27 после S4-27i5 — в разделе S4-27i5 ниже.

**Не входит.** Вложенные #PF/#NP/#DF и shutdown; IST != 0; CPL3 (TSS RSP0); запись A-бита дескриптора; paged STI и HLT; event pump (S4-27i); `run_apic_quantum` на paged; JIT; несколько векторов; 2 MiB и TLB; SMAP/SMEP; x2APIC; тип памяти (PCD/PWT/PAT); MOV SS и NMI; линия IOAPIC/UART/PIT. Риск: Linux гасит EOI `movl` с disp32 при CR0 `0x80050033`, шире BASE; exact-профили не масштабируются, нужен S4-27i. Отчёт: [S4-27h](../reports/STAGE_4_S4-27H_2026-11-09.md).

## S4-27i1: flat event pump (граница инструкции, STI/HLT/wake, IRQ от caller)

Реализация: `crates/od-machine/src/interrupt_pump.rs` (`InterruptPump`, `PumpResult`, `PumpExit`, `PumpRejection`, `PumpTraceEvent`, `PumpInjection`), `crates/od-machine/src/stage4_interrupt_pump_probe.rs` (профиль `Stage4InterruptPumpProbeV1`, строгий загрузчик образа), добавочные crate-private методы `peek_ram_qword` у `DiagnosticVcpuSession` и `DiagnosticHaltWait` (чтение RAM без побочных эффектов для evidence; принятые API не менялись), fixture `tests/fixtures/guest/stage4/interrupt_pump/` (`entry.S`, `layout.ld`), builder `tools/build/build_stage4_interrupt_pump_probe.py`, runner `--stage4-interrupt-pump-probe` (`tools/runner/src/stage4_interrupt_pump_probe.rs`).

**Владелец и состав.** `InterruptPump` владеет `Cpu { Running(DiagnosticVcpuSession) | Waiting(DiagnosticHaltWait) }` и `LocalApicHandle`. Это синхронный цикл вызывающего потока без потока, часов, таймера и очереди событий: запросы приходят от caller (`pump.apic().request_fixed_edge(v)`) или от гостевого MMIO (self-IPI через ICR). Сессия остаётся набором caller-driven шагов, `run_boot`/paged-профиль/JIT pump не затрагивает.

**Порядок границы (один раз на границу инструкции перед fetch).** `run(budget, stop) -> PumpResult { exit, quanta, retired }`: (1) STOP; (2) бюджет шагов; (3) для Running — проверка APIC (IF=1, STI shadow Clear, завершённая граница, `eligible()`), при успехе доставка через `run_apic_quantum` (кадр + первая инструкция handler); (4) иначе одна инструкция: HLT -> `into_halt_wait` (в `run(1)` обычный интерпретатор HLT не исполняет — terminal), STI и CLI -> `run_sti_apic_quantum` (CLI при IF=1 вне STI-shadow не поддержан: принятый путь `StartIfSet` в `od-isa/execute.rs` допускает только `sti`, и CLI при IF=1 и shadow Clear даёт terminal `Unsupported`; pump на обычном `cli` с включёнными прерываниями завершается `Terminal`), IRETQ -> `run_same_cpl0_iretq`, остальное -> `run_apic_quantum`; для Waiting — `poll_apic`: `Entered` -> `into_running`, `Waiting` -> выход `IdleHalt`. Эффекты шага видит проверка следующей границы; отдельной проверки после инструкции нет. Выходы: `IdleHalt` (не terminal; после запроса caller повторный `run` продолжает), `Budget` (возобновляем), `Stopped`, `Terminal(DiagnosticTerminalExit)`, `Rejected(PumpRejection)` (отказ принятого примитива без изменения состояния этим шагом). `Stopped`, `Terminal` и `Rejected` финальны (повторный `run` возвращает тот же выход). Бюджет считает шаги pump (инструкция, доставка, вход в HLT, wake), `retired` — исполненные инструкции (доставка не инструкция).

**Трасса.** `PumpTraceKind`: `Irr`, `Entry { vector, source: Boundary | HaltWake }`, `Eoi`, `Sti`, `Cli`, `Hlt`, `Iretq`. `Irr` и `Eoi` наблюдаются на границе по смене IRR/ISR наблюдаемых векторов (`with_watched_vectors`), поэтому запрос caller виден на следующей границе. Поле `rip` события `Entry` различается по источнику (записанное расхождение): у `Entry{Boundary}` оно равно адресу handler плюс длина первой инструкции handler (она исполняется в том же кванте внутри `run_apic_quantum`); у `Entry{HaltWake}` оно равно адресу handler (первая инструкция handler при wake из HLT в этом кванте не исполняется). Предел `INTERRUPT_PUMP_TRACE_LIMIT` = 512 (иначе `Rejected(TraceLimit)`).

**Режим трассы (S4-37a5, ISSUE-072).** `PumpTraceMode`: `Bounded` (по умолчанию; первые `INTERRUPT_PUMP_TRACE_LIMIT` = 512 событий, следующее завершает прогон `Rejected(TraceLimit)`, поведение S4-27i1..i5 не меняется) и `Ring(n)` (последние `n >= 1` событий; `Ring(0)` отвергается `PumpTraceModeError::ZeroCapacity` без изменения режима, к `Ring(1)` не приводится; при переполнении старейшее выбрасывается, `trace_dropped()` растёт, `TraceLimit` не возникает). Режим выбирается `set_trace_mode` только до первого события (иначе `PumpTraceModeError`). Каждому событию присваивается сквозной `seq` (`trace_next_seq()` = число всех записанных событий): он монотонен, не сбрасывается при выбрасывании и создаётся заново только вместе с pump (reset трассы нет; `reset_irq_queue` трассу и `seq` не трогает), поэтому `trace_dropped() == trace_next_seq() - trace().len()`, а сохранённые события идут по `seq` без пропусков с `trace_first_seq()`. `trace()` всегда возвращает смежный срез в порядке `seq`; `trace_since(seq)` отдаёт сохранённые события с `seq` не ниже заданного. Решения pump трассу не читают. Потребитель-курсор хранит `seq` следующего необработанного события (`Stage4UartIrqProbe`: `next_pump_seq`) и считает потерянные в Ring события (`lost_pump_events`), если pump выбросил их раньше синхронизации; индексный курсор в Ring недопустим: пробы, которые читают трассу индексом (`pump.trace()[seen..]`), работают только в `Bounded`, и Ring им включать нельзя до перевода на `trace_since` (S4-38). Инъекции `RingKeepsLimit` (лимит остался в Ring: `TraceLimit`) и `RingResetsSeq` (`seq` сброшен при выбрасывании: разрыв непрерывности) различимы тестами `pit_irq0_pump`.

**Профиль образа `Stage4InterruptPumpProbeV1`.** Образ `0x4000` байт с GPA `0x1000`: GDT `0x2000` (три дескриптора), IDT `0x3000` (ровно два gate: `0x41` и `0x51`, `0x8E`, селектор 8, IST=0), код `0x4000`; RAM `0x7000` плюс MMIO Local APIC; IDTR ставит загрузчик (pump не исполняет LIDT); начальное состояние: RIP `0x4000`, RFLAGS `0x2`, RSP `0x6FF0`, STI shadow Clear. Вектор `0x41` (класс 4) — handler B (EOI, IRETQ); вектор `0x51` (класс 5) — handler A (self-IPI `0x41` через ICR `0x44041`, EOI, IRETQ). Прологом гость включает Local APIC (SVR = `0x1FF`, три инструкции). Сценарии (один на образ): `sti_hlt` (`STI; HLT; HLT`), `if1_hlt` (`STI; NOP; HLT`), `cli_hlt` (`CLI; HLT`; исполняется при исходном IF=0, поэтому CLI там ничего не меняет: сценарий доказывает только «HLT при IF=0 -> IdleHalt», а не поведение CLI при IF=1, которое даёт terminal `Unsupported`), `rep` (`STI; NOP; MOV ECX,2000; MOV EDI,0x5000; REP STOSB; HLT`). Случай «self-IPI B во время обработчика A» — образ `sti_hlt` с запросом caller вектора `0x51`. Сам по себе IF=0 внутри handler A не пускает B; порядок, показанный трассой с EOI, сам по себе приоритет не доказывает; приоритет на уровне pump доказывает вариант без EOI (после IRETQ IF=1, а B всё равно не принят), на уровне APIC — `eligible() == None`.

**Ожидания (Intel SDM Vol. 3A 6.14.2–6.14.3, 10.8.3.1; Vol. 2 STI, CLI, HLT; не вывод кода).** Кадр внешнего прерывания: SS, RSP, RFLAGS, CS, RIP (пять qword, без кода ошибки); стек выровнен вниз по 16 (`0x6FF0` уже выровнен), новый RSP `0x6FC8` (`0x6FF0 - 40`); кадр снизу вверх: RIP, CS `0x08`, RFLAGS `0x202`, RSP `0x6FF0`, SS `0x10`; RFLAGS в handler `0x002` (шлюз прерывания сбрасывает IF). STI откладывает приём на одну инструкцию: после STI граница перед HLT приёма не допускает, HLT исполняется, ожидание начинается с RIP после HLT; прерывание, разбудившее HLT, кладёт в кадр RIP инструкции после HLT. При IF=1 без shadow и ожидающем векторе приём происходит на границе перед HLT, в кадре RIP указывает на HLT; после IRETQ HLT исполняется. При IF=0 HLT выводит только NMI/SMI/INIT/RESET: ожидающий маскируемый вектор остаётся в IRR, выход `IdleHalt`. Приём вектора возможен, только если его класс приоритета выше обслуживаемого (TPR=0), пока не записан EOI: B (класс 4) при A (класс 5) в обслуживании не eligible (`eligible() == None`) даже после IRETQ без EOI; после EOI A и IRETQ (IF=1) B принимается на границе (RIP кадра B — инструкция, которую прервал A).

**Evidence (механизм, не вывод кода).** Для каждого сценария manifest builder содержит ожидаемые события, кадр (адрес, RIP, CS, RFLAGS, RSP, SS), RFLAGS handler и финальные RIP/RSP/RFLAGS/IRR/ISR; machine-тесты кодируют те же значения независимо, runner-тест сравнивает вывод runner с manifest.

**Инъекции (различимы по `exit`, трассе, IRR/ISR, RIP).** `PumpInjection::SkipShadowCheck`: проверка границы не учитывает STI shadow, pump пытается принять вектор после STI; принятый примитив отказывает (`ProfileRejected`), результат `Rejected(ApicQuantumProfile)` на RIP HLT, IRR вектора сохранён, ISR пуст, вход не произошёл, вход в wait не достигнут. `PumpInjection::HltAsOrdinary`: HLT через `run(1)` — terminal (`Unsupported`), а не wait, вход не происходит. Handler без EOI (вариант образа `--omit-eoi`): ISR остаётся установленным, `eligible() == None`, второй вектор не доставляется (A без EOI: B ждёт в IRR; B без EOI: повторный запрос того же вектора не принимается).

**Отклонения и ограничения.** (1) **Частичный REP:** после `WorkLimit(RepIterationBudget)` (порог 1024 итерации) граница не завершена; pump довершает инструкцию (`run(1)`) без проверки APIC, поэтому прерывание принимается только после REP (кадр: RIP следующей инструкции), тогда как железо может принять его между итерациями с RIP на REP и частичным RCX. Задержка записана как отклонение; проверена тестом (`rep`). (2) **Первая инструкция handler** исполняется внутри `run_apic_quantum` (`run(1)`): она не может быть STI, CLI, HLT или IRETQ (будет terminal `Unsupported`); при wake из HLT доставка не исполняет инструкцию handler (разные RIP сразу после входа: `handler` и `handler+len`). (3) Профиль STI принятого S4-27c допускает в shadow только NOP, STI, CLI; прочие инструкции в shadow дают `Unsupported`; CLI при IF=1 вне shadow не поддержан (`StartIfSet` допускает только `sti`): CLI при IF=1 и shadow Clear даёт terminal `Unsupported`, pump на обычном `cli` с включёнными прерываниями завершается `Terminal`. (4) Инъекция `SkipShadowCheck` демонстрируется отказом принятого примитива (защита в глубину), а не неверным кадром: принять вектор в shadow без изменения принятых API нельзя. (5) Обычный `run(1)` на HLT даёт `Unsupported`, а не `Halted`; `Halted` возвращается только для запуска уже остановленного CPU, что через API pump недостижимо. (6) Модель однопоточная; caller сериализует всё, как у принятых примитивов.

**DRY-долг.** `Stage4UartIrqProbe` (S4-24e) содержит собственный цикл `run/step_waiting/step_running` с теми же шагами; перевод на pump — подсрез S4-27i5 (его тесты — регрессия). До этого логика цикла существует в двух местах. **Закрыт в S4-27i5:** `Stage4UartIrqProbe` работает поверх `InterruptPump`, см. раздел S4-27i5.

**Не входит.** Часы, таймер, очередь IRQ-событий (S4-27i2, i3), paged и pump поверх paged (i4a, i4b), JIT внутри pump, `run_boot` и плата, Linux, PIT/PIC/INTIN2, порт-тик, MOV SS, NMI, CPL3, IST != 0, TLB, 2 MiB, x2APIC, несколько vCPU. Статус S4-27 и S4-27i в плане не менялся. Отчёт: [S4-27i1](../reports/STAGE_4_S4-27I1_2026-11-09.md).

## S4-27i2: owner-driven таймер в pump (dispatch, idle advance, ослабление gate S4-27c по clock)

Реализация: `crates/od-machine/src/interrupt_pump.rs` (владение таймером, `PumpInjection::NoTimerDispatch`, `PumpRejection::{TimerDispatch, TimerAdvance, TimerNoProgress}`, `InterruptPump::virtual_time_ns`), `crates/od-machine/src/local_apic.rs` (`LocalApicHandle::next_timer_deadline`, `LocalApicHandle::has_timer`), `crates/od-machine/src/diagnostic_vcpu.rs` (gate `run_sti_apic_quantum`, `DiagnosticHaltWait::virtual_clock`), `crates/od-machine/src/stage4_interrupt_timer_probe.rs` (профиль `Stage4InterruptTimerProbeV1`, строгий загрузчик), fixture `tests/fixtures/guest/stage4/interrupt_timer/` (`entry.S`, `layout.ld`), builder `tools/build/build_stage4_interrupt_timer_probe.py`, runner `--stage4-interrupt-timer-probe` (`tools/runner/src/stage4_interrupt_timer_probe.rs`).

**Владение таймером.** Если у `LocalApicHandle` есть таймер (`has_timer()`, shared clock S4-28a2), pump сам вызывает `dispatch_timer` на каждой границе: порядок границы — STOP, бюджет шагов, dispatch due-события (Running и Waiting), затем проверка APIC и одна инструкция либо `poll_apic`. Без таймера (S4-27i1) поведение не меняется. Dispatch на границе обслуживает только уже наступивший срок: время по инструкциям равно 0 нс (как в S4-29d; синхронизация по инструкциям — S4-37a), поэтому часы в Running не движутся.

**Idle advance.** В Waiting, если `poll_apic` не нашёл eligible-вектор, pump берёт `LocalApicHandle::next_timer_deadline()`, продвигает часы сессии (`DiagnosticHaltWait::advance_time_ns`, единственный владелец advancer) до этого срока, вызывает `dispatch_timer` и возвращается на границу (следующий шаг делает wake через `poll_apic`). Нет срока — `IdleHalt` без продвижения времени. Срок очереди IRQ-событий отсутствует (очередь — i3). Дополнение к плану (записано как уточнение): при IF=0 маскируемый вектор HLT не будит, поэтому pump не продвигает время и даёт `IdleHalt`, даже если таймер взведён (иначе периодический таймер сжигал бы бюджет, пока вектор копится в IRR); проверено тестом `halted_with_if_clear_is_idle_even_with_an_armed_timer`. Если срок не в будущем после dispatch, результат `Rejected(TimerNoProgress)` (защита от цикла без прогресса); ошибки dispatch и advance — `Rejected(TimerDispatch)` и `Rejected(TimerAdvance)`.

**`LocalApicHandle::next_timer_deadline() -> Option<u64>`.** Read-only: срок (нс виртуального времени) ближайшего срабатывания, которое может поднять прерывание. `None`, если таймера нет, событие не взведено (не запущен, отменён нулевым Initial Count, one-shot уже сработал) или LVT Timer замаскирован (маскированное срабатывание только перевзводит событие и IRQ не поднимает). Метод не потребляет событие, не читает и не продвигает часы; существующие методы не менялись. `has_timer()` — read-only признак наличия таймера (добавлен, чтобы pump без таймера не получал `UnsupportedConfiguration` от `dispatch_timer`).

**Ослабление gate S4-27c по clock (S4-27i2).** Принятый контракт S4-27c отвергал `run_sti_apic_quantum` (`ProfileRejected`), если у сессии настроены clock или RDTSC. Это меняется: наличие clock (и привязанных к нему `RdtscControls`) больше не отвергается, потому что pump с таймером обязан вести STI и CLI через этот квант на сессии с общим clock. RDTSC внутри STI-кванта по-прежнему запрещён, но запрещает его сам профиль, а не байтовая проверка: `run_with_sti_profile` (`crates/od-isa/src/execute.rs`, около строки 15506) передаёт `None` вместо RDTSC controls и source, язык профиля — только CLI/STI/NOP/HLT (ISSUE-069). Байтовая проверка (`next_is_rdtsc`, `crates/od-machine/src/diagnostic_vcpu.rs`, около строк 866–878) лишь переводит одну форму (`0F 31` с опциональным одним REX-префиксом `40`–`4F`) в отказ `ProfileRejected` до любого изменения состояния, в том числе при RDTSC первой инструкцией в STI shadow. Остальные формы дают terminal `Unsupported`, TSC при этом не читается: префиксы `66`/`F2`/`F3`/`2E` и два REX подряд; RDTSCP (`0F 01 F9`); RDPMC (`0F 33`); первая инструкция handler после доставки внутри `run_sti_apic_quantum`. Избыточный отказ: при IF=1, shadow Clear, eligible-векторе и RDTSC на RIP квант отвергается `ProfileRejected`, хотя мог бы исполнить инструкцию handler; именно это проверяет случай 2 теста `sti_apic_rejects_invalid_admission_without_mutation` (`irq_session`: RIP `0x4100`, IF=1, после intin4 вектор `0x51` eligible). Остальные условия gate S4-27c не менялись. Реализация: вместо проверок `clock.is_some()` и `rdtsc_controls.is_some()` — проверка байтов по RIP (`peek_code`, только RAM, без побочных эффектов). Отклонение от буквы задания: «ослабить только clock, RDTSC оставить» нельзя выполнить как отдельную проверку поля `rdtsc_controls`, потому что конструктор `new_with_clock_handle` всегда задаёт и clock, и `RdtscControls`; поле дало бы отказ для любой сессии с clock и сломало бы STI-путь pump, поэтому проверяются байты по RIP. Вариант через `Option<RdtscControls>` отложен (см. ISSUE-069, S4-37a). Сессия без clock и RDTSC ведёт себя как раньше (RDTSC в STI-профиле — terminal `Unsupported`). Тест принятого S4-27c, который проверял отказ по clock (`sti_apic_rejects_invalid_admission_without_mutation`, случай 2), не удалён: он переписан на отказ RDTSC на сессии с clock (комментарий «S4-27i2: ослаблен clock gate»), а принятие clock доказывают `sti_quantum_accepts_a_session_clock` и успешный периодический сценарий.

**Профиль образа `Stage4InterruptTimerProbeV1`.** Образ `0x4000` байт с GPA `0x1000`: GDT `0x2000`, IDT `0x3000` (ровно один gate: вектор `0x61`, класс 6, `0x8E`, селектор 8, IST=0), код `0x4000`; счётчик — dword по GPA `0x5000` (RAM вне образа); RAM `0x7000` плюс MMIO Local APIC; Local APIC с shared timer: частота таймера 100 МГц, TSC 1 ГГц (1 тик = 1 нс), clock принадлежит сессии. Начальное состояние: RIP `0x4000`, RFLAGS `0x2`, RSP `0x6FF0`, shadow Clear, IDTR ставит загрузчик. Гость: SVR = `0x1FF`; Divide Configuration = `0x3`; LVT Timer = `0x20061` (periodic, не маскирован, вектор `0x61`); Initial Count = 625 (взводит таймер); затем `STI; loop: HLT; JMP loop`. Handler: увеличивает счётчик, при достижении `PROBE_WAKEUPS` (по умолчанию 5) отменяет таймер записью Initial Count = 0, пишет EOI, `IRETQ`. Вариант `unarmed` не программирует таймер.

**Ожидания (Intel SDM Vol. 3A 10.5.4, 6.14.2–6.14.3, 10.8.3.1; формула таймера; не вывод кода).** Делитель выбирают биты 3, 1, 0 Divide Configuration (`0x0` -> 2, `0x1` -> 4, `0x2` -> 8, `0x3` -> 16, `0x8` -> 32, `0x9` -> 64, `0xA` -> 128, `0xB` -> 1); счёт идёт на частоте входа таймера, делённой на делитель; периодический таймер перезагружает Initial Count при каждом срабатывании. Период P = Initial Count × делитель / частота = 625 × 16 / 100 МГц = 100 мкс = 100000 нс (в коде таймера `Timer::arm_at`/`next_periodic_deadline` то же правило с округлением вверх до нс; тесты S4-28a4/S4-28b принимают его). Гость взводит таймер при времени 0 (инструкции 0 нс), k-е срабатывание в k × P, поэтому после N = 5 пробуждений время 500000 нс, счётчик 5, пять входов `Entry{HaltWake}` с RIP handler, кадр: RIP после HLT, CS `0x8`, RFLAGS `0x202`, RSP `0x6FF0`, SS `0x10` (по адресу `0x6FC8`), RFLAGS в handler `0x2`; после последнего EOI IRR и ISR вектора пусты, `next_timer_deadline() == None`, CPU в HLT, `IdleHalt`. Трасса: `sti`, `hlt`, затем пять раз `irr`, `entry`, `eoi`, `iretq`, `hlt`.

**Evidence.** Manifest builder содержит параметры таймера и формулу, период, времена пробуждений, время, счётчик, ожидаемую трассу, кадр и финальное состояние; machine-тесты кодируют значения независимо и сэмплируют время на каждом входе; runner-тест сравнивает вывод runner с manifest.

**Инъекция (различима по (stop, wakeups, counter, time)).** `PumpInjection::NoTimerDispatch`: pump не владеет таймером (ни dispatch, ни idle advance). Принятый запуск: `(IdleHalt, 5, 5, 500000)`; инъекция: `(IdleHalt, 0, 0, 0)`, таймер остаётся взведённым (`next_timer_deadline() == Some(100000)`), IRR пуст. Варианты без срока (`unarmed`; LVT замаскирован при взведённом счётчике) также дают `(IdleHalt, 0, 0, 0)`, но `next_timer_deadline()` показывает причину: `None`.

**Отклонения и ограничения.** (1) Gate S4-27c ослаблен по clock; единственная форма RDTSC (`0F 31` с опциональным REX) отвергается проверкой байтов по RIP, остальные формы дают terminal `Unsupported`, отказ шире реального поведения (см. выше). (2) Уточнение: при IF=0 в Waiting время не продвигается. (3) Время по инструкциям 0 нс: TSC и Current Count не меняются при исполнении кода, время идёт только idle advance (синхронизация по инструкциям — S4-37a). (4) Первая инструкция handler исполняется внутри `run_apic_quantum`, а handler обязан состоять из инструкций обычного интерпретатора (ограничение S4-27i1 сохраняется). (5) Периодический таймер при занятом приоритетом векторе (ISR не очищен) сжигает бюджет idle advance до EOI: выход по бюджету, а не `IdleHalt`. Исправлено кодом S4-27i3 (см. раздел S4-27i3, ISSUE-070). (6) Однопоточная модель; caller сериализует всё, как у принятых примитивов.

**Не входит.** Очередь IRQ-событий (S4-27i3), paged-таймер и pump поверх paged (i4), `run_boot` и плата (S4-37a), время по инструкциям (S4-37a), TSC-deadline, x2APIC, несколько таймеров, JIT, Linux. Статус S4-27 и S4-27i в плане не менялся. Отчёт: [S4-27i2](../reports/STAGE_4_S4-27I2_2026-11-09.md).

## S4-27i3: очередь IRQ-событий в pump и арбитраж простоя (ISSUE-070)

Реализация: `crates/od-machine/src/interrupt_pump.rs` (`InterruptPump::{irq_generation, schedule_irq, cancel_irq, reset_irq_queue, irq_queue_len, irq_queue_next_deadline, coalesced_irqs}`, `PumpQueueError`, `PumpInjection::PopBeforeRequest`, `PumpRejection::{QueueRequest, QueueState, QueueRoute}`, `PumpTraceKind::Coalesced`, `INTERRUPT_PUMP_QUEUE_CAPACITY` = 16), `crates/od-machine/src/local_apic.rs` (`LocalApicHandle::timer_vector`, read-only), `crates/od-machine/src/stage4_interrupt_queue_probe.rs` (профиль `Stage4InterruptQueueProbeV1`, строгий загрузчик), fixture `tests/fixtures/guest/stage4/interrupt_queue/` (`entry.S`, `layout.ld`), builder `tools/build/build_stage4_interrupt_queue_probe.py`, runner `--stage4-interrupt-queue-probe` (`tools/runner/src/stage4_interrupt_queue_probe.rs`). Принятые `EventQueue`, `ScheduledEvent` и `EventGeneration` не менялись: pump использует их `schedule`, `cancel`, `peek_due`, `remove_due`, `reset`, `next_deadline`, `generation`.

**Очередь.** Pump владеет одной `EventQueue` ёмкостью 16 и таблицей `key -> (вектор, срок)`. Producer остаётся caller-driven: `schedule_irq(generation, key, vector, due_ns)` ставит событие с фиксированным вектором (вектор < 16 отвергается `InvalidVector`: Local APIC не принимает такие запросы; сессия без clock — `NoClock`; ошибки очереди — `Queue(..)`). Срок не позже текущего времени наступает на следующей границе; более поздний ждёт idle advance. Pump сам событий не создаёт и потоков не имеет.

**Снятие на границе.** Порядок границы: STOP, бюджет шагов, dispatch таймера, dispatch очереди, затем проверка APIC и одна инструкция либо `poll_apic`. Dispatch очереди для каждого due-события: `peek_due` -> `request_fixed_edge(вектор)` -> `remove_due` только после успешного запроса, поэтому при ошибке запроса событие остаётся в очереди; результат `Rejected(QueueRequest { vector, error })`, состояние гостя не меняется, и этот отказ единственный не финальный (после него `run` можно вызвать снова; финальны прочие отказы). За один dispatch снимаются все due-события (не больше ёмкости). Due-событие, вектор которого уже стоит в IRR, схлопывается: новый фронт не возникает, в трассе `Coalesced { vector }` и счётчик `coalesced_irqs()`; подъём IRR отражается одним `Irr`. Два due-события с одним вектором дают один фронт, один вход и один запуск handler. Только фиксированный вектор; события с разными векторами арбитражу `eligible()` не подчиняются сверх принятого.

**Generation.** `irq_generation()` отдаёт токен текущей generation. `reset_irq_queue()` вызывает `EventQueue::reset`, очищает таблицу и возвращает новый токен; старый токен отвергается всеми операциями (`Queue(StaleGeneration)`), события старой generation не доставляются и не дублируются, ключи можно использовать заново. Это сброс очереди pump, а не сброс машины: связь со сбросом APIC/CPU (`run_boot`) — S4-37a.

**Арбитраж простоя (ISSUE-070).** В Waiting без eligible-вектора `idle_advance` продвигает часы до меньшего из двух сроков: таймера (`next_timer_deadline()`) и ближайшего срока очереди, но учитывает только те источники, которые ещё могут изменить IRR. Источник, чей вектор уже стоит в IRR при `eligible() == None` (заблокирован ISR или TPR), не ждётся: новое срабатывание только схлопнулось бы в тот же бит. Для таймера вектор берётся из `LocalApicHandle::timer_vector()` (LVT Timer, не маскирован); для очереди — из таблицы. Если полезных сроков нет, результат `IdleHalt` без продвижения времени (раньше: `Budget` и уход времени на budget·P). IF=0 по-прежнему даёт `IdleHalt` без продвижения. Если срок не в будущем, `Rejected(TimerNoProgress)` (теперь и для срока очереди). После `IdleHalt` из-за заблокированного вектора выход возможен только через EOI гостя, который в HLT выполнить нельзя: состояние остаётся `IdleHalt` до внешнего вмешательства (так же на железе гость не проснётся, пока вектор заблокирован).

**`LocalApicHandle::timer_vector() -> Option<u8>`.** Read-only: вектор LVT Timer, если таймер есть и не маскирован. Существующие методы не менялись.

**Профиль образа `Stage4InterruptQueueProbeV1`.** Образ `0x4000` байт с GPA `0x1000`: GDT `0x2000`, IDT `0x3000` (ровно один gate: вектор `0x62`, класс 6, `0x8E`, селектор 8, IST=0), код `0x4000`; счётчик — dword по GPA `0x5000`; RAM `0x7000` плюс MMIO Local APIC без таймера; TSC 1 ГГц (1 тик = 1 нс), clock принадлежит сессии. Начальное состояние как у `Stage4InterruptTimerProbeV1` (RIP `0x4000`, RFLAGS `0x2`, RSP `0x6FF0`, shadow Clear, IDTR ставит загрузчик). Гость: SVR = `0x1FF`, затем `STI; loop: HLT; JMP loop`; устройств не программирует. Handler: счётчик += 1, EOI, `IRETQ`. Размеры: пролог 16 байт, HLT по `0x4011`, RIP после HLT `0x4012`, handler `0x4014`.

**Ожидания (правила очереди и Intel SDM Vol. 3A 6.14.2–6.14.3, 10.8.3.1; не вывод кода).** Кадр внешнего прерывания: RSP `0x6FC8` (`0x6FF0 - 40`), снизу вверх RIP, CS `0x08`, RFLAGS `0x202`, RSP `0x6FF0`, SS `0x10`; в handler RFLAGS `0x2`; wake из HLT кладёт RIP после HLT, приём на границе при IF=1 перед HLT — RIP самого HLT; EOI снимает ISR. Время после wake из idle advance равно сроку события (инструкции 0 нс). Фиксированный сценарий runner (вектор `0x62` во всех событиях): фаза 1 — caller отключает APIC (`set_global_enabled(false)`) и ставит E1 (срок 0) и F (срок 250000 нс); `request_fixed_edge` отказывает, результат `Rejected(QueueRequest)`, `retired = 0`, RIP `0x4000`, в очереди 2 события; фаза 2 — APIC включён; запрос E1 проходит на первой границе (IRR взведён до программирования SVR), гость включает SVR, `STI; HLT`, wake от E1 в 0 нс, затем idle advance до F, wake в 250000 нс; фаза 3 — caller ставит P1 и P2 (оба со сроком 250000 нс, текущее время): один IRR-фронт, одно слияние, один wake. Итог: 3 wake (0, 250000, 250000 нс), счётчик 3, время 250000 нс, `coalesced` 1, очередь пуста, IRR/ISR вектора пусты, кадр каждого входа `[0x4012, 0x8, 0x202, 0x6ff0, 0x10]`. Трасса: `irr, sti, hlt, entry, eoi, iretq, hlt, irr, entry, eoi, iretq, hlt, irr, coalesced, entry, eoi, iretq, hlt`.

**Evidence.** Manifest builder содержит расписание, ожидания фазы 1, времена wake, счётчик, `coalesced`, кадр, трассу, финал и ожидания инъекции; machine-тесты (`interrupt_queue_pump`, 8 тестов) кодируют значения независимо; runner-тест сравнивает вывод runner с manifest. Тест ISSUE-070 и тест «минимум из двух сроков» идут через загрузчик `Stage4InterruptTimerProbeV1` (периодический таймер, вектор `0x61`): handler без EOI даёт `(IdleHalt, wakeups 1, counter 1, time 2P)` независимо от бюджета (100 и 10000), вектор `(IRR, ISR) = (true, true)`, `next_timer_deadline() == Some(3P)`, повторный `run` — `IdleHalt` за 0 шагов; событие очереди со сроком P/2 будит CPU раньше первого срока таймера (входы в P/2, P, 2P, 3P, 4P; P = 100000 нс).

**Инъекция (различима по (stop, wakeups, counter, time)).** `PumpInjection::PopBeforeRequest`: событие удаляется до `request_fixed_edge`, и ошибка запроса теряет его. Принятый запуск: фаза 1 оставляет 2 события, итог `(IdleHalt, 3, 3, 250000)` (runner) или `(IdleHalt, 2, 2, 250000)` в machine-сценарии из двух событий E1 и F; инъекция: фаза 1 оставляет 1 событие, итог `(IdleHalt, 2, 2, 250000)` (runner) или `(IdleHalt, 1, 1, 250000)` (machine). Тест ISSUE-070 дополнительно проверен мутацией: при отключении проверки «вектор в IRR» тест падает с `Budget`.

**Отклонения и ограничения.** (1) `Rejected(QueueRequest)` — единственный возобновляемый отказ; он добавлен в `run` как исключение из финальности (принятые отказы S4-27i1/i2 не менялись). (2) В Running очередь тоже снимается на границе, но время по инструкциям 0 нс (S4-37a): наблюдаемое срабатывание из будущего идёт только через idle advance; due-событие из Running наблюдаемо (тест «до HLT при IF=1»). (3) Источник со сроком, чей вектор заблокирован, не ждётся, а событие остаётся в очереди; если позже вектор освободится (EOI вне HLT невозможен), событие доставится при следующем dispatch. (4) Для события очереди «заблокирован» означает «вектор уже в IRR при пустом `eligible()`». Событие с другим вектором более высокого приоритета, срок которого позже, продвижение времени не блокирует: это проверено по коду (срок очереди попадает в минимум), но тестом не покрыто — загрузчики допускают один gate, а варианта с блокировкой через TPR в срезе нет. (5) Загрузчик `stage4_interrupt_queue_probe.rs` повторяет примерно половину `stage4_interrupt_timer_probe.rs` (DRY-долг, владелец S4-27i5 или отдельный подсрез). (6) Тип `PumpTraceKind` получил вариант `Coalesced`; разборы `match` в runner-модулях S4-27i1 и S4-27i2 дополнены этим вариантом (поведение не изменено). (7) Сброс очереди не связан со сбросом APIC и CPU. (8) Однопоточная модель; caller сериализует всё, как у принятых примитивов.

**Не входит.** Реестр устройств, обобщённая выборка линий IOAPIC, асинхронные producer, арбитраж приоритетов разных векторов сверх `eligible()`, paged-pump и paged-wake (i4), `run_boot` и плата (S4-37a), время по инструкциям (S4-37a), TSC-deadline, x2APIC, JIT, Linux. Статус S4-27 и S4-27i в плане не менялся; ISSUE-070 в `KNOWN_ISSUES.md` не закрыта (решение координатора после ревью). Отчёт: [S4-27i3](../reports/STAGE_4_S4-27I3_2026-11-09.md).

## S4-27i4a: paged STI/HLT-шаги и paged wake из HaltWait

Реализация: `crates/od-isa/src/execute.rs` (`step_paged_sti`, `step_paged_hlt`, профили `PagedStiLong64V1`, `PagedHltLong64V1`, общий приватный путь `step_paged_one_byte_state_op`), `crates/od-machine/src/paged_memory_adapter.rs` (`step_paged_sti`, `step_paged_hlt`, `PagedStateOpStepReport`), `crates/od-machine/src/paged_apic_entry.rs` (`poll_paged_apic_entry`, `PagedApicEntryResult`, `PagedApicEntryReport`), `crates/od-machine/src/stage4_paged_sti_hlt_probe.rs` (профиль `Stage4PagedStiHltProbeV1`, сценарии, последовательность шагов, инъекции), механический вынос общих частей загрузчика в `stage4_paged_irq_probe.rs` (`build_bus`, `handoff_state`, `expected_image(main_code)`, `validate`; поведение S4-27h не менялось), fixture `tests/fixtures/guest/stage4/paged_sti_hlt/`, builder `tools/build/build_stage4_paged_sti_hlt_probe.py`, runner `--stage4-paged-sti-hlt-probe IMAGE [--budget N] [--inject-idt-not-present | --inject-stack-read-only]`.

**Что было до среза.** Paged `HLT` существовал только в `step_paged_probe_inner` (профиль `PagedCpl0ProbeV1`) и требует `cr3 == None`; `step_paged_cr3_probe` (CR3 = Some) отвергает всё, кроме `MOV CR3` и `MOV r,[m]`. Paged `STI` не было нигде. `DiagnosticHaltWait::poll_apic` входит через flat `deliver_maskable_interrupt` и отвергает CR0/CR3/CR4.

**Точные шаги.** `step_paged_sti` исполняет ровно `FB`, `step_paged_hlt` ровно `F4`; общие предусловия как у `step_paged_mov_dword_store` (Long64/CPL0, CR0 и CR4 = BASE, EFER = `0xD00`, `cr3 < 2^36`, активность Running, STI shadow задан); один байт fetch через tracked walker, данные не читаются; чужой байт (в том числе префиксные формы) — `Unsupported`, декодер не вызывается; общий paged allowlist не расширялся. STI: IF = 1; при IF = 0 до STI — STI shadow `Blocking`, при IF = 1 — `Clear` (SDM Vol. 2B STI); STI внутри shadow отвергается (`StiShadowActive`). HLT: RIP += 1, активность `Halted`, shadow `Clear` (инструкция после STI завершилась); допускается при shadow `Blocking` только с IF = 1, иначе `StiShadowActive`; уже остановленный CPU — `Halted` без инструкции. CPL0 не выше IOPL, #GP не возникает.

**Paged wake.** `poll_paged_apic_entry(state, bus, apic, cr3, efer, context, rflags_defined, committed_write)` — один caller-driven путь для wake из HLT (CPU Halted) и входа на границе (CPU Running): при IF = 0 или STI shadow `Blocking` либо без `eligible()` результат `Idle` (ничего не меняется, остановленный CPU продолжает ждать); иначе доставка на копии состояния через принятый `deliver_paged_external_interrupt` (S4-27h, без изменений) и публикация копии только вместе с IRR -> ISR (`acknowledge_vector`). Исходы: `Entered{vector,new_rsp}` (CPU переводится в Running), `OutsideProfile` (до коммита; остановленный CPU остаётся остановленным, IRR сохранён), `Aborted{phase,cause}` (терминал, модель), `SelectionChanged`. Запрос вектора ставит caller (`request_fixed_edge`); функция ничего не планирует. Flat `DiagnosticHaltWait` не менялся.

**Профиль образа `Stage4PagedStiHltProbeV1`.** Раскладка S4-27h (страницы таблиц, main, handler, IDT, GDT, стек, Local APIC; 4 KiB; CR0/CR4 = BASE; EFER = `0xD00`; SVR = `0x1FF`); main — `STI; HLT; HLT` (`FB F4 F4`), handler — `89 03 48 CF`. Сценарии: `WakeFromHlt` (RFLAGS `0x002` на входе) и `BoundaryBeforeHlt` (RFLAGS `0x202`).

**Последовательность (wake, 7 шагов, 5 инструкций).** `Sti` (RIP `0x401001`, RFLAGS `0x202`, shadow); `Hlt` (RIP `0x401002`, Halted, shadow снят); `IdlePoll` (ничего не запрошено, `Idle`); `Wake` (запрос `0x41`, вход: RIP `0x402000`, RSP `0x700FC8`, RFLAGS `0x002`, IRR 0 / ISR 1); `Eoi` (`89 03` по `0x6020B0`, ISR 0); `Iretq` (RIP `0x401002`, RSP `0x700FF8`, RFLAGS `0x202`); `ResumeHlt` (RIP `0x401003`, Halted). Boundary (6 шагов, 4 инструкции): `Sti` (shadow нет), `Entry` (кадр RIP `0x401001` — адрес HLT), `Eoi`, `Iretq` (возврат в HLT), `Hlt` (RIP `0x401002`), `IdlePoll`.

**Ожидания (Intel SDM Vol. 2B STI, Vol. 2A HLT, Vol. 3A 6.8, 6.14.2/6.14.3, 13.8.4/13.9, 4.8; не вывод кода).** Кадр wake снизу вверх: RIP `0x401002` (после HLT), CS `0x08`, RFLAGS `0x202`, RSP `0x700FF8`, SS `0x10`; новый RSP `0x700FC8` (`0x700FF0 - 40`); итоговые PTE: main, handler, IDT, GDT — A (`0x6023`, `0x7023`, `0x8023`, `0x9023`), APIC и стек — A и D (`0xFEE00063`, `0xA063`).

**Инъекции (модель; различимы по stop, steps, IRR/ISR, RSP, RIP, A/D).** `IdtGateNotPresent`: `OutsideProfile(InvalidGate)` до коммита, 4 шага, 2 инструкции, CPU остаётся Halted (RIP `0x401002`, RSP `0x700FF8`, RFLAGS `0x202`), IRR 1 / ISR 0, `eligible() == Some(0x41)`; A только на IDT (`0x8023`; GDT `0x9003`, стек `0xA003`, handler `0x7003`). `StackReadOnly` (PTE `0xA001`, CR0.WP=1): `DeliveryAborted{StackWrite{OldSs}}`, #PF P|W (код 3) по `0x700FE8`, терминал, вектор не подтверждён (IRR 1 / ISR 0), CR2 и CPU не меняются; A на IDT, GDT, стеке (`0xA021`, D нет).

**Модельные расхождения с железом.** Отказы входа терминальны: аппаратный исход — последовательный #PF/#NP (SDM Vol. 3A 6.15, 6.7), вложенная доставка — S4-33. Тип памяти страницы APIC (PCD/PWT/PAT) не моделируется. Запись кадра — `ImplicitStack` (как в S4-27h).

**Не входит.** Pump поверх paged-бэкенда и трейт бэкенда (S4-27i4b), paged-таймер, очередь IRQ на paged, JIT; вложенные события (S4-33); CPL3, IST != 0; несколько векторов; CLI и префиксные формы STI/HLT; тип памяти LAPIC. Boundary-сценарий проверяется machine-тестом и не доступен из runner. Статус S4-27 и S4-27i в плане не менялся. Отчёт: [S4-27i4a](../reports/STAGE_4_S4-27I4A_2026-11-09.md).

## S4-27i5: перевод UART-probe на pump, общий загрузчик и закрытие оговорок S4-27

Реализация: `crates/od-machine/src/interrupt_pump.rs` (аддитивное расширение: `PumpHost`, `PumpHostEvent`, `PumpHostStep`, `PumpTraceKind::Host`, `PumpRejection::Host`, поле `clock_ns` в `PumpTraceEvent`, `InterruptPump::with_host`, `with_entry_only_delivery`, `host`, `host_mut`, `advance_time_ns`), `crates/od-machine/src/stage4_uart_irq_probe.rs` (перевод), `crates/od-machine/src/stage4_interrupt_probe_common.rs` (общий загрузчик), `stage4_interrupt_pump_probe.rs`, `stage4_interrupt_timer_probe.rs`, `stage4_interrupt_queue_probe.rs` (тонкие обёртки), `tools/runner/src/stage4_interrupt_{pump,timer,queue}_probe.rs` (одна пустая ветка match для нового варианта трассы). Публичный API UART-probe сохранён; поведение S4-24e сохранено для всех сценариев с тестами и runner, кроме финальности STOP (см. «Отклонения и ограничения», п. 6–8); поведение S4-27i1–i3 без хоста не менялось; новой ISA-семантики нет.

**Что переведено.** `Stage4UartIrqProbe` больше не содержит цикла `run/step_waiting/step_running`: он держит `InterruptPump` с наблюдаемым вектором `0x51`. STOP, бюджет, финальность выходов, проверка границы, доставка, STI (`run_sti_apic_quantum`), HLT (`into_halt_wait`), IRETQ (`run_same_cpl0_iretq`), wake из HLT (`poll_apic`), idle advance, события `Irr`/`Entry`/`Eoi`/`Sti`/`Hlt`/`Iretq` исполняет и пишет pump. Публичный API (`load_stage4_uart_irq_probe`, `run`, `state`, `trace`, `serial_bytes`, `uart`, `clock_ns`, `retired_total`, `pending_rx`, `rx_generation`, `vector_state`, `count`, `schedule_rx`, `advance_time_ns`, типы трассы, выхода и отчёта) сохранён; `trace()` — отображение трассы pump на `UartIrqTraceEvent`, обновляется в конце каждого `run`.

**Что осталось адаптером (`UartOwner`, реализация `PumpHost`) и почему.** (1) Очередь RX (`UartRxEventPump`) и её срок: это не IRQ-очередь pump, а очередь доставки байта в регистр приёма UART; хост снимает due-байты на каждом шаге (`dispatch`) и даёт срок для idle advance (`next_deadline`, только будущий срок: due-байт за непрочитанным RBR не ждётся). (2) Линия UART→INTIN4: после каждой операции, меняющей состояние UART, хост снимает `interrupt_line` и передаёт изменение уровня в IOAPIC (`set_intin4`); pump про UART не знает. (3) Мгновенная передача (`drain_tx`). (4) Две инструкции, которых нет у обычного интерпретатора pump: `LIDT [RSP+0]` (`step_flat_lidt`) и `IN AL,DX`/`OUT DX,AL` (`step_pio_u8`); хост получает управление на границе без доставки раньше остальных шагов (`PumpHost::step`). (5) Отображение результатов: `PumpExit`/`PumpRejection` в `Stage4UartIrqProbeExit`, `PumpTraceKind` в `UartIrqTraceKind`.

**Расширение pump (по умолчанию выключено).** Без хоста и без флага pump ведёт себя так же, как в S4-27i1–i3 (проверено всеми тестами pump, 12 + 7 + 8, без правки ожиданий). (a) `PumpHost`: `dispatch` до наблюдения IRR (события хоста попадают в трассу раньше `Irr`), `step` для инструкций хоста (события пишутся после учёта результата, терминальный результат событий не даёт), `next_deadline` для idle advance. Срок хоста ждётся и при IF=0 (для таймера и очереди pump остаётся правило S4-27i2: IF=0 — `IdleHalt` без продвижения), потому что работа хоста сама может поднять IRR; в трассу пишется `Host{idle_advance, to_ns}`, если часы остановились на сроке хоста. (b) `with_entry_only_delivery()`: доставка на границе только входит в handler (`deliver_pending_irq`), первая инструкция handler исполняется отдельным шагом; поэтому `Entry{Boundary}` имеет `rip` = адрес handler, число квантов и retired совпадают со старым S4-24e, а первая инструкция handler не ограничена обычными инструкциями. Режим `run_apic_quantum` (по умолчанию) не менялся. (c) Часы: сессия UART-probe создаётся с общим `VirtualClockHandle` и владеет часами; `advance_time_ns` идёт через сессию; когда CPU остановлен или terminal, продвижение отказывает (`Stage4UartIrqProbeError::ClockState`, новый вариант; `Clock` сохранён).

**Эквивалентность перевода.** Тесты S4-24e (10) прошли без правки ожиданий. Дополнительно семь сценариев (нормальный, RX до IER, два байта, IER=0, поздний unmask, без EOI, без чтения RBR; budget 1000) выполнены на коде до и после (по заявлению исполнителя сравнение с HEAD проверено; STOP и бюджет 3 сравнивались только на новом коде, без HEAD): выходы, `quanta`, `retired`, `clock_ns`, RIP/RFLAGS/IRR/ISR и вся трасса (seq, clock_ns, rip, kind) совпали побайтово. Зафиксированные значения: нормальный сценарий — `IdleHalt`, quanta 42, retired 40, clock 1000; два байта — 55 и 51, clock 2000.

**Отклонения и ограничения.** (1) Текст `Rejected(..)` и `IntegrationError(..)` формируется из отказов pump и может отличаться от текста старого цикла (ни один тест не проверяет текст; тип выхода сохранён, кроме случая (8) ниже). (2) CLI (`FA`): pump исполняет его через `run_sti_apic_quantum`, старый цикл давал terminal `Unsupported`; для образов S4-24e это не наблюдаемо (CLI в профиле нет), событие `Cli` в UART-трассу не отображается. (3) Частичный REP довершается (поведение pump S4-27i1), старый цикл давал `IntegrationError`; в профиле REP нет. (4) Новый вариант `PumpTraceKind::Host` вынудил добавить ветку в три runner-а pump/timer/queue (JSON не меняется: у них нет хоста). (5) Предел трассы общий для событий pump и хоста (512), как в S4-24e. (6) **STOP стал финальным — изменение поведения S4-24e.** Старый цикл на STOP делал `break` с `Stopped` без `finished`: session не останавливалась, следующий `run` продолжал работу. Pump на STOP вызывает `stop_cpu()` и ставит `finished = Stopped`: все следующие `run` возвращают `Stopped`, `advance_time_ns` отказывает `ClockState`. Тесты и runner (`--cancel-after`, один `run`) этого не видят. Принята семантика pump (S4-27i1); возобновляемый STOP в адаптере не сохранён (кандидат ISSUE-072, если понадобится). (7) Доставка внутри STI-кванта (`step_sti_quantum`, `Delivered`): режим entry-only там не действует, первая инструкция handler исполняется в том же кванте, `Entry.rip` = адрес handler + длина этой инструкции. Старый цикл давал `Rejected("STI quantum: …")`. (8) Wake из HLT с ошибкой `into_running`: раньше `IntegrationError`, теперь `Rejected(HaltEntry(ProfileRejected))`; для этого случая тип выхода не сохранён. (9) Уточнение к фразам «поведение S4-24e не менялось» и «публичный API сохранён»: публичный API сохранён; поведение сохранено для всех сценариев с тестами и runner, кроме финальности STOP (6) и отклонений (7), (8).

**Общий загрузчик (DRY-долг S4-27i1/i2/i3).** Три загрузчика совпадали на 90 процентов: проверка образа (GDT, один gate на разрешённый вектор, запрет ненулевых байт вне GDT/IDT/кода), карта памяти с RAM и MMIO Local APIC, шина, начальное Long64/CPL0 состояние, IDTR в состоянии, pump с наблюдаемыми векторами. Теперь это `stage4_interrupt_probe_common.rs`: `FlatProbeSpec` (константы, список векторов, текст ошибки лишнего gate, часы и частота таймера APIC), `validate_flat_image`, `build_flat_pump`, а также `flat_entry_state` и `flat_access_context`, которые использует и загрузчик UART-probe. Три модуля сохраняют свои публичные константы, перечисления ошибок (`From<FlatProbeError>`), функции `load_stage4_interrupt_*_probe` и тексты ошибок; различия (pump без часов, timer — часы и таймер APIC, queue — часы без таймера, порядок наблюдаемых векторов `0x41, 0x51`) заданы данными. Загрузчик UART-probe (IOAPIC, UART, свой валидатор образа с LIDT и IDTR) остаётся отдельным: общий только вход состояния.

**Оговорки S4-27: было и стало.** Было (план, S4-27): «доставка только плоская identity Long64/CPL0 без paged-входа и IRETQ; автоматического event pump и scheduler нет (предлагаемый владелец S4-33)». Стало.
- *Закрыто для paged:* paged вход и IRETQ одного внешнего вектора на Long64/CPL0, 4 KiB, CR0/CR4=BASE, EFER=`0xD00`, IST=0 (S4-27h); paged STI/HLT и paged wake из HaltWait для того же профиля (S4-27i4a).
- *Закрыто для flat event pump:* единый owner-цикл (граница инструкции, STI/HLT/wake, IRQ от caller — S4-27i1; owner-driven таймер APIC с idle advance — S4-27i2; очередь IRQ-событий и арбитраж простоя, ISSUE-070 — S4-27i3); он же теперь исполняет UART-probe (S4-27i5). Владелец event pump — `InterruptPump`, не S4-33. Scheduler и потоки не вводились: цикл синхронный, producer — caller или хост.
- *Остаётся:* (1) pump поверх paged-бэкенда и общий трейт бэкенда — S4-27i4b (открыт); (2) `run_boot`, плата, подключение APIC timer, PIT/PIC/INTIN2, порт-тик и синхронизация часов по инструкциям, ISSUE-067 — S4-37a; (3) общий paged-интерпретатор и декод Linux EOI `movl` с disp32 при CR0 шире BASE — S4-38; (4) вложенные #PF/#NP/#DF и shutdown при доставке, CPL3 и IST != 0, uncached/TLB — S4-33; (5) JIT внутри pump (`not_supported`, отмечается в S4-33); (6) MOV SS и её тень (задача этапа 2, предусловие S4-37/S4-38); NMI неприменимо. Статус `[x]` S4-27 и S4-27i в плане этим срезом не ставился.

**Не входит.** Новые ISA-семантики, второй хост, IRQ-очередь pump для UART (RX остаётся очередью UART), перенос линии UART в pump, paged-pump, JIT. Отчёт: [S4-27i5](../reports/STAGE_4_S4-27I5_2026-11-09.md).

## S4-27i4b: pump поверх paged-бэкенда (трейт `PumpBackend`)

Реализация: `crates/od-machine/src/interrupt_pump.rs` (трейт `PumpBackend`, `BackendStep`, `BackendFlow`, `BackendEnv`, `FlatBackend`; `InterruptPump<B: PumpBackend = FlatBackend>`, `InterruptPump::with_backend`, `backend`, `backend_mut`), `crates/od-machine/src/paged_pump_backend.rs` (`PagedBackend`), `crates/od-machine/src/stage4_interrupt_pump_paged_probe.rs` (профиль `Stage4InterruptPumpPagedProbeV1`, `load_stage4_interrupt_pump_paged_probe`, `enable_stage4_interrupt_pump_paged_apic`, `expected_stage4_interrupt_pump_paged_image`), runner `--stage4-interrupt-pump-paged-probe IMAGE [--budget N] [--request VECTOR@AFTER_QUANTA] [--inject-pop-before-request]` (`tools/runner/src/stage4_interrupt_pump_paged_probe.rs`). Поведение flat не менялось, кроме порядка записи при исчерпании `INTERRUPT_PUMP_TRACE_LIMIT` внутри одного шага (см. отклонение (8)); новой ISA-семантики и расширения paged-allowlist нет.

**Трейт.** Pump остаётся владельцем цикла: STOP, бюджет, dispatch таймера и очереди, `observe`, трасса, idle advance, арбитраж ISSUE-070, очередь IRQ и generation. Бэкенд — сторона CPU: `state`, `is_waiting`, `virtual_time_ns`, `advance_time_ns`, `stop`, `peek_ram_qword` и один `step(BackendEnv) -> BackendStep`, который на границе или в halt-wait делает одно из: wake из HLT, вход в handler, исполнение инструкции по RIP. `BackendStep` — число retired инструкций, события трассы (pump пишет их по порядку с состоянием после шага) и действие `BackendFlow`: `Continue`, `Idle` (CPU остановлен, ничего не eligible: pump продвигает часы к ближайшему сроку или даёт `IdleHalt`) или `Exit(PumpExit)`. `BackendEnv` одалживает шагу APIC, инъекцию и хост S4-27i5 (только flat). `InterruptPump` по умолчанию параметризован `FlatBackend`, поэтому `InterruptPump::new`, `with_host`, `with_entry_only_delivery` и все вызовы S4-27i1–i3, i5 остались как были.

**Flat-реализация.** `FlatBackend` — прежняя логика `step_waiting/step_running` без изменений (`poll_apic`, `run_apic_quantum`, `run_sti_apic_quantum`, `run_same_cpl0_iretq`, `into_halt_wait`, частичный REP, entry-only режим, хост), перенесённая из pump: состояние `Cpu{Running|Waiting}`, `mid_rep` и `entry_only` живут в бэкенде. Единственное структурное изменение pump: вместо вызова приватных `step_waiting/step_running` он берёт `BackendStep`, прибавляет retired, пишет события, затем сам выполняет `Idle` (бывший `idle_advance` над `HaltWait`). Тесты pump (12 + 7 + 8), UART (10), `diagnostic_vcpu_api` 8, `--lib diagnostic_vcpu` 46, `paged_sti_hlt_wake` 9, `stage4_paged_irq_probe` 9 прошли без правки ожиданий. Единственное наблюдаемое отличие от прежнего кода — порядок при исчерпании `INTERRUPT_PUMP_TRACE_LIMIT` внутри одного шага (retired прибавляется до записи событий): недостижимо в тестах.

**Paged-реализация.** `PagedBackend` держит `LongModeState`, `MachineBus`, простые `VirtualClock`, `cr3`, `efer` и размер RAM. Шаг: (1) `poll_paged_apic_entry` (S4-27i4a; IF, shadow, `eligible()`, доставка `deliver_paged_external_interrupt` S4-27h на копии, публикация вместе с IRR→ISR): `Entered` → событие `Entry{HaltWake}` для остановленного CPU или `Entry{Boundary}` для работающего, `Idle` у остановленного → `BackendFlow::Idle`, `OutsideProfile` → `Rejected(HaltWake | InterruptEntry)`, `Aborted` → terminal `InterruptDeliveryAborted`, `SelectionChanged` → terminal `InterruptSelectionChanged`; (2) иначе работающий CPU исполняет инструкцию по RIP: опкод читается tracked fetch того же walker: однобайтовые опкоды — одним fetch, `48 CF` и `89 03` — двумя побайтовыми (A-бит ставится так же, как собственным fetch инструкции), затем `FB` → `step_paged_sti`, `F4` → `step_paged_hlt` (CPU становится Halted), `48 CF` → `step_paged_iretq`, `89 03` → `step_paged_mov_dword_store` (EOI-store handler); любой другой опкод — terminal `Unsupported{opcode}`, как у flat pump. Новых шагов не добавлено. `peek_ram_qword` читает только RAM (MMIO-адрес даёт `None`).

**Что общее, что специфично.** Общее (один код): очередь IRQ и generation, `peek_due → request_fixed_edge → remove_due` и инъекция `PopBeforeRequest`, схлопывание due-событий, idle advance до срока очереди, арбитраж простоя, трасса и `observe`, финальность выходов, бюджет и STOP, API `schedule_irq/cancel_irq/reset_irq_queue`. Специфично: сами шаги CPU и память (session и flat identity против paged walker и tracked шагов), часы (у flat — часы сессии, у paged — собственный `VirtualClock` бэкенда), хост и REP (только flat).

**Профиль образа `Stage4InterruptPumpPagedProbeV1`.** Раскладка S4-27h/S4-27i4a (4 KiB, CR0/CR4 = BASE, EFER = `0xD00`, SVR = `0x1FF`, вектор `0x41`, handler `89 03 48 CF`), main — `STI; HLT; HLT; HLT` (`FB F4 F4 F4`): три HLT нужны для двух пробуждений без перехода (нового ISA-шага нет). Это единственное отличие от образа S4-27i4a; вектор очереди остаётся `0x41` (единственный gate раскладки), а не `0x62` flat-образа. Вход: `InterruptsOff` (RFLAGS `0x002`, STI ставит shadow, HLT завершается в shadow) или `InterruptsOn` (`0x202`, граница перед HLT открыта). Часы 1 ГГц, на 0 нс; APIC без таймера, поэтому время двигает только очередь.

**Проверка.** `cargo test -p od-machine --offline --test interrupt_pump_paged`: 14 тестов. (a) очередь S4-27i3 на paged: due доставлено на 0 нс, future ждёт срока (250000 нс) и будит HLT в этот момент, кадр несёт RIP после HLT, итоговые A/D `0x6023, 0x7023, 0x8023, 0x9023, 0xFEE00063, 0xA063`; вход на границе перед HLT при IF=1 (RIP кадра = адрес HLT); pause/resume (`run(0)`, один шаг = idle advance, событие доставляется один раз); reset: старая generation отвергается (`StaleGeneration`), события старой generation не доставляются; два due с одним вектором — один IRR-фронт, один `Coalesced`, один вход. (b) отказ request (APIC отключён caller, `UnsupportedConfiguration`): событие остаётся, принятое `(IdleHalt, 2 входа, 2 возврата handler, 250000 нс)`, инъекция `PopBeforeRequest` — `(IdleHalt, 1, 1, 250000)`, очередь после отказа 2 против 1. (c) равенство flat и paged: четыре сценария (due+future, вход на границе, схлопывание, отказ request с инъекцией и без) на flat-образе S4-27i3 (вектор `0x62`, entry-only режим) и paged-образе дают равные `Arch`: имена и источники событий трассы и часы в каждом событии, для каждого входа — источник, время, «в handler», RIP кадра относительно HLT, CS, RFLAGS, SS кадра, «сохранённый RSP = верх стека», «новый RSP = верх, выровненный до 16, минус 40», IRR/ISR, IF=0 в handler, число возвратов handler и EOI, IRR/ISR в конце, время, длина очереди, число схлопнутых, IF и RSP в конце. Дополнительно: IDT P=0 даёт `Rejected(HaltWake(InvalidGate))` с сохранённым IRR, код после последнего HLT — terminal `Unsupported{0}`, STOP финален. Runner: `cargo test -p od-runner --offline --test stage4_interrupt_pump_paged_cli` (4; образ собран в тесте независимо от загрузчика).

**Отклонения и ограничения.** (1) Строка «счётчик» flat (dword в RAM) на paged заменена числом возвратов handler (`Iretq` в трассе): у paged-handler нет счётчика, а менять образ S4-27h нельзя; на flat счётчик проверен отдельно и равен числу входов. (2) Вход на границе у paged — только вход (первая инструкция handler исполняется следующим шагом), поэтому сравнение идёт с flat в `with_entry_only_delivery`; режим flat по умолчанию (первая инструкция handler в том же кванте) на paged не имеет аналога. (3) Глобальное отключение APIC сбрасывает его состояние, включая SVR: flat-guest сам повторяет запись SVR, у paged-guest кода APIC нет, поэтому owner повторяет запись загрузчика (`enable_stage4_interrupt_pump_paged_apic`). (4) Инъекции `SkipShadowCheck` и `HltAsOrdinary` не имеют paged-аналога (вход S4-27h сам проверяет shadow; HLT идёт только через `step_paged_hlt`); `NoTimerDispatch` не наблюдается (у paged APIC нет таймера). (5) Хост S4-27i5 и частичный REP — только flat. (6) Отказы входа и paged fetch терминальны или `Rejected` (модель; аппаратный исход — последовательный #PF/#NP, вложенная доставка — S4-33); тип памяти LAPIC не моделируется. (7) Builder на Python для этого образа не создан: образ отличается от образа S4-27i4a одним байтом main, runner-тест собирает его независимо по константам builder S4-27i4a. (8) Порядок записи при исчерпании `INTERRUPT_PUMP_TRACE_LIMIT` внутри одного шага чуть изменён (retired прибавляется до записи событий); в тестах недостижимо. (9) Ошибка `advance_time_ns` в Running: paged даёт `TimerAdvance`, flat — `Control` (разные варианты ошибки при одинаковой ситуации).

**Не входит.** Вложенные события (S4-33), `run_boot` и плата (S4-37a), paged-таймер, JIT внутри pump (`not_supported`, S4-33), несколько векторов, CLI на paged, CPL3 и IST != 0, общий paged-интерпретатор (S4-38). Статус S4-27 и S4-27i в плане не менялся. Отчёт: [S4-27i4b](../reports/STAGE_4_S4-27I4B_2026-11-09.md).

## LAPIC на каждый vCPU (S6-11)

Реализация: `crates/od-machine/src/apic_pair.rs` (`CpuApic`, `PairState`, `Route`), `local_apic.rs` (`LocalApic`, `LocalApicHandle`), `io_apic.rs`, `bus.rs` (`BusDevice::read_from_cpu`/`write_from_cpu`, `MachineBus::access_tracked`), `paged_memory_adapter.rs` (`AdapterBus::Machine`), `vcpu.rs` (`MAX_VCPUS`, `vcpu_set`), `firmware_board.rs` (`FirmwareBoardConfig::vcpu_count`). Источник требований — [SMP_CONTRACT.md](SMP_CONTRACT.md) §2 и §6.

**Число vCPU.** `FirmwareBoardConfig::vcpu_count`, по умолчанию 1, допустимо `1..=4` (`MAX_VCPUS`); иначе `FirmwareBoardError::Config("vcpu_count must be in 1..=MAX_VCPUS")`. Плата держит `vcpus: Vec<Vcpu>` длиной `vcpu_count`: процессор 0 получает состояние входа, остальные — `LongModeState::default()`; исполняется только процессор 0 (запуск AP — INIT/SIPI, S6-13). Блок передачи управления, ACPI blob и CPUID по-прежнему описывают один процессор: их `N` не зависит от `vcpu_count` (S6-15/S7-02). `LocalApic::with_cpus(n)` и `with_shared_timers(clock, hz, n)` создают n LAPIC; `n` вне `1..=4` — `UnsupportedConfiguration`. Прежние конструкторы создают один LAPIC.

**Состояние.** `PairState { cpus: Vec<CpuApic>, route: Route }` под одним `Mutex`, который уже был у пары LAPIC+IOAPIC (блокировок не добавлено). `CpuApic { local: State, timer: Option<Timer> }` — LAPIC одного процессора: свои регистры, свой таймер со своей `EventQueue` (на общих часах платы). `Route` — состояние IOAPIC. Одна блокировка нужна потому, что level-запрос связывает их: доставка ставит IRR/TMR в LAPIC адресата и Remote IRR в `Route`, EOI адресата снимает Remote IRR. Индекс в `cpus` равен APIC ID и индексу `Vcpu`.

**Окно `0xFEE0_0000`.** Доступ идёт в LAPIC того vCPU, который его выполняет. Индекс передаётся явно: `BusDevice::read_from_cpu(cpu, access)`/`write_from_cpu(cpu, access, value)` (по умолчанию вызывают `read`/`write`; различает процессоры только `LocalApic`), `MachineBus::access_tracked(vcpu, …)`, а в адаптере памяти его несёт `AdapterBus::Machine(bus, vcpu)`. Глобальной переменной «текущий процессор» нет. `BusDevice::read`/`write` и `AdapterBus::from(&mut MachineBus)` — доступ процессора 0 (`BOOT_VCPU_INDEX`); так работают все прежние вызовы. Индекс vCPU, которого нет на плате, — `UnsupportedConfiguration` без изменений состояния. Путь CPL0-цикла (`SetStepMmio`) по-прежнему обслуживает только ECAM; диагностический `IdentityMemoryAdapter`/`DiagnosticVcpuSession` и `read_physical`/`write_physical` — процессор 0.

**Регистры LAPIC.** ID (`0x20`) читается как `index << 24` (запись — `ReadOnly`). Остальные регистры, таймер (LVT Timer, начальный/текущий счёт, делитель), IRR/ISR/TMR, SVR, TPR, LVT, ESR хранятся и меняются по процессорам независимо. `LocalApicHandle` адресует LAPIC одного vCPU (`LocalApic::handle()` — процессор 0, `handle_for(i)` и `LocalApicHandle::for_cpu(i)` — процессор `i`): `dispatch_timer`, `request_fixed_edge`, `acknowledge*`, `reset`, `set_global_enabled`, `step_apic_base_msr`, `step_local_apic_cr8` действуют только на него. `reset_checked`/`reset_pair` сбрасывают LAPIC своего процессора и маршруты IOAPIC; LAPIC остальных не трогают. Отказ сброса/выключения при ожидающем EOI level-запросе относится только к LAPIC-адресату маршрута.

**IOAPIC.** Окно `0xFEC0_0000` одно на плату. Старший dword любой записи перенаправления: биты 31:24 — physical APIC ID, остальные зарезервированы (≠ 0 — `UnsupportedConfiguration`). APIC ID, которого нет среди vCPU, — `UnsupportedConfiguration` до любого изменения, для любого входа, в том числе без линии и под маской. Значение читается обратно. INTIN4 (edge/level) и INTIN2 (edge) доставляют запрос в LAPIC с APIC ID своей записи (после reset — 0). Остальные правила — прежние: Fixed, physical (бит 11 = 0), active-high, edge/level, Remote IRR, EOI; lowest priority, logical, NMI и прочие режимы по-прежнему отклоняются при записи low dword. Резервирование вектора level-входа против edge и против LVT Timer действует в том LAPIC, куда адресован вход (одинаковый вектор в разных LAPIC допустим). Смена APIC ID у level-входа отказывается, пока линия высока, ожидается EOI, вектор занят (IRR/ISR) в старом или новом LAPIC, либо вектор совпадёт с LVT Timer или входом 2 нового LAPIC; смена у edge-входа INTIN4 свободна; смена INTIN2 отказывается, если его вектор окажется зарезервированным level-вектором INTIN4 в новом LAPIC. EOI не от LAPIC-адресата Remote IRR не снимает.

**Не входит.** IPI между процессорами — [S6-12a](#доставка-ipi-s6-12a); NMI — [S6-12b](#nmi-s6-12b); INIT/SIPI — [S6-13](#init-sipi-и-ожидание-sipi-s6-13), logical destination, MSI, x2APIC, расширение MADT/CPUID/handoff до `N` (S6-15/S7-02, SMP_CONTRACT §2), исполнение AP, потоки и блокировки сверх прежнего `Mutex`.

## Доставка IPI (S6-12a)
Реализация: `local_apic.rs` (`decode_icr`, запись `ICR_LOW`), `apic_pair.rs` (`PairState::edge_all`). Источник — матрица [SMP_CONTRACT.md](SMP_CONTRACT.md) §5. Срез S6-12a не включал NMI: он реализован в [S6-12b](#nmi-s6-12b).

**Доставляется.** Запись aligned Dword в `ICR_LOW` (`0x300`) vCPU-отправителя ставит запрос Fixed edge в IRR адресатов под тем же замком, что остальное состояние пары; запись в `ICR_HIGH` по-прежнему только хранит байт адресата. Строки:

| Delivery | Shorthand | Destination mode | Вектор | `(Level, Trigger)` | Адресаты |
|---|---|---|---|---|---|
| Fixed | нет | physical | `0x10–0xFF` | `(0,0)` | vCPU с APIC ID = `ICR_HIGH[31:24]` отправителя |
| Fixed | Self | игнорируется | `0x10–0xFF` | `(0,0)` или `(1,0)` | отправитель |
| Fixed | All including self | игнорируется | `0x10–0xFF` | `(0,0)` | все vCPU |
| Fixed | All-but-self | игнорируется | `0x10–0xFF` | `(0,0)` | все, кроме отправителя (при одном vCPU — никто, запись принимается) |

С shorthand байт адресата и Destination Mode игнорируются. Запрос — тот же edge, что у `request_fixed_edge` и IOAPIC: тот же IRR, те же правила резервирования вектора level-входа в LAPIC адресата; одинаковые векторы сливаются в IRR. Адресат выполняет запрос на своей границе исполнения (запуск AP — S6-13), отправитель синхронного исполнения не ждёт.

**Ошибка устройства (`UnsupportedConfiguration`) до любого изменения состояния.** Всё, что не строка таблицы: зарезервированные биты (13, 17:16, 31:20; бит 12 игнорируется), Delivery mode кроме Fixed (Lowest Priority, SMI, зарезервированные 011 и 111; *с S7-32 Lowest Priority без shorthand принимается — одна цель, первая по адресату*), вектор `< 0x10`, logical destination без shorthand (*с S7-32 Fixed и Lowest Priority по логическому адресу принимаются, бит Level 14 — 0 или 1, триггер — фронт; адрес, не называющий ни одного vCPU, — ошибка*), APIC ID, которого нет среди vCPU (включая broadcast `0xFF`, которого в матрице нет), неуказанные пары `(Level, Trigger)` для строки, широкая запись больше 32 бит. Если запрос отказывает любой адресат (LAPIC глобально выключен или вектор зарезервирован level-входом IOAPIC), отказывает вся IPI: ни один LAPIC не меняется (`edge_all` сначала проверяет всех). Строки NMI матрицы SMP_CONTRACT §5 в срезе S6-12a давали эту ошибку; с [S6-12b](#nmi-s6-12b) принимаются обе строки (физический и All-but-self), NMI Self и All остаются ошибкой. Строки INIT и SIPI реализованы в срезе S6-13 ([ниже](#init-sipi-и-ожидание-sipi-s6-13)); в срезе S6-12a они давали эту ошибку.

**Delivery Status** (бит 12) — read-only: при записи игнорируется, читается Idle, потому что доставка в IRR завершена до возврата из записи. Idle означает «отправка закончена», а не «процессор принял прерывание». Хранимое значение `ICR_LOW` — записанное без бита 12; отказанная запись его не меняет.

**Изменение ожиданий S4-25c.** Self с `(Level, Trigger) = (0,0)` раньше отклонялся, теперь принимается: так требует матрица SMP_CONTRACT §5 (кодировка Linux `__prepare_ICR`). `(1,0)` принимается как прежде. Один vCPU: все прочие ожидания self-IPI прежние.

**NMI.** Реализован в срезе S6-12b ([ниже](#nmi-s6-12b)); в срезе S6-12a был отложен (D5/S4-38 для `od-isa/state.rs`, снято владельцем 2026-11-16).

## NMI (S6-12b)
Реализация: `local_apic.rs` (`IpiCommand::Nmi`, `decode_icr`, `LocalApicHandle::pending_nmi`/`take_nmi`), `apic_pair.rs` (`PairState::queue_nmi`), `vcpu.rs` (`NmiBoundary`), `firmware_board.rs` (`LoadedFirmwareBoard::deliver_pending_nmi`), `paged_memory_adapter.rs` (`deliver_nmi_at_boundary`); `od_isa::deliver_nmi_cpl0_set`, `od_isa::NMI_VECTOR`, `LongModeState::nmi_blocked`. Источник — SMP_CONTRACT §5 (две строки NMI), §7; SDM Vol. 3A 6.7.1 и 10.8.1 — по памяти, источник в репозитории недоступен.

**Принимается** (запись в `ICR_LOW`, delivery mode `100`, `(Level, Trigger) = (0,0)`, поле вектора игнорируется): physical без shorthand (адресат — `ICR_HIGH[31:24]` отправителя, существующий vCPU, им может быть и отправитель) и All-but-self (все, кроме отправителя; при одном vCPU — никто, запись принимается; байт адресата и Destination Mode игнорируются). NMI Self и All, логическая адресация без shorthand, APIC ID вне набора vCPU (включая `0xFF`), зарезервированные биты и прочие пары `(Level, Trigger)` — `UnsupportedConfiguration` до изменения состояния. LAPIC адресата с `IA32_APIC_BASE.EN = 0` отказывает (как Fixed, INIT, SIPI); при отказе любого адресата не меняется ни один LAPIC. Программное выключение через SVR NMI не мешает. Delivery Status — Idle, как для остальных строк.

**Ожидающий NMI.** В `State` каждого LAPIC один бит `pending_nmi` (одна «защёлка»). Запись ICR ставит его у каждого адресата; NMI, заставший бит уже взведённым (до доставки или пока доставка блокирована до `IRET`), — то же событие и теряется: запоминается не больше одного. Бит сбрасывается вместе с остальным состоянием LAPIC при INIT-reset, сбросе LAPIC и глобальном выключении (решение реализации, SDM не сверен).

**Доставка** — на границе адресата, как INIT и SIPI: `LoadedFirmwareBoard::deliver_pending_nmi(index)` (`pub(crate)`; её вызывает планировщик S6-15 на границе каждого vCPU, см. раздел «Планировщик режима чередования (S6-15)»; рабочий цикл платы по-прежнему не вызывает). Результат `NmiBoundary`: `NoPending`; `Held` — защёлка остаётся, и только по архитектурным причинам (NMI заблокирован до `IRET`, активное STI-окно `Some(Blocking)`, активность `Shutdown` или `WaitForSipi`); `Delivered` — кадр на стеке гостя, процессор в обработчике, защёлка взята. Через вектор 2 по IDT-шлюзу (`od_isa::deliver_nmi_cpl0_set`): тот же кадр из пяти qword и те же правила шлюза, что у `deliver_external_interrupt_cpl0_set`, но RFLAGS.IF не условие доставки, а `nmi_blocked` — условие. Шлюз вектора 2, который доставить нельзя (P = 0, IST ≠ 0, ошибка записи кадра), — ошибка платы `ExternalInterruptDelivery { vector: 2, .. }`: vCPU не меняется, NMI остаётся ожидающим. Отсутствие модели — не удержание, а ошибка платы `Config` до любого изменения состояния (NMI остаётся ожидающим): активность `None` («vCPU activity not modelled»), `sti_shadow = None` («STI shadow not modelled»), vCPU с состоянием реального режима (`real_mode` — `Some`: AP после SIPI или INIT; «NMI in real mode not modelled», до S6-16) и paging, которое не принимает проход страниц (в нём EFER — собственный `guest_efer` vCPU, а не константа `FIRMWARE_EFER`). AP в `WaitForSipi` по-прежнему `Held`: проверка активности идёт раньше проверки `real_mode`.

**Блокировка.** `Delivered` ставит `LongModeState::nmi_blocked`. Пока он взведён, новый NMI не доставляется (`Held`). Снимает его `IRETQ`: оба пути исполнения (`iretq_cpl0` в `execute.rs` для CPL0-набора и `step_iretq` в `iretq.rs` для локальных профилей) сбрасывают его в единственной точке фиксации, только когда `IRETQ` завершился. Отказавший `IRETQ` флаг не меняет. **Сознательное отступление от SDM:** блокировка снимается только успешным `IRETQ`; при fault самого `IRETQ` (`#GP`, `#PF`) она остаётся до следующего успешного `IRETQ`, тогда как SDM снимает блокировку NMI и при `IRET`, завершившемся fault; источник — по памяти, не сверен (SDM Vol. 3A 6.7.1, источника в репозитории нет); код не меняется. INIT-reset (`Vcpu::apply_startup`) сбрасывает `nmi_blocked` вместе с защёлкой LAPIC. `Halted` процессор принимает NMI независимо от IF: после доставки он `Running`.

**Решения владельца (ревью S6-12b, 2026-11-16).** (а) INIT-reset и `IA32_APIC_BASE.EN = 0` стирают защёлку NMI; INIT также сбрасывает `nmi_blocked`. (г) NMI, принятый в ICR, до S6-15 не доставляется рабочим циклом: доставка на границе — явный вызов `deliver_pending_nmi` (тесты); условие S6-15 — планировщик обязан вызывать её на границе каждого vCPU. (б) IST шлюза NMI — задача S6-12c в docs/plans/STAGE_6.md (обязательна до приёмки S6-18). Открытое расхождение для `Shutdown` — SMP_CONTRACT §6.

**IST шлюза NMI (S6-12c).** Шлюз вектора 2 с IST = 1..7 (Linux ставит IST 2) берёт стек из `TSS.IST[n]` по TR текущего процессора (`TR.base + 0x24 + 8·(n−1)`, правила проверки как у `TSS.RSP0`); подробности — `CPU_CONTRACT.md`, раздел NMI. IST поддержан только для NMI: `#PF`, `#GP`, `INT3` и внешний вектор через шлюз с IST ≠ 0 по-прежнему отказывают `UnsupportedIst`.

**Не входит.** IST для векторов, кроме NMI; доставка в `WaitForSipi` и `Shutdown` (в том числе `hardware_reset_only`); NMI в режимах real/protected (AP после SIPI не имеет paging для доставки, пока S6-16 не исполняет их); NMI в окне MOV SS (отдельного состояния нет: тот же `sti_shadow`); NMI от LINT1 и NMI-источники PMU; вызов из исполняющего цикла (S6-15). Тесты: `local_apic::tests::nmi_*`, `vcpu::tests::an_nmi_to_a_vcpu_waiting_for_sipi_...`, `linux_direct::tests::nmi::*`, `od-isa/tests/cpl0_profile_set.rs` (`nmi_*`), `iretq::tests::a_retired_iretq_ends_nmi_blocking_...`.

## INIT, SIPI и ожидание SIPI (S6-13)
Реализация: `local_apic.rs` (`IpiCommand`, `decode_icr`, `LocalApicHandle::take_startup`), `apic_pair.rs` (`queue_init`, `queue_sipi`, `take_startup`, `PendingStartup`), `vcpu.rs` (`Vcpu::apply_startup`, `new_ap`), `firmware_board.rs` (`apply_pending_startup`); `od_isa::CpuActivity::WaitForSipi`. Источник — SMP_CONTRACT §3–§5.

**Принимается** (запись в `ICR_LOW` после `ICR_HIGH` с physical APIC ID существующего vCPU; shorthand, logical destination и все прочие пары `(Level, Trigger)` — ошибка устройства до изменения состояния): INIT assert — Delivery `101`, вектор ровно 0, `(Level, Trigger) = (1,1)`; INIT deassert — то же с `(0,1)`; SIPI — Delivery `110`, вектор `0x00–0xFF`, `(0,0)`. Адресатом может быть и отправитель; INIT assert, адресованный BSP (vCPU 0), отказывается устройством до изменения состояния (`UnsupportedConfiguration`): INIT для BSP отложен до S6-15/S6-16, применять его пока некому. SIPI и INIT deassert для BSP принимаются (SIPI BSP игнорирует, deassert ничего не меняет). Как и для Fixed, LAPIC адресата с `IA32_APIC_BASE.EN = 0` (глобально выключенный) отказывает и INIT assert, и SIPI (deassert ничего не меняет и принимается при существующем адресате). Программное выключение через SVR (бит 8 сброшен, `EN` взведён) INIT и SIPI не мешает: они принимаются (решение владельца 2026-11-16, SDM 10.4.3 по памяти).

**Ожидающие события в LAPIC адресата.** `State::pending_init` (один бит) и `State::pending_sipi` (`Option<u8>`). Запись ICR лишь ставит событие и не меняет процессор адресата. INIT assert сбрасывает ожидающий SIPI, принятый до него; SIPI после INIT остаётся. Из нескольких SIPI ждёт первый. INIT deassert не меняет никакого состояния.

**Применение.** Адресат применяет события сам, на своей границе: `LoadedFirmwareBoard::apply_pending_startup(index)` забирает их у своего LAPIC (`take_startup`) и применяет к своему `Vcpu` (`apply_startup`), INIT раньше SIPI. Рабочий цикл его не вызывает; его вызывает планировщик S6-15 (`smp_scheduler.rs`, раздел «Планировщик режима чередования (S6-15)») на границе каждого vCPU, перед выбором.
- Забирая INIT, LAPIC адресата возвращается в INIT-reset: регистры, IRR/ISR/TMR, таймер и ожидающие события — как после сброса, LAPIC остаётся включённым, APIC ID равен индексу; Remote IRR level-маршрута IOAPIC к этому LAPIC INIT не трогает, как на железе: его снимает только EOI (решение владельца 2026-11-16).
- AP: INIT загружает `RealModeState::after_init(old_cr0)` (сохраняются только CD и NW), обнуляет гостевые ячейки `IA32_EFER` и CR2 (Table 12-1; `guest_efer = 0`, `guest_cr2 = Some(0)`), остальные ячейки (STAR, LSTAR, SYSENTER, FP) не меняет; активность — `WaitForSipi`. SIPI в `WaitForSipi` загружает `after_sipi(vector)` (CS `vector << 8`, база `vector << 12`, EIP 0) и делает AP `Running`; в любом другом состоянии SIPI игнорируется (и забирается, а не копится).
- BSP: устройство INIT не принимает (см. выше); ветка `Vcpu::apply_startup` для BSP (прежний CR0 — `FIRMWARE_CR0`, не CR0 включения; активность `Running`) достижима только прямым вызовом и проверена тестом. SIPI игнорируется.
- Включение: BSP — активность не моделируется (`Vcpu::activity()` даёт `Running`); каждый AP — `WaitForSipi` с `real_mode = after_init(0x6000_0010)` (CR0 включения), `guest_efer = 0`, `guest_cr2 = None` (читается как 0).

**Не делается.** Исполнение AP и trampoline (S6-15, S6-16): ни один путь не выбирает vCPU в `WaitForSipi`. Only the CPL0 profile set refuses non-Running activity with `NotRunning`. Other entry points do not check WaitForSipi, among them `od_isa::execute::run`, `step`, `run_with_maskable_request` and the `run_configured`-based runners (the loop checks only Halted; `!= Running` is checked only under paged CR0 profiles). The board executes only vCPU 0, so this is not reachable yet. Закрыть до S6-15: все точки входа, кроме CPL0 profile set, должны проверять `WaitForSipi`. `LongModeState` не переписывается по `real_mode` (представления real mode в нём нет). `hardware_reset_only` у `Shutdown` не моделируется: INIT возвращает AP из `Shutdown` в `WaitForSipi`. NMI.

## Планировщик режима чередования (S6-15)
Реализация: `crates/od-machine/src/smp_scheduler.rs` (`SmpScheduler`, `QuantumPolicy`, `SplitMix64`, `StopRequest`, трейт `SmpHost`, `BoardSmpHost`), `clock_owner.rs` (единственный владелец часов), `idle_arbiter.rs` (арбитр простоя), `firmware_board.rs` (`vcpu_mut`, `clock_owner_mut`, `idle_to`). Время и принятое от S4-22/S4-23 — [SMP_CONTRACT.md](SMP_CONTRACT.md) §8. Это строительный блок режима чередования: он **не подключён** к `run_linux_boot_cpl0_set` (тот цикл исполняет только BSP, а AP до S6-16 исполняться не может, см. «Не входит»).

**Цикл.** Один host-поток. Каждая итерация, на границе между шагами: (1) STOP, затем общий бюджет (0 — конец); (2) граница каждого vCPU по порядку индексов: сначала INIT и SIPI из его собственного LAPIC (`apply_pending_startup`), затем его NMI (`deliver_pending_nmi`); halted vCPU с IF = 1 и принимаемым вектором в его LAPIC будит планировщик; (3) если есть vCPU в `Running` — ход (ниже), иначе один вызов `ClockOwner::idle_step` (одно решение арбитра, не больше одного прыжка часов, без ожидания и без цикла внутри). Результаты `Finished`: `Stopped`, `BudgetExhausted`, `AllIdle` (никто не исполняется и никто не может проснуться), `ScheduleEnd`.

**Шаг и граница.** Шаг — завершённая инструкция либо один возврат интерпретатора с частичным прогрессом `REP` (RIP на начале `REP`, `RunExit::RepIterationBudgetExhausted`). Внутри инструкции переключения нет. Частичный прогресс считается шагом кванта, но не retired-инструкцией: бюджет и время (1 нс) считают только завершённые инструкции, `REP` стоит одну при завершении. Интерпретатор останавливает `REP` после `REP_ITERATION_WORK_LIMIT` = 1024 итераций, а не после каждой: шаг `REP` — срез частичного прогресса до 1024 итераций, одна итерация границей не является (L — нормативная константа версии 1 контракта, SMP_CONTRACT §8; перебор расписаний с учётом среза проверяет S6-19).

**Выбор vCPU и квант.** Выбирается следующий в `Running` по кругу, начиная с индекса после прошлого. Ход — вызовы `SmpHost::run` (существующий цикл интерпретатора) до исчерпания кванта, остановки vCPU (`HLT`, `Shutdown`) или STOP; каждый вызов режется `ClockOwner::step_budget`, чтобы не пройти ближайший срок таймера любого LAPIC (после вызова — `account_retired` и `dispatch_timer` по порядку индексов), поэтому моменты тиков не зависят от кванта. Квант режется и общим остатком бюджета. Три входа (`QuantumPolicy`):
- `Fixed(n)`, `n >= 1`; по умолчанию `Fixed(1024)` (`DEFAULT_QUANTUM`; значение не измерено, обоснования, кроме «не 1», нет);
- `Seeded { seed, min, max }`, `1 <= min <= max`; квант каждого хода — `min + SplitMix64::next_u64() % (max - min + 1)`; пределы по умолчанию `1..=256` (`SEED_QUANTUM_MIN/MAX`), ход в один шаг возможен. Генератор: SplitMix64 с состоянием `seed`: `state += 0x9E3779B97F4A7C15; z = state; z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9; z = (z ^ (z >> 27)) * 0x94D049BB133111EB; результат z ^ (z >> 31)`, умножения и сложение по модулю 2^64; любой seed, включая 0, допустим; последовательность проверена эталонными значениями для seed 0. Генератор продвигается ровно один раз за ход, только в режиме `Seeded`. Смещение остатка от деления (меньше 2^-56 для пределов выше) принято;
- `Schedule(Vec<usize>)` — явное расписание «чей шаг»: одна запись — один шаг названного vCPU, ход равен одному шагу. Названный vCPU обязан быть `Running` на этот момент, иначе `ScheduleCpuNotRunning`/`ScheduleCpuOutOfRange` без исполнения; после последней записи — `ScheduleEnd`. Простой (прыжок часов) записей не расходует. `executed_schedule()` выдаёт записанный прогон в этой же форме, (вход для S6-19). Расписание воспроизводит прогон, если хост не зависит от разрезания вызовов и за ход не ставились события границы для исполняющего vCPU. Событие, поставленное во время хода (ICR), применяется на следующей границе — в записанном и в повторном прогоне граница может разойтись. Это ограничение для S6-19.

**Пропуск.** `WaitForSipi` и `Shutdown` не исполняются никогда. `Halted` не исполняется, пока нет принимаемого вектора при IF = 1 (в IRR, не за обслуживаемым вектором или TPR; тот же тест, что `WakeNow` арбитра) либо NMI/INIT/SIPI на границе: NMI доставляется и ставит `Running`, INIT возвращает AP в `WaitForSipi`. Halted с IF = 0 и вектором остаётся пропущенным (только NMI или INIT его будят).

**Время.** Когда `Running` нет, часы одним прыжком идут к ближайшему сроку среди очередей событий всех vCPU (таймеры halted vCPU с IF = 1, чей вектор будет принят, правило арбитра) и один раз диспетчеризуют таймеры; затем цикл идёт на следующий круг, и проснувшийся vCPU получает ход. Единственный двигатель времени — `ClockOwner`; планировщик ничего другого не вызывает. `SmpHost::run` может вернуть `device_ns` (тики PM/PIT/RDTSC), планировщик начисляет их после retired. Хост часы не трогает. `device_ns` начисляется после вызова целиком; время устройства зависит от числа вызовов — хост обязан вызывать `run` так, чтобы тики не пропускались (см. тест `device_time_is_charged_after_the_retired_instructions`).

**Бюджет, счётчики, STOP.** Бюджет общий, в retired-инструкциях всех vCPU; счётчики `VcpuCounters` — на каждый vCPU (`turns`, `steps`, `retired`, `wakes`, `init_applied`, `started`, `nmi_delivered`); счётчики накапливаются между вызовами `run`. STOP читается на каждой границе и передаётся хосту предикатом остановки на каждый шаг хода; при нём не исполняется ни один vCPU. Запрос не снимается планировщиком (снимает вызывающий). Бюджет считает только retired-инструкции; частичный REP его не расходует; остановка долгого REP — STOP или предел цикла (как в Linux-цикле по D7). Ошибка границы (например, NMI в real mode) возвращается как `SmpError::Host`; счётчики этой границы при ошибке не обновляются.

Поздняя ошибка кванта возвращается вместе с выполненной частью вызова. При переходе из legacy в long mode в одном вызове хост складывает обе части. Перед возвратом `SmpError::Host` планировщик учитывает этот префикс в счётчиках и записи хода, вычитает retired из бюджета и начисляет виртуальное время. [ISSUE-527](../KNOWN_ISSUES.md#issue-527--s6-21-учёт-поздней-ошибки-в-режиме-чередования).

**Граница и порядок вызовов.** INIT/SIPI до NMI (INIT приоритетнее NMI; INIT сбрасывает защёлку NMI). Это единственное место, где вызываются `apply_pending_startup` и `deliver_pending_nmi`. Событие, поставленное во время хода (запись ICR для другого или для того же vCPU), применяется на следующей границе, после хода; хост не завершает вызов из-за такого события (иначе прогон зависит от хоста). Это допустимая задержка доставки IPI/NMI, но грубее аппаратной (для NMI самому себе).

**Хост платы (`BoardSmpHost`).** Берёт `LoadedFirmwareBoard` с включённым `enable_clock_owner` (иначе `Config`), даёт активность из `Vcpu::activity()` и IF из модели, на которой идёт vCPU (`Vcpu::interrupts_enabled`: `eflags` legacy-модели или `state.rflags`), LAPIC из `local_apic_of`, границу из двух методов платы и владельца часов платы. Замыкание `run` не должно вызывать `charge_retired` / `charge_ns` / `idle_to` / методы `ClockOwner`: часы двигает только планировщик (условие подключения в S6-16/S6-17). Квант исполнения — замыкание вызывающего: это существующий цикл платы, который отдельный срез вынесет из `run_linux_boot_cpl0_set`; копировать его здесь нельзя. vCPU в legacy-режиме (`Vcpu::legacy` — `Some`: AP после SIPI или INIT) хост исполняет не замыканием `run`, а исполнителем S6-16c (см. [раздел ниже](#s6-16c--исполнитель-ap-в-legacy-режиме-на-плате)); до S6-16c он отказывался (`Config`, «real mode cannot run yet»). `idle_to` платы теперь строит виды арбитра из `Vcpu::activity()` и IF каждого vCPU (раньше: BSP halted, остальные `WaitForSipi`).

**Не входит.** Подключение к `run_linux_boot_cpl0_set` и paged-пробам (цикл BSP-only: `page_table_root` общий на плате, источник IRQ привязан к BSP; paged probe не использует планировщик); исполнение AP (с S6-16c — только trampoline, см. раздел S6-16c); проверки `WaitForSipi` в точках входа `od_isa::execute` вне CPL0-набора (планировщик их не вызывает, но сами точки не проверяют); `page_table_root` в `Vcpu`; запись `IA32_APIC_BASE.EN` из гостя (WRMSR EN), `hardware_reset_only`, синхронизация `legacy` CR0 с `state.cr0` (после S6-16c состояние AP либо legacy-модель, либо `state`, не оба); потоки, приоритеты, «честность» (нет замера); параллельный режим. Тесты: `smp_scheduler::tests::*`, `firmware_board::tests::idle_to_shows_every_vcpu_with_its_own_activity`.

## S6-16a — индекс vCPU в MMIO и LapicIrqSource

Реализация: `bus.rs` (`MachineBusSetStepIo::vcpu`, `MachineBus::split_for_set_step_of_vcpu`), `paged_memory_adapter.rs` (`AdapterBus::RamMmio(ram, mmio, vcpu)`, параметр `vcpu` у `run_cpl0_set_paged_2m_ram_with_cache`), новый `lapic_irq_source.rs` (`LapicIrqSource`). Дополняет раздел [LAPIC на каждый vCPU (S6-11)](#lapic-на-каждый-vcpu-s6-11): там индекс vCPU нёс только `AdapterBus::Machine`; путь MMIO CPL0-цикла был процессор 0.

**Что изменено.**
- `MachineBusSetStepIo` хранит `vcpu`; его `dispatch` обращается к устройству через `read_from_cpu(vcpu, …)`/`write_from_cpu(vcpu, …)`. Для устройств, не различающих процессоры (все, кроме `LocalApic`), это прежние `read`/`write` (так устроены реализации по умолчанию `BusDevice`).
- `MachineBus::split_for_set_step()` остаётся и означает процессор 0; `split_for_set_step_of_vcpu(vcpu)` — цикл vCPU `vcpu`.
- `AdapterBus::RamMmio` несёт индекс vCPU; `AdapterBus::vcpu()` возвращает его (форма `Ram` — по-прежнему процессор 0). Окно `0xFEE0_0000` из цикла CPL0-набора обслуживается как окно того vCPU, который исполняется: чтение `0x20` даёт `index << 24`, запись ICR/EOI/SVR действует на его LAPIC.
- `run_cpl0_set_paged_2m_ram_with_cache(state, bus, vcpu, cr3, …)` получил параметр `vcpu` (после `bus`). `_observed`, `_observed_with_sweep` и плата `run_linux_boot_cpl0_set` передают `BOOT_VCPU_INDEX`.
- `LapicIrqSource(LocalApicHandle)` — `BoardIrqSource` LAPIC одного vCPU. `pending_vector` выбирает старший допустимый вектор через `eligible()`. После успешной доставки через IDT `acknowledge` переводит доставленный вектор из IRR в ISR. Более старший вектор, поставленный в очередь во время записи кадра в гостевой стек, остаётся в IRR. EOI гостя снимает ISR своего LAPIC. `next_deadline_ns` = `None` (сроки таймера LAPIC учитывает `ClockOwner`), `wake_ns` и остальное используют значения по умолчанию.

**Что не меняется для vCPU 0.** Все существующие вызовы передают `BOOT_VCPU_INDEX`; `read_from_cpu(0, …)` у `LocalApic` равно `read`, у остальных устройств — тоже `read`. Окна, которые обслуживает цикл (ECAM, при профиле APIC — LAPIC и IOAPIC), не менялись; прочие MMIO — прежняя типизированная ошибка. Источник `PitIoapicIrqSource` платы не менялся.

**Ограничения.**
- Плата по-прежнему исполняет только процессор 0 (`run_linux_boot_cpl0_set` не подключён к планировщику S6-15): `vcpu != 0` в цикл передают только тесты и будущий исполнитель S6-16. Состояние AP (CS/SS-кэши, IDT) вызывающий готовит сам.
- Индекс должен быть `< vcpu_count` платы; иначе доступ к LAPIC — `UnsupportedConfiguration` как в S6-11 (обращение цикла к окну завершится ошибкой устройства).
- Разделяемый `PendingIrq`/IOAPIC-линии для AP не подключены: `LapicIrqSource` видит только то, что уже в IRR своего LAPIC (самоIPI, IPI от другого vCPU, вход IOAPIC, адресованный этому APIC ID, таймер после `dispatch_timer`).
- `wake_ns` не реализован: пробуждение vCPU из `HLT` остаётся за планировщиком S6-15.
- Страничный размер таблиц: используется тот же обход, что и у процессора 0; отдельных проверок для AP не добавлено.

## S6-16c — исполнитель AP в legacy-режиме на плате

Реализация: новый `ap_legacy_exec.rs` (`run_legacy`, `BoardRam`, `ApLegacyRefusal`), `vcpu.rs` (`LegacyCpu`, `Vcpu::legacy`, `Vcpu::real_mode()`, `Vcpu::interrupts_enabled()`, `init_cpu`, `AP_INIT_EDX`), `smp_scheduler.rs` (`BoardSmpHost::run`), `firmware_board.rs` (`FirmwareBoardError::ApLegacyRefused`). Модели — `od-isa`: `RealModeCpu`, `ProtectedCpu`, `ProtectedCpu::into_long_mode` (S6-16b, [CPU_CONTRACT](CPU_CONTRACT.md)). Дополняет [планировщик S6-15](#планировщик-режима-чередования-s6-15) и [INIT/SIPI (S6-13)](#init-sipi-и-ожидание-sipi-s6-13).

**Состояние vCPU.** `real_mode: Option<RealModeState>` заменено на `legacy: Option<LegacyCpu>`, `LegacyCpu = Real(RealModeCpu) | Protected(ProtectedCpu)`. AP после включения питания и после INIT — `Real(init_cpu(old_cr0))`: `RealModeState::after_init(old_cr0)` (EIP `0xFFF0`, CS-база `0xFFFF0000`, CD/NW старого CR0), все регистры 0, кроме EDX, таблицы дескрипторов сброса. SIPI меняет только `mode` (`after_sipi(vector)`: `CS = vector << 8`, `EIP = 0`). `legacy == None` у BSP до первого INIT и у AP, вошедшего в long mode. Пока `legacy` — `Some`, `state` не процессор: меняется только `activity` и `nmi_blocked`. `Vcpu::real_mode()` отдаёт `RealModeState` (`Some` только в модели real mode) — ожидания тестов S6-13 прежние. INIT заменяет `legacy` на `Real(init_cpu(old_cr0))` из любой модели (старый CR0 — `legacy.cr0()`; при `legacy == None` — `state.cr0` или `FIRMWARE_CR0`, как для BSP раньше).

**EDX после INIT.** `AP_INIT_EDX` = `CPUID.01H:EAX` профиля `DiagnosticV1` платы (`0x600`): по SDM Vol. 3A Table 12-1 EDX после INIT — сигнатура процессора, и гость, читающий `CPUID` после сброса, видит то же значение. Оракульные прогоны S6-14d берут значение хоста `0x306A9` как вход; у профилей платы такого значения нет, поэтому оно не используется. Выбор записан здесь.

**Открыто (CPUID legacy-моделей и EDX).** Источник CPUID legacy-моделей — `CpuidSource::Profile(DiagnosticV1)` по умолчанию (`RealModeCpu::after_sipi` ставит его сам): он НЕ следует за профилем платы, поэтому «EDX после INIT следует за профилем платы» в этом срезе неверно, если профиль платы не `DiagnosticV1`. Для `verify_cpu` реального ядра нужен профиль платы. Код не менялся; решено в S6-18a (раздел «S6-18a — CPUID на каждом vCPU и профиль AP»).

**Исполнение (`run_legacy(board, cpu, budget, stop)`).** Перед каждым шагом, в этом порядке: (1) модель real mode с `cs_protected` и `CR0.PE` становится `ProtectedCpu::from_real_entry`; (2) `ProtectedCpu::target_entered` — передача: `into_long_mode(&vcpu.state)` даёт `LongModeState`, `guest_efer = cpu.efer`, `state.cr3 = Some(cr3 модели)` (CR3 trampoline, не `page_table_root` платы), `legacy = None`; (3) `stop()`; (4) бюджет исчерпан — конец вызова. Переходы (1) и (2) не стоят шага, поэтому вызов, которому достался процессор после уже исполненного 64-битного прыжка, передаёт его сразу. Затем один шаг `RealModeCpu::step` или `ProtectedCpu::step`. Каждый шаг — одна исполненная инструкция (`retired`); `device_ns` 0, часы вызов не трогает. Любой отказ модели — `FirmwareBoardError::ApLegacyRefused { cpu, refusal }`, `refusal: ApLegacyRefusal = Real(RealModeRefusal) | Protected(ProtectedRefusal) | ProtectedEntry(ProtectedEntryRefusal) | LongEntry(LongEntryRefusal)`; модель и память отказавшим шагом не меняются, vCPU остаётся в своей модели `Running`. Исполненные до отказа шаги того же вызова остаются в vCPU, но не попадают ни в счётчик планировщика, ни в часы (ошибка заменяет `Executed` вызова, планировщик ничего за неё не списывает; ошибка завершает прогон, и часы и счётчики ошибочного прогона короче на шаги последнего вызова). Прочие существующие варианты `FirmwareBoardError` не менялись.

**Память (`BoardRam`).** Адаптер `RealModeMemory` над `PhysicalMemory` платы: байт читается и пишется только в области вида RAM (`locate_region_kind == Ram`); MMIO, ROM, framebuffer и дыра — `None`/`false`, то есть отказ модели (`DataUnreadable`, `FetchUnreadable`, `DataUnwritable`, `DescriptorUnreadable` …), а не обращение к устройству и не паника. Записи модели (операнд `LOCK BTS`, бит accessed дескриптора, A/D записей страничных таблиц) идут через `PhysicalMemory::write_bytes` — тот же отслеживаемый путь, что и у гостевых записей: поколение каждой записанной страницы растёт (тест `the_models_writes_advance_the_page_generations_like_a_guest_store`).

**Планировщик (`BoardSmpHost::run`).** vCPU с `legacy == None` — прежний путь: замыкание `run`. vCPU с `legacy` — `run_legacy`; если вызов передал vCPU в long mode (`entered_long_mode`) и от бюджета осталось, то в этом же вызове замыкание `run` исполняет остаток; `Executed` складывается (`retired` и `device_ns` суммируются, `partial_rep` — от замыкания). Так вызов не возвращает ноль шагов при `Running` (`SmpScheduler::turn` вернул бы `NoProgress`): ноль шагов возможен только при `stop()`, который `turn` различает. Замыкание для long mode — то же, что для любого vCPU; его подключение к плате (квант по индексу vCPU с собственным CR3) — S6-16d. `if_set` читает `Vcpu::interrupts_enabled()`: `eflags` модели (у real mode IF не моделируется — всегда 0) или `state.rflags`.

**NMI.** NMI к AP с `legacy == Some(_)` — ошибка `Config("deliver_pending_nmi: NMI in real mode not modelled")` (условия подключения, п. 6(д) раздела S6-16 плана; доставка NMI через IVT не входит; текст прежний и для protected mode), NMI остаётся ожидающим. К AP в long mode (`legacy == None`) работает путь доставки по границе; обход строится по CR3 самого vCPU (`state.cr3.unwrap_or(page_table_root)`, как в кванте S6-16d), поэтому NMI к AP на собственном CR3 доставляется. До S6-16f обход строился по `page_table_root` платы и для AP со своим CR3 возвращал `Config("deliver_pending_nmi: no paging for the vCPU")` (`Cr3Mismatch`); исправлено в S6-16f. Для BSP Linux (CR3 равен `page_table_root`) поведение не изменилось.

**Не входит.** Прерывания в legacy-режиме; `HLT` и остальные инструкции вне форм S6-14a (отказ); устройства (порт и MMIO в моделях нет); подключение к `run_linux_boot_cpl0_set`; квант платы для любого vCPU и собственный CR3 в нём (S6-16d); образ probe и `--stage6-smp-probe` (S6-16e); обмен IPI, повторный SIPI работающему AP, IPI на несуществующий APIC ID (S6-16f). Прямой вызов `Vcpu::apply_startup` с INIT к BSP выставляет ему `legacy` (BSP остаётся `Running`); через LAPIC это недостижимо, INIT к BSP отказывает (S6-13). Тесты: `ap_legacy_exec::tests::*` (14 в S6-16c; в S6-16d их 15: добавлен обмен IPI после SIPI), `smp_scheduler::tests::init_and_sipi_are_applied_on_the_boundary_of_the_target_and_the_started_vcpu_runs_in_real_mode`, `vcpu::tests::*` (S6-13, ожидания прежние, обращение к `real_mode` — через метод).

## S6-16d — квант платы для любого vCPU

Реализация: новый `crates/od-machine/src/firmware_board/vcpu_quantum.rs` (дочерний модуль `firmware_board`: узкая точка доступа к приватным полям платы, чтобы не раздувать `firmware_board.rs`), `LoadedFirmwareBoard::run_vcpu_set_quantum(cpu, budget, stop, set) -> Result<Executed, FirmwareBoardError>`. Рядом: `vcpu.rs` (`Vcpu::skip_first_boundary`), `apic_pair.rs` и `local_apic.rs` (`CpuApic::timer_writes`, `LocalApicHandle::timer_register_writes`), `smp_scheduler.rs` (`SmpHost::board_deadline`, `debug_assert` в `turn`), `firmware_board.rs` (`pump_uart_transmit` вынесена из цикла Linux, `irq_deadline_ns`, `FirmwareBoardError::VcpuQuantum`, `idle_to` читает `Vcpu::interrupts_enabled()`). Дополняет [планировщик S6-15](#планировщик-режима-чередования-s6-15), [S6-16a](#s6-16a--индекс-vcpu-в-mmio-и-lapicirqsource) и [S6-16c](#s6-16c--исполнитель-ap-в-legacy-режиме-на-плате).

**Что делает вызов.** Один вызов цикла CPL0-набора `run_cpl0_set_paged_2m_ram_with_cache` (S6-16a) для vCPU `cpu` в long mode; код Linux-обвязки (NVMe, PM, контрольные точки, наблюдения, кеш выборки) не копируется. Берёт у vCPU, а не у платы: CR3 — `Vcpu::state.cr3` (`page_table_root` платы не читается и не пишется; без CR3 — `Config("vcpu quantum: the vCPU has no CR3")`), EFER — `Vcpu::guest_efer` (не константа `FIRMWARE_EFER`: у AP после trampoline это `0xD01` — SCE, LME, LMA, NXE, у платы `0xD00`; это снимает замечание minor 7 ревью S6-16c), CR2 — ячейка `Vcpu::guest_cr2` (в состояние на время вызова, обратно после него, как в цикле Linux). vCPU с `legacy == Some(_)` — `Config("vcpu quantum: the vCPU is in a legacy mode")`: его исполняет `run_legacy`. MSR- и FP-ячейки — вызывающего: набор `set` строится над ячейками этого vCPU (загрузка из `Vcpu::guest_*` перед вызовом и сохранение после — как `linux_direct` для BSP; хелпер этой работы — S6-21, вне S6-17). RDTSC в наборе нет: у гостя AP нет источника времени.

**Условия подключения планировщика (п. 6 раздела S6-16 плана).**
- (а) срок платы: `SmpHost::board_deadline() -> Option<u64>` (по умолчанию `None`; `BoardSmpHost` отдаёт `next_deadline_ns` источника прерываний платы, `LoadedFirmwareBoard::irq_deadline_ns`). `SmpScheduler::turn` передаёт его в `ClockOwner::step_budget` каждого вызова хода, `idle_step` — в `board_deadlines` арбитра. Срок платы ограничивает ходы (`step_budget`, `idle_step`); снятие срока платы на границе хоста (доставка источника платы BSP, пробуждение по нему) — реализовано в S6-21 шаге 1 (раздел «S6-21 шаг 1» ниже; текст этого пункта ниже описывает состояние на момент S6-16d). Сейчас `BoardSmpHost::boundary` вызывает только `apply_pending_startup` и `deliver_pending_nmi`, `run_vcpu_set_quantum` ставит хуком только `LapicIrqSource` своего LAPIC, поэтому `irq_source` платы (PIC/IOAPIC/PIT) не доставляется никому, в том числе vCPU 0, а wake-view смотрит только на `eligible()` LAPIC. Следствия для реального `BoardSmpHost`: работающий vCPU после срока навсегда ходит по одному шагу; если все vCPU в `HLT`, прыжок простоя до срока даёт `Ok(None)`, следующий круг — `SmpError::IdleNoProgress`. Модель снятия срока есть только в тестовом `Rig` (`smp_scheduler::tests`): прыжок простоя, дошедший до срока, кончается `NoProgress` при `idle_ns > 0` — это не ошибка; тот же ответ без прыжка (срок не снят) — `SmpError::IdleNoProgress`.
- (б) вызов кончается после инструкции, которая записала THR COM1 (байты, прошедшие передачу, переносятся в `com1_log` функцией цикла Linux `pump_uart_transmit`, не копией), записала регистр таймера СВОЕГО LAPIC (`0x320`, `0x380`, `0x3E0`: принятая запись увеличивает счётчик `timer_writes`; новый срок может быть ближе остатка кванта), или прочитала таймер PM / порт PIT (их наносекунды — `Executed::device_ns`, после вызова). Тики, поставленные до вызова, отбрасываются (пришли не от гостя), байт THR другого vCPU переносится в журнал до вызова.
- (в) вызов не трогает часы: ни `charge_retired`, ни `charge_ns`, ни `idle_to`. `SmpScheduler::turn` проверяет это `debug_assert_eq!` (`now_ns` до и после `host.run` равны); существующие тесты хоста его не нарушают.
- (г) `Vcpu::skip_first_boundary`: вызов, кончившийся на `RepIterationBudgetExhausted` (`Executed::partial_rep`), оставляет флаг, следующий вызов этого vCPU передаёт его в `IrqBoundaryHook::skip_first_boundary` (внешнее прерывание в середине REP не доставляется на первой границе); флаг записывается в конце каждого вызова (следующий вызов без частичного REP снимает его), INIT его сбрасывает. Для N = 1 цикл Linux прежний: он держит свой локальный `partial_rep`.
- (е) доставка Fixed-векторов в AP: `IrqBoundaryHook` с `LapicIrqSource` СВОЕГО LAPIC (S6-16a): вектор из IRR идёт через IDT этого vCPU, EOI снимает ISR только его LAPIC; пробуждение из `HLT` — `wake_ready` планировщика (IF = 1 и `eligible()`), доставка — на первой границе следующего вызова.
- (д) NMI к vCPU в legacy-режиме — по-прежнему ошибка границы (S6-16c); NMI к AP в long mode со своим CR3 — доставляется по CR3 vCPU (реализовано и исправлено в S6-16f).

**Выходы вызова.** Нормальные: бюджет (`InstructionBudgetExhausted`), `stop()` или условие устройства выше (`StopRequested`), `HLT` (`state.activity = Halted`), частичный REP. Любой другой выход цикла (исключение, `#PF`, не принятый гостевым IDT, неподдерживаемая инструкция, отказ конфигурации, недоставленное прерывание) — ошибка с сохранённым состоянием: `FirmwareBoardError::VcpuQuantum { cpu, detail }` (`detail` — `Debug` выхода, `page_fault` и `memory_error`) либо `ExternalInterruptDelivery`. `Executed { retired, partial_rep, device_ns }`.

**Ожидания тестов — по SDM (Vol. 3A 11.6 ICR и IPI, 11.8.5 EOI, 11.5 таймер LAPIC, 4.5–4.7 PAE/IA-32e и код ошибки `#PF`; по памяти), не вывод модели.** Fixed IPI в IRR получателя; EOI гостя снимает ISR только своего LAPIC; обработчик AP отвечает IPI; счётчики обработчиков равны числу IPI; `NXE = 0` делает бит 63 записи таблицы зарезервированным (код ошибки `P | W | RSVD = 0b1011`); при `NXE = 1` тот же бит — запрет исполнения страницы, запись в неё допустима.

**Не входит.** Перевод `run_linux_boot_cpl0_set` на планировщик (цикл BSP по-прежнему исполняет только vCPU 0); NVMe doorbell, запрос остановки PM и источник RDTSC в кванте AP; сборка `set` над ячейками vCPU (хелпер — S6-21, вне S6-17); NMI к AP в long mode со своим CR3 (на момент S6-16d обход шёл по `page_table_root`; исправлено в S6-16f); образ probe (S6-16e); обмен IPI с отрицательными случаями (S6-16f); кеш выборки (выключен). Для AP по-прежнему верно открытое про CPUID и EDX (раздел S6-16c).

**RDTSC в поточном режиме (S6-37).** Если плата собрана с `linux_profile` и набор допускает `RDTSC`, `run_vcpu_set_quantum_shared` отдаёт снимок общих часов платы в момент чтения. Завершённое чтение заканчивает квант и добавляет 1 000 нс к `Executed.device_ns`. Планировщик начисляет это время через `ClockOwner` после выполненных инструкций. Запрос на тик принадлежит текущему вызову потока. Эксклюзивный квант S6-16d по-прежнему без источника RDTSC. [Проверки S6-37](../reports/STAGE_6_REPORT.md#s6-37--rdtsc-в-поточном-кванте).

**ACPI PM в поточном режиме (S6-37).** Успешный вызов `run_vcpu_set_quantum_shared` передаёт сохранённый S5 или reset в `QuantumCall.machine_stop`. Наблюдение запроса на границе завершает квант до следующей гостевой инструкции. Планировщик начисляет выполненный префикс вызова перед публикацией `GuestStop`. Причина остаётся в устройстве по контракту ACPI PM. [Проверки S6-37](../reports/STAGE_6_REPORT.md#s6-37--acpi-pm-в-поточном-кванте).

**ACPI PM при старте поточного прогона (S6-37).** При сохранённом S5 или reset планировщик публикует `GuestStop` до старта рабочих потоков. Этот исход имеет приоритет над состоянием vCPU, гостевым бюджетом и ожидающей работой NVMe. Счётчики исполнения начинаются с нуля, очереди устройств и гостевое время сохраняются. [Проверки S6-37](../reports/STAGE_6_REPORT.md#s6-37--acpi-pm-при-старте-потоков).

**NVMe в поточном режиме (S6-37).** На плате с NVMe поточный квант прерывается после записи MMIO. Рабочий поток обслуживает команды контроллера через общую RAM, опрашивает STOP между командами и публикует поколения страниц при DMA-записи. При прерванном обслуживании `QuantumCall.nvme_pending` остаётся за потоком до следующего вызова. Такой вызов обслуживает устройство без выполнения гостевых инструкций, в том числе при нулевом бюджете. Планировщик публикует `BudgetExhausted` после расчёта бюджета и завершения всех начатых квантов и отложенных запросов NVMe. Причины остановки от гостя и ошибки сохраняют приоритет. [Проверки S6-37](../reports/STAGE_6_REPORT.md#s6-37--nvme-в-поточном-кванте).

**NVMe при старте поточного прогона (S6-37).** Если контроллер уже имеет doorbell, стартовое обслуживание резервируется за BSP до создания рабочих потоков. BSP делает вызов обслуживания перед проверкой состояния vCPU, включая legacy и спящий vCPU. После вызова планировщик учитывает бюджет и обрабатывает доступные IRQ на обычной границе. [Проверки S6-37](../reports/STAGE_6_REPORT.md#s6-37--nvme-при-старте-в-простое).

**NVMe INTx в поточном режиме (S6-37).** Общий источник проверяет линию после успешного кванта перед выпуском рабочего потока к следующей гостевой инструкции. Состояние BSP с `IF=0` допускает доставку на другой vCPU по маршруту IOAPIC. Занятый источник или контроллер откладывает проверку. После STOP рабочий поток продолжает проверку через `nvme_pending`, в том числе при нулевом бюджете. [Проверки S6-37](../reports/STAGE_6_REPORT.md#s6-37--nvme-intx-на-ap).

**PIT через IOAPIC в поточном режиме (S6-37).** Поточный арбитр предлагает `ClockOwner` дедлайн для спящего AP с `IF=1`, если установленный источник PIT может доставить фронт в его LAPIC. При выдаче кванта AP рабочий поток ограничивает его ближайшим дедлайном PIT. Следующая выдача использует актуальное состояние таймера. После начисления времени или скачка из простоя источник применяет готовые фронты по текущему маршруту. Успешная запись AP в порты PIT завершает квант при установленном источнике PIT через IOAPIC. [Проверки S6-37](../reports/STAGE_6_REPORT.md#s6-37--pit-на-ap).

**Тесты.** `firmware_board::vcpu_quantum::tests::*` (10): обмен Fixed IPI с EOI (две vCPU, три круга), пробуждение из `HLT`, запись в регистры таймера (0x320, 0x380, 0x3E0) кончает вызов и часы не двигаются, срок платы режет вызов любого vCPU, один vCPU совпадает с прямым вызовом цикла (пять политик), одинаковый seed — одинаковые счётчики, расписание и время, vCPU на своём CR3, EFER vCPU (`0xD01` против `0x501` с NXE = 0), запись THR кончает вызов, `skip_first_boundary` на частичном REP (с контрастом без флага); `ap_legacy_exec::tests::two_vcpus_exchange_fixed_ipis_after_the_ap_was_started_by_sipi_from_guest_code` (BSP — harness в long mode по плану 4.4, AP стартует INIT и SIPI из гостевого кода BSP и проходит trampoline); `smp_scheduler::tests::*` (3 новых: срок платы режет первый вызов хода; прыжок простоя до срока не ошибка, пока хост его снимает; не снятый срок — `IdleNoProgress`). Исправления тестов S6-16c: `long_quantum` теперь вызывает `run_vcpu_set_quantum` (EFER `0xD01`, не `0xD00`; ожидания не изменились: SCE в этих программах не исполняется), `the_state_at_the_hand_over_…` берёт `preserved` до прогона и сверяет сохранённые поля (`kernel_gs_base`, `x87`, `sse`).

## S6-16e — probe v1 (старт AP)

Дата: 2026-11-17. Чекбокс S6-16 в `STAGE_6.md` не отмечен. Реализация: `crates/od-machine/src/stage6_smp_probe.rs` (загрузчик и запуск), `tools/build/build_stage6_smp_probe.py` (сборщик образа, байты закодированы вручную, ни ассемблера, ни компилятора, ни компоновщика), тесты `crates/od-machine/tests/stage6_smp_probe.rs` и `tools/build/test_build_stage6_smp_probe.py`. Опирается на исполнитель AP (S6-16c), квант платы (S6-16d) и планировщик (S6-15). Методика запуска — [SMP.md](../testing/SMP.md) («probe v1»); формы инструкций — [CPU_CONTRACT.md](CPU_CONTRACT.md) («S6-16e»).

**Образ.** `Stage6SmpProbeV1`: 0xB000 байт по GPA `0x2000..0xD000`, воспроизводим (SHA-256 двух сборок совпадает; манифест тоже). Состав: таблицы страниц (PML4 `0x2000`: тождественные первые 2 MiB большой страницей, окно Local APIC `0xFEE00000` страницей 4 KiB), информационная страница `0x7000` (в образе нули), trampoline AP по `0x8000` (вектор SIPI `0x08`) с ячейкой «AP вошёл» `0x8100` (операнд `LOCK BTS`), GDT `0x9000` (null, 32-битный код, 64-битный код, данные), строки `0x9100`, код BSP `0xA000`, код AP в long mode `0xB000`, страница ячеек `0xC000` (флаг «AP поднят» `0xC008`, счётчик стартов `0xC010`, маркер BSP `0xC040`, маркер AP `0xC080`; 64 байта: `magic u32 "MARK"`, регистр APIC ID как его прочитал процессор, 8 резерв, nonce 16..64).

**Информационный блок** (пишет загрузчик, `0x7000`, 64 байта): `magic u32 "SMP6" = 0x36504D53`, `version u32 = 1`, `vcpu_count u32`, `nonce_len u32`, nonce в байтах 16..64 (ASCII `0x21..=0x7E`, 1..=47 байт, дальше нули, один NUL завершает строку). Nonce в образе нет: один образ служит любому nonce и любому числу процессоров; тест проверяет, что nonce в байтах образа не встречается.

**Загрузчик** `load_stage6_smp_probe(image, vcpu_count, nonce)`: плата по умолчанию с `vcpu_count` (1..=4), окна MMIO Local APIC включены, владелец часов включён, `page_table_root` платы = PML4 образа; образ и блок пишутся через `patch_physical_ram`; BSP (vCPU 0) — long mode, CPL0, IF = 0, `CR3` = PML4, `EFER` = `0xD01`, RSP = `0xE000`, CS = `0x10`, SS = `0x18` по GDT образа. AP не получают ничего: они в `WaitForSipi`, пока их не запустит ICR из гостя; свой `CR3` AP берёт из trampoline (S6-16c). Отказы: размер образа, число vCPU, nonce (`Stage6SmpProbeLoadError`). `Stage6SmpProbe::run(quantum, budget)` строит набор CPL0 (CLI, STI, HLT, PIO u8, `MOV CR3` во владении vCPU), `BoardSmpHost` с `run_vcpu_set_quantum` и `SmpScheduler`; возвращает транскрипт COM1, счётчики планировщика (retired, init_applied, started), активность и признак `legacy` каждого vCPU. Ошибка платы (в том числе `ApLegacyRefused` и `VcpuQuantum`) приходит вместе с байтами COM1, записанными до неё.

**Протокол** (план S6-16 п. 1–3; SDM Vol. 3A 11.4.4 и 10.6):
1. BSP читает блок (magic, version: иначе `SMP PROBE FAIL INFO`), включает свой LAPIC (SVR = `0x1FF`), пишет маркер (регистр ID `0xFEE00020` читает MMIO-доступом, nonce копирует из блока), печатает `BSP<id> UP <nonce>`.
2. `vcpu_count == 1`: печатает `SMP PROBE AP ABSENT <nonce>` и `CLI; HLT`. Успеха на одном процессоре нет.
3. Иначе: ICR_HIGH = `1 << 24`; ICR_LOW INIT assert `0xC500`, INIT deassert `0x8500` (no-op по SMP_CONTRACT §5); W = 64 итерации с проверкой, что ячейка «AP вошёл» = 0 (иначе `SMP PROBE FAIL EARLY <nonce>`); SIPI `0x0608`; ожидание флага «AP поднят» не более T = 65536 итераций (иначе `SMP PROBE FAIL TIMEOUT <nonce>`).
4. AP: trampoline (формы S6-14a) → long mode; включает свой LAPIC, читает ID (`1 << 24` у AP 1), пишет маркер, увеличивает счётчик стартов, печатает `AP<id> UP <nonce>`, поднимает флаг, `STI; HLT`.
5. BSP после флага: счётчик == 1 (иначе `FAIL COUNT`), маркер AP (magic, ID `1 << 24`; иначе `FAIL MARKER`), `SMP PROBE OK <nonce>`, `CLI; HLT`.
Порядок строк UART не зависит от расписания при квантах ≤ ~300 000 шагов: AP печатает до флага, BSP печатает итог после него. При большем кванте BSP успевает исполнить ожидание флага (около 327 тыс. шагов) раньше, чем AP напечатает строку, и probe даёт `FAIL TIMEOUT` (ложного `OK` нет; см. «Ограничения»).

**Манифест** (`build_manifest.json`, файлы `expected_uart_n1.txt`, `expected_uart_n2.txt` с `{nonce}`): ожидаемая расшифровка UART и ожидаемые ячейки выведены из протокола и SDM (ICR: SDM 10.6.1; ID в битах 31:24: 10.4.6), а не из прогона. Для варианта `no_long_mode` манифест ожидает ошибку `ApLegacyRefused` и файла UART нет.

**Варианты-поломки** (`--variant`): `no_long_mode` — trampoline после загрузки сегментов не включает PAE/EFER/paging и не прыгает в 64-битный сегмент, а выполняет `HLT`; по SDM AP остановился бы в protected mode, а BSP получил бы таймаут; исполнитель S6-16c знает только формы S6-14a, поэтому отказывает: `ApLegacyRefused { cpu: 1, refusal: Protected(_) }` (тест проверяет это и то, что до отказа в UART есть только строка BSP). `no_flag` — AP не поднимает флаг: BSP печатает `FAIL TIMEOUT` (строка AP на месте; тест проверяет `AllIdle`, флаг = 0, счётчик = 1 и маркер AP).

**Тесты** (`tests/stage6_smp_probe.rs`, 9; образ строит настоящий сборщик через `python3`, без пропуска при его отсутствии): N=2 — три строки UART в манифестном порядке, ячейки (LOCK = 1, флаг = 1, счётчик = 1; значения читаются из манифеста), маркеры BSP (ID 0) и AP (ID `1 << 24`) с nonce, `init_applied = 1`, `started = 1`, AP вышел из legacy-модели, оба процессора в `Halted`; тот же результат (транскрипт, LOCK, флаг, счётчик, маркеры BSP и AP по манифесту) под квантами 1, 2, 7, 100, 100000 и 16 seed (0..16); N=1 — `AP ABSENT` с nonce, никакого `OK`, ячейки AP нулевые; `no_flag` — `FAIL TIMEOUT`; `no_long_mode` — `ApLegacyRefused`; ячейка «AP вошёл» = 1 до запуска (записывает тест) — `FAIL EARLY`; испорченный magic блока — `FAIL INFO`; другой nonce меняет только транскрипт; отказы загрузчика. Сборщик: 12 тестов Python (воспроизводимость, константы совпадают с Rust, значения ICR, байты trampoline, формы кода, варианты).

**Ограничения (записаны, не обойдены).**
- `LIDT` в long mode в наборе есть только как `[RSP+disp8]` (`PagedLidtLong64V1`): IDTR AP загружает trampoline в real mode (`66 0F 01 /3`) с пределом 0, шлюзов нет. Пока вектор не доставляется, `STI; HLT` работает; Fixed IPI к такому AP — S6-16f (нужен IDT со шлюзами в образе и IDTR trampoline на него). **Снято в S6-16f**: образ v2 несёт IDT, IDTR trampoline указывает на неё.
- `OUT DX, AL` в COM1 исполняется; регистр LSR не опрашивается: вызов кванта кончается после записи в THR и передатчик опустошается (S6-16d), так что THRE = 1 на начало каждой инструкции. Это свойство модели, не гарантия реального 16550.
- Проверка «AP до SIPI ничего не исполнил» осмысленна не при любом кванте: ICR-события применяются на границе после хода BSP (S6-15), поэтому при большом кванте INIT и SIPI применяются вместе; ветка `FAIL EARLY` проверена записью ячейки со стороны теста, а не гонкой.
- Nonce не длиннее 47 байт и без пробела и NUL.
- **Граница кванта (измерено ревью, N=2, nonce `ab`, retired [BSP, AP]).** `Fixed(100)` → OK `[825, 195]`; `Fixed(100000)` → OK `[100125, 195]`; `Fixed(300000)` → OK `[300125, 195]`; `Fixed(400000)` и `Fixed(1000000)` → `FAIL TIMEOUT` (строка `AP1 UP` печатается после итога; BSP `[328357, 195]`). BSP исполняет около 327 тыс. шагов на ожидании флага `WAIT_UP = 0x10000` при любом кванте. ОГРАНИЧЕНИЕ: порядок строк UART и OK детерминированы при квантах ≤ ~300 000 шагов; при `Fixed` ≥ ~327 000 (и при любом кванте, где BSP успевает исполнить ожидание флага) probe даёт `FAIL TIMEOUT` — ложного OK нет; устранение — поднять `WAIT_UP` с обоснованием через максимальный квант политики (S6-16f). `WAIT_UP` в v1 не меняется. **Снято в S6-16f**: `WAIT_UP` = `0x100000`, предельный `Fixed` — 4 000 000 шагов.
- Образ не содержит обработчиков; обмен IPI, повторный SIPI работающему AP, IPI на несуществующий APIC ID, диапазон seed, флаг `od-runner --stage6-smp-probe` и CLI-тест — S6-16f (сделано, см. ниже). Планировщик не подключён к циклу Linux (S6-21 шаг 1).

## S6-16f — обмен IPI и отрицательные случаи

Дата: 2026-11-17. Чекбокс S6-16 в `STAGE_6.md` не отмечен. Реализация: образ v2 (`tools/build/build_stage6_smp_probe.py`, сценарии `--scenario`), загрузчик (`crates/od-machine/src/stage6_smp_probe.rs`: политика `Schedule`, `run_recorded`, `vector_state`, `pending_vectors`, счётчики vCPU в отчёте), флаг `od-runner --stage6-smp-probe` (`tools/runner/src/stage6_smp_probe.rs`), тесты `crates/od-machine/tests/stage6_smp_probe.rs` (21), `tools/runner/tests/stage6_smp_probe_cli.rs` (5 + 1 `#[ignore]`), `tools/build/test_build_stage6_smp_probe.py` (14). Методика, пределы кванта, диапазоны seed — [SMP.md](../testing/SMP.md) («probe v2»); формы кода — [CPU_CONTRACT.md](CPU_CONTRACT.md) («S6-16f»).

**Образ v2** (профиль `Stage6SmpProbeV2`; раскладка v1 сохранена, добавлено): IDT `0x9800` (векторы `0..0x3F`, шлюзы 64-битных прерываний, селектор `0x10`; заданы шлюзы 2, `0x30`, `0x31`), обработчики `0x9C00` (AP, вектор `0x30`), `0x9D00` (BSP, вектор `0x31`), `0x9E00` (AP, NMI), дескриптор IDTR BSP `0x8150`, ячейки счётчиков `0xC018` (AP принял `0x30`), `0xC020` (BSP принял `0x31`), `0xC028` (NMI). IDTR AP trampoline грузит в real mode (32-битная база на IDT). IDTR BSP грузит гость: `MOV ESP,desc; LIDT [RSP+0]; MOV ESP,stack` — единственная форма `LIDT` набора CPL0, она исполняется (проверено: предел 0 в дескрипторе даёт `GateBeyondIdtLimit` на векторе `0x31`); загрузчик IDTR не устанавливает. Образ байт в байт воспроизводим для каждого сценария (SHA-256 двух сборок и манифесты совпадают); nonce в образе нет.

**Сценарии** (выбираются при сборке; K — `--rounds`, 1..=100, по умолчанию 16):
1. `normal` — S6-16e: `SMP PROBE OK <nonce>`.
2. `exchange` — BSP делает `STI` и K кругов: Fixed `0x30` на APIC ID 1 (ICR_HIGH `1 << 24`, ICR_LOW `0x30`), ждёт (не более `WAIT_RX` итераций) пока счётчик BSP станет равен номеру круга. Обработчик AP: счётчик++, EOI (`0xFEE000B0` ← 0), Fixed `0x31` на APIC ID 0, `IRETQ`. Обработчик BSP: счётчик++, EOI, `IRETQ`. После K кругов BSP делает `CLI`, сверяет счётчик BSP = K, счётчик AP = K, счётчик стартов = 1 (иначе `SMP PROBE FAIL EXCHANGE <nonce>` / `FAIL COUNT`) и печатает `SMP PROBE OK K=<K> <nonce>`. Порядок UART сохранён: AP печатает до флага, BSP после обмена.
3. `resipi` — перед первым кругом BSP посылает второй SIPI уже работающему AP (ICR `0x0608`, адресат 1). Ожидание: AP не перезапускается (SMP_CONTRACT §5, SIPI применяется только к AP в `WaitForSipi`): счётчик стартов остаётся 1, `started = 1`, `init_applied = 1`, обмен идёт.
4. `bad-ipi` — BSP печатает `SMP PROBE BAD-IPI <nonce>` и пишет ICR на APIC ID `vcpu_count << 24` (первый несуществующий; ICR_LOW Fixed `0x30`). По SMP_CONTRACT §5 адресат вне `0…N−1` — явная ошибка устройства без изменения состояния. Плата возвращает `FirmwareBoardError::VcpuQuantum { cpu: 0, detail }`, где `detail` содержит `MMIO device 1 write failed: UnsupportedConfiguration` на адресе `0xFEE00000` (запись ICR_LOW через MMIO окно LAPIC не прошла; ICR_HIGH записан до неё и вектора не создаёт). Ошибка завершает весь прогон: после маркера в COM1 ничего нет (гость, получивший управление назад, напечатал бы `SMP PROBE FAIL ACCEPTED`), IRR и ISR обоих LAPIC пусты, AP остаётся `Halted` с `wakes = 0`, счётчики обмена нулевые. Работает при любом `vcpu_count` ≥ 2 (проверено 2, 3, 4). Манифест называет ошибку `LapicDeviceError`; тест сопоставляет её с `VcpuQuantum` по тексту (отдельного варианта ошибки в плате нет).
5. `nmi` — BSP шлёт NMI (ICR `0x400`) на APIC ID 1 и ждёт ячейку обработчика вектора 2 (`WAIT_RX`); `SMP PROBE NMI OK <nonce>`. **Измеренное поведение.** Если CR3 AP равен `page_table_root` платы (в probe оба — PML4 образа), AP принимает NMI (`nmi_delivered = 1`), обработчик вектора 2 исполняется, ячейка = 1, транскрипт совпадает с ожидаемым по SDM. Если CR3 AP отличается от `page_table_root` (тест подменяет в trampoline корень на копию PML4 по `0x10000`), NMI тоже доставляется: `deliver_pending_nmi` строит обход по CR3 vCPU, транскрипт полный, `nmi_delivered = 1`, ячейка = 1. В первой редакции S6-16f обход шёл по `page_table_root` платы и такой прогон кончался `Config("deliver_pending_nmi: no paging for the vCPU")` (`Cr3Mismatch`); это найдено ревью и исправлено в этом же срезе.
6. Один vCPU (любой сценарий): `SMP PROBE AP ABSENT <nonce>`, успеха нет.

**Выбор K и кванта.** K = 16 (значение плана; бюджет WAIT_UP не ограничивает). Ожидания `WAIT_UP = WAIT_RX = 0x100000` итераций (пять инструкций BSP на итерацию, около 5,24 млн шагов): ожидание не может кончиться внутри одного хода BSP, пока AP не получил ход, при кванте `Fixed` не больше `STAGE6_SMP_PROBE_MAX_FIXED_QUANTUM` = 4 000 000 шагов; `Seeded` даёт не больше 256. Предел S6-16e (`FAIL TIMEOUT` от ~327 000) снят. Измерено (release, N=2, nonce `ab`, retired [BSP, AP]): `exchange` `Fixed(1)` [1333, 419], `Fixed(7)` [1323, 419], `Fixed(100)` [2468, 419], `Fixed(1024)` [17573, 419], `Fixed(100000)` [1 700 168, 419], `Fixed(1000000)` [17 000 168, 419], `Fixed(4000000)` [68 000 168, 419] — всюду OK; `normal` `Fixed(5000000)` OK, `Fixed(6000000)` `FAIL TIMEOUT` (BSP 5 243 560, строка `AP1 UP` после ошибки, ложного OK нет). Бюджет прогона `Fixed(Q)` — около K·Q (при Q = 4 000 000 больше 68 млн).

**Загрузчик и раннер.** `Stage6SmpProbeQuantum` — `Fixed`, `Seeded` (SplitMix64, 1..=256 шагов), `Schedule(Vec<usize>)` (один шаг названного vCPU на запись; по концу — `Stage6SmpProbeFinish::ScheduleEnd`); тип больше не `Copy`. Отчёт: на vCPU — `retired`, `turns`, `steps`, `wakes`, `init_applied`, `started`, `nmi_delivered`, активность, `legacy`; общие — `turns`, `idle_ns`, `idle_jumps`, `schedule` (только `run_recorded`). Ошибка платы несёт счётчики vCPU (`Board { error, com1, cpus }`). `vector_state(cpu, v)` и `pending_vectors(cpu)` читают IRR/ISR LAPIC vCPU. `od-runner --stage6-smp-probe IMAGE --nonce T [--vcpus N] [--scenario S] [--quantum Q | --seed S | --schedule CPU:STEPS,...] [--budget B] [--record-schedule]`: JSON на stdout (retired, turns, init/started/nmi по vCPU, байты COM1 текстом и hex, ячейки протокола, векторы, оставшиеся в LAPIC, причина завершения, политика кванта, SHA-256 образа); коды выхода 0 (`guest_idle`, `schedule_end`), 3 (неверные аргументы, несовпадение `--scenario` с манифестом рядом с образом), 4 (отказ загрузчика), 5 (ошибка платы, лимит работы, ошибка планировщика). Код выхода 0 при `guest_idle` не означает OK probe: вердикт — по `com1_text` (`OK`/`FAIL`). JSON идентичен побайтно при одинаковых аргументах.

**Тесты.** Rust (21): v1 (9, обновлены) и новые — обмен K = 16 при `Fixed(1024)` (счётчики по манифесту, LAPIC пусты); тот же транскрипт и ячейки при квантах 1, 2, 7, 100, 100000 и seed 0..16 (`wakes ≤ K`: IPI до `HLT` AP будить не нужно, поэтому точное число зависит от расписания); разные seed режут ход по-разному, один seed повторяется точно (включая записанное расписание); K = 1 и K = 100 (другой образ, манифест называет K); `AP ABSENT` во всех сценариях при N = 1; `resipi` (образ отличается от `exchange`, `started = 1`, `init_applied = 1`, счётчик стартов 1, транскрипт и ячейки); `bad-ipi` (ошибка устройства, маркер последний, ISR/IRR пусты до и после, AP не будился) при квантах 1, 2, 7, 100, 1024 и 16 seed и при N = 3, 4; `nmi` с общим CR3 (доставка) и на другом CR3 (отказ платы, как выше); `LIDT` гостя действует (предел 0 → `GateBeyondIdtLimit`); граница кванта (`Fixed(4000000)` — OK, `Fixed(6000000)` — `FAIL TIMEOUT`, ложного OK нет). CLI (5 + 1): фиксированный квант (все поля JSON, хеш образа из манифеста); явное расписание (запись при `--quantum 1`, повтор: тот же транскрипт, `retired`, `started`, конец `schedule_end`, одинаковые байты дважды; шаг не работающего vCPU отвергается `scheduler_error`); все seed 0..=63 (дважды побайтно, COM1 по манифесту, ячейки, число ходов различается минимум у двух seed, nonce в выводе); отрицательные сценарии (N = 1, `bad-ipi` — код 5 и `board_error`, `resipi`, `nmi`); отказы аргументов (код 3), нонс с пробелом (код 4); `#[ignore]` `seed_proof` — длинный диапазон. Python (14): воспроизводимость каждого сценария, шлюзы IDT и дескрипторы по SDM, обработчики, ячейки и ожидания по сценариям, ICR, формы кода.

**Ограничения (записаны, не обойдены).**
- NMI к AP доставляется по CR3 vCPU (см. п. 5). Проверено на двух корнях (PML4 образа и копия PML4 по `0x10000`; нижние уровни таблиц общие), на одном образе таблиц страниц.
- Ошибка устройства LAPIC приходит как `VcpuQuantum` с текстом MMIO-ошибки (`UnsupportedConfiguration`), а не отдельным вариантом `FirmwareBoardError`; манифест сборщика называет её `LapicDeviceError` (`expected.n2.run_error`), типом ошибки платы это не является; тест проверяет текст и адрес окна.
- Повторный SIPI работающему AP не наблюдается напрямую (модель не считает проигнорированный SIPI): доказательство — `started = 1`, счётчик стартов 1, обмен и транскрипт без изменений.
- `Fixed` квант больше 4 000 000 шагов вне диапазона probe; `exchange` под `Fixed(4000000)` проверен только вручную в release (32 с, не в CI).
- Обмен идёт на одном AP и в CPL0 с одним стеком: исключения гостя, IST, TSS, другое кольцо, IOAPIC/MSI не проверяются. Планировщик не подключён к циклу Linux (S6-21 шаг 1); брать «`wakes`» как точное число нельзя (зависит от расписания).
- Оракула порядка, независимого от модели, нет: ожидания записаны по протоколу и SDM (см. «Не покрыто оракулом» в SMP.md).

## S6-17 — согласование адресных пространств и shootdown

Дата: 2026-11-17. Чекбокс S6-17 в `STAGE_6.md` не отмечен. Срез: свойство согласования и сценарий `shootdown` probe (шаги 1–5 плана); шаг 6 (снятие срока платы и доставка источника платы BSP) в этот срез не входит.

**Свойство (шаг 1).** После того как vCPU выполнил свою инвалидацию (`INVLPG`, запись `CR3`, смена `CR4`/`CR0`, влияющая на перевод), он не использует старый перевод. До неё он может видеть старый или новый перевод после чужой записи в таблицы — оба допустимы (Intel SDM Vol. 3A 4.10.4.1, 4.10.4.2; ссылка по памяти, с первоисточником в этом срезе не сверялась). Нормативная формулировка и требования к будущим кешам — [MEMORY_MODEL.md](MEMORY_MODEL.md), раздел «Согласование адресных пространств (S6-17)».

**Что есть на 2026-11-17 (шаг 5, чувствительность).** Кеша перевода, переживающего квант vCPU, нет:
- квант платы `run_vcpu_set_quantum` не имеет кеша выборки («It has no fetch cache», S6-16d); перевод данных и выборки — полный обход таблиц на каждый доступ по `CR3` самого vCPU (`paged_memory_adapter`);
- `FetchTranslationCache` (S4-13/S4-14h/S4-14i) живёт в цикле `run_linux_boot_cpl0_set` (vCPU 0, не в планировщике); постоянный кеш S4-14i (`persistent`) opt-in и к плате не подключён. Политика `GuestOnly` этого кеша уже сбрасывается по `INVLPG`, записи `CR3`, смене `CR0`/`CR4` и доставленному `#PF` (CPU_CONTRACT, S4-14h); один кеш принадлежит одному вызову одного vCPU (шаг 2 выполнен по построению: общего кеша нет);
- `INVLPG m` в наборе CPL0 — no-op с retire (S4-38L7ab; модель без TLB).
Запись из шага 5: **свойство выполняется тривиально; чувствительность не проверялась** (отключённой инвалидации, на которой сценарий обязан падать, не существует). Задача S4-42 не выполнялась. Тест `without_the_invlpg_the_machine_without_a_tlb_still_gives_the_new_frame_so_the_probe_is_not_sensitive_to_it` фиксирует именно это (INVLPG вырезан из образа, результат тот же) и должен начать падать, когда появится кеш перевода, переживающий HLT/квант.

**Сценарий `shootdown` probe (шаг 4).** Выбирается при сборке образа (`--scenario shootdown`, K кругов, `--rounds`, 1..=100, по умолчанию 16). Образы остальных сценариев побайтно прежние (проверено сравнением со сборщиком до среза: 5 сценариев × 3 варианта, образ и манифест). Общие для BSP и AP таблицы страниц (CR3 обоих — PML4 образа): страница X = `0x201000` — 4 KiB страница (`PD[1] → PT_L` по `0x4000+8`, `PT_L[1]` по `0x6008`; `PT_L` — тот же кадр, что держит запись окна Local APIC в `PT_L[0]`). X отображается на кадр A (`0xC000`, страница данных probe) или кадр B (`0x7000`, страница info; загрузчик пишет только первые 64 байта); по смещению `0x100` образ держит `VALUE_A = 0xA1A1A1A1` (в `0xC100`) и `VALUE_B = 0xB2B2B2B2` (в `0x7100`); AP читает X по линейному адресу `0x201100`.
Протокол: AP до флага «AP up» читает X (первый перевод, кадр A) и кладёт значение в `0xC0C8`; BSP после «AP up» проверяет, что там `VALUE_A`. Круг r: BSP записывает младшее двойное слово PTE X (кадр B при нечётном r, A при чётном: `mov [0x6008], eax`), шлёт Fixed-вектор `0x32` на APIC ID 1 и ждёт (до `WAIT_RX` итераций), пока счётчик «после» `0xC038` станет r. Обработчик вектора `0x32` на AP (`0x9F00`): `INVLPG [r8]` (`41 0F 01 38`, r8 = X), чтение X в `0xC0C0`, счётчик подтверждений `0xC030`, EOI, `IRETQ`. Главный цикл AP (при IF = 0 сравнивает подтверждения с постами; равны — `STI; HLT`, иначе читает X ещё раз в `0xC0C8` и увеличивает счётчик постов `0xC038`): вектор, пришедший между сравнением и `HLT`, не теряется (прерывание принимается после начала `HLT`). После поста BSP требует: подтверждений ровно r, значение обработчика и значение главного цикла равны значению нового кадра (круг — A или B). Итог: `SMP PROBE SHOOTDOWN OK K=<K> <nonce>`; любое расхождение — `SMP PROBE FAIL SHOOTDOWN <nonce>`. Исчерпание ожидания поста (`WAIT_RX` итераций) тоже даёт `FAIL SHOOTDOWN`, а не `FAIL TIMEOUT`: таймаут и расхождение значений в этом сценарии по UART неразличимы (ограничение).
Ожидание по SDM: после своей `INVLPG` процессор не использует старый перевод; до неё — вправе, и проба этого не утверждает (на машине без TLB новое значение видно и до `INVLPG`, на железе — не обязано).
Ожидания (UART, ячейки) записаны сборщиком из протокола, не из прогона (`build_manifest.json`: `sd_ack`, `sd_post`, `sd_seen_handler`, `sd_seen_main`, `pte_x_frame`). Кадр PTE сравнивается без младших 12 бит (обход AP ставит бит accessed).

**Что проверено.** Rust (`crates/od-machine/tests/stage6_smp_probe.rs`, 26 тестов, было 21): круг K = 16 при `Fixed(1024)` с ячейками по манифесту и пустыми LAPIC; тот же транскрипт и ячейки при квантах 1, 2, 7, 100, 100000 и seed 0..16; K = 1, 2, 3, 100 (конечный кадр B/A по чётности); чувствительность самого теста: из образа вырезана запись PTE → `FAIL SHOOTDOWN` (AP действительно читает через общую таблицу); вырезана `INVLPG` → тот же успех (тривиальность, см. выше); один vCPU → `AP ABSENT` и для `shootdown`. CLI (`tools/runner/tests/stage6_smp_probe_cli.rs`): `shootdown` на seed `0..=63`, JSON-ячейки `sd_*`. Диапазон `0..=9999` в release — см. [SMP.md](../testing/SMP.md) («probe v3»).

**Ограничения.**
- Один AP, один общий набор таблиц, CPL0, 4 KiB страница, длина цикла ≤ 100: другие CR3 vCPU, PCID, `INVLPG` по адресу в другом кольце, 2 MiB страницы, `MOV CR3` как инвалидация в сценарии не проверяются.
- Шаг 3 плана (уведомление о записи в кадр таблицы от любого vCPU доходит до кешей всех vCPU) — требование к будущему кешу и к S6-30; записано в MEMORY_MODEL.md, сейчас проверять нечего: кеша нет. S6-30b: механизм есть (атомарное поколение страницы, см. раздел «S6-30b» ниже); в режиме чередования шаг 3 проверен; межпоточная дорога тестом не доказана, а в параллельном режиме шаг 3 не закрыт: `FetchTranslationCache::seal` берёт поколения кадров таблиц после чтения байт (порядок «байты, затем поколение»); кеш с `WriteWatch` в параллельном режиме запрещён, пока взятие поколения не перенесено в `insert` до чтения записей обхода — см. S6-34.
- Шаг 6 (снятие срока платы и доставка `irq_source` платы на границе BSP в `BoardSmpHost`, S6-16d п. 6(а)) НЕ реализован; остаётся обязательным условием S6-21.

## S6-18a — CPUID на каждом vCPU и профиль AP

Дата: 2026-11-17. Чекбокс S6-18 в `STAGE_6.md` не отмечен. Реализация: `crates/od-isa/src/cpuid.rs` (`CpuidProfile::query_for_vcpu`), `crates/od-isa/src/real_mode_exec.rs` (`CpuidSource::ProfileOnVcpu`), `crates/od-machine/src/vcpu.rs` (`VcpuCpuid`, `init_cpu_for`, `Vcpu::set_cpuid`, `LINUX_AP_CPUID_PROFILE`), `crates/od-machine/src/firmware_board.rs` (`cpuid_profile`, `set_cpuid_profile`, `into_linux_board`). Закрывает «Открыто (CPUID legacy-моделей и EDX)» раздела S6-16c.

**CPUID на vCPU.** Для платы из `N` vCPU `CPUID.01H:EBX` вычисляется как `(ebx профиля & 0xFFFF) | N << 16 | APIC_ID << 24` при `N > 1`; при `N = 1` — байты профиля без изменений (`0x100`). APIC ID — индекс vCPU. Листья `0xB`, `0x1F`, `0x80000026` остаются нулевыми при любом `N` (так сказано в `SMP_CONTRACT.md`, раздел 2); остальные листы и EDX (включая бит HTT, 28) не меняются. Открыто: по SDM EBX[23:16] определён только при HTT = 1, а профиль HTT не объявляет; Linux читает поле всегда, использует только при HTT. Бит не добавлен: профиль признаков принадлежит `CPU_CONTRACT.md`, решение по topology domains — S6-18 (`SMP_CONTRACT.md` раздел 2), с S6-21.

**Где применено.** К legacy-моделям (`RealModeCpu`) каждого vCPU: поле `Vcpu::cpuid: VcpuCpuid { profile, apic_id, cpu_count }`; `VcpuCpuid::source()` — `CpuidSource::Profile(profile)` при `N = 1` и `CpuidSource::ProfileOnVcpu { .. }` при `N > 1`. Для long mode путь S6-21 использует тот же APIC ID и `N` из `VcpuCpuid` и профиль `Cpl0ProfileSet`, выбранный для BSP и AP.

**EDX после INIT** (SDM Vol. 3A Table 12-1) = `CPUID.01H:EAX` профиля vCPU (`VcpuCpuid::init_edx`), а не константа. У всех четырёх профилей `cpuid.rs` EAX листа 1 равен `0x600`, поэтому значение и ожидания тестов S6-13/S6-16c прежние; константа `AP_INIT_EDX` осталась значением для профиля по умолчанию (используется в тестах). `init_cpu(old_cr0)` — прежняя функция для одиночного процессора (профиль `DiagnosticV1`, индекс 0, `N = 1`); плата использует `init_cpu_for(old_cr0, vcpu.cpuid)`.

**Профиль платы.** `LoadedFirmwareBoard::set_cpuid_profile(profile)` задаёт профиль всем vCPU (APIC ID и `N` сохраняются); модель, которая ещё в состоянии включения питания или INIT (источник и EDX как в старой конфигурации), берёт новый профиль, запущенная — нет. `cpuid_profile()` читает его. INIT и SIPI конфигурацию не меняют. Плата `into_linux_board` (путь `linux-direct`) ставит `LINUX_AP_CPUID_PROFILE`; прочие платы (прошивка, probe) остаются на `DiagnosticV1`.

**Ограничение `VcpuCpuid::new`:** APIC ID и `N` хранятся как `u8` (`index as u8`, `count as u8`); `MAX_VCPUS = 4`, усечение недостижимо, в коде стоит `debug_assert!(count <= MAX_VCPUS)`.

**Выбранный профиль AP: `OdX8664V1Neutral`.** `verify_cpu` ядра требует по leaf 1 EDX маску `0x07008161` (MSR, PAE, CX8, CMOV, FXSR, SSE, SSE2 вместе с FPU) и LM/NX/SYSCALL по `0x80000001`; на `DiagnosticV1`, `X8664V1Stage4A`, `X8664V1Stage4AApic` функция возвращает 1 (`s6_14g_verify_cpu_on_the_cpuid_rs_profiles_takes_the_failure_path`), на `OdX8664V1Neutral` — 0 (`s6_14h_verify_cpu_success_path_on_the_neutral_profile`, новый `s6_18a_verify_cpu_succeeds_on_the_neutral_profile_per_vcpu` — для vCPU 1 из 2 тот же след и EBX = `0x01020100`). Профиль отличается от профиля BSP в CPL0-запуске (BSP: `X8664V1Stage4AApic` (linux-direct; допустим и `DiagnosticV1`); он объявляет только признаки, поведение которых реализовано): trampoline AP до long mode исполняет `verify_cpu` по полной таблице цели, а CPUID AP в long mode (S6-21) будет читать профиль набора. `OdX8664V1Neutral` используется только legacy-моделью AP до перехода в long mode; на железе CPUID от режима не зависит, поэтому расхождение модели осознанное. Следствие, не решённое здесь: ядро видит разные признаки до и после long mode на AP и разные на BSP и AP; условие S6-21: CPUID AP в long mode побайтно равен CPUID BSP в long mode на всех листах, кроме `CPUID.1` EBX[31:24] (APIC ID) и [23:16] (`N`) — для этого `query_for_vcpu` подключается к `Cpl0ProfileSet`; без этого S6-21 заблокирована. `ProtectedCpu` не исполняет `CPUID` вне тестов: `CPUID` исполняется только в real-mode trampoline (`verify_cpu`).

**Тесты.** `od-isa`: `cpuid::tests::s6_18a_*` (2: один vCPU равен `query` побайтно на всех листах и профилях; для `N = 2, 4` меняется только EBX листа 1, поля по рукам из SDM), `real_mode_exec::tests::s6_18a_verify_cpu_succeeds_on_the_neutral_profile_per_vcpu`. `od-machine`: `vcpu::tests::s6_18a_*` (2: набор vCPU и конфигурация; профиль платы, перепосев, переживание INIT/SIPI, плата с одним vCPU), `ap_legacy_exec::tests::s6_18a_an_ap_reads_its_apic_id_and_the_vcpu_count_through_cpuid` (AP исполняет `mov eax,1; cpuid`: EBX = `0x100 | N << 16 | id << 24`, EDX = `0x0781A379`, `N` = 2 и 4). Ожидания трёх тестов `ap_legacy_exec` (`an_ap_retires_nothing_before_sipi`, `init_in_the_middle_…`, `an_unsupported_instruction_…`) изменены: состояние модели сравнивается с `init_cpu_for(.., ap.cpuid)`, потому что поле `cpuid` модели платы AP стало `ProfileOnVcpu { OdX8664V1Neutral, 1, 2 }`; регистры и `mode` те же.

**MADT с `N` записями Local APIC (`S6-18a-MADT`).** `firmware/uefi/src/acpi/madt.rs` сериализует и валидирует одну запись, а `tools/build/build_stage4_acpi_blob.py` упаковывает готовый Rust-блок. Принятый S7-02 закрепляет `cpu_count = 1` в handoff. Для нескольких записей нужны согласованное расширение handoff в S6-18b и согласование с владельцем S7-13. Проверка байтов MADT для N = 1 и N = 2 выполняется после расширения сериализатора.

**Не входит.** CPUID на vCPU в long mode (S6-21); бит HTT и topology domains (открыто выше); MADT с `N > 1`, DSDT-объекты процессоров; `cpu_count` в handoff (S6-18b).

## S6-19 — litmus в режиме чередования

Дата: 2026-11-17. Чекбокс S6-19 в `STAGE_6.md` не отмечен (срез: драйвер, перебор, эталон; S6-19b — флаг/API/CLI-тест, закрытие S6-19 — после S6-20). Реализация: `crates/od-machine/src/litmus_interleaving.rs` (`cfg(test)`), `[dev-dependencies] serde_json = "=1.0.151"` (уже в `Cargo.lock`), данные `docs/reports/data/stage6/s6-19-litmus-interleaving.json`. Методика, числа и результаты — [SMP.md](../testing/SMP.md) («S6-19 — litmus в режиме чередования»).

**Контракт драйвера.** Один vCPU на поток теста создаётся сразу в long mode на плоской identity RAM (harness litmus по SMP_CONTRACT 4.4, не старт AP; записано в выводе). Шаг планировщика — одна инструкция целиком либо срез `REP` до 1024 итераций; драйвер подаёт расписания входом `QuantumPolicy::Schedule` (полный перебор, если число расписаний `(Σ n_i)!/Π n_i!` не выше 1 000 000) или, при seed `0..=9999`, случайным порядком шагов (`random_schedule`: на каждом шаге поток выбирается среди не исчерпанных по SplitMix64(seed)), тоже как `QuantumPolicy::Schedule`; `QuantumPolicy::Seeded` не используется (круговой обход с cpu0 не достигает исходов, где поздний поток идёт раньше cpu0). Эталон — множество `interleaving_instruction_atomic` fixture S6-05 (для счётчиков — формула fixture); перебор должен **равняться** ему, seed — быть подмножеством. Три класса вердикта: равно, «запрещённый» (наблюдён вне эталона, дефект), «не покрыт перебором» (эталонный исход не достигнут). Каждый вызов интерпретатора проверяется: он кончается на границе инструкции либо на RIP той же `REP`; иначе счётчик `instructions_cut` растёт (на корпусе 0).

**Результат.** 8 конечных fixture (SB, MP, LB, 2+2W, IRIW на 4 vCPU, SB-MFENCE, STORE-FORWARDING, UNLOCKED-INC) и счётчики UNLOCKED-INC/THREE-INSN-COUNTER (N до 4, K до 3 полным перебором; N=3,K=2 и N=4,K=4 по seed) — запрещённых исходов нет, не достигнутых полным перебором эталонных исходов нет; seed `0..=9999` на конечных fixture достигает все эталонные исходы, на THREE-INSN-COUNTER N=4,K=4 — 9 значений из 15 (11…16 не покрыты seed, не ошибка). REP режется после L, 2L, … итераций (count ≤ L — один неделимый шаг), проверено полным перебором против независимого абстрактного эталона.

**Ограничения.** (Состояние среза S6-19; после S6-20 7 fixture с `LOCK INC`/`LOCK CMPXCHG`/dword `LOCK XADD`/dword `XCHG` и счётчик LOCK-INC исполняются и дают `equal`, см. [SMP.md](../testing/SMP.md).) 7 конечных fixture с `LOCK INC`/`LOCK CMPXCHG`/dword `LOCK XADD`/dword `XCHG` и счётчик LOCK-INC интерпретатор не исполнял (допуск форм — S6-20); они не были запущены и не подменены. (До S6-19b; см. ниже.) Нет загрузчика и флага `od-runner --stage6-litmus` (планировщик `pub(crate)`), нет CLI-теста: это S6-19b, не сделаны в этом срезе. Параллельные потоки и x86-TSO не моделируются (S6-36); результат — не проверка x86-TSO. `LOOP` не исполняется интерпретатором: счётчики — тело корпуса ×K; драйвер `LOOP` не учитывает (к памяти не обращается; счётчики по шагам с памятью), формула fixture его учитывает — сокращение записано.

**Зависимость `serde_json` (по `docs/policies/DEPENDENCIES.md`) — состояние до S6-19b; см. «Зависимость `serde_json` … изменение S6-19» ниже.** Назначение: разбор JSON fixture корпуса litmus в dev-тестах. Категория: dev-зависимость (`[dev-dependencies]`). Версия `=1.0.151` уже закреплена в 5 crate'ах (od-host, od-testkit, tools/bench, tools/runner, tools/x86-runner) и в `Cargo.lock`. Лицензия MIT OR Apache-2.0. Features по умолчанию. (До S6-19b.) Runtime-дерево `od-machine` не меняется (проверено `cargo tree -p od-machine -e normal`: `serde_json` в нём нет).

## S6-19b — публичный загрузчик и флаг `--stage6-litmus`

Дата: 2026-11-17. Чекбокс S6-19 в `STAGE_6.md` не отмечен (закрытие — после S6-20). Реализация: `crates/od-machine/src/litmus.rs` (публичный API), `litmus_interleaving.rs` (драйвер S6-19, теперь не `cfg(test)`, тесты там же), `tools/runner/src/stage6_litmus.rs`, тест `tools/runner/tests/stage6_litmus_cli.rs`. Методика и формат JSON — [SMP.md](../testing/SMP.md) («S6-19b»).

**Публичная поверхность `od-machine`.** `LitmusCase` (из текста fixture S6-05 или файла: `from_fixture_json`, `from_fixture_file`, `from_formula_json`; непрозрачный), `LitmusPolicy` (`Exhaustive{max_schedules}`, `Seeds(range)`, `Fixed(q)`, `Schedule(list)`), `run_litmus(&case, policy) -> Result<LitmusReport, LitmusError>`, `LitmusReport`/`LitmusOutcome`/`LitmusWitness`/`LitmusUnsupported`/`LitmusVerdict` (простые структуры, без `serde`), `litmus_seed_schedule`. `SmpScheduler`, `QuantumPolicy`, `SmpHost` остаются `pub(crate)`: наружу видны только четыре способа обхода. Загрузчик возвращает `Err(LitmusError::Load)`, а не паникует: неверный JSON/схема, неизвестный регистр, код или ячейка вне 64 КиБ RAM, пересечение областей, число потоков вне 1…14. Форма, которую интерпретатор не исполняет (`LOCK`-формы до S6-20), — `Ok` с вердиктом `Unsupported` (инструкция, байты, cpu, RIP), не `Equal`; выход гостя вне ожидаемых (`Fault`) — `Err(LitmusError::Guest)`. Драйвер тот же, что в S6-19 (общий код, не копия): `try_explore_exhaustive`/`try_explore_seeds`/`try_run_case`; паники прежних `explore_*`/`run_case` остались только в тестах.

**Зависимость `serde_json` (по `docs/policies/DEPENDENCIES.md`) — изменение S6-19.** Назначение: разбор JSON fixture корпуса litmus загрузчиком (`serde_json::Value`, без `derive`; результаты `od-machine` — простые структуры, JSON собирает вызывающий). Категория: **обычная зависимость** `od-machine` (`[dependencies]`, ранее dev). Источник: crates.io (реестр, как у остальных зависимостей workspace). Версия `=1.0.151`, та же, что в od-host, od-testkit, tools/bench, tools/runner, tools/x86-runner; в `Cargo.lock` уже есть. Лицензия MIT OR Apache-2.0. Features по умолчанию. Runtime-дерево `od-machine` растёт на `serde_json` и его зависимости (`itoa`, `memchr`, `serde_core`, `zmij`; `cargo tree -p od-machine -e normal`); транзитивные `itoa`, `memchr`, `serde_core`, `zmij` не содержат запрещённого компонента (`DEPENDENCIES.md`, пункт 4); альтернатива — свой разбор JSON — отвергнута как большая и хрупкая.

**Ограничения.** (Состояние среза S6-19b; S6-20 допустил `LOCK`-формы и `XCHG` с памятью корпуса: 7 fixture и счётчик LOCK-INC теперь `equal`, корпус целиком — 15 `equal`.) `LOCK`-формы и `XCHG` с памятью (7 fixture и счётчик LOCK-INC) тогда не исполнялись. Режим — «SC при неделимой инструкции», не x86-TSO и не параллельный прогон. Корпус не менялся, эталон не менялся.


## S6-21 шаг 1 — источник платы и срок платы в планировщике

Дата: 2026-11-17. Реализация части шага 1 для BSP: `crates/od-machine/src/firmware_board/vcpu_quantum.rs` (`BootProcessorSource`, `run_vcpu_set_quantum`), `crates/od-machine/src/smp_scheduler.rs` (`SmpHost::board_wakes`, `SmpHost::idle_deadline`, `BoardSmpHost::board_deadline`), `crates/od-machine/src/firmware_board.rs` (`LoadedFirmwareBoard::irq_wake_ns`). Закрывает открытый пункт (а) раздела [S6-16d](#s6-16d--квант-платы-для-любого-vcpu) (перенесён из S6-17 шага 6 и S6-16d п. 6(а)).

**Доставка источника платы.** Источник платы (`LoadedFirmwareBoard::irq_source`: PIT → 8259, PIT → IOAPIC, обёртки UART; `BoardIrqSource`) принадлежит BSP (`BOOT_VCPU_INDEX`). `run_vcpu_set_quantum` для `cpu == 0` подставляет в `IrqBoundaryHook` составной источник `BootProcessorSource`: `pending_vector` — вектор источника платы, а если его нет — `LapicIrqSource` своего LAPIC (источник IOAPIC и так отдаёт eligible-вектор LAPIC, тогда запасной ветви не нужно). Допущение модели: порядок «плата, затем LAPIC» задаёт источник (ExtINT PIC вне PPR; IOAPIC — `eligible()` BSP с векторным порядком); LINT0 не моделируется; `acknowledge(v)` идёт в источник, который выдал `v` при выборе. Источник IOAPIC подтверждает уже доставленный вектор, когда в IRR успел появиться старший запрос; `next_deadline_ns`, `pit_write_ends_quantum`, `irq0_edges`, `wake_ns` — источника платы (запись в порты PIT кончает вызов, как в цикле Linux). Доставка — на каждой границе инструкции внутри вызова через IDT и CR3 этого vCPU, как у цикла Linux (`deliver_pending_irq`: IF, STI-тень, `Delivered` → `acknowledge`); при `NoIrqSource` результат побайтно прежний (запасная ветвь — тот же `LapicIrqSource`; тест «один vCPU совпадает с прямым вызовом цикла» не менялся). Остальным vCPU источник платы не предлагается: у них только `LapicIrqSource` своего LAPIC. Цикл `run_linux_boot_cpl0_set` не менялся.

**Пробуждение.** `SmpHost::board_wakes(cpu)` (по умолчанию `false`): `BoardSmpHost` отвечает `true` только для BSP, когда `irq_wake_ns()` (`BoardIrqSource::wake_ns` платы) не позже часов — вектор доставляется сейчас, как у idle-пути цикла Linux (`idle_wake_ns`). `SmpScheduler::wake_ready` будит остановленный vCPU с IF = 1, если в его LAPIC есть приемлемый вектор ИЛИ `board_wakes`. Прежде wake-view смотрел только на `eligible()` LAPIC, и BSP в `HLT` не просыпался от PIT.

**Снятие срока.** (1) `BoardSmpHost::board_deadline` отдаёт срок источника платы, только если он позже часов: срок, до которого часы дошли, снят — его берёт BSP на своей границе (источники применяют все наступившие фронты при ответе на вопрос, `catch_up`), и он больше не режет ходы остальных vCPU до одного шага (раньше источник, державший срок, оставлял каждый следующий вызов в один шаг). (2) `SmpHost::idle_deadline` (по умолчанию `board_deadline`): `BoardSmpHost` отдаёт ближайший момент пробуждения остановленного vCPU с IF = 1, если срок ещё впереди. Для BSP используется `irq_wake_ns()`, для адресованного через IOAPIC AP срок берётся по маршруту INTIN2. Если плата никого разбудить не может (линия замаскирована или в обслуживании, фронтов нет), срока нет и запуск кончается `Finished::AllIdle`, а не `IdleNoProgress` и не бесконечными прыжками от фронта к фронту (цикл Linux в этом случае кончается как `halted`). Прыжок до фронта, дошедший до срока, кончается `NoProgress` с пропуском больше 0 (не ошибка); следующий круг `wake_ready` будит BSP по `board_wakes`.

**Маршрут PIT → IOAPIC → AP (ISSUE-512).** При общем простое `idle_deadline` выбирает ближайший срок среди остановленных vCPU с IF = 1. Для AP `PitIoapicIrqSource::wake_ns_for` проверяет адресат INTIN2 и возможность LAPIC принять вектор по PPR. После достижения срока источник ставит вектор в IRR адресата, который просыпается по своему LAPIC. PIC остаётся источником BSP. В тесте при обоих значениях IF у BSP пробуждается только AP, обслуживает один IRQ и освобождает ISR через EOI.

**Ожидания тестов — по SDM Vol. 3A (гл. 6, прерывания и исключения; по памяти), даташитам 8259A (ICW1..ICW4, IMR, EOI) и 8254 (режим 2; по памяти), не вывод модели.** После ICW1..ICW4 и снятия маски IR0 фронт OUT0 PIT даёт INTR, процессор с IF = 1 берёт вектор `(ICW2 & 0xF8) | 0` через свой IDT (Linux пишет ICW2 мастера `0x30`, `Pic8259Role::icw2`; формула вектора — S7-32, срез O), EOI гостя (OCW2 `0x20` в порт `0x20`) освобождает линию; режим 2 даёт один фронт на `count` тиков PIT (1 193 182 Гц).

**Тесты** (`firmware_board::vcpu_quantum::tests`, 5 новых и 1 изменённый). `irq0_of_the_pit_reaches_the_boot_processor_at_the_boundary_and_not_the_application_processor` (BSP сам настраивает PIC и PIT через `OUT`, `STI`, `HLT`; AP крутится с IDT своего вектора: счётчик обработчика BSP 3..=5 за 40 000 нс, счётчик AP 0, `PitPicStats::acknowledged` равен числу обработанных (или на 1 больше при обрыве бюджета на середине обработчика); это верно для PIC, при IOAPIC счётчик считает все векторы LAPIC BSP, не только IRQ0, `ack_refused = 0`, `wakes` BSP не меньше числа прерываний); `the_idle_jump_goes_to_the_edge_of_the_source_and_the_halted_boot_processor_takes_it` (оба vCPU остановлены, каждый фронт достигается прыжком простоя, BSP будится источником платы, часы продвинулись не меньше, чем на `(n - 1)` периодов); `the_board_source_is_offered_to_the_boot_processor_only` (источник с одним вектором: AP не получает и `acknowledge` не вызывается, BSP берёт на первой границе, после — ничего); `the_board_vector_is_acknowledged_by_its_own_source` (маршрут `acknowledge`: вектор платы подтверждается в источнике платы, вектор LAPIC — через свой шлюз и в ISR LAPIC BSP; одновременность двух векторов не проверяется: LAPIC на границе 0 программно выключен); `an_unwakeable_board_ends_all_idle_without_a_jump` (срок 40 нс, никто не будится: `AllIdle`, без прыжков); `the_deadline_of_the_board_source_cuts_the_call_of_any_vcpu` изменён: после того как часы дошли до срока, следующий вызов — `(1, 60)` (остаток бюджета), не `(1, 1)`. Контроль от противного: без составного источника падают 4 новых теста доставки, без `idle_deadline` — тест `AllIdle`, без фильтра срока — изменённый тест. Тесты планировщика на `Rig` не менялись: значения по умолчанию `idle_deadline` и `board_wakes` дают прежнее поведение.

**Отличие от цикла Linux (осознанное).** В цикле Linux под источником PIC вектор LAPIC, который разбудил бы процессор, — `Config`-ошибка (`idle_to`); в кванте планировщика BSP при том же источнике доставляет вектор LAPIC запасной ветвью составного источника. Счётчики `IrqCounters` вызова локальны и не накапливаются; свидетельство доставки — счётчики самого источника (`PitPicStats`) и память гостя.

**Планировщик в одно-vCPU Linux.** `LoadedFirmwareBoard::enable_linux_interleaved` явно выбирает `SmpScheduler` для запуска с положительным бюджетом и начальной активностью `Running`. Обычный и interleaved запуск вызывают один Linux-квант с теми же обработчиками UART, PM, PIT, RDTSC и NVMe. В interleaved режиме квант сообщает планировщику retirement, время устройств и признак частичного REP. Планировщик владеет начислением времени и простаиванием. Linux сохраняет полный результат кванта, включая штатные выходы без retirement, и после планирования берёт итоговое время с платы. Вход с нулевым бюджетом или неисполняемым vCPU использует прямой путь отчёта.

После пробуждения от маскируемого прерывания Linux сохраняет списание одной единицы гостевого бюджета, включая последний вызов с нулевым бюджетом. Доставка NMI на границе планировщика может разбудить vCPU без этого списания. В PIC-профиле при отсутствии срока источника и ожидающего NMI результат остаётся `Halted`. Попытка пробуждения от LAPIC раньше готовности источника завершается отказом прежнего Linux-пути. [Проверки одно-vCPU профиля](../reports/STAGE_6_REPORT.md#s6-21-шаг-1--планировщик-для-активного-linux-на-одном-vcpu).

**Обычный Linux с часами `Standalone`.** Профиль `stage4a --user-mode` на одном активном vCPU с PIC передаёт тот же Linux-квант планировщику. Планировщик продвигает `VirtualClockHandle` через обычную операцию часов. Ограничение кванта сроком PIC по-прежнему вычисляет сам Linux-квант. Источник платы определяет пробуждение и доставку IRQ. Готовые LAPIC-таймеры и ожидающие INIT/SIPI/NMI не доставляются этим профилем, как в исходном прямом цикле. При ошибке начисления сохраняется уже применённая часть времени и порядок снятия тиков устройств. [Сверка штатного `linux-direct`](../reports/STAGE_6_REPORT.md#s6-21-шаг-1--обычный-linux-с-часами-standalone).

**Seed в Rust-фасаде Linux.** `LoadedFirmwareBoard::enable_seeded_linux_interleaved(seed)` выбирает `ClockOwner` и стандартную политику `SmpScheduler` с квантами от 1 до 256 шагов на одном vCPU. Каждый вызов, который входит в планировщик, начинает последовательность со значения переданного seed. Квант учитывает ограничение устройства и остаток бюджета. При меньшем лимите планировщика квант не учитывается как сокращённый сроком устройства. Частичные возвраты REP расходуют шаги хода при сохранении бюджета retirement. Границы начисления времени зависят от квантов. При смене seed или политики гостевой `RDTSC` может наблюдать другое время. [Проверки](../reports/STAGE_6_REPORT.md#s6-21-шаг-2--seed-в-rust-фасаде-linux).

**Осталось в S6-21.** Параметры `od-runner linux-direct` (число vCPU и seed) входят в шаг 2. Реальная загрузка нескольких vCPU, гостевые счётчики IPI и проверка предупреждений CPUID и APIC ID требуют шагов 3–6.

## S6-24 — остановка всех vCPU, снимок и сброс

Дата: 2026-11-17. Чекбокс S6-24 в `STAGE_6.md` не отмечен. Реализация: `crates/od-machine/src/smp_scheduler.rs` (`SmpScheduler::stop_all`/`resume`/`paused`/`state`/`set_state`/`reset`, `SafePoint`, `StopAck`, `StopError`, `SmpHost::stop_ack`/`settle`, `TurnCarry`, `SchedulerState`), `vm_snapshot.rs` (`VmSnapshot`), `apic_pair.rs` (`PairState::reset_all`), `local_apic.rs` (`LocalApicHandle::snapshot_pair`/`restore_pair`/`reset_all`), `firmware_board.rs` (`LoadedFirmwareBoard::reset_cpu_complex`), `vcpu.rs` (`Vcpu` стал значением: `Clone`, `Eq`), `stage6_smp_probe.rs` (`Stage6SmpProbeSession`: те же операции над probe v2). Источник правил — SMP_CONTRACT §3 (строки «Power-on», «Любое состояние: остановка VM host»), §9. Это значения в памяти процесса: формат файла, версии и manifest — `S9-24`/`S9-25`, барьер с вводом-выводом — `S9-28`; объявлять это снимком машины нельзя.

**Остановка на safe point (G6).** `SmpScheduler::stop_all(host, round_limit)` ставит STOP и собирает подтверждение КАЖДОГО vCPU (`SmpHost::stop_ack(cpu)`): `Boundary` (завершённая инструкция), `RepRestart` (точка частичного прогресса `REP`: последний вызов vCPU кончился внутри `REP`, `skip_first_boundary`), `Asleep(активность)` для `Halted`, `WaitForSipi` и `Shutdown` (гостевая инструкция для подтверждения не нужна). vCPU без подтверждения получает `SmpHost::settle(cpu)` (раунд продвижения к safe point; не исполняет гостевых инструкций и не двигает часы), затем раунд повторяется. По `round_limit` раундов (общий ограниченный срок) остановка кончается `StopError::Timeout { unacked, rounds }` со списком не пришедших vCPU: запрос STOP снят, активность, ожидающие события и сроки не тронуты, `SafePoint` не выдан, снимка нет. Успех возвращает `SafePoint` — по подтверждению на vCPU по индексам и время остановки; STOP остаётся в силе (каждый `run` кончается `Stopped`) до явного `resume`. Флаг STOP без `SafePoint` остановкой не считается: снять состояние без него нельзя. Часы не движутся на всём интервале (§9); более поздний IPI остаётся ожидающим в LAPIC и подтверждения не снимает. В режиме чередования один vCPU исполняется за раз и каждый вызов кончается на границе, поэтому `BoardSmpHost::stop_ack` отвечает по активности vCPU и признаку `REP` без ожидания; нетривиальное ожидание (`None`) нужно параллельному режиму (S6-34) и проверяется на двойнике хоста. Параллельный режим этим не определён.

**Продолжение хода (`TurnCarry`).** Ход, прерванный STOP или концом общего бюджета с остатком кванта, не теряется: остаток (vCPU, квант, остаток кванта, шаги и retired хода) хранится в планировщике. После явного `resume` следующий `run` продолжает этот ход с того же места БЕЗ границы vCPU (в непрерывном прогоне границ внутри хода тоже нет); поэтому прогон, остановленный, снятый, восстановленный и продолженный, равен прогону без остановки: те же ходы, счётчики, `executed_schedule`, часы. Простой `run` без `resume` ведёт себя как раньше: новый ход на границе (то, что вызывающий изменил между вызовами — ICR, состояние vCPU, — применяется сразу; на этом стоят тесты `ap_legacy_exec`), остаток хода отбрасывается. Запись хода (`record_turns`) для прерванного хода сохраняется с накопленными шагами и заменяется записью завершённого. Частичный `REP` тратит шаг кванта, но ничего не отгружает из общего бюджета; поэтому остаток хода пересчитывается на КАЖДОМ вызове хоста как `min(остаток кванта, остаток бюджета)` (исправление по ревью S6-24, M1; первая редакция считала его один раз на ход, и обрыв вызова на частичном `REP` при остатке бюджета отдавал этот остаток другому vCPU). Ход кончается, когда исчерпан квант, или бюджет (остаток кванта уходит в `TurnCarry`), или vCPU перестал работать, или виден STOP. Прогон, обрываемый бюджетом посреди частичного `REP`, с `resume` даёт те же ходы, счётчики, `executed_schedule`, память и часы, что непрерывный (тест `a_budget_that_ends_inside_a_partial_rep_goes_on_with_the_same_vcpu`: `rep stosb` на 2100 итераций на двух vCPU, фиксированные кванты 1…5000 и seed 0..20, обрывы по 1, 2, 3, 4, 7 отгруженных инструкций; контроль от противного: со старым расчётом тест падает).

**Состав `VmSnapshot`.** (1) Каждый vCPU целиком (`Vcpu`): `LongModeState` (регистры, MSR-ячейки, активность, `sti_shadow`, `nmi_blocked`), модель legacy-режима AP (`Real`/`Protected`), `guest_efer`, `guest_cr2`, гостевые ячейки STAR/LSTAR/CSTAR/SYSENTER/SYSCALL_MASK/KERNEL_GS_BASE, FP, `skip_first_boundary`, `cpuid`. (2) Весь `PairState`: каждый LAPIC (регистры, IRR/ISR/TMR, TPR/SVR, ожидающие INIT, SIPI, NMI), его таймер с `EventQueue` (сроки в виртуальном времени, generation, `start`/`initial`/`divide`/`lvt`), счётчик `timer_writes`, и маршрут IOAPIC (записи, `dest`, Remote IRR INTIN4 и SCI, уровни линий). (3) Состояние планировщика: позиция генератора квантов, позиция расписания, текущий vCPU по кругу, прерванный ход, счётчики, запись ходов, простой. (4) Время остановки `now_ns` — только как сторож. **Решение: RAM в снимок S6-24 не входит.** Не входят также устройства платы (UART, PIT, PIC, ACPI PM, NVMe, дисплей), источник IRQ платы, `pci_config_address` и кэши хоста (JIT, переводы): это `S9-24`/`S9-25`, и снимок процессорного комплекса над ушедшей вперёд памятью согласован только сам с собой. Часы не восстанавливаются: время стоит всё время остановки, а сроки в очередях — абсолютное виртуальное время.

**Снятие и восстановление.** `VmSnapshot::take(board, scheduler, safe_point)` требует остановленный планировщик (`paused`) и `SafePoint` этой машины (то же число vCPU, часы на `safe_point.now_ns`, активность каждого vCPU совпадает с подтверждённой, а `Boundary`/`RepRestart` совпадают с признаком `skip_first_boundary` vCPU: `RepRestart` — ровно у vCPU с признаком): иначе `NotStopped`/`StaleSafePoint`; частичного снимка не бывает. `restore(board, scheduler, safe_point)` — `SafePoint` остановки ЭТОЙ машины, как у `take` (число подтверждений должно равняться числу vCPU, иначе `StaleSafePoint`) — только в ТУ ЖЕ машину и только остановленную. Снимок хранит идентичность часов исходной платы, которая проверяется и при отсутствии таймеров LAPIC (`DifferentMachine`). Число vCPU, число LAPIC и их таймеры (одни и те же часы и частота) должны совпадать, часы должны стоять на `now_ns` снимка (`ClockMoved`). Проверки происходят до записи. При успехе восстанавливаются LAPIC и маршрут через `restore_pair`, затем vCPU и планировщик. Восстановление в новую машину не определено. После восстановления и `resume` прогон побайтно равен прогону без остановки (проверено на probe v2, сценарий `exchange`: счётчики, ходы, байты UART, хеш RAM, время).

**Отступление от плана S6-24: маршрут IOAPIC.** План называет IOAPIC среди того, чего S6-24 не трогает; здесь маршрут IOAPIC (записи, `dest`, Remote IRR) входит в снимок и сбрасывается в `reset_cpu_complex`. Обоснование — Remote IRR: после INIT цели его некому снять (EOI не придёт), и без сброса маршрута сброс платы оставлял бы `reset_io`/`set_dest` заблокированными навсегда. Ограничение: `PairState::reset_all` обнуляет и уровни линий (`high_line`, `intin2_high`, `sci_asserted`), которыми управляют устройства, а не процессорный комплекс; сброс без последующего `restore` может потерять уровень линии, которую удерживает устройство. До S9 и до первого использования вне теста это блокер для S6-21 (источник IRQ платы поверх маршрута): либо уровни линий не сбрасываются, либо устройства переутверждают их после сброса. Код уровней не менялся.

**Сброс платы (закрывает перенос S6-13).** `LoadedFirmwareBoard::reset_cpu_complex(bsp_state, bsp_guest_efer)`: BSP получает переданное состояние входа (плата его не хранит), каждый AP — `Vcpu::new_ap` (`WaitForSipi`, real-mode состояние включения, `EFER = 0`, ячейки в нуле), независимо от того, чем он был (Running, Halted, long mode, Shutdown); CPUID каждого vCPU сохраняется. Все LAPIC, не только BSP, возвращаются в состояние включения: ожидающие INIT, SIPI, NMI, IRR/ISR/TMR, TPR и взведённый таймер очищаются, `timer_writes = 0`, маршрут IOAPIC — в состояние сброса вместе с Remote IRR (`PairState::reset_all`, всё или ничего, не блокируется Remote IRR). RAM, устройства, источник IRQ и часы не трогаются. `LocalApicHandle::reset_checked`/`reset_pair`/`reset` остались сбросом одного LAPIC и маршрута (прежняя семантика, тесты IOAPIC не менялись); сбросом платы они не являются.

**Решение по Remote IRR.** Решение владельца 2026-11-16 сохранено: INIT не снимает Remote IRR (его снимает только EOI); после INIT цели ISR пуст, EOI не придёт, `reset_io`, `reset_local` цели и `set_dest` этого маршрута отказывают — как и раньше. Артефакт устранён сбросом платы: он снимает Remote IRR всех маршрутов (на железе это делает hardware reset), после него `reset_io` и `set_dest` проходят. Проверено тестом `the_board_reset_clears_the_remote_irr_that_the_init_of_the_target_left`. Вне сброса платы маршрут с Remote IRR при INIT цели по-прежнему заблокирован, пока цель не пошлёт EOI: если нужно освобождать его и без сброса, это новое решение владельца.

**Решение по broadcast `0xFF` в IOAPIC.** Отказ, семантика не менялась: запись в старший dword записи перенаправления с APIC ID, которого нет среди vCPU (`0xFF` включён при любом числе vCPU), — ошибка устройства `UnsupportedConfiguration` до изменения состояния (`PairState::set_dest`), по правилу SMP_CONTRACT §5–§6 «не перечислено — ошибка»; поддержки broadcast-доставки (logical/«всем») версия 1 не содержит. Как и раньше, ошибка записи в MMIO гостя фатальна для прогона (гость, пишущий `0xFF`, останавливает его явной ошибкой, а не доставляет всем). Тест `the_broadcast_destination_0xff_is_refused_on_every_entry_and_changes_nothing` (две и один vCPU, записи 2, 4, 9, 23). Снятие ограничения — отдельное решение владельца вместе с logical-маршрутизацией (SMP_CONTRACT §10).

**Тесты.** `smp_scheduler::tests`: `stop_all_takes_the_acknowledgement_of_every_vcpu_the_sleeping_ones_included`, `a_stop_without_the_acknowledgement_of_every_vcpu_is_no_stop` (отрицательный: не подтвердивший vCPU 1 — `Timeout`, список, STOP снят, состояние и часы прежние; два не подтвердивших; vCPU, приведённый `settle`, даёт остановку), `a_run_cut_by_the_budget_goes_on_as_the_run_that_was_not_cut` (seed 0..40 и фиксированные кванты, 7 точек обрыва, через `state`/`reset`/`set_state` и без), `a_plain_run_after_a_cut_starts_a_new_turn_and_only_the_resume_continues_the_cut_one`, `a_budget_that_ends_inside_a_partial_rep_goes_on_with_the_same_vcpu` (M1), `the_stop_inside_a_turn_leaves_the_rest_of_the_quantum_for_the_run_that_follows`, `the_state_of_the_scheduler_of_another_number_of_vcpus_is_refused`; `vm_snapshot::tests`: сброс (AP Running -> `WaitForSipi`, пусто во всех LAPIC, BSP на состоянии входа, повторный SIPI запускает AP), Remote IRR, снятие и восстановление (ожидающие Fixed, NMI, INIT, SIPI в обоих LAPIC), отказы (`NotStopped`, `StaleSafePoint` в том числе при расхождении `Boundary`/`RepRestart` с `skip_first_boundary`, `ClockMoved`, `VcpuCount`), `an_ipi_sent_before_the_stop_and_not_yet_accepted_is_accepted_once_after_the_restore` (IPI в IRR цели до остановки: сброс его теряет, восстановление возвращает как `(IRR, не ISR)`, после `resume` `acknowledge()` даёт именно этот вектор один раз, затем `None`, повторные стоп и восстановление второго приёма не дают), таймер LAPIC (one-shot на vCPU 1 и periodic на vCPU 0, взведены до остановки, после сброса очередь пуста, после восстановления сроки те же, срабатывание одно и в тот же момент, как у машины, которую не останавливали), отказ восстановления состояния другого набора LAPIC; `io_apic::tests` (0xFF); `tests/stage6_smp_probe.rs` (probe v2 exchange: остановка и восстановление в точках обрыва по seed, с `reset` и без, итог равен непрерывному; отказы сессии; `ClockMoved` проверяется по имени ошибки; хотя бы одна точка обрыва имеет `lapic_pending(1) == Some(true)` и хотя бы одна — неотработанный запрос в IRR AP, иначе тест не доказывает заявленного). Контроль от противного (проверен): без переноса остатка хода (`carry`) падают два теста планировщика (`a_run_cut_by_the_budget_...`, `the_stop_inside_a_turn_...`); без записи vCPU при восстановлении падает `the_state_of_a_stopped_machine_survives_a_reset_and_comes_back_whole`. Тест probe с отключённым `carry` не прогонялся.

**Не входит.** Снимок RAM и устройств, формат, версии, manifest (`S9-24`/`S9-25`), барьер с вводом-выводом (`S9-28`), восстановление в новую машину, остановка параллельных потоков и ожидание подтверждения от потока (S6-34), сброс устройств платы (машинный сброс S9), снимок `ClockOwner` как значения (он состоит из времени, которое стоит), доставка снимка через `od-runner` и публичный API кроме сессии probe.

---

## S6-25 — биты A/D через compare-and-replace

Дата: 2026-11-17. Чекбокс S6-25 в `STAGE_6.md` не отмечен. Реализация: `crates/od-machine/src/page_walk.rs` (`TableReplace`, `compare_replace_table_entry`, `commit_leaf_dirty`, `TABLE_REPLACE_RETRY_LIMIT`, `TrackedAccessError::TableEntryChanged`), `crates/od-machine/src/physical_memory.rs` (`PhysicalMemory::compare_replace_u64`), `crates/od-isa/src/real_mode_exec.rs` и `paging.rs` (`RealModeMemory::or_u8`, `commit_touches`).

**Единственный путь записи A/D.** Каждая запись accessed (в обходе) и dirty (после проверок операнда) идёт через seam `table_writer(memory, адрес, expected, replacement, callback)`, по умолчанию `compare_replace_table_entry`, то есть через один вызов `PhysicalMemory::compare_replace_u64`: читает 8 байт, при равенстве `expected` пишет `replacement` через `write_u64` (callback один раз после фиксации, generation страниц растёт), иначе возвращает наблюдённое значение и ничего не меняет. Обычных `write_u64` таблицы в производственном коде `page_walk.rs` не осталось (тест `leaf_dirty_is_committed_only_by_the_helper`). Чтение записи — один `read_u64` на 8 байт. Тело `compare_replace_u64` — единственное место, которое S6-31 заменит host-операцией; вызывающий код не меняется. Предусловие: адрес выровнен на 8 для будущего host-CAS (S6-31); вызывающие передают выровненные адреса (база 4K + index·8), `debug_assert!` в теле. Путь AP в legacy-режиме (`od-isa::paging`, A/D — один байт) идёт только через `RealModeMemory::or_u8` (S6-25; заменено в S6-31b; OR не отнимает чужой бит); умолчание — read+write; S6-31: заменить на CAS по значению обхода (см. S6-31 шаг 5), атомарный OR без сравнения недостаточен: может превратить обнулённую PTE в ненулевую.

**Повтор обхода.** Accessed: если replace не прошёл (запись изменилась после чтения), обход повторяется с этого же уровня: запись читается заново, все проверки (present, reserved, PS) выполняются заново, трасса хранит значение последнего чтения; A ставится поверх нового значения, чужие биты сохраняются. Dirty: replace вычисляется от наблюдённого значения (`observed | A | D`), если изменились только биты A/D (чужой D — ничего не пишется, `Ok(None)`; чужая очистка A — ставятся оба); если изменился любой бит кроме A/D (адрес, P, R/W, U/S, PS, NX, а также программные биты 9–11 и 52–62), ничего не пишется и возвращается `TableEntryChanged { bit: Dirty }`: no operand effect, prefix A committed (префикс записанных A на вышестоящих уровнях остаётся, как при `TableWrite` или `#PF`, по S4-09). Ограничение до S6-31: гонка с программными битами Linux (soft-dirty и т.п.) под потоками даст отказ; заменить повтором или терпимостью к не-трансляционным битам в S6-31. Повторный обход всего доступа для D не делается: отступление от плана («с этого уровня») — проверки прав вышестоящих уровней при D уже выполнены, а изменение трансляции листа — ограниченный отказ профиля, как `LtrTransactionError::TranslationChanged`. Предел подряд проваленных replace — `TABLE_REPLACE_RETRY_LIMIT` = 16, дальше `TableEntryChanged`: при исчерпании лимита на уровне N+1 префикс A на уровнях выше остаётся записанным (согласовано с S4-09: префикс committed A, как при `TableWrite` или `#PF`), эффекта операнда нет; в режиме чередования replace не проваливается (инструкция неделима), предел защищает только от неисправного backend.

**Отступление: путь AP (закрыто в S6-31b).** Путь AP (`od-isa::paging`) читал запись побайтно (`read_entry` — восемь `read_u8`) и писал касания отложенным OR без сравнения (`commit_touches` через `RealModeMemory::or_u8`) — отступление от шагов 1–2 плана; в режиме чередования корректно (неделимая инструкция). Заменено в S6-31b на CAS по значению обхода (раздел «S6-31b»); атомарный OR без сравнения недостаточен: может превратить обнулённую PTE в ненулевую.

**Что не меняется.** Семантика A/D из S4-09 (A на каждом использованном уровне, D только в листе и только при записи, префикс committed A при позднем отказе, порядок A, D, операнд). `TrackedAccessReport` и `AdUpdate` прежние. Отдельный путь `GuestMemory::atomic_rmw_integer` (операнд `LOCK`/`XCHG`, S6-20) не используется обходом: обход работает на физическом уровне `PhysicalMemory`, а не на `GuestMemory`; общая точка замены для S6-31 — тела `PhysicalMemory::compare_replace_u64` (S6-31) и `RealModeMemory::or_u8` (S6-25; заменено в S6-31b). Descriptor accessed/busy не A/D таблиц (S6-26).

**Тесты** (`page_walk::tracked_access_tests`): `accessed_replace_that_fails_repeats_the_level_and_keeps_the_other_bit`, `dirty_replace_that_fails_is_recomputed_from_the_observed_entry`, `dirty_replace_that_fails_on_another_vcpus_dirty_bit_writes_nothing_more`, `dirty_replace_refuses_when_a_translation_bit_changed`, `accessed_replace_that_always_fails_stops_at_the_retry_limit`, `two_vcpus_updating_the_same_entries_lose_no_accessed_or_dirty_bit` (два vCPU, перебор 4 пар видов доступа x индекс вторжения 0..8 между вызовами seam первого; различных вторжений 5 (X — чтение: 4 вызова seam, плюс «Y после X») и 6 (X — запись: 5 вызовов, плюс «Y после X»), индексы за последним вызовом повторяют «Y после X» и учтены как повторы, итог равен последовательному в любом порядке; тот же перебор со слепой записью старого значения теряет обновление); `physical_memory::tests::compare_replace_u64_replaces_only_a_matching_value`; `od-isa` `paging::tests::commit_touches_goes_through_the_or_operation_only`.

**Не входит.** Host-атомики и порядок памяти (S6-31/S6-32), общая RAM (S6-30), DMA и другие потоки, формат снимка, литмус-fixture A/D (в корпус S6-19 не добавлялась: план S6-25 её не требует, перебор расписаний сделан тестом `od-machine` на уровне seam).

---

## S6-30a — хранение RAM и функции доступа

Дата: 2026-11-17. Чекбокс S6-30 в `STAGE_6.md` не отмечен (задача идёт тремя срезами). Реализация: `crates/od-machine/src/ram_storage.rs` (`RamStorage`), `crates/od-machine/src/physical_memory.rs` (поле `Region::backing`, функции `load_*`/`store_*`/`load_bytes`/`store_bytes`). Нормативный источник — [`MEMORY_MODEL.md`](MEMORY_MODEL.md) (разделы «Rust atomics», «Выбор по замеру (S6-06)», «Атомарность и разделённые доступы»).

**Тип хранения.** `backing` региона RAM, ROM и Framebuffer — `RamStorage`: `Vec<AtomicU64>` (выделение выровнено на 8, запись через общую ссылку определена; блокировок нет) и `lead = start % 8` неиспользуемых байт в начале. Благодаря `lead` host-адрес байта со смещением `o` сравним с гостевым адресом по модулю 8, поэтому естественно выровненный гостевой доступ шириной 1, 2, 4, 8 — всегда естественно выровненный host-доступ (выравнивание проверяется по host-указателю, а не выводится из гостевого адреса: требование MEMORY_MODEL.md, «Rust atomics»). Регион с начальным адресом не кратным 8 остаётся корректным.

**Функции `&self` (рядом со старыми).** `PhysicalMemory::load_u8/u16/u32/u64`, `store_u8/u16/u32/u64`, `load_bytes`, `store_bytes`:
- выровненный доступ 1, 2, 4, 8 байт — одно host-обращение этой ширины (`AtomicU8/16/32/64::from_ptr`, `SeqCst` — выбор S6-06; порядок пересматривается в S6-32);
- невыровненный скалярный доступ делится на возрастающие по адресу однобайтовые обращения (правило разделения MEMORY_MODEL.md; атомарности и целостности нет, D8);
- массовое копирование (`load_bytes`/`store_bytes`, а также `read_bytes`/`write_bytes`) — последовательность независимых `Relaxed`-обращений до 8 байт: голова до естественной границы, тело по 8, хвост; ширина каждого шага — наибольшая из 8, 4, 2, 1, выровненная в этой точке и помещающаяся в остаток. Копия в целом неатомарна и не упорядочена: другой поток может увидеть любой префикс; выровненное копирование ровно 1, 2, 4, 8 байт — одно обращение, побайтового копирования выровненных доступов нет;
- границы, виды регионов и ошибки (`Unmapped`, `AccessCrossesRegion`, `AddressOverflow`, `EmptyRange`, `UnreadableRegion`, `ReadOnlyRegion`) и их порядок те же, что у `read_bytes`/`write_bytes`; результат побайтно совпадает со старым путём.

**Чего нет (намеренно; закрыто в S6-30b, см. раздел ниже).** Функции `store_*`/`store_bytes` в срезе S6-30a не повышали поколения страниц и не вызывали уведомление «запись зафиксирована»: счётчики оставались обычными целыми за `&mut self`; в S6-30b они атомарны, а `store_*` повышают их и уведомляют, как старый путь. До S6-30c ни один производственный путь эти функции не вызывает (`#[allow(dead_code)]`), ими нельзя писать RAM, за которой следит кеш кода. Старые `read_bytes`/`write_bytes`/`read_u*`/`write_u*` остаются, используют то же хранение (через массовое копирование) и по-прежнему повышают поколения и вызывают уведомление; `GuestMemory`, адаптеры, `&mut self` не менялись. Состояние на S6-30c: старый путь удалён, адаптеры держат `&PhysicalMemory`, трейт остаётся на `&mut self` (раздел S6-30c ниже).

**`compare_replace_u64` (S6-25).** Не менялась: читает и пишет через старый путь (до S6-30c; теперь `load_u64`/`store_u64`); на новом хранении оба обращения — по одному выровненному 8-байтовому (тест `compare_replace_u64_is_two_single_accesses_on_the_new_storage`, в том числе в регионе с нечётным началом). Хост-CAS — S6-31.

**Решение (Opus/high, ревью S6-30a: принять с условием).** Тип хранения — вариант A: `Vec<AtomicU64>` с байтовыми атомарными видами 1/2/4/8 (окончательно). Полный текст решения и инварианты — [`MEMORY_MODEL.md`](MEMORY_MODEL.md), абзац «Решение S6-30a» в разделе «Rust atomics». Кратко: одновременных обращений из разных host-потоков к RAM до S6-34 нет; неконкурентные обращения разной ширины разрешены моделью Rust; конкурентные обращения разной ширины — принятое отклонение от абстрактной машины Rust, опирающееся на однократную атомарность выровненного доступа на host (Arm ARM B2.2.1, SDM Vol. 3A §11.1.1). Инварианты: (а) после создания к байтам RAM нет неатомарных доступов (нет `&[u8]`/`&mut [u8]`, `ptr::copy`, memcpy; S6-30c удаляет выдачу `&mut`); (б) каждое обращение выровнено на host и не шире 8 байт, 16 байт — только примитив S6-31; (в) `unsafe` только в макросах `ram_storage.rs`; (г) целевые host — aarch64 и x86_64 (в `ram_storage.rs` стоит `compile_error!` для остальных `target_arch`: на этих двух сборка не меняется, на прочих host модуль теперь не собирается; прежняя сборка с `Vec<u8>` на них собиралась); (д) конкурентные тесты держат одну ширину на место, смешение ширин под Miri не запускается, его проверяет host-litmus S6-36. Вариант B (`AtomicU64` целиком) отклонён ценой, а не текстом плана: узкие записи — CAS-цикл, узкие `LOCK` RMW — через CAS вместо `LDADD*`. Вариант C (всё побайтно `Relaxed`) отклонён: выровненный доступ перестаёт быть целостным. Порядок `SeqCst` и `Relaxed` для массового копирования — кандидат на пересмотр в S6-32.

**Поправка S6-41b к инварианту (в) (текст; код не менялся).** Вступает в силу вместе с inline-эмиттером памяти S6-41b (б), которого пока нет: сегодня сгенерированный код RAM напрямую не трогает (блок обращается к памяти только через invocation-local helper с копированием, `memory_bridge.rs`), и инварианты (а)–(д) выполняются в прежнем виде. Если inline-доступ пойдёт по сырому host-указателю на байт RAM, то: (в′) `unsafe` разрешён в трёх местах и больше нигде: макросы `ram_storage.rs`; одна функция в `ram_storage.rs`, которая выдаёт host-указатель на естественно выровненный диапазон байт региона RAM и документирует его срок действия (хранение региона не перемещается и не освобождается, пока жив любой блок, исполняющий этот указатель; поэтому указатель не кладётся в кэш за пределы жизни `PhysicalMemory`); вызов опубликованного кода (`PublishedCode` в `od-host`, уже существующий). (а′) Инвариант (а) для сгенерированного кода сохраняется по построению, а не проверкой типов: код обращается к байтам RAM только инструкциями, атомарными на host (`LDR`/`STR`, `LDAR`/`STLR`, `LDADD`/`SWP`/`CAS`/`LDSET`/`LDCLR`) шириной 1, 2, 4 или 8 байт по естественно выровненному адресу; `ldp`/`stp`, `memcpy`-подобные последовательности и невыровненный доступ в нём запрещены (невыровненный доступ — выход в helper, split `LOCK` — шлюз S6-34). (б′) Указатель выдаётся только после проверки прав и выравнивания по host-указателю (требование MEMORY_MODEL, «Rust atomics»), а повышение поколения страницы и отчёт «запись зафиксирована» делает код блока после записи (JIT_CONTRACT S6-40, обязательство 1). Пока эти условия не выполнены и не проверены тестами, inline-доступ к RAM в блок не допускается.

**Невыровненный скаляр: нормативно побайтно.** Новый путь (`load_u*`/`store_u*`) делит невыровненный скалярный доступ на возрастающие однобайтовые обращения. Старый путь (`read_u*`/`write_u*` через массовое копирование) делит тот же доступ кусками (голова до границы, тело, хвост). Оба деления законны по MEMORY_MODEL.md («Атомарность и разделённые доступы»: невыровненный доступ видим по частям, без гарантии целостности); нормативно побайтное деление — у нового пути, куски старого — следствие реализации и исчезают с удалением старого пути в S6-30c. Результаты (байты и ошибки) у обоих путей совпадают.

**Учёт `RamLimits`.** Лимит считает байты региона по карте; выделение держит ещё `lead` (до 7) байт впереди и хвост до границы 8 байт, в сумме не более 14 байт на регион. Разница ничтожна, код учёта не менялся.

**Тесты.** `physical_memory::tests` (+8) и `ram_storage::tests` (+7): совпадение со старым API побайтно на случайных последовательностях (три зерна по 20 000 операций с границами регионов, дырами, ROM, Reserved, Framebuffer, нечётными началами регионов), плюс теневая модель `Vec<u8>` (каждая успешная запись нового API применяется к ней, каждое успешное чтение сверяется с ней, итоговый дамп равен ей), общее хранение старого и нового путей, границы и ошибки, одно обращение нужной ширины на обоих путях (журнал обращений), выравнивание host при каждом `lead`, перебор смещений и длин массового копирования, отсутствие разрыва (tearing) при конкурентных выровненных обращениях, `PhysicalMemory: Send + Sync`.

## S6-30b — счётчики и уведомления

Дата: 2026-11-17. Чекбокс S6-30 в `STAGE_6.md` не отмечен (S6-30c впереди). Реализация: `crates/od-machine/src/physical_memory.rs` (`Region::page_generations`, `PhysicalMemory::commit_write`, `code_page_generation`, `store_*`), `crates/od-machine/src/bus.rs` (`capture_code_page`). Нормативно: [`MEMORY_MODEL.md`](MEMORY_MODEL.md) («Текущее владение RAM и commit», «Согласование адресных пространств (S6-17)»).

**Счётчики.** `Region::page_generations` — `Vec<AtomicU64>`, по счётчику на 4 KiB страницу региона RAM (начальное значение 1; у ROM, Framebuffer и дыр счётчиков нет, как раньше). Все обращения — через `&self`: нет `&mut`, нет `Mutex`/`RwLock`, межпоточных примитивов сверх атомиков нет.

**Одна транзакция записи.** Старый путь (`write_bytes`, `write_u*`, `compare_replace_u64` через `write_u64`) и новый (`store_u8/16/32/64`, `store_bytes`) идут через один приватный `PhysicalMemory::commit_write`. Порядок: (1) разбор и проверка (`EmptyRange`, `Unmapped`, `AccessCrossesRegion`, `AddressOverflow`, `ReadOnlyRegion` для ROM и дыр без хранения) — ошибки те же и в том же порядке, что у `write_bytes`; (2) Framebuffer — запись без поколений и без уведомления; (3) проверка, что ни один счётчик затронутых страниц не равен `u64::MAX` (`CodeGenerationExhausted`, до единого записанного байта); (4) запись данных в хранение; (5) `fetch_add(1, Release)` по каждой затронутой странице; (6) один вызов `committed_write(range)` с диапазоном записи. Отклонённая запись не меняет ни байты, ни счётчики и не уведомляет. `store_*` принимают тот же параметр `committed_write: impl FnMut(GuestPhysicalRange)`, что и `write_*` (сигнатура расширена по сравнению с S6-30a; тесты S6-30a поправлены механически, ожидания не менялись, кроме одного, см. отчёт).

**Порядок памяти и правило читателя.**
- Повышение — `Release`, после записи данных; чтение (`code_page_generation`) — `Acquire`. Читатель, увидевший поколение `g`, видит все байты, записанные до повышения до `g`. Данные пишутся `SeqCst` (выровненные скаляры) или `Relaxed` (массовое копирование): `Release` повышения упорядочивает их все.
- Проверка на исчерпание (`Relaxed`) и повышение — два отдельных атомарных шага. При параллельных писателях одной страницы они не атомарны вместе; переполнение `u64` потребовало бы 2^64 записей в одну страницу (порядка веков при 10^9 в секунду), поэтому проверка не усиливалась; последовательные пути чередоваться не могут.
- **Читатель, кеширующий производное от байтов RAM, читает поколение ДО байт** и хранит его вместе с данными. Запись, попавшая между ними, оставляет сохранённое поколение старше страницы, и следующая проверка отбрасывает запись кеша (лишнее обновление, не устаревшее попадание). Обратный порядок (байты, затем поколение) допускает пару «старые байты, новое поколение» и недопустим. `capture_code_page` (`bus.rs`) переставлен: поколение читается до `read_bytes`; перечитывание первой страницы в `tracked_code_snapshot_two_pages_with_callback` уже шло в этом порядке. В последовательном пути результат тот же.
- Поколение повышается после данных, поэтому в окне между ними читатель может увидеть новые байты со старым поколением: это безопасная сторона (кеш отбросится при следующей проверке).

**Уведомление «запись зафиксирована».** Две дороги; в режиме чередования проверены тестами, межпоточная дорога тестом не доказана (см. ниже). (1) Внутри хода писателя: `committed_write` вызывается один раз после повышения поколений (в колбэке `code_page_generation` уже показывает новое значение, а `load_*` — новые байты); кеш писателя получает его через `FetchTranslationCache::note_write` (политика `WriteWatch` сбрасывает записи, `GuestOnly` только считает). (2) Между vCPU: колбэк вызывается в потоке писателя и чужому кешу не передаётся; чужой кеш (`FetchTranslationCache`) и код-кеш могут увидеть запись через атомарное поколение страницы: `seal`/`revalidate` с `generation_of = code_page_generation` сбрасывают записи, чьи кадры таблиц сменили поколение; кеш кода сравнивает `backing_generation` снимка с текущим. Это верно только в режиме чередования: `FetchTranslationCache::seal` (`fetch_translation_cache.rs`) читает поколения кадров таблиц в конце вызова, уже после того как обход прочитал байты записей таблицы при `insert` (см. `revalidate`). Это порядок «байты, затем поколение», который правило читателя выше запрещает: в параллельном режиме устаревшая запись с политикой `WriteWatch` может пережить запись другого vCPU. В режиме чередования это безопасно (`seal` выполняется до хода другого vCPU). Тест `a_write_to_a_table_frame…` (запись из другого потока) завершает поток через `join`, то есть доказывает happens-before, а не гонку внутри вызова; межпоточная дорога тестом не доказана. Что не закрыто: кеша перевода, переживающего квант платы, на плате нет (S6-17), а `revalidate` вызывается на границе вызова цикла, не на каждом обращении. Правило: в параллельном режиме кеш с `WriteWatch` запрещён, пока поколения кадров таблиц не берутся при `insert` ДО чтения записей обхода и не проверяются перед использованием записи. Параллельный режим — не закрыто, см. S6-34.

**Тесты.** `physical_memory::tests`: старый путь против нового на тех же 3 x 20 000 случайных операциях — теперь с одинаковыми уведомлениями на каждом шаге и одинаковыми поколениями всех страниц окна (теневая модель `Vec<u8>` та же); колбэк после повышения, страница на стыке (обе повышены до колбэка); отклонённые записи без поколений и вызовов; исчерпанное поколение на обоих путях; 4 потока на 4 своих страницах и общая пятая (итоговое поколение точно, у владельца ровно +1 на запись, наблюдатель видит монотонность и связь «поколение `g` — слово не меньше `g - 1`»). `fetch_translation_cache::tests`: запись в кадр таблицы сбрасывает запись кеша и по колбэку (`note_write`), и по поколению (запись из другого потока, обе политики).

**Не входит.** Адаптеры на общей ссылке и удаление `&mut` и старого пути (сделано в S6-30c, раздел ниже); host-CAS (S6-31); порядок TSO и fences (S6-32); конкурентный доступ к шине и устройствам (S6-33).

## S6-30c — адаптеры на общей ссылке и удаление `&mut`

Дата: 2026-11-17. Чекбокс S6-30 в `STAGE_6.md` не отмечен (закрывается после ревью S6-30c и всех трёх срезов). Реализация: `crates/od-machine/src/physical_memory.rs` (единственный путь доступа к RAM — `load_*`/`store_*`/`compare_replace_u64` на `&self`), `crates/od-machine/src/page_walk.rs`, `paged_memory_adapter.rs`, `ap_legacy_exec.rs`, `bus.rs` и др. (`&mut PhysicalMemory` заменён на `&PhysicalMemory`). `od-isa` не менялся.

**Что удалено.** Весь старый путь `PhysicalMemory`: `read_bytes`, `read_u8/16/32/64`, `write_bytes`, `write_u8/16/32/64` (последние на `&mut self`). Выдачи `&mut` на байты RAM не было и раньше (хранение `RamStorage` — атомарные слова, `&[u8]`/`&mut [u8]` к нему нет); остальной `&mut` в публичном API RAM был у записей (`&mut self`) и у адаптеров (`&mut PhysicalMemory`) — он убран. `load_*`/`store_*` стали `pub` и единственными; `compare_replace_u64` теперь `&self` (читает через `load_u64`, пишет через `store_u64`, поведение и ошибки те же). `load_rom_bytes` остаётся на `&mut self`: единственный писатель ROM, шаг загрузчика до того, как память становится общей; гостю он недоступен. Все вызовы в `od-machine` и `od-bench` переведены (`read_*` в `load_*`, `write_*` в `store_*`; вызовов в `od-runner` не было); тесты S6-30a/b «старый путь против нового» заменены проверкой по независимым оракулам (см. тесты).

**Адаптеры.** Формы адаптера, ходящие в RAM напрямую (`AdapterBus::Ram`, `AdapterBus::RamMmio` в `paged_memory_adapter.rs`, `BoardRam` в `ap_legacy_exec.rs`, адаптеры `pio_step`, `paged_pio_step`, `paged_probe`, `translation_probe_cache`), держат `&PhysicalMemory`; каждый vCPU строит свой адаптер (на попытку инструкции), общим у них только RAM. На производственном пути (плата) ссылка на RAM добывается через `MachineBus::split_for_set_step_of_vcpu(&mut self)` и `split_for_pio_step(&mut self)`, и её время жизни привязано к исключительному заимствованию шины: адаптеры двух vCPU на плате одновременно не живут. Одновременно они существуют только в коде, который держит `&PhysicalMemory` напрямую (тесты). Форма `AdapterBus::Machine(&mut MachineBus)` и `IdentityMemoryAdapter` по-прежнему держат `&mut MachineBus`: шина владеет устройствами и MMIO, модель устройств не менялась (S6-33); сами `split_for_*` отдают `&PhysicalMemory`.

**Владение адаптером (S6-30c, нормативно; решение Opus high).** `od_isa::GuestMemory` — интерфейс адаптера одного vCPU, а не RAM. Методы принимают `&mut self`: исключительность относится к состоянию адаптера (протокол обхода, ошибки, отчёт A/D, колбэк `committed_write`, кеш выборки), а не к байтам RAM. Адаптер (1) держит RAM только как разделяемую ссылку на `PhysicalMemory` (`&PhysicalMemory`; в параллельном режиме допустим `Arc<PhysicalMemory>`): никогда не `&mut PhysicalMemory`, не `&mut [u8]` к RAM и не копию байт; (2) создаётся и используется одним потоком своего vCPU и не разделяется между vCPU, `Send` адаптеру не требуется. Общее между vCPU — только `PhysicalMemory` (все обращения `&self`, `Send + Sync`) и, после S6-33, устройства под своими блокировками. **Условие пересмотра:** если S6-33, S6-34 или S6-40+ потребуют одного экземпляра `GuestMemory`, вызываемого из нескольких потоков, или повторного входа в тот же адаптер во время его вызова, трейт переводится на `&self` отдельным срезом, одним изменением всех реализаций.

**Поведение.** Результаты, ошибки границ и их порядок, порядок уведомлений и поколений прежние (тесты S6-30a/b и весь набор `od-machine` без изменения ожиданий; контрольный Linux побайтно). Различие по реализации: производственные чтения скаляров теперь идут через `load_u*` (одно `SeqCst`-обращение для выровненного 1/2/4/8, ascending-байты для невыровненного), раньше — через массовое копирование (`Relaxed`, куски). В режиме чередования результат тот же; порядок пересматривается в S6-32.

**Публичная поверхность.** Doctests на `PhysicalMemory` (`physical_memory.rs`): вызов `store_u64`/`compare_replace_u64`/`load_u64` через `&PhysicalMemory` компилируется, `read_u8`, `write_bytes` — `compile_fail,E0599` (методов нет).

**Тесты.** `physical_memory::tests`: `access_functions_match_the_region_rules_and_a_byte_model` (3 зерна по 20 000 операций; вместо сравнения со старым путём — независимый оракул правил регионов `reference_access`, написанный по карте теста, а не по коду `PhysicalMemory`: ошибки и их порядок; плюс байтовая модель `Vec<u8>` окна, уведомления на каждом шаге; поколения — каждые 64 шага и в конце) заменил `new_access_functions_match_the_old_api_byte_for_byte`; `stores_through_one_shared_reference_are_seen_through_another` (две общие ссылки на одну память, записи и чтения любой ширины) заменил `old_and_new_paths_share_one_storage`; `boundary_errors_follow_the_region_rules` заменил `new_functions_keep_the_boundary_errors_of_the_old_ones`; `aligned_accesses_are_single_host_accesses` и `an_exhausted_generation_rejects_the_write` потеряли только половину про старый путь; остальные тесты (`compare_replace_u64_*`, уведомления, поколения, конкурентные, границы регионов, ROM, Framebuffer) переведены механически (`read_*` в `load_*`, `write_*` в `store_*`), их ожидания не менялись. `paged_memory_adapter::tests::adapters_of_two_vcpus_share_one_ram` (+1): два адаптера цикла набора одновременно на одной памяти (запись одного видна другому, у каждого свой журнал `committed_write`), затем четыре потока, в каждом свой адаптер на той же памяти. Doctests на `PhysicalMemory` (+3): вызов через `&PhysicalMemory` компилируется; `read_u8` и `write_bytes` — `compile_fail,E0599`. Итог: `od-machine` полным прогоном — 1873 passed (было 1869: +1 тест адаптеров, +3 doctests), 0 failed, 8 ignored (прежние); `od-isa` — 4490 passed, 0 failed (код не менялся).

**Не входит.** Конкурентный доступ к шине и устройствам, порядок блокировок (S6-33); host-CAS в `compare_replace_u64`/`or_u8` (S6-31); порядок TSO и fences (S6-32); параллельный режим целиком, перенос взятия поколения кадров таблиц в `insert` (S6-34); в параллельном режиме ссылка на RAM добывается без `&mut MachineBus`: `PhysicalMemory` вне блокировки устройств (отдельным полем или `Arc`), в согласии с S6-33 «не брать блокировку шины на доступ к RAM» (S6-33/S6-34).

## S6-31 — LOCK на host-примитивах

Дата: 2026-11-17. Чекбокс S6-31 в `STAGE_6.md` не отмечен. Реализация: `crates/od-machine/src/ram_storage.rs` (`atomic_compare_exchange`, `atomic_swap`, `atomic_fetch_add` на ячейке), `physical_memory.rs` (`PhysicalMemory::atomic_compare_exchange/swap/fetch_add/rmw`, `compare_replace_u64`, `split_lock_refusals`, `MemoryAccessError::MisalignedAtomic`, `commit_write_with`), `page_walk.rs` (`RamAccessOperation::AtomicTarget`, `TrackedAccessOutcome::Located`, `locate_atomic_target`), `paged_memory_adapter.rs` (переопределение четырёх атомарных методов `GuestMemory` в `PagedMovMemoryAdapter`). `od-isa` не менялся (только комментарии). Основание выбора — [`MEMORY_MODEL.md`](MEMORY_MODEL.md), «Выбор по замеру (S6-06)».

**Шаг 1. Примитивы 1/2/4/8 байт.** Rust std на ячейке `RamStorage` (`AtomicU8/16/32/64::from_ptr`), всегда `SeqCst`: `compare_exchange`, `swap`, `fetch_add` (S6-06 выбрал `fetch_add(SeqCst)`; на aarch64 с LSE компилятор даёт `CASAL*`, `SWPAL*`, `LDADDAL*`, на x86_64 — `lock cmpxchg`, `xchg`, `lock xadd`; фактические инструкции — в отчёте, раздел S6-31). Ячейка выровнена на host: выровненный гостевой адрес даёт выровненный host-адрес благодаря `lead` (S6-30a); `locked_cell` проверяет это жёстко (`assert!`, не `debug_assert!`: `from_ptr` на невыровненном адресе — неопределённое поведение). Никакого `Mutex`/`RwLock`, никакой глобальной блокировки. Единый API в `PhysicalMemory` (все на `&self`, все с `committed_write`): `atomic_compare_exchange`, `atomic_swap`, `atomic_fetch_add` (по одной host-операции) и `atomic_rmw(address, width, update)` — цикл host-`compare_exchange` над чистой функцией `update` (формы `AND`/`OR`/`BTS`/`BTR`/`XADD`/`CMPXCHG`, которые инструкции передают замыканием через `atomic_rmw_integer`; `update` может вызываться повторно, сохраняется значение удачной попытки; результат записывается всегда, как у S6-20). Отдельные `fetch_and/or/xor` не добавлены: вызвать их некому (инструкции отдают непрозрачное замыкание), цикл сравнения даёт то же без нового примитива. Порядок записи тот же, что у любой записи RAM: проверка (`Unmapped`, `AccessCrossesRegion`, `ReadOnlyRegion` для ROM и дыр, `CodeGenerationExhausted`), host-операция, `Release`-повышение поколения страницы, один вызов `committed_write(range)`. Как у x86 `CMPXCHG`, операция считается записью и при несовпадении сравнения (поколение растёт, колбэк один раз): так делал последовательный путь S6-20 (записывал старое значение), наблюдатели (кеши кода) видят те же уведомления. `Framebuffer` — обычная память без поколений и колбэка; MMIO и ROM не обслуживаются (ниже).

**Шаг 2. 16-байтный примитив — не сделан (отступление от шага 3 плана).** Пара 16-байтных инструкций не сделана; цена — выравнивание хранения `RamStorage` на 8 байт (S6-30a) и новый inline asm; принято Opus high. `CMPXCHG8B`/`CMPXCHG16B` в декодере нет (вопрос D6: однопроцессорную семантику добавляет срез этапа 2; этап 6 даёт атомарный примитив, когда инструкция появится); единственный другой потребитель по контракту — `LOCK` на невыровненном операнде внутри выровненного 16-байтного блока — не порождается ядром Linux (его `.smp_locks` — выровненные формы). Пояснение к цене: 16-байтная пара (`LDAXP`/`STLXP`, S6-06; `cmpxchg16b` на x86_64) требует 16-байтного выравнивания host-адреса, а `RamStorage` выровнен на 8 (`Vec<AtomicU64>`): примитив потребовал бы перекладки хранения на сверхвыровненные ячейки и нового аудита `unsafe` (inline asm). Условие пересмотра: появление `CMPXCHG16B` (D6) либо гостевого кода с невыровненным `LOCK` в пределах 16 байт.

**Шаг 3. Невыровненный `LOCK`.** Операнд, не выровненный по своей ширине (в том числе пересекающий страницу или кеш-линию), — явная ошибка со счётчиком: `PagedMovMemoryAdapter` отказывает ДО обхода таблиц (ни accessed, ни dirty бит не ставится, колбэков нет) ошибкой `MemoryError::Backend("locked operation of N bytes at 0x… is not naturally aligned: a split lock is refused…")`, прогон гостя останавливается как при любой ошибке памяти; `PhysicalMemory::split_lock_refusals()` растёт на каждый отказ (на физическом уровне те же операции дают `MemoryAccessError::MisalignedAtomic`, тоже со счётчиком). Это изменение поведения относительно S6-20: последовательный путь исполнял невыровненный `LOCK` как чтение и запись (верно в режиме чередования); теперь он отказывает, чтобы никакой путь не исполнял split lock неатомарно (D8: остановка всех vCPU — S6-34, шаг 8; пара 16-байтных операций — шаг 2 выше). Побайтно-контрольный прогон Linux изменения не показал (ядро не выполняет невыровненных `LOCK`). **Обновление S6-32:** отказ со счётчиком остался только в параллельном режиме; в режиме чередования невыровненный `LOCK` снова исполняется последовательно, как в S6-20 (раздел «S6-32», шаг 5).

*Отступление от шага 3 плана (пара 16-байтных инструкций не сделана; цена — выравнивание хранения `RamStorage` на 8 байт (S6-30a) и новый inline asm; принято Opus high).* Отказ распространяется не только на `LOCK`, но и на формы, которые набор исполняет как `LOCK`: `XADD`/`CMPXCHG` без префикса, DS-формы и `XCHG`. Для форм без префикса на железе split lock нет и невыровненный операнд законен; это расширение отказа сделано сознательно. Условие возврата: в режиме чередования «остановка всех vCPU» выполняется сама собой (один поток, неделимая инструкция); при появлении переключателя режима режим чередования снова исполняет split `LOCK` последовательно, как в S6-20. **Условие возврата ЗАКРЫТО в S6-32** (раздел «S6-32», шаг 5: в режиме чередования невыровненный `LOCK` исполняется последовательно, счётчик не растёт; в параллельном режиме отказ со счётчиком остаётся до S6-34 шаг 8).

**Шаг 4. Порядок.** Все `LOCK`-операции и `XCHG` с памятью — `SeqCst` на host (полный барьер на x86_64 и `…AL` с acquire-release на aarch64; загрузки и записи других обращений этого потока не переупорядочиваются с ними ни в какую сторону, поскольку они одновременно acquire и release, а общий порядок `SeqCst` задан между всеми такими операциями). Порядок обычных обращений и `MFENCE` — S6-32, здесь не меняется.

**Шаг 5. A/D при обходе (две точки).** (1) `PhysicalMemory::compare_replace_u64` (путь `page_walk`: `compare_replace_table_entry` → seam `table_writer`) — теперь одна host-операция `AtomicU64::compare_exchange` на ячейке без предварительного чтения: при равенстве `expected` записывает `replacement`, повышает поколение, зовёт колбэк один раз и возвращает `Ok(Ok(()))`; иначе ничего не меняет, колбэка нет, результат `Ok(Err(observed))`; невыровненный адрес — `MisalignedAtomic` (счётчик). Протокол повтора S6-25 не менялся (accessed: повтор уровня, dirty: пересчёт от наблюдённого). Уточнение: `commit_leaf_dirty` теперь допускает конкурентное изменение битов, которые трансляция игнорирует (9–11, 52–62: soft-dirty и подобные у ядра), как и A/D — пересчитывает от наблюдённого значения и повторяет, вместо отказа `TableEntryChanged` (этого S6-25 ждал от S6-31); изменение любого трансляционного бита по-прежнему отказ без записи. Тест `dirty_replace_absorbs_a_change_of_the_software_bits`. (2) **Путь AP** (`od-isa::paging::commit_touches` / `RealModeMemory::or_u8`) — в S6-31 не заменён (требовал изменения трейта `od-isa`); **заменён в S6-31b** (раздел «S6-31b» ниже: `RealModeMemory::compare_replace_entry`, `Translation::touches` несёт прочитанное значение, протокол отбрасывания при изменении трансляционных битов). Что было в S6-31: пара «адрес младшего байта, маска» и `or_u8` как чтение с записью; атомарный OR без сравнения недостаточен (может превратить обнулённую PTE в ненулевую).

**Транзакция `LOCK` в адаптере.** При CPL0 `RamAccessOperation::AtomicTarget { width }` — трансляционная половина: тот же эффект на таблицы страниц, что имела последовательная пара «чтение операнда, запись операнда» (обход на чтение: accessed-биты и ошибки чтения без бита W; обход на запись: права записи, accessed, ошибки записи; проверка области: RAM и Framebuffer допустимы, ROM и дыры — `ReadOnlyRegion`, MMIO — `DeviceUnavailable` до dirty-бита и до устройства; затем dirty-бит листа), но без обращения к данным; результат `TrackedAccessOutcome::Located(физический адрес)`. Затем одна host-операция по этому адресу и одно уведомление `committed_write` (в список эффектов адаптера или колбэк, как у записей обхода). Адаптер принимает ту же ширину, что принимали его `read` и `write` этого профиля (`locked_width`), иначе — `unsupported` как раньше. Счётчики обходов (`WalkCounter`) считают две операции — чтение и запись, как прежде. **Отступление от SDM (унаследовано от S6-20).** Обход для не присутствующей страницы — чтение без бита W (SDM требует W=1 для RMW); последствие — лишний `#PF` на чтение перед `#PF` на запись, итог тот же; исправление изменило бы путь Linux, отложено. Контрольный Linux-прогон S6-10 побайтно прежний (отчёт S6-31); число `LOCK`-операций, исполненных в этом прогоне, не измерялось; путь подтверждён тем, что формы без префикса и DS-формы идут через `atomic_rmw_integer`, а контрольный прогон побайтно совпал.

**Тесты.** `ram_storage::tests` (+3): одна host-операция на ширину, lead и смещение (журнал `traced`), оборачивание на ширине, соседние байты не тронуты; невыровненная ячейка — паника до обращения; 4 потока, точная сумма и счётчик на сравнении по каждой ширине. `physical_memory::tests` (+4, один переименован): тот же перебор с моделью байтов на 30 000 случайных операций (результат, число уведомлений и поколений), отказы (невыровненный на всех ширинах и со всеми смещениями, ROM, Reserved, дыра, Framebuffer, исчерпанное поколение), 4 потока на каждой ширине (точная сумма `fetch_add`, взаимное исключение спин-замком на сравнении с обычными загрузкой и записью, счётчик через замыкание), `compare_replace_u64` под 4 потоками (ни один бит и ни одно приращение не потеряно, число уведомлений равно числу успехов); `compare_replace_u64_is_one_host_compare_exchange_on_the_new_storage` вместо «двух обращений» S6-25. `paged_memory_adapter::tests` (+4): эквивалентность последовательной паре «чтение, запись» (таблицы, данные, запись о `#PF`, состояние ошибок и последовательность уведомлений) для 2 состояний A/D, 6 листьев, 4 ширин, 5 операций; отказы невыровненных (счётчик, ни одного изменённого бита) и MMIO/ROM; 4 адаптера в 4 потоках на общей RAM с чистыми A/D (гонка за биты таблиц и точные счётчики на каждой ширине); 4 потока интерпретируют настоящие `lock cmpxchg` (с повтором), `lock xadd`, `lock inc` и `xchg`-спинлок вокруг обычного увеличения. `page_walk::tracked_access_tests` (+1): программные биты. Проверено от противного: замена host-операций на «загрузка, затем запись» роняет все пять межпоточных тестов. Litmus режима чередования (`litmus_interleaving`) работает на собственной плоской памяти `LitmusRam` (`Vec<u8>`) и по построению не затронут; он проходит.

**Не входит.** Порядок обычных обращений и `MFENCE` (S6-32); 16-байтный примитив и split lock с остановкой всех vCPU (S6-34 шаг 8; в S6-31 отказ со счётчиком в обоих режимах, с S6-32 — только в параллельном, в режиме чередования последовательно); путь AP (сделан в S6-31b, раздел ниже); конкурентный доступ к шине и устройствам (S6-33); параллельный режим (S6-34).

**Выровненный `LOCK ADD` при CPL3.** Для пользовательского `AtomicTarget` выполняется один обход с правом записи. Он проверяет присутствие, U/S и R/W для листьев 4 КиБ и 2 МиБ. При #PF в коде выставлены W и U. После проверки всего физического диапазона фиксируется D-бит листа, затем существующий атомарный путь общей RAM изменяет операнд. При отказе обхода A-биты предков могут быть зафиксированы, а операнд и D-бит сохраняются. [ISSUE-572](../KNOWN_ISSUES.md#issue-572--s6-22-lock-add-общего-счётчика-в-cpl3).

## S6-31b — CAS по значению обхода на пути AP

Дата: 2026-11-17. Чекбоксы S6-31 и S6-31b в `STAGE_6.md` не отмечены. Реализация: `crates/od-isa/src/paging.rs` (`Touch`, `Translation::touches`, `read_entry`, `commit_touches`), `crates/od-isa/src/real_mode_exec.rs` (трейт `RealModeMemory`), `crates/od-isa/src/protected_exec.rs` (`ProtectedCpu::step`), `crates/od-machine/src/ap_legacy_exec.rs` (`BoardRam`). Протокол — решение ревью S6-31 (Opus, high); здесь он записан как реализованный.

**Трейт.** `RealModeMemory::or_u8` удалён (его поведение — «прочитать байт, записать байт» — было отступлением S6-31, заменено). Добавлены два метода. `read_u64(linear) -> Option<u64>` — одна 8-байтная загрузка записи (по умолчанию восемь `read_u8`; `BoardRam` — одна выровненная `PhysicalMemory::load_u64`; не выровненный или выходящий из области RAM адрес читается побайтно, host-загрузки по невыровненной ячейке нет). `compare_replace_entry(linear, expected, replacement) -> Option<Result<(), u64>>` — сравнение и замена 8 байт записи: `Some(Ok(()))` — заменено; `Some(Err(observed))` — запись изменилась, ничего не записано; `None` — запись не читается или записываемый байт не записываем (ничего не записано). Отступление от формы плана (`Result<(), u64>`): внешний `Option` нужен для «нечитаемо/не записываемо» (в плане это случай без значения `observed`). Умолчание — чтение и запись только отличающихся байт (для однопоточных реализаций: образы в тестах); `BoardRam` переопределяет его одной host-операцией `PhysicalMemory::compare_replace_u64` (S6-31, `SeqCst`), поэтому изменение записи другим vCPU не затирается. Невыровненный адрес записи или запись вне одной области RAM в `BoardRam` — `None` (счётчик split lock не растёт: адрес записи таблицы всегда выровнен на 8 по построению).

**`Translation::touches`.** Вектор `Touch { at, read, mask }`: адрес записи, значение (8 байт), которое прочитал обход, и маска (A, для листа записи — A и D). `read_entry` читает запись одним `read_u64`.

**`commit_touches(mem, touches) -> usize`** (число отброшенных). Для каждого касания: `expected = read`; `replacement = expected | mask`; если они равны — ничего не делать; `compare_replace_entry(at, expected, replacement)`. При `Err(observed)`: сравнить `observed` и `read` по трансляционным битам (все, кроме A, D, 9–11 и 52–62; те же игнорируемые биты, что у `commit_leaf_dirty` в `page_walk.rs`, профиль без protection keys). Равны — `expected = observed`, повторить (A/D ставится поверх наблюдённого значения, чужие биты сохраняются). Трансляционный бит изменился, или число неудач больше 16 (`TOUCH_RETRY_LIMIT`, как `TABLE_REPLACE_RETRY_LIMIT`) — касание **отбрасывается**: A/D не ставится, запись остаётся такой, какой её сделал другой писатель. Использование устаревшей трансляции допустимо для A и для остальных касаний (аналог устаревшей записи TLB); для D аналогия неточна, см. «Ограничение порядка D на пути AP» ниже. Обнулённая запись не становится ненулевой: `P` — трансляционный бит. Одно касание — одна запись; касания одной записи из нескольких обходов одной инструкции (загрузка dword идёт обходом на байт) независимы: второе и следующие видят A, поставленный первым, не пишут повторно.

**Инструкция не повторяется.** `ProtectedCpu::step` вызывает `commit_touches` после `step_collecting`, когда эффекты инструкции уже зафиксированы; повтор продублировал бы их. Поэтому отброшенное касание — тихое отсутствие A/D, число отброшенных возвращает `commit_touches` (шаг его не использует; счётчика на `ProtectedCpu` нет). Отказ из плана S6-25 («повтор или отказ») здесь — отказ без повтора.

**Что не менялось.** Поведение однопоточного пути (то же состояние записи для 2 состояний A/D x листья, тест); путь Linux (адаптер платы, `page_walk`) не затронут, контрольный прогон побайтно прежний (отчёт); отступления S6-31 остаются: 16-байтная пара не построена; обход для не присутствующей страницы — чтение без бита W (S6-20). `can_write` в `translate` по-прежнему проверяет младший байт записи: для 8-выровненной записи в одной области RAM это то же, что 8 байт.

**Ограничение: `can_write` только младшего байта (m1).** `translate` проверяет `can_write` только адреса записи (младший байт), а `BoardRam::compare_replace_entry` требует все 8 байт в одной области RAM. Недостижимо на картах с границами областей по страницам. При границе внутри 8-байтной записи `commit_touches` паникует на `None` (`expect`), как прежний `or_u8` через `assert!`. Исправление — проверка записываемости всей записи в `translate` (отдельная задача, не S6-31b).

**Порядок D на пути AP (m3).** Внутри `ProtectedCpu::step` запись операнда предшествует `commit_touches`. При общем доступе к таблицам вне исключительной секции другой писатель может изменить трансляцию после записи операнда. Тогда CAS отбросит D, и наблюдатель сможет получить PTE без D для уже записанной страницы. Требование к публикации для legacy AP на штатной параллельной плате выполняет исключительная секция вокруг всего шага. [Порядок D на пути AP в S6-34](#порядок-d-на-пути-ap-шаг-10).

**Тесты.** `od-isa`: `paging::tests` — запись только через compare-and-replace (журнал); эквивалентность прежнему OR для 2 состояний A/D x 6 листьев (4 КиБ и 2 МиБ x чтение/запись/выборка); изменение трансляционного бита между чтением и CAS (новый кадр, RW снят, P снят, обнуление, XD) — касание отбрасывается, запись как у другого писателя, остальные три касания применены; изменение только A/D и игнорируемых бит — повтор поверх наблюдённого; обнулённая запись остаётся нулевой, бесконечное вмешательство заканчивается по пределу; умолчания трейта на образе. `protected_exec::tests` — шаг `MOV EAX, [..]` с вмешательством перед первым CAS: инструкция исполнена один раз (EAX, EIP), запись изменённой трансляции без A, запись с изменёнными игнорируемыми битами получила A. `od-machine`: `ap_legacy_exec::tests::entry_cas` (4) — одна загрузка и один CAS у `BoardRam` (поколение, ROM, дыра, невыровненный адрес без счётчика), эквивалентность прежнему OR на RAM платы, отбрасывание и поглощение на RAM платы, межпоточный обход AP (400 000 раундов `translate` и `commit_touches` с записью) против второго потока, меняющего младший байт (P, RW, US, PS/PAT) и игнорируемые биты той же записи через CAS, сбрасывающего A/D и обнуляющего запись: ни одно изменение не потеряно, обнулённая запись остаётся нулевой (проверка без маски A/D: ноль не становится 0x20/0x60; отброшенных касаний в прогоне больше нуля — гонка действительно происходит). Проверено от противного: замена host-CAS в `BoardRam::compare_replace_entry` на загрузку и запись роняет межпоточный тест (3 запуска из 3; замена возвращена).

**Не входит.** Порядок обычных обращений (S6-32); параллельный режим и потоки vCPU (S6-34); 16-байтный примитив; W-бит для не присутствующей страницы; запись A-бита дескриптора (`write_u8` через `store_bytes`) — не запись таблицы страниц, не менялась.

## 16-байтовый доступ CPL3 в LinuxDirect (S6-22)

`PagedMovMemoryAdapter` полного набора LinuxDirect допускает явный 16-байтовый операнд CPL3 в RAM или framebuffer. Для каждой затронутой линейной страницы выполняется проверка прав гостя и всего физического диапазона в пределах этой страницы. Второй диапазон проверяется при пересечении границы 4 КиБ. Физические диапазоны могут находиться в несмежных кадрах.

При чтении данные сначала собираются в промежуточный массив. Результат публикуется после полного чтения. При записи обход сохраняет трансляции обоих диапазонов и фиксирует необходимые D-биты до записи операнда. Запись идёт по сохранённым адресам, в том числе когда первый диапазон изменяет таблицу страниц. При #PF на любой странице данные операнда сохраняются, A-биты завершённых обходов учитываются обычным способом. Порядок доступа и уведомления о записи определяет `PhysicalMemory`. [ISSUE-566](../KNOWN_ISSUES.md#issue-566--s6-22-movq-из-памяти-в-xmm-останавливает-glibc).

## S6-32 — порядок обычных доступов и fences

Дата: 2026-11-17. Чекбокс S6-32 в `STAGE_6.md` не отмечен. Реализация: `crates/od-machine/src/physical_memory.rs` (`ExecutionMode`, `PhysicalMemory::set_execution_mode`, макрос `by_mode!`, `fence`, `host_fences_issued`), `ram_storage.rs` (порядок — параметр массового копирования и `try_load_u8`), `bus.rs` и `firmware_board.rs` (переключатель на уровне платы), `paged_memory_adapter.rs` (`PagedMovMemoryAdapter::fence`), `crates/od-isa/src/memory.rs` (`FenceKind`, `GuestMemory::fence`), `crates/od-isa/src/execute.rs` (набор CPL0 вызывает `fence`). Нормативный источник выбора — [`MEMORY_MODEL.md`](MEMORY_MODEL.md), «Выбор по замеру (S6-06)» и «Решение S6-30a».

**Шаг 1. Порядок по режимам.** Выбор не нов: параллельный режим берёт замеренный S6-06 вариант (Rust `SeqCst` для обычных loads/stores 1/2/4/8 байт), режим чередования не ставит барьеров, как требует план.

| Класс обращения | Режим чередования (`Interleaved`, по умолчанию) | Параллельный режим (`Parallel`) |
|---|---|---|
| Скалярные `load_u8/16/32/64`, `store_u8/16/32/64` (выровненные — одно host-обращение, невыровненные — возрастающие однобайтовые) | `Relaxed`: один host-поток исполняет все vCPU и всех писателей, наблюдателя нет | `SeqCst` (S6-06: загрузка 2,237x к relaxed, запись 1,047x в пределах шума; более простой вариант из допустимых) |
| Массовое копирование (`load_bytes`, `store_bytes`: куски операнда обхода, DMA, загрузчик патчей) и чтение байта кадра `read_ram_frame_u8` | `Relaxed` (как было) | `SeqCst` на каждом host-обращении копирования; копия в целом по-прежнему не атомарна (решение S6-30a не меняется) |
| `LOCK`-операции, `XCHG`, `compare_replace_u64` | `SeqCst` (не менялось) | `SeqCst` (не менялось) |
| Поколения страниц | `Release` повышение, `Acquire` чтение (не менялось) | то же |
| `MFENCE` | нет операции | `fence(SeqCst)` |
| `SFENCE` | нет операции | `fence(Release)` |
| `LFENCE` | нет операции | `fence(Acquire)` |

Обоснование параллельной колонки. Порядок `SeqCst` для обычных обращений достаточен по смыслу (MEMORY_MODEL.md, таблица «Гарантии и классы доступа») и запрещает IRIW; он сильнее x86-TSO (запрещает разрешённый гостю SB `0,0`) — принятая цена, записана в S6-06. `Acquire`-only недостаточен для IRIW, `Release`-only не задаёт общего порядка (MEMORY_MODEL.md, «Rust atomics»). Host-fence после обычного `SeqCst`-доступа ничего не добавляет к модели; барьеры оставлены, потому что контракт их называет и барьер не убирается «потому что тест проходит» (план, «Не делать»). Соответствие видов fence порядкам — по строкам таблицы MEMORY_MODEL.md: полный барьер `MFENCE` — `SeqCst`; `SFENCE` (store→store) — `Release`; `LFENCE` (наблюдаемая часть load→load/store) — `Acquire`; отдельная модель локальной сериализации исполнения `LFENCE` не строится (MEMORY_MODEL.md: нужна только если она наблюдаема). Не опираемся на аппаратный TSO: порядок задан host-порядками Rust, а не поведением x86 хоста. Реализация порядка: одна ветка по режиму с константным порядком в каждой ветви (`by_mode!`), а не `Ordering` как значение времени выполнения: первая версия с значением времени выполнения давала +5–7 % на baseline `page_walk_compare` (данные `s6-32/first-version/`, отчёт S6-32).

Уточнение к S6-30c. В S6-30c записано, что производственные чтения скаляров идут через `load_u*`. Это верно для обращений шины (`read_ram`) и выборки байта, но операнд обхода (`page_walk.rs`: чтение и запись операнда шириной 1/2/4/8 и куски операнда на границе страницы) идёт через `load_bytes`/`store_bytes`. Поэтому порядок режима применён и к массовому копированию (строка таблицы выше): иначе обычный доступ гостя в параллельном режиме остался бы `Relaxed`. Это пересмотр отметки S6-30a «порядок массового копирования — кандидат на пересмотр в S6-32»: решено — порядок копирования следует режиму, `Relaxed` остаётся только в режиме чередования и при загрузке ROM (`load_rom_bytes`, до того как память становится общей).

**Шаг 2. Переключатель.** Один, на уровне машины: `od_machine::ExecutionMode { Interleaved, Parallel }` (по умолчанию `Interleaved`). Значение хранится в `PhysicalMemory` (поле `mode`, обычное, не атомарное: `set_execution_mode(&mut self, …)` вызывается, пока память принадлежит платформе единолично, до того как адаптеры vCPU берут разделяемую ссылку). Путь установки: `LoadedFirmwareBoard::set_execution_mode` -> `MachineBus::set_execution_mode` -> `PhysicalMemory::set_execution_mode`. Адаптеры vCPU и интерпретатор не получают режим: адаптер держит `&PhysicalMemory` и вызывает `load_*`/`store_*`/`fence`, которые сами выбирают порядок. Трейт `GuestMemory` режима не знает. Параллельный режим этим срезом НЕ включается ни в одном запуске: потоки vCPU — S6-34; переключатель выставлен только в тестах (в том числе в межпоточных, где обычные обращения обязаны быть упорядочены) и в замере ниже. Однопоточный результат в обоих режимах побайтно одинаков (тесты ниже).

**Шаг 3. Fences.** До среза набор CPL0 исполнял `MFENCE`, `SFENCE`, `LFENCE` как пустые операции, не обращаясь к памяти. Теперь исполнение в `od-isa` (`step_configured`, ветки набора) вызывает `GuestMemory::fence(kind)` с видом `FenceKind::{Full, Store, Load}` по одному вызову на исполненную инструкцию. Это новый provided-метод трейта, по умолчанию `Ok(())` (no-op): все существующие реализации (адаптеры, тестовые памяти) компилируются без изменений и не получают эффекта. Оболочка границы выборки в `execute.rs` пробрасывает вызов во внутреннюю память. Переопределяет метод только `PagedMovMemoryAdapter` (адаптер набора CPL0): `PhysicalMemory::fence(kind)` — host-fence в параллельном режиме, ничего в режиме чередования. Вызов не меняет байты RAM, поколения, уведомления `committed_write` и состояние CPU, кроме RIP и счётчика (как раньше); ошибка памяти сообщается обычным путём и fence не считается исполненным. Метод `mfence(context)` (S2-21g, вне набора CPL0) не менялся и набором больше не вызывается для `MFENCE`; вне набора `MFENCE` по-прежнему вызывает его, `LFENCE`/`SFENCE` вне набора не поддержаны, как раньше. Тестовые изменения: новые тесты `od-isa`; в `cpl0_profile_set.rs` тестовая память получила журнал вызовов `fence`.

**Шаг 4. Обход таблиц.** Каждая запись таблицы читается одним 8-байтовым обращением: основной обход (`page_walk::read_table_entry` -> `PhysicalMemory::load_u64`, A/D — одна `compare_replace_u64` на бит) и путь AP (`paging::read_entry` -> `RealModeMemory::read_u64`, с S6-31b). Остальные `read_u8` в `protected_exec.rs` — выборка команды, данные и байты дескрипторов (не записи таблиц страниц); побайтных чтений записей таблиц не осталось. Закрыто тестами со счётчиком обращений (ниже).

**Норма fence.** Гостевые `MFENCE`/`SFENCE`/`LFENCE` исполняются вызовом `GuestMemory::fence(kind)`; любая обёртка `GuestMemory` обязана пробрасывать этот вызов (как `FetchCapture`). Режим чередования: host-операции нет (граница инструкции даёт порядок). Параллельный режим: host-fence только средствами `std::sync::atomic::fence` (не inline-asm `DMB`), не слабее: `MFENCE` → `fence(SeqCst)`, `SFENCE` → `fence(Release)`, `LFENCE` → `fence(Acquire)`; усиливать допустимо, ослаблять нельзя. Обоснование: `MFENCE` — единственный, запрещающий store→load (SB 0,0); `SFENCE`/`LFENCE` — части release/acquire. Пока обычные обращения `SeqCst`, все три избыточны; они нижняя граница: если обычный порядок ослабят до `Acquire`/`Release`, `fence(SeqCst)` для `MFENCE` станет обязательным. Отображение в хост: aarch64 — `DMB ISH` / `DMB ISH` / `DMB ISHLD`; x86_64 — `MFENCE` / нет / нет; определяет компилятор, опора на аппаратный TSO не допускается. Не моделируется: сериализация `LFENCE` (интерпретатор завершает инструкцию до следующей; `RDTSC` — виртуальное время), WC/non-temporal у `SFENCE` (non-temporal исполняется как обычная запись).

**Шаг 5. Split LOCK по режимам.** Условие возврата из S6-31 (отступление «split lock — отказ в обоих режимах») закрыто этим срезом. `PagedMovMemoryAdapter::locked_target` возвращает `LockedTarget::{Host { width, physical }, Serial}`. Невыровненный операнд (в том числе через границу кеш-линии или страницы): в режиме чередования — `Serial`, и каждый из четырёх методов `atomic_*_integer` исполняет тело умолчания трейта (S6-20): `read_integer` → вычисление → `write_integer` через собственные `read`/`write` адаптера, то есть с теми же обходами, A/D и уведомлениями, что до S6-31; неделимость даёт то, что планировщик переключает vCPU только на границе инструкции; счётчик `split_lock_refusals` не растёт. `CMPXCHG` при несовпадении, как в S6-20, всё равно пишет прежнее значение (проверка прав записи, D-бит, уведомление). В параллельном режиме — отказ со счётчиком до обхода, как в S6-31 (до остановки всех vCPU, S6-34 шаг 8). Ширина, не допускаемая профилем адаптера для чтения и записи, по-прежнему отказ `unsupported` в обоих режимах (проверка до выравнивания). Выровненный операнд идёт на host-атомике в обоих режимах без изменений.

**Не входит.** Параллельный режим целиком, потоки vCPU, перенос взятия поколений кадров таблиц (S6-34); шина и устройства (S6-33); остановка всех vCPU для split `LOCK` в параллельном режиме (S6-34 шаг 8; до этого там отказ со счётчиком); 16-байтная пара.

**Тесты.** `od-isa` (+5): набор CPL0 вызывает `fence` один раз с видом инструкции для каждой из трёх форм, последовательность вызывает их в порядке программы, `MFENCE` набора не доходит до `mfence` (S2-21g), отказ `fence` не исполняет инструкцию и не меняет состояние; обход читает запись одним `read_u64` и не вызывает `read_u8` (4 КиБ и 2 МиБ x чтение, запись, выборка). `od-machine` (+11): `physical_memory::tests` (4) — режим по умолчанию и чтение переключателя; порядок каждого host-обращения (скалярного, невыровненного, массового) равен порядку режима (журнал порядков `ram_storage::traced_orders`); тот же результат, тот же журнал уведомлений, те же поколения всех страниц окна и тот же дамп RAM в обоих режимах на 3 x 20 000 случайных операциях; `fence` даёт host-fence только в параллельном режиме и не меняет ни байта, ни поколения; `page_walk::tracked_access_tests` (1) — каждая запись таблицы затрагивается только 8-байтовыми обращениями по своему адресу, для чтения, записи и выборки в обоих режимах (журнал `ram_storage::traced`); `paged_memory_adapter::tests` (5) — `fence` адаптера доходит до памяти (счётчик host-fence 0 и 3), RAM не меняется; набор CPL0 исполняет настоящие байты `MFENCE`/`SFENCE`/`LFENCE` с обычными записью и чтением: состояние, число исполненных, каждый байт RAM, журнал `committed_write` и поколения одинаковы в обоих режимах, host-fence 0 и 4; невыровненный `LOCK` в режиме чередования равен последовательной паре (результат, ошибки, таблицы двух страниц, данные, журнал уведомлений; по таблице выровненного эквивалента: старое значение и записанное; счётчик отказов 0) на 3 ширинах x 3–4 смещения (внутри строки, через строку, через страницу) x 5 операций x 4 пары листьев x 2 состояния A/D; несовпавший `CMPXCHG` пишет прежнее значение (D-бит обеих страниц, уведомление), на странице только для чтения — `#PF` и без записи; в параллельном режиме тот же операнд отказ со счётчиком (3 вызова, 3 отказа), выровненный операнд той же памяти исполняется; `firmware_board::tests` (1) — один переключатель на плате, по умолчанию чередование, значение доходит до памяти шины. Межпоточные тесты `physical_memory` (4 теста, 5 запусков потоков), `paged_memory_adapter` (3), `ap_legacy_exec` (1), `fetch_translation_cache` (1) переведены в параллельный режим: они моделируют параллельный запуск, и их обычные обращения обязаны быть упорядочены (в режиме чередования по определению нет другого потока).

## Порядок блокировок (S6-33)

Нормативный порядок захвата блокировок `od-machine` для потоков vCPU (S6-34). Источники: `bus.rs` (`MachineBus::lock_devices`), `apic_pair.rs`, `local_apic.rs`, `virtual_clock.rs`, `*_irq_source.rs`, тесты `bus/shared_tests.rs`.

**Блокировки по уровням.** Берутся только сверху вниз; на каждом шаге не более одной блокировки одного уровня.

| Уровень | Блокировка | Где |
|---|---|---|
| 1 | Блокировка таблицы устройств шины: одна на шину, `MachineBus::devices` (`Mutex<Vec<Box<dyn BusDevice>>>`) | `bus.rs`; берёт поток, у которого есть только `&MachineBus` |
| 1 | Блокировка состояния источника IRQ: по одной на источник (`PitPicIrqSource`, `PitIoapicIrqSource`, `UartPicIrqSource`, `UartIoapicIrqSource`) | `*_irq_source.rs` |
| 2 | Блокировка состояния одного устройства: APIC (`Mutex<PairState>`: все LAPIC и маршруты IOAPIC вместе), PIT (`state`, `port_tick`), PIC и ELCR, UART, ACPI PM, дисплей, NVMe | `apic_pair.rs`, `pit_8254.rs`, `pic_8259.rs`, `uart16550.rs`, `acpi_pm.rs`, `display_device.rs`, `nvme/` |
| 3 | Блокировка виртуальных часов | `virtual_clock.rs` |

Блокировки шины и источника — одного уровня и друг в друга не вкладываются (ни в каком порядке): под блокировкой шины не вызывается источник, под блокировкой источника не берётся блокировка шины.

**Правила (нормативно).**

1. **Только вниз.** Шина (или источник) → устройство → часы. Единственное вложение между блокировками устройств и часов (уровень 2 → 3): APIC → часы (`Timer::arm_at`, `due_at`, `dispatch_timer`: замок часов держится, пока замыкание `with_dispatch_guard` работает над уже взятым состоянием APIC и других замков не берёт). Вложения 1→2 (шина → устройство; источник IRQ → устройство: UART-источники через `uart.interrupt_line` и `io_apic.set_intin4` / `pic.raise_irq`) допускаются; PIT-источник → `clock.snapshot` (1→3) допускается. Обратных путей нет (часы → устройство, устройство → шина, APIC → источник).
2. **Устройство не вызывает устройство.** Под блокировкой устройства не берётся блокировка другого устройства, шины или источника, и не вызывается метод чужого `Handle`. `BusDevice` получает только `DeviceAccess` и не имеет ссылки на шину; связь устройств идёт через источники IRQ, которые держат хэндлы и обращаются к устройствам по одному, не вложенно (`UartIoapicIrqSource::pending_vector` сначала `sample`, который отпускает свои замки, затем вызывает источник PIT). Единственная пара устройств с общей блокировкой — LAPIC и IOAPIC (одно состояние `PairState`, потому что уровневый запрос связывает IRR/TMR адресата и Remote IRR маршрута).
3. **Блокировка устройства не пережидает инструкцию.** Замок берётся на одно обращение и отпускается до возврата из него; поток vCPU не засыпает (`HLT`, ожидание SIPI, `park`) и не ждёт другой поток, держа замок.
4. **Доставка IPI.** Целевое правило для любого разбиения APIC по LAPIC (S6-38): запись ICR читается и проверяется (`decode_icr`) под блокировкой своего LAPIC, значение копируется, блокировка отпускается, затем доставка идёт под блокировкой адресата; две блокировки LAPIC одновременно не держатся. Сегодня блокировка APIC одна на все LAPIC и маршруты IOAPIC (см. S6-33 ниже), запись ICR разбирается и доставляется под ней целиком (`LocalApic::write_from_cpu`, `0x300`): пары замков LAPIC нет, правило выполнено по построению, а доставка атомарна относительно прочих операций APIC. Запрос IOAPIC в LAPIC (`set_intin4`, `pulse`) идёт под той же блокировкой.
5. **RAM вне блокировок.** `PhysicalMemory` — отдельное поле шины, доступное по `&self` (`MachineBus::physical_memory`) без блокировки таблицы устройств и без какой-либо другой; обращения к RAM состоят из атомарных операций (S6-30, S6-32) и блокировок не берут. Поэтому обращение к RAM допустимо из-под любой блокировки уровней 1–3, а поток, держащий блокировку устройств, не задерживает обращений к RAM других потоков.
6. **Исключительный путь не блокирует.** Код, которому принадлежит шина (`&mut MachineBus`: `split_for_*`, `read_port`, `write_port`, `read_physical`, `write_physical`, `access_tracked`), добывает таблицу устройств через `Mutex::get_mut` без захвата блокировки: исключительность доказывает заимствование. Поэтому режим чередования не платит за блокировку шины. Блокировку берут только методы с `&self`: `lock_devices`, `read_port_shared`, `write_port_shared`, `read_mmio_shared`, `write_mmio_shared`.
7. **Отравление.** Все блокировки берутся с `unwrap_or_else(PoisonError::into_inner)`: паника устройства под блокировкой остаётся паникой хоста в своём потоке (не скрывается), но не делает шину недоступной для остальных; это уже было правилом всех устройств. Условие для S6-34: паника в потоке vCPU завершает прогон для всех vCPU как ошибка хоста; машина на частично обновлённом состоянии не продолжает работу; `into_inner` нужен только затем, чтобы не зависли остальные потоки и можно было снять диагностику.

**Проверка.** `bus::shared_tests` (ниже): отсутствие пути от APIC, IOAPIC и источников к блокировке шины проверяется тем, что их операции завершаются, пока другой поток держит блокировку шины; направление «шина → APIC» — работой через `lock_devices` и ожиданием обращения к LAPIC на занятой блокировке APIC; взаимные записи ICR двух потоков — через шину и без неё, с тайм-аутом.

**Что меняется при дроблении (S6-38).** Дробить блокировку шины (по устройству) или APIC (по LAPIC) можно только по замеру. Новые блокировки получают место в таблице выше; правило 4 становится обязательным буквально; порядок захвата нескольких блокировок одного уровня — по возрастанию индекса устройства; для нескольких LAPIC правило 4 («две блокировки LAPIC одновременно не держатся») уточняется условием (4) решения ревью S6-33 (ниже): для одного адресата — прочитать и проверить под своей блокировкой, отпустить, доставить под блокировкой адресата; для shorthand и для §5 (б) `SMP_CONTRACT.md` — захват блокировок адресатов по возрастанию APIC ID либо пересмотр §5 (б) решением владельца.

## S6-33 — шина и устройства для нескольких потоков

Дата: 2026-11-17. Чекбокс S6-33 в `STAGE_6.md` не отмечен. База — `98ac1e1`. Реализация: `crates/od-machine/src/bus.rs`, `irq_source.rs`, `pci_ecam.rs`, `uart_pic_irq_source.rs`, `uart_ioapic_irq_source.rs`, `pit_pic_irq_source.rs`, `pit_ioapic_irq_source.rs`, `apic_pair.rs` (комментарий), тесты `bus/shared_tests.rs`. `od-isa` не менялся. Потоков vCPU нет и этим срезом они не запускаются (S6-34): готовится то, чем они будут пользоваться.

**Что было.** Устройства и хэндлы (APIC/IOAPIC, PIT, PIC, UART, ACPI PM, дисплей, NVMe, часы) уже были `Arc<Mutex<…>>` с одной блокировкой состояния на устройство. Не были потокобезопасны: четыре источника IRQ (`Rc<Cell/RefCell>`), таблица устройств шины (`Vec<Box<dyn BusDevice>>` без блокировки, доступ только через `&mut MachineBus`), трейты `BusDevice`, `BoardIrqSource`, `PciFunction` без `Send`.

**Что сделано.**
1. `BusDevice: Send`, `BoardIrqSource: Send`, `PciFunction: Send`: все типы устройств платы удовлетворяют требованию без изменений, кроме источников IRQ и тестовых заглушек.
2. Источники IRQ: `Rc<RefCell<Inner>>` → `Arc<Mutex<Inner>>` (`PitPicIrqSource`, `PitIoapicIrqSource`); `Rc<Inner>` с `Cell` → `Arc<Inner>` с `Mutex<Line>` (`UartPicIrqSource`, `UartIoapicIrqSource`; последовательность «прочитать линию, сравнить, подать, записать» целиком под одной блокировкой, поэтому один подъём линии не считается дважды). Тестовые `Rc<Cell>`/`Rc<RefCell>` в `vcpu_quantum.rs`, `linux_direct.rs`, `pci_ecam.rs` переведены на `Arc` с атомиками/`Mutex`.
3. `MachineBus::devices` — `Mutex<Vec<Box<dyn BusDevice>>>` (одна блокировка на доступ к устройствам шины). Исключительные пути берут таблицу через `get_mut` без захвата (правило 6): режим чередования не платит за блокировку. Новые методы на `&self` для потока, у которого только `&MachineBus`: `lock_devices`, `read_port_shared`, `write_port_shared`, `read_mmio_shared(vcpu, …)`, `write_mmio_shared(vcpu, …)` (доступ к LAPIC — как доступ указанного vCPU). Поиск устройства MMIO вынесен в `mmio_target`, им пользуются исключительные и общие методы.
4. RAM вне блокировки: `PhysicalMemory` остаётся полем шины, `physical_memory(&self)` блокировки не берёт (условие «Учесть (S6-30c)»). `MachineBus`, `PhysicalMemory`, `LocalApic`, все хэндлы устройств, часы и источники IRQ — `Send + Sync`, `LoadedFirmwareBoard` — `Send` (проверено компиляцией в тесте).
5. Блокировка APIC остаётся одной на все LAPIC и маршруты IOAPIC.

**Отступление от плана (принято ревью Opus high с условием пересмотра S6-38).** План (шаг 2) называет «отдельную блокировку на каждый LAPIC». Модель сегодня держит все LAPIC и маршрут IOAPIC под одним `Mutex<PairState>` (S6-11, `apic_pair.rs`): уровневый запрос меняет IRR/TMR адресата и Remote IRR маршрута, EOI адресата снимает Remote IRR, снимок и откат (S6-24) берут состояние целиком. Дробление по LAPIC — переработка `PairState` и всех операций над ним; план требует «сначала самый простой вариант» и дробить по замеру (S6-38). Одна блокировка проще плановой, взаимная блокировка двух LAPIC в ней невозможна по построению, ICR доставляется атомарно. Цена: IPI и доступы к LAPIC разных vCPU сериализуются; влияние на масштаб — предмет замера S6-38.

> Решение ревью S6-33 (Opus high, 2026-11-17): одна блокировка APIC на все LAPIC и маршруты IOAPIC принята как отступление от шага 2. Основания: встречная связь «маршрут → LAPIC» (доставка) и «LAPIC → маршрут» (EOI снимает Remote IRR); отказ Fixed IPI целиком по SMP_CONTRACT §5 (б); согласованный снимок и сброс §9/S6-24. Условия: (1) код S6-34 не опирается на атомарность доставки ICR относительно прочих операций APIC сверх §5 (б) и §9; (2) S6-34 держит блокировку шины и блокировку APIC не дольше одного обращения, в том числе в опросе границы (`eligible`, `pending_vector`, `wake_ns`), и никогда на весь шаг или квант; (3) S6-38 обязательно замеряет ожидание блокировки APIC и блокировки шины при 2 и 4 потоках vCPU (доля ожидания, масштаб относительно 1 потока) на Linux и на синтетике IPI/EOI/TPR. Если блокировка APIC или путь «шина → LAPIC» — главное узкое место масштаба, дробление обязательно. Варианты: окно LAPIC вне блокировки шины плюс замок на каждый LAPIC, либо безблокировочный признак ожидания для опроса границы. (4) При дроблении правило 4 уточняется. Для одного адресата: «прочитать и проверить под своей блокировкой, отпустить, доставить под блокировкой адресата». Для shorthand и для §5 (б): захват блокировок адресатов по возрастанию APIC ID либо пересмотр §5 (б) решением владельца. EOI → Remote IRR: отпустить замок LAPIC до захвата маршрута.

**Не входит.** Потоки vCPU, kick, остановка (S6-34); дробление блокировок (S6-38); поток, ждущий в `HLT`, и его пробуждение (S6-34); перенос `GuestMemory` на `&self` (условие пересмотра S6-30c не наступило: адаптер по-прежнему принадлежит одному потоку vCPU, шина доступна ему по `&MachineBus` через методы `*_shared`; перевод путей набора CPL0 с `split_for_*(&mut self)` на `*_shared` — работа S6-34).

**Тесты** (`bus::shared_tests`, +11): `Send`/`Sync` всего, до чего дотягивается поток vCPU; взаимные записи ICR двух потоков через шину (3000 синхронных раундов и 3000 свободных, оба счётчика равны числу записей, каждый из 208 различных векторов дошёл до адресата) и через хэндлы без шины, с тайм-аутом 30 с; смешанное использование шины, хэндла и RAM тремя потоками; (а) операции APIC, IOAPIC, PIT, PIC, UART и всех четырёх источников завершаются, пока другой поток держит блокировку шины; порядок «шина → APIC» работает, а обращение к LAPIC через шину ждёт занятую блокировку APIC, держа блокировку шины; (в) RAM читается и пишется (20 000 обращений) под удерживаемой блокировкой шины и при ожидающем её втором потоке; совпадение общих и исключительных обращений (результаты и ошибки); паника под блокировкой не блокирует шину; источники и записи устройств из пяти потоков завершаются. Существующие тесты (`ap_legacy`, `firmware_board`, `vcpu_quantum`, `stage6_smp_probe`, источники IRQ) проходят без изменения ожиданий. Контрольный Linux режима чередования побайтно прежний (отчёт S6-33).

## S6-34 — потоки vCPU, kick и остановка

Дата: 2026-11-17. Чекбокс S6-34 в `STAGE_6.md` не отмечен. База — `049005a`. Реализация: `crates/od-machine/src/parallel_run.rs` (новый модуль: `run_threads`, `ParallelControl`, потоки, kick, остановка, шлюз split `LOCK`, арбитр простоя), `bus.rs` (`split_for_set_step_shared`, `DeviceTable`, `SplitLockGate`), `paged_memory_adapter.rs` (`LoopBus`, `run_cpl0_set_loop`, шлюз в `locked_target`, `remember_fetch_translation`), `firmware_board.rs` (`ThreadParts`, `deliver_pending_nmi_of`), `firmware_board/vcpu_quantum.rs` (`run_vcpu_set_quantum_shared`), `ap_legacy_exec.rs` (`run_legacy_on`), `apic_pair.rs`/`local_apic.rs` (приёмник пробуждения), `fetch_translation_cache.rs`, `physical_memory.rs`, `stage6_smp_probe.rs` (`run_parallel`), `tools/runner/src/stage6_smp_probe.rs` (`--parallel`). `od-isa` не менялся. Режим чередования остаётся режимом по умолчанию и не менялся: его пути идут через `LoopBus::Exclusive` и `DeviceTable::Exclusive` (таблица устройств без захвата, как в S6-33), результаты контрольного Linux прежние (отчёт).

**Модель потоков.** `run_threads(board, config, quantum, controller)` переводит плату в `ExecutionMode::Parallel` на время прогона (и возвращает прежний режим), делит её на общее (`&MachineBus`: RAM без блокировки и таблица устройств под одной блокировкой шины; Local APIC; владелец часов под замком; журнал COM1 под замком) и vCPU (каждый поток владеет ровно одним, vCPU лежит под мьютексом, который поток держит, только пока vCPU исполняется). Один поток стандартной библиотеки на vCPU (`std::thread::scope`, асинхронного runtime нет); поток вызывающего — контроллер (`ParallelControl`): остановить всех, посмотреть остановленную машину, возобновить, завершить. Когда контроллер вернулся, прогон завершается, и функция возвращается только после `join` каждого потока (`threads_started == threads_exited == N` в отчёте). Выбор режима — параметр запуска: `Stage6SmpProbe::run_parallel` и `od-runner --stage6-smp-probe ... --parallel [--quantum Q] [--timeout-s N]` (без параметра работает планировщик S6-15; `--seed`, `--schedule`, `--record-schedule` с `--parallel` отказ: расписания нет). Квант — шагов за один вызов цикла интерпретатора потока, до среза по ближайшему сроку таймера LAPIC и по общему бюджету (в retired всех vCPU вместе).

**Исходное состояние потоков.** До создания потоков планировщик заполняет публикуемые активность, IF и признак перезапуска REP из каждого vCPU. Поэтому уже первый запрос STOP получает подтверждение, согласованное с состоянием vCPU. Правки контроллера на остановке публикуются по завершении callback под блокировкой слота vCPU. Повторный STOP собирает подтверждения из обновлённых данных.

**Восстановленные пробуждения.** `idle_heals` считает решения `IdleOutcome::Woken`, при которых повторная проверка перед kick видит все vCPU спящими без ожидающего kick. Флаг kick читается до признака занятости, который поток поднимает перед очисткой флага. Прямые пробуждения от источника платы проходят по своим веткам. [Регрессии S6-37](../reports/STAGE_6_REPORT.md#s6-37--наблюдение-восстановленных-пробуждений).

**Общий бюджет.** Перед гостевым квантом рабочий поток атомарно резервирует инструкции из доступного остатка. После начисления выполненной работы он возвращает неиспользованный резерв. Нулевой доступный остаток при незавершённых квантах оставляет другие потоки ждать возврата без удержания слота vCPU. `BudgetExhausted` публикуется, когда сумма retired достигла бюджета и отложенная работа NVMe обслужена.

**Проход потока.** (1) барьер (конец прогона, запрос остановки всех, чужой split `LOCK`: поток стоит и сообщает об этом); (2) граница: INIT и SIPI своего LAPIC, затем NMI (порядок `BoardSmpHost::boundary`, S6-15); (3) `Running`: один вызов цикла (`run_cpl0_set_loop` над `LoopBus::Shared`, для AP в legacy-режиме — `run_legacy_on` на общей RAM, а после 64-битного прыжка трамплина остаток бюджета — длинный квант в том же вызове), затем начисление retired и тиков устройств владельцу часов; `Halted`: проснуться, если IF = 1 и в LAPIC допустимый вектор (для BSP — и если источник платы может его разбудить), иначе спать; `WaitForSipi`/`Shutdown`: спать. Для vCPU в legacy-режиме NMI — ошибка прогона, как в S6-15.

**Порядок блокировок для потоков (дополнение к порядку S6-33; нормативно).** Над уровнями 1–3 появились блокировки прогона: слот vCPU (мьютекс, один на vCPU; поток держит его, пока vCPU исполняется, и отпускает до сна, барьера и подтверждения остановки; контроллер берёт его только у подтвердившего остановку) -> замок источника платы (`irq`: источник IRQ платы, его держит только поток BSP на время вызова и арбитр простоя без ожидания) -> замок владельца часов (`owner`: берётся на начисление retired/тиков, на расчёт бюджета шага и на решение арбитра простоя; внутри него `dispatch_timer` берёт замок APIC, а он — замок часов: вложение уровня 2 -> 3 из S6-33, не новое) и замок журнала COM1 (берётся на весь «насос» передачи UART: под ним берутся замок источника UART и замок UART; оба не вложены друг в друга) -> уровни 1–3 S6-33. Единственное обращение вверх — `try_lock` источника платы под замком владельца часов в арбитре простоя: оно не ждёт, занятый источник означает «поток не простаивает». Блокировки таблицы устройств, APIC и источников IRQ берутся на одно обращение (правило 3 S6-33): оболочка набора CPL0 для потока (`DeviceTable::Shared`) берёт замок таблицы на одно обращение к порту или MMIO и отпускает его до возврата из обращения; опрос границы (`LapicIrqSource::pending_vector`, `eligible`) — одно обращение к APIC. Условия решения ревью S6-33: (1) код S6-34 не опирается на атомарность доставки ICR сверх §5 (б) и §9 SMP_CONTRACT: он видит ожидающее событие в своём LAPIC, а не момент доставки; (2) ни замок шины, ни замок APIC не держится дольше одного обращения, ни на шаг, ни на квант (проверено тестом `the_shared_set_step_view_holds_the_device_lock_for_one_access_only` и тем, что все тесты ниже идут без взаимной блокировки); (3) замер ожидания замка APIC/шины при 2 и 4 потоках — обязанность S6-38, этим срезом не снимался; (4) дробление не делалось, правило 4 порядка блокировок остаётся «выполнено по построению». Правило отравления (правило 7 S6-33): паника в потоке vCPU пойманa (`catch_unwind`), прогон заканчивается для всех как `ParallelFinish::Panicked { cpu }`, остальные потоки доходят до границы и выходят; машина на частично обновлённом состоянии не продолжается; все блокировки берутся с `into_inner`, чтобы не зависли остальные потоки (тест `the_error_or_the_panic_of_one_vcpu_ends_the_run_for_all`).

**Kick и сон (шаг 2).** Поток без работы спит в `thread::park` и ничего не крутит. Приёмник пробуждения (`WakeSink` в `PairState`) вызывается из состояния APIC под замком APIC при каждой постановке запроса в LAPIC адресата (Fixed-вектор от устройства, таймера или IPI в `raise_edge`/`level_pin`, INIT, SIPI, NMI) и делает ровно два действия без замков: ставит флаг kick адресата и `unpark`ит его поток (вызов без ожидания; замок выше APIC в нём брать нельзя). Протокол без потерянного пробуждения: поток сбрасывает свой флаг ДО того, как смотрит в LAPIC (`INIT/SIPI/NMI`, `eligible`), и засыпает, только если флаг всё ещё сброшен перед `park`; событие, поставленное после просмотра, ставит флаг после сброса (сон кончается сразу); поставленное до просмотра видит сам просмотр; пробуждение между проверкой и `park` — токен `unpark`, он не теряется; `park` может вернуться без kick, цикл проверяет флаг снова. Источник платы (PIC/PIT, не LAPIC) никого не будит: поток, сдвинувший часы, смотрит, спит ли BSP в `HLT` с IF = 1, и kick-ает его, если `wake_ns` источника наступил, а при занятом замке источника (`try_lock` вернул `WouldBlock`) kick-ает без проверки (лишнее пробуждение безвредно, потерянное оставило бы линию без обслуживания); то же делает арбитр простоя после скачка и, не принимая решения, когда BSP спит в `HLT` с IF = 1, а срок источника уже наступил. Проверено от противного (по два теста: `a_wake_up_posted_around_the_sleep_is_never_lost` — 3000 IPI с разной задержкой, и `a_wake_up_posted_while_the_vcpu_is_between_its_look_and_its_park_is_never_lost` — окно между просмотром и `park` удерживается намеренно через тестовую точку `pre_sleep`, 500 раундов): (А) kick без флага, только `unpark`, ронял тест на первом раунде (IPI потерян, ожидание 20 с); (Б) поток, сбрасывающий флаг не до просмотра, а после него (в начале сна), роняет оба теста (окно: «IPI, поставленный между просмотром и `park`, потерян», раунд 1 и раунд 180 из 3000); оба возвращены. Оговорка: пока ВСЕ потоки спят, арбитр простоя при каждом уходе в сон видит ожидающий допустимый вектор спящего vCPU (`Woken`) и будит его сам, то есть потерянное пробуждение «лечится»; поэтому оба теста держат BSP занятым (`jmp .`), иначе мутация (Б) не ловилась бы.

**Остановка всех vCPU (шаги 3–5).** `ParallelControl::stop_all(timeout)`: ставит запрос (`stop_all`, `SeqCst`), kick-ает те vCPU, которые ещё не подтвердили, и ждёт подтверждение каждого до срока; на тайм-ауте запрос снимается, ошибка называет не пришедших (`ParallelStopError::Timeout { unacked }`), и ни состояние, ни ожидающее событие, ни срок не менялись. Подтверждение — флаг `safe` потока: поток ставит его (а) на барьере (между инструкциями: интерпретатор опрашивает `stop_all`, `finish` и чужой шлюз split `LOCK` между инструкциями и в `REP` после любой итерации и выходит из вызова; vCPU на точке рестарта `REP` подтверждает `RepRestart`), (б) уходя в сон: vCPU в `HLT`, в ожидании SIPI или в `Shutdown` подтверждает сразу и остаётся спать, без гостевой инструкции (`Asleep`). Подтверждение не ждёт замка, который держит остановленный или спящий vCPU: никакой поток не держит замок устройства, APIC, часов или слот vCPU через `HLT`, ожидание SIPI, сон и барьер (правило 3). Выход из `HLT` и из ожидания SIPI всегда сначала проходит барьер (шаг 4): разбуженный kick-ом поток видит запрос остановки, ничего не начинает, а IPI/прерывание остаётся ожидающим в LAPIC (тест `an_ipi_to_a_sleeping_stopped_vcpu_waits_for_the_resume_and_is_not_lost`: остановка, два IPI спящему, 30 мс — ни вызова, ни шага, ни часов, вектор в IRR; после `resume` счётчик обработчика растёт, вектор снят; 50 повторов). Порядок (шаг 5): всё, что поток записал в свой vCPU (слот отпущен до подтверждения), в RAM и в устройства, предшествует подтверждению (`SeqCst`, то есть не слабее `Release`); контроллер читает его (`SeqCst`, не слабее `Acquire`) и берёт слот vCPU, поэтому видит всё. Возобновление — то же в обратную сторону: записи контроллера, затем запрос снимается (`Release`), поток читает его (`Acquire`). Рукопожатие «кто стоит» — шаблон Деккера: поток сначала сбрасывает свои `safe`/`quiet`, затем читает запросы; запрашивающий сначала ставит запрос, затем читает флаги; оба `SeqCst` (нужен единый общий порядок), иначе устаревшее `safe` пропустило бы запускающийся поток. По MEMORY_MODEL.md: порядок записей одного потока и другого через флаг подтверждения — `Release`/`Acquire` на одной ячейке; RAM остановленных читается обычными `SeqCst`-обращениями, так что доказательства S6-06 не меняются. Проверено: 300 остановок на цикле «записать счётчик, затем увеличить регистр» — разность регистра и памяти у остановленного всегда 0 или 1 (`the_stopped_state_and_the_ram_the_controller_reads_agree_with_what_the_vcpu_left`). `resume` после остановки, при которой все спали, запускает решение арбитра простоя сам (иначе никто не двинул бы часы).

**Время (шаг 6).** Виртуальное время на остановке стоит: поток начисляет часы только внутри своего прохода, до подтверждения, скачок простоя берёт замок владельца часов и перечитывает запрос остановки под ним, а контроллер после подтверждений один раз берёт тот же замок. В работе часы двигает только владелец часов (D7, S6-15/S6-21): он один, под замком; поток после вызова вызывает `account_retired`/`account_device_ns`, а скачок простоя (`idle_step`, не больше одного скачка за решение, повтор пока только `Advanced`) делает поток, уходящий в сон последним, когда ни один другой не занят и ни у одного нет ожидающего kick; «ничего не разбудит» завершает прогон как `AllIdle` (параметр `end_when_idle = false` оставляет прогон открытым для событий контроллера и устройств). Состав «не занят» публикуется потоками (`busy`, `activity`, IF, REP-признак): атомики. Порядок начисления между потоками не детерминирован, поэтому момент срабатывания таймера зависит от расписания потоков — детерминизма параллельный режим не обещает.

Запросы времени от ACPI PM и PIT принадлежат плате. При запуске интерпретатора очередь от внешних обращений очищается до запуска потоков. Host-прогон сохраняет ожидающие тики и передаёт их владельцу часов через обслуживание устройств. В ходе прогона общий интерпретатор забирает накопленные запросы под мьютексами устройств в конце кванта и передаёт их владельцу часов. Это позволяет начислить тик, поставленный в очередь другим vCPU, ровно один раз. [ISSUE-525](../KNOWN_ISSUES.md#issue-525--s6-34-потеря-тиков-acpi-pm-и-pit-в-параллельном-режиме).

**Завершение (шаг 7).** Конец прогона, `STOP` контроллера (возврат из его замыкания), ошибка одного vCPU (ошибка вызова, границы, начисления часов или шлюза), паника потока и исчерпанный бюджет ставят `finish` и kick-ают всех; каждый поток доходит до границы и выходит; результат — первая причина. Зависших потоков нет: тесты проверяют `threads_started == threads_exited`, возврат функции после `join` и конечное время.

При позднем отказе общий интерпретатор возвращает выполненный префикс вместе с исходной ошибкой. Переход из legacy в long mode в одном вызове учитывает обе части. Поток начисляет retired и время устройств владельцу часов и обновляет счётчики перед публикацией ошибки этого вызова. [ISSUE-526](../KNOWN_ISSUES.md#issue-526--s6-37-префикс-ошибочного-long-mode-кванта).

**Split `LOCK`, не помещающийся в 16 байт (шаг 8; сделан).** Протокол `SharedState::split_acquire` (шлюз `SplitLockGate`, поле набора CPL0 `MachineBusSetStepIo::gate`):
1. Адаптер vCPU вызывает `acquire` в `atomic_*_integer` для невыровненного операнда (в том числе через границу строки или страницы) в параллельном режиме, ДО любого видимого эффекта инструкции: до первого обхода операнда, то есть до A/D-битов операнда. (Выборка инструкции и вычисление адреса уже были; их эффект — A-биты кода, он идемпотентен и повторяется без вреда.)
2. Запрос — `compare_exchange(NO_OWNER, cpu)` на `split_owner`. Победитель — владелец: kick-ает всех и ждёт, пока каждый другой vCPU станет тихим (`quiet`): поток, исполняющий гостя, читает `split_owner` в опросе остановки между инструкциями, кончает вызов на ближайшей границе, идёт на барьер и сообщает `quiet`; спящий тих всегда; поток, ждущий в собственном `acquire`, тих.
3. Владелец выполняет последовательную пару S6-32 (`locked_serial`: обычное чтение, обновление, обычная запись через собственные `read`/`write` адаптера: те же обходы, A/D и уведомления, что у режима чередования) — никто больше не обращается к RAM — и отпускает шлюз (`Drop`, при любом результате, в том числе `#PF`), kick-ая всех.
4. Проигравший (обмен проигран или шлюз уже чей-то) — ещё до первого эффекта своей инструкции: сообщает `quiet` (это «подтверждение остановки» победителю), ждёт отпускания и просит заново. Расхождение с формулировкой плана: проигравший не «откатывается к началу инструкции» (откатывать нечего: до `acquire` инструкция не произвела видимого эффекта, а чтение регистров и вычисление адреса откатом не восстанавливаются: они не менялись), а ждёт внутри адаптера и повторяет запрос; наблюдаемо это тот же результат: ни одного эффекта до повторного запроса, остановка победителя подтверждена, повтор после возобновления. **Отступление шага 8 принято Opus high (ревью S6-34).** Проигравший не откатывается к началу инструкции, а ждёт внутри адаптера и повторяет запрос; до `acquire` эффектов нет, кроме идемпотентных A-битов выборки; декодированные байты инструкции исполняются как есть: межпроцессорная модификация кода без сериализации не определена по SDM 8.1.3, откат её не лечил бы. Условие: проигравший ждёт, держа свой слот vCPU, а BSP ещё и замок источника платы `irq` (`call()` держит его весь вызов); поэтому ни один путь к `quiet`/`safe` не должен ждать этих замков (условие для S6-37: Linux, источник платы, IOAPIC -> AP). Отдельно: шлюз не покрывает записи контроллера в RAM через `ParallelControl::bus()` во время работы (контроллер, пишущий в RAM работающей машины, не проходит шлюз). Остановка контроллера (`stop_all`) на проигравшего не действует через `quiet`: его `safe` не ставится, пока инструкция не кончилась (он посреди инструкции), контроллер ждёт его до срока; проигравший кончает инструкцию после отпускания шлюза и подтверждает на следующем барьере.
5. Тайм-аут (`split_timeout`, по умолчанию 30 с): владелец, не дождавшийся тишины, отдаёт шлюз и инструкция заканчивается типизированной ошибкой до эффекта (`VcpuQuantum`, прогон заканчивается ошибкой); проигравший, не дождавшийся отпускания, так же. Исполнение двумя host-доступами никогда.
6. Без шлюза (адаптер вне потоков прогона, например тест адаптера) в параллельном режиме остаётся явный отказ со счётчиком `split_lock_refusals` (шаг 11): он убран только там, где есть шлюз. Режим чередования не менялся: пара S6-32, счётчик не растёт.
Учёт: `ParallelCpuReport::split_locks` (операций, выполненных под шлюзом) и `split_losses` (раз, когда запрос проиграл). Проверки — ниже.

**Кеш перевода (шаг 9).** Кеш потока параллельного режима — «строгий» (`FetchTranslationCache::with_insert_generations`, `is_strict`): поколения кадров таблиц берутся при `insert` ДО чтения записей обхода, из которого сделана запись (правило читателя S6-30b). `remember_fetch_translation` в строгом кеше (1) читает без записи обход, называющий кадры таблиц, (2) читает поколение каждого, (3) делает второй такой же обход и сохраняет запись только если кадры те же; `insert_with_generations` хранит поколения в записи, `seal` и `revalidate` сверяют с ними (самое старое поколение кадра среди записей), а не с поколением, прочитанным в конце вызова. Запись в кадр таблицы другим vCPU после (2) поэтому всегда новее записи кеша и сбрасывает её на следующем `revalidate`/`seal`: лишнее обновление, не устаревший ответ. Собственные зафиксированные записи потока (они повышают поколение страницы на 1 до колбэка `note_write`) строгий кеш «впитывает»: `note_write` увеличивает ожидаемое поколение записи на 1 для каждой затронутой страницы-таблицы, поэтому политика `GuestOnly` (TLB: собственные записи гостя в таблицы ничего не сбрасывают) не получает ложного сброса, а чужая запись, не прошедшая через его `note_write`, видна как расхождение. Кеш без строгости (все существующие: Linux-цикл, `linux_direct`, диагностика) ведёт себя как раньше: поколения читаются на `seal` (порядок «байты, затем поколение», допустим только в режиме чередования, где `seal` идёт до хода другого vCPU), обход один, результаты и счётчики те же (`persistent_cache_*` тесты `linux_direct` без изменения ожиданий). Кеш — свойство вызова одного vCPU (`RefCell`, не общий): правило нужно, потому что запись другого vCPU не приходит в `note_write` (колбэк потока писателя). Проверки на границе вызова, не на каждое попадание: внутри вызова чужая запись в таблицу может не быть замечена до `INVLPG`/`MOV CR3` гостя или конца вызова (устаревание допустимо до собственной инвалидации процессора, как у TLB); политика `WriteWatch` в параллельном режиме поэтому не обещает «точность против машины без TLB». Кеш в кванте платы (`run_vcpu_set_quantum_shared`) не используется (`None`), как и в режиме чередования; условие закрыто для кеша, когда его подключат (S6-37). Тесты: `a_pte_rewritten_by_another_thread_is_never_served_stale_from_the_fetch_cache` (поток переписывает PTE кода 20 000 раз, другой поток исполняет последовательность цикла `revalidate` -> выборка через кеш -> `seal`; ответ не старее последней завершённой записи; от противного: замена поколений на `[None; 4]` даёт 8 476 устаревших ответов), `the_generation_taken_at_insert_makes_a_write_before_the_seal_drop_the_entry` (детерминированное чередование, строгий и нестрогий кеш), `a_strict_cache_absorbs_its_own_table_writes_and_drops_the_entry_for_another_vcpus`.

#### Порядок D на пути AP (шаг 10)

При старте AP на штатной параллельной плате `run_legacy_on` получает `DescriptorGate` до первой выборки и освобождает его после `ProtectedCpu::step` с отложенной фиксацией A/D. Общий шлюз ждёт, пока остальные vCPU закончат текущую инструкцию или синхронное обслуживание DMA, и удерживает их на границе. Они могут наблюдать результат записи и обновлённый PTE после освобождения шлюза. `BoardRam` отклоняет запись дескриптора, если его образ пересекает запись таблицы, прочитанную при обходе этого шага. Гарантия действует на штатной карте RAM с границами по страницам.

Порядок записи внутри `ProtectedCpu::step` описан в S6-31b (m3). Сохранение исключительной секции входит в контракт параллельного старта AP. Для пути, допускающего конкурентный доступ к RAM в обход шлюза, потребуется собственный порядок записи операнда и A/D. [Проверка S6-34](../reports/STAGE_6_REPORT.md#s6-34-шаг-10--публикация-dirty-при-старте-ap).

**`PhysicalMemory`: счётчики вне горячего пути (шаг 12).** Счётчики, которые пишутся при работе потоков, вынесены в `ColdCounters` (`repr(align(128))`, свои кеш-линии: запись в них не инвалидирует линию `regions`/`mode`, которую читает каждое обращение). `host_fences` существует только в тестовой сборке (`#[cfg(test)]`, как и `host_fences_issued`): гостевой `MFENCE` параллельного прогона не увеличивает общий счётчик; `split_lock_refusals` остаётся (редкий путь). Тест `the_write_counters_do_not_share_a_cache_line_with_the_fields_every_access_reads`.

**Тесты.** `od-machine` +26 passed (полный прогон: 33 набора, 1937 passed, 0 failed, 9 ignored), `od-runner` +1. Потоки (`parallel_run::tests`, 18): обмен IPI двух потоков, потоки соединены; обмен верен каждый раз при квантах 1, 2, 3, 7, 100, 1024 (6 x 25 прогонов); потерянное пробуждение — 3000 IPI с задержкой, сканирующей путь `IRETQ` -> `park`, и 500 раундов с намеренно удерживаемым окном «просмотр LAPIC — `park`»; спящий vCPU не делает ни вызова, ни шага и подтверждает остановку за < 1 с; IPI спящему остановленному vCPU не запускает его до `resume` и не теряется (50 повторов, часы и счётчики стоят); тайм-аут остановки называет зависший vCPU, запрос снят, повторная остановка проходит; остановка посреди обмена IPI (100 попыток, квант 1, порог 40…4000 retired; не меньше половины остановок внутри обмена): ничто не растёт 5 мс, после `resume` обмен кончается верно, LAPIC пусты; состояние и RAM остановленного согласованы (300 остановок); конец прогона контроллером и бюджетом, ошибка и паника одного vCPU, потоки соединены; litmus-счётчик с `LOCK INC` — выровненный (2 и 4 vCPU x 5 x 1500), split через границу строки и страницы в листе 2 МиБ и в двух кадрах (2 и 4 x 5 x 400, `split_locks` каждого vCPU равен числу операций), смешанный (split `LOCK INC` на одних vCPU и выровненный `LOCK INC` на общих байтах у других, 2 и 4 x 5 x 300, оба режима таблиц), гонка за шлюз реально была (`split_losses > 0`); split `LOCK`, не получивший тишину остальных, падает до эффекта (операнд 0); без шлюза в параллельном режиме отказ со счётчиком остался; приёмник пробуждения слышит каждую постановку (вектор, IPI, INIT, SIPI, NMI) и ничего при чтении, переживает восстановление снимка. Прочие: `bus::shared_tests` (+2): вид набора CPL0 на `&MachineBus` равен исключительному, замок устройств на одно обращение; `fetch_translation_cache` (+2): поколение при вставке (строгий и нестрогий кеш), впитывание собственных записей; `paged_memory_adapter` (+1): межпоточный обход, PTE переписывается 20 000 раз; `physical_memory` (+1): счётчики на своих линиях; `tests/stage6_smp_probe.rs` (+2, +1 `#[ignore]`): probe на потоках во всех сценариях (normal, exchange, resipi, nmi, shootdown) при квантах 1, 7, 100, 1024 (по 6 раундов), AP отсутствует при одном vCPU; `tools/runner/tests/stage6_smp_probe_cli.rs` (+1): `--parallel`. Длинные формы (`#[ignore]`, release, команды в [SMP.md](../testing/SMP.md)): probe 2000 пар `exchange`+`shootdown` — 12,5 с, все верно; остановка посреди обмена 1000 попыток — 749 остановок внутри обмена, все обмены кончились верно (12,5 с); пробуждение 50 000 + 5 000 раундов — 3,4 с, ни одного потерянного; litmus split `LOCK` 200 раундов x 2 и 4 vCPU x 2 раскладки x 2 счётчика по 1000 на vCPU — счётчики точны, 513 099 проигранных запросов за шлюз, 28,8 с. От противного (мутации, возвращены): шлюз не ждёт `quiet` — смешанный litmus падает сразу («счётчик выровненного потерял обновление»), простой litmus split этого не ловит (остальные vCPU трогают операнд только через шлюз), поэтому смешанный добавлен; kick без флага и сброс флага после просмотра — тесты пробуждения; замена поколений при вставке на `[None; 4]` — 8 476 устаревших ответов кеша.

**Не входит / открыто.** Замер масштаба и ожидания замков APIC/шины (S6-38); Linux на нескольких потоках vCPU и подключение квантов Linux-цикла к потокам (S6-37); состояние остановленной параллельной машины для снимка и сброс (S6-35: остановка готова, снятие состояния S6-24 на параллельной остановке — там); litmus параллельного режима и чувствительность (S6-36); 16-байтная пара; публикация Dirty на будущих legacy-путях с доступом к RAM в обход шлюза (шаг 10, см. выше); потоки в режиме `Interleaved` не запускаются. Записано по ревью S6-34: (m2) барьер сбрасывает `safe` до проверки остановки: при входе в барьер с поднятой остановкой и спящим, которому пришёл kick, флаг `safe` на мгновение false (условие для S6-35: не сбрасывать, если `stop_all` уже стоит и `safe` поднят); срок `split_timeout` отсчитывается один раз на инструкцию, при длительной конкуренции за шлюз возможно голодание проигравшего, ошибка через 30 с; `resume()` делает `kick_all`, все спящие просыпаются впустую (цена для S6-38); счётчик `idle_heals` предложен для наблюдения за восстановлением пропущенных kick и добавлен в S6-37. Штатные обмены со всеобщим сном проверяют нулевое значение.

## S6-35 — состояние на остановленной параллельной машине

`StopProof` привязан к подтверждённой паузе конкретного параллельного прогона. Повторный `stop_all` без `resume` выдаёт подтверждение для той же паузы. После `resume` подтверждение недействительно и при следующей остановке даёт `StaleSafePoint`, даже если совпали время и состояния vCPU. Снимок проверяет принадлежность подтверждения текущей паузе до чтения vCPU и перед возвратом.

`ParallelControl::stop_all` проверяет, что прогон продолжается, после сбора флагов `safe` и построения подтверждения. Выходящий поток также выставляет `safe`, поэтому завершившийся прогон возвращает `ParallelStopError::Finished`. Доступ к vCPU и `VmSnapshot::take_parallel` требуют действующей остановки живого прогона. Снимок повторяет эту проверку после чтения CPU, LAPIC и состояния планировщика. [Проверки](../reports/STAGE_6_REPORT.md#s6-35--завершение-прогона-во-время-stop).

Дата: 2026-11-18. Чекбокс S6-35 в `STAGE_6.md` не отмечен: шаги 1–3 плана выполнены без формата файла, сквозная проверка с форматом снимка blocked до `S9-24`, `S9-25`. База — `86f5e9f`. Реализация: `vm_snapshot.rs` (`VmSnapshot::take_parallel`), `stage6_smp_probe.rs` (`Stage6SmpProbe::run_parallel_to_a_stop`, `Stage6SmpProbeParallelStop`, `Stage6SmpProbeSnapshot::{same_processor_state, pending_vectors}`); тесты — `parallel_snapshot_tests.rs` и `tests/stage6_smp_probe.rs`. Барьер и протокол остановки в `parallel_run.rs`, шина и RAM не менялись; добавлен только читающий метод `ParallelControl::vcpu_count` (число потоков прогона, для сверки с `proof`).

**Что есть (шаг 1).** Значение `VmSnapshot` из S6-24 снимается и на машине, остановленной параллельным режимом. Правило одно: **снимок в параллельном режиме берётся только при `StopProof` из `ParallelControl::stop_all`, пока остановка действует** (между `stop_all` и `resume`). `VmSnapshot::take_parallel(control, proof, scheduler)` вызывает контроллер прогона (замыкание `run_threads`); плата в это время отдана потокам, поэтому vCPU читаются через `ParallelControl::with_vcpu` (клон под мьютексом слота, доступен только подтвердившему остановку), а набор Local APIC и маршрут IOAPIC — через `ParallelControl::lapic` того же прогона. Отказы: остановки нет (не было, либо `resume` уже снял её) — `NotStopped`; часы сдвинулись после `proof`, или vCPU не такой, каким подтвердил (`holds`: те же три вида подтверждения, что в `take` — граница, точка рестарта `REP`, сон) — `StaleSafePoint`. Если флаг безопасности остаётся опущен дольше двух секунд — `SafeFlagTimeout`. Частичного снимка (часть vCPU на safe point, часть нет) не бывает: без `proof` и без остановки значения нет. Порядок чтения — шаг 5 S6-34: подтверждение `SeqCst` после всех записей потока, чтение контроллера после подтверждения каждого; ни один поток не пишет ни в vCPU, ни в RAM, ни в часы; единственный, кто пишет в Local APIC, — сам контроллер (устройства со своими потоками, которые тоже могли бы писать в LAPIC, появятся в `S9-28`, не раньше). **Допущение:** `take_parallel` вызывается только контроллером прогона между `stop_all` и `resume`; параллельного `resume` нет (контроллер — один поток и делает и то и другое). Число подтверждений в `proof` обязано равняться числу потоков прогона, иначе `StaleSafePoint` до чтения любого vCPU (долг m-1 ревью S6-35, сделан: раньше лишнее подтверждение дало бы панику индексации).

**Что в снимке.** Все vCPU (состояние процессорной модели, активность, MSR-ячейки, CR2, CPUID, флаг рестарта `REP`); все Local APIC с ожидающими INIT/SIPI/NMI, IRR/ISR/TMR, таймерами и их `EventQueue` (абсолютные сроки в виртуальном времени, generation) и маршрутом IOAPIC с Remote IRR (`PairState`) — **ожидающие события LAPIC в снимке**; виртуальное время (не восстанавливается: стоит, служит сторожем); состояние планировщика режима чередования, который пойдёт после восстановления: у параллельного режима планировщика нет, поэтому это состояние планировщика «при включении» с политикой, заданной вызывающим (в пробе — `Seeded(seed)`).

**Чего в снимке нет.** RAM — не в снимке (`S9-24`). Устройства платы (UART, PIT, PIC, ACPI PM, NVMe, дисплей) и источник прерываний платы — не в снимке (`S9-25`). Кеши host (перевода, декодирования) — производные, не сохраняются. Это не снимок машины и не файл: формат, версии, manifest — `S9-24`/`S9-25`; барьер с вводом-выводом — `S9-28`. Детерминизм параллельного режима не заявляется: остановка в параллельном режиме приходится на непредсказуемый момент, и то, что гость делает дальше, определяется потоками, а не снимком.

**Восстановление.** Только в ту же машину и только в режиме чередования: `ParallelControl` не умеет восстанавливать (плата отдана потокам, сброс комплекса и `restore` требуют `&mut` платы). Порядок теста: остановить параллельный прогон на safe point -> снять состояние в контроллере -> закончить прогон (потоки соединены; машина стоит там, где остановилась: ни один vCPU, LAPIC или часы не сдвинулись — проверено равенством состояния после прогона состоянию из контроллера) -> `reset_cpu_complex` в той же машине (AP в `WaitForSipi`, BSP на состоянии входа, все LAPIC и их очереди очищены; RAM, устройства и часы не тронуты) -> `restore` -> продолжить в режиме чередования с фиксированным seed. Побайтного равенства с непрерывным прогоном не требуется и не заявляется. Протокол пробы не допускает повторного INIT/SIPI поверх выполненного обмена, но тест его и не повторяет: после `restore` и RAM, и состояние vCPU/LAPIC — те, что были на остановке, поэтому гость продолжает с той же точки, а не стартует заново; сброс нужен только как доказательство, что восстановление возвращает состояние из пустого, а не оставляет его как было. Восстановление в новую машину (RAM и устройства взять неоткуда) — `S9-24`/`S9-25`, здесь не делается и не проверяется.

**Что проверяют тесты.** (1) Probe v2 (`exchange`) и v3 (`shootdown`) на потоках, остановка по числу retired инструкций из seed, квант потоков 1/7/100, состояние на остановке == состояние машины после прогона (vCPU, LAPIC, часы), IRR/ISR каждого LAPIC до сброса == в снимке, после сброса пусто, после `restore` равно снятому; продолжение в режиме чередования завершается `AllIdle` с транскриптом из манифеста сборщика, ячейками протокола, пустыми LAPIC и обоими vCPU в `Halted`. (2) Фикстура платы (BSP крутится с IF = 1 и принимает периодический таймер LAPIC, AP спит в `HLT` с IF = 1 и принимает одноразовый таймер; на остановке контроллер ставит IPI остановленному спящему AP): сроки таймеров до сброса, после сброса (очереди пусты) и после `restore` одинаковы; IPI ждёт в IRR AP до `resume`; после продолжения обработчик AP вызван ровно дважды (IPI и одноразовый таймер, не потерян и не удвоен), одноразовый таймер израсходован, число вызовов обработчика периодического таймера плюс запрос в полёте равно числу прошедших периодов `now / период` (не потерян и не удвоен). (3) Отказы: снимок без остановки, по `proof` уже снятой остановки и по `proof` более ранней остановки (часы сдвинулись) отказывает. (4) vCPU, спящий на остановке и разбуженный kick-ом от IPI, проходит барьер потока и на миг опускает `safe` (запись S6-34, m2): барьер S6-34 не менялся [изменён в S6-36: условия 1–3; M-1 — ожидание флага вернулось]. Тест на 300 остановок окно m2 не воспроизводит (проходит и при нулевом ожидании); это проверка отсутствия поломки, не проверка m2. (5) Доказательство с числом подтверждений не по числу потоков (лишнее, меньше, пустое) отказывает `StaleSafePoint` без паники и без изменения машины, а верное доказательство той же остановки после этого снимок даёт (`a_proof_with_another_number_of_acknowledgements_is_stale_and_changes_nothing`).

**Ограничения.** (а) Снимок берётся и восстанавливается только в одной и той же машине и процессе. (б) `ParallelControl` не восстанавливает: между остановкой и `restore` прогон потоков заканчивается. (в) Расписание, число `retired` и точка остановки недетерминированы; проверяется итог гостя и сам снимок. (г) Таймер LAPIC в пробе не используется (гость его не программирует), поэтому проверка сроков идёт на фикстуре платы (п. 2), а не на пробе. (д) **m2 ревью S6-34 — открытое ограничение до S6-36.** [Изменено в S6-36: условия 1–3 выполнены; по M-1 ревью S6-36 окно сужено, не устранено, ожидание флага `READ_WAIT` вернулось с отдельной ошибкой `SafeFlagTimeout` (раздел S6-36, «Условия 1–3», п. (3)). Текст ниже — состояние на S6-35.] Барьер `parallel_run.rs` (около строк 463–489) опускает `safe` у спящего vCPU, разбуженного kick: окно — один отрезок от выхода из сна до чтения `stop_all`. Корректность снимка от окна не зависит: поток в нём не держит мьютекс vCPU, LAPIC, RAM или часов; при `stop_all = true` проход через барьер невозможен; успешное чтение — это `with_vcpu` -> `Some` (`stop_all` и `safe`), затем клон и `holds` на клоне. Последствие — только возможный отказ `StaleSafePoint` по истечении 2 с ожидания флага (`READ_WAIT`). Точный тест окна не написан (тест на 300 остановок окно не воспроизводит, см. п. 4). **Закрывается в S6-36 по условиям:** (1) в начале барьера: если `safe` уже поднят и `stop_all` прочитан как true — сразу парковаться, не опуская флаги; (2) `#[cfg(test)]`-зацепка в барьере между сбросом `safe` и повторным подъёмом (по образцу `pre_sleep`) и точный тест «поток удерживается в окне — `take_parallel` — после правки окна нет»; (3) решить судьбу `READ_WAIT`: убрать или оставить как страховку с отдельным видом ошибки. Условия 1–3 перенесены в `STAGE_6.md`, S6-36. (е) Сквозная проверка с форматом снимка (запись, чтение, версия, восстановление в новую машину, RAM, устройства, барьер ввода-вывода) — blocked до `S9-24`, `S9-25`, `S9-28`.

## S6-36 — litmus в параллельном режиме и чувствительность

Дата: 2026-11-18. Чекбокс S6-36 в `STAGE_6.md` не отмечен. База — `1ee677f`. Реализация: `litmus_parallel.rs` (`run_litmus_parallel`, корпус на потоках S6-34), `tools/runner/src/stage6_litmus.rs` (`--parallel --vcpus N --iterations R`), `host_order_litmus.rs` (host-тест MP и 2+2W на функциях доступа к RAM, `#[cfg(test)]`), два feature `od-machine` для тестов и замеров (`weak-host-ordering`, `nonatomic-lock-test`), условия 1–3 переноса из S6-35 в `parallel_run.rs` и `vm_snapshot.rs`. Данные: [`docs/reports/data/stage6/s6-36/`](../reports/data/stage6/s6-36/), методика и команды: [`SMP.md`](../testing/SMP.md), отчёт: [`STAGE_6_REPORT.md`](../reports/STAGE_6_REPORT.md), раздел «S6-36».

**Корпус в параллельном режиме (шаг 1).** Раунд — один вызов кванта каждого потока vCPU, то есть он идёт через тот же цикл потока S6-34 (барьер, граница, начисление часов владельцу), через настоящий интерпретатор и настоящий путь RAM (обычные доступы `SeqCst`, `LOCK` на host-атомиках, host-fence на гостевые fences). vCPU создаются сразу в 64-битном режиме на одном наборе таблиц (тождественное отображение первых 2 МиБ), SMP_CONTRACT 4.4, только для litmus, это не старт AP. Внутри вызова: поток litmus `t` получает начальные регистры и RIP; стартовый барьер (host-барьер на спине по ВСЕМ vCPU машины) проходится внутри первого опроса `stop` интерпретатора, то есть после настройки кванта и до первой гостевой инструкции; затем псевдослучайная задержка 0…256 `spin_loop` на поток и раунд (чтобы потоки не стояли в ногу после барьера); исполняется ровно число инструкций потока (бюджет вызова, без `HLT`); конечный барьер, последний прибывший читает наблюдаемые ячейки из RAM, кладёт исход в гистограмму и возвращает ячейки к начальным значениям. Какой поток litmus на каком vCPU — часть конфигурации: раунд `r` берёт размещение `r mod P`, где размещения — все инъективные отображения `T` потоков litmus на `V` vCPU (2 на 2 vCPU; 12 для двух потоков на 4 vCPU; 24 для четырёх на 4 vCPU), vCPU без потока в раунде только проходит барьеры. Эталон — множество x86-TSO fixture (для счётчика — формула `x86_tso` из `counters-formula.json`, поле `Case::reference_tso`), читается из файла, не из прогона; наблюдаемое множество обязано быть подмножеством. Параллельный режим недетерминирован: вывод несёт время и скорость и не повторяется побайтно.

**Конфигурации.** Тест до двух потоков (14 fixture и три счётчика N = 2, K = 2) — на 2 и на 4 vCPU; IRIW (четыре потока) — только на 4 vCPU; на 2 vCPU `--parallel` печатает для него `not_applicable` и результата не даёт. Дополнительно счётчики N = 4, K = 2 (`LOCK-INC`, `UNLOCKED-INC`) — на 4 vCPU (четыре потока). Число раундов: **1 000 000 на тест на процесс, три свежих процесса на конфигурацию, 3 000 000 на тест на конфигурацию** (замер темпа: 2 vCPU 66–140 тыс. раундов/с, 4 vCPU 46–66 тыс. (хост был занят, см. `host-info.txt`), то есть 7–15 с и 15–22 с на тест на процесс; кандидат `SMP.md` 10 000 000 на процесс вышел бы на 2 часа и больше на конфигурацию, поэтому число снижено и записано здесь как число этого gate). Результат: 30 прогонов (3 процесса x 2 и 4 vCPU x корпус и счётчики), все exit 0, **111 000 000 раундов, запрещённых исходов нет**, `late_barrier_rounds` 0; на 2 vCPU 14 fixture и три счётчика (IRIW — `not_applicable`), на 4 vCPU 15 fixture, три счётчика N = 2 и два счётчика N = 4; наблюдены все исходы x86-TSO, кроме `00` у `SB` и `SB-LOCK-LOCAL` (недостижим при `SeqCst`), IRIW — 15 из 15 (`litmus-parallel/summary.md`)

**Чувствительность, часть 1 — host (шаг 2).** `host_order_litmus.rs`: два потока host в тесном цикле, без интерпретатора, вызывают `PhysicalMemory::store_u32`/`load_u32` памяти в параллельном режиме. Раунд на своей паре ячеек (`x` и `y` на разных страницах и кеш-линиях), сброс и публикация host-барьером на спине, псевдослучайная задержка 0…3 `spin_loop`, доступы, второй барьер. MP: P0 пишет `x = 1`, `y = 1`; P1 читает `y`, `x`; запрещено `r(y) = 1, r(x) = 0`. 2+2W: P0 пишет `x = 1`, `y = 1`; P1 пишет `y = 2`, `x = 2`; запрещено итоговое `(x = 1, y = 2)`, `(x = 2, y = 1)` допустимо. Штатная сборка (обычные доступы `SeqCst`): **100 000 000 раундов каждого шаблона, ни одного запрещённого исхода** (46,5 с). Сборка с `weak-host-ordering` (обычные доступы `Relaxed`, host-fence не ставятся, `Release`-подъём поколения страницы тоже `Relaxed`: он после каждой записи упорядочивает записи одного vCPU на практике): запрещённый MP **наблюдается** — 334 и 462 случая `r(y) = 1, r(x) = 0` из 100 000 000 раундов в двух прогонах (`host-order-weak-100M-run1.txt`, `-run2.txt`), штатная сборка — 0 из 100 000 000; 2+2W не наблюдается ни разу (какой порядок нарушен, тест не различает; 2+2W, которому нужна перестановка записей, не наблюдался). Данные — `host-order-*.txt`. Сборка с ослаблением включена в силе проверкой порядка обращений: тест `the_parallel_accesses_use_the_order_of_the_build` читает записанный порядок (`SeqCst` в штатной, `Relaxed` в ослабленной) и число host-fence, а существующий `a_fence_places_a_host_fence_only_in_the_parallel_mode_and_changes_no_byte` в ослабленной сборке падает.

**Что именно ослаблено в сборке `weak-host-ordering` (ревью S6-36, условие (в)).** Чувствительность показана для порядка пути записи ЦЕЛИКОМ: обычные доступы `SeqCst` -> `Relaxed`, host-fence на гостевые fences не ставятся, а `Release`-подъём поколения страницы (`physical_memory.rs`, `fetch_add`) тоже `Relaxed`. То есть показано, что сломанный путь в сумме виден, а не что каждое из трёх звеньев по отдельности нужно. Кеш кода в такой сборке НЕ гарантирует публикацию записи (читатель, увидевший новое поколение, не обязан видеть данные): допустимо только для тестовой сборки. Факт: предварительный прогон с `Release`-поколением (ослаблены только обычные доступы и fence) не сохранён; решение ослабить и поколение принято по результатам прогона, который не сохранён; данных о нём в `docs/reports/data/stage6/s6-36/` нет, и они здесь не восстанавливаются.

**Статистическая сила gate (ограничение, ревью S6-36, условие (а)).** При 0 запрещённых исходов из 3 000 000 раундов (правило трёх) верхняя граница 95% на раунд — около 1·10^-6 на тест и конфигурацию; при кандидате 30 000 000 — около 1·10^-7. Это не вероятностная гарантия: раунды не независимы (барьеры, цикл размещений, один host), другой host (Intel, другое поколение ARM) не проверялся. Гостевой корпус не измерил чувствительность к перестановкам host: он ловит неатомарный `LOCK` (на 200 000 раундов), но в сборке `weak-host-ordering` не прогонялся; чувствительность к перестановкам показана только host-тестом (MP, 334 и 462 из 100 000 000, то есть около (3–5)·10^-6 на раунд). Основание достаточности барьеров — источники S6-04 (`MEMORY_MODEL.md`) и S6-06 (замер host-примитивов), а не этот gate. Открыто (не обязательно): прогнать корпус в `weak-host-ordering` на тех же 1 000 000 раундов на процесс и записать, видит ли он перестановку.

**Чувствительность, часть 2 — гость (шаг 3).** Feature `nonatomic-lock-test` (другой, не связанный с первым) делает блокированные операции RAM (`atomic_compare_exchange`, `atomic_swap`, `atomic_fetch_add`, `atomic_rmw`) обычным чтением и обычной записью. Тест `parallel_run::tests::the_lock_inc_counter_is_exact_and_loses_updates_in_the_nonatomic_lock_build`: счётчик `LOCK INC dword [x]` на 2 и 4 vCPU по 20 000 на vCPU. Штатная сборка: счёт точен (по три попытки на конфигурацию). Сборка с feature: счёт НИЖЕ точного — 2 vCPU: потеряно 87 из 40 000 инкрементов (в предварительном прогоне 38), 4 vCPU: 752 из 80 000 (289) (`lock-inc-sensitivity.txt`); тест падает, если ослабленная сборка считает точно. Ранее написанные тесты с точным счётом (`the_aligned_lock_counter_is_exact_on_two_and_four_vcpus`, `a_split_lock_is_atomic_against_an_aligned_lock_on_the_bytes_they_share`) в этой сборке тоже падают. Корпус litmus в параллельном режиме с этим feature (`nonatomic-lock-corpus/`, 2 vCPU, 200 000 раундов): запрещённые исходы в `LOCK-CMPXCHG-MUTEX`, `LOCK-XADD`, `SB-LOCK`, `XCHG-MUTEX` и в счётчике `LOCK-INC` (N = 2, K = 2 и N = 4, K = 2): гостевой litmus ловит неатомарный `LOCK`. Split `LOCK` через шлюз (шаг 8 S6-34) от feature не страдает: операция выполняется при остановленных остальных vCPU.

**Шаг 4 (итог по основаниям G5).** Часть 1 показала запрещённый исход в ослабленной сборке, поэтому запись «не наблюдалось» не нужна; она ограничена: чувствительность host-теста к ослаблению — около (3–5)·10^-6 запрещённых исходов на раунд MP (нарушение порядка чтения), перестановка записей на этом host (Apple M3 Max, ARM64) не видна. Основание G5 по-прежнему три: часть 2 показала потерю, штатный прогон чист, достаточность барьеров обоснована источником (S6-04, выбор S6-06), а не прошедшим тестом.

**Условия 1–3 переноса из S6-35 (m2 ревью S6-34) — закрыты; окно сужено, не устранено (ревью S6-36, M-1: ожидание флага вернулось, п. (3)).** (1) Барьер потока: если `safe` уже поднят и `stop_all` прочитан как true, поток сразу паркуется, не опуская флаги; иначе прежний порядок «сбросить, затем читать». Спящий vCPU, разбуженный kick во время остановки (остановка уже в силе и `safe` поднят), больше не опускает `safe`; остаточное окно потока, разбуженного ДО остановки, — в (3). (2) `ParallelConfig::barrier_window` (`#[cfg(test)]`, по образцу `pre_sleep`): вызывается между сбросом `safe`/`quiet` и чтением запросов. Тест `a_kick_of_the_stopped_ap_never_lowers_safe_in_the_barrier_and_the_snapshot_is_taken`: машина остановлена, AP спит в `HLT` (его `safe` поднят), зацепка держит поток AP в окне, если он туда попадает; контроллер ставит IPI спящему AP, ждёт до 100 мс, вызывает `take_parallel`; 20 раундов. На старом барьере (мутация: `let standing = false && ...`, возвращена) поток удерживается в окне в каждом раунде, `take_parallel` отказывает `StaleSafePoint` (20 из 20 раундов), тест падает; на новом поток в окно не попадает, снимок взят целиком и верно (IRR AP содержит вектор IPI). (3) `READ_WAIT` **возвращён** как страховка (ревью S6-36, M-1; первоначальное решение «убран» отменено, утверждение «между `stop_all`, давшим доказательство, и `resume` ни один поток не опускает `safe`» было ЛОЖНЫМ). Остаточное окно: поток, разбуженный kick до остановки, читает `standing = safe && stop_all = false` (`stop_all` ещё не поставлен); контроллер ставит `stop_all` и получает `StopProof` (`safe` потока ещё поднят с прошлой остановки); поток выполняет `safe.store(false)`, читает `stop_all` (true) и снова поднимает `safe`. Окно — несколько инструкций: поток в нём не держит мьютекс vCPU и ничего в vCPU не меняет, поэтому состояние не портится, но `with_vcpu` в этот миг видит `safe` опущенным. Закрытие: `VmSnapshot::take_parallel` ожидает флаг (`yield_now` в цикле) до `READ_WAIT` = 2 с (окно — микросекунды; 2 с покрывают поток, снятый хостом с ядра в этот миг), при снятой остановке во время ожидания — `NotStopped`; по истечении — отдельный вариант `SnapshotError::SafeFlagTimeout { cpu }` (НЕ `StaleSafePoint`: доказательство не устарело, остановка действует, поток медленный; повторить можно). `StaleSafePoint` остаётся для чужого доказательства: другое число подтверждений, ушедшие часы, vCPU не такой, каким подтвердил. Для теста добавлена вторая `#[cfg(test)]`-зацепка `ParallelConfig::standing_window` (между чтением `standing` и `safe.store(false)`), тест `a_thread_between_its_two_stores_in_the_barrier_is_waited_for_and_not_refused`: поток AP удерживается в `standing_window` после resume, контроллер ставит остановку и получает доказательство, поток отпускается и удерживается вторично в `barrier_window` (`safe` опущен под остановкой с доказательством); `take_parallel_within` с 100 мс даёт `SafeFlagTimeout { cpu: 1 }` и остановка остаётся; `take_parallel` с ожиданием по умолчанию, пока помощник отпускает поток через 150 мс, снимает снимок целиком (без ожидания — ложный отказ). Тесты S6-34, S6-35 не менялись и проходят; в `parallel_snapshot_tests.rs` в существующие литералы `ParallelConfig` добавлены только поля `barrier_window: None` и `standing_window: None`.

**Features и штатная сборка.** `weak-host-ordering` и `nonatomic-lock-test` объявлены в `crates/od-machine/Cargo.toml` без вхождения в `default` (в крейте нет `default`), ни один пакет рабочего пространства их не включает (`cargo metadata`: `od-host`, `od-runner` — features `[]`, `od-bench` — только `experimental-translation-probe-cache`; `features-check.txt`). Вывод `--stage6-litmus --parallel` несёт поле `build` (`weak_host_ordering`, `nonatomic_lock_test`); тест `stage6_litmus_parallel_cli` проверяет, что в штатной сборке оба `false`. Предупреждение: `cargo build --all-features` или `cargo test --workspace --all-features` включит оба, и тесты с точным счётом и порядком `SeqCst` в такой сборке падают; такие сборки не штатные.

**Ограничения.** (а) Гостевой litmus между двумя гостевыми доступами исполняет сотни инструкций host и почти не способен поймать аппаратную перестановку: основание отсутствия запрещённых исходов — источник (S6-04), host-тест и корпус дополняют его, не заменяют. (б) Наблюдаемое множество корпуса — подмножество SC: в параллельном режиме обычные доступы `SeqCst`, `SB` с исходом `00` не достигается, это разрешённое поведение, которого эта реализация не воспроизводит. (в) Распределение исходов зависит от скорости интерпретатора; задержка после барьера и размещения разнообразят их (в каждом процессе наблюдены все исходы множества x86-TSO, кроме `00` у `SB` и `SB-LOCK-LOCAL`, который при `SeqCst` недостижим), но не дают перебора. (г) Окно барьера опускания `safe` закрыто только для случая, который описывает условие 1 (поток, пришедший к барьеру с поднятым `safe` под действующей остановкой). Остаётся окно потока, разбуженного ДО остановки (M-1 ревью, условие 3 п. (3)): несколько инструкций между чтением `standing` и повторным подъёмом `safe`; поток в них не держит мьютекс vCPU и ничего не меняет, состояние не портится; ложный отказ закрыт ограниченным ожиданием флага в `take_parallel` (2 с, затем `SafeFlagTimeout`), точный тест — `a_thread_between_its_two_stores_in_the_barrier_is_waited_for_and_not_refused`. Других мест, где поток под остановкой опускает `safe`, в коде нет (проверено чтением и тестами 300 + 20 остановок + тестом окна). (д) Наблюдаемость восстановления пробуждений через `idle_heals` передана в S6-37. [Результат](../reports/STAGE_6_REPORT.md#s6-37--наблюдение-восстановленных-пробуждений).

## S6-37 — LinuxDirect на рабочем потоке vCPU

`run_linux_direct_board_set_user_mode_threaded` принимает загруженную плату с одним BSP, профиль Stage4A или Stage4AApic и пределы кванта и времени ожидания. Stage4A проводит PIT и UART через 8259, а Stage4AApic проводит PIT и UART через IOAPIC INTIN2 и INTIN4 к LAPIC и включает доступ к `IA32_APIC_BASE`. Обе конфигурации используют один интерпретатор CPL0/CPL3 на общей шине. Если плата уже использует 8259, выбор Stage4AApic возвращает ошибку с нулём исполненных инструкций. Кеш трансляции инструкций при включённом `fetch_cache` принадлежит потоку и проверяет поколения таблиц. `ClockOwner` начисляет завершённые инструкции, время устройств и простой. Пробуждение из HLT и доставка исключения сами не расходуют бюджет инструкций.

Квант сохраняет полный отчёт интерпретатора. При терминальном гостевом результате поток заканчивает начисление выполненного префикса и только затем публикует `GuestExit`. Ошибки исполнения на стороне host получают `run_failed` с числом уже начисленных инструкций. Таймаут запрашивает остановку на границе рабочего потока и ждёт его выхода. После join отчёт строится из состояния BSP, включая его CR3 и диагностическое чтение по итоговому CR3 с правами гостя. Если обычный запуск изменил временную ячейку CF8, при переходе на поточный режим адрес переносится в устройство PCI до старта работника. Поточный отчёт и продолжение запуска используют адрес CF8 устройства платы. [Проверки](../reports/STAGE_6_REPORT.md#s6-37--linuxdirect-на-рабочем-потоке-vcpu).

## S6-41c — указатель на операнд RAM и поправка (в′) к S6-30a (нормативно)

Дата: 2026-11-18. Срез S6-41c принят после независимого ревью `gpt-6.1-sol/high`. Прежние разделы S6-30a, S6-30b, S6-31 не менялись; раздел S6-30a, абзац «Поправка S6-41b к инварианту (в) (текст; код не менялся)», здесь становится нормативным (условие «вступает в силу вместе с inline-эмиттером памяти» выполнено срезом S6-41c) и уточняется. Реализация: `crates/od-machine/src/ram_storage.rs` (`RamStorage::operand_ptr`), `crates/od-machine/src/physical_memory.rs` (`PhysicalMemory::atomic_operand_window`, `AtomicOperandWindow`), `crates/od-machine/src/paged_memory_adapter.rs` (`locate_paged_atomic_operand`, `PagedAtomicOperand`, `with_paged_cpl0_set_memory`). Нормативный текст со стороны JIT — [JIT_CONTRACT.md, S6-41c](JIT_CONTRACT.md#s6-41c-inline-эмиттер-памяти-для-атомарных-операций).

**Поправка (в′): что нормативно.**
- `unsafe`, который трогает байты RAM или делает арифметику над указателем хранения, разрешён ровно в двух местах `crates/od-machine/src/ram_storage.rs`: в макросах `cell_access!`/`cell_rmw!` (как и было) и в одной функции `RamStorage::operand_ptr`. В остальном `od-machine` таких мест нет, кроме тестовых проверок (`cfg(test)`, например `AtomicU32::from_ptr` над окном в тесте `physical_memory.rs`). Третье место прежней поправки — вызов опубликованного кода (`PublishedCode`) и `extern "C"` helper блока в `od-host` — к байтам RAM не относится: указатель на RAM они получают готовым из `od-machine`. В JIT-эмиттере (`crates/od-jit`) `unsafe` в пути, выдающем блок, нет (он выдаёт слова команд).
- `RamStorage::operand_ptr(offset, width)` выдаёт `*mut u8` на естественно выровненный операнд ширины 1, 2, 4 или 8 внутри хранения и `None` во всех остальных случаях (иная ширина, выход за хранение, невыровненный host-адрес). Выравнивание проверяется по host-указателю (`lead + offset` кратно ширине), а не выводится из гостевого адреса: требование MEMORY_MODEL («Rust atomics»). Срок действия: хранение (`Vec<AtomicU64>`) создаётся один раз в `new_zeroed` и не перемещается и не меняет размер, поэтому указатель действителен, пока жив `PhysicalMemory`. Указатель не выдаётся за пределы вызова: `PhysicalMemory::atomic_operand_window` возвращает `AtomicOperandWindow<'m>`, заимствующий память; сырой указатель живёт в записи вызова одного блока и не кладётся в кэш. Ссылки Rust (`&[u8]`, `&mut`) на байты по этому указателю не делаются.
- (а′) сохраняется по построению: по указателю код обращается к байтам только инструкциями, атомарными на host, шириной ровно операнда (`LDADD`/`SWP`/`CAS`/`LDSET`/`LDCLR` в блоке S6-41c); `ldp`/`stp`, memcpy-подобные последовательности и невыровненный доступ в блоке запрещены. Невыровненный операнд идёт не inline: помощник отказывает (`PagedAtomicOperand::Misaligned`, `MisalignedAtomic` у окна) до обхода страниц, без счётчика отказов split `LOCK` и без эффекта; шаг интерпретатора, который несёт шлюз S6-34 (шаг 8), исполняет такую инструкцию (проверено: после отказа интерпретатор даёт то же состояние, что и без JIT).
- (б′) выполнено: окно выдаётся только после проверки прав и выравнивания (обход `locked_target` с намерением записи, затем `atomic_operand_window`), а повышение поколения и отчёт `committed_write` делает код блока после записи данных (порядок: данные, `fetch_add` с release по поколению каждой затронутой страницы, отчёт; JIT_CONTRACT S6-40, обязательство 1).

**`PhysicalMemory::atomic_operand_window(address, width)`.** Окно для одной inline-операции: host-указатель операнда и счётчик поколения страницы (выровненный операнд до 8 байт не пересекает страницу; массив поколений в записи вызова блока — до двух счётчиков). Ничего не пишет и не повышает. Ошибки до любого эффекта и без счёта: ошибки региона записи (`Unmapped`, `AccessCrossesRegion`, `AddressOverflow`), `InvalidPhysicalWidth` для иной ширины, `ReadOnlyRegion` для ROM, дыр и framebuffer (память устройства без поколений, операнд остаётся интерпретатору), `MisalignedAtomic` (не считается в `split_lock_refusals`), `CodeGenerationExhausted`.

**`locate_paged_atomic_operand` — «перевести и проверить» с намерением записи.** Обход — тот же `PagedMovMemoryAdapter::locked_target` (`RamAccessOperation::AtomicTarget`), что у `atomic_*_integer` интерпретатора: обход на чтение, затем на запись (Present, Write с WP на CPL0, User, зарезервированные биты; accessed и dirty-биты; #PF записи), затем проверка физического региона. Намерение записи действует на любой исход операции, поэтому ветка неравенства `CMPXCHG` берёт тот же #PF на странице только для чтения и ставит тот же dirty-бит на записываемой: x86 в этой ветке пишет назначение обратно, и `PhysicalMemory::atomic_compare_exchange` у интерпретатора всегда сообщает запись (`(old, true)`), поэтому и поколение страницы у него растёт при неравенстве; inline-блок повышает поколение безусловно, как интерпретатор (ARM `CAS` при неравенстве память не пишет, лишнее повышение поколения безопасно). Невыровненный операнд отклоняется до обхода (ни accessed, ни dirty). Прочие результаты (MMIO, ROM, framebuffer, пробел, ошибка backend обхода) — `Declined`: инструкция остаётся интерпретатору, обход мог поставить accessed-биты ровно так, как их поставит обход интерпретатора. Записи таблиц страниц обхода уходят в колбэк `committed_write` сразу, запись операнда — после возврата блока.

**Тесты.** `ram_storage::tests::operand_ptr_is_naturally_aligned_in_range_and_refuses_everything_else`, `ram_storage::tests::a_host_atomic_through_the_operand_pointer_is_seen_by_the_safe_loads`; `physical_memory::tests::the_atomic_operand_window_serves_aligned_ram_and_refuses_the_rest`; `paged_memory_adapter::tests::locating_an_atomic_operand_leaves_what_the_interpreters_unequal_cmpxchg_left` (4 ширины × 7 листьев (в том числе только для чтения, не present, без dirty) × 2 варианта accessed/dirty верхних уровней: таблицы, #PF (структурно равный `ArchitecturalFault`), уведомления и окно равны результату `atomic_compare_exchange_integer` на двойнике памяти), `paged_memory_adapter::tests::a_misaligned_atomic_operand_is_declined_before_any_walk`.

**Не входит.** Допуск блока в `PagedFetchedLoadSession` (S6-41d); split `LOCK` inline (идёт через интерпретатор и шлюз S6-34); `unsafe`-функций вне перечисленных мест нет, но слой выше (`od-host`, `atomic_inline.rs`) вызывает опубликованный код и `extern "C"` helper по контракту JIT_CONTRACT S6-41c.

## S6-41d — расширение API: `MachineBus::atomic_inline_memory`

Публичный метод `MachineBus::atomic_inline_memory(&self) -> &PhysicalMemory` отдаёт общую ссылку на память шины. Он дублирует `pub(crate)` `physical_memory()` и расширяет публичный API `od-machine` (обоснование: inline-helper атомарных блоков `od-host` и host-тесты S6-41d работают на `PhysicalMemory`). Ссылка не позволяет менять карту памяти и режим исполнения (`set_execution_mode` требует `&mut`). Устранение дубля или сужение видимости — вне S6-41d (см. `JIT_CONTRACT.md`, S6-41d, ограничения).

## S6-42b — привязка JIT-цикла к потокам S6-34 и A7 (расширение API)

Дата: 2026-11-18. Не меняет порядок замков, правила 1–3 порядка замков S6-33 и нормативный текст S6-30…S6-36; фиксирует, что `od-machine` теперь отдаёт хосту JIT.

- **Методы шины на `&self`.** `MachineBus::{access_tracked_ram, access_tracked_2m_ram, tracked_code_snapshot, tracked_code_snapshot_with_callback, tracked_code_snapshot_two_pages_with_callback, read_tracked_bounded_ram_read, read_tracked_bounded_ram_read_with_fault_trace}` и приватные `capture_code_page`, `read_virtual_ram_diagnostic` берут `&self`: они ходят только по `PhysicalMemory` (обходы страниц принимали `&PhysicalMemory` и раньше), устройств не касаются. Вызовы с `&mut MachineBus` компилируются как прежде.
- **Адаптеры JIT на `&MachineBus`.** `with_paged_mov_memory`, `with_paged_mov_dword_memory`, `with_paged_mov_store_memory` принимают `&MachineBus` (профили `ram_only`: устройство не достигается). Вид `AdapterBus::Shared(&MachineBus, vcpu)` обслуживает то же, что `Machine`, кроме путей, которым нужна таблица устройств (`access_tracked`): для них типизированная ошибка, как у вида `Ram`.
- **Интерпретатор на `&MachineBus`.** `run_paged_probe_shared(state, &MachineBus, vcpu, cr3, efer, context, budget, rflags_defined, stop, committed_write)` — базовый профиль `run_paged_probe` (тот же `run_paged_probe_inner`, шаг вынесен в `ProbeStep::run`, общий для обоих видов шины). Порты — через `MachineBus::split_for_set_step_shared`: замок устройств берётся на одно обращение и отпускается до возврата (правило 3).
- **Источник платы у загрузочного процессора.** В `parallel_run` (S6-34) поток vCPU 0 держал замок `irq` через весь вызов кванта. `run_threads_with(.., source_free = true, ..)` (квант, который источник не использует) берёт замок источника и замок владельца часов только на расчёт бюджета; обычный `run_threads` (`source_free = false`) не изменился. Тест `parallel_run::tests::a_source_free_run_holds_no_board_lock_across_the_quantum_call` видит оба случая.
- **Публичный фасад `host_parallel`** (`run_host_threads`, `HostParallelConfig`, `HostParallelControl`, `HostQuantumCall`, `HostQuantumFn`, `HostThreadExitFn`, `HostQuantumExit`, `HostQuantumResult`, `HostParallelReport`, `HostParallelFinish`, `HostStopProof`, `HostStopError`, `host_parallel_board`, `host_install_vcpu`): потоки, барьер, kick, `stop_all`/`resume` и начисление часов — те же, что у интерпретатора; квант — функция хоста над CPU-состоянием своего vCPU и `&MachineBus`. `run_host_threads` принимает хук выхода потока (`HostThreadExitFn`): он вызывается на потоке vCPU после его последнего кванта и до учёта `threads_exited`, чтобы состояние потока (сессия JIT и часть region) освобождалось владельцем-потоком, а не деструктором `thread_local!`, который `thread::scope` не ждёт. **Шлюз split `LOCK` квант хоста НЕ получает:** `HostQuantumCall` без gate, `split_for_set_step_shared` даёт `gate: None`, `AdapterBus::Shared::split_gate()` — `None`; гостевой split `LOCK` на этом пути — отказ `MisalignedAtomic` и ошибка run, не проход через шлюз (опрос `other_split` работает; открыто, S6-42c). `HostParallelControl::calls(cpu)` паникует при `cpu >= vcpu_count()`. `parallel_run` остаётся `pub(crate)`. Квант не получает источник платы, UART, PIT, лог COM1 и не доставляет прерывания (S6-42c в `JIT_CONTRACT.md`, S6-42b).
- Контракт JIT — [JIT_CONTRACT.md, S6-42b](JIT_CONTRACT.md#s6-42b-привязка-к-parallelcontrol-и-a7-над-machinebus-без-замка).

## S6-42c — расширение API `host_parallel`: граница прерывания, шаг набора CPL0, шлюз и корни NMI

Дата: 2026-11-18. Порядок замков (S6-33) и нормативный текст S6-30…S6-36 не меняются. `HostQuantumCall` получил `lapic` и `gate` (`HostSplitGate`, непрозрачная обёртка над шлюзом split `LOCK` потока; `HostSplitGate::none()` — отказ split-операнда, как до S6-42c); новые функции `host_deliver_pending_irq` (доставка вектора своего LAPIC через IDT гостя, тот же `LapicIrqSource` и та же доставка, что у цикла CPL0-набора) и `run_host_set_step` (одна инструкция набора CPL0 над `&MachineBus` с шлюзом и доставкой гостевых `#PF`/`#GP(0)`); `run_host_threads` принимает `nmi_roots` (корень для NMI vCPU без CR3; без корня NMI — ошибка run, адрес 0x1000 не читается); `HostParallelControl::lapic`; `HostParallelReport::{nmi_delivered, split_locks}`. Пути интерпретатора не менялись. Подробности — [JIT_CONTRACT.md, S6-42c](JIT_CONTRACT.md#s6-42c-прерывания-исключения-и-шлюз-split-lock-в-квантах-хоста).

В S6-42d `run_host_threads_with_nmi_paging` получил по одному `HostNmiPaging { root, efer }` на vCPU. `JitHostQuantum::nmi_paging` берёт оба значения из настроек того же кванта. Собственный CR3 vCPU имеет приоритет над переданным корнем. EFER для обхода берётся из переданной пары без изменения гостевого MSR. При отсутствии пары требуются собственный CR3 и EFER vCPU, при отсутствии корня NMI завершается явной ошибкой. `run_host_threads` с передачей только корней и интерпретатор используют гостевой EFER. Пара фиксирована на время запуска.

В S6-42d нативный путь принимает исходный `#PF` с соответствующей записью обхода страниц или `#GP(0)`. В потоке vCPU `HostSplitGate::deliver_native_fault` доставляет исключение через IDT с параметрами paging текущего кванта. Интерпретатор использует тот же шлюз с адаптером исходного шага. Уведомления о записях кадра, Accessed и обхода страниц выдаются после освобождения шлюза. Успешная доставка сохраняет бюджет инструкции. При отказе доставки исходная ошибка остаётся в отчёте, а при прерванном входе указывается и причина отказа. Предел в 256 последовательных доставок действует между квантами одного потока vCPU. Завершённая инструкция или `INIT` обнуляет счётчик. Прямой `deliver_host_native_fault` предназначен для вызывающей стороны с исключительным доступом к RAM.

CPL0-квант JIT использует `run_host_set_step` с профилем CPU-owned CR3. После записи `MOV CR3` квант передаёт новое значение из `state.cr3` следующему шагу и границе IRQ. Для NMI то же состояние имеет приоритет на границе рабочего потока. [Контракт JIT](JIT_CONTRACT.md#s6-42c-прерывания-исключения-и-шлюз-split-lock-в-квантах-хоста).

`HostQuantumCall::skip_first_boundary` даёт CPL0-кванту признак рестарта REP из состояния vCPU. Вызов без гостевого шага сохраняет признак. При частичном REP этот же признак участвует в подтверждении STOP, а новый рабочий поток той же платы продолжает REP перед опросом маскируемых IRQ.

Для профиля `with_cpl0_cpuid` топологию задают APIC ID из `HostQuantumCall::cpu` и число процессоров платы из `HostQuantumCall::vcpu_count`. Квант получает число процессоров из конфигурации CPUID своего vCPU.

`HostQuantumCall::board_irq` даёт BSP источник платы для текущего вызова. `HostBoardIrqSource::deliver_pending` применяет порядок источника платы и LAPIC, общий с квантом интерпретатора. В режиме `source_free` общий замок IRQ берётся отдельно на выбор и подтверждение. Кадр через IDT записывается между операциями. `HostSetStepReport::pit_written` сообщает об успешной записи в программирующий порт PIT. Если источник задаёт срок от PIT, квант заканчивается после этой инструкции. Выбор источника зависит от конфигурации платы, например `LoadedFirmwareBoard::enable_pit_pic_irq()` подключает IRQ0 через PIC.

`HostQuantumCall::devices` даёт доступ к очередям COM1 и тиков портов. `HostSetStepReport::port_accessed` отмечает успешный PIO, после которого CPL0-квант проверяет ожидающую работу. `HostDeviceServices::service` переносит завершённые байты COM1 в общий журнал и забирает накопленные тики PIT/PM. Квант возвращает суммарный результат через `HostQuantumResult::device_ns`, поток начисляет его владельцу часов. Очереди общие, поэтому тики может забрать другой рабочий поток. Порядок их видимости определяется начислениями в параллельном планировщике.

`HostDeviceServices::stop_request` читает первый запрос ACPI PM. CPL0-квант возвращает `HostQuantumExit::MachineStop` с `MachineStopReason`. После начисления времени и публикации счётчиков рабочий поток завершает прогон с `HostParallelFinish::GuestStop`. Самый высокий приоритет у ошибки или паники рабочего потока. Затем идут запрос гостевой остановки, административное завершение и обычный предел прогона. При равном приоритете сохраняется первый опубликованный результат. Гостевые `HLT` и архитектурный `Shutdown` задают состояние отдельного vCPU. Для запроса reset устройство сохраняет защёлку до операции сброса машины.

<a id="s6-42d-nvme-stop"></a>

`HostSetStepReport::mmio_written` отмечает успешную запись MMIO. CPL0-квант с установленным NVMe проверяет ожидающий doorbell и обслуживает его между вызовами гостевого кода через `HostDeviceServices::service_nvme_until`. Интерфейс `GuestDma` ограничен RAM. Замок состояния NVMe сериализует команду верхнего уровня или выдачу готовых событий AER. DMA обращается к `PhysicalMemory`. После освобождения замка публикуются уведомления о записанных диапазонах, затем поток опрашивает STOP. При занятом замке поток обслуживания снова опрашивает STOP перед следующей попыткой взять замок.

Работу, прерванную на этой границе, worker продолжает после resume в том же прогоне до следующей гостевой инструкции или перехода в простой. Обслуживание после HLT сохраняет гостевое состояние Halted и даёт ноль исполненных гостевых инструкций. Подтверждение STOP ждёт возврата из кванта и учёта его результатов. Бюджет прогона и терминальные запросы сохраняют обычный порядок завершения. Владелец платы выбирает backend при настройке NVMe. [Ограничение STOP](../KNOWN_ISSUES.md#issue-515-s6-42d-nvme-stop-latency).

Для платформы с IOAPIC владелец платы включает `enable_pit_ioapic_irq()` и затем `enable_uart_ioapic_irq4()`. Конфигурация с PCI NVMe дополняет этот источник уровнем INTx через INTIN16. При настроенном маршруте CPL0-квант доставляет прерывание завершения и подтверждает его через существующий источник платы. Гость снимает запрос продвижением CQ head и записью LAPIC EOI.

## S6-41e — расширение API: хук исполнения litmus (`LitmusStepHook`)

Дата: 2026-11-18. Не меняет модель памяти, порядок замков и нормативный текст S6-30…S6-36. `od-machine` отдаёт хосту точку подключения исполнителя инструкций litmus: трейт `LitmusStepHook` (`LitmusStep`, `LitmusStepOutcome`), `run_litmus_parallel_hooked` (параллельный драйвер S6-36 с хуком; без хука `run_litmus_parallel` идёт тем же кодом) и `run_litmus_interleaved_hooked` (перебор S6-19 на шине платы, `LitmusInterleavedReport`), а также `LitmusCase::program()` (инструкции fixture). Драйвер S6-36 пересекает стартовый барьер до хука, как до интерпретатора; `LitmusParallelReport` получил `hooked_retired`/`interpreted_retired`. В прогонах с хуком первые 2 МиБ отображены страницами 4 КиБ (JIT обходит только их). Подробности и результаты — [JIT_CONTRACT.md, S6-41e](JIT_CONTRACT.md#s6-41e-litmus-s6-19s6-36-с-jit-в-обоих-режимах).

## S6-43a — probe cross-modifying code на двух vCPU (без изменений API `od-machine`)

Дата: 2026-11-18. Проба использует существующие `run_host_threads_with_nmi_paging`, `run_host_set_step` и общую `&MachineBus`. Эталон исполняет те же байты гостя в CPL0 без JIT. API `od-machine`, порядок замков и нормативный текст S6-30…S6-36 сохранены. [Контракт и результаты](JIT_CONTRACT.md#s6-43a-probe-cross-modifying-code-на-двух-vcpu-с-включённым-jit).

## S7-31j — порты ввода-вывода, которые плата не объявляет

Порт без привязки по-прежнему останавливает машину (`BusAccessError::UnmappedPort`), а не возвращает `0xFF`: это сознательное правило модели (`absent_ports.rs`, S4-29): гость, обратившийся к неизученному порту, должен быть виден как остановка, а не скрыт значением плавающей шины. Общую замену «любой необъявленный порт читается как `0xFF`, запись игнорируется» не вводим: она сменила бы смысл остановок во всех платах (включая прогоны Linux direct и свидетельства стадий 4–6) и спрятала бы обращения к PIC, PIT, 8042 и прочим устройствам, которые должны падать (тест `linux_profile_keeps_the_8042_and_neighbours_as_traps`). Вместо этого явно объявляются отсутствующие устройства, как для `0xED`, `0x80` и CMOS: под `linux_profile` отсутствующие UART COM2 (`0x2F8..0x2FF`), COM3 (`0x3E8..0x3EF`) и COM4 (`0x2E8..0x2EF`) читаются как `0xFF` (байтовый доступ), запись игнорируется, другая ширина — ошибка устройства. Основание: драйвер 8250 ядра зондирует все четыре устаревшие позиции UART, и отсутствующий UART на ISA-шине отвечает `0xFF` (плавающая шина); COM1 (`0x3F8`) объявлен платой и работает как прежде, соседние порты (`0x2F7`, `0x300`, `0x3E7`, `0x3F0`, `0x2E7`) остаются ловушками. Тесты: `firmware_board::tests::linux_profile_absent_uarts_read_ff_ignore_writes_and_leave_com1_alone`, тесты `absent_ports` на всех шести областях. Каждая следующая остановка на неизученном порту решается так же: отдельным решением о конкретном устройстве.

## S11-32 — модель NIC, совместимая с Intel 82540EM (расширение API)

Дата: 2026-11-22. Не меняет модель памяти, порядок замков и нормативный текст других разделов. `od-machine` отдаёт хосту PCI-функцию `00:06.0` (BAR0 `[0xFC02_0000, 0xFC04_0000)`, INTA на GSI 18): `NicDevice`, `NicHandle`, `NicPciFunction`, `NicConfig`, `NicCounters`, `NicServiceReport`. Хост подключает backend через `NicHandle::attach_backend(Box<dyn NetBackend>)` ([NETWORK.md](NETWORK.md) раздел 4) и обслуживает кольца вызовом `NicHandle::service` между квантами; цикл платы `FirmwareBoardConfig` делает это сам, когда задана опция `nic` (по умолчанию `None`, плата, handoff и хеш ACPI-манифеста байт в байт прежние). Линия прерывания — `IoApicHandle::set_nic_line` (GSI 18, level, active-low). Регистры, кольца, EEPROM, PHY, классы ошибок и границы: [NIC_DEVICE.md](NIC_DEVICE.md). `FirmwareMmio` получил поле `nic`, а таблица окон набора прошивки вычисляется из присутствующих устройств вместо статического массива из 64 вариантов.
