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
включая отказ region или cross-region access, не вызывает callback. Этот hook
является только локальным уведомлением о committed bytes; он ещё не подключён к
JIT code-dependency invalidation и не доказывает S4-15—S4-16.

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
