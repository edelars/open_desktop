# Контракт машины: physical memory, bus, bridge and translation probe

**Статус:** локально принят ограниченный срез; это не контракт полной VM и не
подтверждение MMU, устройств, загрузки Linux или JIT-интеграции.

Нормативная карта платы и будущие boot reservations остаются в
[VIRTUAL_BOARD.md](VIRTUAL_BOARD.md) и в принятом manifest S4-00. Этот документ
фиксирует уже реализованные bounded-срезы S4-02—S4-09.

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
  бит 1 `EVENTS`; биты 2–3 зарезервированы, равны `0`). С блоком курсора
  (раздел «Display device, cursor (S10-14)» ниже) `CAPS = 0x7`. Один тип `DisplayDevice` обслуживает
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
- **Не реализовано.** Блок второго экрана, прерывания, snapshot,
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
устройства написан и ожидает ревью. Display host ABI, VMHost, frame transport v2
и UI не реализованы.

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
- **Не реализовано.** Display host ABI и test-only профиль `3`, VMHost, frame
  transport v2, наложение в UI, snapshot, драйверные последовательности курсора
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

`LocalApic` is a one-CPU, fixed-profile xAPIC `BusDevice` at MMIO base
`0xFEE00000`, with APIC ID 0. It supports aligned 32-bit accesses on the
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
Initial Count starts or replaces the countdown, and zero cancels it. Mode and
divider changes are accepted only while no deadline is queued. Vector and mask
changes may be made before expiry without restarting the active phase. The
divider encodings and the read-only Current Count register keep their existing
meaning.

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

Only the Fixed Self edge combination is supported. Other delivery modes,
shorthands, level trigger, invalid vector/reserved bits and invalid widths or
alignment fail without mutation through the local `DeviceError` contract,
not guest exception delivery. No IPI bus, external destination, ESR, CPU
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
IDT/IF, HLT wakeup, JIT/session integration, or default board wiring. Behavior
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
Version и ESR добавлены в S4-25f; CMCI, Thermal, Perf остаются так): чтение и запись дают
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
| Version `0x30` | `0x00030014` (с S4-25f; до него `UnknownOffset`) |

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
= 0 устанавливает mask во всех трёх LVT (SDM §13.4.7.2); снятие mask при
software disable отклоняется (`UnsupportedConfiguration`, без изменений), как
для Timer LVT. Запись masked значения разрешена. Mask не влияет на IRR/ISR/TMR:
ни одна запись в LINT/Error LVT не создаёт запрос.

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

**Perf `0x340`, Thermal `0x330`, CMCI `0x2f0`.** Не реализованы: чтение и запись
дают `UnknownOffset` без мутации. Условие: CPUID гостя не объявляет PMU
(architectural performance monitoring), thermal monitoring (TM) и CMCI
(MCG_CAP.CMCI), а Version отдаёт Max LVT = 3, поэтому Linux не обращается к
`LVTPC`, `LVTTHMR` и `LVTCMCI`. При изменении CPUID эти LVT обязаны быть
добавлены вместе с Max LVT.

Не входят: доставка с LINT-пинов, NMI/SMI/INIT/ExtINT, доставка Error LVT,
выставление ESR, x2APIC, multi-CPU, IOAPIC (S4-26c), CPU delivery, CPUID,
Linux acceptance (S4-38). См. [отчёт S4-25f](../reports/STAGE_4_S4-25F_2026-11-09.md)
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
  24 входа. Read-only: запись — `UnsupportedConfiguration` без изменений.
- **Arbitration ID (`0x02`).** Читается как `ID << 24` (82093AA загружает его из
  ID при записи ID; шины APIC в модели нет, арбитража тоже). Read-only: запись —
  `UnsupportedConfiguration`.
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
  с бит 13 = 1 — `UnsupportedConfiguration` без изменений; INTIN4 остаётся только
  active-high даже под маской; биты 12 и 14
  (read-only) отбрасываются. Vector: для unmasked записи `0x10..=0xFE`; для masked
  записи допустим любой байт (reset-состояние уже masked с vector 0, а Linux
  маскирует входы с vector 0). Расширения относительно S4-26a: любой vector под маской и active-low под маской на входах без линии (SCI GSI 9):
  прежние отказы (vector 0/15/0xFF без маски) сохранены. High dword: принимается
  только 0 (physical destination 0, то есть Local APIC ID 0; reserved 23:0 = 0);
  любое другое значение — `UnsupportedConfiguration` без изменений.

**Недопустимые записи** (vector < 16 при unmasked, `0xFF` при unmasked, любой
delivery mode кроме Fixed — lowest priority, SMI, NMI, INIT, ExtINT и зарезервированные
`011`/`110`, polarity (кроме masked на входах без линии), reserved-биты, ненулевой high dword) всегда
`UnsupportedConfiguration` и не меняют ни один вход, ID, Remote IRR, IRR/ISR/TMR.
Тесты сравнивают полный снимок `PairState` до и после.

**Ограничения S4-26c.** (а) SCI GSI 9 (level/active-low) принимается только
маскированным; разблокировка такого входа — отказ, потому что active-low не
моделируется: риск для S4-29/S4-38 (включение SCI). (б) EOI-регистра IOAPIC по
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

**Согласование с MADT.** MADT прошивки (`firmware/uefi/src/acpi/madt.rs`, константы
`IOAPIC_ID = 1`, `IOAPIC_PIN_COUNT = 24`, блоб-фикстура
`tests/fixtures/guest/stage4/acpi_blob/stage4_acpi_blob.bin`) и модель связаны
тестом `io_apic::tests::model_agrees_with_the_firmware_madt`: ID читается из байтов
MADT фикстуры, число входов (не поле MADT) — из константы прошивки; модель обязана
отдавать тот же ID и MRE = `IOAPIC_PIN_COUNT - 1`. Изменение любой из сторон ломает тест.

Источники: Intel [82093AA I/O APIC document 290566-001](https://intel-vintage-developer.eu5.org/DESIGN/PCISETS/DATASHTS/29056601.PDF),
preliminary release, §§3.1–3.2.4 (регистры и redirection table). В этом срезе
текст документа и SDM заново не перечитывался: поля ID (RW 27:24), Version (RO,
MRE 23:16), Arbitration ID (RO, загружается из ID) и раскладка redirection entry
взяты из принятых S4-26a/b и памяти исполнителя; это НЕ ПРОВЕРЕНО по тексту. См. [отчёт S4-26c](../reports/STAGE_4_S4-26C_2026-11-09.md) и
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
  и устройства handoff block. NVRAM, TPM CRB, ECAM (по умолчанию; с флагом `pci_ecam` — S4-30c, ниже) — записи с `PRESENT = 0`, в
  карту и на шину не добавляются.
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
- Не реализованы: повторное копирование shadow при сбросе (S7-26), NVRAM (S7-10),
  ACPI fixed hardware и reset register (S7-14), TPM CRB (S7-22), ECAM (S4-30),
  NVMe: PCI-привязка и прерывания (S7-16: срезы A и B выполнены, раздел «NVMe controller» ниже), доставка IRQ и APIC timer в paged-пути, SMP, выбор размеров RAM и
  UUID/серийного номера из командной строки runner (используются значения по
  умолчанию: 128 MiB, один экран, нулевой UUID, пустой серийный номер).
- Расширение handoff block (новые поля и устройства) повышает `version_minor`
  по правилам раздела 6.4 FIRMWARE_BOOT.md; машина выдаёт v1.0 и принимает образы
  с любым `handoff_minor_max`; v1.1 (запись NVMe) — только при `FirmwareBoardConfig::nvme = Some(..)`
  и образе с `handoff_minor_max ≥ 1`.

Проверки: `cargo test -p od-machine --offline` (модуль `firmware_board`, `physical_memory`),
`cargo test -p od-runner --offline --test firmware_boot_cli`,
`python3 -m unittest tools/build/test_build_firmware.py`; методика —
[FIRMWARE.md](../testing/FIRMWARE.md).

## NVMe controller (S7-16, срезы A и B)

**Статус: срезы A (управление) и B (ввод-вывод `Read`, `Write`, `Flush`) реализованы
исполнителем, ожидают ревью.** Контракт устройства,
регистры, команды, границы, разрывы и значения без сверки со спецификацией —
[NVME_DEVICE.md](NVME_DEVICE.md); код — `crates/od-machine/src/nvme/`.

- **Шина.** Контроллер — `BusDevice` на одном `Mmio`-регионе `[0xFC00_1000, +0x2000)`
  (`NVME_MMIO_BASE`, `NVME_MMIO_BYTES`), как дисплей; `MachineBus` не менялся.
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
| `\_S5` `SLP_TYP` | `5` (`ACPI_SLP_TYP_S5`; пакет `\_S5` составит S4-31) |

**Регистры.** Доступ — только `IoSpace::Pio`, ширина 1/2/4 (Qword — `UnsupportedWidth`), естественное выравнивание (`Misaligned`), доступ не выходит за один регистр (`CrossesRegister`, в частности 32-битный доступ к `PM1_STS`+`PM1_EN` отвергается), промежутки блока (`0x606`, `0x607`, `0x60D..0x60F`) — `UnknownOffset`; все проверки до изменения состояния.
- `PM1_STS` (`+0`, 16 бит): write-one-to-clear; источников событий нет, поэтому после сброса `0` и не меняется (`TMR_STS`, `PWRBTN_STS` и прочие не взводятся).
- `PM1_EN` (`+2`, 16 бит): хранит биты `TMR_EN`, `GBL_EN`, `PWRBTN_EN`, `SLPBTN_EN`, `RTC_EN` (маска `0x0721`), остальные читаются `0`. SCI-маршрута нет, включённое событие ничего не вызывает.
- `PM1_CNT` (`+4`, 16 бит): `SCI_EN` (бит 0) равен `1` с сброса и записью не меняется; `BM_RLD` (бит 1) и `SLP_TYP` (биты 10–12) хранятся; `GBL_RLS` (бит 2) и `SLP_EN` (бит 13) только для записи и читаются `0`, `GBL_RLS` игнорируется. Запись `SLP_EN = 1` с `SLP_TYP = 5` (с учётом `SLP_TYP`, записанного в той же или предыдущей записи, в том числе побайтно) защёлкивает причину `Shutdown`. Запись `SLP_EN = 1` с другим `SLP_TYP` игнорируется: другие состояния сна не объявлены (`\_S1..\_S4` в DSDT нет), машина не засыпает, `SLP_EN` читается `0`.
- `PM_TMR` (`+8`, 32 бита): `floor(now_ns × 3 579 545 / 10^9)` по virtual clock платы, маска 24 бита (счётчик заворачивается каждые 4.687 с). Время не идёт само: оно двигается только `LoadedFirmwareBoard::advance_time_ns` и `run_boot`; чтение время не двигает. **Ограничение (ISSUE-067):** `run_boot` двигает часы на `instructions_executed` только между квантами, а квант заканчивается на записи в COM1 THR, doorbell NVMe, остановке PM, `HLT` или конце бюджета; внутри кванта `PM_TMR` стоит, и ожидание по нему (UEFI `Stall`, калибровка Linux) не завершится до конца бюджета. Механизм квантов не менялся; решение (закрывать квант на чтении `PM_TMR`) — S4-37/S4-38. Запись — `ReadOnly`.
- `RESET_REG` (`+0xC`, 1 байт): чтение — `WriteOnly`, запись шире байта — `CrossesRegister`. Запись `0x06` защёлкивает причину `Reset`; другое значение игнорируется.

**Остановка.** Причина защёлкивается на устройстве (`MachineStopReason::{Reset, Shutdown}`), первая причина остаётся: повторные команды её не меняют, снять остановку сможет только операция сброса (S7-26). `AcpiPmHandle::stop_request()` читает причину. С флагом `LoadedFirmwareBoard::run_boot` добавляет её опрос в предикат STOP после каждой инструкции: машина останавливается сразу после `OUT`, `FirmwareRunReport { exit: PagedProbeExit::Stopped, stop_reason: Some(..) }`; остановка по запросу не превращается в `BudgetExhausted`, повторный `run_boot` не исполняет ни одной инструкции. Без флага `stop_reason` всегда `None`. Сам сброс (состояние CPU и устройств, перезапуск) этим срезом не выполняется.

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

## Boot contract собственного ядра (S4-32d)

Собственное маленькое ядро этапа 4 — отдельный профиль загрузки, а не протокол Linux и не общий ELF/segment loader: загрузчик принимает только raw image фиксированного размера. Entry state общий для всех probe-образов и зашит в код загрузчика (`stage4_flat_probe.rs`, значения раздела S4-32a, стр. 537–539): Long64/CPL0, CR3 = 0x1000, RIP = 0x400000, RSP = 0xA01000 (у Alias — 0xA00000), RDX = 0x3F8. Artifact не может его изменить. Профили различаются только значением RCX (0, store_value или payload) и CPU-профилем `run_stack` (разделы S4-32b и S4-32c). Собственные page tables лежат в image; загрузчик их не синтезирует, только проверяет. Загрузчик строит собственную маленькую карту (RAM и COM1), а не карту firmware board v1; «production memory» означает production RAM owner на этой карте. Неверный entry, битые code, page tables или stack mapping дают `loader_error` (exit 4) и ни одного UART-байта. Исполнение — только через interpreter. Не входит: размещение на board v1, ELF loader, exception delivery, IRQ, JIT.

Фиксированный direct Long64/CPL0 handoff достаточен для S4-32 и расширяется в S4-33 (CPL3, timer/IRQ, IDT/GDT, exception delivery). Подтверждение пользователя требуется только для фиксации: entry state задаёт код, а не artifact.

## Минимальная пара 8259 (S4-29a)

Реализация: `crates/od-machine/src/pic_8259.rs` (`Pic8259`, `Pic8259Pair`, `Pic8259Handle`, `Pic8259Snapshot`, `isa_irq_vector`). Устройство не подключено к плате (`FirmwareBoardConfig` не менялся, handoff и карта портов платы не затронуты): регистрация и порядок на шине — задача S4-29f/S4-37.

**Порты.** Master `0x20/0x21`, slave `0xA0/0xA1`; по одной `PortRegionBinding` длиной 2 на чип (устройство видит смещение относительно своей привязки, поэтому чипы — два устройства). Смещение 0 — команда (A0 = 0), 1 — данные. Пересечений с COM1 `0x3F8..=0x3FF`, conf1 `0xCF8..=0xCFF` и PM-блоком `0x600..0x610` нет (тест карты); NVMe, ECAM, LAPIC, IOAPIC — MMIO. Доступ: только `IoSpace::Pio` (иначе `UnsupportedSpace`), только байт (`UnsupportedWidth`), смещение 0..=1 (`UnknownOffset`); ошибка возвращается до изменения состояния.

**ISA_IRQ_VECTOR.** В проекте константы не было; взято из Linux x86-64: `ISA_IRQ_VECTOR(irq) = IRQ0_VECTOR + irq`, `IRQ0_VECTOR = FIRST_EXTERNAL_VECTOR (0x20) + 16 = 0x30`. Принимаются ровно ICW2 `0x30` (master) и `0x38` (slave); PC-векторы `0x08/0x70` отвергаются (они пересекаются с исключениями CPU, Linux их не пишет). Это значение вектора, которое S4-29f свяжет с IOAPIC-маршрутом.

**Reset.** `IMR = 0xFF` (всё замаскировано), фаза `Uninitialized`, ICW2/ICW3 не заданы, чтение команды-порта выбирает IRR. OCW1 принимается и в этой фазе (Linux пишет IMR до init и в `probe_8259A`).

**Автомат ICW.** Запись в команду-порт с битом 4 = 1 — ICW1. Принимается только cascade (SNGL = 0), edge (LTIM = 0), 8086 (IC4 = 1); биты 7:5 и бит 2 в режиме 8086 не используются и игнорируются (Linux пишет `0x11`). Принятая ICW1, как у микросхемы, очищает `IMR` в 0, выбирает IRR для чтения и сбрасывает ICW2/ICW3; допустима посреди последовательности (рестарт). Затем запись в порт данных: ICW2 — ровно вектор чипа; ICW3 — master `0x04` (slave на IR2), slave `0x02` (cascade identity); ICW4 — `0x01` (8086, normal EOI); AEOI (бит 1), BUF (бит 3), SFNM (бит 4), reserved биты 7:5 и MCS-80 (бит 0 = 0) отвергаются, бит 2 (M/S) при BUF = 0 игнорируется. Любое недопустимое значение — `DeviceError::UnsupportedConfiguration` без изменения состояния (в том числе фазы), шаг можно повторить. После ICW4 фаза `Initialized`; запись в порт данных — снова OCW1. Чтение порта данных возвращает `IMR` в любой фазе.

**OCW.** Запись в команду-порт с битами 4:3 = 00 — OCW2: принимается любое значение (EOI, specific EOI, rotate, set priority) без эффекта: нет линий и ISR. Биты 4:3 = 01 — OCW3: биты 1:0 (`RR`, `RIS`) выбирают чтение IRR (`0x0A`) или ISR (`0x0B`), `RR = 0` выбор не меняет; poll (бит 2), SMM/ESMM (биты 5:6) и бит 7 — `UnsupportedConfiguration`. Чтение команда-порта возвращает 0 для IRR и для ISR (линий и состояния обслуживания нет). OCW2/OCW3 между ICW1 и ICW4 — `DeviceError::Busy` (поведение микросхемы там не моделируется).

**Доставка.** Ни входных линий, ни выхода INTR; пара ничего не доставляет и не влияет на ExtINT/LINT0.

**Не входит.** ELCR `0x4D0/0x4D1`; ExtINT/LINT0; AEOI; IRQ-линии и маршрут IRQ0 (S4-29f); poll и special mask mode; PIT, CMOS/`0xED`, INTIN2; подключение к плате и handoff; MADT/FADT (`PCAT_COMPAT`). Отчёт: [S4-29a](../reports/STAGE_4_S4-29A_2026-11-09.md).

## Явно отсутствующие порты (S4-29b)

Реализация: `crates/od-machine/src/absent_ports.rs` (`AbsentPorts`, `AbsentPortRegion`). Устройства не подключены к плате (`FirmwareBoardConfig` не менялся, handoff и карта портов платы не затронуты): регистрация — S4-37.

**Зачем.** Порт без привязки даёт `BusAccessError::UnmappedPort` (остановка), а не чтение `0xFF`. Stock Linux обращается к `0x70/0x71` (`CMOS_READ`/`CMOS_WRITE`) и к `0xED` (`outb_p`/`io_delay`), хотя такого оборудования на плате нет. Поэтому они оформлены как устройства, которых определённо нет.

**Порты и поведение.** `0x70` (индекс) и `0x71` (данные): одна привязка длиной 2 (`0x72/0x73` не привязаны). `0xED`: одна привязка длиной 1. Чтение — `0xFF` (плавающая шина данных), запись игнорируется без побочного эффекта. Устройство не хранит состояния: ничего записанного прочитать нельзя, индекс CMOS не запоминается, бит NMI-disable (бит 7 порта `0x70`) не моделируется. Часы RTC и CMOS RAM не моделируются (S7-14).

**Политика ширин.** Только байт. Слово, dword и qword на устройстве — `DeviceError::UnsupportedWidth` без эффекта; MMIO — `UnsupportedSpace`; смещение за привязкой — `UnknownOffset`. Основание: единая политика PIO-устройств модели (8259, UART, conf1 отказывают, а не выдумывают значение), Linux на этих портах использует только `inb`/`outb`; обращение шире байта означает, что гость делает то, что модель не аудировала, и остановка информативнее выдуманного `0xFFFF`. На шине: слово на `0x70` доходит до устройства (`Device{UnsupportedWidth}`), слово на `0x71`, dword на `0x70` и любое широкое обращение к `0xED` пересекают границу привязки — `BusAccessError::CrossesRegion` до устройства, qword — `UnsupportedWidth` на шине.

**Связь с FADT.** Решение «CMOS/RTC отсутствует» закреплено в двух местах: здесь (`CMOS_RTC_PRESENT = false`, порты `0x70/0x71` — устройство «absent») и в FADT (`IAPC_BOOT_ARCH` бит 5 `CMOS_RTC_NOT_PRESENT = 1`, `firmware/uefi/src/acpi/fadt.rs`). Тест `model_agrees_with_the_fadt_iapc_boot_arch` читает байты FADT из закоммиченного ACPI-blob (смещение 109) и текст константы прошивки, сверяет их с моделью; `LEGACY_DEVICES` (бит 0) и `8042` (бит 1) в FADT равны 0. Если S7-14 добавит RTC, обе стороны меняются вместе: тест упадёт, пока этого не сделано. Замечание: порт `0xED` в FADT не отражается.

**Остаются без привязки (решение открыто).** `0x60/0x64` (8042) и `0x80` (POST) не имеют устройства; обращение — `UnmappedPort` (остановка). Это ловушка, а не решение: пока пользователь не выбрал (модель, sink, остановка), устройств для них нет; тест `open_decision_ports_stay_unmapped` фиксирует состояние и должен быть изменён осознанно. FADT при этом объявляет `8042 = 0` и `LEGACY_DEVICES = 0`.

**Не входит.** RTC, CMOS RAM, NMI-бит, 8042, `0x80`, PIT (S4-29c), подключение к плате и handoff. Отчёт: [S4-29b](../reports/STAGE_4_S4-29B_2026-11-09.md).

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

**Порт-тик.** Каждое успешное чтение порта B `0x61` — чтение «временного порта»: устройство `PitPortB` считает его (`port_b_reads`) и ставит в очередь `PIT_PORT_B_READ_TICK_NS = 1000` нс виртуального времени (`take_port_tick_ns` забирает и обнуляет очередь). Значение, которое вернуло чтение, снято до тика. Устройство само часы не двигает: у `VirtualClock` один продвигающий — сессия vCPU (S4-22a/b, S4-28a2). Владелец часов забирает очередь после инструкции и продвигает часы на неё. Отказанное чтение (правила gate 2) тик не ставит. Записи в `0x61`, обращения к `0x40..0x43` и управляющие слова тик не ставят; чтение счётчика `0x42` и `PM_TMR` в этом срезе временными портами не объявлены (решение п. 5 перечисляет их, срез S4-29d по постановке — только `0x61`).

**Что владелец применяет.** Профиль `Stage4PitCalibrateProbeV1` берёт очередь после каждой PIO-инструкции и вызывает `DiagnosticVcpuSession::advance_time_ns`. Время по числу выполненных инструкций в этом профиле не идёт (одна инструкция — 0 нс); единственный источник времени — порт-тик. Поэтому TSC (1 ГГц, один такт на наносекунду) и OUT2 двигаются только чтениями `0x61`. `LoadedFirmwareBoard::run_boot` и `PagedVcpuSession` порт-тик не применяют (ISSUE-067 в их путях остаётся открытой до их подключения к PIT).

**Ограничения (расхождение с решением п. 5 плана, по ревью Opus).** Тик на `0x42` и на `PM_TMR` в S4-29d не объявлен: решение п. 5 перечисляет их, срез по постановке — только `0x61`. Синхронизации часов по выполненным инструкциям нет: инструкция = 0 нс в этом профиле. Это записано как открытое; закрытие ISSUE-067 перенесено в S4-37a. Без `0x42` и `PM_TMR` Linux `quick_pit_calibrate` на этой модели время не продвигает.

**Gate 2 (решение п. 6): отказ без остановки счёта.** С `UnsupportedConfiguration` и без изменения состояния отвергаются: запись данных канала 2 (загрузка), пока gate 2 = 0; чтение счётчика канала 2, counter latch канала 2 и чтение порта B (бит 5 = OUT2) при загруженном канале 2 и gate 2 = 0; запись в порт B, опускающая gate 2, пока канал 2 загружен (в режиме 0 канал остаётся «загруженным» и после терминального счёта, до нового управляющего слова). Чтение порта B при незагруженном канале 2 допустимо при любом gate. Gate счёт не останавливает и не перезапускает. Состояние «загружен при gate 2 = 0» через порты недостижимо (загрузка требует gate 2 = 1, сброс отвергается); правило чтения проверяется тестом с принудительно выставленным состоянием.

**Профиль `Stage4PitCalibrateProbeV1`.** Плоский identity-образ Long64/CPL0: GPA `0x1000`, размер `0x4000`, RAM `0x7000`, GDT `0x2000`, код `0x4000`, стек `0x6ff0`; привязки PIO `0x40..0x43`, `0x61`, COM1 `0x3f8` (polling TX). RDTSC читает общий `VirtualClockHandle` (`RdtscControls { cr4_tsd: false }`, `tsc_hz = 1_000_000_000`). Гость повторяет `pit_calibrate_tsc` (gate high через `(inb(0x61) & ~2) | 1`, управляющее слово `0xb0`, счёт 11931, RDTSC, цикл по `0x61` до бита 5, `pitcnt >= 1000`, `tscmax <= 10 * tscmin`, частота = `delta / 10` кГц) и MSB-форму `quick_pit_calibrate` (счёт `0xffff`, на каждом проходе опрос `0x61`, латч канала 2 словом `0x80`, чтение LSB и MSB порта `0x42`; стадия 1 пока MSB = `0xff`, стадия 2 пока `0xfe`; порог: каждая стадия более 5 проходов; частота = `delta * 1193182 / 256000`). Результат — запись в COM1 (`A`, статус, 5 x u64; `B`, статус, 4 x u64; `Z`), затем HLT. HLT без пробуждения владелец воспринимает как завершение (`halted`): `run` диагностической сессии HLT вне halt wait не исполняет.

**Инъекции владельца.** `NoPortTick` — владелец отбрасывает очередь тика (поведение до S4-29d): время стоит, OUT2 не поднимается, гость крутится до исчерпания бюджета (`budget_exhausted`, код runner 5). `CoarsePortTick { ns_per_read }` — отрицательный контроль: 100 мкс на чтение; сработали собственные пороги гостя (`pitcnt < loopmin`, статус A = 1; MSB-стадии ≤ 5 проходов, статус B = 3). Вариант образа `--omit-gate` (гость не поднимает gate 2) отвергается моделью при загрузке канала 2.

**Ограничения.** Расхождение с буквой решения п. 5: чтение `0x42` не тикает, поэтому MSB-форма использует `0x61` как драйвер времени, а `0x42` читается через latch (в Linux `pit_verify_msb` читает `0x42` дважды без латча). Порог `count > 5` и два этапа сохранены. PIT IRQ (INTIN2) — S4-29f; cmdline — S4-29g. Отчёт: [S4-29d](../reports/STAGE_4_S4-29D_2026-11-09.md).

## Вход IOAPIC INTIN2, только edge (S4-29e)

Реализация: `crates/od-machine/src/apic_pair.rs` (`Route::intin2_high`, `PairState::set_line2`, `PairState::set_low2`), `crates/od-machine/src/io_apic.rs` (`IoApicHandle::set_intin2`, `valid_low`). Зеркалит S4-26a для входа 2; INTIN4 не менялся (кроме одного симметричного отказа ниже). К плате, manifest, handoff, MADT/FADT и firmware не подключено; PIT к входу не привязан (S4-29f).

**Вход.** `set_intin2(level)` хранит уровень линии в `intin2_high`. Рост 0 -> 1 при `entry 2` без маски запрашивает вектор из `pins[2]` у Local APIC как fixed edge (тем же путём `PairState::edge`, что и INTIN4). Под маской фронт теряется, unmask событие не доставляет; high-to-high — не фронт. Если Local APIC отказал (выключен), фронт не потребляется и `intin2_high` не меняется. Вызывающий сериализует операции обоих контроллеров.

**Entry 2.** Low dword: вектор `0x10..=0xFE`, Fixed, physical destination, active-high, edge. Масочный entry может хранить любой вектор и (как остальные пины без INTIN4) active-low/LEVEL, но unmask с LEVEL, active-low, вектором `0x00..=0x0F`/`0xFF`, non-Fixed delivery или logical destination — `UnsupportedConfiguration` без изменения снимка. High dword (селектор `0x15`): по политике S4-26c принимается только 0 (physical APIC ID 0), читается 0; отдельного хранения нет. Remote IRR на entry 2 не существует: бит 14 читается 0, `route.remote_irr` принадлежит только INTIN4.

**Конфликт с INTIN4 (политика S4-26b).** Level-маршрут INTIN4 резервирует свой вектор против edge-запросов, в том числе под маской. Поэтому: (1) unmask entry 2 с вектором, равным вектору level INTIN4, — отказ; (2) запись INTIN4 с LEVEL (с маской или без) на вектор unmasked entry 2 — отказ (новый симметричный отказ в `PairState::set_low`, затрагивает только случай unmasked entry 2). Edge INTIN4 на тот же вектор допустим, запросы сливаются в IRR. Совпадение вектора с Timer LVT и PIC не проверяется.

**Reset.** `IoApicHandle::reset` и `reset_pair`: entry 2 masked, линия низкая, IRR не затрагивается IOAPIC-сбросом, Remote IRR 0.

**Не входит.** Level и Remote IRR для INTIN2, другие пины (кроме INTIN2 и INTIN4), PIT и IRQ0 (S4-29f), насос событий, EOI-регистр `0x40`, подключение к плате, MADT/FADT. Отчёт: [S4-29e](../reports/STAGE_4_S4-29E_2026-11-09.md).

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
