# Контракт первого JIT

**Статус:** начальный профиль этапа 3; зафиксирован в S3-00. Это контракт
первого проверяемого JIT-среза, а не описание полной x86-64 совместимости.

## 1. Источники и предел доказательств

Первый профиль берёт только S0-09 из [CPU contract](CPU_CONTRACT.md) и
принимает ровно формы, подтверждённые [отчётом этапа 1](../reports/STAGE_1_REPORT.md).
Проверка этапа 1 закрыта для своего старого register-only исполнителя; она не
подтверждает новый block pipeline этапа 3. Полный corpus этапа 1 подключается
к новому pipeline после dispatcher и переходов между блоками (S3-15).

Текущая реализация `od-jit` исполняет S0-09 в прямом fragment translator:
целочисленные операции и ветвления генерируются как ARM64, а host helper
вычисляет определённые x86 flags. Это исходная реализация, не доказательство
наличия общего typed IR, нового block builder, dispatcher или code cache.
`od-jit` сохраняет legacy `FragmentState` interface и не предоставляет v2
architectural-state entry; test runner отклоняет выбор JIT backend для v2
fixtures. Для новых форм нельзя считать JIT lowering завершённым только потому,
что декодер или интерпретатор этапа 2 уже принимает их.

Для кодирования A64 используется [Arm A-profile A64 Instruction Set
Architecture, DDI 0602, версия 2026-06](https://developer.arm.com/documentation/ddi0602/2026-06/Base-Instructions).
Для общих правил вызовов используется [AAPCS64, выпуск 2025Q4](https://github.com/ARM-software/abi-aa/blob/2025Q4/aapcs64/aapcs64.rst).
Платформенные отличия Apple задаёт документ [Writing ARM64 code for Apple
platforms](https://developer.apple.com/documentation/xcode/writing-arm64-code-for-apple-platforms)
(доступ 2026-10-26); в частности, x18 зарезервирован, а SP сохраняет
16-байтовое выравнивание. Для executable memory и синхронизации instruction
cache применяются указания Apple [Porting just-in-time compilers to Apple
silicon](https://developer.apple.com/documentation/apple-silicon/porting-just-in-time-compilers-to-apple-silicon)
(доступ 2026-10-26). До реализации host path зафиксировать реально
используемые macOS, Xcode/SDK и entitlements; документация не заменяет проверку
целевой конфигурации.

## 2. Матрица начального среза

Ограничение кодировок намеренно точное: REX byte `48`, только регистры
RAX/RCX/RDX/RBX, без high-byte registers и памяти. Для ModR/M `mod` обязан быть
`11b`, а оба индекса `reg` и `r/m` — от 0 до 3 включительно.

| Guest form | Начальный путь | Состояние и сравнение | Внешняя зависимость |
|---|---|---|---|
| `48 B8..BB imm64` — `MOV r64, imm64` | Native ARM64 lowering; flags не меняются | Четыре разрешённых GPR, RIP, сохранение определённых входных flags и exit reason | S0-09 CPU contract; точная форма уже принята decoder/interpreter этапа 2 |
| `48 03 /r`, `mod=11b` — `ADD r64, r/m64` | Native ARM64 arithmetic; собственный host helper материализует CF/PF/AF/ZF/SF/OF | Четыре разрешённых GPR и все определённые flags | Точные operand order/flags из CPU contract; A64 encoding из DDI 0602 |
| `48 2B /r`, `mod=11b` — `SUB r64, r/m64` | Native ARM64 arithmetic; helper материализует определённые x86 flags | Четыре разрешённых GPR и все определённые flags | То же; арифметическая семантика принадлежит `od-isa` |
| `48 3B /r`, `mod=11b` — `CMP r64, r/m64` | Native ARM64 subtraction без записи результата; helper материализует определённые x86 flags | GPR не меняются; сравниваются определённые flags и RIP | То же |
| `74 cb` — `JZ rel8` | Native ARM64 conditional control flow по материализованному ZF | RIP/exit reason и заданное состояние GPR/flags | Цель должна быть границей инструкции внутри входного буфера либо его концом |
| `75 cb` — `JNZ rel8` | Native ARM64 conditional control flow по материализованному ZF | То же | То же |

Формы таблицы образуют единственное начальное множество native JIT lowering.
Для `MOV` принимаются только четыре назначения из таблицы. Для арифметики оба
регистра operand должны иметь индекс 0–3. Другие REX bytes, ширины, opcode
варианты, addressing modes, префиксы и состояния в этот профиль не входят.
Интерпретатор может поддерживать больше форм; это не расширяет таблицу.

Для каждого исполнения сравнивать RAX/RCX/RDX/RBX, RIP, причину выхода и все
определённые на наблюдаемом exit flags из CF/PF/AF/ZF/SF/OF. Это включает
флаги, определённые предыдущей инструкцией и сохранённые последующими `MOV` или
ветвлением. Не сравнивать флаги, оставленные undefined последним flag-writing
оператором, и прочие неоговорённые биты RFLAGS. Память и архитектурные faults в
данном профиле отсутствуют.

## 3. Формы за пределами среза

| Класс | Путь на текущем шаге | Условие дальнейшего включения |
|---|---|---|
| Любые memory operands, включая ограниченный v2 memory profile | Test runner отклоняет выбор JIT для v2 state; `od-jit` не имеет v2 entry. Не обходить `GuestMemory` и не выполнять host pointer access | Подтверждённая семантика конкретной формы в S2, checked bridge S3-19, lowering S3-20 и необходимые native fault evidence |
| Остальные формы, уже принятые interpreter этапа 2 | Runner не допускает их к текущему legacy JIT. Interpreter fallback запланирован в S3-16 и не реализован текущим contract | Завершённая interpreter semantics конкретной формы и fallback с точным progress/state |
| x87, SIMD, privilege/system, interrupts, unsupported/invalid decode | Runner не допускает эти формы к текущему legacy JIT; interpreter путь разрешён только если соответствующая форма уже реализована и запрошен явный fallback после S3-16 | Отдельные S2 contracts, CPU state mapping и сопоставимые внешние evidence для каждой формы |

Не вводить один общий `supported-v2` переключатель. Каждая новая форма требует
отдельного решения о lowering, fallback, сравнении состояния и prerequisites.
Ни native execution, ни память, ни исключения этапа 2 не наследуются этапом 3
автоматически.

## 4. Начальные runtime bounds

- Один гостевой vCPU; компиляция блоков последовательная.
- Вход компилятора — неизменяемый bounded code buffer. Компилятор не выполняет
  гостевой код и не должен читать за границу буфера.
- Начальный предел блока: 32 guest instructions, 256 guest bytes и 4096 bytes
  emitted ARM64 code; срабатывает первый достигнутый предел.
- Блок заканчивается на управляющей инструкции, неподдержанной форме, конце
  доступного буфера или достигнутом пределе. Fault/Unsupported за уже собранным
  корректным префиксом должен наблюдаться только при достижении соответствующего
  RIP исполнителем.
- Это явные начальные ограничения, не результат измерения оптимальности.
  Переходы между блоками и cache принадлежат следующим задачам, а cache появится
  не раньше S3-18.

S3-04 реализует только decoder-backed block builder поверх immutable borrowed
`CodeView`. `base_rip + len` и entry offset проверяются без переполнения; entry
может быть любой позицией в view или ровно `code_end` и считается новой decode
boundary. Настраиваемые limits должны быть положительными и не превышать 32
instructions/256 bytes. Блок сохраняет точные `[guest_rip, next_rip)` ranges для
успешно декодированных instructions; range остаётся одной записью и при
пересечении страницы, без предположений о guest page mapping.

Decoder получает read-only `GuestMemory`, читающий только внутри view. Fetch за
концом доступного буфера создаёт `DecodeBoundary` stop metadata; это deferred
decode/fetch result, не guest `#PF`. Ошибка на entry возвращается как `NoBlock`
с тем же deferred record, чтобы dispatcher/interpreter наблюдал её в нужный
момент. Buffer exhaustion и block limits сами по себе не являются guest faults.
Unsupported decoded form не включается в block; builder останавливается перед
ней. S0-09 admission использует decoder opcode/prefix/operand metadata и точную
длину известных форм, поэтому лишние и повторные prefixes не проходят как
поддержанная форма.

Условная branch завершает построение немедленно, до чтения недостижимого хвоста
или target. Target в уже декодированном range должен совпадать с decoded
instruction start или `code_end`; если он попадает внутрь instruction, builder
останавливается перед Jcc и сохраняет ранее собранный prefix. Target вычисляется
64-битным wrapping сложением `next_rip` и sign-extended displacement, как в
interpreter. Если target совпадает с `code_end`, он классифицируется как
`CodeEnd`; если совпадает с `next_rip` текущей branch, он классифицируется как
`NextRipBoundary`, даже когда за ним есть неразобранные bytes. Это доказывает
только sequential boundary по длине декодированной branch; будущие bytes после
него не декодируются и не объявляются валидной инструкцией. Targets на уже
декодированные starts сохраняют `KnownInstructionStart`. Другие forward/unparsed
и внешние targets имеют статус `Unresolved`. Такой target пока нельзя передать
в `ir::verify` как проверенную ветвь: verifier принимает только guest-record
starts, последний `next_rip` или точный `code_end` из block metadata. S3-05
направляет unresolved branch на interpreter до Jcc, не включая и не retiring
её; дальнейшее разрешение target для dispatch остаётся S3-15.

`BasicBlock.guest_ranges` хранит точные ranges декодированного блока. Они не
содержат идентичность или версию executable code и сами не являются cache
dependencies. Code identity/version tracking принадлежит S3-17, а проверка
изменений и cache invalidation — S3-23.

### S3-05: decoded effects to typed IR

`lowering::lower` переиспользует уже decoded `long_decode::Instruction` и
builder admission для точных форм MOV immediate qword, ADD/SUB/CMP qword
register-register и JZ/JNZ rel8. IR хранит effects и операнды decoder output;
он не повторяет x86 decode или CPU semantics. Arithmetic lowerer читает оба
источника до destination write, CMP не пишет GPR, MOV не выдаёт flags write,
а все значения имеют W64. Каждый включённый instruction record имеет ровно
один commit.

Для разрешённой Jcc commit marker и следующий terminal `Branch` — единая
граница retirement и выбора RIP. Между ними запрещены helper calls,
safepoints и любые наблюдаемые exits. Target `CodeEnd` принимается verifier
только при точном совпадении с `Program.code_end`, скопированным из
`BasicBlock`/валидированного `CodeView`; все guest records обязаны лежать до
этого exclusive end. Это не произвольный список разрешённых адресов.

Unresolved forward/external Jcc не включается в IR records и не коммитится.
Непустой prefix завершается `NeedInterpreter` на RIP этой branch; если prefix
пуст, lowering возвращает `NoNativePrefix`. Mid-instruction target завершается
`Unsupported` до Jcc. Unsupported form также даёт явный Unsupported exit на
problem RIP. Buffer/limit completion использует последний `next_rip`. Любой
`DecodeBoundary` остаётся отдельным deferred metadata; lowering не объявляет
его guest fault или успешным code end. Публикация и runtime dispatch ещё не
подключены.

## 5. Runtime boundary (S3-02)

Legacy fragment entry продолжает использовать `FragmentState` и собственную
сигнатуру `(*mut FragmentState, u64, *mut u64, *const u8) -> u32`. Этот путь
не является новым runtime ABI и не преобразуется неявно в architectural state.
Переход к нему из `LongModeState` возможен только через существующий
fallible `FragmentState::try_from(&LongModeState)`; потеря дополнительных
регистров, flags и режима запрещена.

Будущий block entry внутри одной linked Rust build принимает non-null
`*mut RuntimeContextV1` и возвращает raw `u32` exit tag. `RuntimeContextV1` —
versioned `repr(C)` record: `state` — opaque non-null `*mut LongModeState`,
`initial_budget` — входной неизменяемый лимит этого вызова, `remaining_budget`
и `retired_instructions` — выходы, `stop_requested` — обязательный non-null
указатель на живой выровненный atomic byte. `stop_rip_enabled` задаётся raw
значением 0 (disabled) или 1 (enabled); иные значения и ненулевой `reserved`
являются host contract failure. При disabled `stop_rip` игнорируется. Ни один
из этих полей не имеет Rust `Option` encoding.

При любом определённом runtime exit `remaining_budget` равен начальному
лимиту минус число retired guest instructions, а `retired_instructions`
содержит именно число инструкций, уже committed этим вызовом. Их сумма
сохраняет бюджет при переходе dispatcher между blocks; `NeedInterpreter`
возвращает RIP неподдержанной инструкции после ранее committed префикса и
только committed state, поэтому interpreter может продолжить с остатком
бюджета без двойного retirement. Инструкция
retired только после полного commit её effects и next RIP. `initial_budget` не
меняется. `LongModeState` не является C record: native code получает `gpr`,
`rip` и `rflags` по `offset_of!` значениям, вычисленным в том же `od-jit`
build, и не читает другие поля. Эти offsets не являются стабильным on-disk,
IPC или cross-version ABI. Внешние raw ABI используют только примитивы,
указатели и явно `repr(C)` records; Rust enum, `Result`, `Option` и trait
objects через такую границу не передаются.

Host wrapper формирует `context` из валидных Rust references и удерживает их
заимствования вместе с `LongModeState` и `AtomicU8` на весь call; pointers
должны быть non-null, aligned и доступны по назначению. Raw ABI не позволяет
достоверно проверить lifetime или доступность произвольного address. Их
нарушение — ошибка вызывающей стороны с неопределённым поведением, не
диагностируемый runtime exit и не guest fault. Перед generated call host
wrapper проверяет scalar fields `reserved == 0` и `stop_rip_enabled ∈ {0, 1}`;
невалидные значения являются host-side contract failure. Stop byte читается
атомарно с acquire порядком; host публикует запрос с release порядком. Ноль
означает отсутствие запроса. При отсутствии cooperative-stop источника host
передаёт адрес собственного нулевого atomic byte, не null.

Block exit tags (`repr(u32)`; остальные значения — host contract failure):

| Tag | Значение | Действие dispatcher |
|---|---:|---|
| `BlockComplete` | 0 | Продолжить dispatch с committed state/RIP |
| `BudgetExhausted` | 1 | Вернуть budget exit |
| `NeedInterpreter` | 2 | Передать interpreter на том же committed RIP/state |
| `StopRequested` | 3 | Вернуть cooperative stop |
| `StopAddress` | 4 | Остановиться на `stop_rip`, не исполняя инструкцию по этому RIP |
| `Unsupported` | 5 | Не исполнять форму; interpreter допускается только при явном fallback policy |

`GuestFault` (tag 6) и `EventPending` (tag 7) зарезервированы и недопустимы в
runtime contract v1: начальный S0-09 профиль не содержит faulting forms или
event input/model. Возврат этих или любого неизвестного tag — host contract
failure. В будущей версии fault exit потребует отдельного raw fault record с
явными полями, до его появления гостевой fault не заявляется.

Compile result остаётся Rust-side: успешная компиляция даёт непубликованный
код/metadata; неподдержанная форма классифицируется как `Unsupported`, а
ошибка декодирования, размера или внутреннего контракта — как compilation
failure. Только runtime `NeedInterpreter` является запросом продолжить через
interpreter, причём с неизменённого RIP и корректно committed state.
`Unsupported` не превращается в fallback сам по себе. Guest fault будет guest
результатом только после добавления fault record в следующую версию; сейчас tag
зарезервирован. Неизвестный raw tag, invalid pointer, mapping/publication
failure, helper ABI violation и panic/unwind — ошибка host/runtime, не guest
exit.
`StopAddress`, `StopRequested`, `BudgetExhausted`, `Unsupported` и
`NeedInterpreter` не retire текущую инструкцию; `remaining_budget` и
`retired_instructions` всё же отражают ранее committed инструкции текущего
вызова. `BudgetExhausted` возвращается только с нулевым remaining budget.

На каждой границе guest instruction entry проверяет включённый `stop_rip`,
затем cooperative STOP, затем оставшийся budget, и только потом fetch/decode.
Таким образом, STOP и budget не скрывают stop-at-RIP, а fetch не выполняется
после любого из этих выходов. Commit RIP/state для уже завершённой инструкции
сохраняется.

Scalar helper ABI alias: `(operation: u32, lhs: u64, rhs: u64, old_rflags: u64,
output_rflags: *mut u64) -> u32`. Operations: Add=1, Subtract=2, Compare=3;
reserved status tags: Success=0, Unsupported=1, InvalidArgument=2. Это только
calling contract; реализация helper и его arithmetic semantics появятся вместе
с lowering, который его вызывает. По контракту helper не меняет state, RIP
или budget, пишет только `output_rflags` при Success и не unwinds через ABI.

Generated ARM64 code следует AAPCS64: после helper call считать volatile
`X0–X17`, `X30`, `NZCV` и caller-clobbered SIMD registers уничтоженными; не
оставлять в них live значения. Сохранять `X19–X29`, держать `SP` выровненным на
16 байт и не использовать зарезервированный `X18`. Callee обязан сохранить
`X19–X29`; это правило не разрешает emitter временно держать live значения в
callee-saved регистрах без корректного prologue/epilogue.

Executable handle владеет только `od-host`: он публикует код, синхронизирует
instruction cache и удерживает mapping живым весь generated call. `od-jit`
возвращает code bytes/metadata и не зависит от `od-host`; host failure при
allocate/publish/invoke остаётся host-side. Старые C ABI и fragment mapping
остаются совместимыми.

Для macOS используется существующая граница публикации кода в `od-host`.
Apple описывает `MAP_JIT`, hardened-runtime entitlement и необходимость
`sys_icache_invalidate` после записи кода. Не смешивать
`pthread_jit_write_protect_np` с JIT write callback allowlist: Apple указывает,
что allowlist entitlement запрещает вызов первого API. Реальное поведение
проверить на выбранных Apple Silicon host и signing configuration; Stage 0
оставляет поставку/notarization открытой.

## 6. Host и этапная приёмка

Основной ARM64 host для первого JIT-среза — Apple Silicon M3 Max/macOS,
дополнительный ARM64 host — M1 Pro. Единственный физический x86 oracle,
зафиксированный в [STAND](../testing/STAND.md) и [отчёте этапа
1](../reports/STAGE_1_REPORT.md), — Mac mini с Ubuntu. Это указывает, где
проводить нужные проверки, но не утверждает, что хосты повторно проверены для
текущей ревизии.

S3-14 требует тройного сравнения нового JIT, interpreter и физического x86 для
допущенных одно-блочных программ. Полный Stage 1 corpus с несколькими блоками
откладывается до S3-15, когда появится dispatcher. Проверка только interpreter
vs JIT, старый S0 JIT, сборка или ARM64 smoke не заменяют этот критерий.

Измерения скорости относятся к S3-35 и S3-36 и проводятся раздельно для
холодной компиляции/исполнения и прогретого cache. До этих данных contract не
заявляет ускорения.
