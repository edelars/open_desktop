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

Первое подмножество нового JIT, сравниваемое состояние, неподдержанные формы и
источники платформы описаны в [контракте JIT](../../docs/specs/JIT_CONTRACT.md).
Проверки корректности и измерений по задачам этапа — в
[методике JIT](../../docs/testing/JIT.md). Добавлять формы инструкций можно
после подтверждения их interpreter semantics и требуемых внешних проверок.
