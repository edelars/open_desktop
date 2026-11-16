# Контракт нескольких процессоров (SMP)

**Статус:** документный контракт S6-03 принят после независимого архитектурного ревью `gpt-6.1-sol/high`. Реализации многопроцессорной платы, нескольких LAPIC и планировщика нет. Контракт описывает первый SMP target; отмеченные ниже границы остаются открытыми до зависимых задач.

Связанные документы: [план этапа 6](../plans/STAGE_6.md#s6-03-описать-контракт-нескольких-процессоров), [отчёт этапа 6](../reports/STAGE_6_REPORT.md), [CPU contract](CPU_CONTRACT.md), [контракт машины](MACHINE_CONTRACT.md), [профиль виртуальной платы](VIRTUAL_BOARD.md), [модель памяти](MEMORY_MODEL.md), [вход в прошивку](FIRMWARE_BOOT.md).

## 1. Область и источники

Нормативная архитектура x86 в этом документе: Intel® 64 and IA-32 Architectures Software Developer’s Manual, Volume 3A, System Programming Guide, Part 1, order 253668-093US, September 2026. Используются §§7.7.1–7.7.2 (NMI), 11.4.1–11.4.2 (BSP/AP и INIT/SIPI), 12.1.1, Table 12-1 (состояние после INIT), 13.4.7.3–13.4.7.4 (состояние LAPIC после INIT и INIT-deassert), 13.6.2 и описание ICR в §13.6 (адресаты и отправка IPI), §§13.8–13.9 (LAPIC/IOAPIC delivery и EOI), §13.5.2 (APIC vector range), §7.15/Event 8 “Double Fault Exception” (shutdown wake/restart, pp. 7-35–7-37). [Официальный каталог Intel SDM](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html) указывает version 093; [Volume 3A PDF](https://cdrdv2.intel.com/v1/dl/getContent/671190) содержит идентификатор 253668-093US и дату September 2026.

Linux evidence ограничен Ubuntu source package 7.0.0-38.38 из [S6-02 inventory](../reports/data/stage6/ap-start-inventory.json). Файлы не являются аутентифицированной upstream source-to-binary цепочкой. Принятый S6-02 static target path — физический xAPIC и native INIT/SIPI при заданном OpenDesktop-профиле; это не runtime-наблюдение. Текущая исполняемая Stage4A плата остаётся однопроцессорной и APIC-clear.

ACPI — Specification 6.6: §5.2.12.2 (Processor Local APIC structures), §5.2.12.12 (x2APIC structures), §8.4 (processor device object). В первой версии включается xAPIC type 0; type 9/x2APIC и type 0x10/Multiprocessor Wakeup не публикуются.

## 2. Набор vCPU и идентичность

- `N` задаётся конфигурацией платы при её создании и неизменен до выключения. Первое SMP target допускает `N = 2`; `N = 4` становится поддержанным только после gate измерений S6-38. `N = 1` сохраняет текущую конфигурацию. Значения больше 4 не входят в версию 1.
- Индекс vCPU 0 — BSP. Для всех vCPU `APIC_ID = index`; идентификаторы стабильны и равны `0…N−1`.
- Для `N > 1` CPUID.1 EBX сообщает `N` в поле logical processor count и ID текущего vCPU в поле initial APIC ID. CPUID topology leaves `0x0b`, `0x1f`, `0x80000026` в reference CPU profile отсутствуют.
- **Ограничение topology:** guest vendor `OpenDesktop ` не распознаётся vendor switch Linux 7.0.0-38.38 `topology_common.c`. Linux читает из CPUID.1 EBX начальный APIC ID и count shift, но не запускает известный legacy или extended topology parser; topology domains остаются defaults для одного logical CPU с нулевыми shifts. Поэтому CPUID/MADT/`cpu_count` должны согласовываться, но package/core classification Linux для N=2/4 этим source path не доказана. Решение по topology domains принадлежит S6-18.
- MADT содержит ровно `N` enabled Processor Local APIC entries, по одной на UID/APIC ID `0…N−1`. DSDT содержит matching processor device objects (`_HID=ACPI0007`, `_UID` равен MADT UID). Публикация MP Wakeup type `0x10` противоречит выбранному INIT/SIPI handoff.
- Платы и firmware передают одно и то же `N`. Firmware handoff block v1.0 сейчас нормативно требует `cpu_count=1`; переход к `N>1` — отдельное согласованное изменение S7-02, здесь он отмечен как зависимость, но не изменяется.

## 3. Состояния vCPU и переходы

Каждый vCPU владеет своим архитектурным состоянием CPU и своим LAPIC. Межпроцессорное сообщение записывается как pending event в LAPIC адресата. Изменить состояние чужого CPU можно только при обработке события самим адресатом на своей границе исполнения.

| Текущее состояние | Событие адресата | Результат |
|---|---|---|
| Power-on, индекс 0 | machine reset/handoff | `Running` с BSP ролью и состоянием handoff из `FIRMWARE_BOOT.md`. |
| Power-on, индекс > 0 | machine reset/handoff | `WaitForSipi`; AP не исполняет код до SIPI. |
| `Running` или `Halted`, AP | INIT assert | На своей границе применяет INIT state из §4, очищает локальное interrupt state согласно LAPIC INIT reset и входит в `WaitForSipi`. Pending STARTUP, принятый до INIT, удаляется; последующий SIPI остаётся отдельным событием. |
| `WaitForSipi`, AP | SIPI | Первый SIPI переводит AP в `Running` с real-mode стартом по адресу `vector × 4096`; `vector` — младшие 8 бит поля Vector (`ICR[7:0]`). |
| AP уже покинул `WaitForSipi` | SIPI | Игнорируется: повторный startup не сбрасывает выполняющийся AP. |
| `Running` | HLT retirement | `Halted`; архитектурное состояние CPU сохраняется. |
| `Halted` | принимаемый Fixed interrupt | `Running`, когда interrupt допускается IF/priority правилами LAPIC/CPU; запрос доставляется как interrupt vector, не как прямой вызов handler. |
| `Halted` | NMI, NMI delivery не заблокирован | `Running`; NMI принимается независимо от IF и доставляется вектором 2. Если NMI blocking активен, запрос остаётся единственным pending NMI, CPU остаётся `Halted` и вложенной доставки нет. |
| Исполняемое состояние | Fault при обработке #DF, вызывающий shutdown | `Shutdown`, исполнение прекращается. Состояние запоминает `hardware_reset_only=true`, если shutdown произошёл во время NMI handler или SMM; иначе `false`. |
| `Shutdown` | machine reset | BSP получает BSP reset/handoff state; остальные vCPU — `WaitForSipi`. |
| `Shutdown` (`hardware_reset_only=false`) | INIT или NMI | INIT применяет соответствующий BSP/AP reset path; NMI возобновляет обработку через handler vector 2. Любые pending события обрабатываются после wake event. |
| `Shutdown` (`hardware_reset_only=true`) | INIT/NMI | Остаётся `Shutdown`; ни INIT, ни NMI не рестартуют CPU. Intel SDM требует hardware reset, если shutdown произошёл во время NMI handler или SMM. |
| Любое состояние | остановка VM host | vCPU приостанавливается на указанной безопасной границе; архитектурное состояние и LAPIC pending events сохраняются. Это pause, а не гостевой INIT/reset. |

Scheduler не выбирает `WaitForSipi` и `Shutdown` для исполнения. `Halted` может быть пропущен до появления принимаемого события. Pending INIT/SIPI не меняет удалённое состояние немедленно и не теряется при pause.

## 4. INIT, SIPI и reset state

Для выбранной Intel 64 family 06H архитектурной таблицы INIT из SDM §12.1.1/Table 12-1 CPU после INIT находится в real-address mode с `EFLAGS=2`, `EIP=0xFFF0`, `CR2/CR3/CR4=0`, `EFER=0`; по footnote 2 Table 12-1 `CR0 = (old_CR0 & 0x60000000) | 0x10`, то есть сохраняются только прежние `CD/NW`, устанавливается `ET=1`, а все прочие биты `CR0` очищаются. `CS` получает selector `0xF000`, base `0xFFFF0000`, limit `0xFFFF`; `SS/DS/ES/FS/GS` — selector/base 0 и limit `0xFFFF`. `EAX`, `EBX`, `ECX`, `ESI`, `EDI`, `EBP`, `ESP` и `R8–R15` очищаются; `EDX` сохраняет processor signature. `GDTR/IDTR` получают base 0 и limit `0xFFFF`; `LDTR/TR` получают selector/base 0, limit `0xFFFF`, reset attributes; debug registers получают значения Table 12-1.

INIT не равен power-on reset. Не очищаются x87/MMX, XMM/MXCSR, XCR0/XSS, `IA32_TSC`, `IA32_TSC_AUX`, `IA32_TSC_ADJUST`, `IA32_TSC_DEADLINE`, `IA32_SYSENTER_CS/ESP/EIP`, `IA32_STAR/LSTAR`, MTRRs, performance counters и machine-check banks. FS/GS base и EFER — исключения к общему сохранению MSRs: Table 12-1 задаёт для них reset values. Internal caches сохраняются; TLB и BTB инвалидируются (§12.1.1). LAPIC возвращается к INIT-reset state, сохраняя APIC ID и arbitration ID (§13.4.7.3).

- INIT для AP выполняется как выше и оставляет его в `WaitForSipi`; SIPI задаёт низкий real-mode startup vector.
- INIT для BSP тоже выполняет архитектурное INIT state, но BSP flag/роль сохраняется. BSP не становится AP и не ждёт SIPI; после reset boundary он начинает bootstrap execution с reset-vector state. INIT protocol не выполняется заново после выбора BSP (SDM §11.4.2).
- INIT-deassert не является повторным INIT и не запускает код. Linux 7.0.0-38.38 посылает targeted INIT-deassert между assert и SIPI. В S6 xAPIC его запись принимается как совместимое no-op для CPU state и LAPIC pending state; delivery завершается, BSP/AP state не меняется. Это ограниченное поведение target, а не утверждение, что INIT-deassert поддерживается каждым поколением Intel CPU.
- SIPI запускает только AP в `WaitForSipi`. Для вектора `v` стартовый physical address равен `v << 12`; выбранный Linux target использует адрес из low-memory trampoline. SIPI для BSP или уже работающего AP завершается без повторной инициализации.

## 5. ICR: закрытый набор кодировок версии 1

Для xAPIC запись aligned Dword в `ICR_HIGH` сохраняет только destination byte и сама по себе не отправляет IPI. Aligned Dword запись в `ICR_LOW` начинает отправку; read-only Delivery Status bit игнорируется при записи и читается как Idle после завершения отправки. Сначала применяется таблица допустимых delivery/destination/shorthand. Затем таблица пары `(Level, Trigger)`. **Любая комбинация, не перечисленная как поддерживаемая ниже, возвращает явную ошибку LAPIC device и не ставит событие в очередь.** Reserved ICR bits дают device error. В shorthand destination byte и Destination Mode bit игнорируются, как требует xAPIC; это не реализует logical routing. Отправитель не ждёт синхронного исполнения адресата.

| Delivery mode | Destination mode | Shorthand | Vector | `(Level, Trigger)` | Итог |
|---|---|---|---|---|---|
| Fixed | Physical | None | `0x10–0xFF` | `(0,0)` | Доставить targeted Fixed IPI адресату `APIC_ID`; событие входит в локальный LAPIC pending/IRR. Это кодировка pinned Linux. |
| Fixed | ignored by shorthand | Self | `0x10–0xFF` | `(0,0)` или `(1,0)` | Доставить Fixed IPI отправителю. `(1,0)` сохраняет существующий S4-25c encoding; `(0,0)` — Linux source encoding. |
| Fixed | ignored by shorthand | All including self | `0x10–0xFF` | `(0,0)` | Доставить Fixed IPI всем vCPU, включая отправителя. Требуется планом S6-12. |
| Fixed | ignored by shorthand | All-but-self | `0x10–0xFF` | `(0,0)` | Доставить Fixed IPI каждому другому vCPU; используется Linux reboot stop path. |
| NMI | Physical | None | поле ignored; Linux emits 0 | `(0,0)` | Targeted NMI event. |
| NMI | ignored by shorthand | All-but-self | поле ignored; Linux emits 0 | `(0,0)` | NMI всем другим vCPU; используется Linux reboot/crash/debug stop paths. |
| INIT | Physical | None | ровно 0 | `(1,1)` assert; `(0,1)` deassert | Assert применяет INIT к адресату; deassert принимается как no-op CPU/LAPIC state. |
| Start-up (SIPI) | Physical | None | `Vector[7:0]` (`0x00–0xFF`) | `(0,0)` | Поставить SIPI vector адресату. |
| Все прочие значения: включая Lowest Priority, SMI, reserved modes, logical destination без shorthand, NMI Self/All, INIT/SIPI shorthand и неуказанные пары Level/Trigger | — | — | — | — | Явная ошибка LAPIC device; состояние всех vCPU остаётся прежним. |

Linux `__prepare_ICR()` в pinned source для Fixed/NMI и SIPI формирует `Level=0, Trigger=0`; trigger для этих delivery modes не управляет доставкой. Fixed дополнительно сохраняет уже принятый S4-25c `(Level, Trigger)=(1,0)` self-IPI encoding. Поэтому target принимает обе существующие формы, а не навязывает `Level=1` для Linux edge IPIs. Для INIT assert Linux пишет level-triggered assert; затем targeted level-triggered deassert; SIPI отправляется targeted physical с vector `start_eip >> 12`.

Fixed IPI vector доставляется как maskable APIC interrupt через CPU/LAPIC acceptance path. Набор `0x10–0xFF` допускает SDM §13.5.2; `0xFF` также покрывает существующий S4-25c contract. Все Fixed shorthand из S6-12 (`Self`, `All including self`, `All-but-self`) перечислены выше. Lowest Priority отсутствует потому, что Linux target выбирает physical-flat xAPIC и не нуждается в priority arbitration; SMI, logical destinations и дополнительные shorthand не входят в доказанный S6-02 target.

## 6. IOAPIC, MSI и локальная адресация LAPIC

Для reference `OpenDesktop` target выбран physical-flat xAPIC: APIC destinations физические, `APIC_ID` адресата берётся из CPUID/MADT-consistent набора. IOAPIC и MSI маршрутизируют только Fixed physical interrupts к APIC ID `0…N−1`; logical и Lowest Priority routing для этого профиля отклоняются. MSI composition сохраняет физический адресат и Fixed delivery. Edge/level, remote IRR, EOI и допустимая polarity наследуются из реализованных ограниченных правил IOAPIC в [контракте машины](MACHINE_CONTRACT.md#fixed-edge-ioapic-route-s4-26a) и его следующих S4 slices; S6-03 не расширяет electrical/SCI профили.

Каждый vCPU имеет отдельный LAPIC state. Владелец CPU execution context передаёт свой vCPU index в его memory adapter; доступ по LAPIC MMIO window адресуется LAPIC именно этого vCPU, без глобальной переменной «текущий APIC». Состояние на S6-11: плата создаёт N LAPIC по конфигурации (`vcpu_count`, по умолчанию 1), окно адресуется LAPIC выполняющего vCPU через явный индекс (см. [контракт машины](MACHINE_CONTRACT.md#lapic-на-каждый-vcpu-s6-11)); индекс пока не проводится от `Vcpu` на рабочих путях MMIO, Fixed IPI реализован (S6-12a, см. [контракт машины](MACHINE_CONTRACT.md#доставка-ipi-s6-12a)); NMI, INIT и SIPI не реализованы, исполнение AP не запускается (S6-13/S6-15).

## 7. NMI

NMI IPI принимает physical target и all-but-self broadcast в доказанных Linux путях. Поле vector в ICR не задаёт гостевой handler: CPU получает NMI event с архитектурным вектором 2 (§7.7.1, §7.7.3). NMI не зависит от IF. После начала NMI delivery устанавливается NMI blocking; последующие NMI не вкладываются до начала исполнения `IRET` (§7.7.2). При нескольких запросах до первой доставки они coalesce до одного события; пока блокировка активна, LAPIC сохраняет не более одного следующего ожидающего NMI, дополнительные запросы coalesce/drop. После `IRET` ожидающий NMI может быть доставлен на следующей границе инструкции. Для reference CPUID topology limits NMI-source reporting/FRED не включены.

NMI обязателен в контракте: S6-02 source inventory находит targeted NMI и NMI all-but-self для reboot timeout fallback, panic/kdump, restart stop-other-CPU и KGDB roundup; PMU hard-lockup NMI остаётся условным и не считается основанием включать PMU. AP startup сам по себе NMI не отправляет.

## 8. Время и два режима исполнения — открыто до S4-22/S4-23

S4-22 и S4-23 как родительские задачи ещё не приняты; существующие bounded slices не задают завершённую SMP time/event contract. Поэтому этот раздел намеренно **не выбирает** владельца TSC, способ продвижения времени или host wait policy.

| Режим | Уже принято для S6 | Открытые определения |
|---|---|---|
| Чередование | Один host-thread выполняет все пишущие в RAM vCPU. Граница — между завершёнными гостевыми инструкциями или на restart point частично исполненного `REP`, где RIP остаётся на этой инструкции, а регистры/память отражают уже совершённые итерации. Обычная guest instruction — один scheduler step после полного commit. Для `REP` один step — ровно одна успешно завершённая string-iteration; на её boundary память и `RCX/RSI/RDI` (и сравнивающие флаги для `CMPS/SCAS`) уже отражают эту итерацию, а RIP остаётся на начале REP, пока повтор не завершён. При начальном count=0 REP завершает одну инструкцию без итераций за один step. При остановке по repeat condition последняя фактически выполненная итерация — единственный step для завершения: RIP продвигается в том же commit, дополнительного пустого step нет. Quantum измеряется целым числом таких steps; счётчик увеличивается после каждого полного instruction commit или каждой REP iteration boundary. Квант заканчивается после заданного числа steps одного vCPU, затем выбирается следующий runnable vCPU. Переключение не происходит внутри инструкции или committed `REP` progress point; sequential order CPU steps является SC-наблюдаемым порядком режима. Одинаковые guest inputs + seed дают одинаковое исполнение (включая счётчики instructions на vCPU, UART bytes и final state) только если virtual time основано на исполненных инструкциях и нет внешнего I/O; до решения S4-22 это условие не принято. | TSC каждого vCPU/offset; связь TSC с `VirtualClock`; что продвигает время; поведение, когда все vCPU в `HLT`; deadline wakeups и host wait — открыть до S4-22/S4-23. |
| Параллельный | Предназначен для x86-TSO и не обещает детерминизма расписания; seed и quantum lengths не обещают одинаковый interleaving host threads. | Владение/масштаб виртуального времени, per-vCPU TSC и синхронизация; all-HLT и event wakeup; host wait — открыть до S4-22/S4-23. |

После принятия S4-22/S4-23 обновить этот раздел по их фактическому контракту; не выводить автоматическое продвижение времени из существующего caller-driven `VirtualClock` или event queue.

## 9. Остановка, pause и границы владения

По D1 этап 6 владеет остановкой vCPU и сохранением их состояния; S9-28 — полным barrier с I/O, epoch и публикацией. Stop request доставляется каждому vCPU как host control event. Все vCPU участвуют в stop, включая `Halted` и `WaitForSipi`; для спящих vCPU acknowledgement не требует гостевой инструкции. Каждый активный vCPU до общего конечного deadline достигает безопасной границы: завершённая инструкция либо разрешённая точка частичного прогресса `REP`. На safe point всё изменяемое guest CPU состояние находится в структуре vCPU, без скрытых live host-register значений. После подтверждения всех CPU scheduler прекращает выбор vCPU и сохраняет CPU state, LAPIC registers/IRR/ISR/TMR, pending INIT/SIPI/NMI, таймеры и очередь событий с прежними сроками. Позже прибывший interrupt/IPI остаётся pending и не снимает stop acknowledgement; выход из `HLT`/`WaitForSipi` сначала проверяет stop request, и guest execution возобновляется только по явной команде resume. Виртуальное время на остановке не продвигается. Виртуальное время стоит на всём интервале stop. Timeout возвращает ошибку с перечнем vCPU, не достигших safe point; stop request снимается, припаркованные vCPU возвращаются к прежнему runnable/sleep state, pending events и deadlines сохраняются без потери/дублирования, частичный snapshot не публикуется. Полный storage/device/I/O snapshot barrier здесь не обещан и остаётся S9-28.

## 10. Не входит в первую версию и оставшиеся границы

- x2APIC и MADT type 9; logical/cluster routing; Lowest Priority; SMI; hot-add/hot-remove; topology с несколькими threads per core; больше 4 vCPU.
- Generic `OpenDesktop` Linux package/core topology — S6-18; Windows CPU-06 — трасса Stage 8.
- Runtime AP execution, NMI/INIT/SIPI (Fixed IPI реализован в S6-12a), multi-LAPIC wiring по умолчанию (плата с `vcpu_count > 1` собирается, но по умолчанию 1), двух-/четырёх-vCPU Linux boot и выбранный delay provider ещё не выполнены.
- TSC/HLT/event timing до S4-22/S4-23 остаётся открытым.
- Реализация CPU exception/interrupt delivery остаётся в границах задач этапов 4/6; таблица описывает целевой контракт, не наличие реализации.

## 11. Проверки будущей реализации

S6-10/S6-11/S6-15/S6-17/S6-18 должны проверять как минимум: AP не исполняется до SIPI; INIT/SIPI меняет только target на его границе; INIT/AP reset registers и INIT BSP behavior; повторный SIPI ignored; все допустимые ICR кодировки таблицы; каждая запрещённая комбинация возвращает ожидаемый device error без изменения CPU/LAPIC; Fixed/NMI physical IDs и all-but-self destinations; NMI vector 2, coalescing и IRET unblock; IOAPIC/MSI physical Fixed routes; CPUID/MADT/handoff `cpu_count` consistency; безопасную остановку и сохранение LAPIC pending state. Проверки времени нельзя считать закрытыми до S4-22/S4-23. Полный контракт принимает независимое архитектурное ревью до реализации.
