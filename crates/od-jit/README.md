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
records: S3-04/S3-05 должны строить их из успешно декодированных инструкций.
IR пока не подключён к fragment compiler;
lowering, memory/helper effects и runtime dispatch остаются следующими задачами.

Первое подмножество нового JIT, сравниваемое состояние, неподдержанные формы и
источники платформы описаны в [контракте JIT](../../docs/specs/JIT_CONTRACT.md).
Проверки корректности и измерений по задачам этапа — в
[методике JIT](../../docs/testing/JIT.md). Добавлять формы инструкций можно
после подтверждения их interpreter semantics и требуемых внешних проверок.
