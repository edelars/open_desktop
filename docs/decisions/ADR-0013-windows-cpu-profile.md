# ADR-0013 — CPU-профиль Windows

**Дата:** 2026-11-20 (подготовка); 2026-11-21 (архитектурное ревью и решение владельца по идентичности CPU)

**Задача:** [S8-03](../plans/STAGE_8.md#s8-03-принять-adr-cpu-профиля-windows-и-передать-пробелы-этапу-2)

**Статус:** **принято (архитектурное ревью; идентичность CPU — вариант B подтверждён владельцем 2026-11-21).** Решения 1 и 3–6 (набор обязательных расширений, бит гипервизора, XSAVE/AVX, TSC, compatibility mode) приняты архитектурным ревью 2026-11-21 (Claude Opus / `high`; запись — [отчёт этапа 8, §9](../reports/STAGE_8_REPORT.md#9-ревью)). Идентичность CPU (решение 2): владелец проекта выбрал вариант B (vendor `GenuineIntel`, модель из списка Microsoft для Windows 11) 2026-11-21 и принял продуктовый и юридический риск; конкретная модель — Intel Celeron N5105 — выбрана архитектурным агентом по решению владельца «вариант B» (архитектурное решение, не решение владельца); значения — в решении 2. Реализации нет: биты и идентичность объявляются только задачами этапа 2 S2-51…S2-58 по правилам решения 1.

## Вопрос

Какой CPU видит Windows 11 Pro x64 ([GUEST_REQUIREMENTS.md](../specs/GUEST_REQUIREMENTS.md), ISO и build — [SUPPORT_MATRIX.md](../specs/SUPPORT_MATRIX.md)): vendor, family/model/stepping, brand string, обязательные расширения ISA, режимы и биты CPUID. Какие из них отсутствуют в текущем профиле и кто их реализует. Сейчас [CPU_CONTRACT.md](../specs/CPU_CONTRACT.md) содержит только Linux-цель `od-x86-64-v1-neutral-v0.1` и строку «Windows/Linux CPU profile — Не определён».

## Источники

Дата доступа ко всем источникам — 2026-11-20. Страницы получены HTTP-запросом (статус 200) и разобраны из HTML; PDF — `pdftotext` (poppler 26.10.0). Обозначения M1, M2, M17, M20 совпадают с [аудитом S7-30](../specs/WINDOWS_PLATFORM_REQUIREMENTS.md#источники).

| ID | Документ | Дата / редакция | Что взято |
|---|---|---|---|
| M1 | [Windows 11 requirements](https://learn.microsoft.com/en-us/windows/whats-new/windows-11-requirements) | `ms.date` 2026-07-14 | «1 gigahertz (GHz) or faster with two or more cores on a compatible 64-bit processor»; раздел «Virtual machine support»: «Two or more virtual processors», «The VM host processor must also meet Windows 11 processor requirements» и оговорка «There may be some instances where this requirement for the VM host doesn't apply»; «Client Hyper-V: requires a processor with second-level address translation (SLAT)» |
| M2 | [Minimum Hardware Requirements for Windows 11](https://download.microsoft.com/download/7/8/8/788bf5ab-0751-4928-a22c-dffdc23c27f2/Minimum%20Hardware%20Requirements%20for%20Windows%2011.pdf) (PDF) | «June 2021»; SHA-256 файла `4e29ce08154ea6be734e662985041bcc80bb8b09fa4293c819d5fca5daac160f` (совпадает с S7-30) | §3.5 Processor: «Supports 2 Processor Cores», «Compatible with the x64 or ARM64 instruction set», «Supports PAE, NX and SSE4.1», «Supports CMPXCHG16b, LAHF/SAHF, and PrefetchW», «Meets the supported processor generation list» |
| M20 | [Windows Processor Requirements](https://learn.microsoft.com/en-us/windows-hardware/design/minimum/windows-processor-requirements) | `ms.date` 2026-08-31 | Списка инструкций нет; отсылает к M2 и спискам поддерживаемых процессоров |
| NET11 | [.NET 11 breaking change: Minimum hardware requirements updated](https://learn.microsoft.com/en-us/dotnet/core/compatibility/jit/11/minimum-hardware-requirements) | `ms.date` 2026-11-10 | «the baseline is updated from x86-64-v1 to x86-64-v2 … also guaranteeing CX16, POPCNT, SSE3, SSSE3, SSE4.1, and SSE4.2. This guarantee is required by Windows 11 and by all x86/x64 CPUs officially supported on Windows 10». Это документ Microsoft, но не нормативный документ Windows; используется как косвенный источник для SSE3/SSSE3/SSE4.2/POPCNT |
| BC5D | [Bug Check 0x5D UNSUPPORTED_PROCESSOR](https://learn.microsoft.com/en-us/windows-hardware/drivers/debugger/bug-check-0x5d--unsupported-processor) | `ms.date` 2017-05-23 | Значение `0x0000005D`, параметров нет, причина: «Windows requires a higher-grade processor than the one you are using». Условия, при которых ядро его выдаёт, не описаны |
| WOW | [WOW64 Implementation Details](https://learn.microsoft.com/en-us/windows/win32/winprog64/wow64-implementation-details) | `ms.date` 2018-05-31 | «(x64 only) Wow64Cpu.dll provides support for running x86 programs on x64»; программный эмулятор x86 указан только для Itanium и ARM64; «the WOW64 CPU transitions back to native 64-bit mode» — 32-битный код исполняется процессором, то есть в IA-32e compatibility mode |
| TLFS | [Hypervisor Feature and Interface Discovery](https://learn.microsoft.com/en-us/virtualization/hyper-v-on-windows/tlfs/feature-discovery) | `ms.date` 2020-10-15 | CPUID.01h:ECX[31] — «hypervisor present bit»; при нём гарантированы листы `0x40000000` и `0x40000001`; «software only base compatibility decisions on the interface signature reported through leaf 0x40000001» |
| HVREQ | [System Requirements for Hyper-V on Windows and Windows Server](https://learn.microsoft.com/en-us/windows-server/virtualization/hyper-v/host-hardware-requirements) | `ms.date` 2025-06-26 | Hyper-V требует SLAT, «VM Monitor Mode extensions», Intel VT или AMD-V, DEP; проверка — `systeminfo.exe` |
| SDM | Intel 64 and IA-32 Architectures Software Developer's Manual, Vol. 2A, CPUID | — | **Номера битов CPUID в этом ADR не сверены с текстом SDM** (PDF в этой задаче не получен; AMD APM Vol. 3 не загрузился). Исполнитель каждой задачи этапа 2 сверяет бит до кода |

Найдены, но **не использованы как источник**: обсуждения Microsoft Q&A и Tech Community о требовании POPCNT/SSE4.2 в 24H2 и о «LabConfig» — только как подсказка, где искать.

## Требования Windows к CPU

| № | Требование | Источник | Статус |
|---|---|---|---|
| W-01 | Набор инструкций x64 (long mode) | M2 §3.5 | по документации Microsoft |
| W-02 | PAE | M2 §3.5 | по документации Microsoft |
| W-03 | NX | M2 §3.5 | по документации Microsoft |
| W-04 | SSE4.1 | M2 §3.5 | по документации Microsoft |
| W-05 | CMPXCHG16B | M2 §3.5 | по документации Microsoft |
| W-06 | LAHF/SAHF (в 64-битном режиме) | M2 §3.5 | по документации Microsoft; «в 64-битном режиме» — толкование: в 32-битных режимах LAHF/SAHF есть всегда, CPUID-бит относится к 64-битному режиму (SDM, не сверено) |
| W-07 | PrefetchW | M2 §3.5 | по документации Microsoft |
| W-08 | ≥ 2 ядер (для VM — ≥ 2 vCPU) | M2 §3.5, M1 | по документации Microsoft; владелец — этап 6 (S6-51) |
| W-09 | ≥ 1 GHz | M1, M2 §3.5 | по документации Microsoft; как Setup определяет частоту без листа частоты CPUID — **источник не найден**, наблюдать на S8-24 |
| W-10 | Процессор из списка поддерживаемых поколений | M2 §3.5, M1, M20 | по документации Microsoft; как это проверяет Setup при чистой установке — **источник не найден** (решение 2) |
| W-11 | SSE4.2 и POPCNT | NET11 (косвенно) | по документации Microsoft (косвенно); нормативного документа Windows не найдено |
| W-12 | SSE3 и SSSE3 | NET11 (косвенно) | то же. В плане S8-03 этой группы нет; добавлена этим ADR: x86-64-v2 включает её, а NET11 называет весь v2-набор обязательным для Windows 11 |
| W-13 | XSAVE/AVX | — | **источник не найден**: M1, M2, M20 их не называют. Наблюдать на S8-24: остановка машины на инструкции `VEX`/`XSAVE*` с модулем и RVA (снимок S8-10) |
| W-14 | Инвариантный TSC | — | **источник не найден**; связан с TM-01 (источник времени ОС) — S8-33 |
| W-15 | IA-32e compatibility mode в CPL3 | WOW | по документации Microsoft (что x86-код исполняется процессором на x64); нужен WoW64 (S8-60) и 32-битным установщикам |
| W-16 | VMX/SVM, SLAT | HVREQ, M1 | по документации Microsoft; нужны только Hyper-V/VBS/WSL2 — вне профиля ([WINDOWS_GUEST_PROFILE.md](../specs/WINDOWS_GUEST_PROFILE.md#7-функции-виртуализации-и-изоляции)) |
| W-17 | Бит гипервизора CPUID.1 ECX[31] и листы `0x40000000`+ | TLFS | по документации Microsoft (семантика); требованием не является |
| W-18 | Прочие функции, которые ядро может проверять: CLFLUSH (CPUID.1 EDX[19]), MTRR (EDX[12]), MCE/MCA (EDX[7], EDX[14]), RDTSCP, лист 7, листы топологии | — | **источник не найден**; текущий профиль их не объявляет. Наблюдать на S8-24 (снимок S8-10: неподдержанный лист CPUID, MSR или инструкция). Лист топологии для 2 vCPU — S6-18. **Наблюдение S7-32 (2026-11-26):** ядро требует DE, MCE, MTRR, MCA и CLFSH (маска `0x0789F3FD`, иначе bugcheck 0x5D); объявлены [дополнением «S7-32, срез I»](#дополнение-s7-32-срез-i-2026-11-26-признаки-обязательные-для-ядра-windows) |

**Проверка совместимости процессора в Setup (п. 2 S8-03).** M2 требует соответствия списку поколений, M1 — чтобы процессор VM-host соответствовал требованиям, с оговоркой о случаях, когда это не применяется. Что именно сверяет Setup WinPE при чистой установке (vendor, family/model/stepping, CPUID-биты, частоту) — **источник не найден**. Наблюдать на S8-24 по `setupact.log`/`setuperr.log` (S8-12). Ключи `LabConfig` (`BypassCPUCheck` и др.) — обход по словарю плана этапа 8: прогон с ними — отладочный, gate по нему не принимается.

## Сравнение с текущим профилем

Текущий профиль — [CPU_CONTRACT.md, «Ubuntu Generic amd64»](../specs/CPU_CONTRACT.md#ubuntu-generic-amd64-od-x86-64-v1-neutral-v01): `CPUID(0)` EAX = 1 (максимальный базовый лист 1), vendor `OpenDesktop `; `CPUID(1)` EAX = `0x600`, ECX = 0, EDX = `0x0781A379`; `CPUID(0x80000001)` ECX = 0, EDX = `0x20100800` (SYSCALL, NX, LM); листы `0x80000002..0x80000007` нулевые; `CR4.OSXSAVE` должен оставаться 0. «Реализация» ниже — поиск имени формы по `crates/od-isa/src` на коммите `3693319` (`rg -i`, совпадений нет) и записи CPU_CONTRACT; это не доказательство отсутствия частичной реализации под другим именем — исполнитель задачи этапа 2 проверяет при старте.

| Требование | Бит CPUID (SDM, не сверено) | Объявлено в профиле Linux | Реализация | Есть/нет | Владелец реализации |
|---|---|---|---|---|---|
| W-01 x64 | `80000001` EDX[29] LM | да | Long64 в границах принятых срезов этапа 2: наборы CPL0/CPL3 (`crates/od-isa/tests/cpl0_profile_set.rs`, `cpl3_profile_set.rs`, `cpl3_syscall.rs`) | частично: полной приёмки этапа 2 нет | этап 2: [S2-24](../plans/STAGE_2.md#s2-24-protected-и-compatibility-mode), S2-25 (EFER), S2-41, S2-44 |
| W-02 PAE | `1` EDX[6] | да | 4-уровневая трансляция — `crates/od-machine/src/page_walk.rs` (`walk_long64_4k`; тест `nonpresent_ps_is_a_page_fault_and_pml4_pt_bit_seven_keep_their_roles`); `CR4.PAE` — S2-25 | частично | этап 4 (MMU: S4-07, S4-08 приняты; 2 MiB — S4-10, не принят); CR4 — S2-25, не принята; объявление — [S2-58](../plans/STAGE_2.md#s2-58-объявить-pae-nx-и-apic-в-windows-профиле-adr-0013), **ждёт принятия этих задач** |
| W-03 NX | `80000001` EDX[20] | да | бит XD в MMU этапа 4: `page_walk.rs` (`tracked_nx_matrix_checks_fetch_faults_reserved_bits_and_data_access`), `firmware_board.rs` (`instruction_fetch_from_rom_is_a_page_fault_by_nx`); `EFER.NXE` — S2-25 | частично | этап 4 (MMU: S4-08 принята; XD в 2 MiB — S4-10, не принят); EFER — S2-25, не принята; объявление — [S2-58](../plans/STAGE_2.md#s2-58-объявить-pae-nx-и-apic-в-windows-профиле-adr-0013), **ждёт принятия этих задач** |
| W-04 SSE4.1 | `1` ECX[19] | нет | не найдено | нет | этап 2, [S2-55](../plans/STAGE_2.md#s2-55-sse41-adr-0013) |
| W-05 CMPXCHG16B | `1` ECX[13] CX16 | нет | не найдено | нет | этап 2, [S2-51](../plans/STAGE_2.md#s2-51-cmpxchg16b-adr-0013) |
| W-06 LAHF/SAHF | `80000001` ECX[0] | нет | не найдено | нет | этап 2, [S2-52](../plans/STAGE_2.md#s2-52-lahfsahf-в-64-битном-режиме-adr-0013) |
| W-07 PrefetchW | `80000001` ECX[8] | нет | не найдено | нет | этап 2, [S2-53](../plans/STAGE_2.md#s2-53-prefetchw-adr-0013) |
| W-11 SSE4.2, POPCNT | `1` ECX[20], ECX[23] | нет | не найдено | нет | этап 2, [S2-56](../plans/STAGE_2.md#s2-56-sse42-и-popcnt-adr-0013) |
| W-12 SSE3, SSSE3 | `1` ECX[0], ECX[9] | нет | не найдено | нет | этап 2, [S2-54](../plans/STAGE_2.md#s2-54-sse3-и-ssse3-adr-0013) |
| W-13 XSAVE/AVX | `1` ECX[26–28] | нет; `CR4.OSXSAVE` = 0 | — | нет (не объявляем, решение 4) | — |
| W-14 инвариантный TSC | `80000007` EDX[8] | нет (лист `0x80000007` нулевой) | virtual clock этапа 4 | нет (решение 5) | S8-33 → этап 4/6 |
| W-15 compatibility mode CPL3 | — (режим, `CS.L = 0` при `EFER.LMA = 1`) | не входит в профиль; есть только переходный режим trampoline S6-14 | нет | нет | этап 2, [S2-24](../plans/STAGE_2.md#s2-24-protected-и-compatibility-mode) |
| x87 FPU (x86-64-v1) | `1` EDX[0] | да | `FNINIT` и простой `FXSAVE` набора CPL0 (S4-38L9d, `cpuid.rs:9-11`); остальные x87-формы — срезы S2-28…S2-32 | частично | этап 2, S2-28…S2-32 |
| CMOV (x86-64-v1) | `1` EDX[15] | да | CMOVcc — локальные срезы S2-16a/b/c | частично: приёмки S2-16 нет | этап 2, S2-16 |
| CX8 / CMPXCHG8B (x86-64-v1) | `1` EDX[8] | да | не найдено; CPU_CONTRACT: CMPXCHG8B/16B не добавлялись | нет | этап 2, S2-21 (формы CMPXCHG8B в тексте S2-21 не названы явно — уточнить при приёмке) |
| FXSR (x86-64-v1) | `1` EDX[24] | да | узкие формы `FXSAVE`/`FXRSTOR` (S2-39 срезы, набор CPL0) | частично | этап 2, S2-39 |
| MMX (x86-64-v1) | `1` EDX[23] | да | не найдено (`EMMS` в `od-isa/src` нет) | нет | этап 2, S2-33 |
| SSE, SSE2 (x86-64-v1) | `1` EDX[25], EDX[26] | да | узкие локальные срезы S2-34/S2-35/S2-37 (MOVUPS, XORPS/XORPD, часть PADD*/PCMP*, часть SSE FP) | частично | этап 2, S2-34…S2-39 |
| SYSCALL/SYSRET (x86-64-v1) | `80000001` EDX[11] | да | узкие формы: `crates/od-isa/tests/cpl3_syscall.rs` | частично | этап 2, S2-27 |
| TSC, MSR (RDTSC, RDMSR/WRMSR) | `1` EDX[4], EDX[5] | да | узкие срезы S2-26 (`RDTSC`, `RDMSR`/`WRMSR` с профилем MSR) | частично | этап 2, S2-25/S2-26 |
| PSE, PGE, PAT | `1` EDX[3], EDX[13], EDX[16] | да | в текстах задач S2-00…S2-44 слов PSE/PGE/PAT нет | нет | **задачи этапа 2 не созданы** — профиль Windows эти биты не объявляет, пока не появится задача-владелец |
| APIC | `1` EDX[9] | да | Local APIC одного CPU — S4-25 (принят); бит APIC в CPUID-профиле — S4-38L8d (реализовано, ждёт ревью); S2-27 запрещает APIC в `od-isa` | частично | этап 4 (S4-25, S4-38L8d); объявление — [S2-58](../plans/STAGE_2.md#s2-58-объявить-pae-nx-и-apic-в-windows-профиле-adr-0013), **ждёт принятия S4-38L8d** |
| W-17 бит гипервизора | `1` ECX[31] | нет | — | не объявляем (решение 3) | — |
| CR4 | — | профили фиксируют `CR4` и узкие политики (`Cr4Profile::BASE`; широкие политики CR0/CR4, включая PGE, вынесены за S2-45) | общая запись CR4 с проверкой по объявленным функциям отсутствует | нет | этап 2, [S2-25](../plans/STAGE_2.md#s2-25-cr-msr-и-переходы-режима) (CR, MSR и переходы режима) |
| Идентичность: vendor, family/model/stepping, brand string | лист 0, `1` EAX, `0x80000002..4` | `OpenDesktop `, `0x600`, brand string пуст | таблица CPUID — [S2-26](../plans/STAGE_2.md#s2-26-cpuid-системные-инструкции-и-время) | — | решение 2, таблица — [S2-57](../plans/STAGE_2.md#s2-57-таблица-cpuid-windows-профиля-adr-0013) |

**Какой CPUID видит путь firmware board сегодня** (сверено по коду на коммите `3693319` плюс рабочее дерево). Профиль задаётся на каждом vCPU (`VcpuCpuid::new`). По умолчанию это `CpuidProfile::DiagnosticV1` с нулевыми масками ECX и EDX (`crates/od-machine/src/vcpu.rs:251`; `crates/od-isa/src/cpuid.rs:3-5`; доккомментарий `FirmwareBoard::cpuid_profile`, `crates/od-machine/src/firmware_board.rs:1674-1681`: «`DiagnosticV1` until `set_cpuid_profile` changes it»). `FirmwareBoard::set_cpuid_profile` (`firmware_board.rs:1689`) вне тестов вызывается только в Linux-конструкторе `into_linux_board` (`firmware_board.rs:4523`) со значением `LINUX_AP_CPUID_PROFILE = OdX8664V1Neutral` (`vcpu.rs:42`) — для процессоров приложений; CPUID загрузочного процессора в long mode на `linux-direct` берётся из `members.cpuid_profile` (`crates/od-machine/src/linux_direct.rs:1255`, по умолчанию `DiagnosticV1`). Путь Windows/UEFI на `FirmwareBoard` профиль не выбирает: гость видит `DiagnosticV1` (нулевые маски — не x86-64-v1 и не загрузочный профиль). `OdX8664V1Neutral` не годится как основа Windows-профиля: `cpuid.rs:14-16` прямо говорит, что он объявляет биты, поведение которых движок ещё не реализует (фраза там же «no existing path uses it» устарела: Linux-конструктор платы выбирает его для процессоров приложений). Выбор профиля для платы Windows — параметр платы в [S2-57](../plans/STAGE_2.md#s2-57-таблица-cpuid-windows-профиля-adr-0013).

## Решение 1. Набор расширений Windows-профиля

*Статус: принято (архитектурное ревью 2026-11-21).*

Windows-профиль — новая версия `od-x86-64-v2-windows-v0.1` в CPU_CONTRACT, отдельно от Linux-цели `od-x86-64-v1-neutral-v0.1` (её evidence и решения S2-00 не переписываются). Обязательный набор: x86-64-v2 (CX16, LAHF/SAHF, POPCNT, SSE3, SSSE3, SSE4.1, SSE4.2), PrefetchW, PAE, NX, IA-32e compatibility mode в CPL3. Не объявляются: XSAVE/AVX и выше, VMX/SVM, бит гипервизора, инвариантный TSC до решения 5.

Каждый бит CPUID включается в Windows-профиль только после принятия задачи этапа 2, которая реализует соответствующие формы в интерпретаторе и даёт честный отказ JIT с переходом в интерпретатор (правило этапа 2, §4.3). Объявлять возможность до реализации запрещено (S8-03 «Не делать»). Правило относится и к битам, которые уже перечислены в Linux-цели (EDX `0x0781A379`: FPU, PSE, TSC, MSR, PAE, CX8, APIC, PGE, CMOV, PAT, MMX, FXSR, SSE, SSE2; `80000001` EDX: SYSCALL, NX, LM): Windows-профиль **не наследует** их таблицу целиком, а включает каждый бит только после приёмки задачи-владельца (таблица «бит → задача» в [S2-57](../plans/STAGE_2.md#s2-57-таблица-cpuid-windows-профиля-adr-0013)); где задачи нет, профиль бит не объявляет. PAE и NX (обязательны по M2 §3.5) и APIC объявляет отдельная задача [S2-58](../plans/STAGE_2.md#s2-58-объявить-pae-nx-и-apic-в-windows-профиле-adr-0013); она ждёт S2-25, S4-10 и S4-38L8d. До её принятия S2-57 блокирован, а Windows-профиль эти биты не объявляет. SSE4.2/POPCNT и SSE3/SSSE3 включены сразу, не дожидаясь наблюдения на S8-24: объявлять реализованное безопасно, а цикл «установка → остановка → реализация» занимает дни.

**Альтернативы.** (а) Только список M2 без SSE3/SSSE3/SSE4.2/POPCNT — меньше работы, но NET11 называет весь v2-набор обязательным для Windows 11; риск остановки на первой же инструкции SSE4.2/POPCNT в ядре 24H2+ и дней на цикл обнаружения. (б) Сразу x86-64-v3 (AVX/AVX2/BMI/FMA, XSAVE) — источника требования нет, объём реализации (VEX, YMM, XSAVE-состояние) кратно больше; против правила этапа 2 «не добавлять AVX/AVX2/BMI на будущее» (S2-40). (в) Расширить Linux-профиль — смешало бы две цели с разным evidence.

### Дополнение S7-32 (2026-11-25): весь EDX целевой таблицы

*Статус: принято архитектурным решением исполнителя S7-32 (Opus high) по поручению владельца проекта от 2026-11-25 «архитектуру решай сам»; независимое ревью — вместе с кодом среза S7-32.*

Windows-профиль объявляет в `CPUID.01H:EDX` всю целевую таблицу `0x0781A379` (FPU, PSE, TSC, MSR, PAE, CX8, APIC, PGE, CMOV, PAT, MMX, FXSR, SSE, SSE2), в `CPUID.80000001H:EDX` — `0x20100800` (SYSCALL, NX, LM). До этого дня объявлялись только PAE, CX8, APIC, CMOV (`0x8340`), а EDX `80000001` был нулевым; остальные биты ждали отдельных решений владельца (S2-57, срез C).

**Основание.** (1) Без FPU/FXSR/SSE член FP набора CPL0 отказывает профилю: первая остановка прогона Windows-профиля — `Rejected(Cpl0Set(FpFeaturesNotDeclared))` на 72 тыс. инструкций прошивки (запись S7-32 в отчёте этапа 7). (2) PAE, NX, SSE2 и CMPXCHG16B обязательны для Windows 11 (M2 §3.5), без LM/SYSCALL ядро x64 не работает. (3) Основные формы этих битов исполняются интерпретатором (FP-член, ядра SSE/MMX, `RDTSC`, `RDMSR`/`WRMSR` моделируемых MSR, страницы 2 МиБ); Linux-прогоны единого цикла объявляют из них только FPU, TSC, FXSR, SSE, SSE2 (`X8664V1Stage4AExt2`), поэтому пути под PSE, PGE, PAT, MMX, CMOV в госте под объявленным битом до S7-32 не проверялись. NX отложили до приёмки S4-10, а S4-10 принят владельцем 2026-11-21.

**Известные пробелы остаются остановками прогона, не тихими результатами.** x87-формы вне ограниченного профиля (S2-30…S2-32), формы `FXSAVE`/`FXRSTOR` вне реализованных, MSR вне моделируемого списка (с S7-32, срез F, единый цикл Windows-гостя подключает файл MSR Windows `with_msr_windows` с `IA32_PAT`, `IA32_MISC_ENABLE`, `IA32_TSC`, `IA32_PLATFORM_ID`, `IA32_BIOS_SIGN_ID` и опцией `with_msr_windows_unknown_as_stop`: обращение к MSR вне списка — остановка, отчёт называет индекс и направление; до среза F плата файл не подключала, и первая остановка была на `RDMSR 0x10`), compatibility mode (S2-24) — каждый первый такой случай Windows видно как остановку с модулем и RVA (S8-10), а не как неверный результат. Не объявляются по-прежнему: VME, DE, MCE, SEP, MTRR, MCA, PSE-36, CLFSH (до исполнения `CLFLUSH`), HTT, Page1GB, RDTSCP, инвариантный TSC (решение 5), XSAVE/AVX (решение 4), бит гипервизора (решение 3).

**Альтернатива** — объявлять биты по одному после приёмки задач этапа 2 (прежнее правило) — оставляет S7-32 заблокированным первой же инструкцией FP-члена; отвергнута. Linux-цель `od-x86-64-v1-neutral-v0.1` не меняется.

### Дополнение S7-32, срез I (2026-11-26): признаки, обязательные для ядра Windows

*Статус: принято архитектурным решением исполнителя S7-32 (Opus high) по поручению владельца проекта от 2026-11-25 «архитектуру решай сам»; независимое ревью — вместе с кодом среза I. Закрывает открытый вопрос 3 (W-18) в части CLFLUSH, MTRR и MCE/MCA.*

**Наблюдение.** Прогон официального носителя (`od-runner boot-firmware --cpu-loop unified --guest windows`) доходит до `ntoskrnl`, и ядро останавливается через `KeBugCheckEx(0x5D UNSUPPORTED_PROCESSOR, 0x0781A379, 0x20100800, 0x101, 0)`: параметры — `CPUID.01H:EDX`, `CPUID.80000001H:EDX` и `CPUID.80000001H:ECX` профиля. Проверка ядра (функция по RVA `0xb6805c`, сравнение по `0x140b683af`) требует `(CPUID.01H:EDX & 0x0789F3FD) == 0x0789F3FD`, затем `80000001H:EDX` SYSCALL [11] и NX [20], `01H:ECX` CX16 [13], `80000001H:ECX` LAHF [0]. Профиль не объявлял пять битов маски: DE [2], MCE [7], MTRR [12], MCA [14], CLFSH [19]. Объявленный набор остаётся подмножеством функций модели: у Celeron N5105 (решение 2) все пять есть — это базовые признаки процессоров Intel 64 (DE, MCE, MTRR, MCA — с P6, CLFSH — с Pentium 4); листом CPUID самой модели не сверено. Этим срезом они объявляются, `CPUID.01H:EDX = 0x0789F3FD`.

**Что стоит за каждым битом** (значения — `CPU_CONTRACT.md`, «Windows MSR file (S2-26h)», абзацы «S7-32 slice I»):

| Бит | Поведение модели | Решение |
|---|---|---|
| DE [2] | `CR4.DE` — хранимый бит политики набора CPL0; при `CR4.DE = 1` `MOV` в/из DR4/DR5 — `#UD`, при 0 — псевдонимы DR6/DR7 с их правилами (SDM Vol. 3B «Debug Registers DR4 and DR5»); I/O-точки останова невозможны: набор не разрешает включать точки останова в DR7 | до среза I запись DR4/DR5 была отказом, теперь архитектурное поведение |
| MCE [7] | `CR4.MCE` — хранимый бит; `#MC` (вектор 18) модель не порождает никогда: аппаратных ошибок нет | — |
| MTRR [12] | MSR MTRR (SDM Vol. 3A «Memory Type Range Registers (MTRRs)»): `IA32_MTRRCAP = 0x508` (VCNT = 8, FIX, WC; SMRR нет), `IA32_MTRR_DEF_TYPE`, 8 пар PHYSBASE/PHYSMASK, 11 фиксированных; резервные биты (MAXPHYADDR 36) и недопустимые типы — `#GP(0)`; после сброса MTRR выключены (все нули); типы памяти на доступы не влияют — кешей в модели нет | VCNT = 8 — выбор модели (значение физического N5105 не сверено): хватает на окно устройств платы с запасом для ОС; FIX = 1 (фиксированные MTRR есть), WC = 1 |
| MCA [14] | `IA32_MCG_CAP = 0x4` (4 банка; MCG_CTL_P, EXT_P, CMCI_P, TES_P, SER_P, EMC_P, ELOG_P, LMCE_P = 0), `IA32_MCG_STATUS` (RIPV, EIPV, MCIP), банки `0x400 + 4i`: CTL — любое значение, STATUS/ADDR/MISC читаются 0, запись 0 принимается, ненулевое — `#GP(0)` (SDM Vol. 3B «Machine-Check MSRs»: очищаются записью нулей, единицы — `#GP`); `IA32_MCG_CAP` только для чтения | 4 банка — выбор модели (небольшое ненулевое число; значение N5105 не сверено); без MCG_CTL_P, CMCI, TES, SER, LMCE: соответствующие MSR не существуют |
| CLFSH [19] | `CLFLUSH m8` исполняется (пакет W1: проверка доступа как у чтения байта, кешей нет), с этого среза и на CPL3 (SDM: инструкция не привилегированная); без CLFSH в члене CPUID набора — `#UD` (SDM Vol. 2A «CLFLUSH»); чтение байта доходит и до окна MMIO ([ISSUE-695](../KNOWN_ISSUES.md#issue-695)); `CPUID.01H:EBX[15:8] = 8` — строка 64 байта | `EBX = 0x800` для одного vCPU: до среза I — `0x100` Linux-цели (поле размера 1, без смысла при CLFSH = 0); индекс бренда 0 (есть brand string), EBX[23:16] (действительно только при HTT, не объявлен) и EBX[31:24] (APIC ID) — по vCPU через `query_for_vcpu` (S6-18a), как раньше |

**CR0.CD/NW.** HAL при программировании MTRR входит в режим без заполнения кеша (`CR0.CD = 1`), поэтому политика набора CPL0 хранит CD и NW; `NW = 1` при `CD = 0` — `#GP(0)` (SDM Vol. 2B «MOV—Move to/from Control Registers»). Кешей нет: CD, NW и `WBINVD` ничего наблюдаемого не меняют.

**Политика CR: только Windows-профиль или везде.** Ходок страниц платы (`page_walk.rs`, общий для Long64, PAE и 32-битного режима) принимает CR4.DE/MCE и CR0.CD/NW **всегда**, как биты снимка: в трансляции они не участвуют, а ходок общий для всех профилей; прежняя попытка расширить только `od-isa` откатилась именно из-за отказа ходока. Решает CPU: `MOV CR4`, ставящий DE или MCE, — `#GP(0)`, если член CPUID набора их не объявляет (так же, как `WRMSR EFER.NXE` без NX, S2-25), и такой CR4 отвергается на входе набора (`Cr4OutsidePolicy`, в том числе после `ProtectedCpu::into_long_mode`); их объявляет только Windows-профиль, поэтому с Linux-профилями (их CPUID не менялся) набор CPL0 с CR4.DE/MCE не исполняется — до среза I такая запись была остановкой, теперь архитектурный `#GP(0)`; ядро Linux без CPUID.MCE CR4.MCE не ставит. Модель защищённого режима (`ProtectedCpu`) CR4 с CPUID не сверяет ([ISSUE-694](../KNOWN_ISSUES.md#issue-694)). CD/NW от CPUID не зависят и допускаются везде. Политика, которую набор проверяет на каждом шаге, — только маска; «NW без CD» и сверка DE/MCE с CPUID — в формах записи и на входе. Изменения для Linux-профилей (архитектурные ответы): CR0.CD/NW принимаются (было: остановка), DR4/DR5 — псевдонимы DR6/DR7 (было: остановка записи), запись CR4.DE/MCE — `#GP(0)` (было: остановка), `CLFLUSH` — `#UD`, потому что Linux-профили CLFSH не объявляют (было: проверка чтением, W1).

**Прошивка программирует MTRR.** По SDM MTRR после сброса выключены (вся память UC), включает их прошивка платформы. Прошивка исполняет `CPUID(1)` в ранней инициализации на всех циклах (тем же `push rbx; cpuid; pop rbx`, что и для SMBIOS Processor ID); если `CPUID.01H:EDX.MTRR = 1`, `firmware/uefi` пишет `IA32_MTRR_DEF_TYPE = E | WB` (`0x806`, фиксированные выключены: ниже 1 MiB на плате только RAM) и UC для окна устройств ниже 4 GiB — от нижнего не-RAM региона handoff (MMIO, ROM, NVRAM, framebuffer, reserved) до 4 GiB блоками степени двойки (на плате Windows одна пара: `0xF8000000`, 128 MiB); RAM внутри окна, устройство выше 4 GiB или нехватка пар — останов `0x36`. Последовательность — SDM «MTRR Considerations in MP Systems» для одного процессора (CD = 1, `WBINVD`, сброс TLB, E = 0, пары, E = 1, `WBINVD`, сброс TLB, CD = 0). Linux-профили и firmware-цикл MTRR не объявляют — прошивка ничего не пишет.

**Неизвестные MSR** в прогоне Windows остаются явной остановкой с индексом (срез F). MSR, которых по объявленным возможностям нет (`IA32_MCG_CTL`, банки с 4-го, `IA32_MCi_CTL2`, пары MTRR с 8-й), в модель не включены: на объявленном CPU это `#GP(0)`, в прогоне Windows — остановка, чтобы такое обращение было видно.

**Не объявляются по-прежнему:** VME, SEP, PSE-36, HTT, Page1GB, RDTSCP, инвариантный TSC (решение 5), XSAVE/AVX (решение 4), бит гипервизора (решение 3).

**Альтернатива** — «обойти» проверку ядра другим путём (подмена параметров, отключение проверки) — запрещена правилами S7-32; объявить биты без модели MSR — дало бы ложный профиль (ядро и HAL читают `IA32_MTRRCAP`/`IA32_MCG_CAP` по этим битам).

## Решение 2. Идентичность CPU

*Статус: принято (архитектурное ревью 2026-11-21); вариант B выбран владельцем проекта 2026-11-21, продуктовый и юридический риск принят владельцем. Модель — Intel Celeron N5105: архитектурное решение, выбрана архитектурным агентом по решению владельца «вариант B» (ниже).*

| Вариант | Значения | Риск совместимости | Продуктовый и юридический риск |
|---|---|---|---|
| A. Собственный vendor | `OpenDesktop `, family 6 / model 0 / stepping 0, brand string «Open Desktop Virtual CPU» | Высокий и **не измеренный**: проверка Setup на список поддерживаемых поколений (W-10), вероятно, не пройдёт для неизвестного vendor — источника нет, это гипотеза. Связь неизвестного vendor с bugcheck 0x5D (BC5D) — **источник не найден**. Ядро не пойдёт по vendor-специфичным путям Intel/AMD — меньше неизвестных MSR | Нет: машина не выдаёт себя за чужой процессор |
| B. Vendor `GenuineIntel` + family/model/stepping поколения из списка Microsoft | Конкретная модель — по списку Microsoft поддерживаемых процессоров Intel и с набором функций не шире объявленного (без AVX — кандидаты среди Pentium/Celeron поддерживаемых поколений; **сверить по Intel ARK**); brand string — по решению владельца | Ниже для проверки Setup (гипотеза). Ядро и загрузчик пойдут по Intel-путям: загрузка драйвера микрокода по vendor, чтение `IA32_BIOS_SIGN_ID`, попытка обновления микрокода через MSR, Intel-специфичные MSR (частота, `MISC_ENABLE`, функции управления) — каждый нужно моделировать честно или отказывать по контракту; **источник не найден**, наблюдать на S8-24 | Машина представляется процессором Intel: товарный знак, возможное введение в заблуждение пользователя, условия лицензии Windows. Решает только владелец |
| C. Vendor `AuthenticAMD` + модель из списка AMD | Аналогично B | Аналогично B (AMD-пути) | Аналогично B (AMD) |
| D. Собственный vendor + бит гипервизора с собственной сигнатурой | A + CPUID.1 ECX[31] = 1, лист `0x40000000` с сигнатурой Open Desktop, `0x40000001` ≠ `Hv#1` | По TLFS решения о совместимости принимаются по сигнатуре интерфейса; ослабляет ли это проверку Setup — **источник не найден**. Enlightenments Hyper-V не реализуются | Нет |

**Рекомендация архитектурного агента (2026-11-20, до решения владельца; заменена решением ниже).** Задачи этапа 2 (решение 1) не зависят от выбора и начинаются сразу. Рабочее значение для отладочных прогонов до решения владельца — **A** (текущий vendor и честный brand string): так первые прогоны S7-32/S8-24 покажут, на чём именно останавливается Windows с собственным vendor (`setuperr.log`, bugcheck, снимок S8-10), а не будут гаданием. Для приёмочных прогонов G2 рекомендуется **заранее** получить решение владельца, допускает ли он **B** (или C) как запасной вариант, если A не проходит проверку Setup без обхода: без такого решения G2 при отказе A blocked, а обход `LabConfig` gate не закрывает. Выбор конкретной модели для B — после решения владельца, по списку Microsoft и Intel ARK, с записью значений в CPU_CONTRACT и [S2-57](../plans/STAGE_2.md#s2-57-таблица-cpuid-windows-профиля-adr-0013).

### Решение владельца (2026-11-21)

Владелец проекта выбрал вариант **B**: vendor `GenuineIntel`, family/model/stepping и brand string модели из списка Microsoft поддерживаемых процессоров Windows 11. Продуктовый и юридический риск варианта B (машина представляется процессором Intel: товарный знак, возможное введение пользователя в заблуждение, условия лицензии Windows; столбец «Продуктовый и юридический риск» таблицы выше) **принят владельцем**. Дата решения — 2026-11-21. Вариант A для Windows-профиля не используется; прогон с идентичностью A допустим только как отладочный и gate не закрывает. Варианты C и D не выбраны.

### Выбор модели для варианта B

Владелец подтвердил вариант B; конкретная модель — архитектурное решение (выбрана архитектурным агентом по решению владельца «вариант B»), не решение владельца.

Дата доступа ко всем источникам этого раздела — 2026-11-21. Страницы Microsoft и файл Intel на GitHub получены HTTP-запросом (статус 200) и разобраны из HTML/Markdown; страницы спецификаций Intel на `intel.com` на прямой запрос отвечают 403 и прочитаны через инструмент WebFetch (статус 200).

| ID | Документ | Дата / редакция | Что взято |
|---|---|---|---|
| MSI25 | [Windows 11, version 25H2 supported Intel processors](https://learn.microsoft.com/en-us/windows-hardware/design/minimum/supported/windows-11-25h2-supported-intel-processors) | `ms.date` 2025-10-16 | Список по **сериям**, не по моделям (столбцы Manufacturer, Product Name, Series); среди серий без AVX-моделей (классификация автора ADR по Intel ARK; список Microsoft AVX не указывает) — «Celeron N4000 Series», «Celeron N5000 Series», «Celeron J4000 Series», «Pentium Silver J5000 Series», «Pentium Silver N6000 Series». Вводная: «OEMs may use the following CPUs for new Windows 11 devices»; «Subsequently released and future generations of processors which meet the same principles will be considered as supported, even if not explicitly listed»; «Updates to the processor lists will occur at each subsequent general availability of Windows» |
| MSI24 | [Windows 11, version 24H2 supported Intel processors](https://learn.microsoft.com/en-us/windows-hardware/design/minimum/supported/windows-11-24h2-supported-intel-processors) | `ms.date` 2025-02-27 | Те же пять серий |
| — | Список для 26H2 (целевой build 26300, [SUPPORT_MATRIX.md](../specs/SUPPORT_MATRIX.md)) | — | **Источник не найден**: адрес по шаблону `…/windows-11-26h2-supported-intel-processors` вернул HTTP 404, поиск по learn.microsoft.com страницы не нашёл. Используется последний найденный список — MSI25 |
| UCODE | [Intel Linux Processor Microcode Data Files, `releasenote.md`](https://github.com/intel/Intel-Linux-Processor-Microcode-Data-Files/blob/main/releasenote.md) | файл `releasenote.md` на выпуске `microcode-20261110`; строка JSL 06-9c-00/01 — раздел `microcode-20240312` (проверено выборкой WebFetch 2026-11-21; та же строка с другими версиями микрокода есть в разделе `microcode-20230214`) | Сигнатуры (family-model-stepping/маска платформы) с названием моделей: «JSL \| A0/A1 \| 06-9c-00/01 \| … \| Pentium N6000/N6005, Celeron N4500/N4505/N5100/N5105»; «GLK-R \| R0 \| 06-7a-08/01 \| … \| Pentium J5040/N5030, Celeron J4125/J4025/N4020/N4120». Для настольных Celeron/Pentium (06-9e-0b, 06-a5-03) файл называет только «Core Gen8»/«Core Gen10» |
| ARK5105 | [Intel Celeron Processor N5105 — Product Specifications](https://www.intel.com/content/www/us/en/products/sku/212328/intel-celeron-processor-n5105-4m-cache-up-to-2-90-ghz/specifications.html) | без даты редакции | «Products formerly Jasper Lake»; 4 ядра, 4 потока; Base Frequency 2.00 GHz, Burst Frequency 2.90 GHz; Instruction Set Extensions «Intel® SSE4.2»; Intel VT-x «Yes»; Intel AES New Instructions «Yes»; Launch Q1'21 |
| ARK4120 | [Intel Celeron Processor N4120 — Product Specifications](https://www.intel.com/content/www/us/en/products/sku/197309/intel-celeron-processor-n4120-4m-cache-up-to-2-60-ghz/specifications.html) | без даты редакции | «Products formerly Gemini Lake Refresh»; 4 ядра, 4 потока; Base 1.10 GHz, Burst 2.60 GHz; Instruction Set Extensions «Intel® SSE4.2»; VT-x «Yes»; AES-NI «Yes»; Launch Q4'19 |

**Что проверяет Setup.** Для всех кандидатов одинаково: какие поля (vendor, family/model/stepping, brand string, биты CPUID, частота) Setup WinPE сверяет при чистой установке и совпадает ли эта проверка со списком MSI25, адресованным OEM, — **источник не найден** (W-10). Наблюдать на S8-24 по `setupact.log`/`setuperr.log` (S8-12); обход `LabConfig` gate не закрывает.

| Кандидат | Серия в списке Microsoft | Сигнатура (UCODE) → `CPUID(1).EAX` | AVX (ARK) | Биты CPUID модели, противоречащие объявленным |
|---|---|---|---|---|
| **Intel Celeron N5105** (Jasper Lake) — **выбран** | «Celeron N5000 Series» (MSI25, MSI24) — **сопоставление автора ADR**: списки Microsoft не перечисляют модели N5105, отнесение модели к серии сделано автором | 06-9C-00 → `0x000906C0` | нет: из расширений названо только SSE4.2 | У модели есть VT-x и AES-NI (ARK5105), профиль их не объявляет (VMX — решение 1 и W-16; AES-NI в профиле нет). XSAVE, лист 7, MTRR/MCE/CLFLUSH и прочее у модели первоисточником Intel в этой задаче не сверены; профиль их не объявляет (решения 1, 4; W-18). Обратного противоречия (объявленный бит, которого у модели нет) по ARK не найдено, но ARK перечисляет только SSE4.2: наличие у модели CX16, LAHF/SAHF, PrefetchW, POPCNT, SSE3/SSSE3/SSE4.1 первоисточником не сверено — сверяет исполнитель S2-57 по Intel SDM / документации модели. 4 ядра модели против 2 vCPU профиля (S8-05) — листы топологии по S6-18 |
| Intel Celeron N4120 (Gemini Lake Refresh) | «Celeron N4000 Series» (MSI25, MSI24) | 06-7A-08 → `0x000706A8` | нет: только SSE4.2 | То же, что у N5105 (VT-x и AES-NI у модели есть, профиль не объявляет) |
| Настольные Celeron G4900 / G5900 (Coffee Lake / Comet Lake) | «Celeron G4000 Series», «Celeron G5000 Series» | UCODE не называет эти модели (только «Core Gen8»/«Core Gen10») | ARK в этой задаче не проверялся | Отклонён: сигнатура конкретной модели первоисточником не установлена |
| Intel Processor N100/N200, Atom X7000, Core N300, Celeron/Pentium 7000 | есть | — | ARK в этой задаче не проверялся | Не рассматривались: отсутствие AVX у этих моделей первоисточником не подтверждено; выбор сделан среди моделей, для которых ARK называет только SSE4.2 |

**Выбор: Intel Celeron N5105** (архитектурное решение, не решение владельца). Основания: (1) серия «Celeron N5000 Series» есть в MSI25 и MSI24 (отнесение N5105 к серии — сопоставление автора ADR, Microsoft модели не перечисляет); (2) по ARK5105 из расширений у модели только SSE4.2, AVX нет — объявляемый набор (x86-64-v2 без AVX/XSAVE и без VMX, решения 1 и 4) не требует функций шире модели в части AVX/XSAVE/VMX; (3) сигнатура взята из первоисточника Intel с названием модели (UCODE); (4) базовая частота 2.00 GHz против 1.10 GHz у N4120 — запас над W-09 (≥ 1 GHz), если Setup берёт частоту из brand string (**гипотеза**, источник не найден); (5) более позднее поколение (Q1'21 против Q4'19). VT-x и AES-NI модели профиль не объявляет: объявленный набор — подмножество функций модели. То, что программа обязана определять функции по битам CPUID, а не по модели, — правило Intel SDM, **не сверено** (SDM в задаче не получен).

**Значения идентичности Windows-профиля** (вносятся в CPU_CONTRACT и таблицу [S2-57](../plans/STAGE_2.md#s2-57-таблица-cpuid-windows-профиля-adr-0013) при реализации S2-57; этим ADR не объявляются):

| Поле | Значение | Источник |
|---|---|---|
| Vendor, `CPUID(0)` EBX/EDX/ECX | `GenuineIntel` (`Genu`, `ineI`, `ntel`) | решение владельца; порядок регистров — SDM, не сверено |
| Family / model / stepping | 6 / `0x9C` / 0 (extended model 9, model `0xC`, extended family 0, type 0) | UCODE (06-9c-00) |
| `CPUID(1).EAX` | `0x000906C0` | вычислено из сигнатуры UCODE по раскладке SDM (не сверено) |
| Brand string, `CPUID(0x80000002..0x80000004)` | `Intel(R) Celeron(R) N5105 @ 2.00GHz` | состав (имя модели, базовая частота) — ARK5105; **точная строка физического экземпляра — источник не найден**: исполнитель S2-57 сверяет её с экземпляром или документом Intel до объявления, при расхождении берётся строка экземпляра |
| `CPUID(1).EBX`, максимальный базовый и расширенный лист, листы топологии, `0x80000008` | не определяются этим решением | S2-57, S6-18 |
| Биты функций | только по решению 1 (каждый после приёмки задачи-владельца) | решение 1 |

**Последствия варианта B для других компонентов** (не решаются этим ADR):

- Intel-пути ядра и загрузчика (таблица вариантов выше): загрузка обновления микрокода по vendor, MSR платформы и микрокода (`IA32_PLATFORM_ID`, `IA32_BIOS_SIGN_ID`, `IA32_BIOS_UPDT_TRIG`), частоты и `IA32_MISC_ENABLE` — перечень и номера MSR — **гипотеза, не сверено**; каждый MSR моделируется честно или отказывает по контракту MSR (S2-25, S2-26). Наблюдать на S8-24 (снимок S8-10).
- SMBIOS Type 4 ([ACPI_SMBIOS_TABLES.md](../specs/ACPI_SMBIOS_TABLES.md)) сейчас берёт Processor ID из `CPUID(1)` профиля DiagnosticV1, а Max/Current Speed — из `virtual_cpu_mhz` handoff v1.9 ([FIRMWARE_BOOT.md](../specs/FIRMWARE_BOOT.md)), значение 1000. Для платы Windows Processor ID должен совпасть с `CPUID(1)` выбранной модели, а 1000 MHz расходится с brand string «@ 2.00GHz». Владелец — этап 7 (SMBIOS, handoff) вместе с S2-57; открытый вопрос 6.
- Список для 26H2 не найден: при появлении списка 26H2 (или при выходе следующей версии) исполнитель S2-57 или S8-24 проверяет, что серия «Celeron N5000 Series» в нём осталась; если нет — новое решение о модели (вариант B сохраняется).

## Решение 3. Бит гипервизора

*Статус: принято (архитектурное ревью 2026-11-21).*

CPUID.1 ECX[31] = 0, листы `0x40000000` и выше возвращают нули, как и любой неперечисленный лист профиля. Мы не гипервизор в смысле TLFS и не реализуем ни одного его интерфейса; enlightenments Hyper-V не реализуются. Объявление бита по TLFS обязывает ответить на листы `0x40000000`/`0x40000001`, а изменения поведения Windows при неизвестной сигнатуре не документированы — **источник не найден**. Пересмотр — если на S8-24 найдётся документированное Microsoft поведение, которое требует бита (например, допуски таймаутов), или по варианту D решения 2.

Следствие для пользователя: `systeminfo` покажет требования Hyper-V как невыполненные (нет VMX/SVM и SLAT, HVREQ), VBS/HVCI/Credential Guard/WSL2/Sandbox недоступны — это записано в [WINDOWS_GUEST_PROFILE.md](../specs/WINDOWS_GUEST_PROFILE.md#7-функции-виртуализации-и-изоляции).

## Решение 4. XSAVE/AVX

*Статус: принято (архитектурное ревью 2026-11-21).*

Не объявляются; `CR4.OSXSAVE` остаётся 0, состояние FP/SIMD сохраняется через FXSAVE/FXRSTOR. Источника требования XSAVE/AVX для Windows 11 нет (CPU-04). Если S8-24 остановится на `XSAVE*`/`XGETBV`/VEX-инструкции при CPUID без этих битов — это поведение гостя, противоречащее объявленному профилю, и разбирается по S8-13 с модулем и RVA; новое решение принимается только с источником или наблюдением.

## Решение 5. TSC

*Статус: принято (архитектурное ревью 2026-11-21).*

Инвариантный TSC (`CPUID.80000007` EDX[8]) не объявляется, пока S8-33 и владелец времени машины (этап 4, при нескольких vCPU — этап 6) не подтвердят, что TSC всех vCPU идёт с постоянной частотой и согласованно от virtual clock. Без бита Windows вправе выбрать другой источник времени (TM-01, источник не найден). Объявление бита без такой гарантии дало бы Windows ложное основание использовать TSC как источник QPC.

## Решение 6. Compatibility mode в CPL3

*Статус: принято (архитектурное ревью 2026-11-21).*

Реализуется в [S2-24](../plans/STAGE_2.md#s2-24-protected-и-compatibility-mode) (не новая задача). ADR-0013 добавляет к S2-24 критерий для WoW64: дальний переход и возврат между 64-битным и 32-битным сегментом кода CPL3 (`CS.L = 0`, `CS.D = 1`), исполнение 32-битного кода с 32-битной адресацией в compatibility mode, системные вызовы WoW64 через переход обратно в 64-битный режим (WOW), исключения и прерывания из compatibility mode с переходом в 64-битный обработчик. Точный набор переходов и форм Wow64Cpu — **источник не найден**, наблюдать на S8-60. S8-60 blocked до S2-24.

## Передача пробелов этапу 2

| Группа | Задача этапа 2 | Включает бит CPUID Windows-профиля | Ждёт её |
|---|---|---|---|
| CMPXCHG16B | [S2-51](../plans/STAGE_2.md#s2-51-cmpxchg16b-adr-0013) | CPUID.1 ECX.CX16 | S8-24 |
| LAHF/SAHF в 64-битном режиме | [S2-52](../plans/STAGE_2.md#s2-52-lahfsahf-в-64-битном-режиме-adr-0013) | `80000001` ECX.LAHF-SAHF | S8-24 |
| PrefetchW | [S2-53](../plans/STAGE_2.md#s2-53-prefetchw-adr-0013) | `80000001` ECX.PRFCHW | S8-24 |
| SSE3, SSSE3 | [S2-54](../plans/STAGE_2.md#s2-54-sse3-и-ssse3-adr-0013) | CPUID.1 ECX.SSE3, ECX.SSSE3 | S8-24 |
| SSE4.1 | [S2-55](../plans/STAGE_2.md#s2-55-sse41-adr-0013) | CPUID.1 ECX.SSE4_1 | S8-24 |
| SSE4.2, POPCNT | [S2-56](../plans/STAGE_2.md#s2-56-sse42-и-popcnt-adr-0013) | CPUID.1 ECX.SSE4_2, ECX.POPCNT | S8-24 |
| Таблица CPUID Windows-профиля и идентичность | [S2-57](../plans/STAGE_2.md#s2-57-таблица-cpuid-windows-профиля-adr-0013) | все биты выше и биты Linux-цели — каждый только после приёмки задачи-владельца (таблица «бит → задача» в S2-57); vendor/brand по решению 2 | S8-24 |
| PAE, NX, APIC | [S2-58](../plans/STAGE_2.md#s2-58-объявить-pae-nx-и-apic-в-windows-профиле-adr-0013) (ждёт S2-25, S4-10, S4-38L8d) | CPUID.1 EDX.PAE, EDX.APIC; `80000001` EDX.NX | S2-57, через неё S8-24 |
| Compatibility mode CPL3 | [S2-24](../plans/STAGE_2.md#s2-24-protected-и-compatibility-mode) (критерий решения 6) | — | S8-60 |

Общая запись CR4 с проверкой по объявленным функциям (PGE, OSFXSR, OSXMMEXCPT и др.) остаётся за существующей задачей S2-25; новая задача не создаётся.

## Предложение этапу 7

Ядро NT исполняется уже в WinPE, поэтому аудит S7-30 (CPU-01…CPU-04, открытый вопрос 6) относит CPU-профиль к предпосылкам S7-32. Предлагается владельцу этапа 7 добавить S2-51…S2-58 в blocked-список [S7-32](../plans/STAGE_7.md#s7-32-загрузить-установочный-образ-windows-через-собственную-uefi-до-экрана-setup). Ответ записывается в отчёт этапа 8; до ответа S7-32 и S8-24 считают эти задачи предпосылками S8-24 (план этапа 8, §2).

## Последствия

- S8-24 ждёт S2-51…S2-58, включая часть S2-57 об идентичности: владелец выбрал вариант B (2026-11-21), S2-57 объявляет vendor `GenuineIntel`, сигнатуру и brand string Intel Celeron N5105 из решения 2.
- Linux-цель не меняется; её tests и evidence остаются привязаны к v1.
- Каждая новая форма даёт работу этапу 3 (честный отказ JIT; ускорение — отдельно).
- Физическая проверка на Mac mini (oracle) возможна для форм, которые исполняет его CPU; что не исполняется — остаётся без hardware coverage по правилу S2-40, п. 3.
- Если Setup отклонит и выбранную модель варианта B без обхода, G2 этапа 8 остаётся blocked с записанной причиной (`setuperr.log`, S8-12); обход `LabConfig` не становится приёмкой, выбор другой модели — новое решение по этому ADR.

## Открытые вопросы

1. ~~Решение владельца по идентичности CPU (решение 2), с датой.~~ Закрыт 2026-11-21: вариант B, Intel Celeron N5105 (решение 2).
2. Номера битов CPUID — сверка с Intel SDM Vol. 2A исполнителями S2-51…S2-58.
3. Требует ли Windows CLFLUSH, MTRR, MCE/MCA, RDTSCP, лист 7 (W-18) — наблюдение на S8-24. Частично закрыт 2026-11-26 ([дополнение «S7-32, срез I»](#дополнение-s7-32-срез-i-2026-11-26-признаки-обязательные-для-ядра-windows)): ядро требует DE, MCE, MTRR, MCA и CLFSH (bugcheck 0x5D без них), они объявлены; RDTSCP и лист 7 — открыты.
4. Как Setup определяет частоту CPU (W-09) при отсутствии листа частоты — наблюдение на S8-24; при отказе — решение об объявлении листа `0x16` (Intel) или brand string с частотой — отдельно.
5. Статус CPU-03 в аудите S7-30 («источник не найден») можно уточнить ссылкой на NET11 — решение владельца аудита; эта задача ставит только отметку «ADR-0013».
6. SMBIOS Type 4 для платы Windows: Processor ID из `CPUID(1)` выбранной модели и `virtual_cpu_mhz` (сейчас 1000) против brand string «@ 2.00GHz» — решение владельца этапа 7 вместе с S2-57 (решение 2, «Последствия варианта B»).
7. Точная brand string физического экземпляра Celeron N5105 и наличие у модели CX16, LAHF/SAHF, PrefetchW, POPCNT, SSE3/SSSE3/SSE4.1 — сверка исполнителем S2-57 по первоисточнику Intel до объявления.
8. Список поддерживаемых процессоров Microsoft для 26H2 не найден (решение 2); проверить при его появлении.
