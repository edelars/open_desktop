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

## CPU-state to diagnostic translation snapshot

`Long64ProbeConfig::from_cpu_state` строит immutable diagnostic snapshot из
`LongModeState::cr0` и `LongModeState::cr4`; отсутствующий CR0 отвергается
раньше отсутствующего CR4 через `MissingCpuRegister`. CR3 и EFER остаются
явными аргументами, поскольку текущий `LongModeState` ими не владеет.

После извлечения raw values метод вызывает существующий `Long64ProbeConfig::new`.
Он не подставляет defaults, не маскирует unsupported bits, не меняет CPU/RAM/A/D
и не создаёт guest exception. Ошибки остаются configuration-admission results;
snapshot не меняется, если caller позже изменит CPU state или построит новый
snapshot с другим CR3/EFER.

Этот bridge позволяет диагностическому probe использовать CPU-owned CR0/CR4
значения и явный CR3/EFER context, включая WP/NXE effects уже существующего
validator. Он не реализует MOV CR3, WRMSR/EFER, privilege checks, mode transition
history, startup sequencing, TLB invalidation или CPU-connected paging.

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
и не доставляет IRQ. Dispatch, periodic timers, reset/generation, host wait и
межмашинное владение остаются за будущими срезами S4-23.
