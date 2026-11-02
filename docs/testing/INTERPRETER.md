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
The separate local-only `s2-long-null-ds-es-v1` profile admits only bare
`8E D8`/`8E C0` null-selector loads at Long64/CPL0 with AX=0, repeated DS,
and DS→ES composition. It observes the 16-bit DS/ES selectors and explicit
unusable bits; it performs no GDT/GDTR access and adds no memory form. Native
and JIT execution stay unsupported, and this local slice does not establish
an executable Ubuntu startup prefix or close S2-22.
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

## SCAS B/W/D/Q — S2-18h local interpreter coverage

The local Long64 interpreter admits only bare `AE`, bare `AF`, `66 AF`, and
effective REX.W + `AF` (66 may precede the effective REX). It performs one
read from flat `ES:[RDI]`, compares the matching accumulator width with CMP
semantics, and updates arithmetic flags plus DF-directed wrapping RDI after a
successful read. RAX, RSI, RCX, RAM, DF, and other non-arithmetic state remain
unchanged. Unsupported prefix combinations stay admission failures; this does
not claim #UD priority. Current-instruction read faults are locally atomic,
while earlier retired instructions remain committed. JIT/native are denied.

Coverage includes an independent 6,144-case full-state/flags matrix, exact
read-only access traces, the 48-case prefix reject set, all REX.W values and
66/REX ordering, literal schema-v2 fixtures, fault/budget boundaries, and
TEST→SCAS definedness on success and fault. The fixture generator serializes
hand-authored literal values only. Unmapped source errors remain harness
errors, and a noncanonical source produces #GP(0) in this local profile.
ISSUE-016 and ISSUE-019 remain open; no physical result is claimed.

## CMPS B/W/D/Q — S2-18i local interpreter coverage

The bounded Long64 profile admits bare `A6`, `66 A7`, bare `A7`, and
effective REX.W + `A7`, with 64-bit addressing. It reads the complete element
at pre-state DS:[RSI] before attempting ES:[RDI], computes CMP flags as
`memory[RSI] - memory[RDI]`, and commits both DF-directed wrapping pointers,
flags, RIP and retirement only after both reads succeed. All other GPRs and
RAM are preserved. Two accesses occur even when RSI equals RDI. Canonical48
faults from either address produce #GP(0); missing mapping, permission, and
backend failures remain harness errors unless the backend reports an
architectural fault. The RSI-first rule is local bounded-profile policy only;
it is not hardware priority evidence for dual-invalid operands. MMIO rollback
is not claimed. JIT/native are denied.

The local matrix checks 6,144 full states and independent flags, 64 injected
read-failure combinations, exact trace order and zero writes, 48 prefix
rejects, REX.W/66 normalization, alias/overlap, alignment and page edges,
pointer wrap, canonical start/crossing and budget/fault behavior. Exactly 33
literal schema-v2 fixtures cover positive, unmapped, noncanonical, budget,
normalization, and TEST composition outcomes. Their generator serializes
literal expected values. ISSUE-020 tracks the unknown dual-invalid winner;
ISSUE-016/019 remain open and physical/native checks are deferred to the shared
final acceptance substage.

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

Локальный S2-20d boundary для bare `0F 0B` проверяет две fetch callbacks и
`#UD` только после доступности обоих байтов, включая opcode split across
adjacent executable regions. Truncated/missing second byte остаётся
`RunExit::MemoryError`; injected architectural errors становятся исходным
fault payload, а injected backend errors сохраняют свой backend error class.
Отказ первого fetch прекращает decode. Все paths оставляют retirement равным
нулю и сохраняют полное CPU state и RAM; tracing подтверждает отсутствие
data accesses. Это тестирует `FlatMemory`/callback API и не утверждает
аппаратный fetch или приоритет `#UD`. Fixture-v2 не может задать
`expected_exit: HarnessError`, поэтому truncated/injected failures проверяются
только в testkit, а полный и split-region UD2 проходят CLI snapshots. См.
[S2-20d contract](../specs/CPU_CONTRACT.md#local-ud2-fetch-boundary--s2-20d).

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

## REP MOVSB/W/D/Q — S2-19d local interpreter coverage

Exact `F3 A4`, `F3 66 A5`, `F3 A5` and effective REX.W `A5` are admitted for
byte/word/dword/qword. The decoder accepts all `48..4F` REX.W bytes and the
specified 66 ordering, while rejecting F3 66 A4, duplicate or conflicting
prefixes, segment/address overrides and non-W REX A5 forms. Existing non-REP
MOVS and REP STOS/LODS retain their admission and execution behavior.

Each repeat performs one source read followed by one destination write. Only a
successful pair commits both DF-directed index changes and decrements RCX. A
read error skips the write; a write error occurs after the source read but
commits no destination or current-iteration registers. Previously successful
transfers remain committed; replay rereads the current source. Zero count makes
no data accesses. Completion precedes STOP and the 1024-success local limit;
STOP precedes the limit. Fault/STOP/limit preserve the REP-start RIP and retire
zero instructions. RAX, flags/definedness, DF and unrelated state remain
unchanged within a run. Fresh `run()` calls reinitialize definedness;
intentional continuation can carry the returned mask through the explicit
S2-19i API. Missing mapping/backend errors remain harness errors rather than synthetic #PFs, and
this does not claim MMIO rollback or SMP atomicity.

Local evidence includes 12,288 full-state completions with alternating ordered
read/write logs, 192 injected read/write failures across four error classes,
16 STOP/replay cases, 24 work-limit cases, 36 sequential overlap cases, 48
prefix rejects, and 56 literal schema-v2 fixtures. JIT remains fail-closed;
native and physical execution are deferred. Source-first ordering remains a
local policy under [ISSUE-021](../KNOWN_ISSUES.md#issue-021--не-подтверждён-приоритет-dual-invalid-для-movs-rsirdi).
ISSUE-016 and ISSUE-020 remain open.

## REP LODSB/W/D/Q with address-size 32 — S2-19j

S2-19j admits only `F3 67 AC`, `F3 67 66 AD`, `F3 67 AD`, and effective
REX.W `F3 67 48 AD`, with the existing normalized 66/REX ordering for AD.
Each successful read uses zero-extended ESI, commits the width-specific RAX
alias, advances ESI modulo 2^32 according to DF, and decrements ECX, which
zero-extends into RCX. When ECX starts at zero, the instruction preserves the
full RCX and avoids data access. A failed next read preserves earlier elements
and leaves the current iteration uncommitted. REP completion, STOP, tracked
continuation and the 1024-read work limit use the existing S2-19c behavior.

Local checks cover all four widths, DF=0/1, high RSI/RCX halves, zero count,
post-read faults, qword DF subtraction on ESI, ESI wrap at 2^32, REP work-limit
continuation, and normalized prefix acceptance. Nine literal schema-v2
fixtures include one success per width, a qword ESI wrap, zero count with a
noncanonical RSI, zero instruction budget, initial STOP, and a partial qword
read failure; their generator serializes fixed answers. Address-size-64 REP
LODS remains covered by S2-19c. JIT/native and physical execution remain
deferred; no hardware restart-state claim is made.

## REP STOS with address-size 32 — S2-19k

S2-19k admits bounded `F3 67 AA`, `F3 67 66 AB`, `F3 67 AB`, and effective
REX.W `F3 67 48 AB`. It uses zero-extended EDI and ECX; each successful
width-sized store commits the EDI update modulo 2^32 and zero-extended ECX
decrement. ECX=0 preserves the complete RCX/RDI and performs no data access.
The existing partial-progress, STOP, tracked-definedness and 1024-success
work-limit behavior applies. No JIT/native or physical execution is claimed.

Eleven literal schema-v2 fixtures cover all widths, DF=1, EDI wrap, zero ECX
with high-half values, a partial store fault, zero instruction budget, initial
STOP, and normalized operand-size order. Their generator serializes fixed
expected snapshots. `od-isa` tests separately exercise count/width/direction
matrices, no-access zero count, and no-commit on the current failed store.

## REP MOVS with address-size 32 — S2-19l

S2-19l admits the four bounded address-size-32 MOVS widths. ECX selects the
count, ESI/EDI addresses are zero-extended for each access, and successful
element progress updates both pointers modulo 2^32 and zero-extends the
decremented ECX into RCX. ECX=0 preserves full RCX/RSI/RDI and avoids accesses.
Each element is one width-sized source read followed by an all-or-error
destination write; bytes within an element do not wrap at 4 GiB. Prior
iterations persist across a fault, overlap sees current RAM on every iteration,
and a failed current destination write does not commit that element's pointers
or count. Completion, STOP, tracked resume, and the 1024-success work limit
retain existing REP ordering. Partial exits keep the REP RIP and retire zero.

The interpreter suite checks 12,288 address-size-32 width/DF/count/flag states
alongside the existing 12,288 address-size-64 states, plus 36 address-size-32
overlap cases and ECX boundaries 0, 1, 1023, 1024, 1025, and `0xffffffff`.
Twelve literal schema-v2 fixtures record complete input/output snapshots,
including high-half aliases, zero count, pointer wrap, source/destination
faults, budget/STOP boundaries, and normalized prefixes. The generator only
serializes fixed expected data. CLI tests assert full snapshots and JIT denial;
native admission has an explicit pre-tracee rejection. ISSUE-021 keeps the
source-first policy local; dual-invalid hardware priority is unverified.
Physical, Mac mini, and SSH checks remain deferred to shared final acceptance.

## REPE SCASB address-size 32 — S2-19m

Only exact `F3 67 AE` and `67 F3 AE` (three bytes) are admitted in Long64, and
both decode to byte `RepScas`. Existing exact address-size-64 `F3 AE` remains
unchanged. S2-19m did not include `AF`; S2-19o adds bounded W/D/Q
address-size-32 `AF` forms, and S2-19p adds bounded `A7` W/D/Q CMPS. REPNE,
unsupported additional 66/REX/segment/LOCK prefixes and repeated groups remain
rejected by both decoder and executor.

The full REPE entry/restart gate still runs before checking ECX, including at
ECX=0: arithmetic values must satisfy `RFLAGS & 0x8d5 == 0x44`, and the carried
definedness mask must include `0x8d5`. Count uses ECX. Zero ECX preserves all
RCX/RDI bits and flags, performs no address check or data read, and retires
normally at RIP+3. Nonzero iterations read one byte from zero-extended EDI;
successful AL-byte comparison commits CMP flags, modulo-32 EDI ±1 and
zero-extended ECX-1. The current read fault does not commit, while prior
iterations remain. Completion/mismatch precedes STOP and then the 1024-success
limit; partial outcomes keep REP RIP and retirement zero. Tracked resume carries
the previous definedness mask.

Coverage includes 131,072 byte comparisons against an independent CMP model,
the full value/definedness/count/DF gate-rejection matrix for both prefix orders,
zero count with nonzero RCX high half and invalid EDI, ECX/EDI zero-extension,
partial read fault, completion/STOP/limit ordering, and CMP→REP success versus
TEST→REP undefined-AF rejection. Thirteen literal schema-v2 fixtures compare
complete CPU and memory snapshots; their generator serializes hand-authored
input and expected tables only. The CLI also checks complete snapshots and
keeps JIT denied. No native or physical execution is claimed.

## REPE SCASW/D/Q address-size 32 — S2-19o

Long64 additionally admits normalized F3+67 `AF` forms for SCASW/D/Q. The
prefix parser retains legacy ordering: `F3 67 48 66 AF` is W, `F3 67 66 48
AF` is Q, and `F3 48 67 AF` is D because the later 67 resets the earlier REX.
Address-size-32 `F3 67 AE` / `67 F3 AE` remains the existing SCASB slice;
address-size-32 `A7` CMPS and F2, LOCK, segment, repeated-group, and effective
non-W REX variants remain unsupported. Decoder and executor admission agree.

Before ECX or data access, including zero ECX, the full local gate still
requires `RFLAGS & 0x8d5 == 0x44` and all bits in `0x8d5` defined. Zero ECX
preserves full RCX/RDI/RAX/RSI and flags and performs no memory access. For
each nonzero element, the executor reads one contiguous 2/4/8-byte value at
zero-extended EDI, compares AX/EAX/RAX, then commits CMP flags, modulo-32 EDI
by the width according to DF, and zero-extended ECX-1. A read crossing 4 GiB
does not split or wrap. Faulting elements have no current commit; prior
elements remain. Completion/mismatch precedes STOP, followed by the 1024-read
limit; partial RIP/retirement and tracked resume follow the existing REP
contract. JIT/native remain denied.

Coverage includes 1,350 W/D/Q model states against an independent CMP flags
calculation, both DF values, upper-half preservation, zero ECX with invalid
full RDI, valid low EDI with noncanonical high bits, contiguous reads across
4 GiB, width-tail faults, and 14 literal schema-v2 snapshots. Fixture
generation serializes fixed input and expected values only. ISSUE-020 remains
open; no native or physical execution is claimed.

## REPE CMPSB address-size 32 — S2-19n

Only exact `F3 67 A6` and `67 F3 A6` (length three) are admitted in Long64;
both decode as byte `RepCmps`. Existing address-size-64 `F3 A6` remains
unchanged. `A7`, REPNE, additional 66/REX/segment/LOCK, and repeated prefixes
remain unsupported by both decoder and executor admission.

The complete local entry/restart gate is checked before ECX, including ECX=0:
`RFLAGS & 0x8d5 == 0x44` and carried definedness includes `0x8d5`. ECX=0
preserves full RCX/RSI/RDI and flags, skips both reads, and retires at RIP+3.
For each nonzero iteration, the interpreter reads one byte at zero-extended ESI
then one byte at zero-extended EDI. After both reads, it applies existing
source-minus-destination CMP flags, updates both indices modulo 2^32 by DF, and
zero-extends ECX-1 into RCX. A current read fault commits no current comparison;
a successful source read remains in the trace after destination failure.
Source-first, including the dual-invalid winner, is local ISSUE-020 policy and
does not establish hardware fault priority. Completion/mismatch precedes STOP,
then the 1024-success limit. Tracked resume carries the produced definedness
mask.

Coverage includes the independent 256×256×DF comparison matrix for address64
and both addr32 spellings; high halves, count zero, modulo-32 wrap, both read
failures, dual-invalid source-first trace, replay with changed source data,
STOP, limit/resume, CMP→REP success, and TEST→REP undefined-AF rejection.
Thirteen literal schema-v2 fixtures compare full state; the generator
serializes authored values only. Runner CLI compares snapshots and keeps JIT
denied. Native, physical x86, Mac mini and SSH execution were not run.

```sh
python3 tests/cpu/generate_stage2_rep_cmpsb_addr32.py --check
python3 tests/cpu/test_stage2_rep_cmpsb_addr32_generator.py
cargo test -p od-testkit --offline rep_cmpsb_addr32_literal_fixtures_match_full_expected_answers
cargo test -p od-runner --test fixture_v2 --offline rep_cmpsb_addr32_cli_fixtures_check_full_state_and_jit_denial
```

## REPE CMPSW/D/Q address-size 32 — S2-19p

The Long64 bounded profile admits `A7` only as REPE CMPSW/D/Q with normalized
F3+67: W=`F3 67 66 A7`, D=`F3 67 A7`, Q=`F3 67 REX.W A7`. Legacy ordering
supports `F3 67 48 66 A7` as W, `F3 67 66 48 A7` as Q and `F3 48 67 A7` as D.
All effective REX.W bytes `48..4F` are admitted. Address-size-64 CMPS widths,
`F3 A6` CMPSB, and address-size-32 CMPSB spellings are unchanged. F2, LOCK,
segments, repeated prefixes, effective non-W REX and other forms are rejected
by both decoder and executor. JIT/native admission remains closed.

The local gate runs before ECX or reads, including at ECX=0:
`RFLAGS & 0x8d5 == 0x44`, with all `0x8d5` defined. Zero count keeps full
RCX/RSI/RDI/flags and does not touch data. Nonzero iterations perform separate
contiguous width reads in source-first order from zero-extended ESI then EDI,
compare source−destination, and only after both reads commit CMP flags, ESI/EDI
modulo 2^32 by DF and zero-extended ECX-1. Each access can cross 4 GiB as one
contiguous range. Destination failure preserves the source trace but none of
the iteration state; replay reads both values again. Source-first and the
dual-invalid winner remain local ISSUE-020 policy, with no hardware-priority or
MMIO-rollback claim. Completion/mismatch precedes STOP, then the 1024-success
limit; partial exits keep RIP at REP and retire zero. Tracked resume carries
the previous definedness mask.

The independent model checks 12 values per operand, each width and DF across
the addr64 forms and normalized addr32 forms. Focused executor cases cover
zero count, full high halves, 32-bit wrap in both directions, W/D/Q accesses
starting at `0xffffffff`, destination failure and retry after changing source,
dual-invalid source-first trace, gate rejection, and decoder/executor rejection
agreement. Fourteen literal schema-v2 fixtures cover zero/high halves, equal
and mismatch results, width normalization, a full Qword read across 4 GiB,
source/destination mapping failures, zero budget, 1024-success limit,
CMP→REP and TEST→REP gate behavior, and invalid prefix/gate cases. Their
generator serializes authored bytes and complete snapshots only.

```sh
python3 tests/cpu/generate_stage2_rep_cmps_wdq_addr32.py --check
python3 -m unittest tests.cpu.test_stage2_rep_cmps_wdq_addr32_generator
cargo test -p od-testkit --offline rep_cmps_wdq_addr32_literal_fixtures_match_full_expected_answers
cargo test -p od-runner --test fixture_v2 --offline rep_cmps_wdq_addr32_cli_fixtures_check_full_state_and_jit_denial
```

See the [S2-19p CPU contract](../specs/CPU_CONTRACT.md#local-s2-19p-bounded-repe-cmpswdq-address-size-32-contract)
and [Stage 2 report](../reports/STAGE_2_REPORT.md#s2-19p--bounded-repe-cmpswdq-address-size-32).
Native/physical, Mac mini and SSH execution were not performed.

## REPNE SCASB address-size 32 — S2-19q

Only normalized `F2 67 AE` / `67 F2 AE` is admitted. Existing F3 byte and wide
SCAS behavior is unchanged. REPNE's local entry gate requires all arithmetic
flags defined but accepts any concrete values; this applies before ECX=0. A
zero count avoids data reads and preserves full RCX/RDI/RAX/flags/mask.
Successful iterations read one byte at zero-extended EDI, compute AL−byte CMP
flags, and commit flags, modulo-32 DF-directed EDI and zero-extended ECX−1.
REPNE ends on count zero or ZF=1. Completion takes priority over STOP and the
1024-read limit.

`run_with_rep_continuation` carries an opaque token between calls. It records
the original flags/definedness and expected state checkpoint; stale state and
context are rejected before callbacks/fetch. After decode, normalized
instruction identity is validated before data access. Architectural fetch/data
faults restore original flags/mask while retaining completed count/index
progress and clear the token. Backend memory errors preserve the current token.
Completion clears it. Old execution APIs reject the new instruction. Schema-v2
fixtures are one-shot fresh calls with no serialized token; a partial fixture
snapshot cannot be used as continuation state.

Tests use an independent CMP model and cover both DF settings, initial flag
values, count zero and nonzero, continuation, stale checkpoint and a later
architectural fault. Intel SDM Combined Vol. 2B REP/REPE/REPNE p. 4-568, rev.
093, is the source for the restart-flag rule. Local fault behavior is not a
claim about hardware #PF delivery or priority. JIT/native/physical/Mac mini/SSH
execution were not performed.

See the [S2-19q CPU contract](../specs/CPU_CONTRACT.md#local-s2-19q-bounded-repne-scasb-address-size-32-contract)
and [report](../reports/STAGE_2_REPORT.md#s2-19q--bounded-repne-scasb-addr32).

## REPNE CMPSB address-size 32 — S2-19r local interpreter coverage

Only normalized `F2 67 A6` / `67 F2 A6` is admitted. Existing address-size-64
and F3 profiles remain unchanged; other F2 forms and JIT/native admission stay
closed. A full arithmetic-flag-definedness gate runs before ECX or data reads,
including at zero count. Zero ECX preserves complete RCX/RSI/RDI/RFLAGS and
retires once. Nonzero iterations read zero-extended ESI first, then EDI, compare
source−destination, and commit flags, modulo-32 indices and zero-extended ECX
only after both reads succeed. REPNE ends at ECX=0 or ZF=1 before STOP/1024.

The explicit `run_with_rep_continuation` token supports SCASB or CMPSB with a
tagged operand checkpoint and shared state/context fields. State and context
are validated before callbacks/fetch; decoded operation identity (including
AE↔A6 token kind) is checked before data reads. Architectural read faults
restore entry flags/mask, retain earlier completed iterations and clear the
token. Backend errors preserve the token checkpoint; retry reads both operands
again, so no source byte is retained. Schema-v2 fixtures are fresh one-shot
calls and the testkit carries continuation only in a caller-side multi-run
sidecar. This is local source-first behavior under ISSUE-020, without hardware
dual-fault priority or MMIO rollback claims.

Coverage includes independent byte-CMP results under both DF values and zero/
nonzero counts, exact read ordering, destination backend-error retry after
changing source data, source/destination architectural faults after progress,
stale RSI and cross-kind token rejection, old entry API rejection, a caller
sidecar STOP/resume, and two literal schema-v2 full snapshots. Runner CLI checks
the complete snapshots and confirms JIT denial. No native/JIT/physical, Mac
mini or SSH execution was performed.

See the [S2-19r CPU contract](../specs/CPU_CONTRACT.md#local-s2-19r-bounded-repne-cmpsb-address-size-32-contract)
and [report](../reports/STAGE_2_REPORT.md#s2-19r--bounded-repne-cmpsb-addr32).

## REPE CMPSB — S2-19f local interpreter coverage

Only exact `F3 A6` is admitted in Long64/address64. Even at RCX zero, entry
requires arithmetic values exactly PF|ZF and full arithmetic definedness;
this is a local restart restriction. Zero count avoids both address checks and
reads, preserves flags, and retires once. For nonzero count, DS:[RSI] is read
before ES:[RDI]; source minus destination uses the existing CMP semantics.
Flags, both wrapping indices, and RCX commit only after both reads succeed.
Either error leaves the current iteration uncommitted, while a second-read
error retains the successful first-read trace. Earlier equal progress remains
replayable from the REP start without hidden source state. No MMIO rollback or
hardware dual-fault priority is claimed; ISSUE-020 records the local RSI-first
policy.

Coverage includes 131,072 independent byte CMP cases, 160 termination paths,
16,380 value/definedness/count/DF gate rejects, 48 injected source/destination
faults, both DF directions, aliases/overlap, STOP/replay, limit and STOP
priority, mismatch at iteration 1024, RCX maximum, zero count with invalid
pointers, budget zero, and wrapping/noncanonical behavior. Thirty-one literal
schema-v2 fixtures cover those representative boundaries and compositions;
the generator serializes predetermined complete answers. JIT/native remain
denied and there is no physical or Mac mini acceptance claim.

## REPE SCASW/D/Q — S2-19g local interpreter coverage

The existing exact `F3 AE` byte slice remains. This extension admits only
normalized group1 F3 with AF in word, dword, or effective-REX.W qword width:
`F3 66 AF`, `F3 AF`, and `F3 REX.W AF`. Prefix order matters: `F3 48 66 AF`
is SCASW because the later legacy 66 discards the earlier REX; `F3 66 48 AF`
is SCASQ because its final REX.W is effective and overrides 66. Each `48..4F`
REX.W byte is accepted when effective. Segment/address overrides and non-W REX
are rejected. `F3 66 AE`, REPNE, LOCK, and general REP strings remain outside
this bounded profile.

Before any read, including RCX=0, the entry/restart gate requires concrete
arithmetic values PF|ZF and all arithmetic flags defined. This is a local
profile restriction, not an x86 requirement. Zero count does not validate
RDI. Nonzero iterations compare AX/EAX/RAX with the selected memory width,
preserve all of RAX (including SCASD's upper half), and commit flags, wrapping
RDI by operand width, and RCX only after the read succeeds. Earlier equal
iterations remain committed on a later fault, with RIP at REP and no retirement;
STOP/limit replay uses the same gate without hidden state. Count completion and
mismatch take priority over STOP/limit; continuing matches check STOP before
the 1024 iteration cap. Undefinedness continuation and generic REPNE are not
claimed.

Coverage includes 864 single comparisons, 720 count/mismatch termination
paths, 49,140 W/D/Q gate rejects (the byte gate matrix remains separately
covered), 72 injected read failures, 12 STOP/replay paths and 18 work-limit
paths. Additional checks cover mismatch at iteration 1024, STOP/limit order,
initial STOP, budget zero, RCX maximum, zero count with invalid RDI,
unaligned accesses, range failure, wrapping, and noncanonical post-index
values. Exactly 59 literal schema-v2 fixtures check width behavior,
prefix-order normalization, and CMP/TEST compositions. JIT and native remain
denied; no Mac mini, SSH, or physical execution is claimed.


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

## Local register-only PSUBUSW slice — S2-35.9

The local schema-v2 `s2-long-psubusw-reg-v1` profile admits only
`66 [one optional REX.R/B] 0F D9 /r` with ModR/M.mod=3. Eight independent
little-endian u16 lanes compute `max(DEST - SRC, 0)`. Tests cover all 16×16
register pairs, every u16 lhs against selected rhs boundaries, underflow,
equality, no-underflow and unsigned-boundary sentinels, adjacent-lane isolation,
raw byte order, complete state preservation and no data-memory callbacks.
Schema/decode rejection, missing state, truncation, STOP/budget and literal
fixture execution are checked. Native/JIT and physical execution remain
excluded; SSE2 and CR0/CR4 conditions are assumptions only. Parent S2-34/S2-35
and the global `S2-35 → S2-34` dependency remain open.


## Local S2-18n address-size-32 CMPS fixture method

The 63 schema-v2 fixtures use handwritten encodings, ESI/EDI inputs, memory
bytes, and expected snapshots. `tests/cpu/generate_stage2_cmps_addr32.py`
serializes those values and writes their explicit file list to
`work/s2-18n-files.json`; it does not invoke the interpreter as an oracle.
The cases record continuous operand reads across 4 GiB and 32-bit pointer
wrap separately. Dual-unmapped fixtures expect the local ESI-first error only;
they do not establish hardware fault priority or MMIO rollback. See the
[S2-18n contract](../specs/CPU_CONTRACT.md#local-s2-18n-non-rep-address-size-32-cmps-contract).

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

## Non-REP LODS address-size 32 — S2-18k

The local profile admits only `67 AC`, `67 66 AD`, `67 AD`, and `67 48 AD` in
Long64. Prefix interpretation uses the decoder's normalized state: REX.W wins
over 66, and a later legacy prefix clears an earlier REX. Address-size 32
zero-extends the starting ESI for one width-sized flat DS read; byte lanes do
not wrap individually at 4 GiB. After a successful read, RSI is adjusted by
DF and the operand width modulo 2^32, then zero-extended. A fault preserves the
full original RSI/RAX, all state and RAM, and the faulting RIP.

The interpreter tests cover 18,432 complete successful snapshots, 96 pointer
boundary cases, 96 injected read failures and 73 enumerated prefix rejects.
They also check normalized prefix order, truncated fetch, budget/STOP/stop_rip,
RCX=0, TEST→LODS with AF undefined, and CLD/STD direction changes. Address64
LODS and REP LODS retain their earlier coverage. REP67, every segment override
(including FS/GS), and non-W REX on AD remain outside LODS admission; other
string operation admission is unchanged. JIT/native execution is not enabled.

## REP STOSW/D/Q — S2-19b local interpreter coverage

The local profile extends exact `F3 AA` with `F3 66 AB` (word), `F3 AB`
(dword), and effective REX.W `F3 REX.W AB` (qword), including all `48..4F`
REX.W forms and `66` ordering. Byte remains exact `F3 AA`; unsupported prefixes
remain admission failures. Per-iteration width writes commit before DF-directed
wrapping RDI and RCX updates. Zero count performs no access; completion precedes
STOP, STOP precedes the 1024-successful-store work limit, and partial/faulted
execution leaves RIP at the REP start with successful earlier stores retained.
The retirement budget counts the full instruction once. `run()` remains a
fresh entry and initializes definedness on each call; explicit tracked
continuation is covered by S2-19i. JIT/native admission remains closed.

Local evidence includes the full 12,288-state completion matrix, prefix and
byte regressions, literal schema-v2 fixtures, replay/fault/limit cases, and
TEST→REP definedness checks. Missing mappings remain harness errors, not guest
page faults. ISSUE-016 and ISSUE-020 remain open; physical acceptance is
deferred. See the [CPU contract](../specs/CPU_CONTRACT.md) and
[fixture notes](../../tests/fixtures/x86/stage2/README.md).

## REP LODSB/W/D/Q — S2-19c local interpreter coverage

The bounded interpreter admits exact `F3 AC` byte, `F3 66 AD` word, `F3 AD`
dword and effective REX.W `AD` qword forms. All REX.W values `48..4F` are
accepted; optional 66 ordering is normalized for `AD`, with effective REX.W
taking precedence. Unsupported prefixes remain admission failures. REP LODS
reads each element from flat `DS:[RSI]` and commits the width-appropriate RAX
alias, wrapping DF-directed RSI step and RCX decrement only after that read
succeeds. It preserves flags/definedness, DF, RDI and RAM.

Zero count avoids reads and RSI validation. Completion is checked before STOP,
then the 1024-successful-read work limit. Completed REP retires once; partial
STOP/limit/fault keeps RIP at REP start and current REP retirement zero. Prior
successful values and index/count progress persist; replay starts at the next
source element. Missing mappings remain harness errors, not synthesized guest
page faults, and no MMIO rollback is claimed. `run()` remains a fresh entry and
reinitializes definedness; S2-19i covers continuation through the explicit
tracked-entry API.

Local evidence includes the 12,288-state completion matrix, 96 injected read
errors, 16 STOP/replay states plus initial STOP and STOP at the limit boundary, 24 limit boundaries, bounded
`RCX=u64::MAX`, address/alignment/page/wrap edges, TEST→REP definedness, 48
prefix rejects and 36 full-state literal fixtures. Non-REP LODS and REP STOS
regressions remain covered. Native/JIT and physical execution remain deferred;
ISSUE-016 and ISSUE-020 remain open. See the [CPU contract](../specs/CPU_CONTRACT.md)
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).

## Tracked REP continuation — S2-19i

The fixture-v2 schema remains unchanged. Six literal cases run TEST before
REP STOSB, LODSB, or MOVSB with DF=0/1 and `RCX=1025`. The first call retires
TEST, performs 1024 REP iterations, and exposes the partial state with RIP at
REP, `RCX=1`, and `rflags_defined=0x0cc7`. The expected-data sidecar records
both this partial snapshot and the final snapshot after resuming with the
first result's definedness mask. `run()` is still the fresh-entry API; callers
that intentionally continue may pass the returned mask to
`run_with_rflags_defined()`. This is local interpreter API evidence and does
not establish hardware restart state or fault priority. ISSUE-016/020/021 stay
open; JIT/native and physical acceptance remain deferred.
## REPE CMPSW/D/Q — S2-19h local interpreter coverage

This extends exact S2-19f `F3 A6` byte support to `F3 66 A7` word, `F3 A7`
dword, and effective-REX.W `F3 A7` qword. Normalized `66 F3 A7` and
`F3 48 66 A7` are word; `F3 66 48 A7` is qword; all effective REX.W bytes
`48..4F` are accepted. `F3 66 A6`, F2, LOCK, address/segment overrides, and
non-W REX remain rejected. The arithmetic entry/restart gate applies before
every read, including zero count.

Each iteration reads width-sized DS:[RSI] before ES:[RDI], compares source
minus destination, then commits flags, width-sized DF-directed pointer steps,
and RCX only after both reads. The first read remains visible on a second-read
fault, but the current iteration is uncommitted; prior equal iterations remain
committed. Count completion and mismatch precede STOP and the 1024 iteration
limit. RSI-first is local policy only; ISSUE-020 still records the unknown
hardware dual-fault winner for CMPS B/W/D/Q. JIT/native admission and physical,
Mini, and SSH execution remain unclaimed.

Local checks include decoder prefix admission (35 requested W/D/Q rejects plus
non-W REX coverage), 864 W/D/Q single-comparison states against an independent
CMP flag oracle, 720 termination paths, 49,140 entry-gate rejects, 144 injected
read failures, 12 STOP/replay cases and 18 count/DF/width work-limit cases.
Separate cases cover alias/overlap and unaligned offsets, ordered reads,
cross-page access widths, exact range failure, canonical pointer wrap,
noncanonical post-index, zero count with invalid pointers, initial STOP,
STOP-versus-limit, RCX maximum, budget zero, truncated fetch and mismatch at
iteration 1024. Exactly 71 literal fixtures compare complete interpreter
state and typed fault answers; the CLI asserts JIT denial for every fixture. A
Linux-only test asserts native admission denial for all fixtures and was not
executed on this macOS host. No native execution, physical, Mini or SSH
acceptance is claimed.

## CLD/STD — S2-18j local interpreter coverage

Bare primary-map `FC`/`FD` only: clear/set DF, advance RIP by one and retire
once. Every other RFLAGS bit and definedness bit is preserved, and the
instructions perform no data access, pointer validation or x87/segment check.
All single-prefix forms are outside this bounded admission profile. Local unit
coverage includes 256 complete-state cases, 256 definedness subsets, 54
prefix rejects, switching, budget/stop/fetch boundaries and one-byte fetch.
`D9 FC` remains FRNDINT. The 32 literal fixtures have fresh generated inputs,
and the CLI full-state check confirms JIT denial without fallback. Validation
passed: 102 Python CPU tests, workspace check and 1,345 workspace tests. Native
admission denial is covered by a Linux-only unit test not run on this macOS host;
no physical support is claimed and S2-18 remains open.

## Non-REP STOS address-size 32 — S2-18l

The local Long64 interpreter fixture profile covers `67 AA`, `67 66 AB`,
`67 AB`, and `67 48 AB`. It uses zero-extended EDI as the starting address,
one continuous width-sized write, and a zero-extended DF-directed modulo-32
RDI update after success. Full RAX is preserved. Write errors preserve CPU
and RAM and retire zero under the local ISSUE-016 policy; this is not a
hardware restart-state assertion.

Exactly 43 literal cases cover width/direction successes, unmapped and
read-only writes, zero budget, pointer wrap, continuous 4-GiB crossing,
prefix normalization, TEST and CLD/STD compositions. Expected RDI and memory
are literal fixture values. CLI checks full CPU/definedness/RAM snapshots,
retirement and error outcomes, and confirms JIT is fail-closed. Ordered write
accesses are asserted by od-isa core tests. A Linux-only native-admission
check rejects all four forms before tracee execution; no native, Mac mini,
SSH or physical acceptance is claimed, and S2-18 remains open.

```sh
python3 tests/cpu/generate_stage2_stos_addr32.py --check
python3 -m unittest tests.cpu.test_stage2_stos_addr32_generator
cargo test -p od-runner --test fixture_v2 stos_addr32_cli_fixtures_match_literal_full_state_and_keep_jit_closed --offline
```

## Non-REP SCAS address-size 32 — S2-18m

The local Long64 interpreter profile covers `67 AE`, `67 66 AF`, `67 AF`, and
`67 48 AF`. It truncates only the initial EDI address, reads one contiguous
width-sized operand, compares with CMP semantics, commits all six arithmetic
flags, then updates RDI modulo 2^32 using DF. Full RAX is preserved. Input
arithmetic flags and definedness are unrestricted for non-REP SCAS; the REPE
entry gate does not apply. Read errors preserve state, memory, flags, and flag
definedness with zero retirement under the local model.

The 39 literal fixtures cover width/direction, mapped/unmapped inputs, zero
budget, pointer wrap, 4-GiB reads, prefix normalization, TEST and CLD/STD
composition. The core suite separately checks full-state flags, definedness,
single reads, and errors. The CLI checks exact snapshots/error details and
JIT denial. The native admission test is Linux-only and was compile-checked
but not run on this macOS host; no physical compatibility is claimed.

```sh
python3 tests/cpu/generate_stage2_scas_addr32.py --check
python3 -m unittest tests/cpu/test_stage2_scas_addr32_generator.py
cargo test -p od-runner --test fixture_v2 scas_addr32_cli_fixtures_match_literal_full_state_and_keep_jit_closed --offline
```

## Non-REP MOVS address-size 32 — S2-18o

The bounded profile accepts `67 A4`, `67 66 A5`, `67 A5`, and `67 48 A5`;
the decoder-normalized REX order cases are `48 67 A5`, `67 48 66 A5`, and
`67 66 48 A5`. The interpreter zero-extends ESI/EDI for one full source read
and one full destination write, then updates both indices modulo 2^32. Ranges
crossing 4 GiB are contiguous. Source bytes are captured before a possibly
overlapping write. Source failure stops before destination; destination
failure leaves the source read in trace and commits no CPU state. The bounded
memory backend has all-or-error writes, so no RAM prefix is committed. These
are local policies under ISSUE-016/021, not hardware restart or dual-invalid
priority claims.

The 73 literal fixtures cover 8 successes, 8 unmapped source/destination
errors, 4 read-only destination errors, 4 budget-zero cases, 16 pointer wraps,
6 continuous cross-4-GiB spans, 2 prefix normalizations, 3 TEST compositions,
8 CLD/STD compositions, 4 aliases, 4 dual-invalid cases, and 6 overlaps. One
success uses a read-only source and write-only destination. Core tests cover
stop-RIP and other fetch/stop boundaries independently. They also cover
55,296 complete states, 512 definedness masks, 192 pointer boundaries, 576
access failures, 72 aliases, 36 overlaps, 32 dual-invalid combinations, and
the requested retry/fetch/fault paths. Runner CLI verifies complete literal
snapshots and JIT denial. Native remains address-size-64 only; no native, JIT,
Mac mini, or physical acceptance is claimed here.

```sh
python3 tests/cpu/generate_stage2_movs_addr32.py --check
python3 -m unittest tests.cpu.test_stage2_movs_addr32_generator
cargo test -p od-runner --test fixture_v2 movs_addr32_cli_fixtures_match_literal_snapshots_and_keep_jit_closed --offline
cargo test -p od-runner --test fixture_v2 movs_addr32_rejects_73_prefix_variants_as_unsupported_end_to_end --offline
```


## REPNE SCASW/D/Q address-size 32 — S2-19s

The bounded form is normalized Long64 F2+67 AF at W/D/Q width, including the
REX reset and effective REX.W permutations specified in the CPU contract. Other
F2 AF/A7, LOCK, segment and duplicate-prefix forms remain closed. Existing F2
addr32 SCASB and F3 behavior are unchanged. `run_with_rep_continuation` is the
only admitted runner API. Its opaque token carries operation kind, operand and
address width, instruction length, full accumulator checkpoint, shared REP state,
context and original flags/mask. Equivalent same-length prefix order may resume;
changed instruction identity is rejected before data access.

All arithmetic flags must be defined before count/data access, while concrete
flag values are arbitrary. Zero ECX preserves full RCX/RDI/RAX/flags/mask and
performs no read. A successful iteration makes one width-sized read at
zero-extended EDI, applies width-specific accumulator-minus-memory CMP flags,
then commits EDI ± width modulo 32 and zero-extended ECX-1. A single access may
cross 4 GiB. Count-zero or ZF=1 completes before STOP; the 1024-success limit is
checked afterward. Architectural faults restore entry flags/mask while retaining
prior count/index progress and clear the token. Backend errors keep checkpoint
and token for retry. Old entry APIs reject W/D/Q.

Focused local coverage includes an independent subtraction model across all
widths, DF, starting ZF, equality/mismatch and counts; zero-count and high-half
state; contiguous Qword boundary/tail behavior; backend and architectural errors
after progress; STOP/1024 continuation; changed-length identity rejection; and
old API rejection. Three literal schema-v2 full snapshots are generated from
handwritten state by serialization only. JIT/native and physical execution were
not performed. See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-19s-bounded-repne-scaswdq-address-size-32-contract)
and [report](../reports/STAGE_2_REPORT.md#s2-19s--bounded-repne-scaswdq-addr32).

## REPNE CMPSW/D/Q address-size 32 — S2-19t

The bounded profile admits only the exact normalized Long64 `F2`+`67 A7`
W/D/Q prefix forms documented in the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-19t-bounded-repne-cmpswdq-address-size-32-contract).
Existing REPNE CMPSB and F3 profiles are unchanged; other prefix groups,
address-size-64 F2 and legacy runner APIs remain closed. All arithmetic flags
must be defined before the count/data gate, but concrete flag values do not
gate execution. Each iteration reads width bytes at ESI then EDI as two
contiguous accesses, compares source−destination, and commits both modulo-32
indices, flags, and zero-extended ECX only after both reads succeed.

The continuation token records CMPS width and source index with shared REP
checkpoint, address size, instruction length, context, and entry flags/mask.
Architectural faults restore the entry flags/mask and preserve completed prior
progress; backend errors retain the token so retry rereads both operands.
Dual-invalid ESI-first behavior remains the local ISSUE-020 policy. Three
literal schema-v2 W/D/Q snapshots use a serialization-only generator; the
token remains caller-side. The focused tests added for this slice include a
W/D/Q independent CMP model, token identity rejection/reordered-prefix resume,
faults at either read after progress, source-first dual-invalid traces,
changed-source/destination backend retry, overlapping operands, contiguous
4-GiB reads and width-exact tail failures, testkit STOP/1024/multiple resume,
and CLI snapshot/JIT denial. The full workspace tests/check, formatter,
131-test Python suite and fixture freshness checks passed before independent
review. No native/JIT/physical, Mac mini or SSH execution is claimed.

```sh
python3 tests/cpu/generate_stage2_repne_cmps_wdq_addr32.py --check
python3 -m unittest tests.cpu.test_stage2_repne_cmps_wdq_addr32_generator
cargo test -p od-testkit --offline repne_cmps_wdq_addr32
cargo test -p od-runner --test fixture_v2 repne_cmps_wdq_addr32 --offline
```

See the [plan](../plans/STAGE_2.md#s2-19t--bounded-repne-cmpswdq-addr32)
and [report](../reports/STAGE_2_REPORT.md#s2-19t--bounded-repne-cmpswdq-addr32).

## Local x87/SSE state snapshot — S2-34.1

The local `s2-long-x87-sse-state-v1` profile transfers the complete existing
x87 snapshot and independent raw SSE state through exactly one NOP. It requires
16 XMM registers at 16 bytes each and raw u32 MXCSR; the comparator observes all
register bytes and MXCSR. `diagnostic_mxcsr_mask` is fixed at `0x0000ffbf` for
this profile only (default without optional DAZ), not a universal CPUID mask or
guest `#GP` rule. The literal fixture uses instruction budget 1 and a stop RIP
immediately after `90`. Schema validation and runner execution verify local
snapshot/state preservation only. No SSE instruction, physical verification,
native/JIT path, or broader FP/SIMD behavior is established; S2-34, S2-38 and
S2-39 remain open.

## Local register MOVUPS — S2-34.2

The separate `s2-long-movups-reg-v1` profile admits exactly one legacy
`0F 10 /r` register form with ModR/M.mod=3. The destination is ModR/M.reg and
the source is ModR/M.r/m. It accepts no prefix or one effective REX directly
before `0F 10`; REX.R/B select XMM0–XMM15. REX.W/X are excluded. Legacy
`66`, F2/F3, segment/address, LOCK and duplicate/ignored prefix variants,
memory ModR/M forms, `0F 11`, SSE2 alternatives and VEX/EVEX all fail closed.
Encoding checked against Intel SDM version 093, Vol. 2B, MOVUPS.

The local diagnostic profile assumes SSE available, CR0.EM=0, CR0.TS=0 and
CR4.OSFXSR=1, without modeling those controls or their faults. Success copies
all 128 raw source bits, including self-copy, and leaves x87/MXCSR/GPR/RFLAGS
unchanged; RIP and retirement advance after the copy. No memory/alignment,
MXCSR, FP, `#UD/#NM`, native/JIT or physical execution behavior is claimed.
The literal runner case checks a high-register transfer and complete snapshot;
decoder tests cover every register pair and rejected prefix/memory/truncation
variants. This does not close S2-34/D/System. The S2-34.1 state-only profile
continues to accept only NOP.

## Reverse-direction register MOVUPS — S2-34.3

The same `s2-long-movups-reg-v1` fixture profile also admits legacy `0F 11 /r`
when ModR/M.mod=3. This direction reads the source from ModR/M.reg + REX.R and
writes the destination at ModR/M.r/m + REX.B. Literal `0F 11 C1` coverage
checks XMM0→XMM1 with all x87, 16 XMM registers and MXCSR observed; decoder
coverage checks all 256 source/destination pairs for both directions, including
`45 0F 11 C1` as XMM8→XMM9. Runner coverage also checks self-copy and full state
preservation; an executor test asserts no data-memory read or write occurs.
The schema/testkit admits both forms while rejecting the same legacy prefixes,
memory forms, REX.W/X, VEX and EVEX; native execution denies both directions.
The profile retains SSE, CR0.EM=0, CR0.TS=0 and CR4.OSFXSR=1 as explicit
preconditions. No physical or broader S2-34/D/System result is claimed, and
the S2-34.1 state-only profile remains NOP-only.

## Local MOVUPS memory load from `[RAX]` — S2-34.4

The new, separate `s2-long-movups-mem-load-v1` profile executes exactly one
legacy `0F 10 /r` with ModR/M.mod=00 and r/m=000. Bare forms target XMM0–7;
one effective REX.R selects XMM8–15. REX.B/X/W, all other memory address
forms, `0F 11`, legacy prefixes, duplicate prefixes, VEX and EVEX are rejected.
The previous `s2-long-x87-sse-state-v1` remains NOP-only and
`s2-long-movups-reg-v1` remains register-only. Encoding follows Intel SDM
revision 093, Vol. 2B, MOVUPS `0F 10 /r`, Op/En RM; the official [Intel SDM
page](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html)
lists version 093.

Execution validates the entire 16-byte canonical48 range and makes one
`GuestMemory::read` call into a temporary before copying all bytes to the target
XMM register. No integer-width helper or `OperandSize::Qword` represents the
128-bit access. Unaligned reads are allowed. x87, MXCSR, source memory, GPR and
RFLAGS remain unchanged, and RIP/retirement advance only on success. Local
injected read/fetch faults check no-commit; they do not establish native fault
priority or restart state. Assumptions are SSE available, CR0.EM=0, CR0.TS=0,
and CR4.OSFXSR=1; none of the controls, `#UD/#NM`, MXCSR operations, stores,
native/JIT or physical execution are implemented. This is one form in the
chosen Generic amd64 baseline work, not a complete or validated generic CPU
profile. The handwritten fixture checks an unaligned RAX source, XMM8, byte
order and memory preservation; local tests cover all destinations, one exact
16-byte callback, boundary ranges, fault atomicity and fail-closed neighbors.

## Local MOVUPS memory store to `[RAX]` — S2-34.5

The separate `s2-long-movups-mem-store-v1` profile executes exactly one legacy
`0F 11 /r`, ModR/M.mod=00 and r/m=000. ModR/M.reg selects source XMM0–7, with
one effective REX.R selecting XMM8–15. It rejects REX.B/X/W, other memory
address forms, `0F 10`, legacy prefixes, duplicate prefixes/REX, VEX and EVEX.
The encoding and direction follow Intel SDM revision 093, Vol. 2B, MOVUPS
`NP 0F 11 /r`, Op/En B (`xmm2/m128, xmm1`): XMM is the source and memory the
destination. The SDM permits unaligned memory operands, lists no SIMD FP
exceptions, and delegates other exceptions to Type 4 with a note that #AC
treatment varies. This local profile does not implement those gates or faults.

The executor validates the complete 16-byte canonical48 range, snapshots the
selected XMM as raw `[u8; 16]`, then calls `GuestMemory::write` once with those
16 bytes; it makes no data read and uses no integer-width helper. RIP and
retirement advance only after successful write. Failed backend writes leave
CPU state unchanged. The FlatMemory fixture backend tests all-or-error for its
private synchronous RAM, without asserting hardware 16-byte atomicity, MMIO, or
SMP behavior. Upper-u64 range overflow remains ISSUE-012. Local tests and the
literal fixture cover all 16 sources, byte order, unaligned address, canaries,
one write/no read, permission/mapping/backend errors without partial mutation,
canonical boundaries, forbidden/truncated encodings, STOP/budget, and
native/JIT denial. x86-64-v1 remains only an ABI floor, not evidence that this
slice completes a generic CPU profile. See the
[S2-34.5 plan](../plans/STAGE_2.md#s2-345--local-only-legacy-movups-rax-xmm).

## Local MOVUPS base+displacement memory forms — S2-34.6a

The separate `s2-long-movups-mem-base-v1` profile admits one legacy `0F 10 /r`
load or `0F 11 /r` store using ModR/M memory mode 00/01/10, base GPR and absent,
signed disp8 or signed disp32. REX.R extends XMM selection and REX.B extends the
base register. It rejects REX.W/X, SIB (`r/m=100`), mod=00/rm=101 RIP-relative,
address-size 32, segment and other legacy prefixes, VEX and EVEX. The earlier
state-only, register and exact `[RAX]` profiles retain their admissions.

The decoder uses the existing shared `rm_operand` parser and `MemoryOperand`;
no duplicate effective-address decoder was added. Execution validates all 16
bytes through the shared range path. Loads use one `GuestMemory::read` into a
temporary before XMM/RIP commit. Stores capture raw XMM bytes and use one
`GuestMemory::write` before RIP commit. Backend errors preserve the entire CPU
state. FlatMemory all-or-error behavior covers its local fixture RAM only and
does not claim hardware atomicity. Default DS/SS behavior and noncanonical
#SS/#GP attribution stay with `MemoryOperand`; RBP is SS-attributed, R13 is
data-attributed, and RSP/R12 cannot be encoded without the excluded SIB form.
ISSUE-012's upper-u64 AddressOverflow behavior is retained. Tests cover both
directions, all XMM registers, supported base classes, signed displacement
extrema, raw bytes, exact access callbacks, canonical range faults, failures,
and forbidden/truncated forms. The handwritten fixture exercises
`MOVUPS [R8-7],XMM8`. Native/JIT and physical execution remain outside this
local profile.

## Local MOVUPS RIP-relative memory forms — S2-34.6b

The same `s2-long-movups-mem-base-v1` schema-v2 profile now also admits legacy
`0F 10 /r` loads and `0F 11 /r` stores with ModR/M.mod=00/rm=101 and signed
disp32. The address uses the complete next RIP (optional REX + `0F` + opcode +
ModR/M + disp32), then adds the sign-extended displacement. REX.R may select
XMM8–15. SDM 093 Vol. 2A §2.2.1.6 says REX.B does not affect this special
encoding; the bounded profile rejects REX.B=1 here, so it never misdecodes the
form as R13. All other REX bits follow the existing allowlist. SIB, address/
segment/mandatory prefixes, VEX/EVEX, alignment/control gates and native/JIT
remain excluded. The previous S2-34.6a base+disp forms still work unchanged.

Decoder and interpreter tests cover both directions, zero/positive/negative
disp32, instruction lengths 7 and 8, REX.R high XMM, next-RIP and signedness
distinguishers, full CPU/memory canaries, one contiguous 16-byte callback,
canonical range checks, injected no-commit, truncated disp32 at each byte with
no data callback, STOP/budget and forbidden SIB/prefix/REX.B encodings. Two
handwritten fixtures cover high-XMM load/store and unaligned canary-preserving
memory; the fixture schema helper computes RIP-relative addresses from the
fixture's initial RIP and the decoded full instruction length. No physical x86
or alignment/control behavior is claimed. See the
[S2-34.6b plan](../plans/STAGE_2.md#s2-346b--bounded-rip-relative-movups-memory-forms)
and [CPU contract](../specs/CPU_CONTRACT.md#s2-346b-rip-relative-extension).

## Local MOVUPS SIB-addressed memory forms — S2-34.6c

The same `s2-long-movups-mem-base-v1` profile additionally admits legacy
`0F 10 /r` loads and `0F 11 /r` stores with SIB addressing and 64-bit address
size. It reuses `rm_operand`/`MemoryOperand`, including SIB scale 1/2/4/8,
index absent versus R12 according to REX.X, no-base `mod=00/base=5` regardless
of REX.B, and REX.B extension for `mod=01/10` bases. RSP/RBP actual bases retain
default SS and #SS canonical fault attribution. R12/R13 bases and index-only or
no-base references are DS/#GP attributed. The encoding rules were verified
against Intel SDM revision 093, Vol. 2A §§2.1.3–2.1.5, 2.2.1.1–2.2.1.3 and
Vol. 1 §§3.3.7.1, 3.7.4.1, 3.7.5.1, plus Vol. 2B MOVUPS:
[official SDM page](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html),
[Vol. 2A PDF](https://cdrdv2.intel.com/v1/dl/getContent/671199),
[Vol. 2B PDF](https://cdrdv2.intel.com/v1/dl/getContent/671241),
[Vol. 1 PDF](https://cdrdv2.intel.com/v1/dl/getContent/671436).

Both directions use the existing full-range 16-byte path. Loads read once into
a temporary before XMM/RIP commit; stores capture raw XMM bytes, write once,
then commit RIP. Failures leave CPU state unchanged. Flat fixture RAM
all-or-error behavior does not imply hardware atomicity or MMIO/SMP semantics.
Tests cover scales, RSP/RBP/R12/R13/no-base, absent/normal/R12 indexes, both
directions and all XMM registers, displacement modes, exact callback width,
canaries, #SS/#GP attribution, backend errors, rejected prefixes, truncation,
STOP/budget, and two literal schema-v2 fixtures. The fixture runner checks
interpreter behavior and preserves native/JIT denial. S2-34 remains open, and
x86-64-v1 remains only the ABI floor. See the
[S2-34.6c plan](../plans/STAGE_2.md#s2-346c--shared-sib-addressed-movups-memory-forms)
and [CPU contract](../specs/CPU_CONTRACT.md#s2-346c-sib-addressed-extension).


## Local XORPS register slice — S2-34.7a

The separate `s2-long-xorps-reg-v1` schema-v2 diagnostic profile admits one
legacy `0F 57 /r` register form with ModR/M.mod=3. Bare form and one effective
REX.R/B are supported; W/X, other prefixes, memory forms, VEX and EVEX are
rejected at this profile boundary. The interpreter XORs all 128 raw XMM bits
without host FP conversion and preserves MXCSR, x87, GPR, flags and other XMM
registers. Fixture state requires x87/SSE snapshots; the existing MOVUPS
profiles remain exact and separate. SSE/CR0/CR4 preconditions are assumed;
control-register gates, `#UD`/`#NM`, native/JIT and physical execution are not
modeled. See the [CPU contract](../specs/CPU_CONTRACT.md#local-profile-s2-347a-s2-long-xorps-reg-v1)
and [fixture](../../tests/fixtures/x86/stage2/s2_34_7a_xorps_register.case.json).

## Local ANDPS register slice — S2-34.7b

The separate `s2-long-andps-reg-v1` schema-v2 diagnostic profile admits one
legacy `NP 0F 54 /r` register form with ModR/M.mod=3. ModR/M.reg selects the
destination and ModR/M.r/m the source. Bare form and one effective REX.R/B
are supported for XMM0–XMM15; W/X, other prefixes, memory forms, VEX and EVEX
are rejected. The interpreter ANDs all 128 raw XMM bits without host FP
conversion and preserves MXCSR, x87, GPR, flags and other XMM registers. The
Intel SDM revision 093 Vol. 2A entry specifies Op/En A and the legacy operation
`DEST := DEST BITWISE AND SRC`; see the [official Volume 2A PDF]
(https://cdrdv2.intel.com/v1/dl/getContent/671199). Fixture state requires
x87/SSE snapshots and remains separate from MOVUPS/XORPS profiles. SSE/CR0/CR4
preconditions are assumed; control-register gates, `#UD`/`#NM`, native/JIT and
physical execution are not modeled. See the [CPU contract]
(../specs/CPU_CONTRACT.md#local-profile-s2-347b-s2-long-andps-reg-v1) and
[literal fixture](../../tests/fixtures/x86/stage2/s2_34_7b_andps_register.case.json).

## Local ORPS register slice — S2-34.7c

The separate `s2-long-orps-reg-v1` schema-v2 diagnostic profile admits one
legacy `NP 0F 56 /r` register form with ModR/M.mod=3. ModR/M.reg selects the
destination and ModR/M.r/m the source. Bare form and one effective REX.R/B are
supported for XMM0–XMM15; W/X, other prefixes, memory forms, VEX and EVEX are
rejected. The interpreter ORs all 128 raw XMM bits without host FP conversion
and preserves MXCSR, x87, GPR, flags and unrelated XMM registers. Intel SDM
revision 093 Vol. 2B specifies Op/En RM and raw OR semantics ([official Volume
2B PDF](https://cdrdv2.intel.com/v1/dl/getContent/671241), pages 4-171–4-172).
SSE/CR0/CR4 preconditions are assumed; gates, exception delivery, native/JIT
and physical execution are not modeled. See the [CPU contract]
(../specs/CPU_CONTRACT.md#local-profile-s2-347c-s2-long-orps-reg-v1) and
[literal fixture](../../tests/fixtures/x86/stage2/s2_34_7c_orps_register.case.json).

## Local ANDNPS register slice — S2-34.7d

The separate `s2-long-andnps-reg-v1` schema-v2 diagnostic profile admits one
legacy `NP 0F 55 /r` register form with ModR/M.mod=3. ModR/M.reg is the
destination and ModR/M.r/m is the source. Bare form and one effective REX.R/B
are supported for XMM0–XMM15; W/X, other prefixes, memory forms, VEX and EVEX
are rejected. The interpreter computes `(~old destination) & source` across
all 128 raw XMM bits; it preserves MXCSR, x87, GPR, flags and unrelated XMM
registers. Intel SDM revision 093 Vol. 2A specifies Op/En RM and explicitly
inverts legacy DEST before ANDing SRC ([official Volume 2A PDF]
(https://cdrdv2.intel.com/v1/dl/getContent/671199), pages 3-73–3-75). Fixture
state requires x87/SSE snapshots and remains separate from existing profiles.
SSE/CR0/CR4 preconditions are assumed; gates, `#UD`/`#NM`, native/JIT and
physical execution are not modeled. See the [CPU contract]
(../specs/CPU_CONTRACT.md#local-profile-s2-347d-s2-long-andnps-reg-v1) and
[literal fixture](../../tests/fixtures/x86/stage2/s2_34_7d_andnps_register.case.json).

## Local MOVSS register slice — S2-34.8a

The exact fixture profile `s2-long-movss-reg-v1` runs only legacy
`F3 0F 10 /r` with ModR/M.mod=3 and the bounded prefix ordering documented in
the CPU contract. It copies the low 32 raw bits and preserves the destination's
upper 96 bits. Decoder/executor tests cover all 16×16 register pairs, state
preservation with RCX and DF set, no data-memory accesses, unsupported forms,
fetch truncation, stop/budget, and absent x87/SSE state. The literal runner case
uses XMM8/XMM9 and verifies interpreter execution; native/JIT remain unsupported.
The profile does not claim feature-gate, exception-delivery, or physical CPU
verification.

## Local reverse-direction MOVSS register slice — S2-34.8b

The exact profile `s2-long-movss-store-reg-v1` runs only legacy
`F3 0F 11 /r`, ModR/M.mod=3, with one mandatory F3 and optionally one REX.R/B
immediately after F3 and before `0F`. It maps ModR/M.reg to source and
ModR/M.r/m to destination, copies the low 32 raw bits and preserves the
destination's upper 96 bits. The register-only profile makes no data-memory
callbacks; F3 is an opcode prefix, so RCX and DF are independent. Decoder and
executor coverage includes all 16×16 mappings, high registers, aliasing,
complete state preservation, unsupported prefixes/forms, truncation, missing
state, stop/budget and callback absence. Its literal XMM8→XMM9 case is
interpreter-only; native/JIT, feature gates, exception delivery and physical
verification are not claimed. See the [CPU contract]
(../specs/CPU_CONTRACT.md#local-reverse-direction-legacy-movss-register-slice--s2-348b),
[fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-348b-local-reverse-direction-register-movss),
and [plan](../plans/STAGE_2.md#s2-348b--local-legacy-reverse-direction-movss-register-to-register).

## Local legacy MOVSS m32 load from `[RAX]` — S2-34.8c

The exact profile `s2-long-movss-mem-load-v1` admits legacy `F3 0F 10 /r`
with ModR/M.mod=00/rm=000 and destination XMM0–XMM15. One optional REX.R
selects XMM8–XMM15; effective REX.B/X/W, SIB, displacement and other addressing
forms are rejected. It reads exactly four bytes once through the shared
`MemoryOperand` path, sets the destination low dword, clears bits 127:32, and
preserves the rest of CPU state. XMM/RIP commit only after a successful local
memory read. These tests establish backend failure rollback and callback
counts, not architectural fault ordering or guest fault delivery.

Coverage includes every XMM destination decode, unaligned RAX, range edge and
injected read failure, unchanged state on failure, no data callback on truncated
fetch, malformed prefix/opcode/addressing, STOP/budget, and missing SSE state.
The literal fixture runs in the interpreter; native/JIT are denied. SSE/CR0/CR4
preconditions are assumed. No CPUID/feature gate or `#UD`/`#NM` delivery is
claimed. See the [CPU contract](../specs/CPU_CONTRACT.md#local-legacy-movss-m32-load-profile--s2-348c),
[fixture](../../tests/fixtures/x86/stage2/s2_34_8c_movss_memory_load_rax.case.json),
and [plan](../plans/STAGE_2.md#s2-348c--local-legacy-movss-m32-load-from-rax).

## Local legacy MOVSS m32 store to `[RAX]` — S2-34.8d

The exact profile `s2-long-movss-mem-store-v1` accepts legacy `F3 0F 11 /r`
only for ModR/M.mod=00/rm=000 and one optional REX.R after F3. ModR/M.reg
selects the XMM0–XMM15 source; the bounded destination is `[RAX]`. REX.B/X/W,
SIB, displacement and all other prefixes/addressing forms are rejected. One
four-byte write stores the source low dword; XMM and unrelated CPU state stay
unchanged, and RIP advances only after a successful write through the shared
`MemoryOperand` validation path.

Execution coverage verifies all source indices, unaligned writes, surrounding
canaries, exact write/no-read callback counts, range boundaries, injected write
failure rollback, truncated fetch without data access, malformed encodings,
STOP/budget and absent SSE state. The literal fixture is interpreter-only;
native/JIT are denied. This establishes only local backend behavior, not fault
priority, guest exception delivery, alignment guarantees or physical behavior.
SSE/CR0/CR4 prerequisites are assumed. See the [CPU contract]
(../specs/CPU_CONTRACT.md#local-legacy-movss-m32-store-profile--s2-348d),
[fixture](../../tests/fixtures/x86/stage2/s2_34_8d_movss_memory_store_rax.case.json),
and [plan](../plans/STAGE_2.md#s2-348d--local-legacy-movss-m32-store-to-rax).

## Local legacy scalar-double MOVSD register slice — S2-34.9a

The exact profile `s2-long-movsd-reg-v1` admits legacy `F2 0F 10 /r` with
ModR/M.mod=3. One optional REX may occur after mandatory F2 and before `0F`;
only REX.R/B are allowed, selecting XMM0–XMM15. ModR/M.reg is the destination
and ModR/M.r/m the source. The interpreter copies the source low qword as raw
bits and preserves the destination upper qword. RCX and DF are untouched, and
there are no data-memory callbacks. F2 is the mandatory opcode prefix rather
than REPNE; primary-map string MOVSD `A5` is a different instruction and is
rejected by this profile.

The Intel SDM revision 093, Vol. 2B MOVSD table and legacy operation specify
the encoding, operand direction, raw low-qword copy, and unchanged upper 64
bits (pp. 4-108–4-110). Decoder tests cover all 16×16 mappings, all REX.R/B
combinations, aliasing, wrong prefixes, memory/reverse/string/VEX/EVEX forms
and fetch truncation. Executor tests use zero, all-one, alternating, and
NaN-like raw patterns with upper-bit sentinels; they check all unrelated state,
RCX/DF, missing SSE/x87 state, no memory callbacks, STOP and budget behavior.
The literal XMM8←XMM9 fixture is interpreter-only; native/JIT, CPUID gates,
`#UD`/`#NM` delivery and physical execution are not claimed. SSE2 state and
architectural preconditions are explicit local assumptions. See the [CPU
contract](../specs/CPU_CONTRACT.md#local-legacy-scalar-double-movsd-register-profile--s2-349a),
[fixture](../../tests/fixtures/x86/stage2/s2_34_9a_movsd_register.case.json),
and [plan](../plans/STAGE_2.md#s2-349a--local-legacy-scalar-double-movsd-register-to-register).

## Local reverse-direction scalar-double MOVSD register slice — S2-34.9b

The exact profile `s2-long-movsd-store-reg-v1` admits only legacy
`F2 0F 11 /r` with ModR/M.mod=3. One optional REX may follow F2 before `0F`;
only REX.R/B are admitted, selecting XMM0–XMM15. ModR/M.reg is the source and
ModR/M.r/m the destination. Execution copies the raw source low qword and
preserves the destination upper qword and all unrelated state. F2 is an opcode
prefix, not REPNE; RCX and DF are untouched, with no data-memory callbacks.
This form is separate from forward `F2 0F 10` and string `A5` MOVSD.

Decoder tests cover the full 16×16 register matrix, REX.R/B and rejection of
memory, F3/66/other prefixes, VEX/EVEX, truncation and the string opcode.
Executor tests verify raw qword copying, upper-qword preservation, full state,
missing SSE/x87 state, truncated fetch, STOP/budget and no data-memory access.
The literal XMM9←XMM8 case uses asymmetric upper sentinels, RCX=0 and DF=1;
native/JIT and physical execution are not claimed. Intel SDM revision 093
Vol. 2B lists `F2 0F 11 /r` as `MOVSD xmm2/m64,xmm1`, assigning ModR/M.reg to
the source and r/m to the destination. See the [CPU
contract](../specs/CPU_CONTRACT.md#local-reverse-direction-scalar-double-movsd-register-profile--s2-349b),
[fixture](../../tests/fixtures/x86/stage2/s2_34_9b_movsd_store_register.case.json),
and [plan](../plans/STAGE_2.md#s2-349b--local-legacy-reverse-direction-scalar-double-movsd-register).

## Local scalar-double MOVSD memory-load slice — S2-34.9c

The exact fixture profile `s2-long-movsd-mem-load-v1` admits only legacy
`F2 0F 10 /r` with ModR/M.mod=00, r/m=000 (`[RAX]`), mandatory F2 and at most
one following REX byte whose only permitted extension bit is REX.R. ModR/M.reg
selects XMM0–XMM15. REX.W/X/B, other bases, SIB/displacement, other legacy
prefixes, `0F 11`, VEX/EVEX, and string MOVSD `A5` remain rejected. F2 is the
mandatory SIMD opcode prefix, not REPNE.

Intel SDM revision 093, Vol. 2B MOVSD `F2 0F 10 /r`, Op/En A, defines the
destination in ModR/M.reg and the m64 source in ModR/M.r/m. The legacy load
copies raw source bits 63:0 to the destination low qword, clears destination
bits 127:64, and leaves vector bits above 127 unchanged (pp. 4-108–4-110).
The executor validates every byte of `[RAX..RAX+7]` against canonical48 before
issuing one eight-byte read. It commits XMM and RIP only after the read
succeeds, preserving other XMM registers, x87, MXCSR, GPR and RFLAGS.

Tests cover every XMM destination with and without REX.R, asymmetric lane
patterns and canaries, exactly one 8-byte read/no writes, canonical lower/upper
boundaries, range overflow, injected read failure, truncated/failing fetch,
missing SSE state, STOP and budget. Schema tests reject neighboring addressing,
prefix, register, store, VEX/EVEX and string forms. A handwritten XMM8 literal
fixture checks the complete interpreter snapshot and native/JIT denial.
Fault priority, guest #PF delivery, alignment claims, CPUID/control-register
gates, native/JIT execution and physical x86 remain outside this local slice.
SSE2 state and architectural preconditions are assumed. See the [CPU contract]
(../specs/CPU_CONTRACT.md#local-legacy-scalar-double-movsd-memory-load-profile--s2-349c),
[fixture](../../tests/fixtures/x86/stage2/s2_34_9c_movsd_memory_load_rax.case.json),
and [plan](../plans/STAGE_2.md#s2-349c--local-legacy-scalar-double-movsd-m64-load-from-rax).

## Local scalar-double MOVSD memory-store slice — S2-34.9d

The separate exact profile `s2-long-movsd-mem-store-v1` accepts only
`F2 0F 11 /r`, ModR/M.mod=00 and r/m=000 (`[RAX]`), with optional one REX.R
byte immediately after F2. ModR/M.reg selects source XMM0–XMM15. The interpreter
validates the full canonical48 eight-byte destination and performs exactly one
`GuestMemory::write` from the source low qword; it makes no data read. XMM,
x87, MXCSR, GPR and flags are preserved, and RIP commits only after the write
succeeds.

Coverage includes every source register, exact write width/address, canary
preservation, backend write failure, missing SSE state, stop/budget and decode
failure boundaries, and unsupported address/prefix/opcode forms. The handwritten
XMM8 fixture verifies the full interpreter snapshot and memory bytes. Native/JIT
and physical execution remain denied or unverified; hardware fault priority,
exception delivery, CPUID, and SSE control-register gates are outside this
slice. See the [CPU contract](../specs/CPU_CONTRACT.md#local-legacy-scalar-double-movsd-memory-store-profile--s2-349d),
[fixture](../../tests/fixtures/x86/stage2/s2_34_9d_movsd_memory_store_rax.case.json),
and [plan](../plans/STAGE_2.md#s2-349d--local-legacy-scalar-double-movsd-m64-store-to-rax).

## Local register-only PXOR slice — S2-35.1

The separate `s2-long-pxor-reg-v1` diagnostic profile decodes only
`66 [REX.R/B] 0F EF /r`, ModR/M.mod=3, and executes raw 128-bit XOR. It
preserves all non-destination state and causes no data callbacks. Unprefixed
MMX PXOR, memory forms, other legacy prefixes, REX.W/X, VEX/EVEX and native/JIT
are denied. The profile assumes SSE2 and CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1; it
makes no CPUID assertion and does not deliver control-gate exceptions. Parent
S2-34 and S2-35 remain open.

## Local register-only PADDB slice — S2-35.2

The local profile `s2-long-paddb-reg-v1` accepts only `66 [REX.R/B] 0F FC /r`
with ModR/M.mod=3, where the single REX byte is optional. It assumes SSE2,
CR0.EM=0, CR0.TS=0 and CR4.OSFXSR=1; those are not runtime gates or CPUID
advertisements. Tests cover all register pairs, all byte-valued operand pairs,
lane independence/wraparound, full state preservation, unsupported adjacent
encodings, fixture/runner/JIT/native rejection paths. No physical SSE result is
claimed. S2-34, S2-35 and `S2-35 → S2-34` remain open.


## Local register-only PSUBB slice — S2-35.3

The `s2-long-psubb-reg-v1` profile admits `66 [REX.R/B] 0F F8 /r`, ModR/M.mod=3,
with an optional single REX byte. It subtracts source from destination bytewise
modulo 256; no borrow crosses lanes and the result is not saturated. Tests
cover all 16×16 registers, all 65,536 single-byte operand pairs, noncommutative
order, lane sentinels, self-subtraction, REX extensions, state preservation,
missing SSE state, truncation, stop/budget, fixture acceptance and native/JIT
deny paths. The profile assumes SSE2 and CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1; no
CPUID or physical SSE claim is made. Parent S2-34 and S2-35 remain open.

## Local register-only PADDUSB slice — S2-35.4

The `s2-long-paddusb-reg-v1` profile admits only `66 [one optional REX.R/B]
0F DC /r` with ModR/M.mod=3. ModR/M.reg selects the destination XMM register
and r/m the source; each unsigned byte lane independently saturates its sum
at `0xff`. Checks cover all 16×16 register pairs, all 65,536 byte pairs,
boundaries distinguishing unsigned from signed saturation, lane independence,
state preservation, missing state, truncation, stop/budget, strict fixture
admission, and native/JIT denial. The executor does not issue data-memory
callbacks. It assumes SSE2 and CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1; it does not
query CPUID or deliver control-gate exceptions. Memory, PADDUSW/other widths,
signed saturation, other prefixes, REX.W/X, VEX/EVEX, native/JIT and physical
execution remain excluded. Parent S2-34 and S2-35, plus `S2-35 → S2-34`, remain
open.

## Local register-only PSUBUSB slice — S2-35.5

The `s2-long-psubusb-reg-v1` fixture admits exactly `66 [one optional REX.R/B] 0F D8 /r`, ModR/M.mod=3, with ModR/M.reg as destination and ModR/M.r/m as source. Each byte computes unsigned saturating subtraction independently (`max(DEST - SRC, 0)`). Tests cover all 16×16 register pairs, all 65,536 byte pairs on fixed registers, unsigned/order boundary vectors, lane isolation, self-subtraction, preservation of unrelated state and memory, and strict profile rejection. Interpreter state, truncation, STOP and budget behavior are local checks; native/JIT and physical claims are excluded. SSE2 and CR0/CR4 are assumptions only.

## Local register-only PADDW slice — S2-35.6

The local schema-v2 `s2-long-paddw-reg-v1` diagnostic profile executes eight
independent little-endian 16-bit XMM lane additions modulo 65536. Only
`66 [one optional REX.R/B] 0F FD /r` with ModR/M.mod=3 is admitted. This adds
no MMX, memory form, other widths, saturation, alternate prefixes, REX.W/X,
VEX/EVEX, JIT/native path or physical execution. SSE2 availability and relevant
CR0/CR4 preconditions are assumptions. Parent S2-34/S2-35 and the global
S2-35 → S2-34 dependency remain open.

S2-35.7 adds local-only `PSUBW` for `66 [one optional REX.R/B] 0F F9 /r`
with register ModR/M. Eight independent little-endian u16 lanes subtract
modulo 65536. The narrow profile excludes MMX, memory, PSUBD/Q, saturation,
extra prefixes, REX.W/X, VEX/EVEX, JIT/native and physical execution. SSE2 and
CR0/CR4 conditions remain assumptions. See the [CPU contract](../specs/CPU_CONTRACT.md#local-register-only-psubw-profile--s2-357).


## Local register-only PADDUSW slice — S2-35.8

The local schema-v2 `s2-long-paddusw-reg-v1` diagnostic profile executes eight
independent little-endian unsigned word additions with saturation at 65535. It
admits only `66 [one optional REX.R/B] 0F DD /r` with register ModR/M. Tests
cover all 16×16 register pairs, all u16 lhs values against selected rhs
boundaries, saturation and signed-boundary sentinels, adjacent-lane isolation,
raw little-endian state, preservation and no data-memory callbacks. Schema,
interpreter, STOP/budget and literal CLI paths are checked; JIT/native and
physical execution remain excluded. SSE2/CR0/CR4 conditions are assumptions.


## Local register-only PSUBSW slice — S2-35.13

The schema-v2 profile `s2-long-psubsw-reg-v1` covers only `66 [one optional
REX.R/B] 0F E9 /r`, register ModR/M, XMM0–15. Eight little-endian signed
word lanes subtract independently in widened i32 and clamp to the signed-word
range. Checks cover all 16×16 register pairs and all 65,536 lhs values against
seven rhs boundaries, including lane/asymmetry sentinels; complete state
preservation, no data callbacks, missing-state and fetch/STOP/budget handling,
strict decoder/schema negatives, literal CLI execution and JIT/native denial.
This local slice makes no claim for MMX, memory, other widths, prefixes,
REX.W/X, VEX/EVEX, host exception delivery, or physical acceptance. Mac mini/SSH
remains deferred to the combined final substage.

## Local register-only PADDD slice — S2-35.14

The schema-v2 profile `s2-long-paddd-reg-v1` covers only `66 [one optional
REX.R/B] 0F FE /r`, register ModR/M, XMM0–15. Four little-endian dword lanes
add modulo 2^32 independently. Checks cover all 16×16 register pairs,
deterministic raw-u32 samples checked against a u64 masked reference, lane and
carry sentinels, walking bits, full state preservation, no data callbacks,
missing-state and fetch/STOP/budget handling, strict decoder/schema negatives,
literal CLI execution and JIT/native denial. This local slice does not cover
MMX, memory, other widths, saturation, extra prefixes, REX.W/X, VEX/EVEX, host
exception delivery, or physical acceptance. Mac mini/SSH remains deferred to
the combined final substage.

## Local PUNPCKHDQ slice — S2-35.29

The local profile `s2-long-punpckhdq-reg-v1` decodes only
`66 [one optional REX.R/B] 0F 6A /r` with register ModR/M and interleaves the
two high little-endian dwords into `[DEST[2], SRC[2], DEST[3], SRC[3]]`. Tests
cover the high-dword sentinel, aliasing, all consumed byte values and walking
bits, low-half independence, every XMM pair, state and no-callback preservation,
missing state, truncation, stop/budget atomicity, strict encoding negatives,
and the literal fixture. JIT and native admission remain closed; no physical
acceptance is claimed. See the [CPU contract](../specs/CPU_CONTRACT.md) and
[Stage 2 plan](../plans/STAGE_2.md#s2-3529--local-only-bounded-register-punpckhdq).

## Local register-only PSHUFD slice — S2-35.35

The `s2-long-pshufd-reg-v1` profile admits only
`66 [one optional REX.R/B] 0F 70 /r ib`, ModR/M.mod=3. Local execution
selects four dword lanes from the original source according to imm8, safely
covering source/destination aliasing. Checks cover all 16×16 register pairs
under all 256 controls, preserved full state/source, no data callbacks, strict
decode negatives (including MMX, memory, prefix/REX, VEX and EVEX forms),
missing state and truncation immediately before imm8 with exact fetch/IP
atomicity, plus zero-budget and stop-at-current-RIP preservation. The
handwritten XMM8/XMM9 fixture is rejected by native admission;
JIT support is not enabled and physical execution is not claimed. SSE2 and
CR0/CR4 preconditions are assumptions only.

# S2-34.16 — reverse-direction register MOVUPD

Локальный профиль `s2-long-movupd-reg-reverse-v1` ограничен long-mode
encoding `66 [one optional REX.R/B] 0F 11 /r` при ModR/M.mod=3. ModR/M.reg
и REX.R выбирают источник, ModR/M.r/m и REX.B — назначение. Интерпретатор
копирует все 128 raw bits без обращения к data memory; SSE2 и CR0/CR4
предусловия предполагаются, отсутствие local SSE state отказывает без commit.
Decoder/executor покрывают пары XMM0–XMM15 и строгие отрицательные формы.
Добавлен handwritten literal high-XMM fixture; interpreter pass и JIT/native
deny проверки включены. Это локальное подтверждение формы, без alignment,
physical execution или parent S2-34 acceptance claim.


## Local register-only XORPD slice — S2-34.17

The `s2-long-xorpd-reg-v1` schema-v2 profile accepts only `66 [one optional REX.R/B] 0F 57 /r`, ModR/M.mod=3. The od-isa executor test checks all 16×16 register pairs against a full-state expected snapshot, including alias/self-XOR, and asserts zero data-memory callbacks for every pair. Additional tests cover high-XMM decoding, strict prefix/memory/VEX/EVEX negatives, missing SSE state and truncated fetch atomicity. The handwritten XMM8/XMM9 fixture is executed through both testkit and the fixture CLI runner; the runner compares the full literal result and confirms JIT returns `Unsupported` without a snapshot. Native admission remains denied. SSE2 and CR0/CR4 preconditions are assumptions. No physical execution or parent S2-34 acceptance is claimed. See the [CPU contract](../specs/CPU_CONTRACT.md#local-profile-s2-3417-s2-long-xorpd-reg-v1) and [fixture](../../tests/fixtures/x86/stage2/s2_34_17_xorpd_register.case.json).


## Local register-only ANDNPD slice — S2-34.20

The `s2-long-andnpd-reg-v1` profile accepts only `66 [one optional REX.R/B] 0F 55 /r` with ModR/M.mod=3. Destination is ModR/M.reg + REX.R, source is ModR/M.r/m + REX.B, and the raw 128-bit result is `(~old_destination) & source`. Tests distinguish operand order (`~0xF0 & 0xCC == 0x0C`, versus `0x30` reversed), cover all 16×16 XMM pairs and self-alias zero, and assert full state preservation with zero data callbacks. Decoder and schema checks reject invalid prefixes, memory, truncation, VEX/EVEX, and neighboring packed logical opcodes; prefix-free ANDNPS stays in its separate profile. Missing SSE state and truncated fetch refuse without commit. The handwritten XMM8/XMM9 fixture is interpreted, JIT returns `Unsupported`, and native denial is covered in the Linux-only x86-runner test. SSE2/CR0/CR4 prerequisites remain assumptions. No physical execution or parent S2-34 acceptance is claimed. See the [CPU contract](../specs/CPU_CONTRACT.md#local-profile-s2-3420-s2-long-andnpd-reg-v1), [plan](../plans/STAGE_2.md#s2-3420--local-only-register-andnpd), and [fixture](../../tests/fixtures/x86/stage2/s2_34_20_andnpd_register.case.json).

## Local MOVUPD `[RAX]` load — S2-34.21

The decoder/interpreter slice admits only mandatory-66 long-mode
`MOVUPD XMM,[RAX]`: ModR/M.mod=00, r/m=000, optional REX.R (`40`/`44` only).
It validates the entire canonical-48 16-byte range, performs one exact-width read
into a temporary buffer, and commits XMM plus RIP only on success. This accepts
unaligned addresses locally without asserting hardware alignment or `#AC`
semantics. Focused od-isa tests cover all 16 destinations, exact callbacks,
read failures, canonical crossing/overflow, truncation and strict addressing
negatives. The handwritten schema-v2 fixture passes through the CLI interpreter;
JIT is explicitly denied. Workspace checks and the Linux native-denial compile
check are recorded in the [Stage 2 report](../reports/STAGE_2_REPORT.md#s2-3421--local-movupd-rax-load-profile).
Physical execution status is `not_run`; parent S2-34 remains open.

## Local MOVUPD SIB memory profile — S2-34.25

The `s2-long-movupd-mem-sib-v1` fixture profile requires mandatory `66`, a
memory ModR/M with r/m=100, and a following SIB byte. The od-isa matrix checks
both load/store opcodes across mod=00/01/10, every scale, all XMM registers,
REX.R/X/B combinations and ordinary/extended/no-index/no-base encodings.
Handwritten fixtures exercise indexed load/store and mod=00/base=5 signed
absolute displacement load/store with REX.B ignored. Testkit validates the
original encoded ModR/M/SIB class and verifies that old exact `[RAX]`, base,
RIP-relative and MOVUPS profiles reject SIB even when data is mapped. Callback
checks assert one 16-byte access and unchanged CPU state after injected errors;
runner fixtures check raw payloads, canaries and interpreter/JIT behavior.
Physical execution is `not_run`; see the [Stage 2 report](../reports/STAGE_2_REPORT.md).

## Local scalar RCPSS register profile — S2-37b

The schema-v2 `s2-long-rcpss-reg-v1` profile admits exactly one Long64 legacy
`F3 [optional adjacent REX.R/B] 0F 53 /r` register instruction with
ModR/M.mod=3 and XMM0–15. The executor reads the source low dword before write,
calls `Binary32::reciprocal_sse_raw()`, and changes only the destination low
dword. The profile requires the existing raw x87/SSE snapshots; no guest data
memory access, MXCSR mutation, FP flags/exceptions, CPUID or #UD/#NM delivery
is modeled. SSE and CR0/CR4 are preconditions. The literal XMM8←XMM9 fixture
uses `2.0`→local `0.5`; exact helper bits are not a universal hardware claim.
JIT/native remain unsupported. ISSUE-025 keeps the tiny-output threshold
ambiguous; final hardware validation is reserved for the combined Mac mini
substage and checks the `3/8192` error envelope for non-flushed normal outputs.
See [CPU contract](../specs/CPU_CONTRACT.md#local-s2-37b-register-only-rcpss),
[plan](../plans/STAGE_2.md#s2-37b--local-long64-register-only-legacy-rcpss),
and [fixture](../../tests/fixtures/x86/stage2/s2_37b_rcpss_register.case.json).

## Local packed RSQRTPS register profile — S2-37c

The schema-v2 profile `s2-long-rsqrtps-reg-v1` admits one Long64 legacy
`NP [optional adjacent REX.R/B] 0F 52 /r` instruction, ModR/M.mod=3, XMM0–15.
The executor copies all 16 source bytes, computes four independent binary32
lanes with `Binary32::reciprocal_sqrt_sse_raw()`, then writes the full
destination XMM. This preserves other registers and raw MXCSR and makes no data
memory access. The profile requires raw x87/SSE snapshots and assumes SSE plus
CR0/CR4 permission; it does not model CPUID, #UD/#NM, MXCSR or SIMD exceptions.

The XMM8←XMM9 fixture contains `4.0`, `-0`, `+∞`, and `-1`; literal expected
lanes are `0.5`, `-∞`, `+0`, and the negative indefinite QNaN. This checks
lane order without using the production helper as its sole oracle. Hardware
comparisons must use the `3/8192` error bound for non-flushed normal outputs,
not require equal approximate bits. JIT/native remain unsupported; physical
validation is reserved for the combined Mac mini substage. See [CPU contract](../specs/CPU_CONTRACT.md#local-s2-37c-register-only-rsqrtps),
[plan](../plans/STAGE_2.md#s2-37c--local-long64-register-only-legacy-rsqrtps),
and [fixture](../../tests/fixtures/x86/stage2/s2_37c_rsqrtps_register.case.json).

## Local packed RCPPS register profile — S2-37d

The schema-v2 profile `s2-long-rcpps-reg-v1` admits one Long64 legacy
`NP [optional adjacent REX.R/B] 0F 53 /r` instruction, ModR/M.mod=3, XMM0–15.
The executor snapshots all 16 source bytes, computes four independent
binary32 lanes with `Binary32::reciprocal_sse_raw()`, then writes the full
destination XMM. Other CPU state, including raw MXCSR, remains unchanged and no
data-memory callback occurs. Missing SSE state refuses before commit. The
profile assumes SSE and CR0/CR4 permission; it does not model CPUID, #UD/#NM,
MXCSR or SIMD exceptions.

Literal lanes cover `2→0.5`, `-0→-∞`, `+∞→+0`, and `-2→-0.5`, plus NaNs,
subnormals, RC independence, and the local tiny-result policy. ISSUE-025 keeps
the hardware tiny-result cutoff unresolved; helper bits are not an exact CPU
claim. Final physical checks use the `3/8192` relative-error bound on
non-flushed normal outputs, without exact-bit equality. JIT/native remain
unsupported, and Mac mini checks remain in the combined final substage. See
[CPU contract](../specs/CPU_CONTRACT.md#local-s2-37d-register-only-rcpps),
[plan](../plans/STAGE_2.md#s2-37d--local-long64-register-only-legacy-rcpps),
[fixture](../../tests/fixtures/x86/stage2/s2_37d_rcpps_register.case.json), and
[ISSUE-025](../KNOWN_ISSUES.md#issue-025--intel-rcpssrcpps-threshold-inequalities-overlap).

## Local scalar MINSS register profile — S2-37e

`s2-long-minss-reg-masked-v1` admits exactly one Long64 legacy
`F3 [optional adjacent REX.R/B] 0F 5D /r` instruction, ModR/M.mod=3,
XMM0–15. The profile requires SSE state; all six MXCSR exception masks set;
DAZ/FTZ clear; and reserved bits 16–31 clear. Any RC and pre-existing sticky
flags are accepted. Unsupported state is a local profile refusal, with no
architectural fault claim.

For either QNaN or SNaN, MINSS selects operand 2 and sets IE sticky. The
higher-priority invalid condition suppresses a new DE from a subnormal in that
same scalar operation; pre-existing DE remains sticky. With no invalid
condition, a subnormal input sets DE. Non-NaN value selection uses `Binary32::min_sse_raw_result`, including operand
2 for equality and signed-zero ties. Both low operands are captured before
write for aliasing. Only destination low 32 bits and MXCSR IE/DE can change;
destination upper 96 bits and all remaining state are preserved. There is no
data-memory access, JIT or native execution. Intel SDM Rev. 093 Vol. 1 §§4.9.2, 11.5,
Appendix D Tables D-9/D-15 and Vol. 2B `MINSS` are the normative sources. Full
unmasked exception delivery and the remaining S2-38 fault ordering are open.
See [the plan](../plans/STAGE_2.md#s2-37e--local-long64-register-only-legacy-minss-with-masked-status),
[CPU contract](../specs/CPU_CONTRACT.md#local-s2-37e-register-only-minss-with-masked-status),
and [literal fixture](../../tests/fixtures/x86/stage2/s2_37e_minss_masked_register.case.json).

## Local scalar MAXSS register profile — S2-37f

`s2-long-maxss-reg-masked-v1` admits only Long64
`F3 [optional adjacent REX.R/B] 0F 5F /r`, ModR/M.mod=3, XMM0–15. The local
profile requires SSE state, all six MXCSR exception masks set, DAZ/FTZ clear
and MXCSR reserved bits clear. Any RC and existing sticky flags are allowed.

Either QNaN or SNaN selects the raw second operand and sets IE sticky. The
higher-priority invalid condition suppresses a new DE from a subnormal in that
same scalar operation; pre-existing DE remains sticky. A subnormal input sets
DE when that operation has no invalid condition. For non-NaNs, the
existing `Binary32::max_sse_raw_result` helper selects the greater value and
uses operand 2 for equal values and signed-zero ties. Only the destination low
dword and IE/DE status bits are updated; all other state is preserved. Missing
SSE, unsupported MXCSR, and invalid encodings refuse before mutation. The
profile makes no data-memory access and remains unavailable to JIT/native.
These are masked-status semantics only; unmasked #XM and wider S2-38 fault
handling remain open.

Normative source: Intel SDM Rev. 093 Vol. 1 §§4.9.2, 11.5, Appendix D Tables
D-9/D-13/D-15, and Vol. 2B `MAXSS`. See the [plan](../plans/STAGE_2.md#s2-37f--local-long64-register-only-legacy-maxss-with-masked-status),
[CPU contract](../specs/CPU_CONTRACT.md#local-s2-37f-register-only-maxss-with-masked-status),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-37f-local-masked-status-maxss-register-profile).

## Local packed MINPS register profile — S2-37g

`s2-long-minps-reg-masked-v1` admits only a bare or one-adjacent-REX Long64
register `0F 5D /r` form with ModR/M.mod=3, XMM0–15. The distinct F3-prefixed
MINSS operation remains available in its scalar profile. The profile requires
SSE state, all six MXCSR exception masks set, DAZ/FTZ clear and reserved MXCSR
bits clear; RC and pre-existing sticky bits are accepted.

MINPS snapshots both full XMM inputs, selects four raw binary32 lanes using
`Binary32::min_sse_raw_result`, and commits a full destination XMM. Any QNaN or
SNaN lane selects raw operand 2 and sets sticky IE. For each lane, invalid
status suppresses a new DE from a subnormal in that lane; otherwise a subnormal
input sets DE. Effective lane IE/DE flags are ORed across lanes, so distinct
lanes can add both bits. Prior sticky bits are preserved. Unsupported state refuses before mutation; no data-memory callback
occurs. JIT/native admission remains closed. Unmasked exception delivery and
broader S2-38 fault ordering are not implemented. See the [plan](../plans/STAGE_2.md#s2-37g--local-long64-register-only-legacy-minps-with-masked-status),
[CPU contract](../specs/CPU_CONTRACT.md#local-s2-37g-register-only-minps-with-masked-status),
and [literal fixture](../../tests/fixtures/x86/stage2/s2_37g_minps_masked_register.case.json).

## Local packed MAXPS register profile — S2-37h

`s2-long-maxps-reg-masked-v1` admits only a bare or one-adjacent-REX Long64
register `0F 5F /r` form with ModR/M.mod=3, XMM0–15. The F3-prefixed MAXSS
operation remains separate. The profile requires SSE state, all six MXCSR
exception masks set, DAZ/FTZ clear and reserved MXCSR bits clear; RC and
pre-existing sticky bits are accepted.

MAXPS snapshots both full XMM operands, selects four raw binary32 lanes using
`Binary32::max_sse_raw_result`, and commits a full destination XMM. Any QNaN or
SNaN lane selects raw operand 2 and sets sticky IE. For each lane, invalid
status suppresses a new DE from a subnormal in that lane; otherwise a subnormal
input sets DE. Effective lane IE/DE flags are ORed across lanes, so distinct
lanes can add both bits. Prior sticky bits are preserved. Unsupported state refuses before mutation; no data-memory callback
occurs. JIT/native admission remains closed. Unmasked exception delivery and
broader S2-38 fault ordering are not implemented. See the [plan](../plans/STAGE_2.md#s2-37h--local-long64-register-only-legacy-maxps-with-masked-status),
[CPU contract](../specs/CPU_CONTRACT.md#local-s2-37h-register-only-maxps-with-masked-status),
and [literal fixture](../../tests/fixtures/x86/stage2/s2_37h_maxps_masked_register.case.json).


## Local scalar MINSD register profile — S2-37i

`s2-long-minsd-reg-masked-v1` admits one Long64 `F2 [optional adjacent
REX.R/B] 0F 5D /r` register instruction (ModR/M.mod=3, XMM0–15). F2 selects
MINSD globally, F3 remains MINSS and the bare form remains MINPS. The profile
requires SSE state, all six MXCSR exception masks, DAZ/FTZ clear and reserved
bits 16–31 clear; any RC and prior sticky bits are accepted.

The operation snapshots only the two low qwords. Either QNaN or SNaN input
selects raw RHS and sets IE. Invalid status suppresses a new DE from a
subnormal in that same scalar operation; pre-existing DE remains sticky. With
no invalid condition, a subnormal low qword sets DE. Equal values and
signed-zero ties select RHS. It commits only the destination low qword and ORs
effective IE/DE, preserving the destination upper qword and all unrelated state.
Upper-qword NaN/subnormal sentinels do not affect result or flags. The literal
fixture includes those sentinels; tests cover all 256 XMM pairs, raw NaN/zero
semantics, all RC values, preflight atomicity and JIT/native denial. Full
unmasked exception delivery and S2-38 fault ordering remain open. See the
[plan](../plans/STAGE_2.md#s2-37i--local-long64-register-only-legacy-minsd-with-masked-status),
[CPU contract](../specs/CPU_CONTRACT.md#local-s2-37i-register-only-minsd-with-masked-status),
and [literal fixture](../../tests/fixtures/x86/stage2/s2_37i_minsd_masked_register.case.json).


## Local scalar MAXSD register profile — S2-37j

`s2-long-maxsd-reg-masked-v1` admits one Long64 `F2 [optional adjacent
REX.R/B] 0F 5F /r` register instruction (ModR/M.mod=3, XMM0–15); F3 remains
MAXSS and the bare form remains MAXPS. The profile requires SSE state, all six
MXCSR exception masks, DAZ/FTZ clear and reserved bits 16–31 clear.

Only low qwords are read. The raw MAX selector chooses RHS for NaN, equal-value
and signed-zero cases. New DE is suppressed by same-operation IE, while old
sticky flags remain set. Only destination low qword and MXCSR sticky IE/DE are
updated; upper qwords and unrelated state are preserved. Tests exercise all 256
pairs, literal edge cases, state refusal and closed JIT/native paths. Full
unmasked delivery remains open. See [plan](../plans/STAGE_2.md#s2-37j--local-long64-register-only-legacy-maxsd-with-masked-status),
[CPU contract](../specs/CPU_CONTRACT.md#local-s2-37j-register-only-maxsd-with-masked-status),
and [fixture](../../tests/fixtures/x86/stage2/s2_37j_maxsd_masked_register.case.json).

## Local packed MINPD register profile — S2-37k

`s2-long-minpd-reg-masked-v1` admits one Long64 `66 [optional adjacent
REX.R/B] 0F 5D /r` register instruction (ModR/M.mod=3, XMM0–15); bare MINPS,
F3 MINSS and F2 MINSD remain distinct. The profile requires SSE state, all six
MXCSR exception masks, DAZ/FTZ clear and reserved bits 16–31 clear.

The interpreter snapshots both complete XMM operands and applies the raw
Binary64 MIN selector to each 64-bit lane. Each lane selects raw RHS for NaN,
equal values and signed-zero ties. New DE is suppressed only in a lane that
also raises IE; effective IE/DE are ORed across lanes and old sticky flags are
preserved. The complete destination XMM is updated after preflight. Tests cover
all 256 register pairs, lane order and aliasing, raw NaN/zero/infinity/equality/
subnormal cases, every RC value, refusal and closed JIT/native paths. Full
unmasked delivery remains open. See [plan](../plans/STAGE_2.md#s2-37k--local-long64-register-only-legacy-minpd-with-masked-status),
[CPU contract](../specs/CPU_CONTRACT.md#local-s2-37k-register-only-minpd-with-masked-status),
and [fixture](../../tests/fixtures/x86/stage2/s2_37k_minpd_masked_register.case.json).

## Local packed MAXPD register profile — S2-37l

`s2-long-maxpd-reg-masked-v1` admits one Long64 `66 [optional adjacent
REX.R/B] 0F 5F /r` register instruction (ModR/M.mod=3, XMM0–15); bare MAXPS,
F3 MAXSS and F2 MAXSD remain distinct. The profile requires SSE state, all six
MXCSR exception masks, DAZ/FTZ clear and reserved bits 16–31 clear.

The interpreter snapshots both complete XMM operands and applies the raw
Binary64 MAX selector to each 64-bit lane. Each lane selects raw RHS for NaN,
equal values and signed-zero ties. New DE is suppressed only in a lane that
also raises IE; effective IE/DE are ORed across lanes and old sticky flags are
preserved. The complete destination XMM is committed after preflight. Tests
cover all 256 register pairs, lane order and aliasing, raw NaN/zero/infinity/
equality/subnormal cases, every RC value, refusal and closed JIT/native paths.
Full unmasked delivery remains open. See [plan](../plans/STAGE_2.md#s2-37l--local-long64-register-only-legacy-maxpd-with-masked-status),
[CPU contract](../specs/CPU_CONTRACT.md#local-s2-37l-register-only-maxpd-with-masked-status),
and [fixture](../../tests/fixtures/x86/stage2/s2_37l_maxpd_masked_register.case.json).

## Local FADDP precision profile — S2-30cz

Bare Long64 `DE C0..C7` uses the bounded FCW-controlled binary80 addition
profile from S2-30ct: twelve all-masked PC24/53/64 controls, no x87 selectors,
`FSW & 0x80df == 0`, Valid canonical nonzero Normal operands, normalized
exponent gap≤64 and a finite Normal result. Both operands are validated before
commit. Success writes to old physical `(TOP+i)&7`, empties old physical TOP,
then advances TOP; PE is sticky from `inexact` and C1 reports `incremented`.
The full x87 snapshot, instruction retirement and no-data-access behavior are
checked by local fixtures. JIT/native admission and guest exception delivery
remain unsupported. See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-30cz-bounded-faddp-sti-st0-de-c0c7),
[plan](../plans/STAGE_2.md#s2-30cz-bare-bounded-precision-faddp-sti-st0), and
[report](../reports/STAGE_2_REPORT.md#s2-30cz--bounded-faddp-sti-st0).
