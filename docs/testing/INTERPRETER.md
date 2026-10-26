# Проверка эталонного интерпретатора — этап 2

Нормативные формы, источники и фактическое покрытие перечисляются в
[CPU_CONTRACT](../specs/CPU_CONTRACT.md). Порядок реализации —
[S2-00…S2-44](../plans/STAGE_2.md). Этот документ не подтверждает завершение этапа.

## Профили и порядок

Первый профиль `s2-long-integer-v1` — диагностический long mode с одним CPU,
16 GPR, RIP u64, operand widths 8/16/32/64 и address widths 32/64.
Память — ограниченная fixture RAM; paging и MMIO не подразумеваются.
Отдельные local-only профили `s2-long-fs-base-v1` и
`s2-long-gs-base-v1` добавляют ровно по одной qword load форме с базой
соответствующего сегмента; они не расширяют integer profile и не заявляют
selector/descriptor, privilege или paging semantics.
Профили `s2-long-fs-selector-v1` и `s2-long-gs-selector-v1` локально
добавляют только `48 8C E0` (`MOV RAX, FS`) и `48 8C E8` (`MOV RAX, GS`):
каждый наблюдает свой 16-bit selector независимо от соответствующей base и
не загружает или проверяет descriptor.
Профиль `s2-long-gs-load-v1` admits the existing exact `8E E8` descriptor
load and one local-only composition program `8E E8 65 48 8B 00`; its bounded
fixture validator rejects other byte sequences and budgets. The composition
proves that the loaded GS.base feeds the existing GS qword load form. It does
not expand native admission or claim Linux readiness; S2-22 remains open.
Сначала A (integer/memory/stack), затем B (strings/REP/exits), C (system/modes),
D (x87/MMX/SSE/SSE2). CPUID не рекламирует незавершённые группы.

Для следующего Linux пути выбран прямой 64-bit entry по Linux boot protocol,
§1.14. Для Ubuntu image `7.0.0-38-generic` из package `7.0.0-38.38` статически
проверены exact image/config hashes, header 2.15 и `xloadflags=0x007f`;
[полный отчёт](../reports/STAGE_2_LINUX_PROFILE.md) отделяет измеренные bytes
от выводов по upstream v7.0. Image не staged/booted, APT/PE signatures не
проверены. CPU входит
в long mode с paging, GDT, сегментами и выключенными interrupts; RSI указывает
на boot_params. Identity mappings создаёт machine/loader этапа 4. Real-mode
startup не требуется этой точкой входа; это не исключает иных режимов после
входа. В upstream v7.0 `configure_5level_paging` использует 32-bit trampoline
при mismatch текущего и требуемого LA57; direct64 сам по себе не исключает
этот путь. Требуется сопоставление с Ubuntu source/build и реальным boot.
Config уточнил required capabilities, но полный reachable ISA/CPUID/MSR/mode
inventory ещё не принят. Сейчас принят диагностический профиль A; обязательность
дополнительных C/D форм должна быть подтверждена до S2-44. Windows profile
остаётся открытым и не выводится из наличия установочного ISO.

## Наблюдение и выходы

Новый core получает fetch/read/write через CPU memory interface.
Fetch запрашивает необходимые байты, decode выполняется с текущего RIP;
недостижимый хвост не проверяется. Budget/stop, unsupported implementation,
architectural exception и внутренняя host error — разные результаты.
Commit определяется каждой формой; для REP проверяется частичный прогресс.
Flat RAM mapping не порождает выдуманные x86 page-fault codes.

Существующий S0 fragment entry остаётся отдельным compatibility API:
четыре GPR, RIP как индекс code buffer, предварительный decode, проверенные
branch boundaries и остановка на buffer end. Это не модель нового core.
Fixture/result v1, C ABI v0 и native trampoline сохраняют старый профиль.
Переход к новому состоянию требует явного проверяемого adapter, а не усечения.

## Реализованный step/run — S2-05

`od_isa::execute::step` декодирует одну инструкцию по текущему guest RIP,
побайтно запрашивая только нужные bytes. Возвращаются `Advanced`, `Branched`,
`Fault` или `Unsupported`; mapping/backend errors возвращаются отдельно как
`MemoryError`. В checkpoint S2-05 исполнялись только точные S0 формы: MOV imm64 в RAX…RBX,
ADD/SUB/CMP register-register r64 и JZ/JNZ rel8. Полное состояние с 16 GPR
не означает, что появились новые instruction forms. Trap/wait/event delivery
не реализованы и успешными выходами не подменяются.

`execute::run` на каждой instruction boundary проверяет в таком порядке:
явный `stop_rip`, cooperative STOP callback, исчерпание budget, затем fetch.
Поэтому `stop_rip` побеждает STOP/budget; STOP побеждает нулевой budget.
Budget считает только завершённые инструкции, включая взятый и невзятый Jcc.
Fault/unsupported/backend error не увеличивают счётчик и сохраняют faulting RIP,
GPR и flags; завершённые перед ними инструкции остаются наблюдаемыми.
Для register-only форм S2-05 commit происходит после успешного decode и
проверки взятой цели перехода. Проверяется 48-bit canonical address, LA57 нет.

У нового core нет неявного конца code buffer: `stop_rip: None` продолжает fetch
по следующему RIP, и недоступный адрес даёт memory error. Ветвление не требует
статической карты границ: цель декодируется при достижении; недостижимый
неподдерживаемый хвост не влияет на исполнение. Остановка на заданном адресе —
правило harness, а не CPU instruction или guest fault.

Старый `interpret` теперь выполняет прежний статический S0 decode и проверки
branch/entry boundaries, затем вызывает этот core через compatibility adapter.
Конец buffer задаётся явным `stop_rip`. Сохраняется прежний порядок: полный
decode до исполнения, buffer end до budget, нулевой budget до проверки entry.
Несогласованность предварительно проверенного S0 и core возвращает
`UnexpectedCoreExit`, а не выдуманное guest exception. JIT и v1 oracle остаются
в старом профиле; adapter не расширяет их ISA.

Проверки `od-testkit` покрывают разные entry RIP, прямой/обратный Jcc,
недостижимый невалидный хвост, точный budget и приоритет STOP, отсутствие
неявной остановки в конце mapping, faulting fetch и неизменность состояния.
Фактические локальные и физические результаты — в
[отчёте этапа](../reports/STAGE_2_REPORT.md#s2-05--пошаговое-исполнение).

S2-06 принят в decode-only границах: нормализованный decoder распознаёт дополнительные
widths, register MOV и near JZ/JNZ, но execution allowlist сохраняет только
перечисленные выше формы S2-05. Decoder tests проверяют распознавание bytes,
не исполнение этих форм. Данные физической регрессии S2-05 получены до
decoder integration и v2 review fixes.

S2-07 добавил decode-only ModR/M/SIB, displacement, RIP-relative и 67
address-size forms; 6228 raw-byte combinations проверены локально. FS/GS memory
forms остаются Unsupported, execution по-прежнему S0 до S2-08. Effective-address
helper проверяет только стартовый canonical адрес: full-width range должен
проверяться при memory execution S2-08. Для noncanonical start #SS относится
к actual base RSP/RBP, #GP — к остальным, включая R12/R13; одних младших
encoding bits недостаточно для выбора #SS.

## MOV и v2 execution — S2-08

Реализованы MOV B0–BF immediate register и 88–8B register/register/load/store
с widths 8/16/32/64, всеми 16 GPR и high-byte/REX правилами. MOV сохраняет
flags. Memory forms используют ModR/M/SIB, RIP-relative и 67; 67 оборачивает
только effective start, поэтому operand может пересекать 4 GiB.
Перед backend проверяется canonical диапазон всей ширины: #SS для actual
RSP/RBP base, #GP для остальных bases, включая R12/R13. Неуспех сохраняет
RIP/GPR/flags и не даёт частичной записи. Endpoint overflow u64 отдельно
возвращает AddressOverflow без backend/commit; его архитектурная трактовка
остаётся [ISSUE-012](../KNOWN_ISSUES.md#issue-012--не-подтверждена-семантика-operand-range-через-верх-u64).

Arithmetic остаётся S0 ADD/SUB/CMP r64 первых четырёх регистров, branches —
short JZ/JNZ. Другие decoded forms и FS/GS memory не исполняются. Старый
fragment API, JIT и native v1 сохраняют свой профиль.

CLI выполняет schema-v2 `--fixture` с engine interpreter, снимает полное state,
memory и fault snapshot. V2 JIT даёт явный Unsupported с exit 4; mapping/backend
errors дают Harness с exit 5, architectural fault сохраняется отдельным outcome.
Deadline проверяется cooperative на instruction boundary отдельным признаком:
он даёт TimedOut/exit 5, а не StopRequested или guest exception.
Проверяемые программы находятся в [stage2 fixtures](../../tests/fixtures/x86/stage2/).
Пример запуска:

```sh
cargo run --offline -p od-runner -- --fixture tests/fixtures/x86/stage2/mov_load_add_store.case.json --engine interpreter --json
```

S2-08 принят как локальный working slice. Локальные результаты не являются физической
проверкой новых memory форм; такой oracle относится к S2-09.

S2-09 затем подтвердил три положительные hardware-layout MOV/memory программы
на Mac mini: два повтора interpreter/native, full16/RSP/RIP, flags `0xcd7`,
вся fixture memory и exact counts, ноль расхождений и стабильные повторы.
На том же fingerprint прошла legacy регрессия 1010 cases ×2. Linux сборка
и native v2 tests также прошли. Границы, raw run IDs и reproduction — в
[CPU_ORACLE](CPU_ORACLE.md#native-v2-s2-09-границы-физического-подтверждения),
малое [evidence](../reports/data/stage2/native-2026-10-22.json) сохраняет hashes.
Подтверждены эти положительные программы на одном Intel CPU, не полный этап,
privileged/faulting forms, AMD или v2 JIT. ISSUE-011/012 остаются открытыми.

## S2-10 — дополнительные transfer forms

Точные bytes, widths и исключения перечислены в
[CPU contract](../specs/CPU_CONTRACT.md#передача-данных-и-lea--s2-10).
Реализуются C6/C7 immediate r/m (imm32 sign-extension для qword), A0–A3 moffs,
MOVSX/MOVSXD/MOVZX, LEA и register-only XCHG/NOP90. Flags сохраняются.
Arithmetic/short branch execution сохраняет прежние границы; memory XCHG
остаётся Unsupported без фиктивной atomicity.

Review проверяет commit после успешного memory access, отсутствие backend
вызовов/canonical check у LEA, next RIP включая C7 immediate, различие NOP90
и XCHG EAX,EAX, а также AL/AH exchange с чтением обеих aliases до writes.
Для новых групп подготовлены пять дополнительных fixtures с полным сохранением
промежуточных результатов и canaries. Linux workspace tests прошли; физический
run `stage2-20261022-214049-874a0bf0` подтвердил восемь v2 cases ×2, включая
пять новых, full16/RSP/RIP, flags `0xcd7`, всю memory и exact counts без
расхождений, стабильные повторы. На том же fingerprint прошли legacy 1010 cases ×2.
S2-10 принят в этих положительных границах; полный этап 2 не принят.
Долговечное [evidence](../reports/data/stage2/transfers-2026-10-22.json)
фиксирует manifests, exact inputs, окружение и reproduction argv.

## S2-11 — arithmetic execution

Поддержаны ADD/ADC/SUB/SBB/CMP, INC/DEC/NEG widths8/16/32/64 с точными
reg/memory/immediate forms, sign-extension и eager CF/PF/AF/ZF/SF/OF.
INC/DEC сохраняют CF, CMP не пишет destination, DF сохраняется. Opcode
границы и normative sources — в
[контракте](../specs/CPU_CONTRACT.md#арифметика-и-eager-flags--s2-11).
Read/write fault не меняет RIP/GPR/flags/RAM и не retire-ит инструкцию;
локальные injected #PF/backend tests проверяют второй logical region.
Native positive profile проверяет ranges до step и не доказывает гостевой #PF.

313 новых fixtures имеют budget1, literal full-state/flags/memory answers,
canaries и masks `0xcd7`. Материализатор не исполняет x86 semantics.
Physical run `stage2-20261022-215943-adeb1874` прошёл 321 cases ×2 (включая
восемь прежних), full16/RIP/flags/memory/count совпали, repeat2 стабильны.
Legacy 1010 cases ×2 прошёл на том же fingerprint; local и actual Linux
workspace tests прошли. [Evidence](../reports/data/stage2/arithmetic-2026-10-22.json)
фиксирует hashes/times/inputs/machines/reproduction. S2-11 принят для этого
положительного профиля на одном Intel, без AMD/privileged/v2-JIT/full-stage claim.
LOCK/REP и FS/GS data arithmetic остаются Unsupported.

## S2-12 — logic и semantic flags masks

AND/OR/XOR и TEST определяют SF/ZF/PF, сбрасывают CF/OF и оставляют AF
undefined; NOT сохраняет flags и mask. Opcode/width/immediate boundaries —
в [CPU contract](../specs/CPU_CONTRACT.md#логика-test-и-definedness--s2-12).
TEST сохраняет полный destination и не вызывает memory write. Undefined
AF value не участвует в comparison: `rflags_defined` становится `0xcc7`.

Mask обновляет shared decoded `FlagEffects` только после успешного retirement,
native — после TRAP_TRACE; это semantic metadata, а не expected evaluator.
Fault/unsupported mask не меняет. `XOR;MOV;NOT` сохраняет undefined AF;
`XOR;ADD` возвращает mask `0xcd7`. Local injected faults проверяют точный
no-commit/no-retirement/no-mask-update; native positive suite не доказывает #PF.

197 новых logic cases вместе с 321 прежним прошли physical 518×2:
1036 snapshots, ноль различий, 518 стабильных repeat2. В каждом backend
684 snapshots mask `0xcd7`, 352 — `0xcc7`, full16/RIP/RFLAGS/memory/exception/count
наблюдаются. На том же fingerprint legacy 1010×2 прошёл без interpreter/native
и JIT/native различий. Local146/Linux167 workspace tests и оба generator
`--check` прошли. [Evidence](../reports/data/stage2/logic-2026-10-22.json)
не заменяет проверку AMD/privileged/v2 JIT или полного этапа. LOCK/REP и
FS/GS data logic остаются Unsupported.

## S2-13 — shifts и rotates

Поддержанные формы, immediate/CL counts, widths и SDM093 entries перечислены
в [CPU contract](../specs/CPU_CONTRACT.md#сдвиги-и-вращения--s2-13). Count
masking/modulo проверяется до host shift; definedness CF/OF зависит от
effective count и формы. Zero effective count сохраняет destination, RFLAGS
и `rflags_defined`. Для SHLD/SHRD с count выше ширины destination undefined;
эти значения не являются exact oracle expectations. Dword write при нулевом
count сохраняет обычное long-mode zero-extension поведение.

Fault injection на read и write для memory RMW, при zero/nonzero effective
count, проверяет no-retirement и отсутствие CPU/RIP/definedness/RAM commit.
Physical run охватывал 142 register и 2 успешных memory RMW cases; запреты
range/permissions отсекаются до native исполнения, поэтому native host-signal
fault snapshot в этой методике отсутствует. Две 16-bit SHLD/SHRD raw17 формы
проверяются локально на отсутствие panic и undefined arithmetic flags, но
исключены из physical exact comparison из-за architecturally undefined
destination. Полные проверки и hashes — в [S2-13 report](../reports/STAGE_2_REPORT.md#s2-13--сдвиги-и-вращения)
и [evidence](../reports/data/stage2/shifts-2026-10-22.json). LOCK/REP,
privileged state, AMD semantics, v2 JIT и полный этап остаются вне этих claims.

## S2-14 — multiply/divide positive physical slice

Формы и точные Intel SDM093 citations находятся в
[CPU contract](../specs/CPU_CONTRACT.md#умножение-деление-и-de--s2-14-успешные-формы-физически-проверены).
Interpreter поддерживает one-operand MUL/IMUL, two/three-operand IMUL и
one-operand DIV/IDIV с implicit accumulator/high-half state. На успешном
multiply CF/OF определены по overflow, остальные arithmetic flags undefined;
на успешном divide все arithmetic flags undefined. Делитель-ноль и quotient
overflow дают локальный `#DE` до destination write/retirement, состояние до
инструкции сохраняется. Signed quotient округляется к нулю.

Генератор сообщает 38 актуальных positive-operation case/expected pairs; все
они завершаются `StopAddress` после одной инструкции без exception. CLI integration test
`multiply_divide_cli_fixtures_match_literal_full_state_and_flag_definedness`
сверяет локальный результат со всеми literal answers. Divide-fault cases,
включая divisor zero и signed minimum / −1, находятся только в локальном
`od-isa` unit test `divide_errors_preserve_exact_state_and_report_de_before_retirement`;
они не входят в эти fixtures. Физический прогон включил все 38 успешных
fixtures ×2. В полном корпусе 700 inputs было 1400 comparisons, ноль
interpreter/native differences и 700/700 стабильных cases на repeat 2; оба
backend сняли все 16 GPR, RIP и RFLAGS. Target: Intel Core i5-3210M, Ubuntu
x86_64, Linux `6.17.0-41-generic`, Rust/Cargo 1.99.1. Run manifest и хеши
приведены в [отчёте](../reports/STAGE_2_REPORT.md#s2-14--умножение-и-деление),
компактные данные — в [evidence JSON](../reports/data/stage2/multiply-divide-2026-10-22.json).
Divide-fault inputs остаются только в local `od-isa` unit test; физические
fixtures выполняют успешные операции и не подтверждают physical `#DE` delivery
или Linux host-signal/context mapping. v2 JIT не участвовал.

## Проверки

До изменения CPU выполняются существующие baseline-команды:

```sh
python3 tests/cpu/stage1_harness.py --suite local
python3 tests/cpu/stage1_harness.py --suite oracle --generated-count 1000 --seed 0x5eedcafe --repeats 2
```

Они проверяют S0-09, не будущий integer/memory профиль. Каждая новая группа
проходит [checklist ISA](ADDING_ISA_GROUP.md): точные bytes и widths, defined
flags, fault ordering, ручные и generated fixtures, физический oracle с двумя
повторами. Новые команды документируются только после реализации.
Hardware unavailable/skipped не считается успехом. Привилегированные CPU
проверки через callbacks не заменяют machine integration и независимый oracle.

## Карта совместимости S2-01

| Потребитель | Сохраняемая граница |
|---|---|
| od-isa `interpret`/`decode` | S0 buffer contract и прежние flags |
| od-jit emitter, flags helper, native entry | Только S0 формы; layout fragment state проверяется отдельно |
| od-host CompiledFragment и OdCpuStateV0 | Четыре GPR; неизменные C layout и result codes |
| include/open_desktop и Swift VMHost bridge | Публичный C header v0, control protocol v0 |
| od-testkit case/result/compare/generate/minimize | Strict v1, прежние digests и S0 branch boundaries |
| od-runner main/test_cli | Старые CLI/JSON значения и входное состояние S0 |
| od-x86-runner native trampoline/supervisor | Собственный четырёхрегистровый trampoline layout, signal observations |

Изменения core не расширяют поддерживаемый JIT ISA. Совместимость подтверждается
исполнением старых fixtures, а не одним совпадением размеров структур.

## Новые fixture API и память

Формат v2, явное преобразование v1 и versioned comparison описаны в
[TEST_CASE_FORMAT](TEST_CASE_FORMAT.md#version-2-long-mode-integer-state).
В S2-08 подключено interpreter execution v2 в описанном выше MOV срезе;
локальные aggregate checks прошли. Conversion/comparator сами по себе не
подтверждают более широкий CPU profile.

`od_isa::memory::GuestMemory` разделяет fetch/read/write, guest linear address,
mode и CPL; host pointers через этот интерфейс не передаются. Slice length
задаёт ширину; integer helpers выполняют little-endian чтение/запись.
`MemoryError::Architectural` содержит CPU fault, остальные варианты описывают
fixture mapping/permissions, переполнение адреса или внутренний backend error.
Неотображённый flat range не считается доказанным x86 #PF.

`od_testkit::memory::FlatMemory` владеет только ограниченной fixture RAM.
Обращение может пересекать смежные regions; все адреса и права проверяются до
изменения RAM или read destination. Ошибка оставляет их неизменными.
`inject_once(address, kind, error)` возвращает заданную ошибку на первом
совпавшем обращении после проверки переполнения и до mapping/side effects.
Это тестовый механизм, не универсальные memory transactions и не MMIO backend.
Проверены независимые права fetch/read/write, little-endian widths, смежные
regions, границы 4096/16384, overflow и отсутствие частичной записи при ошибке.
Отдельные regressions проверяют отказ permission во втором смежном region
без изменения RAM/read destination и срабатывание injected error ровно один раз.

## Local S2-31j x87 conversion slice

The exact bare `D9 18` (`FSTP m32fp [RAX]`) profile uses integer-only
binary80-to-binary32 rounding, a canonical48 Dword preflight, and one atomic
write before popping the old physical TOP. Local vectors and the exact
`D9 18 DF E0` fixture cover RC, ties, exponent boundaries, status, canaries,
and failure atomicity. The profile rejects out-of-domain values before data
access; it does not claim x87 exception delivery, hardware exception priority,
native equivalence, or physical x86 confirmation. See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-31j-bounded-fstp-m32fp-rax-d9-18) and [fixtures](../../tests/fixtures/x86/stage2/README.md).

## Local S2-31k x87 conversion slice

The exact bare `D9 10` (`FST m32fp [RAX]`) profile reuses S2-31j's integer-only
binary80-to-binary32 conversion, canonical48 Dword preflight, and one atomic
write. Success preserves TOP, all tags and all 80-bit payloads while applying
the local PE/C1 and FIP/FDP policy. Literal fixtures and focused interpreter
checks cover RC, ties, signs, exponent boundaries, no-pop preservation and
failure atomicity. The profile does not claim x87 exception delivery, hardware
exception priority, native equivalence or physical x86 confirmation. See the
[CPU contract](../specs/CPU_CONTRACT.md#local-s2-31k-bounded-fst-m32fp-rax-d9-10)
and [fixtures](../../tests/fixtures/x86/stage2/README.md).

## Local S2-31l x87 conversion slice

The exact bare `DD 18` (`FSTP m64fp [RAX]`) profile uses integer-only
binary80-to-binary64 rounding, a canonical48 Qword preflight, and one atomic
write before popping the old physical TOP. It intentionally accepts source
exponents only from -1022 through 1022, despite finite binary64 values at
exponent 1023. Literal fixtures cover `DD 18` and `DD 18 DF E0`; focused
interpreter tests cover rounding, stack state, memory atomicity and faults.
Native x87 admission remains closed, and no physical x86 acceptance is claimed.
See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-31l-bounded-fstp-m64fp-rax-dd-18)
and [fixtures](../../tests/fixtures/x86/stage2/README.md).

## Local S2-31m x87 conversion slice

Exact bare `DD 10` (`FST m64fp [RAX]`) reuses the S2-31l integer-only
binary80-to-binary64 conversion, environment gates, and -1022..1022 local
exponent interval. It validates the canonical48 Qword range before one
atomic write with no read, then updates PE/C1 and FIP/FDP/RIP while preserving
TOP, all tags and all raw payloads, including unrounded ST(0). Literal fixtures
cover `DD 10` and `DD 10 DF E0`; interpreter checks cover independent RC/tie
values, all TOP positions, canaries, address and store faults, invalid
preflight state, and execution boundaries. The C0/C2/C3 retention and exact
C1 behavior are local deterministic policies. Native x87 admission stays
closed; these checks do not establish physical x86 behavior. See the
[CPU contract](../specs/CPU_CONTRACT.md#local-s2-31m-bounded-fst-m64fp-rax-dd-10)
and [fixtures](../../tests/fixtures/x86/stage2/README.md).

## Local S2-31n x87 integer-load slice

Exact bare `DB 00` (`FILD m32int [RAX]`) requires the bounded FCW/FSW gates
and an Empty physical `(TOP+7)&7` target before memory access. One
canonical48-checked Dword read feeds an integer-only exact binary80 packer;
success pushes the payload/tag, clears C1, records FIP/FDP and retires once.
The old ST(0) tag and all unrelated tags/payloads remain uninspected and
unchanged. The `DB 00 DF E0` fixture observes the new status through FNSTSW AX.
Native x87 admission stays closed; general exception delivery, hardware fault
priority and physical x86 behavior remain open. See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-31n-bounded-fild-m32int-rax-db-00), [plan](../plans/STAGE_2.md#s2-31n-bare-bounded-fild-m32int-rax-db-00-partial-slice), and [fixtures](../../tests/fixtures/x86/stage2/README.md).

## Local S2-31o x87 Word integer-load slice

Exact bare `DF 00` (`FILD m16int [RAX]`) reuses the same FCW/FSW and Empty
physical push-destination gates as S2-31n. It validates `[RAX,RAX+1]` as one
canonical48 Word range, makes exactly one typed Word read, sign-extends i16 to
i64, and uses the shared integer-only i64 binary80 packer. A successful read
pushes to `(TOP+7)&7`, clears C1, preserves PE and unrelated x87/CPU state, and
updates FIP/FDP/RIP. Zero has an all-zero payload and Zero tag. Literal
`DF 00` and `DF 00 DF E0` fixtures distinguish a Word read from a Dword read
using a nonzero adjacent byte; interpreter vectors also cover all TOP×RC,
signed-word extrema, sign extension, stale Empty payloads, canaries, canonical
range/read failures, and execution boundaries. Native admission stays closed;
general x87 exceptions and physical x86 behavior remain open. See the [CPU
contract](../specs/CPU_CONTRACT.md#local-s2-31o-bounded-fild-m16int-rax-df-00),
[plan](../plans/STAGE_2.md#s2-31o-bare-bounded-fild-m16int-rax-df-00-partial-slice),
and [fixtures](../../tests/fixtures/x86/stage2/README.md).

## Local S2-31p x87 Qword integer-load slice

Exact bare `DF 28` (`FILD m64int [RAX]`) reuses the shared FCW/FSW and Empty
physical `(TOP+7)&7` destination gates. It validates the complete canonical48
Qword range, performs exactly one typed eight-byte read, then sign-interprets
the value as i64 and packs it exactly into binary80 with integer arithmetic.
The shared packer uses `unsigned_abs`, handles `i64::MIN`, emits ten zero bytes
for zero, and does not depend on RC. Success updates only the destination
payload/tag, TOP, C1, FIP/FDP/RIP and retirement; PE, FCW/FOP, other status,
unrelated tags/payloads, GPRs and RFLAGS retain the local policy. Literal
vectors cover signed extrema, values near `2^53` and `2^62`, and small signed
values over every TOP×RC pair. Atomicity checks cover unaligned canaries,
exact read width, canonical/range failures, architectural/backend read faults,
gates and execution boundaries. Exact `DF 28` and `DF 28 DF E0` fixtures
validate their bytes independently of filenames. Native admission remains
denied; general x87 exceptions and physical x86 behavior remain open. See the
[CPU contract](../specs/CPU_CONTRACT.md#local-s2-31p-bounded-fild-m64int-rax-df-28),
[plan](../plans/STAGE_2.md#s2-31p-bare-bounded-fild-m64int-rax-df-28-partial-slice),
and [fixtures](../../tests/fixtures/x86/stage2/README.md).

## Local S2-31q x87 raw binary80 store slice

Exact bare `DB 38` (`FSTP m80fp [RAX]`) copies the original ten-byte raw
payload from Valid canonical-Normal physical TOP to `[RAX,RAX+9]` in one
`GuestMemory::write` call and performs no read. The complete canonical48 range
and checked endpoint are validated first. After a successful store, only the
old TOP tag, TOP, C1, FIP/FDP and RIP change; all payloads and other CPU state
remain preserved. Sticky PE and all four accepted RC control words are allowed.
C0/C2/C3 retention is a local deterministic policy because the SDM calls these
condition codes undefined for FSTP.

Interpreter coverage includes every TOP and wrap, raw fractional and minimum/
maximum Normal payloads, PE/C1/status retention, unaligned canaries and a
nonzero byte 11, one-write/no-read accounting, invalid source/environment
before access, full canonical range and overflow checks, denied and injected
backend/#PF writes, and fetch/truncation/STOP/zero-budget atomicity. Schema and
CLI fixtures contain literal `DB 38` and `DB 38 DF E0`; native admission denies
the operation. General exception delivery, MMU/MMIO, and physical x86 behavior
remain outside this local slice. See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-31q-bounded-fstp-m80fp-rax-db-38), [Stage 2 plan](../plans/STAGE_2.md#s2-31q-bare-bounded-fstp-m80fp-rax-db-38-partial-slice), and [report](../reports/STAGE_2_REPORT.md).

## Local S2-31t x87 binary64 load slice

Exact bare `DD 00` (`FLD m64fp [RAX]`) performs one unaligned-capable Qword
read and expands only binary64 Normal values into canonical binary80 using
integer fields. Independent literals cover ±1, adjacent fractions, minimum
Normal, maximum Normal, and unbiased exponent +1023 across all TOP×FCW pairs.
The tests check push/status policy, preserved x87 slots and unrelated CPU
state, invalid classes after one read, invalid gates before access, canonical
range errors, decode/prefix preservation and execution boundaries. Schema and
CLI fixtures cover exact `DD 00`, `DD 00 DF E0`, and zero-class rejection;
native admission remains denied. General x87 exceptions and physical x86
behavior remain unverified. See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-31t-bounded-fld-m64fp-rax-dd-00), [plan](../plans/STAGE_2.md#s2-31t-bare-bounded-fld-m64fp-rax-dd-00-partial-slice), and [fixture notes](../../tests/fixtures/x86/stage2/README.md).

## Local S2-31u signed-zero extension (historical)

S2-31u extends the S2-31t exact `DD 00` load to accept binary64 `+0` and `-0`
after its one Qword read. Independent literals require all-zero binary80 for
+0 and sign byte `0x80` for -0, both with the Zero tag. Normal values keep the
S2-31t conversion and Valid tag. In the S2-31u slice, positive and negative
nonzero minimum subnormals, infinity and NaNs remained Unsupported without
CPU commit after the read; S2-31z extends the current DD00 path to masked
binary64 subnormals. The matrix covers all TOP×FCW settings and preserves the existing
status, destination, address, and native-denial checks. See the [S2-31u CPU
contract](../specs/CPU_CONTRACT.md#local-s2-31u-bounded-fld-m64fp-rax-dd-00-signed-zero-extension),
[plan](../plans/STAGE_2.md#s2-31u-extend-bounded-fld-m64fp-rax-dd-00-to-signed-zero),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


## Local S2-31aq signed-infinity m32 stores

Exact D910/D918 accepts canonical binary80 infinity only with a Special tag
and emits signed binary32 infinity by integer-field conversion. It adds no
status flags and preserves PE. The checked Dword write is atomic and has no
data read; D918 pops only after success. Normal and signed-zero behavior stays
unchanged. See the [S2-31aq contract](../specs/CPU_CONTRACT.md#local-s2-31aq-signed-infinity-fstfstp-m32fp-rax).

## Local S2-31v signed-zero extension (historical)

S2-31v extends the bounded `D9 00` binary32 load to accept +0 and -0 after its
single Dword read. The payload is ten zero bytes for +0 or ten zero bytes with
the final sign byte `0x80` for -0; both use the Zero tag. Existing Normal
expansion and all pre-read x87/range gates remain unchanged. Nonzero
subnormals, infinity and NaNs remain Unsupported after the read with no CPU
commit. Literal zero vectors cover both signs over all TOP×FCW pairs; separate
single-setup rejection cases verify both signs of minimum subnormal after one
Dword read. Schema/CLI fixtures assert both
signed payloads, while the Unsupported fixture uses positive minimum
subnormal. See the [S2-31v CPU contract](../specs/CPU_CONTRACT.md#local-s2-31v-bounded-fld-m32fp-rax-d9-00-signed-zero-extension).

## Local S2-31y masked binary32-subnormal extension

At the S2-31y stage, exact bare `D9 00` accepts masked nonzero binary32
subnormal sources after its one Dword read. It normalizes the fraction into a canonical Normal
binary80 payload, preserves sign, assigns Valid, and sets sticky DE. The D900
preflight permits an existing DE (`FSW & 0x80dd == 0`); other load paths keep
their stricter shared gate. UE and PE are not added, C1 is cleared, and other
status/payload/TOP/pointer behavior follows the existing bounded load policy.
All four FCWs are fully masked; unmasked behavior and exception delivery are
not included. Signed zero and Normal are unchanged. At that stage infinity
and NaN remained Unsupported post-read; S2-31af now accepts signed infinity
and keeps QNaN/SNaN Unsupported. Binary80 `DB 28` subnormals retain their
separate S2-31x behavior.

Independent all-TOP×FCW literals cover signed minimum, interior and largest
subnormals. The exact D900 success fixture loads positive minimum subnormal;
the D900/DFE0 composition fixture loads negative minimum subnormal and checks
DE through FNSTSW. See the [S2-31y CPU contract](../specs/CPU_CONTRACT.md#local-s2-31y-bounded-fld-m32fp-rax-d9-00-masked-subnormal-extension).

## Local S2-31z masked binary64-subnormal extension

Current exact bare `DD 00` accepts masked nonzero binary64 subnormal sources
after its one Qword read. It normalizes the fraction into a canonical Normal
binary80 payload, preserves sign, assigns Valid, and sets sticky DE. The DD00
preflight permits an existing DE (`FSW & 0x80dd == 0`); FILD and m80 load
paths retain their stricter shared gate. UE and PE are not added, C1 is
cleared, and other status/payload/TOP/pointer behavior follows the existing
bounded load policy. All four FCWs are fully masked; unmasked behavior and
exception delivery are not included. Signed zero and Normal are unchanged.
At the S2-31z stage, infinity and NaN remained Unsupported post-read; S2-31ae
added infinity and S2-31ag adds canonical QNaN. SNaN remains Unsupported.
Binary80 `DB 28` subnormals retain their separate S2-31x behavior.

Independent all-TOP×FCW literals cover signed minimum, interior and largest
subnormals. The exact DD00 success fixture loads positive minimum subnormal;
the DD00/DFE0 composition fixture loads negative minimum subnormal and checks
DE through FNSTSW. See the [S2-31z CPU contract](../specs/CPU_CONTRACT.md#local-s2-31z-bounded-fld-m64fp-rax-dd-00-masked-subnormal-extension).

## Local S2-31aa canonical binary80-infinity load

Exact bare `DB 28` accepts only canonical signed binary80 infinities in this
added class: exponent `0x7fff`, explicit integer bit set, fraction zero. The
raw ten-byte encoding is preserved and the pushed tag is Special. The existing
`0x80df` FSW/FCW gates, Empty destination, canonical48 preflight, one read,
C1-clear, push/TOP and pointer/retirement behavior remain unchanged; no status
flags are added. Normal, signed-zero and binary80-subnormal paths are
unchanged. At the S2-31aa stage NaNs, pseudo-infinities, pseudo-denormals,
pseudo-NaNs and unnormals remained post-read Unsupported without CPU commit;
S2-31ab adds canonical QuietNaNs. Tests cover both signs over all TOP×FCW
pairs, invalid neighboring encodings, memory and execution-boundary atomicity,
and explicit JIT denial. See the [S2-31aa CPU contract](../specs/CPU_CONTRACT.md#local-s2-31aa-bounded-fld-m80fp-rax-db-28-canonical-infinities), [plan](../plans/STAGE_2.md#s2-31aa-extend-bounded-fld-m80fp-rax-db-28-to-canonical-infinities), and [fixture notes](../../tests/fixtures/x86/stage2/README.md).

## Local S2-31ab canonical binary80 QuietNaN load

Exact bare `DB 28` additionally accepts canonical binary80 QuietNaN encodings:
biased exponent `0x7fff`, explicit integer bit set, quiet bit set, and nonzero
fraction. It preserves the raw ten-byte source and pushes with Special tag.
The existing all-masked FCW and `FSW & 0x80df` gates, Empty destination,
canonical48 preflight, one read, C1-clear and push/pointer/retirement policy
remain unchanged. The load adds no status flags, including IE, and preserves
sticky PE. Signed zero, Normal, subnormal and infinity paths are unchanged.
Signaling NaNs, pseudo-NaNs, pseudo-denormals, pseudo-infinities, unnormals and
other classes remain post-read Unsupported without CPU commit. Native/JIT
admission and unmasked exception delivery remain outside the supported path.

Independent payload/sign vectors cover all TOP×FCW pairs, including negative
indefinite QNaN, exact raw payload, Special tag, PE retention and IE clear.
The positive bare QNaN and negative-indefinite `DB 28 DF E0` fixtures run
through the CLI and FNSTSW composition. At the S2-31ab stage, the no-stop,
budget-one Unsupported schema fixture used canonical positive SNaN; QNaNs and
neighboring classes did not match that exception. See the [S2-31ab CPU
contract](../specs/CPU_CONTRACT.md#local-s2-31ab-bounded-fld-m80fp-rax-db-28-canonical-quietnans), [plan](../plans/STAGE_2.md#s2-31ab-extend-bounded-fld-m80fp-rax-db-28-to-canonical-qnans), and [fixture notes](../../tests/fixtures/x86/stage2/README.md).

## Local S2-31ac canonical binary80 SignalingNaN load

Exact bare `DB 28` additionally accepts canonical binary80 SignalingNaNs:
biased exponent `0x7fff`, explicit integer bit set, quiet bit clear and
nonzero remaining fraction. It preserves all ten raw bytes, including the
signaling bit and payload, and pushes with Special tag. The all-masked FCWs,
`FSW & 0x80df` gate, Empty destination, canonical48 preflight and one-read
behavior remain unchanged. No status bits are added, including IE; PE and
C0/C2/C3 are preserved under the existing C1-clear, push/TOP, pointer and
retirement policy.

Minimum, middle and maximum payloads of both signs cover all TOP×FCW pairs
with exact raw bytes, Special tag, one read and no writes. Bare positive and
negative maximum-payload `DB 28 DF E0` fixtures run through CLI/FNSTSW. The
no-stop, budget-one Unsupported schema fixture now uses positive
pseudo-infinity; canonical SNaNs, pseudo-SNaNs, pseudo-QNaNs, pseudo-denormals
and unnormals do not match the narrow exception. Native/JIT denial and
unmasked exception delivery remain outside this path. See the [S2-31ac CPU
contract](../specs/CPU_CONTRACT.md#local-s2-31ac-bounded-fld-m80fp-rax-db-28-canonical-signalingnans),
[plan](../plans/STAGE_2.md#s2-31ac-extend-bounded-fld-m80fp-rax-db-28-to-canonical-snans),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).

## Local S2-31ad `FLD m80fp` pseudo-denormal load

Exact bare `DB 28` accepts E=0,J=1 pseudo-denormals under the existing four
all-masked FCWs, FSW gate, Empty destination and ten-byte preflight. Preserve
all raw bytes (including E=0), assign Special and add no DE or other status
flags. Coverage includes signed zero/minimum/maximum fraction and matching
exponent-one Normal controls across TOP×FCW, while pseudo-infinity/NaNs and
unnormals remain Unsupported after one read without commit. This does not add
arithmetic consumption, unmasked delivery or native x87 execution.


## Local S2-31ae `FLD m64fp` signed-infinity load

Exact bare `DD 00` additionally accepts signed binary64 infinities, preserving
the sign while constructing canonical binary80 infinity and a Special tag.
It retains the four FCWs, `FSW & 0x80dd` gate, Empty destination, one Qword
read and C1-clear/push/pointer policy. Sticky DE and PE survive the load.
At the S2-31ae stage, signed QNaNs and SNaNs rejected after one read without
CPU commit; S2-31ag adds canonical QNaN. Independent
literal vectors cover signs across TOP×FCW and a subnormal-to-infinity sequence;
positive bare and negative DD00/DFE0+FNSTSW fixtures run through the CLI.
Unmasked delivery and native/JIT execution remain outside scope. See the
[S2-31ae CPU contract](../specs/CPU_CONTRACT.md#local-s2-31ae-bounded-fld-m64fp-rax-dd-00-signed-infinities).


## Local S2-31af `FLD m32fp` signed-infinity load

Exact bare `D9 00` additionally accepts signed binary32 infinities, preserving
the sign while constructing canonical binary80 infinity and a Special tag. It
retains four all-masked FCWs, the `FSW & 0x80dd` gate, Empty destination, one
Dword read and C1-clear/push/pointer policy; sticky DE/PE survive. Signed
QNaNs and SNaNs reject after one read without CPU commit. Independent literals
cover both signs across TOP×FCW and subnormal-to-infinity sticky status. Bare
positive D900 and negative D900/DFE0+FNSTSW fixtures run through CLI. Unmasked
delivery and native/JIT execution remain outside this slice. See the
[S2-31af CPU contract](../specs/CPU_CONTRACT.md#local-s2-31af-bounded-fld-m32fp-rax-d9-00-signed-infinities).

## Local S2-31ag `FLD m64fp` canonical quiet-NaN load

Exact bare `DD 00` accepts canonical binary64 QNaNs with the quiet bit set,
converts the fraction by integer shift into canonical binary80 while
preserving sign and payload, and pushes with Special tag. No exception status
is added; sticky DE/PE and existing C1/TOP/pointer/retirement behavior remain.
The all-masked FCWs, `FSW & 0x80dd` gate, Empty destination, canonical48
eight-byte preflight and one Qword read are unchanged. SNaNs reject after one
read without commit; the current exact DD00/budget-one/no-stop schema fixture
uses SNaN. Signed quiet-only, low, middle and maximum payloads cover TOP×FCW;
a subnormal→QNaN sequence checks sticky DE/PE. Bare positive and negative
payload-bearing DD00/DFE0+FNSTSW fixtures run through CLI. JIT and native
admission remain closed. See the [S2-31ag CPU contract](../specs/CPU_CONTRACT.md#local-s2-31ag-bounded-fld-m64fp-rax-dd-00-canonical-qnans).

## Local S2-31ah `FLD m32fp` canonical quiet-NaN load

Exact bare `D9 00` accepts canonical binary32 QNaNs with exponent `0xff` and
fraction bit 22 set, preserving sign and lower payload through integer
conversion to canonical binary80 and pushing with Special tag. No exception
status is added; sticky DE/PE and existing C1/TOP/pointer/retirement behavior
remain. The all-masked FCWs, `FSW & 0x80dd` gate, Empty destination,
canonical48 four-byte preflight and one Dword read are unchanged. SNaNs were
rejected after one read at S2-31ah; S2-31aj now accepts them. The current
exact-D900/budget-one/no-stop schema fixture represents an invalid environment
gate. Signed quiet-only, low, middle and maximum payloads
cover TOP×FCW; a subnormal→QNaN sequence checks sticky DE/PE. Bare positive
and negative payload-bearing D900/DFE0+FNSTSW fixtures run through CLI. JIT
and native admission remain closed. See the [S2-31ah CPU contract](../specs/CPU_CONTRACT.md#local-s2-31ah-bounded-fld-m32fp-rax-d9-00-canonical-qnans).

## Local S2-31aj `FLD m32fp` canonical signaling-NaN load

Exact bare `D9 00` accepts canonical binary32 SNaNs with exponent `0xff`, a
nonzero fraction, and quiet bit 22 clear. Integer conversion sets binary80's
explicit integer and quiet bits, shifts the payload by 40, preserves sign, and
pushes with Special tag. Sticky IE alone is added; sticky DE/PE remain, while
ES/SF/B and other flags are not introduced. Existing zero, Normal, subnormal,
infinity and QNaN paths remain unchanged. The all-masked FCWs,
`FSW & 0x80dd` gate, Empty destination, canonical48 Dword preflight and
one-read policy remain fixed. A subsequent D900 load stays blocked until
FNCLEX clears status. Binary64 and raw binary80 SNaNs keep their separate
behavior. Native/JIT and unmasked delivery remain outside scope. See the
[S2-31aj CPU contract](../specs/CPU_CONTRACT.md#local-s2-31aj-bounded-fld-m32fp-rax-d9-00-canonical-snans).

## Local S2-31ai `FLD m64fp` canonical signaling-NaN load

Exact bare `DD 00` now accepts canonical binary64 SNaNs (exponent `0x7ff`,
nonzero fraction, quiet bit clear) of either sign. One Qword read feeds
integer-only binary80 construction with significand
`0xc000000000000000 | (fraction << 11)`, exponent `0x7fff`, preserved sign,
and Special tag. Sticky IE alone is added; DE/PE remain sticky, and ES/SF/B
are not introduced under the admitted all-masked FCWs. Existing `FSW & 0x80dd`,
Empty, push, C1/TOP/pointer/retirement and canonical48-read rules remain.
Pre-existing IE rejects before reading; subsequent DD00 loads stay blocked
until FNCLEX, and FNSTSW exposes IE. Signed low/middle/max vectors, DE/PE,
fault atomicity, gates, recovery and bare/DFE0 CLI fixtures are covered.
Binary32 SNaN and m80 raw-SNaN behavior are unchanged. Unmasked delivery and
native/JIT execution remain outside scope. See the [S2-31ai CPU contract](../specs/CPU_CONTRACT.md#local-s2-31ai-bounded-fld-m64fp-rax-dd-00-canonical-snans).

## Local S2-31ak sticky flags on exact m32/m64 FLD

Only exact D900 and DD00 now permit sticky IE, DE and PE at entry; the local
mask is `FSW & 0x80dc == 0`. ZE, OE, UE, SF, ES and B still reject before the
operand read. Accepted loads preserve IE/DE/PE, with SNaN adding IE and a
nonzero subnormal adding DE. FCWs, destination, read discipline and push
policy stay unchanged, as do FILD, m80, store, arithmetic and other x87 gates.
Tests cover every sticky-flag combination across source classes, FCWs and TOP,
cross-width compositions, repeated SNaNs, fault atomicity, forbidden status
bits, and FNCLEX recovery. The CLI/FNSTSW SNaN fixtures expose IE. See the
[S2-31ak contract](../specs/CPU_CONTRACT.md#local-s2-31ak-sticky-exception-flags-for-bounded-m32m64-fld).

## Local S2-31al signed-zero binary32 stores

Exact `D9 10` and `D9 18` accept a source only when physical TOP is tagged Zero
and its ten raw bytes are canonical binary80 zero. The stored Dword preserves
its sign. The existing four FCWs and `FSW & 0x80df` gate remain; exact zero
preserves PE, adds no flags, and clears C1. The complete canonical48 Dword
range is checked before one write and no data read. FST preserves the full
stack; FSTP updates the old TOP tag and TOP only after the write succeeds.
Literal tests cover TOP×FCW×sign×PE/C1, FLD-zero compositions, mismatched
payload/tag rejection, denied/#PF stores and canonical-range failure.
See the [S2-31al contract](../specs/CPU_CONTRACT.md#local-s2-31al-signed-zero-fst-m32fp-and-fstp-m32fp).

## Local S2-31am signed-zero binary64 stores

Exact `DD 10` and `DD 18` accept a source only when physical TOP is tagged Zero
and its raw ten-byte binary80 value is canonical zero. The stored Qword
preserves its sign. Existing all-masked FCWs and `FSW & 0x80df` remain; exact
zero preserves PE, adds no status flags, and clears C1. The full canonical48
Qword range is checked before one atomic write, with no data read. FST
preserves the complete stack; FSTP changes old TOP's tag and TOP only after the
write succeeds. Coverage includes TOP×FCW×sign×PE/C1, cross-width FLD-zero
roundtrips, mismatched tags/payloads, forbidden status, backend and #PF
failures, canonical boundary behavior, CLI/FNSTSW, and native/JIT denial.
See the [S2-31am contract](../specs/CPU_CONTRACT.md#local-s2-31am-signed-zero-fst-m64fp-and-fstp-m64fp).


## Local S2-31an signed-zero raw binary80 store

The exact `DB 38` FSTP m80fp path accepts canonical signed zero when physical
TOP is tagged Zero. It copies the original ten bytes unchanged, performs one
atomic ten-byte write and no data read, and pops only after success. Valid-tagged
canonical Normal behavior is unchanged. See the [S2-31an contract](../specs/CPU_CONTRACT.md#local-s2-31an-signed-zero-fstp-m80fp-rax-db-38).


## Local S2-31ao signed-infinity raw binary80 store

Exact `DB 38` additionally accepts Special-tagged canonical infinity and copies
all ten raw bytes unchanged. The existing Normal and signed-zero source pairs,
FCW/FSW gates, one-write/no-read, and post-success pop remain fixed. See the
[S2-31ao contract](../specs/CPU_CONTRACT.md#local-s2-31ao-signed-infinity-fstp-m80fp-rax-db-38).


## Local S2-31ap signed-infinity binary64 stores

Exact DD10/DD18 stores accept canonical binary80 infinity with Special tag and
emit signed binary64 infinity by integer-field conversion. Exact infinity adds
no flags and preserves PE. One Qword write precedes any DD18 pop; DD10 retains
the source stack. Normal and zero stores are unchanged. Tests cover TOP×FCW,
signed values, fault/range atomicity, all-width loads, fixture/CLI reachability,
and native/JIT denial. See the [S2-31ap contract](../specs/CPU_CONTRACT.md#local-s2-31ap-signed-infinity-fstfstp-m64fp-rax).


## Local S2-31ar canonical QuietNaN m80 store

Exact DB38 accepts a Special-tagged canonical binary80 QuietNaN and copies all
10 bytes unchanged. It adds no flags, retains the existing FCW/FSW gate, performs
one atomic write/no read, and pops only after success. SNaN, pseudo-QNaN and
mismatched tag/class pairs reject before access. See the [S2-31ar contract](../specs/CPU_CONTRACT.md#local-s2-31ar-canonical-quietnan-fstp-m80fp-rax-db-38).

## Local S2-31as canonical SignalingNaN m80 store

Exact `DB 38` may raw-store canonical binary80 SignalingNaN from Special-tagged
physical TOP. It preserves sign, payload, and the clear quiet bit, adds no
flags, validates the full ten-byte destination, writes once, and pops only
after success. Pseudo-SNaNs and mismatched tag/class pairs reject before
access. See the [S2-31as contract](../specs/CPU_CONTRACT.md#local-s2-31as-canonical-signalingnan-fstp-m80fp-rax-db-38).

## Local S2-31at canonical-subnormal m80 store

Exact `DB 38` may raw-store canonical binary80 subnormal from Special-tagged
physical TOP. It preserves sign and fraction bytes, adds no flags, validates
the full ten-byte destination, writes once, and pops only after success.
Pseudo-denormals and mismatched tag/class pairs reject before access. See the
[S2-31at contract](../specs/CPU_CONTRACT.md#local-s2-31at-canonical-subnormal-fstp-m80fp-rax-db-38).


## Local S2-31au canonical QuietNaN binary64 stores

Exact DD10/DD18 accepts Special-tagged canonical binary80 QuietNaNs, converting
by integer fields and truncating the lower 11 fraction bits without rounding or
new flags. Cross-width values, payload boundaries, status preservation, atomic
write/pop behavior, CLI/FNSTSW and JIT/native denial are covered. See the
[S2-31au contract](../specs/CPU_CONTRACT.md#local-s2-31au-canonical-quietnan-fstfstp-m64fp-rax-dd-10-dd-18).


## Local S2-31av canonical QuietNaN binary32 stores

Exact D910/D918 accepts Special-tagged canonical binary80 QuietNaNs and
truncates lower 40 fraction bits without rounding or new flags. Signed payload
boundaries, all-width FLD-to-store sequences, status preservation, atomic
write/pop behavior, CLI/FNSTSW and JIT/native denial are covered. See the
[S2-31av contract](../specs/CPU_CONTRACT.md#local-s2-31av-canonical-quietnan-fstfstp-m32fp-rax-d9-10-d9-18).


## Local S2-31aw masked binary80 SignalingNaN to binary64 stores

Exact DD10/DD18 accepts Special-tagged canonical binary80 SNaNs, quiets the
binary64 output, preserves sign and upper 51 payload bits, and sets IE only
after a successful atomic write. Lower 11 payload bits truncate without PE; PE
is preserved and the source's ten bytes are unchanged. The matrix covers signs,
opcodes, TOP, FCWs, PE/C1, truncation boundaries, faults, FNSTSW and FNCLEX
recovery. CLI fixtures run in the interpreter and are denied by JIT/native;
native runtime and physical acceptance remain deferred under ISSUE-019. See the
[S2-31aw CPU contract](../specs/CPU_CONTRACT.md#local-s2-31aw-canonical-signalingnan-fstfstp-m64fp-rax-dd-10-dd-18).


## Local S2-31ax masked binary80 SignalingNaN to binary32 stores

Exact D910/D918 quiet a canonical Special-tagged binary80 SNaN into binary32,
preserving sign and the upper 22 payload bits while truncating lower 40 without
rounding or PE. Sticky IE commits only after successful atomic Dword write;
PE and source bytes remain unchanged. The all-TOP/FCW and payload-boundary
matrix, QNaN differential, faults, FNSTSW/FNCLEX recovery, interpreter
fixtures and JIT/native denial are covered. Native runtime and physical
acceptance remain deferred under ISSUE-019. See the
[S2-31ax CPU contract](../specs/CPU_CONTRACT.md#local-s2-31ax-canonical-signalingnan-fstfstp-m32fp-rax-d9-10-d9-18).


## Local S2-31ay sticky IE in m32/m64 stores

Exact D910/D918 and DD10/DD18 stores admit pre-existing masked IE while
continuing to reject other forbidden status bits. Incoming IE/PE remains
sticky; successful SNaN/inexact writes add IE/PE only after memory commits, and
FSTP pops only after a successful write. Repeated stores after SNaN no longer
require an intervening FNCLEX. Cross-product ISA tests, SNaN FLD→repeated store
sequences, CLI fixtures with incoming IE, FNSTSW/FNCLEX, and JIT denial cover
the change. Other x87 gates and native admission remain unchanged; native
runtime and physical acceptance remain deferred under ISSUE-019. See the
[S2-31ay CPU contract](../specs/CPU_CONTRACT.md#local-s2-31ay-admit-sticky-ie-fstfstp-m32-m64).


## Local S2-31az sticky DE in bounded m32/m64 stores

The exact D910/D918 and DD10/DD18 store helpers now admit pre-existing masked
DE along with IE/PE (`FSW & 0x80dc == 0`). All incoming IE/DE/PE combinations
remain sticky; SNaN adds IE and inexact finite conversion adds PE only after
the atomic write, with no new DE. Other forbidden status bits and every
unrelated x87 gate remain unchanged. Binary32 subnormal FLD followed by m64
FST/FSTP confirms the loaded value converts to binary64 normal while carrying
DE forward. Binary32/64 subnormal store results remain unsupported. See the
[S2-31az contract](../specs/CPU_CONTRACT.md#local-s2-31az-admit-sticky-de-m32-m64-fstfstp).

## Local S2-30ch indexed FCMOVU

The Long64 interpreter admits exact unprefixed DA D8..DF only. It reuses the
bounded FCMOV operand and environment gate, preflights TOP/source before PF
for every non-self index even when untaken, and checks only TOP for self-index.
PF=1 raw-copies the ten source bytes and tag; PF=0 keeps the destination. The
full-state success and rejection matrices, execution-boundary atomicity, exact
fixture streams and JIT denial are local evidence; C1 and Special/NaN behavior
remain closed. See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-30ch-bounded-indexed-fcmovu-st0-sti-da-d8df).

## Local S2-30ci exact bare FCMOVNU

The interpreter admits only unprefixed Long64 `DB D9` (`FCMOVNU ST(0), ST(1)`).
It preflights both physical participants and the strict FCW/FSW gates before
PF, including the untaken path. PF=0 copies raw binary80 bytes and the source
tag; PF=1 retains the destination. Local full-state matrices cover 51,200
successes and 21,504 invalid class/tag rejects. The complete contract and
limits are recorded in the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-30ci-exact-bare-fcmovnu-st0-st1-db-d9).

## Local S2-18f non-REP LODS B/W/D/Q

The S2-18f interpreter profile admits bare `AC`, `66 AD`, bare `AD`, and
effective REX.W + `AD`, with flat DS:[RSI] and 64-bit addressing. It makes one
width-exact read and commits accumulator and DF-directed wrapping RSI only
after success. It preserves all other architectural state and RAM. The full
success matrix contains 6,144 cases and checks complete CPU state, exact RAM,
one read and zero writes. Additional checks cover aligned and unaligned starts,
4 KiB crossing, exact readable end, pointer wrap, noncanonical final RSI,
read-fault no-commit, prefix rejection, and instruction-fetch/budget boundaries.
There are 22 literal CLI fixtures. A noncanonical DS source yields #GP; a
missing bounded-memory mapping remains a harness error. REP, LOCK, 67,
segment prefixes, effective non-W REX on `AD`, prefixed `AC`, JIT and native
execution are outside this slice. See the
[CPU contract](../specs/CPU_CONTRACT.md) and
[fixture notes](../../tests/fixtures/x86/stage2/README.md).
