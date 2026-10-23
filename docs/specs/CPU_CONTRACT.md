# CPU contract v0.1 — ограниченный профиль этапа 0

**Дата:** 22 октября 2026 года. **Статус:** контракт минимального прототипа S0-09 принят для сравнения interpreter/JIT; целевой CPUID/ISA-профиль для Windows и Linux не утверждён и не выводится из этого документа.

Этот документ разделяет будущую системную модель CPU и минимальную семантику вертикального среза. Прототип не эмулирует загрузочный CPU, ОС, привилегии, MMU, SIMD или полный x86-64. Все детали кодировок и вычисления флагов должны соответствовать Intel/AMD manuals из раздела источников.

## Архитектурный контракт целевого движка

| Область | Контракт / статус |
|---|---|
| Идентичность архитектуры | Гостевая архитектура — x86-64; host ISA — ARM64. Представление гостевого состояния не зависит от host-регистров и ABI. |
| Состояние CPU | В полном `CpuState` потребуются RIP, GPR, RFLAGS, режим, сегменты, control registers, MSR, векторное состояние и pending events. Поля появляются вместе с поддерживаемыми режимами; прототип ниже использует только подмножество. |
| CPUID и MSR | Нужны для будущих ОС/ПО, но конкретные листья, биты, значения и MSR не выбраны. Нельзя объявлять возможности, не реализованные и не проверенные. |
| Режимы и привилегии | Для Windows boot потребуется определить long mode, CPL, сегменты, control registers, paging и переходы firmware/loader. Точный минимум ожидает анализа выбранного ISO и S0-05. |
| Адреса | В целевой модели различать guest virtual/linear, guest physical и host address. Translation и MMIO принадлежат machine/MMU boundaries. Их ширины и canonical-address правила для профиля ОС не зафиксированы. |
| Исключения/events | Каждый реализованный instruction form должен задавать decode faults, architectural faults и состояние на exit. Отображение в guest exception нельзя заменять host panic или общим перехватом. Приоритеты для незаданного набора инструкций не утверждаются этим черновиком. |
| Память и атомарность | SMP/атомарные операции, fences и memory ordering вне прототипа. Для будущего многопоточного профиля нужен отдельный контракт и проверки соответствующей модели памяти x86. |
| JIT state | На каждом наблюдаемом exit должны материализоваться определённые архитектурные регистры/флаги и guest instruction offset. Восстановление после исключений и асинхронных событий остаётся отдельной задачей. |

Никакой CPU capability profile для запуска Windows/Linux пока не выбран. Таблицы Windows minimums и reference VM в [GUEST_REQUIREMENTS.md](GUEST_REQUIREMENTS.md) не подменяют исследование конкретного install media, CPU feature checks и драйверов.

## Предлагаемый профиль тестового прототипа S0-09

Прототип — изолированный пользовательский fragment runner с единым vCPU, только immutable code buffer и register-only вычислениями. Это набор для сравнения decoder → interpreter/JIT, а не профиль x86-64 машины.

### Состояние и запуск

- Режим: 64-bit instruction mode; никаких переходов режима, CPL transitions, сегментных обращений или privilege checks.
- Состояние: `RAX`, `RCX`, `RDX`, `RBX`, `RIP` и подмножество `RFLAGS`. Host-регистры не считаются архитектурным состоянием.
- Начальные значения GPR и `RIP` задаются входным manifest программы; `RIP` — byte offset внутри code buffer. Начальные арифметические flags очищены, reserved bit 1 RFLAGS установлен.
- Операции используют 64-bit width. Конкретные разрешённые prefix/opcode/ModR/M формы перечисляются в таблице ниже; прочие формы не поддерживаются.
- Исполнение останавливается при достижении `RIP == code_length`, исчерпании instruction budget или явной ошибке. Итог включает перечисленные регистры и определённые flags.

### Формы инструкций

| Инструкция | Минимальная форма S0-09 | Архитектурный эффект |
|---|---|---|
| `MOV r64, imm64` | `REX.W + B8+rd + imm64`, назначения только RAX/RCX/RDX/RBX | Записывает 64-bit immediate в назначенный GPR; flags не меняет. |
| `ADD r64, r/m64` | `REX.W + 03 /r`; оба операнда — один из четырёх разрешённых GPR, без памяти | Складывает source с destination; устанавливает CF/PF/AF/ZF/SF/OF по x86-64 semantics. |
| `SUB r64, r/m64` | `REX.W + 2B /r`; register-register | Вычитает source из destination; устанавливает CF/PF/AF/ZF/SF/OF. |
| `CMP r64, r/m64` | `REX.W + 3B /r`; register-register | Вычисляет destination − source, не записывает результат; устанавливает CF/PF/AF/ZF/SF/OF. |
| `JZ rel8` | `74 cb` | Если ZF=1, переход на signed rel8 от конца инструкции; иначе продолжение. |
| `JNZ rel8` | `75 cb` | Если ZF=0, переход на signed rel8 от конца инструкции; иначе продолжение. |

Условный переход может вести только на начало декодированной инструкции внутри code buffer либо ровно на `code_length` (остановка). Переход в середину инструкции, за границу, назад к недекодируемому смещению и за budget завершается явным результатом runner. Нет `CALL/RET`, стека, памяти данных, системных инструкций, CPUID/MSR, x87/SSE/AVX, interrupt delivery или MMU.

Декодер читает байты через ограниченный интерфейс; один запрос не читает за концом buffer. Максимальная архитектурная длина x86 инструкции — 15 bytes; усечённая и неразрешённая форма возвращает отдельную ошибку без частичного исполнения. Перед реализацией зафиксировать детальную таблицу принимаемых байтовых форм/REX/ModR/M, чтобы decoder не принимал более широкие формы случайно.

### Flag contract

Для `ADD`, `SUB` и `CMP` результат флагов CF/PF/AF/ZF/SF/OF определяется соответствующими x86 operation semantics. `JZ/JNZ` читают только ZF. На каждом сравнении interpreter/JIT проверяются все flags, определённые данной инструкцией. Флаги или состояния, которые архитектура определяет как undefined, не используются тестами и не считаются обязательным совпадением. Другие RFLAGS биты не моделируются и в CPUID не рекламируются.

### Ошибки и корректность

- Неизвестные, неподдерживаемые и усечённые формы дают различимые decode errors; они не исполняются приблизительно.
- Некорректная цель ветвления, instruction budget exhaustion и отсутствие доступного байта — различимые причины остановки.
- Host panic, memory safety failure и JIT code fault — дефекты реализации, не guest exception.
- Interpreter и JIT используют одинаковый набор входных байтов и сравниваются по exit reason, guest RIP, GPR и flags, определённым этой версией контракта.
- Собственные hex-fixtures для этапа 0 находятся в [`tests/fixtures/x86/`](../../tests/fixtures/x86/README.md): арифметика с переполнением, зависимый `SUB/JNZ` цикл, `CMP/JZ`, загрузка immediate и неподдерживаемый opcode. Команды сравнивают interpreter и JIT; источником семантики остаётся этот контракт и Intel/AMD manuals.

## Не включено и будущие решения

| Возможность | Статус | Что нужно до объявления |
|---|---|---|
| Windows/Linux CPU profile | Не определён | Exact media/build, сбор boot requirements, обязательные CPUID/MSR/ISA и драйверные проверки |
| x86-64 application baseline | Не определён | Результаты выбранных приложений и минимальные ABI/runtime requirements |
| 32-bit compatibility | Не включена в S0-09 | Guest mode transitions, segmentation, syscall/exception behavior и тестовый набор |
| x87/SSE/AVX | Не включены в S0-09 | State layout, exception/masking semantics, feature gating и отдельная проверка каждого расширения |
| Paging/MMU и TLB | Не включены в S0-09 | GVA→GPA translation, permissions, page faults, address-width profile и invalidation contract |
| SMP, atomics, fences | Не включены в S0-09 | x86 memory ordering, locked operations, interrupt/scheduler model и stress tests |
| Privileged/system instructions, APIC/timers, exceptions | Не включены в S0-09 | Выбранный board/boot profile, точная архитектурная семантика и приоритеты событий |

## Профиль этапа 2: s2-long-integer-v1

Принят диагностический профиль A: long mode, один CPU, 16 GPR, RIP u64,
operand sizes 8/16/32/64, address sizes 32/64, bounded fixture RAM.
Это целевой объём; таблица не объявляет формы реализованными. S0-09 выше
остаётся отдельным совместимым профилем. Методика — [INTERPRETER](../testing/INTERPRETER.md).

| Группа / точные формы | Режим / operand / address | Feature gate | Реализация и проверки | Обязательность / источник |
|---|---|---|---|---|
| GPR RAX…R15, low8/16/32/64, AH/CH/DH/BH без REX; 32-bit writes zero-extend | long / 8–64 / — | long mode; отдельного CPUID бита нет | S2-02; инструкция пока не подразумевается | A; Intel SDM 093 Vol.1 §3.4.1, §3.4.1.1 |
| MOV 88–8B, B0–BF, C6/C7 /0; MOVZX 0F B6/B7; MOVSX 0F BE/BF; MOVSXD 63; LEA 8D | long / допустимые 8–64 / 32,64 | baseline long mode | S2-08/09 MOV и S2-10 transfer срезы подтверждены на одном Intel; точные формы и hardware boundaries ниже | A; Vol.2 соответствующие mnemonic entries и encoding tables |
| ADD/ADC/SUB/SBB/CMP, AND/OR/XOR/TEST: reg↔r/m, accumulator/immediate, groups 80/81/83; INC/DEC FE/FF /0,/1; NEG/NOT F6/F7 /3,/2 | long / 8–64 / 32,64 | baseline | S2-11 arithmetic и S2-12 AND/OR/XOR/TEST/NOT подтверждены в описанных ниже положительных формах; undefined AF исключается из comparisons | A; Vol.2 entries; CF/PF/AF/ZF/SF/OF definedness по форме |
| SHL/SHR/SAR/ROL/ROR/RCL/RCR D0–D3,C0/C1; SHLD/SHRD 0F A4/A5/AC/AD | long / 8–64 (double shifts 16–64) / 32,64 | baseline | Открыто S2-13 | A; Vol.2 entries, count masking и undefined flags обязательны |
| MUL/IMUL/DIV/IDIV F6/F7 /4…7, IMUL 69/6B/0F AF; CBW/CWDE/CDQE 98 и CWD/CDQ/CQO 99 | long / по форме / 32,64 | baseline | S2-14 successful multiplication/division forms passed physical comparison; local #DE tests only. S2-15a sign-extension forms passed local checks; six fixtures await consolidated physical mini-check | A; Vol.2 entries, #DE до commit |
| XCHG 86/87,90+r; BSWAP 0F C8+r; BT/BTS/BTR/BTC 0F A3/AB/B3/BB,BA /4…7; BSF/BSR 0F BC/BD | long / по форме / 32,64 | baseline | S2-10 register-register XCHG covered; memory XCHG and LOCK are S2-21. S2-15b BSWAP (32/64) passed local checks; 7 fixtures await mini-check. S2-15c register-offset, S2-15f immediate-register, S2-15g immediate-memory-destination, and S2-15h register-index-memory-destination BT-family slices passed local checks; 12/48/48/72 fixtures await mini-check. S2-15d/e BSF/BSR register and ordinary memory-source forms (16/32/64) passed local checks; 23 fixtures await mini-check. FS/GS-prefixed BitScan memory sources remain Unsupported; register-source forms accept these prefixes. F3-prefixed LZCNT/TZCNT remain outside baseline. Baseline forms are locally covered; S2-15 awaits consolidated physical mini-check | A; Vol.2 entries |
| Jcc 70–7F/0F 80–8F, SETcc 0F 90–9F, CMOVcc 0F 40–4F; JMP EB/E9/FF /4; CALL E8/FF /2; RET C3/C2; PUSH/POP 50–5F,68/6A,FF /6,8F /0; PUSHF/POPF 9C/9D | long / по форме 16,32,64 / 32,64 | CMOV bit для CMOV; остальные baseline | S0 JZ/JNZ plus S2-16a all 16 short/near Jcc and direct relative EB/E9 JMP locally covered; 68 fixtures await mini-check. S2-16b all 16 SETcc byte forms locally covered by 79 fixtures; S2-16c all 16 CMOVcc forms locally covered by 70 fixtures, including source reads on false conditions and read-fault no-commit. S2-16d covers register-target near indirect JMP FF /4 r/m64 (3 fixtures); S2-16e covers ordinary memory r/m64 targets (7 fixtures), reading qword before canonicality check and preserving no-commit on read/#GP. FS/GS and other out-of-profile addressing, CALL, and far transfers remain open in S2-16. S2-17a covers only 64-bit GPR PUSH/POP opcodes 50–5F + REX.B (8 fixtures); S2-17b covers 64-bit immediate PUSH 6A ib/68 id, sign-extended to qword (8 fixtures); S2-17c adds near relative CALL rel32 (6 fixtures); S2-17d adds register near indirect CALL FF /2 r/m64 + REX.B (5 fixtures); S2-17e adds ordinary memory-source FF /2 r/m64 (9 fixtures), reading qword before target validation and bounded stack push; S2-17f adds only near RET imm16 C2 iw in 64-bit operand mode (9 fixtures), adding the unsigned imm16 to RSP after popping the return address; S2-17g adds 16-bit GPR PUSH/POP 50–5F via 66 and optional REX.B (10 fixtures), preserving qword default when 66 is absent; S2-17h adds only 16-bit immediate PUSH 66 6A ib/66 68 iw (11 fixtures), sign-extending imm8 for 6A and writing a literal word for 68; S2-17i adds qword memory-source PUSH FF /6 for ordinary base/REX.B/SIB/RIP-relative/address32 forms (9 fixtures), reading source before bounded stack write; S2-17j adds ordinary qword memory-destination POP 8F /0 (7 fixtures), reading at old RSP, incrementing RSP, then computing destination EA using updated RSP; S2-17k adds memory-source PUSH 66 FF /6 r/m16 (8 fixtures), with pre-push EA; S2-17l adds memory-destination POP 66 8F /0 r/m16 (7 fixtures), reading old RSP then using post-increment EA; S2-17m adds memory PUSH 66 48 FF /6 and POP 66 48 8F /0 as qword forms (4 fixtures), with REX.W taking precedence over 66; S2-17n covers register PUSH/POP with 66 + REX (6 fixtures), including effective final-REX ordering and REX.B extended registers. S2-17o covers bare `9D` POPFQ at the runner's default CPL3/IOPL0: only CF/PF/AF/ZF/SF/DF/OF are writable; bit 1 and all other RFLAGS bits are preserved, and TF/NT/AC source or current bits return `Unsupported` before CPU commit. S2-17p covers bare `9C` PUSHFQ in long mode: it writes `RFLAGS AND 00000000_00FCFFFFH` as a qword, clearing RF/VM in the stored image and preserving RFLAGS. Its physical fixture masks IF/IOPL and other unmodeled bits. S2-17q covers `66 9C` PUSHFW: it writes the lower FLAGS word at `RSP - 2` and preserves RFLAGS; a REX before 66 is ignored by the shared prefix parser, while an effective REX or other prefix remains outside this slice. ACCESS context currently supports CPL but not IOPL selection, so other POPF privilege/IOPL combinations remain out of profile. ISSUE-014 tracks unresolved RSP post-state on qword and word destination-store faults, which are not asserted by the fault fixtures. Independent Sol-medium reviews are clean; local checks passed. Linux runner tests were not linkable on macOS for the earlier register slice; S2-17e Linux native tests were not executed. Combined noncanonical target and invalid-stack priority remains unverified across relative/register/memory CALL (ISSUE-013); no dual-fault fixture/snapshot exists. All 558 S2-15/16a/b/c/d/e/17a/b/c/d/e/f/g/h/i/j/k/l/m/n/o/p/q/r/s/t fixtures await the shared mini-check. Far returns, IRET, other RET operand-width/prefix variants, other memory POP forms, other FF groups, FS/GS memory forms, other PUSHF width/prefix forms, other POPF privilege levels, and other planned forms remain open in S2-17 | A; Vol.2 entries, стек и точный RIP |
| MOVS/CMPS/STOS/LODS/SCAS A4–AF; REP/REPE/REPNE; CLD/STD | long / 8–64 / 32,64 | baseline | S2-18a locally implements bare `A4` MOVSB; S2-18b adds normalized `66 A5` MOVSW; S2-18c adds bare `A5` MOVSD (default dword width); S2-18d adds effective `REX.W + A5` MOVSQ (qword); S2-18e adds bare `AA` STOSB; S2-19a adds only `F3 AA` REP STOSB. MOVS uses implicit flat `[RSI]` read then `[RDI]` write and updates both pointers only after both accesses succeed, by ±1/±2/±4/±8 according to DF. STOSB writes AL to flat `[RDI]`, updates only RDI by ±1 after a successful store, and preserves RSI and flags. REP STOSB with RCX=0 performs no destination access; each successful store updates RDI by ±1 and decrements RCX, with flags unchanged. One instruction invocation processes at most 1024 successful iterations; this work limit is distinct from instruction retirement budget. STOP or work-limit exhaustion between iterations preserves partial state and leaves RIP at `F3 AA`; a fault preserves earlier iterations and leaves current iteration RDI/RCX unchanged. This is the local execution contract, not a general claim about exception priority. MOVSW accepts ignored misplaced REX before 66 (`48 66 A5`); effective REX.W overrides 66 (`66 48 A5` is MOVSQ). Other REP string forms, other STOS widths, segment, and address-size prefixes remain unsupported. Literal cases cover DF=0/1, zero count, aliases/canaries, and injected MOVS read/write or STOSB destination #PF; REP destination-fault fixture records only the selected partial-progress policy. Bare non-REP destination-fault index/RIP remains unconfirmed (ISSUE-016). Physical checks are deferred to shared mini acceptance. | B; Vol.2 entries, DF и частичный прогресс |
| XADD/CMPXCHG/CMPXCHG8B/16B; LOCK; fences | long / по форме / 32,64 | CX8/CX16/SSE2 по форме | S2-21a bare qword memory XCHG; S2-21b bare qword register XADD; S2-21c bare qword register CMPXCHG; S2-21d bare qword memory XADD; S2-21e bare qword memory CMPXCHG; S2-21f LOCK qword memory XADD only under serial/private synchronous RAM precondition; S2-21g bare MFENCE on explicitly opted-in private synchronous RAM. Other widths, LOCK forms and fence forms remain open. SMP/shared-memory ordering не заявляется | B и уточняемый Linux; Vol.2 entries |
| Segments, CR0/2/3/4, EFER, MSR, CPUID, descriptor/event/system instructions | long и требуемые transitions / по форме | Только завершённые feature groups | Открыто S2-22…27,41 | C; Vol.3 memory/protection/events; machine paging/events — этап 4 |
| x87, MMX, SSE/SSE2 и save/restore | по форме | FPU/MMX/FXSR/SSE/SSE2 и OS state | Открыто S2-28…39 | D; Vol.1 chapters x87/MMX/SSE и Vol.2 entries |
| Прочие расширения Linux/Windows | Не определено до исследования image/config | Не рекламировать заранее | Открыто S2-40 | Уточнить по реальному kernel/profile; возраст oracle CPU не основание исключения |

Для ещё не реализованных строк таблица задаёт группы и диапазоны кодировок,
а точные допустимые сочетания, ошибки и порядок side effects фиксируются перед
реализацией каждой формы. Whole-group CPUID bits до этого не выдаются.
Manual/generated/hardware tests S0 относятся только к исходной таблице S0-09;
новый профиль не наследует их как доказательство более широкого покрытия.

S2-16a interpreter coverage: `ConditionCode` explicitly models all 16
legacy conditions from CF/PF/ZF/SF/OF; short/near Jcc and direct relative JMP
use signed displacements from the complete instruction's next RIP with
long-mode 64-bit wrapping. Local literal coverage and remaining unsupported
prefixes/forms are recorded in the S2-16 section of the [Stage 2 plan](../plans/STAGE_2.md).
Its 68 literal fixtures are deferred to the same final physical Mac mini
acceptance substage as the 216 S2-15 fixtures; the pool later grew to 433 with
S2-16b/c. Independent review completed cleanly with no findings. S2-16b adds
all 16 SETcc encodings for byte
register and ordinary memory destinations. It preserves all flags and
definedness, writes exactly one memory byte, and leaves CPU state uncommitted
on write fault. Independent literal coverage exercises all conditions, byte
register aliases, memory addressing, and unsupported prefix boundaries. The
79 fixtures joined the shared pool; the later S2-16c addition brings the total
to 433. S2-16c adds all 16 CMOVcc forms, reads the source even when the condition
is false, and preserves no-commit behavior on read fault. A false 32-bit form
clears the upper dword after successful source read; its 70 literal fixtures
await the same physical mini acceptance. Indirect JMP remained open after
S2-16c; S2-16d later adds a register-target near form. Other control/stack
forms remain open in S2-17. S2-17a locally adds
only 64-bit GPR PUSH/POP opcode forms `50–5F` with `REX.B`: 8 fixtures cover
RSP aliasing, a bounded guest stack, the full canonical range, `#SS(0)`, and
fault no-commit. Independent Sol medium review was clean. These join the shared
physical pool, then 441 fixtures; immediate/16-bit/flags/memory PUSH/POP,
CALL/RET, and indirect forms remained open at S2-17a. S2-17b adds only the
64-bit immediate PUSH forms `6A ib` and `68 id`, sign-extended to qword, with
8 boundary fixtures covering stack bounds, faults, truncated fetch, and
no-commit. Independent Sol medium review is clean; physical validation remains
deferred in the shared pool, then 449 fixtures. S2-16d added three register-
target near indirect JMP cases, bringing the pool to 452; S2-16e adds 7
ordinary memory-target cases, bringing the shared pool to 459. S2-17c adds six
near relative CALL fixtures, bringing the pool to 465. S2-17d adds five
register near indirect CALL `FF /2 r/m64` fixtures, with the corrected stack
sentinel `A5`; S2-17e adds nine ordinary memory-source `FF /2 r/m64` fixtures.
At the end of S2-17e the shared pool was 479. Independent Sol-medium reviews are clean and
local checks passed. Linux runner tests could not link on macOS for the earlier
register slice; S2-17e Linux native tests were not executed. Physical validation
remains deferred to the shared final Mac mini acceptance. Combined
noncanonical-target plus invalid-stack fault priority remains unverified across
relative, register-indirect, and memory-indirect CALL ([ISSUE-013](../KNOWN_ISSUES.md#issue-013--не-подтверждён-приоритет-target-gp-и-stack-fault-для-near-call));
the dual-fault case is not in the corpus. FS/GS and out-of-profile addressing,
CALL, and far transfers remain open in S2-16. Other immediate and 16-bit
PUSH/POP, PUSHF/POPF, memory PUSH/POP, RET width/prefix variants, far returns,
and IRET remain open in S2-17.

S2-17f adds only near `RET imm16` (`C2 iw`) in 64-bit operand mode. The
unsigned immediate is added to RSP after the qword return-address pop. Nine
literal fixtures cover immediate boundaries, nested cleanup, budget exhaustion,
and fault no-commit cases. Independent Sol-medium review is clean. Workspace
checks, formatting, all 21 generators, Python checks, and Linux-target checks
passed; physical validation remains deferred to the shared final Mac mini
substage. At S2-17f completion the pool was 488 fixtures; S2-17g raised it to 498 and
S2-17h to 509. Far returns, IRET, 16-bit operand width, and prefix variants
remained open for RET at that point.

S2-17g adds only 16-bit GPR PUSH/POP opcodes `50–5F` with `66` and optional
`REX.B`; without `66` the default qword operand size is preserved. PUSH reads
the word source before decrementing RSP. POP increments RSP by two, then writes
the low word, including the SP alias. Ten fixtures (7 successful, 3 fault)
cover extended registers, ordering, faults, and truncated fetch. The carry case
starts at RSP `0x30ffff` with word `0x2468` and produces `0x312468`. Independent
Sol review is clean. Workspace/fmt, all 22 generators, Python, and Linux-target
checks passed; Linux native admission was cross-compiled only. Physical checks
remain deferred. The shared pool was 498 after S2-17g; S2-17h brought it to 509. Memory PUSH/POP, flags, and other planned forms remain open.

S2-17h adds only 16-bit immediate PUSH `66 6A ib` and `66 68 iw`: `6A`
sign-extends imm8 to a word, while `68` pushes the literal word. Both decrement
RSP by two and write exactly two bytes. Eleven literal fixtures cover immediate
boundaries and fault no-commit. Provenance is Intel SDM order 253667-093US,
Vol.2B PUSH pp.4-522–4-525. Independent Sol-medium review is clean. Workspace
checks, formatting, all generator freshness, Python, Linux-target, and native
`od-x86-runner` checks passed. Physical validation is deferred; the pool is now
509 fixtures. Memory PUSH/POP, flags, and other planned forms remain open.

S2-17i adds ordinary qword memory-source PUSH `FF /6`: the qword source is
read before the bounded stack write. Nine fixtures cover base, `REX.B`, SIB,
RIP-relative, address32, `[RSP]` source, budget exhaustion, source `#PF`, and
stack `#SS`, with no-commit on faults. Independent Sol-medium review is clean.
Workspace tests/checks, formatting, generator freshness, Linux-target, Python,
diff, and graph checks passed; Linux admission tests were cross-compiled only.
Physical validation remains deferred and the shared pool is now 518. Memory
POP, FS/GS-prefixed and `66` memory forms, flags, and other groups remain open.

S2-17j adds ordinary qword memory-destination POP `8F /0`: it reads from old
RSP, increments RSP, computes the memory EA using post-increment state, then
stores the qword. Six successful fixtures cover base/REX.B/SIB/RIP-relative/
address32 and `[RSP]`; the seventh checks store `#PF` and no memory write while
intentionally omitting CPU post-state assertions. ISSUE-014 tracks the unresolved
RSP commit state on destination-store fault; it is not a confirmed defect.
Independent Sol-medium review is clean. Workspace suite/checks, formatting,
25-generator freshness and Linux-target checks passed. Physical validation is
deferred; current shared pool was 525 fixtures before S2-17k/l. Other memory POP forms,
FS/GS-prefixed and `66` memory forms remain open.

S2-17k adds memory-source PUSH `66 FF /6 r/m16`. Source EA is evaluated before
RSP decrement; source and stack accesses are exactly two bytes. Address-size
`67` changes EA calculation only, and RSP remains 64-bit. Eight fixtures cover
six successful addressing/source cases, including `[RSP]` with high RSP, plus
source-read `#PF` and stack `#SS` with no-commit. Independent Sol-medium review
is clean. Workspace/fmt, 26 generators, Python, Linux-target, diff, and graph
checks passed. Physical validation remains deferred to the shared final mini
acceptance; the pool was 533 before S2-17l. Other memory POP, other `FF` groups, FS/GS memory
forms remain open. ISSUE-014 covers unresolved RSP post-state on both qword and word destination-store faults; neither is a confirmed runtime defect.

S2-17l adds memory-destination POP `66 8F /0 r/m16`: it reads two bytes at
old RSP, stages a two-byte RSP increment, computes EA after the increment
(including `[RSP]`), and stores exactly two bytes. Seven fixtures comprise six
successful addressing cases and a store-`#PF` case that asserts no memory write
but intentionally leaves CPU post-state unasserted. Independent Sol-medium
review is clean. Workspace/fmt/Linux-target, 22 generator tests, all generator
freshness, graph, and diff checks passed. Physical validation remains deferred
to the shared final mini acceptance; the pool was 540 after S2-17l. Other memory POP
forms and the `FF`/FS/GS forms listed above remain open.

S2-17m locally covers memory PUSH `66 48 FF /6` and memory POP `66 48 8F /0`.
In both forms, `REX.W` selects qword width over the preceding `66` prefix,
per Intel SDM revision 093 Vol.2A §2.2.1.2 pp.2-8–2-9. Duplicate legacy
prefix groups stay Unsupported under the current profile; among repeated REX
bytes only the final REX before the opcode is effective. Four literal fixtures
cover ordinary source/destination memory and the pre-decrement `[RSP]` PUSH
and post-increment `[RSP]` POP address rules. Physical validation remains
deferred to the shared final mini acceptance; the pool was 544 fixtures at
S2-17m completion.

Принятое decode coverage S2-06: собственный `long_decode`
нормализует prefixes/REX/66/67/opcode maps, MOV immediate 8–64 и
register-register MOV, ADD/SUB/CMP register widths, short/near JZ/JNZ.
В checkpoint S2-06 memory addressing, SIMD и REP не поддержаны; duplicate legacy groups дают
Unsupported как граница среза. Влияет только properly located REX, ранние и
misplaced REX игнорируются по SDM. LOCK консервативно Unsupported без заявления
о приоритете #UD/fetch fault; Invalid/#UD задан только для complete UD2.
Это не расширяет execution allowlist: исполняются только S0 MOV imm64,
ADD/SUB/CMP r64 для первых четырёх регистров и short JZ/JNZ, как в S2-05.
Прошедшие 19 od-isa tests не являются native execution evidence.
Точные источники decoder — Intel SDM 093
[Vol.2A](https://cdrdv2.intel.com/v1/dl/getContent/671199), §§2.1.1–2.2.1,
ADD/CMP/Jcc/LOCK, и [Vol.2B](https://cdrdv2.intel.com/v1/dl/getContent/671241),
MOV/SUB. Итоговый статус — в [отчёте](../reports/STAGE_2_REPORT.md#s2-06--decoder).

S2-07 расширил decode-only coverage на ModR/M/SIB, displacement,
RIP-relative и 67 address-size forms. Проверены 6228 raw-byte combinations;
FS/GS memory forms остаются Unsupported. Effective-address helper проверяет
canonical start address, не весь диапазон operand: full-width проверка
относится к S2-08. Для noncanonical start #SS выбирается по actual base
RSP/RBP, а #GP — для остальных, включая R12/R13. Execution сохраняет S0
allowlist до S2-08. Источники: SDM093 Vol.2A §§2.1.3–2.1.5, §2.2.1.2
Tables 2-4/2-5, §2.2.1.3, §2.2.1.6 Table 2-7; Vol.1 §§3.3.7.1,
3.4.2.1, 3.7.4.1. Результаты — в [отчёте](../reports/STAGE_2_REPORT.md#s2-07--modrmsib-и-effective-address).

### Исполнение MOV — S2-08

Реализованы B0–BF immediate-to-register и 88–8B register/register,
load/store в widths 8/16/32/64, с 16 GPR, high bytes без REX и REX low-byte
aliases. MOV сохраняет RFLAGS; 8/16-bit writes сохраняют верхние биты,
32-bit writes zero-extend. ModR/M/SIB, RIP-relative и 67 задают адрес.
67 ограничивает вычисление effective start до 32 бит, но ширина доступа
не оборачивается на 4 GiB. FS/GS memory forms остаются Unsupported.

Перед backend проверяется canonical диапазон всей ширины operand; #SS
выбирается для actual base RSP/RBP, #GP — для остальных, включая R12/R13.
Load не меняет destination до успеха, store проверяется полностью до записи;
ошибка сохраняет faulting RIP/GPR/flags. Непредставимый u64 endpoint возвращает
`MemoryError::AddressOverflow`, без backend и commit: это консервативная
boundary policy, не доказанная guest exception. Открыто
[ISSUE-012](../KNOWN_ISSUES.md#issue-012--не-подтверждена-семантика-operand-range-через-верх-u64).

В checkpoint S2-08 arithmetic execution ограничен S0 ADD/SUB/CMP r64 первых четырёх
регистров, branches — S0 short JZ/JNZ. Другие decoder descriptions не становятся
исполняемыми. C6/C7, memory arithmetic, extensions, stack и near Jcc execution
не заявлены. S2-08 принят как локальный working slice после aggregate checks;
в S2-09 затем получены физические результаты трёх положительных MOV/memory
программ ×2 с full16/RSP/RIP, flags `0xcd7`, всей memory и exact counts на
одном Intel CPU; legacy 1010 cases ×2 прошёл на тех же исходниках.
[Evidence](../reports/data/stage2/native-2026-10-22.json) не расширяет подтверждение
на все формы, privileged faults, AMD или v2 JIT. ISSUE-012 остаётся открытым.

### Передача данных и LEA — S2-10

Нормативная опора S2-10 — Intel SDM revision 093:
[Vol.2B](https://cdrdv2.intel.com/v1/dl/getContent/671241) MOV pp.4-28–4-31,
MOVSX/MOVSXD p.4-120, MOVZX p.4-130;
[Vol.2A](https://cdrdv2.intel.com/v1/dl/getContent/671199) LEA pp.3-547–3-548;
[Vol.2D](https://cdrdv2.intel.com/v1/dl/getContent/671143) XADD p.6-23 and XCHG pp.6-32–6-33.
Это publications 253666-093US, 253667-093US и 334569-093US, September 2026.
Исполнитель CPU прочитал их до реализации; независимая проверка повторно
получила official PDFs через системный curl с проверкой TLS и обработала
bytes в памяти через pdftotext. PDF pages: Vol.2A 665–666; Vol.2B 36–39,
128 и 138; Vol.2D 35–36. Само чтение manual не является аппаратным подтверждением.

| Дополнительная форма | Widths и эффект |
|---|---|
| C6 /0 ib; C7 /0 iw/id | MOV immediate в r/m8 либо r/m16/32/64; для r/m64 imm32 sign-extends до 64. Другие /extensions не принимаются как MOV. |
| A0–A3 moffs | MOV accumulator↔memory: AL либо AX/EAX/RAX; offset имеет address width 64 или 32 при 67. Это absolute offset, без ModR/M. |
| 0F BE/BF MOVSX | Sign-extension byte→16/32/64 либо word→32/64; source register или memory, destination register. Word→word BF остаётся Unsupported. |
| 0F B6/B7 MOVZX | Zero-extension byte→16/32/64 либо word→32/64; source register или memory. Word→word B7 остаётся Unsupported. |
| 63 /r MOVSXD | 66: word→word; без REX.W/66: dword→dword; REX.W: dword→qword sign-extension. 32-bit destination zero-extends верхние биты GPR. |
| 8D /r LEA | Memory addressing expression→r16/32/64; address width 32/64 независим от destination width. Нет data-memory read/write и canonical-address check; FS/GS base не применяется. Register source encoding даёт #UD после полного decode. |
| 86/87 /r и 90+r XCHG | Register forms: 86 byte, включая AL/AH aliasing без REX; 87 word/dword/qword; 90+r меняет accumulator с выбранным register. S2-21a добавляет только bare REX.W + `87 /r` memory qword XCHG: один обязательный `GuestMemory::atomic_exchange_integer` call, затем запись old value в register после успеха. |
| 0F C1 /r XADD | S2-21b adds bare REX.W qword register-register form; S2-21d adds bare REX.W qword memory-destination form through one `GuestMemory::atomic_fetch_add_integer` call. The old destination value is written to the source register, the sum is written to the destination, and CF/PF/AF/ZF/SF/OF reflect the qword addition. |
| 0F B1 /r CMPXCHG | S2-21c adds bare REX.W qword register-register form; S2-21e adds bare REX.W qword memory-destination form through one `GuestMemory::atomic_compare_exchange_integer` call. RAX is compared with the old destination; equality stores the source, mismatch copies the old destination to RAX; CF/PF/AF/ZF/SF/OF reflect the comparison. |
| 90 с register encoding 0 | NOP сохраняет весь GPR, включая верхние биты RAX. Это отличается от `87 c0` XCHG EAX,EAX, который записывает 32-bit destination и очищает верх RAX. REX.B выбирает extended register вместо NOP. |

Все формы таблицы, кроме XADD и CMPXCHG, сохраняют RFLAGS; XADD меняет только
CF/PF/AF/ZF/SF/OF согласно арифметике суммы, CMPXCHG — согласно сравнению.
8/16-bit writes сохраняют
остальные биты, 32-bit destination zero-extends, 64-bit write заменяет весь GPR.
Memory source сначала полностью читается; destination/RIP не меняются при
fault/error. Store полностью проверяется до mutation по memory contract.
RIP-relative C6/C7 использует next RIP после всего instruction, включая immediate.
Data-memory canonical/range/permission/backend checks и ISSUE-012 сохраняются;
LEA вычисляет только offset и не вызывает эти data-memory проверки.

В исходный S2-10 transfer-срез не входили LOCK, CMPXCHG/XADD, fences, FS/GS
data accesses, segment/control moves, stack operations и новые
arithmetic/near-Jcc execution. Позднее S2-21b добавил только qword
register-register XADD, S2-21c — qword register-register CMPXCHG, S2-21d —
bare qword memory XADD и S2-21e — bare qword memory CMPXCHG через отдельные
единичные backend operations; другие widths и LOCK остаются открыты в S2-21.
S2-21f добавляет только LOCK qword memory XADD через существующий
`atomic_fetch_add_integer`, при оговорённом ниже serial/private-RAM precondition.
S2-21g добавляет только bare `0F AE F0` MFENCE через явную backend completion
границу для opt-in private synchronous RAM; другие fence формы остаются вне
профиля.
Memory XCHG не заменяется неатомарной парой read/write. S2-10 принят после
Linux tests и двух физических повторов восьми v2 cases (три прежних и пять
новых transfer программ), вместе с legacy 1010 cases ×2 на том же fingerprint.
Совпали full16/RSP/RIP, flags `0xcd7`, вся memory и exact counts;
[evidence](../reports/data/stage2/transfers-2026-10-22.json) относится к этим
положительным программам на одном Intel, не к полной ISA/AMD/privileged faults/v2 JIT.

S2-21a ограничивает memory XCHG bare effective REX.W + `87 /r` qword form без LOCK и address/segment
overrides; REX.R/X/B bits may extend register/address selection. `GuestMemory::atomic_exchange_integer` является
обязательным единичным backend operation: возвращает старый qword, записывает
replacement, а при error не меняет память. Executor пишет old value в GPR только
после успешного вызова. Этот интерфейс не раскладывается на обычные read/write;
гарантии синхронизации за пределами конкретного вызова определяет backend.
`FlatMemory` test backend подтверждает границы валидации/commit для fixture,
но это не заявляет SMP, production-backend или аппаратную атомарность.
Один local fixture проверяет alias RAX/address и qword на границе двух
смежных областей; native selection и физическая проверка не включены.

S2-21b ограничен bare effective REX.W + `0F C1 /r` register-register qword
XADD без LOCK и иных overrides. Единственный literal fixture использует RAX в
обоих полях ModR/M: destination получает сумму, flags задаются арифметикой
qword ADD, остальные GPR сохраняются. Memory destination, другие widths,
CMPXCHG и native/physical claims остаются за пределами среза.

S2-21d поддерживает только bare effective REX.W + `0F C1 /r` qword XADD с
memory destination. Источник считывается до вычисления effective address,
затем вызывается один `GuestMemory::atomic_fetch_add_integer`: метод возвращает
старое значение и сохраняет wrapping sum; ошибка не меняет память. После
успешного вызова старое значение записывается в source GPR, затем обновляются
арифметические flags. Ошибка не коммитит CPU state или retire count. Этот API
не раскладывается на обычные read/write, но сам по себе не обещает LOCK,
SMP-ordering или hardware atomicity. `FlatMemory` подтверждает проверку всей
области до commit и no-mutation-on-error для split-region qword. Один
local-only literal fixture проверяет RAX как source/address base, unaligned
qword через две смежные области, wrapping result и canaries. LOCK и overrides,
native admission и физическая проверка остаются вне среза.

S2-21c ограничен bare effective REX.W + `0F B1 /r` register-register qword
CMPXCHG без LOCK и иных overrides. Сравнение выполняется между RAX и
destination: при равенстве source записывается в destination, иначе прежний
destination переносится в RAX. `CF/PF/AF/ZF/SF/OF` отражают сравнение как
для CMP, остальные RFLAGS сохраняются. Два local-only literal fixtures
покрывают обе ветви, включая source/destination alias в неравной ветви;
memory destination, native admission и физическая проверка не входят в этот
срез. Основание: Intel SDM revision 093, Vol.2A, CMPXCHG; версия документа
публикуется на [официальной странице Intel SDM](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html).

S2-21e ограничен bare effective REX.W + `0F B1 /r` qword memory destination.
`GuestMemory::atomic_compare_exchange_integer` возвращает наблюдённое старое
значение, сравнивает с сохранённым RAX и записывает source при совпадении.
Несовпадение тоже проверяет write access; при любом backend error память и CPU
state остаются без commit. Исполнитель snapshot-ит RAX и source, вычисляет EA
из pre-instruction state, вызывает метод один раз, затем обновляет CMP flags и
копирует observed в RAX только при несовпадении. Два local-only literal cases
покрывают equal store и unequal triple alias `[RAX], RAX`; оба qword unaligned
и проходят через две adjacent regions с canaries. LOCK, other widths, native
admission, SMP и physical claims остаются вне среза. Основание: Intel SDM
revision 093 Vol.2A, CMPXCHG; fixture serialization не вычисляет x86 answers.

S2-21f ограничен одним `F0 + effective REX.W + 0F C1 /r` qword memory
destination. LOCK разрешён для XADD только с memory destination; register
destination и остальные LOCK forms остаются `Unsupported` в этом профиле.
Исполнитель сохраняет обычный XADD порядок source/prestate EA/один
`GuestMemory::atomic_fetch_add_integer`/commit source и flags. Этот slice
разрешён только для последовательного interpreter с private, synchronous
обычной RAM: между операциями нет concurrent readers/writers, DMA, shared
aliases, delayed writes или MMIO. Это integration precondition; trait не
передаёт LOCK capability и вызов сам по себе не обеспечивает полного x86 LOCK
ordering или SMP semantics. Не использовать этот profile с shared/asynchronous
backend до отдельного memory-ordering contract. Не выведена hardware #AC или
split-lock policy: unaligned fixture проверяет только локальные байты backend.
Остальные widths, LOCK forms, native admission и physical claims остаются вне среза. Основание:
Intel SDM revision 093, Vol.2D XADD pp.6-23–6-24 и Vol.2A LOCK prefix;
[официальный PDF Intel SDM Vol.2A–D](https://cdrdv2-public.intel.com/782156/325383-sdm-vol-2abcd.pdf)
и [страница публикаций Intel SDM](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html).

S2-21g принимает только unprefixed `0F AE F0` в текущем long-mode profile.
Intel SDM revision 093 (253667-093US), Vol.2B MFENCE p.4-15 задаёт порядок: предшествующие
loads/stores становятся видимыми до последующих loads/stores; MFENCE не
сериализует instruction stream. Hardware принимает и другие `0F AE Fx`
варианты, игнорируя r/m bits, но этот профиль намеренно их не принимает и не
объявляет архитектурно недопустимыми. `GuestMemory::mfence` — обязательный
backend вызов без default no-op. Успех означает завершённые ранее вызванные
loads/stores; opt-in сейчас разрешён только private synchronous RAM
`FlatMemory`, где owned bytes обновлены до возврата. Code-only backend явно
отвергает fence; будущие backends обязаны определить и отдельно проверить свою
completion boundary до opt-in. Исполнитель не читает operand/data memory, не
меняет GPR/RFLAGS и коммитит RIP/retirement только после backend success. На
backend error fence не retired; ранее retired эффекты остаются. Fixture
store → fence → load и spy/failure/fetch tests подтверждают только этот local
contract, не shared/SMP ordering. CPUID SSE2 profile, native admission и
physical validation не меняются. Источник: [официальная страница Intel SDM
(revision 093)](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html),
[официальный Vol.2B PDF](https://cdrdv2.intel.com/v1/dl/getContent/671241).

### S2-22a — единственная FS-base форма в отдельном локальном профиле

S2-22a вводит отдельный schema-v2 CPU profile `s2-long-fs-base-v1`; он не
меняет wire shape и digest существующего `s2-long-integer-v1`. Поддерживается
только `64 48 8B 00` (`MOV RAX, qword ptr FS:[RAX]`) в long64/canonical48.
Начальное состояние нового профиля обязательно содержит canonical48
`fs_base`; наблюдаемый snapshot также содержит `fs_base`, а comparator требует
его точного совпадения. Это начальное состояние harness, не исполнение MOV
Sreg/WRMSR и не загрузка селектора или descriptor.

Исполнитель вычисляет effective offset из начального RAX, прибавляет полный
64-bit FS.base и только затем проверяет canonical48 всего qword linear range.
LEA по-прежнему вычисляет offset без FS.base. В память передаётся только уже
вычисленный linear address через существующий flat `GuestMemory` contract;
backend не получает segment selector, descriptor или режим сегментации. Для
выбранного `[RAX]` noncanonical linear address даёт modeled `#GP(0)`, без
объявления порядка относительно paging, #AC, LA57/LAM/LASS или descriptor
faults. На чтении или canonicality error не меняются RAX, RIP, RFLAGS,
FS.base и retirement; data callback не выполняется при canonicality error.

Одна local-only literal fixture использует `RAX=0000800000400000H` и
`FS.base=FFFF800000000000H`: итоговый linear address равен `0000000000400000H`.
Проверяются alias RAX как до чтения, так и как destination, полный наблюдаемый
state, qword data и точный fetch. Локальные tests дополнительно покрывают
canonical offset с noncanonical результатом (`#GP(0)`), backend read fault,
truncated fetch и точные соседние FS/GS decoder boundaries. Новый профиль и
FS-prefixed memory MOV внутри старого профиля fail-closed исключены из native
admission/runtime; native-selected subset не меняется. GS, memory stores,
другие регистры/адресации/ширины, `67`, segment loads, descriptors, privilege
transitions и прочие сегментные проверки остаются открытыми в S2-22.

Источник: Intel SDM revision 093 (253665-093US), Vol.1 §§3.3.7–3.3.7.1
pp.3-10–3-11 (сначала truncation effective offset, затем полный segment base
и canonical check), §3.4.2.1 p.3-15 (FS/GS bases и отсутствие long-mode
segment-limit checks), [официальный Vol.1 PDF](https://cdrdv2.intel.com/v1/dl/getContent/671436);
Vol.2B MOV pp.4-28–4-31, (253667-093US), [официальный Vol.2B PDF](https://cdrdv2.intel.com/v1/dl/getContent/671241).

### S2-22b — единственная GS-base форма в отдельном локальном профиле

S2-22b вводит schema-v2 профиль `s2-long-gs-base-v1`; wire shape
`s2-long-integer-v1` и `s2-long-fs-base-v1` сохраняется. Поддерживается только
`65 48 8B 00` (`MOV RAX, qword ptr GS:[RAX]`). Начальное и наблюдаемое состояние
содержат обязательный canonical48 `gs_base`, который comparator проверяет.

Исполнение прибавляет полный 64-bit GS.base к pre-state offset RAX и проверяет
canonical48 полного qword linear range до передачи адреса существующему flat
`GuestMemory`. При `#GP(0)`, backend read error или truncated fetch CPU state,
GS.base и retirement не коммитятся; canonicality failure не вызывает data
callback. Это не моделирует selectors, descriptors, privilege, paging, LA57,
LAM/LASS или fault delivery. Native admission/runtime исключает новый профиль,
поскольку ptrace snapshot не наблюдает гостевой GS.base. Соседние FS/GS формы
остаются unsupported.

Одна local-only literal fixture задаёт `RAX=0000800000400000H` и
`GS.base=FFFF800000000000H`, получая linear address `0000000000400000H`.
Источник: Intel SDM revision 093 (253665-093US), Vol.1 §§3.3.7–3.3.7.1
pp.3-10–3-11 и §3.4.2.1 p.3-15; Vol.2B MOV pp.4-28–4-31; те же официальные
PDF, указанные в S2-22a.

### S2-22c — единственное чтение FS selector в отдельном локальном профиле

S2-22c вводит schema-v2 profile `s2-long-fs-selector-v1`; старые integer,
FS-base и GS-base wire shapes остаются неизменны. Новый profile требует
canonical48 `fs_base` и 16-bit `fs_selector` в начальном и наблюдаемом state.
Поддерживается только `48 8C E0` (`MOV RAX, FS`): selector расширяется нулями
до 64 бит в RAX, RIP увеличивается на 3, retirement — на 1. Остальные GPR,
RFLAGS, FS.base и FS.selector сохраняются.

Форма читает уже имеющийся selector и не загружает его: GDT/LDT не читаются,
selector не валидируется, hidden descriptor state не создаётся. Это не
реализует segment load, privilege/type/present/stack checks или segment-base
memory access. Новый profile local-only: native fail-closed до mapping/tracee,
а legacy native admission отклоняет selector-read после decode. Legacy
fragment adapter также отвергает ненулевой FS selector. Остальные Sreg,
destination, widths, prefixes, memory формы и 8E остаются Unsupported.

Literal fixture проверяет zero-extension (`FS.selector=F123H`), изменение RAX,
сохранение FS.base и остального CPU state. Unit tests покрывают точную decode
границу, truncation и сохранение state. Никакие descriptor/privilege semantics
из этого среза не следуют.

Источник: Intel SDM revision 093, Vol.1 (253665-093US) §3.4.2 p.3-13
(segment registers contain selectors), [официальный Vol.1 PDF](https://cdrdv2.intel.com/v1/dl/getContent/671436);
Vol.2B MOV pp.4-28–4-31 (253667-093US; REX.W + 8C /r zero-extends selector
to r64), [официальный Vol.2B PDF](https://cdrdv2.intel.com/v1/dl/getContent/671241).

### S2-22d — единственное чтение GS selector в отдельном локальном профиле

S2-22d вводит schema-v2 profile `s2-long-gs-selector-v1`; старые integer,
FS/GS-base и FS-selector wire shapes не меняются. Новый profile требует
canonical48 `gs_base` и 16-bit `gs_selector` в initial/result state. Принимается
только `48 8C E8` (`MOV RAX, GS`): GS selector zero-extends в RAX, RIP
увеличивается на 3, retirement — на 1. Все прочие поля сохраняются.

Форма читает уже существующий selector: она не загружает или не валидирует
его, не читает GDT/LDT, не обращается к data memory и не моделирует hidden
descriptor state, privilege или faults загрузки сегмента. Новый profile
local-only и native fail-closed; legacy native admission отклоняет форму после
decode. Legacy fragment adapter отвергает ненулевой GS selector. Другие Sreg,
destination, widths, prefixes, memory формы и 8E остаются Unsupported.
S2-22 при этом остаётся открытым для descriptor loads, attributes, privilege и
stack semantics.

Источник: Intel SDM revision 093, Vol.1 (253665-093US) §3.4.2 p.3-13
(segment registers contain 16-bit selectors), [официальный Vol.1 PDF](https://cdrdv2.intel.com/v1/dl/getContent/671436);
Vol.2B MOV pp.4-28–4-31 (253667-093US; REX.W + 8C /r zero-extends to r64,
ModR/M E8 selects GS and RAX), [официальный Vol.2B PDF](https://cdrdv2.intel.com/v1/dl/getContent/671241).

### S2-22e — bounded CPL3 `VERW AX` with GDT only

This local-only schema-v2 profile is `s2-long-verw-gdt-v1`. It requires
`gdtr_base` and 16-bit `gdtr_limit` in initial and result state. The base and
the configured table range must be canonical48. Older profiles continue to
omit both fields. `LongModeState` represents this table register as optional
`Gdtr`; the legacy fragment adapter rejects a present GDTR rather than silently
dropping it. The profile fixes instruction CPL to Ring3 through the existing
`AccessContext` default.

The only accepted instruction is bare `0F 00 E8` (`VERW AX`). It reads the low
16 bits of RAX as the selector and preserves all GPRs, including all of RAX.
Null selectors (index bits zero) clear ZF without a descriptor read. TI=1 is
outside this GDT-only profile and returns Unsupported before any data access.
For other selectors, `selector & 0xfff8` is the byte offset; the complete
8-byte descriptor must fit within `gdtr_limit`, otherwise ZF is cleared without
a read. An in-range descriptor is fetched with one 8-byte GuestMemory read.
ZF is set only for an S=1 writable data descriptor whose DPL is at least both
CPL and RPL. Read-only data, code, system/gate descriptors, and privilege
mismatches clear ZF. Present is not checked, and the accessed bit is not
modified. ZF is the only flag changed; RIP advances by three and retirement
increases by one after a completed check, including checks that clear ZF.

The GDT read uses `AccessOrigin::DescriptorTable`. `AccessContext.privilege`
remains the instruction CPL (Ring3) for DPL/RPL checks, while
`page_privilege()` returns Ring0 for this individual descriptor-table access,
as required by Intel SDM Vol.3A §6.11.5. This origin does not spread to ordinary
memory operands. GuestMemory remains a linear-address byte interface; the
bounded fixture backend does not claim paging or exception delivery. An
architectural read fault propagates and commits no CPU state or retirement; a
backend error remains a harness/backend failure and does not become ZF=0.

Tests cover writable and read-only data, code, system, DPL mismatch, RPL0,
null selector, TI rejection, exact-fit and one-byte-short table bounds, no-read
cases, descriptor-byte preservation, one 8-byte read with CPL3/descriptor
origin/page privilege Ring0, truncated fetch, neighboring encodings, and
injected page/backend read faults with no commit. Eight literal cases cover
the principal successful and ZF-clearing cases. Native execution rejects this
profile before mapping/tracee creation, and legacy native admission rejects the
decoded form. This extraction does not implement descriptor loading, hidden
segment caches, present faults, LDT, other privilege levels, stack checks, or
other modes; S2-22 and S2-27 remain open for their remaining work.

Sources: Intel SDM revision 093, Vol.2C (326018-093US), “VERR/VERW—Verify a
Segment for Reading or Writing,” pp.5-162–5-163, [official Vol.2C PDF](https://cdrdv2.intel.com/v1/dl/getContent/671072);
Vol.3A (325384-093US), §3.5.1 “Segment Descriptor Tables,” §6.10.2 p.6-25,
and §6.11.5 p.6-29, [official combined Vol.3 PDF](https://cdrdv2.intel.com/v1/dl/getContent/671447).

### S2-22f — CPL3 GDT-only `MOV FS, AX` descriptor load

This partial local-only schema-v2 profile is `s2-long-fs-load-v1`. It admits
only bare `8E E0` (`MOV FS, AX`) at Long64/CPL3. The source is the low 16 bits
of RAX; memory sources, other source registers/Sreg destinations, prefixes,
GS/SS, and every other encoding remain Unsupported. Initial and result state
require canonical48 `fs_base`, the 16-bit visible initial `fs_selector`,
`gdtr_base`, `gdtr_limit`, and an explicit FS cache (`fs_cache_limit`, `fs_cache_access`,
`fs_cache_flags`). Existing profiles omit these fields and retain their wire
shapes. The legacy fragment adapter rejects non-default hidden cache state.

The profile does not require the initial cache to match the descriptor bytes
currently in the GDT: cached state may outlive changes to the table. A
successful load replaces `fs_selector`, sets `fs_base` to the zero-extended
32-bit descriptor base, and replaces the effective cached limit and descriptor
attributes. The effective limit expands the 20-bit limit with low 12 one bits
when G=1; otherwise it is the 20-bit limit. `fs_cache_access` stores the
descriptor access byte and `fs_cache_flags` stores the descriptor flag nibble.
These cached fields are observed state, but this slice does not apply their
limit/type/privilege attributes to ordinary FS memory accesses. GPRs, RFLAGS
and their definedness, GDTR, GS state and memory are preserved on success.
RIP advances by two and retirement increases by one.

The selector in AX must be non-null and use TI=0; null selectors 0000–0003 and
TI=1 selectors are Unsupported before descriptor access. The initial visible
`fs_selector` is preserved as input state and may independently have any 16-bit
value. TI/LDT is outside the profile. For a non-null GDT selector,
`selector & 0xfff8` selects the descriptor. The complete 8-byte entry must fit
within the GDTR limit; otherwise the instruction raises #GP(selector) without
a read. An in-range entry is fetched by one 8-byte `GuestMemory::read` with
descriptor-table origin. Architectural CPL remains Ring3 for checks while the
read uses supervisor page privilege. A read fault propagates its original
vector, error code and linear address with no CPU/retirement commit; backend
errors remain backend failures.

After a successful read, validation order is: require S=1 and a data or
readable-code type; require `max(CPL,RPL) <= DPL` for data and nonconforming
readable code (conforming readable code follows its conforming-code rule); then
check P; then inspect A. Type/privilege failures raise #GP(selector); P=0 raises
#NP(selector). Both selector exceptions use error code `selector & 0xfffc` with
EXT=IDT=0 and RPL cleared. For an otherwise-valid present descriptor with A=0,
the interpreter makes exactly one `GuestMemory::write` at
`descriptor_address + 5`, with the same descriptor-table `AccessContext` and
the one-byte value `access | 1`. An A=1 descriptor requires no write. The
byte-sized backend call is the selected semantic memory access; it does not
claim a physical bus transaction width. Only after a successful A-bit write
does the interpreter update the visible selector, zero-extended base32,
effective limit and cached attributes, then commit RIP and retirement. A read
or write error propagates with no current-instruction CPU/RIP/retirement commit;
the generic `GuestMemory` no-mutation-on-error contract applies. The descriptor
write is admitted only for private synchronous RAM with serial interpreter
execution and no external observer/writer, DMA, shared alias, delayed write or
MMIO. A=0 is not supported by native profile admission.

Tests cover exact decode/truncation/neighbors; null/TI/CPL boundary; full and
short GDT bounds; one descriptor read with correct origin; read faults and
backend errors without commit; data/read-only/readable-code/system/execute-only
types; CPL3/RPL0/DPL0 privilege regression; #GP before #NP priority; P=0 #NP;
A=0 writable/read-only-data success with access-byte-only update; A=1 success
without write permission; exact read8 then write1 trace and descriptor context;
architectural/backend write errors without commit; validation faults before an
armed write error; split adjacent GDT regions where only the access byte is
writable; one write across repeated loads; a committed load/write before a
later fault; base32 high-bit zero extension, G=0/G=1 limit expansion, attribute
preservation, full state and cache lifetime.
This CPL3 slice does not claim a full RPL/CPL differential matrix. Native
execution rejects the profile before mapping/tracee creation and legacy native
admission rejects the decoded form. Beyond the single A-bit write for this
exact `MOV FS, AX` form, it implements no general accessed-bit updates for other
descriptor loads. LDT, null loads, other privilege levels, stack semantics,
present-bit updates, trap delivery, and general FS limit/type checks remain
outside this slice.

Sources: Intel SDM revision 093, Vol.2B (253667-093US), MOV pp.4-28–4-31,
[official PDF](https://cdrdv2.intel.com/v1/dl/getContent/671241); Vol.3A
(325384-093US), §§3.4.3 p.3-8, 3.4.4 p.3-9, 3.4.5.1 pp.3-12–3-13, 3.5.1
p.3-15, 5.6.1 pp.5-33–5-35, 5.7 pp.5-37–5-38, 6.4.1.1 p.6-6,
6.6–6.6.1 pp.6-8–6-9, 6.11.5 p.6-29, 7.13 pp.7-19–7-20 and Event 11
p.7-41, 11.1.2.1 p.11-3 Automatic Locking,
[official combined Vol.3 PDF](https://cdrdv2.intel.com/v1/dl/getContent/671447).

### S2-22h — CPL3 GDT-only `MOV GS, AX`

This partial local-only schema-v2 profile is `s2-long-gs-load-v1`. It admits
only bare `8E E8` (`MOV GS, AX`) at Long64/CPL3, with AX as the source. Its
wire state requires canonical48 `gs_base`, the visible 16-bit `gs_selector`,
`gdtr_base`/`gdtr_limit`, and the explicit GS descriptor cache
(`gs_cache_limit`, `gs_cache_access`, `gs_cache_flags`). Existing profiles,
including `s2-long-fs-load-v1`, keep their existing wire shapes. FS state is
preserved by the execution path; this profile does not add FS fields to its
wire state.

The selector must be non-null and use TI=0; null and LDT selectors, other CPLs,
missing GDTR/cache state, prefixes, other source registers, memory sources,
and other Sreg destinations remain Unsupported before descriptor access. For
an admitted selector, the implementation applies the same bounded GDT checks
as the FS load: full 8-byte bounds and canonical-address validation, one
descriptor-origin read with architectural CPL3 and page privilege Ring0, then
S/type/readability, privilege, P and A checks in that order. Type/privilege and
bounds faults raise #GP(selector); P=0 raises #NP(selector), with error code
`selector & 0xfffc`. A=0 causes one byte write of `access | 1` at descriptor
byte 5 using the same context; A=1 is read-only. Only after the final memory
operation succeeds does the interpreter update GS selector, zero-extended
base32 and effective limit/cache attributes, then advance RIP by two and
retire once. GPRs, RFLAGS, all FS state and GDTR are preserved.

The effective cached limit expands the 20-bit descriptor limit when G=1;
cache attributes are observable state and do not gate ordinary GS memory
accesses in this slice. A=0 writes are limited to private synchronous RAM with
sequential execution and no concurrent GDT writer, shared alias, DMA, delayed
write or MMIO. The byte-sized backend operation is the selected semantic access
size; it does not claim physical transaction width. Native profile admission
fails before mapping, and legacy native admission rejects the decoded form.
Automatic LOCK ordering remains the general open ISSUE-017. This exact load
does not close S2-22 or imply Linux user-profile support.

Tests cover exact decode/truncation and rejected neighbors, A=0/A=1, high-bit
base32, G=0/G=1 limits and cache attributes, full FS-state preservation,
null/TI/CPL/missing-state no-read behavior, bounds and fault priority, one
read8 then optional write1 trace with descriptor origin/CPL3/pageRing0,
architectural/backend/permission errors without CPU commit, split GDT regions,
repeated selectors, later-fault persistence, literal result comparison,
schema isolation, old FS-profile compatibility, and native/legacy rejection.

This task reuses the Intel SDM revision 093 MOV and descriptor sources already
recorded for S2-22f/g; no new independent PDF verification is claimed.

### S2-22i — loaded GS.base composition fixture

One additional local-only literal fixture uses the existing
`s2-long-gs-load-v1` profile to run exactly `8E E8 65 48 8B 00`: load GS from
the CPL3 GDT selector `0x000b`, then execute the already-supported
`MOV RAX, qword ptr GS:[RAX]`. Profile validation admits the original one-
instruction load fixtures and this exact two-instruction byte sequence with a
two-instruction budget and matching stop RIP; it does not admit other program
shapes. No opcode semantics or native/physical eligibility are added.

The literal starts with `RAX=0x0b`, `GS.base=0x00400000`, GDTR at
`0x00200000` with limit 15, and an A=0 writable DPL3 data descriptor at
`0x00200008` encoding base32 `0x80000000`, limit 0 and flags 0. The first
instruction changes only descriptor byte `0x0020000d` from F2 to F3 and loads
the GS selector/base/cache; it retires once at RIP `0x00100002`. The second
reads qword `0x1122334455667788` at `0x8000000b`, yielding RIP `0x00100006`
and two total retirements. A contrasting canary at the old base address
`0x0040000b` remains untouched. This deliberately uses offset `0x0b` beyond
the cached limit 0; as defined for the existing Long64 GS memory form in
S2-22b, long-mode FS/GS runtime segment limits are not checked. The descriptor
is not reread by the ordinary data access.

Unit trace evidence records the descriptor read8 and optional A-byte write1
with descriptor-table origin, architectural CPL3 and page privilege Ring0,
followed by only the explicit qword data read at `0x8000000b` with ordinary
CPL3 context. A later data-read fault preserves the first instruction's GS
cache, A-bit memory write and retirement while leaving the second instruction
unretired. A canonicality case uses initial RAX `0x00007fff8000000b`: its low
selector remains `0x000b`, the loaded base produces noncanonical
`0x000080000000000b`, and the existing canonicality rule raises #GP(0) before
any data callback while preserving the first instruction's committed effects.
Canonicality and no-limit behavior reuse the checks already recorded for
S2-22a/b; no independent manual verification is claimed.

### S2-22j — loaded FS.base composition fixture

One additional local-only literal fixture uses `s2-long-fs-load-v1` for
exactly `8E E0 64 48 8B 00`: load FS from the CPL3 GDT selector `0x000b`, then
execute the already-supported `MOV RAX, qword ptr FS:[RAX]`. Profile
validation admits the existing one-instruction `8E E0` fixtures and this
exact two-instruction program only, with matching budget, code mapping and
stop RIP. It does not add other byte shapes or native/physical eligibility.

The literal starts with `RAX=0x0b`, `FS.base=0x00400000`, GDTR at
`0x00200000` with limit 15, and an A=0 writable DPL3 data descriptor at
`0x00200008` encoding base32 `0x80000000`, limit 0 and flags 0. The first
instruction changes descriptor byte `0x0020000d` from F2 to F3 and loads the
FS selector/base/cache; it retires at RIP `0x00100002`. The second reads qword
`0x1122334455667788` at `0x8000000b`, reaches RIP `0x00100006`, and retires
for a total of two instructions. The canary at old-base address
`0x0040000b` remains unchanged. As in S2-22b, this uses offset `0x0b` past
cache limit zero because Long64 FS/GS runtime limits are not checked. The
ordinary data read uses loaded cached FS.base and does not reread the GDT.

Unit traces assert descriptor read8 and optional A-byte write1 at CPL3 with
descriptor-table origin/pageRing0, followed by exactly one explicit CPL3 data
read8. Later data-read #PF preserves the first instruction's FS/cache,
descriptor write and retirement. A noncanonical linear address raises
#GP(0) without a data callback and preserves the same first-instruction
effects. These canonicality/no-limit properties reuse S2-22a/b evidence; no
independent manual verification is claimed. Fixture bytes/state/data are
handwritten inputs; Rust tests keep expected post-state and memory answers as
literals. S2-22 and ISSUE-017/018 remain open.


### Арифметика и eager flags — S2-11

Реализованы long-mode ADD/ADC/SUB/SBB/CMP widths 8/16/32/64: оба направления
reg↔r/m (`00–03`, `10–13`, `28–2B`, `18–1B`, `38–3B`), accumulator immediate
(`04/05`, `14/15`, `2C/2D`, `1C/1D`, `3C/3D`) и groups `80/81/83`
с `/0,/2,/5,/3,/7` соответственно. INC/DEC используют FE/FF `/0,/1`,
NEG — F6/F7 `/3`. Short 40–4F INC/DEC не принимаются в long mode: это REX.
Memory operands используют ранее реализованные ModR/M/SIB/RIP-relative и 67;
8/16-bit writes сохраняют верхние биты, 32-bit writes zero-extend.

`83` sign-extends imm8 до operand width; qword accumulator/group81 forms
sign-extend imm32. Арифметического imm64 нет. ADC/SBB используют входной CF
без потери carry/borrow при переполнении `rhs+CF`. ADD/ADC/SUB/SBB/CMP
вычисляют CF/PF/AF/ZF/SF/OF eager. CMP только меняет flags и сохраняет весь
destination, включая upper32. INC/DEC сохраняют входной CF, меняя остальные
пять flags. NEG: CF=0 только для нуля, иначе 1; signed minimum устанавливает OF.
DF и остальные незатронутые flags сохраняются.

Точные нормативные entries Intel SDM093:
[Vol.2A](https://cdrdv2.intel.com/v1/dl/getContent/671199) ADC pp.3-9–3-11,
ADD pp.3-14–3-15, CMP pp.3-161–3-162, DEC pp.3-262–3-263,
INC pp.3-457–3-458; [Vol.2B](https://cdrdv2.intel.com/v1/dl/getContent/671241)
NEG pp.4-158–4-159, SBB pp.4-613–4-615, SUB pp.4-685–4-686.
В каждом entry проверены opening encoding table, Operation, Flags Affected
и 64-Bit Mode Exceptions. Общие flags — Vol.1 §3.4.3.1; aliases — §3.4.1.1.

Memory RMW сначала читает source/destination и подготавливает result/flags;
запись проходит целиком до CPU commit. Read/write fault или backend error
сохраняет faulting RIP/GPR/flags/RAM, инструкция не retire. CMP memory
не вызывает write, даже прежним значением. Это подтверждено injected local
read/write #PF/backend tests во втором logical region, не аппаратной доставкой
guest #PF. Native positive profile проверяет диапазоны/permissions до step;
host signal не преобразуется в guest fault. В checkpoint S2-11 LOCK/REP,
FS/GS data arithmetic, logic/TEST, system/SIMD и полный CPUID profile не включены.

313 буквальных one-instruction arithmetic fixtures охватывают формы и widths,
carry/borrow/overflow/AF/parity/zero, CF-preservation, negative immediates,
aliases и address32; это явная матрица, не весь cross-product 16 GPR/addresses.
Материализатор упаковывает handwritten templates и literal answers, не вычисляет
x86 semantics. Все 313 вместе с восемью прежними v2 cases прошли physical
interpreter/native сравнение дважды на одном Intel, без расхождений, со
стабильными повторами; legacy 1010 cases ×2 прошёл на том же fingerprint.
[Evidence](../reports/data/stage2/arithmetic-2026-10-22.json) не подтверждает AMD,
privileged faults, v2 JIT или полный этап 2. ISSUE-011/012 остаются открытыми.

### Логика, TEST и definedness — S2-12

Реализованы long-mode AND/OR/XOR widths8/16/32/64: reg↔r/m обеих
направленностей (`20–23`, `08–0B`, `30–33`), accumulator immediate
(`24/25`, `0C/0D`, `34/35`), groups `80/81/83` с `/4,/1,/6` соответственно.
TEST принимает `84/85` reg/r/m, `A8/A9` accumulator immediate и `F6/F7 /0`
immediate r/m; он сохраняет весь destination, включая upper32 GPR, и не
вызывает memory write. NOT использует F6/F7 `/2` reg/memory, инвертирует
operand с обычными alias/32-bit zero-extension правилами и сохраняет flags.
`83` imm8 и qword imm32 sign-extend по форме; memory address32/64,
ModR/M/SIB/RIP-relative наследуют существующий контракт. LOCK/REP,
FS/GS data logic, system/SIMD и полный CPU profile не включены.

Intel SDM093: [Vol.2A](https://cdrdv2.intel.com/v1/dl/getContent/671199)
AND pp.3-60–3-61; [Vol.2B](https://cdrdv2.intel.com/v1/dl/getContent/671241)
OR pp.4-163–4-164, NOT pp.4-161–4-162, TEST pp.4-721–4-722;
[Vol.2D](https://cdrdv2.intel.com/v1/dl/getContent/671143) XOR pp.6-40–6-41.
AND/OR/XOR/TEST сбрасывают CF/OF и определяют SF/ZF/PF; AF undefined.
NOT не меняет ни значения flags, ни их определённость. DF сохраняется.
Значение undefined AF не является expected результатом и не сравнивается.

`FlagEffects` — общая decoded metadata `defined/undefined`, отдельно от
вычисленных RFLAGS values. Mask обновляется только после успешного retirement;
native tracer делает это после подтверждённого TRAP_TRACE. Это не expected
evaluator и не подмена hardware values. Fault/unsupported не меняет mask.
Logic/TEST снимает AF из mask: `0xcd7 → 0xcc7`; MOV/NOT сохраняют mask;
последующая arithmetic, определяющая AF, восстанавливает его. Поэтому
`XOR; MOV; NOT` сохраняет AF undefined, а `XOR; ADD` восстанавливает defined AF.
V2 snapshot хранит semantic `rflags_defined` отдельно от fixture value mask.

197 literal logic fixtures и предыдущие 321 cases прошли physical
interpreter/native сравнение дважды (518×2): ноль различий, стабильные повторы.
Оба backend зафиксировали 684 snapshots с mask `0xcd7` и 352 с `0xcc7`.
Local injected read/write architectural/backend failures подтверждают отсутствие
retirement, partial RAM/CPU commit и mask update; это не hardware guest #PF.
Positive native admission исключает недоступные ranges/permissions до step,
host signal остаётся отдельным outcome. [Evidence](../reports/data/stage2/logic-2026-10-22.json)
не подтверждает AMD, privileged faults, v2 JIT или полный этап 2.
ISSUE-011/012 остаются открытыми.

### Сдвиги и вращения — S2-13

В диагностическом long-mode профиле реализованы SHL/SAL/SHR/SAR, ROL/ROR,
RCL/RCR и SHLD/SHRD с immediate и CL counts в поддержанных register и memory
формах. Count проходит нормативное masking и, для вращений, требуемое
сокращение effective count; нулевой effective count сохраняет destination,
RFLAGS и semantic definedness. SHLD/SHRD с count больше operand width имеет
architecturally undefined destination и не сравнивается как точное значение.
Для SHL/SAL/SHR/SAR, вращений и double shifts CF/OF definedness зависит от
effective count и формы; остальные изменяемые flags и их definedness следуют
таблицам конкретных инструкций. Значения undefined flags/destination не
используются как expected values. 32-bit destination writes сохраняют
архитектурное zero-extension правило long mode.

Точные нормативные entries Intel SDM revision 093: Vol.2B SAL/SAR/SHL/SHR
pp.4-582–4-585 и RCL/RCR/ROL/ROR pp.4-521–4-524; Vol.2A SHLD p.4-640,
SHRD p.4-643. Общие правила 32-bit GPR writes — Vol.1 §3.4.1.1. В частности,
две 16-bit SHLD/SHRD raw17 cases с undefined destination исключены из
physical exact comparison; локальные tests проверяют отсутствие panic и
undefined arithmetic flags. README `od-isa` уточняет count-zero правило для
dword destination.

Memory RMW читает и подготавливает результат до записи и CPU commit. Локальные
injected read/write faults для нулевого и ненулевого effective count проверяют
сохранение CPU/RIP/definedness/RAM и отсутствие retirement. Native cases
проверяют успешные memory RMW; admission проверяет ranges/permissions до
исполнения. Harness не получает post-host-signal snapshot, поэтому native
physical fault-state preservation не заявляется.

142 register cases и 2 memory RMW cases прошли physical interpreter/native
comparison дважды; interpreter/native diffs — ноль, все 662 suite cases
стабильны на repeat 2. По обоим backends маски snapshots: `0x04c6` — 16,
`0x04c7` — 12, `0x04d7` — 32, `0x0cc7` — 412, `0x0cd7` — 852 (1324 всего).
Legacy regression: 1010 inputs ×2, ноль diffs/errors. [Evidence](../reports/data/stage2/shifts-2026-10-22.json)
и raw manifests фиксируют fingerprint, hashes, inputs и окружение; это не
подтверждает AMD, privileged faults, v2 JIT или полный этап. ISSUE-011/012
остаются открытыми.

### Умножение, деление и `#DE` — S2-14 (успешные формы физически проверены)

Локально исполняются однопараметрические MUL/IMUL (`F6/F7 /4,/5`) с
результатом в AX либо DX:AX, EDX:EAX, RDX:RAX; двухоперандный IMUL
(`0F AF`) и IMUL с immediate (`69/6B`) записывают destination-width
результат, immediate sign-extends по encoding. Для MUL/IMUL CF/OF отражают
переполнение по форме, SF/ZF/AF/PF undefined. DIV/IDIV (`F6/F7 /6,/7`)
делят AX, DX:AX, EDX:EAX или RDX:RAX на r/m и пишут quotient/remainder в
соответствующие implicit halves; signed IDIV округляет quotient к нулю.
Нулевой divisor и quotient вне диапазона порождают локальный `#DE` до commit;
по успешному DIV/IDIV все arithmetic flags undefined. Undefined значения
flags исключаются из literal comparisons.

Verified Intel SDM revision 093 references: [Vol.2B](https://cdrdv2.intel.com/v1/dl/getContent/671241)
MUL pp.4-140–4-141; [Vol.2A](https://cdrdv2.intel.com/v1/dl/getContent/671199)
IMUL pp.3-451–3-454, DIV pp.3-264–3-266, IDIV pp.3-448–3-450. This
volume split was checked against the official revision 093 PDFs; DIV/IDIV are
in Vol.2A, as is IMUL. Vol.1 §3.4.1.1 covers GPR aliases and 32-bit writes.

The local fixture generator materializes 38 cases, and
`tools/runner/tests/fixture_v2.rs::multiply_divide_cli_fixtures_match_literal_full_state_and_flag_definedness`
checks all 38 successful operations against literal full GPR/RIP/flags-definedness/
memory answers; each has normal stop outcome and no exception. Divide-error
inputs (zero divisor and unrepresentable quotient, including signed minimum / −1)
are tested by the local `od-isa` unit test
`divide_errors_preserve_exact_state_and_report_de_before_retirement`. S2-20a
adds two public fixture-v2/CLI cases for byte `DIV BL`: zero divisor and
quotient overflow. They assert vector 0, unavailable error code/fault address,
the faulting instruction RIP, no retirement, and unchanged full CPU/memory
state. S2-20c adds one analogous qword `IDIV RCX` quotient-overflow case for
signed 128-bit minimum divided by −1. These local-only cases are separate from
the 38 successful-operation fixtures and have not been included in native
acceptance selection or the physical mini-run. They do not assert conflict
priority or native exception delivery.
The generic native admission path currently accepts register-form DIV, but this
slice does not execute these fault cases against that backend, including the
register-form IDIV case.
Локальные workspace tests/check прошли. Итоговый физический прогон сравнил 38
успешных S2-14 fixtures ×2 в корпусе из 700 inputs (1400 comparisons), без
различий interpreter/native и со стабильностью всех 700 cases на repeat 2.
Снимки включают полные 16 GPR, RIP и RFLAGS; v2 JIT не участвовал.
Основные divide-error inputs также проверяются в local `od-isa` tests; два
byte DIV cases (S2-20a) и один qword IDIV case (S2-20c) дополнительно проходят
fixture-v2/CLI. Физическая
доставка `#DE` и Linux host signal/context mapping не подтверждены. Подробности:
[S2-14 report](../reports/STAGE_2_REPORT.md#s2-14--умножение-и-деление) и
[evidence](../reports/data/stage2/multiply-divide-2026-10-22.json).

### Complete `UD2` exception exit — S2-20b

The complete two-byte opcode `0F 0B` raises vector 6 (`#UD`). Local fixture-v2
case `s2_20b_ud2` checks the public CLI snapshot: unavailable error code and
fault address, faulting RIP at the first UD2 byte, no retired instruction, and
unchanged full GPR/RFLAGS/RIP and code memory. This applies only after both
instruction bytes have been fetched; it makes no claim about #UD versus fetch
fault priority, invalid prefixes, native exception delivery, or physical
hardware behavior. Literal bytes and answers are serialized by
[`generate_stage2_ud2.py`](../../tests/cpu/generate_stage2_ud2.py), based on
Intel SDM revision 093 Vol.2A UD2.

### Integer bit operations — S2-15 local slices

Locally checked S2-15a–h slices cover sign-extension opcodes 98/99, BSWAP,
register-offset and immediate BT-family operations, BT-family memory destinations
with immediate or register bit indices, and ordinary BSF/BSR register and
memory-source forms. Their 216 literal fixtures remain pending one consolidated
physical Mac mini mini-check. Together with register-register XCHG from S2-10,
all baseline integer forms in the CPU contract row are locally covered. S2-15
remains open until the consolidated physical check; Stage 2 remains open under
the broader roadmap criteria.

For register-index BT/BTS/BTR/BTC memory forms, interpret the bit offset as a
signed operand-width integer `i`, derive the memory word with `floor(i / w)`,
and the in-word bit with Euclidean modulo `i mod w`; `w` is the operand width
in bits. Access exactly `w/8` bytes at the adjusted address. BT performs no
write; BTS/BTR/BTC always write the full-width result. Read/write faults do not
commit CPU state, flags, RAM or retirement. Native admission checks the adjusted
range and permissions; it is not physical execution. Intel SDM revision 093,
Vol.2A gives the general bit-index rule at pp. 3-11–3-12 and instruction
semantics at BT 3-112–3-113, BTC 3-114–3-115, BTR 3-116–3-117 and BTS 3-118–3-119.

FS/GS-prefixed BitScan memory sources remain Unsupported while segment bases
are not modeled; register-source BitScan forms accept FS/GS prefixes because
the segment override does not affect a register operand. Regression
`canonical48_checks_select_ss_only_for_rsp_rbp_bases` checks both rejection of
FS/GS memory BitScan forms and acceptance with register sources. LOCK/atomic
forms are outside these non-LOCK slices and belong to S2-21. F3-prefixed
BSF/BSR, including feature-dependent LZCNT/TZCNT, remain outside the baseline.
Independent S2-15h code/SDM review completed cleanly with no findings.

Для раннего Linux принят прямой 64-bit entry из Linux boot protocol §1.14:
long mode с paging, identity mappings требуемых buffers, GDT и selectors,
IF=0, RSI=boot_params. Установку mapping и доставку machine events проверяет
этап 4; CPU callbacks отдельно проверяют проверки привилегий и CPU state.
Real-mode entry не требуется выбранным handoff. Статически исследованы exact
Ubuntu amd64 package `7.0.0-38.38`, image `7.0.0-38-generic`, его setup header
и buildinfo config; SHA-256 и значения header опубликованы в
[Linux profile report](../reports/STAGE_2_LINUX_PROFILE.md). Image/config
разобраны в памяти по official sources, не staged/booted; APT/PE signatures
не проверены. Required capabilities config — нижняя граница исследования,
не готовый ISA contract. Upstream v7.0 `configure_5level_paging` требует
32-bit trampoline при mismatch paging levels; direct64 не исключает этот
путь. Нужны сопоставление с Ubuntu source/build и проверка boot.
Reachable ISA/переходы и полный профиль C/D не приняты; это открытые
критерии S2-44, не исключение C/D из этапа.
Windows CPU profile также остаётся открытым.

## Local profile S2-28a: `s2-long-x87-state-v1`

This state-only profile is separate from the existing integer profiles. Its
v2 fixture/result state contains an optional `x87` object only when this
profile is selected; an absent object in older profiles means x87 state is
outside that profile, not that the FPU was initialized. The object preserves
eight physical 10-byte register payloads byte-for-byte, the full 16-bit
physical tag word, FCW, FSW, FIP, FDP, and FOP. Register payload bytes are
serialized in increasing memory-address order as exactly 20 lowercase hex
digits per register. Tag values are the full-tag-word encodings 00 valid, 01
zero, 10 special, and 11 empty, indexed by physical register. TOP is derived
from FSW bits 13:11; logical ST(i) selects physical register `(TOP+i)&7`.
Tag lookup remains physical, and empty registers retain their serialized
payload bytes.

For this Long64 contract FIP and FDP are 64-bit offsets, as Vol.1 §8.1.8
specifies for processors supporting IA-32e mode. The profile includes no FCS
or FDS selectors: the selected Long64 state image uses the 64-bit save form
(REX.W), whose Vol.1 §8.1.8 rules say selectors are not saved. FOP is stored
as its 11 architectural bits; values above `0x7ff` are rejected. FCW/FSW and
pointer values otherwise remain raw, including status exception flags. There
is no separate pending-exception boolean and this profile performs no x87
exception delivery.

`X87State::fixture_seed()` is deterministic test data, not an architectural
reset. `X87State::fninit()` models the FNINIT state transition only: FCW becomes
`0x037f`, FSW `0`, full FTW `0xffff`, FIP/FDP/FOP `0`, while all 80-bit data
payloads remain unchanged. It does not model FINIT's pending-exception check
or wait behavior. This is distinct from power-up/RESET/INIT: Vol.3A §11.1.1
Table 11-1 note 5 says INIT leaves x87 FPU and MMX registers unchanged.

The local fixture profile is restricted to one NOP with budget 1 and a stop
RIP immediately after it. Fixture conversion loads the supplied x87 object
into `LongModeState`, and result conversion observes that executed state;
comparisons inspect the entire object. The fragment adapter explicitly rejects
`Some(x87)`. Native x87 observation is unavailable, so native execution rejects
this profile; no FPU CPUID feature or x87 instruction support is claimed.
Remaining x87 semantics and integration stay open under S2-28–S2-39.

Normative basis: Intel SDM revision 093, Vol.1 §§8.1.2, 8.1.3.1, 8.1.5,
8.1.7–8.1.9 and 8.5; Vol.2A “FINIT/FNINIT—Initialize Floating-Point Unit,”
pp.3-345–3-346; and Vol.3A §11.1.1 Table 11-1 note 5. Official manuals:
[Vol.1 253665-093US](https://cdrdv2.intel.com/v1/dl/getContent/671436),
[Vol.2A 253666-093US](https://cdrdv2.intel.com/v1/dl/getContent/671199),
[Vol.3A combined manual](https://cdrdv2.intel.com/v1/dl/getContent/671447).

## Local S2-29a: raw binary80 representation and classification

`od-isa::x87::X87Binary80` is an immutable wrapper for ten bytes in memory
order. Its accessors preserve sign, exponent, the complete 64-bit significand,
explicit integer bit, fraction and Q bit exactly. `classify()` reports only
the raw encoding class: zero, subnormal, normal, infinity, quiet/signaling
NaN, pseudo-denormal, unnormal, pseudo-infinity, or quiet/signaling
pseudo-NaN. For E=0,J=1 the class is pseudo-denormal; for 0<E<0x7fff,J=0 it
is unnormal including zero fraction; for E=0x7fff,J=0 it is a pseudo-infinity
or pseudo-NaN. Q is significand bit 62 and J is bit 63. Classifying a value
does not read or change `X87Tag` or any `X87State` field. Payloads are never
quieted, normalized, rounded or rewritten.

Normative basis: Intel SDM revision 093, Vol. 1 §4.2.2/Table 4-3 and
§8.2.2/Table 8-3 ([253665-093US](https://cdrdv2.intel.com/v1/dl/getContent/671436)).
This raw representation and classification do not provide arithmetic,
normalization, rounding, exceptions or x87 instruction execution. S2-29 and
the S2-28 umbrella remain open.

## Local S2-29b: finite normalization and shift-jam helpers

`X87Binary80::normalize_finite()` accepts only raw zero, subnormal,
pseudo-denormal and normal classes. It returns sign, a 64-bit significand with
the leading one at bit 63 for nonzero values, the binary exponent associated
with that bit, and the original `X87Binary80Class`. Zero retains its sign and
has zero exponent/significand. A subnormal is left-shifted to bit 63 with the
exponent reduced by that shift. A pseudo-denormal uses the minimum normal
exponent but retains its `PseudoDenormal` source class. The raw 10 bytes and
classification remain unchanged. Infinity, NaNs, unnormal, and pseudo-special
encodings return `None`.

`shift_right_jam()` shifts an unsigned 64-bit integer right and ORs any
discarded one bits into the least significant bit; distances at or above 64
return 1 for nonzero input and 0 otherwise. `X87Grs::from_low_bits()` exposes
bits 2/1/0 as guard/round/sticky when a caller has reserved those positions.
These are integer helpers only: no rounding decision, arithmetic result,
status/exception mutation, or x87 instruction semantics are defined. They use
no host floating point. Manual result vectors and unit tests cover signed
zero, subnormal endpoints, pseudo-denormal provenance, normal/minimum and
maximum finite values, rejected classes, and shift boundaries/sticky state.
S2-29 and S2-28 remain open.

## Local S2-29c: 128-bit right-shift-jam and significand bridge

`shift_right_jam_u128(value, distance)` is a standalone logical right shift
that ORs all discarded one bits into bit zero. Distance zero returns the input
unchanged; distances from 1 through 127 perform the shift and jam; distances
at least 128 return one for nonzero input and zero for zero input. Repeated
right jams preserve an existing sticky one. A left shift cannot reconstruct
discarded information.

The exact bridge for a normalized 64-bit finite significand is
`(significand as u128) << 3`. Once shifted with jam, callers cast only the
low three bits to `X87Grs`; the retained significand is `aligned >> 3`.
Literal vectors cover zero, bit 127, `u128::MAX`, repeated sticky propagation,
all specified width boundaries through `u32::MAX`, and normalized finite
vectors retaining sign and source-class metadata. This helper adds no
rounding, arithmetic, exception/status behavior, instruction execution, or
precision claim. S2-29 and S2-28 remain open.

## Local S2-29d: ordered finite-pair alignment

`od_isa::x87::align_finite_pair(lhs, rhs)` returns
`(common_exponent, lhs_significand, rhs_significand)` and preserves operand
order. Two nonzero inputs use the larger exponent; a zero paired with a
nonzero value uses the nonzero exponent; two zeros use exponent zero. Zero
operands align to zero. Each nonzero significand is converted to
`(significand as u128) << 3` and passed to `shift_right_jam_u128` with the
nonnegative exponent difference. The helper does not modify input sign,
source class, or source values, and performs no magnitude sorting.

The low jammed bit indicates only a nonzero discarded tail. It is not an
exact magnitude comparison or architectural inexact status. A future
subtraction or left renormalization requires a separate proof that this
representation retains sufficient information. Literal unit vectors cover
the required gap boundaries, finite endpoints, pseudo-denormal/normal
provenance, signed zeros, negative exponents, and reversed input order. No
addition/subtraction result, normalization, rounding, precision control,
exception/status, special value, guest instruction, or native snapshot is
defined. S2-29 and S2-28 remain open.

## Local S2-29e: exact same-exponent finite sum

`od_isa::x87::add_finite_same_exponent(lhs, rhs)` returns an exact
`(exponent, coefficient)` pair when the exponents of nonzero finite normalized
inputs match. Its value is `coefficient * 2^(exponent - 63)`. Each operand
contributes its 64-bit significand as a signed `i128`; no GRS extension or
jammed alignment is used. The sum remains unnormalized, preserving carries,
cancellation and the exact signed coefficient, and does not carry a source
class. One zero contributes nothing and the other input selects the exponent;
two zeros return `(0, 0)` without selecting an architectural signed zero.
Unequal nonzero exponents return `None`.

Literal unit vectors cover positive/negative carry and maximum sums,
adjacent-significand cancellation to `+1` and `-1`, exact cancellation, negative
exponents, both zero operand orders, matching pseudo-denormal/normal inputs
with distinct source classes, unequal exponent rejection, and unchanged input
values and raw encoding. This primitive defines no jammed alignment,
binary80 packing, result normalization, unequal-exponent arithmetic,
rounding/PC/RC, signed-zero selection, exceptions/flags, or instruction
behavior. S2-29 and S2-28 remain open.

## Local S2-29f: exact coefficient normalization

`od_isa::x87::normalize_exact_coefficient(exponent, coefficient)` interprets
`coefficient * 2^(exponent - 63)`. For nonzero input it returns
`(sign, normalized_exponent, magnitude)`, where `magnitude` is the exact
unsigned coefficient shifted left until its leading one is at bit 127. The
normalized exponent is `exponent + (floor(log2(abs(coefficient))) - 63)`;
`None` means only that this final exponent overflows `i32`. The unsigned
magnitude operation handles `i128::MIN`. Zero returns `(false, 0, 0)` for any
input exponent and does not preserve a sign.

Literal vectors cover the signed and boundary coefficients, including
`2^64 + 1` low-bit preservation, exponent-edge success/overflow, and zero at
both exponent limits. Composition with S2-29e covers adjacent cancellation,
exact zero, maximum-finite sum with normalized exponent 16384, and adjacent
significand difference at the minimum-normal exponent; the inputs, raw values,
and source-class provenance stay unchanged. This integer helper consumes no
jammed alignment data and defines no binary80 packing, GRS/rounding, clipping,
exceptions/flags, guest instruction, or CPUID behavior. S2-29 and S2-28 remain
open.

## Local S2-29g: exact finite significand product

`od_isa::x87::multiply_finite_significands(lhs, rhs)` multiplies the original
finite normalized significands as unsigned `u128` operands. For nonzero inputs,
it returns `(sign, exponent, magnitude)` representing
`(-1)^sign * magnitude * 2^(exponent - 127)`, with
`sign = lhs.sign XOR rhs.sign` and `exponent = lhs.exponent + rhs.exponent + 1`.
The product remains exact;
its leading one may be at bit 126 or 127, and the result is not normalized or
narrowed through `i128`. `None` reports only checked `i32` exponent overflow.
Any zero input returns `(false, 0, 0)` with no architectural signed-zero
selection. The helper reads original significands directly, never consumes
jammed alignment, and leaves finite values, raw encodings and source-class
provenance unchanged.

Independent literal vectors cover minimum and maximum significands, mixed
boundary patterns, sign XOR and both operand orders, signed-zero pairs,
exponent underflow/overflow and a successful boundary sum, plus multiplication
of normalized raw finite values with pseudo-denormal/normal provenance. This
primitive does not normalize, round, pack binary80, set architectural
overflow/underflow or exception/status state, or execute guest instructions.
S2-29 and S2-28 remain open.

## Local S2-29h: exact unsigned magnitude normalization

`od_isa::x87::normalize_exact_magnitude(sign, exponent, magnitude)` interprets
`(-1)^sign * magnitude * 2^(exponent - 127)`. For nonzero input it returns
`(sign, normalized_exponent, normalized_magnitude)`, placing the magnitude's
leading one at bit 127 by shifting left only. The normalized exponent is the
input exponent minus that shift; `None` means only that this final exponent
does not fit in `i32`. Zero returns `(false, 0, 0)` for any sign and exponent.
The helper operates directly on `u128` and preserves all magnitude bits.

Literal vectors cover bit 126/127 normalization, `u128::MAX`, low-bit
preservation for `(2^64 + 1) << 63`, both signs, idempotence, zero at both
exponent limits, and `i32` minimum boundary success/failure for magnitudes 1
and `2^126`. Composition with S2-29g covers minimum-times-minimum,
maximum-times-maximum, and minimum-times-maximum while preserving inputs and
source-class provenance. This helper defines no division, rounding, GRS,
binary80 packing, architectural flags/exceptions, guest instruction, or native
execution behavior. S2-29 and S2-28 remain open.

## Local S2-29i: exact normalized finite quotient and remainder

`od_isa::x87::divide_finite_significands(lhs, rhs)` returns an
`X87FiniteQuotient` for finite normalized significands. For nonzero operands,
it represents
`(-1)^sign * (quotient + remainder / denominator) * 2^(exponent - 63)`.
With significands `a` and `d`, the helper uses `k = 63` if `a >= d`, else
`k = 64`, computes `D = (a as u128) << k`, and returns the integer quotient
and exact remainder of `D / d`. The original denominator is retained,
`2^63 <= quotient < 2^64`, `remainder < denominator`, and exponent is
`lhs.exponent - rhs.exponent + (63 - k)` if it fits in `i32`. Result sign is
the input sign XOR. No source values or provenance are modified.

A zero divisor returns `None`, including 0/0. A zero numerator with a
nonzero divisor produces canonical unsigned zero fields while retaining the
denominator; this does not decide architectural signed zero or define guest
divide-by-zero delivery. Literal vectors cover normalized quotient and
remainder boundaries, both sign combinations, zero precedence, raw subnormal
and pseudo-denormal composition, input preservation, and synthetic exponent
conversion limits. This primitive uses no alignment, jam, GRS, rounding,
binary80 packing, architectural flags/exceptions, guest instruction, or native
execution behavior. S2-29 and S2-28 remain open.

## Local S2-29j: pure unsigned rational nearest-integer ties-to-even

`od_isa::x87::round_quotient_ties_even(q, r, d)` rounds `q + r / d` to the
nearest integer with ties to even. It returns `None` for `d == 0` or `r >= d`.
The remainder is widened before doubling for the half comparison, and the
rounded result is `u128`, preserving a carry from `u64::MAX` to `2^64`.

Literal tests cover below/at/above-half cases, both quotient parities at an
exact tie, exact integers, invalid inputs, large remainders, and the `u64::MAX`
carry. Composition tests consume S2-29i divider vectors. Tie and carry cases
are synthetic generic rational inputs because the normalized S2-29i divider
cannot produce them. This helper excludes x87 RC/PC, binary80 formatting or
packing, guest FDIV, signs, exponent adjustment, architectural flags, and
signed zero; it does not implement x87 instruction or control-word semantics.
S2-29 and S2-28 remain open.

## Local S2-29k: pure mathematical rational rounding modes

`od_isa::x87::round_quotient_magnitude(sign, q, r, d, mode)` rounds the exact
coefficient `(-1)^sign * (q + r/d)` on the integer lattice and returns its
unsigned magnitude. `RationalRoundingMode` provides nearest with ties to even,
toward zero, toward positive, and toward negative. The sign affects only the
increment decision for directed modes. The helper returns `None` only when
`d == 0` or `r >= d`; exact integers return `q` in every mode. Its widened
half comparison safely handles all `u64` inputs, and `u128` preserves a carry
from `u64::MAX` to `2^64`. `round_quotient_ties_even` remains a positive-sign
nearest-even compatibility wrapper.

Literal vectors cover each mode with positive and negative fractions, nearest
ties with even and odd quotients, small fractions around zero, exact integers,
invalid inputs in every mode, the widened-half boundary, and carry beyond
`u64::MAX`. Composition vectors pass S2-29i quotient, remainder, denominator,
and sign getters for positive and negative divisions. Synthetic tie and carry
vectors cannot arise from normalized S2-29i divider outputs. This helper does
not define control-word values, a precision parameter, exponent changes,
signed zero, x87 RC/PC, binary80 packing, status effects, or guest instruction
semantics. S2-29 and S2-28 remain open.

## Local S2-29l: direct exact quotient rounding to significant precision

`od_isa::x87::round_quotient_to_precision(sign, q, r, d, precision, mode)`
rounds `(-1)^sign * (q + r/d)` directly to 1–64 significant bits. Valid input
requires `d != 0`, `r < d`, and either `q == 0 && r == 0` or a nonzero
normalized quotient with bit 63 set. Invalid precision or quotient metadata
returns `None`; exact zero returns zero for every valid precision and mode.

For `k = 64 - precision`, it retains `t = q >> k` and examines
`lo = q - (t << k)` together with the exact remainder. Nearest mode compares
that combined tail with half an ulp, including ties-to-even, without multiplying
or rounding an intermediate value. Directed modes increment only when the
discarded tail is nonzero and the direction/sign require it. The returned
`u128` preserves a carry to `2^precision` without changing an exponent. Given
S2-29i exponent `e`, the rounded magnitude represents
`(-1)^sign * magnitude * 2^(e + 1 - precision)`; exponent adjustment is a
caller responsibility.

Literal vectors cover all modes and signs, precision endpoints and reduced
precisions, exact zero, invalid p/d/r/q inputs, tie parity, directed rounding
of discarded quotient bits and rational remainder, `u64::MAX` denominator
halfway boundaries, and a direct-rounding vector that distinguishes the
result from double rounding. p=64 synthetic tie/carry inputs are unreachable
from normalized divider output; ties at reduced precision remain valid cases.
Composition vectors use the exact 1/3 S2-29i quotient/remainder at p=24, 53,
and 64 and check negative directed modes. This is a mathematical helper only:
it does not implement x87 PC/RC/control-word semantics, exponent normalization
or adjustment, subnormal/range handling, binary80 packing, architectural
flags/exceptions, signed-zero selection, or guest FDIV. S2-29 and S2-28 remain
open.

## Local S2-29m: bounded normal finite binary80 packing

`od_isa::x87::pack_normal_rounded_significand(sign, exponent, magnitude,
precision)` packs an already-rounded nonzero value
`(-1)^sign * magnitude * 2^(exponent + 1 - precision)` for precision 1–64.
The magnitude must be in `[2^(precision-1), 2^precision]`. For a magnitude
below `2^precision`, packing shifts it exactly so its leading bit occupies the
explicit binary80 integer bit and keeps the input exponent. At the carry value
`2^precision`, packing uses significand `0x8000000000000000` and increments the
exponent exactly once. The final exponent must be `[-16382, 16383]`; bytes are
the little-endian 64-bit significand followed by the little-endian sign and
biased-exponent word. Packing performs no rounding. `None` denotes only an
invalid bounded input or unsupported normal exponent range.

Literal vectors cover p=1/2/24/53/63/64, signs, carry, minimum normal and carry
into it, and maximum finite. Composition uses exact S2-29i 1/3 quotient results
at p=24/53/64 and negative p=24 directed-rounding values. S2-29n adds bounded
numeric subnormal packing; special encodings, architectural overflow/underflow,
x87 PC/RC, flags/exceptions, signed-zero operation rules, and guest instruction
behavior remain open under S2-29/S2-28 and ISSUE-019.

## Local S2-29n: direct exact quotient-to-subnormal rounding and packing

`od_isa::x87::round_pack_subnormal_quotient(sign, e, q, r, d, mode)` consumes
the exact nonzero value `(-1)^sign * (q + r/d) * 2^(e - 63)`. Its bounded input
requires `q >= 2^63`, `d > 0`, `r < d`, and `e <= -16383`. It computes
`k = -16382 - e` in a widened signed integer. For `1 <= k <= 63`, it rounds
the original quotient and rational remainder directly once at precision
`p = 64-k`. At `k=64`, it rounds against the exact half-minimum-subnormal
boundary; for `k>=65`, nearest and toward-zero produce zero while a directed
mode away from zero produces one minimum-subnormal unit. Magnitudes below
`2^63` encode with exponent field zero; `2^63` encodes as minimum normal. The
input sign bit is kept for rounded zero. `None` denotes only an invalid input
domain.

Literal byte vectors cover positive and negative signs, all four mathematical
rounding modes, exact subnormal values, half the minimum subnormal, `k=1/63/64/65`,
minimum and maximum subnormal, rounding to minimum normal, `i32::MIN`, invalid
domain inputs, and a direct-rounding case that differs from double rounding.
Composition uses exact S2-29i divider getter results for minimum normal divided
by two and by three.

This helper provides numerical finite normal/subnormal rounding and encoding
only. It does not implement x87 RC/PC or control-word semantics, architectural
underflow/precision flags or exceptions, special encodings, signed-zero
operation rules, or guest FDIV. S2-29 and S2-28 remain open under ISSUE-019.

## Источники архитектурной семантики

- Intel, [Intel 64 and IA-32 Architectures Software Developer Manuals](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html), версия 093 на дату документа: базовая архитектура и instruction reference. GPR/subregister правила сверены с [Volume 1](https://cdrdv2.intel.com/v1/dl/getContent/671436), §3.4.1.1. Для реализации сверять конкретную инструкцию и выбранный режим по manual.
- AMD, [AMD64 Architecture Programmer’s Manual, Volumes 1–5](https://docs.amd.com/v/u/en-US/40332_4.09_APM_PUB), публикация 4.09: дополнительный нормативный источник. Тело manual через web fetch этой сессии не прочитано: [ISSUE-011](../KNOWN_ISSUES.md#issue-011--amd-apm-409-не-прочитан-через-web-fetch). Поисковые выдержки не подтверждают AMD semantics; расхождения Intel/AMD нужно проверить и фиксировать до расширения общего профиля.
- Linux, [x86 boot protocol](https://docs.kernel.org/arch/x86/boot.html), §1.14 «64-bit Boot Protocol»: требования выбранного прямого 64-bit entry; exact header/version/xloadflags candidate image статически проверены в [Linux profile report](../reports/STAGE_2_LINUX_PROFILE.md), boot ещё не выполнен.
- [План S0-09](../plans/STAGE_0.md#s0-09-реализовать-ограниченный-сквозной-x86--arm64-прототип) задаёт границу вертикального среза и требования к сравнению interpreter/JIT.


S2-17n locally covers register PUSH/POP opcodes `50–5F` with `66` plus an
effective REX prefix. Intel SDM revision 093 Vol.2A §2.2.1.2 pp.2-8–2-9
specifies that REX.W selects qword despite 66H and that only the final REX before
the opcode is effective. Six fixtures cover word/qword PUSH and POP, REX.B
register extensions, repeated REX ordering, and word POP upper-bit preservation.
Local full-state and native-admission checks are included; physical verification
remains deferred to the shared final mini acceptance. The pool was 550 fixtures
at S2-17n completion.

S2-17o covers only bare long-mode `POPFQ` (`9D`) in the runner's default
CPL3/IOPL0 execution contract. It updates CF/PF/AF/ZF/SF/DF/OF, preserving bit 1
and all other RFLAGS bits in the interpreter projection; IF/IOPL, RF and ID
preservation are tested outside the literal v2 snapshot mask. Source or current TF, NT, or AC
returns `Unsupported` before CPU/RIP/RSP/RFLAGS commit and writes no memory; a
stack read fault also does not commit CPU or flag-definedness. A read may still
appear in the backend access trace and is not guest exception delivery. Two
literal fixtures and a fixture-driven TF/AC no-commit check are local; physical
acceptance remains deferred. `AccessContext` models CPL but does not select IOPL,
so other POPF privilege combinations remain open. S2-17p implements only bare
long-mode `PUSHFQ` (`9C`) with the SDM093 qword image mask
`00000000_00FCFFFFH`; RF and VM are cleared in the stored image and RFLAGS is
unchanged. The local literal uses RFLAGS `0xCD7`. Its physical memory comparison
checks the modeled flags and the cleared RF/VM bits while masking IF/IOPL and
other unmodeled fields. Other PUSHF widths and prefix variants remain open except the exact S2-17q `66 9C` word form.
S2-17r adds only long-mode `66 9D` POPFW for CPL3/IOPL0: CF/PF/AF/ZF/SF/DF/OF are loaded from the word; IF, IOPL, bit 1, and all other unmodeled bits are preserved. Source or current TF=1 is Unsupported. Nonzero current IOPL returns Unsupported before stack access. NT=1 is also Unsupported for source and initial state because SDM 093 Vol.2B Table 4-15 pp.4-395–4-396 conflicts with Vol.1 §7.3.13.2 p.7-22; see [ISSUE-015](../KNOWN_ISSUES.md#issue-015--разногласие-sdm-по-nt-для-popf-word-на-cpl3iopl0). The literal v2 fixture compares RFLAGS only through mask `0xCD7`; a separate local test asserts exact preservation of unmasked IF, reserved, ID and upper bits while the source IOPL is 3 and current IOPL is 0. Native and interpreter admission reject nonzero current IOPL. Physical checking remains deferred to shared acceptance. S2-17s adds `66 48 9C` / `66 48 9D`: effective REX.W selects qword despite 66H, while `66` with effective REX.W=0 remains word. Its fixture checks PUSHFQ's SDM093 qword image and POPFQ with TF/NT/IOPL zero; local assertions verify the entire 8-byte stack write, canaries, unchanged RFLAGS, final RIP and full state. Other prefix combinations remain open. S2-17t covers the normalized prefix state for `66 C3` and `48 66 C3`: Vol.1 §6.4.7 forces near-branch operation-size effects to 64 bits in 64-bit mode and silently ignores operand-size prefixes; Vol.2A §2.2.1 says a REX byte is ignored unless it immediately precedes the opcode or 0F, so the leading 48 is ineffective before 66. Both literal fixtures check qword target/pop semantics and full state. Effective REX (`66 48 C3`), duplicate 66, and other effective prefix states remain outside this slice. Physical acceptance is deferred.
The shared pool is 558.
