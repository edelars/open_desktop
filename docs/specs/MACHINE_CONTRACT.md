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
a nonzero divisor; IER must remain zero, FIFO is disabled, and IIR reports no
interrupt.

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
Long64 и страницы 4 KiB. Present PS в PDPT или PD не означает fallback или
частичную поддержку huge page: `probe_long64_4k` и `access_long64_4k_ram`
возвращают `UnsupportedPageSize` с уровнем, адресом entry и исходным значением.
Pure probe не меняет RAM; tracked path сохраняет уже committed A updates
предков, но не меняет отвергнутую entry, не спускается к нижним таблицам и не
трогает operand.

Проверка `Present` выполняется раньше размера страницы: nonpresent entry с PS
остаётся `PageFault`. Для present PDPT/PD PS admission refusal предшествует
проверке остальной large-page формы в этом диагностическом контракте; это не
заявление об архитектурном приоритете реального x86 fault. PML4 bit 7 остаётся
reserved-bit outcome, а PT bit 7 не выбирает huge page и не мешает 4 KiB leaf.

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
прежний путь; PS в PDPT остаётся `UnsupportedPageSize`.

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

The xAPIC version register (`0x030`) is unsupported; no Version value is
advertised because this slice has no implemented LVT entries. LVT, APIC-base
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
automatic scheduling, or MOV-SS/NMI arbitration. Parent S4-27 remains open. See
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

**Не реализовано.** MCFG и PNP0C02/`_CRS` (S4-31); генерация E820 и loader (S4-37); host bridge и любые функции; BAR→MMIO mapping и PCI MMIO aperture; MSI/MSI-X; ACPI PM и reset register (S7-14a); дефолтный handoff не менялся.
