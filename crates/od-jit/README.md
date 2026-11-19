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

S3-23b connects the same `CodeInvalidation` dependency matcher to the
capacity-one `MemoryPairCache`. Notifications drop matching inactive entries
between completed calls; the caller supplies the updated dependency snapshot
to the next lookup. This remains a fixture contract and does not add owner or
MMU integration.

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

S3-29a добавляет отдельный opt-in uncached correctness candidate для
demand-driven W64 ADD/SUB/CMP flags. Он хранит последний operation и исходные
operands в dedicated frame slots, материализует flags при JZ/JNZ или на
взятом exit stub и помечает pending-RFLAGS ranges/boundaries как inadmissible
для canonical snapshots. Eager остаётся default/reference; candidate не
попадает в shared cache, не выбирает production policy и не заявляет speedup.
S3-29 остаётся open до остальных correctness gates и итогового сравнения.

S3-29b adds `dispatch::run_with_nop_fallback_demand_flags_candidate`, an
explicit uncached multi-block entry point. It shares the existing NOP-fallback
dispatcher loop and changes only which compiler is called per native block.
The eager `run_with_nop_fallback` entry point and shared cache identity remain
unchanged. Native host coverage verifies candidate PC-map metadata, Jcc loops,
arithmetic/NOP/native continuation, budgets, exact stop/resume boundaries,
event handoff and mapping release. This path remains opt-in and unselected;
performance comparison and Mini checks are deferred until Stage 3 ends.

S3-33a adds separate uncached opt-in `compile_block_for_native_inc_rax*` and
`dispatch::run_with_native_inc_rax*` paths for exact admitted bare `48 FF C0`.
Lowering uses W64 Add+1 and an existing IR `WriteMask`; runtime merges only
PF/AF/ZF/SF/OF, preserving CF. A pending full writer in the demand-flags
candidate is materialized before INC, with its live allocator homes preserved
across the helper call. INC then materializes its partial flags eagerly, so no
new pending descriptor/helper ABI is needed. The S3-29c interpreter-bridge path,
eager default and shared cache remain unchanged. Host correctness covers both
CF inputs, arithmetic edges, native branch consumers, retirement, candidate
budget and STOP-RIP handling. This slice remains exact-form only and makes no
performance claim; Mini and measurements remain deferred to the end of Stage 3.
See [S3-33a report](../../docs/reports/STAGE_3_S3-33a_2026-10-30.md).

S3-33b adds a separate uncached opt-in `S0_09NopDecRaxAdmission` and
`compile_block_for_native_dec_rax*` / `dispatch::run_with_native_dec_rax*`
paths for exact bare `48 FF C8`. W64 Subtract+1 updates only PF/AF/ZF/SF/OF,
preserving CF and other bits. The demand-flags candidate materializes a pending
full writer before DEC and preserves allocator live homes across the helper;
the partial flags then merge eagerly without a new pending descriptor or helper
ABI. INC admission and APIs remain separate and INC-only. This partial correctness
slice is uncached and makes no performance claim; Mini and performance checks
remain deferred to the end of Stage 3. See
[S3-33b report](../../docs/reports/STAGE_3_S3-33b_2026-10-30.md).

S3-33c adds a separate uncached opt-in `S0_09NopNegRaxAdmission` and eager /
demand-flags `compile_block_for_native_neg_rax*` and
`dispatch::run_with_native_neg_rax*` APIs for exact bare `48 F7 D8`. Lowering
uses Read RAX, constant zero and W64 `Subtract(0, RAX)` with full arithmetic
flags, then writes RAX and commits. NEG consumes no old CF; demand-flags replaces
the pending full writer without new IR, descriptor, or helper ABI. INC and DEC
admissions remain operation-specific. This is uncached partial correctness
evidence with no performance claim; Mini and performance checks remain deferred
until Stage 3 ends. See
[S3-33c report](../../docs/reports/STAGE_3_S3-33c_2026-10-30.md).

S3-33d adds a separate uncached opt-in profile for exact bare `48 83 C0 ib`
(`ADD RAX, sign-extended imm8`) through `S0_09NopAddRaxImm8Admission` and
`compile_block_for_native_add_rax_imm8*` /
`dispatch::run_with_native_add_rax_imm8*`. Admission records the decoder-owned
sign-extended immediate for all 256 byte values and cross-checks it against the
source byte. Lowering uses the decoder-owned value as a W64 constant with full
arithmetic flags; existing INC/DEC/NEG and generic
admissions stay unchanged. This is partial correctness evidence only; Mini and
performance checks remain deferred until the end of Stage 3. See the
[S3-33d report](../../docs/reports/STAGE_3_S3-33d_2026-10-30.md).

S3-33e adds a separate uncached opt-in `S0_09NopCmpRaxImm8Admission` and
eager/demand-flags compile and dispatch APIs for exact bare `48 83 F8 ib`
(`CMP RAX, sign-extended imm8`). Admission cross-checks all 256 source-byte
values against the decoder-owned sign-extended value. Lowering uses W64
`Compare` with full arithmetic flags and no GPR write; prior profiles remain
operation-specific. This is partial correctness evidence only. Mini and
performance checks remain deferred to the end of Stage 3. See
[S3-33e report](../../docs/reports/STAGE_3_S3-33e_2026-10-30.md).

S3-33f adds a separate uncached opt-in `S0_09NopSubRaxImm8Admission` and
eager/demand-flags compile and dispatch APIs for exact bare `48 83 E8 ib`
(`SUB RAX, sign-extended imm8`). Admission cross-checks all 256 source-byte
values against the decoder-owned sign-extended immediate. Lowering reads RAX,
uses that decoded value as a W64 constant, performs `Subtract` with full
`FlagEffect::Arithmetic`, writes RAX, and commits once. Existing arithmetic
profiles remain operation-specific. This is partial correctness evidence only;
Mini and performance checks remain deferred until the end of Stage 3. See
[S3-33f report](../../docs/reports/STAGE_3_S3-33f_2026-10-30.md).

S3-33g adds a separate uncached opt-in `S0_09NopNotRaxAdmission` and
eager/demand-flags compile and dispatch APIs for exact bare `48 F7 D0`
(`NOT RAX`). Lowering uses W64 `u64::MAX - RAX` with `FlagEffect::Preserve`.
The runtime emitter accepts only this narrow W64 subtract form with a result;
NOT itself uses no flags helper and leaves full RFLAGS and any prior pending
writer descriptor/operands untouched. Mini and performance checks remain
deferred until the end of Stage 3. See
[S3-33g report](../../docs/reports/STAGE_3_S3-33g_2026-10-30.md).

S3-33h adds a separate uncached opt-in profile for exact bare `48 8D 03`
(`LEA RAX,[RBX]`). Admission checks both decoder shape and the exact three
source bytes; equivalent SIB/displacement encodings, other registers, address
widths, prefixes, truncation, and branch targets inside the instruction are
rejected. Lowering is only `ReadGpr(RBX,W64)` → `WriteGpr(RAX,W64)` → one
commit. LEA has no memory/data helper, arithmetic or flags effect, and keeps
any pending-flags descriptor and saved operands intact. Separate eager and
demand-flags candidate compile/dispatch APIs remain uncached and opt-in. Mini
and performance checks remain deferred until the end of Stage 3. See
[S3-33h report](../../docs/reports/STAGE_3_S3-33h_2026-10-30.md).

S3-33j adds separate eager/candidate uncached entry points for exact bare
`LEA RAX,[RBX+disp8]` (`48 8D 43 db`). All 256 displacement bytes are admitted
only when decoder metadata reports W64 RAX, address64, RBX base, no index,
scale 1, and the same signed i8 displacement. It lowers to RBX read, signed
constant, W64 preserving Add, RAX write, and one commit. It performs no data
access, canonicality check, or helper call; flags and pending-writer state are
preserved. The previous no-displacement profile remains distinct. Exact local
differential evidence does not confirm physical source bytes; Mini and
performance checks remain deferred until Stage 3 ends. See the
[S3-33j report](../../docs/reports/STAGE_3_S3-33j_2026-10-30.md).

S3-33k adds a separate uncached opt-in profile for exact bare `MOV RAX,
sign-extended imm32` bytes `48 C7 C0 id` (seven bytes). Admission requires
decoder-owned W64 RAX `/0`, exact length and exact opcode/register bytes, and
checks that the decoder-owned immediate agrees with the little-endian source
bytes. Lowering consumes the decoded signed-extended constant through
Constant→WriteGpr and one commit, preserving all other GPRs, RFLAGS, and
pending flag writer state without arithmetic, data-memory, or helper
operations. Local parity covers preceding register ADD/CMP writers and JZ/JNZ
consumers across MOV.
Eager and demand-flags candidate compile/dispatch APIs remain separate. The
existing `48 B8 imm64` profile is unchanged. Local evidence is specific to the
exact new bytes; it does not establish a physical-x86 oracle for this encoding.
Mini and performance checks remain deferred until Stage 3 ends. See the
[S3-33k report](../../docs/reports/STAGE_3_S3-33k_2026-10-30.md).

S3-33l adds a separate uncached opt-in profile for exact `LEA RAX,[RBX+RCX]`
(`48 8D 04 0B`). It admits only W64/address64, RBX base, RCX index, scale 1,
and zero displacement. Lowering reads both source GPRs before writing RAX,
uses W64 Add with preserved flags, then commits once. This computes a register
sum and performs no data-memory access or helper call. The profile remains
separate from the no-displacement and disp8 LEA profiles. Local parity evidence
does not include a dedicated physical fixture for these exact bytes.

[S3-33l report](../../docs/reports/STAGE_3_S3-33l_2026-10-30.md).

S3-33m adds a separate uncached profile for exactly `48 8D 05 dd dd dd dd`
(`LEA RAX,[RIP+disp32]`). Admission binds all seven bytes to the decoded W64
RAX form, address64 `NextRip` base, no index, scale 1, and the decoder-owned
signed displacement. Lowering writes the wrapping sum of this instruction's
guest `next_rip` and displacement as a W64 constant, preserving flags and all
other GPRs. It performs no data access, canonicality check, or helper call.
This is local differential evidence for these exact bytes, not a physical
fixture claim.

[S3-33m report](../../docs/reports/STAGE_3_S3-33m_2026-10-30.md).

S3-33i adds a separate uncached opt-in profile for the exact bare bytes
`48 87 D8` (`XCHG RAX,RBX`). Admission checks both the decoded W64 register
pair and all three source bytes; equivalent ModRM/short-register forms, self
exchange, other registers or widths, memory operands, LOCK/other prefixes,
truncation, and branch targets inside the instruction are rejected. Lowering
reads both old values before writing either destination, then writes
`RAX <- old RBX` and `RBX <- old RAX` under one commit. The emitter admits only
this exact ordered dual-write IR shape and keeps intermediate PC-map snapshots
replay-only or inadmissible, so the guest cannot observe a half exchange. No
flags/helper effect is introduced; full RFLAGS/definedness and pending writer
descriptor/operands are preserved. Eager and demand-flags candidate APIs remain
separate and uncached. This proves register exchange only; it makes no memory
atomicity claim. Mini and performance checks remain deferred until Stage 3
ends. See [S3-33i report](../../docs/reports/STAGE_3_S3-33i_2026-10-30.md).

S3-33n adds a separate opt-in, uncached `NativeRegisterV1Admission` that
composes only restricted S0-09, the existing bare-NOP interpreter fallback,
and the exact native forms already committed by S3-33a–m. It uses one
complete-code admission/boundary witness and reuses the existing exact
per-instruction lowering semantics. Eager and demand-flags-candidate compile
and dispatch are explicit APIs; isolated exact profile admissions remain
unchanged. Implementation and verification status are tracked in the
[S3-33n report](../../docs/reports/STAGE_3_S3-33n_2026-10-30.md).

Первое подмножество нового JIT, сравниваемое состояние, неподдержанные формы и
источники платформы описаны в [контракте JIT](../../docs/specs/JIT_CONTRACT.md).
Проверки корректности и измерений по задачам этапа — в
[методике JIT](../../docs/testing/JIT.md). Добавлять формы инструкций можно
после подтверждения их interpreter semantics и требуемых внешних проверок.

The bounded S2-30ce `DB D1` FCMOVNBE operation has no v2 JIT lowering; runner
fixtures must continue to return `unsupported` for this instruction on the JIT
backend.

The local S2-19g interpreter-only `F3 66 AF`, `F3 AF`, and effective-REX.W
`F3 AF` REPE SCAS widths also have no v2 JIT lowering. Their literal fixtures
must remain unsupported on the JIT backend, with no interpreter fallback.

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

Кешированный S0-09 dispatcher в пределах одного вызова может повторно
использовать индекс только что выполненного блока и индекс предыдущего
блока. Это покрывает как повторный вход на тот же RIP, так и чередование
двух RIP. При попадании dispatcher использует позицию после обновления
порядка LRU и корректирует сохранённый индекс, если запись сдвинулась.
При установке нового блока сохранённый индекс сбрасывается до вытеснения.
Перед каждым входом проверяются снимок зависимостей, код, бюджет, события
и условия остановки. Первый вход и переход на другой RIP, для которого
индекс не сохранён, проходят обычное построение блока и поиск ключа.

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

S3-38c adds the opt-in cached demand-flags NativeRegisterV1 candidate through
`run_cached_native_register_v1_demand_flags_candidate_controlled`. It shares
the bounded cache lifecycle loop with eager execution but has its own compile
flavor, so strategy artifacts never cross-hit. It remains experimental;
`NativeRegisterV1Fixture` is unchanged, pending flag operands are execution
local, eager remains the default, and this slice makes no performance claim.
