# od-jit

Собственный JIT x86 → ARM64. Граница crate включает IR, lowering, register
allocation, emitter, block dispatch, cache metadata и state maps. Guest memory
translation и владение macOS executable memory остаются в своих компонентах.

## Current implementation

Текущая реализация — прямой fragment translator для ограниченного
register-only профиля S0-09. Arithmetic и control flow генерируются как native
ARM64; собственный host helper материализует x86 CF/PF/AF/ZF/SF/OF. Исполняемую
память предоставляет `od-host`.

Этот fragment path не является typed-IR basic-block pipeline, запланированным
на этапе 3. Он не подтверждает multi-block dispatch, повторное использование
block cache, invalidation self-modifying code, memory lowering или общий
interpreter fallback. `od-jit` сохраняет legacy `FragmentState` interface и
не предоставляет v2 architectural-state entry; test runner отклоняет выбор
JIT backend для v2 fixtures.

S3-02 добавил `repr(C)` runtime context, raw tags, same-build `LongModeState`
field offsets и alias скалярного arithmetic helper ABI в `runtime`. Это задаёт
будущую границу block entry, но не реализует block compiler/dispatcher/helper
и не меняет legacy fragment entry. См. [runtime contract](../../docs/specs/JIT_CONTRACT.md#5-runtime-boundary-s3-02).

S3-03 добавил публичный линейный typed IR в `ir`: временные значения отделены
от четырёх GPR начального профиля, записи guest-инструкций содержат RIP и
next RIP, эффекты ограничены integer/register/flags действиями, а каждая
инструкция имеет commit marker и программа заканчивается явным branch или
exit. `ir::verify` проверяет определения и widths temporaries, номера GPR и
маски флагов, commit markers, terminal и branch boundaries, выведенные из
полного списка непрерывных IR guest records. Verifier подтверждает внутреннюю
структуру IR, причём branch fallthrough и exit target должны совпадать с
`next_rip` последней guest-записи. Verifier не подтверждает происхождение
records; decoder-backed lowering добавлен в S3-05. IR пока не подключён к
fragment compiler; memory/helper effects и runtime dispatch остаются следующими
задачами.

S3-04 добавил `block::CodeView` и `block::build_block`: borrowed code bytes
декодируются по одной инструкции через read-only `GuestMemory`, с пределами не
выше 32 инструкций/256 bytes. Записи содержат decoded instruction, guest RIP,
next RIP и точный byte range. Лимит, buffer exhaustion, unsupported form,
deferred decode boundary и branch представлены разными stop reasons; ошибка
первой инструкции даёт `NoBlock`, а не гостевой fault. Ветвление останавливает
сборку без чтения хвоста; незнакомая forward target явно остаётся unresolved.
Lowering не передаёт её verifier как доказанную branch: он исключает Jcc из IR
и завершает prefix с `NeedInterpreter` (для пустого prefix — `NoNativePrefix`).
Dispatch resolution остаётся частью S3-15. Target
внутрь decoded instruction останавливает сборку перед branch, сохраняя уже
собранный prefix. Branch target считается 64-битным wrapping сложением, как в
interpreter. `code_end` и точный `next_rip` текущей Jcc классифицируются как
известные sequential boundaries; это не декодирует следующие bytes. Другие
forward targets остаются unresolved. `BasicBlock.guest_ranges` фиксирует bytes,
но не code identity или version: tracking принадлежит S3-17, cache invalidation
— S3-23.

S3-05 добавил `lowering::lower`, который строит и verifier-checks typed IR из
decoder records для принятых S0-09 MOV imm64, ADD/SUB/CMP reg64 и JZ/JNZ.
Lowering сохраняет RIP boundaries, читает оба арифметических источника до
записи destination, не пишет GPR для CMP и не создаёт flags write для MOV.
Каждая включённая instruction имеет один commit. Commit перед terminal branch
и branch target selection составляют одну retirement boundary; observable
operation между ними запрещена контрактом.

`CodeEnd` передаётся из `BasicBlock` как единственная дополнительная target
boundary, а verifier проверяет, что records лежат внутри этого code view.
Unresolved branch не попадает в IR: prefix завершается `NeedInterpreter` на RIP
Jcc, а branch без prefix даёт `NoNativePrefix`. Mid-instruction target и другие
unsupported cases дают явный `Unsupported` до неподдержанной инструкции.
`DecodeBoundary` остаётся отдельной deferred metadata. Это lowering-представление
без ARM64 emission, publication или dispatcher; для unresolved targets остаётся
dispatch integration в S3-15. См. [JIT contract](../../docs/specs/JIT_CONTRACT.md#s3-05-decoded-effects-to-typed-ir).

S3-06 выделил `arm64::Encoder` из legacy fragment translator. Encoder хранит
words, имеет typed GPR/SP/ZR operands, проверяет размеры immediates и signed /
scaled offsets, принимает только принадлежащие encoder labels для branch/call и выдаёт
буфер только после успешного `finish`. `EncodedCode` сериализует words в
little-endian bytes. Текущий translator переведён на эти методы; код публикации,
helper ABI и allocator не добавлялись. Machine words сверяются с
[локальным Apple assembler fixture](tests/fixtures/arm64_encoder.s); это
проверяет только отмеченные instruction/operand cases, не расширение ISA.
Pair writeback отвергает base-register overlap, а LDP — повторный destination.
Полный legacy MOV fragment сохраняет прежний word vector, включая MOVZ X0,#0
на successful completion.

S3-08 добавил `offset_of!` constants для всех полей `RuntimeContextV1`, явный
AAPCS64 contract для `BlockEntry`/scalar helper, и raw host-contract failure
sentinel, отдельный от `RuntimeExitTag`. `od-host` проверяет sentinel как host
error. Его native test-only ARM64 probe вызывает typed-Encoder entry и nested
helper, проверяя callee-saved registers, SIMD canaries, output staging и helper
error statuses. Это не добавляет arithmetic semantics, x86 lowering, block
compiler, generic raw-entry invocation или dispatcher.

S3-09 добавляет `allocator::Allocation`: детерминированные постоянные homes
назначаются в порядке определения temp, с бюджетом 0..=4 для x9..x12; остальные
W64 temps получают отдельные 8-byte SP-relative slots. Caller явно задаёт
helper boundary before/after operation; boundary у terminal или после него
отклоняется, сохраняя commit/branch atomicity. Liveness рассчитывается по IR def/use, а
caller-saved register homes, живые через boundary, получают отдельные
capture/save/restore slots. Transfer API ограничивает scratch x13..x15 и
использует checked ARM64 SP-relative LDR/STR. Frame layout учитывает reserved
prefix, 16-byte alignment, диапазон адресуемых slots и transfer counters.
Allocator не встраивает call в IR и не выводит clobber из Integer. Unit coverage
добавляет `ReadGpr` value, живое через две helper boundaries, с одинаковым
capture slot. Native od-host probe проверяет четыре register homes и два spills
через helper, который портит x1..x17, одновременно сохраняя AAPCS64
callee-saved/SIMD canaries.

S3-10 добавил `emitter::emit_data_effects` для W64 MOV imm64 и ADD/SUB/CMP
reg64. Он потребляет проверенную программу вместе с принадлежащей ей
allocation, генерирует ARM64 transport через homes/spills и выдаёт temp values,
GPR candidates и отдельный CMP scratch result. Два arithmetic operands читаются
до result write, включая alias. `WriteGpr` затрагивает только отдельный staging
copy; emitter возвращает `DATA_EFFECTS_UNCOMMITTED_V1` и не меняет canonical
CPU state, flags, RIP, budget или retirement. На границе S3-10 flags оставались
`ArithmeticIncomplete`, branch terminals отвергались; S3-11 ниже добавляет
eager flags и staged branch selection. Native od-host test
исполняет полный decoder-to-emitter путь с budgets 0..=4 и сравнивает staged
vectors. Это промежуточный test harness, не committed guest execution, x86
physical comparison или benchmark; eager flags и staged branch selection
добавляет S3-11, architectural commit и recovery остаются S3-12. Текущий
S3-11 срез добавляет eager W64 CF/PF/AF/ZF/SF/OF через
общий `od-isa::integer_flags` helper и тонкий проверяемый ABI adapter. Emitter
обновляет только staged flags, выбирает staged RIP для JZ/JNZ, snapshots temps
и GPR candidates при их определении и возвращает тот же uncommitted sentinel;
canonical guest state и retirement не меняются. Helper failures возвращают
отдельный raw host-contract sentinel до чтения результата или branch selection.
Native tests покрывают taken/not-taken JZ/JNZ, MOV flag preservation, CMP без
GPR write и helper clobbers/status errors. S3-11 завершена.

S3-12 добавляет `block_emitter::emit_runtime_block` и immutable `statepoints::PcMap`.
Runtime entry использует отдельный staging frame и публикует GPR, flags, RIP,
budget и retirement только после успешных effects каждой guest instruction.
Jcc commit включает выбор target/fallthrough. На каждой внутренней границе
guards проверяют stop RIP, cooperative STOP и budget именно в этом порядке.
PC metadata покрывает emitted bytes aligned half-open ranges и хранит отдельные
guest commit points. Ranges фиксируют конкретный RIP либо явно ссылаются на
canonical RIP для общих branch-return/epilogue путей; helper failure stubs
привязаны к точному uncommitted RIP.

`od-host::PublishedCode::publish_block` хранит карту вместе с executable mapping.
Safe block execution ограничен production helper provenance и проверенным entry
RIP. Native tests покрывают committed prefix, failure attribution, planned
normal exits и branches. PC maps не обещают восстановление произвольных host
crashes, memory faults, ABI violations, panic или asynchronous snapshots; эти
возможности остаются за пределами текущего runtime slice. Independent review
исправлений прошёл без оставшихся замечаний; primary layout check остаётся за
root task. См. подробности и evidence в JIT contract и плане этапа.

Первое подмножество нового JIT, сравниваемое состояние, неподдержанные формы и
источники платформы описаны в [контракте JIT](../../docs/specs/JIT_CONTRACT.md).
Проверки корректности и измерений по задачам этапа — в
[методике JIT](../../docs/testing/JIT.md). Добавлять формы инструкций можно
после подтверждения их interpreter semantics и требуемых внешних проверок.

Локальный S3-18a partial добавляет отдельный `dispatch::run_cached` для
fixture-v1 register-only dispatcher. Cache удерживает не более одного
compiled block и opaque host-published handle; same-key reuse сравнивает
полный `TranslationKey` и пропускает lowering/emission/publication. Dependency
snapshot проверяется на hit, до публикации и до входа; stale state завершает
вызов без retry. Максимум — 512 KiB логических key/code/state-map bytes,
включая копию для публикации; allocator/RSS overhead не входит. Это не
memory JIT, MMU invalidation или performance result; S3-18 остаётся открытой
до полного invalidation/lifecycle/performance acceptance. Отдельный
`dispatch::MultiEntryBlockCache` partial добавляет opt-in FIFO cache для
`PublishedArenaBackend`: максимум 16 entries, 65,536 emitted bytes и 512 KiB
logical key/block/PC-map metadata, с capacity check перед execution. Hits не
обновляют FIFO order. Candidate over-limit отклоняется до eviction; подходящий
miss вытесняет только неактивные старые handles между завершёнными calls и
сбрасывает их до publication. Counters показывают hits/misses/compiles/
publications/evictions/resident bytes. LRU доступен как явная policy для
измерения; `MultiEntryBlockCache::new` сохраняет FIFO. Limits и обе policy
предварительны и не являются performance claim; требуется repeat end-to-end
FIFO-vs-LRU measurement. `SingleEntryBlockCache::new` и обычные
release-on-return executor contracts сохраняют прежнее поведение.

`od-bench dispatch-cache` имеет implementation-only native mode для uncached
publication, single-entry arena, FIFO(16) и opt-in LRU(16). Он требует exact
frozen S3-17 fixture list/trace и проводит admission/oracle checks до timing.
Долгий run ещё не выполнялся; эта часть не содержит performance result и не
закрывает S3-18.

S3-13 добавляет `block_compile::compile_block`: bounded block → typed lowering
→ allocation → runtime block emission. `NoNativePrefix` остаётся отдельным
результатом, а emitted block свыше 4096 bytes отклоняется до публикации.
Runner v1 opt-in подключён отдельно от legacy fragment translator; модуль
компилятора сам не публикует и не исполняет код. Реальные CLI limits и exit
mapping описаны в [JIT contract](../../docs/specs/JIT_CONTRACT.md) и отчёте
S3-13.

Первый локальный S3-15 handoff slice добавляет `compile_block_for_dispatch`
с явным `AdmittedInstructionBoundaries` из полного fixture-v1 decode.
Verifier принимает typed `DispatchBranch` только если target/fallthrough и все
IR instruction records входят в этот admission. Witness перепроверяет полный
fixture-v1 decode по тем же bytes; dispatch compiler также валидирует entry и
все decoded block boundaries. Runtime атомарно commit-ит
Jcc и возвращает выбранный RIP как `DispatchCandidate` (tag 8), без повторного
исполнения branch. Старый `compile_block` и runner сохраняют one-block
`BlockComplete`/`NeedInterpreter` behavior. `dispatch::run` итеративно
компилирует и исполняет admitted blocks с точным remaining budget, проверкой
counters и требованием положительного retirement до продолжения. Host callback
держит lifecycle одного publish/execute/drop на блок. Один отдельный
S3-18a Rust cache partial описан ниже; S3-15 CLI не использует cache и не
выполняет interpreter fallback. Полный этап остаётся открытым.

Локальный S3-17 partial добавляет `translation::TranslationKey`: полный
immutable `CodeView` content/base/end, entry, compile flavor, terminal class,
guest instruction fetch ranges и caller-provided mapping/backing identities
with versions. Dependency spans обязаны покрыть весь CodeView без gaps и
overlaps, иметь executable permission и согласованные версии. Stable IDs и
generation принадлежат caller; `CodeView` пока не подключён к MMU и не
подтверждает production invalidation. Это только typed metadata/equality,
без production cache lookup/reuse/eviction. Отдельный bounded `tools/bench`
prototype готовит recorded dispatch trace и сценарии сравнения `Vec`/HashMap;
фактический capture и timed runs ещё не выполнялись, поэтому структура не
выбрана и ускорение не заявляется. S3-17 остаётся открытой.
