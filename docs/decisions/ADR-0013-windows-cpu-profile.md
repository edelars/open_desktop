# ADR-0013 — CPU-профиль Windows

**Дата:** 2026-11-20

**Задача:** [S8-03](../plans/STAGE_8.md#s8-03-принять-adr-cpu-профиля-windows-и-передать-пробелы-этапу-2)

**Статус:** **подготовлено; ждёт архитектурного ревью; идентичность CPU — решение владельца.** Решения 1 и 3–6 (набор обязательных расширений, бит гипервизора, XSAVE/AVX, TSC, compatibility mode) — предложения архитектурного агента (Claude Opus / `high`) и **ждут архитектурного ревью**; принятыми не считаются. Идентичность CPU (решение 2) имеет продуктовое и юридическое последствие и окончательной не является до записанного решения владельца. Чекбокс S8-03 не отмечен; задачи этапа 2, созданные этим ADR, помечены «добавлено ADR-0013, ждёт ревью».

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
| W-18 | Прочие функции, которые ядро может проверять: CLFLUSH (CPUID.1 EDX[19]), MTRR (EDX[12]), MCE/MCA (EDX[7], EDX[14]), RDTSCP, лист 7, листы топологии | — | **источник не найден**; текущий профиль их не объявляет. Наблюдать на S8-24 (снимок S8-10: неподдержанный лист CPUID, MSR или инструкция). Лист топологии для 2 vCPU — S6-18 |

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

*Статус: ждёт архитектурного ревью.*

Windows-профиль — новая версия `od-x86-64-v2-windows-v0.1` в CPU_CONTRACT, отдельно от Linux-цели `od-x86-64-v1-neutral-v0.1` (её evidence и решения S2-00 не переписываются). Обязательный набор: x86-64-v2 (CX16, LAHF/SAHF, POPCNT, SSE3, SSSE3, SSE4.1, SSE4.2), PrefetchW, PAE, NX, IA-32e compatibility mode в CPL3. Не объявляются: XSAVE/AVX и выше, VMX/SVM, бит гипервизора, инвариантный TSC до решения 5.

Каждый бит CPUID включается в Windows-профиль только после принятия задачи этапа 2, которая реализует соответствующие формы в интерпретаторе и даёт честный отказ JIT с переходом в интерпретатор (правило этапа 2, §4.3). Объявлять возможность до реализации запрещено (S8-03 «Не делать»). Правило относится и к битам, которые уже перечислены в Linux-цели (EDX `0x0781A379`: FPU, PSE, TSC, MSR, PAE, CX8, APIC, PGE, CMOV, PAT, MMX, FXSR, SSE, SSE2; `80000001` EDX: SYSCALL, NX, LM): Windows-профиль **не наследует** их таблицу целиком, а включает каждый бит только после приёмки задачи-владельца (таблица «бит → задача» в [S2-57](../plans/STAGE_2.md#s2-57-таблица-cpuid-windows-профиля-adr-0013)); где задачи нет, профиль бит не объявляет. PAE и NX (обязательны по M2 §3.5) и APIC объявляет отдельная задача [S2-58](../plans/STAGE_2.md#s2-58-объявить-pae-nx-и-apic-в-windows-профиле-adr-0013); она ждёт S2-25, S4-10 и S4-38L8d. До её принятия S2-57 блокирован, а Windows-профиль эти биты не объявляет. SSE4.2/POPCNT и SSE3/SSSE3 включены сразу, не дожидаясь наблюдения на S8-24: объявлять реализованное безопасно, а цикл «установка → остановка → реализация» занимает дни.

**Альтернативы.** (а) Только список M2 без SSE3/SSSE3/SSE4.2/POPCNT — меньше работы, но NET11 называет весь v2-набор обязательным для Windows 11; риск остановки на первой же инструкции SSE4.2/POPCNT в ядре 24H2+ и дней на цикл обнаружения. (б) Сразу x86-64-v3 (AVX/AVX2/BMI/FMA, XSAVE) — источника требования нет, объём реализации (VEX, YMM, XSAVE-состояние) кратно больше; против правила этапа 2 «не добавлять AVX/AVX2/BMI на будущее» (S2-40). (в) Расширить Linux-профиль — смешало бы две цели с разным evidence.

## Решение 2. Идентичность CPU — ждёт решения владельца

| Вариант | Значения | Риск совместимости | Продуктовый и юридический риск |
|---|---|---|---|
| A. Собственный vendor | `OpenDesktop `, family 6 / model 0 / stepping 0, brand string «Open Desktop Virtual CPU» | Высокий и **не измеренный**: проверка Setup на список поддерживаемых поколений (W-10), вероятно, не пройдёт для неизвестного vendor — источника нет, это гипотеза. Связь неизвестного vendor с bugcheck 0x5D (BC5D) — **источник не найден**. Ядро не пойдёт по vendor-специфичным путям Intel/AMD — меньше неизвестных MSR | Нет: машина не выдаёт себя за чужой процессор |
| B. Vendor `GenuineIntel` + family/model/stepping поколения из списка Microsoft | Конкретная модель — по списку Microsoft поддерживаемых процессоров Intel и с набором функций не шире объявленного (без AVX — кандидаты среди Pentium/Celeron поддерживаемых поколений; **сверить по Intel ARK**); brand string — по решению владельца | Ниже для проверки Setup (гипотеза). Ядро и загрузчик пойдут по Intel-путям: загрузка драйвера микрокода по vendor, чтение `IA32_BIOS_SIGN_ID`, попытка обновления микрокода через MSR, Intel-специфичные MSR (частота, `MISC_ENABLE`, функции управления) — каждый нужно моделировать честно или отказывать по контракту; **источник не найден**, наблюдать на S8-24 | Машина представляется процессором Intel: товарный знак, возможное введение в заблуждение пользователя, условия лицензии Windows. Решает только владелец |
| C. Vendor `AuthenticAMD` + модель из списка AMD | Аналогично B | Аналогично B (AMD-пути) | Аналогично B (AMD) |
| D. Собственный vendor + бит гипервизора с собственной сигнатурой | A + CPUID.1 ECX[31] = 1, лист `0x40000000` с сигнатурой Open Desktop, `0x40000001` ≠ `Hv#1` | По TLFS решения о совместимости принимаются по сигнатуре интерфейса; ослабляет ли это проверку Setup — **источник не найден**. Enlightenments Hyper-V не реализуются | Нет |

**Рекомендация архитектурного агента.** Задачи этапа 2 (решение 1) не зависят от выбора и начинаются сразу. Рабочее значение для отладочных прогонов до решения владельца — **A** (текущий vendor и честный brand string): так первые прогоны S7-32/S8-24 покажут, на чём именно останавливается Windows с собственным vendor (`setuperr.log`, bugcheck, снимок S8-10), а не будут гаданием. Для приёмочных прогонов G2 рекомендуется **заранее** получить решение владельца, допускает ли он **B** (или C) как запасной вариант, если A не проходит проверку Setup без обхода: без такого решения G2 при отказе A blocked, а обход `LabConfig` gate не закрывает. Выбор конкретной модели для B — после решения владельца, по списку Microsoft и Intel ARK, с записью значений в CPU_CONTRACT и [S2-57](../plans/STAGE_2.md#s2-57-таблица-cpuid-windows-профиля-adr-0013).

Решение владельца записывается сюда с датой. До него S8-03 не принимается, а значения vendor/family/model Windows-профиля, отличные от A, не объявляются.

## Решение 3. Бит гипервизора

*Статус: ждёт архитектурного ревью.*

CPUID.1 ECX[31] = 0, листы `0x40000000` и выше возвращают нули, как и любой неперечисленный лист профиля. Мы не гипервизор в смысле TLFS и не реализуем ни одного его интерфейса; enlightenments Hyper-V не реализуются. Объявление бита по TLFS обязывает ответить на листы `0x40000000`/`0x40000001`, а изменения поведения Windows при неизвестной сигнатуре не документированы — **источник не найден**. Пересмотр — если на S8-24 найдётся документированное Microsoft поведение, которое требует бита (например, допуски таймаутов), или по варианту D решения 2.

Следствие для пользователя: `systeminfo` покажет требования Hyper-V как невыполненные (нет VMX/SVM и SLAT, HVREQ), VBS/HVCI/Credential Guard/WSL2/Sandbox недоступны — это записано в [WINDOWS_GUEST_PROFILE.md](../specs/WINDOWS_GUEST_PROFILE.md#7-функции-виртуализации-и-изоляции).

## Решение 4. XSAVE/AVX

*Статус: ждёт архитектурного ревью.*

Не объявляются; `CR4.OSXSAVE` остаётся 0, состояние FP/SIMD сохраняется через FXSAVE/FXRSTOR. Источника требования XSAVE/AVX для Windows 11 нет (CPU-04). Если S8-24 остановится на `XSAVE*`/`XGETBV`/VEX-инструкции при CPUID без этих битов — это поведение гостя, противоречащее объявленному профилю, и разбирается по S8-13 с модулем и RVA; новое решение принимается только с источником или наблюдением.

## Решение 5. TSC

*Статус: ждёт архитектурного ревью.*

Инвариантный TSC (`CPUID.80000007` EDX[8]) не объявляется, пока S8-33 и владелец времени машины (этап 4, при нескольких vCPU — этап 6) не подтвердят, что TSC всех vCPU идёт с постоянной частотой и согласованно от virtual clock. Без бита Windows вправе выбрать другой источник времени (TM-01, источник не найден). Объявление бита без такой гарантии дало бы Windows ложное основание использовать TSC как источник QPC.

## Решение 6. Compatibility mode в CPL3

*Статус: ждёт архитектурного ревью.*

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

- S8-24 ждёт S2-51…S2-58 (кроме части S2-57 об идентичности, если владелец выберет A).
- Linux-цель не меняется; её tests и evidence остаются привязаны к v1.
- Каждая новая форма даёт работу этапу 3 (честный отказ JIT; ускорение — отдельно).
- Физическая проверка на Mac mini (oracle) возможна для форм, которые исполняет его CPU; что не исполняется — остаётся без hardware coverage по правилу S2-40, п. 3.
- Если владелец отклонит B/C, а A не пройдёт проверку Setup без обхода, G2 этапа 8 остаётся blocked с записанной причиной; обход не становится приёмкой.

## Открытые вопросы

1. Решение владельца по идентичности CPU (решение 2), с датой.
2. Номера битов CPUID — сверка с Intel SDM Vol. 2A исполнителями S2-51…S2-58.
3. Требует ли Windows CLFLUSH, MTRR, MCE/MCA, RDTSCP, лист 7 (W-18) — наблюдение на S8-24.
4. Как Setup определяет частоту CPU (W-09) при отсутствии листа частоты — наблюдение на S8-24; при отказе — решение об объявлении листа `0x16` (Intel) или brand string с частотой — отдельно.
5. Статус CPU-03 в аудите S7-30 («источник не найден») можно уточнить ссылкой на NET11 — решение владельца аудита; эта задача ставит только отметку «ADR-0013».
