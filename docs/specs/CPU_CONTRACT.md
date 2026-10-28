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

Никакой полный CPU capability profile для запуска Windows/Linux пока не выбран. Для целевого Ubuntu Generic `amd64` принят лишь ABI/ISA floor: нормативная интерпретация — baseline x86-64 psABI, то есть x86-64-v1; v2/v3/v4 не принимаются. Это решение не утверждает полный CPU/CPUID/MSR/OS boot profile и не одобряет конкретные значения CPUID или MSR. Синтетические vendor/family/model, CPUID mask и MSR allowlist остаются candidate/unresolved до exact-source/binary gate. Таблицы Windows minimums и reference VM в [GUEST_REQUIREMENTS.md](GUEST_REQUIREMENTS.md) и статический Ubuntu config не подменяют анализ конкретного boot path и проверку исполнения. См. [S2-00 Linux profile evidence](../reports/STAGE_2_LINUX_PROFILE.md#решение-по-generic-amd64-baseline).

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
| MOVS/CMPS/STOS/LODS/SCAS A4–AF; REP/REPE/REPNE; CLD/STD | long / 8–64 / 32,64 | baseline | S2-18a locally implements bare `A4` MOVSB; S2-18b adds normalized `66 A5` MOVSW; S2-18c adds bare `A5` MOVSD (default dword width); S2-18d adds effective `REX.W + A5` MOVSQ; S2-18e adds bare `AA` STOSB; S2-18g adds non-REP `66 AB` STOSW, bare `AB` STOSD, and effective REX.W + `AB` STOSQ; S2-18f adds bounded non-REP LODS `AC`/`AD` byte/word/dword/qword forms; **S2-18h adds bounded non-REP SCAS `AE`/`AF` byte/word/dword/qword forms; S2-18j adds only bare CLD `FC` and STD `FD` with DF-only flag effects**; S2-18o adds bounded non-REP address-size-32 MOVSB/W/D/Q with one source read, one destination write, and success-only ESI/EDI modulo-32 commit; its local source-first/fault policies remain ISSUE-016/021. S2-19a adds exact `F3 AA` REP STOSB; S2-19b extends the shared REP STOS operation to `F3 66 AB` word, `F3 AB` dword and effective REX.W `AB` qword, while byte remains exact-only. S2-19c adds exact `F3 AC` REP LODSB and F3/optional-66/effective-REX.W `AD` REP LODSW/D/Q; zero count avoids RSI access and successful elements commit RAX/RSI/RCX before the next STOP/limit boundary. S2-19d adds exact `F3 A4/A5` REP MOVSB/W/D/Q with source-first read then destination write and RSI/RDI/RCX commit only after both succeed; its local dual-invalid ordering is tracked separately. MOVS uses implicit flat `[RSI]` read then `[RDI]` write and updates both pointers only after both accesses succeed, by ±1/±2/±4/±8 according to DF. STOSB writes AL to flat `[RDI]`, updates only RDI by ±1 after a successful store, and preserves RSI and flags. LODS performs one read from flat `DS:[RSI]`; AL/AX preserve upper RAX, EAX zero-extends, and RAX replaces all bits; RSI wraps by ±width on successful read. LODS preserves RCX, RDI, flags/definedness and all other state. SCAS reads one width-sized value from flat `ES:[RDI]`, compares AL/AX/EAX/RAX against it using CMP flag semantics, and adjusts only RDI by ±width after a successful read. It preserves the complete accumulator, RSI, RCX, DF and other non-arithmetic state; successful retirement defines CF/PF/AF/ZF/SF/OF. Its exact admitted forms are bare `AE`, bare `AF`, `66 AF`, and effective REX.W + `AF` (66 may also be present); unsupported encodings are admission failures and do not assert #UD. Its local fault contract is atomic for the current SCAS operation; prior instructions already retired remain committed. LODS exact prefixes are bare `AC`, bare `AD`, `66 AD`, and effective REX.W + `AD` (66 may also be present); unsupported encodings are admission failures and do not assert #UD. REP STOSB with RCX=0 performs no destination access; each successful store updates RDI by ±1 and decrements RCX, with flags unchanged. One instruction invocation processes at most 1024 successful iterations; this work limit is distinct from instruction retirement budget. STOP or work-limit exhaustion between iterations preserves partial state and leaves RIP at `F3 AA`; a fault preserves earlier iterations and leaves current iteration RDI/RCX unchanged. This is the local execution contract, not a general claim about exception priority. MOVSW accepts ignored misplaced REX before 66 (`48 66 A5`); effective REX.W overrides 66 (`66 48 A5` is MOVSQ). For the address-size-64 MOVS forms in this row, segment and address-size prefixes remain unsupported except for the separately bounded non-REP address-size-32 MOVS forms in S2-18o. Literal cases cover DF=0/1, zero count, aliases/canaries, and injected MOVS read/write or STOSB destination #PF; LODS and SCAS tests include full 6,144-state matrices and complete-state/definedness checks. A missing bounded-memory mapping remains a harness error; it is not converted to a guest exception. Bare non-REP MOVS destination-fault index/RIP remains unconfirmed (ISSUE-016); LODS/SCAS read faults commit no state for the faulting instruction. Physical checks are deferred to shared mini acceptance. | B; Vol.2 entries, DF и частичный прогресс |
| XADD/CMPXCHG/CMPXCHG8B/16B; LOCK; fences | long / по форме / 32,64 | CX8/CX16/SSE2 по форме | S2-21a bare qword memory XCHG; S2-21b bare qword register XADD; S2-21c bare qword register CMPXCHG; S2-21d bare qword memory XADD; S2-21e bare qword memory CMPXCHG; S2-21f LOCK qword memory XADD only under serial/private synchronous RAM precondition; S2-21g bare MFENCE on explicitly opted-in private synchronous RAM. Other widths, LOCK forms and fence forms remain open. SMP/shared-memory ordering не заявляется | B и уточняемый Linux; Vol.2 entries |
| Segments, CR0/2/3/4, EFER, MSR, CPUID, descriptor/event/system instructions | long и требуемые transitions / по форме | Только завершённые feature groups | Открыто S2-22…27,41 | C; Vol.3 memory/protection/events; machine paging/events — этап 4 |
| x87, MMX, SSE/SSE2 и save/restore | по форме | FPU/MMX/FXSR/SSE/SSE2 и OS state | Открыто S2-28…39 | D; Vol.1 chapters x87/MMX/SSE и Vol.2 entries |
| Прочие расширения Linux/Windows | Не определено до исследования image/config | Не рекламировать заранее | Открыто S2-40 | Уточнить по реальному kernel/profile; возраст oracle CPU не основание исключения |

### Local S2-18g STOSW/D/Q contract

The bounded interpreter accepts `66 AB` (word), bare `AB` (dword), and
effective `REX.W + AB` (qword) in Long64 with address-size 64 and implicit flat
`ES:[RDI]`. REX.W overrides 66; a REX byte before a later legacy prefix is
ignored by the shared normalizer (`48 66 AB` is word; `66 48 AB` is qword). All
`48..4F` REX.W values are accepted. Group1 prefixes, `67`, segment overrides,
effective non-W REX, and REP/LOCK are outside the profile.

Each success performs one width-sized all-or-error write of low RAX at the
pre-instruction RDI, then wraps RDI by ±width according to DF, advances RIP, and
retires once. RAX, RSI, RCX, and RFLAGS remain unchanged. RSI is never accessed or
validated. A noncanonical destination returns #GP(0); bounded-memory and
backend errors remain harness errors unless the backend explicitly reports an
architectural fault. Failure commits neither CPU state nor RAM. No LOCK/SMP
atomicity is claimed; JIT/native and physical acceptance remain deferred.

### Local S2-18h SCAS B/W/D/Q contract

The bounded Long64 interpreter accepts bare `AE` SCASB, bare `AF` SCASD,
`66 AF` SCASW, and effective `REX.W + AF` SCASQ with address-size 64 and
implicit flat `ES:[RDI]`. All `48..4F` REX.W forms are accepted. Effective
REX.W overrides 66 (`66 48 AF` is qword); a REX before a later 66 is
normalized away (`48 66 AF` is word). REP, LOCK, 67, every segment override,
effective non-W REX on `AF`, and all prefixed `AE` forms are outside the profile.

SCAS performs one read of 1/2/4/8 bytes from pre-state RDI, compares the
pre-state AL/AX/EAX/RAX with the memory value using the existing CMP result
and flag rules, and commits arithmetic flags plus the DF-directed wrapping
RDI update only after the read succeeds. It preserves all of RAX (including
the upper half after SCASD), RSI, RCX, RAM and DF. Successful retirement
advances RIP and defines `ARITHMETIC_FLAGS`; undefined flags are zero. A
noncanonical source returns #GP(0); missing mappings, permissions and backend
errors remain harness errors unless the backend returns an architectural
fault. The faulting SCAS instruction commits no CPU or memory state; any earlier
retired instruction remains committed. JIT/native admission is closed.

Interpreter checks include 6,144 full-state comparisons with independent flag
calculation, one read/no writes, all alignment residues, page/range boundaries,
wrapping and noncanonical post-index values, ignored RSI values, prefix/REX
normalization, atomic faults, and TEST→SCAS definedness. Exactly 24 literal
schema-v2 fixtures cover width/direction successes, unmapped and noncanonical
sources, zero budget, normalization, and TEST composition success/fault. The
generator serializes predetermined input/output literals only. ISSUE-016 and
ISSUE-019 remain open; physical execution is deferred to the shared final
acceptance substage.

### Local S2-18i CMPS B/W/D/Q contract

The bounded Long64 profile accepts bare `A6` CMPSB, `66 A7` CMPSW, bare
`A7` CMPSD, and effective `REX.W + A7` CMPSQ with address-size 64. All
`48..4F` REX.W bytes are accepted; effective REX.W overrides 66, while a REX
before a later 66 is normalized away (`48 66 A7` is word; `66 48 A7` is
qword). CMPSB is bare-only. REP, LOCK, 67, segment overrides and effective
non-W REX on A7 are outside this profile.

Each successful instruction performs two width-sized reads using pre-state
addresses in strict order: flat DS:[RSI], then flat ES:[RDI]. It computes
`memory[RSI] - memory[RDI]` with CMP flags, then commits only arithmetic flags,
DF-directed wrapping RSI/RDI, RIP and one retirement. RAX, RCX and RAM are
unchanged. RSI=RDI still performs two reads. Successful retirement defines
`ARITHMETIC_FLAGS`; failed reads define no additional flags and commit no CPU
state. A noncanonical start or final byte in either operand produces #GP(0).
Missing mapping, permission and backend errors remain harness errors unless the
backend explicitly reports an architectural fault; no #PF is synthesized.

If the first access fails, there is no second access. If the second fails, the
first read remains observable in the local trace while CPU commit is withheld.
The selected RSI-first order is bounded local policy and is not hardware
priority evidence for dual-invalid operands. No MMIO or side-effect rollback is
claimed. Dual-invalid winner remains open as ISSUE-020. Exactly 33 literal
schema-v2 fixtures preserve the existing schema/profile; the generator only
serializes stated inputs and expected full states. Local full-state coverage,
read-order/failure matrix, prefix boundary and address tests do not establish
native/JIT or physical x86 behavior. Mac mini/SSH/native/physical acceptance
remains part of the single final substage.

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
ADD/CMP/Jcc/LOCK, и [Vol.2B](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf),
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
[Vol.2B](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf) MOV pp.4-28–4-31,
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
[официальный Vol.2B PDF](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf).

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
Vol.2B MOV pp.4-28–4-31, (253667-093US), [официальный Vol.2B PDF](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf).

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
to r64), [официальный Vol.2B PDF](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf).

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
ModR/M E8 selects GS and RAX), [официальный Vol.2B PDF](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf).

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
[official PDF](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf); Vol.3A
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

### S2-22k — Long64/CPL0 null DS and ES loads

The separate local-only `s2-long-null-ds-es-v1` profile admits bare `8E D8`
(`MOV DS, AX`) and `8E C0` (`MOV ES, AX`) only when AX is zero. Its exact
allowlist also includes a repeated DS load and the two-instruction DS→ES
composition fixture. Only each visible 16-bit selector and its modeled
unusable bit change; GPRs, RFLAGS and other segment state are preserved, and
RIP/retirement advance after each successful instruction. The low AX is read,
so nonzero high RAX bits do not make a null selector nonzero.

No GDTR is required or read. The null load does not read GDT[0], create a
descriptor cache, or perform accessed-bit writes. DS/ES base and effective
segmentation rules remain outside this profile; it adds no memory forms. The
profile implies CPL0 and rejects nonzero AX and prefixed/neighboring forms
without commit. The schema requires non-null boolean unusable-state fields for
this profile and rejects those fields, including explicit `null`, in older
profiles. Native and JIT execution remain unsupported. Four literal fixtures
check DS, ES, repeated DS, and DS→ES; unit tests also check complete
modeled state, flags-definedness, no descriptor/data callback, high RAX,
rejected forms, CPL3 and truncated fetch. This local slice does not establish
an executable Ubuntu startup prefix or close S2-22.

Source: Intel SDM revision 093 (253667-093US), Vol.2B `MOV` pp.4-28–4-31,
[official Vol.2B PDF](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf); Vol.3A
(325384-093US), §§3.4.3–3.5.1,
[official combined PDF](https://cdrdv2.intel.com/v1/dl/getContent/671447).
These exact revision links were consulted for MOV null-selector rules and
Long64 segment behavior; S2-22k claims no broader exception or memory rules.


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
INC pp.3-457–3-458; [Vol.2B](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf)
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
AND pp.3-60–3-61; [Vol.2B](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf)
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

Verified Intel SDM revision 093 references: [Vol.2B](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf)
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

### Local UD2 fetch boundary — S2-20d

For bare `0F 0B`, this local contract reports `#UD` only after successful
fetch callbacks for both opcode bytes. A complete instruction at the end of
an executable `FlatMemory` region performs exactly two fetches; two adjacent
executable regions produce the same result. A missing byte or an injected
second-fetch error remains the original backend/architectural memory error,
including its class and payload. A first-fetch failure does not decode or
commit CPU state. All these exits retire zero instructions and preserve the
full state and RAM; the instruction performs no data accesses.

These statements describe only the local FlatMemory/callback boundary. They
make no claim about physical instruction fetch, hardware `#UD` priority,
general fault priority, other invalid encodings, trap delivery, MMU behavior,
or Native/JIT execution. The complete and split-region CLI fixtures remain
local-only. See the [S2-20d report](../reports/STAGE_2_REPORT.md#s2-20d--complete-ud2-fetch-boundary-in-local-memory-callbacks).

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

The local fixture profile admits one exact instruction (`90` NOP, S2-30a
`DB E3` FNINIT, S2-30b `DF E0` FNSTSW AX, or an explicitly listed S2-30 x87
arithmetic encoding) with budget 1 and a stop RIP
immediately afterward. Fixture
conversion loads the supplied x87 object into `LongModeState`, and result
conversion observes that executed state; comparisons inspect the entire
object. The fragment adapter explicitly rejects `Some(x87)`. Native x87
observation is unavailable, so native execution rejects this profile; no FPU
CPUID feature or general x87 support is claimed. Remaining x87 semantics and
integration stay open under S2-28–S2-39.

## Local profile S2-34.1: `s2-long-x87-sse-state-v1`

This local-only schema-v2 profile combines the complete existing raw x87
snapshot with an independent optional `SseState` carried by `LongModeState`.
The SSE object contains 16 XMM registers, each exactly 16 bytes serialized as
32 lowercase hexadecimal digits in increasing memory-address order, and raw
MXCSR as a u32. It transfers state through exactly one `90` NOP with budget 1
and a stop RIP immediately after the instruction. Result comparison observes
all XMM bytes and MXCSR; the profile's x87 fields remain required and compared.

The fixture's `diagnostic_mxcsr_mask` is fixed to `0x0000ffbf`. This is bounded
profile metadata for diagnostic fixture coverage, with optional DAZ excluded
from the default mask. It is not a universal hardware CPUID mask and does not
define guest `#GP` behavior. The core state stores raw MXCSR without masking or
interpreting its bits. Older profiles omit the `sse` object and reject it when
present. The C fragment adapter fails closed on `Some(sse)`; native admission
also rejects this local-only profile.

This establishes only lossless local snapshot/state preservation. It adds no
SSE instruction execution, LDMXCSR/STMXCSR, FXSAVE layout, CR0/CR4 gates,
`#NM`/`#XM`, XSAVE/AVX state, native/JIT admission, or physical verification.
S2-34, S2-38 and S2-39 remain open.

## Local profile S2-34.2: `s2-long-movups-reg-v1`

This separate schema-v2 local profile requires the S2-34.1 x87 and raw SSE
snapshots and admits exactly one legacy register-to-register MOVUPS in either
direction, with ModR/M.mod=3. `0F 10 /r` selects the destination from
ModR/M.reg and the source from ModR/M.r/m. `0F 11 /r` is the reverse encoding:
ModR/M.reg selects the source and ModR/M.r/m selects the destination. A bare
encoding or one effective REX byte immediately before the opcode map is
accepted. REX.R extends ModR/M.reg and REX.B extends ModR/M.r/m through XMM15;
for example `0F 11 C1` copies XMM0 to XMM1 and `45 0F 11 C1` copies XMM8 to
XMM9. This profile excludes REX.W/X, `66`, F2/F3, segment and address
overrides, LOCK, duplicate/ignored prefix variants, memory forms, MOVUPD/MOVSS/
MOVSD, VEX and EVEX. The forms are checked against Intel SDM revision 093,
Vol. 2B, MOVUPS entries `0F 10 /r` and `0F 11 /r`; the latter is Op/En B
(`xmm2/m128, xmm1`), with `xmm1` as source and `xmm2/m128` as destination.
Intel's [official SDM download page](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html)
lists the downloadable manuals at version 093.

Execution assumes SSE is available, CR0.EM=0, CR0.TS=0 and CR4.OSFXSR=1. These
are explicit diagnostic-profile preconditions, not modeled control-register
state or runtime checks. `#UD`, `#NM` and control faults are not modeled. On
success, the source's 128 raw bits replace the destination's 128 raw bits,
including when both register indices are equal; neither encoding accesses data
memory when ModR/M.mod=3. x87, raw MXCSR, GPR and RFLAGS are preserved. RIP and
retirement advance only on success. This profile does
not model memory/alignment semantics, MXCSR effects, floating-point behavior,
SSE exceptions, native/JIT support or physical execution. Native admission
fails closed and no JIT path exists. The S2-34.1 state-only profile remains
NOP-only; S2-34/D/System profile remain open.

## Local profile S2-34.4: `s2-long-movups-mem-load-v1`

This is a separate schema-v2 diagnostic profile. It retains both x87 and raw
SSE snapshots and admits exactly one legacy `0F 10 /r` memory-source form:
ModR/M.mod=00 and r/m=000, so the source is `[RAX]` with no SIB or displacement.
The destination is ModR/M.reg; one effective REX.R may extend it to XMM8–XMM15.
Bare encoding also selects XMM0–XMM7. REX.B/X/W are excluded. The state-only
`s2-long-x87-sse-state-v1` remains NOP-only, and register-only
`s2-long-movups-reg-v1` retains only its two register forms. This opcode and
operand direction follows Intel SDM revision 093, Vol. 2B, MOVUPS `0F 10 /r`,
Op/En RM (`xmm1` destination from `xmm2/m128`); Intel's [official SDM page](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html)
lists the downloadable manual set at version 093.

The profile assumes SSE is available, CR0.EM=0, CR0.TS=0 and CR4.OSFXSR=1.
It does not model these controls or their faults, nor `#UD`/`#NM`. For execution,
the interpreter checks that all 16 bytes from RAX through RAX+15 are canonical
in its canonical48 diagnostic model, then calls `GuestMemory::read` exactly
once with a 16-byte temporary. The width is not represented as
`OperandSize::Qword`; the path validates the 16-byte range directly. This
unaligned instruction has no alignment-fault behavior in this slice. After a
successful read, all bytes replace the selected XMM register in increasing
memory-address order. x87, MXCSR, source memory, GPR and RFLAGS remain unchanged;
RIP advances and one instruction retires only on success. Local injected
backend/fetch faults test no-commit behavior but do not establish hardware
fault priority or restart state.

The profile rejects stores, other bases/indexes, SIB/displacement, segment or
address overrides, `66`/F2/F3/LOCK/duplicate prefixes, VEX/EVEX, MXCSR/control
operations, native/JIT execution and partial stores. The literal fixture and
local tests cover unaligned source bytes, all 16 destinations, source/memory
canaries, one 16-byte read, canonical boundaries and fail-closed variants.
Choosing Generic amd64 baseline does not make this one form a complete or
validated generic CPU profile. No physical execution is claimed; S2-34/D/System
remain open.

## Local profile S2-34.5: `s2-long-movups-mem-store-v1`

This separate schema-v2 diagnostic profile requires the existing x87 and raw
SSE snapshots and admits exactly one legacy memory-destination MOVUPS:
`0F 11 /r`, ModR/M.mod=00 and r/m=000, addressing `[RAX]` with no SIB or
displacement. ModR/M.reg with optional effective REX.R selects source XMM0–15.
Only absent REX or one immediately preceding REX with REX.R as the only
extension bit is allowed; REX.B/X/W, `0F 10`, other bases, SIB, displacement,
legacy segment/address/operand/repeat/LOCK prefixes, duplicate prefixes/REX,
VEX and EVEX remain rejected. The profile is separate from and does not extend
the state-only, register MOVUPS, or memory-load profile contracts.

Intel SDM revision 093, Vol. 2B, MOVUPS `NP 0F 11 /r`, Op/En B, specifies
`MOVUPS xmm2/m128, xmm1`, where `xmm1` is the source and `xmm2/m128` the
destination. The SDM states that a memory operand may be unaligned without
generating #GP and lists no SIMD floating-point exceptions; other exceptions
are delegated to Type 4 and note that #AC treatment varies. This local profile
assumes SSE availability, CR0.EM=0, CR0.TS=0 and CR4.OSFXSR=1, but does not
model control gates, #UD/#NM, #AC, segment/page faults, or architectural fault
priority/restart state. Intel's [official SDM page](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html)
lists downloadable manuals at revision 093; the [Vol. 2B, revision 093 manual](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf)
contains the MOVUPS entry.

Execution validates the full 16-byte canonical48 range, copies the selected
XMM's raw `[u8; 16]` into a temporary before backend access, then issues exactly
one `GuestMemory::write` with 16 bytes. It issues no data read and does not
represent the width as `OperandSize::Qword`. RIP and retirement commit only
after a successful write; backend error leaves CPU state unchanged. FlatMemory
tests its own private synchronous fixture-RAM all-or-error policy for failed
cross-region, read-only, and injected writes. This backend policy claims no
hardware 16-byte atomicity and makes no MMIO or SMP guarantee. The upper-u64
range overflow limitation remains ISSUE-012. Native/JIT admission is closed.
The x86-64-v1 selection is only an ABI floor; this slice does not complete a
generic CPU profile or S2-34/D/System. See the
[S2-34.5 plan](../plans/STAGE_2.md#s2-345--local-only-legacy-movups-rax-xmm).

## Local profile S2-34.7a: `s2-long-xorps-reg-v1`

This separate schema-v2 profile requires the S2-34.1 raw x87/SSE snapshots
and admits one legacy register-only `XORPS` instruction: `0F 57 /r` with
ModR/M.mod=3. ModR/M.reg selects the destination and ModR/M.r/m selects the
source. A bare instruction or one effective REX byte is allowed; REX.R and
REX.B extend those fields to XMM0–XMM15. REX.W/X, all legacy prefixes,
duplicate/ignored prefixes, memory operands, VEX and EVEX are outside the
profile. The encoding and operand direction follow Intel SDM revision 093,
Vol. 2B, XORPS entry `NP 0F 57 /r`, Op/En RM; the official [Intel SDM page]
(https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html)
lists revision 093 downloads.

The operation XORs each of the 16 raw destination bytes with the corresponding
source byte. It performs no host floating-point conversion and changes no
MXCSR, x87, GPR, RFLAGS or other XMM state. SSE availability, CR0.EM=0,
CR0.TS=0 and CR4.OSFXSR=1 are diagnostic-profile preconditions; control-register
gates and `#UD`/`#NM` delivery are not modeled. The MOVUPS profiles retain
their exact existing admissions. Native/JIT admission and physical execution
are not claimed; S2-34/D/System remains open. See the [S2-34.7a plan]
(../plans/STAGE_2.md#s2-347a--local-legacy-xorps-register-to-register).

## Local profile S2-34.7b: `s2-long-andps-reg-v1`

This separate schema-v2 profile requires the S2-34.1 raw x87/SSE snapshots
and admits one legacy register-only `ANDPS` instruction: `NP 0F 54 /r` with
ModR/M.mod=3. ModR/M.reg selects the destination and ModR/M.r/m selects the
source. A bare instruction or one effective REX byte is allowed; REX.R and
REX.B extend those fields to XMM0–XMM15. REX.W/X, all other legacy prefixes,
duplicate/ignored prefixes, memory operands, VEX and EVEX are outside the
profile. Intel SDM revision 093, Vol. 2A, ANDPS entry defines Op/En A as
ModRM.reg destination and ModRM.r/m source, with `DEST[127:0] := DEST[127:0]
BITWISE AND SRC[127:0]`; see the official [Intel SDM Volume 2A PDF]
(https://cdrdv2.intel.com/v1/dl/getContent/671199), ANDPS pages 3-72–3-74.

Execution ANDs the 16 raw destination bytes with the corresponding source
bytes. It performs no host floating-point conversion and changes no MXCSR,
x87, GPR, RFLAGS or other XMM state. SSE availability, CR0.EM=0, CR0.TS=0 and
CR4.OSFXSR=1 are diagnostic-profile preconditions; control-register gates and
`#UD`/`#NM` delivery are not modeled. Native/JIT admission and physical
execution are not claimed; S2-34/D/System remains open. See the
[S2-34.7b plan](../plans/STAGE_2.md#s2-347b--local-legacy-andps-register-to-register).

## Local profile S2-34.7c: `s2-long-orps-reg-v1`

This separate schema-v2 diagnostic profile requires raw x87/SSE snapshots and
admits one legacy register-only `ORPS`: `NP 0F 56 /r` with ModR/M.mod=3.
ModR/M.reg selects the destination and ModR/M.r/m selects the source. Bare
encoding or one effective REX byte is accepted; REX.R/B extend the operands to
XMM0–XMM15, while REX.W/X, all other prefixes, memory forms, VEX and EVEX are
outside the profile. Intel SDM revision 093, Vol. 2B, ORPS entry specifies
Op/En RM and defines the legacy 128-bit destination as the bitwise OR of its
old value and the source ([official Volume 2B PDF]
(https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf), pages 4-171–4-172).

Execution ORs the 16 raw bytes with no host floating-point conversion and
preserves MXCSR, x87, GPR, RFLAGS and every unrelated XMM register. SSE
availability and CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1 are diagnostic preconditions;
control-register gates, `#UD`/`#NM`, native/JIT admission and physical execution
are not modeled. See the [S2-34.7c plan]
(../plans/STAGE_2.md#s2-347c--local-legacy-orps-register-to-register).

## Local profile S2-34.7d: `s2-long-andnps-reg-v1`

This separate schema-v2 diagnostic profile requires raw x87/SSE snapshots and
admits one legacy register-only `ANDNPS`: `NP 0F 55 /r` with ModR/M.mod=3.
ModR/M.reg selects the destination and ModR/M.r/m selects the source. Bare
encoding or one effective REX byte is accepted; REX.R/B extend operands to
XMM0–XMM15, while REX.W/X, other prefixes, duplicate/ignored prefixes, memory
forms, VEX and EVEX are outside the profile. Intel SDM revision 093, Vol. 2A,
ANDNPS specifies Op/En RM (`ModRM.reg` read/write, `ModRM.r/m` read) and the
legacy operation `DEST[127:0] := (~DEST[127:0]) BITWISE AND SRC[127:0]`; the
official [Intel SDM Volume 2A PDF](https://cdrdv2.intel.com/v1/dl/getContent/671199)
lists `0F 55 /r` and this direction in the ANDNPS entry, pages 3-73–3-75.

Execution reads the original destination and source raw bytes before writing
the destination, so self-aliasing uses the old value for both operands. It
does no host floating-point conversion and preserves MXCSR, x87, GPR, RFLAGS
and every other XMM register. SSE availability and CR0.EM=0, CR0.TS=0,
CR4.OSFXSR=1 are diagnostic preconditions; control-register gates, `#UD`/`#NM`,
native/JIT admission and physical execution are not modeled. See the
[S2-34.7d plan](../plans/STAGE_2.md#s2-347d--local-legacy-andnps-register-to-register).

## Local profile S2-34.6a: `s2-long-movups-mem-base-v1`

This separate schema-v2 diagnostic profile requires the existing x87 and raw
SSE snapshots and admits one legacy `0F 10 /r` load or `0F 11 /r` store with
ModR/M memory mode 00/01/10, ordinary 64-bit base plus absent/signed disp8/
signed disp32. ModR/M.reg plus REX.R selects the XMM register; ModR/M.r/m plus
REX.B selects the GPR base. REX.W/X, SIB (`r/m=100`), ModR/M.mod=00/rm=101
(RIP-relative), address-size 32, segment overrides, other legacy prefixes,
VEX and EVEX remain unsupported. The state-only, register, `[RAX]` load and
`[RAX]` store profiles retain their exact previous schema shapes and admissions.

Encoding follows Intel SDM revision 093, Vol. 2B MOVUPS `0F 10 /r` (Op/En RM)
and `0F 11 /r` (Op/En B), plus Vol. 2A §§2.1.3–2.1.5 and 2.2.1.1–2.2.1.3
for ModR/M, REX and displacement ([official Intel SDM page](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html),
[Vol. 2A PDF](https://cdrdv2.intel.com/v1/dl/getContent/671199),
[Vol. 2B PDF](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf)).

Execution reuses `MemoryOperand` and its full-range canonical48 validation,
using an explicit 16-byte length rather than an integer `OperandSize`. Loads
perform exactly one `GuestMemory::read` into a temporary before committing XMM
and RIP. Stores capture raw `[u8; 16]` source bytes before exactly one
`GuestMemory::write` and commit RIP only after success. Backend failure
preserves full CPU state. FlatMemory's all-or-error behavior is only its private
fixture-RAM policy; it makes no hardware 16-byte atomicity claim. Existing
default DS/SS base behavior and #SS/#GP noncanonical attribution come from
`MemoryOperand`: RBP is SS-attributed and R13 data/#GP-attributed. RSP and R12
require SIB and cannot be encoded in this no-SIB slice. The upper-u64
AddressOverflow behavior remains ISSUE-012. No alignment faults, control gates,
`#UD/#NM`, segment overrides, address-size 32, SIB, RIP-relative, native/JIT,
VEX/EVEX behavior or physical execution are implemented.

### S2-34.6b RIP-relative extension

This same schema-v2 profile additionally admits one legacy `0F 10 /r` load or
`0F 11 /r` store when ModR/M.mod=00 and r/m=101. The disp32 is signed. Its
effective address is `next_rip + sign_extend(disp32)`, where `next_rip` is after
the complete instruction, including an optional REX byte. REX.R continues to
extend the XMM field. Intel SDM revision 093, Vol. 2A §2.2.1.6, pp. 2-11–2-12,
[official PDF](https://cdrdv2.intel.com/v1/dl/getContent/671199), states that
r/m=101 selects RIP-relative addressing independent of REX; `REX.B=1` does not
select R13. This bounded profile rejects REX.B=1 on this form, while leaving
REX.R available. Existing S2-34.6a base forms retain their admission.

The decoder uses the existing shared `rm_operand` path and `MemoryOperand::NextRip`;
execution uses the existing full-range 16-byte validator and one-read/one-write
path. Loads commit XMM/RIP only after a successful read; stores commit RIP only
after the single write succeeds. Tests and literal fixtures cover both
directions, positive/zero/negative disp32, with and without REX.R (7/8-byte
lengths), high XMM selection, current-RIP and unsigned-disp discriminators,
canaries, canonical/range errors, injected no-commit, each truncated disp32
byte with no data access, STOP/budget and SIB/prefix/REX.B rejection. Address
size and segment overrides, mandatory prefixes, VEX/EVEX, alignment/control
behavior, native/JIT and physical execution remain outside this profile.

### S2-34.6c SIB-addressed extension

The same profile additionally admits legacy `0F 10 /r` load and `0F 11 /r`
store with SIB addressing in address-size-64 mode. Address calculation is
provided by the shared `rm_operand` parser and `MemoryOperand`. Scale is 1/2/4/8;
SIB index=4 means no index unless REX.X=1, where it selects R12. With
ModR/M.mod=00 and SIB base=5, the base is absent and signed disp32 is used
regardless of REX.B. Mod=01/10 allow REX.B-extended bases, including R13.
Default SS and noncanonical #SS attribution apply when the actual base is RSP
or RBP; R12/R13 base, index-only and no-base addresses use DS/#GP attribution.
The normative details were checked against Intel SDM revision 093, Vol. 2A
§§2.1.3–2.1.5, 2.2.1.1–2.2.1.3 and Vol. 1 §§3.3.7.1, 3.7.4.1, 3.7.5.1,
plus Vol. 2B MOVUPS `0F 10 /r` and `0F 11 /r`:
[official SDM page](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html),
[Vol. 2A PDF](https://cdrdv2.intel.com/v1/dl/getContent/671199),
[Vol. 2B PDF](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf),
[Vol. 1 PDF](https://cdrdv2.intel.com/v1/dl/getContent/671436).

Loads still read exactly 16 bytes into a temporary before XMM/RIP commit;
stores capture 16 raw source bytes and issue exactly one write before RIP
commit. Errors preserve CPU state. Flat fixture RAM behavior makes no hardware
atomicity, MMIO, or SMP claim. Tests cover both directions, scale/index/base
classes, displacements, extended XMMs, callbacks, canaries, canonical fault
attribution, errors, truncation, rejected prefixes, STOP/budget, and literal
fixtures. Native/JIT remains denied. REX.W, address-size 32, legacy segment or
mandatory prefixes, VEX/EVEX, alignment and control gates remain unsupported.
S2-34 is still open; x86-64-v1 is only the ABI floor.

## Local S2-30a: bare `FNINIT` (`DB E3`)

The local interpreter accepts exactly unprefixed `DB E3`, and only when
`LongModeState.x87` already contains `Some(X87State)`. The profile assumes x87
availability. Execution calls `X87State::fninit()`, advances RIP by two and
retires one instruction. It preserves GPRs, RFLAGS, and all eight 10-byte
register payloads while setting FCW=`0x037f`, FSW=`0`, FTW=`0xffff`, and
FIP/FDP/FOP to zero. FNINIT itself raises no x87 floating-point exception.
The executor does not check CR0.EM/TS or deliver #NM. It does not synthesize
missing x87 state.

Prefixes, other `DB` encodings, and `9B DB E3` FINIT remain Unsupported.
Native execution still rejects x87 profiles and FNINIT; no FPU capability is
advertised. This slice does not implement x87 pending-exception delivery or
FINIT's preceding WAIT behavior, nor any other x87 instruction.

Tests cover exact decoding and rejection neighbors, arbitrary register payload
preservation, complete environment reset, full GPR/RFLAGS preservation,
truncated/faulting fetch without commit, missing-state rejection, and STOP or
budget boundaries without fetch or retirement. The testkit runner observes all
x87 fields through the existing `s2-long-x87-state-v1` state profile.

Normative source: Intel® 64 and IA-32 Architectures Software Developer's Manual,
revision 093, Vol.2A, “FINIT/FNINIT—Initialize Floating-Point Unit,”
pp.3-345–3-346, [official Vol.2A PDF](https://cdrdv2.intel.com/v1/dl/getContent/671199).
The manual specifies `DB E3` as FNINIT and `9B DB E3` as FINIT with WAIT;
FNINIT restores FCW to `037F`, clears FSW and the last instruction/data
pointers/opcode, marks all tags empty, preserves the eight data-register
contents, and does not generate floating-point exceptions.

Normative basis: Intel SDM revision 093, Vol.1 §§8.1.2, 8.1.3.1, 8.1.5,
8.1.7–8.1.9 and 8.5; Vol.2A “FINIT/FNINIT—Initialize Floating-Point Unit,”
pp.3-345–3-346; and Vol.3A §11.1.1 Table 11-1 note 5. Official manuals:
[Vol.1 253665-093US](https://cdrdv2.intel.com/v1/dl/getContent/671436),
[Vol.2A 253666-093US](https://cdrdv2.intel.com/v1/dl/getContent/671199),
[Vol.3A combined manual](https://cdrdv2.intel.com/v1/dl/getContent/671447).

## Local S2-30b: bare `FNSTSW AX` (`DF E0`)

The local long-mode interpreter accepts exactly the unprefixed bytes `DF E0`
when `LongModeState.x87` already contains `Some(X87State)`. It reads the raw
pre-operation 16-bit `status_word` and writes that value to AX through the
existing word-sized GPR write, preserving RAX[63:16], all other GPRs, and
RFLAGS. RIP advances by two and one instruction retires. Missing x87 state is
Unsupported. This existing local state profile assumes x87 availability and
does not test CR0.EM/TS, deliver #NM, or add a preceding WAIT.

Intel SDM revision 089 Vol.2A defines C0/C1/C2/C3 as undefined after FNSTSW.
Accordingly, the raw pre-operation FSW bits copied to AX include the concrete
local values of those fields, but callers must not treat those four bits as
architecturally preserved or require future native equality. The local model
currently leaves the x87 snapshot untouched as a deterministic implementation
policy; full raw-state comparison in this local-only fixture profile checks
that policy, not an architectural guarantee. FNSTSW raises no x87 floating-
point exception.

All prefixes, other `DF` ModR/M bytes (including memory forms), and `9B DF E0`
are Unsupported. Fetch faults, STOP, exhausted budget and missing state commit
neither GPR nor x87 state. Native execution rejects both the x87 profile and
FNSTSW instruction admission. This does not synthesize missing x87 state and
does not advertise an FPU feature.

Normative source: Intel® 64 and IA-32 Architectures Software Developer's
Manual, revision 089, Vol.2A, “FNSTSW—Store x87 FPU Status Word,” p.3-385,
[official Vol.2A PDF](https://cdrdv2-public.intel.com/868140/253666-089-sdm-vol-2a.pdf).
The instruction reference specifies AX as the destination for `DF E0`, marks
C0/C1/C2/C3 undefined, and states that FNSTSW does not generate floating-point
exceptions.

## Local S2-30c: bare `FNCLEX` (`DB E2`)

The local long-mode interpreter accepts exactly unprefixed `DB E2` when
`LongModeState.x87` already contains `Some(X87State)`. It clears FSW[0:7]
and FSW[15] with the mask `0x80ff`, preserving TOP and the concrete C0/C1/C2/C3
values only as deterministic local policy. Intel leaves C0/C1/C2/C3 undefined
after FNCLEX, so their local retention is not an architectural preservation
claim or a native-equality requirement. FCW, FTW, all eight raw data payloads,
instruction/data pointers, last opcode, GPRs, RFLAGS, and MXCSR are untouched.
RIP advances by two and one instruction retires.

This existing local state profile assumes x87 is available. It does not check
CR0.EM/TS, deliver #NM, insert WAIT, check pending unmasked exceptions, or
deliver floating-point exceptions. FNCLEX is a no-wait instruction and raises
no floating-point exception in this slice. All prefixes, other `DB` second
bytes and `9B DB E2` remain Unsupported; missing state is Unsupported. Fetch
faults, STOP and exhausted budget do not commit. Native x87-profile execution
and FNCLEX admission remain closed; no FPU feature is advertised.

Normative source: Intel® 64 and IA-32 Architectures Software Developer's
Manual, revision 093, Vol.2A, “FCLEX/FNCLEX—Clear Exceptions,” pp.3-320–3-321,
[official Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf).
It specifies bare `DB E2` as the no-wait form, clears exception flags PE/UE/OE/
ZE/DE/IE, ES, SF and B, leaves C0–C3 undefined, and specifies no floating-
point exception delivery or MXCSR effect.

## Local S2-30d: exact-only `FADD ST(0), ST(1)` (`D8 C1`)

The local long-mode interpreter admits exactly bare `D8 C1` when
`LongModeState.x87` already contains `Some(X87State)`. It requires FCW exactly
`0x037f`, FSW exception/ES/B bits clear (`status_word & 0x80ff == 0`), Valid
tags at physical TOP and `(TOP + 1) & 7`, canonical raw Normal binary80 values
in both payloads, and equal normalized exponents. Zero operands, cancellation,
unequal exponents, special, pseudo, subnormal or unnormal raw encodings,
inconsistent tags/environment and any unsupported result are rejected as
`Unsupported` before state mutation.

The exact coefficient is obtained with `add_finite_same_exponent`. Before
packing, its magnitude must be at most `u64::MAX`, or be an even 65-bit value;
an odd wider coefficient would lose a real low bit and is rejected. The result
uses `round_pack_exact_coefficient` in nearest-ties-even mode only after this
exactness proof, and must classify as canonical Normal. This handles an exact
carry without using rounding as a substitute for the proof. Overflow and
zero/subnormal results are rejected with no commit.

On success only the physical TOP 80-bit payload changes. FSW.C1 is cleared,
FIP is set to the instruction's starting RIP, and RIP advances by two. TOP,
tags, every other payload, FCW, the rest of FSW, FPR pointers except FIP, FOP,
GPRs and RFLAGS are preserved. C0/C2/C3 are architecturally undefined after
FADD; preserving their concrete values is a local deterministic policy. FDP is
undefined for this register-only form; retaining it is also local policy and
not a native equality claim. Modern FOP state is left untouched on this exact
no-exception path.

This local slice assumes x87 availability. It does not check CR0.EM/TS, deliver
#NM, advertise an FPU capability, or model general x87 precision/rounding,
special values, exceptions, stack faults or exception delivery. All other
encodings, prefixes, missing state and unsupported operands/results remain
Unsupported. Fetch faults, STOP and exhausted budget do not commit. Native x87
profile execution and FADD admission remain closed.

Normative source: Intel® 64 and IA-32 Architectures Software Developer's
Manual, revision 093, Vol.2A, “FADD/FADDP/FIADD—Add,” pp.3-311–3-313, and Vol.1
§§8.1.5.2–8.1.9.1, pp.8-7–8-11, for x87 state, precision control, rounding,
tags and FIP/FDP/FOP ([official Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf),
[official Vol.1 PDF](https://cdrdv2-public.intel.com/929351/253665-093-sdm-vol-1.pdf)).
The local profile is narrower than the architectural instruction.

## Local S2-30e: exact-only `FADD ST(0), ST(i)` (`D8 C0+i`)

The local long-mode interpreter accepts exactly the bare encodings `D8 C0`
through `D8 C7`, mapping the low ModR/M bits to source index `i`. It retains
every S2-30d precondition and result restriction: existing x87 state, FCW
`0x037f`, clear FSW `0x80ff` mask, Valid tags for used operands, canonical
Normal payloads, equal normalized exponents, exact coefficient representability
and canonical Normal output. Destination is physical TOP; source is
`(TOP+i)&7`.

When `i=0`, physical TOP is read once and used for both operands. No tag outside
the operands used by the instruction is required. Both raw inputs and the
validated result are obtained before mutation; success writes the destination
payload once. It clears FSW.C1, sets FIP to starting RIP and advances RIP by
two. TOP, tags, all non-destination payloads, FCW, remaining FSW fields, FDP,
FOP, GPRs and RFLAGS are retained. Preserving architecturally undefined C0/C2/C3
and FDP is the same local deterministic policy as S2-30d; FOP is unchanged on
the exact no-exception path. Unsupported input or encoding returns Unsupported
without state commit.

Prefixes, D8 ModR/M values outside C0–C7, memory forms, DC/DE and WAIT remain
unsupported. Native x87 execution and admission remain closed. Literal vectors
cover every source index at varied TOP values, including the `i=0` alias with
empty unrelated tags, and wrap cases `i=1, TOP=7` and `i=7, TOP=7`. This only generalizes source
selection; the equal-exponent exact-only arithmetic domain is unchanged.

## Local S2-30f: exact-only `FADD ST(i), ST(0)` (`DC C0+i`)

The local long-mode interpreter accepts exactly bare `DC C0` through `DC C7`,
mapping the low ModR/M bits to destination index `i`. All S2-30d arithmetic,
environment, and result restrictions remain in force: existing x87 state, FCW
`0x037f`, clear FSW `0x80ff` mask, Valid tags for used operands, canonical
Normal payloads, equal normalized exponents, exact coefficient representability
and canonical Normal output. Source is logical ST(0), at physical TOP; the
destination is logical ST(i), at physical `(TOP+i)&7`.

For `i=0`, the one physical register is read once as both operands and only
its tag is required. For `i!=0`, both used tags must be Valid. Both raw
operands are captured and the complete result is validated before mutation.
Success writes only the destination payload once, clears FSW.C1, sets FIP to
starting RIP, and advances RIP by two. There is no pop. TOP, tags, source and
all other payloads, FCW, remaining FSW fields, FDP, FOP, GPRs and RFLAGS are
preserved. C0/C2/C3 and FDP retention follows the S2-30d local deterministic
policy for architecturally undefined state. Unsupported inputs and encodings
return Unsupported without state commit.

Prefixes, DC ModR/M values outside C0–C7, memory forms, other D8 forms, DE
and WAIT remain unsupported. Native x87 execution and admission remain closed.
Literal expected-byte vectors cover every destination index with varied TOP
and wrap, `i=0` with unrelated empty tags, and source preservation for `i!=0`.
Reject vectors cover used bad tags, special operands, unequal exponents and
inexact coefficients; decoder checks cover all DC ModR/M bytes, prefixes and
truncation. This changes destination selection only and does not enlarge the
S2-30d exact-only arithmetic domain.

Normative source: Intel® 64 and IA-32 Architectures Software Developer's
Manual, revision 093, Vol.2A, “FADD/FADDP/FIADD—Add,” pp.3-311–3-313, and Vol.1
§§8.1.8–8.1.9.1, pp.8-9–8-11 ([official Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf),
[official Vol.1 PDF](https://cdrdv2-public.intel.com/929351/253665-093-sdm-vol-1.pdf)).
The local profile remains narrower than the architectural instruction.

## Local S2-30g: exact-only `FADDP ST(i), ST(0)` (`DE C0+i`)

The local long-mode interpreter accepts exactly bare `DE C0` through `DE C7`,
mapping the low ModR/M bits to destination index `i`. It retains all S2-30d
environment, operand and result restrictions: existing x87 state, FCW
`0x037f`, clear FSW `0x80ff` mask, Valid tags for used physical operands,
canonical Normal inputs, equal normalized exponents, exact coefficient
representability and canonical Normal output. The destination is physical
`(TOP+i)&7`; source ST(0) is physical TOP. For `i=0`, the shared register is
captured once and requires only its single Valid tag. For `i!=0`, both used tags
must be Valid. Both raw operands and the complete result are validated before
the first state mutation.

On success, write result bytes to the old physical destination, mark old
physical TOP Empty, increment TOP modulo eight, clear FSW.C1, set FIP to the
starting RIP, advance RIP by two and retire once. Do not rotate the physical
register array or tag word. For `i=0`, writing then emptying old TOP leaves the
new bytes beneath an Empty tag; they are not logically accessible after pop.
For `i=1`, the destination is new ST(0). Preserve all other raw payloads and
tags, FCW, remaining FSW bits, FDP, FOP, GPRs and RFLAGS. C0/C2/C3 and FDP
retention are local deterministic policies; this slice does not model general
x87 flags or exception delivery. Unsupported inputs and encodings return
Unsupported without state commit.

Prefixes, other DE ModR/M bytes, memory forms, WAIT, and native admission remain
unsupported. Literal vectors cover every destination index and TOP, physical
payload sentinels, `i=0` behavior, new ST(0) at `i=1`, TOP wrap, exact carry and
full-state rejects including i=0 overflow. Fixture schema accepts only the
eight bare encodings, and native x87 execution remains closed.

Normative source: Intel® 64 and IA-32 Architectures Software Developer's
Manual, revision 093, Vol.2A, “FADD/FADDP/FIADD—Add,” pp.3-311–3-313, and Vol.1
§§8.1.7–8.1.9, pp.8-8–8-11 ([official Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf),
[official Vol.1 PDF](https://cdrdv2-public.intel.com/929351/253665-093-sdm-vol-1.pdf)).
The local profile is narrower than the architectural instruction.

## Local S2-30h: exact-only `FSUB ST(0), ST(i)` (`D8 E0+i`)

The local long-mode interpreter accepts exactly bare `D8 E0` through `D8 E7`,
using `ST(0)` as physical TOP and `ST(i)` as physical `(TOP+i)&7`. The
environment gate is the S2-30d profile: an existing `X87State`, FCW `0x037f`,
and `FSW & 0x80ff == 0`. Both used physical tags must be Valid (one tag for
the alias), both raw values must classify as canonical Normal binary80, and
their normalized exponents must match. After validating the source, the
interpreter toggles only bit 15 in a local copy of its binary80 bytes and uses
the existing exact same-exponent addition path to form `DEST − SRC`; the
source register bytes are not changed. A result is accepted only when its
coefficient is at most `u64::MAX`, or is an even 65-bit magnitude no greater
than `2^65−2`, and packing yields canonical Normal binary80.

`i=0` is always Unsupported because source and destination alias, so the
operation cancels exactly. Other rejects include missing x87 state, bad used
tags, dirty FCW/FSW, unequal exponents, zero/subnormal/pseudo-denormal/unnormal/
infinity/NaN operands, odd 65-bit coefficient, exact subnormal result and
overflow. Every reject precedes state mutation.

Success replaces only the physical TOP payload, clears FSW.C1, sets FIP to the
starting RIP, advances RIP by two and retires once. TOP, FTW, FCW, FDP, FOP,
other FSW bits, other payloads, GPRs and RFLAGS are preserved. C0/C2/C3 are
undefined in the SDM; retaining those bits and FDP is the local deterministic
policy. This profile does not add a CPUID/native feature or model general x87
rounding, exceptions, stack faults, CR0.EM/TS or #NM. Native instruction and
profile admission remain closed for all eight encodings.

Literal byte vectors cover `1.5−1=0.5`, `1−1.5=−0.5`, both negative-source
forms, `1−(−1)=2`, `−1−1=−2`, the exact `±2^-63` differences, maximum
significand carry, and minimum Normal output. Odd 65-bit coefficient,
identical/cancellation including every i=0/TOP, exact subnormal and overflow
cases compare the full pre/post state. Decoder tests cover all ModR/M values,
prefixes, WAIT, truncation, memory/reverse forms and fetch behavior. Schema and
runner checks accept i=1..7 and observe Unsupported with no state change for
i=0.

Normative source: Intel SDM revision 093, Vol.2A, “FSUB/FSUBP/FISUB—Subtract,”
pp.3-394–3-396: the operation is `DEST − SRC`, the `D8 E0+i` register form is
specified directly, and C0/C2/C3 are undefined. For this exact-only profile,
C1=0 is the architectural non-rounded-result value. ([official Vol.2A PDF,
pp.3-394–3-396](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=512))

## Local S2-30i: exact-only `FSUB ST(i), ST(0)` (`DC E8+i`)

The local long-mode interpreter accepts exactly bare `DC E8` through `DC EF`,
using `ST(i)` at physical `(TOP+i)&7` as destination and `ST(0)` at physical
TOP as source. It retains the S2-30d environment gate: an existing
`X87State`, FCW `0x037f`, and `FSW & 0x80ff == 0`. Both used physical tags must
be Valid, both raw operands must be canonical Normal binary80, and normalized
exponents must match. After validating the original source, the interpreter
toggles only bit 15 in a local ST(0) copy and uses the exact same-exponent
addition path to form `DEST − SRC`; source payload bytes are preserved. The
accepted coefficient bound remains at most `u64::MAX` or an even 65-bit
magnitude no greater than `2^65−2`, and the packed output must be canonical
Normal binary80.

`i=0` is Unsupported because destination and source alias and the exact result
is cancellation. Other rejects include missing state, bad used tags, dirty
FCW/FSW, unequal exponents, all non-Normal classes, rejected exact
coefficients, zero/subnormal output and overflow. Every reject precedes state
mutation.

Success replaces only physical `(TOP+i)&7`, clears FSW.C1, sets FIP to the
starting RIP, advances RIP by two and retires once. It preserves physical TOP,
FTW, FCW, FDP, FOP, other FSW bits, source ST(0), all other payloads, GPRs and
RFLAGS. Retaining undefined C0/C2/C3 and FDP is local deterministic policy.
Native instruction and profile admission remain closed for all eight
encodings.

Literal vectors include `1.5−1=+0.5`, `1−1.5=−0.5`, negative permutations,
`1−(−1)=2`, `−1−1=−2`, `±2^-63`, maximum exact carry and minimum Normal.
Every TOP×i pair checks physical placement and unchanged source. Alias, unequal
exponents, special/non-Normal values, odd 65-bit coefficient, exact subnormal
result and overflow reject atomically. Decoder checks the complete DC ModR/M
space, prefixes, WAIT, truncation, memory forms, `FSUBR` and `FSUBP`; schema
checks alias Unsupported, while the runner matrix covers i=1..7. This local
instruction does not add general rounding, exceptions, stack faults, CR0
behavior or native x87 observation.

Normative source: Intel SDM revision 093, Vol.2A, “FSUB/FSUBP/FISUB—Subtract,”
pp.3-394–3-396 ([official Vol.2A PDF,
pp.3-394–3-396](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=512)).

## Local S2-30j: exact-only `FSUBP ST(i), ST(0)` (`DE E8+i`)

The local long-mode interpreter accepts exactly bare `DE E8` through `DE EF`,
with the low ModR/M bits selecting destination `i`. It reuses the S2-30i exact
subtraction preconditions and helper: existing x87 state, FCW `0x037f`,
`FSW & 0x80ff == 0`, Valid tags for both used physical registers, canonical
Normal operands, equal normalized exponents, the accepted exact coefficient
bound, and a canonical Normal output. Destination is old physical
`(TOP+i)&7`; source is old physical TOP. `i=0` is Unsupported because the
operands alias and the result is exact cancellation. All checks complete
before mutation.

On success, write the result bytes to the old physical destination, mark old
physical TOP Empty while leaving its bytes in place, increment TOP modulo
eight, clear C1, set FIP to starting RIP, advance RIP by two and retire once.
Do not rotate physical registers or FTW. Preserve other payloads, other tags,
FCW, remaining FSW bits, FDP, FOP, GPRs and RFLAGS. Every arithmetic/domain
reject and fetch/stop/zero-budget path leaves state unchanged. Prefixes,
FSUBRP `DE E0+i`, and DE ModR/M bytes outside the existing FADDP `DE C0+i`
and this FSUBP range remain unsupported; native admission stays closed for
all eight FSUBP codes.

Literal vectors cover positive/negative subtraction orders, opposite signs,
`±2^-63`, maximum even carry and minimum Normal. Every TOP×i pair checks the
physical destination and TOP wrap/pop. Alias for every TOP, bad environment,
tags, unequal exponents, special/non-Normal values, cancellation, odd 65-bit
coefficient, exact subnormal and overflow compare full state. Decoder checks
the full DE ModR/M space and prefixes; schema accepts eight codes; native
profile rejection covers all eight.

Normative source: Intel SDM revision 093, Vol.2A, “FSUB/FSUBP/FISUB—Subtract,”
pp.3-394–3-396 ([official PDF, pp.3-394–3-396](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=512)).

## Local S2-30k: exact-only `FSUBR ST(0), ST(i)` (`D8 E8+i`)

The local long-mode interpreter accepts exactly bare `D8 E8` through `D8 EF`.
`ST(0)` is old physical TOP and `ST(i)` is physical `(TOP+i)&7`. The arithmetic
helper is called with logical destination `i` and source `0`, so it computes
`ST(i) − ST(0)`; the architectural write still targets old physical TOP. The
operation retains the S2-30d gate: existing x87 state, FCW `0x037f`,
`FSW & 0x80ff == 0`, Valid tags for used physical slots, canonical Normal
operands with equal normalized exponents, an accepted exact coefficient
(`<= u64::MAX` or even 65-bit `<= 2^65−2`), and canonical Normal output.
Alias `i=0`, missing/bad state, unequal exponents, other raw classes,
cancellation, odd 65-bit coefficient, exact zero/subnormal result and overflow
are Unsupported before mutation.

Success replaces only old physical TOP payload, clears C1, sets FIP to starting
RIP, advances RIP by two and retires once. It preserves TOP, FTW, FCW, FDP,
FOP, other FSW bits, other payloads, GPRs and RFLAGS. C0/C2/C3 are undefined;
retaining their original snapshot bits is local deterministic policy. General
x87 rounding/exceptions, special values, stack faults, CR0 behavior and native
x87 execution remain unsupported. Native profile/admission remains closed for
all eight encodings.

Literal vectors cover `1.5−1=+0.5`, `1−1.5=−0.5`, the negative permutations,
`1.5−(−1)=+2.5`, `−1.5−1=−2.5`, `±2^-63`, maximum even carry, and minimum
Normal. Every TOP×i=1..7 pair verifies the old physical TOP destination and
preserved source/sentinels. Alias and environment/tag/exponent/class/
coefficient/subnormal/overflow rejects compare complete state. Decode coverage
checks all D8 ModR/M values, prefixes, WAIT, truncation and adjacent D8/DC/DE
forms. The fixture schema admits all eight encodings and represents `D8 E8`
with `Unsupported`, no stop RIP and budget one; the local literal runner checks
success state and an unchanged alias snapshot.

Normative source: Intel SDM revision 093, Vol.2A pp.3-397–3-399,
“FSUB/FSUBR/FSUBP/FISUBR—Subtract” ([official PDF, pp.3-397–3-399](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=515)).
It defines `D8 E8+i` as `ST(i) − ST(0)`, C1 clear when no round-up occurs,
and C0/C2/C3 undefined.

## Local S2-30l: exact-only `FSUBR ST(i), ST(0)` (`DC E0+i`)

The local long-mode interpreter accepts exactly bare `DC E0` through `DC E7`.
For `i`, it computes logical `ST(0) − ST(i)` with
`exact_fsub_result(state, 0, i)` and writes to old physical `(TOP+i)&7`.
The domain remains the S2-30d exact finite profile: FCW `0x037f`,
`FSW & 0x80ff == 0`, Valid used tags, canonical Normal operands with equal
normalized exponents, accepted exact coefficient bounds and canonical Normal
output. Alias `i=0` and every other unsupported state reject before commit.

Success changes only old physical `ST(i)` payload, clears C1, records starting
RIP in FIP, advances RIP by two and retires once. TOP, FTW, source physical
TOP payload, other x87 fields, GPRs and RFLAGS are preserved. C0/C2/C3 remain
deterministic local snapshot bits although architecturally undefined. Prefixes,
other DC groups, and native x87 admission remain unsupported. Local validation
covers literal signs/order, exact boundaries, every TOP×i placement,
full-state domain rejection, fetch/stop/budget atomicity, complete DC ModR/M
and prefix decoding, fixture schema, and native denial.

Normative source: Intel SDM revision 093, Vol.2A pp.3-397–3-399
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=515));
`FSUBR` computes `SRC−DEST`, C1 is clear for exact results, and C0/C2/C3 are
undefined.

## Local S2-30m: exact-only `FSUBRP ST(i), ST(0)` (`DE E0+i`)

The local long-mode interpreter accepts exactly bare `DE E0` through `DE E7`.
It uses the S2-30d exact finite gate and calls
`exact_fsub_result(state, 0, i)` to compute old `ST(0) − ST(i)`. The result
goes to old physical `(TOP+i)&7`; only after that write does the operation
mark old TOP Empty and increment TOP modulo eight. `i=0` is the decoded
self-alias but is Unsupported because it produces exact cancellation. The
no-operand `DE E1` alias represents `FSUBRP ST(1), ST(0)` (`i=1`) and executes
through the same local register form.
Missing state, invalid environment/tags, non-Normal or unequal-exponent
operands, rejected coefficient, cancellation, subnormal/zero output and
overflow all reject before state mutation.

Success clears C1, records starting RIP in FIP, advances RIP by two and retires
once. The popped TOP payload remains byte-for-byte present under its Empty tag;
other tags/payloads, FCW, remaining FSW, FDP, FOP, GPRs and RFLAGS are
preserved. There is no physical register rotation. C0/C2/C3 retention is local
snapshot policy because the SDM leaves them undefined. STOP, fetch fault,
truncation and zero instruction budget do not commit. The decoder covers all
256 DE ModR/M values and rejects prefixes including REX and WAIT while
preserving FADDP DE C0..C7 and FSUBP DE E8..EF. Schema accepts all eight codes
and models DE E0 as Unsupported, `stop_rip: null`, budget one. Native profile
and instruction admission remain closed for all eight encodings.

Local literal coverage checks every TOP×i physical destination and pop,
operand order/sign vectors including ±2^-63, maximum accepted even 65-bit
coefficient and minimum Normal output, plus full-state atomic rejects for the
numeric/environment/tag domain. Fixture-runner tests repeat all TOP×i and
alias behavior. No general x87 rounding, exception delivery, CR0/#NM behavior,
or physical x87 observation is claimed.

Normative source: Intel SDM revision 093, Vol.2A, “FSUBR/FSUBRP/FISUBR—Reverse
Subtract,” pp.3-397–3-399 ([official PDF,
pp.3-397–3-399](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=515));
FSUBRP computes old `ST(0) − ST(i)` before pop, exact results clear C1, and
C0/C2/C3 are undefined.

## Local S2-30n: exact-only `FMUL ST(0), ST(i)` (`D8 C8+i`)

The interpreter accepts exactly bare `D8 C8` through `D8 CF` with an existing
`Some(X87State)`. It requires FCW `0x037f`, `FSW & 0x80ff == 0`, Valid tags
for used physical operands, and raw canonical Normal binary80 operands. For
`i=0`, the sole TOP register is read once and squared. Normalized exponents may
differ. The integer product helpers compute the full exact 128-bit magnitude;
after normalization the low 64 bits must be zero, then the retained high 64
bits are packed at precision 64. The result must classify as canonical Normal.
Inexact, zero/subnormal, overflow, missing/bad state, invalid tags or special/
noncanonical encodings are Unsupported before commit.

Success changes only the old physical TOP payload, clears C1, records starting
RIP as FIP, advances RIP by two and retires once. TOP, tags, other payloads,
FCW, other FSW fields, FDP/FOP, GPRs and RFLAGS are preserved. C0/C2/C3 remain
concrete only by local deterministic policy; the SDM leaves them undefined.
Decode coverage enumerates D8 ModR/M and prefix boundaries; fixture schema and
runner cover all TOP×i pairs, including the i=0 square and physical sentinels.
Native x87 profile and instruction admission remain closed.

Normative source: Intel SDM revision 093, Vol.2A, “FMUL/FIMUL—Multiply,”
pp.3-360–3-362 ([official PDF, Vol.2A p.3-360](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=478)).
`D8 C8+i` computes old `ST(0) * ST(i)` without popping; C1 is cleared for an
exact result and C0/C2/C3 are undefined. This local profile implements no
general rounding, special-value, exception, CR0/#NM, or native x87 behavior.

## Local S2-30o: exact-only `FMUL ST(i), ST(0)` (`DC C8+i`)

The interpreter accepts exactly bare `DC C8` through `DC CF` with an existing
`Some(X87State)`. It reuses the S2-30n FMUL environment, tag, raw-input and
exact-product gates: FCW `0x037f`, `FSW & 0x80ff == 0`, Valid tags for used
physical operands, canonical Normal binary80 inputs, and a normalized product
whose low 64 bits are zero. Exponents may differ. The precision-64 packed
result must remain canonical Normal. `i=0` reads TOP once, squares it, and
requires only its Valid tag. Rejections occur before commit.

Success writes only old physical `(TOP+i)&7`, clears C1, sets FIP to starting
RIP, advances RIP by two and retires once. There is no pop or TOP change; for
`i!=0`, the source and all other payloads, tags, x87 fields, GPRs and RFLAGS
are preserved. For `i=0`, the sole TOP payload is replaced by its square.
Decode coverage enumerates DC ModR/M and prefixes while preserving DC C0..C7
FADD, E0..E7 FSUBR and E8..EF FSUB. Fixture schema and runner cover all eight
indices. Native x87 admission remains closed.

Normative source: Intel SDM revision 093, Vol.2A, “FMUL/FIMUL—Multiply,”
pp.3-360–3-362 ([official PDF, Vol.2A p.3-360](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=478)).
`DC C8+i` computes old `ST(i) * ST(0)` without popping; exact results clear C1,
and C0/C2/C3 are undefined. General rounding, special values, exception
delivery, CR0/#NM and native x87 behavior are not implemented.

## Local S2-30p: exact-only `FMULP ST(i), ST(0)` (`DE C8+i`)

The interpreter accepts exactly bare `DE C8` through `DE CF` with an existing
`Some(X87State)`. It reuses the S2-30n exact FMUL gate: FCW `0x037f`,
`FSW & 0x80ff == 0`, Valid tags for used physical operands, canonical Normal
binary80 inputs, and a normalized product whose low 64 bits are zero. Exponents
may differ. The precision-64 result must remain canonical Normal. `i=0` reads
the old TOP value once, computes its square and requires only that one Valid
tag. Numeric, environment, tag, raw-class and missing-state rejects occur
before commit. The no-operand `DE C9` encoding is the `i=1` form,
`FMULP ST(1), ST(0)`.

Success writes the product to old physical `(TOP+i)&7`, then marks old physical
TOP Empty while retaining that register payload. It increments TOP, clears C1,
sets FIP to starting RIP, advances RIP by two and retires once. All other tags,
payloads, x87 fields, GPRs and RFLAGS remain unchanged. For `i=0`, the square is
written into old TOP before that same register is emptied. The decoder accepts
DE C8..CF while preserving FADDP DE C0..C7, FSUBRP DE E0..E7 and FSUBP DE
E8..EF; prefixes, WAIT and truncated encodings remain unsupported. Fixture
schema accepts all eight encodings, including a `DE C8` Unsupported/no-stop
case with budget one, and the runner checks every TOP×i pair including exact
square success. Native x87 admission remains closed.

Normative source: Intel SDM revision 093, Vol.2A, “FMUL/FIMUL—Multiply,”
pp.3-360–3-362 ([official PDF, Vol.2A p.3-360](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=478)).
`DE C8+i` computes old `ST(i) * ST(0)` before popping old `ST(0)`; exact
results clear C1, and C0/C2/C3 are undefined. General rounding, special values,
exception delivery, CR0/#NM and native x87 behavior are not implemented.

## Local S2-30q: exact-only `FDIV ST(0), ST(i)` (`D8 F0+i`)

The interpreter accepts exactly bare `D8 F0` through `D8 F7` with an existing
`Some(X87State)`. Require FCW `0x037f`, `FSW & 0x80ff == 0`, Valid tags for the
used physical operands and canonical Normal binary80 encodings. Normalize the
two significands and use integer division; the raw quotient remainder must be
zero before packing. The quotient is packed at precision 64 and must classify
as canonical Normal. This permits unequal exponents and both quotient
normalization branches, while rejecting zero operands, specials, noncanonical
raw inputs, inexact quotients, overflow and subnormal/zero results before
commit. For `i=0`, read one Valid tag and return exact `+1` for either operand
sign. No rounded-division helper is used as an exactness test.

Success replaces only old physical TOP's payload, clears C1, records starting
RIP as FIP, advances RIP by two and retires once. TOP, tags and all other x87
fields, registers, GPRs and RFLAGS stay unchanged. Decoder coverage enumerates
all D8 ModR/M values and rejects prefixes, WAIT and truncation; adjacent D8
groups retain their prior meanings. The fixture schema and runner cover all
eight encodings and an inexact Unsupported case. Native admission remains
closed. Other FDIV directions, DC/DE forms, and memory/integer forms remain
unsupported.

Normative source: Intel SDM revision 093, Vol.2A, “FDIV/FIDIV—Divide,”
pp.3-333–3-335 ([official PDF, Vol.2A p.3-333](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=451)).
`D8 F0+i` divides old `ST(0)` by old `ST(i)` without popping; successful exact
results clear C1, while C0/C2/C3 are undefined. General rounding, architectural
exceptions, special values, CR0/#NM and native x87 behavior are not modeled.

## Local S2-30r: exact-only `FDIVP ST(i), ST(0)` (`DE F8+i`)

The interpreter accepts exactly bare `DE F8` through `DE FF` with an existing
`Some(X87State)`. It requires FCW `0x037f`, `FSW & 0x80ff == 0`, Valid tags
for the used physical operands and canonical Normal binary80 encodings. The
ordered exact division gate computes old `ST(i)/ST(0)`: it normalizes both
significands, requires zero quotient remainder before packing, packs at
precision 64 and accepts only canonical Normal output. Unequal exponents are
allowed. Zero operands, specials, noncanonical raw values, inexact quotients,
overflow and subnormal/zero output are rejected before mutation. `i=0` reads
one Valid physical TOP tag and yields exact `+1` for either sign.

Success writes the result to old physical `(TOP+i)&7`, then empties old
physical TOP and increments TOP. For `i=0`, it writes `+1` to old TOP before
emptying that same register. Success clears C1, records starting RIP as FIP,
advances RIP by two and retires once; all other tags, payloads, x87 fields,
GPRs and RFLAGS remain unchanged. The decoder accepts DE F8..FF while
preserving DE C0..CF and DE E0..EF groups; DE F0..F7 FDIVRP, prefixes, WAIT
and truncation remain unsupported. The fixture schema and runner cover all
eight encodings and atomic inexact `DE F9` rejection with no stop RIP and
budget one. Native admission remains closed.

Normative source: Intel SDM revision 093, Vol.2A, “FDIV/FIDIV—Divide,”
pp.3-333–3-335 ([official PDF, Vol.2A p.3-333](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=451)).
`DE F8+i` divides old `ST(i)` by old `ST(0)` before popping old `ST(0)`;
exact results clear C1, while C0/C2/C3 are undefined. General rounding,
architectural exceptions, special values, CR0/#NM and native x87 behavior are
not modeled.

## Local S2-30s: exact-only `FDIVRP ST(i), ST(0)` (`DE F0+i`)

The interpreter accepts exactly bare `DE F0` through `DE F7` with an existing
`Some(X87State)`. It requires FCW `0x037f`, `FSW & 0x80ff == 0`, Valid tags
for used physical operands and canonical Normal binary80 inputs. The ordered
exact division gate computes old `ST(0)/ST(i)`: it normalizes both
significands, requires zero quotient remainder before packing, packs at
precision 64 and accepts only canonical Normal output. Unequal exponents are
allowed. Zero operands, specials, noncanonical raw values, inexact quotients,
overflow and subnormal/zero output reject before mutation. The no-operand
`DE F1` alias is `FDIVRP ST(1), ST(0)`; `i=0` reads one Valid physical TOP
tag and yields exact `+1` for either sign.

Success writes the result to old physical `(TOP+i)&7`, then empties old
physical TOP and increments TOP. For `i=0`, it writes `+1` to old TOP before
emptying that same register. Success clears C1, records starting RIP as FIP,
advances RIP by two and retires once; all other tags, payloads, x87 fields,
GPRs and RFLAGS remain unchanged. The decoder accepts DE F0..F7 while
preserving all neighboring DE groups; prefixes, REX, WAIT and truncation
remain unsupported. The fixture schema and runner cover all eight encodings
and atomic inexact `DE F1` rejection with no stop RIP and budget one. Native
admission remains closed.

Normative source: Intel SDM revision 093, Vol.2A,
“FDIVR/FDIVRP/FIDIVR—Reverse Divide,”
pp.3-336–3-338 ([official PDF, Vol.2A p.3-336](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=454)).
`DE F0+i` divides old `ST(0)` by old `ST(i)` before popping old `ST(0)`;
exact results clear C1, while C0/C2/C3 are undefined. General rounding,
architectural exceptions, special values, CR0/#NM and native x87 behavior are
not modeled.

## Local S2-30t: exact-only `FDIVR ST(0), ST(i)` (`D8 F8+i`)

The interpreter accepts exactly bare `D8 F8` through `D8 FF` with an existing
`Some(X87State)`. It requires FCW `0x037f`, `FSW & 0x80ff == 0`, Valid tags
for the used physical operands and canonical Normal binary80 inputs. The
ordered exact division gate computes old `ST(i)/ST(0)`: it normalizes both
significands, requires zero quotient remainder before packing, packs at
precision 64 and accepts only canonical Normal output. Unequal exponents are
allowed. Zero operands, specials, noncanonical raw values, inexact quotients,
overflow and subnormal/zero output reject before mutation. `D8 F9` encodes
`FDIVR ST(0), ST(1)`; `i=0` reads one Valid physical TOP
tag and yields exact `+1` for either sign without popping or changing TOP/tag.

Success writes the result to old physical TOP, clears C1, records starting RIP
as FIP, advances RIP by two and retires once; all other tags, payloads, x87
fields, GPRs and RFLAGS remain unchanged. The decoder accepts D8 F8..FF while
preserving the other D8 register ranges; prefixes, REX, WAIT and truncation
remain unsupported. The fixture schema and runner cover all eight encodings
and atomic inexact `D8 F9` rejection with no stop RIP and budget one. Native
admission remains closed.

Normative source: Intel SDM revision 093, Vol.2A,
“FDIVR/FDIVRP/FIDIVR—Reverse Divide,”
pp.3-336–3-338 ([official PDF, Vol.2A p.3-336](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=454)).
`D8 F8+i` divides old `ST(i)` by old `ST(0)` without popping; exact results
clear C1, while C0/C2/C3 are undefined. General rounding, architectural
exceptions, special values, CR0/#NM and native x87 behavior are not modeled.

## Local S2-30u: exact-only `FDIVR ST(i), ST(0)` (`DC F0+i`)

The interpreter accepts exactly bare `DC F0` through `DC F7` with an existing
`Some(X87State)`. It requires FCW `0x037f`, `FSW & 0x80ff == 0`, Valid tags for
the used physical operands and canonical Normal binary80 inputs. The ordered
exact division gate computes old `ST(0)/ST(i)`: it normalizes both
significands, requires zero quotient remainder before packing, packs at
precision 64 and accepts only canonical Normal output. Unequal exponents are
allowed. Zero operands, specials, noncanonical raw values, inexact quotients,
overflow and subnormal/zero output reject before mutation. `DC F1` encodes
`FDIVR ST(1), ST(0)`; `i=0` reads one Valid physical TOP tag and yields exact
`+1` for either sign while retaining the tag and TOP.

Success writes the result to old physical `(TOP+i)&7`, clears C1, records
starting RIP as FIP, advances RIP by two and retires once; all other tags,
payloads, x87 fields, GPRs and RFLAGS remain unchanged. The decoder accepts DC
F0..F7 while preserving the other supported DC register ranges; prefixes,
REX, WAIT and truncation remain unsupported. The fixture schema and
interpreter runner admit these eight exact forms, including the inexact
Unsupported/no-stop case. The native backend explicitly denies all eight
encodings, so native x87 admission remains closed.

Normative source: Intel SDM revision 093, Vol.2A,
“FDIVR/FDIVRP/FIDIVR—Reverse Divide,”
pp.3-336–3-338 ([official PDF, Vol.2A p.3-336](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=454)).
`DC F0+i` divides old `ST(0)` by old `ST(i)` without popping; exact results
clear C1, while C0/C2/C3 are undefined. General rounding, architectural
exceptions, special values, CR0/#NM and native x87 behavior are not modeled.

## Local S2-30v: exact-only `FDIV ST(i), ST(0)` (`DC F8+i`)

The interpreter accepts exactly bare `DC F8` through `DC FF` with an existing
`Some(X87State)`. It requires FCW `0x037f`, `FSW & 0x80ff == 0`, Valid tags for
the used physical operands and canonical Normal binary80 inputs. The ordered
exact division gate computes old `ST(i)/ST(0)`: it normalizes both
significands, requires zero quotient remainder before packing, packs at
precision 64 and accepts only canonical Normal output. Unequal exponents are
allowed. Zero operands, specials, noncanonical raw values, inexact quotients,
overflow and subnormal/zero output reject before mutation. `DC F9` encodes
`FDIV ST(1), ST(0)`; `i=0` reads one Valid physical TOP tag and yields exact
`+1` for either sign while retaining the tag and TOP.

Success writes the result to old physical `(TOP+i)&7`, clears C1, records
starting RIP as FIP, advances RIP by two and retires once; all other tags,
payloads, x87 fields, GPRs and RFLAGS remain unchanged. The decoder accepts DC
F8..FF while preserving DC F0..F7 FDIVR and the other supported DC register
ranges; prefixes, REX, WAIT and truncation remain unsupported. The fixture
schema and interpreter runner admit these eight exact forms, including the
inexact `DC F9` Unsupported/no-stop case. The native backend explicitly denies
all eight encodings, so native x87 admission remains closed.

Normative source: Intel SDM revision 093, Vol.2A pp.3-333–3-335,
“FDIV/FIDIV—Divide” ([official PDF, Vol.2A p.3-333](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=451)). Exact division clears C1;
C0/C2/C3, general rounding, architectural exceptions, special values, CR0/#NM
and native x87 behavior are outside this local profile.

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

## Local S2-29o: finite division to one rounded binary80 result

`od_isa::x87::divide_finite_to_binary80(lhs, rhs, mode)` composes the bounded
finite-significand divider, rational precision rounder, and normal/subnormal
packers. It rejects either zero operand before division. For nonzero operands,
the output sign is `lhs.sign() XOR rhs.sign()`, and the divider's exact
quotient, remainder, and denominator are retained until rounding. If its
unrounded quotient exponent is `e >= -16382`, the helper rounds once to 64
significant bits and packs the normal result. If `e <= -16383`, it passes the
original exact quotient tail directly to `round_pack_subnormal_quotient`,
allowing a rounded zero to retain the computed sign and allowing a reachable
carry to minimum normal. `None` means only rejection by this bounded helper,
including zero operands, exponent conversion failure, or a result outside its
normal finite range; it does not mean guest #Z or overflow.

Literal composition vectors cover exact normal/subnormal values, directed
positive/negative 1/3, minimum normal, largest subnormal and its tie carry to
minimum normal, minimum subnormal, below/at/above half minimum subnormal,
signed-zero rounding, maximum finite and unsupported upper range, all four
input sign combinations, zero operands, and unchanged inputs/provenance. The
reachable anti-double-round vector uses `lhs=(e=-16382,sig=0x8000000000000002)`
and `rhs=(e=0,sig=0xc000000000000000)`; the exact divider produces
`e=-16383, q=0xaaaaaaaaaaaaaaad, r=0x4000000000000000,
d=0xc000000000000000`, and nearest rounding yields bytes
`57 55 55 55 55 55 55 55 00 00`. Normal p=64 synthetic ties/carry remain
unreachable from normalized divider outputs; the subnormal carry is reachable.

This is numerical finite division only. No x87 control-word interpretation,
status flags, architectural exceptions, special-value handling, guest #Z/
overflow, or guest FDIV is implemented. S2-29 and S2-28 remain open under
ISSUE-019.

## Local S2-29p: exact coefficient to one rounded binary80 result

`od_isa::x87::round_pack_exact_coefficient(exponent, coefficient, mode)`
rounds the exact value `coefficient * 2^(exponent - 63)` once into finite
normal or subnormal binary80. Coefficient zero is rejected because this helper
does not select a signed-zero result. For nonzero input it uses
`normalize_exact_coefficient`, whose exact even magnitude is decomposed as
`q=(magnitude >> 64)`, `r=(magnitude as u64) >> 1`, and `d=2^63`; thus
`magnitude / 2^64 = q + r/d`. If the normalized exponent is at least `-16382`,
it rounds this exact quotient once to p=64 and calls normal packing. Smaller
exponents pass the original `q/r/d` directly to subnormal packing, avoiding
intermediate rounding. `None` propagates only bounded normalization or packer
rejection.

Literal byte vectors cover signed `2^63`, normal p=64 ties with even and odd
retained values, synthetic carry to 4, signed minimum subnormal, half-minimum
subnormal under all modes, signed directed zero, largest subnormal, tie into
minimum normal, maximum finite, upper-range rejection, `i128::MIN/MAX`, and
exponent edges. The subnormal anti-double-round vector
`(exponent=-16385, coefficient=2^64+11)` rounds to significand
`0x2000000000000001`; pre-rounding the normalized coefficient would produce
the wrong last bit. S2-29e composition checks cancellation to ±1, carry, one
zero operand, unequal exponent rejection, and unchanged operands. Synthetic
inexact subnormal vectors exercise general coefficient math and are not
reachable from exact binary80 same-exponent sums.

This is a numerical coefficient adapter only. It does not define x87 RC/PC or
control-word behavior, architectural status/flags/exceptions, signed-zero
selection, special values, or guest instructions. S2-29 and S2-28 remain open
under ISSUE-019.

## Local S2-29q: same-exponent finite addition to one rounded binary80 result

`od_isa::x87::add_finite_same_exponent_to_binary80(lhs, rhs, mode)` composes
`add_finite_same_exponent` with `round_pack_exact_coefficient`. Both operands
are already-normalized `X87Finite` values. A single zero operand is neutral in
either order and the output is canonical binary80. Two zeros and exact
cancellation return `None` because no architectural zero sign is selected.
Unequal nonzero normalized exponents also return `None`; this bounded path does
not align or jam. Any other `None`, including a result above maximum finite,
means helper rejection and is not a guest exception.

Literal raw-byte vectors pass through `normalize_finite()` and cover ±1+±1,
positive and negative normal ties with even and odd retained bits under all
four mathematical modes, adjacent minimum normals subtracting to ±minimum
subnormal, maximum subnormal, two half-minimum-normal inputs summing to minimum
normal, minimum-subnormal addition, zero with normal/subnormal/pseudo-denormal
in both operand orders, maximum finite plus zero, finite-range rejection,
cancellation, all four signed-zero pairs, and unequal exponents. Inputs remain
unchanged, including their original raw encodings and source-class provenance.

For two significands no larger than `2^64-1`, the exact signed-sum magnitude is
at most `2^65-2`; the generic coefficient rounder's synthetic `2^65-1`
carry-to-four input is unreachable from this operation. Every finite binary80
value is an integer multiple of minimum subnormal, so exact sums in the
subnormal range are minimum-subnormal multiples as well. Inexact subnormal
tails, half-minimum-subnormal ties, and underflow-to-zero cannot arise here.
This helper does not implement FADD/FSUB decode or execution, stack behavior,
x87 control-word mapping, flags, exceptions, special values, or architectural
zero selection. S2-29, S2-28, and ISSUE-019 remain open.

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


## Local S2-30w: bounded `FSTP ST(i)` (`DD D8+i`)

The interpreter accepts exactly bare `DD D8` through `DD DF` with an existing
`Some(X87State)`. It requires FCW `0x037f`, `FSW & 0x80ff == 0`, a Valid tag
at old physical TOP, and a canonical Normal binary80 source. The source is
captured as its original ten bytes before any mutation. No destination payload
or old destination tag is read, so all four old destination tag values are
accepted when `i != 0`.

Success copies the exact bytes to old physical `(TOP+i)&7`, marks that physical
tag Valid, then marks old TOP Empty and advances TOP modulo eight without
rotating the physical register array. For `i=0`, the old TOP payload receives
the source bytes and its final tag is Empty. For `i=1`, the old ST(1) destination
becomes new ST(0). FIP is set to the instruction-start RIP and RIP advances by
two. C1 is preserved as an explicit local policy; remaining FSW bits other than
TOP, FCW, FDP, FOP, GPRs, RFLAGS and unrelated physical payloads/tags are
preserved.

Invalid source tag/class, invalid FCW/FSW, missing x87 state, unsupported
encoding, fetch fault/truncation, STOP and exhausted budget do not commit state
or retire. No memory form, numeric conversion/rounding, stack-fault or `#NM`
delivery, or native x87 admission is modeled.

Normative source: Intel SDM revision 093, Vol.2A, “FST/FSTP—Store Floating-Point
Value,” pp.3-386–3-387 ([official PDF, Vol.2A p.3-386](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=504)). Intel defines `DD D8+i` as copying ST(0) to ST(i) and popping; this local contract narrows the accepted source/environment and preserves C1 concretely.

## Local S2-30x: bounded `FST ST(i)` (`DD D0+i`)

The interpreter accepts exactly bare `DD D0` through `DD D7` with an existing
`Some(X87State)`. It reuses the bounded x87 store source gate: FCW `0x037f`,
`FSW & 0x80ff == 0`, a Valid tag at old physical TOP, and a canonical Normal
binary80 source. The source is captured as its original ten bytes before any
mutation. No destination payload or old destination tag is read, so all four
old destination tag values are accepted when `i != 0`.

Success copies the exact bytes to old physical `(TOP+i)&7` and marks that
physical tag Valid without rotating the register array. TOP and every FSW bit
are preserved; for `i=0`, the source bytes and Valid tag remain unchanged. FIP
is set to the instruction-start RIP and RIP advances by two. FCW, FDP, FOP,
unrelated physical payloads/tags, GPRs and RFLAGS are preserved.

Invalid source tag/class, invalid FCW/FSW, missing x87 state, unsupported
encoding, fetch fault/truncation, STOP and exhausted budget do not commit state
or retire. No memory form, numeric conversion/rounding, stack-fault or `#NM`
delivery, or native x87 admission is modeled. Preserving C1 and the other FSW
bits is a deterministic local policy; this bounded slice is not claimed to
have hardware validation.

Decoder tests enumerate all DD ModR/M bytes, preserve `DD D8..DF` as FSTP,
and reject prefixes, REX, WAIT and truncation. `od-isa` vectors cover every
TOP/index pair and each previous destination tag for `i!=0`, raw signed and
Normal-edge bytes, full FSW preservation, and full-state atomic rejection.
The fixture schema admits all eight encodings and a source-tag rejection case
(`DD D1`, no stop RIP, budget one); CLI runner checks cover all destination
indices. Native admission explicitly denies all eight encodings.

Normative source: Intel SDM revision 093, Vol.2A, “FST/FSTP—Store Floating-Point
Value,” pp.3-386–3-387 ([official PDF, Vol.2A p.3-386](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=504)). Intel defines `DD D0+i` as copying ST(0) to ST(i); this local slice narrows accepted source/environment and preserves all FSW bits concretely.

## Local S2-30y: bounded `FCHS` (`D9 E0`)

The interpreter accepts exactly bare `D9 E0` with an existing `Some(X87State)`.
It requires FCW `0x037f`, `FSW & 0x80ff == 0`, a Valid tag at old physical
TOP, and canonical Normal binary80 ST(0). Before mutation it snapshots the
source's ten bytes; success changes only `bytes[9] ^= 0x80` in old physical
TOP. It clears FSW C1 (bit 9) while preserving TOP, all tags and every other
state field. C0/C2/C3 retention is a deterministic local snapshot policy.
FIP receives the instruction-start RIP, RIP advances by two, and the operation
retires once.

Invalid/missing environment, tag or source class, every other D9 second byte,
prefix/REX/WAIT, truncation, STOP and exhausted budget do not commit state.
Zero, subnormal and special values are unsupported in the S2-30y Normal-only
slice; the separate S2-30ao addition admits canonical signed zero. No memory form, CR0/#NM,
exception delivery, native admission or FPU feature claim is included.

Local coverage enumerates all 256 second bytes with only E0 accepted and checks
all TOP positions, literal positive/negative minimum/maximum/full-significand
Normal payloads, exact sign-bit-only changes, initial C1 clear/set, and double
FCHS payload restoration with C1 remaining clear. Rejection tests compare full
state. The schema and CLI have one success fixture and one actual source-tag
rejection with `stop_rip: null` and instruction budget one. Native x87
admission denies `D9 E0`.

Normative source: Intel SDM revision 093, Vol.2A, “FCHS—Change Sign,” p.3-318
([official PDF, Vol.2A p.3-318](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=436)).

## Local S2-30ao: signed-zero `FCHS` admission (`D9 E0`)

FCHS retains the S2-30y Normal behavior and additionally accepts ST(0) only
when its tag is Zero and its raw ten-byte value is exact signed zero: bytes
0..7 are zero, and bytes 8..9 are either `00 00` or `00 80`. FCHS toggles only
byte 9 bit 7. It preserves the Zero tag, TOP and all other state/payload bytes,
clears C1, retains C0/C2/C3 as a local snapshot policy, records instruction
start RIP in FIP, advances RIP by two and retires once. At that historical
point FTST and FST/FSTP ST(i) still used the Normal-only gate; S2-30ap separately
extends FABS, and S2-30aq/ar/as later extend FTST/FST/FSTP respectively.
Other tags/classes/payloads, invalid controls/status and execution-boundary
failures reject atomically. Native/JIT admission stays closed. Evidence is
local interpreter/unit and project-fixture coverage, not physical x86 evidence;
physical acceptance remains under ISSUE-019.

## Local S2-30z: bounded `FABS` (`D9 E1`)

The interpreter accepts exactly bare `D9 E1` with an existing
`Some(X87State)`. It reuses the bounded canonical-Normal ST(0) source gate:
FCW `0x037f`, `FSW & 0x80ff == 0`, Valid tag at old physical TOP, and canonical
Normal binary80 source bytes. It snapshots all ten raw bytes and clears only
the sign bit with `source[9] &= 0x7f` in old physical TOP. Success clears FSW
C1 (bit 9), preserves TOP, tags and all other state, records start RIP in FIP,
advances RIP by two, and retires once. Retention of C0/C2/C3 is the deterministic
local snapshot policy.

Missing/bad state, invalid control/status, non-Valid tag, non-Normal source,
every other D9 second byte, prefixes/REX/WAIT, truncation, fetch failure, STOP
and exhausted budget do not commit state. Zero/subnormal/special values,
memory forms, CR0/#NM, exception delivery, native admission and FPU feature
claims are excluded.

Tests cover all TOP values, both source signs, minimum/maximum/full-significand
Normal payloads, C1 initially clear/set, preserved C0/C2/C3, idempotence, full-
state rejections and execution boundaries. Exhaustive D9 decoding accepts E0
as FCHS and E1 as FABS only; fixture schema and CLI cover success plus a real
Empty-tag rejection with `stop_rip: null` and budget one. Native x87 admission
denies `D9 E1`.

Normative source: Intel SDM revision 093, Vol.2A, “FABS—Absolute Value,” p.3-309
([official PDF, Vol.2A p.3-309](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=427)).

## Local S2-30ap: signed-zero `FABS` admission (`D9 E1`)

FABS retains the S2-30z Normal behavior and additionally accepts ST(0) only
when its tag is Zero and its raw ten-byte value is exact signed zero: bytes
0..7 are zero, and bytes 8..9 are either `00 00` or `00 80`. FABS clears only
byte 9 bit 7, so either signed zero becomes positive zero. It preserves the
Zero tag, TOP, other payloads and unrelated state, clears C1, retains C0/C2/C3
as the local snapshot policy, records FIP, advances RIP by two and retires
once. FCHS and FABS share only this narrow sign-operation source gate;
FTST's separate signed-zero extension is specified in S2-30aq. At this
historical point FST/FSTP ST(i) remained on the canonical-Normal-only gate;
S2-30ar later extends bare FST, and S2-30as supersedes the FSTP limit.

Malformed Zero payloads, wrong tags, invalid FCW/FSW, missing state, and
fetch/prefix/truncation/STOP/budget boundaries reject atomically. Coverage
includes both signed zeros × all TOP × all 16 condition-code states, repeated
FABS/full-state idempotence, Normal and FCHS regressions, and FLD m32/m64 →
FABS → FST → FNSTSW. The handwritten project fixture is exactly
`d900d9e1d910dfe0` and retires four instructions. Native/JIT remain closed;
these local results do not establish physical x87 behavior or broader
architectural semantics under ISSUE-019.

## Local S2-30aa: bounded `FTST` (`D9 E4`)

The interpreter accepts exactly bare `D9 E4` with an existing
`Some(X87State)`, FCW `0x037f`, `FSW & 0x80ff == 0`, Valid tag at old physical
TOP, and canonical-Normal binary80 ST(0) from
`bounded_x87_normal_st0_source`. Positive Normal sets C3/C2/C0 to `000`; a
negative Normal sets them to `001`. Both clear C1. The whole condition-code
field is replaced as `(old_fsw & !0x4700) | (negative ? 0x0100 : 0)`, so no
previous C0/C1/C2/C3 value survives. TOP, all physical raw payloads, tags and
all other x87/CPU fields are preserved. FIP receives instruction-start RIP,
RIP advances by two, and the operation retires once.

For this historical S2-30z slice, missing state, invalid FCW/FSW, non-Valid tag, non-Normal source, every other
D9 second byte, prefixes/REX/WAIT, truncation, fetch failure, STOP and exhausted
budget reject without state commit. Signed zero, subnormal/special values,
memory forms, exceptions, stack faults, CR0/#NM, native admission and FPU
feature claims are outside this slice. S2-30ap later admits exact Zero-tagged
signed zero to FABS, and S2-30ar later admits it to bare FST ST(i).

Coverage enumerates all 256 D9 second bytes, accepting E0 as FCHS, E1 as FABS,
and E4 as FTST only; it checks all TOP values, both signs, minimum/maximum and
full-significand canonical Normals, all 16 initial condition-code combinations,
raw payload/tag sentinels, full-state rejection boundaries, and FTST followed
by FNSTSW AX. Two literal success fixtures cover positive and negative results;
an Empty-tag fixture rejects with no stop RIP and instruction budget one. The
schema and CLI admit these fixtures. Native x87 admission explicitly denies
D9 E4.

Normative source: Intel SDM revision 093, Vol.2A, “FTST—Test,” pp.3-400–3-401
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

## Local S2-30aq: canonical signed-zero `FTST` (`D9 E4`)

The exact bare `D9 E4` interpreter form retains the existing x87-state,
FCW `0x037f`, and `FSW & 0x80ff == 0` gates. It accepts canonical-Normal
Valid ST(0), plus only exact Zero-tagged signed zero: raw significand bytes
0..7 are zero and the final two bytes are `00 00` or `00 80`. A Normal value compares less or greater according to its
sign. Either signed zero compares equal and replaces the condition field with
`(old_fsw & !0x4700) | 0x4000`, setting C3 and clearing C0/C1/C2. It preserves
the sign and all raw source bytes, Zero tag, TOP, FCW, FDP, FOP, all other x87
and CPU state, records FIP, advances RIP by two, and retires once.

At this historical point this admission was FTST-specific and FST/FSTP ST(i)
remained canonical-Normal-only. S2-30ar later extends bare FST; FSTP remains
Normal-only.
Malformed Zero payloads, mismatched tags, special/subnormal values, bad FCW or
FSW, missing state, prefixes/REX/WAIT, malformed opcode/truncation, fetch
failure, STOP, and exhausted budget reject without state commit. Tests cover
both signed zeros × all TOP × all 16 prior condition-code combinations,
repeated idempotence, and m32/m64 FLD→FTST→FNSTSW compositions for both signs,
plus Normal regressions and full-state boundaries. The literal project
composition is exactly `d900d9e4dfe0`: FLD m32fp → FTST → FNSTSW,
three retirements. One-op admission remains outside the native profile, and
native/JIT execution remain closed. Local interpreter and fixture evidence do
not establish physical x87 behavior; ISSUE-019 remains open.

## Local S2-30at: canonical signed-infinity `FTST` (`D9 E4`)

Bare `D9 E4` retains the existing `Some(X87State)`, FCW `0x037f`, and
`FSW & 0x80ff == 0` gates and additionally admits only Special-tagged canonical
binary80 infinity at ST(0): exponent `0x7fff`, explicit integer bit `J=1`, and
zero fraction. Negative infinity compares Less and sets C0; positive infinity
compares Greater and clears C0. The existing whole-condition-field commit
`(old_fsw & !0x4700) | (negative ? 0x0100 : 0)` clears C1/C2/C3 and retains
other FSW bits. TOP, all raw register bytes, tags and unrelated state remain
unchanged; FIP receives instruction-start RIP, RIP advances by two, and one
instruction retires. Existing canonical-Normal and Zero-tagged signed-zero
behavior is unchanged. FCHS/FABS/FST/FSTP gates are not widened.

Pseudo-infinity (J=0), NaNs, subnormal/unsupported/malformed encodings, wrong
tags, invalid environment, missing x87 state, and decode/fetch/prefix/REX/WAIT/
truncation/STOP/budget failures remain atomic rejects. Interpreter coverage
crosses both infinity signs × all TOP values × all 16 old condition-code fields
and repeated execution, with the existing Normal/Zero regressions. Two
handwritten `project_fixture` cases reuse exactly `d900d9e4dfe0`, loading
binary32 positive or negative infinity before FTST and FNSTSW. No schema
sequence was added. JIT and native execution remain denied; these local checks
do not establish physical x87 behavior. Normative instruction source: Intel
SDM revision 093, Vol.2A, “FTST—Test,” pp.3-400–3-401
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

## Local S2-30ar: canonical signed-zero `FST ST(i)` (`DD D0+i`)

The exact bare `DD D0+i`, `i=0..7`, interpreter form retains the existing
x87-state, FCW `0x037f`, and `FSW & 0x80ff == 0` gates and Valid-tag
canonical-Normal source behavior. It additionally accepts only Zero-tagged
exact signed zero: the first nine raw bytes are zero and the last byte is
`0x00` or `0x80`. The source's ten bytes and tag are captured before mutation,
copied to physical `(old_TOP+i)&7`, and the destination tag is set to Zero for
signed zero or Valid for Normal. For `i!=0`, the destination's prior payload and
tag are ignored. For `i=0`, the same bytes and Zero tag remain in place. No
physical register rotation or TOP change occurs.

TOP, every FSW bit, FCW, FDP, FOP, unrelated physical payloads/tags, GPRs and
RFLAGS are preserved; retaining condition bits including C1 is an explicit
deterministic local policy. FIP receives instruction-start RIP, RIP advances
by two, and the instruction retires once. `FSTP ST(i)` remained on the prior Normal-only source gate at this historical
point; S2-30as later extends it. Wrong tags, malformed Zero payloads, bad FCW/FSW,
missing state, unsupported encodings, fetch fault/truncation, STOP and
exhausted budget reject without state commit. Native/JIT admission remains
closed.

Coverage includes both signed zeros × all TOP × all indices × all 16 prior
condition-code states, sentinel old destination payloads/tags, alias and
idempotence, retained Normal behavior, historical FSTP zero rejection for all indices (superseded by S2-30as),
and atomic bad-source/environment/execution-boundary checks. Local m32/m64
FLD→FST ST(1)→FNSTSW compositions and exact fixture `d900ddd1dfe0` are checked;
the fixture retires three instructions. These local results do not establish
physical x87 behavior; ISSUE-019 remains open.



## Local S2-30as: signed-zero through `FSTP ST(i)` (`DD D8+i`)

The shared bounded FST/FSTP register-store source gate admits either a Valid-tag
canonical Normal or a Zero-tag exact signed zero. Exact zero has bytes 0..8 all
zero and byte 9 equal to `0x00` or `0x80`. Both operations retain the existing
FCW `0x037f` and `FSW & 0x80ff == 0` requirements; other Normal-only x87
consumers are unchanged.

For bare `DD D8+i`, capture the source's ten bytes and tag, old physical TOP,
and `i` before mutation. Copy bytes and tag to physical `(old_TOP+i)&7`, then
mark old physical TOP Empty, then set TOP to `(old_TOP+1)&7`. The array is not
rotated. Thus the `i=0` payload remains but its final tag is Empty; for `i=1`
the copied value becomes new ST(0). Preserve FSW bits other than TOP, including
C1, FCW, FDP, FOP, GPRs, RFLAGS and unrelated payloads/tags. Set FIP to the
instruction RIP, advance RIP by two, and retire once.

Coverage includes both signed zeros × all eight TOP positions × all indices ×
all 16 condition-code combinations, full-state comparisons, previous destination
payload/tag sentinels for `i!=0`, alias `i=0`, `i=1`, TOP=7 wrap, existing
Normal cases, malformed Zero, Zero-tagged Normal, wrong tags, special classes,
bad controls, missing state, prefixes/REX/WAIT, truncation, fetch fault, STOP,
and budget rejection. Local m32/m64 FLD→FSTP ST(1)→FNSTSW compositions cover
both signs. The only added fixture stream is exactly `d900ddd9dfe0`, six bytes
and three retirements in the existing x87 fixture profile. Native/JIT remain
closed; these local checks do not establish physical x87 behavior, and ISSUE-019
remains open.

The former Normal-only FST/FSTP source limit is superseded by S2-30au below.

## Local S2-30au: canonical signed infinity through `FST/FSTP ST(i)`

The shared bounded register-store source gate accepts three exact tag/class
pairs: Valid-tag canonical Normal, Zero-tag exact signed zero, and
Special-tag canonical binary80 infinity. Infinity requires E=`0x7fff`, J=1,
and zero fraction, with either sign. FCW remains exactly `0x037f` and
`FSW & 0x80ff` must be zero; all admitted source bytes and tags are copied
unchanged. C1 continues to follow the existing local policy: FST preserves
every status bit, while FSTP preserves all bits except that TOP advances.

Bare `DD D0+i` copies the ten bytes and Special tag to physical
`(old_TOP+i)&7`, preserves TOP, and records FIP. Bare `DD D8+i` copies first,
marks old physical TOP Empty, and advances TOP modulo eight. The register array
is not rotated: with `i=0` the copied infinity payload remains but its final
tag is Empty; with `i=1` the copy becomes new ST(0). Pseudo-infinity, NaNs,
subnormal, malformed encodings, and wrong tags remain atomic Unsupported.
JIT/native admission is unchanged and remains closed.

## Local S2-30ab: bounded `FCOM ST(1)` (`D8 D1`)

The interpreter accepts exactly the unprefixed two-byte encoding `D8 D1` with
an existing `Some(X87State)`, FCW `0x037f`, `FSW & 0x80ff == 0`, Valid tags at
both old physical TOP (lhs ST(0)) and `(TOP+1)&7` (rhs ST(1)), and canonical
Normal binary80 operands. Both operands are fully validated before any state
change. Comparison uses the finite exponent and all 64 significand bits; it
does not convert through host floating point or subtract the operands.

The result replaces the complete condition-code field with C0/C1/C2/C3 values
`0000` when ST(0)>ST(1), `0001` when ST(0)<ST(1), and `1000` when equal
(bit order C3,C2,C1,C0). Commit is `(old_fsw & !0x4700) | result`; this clears
C1/C2 and replaces all four condition bits. TOP, tags, all physical raw
payloads, FCW, every other FSW bit, FDP/FOP and all remaining CPU state are
preserved. FIP receives instruction-start RIP, RIP advances by two, and the
instruction retires once.

For the original S2-30ab slice, missing x87 state/environment, either invalid
tag or either non-Normal operand, other comparison forms and unsupported D8
encodings, prefixes/REX/WAIT, truncation, fetch failure, STOP and exhausted
budget reject without state commit. Existing D8 arithmetic forms remain
admitted. S2-30ac below generalizes the supported register comparison forms.
No pop, FCOMP/FCOMPP/
FUCOM, memory operand, NaN/zero/subnormal behavior, exception delivery,
#NM/WAIT, native admission or CPUID/FPU feature claim is included.

Coverage includes all eight TOP values, all three orderings, positive and
negative same-sign comparisons, opposite signs in both orders, exponent and
low-significand-bit ordering, minimum/maximum Normal boundaries, every initial
condition-code combination, complete physical-register sentinels, independent
invalid tag/class/environment rejection for both operands, and fetch/truncate/
STOP/budget atomicity. Three literal fixtures cover greater/equal/less results;
an Empty-tag ST(1) fixture is a real `stop_rip: null`, budget-one rejection.
The CLI also checks `FCOM ST(1)` followed by `FNSTSW AX`. The S2-30ab schema
admission was limited to this instruction and observation sequence; the schema
is generalized to all eight register forms by S2-30ac below. Native x87
admission explicitly denies the instruction.

Normative source: Intel SDM revision 093, Vol.2A,
“FCOM/FCOMP/FCOMPP—Compare Floating-Point Values,” pp.3-324–3-326
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

## Local S2-30ad: bounded `FCOMP ST(1)` alias (`D8 D9`)

This local form admits exactly the bare two-byte encoding `D8 D9`, the
zero-operand FCOMP alias for comparison of old ST(0) with old ST(1). It reuses
the bounded S2-30ac compare gate: x87 state must exist, FCW must be `0x037f`,
`FSW & 0x80ff` must be clear, and both old physical operands must have Valid
tags and canonical-Normal binary80 payloads. The comparison uses their old
values and exact sign/exponent/64-bit-significand ordering.

After the comparison is fully preflighted, set FSW condition bits to
`(old_fsw & !0x4700) | result`, with greater=`0`, less=`0x0100`, equal=`0x4000`
(therefore C1 clears). Mark the old physical TOP tag Empty, advance TOP to
`(old_top+1)&7`, and preserve every raw register payload without rotation or
clearing. All other physical tags, FCW, the remaining FSW fields, FDP/FOP and
other CPU state remain unchanged. FIP receives instruction-start RIP, RIP
advances by two and the instruction retires once. Missing state/environment,
invalid tags/classes, fetch/truncation, STOP and budget rejection leave full
state unchanged.

Only `D8 D9` is added to the D8 map. Existing `D8 D0..D7` FCOM and D8
arithmetic ranges remain intact; `D8 D8` and `D8 DA..DF` remain unsupported.
Prefixes, REX, WAIT, memory operands, other FCOMP/FCOMPP forms, special values,
exception delivery, native execution and FPU feature claims are outside this
slice. Literal coverage includes every TOP (including wrap at TOP=7), all 16
initial condition-code combinations, greater/less/equal, sign/exponent/low
significand distinctions, independent lhs/rhs tag and class rejection,
sentinels, and fetch/truncation/STOP/budget atomicity. The fixture schema and
CLI admit only `D8 D9` plus the exact `D8 D9 DF E0` FNSTSW AX observation
sequence; literal fixtures cover success and no-stop/budget-one invalid rhs.
Native admission explicitly denies D8 D9.

Normative source: Intel SDM revision 093, Vol.2A,
“FCOM/FCOMP/FCOMPP—Compare Floating-Point Values,” pp.3-324–3-326
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

## Local S2-30ae: bounded `FCOMP ST(i)` (`D8 D8+i`)

This local form admits exactly the eight bare two-byte encodings `D8 D8`
through `D8 DF`. The left operand is old physical TOP (ST(0)); the right
operand is old physical `(TOP+i)&7`. It reuses the bounded S2-30ac exact
compare gate: x87 state must exist, FCW must be `0x037f`, `FSW & 0x80ff` must
be clear, and both old physical operands must have Valid tags and
canonical-Normal binary80 payloads. For `i=0`, validate the shared physical
register before returning equality, then perform the same single pop.
Comparison uses exact sign/exponent/full-64-bit-significand ordering.

After full preflight, set FSW condition bits to `(old_fsw & !0x4700) | result`,
with greater=`0`, less=`0x0100`, equal=`0x4000` (therefore C1 clears). Mark the
old physical TOP tag Empty, advance TOP to `(old_top+1)&7`, and preserve every
raw physical register payload without rotation or clearing. All other tags,
FCW, remaining FSW fields, FDP/FOP and other CPU state remain unchanged. FIP
receives instruction-start RIP, RIP advances by two and the instruction
retires once. Missing state/environment, invalid tags/classes,
fetch/truncation, STOP and budget rejection leave full state unchanged.

The D8 map adds only `D8 D8..DF` for FCOMP; existing `D8 D0..D7` FCOM and D8
arithmetic ranges remain intact. Prefixes, REX, WAIT, memory operands, other
FCOMP/FCOMPP forms, special values, exception delivery, native execution and
FPU feature claims are outside this slice. Literal and interpreter coverage
includes all 64 TOP×i pairs (including i=0 and TOP=7 wrap), all 16 initial
condition-code combinations, greater/less/equal, sign/exponent/low-significand
distinctions, independent lhs/rhs tag and class rejection, environment gates,
payload/tag sentinels, and fetch/truncation/STOP/budget atomicity. The fixture
schema admits all eight encodings and each exact `D8 D8+i DF E0` FNSTSW AX
observation sequence with budgets one and two. Literal success fixtures cover
every index; the invalid-rhs fixture rejects with no stop RIP and budget one.
Native admission explicitly denies all eight FCOMP forms.

Normative source: Intel SDM revision 093, Vol.2A,
“FCOM/FCOMP/FCOMPP—Compare Floating-Point Values,” pp.3-324–3-326
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

## Local S2-30ac: bounded `FCOM ST(i)` (`D8 D0+i`)

This slice generalizes S2-30ab to exactly the eight bare two-byte encodings
`D8 D0` through `D8 D7`. The left operand is old physical TOP (ST(0)); the
right operand is old physical `(TOP+i)&7` (ST(i)). The existing gate remains:
an x87 state must exist, FCW must be `0x037f`, `FSW & 0x80ff` must be clear,
both operand tags must be Valid, and both raw values must be canonical Normal
binary80. The `i=0` alias validates the shared tag and Normal payload once and
returns equality.

Ordering uses sign, normalized finite exponent and all 64 significand bits,
without host floating-point conversion or subtraction. Greater returns `0`,
less returns `0x0100`, and equal returns `0x4000`; commit is
`(old_fsw & !0x4700) | result`. TOP, tags, all physical raw payloads, FCW,
other FSW bits, FDP/FOP and all remaining CPU state are preserved. FIP gets
instruction-start RIP, RIP advances by two, and the instruction retires once.
Both operands are fully preflighted before commit; rejected inputs leave full
state unchanged.

Only `D8 D0..D7` are added to the D8 ModR/M map. Existing D8 arithmetic ranges
remain intact, while at the S2-30ac stage `D8 D8..DF` were unsupported (S2-30ae
adds the bounded FCOMP register forms below). Prefixes, REX, WAIT, memory operands, pop forms, NaN/special/zero/subnormal inputs,
exception delivery, native execution and FPU feature claims are outside scope.

Coverage includes all 64 TOP×i placements, i=0 alias and TOP wrap, all three
relations, sign/exponent/low-significand ordering, Normal endpoints, all
initial condition-code combinations, independent tag/class rejection for
both non-alias operands, alias failure, environment rejection, sentinels and
fetch/truncation/STOP/budget atomicity. Schema and CLI vectors cover every
index, the exact `D8 D0+i DF E0` observation sequence, and a no-stop,
budget-one invalid-tag rejection. Native admission explicitly denies all
eight compare encodings.

Normative source: Intel SDM revision 093, Vol.2A,
“FCOM/FCOMP/FCOMPP—Compare Floating-Point Values,” pp.3-324–3-326
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

## Local S2-30af: bounded `FCOMPP` (`DE D9`)

This slice admits exactly bare `DE D9`. It reuses the bounded canonical-Normal
compare gate for old physical ST(0) and ST(1): x87 state exists, FCW is
`0x037f`, `FSW & 0x80ff` is clear, and both old physical tags are Valid with
canonical Normal binary80 payloads. Both operands are fully preflighted before
any mutation.

The compare replaces C0/C1/C2/C3 with greater=`0`, less=`0x0100`, equal=`0x4000`
using `(old_fsw & !0x4700) | result`. Commit marks old physical TOP and
`(TOP+1)&7` Empty, advances TOP to `(old_top+2)&7`, and does not rotate or
rewrite any raw payload. All other six tags, FCW, remaining FSW fields, FDP/FOP,
GPRs, RFLAGS and other CPU state are preserved. FIP receives instruction-start
RIP; RIP advances by two and the instruction retires once. Rejection leaves the
complete state unchanged.

Only `DE D9` is added. Existing DE arithmetic remains intact; `DE D8` and
`DE DA..DF` remain unsupported. Prefixes, REX, WAIT, truncation/fetch failure,
memory operands, specials, exceptions, native execution and FPU feature claims
remain outside this slice. Literal interpreter coverage checks all TOP values
including 6→0 and 7→1, all 16 initial condition fields and three relations,
physical payload/tag sentinels (including an unused Empty old ST(2)), and
independent bad-lhs/bad-rhs plus environment and execution-boundary atomicity.
The schema admits exact `DE D9` and `DE D9 DF E0`; its separate no-stop,
budget-one rejection rule requires an actually invalid old ST(1) tag. Native
admission explicitly denies `Operation::Fcompp` and `DE D9`.

Normative source: Intel SDM revision 093, Vol.2A,
“FCOM/FCOMP/FCOMPP—Compare Floating-Point Values,” pp.3-324–3-326
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

## Local S2-30ag: bounded `FUCOM ST(0), ST(1)` (`DD E1`)

The local profile accepts exactly bare `DD E1` and requires an existing
`Some(X87State)`, FCW `0x037f`, `FSW & 0x80ff == 0`, Valid tags at old physical
TOP and `(TOP+1)&7`, and canonical Normal binary80 payloads. Both operands are
preflighted before any state commit. Ordering reuses the exact integer
`bounded_x87_fcom_result(state, 1)` helper: greater maps to condition `0`, less
to `0x0100`, and equal to `0x4000`. Commit replaces the full condition-code
field with `(old_fsw & !0x4700) | relation`. C1 is therefore zero for this
normal case as an explicit deterministic local policy; the SDM specifies C1=0
on stack underflow but leaves its value undefined for this bounded normal case.

FUCOM does not pop. TOP, all tags, all ten-byte physical register payloads,
FCW, remaining FSW fields, FDP/FOP, GPRs and RFLAGS remain unchanged. FIP is set
to instruction-start RIP, RIP advances by two, and one instruction retires.
Missing state, invalid environment/tag/class, fetch failure/truncation, STOP or
exhausted budget leave the complete state unchanged.

Only canonical Normal finite operands are included. Zero, subnormal, infinity,
NaN and noncanonical encodings, memory forms, FUCOMP/FUCOMPP, exception
handling, stack faults, `#NM`, native execution and FPU feature claims are
excluded. Therefore FUCOM's distinct quiet-NaN comparison behavior is not
implemented by this slice. Decoder coverage enumerates all DD ModR/M bytes and
accepts only existing D0..D7 FST, D8..DF FSTP, and new E1 FUCOM; prefixes, REX,
WAIT and truncation remain rejected. Literal interpreter vectors cover all TOP
values, three relations and all 16 incoming condition fields, signed operands,
exponent ordering and low-significand differences. Independent lhs/rhs tag and
class rejects plus environment and execution-boundary rejects are atomic.
Schema/CLI fixtures cover `dde1`, `dde1dfe0`, and a genuine invalid-rhs-tag
no-stop/budget-one rejection. Native admission explicitly denies the operation.

Normative source: Intel SDM revision 093, Vol.2A,
“FUCOM/FUCOMP/FUCOMPP—Unordered Compare Floating-Point Values,” pp.3-402–3-404
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

## Local S2-30ah: bounded `FUCOMP ST(0), ST(1)` (`DD E9`)

Accept exactly bare `DD E9` with existing `Some(X87State)`. Before changing
status, tags, or TOP, require FCW `0x037f`, `FSW & 0x80ff == 0`, Valid tags at
old physical TOP and `(TOP+1)&7`, and canonical Normal binary80 operands using
`bounded_x87_fcom_result(state, 1)`. Greater maps to `0`, less to `0x0100`, and
equal to `0x4000`; commit the condition field as `(old_fsw & !0x4700) | result`.
C1 is zero by explicit deterministic local policy for this bounded normal case.
Then set only the old physical TOP tag to Empty and TOP to `(old+1)&7`. Preserve
the physical register array byte-for-byte, including old ST(1), and preserve
all other tags, FCW, unrelated FSW bits, FDP/FOP, GPRs and RFLAGS. FIP becomes
instruction-start RIP, RIP advances by two, and one instruction retires.

Any missing state, invalid environment/tag/class, fetch failure/truncation,
STOP, or exhausted budget rejects without a state commit. Decoder acceptance
adds only DD E9 and retains DD D0..D7 FST, DD D8..DF FSTP, and DD E1 FUCOM;
DD E8, DD EA..EF, prefixes, REX and WAIT remain unsupported. Zero, subnormal,
infinity, NaN, noncanonical encodings, memory forms, exception delivery, stack
faults, `#NM`, native execution and FPU feature claims are excluded. In
particular, FUCOM's distinct quiet-NaN behavior is outside the Normal-only
domain.

Literal fixtures cover exact `dde9`, `dde9dfe0`, and a genuine invalid-old-ST(1)
tag rejection with no stop RIP and budget one. Interpreter vectors cover all
TOP × relation × initial-condition combinations, TOP wrap, untouched physical
payload sentinels, independent invalid operand tags/classes, environment and
execution-boundary atomicity. The v2 schema ties the rejection rule to the
actual old physical ST(1) tag, and native admission explicitly denies DD E9.

Normative source: Intel SDM revision 093, Vol.2A,
“FUCOM/FUCOMP/FUCOMPP—Unordered Compare Floating-Point Values,” pp.3-402–3-404
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

## Local S2-30ai: bounded `FUCOMPP` (`DA E9`)

Accept exactly bare `DA E9` when `Some(X87State)` exists. Before mutation,
require FCW `0x037f`, `FSW & 0x80ff == 0`, Valid tags at the old physical TOP
and `(TOP+1)&7`, and canonical Normal values for both operands through
`bounded_x87_fcom_result(state, 1)`. Greater sets the condition field to `0`,
less to `0x0100`, and equal to `0x4000` via
`(old_fsw & !0x4700) | relation`. Clearing C1 for this normal case is the
bounded profile's explicit local deterministic policy.

After both operands pass, set both old physical tags to Empty and set TOP to
`(old+2)&7`. Keep all ten-byte physical register payloads untouched and do not
rotate the register array. Preserve all other tags, FCW, unrelated FSW bits,
FDP/FOP, GPRs and RFLAGS. Set FIP to the instruction-start RIP, advance RIP by
two and retire exactly once.

Missing state, invalid environment/tag/class, every other DA ModR/M, prefixes,
REX, WAIT, fetch failure/truncation, STOP and exhausted budget reject without a
state commit. DA E9 is the only new DA form; DE D9 FCOMPP and all other existing
forms retain their behavior. Zero, subnormal, infinity, NaN, noncanonical
encodings, exception delivery, stack faults, `#NM`, native execution and FPU
feature claims are excluded.

Interpreter coverage spans all eight TOP values, three relations and all 16
incoming condition fields, including TOP wrap from 6 to 0 and 7 to 1. An unused
old ST(2) may be Empty with arbitrary payload. Independent invalid operand tags
and classes, environment, fetch/truncation/STOP/budget boundaries reject
atomically. Schema and literal fixtures admit exact `dae9`, exact
`dae9dfe0`, and a genuine invalid old physical ST(1)-tag rejection with no
stop RIP and budget one. Native admission explicitly denies DA E9.

The Intel SDM revision 093 Vol.2A opcode table and prose on pp.3-402–3-404
specify two pops, while the pseudocode on p.3-403 depicts one. This bounded
slice follows the explicit opcode table/prose. C1=0 is local policy for the
accepted normal case.

Normative source: Intel SDM revision 093, Vol.2A,
“FUCOM/FUCOMP/FUCOMPP—Unordered Compare Floating-Point Values,” pp.3-402–3-404
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

## Local S2-30aj: bounded `FCOMI ST(0), ST(1)` (`DB F1`)

Accept exactly bare `DB F1`. Before mutation, require `Some(X87State)`, FCW
`0x037f`, `FSW & 0x80ff == 0`, Valid tags at old physical TOP and
`(TOP+1)&7`, and canonical-Normal binary80 operands by
`bounded_x87_fcom_result(state, 1)`. Greater sets relation flags to zero, less
sets CF, and equal sets ZF. Clear CF/PF/AF/ZF/SF/OF before setting the relation;
`FlagEffects` defines all six bits (`0x8d5`) on successful retirement.

Clear only FSW.C1. Preserve C0/C2/C3, TOP, all physical tags and ten-byte
payloads, FCW, FDP/FOP, every other FSW bit, all GPRs and memory. Set FIP to
instruction-start RIP, advance RIP by two and retire once. Missing state,
invalid environment/tag/class, other unadmitted DB encodings, prefixes, REX,
WAIT, fetch/truncation, STOP and exhausted budget reject without a state commit.
Existing DB E2 FNCLEX and DB E3 FNINIT remain admitted. Exceptions and the
other FCOMI-family forms are excluded. Native admission explicitly denies
`Operation::Fcomi` and DB F1.

Interpreter coverage spans all TOP × three relations × 16 initial FSW
condition fields, all 64 initial subsets of the affected RFLAGS bits with
definedness, invalid tags/classes for both operands, invalid environment and
execution-boundary atomicity. Literal fixture/schema coverage uses exact `dbf1`
and `dbf1dfe0`, plus no-stop/budget-one rejection for an Empty actual old ST(1)
tag. Exhaustive DB decoder tests admit only E2, E3 and F1 as bare forms, reject
every other second byte, and reject every prefix for FCOMI.

Normative source: Intel SDM revision 093, Vol.2A,
“FCOMI/FCOMIP/FUCOMI/FUCOMIP—Compare Floating-Point Values,” pp.3-327–3-329.

## Local S2-30ak: bounded `FCOMIP ST(0), ST(1)` (`DF F1`)

Accept exactly bare `DF F1`. Before mutation, require `Some(X87State)`, FCW
`0x037f`, `FSW & 0x80ff == 0`, Valid tags at old physical TOP and
`(TOP+1)&7`, and canonical-Normal binary80 operands by
`bounded_x87_fcom_result(state, 1)`. Greater sets relation flags to zero, less
sets CF, and equal sets ZF. Clear CF/PF/AF/ZF/SF/OF before setting the relation;
`FlagEffects` defines all six bits (`0x8d5`) on successful retirement.

Clear only FSW.C1, empty the tag at actual old physical TOP, and set TOP to
`(old TOP+1)&7`. Preserve the Valid tag at old physical ST(1), all payloads,
C0/C2/C3, FCW, FDP/FOP, every other FSW bit, all GPRs and memory. Set FIP to
instruction-start RIP, advance RIP by two and retire once. Missing state,
invalid environment/tag/class, other DF encodings, prefixes, REX, WAIT,
fetch/truncation, STOP and exhausted budget reject without a state commit.
The DF decoder admits only existing `DF E0` FNSTSW AX and new `DF F1` FCOMIP;
DB E2/E3/F1 keeps FNCLEX/FNINIT/FCOMI. Exceptions and all other FCOMI-family
forms remain outside this slice. Native admission explicitly denies
`Operation::Fcomip` and DF F1.

Interpreter coverage spans all TOP × three relations × 16 initial FSW
condition fields, all 64 initial subsets of the affected RFLAGS bits with
definedness, bad tags/classes, environment and instruction-boundary atomicity.
Literal fixtures use exact `dff1` and `dff1dfe0`, plus no-stop/budget-one
rejection with Empty actual old physical ST(1). Exhaustive DF decoding admits
only E0/F1 and rejects every other ModR/M byte and all prefixes.

Normative source: Intel SDM revision 093, Vol.2A,
“FCOMI/FCOMIP/FUCOMI/FUCOMIP—Compare Floating-Point Values,” pp.3-327–3-329
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

## Local S2-31q: bounded `FSTP m80fp [RAX]` (`DB 38`)

The interpreter admits only exact bare `DB 38` in Long64 with an existing x87
snapshot. The old physical TOP tag must be Valid and its original ten-byte
binary80 payload must classify as canonical Normal. Accept FCW `0x037f`,
`0x077f`, `0x0b7f`, or `0x0f7f`, and initially require `FSW & 0x80df == 0`;
S2-31ba later widens the current DB38 gate to `FSW & 0x80dc == 0`, admitting
sticky IE/DE/PE. The destination receives the exact original bytes, with no
conversion or host floating-point operation.

Before data access, validate the complete canonical48 ten-byte range
`[RAX,RAX+9]` and checked `u64` endpoint arithmetic. Then issue exactly one
atomic `GuestMemory::write(address, context, &raw)` and no data read. Only after
success, empty old physical TOP, advance TOP modulo eight, and clear C1. Preserve
PE and all other FSW bits, including C0/C2/C3 as an explicit deterministic
local policy because the SDM marks those condition codes undefined for FSTP.
Preserve every physical register payload, all other tags, FCW/FOP and unrelated
state. Record instruction-start RIP in FIP and RAX in FDP, advance RIP by two
and retire once. Architectural faults use the guest fault path; backend errors
remain diagnostics and neither path commits CPU state.

Exact schema/CLI fixtures cover `DB 38` and `DB 38 DF E0`; the code bytes define
the instruction independent of fixture names. Interpreter tests cover all TOP
values including wrap, fractional and exponent-endpoint raw payloads, every
accepted FCW, PE/C1 and condition-code retention, unaligned ten-byte storage
with canaries and a nonzero byte 11, one write/no reads, canonical range and
overflow, denied/backend/#PF stores, unsupported environment/tag/class before
access, and fetch/truncation/STOP/budget boundaries. Prefixes, REX, WAIT and
other unadmitted DB /7 store forms remain unsupported; existing DB integer,
status, control and register operations retain their behavior. Native admission
explicitly denies this operation. This bounded slice does not implement x87 exception delivery,
physical x86 acceptance, MMU or MMIO behavior and does not complete S2-31.

Normative source: Intel SDM revision 093, Vol.2A, “FST/FSTP—Store Floating
Point Value,” pp.3-386–3-387 (`DB /7`), official PDF p.504
([Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=504));
Vol.1 §§8.1.8, 8.1.9 and §8.6 for x87 state and FIP/FDP. See also the
[Stage 2 plan](../plans/STAGE_2.md#s2-31q-bare-bounded-fstp-m80fp-rax-db-38-partial-slice)
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).

<a id="local-s2-31cv-pseudo-infinity-m64-fistp"></a>

## Local S2-31cv: Special-tagged pseudo-infinity through m64 `FISTP`

Only bare `DF 38` admits a Special-tagged binary80 pseudo-infinity with exact
sign/exponent `0x7fff` or `0xffff` and zero significand (J=0). Store Qword
integer-indefinite `0x8000000000000000`, add IE, clear C1, and add no PE or
other current exception flags. Preserve historical sticky flags and C0/C2/C3.
The current FCW whitelist, `FSW & 0x80c0` gate, bare Long64 RAX addressing,
canonical48 8-byte preflight, and atomic single Qword write/no read remain
fixed. Commit status, pointers, RIP, and the FISTP pop only after the write
succeeds; preserve all ten source bytes.

Canonical signed zero continues to store zero without adding flags; canonical
J=1 infinity retains its established integer-indefinite/IE behavior. Pseudo-
NaNs, unnormals, pseudo-denormals, mismatched tags, unmasked or otherwise
disallowed controls/status, prefixes, m16/m32 behavior changes, m64 FIST, and
other forms remain unsupported. FLD m80 rejects pseudo-infinity, so no FLD
composition is claimed. JIT/native admission remains closed. Physical x86 and
native runtime acceptance remain deferred to the combined final S2-31 substage
under ISSUE-019.

The 8,192-case matrix crosses both signs, four FCWs, eight TOP values, 64
sticky combinations, and both previous C1 values. Literal assertions cover
Qword bytes/status, no new PE, C1 clearing, sticky and condition-code
preservation, raw source payload, pop/TOP wrap, canaries, and exactly one
Qword write with no read. Rejections and atomicity checks cover malformed
neighbors, wrong tags, controls, denied writes, and canonical48 faults. A
seeded CLI fixture verifies FNSTSW/FNCLEX and keeps JIT/native admission denied.
See the [plan](../plans/STAGE_2.md#s2-31cv-pseudo-infinity-m64-fistp),
[report](../reports/STAGE_2_REPORT.md#s2-31cv--special-tagged-pseudo-infinity-through-m64-fistp),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).




<a id="local-s2-31bt-exact-signed-zero-fist-m32int-rax-db-10-db-18"></a>

## Local S2-31bt: exact signed-zero `FIST`/`FISTP m32int [RAX]` (`DB 10`/`DB 18`)

The bounded `DB 10` and `DB 18` forms admit `X87Tag::Zero` only when its raw
binary80 payload is canonical zero: zero significand and sign/exponent word
`0x0000` or `0x8000`. Either sign stores the exact Dword integer zero under all
four admitted RC modes. This conversion generates no exceptions, preserves
incoming PE, clears C1, and retains the existing C0/C2/C3 behavior.

The existing FCW whitelist, `FSW & 0x80df == 0` gate, bare Long64 RAX form, and
complete canonical48 Dword range check remain in force. The source payload is
unchanged. After one atomic Dword write succeeds, FIST updates status and
pointers while preserving the physical tag and TOP; FISTP additionally empties
the old TOP tag and advances TOP modulo eight. Write/range failures leave CPU
state and memory unchanged. This support does not admit m16/m64 zero stores or
nonzero magnitudes below one.

ISA coverage crosses both signs, four RCs, both opcodes, all TOP values, prior
PE and C1 states, and checks tag/class/control rejection and fault atomicity.
Literal CLI fixtures compose each opcode with FNSTSW. JIT and native admission
remain closed; native runtime and physical behavior remain deferred under
ISSUE-019. See the [plan](../plans/STAGE_2.md#s2-31bt-exact-signed-zero-fist-m32int),
[report](../reports/STAGE_2_REPORT.md#s2-31bt--exact-signed-zero-fist-m32int),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


<a id="local-s2-31bu-exact-signed-zero-fist-m16int-rax-df-10-df-18"></a>

## Local S2-31bu: exact signed-zero `FIST`/`FISTP m16int [RAX]` (`DF 10`/`DF 18`)

The bounded `DF 10` and `DF 18` forms admit `X87Tag::Zero` only when the raw
binary80 payload is canonical zero: zero significand and sign/exponent word
`0x0000` or `0x8000`. Either sign stores the exact Word integer zero under all
four admitted RC modes. No exceptions are generated; incoming PE is preserved,
C1 is cleared, and C0/C2/C3 keep their existing behavior.

The FCW whitelist, `FSW & 0x80df == 0` gate, bare Long64 RAX form, and complete
canonical48 Word range check remain in force. The source bytes are unchanged.
After one atomic Word write succeeds, FIST commits status/pointers while keeping
the physical tag and TOP; FISTP additionally empties the old TOP tag and
advances TOP modulo eight. Write/range failures preserve full CPU state and
memory. m32 behavior is unchanged; m64 zero stores and nonzero magnitudes below
one remain unsupported.

Coverage crosses signs, RCs, opcodes, TOP, incoming PE/C1, and exercises
tag/payload/control/prefix rejection, no-read/single-Word-write behavior, and
fault atomicity. A literal CLI/FNSTSW fixture is checked; JIT/native admission
remain closed. Native runtime and physical behavior remain deferred under
ISSUE-019. See the [plan](../plans/STAGE_2.md#s2-31bu-exact-signed-zero-fist-m16int),
[report](../reports/STAGE_2_REPORT.md#s2-31bu--exact-signed-zero-fist-m16int),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


<a id="local-s2-31bv-exact-signed-zero-fistp-m64int-rax-df-38"></a>

## Local S2-31bv: exact signed-zero `FISTP m64int [RAX]` (`DF 38`)

The bounded bare `DF 38` form admits `X87Tag::Zero` only when the raw binary80
payload is canonical zero: zero significand and sign/exponent word `0x0000` or
`0x8000`. Either sign stores the exact Qword integer zero under all four
admitted RC modes. No exceptions are generated; incoming PE is preserved, C1
is cleared, and C0/C2/C3 keep their existing behavior.

The FCW whitelist, `FSW & 0x80df == 0` gate, bare Long64 RAX form, and complete
canonical48 Qword range check remain in force. The source bytes are unchanged.
After one atomic Qword write succeeds, FISTP commits status/pointers, empties
the old TOP tag, and advances TOP modulo eight. Write/range failures preserve
full CPU state and memory. No FIST m64, other forms, nonzero magnitudes below
one, or masked-invalid behavior is admitted.

Coverage crosses signs, RCs, TOP, incoming PE/C1, and exercises tag/payload/
class/control/prefix rejection, no-read/single-Qword-write behavior, signed-i64
regressions, and fault atomicity. A literal CLI/FNSTSW fixture is checked; JIT
and native admission remain closed. Native runtime and physical behavior remain
deferred under ISSUE-019. See the [plan](../plans/STAGE_2.md#s2-31bv-exact-signed-zero-fistp-m64int),
[report](../reports/STAGE_2_REPORT.md#s2-31bv--exact-signed-zero-fistp-m64int),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


<a id="local-s2-31bw-round-normal-binary80-fractions-m32-fist"></a>

## Local S2-31bw: round Normal binary80 fractions through m32 `FIST`/`FISTP` (`DB 10`/`DB 18`)

Only bare `DB 10` and `DB 18` admit a Valid-tagged canonical binary80 Normal
source with unbiased exponent -16382 through -1. These are nonzero magnitudes
below one. Nearest-ties-even stores zero through exact 0.5 and one above 0.5;
toward-zero stores zero; toward-positive stores +1 only for positive input;
toward-negative stores -1 only for negative input. Every newly admitted value
is inexact and adds sticky PE after the atomic Dword write. C1 is set iff the
rounded magnitude is one; no IE/DE/OE/UE is added.

Keep the FCW whitelist, `FSW & 0x80df == 0`, bare Long64 RAX addressing, full
canonical48 Dword range preflight, raw source bytes, and existing FIST/FISTP
state/pointer/pop ordering. Incoming PE is preserved. C0/C2/C3 retain their
current policy. The fraction path uses comparisons rather than large shifts or
divisors; e=-1 alone has a nearest-mode half threshold, and lower exponents
round to zero under nearest. Existing signed-zero and integer paths are
unchanged; m16/m64 fractional admission and noncanonical classes remain
unsupported.

Coverage crosses both signs, all four RCs, both opcodes, every TOP, incoming
PE/C1, exact half and adjacent values, the value immediately below one,
minimum Normal, and exponents -2/-63/-64/-65. It also checks rejected tags and
classes, environment gates, one-write/no-read behavior, write/range atomicity,
CLI/FNSTSW, and JIT/native denial. Native runtime and physical behavior remain
deferred under ISSUE-019. See the [plan](../plans/STAGE_2.md#s2-31bw-round-binary80-fractions-through-bounded-m32-fist),
[report](../reports/STAGE_2_REPORT.md#s2-31bw--round-binary80-fractions-through-m32-fistfstp),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


<a id="local-s2-31bx-round-normal-binary80-fractions-m16-fist"></a>

## Local S2-31bx: round Normal binary80 fractions through m16 `FIST`/`FISTP` (`DF 10`/`DF 18`)

Only bare `DF 10` and `DF 18` admit a Valid-tagged canonical binary80 Normal
source with unbiased exponent -16382 through -1. These are nonzero magnitudes
below one. The same comparison-only rule as m32 applies: nearest-ties-even
stores zero through exact 0.5 and signed one above it; toward-zero stores zero;
toward-positive stores +1 only for positive input; toward-negative stores -1
only for negative input. Every newly admitted value adds sticky PE after the
atomic Word write. C1 is set iff the rounded magnitude is one; no IE/DE/OE/UE
is added.

Keep the FCW whitelist, `FSW & 0x80df == 0`, bare Long64 RAX addressing, full
canonical48 Word-range preflight, raw source bytes, and existing FIST/FISTP
state/pointer/pop ordering. Incoming PE is preserved; C0/C2/C3 retain the
existing policy. Existing signed-zero, ±1 and signed-i16 boundary behavior is
unchanged. m32 behavior is unchanged, and m64 fractional admission remains
unsupported.

Coverage crosses both signs, all four RCs, both opcodes, every TOP, incoming
PE/C1, exact half and adjacent values, the largest fraction below one,
minimum Normal, and exponents -2/-63/-64/-65. Tests also cover tag/class/
environment rejection, Word canaries, one-write/no-read, write/range
atomicity, CLI/FNSTSW, and JIT/native denial. Native runtime and physical
behavior remain deferred under ISSUE-019. See the [plan](../plans/STAGE_2.md#s2-31bx-round-binary80-fractions-through-bounded-m16-fist),
[report](../reports/STAGE_2_REPORT.md#s2-31bx--round-binary80-fractions-through-m16-fistfstp),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


<a id="local-s2-31by-round-normal-binary80-fractions-m64-fistp"></a>

## Local S2-31by: round Normal binary80 fractions through m64 `FISTP` (`DF 38`)

Only bare `DF 38` admits a Valid-tagged canonical binary80 Normal source with
unbiased exponent -16382 through -1. It stores Qword zero or signed one using
the same comparison-only rule as m16/m32: nearest-ties-even stores zero through
exact 0.5 and signed one above; toward-zero stores zero; directed modes store
signed one only when rounding away from zero. Every admitted nonzero fraction
adds sticky PE after the atomic Qword write; C1 is set iff magnitude one is
selected. No IE/DE/OE/UE is added.

Keep FCW whitelist, `FSW & 0x80df == 0`, bare Long64 RAX addressing, full
canonical48 Qword-range preflight, source bytes, C0/C2/C3, and FISTP state/pop
ordering. Incoming PE is preserved. Do not add FIST m64 or alter m16/m32
fraction behavior. Invalid tags, subnormals, pseudo encodings, malformed
specials, prefixes, and unsupported forms remain rejected before memory access.

Coverage crosses both signs, all four RCs, every TOP, incoming PE/C1, half
neighbors, largest fraction below one, minimum Normal, and exponents
-2/-63/-64/-65, alongside existing signed-zero, ±1, and i64-boundary tests.
Fault/range atomicity, CLI/FNSTSW, and JIT/native denial are covered. Native
runtime and physical acceptance remain deferred under ISSUE-019. See the
[plan](../plans/STAGE_2.md#s2-31by-round-normal-binary80-fractions-through-bounded-m64-fistp),
[report](../reports/STAGE_2_REPORT.md#s2-31by--round-binary80-fractions-through-m64-fistp),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


<a id="local-s2-31bz-masked-canonical-infinity-fist-m32int"></a>

## Local S2-31bz: masked canonical infinity through m32 `FIST`/`FISTP` (`DB 10`/`DB 18`)

Only bare `DB 10` and `DB 18` admit Special-tagged canonical binary80
infinity: significand `0x8000000000000000` and sign/exponent word `0x7fff` or
`0xffff`. With an existing admitted FCW (all exceptions masked), either sign
and every RC mode store integer indefinite `0x80000000`. After the atomic
Dword write, set sticky IE, clear C1, preserve incoming PE and C0/C2/C3, and
add no PE/DE/OE/UE/SF; ES/B remain clear. The existing FSW gate continues to
reject incoming IE.

Keep bare Long64 RAX addressing, canonical48 four-byte preflight, raw source
bytes, one write/no read, and post-write commit. FIST retains tag/TOP; FISTP
empties the old TOP and advances it only after successful write. NaNs,
pseudo-infinity, mismatched tags, finite out-of-range values, m16/m64 and
unmasked delivery remain outside this extension.

Coverage includes both signs, all admitted FCWs, both opcodes, every TOP,
incoming PE/C1, canonical/tag/class/control/environment/prefix rejection,
finite -2^31 distinction, write/range atomicity, and a literal FISTP/FNSTSW
fixture. A direct FISTP/FNSTSW/FNCLEX composition verifies IE observation and
clearing. JIT/native denial remains explicit; native runtime and physical
behavior remain deferred under ISSUE-019. See the
[plan](../plans/STAGE_2.md#s2-31bz-masked-binary80-infinity-through-bounded-m32-fist),
[report](../reports/STAGE_2_REPORT.md#s2-31bz--masked-binary80-infinity-through-m32-fistfstp),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


<a id="local-s2-31ca-masked-canonical-infinity-fist-m16int"></a>

## Local S2-31ca: masked canonical infinity through m16 `FIST`/`FISTP` (`DF 10`/`DF 18`)

Only bare `DF 10` and `DF 18` admit Special-tagged canonical binary80
infinity: significand `0x8000000000000000` and sign/exponent word `0x7fff` or
`0xffff`. With an existing admitted FCW (all exceptions masked), either sign
and every RC mode store Word integer indefinite `0x8000`. After the atomic
Word write, set sticky IE, clear C1, preserve incoming PE and C0/C2/C3, and add
no PE/DE/OE/UE/SF; ES/B remain clear. The existing FSW gate continues to
reject incoming IE.

Keep bare Long64 RAX addressing, canonical48 two-byte preflight, raw source
bytes, one write/no read, and post-write commit. FIST retains tag/TOP; FISTP
empties the old TOP and advances it only after successful write. Canonical
NaNs, pseudo-infinity, mismatched tags, finite out-of-range values, m64, and
unmasked delivery remain outside this extension; m32 behavior is unchanged.

Coverage includes both signs, all admitted FCWs, both opcodes, every TOP,
incoming PE/C1, canonical/tag/class/control/environment/prefix rejection,
finite -32768 distinction, Word canaries, write/range atomicity and a literal
FISTP/FNSTSW fixture. A direct FISTP/FNSTSW/FNCLEX sequence verifies IE
observation and clearing. JIT/native denial remains explicit; native runtime
and physical behavior remain deferred under ISSUE-019. See the
[plan](../plans/STAGE_2.md#s2-31ca-masked-binary80-infinity-through-bounded-m16-fist),
[report](../reports/STAGE_2_REPORT.md#s2-31ca--masked-binary80-infinity-through-m16-fistfstp),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


<a id="local-s2-31cb-masked-canonical-infinity-fistp-m64int"></a>

## Local S2-31cb: masked canonical infinity through m64 `FISTP` (`DF 38`)

Only bare `DF 38` admits Special-tagged canonical binary80 infinity:
significand `0x8000000000000000` and sign/exponent word `0x7fff` or `0xffff`.
With an existing admitted FCW (all exceptions masked), either sign and every
RC mode store Qword integer indefinite `0x8000000000000000`. After the atomic
Qword write, set sticky IE, clear C1, preserve incoming PE and C0/C2/C3, and
add no PE/DE/OE/UE/SF; ES/B remain clear. The existing FSW gate continues to
reject incoming IE.

Keep bare Long64 RAX addressing, canonical48 eight-byte preflight, raw source
bytes, one write/no read, and post-write commit. FISTP empties old TOP and
increments TOP modulo eight only after successful write. Exact finite -2^63
produces the same Qword bits without IE. NaNs, pseudo-infinity, malformed
specials, mismatched tags, finite overflow, prefixed forms, and unmasked
delivery remain outside this extension; no FIST m64 form is added.

Coverage includes both signs, all admitted FCWs, every TOP, incoming PE/C1,
tag/class/control/prefix rejection, Qword canaries and write/range atomicity,
exact -2^63 distinction, and a literal FISTP/FNSTSW fixture. A direct
FISTP/FNSTSW/FNCLEX sequence verifies IE observation and clearing. JIT/native
denial remains explicit; native runtime and physical behavior remain deferred
under ISSUE-019. See the
[plan](../plans/STAGE_2.md#s2-31cb-masked-binary80-infinity-through-bounded-m64-fistp),
[report](../reports/STAGE_2_REPORT.md#s2-31cb--masked-binary80-infinity-through-m64-fistp),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


<a id="local-s2-31bs-sticky-oe-on-raw-binary80-fstp-m80fp"></a>

## Local S2-31bs: sticky OE on raw `FSTP m80fp [RAX]` (`DB 38`)

Only the bounded `DB 38` source helper admits `FSW & 0x80c4 == 0`, adding
incoming masked OE to the previously admitted IE/DE/UE/PE flags. ZE, SF, ES and
B remain rejected. FCWs, exact opcode, source tag/class pairs, addressing and
raw ten-byte write remain unchanged. The raw source is never normalized or
quieted, and the instruction generates no exceptions: OE-only signaling-NaN
stores preserve the source and do not add IE or PE. C1 is cleared as in the
existing raw path; C0/C2/C3 and incoming flags are preserved. A failed write or
range check commits no status, pointer, RIP or pop state.

Coverage includes all 32 IE/DE/OE/UE/PE combinations over accepted classes,
signs, FCWs and TOPs; m32/m64 overflow followed by DB38 without FNCLEX; OE-only
SNaN raw-byte preservation; incoming-OE fault/range atomicity; FNSTSW/FNCLEX;
and an OE-only SNaN CLI fixture. JIT and native admission remain denied.
Native runtime and physical acceptance remain deferred under ISSUE-019. See
the [plan](../plans/STAGE_2.md#s2-31bs-admit-sticky-oe-to-raw-binary80-fstp),
[report](../reports/STAGE_2_REPORT.md#s2-31bs--sticky-oe-on-raw-binary80-fstp-m80fp),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


<a id="local-s2-31ar-canonical-quietnan-fstp-m80fp-rax-db-38"></a>

## Local S2-31ar: canonical QuietNaN `FSTP m80fp [RAX]` (`DB 38`)

Exact `DB 38` additionally accepts a canonical binary80 QuietNaN only with a
physical Special tag: exponent `0x7fff`, explicit integer bit one, quiet
fraction bit one, either sign, and arbitrary remaining payload. Copy the raw
ten bytes unchanged, including sign and payload. Do not normalize, round, use
host floating point, or set new status flags. Preserve incoming IE/DE/PE and
C0/C2/C3; clear C1. S2-31ba widens the current DB38 gate to
`FSW & 0x80dc == 0`.

Retain four FCWs and the current `FSW & 0x80dc == 0` gate. Validate the full
canonical48 ten-byte range, perform exactly one atomic write/no read, and
pop/advance TOP only after success. Existing Valid+Normal, Zero+Zero, and
Special+Infinity pairs remain accepted. SNaN, pseudo-QNaN/SNaN, pseudo-infinity,
unnormal, pseudo-denormal, other classes and mismatched tags reject before
access.
Native/JIT admission stays closed. The canonical binary80 QNaN layout is
specified by Intel SDM Vol.1 Table 4-3 (revision 090); DB /7 FSTP m80fp is in
Vol.2A pp.3-386–3-387 ([official revision 093 PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=503)).
See the [S2-31ar plan](../plans/STAGE_2.md#s2-31ar-store-canonical-binary80-quietnan-through-bounded-fstp-m80fp),
[report](../reports/STAGE_2_REPORT.md#s2-31ar--canonical-quietnan-binary80-store-through-bounded-fstp-m80fp),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).

<a id="local-s2-31as-canonical-signalingnan-fstp-m80fp-rax-db-38"></a>

## Local S2-31as: canonical SignalingNaN `FSTP m80fp [RAX]` (`DB 38`)

Exact `DB 38` additionally accepts canonical binary80 SignalingNaN only with
physical TOP tagged Special: exponent `0x7fff`, explicit integer bit one,
quiet bit zero, and a nonzero remaining fraction, with either sign. Copy the
raw ten bytes unchanged. In particular, do not set the quiet bit, alter the
payload, or add IE or any other status flag. Keep the current C1-clear and
IE/DE/PE/C0/C2/C3-preserve store policy, the four FCWs, and the S2-31ba
`FSW & 0x80dc == 0` gate.

Validate the full canonical48 ten-byte range, perform one atomic write/no read,
and pop/advance TOP only after successful write. Valid+Normal, Zero+Zero,
Special+Infinity, and Special+QuietNaN remain accepted. Pseudo-SNaN, other
unsupported encodings, and mismatched tags reject before access. Native/JIT
admission stays closed. Intel SDM Vol.1 Table 4-3 defines the canonical
binary80 NaN encoding; Vol.2A pp.3-386–3-387 documents the DB /7 FSTP m80fp
operation ([official revision 093 PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=503)).
See the [S2-31as plan](../plans/STAGE_2.md#s2-31as-store-canonical-binary80-signalingnan-through-bounded-fstp-m80fp),
[report](../reports/STAGE_2_REPORT.md#s2-31as--canonical-signalingnan-binary80-store-through-bounded-fstp-m80fp),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).

<a id="local-s2-31at-canonical-subnormal-fstp-m80fp-rax-db-38"></a>

## Local S2-31at: canonical subnormal `FSTP m80fp [RAX]` (`DB 38`)

Exact `DB 38` additionally accepts canonical binary80 subnormal only with
physical TOP tagged Special: exponent zero, explicit integer bit zero, and
nonzero fraction, with either sign. Copy the raw ten bytes unchanged. Do not
normalize, flush, round, or add DE/UE/PE or any other status flags. Keep the
current C1-clear and IE/DE/PE/C0/C2/C3-preserve store policy, four FCWs, and
the S2-31ba `FSW & 0x80dc == 0` gate.

Validate the full canonical48 ten-byte range, perform one atomic write/no read,
and pop/advance TOP only after successful write. Valid+Normal, Zero+Zero,
Special+Infinity, Special+QuietNaN, and Special+SignalingNaN remain accepted.
Pseudo-denormal, other unsupported encodings, and mismatched tags reject
before access. Native/JIT admission stays closed. The binary80 encoding and
reconstructed tag class follow Intel SDM revision 093 Vol.1 Tables 4-3 and
8-3; Vol.2A pp.3-386–3-387 documents DB /7 FSTP m80fp ([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=503)).
See the [S2-31at plan](../plans/STAGE_2.md#s2-31at-store-canonical-binary80-subnormal-through-bounded-fstp-m80fp),
[report](../reports/STAGE_2_REPORT.md#s2-31at--canonical-subnormal-binary80-store-through-bounded-fstp-m80fp),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).

## Local S2-31r: bounded `FLD m80fp [RAX]` (`DB 28`)

The interpreter admits only exact bare `DB 28` in Long64 with existing x87
state. It requires FCW `0x037f`, `0x077f`, `0x0b7f`, or `0x0f7f`,
`FSW & 0x80df == 0`, and an Empty physical destination `(TOP+7)&7`. The old
logical ST(0) tag and payload are not inspected. The full canonical48 range
`[RAX,RAX+9]` and checked endpoint are validated before one
`GuestMemory::read(address, context, &mut [u8; 10])`. After that read, only raw
canonical-Normal binary80 is accepted. Its bytes are copied exactly to the new
physical TOP; no numeric conversion occurs. Every other binary80 class returns
local Unsupported after the single read, with CPU state unchanged.

On success, write the new payload and Valid tag, set TOP to the destination,
clear C1, preserve PE and every other FSW bit, preserve FCW/FOP and all other
tags/payloads, record FIP/FDP, advance RIP by two and retire once. Retaining
C0/C2/C3 is the deterministic local policy used by the existing bounded FILD
pushes. Architectural faults use the guest-fault path; backend errors remain
diagnostics. `DB 30`, all other unadmitted DB forms and every prefix/REX/WAIT
encoding remain unsupported. Existing DB forms, including DB 38 FSTP and
DB 00 FILD, retain their meanings. Native x87 execution and general exception
delivery remain unavailable.

Interpreter checks cover all TOP values and wrap; every accepted FCW; arbitrary
unrelated tags/payloads and irrelevant old ST(0); sticky PE, incoming C1 and
retained C0/C2/C3; independent raw positive/negative fractional and minimum/
maximum exponent vectors; one unaligned exact ten-byte read/no writes; invalid
classes after the one read; canonical start/end/crossing/overflow; injected
architectural #PF/backend errors; invalid environment/destination/missing
state; and fetch, truncation, STOP and zero-budget atomicity. Exhaustive DB
ModR/M and prefix checks preserve existing forms and specifically reject DB30.
Schema-v2 and CLI fixtures cover exact `DB 28`, exact `DB 28 DF E0`, and an
invalid-class no-commit case. S2-31 remains partial.

Normative source: Intel SDM revision 093, Vol.2A, “FLD—Load Floating-Point
Value,” pp.3-352–3-353, official PDF p.470
([Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=470));
Vol.1 §§8.1.8, 8.1.9 and §8.6 for x87 state and FIP/FDP. See the
[Stage 2 plan](../plans/STAGE_2.md#s2-31r-bare-bounded-fld-m80fp-rax-db-28-partial-slice)
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).

## Local S2-31w: bounded `FLD m80fp [RAX]` (`DB 28`) signed-zero extension

This updates the current S2-31r behavior while preserving that report as the
historical Normal-only result. Exact bare `DB 28` keeps the existing x87 state,
FCW/FSW/Empty-destination gates, complete canonical48 ten-byte range check,
and one exact read. After the read, canonical Normal bytes remain unchanged
and receive `X87Tag::Valid`; either canonical signed-zero encoding remains
unchanged and receives `X87Tag::Zero`. Nonzero subnormals, infinity, NaNs and
other non-Normal classes return local Unsupported after the read without any
CPU commit. In particular both signs of minimum nonzero binary80 subnormal are
still excluded.

Success retains the existing bounded push effects: set TOP to the destination,
clear C1, preserve PE and all other status bits, FCW/FOP, other tags/payloads,
record FIP/FDP, advance RIP by two and retire once. No memory write is issued.
Faults and backend failures retain the existing behavior. Native admission
remains denied.

Intel SDM revision 093 Vol.2A FLD m80fp pp.3-352–3-353 (official PDF p.470)
does not define a denormal exception for the m80fp source. Therefore the
binary32/binary64 #D behavior is not applied here. Denormal and unmasked
exception semantics beyond this narrow class decision require a dedicated
future slice.

Literal interpreter coverage checks +0/-0 over every TOP×FCW pair, unchanged
Normal values, exact single-read behavior, Zero tags, and full-state rejection
for ±minimum subnormal and other unsupported classes. The `DB 28` fixture uses
+0; the `DB 28 DF E0` composition uses -0 and checks raw sign plus Zero tag.
The Unsupported fixture now uses positive minimum subnormal. Schema exception
admission is restricted to nonzero binary80 subnormal data for exact `DB 28`,
budget one and no stop RIP.

[Stage 2 plan](../plans/STAGE_2.md#s2-31w-extend-bounded-fld-m80fp-rax-db-28-to-signed-zero),
[historical S2-31r report](../reports/STAGE_2_REPORT.md#s2-31r--bounded-fld-m80fp-partial-slice),
[fixture notes](../../tests/fixtures/x86/stage2/README.md).

## Local S2-31x: bounded `FLD m80fp [RAX]` (`DB 28`) canonical subnormal extension

This supersedes the S2-31w class decision while preserving its gates and
history. Exact bare `DB 28` requires the same x87 state, FCW
`0x037f`/`0x077f`/`0x0b7f`/`0x0f7f`, `FSW & 0x80df == 0`, Empty physical
destination, checked canonical48 ten-byte range, and exactly one raw read.
After the read, preserve canonical Normal bytes and assign Valid; preserve
signed-zero bytes and assign Zero; preserve canonical nonzero binary80
Subnormal bytes and assign Special. Subnormal means exponent zero, explicit
integer bit clear, and nonzero fraction. No normalization or generic tag
inference occurs.

The existing push, TOP wrap, C1 clear, sticky PE and unrelated status
preservation, FCW/FOP, other tags/payloads, FIP/FDP/RIP update and one
retirement remain unchanged. No DE, UE or PE status bit is added. A
pseudo-denormal (exponent zero with explicit integer bit set), infinity, NaN,
unnormal or pseudo-special remained Unsupported in the S2-31x result after
exactly one read and without CPU commit. S2-31aa supersedes only the canonical
signed-infinity class while retaining the other rejection cases. Faults,
backend denial and fetch/truncation/STOP/budget behavior remain unchanged.
Native x87 admission remains closed.

Independent literals cover positive/negative minimum, largest and interior
subnormals across TOP×FCW with unrelated tags/payloads. CLI fixtures cover
positive minimum `DB 28` and negative largest `DB 28 DF E0`. The narrow
schema/CLI Unsupported exception now uses positive pseudo-denormal under exact
`DB 28`, budget one and no stop RIP; a negative pseudo-denormal and other
special classes remain excluded. The S2-31w section above remains the
historical signed-zero result.

Normative source: Intel SDM revision 093, Vol.2A FLD m80fp pp.3-352–3-353
(official PDF p.470)
([Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=470)).
That entry specifies no `#D` for this m80fp source. No binary32/binary64
denormal behavior is inferred. General exception delivery and physical
acceptance remain outside this local slice. See the
[Stage 2 plan](../plans/STAGE_2.md#s2-31x-extend-bounded-fld-m80fp-rax-db-28-to-canonical-subnormals),
[fixture notes](../../tests/fixtures/x86/stage2/README.md), and
[S2-31x report](../reports/STAGE_2_REPORT.md#s2-31x--bounded-fld-m80fp-canonical-subnormal-extension).

## Local S2-31v: bounded `FLD m32fp [RAX]` (`D9 00`) signed-zero extension

S2-31v extends exact bare `D9 00` to accept binary32 +0 and -0 after the
existing one Dword read. All x87 state, FCW, FSW, Empty destination, canonical48
range, fetch and encoding gates remain unchanged. A source whose magnitude
bits are zero packs to ten zero bytes for +0 or ten zero bytes with final byte
`0x80` for -0; both set the destination tag to Zero. Normal exponent fields
1..254 keep the existing integer expansion and Valid tag. Nonzero subnormals,
infinity and NaNs remain Unsupported after the read without CPU commit.

The push, TOP wrap, C1 clearing, PE and condition-code retention, FIP/FDP/RIP,
retirement, fault and backend-error behavior are unchanged. Tests include both
signed zeros over every TOP×FCW pair, preserve the existing Normal vectors, and
verify positive/negative minimum-subnormal rejection after exactly one Dword
read. Exact `D9 00` and `D9 00 DF E0` schema/CLI fixtures assert Zero tags and
signed payloads. The Unsupported fixture uses positive minimum subnormal; the
schema admits that result only for a nonzero binary32 subnormal with exact
`D9 00`, budget one and no stop RIP. Native admission remains explicitly
denied. General x87 exception delivery and physical x86 acceptance remain
outside this partial slice; S2-31 and ISSUE-019 remain open.

Normative source: Intel SDM revision 093, Vol.2A, “FLD—Load Floating-Point
Value,” pp.3-352–3-353, official PDF pp.470–471
([Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=470));
Vol.1 §§8.1.8, 8.1.9 and §8.6 for x87 state and FIP/FDP. Signed-zero packing
and tag details are local project policy.

## Local S2-31t: bounded `FLD m64fp [RAX]` (`DD 00`)

The interpreter admits only exact bare `DD 00` in Long64 with an existing x87
snapshot. It requires FCW exactly `0x037f`, `0x077f`, `0x0b7f`, or `0x0f7f`,
`FSW & 0x80df == 0`, and Empty physical push destination `(TOP+7)&7`; old
physical ST(0) is irrelevant. Checked address addition and canonical48 checks
cover the full eight-byte `[RAX,RAX+7]` range before exactly one Qword read.

At the original S2-31t slice, only binary64 Normal exponent fields 1..2046
were accepted. Zero, subnormals, infinities and NaNs returned Unsupported
with no CPU commit. The conversion is integer field expansion: the 53-bit
significand is shifted left 11 places, the exponent becomes
`source_exp - 1023 + 16383`, and the sign is copied to the binary80 sign field.
This includes the complete unbiased binary64 range -1022..=+1023; the store
conversion's narrower exponent range does not apply.

Success replaces the destination payload and tag with Valid, decrements TOP,
clears C1 while preserving PE and other status bits, records FIP/FDP, advances
RIP by two and retires once. C0/C2/C3 are retained by local policy. GPRs,
RFLAGS, FCW, FOP, other tags and payloads remain unchanged. Read faults preserve
CPU state and follow the existing guest-fault/backend-diagnostic path. Native
admission remains closed. Independent raw binary80 tests and exact `DD 00`,
`DD 00 DF E0`, and invalid-class fixtures cover the bounded profile; prefix
and neighboring encodings remain rejected. This does not define general x87
exception delivery, physical x86 acceptance, MMU/MMIO behavior, or complete
S2-31.

Normative source: Intel SDM revision 093, Vol.2A, “FLD—Load Floating-Point
Value,” pp.3-352–3-353, official PDF p.470
([Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=470));
Vol.1 §§8.1.8, 8.1.9 and §8.6 for x87 state and FIP/FDP. See the
[Stage 2 plan](../plans/STAGE_2.md#s2-31s-bare-bounded-fld-m32fp-rax-d9-00-partial-slice)
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).

## Local S2-31u: bounded `FLD m64fp [RAX]` signed-zero extension

S2-31u extends the historical Normal-only S2-31t slice. Exact bare `DD 00`,
the FCW/FSW/Empty-push gates, checked full canonical48 Qword range, and exactly
one Qword read are unchanged. After the read, binary64 magnitude zero accepts
either sign: +0 packs to ten zero bytes; -0 packs to ten zero bytes with final
byte `0x80`; both receive `X87Tag::Zero`. Normal exponent fields 1..2046 retain
the existing exact integer expansion and `Valid` tag. Nonzero subnormals,
infinity and NaNs remain Unsupported after that one read, with no CPU commit.

Success pushes to physical `(old_TOP+7)&7`, clears C1 and retains PE and other
status bits, updates FIP/FDP/RIP and retires once. Other payloads/tags, GPRs,
RFLAGS, FCW and FOP remain unchanged. Sticky PE/C1 behavior, old ST(0)
independence, TOP wrap, all TOP×RC pairs, independent signed-zero/subnormal
literals, exact schema/CLI fixtures and native denial are covered. The
S2-31t report remains historical evidence for the prior Normal-only domain.
General x87 exception delivery, physical behavior and MMU/MMIO/SMP remain
unverified; S2-31 and ISSUE-019 stay open.

Normative source: Intel SDM revision 093, Vol.2A, “FLD—Load Floating-Point
Value,” pp.3-352–3-353, official PDF pp.470–471
([Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=470));
Vol.1 §§8.1.8, 8.1.9 and §8.6 for x87 state and FIP/FDP. The bounded Zero
packing and tag policy above is local project policy.

## Local S2-31j: bounded `FSTP m32fp [RAX]` (`D9 18`)

The interpreter admits only exact bare `D9 18` in Long64 with an existing x87
snapshot. Preflight examines the old physical TOP tag and payload only. The tag
must be Valid, the raw binary80 value must classify as canonical Normal, and
the unbiased exponent must be -126..126. FCW must equal `0x037f`, `0x077f`,
`0x0b7f`, or `0x0f7f`; require `FSW & 0x80df == 0`. Sticky PE and condition
codes are permitted. No host floating-point conversion is used.

For binary80 significand `s`, derive the 24-bit quotient `q=s>>40`, remainder
`r=s & ((1<<40)-1)`, and denominator `d=1<<40`. Use
`round_quotient_magnitude(sign,q,r,d,RC)`. The rounded magnitude must remain a
normal 24-bit significand. A carry to `2^24` shifts the significand right and
increments the unbiased exponent; if that produces exponent 127, reject before
memory access. Input exponents outside -126..126 are rejected, so this profile
does not produce binary32 subnormals, infinities, or NaNs. Encode sign,
exponent+127 and fraction directly as IEEE-754 binary32 bits.

After preflight, check the complete canonical48 Dword range `[RAX,RAX+3]` and
perform exactly one atomic `GuestMemory::write_integer` Dword store. The
instruction performs no data read. Only after that write succeeds, mark old
physical TOP Empty, advance TOP by one modulo eight, set PE sticky iff `r != 0`,
and set C1 iff rounding increased the magnitude (exact results clear C1 by
local policy). Preserve C0/C2/C3 by local deterministic policy, all eight raw
payloads, other tags, FCW/FOP and unrelated state. Record the instruction-start
RIP in FIP and the destination linear address in FDP; advance RIP by two and
retire once.

Literal fixtures cover exact `D9 18` and `D9 18 DF E0` composition. Interpreter
vectors cover all TOP values, 1.0, even/odd ties, positive/negative directed
rounding, minimum/maximum normal exponents, carry from exponent 126, rejected
exponents -127/127, C1/PE behavior, canaries and one-write/no-read behavior.
Missing state, invalid FCW/FSW/tag/class/exponent, complete-range canonical
failure, store errors and fetch/truncation/STOP/budget boundaries leave CPU
state uncommitted; rejected source/environment/range cases access no data.
Prefixes, WAIT and other D9 bytes remain unsupported. Native x87 admission
explicitly denies this operation. Flat-memory tests do not establish MMU/page
walk, MMIO ordering, physical x86 behavior, exception delivery, or hardware
exception priority. This local slice does not complete S2-31.

Normative source: Intel SDM revision 093, Vol.2A, “FST/FSTP—Store Floating
Point Value,” pp.3-386–3-387 (`D9 /2`/`D9 /3`); Vol.1 §§8.1.8, 8.1.9 and §8.6
for x87 state and FIP/FDP context ([official Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=504),
[official Vol.1 PDF](https://cdrdv2-public.intel.com/929351/253665-093-sdm-vol-1.pdf)).

## Local S2-31k: bounded `FST m32fp [RAX]` (`D9 10`)

The interpreter admits only exact bare `D9 10` in Long64 with an existing x87
snapshot. It uses the same source/environment gates and integer-only conversion
as S2-31j: the old physical TOP tag must be Valid; its raw binary80 payload
must classify as canonical Normal with unbiased exponent -126..126; FCW must
be `0x037f`, `0x077f`, `0x0b7f`, or `0x0f7f`; and require `FSW & 0x80df == 0`.
Sticky PE and condition codes are permitted. Finite normal binary32 is the only
accepted result; input exponents outside the range and a rounding carry past
the local exponent range reject before data access. No host floating-point
conversion is used.

For significand `s`, derive `q=s>>40`, `r=s & ((1<<40)-1)`, and
`d=1<<40`, then call the shared `round_quotient_magnitude(sign,q,r,d,RC)`.
Rounding, signed directed behavior and packing are identical to S2-31j.

After preflight, check the full canonical48 Dword destination `[RAX,RAX+3]`
and perform exactly one atomic `GuestMemory::write_integer` Dword store with no
data read. Only after success, set PE sticky iff `r != 0`, set C1 iff rounding
increased magnitude (exact results clear C1 by local policy), record the
instruction-start RIP in FIP and destination in FDP, advance RIP by two and
retire once. Preserve TOP, all eight tags, all raw payloads, FCW/FOP and
unrelated state. C0/C2/C3 retention and exact C1 behavior are deterministic
local policy; no hardware equivalence claim is made. Architectural store faults
are surfaced as guest faults; backend diagnostics remain errors.

Literal fixtures cover exact `D9 10` and exact `D9 10 DF E0` composition.
Interpreter checks cover all TOP values, every tag/payload sentinel, RC/tie/
sign behavior, exponent endpoints and carry rejection, canaries, one-write/no-
read behavior, canonical range, backend store failure, architectural `#PF`,
invalid source/environment, missing x87 state and fetch/truncation/STOP/budget
boundaries. Prefixes, WAIT and all neighboring D9 encodings remain unsupported.
Native x87 admission explicitly denies this operation. Flat-memory tests do
not establish MMU/page walks, MMIO, physical x86 behavior, exception delivery,
or hardware exception priority. This slice does not complete S2-31.

Normative source: Intel SDM revision 093, Vol.2A, “FST/FSTP—Store Floating
Point Value,” pp.3-386–3-387 (`D9 /2`/`D9 /3`); Vol.1 §§8.1.8, 8.1.9 and §8.6
([official Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=504),
[official Vol.1 PDF](https://cdrdv2-public.intel.com/929351/253665-093-sdm-vol-1.pdf)).

## Local S2-31l: bounded `FSTP m64fp [RAX]` (`DD 18`)

The interpreter admits only exact bare `DD 18` in Long64 with an existing x87
snapshot. The old physical TOP tag must be Valid and its raw binary80 payload
canonical Normal. Accept only FCW `0x037f`, `0x077f`, `0x0b7f`, or `0x0f7f`, and
require `FSW & 0x80df == 0`; sticky PE and C0/C2/C3 are permitted. For
significand `s`, calculate `q=s>>11`, `r=s&0x7ff`, and `d=0x800`, then use the
shared integer-only `round_quotient_magnitude(sign,q,r,d,RC)`. On carry to
`2^53`, shift the rounded quotient right and increment the unbiased exponent.
This local result profile accepts exponents -1022..1022; exponent 1023 is
intentionally excluded although finite binary64 values exist there. Pack the
sign bit, biased exponent and 52-bit fraction directly. No host floating-point
cast is used.

Before data access, preflight conversion and the complete canonical48 Qword
range `[RAX,RAX+7]`. Issue exactly one atomic Qword write and no read. Only
after the write succeeds, empty old physical TOP, advance TOP modulo eight,
set PE sticky iff `r != 0`, and set C1 iff rounding increased magnitude
(exact results clear C1 by local policy). Preserve all eight raw payloads,
every tag except old TOP, FCW/FOP, C0/C2/C3 and other CPU state. Record FIP as
instruction-start RIP and FDP as RAX, advance RIP by two and retire once.
Architectural write faults map through the guest-fault report path; backend
errors remain diagnostic errors.

Independent integer literals cover both signs and four RC modes, exact,
below-half, half-even, half-odd, above-half, discarded-bit boundaries,
exponent endpoints and carry rejection. Interpreter checks cover all TOPs and
wrap, payload/tag sentinels, PE/C1, one-write/no-read, canaries, noncanonical
and crossing ranges, backend errors, injected `#PF`, invalid source and
environment before memory, fetch/truncation/STOP/budget. Two local schema-v2
fixtures cover exact `DD 18` and `DD 18 DF E0` composition. This slice adds no other FSTP m64fp ModR/M or addressing form; prefixes, REX
and WAIT are rejected for this operation. Existing DD register/status forms
retain their current behavior. Native
admission explicitly denies this operation. This partial slice does not
complete S2-31 or claim physical x86 acceptance.

Normative source: Intel SDM revision 093, Vol.2A, “FST/FSTP—Store Floating
Point Value,” pp.3-386–3-387 (`DD /2`/`DD /3`)
([official PDF, Vol.2A p.3-386](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=504));
Vol.1 §§8.1.8, 8.1.9 and §8.6 for x87 state and FIP/FDP.

## Local S2-31m: bounded `FST m64fp [RAX]` (`DD 10`)

The interpreter admits only exact bare `DD 10` in Long64. It reuses the
unchanged `bounded_x87_m64fp_result` conversion and therefore has the same
preflight gates as S2-31l: existing x87 state, Valid old physical TOP tag,
canonical-Normal raw binary80 source, FCW exactly `0x037f`, `0x077f`,
`0x0b7f`, or `0x0f7f`, and `FSW & 0x80df == 0`. For significand `s`, it
rounds `q=s>>11`, `r=s&0x7ff`, `d=0x800` with integer quotient/remainder
rounding according to RC and packs binary64 bits directly. Original and
rounded unbiased exponents must both remain in -1022..1022. Exponent 1023 is
excluded by this bounded local profile although some finite binary64 values
exist there. No host floating-point cast is used.

Before data access, validate the complete canonical48 Qword range
`[RAX,RAX+7]`; make exactly one atomic Qword write and no data read. Only after
success, set sticky PE iff `r != 0`, and set C1 iff rounding increased
magnitude (exact results clear C1 by local policy). Record instruction-start
RIP in FIP and RAX in FDP, advance RIP by two and retire once. Preserve TOP,
all physical tags, all eight raw payloads including the unrounded old ST(0),
FCW/FOP, C0/C2/C3 and unrelated state. This preservation of condition-code
bits and exact C1 handling are deterministic local policies, not hardware
guarantees. Architectural store faults use the guest-fault path; backend
diagnostics remain errors.

Independent literal binary64 values cover both signs, all RCs, exact,
below-half, half-even, half-odd and above-half inputs, discarded-bit and
exponent boundaries, and carry rejection. Interpreter checks cover no-pop
preservation for all TOP values including TOP 7, all tag/payload sentinels,
the unrounded ST(0) after inexact conversion, Qword bytes/canaries, one-write
and no-read behavior, distinct noncanonical-start and canonical-crossing
ranges, backend write error, injected architectural `#PF`, invalid
source/environment before access, and fetch/truncation/STOP/budget atomicity.
Literal fixtures admit exact bare `DD 10` and exact `DD 10 DF E0` composition.
Prefixes, REX, WAIT and neighboring DD forms remain unsupported; native x87
admission explicitly denies this operation. Flat memory does not establish
MMU/page walks, MMIO, SMP, physical x86 behavior, exception delivery, or
hardware exception priority. This local slice does not complete S2-31.

Normative source: Intel SDM revision 093, Vol.2A, “FST/FSTP—Store Floating
Point Value,” pp.3-386–3-387 (`DD /2` FST and `DD /3` FSTP)
([official PDF, Vol.2A p.3-386](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=504),
[official PDF, Vol.2A p.3-387](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=505));
Vol.1 §§8.1.8, 8.1.9 and §8.6 for x87 state and FIP/FDP.

## Local S2-31n: bounded `FILD m32int [RAX]` (`DB 00`)

The Long64 interpreter accepts only exact bare `DB 00` with an existing x87
snapshot. It requires FCW exactly `0x037f`, `0x077f`, `0x0b7f`, or `0x0f7f`,
`FSW & 0x80df == 0`, and an Empty tag at physical destination
`(old_TOP+7)&7`. It deliberately does not inspect the old physical TOP tag,
the destination's old payload, or the other seven tags/payloads. Validate the
complete canonical48 Dword range `[RAX,RAX+3]` before exactly one typed Dword
read; no data writes occur.

After a successful read, sign-extend the Dword as i32 and convert it exactly
to binary80 using integer arithmetic. Zero becomes ten zero bytes with a Zero
tag. For nonzero values, widen to i64 before taking the absolute value, then
use `e=floor(log2(magnitude))`, significand `magnitude << (63-e)`, biased
exponent `16383+e`, and the source sign bit. Commit the payload and Valid/Zero
tag to the physical destination, set TOP to that destination, clear C1, set
FIP to starting RIP and FDP to RAX, advance RIP by two, and retire once.
Preserve FCW/FOP, GPR/RFLAGS and all seven unrelated tags/payloads. C0/C2/C3
retention is an explicit local policy because they are undefined for this
operation; this is not a hardware guarantee. Existing Empty old ST(0) is
allowed. A nonempty destination is Unsupported before data access.

Architectural memory read faults map through the guest-fault path and backend
failures remain diagnostic errors. Invalid environment/destination,
noncanonical start or crossing range, read failures, fetch/truncation, STOP,
and zero budget leave CPU state and RAM unchanged. The implementation makes no
hardware combined-fault priority claim. Decoder coverage exhausts all DB
ModR/M values and prefixes; within the FILD family only `DB 00` is accepted,
while existing DB forms remain supported. Fixtures cover exact
`DB 00` and exact `DB 00 DF E0` composition. Native x87 admission explicitly
denies FILD. The slice does not model general x87 exceptions, `#NM`, MMU/MMIO,
SMP, or physical x86 acceptance, and does not complete S2-31.

Normative source: Intel SDM revision 093, Vol.2A, “FILD—Load Integer,”
pp.3-342–3-343
([official PDF, Vol.2A p.3-342](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=460),
[official PDF, Vol.2A p.3-343](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=461));
Vol.1 §§8.1.8, 8.1.9 and §8.6 for x87 stack state and FIP/FDP.

## Local S2-31o: bounded `FILD m16int [RAX]` (`DF 00`)

The Long64 interpreter admits exactly bare `DF 00` with an existing x87
snapshot. It uses the S2-31n gates: FCW exactly `0x037f`, `0x077f`,
`0x0b7f`, or `0x0f7f`; `FSW & 0x80df == 0`; and Empty tag at physical
destination `(old_TOP+7)&7`. It does not inspect old physical TOP, the stale
destination payload, or any other tags/payloads. It validates the complete
canonical48 Word range `[RAX,RAX+1]`, then issues exactly one typed Word read
and no writes.

Sign-extend the read `i16` to i64, then use the shared exact integer-only
`pack_i64_binary80` conversion. Zero becomes ten zero bytes with a Zero tag;
nonzero values become canonical binary80 with a Valid tag. After a successful
read, write only the destination payload/tag, set TOP to that destination,
clear C1, set FIP to instruction-start RIP and FDP to RAX, advance RIP by two,
and retire once. Preserve PE, FCW/FOP, GPR/RFLAGS and every unrelated tag and
payload. C0/C2/C3 retention is a deterministic local policy because they are
undefined here. Existing Empty or occupied old ST(0) is permitted. Read
architectural faults use the guest-fault path; backend failures remain
diagnostics. Invalid preflight rejects before memory access.

Literal vectors cover zero, ±1/2/3, 32767/-32768, every TOP and RC, sign
extension of `0x8000` and `0xffff`, wrap, Empty stale payload, unrelated
arbitrary tags/payloads, sticky PE and C1 clearing, occupied/Empty old ST(0),
unaligned Word access with canaries and one exact two-byte read, canonical
start/crossing, architectural/backend read failures, and fetch/truncation,
STOP and zero-budget atomicity. Exhaustive DF ModR/M and prefix coverage admits
only new `DF 00` while retaining `DF 10`, `DF 18`, `DF 38`, `DF E0`, `DF E9`,
`DF F1`, and all existing DB forms. Exact fixtures are `DF 00` and
`DF 00 DF E0`. Native x87 admission explicitly denies this form. The bounded
profile does not model general x87 exceptions, `#NM`, MMU/MMIO, SMP, or
physical x86 acceptance, and does not complete S2-31.

Normative source: Intel SDM revision 093, Vol.2A, “FILD—Load Integer,”
pp.3-342–3-343
([official PDF, Vol.2A p.3-342](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=460),
[official PDF, Vol.2A p.3-343](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=461));
Vol.1 §§8.1.8, 8.1.9 and §8.6 for x87 stack state and FIP/FDP.

## Local S2-31p: bounded `FILD m64int [RAX]` (`DF 28`)

The Long64 interpreter admits exactly bare `DF 28` with an existing x87
snapshot. It uses the bounded FILD gates: FCW exactly `0x037f`, `0x077f`,
`0x0b7f`, or `0x0f7f`; `FSW & 0x80df == 0`; and Empty tag at physical
destination `(old_TOP+7)&7`. It does not inspect old physical TOP, the stale
destination payload, or any other tags/payloads. It validates the complete
canonical48 Qword range `[RAX,RAX+7]` before exactly one typed Qword read and
no writes.

Interpret the Qword as signed i64 and use integer-only exact binary80 packing.
Zero becomes ten zero bytes with a Zero tag; nonzero values become canonical
binary80 with a Valid tag. The shared packer uses `unsigned_abs`, so `i64::MIN`
does not require signed negation; no floating-point conversion or RC-dependent
rounding occurs. After a successful read, write only the destination
payload/tag, set TOP to that destination, clear C1, set FIP to instruction-start
RIP and FDP to RAX, advance RIP by two, and retire once. Preserve PE, FCW/FOP,
GPR/RFLAGS and every unrelated tag/payload. C0/C2/C3 retention is the local
FILD policy. Old ST(0) may be Empty or occupied. Architectural read faults use
the guest-fault path; backend failures remain diagnostics. Invalid preflight
rejects before memory access.

Independent literal vectors cover zero, ±1/2/3, `i64::MIN`, `i64::MAX`,
±(`2^53−1`), `2^53`, ±(`2^53+1`) and ±(`2^62−1`), ±`2^62`,
±(`2^62+1`), across every TOP and RC. Tests cover TOP wrap, stale Empty payload, unrelated tags/payloads,
old ST(0) Empty/occupied, PE preservation and C1 clearing. An unaligned Qword
with canaries proves one exact eight-byte read and no ninth-byte read. Tests
also cover noncanonical start, canonical crossing, range arithmetic overflow,
architectural #PF/backend failures and fetch/truncation/STOP/zero-budget
atomicity. Exhaustive DF ModR/M and legacy/REX prefixes admit only the new
`DF 28`, preserving DF 00/10/18/38/E0/E9/F1 and existing DB forms. Exact
fixtures are `DF 28` and `DF 28 DF E0`; native x87 admission explicitly denies
the operation. The bounded profile does not model general x87 exceptions,
`#NM`, MMU/MMIO, SMP or physical x86 acceptance, and does not complete S2-31.

Normative source: Intel SDM revision 093, Vol.2A, “FILD—Load Integer,”
pp.3-342–3-343
([official PDF, Vol.2A p.3-342](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=460),
[official PDF, Vol.2A p.3-343](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=461));
Vol.1 §§8.1.8, 8.1.9 and §8.6 for x87 stack state and FIP/FDP.

## Local S2-31b: bounded `FISTP m32int [RAX]` (`DB 18`)

The interpreter admits only bare `DB 18` in Long64: a Dword store through
RAX, with address size 64, no index/displacement/segment override, and a full
canonical48 check over all four bytes. Preflight requires an existing x87
snapshot, one of FCW `0x037f`, `0x077f`, `0x0b7f`, or `0x0f7f`, and
`FSW & 0x80df == 0`; sticky PE is allowed. Only the old physical TOP tag and
payload are inspected. The tag must be Valid, the raw binary80 value canonical
Normal, and its unbiased exponent 0..31. Other physical tags and register
payloads remain unexamined.

For significand `s`, exponent `e`, and `k=63-e`, compute `d=1<<k`,
`q=s>>k`, and `r=s&(d-1)`, then use integer-only
`round_quotient_magnitude(sign,q,r,d,RC)`. Accept a rounded magnitude at most
`0x7fffffff` for positive inputs and `0x80000000` for negative inputs. Encode
the low 32 bits with unsigned two's-complement arithmetic. Any unsupported
source, out-of-domain value or signed-i32 overflow returns Unsupported before
the data access.

After source preflight, validate the complete canonical48 range using
`MemoryOperand::access_address_48`. Issue exactly one `GuestMemory::write_integer`
Dword store, with no data read. Store errors propagate unchanged; x87 and CPU
state remain unchanged and the backend's atomic-write contract leaves RAM
unchanged. Only after success, mark the old physical TOP tag Empty, increment
TOP modulo eight, set PE sticky iff `r != 0`, set C1 iff rounded magnitude
exceeds `q`, record FIP as the starting RIP and FDP as the destination linear
address, then advance RIP by two and retire once. Preserve all eight raw
payloads, FCW, FOP, C0/C2/C3, other status bits, GPRs, RFLAGS and unrelated
state. Flat fixture mapping/permission errors are not translated to #PF.

The schema admits only exact `DB 18` or `DB 18 DF E0` programs; native x87
admission explicitly denies this operation. This local slice does not model
unmasked exceptions, deferred delivery, stack faults, CR0/#NM, MMIO, SMP,
translation/page walks, or general x87 semantics. It does not complete S2-31.

Normative source: Intel SDM revision 093, Vol.2A, “FIST/FISTP—Store Integer,”
pp.3-347–3-349 ([official Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

## Local S2-31c: bounded `FIST m32int [RAX]` (`DB 10`)

The interpreter admits only bare `DB 10` in Long64: a Dword store through
RAX, with address size 64, no index/displacement/segment override, and a full
canonical48 check over all four bytes. Its preflight and conversion domain are
the same as S2-31b: an existing x87 snapshot; FCW exactly `0x037f`, `0x077f`,
`0x0b7f`, or `0x0f7f`; `FSW & 0x80df == 0`; Valid old physical TOP tag;
canonical-Normal raw binary80 source with unbiased exponent 0..31; and an
integer-rounded result within signed i32 range. Unsupported environment,
source, or overflow returns Unsupported before data access.

The conversion uses integer quotient/remainder rounding and unsigned
two's-complement encoding, as specified for S2-31b. After validating the full
canonical48 range, issue exactly one `GuestMemory::write_integer` Dword store
with no data read. Store errors propagate and leave CPU/x87 state unchanged.
After success, set PE sticky iff the conversion had a remainder, set C1 iff
rounding increased the magnitude, record FIP as starting RIP and FDP as the
destination, advance RIP by two and retire once. Preserve TOP, all physical
tags, all eight raw payloads, FCW, FOP, C0/C2/C3, other status bits, GPRs,
RFLAGS and unrelated state.

Literal schema fixtures admit exact `DB 10` and `DB 10 DF E0`; prefix, nearby
encoding, invalid-state, overflow and execution-boundary cases remain rejected.
Native x87 admission explicitly denies this operation. The local slice does
not model unmasked exceptions, deferred delivery, stack faults, CR0/#NM, MMIO,
SMP, translation/page walks, or general x87 semantics and does not complete
S2-31.

Normative source: Intel SDM revision 093, Vol.2A, “FIST/FISTP—Store Integer,”
pp.3-347–3-349 ([official Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

## Local S2-31d: bounded `FISTP m16int [RAX]` (`DF 18`)

The interpreter admits only bare `DF 18` in Long64: a Word store through RAX,
with address size 64, no index/displacement/segment override, and a full
canonical48 check over both bytes. Preflight requires an existing x87 snapshot,
FCW exactly `0x037f`, `0x077f`, `0x0b7f`, or `0x0f7f`, and
`FSW & 0x80df == 0`; sticky PE is allowed. Only the old physical TOP tag and
payload are inspected. The tag must be Valid, the raw binary80 value canonical
Normal, and its unbiased exponent 0..15. Other physical tags and payloads
remain unexamined.

For significand `s`, exponent `e`, and `k=63-e`, compute `d=1<<k`,
`q=s>>k`, and `r=s&(d-1)`, then use integer-only
`round_quotient_magnitude(sign,q,r,d,RC)`. Accept a rounded magnitude at most
`0x7fff` for positive inputs and `0x8000` for negative inputs. Encode the low
16 bits with unsigned two's-complement arithmetic. Unsupported source,
out-of-domain values and signed-i16 overflow return Unsupported before data
access.

After source preflight, validate the complete canonical48 range using
`MemoryOperand::access_address_48`. Issue exactly one `GuestMemory::write_integer`
Word store, with no data read. Store errors propagate unchanged, and CPU/x87
state remains unchanged. Only after success, mark old physical TOP Empty,
increment TOP modulo eight, set PE sticky iff `r != 0`, set C1 iff rounding
increased magnitude, record FIP/FDP, advance RIP by two and retire once.
Preserve every raw payload and unrelated state. Literal fixtures cover all RC
modes with both signs, tie cases, exact and rounded signed-word boundaries,
the exact `DF 18 DF E0` status composition, overflow and invalid tag/status/FCW
cases. Interpreter checks cover no-read/single-Word-write behavior, complete
state preservation, full-range canonical rejection, permission failure and
injected architectural fault atomicity. Native x87 admission explicitly
denies this operation. Flat fixture RAM does not establish MMU, MMIO, SMP,
physical execution, exception delivery, or general x87 semantics.

Normative source: Intel SDM revision 093, Vol.2A, “FIST/FISTP—Store Integer,”
pp.3-347–3-349 ([official Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

S2-31ck later extends this initial DF 18 slice: finite rounded overflow now
stores integer-indefinite and adds IE after a successful store. The former
overflow and exponent-16 fixtures now execute successfully.

## Local S2-31e: bounded `FIST m16int [RAX]` (`DF 10`)

The interpreter admits bare `DF 10` in Long64 as a Word store through RAX,
with 64-bit address size, no index/displacement/segment override, and a full
canonical48 check over both bytes. Preflight mirrors S2-31d: an existing x87
snapshot; FCW exactly `0x037f`, `0x077f`, `0x0b7f`, or `0x0f7f`; and
`FSW & 0x80df == 0`, with sticky PE allowed. Only the old physical TOP tag and
payload are inspected. The tag must be Valid, the raw binary80 value canonical
Normal, and its unbiased exponent 0..15. Other physical tags and payloads are
not examined. Integer quotient/remainder rounding follows FCW.RC, and the
rounded result must fit signed i16.

After preflight, validate the complete canonical48 two-byte destination and
issue exactly one atomic `GuestMemory::write_integer` Word store, with no data
read. Store errors propagate unchanged and preserve CPU/x87 state and RAM.
Only after success, set PE sticky iff the conversion had a remainder, set C1
iff rounding increased magnitude, record FIP/FDP, advance RIP by two and
retire once. Preserve TOP, every physical tag, all eight raw payloads, FCW,
FOP, C0/C2/C3, other status bits, GPRs, RFLAGS and unrelated state.

Literal fixtures admit only `DF 10` and exact `DF 10 DF E0`; they cover
no-pop state, FNSTSW composition, signed-word rounding/boundaries, overflow
and invalid tag/status/control/exponent rejection. Decoder and schema reject
prefixes and nearby encodings. Native x87 admission explicitly denies the
operation. This local slice does not model unmasked exceptions, deferred
delivery, stack faults, CR0/#NM, MMIO, SMP, translation/page walks, or general
x87 semantics and does not complete S2-31.

Normative source: Intel SDM revision 093, Vol.2A, “FIST/FISTP—Store Integer,”
pp.3-347–3-349 ([official Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

S2-31ck later extends this initial DF 10 slice: finite rounded overflow now
stores integer-indefinite and adds IE after a successful store. The former
overflow and exponent-16 fixtures now execute successfully.

## Local S2-31f: bounded `FISTP m64int [RAX]` (`DF 38`)

The interpreter admits only bare `DF 38` in Long64 as an eight-byte Qword
store through RAX, with no index, displacement, or segment override. Preflight
requires an existing x87 snapshot, FCW exactly `0x037f`, `0x077f`, `0x0b7f`, or
`0x0f7f`, and `FSW & 0x80df == 0`; sticky PE is allowed. Only old physical TOP
is read. Its tag must be Valid, its binary80 encoding canonical Normal, and
its unbiased exponent 0..63. Other physical tags and payloads are not
examined.

For significand `s`, exponent `e`, and `shift = 63 - e`, calculate
`denominator = 1 << shift`, `quotient = s >> shift`, and
`remainder = s & (denominator - 1)`. This includes `e=63`, where shift is
zero, denominator is one, and remainder is zero. Apply the existing
integer-only `round_quotient_magnitude(sign, quotient, remainder, denominator,
RC)` helper. Before narrowing, accept positive magnitudes through
`0x7fff_ffff_ffff_ffff` and negative magnitudes through
`0x8000_0000_0000_0000`; encode accepted output with unsigned 64-bit
two's-complement arithmetic. Overflow and unsupported source/configuration
return Unsupported before data access. This limited policy does not produce
the architectural indefinite integer result for masked invalid conversion.

After conversion, validate the complete canonical48 eight-byte range with
`MemoryOperand::access_address_48`, then issue exactly one atomic
`GuestMemory::write_integer` Qword store without reading destination data.
Store errors propagate unchanged and leave CPU/x87 state and RAM unchanged.
Only after success, mark old physical TOP Empty, advance TOP modulo eight,
keep PE sticky and set it when the conversion had a remainder, set C1 iff
rounding increased magnitude, record FIP as old RIP and FDP as RAX, advance
RIP by two and retire once. Preserve all other tags, raw payloads, FCW/FOP,
condition codes, GPRs and RFLAGS.

Literal fixtures cover exact `DF 38`, exact `DF 38 DF E0` FNSTSW observation,
Qword canaries, signed-i64 maximum/minimum and positive overflow. Interpreter
vectors independently cover all RC/sign ties, negative 2^63 in every RC mode,
`2^63 - 0.5` in all four RC modes, positive 2^63, negative `2^63+1`, exponent
64, TOP wrap, canonical-range rejection and atomic permission/#PF store
failures. Prefixes and nearby forms are rejected; native x87 admission
explicitly denies `FISTP m64int`. Exception delivery, the indefinite-value
path, MMU translation, MMIO, SMP and physical-x86 behavior remain outside this
slice, which does not complete S2-31.

Normative source: Intel SDM revision 093, Vol.2A, “FIST/FISTP—Store Integer,”
pp.3-347–3-349 ([official Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

## Local S2-31g: bounded `FNSTCW m2byte [RAX]` (`D9 38`)

The interpreter admits only exact bare `D9 38` in Long64 with an existing x87
snapshot. It stores the raw 16-bit FCW little-endian to the fixed Word
destination `[RAX]`. Any FCW and any FSW value admitted by the existing state
schema are accepted, including pending/status bits and an empty x87 stack; the
instruction does not inspect ST(0), tags or TOP. The no-wait `FNSTCW` form does
not check for pending unmasked floating-point exceptions.

Validate the entire canonical48 range `[RAX, RAX+1]` before issuing exactly one
atomic `GuestMemory::write_integer` Word write. No data read occurs. On success,
only RIP advances by two and the instruction retires once. Preserve the full
x87 snapshot, including FCW, FSW, tags/TOP, payloads, FIP/FDP/FOP, as well as
all GPRs and RFLAGS. Address errors and native backend write errors propagate
with their original memory-error semantics; no CPU state commits on any write
failure. Fetch, truncation, stop and budget boundaries also do not commit.

The schema admits only exact `D9 38` and `D9 38 DF E0` composition. Literal
fixtures cover the four standard FCWs, an arbitrary FCW with pending status
bits, empty tags, raw payload/pointer sentinels, unaligned Word canaries and
FNSTSW observation. Interpreter checks cover the complete canonical range,
including a second-byte failure, and preserved CPU/FPU state. Prefixes and
nearby D9 forms are unsupported; native x87 admission explicitly denies this
operation. The no-wait `FNSTCW` form does not check pending x87 exceptions; the
`#MF` behavior associated with waiting `FSTCW` is not applied to this opcode.
Undefined C0-C3 status bits are preserved deterministically by the local
snapshot contract without claiming native equality. The local profile does not
model general x87 exception delivery, MMU translation, MMIO or physical x86
behavior and does not complete S2-31.

Normative source: Intel SDM revision 093, Vol.2A, “FSTCW/FNSTCW—Store x87 FPU
Control Word,” pp.3-388–3-389 (`D9 /7`); Vol.1 §§8.1.8, 8.1.9 and §8.6 for
x87 state and instruction-pointer/data-pointer context ([official Vol.2A
PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf),
[official Vol.1 PDF](https://cdrdv2-public.intel.com/929351/253665-093-sdm-vol-1.pdf)).

## Local S2-31h: bounded `FNSTSW m2byte [RAX]` (`DD 38`)

The interpreter admits only exact bare `DD 38` in Long64 with an existing x87
snapshot. It stores the raw pre-operation FSW little-endian through one atomic
`GuestMemory::write_integer` Word write to `[RAX]`. It performs no data read,
stack/tag/TOP classification, FCW/FSW gate, or arithmetic exception check. The
complete two-byte destination must be canonical48 before the write. Only after
a successful store does RIP advance by two and retire once. GPRs, RFLAGS, and
the entire x87 snapshot remain unchanged, including C0-C3 by local deterministic
policy; this does not claim native equivalence for those architecturally
undefined bits.

Missing x87 state, address validation, store error, fetch/truncation, stop, and
budget failures leave CPU state uncommitted. Memory errors propagate. The exact
schema admits `DD 38` and `DD 38 DF E0` (FNSTSW AX); prefixes, including waiting
`9B DD 38`, and neighboring DD forms are rejected. There is no `#MF` delivery
for this no-wait form and no `#NM`/CR0 behavior claim. Native admission
explicitly denies the operation.

Literal fixtures cover raw FSW `0000`, mixed status, and `ffff`; arbitrary FCW
and FSW with Empty tags and x87/GPR sentinels; unaligned Word canaries; exact
FNSTSW AX composition; and canonical-range rejection. Interpreter tests cover
one Word write with no data read, complete snapshot preservation, missing
state, write permission/backend failure, both canonical boundaries, fetch
failure, stop, and budget exhaustion. These local FlatMemory checks do not
establish MMU translation, page-walk behavior, MMIO ordering, `#MF` delivery,
`#NM`/CR0 behavior, or physical x86 equivalence.

Normative source: Intel SDM revision 093, Vol.2A, “FSTSW/FNSTSW—Store x87 FPU
Status Word,” pp.3-392–3-393 (`DD /7`); Vol.1 §§8.1.8, 8.1.9 and 8.3.12 for
x87 state and condition-code context ([official Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf),
[official Vol.1 PDF](https://cdrdv2-public.intel.com/929351/253665-093-sdm-vol-1.pdf)).

## Local S2-31i: bounded `FLDCW m2byte [RAX]` (`D9 28`)

The interpreter admits only exact bare `D9 28` in Long64 with an existing x87
snapshot. Before touching memory, require old FCW exactly `0x037f`, `0x077f`,
`0x0b7f`, or `0x0f7f`, and `FSW & 0x80df == 0`. Sticky PE and C0-C3 condition
bits are allowed; tags, TOP, and payloads are not inspected.

Validate the complete canonical48 range `[RAX, RAX+1]`, then issue exactly one
`GuestMemory::read_integer` Word read. Accept the loaded little-endian FCW only
if it is one of the same four values. An invalid old environment rejects
before data access. An invalid loaded FCW returns Unsupported after its single
read and before state mutation. A successful load changes only
`x87.control_word`, advances RIP by two, and retires once. Preserve the full
FSW (including architecturally undefined C0-C3 by local deterministic policy),
TOP, tags, all eight payloads, FIP/FDP/FOP, GPRs, and RFLAGS. Read and address
errors propagate and preserve CPU state.

Tests cover all 16 old/new FCW pairs, each loaded RC mode through FRNDINT,
`D9 28 D9 FC DF E0` composition, invalid old FCW/FSW before access, invalid
loaded FCW after one read, missing state, second-byte read failure,
canonical48 crossing, fetch/truncation/STOP/budget boundaries, and exhaustive
D9 ModR/M plus prefix/WAIT rejection. Native x87 admission explicitly denies
FLDCW. This local slice preserves pending-status bits when they are allowed by
the old-state gate but does not deliver pending or unmasked exceptions; WAIT,
`#MF`, `#NM`/CR0, MMU translation, MMIO, SMP and physical x86 behavior remain
outside the slice. It does not complete S2-31.

Normative source: Intel SDM revision 093, Vol.2A, “FLDCW—Load x87 FPU Control
Word,” pp.3-356–3-357 (`D9 /5`); Vol.1 §§8.1.8, 8.1.9 and §8.6 for x87 state
and instruction-pointer/data-pointer context ([official Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf),
[official Vol.1 PDF](https://cdrdv2-public.intel.com/929351/253665-093-sdm-vol-1.pdf)).

## Local S2-31a: bounded `FRNDINT` (`D9 FC`)

The interpreter admits only bare `D9 FC` in an existing x87-state snapshot.
Preflight reads the old physical TOP tag and payload only: the tag must be
Valid, the raw value canonical Normal, and its unbiased exponent must be 0..62.
Other physical tags and payloads are not examined. FCW must be exactly one of
`0x037f`, `0x077f`, `0x0b7f`, or `0x0f7f`, selecting PC64, all exception masks,
and nearest-even, toward-negative, toward-positive, or toward-zero RC. Require
`FSW & 0x80df == 0`: all exception/status failure bits except sticky PE must be
clear; TOP and condition codes are allowed.

For significand `s`, exponent `e`, and `k=63-e`, preflight computes
`d=1<<k`, `q=s>>k`, and `r=s&(d-1)`, then calls the integer-only
`round_quotient_magnitude(sign,q,r,d,RC)`. It packs the nonzero rounded integer
with `pack_normal_rounded_significand(sign,floor(log2(m)),m<<(63-n),64)`.
This includes values that round to either signed `2^63`; no host integer cast is
used. The source domain ensures `m` is nonzero.

Commit replaces only the old physical TOP payload. PE is set sticky exactly
when `r != 0`; C1 is set exactly when rounded magnitude exceeds `q`, otherwise
cleared. C0/C2/C3 retain their concrete values as an explicit local policy.
Tags, TOP, FCW, FDP, FOP, GPR, RFLAGS and all other state are preserved; FIP is
set to the starting RIP, RIP advances by two, and the instruction retires once.
Every unsupported source/environment and fetch, truncation, STOP or budget
boundary is atomic. The fixture schema accepts only `D9 FC` or the exact
`D9 FC DF E0` FNSTSW composition; native execution explicitly rejects D9 FC.

This local slice does not model zero/subnormal/special values, other exponent
ranges, PC settings, unmasked exceptions, pending/deferred delivery, CR0/#NM,
WAIT, memory forms, or general x87 semantics. It does not complete S2-31.

Normative source: Intel SDM revision 093, Vol.2A, “FRNDINT—Round to Integer,”
p.3-372; Vol.1 §§8.1.9 and 8.5.6
([official Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=490)).

## Local S2-31dc: canonical signed-zero `FRNDINT` (`D9 FC`)

The exact bare `D9 FC` interpreter gate additionally accepts physical TOP only
when its tag is `Zero`, all eight significand bytes are zero, and its
sign/exponent bytes are exactly `00 00` or `00 80`. Success preserves all ten
source bytes, the Zero tag, TOP, FCW, FDP, FOP, all other physical registers,
GPRs and RFLAGS. It clears C1, adds no exception flags, and preserves sticky
PE and concrete C0/C2/C3. The existing four-FCW whitelist and
`FSW & 0x80df == 0` mask are unchanged. FIP receives starting RIP, RIP advances
by two, and the instruction retires once. No data memory is accessed and no
stack change occurs.

Mismatched tags and zero-like malformed payloads remain Unsupported, as do
large Normals outside the S2-31dd range, subnormals, pseudo-denormals,
infinities, NaNs, unnormals and pseudo-special encodings. Invalid controls,
status bits, prefixes and fetch/truncation/STOP/budget boundaries preserve the
complete state. Tests include a 256-case sign×FCW×TOP×PE×C1 matrix, concrete
C0/C2/C3 combinations, malformed tag/payload rejects, a literal negative-zero
FNSTSW composition, and the unchanged Normal rounding regression. Native x87
and JIT admission remain explicitly closed. FLD m80fp→FRNDINT composition is
not currently covered by a fixture. Physical acceptance remains part of the
combined final S2-31 substage, and S2-31 remains partial under ISSUE-019.

## Local S2-31dd: exact-integer Normal `FRNDINT` (`D9 FC`)

Extend the S2-31a Normal source domain to canonical binary80 values with
unbiased exponent `63..=16383` (biased exponent `0x403e..=0x7ffe`), Valid old
physical TOP tag, J=1, either sign, and any fraction. These values are already
integral, so success returns the original ten bytes unchanged, preserves every
tag and TOP, clears C1, adds no exception flags, and preserves PE and C0/C2/C3.
The identity branch occurs before `63-e` is computed and before rounding or
packing. FCW remains exactly one of `0x037f/0x077f/0x0b7f/0x0f7f`, and
`FSW & 0x80df` must be zero. FDP/FOP and unrelated state are preserved; FIP is
old RIP, RIP advances by two, and retirement is one. No memory or stack access
occurs. Wrong tags, malformed J/exponent encodings, special values, subnormals,
low-exponent Normals outside the existing S2-31a rounding profile, and invalid
environment remain Unsupported atomically.

Tests cover exponents 63, 64, 127, 1023, and 16383 against minimum,
successor, mixed, and maximum significands; both signs; all FCWs/TOPs; and
incoming PE and C1. The exponent-62 fractional boundary still rounds and sets
PE, while exponent 63 is an exact identity with no new PE. A literal
FNSTSW-composition fixture observes the retained status and raw bytes. Native
x87/JIT admission remains closed; physical acceptance and general exception
behavior remain open under ISSUE-019.

Normative source: Intel SDM revision 093, Vol.2A, “FRNDINT—Round to Integer,”
p.3-372; Vol.1 §§8.1.9 and 8.5.6 (same sources as S2-31a above).

## Local S2-31de: canonical signed-infinity `FRNDINT` (`D9 FC`)

The existing bare register-only `D9 FC` gate additionally accepts physical
TOP only when its tag is Special and its raw binary80 bytes encode canonical
signed infinity: significand `0x8000000000000000` and sign/exponent field
`0x7fff` or `0xffff`. It preserves all ten bytes, Special tag, TOP, sticky PE,
and C0/C2/C3; C1 is cleared and no exception flags are added. The exact four
FCW values and `FSW & 0x80df == 0` status mask are unchanged. FDP/FOP and all
unrelated state are preserved, FIP receives the starting RIP, RIP advances by
two, and one instruction retires. No data or stack access occurs.

Pseudo-infinity, NaNs, nonzero fractions, wrong exponents/tags, invalid
controls/status, prefixes and fetch/truncation/STOP/budget boundaries remain
Unsupported or unretired atomically. Existing zero and Normal paths are
unchanged. The 256-case interpreter matrix covers both signs, four FCWs, all
TOPs, incoming PE and both C1 values with independent complete-state
expectations. A literal `FLD m80fp` → `FRNDINT` → `FNSTSW AX` fixture loads
negative infinity and observes the retained payload and status. JIT/native x87
admission remains denied; physical x86 acceptance and general exception
delivery are outside this local contract under ISSUE-019.

Normative encoding source: Intel SDM revision 093, Vol.1 §8.2.2/Table 8-3;
instruction sources are Vol.2A “FLD—Load Floating-Point Value” and
“FRNDINT—Round to Integer,” p.3-372.

## Local S2-31df: canonical quiet-NaN `FRNDINT` (`D9 FC`)

The exact bare register-only `D9 FC` gate additionally accepts the physical
TOP only when its tag is Special and the binary80 bytes encode a canonical
quiet NaN: exponent `0x7fff`, J=1, bit 62=1, either sign, and any lower 62
payload bits. Success is an identity over all ten bytes and the Special tag;
TOP, PE and C0/C2/C3 are preserved, C1 is cleared, and no exception flags are
added. The four accepted FCWs and `FSW & 0x80df == 0` gate are unchanged.
FDP/FOP and unrelated state are preserved, FIP receives old RIP, RIP advances
by two, and the instruction retires once. No memory or stack access occurs.

Quiet-bit-clear SNaNs, pseudo-NaNs with J=0, pseudo-infinity, other exponents,
wrong tags, subnormals and pseudo-denormals remain rejected; the existing
exponent 0..62 Normal rounding slice is unchanged.
Coverage includes four significands × both signs × four FCWs × eight TOPs × PE
× old C1 (1,024 cases), invalid source/tag/control/status, execution-boundary
atomicity and literal `FLD m80fp` → `FRNDINT` → `FNSTSW AX` composition. JIT and
native x87 admission remain closed. Physical x86 acceptance and general
exception delivery remain open under ISSUE-019.

Normative source: Intel SDM revision 093, Vol.1 §8.2.2/Table 8-3 and Vol.2A
“FLD—Load Floating-Point Value” and “FRNDINT—Round to Integer,” p.3-372.

## Local S2-31dg: canonical signaling-NaN `FRNDINT` (`D9 FC`)

The exact bare register-only `D9 FC` gate additionally accepts the physical
TOP only when its tag is Special and the bytes encode a canonical binary80
SNaN: exponent `0x7fff`, J=1, bit 62=0, and at least one of the lower 62
significand bits is set. Setting only bit 62 quiets the value. Preserve its
sign, exponent, J bit, lower payload, Special tag, TOP and all unrelated state.
Add sticky IE and clear C1; add no PE or other exception flags, and preserve
old PE and C0/C2/C3. Keep the four-FCW whitelist and `FSW & 0x80df == 0` gate,
so historical IE remains rejected. Preserve FDP/FOP, set FIP to old RIP,
advance RIP by two and retire exactly once. No memory or stack access occurs.

Canonical infinity (zero lower payload) and QNaNs retain their existing
identity behavior. Pseudo-NaNs, subnormals, pseudo-denormals, unnormals,
malformed inputs, wrong tags, invalid controls or status, and
fetch/truncation/STOP/budget failures remain unsupported and atomic. A
1,024-case independent matrix covers four SNaN significands, both
signs, all four FCWs and TOPs, incoming PE and prior C1. Boundary tests cover
zero lower payload and QNaN identity boundaries, J=0 and wrong tags. The CLI fixture composes
`FLD m80fp`, `FRNDINT`, `FNSTSW AX`, `FNCLEX`, and a second `FRNDINT`; a local
test confirms re-entry before `FNCLEX` is rejected after IE becomes sticky.
JIT/native admission remains closed. Physical x86 acceptance and general
exception delivery remain deferred under ISSUE-019.

Normative source: Intel SDM revision 093, Vol.1 §8.2.2/Table 8-3 and Vol.2A
“FLD—Load Floating-Point Value” and “FRNDINT—Round to Integer,” p.3-372.

## Local S2-31dh: historical sticky IE in bounded `FRNDINT` (`D9 FC`)

The existing exact bare register-only gate admits sticky IE by requiring
`FSW & 0x80de == 0`; the FCW whitelist and all other gate conditions remain
unchanged. IE and PE are independently sticky. A historical IE does not
change the source value, quieting, or current exception result: exact and
identity sources add no flags, inexact Normal rounding adds PE according to
its current remainder, and a current canonical SNaN is quieted and sets IE.
The commit still clears/recomputes C1 from the current instruction and
preserves C0/C2/C3 and all other state.

DE, ZE, OE, UE, SF, ES, and B remain rejected, including when paired with IE.
All source and tag domains, rounding, metadata, atomic execution boundaries,
memory/stack behavior, and unsupported JIT/native admission remain unchanged.
Coverage spans signed zero, exact/inexact and large integral Normals, signed
infinity, QNaN and SNaN across sign, FCW, TOP, all IE/PE sticky combinations,
and old C1; it includes IE plus each forbidden bit, successive SNaN then QNaN
FRNDINT without FNCLEX, and the existing FNSTSW/FNCLEX fixture sequence.
This local contract does not add an IE producer or define general exception
delivery.

## Local S2-31dj: historical sticky DE in bounded `FRNDINT` (`D9 FC`)

The exact bare register-only `D9 FC` gate now uses `FSW & 0x80dc == 0`,
admitting and preserving historical DE alongside IE and PE. The exact FCW
whitelist `037f/077f/0b7f/0f7f` is unchanged. Current source classification,
rounding, SNaN quieting/current IE behavior, PE generation and C1 recomputation
remain independent of historical DE. ZE, OE, UE, SF, ES and B remain rejected,
also when combined with any admitted IE/DE/PE bits. Source/tag domains,
execution atomicity, metadata, memory and stack behavior, and JIT/native
denial remain unchanged.

Interpreter coverage crosses supported source classes, canonical-Normal
exponent representatives, signs, RCs, TOPs, IE/DE/PE sticky combinations and
prior C1. The literal
seeded `D9 FC DF E0` fixture begins with IE/DE/PE on under FCW `0x037f` and
checks sticky status plus C1 recomputation. The literal
`FLD m32fp/m64fp subnormal → FRNDINT → FNSTSW AX` CLI fixtures verify that
FLD's subnormal input first becomes a canonical-Normal stack value with DE,
then FRNDINT rounds it while retaining DE and adds PE for the inexact result.
Other forbidden status bits paired with admitted IE/DE remain atomic rejects.
The four all-masked FCW values remain the only accepted controls; native/JIT
admission stays closed.

Physical x86/Mac mini acceptance and general x87 exception delivery remain
open under ISSUE-019 and are deferred to the combined final S2-31 acceptance
substage.

## Local S2-31dk: historical sticky UE in bounded `FRNDINT` (`D9 FC`)

The bare register-only `D9 FC` gate now uses `FSW & 0x80cc == 0`, admitting
historical UE alongside the already admitted IE, DE and PE. UE remains sticky.
An exact or identity source does not create PE from UE alone; current inexact
Normal rounding adds sticky PE, while a current canonical SNaN adds sticky IE
and quiets its payload without adding PE. The exact FCW whitelist
`037f/077f/0b7f/0f7f`, source/tag domains, rounding, C1 recomputation,
metadata, memory/stack behavior and atomicity are unchanged. ZE, OE, SF, ES and
B remain rejects even when UE and other admitted sticky bits are present.
Unsupported classes remain atomic rejects, and native/JIT admission remains
closed.

Interpreter vectors cross exact Normal, inexact Normal and canonical SNaN
sources with all four RCs, every TOP, incoming IE/DE/PE combinations and prior
C1. The exact UE-only case verifies no PE is synthesized, the inexact case
verifies UE+PE, and SNaN verifies UE+IE without PE. Literal CLI fixtures cover
seeded exact UE with `D9 FC DF E0` and `FST m32fp` underflow followed by
`FRNDINT` and `FNSTSW AX`; the latter verifies the store-generated UE and PE
pass through FRNDINT. Physical x86/Mac mini acceptance and general x87
exception behavior remain deferred to the combined final S2-31 acceptance
substage under ISSUE-019.

## Local S2-31dl: historical sticky OE in bounded `FRNDINT` (`D9 FC`)

The bare register-only `D9 FC` gate now uses `FSW & 0x80c4 == 0`, admitting
historical OE alongside IE, DE, UE and PE. OE is preserved and does not itself
create PE. Exact and identity operations keep incoming PE unchanged; current
inexact Normal rounding adds PE, and current canonical SNaN quieting adds IE
without PE. The all-masked FCWs, source/tag domain, rounding, C1 behavior,
metadata and atomicity remain unchanged. ZE, SF, ES and B remain atomic rejects
when combined with OE and any admitted sticky flags. Native/JIT admission stays
closed.

Independent interpreter coverage crosses exact and fractional Normal and
canonical SNaN over signs, all four RCs, every TOP, incoming IE/DE/UE/PE and
prior C1. Identity coverage includes signed zero, large integral Normal,
infinity and QNaN. Literal CLI fixtures cover seeded OE and
`FST m32fp` overflow → `FRNDINT` → `FNSTSW AX` over `(2 − 2^-63) × 2^127`;
the latter begins with FSW=0,
checks the store's OE+PE+C1 production, then observes sticky OE+PE after
FRNDINT clears C1. Physical x86/Mac mini acceptance and general x87 exception
behavior remain deferred to the combined final S2-31 substage under ISSUE-019.

## Local S2-31dm: historical sticky ZE in bounded `FRNDINT` (`D9 FC`)

The exact bare register-only `D9 FC` gate uses `FSW & 0x80c0 == 0`, admitting
historical ZE while continuing to reject SF, ES and B. ZE is retained as
incoming sticky state; this FRNDINT slice does not produce ZE. Historical
IE/DE/OE/UE/PE remain independently sticky. Exact/identity sources do not add
PE, fractional canonical-Normal sources add PE, and a current canonical SNaN
is quieted and adds IE without PE. The exact four-FCW whitelist, source/tag
domains, rounding, C1 recomputation, metadata and atomicity remain unchanged.
No FDIV-zero producer or producer-domain change is included, and JIT/native
admission stays closed.

Interpreter coverage crosses seeded ZE and all combinations of IE/DE/OE/UE/PE
with supported source classes, signs, all FCWs/TOPs and prior C1. SF/ES/B
remain atomic rejects when paired with ZE and sticky flags. The literal seeded
`D9 FC DF E0` CLI fixture observes ZE+OE in AX and FSW; its provenance
explicitly identifies the ZE as seeded, with no producer in the guest code.
Physical x86/Mac mini acceptance and general x87 exception delivery remain
deferred to the combined final S2-31 acceptance substage under ISSUE-019.

## Local S2-31dn: historical SF+IE in bounded `FRNDINT` (`D9 FC`)

For the existing bare register-only `D9 FC` slice, retain FCWs
`0x037f/0x077f/0x0b7f/0x0f7f`. The base status gate is
`FSW & 0x8080 == 0`; an additional guard rejects when SF is set without IE.
Therefore ES/B always reject, and SF-only rejects, while historical SF is
admitted only with IE. Historical SF+IE and independent DE/ZE/OE/UE/PE remain
sticky. Existing source and tag domains are unchanged: Empty, mismatched and
raw-malformed sources remain Unsupported atomically. Current FRNDINT behavior
still determines new IE/PE and the result; status commit preserves old C1 only
as input to the clear operation, then sets C1 solely from the current rounded
result. Both SF and IE survive successful FRNDINT. JIT/native admission remains
closed. ES/B and pending/#MF/WAIT behavior are unchanged.

The independent matrix covers SF+IE × 32 combinations of DE/ZE/OE/UE/PE ×
eight source classes × both signs × four FCWs × all TOP values × both prior C1
values (32,768 cases). Separate controls reject SF-only for ordinary and
current-SNaN inputs and retain ES/B rejection; existing malformed/empty/tag
mismatch controls remain active. The seeded literal `D9 FC DF E0` fixture
starts with source 1.0, FCW `0x037f`, and FSW `0x0241`, then observes
AX/FSW `0x0041`: IE and SF are present in its initial project_fixture snapshot,
not produced by a guest instruction, and the fixture is not native evidence.
The normative basis is Intel SDM revision 093 Vol.1 §8.1.3.4 p.8-6
([official volume](https://cdrdv2-public.intel.com/v1/dl/getContent/671436)) and
§8.5.1.1 p.8-26 (stack fault sets IE+SF), plus Vol.2A FRNDINT p.3-372
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=490)).
Physical x86 acceptance and general exception delivery remain deferred to the
combined final S2-31 acceptance substage under ISSUE-019.

## Local S2-31di: small canonical-Normal `FRNDINT` (`D9 FC`)

Extend the exact bare register-only S2-31a gate to admit only Valid-tagged,
canonical Normal binary80 values with unbiased exponent `-16382..=-1` (biased
exponent `1..=0x3ffe`), either sign. Keep the four-FCW whitelist and
`FSW & 0x80de == 0`; pre-existing IE and PE remain independently sticky.

Every admitted source is nonzero and inexact. Since its magnitude is below
one, nearest-even returns signed zero except when `e=-1` and the significand
is strictly greater than `0x8000000000000000`; exact one-half ties to zero.
Toward-zero returns signed zero. Toward-negative returns `-1` only for a
negative source; toward-positive returns `+1` only for a positive source.
Zero is encoded with the source sign, a zero significand and `X87Tag::Zero`;
one is canonical Valid binary80 `±1` (`J=1`, exponent `0x3fff`). Every result
adds sticky PE, C1 is set exactly for magnitude one, and C0/C2/C3 plus all
other status bits are preserved.

The small-exponent branch uses comparisons on exponent/significand and never
forms the oversized `2^(63-e)` denominator. Commit updates only old physical
TOP's payload/tag, PE/C1, FIP, RIP and retirement; TOP, FDP, FOP, unrelated
registers/tags, GPR/RFLAGS and data memory remain unchanged. Canonical-range,
source-class/tag, FCW/FSW, fetch/truncation/STOP/budget rejects are atomic.
Coverage spans ±1/2 and adjacent representable values, values just below one,
`e=-2,-63,-64,-65,-16382`, TOP, all FCWs, incoming IE/PE, prior C1, rounded
zero re-entry, full payload/tag/status preservation, and a literal
FRNDINT→FNSTSW composition. JIT/native admission remains closed; subnormals,
pseudo-denormals, malformed encodings, mismatched tags, unmasked exceptions,
and general x87 exception delivery remain outside this slice. Physical
acceptance is deferred to the combined final S2-31 substage under ISSUE-019.

## Local S2-31do: canonical binary80 subnormal `FRNDINT` (`D9 FC`)

Extend the existing bare register-only Long64 `D9 FC` gate only for physical
ST(0) tagged Special whose raw bytes are a canonical nonzero binary80 subnormal:
biased exponent zero, J=0, and nonzero fraction, with either sign. Pseudo-
denormals, malformed encodings, tag mismatches, and unsupported neighboring
raw encodings remain Unsupported atomically. Existing admitted Zero, Normal,
infinity and NaN paths are unchanged. The exact
four-FCW whitelist and current `FSW & 0x8080 == 0`, SF-requires-IE status gate
remain in force; sticky IE, DE, PE and SF+IE are retained.

Every admitted magnitude is below one-half. RN and RZ return signed zero;
RD returns -1 for negative input and +0 for positive input; RU returns +1 for
positive input and -0 for negative input. Every execution adds sticky DE and
PE. C1 is set exactly for either signed-one result and otherwise cleared; C0,
C2, C3, TOP, FCW, unrelated tags/registers, FDP/FOP, GPRs and RFLAGS are
preserved. Result one uses canonical binary80 with Valid tag; result zero uses
signed zero with Zero tag. FIP receives old RIP and RIP advances by two. No
data memory or stack access occurs. Current DE is carried through the local
preflight/commit tuple; this does not introduce a general exception framework.

Coverage includes minimum and maximum canonical fractions plus two interior
fractions across sign, RC, TOP, prior DE/PE/C1 and legal IE/SF pairings, complete
state canaries, repeat FRNDINT, FNSTSW/FNCLEX and atomic neighbor rejects. The
seeded literal `D9 FC DF E0` fixture uses explicit `project_fixture`
provenance; it is not native evidence. JIT/native admission remains denied.
Physical x86 and Mac mini acceptance, unmasked exception delivery and broader
x87 semantics remain deferred to the combined final S2-31 substage under
ISSUE-019.

Normative source: Intel SDM revision 093, Vol.1 §8.2.2/Table 8-3 and Vol.2A
“FRNDINT—Round to Integer,” p.3-372.

## Local S2-31dp: Special-tagged binary80 pseudo-denormal `FRNDINT` (`D9 FC`)

For bare Long64 `D9 FC`, the existing Special-tagged finite-input branch also
accepts a pseudo-denormal with biased exponent zero, explicit integer bit J=1,
and any fraction including zero. Intel SDM revision 093 Vol.1 §8.2.2 gives
these encodings effective exponent -16382; as arithmetic operands they raise
DE. They round identically to the corresponding E=1 canonical Normal with the
same significand: RN/RZ produce signed zero, RD produces -1 for negative and
+0 for positive, and RU produces +1 for positive and -0 for negative. Each
current pseudo-denormal operation adds sticky DE and PE; C1 is set only for
either signed-one result. Results use Zero or Valid tags accordingly.

The implementation only extends the exact Special finite branch. The decoder,
classification, FCW/status gates, preflight/commit atomicity, metadata and
register-only behavior remain unchanged. Pseudo-infinity/NaN, malformed
inputs, other tags and encoding/tag mismatches remain Unsupported atomically.
At this stage, unnormal inputs were also rejected; S2-31dq below adds their
separate masked-invalid response without changing this pseudo-denormal path.
JIT/native admission remains closed.

Coverage compares pseudo-denormal and canonical E=1 Normal encodings over
fraction 0/1/interior/max, sign, all four RC modes, every TOP, prior DE/PE/C1
and legal IE/SF sticky combinations. Result bytes, result tags, PE and C1
match; pseudo-denormals add DE while canonical Normals preserve only incoming
DE. A literal `DB 28 D9 FC DF E0` `project_fixture` demonstrates that bounded
FLD retains raw E=0/J=1/Special and FRNDINT then adds DE+PE. Repeat FRNDINT,
FNSTSW, FNCLEX and re-entry are checked locally. Physical x86 and Mac mini
acceptance, unmasked exception delivery and broader x87 semantics remain
deferred to the combined final S2-31 substage under ISSUE-019.

Normative source: Intel SDM revision 093, Vol.1 §8.2.2 and Table 8-3, and the
Vol.2A “FRNDINT—Round to Integer” entry, which specifies #D for a denormal
source operand and #P for a nonintegral result.

<a id="local-s2-31dq-special-unnormal-frndint-d9-fc"></a>

## Local S2-31dq: Special-tagged binary80 unnormal `FRNDINT` (`D9 FC`)

Only exact bare Long64 `D9 FC` accepts this added class: physical TOP must be
tagged Special and the raw binary80 biased exponent must be 1..=`0x7ffe`, J=0,
with either sign and any fraction. For the four existing all-masked FCWs
`0x037f`, `0x077f`, `0x0b7f`, `0x0f7f`, the result is always the fixed negative
floating-point indefinite `00 00 00 00 00 00 00 c0 ff ff` (little-endian bytes),
with Special tag. Source sign and payload are discarded. The operation adds
sticky IE, clears C1, and adds no current DE/PE/SF/ZE/OE/UE. Previously admitted
sticky IE/DE/ZE/OE/UE/PE are preserved, as is SF only when paired with IE.

This is a local masked-invalid response for an unsupported binary80 encoding.
The result is the existing canonical binary80 QNaN indefinite, so a repeated
FRNDINT preserves the ten bytes and adds no new flags. The established FCW and
FSW admission gates, TOP, unrelated registers/state, FIP/RIP/retirement and
instruction preflight/commit boundary are unchanged. Mismatched tags/classes,
other controls/status, malformed encodings and fetch/STOP/budget boundaries
remain atomic rejects. This does not change FLD's rejection of Unnormal or
binary80 classification; the existing SNaN quieting path remains separate.
JIT/native admission remains denied.

The independent expected-value matrix covers exponent 1/interior/`0x7ffe`,
fraction 0/1/bit62/max, both signs, all RC modes and TOP values, each legal
historical IE/SF pairing, all combinations of prior DE/ZE/OE/UE/PE, and prior
C1. Full-state canaries and no-memory-access behavior are checked. Repeat
FRNDINT plus a literal `D9 FC DF E0 DB E2` project fixture verify QNaN
re-entry, FNSTSW observation of IE and FNCLEX clearing; the seeded raw Special
state is project-fixture evidence, not native evidence.

Normative basis: Intel SDM revision 093 Vol.2A “FRNDINT—Round to Integer,”
p.3-372, lists unsupported format as #IA; Vol.1 Table 8-10 maps an arithmetic
operation on an unsupported format to the QNaN floating-point indefinite;
Vol.1 Table 4-3 and §4.8.3.7 define its binary80 encoding. The separate SNaN
masked response in Table 8-10 is not used here. See the [plan](../plans/STAGE_2.md#s2-31dq-special-tagged-binary80-unnormal-through-bounded-frndint), [report](../reports/STAGE_2_REPORT.md#s2-31dq--special-tagged-binary80-unnormal-through-bounded-frndint), and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-31dq-frndint-unnormal-fixture).

<a id="local-s2-31dr-special-pseudo-infinity-frndint-d9-fc"></a>

## Local S2-31dr: Special-tagged binary80 pseudo-infinity `FRNDINT` (`D9 FC`)

Only exact bare Long64 `D9 FC` adds this input: physical TOP is Special-tagged
and its binary80 encoding has E=`0x7fff`, J=0, zero fraction and either sign.
Under FCW `0x037f`, `0x077f`, `0x0b7f` or `0x0f7f`, FRNDINT returns the fixed
negative floating-point indefinite bytes `00 00 00 00 00 00 00 c0 ff ff` and
retains Special. It sets sticky IE, clears C1, and raises no new DE/PE/SF/ZE/OE/
UE. Existing IE/DE/ZE/OE/UE/PE are preserved, as is SF only when IE is already
set. All existing control/status gates and preflight/commit behavior remain.

S2-31dr itself did not admit pseudo-NaNs (E=`0x7fff`, J=0, nonzero fraction);
S2-31ds below adds that class separately. Canonical J=1 infinity, QNaN and
SNaN retain their prior FRNDINT behavior. E=0/J=0/
zero-fraction with Special tag is a canonical-zero/tag mismatch and remains
Unsupported. FLD, the binary80 classifier, JIT and native admission are
unchanged. TOP, unrelated physical registers, FCW, data pointer, opcode,
GPR/RFLAGS, and memory behavior remain as before. The invalid indefinite is a
canonical QNaN identity on repeated FRNDINT.

The independent matrix crosses both signs, four rounding controls, all TOPs,
three legal historical IE/SF combinations, all 32 DE/ZE/OE/UE/PE combinations
and prior C1. Full state and no-memory-access canaries plus repeat and exact
neighbor rejection are covered. The literal seeded `D9 FC DF E0 DB E2`
`project_fixture` observes IE with FNSTSW and clears it with FNCLEX; it is not
native evidence. Mac mini, SSH and physical x86 execution are excluded here;
remaining x87/host acceptance stays open under ISSUE-019. See the [plan](../plans/STAGE_2.md#s2-31dr-special-tagged-binary80-pseudo-infinity-through-bounded-frndint), [report](../reports/STAGE_2_REPORT.md#s2-31dr--special-tagged-binary80-pseudo-infinity-through-bounded-frndint), and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-31dr-frndint-pseudo-infinity-fixture).

<a id="local-s2-31ds-special-pseudo-nan-frndint-d9-fc"></a>

## Local S2-31ds: Special-tagged binary80 pseudo-NaN `FRNDINT` (`D9 FC`)

Only exact bare Long64 `D9 FC` adds this input: physical TOP is Special-tagged
and binary80 has E=`0x7fff`, J=0, nonzero fraction and either sign. The fraction
may have Q=bit 62 clear or set. Under FCW `0x037f`, `0x077f`, `0x0b7f` or
`0x0f7f`, FRNDINT returns fixed negative floating-point indefinite bytes
`00 00 00 00 00 00 00 c0 ff ff`, retains Special, sets sticky IE and clears
C1. It raises no new DE/PE/SF/ZE/OE/UE. Existing IE/DE/ZE/OE/UE/PE survive;
SF survives only paired with historical IE. This is the same local masked
unsupported-format path as Special-tagged unnormals and pseudo-infinity.

Do not apply canonical SNaN quieting or preserve the pseudo-NaN sign/payload.
Canonical J=1 infinity, QNaN and SNaN retain their established behavior.
Existing FCW/status/prefix/fetch/stop/budget gates, preflight atomicity, TOP,
metadata and all other physical registers are unchanged. FLD, binary80
classification and JIT/native admission are unchanged; wrong-tag inputs remain
Unsupported. The fixed result is a canonical QNaN and remains identical on a
repeated FRNDINT.

The independent 49,152-case matrix covers four literal fractions
`1`, `0x3fff_ffff_ffff_ffff`, `0x4000_0000_0000_0000`, and
`0x7fff_ffff_ffff_ffff`, both signs, all four RC controls, eight TOP values,
three legal historical IE/SF combinations, all 32 prior DE/ZE/OE/UE/PE
combinations and both prior C1 values, comparing full state and memory
canaries. Literal seeded Q=0 and Q=1 `D9 FC DF E0 DB E2` project fixtures
observe IE through FNSTSW and clear it with FNCLEX. They are not native
evidence. Mac mini, SSH and physical x86 execution are excluded; ISSUE-019
remains open.

Normative basis: Intel SDM revision 093 Vol.2A “FRNDINT—Round to Integer,”
p.3-372, lists unsupported format as #IA; Vol.1 §8.2.2 Table 8-3 and Table
8-10 define masked invalid handling and its indefinite result; Vol.1 Table
4-3 defines the binary80 indefinite encoding. The canonical SNaN response in
Table 8-10 is not used for pseudo-NaNs. See the [plan](../plans/STAGE_2.md#s2-31ds-special-tagged-binary80-pseudo-nan-through-bounded-frndint), [report](../reports/STAGE_2_REPORT.md#s2-31ds--special-tagged-binary80-pseudo-nans-through-bounded-frndint), and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-31ds-frndint-pseudo-nan-fixtures).

## Local S2-30al: bounded `FUCOMI ST(0), ST(1)` (`DB E9`)

Accept exactly bare `DB E9`. Before mutation call
`bounded_x87_fcom_result(state, 1)`, requiring an existing x87 snapshot, FCW
`0x037f`, `FSW & 0x80ff == 0`, Valid tags at old physical TOP and
`(TOP+1)&7`, and canonical-Normal binary80 values. The bounded comparator
returns only ordered relations: greater clears all six affected RFLAGS bits,
less sets CF, and equal sets ZF. `FlagEffects` defines CF/PF/AF/ZF/SF/OF
(`0x8d5`) on successful retirement.

Clear only FSW.C1; preserve C0/C2/C3, TOP, both physical tags, all ten-byte
register payloads, FCW, FDP/FOP, other FSW bits, GPRs and memory. Set FIP to
instruction-start RIP, advance RIP by two, and retire once. Reject missing
state, bad environment, tags and classes atomically. No pop, unordered QNaN
handling, x87 exception delivery, `#NM` handling or native x87 execution is
modeled.

The DB decoder admits only E2 FNCLEX, E3 FNINIT, F1 FCOMI and E9 FUCOMI as
bare encodings. The existing DF E0 FNSTSW AX and DF F1 FCOMIP forms remain
admitted; all other DB/DF forms, prefixes, REX, WAIT and memory operands are
rejected. Literal schema/CLI fixtures use exact `dbe9dfe0` and `dbe9`; the
no-stop/budget-one reject has Empty actual old physical ST(1). The schema
rejects `dbf8dfe0`. Decoder and interpreter tests cover every DB second byte
and prefix, all TOP × ordered relations × initial FSW condition fields, all 64
affected RFLAGS masks with definedness, invalid operand tags/classes and
environment, and fetch/truncation/STOP/budget atomicity. Native admission
explicitly denies `Operation::Fucomi` and DB E9.

Normative source: Intel SDM revision 093, Vol.2A,
“FCOMI/FCOMIP/FUCOMI/FUCOMIP—Compare Floating-Point Values,” pp.3-327–3-329
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

## Local S2-30am: bounded `FUCOMIP ST(0), ST(1)` (`DF E9`)

Accept exactly bare `DF E9`. Before any mutation, call
`bounded_x87_fcom_result(state, 1)`, which requires an existing x87 snapshot,
FCW `0x037f`, `FSW & 0x80ff == 0`, Valid tags at old physical TOP and
`(TOP+1)&7`, and canonical-Normal binary80 operands. Greater clears the six
affected RFLAGS bits, less sets CF, and equal sets ZF. `FlagEffects` defines
CF/PF/AF/ZF/SF/OF (`0x8d5`) on successful retirement.

Clear only FSW.C1; empty the tag at actual old physical TOP and set TOP to
`(old TOP+1)&7`. Preserve old ST(1)'s Valid tag, every physical raw payload,
C0/C2/C3, FCW, FDP/FOP, every other FSW bit, GPRs and memory. Set FIP to the
instruction-start RIP, advance RIP by two and retire once. Missing state, bad
environment/tag/class, fetch/truncation, STOP and exhausted budget reject
without commit. Unordered comparisons, special values, x87 exceptions and
native execution are excluded.

The exact DF allowlist is E0 FNSTSW AX, E9 FUCOMIP and F1 FCOMIP. Existing DB
E2 FNCLEX, E3 FNINIT, E9 FUCOMI and F1 FCOMI remain admitted; all other DB/DF
forms, prefixes, REX, WAIT and memory operands reject. Exact literal schema/CLI
programs are `dfe9dfe0` and `dfe9`; the no-stop/budget-one rejection sets TOP=5
and the actual physical `(TOP+1)&7` tag to Empty. Schema and CLI reject
`dff8dfe0` and the 66/48/9B-prefixed forms. Interpreter coverage spans every
TOP × ordered relation × 16 initial FSW condition fields and all 64 affected
RFLAGS masks with definedness, including TOP wrap and atomic invalid-input and
execution-boundary rejection. Native admission explicitly denies
`Operation::Fucomip` and DF E9.

Normative source: Intel SDM revision 093, Vol.2A,
“FCOMI/FCOMIP/FUCOMI/FUCOMIP—Compare Floating-Point Values,” pp.3-327–3-329
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

## Local S2-30an: indexed bounded FCOMI-family register forms

Accept exactly the bare two-byte register forms `DB E8+i` FUCOMI,
`DB F0+i` FCOMI, `DF E8+i` FUCOMIP, and `DF F0+i` FCOMIP for `i=0..7`.
The decoder records the low three ModR/M bits as `source:u8`. Existing DB E2
FNCLEX, DB E3 FNINIT, and DF E0 FNSTSW AX retain their behavior. Every other
DB/DF byte, legacy or REX prefix, WAIT, and memory ModR/M form is Unsupported.

Before mutation, preflight `bounded_x87_fcom_result(state, i)`: require an
existing x87 snapshot, FCW `0x037f`, `FSW & 0x80ff == 0`, and Valid canonical-
Normal old physical TOP. If `i != 0`, also require and read Valid canonical-
Normal actual physical `(TOP+i)&7`. If `i=0`, compare TOP with itself and do
not inspect ST(1) or any other register. The relation sets RFLAGS after
clearing mask `0x8d5`: greater sets none, less sets CF, equal sets ZF; all six
masked bits become defined on retirement. Clear only FSW.C1.

FCOMI/FUCOMI preserve TOP, tags and all payloads. FCOMIP/FUCOMIP empty actual
old TOP and advance TOP once with wrap, including the `i=0` alias. All payload
arrays remain unchanged. Every family records FIP, advances RIP by two, and
retires once. Used operand/environment failures and fetch/truncation/STOP/
budget boundaries reject atomically. No unordered/special values, exception
delivery, CR0/#NM, or native x87 execution are included.

The schema accepts exactly 32 encodings alone or followed by DF E0, totaling
64 exact program strings. Its budget-one/no-stop source rejection is narrow:
for nonzero `i`, the actual encoded physical source tag must be invalid while
the old TOP remains Valid; for `i=0`, the Valid old TOP uses an exact all-zero
payload to exercise class rejection. Existing `i=1` cases remain accepted.
Exhaustive decoder tests cover all second bytes and every legacy/REX/WAIT
prefix. Interpreter matrices cover all TOP × source × ordered relation × 16
initial FSW condition values and all 64 initial affected-RFLAGS masks for each
family and source, including self compare with other tags Empty and physical
slot/pop wrap behavior. Literal fixtures cover these boundaries and real
actual-source rejects. Native instruction admission explicitly denies all
four indexed `Operation` families.

Normative source: Intel SDM revision 093, Vol.2A,
“FCOMI/FCOMIP/FUCOMI/FUCOMIP—Compare Floating-Point Values,” pp.3-327–3-329
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

## Local S2-31s: bounded `FLD m32fp [RAX]` (`D9 00`)

The interpreter admits only exact bare `D9 00` in Long64 with existing x87
state. It requires FCW `0x037f`, `0x077f`, `0x0b7f`, or `0x0f7f`,
`FSW & 0x80df == 0`, and an Empty physical destination `(TOP+7)&7`. The old
logical ST(0) is not inspected. Before memory access it checks the full
canonical48 Dword range `[RAX,RAX+3]` and checked endpoint addition, then makes
exactly one `GuestMemory::read_integer` Dword read. The resulting four little-
endian source bytes are accepted only for exponent fields 1..254. Zero,
subnormal, infinity and NaN return local Unsupported after that single read,
without CPU commit.

Conversion is an exact integer expansion. The binary80 significand is
`((1<<23)|fraction)<<40`; the exponent is `source_exponent - 127 + 16383`; the
source sign is preserved. All binary32 Normal values are included, including
source exponent 127 and exponent fields 1 and 254. No host float or store-side
conversion helper is used.

On success, the destination receives the expanded raw payload and Valid tag,
TOP becomes `(old_TOP+7)&7`, and C1 is cleared. PE and every other FSW bit
except the TOP field are retained; retaining C0/C2/C3 is the local policy.
FCW/FOP, unrelated slots, GPRs and RFLAGS are preserved. FIP/FDP are recorded,
RIP advances by two, and the instruction retires once. Architectural read
faults follow the guest-fault path and backend errors remain diagnostics.
Every other D9 ModR/M value and every prefix/REX/WAIT encoding remains outside
the admitted form. Native admission explicitly denies `Operation::FldM32FpRax`.

Interpreter checks use independent literal binary80 payloads for ±1, adjacent
fractions, minimum and maximum Normal inputs, and exponent 127, across all
TOP×FCW settings. They also check sticky PE/C1 clearing, local condition-code
retention, preserved unrelated slots and state, one unaligned four-byte read
with no writes/canaries, special values rejected after one read, invalid gates
before access, canonical crossing/overflow, injected #PF/backend errors, and
fetch/truncation/STOP/zero-budget atomicity. Exact schema/CLI fixtures cover
`D9 00`, `D9 00 DF E0`, and invalid-class no-commit behavior. Exhaustive D9
decode coverage preserves the existing D9 forms and rejects all prefixes.
Physical x86 behavior, general x87 exception delivery, MMU/MMIO/SMP and native
x87 state remain unverified; S2-31 stays partial under ISSUE-019.

## Local S2-31y: bounded `FLD m32fp [RAX]` (`D9 00`) masked-subnormal extension

S2-31y extends exact bare `D9 00` to nonzero binary32 subnormal sources while
retaining the bounded S2-31v gates: existing x87 state, FCW `037f/077f/0b7f/
0f7f`, Empty physical destination `(TOP+7)&7`, complete canonical48 Dword
range, and one Dword read with no writes. The D900-only FSW gate is now
`FSW & 0x80dd == 0`, allowing a pre-existing sticky DE while continuing to
reject all other masked status bits. At that point FILD, m64fp and m80fp
retained their existing `0x80df` status gate; S2-31z later extends the DD00
path with its own DE-permitting preflight, while FILD and m80fp stay strict.

For exponent field zero and nonzero fraction `f`, let `k=floor(log2(f))`.
The pushed binary80 payload is canonical Normal with significand
`f << (63-k)`, unbiased exponent `k-149` (biased exponent `16383+k-149`),
and the source sign. Its tag is Valid. The operation sets sticky FSW.DE and
does not set UE or PE; it clears C1 and preserves other flags, payloads, TOP
and pointer behavior as in the existing D900 load policy. Repeated masked
subnormal loads retain DE. Binary32 signed zero remains Zero-tagged; Normal
expansion is unchanged. At the S2-31y stage, infinity and NaN remained
Unsupported after one read without CPU commit; S2-31af now accepts infinity
and narrows the current Unsupported schema case to canonical QNaN. Unmasked
FCWs remain outside the supported profile; no exception delivery or unmasked
push behavior is claimed. Native admission remains closed.

Independent literal vectors cover signed minimum, interior and largest
subnormals across all TOP×FCW pairs, including sticky PE/C1 behavior and a
repeated load. Exact D900 positive-subnormal and D900/DFE0 negative-subnormal
fixtures exercise the CLI and FNSTSW composition. At the S2-31y stage, the narrow Unsupported schema fixture used infinity or
NaN, exact D900, budget one and no stop RIP. S2-31af supersedes this: infinity
is supported and the current exception fixture uses canonical QNaN only. The
former S2-31v rejection of minimum subnormal records that historical contract
only.

Normative source: Intel SDM revision 093, Vol.1 §8.5.2 for masked denormal
operand handling; Vol.2A, “FLD—Load Floating-Point Value,” pp.3-352–3-353,
official PDF p.470
([Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=470)).
The local contract is limited to the all-masked profile and does not infer
unmasked exception behavior. The separate binary80 m80fp subnormal behavior
remains as documented in S2-31x.

## Local S2-31z: bounded `FLD m64fp [RAX]` (`DD 00`) masked-subnormal extension

S2-31z extends exact bare `DD 00` to nonzero binary64 subnormal sources while
retaining the bounded S2-31u gates: existing x87 state, FCW `037f/077f/0b7f/
0f7f`, Empty physical destination `(TOP+7)&7`, complete canonical48 Qword
range, and one Qword read with no writes. The DD00-only FSW gate is
`FSW & 0x80dd == 0`, allowing a pre-existing sticky DE while rejecting the
other masked status bits. FILD and m80fp retain their existing `0x80df`
status gate; the D900 gate retains the separate S2-31y policy.

For exponent field zero and nonzero fraction `f`, let `k=floor(log2(f))`.
The pushed binary80 payload is canonical Normal with significand
`f << (63-k)`, unbiased exponent `k-1074` (biased exponent `16383+k-1074`),
and the source sign. Its tag is Valid. The operation sets sticky FSW.DE and
does not set UE or PE; it clears C1 and preserves other flags, payloads, TOP
and pointer behavior as in the existing DD00 load policy. Repeated masked
subnormal loads retain DE. Binary64 signed zero remains Zero-tagged; Normal
expansion is unchanged. At the S2-31z stage, infinity and NaN remained
Unsupported after the one read with no CPU commit; S2-31ae added infinity and
S2-31ag adds canonical QNaN. SNaN remains Unsupported. Unmasked controls
remain outside the supported profile; no exception delivery or unmasked push
behavior is claimed. Native admission remains closed.

Independent literal vectors cover signed minimum, interior and largest
subnormals across all TOP×FCW pairs, including sticky PE/C1 behavior and a
repeated load. Exact DD00 positive-subnormal and DD00/DFE0
negative-subnormal fixtures exercise the CLI and FNSTSW composition. The
narrow Unsupported schema fixture now uses infinity; its admission requires
exact DD00, budget one, no stop RIP, and binary64 exponent field 2047. The
earlier S2-31u report records the signed-zero extension with subnormal
rejection as it stood at that time.

Normative source: Intel SDM revision 093, Vol.1 §8.5.2 for masked denormal
operand handling; Vol.2A, “FLD—Load Floating-Point Value,” pp.3-352–3-353,
official PDF p.470
([Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=470)).
This local contract is limited to the all-masked profile and does not infer
unmasked exception behavior. The separate binary80 m80fp subnormal behavior
remains as documented in S2-31x.

## Local S2-31aa: bounded `FLD m80fp [RAX]` (`DB 28`) canonical infinities

S2-31aa extends exact bare `DB 28` to canonical positive and negative binary80
infinities. It retains the S2-31x preflight: existing x87 state, FCW
`0x037f`/`0x077f`/`0x0b7f`/`0x0f7f`, `FSW & 0x80df == 0`, Empty physical
destination `(TOP+7)&7`, complete canonical48 ten-byte range and one raw
read with no writes. After that read, accept only exponent `0x7fff`, explicit
integer bit set, and zero fraction. Preserve the raw ten-byte payload and push
with `X87Tag::Special`.

The existing push/TOP, C1-clear, FCW/FOP, pointer and single-retirement
behavior remains unchanged; the load adds no status flags. Normal, signed zero
and canonical subnormal behavior remains as documented in S2-31t/w/x. At the
S2-31aa stage, NaNs, pseudo-infinities, pseudo-denormals, pseudo-NaNs and
unnormals remained Unsupported after one read without CPU commit; S2-31ab
supersedes only canonical QuietNaNs. No x87
exception delivery or unmasked behavior is implemented. Native x87 admission
remains closed.

Signed infinity literals cover all TOP×FCW pairs and assert exact payload,
Special tag, status preservation, one ten-byte read, and no writes. CLI
fixtures cover bare positive infinity and negative infinity followed by
FNSTSW. At the S2-31aa stage the narrow Unsupported schema fixture used
positive canonical QNaN; S2-31ab changes the current fixture to positive
canonical SNaN, still restricted to exact `DB 28`, budget one, and no stop RIP.
Pseudo-denormals, infinities, QNaNs and other neighboring encodings are excluded
from the rejection exception predicate. Memory faults, canonical-address preflight,
fetch/truncation, STOP, budget and atomicity checks remain in force.

Normative source: Intel SDM revision 093, Vol.1 Table 4-3 gives the canonical
double-extended infinity encoding (all exponent bits set, integer bit set, zero
fraction); Table 8-3 distinguishes pseudo-infinity and unsupported encodings
([Vol.1 PDF](https://cdrdv2.intel.com/v1/dl/getContent/671436#page=92)). Vol.2A
FLD m80fp is the instruction reference and specifies no `#D`
([Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=470)).
No unmasked exception or physical-hardware behavior is inferred. See the
[Stage 2 plan](../plans/STAGE_2.md#s2-31aa-extend-bounded-fld-m80fp-rax-db-28-to-canonical-infinities),
[fixture notes](../../tests/fixtures/x86/stage2/README.md), and
[S2-31aa report](../reports/STAGE_2_REPORT.md#s2-31aa--bounded-fld-m80fp-canonical-infinity-extension).

## Local S2-31ab: bounded `FLD m80fp [RAX]` (`DB 28`) canonical QuietNaNs

S2-31ab extends exact bare `DB 28` from the S2-31aa profile to canonical
binary80 QuietNaN encodings: biased exponent `0x7fff`, explicit integer bit
set, quiet bit set, and nonzero fraction. The source remains the raw ten-byte
payload, and the pushed destination receives `X87Tag::Special`. The existing
all-masked FCWs, `FSW & 0x80df == 0`, Empty physical destination `(TOP+7)&7`,
canonical48 ten-byte range and one-read/no-write preflight are unchanged.

The load sets no status flags, including IE; it preserves sticky PE and the
existing C1-clear, TOP/push, FCW/FOP, pointer and retirement policy. Signed
zero, Normal, subnormal and infinity behavior remains unchanged. Canonical
signaling NaNs, pseudo-NaNs, pseudo-denormals, pseudo-infinities, unnormals and
other classes remain post-read Unsupported without CPU commit at this stage. No
exception delivery or unmasked behavior is added. Native x87 admission remains
closed.

Independent payload/sign literals cover the complete TOP×FCW matrix, including
the negative indefinite QNaN. They assert raw payload, Special tag, preserved
PE, no IE, exact read count and no writes. Bare positive-QNaN and negative
indefinite `DB 28 DF E0` fixtures run through the CLI/FNSTSW path. The narrow
Unsupported schema fixture at the S2-31ab stage used positive canonical SNaN
for exact `DB 28`, budget one and no stop RIP; S2-31ac replaces it with positive
pseudo-infinity. JIT and native profiles continue to deny `DB 28`.

Intel SDM revision 093 Vol.1 Table 4-3 describes canonical quiet/signaling
NaNs; §8.2.2/Table 8-3 describes pseudo-NaNs and other unsupported encodings
([Vol.1 PDF](https://cdrdv2.intel.com/v1/dl/getContent/671436)); the
Vol.2A FLD m80fp entry remains the form reference
([Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=470)).
No physical or Mac mini acceptance is claimed. See the [Stage 2 plan](../plans/STAGE_2.md#s2-31ab-extend-bounded-fld-m80fp-rax-db-28-to-canonical-qnans),
[fixture notes](../../tests/fixtures/x86/stage2/README.md), and
[S2-31ab report](../reports/STAGE_2_REPORT.md#s2-31ab--bounded-fld-m80fp-canonical-quietnan-extension).

## Local S2-31ac: bounded `FLD m80fp [RAX]` (`DB 28`) canonical SignalingNaNs

S2-31ac extends exact bare `DB 28` to canonical binary80 SignalingNaNs only:
biased exponent `0x7fff`, explicit integer bit set, quiet bit clear, and
remaining fraction nonzero. The source's raw ten bytes are preserved exactly,
including its clear quiet bit and payload; the pushed destination receives
`X87Tag::Special`. Keep the existing all-masked FCWs, `FSW & 0x80df == 0`,
Empty physical destination `(TOP+7)&7`, canonical48 ten-byte range and
one-read/no-write preflight.

The operation adds no exception flags, especially IE, and preserves PE and
C0/C2/C3. Existing C1-clear, TOP/push, FCW/FOP, pointer and retirement policy
remains unchanged. Canonical QuietNaN, signed zero, Normal, subnormal and
infinity behavior remains as documented in prior slices. Pseudo-SNaNs,
pseudo-infinity, pseudo-QNaNs, pseudo-denormals, unnormals and other classes
remain post-read Unsupported without CPU commit. Unmasked delivery and native
x87/JIT execution are not added.

Independent minimum, middle and maximum SNaN payloads of both signs cover every
TOP×FCW pair and assert exact raw bytes, Special tag, status preservation,
one ten-byte read and no writes. Positive bare DB28 and negative
DB28/DFE0-with-FNSTSW fixtures run through the CLI. The narrow Unsupported
schema/CLI exception now uses positive pseudo-infinity and remains restricted
to exact DB28, budget one and no stop RIP; canonical SNaNs and neighboring
encodings are excluded. Intel SDM revision 093 Vol.1 Table 4-3 describes
canonical signaling and quiet NaNs; §8.2.2/Table 8-3 describes pseudo-NaNs
and other unsupported encodings. Vol.2A Table 3-47 reconstructs E=0,J=1 as Special
([Vol.1 PDF](https://cdrdv2.intel.com/v1/dl/getContent/671436)); Vol.2A FLD
m80fp remains the instruction reference
([Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=470)).
No physical or Mac mini acceptance is claimed. See the [Stage 2 plan](../plans/STAGE_2.md#s2-31ac-extend-bounded-fld-m80fp-rax-db-28-to-canonical-snans),
[fixture notes](../../tests/fixtures/x86/stage2/README.md), and
[S2-31ac report](../reports/STAGE_2_REPORT.md#s2-31ac--bounded-fld-m80fp-canonical-signalingnan-extension).

## Local S2-31ad: bounded `FLD m80fp [RAX]` (`DB 28`) pseudo-denormals

S2-31ad extends exact bare `DB 28` to binary80 pseudo-denormals: exponent zero,
explicit integer bit set, any fraction and either sign. Preserve all ten source
bytes, including exponent zero, and push `X87Tag::Special`. Do not normalize the
exponent to one. Under the all-masked FCW profile this load adds no DE or other
status flags; the value is only represented in x87 state and this slice does
not implement arithmetic consumption or unmasked exception delivery.

Retain the four accepted FCWs, `FSW & 0x80df == 0`, Empty destination, full
canonical48 ten-byte preflight, exactly one raw read, and existing C1-clear,
push/TOP, pointer and retirement behavior. Pseudo-infinity, pseudo-NaNs,
unnormals and other unsupported classes remain post-read Unsupported without
CPU commit. Native/JIT admission remains closed.

Independent raw literals cover both signs, zero/minimum/maximum fraction, and
matching exponent-one Normal controls across all TOP×FCW pairs. The tests assert
that pseudo-denormal source bytes and exponent zero are preserved, the tag is
Special, and status is unchanged except TOP/C1 policy. Positive bare and
negative DB28/DFE0 FNSTSW fixtures run through the CLI. The exact DB28,
budget-one, no-stop Unsupported schema exception continues to use positive
pseudo-infinity; pseudo-denormals do not match it.

Intel SDM revision 093 Vol.1 §8.2.2/Table 8-3 describes pseudo-denormals as
supported encodings handled as denormals; Vol.2A FLD m80fp specifies no `#D`
for the m80fp form ([Vol.1 PDF](https://cdrdv2.intel.com/v1/dl/getContent/671436),
[Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=470)).
The raw FXSAVE/FSAVE tag reconstruction assigns E=0,J=1 to Special (Vol.2A
Table 3-47, official PDF page 533). No arithmetic or unmasked behavior is
inferred. See the [Stage 2 plan](../plans/STAGE_2.md#s2-31ad-extend-bounded-fld-m80fp-rax-db-28-to-pseudo-denormals),
[fixture notes](../../tests/fixtures/x86/stage2/README.md), and
[S2-31ad report](../reports/STAGE_2_REPORT.md#s2-31ad--bounded-fld-m80fp-pseudo-denormal-extension).

## Local S2-31ae: bounded `FLD m64fp [RAX]` (`DD 00`) signed infinities

S2-31ae extends exact bare `DD 00` to signed binary64 infinities only
(exponent field `0x7ff`, fraction zero). After the existing canonical48
eight-byte preflight and exactly one Qword read, it constructs canonical
binary80 infinity by integer fields: explicit integer bit set, exponent
`0x7fff` with the source sign, zero fraction. The pushed physical slot receives
`X87Tag::Special`; no host floating-point conversion is involved.

Retain all four accepted FCWs, the DD00-local `FSW & 0x80dd == 0` gate, Empty
destination, TOP decrement, C1 clear, FIP/FDP update and one retirement.
Sticky DE and PE are preserved. Binary64 QNaN and SNaN encodings remained
post-read Unsupported at the S2-31ae stage, with no CPU commit. Its exact
schema Unsupported exception used binary64 QNaN for exact DD00, budget-one,
no-stop fixtures; S2-31ag supersedes this predicate by accepting QNaNs and
narrowing the current rejection to SNaN. FILD and m80fp retain their separate
`0x80df` status gate and m80fp subnormal loads add no DE. Unmasked exception
delivery is deferred; the SDM says FLD pushes even with an unmasked exception,
so this all-masked local profile does not claim that behavior. JIT/native
admission remains closed.

Independent positive and negative infinity literals span all TOP×FCW pairs.
A subnormal followed by infinity verifies sticky DE and PE across repeated
loads. Bare positive DD00 and negative DD00/DFE0+FNSTSW fixtures verify exact
binary80 bytes, Special tags, status and retirement; signed QNaN/SNaN neighbors
reject after one read. Intel SDM revision 093 Vol.1 Table 4-3 gives floating
point encodings; Vol.2A FLD m64fp, pp.3-352–3-353 (official PDF p.470), is the
instruction reference. The reconstructed x87 tag classification follows
Vol.2A Table 3-47, “Recreating FSAVE Format” (official PDF p.533). See the
[Vol.1 PDF](https://cdrdv2.intel.com/v1/dl/getContent/671436) and
[Vol.2A PDF, FLD](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=470),
[Vol.2A PDF, Table 3-47](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=533).
See the [Stage 2 plan](../plans/STAGE_2.md#s2-31ae-extend-bounded-fld-m64fp-rax-dd-00-to-infinities),
[fixture notes](../../tests/fixtures/x86/stage2/README.md), and
[S2-31ae report](../reports/STAGE_2_REPORT.md#s2-31ae--bounded-fld-m64fp-signed-infinity-extension).

## Local S2-31af: bounded `FLD m32fp [RAX]` (`D9 00`) signed infinities

S2-31af extends exact bare `D9 00` to signed binary32 infinities only
(exponent field 255, fraction zero). After the existing canonical48 four-byte
preflight and exactly one Dword read, construct binary80 infinity by integer
fields: significand `0x8000000000000000`, exponent `0x7fff` with the source
sign. Push it with `X87Tag::Special`; no host floating-point conversion or new
status flag is used.

Retain the four all-masked FCWs, D900-local `FSW & 0x80dd == 0` gate, Empty
destination, C1-clear, TOP, FIP/FDP, and retirement policy. Sticky DE and PE
are preserved. Existing zero and Normal behavior and S2-31y masked subnormal
normalization/DE behavior remain unchanged. Binary32 QNaN and SNaN inputs stay
post-read Unsupported after exactly one Dword read with no CPU commit. At the
S2-31af stage, the schema Unsupported exception was limited to canonical
QNaN; S2-31ah supersedes it by accepting QNaNs and requiring canonical SNaN
for the exact D900/budget-one/no-stop exception. Infinity, QNaN, zero, Normal
and subnormal sources do not match that current predicate. FILD and m64/m80
status gates remain unchanged. Unmasked exception delivery is deferred; FLD
can push despite an unmasked exception, so no generic pre-read suppression is
inferred. JIT/native admission remains closed.

Independent positive and negative infinity literals span all TOP×FCW pairs.
A minimum-subnormal load followed by infinity checks sticky DE and PE. The
bare positive D900 and negative D900/DFE0+FNSTSW fixtures verify exact raw
binary80 bytes, Special tags, status and retirement. Signed QNaN/SNaN neighbors
reject after one read without CPU commit. Intel SDM revision 093 Vol.1 §8.5.2
describes masked denormal-operand handling; Vol.1 Table 4-3 gives floating
point encodings. Vol.2A FLD m32fp, pp.3-352–3-353 (official PDF p.470), is the
instruction reference; reconstructed tag classification follows Vol.2A Table
3-47, “Recreating FSAVE Format” (official PDF p.533). See the
[Vol.1 PDF](https://cdrdv2.intel.com/v1/dl/getContent/671436),
[Vol.2A PDF, FLD](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=470),
[Vol.2A PDF, Table 3-47](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=533),
[Stage 2 plan](../plans/STAGE_2.md#s2-31af-extend-bounded-fld-m32fp-rax-d9-00-to-infinities),
[fixture notes](../../tests/fixtures/x86/stage2/README.md), and
[S2-31af report](../reports/STAGE_2_REPORT.md#s2-31af--bounded-fld-m32fp-signed-infinity-extension).

## Local S2-31ag: bounded `FLD m64fp [RAX]` (`DD 00`) canonical QNaNs

S2-31ag extends exact bare `DD 00` to canonical binary64 quiet NaNs: exponent
field `0x7ff`, fraction bit 51 set, and arbitrary lower payload bits, for
either sign. After the existing canonical48 eight-byte preflight and exactly
one Qword read, construct binary80 using integer fields: significand
`0x8000000000000000 | (fraction << 11)`, exponent `0x7fff`, and the source
sign. Push with `X87Tag::Special`, preserving payload bits in the mapped
fraction field. No host floating-point conversion is used.

Retain the four FCWs, DD00-local `FSW & 0x80dd == 0` gate, Empty destination,
C1-clear/TOP, FIP/FDP and one-retirement behavior. Add no status flags,
including IE; sticky DE and PE remain preserved. Existing signed zero, Normal,
subnormal and infinity paths remain unchanged. Binary64 SNaNs remain post-read
Unsupported after one Qword read without CPU commit. The exact DD00, budget-one,
no-stop schema exception now requires a canonical SNaN; QNaNs, infinities,
zero, Normal and subnormal inputs do not match. Keep m32 NaNs, unmasked
exception delivery and JIT/native execution outside this supported path; the
JIT and native profiles continue to deny DD00 loads. m80fp subnormal behavior
remains distinct and adds no DE.

Independent signed quiet-only, low-payload, middle-payload and maximum-payload
vectors cover every TOP×FCW pair. A minimum-subnormal load followed by a
payload-bearing QNaN checks sticky DE/PE and unchanged status policy. The
positive bare DD00 and negative payload-bearing DD00/DFE0+FNSTSW fixtures
verify exact bytes, Special tag and retirement through CLI. SNaN rejection
tests assert one Qword read and no CPU commit. Intel SDM revision 093 Vol.1
Table 4-3 describes binary64 quiet-NaN encoding; Vol.2A FLD m64fp,
pp.3-352–3-353 (official PDF p.470), specifies the load form. Reconstructed
tag classification follows Vol.2A Table 3-47, “Recreating FSAVE Format”
(official PDF p.533). This bounded all-masked profile makes no claim about
unmasked delivery. See the [Vol.1 PDF](https://cdrdv2.intel.com/v1/dl/getContent/671436),
[Vol.2A PDF, FLD](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=470),
[Vol.2A PDF, Table 3-47](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=533),
[Stage 2 plan](../plans/STAGE_2.md#s2-31ag-extend-bounded-fld-m64fp-rax-dd-00-to-canonical-qnans),
[fixture notes](../../tests/fixtures/x86/stage2/README.md), and
[S2-31ag report](../reports/STAGE_2_REPORT.md#s2-31ag--bounded-fld-m64fp-canonical-qnan-extension).

## Local S2-31ah: bounded `FLD m32fp [RAX]` (`D9 00`) canonical QNaNs

S2-31ah extends exact bare `D9 00` to canonical binary32 quiet NaNs:
exponent field `0xff`, fraction bit 22 set, and arbitrary lower payload bits,
for either sign. After the existing canonical48 four-byte preflight and
exactly one Dword read, construct binary80 using integer fields: significand
`0x8000000000000000 | (fraction << 40)`, exponent `0x7fff`, and the source
sign. Push with `X87Tag::Special`, preserving mapped payload bits. No host
floating-point conversion is used.

Retain the four all-masked FCWs, D900-local `FSW & 0x80dd == 0` gate, Empty
destination, C1-clear/TOP, FIP/FDP, and one-retirement policy. Add no exception
flags, including IE; sticky DE and PE are preserved. Existing zero, Normal,
masked-subnormal, and signed-infinity paths remain unchanged. At the S2-31ah
stage binary32 SNaNs were post-read Unsupported; S2-31aj now accepts them as
described below. The exact current D900/budget-one/no-stop Unsupported schema
exception represents an invalid environment gate, independent of operand
class. FILD and m64/m80 gates remain unchanged. Unmasked exception delivery
and JIT/native execution remain outside this supported path; native/JIT
profiles continue to deny D900 loads.

Independent signed quiet-only, low-payload, middle-payload and maximum-payload
vectors cover every TOP×FCW pair. A minimum-subnormal load followed by a
negative payload-bearing QNaN verifies sticky DE/PE and unchanged status
policy. Positive bare D900 and negative payload-bearing D900/DFE0+FNSTSW
fixtures verify exact bytes, Special tag and retirement through CLI. SNaN
rejection tests assert one Dword read and no CPU commit. Intel SDM revision
093 Vol.1 Table 4-3 describes binary32 quiet-NaN encoding; Vol.2A FLD m32fp,
pp.3-352–3-353 (official PDF p.470), specifies the load form. Reconstructed
tag classification follows Vol.2A Table 3-47, “Recreating FSAVE Format”
(official PDF p.533). This bounded all-masked profile makes no claim about
unmasked delivery. See the [Vol.1 PDF](https://cdrdv2.intel.com/v1/dl/getContent/671436),
[Vol.2A PDF, FLD](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=470),
[Vol.2A PDF, Table 3-47](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=533),
[Stage 2 plan](../plans/STAGE_2.md#s2-31ah-extend-bounded-fld-m32fp-rax-d9-00-to-canonical-qnans),
[fixture notes](../../tests/fixtures/x86/stage2/README.md), and
[S2-31ah report](../reports/STAGE_2_REPORT.md#s2-31ah--bounded-fld-m32fp-canonical-qnan-extension).

## Local S2-31aj: bounded `FLD m32fp [RAX]` (`D9 00`) canonical SNaNs

S2-31aj extends exact bare `D9 00` to canonical binary32 signaling NaNs:
exponent field `0xff`, nonzero fraction, and quiet bit 22 clear, for either
sign. After canonical48 four-byte preflight and one Dword read, construct
binary80 with integer fields only: significand
`0xc000000000000000 | (u64::from(fraction) << 40)`, exponent `0x7fff`, and
preserved sign. Push with `X87Tag::Special`; the binary80 quiet bit is set and
the mapped source payload remains unchanged.

Retain the four all-masked FCWs, D900-local `FSW & 0x80dd == 0` gate, Empty
destination, C1-clear/TOP, FIP/FDP, canonical48 range, one-read and
one-retirement policy. Add sticky IE only; preserve DE/PE and do not add ES,
SF, B, or other status flags. Existing signed zero, Normal, masked-subnormal,
infinity, and QNaN paths are unchanged. Existing IE and unmasked FCWs remain
Unsupported before the operand read. A later D900 load is similarly rejected
until FNCLEX clears status. Faults and failed reads leave CPU state
uncommitted. Binary64 SNaNs use the separate S2-31ai behavior; m80 raw SNaNs
remain byte-preserving with no new flags. Unmasked exception delivery,
native/JIT execution and physical behavior remain outside this slice.

Independent signed minimum, middle and maximum payload literals cover every
TOP×FCW pair, with exact binary80 quieted payloads and sticky DE/PE checks.
The clean TOP=0 result has FSW `0x3801`. Tests cover SNaN→quieted-QNaN,
pre-existing IE and unmasked-FCW pre-read gates, one-Dword-read failures,
later-load rejection, FNCLEX clearing exception flags while preserving
TOP/tags/payloads, and recovery through a QNaN load. Positive bare and
negative payload-bearing D900/DFE0+FNSTSW fixtures run through the CLI. The
exact Unsupported schema exception is now environment-driven: exact D900,
one instruction, no stop RIP, and an invalid FCW or a set bit in `0x80dd`;
the operand class does not affect matching. Native/JIT profiles continue to
deny the load.

Intel SDM revision 093 Vol.1 §4.8.3.5/Table 4-8 describes SNaN quieting by
setting the most-significant fraction bit; §8.1.3.3 describes x87 invalid
operation status. Vol.2A FLD m32fp identifies SNaN as #IA; reconstructed tag
classification follows Table 3-47, “Recreating FSAVE Format” (PDF p.533). The
local contract covers the all-masked profile only and makes no claim about
unmasked delivery. See [Vol.1 §4.8.3.5/Table 4-8](https://cdrdv2-public.intel.com/929351/253665-093-sdm-vol-1.pdf#page=105),
[Vol.1 §8.1.3.3](https://cdrdv2-public.intel.com/929351/253665-093-sdm-vol-1.pdf#page=211),
[Vol.2A FLD](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=470),
[Vol.2A Table 3-47](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=533),
[Stage 2 plan](../plans/STAGE_2.md#s2-31aj-extend-bounded-fld-m32fp-rax-d9-00-to-canonical-snans),
[fixture notes](../../tests/fixtures/x86/stage2/README.md), and
[S2-31aj report](../reports/STAGE_2_REPORT.md#s2-31aj--bounded-fld-m32fp-canonical-snan-extension).

## Local S2-31ai: bounded `FLD m64fp [RAX]` (`DD 00`) canonical SNaNs

S2-31ai extends exact bare `DD 00` to canonical binary64 signaling NaNs:
exponent `0x7ff`, nonzero fraction, and quiet bit 51 clear, for either sign.
After canonical48 eight-byte preflight and one Qword read, convert only integer
fields to canonical binary80: significand
`0xc000000000000000 | (fraction << 11)`, exponent `0x7fff`, and preserved
sign. Store the resulting ten bytes and assign `X87Tag::Special`.

The existing four FCWs, `FSW & 0x80dd == 0` entry gate, Empty push destination,
C1-clear/TOP/pointer/retirement policy remain fixed. Set sticky IE only;
preserve DE/PE and do not add ES, SF, B, or other exception/status flags.
Pre-existing IE is rejected before the operand read. A successfully loaded
SNaN leaves IE set, so another DD00 load is rejected at preflight until FNCLEX
clears the status word; FNSTSW can observe IE. Faults and failed reads leave
CPU state uncommitted. m32 SNaNs and raw m80 SNaNs retain their earlier
operation-specific policies. Unmasked exception delivery, native/JIT
execution, and physical behavior remain outside this slice.

Independent signed low, middle and maximum-payload SNaN literals cover every
TOP×FCW pair. Exact binary80 payload/tag and `FSW=0x3801` at clean TOP=0 are
checked; sticky DE/PE sequences, one-read faults, pre-read IM=0/pre-existing
IE rejection, subsequent-load rejection, FNCLEX recovery, and bare plus
DD00/DFE0+FNSTSW CLI fixtures are covered. The exact Unsupported fixture is
now environment-driven: exact DD00, one instruction, no stop RIP, and an
invalid FCW or a status bit in `0x80dd`; its operand class is immaterial.
Native and JIT profiles continue to reject this operation.

## Local S2-31ak: sticky exception flags for bounded m32/m64 FLD

S2-31ak supersedes the D900/DD00 `FSW & 0x80dd == 0` gates recorded by the
earlier bounded load slices. Only these exact `D9 00` and `DD 00` paths now
require `FSW & 0x80dc == 0`, allowing pre-existing IE, DE and PE in any
combination. ZE, OE, UE, SF, ES and B remain forbidden before the memory read.
The four all-masked FCWs, Empty destination, canonical48 range preflight,
one Dword/Qword raw read, C1/TOP/pointer and retirement behavior are unchanged.

Accepted loads preserve sticky IE/DE/PE. Canonical binary32/binary64 SNaN
loads add IE, and nonzero subnormal loads add DE; no status flag is cleared
except the existing load-local C1 policy. The gate change is limited to D900
and DD00. FILD, m80 loads, stores, arithmetic and other x87 operations retain
their existing predicates. Native/JIT admission and unmasked exception
delivery remain deferred.

Verification covers all eight IE/DE/PE combinations across source classes,
FCWs and TOP values; same-width and mixed-width SNaN/ordinary/QNaN/subnormal
sequences; repeated SNaNs; second-read fault atomicity; each forbidden status
bit before read; and unchanged rejection for unrelated x87 gates. The exact
D900/DD00 budget-one/no-stop schema cases now require a forbidden status bit
instead of relying on a rejected operand class. Existing CLI/FNSTSW SNaN
fixtures observe sticky IE. See the [S2-31ak plan](../plans/STAGE_2.md#s2-31ak-preserve-sticky-iedepe-across-exact-m32m64-fld-loads), [report](../reports/STAGE_2_REPORT.md#s2-31ak--sticky-iedepe-across-bounded-m32m64-fld), and [fixture notes](../../tests/fixtures/x86/stage2/README.md).

Intel SDM revision 093 Vol.1 §4.8.3.5 and Table 4-8 describe SNaN quieting by
setting the most-significant fraction bit and the masked-invalid response;
§8.1.3.3 describes x87 invalid-operation status. Vol.2A FLD m64fp describes
the load form and its invalid-operation source. Reconstructed tag
classification follows Vol.2A Table 3-47, “Recreating FSAVE Format.” The local
contract covers only the all-masked profile and makes no claim about unmasked
delivery. See [Vol.1 §4.8.3.5/Table 4-8](https://cdrdv2-public.intel.com/929351/253665-093-sdm-vol-1.pdf#page=105),
[Vol.1 §8.1.3.3](https://cdrdv2-public.intel.com/929351/253665-093-sdm-vol-1.pdf#page=211),
[Vol.2A PDF, FLD](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=470),
[Vol.2A PDF, Table 3-47](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=533),
[Stage 2 plan](../plans/STAGE_2.md#s2-31ai-extend-bounded-fld-m64fp-rax-dd-00-to-canonical-snans),
[fixture notes](../../tests/fixtures/x86/stage2/README.md), and
[S2-31ai report](../reports/STAGE_2_REPORT.md#s2-31ai--bounded-fld-m64fp-canonical-snan-extension).

## Local S2-31al: signed-zero `FST m32fp` and `FSTP m32fp`

Extend only exact bare `D9 10` and `D9 18` in Long64. The old physical TOP
source is accepted as zero only when its tag is Zero and the raw binary80 bytes
classify as canonical Zero (`exponent=0`, significand=0). Preserve the sign bit
when storing binary32 `+0` (`00000000`) or `-0` (`00000080`). The existing
Normal conversion path is unchanged; a Zero tag with a nonzero/pseudo payload,
or a non-Zero tag with a zero payload, remains Unsupported before data access.

Retain the existing four FCWs and current S2-31az `FSW & 0x80dc == 0` gate, which permits
sticky IE/DE/PE and C1. An exact zero store adds no status flags, preserves IE/DE/PE, and
clears C1 under the local exact-result policy. Check the full canonical48
Dword range before exactly one atomic Dword write; do not read data memory.
Only after a successful write may D918 empty old physical TOP and advance TOP.
D910 preserves TOP, all tags and all raw payloads. Store faults and invalid
ranges leave CPU and memory unchanged.

Tests cover both signs for every TOP and FCW with incoming PE/C1 combinations,
exact bytes, one-write/no-read behavior, and FLD-zero→FST/FSTP composition.
Tag/payload mismatches, Empty source, pseudo-denormal payload, denied and
architectural write failures, and canonical-range failure reject before
commit. Literal CLI fixtures observe signed bytes and FNSTSW state. Native and
JIT admission remain closed. See the [S2-31al plan](../plans/STAGE_2.md#s2-31al-store-signed-binary80-zero-through-bounded-m32-fstfstp), [report](../reports/STAGE_2_REPORT.md#s2-31al--signed-zero-stores-for-bounded-m32-fstfstp), and [fixture notes](../../tests/fixtures/x86/stage2/README.md).

## Local S2-31am: signed-zero `FST m64fp` and `FSTP m64fp`

Extend only exact bare `DD 10` and `DD 18` in Long64. The old physical TOP
source is accepted as zero only when its tag is Zero and its ten raw binary80
bytes classify as canonical Zero (`exponent=0`, significand=0). Preserve the
sign bit in binary64 `+0` (`0000000000000000`) or `-0`
(`0000000000000080`). Existing Normal conversion remains unchanged; a Zero
tag with a nonzero/pseudo payload, or a non-Zero tag with zero payload,
remains Unsupported before data access.

Retain the existing four FCWs and current S2-31az `FSW & 0x80dc == 0` gate. Sticky IE/DE/PE and C1
may enter; exact zero preserves IE/DE/PE, adds no flags, and clears C1. Check the
complete canonical48 Qword range before exactly one atomic Qword write; do not
read data memory. Only after a successful write may DD18 empty old physical
TOP and advance TOP. DD10 preserves TOP, all tags and all raw payloads. Store
faults and invalid ranges leave CPU state uncommitted.

Tests cover both signs across TOP×FCW×incoming PE/C1, literal output bytes,
m32↔m64 FLD-zero compositions, bad tag/payload pairs, every forbidden FSW
gate bit, denied and architectural write failures, canonical upper-boundary
behavior, FST preservation and FSTP pop. Positive bare and negative
DFE0/FNSTSW fixtures are exercised by CLI/schema and native-denial checks; JIT
decode denial remains explicit. See the [S2-31am plan](../plans/STAGE_2.md#s2-31am-store-signed-binary80-zero-through-bounded-m64-fstfstp), [report](../reports/STAGE_2_REPORT.md#s2-31am--signed-zero-stores-for-bounded-m64-fstfstp), and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


## Local S2-31an: signed-zero `FSTP m80fp [RAX]` (`DB 38`)

Exact `DB 38` accepts either the already-supported Valid-tagged canonical
Normal binary80 source or a Zero-tagged canonical binary80 zero whose exponent
and all 64 significand bits are zero. It writes the original ten bytes
unchanged, so positive zero is `00000000000000000000` and negative zero is
`00000000000000000080`. Mismatched tag/payload pairs remain Unsupported before
operand access.

The existing four FCWs remain; S2-31ba widens the current DB38 gate to
`FSW & 0x80dc == 0`. Zero creates no new status flags, preserves IE/DE/PE and
C0/C2/C3, and clears C1. After complete
canonical48 ten-byte preflight, exactly one atomic write occurs and no data read
is performed. Only successful writes pop/empty physical TOP and advance TOP;
all raw payloads and existing pointer/retirement behavior are preserved.
Native/JIT admission remains closed. See the [S2-31an plan](../plans/STAGE_2.md#s2-31an-store-canonical-signed-binary80-zero-through-bounded-fstp-m80fp), [report](../reports/STAGE_2_REPORT.md#s2-31an--signed-zero-binary80-stores-through-bounded-fstp-m80fp), and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


## Local S2-31ao: signed-infinity `FSTP m80fp [RAX]` (`DB 38`)

Exact `DB 38` additionally accepts a canonical binary80 infinity when physical
TOP has tag Special, exponent `0x7fff`, explicit integer bit set, and fraction
zero. Preserve all ten raw bytes unchanged: `+∞` is
`0000000000000080ff7f`, and `-∞` is `0000000000000080ffff`. Existing
Valid+canonical-Normal and Zero+canonical-Zero pairs remain accepted.
Pseudo-infinity, NaNs, finite special classes, unnormal encodings, and infinity
with Valid/Zero/Empty tags remain Unsupported before data access.

Retain the four FCWs and the S2-31ba `FSW & 0x80dc == 0` store gate. Infinity
adds no flags, preserves IE/DE/PE and C0/C2/C3, and clears C1 under the
existing local policy.
Validate the complete canonical48 ten-byte range and perform exactly one atomic
write/no read; pop and advance TOP only after success. Preserve all payloads,
pointers, and retirement behavior. JIT/native admission remains denied. The
canonical infinity field pattern follows Intel SDM Vol.1 §8.2.2/Table 8-3; raw
FSTP m80fp store semantics are described in Vol.2A pp.3-386–3-387 ([official
revision 093 PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=503)). See the [S2-31ao plan](../plans/STAGE_2.md#s2-31ao-store-canonical-signed-binary80-infinity-through-bounded-fstp-m80fp), [report](../reports/STAGE_2_REPORT.md#s2-31ao--signed-infinity-binary80-store-through-bounded-fstp-m80fp), and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


## Local S2-31ap: signed-infinity `FST/FSTP m64fp [RAX]`

Exact `DD 10` and `DD 18` accept a source infinity only when physical TOP has
`Special` tag and canonical binary80 infinity fields (exponent `0x7fff`,
explicit integer bit one, zero fraction). Convert integer fields to signed
binary64 infinity: `+∞` bytes `000000000000f07f`, `-∞` bytes
`000000000000f0ff`. The conversion is exact and sets neither PE nor any other
new status flag. Preserve a preexisting PE; clear C1 under the local exact-store
policy, with C0/C2/C3 unchanged.

Retain the four admitted FCWs and current `FSW & 0x80dc == 0` gate. The complete
canonical48 Qword range is checked before exactly one atomic write; no data read
is performed. DD10 preserves TOP, tags and all raw payloads. DD18 empties old
physical TOP and advances TOP only after successful write. Existing Normal
rounding limits and signed-zero behavior remain unchanged. Reject pseudo-infinity,
NaNs, finite specials and mismatched source tags before memory access. JIT/native
admission stays closed. The canonical binary80 Infinity encoding is specified
in Intel SDM revision 093 Vol.1 §8.2.2/Table 8-3; FST/FSTP m64fp behavior is
listed in Vol.2A pp.3-386–3-387 ([official PDF, pp.3-386–3-387](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=504)).
See the [S2-31ap plan](../plans/STAGE_2.md#s2-31ap-store-canonical-signed-binary80-infinity-through-bounded-m64-fstfstp),
[report](../reports/STAGE_2_REPORT.md#s2-31ap--signed-infinity-stores-for-bounded-m64-fstfstp),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


## Local S2-31aq: signed-infinity `FST/FSTP m32fp [RAX]`

Exact `D9 10` and `D9 18` accept canonical binary80 infinity only when
physical TOP has the Special tag, exponent `0x7fff`, explicit integer bit set,
and zero fraction. Integer-field conversion emits binary32 `+∞` bytes
`0000807f` or `-∞` bytes `000080ff`; the exact-result indicators are false.
This path adds no status flags and preserves PE, with C1/C0/C2/C3 following the
existing exact-store policy.

Retain the four admitted FCWs and current `FSW & 0x80dc == 0` gate. Check the complete
canonical48 Dword range before exactly one atomic write; perform no data read.
D910 preserves TOP, tags, and payloads. D918 pops only after successful write.
Normal and signed-zero behavior remains unchanged. Pseudo-infinity, NaNs,
other Special classes, and mismatched tags reject before access. JIT/native
admission stays closed. The canonical infinity encoding follows Intel SDM
revision 093 Vol.1 §8.2.2/Table 8-3; FST/FSTP m32fp semantics are in Vol.2A
pp.3-386–3-387 ([official PDF, pp.3-386–3-387](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=504)).
See the [S2-31aq plan](../plans/STAGE_2.md#s2-31aq-store-canonical-signed-binary80-infinity-through-bounded-m32-fstfstp),
[report](../reports/STAGE_2_REPORT.md#s2-31aq--signed-infinity-stores-for-bounded-m32-fstfstp),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


<a id="local-s2-31au-canonical-quietnan-fstfstp-m64fp-rax-dd-10-dd-18"></a>

## Local S2-31au: canonical QuietNaN `FST/FSTP m64fp [RAX]`

Exact `DD 10` and `DD 18` accept a canonical binary80 QuietNaN only when
physical TOP has the Special tag, exponent `0x7fff`, explicit integer bit one,
and quiet fraction bit one. Convert with integer fields: preserve sign and the
upper 52 fraction bits, truncating the lower 11 bits. The conversion is exact
under this bounded policy: it does not round, set PE or IE, or use host floating
point. Preserve sticky PE and clear C1; C0/C2/C3 follow the existing local
store policy.

Retain the four FCWs and current `FSW & 0x80dc == 0` gate. Preflight the full
canonical48 Qword range, then make exactly one atomic write and no data read.
DD10 preserves TOP/tags/payloads; DD18 pops only after successful write. Keep
Normal, Zero and Infinity behavior. SNaN, pseudo-NaN, other special encodings,
and mismatched tags reject before access. JIT/native admission remains closed;
general x87 exception delivery and physical acceptance remain outside this
slice. See the [plan](../plans/STAGE_2.md#s2-31au-store-canonical-binary80-quietnan-through-bounded-fstfstp-m64fp),
[report](../reports/STAGE_2_REPORT.md#s2-31au--canonical-quietnan-binary80-to-binary64-stores),
and [fixtures](../../tests/fixtures/x86/stage2/README.md).


<a id="local-s2-31ay-admit-sticky-ie-fstfstp-m32-m64"></a>

## Local S2-31ay: masked sticky IE in m32/m64 `FST/FSTP [RAX]`

Only the status gate in the exact D910/D918 m32 and DD10/DD18 m64 conversion
helpers changes, from `FSW & 0x80df == 0` to `FSW & 0x80de == 0`. This admits
sticky IE while still rejecting ZE, DE, OE, UE, SF, ES, and B. The four admitted
FCWs, source class/tag rules, full canonical48 range checks, memory access
shape, and all other x87 status gates remain unchanged.

Successful stores preserve incoming IE and PE. A successful SNaN store ORs IE;
an inexact finite store ORs PE; both status changes happen only after the
atomic write succeeds. ES/B remain clear, C1 follows the existing conversion
policy, and FSTP pops only after success. Repeated stores can proceed with IE
set; `FNSTSW` observes it and `FNCLEX` can clear it, but is no longer required
between otherwise admitted m32/m64 stores. m80, integer, load, and arithmetic
gates are unchanged. Native/JIT admission remains closed and physical
acceptance stays under ISSUE-019. See the [plan](../plans/STAGE_2.md#s2-31ay-admit-sticky-ie-in-bounded-m32-m64-fst-fstp),
[report](../reports/STAGE_2_REPORT.md#s2-31ay--sticky-ie-m32-m64-stores), and
[fixtures](../../tests/fixtures/x86/stage2/README.md).


<a id="local-s2-31aw-canonical-signalingnan-fstfstp-m64fp-rax-dd-10-dd-18"></a>

## Local S2-31aw: masked canonical SignalingNaN `FST/FSTP m64fp [RAX]`

Exact `DD 10` and `DD 18` accept a canonical binary80 SignalingNaN only when
physical TOP is Special-tagged, exponent is `0x7fff`, the explicit integer bit
is one, the quiet bit is clear, and the fraction is nonzero. Convert by integer
fields: preserve sign and the upper 51 payload bits, set binary64's quiet bit,
and truncate lower 11 bits without rounding or PE. Add sticky IE after the
Qword write succeeds; retain PE and clear C1. Under the admitted profile ES,
B, and SF stay clear. The ten-byte source register payload is never rewritten.

Keep the four FCWs and current `FSW & 0x80dc == 0` gate, canonical48 full-Qword
preflight, one atomic write/no data read, and existing pointer/retirement
policy. DD10 preserves the stack; DD18 empties old TOP and advances TOP only
after a successful store. `FNSTSW` observes IE; a subsequent m32/m64 store
remains admitted while IE stays set, and `FNCLEX` is available to clear it.
QNaNs continue through the
S2-31au no-new-flags path; pseudo-SNaNs, malformed payloads, mismatched tags,
and forbidden environment bits reject before access. JIT/native admission and
general unmasked exception delivery remain outside scope. This narrow masked
behavior follows Intel SDM revision 093 Vol. 2A FST/FSTP and Vol. 1 §8.5.1
([Vol. 2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=505),
[Vol. 1 PDF](https://cdrdv2.intel.com/v1/dl/getContent/671436#page=232)).
See the [plan](../plans/STAGE_2.md#s2-31aw-store-canonical-binary80-signalingnan-through-bounded-fstfstp-m64fp),
[report](../reports/STAGE_2_REPORT.md#s2-31aw--canonical-signalingnan-binary80-to-binary64-stores),
and [fixtures](../../tests/fixtures/x86/stage2/README.md).
<a id="local-s2-31av-canonical-quietnan-fstfstp-m32fp-rax-d9-10-d9-18"></a>

## Local S2-31av: canonical QuietNaN `FST/FSTP m32fp [RAX]`

Exact `D9 10` and `D9 18` accept a canonical binary80 QuietNaN only when
physical TOP has Special tag, exponent `0x7fff`, explicit integer bit one, and
quiet fraction bit one. Convert with integer fields: preserve sign and fraction
bits 62..40 as the binary32 23-bit fraction, truncating lower 40 bits without
rounding. This bounded conversion adds no flags, including PE, and does not use
host floating point. Preserve sticky PE and clear C1 under the existing exact
store policy.

Retain the four FCWs and current `FSW & 0x80dc == 0` gate. Preflight the full
canonical48 Dword range and perform exactly one atomic write/no data read. D910
preserves TOP/tags/payloads; D918 pops only after a successful write. Keep
Normal, Zero and Infinity behavior. SNaNs, pseudo-NaNs, other special classes,
and mismatched tags reject before access. JIT/native admission remains closed;
general exception delivery and physical acceptance remain outside this slice.
See the [plan](../plans/STAGE_2.md#s2-31av-store-canonical-binary80-quietnan-through-bounded-fstfstp-m32fp),
[report](../reports/STAGE_2_REPORT.md#s2-31av--canonical-quietnan-binary80-to-binary32-stores),
and [fixtures](../../tests/fixtures/x86/stage2/README.md).


<a id="local-s2-31ax-canonical-signalingnan-fstfstp-m32fp-rax-d9-10-d9-18"></a>

## Local S2-31ax: masked canonical SignalingNaN `FST/FSTP m32fp [RAX]`

Exact `D9 10` and `D9 18` accept a canonical binary80 SignalingNaN only when
physical TOP is Special-tagged, exponent is `0x7fff`, the explicit integer bit
is one, the quiet bit is clear, and the fraction is nonzero. Convert by
integer fields: preserve sign, set binary32's quiet bit, copy fraction bits
61..40 as its remaining 22 payload bits, and truncate low 40 bits without
rounding or PE. Set sticky IE only after the full-range Dword write succeeds;
preserve PE and the source register bytes. The admitted masked profile leaves
ES/B/SF clear and clears C1 under the existing exact-store policy.

Keep four FCWs and current `FSW & 0x80dc == 0`. Preflight the full canonical48 Dword
range and perform exactly one atomic write/no data read. D910 preserves TOP,
tags and payloads; D918 empties old TOP and advances only after a successful
write. QNaNs retain their S2-31av no-new-flags behavior. `FNSTSW` observes IE;
a subsequent m32/m64 store remains admitted with IE set, and `FNCLEX` is
available to clear it.
Pseudo-SNaNs, other special classes, and mismatched tags reject before access.
JIT/native admission, unmasked exception delivery, native runtime execution,
and physical acceptance remain outside scope. This bounded masked behavior
follows Intel SDM revision 093 Vol. 2A FST/FSTP and Vol. 1 §8.5.1
([Vol. 2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=505),
[Vol. 1 PDF](https://cdrdv2.intel.com/v1/dl/getContent/671436#page=232)). See
the [plan](../plans/STAGE_2.md#s2-31ax-store-canonical-binary80-signalingnan-through-bounded-fstfstp-m32fp),
[report](../reports/STAGE_2_REPORT.md#s2-31ax--canonical-signalingnan-binary80-to-binary32-stores),
and [fixtures](../../tests/fixtures/x86/stage2/README.md).


<a id="local-s2-31az-admit-sticky-de-m32-m64-fstfstp"></a>

## Local S2-31az: admit sticky DE in bounded m32/m64 `FST/FSTP [RAX]`

Only the exact D910/D918 m32 and DD10/DD18 m64 conversion helpers widen their
FSW gate from `FSW & 0x80de == 0` to `FSW & 0x80dc == 0`. This admits masked
sticky DE alongside IE and PE while continuing to reject ZE, OE, UE, SF, ES,
and B. The FCW whitelist, source classes/tags, canonical finite-result range,
addressing and all other x87 gates remain unchanged.

Successful stores preserve incoming IE/DE/PE. SNaN conversion ORs IE and an
inexact finite conversion ORs PE only after the full-range atomic write
succeeds; stores do not generate DE. ES/B remain clear. FSTP changes TOP/tag
only after the write succeeds. A minimum or largest binary32 subnormal loaded
by D900 can therefore be stored as an exact binary64 normal by DD10/DD18 while
retaining DE. Binary32/64 subnormal store results remain Unsupported.
`FNSTSW` observes the retained status and `FNCLEX` clears it. JIT/native
admission and physical acceptance remain deferred under ISSUE-019. See the
[plan](../plans/STAGE_2.md#s2-31az-admit-sticky-de-in-bounded-m32-m64-fst-fstp),
[report](../reports/STAGE_2_REPORT.md#s2-31az--sticky-de-in-bounded-m32-m64-stores),
and [fixtures](../../tests/fixtures/x86/stage2/README.md).


<a id="local-s2-31ba-sticky-ie-de-raw-fstp-m80fp-db-38"></a>

## Local S2-31ba: sticky IE/DE on raw `FSTP m80fp [RAX]` (`DB 38`)

Only `bounded_x87_m80fp_source` widens its FSW gate from
`FSW & 0x80df == 0` to `FSW & 0x80dc == 0`. This admits masked sticky IE and DE
alongside PE while continuing to reject ZE, OE, UE, SF, ES and B. Keep the four
FCWs and current source/tag pairs unchanged: Valid+canonical Normal,
Zero+canonical Zero, and Special+canonical Infinity, QNaN, SNaN or Subnormal.
Pseudo-denormals, unnormals and mismatched tags remain Unsupported before
memory access.

The store copies all ten source bytes exactly; it neither normalizes or quiets
the encoding nor generates status flags. Preserve incoming IE/DE/PE and C0/C2/C3,
clear C1, and update TOP/tag only after the complete canonical48 ten-byte range
has passed preflight and the one atomic write succeeds. Write failure leaves the
full CPU state and destination bytes unchanged.

Cross-width masked sequences preserve the load result and its flags: binary32
or binary64 subnormal FLD followed by DB38 stores a canonical binary80 Normal
while retaining DE without UE/PE; binary32/64 SNaN FLD followed by DB38 stores
the quieted binary80 QNaN with IE; raw binary80 SNaN FLD followed by narrow FST
and DB38 stores the original signaling encoding with IE. `FNSTSW` observes the
sticky flags; `FNCLEX` clears them. General x87 exception delivery, JIT/native
admission, native runtime execution and physical acceptance remain outside this
bounded slice under ISSUE-019. DB /7 FSTP m80fp is specified in Intel SDM
revision 093 Vol.2A pp.3-386–3-387 ([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=503)).
See the [plan](../plans/STAGE_2.md#s2-31ba-admit-sticky-ie-de-to-raw-binary80-fstp),
[report](../reports/STAGE_2_REPORT.md#s2-31ba--sticky-ie-de-on-raw-binary80-fstp-m80fp),
and [fixtures](../../tests/fixtures/x86/stage2/README.md).


<a id="local-s2-31bb-finite-exponent-127-bounded-m32-stores"></a>

## Local S2-31bb: finite exponent 127 through bounded m32 `FST/FSTP [RAX]`

For exact `D9 10` and `D9 18`, extend only the finite canonical-Normal
binary80 source exponent interval in `bounded_x87_m32fp_result` from
`-126..=126` to `-126..=127`. Keep the existing integer significand rounding
and all four FCWs. Normalize a rounded significand carry before checking the
final exponent: a carry from 126 to 127 is a valid binary32 normal result, but
a carry from 127 to 128 returns Unsupported before destination access. A
source already at exponent 128 also remains Unsupported. This path does not
deliver masked overflow or set OE.

Keep the minimum exponent, subnormal-result rejection, `FSW & 0x80dc == 0`
gate, FCW whitelist, source tags/classes, addressing, and post-write status,
pointer and pop behavior unchanged. A rejected input leaves the full CPU state
and destination memory unchanged. Successful stores retain the established
sticky IE/DE/PE policy; no overflow flags are synthesized. D910 preserves TOP;
D918 pops only after its atomic Dword write succeeds.

Separate literal coverage includes a D900 FLD of positive maximum finite
binary32, a D910 positive maximum-finite store, and a D918 negative
maximum-finite store with DFE0 observing status after pop. ISA vectors cover
both opcodes, all TOPs and FCWs, signs, all incoming IE/DE/PE combinations,
exact maximum finite, ties and directed rounding at exponents 126/127, carry to
valid exponent 127, carry to rejected exponent 128, and an exponent-128 source.
Overflow rejects before memory access; denied writes and canonical-range
failures preserve state and bytes. JIT and native admission remain closed.
Native runtime and physical acceptance remain deferred under ISSUE-019. See
the [plan](../plans/STAGE_2.md#s2-31bb-finite-exponent-127-bounded-m32-stores),
[report](../reports/STAGE_2_REPORT.md#s2-31bb--finite-exponent-127-bounded-m32-stores),
and [fixtures](../../tests/fixtures/x86/stage2/README.md).


<a id="local-s2-31bc-finite-exponent-1023-bounded-m64-stores"></a>

## Local S2-31bc: finite exponent 1023 through bounded m64 `FST/FSTP [RAX]`

For exact `DD 10` and `DD 18`, extend only the finite canonical-Normal
binary80 source exponent interval in `bounded_x87_m64fp_result` from
`-1022..=1022` to `-1022..=1023`. Keep integer significand rounding and all
four FCWs. Normalize rounded significand carry before checking the final
exponent: carry from 1022 to 1023 is a valid binary64 normal result, but a
carry from 1023 to 1024 returns Unsupported before destination access. A
source already at exponent 1024 also remains Unsupported. This path does not
deliver masked overflow or set OE.

At this S2-31bc stage, subnormal-result rejection remains in place; S2-31be
later adds exact subnormal outputs. Keep the minimum exponent,
`FSW & 0x80dc == 0` gate, source classes/tags, addressing, and post-write
status/pointer/pop behavior unchanged. Rejected sources leave all CPU state and
destination bytes unchanged. Successful stores retain sticky IE/DE/PE and only generate PE for
inexact finite rounding; SNaN IE behavior remains unchanged. DD10 preserves
TOP; DD18 pops only after its atomic Qword write succeeds.

Separate maximum-finite fixtures exercise DD00 FLD, DD10 positive FST, and
DD18 negative FSTP followed by FNSTSW. ISA vectors cover both signs, both
opcodes, every TOP and FCW, all incoming IE/DE/PE combinations, exponent-1023
maximum finite, ties and directed rounding at 1022/1023, carry to valid 1023,
carry to rejected 1024, an exponent-1024 source and below-minimum rejection.
Overflow rejects before memory access; denied writes and canonical-range
failures preserve full state and bytes. JIT/native admission remains closed;
native runtime and physical acceptance remain deferred under ISSUE-019. See
the [plan](../plans/STAGE_2.md#s2-31bc-finite-exponent-1023-bounded-m64-stores),
[report](../reports/STAGE_2_REPORT.md#s2-31bc--finite-exponent-1023-bounded-m64-stores),
and [fixtures](../../tests/fixtures/x86/stage2/README.md).


<a id="local-s2-31bd-exact-binary32-subnormal-results"></a>

## Local S2-31bd: exact subnormal results through bounded m32 `FST/FSTP [RAX]`

For exact `D9 10` and `D9 18`, accept a canonical Normal binary80 source with
unbiased exponent `-149..=-127` only when its exact value is representable as
a binary32 subnormal. The integer conversion uses
`shift = -exponent - 86` (41..63), requires all discarded significand bits to
be zero, and requires the resulting fraction to be `1..=0x7fffff`. Preserve
the source sign. This path does not depend on RC and does not set DE, UE or PE.

An inexact subnormal result, source exponent below -149, invalid tag/class,
pseudo-encoding or malformed source remains Unsupported before destination
access. The existing FCWs, `FSW & 0x80dc == 0` gate, normal rounding path,
minimum-normal result, canonical48 Dword preflight, atomic write, status/pointer
behavior and FSTP pop ordering remain unchanged. Sticky IE/DE/PE are preserved;
C1 follows the existing exact-store clear policy. A denied write or failed
range check leaves CPU state and destination memory unchanged.

Literal D910 and D918/DFE0 fixtures store positive and negative minimum
subnormal binary32 values while retaining sticky DE; FNSTSW observes DE after
the pop. Direct FLD subnormal→FST tests prove DE survives without PE. Boundary
successful exact vectors cover shifts 63, 62 and 41; low-bit rejection
vectors cover every shift from 41 through 63. Coverage also includes the
minimum and maximum subnormal,
1.5-minimum-subnormal and half-step-above-maximum rejects, exponent -150 and
minimum normal. This is a narrow local conversion policy; other widths and
general unmasked exception delivery remain unchanged. JIT/native admission,
native runtime and physical acceptance remain deferred under ISSUE-019. See
the [plan](../plans/STAGE_2.md#s2-31bd-exact-binary32-subnormal-results-through-m32-stores),
[report](../reports/STAGE_2_REPORT.md#s2-31bd--exact-binary32-subnormal-results-through-m32-stores),
and [fixtures](../../tests/fixtures/x86/stage2/README.md).

S2-31bf extends the exact-only m32 subnormal result policy with masked inexact
rounding when the stored result remains a nonzero binary32 subnormal.


<a id="local-s2-31be-exact-binary64-subnormal-results"></a>

## Local S2-31be: exact subnormal results through bounded m64 `FST/FSTP [RAX]`

For exact `DD 10` and `DD 18`, accept a Valid-tagged canonical Normal binary80
source with unbiased exponent `-1074..=-1023` only when its exact value is
representable as a binary64 subnormal. Integer conversion uses
`shift = -exponent - 1011` (12..63), requires all discarded significand bits
to be zero, and requires the fraction to be `1..=0x000fffffffffffff`. Preserve
the sign. The conversion is independent of RC and adds no DE, UE or PE.

Inexact subnormal results, source exponents below -1074, invalid tags/classes,
pseudo-encodings and malformed sources remain Unsupported before destination
access. Existing FCWs, `FSW & 0x80dc == 0`, normal conversion and minimum-normal
behavior remain unchanged. Sticky IE/DE/PE are preserved, C1 follows the exact
store clear policy, and C0/C2/C3 follow the existing conversion policy. A
denied write or failed canonical48 Qword range check leaves CPU state and
destination memory unchanged; DD18 pops only after successful storage.

Signed minimum, adjacent and maximum-subnormal vectors, nontrivial fractions,
all shift-boundary rejects, 1.5 minimum subnormal, the half-step above maximum
subnormal and exponent -1075 are covered across TOP/FCW/sticky-flag matrices.
Minimum normal remains a normal-path control. DD00 subnormal FLD→DD10/DD18
composition proves DE retention without UE/PE; literal CLI fixtures exercise
bare FST and FSTP/FNSTSW. JIT/native admission remains closed. The Intel SDM
defines masked underflow only for tiny inexact results [Vol. 1, §4.9.1.5](https://cdrdv2.intel.com/929351/253665-093-sdm-vol-1.pdf#page=111); native runtime and physical acceptance remain deferred under ISSUE-019. See the
[plan](../plans/STAGE_2.md#s2-31be-exact-binary64-subnormal-results-through-m64-stores),
[report](../reports/STAGE_2_REPORT.md#s2-31be--exact-binary64-subnormal-results-through-m64-stores),
and [fixtures](../../tests/fixtures/x86/stage2/README.md).


<a id="local-s2-31bf-masked-inexact-binary32-subnormal-results"></a>

## Local S2-31bf: masked inexact subnormal results through m32 FST/FSTP

For only `D9 10` and `D9 18`, extend the bounded binary32 store path to
Valid-tagged canonical Normal binary80 sources with unbiased exponent
`-149..=-127`. Use integer quotient/remainder with
`shift = -exponent - 86` (41..63) and the accepted FCW's sign-aware rounding
mode. A rounded fraction of `1..=0x7fffff` is a supported binary32 subnormal.
A zero result, a source below -149, or promotion/carry to minimum normal remains
Unsupported before data access. Existing exact-subnormal stores keep their
status behavior.

An accepted inexact subnormal store adds sticky UE+PE (`0x0030`) and no DE.
C1 is set only when rounding increases the result magnitude; existing IE/DE/PE
remain sticky. The current FCW set and `FSW & 0x80dc == 0` gate remain in force,
so a pre-existing UE rejects the operation. Compute the rounded result before
access, but commit flags, FIP/DIP, and an FSTP pop only after the atomic Dword
write succeeds. Denied writes and canonical-range failures preserve full CPU
state and destination bytes.

Literal coverage includes 1.5 and 2.5 minimum-subnormal multiples, both signs,
all rounding modes, tie neighbors, every shift 41..63, and maximum-subnormal
directed rounding versus promotion rejection. Matrices cover both opcodes, all
TOP values and sticky IE/DE/PE combinations. CLI fixtures expose UE+PE with
FNSTSW; local instruction composition verifies FNCLEX clears them. Forbidden
tags/classes, the FSW UE gate and store atomicity are checked. JIT/native
admission stays closed; native runtime and physical acceptance remain deferred
under ISSUE-019. See the
[plan](../plans/STAGE_2.md#s2-31bf-masked-inexact-binary32-subnormal-results-through-m32-stores),
[report](../reports/STAGE_2_REPORT.md#s2-31bf--masked-inexact-binary32-subnormal-results-through-m32-stores),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


<a id="local-s2-31bg-masked-inexact-binary64-subnormal-results"></a>

## Local S2-31bg: masked inexact subnormal results through m64 FST/FSTP

For only `DD 10` and `DD 18`, extend the bounded binary64 store path to
Valid-tagged canonical Normal binary80 sources with unbiased exponent
`-1074..=-1023`. Use integer quotient/remainder with
`shift = -exponent - 1011` (12..63) and the accepted FCW's sign-aware rounding
mode. Accept only rounded fractions `1..=0x000fffffffffffff`. A zero result,
a source below -1074, or promotion/carry to minimum normal remains Unsupported
before data access. Existing exact-subnormal stores retain their previous
status behavior.

An accepted inexact subnormal store adds sticky UE+PE (`0x0030`) and no DE.
C1 is set only when rounding increases the magnitude; incoming IE/DE/PE remain
sticky. The existing FCW whitelist and `FSW & 0x80dc == 0` gate remain in
force, so pre-existing UE rejects the operation. Compute the result before
access, but commit flags, FIP/DIP, RIP and an FSTP pop only after the atomic
Qword write succeeds. Denied writes and canonical-range failures preserve full
CPU state and destination bytes.

Coverage includes 1.5 and 2.5 minimum-subnormal ties and neighbors, both signs,
all rounding modes, every shift 12..63, maximum-subnormal directed rounding
versus minimum-normal promotion rejection, both opcodes/all TOPs/all incoming
IE/DE/PE combinations, below-minimum and UE-gate rejection, and FNSTSW/FNCLEX.
Write and range failures are atomic. Literal CLI fixtures expose UE+PE; JIT
and native admission remain closed. Native runtime and physical acceptance
remain deferred under ISSUE-019. See the
[plan](../plans/STAGE_2.md#s2-31bg-masked-inexact-binary64-subnormal-results-through-m64-stores),
[report](../reports/STAGE_2_REPORT.md#s2-31bg--masked-inexact-binary64-subnormal-results-through-m64-stores),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


## Local S2-31bh: sticky UE re-entry for bounded m32/m64 stores

Only bounded `D9 10`, `D9 18`, `DD 10`, and `DD 18` result helpers admit the
FSW mask `0x80cc`. Incoming masked IE/DE/UE/PE combinations are preserved;
ZE/OE/SF/ES/B remain rejected, and FCW whitelists are unchanged. A pre-existing
UE does not itself synthesize PE. Conversion-generated IE, PE, or UE+PE is
committed together with status, pointers, RIP, and any FSTP pop only after the
atomic Dword or Qword write succeeds. Exact subnormal stores retain their prior
status behavior; inexact subnormal stores retain the S2-31bf/bg rounding rules.
FLD and m80 gates remain unchanged.

Coverage includes all 16 sticky IE/DE/UE/PE combinations across admitted
classes, all four store opcodes and FCWs, exact/inexact paths, repeated masked
underflow, m32 underflow followed by an exact m64 store from the unchanged
binary80 source, FNSTSW/FNCLEX, fault atomicity, forbidden status/control
values, and CLI/JIT/native denial. Native execution and physical acceptance
remain deferred under ISSUE-019.


## Local S2-31bi: sticky UE on raw binary80 FSTP m80fp

Only the bounded `DB 38` helper admits `FSW & 0x80cc == 0`, allowing incoming
masked IE/DE/UE/PE in any combination while preserving rejection of ZE/OE/SF/ES/B.
FCWs, accepted source tag/class pairs, the raw ten-byte value, and C1-clear
policy remain unchanged. The store generates no flags: UE alone does not add
IE/PE, and signaling NaNs are neither quieted nor normalized. Status, pointers,
RIP and the FSTP pop commit only after the single atomic ten-byte write
succeeds.

Coverage includes all 16 sticky combinations across accepted classes/signs,
FCWs and TOP; UE-only SNaN preservation; m32/m64 underflow followed by raw
m80 FSTP from the unchanged source; FNSTSW/FNCLEX; forbidden statuses and
controls; write/range atomicity; and CLI/JIT/native denial. Native execution and
physical acceptance remain deferred under ISSUE-019.


## Local S2-31bj: extreme binary32 underflow to zero or minimum subnormal

Only bounded `D9 10` and `D9 18` accept canonical Normal binary80 exponents
from -16382 through -150. Convert directly to signed zero or signed minimum
binary32 subnormal under the existing four FCWs. At exponent -150, compare the
source significand with the exact half-way value; below -150, only directed
rounding away from zero selects the minimum subnormal. This avoids shifting by
the full exponent distance. All admitted values are inexact: after a successful
atomic Dword write, set sticky UE+PE and set C1 iff the minimum subnormal was
selected. Preserve sign on zero and leave the binary80 source unchanged.

FCW/FSW gates, tags, exact conversions and atomic status/pointer/RIP/FSTP-pop
commit remain as before. Coverage includes half-neighbors, lowest exponent,
both signs/RCs, all TOPs/opcodes/sticky IE/DE/UE/PE combinations, exact minimum
control, FNSTSW/FNCLEX, raw-m80 composition, faults, forbidden classes/gates, and
CLI/JIT/native denial. Native runtime and physical acceptance remain deferred
under ISSUE-019. See [plan](../plans/STAGE_2.md#s2-31bj-round-extreme-underflow-through-binary32-stores),
[report](../reports/STAGE_2_REPORT.md#s2-31bj--extreme-binary32-underflow-to-zero-or-minimum-subnormal),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


## Local S2-31bk: extreme binary64 underflow to zero or minimum subnormal

Only bounded `DD 10` and `DD 18` accept canonical Normal binary80 exponents
from -16382 through -1075. Convert directly to signed zero or signed minimum
binary64 subnormal under the existing four FCWs. At exponent -1075, compare the
significand with the exact half-way value; below -1075, only directed rounding
away from zero selects the minimum subnormal. This avoids shifting by the full
exponent distance. All admitted values are inexact: after a successful atomic
Qword write, set sticky UE+PE and set C1 iff minimum subnormal is selected.
Preserve sign on zero and leave the binary80 source unchanged.

FCW/FSW gates, tags, exact conversions and atomic status/pointer/RIP/FSTP-pop
commit remain unchanged. Coverage includes half-neighbors, lowest exponent, both
signs/RCs, all TOPs/opcodes/sticky IE/DE/UE/PE combinations, exact-minimum
control, FNSTSW/FNCLEX, raw-m80 composition, faults, forbidden classes/gates, and
CLI/JIT/native denial. Native runtime and physical acceptance remain deferred
under ISSUE-019. See [plan](../plans/STAGE_2.md#s2-31bk-round-extreme-underflow-through-binary64-stores),
[report](../reports/STAGE_2_REPORT.md#s2-31bk--extreme-binary64-underflow-to-zero-or-minimum-subnormal),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).

## Local S2-31bl: binary80 subnormal to binary32 underflow rounding

Only bounded `D9 10` and `D9 18` accept Special-tagged canonical binary80
Subnormal sources (`E=0`, `J=0`, nonzero fraction), for either sign. All such
magnitudes are below half of the minimum binary32 subnormal. Nearest and
toward-zero store signed zero; toward-positive stores positive minimum
subnormal or negative zero; toward-negative stores negative minimum subnormal
or positive zero. Every admitted result is inexact and underflowed: commit
sticky UE+PE only after a successful atomic Dword write, and set C1 iff minimum
subnormal is selected. Preserve source bytes and the existing FSW/FCW gate,
status, pointer, TOP and FSTP-pop atomicity. Reject noncanonical encodings,
other tags and unsupported classes before memory access.

Coverage includes fractions 1, 2, 0x4000... and 0x7fff..., both signs/rounding
modes/opcodes/TOPs and all 16 sticky IE/DE/UE/PE states, C1 replacement,
raw-m80 source preservation, zero/minimum-normal controls, forbidden classes,
write/range atomicity, FNSTSW/FNCLEX, and CLI/JIT/native denial. Native runtime
and physical acceptance remain deferred under ISSUE-019. See [plan](../plans/STAGE_2.md#s2-31bl-round-binary80-subnormals-through-binary32-stores),
[report](../reports/STAGE_2_REPORT.md#s2-31bl--binary80-subnormal-to-binary32-underflow-rounding),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).

## Local S2-31bm: binary80 subnormal to binary64 underflow rounding

Only bounded `DD 10` and `DD 18` accept Special-tagged canonical binary80
Subnormal sources (`E=0`, `J=0`, nonzero fraction). Their magnitudes are below
half of the minimum binary64 subnormal. Nearest and toward-zero store signed
zero; toward-positive stores positive minimum subnormal or negative zero;
toward-negative stores negative minimum subnormal or positive zero. Every
admitted conversion is inexact and underflowed: commit sticky UE+PE after a
successful atomic Qword write, set no DE, and set C1 iff minimum subnormal is
selected. Preserve the ten source bytes and the existing FCW/FSW gate, with
status, pointer, RIP and FSTP-pop state committed atomically. Reject other
tags, noncanonical encodings and unsupported classes before access.

Coverage includes the four requested fractions, both signs/rounding modes/
opcodes/all TOPs and all 16 sticky IE/DE/UE/PE combinations, C1 replacement,
negative signed-zero/minimum-subnormal results, FLD m80→FST m64→FNCLEX→FSTP m80
preservation, write/range atomicity, invalid classes/tags/gates, FNSTSW/FNCLEX,
and literal CLI/JIT/native denial. Native runtime and physical acceptance
remain deferred under ISSUE-019. See [plan](../plans/STAGE_2.md#s2-31bm-round-binary80-subnormals-through-binary64-stores),
[report](../reports/STAGE_2_REPORT.md#s2-31bm--binary80-subnormal-to-binary64-underflow-rounding),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).

## Local S2-31bn: binary80 exponent -127 to binary32 minimum normal

Only bounded `D9 10` and `D9 18` extend the existing Valid-tagged canonical
binary80 Normal m32 conversion for exponent -127. Compute the destination
rounding directly from `S >> 41` and the low 41-bit remainder under the current
sign-aware RC. Newly accept only cases where this direct result is exactly
`0x800000`, and store binary32 minimum normal (`0x00800000`, with source sign).
The stored result is never formed from an auxiliary rounded value.

For this minimum-normal result, add PE and C1. Determine UE separately by
rounding the unbounded 24-bit significand using `S >> 40` and its low 40-bit
remainder under the same sign/RC; add UE iff the rounded significand is less
than `1 << 24`. This makes UE independent of the final binary32 exponent and
preserves an existing UE when no new UE is indicated. Existing FCW/FSW gates,
all source bytes, and every non-target conversion remain unchanged. Write one
Dword atomically, then commit sticky flags, FIP/FDP/RIP and FSTP pop. Any
preflight, range or write failure leaves CPU state and destination unchanged.

The exact-integer oracle and ISA matrix cover the nearest-even destination
threshold, directed-rounding threshold and neighbors, the distinct 24-bit UE
threshold, both signs, all four FCWs, both opcodes, every TOP and all 16 sticky
IE/DE/UE/PE combinations. Maximum-subnormal and exact minimum-normal controls,
write/range atomicity and FNSTSW/FNCLEX are covered. Literal CLI fixtures expose
UE+PE+C1 with FNSTSW and a negative FSTP minimum-normal result where no new UE is
raised; JIT and native admission deny both fixtures. Native runtime and
physical acceptance remain deferred under ISSUE-019. See [plan](../plans/STAGE_2.md#s2-31bn-round-exponent--127-binary80-normals-directly-to-binary32-minimum-normal),
[report](../reports/STAGE_2_REPORT.md#s2-31bn--binary80-normal-to-binary32-minimum-normal-rounding),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).

## Local S2-31bo: binary80 exponent -1023 to binary64 minimum normal

Only bounded `DD 10` and `DD 18` extend the existing Valid-tagged canonical
binary80 Normal m64 conversion for exponent -1023. Compute destination bits
directly by rounding `S / 2^12` under the current sign-aware RC. Newly accept
only when that direct result is exactly `1 << 52`, and store binary64 minimum
normal (`0x0010000000000000`, with source sign). Do not construct the stored
value from an auxiliary rounded significand.

For this result, add PE+C1. Determine UE independently by rounding the
unbounded 53-bit significand from `S >> 11` and the low 11-bit remainder under
the same sign/RC; add UE iff that rounded significand remains below `1 << 53`.
Preserve any pre-existing IE/DE/UE/PE. The existing FCW/FSW gates, source bytes
and all other m64 paths remain unchanged. The Qword write precedes sticky flags,
FIP/FDP/RIP updates and FSTP pop; write, range and guest faults leave CPU state
and memory unchanged.

The exact-integer oracle covers nearest-even and directed-rounding destination
thresholds and neighbors, the separate wide UE threshold, maximum-subnormal
and minimum-normal controls, both signs, all four RCs, both opcodes, every TOP
and all 16 sticky IE/DE/UE/PE combinations. Local tests also cover write/range
atomicity and FNSTSW/FNCLEX. Literal CLI fixtures expose UE+PE+C1 at the
nearest threshold and PE+C1 without new UE for a negative FSTP directed case;
JIT and native admission deny both. Native runtime and physical x86 behavior
remain deferred under ISSUE-019. See the [plan](../plans/STAGE_2.md#s2-31bo-round-exponent--1023-binary80-normals-directly-to-binary64-minimum-normal), [report](../reports/STAGE_2_REPORT.md#s2-31bo--binary80-normal-to-binary64-minimum-normal-rounding), and [fixture notes](../../tests/fixtures/x86/stage2/README.md).

## Local S2-31bp: binary80 exponent 127 overflow to binary32 infinity

Only bounded `D9 10` and `D9 18` extend the existing Valid-tagged canonical
binary80 Normal m32 conversion for exponent 127. When sign-aware rounding of
`S / 2^40` produces `1 << 24`, store signed binary32 infinity. A successful
atomic Dword write then commits sticky OE+PE and C1. No IE, DE or UE is
generated; prior IE/DE/UE/PE remain preserved. The existing FCW whitelist and
`FSW & 0x80cc == 0` gate remain, so sticky OE blocks later stores until
FNCLEX. Source bytes and FST/FSTP state/pointer/pop behavior remain unchanged.
Exponent-128 sources and any other unsupported classes still reject before
memory access.

An exact-integer matrix covers `0xffffff0000000000`, its successor, the
nearest-even predecessor/midpoint/successor, and the maximum significand, for
both signs, every RC, both opcodes, every TOP and all 16 sticky
IE/DE/UE/PE combinations. Maximum finite output and exponent-128 rejection,
Dword write/range atomicity, FNSTSW/FNCLEX, sticky-OE re-entry, and literal
interpreter fixtures with JIT/native denial are covered. Native runtime and
physical x86 behavior remain deferred under ISSUE-019. See the [plan](../plans/STAGE_2.md#s2-31bp-round-exponent-127-binary80-normals-to-binary32-infinity-on-overflow), [report](../reports/STAGE_2_REPORT.md#s2-31bp--binary80-normal-to-binary32-infinity-on-overflow), and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


## Local S2-31bq: binary80 exponent 1023 overflow to binary64 infinity

Only bounded `DD 10` and `DD 18` extend the existing Valid-tagged canonical
binary80 Normal m64 conversion at exponent 1023. Sign-aware rounding of
`S / 2^11` that carries to `1 << 53` stores signed binary64 infinity. After a
successful atomic Qword write, commit sticky OE+PE and C1; do not generate IE,
DE or UE. Preserve prior sticky IE/DE/UE/PE, source bytes, and FST/FSTP stack,
pointer, and retirement behavior. The existing FCW whitelist and
`FSW & 0x80cc == 0` gate remain, so OE prevents another bounded store until
FNCLEX clears it. Exponent-1024 and malformed sources reject before access.

An exact-integer oracle covers maximum finite, first value above maximum,
nearest predecessor/tie/successor, and maximum significand for both signs, all
RCs, both opcodes, every TOP, and all sticky IE/DE/UE/PE combinations. Tests
also cover directed finite/infinity selection, exponent-1024 rejection, write
and range atomicity, FNSTSW/FNCLEX, and literal interpreter fixtures. JIT and
native admission remain closed; native runtime and physical acceptance remain
deferred under ISSUE-019. See the [plan](../plans/STAGE_2.md#s2-31bq-round-exponent-1023-binary80-normals-to-binary64-infinity-on-overflow),
[report](../reports/STAGE_2_REPORT.md#s2-31bq--binary80-normal-to-binary64-infinity-on-overflow),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


## Local S2-31br: sticky status admission for bounded m32/m64 FST/FSTP

Only bounded `D9 10`, `D9 18`, `DD 10`, and `DD 18` admit prior sticky IE,
DE, OE, UE, and PE by using `FSW & 0x80c4 == 0`; ZE, SF, ES, and B still reject.
The FCW whitelist, source and address gates, and conversion results remain
unchanged. Prior sticky exception flags are preserved, while current
conversion flags and C1 are recomputed after a successful atomic write. In
particular, OE alone does not set PE. FST/FSTP source, stack, pointer, and
retirement behavior remains atomic. Other x87 operations keep their existing
status gates.

The ISA matrix covers 32 sticky IE/DE/OE/UE/PE combinations over both widths,
both opcodes, signs, RCs and TOPs, with representative zero, finite and special
values. It also checks sticky-OE overflow re-entry without FNCLEX, FNSTSW/FNCLEX,
and write/range atomicity. An OE-only exact m64 CLI fixture verifies preserved
OE without newly generated PE or C1. JIT and native admission remain closed;
native runtime and physical x86 behavior remain deferred under ISSUE-019. See
the [plan](../plans/STAGE_2.md#s2-31br-admit-sticky-status-on-bounded-m32m64-stores),
[report](../reports/STAGE_2_REPORT.md#s2-31br--sticky-status-admission-for-bounded-m32m64-stores),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


<a id="local-s2-31cc-masked-canonical-qnan-fist-m32int"></a>

## Local S2-31cc: masked canonical quiet NaN through m32 `FIST`/`FISTP` (`DB 10`/`DB 18`)

Only bare `DB 10` and `DB 18` admit Special-tagged canonical binary80 quiet
NaNs: exponent `0x7fff`, either sign, J=1 and quiet bit=1, with any lower
payload. Under the existing FCW whitelist, either sign and every RC mode store
Dword integer indefinite `0x80000000`. After the atomic Dword write, set sticky
IE, clear C1, preserve incoming PE and C0/C2/C3, and add no PE/DE/OE/UE/SF;
ES/B remain clear. The FSW gate continues to reject incoming IE.

Keep bare Long64 RAX addressing, canonical48 four-byte preflight, raw source
bytes, one write/no read, and post-write FIST/FISTP commit. FIST retains tag/TOP;
FISTP empties old TOP and advances it only after successful write. SNaN, J=0
pseudo-NaNs, malformed or mismatched tags, finite out-of-range inputs, m16/m64
QNaNs and unmasked delivery remain unsupported.

Coverage includes both signs, representative payloads, admitted FCWs, opcodes,
TOP and incoming PE/C1, malformed/tag/class rejection, write/range atomicity,
and FISTP/FNSTSW/FNCLEX observation. JIT/native admission remains closed;
native runtime and physical behavior remain deferred under ISSUE-019. See the
[plan](../plans/STAGE_2.md#s2-31cc-masked-binary80-qnan-through-bounded-m32-fist),
[report](../reports/STAGE_2_REPORT.md#s2-31cc--masked-binary80-qnan-through-m32-fistfstp),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


<a id="local-s2-31cd-masked-canonical-qnan-fist-m16int"></a>

## Local S2-31cd: masked canonical quiet NaN through m16 `FIST`/`FISTP` (`DF 10`/`DF 18`)

Only bare `DF 10` and `DF 18` admit Special-tagged canonical binary80 quiet
NaNs: exponent `0x7fff`, either sign, J=1 and quiet bit=1, with arbitrary lower
payload. Under the existing FCW whitelist, either sign and every RC mode store
Word integer indefinite `0x8000`. After the atomic Word write, set sticky IE,
clear C1, preserve incoming PE and C0/C2/C3, and add no PE/DE/OE/UE/SF; ES/B
remain clear. The FSW gate continues to reject incoming IE.

Keep bare Long64 RAX addressing, full canonical48 two-byte preflight, raw source
bytes, one write/no read, and post-write commit. FIST retains tag/TOP; FISTP
empties old TOP and advances it only after successful write. SNaN, J=0
pseudo-NaNs, malformed or mismatched tags, m64 QNaNs and unmasked delivery
remain unsupported; m32 behavior remains unchanged.

Coverage includes signs, representative payloads, admitted FCWs, opcodes, every
TOP and incoming PE/C1, malformed/tag/class/control/prefix rejection, Word
canaries, write/range atomicity and FISTP/FNSTSW/FNCLEX observation. JIT/native
admission remains closed; native runtime and physical behavior remain deferred
under ISSUE-019. See the
[plan](../plans/STAGE_2.md#s2-31cd-masked-binary80-qnan-through-bounded-m16-fist),
[report](../reports/STAGE_2_REPORT.md#s2-31cd--masked-binary80-qnan-through-m16-fistfstp),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


<a id="local-s2-31ce-masked-canonical-qnan-fistp-m64int"></a>

## Local S2-31ce: masked canonical quiet NaN through m64 `FISTP` (`DF 38`)

Only bare `DF 38` admits Special-tagged canonical binary80 quiet NaNs:
exponent `0x7fff`, either sign, J=1 and quiet bit=1, with arbitrary lower
payload. Under the existing FCW whitelist, either sign and every RC mode store
Qword integer indefinite `0x8000000000000000`. After the atomic Qword write,
set sticky IE, clear C1, preserve incoming PE and C0/C2/C3, and add no
PE/DE/OE/UE/SF; ES/B remain clear. The FSW gate continues to reject incoming
IE.

Keep bare Long64 RAX addressing, full canonical48 eight-byte preflight, raw
source bytes, one write/no read, and post-write FISTP commit/pop. SNaN, J=0
pseudo-NaNs, malformed or mismatched tags, finite overflow, non-pop m64 and
unmasked delivery remain unsupported. Existing m16/m32 QNaN paths are unchanged.

Coverage includes signs, representative payloads, admitted FCWs, every TOP and
incoming PE/C1, malformed/tag/class/control/prefix rejection, Qword write/range
atomicity and FISTP/FNSTSW/FNCLEX observation. JIT/native admission remains
closed; native runtime and physical behavior remain deferred under ISSUE-019.
See the [plan](../plans/STAGE_2.md#s2-31ce-masked-binary80-qnan-through-bounded-m64-fistp),
[report](../reports/STAGE_2_REPORT.md#s2-31ce--masked-binary80-qnan-through-m64-fistp),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


<a id="local-s2-31cf-masked-canonical-snan-fist-m32int"></a>

## Local S2-31cf: masked canonical signaling NaN through m32 `FIST`/`FISTP` (`DB 10`/`DB 18`)

Only bare `DB 10` and `DB 18` admit Special-tagged canonical binary80 SNaNs:
exponent `0x7fff`, either sign, J=1, quiet bit=0, and a nonzero lower 62-bit
payload. Under the existing FCW whitelist, either sign and every RC mode store
Dword integer indefinite `0x80000000`. After the atomic Dword write, set sticky
IE, clear C1, preserve incoming PE and C0/C2/C3, and add no PE/DE/OE/UE/SF or
ES/B. Preserve the raw signaling source bytes and existing addressing and
post-write FIST/FISTP commit/pop behavior. Canonical infinity and QNaN remain
covered by earlier slices; J=0 pseudo-NaNs, mismatched tags, finite overflow,
and unmasked delivery remain unsupported. The m16 and m64 SNaN operations are
outside this m32 slice and are specified separately in S2-31cg and S2-31ch.

Coverage includes four payload shapes, both signs, all admitted FCWs, both
opcodes, every TOP and incoming PE/C1 (2,048 combinations), with source-byte,
tag/pop, write/range atomicity and control checks. A literal FISTP/FNSTSW
fixture and FNSTSW/FNCLEX unit sequence observe and clear IE. JIT/native
admission remains closed; native runtime and physical behavior remain deferred
under ISSUE-019. See the [plan](../plans/STAGE_2.md#s2-31cf-masked-binary80-snan-through-bounded-m32-fist),
[report](../reports/STAGE_2_REPORT.md#s2-31cf--masked-binary80-snan-through-m32-fistfstp),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


<a id="local-s2-31cg-masked-canonical-snan-fist-m16int"></a>

## Local S2-31cg: masked canonical signaling NaN through m16 `FIST`/`FISTP` (`DF 10`/`DF 18`)

Only bare `DF 10` and `DF 18` admit Special-tagged canonical binary80 SNaNs:
exponent `0x7fff`, either sign, J=1, quiet bit=0, and a nonzero lower 62-bit
payload. Under the existing FCW whitelist, either sign and every RC mode store
Word integer indefinite `0x8000`. After the atomic Word write, set sticky IE,
clear C1, preserve incoming PE and C0/C2/C3, and add no PE/DE/OE/UE/SF or ES/B.
Preserve raw signaling source bytes and the current addressing and post-write
FIST/FISTP state/pop behavior. The m32 SNaN behavior is unchanged; J=0
pseudo-NaNs, mismatched tags and unmasked delivery remain unsupported. The m64
SNaN form is specified separately in S2-31ch.

Coverage includes four payload patterns, both signs, all admitted FCWs, both
opcodes, every TOP and incoming PE/C1 (2,048 combinations), raw source/tag/pop
preservation, one Word write/no read, class/tag/control/prefix rejection,
range and write atomicity, and FNSTSW/FNCLEX. The former m64 `DF 38` denial is
superseded by S2-31ch. JIT/native admission remains
closed; native runtime and physical behavior remain deferred under ISSUE-019.
See the [plan](../plans/STAGE_2.md#s2-31cg-masked-binary80-snan-through-bounded-m16-fist),
[report](../reports/STAGE_2_REPORT.md#s2-31cg--masked-binary80-snan-through-m16-fistfstp),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


<a id="local-s2-31ch-masked-canonical-snan-fistp-m64int"></a>

## Local S2-31ch: masked canonical signaling NaN through m64 `FISTP` (`DF 38`)

Only bare `DF 38` admits Special-tagged canonical binary80 SNaNs: exponent
`0x7fff`, either sign, J=1, quiet bit=0, and a nonzero lower 62-bit payload.
Under the existing FCW whitelist, either sign and every RC mode store Qword
integer indefinite `0x8000000000000000`. After the atomic Qword write, set
sticky IE, clear C1, preserve incoming PE and C0/C2/C3, and add no
PE/DE/OE/UE/SF or ES/B. Preserve raw signaling source bytes and post-write
FISTP commit/pop behavior. The existing FSW gate rejects incoming IE.

Keep bare Long64 RAX addressing, full canonical48 Qword preflight, one write/no
read, and raw payload preservation. J=0 pseudo-NaNs, malformed or mismatched
tags, finite overflow and unmasked delivery remain unsupported. Existing m16
and m32 SNaN paths are unchanged; no non-pop m64 form is admitted.

Coverage includes four payload patterns, both signs, all FCWs, every TOP and
incoming PE/C1 (1,024 combinations), source/tag/pop preservation, class/tag/
control/prefix rejection, Qword canaries, range/write atomicity and
FNSTSW/FNCLEX. JIT/native admission remains closed; native runtime and physical
behavior remain deferred under ISSUE-019. See the
[plan](../plans/STAGE_2.md#s2-31ch-masked-binary80-snan-through-bounded-m64-fistp),
[report](../reports/STAGE_2_REPORT.md#s2-31ch--masked-binary80-snan-through-m64-fistp),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


<a id="local-s2-31ci-admit-sticky-ie-bounded-integer-stores"></a>

## Local S2-31ci: admit sticky IE on bounded integer stores

Only bare DB 10, DB 18, DF 10, DF 18 and DF 38 integer-store forms change
their entry status mask from 0x80df to 0x80de. This admits incoming IE and PE
together or independently, while DE/ZE/OE/UE/SF/ES/B remain blocked. The FCW
whitelist, source/tag classes, finite bounds, addressing and post-write
FIST/FISTP commit rules do not change. FILD, FRNDINT, floating stores and every
other x87 gate remain unchanged.

Preserve incoming IE/PE. Historical IE does not force an indefinite integer
and does not synthesize PE; only flags produced by the current conversion are
added after its successful atomic write. C1 reflects only the current
conversion, while C0/C2/C3 and raw source bytes are preserved. Status, pointers,
RIP and pop/tag updates remain post-write, so failed writes and range faults
leave the full architectural state and memory unchanged.

The independent result matrix crosses zero, exact/inexact Normal, fraction,
infinity, QNaN and SNaN with both signs, all five forms, four FCWs, every TOP,
four incoming IE/PE combinations and both incoming C1 states (17,920 cases).
Additional tests cover IE-only exact finite conversion, masked-invalid FIST to
FISTP without FNCLEX, IE+PE write/range atomicity and IE combined with each
forbidden status bit across the five forms. Literal CLI fixtures show
masked-invalid status and exact finite storage under sticky IE; direct
FNSTSW/FNCLEX composition observes then clears exception flags. JIT/native
admission remains closed; native runtime and physical x86 behavior are
unverified under ISSUE-019. See the
[plan](../plans/STAGE_2.md#s2-31ci-admit-sticky-ie-on-bounded-integer-stores),
[report](../reports/STAGE_2_REPORT.md#s2-31ci--sticky-ie-on-bounded-integer-stores),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


<a id="local-s2-31cj-masked-finite-overflow-m32-fist"></a>

## Local S2-31cj: masked finite overflow for m32 `FIST`/`FISTP`

For canonical Valid-tagged binary80 Normal input to bare DB 10/DB 18 only,
compute the RC-rounded integer before checking signed i32 bounds. If the
rounded integer is outside `[-2^31, 2^31-1]`, write integer-indefinite
`0x80000000`, then add IE and clear C1 after the atomic Dword write. Do not add
PE/OE/UE/DE/SF/ES/B. Preserve incoming IE/PE and C0/C2/C3. An out-of-range
real value that rounds back into range remains a finite integer result with
PE and current-operation C1 as appropriate. For unbiased exponent at least 32,
classify invalid before any wide shift; for exponents 0 through 31, use exact
significand quotient/remainder rounding before the bounds comparison. Negative
exponent fractions retain their existing exact comparison-based path.

Keep the FCW whitelist, 0x80de FSW gate, canonical48 Dword preflight, bare RAX
addressing, one write/no read, raw source bytes, and post-write state commit.
S2-31ck subsequently extended the m16 overflow path, and S2-31cl adds the
bounded m64 overflow path. Binary80 subnormals, malformed classes and unmasked
behavior remain unchanged.
See the
[plan](../plans/STAGE_2.md#s2-31cj-masked-finite-overflow-to-m32-fistfstp),
[report](../reports/STAGE_2_REPORT.md#s2-31cj--masked-finite-overflow-to-m32-fistfstp),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


<a id="local-s2-31ck-masked-finite-overflow-m16-fist"></a>

## Local S2-31ck: masked finite overflow for m16 `FIST`/`FISTP`

For canonical Valid-tagged binary80 Normal input to bare DF 10/DF 18 only,
compute the RC-rounded integer before checking signed i16 bounds. If the
rounded integer is outside `[-2^15, 2^15-1]`, write Word integer-indefinite
`0x8000`, then add IE and clear C1 after the atomic Word write. Do not add
PE/OE/UE/DE/SF/ES/B. Preserve incoming IE/PE and C0/C2/C3. An out-of-range
real value that rounds back into range remains a finite integer result with
PE and current-operation C1 as appropriate. For unbiased exponent at least 16,
classify invalid before any wide shift; for exponents 0 through 15, use exact
significand quotient/remainder rounding before the bounds comparison.
Negative-exponent fractions retain their existing comparison-based path.

Keep the FCW whitelist, 0x80de FSW gate, canonical48 Word preflight, bare RAX
addressing, one write/no read, raw source bytes, and post-write state commit.
The m32 overflow path, binary80 subnormals, malformed classes and unmasked
behavior remain unchanged; S2-31cl later adds bounded m64 finite overflow. See
the
[plan](../plans/STAGE_2.md#s2-31ck-masked-finite-overflow-to-m16-fistfstp),
[report](../reports/STAGE_2_REPORT.md#s2-31ck--masked-finite-overflow-to-m16-fistfstp),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


<a id="local-s2-31cl-masked-finite-overflow-m64-fistp"></a>

## Local S2-31cl: masked finite overflow through m64 `FISTP` (`DF 38`)

Only bare `DF 38` admits Valid-tagged canonical binary80 Normal values for
finite rounded overflow handling. Round the exact significand quotient and
remainder with the existing FCW rounding mode first, then compare the signed
integer result to `[-2^63, 2^63-1]`. A rounded out-of-range value stores Qword
integer-indefinite `0x8000000000000000`; only after the atomic Qword write does
the operation add sticky IE and clear C1. It adds no PE/OE/UE/DE/SF/ES/B and
preserves incoming IE/PE and C0/C2/C3. Values just outside the real-number
range that round back into signed range remain finite; inexact roundback adds
PE and sets C1 only when the rounded magnitude increased.

At unbiased exponent 63, positive magnitudes are out of range; negative
significand `0x8000000000000000` is exact `i64::MIN`, and larger magnitudes
are invalid. Exponent 64 and above is invalid without oversized shifts. The
existing comparison-only fraction path below one is unchanged. Keep the FCW
whitelist, `FSW & 0x80de` gate, bare RAX, full canonical48 Qword preflight, one
atomic write/no read, raw source bytes, and post-write status/pointer/RIP/pop
commit. Subnormals, malformed sources, broader status admission and unmasked
delivery remain outside this slice.

Independent literal vectors cover both sides of the signed boundaries,
half-integer roundback, exponent 64/65/16383 with minimum/mixed/maximum
significands, both signs, all RC modes, TOP values, sticky IE/PE and prior C1.
Write/range failure tests preserve complete state and memory; FNSTSW/FNCLEX
observes and clears IE. CLI fixture coverage demonstrates integer-indefinite
and IE, while JIT/native admission remains closed. See the
[plan](../plans/STAGE_2.md#s2-31cl-masked-finite-overflow-to-m64-fistp),
[report](../reports/STAGE_2_REPORT.md#s2-31cl--masked-finite-overflow-to-m64-fistp),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


<a id="local-s2-31cm-admit-sticky-de-bounded-integer-stores"></a>

## Local S2-31cm: admit sticky DE on bounded integer stores

Only the existing bare `DB 10`, `DB 18`, `DF 10`, `DF 18` and `DF 38` bounded
integer-store forms now admit incoming DE. Their shared entry condition is
`FSW & 0x80dc == 0`: incoming IE, DE and PE are preserved independently,
while ZE/OE/UE/SF/ES/B remain rejected. DE by itself does not alter the
conversion result or add IE/PE. Only current conversion flags and C1 are
updated, after the atomic Dword/Word/Qword write succeeds. The FCW whitelist,
source/tag/class/range rules, address form, payload preservation and pop
ordering remain unchanged. Binary80 subnormal and pseudo-denormal integer
sources remain rejected; FLD may create a canonical Normal value from a
binary32/binary64 subnormal, after which the existing exact fraction path can
round it to an integer and retain sticky DE.

The matrix covers 35,840 combinations across seven admitted classes, both
signs, five forms, four FCWs, eight TOP values, incoming IE/DE/PE and both prior
C1 states. Forty composition cases cover min/max signed subnormals of both
source widths through all five stores without FNCLEX. Five-form write/range
fault tests preserve complete state and memory with IE|DE|PE set; each
forbidden status is also tested combined with DE. CLI fixtures exercise the
FISTP/FNSTSW pair with the corresponding loaded binary80 intermediate and
sticky DE. JIT/native admission remains denied. Native runtime and physical
x86 acceptance remain deferred under ISSUE-019. See the
[plan](../plans/STAGE_2.md#s2-31cm-admit-sticky-de-on-bounded-integer-stores),
[report](../reports/STAGE_2_REPORT.md#s2-31cm--sticky-de-on-bounded-integer-stores),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


<a id="local-s2-31cn-admit-sticky-ue-bounded-integer-stores"></a>

## Local S2-31cn: admit sticky UE on bounded integer stores

Only the existing bare `DB 10`, `DB 18`, `DF 10`, `DF 18` and `DF 38` bounded
integer-store forms now admit incoming UE. Their shared entry condition is
`FSW & 0x80cc == 0`: IE, DE, UE and PE are preserved independently, while
ZE/OE/SF/ES/B remain rejected. Historical UE by itself does not alter the
rounded integer or synthesize IE/PE. Current-operation flags and C1 remain
operation-specific and commit only after the atomic Dword/Word/Qword write.
FCW, source/tag/class/range/address rules and FIST/FISTP pop ordering are
unchanged. Raw binary80 subnormal and pseudo-denormal integer operands remain
unsupported.

The matrix covers 71,680 combinations across seven admitted classes, both
signs, five forms, four FCWs, eight TOP values, all IE/DE/UE/PE combinations
and both prior C1 values. Forty FST m32→integer-store composition cases first
produce sticky UE|PE from 1.5 times the minimum binary32 subnormal, then verify
that each store preserves UE|PE and recomputes C1. Five-form denied-write and
canonical-range tests preserve full state/memory with IE|DE|UE|PE set; UE
combined with each remaining forbidden status rejects before access. Literal
CLI fixtures cover m32/m64 FISTP followed by FNSTSW. JIT/native admission
remains denied, while native runtime and physical x86 acceptance remain
deferred under ISSUE-019. See the
[plan](../plans/STAGE_2.md#s2-31cn-admit-sticky-ue-on-bounded-integer-stores),
[report](../reports/STAGE_2_REPORT.md#s2-31cn--sticky-ue-on-bounded-integer-stores),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


<a id="local-s2-31co-admit-sticky-oe-bounded-integer-stores"></a>

## Local S2-31co: admit sticky OE on bounded integer stores

Only bare `DB 10`, `DB 18`, `DF 10`, `DF 18` and `DF 38` bounded integer
stores admit incoming sticky OE. Their shared gate is `FSW & 0x80c4 == 0`,
which preserves IE/DE/OE/UE/PE while rejecting ZE/SF/ES/B. Historical OE does
not influence the current result or synthesize IE/PE; C1 is recomputed for the
current conversion. Atomic write-before-state commit, FCW and source/range
limits remain unchanged. Raw binary80 subnormal and pseudo-denormal operands
remain unsupported, and OE admission does not extend to any other x87 gate.

The matrix covers 143,360 combinations across seven admitted source classes,
both signs, five forms, four FCWs, eight TOP values, all 32 sticky exception
combinations and both prior C1 values. Actual FST m32 overflow composition
checks the overflowing sign/rounding cases and all five integer stores; the
float store produces infinity with OE|PE, and integer conversion stores
indefinite with IE while retaining OE|PE and clearing C1. Five-form write/range
fault atomicity, OE plus ZE/SF/ES/B rejection, OE-only exact finite conversion,
invalid re-entry and FNSTSW/FNCLEX are covered. CLI fixtures observe sticky OE
through FISTP/FNSTSW; JIT/native admission remains denied. Physical x86 and native
runtime acceptance remain deferred under ISSUE-019. See the
[stage plan](../plans/STAGE_2.md#s2-31co-admit-sticky-oe-on-bounded-integer-stores),
[report](../reports/STAGE_2_REPORT.md#s2-31co--sticky-oe-on-bounded-integer-stores),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


<a id="local-s2-31cp-admit-historical-sticky-ze-bounded-integer-stores"></a>

## Local S2-31cp: admit historical sticky ZE on bounded integer stores

Only bare `DB 10`, `DB 18`, `DF 10`, `DF 18` and `DF 38` bounded integer
stores admit incoming historical ZE. Their shared FSW gate changes from
`FSW & 0x80c4 == 0` to `FSW & 0x80c0 == 0`, preserving IE/DE/ZE/OE/UE/PE
independently while continuing to reject SF/ES/B. No other x87 gate changes.

ZE by itself does not affect the converted integer, generate a current
exception flag, or retain incoming C1. Existing conversion PE/IE and the
current-operation C1 result are committed only after the atomic Word/Dword/
Qword write. The existing FCW whitelist, seven admitted source/tag classes,
rounding, addressing, canonical-range checks, payload preservation and
FIST/FISTP stack behavior remain unchanged. Binary80 subnormal and
pseudo-denormal integer sources stay unsupported. This is historical-status
admission only: it adds no division-by-zero operation or ZE delivery, and does
not model unmasked exception delivery.

The complete matrix covers 286,720 cases: seven source classes, both signs,
five forms, four admitted FCWs, eight TOP values, 64 IE/DE/ZE/OE/UE/PE
combinations, and both prior C1 states. It checks ZE-only exact conversion,
ZE plus current PE/inexact rounding, and ZE plus current IE/invalid conversion.
Consecutive stores without FNCLEX preserve ZE, then FNSTSW observes and FNCLEX
clears it. Permission and canonical-range faults across all five forms preserve
all six sticky flags, full CPU state, and destination bytes. ZE combined with
SF, ES or B rejects before memory access; unsupported FCWs and source classes
remain rejected. Literal CLI fixtures seed ZE alone and observe it through
FISTP/FNSTSW. JIT/native admission remains closed. Physical x86 acceptance is
deferred to the combined final S2-31 substage. See the
[stage plan](../plans/STAGE_2.md#s2-31cp-admit-historical-sticky-ze-on-bounded-integer-stores),
[report](../reports/STAGE_2_REPORT.md#s2-31cp--historical-sticky-ze-on-bounded-integer-stores),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


<a id="local-s2-31cq-canonical-subnormal-m32-fist"></a>

## Local S2-31cq: canonical binary80 subnormals through m32 `FIST`/`FISTP`

Only exact bare `DB 10` and `DB 18` admit a Special-tagged canonical nonzero
binary80 subnormal: exponent field zero, J=0 and fraction nonzero. The value's
magnitude is less than one half. For nearest-even and toward-zero, store zero;
toward-negative stores `0xffffffff` only for a negative source; toward-positive
stores `1` only for a positive source. Every admitted conversion adds sticky PE
because each source is inexact as an integer. C1 is recomputed and is set iff
the result magnitude is one. No DE, UE, IE, OE, ZE, SF, ES or B flag is newly
generated. Preserve incoming IE/DE/ZE/OE/UE/PE independently under the existing
`FSW & 0x80c0 == 0` gate, and retain C0/C2/C3. FCW remains exactly
`037f/077f/0b7f/0f7f`.

Retain the full canonical48 four-byte destination preflight and exactly one
atomic Dword write without a data read. Commit PE/C1, instruction/data pointers
and RIP only after successful write. FIST preserves TOP and tags; FISTP empties
old physical TOP and advances it after successful write. Preserve the raw
ten-byte ST(0) payload. The source gate remains width-limited: m16/m64,
pseudo-denormals, malformed encodings, mismatched tags, unmasked delivery and
general exception delivery remain unsupported. The class test admits only
exponent-zero, J=0, nonzero-fraction encodings; zero, pseudo-denormal and
unnormal encodings do not pass.

The local matrix covers 65,536 combinations: four fractions (`1`, `2`,
`0x4000000000000000`, `0x7fffffffffffffff`), both signs, four RCs, FIST and
FISTP, eight TOP values, all 64 incoming sticky-flag combinations and both
prior C1 values. Additional checks compose FLD m80 with both stores, reject
wrong tags and noncanonical classes and m16/m64 forms before data access,
preserve full CPU/memory state on write/range faults, and observe PE through
FNSTSW before FNCLEX clears the sticky exception flags. A literal CLI fixture
seeds a negative subnormal and executes FISTP/FNSTSW; the FLD m80 composition
also executes FISTP/FNSTSW/FNCLEX. JIT/native admission remains denied.
Physical x86 and native runtime acceptance remain deferred
under ISSUE-019. See the [plan](../plans/STAGE_2.md#s2-31cq-canonical-subnormal-m32-fist),
[report](../reports/STAGE_2_REPORT.md#s2-31cq--canonical-subnormal-m32-fist),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).

<a id="local-s2-31cr-canonical-subnormal-m16-fist"></a>

## Local S2-31cr: canonical binary80 subnormals through m16 `FIST`/`FISTP`

Only exact bare `DF 10` and `DF 18` admit a Special-tagged canonical nonzero
binary80 subnormal: exponent field zero, J=0, and nonzero fraction. Its
magnitude is strictly less than one half. Nearest-even and toward-zero store
zero; toward-negative stores `0xffff` only for negative input; toward-positive
stores `1` only for positive input. Every conversion adds sticky PE. C1 is
recomputed and is set iff the stored magnitude is one. No DE, UE, IE, OE, ZE,
SF, ES, or B flag is newly generated. Preserve incoming IE/DE/ZE/OE/UE/PE
under the existing `FSW & 0x80c0 == 0` gate, and retain C0/C2/C3. FCW remains
exactly `037f/077f/0b7f/0f7f`.

Retain bare Long64 RAX addressing, the full canonical48 two-byte destination
preflight, and exactly one atomic Word write without a data read. Commit PE/C1,
instruction/data pointers, RIP, and FISTP tag/TOP changes only after a
successful write. FIST preserves TOP and tags; FISTP empties old physical TOP
and advances it. Preserve the raw ten-byte ST(0) payload. The source gate is
width-limited: m64 subnormals, pseudo-denormals, malformed encodings,
mismatched tags, unmasked delivery, and general exception delivery remain
unsupported. The m32 subnormal path remains unchanged.

The local matrix covers 65,536 combinations: four fractions (`1`, `2`,
`0x4000000000000000`, `0x7fffffffffffffff`), both signs, four RCs, FIST and
FISTP, eight TOP values, all 64 incoming sticky-flag combinations, and both
prior C1 values. Additional checks compose FLD m80 with both stores and
FNSTSW/FNCLEX; reject wrong tags, zero/pseudo-denormal/malformed sources,
unsupported FCWs/status, and m64 before data access; verify Word sentinels and
write/range fault atomicity; and exercise literal CLI, JIT denial, and native
admission denial. Physical x86 and native runtime acceptance remain deferred
under ISSUE-019. See the [plan](../plans/STAGE_2.md#s2-31cr-canonical-subnormal-m16-fist),
[report](../reports/STAGE_2_REPORT.md#s2-31cr--canonical-subnormal-m16-fist),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


<a id="local-s2-31cs-canonical-subnormal-m64-fistp"></a>

## Local S2-31cs: canonical binary80 subnormal through `FISTP m64int [RAX]` (`DF 38`)

Only bare `DF 38` admits a Special-tagged canonical binary80 nonzero
subnormal: exponent zero, explicit integer bit J=0, and nonzero fraction.
Every such magnitude is below one half. With FCW `0x037f` (RN) or `0x0f7f`
(TZ), store zero; under `0x077f` (RD), store all-ones only for a negative
source; under `0x0b7f` (RU), store one only for a positive source. Encode
negative one with unsigned two's-complement arithmetic. Every admitted
conversion adds sticky PE, and C1 is set exactly when the stored magnitude is
one. Preserve all incoming sticky exception flags and C0/C2/C3. The existing
`FSW & 0x80c0 == 0` gate still rejects SF/ES/B; the four-FCW whitelist remains.

Keep Long64, exact bare RAX addressing, and the complete canonical48
eight-byte preflight. Issue one atomic Qword write and no data read. Only after
success empty the old physical TOP tag, advance TOP modulo eight, commit PE/C1,
FIP/FDP/RIP and retirement. Preserve all raw x87 payloads, including the
popped source's ten bytes, and unrelated state. No FIST m64 form is added.
Pseudo-denormals, malformed encodings, mismatched tags, unmasked cases,
prefixes, and other integer-store forms remain outside this extension; m16 and
m32 subnormal behavior is unchanged.

Coverage includes four literal fractions × both signs × all RC × all TOP × 64
incoming sticky combinations × both old C1 values (32,768 cases), a literal
FLD m80→FISTP→FNSTSW/FNCLEX fixture sequence, malformed/class/tag/control
rejection, Qword canaries, write/range fault atomicity, TOP wrap, and raw source
preservation. m16/m32 regression checks remain in the workspace. JIT/native
admission stays denied. Native runtime and physical x86 acceptance remain
deferred under ISSUE-019. See the
[plan](../plans/STAGE_2.md#s2-31cs-canonical-subnormal-m64-fistp),
[report](../reports/STAGE_2_REPORT.md#s2-31cs--canonical-subnormal-m64-fistp),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


<a id="local-s2-31ct-pseudo-infinity-m32-fist"></a>

## Local S2-31ct: Special-tagged pseudo-infinity through m32 `FIST`/`FISTP`

Only bare DB 10 and DB 18 admit a Special-tagged binary80 pseudo-infinity:
sign/exponent `0x7fff` or `0xffff`, zero significand, all-one exponent, and
explicit integer bit J=0. Store Dword integer-indefinite `0x80000000`, add
current IE, and clear C1. Do not add current PE or other exception flags.
Preserve historical sticky flags and C0/C2/C3. Keep the current FCW
whitelist, `FSW & 0x80c0` gate, Long64 bare RAX addressing, full canonical48
Dword preflight, and atomic single write/no read. Commit status, FIP/FDP/RIP,
and FISTP's one physical TOP pop only after the write succeeds. FIST retains
TOP. Preserve the raw ten-byte source in either form.

Canonical signed zero and canonical infinity (J=1) remain distinct. Pseudo-
NaNs, wrong tags, m16/m64, prefixes, unmasked or otherwise disallowed status,
and all other forms remain unsupported. FLD m80 does not accept
pseudo-infinity, so no FLD composition is part of this contract.

The 16,384-case matrix crosses both sign/exponent encodings, all four FCWs,
both forms, all TOP positions, 64 sticky combinations and both incoming C1
values. Literal expected values verify IE without new PE, C1 clearing, flag
preservation, source bytes, canaries and TOP wrap. Rejection and fault tests
cover pseudo-NaNs, wrong tags, m16/m64, denied writes and canonical-range
crossing. FNSTSW/FNCLEX and literal CLI observation are covered; JIT/native
admission remains closed and physical x86 validation remains deferred under
ISSUE-019. See the [plan](../plans/STAGE_2.md#s2-31ct-pseudo-infinity-m32-fist),
[report](../reports/STAGE_2_REPORT.md#s2-31ct--special-tagged-pseudo-infinity-through-m32-fist),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


<a id="local-s2-31cu-pseudo-infinity-m16-fist"></a>

## Local S2-31cu: Special-tagged pseudo-infinity through m16 `FIST`/`FISTP`

Only bare DF 10 and DF 18 admit a Special-tagged binary80 pseudo-infinity:
exact sign/exponent `0x7fff` or `0xffff` with a zero 64-bit significand
(J=0). Store Word integer-indefinite `0x8000`, add current IE, and clear C1;
do not add PE or another current exception flag. Preserve historical sticky
flags and C0/C2/C3. Keep the current FCW whitelist, `FSW & 0x80c0` gate,
Long64 bare RAX addressing, full canonical48 Word preflight, and atomic single
write/no read. Commit status, FIP/FDP/RIP, and FISTP's one physical TOP pop
only after the write succeeds; FIST retains TOP. Preserve the raw ten-byte
source in either form.

Canonical signed zero and canonical infinity (J=1) remain separate. Pseudo-
NaNs, wrong tags, m64, prefixes, unmasked or otherwise disallowed status, and
all other forms remain unsupported. Existing m32 pseudo-infinity behavior is
unchanged. FLD m80 does not accept pseudo-infinity, so no FLD composition is
part of this contract.

The 16,384-case ISA matrix crosses both raw sign/exponent values, all four
FCWs, DF10/DF18, all TOP positions, all 64 incoming sticky-flag combinations,
and both prior C1 values. Literal Word bytes and status expectations verify IE
without PE, C1 clearing, C0/C2/C3 and sticky preservation, source payloads,
canaries, and TOP/tag behavior. Rejection and fault tests cover pseudo-NaNs,
wrong tags, DF38, forbidden controls/status, denied writes, and canonical-range
crossing. FNSTSW/FNCLEX and a literal CLI fixture are covered; JIT/native
admission remains closed and physical x86 validation remains deferred under
ISSUE-019. See the
[plan](../plans/STAGE_2.md#s2-31cu-pseudo-infinity-m16-fist),
[report](../reports/STAGE_2_REPORT.md#s2-31cu--special-tagged-pseudo-infinity-through-m16-fist),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


<a id="local-s2-31cw-special-pseudo-nan-m32-fist"></a>

## Local S2-31cw: Special-tagged pseudo-NaNs through m32 `FIST`/`FISTP`

Only bare `DB 10` and `DB 18` admit a Special-tagged binary80 pseudo-NaN
(exponent `0x7fff`, J=0, nonzero fraction). Both Q-bit variants and either sign
store Dword integer-indefinite `0x80000000`, add current IE, and clear C1. They
add no PE or other current flags and preserve historical sticky flags and
C0/C2/C3. The existing FCW whitelist and `FSW & 0x80c0` gate, bare Long64 RAX
addressing, complete canonical48 Dword preflight, atomic one-write/no-read,
write-before-commit order and raw ten-byte payload preservation remain. FIST
retains TOP; FISTP pops once after the write succeeds.

Pseudo-infinity with zero fraction, canonical J=1 QNaNs/SNaNs and non-all-one
exponents remain distinct. At the original S2-31cw boundary, wrong tags,
pseudo-denormals, unnormals, m16/m64 pseudo-NaNs, FLD and unmasked delivery
remained unsupported; S2-31cx later extends only the bounded m16 FIST/FISTP
pseudo-NaN cases below. There is no catch-all unsupported-class branch.
JIT/native admission stays closed, and
physical x86 acceptance remains deferred to the combined final S2-31 substage
under ISSUE-019.

The 65,536-case matrix crosses four literal fractions (`1`,
`0x3fff_ffff_ffff_ffff`, `0x4000_0000_0000_0000`,
`0x7fff_ffff_ffff_ffff`), both signs, four FCWs, both forms, all TOP values,
64 sticky combinations and both prior C1 values. Independent expected Dword
and status values verify IE without PE, C1 clearing, preservation, raw source,
canaries, exact single write/no read and FIST/FISTP stack behavior. Controls
cover pseudo-infinity, canonical QNaN/SNaN, non-all-one exponent, wrong tags,
excluded widths and forbidden controls/status; Dword write and canonical48
range failures preserve state and memory. An ISA FNSTSW/FNCLEX composition
observes and clears IE; a direct seeded CLI fixture observes IE with FNSTSW.
FLD composition is excluded because FLD rejects J=0 pseudo-NaNs. See the
[plan](../plans/STAGE_2.md#s2-31cw-special-pseudo-nan-m32-fist),
[report](../reports/STAGE_2_REPORT.md#s2-31cw--special-tagged-pseudo-nans-through-m32-fist),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


<a id="local-s2-31cx-special-pseudo-nan-m16-fist"></a>

## Local S2-31cx: Special-tagged pseudo-NaNs through m16 `FIST`/`FISTP`

Only bare `DF 10` and `DF 18` admit a Special-tagged binary80 pseudo-NaN
(exponent `0x7fff`, J=0, nonzero fraction). Both Q-bit variants and either
sign store Word integer-indefinite `0x8000`, add current IE, and clear C1. They
add no PE or other current flags and preserve historical sticky flags and
C0/C2/C3. Keep the existing FCW whitelist, `FSW & 0x80c0` gate, bare Long64
RAX address, complete canonical48 two-byte preflight, atomic one-write/no-read,
write-before-commit ordering, and raw ten-byte source preservation. FIST retains
TOP; FISTP pops once only after the write succeeds.

Pseudo-infinity with zero fraction and canonical J=1 QNaN/SNaN keep their
existing m16 behavior. Wrong tags, pseudo-denormals, unnormals, non-all-one
exponents, m64 pseudo-NaNs, FLD and unmasked delivery remain unsupported. The
m32 pseudo-NaN behavior is unchanged; no generic special-class fallback is
added. JIT/native admission remains closed, and physical x86 acceptance remains
deferred to the combined final S2-31 substage under ISSUE-019.

The 65,536-case matrix crosses four nonzero fractions (`1`,
`0x3fff_ffff_ffff_ffff`, `0x4000_0000_0000_0000`,
`0x7fff_ffff_ffff_ffff`), both signs, four FCWs, both forms, all TOP values,
64 sticky combinations and both old C1 values. Independent expectations cover
Word bytes, IE without PE, C1 clearing, preserved status/source, canaries, one
write/no read and FIST/FISTP stack effects. Rejection and fault checks cover
wrong tags, other exponents, pseudo-denormals, unnormals, m64, forbidden
FCW/FSW, denied writes and canonical48 crossing. ISA FNSTSW/FNCLEX and a
seeded CLI fixture observe and clear IE. See the
[plan](../plans/STAGE_2.md#s2-31cx-special-pseudo-nan-m16-fist),
[report](../reports/STAGE_2_REPORT.md#s2-31cx--special-tagged-pseudo-nans-through-m16-fist),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


<a id="local-s2-31cy-special-pseudo-nan-m64-fistp"></a>

## Local S2-31cy: Special-tagged pseudo-NaN through m64 `FISTP`

Only bare `DF 38` admits a Special-tagged binary80 pseudo-NaN with exponent
`0x7fff`, explicit integer bit J=0, and a nonzero fraction. Both values of bit
62, arbitrary remaining nonzero fraction bits and either sign store Qword
integer-indefinite `0x8000000000000000`, add IE, and clear C1. They add no PE or
other new status flags and preserve historical sticky flags and C0/C2/C3.
Retain the FCW whitelist `0x037f`, `0x077f`, `0x0b7f`, `0x0f7f`,
`FSW & 0x80c0 == 0`, bare Long64 RAX addressing, full canonical48 eight-byte
preflight, one atomic Qword write/no read, raw ten-byte source, and FISTP
write-before-commit ordering. Only after the write succeeds does FISTP update
status/FIP/FDP/RIP, empty old physical TOP, and increment TOP exactly once,
including wrap.

Zero-fraction J=0 pseudo-infinity and canonical J=1 infinity, QNaN and SNaN
retain their previous m64 behavior. Wrong tags, pseudo-denormals, unnormals,
other exponents, m16/m32 semantic changes, FLD and unmasked delivery remain
outside this extension. There is no generic special-class fallback. JIT/native
admission remains denied and physical x86/Mac mini acceptance remains deferred
to the combined final S2-31 substage under ISSUE-019.

The 32,768-case matrix crosses four literal fractions (`1`,
`0x3fff_ffff_ffff_ffff`, `0x4000_0000_0000_0000`,
`0x7fff_ffff_ffff_ffff`), both signs, four FCWs, all TOP values, 64 sticky
combinations and both prior C1 values. Independent expected Qword bytes and
status literals verify IE without newly setting PE, C1 clearing, preservation,
raw bytes, canaries, a single write/no read and FISTP TOP/tag wrap. Additional
coverage checks zero/pseudo-infinity and canonical J=1 controls; tag, class,
width, FCW and status rejection; denied-write/canonical48-range atomicity;
FNSTSW/FNCLEX; a direct seeded fixture; and JIT/native denial. Existing m16 and
m32 behavior is covered by their unchanged regression matrices. See the
[plan](../plans/STAGE_2.md#s2-31cy-special-pseudo-nan-m64-fistp),
[report](../reports/STAGE_2_REPORT.md#s2-31cy--special-tagged-pseudo-nan-through-m64-fistp),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


<a id="local-s2-31cz-special-unnormals-m32-fist"></a>

## Local S2-31cz: Special-tagged binary80 unnormals through m32 `FIST`/`FISTP`

Only bare `DB 10` and `DB 18` admit a Special-tagged binary80 unnormal with
biased exponent 1 through `0x7ffe`, explicit integer bit J=0, either sign, and
any fraction including zero. Each conversion stores Dword integer-indefinite
`0x80000000`, adds IE, clears C1, and adds no PE/DE/UE/OE/ZE/SF/ES/B. Preserve
all incoming sticky flags and C0/C2/C3. Retain the FCW whitelist
`0x037f`, `0x077f`, `0x0b7f`, `0x0f7f`, the `FSW & 0x80c0 == 0` gate, bare
Long64 RAX, full canonical48 Dword preflight, one atomic Dword write/no read,
raw ten-byte source preservation, and write-before-commit behavior. FIST keeps
TOP; FISTP empties the old physical TOP and increments TOP only after a
successful write.

Exponent-zero and exponent-`0x7fff` cases retain their existing behavior.
J=1 canonical encodings, pseudo-denormals, wrong tags, m16/m64 forms, FLD and
unmasked delivery are not extended; JIT/native admission remains closed.

Independent literal coverage crosses exponents 1/`0x3fff`/`0x7ffe`, fractions
0/1/`0x4000_0000_0000_0000`/`0x7fff_ffff_ffff_ffff`, both signs, four FCWs,
both forms, all TOP values, 64 historical sticky combinations and both prior
C1 values (196,608 cases). It checks integer-indefinite bytes/status, IE without
PE/DE, preserved flags/source, canaries, one write/no read, and TOP/tag rules.
Rejections/faults cover malformed classes/tags, boundaries, other widths,
forbidden controls/status, denied writes and canonical48 crossing. FNSTSW then
FNCLEX and the seeded CLI fixture observe and clear IE. Physical x86 and Mac
mini acceptance remain deferred to the combined final S2-31 substage under
ISSUE-019. See the [plan](../plans/STAGE_2.md#s2-31cz-special-unnormals-m32-fist),
[report](../reports/STAGE_2_REPORT.md#s2-31cz--special-tagged-unnormals-through-m32-fist),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).

<a id="local-s2-31da-special-unnormals-m16-fist"></a>

## Local S2-31da: Special-tagged binary80 unnormals through m16 `FIST`/`FISTP`

Only bare `DF 10` and `DF 18` admit a Special-tagged binary80 unnormal whose
biased exponent is 1..=`0x7ffe`, explicit integer bit J=0, either sign, and
arbitrary fraction including zero. The conversion writes Word integer-indefinite
`0x8000`, adds IE and clears C1; it adds no PE/DE/UE/OE/ZE/SF/ES/B and preserves
incoming sticky flags and C0/C2/C3. The existing FCW whitelist
`0x037f`, `0x077f`, `0x0b7f`, `0x0f7f`, FSW gate `FSW & 0x80c0 == 0`, bare
Long64 RAX form, complete canonical48 Word preflight, one atomic Word write/no
read, ten raw source bytes and write-before-commit ordering are retained. FIST
keeps TOP; FISTP empties the old physical TOP and increments TOP only after a
successful write.

Exponent zero and `0x7fff` behavior, J=1 encodings, pseudo-denormals and wrong
tags retain their existing behavior. The m32 unnormal path remains as defined
by S2-31cz; m64 unnormal, FLD composition, unmasked delivery and JIT/native
admission remain unsupported. The independent matrix has 196,608 cases (3
exponents × 4 fractions × 2 signs × 4 FCWs × 2 forms × 8 TOP values × 64 sticky
combinations × 2 prior C1 values). Separate checks cover the zero-significand
unsupported-format case, E=0/E=0x7fff and J=1 boundaries, rejection/fault
atomicity, status visibility, raw source bytes, canaries and stack behavior.
Physical x86 and Mac mini
acceptance remain deferred to the combined S2-31 substage under ISSUE-019. See
the [plan](../plans/STAGE_2.md#s2-31da-special-unnormals-m16-fist),
[report](../reports/STAGE_2_REPORT.md#s2-31da--special-tagged-unnormals-through-m16-fist),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).

<a id="local-s2-31db-special-unnormals-m64-fistp"></a>

## Local S2-31db: Special-tagged binary80 unnormals through m64 `FISTP`

Only bare `DF 38` admits a Special-tagged binary80 unnormal whose biased
exponent is 1..=`0x7ffe`, explicit integer bit J=0, either sign, and arbitrary
fraction including zero. The conversion writes Qword integer-indefinite
`0x8000000000000000`, adds IE and clears C1; it adds no PE/DE/UE/OE/ZE/SF/ES/B
and preserves incoming sticky flags and C0/C2/C3. It retains the existing FCW
whitelist `0x037f`, `0x077f`, `0x0b7f`, `0x0f7f`, FSW gate `FSW & 0x80c0 == 0`,
bare Long64 RAX addressing, complete canonical48 Qword preflight, one atomic
Qword write without a data read, ten raw source bytes and write-before-commit
ordering. Success empties the old physical TOP and increments TOP only after the
write. No non-pop m64 form is admitted.

Exponent zero and `0x7fff`, J=1 encodings, pseudo-denormals and wrong tags
retain their existing behavior. m16/m32 unnormal behavior remains defined by
S2-31da/S2-31cz. FLD composition, unmasked delivery, JIT/native admission,
physical x86 and Mac mini acceptance remain outside this slice. The independent
98,304-case matrix covers three finite exponents, four fractions, signs, four
FCWs, all TOP values, 64 sticky combinations and prior C1; separate checks cover
boundaries, tags, controls/status, faults, status visibility and a seeded CLI
fixture. See the [plan](../plans/STAGE_2.md#s2-31db-special-unnormals-m64-fistp),
[report](../reports/STAGE_2_REPORT.md#s2-31db--special-tagged-unnormals-through-m64-fistp),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).

<a id="local-s2-31dt-historical-sticky-ue-for-bounded-m32m64-fld"></a>

## Local S2-31dt: historical sticky UE for exact bounded m32/m64 `FLD`

Only exact bare Long64 `D9 00` (`FLD m32fp [RAX]`) and `DD 00`
(`FLD m64fp [RAX]`) admit historical sticky underflow by changing their
operation-local status mask from `0x80dc` to `0x80cc`. Historical UE is
preserved unchanged. A current ordinary zero/Normal/infinity/QNaN load adds no
IE/DE/PE; a current nonzero subnormal adds DE; a current SNaN adds IE and keeps
the existing binary80 quieting/payload mapping. Current flags combine with
prior IE/DE/UE/PE independently.

The four FCWs remain `0x037f`, `0x077f`, `0x0b7f`, and `0x0f7f`. In the
S2-31dt slice OE still rejected before the single data read; S2-31du admits
historical OE for these two exact forms. ZE/SF/ES/B continue to reject.
Empty physical destination, bare RAX
mapping, full canonical48 range preflight, one raw Dword/Qword read, C1 policy,
TOP/tag push, FIP/FDP, RIP, retirement and write-free load behavior are
unchanged. Failed preflight/read/fetch/boundary conditions remain atomic.
FILD, FLD m80fp, stores, FRNDINT, other operations, JIT and native admission
retain their existing contracts.

Local evidence includes all 16 IE/DE/UE/PE histories, all four FCWs, all
TOPs and prior C1 across both widths and signed zero, Normal boundary/interior,
infinity, QNaN, SNaN and subnormal source classes. Literal UE-only D900/DD00
fixtures observe FSW with FNSTSW. Same- and cross-width underflow-store/load
compositions and FNSTSW/FNCLEX re-entry are included. The independent project
fixtures are not native x86 evidence; general exception delivery and physical
acceptance remain deferred under ISSUE-019. See the [plan](../plans/STAGE_2.md#s2-31dt-admit-historical-sticky-ue-through-exact-binary32binary64-fld), [report](../reports/STAGE_2_REPORT.md#s2-31dt--historical-sticky-ue-for-bounded-m32m64-fld), and [fixture notes](../../tests/fixtures/x86/stage2/README.md).

<a id="local-s2-31du-historical-sticky-oe-for-bounded-m32m64-fld"></a>

## Local S2-31du: historical sticky OE for exact bounded m32/m64 `FLD`

Only exact bare Long64 `D9 00` (`FLD m32fp [RAX]`) and `DD 00`
(`FLD m64fp [RAX]`) lower their operation-local status mask from `0x80cc` to
`0x80c4`. This admits and preserves historical OE alongside IE/DE/UE/PE.
Current ordinary zero/Normal/infinity/QNaN sources add no IE/DE/PE; current
nonzero subnormals add DE; current SNaNs add IE and retain the existing binary80
quieting and payload mapping. ZE/SF/ES/B remain pre-read rejects.

The four FCWs, Empty physical destination, bare RAX mapping, full canonical48
range preflight, one raw Dword/Qword read, C1 policy, TOP/tag push, FIP/FDP, RIP,
retirement and write-free load behavior remain unchanged. Failed
preflight/read/fetch/boundary conditions remain atomic. FILD, FLD m80fp, stores,
FRNDINT, every other x87 gate, JIT and native admission retain their existing
contracts.

Independent full-state interpreter coverage checks 131,072 cases across both
widths, eight signed-zero/Normal-edge/interior/infinity/QNaN/SNaN/subnormal
sources, all four FCWs, all TOPs, both prior-C1 values and all 32
IE/DE/OE/UE/PE histories. It checks complete state, memory/read history and
canaries; ordinary sources add no new flags, current subnormal adds DE, SNaN
adds IE, and C1 clears. Seeded OE-only Normal-source FNSTSW fixtures and
same-width FST overflow → FLD infinity → FNSTSW/FNCLEX project fixtures cover
both widths. The producer compositions use nearest and directed-away RC only;
no OE producer claim is made for toward-zero-directed modes. ZE/SF/ES/B rejects
remain pre-read checks; a schema regression keeps bare budget-one/no-stop
Normal Unsupported fixtures from treating OE-only as a rejection reason.
These are local interpreter/project-fixture results, not native x86 evidence.
General exception delivery, native runtime and physical acceptance remain open
under ISSUE-019. See the [plan](../plans/STAGE_2.md#s2-31du-admit-historical-sticky-oe-through-exact-binary32binary64-fld), [report](../reports/STAGE_2_REPORT.md#s2-31du--historical-sticky-oe-for-bounded-m32m64-fld), and [fixture notes](../../tests/fixtures/x86/stage2/README.md).

<a id="local-s2-30av-canonical-signed-infinity-fchs-fabs"></a>

## Local S2-30av: canonical signed infinity through bounded `FCHS/FABS`

Bare `D9 E0` FCHS and `D9 E1` FABS keep their exact FCW `0x037f` and FSW
`FSW & 0x80ff == 0` gates, and retain canonical-Normal Valid and exact
Zero-tagged signed-zero sources. They additionally admit only the tag/class pair
`(Special, Infinity)` at ST(0), where binary80 has exponent `0x7fff`, integer
bit J=1 and zero fraction, with either sign. FCHS toggles raw bit 7 of byte 9;
FABS clears it. Both preserve Special tag, TOP, C0/C2/C3 and all other state,
clear C1, record FIP, advance RIP by two and retire once. Signaling/pseudo
infinities, NaNs, pseudo-NaNs, subnormals, pseudo-denormals, unnormals, malformed
encodings and wrong tags remain atomically unsupported. No JIT or native gate
is widened.

Local interpreter coverage spans both operations × both signs × all TOP values
× all 16 prior condition-code combinations, compares full machine/x87 state,
checks double-FCHS restoration and FABS idempotence, rejects wrong tags and
neighboring classes, and checks invalid FCW/FSW, missing state, fetch,
truncation, STOP and budget boundaries. m32/m64 FLD → sign operation → FST →
FNSTSW compositions cover both signs. The two handwritten project fixtures use
only `d900d9e0d910dfe0` and `d900d9e1d910dfe0`; a no-stop budget-one fixture
checks wrong-tag rejection, and runner assertions retain JIT denial. These are
local interpreter/project-fixture results, not physical x86 evidence. Physical
and Mac mini acceptance remain open under ISSUE-019. See the
[plan](../plans/STAGE_2.md#s2-30av-admit-canonical-signed-infinity-to-bounded-fchsfabs),
[report](../reports/STAGE_2_REPORT.md#s2-30av--canonical-signed-infinity-through-bounded-fchsfabs),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30av-canonical-signed-infinity-fchsfabs).

<a id="local-s2-30aw-canonical-qnan-fchs-fabs"></a>

## Local S2-30aw: canonical binary80 QuietNaN through bounded `FCHS/FABS`

Bare `D9 E0` and `D9 E1` retain FCW `0x037f`, `FSW & 0x80ff == 0`,
canonical-Normal Valid and exact Zero-tagged signed zero, and Special-tagged
canonical infinity. The shared source gate additionally admits only
`(Special, QuietNaN)` for canonical binary80 QuietNaN: exponent `0x7fff`,
explicit integer bit J=1, quiet bit Q=1, either sign, and any low 62 payload
bits including zero. FCHS toggles only raw sign bit 7 of byte 9; FABS clears
only that bit. Both preserve all remaining ten bytes, the Special tag, TOP,
C0/C2/C3 and other state, clear C1, set FIP, advance RIP by two, and retire
once without adding exception flags. SNaN, pseudo-NaNs, pseudo-infinity,
subnormal, pseudo-denormal, unnormal and wrong tag/class pairs stay
unsupported atomically. Classifier, schema and JIT/native admission are
unchanged.

The local full-state matrix crosses both operations, signs, TOP values, all
16 condition-code combinations and representative low-62 payload values
including both boundaries. Project fixtures use the existing exact m32 FLD
QNaN composition for both operations and input signs; direct interpreter
compositions cover both m32/m64 widths, operations and signs through same-width
FST and FNSTSW. These local results do not establish physical x87 behavior; ISSUE-019
remains open. See the [plan](../plans/STAGE_2.md#s2-30aw-admit-canonical-binary80-quietnan-to-bounded-fchsfabs),
[report](../reports/STAGE_2_REPORT.md#s2-30aw--canonical-binary80-quietnan-through-bounded-fchsfabs),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30aw-canonical-binary80-quietnan-fchsfabs).

<a id="local-s2-30ax-canonical-snan-fchs-fabs"></a>

## Local S2-30ax: canonical binary80 SignalingNaN through bounded `FCHS/FABS`

The shared `FCHS/FABS` source gate additionally admits only
`(Special, SignalingNaN)` for canonical binary80: exponent `0x7fff`, explicit
integer bit J=1, quiet bit Q=0, and nonzero lower 62-bit payload, with either
sign. The J=1/Q=0 zero-payload encoding remains admitted by the existing
canonical-infinity class path. FCHS toggles only raw sign bit 7 of byte 9;
FABS clears only that bit.
Both preserve Q=0, every payload bit, Special tag, TOP, C0/C2/C3 and all other
state, clear C1, set FIP, advance RIP by two, and retire once without adding
IE/PE. Pseudo-NaNs, pseudo-infinity, subnormal,
pseudo-denormal, unnormal, wrong tags and invalid environment stay
unsupported atomically. No decoder, classifier, schema, JIT or native gate
changes.

The full-state interpreter matrix crosses both operations, signs, all TOPs,
all 16 prior condition-code combinations and payloads `{1, 1<<61,
0x0123456789abcdef, (1<<62)-1}`. It checks full state and canaries,
double-FCHS restoration, repeated-FABS idempotence, malformed neighbors,
wrong tags/environment and execution boundaries. A raw `FLD m80fp` → FCHS or
FABS → `FSTP m80fp` → `FNSTSW AX` unit composition checks SNaN bytes after load
and operation and in the stored ten bytes for both signs/operations, without
using m32/m64 conversion. Four exact schema-v2 CLI fixtures seed the raw SNaN
in ST(0) and execute one bare sign operation with budget one; they cover both
signs/operations and verify full state plus JIT denial. A wrong-tag fixture
verifies atomic rejection.
Local interpreter/project-fixture evidence does not establish physical x86
behavior. ISSUE-019 remains open. See the [plan](../plans/STAGE_2.md#s2-30ax-admit-canonical-binary80-signalingnan-to-bounded-fchsfabs),
[report](../reports/STAGE_2_REPORT.md#s2-30ax--canonical-binary80-signalingnan-through-bounded-fchsfabs),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30ax-canonical-binary80-signalingnan-fchsfabs).

<a id="local-s2-30ay-canonical-subnormal-fchs-fabs"></a>

## Local S2-30ay: canonical binary80 subnormal through bounded `FCHS/FABS`

The shared `FCHS/FABS` source gate additionally admits only
`(Special, Subnormal)` for canonical binary80 E=0, J=0 and nonzero 63-bit
fraction, with either sign. Existing FCW `0x037f` and `FSW & 0x80ff == 0`
gates and Normal/zero/infinity/QNaN/SNaN admissions are unchanged. FCHS toggles
only raw sign bit 7 of byte 9; FABS clears only that bit. Both preserve all
fraction bits, Special tag, TOP, C0/C2/C3 and all other state, clear C1, set
FIP, advance RIP by two and retire once without adding DE/UE/PE/IE. E=0,J=1
pseudo-denormals, unnormals, pseudo-special encodings and wrong tags remain
unsupported atomically. No classifier, schema, decode, other gate, JIT or native
admission changes.

The 2,048-case interpreter matrix crosses both operations, both signs, all
TOPs, all 16 prior condition-code combinations and fractions `{1, 1<<62,
0x0123456789abcdef, (1<<63)-1}`. It checks full state and canaries,
double-FCHS restoration, repeated-FABS idempotence, raw fraction preservation,
no new exception flags, malformed neighbors, wrong tags/environment and
execution-boundary atomicity. A raw `FLD m80fp` → FCHS/FABS → `FSTP m80fp` →
`FNSTSW AX` unit composition inspects bytes after load/operation/store for both
signs and operations and verifies DE remains clear. Four exact schema-v2
budget-one fixtures seed canonical raw binary80 subnormals directly in ST(0);
they check both signs/operations and JIT denial. A wrong-tag fixture rejects
atomically. Local interpreter/project-fixture evidence does not establish
physical x86 behavior; ISSUE-019 remains open. See the
[plan](../plans/STAGE_2.md#s2-30ay-admit-canonical-binary80-subnormal-to-bounded-fchsfabs),
[report](../reports/STAGE_2_REPORT.md#s2-30ay--canonical-binary80-subnormal-through-bounded-fchsfabs),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30ay-canonical-binary80-subnormal-fchsfabs).

<a id="local-s2-30bc-historical-ie-fchs-fabs"></a>

## Local S2-30bc: historical sticky IE through bounded `FCHS/FABS`

Only `bounded_x87_sign_st0_source` changes its FSW filter to `0x80fe`: exact
FCW `0x037f` is retained, historical IE (bit 0) is admitted independently of
the source, and DE/ZE/OE/UE/PE/SF/ES/B remain rejected, including when combined
with IE. The same six admitted pairs remain: Valid+Normal, Zero+exact signed
zero, and Special+Infinity/QNaN/SNaN/Subnormal. FCHS toggles only the raw sign
bit; FABS clears only it. Both preserve IE and all other previously preserved
state, clear C1, add no flags, and do not quiet a current SNaN. Other operation
gates, decode, schema and JIT/native admission are unchanged.

Local tests cover 6,144 full-state cases across opcodes/classes/signs/TOP/CC/IE,
forbidden status combinations, invalid environment/tag and execution boundaries,
repeated operations, status observation/clearing, and same-width m32/m64 SNaN
load→sign operation→store→FNSTSW compositions. The four schema-v2 producer
fixtures use m32 SNaNs and check that FLD itself produces QNaN+IE before the sign
operation. Local checks do not establish native or physical x86 behavior;
ISSUE-019 remains open. See the [plan](../plans/STAGE_2.md#s2-30bc-admit-historical-sticky-ie-to-bounded-fchsfabs),
[report](../reports/STAGE_2_REPORT.md#s2-30bc--historical-sticky-ie-through-bounded-fchs-fabs),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30bc-historical-ie-fchs-fabs).

<a id="local-s2-30bd-historical-de-fchs-fabs"></a>

## Local S2-30bd: historical sticky DE through bounded `FCHS/FABS`

Only `bounded_x87_sign_st0_source` changes its FSW filter from `0x80fe` to
`0x80fc`. Exact FCW `0x037f` and the six admitted tag/class pairs remain fixed:
Valid+Normal, Zero+exact signed zero, and Special+Infinity/QNaN/SNaN/Subnormal.
IE and DE are independently admitted and preserved, including together; ZE/OE/
UE/PE/SF/ES/B remain rejected when combined with either or both. FCHS toggles
only raw sign, FABS clears only raw sign, C1 is cleared, and all other retained
state and tag/TOP/CC policy is unchanged. No DE is synthesized by a raw
binary80 subnormal source and no IE is synthesized by a current SNaN.

Coverage includes 12,288 full-state cases, each forbidden status bit combined
with all four IE/DE combinations, bad environment/tag/missing-state and
execution-boundary atomic rejection with DE present, repeated operations, and
FNSTSW/FNCLEX. Binary32 and binary64 FLD subnormal compositions cover both
signs, both operations, minimum/interior/largest values and initial IE clear/set;
they verify Normal+Valid+DE after load, sign-only raw updates, exact same-width
FST output and no new UE/PE. Four schema-v2 m32 producer fixtures plus seeded
DE-only success and combined-status rejection fixtures are local CLI evidence;
JIT denies these forms. Native/JIT and physical behavior remain outside this
slice; ISSUE-019 remains open. See the [plan](../plans/STAGE_2.md#s2-30bd-admit-historical-sticky-de-to-bounded-fchsfabs),
[report](../reports/STAGE_2_REPORT.md#s2-30bd--historical-sticky-de-through-bounded-fchsfabs),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30bd-historical-de-fchs-fabs).

<a id="local-s2-30be-historical-pe-fchs-fabs"></a>

## Local S2-30be: historical sticky PE through bounded `FCHS/FABS`

Only `bounded_x87_sign_st0_source` changes its FSW filter from `0x80fc` to
`0x80dc`. Exact FCW `0x037f` and the same six tag/class pairs remain required:
Valid+Normal, Zero+exact signed zero, and Special+Infinity/QNaN/SNaN/Subnormal.
The gate admits exactly all eight IE/DE/PE combinations and rejects ZE/OE/UE/
SF/ES/B, including when combined with any IE/DE/PE subset. FCHS toggles only
the raw sign bit; FABS clears only it. Both clear C1 and preserve historical
IE/DE/PE, CC/TOP/tag, raw payload, current metadata and retirement behavior.
Decoder/schema/native admission and the JIT deny gate are unchanged.

Coverage includes 24,576 full-state cases, forbidden-status combinations for
all eight historical states, malformed class/tag/raw values, exact FCW, missing
state and fetch/truncation/STOP/budget/fault atomic rejection under PE. FCW
`0x037f` FRNDINT producer tests cover ±1.25→±1, ±1.5→±2 and ±0.5→±0, observing
both C1 states before FCHS/FABS clears C1 while preserving PE. A canonical
binary80 subnormal rounds to Zero-tagged signed-zero raw bytes with full FSW
`0x0022`; FCHS toggles the complete raw sign encoding and FABS returns it to
positive zero, preserving the Zero tag and full FSW `0x0022` after each
instruction. FNSTSW/FNCLEX then observes and clears status. Two exact schema-v2
CLI streams seed ±1.5 and compose FRNDINT,
FCHS/FABS and FNSTSW; seeded PE-only and IE|DE|PE success plus PE|UE and
IE|DE|PE|SF rejection fixtures check status admission and atomicity. JIT still
denies these profiles. Native runtime and physical x86 behavior remain
unverified; ISSUE-019 stays open. See the [plan](../plans/STAGE_2.md#s2-30be-admit-historical-sticky-pe-to-bounded-fchsfabs),
[report](../reports/STAGE_2_REPORT.md#s2-30be--historical-sticky-pe-through-bounded-fchsfabs),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30be-historical-pe-fchs-fabs).

<a id="local-s2-30bg-historical-oe-through-bounded-fchsfabs"></a>

## Local S2-30bg: historical sticky OE through bounded `FCHS/FABS`

Only `bounded_x87_sign_st0_source` changes its FSW mask from `0x80cc` to
`0x80c4`. Exact FCW `0x037f` and the same six accepted source tag/class pairs
remain. The gate preserves every combination of IE/DE/OE/UE/PE; ZE, SF, ES and
B remain forbidden, including with all 32 histories. FCHS toggles only the
binary80 sign bit and FABS clears only it. Both clear C1, preserve all other
status bits, tag/TOP/raw payload, FIP/RIP and retirement semantics, and generate
no flags. Decode/schema/classifier/arithmetic/JIT/native admission is unchanged.

Coverage has 98,304 full-state matrix cases and 3,072 atomic forbidden-status
cases, plus bad FCW/tag/raw/missing-state and fetch/truncation/STOP/budget/fault
rejection with OE. Non-popping FST m32/m64 producer checks use round-to-nearest
midpoint significands `0xffffff8000000000` at exponent 127 and
`0xfffffffffffffc00` at exponent 1023, with midpoint−1, midpoint, midpoint+1
and maximum-significand neighbors, both signs. Midpoint and higher values
produce signed infinity with OE|PE and C1 set; midpoint−1 produces no OE. Full
state and canary memory snapshots verify the Normal+Valid source and memory
remain unchanged by FST/sign processing except the store bytes and sign-bit
transform. Seeded IE/DE/UE/PE are preserved, FNSTSW observes OE|PE with C1
cleared after the sign operation, and FNCLEX clears status. Failed writes and
out-of-range writes are atomic. CLI fixtures cover exact m32/m64 streams, an
OE-only seeded success, and ZE rejects; JIT remains denied. Local checks do not
establish native/runtime or physical x86 behavior. ISSUE-019 remains open. See
the [plan](../plans/STAGE_2.md#s2-30bg-admit-historical-sticky-oe-to-bounded-fchsfabs),
[report](../reports/STAGE_2_REPORT.md#s2-30bg--historical-sticky-oe-through-bounded-fchsfabs),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30bg-historical-oe-through-bounded-fchsfabs).

<a id="local-s2-30bh-historical-ze-through-bounded-fchsfabs"></a>

## Local S2-30bh: historical sticky ZE through bounded `FCHS/FABS`

Only `bounded_x87_sign_st0_source` changes its FSW filter from `0x80c4` to
`0x80c0`. FCW remains exactly `0x037f`; the existing six tag/class pairs are
unchanged. All 64 combinations of IE/DE/ZE/OE/UE/PE are admitted and preserved.
SF, ES and B remain rejected atomically, including when combined with histories.
FCHS toggles only the binary80 sign bit; FABS clears only it. Both clear C1,
preserve TOP, tags, payload, metadata, retirement and all other status, and
generate no exception flags. ZE is seeded explicitly; this slice has no
divide-by-zero producer. Arithmetic, decode, JIT and native gates are unchanged.

The full-state matrix has 196,608 cases and the forbidden-status product has
4,608 atomic rejects. Additional checks cover environment/tag/raw/missing-state
and fetch/truncation/STOP/budget/fault boundaries with ZE, repeated operations,
FNSTSW/FNCLEX, and project CLI fixtures for seeded ZE-only/all-six success and
ZE|SF/all-six|ES rejection. FRNDINT compositions check seeded ZE through
inexact ±1.5/±1.25 rounding, exact ±1/±0, and SNaN quieting. The SNaN becomes
QNaN and IE is produced by FRNDINT before the sign operation preserves ZE|IE.
These are local interpreter/fixture results, not physical x86 or native evidence;
ISSUE-019 remains open for combined final acceptance.

<a id="local-s2-30bi-historical-sf-ie-fchs-fabs"></a>

## Local S2-30bi: historical stack-fault status through bounded `FCHS/FABS`

The shared sign-operation gate preserves the exact FCW `0x037f` and six
existing source tag/class pairs. It accepts all 64 prior IE/DE/ZE/OE/UE/PE
combinations with SF clear, and those same histories with SF set only when IE
is also set. Equivalent predicate: `FSW & 0x8080 == 0 && (FSW & 0x0040 == 0 ||
FSW & 0x0001 != 0)`. Standalone SF and SF without IE reject; ES/B remain
rejected. Sign/payload, C1/TOP/tag, metadata, FIP/RIP and retirement behavior
remain unchanged. The operations generate no flags and do not quiet SNaN.

The full-state matrix retains 196,608 SF-clear cases and adds 98,304 SF|IE
cases (294,912 total). Reject matrices cover 768 SF-without-IE combinations and
6,144 ES/B cases across all 128 low-seven-bit states. Bare project fixtures seed
IE|SF explicitly: `D9 E0` changes +1 to -1 and `D9 E1` changes -1 to +1;
`0x0241` becomes `0x0041` after clearing C1. FRNDINT compositions seed ±1.5 and
`0x0241`; FRNDINT adds PE/C1, FCHS/FABS clears C1, and FNSTSW observes `0x0061`
at TOP=0. No guest stack-fault producer exists in this scope; FRNDINT is only a
composition step. Fixtures are local-only and not native or physical evidence.
Empty sources, SF-only, SF|DE without IE, ES, B, invalid/missing state, bad FCW
and execution boundaries reject atomically. JIT remains denied.
Stepwise ISA coverage asserts the full state after each FRNDINT, FCHS/FABS,
FNSTSW and FNCLEX operation with IE|SF seeded. It covers inexact finite ties
and SNaN→QNaN, where FRNDINT performs quieting and IE handling. FNCLEX clears
SF and all exception/summary bits, and a final FNSTSW reads zero.

Normative reference: [Intel SDM Vol.1, revision 081, §8.1.3.4 p.8-6 and
§8.5.1.1 p.8-26](https://cdrdv2-public.intel.com/789574/253665-sdm-vol-1.pdf#page=208).
ISSUE-019 remains open. Further ES/B relaxation requires a separate
pending-exception contract and final physical x86 acceptance.

<a id="local-s2-30az-canonical-qnan-fst-fstp-sti"></a>

## Local S2-30az: canonical binary80 QuietNaN through bounded `FST/FSTP ST(i)`

Only `bounded_x87_fst_sti_source` is extended. Alongside the existing Valid-tag
canonical Normal, Zero-tagged exact signed zero and Special-tagged canonical
infinity cases, it admits `(Special, QuietNaN)` only when binary80 has E=`0x7fff`,
J=1 and Q=1. Either sign and every low-62-bit payload, including zero, are
accepted as classified by the existing binary80 classifier. FCW remains exactly
`0x037f`, and `FSW & 0x80ff` remains zero. All ten source bytes and the Special
tag are copied unchanged. No exception flags are added.

Bare `DD D0+i` FST writes the captured bytes/tag to physical
`(old_TOP+i)&7`, preserving TOP and the full FSW. Bare `DD D8+i` FSTP copies
first, empties old physical TOP, then advances TOP modulo eight; the `i=0`
payload remains with Empty tag, and `i=1` becomes new ST(0). The established
local C1 policy, FIP/RIP update and single retirement are unchanged. Wrong tags,
SNaN, pseudo-NaN, pseudo-infinity, subnormal, pseudo-denormal, unnormal,
malformed encodings and bad environments reject atomically. Decode, classifier,
schema, other gates and JIT/native admission are unchanged.

The interpreter matrix covers 16,384 full-state cases: both operations, both
signs, all TOP/index pairs, 16 condition-code states and payloads `{0, 1,
1<<61, (1<<62)-1}`. It checks overwritten destination tag/payload sentinels,
alias, `i=1`, TOP wrap and copy-before-pop. Rejections cover neighboring
classes, wrong tags, FCW/FSW, absent x87 state and execution boundaries. Four
existing-schema fixtures exercise m32 QNaN FLD→FST/FSTP ST(1)→FNSTSW for both
signs; direct interpreter compositions cover m32 and m64 for both signs and
store forms. This is local interpreter/project-fixture evidence, not native or
physical x86 validation. ISSUE-019 remains open. See the
[plan](../plans/STAGE_2.md#s2-30az-admit-canonical-binary80-quietnan-to-bounded-fstfstp-sti),
[report](../reports/STAGE_2_REPORT.md#s2-30az--canonical-binary80-quietnan-through-bounded-fstfstp-sti),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30az-canonical-qnan-fst-fstp-sti).


<a id="local-s2-30ba-canonical-snan-fst-fstp-sti"></a>

## Local S2-30ba: canonical binary80 SignalingNaN through bounded `FST/FSTP ST(i)`

Only `bounded_x87_fst_sti_source` is extended. Existing Valid-tag canonical
Normal, Zero-tagged exact signed zero, and Special-tagged canonical infinity
and QNaN remain admitted. The new pair is `(Special, SignalingNaN)` only for
E=`0x7fff`, J=1, Q=0 and nonzero lower-62-bit payload, either sign. The J=1,
Q=0 zero-payload encoding remains canonical Infinity. The classifier and wire
schema do not change. FCW stays `0x037f`; `FSW & 0x80ff` stays zero.

FST copies all ten bytes and the Special tag to physical `(old_TOP+i)&7` and
preserves TOP/FSW. FSTP copies first, empties old physical TOP and increments
TOP modulo eight. For `i=0`, bytes remain with Empty tag; for `i=1`, the copy is
new ST(0). C1, condition-code policy, FIP/RIP and single retirement are
unchanged. No IE/PE is added and Q remains clear. Pseudo-infinity, pseudo-NaNs,
subnormal, pseudo-denormal, unnormal, malformed values, wrong tags and invalid
environments reject atomically; canonical Infinity remains admitted through its
existing path. JIT/native admission remains closed.

Coverage includes a 16,384-case sign/TOP/index/condition/payload full-state
matrix, canaries and destination sentinels, alias/pop/wrap behavior, rejection
boundaries and raw FLD m80fp→FST/FSTP ST(1)→FNSTSW compositions. Four seeded
bare schema-v2 fixtures check both signs and forms at budget one / stop RIP +2;
a wrong-tag fixture checks no state or RIP commit. The wire schema remains
unchanged; the `od-testkit` profile allowlist admits only exact DD D0+i / DD
D8+i one-instruction store streams. m32/m64 SNaN load compositions are
excluded because those loads quiet and raise IE. These are
local interpreter/project-fixture results, not native or physical x86
validation. ISSUE-019 remains open. See the [plan](../plans/STAGE_2.md#s2-30ba-admit-canonical-binary80-signalingnan-to-bounded-fstfstp-sti),
[report](../reports/STAGE_2_REPORT.md#s2-30ba--canonical-binary80-signalingnan-through-bounded-fstfstp-sti),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30ba-canonical-snan-fst-fstp-sti).

<a id="local-s2-30bb-canonical-subnormal-fst-fstp-sti"></a>

## Local S2-30bb: canonical binary80 subnormal through bounded `FST/FSTP ST(i)`

Only `bounded_x87_fst_sti_source` is extended. Alongside the existing Valid-tag
canonical Normal, Zero-tagged exact signed zero and Special-tagged canonical
infinity/QNaN/SNaN pairs, it admits `(Special, Subnormal)` only for E=0, J=0
and a nonzero 63-bit fraction, either sign. FCW remains exactly `0x037f`,
and `FSW & 0x80ff` remains zero. The raw ten bytes and Special tag are
preserved, including the sign and every fraction bit; no DE/UE/PE/IE is added.

Bare FST copies to physical `(old_TOP+i)&7` without changing TOP. Bare FSTP
copies first, empties old physical TOP and increments TOP modulo eight. Thus
`i=0` leaves copied bytes with Empty tag; `i=1` places the copy at new ST(0).
The existing C1, FIP/RIP and retirement policy is unchanged. Zero under Special,
wrong-tag subnormals, pseudo-denormals, unnormals, pseudo-special encodings,
malformed values and invalid environments reject atomically. Classifier,
decoder, schema, other gates and JIT/native admission are unchanged.

The independent 16,384-case state matrix crosses both opcodes, signs, TOP,
indices, all 16 prior condition-code combinations and fractions `{1, 1<<62,
0x0123456789abcdef, (1<<63)-1}`; it checks complete state, physical payload/tag
canaries, aliases, copy-before-pop and TOP wrap. Neighboring-class/wrong-tag,
control/status, missing-state and fetch/truncation/prefix/WAIT/STOP/budget/fault
atomicity are covered. Raw FLD m80fp→FST/FSTP ST(1)→FNSTSW unit compositions
cover both signs and assert byte preservation/no DE. Four seeded bare schema-v2
fixtures run at budget one and stop RIP +2; wrong-tag rejection preserves full
state, and successful fixtures are JIT-denied. The schema remains unchanged.
These local checks do not establish physical x86 behavior. ISSUE-019 remains
open. See [plan](../plans/STAGE_2.md#s2-30bb-admit-canonical-binary80-subnormal-to-bounded-fstfstp-sti),
[report](../reports/STAGE_2_REPORT.md#s2-30bb-canonical-binary80-subnormal-through-bounded-fstfstp-sti),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30bb-canonical-binary80-subnormal-fst-fstp-sti).


<a id="local-s2-30bf-historical-ue-fchs-fabs"></a>

## Local S2-30bf: historical sticky UE through bounded `FCHS/FABS`

Only `bounded_x87_sign_st0_source` changes its FSW filter from `0x80dc` to
`0x80cc`. Exact FCW `0x037f` and the six accepted tag/class pairs remain. The
gate admits all 16 IE/DE/UE/PE combinations and rejects ZE/OE/SF/ES/B combined
with every such history state. FCHS/FABS preserve historical flags and all
existing raw/tag/CC/TOP/pointer/retirement invariants, clear C1, and add no
exceptions. Raw binary80 subnormal sources still do not set DE. Decode,
classifier, arithmetic, JIT and native gates are unchanged.

Local coverage includes 49,152 full-state cases and forbidden-bit products,
plus m32/m64 non-popping FST underflow producers for signed zero and minimum
subnormal. Producer assertions inspect UE|PE and C1 immediately after FST,
verify exact destination bytes and unchanged Normal+Valid binary80 source, then
verify FCHS/FABS changes only the sign and preserves the already-produced
status. CLI fixture streams cover seeded status preservation and atomic
rejection; JIT remains denied. Native/runtime and physical x86 behavior remain
unverified; ISSUE-019 stays open. See the [plan](../plans/STAGE_2.md#s2-30bf-admit-historical-sticky-ue-to-bounded-fchsfabs),
[report](../reports/STAGE_2_REPORT.md#s2-30bf--historical-sticky-ue-through-bounded-fchs-fabs),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30bf-historical-ue-fchs-fabs).

## Local S2-30bj: bare `FXCH ST(1)` (`D9 C9`)

The local slice admits only exact no-prefix Long64 bytes `D9 C9`. It requires
present x87 state, FCW `0x037f`, and `FSW & 0x80ff == 0`; both physical
`TOP` and `(TOP+1)&7` must independently carry Valid tags and canonical-Normal
raw binary80 encodings. Both checks complete before either register changes.
Execution exchanges the complete ten-byte physical payloads. It preserves both
tags, TOP, and C0/C2/C3 under a deterministic local policy; the latter are
architecturally undefined and this is not a hardware-equality claim. It clears
C1, adds no exception flags, and performs no push, pop, arithmetic, rounding,
or stack-underflow response. FIP records the old RIP; FDP/FOP follow the
register-only pointer policy. RIP advances by two, one instruction retires,
and GPR, RFLAGS, and memory are unchanged.

Empty/Zero/Special or otherwise mismatched tags, any non-Normal or malformed
raw encoding, any FCW other than `0x037f`, any set bit in FSW mask `0x80ff`, or
missing x87 state reject without partial swap or metadata/RIP changes. This was
the original S2-30bj `i=1` slice; the indexed extension is specified below.

## Local S2-30bk: bounded bare `FXCH ST(i)` (`D9 C8+i`)

The local indexed extension admits exact no-prefix Long64 bytes `D9 C8+i` for
`i=0..7`. It uses the same FCW/status gate, canonical-Normal Valid-tag checks,
and atomic raw ten-byte exchange policy as S2-30bj. Physical source is
`(TOP+i)&7`; `i=0` has only one participant, so it validates that entry once,
exchanges it with itself, clears C1, records FIP, advances RIP by two, and
retires normally. Empty, malformed, or non-Normal unused cells do not affect
the self form. TOP, tags, C0/C2/C3, FDP/FOP, GPR, RFLAGS and memory retain the
prior local policy. No hardware-equality claim is made for undefined CC values.

Preflight checks every participating operand before mutation. All ten raw bytes
are swapped for nonzero indices; no push, pop, rounding, arithmetic, stack
exception, IE/SF, #MF/#NM/WAIT, JIT, or native behavior is added. Schema-v2
permits only the eight individual encodings, each optionally followed by
`DF E0`, or two repetitions of the same encoding followed by `DF E0`.
Validation covers 14,336 multi-index plus 1,024 self-exchange success states,
malformed unused-cell canaries, participating-operand/environment rejection,
decoder and instruction-boundary behavior, and literal project fixtures.
ISSUE-019 remains open for excluded x87 classes/exceptions and physical/native
acceptance. See the [plan](../plans/STAGE_2.md#s2-30bk-expand-bounded-fxch-from-i1-to-all-register-indices),
[report](../reports/STAGE_2_REPORT.md#s2-30bk--expand-bounded-fxch-to-all-indices),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30bk-bounded-fxch-sti).

## Local S2-30bl: canonical signed zero through bounded `FXCH ST(i)`

This extension retains exact bare Long64 `D9 C8+i`, FCW `0x037f`, and
`FSW & 0x80ff == 0`. A physical participant is accepted as either a
Valid-tagged canonical-Normal binary80 value or a Zero-tagged exact signed
zero: all 64 significand bits and the 15 exponent bits are zero, with either
sign bit. A Valid-tagged zero, malformed Zero payload, Empty/Special tag,
subnormal, pseudo-denormal, unnormal, infinity, or NaN remains unsupported.

For `i!=0`, physical `(TOP+i)&7` is preflighted together with TOP before
mutation; on success, the complete ten-byte payloads and their physical two-bit
tags exchange atomically. This lets Zero↔Normal and Zero↔Zero exchange without
reclassifying or normalizing bytes. For `i=0`, only TOP is inspected and its
payload and tag are preserved; all other physical entries, including malformed
ones, are ignored. TOP is unchanged, C1 clears, no flags are added, and C0/C2/C3
remain under the existing deterministic local policy. FIP/RIP and all other
state follow S2-30bk; no arithmetic, rounding, push/pop, stack-fault response,
JIT, or native behavior is added.

The v2 stream allowlist remains unchanged. Coverage retains S2-30bk's 15,360
Normal-only states and adds 32,256 indexed mixed states (nine ordered magnitude
pairs, four sign combinations, seven nonzero indices, eight TOP values, and
sixteen prior condition-code patterns) plus 256 self-Zero cases. Full-state
assertions include raw payloads, complete tag words, both Zero/Normal transfer
orders, opposite signed-zero transfer, wrap, double exchange, and untouched
canaries. Malformed Zero, wrong tag, Empty/Special, Valid-tagged zero, other
unsupported encodings, environment, instruction-fetch, STOP, and budget cases
reject atomically. Schema validation permits canonical Zero participants and
continues to reject malformed Zero; JIT remains explicitly unsupported.

The instruction form is described by Intel SDM rev.093 Vol.2A, FXCH entry,
p.3-407; physical tag classification is Intel SDM rev.081 Vol.1 §8.1.7. This
local extension makes no hardware-equality claim for undefined C0/C2/C3.
ISSUE-019 remains open for Empty/stack faults, special classes, and physical
acceptance. See the [plan](../plans/STAGE_2.md#s2-30bl-admit-canonical-signed-zero-to-bounded-fxch),
[report](../reports/STAGE_2_REPORT.md#s2-30bl--canonical-signed-zero-through-bounded-fxch),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30bl-canonical-signed-zero-fxch).

## Local S2-30bm: bounded register-only `FINCSTP` / `FDECSTP`

Only exact bare Long64 `D9 F7` FINCSTP and `D9 F6` FDECSTP are admitted. The
profile requires present x87 state, FCW `0x037f`, and `FSW & 0x80ff == 0`.
FINCSTP updates TOP to `(old TOP+1)&7`; FDECSTP updates it to
`(old TOP+7)&7`. Both clear C1. C0/C2/C3 are architecturally undefined for
these operations; this implementation preserves them as a deterministic local
policy and makes no hardware-equality claim.

All eight raw register cells and the complete tag word remain byte-for-byte
unchanged. Payloads and tags are opaque, including Empty, Special, mixed, and
mismatched states; execution does not classify or inspect them. The operations
raise no new floating exception/status flags. FIP records old RIP, RIP advances
by two and one instruction retires. FDP/FOP follow the register-only policy;
GPR, RFLAGS and memory remain unchanged. Missing state, bad FCW, any status bit
in `0x80ff`, prefixes, neighbors, and instruction-boundary failures reject
without partial state changes. ES/B and #MF/#NM/WAIT/pending-exception/IE/SF
semantics remain unsupported.

The v2 schema admits exactly `d9f7`, `d9f6`, `d9f7dfe0`, `d9f6dfe0`,
`d9f7d9f6dfe0`, and `d9f6d9f7dfe0`. Local coverage comprises 1,536 full-state
matrix cases plus repeats, inverse pairs, status-gate and boundary rejection,
FXCH composition under adjusted TOP, and four literal status-observation
fixtures. JIT/native execution and physical x86 behavior are not established;
ISSUE-019 remains open. See the [plan](../plans/STAGE_2.md#s2-30bm-bounded-register-only-fincstp--fdecstp),
[report](../reports/STAGE_2_REPORT.md#s2-30bm--bounded-register-only-fincstp-and-fdecstp),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30bm-bounded-fincstpfdecstp).

### Local S2-30bn: bounded FLD1 and FLDZ constant pushes

The interpreter admits exactly bare Long64 `D9 E8` and `D9 EE`. It requires present `X87State`, `FCW=0x037f`, `FSW & 0x80ff == 0`, and an Empty physical push destination `(old_TOP+7)&7`. Successful FLD1 writes exact binary80 `0000000000000080ff3f` and marks Valid; FLDZ writes ten zero bytes and marks Zero. Both set TOP to the destination, clear C1, record FIP as old RIP, advance RIP by two and retire once. The old payload at the Empty destination is never inspected; other seven payloads/tags are unchanged. FSW C0/C2/C3 retention and FDP/FOP preservation are local deterministic policies, not hardware-equality claims. An occupied destination of any tag rejects atomically; masked stack overflow, IE/SF, indefinite, partial pushes, rounding, memory, flags, #NM, WAIT, JIT and native admission are not modeled. Four exact schema allowlist byte streams are covered; an additional project fixture reuses bare `D9 E8` to verify occupied-destination rejection with zero retirement. This local interpreter evidence does not close ISSUE-019 or establish physical x86 behavior.


### Local S2-30bo: bounded bare `FLD ST(0)` (`D9 C0`)

Only exact unprefixed Long64 `D9 C0` is admitted; `D9 C1..C7` remain unsupported. The profile requires present x87 state, FCW `0x037f`, `FSW & 0x80ff == 0`, old physical TOP with either a Valid tag plus canonical-Normal binary80 or a Zero tag plus exact signed binary80 zero, and physical `(old_TOP+7)&7` tagged Empty. Both entries are preflighted before mutation. Execution captures the old physical TOP's ten bytes/tag, copies them to the Empty destination, leaves the old source unchanged (now logical ST(1)), and then sets TOP to the destination. Stale destination bytes are opaque. C1 clears; no new flags are added; FIP is old RIP and RIP advances by two with one retirement. GPR/RFLAGS/memory and remaining x87 state follow the register-only policy. C0/C2/C3 and FDP/FOP retention are deterministic local policy without a hardware-equality claim.

Empty/Special/malformed/wrong-tag sources, every occupied destination tag, bad FCW, any masked FSW bit, missing x87 state, prefixes/neighbors and fetch/truncation/STOP/budget/fault boundaries reject atomically. Stack overflow/underflow response, IE/SF, indefinite, #MF/#NM/WAIT, JIT/native acceptance and physical behavior are not modeled. Local coverage includes 5,120 full-state successes, seven repeated pushes then atomic eighth rejection, exact producer compositions, raw bytes/tag-word checks and literal fixtures. The schema allowlist is exactly `d9c0`, `d9c0dfe0`, `d9e8d9c0dfe0`, and `d9eed9c0dfe0`. ISSUE-019 remains open. See the [plan](../plans/STAGE_2.md#s2-30bo-bare-fld-st0-bounded-raw-copy-push-slice), [report](../reports/STAGE_2_REPORT.md#s2-30bo--bounded-fld-st0), and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30bo-bounded-fld-st0).


### Local S2-30bp: bounded bare `FLD ST(i)` (`D9 C0..C6`)

The interpreter admits only exact unprefixed Long64 `D9 C0..C6`; `D9 C7`, prefixes, and other neighboring encodings remain unsupported. For source index `i`, it preflights physical source `(old_TOP+i)&7` and push destination `(old_TOP+7)&7` before mutation. The source must be Valid/canonical-Normal or Zero/exact signed zero, and destination must be Empty; source bytes/tag are captured and copied exactly, source remains unchanged, and bytes under the Empty destination are opaque. TOP moves to destination, C1 clears, FIP records old RIP, and RIP advances two bytes with one retirement. FCW is exactly `0x037f`, `FSW & 0x80ff` must be zero, and missing state/invalid classes/tags/environment reject atomically. C0/C2/C3 and FDP/FOP remain deterministic local policies with no hardware-equality claim. Stack exceptions, IE/SF, indefinite, #MF/#NM/WAIT, JIT/native and physical behavior remain outside scope.

Local coverage has a 35,840-case full-state matrix (seven indices × five magnitudes × two signs × eight TOP values × sixteen prior CC patterns × four canary snapshots), rejection/boundary tests, and literal fixture evidence for every newly admitted index, signed zero/wrap, nonadjacent source with Empty ST(0), wrong source tag, and occupied destination. The exact schema allowlist is seven `D9 C0..C6` streams, their seven optional `FNSTSW AX` forms, and the existing FLD1/FLDZ producer streams with one indexed load and FNSTSW. JIT/native/physical checks remain deferred to the final shared physical substage; ISSUE-019 remains open. See the [plan](../plans/STAGE_2.md#s2-30bp-bare-fld-sti-i06-bounded-raw-copy-push-slice), [report](../reports/STAGE_2_REPORT.md#s2-30bp--bounded-fld-sti), and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30bp-bounded-fld-sti).


### Local S2-30bq: bounded bare `FFREE ST(0)` (`DD C0`)

The interpreter admits only exact unprefixed Long64 `DD C0`. It requires present x87 state, FCW `0x037f`, and `FSW & 0x80ff == 0`. The old physical TOP tag and payload are opaque; the only x87 data change is setting that physical tag to Empty, including an idempotent Empty-to-Empty case. TOP, all eight ten-byte raw payloads, other tags, C0..C3 including C1, FCW/FSW, FDP/FOP, GPR, RFLAGS and memory are preserved under the local policy. FIP records old RIP, RIP advances by two and the instruction retires once. No exception/pending/native/JIT behavior is added. `DD C1..C7`, prefixes, memory forms and neighboring encodings remain unsupported.

Local evidence comprises the 3,072-case TOP/CC/tag/payload matrix, environment/decode/boundary atomicity, exact schema streams `ddc0`, `ddc0dfe0`, `ddc0d9c0dfe0`, and `ddc0d9f7d9e8dfe0`, plus literal fixtures including TOP=7 with C1 set, already Empty, opaque SNaN/malformed payload, and `FFREE→FINCSTP→FLD1` reuse. These are interpreter/schema results only; ISSUE-019 remains open and native/JIT/physical acceptance is not established. See [plan](../plans/STAGE_2.md#s2-30bq-bare-ffree-st0-bounded-tag-only-free-slice), [report](../reports/STAGE_2_REPORT.md#s2-30bq--bounded-ffree-st0), and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30bq-bounded-ffree-st0).

### Local S2-30br: bounded bare `FFREE ST(i)` (`DD C0..C7`)

The exact unprefixed Long64 forms `DD C0..C7` select physical
`(old_TOP+i)&7`. Require x87 state, FCW `0x037f`, and
`FSW & 0x80ff == 0`. The selected prior tag and ten-byte payload are opaque;
success changes only that tag to Empty and records FIP. Preserve TOP, all
payloads and other tags, complete FCW/FSW including C0..C3, FDP/FOP, GPR,
RFLAGS and memory. RIP advances by two and one instruction retires. Repeating
the instruction on Empty remains accepted. No classification, payload change,
pop, exception, pending state or native/JIT admission is modeled.

The full-state interpreter matrix has 24,576 cases (eight indices, eight TOPs,
sixteen CC patterns, four tags and six opaque payloads), retaining the S2-30bq
3,072-case ST(0) matrix. Decoder and schema checks cover all eight forms; the
schema allows exactly nineteen indexed FFREE streams, including the specified
`ST(7)→FLD1` reuse and the existing `ST(0)→FLD ST(0)` atomic rejection. Seven
literal indexed fixtures and an environment rejection fixture exercise CLI
snapshots and JIT denial. These are local interpreter/schema results only;
ISSUE-019 remains open, and native/JIT/physical x86 behavior is not established.
See [plan](../plans/STAGE_2.md#s2-30br-generalize-bounded-ffree-to-all-sti-indices),
[report](../reports/STAGE_2_REPORT.md#s2-30br--bounded-ffree-sti), and
[fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30br-bounded-ffree-sti).

### Local S2-30bs: bounded bare `FNOP` (`D9 D0`)

Only exact unprefixed Long64 `D9 D0` is admitted, and only when x87 state is
present, FCW is exactly `0x037f`, and `FSW & 0x80ff == 0`. No stack tag,
TOP-dependent cell, or raw binary80 payload is inspected. Success preserves
the complete x87 snapshot except for FIP, which becomes old RIP; this includes
TOP, FTW, all eight raw 80-bit values, FCW, complete FSW and C0..C3 (including
the local C1 undefined-field policy), FDP and FOP. GPR, RFLAGS, MXCSR, pending
state, and data memory are unchanged. RIP advances by two and one instruction
retires. No exception or pending-state behavior is added.

The local matrix covers 4,608 combinations of TOP, condition-code pattern, FTW
pattern and raw payload layout. The decoder rejects every prefix, `9B D9 D0`,
`D9 D1..D7`, and truncation while retaining existing neighboring D9 forms.
Strict-gate rejects, fetch/truncation, STOP and budget boundaries leave state
unchanged. Exact schema streams are `d9d0`, `d9d0dfe0`,
`d9d0d9d0dfe0`, and `ddc7d9d0d9e8dfe0`; environment rejection is only bare
`d9d0` with budget one. Literal fixtures and the runner verify opaque TOP=7 / C1
state, all-Empty state, repeated FNOP, FFREE→FNOP→FLD1 composition, forbidden
ES|B, invalid FCW, and JIT denial. This is local interpreter evidence only;
native and physical x86 behavior remain unverified under ISSUE-019.
# Local S2-30bt: bounded bare FXAM (`D9 E5`)

The local Long64 interpreter admits only unprefixed `D9 E5` when x87 state is present, FCW is `0x037f`, `FSW & 0x80ff == 0`, and physical old TOP contains either a Valid-tagged canonical Normal binary80 value or a Zero-tagged exact signed zero. Normal yields CC `0x0400`; zero yields `0x4000`; C1 is additionally set to the operand sign. All four condition-code bits are replaced with `(old_FSW & !0x4700) | cc`. The remaining status/control words, TOP, tags, eight raw cells, FDP/FOP and all non-x87 machine state are preserved. FIP receives old RIP; RIP advances by two and retirement increments once.

All other tags/classes, malformed payloads and environment failures reject without mutation. Empty and Special FXAM encodings, exception delivery, pending state, JIT and native execution are outside this contract. This local slice does not change the separate FTST source gate and does not establish physical x86 behavior. See [S2-30bt plan](../plans/STAGE_2.md#s2-30bt-bounded-bare-fxam-normal-and-signed-zero-classification), [report](../reports/STAGE_2_REPORT.md#s2-30bt--bounded-bare-fxam), and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30bt-bounded-fxam).

## Local S2-30bu: bounded `FCMOVB ST(0), ST(1)` (`DA C1`)

Admit only exact unprefixed Long64 `DA C1` with present x87 state, FCW
`0x037f`, and `FSW & 0x82ff == 0`. Before reading RFLAGS.CF, validate both
physical old TOP and `(old_TOP+1)&7`: each must be Valid-tagged canonical
Normal, or Zero-tagged exact positive/negative binary80 zero. This applies on
the untaken path too. If CF=1, copy the source's exact ten bytes and tag to
old TOP; if CF=0, preserve both values and tags. Preserve TOP, the entire FSW
(including its required C1=0), FCW, other x87 fields, and all remaining machine
state. Record FIP=old RIP, advance RIP by two, and retire once. No memory access,
pop, x87 exception delivery, JIT, or native execution is included. Keep
`DA E9` FUCOMPP unchanged. This local profile intentionally restricts incoming
C1 to zero even though the instruction does not define FSW.C1; it does not
claim a physical value for that undefined bit.

Success tests cover 51,200 full-state cases: ten admitted values (minimum
Normal, 1, 1+ULP and maximum finite, each with both signs, plus exact ±0),
all destination/source pairs, both CF values, eight TOPs, eight C0/C2/C3
patterns and four PF/ZF patterns.
Rejections cover 21,504 class/tag cases (42 nonadmitted class/tag pairs, sign,
CF, participant position, TOP and C0/C2/C3 patterns), plus environment and
boundary cases. The schema permits exactly `dac1`, `dac1dfe0`,
`dac1dac1dfe0`, `d9e8d9e0dbf1dac1dfe0`, and
`d9e8d9e0d9e8dbf1dac1dfe0`, with budgets 1, 2, 3, 6 and 6. Literal fixtures
cover taken/untaken tag preservation, exact negative-zero copy, TOP wrap,
repeat, both FLD1/FCHS/FCOMI producers, source rejection with CF=0, destination
rejection, C1=1 and bad FCW. Native/JIT remain denied; physical acceptance is
deferred under ISSUE-019.

This S2-30bu ST(1)-only scope is historical and is superseded for register
source indices by [S2-30bv](#local-s2-30bv-bounded-indexed-fcmovb-sti-da-c0c7).

## Local S2-30bv: bounded indexed `FCMOVB ST(0), ST(i)` (`DA C0..C7`)

Admit only exact unprefixed Long64 `DA C0..C7`. The low ModR/M bits are the
source index `i`, and the physical source is `(old_TOP+i)&7`. Preserve the
S2-30bu environment gate: x87 state is present, FCW is exactly `0x037f`, and
`FSW & 0x82ff == 0`. Before reading CF, validate old physical TOP and the
indexed source for `i>0`, even when CF is clear. Each inspected participant
must be Valid-tagged canonical Normal or Zero-tagged exact signed binary80
zero. For `i=0`, TOP is the only inspected slot; no tag or payload from ST(1)
or another slot may be consulted. Both self-index branches leave that slot's
raw bytes and tag unchanged. For CF=1 and `i>0`, copy the exact source bytes
and tag to old TOP; CF=0 preserves both. Preserve TOP, complete FSW, FCW,
other x87 fields and all other machine state except FIP=old RIP, RIP+2 and one
retirement. C1=1 remains unsupported. Keep `DA E9` FUCOMPP unchanged. There is
no memory, pop, exception delivery, JIT or native execution in this profile.

The interpreter matrices cover 363,520 successful full-state vectors: 51,200
per index for `i=1..7`, plus 5,120 self-index vectors. Invalid operand/tag
coverage is 161,280 vectors: 21,504 per nonzero index and 10,752 self-index
vectors, including invalid participants when CF=0. Per-index environment and
execution boundary checks are also local tests. The schema admits only eight
bare DA C0..C7 streams, their eight FNSTSW-terminated forms, the S2-30bu repeat,
and its two producer streams (19 streams total). Literal fixtures cover each
index, source/tag preservation, signed zero, TOP wrap, self-index with opaque
unrelated registers, an invalid indexed source with CF=0 and indexed env
rejection. These results do not resolve full x87 semantics or physical/native
acceptance; ISSUE-019 remains open. See [plan](../plans/STAGE_2.md#s2-30bv-indexed-bare-fcmovb-st0-sti),
[report](../reports/STAGE_2_REPORT.md#s2-30bv--bounded-indexed-fcmovb-sti), and
[fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30bv-bounded-indexed-fcmovb-sti).

## Local S2-30bw: bounded `FCMOVNB ST(0), ST(1)` (`DB C1`)

Admit only exact unprefixed Long64 `DB C1`. Require present x87 state, FCW
`0x037f`, and `FSW & 0x82ff == 0`. Before reading RFLAGS.CF, preflight both
physical old TOP and `(old_TOP+1)&7`; each must be Valid-tagged canonical
Normal or Zero-tagged exact signed binary80 zero, including when CF=1 and the
move is untaken. For CF=0, copy the source's exact ten bytes and tag to old
TOP; for CF=1, preserve both operands. Preserve TOP, complete FSW, FCW,
remaining x87 state, RFLAGS, GPRs and memory. Set FIP to old RIP, advance RIP
by two and retire once. No memory form, exception delivery, native execution
or JIT lowering is included. Existing DB forms and `DA C0..C7` FCMOVB remain
unchanged.

The local success matrix contains 51,200 snapshots across ten admitted values,
both CF outcomes, all TOP values, C0/C2/C3, and PF/ZF. Invalid class/tag
coverage contains 21,504 snapshots (42 invalid pairs, signs, both CF values,
both operand positions, eight TOPs and eight condition combinations). The
schema accepts exactly `dbc1`, `dbc1dfe0`, `dbc1dbc1dfe0`,
`d9e8d9e8d9e0dbf1dbc1dfe0`, and `d9e8d9e0d9e8dbf1dbc1dfe0`, with budgets
1/2/3/6/6. Native/JIT remain closed; physical behavior remains unverified
under ISSUE-019. See [plan](../plans/STAGE_2.md#s2-30bw-bare-fcmovnb-st0-st1),
[report](../reports/STAGE_2_REPORT.md#s2-30bw--bounded-fcmovnb-st0-st1), and
[fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30bw-bounded-fcmovnb-st0-st1).

The former DB C1-only register extent is historical and superseded by
[S2-30bx](#local-s2-30bx-bounded-indexed-fcmovnb-st0-sti-db-c0c7).

## Local S2-30bx: bounded indexed `FCMOVNB ST(0), ST(i)` (`DB C0..C7`)

Admit exact unprefixed Long64 `DB C0..C7`; the low ModR/M bits select `i`.
Require present x87 state, FCW `0x037f`, and `FSW & 0x82ff == 0`. For `i>0`,
preflight physical old TOP and `(old_TOP+i)&7` before reading RFLAGS.CF,
including the untaken path. For `i=0`, validate only old TOP; unrelated tags
and payloads remain opaque. Every required operand is Valid-tagged canonical
Normal or Zero-tagged exact signed binary80 zero. CF=0 copies ten raw source
bytes and the source tag into TOP; CF=1 preserves payloads and tags. Preserve
all other state except FIP, RIP and retirement. Existing FCMOVB remains
unchanged. Memory forms, exception delivery, native execution and JIT are
excluded.

Local interpreter coverage contains 363,520 success snapshots comparing full
x87 state, RFLAGS and RIP, and 161,280 invalid class/tag snapshots comparing
complete machine state across all indices. Independent expected-state checks
and the CF-complement property are included. Schema acceptance is
limited to 19 exact streams; eight literal fixtures cover DB C0..C7 and the
runner checks snapshots while denying JIT. No physical x86 evidence is
claimed; ISSUE-019 remains open. See [plan](../plans/STAGE_2.md#s2-30bx-indexed-bare-fcmovnb-st0-sti),
[report](../reports/STAGE_2_REPORT.md#s2-30bx--bounded-indexed-fcmovnb-st0-sti), and
[fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30bx-bounded-indexed-fcmovnb-sti).

## Local S2-30bz: bounded indexed bare `FCMOVE ST(0), ST(i)` (`DA C8..CF`)

Admit exact unprefixed Long64 `DA C8..CF`; the low ModR/M bits select `i`.
Require present x87 state, FCW `0x037f`, and `FSW & 0x82ff == 0`. For `i>0`,
preflight physical old TOP and `(old_TOP+i)&7` before consulting integer
`RFLAGS.ZF`, including when ZF=0. For `i=0`, validate only old TOP and do not
inspect unrelated physical slots. Every required participant must have a
Valid tag and canonical Normal binary80 value, or a Zero tag and exact signed
binary80 zero. When ZF=1, copy the source's exact ten raw bytes and tag into
old physical TOP; when ZF=0, preserve operands. Preserve TOP, complete FSW and
FCW, RFLAGS, GPRs, memory and all other state. Set FIP to old RIP, advance RIP
by two and retire once. FSW.C3 is not the condition; C1=1 remains closed under
the environment gate while other C0/C2/C3 values are preserved. Existing
FCMOVB and FUCOMPP behavior is unchanged. Memory forms, exception delivery,
JIT, native execution and physical acceptance are outside this bounded
contract.

The interpreter checks 363,520 full-state successes (51,200 for each nonzero
index and 5,120 for self-index) and 161,280 atomically rejected class/tag
cases (21,504 per nonzero index and 10,752 for self-index). The self-index
cases do not inspect a second physical slot. Additional checks cover forbidden
status bits, invalid FCW, missing x87, per-index prefix/decode, fetch,
truncation, STOP and budget boundaries. The exact schema admits 19 streams:
all eight bare encodings, their eight `DF E0` observation forms, repeat, and
two producer streams. Literal fixtures cover indices 0 and 2..7 (the existing
ST(1) fixtures cover index 1), both self-index ZF outcomes, TOP wrap, raw/tag
copying, signed zero, invalid source with ZF=0 and indexed environment
rejection. Runner assertions compare complete x87 snapshots for all success
fixtures, including the second repeat transfer, and deny JIT. Evidence is
local interpreter/schema/runner evidence only; ISSUE-019 remains open. See
[plan](../plans/STAGE_2.md#s2-30bz-bare-fcmove-st0-sti-da-c8cf), [report](../reports/STAGE_2_REPORT.md#s2-30bz--bounded-indexed-fcmove-st0-sti), and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30bz-bounded-indexed-fcmove-st0-sti).

## Local S2-30ca: bounded bare `FCMOVNE ST(0), ST(1)` (`DB C9`)

Historical ST(1)-only coverage; superseded by indexed S2-30cb.

The exact unprefixed Long64 `DB C9` form validates x87 state, FCW `0x037f`,
`FSW & 0x82ff == 0`, and both old physical TOP and TOP+1 as Valid canonical
Normal or Zero exact signed binary80 before consulting integer ZF. ZF=0 copies
the source's ten raw bytes and tag to TOP; ZF=1 preserves both cells. C1=1
remains unsupported. All other state is preserved except FIP=old RIP, RIP+2 and
one retirement. Memory, exception, JIT and native paths remain unsupported.

Local validation covers an exact bare decoder case, 51,200 complete success
snapshots, 21,504 invalid class/tag rejection snapshots, missing-state and
environment gates, and two FCOMI producer sequences that verify ZF and the
resulting source copy/preservation. The five-stream schema, two literal success
fixtures, a ZF=1 invalid-source fixture and JIT denial are covered. Broader boundary tests are not added for
this exact single form.
ISSUE-019 remains open; see the [S2-30ca plan](../plans/STAGE_2.md#s2-30ca-bare-fcmovne-st0-st1-db-c9).

## Local S2-30cb: bounded indexed bare `FCMOVNE ST(0), ST(i)` (`DB C8..CF`)

The exact unprefixed Long64 DB C8..CF forms decode `i` from `ModR/M & 7` and
reuse the bounded indexed FCMOV operand preflight. For `i>0`, validate the
old physical TOP and `(TOP+i)&7` before integer ZF, even when ZF=1; for `i=0`,
validate only TOP. Each participant must be Valid with canonical Normal
binary80 or Zero with exact signed zero. FCW must equal `0x037f`,
`FSW & 0x82ff` must be zero, and C1=1, forbidden environment, invalid tag/class,
or missing x87 state rejects atomically. ZF=0 copies exactly ten source bytes
and its tag to TOP; ZF=1 preserves both. Only FIP, RIP+2 and retirement change
on success. JIT/native remain unsupported.

The indexed matrices cover 363,520 complete successes and 161,280 atomic
invalid class/tag rejections. The 19-stream schema admits each exact one-step
and FNSTSW stream, a repeated C9 transfer, and the two existing FCOMI producer
programs. Literal runner fixtures cover self-index ZF cases, indices 2..7,
wrap, canonical Normal/Zero and negative-zero raw/tag transfer, repeat,
producer results, invalid source with ZF=1, and a forbidden environment. Unit
vectors also check each index's environment/status/boundary rejection and the
complement relation with indexed FCMOVE. Evidence is local interpreter/schema
coverage only; ISSUE-019 remains open and no physical or native execution is
claimed.

## Local S2-30cc historical: bounded exact bare `FCMOVBE ST(0), ST(1)` (`DA D1`)

Admit only the exact unprefixed Long64 `DA D1` form. Require present x87 state,
FCW `0x037f`, and `FSW & 0x82ff == 0`. Before reading integer RFLAGS, preflight
both old physical TOP and TOP+1, including the untaken CF=ZF=0 case. Each cell
must be Valid-tagged canonical Normal binary80 or Zero-tagged exact signed
binary80 zero. Copy the source's exact ten bytes and tag into old TOP exactly
when integer `RFLAGS.CF || RFLAGS.ZF`; otherwise preserve both cells. FSW
condition bits do not select the move. Preserve TOP, full FSW/FCW, all integer
flags, GPRs, FDP, FOP and other state; update FIP to old RIP, advance RIP by two
and retire once. Rejections commit no state. Memory forms, x87 exception
delivery, JIT, native execution and physical acceptance are excluded.

Local interpreter evidence comprises 51,200 complete success states and
86,016 atomic invalid class/tag rejects. Coverage spans 10×10 operand values,
all four CF/ZF rows, both PF values, all TOP positions and all eight C0/C2/C3
combinations; invalid participants are checked at both positions under all
condition rows. Additional checks cover missing x87 state, FCW/FSW gates,
prefixes, neighboring encodings and execution boundaries. The fixture schema
admits exactly six streams: bare, FNSTSW observation, repeat, and three FCOMI
producers. Handwritten runner fixtures check the truth table, raw signed-zero/
tag copy, TOP wrap, both repeat steps, greater/equal/less producers, invalid
source and bad environment with complete state/memory assertions; all fixtures
deny JIT. This evidence remains local only and does not resolve ISSUE-019.

## Local S2-30cd: bounded indexed `FCMOVBE ST(0), ST(i)` (`DA D0..D7`)

Extend the historical S2-30cc `DA D1` slice to exact unprefixed Long64
`DA D0..D7`, represented as `Operation::Fcmovbe { source }`. Require present
x87 state, FCW `0x037f`, and `FSW & 0x82ff == 0`. Before examining integer
RFLAGS, validate old physical TOP and `(TOP+i)&7` for i>0; self-index i=0
validates TOP once and leaves every unrelated physical register/tag opaque.
Each participating cell must be Valid-tagged canonical Normal binary80 or
Zero-tagged exact signed binary80 zero, including when `CF=ZF=0`. Copy the
source's raw ten bytes and tag to old TOP exactly when integer `CF || ZF`;
otherwise preserve it. Preserve all other state except FIP=old RIP, RIP+=2,
and one retirement. Rejections are atomic. C1=1, memory forms, exception
delivery, JIT, native execution and physical acceptance remain excluded.

Local interpreter matrices contain 363,520 complete success snapshots:
51,200 for each i=1..7 and 5,120 self-index snapshots. The invalid class/tag
matrix contains 645,120 snapshots: 86,016 for each i=1..7 and 43,008 for i=0.
Coverage includes all four CF/ZF rows, both PF values, all TOP values, all
C0/C2/C3 combinations, tag/raw transfer, and TOP wrap. Schema v2 admits exactly
twenty streams: DA D0..D7, each optionally followed by FNSTSW AX, the retained
DAD1 repeat, and three retained FCOMI producers. Literal fixtures and the
runner compare complete state/memory, RFLAGS, RIP/FIP and retirement, and deny
JIT for every index. This is local evidence only; ISSUE-019 and physical x86
behavior remain open.

## Local S2-30ce: bounded bare `FCMOVNBE ST(0), ST(1)` (`DB D1`)

Admit only exact unprefixed Long64 `DB D1`. Require present x87 state, FCW
`0x037f`, and `FSW & 0x82ff == 0`. Before reading integer CF/ZF, preflight
physical TOP and `(TOP+1)&7`, including when the move is untaken. Each
participant must be Valid-tagged canonical Normal binary80 or Zero-tagged
exact signed binary80 zero. Copy the source's raw ten bytes and tag to TOP
exactly when `CF=0 && ZF=0`; otherwise preserve both cells. Preserve all state
except FIP=old RIP, RIP+=2 and one retirement. Rejections are atomic.

Coverage includes 51,200 successful snapshots (12,800 taken and 38,400
untaken), 86,016 invalid class/tag rejects, an explicit same-CF/ZF complement
property against FCMOVBE, environment/decode/boundary checks, and repeat
intermediate states plus producer checkpoints after FCOMI/FCHS. The schema
admits exactly the six documented bare, observation, repeat, and FCOMI
producer streams. Literal runner fixtures check
full machine and memory snapshots and deny JIT. C1=1, other DB D0..D7 forms,
memory operands, x87 exception delivery, native/JIT execution, and physical
x86 acceptance remain outside this local contract. ISSUE-019 remains open.
See the [S2-30ce plan](../plans/STAGE_2.md#s2-30ce-exact-bare-fcmovnbe-st0-st1-db-d1),
[report](../reports/STAGE_2_REPORT.md#s2-30ce--bare-fcmovnbe-st0-st1), and
[fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30ce-bare-fcmovnbe-st0-st1-db-d1).

## Local S2-30cf: bounded indexed `FCMOVNBE ST(0), ST(i)` (`DB D0..D7`)

Extend the historical S2-30ce `DB D1` behavior to exact unprefixed Long64
`DB D0..D7`, carrying the index in `Operation::Fcmovnbe { source }`. Keep the
present-x87, FCW `0x037f`, and `FSW & 0x82ff == 0` gates. Before reading integer
CF/ZF, validate physical TOP and `(TOP+i)&7` when i>0; self-index validates
TOP only, and other stack cells remain opaque. Every participant is
Valid-tagged canonical Normal or Zero-tagged exact signed zero. Copy raw ten
bytes and tag to TOP exactly when both CF and ZF are clear. Valid execution
preserves all state except FIP=old RIP, RIP+=2 and one retirement; every reject
is atomic. C1=1, memory forms, exception delivery, JIT/native execution and
physical acceptance remain outside this contract.

Interpreter matrices contain 363,520 successes (7×51,200 indexed and 5,120
self-index) and 645,120 invalid class/tag rejects (7×86,016 indexed and 43,008
self-index). Same-CF/ZF FCMOVBE comparison covers every index; for i=0, both preserve
the same cell. The
exact schema contains 20 streams: each DB D0..D7 alone and followed by FNSTSW
AX, the retained DB D1 repeat, and the three FCOMI producer streams. Literal
fixtures cover indices 0 and 2..7, all four self CF/ZF rows, wrap, both Normal/Zero transfer directions and signed-zero transfer, invalid
taken and untaken sources, indexed environment rejection, and boundary
atomicity. Runner snapshots compare full state and memory and
deny JIT at every index. Evidence is local only; ISSUE-019 remains open.
See [plan](../plans/STAGE_2.md#s2-30cf-indexed-bare-fcmovnbe-st0-sti-db-d0d7),
[report](../reports/STAGE_2_REPORT.md#s2-30cf--indexed-bare-fcmovnbe-st0-sti), and
[fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30cf-indexed-bare-fcmovnbe-st0-sti-db-d0d7).


## Local S2-30cg: historical bare `FCMOVU ST(0), ST(1)` (`DA D9`)

The original local slice accepted only `DA D9`, with TOP and TOP+1 preflight
before PF, and recorded 512 success plus 768 invalid class/tag snapshots.
Its two TEST-producer fixtures remain as compatibility coverage. Indexed
behavior is documented under S2-30ch; unordered/NaN inputs, exception delivery
and physical x86 execution remain outside the contract.

## Local S2-30ch: bounded indexed `FCMOVU ST(0), ST(i)` (`DA D8..DF`)

The interpreter admits only exact unprefixed Long64 encodings `DA D8..DF`,
with the low ModR/M bits stored as the source index. It requires x87 state,
FCW `0x037f`, `FSW & 0x82ff == 0`, and used participants tagged Valid with
canonical Normal values or Zero with exact signed zero. Before integer PF is
read, `i>0` preflights old physical TOP and `(old_TOP+i)&7`, even for PF=0;
`i=0` validates TOP alone and retains its exact bytes/tag without inspecting
other cells. PF=1 raw-copies the source's ten bytes and tag to TOP; PF=0 keeps
the destination. C1 remains closed and Special/NaN values are unsupported.
All other state and memory remain unchanged except FIP=old RIP, RIP+=2 and one
retirement. Any rejected state or boundary leaves CPU state unchanged.

Local unit coverage comprises 363,520 success snapshots and 161,280 invalid
class/tag snapshots. The schema admits exactly 18 streams: each DA D8..DF bare
instruction (budget 1) and with FNSTSW AX (budget 2), DA D9 repeated with
FNSTSW (budget 3), and the retained FLD1/FLDZ/TEST/DA D9/FNSTSW producer
(budget 5). Nine bare success literals cover self PF=0/1 and indices 1..7;
repeat and both producer fixtures are retained, with invalid-source PF=0/1,
invalid destination, C1, ES, B and FCW rejection literals. The runner compares
full state and memory and confirms JIT denial. No unordered/NaN semantics,
general x87 exception delivery, native behavior or physical x86 acceptance is
claimed. ISSUE-019 remains open.

## Local S2-30ci: historical bare `FCMOVNU ST(0), ST(1)` (`DB D9`)

The local Long64 interpreter originally accepted only unprefixed `DB D9`. Before consulting
integer PF it requires x87 state, FCW `0x037f`, `FSW & 0x82ff == 0`, and both
physical slots TOP and `(TOP+1)&7` to be Valid-tagged canonical Normal or
Zero-tagged exact signed zero. The pair is validated even when PF=1 leaves the
destination untaken. PF=0 copies all ten raw source bytes and its two-bit tag
to physical TOP. PF=1 leaves TOP bytes/tag unchanged. C1=1 is rejected by the existing status gate; successful inputs have C1=0. No
other condition-code bit, FCW/FSW value, integer flag, register or memory byte
changes. On success, x87 FIP receives old RIP,
RIP advances by two and one instruction retires. Gate and operand failures are
atomic. This historical one-index scope is superseded by S2-30cj; its fixtures
remain compatibility evidence. No unordered/NaN behavior is included.

Local interpreter evidence is 51,200 full-state success snapshots, evenly
split between taken and untaken, and 21,504 invalid class/tag snapshots. The
original four-stream fixture allowlist, literal cases, producer intermediate-state
checks, prefix/boundary rejects and JIT denial are local checks. Native/JIT
execution, x87 exception delivery and physical x86 behavior remain unverified;
see [ISSUE-019](../KNOWN_ISSUES.md#issue-019--семантика-арифметики-и-инструкций-x87-остаётся-открытой).

## Local S2-30cj: bounded indexed `FCMOVNU ST(0), ST(i)` (`DB D8..DF`)

The interpreter admits only exact unprefixed Long64 encodings `DB D8..DF`,
retaining `ModR/M & 7` as source. Require present x87, FCW `0x037f`,
`FSW & 0x82ff == 0`, and each used participant tagged Valid with canonical
Normal binary80 or Zero with exact signed zero. For `i>0`, preflight physical
TOP and `(TOP+i)&7` before integer PF on taken and untaken paths. For `i=0`,
check TOP only and do not inspect any other cell. PF=0 copies the exact ten
source bytes and source tag into TOP; PF=1 keeps TOP. Self-index changes no
payload or tag. C1 remains closed and Special/NaN values are unsupported.
Preserve all other state and memory except FIP=old RIP, RIP+=2 and one
retirement. Rejects leave the complete state unchanged.

Local unit coverage is 363,520 complete success snapshots (358,400 for
`i>0`, plus 5,120 self-index) and 161,280 invalid class/tag snapshots
(150,528 for `i>0`, plus 10,752 self-index). Exact schema has 18 streams;
literal fixtures cover every index, self PF=0/1, signed zero, wrap, invalid
participants under PF=0/1, bad destination/environment, repeat and both TEST
producers. Independent complete snapshots check the FCMOVU complement. The testkit literal interpreter checks full state and memory, and the CLI
runner denies JIT for both accepted and rejected rows. Native/JIT execution, x87
exception delivery and physical x86 acceptance remain unverified; ISSUE-019
stays open.

### Local S2-19b bounded REP STOSW/D/Q contract

The interpreter extends `F3 AA` REP STOSB with `F3 66 AB` word, `F3 AB`
dword, and effective REX.W + `AB` qword; all `48..4F` values are admitted.
For AB, only F3, optional 66 (including `66 F3 AB`), no segment/address override, and absent REX or
effective REX.W are accepted. `F3 48 66 AB` normalizes to word and
`F3 66 48 AB` to qword. `F3 66 AA` remains unsupported; all other REP/LOCK,
67, segment, duplicate-66 and non-W REX variants remain outside the profile.

Zero RCX completes without destination validation or access. Each nonzero
iteration performs one all-or-error width write of low RAX at current RDI;
only success commits the wrapping DF-directed RDI step and RCX decrement. Full
completion wins over STOP and work-limit checks. Between iterations STOP wins
over the 1024-successful-store local work limit. STOP, limit, or a fault leaves
RIP on the REP instruction and retires zero; earlier successful memory and
index/count progress remain committed. The one-instruction retirement budget
still counts the whole completed REP once. RAX, RSI, RFLAGS and in-run flag
definedness are preserved. A fresh `run()` resets definedness; intentional
cross-run continuation can preserve its returned mask through the explicit
tracked-entry API described by S2-19i. No bulk memory operation or RSI access
is performed. ISSUE-016 and ISSUE-020 remain open; JIT/native and physical
execution are deferred.

### Local S2-19c bounded REP LODSB/W/D/Q contract

The interpreter admits exact `F3 AC` LODSB, `F3 66 AD` LODSW, `F3 AD`
LODSD, and effective REX.W `AD` LODSQ. For `AD`, F3 may be accompanied by 66
and no segment/address-size prefix; REX must be absent or effective REX.W.
Every `48..4F` REX.W byte is accepted. Prefix normalization accepts `66 F3
AD` and `F3 48 66 AD` as word, while `F3 66 48 AD` is qword. `F3 66 AC`,
F2/LOCK/67/segment overrides, non-W REX on `AD`, other prefixed `AC`, and
other REP forms remain unsupported admission cases. Native/JIT admission
remains closed.

With `RCX=0`, the instruction completes without reading or validating RSI and
preserves RAX/RSI. Otherwise, each iteration performs exactly one width-sized
read from flat `DS:[RSI]`; only after success does it update AL/AX/EAX/RAX via
`RegisterView`, wrapping-adjust RSI by ±width according to DF, and decrement
RCX. Completion is checked before STOP, which is checked before the 1024
successful-read work limit. Full completion advances RIP and retires one
instruction. STOP and limit exhaustion keep RIP at the REP start and retire
zero. A read fault leaves the current iteration untouched while preserving all
earlier completed accumulator/index/count updates; replay starts at the next
element without rereading completed elements. Flags, DF, RDI and RAM do not
change.

The bounded-memory backend reports missing mappings as harness errors; the
interpreter does not synthesize `#PF`. This local contract makes no MMIO
rollback claim. Fresh `run()` calls reinitialize definedness; intentional
cross-run continuation is available through the explicit S2-19i tracked-entry
API. The 12,288-state completion matrix, 96 injected errors, STOP/replay
and limit checks, pointer/mapping edges, TEST→REP definedness, 48 prefix
rejections and exactly 36 literal schema-v2 fixtures are documented in the
[Stage 2 report](../reports/STAGE_2_REPORT.md#s2-19c--bounded-f3-acad-rep-lodsbwdq-local).
ISSUE-016 and ISSUE-020 remain open; physical/native execution is deferred to
the shared final acceptance.

### Local S2-19j bounded REP LODS address-size-32 contract

The existing REP LODS operation additionally admits Long64 address-size-32
forms `F3 67 AC` (byte), `F3 67 66 AD` (word), `F3 67 AD` (dword), and
`F3 67 48 AD` (qword). For AD, normalized prefix ordering retains the existing
operand-size rule: effective REX.W overrides 66, and a later legacy prefix
clears an earlier REX. The bounded decoder also accepts the exercised
normalizations `F3 66 67 AD` and `F3 67 48 66 AD` as word and `F3 67 66 48 AD`
as qword. REP byte LODS remains byte-only apart from the address override.
Other segment, LOCK, REP/REPNE, and unsupported prefix combinations remain
outside admission. JIT/native admission stays closed.

Each iteration reads exactly one operand width from flat `DS:[ESI]`, using
zero-extended ESI as the starting address. After a successful read, the
interpreter writes AL/AX/EAX/RAX using the existing register alias rules,
updates ESI by ±width modulo 2^32 and zero-extends the result, then decrements
ECX and writes the zero-extended result to RCX. A width-sized read stays
contiguous across a 4 GiB boundary. For address-size 32, `ECX=0` does not
validate or access RSI, does not decrement, preserves all of RCX, and completes
normally. Current-read failure
does not commit that iteration; previous successful elements remain in the
snapshot. STOP, completion, and the local 1024-success limit preserve the
existing REP LODS ordering. Full completion retires once; partial outcomes
leave RIP at the REP instruction and retire zero. Flags, DF, RDI and RAM are
unchanged. Missing mappings remain harness errors, and no MMIO rollback,
hardware restart-state, or dual-fault priority claim is made. The explicit
tracked-entry API can carry RFLAGS definedness across partial resume; ordinary
`run()` remains a fresh entry.

Local evidence and limitations are recorded in the
[S2-19j plan](../plans/STAGE_2.md#s2-19j-bounded-rep-lods-с-address-size-32) and
[Stage 2 report](../reports/STAGE_2_REPORT.md#s2-19j-bounded-rep-lods-address-size-32).
ISSUE-016/020/021 remain open; native/JIT and physical execution are deferred
to shared final acceptance.

### Local S2-19k bounded REP STOS address-size-32 contract

The existing REP STOS operation additionally admits Long64/address-size-32
forms `F3 67 AA` (byte), `F3 67 66 AB` (word), `F3 67 AB` (dword), and
effective REX.W `F3 67 48 AB` (qword). The exercised normalized forms retain
the existing 66/REX rule: `F3 66 67 AB` and `F3 67 48 66 AB` are word,
`F3 67 66 48 AB` is qword, and `F3 48 67 AB` is dword. Other segment, LOCK,
REP/REPNE, and unsupported prefix combinations remain outside this bounded
admission. JIT/native admission stays closed.

Each iteration writes exactly one width-sized value from AL/AX/EAX/RAX to the
zero-extended EDI address, then advances EDI by ±width modulo 2^32 according
to DF and decrements ECX with zero-extension into RCX. ECX=0 avoids memory
access and preserves the complete RCX and RDI values. A failed current write
does not commit its iteration; prior successful stores and their count/index
progress remain visible. REP completion, STOP, tracked continuation and the
1024-success limit use the existing REP ordering. Completion retires once;
partial exits keep RIP at the instruction and retire zero. Flags and RAM
contents are preserved except for successful stores. Missing mappings remain
harness errors. No MMIO rollback, hardware restart-state, fault-priority,
native/JIT, or physical behavior is claimed; ISSUE-016/020/021 remain open.

Local implementation and limitations are recorded in the
[S2-19k plan](../plans/STAGE_2.md#s2-19k-bounded-rep-stos-с-address-size-32) and
[Stage 2 report](../reports/STAGE_2_REPORT.md#s2-19k--bounded-rep-stos-address-size-32).

### Local S2-19l bounded REP MOVS address-size-32 contract

The bounded Long64 interpreter additionally admits `F3 67 A4` MOVSB,
`F3 67 66 A5` MOVSW, `F3 67 A5` MOVSD and effective REX.W `F3 67 48 A5` MOVSQ.
The already documented normalized 66/REX prefix ordering remains available for
A5. Other newly prefixed forms remain unsupported; this local boundary is not a
claim that all excluded encodings architecturally raise #UD. No JIT/native
admission or fixture schema/profile change is included.

REP count uses ECX. ECX=0 avoids all data-address validation/access and preserves
full RCX/RSI/RDI. Each active iteration reads one contiguous width-sized element
from zero-extended ESI, then performs one all-or-error write to zero-extended
EDI. A transfer does not split or wrap its byte range at 4 GiB. Only after the
store succeeds are ESI and EDI updated modulo 2^32 according to DF and ECX
decremented with zero-extension into RCX. The faulting iteration does not commit
register state or destination bytes; earlier iterations remain visible. Overlap
observes the current RAM contents at each subsequent source read.

Completion takes precedence over STOP and the 1024-success local work limit;
STOP takes precedence over that limit. Full completion retires once. Fault, STOP,
and limit leave RIP at REP and retire zero. Tracked continuation preserves the
definedness mask; ordinary `run()` remains a fresh entry. Missing mappings are
harness errors. Source-first access order is a local policy, not a hardware
priority assertion for dual-invalid source/destination addresses; ISSUE-021
remains open. No MMIO rollback, JIT/native behavior, physical behavior, or
hardware restart-state claim is made.

Implementation evidence is recorded in the
[S2-19l plan](../plans/STAGE_2.md#s2-19l-bounded-rep-movs-с-address-size-32),
[interpreter notes](../testing/INTERPRETER.md#rep-movs-with-address-size-32--s2-19l),
and [Stage 2 report](../reports/STAGE_2_REPORT.md#s2-19l--bounded-rep-movs-address-size-32).

### Local S2-19m bounded REPE SCASB address-size-32 contract

The Long64 interpreter additionally admits only the byte SCAS form with exact
prefix sequences `F3 67 AE` and `67 F3 AE`, each of length three, decoded as
`Operation::RepScas { size: Byte }`. Existing address-size-64 `F3 AE` remains
unchanged. This byte-only S2-19m slice does not include `AF`; S2-19o below adds
bounded address-size-32 W/D/Q `AF`, and S2-19p below adds bounded W/D/Q `A7`
CMPS. REPNE, unsupported extra 66/REX/segment/LOCK prefixes and repeated
prefixes remain outside the combined local profile.
Decoder and execution admission enforce the same boundary. JIT/native admission
remains closed.

Before count handling, including when ECX is zero, the complete existing gate
requires `RFLAGS & 0x8d5 == 0x44` and the definedness mask to include `0x8d5`.
The count is ECX. ECX=0 preserves full RCX/RDI and flags, performs no data
address validation or read, advances RIP by three, and retires once. Each
nonzero iteration reads one byte at zero-extended EDI and compares AL with the
byte using CMP semantics. A successful read commits arithmetic flags, EDI ±1
modulo 2^32 according to DF, and zero-extended ECX-1. A fault leaves that
iteration uncommitted while prior iterations remain committed. ECX completion
or ZF=0 precedes STOP, which precedes the 1024-success limit. Fault, STOP and
limit preserve REP RIP and retire zero; tracked continuation uses the previous
definedness mask. Missing mappings remain harness errors.

Local evidence and exclusions are recorded in the
[S2-19m plan](../plans/STAGE_2.md#s2-19m-bounded-repe-scasb-с-address-size-32),
[interpreter notes](../testing/INTERPRETER.md#repe-scasb-address-size-32--s2-19m),
and [Stage 2 report](../reports/STAGE_2_REPORT.md#s2-19m--bounded-repe-scasb-address-size-32).

### Local S2-19n bounded REPE CMPSB address-size-32 contract

The Long64 interpreter admits only exact `F3 67 A6` and `67 F3 A6`, each
three bytes, decoded as byte `Operation::RepCmps`. Existing address-size-64
`F3 A6` remains unchanged. Address-size-32 `A7`, REPNE, extra 66/REX/segment/
LOCK prefixes, and repeated prefixes remain outside this profile. Decoder and
execution admission enforce the same boundary; JIT/native admission stays
closed.

Before consulting ECX, including at zero count, the existing local gate
requires `RFLAGS & 0x8d5 == 0x44` and definedness to contain `0x8d5`. Count uses
ECX. ECX=0 preserves full RCX/RSI/RDI and flags, performs no address check or
data read, advances RIP by three, and retires once. Each nonzero iteration reads
one byte from zero-extended ESI, then one byte from zero-extended EDI. It
compares source minus destination using existing CMPSB/CMP semantics. Only after
both reads succeed does it commit arithmetic flags, modulo-32 ESI/EDI ±1 by DF,
and zero-extended ECX-1. A failed second read retains the successful source
read in the local trace but commits no current-iteration CPU state. Earlier
iterations remain committed. Source-first and dual-invalid results are local
ISSUE-020 policy only; no hardware priority or MMIO rollback is claimed.

ECX completion or ZF clear completes before STOP, which precedes the 1024
successful-comparison limit. Fault, STOP, and limit preserve REP RIP and retire
zero. Tracked continuation carries the prior definedness mask; fresh `run()`
remains a fresh entry. Missing mappings remain harness errors.

The implementation evidence is in the [S2-19n plan](../plans/STAGE_2.md#s2-19n--bounded-repe-cmpsb-с-address-size-32),
[interpreter notes](../testing/INTERPRETER.md#repe-cmpsb-address-size-32--s2-19n),
and [Stage 2 report](../reports/STAGE_2_REPORT.md#s2-19n--bounded-repe-cmpsb-address-size-32). ISSUE-020 remains open.

### Local S2-19o bounded REPE SCASW/D/Q address-size-32 contract

The Long64 interpreter adds only wide `AF` REPE SCAS forms with address-size
32. Normalized group1 F3 plus 67 accepts W (`F3 67 66 AF`), D (`F3 67 AF`),
or Q (`F3 67 REX.W AF`). Legacy prefix order is respected: a later 66 resets
an earlier REX.W (`F3 67 48 66 AF` is W), while a final effective REX.W
overrides 66 (`F3 67 66 48 AF` is Q). `F3 48 67 AF` is D because 67 resets
the earlier REX. Every effective `48..4F` REX.W encoding is admitted. The
existing address-size-64 W/D/Q slice and address-size-32 SCASB `AE` forms stay
unchanged. Address-size-32 CMPS `A7`, F2, LOCK, segment overrides, repeated
prefix groups, effective non-W REX and other prefixes remain outside this
profile. Decoder and executor admission agree; JIT/native remain denied.

Before checking ECX, including ECX=0, the local entry/restart gate requires
`RFLAGS & 0x8d5 == 0x44` and definedness to contain `0x8d5`. Count uses ECX.
Zero ECX preserves complete RCX, RDI, RAX, RSI and flags, performs no data
address validation or read, advances RIP by the full instruction length and
retires once. Each nonzero iteration performs one contiguous width-sized read
from zero-extended EDI, then compares AX/EAX/RAX against the memory value.
After a successful read it commits CMP flags, EDI ± width modulo 2^32 and
zero-extended ECX-1. The complete operand read may cross 4 GiB; it does not
wrap into a second read at address zero. A failed current read commits no
current-iteration state; earlier successful iterations remain committed.
Completion or mismatch precedes STOP, then the 1024-success limit. Fault,
STOP and limit preserve REP RIP and retire zero; tracked resume carries the
prior definedness mask. Missing mappings remain harness errors.

Evidence is recorded in the [S2-19o plan](../plans/STAGE_2.md#s2-19o--bounded-repe-scaswdq-с-address-size-32),
[interpreter notes](../testing/INTERPRETER.md#repe-scaswdq-address-size-32--s2-19o),
and [Stage 2 report](../reports/STAGE_2_REPORT.md#s2-19o--bounded-repe-scaswdq-address-size-32).
ISSUE-020 remains open.

### Local S2-19p bounded REPE CMPSW/D/Q address-size-32 contract

The Long64 interpreter accepts only address-size-32 `A7` W/D/Q REPE CMPS
forms: normalized F3+67 with optional 66 and effective REX.W. `F3 67 66 A7`
is W, `F3 67 A7` is D, and `F3 67 48 A7` is Q. Legacy ordering preserves the
usual reset rule: `F3 67 48 66 A7` is W, `F3 67 66 48 A7` is Q, and
`F3 48 67 A7` is D. All effective `48..4F` REX.W bytes are admitted. Existing
addr64 CMPS B/W/D/Q and addr32 CMPSB remain unchanged. F2, LOCK, segments,
repeated prefix groups, effective non-W REX and other forms remain outside
this bounded profile. Decoder and executor admission agree; JIT/native remain
closed.

The local entry/restart gate precedes count and memory access, even for ECX=0:
`RFLAGS & 0x8d5 == 0x44`, with `0x8d5` defined. Count uses ECX. Zero count
preserves the complete RCX/RSI/RDI and flags, performs no data reads, and
retires once at the full instruction length. A nonzero iteration first reads
one contiguous width-sized operand from zero-extended ESI, then one from
zero-extended EDI. It compares source minus destination with existing CMP
semantics. Only after both reads succeed does it commit flags, DF-directed
ESI/EDI steps modulo 2^32, and zero-extended ECX-1. The full access is
contiguous across 4 GiB: only the starting address is truncated. A failed first
read prevents the second; a failed second read keeps the first trace entry but
commits no state for that iteration. Replay reloads both operands. Previously
committed iterations remain. Source-first and dual-invalid winner are local
ISSUE-020 policy only; no hardware priority or MMIO rollback is claimed.

Completion or mismatch precedes STOP, followed by the 1024-success limit.
Fault/STOP/limit keep RIP at the REP instruction and retire zero. Tracked
resume carries the definedness mask. Local tests cover independent W/D/Q CMP
results, both DF values, prefix admission/rejection, high halves, zero count,
both 32-bit wrap directions, contiguous boundary tails, source/destination and
dual-invalid reads, retry with changed source data, STOP/limit, and CMP/TEST
composition. Fourteen literal schema-v2 fixtures carry full snapshots; their
generator serializes authored expectations only. JIT remains denied. Native,
physical x86, Mac mini and SSH execution are not claimed.

See the [S2-19p plan](../plans/STAGE_2.md#s2-19p--bounded-repe-cmpswdq-с-address-size-32),
[interpreter notes](../testing/INTERPRETER.md#repe-cmpswdq-address-size-32--s2-19p),
[report](../reports/STAGE_2_REPORT.md#s2-19p--bounded-repe-cmpswdq-address-size-32)
and [fixtures](../../tests/fixtures/x86/stage2/README.md#bounded-repe-cmpswdq-address-size-32-fixtures-s2-19p).
ISSUE-020 remains open.

### Local S2-19q bounded REPNE SCASB address-size-32 contract

Only Long64 `F2 67 AE` and `67 F2 AE` are admitted. They normalize to one
addr32 REPNE SCASB operation. Existing F3 forms remain unchanged; other F2
SCAS/CMPS, wide AF, extra/repeated prefixes, JIT and native remain unsupported.
The legacy `run`, `run_with_rflags_defined`, and single-step APIs fail closed
for this form. Cross-run continuation is available only through
`run_with_rep_continuation` and its opaque in-memory `RepContinuation` token.

Before checking ECX or accessing data, the local gate requires all six
arithmetic flags to be defined. Concrete RFLAGS values are not restricted.
ECX=0 performs no data access and preserves full RCX/RDI/RAX/RFLAGS and the
definedness mask, then retires once. Each successful nonzero iteration reads
one byte from zero-extended EDI, compares AL minus that byte using the existing
CMP implementation, commits the result flags, increments/decrements EDI modulo
2^32 by DF, and zero-extends ECX-1. REPNE finishes when ECX reaches zero or ZF
becomes one. Completion is checked before STOP and the 1024-read invocation
limit.

STOP/limit preserves the latest comparison flags and a token checkpoint at the
REP RIP. Resume validates full relevant RAX/RCX/RDI/RFLAGS/definedness/context
state before callbacks or instruction/data access, then validates normalized
instruction identity after decode and before data read. A mismatch returns an
input error without architectural mutation. Architectural fetch/data faults
restore entry RFLAGS and definedness while retaining prior count/index progress
and clear the token. Backend `MemoryError` keeps current state and token for
retry. Completion clears the token. A fresh call with `None` explicitly starts
a new boundary; partial schema-v2 snapshots do not persist the token.

Intel SDM Combined Vol. 2B, REP/REPE/REPNE, p. 4-568 (revision 093) specifies
the restart flag restoration basis. This implementation does not claim precise
hardware fault delivery or #PF priority. Local API behavior is covered by
independent CMP modeling, prefix-order acceptance, STOP/resume, stale token,
fault rollback, and old-entry rejection tests. See the [S2-19q plan](../plans/STAGE_2.md#s2-19q--bounded-repne-scasb-с-address-size-32),
[interpreter notes](../testing/INTERPRETER.md#repne-scasb-address-size-32--s2-19q),
and [report](../reports/STAGE_2_REPORT.md#s2-19q--bounded-repne-scasb-addr32).

### Local S2-19r bounded REPNE CMPSB address-size-32 contract

Only Long64 `F2 67 A6` and `67 F2 A6` are admitted as addr32 REPNE CMPSB.
Existing address-size-64 F3 CMPS and address-size-32 F3 CMPSB stay unchanged.
Address-size-64 F2, wide F2 CMPS, LOCK, segment overrides, and extra or
repeated prefixes remain unsupported. JIT/native admission remains closed.
Legacy `run`, `run_with_rflags_defined`, and step APIs fail closed. The
explicit `run_with_rep_continuation` API accepts this form alongside S2-19q
SCASB; its opaque token records a tagged accumulator or source-index checkpoint
and shared RIP/count/destination/flags/mask/context state. Tokens stay with the
caller; schema v2 remains unchanged.

Before reading ECX or data, the entry gate requires all arithmetic flags to be
defined, including when ECX is zero; concrete RFLAGS values are unrestricted.
Zero ECX performs no data reads and preserves full RCX/RSI/RDI/RFLAGS and mask,
then retires once. Otherwise read one byte from zero-extended ESI followed by
one byte from zero-extended EDI, then compute source minus destination. Only
after both reads succeed commit CMP flags, ESI/EDI modulo 32 according to DF,
and zero-extended ECX-1. Completion at ECX zero or ZF one wins over STOP and
the 1024-success work limit.

An architectural source or destination read fault restores entry RFLAGS and
definedness, retains progress from earlier completed iterations, and clears the
token; destination failure leaves the completed source access in the trace.
A backend `MemoryError` leaves current architectural state and token intact.
Retry re-reads source and destination; the token does not retain a source byte.
Resume checks the full relevant checkpoint and context before callbacks/fetch,
then checks decoded operation identity before data reads. SCASB and CMPSB tokens
cannot cross-resume. `None` begins a fresh boundary; a partial fixture snapshot
cannot restore its token. The source-first and dual-invalid outcome is local
ISSUE-020 policy only, without a hardware priority or MMIO rollback claim.

Tests use an independent byte CMP model and cover both prefix orders, zero
count, continuation and work-limit boundaries, both read-fault positions,
source-first trace, backend retry after changed source data, stale state and
instruction cross-kind rejection, old entry APIs, literal schema-v2 full
snapshots, and JIT denial. No native/JIT/physical, Mac mini, or SSH run is
claimed. See the [S2-19r plan](../plans/STAGE_2.md#s2-19r--bounded-repne-cmpsb-с-address-size-32),
[interpreter notes](../testing/INTERPRETER.md#repne-cmpsb-address-size-32--s2-19r),
and [report](../reports/STAGE_2_REPORT.md#s2-19r--bounded-repne-cmpsb-addr32).

### Local S2-19d bounded REP MOVSB/W/D/Q contract

The interpreter admits exact `F3 A4` MOVSB, `F3 66 A5` MOVSW, `F3 A5` MOVSD,
and effective REX.W `A5` MOVSQ. For A5, F3 may be accompanied by 66; segment
and address-size overrides are rejected, and REX must be absent or effective
REX.W. All `48..4F` REX.W bytes are admitted. Normalization accepts `66 F3 A5`
and `F3 48 66 A5` as word, while `F3 66 48 A5` is qword. `F3 66 A4`, F2,
LOCK, 67, segment overrides, non-W REX on A5 and other prefixed A4 forms are
unsupported. Existing non-REP MOVS and REP STOS/LODS behavior is retained;
JIT/native admission stays closed.

At `RCX=0`, REP completes without validating or accessing RSI/RDI. Each
iteration performs exactly one width-sized read from flat `DS:[RSI]`, followed
by exactly one all-or-error width write to flat `ES:[RDI]`. Only after both
succeed are RSI/RDI wrapping-adjusted by ±width according to DF and RCX
decremented. Completion wins over STOP and the local 1024-success limit; STOP
wins over the limit. A completed REP retires once, while STOP/limit/fault leaves
RIP at the REP start and retires zero. A failed source read skips the write. A
failed destination write follows a successful read but commits neither memory
destination nor indices/count for that iteration; replay reads that source
again because no pending value is kept. Previous successful transfers persist.
This source-first ordering is a local policy, not a confirmed winner for two
simultaneously invalid addresses; see ISSUE-021. No MMIO rollback or SMP
atomicity claim is made. RAX, flags/definedness and DF are unchanged within one
run; fresh `run()` calls reinitialize flag definedness. Intentional cross-run
continuation is covered by the explicit S2-19i tracked-entry API. Missing mappings remain harness errors, not
synthesized `#PF`.

Local evidence is defined by the [S2-19d plan](../plans/STAGE_2.md#s2-19d--f3-a4a5-rep-movsbwdq-ограниченный-локальный-контракт); native and physical acceptance remains deferred to the shared final acceptance.

### Local S2-19i tracked continuation after REP work limit

`run()` remains the fresh-entry API and initializes integer RFLAGS
definedness to `INTEGER_FLAGS` on every call. `run_with_rflags_defined()` is an
explicit tracked-entry API: it accepts only a mask contained in
`INTEGER_FLAGS`, rejects invalid masks before STOP/fetch/access, and returns
the updated mask in `RunResult`. Passing that returned mask to a subsequent
call preserves metadata across an intentional continuation. This does not
change schema-v2 fixture format or infer definedness from concrete RFLAGS.

Six schema-v2 fixtures compose `TEST RAX,RAX` with REP STOSB, LODSB, or MOVSB
for DF=0/1 and count 1025. The first invocation retires TEST, completes the
1024-successful-iteration work limit, leaves RIP at REP with `RCX=1`, and
returns the TEST mask (`0x0cc7`, AF undefined). The sidecar records literal
partial and final snapshots. A second invocation passes the first result's
mask explicitly and completes the last iteration once, preserving the
undefined-AF metadata. These local tests establish API continuation behavior
for the bounded interpreter, not hardware restart state, fault priority, or
JIT/native/physical support. ISSUE-016, ISSUE-020 and ISSUE-021 remain open.

### Local S2-19e bounded `F3 AE` REPE SCASB contract

The bounded interpreter admits only exact bytes `F3 AE` in Long64 with
address-size 64. This is a local profile restriction, not a claim that other
architectural REPE SCAS forms are invalid. Before any data access, including
when RCX is zero, admission requires
`RFLAGS & ARITHMETIC_FLAGS == PF|ZF` (`0x44`) and all six arithmetic flags to
be defined (`rflags_defined & ARITHMETIC_FLAGS == 0x8d5`). Either DF value is
allowed. A failed gate returns Unsupported without changing state or reading
data. Prefixes, REPNE, other widths, CMPS, JIT, and native execution remain out
of scope.

With RCX zero, do not validate or read RDI; preserve flags and advance RIP by
two. Otherwise each successful iteration reads one byte from flat `ES:[RDI]`,
computes the low-byte `AL - byte` CMP flags, updates RDI by wrapping ±1 under
DF, and decrements RCX. RAX, RSI, RAM, and non-arithmetic flags are preserved.
Terminate after the read/commit when RCX reaches zero or ZF clears; mismatch
completion precedes STOP and the 1024-successful-read work limit. On a matching
continuation, check STOP before the work limit. A fault leaves the current
iteration uncommitted; prior equal iterations remain reflected in RDI/RCX and
flags, and RIP remains on the REP instruction. STOP or work-limit exhaustion
also leaves RIP there and retires no instruction. Replay re-enters the same
definedness/value gate: each committed equal compare preserves exactly `PF|ZF`
as arithmetic values, with the complete arithmetic definedness mask, so no
hidden restart state is required. The flag admission is deliberately narrower
than x86 architectural requirements.

Tests cover 131,072 single-comparison states using an independent CMP flags
model, 160 count/mismatch termination paths, all 16,380 invalid arithmetic
value/definedness/count/DF combinations, 24 injected read failures, STOP/replay,
the 1023/1024/1025 work-limit boundaries, mismatch at iteration 1024, STOP vs
limit priority, zero count, invalid next pointer, and exactly 24 literal v2
fixtures. No physical, native, or JIT execution is claimed. ISSUE-016,
ISSUE-019, ISSUE-020 and ISSUE-021 remain open.

### Local S2-19f bounded `F3 A6` REPE CMPSB contract

The interpreter admits only exact bytes `F3 A6` in Long64/address-size 64.
Before any data access, including at RCX zero, it requires arithmetic values
exactly `PF|ZF` (`0x44`) and all arithmetic flags defined (`0x8d5`). This is a
local entry/restart profile restriction, not an architectural requirement for
REPE CMPS. Either DF value is admitted. Rejected gate state is unchanged, with
no data read or retirement. Other prefix combinations are unsupported; this
does not claim #UD priority. Existing `F3 AE` semantics are unchanged.

At RCX zero, RSI/RDI are not checked or read; flags are preserved, RIP advances
by two, and the instruction retires once. Otherwise each iteration reads one
byte from flat `DS:[RSI]`, then one byte from flat `ES:[RDI]`, and computes
`source - destination` with existing CMP semantics. Only after both reads
succeed does the iteration commit arithmetic flags, wrapping RSI/RDI by ±1
according to DF, and decrement RCX. Source-first ordering is the local policy
and does not establish a dual-invalid hardware exception winner (ISSUE-020).
There is no claim of rollback for MMIO/backend side effects.

RCX zero or ZF clear completes before STOP and the local limit of 1024
successful iterations. A matching continuation checks STOP before the limit.
On either read error, the current flags, indices, and count are not committed;
a successful first read remains visible in the access trace if the second fails.
Earlier equal iterations persist. Fault, STOP, and work-limit exits keep RIP at
the REP instruction and retire zero; replay performs both reads again without
hidden source state. All other registers, non-arithmetic flags, and RAM are
preserved. Missing mappings and backend errors remain harness errors.

Local evidence covers 131,072 independent byte-comparison states, 160
termination paths, 16,380 rejected value/definedness/count/DF combinations,
48 injected source/destination read failures, alias/overlap and STOP/replay,
limit and boundary behavior, and 31 literal schema-v2 fixtures. The generator
serializes fixed answers only. JIT and native admission remain closed; no
physical, Mini, or SSH execution is claimed. ISSUE-020 tracks the unknown
dual-fault winner.

### Local S2-19g bounded `F3 AF` REPE SCASW/D/Q contract

The Long64/address-size-64 interpreter adds only `F3 66 AF` SCASW, `F3 AF`
SCASD, and `F3 effective-REX.W AF` SCASQ. Prefix order determines whether
REX.W remains effective: `F3 48 66 AF` decodes as SCASW because the later
legacy 66 discards the earlier REX, while `F3 66 48 AF` decodes as SCASQ
because the final REX.W is effective and overrides 66. All `48..4F` bytes are
admitted when REX.W is effective. These AF forms require normalized group1
F3, no segment or address-size override, and either no effective REX or
effective REX.W. Exact `F3 AE` SCASB remains
available; `F3 66 AE` remains Unsupported. The explicit reject set includes
LOCK, REPNE, duplicate group1, address/segment overrides, and non-W REX cases.
No general REPNE or prefix policy is implied.

Before any data read, including RCX zero, the existing local SCAS entry gate
requires concrete arithmetic values exactly `PF|ZF` (`0x44`) and complete
arithmetic definedness (`0x8d5`). DF may be either value. This is a bounded
profile/restart condition, not an x86 architectural requirement. At RCX zero,
RDI is not checked, flags are preserved, and RIP advances by the decoded
instruction length.

For each nonzero iteration, compare AX/EAX/RAX with a Word/Dword/Qword read at
flat `ES:[RDI]` using existing CMP flag semantics. RAX is preserved in full,
including its upper half for SCASD; RSI, RAM, and non-arithmetic flags are
preserved. Only after a successful read are arithmetic flags, wrapping
DF-directed RDI by operand width, and RCX decremented. A failed read leaves
that iteration uncommitted. Count completion or mismatch finishes before STOP
and the local 1024-success limit; an equal continuation checks STOP before the
limit. Fault/STOP/limit keeps RIP at the REP and retires zero. Replay re-enters
the same gate: equal progress leaves exactly PF|ZF and all arithmetic flags
defined. JIT and native admission remain explicitly closed for these widths.

Coverage adds 864 independent single-comparison states, 720 termination paths,
49,140 W/D/Q gate rejects, 72 injected read failures, 12 STOP/replay paths, 18
limit paths, mismatch/STOP/limit boundary checks, RCX maximum, zero invalid-RDI,
alignment/range/wrap and noncanonical post-index checks, plus exactly 59 literal
schema-v2 fixtures. S2-19e retains its byte comparison and gate matrices. The
generator serializes fixture data; it is not an execution oracle. No native,
Mini, SSH, or physical execution is claimed.

### Local S2-19h bounded `F3 A7` REPE CMPSW/D/Q contract

The Long64/address-size-64 interpreter extends the exact `F3 A6` CMPSB form
with `F3 66 A7` CMPSW, `F3 A7` CMPSD, and effective-REX.W `F3 A7` CMPSQ.
Normalized `66 F3 A7` and `F3 48 66 A7` are word forms; `F3 66 48 A7` is
qword. All `48..4F` REX.W bytes are admitted when effective. `F3 66 A6`, F2,
LOCK, `67`, segment overrides, and non-W REX are rejected in this bounded
profile. This does not generalize prefix validity or widen JIT/native support.

Before any read, including RCX zero, the local entry/restart gate requires
concrete arithmetic values exactly `PF|ZF` (`0x44`) and all
`ARITHMETIC_FLAGS` defined (`0x8d5`). Nonzero iterations read the selected
width at flat `DS:[RSI]`, then flat `ES:[RDI]`, and compare source minus
destination using existing CMP semantics. Commit flags, wrapping DF-directed
RSI/RDI steps of operand width, and RCX decrement only after both reads
succeed. A failed second read leaves the first read visible in the local access
trace but commits no state for that iteration; prior complete equal iterations
remain committed. RSI-first is a local bounded-profile policy only and does not
establish dual-fault hardware priority. No MMIO rollback is claimed.

RCX zero does not validate either index or perform data reads. Count completion
or mismatch precedes STOP and the 1024-success work limit. Fault, STOP, and
limit keep RIP at the REP instruction and retire zero. Replay re-enters the
same concrete flag gate. ISSUE-020 continues to track the unresolved dual-fault
winner for CMPS B/W/D/Q.

Local coverage includes the 864-state independent CMP oracle, 720 termination
paths, 49,140 gate rejects, 144 injected read failures, 12 STOP/replay paths,
18 work-limit paths, prefix normalization/rejection, ordered reads, overlap,
page/range/wrap/fetch boundaries, RCX maximum and mismatch at iteration 1024.
Exactly 71 schema-v2 fixtures cover 19 per width, two normalization cases, and
four CMP/TEST compositions per width. The CLI verifies full state and JIT
denial; Linux-only native admission rejects these fixtures. This host did not
run the Linux-only test. Native execution and physical acceptance are untested.


### Local S2-18j CLD/STD contract

The bounded Long64 primary-map profile accepts only bare one-byte `FC` CLD and
`FD` STD. Each successful instruction clears/sets only RFLAGS.DF, advances RIP
by one and retires once. All other RFLAGS bits, all GPRs, segments, x87 state,
and memory are preserved; there are no data accesses or pointer/privilege checks.
`FlagEffects` has `defined = 1 << 10`, `undefined = 0`, so all other definedness
bits are preserved. Prefix forms are admission failures within this bounded
profile; no architectural #UD or fault-priority claim is made. Primary-map `FC`
is not D9-map `FC` FRNDINT. JIT/native/physical support is closed/deferred.

### Local S2-18k non-REP address-size-32 LODS contract

The bounded Long64 profile accepts non-REP `67 AC` LODSB, `67 66 AD` LODSW,
`67 AD` LODSD, and `67 48 AD` LODSQ. Prefixes use normalized decoder state:
effective REX.W overrides 66, while a following legacy prefix discards an
earlier REX. Repeated 67, REP67, segment overrides, and effective non-W REX on
AD remain outside LODS admission; admission for other string operations is
unchanged. These are profile
boundaries, not universal #UD assertions.

The initial linear address is zero-extended ESI with flat DS. Exactly one
width-sized read is issued from that starting address; address-size wrap does
not apply per byte, so a qword at ESI=`ffffffff` spans through
`1_0000_0006`. On success AL/AX preserve high RAX, EAX zero-extends, and RAX
receives a qword; RSI becomes zero-extended `(ESI +/- width) mod 2^32`, based
on DF. RIP advances by decoded length and one instruction retires. Any read
fault/error leaves CPU and RAM unchanged; high RSI/RAX and faulting RIP remain
as before and retirement is zero. High RSI bits discarded by address-size
truncation alone do not fault. RDI, RCX, flags and their definedness, segments,
x87 state, and RAM are otherwise preserved. JIT/native/physical support is not
enabled by this local interpreter contract.

### Local S2-18l non-REP address-size-32 STOS contract

The bounded Long64 profile accepts non-REP `67 AA` STOSB, `67 66 AB`
STOSW, `67 AB` STOSD, and `67 48 AB` STOSQ. Prefix normalization uses the
effective decoded address-size and REX state: effective REX.W wins over 66,
and a legacy prefix after REX discards that earlier REX. Thus `67 48 66 AB`
is word, `67 66 48 AB` is qword, and `48 67 AB` is dword. REP/REPNE,
segment overrides, and other address-size-32 string instructions remain
outside this bounded STOS profile, except the already-admitted non-REP LODS
forms in S2-18k. These boundaries do not assert a universal #UD or
prefix-precedence rule.

The starting linear destination is zero-extended EDI with flat ES. One
width-sized write spans continuously from that address; byte accesses do not
individually wrap at 4 GiB, so a qword at EDI=`ffffffff` reaches
`1_0000_0006`. After a successful write, RDI becomes the zero-extended
`(EDI +/- width) mod 2^32`, selected by DF. The complete RAX is the source and
is preserved, including after STOSD. RSI, RCX, flags and their definedness,
segments, x87 state, and all other registers are preserved. Successful
execution advances RIP and retires once.

Any store error leaves the complete CPU snapshot and RAM unchanged and retires
zero. Architectural store faults remain guest faults; unmapped, permission,
and injected backend failures remain HarnessError. This no-commit rule is a
bounded local memory-model policy tracked by ISSUE-016, not a claim about
hardware restart state or fault precedence. JIT/native admission remains
closed pending shared final acceptance.

### Local S2-18m non-REP address-size-32 SCAS contract

The bounded Long64 profile accepts non-REP `67 AE` SCASB, `67 66 AF` SCASW,
`67 AF` SCASD, and `67 48 AF` SCASQ. Effective REX.W determines AF width;
a legacy prefix after REX discards that earlier REX, so `67 48 66 AF` is word
and `67 66 48 AF` is qword. REP/REPNE, segment overrides, repeated `67`, and
effective non-W REX on AF remain outside this SCAS profile. These are local
profile boundaries, not universal #UD or precedence claims.

The starting linear address is zero-extended EDI with flat ES. SCAS performs
one continuous width-sized read from that address; bytes do not wrap
individually at 4 GiB, so a qword at EDI=`ffffffff` reaches `1_0000_0006`.
After a successful read, the accumulator is compared with memory using CMP
semantics, all six arithmetic flags and their definedness are committed, and
RDI becomes zero-extended `(EDI +/- width) mod 2^32` according to DF. Full RAX
is preserved, including SCASD. RSI, RCX, DF, RAM, and unrelated state are
preserved.

Non-REP SCAS has no REPE entry-flags gate: all supported input arithmetic-flag
values and definedness masks are accepted. A successful SCAS defines all six
arithmetic flags. A failed read preserves the full pre-state, including
RFLAGS definedness and the untruncated RDI; it does not retire. Backend read
errors remain HarnessError and architectural faults remain guest faults under
the local model. TEST→SCAS success defines AF; TEST→read-error keeps AF
undefined. This local behavior does not claim hardware restart state or fault
priority. JIT/native execution and physical acceptance remain deferred.

### Local S2-18n non-REP address-size-32 CMPS contract

The bounded Long64 profile covers non-REP `67 A6` CMPSB, `67 66 A7` CMPSW,
`67 A7` CMPSD, and `67 48 A7` CMPSQ. The effective address size zero-extends
ESI and EDI for the two implicit flat accesses. Each operand is one contiguous
width-sized read, including a read that crosses 4 GiB; its bytes do not wrap
individually. The operation reads DS:[ESI] first and ES:[EDI] second, computes
`memory[ESI] - memory[EDI]`, then commits CMP flags and zero-extended
`(ESI/EDI +/- width) mod 2^32` according to DF. RSI/RDI upper halves therefore
become zero only on successful retirement. RAX, RCX, RAM, and unrelated state
are preserved.

If the first read fails, the second is not attempted. If the second read
fails, the first read remains observable in the local access trace, while no
CPU state or retirement is committed. Missing mappings and backend errors
remain HarnessError; architectural faults remain guest faults under the local
model. The ESI-first order is a bounded local policy and does not establish
hardware priority when both operands are invalid. No MMIO rollback is claimed.
TEST→CMPS success defines all arithmetic flags; a read error retains TEST's
undefined AF. The 63 literal schema-v2 fixtures cover success, independent
and dual unmapped reads, zero budget, pointer wraps, continuous 4 GiB reads,
normalization, TEST, CLD/STD, aliases, and dual-invalid local ordering. JIT,
native, Mac mini, and physical acceptance remain deferred; ISSUE-020 stays
open. Core regressions additionally cover 55,296 successful full-state cases,
512 flag-definedness masks, 576 injected read failures, 192 boundaries, 72
aliases, 36 overlaps, and 128 dual-invalid combinations (four distinct
source/destination error pairs × four widths × both DF values × two high-half
choices for each pointer). Retry coverage changes source RAM after a second-read
failure and checks that both operands are re-read.

### Local S2-18o non-REP address-size-32 MOVS contract

The bounded Long64 interpreter profile admits `67 A4` MOVSB, `67 66 A5`
MOVSW, `67 A5` MOVSD, and `67 48 A5` MOVSQ. Prefix normalization also admits
`48 67 A5` as dword, `67 48 66 A5` as word, and `67 66 48 A5` as qword,
matching the already bounded string-operation decoder profile. Other REP,
segment, repeated address-size, and non-W REX combinations remain unsupported
by this slice; these are local profile limits, not universal #UD claims.

Each initial address is independently zero-extended from ESI and EDI. Execution
performs one full source read followed by one width-sized destination write;
source and destination byte ranges remain contiguous across 4 GiB. The source
value is captured before writing, so overlapping ranges use the original bytes.
After a successful write, ESI and EDI become zero-extended
`(ESI/EDI +/- width) mod 2^32` according to DF. RIP advances by decoded length
and the instruction retires once. RAX, RCX, arithmetic flags and their
definedness, and unrelated state remain unchanged. RCX=0 does not suppress this
non-REP operation.

On source failure, destination access is not attempted. On destination failure,
the source read remains visible in the local access trace, but no CPU state or
retirement commits; the bounded GuestMemory write contract prevents RAM
mutation on a failed write. This is local all-or-error memory policy under
[ISSUE-016](../KNOWN_ISSUES.md#issue-016--не-установлен-restart-state-movs-после-destination-fault),
not a hardware restart-state, MMIO rollback, or LOCK/SMP atomicity claim.
Source-first ordering when both addresses are invalid is separately tracked by
[ISSUE-021](../KNOWN_ISSUES.md#issue-021--не-подтверждён-приоритет-dual-invalid-для-movs-rsirdi)
and does not assert hardware exception priority. The 73 literal schema-v2
fixtures cover width/DF, unmapped read/write, four read-only destination
errors, zero budget, wraps, continuous cross-4-GiB ranges, accepted prefix
normalization, TEST and CLD/STD composition, aliases, partial overlaps,
dual-invalid source-first behavior, and read-only source/write-only
destination. Core tests cover stop-RIP separately. Core matrices cover
55,296 full-state successes, 512 definedness masks, 192 pointer boundaries,
576 access failures, 72 aliases, 36 overlaps and 32 dual-invalid combinations.
JIT/native and physical support remain closed pending shared final acceptance.


### Local S2-19s bounded REPNE SCASW/D/Q address-size-32 contract

The decoder admits only Long64 `F2`+`67` `AF` width forms. W is
`F2 67 66 AF`; D is `F2 67 AF` or `67 F2 AF`; Q is `F2 67 48 AF` with
effective REX.W `48..4F`. Legacy ordering also normalizes
`F2 67 48 66 AF` to W, `F2 67 66 48 AF` to Q, and `F2 48 67 AF` to D
because the later legacy prefix clears REX. F2 A7, addr64 F2, other byte F2 AF,
LOCK, segment, duplicate groups and effective non-W REX remain unsupported.
Existing F2 addr32 SCASB and F3 SCAS profiles remain unchanged. The legacy
entry APIs fail closed; callers use `run_with_rep_continuation`.

Before ECX or memory access, all six arithmetic flags must be defined; concrete
RFLAGS are unrestricted. ECX=0 performs no data access, preserves full
RCX/RDI/RAX/RFLAGS/definedness, advances RIP and retires once. Each nonzero
iteration performs one width-sized contiguous read at zero-extended EDI, computes
accumulator-width minus memory CMP flags, then commits flags, EDI ± width modulo
2^32, and zero-extended ECX-1. A crossing operand remains one contiguous access
across 4 GiB. REPNE completes at ECX=0 or ZF=1 before STOP and then the 1024
successful-read cap.

The opaque token records SCAS/CMPS kind, operand/address size, decoded instruction
length, full SCAS RAX or CMPS RSI checkpoint, shared RCX/RDI/RFLAGS/definedness/
context and original flags/mask. Equivalent prefix encodings of equal semantics
and length can resume. Cross-kind/width/address/length mismatches fail before
data access. Architectural fetch/data faults restore entry flags/mask, retain
previously committed count/index progress and clear the token; backend errors
preserve checkpoint/token. The token is caller-owned only; schema v2 does not
serialize it. No hardware fault priority or backend side-effect rollback is
claimed.

Local tests include independent CMP calculations across W/D/Q, DF, starting ZF,
equality/mismatch/count; high halves and EDI truncation; 4-GiB contiguous/tail
behavior; fault rollback/retry; STOP and 1024-limit resume; token identity; and
old API rejection. Literal fixture snapshots use a serialization-only generator.
JIT/native and physical x86 acceptance remain unverified. See the [plan](../plans/STAGE_2.md#s2-19s--bounded-repne-scaswdq-с-address-size-32),
[interpreter notes](../testing/INTERPRETER.md#repne-scaswdq-address-size-32--s2-19s),
[fixture notes](../../tests/fixtures/x86/stage2/README.md#bounded-repne-scaswdq-address-size-32-fixtures-s2-19s),
and [report](../reports/STAGE_2_REPORT.md#s2-19s--bounded-repne-scaswdq-addr32).

### Local S2-19t bounded REPNE CMPSW/D/Q address-size-32 contract

The decoder admits only Long64 `F2`+`67` `A7` width forms normalized by the
S2-19s wide SCAS prefix allowlist: D is `F2 67 A7` or `67 F2 A7`; W is
`F2 67 66 A7` or `67 F2 66 A7`; Q is `F2 67 REX.W A7` or
`67 F2 REX.W A7`, where effective REX.W is any `48..4F`. Legacy ordering
also admits `F2 REX.W 67 A7` as D, `F2 67 REX.W 66 A7` as W, and
`F2 67 66 REX.W A7` as Q because the later legacy prefix resets REX. Existing
REPNE CMPSB and F3 profiles remain unchanged. Other prefixes/repeats,
address-size-64 F2, segments, LOCK, effective non-W REX and other F2 CMPS
forms remain unsupported. The explicit continuation API is the only runner;
older entry APIs fail closed and JIT/native admission stays closed.

All arithmetic flag bits must be defined before ECX or data reads; their
concrete values are unrestricted. ECX=0 preserves full RCX/RSI/RDI/RFLAGS and
definedness and performs no data access. Otherwise each iteration reads one
width-sized contiguous operand at zero-extended ESI, then one at zero-extended
EDI, compares source−destination, and commits CMP flags, both modulo-32 indices
adjusted by ±width according to DF, and zero-extended ECX−1 only after both
reads succeed. Neither operand wraps at 4 GiB. Completion on ECX=0 or ZF=1
precedes STOP and the 1024-success work limit.

The caller-owned opaque token records CMPS kind, width, address size, decoded
instruction length, RSI checkpoint and shared RCX/RDI/RFLAGS/definedness/context
plus original flags/mask. State is checked before callbacks/fetch; decoded
identity is checked before data. Same-length equivalent prefix reorder is
accepted; cross-kind/width/address/length is rejected. Architectural read
faults restore entry flags/mask while retaining prior completed progress and
clear the token. Backend errors preserve checkpoint/token and retry rereads
both operands. Source-first and dual-invalid behavior are local ISSUE-020
policy only; no hardware-priority or MMIO rollback claim is made. Schema v2
does not serialize the token. Literal W/D/Q fixture snapshots are generated
from handwritten bytes and expected state by serialization only. See the
[S2-19t plan](../plans/STAGE_2.md#s2-19t--bounded-repne-cmpswdq-addr32),
[interpreter notes](../testing/INTERPRETER.md#repne-cmpswdq-address-size-32--s2-19t),
[fixture notes](../../tests/fixtures/x86/stage2/README.md#bounded-repne-cmpswdq-address-size-32-fixtures-s2-19t),
and [report](../reports/STAGE_2_REPORT.md#s2-19t--bounded-repne-cmpswdq-addr32).

## Local legacy MOVSS register slice — S2-34.8a

Profile `s2-long-movss-reg-v1` admits only `F3 0F 10 /r` with ModR/M.mod=3,
one effective F3 mandatory prefix, and optionally one REX immediately after F3
and before `0F`; only REX.R/B are supported, selecting XMM0–XMM15. It copies
raw `SRC[31:0]` into `DEST[31:0]` and preserves old `DEST[127:32]` and all
other state. Intel SDM revision 093 Vol. 2B MOVSS distinguishes this register
form from `F3 0F 10 /r` with a memory source, which clears destination bits
127:32. The prefix F3 selects this opcode form; it is not REP behavior. No data
memory callback occurs. Memory operands, VEX/EVEX, F2/66, `F3 0F 11`, prefix
permutations, control-register/feature gates, #UD/#NM delivery, and native/JIT
admission are outside this local profile.

Source: [Intel SDM revision 093 Vol. 2B PDF](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf),
[official Intel SDM page](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html).

## Local reverse-direction legacy MOVSS register slice — S2-34.8b

Отдельный exact profile `s2-long-movss-store-reg-v1` принимает только legacy
`F3 0F 11 /r` с ModR/M.mod=3. F3 обязателен; допускается максимум один
effective REX.R/B непосредственно после F3 и перед `0F`; REX.W/X и другие
префиксы отвергаются. XMM индексы — 0–15. Для `0F 11`, ModR/M.reg обозначает
source XMM, а ModR/M.r/m — destination XMM. Копируются raw bits 31:0;
destination bits 127:32 сохраняются. Decoder operation сохраняет нормальную
семантическую пару `{ destination, source }` независимо от направления opcode.

Intel SDM revision 093 Vol. 2B MOVSS `F3 0F 11 /r`, Op/En C, задаёт
`xmm2/m32, xmm1`: ModR/M.r/m — destination, ModR/M.reg — source; при двух
XMM operands старшие 96 destination bits не изменяются. F3 здесь обязательный
opcode prefix, не REP; RCX и DF не участвуют, data-memory callbacks отсутствуют.
Memory forms, `F3 0F 10`, VEX/EVEX, F2/66, prefix permutations, feature gates,
#UD/#NM delivery, native/JIT и physical CPU execution не входят в этот local
profile. См. [официальный Intel SDM revision 093 Volume 2B PDF]
(https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf) и
[страница Intel SDM](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html).

## Local legacy MOVSS m32 load profile — S2-34.8c

Exact fixture profile `s2-long-movss-mem-load-v1` admits only legacy
`F3 0F 10 /r` with one effective F3, optionally followed by one REX before
`0F`, and ModR/M.mod=00/rm=000 (`[RAX]`). REX.R may extend the destination
index to XMM8–XMM15. REX.W/X/B, SIB, displacement, RIP-relative, other bases,
register sources, store direction, other mandatory prefixes, VEX and EVEX are
excluded. The source is exactly four bytes; a successful read sets destination
bits 31:0 from memory and clears bits 127:32. Unrelated state is preserved.

The decoder uses the shared `MemoryOperand` address-range validation. The
executor makes one four-byte `GuestMemory::read`, and updates XMM/RIP only after
that operation succeeds. This describes the local backend contract; it does
not claim architectural fault priority, guest exception delivery, alignment
fault behavior, or page-fault precision. Truncated instruction fetch produces
no data-memory callback in the local test backend.

Intel SDM revision 093, Vol. 2B, Order Number 253667-093US (September 2026),
MOVSS table identifies `F3 0F 10 /r`, Op/En A, `MOVSS xmm1,m32`; the legacy
memory-source operation clears `DEST[127:32]` (pp. 4-117–4-119). Source:
[official Intel PDF](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf) and
[Intel SDM page](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html).
SSE and CR0/CR4 preconditions are assumed. Feature gates, CPUID delivery,
#UD/#NM delivery, native/JIT and physical execution remain unimplemented.

## Local legacy MOVSS m32 store profile — S2-34.8d

Exact fixture profile `s2-long-movss-mem-store-v1` admits only legacy
`F3 0F 11 /r` with ModR/M.mod=00/rm=000 (`[RAX]`). One effective REX.R may
follow F3 before `0F` and extend the ModR/M.reg source to XMM8–XMM15. REX.W/X/B,
SIB, displacement, RIP-relative, other bases, register destination, F2/66,
prefix permutations, VEX and EVEX are excluded. A successful operation writes
exactly the source XMM low four bytes through the shared `MemoryOperand` path;
it does not modify XMM or other CPU state. The local executor commits RIP only
after `GuestMemory::write` succeeds.

Intel SDM revision 093, Vol. 2B, Order Number 253667-093US (September 2026),
MOVSS table identifies `F3 0F 11 /r`, Op/En C, `MOVSS xmm2/m32,xmm1`;
ModR/M.r/m is the destination and ModR/M.reg the source. The memory-store
operation copies `SRC[31:0]` to `DEST[31:0]` (pp. 4-117–4-119). Sources:
[official Intel PDF](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf) and
[Intel SDM page](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html).

The tests establish the exact profile's decoder allowlist, one-write/no-read
callback contract, four-byte range validation, and CPU-state rollback on local
backend write failure. They do not establish architectural fault priority,
guest exception delivery, alignment guarantees, or page-fault precision.
SSE/CR0/CR4 prerequisites are assumed. Native/JIT, CPUID/feature gates,
#UD/#NM delivery and physical execution remain outside scope.

## Local legacy scalar-double MOVSD register profile — S2-34.9a

The exact local profile `s2-long-movsd-reg-v1` accepts only legacy
`F2 0F 10 /r` with ModR/M.mod=3 and XMM indices 0–15. The mandatory F2 may be
followed by at most one REX immediately before `0F`; only REX.R and REX.B are
allowed. ModR/M.reg selects the destination, and ModR/M.r/m selects the
source. Execution copies the raw source bits 63:0 into destination bits 63:0
and preserves destination bits 127:64. It does not interpret floating-point
values, change flags, consume RCX, consult DF, or access data memory. F2 is the
mandatory opcode prefix, not REPNE. This scalar SIMD instruction is distinct
from string `MOVSD` opcode `A5`, which remains outside this profile.

Intel SDM revision 093, Vol. 2B, Order Number 253667-093US (September 2026),
MOVSD entry `F2 0F 10 /r`, Op/En A, assigns ModR/M.reg as the destination and
ModR/M.r/m as the source. The legacy register operation copies `SRC[63:0]` to
`DEST[63:0]` and leaves `DEST[MAXVL:64]` unmodified (pp. 4-108–4-110). See
the [official Intel PDF](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf) and
[Intel SDM page](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html).
The local profile assumes explicit SSE2 state and required architectural
preconditions; it does not advertise CPUID, implement feature gates, or deliver
`#UD`/`#NM`. Memory forms, `0F 11`, MOVSS/MOVUPS, VEX/EVEX, native/JIT and
physical execution are outside the profile.

## Local reverse-direction scalar-double MOVSD register profile — S2-34.9b

The exact local profile `s2-long-movsd-store-reg-v1` accepts only legacy
`F2 0F 11 /r` with ModR/M.mod=3. F2 is mandatory; optionally one REX byte may
follow F2 immediately before `0F`, with only REX.R/B allowed. These bits select
XMM0–XMM15. ModR/M.reg is the source and ModR/M.r/m the destination. Execution
copies raw `SRC[63:0]` into `DEST[63:0]` and preserves `DEST[127:64]` and all
other state. It does not interpret floating-point values, change flags, consume
RCX, consult DF or access data memory. F2 is the opcode prefix, not REPNE.
String MOVSD opcode `A5` remains a distinct unsupported instruction here.

Intel SDM revision 093, Vol. 2B, MOVSD entry `F2 0F 11 /r`, Op/En C,
specifies `MOVSD xmm2/m64, xmm1`: ModR/M.r/m is destination and ModR/M.reg is
source; the legacy register form updates only the destination low qword
(pp. 4-108–4-110). See the [official Intel PDF]
(https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf) and [Intel SDM page]
(https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html).
The isolated profile rejects memory forms, F3/66/other prefixes, VEX/EVEX,
and prefix permutations. SSE2 state and architectural preconditions are
assumed; feature gates, `#UD`/`#NM`, native/JIT and physical execution are not
implemented.

## Local legacy scalar-double MOVSD memory-load profile — S2-34.9c

The exact profile `s2-long-movsd-mem-load-v1` accepts only legacy
`F2 0F 10 /r` with ModR/M.mod=00 and r/m=000, addressing `[RAX]`. F2 is
mandatory; optionally one REX byte may follow it immediately before `0F`, with
only REX.R allowed (including a REX byte with no extension bits). ModR/M.reg
and REX.R select XMM0–XMM15. Other bases, SIB, displacement, REX.W/X/B,
segment/address/operand prefixes, LOCK, duplicate prefixes/REX, `0F 11`, VEX,
EVEX and string MOVSD `A5` are outside this exact schema.

Intel SDM revision 093, Vol. 2B, MOVSD entry `F2 0F 10 /r`, Op/En A,
specifies ModR/M.reg as the XMM destination and ModR/M.r/m as the m64 source.
The legacy memory-load operation copies `SRC[63:0]` to `DEST[63:0]`, clears
`DEST[127:64]`, and leaves `DEST[MAXVL:128]` unmodified (pp. 4-108–4-110).
See the [official Intel PDF](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf)
and [Intel SDM page](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html).
F2 is the opcode prefix, not REPNE; string MOVSD opcode `A5` remains separate.

Execution validates the complete canonical48 `[RAX..RAX+7]` range before one
8-byte read. It commits the raw low qword, clears the destination upper qword,
and advances RIP only after that read succeeds. Other XMM registers, x87,
MXCSR, GPR and RFLAGS are preserved. This local behavior does not claim
hardware fault priority, precise guest exception delivery, alignment behavior,
feature gates, CPUID support or native/JIT/physical execution. SSE2 state and
architectural preconditions are assumed. See the [S2-34.9c plan]
(../plans/STAGE_2.md#s2-349c--local-legacy-scalar-double-movsd-m64-load-from-rax).

## Local legacy scalar-double MOVSD memory-store profile — S2-34.9d

The exact profile `s2-long-movsd-mem-store-v1` admits only legacy
`F2 0F 11 /r` with ModR/M.mod=00 and r/m=000 (`[RAX]`). F2 is mandatory; an
optional single REX byte may follow F2 immediately, with REX.R as its only
permitted extension bit. ModR/M.reg plus REX.R selects source XMM0–XMM15.
Other addressing, segment/address/operand prefixes, LOCK, duplicate prefixes
or REX, REX.W/X/B, `0F 10`, VEX, EVEX, and string MOVSD `A5` are outside this
profile. This store direction remains separate from register reverse-direction
MOVSD `0F 11`.

Intel SDM revision 093, Vol. 2B, MOVSD `F2 0F 11 /r`, Op/En C, identifies
ModR/M.r/m as the write destination and ModR/M.reg as the XMM source. The
legacy operation stores only `SRC[63:0]` to m64. The interpreter validates the
full canonical48 `[RAX..RAX+7]` range, then issues exactly one eight-byte
`GuestMemory::write` from the source low qword. It does not read data memory or
mutate XMM, x87, MXCSR, GPR, or flags; RIP advances only after successful
write. Backend errors preserve CPU state as local no-commit behavior, without
claiming hardware fault priority or guest fault delivery. See the [official
Intel SDM page](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html)
for the revision-093 manual set and [Vol. 2B MOVSD entry](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf).

The literal XMM8 fixture and interpreter tests verify exact write bytes,
surrounding canaries, all source indices, error and execution boundaries, and
strict profile rejection. SSE2 state and architectural preconditions are
assumed. This is local interpreter/schema evidence only; alignment or hardware
fault ordering, CPUID, native/JIT, and physical execution are not covered.

## S2-34 closure inventory

The following matrix is an inventory of current bounded local profiles and
open completion work. It does not define the generic CPU profile or close
parent S2-34. Exact allowlists remain in the individual profile sections above.

| Status/group | Exact forms or open scope | Requirements/evidence and next step |
|---|---|---|
| Implemented-local: `MOVUPS` | Exact legacy `0F 10 /r` and `0F 11 /r`: register `mod=3`; m128 `[RAX]` `mod=00,r/m=000`; 64-bit base+disp `mod=00/01/10` excluding RIP/SIB selectors; RIP-relative `mod=00,r/m=101`; SIB forms. Bare or one profile-allowed REX, exact limits in 6a–6c. | Register forms have no data fault/alignment access. Memory forms use one 16-byte local callback, with no CPU commit on backend failure. Alignment, guest fault delivery/priority, feature gates, native/JIT and physical behavior are not established. Local fixtures/matrices exist; parent native-oracle register and memory evidence remains due. Physical SSE is `not_run` because current physical harness admits integer profiles only. No new local slice is needed. |
| Implemented-local: `MOVAPS` / `MOVAPD` | Register-only MOVAPS `0F 28 /r`, reverse-direction MOVAPS `0F 29 /r`, MOVAPD `66 0F 28 /r`, and reverse-direction MOVAPD `66 0F 29 /r`, each ModR/M.mod=3 with a separate exact local profile. | The profiles copy 128 raw XMM bits without data-memory access and preserve unrelated state. SSE/SSE2 preconditions are assumed; local matrices/fixtures exist. Memory alignment/fault behavior, SSE exception gates, JIT/native, physical execution and parent acceptance remain open. These register-only forms imply no aligned-memory behavior. |
| Implemented-local: packed logic | `0F 57` `XORPS`, `0F 54` `ANDPS`, `0F 56` `ORPS`, `0F 55` `ANDNPS`, `/r, mod=3`; bare or one REX with R/B extension, no other legacy prefixes. | Raw XMM logic, no memory/alignment/fault path; SSE and CR0/CR4 conditions are assumed. Local semantic/fixture evidence exists; native-oracle confirmation and profile consistency remain open. Physical SSE is `not_run`. |
| Implemented-local: `MOVSS` | `F3 0F 10 /r` and `F3 0F 11 /r`: register `mod=3` both directions and m32 `[RAX]` `mod=00,r/m=000` load/store; mandatory F3 and at most one optional profile-allowed immediate REX (8a–8d). | Register low-dword transfer preserves destination upper 96 bits; memory load clears them. One 4-byte local callback; local rollback only. Alignment, guest exception ordering/delivery and control gates are unverified. Native oracle remains due; physical SSE is `not_run`. |
| Implemented-local: `MOVSD` | `F2 0F 10 /r` and `F2 0F 11 /r`: register `mod=3` both directions and m64 `[RAX]` `mod=00,r/m=000` load/store; mandatory F2 and at most one optional profile-allowed immediate REX (9a–9d). Scalar-double SIMD, not string opcode `A5`. | Register low-qword transfer preserves destination upper 64 bits; memory load clears them. One 8-byte local callback; local rollback only. Alignment, guest exception ordering/delivery and control gates are unverified. Native oracle remains due; physical SSE is `not_run`. |
| Planned/unimplemented | Additional moves and logical forms only as selected by the eventual profile; parent plan identifies aligned/unaligned, scalar/packed and upper-lane distinctions, but does not authorize widening these exact fixture profiles. | For each chosen form record state preconditions, fault and alignment semantics, access width/commit ordering, then add decode negatives, state/lane matrices, boundary/fault tests and native-oracle evidence. Next slice is one explicitly named encoding with a closed local allowlist. |
| Excluded in current local profiles | Prefixes, REX combinations and addressing forms outside each exact profile; VEX/EVEX; feature/control faults; FP arithmetic/MXCSR instruction behavior; native/JIT. | No behavior is claimed. Local no-commit and callback tests do not establish hardware fault order, guest exception delivery, alignment behavior or atomicity. Keep exclusions until the parent profile selects them. |
| Profile-undecided/system work | Generic CPU/CPUID/MSR/boot profile, SSE/SSE2 exposure and gates, MXCSR supported/reserved mask, and system/save-restore integration. | Local profiles assume SSE/SSE2 and relevant CR0/CR4 preconditions; they do not implement `#UD/#NM/#GP` delivery and no universal MXCSR mask is implied. The owning profile/system tasks must establish coherent advertised state and requirements; preserve S2-35, S2-36, S2-38 and S2-39 dependencies. |

A bounded diagnostic slice may start once one exact encoding and its local
state, assumptions and failure/no-commit contract are written down and local
semantic plus literal fixture evidence is available. It does not depend on
settling generic CPUID/MSR/boot policy. Parent S2-34 completion still requires
the selected profile's full state/gate/fault/alignment requirements and native
oracle results for real register and memory cases. The current physical
harness is integer-only, so physical SSE status is `not_run`. Keep the Mac
mini/SSH batch at the end-of-stage acceptance; this inventory neither moves
nor executes it. S2-34 remains open.

## Local register-only `PXOR` profile — S2-35.1

The exact fixture profile `s2-long-pxor-reg-v1` admits only legacy
`66 [one optional REX.R/B] 0F EF /r` with ModR/M.mod=3. The mandatory 66
selects the XMM form; bare `0F EF` is MMX and is rejected. ModR/M.reg is the
XMM destination and ModR/M.r/m is the source, extended independently to
XMM0–XMM15 by REX.R/B. Execution XORs the 128 raw destination bits with source
bits and preserves unrelated state, including MXCSR, x87, GPR, RFLAGS and the
other XMM registers. No data-memory callback is made.

Intel SDM revision 093 Vol. 2B, PXOR entry lists unprefixed `0F EF /r` as MMX
and `66 0F EF /r` as SSE2, with ModR/M destination/source semantics. Source:
[Intel SDM revision 093 Vol. 2B PDF](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf)
and [official Intel SDM page](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html).
The diagnostic fixture assumes SSE2 availability, CR0.EM=0, CR0.TS=0 and
CR4.OSFXSR=1. It makes no CPUID claim and does not model #UD/#NM gates or
delivery. Memory forms, other prefixes, REX.W/X, VEX/EVEX, native/JIT and
physical execution remain unsupported. This child slice leaves S2-34, S2-35
and their global dependency open.

## Local register-only `PADDB` profile — S2-35.2

The exact profile `s2-long-paddb-reg-v1` admits only legacy
`66 [one optional REX.R/B] 0F FC /r` with ModR/M.mod=3. The mandatory `66`
selects the XMM/SSE2 form; bare `0F FC` is MMX and is rejected. ModR/M.reg is
the XMM destination and ModR/M.r/m is the source, each extended independently
to XMM0–XMM15 by REX.R/B. Each corresponding byte lane is added modulo 256,
with no carry between lanes and no saturation. All other XMM, x87, MXCSR, GPR,
RFLAGS and memory state is preserved, and execution makes no data-memory
callback.

The encoding and operand order follow Intel SDM revision 093 Vol. 2B, PADDB
entry (`66 0F FC /r`; `xmm1` destination, `xmm2/m128` source):
[official Intel SDM revision 093 Vol. 2B PDF](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf),
[Intel SDM page](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html).
The profile assumes SSE2 availability, CR0.EM=0, CR0.TS=0 and CR4.OSFXSR=1;
it does not query CPUID or model `#UD/#NM` or control-register faults. Memory
forms, PADDW/PADDD/PADDQ, saturating operations, other prefixes, REX.W/X,
VEX/EVEX, native/JIT and physical execution are excluded. S2-34, S2-35 and the
global dependency `S2-35 → S2-34` remain open. See the
[S2-35.2 plan](../plans/STAGE_2.md#s2-352--local-only-bounded-register-paddb).


## Local register-only `PSUBB` profile — S2-35.3

The exact profile `s2-long-psubb-reg-v1` admits only legacy
`66 [one optional REX.R/B] 0F F8 /r` with ModR/M.mod=3. Mandatory `66`
selects the XMM/SSE2 form; bare `0F F8` is MMX and rejected. ModR/M.reg is
the XMM destination, ModR/M.r/m the source (XMM0–XMM15). Each byte lane is
`(DEST - SRC) mod 256`, independently, without borrow between lanes or
saturation. Other XMM, x87, MXCSR, GPR, RFLAGS and memory state are preserved;
there are no data callbacks.

The encoding and semantics follow the PSUBB entry in Intel SDM revision 093,
Vol. 2B: [official Intel SDM PDF](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf),
[official Intel SDM page](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html).
The profile assumes SSE2, CR0.EM=0, CR0.TS=0 and CR4.OSFXSR=1 without CPUID
claims or control-fault delivery. Memory, PSUBW/PSUBD/PSUBQ, saturating forms,
other prefixes, REX.W/X, VEX/EVEX, native/JIT and physical execution are
excluded. S2-34 and S2-35 and `S2-35 → S2-34` remain open. See the
[S2-35.3 plan](../plans/STAGE_2.md#s2-353--local-only-bounded-register-psubb).

## Local register-only `PSHUFD` profile — S2-35.35

The exact profile `s2-long-pshufd-reg-v1` admits legacy
`66 [one optional REX.R/B] 0F 70 /r ib` with ModR/M.mod=3. Mandatory `66`
selects the XMM/SSE2 form. ModR/M.reg is destination and ModR/M.r/m is source,
each selecting XMM0–XMM15. For output dword lane `i`, imm8 bits `2*i+1:2*i`
select the source dword copied to that lane. Execution reads all four dwords
from the original source before writing the destination, so aliasing is
well-defined; other XMM, x87, MXCSR, GPR, RFLAGS and memory state is preserved,
and execution makes no data-memory callback.

The encoding and semantics follow the PSHUFD entry in Intel SDM revision 093,
Vol. 2B: [official Intel SDM PDF](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf),
[Intel SDM page](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html).
SSE2 and CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1 are assumptions, with no CPUID or
control-fault claim. Memory, other prefixes, REX.W/X, VEX/EVEX, native/JIT and
physical execution are excluded. S2-34, S2-35 and global `S2-35 → S2-34` remain
open. See the [S2-35.35 plan](../plans/STAGE_2.md#s2-3535--local-only-bounded-register-pshufd).

## Local register-only immediate `PSLLW` profile — S2-35.37

The diagnostic profile `s2-long-psllw-imm-reg-v1` admits exactly
`66 [optional REX.B] 0F 71 /6 ib`, ModR/M.mod=3. `/6` is required and
ModR/M.r/m selects XMM0–15. Each little-endian u16 lane shifts logically left
by the unsigned imm8; counts 0–15 retain the low 16 bits and counts >=16 zero
all lanes without count masking. No carry crosses lanes; flags and MXCSR are
unchanged. SSE2 and CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1 are assumptions only.
Memory/MMX, REX.R/X/W, other shifts, VEX/EVEX, feature gating, and exception
delivery are outside this profile. See the [S2-35.37 plan](../plans/STAGE_2.md#s2-3537--local-only-bounded-register-immediate-psllw).

## Local register-only immediate `PSLLD` profile — S2-35.41

The diagnostic profile `s2-long-pslld-imm-reg-v1` admits exactly
`66 [no REX or optional 41] 0F 72 /6 ib`, ModR/M.mod=3. `/6` is required and
ModR/M.r/m selects XMM0–15; `REX.B` extends the destination. Four independent
little-endian u32 lanes shift logically left by unsigned imm8. Counts 0–31
retain the low 32 result bits; counts >=32 clear all lanes without count
masking. No carry crosses dwords; flags and MXCSR are unchanged. The
implementation assumes SSE2 and CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1. Memory/MMX,
REX.R/X/W, extra prefixes, VEX/EVEX, feature gating, exception delivery,
JIT/native execution, and physical behavior are outside this profile. See the
[S2-35.41 plan](../plans/STAGE_2.md#s2-3541--local-only-bounded-register-immediate-pslld).

## Local register-only immediate `PSRLQ` profile — S2-35.45

The diagnostic profile `s2-long-psrlq-imm-reg-v1` admits exactly
`66 [no REX or optional 41] 0F 73 /2 ib`, ModR/M.mod=3. `/2` selects PSRLQ;
`/6` PSLLQ and `/3` PSRLDQ are rejected. ModR/M.r/m selects XMM0–15, with
REX.B extending the destination. Two independent little-endian u64 lanes shift
logically right by unsigned, unmasked imm8: counts 0–63 zero-fill and counts
>=64 clear both lanes. Bits never cross qword boundaries. Flags and MXCSR are
unchanged. SSE2 and CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1 are assumptions only.
Memory/MMX forms, REX.R/X/W, additional prefixes, VEX/EVEX, feature gating,
exception delivery, JIT/native execution, and physical behavior are excluded.
See the [S2-35.45 plan](../plans/STAGE_2.md#s2-3545--local-only-bounded-register-immediate-psrlq).

## Local register-only immediate `PSLLQ` profile — S2-35.44

The diagnostic profile `s2-long-psllq-imm-reg-v1` admits exactly
`66 [no REX or optional 41] 0F 73 /6 ib`, ModR/M.mod=3. `/6` selects PSLLQ,
`/7` PSLLDQ is rejected, and ModR/M.r/m selects XMM0–15. Two independent
little-endian u64 lanes shift logically left by unmasked unsigned imm8: counts
0–63 retain the low 64 bits and counts >=64 clear both lanes. Lane bits never
carry between qwords. Flags and MXCSR are unchanged. SSE2 and CR0.EM=0,
CR0.TS=0, CR4.OSFXSR=1 are assumptions only. Memory/MMX, bare REX, REX.R/X/W,
extra prefixes, VEX/EVEX, feature gating, exception delivery, JIT/native
execution, and physical behavior are outside this profile. See the
[S2-35.44 plan](../plans/STAGE_2.md#s2-3544--local-only-bounded-register-immediate-psllq).

## Local register-only immediate `PSLLDQ` profile — S2-35.46

The diagnostic profile `s2-long-pslldq-imm-reg-v1` admits exactly
`66 [no REX or optional 41] 0F 73 /7 ib`, ModR/M.mod=3. ModR/M.r/m selects
destination XMM0–15. `/7` selects PSLLDQ; `/6` PSLLQ and `/3` PSRLDQ are
separate forms. The imm8 is an unsigned, unmasked byte count. For count below
16, result byte `i` is original destination byte `i-count` when `i >= count`,
else zero. Count >=16 clears all 16 bytes. Flags and MXCSR are unchanged.
SSE2 and CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1 are assumptions only. Bare REX 40,
REX.R/X/W, other prefixes, memory destination, and VEX/EVEX forms are outside
the profile.

See the [S2-35.46 plan](../plans/STAGE_2.md#s2-3546--local-only-bounded-register-immediate-pslldq).

## Local register-only immediate `PSRLDQ` profile — S2-35.47

The diagnostic profile `s2-long-psrldq-imm-reg-v1` admits exactly
`66 [no REX or optional 41] 0F 73 /3 ib`, ModR/M.mod=3. ModR/M.r/m selects
destination XMM0–15, with REX.B extending the register. `/3` selects PSRLDQ;
`/2` PSRLQ and `/7` PSLLDQ are separate forms. The imm8 is an unsigned,
unmasked byte count. For count below 16, result byte `i` is original
destination byte `i+count` when that source index is below 16, else zero.
Count >=16 clears the full XMM. The operation uses an original-value snapshot;
flags and MXCSR are unchanged. SSE2 and CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1 are
assumptions only. Bare REX 40, REX.R/X/W, extra prefixes, memory/MMX, VEX/EVEX,
and JIT/native execution are outside this profile.

See the [S2-35.47 plan](../plans/STAGE_2.md#s2-3547--local-only-bounded-register-immediate-psrldq).

## Local register-only immediate `PSRLD` profile — S2-35.42

The diagnostic profile `s2-long-psrld-imm-reg-v1` admits exactly
`66 [no REX or optional 41] 0F 72 /2 ib`, ModR/M.mod=3. `/2` is required;
`/6` selects the separate PSLLD sibling and is rejected by this PSRLD profile.
ModR/M.r/m selects XMM0–15. Four independent little-endian u32 lanes shift
logically right by unsigned, unmasked imm8: counts 0–31 zero-fill and counts
>=32 clear the lane. Lane bits never carry between dwords. Flags and MXCSR are
unchanged. SSE2 and CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1 are assumptions only.
Memory/MMX forms, REX.R/X/W, additional prefixes, VEX/EVEX, feature gating,
exception delivery, JIT/native execution, and physical behavior are excluded.
See the [S2-35.42 plan](../plans/STAGE_2.md#s2-3542--local-only-bounded-register-immediate-psrld).

## Local register-only immediate `PSRLW` profile — S2-35.38

The diagnostic profile `s2-long-psrlw-imm-reg-v1` admits exactly
`66 [optional REX.B] 0F 71 /2 ib`, ModR/M.mod=3. `/2` is required and
ModR/M.r/m selects XMM0–15; `/6` is the separate PSLLW selector and is rejected
by this profile. Each little-endian u16 lane shifts logically right by unsigned
imm8; counts 0–15 shift and zero-fill, while counts >=16 clear all lanes
without count masking. Lane bits never carry across words. Flags and MXCSR are
unchanged. SSE2 and CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1 are assumptions only.
Memory/MMX, other shifts, REX.R/X/W, VEX/EVEX, feature gating, exception
delivery, JIT/native execution, and physical behavior are outside this profile.
See the [S2-35.38 plan](../plans/STAGE_2.md#s2-3538--local-only-bounded-register-immediate-psrlw).

## Local register-only immediate `PSRAW` profile — S2-35.39

The diagnostic profile `s2-long-psraw-imm-reg-v1` admits exactly
`66 [no REX or optional 41] 0F 71 /4 ib`, ModR/M.mod=3. `/4` is required and
ModR/M.r/m selects XMM0–15. `/2` PSRLW and `/6` PSLLW are separate selectors
and are rejected by this profile. Each signed i16 lane shifts arithmetically
right by unsigned imm8: counts 0–15 sign-fill and counts >=16 yield all ones
for negative inputs or zero for nonnegative inputs. Flags and MXCSR are
unchanged. SSE2 and CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1 are assumptions only.
Memory/MMX, bare REX, REX.R/X/W, VEX/EVEX, feature gating, exception delivery,
JIT/native execution and physical behavior are excluded. Encoding and semantics
follow the PSRAW entry in Intel SDM revision 093, Vol. 2B: [official Intel SDM
PDF](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf),
[Intel SDM page](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html).
See the [S2-35.39 plan](../plans/STAGE_2.md#s2-3539--local-only-bounded-register-immediate-psraw).

## Local register-only immediate `PSRAD` profile — S2-35.43

The diagnostic profile `s2-long-psrad-imm-reg-v1` admits exactly
`66 [no REX or optional 41] 0F 72 /4 ib`, ModR/M.mod=3. `/4` is required;
ModR/M.r/m selects XMM0–15 and optional REX.B extends the destination. `/2`
PSRLD and `/6` PSLLD are separate sibling selectors and are rejected by the
PSRAD fixture profile. Four independent little-endian signed i32 lanes shift
arithmetically right by unsigned, unmasked imm8. Counts 0–31 sign-fill;
counts >=32 produce `ffffffff` for negative lanes and zero otherwise. Flags
and MXCSR are unchanged. SSE2 and CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1 are
assumptions only. Memory/MMX, bare REX, REX.R/X/W, extra prefixes, VEX/EVEX,
feature gating, exception delivery, JIT/native execution and physical behavior
are excluded. See the [S2-35.43 plan](../plans/STAGE_2.md#s2-3543--local-only-bounded-register-immediate-psrad).

## Local register-only `PADDUSB` profile — S2-35.4

The exact profile `s2-long-paddusb-reg-v1` admits legacy
`66 [one optional REX.R/B] 0F DC /r`, ModR/M.mod=3 only. Mandatory `66`
selects XMM/SSE2; bare `0F DC` selects MMX and is rejected. ModR/M.reg is the
destination and ModR/M.r/m the source (XMM0–XMM15). Each byte computes
`min(unsigned(DEST) + unsigned(SRC), 255)` independently. `0xff+0x01` and
`0xff+0xff` yield `0xff`; `0x7f+0x01` yields `0x80`; `0x80+0x80` yields
`0xff`. No carry crosses lanes. State beyond the destination XMM register is
preserved, with no data-memory callbacks.

Encoding, operand order and unsigned saturation follow Intel SDM revision 093,
Vol. 2B, PADDUSB: [official Intel SDM PDF](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf),
[official Intel SDM page](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html).
SSE2, CR0.EM=0, CR0.TS=0 and CR4.OSFXSR=1 are assumptions only. No CPUID or
control-fault claim is made. Memory, PADDUSW/other widths, signed saturation,
other prefixes, REX.W/X, VEX/EVEX, native/JIT and physical execution are
excluded. S2-34, S2-35 and `S2-35 → S2-34` remain open.

## Local register-only PSUBUSB profile — S2-35.5

The exact profile `s2-long-psubusb-reg-v1` admits legacy `66 [one optional REX.R/B] 0F D8 /r` only when ModR/M.mod=3. ModR/M.reg is the XMM destination; ModR/M.r/m is the source. Sixteen unsigned byte lanes independently compute `max(DEST - SRC, 0)` without cross-lane borrow. Mandatory 66 distinguishes the XMM/SSE2 encoding from bare MMX `0F D8`. No memory or other packed widths, signed saturation, extra prefixes, REX.W/X, VEX/EVEX, native/JIT or physical behavior is included. SSE2 availability, CR0.EM=0, CR0.TS=0 and CR4.OSFXSR=1 are assumptions only; CPUID and exception delivery are not modeled.

The encoding and lane semantics follow the PSUBUSB entry in Intel SDM revision 093, Vol. 2B: [official Intel PDF](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf), [Intel SDM manuals index](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html). See the [S2-35.5 plan](../plans/STAGE_2.md#s2-355--local-only-bounded-register-psubusb).

## Local register-only `PADDW` profile — S2-35.6

The encoding and independent 16-bit lane addition semantics follow the PADDW
entry in Intel SDM revision 093, Volume 2B: [official Intel SDM](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf), [Intel SDM index](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html). The exact local profile `s2-long-paddw-reg-v1` admits only `66 [one optional REX.R/B] 0F FD /r` with ModR/M.mod=3. ModR/M.reg is destination, ModR/M.r/m is source. Eight little-endian 16-bit lanes add independently modulo 65536. Bare MMX, other packed widths, saturation, memory, other prefixes, REX.W/X, VEX/EVEX, native/JIT and physical execution are excluded. SSE2 and CR0/CR4 prerequisites are assumptions; there are no CPUID or exception-delivery claims. S2-34, S2-35 and global S2-35 → S2-34 remain open.

## Local register-only `PSUBW` profile — S2-35.7

The exact profile `s2-long-psubw-reg-v1` admits only legacy
`66 [one optional REX.R/B] 0F F9 /r` with ModR/M.mod=3. Mandatory 66 selects
the XMM/SSE2 form; bare `0F F9` is MMX and rejected. ModR/M.reg is destination
and ModR/M.r/m is source, extended only to XMM0–XMM15. Each of eight
little-endian u16 lanes computes `(DEST - SRC) mod 65536`; borrow wraps within
the word and never crosses lanes. Other XMM, x87, MXCSR, GPR, RFLAGS and memory
state are preserved; there are no data-memory callbacks.

Encoding, operand order and lane semantics follow PSUBW in Intel SDM revision
093, Volume 2B: [official Intel SDM PDF](https://cdrdv2-public.intel.com/671110/325383-sdm-vol-2abcd.pdf),
[Intel SDM index](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html).
The profile assumes SSE2 availability, CR0.EM=0, CR0.TS=0 and CR4.OSFXSR=1;
it makes no CPUID or control-fault-delivery claim. Memory, PSUBD/PSUBQ,
saturating forms, other prefixes, REX.W/X, VEX/EVEX, native/JIT and physical
execution are excluded. S2-34, S2-35 and global `S2-35 → S2-34` remain open.
See the [S2-35.7 plan](../plans/STAGE_2.md#s2-357--local-only-bounded-register-psubw).


## Local register-only `PADDUSW` profile — S2-35.8

The exact profile `s2-long-paddusw-reg-v1` admits only legacy
`66 [one optional REX.R/B] 0F DD /r` with ModR/M.mod=3. ModR/M.reg is the
destination and ModR/M.r/m is the source, extended to XMM0–XMM15 by REX.R/B.
Eight independent little-endian unsigned 16-bit lanes compute
`min(DEST + SRC, 65535)`; overflow saturates within that word. The encoding,
operand order, MMX bare opcode distinction and unsigned saturation follow
PADDUSW in Intel SDM revision 093, Volume 2B pp. 4-212–4-214: [official Intel SDM](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf), [Intel SDM index](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html).
The profile assumes SSE2 availability, CR0.EM=0, CR0.TS=0 and CR4.OSFXSR=1;
these are assumptions, with no CPUID or exception-delivery claims. Bare MMX,
memory, other widths, signed saturation, additional prefixes, REX.W/X, VEX/EVEX,
JIT/native and physical execution are excluded. S2-34, S2-35 and global
`S2-35 → S2-34` remain open. See the [S2-35.8 plan](../plans/STAGE_2.md#s2-358--local-only-bounded-register-paddusw).

## Local register-only `PSUBUSW` profile — S2-35.9

The exact profile `s2-long-psubusw-reg-v1` admits only legacy
`66 [one optional REX.R/B] 0F D9 /r` with ModR/M.mod=3. ModR/M.reg is the XMM
destination and ModR/M.r/m the source, extended to XMM0–XMM15. Eight
little-endian unsigned 16-bit lanes independently compute
`max(DEST - SRC, 0)`; borrow saturates at zero within each lane. Bare `0F D9`
is MMX and rejected. Memory, other packed widths, signed saturation, additional
prefixes, REX.W/X, VEX/EVEX, JIT/native and physical execution are excluded.
SSE2 availability, CR0.EM=0, CR0.TS=0 and CR4.OSFXSR=1 are assumptions only;
there is no CPUID or exception-delivery claim.

Encoding, operand order and saturation semantics follow PSUBUSW in Intel SDM
revision 093, Volume 2B: [official instruction reference](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf),
[Intel SDM index](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html).
See the [S2-35.9 plan](../plans/STAGE_2.md#s2-359--local-only-bounded-register-psubusw).

## Local register-only `PADDSB` profile — S2-35.10

The exact profile `s2-long-paddsb-reg-v1` admits only legacy
`66 [one optional REX.R/B] 0F EC /r` with ModR/M.mod=3. ModR/M.reg is the XMM
destination and ModR/M.r/m is the source, extended only through XMM0–XMM15.
Each of 16 byte lanes reinterprets destination and source as signed i8, adds,
clamps to [-128, 127], and encodes the result as its raw u8 byte. This is
signed saturation: for example, `0xff + 0x01` produces `0x00`, while
`0x80 + 0x7f` produces `0xff`; unsigned saturation would differ. Bare `0F EC`
is MMX and rejected. PADDSW, signed subtraction, memory, extra prefixes,
REX.W/X, VEX/EVEX, JIT/native and physical execution are excluded.

The interpreter requires the local x87/SSE state and preserves every field
except the destination XMM register and RIP; RFLAGS and MXCSR remain unchanged.
Execution makes no data-memory callbacks. SSE2 availability, CR0.EM=0,
CR0.TS=0 and CR4.OSFXSR=1 are assumptions only. No CPUID, #UD/#NM/#GP delivery,
or universal MXCSR mask is modeled. Encoding and signed saturation follow
PADDSB in Intel SDM revision 093, Volume 2B: [official Intel SDM PDF](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf),
[Intel SDM index](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html).
S2-34, S2-35 and global `S2-35 → S2-34` remain open. See the
[S2-35.10 plan](../plans/STAGE_2.md#s2-3510--local-only-bounded-register-paddsb).

## Local register-only `PADDSW` profile — S2-35.12

The exact diagnostic profile `s2-long-paddsw-reg-v1` admits only legacy
`66 [one optional REX.R/B] 0F ED /r` with ModR/M.mod=3. The mandatory `66`
selects the XMM/SSE2 encoding from bare MMX `0F ED`; ModR/M.reg is destination
and ModR/M.r/m is source, with REX.R/B extending the indices through XMM15.
Eight independent little-endian signed i16 lanes are summed in i32 and clamped
to [-32768, 32767]. Only destination XMM and RIP change; all other state,
including flags and MXCSR, is preserved and no data-memory callback occurs.
Missing raw x87/SSE state and unsupported/truncated encodings reject atomically.
SSE2 availability and CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1 are assumptions only.
No CPUID, guest exception-delivery, memory, JIT/native or physical-hardware
claim is made. See the [S2-35.12 plan](../plans/STAGE_2.md#s2-3512--local-only-bounded-register-paddsw).


## Local register-only `PSUBSW` profile — S2-35.13

The diagnostic profile `s2-long-psubsw-reg-v1` admits only legacy
`66 [one optional REX.R/B] 0F E9 /r` with ModR/M.mod=3. Mandatory `66` selects
the XMM/SSE2 operation from bare MMX `0F E9`; ModR/M.reg is the destination
and ModR/M.r/m is the source, extended through XMM15 by REX.R/B. Eight
independent little-endian signed i16 lanes compute `DEST - SRC` in widened i32
and clamp to [-32768, 32767]. Only the destination XMM register and RIP change;
all other modeled state, including RFLAGS and MXCSR, is preserved, and no
data-memory callback occurs. Missing raw x87/SSE state and unsupported or
truncated encodings reject before state mutation.

Bare MMX, memory forms, other widths, extra prefixes, REX.W/X, VEX/EVEX, JIT,
native execution and physical acceptance are excluded. SSE2 availability and
CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1 are assumptions. No CPUID or guest exception
delivery claim is made. Encoding and semantics follow `PSUBSW` in Intel SDM
revision 093, Volume 2B: [official Intel SDM PDF](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf),
[Intel SDM index](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html).
See the [S2-35.13 plan](../plans/STAGE_2.md#s2-3513--local-only-bounded-register-psubsw).

## Local register-only `PADDD` profile — S2-35.14

The exact local profile `s2-long-paddd-reg-v1` admits legacy
`66 [one optional REX.R/B] 0F FE /r` only when ModR/M.mod=3. ModR/M.reg is the
XMM destination and ModR/M.r/m is the source. Four little-endian u32 lanes add
independently modulo 2^32; carry remains within a lane. Mandatory 66 selects
XMM/SSE2 rather than bare MMX. The slice excludes PSUBD/PADDQ, saturation,
memory operands, extra prefixes, REX.W/X, VEX/EVEX, native/JIT, and physical
execution. SSE2 availability, CR0.EM=0, CR0.TS=0, and CR4.OSFXSR=1 are assumed;
there is no CPUID or exception-delivery claim. S2-34, S2-35, and global
S2-35 → S2-34 remain open. See the [S2-35.14 plan](../plans/STAGE_2.md#s2-3514--local-only-bounded-register-paddd).

## Local register-only `PSUBD` profile — S2-35.15

The exact local profile `s2-long-psubd-reg-v1` admits legacy
`66 [one optional REX.R/B] 0F FA /r` only when ModR/M.mod=3. ModR/M.reg is
the XMM destination and ModR/M.r/m is the source. Four little-endian u32 lanes
subtract independently modulo 2^32; borrow stays within each lane. Mandatory
66 selects XMM/SSE2 rather than bare MMX. This slice excludes PADDQ/PSUBQ,
other widths, saturation, memory operands, extra prefixes, REX.W/X, VEX/EVEX,
native/JIT, and physical execution. SSE2 availability, CR0.EM=0, CR0.TS=0, and
CR4.OSFXSR=1 are assumptions; there is no CPUID or exception-delivery claim.
S2-34, S2-35 and global S2-35 → S2-34 remain open. See the
[S2-35.15 plan](../plans/STAGE_2.md#s2-3515--local-only-bounded-register-psubd).
## Local register-only `PCMPEQB` profile — S2-35.18

The diagnostic profile `s2-long-pcmpeqb-reg-v1` admits only legacy
`66 [one optional REX.R/B] 0F 74 /r` with ModR/M.mod=3. Mandatory `66`
selects XMM/SSE2 rather than the bare MMX form. ModR/M.reg is the destination
and ModR/M.r/m the source, extended through XMM15 by REX.R/B. Each byte lane
independently becomes `FF` when the destination byte equals the source byte,
and `00` otherwise. Only destination XMM and RIP change; all other modeled
state, including RFLAGS and MXCSR, remains unchanged, and no data-memory
callback occurs.

Bare MMX, PCMPEQW/D/Q, PCMPGT*, and PCMPEQQ (a separate SSE4.1 instruction),
memory operands, additional prefixes, REX.W/X, VEX/EVEX, JIT/native execution
and physical acceptance are excluded. Encoding and semantics follow PCMPEQB in
Intel SDM revision 093, Volume 2B. SSE2
availability and CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1 are assumptions only. No
CPUID or exception-delivery claim is made. See the
[S2-35.18 plan](../plans/STAGE_2.md#s2-3518--local-only-bounded-register-pcmpeqb).


## Local register-only `PCMPGTB` profile — S2-35.21

The exact diagnostic profile `s2-long-pcmpgtb-reg-v1` admits only legacy
`66 [one optional REX.R/B] 0F 64 /r` with ModR/M.mod=3. ModR/M.reg is the
XMM destination and ModR/M.r/m the source, extended to XMM0–15. Each byte lane
independently writes `FF` iff the destination interpreted as signed i8 is
strictly greater than the source interpreted as signed i8; otherwise it writes
`00`. Only destination XMM and RIP change; flags, MXCSR and other modeled state
are preserved, and the operation invokes no guest data-memory callback.

Coverage compares all 65,536 raw byte pairs against an independent signed-byte
mapping, includes asymmetric and strict/equal cases and signed boundaries,
mixed lanes, the full 16×16 XMM/REX matrix, atomic stop/budget/truncation/missing
state behavior, exact-profile negatives and a literal XMM8/XMM9 fixture. Bare
MMX, PCMPGTW/D/Q, other comparisons, memory, extra prefixes, REX.W/X,
VEX/EVEX, feature gates, exception delivery, JIT/native execution and physical
acceptance are excluded. PCMPGTQ requires SSE4.2 and is outside this SSE2 slice.
SSE2 plus CR0.EM=0, CR0.TS=0 and CR4.OSFXSR=1 are assumptions only; no CPUID
or exception-delivery claim is made. Encoding and semantics follow PCMPGTB in
Intel SDM revision 093, Volume 2B. S2-34, S2-35 and global S2-35 → S2-34 remain
open; Mac mini/SSH acceptance is deferred. See the
[S2-35.21 plan](../plans/STAGE_2.md#s2-3521--local-only-bounded-register-pcmpgtb).

## Local register-only PUNPCKHDQ slice — S2-35.29

The profile `s2-long-punpckhdq-reg-v1` admits only legacy
`66 [one optional REX.R/B] 0F 6A /r` with ModR/M.mod=3. ModR/M.reg selects the
destination XMM register and r/m selects the source. The destination becomes
the four little-endian u32 elements `[old_DEST[2], SRC[2], old_DEST[3],
SRC[3]]`; its low two input dwords and the source low two dwords are ignored.
All destination bits are overwritten. The profile assumes SSE2 and
CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1; it does not implement feature/control gates.
MMX, memory, other prefixes, REX.W/X, VEX/EVEX, JIT/native execution, guest
exception delivery and physical acceptance are excluded. Parent S2-34, S2-35
and the global S2-35 → S2-34 dependency remain open.


## Local register-only PUNPCKHQDQ slice — S2-35.31

The profile `s2-long-punpckhqdq-reg-v1` admits legacy
`66 [one optional REX.R/B] 0F 6D /r` with ModR/M.mod=3. ModR/M.reg selects the
destination XMM register and r/m selects the source. The destination becomes
the two little-endian u64 qwords `[old_DEST[1], SRC[1]]`; low qwords are ignored.
PUNPCKHQDQ has no MMX form, so bare `0F 6D /r` is rejected. The profile assumes
SSE2 and CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1; feature/control gates, memory, MMX,
other prefixes, REX.W/X, VEX/EVEX, JIT/native execution, guest exception
delivery and physical acceptance are excluded. S2-34, S2-35, and global
S2-35 → S2-34 remain open. See the
[S2-35.31 plan](../plans/STAGE_2.md#s2-3531--local-only-bounded-register-punpckhqdq).

## Local register-only `PAND` profile — S2-35.49

The exact schema-v2 fixture profile `s2-long-pand-reg-v1` admits only legacy
`66 [one optional REX.R/B] 0F DB /r` with ModR/M.mod=3. Mandatory `66`
selects the XMM/SSE2 form; bare `0F DB /r` is MMX and rejected. ModR/M.reg is
the destination and ModR/M.r/m the source, independently extended to XMM0–15
by REX.R and REX.B. Execution snapshots the source and computes the raw 128-bit
AND into the destination. Aliasing is supported. No memory callback occurs;
RFLAGS, MXCSR, x87, GPRs, and non-destination XMM registers are preserved.

Only the exact bounded byte form is admitted. REX.W/X, any other or duplicate
prefix, memory operands, VEX and EVEX are excluded. SSE2 availability,
CR0.EM=0, CR0.TS=0 and CR4.OSFXSR=1 are diagnostic preconditions; this profile
does not model those gates or `#UD/#NM` delivery. JIT/native admission and
physical execution remain unsupported. See the
[S2-35.49 plan](../plans/STAGE_2.md#s2-3549--local-only-bounded-register-pand).

## Local register-only `PANDN` profile — S2-35.50

The exact schema-v2 fixture profile `s2-long-pandn-reg-v1` admits legacy
`66 [one optional REX.R/B] 0F DF /r` with ModR/M.mod=3. Mandatory `66` selects
the XMM/SSE2 form; bare `0F DF /r` is MMX and rejected. ModR/M.reg selects the
XMM destination and ModR/M.r/m the source, independently extended through
XMM15 by REX.R and REX.B. Execution snapshots both old operands and computes
each destination byte as `!old_DEST & SRC`; aliasing follows that same rule.
No memory callback occurs. Flags, MXCSR, x87, GPRs, and non-destination XMM
registers remain unchanged.

Only the exact bounded register form is admitted. Memory, REX.W/X, other or
duplicate prefixes, VEX/EVEX, and sibling opcodes including PAND and POR are
excluded. SSE2 availability, CR0.EM=0, CR0.TS=0, and CR4.OSFXSR=1 are diagnostic
preconditions; this profile does not model those gates or `#UD/#NM` delivery.
JIT/native admission and physical execution remain unsupported. See the
[S2-35.50 plan](../plans/STAGE_2.md#s2-3550--local-only-bounded-register-pandn).

## Local register-only `POR` profile — S2-35.51

The exact schema-v2 fixture profile `s2-long-por-reg-v1` admits long-mode
`66 [one optional REX.R/B] 0F EB /r` with ModR/M.mod=3. Mandatory `66`
selects the XMM/SSE2 operation; bare `0F EB /r` is MMX and rejected.
ModR/M.reg is the destination and ModR/M.r/m the source; REX.R and REX.B
independently select XMM0–XMM15. Execution snapshots both operands and stores
`old_DEST | old_SRC` across the full 128-bit XMM register. Aliasing is
supported. No memory callback occurs; RFLAGS, MXCSR, x87, GPRs, and every
non-destination XMM register are preserved.

Only this exact register form is admitted. Memory, REX.W/X, other or duplicate
prefixes, VEX/EVEX, sibling opcodes and other forms are excluded. SSE2
availability, CR0.EM=0, CR0.TS=0, and CR4.OSFXSR=1 are diagnostic preconditions;
this profile does not model those gates or `#UD/#NM` delivery. JIT/native
admission and physical execution remain unsupported. See the
[S2-35.51 plan](../plans/STAGE_2.md#s2-3551--local-only-bounded-register-por).


## Local register-only MOVAPS profile — S2-34.11

The exact diagnostic profile `s2-long-movaps-reg-v1` admits long-mode `[one optional REX.R/B] 0F 28 /r` only when ModR/M.mod=3. ModR/M.reg is the XMM destination and r/m is the source; REX.R and REX.B independently select XMM8–XMM15. The interpreter copies all 128 raw source bits, including arbitrary floating-point bit patterns, and changes no flags, MXCSR, x87, GPRs, or other XMM registers. Alias/self-copy is supported; execution performs no data-memory access.

Memory, reverse-direction `0F 29`, MOVUPS `0F 10`, mandatory-66 MOVAPD, F2/F3, segment/address/LOCK prefixes, duplicate REX, REX.W/X, VEX/EVEX, and incomplete instruction fetches are outside the profile. SSE preconditions (CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1) are assumptions; missing local SSE state refuses without commit. Alignment and memory fault semantics, real #UD/#NM gates, JIT/native execution, and physical execution are not modeled. This does not imply acceptance of parent S2-34 or coverage of MOVUPS/MOVAPD. See the [S2-34.11 plan](../plans/STAGE_2.md#s2-3411--local-only-register-movaps).

## Local register-only reverse-direction MOVAPS profile — S2-34.12

The distinct exact diagnostic profile `s2-long-movaps-reg-reverse-v1` admits
long-mode `[one optional REX.R/B] 0F 29 /r` only when ModR/M.mod=3. ModR/M.reg
plus REX.R selects the XMM source; ModR/M.r/m plus REX.B selects the
destination. Therefore `0F 29 C1` copies XMM0 to XMM1, and `45 0F 29 C1`
copies XMM8 to XMM9. Execution copies all 128 raw source bits, supports alias
self-copy, preserves the source and all unrelated XMM, MXCSR, x87, GPR and
flags state, and performs no data-memory access. Missing local SSE state refuses
without commit. CR0.EM=0, CR0.TS=0 and CR4.OSFXSR=1 are assumed preconditions.

Memory, forward-direction `0F 28`, MOVUPS `0F 11`, mandatory-66 MOVAPD, F2/F3,
segment/address/LOCK prefixes, duplicate REX, REX.W/X, VEX/EVEX and incomplete
instruction fetches are outside this profile. Alignment and memory fault
semantics, real #UD/#NM gates, JIT/native execution and physical execution are
not modeled; the register-only profile makes no aligned-memory claim. This
slice does not imply acceptance of parent S2-34/S2-35. See the
[S2-34.12 plan](../plans/STAGE_2.md#s2-3412--local-only-reverse-direction-register-movaps)
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-3412-local-reverse-direction-register-movaps-diagnostic-profile).

## Local register-only MOVAPD profile — S2-34.13

The exact diagnostic profile `s2-long-movapd-reg-v1` admits long-mode
`66 [one optional REX.R/B] 0F 28 /r` only when ModR/M.mod=3. Mandatory `66`
distinguishes this SSE2 MOVAPD form from MOVAPS. ModR/M.reg plus REX.R selects
the XMM destination and ModR/M.r/m plus REX.B selects the source. The
interpreter copies all 128 raw bits, supports aliases and arbitrary floating
point bit patterns, and changes no flags, MXCSR, x87, GPRs or other XMM
registers. RIP advances on success. No data-memory callbacks occur. Missing
local SSE state refuses without commit.

No other legacy prefixes, duplicate or out-of-order prefixes, REX.W/X,
ModR/M memory operands, VEX/EVEX, reverse-direction `66 0F 29 /r`, or MOVUPD
`66 0F 10 /r` are admitted. SSE2 availability, CR0.EM=0, CR0.TS=0 and
CR4.OSFXSR=1 are assumed; control-register and real `#UD/#NM` behavior are not
modeled. Alignment/fault behavior, JIT/native execution and physical execution
remain outside this register-only profile, and parent S2-34 acceptance is not
implied. See the [S2-34.13 plan](../plans/STAGE_2.md#s2-3413--local-only-register-movapd)
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-3413-local-register-movapd-diagnostic-profile).

## Local register-only reverse-direction MOVAPD profile — S2-34.14

The exact diagnostic profile `s2-long-movapd-reg-reverse-v1` admits long-mode
`66 [one optional REX.R/B] 0F 29 /r` only when ModR/M.mod=3. Mandatory `66`
selects SSE2 MOVAPD. ModR/M.reg plus REX.R selects the XMM source; ModR/M.r/m
plus REX.B selects the destination. The interpreter copies all 128 raw bits,
supports aliasing, and changes no flags, MXCSR, x87, GPRs, or other XMM
registers. It performs no data-memory access. SSE2 and CR0.EM=0, CR0.TS=0,
CR4.OSFXSR=1 are assumed; missing local SSE state refuses without commit.

Missing/duplicate/misplaced `66`, forward `66 0F 28`, MOVUPS/MOVUPD, memory,
F2/F3, segment/address/LOCK prefixes, duplicate REX, REX.W/X, VEX/EVEX, and
incomplete instruction fetches are outside the profile. Reverse-direction
execution remains local interpreter-only; JIT/native and physical execution
are not implemented. No aligned-memory behavior or parent S2-34 acceptance is
implied. See the [S2-34.14 plan](../plans/STAGE_2.md#s2-3414--local-only-reverse-direction-register-movapd)
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-3414-local-reverse-direction-register-movapd-diagnostic-profile).
