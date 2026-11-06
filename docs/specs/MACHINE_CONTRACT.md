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
- `MemoryRegionKind` различает `Ram`, `Rom`, `Reserved` и `Mmio`. Сейчас backing
  создаётся только для RAM; ROM/reserved/MMIO классифицируются и отклоняют доступ
  до появления соответствующих владельцев.
- Port I/O, E820, boot reservations, aliases и remapping пока не реализованы и
  не должны моделироваться перекрывающимися RAM regions.

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

2 MiB/1 GiB leaves, LA57, PCID и другие paging modes не реализованы и не
объявлены optional до фиксации manifest S4-00. Их добавление потребует отдельной
проверки alignment, reserved bits, permissions и A/D effects, а также отрицательных
тестов; текущий admission boundary не заменяет эту работу.

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
fault metadata and full CPU state. This bounded slice does not implement LGDT,
descriptor caching, IDT gate checks, exception delivery, cross-page descriptor
reads, or production paging ownership. Independent review is pending.

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
