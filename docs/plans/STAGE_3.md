# Этап 3. Первый JIT x86 → ARM64 — пошаговый план реализации

**Дата:** 27 октября 2026 года. **Статус:** этап начат; S3-00, S3-02, S3-03, S3-04, S3-05, S3-06, S3-07, S3-08, S3-09, S3-10, S3-11, S3-12 и S3-13 завершены; S3-01 в работе; S3-19b/S3-20a/S3-20b/S3-20c/S3-20d/S3-20e/S3-20f/S3-20g/S3-20h/S3-20i/S3-20j/S3-20k/S3-20l/S3-20m/S3-20n/S3-20o/S3-20p/S3-20q/S3-20r/S3-20s/S3-20t/S3-20u/S3-20v/S3-20w/S3-20x/S3-20y/S3-20z/S3-20aa/S3-20ab/S3-20ac/S3-20ad/S3-20ae/S3-20af/S3-20ag/S3-20ah/S3-20ai/S3-20aj/S3-20ak/S3-20al/S3-20am/S3-20an/S3-20ao/S3-20ap/S3-20aq/S3-20ar/S3-20as/S3-20at/S3-20au/S3-20av/S3-20aw/S3-20ax/S3-20ay/S3-20az/S3-20ba/S3-20bb/S3-20bc/S3-20bd/S3-20be/S3-20bf/S3-20bg/S3-20bh/S3-20bi/S3-20bj/S3-20bk/S3-20bl/S3-20bm/S3-20bn/S3-20bo/S3-20bp/S3-20bq/S3-20br/S3-20bs/S3-20bt/S3-20bu/S3-20bv/S3-26a/S3-26b/S3-26c/S3-26d/S3-27a/S3-27b/S3-22a/S3-23a/S3-23c/S3-23d/S3-23e/S3-23f/S3-23g/S3-24a/S3-25a/S3-25b реализованы как bounded partial slices, общие memory lowering и S3-19/S3-20/S3-22/S3-23/S3-24/S3-25/S3-26/S3-27 остаются открытыми. S3-21 — финальный физический gate после завершения локальной работы Stage 3.

План предназначен для модели, которой нужны небольшие последовательные задания, конкретные файлы, зависимости и проверяемый результат. Выполнять одну задачу `S3-NN` за раз. Не реализовывать весь JIT одним изменением. Большие группы внутри задачи проходить по указанным подшагам.

Нормативный объём — [этап 3 дорожной карты](../ROADMAP.md#этап-3-первый-jit-x86--arm64). Границы компонентов определяют [карта проекта](../PROJECT_LAYOUT.md) и [архитектура](../ARCHITECTURE.md). Семантика x86 принадлежит [CPU contract](../specs/CPU_CONTRACT.md) и [интерпретатору этапа 2](STAGE_2.md). Профиль первого среза зафиксирован в [контракте JIT](../specs/JIT_CONTRACT.md), методика — в [проверках JIT](../testing/JIT.md). Этот план описывает перевод и исполнение уже определённой семантики, а не второй CPU-профиль.

## 1. Проверяемый результат этапа

Собственный JIT переводит поддержанное подмножество x86 в ARM64, повторно использует опубликованные блоки, передаёт управление между ними и собственному интерпретатору, сохраняет точное состояние на выходах и не исполняет устаревший код после его изменения.

```text
guest RIP + CPU context + memory/code versions
  → поиск пригодного блока
  → при промахе: fetch/decode → свой IR → ARM64 → безопасная публикация
  → исполнение блока
  → следующий блок / interpreter step / точный выход
  → материализация состояния и проверка по interpreter/native x86
```

Порядок проверяемых срезов:

1. **A:** один блок для уже проверенного register-only профиля, свой IR, ARM64, eager flags, точный snapshot.
2. **B:** dispatcher, несколько блоков, ограниченный cache, корректный interpreter fallback и STOP/budget.
3. **C:** поддержанные операции памяти, faults, инвалидация изменённого кода и безопасное освобождение блоков.
4. **D:** вычисление flags по потребности, улучшения register allocation и другие подтверждённые замерами изменения, интеграция и итоговая приёмка.

Для каждого среза записывать exact instruction forms, режим, состояние, memory capabilities и внешние зависимости. Не называть исполнение через interpreter нативным JIT lowering. Успешная сборка или совпадение только двух собственных движков не закрывает физическую проверку.

## 2. Исходное состояние и зависимости

Состояние зависимостей перепроверено 26 октября 2026 года:

- [Этап 1 принят](../reports/STAGE_1_REPORT.md) для ограниченного S0-09 register-only профиля. Есть физический oracle, fixtures, генератор, comparator, reducer и повторяемый harness.
- [Этап 2 выполняется](../reports/STAGE_2_REPORT.md): отчёт фиксирует принятые срезы на момент последней записи, но этап 2 в целом не принят и развивается параллельно. Проверки S3 привязывать к точному fingerprint; изменения этапа 2 не расширяют JIT-профиль автоматически.
- `crates/od-jit/src/lib.rs` содержит ограниченный прямой fragment translator и ARM64 emitter. Он не доказывает наличие общего IR, dispatcher, block cache или self-modifying-code support.
- `crates/od-host/src/lib.rs` содержит `ExecutableMemory`, `CompiledFragment`, macOS MAP_JIT path, переключение write protection и синхронизацию instruction cache. Эти механизмы нужно расширять с сохранением проверенных ограничений, а не писать второй независимый host allocator.
- Старый fragment state/C ABI и новый архитектурный state этапа 2 различаются. JIT layout использует `offset_of!`; перед новой реализацией проверить фактические типы, adapters, generated accesses и helper signatures.
- Имеются fixture v1 и новый v2 path. Текущий v2 JIT возвращает явный Unsupported; этот отказ не означает готовую JIT-поддержку расширенного state или памяти.
- В [протоколе измерений](../testing/BENCHMARKS.md) и [baseline S0-13](../reports/STAGE_0_CPU_BASELINE_2026-10-22.md) описан узкий fragment benchmark. Старые числа не являются baseline будущего block JIT на новых программах.

**Основной host:** M3 Max, macOS/ARM64. **Дополнительный ARM64 host:** M1 Pro в пределах доступной конфигурации. **Единственный физический x86 oracle:** Mac mini с Ubuntu, SSH `senya@192.168.1.14`; в этапе 1 использован checkout `/home/senya/projects/open_desktop-stage1`. Актуальные CPU/OS/toolchain/page sizes брать из [STAND.md](../testing/STAND.md) и перепроверять перед прогонами. Старый remote checkout не считать синхронизированным автоматически.

Не требуется ждать завершения всей ISA этапа 2 для среза A. Но новую форму нельзя принять в JIT до подтверждения её interpreter semantics и независимой проверки. Для C обязательны S2-09 и соответствующие memory/fault cases; для REP/system/FP fallback нужны реально завершённые соответствующие задачи этапа 2. Если зависимости нет, продолжать независимую задачу, а зависимую оставить открытой.

Известные ограничения находятся в [KNOWN_ISSUES](../KNOWN_ISSUES.md): ISSUE-004 — поставка/подпись, ISSUE-010 — неполное извлечение C header графом, ISSUE-011 — проверка источника AMD, ISSUE-012 — неподтверждённая граница operand range через верх u64. Не переопределять эти условия внутри JIT и не записывать их закрытыми по результату этого плана.

## 3. Правила для исполнителя

1. Перед задачей прочитать корневой `AGENTS.md`, этот план, карту проекта, применимые `crates/`, `tools/`, `tests/`, `apps/` инструкции и вложенные `AGENTS.md` целевых файлов.
2. Проверить `git status --short`. В дереве могут выполняться задачи этапа 2; не перезаписывать их и не откатывать ради чистого baseline. Для доказательств сохранять revision и fingerprint фактически проверенных исходников.
3. При наличии графа выполнить `graphify query "<вопрос текущей задачи>"` до поиска/чтения исходников. Ограничение графа по ABI учитывать: найденный реальный C header и его consumers нужно проверить непосредственно.
4. Выбрать ближайшую незавершённую задачу с готовыми зависимостями. Сначала сформулировать вход, ожидаемое состояние/выход и негативный случай; затем реализовать минимальный путь.
5. Использовать собственные IR, emitter, allocator и runtime. Не подключать LLVM, Cranelift, готовый DBT/JIT или чужие x86 decoder/semantics. Общая инфраструктура разрешена по [политике зависимостей](../policies/DEPENDENCIES.md).
6. Нормативные источники фиксировать с версиями: x86 semantics — Intel/AMD и CPU contract; ARM64 encoding/ABI — официальные Arm и Apple документы; executable memory — выбранный macOS API contract. Не выводить ABI из случайного успешного запуска.
7. Производительность измерять до окончательного выбора критичного решения. Простой корректный вариант допустим как baseline; его скорость не считается доказанной. Для нового горячего участка сравнить минимальные варианты на одинаковой нагрузке.
8. Не добавлять background compiler, SMP cache protocol, глобальный SSA, persistent code cache или универсальную plugin-систему без конкретного требования. Первый runtime — один vCPU, последовательная компиляция и явные безопасные точки.
9. Guest fault, unsupported form, compiler failure, allocation failure, host crash, budget и STOP различать. Не превращать внутренний сбой в guest exception или успешный fallback.
10. После задачи выполнить её проверки и общие проверки раздела 11. После изменения кода — `graphify update .`. Обновить документацию и реальные найденные проблемы.
11. `[x]` означает работающую реализацию с доказательствами. Не закрывать JIT-группу только тем, что её целиком исполнил interpreter. Не закрывать hardware suite при skip/недоступном SSH.
12. В отчёте указать ID, файлы, поддержанные формы, проверки, измерения при наличии, ограничения и следующий ID. Этап закрывается только S3-41.

## 4. Размещение и обязательные контракты

### 4.1. Где менять код

| Ответственность | Место |
|---|---|
| IR, block builder, lowering, ARM64 encoding, register allocation | `crates/od-jit/src/` |
| Dispatcher, block lookup, metadata, зависимости от code pages | `crates/od-jit/src/` |
| macOS executable mappings, write protection, I-cache sync, host resources | `crates/od-host/src/` |
| CPU state, decoder, step interpreter, архитектурные exceptions | существующий `crates/od-isa/src/` |
| Guest translation, RAM/MMIO, mapping changes и реальные device writers | `crates/od-machine/`, по мере этапа 4 |
| Fixture memory, generation/comparison/reduction | существующий `crates/od-testkit/src/` |
| CLI, сбор сравнения/benchmarks | существующий `tools/runner/`, `tools/bench/` |
| CPU/JIT scenarios | `tests/cpu/`; host/process scenarios — `tests/system/` |
| Собственные малые входы | предлагаемый `tests/fixtures/x86/stage3/` |
| Контракт JIT | будущий `docs/specs/JIT_CONTRACT.md` |
| Методика | будущий `docs/testing/JIT.md`, существующий BENCHMARKS |
| Факты и замеры | будущий `docs/reports/STAGE_3_REPORT.md`, `docs/reports/data/stage3/` |
| Временные данные | `work/tmp/stage3/<run-id>/` |

Новые имена — предлагаемые пути, не существующие реализации. Создавать модули `ir.rs`, `block.rs`, `arm64/`, `regalloc.rs`, `runtime.rs`, `cache.rs`, `state_map.rs` внутри `od-jit` только при появлении соответствующего кода. Не дробить проект на новые crates заранее.

`od-jit` не импортирует `od-host`. Он получает нужный узкий интерфейс публикации/владения executable code; реализацию macOS предоставляет верхний слой. `od-machine` предоставляет guest memory/context, а не переносит MMU в JIT. Runtime не зависит от `od-testkit`. Существующий публичный C ABI не расширять молча.

### 4.2. Выполнение и состояние

- Один canonical архитектурный state на границе исполнения; layout generated code и helpers задан явно и проверен. Host-register cache значений не становится новым архитектурным состоянием.
- На выходе доступны правильные GPR, flags, RIP, memory effects, exit reason и согласованный instruction count. Host/instrumentation instructions в guest count не входят.
- Любой вызов helper имеет объявленные reads/writes/clobbers, fault behavior и точку commit. До callback материализуются необходимые значения; после callback инвалидируются затронутые host-register bindings.
- Соблюдать ARM64/macOS calling convention, stack alignment, callee-saved и platform-reserved registers. Никакой Rust unwind через generated code. Host panic остаётся внутренней фатальной ошибкой, а не guest fault.
- Не держать конфликтующие Rust references на CPU/RAM одновременно с доступом generated code или callback. Владение и lifetime проверяются на границе, а не объясняются одним `unsafe` блоком.
- Fallback вызывает существующий interpreter на точном текущем RIP и с общим budget/event context. Он должен продвинуть выполнение либо вернуть явный выход; бесконечный цикл «compile unsupported → retry того же блока» запрещён.

### 4.3. Блок и его зависимости

Блок содержит entry context, последовательность guest instructions, exit edges, guest-PC/host-PC metadata, необходимые code dependencies и executable handle. Границы: control transfer, неподдерживаемая форма, необходимый serializing/context-changing effect, лимит размера и безопасная точка. Максимальные guest instructions/bytes и host code size задаются явно.

Compiler не исполняет гостя. Ошибка fetch следующей инструкции при построении блока не должна возникать раньше, чем interpreter дошёл бы до неё: закончить уже построенный валидный префикс, а fault получить при достижении faulting RIP. То же относится к unsupported/invalid байтам после ещё не исполненных инструкций. Не читать всю будущую страницу ради удобства decoder.

Cache key учитывает entry RIP и действительно значимый decode/execution context: профиль/режим, relevant sizes/privilege, identity/version адресного пространства и прочие специализации. Конкретный набор записать в JIT_CONTRACT. Не включать весь CpuState без причины и не ограничиваться одним RIP. Constant assumptions обязаны иметь guard/version dependency.

Code dependencies покрывают все затронутые гостевые страницы/регионы и mapping identities. Host page size и guest page size могут отличаться. Guest virtual aliases одной памяти должны инвалидировать один и тот же зависимый перевод. Для flat fixtures разрешён простой region generation; это ещё не production physical-page tracking.

### 4.4. Публикация и освобождение

Сначала сформировать и проверить bytes/relocations/metadata, затем записать код через выбранный host API, выполнить требуемую синхронизацию и только после этого сделать entry доступным runtime. Для публикации проверить, что версии исходного кода ещё актуальны.

Первый вариант публикует immutable blocks. Изменённый перевод публиковать как новый block, старый выводить из lookup и освобождать после отсутствия активного исполнения/ссылок. Не patch-ить исполняемый блок на месте без отдельного доказанного протокола. Eviction и invalidation не имеют права уничтожить код под текущим PC.

Отдельно проверить allocation failure, публикацию после invalidation, повторный destroy, освобождение после fault и исчерпание cache capacity. Для одного vCPU достаточно простого явного active-block/retired-block lifecycle; SMP reclamation относится к последующему этапу.

### 4.5. Изменение гостевого кода

Учитывать записи interpreter, JIT, debugger/loader и будущих device/DMA writers через общую границу уведомления memory owner. Страница не обязана быть read-only, чтобы содержать уже переведённый код.

Инвалидации cache entry недостаточно, если запись изменила ещё не исполненные инструкции текущего блока. Для первой корректной реализации после записи в зависимый код немедленно выйти на безопасной границе и продолжить с правильного guest RIP через повторную проверку/перевод. Не продолжать stale host instructions. Оптимизировать этот случай только после измерений и сохранения семантики.

Host PC map нужен для диагностики и восстановления на предусмотренных выходах. Он не превращает произвольный SIGSEGV/SIGBUS generated code в корректное гостевое исключение. Первый memory path использует явные checked helpers; неожиданный host fault диагностируется как ошибка VMHost.

## 5. Порядок и статусы задач

Внешние зависимости S2 относятся к [плану этапа 2](STAGE_2.md). Готовность перепроверяется перед исполнением задачи. Условные оптимизации можно оставить без внедрения с результатами сравнения; обязательные функции roadmap нельзя молча объявлять необязательными.

| Статус | ID | Задача | Зависимости |
|---|---|---|---|
| [x] | S3-00 | Зафиксировать поддержанное подмножество и источники | Принятый профиль этапа 1; фактический статус S2 |
| [ ] | S3-01 | Baseline и протокол сравнения вариантов | S3-00 |
| [x] | S3-02 | Runtime/state/helper contract | S3-00 |
| [x] | S3-03 | Типизированный IR и verifier | S3-02 |
| [x] | S3-04 | Построение bounded basic block | S3-03 |
| [x] | S3-05 | Перевод decoder output в IR | S3-04 |
| [x] | S3-06 | Проверяемый ARM64 encoder | S3-02 |
| [x] | S3-07 | Executable publication и ownership | S3-02, S3-06 |
| [x] | S3-08 | Generated entry и helper ABI | S3-07 |
| [x] | S3-09 | Простой allocator baseline | S3-03, S3-08 |
| [x] | S3-10 | Lowering первого integer subset | S3-05, S3-06, S3-09 |
| [x] | S3-11 | Корректные eager flags | S3-10 |
| [x] | S3-12 | State points и PC maps | S3-11 |
| [x] | S3-13 | Первый сквозной запуск одного блока | S3-07, S3-12 |
| [ ] | S3-14 | Тройное сравнение register-only | S3-13 |
| [ ] | S3-15 | Dispatcher, budget и переходы | S3-14 |
| [ ] | S3-16 | Interpreter fallback с прогрессом | S3-15; нужный S2 step |
| [ ] | S3-17 | Cache key и code dependencies | S3-04, S3-15 |
| [ ] | S3-18 | Ограниченный block cache | S3-07, S3-17 |
| [partial] | S3-19 | Checked memory/helper bridge | S3-08, S3-16; S2-04/08 |
| [partial] | S3-19a | Invocation-local checked helper ABI и native probe | Узкий standalone slice внутри S3-19; production block exits остаются открыты |
| [partial] | S3-19b | Production V2 block/helper path для одного MOV r64 load | Точный `48 8B /r`, base/dst RAX..RBX; без cache/general IR |
| [partial] | S3-20 | Memory lowering и partial registers; проверенные MOV qword load/store slices и два точных bounded two-site pair | S3-19; interpreter соответствующих форм |
| [partial] | S3-20b | Production V2 MOV qword store с private-RAM unsafe entry | Точный `48 89 /r`, source/base RAX..RBX; остальные stores и executable aliases запрещены |
| [partial] | S3-20c | Один-byte SIB extension для V2 MOV qword load/store | ModRM `mod=00,r/m=100`; base/index/data RAX..RBX, scale 1/2/4/8, no displacement |
| [partial] | S3-20d | Signed disp8/disp32 для direct и SIB V2 MOV load/store | ModRM `mod=00/01/10`; source lengths 3/4/7 direct и 4/5/8 SIB; прочие address forms отвергаются |
| [partial] | S3-20e | RIP-relative disp32 для V2 MOV load/store | Только `mod=00,r/m=101`, ровно 7 bytes, qword и data register RAX..RBX |
| [partial] | S3-20f | Address-size override `67 48 8B/89` для V2 MOV | Только ModRM `mod=00`, qword data + dword address, RAX..RBX base/data, ровно 4 bytes |
| [partial] | S3-20g | Signed disp8/disp32 для V2 MOV с address-size override | `67 48 8B/89 /r`, ModRM `mod=01/10`, qword data + dword address, base/data RAX..RBX, exact lengths 5/8; без SIB и других форм |
| [partial] | S3-20h / S3-26a | Checked qword load через общий BasicBlock → typed IR → allocator → runtime emitter | Только `48 8B /r`, ModRM `mod=00`, data/base RAX..RBX, без SIB/displacement; V1/direct V2 остаются отдельными |
| [partial] | S3-20i / S3-26b | Checked qword store через общий BasicBlock → typed IR → allocator → runtime emitter | Только `48 89 /r`, ModRM `mod=00`, source/base RAX..RBX; отдельный typed entry, no replay после helper write |
| [partial] | S3-20j / S3-26c / S3-20k / S3-26d | Один generated entry для ровно двух exact pairs: `48 89 03; 48 8B 0B` и `48 8B 03; 48 89 02` | Только private synchronous ordinary RAM; two guest commits; STOP/fault/budget continuation bounded; singleton APIs сохранены |
| [partial] | S3-20l | RIP-relative qword MOV через shared IR и bounded memory dispatcher | Только singleton `48 8B/89 /r`, ModRM `mod=00,r/m=101`, disp32, data RAX..RBX; exact 7-byte source; no wider block/prefix/SIB |
| [partial] | S3-20m | Direct-base signed disp8/disp32 MOV через shared IR и bounded dispatcher | Только singleton `48 8B/89 /r`, ModRM `mod=01/10`, base/data RAX..RBX, exact length 4/7; no SIB/prefix/wider block |
| [partial] | S3-20n | No-displacement SIB MOV через shared IR и bounded dispatcher | Только `48 8B/89 /r sib`, ModRM `mod=00,r/m=100`, data/base/index RAX..RBX, scale 1/2/4/8, ровно 4 bytes |
| [partial] | S3-20o | Address32 direct-base qword MOV через shared IR и bounded dispatcher | Только `67 48 8B/89 /r`, ModRM `mod=00`, Dword EA/Qword data, base/data RAX..RBX, ровно 4 bytes |
| [partial] | S3-20p | Address32 signed-displacement qword MOV через shared IR и bounded dispatcher | Только `67 48 8B/89 /r disp8|disp32`, ModRM `mod=01/10`, Dword EA/Qword data, base/data RAX..RBX, ровно 5/8 bytes |
| [partial] | S3-20q | 64-bit SIB signed-displacement qword MOV через shared IR и bounded dispatcher | Только `48 8B/89 /r sib disp8|disp32`, ModRM `mod=01/10,r/m=100`, base/index/data RAX..RBX, scale1/2/4/8, ровно 5/8 bytes |
| [partial] | S3-20r | Address32 no-displacement SIB qword MOV через shared IR и bounded dispatcher | Только `67 48 8B/89 /r sib`, ModRM `mod=00,r/m=100`, data/base/index RAX..RBX, scale1/2/4/8, ровно 5 bytes |
| [partial] | S3-20s | Address32 signed disp8 SIB qword MOV через shared IR и bounded dispatcher | Только `67 48 8B/89 /r sib disp8`, ModRM `mod=01,r/m=100`, data/base/index RAX..RBX, scale1/2/4/8, ровно 6 bytes |
| [partial] | S3-20t | Address32 signed disp32 SIB qword MOV через shared IR и bounded dispatcher | Только `67 48 8B/89 /r sib disp32`, ModRM `mod=10,r/m=100`, data/base/index RAX..RBX, scale1/2/4/8, ровно 9 bytes; singleton source metadata bound 9 bytes |
| [partial] | S3-20u | RBP-base signed disp8 qword MOV через shared IR и bounded dispatcher | Только `48 8B/89 /r disp8`, ModRM `mod=01,r/m=101`, RBP base/data RAX..RBX, ровно 4 bytes; decoder preserves RBP #SS semantics |
| [partial] | S3-20v | RSP-base qword MOV через exact no-index SIB `0x24` в shared IR и bounded dispatcher | Только `48 8B/89 /r sib`, ModRM `mod=00,r/m=100`, SIB `0x24`, RSP base, no index/displacement, data RAX..RBX, ровно 4 bytes |
| [partial] | S3-20w | RSP-base qword MOV через exact no-index SIB `0x24` + disp8 | Только `48 8B/89 /r sib disp8`, ModRM `mod=01,r/m=100`, RSP base/no index, ровно 5 bytes |
| [partial] | S3-20x | RSP-base qword MOV через exact no-index SIB `0x24` + disp32 | Только `48 8B/89 /r sib disp32`, ModRM `mod=10,r/m=100`, RSP base/no index, ровно 8 bytes |
| [partial] | S3-20y | RBP-base signed disp32 qword MOV через shared IR и bounded dispatcher | Только `48 8B/89 /r disp32`, ModRM `mod=10,r/m=101`, RBP base/data RAX..RBX, ровно 7 bytes; RIP-relative `mod=00/rm=101` остаётся отдельной формой |
| [partial] | S3-20z | RSP как data register в qword MOV через shared IR и bounded dispatcher | Только `48 8B/89 /r`, ModRM `mod=00`, data RSP, base RAX..RBX, ровно 3 bytes; generic GPR admission не расширяется |
| [partial] | S3-20aa | RBP как data register в qword MOV через shared IR и bounded dispatcher | Только `48 8B/89 /r`, ModRM `mod=00`, data RBP, base RAX..RBX, ровно 3 bytes; generic GPR admission не расширяется |
| [partial] | S3-20ab | RBP одновременно data и base для qword MOV с signed disp8 | Только `48 8B/89 6D ib`, ModRM `mod=01/reg=RBP/rm=RBP`, ровно 4 bytes; адрес использует старый RBP |
| [partial] | S3-20ac | RSP одновременно data и base в qword MOV через exact SIB | Только `48 8B/89 24 24`, ModRM `mod=00/rm=100`, SIB `0x24`, ровно 4 bytes; адрес использует старый RSP |
| [partial] | S3-20ad | RSP одновременно data и base в qword MOV через exact SIB + signed disp8 | Только `48 8B/89 64 24 ib`, ModRM `mod=01/reg=RSP/rm=100`, SIB `0x24`, ровно 5 bytes; адрес и store payload используют старый RSP |
| [partial] | S3-20ae | RSP одновременно data и base в qword MOV через exact SIB + signed disp32 | Только `48 8B/89 A4 24 id`, ModRM `mod=10/reg=RSP/rm=100`, SIB `0x24`, ровно 8 bytes; адрес и store payload используют старый RSP, байты displacement 4..7 сохраняются |
| [partial] | S3-20af | RBP одновременно data и base в qword MOV через signed disp32 | Только `48 8B/89 AD id`, ModRM `mod=10/reg=RBP/rm=RBP`, ровно 7 bytes; адрес и store payload используют старый RBP |
| [partial] | S3-20ag | RBP как data и RSP как base через exact no-index SIB | Только `48 8B/89 2C 24`, ModRM `mod=00/reg=RBP/rm=SIB`, SIB `0x24`, ровно 4 bytes |
| [partial] | S3-20ai | RSP как data и RBP как base через direct disp8/disp32 | Только `48 8B/89 65 ib` или `48 8B/89 A5 id`, ModRM `mod=01/10,reg=RSP,rm=RBP`, ровно 4/7 bytes; `mod=00/rm=5` остаётся RIP-relative |
| [partial] | S3-20aj | RSP/RBP как data через direct disp8/disp32 от RAX..RBX | Только `48 8B/89 /r disp8|disp32`, ModRM `mod=01/10`, `reg=RSP|RBP`, `rm=RAX..RBX`, ровно 4/7 bytes; без SIB |
| [partial] | S3-20ak | RIP-relative qword MOV с RSP/RBP data через shared IR и bounded dispatch | Только `48 8B/89 25/2D disp32`, ModRM `mod=00,r/m=101`, `reg=RSP|RBP`, ровно 7 bytes; EA от NextRip текущей инструкции |
| [partial] | S3-20al | RSP/RBP data через indexed SIB qword MOV | Только существующие 64-bit indexed SIB формы с ModRM `reg=RSP|RBP`, base/index RAX..RBX, scale 1/2/4/8; lengths 4/5/8 |
| [partial] | S3-20am | RSP/RBP data через address32 direct-base qword MOV | Только `67 48 8B/89 /r [disp8|disp32]`, ModRM `mod=00/01/10`, `reg=RSP|RBP`, base RAX..RBX; длины 4/5/8 |
| [partial] | S3-20an | RSP/RBP data через address32 indexed-SIB qword MOV | Только `67 48 8B/89 /r sib [disp8|disp32]`, ModRM `mod=00/01/10,r/m=100`, data RSP/RBP, base/index RAX..RBX, scale 1/2/4/8; длины 5/6/9 |
| [partial] | S3-20ao | RSP/RBP data через address32 no-index SIB qword MOV | Только `67 48 8B/89 /r 24 [disp8|disp32]`, ModRM `mod=00/01/10,r/m=100`, SIB `0x24`, старый RSP как низко-32-битная база, data RAX..RBP; длины 5/6/9 |
| [partial] | S3-20ap | RSP/RBP data через address32 direct EBP-base qword MOV | Только `67 48 8B/89 /r disp8|disp32`, ModRM `mod=01/10,r/m=101`, старый EBP/RBP low32 base, data RAX..RBP; длины 5/8, `mod=00/rm=101` исключён |
| [partial] | S3-20aq | RSP/RBP data через indexed SIB с RSP base | Только `48 8B/89 /r sib [disp8|disp32]`, ModRM `mod=00/01/10,r/m=100`, SIB base=RSP, index RAX..RBX, scale 1/2/4/8, data RAX..RBP; длины 4/5/8 |
| [partial] | S3-20ar | RSP/RBP data через indexed SIB с RBP base | Только `48 8B/89 /r sib disp8|disp32`, ModRM `mod=01/10,r/m=100`, SIB base=RBP, index RAX..RBX, scale 1/2/4/8, data RAX..RBP; длины 5/8; `mod=00/base=101` остаётся no-base и запрещён |
| [partial] | S3-20as | Address32 qword MOV через indexed SIB с RSP base | Только `67 48 8B/89 /r sib [disp8|disp32]`, ModRM `mod=00/01/10,r/m=100`, SIB base=RSP, index RAX..RBX, scale 1/2/4/8, data RAX..RBP; Dword EA/Qword data; длины 5/6/9 |
| [partial] | S3-20at | Address32 qword MOV через indexed SIB с RBP base | Только `67 48 8B/89 /r sib disp8|disp32`, ModRM `mod=01/10,r/m=100`, SIB base=RBP, index RAX..RBX, scale 1/2/4/8, data RAX..RBP; Dword EA/Qword data; длины 6/9; `mod=00/base=101` остаётся no-base и запрещён |
| [partial] | S3-20au | Absolute qword MOV через no-base/no-index SIB disp32 | Только `48 8B/89 /r 25 disp32`, ModRM `mod=00,r/m=100`, SIB ровно `0x25`, data RAX..RBP; длина 8; адрес — sign-extended disp32 |
| [partial] | S3-20av | Address32 absolute qword MOV через no-base/no-index SIB disp32 | Только `67 48 8B/89 /r 25 disp32`, ModRM `mod=00,r/m=100`, SIB ровно `0x25`, data RAX..RBP; Dword EA/Qword data; длина 9 |
| [partial] | S3-20aw | Address64 indexed no-base SIB disp32 qword MOV | Только `48 8B/89 /r sib disp32`, ModRM `mod=00,r/m=100`, SIB base=101/index RAX..RBX/scale 1/2/4/8, data RAX..RBP; длина 8; SIB `0x25` no-index остается отдельным profile |
| [partial] | S3-20ax | Address32 indexed no-base SIB disp32 qword MOV | Только `67 48 8B/89 /r sib disp32`, ModRM `mod=00,r/m=100`, SIB base=101/index RAX..RBX/scale 1/2/4/8, data RAX..RBP; Dword EA/Qword transfer; длина 9 |
| [partial] | S3-20ay | Address64 no-displacement SIB qword MOV с индексом RBP | Только `48 8B/89 /r sib`, ModRM `mod=00,r/m=100`, base RAX..RBX, index RBP, scale 1/2/4/8, data RAX..RBP; длина 4; остальные индексные профили остаются exact |
| [partial] | S3-20ba | Address32 indexed SIB qword MOV with RBP index | Только `67 48 8B/89 /r sib [disp8|disp32]`, ModRM `mod=00/01/10,r/m=100`, base RAX..RBX, index RBP, scale 1/2/4/8, data RAX..RBP; Dword EA/Qword data; длины 5/6/9 |
| [partial] | S3-20bb | Address64 indexed SIB qword MOV with RSP base and RBP index | Только `48 8B/89 /r sib [disp8|disp32]`, ModRM `mod=00/01/10,r/m=100`, SIB base=RSP/index=RBP, scale 1/2/4/8, data RAX..RBP; длины 4/5/8 |
| [partial] | S3-20bc | Address64 indexed SIB qword MOV with RBP base and RBP index | Только `48 8B/89 /r sib disp8|disp32`, ModRM `mod=01/10,r/m=100`, SIB base=index=RBP, scale 1/2/4/8, data RAX..RBP; длины 5/8; `mod=00/base=101` остаётся no-base |
| [partial] | S3-20bd | Address32 indexed SIB qword MOV with RSP base and RBP index | Только `67 48 8B/89 /r sib [disp8|disp32]`, ModRM `mod=00/01/10,r/m=100`, SIB base=RSP/index=RBP, scale 1/2/4/8, data RAX..RBP; Dword EA/Qword data; длины 5/6/9 |
| [partial] | S3-20be | Address32 indexed SIB qword MOV with RBP base and RBP index | Только `67 48 8B/89 /r sib disp8|disp32`, ModRM `mod=01/10,r/m=100`, SIB base=index=RBP, scale 1/2/4/8, data RAX..RBP; Dword EA/Qword data; длины 6/9; `mod=00/base=101` остаётся no-base |
| [partial] | S3-20bg | Address32 indexed no-base SIB qword MOV with RBP index | Только `67 48 8B/89 /r sib disp32`, ModRM `mod=00/rm=100`, SIB base=101/index=RBP/scale 1/2/4/8, data RAX..RBP; Dword EA/Qword data; длина 9; 20bf Qword profile остаётся отдельным |
| [partial] | S3-20bh | Address64 direct-base qword MOV через RSI/RDI | Только `48 8B/89 /r`, ModRM `mod=00/rm=110|111`, data RAX..RBP; RSI/RDI — base only, no SIB/displacement/prefix; длина 3 |
| [partial] | S3-20bi | Address64 direct-base qword MOV через RSI/RDI с displacement | Только `48 8B/89 /r disp8|disp32`, ModRM `mod=01/10/rm=110|111`, data RAX..RBP; RSI/RDI — base only; длины 4/7 |
| [partial] | S3-20bj | Address32 direct-base qword MOV через RSI/RDI | Только `67 48 8B/89 /r [disp8|disp32]`, ModRM `mod=00/01/10`, `rm=110|111`, data RAX..RBP; длины 4/5/8; EA по low32 базы и signed displacement |
| [partial] | S3-20bk | Address64 indexed SIB qword MOV с RSI/RDI base | Только `48 8B/89 /r sib`, ModRM `mod=00/rm=100`, SIB base RSI/RDI, index RAX..RBX, scale 1/2/4/8, data RAX..RBP; длина 4 |
| [partial] | S3-20bl | Address64 indexed SIB qword MOV с RSI/RDI base и displacement | Только `48 8B/89 /r sib disp8|disp32`, ModRM `mod=01/10,r/m=100`, SIB base RSI/RDI, index RAX..RBX, scale 1/2/4/8, data RAX..RBP; длины 5/8 |
| [partial] | S3-20bm | Address32 indexed SIB qword MOV с RSI/RDI base | Только `67 48 8B/89 /r sib [disp8|disp32]`, ModRM `mod=00/01/10,r/m=100`, SIB base RSI/RDI, index RAX..RBX, scale 1/2/4/8, data RAX..RBP; Dword EA/Qword data; длины 5/6/9 |
| [partial] | S3-20bn | Address64 indexed SIB qword MOV с RSI/RDI base и RBP index | Только `48 8B/89 /r sib [disp8|disp32]`, ModRM `mod=00/01/10,r/m=100`, SIB base RSI/RDI, index RBP, scale 1/2/4/8, data RAX..RBP; Qword EA/data; длины 4/5/8 |
| [partial] | S3-20bp | Address64 no-displacement SIB qword MOV with RSI/RDI index | Только `48 8B/89 /r sib`, ModRM `mod=00/rm=100`, base RAX..RBX, index RSI/RDI, scale 1/2/4/8, data RAX..RBP; длина 4 |
| [partial] | S3-20br | Address32 indexed SIB qword MOV с low base и RSI/RDI index | Только `67 48 8B/89 /r sib [disp8|disp32]`, ModRM `mod=00/01/10,r/m=100`, base RAX..RBX, index RSI/RDI, scale 1/2/4/8, data RAX..RBP; Dword EA/Qword data; длины 5/6/9 |
| [ ] | S3-21 | Финальный физический gate для memory slice после локального Stage 3 | S3-20; S2-09 и нужный native profile |
| [partial] | S3-22 | Смена execution/mapping context | S3-18, S3-21 |
| [partial] | S3-22a | Bounded S0_09 fixture context generation/profile in translation keys and cached dispatch | Current synchronous compile/publish holds a caller-owned frozen snapshot; production owner lease and revalidation for any future async/unfrozen compilation remain open |
| [partial] | S3-23a | Fixture owner notifications для изменения backing/mapping и cache invalidation между sync dispatch calls | Только неактивные entries; current synchronous caller holds frozen snapshot; production owner/writers and future async/unfrozen candidate commit remain open |
| [partial] | S3-23c | Fixture-only V2 generated store to a different tracked executable backing | One vCPU/fixed RAM; active-source overlap rejected; no active-code retirement or production MMU |
| [partial] | S3-23d | Shared fixture write transaction for interpreter MOV, V2 store, and control writes | Interpreter and control adapters commit between calls; production MMU/DMA/MMIO/SMP remain open |
| [partial] | S3-23e | Region-scoped executable-write notification and redispatch | Separate store entry invalidates cached prefix/tail and aliases before next lookup; no active native tail or S3-24 claim |
| [partial] | S3-23f | Fixture qword store across two adjacent mappings of one backing | At most two contiguous same-backing mappings; all-or-nothing single-slice copy and one version/notification |
| [partial] | S3-23g | Fixture qword store across two adjacent mappings with distinct BackingIds | Fixed two-segment plan, one-vCPU copy transaction, one version/notification per backing; no hardware atomicity or SMP |
| [ ] | S3-23 | Инвалидация code dependencies | S3-18, S3-21, S3-22; полный writer/MMU integration |
| [ ] | S3-24 | Self-modifying code в текущем блоке | S3-23 |
| [partial] | S3-24a | Active native three-instruction artifact exits after committed store changes already executed prefix | One typed V3 slot/call; future-tail writes rejected; future-instruction SMC oracle and general blocks remain open |
| [partial] | S3-27a | Exact V3 store architectural fault payload and precise prefix state | Fixture-injected #PF after checked write entry; host decoder negatives; full S3-27 fault/fetch/PC-map coverage remains open |
| [partial] | S3-27b | Fault on interpreter fetch at `A+6` after one exact two-site pair | Native pair commits are compared against interpreter-only steps; fixture-only boundary evidence, production MMU/dispatcher remains open |
| [ ] | S3-25 | Eviction, retirement и освобождение | S3-18, S3-24 |
| [partial] | S3-25a | One-vCPU synchronous cache/arena handle lifetime and safe-point checks | Inactive entries only; active-code retirement, SMP, deferred queue not included |
| [partial] | S3-25b | Capacity-two arena lifetime for one V3 artifact and one cached prefix | V3 slot survives cache clear/invalidation; stale source rejected; full retirement/SMP/deferred queue remain open |
| [ ] | S3-26 | Порядок side effects и memory classes | S3-19, S3-24 |
| [ ] | S3-27 | Точные fault exits и восстановление state | S3-12, S3-21, S3-26 |
| [ ] | S3-28 | STOP/events и безопасные точки | S3-15, S3-16, S3-27 |
| [ ] | S3-29 | Flags по потребности и сравнение стратегий | S3-01, S3-27, S3-28 |
| [ ] | S3-30 | Выбор allocator по измерениям | S3-01, S3-29 |
| [ ] | S3-31 | Прямые связи блоков, если оправданы | S3-25, S3-28, S3-30 |
| [ ] | S3-32 | Локальные IR-преобразования, если оправданы | S3-03, S3-27, S3-30 |
| [ ] | S3-33 | Расширение lowering по готовности S2 | S3-27, S3-29; готовый interpreter/native subset |
| [ ] | S3-34 | Corpus, reducer и негативные проверки | S3-24–S3-33 в принятой конфигурации |
| [ ] | S3-35 | Измерить холодное исполнение | S3-01, S3-34 |
| [ ] | S3-36 | Измерить прогретое исполнение | S3-35 |
| [ ] | S3-37 | Cache churn и ресурсы | S3-25, S3-34 |
| [ ] | S3-38 | Интеграция VMHost и ABI | S3-28, S3-34 |
| [ ] | S3-39 | Повторить macOS/ARM smoke на M1 Pro | S3-38 |
| [ ] | S3-40 | Зафиксировать решения по данным | S3-35–S3-39 |
| [ ] | S3-41 | Документация и приёмка этапа | Все обязательные задачи |

Первый вертикальный путь: `00 → 02 → 03 → 04 → 05`, затем `06 → 07 → 08 → 09 → 10 → 11 → 12 → 13 → 14`. Baseline S3-01 выполняется до выбора оптимизаций. Тесты добавляются в каждой задаче, S3-34 объединяет их и проверяет сам стенд.

## 6. Первый блок: от IR до физического сравнения

### S3-00. Зафиксировать область первого JIT

**Файлы:** [JIT_CONTRACT](../specs/JIT_CONTRACT.md), [JIT testing](../testing/JIT.md); ссылки на CPU_CONTRACT и текущие отчёты.

1. Сопоставить реальные формы interpreter/JIT/native. Начать с подтверждённого S0 register subset; расширенный v2/memory профиль вести отдельными строками.
2. Для каждой строки указать native lowering, helper, interpreter fallback или unsupported, область сравнения и внешний prerequisite.
3. Зафиксировать один vCPU, последовательную компиляцию, первые block limits и выбранную host-конфигурацию. Лимиты — явные начальные ограничения, а не утверждение об оптимальности.
4. Записать нормативные источники ARM64/ABI/macOS API и версии, нужные для конкретных используемых механизмов.

**Готово:** [контракт JIT](../specs/JIT_CONTRACT.md) фиксирует точные байтовые формы первого S0-09 subset, native/helper/unsupported границы, сравниваемое состояние, начальные runtime limits, host configuration и источники ISA/ABI/macOS API. [Методика](../testing/JIT.md) задаёт порядок локального и физического сравнения и prerequisites будущих срезов. Этап 2 не объявлен завершённым, а memory semantics не стала основанием JIT-приёмки.

### S3-01. Подготовить baseline и сравнение вариантов

**Файлы:** BENCHMARKS, существующий runner/bench harness; evidence после реальных запусков.

1. Запустить текущий local/oracle набор этапа 1 и сохранить fingerprint. Зафиксировать фактический статус S2 memory/native проверок.
2. Описать workloads: линейный integer, короткий/длинный loop, conditional branches, несколько blocks, helper-heavy путь; memory/fallback/invalidation добавлять вместе с их реализацией.
3. Разделить cold process start, cold code cache и warm block execution. Задать одинаковые входы, начальный state, guest work, correctness и build conditions.
4. Подготовить повторные calibrated samples с переменным порядком вариантов и сырыми durations/counts. Не придумывать speedup threshold без требований; шум не считать улучшением.

**Готово:** воспроизводимый протокол позволяет сравнивать решения до их окончательного выбора. Отсутствующие workloads отмечены, старые fragment numbers не перенесены на block JIT.

### S3-02. Определить runtime contract

**Файлы:** od-jit, od-host interface boundary, JIT_CONTRACT.

1. Описать runtime context, входы/выходы блока, state layout, budget, stop/event checks и helper ABI.
2. Развести guest fault, fallback request, unsupported, compilation failure и host failure. Зафиксировать, какие ошибки допускают документированный interpreter path, а какие прекращают запуск.
3. Описать executable handle, ownership и обязанности publisher. Не создавать циклическую зависимость `od-jit ↔ od-host`.

**Готово:** интерфейс покрывает конкретный первый срез и типы выходов; нет Rust enum/layout guesses в generated ABI и необъявленных clobbers.

### S3-03. Ввести небольшой типизированный IR

**Предлагаемые файлы:** `od-jit/src/ir.rs` и локальные tests.

1. Добавить только нужные операции: constants/temporaries, reads/writes guest state, integer effects, flags, branches, exits; memory/helper forms добавлять с их задачами.
2. Каждой операции задать width, inputs/outputs и side effects. Guest instruction boundary и commit point должны оставаться представимыми.
3. Реализовать verifier: определённость values, widths, допустимые targets, обязательные exits и state metadata. Не начинать с глобального SSA-оптимизатора.

**Готово:** некорректный IR отвергается до executable publication. Тесты проверяют реальные нарушения контракта, а не только существование enum.

### S3-04. Построить bounded basic block

**Файлы:** `od-jit/src/block.rs`, decoder adapter.

1. Декодировать с entry RIP по одному instruction, используя существующий decoder и mode context.
2. Останавливать построение на terminator, лимите, context-changing effect или форме без lowering. Не интерпретировать недостижимый хвост.
3. На fetch/decode fault после валидного префикса завершить префикс с выходом к следующему RIP. Fault на первой инструкции наблюдать в нужной runtime точке, не раньше исполнения предыдущего блока.
4. Сохранить guest byte ranges всех decoded instructions, включая переход через границу страницы. Code identity/version tracking относится к S3-17, проверка изменения и cache invalidation — к S3-23.

**Готово:** tests покрывают entry на новой boundary, entry на code_end, instruction/byte limits, truncation на первой инструкции и после валидного префикса, unsupported form после префикса, branch без декодирования хвоста, target внутрь инструкции, zero-displacement target на `next_rip`/`code_end` и unresolved forward target. Для декодированного префикса сохранены точные guest ranges; fetch exhaustion не назван guest fault. Mid-instruction target останавливает сборку до branch и сохраняет prefix. Branch target вычисляется 64-битным wrapping сложением, как в interpreter; `next_rip` текущей Jcc считается известной sequential boundary, но последующие bytes не декодируются. Остальные unresolved targets не передаются текущему IR verifier до решения S3-05/S3-15. Ranges не устанавливают code identity/version; tracking принадлежит S3-17, а cache invalidation — S3-23.

### S3-05. Перевести decoder output в IR

1. Реализовать MOV/ADD/SUB/CMP/JZ/JNZ exact forms первого профиля. Сохранить operand widths, flags effects и guest RIP каждой инструкции.
2. Неподдержанный lowering оформлять явным block exit/fallback request, не NOP и не приблизительным результатом.
3. Использовать decoder description; не декодировать x86 повторно в emitter и не копировать семантику в runner.
4. Для unresolved Jcc не включать branch в IR: сохранить prefix-only `NeedInterpreter` outcome, а для пустого prefix возвращать явный `NoNativePrefix`. Mid-instruction target должен завершаться `Unsupported` до Jcc.
5. Передавать точный `CodeView.code_end` в `Program`; verifier принимает только эту дополнительную boundary и проверяет, что guest records находятся внутри неё. Commit marker и terminal branch вместе образуют retirement/target-selection boundary без observable operation между ними.

**Готово:** IR содержит наблюдаемые effects каждого ручного case, verifier проходит; неверная или неподдержанная форма не публикуется как успешный native block. Targeted tests должны покрывать operand alias/order, effects, branch boundary cases, prefix fallback, deferred decode boundary и code_end admission.

### S3-06. Выделить и проверить ARM64 encoder

**Файлы:** `od-jit/src/arm64/encoder.rs`, узкий публичный модуль `arm64`, существующий caller `od-jit/src/lib.rs` и небольшой local assembler fixture.

1. Перенести существующие используемые encodings в typed fallible API без изменения legacy fragment output. Отдельно типизировать GPR, SP и zero registers; проверять immediate, scaled/signed offsets, alignment и little-endian serialization.
2. Поддержать текущие branch/call encodings и local-label relocations. `finish` не должен возвращать output при unresolved labels или displacement вне диапазона; displacement не обрезать молча.
3. Сверить небольшой набор слов с локальным Apple ARM64 assembler fixture и нормативной A64 reference; зафиксировать toolchain/target. Не добавлять executable-memory publication (S3-07), entry/helper ABI (S3-08), allocator (S3-09), lowering (S3-10), veneers/relaxation или общую ISA framework.

**Готово:** known encodings и range failures проверены независимо от собственного x86 decoder; в `od-jit` fragment translator сохраняется прежний word output. Несовместимая или неразрешённая encoding возвращает ошибку до выдачи буфера.

### S3-07. Сделать публикацию кода управляемой

**Файлы:** существующий executable-memory path od-host и узкий publisher interface.

1. Повторно использовать macOS JIT APIs и соответствующую конфигурацию подписи. Проверить min-OS/API availability и lifecycle на выбранных host версиях.
2. Разделить writable construction и executable publication, выполнить необходимую instruction-cache синхронизацию до entry visibility.
3. Возвращать owning handle; allocation/publication failure не возвращает usable entry. Проверить очистку частично созданного mapping.
4. Сначала immutable blocks; никаких конкурентных write/execute в одной области и patching активного кода.

**Фактически сделано:** `od-host::PublishedCode::publish` строит байты в private `MAP_JIT`, RAII guard возвращает write protection до cache sync, `sys_icache_invalidate` происходит до выдачи owner. Owner удерживает один mapping и освобождает его в Drop. Проверены отказ второй публикации и повторное использование slot после Drop; C ABI переводит конкурентный host resource отказ в существующий `STATUS_BUSY`, layout/signatures C ABI не менялись. `cargo fmt --all -- --check`, `cargo check --workspace --all-targets --offline`, `cargo test -p od-host --offline` (8/8), `git diff --check`; layout check прошёл в основном checkout. Независимое review clean после исправления ссылки на ISSUE-021.

Apple SDK 27.0 объявляет `pthread_jit_write_protect_np` доступным с macOS 11.0; явный deployment target проекта не найден. В Hardened Runtime с allow-jit допускается одна MAP_JIT region на процесс, поэтому `od-host` допускает один standalone mapping или одну shared arena. Указанная write protection действует на текущий thread и не является process-wide synchronization с посторонними JIT users; `od-host` не пишет standalone опубликованный code повторно. Подробная граница — [ISSUE-021](../KNOWN_ISSUES.md#issue-021--ограничение-одной-map_jit-region-и-незавершённая-multi-block-интеграция).

Ad-hoc подписанный Hardened Runtime VMHost smoke с allow-jit entitlement и JIT execution через launcher прошёл на macOS 26.7.1 ARM64 / Xcode SDK 27.0 вне debugger. Это локальная проверка текущей машины, не Developer ID signing, notarization, packaged-app acceptance или подтверждение другой версии macOS; ISSUE-004 остаётся открытой. S3-18b добавляет host-only bounded slots внутри одной region, но od-jit multi-entry cache integration и performance-based capacity choice остаются открытыми. Не использовать per-block mmap как multi-block implementation.

### S3-08. Реализовать generated entry и helper calls

1. Зафиксировать prologue/epilogue, stack alignment, callee-saved/platform registers и representation arguments/results.
2. Создать один реальный helper call с объявленными clobbers; проверить state до/после, nested host call и error return.
3. Исключить unwind через generated code и aliasing живых Rust references с generated state access. Host failures не преобразовывать в guest exceptions.

S3-08 выполнена: `runtime` объявляет все `RuntimeContextV1` offsets через `offset_of!`, `BlockEntry`/helper AAPCS64 clobber contract и отдельный raw helper-failure sentinel. `od-host::decode_block_entry_result` отвергает sentinel как host contract failure до guest-exit interpretation. Native ARM64 test-only typed-Encoder probe создаёт frame record через x29/x30, сохраняет x19-x29, вызывает nested Rust helper по ABI `(w0,x1,x2,x3,x4)`, проверяет exact arguments, volatile x1-x17 clobbers, SIMD D8-D15 canaries, status 0/1/2/unknown и failure output staging без guest-state write/retirement. Он не добавляет x86 helper semantics или общий block entry compiler.

`cargo fmt --all -- --check`, `cargo check --workspace --all-targets --offline`, `cargo test -p od-host --offline` прошли на Apple M3 Max/macOS 26.7.1 (11 tests); generated-entry success/error probes дополнительно прошли в ad-hoc подписанном Hardened Runtime test process с allow-jit entitlement. `git diff --check`, independent review clean и `tools/check_layout.py` прошли. C ABI не менялся. S3-13 ещё должен соединить этот contract с IR lowering; raw generic entry invocation остаётся unsafe.

### S3-09. Реализовать простой allocator baseline

1. Для первого набора temporaries выбрать простой проверяемый mapping/spill strategy с ограниченным scratch register set.
2. Реализовать stack slots и загрузку/сохранение values вокруг helper calls; запрещённые host registers исключить явно.
3. Добавить проверки конфликтов live values и spill slot bounds. Считать spill/reload/code size для будущего сравнения.

S3-09 выполнена: реализован первый baseline в `od-jit::allocator`: fixed homes x9..x12 в порядке определения, budget 0..=4, отдельные W64 spill slots, explicit helper boundaries/liveness/capture, transfer emission и checked frame. Unit tests и native helper-clobber probe добавлены; результат остаётся baseline cost estimate, не benchmark и не оптимизация. Независимое review clean; workspace check, форматирование, `od-jit` 46/46 tests, `od-host` 12/12 tests, diff-check и Graphify update прошли. Ограниченное покрытие комбинаций liveness зарегистрировано в [ISSUE-022](../KNOWN_ISSUES.md#issue-022--ограниченное-покрытие-liveness-allocator-tests) и расширяется при S3-10. Production consumption относится к S3-10; allocator не добавляет guest semantics.

### S3-10. Выполнить первый integer lowering

1. Добавить native lowering constants, MOV и арифметического результата ADD/SUB/CMP первого профиля.
2. Правильно обрабатывать guest widths и host zero/sign extension; CMP не записывает arithmetic result в guest destination.
3. Проверить chains temporaries, spills и входные registers с крайними bit patterns.

S3-10 выполнена: `od-jit::emitter::emit_data_effects` потребляет verifier-checked decoder-derived `Program` и связанную с ним `Allocation`; native od-host test проходит полный путь `CodeView → BasicBlock → lowering → allocation → emitter → publication → execution` для MOV imm64 и ADD/SUB/CMP reg64. Проверены register budgets 0..=4, spills, aliasing/equal operands, крайние bit patterns, многословная immediate и неравные SUB/CMP operands с borrow/wrap. `CMP` производит только наблюдаемый scratch result и не записывает гостевой GPR. Arithmetic flags остаются явно неполными; emitter не обновляет canonical `LongModeState`, RIP, budget или retirement и возвращает host-only uncommitted sentinel. Ветка/Jcc и неподдержанные exits отвергаются. Это промежуточные production bytes и staging probe, а не готовая guest-инструкция: S3-11 должен материализовать flags и определить точку архитектурного commit перед успешным guest exit. Проверки: `cargo fmt --all -- --check`, workspace all-target check, `od-jit` 50/50 tests, `od-host` 13/13 tests, `git diff --check`; integration test прошёл вне Cargo invocation в подписанном Hardened Runtime процессe с allow-jit entitlement; independent review clean, layout check успешен. Coverage ограничения allocator указаны в [ISSUE-022](../KNOWN_ISSUES.md#issue-022--ограниченное-покрытие-liveness-allocator-tests).

### S3-11. Реализовать eager flags baseline

1. Материализовать определённые CF/PF/AF/ZF/SF/OF для текущих форм, сохраняя требуемые неизменяемые bits.
2. Не отождествлять host NZCV с полным x86 RFLAGS. Jcc читает guest condition, а helper clobber не уничтожает pending значение.
3. Сверить carry/borrow, signed overflow, auxiliary carry и parity по независимым byte fixtures.

S3-11 выполнена: выделен общий `od-isa::integer_flags` helper для W64 ADD/SUB/CMP; тонкий `od-jit` ABI adapter читает output только при status Success, а data-effect emitter оставляет flags/выбранный RIP в uncommitted staging. Native tests покрывают оба исхода JZ/JNZ, MOV flag preservation, CMP без GPR write, helper failure без текущего guest commit и helper-clobber allocator homes. Literal vectors и interpreter cross-checks проверяют шесть определённых flags и сохранение остальных RFLAGS bits. Independent review clean; workspace check, fmt, `od-jit` 51/51, `od-host` 16/16, targeted `integer_flags` 3/3, diff-check и layout check прошли. Полный `od-isa` suite по-прежнему имеет пять несвязанных x87/decoder failures, зарегистрированных в [ISSUE-023](../KNOWN_ISSUES.md#issue-023--stage-1-test-expectations-reject-currently-accepted-x87-forms). Архитектурный commit и recovery остаются в S3-12.

### S3-12. Добавить state points и PC maps

1. Для каждой guest instruction и каждого предусмотренного exit записать guest RIP, host code range, commit state и способ материализации live values.
2. Восстановить точное состояние при helper/fallback/normal exits; не записывать next RIP до проверки faulting операции.
3. Проверить maps после relocation и связывание metadata с lifetime executable block. Padding/prologue/epilogue не выдавать за отдельные guest instructions.

S3-12 implementation добавляет production runtime emitter для текущего W64
register-only среза, commit markers, checked half-open PC ranges и отдельные
committed guest-boundary points. `PublishedCode::publish_block` держит карту с
mapping lifetime; безопасный host wrapper принимает только production-helper
provenance. Native tests проверяют normal exits, внутренние stop/budget guards,
JZ/JNZ selection и helper failure после committed prefix. Canonical state
обновляется лишь после успешных effects; helper failure оставляет текущую
инструкцию на её guest RIP. Общие branch-return/epilogue ranges указывают на
canonical RIP, тогда как site-specific failure stubs хранят фиксированный RIP;
reversed ranges отвергаются валидацией карты. Независимый review прошёл без
оставшихся замечаний; workspace/package checks и primary-checkout layout check
успешны.

Ограничение: ranges описывают emitted host bytes и planned exits, а не дают
универсальное восстановление для произвольного host PC, crash, panic, ABI
violation или asynchronous snapshot. Memory/fault replay отсутствует. Нативный
test evidence относится к production ARM64 block path на настроенном allow-jit
host; физическое сравнение с x86 относится к S3-14.

### S3-13. Подключить один блок к CLI

**Файлы:** существующий od-runner, od-host/od-jit adapters, testkit result path.

1. Выполнить compile → publish → execute → snapshot для первого профиля с теми же входами и compare scope, что у interpreter.
2. Сохранить старый fragment entry/ABI и fixtures. Новый путь должен быть различим в metadata результата, без подмены исторических evidence.
3. Проверить completed, unsupported, allocation/compile error и malformed result paths.

Текущая реализация добавляет opt-in `--fixture CASE_V1.json --engine
jit-block --json` с отдельным `jit_ir_block` backend tag. `od-jit` adapter
сохраняет `NoNativePrefix`, строит allocation/runtime emitter для одного
bounded block и отклоняет generated code свыше 4096 bytes до публикации. Runner
сохраняет старый `jit`/`--hex` path, делает fixture-v1 static decoder и
initial-RIP instruction-boundary compatibility checks, публикует и исполняет
один block ровно один раз, передаёт
`stop_rip=code_end` и не выполняет interpreter fallback/branch dispatch. V2
JIT остаётся unsupported. Child tests покрывают completed, budget, terminal
branch, недопустимую форму, internal branch без dispatcher, v2 denial и коды
выхода invalid/unsupported/harness. Независимый review нашёл и подтвердил
исправление compatibility-проверки initial RIP; повторные package checks прошли.

`BlockComplete` нормализуется сначала по фактическому RIP к `ReachedCodeEnd`,
затем по нулевому remaining budget к `InstructionBudgetExhausted`; другой
internal RIP означает `Unsupported`, так как для него нужен следующий block.
Это только одно-блочный execution path, не dispatcher и не сравнение с
физическим x86.

**Готово:** одна документированная команда реально исполняет IR-based block и выдаёт machine-readable result. Нулевой код возврата не появляется при fallback failure или отсутствии lowering.

### S3-14. Подтвердить register-only срез физически

1. Составить явный manifest программ, целиком исполняемых одним блоком: линейные формы и разрешённые terminal branches. Использовать подходящие неизменённые fixtures этапа 1 и собственные новые случаи; не требовать ещё отсутствующий dispatcher.
2. Сравнить interpreter, новый JIT и Mac mini native по определённому состоянию; повторить каждый case минимум дважды. Завершение native программы должно совпадать с выбранной границей сравнения.
3. Сохранить input, snapshots, fingerprints, host/toolchain и failures. Включить ABI/spill/branch boundary cases нового pipeline. Полный multi-block corpus этапа 1 запускается после S3-15.

Локальная подготовка добавила manifest
[`stage3_s3_14_jit_block.json`](../../tests/cpu/manifests/stage3_s3_14_jit_block.json):
8 исторических одно-блочных cases и 5 новых boundary inputs. Он исключает
`s0_09_sub_jnz_loop` (backward edge требует повторного dispatch) и
`s1_06_jz_not_taken` (fallthrough продолжает после Jcc в следующую инструкцию).
Новые случаи различают taken JZ/JNZ через валидный хвост до `code_end`,
not-taken JZ/JNZ с известной backward target boundary и fallthrough в
`code_end`, а также длинную цепочку helper calls с aliased ADD/SUB и
live-through значением. Целевой `od-jit` test на этой цепочке использует ту же
production allocation config и проверяет реальные value spill slots и helper
captures. Отдельный budget-boundary fixture в manifest отмечен
`local_only`: он завершает блок до `code_end` и не входит в физический набор.
Команда для будущего physical acceptance:

```sh
python3 tests/cpu/stage1_oracle.py --engine jit-block --manifest \
  tests/cpu/manifests/stage3_s3_14_jit_block.json --repeats 2 \
  --remote-root /ABSOLUTE/PATH/TO/ISOLATED_STAGE3_CHECKOUT
```

Legacy oracle сохраняет `jit` по умолчанию; `jit-block` без явных cases/manifest отклоняется,
а его результаты должны иметь backend `jit_ir_block`. Run manifest фиксирует
stage/suite, engine/backend и fixture/program digests; перед будущим oracle он
проверит что каждый обязательный fixture начинается с RIP 0 и ожидает
`reached_code_end`. Harness сравнивает interpreter непосредственно с выбранным
JIT, каждый из них — с native, и сравнивает каждый backend между повторами.
Orchestration tests проверяют выбор engine/backend, два повтора, отсутствие
implicit selection и failure при неверном backend tag. Все тринадцать manifest inputs локально совпали
между interpreter и `jit_ir_block`; вместе с harness tests это подтверждает
только локальную подготовку, не запуск Mac mini или triple physical acceptance.

Первый удалённый preflight для ревизии `980efd3` остановился на общем наборе
`od-isa`, `od-testkit`, `od-runner`, `od-x86-runner`: 706/711 тестов прошли,
пять известных x87/decode cases из [ISSUE-023](../KNOWN_ISSUES.md#issue-023--stage-1-test-expectations-reject-currently-accepted-x87-forms)
упали до сборки и запуска native fixtures. Полный stdout/stderr этого запуска
не был сохранён. Для `jit-block` harness теперь запускает только suites
`od-testkit`, `od-runner`, `od-x86-runner` (один `od-isa` всё равно собирается
как dependency), а стандартный `jit` сохраняет прежний полный набор. Любой
результат remote preflight записывает полный доступный stdout/stderr и exit
code в run folder до продолжения или abort; timeout сохраняет partial output,
timeout status, limit и команду (exit code остаётся null). Harness tests
покрывают routing, nonzero exit и timeout.
После этой правки повторная доставка ревизии `b8924dc` в отдельный remote
checkout остановилась на SSH connect timeout. Native fixtures не запускались;
физическая приёмка остаётся открытой. Детали попыток записаны в
[`S3-14 report`](../reports/STAGE_3_S3-14_2026-10-26.md).
S3-14 остаётся открытым.

**Готово:** срез A подтверждён тройным сравнением. Неподдержанные обязательные cases, недоступный SSH и mismatch оставляют задачу открытой.

## 7. Dispatcher, cache и память

### S3-15. Реализовать dispatcher и точный budget

1. После блока обработать next RIP, conditional/indirect target и выход без recursion по длинной цепочке блоков.
2. Считать guest instructions в JIT и fallback по единому contract. При недостаточном остатке budget использовать точный ограниченный путь, а не исполнять весь блок сверх лимита.
3. Проверить loop, zero/one budget, выход посреди блока, сохранение flags и bounded stack usage dispatcher.
4. Теперь прогнать все 10 ручных cases и фиксированный corpus из 1000 generated cases этапа 1 через новый runtime, interpreter и native oracle, минимум два повтора. Старые inputs/digests сохранить; сравнение только одного блока из программы больше не является достаточным.

**Готово:** multi-block program даёт состояние interpreter при том же budget; ошибочный target или невозможный exit не зацикливает runtime.

**Локальная partial реализация:** `od-jit` теперь имеет отдельный
`compile_block_for_dispatch`/`lower_for_dispatch` путь, который принимает
boundary witness только из полного непрерывного fixture-v1 decode, повторно
сверенного с теми же bytes. Dispatch API проверяет entry и все decoded block
ranges против этого admission. Typed `DispatchBranch` дополнительно
проверяется verifier по target, fallthrough, всем included IR records и точному
`code_end`. Runtime commit-ит выбранный Jcc RIP один раз и
возвращает `DispatchCandidate` (tag 8) с обычными guest counters. Legacy
`compile_block`/S3-13 runner не получают admission и сохраняют прежний
one-block `BlockComplete`/`NeedInterpreter` результат.

Следующий локальный partial срез добавляет `od_jit::dispatch::run` и
`od_host::PublishedBlockExecutor`. Dispatcher компилирует только текущий
admitted RIP, передаёт generated block точный remaining budget и проверяет
retired/remaining против входного budget и emitted instruction count. Он
завершает на `code_end`, возвращает budget/runtime exit, продолжает только на
admitted RIP после положительного retirement и отклоняет zero-progress loop.
Начальный `code_end` проверяется раньше нулевого budget; при budget=0
компиляция и execution callback не вызываются. Host executor публикует,
исполняет и уничтожает ровно один `PublishedCode` за вызов, до следующей
итерации; рекурсии и одновременно живых mappings нет. Целевые od-jit tests
покрывают двухблочный переход, budget forwarding, malformed counters,
unadmitted target, runtime unsupported exit и zero progress. Native od-host
integration test покрывает последовательные mappings и положительный
self-loop на macOS ARM64.

Ограниченный checked-memory continuation добавлен отдельно в
`od_jit::memory_dispatch::run` и `od_host::PublishedMemoryBlockExecutor`.
Он работает только с двумя принятыми two-site artifact sequences и singleton
MOV qword load/store продолжениями, по immutable borrowed `CodeView` и явной
fixture execution boundary. Общий budget уменьшается по фактическому retirement;
STOP/stop-RIP, архитектурный second-site fault и host publication failure
сохраняют точный prefix state. Host executor освобождает каждый mapping до
следующей итерации. Unsafe caller contract сохраняет private synchronous RAM,
all-or-nothing write, no-MMIO/no-alias/no-concurrency требования; он не
подменяет capability. `od-machine` пока не содержит production owner, поэтому
owner-backed VMHost/MMU integration и owner-supplied code fetch остаются
открытыми. Явная fixture boundary — byte-window limit, не guest fetch; полный
code-fetch fault delivery остаётся непокрытым до owner integration. Две focused native differential проверки
сравнивают independent interpreter snapshots, budgets 1..=4, intra-pair и
inter-pair stop, data fault/resume without duplicate read, partial byte windows
at A+1/A+2/A+4, complete unsupported/invalid versus genuinely truncated decode,
cooperative STOP before entry/inside both pairs/after the final callback with
resume from actual RIP, unsupported tail и host error после первого committed
block. См. [memory-dispatch report](../reports/STAGE_3_S3-15_2026-10-27.md#bounded-checked-memory-dispatcher-partial).

#### S3-15n. Production memory-owner integration prerequisites

The borrowed dispatcher remains a bounded fixture/API slice. Production
`od-machine` RAM/MMU ownership is a prerequisite and is not implemented by
S3-15n. Keep component responsibility explicit: `od-machine` owns RAM backing,
guest mappings, translation/permissions, code-source leases and committed-write
notifications; `od-jit` owns fetch/decode continuation policy, compile/cache/
dispatch and dependency invalidation; `od-host` owns executable publication
and mapping release. The dependency table and lifetime/fault contract are in
[JIT_CONTRACT](../specs/JIT_CONTRACT.md#production-memory-owner-integration-prerequisites-s3-15n).

The production path requires: (1) a synchronous private-RAM capability and
owner-backed code lease identifying byte ranges, backing/mapping identity and
generation, permissions and immutable lifetime; (2) owner-supplied incremental
fetch/decode so a later guest fetch fault is deferred until continuation
reaches its RIP, while compile errors remain host errors and fixture-window
end is not guest termination; (3) stop-RIP → STOP → budget → fetch ordering;
(4) one committed-write notification batch from every writer, including
aliases, mapping/protection changes and active-code dirty state, observed
before the next lookup/publication; (5) safe revocation and lease/mapping
release on all exits. Generic `GuestMemory` is not proof of private RAM or
revocation. Avoid adding trait layers until the concrete `od-machine` owner
exists and its required operations are known.

Acceptance must exercise permission and revocation, cross-page fetch, deferred
next-instruction faults after committed prefixes, all writer notifications and
aliases, stale-code suppression at the next safe point, plus publication and
lease release before another artifact. This prerequisite does not complete
S3-15: the full Stage 1 corpus, physical Mac mini comparison, and measured
cold/warm performance gates remain separate dependencies.

Следующий локальный CLI partial добавляет opt-in fixture-v1 engine
`jit-dispatch` и отдельный сериализуемый backend `jit_ir_dispatch`. Runner
повторно связывает полный decode с immutable fixture bytes, проверяет entry
boundary и вызывает итеративный dispatcher без fallback. Child tests на
macOS ARM64 покрывают taken/not-taken RIP, переходы между блоками,
backward/self-loop, budget внутри блока, budget-exact code_end, unsupported
decode/target и сохранность `jit`/`jit-block`. Fixture schema v1 запрещает
нулевой budget, поэтому его приоритет остаётся проверен API unit test. Это
не запускает 10-case плюс 1000-case corpus или физический x86 oracle;
S3-14 физически остаётся открытой, а S3-15 открыта до полного acceptance
критерия выше. Проверки текущего partial slice прошли:
`cargo fmt --all -- --check`, `cargo check -p od-jit --all-targets --offline`,
`cargo test -p od-jit --offline` (64 tests), `cargo check -p od-host
--all-targets --offline`, `cargo test -p od-host --offline` (24 tests,
включая native iterative dispatch на текущем macOS ARM64 host),
`cargo test -p od-testkit --offline` (184 tests), `cargo test -p od-runner
--offline` (304 total: 101 unit, 4 jit-block CLI, 3 jit-dispatch CLI, 196 v2),
`cargo check --workspace --all-targets --offline`, `git diff --check` и
`graphify update .`. Независимый review CLI-среза чистый.

Harness для следующей проверки также поддерживает `jit-dispatch`: по умолчанию
выбирается 10 ручных cases плюс 1000 generated cases с seed `0x5eedcafe`, два
повтора. `--local-only` запускает только interpreter и `jit-dispatch`, сохраняет
frozen fixtures, сырые результаты и cross/repeat comparisons, а manifest явно
помечает `evidence_scope=local_only`, `physical_x86=not_run`. Этот режим не
засчитывается как physical acceptance. Для `jit-dispatch` удалённый Cargo
preflight использует `od-testkit`, `od-runner` и `od-x86-runner`; `od-isa`
остаётся dependency, но полный unit suite пропускается из-за пяти известных
x87 failures ISSUE-023. Полный физический режим сохраняет тройное сравнение и
metadata. Сам 10+1000 local-only прогон и его повторные результаты должны быть
отделены от hardware acceptance. На revision для этого среза local-only прогон
завершился успешно: 10 manual + 1000 generated inputs, два повтора, 4040
локальных engine results, включая 2020 interpreter↔dispatch comparisons и
2020 per-engine repeat comparisons, seed `0x5eedcafe`, corpus SHA-256
`9356e916b2037a3f5a082e8366fe33bf4277ab00812489809bfc586210744d35`.
Manifest: `work/stage1-oracle/stage1-20261027-000237-882135f1/manifest.json`;
summary: `work/stage1-harness/stage1-harness-20261027-000235-842de746/summary.json`.
Физический triple run не выполнялся и остаётся pending; локальное совпадение двух
собственных движков не закрывает S3-14/S3-15. Фактический локальный результат и
machine-readable summary записаны в
[`S3-15 report`](../reports/STAGE_3_S3-15_2026-10-27.md).

### S3-16. Подключить interpreter fallback

1. На форме без lowering материализовать state и вызвать реально поддерживающий её interpreter step. При отсутствии поддержки обоих вернуть unsupported.
2. Согласовать budget, STOP, fault и memory effects. После helper обновить context/code validity перед возвратом в JIT.
3. Защититься от нулевого прогресса и повторной компиляции того же неподдержанного entry без исполнения.
4. Учитывать compiled instructions, helper calls и interpreter steps раздельно в metadata/benchmarks.

**Готово:** смесь native blocks и interpreter instructions совпадает с чистым interpreter. Новые S2 формы используются только после их проверки; all-fallback workload не называется JIT-ускорением.

**Локальная partial реализация (S3-16):** добавлены отдельные
`S0_09NopAdmission` и `run_with_nop_fallback`. Admission строит полные
instruction boundaries по immutable `CodeView`, допускает S0-09 и только exact
bare `0x90`, проверяет branch targets по этим boundaries и отклоняет прочие
формы. Для NOP используется существующий `od_isa::execute::step` с fetch-only
memory, ограниченной одним байтом; x86 effects в JIT не повторяются.
`ControlledBlockExecutor` передаёт один `stop_rip` и тот же `AtomicU8 STOP` в
native runtime и pre-step guards. Native mapping освобождается до fallback.
Пер-часть native/interpreter retirement проверяется общей budget-инвариантой.
Tests покрывают all-fallback NOP, native prefix → NOP → native CMP/Jcc, flags,
budgets, stop order, отказ admission и terminal executor errors. Это отдельный
Rust API: fixture `jit-dispatch`, schema v1, legacy JIT и generic Stage 2
fallback не расширены. S3-16 остаётся открытой; эта узкая NOP проверка не
подтверждает общий fallback contract.

**S3-16m bounded-memory partial:** `memory_dispatch::run` остаётся строгим и
по-прежнему возвращает `Unsupported` на bare `0x90`. Новый opt-in
`run_with_nop_fallback` reuses the strict dispatcher with the remaining total
budget, and steps only the decoder-confirmed exact bare byte `90` through
`od_isa::execute::step` and a one-byte immutable fetch view. It returns separate
native/interpreter retirements and checks their sum against consumed budget.
The synchronous host executor releases `PublishedCode` before returning, so
the interpreter step follows native mapping drop; next native publication also
proves the single mapping can be reused. Tests compare the complete CPU state,
RAM and data callback counts against the full interpreter for budgets 0..=5,
plus STOP/stop-RIP before and after NOP, continuation without replaying the
first pair, unsupported tail preservation and strict-mode rejection. Prefix
NOP, `0F 1F`, PAUSE and all other non-exact forms never enter fallback. This
does not add production owner integration or general Stage 2 fallback; S3-16
remains open.

### S3-17. Определить cache key и зависимости

1. Записать minimum relevant context key по разделу 4.3; включить все реальные compile-time assumptions и версии code/mappings.
2. Собрать зависимости blocks, пересекающих несколько regions/pages. Определить identity для aliases и повторного использования адреса памяти.
3. Проверить разные bytes/context при одинаковом RIP и одинаковые bytes при изменённой permissions/mapping identity.

**Готово:** stale или несовместимый block не считается hit. Выбор структуры поиска предварительно сравнен на representative key/workload; простой вариант допустим как измеренный baseline.

**Локальная partial реализация:** `od_jit::translation::TranslationKey`
требует typed compile flavor, entry RIP, полные bytes/base/end `CodeView`,
terminal classification, actual block instruction ranges и caller-provided
mapping/backing identity/version spans. Spans обязаны без дыр и overlap
покрывать весь CodeView; каждый span должен иметь execute permission, а версии
одной mapping/backing identity внутри snapshot должны совпадать. Разные mapping
identities могут alias один backing ID/generation. IDs и ненулевые версии
предоставляет владелец памяти; текущий `CodeView` не связан с реальным MMU и не
получает generation или identity из host pointer. Key использует полное
сравнение content при hash collisions. Full production cache/invalidation и
выбор структуры поиска не добавлены; отдельный one-entry S3-18a partial
описан ниже, а S3-17 остаётся открытой до измерения и оставшихся acceptance
checks.

**Benchmark prototype:** `tools/bench` содержит ARM64 macOS `record` wrapper
вокруг реальных `dispatch::run` block-entry вызовов и portable `measure`
replay. Replay заново строит keys вне timing и сравнивает `Vec` с
`std::collections::HashMap` при одинаковом полном key equality и
`dependencies_match_snapshot`. Отдельно предусмотрены cold construction+insert
и warmed lookup+dependency validation; сценарии включают recorded trace,
key-size ladder/observed count, hot loops, unique sequence, mixed hit/miss,
stale mapping/backing/permission, aliases и 1/2/4 dependency spans. Это только
прототип harness. На frozen corpus выполнены три release process runs; 18
сценариев и сырые результаты описаны в [отчёте измерений](../reports/STAGE_3_S3-17-lookup-measurements-2026-10-26.md).
Эти table-level результаты не являются end-to-end JIT speedup и пока не
выбрали production lookup/cache policy. S3-17 остаётся открытой для reviewer
acceptance и оставшихся cache-key/dependency требований. Ограничения и команды
описаны в [протоколе измерений](../testing/BENCHMARKS.md#s3-17-translation-key-lookup-prototype).

### S3-18. Реализовать ограниченный cache

1. Добавить lookup, compile miss, publish и reuse; проверить версии до вставки и входа в block.
2. Задать capacity по blocks/code bytes/metadata и явный путь при исчерпании. Не допустить неограниченного роста при уникальных RIP.
3. Считать hit/miss/compile/publication и bytes. До S3-25 освобождать только заведомо неактивные entries; активные сохранять до безопасной точки.

**Готово:** повтор программы использует block без новой compile/publication, capacity корректен, изменённый dependency не попадает в execution.

**S3-18a local partial:** `od_jit::dispatch::run_cached` adds one resident
fixture-dispatch block with an opaque published handle supplied by a generic
backend; `od-host` implements the backend without changing existing
release-on-return executor contracts. Same-key hits rebuild/validate the
bounded guest record and full `TranslationKey`, then skip lowering, allocation,
emission and publication. Miss replacement drops the old published owner
before publishing the candidate. The cache rejects capacities other than one
and caps logical key/code/state-map/publication-copy contents at 512 KiB;
allocator capacity, page rounding and host-handle internals are explicitly
outside that logical bound. Dependency/source validation occurs on hit, before
publish and before entry; a lower bound rejects oversized source/dependency
payloads before key copying. Stale input returns an error without retry. STOP
is invocation-local and resampled at guest boundaries; budget counters remain
per guest instruction. Mock tests cover same-key reuse, self-loop/budget, A→B replacement
and drop ordering, dependency changes, invalid snapshots, oversize candidates,
publication errors, clear/drop lifetime and zero-budget/code-end priority. A
native od-host fixture compares cached and uncached state/results and repeats a
cache hit. This S3-18a subsection describes only the one-entry policy and does
not integrate the separate host arena described below. One-entry hit, miss,
compile, publication and replacement counters are available for measurement;
S3-18c adds multi-entry hit/miss/compile/publication/eviction and resident-byte
counters. Memory lowering, production MMU
identity/invalidation, and benchmark evidence remain absent. S3-18 stays open;
S3-22/23/25 remain separate context, invalidation and retirement work.

**S3-18b host arena partial:** `od-host::PublishedArenaBackend` owns one
`MAP_JIT` mapping with a fixed slot size of 4096 bytes and a bounded capacity of
1..=16 blocks. Mapping length is checked and rounded using the runtime page
size. A preallocated free-slot stack tracks slots; each
`PublishedArenaBlock` handle retains the arena, slot generation and PC map.
The arena uses the same global `MapJitLease` as `PublishedCode`, so concurrent
standalone/another-arena publication returns typed `MappingAlreadyLive` until
the last backend or handle drops. Code publication validates the slot length
and metadata, temporarily enables current-thread JIT write access only for the
slot copy, restores protection, and performs instruction-cache invalidation
before exposing the handle. Slot exhaustion does not consume a slot; there is
no per-slot mmap/munmap, relocation or compaction. Arena calls reuse the shared
verified call/result decoder. Native tests cover two simultaneous live blocks,
full capacity, reuse of a dropped slot while another block remains executable,
foreign handles, owner/handle lifetime and checked size overflow. The 16-slot
limit is a provisional capacity choice without performance evidence. This host
publication API does not alter S3-18a's one-entry cache. S3-18 remains open.

**S3-18c multi-entry cache partial:** `od_jit::dispatch::MultiEntryBlockCache`
is a separate opt-in path using the shared cached-dispatch control loop and an
opaque `CachedBlockBackend` handle. `PublishedArenaBackend` reports slot
capacity through `MultiBlockCachedBackend`; the runner rejects a cache whose
configured entry count exceeds that capacity before compile or publication.
The cache uses insertion-order `Vec` lookup with FIFO eviction; hits do not
refresh order. Its hard limits are 1..=16 entries, 65,536 actual emitted code
bytes total, 512 KiB aggregate logical entry/key/PC-map metadata, and the
existing 4096-byte per-block compiler cap. Individual over-limit candidates
are rejected without eviction. A miss is compiled and source/dependency
validated before capacity eviction; inactive oldest handles are dropped before
publication. Publish failure leaves those evictions in effect. A stale check
after publication drops the candidate without execution and does not retry.
Counters expose hits, misses, successful compiles/publications, evictions and
resident code/metadata bytes. Tests cover A→B→A, self-loop/budget, sixteen-key
reuse, exact seventeenth-key FIFO victim, metadata-driven multi-eviction,
oversized candidates, malformed dependency rejection, stale post-publication
handle drop, publication failure, capacity
mismatch, and accounting bounds. A native od-host fixture compares cached
arena execution with uncached execution and verifies reuse. This remains
partial: no multi-entry benchmark conclusion, production MMU
identity/invalidation, memory lowering, or S3-22/23/25 lifecycle work is
claimed. S3-18 remains open.

Future measurement must compare the 16-key workload against FIFO/LRU and
end-to-end execution with repeat samples before choosing policy or capacity;
the current implementation makes no optimality or speedup claim.

**S3-18d1 implementation partial:** `od-bench dispatch-cache` adds an opt-in
native Apple Silicon end-to-end harness for uncached publication, one-entry
arena cache, FIFO(16), and LRU(16). It accepts only the exact frozen S3-17
case-list/trace fingerprints, checks full fixture-v1 decode admission and
recorded entry boundaries, and prepares the interpreter oracle before timing.
Deterministic scenarios cover the 1010-case cold sweep, two/four warmed corpus
repeats, hot 1/8/16 single-block sets, a recorded multi-block input, a 17-key
capacity edge, and hot8/hot16 followed by a distinct insertion. Variants share
the calibrated repetition count and therefore execute equal operation counts
in paired samples; each sample must accumulate at least 200 ms or the run
fails without a result. The JSON report records selected cases, 20 paired raw
samples, per-sample counters/high-water, arena reservation size, summaries and
paired ratios, and is written atomically. An opt-in LRU seam was added;
`MultiEntryBlockCache::new` still selects FIFO. This implementation has not run
the long comparison and makes no policy or speed claim. S3-18 remains open
pending independent review and three process-level measurements; S3-22/23/25
remain separate work.

### S3-19. Подключить checked memory bridge

1. Использовать существующий GuestMemory/AccessContext и подтверждённые fault outcomes этапа 2 через узкий generated helper ABI.
2. Различать fetch/read/write, address overflow/backend error и architectural fault; не добавлять альтернативную MMU в JIT.
3. На первом пути все memory operations checked. Direct host-memory fast path рассматривать только отдельно после проверки ownership, mappings и замеров.

**Готово:** helper success/fault/internal error сохраняют правильный state и exit reason, не обходят текущие ограничения CPU contract и ISSUE-012.

#### S3-19a. Invocation-local helper ABI partial

`od-jit::memory_bridge` добавляет invocation-local `CheckedMemoryAdapter` над
`&mut dyn GuestMemory` и отдельный fixed-width `CheckedMemoryHelperV1`. Его
status namespace — `0 Success`, `1 ArchitecturalFault`, `2 MemoryError`,
`3 InvalidArgument`; он не является `RuntimeExitTag` и не меняет
`RuntimeContextV1` (включая зарезервированный invalid tag 6). Error payload
передаётся только фиксированными C-полями: architectural vector/error code/
linear address либо уточнение boundary/backend ошибки; `MemoryError::Backend`
text не пересекает ABI.

Первый профиль разрешает fetch шириной 1 и integer read/write шириной 1/2/4/8.
Read destination меняется только после успешного backend read; write вызывает
один `GuestMemory::write`. Adapter не владеет CPU state/counters, не вычисляет
effective address и не выполняет lowering. Native ARM64 od-host test строит
probe через checked encoder, напрямую вызывает helper и проверяет success,
staged load candidate/retirement sentinel commit только при success и отдельный
backend-error status с сохранением этих sentinels при ошибке; unit tests bridge
проверяют один backend write.
`S3-19a` не реализует block exit/fault replay, production integration, MMU,
MMIO, RMW/atomic, REP, descriptor accesses, direct host-memory access или
lowering; полный S3-19 и S3-20 остаются открытыми.

#### S3-19b / S3-20a. Один production V2 MOV load

Отдельный `compile_memory_load_block_v2` использует единственный источник
decoded semantics — `od_isa::long_decode` — и выдаёт приватный compiler-issued
V2 artifact только для exact `48 8B /r`, ModRM `mod=00`, destination/base
RAX..RBX. Он не допускает SIB, displacement, segment prefix или address-size
override и останавливает native block после этого одного load. В ARM64-коде
нет вычисления effective address: код передаёт site index доверенному Rust
helper, который вызывает `MemoryOperand::access_address_48`, затем
`CheckedMemoryHelperV1`.

V2 имеет отдельные `RuntimeContextV2`/`BlockEntryV2`, compile flavor и host
publish/execute APIs; V1 entry остаётся отдельным и отклоняет V2 до pointer
cast, tag 6 в V1 остаётся invalid. Успешный read сначала записывается в staging,
после чего одним commit меняются destination, RIP и retirement; fault и
boundary/backend errors сохраняют canonical state на faulting RIP и
различаются по результату host API. `RAX` alias и non-alias формы проверены на
ARM64 native path вместе с fault payload, boundary/backend separation,
budget/STOP/stop-RIP. Native success, architectural-fault preservation и
boundary error сравниваются с `od_isa::execute::step` на независимых copies
CPU/RAM, включая полный CPU state, RAM bytes и read count. Этот путь не использует общий IR/allocator, не имеет
PC-map, cache или multiblock integration; source bytes/decoded site/compiler
provenance остаются частью отдельного V2 artifact. Memory pointers и RAM
contents invocation-local и не входят в translation key.

#### S3-20b. Один production V2 MOV store

`compile_memory_store_block_v2` принимает ровно одну инструкцию `48 89 /r`,
ModRM `mod=00`, source/base RAX..RBX, qword; SIB, displacement и любые
дополнительные байты/prefixes отклоняются. Store использует тот же decoded
site/helper/emitter family, отдельные source/destination metadata, compile
flavor, artifact/publication type и host entry. Load entry отклоняет store
artifact до native call.

Store доступен только через unsafe `execute_memory_store_v2`: caller обязан
предоставить private synchronous ordinary RAM с all-or-nothing write, без MMIO,
DMA, конкурентных или отложенных эффектов; backing не исполняется и не
алиасит cached guest code, и не становится executable до drop compiled block.
Вызов helper делает один checked write после захвата address/source; ошибка не
меняет RAM или canonical CPU state, success затем коммитит RIP/retirement и
только после этого читает STOP. `MOV [RAX],RAX`, little-endian bytes,
interpreter differential success/fault/backend-boundary cases и поздний STOP
проверены локально на независимых snapshots; entry STOP/stop-RIP/zero-budget
guards возвращаются без записи. Code writes не допускаются до
S3-23. Это не общий memory lowering и не cache/dispatch integration.

Stores beyond this exact isolated slice, other address forms, general memory
lowering and dispatch/cache integration remain open. Локальные Rust/ARM64 проверки разрешены;
физические Mac mini/x86 проверки не запускать до финального S3-21 gate после
завершения остальных локальных задач Stage 3.

#### S3-20c. Один SIB byte для V2 MOV load/store

Обе production V2 qword операции принимают ровно четырёхбайтовую форму
`48 8B/89 /r sib`: ModRM `mod=00,r/m=100`, data/base/index RAX..RBX,
scale 1/2/4/8, без displacement, лишнего prefix или хвостовых байтов. Index
`100` (no-index), base `101` (no-base/disp32), RSP/RBP и R8+ формы запрещены.
Существующие direct-base трёхбайтовые encoding остаются допустимы.

`long_decode::decode` остаётся единственным декодером; компилятор сверяет raw
ModRM/SIB биты с decoded length/operation/register/base/index/scale, но не
вычисляет effective address. Source identity хранит bounded `[u8; 4]` плюс
length, а site использует существующий `MemoryOperand::access_address_48` с
wrapping и canonical48 правилами. ABI, V2 tag, load/store publication/API,
store safety и commit ordering не меняются.

Локальные ARM64 native differential tests сравнивают все 256 валидных
data-register/base/index/scale комбинаций отдельно для load и store с
`od_isa::execute::step` на независимых полных CPU/RAM snapshots. Проверены
little-endian store bytes, aliases, wrap-to-canonical, noncanonical guest fault,
backend boundary error и отсутствие эффектов при STOP/stop-RIP/нулевом budget.
Invalid/truncated/tail/displacement/no-index/no-base/extended/prefix формы
отвергаются. Срез описан в [S3-20c report](../reports/STAGE_3_S3-20c_2026-10-27.md).
Физическая проверка Mac mini остаётся отложенной до финального S3-21 gate.

#### S3-20d. Signed displacement в direct и SIB V2 MOV

Обе qword операции `48 8B/89 /r` допускают ModRM `mod=00`, signed disp8 при
`mod=01` и signed disp32 при `mod=10`, с существующей direct-base или
однобайтовой SIB формой. Точные длины без SIB: 3/4/7 bytes; с SIB: 4/5/8
bytes. Base, index и data register ограничены RAX..RBX; scale SIB — 1/2/4/8.
Mod11, RIP-relative, base-less, no-index, RSP/RBP/R8+, любые иные prefixes,
address-size override и trailing bytes отвергаются.

`long_decode::decode` остаётся единственным декодером и задаёт `MemoryOperand`,
signed displacement и `next_rip`. Immutable artifact identity хранит полный
bounded source `[u8; 8]` и длину. Общий `MemorySiteV2` validator проверяет
decoded site shape и согласованность raw ModRM/SIB/source length, не вычисляя
EA и не дублируя sign extension. Runtime использует прежний
`MemoryOperand::access_address_48` для wrapping, canonical48 и access-range
правил. ABI, V1, checked helper, load/store safety и commit ordering прежние.

Локальные native differential tests перебирают допустимые base/index/data/scale
и mod формы для load/store, включая disp8 `-128/-1/0/1/127` и disp32 min/max
с соседями. Проверяются одинаковые адреса у эквивалентных disp8/disp32 при
разных `next_rip`, aliases, wrap-to-canonical, final noncanonical и endpoint
faults, boundary/permission/backend errors, no-effect guards и поздний STOP
после одного store. Результат описан в
[S3-20d report](../reports/STAGE_3_S3-20d_2026-10-27.md). Mac mini gate остаётся
только финальным S3-21.

#### S3-20e. RIP-relative V2 MOV

Добавлен узкий RIP-relative профиль для qword `48 8B/89 /r`: ModRM
`mod=00,r/m=101`, data register RAX..RBX, signed disp32 и ровно семь байтов
инструкции. `long_decode::decode` остаётся единственным декодером; immutable
site связывает все source bytes, decoded `MemoryOperand` и `next_rip`.
Runtime принимает только state с `RIP == site.guest_rip` и вызывает
`MemoryOperand::access_address_48(state, site.next_rip)`; emitter не вычисляет
адрес и host path не перебазирует instruction.

Compiler coverage проверяет обе операции, все четыре data registers и
`i32::MIN/-8/-7/-1/0/1/8/i32::MAX`, включая site после двух байтов внутри
CodeView. Native/interpreter differential проверяет те же байты на двух
guest RIP для всех пар operation/register/displacement, сравнивая полный CPU
state и RAM; случаи вне fixture RAM или с store в executable code не коммитят
изменения. Поддержка не распространяется на другие RIP формы, prefixes,
trailing bytes или общую IR/cache интеграцию. Результат и фактические проверки
записаны в [S3-20e report](../reports/STAGE_3_S3-20e_2026-10-27.md). Mac mini
проверка остаётся только финальным S3-21 gate после локальной работы Stage 3.

#### S3-20f. Address-size override V2 MOV

Отдельная admission ветка принимает только `67 48 8B/89 /r`: ModRM
`mod=00`, RAX..RBX в data и base полях, qword data, dword address size,
без index/SIB/displacement, длина ровно четыре байта. `long_decode::decode`
остаётся единственным decoder; runtime вызывает
`MemoryOperand::access_address_48` и не дублирует zero extension или mask.
Старые direct/SIB/displacement/RIP profiles не расширены этой веткой.

Local native/interpreter differential покрывает 16 data/base сочетаний для
обоих направлений, включая верхние биты базовых GPR, полные qword значения,
`MOV RAX,[EAX]` и sparse successful qword access from `0xffffffff` through
`0x100000006`. Dword address-size truncation applies to the starting effective
address only; operand byte addresses then advance linearly without 32-bit
wrap. A separate unmapped case checks fault atomicity. Truncated, trailing,
prefix, mod01/mod10/mod11, SIB и base-less формы
отвергаются. Отчёт: [S3-20f](../reports/STAGE_3_S3-20f_2026-10-27.md). Mac mini
проверка остаётся финальным S3-21 gate.

#### S3-20g. Signed displacement при address-size override

V2 load/store дополнительно принимают `67 48 8B/89 /r disp8|disp32` с
ModRM `mod=01/10`; data/base RAX..RBX, длины ровно 5/8 байт. Существующий
`mod=00` остаётся доступен. SIB, RIP-relative/base-less форма, другие
префиксы и регистры остаются запрещены. Decoder по-прежнему единственный
источник `MemoryOperand`, signed displacement и `next_rip`; raw-byte admission
сверяет только encoding и decoded metadata. Lowering/helper не вычисляют EA и
не повторяют sign extension: адрес и полная qword-проверка идут через
`MemoryOperand::access_address_48`.

Локальные native/interpreter differential cases покрывают disp8
`-128/-1/0/1/127`, границы disp32 и соседние значения, верхние биты base,
32-bit EA wrap и alias destination/source с base. Для qword от `0xffff_fffc`
проверено линейное продолжение до `0x1_0000_0003`; отсутствующий последний
байт сохраняет CPU/RAM для load и store. Truncated/trailing bytes, SIB,
неподдержанные регистры и дополнительные/переставленные prefixes отклоняются.
Отчёт: [S3-20g](../reports/STAGE_3_S3-20g_2026-10-27.md). Проверка Mac mini
остаётся частью финального S3-21 gate.

### S3-20. Добавить memory lowering

1. Начать с проверенных S2 MOV load/store forms, затем подключать остальные формы по готовности interpreter/native.
2. Правильно вычислять guest effective address, widths, partial-register writes и порядок accesses. Не повторять decoder semantics вручную в emitter.
3. Сохранить commit ordering: fault не оставляет запрещённую запись в register/flags/RIP, successful store уведомляет memory owner о side effect.

**Готово:** memory fixtures с boundary/unaligned/alias cases совпадают локально с interpreter; до S3-21 это не физическая приёмка памяти.

#### S3-20h / S3-26a. Один checked qword load через общий IR runtime path

Отдельный opt-in builder допускает ровно `48 8B /r`, ModRM `mod=00`, data и
base RAX..RBX, без SIB/displacement. `CheckedLoad` — typed IR effect с
qword `MemoryOperand` и success-only temporary; verifier связывает его с
единственной текущей instruction, валидным descriptor, `WriteGpr` этого
temporary и commit после write. Lowering получает операнд только из decoder.

Allocator видит реальную helper boundary, планирует spill/capture и restore;
общий runtime emitter вызывает существующий checked-memory helper по site
index 0, оставляет результат staged до GPR/RIP/counter commit и строит общую
PC map с helper-call и commit границами. Новый compile flavor, compiler-issued
artifact, опубликованная metadata и `RuntimeContextV2` entry имеют отдельные
типы. V1 executor и прямой V2 load API отвергают этот artifact до native
entry. Существующий прямой V2 compiler остаётся самостоятельным baseline.

Локальный native ARM64 test сравнивает alias load `[RAX] → RAX` с прямым V2 и
interpreter по полному CPU state и памяти. Также проверены qword success,
retirement, architectural fault с неизменным destination/flags/RIP/state,
backend error, STOP, stop-RIP, zero budget, verifier negatives, и границы
helper/commit в PC map. Это один bounded fixture slice: нет общего multiblock
memory lowering, stores, MMIO, atomic операций или автоматического host fault
recovery. См. [S3-20h/S3-26a report](../reports/STAGE_3_S3-20h_2026-10-27.md).

#### S3-20i / S3-26b. Один checked qword store через общий IR runtime path

Отдельный opt-in builder допускает ровно `48 89 /r`, ModRM `mod=00`,
source/base RAX..RBX, без SIB/displacement. `CheckedStore` явно хранит guest
source GPR и decoder `MemoryOperand`; verifier требует единственный store и
следующий непосредственно за ним commit, отвергая дублирование и эффекты до
commit. У store отдельные compile flavor, artifact, публикация и invocation
entry. V1, direct V2 load, IR load и direct V2 store APIs отвергают этот
artifact до native entry.

Generated entry исполняет stop-RIP → STOP → budget guards, один checked helper
call, затем сразу фиксирует RIP/budget/retirement; лишь после полного commit
читает STOP. Store, завершённый при поднятии STOP внутри callback, сохраняется
и возвращает `StopRequested`. Helper errors сохраняют CPU/RAM и не retire-ят.
Host entry сохраняет те же unsafe private synchronous ordinary-RAM
предпосылки, что direct V2 store.

PC map помечает helper call и путь успешного возврата к commit как
`InadmissibleAfterEffect`, поэтому неизвестный host fault не может быть
превращён в replay guest store. Ошибка helper остаётся planned/replayable,
поскольку helper не меняет RAM при ошибке. Native ARM64 fixture сравнивает IR,
direct V2 и interpreter на независимых snapshots `MOV [RAX], RAX`, проверяет
ровно один callback/write, commit/retirement, STOP после write, architectural и
backend errors, pre-entry guards, verifier negatives, PC-map и typed-entry
rejection. Ограничения: одна инструкция, private ordinary data RAM; без MMIO,
DMA, concurrency, atomics, executable aliases, active SMC, multiblock порядка
или performance claim. Факты записаны в
[S3-20i/S3-26b report](../reports/STAGE_3_S3-20i_2026-10-27.md).

#### S3-20j / S3-26c / S3-20k / S3-26d. Один generated entry для двух точных memory sequences

Отдельный opt-in artifact допускает ровно две sequences: `48 89 03 48 8B 0B`
(`MOV [RBX], RAX; MOV RCX, [RBX]`) и `48 8B 03 48 89 02`
(`MOV RAX, [RBX]; MOV [RDX], RAX`). Builder/lowering сохраняют обе instruction
records в одном `BasicBlock`/IR; verifier связывает каждую memory operation,
GPR write и commit с активной instruction. Typed artifact хранит выбранную
sequence и `[site0, site1]`; общий emitter проверяет и исполняет ровно одну из
этих форм. Single-site helper/publication/execution APIs сохраняют проверки
index 0 и отвергают двух-site artifacts.

Один native entry запускает оба site последовательно и допускает повторный
вход только на RIP второй инструкции. Store→load сохраняет первый store при
ошибке load; load→store сохраняет RAX после load при ошибке store. Второй site
может fault и на continuation с нулём retired в этом вызове, не повторяя первый
site. Guards второго instruction имеют порядок stop-RIP → STOP → budget.
Helper/fault/commit PC map ranges сохраняют site и guest instruction; store
helper/commit остаётся non-replayable.

Локальная native ARM64 fixture сравнивает state/RAM с двумя interpreter steps
на независимых copies и проверяет callback order, fault payload/partial commit,
STOP, budget, continuation после изменения source RAM, guards и typed entry.
Slice остаётся private synchronous ordinary RAM без branch, code aliases, MMIO,
atomics, произвольных multi-site blocks или performance claim. См.
[S3-20j/S3-26c/S3-20k/S3-26d report](../reports/STAGE_3_S3-20j_2026-10-27.md).

#### S3-20l. RIP-relative MOV in shared IR and bounded dispatch

The existing decoded `MemoryOperand` is carried unchanged through
`CheckedLoad`/`CheckedStore`; effective-address semantics continue to come from
`access_address_48` at that instruction's `next_rip`. The opt-in singleton
profile accepts only exact seven-byte `48 8B/89 /r disp32`, ModRM
`mod=00,r/m=101`, qword data register RAX..RBX. Shared-IR verification, typed
host publication/execution validation, and `memory_dispatch::compile_at` admit
the same shape. A truncated explicit byte window returns `BoundaryReached`
before publication or a data-memory callback. Existing direct-base and pair
profiles retain their exact admission.

Native ARM64 tests cover direct V2/shared IR/interpreter parity for negative,
zero and positive displacement and a CodeView entry after a prefix. The
dispatcher composes this singleton, the existing bare-NOP interpreter step,
and an existing two-site pair in both orders; it checks budgets 0..4,
actual-RIP continuation, and 1..6-byte partial windows. Shared-IR load/store
also check architectural and backend errors plus a committed STOP-after-store.
This remains a bounded fixture, not an
`od-machine`/MMU owner integration or a performance claim. See the
[S3-20l report](../reports/STAGE_3_S3-20l_2026-10-27.md).

#### S3-20m. Direct-base signed displacement through shared IR and dispatch

Shared checked singletons additionally admit only `48 8B/89 /r disp8` with
ModRM `mod=01` and exact length four, or `48 8B/89 /r disp32` with ModRM
`mod=10` and exact length seven. Base and data/source register are independently
bounded to RAX..RBX; SIB, `67`, any extra prefix, register form and trailing
bytes remain rejected. The decoder-provided signed `MemoryOperand` remains the
only effective-address source, and the existing checked-memory helper performs
the canonical, overflow and access checks.

Direct V2, shared IR, interpreter, and bounded dispatcher tests cover disp8
extrema and disp32 extrema/wraparound, aliasing load destination/base and store
source/base, noncanonical starts/ends, backend failures, successful STOP after
store, and partial byte windows. Mixed dispatch composes a new disp8 singleton
with the existing NOP fallback and pair. No pair or unsafe data-RAM contract
changes. See the [S3-20m report](../reports/STAGE_3_S3-20m_2026-10-27.md).

#### S3-20n. No-displacement SIB MOV in shared IR and bounded dispatch

Shared checked singleton compilation additionally accepts exactly four-byte
`48 8B/89 /r sib`: ModRM `mod=00,r/m=100`, data/base/index encodings
RAX..RBX, qword operand/address sizes, and SIB scale 1/2/4/8. SIB index
`100` (no index), base `101` (no base), other base/index encodings,
displacement forms, `67`, other prefixes and trailing bytes remain rejected.
The `MemorySiteV2` source validator binds ModRM/SIB to decoded operation,
registers, base/index and scale. IR verification also requires the four-byte
instruction range. Direct-base disp8 remains separately admitted by ModRM
`mod=01`, so its four-byte length cannot be confused with SIB `mod=00`.

The decoder-provided `MemoryOperand` and existing `access_address_48` remain
the only effective-address semantics; no SIB address calculation was added.
Native direct V2/shared-IR/interpreter tests enumerate both operations, all
data/base/index registers and scales, including aliases and a zero-valued
index. SIB-derived cases cover multiply/add wrapping, noncanonical start/end,
backend errors and a committed store followed by STOP. Bounded dispatch
composes SIB singleton → bare NOP interpreter fallback → existing pair across
budgets 0..4 and partial pre-SIB byte windows. Pair admissions and the unsafe
private synchronous RAM contract are unchanged. See the
[S3-20n report](../reports/STAGE_3_S3-20n_2026-10-27.md).

#### S3-20o. Address32 direct-base qword MOV in shared IR and bounded dispatch

The shared checked singleton additionally accepts only exact
`67 48 8B/89 /r`, ModRM `mod=00`, data/base RAX..RBX, instruction length
four. The descriptor retains the architectural distinction between qword
operand width and dword effective-address width. No SIB, displacement,
extended register, reordered/extra prefix, or trailing byte is accepted. The
raw-byte check is a separate branch from four-byte SIB (`48` prefix) and
direct disp8 (`mod=01`) forms. The decoder-provided `MemoryOperand` and
`access_address_48` remain the sole address semantics; no mask, zero-extension,
or qword access splitting was added.

Native direct V2/shared IR/interpreter cases cover load/store and every data
and base register pair, including source/destination aliasing the base and
noncanonical u64 base values whose low dword is used. A qword at
`0xffff_fffc` uses one contiguous callback through `0x1_0000_0003`. Exact
architectural faults preserve CPU/RAM/budget/retirement, backend errors remain
host errors, guards skip access, and a successful store remains committed
when STOP follows. Bounded dispatch composes the singleton → bare NOP
interpreter fallback → existing pair with budgets 0..4, STOP and continuation
from actual RIP. See the
[S3-20o report](../reports/STAGE_3_S3-20o_2026-10-27.md).

#### S3-20p. Address32 signed displacement through shared IR and bounded dispatch

The shared checked singleton additionally admits only exact
`67 48 8B/89 /r disp8|disp32` with ModRM `mod=01/10`, data/base RAX..RBX,
and exact lengths five/eight. The descriptor retains qword data width and
dword address width plus the decoder-provided signed displacement. Existing
`MemoryOperand::access_address_48` remains the only EA and access-range
semantics; admission checks the encoding and displacement range but does not
re-sign-extend or compute an address. No SIB, RIP-relative form,
extra/reordered prefixes, wider register, or multi-instruction profile is
added; pair admissions remain unchanged.

Native direct V2/shared IR/interpreter cases compare disp8 -128/-1/0/1/127
and disp32 MIN/MIN+1/-1/0/1/MAX-1/MAX for load and store, with destination or
source aliasing the base. They verify high base bits are ignored, signed
addition wraps modulo 2^32, and a qword at `0xffff_fffc` uses one contiguous
8-byte callback through `0x1_0000_0003`. IR architectural faults preserve exact
payload and CPU/RAM/budget/retirement; backend failures remain host errors;
successful store+STOP remains committed. Bounded dispatch composes disp8
singleton → bare NOP fallback → existing pair for budgets 0..4 and resumes
from the actual RIP; partial disp8/disp32 windows stop without effects. See
the [S3-20p report](../reports/STAGE_3_S3-20p_2026-10-27.md).

#### S3-20q. 64-bit SIB displacement through shared IR and bounded dispatch

The shared checked singleton additionally admits exactly
`48 8B/89 /r sib disp8|disp32`: ModRM `mod=01/10,r/m=100`; RAX..RBX data,
base, and index; SIB scale 1/2/4/8; and source lengths five/eight. It rejects
no-index/no-base encodings, `67`, extended registers, and additional prefixes.
The SIB-with-displacement source path is distinct from address32 direct-base
displacements and direct-base 64-bit displacement forms; length alone does
not select an addressing mode. Descriptor displacement and operand fields
come from the decoder, and `MemoryOperand::access_address_48` remains the only
effective-address, canonicality, and full qword-range implementation.

Native direct V2/shared IR/interpreter parity covers disp8 -128/-1/0/127 and
disp32 MIN/-1/0/MAX across all scales and both MOV directions. Additional
cases alias data with base/index and base with index. Callback address and
length checks verify one qword operation, including signed arithmetic that
wraps into the canonical target. Architectural fault payloads preserve CPU,
RAM, budget, and retirement across all three execution paths; backend errors
remain host errors, and SIB store+STOP retains its commit. Bounded dispatch
composes the disp8 singleton → bare NOP fallback → existing pair across
budgets 0..4, resumes from the actual RIP after STOP, and rejects invalid SIB
fields without a helper call. Partial disp8/disp32 windows stop before
publication or data access. Pair admissions remain unchanged. See the
[S3-20q report](../reports/STAGE_3_S3-20q_2026-10-27.md).

#### S3-20r. Address32 no-displacement SIB through shared IR and bounded dispatch

The shared checked singleton admits exactly `67 48 8B/89 /r sib` with ModRM
`mod=00,r/m=100`, RAX..RBX data/base/index registers, scale 1/2/4/8, qword
data, dword address size and exact length five. The raw source validator
distinguishes it from address32 direct-base disp8 (ModRM `mod=01`), ordinary
64-bit SIB (no `67` prefix), and displacement-bearing SIB forms. The decoder's
`MemoryOperand` and `access_address_48` remain the only address semantics; no
EA computation was added. No-index/no-base specials, RSP/RBP or extended
register encodings, address32 SIB with displacement, extra prefixes and
trailing bytes remain unsupported. The 9-byte addr32 SIB disp32 form is
outside the fixed eight-byte site storage and is rejected. Pair admissions
remain unchanged.

Descriptor and native direct V2/shared IR/interpreter checks cover both MOV
directions, every supported data/base/index register and scale, aliases,
ignored high GPR halves, and wrapping modulo 2^32. Callback evidence includes
a qword beginning at `0xffff_fffc`, proving the qword access continues linearly
past 4 GiB. Fault payloads preserve state/RAM/budget/retirement, backend errors
remain host errors, and successful store+STOP retains its commit. Bounded
dispatch composes addr32 SIB → bare NOP fallback → existing pair at budgets
0..4, checks no-effect guards and continuation from actual RIP, and rejects
partial windows and forbidden fields before memory access. See the
[S3-20r report](../reports/STAGE_3_S3-20r_2026-10-27.md).

#### S3-20s. Address32 signed disp8 SIB through shared IR and bounded dispatch

The checked singleton additionally admits exactly `67 48 8B/89 /r sib disp8`,
ModRM `mod=01,r/m=100`, RAX..RBX data/base/index, SIB scale 1/2/4/8, qword
data, dword address size and exact length six. The site stores at most eight
source bytes; address32 SIB disp32 remains rejected because it is nine bytes.
The decoder's `MemoryOperand` and `access_address_48` remain the sole
displacement/effective-address and qword-range semantics. No prefix variants,
special SIB fields, pair changes, or alternate encodings are admitted.

Descriptor and direct V2/shared IR/interpreter cases cover disp8
`-128/-1/0/127`, all scales and supported register aliases, high GPR halves,
modulo-2^32 base/index/displacement arithmetic, and contiguous qword access
across `0xffff_ffff`. Fault payloads and no-commit behavior, backend errors,
and store+STOP commit are checked. Bounded dispatch composes the six-byte
singleton → bare NOP fallback → existing pair across budgets 0..4; a six-byte
singleton boundary proves the form is not mistaken for a two-instruction pair
by length. Continuation resumes at actual RIP without repeating effects.
Partial byte windows and forbidden encodings stop before helper access. See
the [S3-20s report](../reports/STAGE_3_S3-20s_2026-10-27.md).

#### S3-20t. Address32 signed disp32 SIB through shared IR and bounded dispatch

The checked singleton admits exactly `67 48 8B/89 /r sib disp32`, ModRM
`mod=10,r/m=100`, RAX..RBX data/base/index, SIB scale 1/2/4/8, qword data,
dword address size and exact length nine. Singleton source metadata capacity
is now the named `MEMORY_SITE_SOURCE_BYTES_MAX = 9`; the Rust return type of
`source_bytes_storage()` changes from `([u8; 8], u8)` to `([u8; 9], u8)`.
This capacity applies only to singleton source identity. Qword data staging,
two-site source `[u8; 6]`, ActiveCodeV3 source `[u8; 23]`, and RuntimeContext/C
ABI layouts are unchanged. Validate the decoded length before slicing or
copying all nine bytes. The decoder's `MemoryOperand` and
`access_address_48` remain the only displacement and effective-address source.

Descriptor and direct V2/shared IR/interpreter tests cover disp32 MIN/-1/0/MAX
for both operations, all scales and the RAX address/data alias, including the
contiguous qword callback at `0xffff_fffc`. A pair of code sources differs only
in byte nine and retains distinct displacement metadata. Architectural fault
payloads preserve CPU/RAM/budget/retirement; backend errors remain host errors;
successful store+STOP retains its write. Bounded dispatch composes the
nine-byte singleton → bare NOP interpreter bridge → existing pair, checks
budgets 0..4, stop-RIP, callback STOP, and continuation from actual RIP without
repeating the first access. Windows 1..8 and malformed encodings are rejected
before a helper callback. See the [S3-20t report](../reports/STAGE_3_S3-20t_2026-10-27.md).

#### S3-20u. RBP-base signed disp8 through shared IR and bounded dispatch

Admit only the singleton `48 8B/89 /r disp8`: ModRM `mod=01,r/m=101`,
RBP base, data register RAX..RBX, qword data, 64-bit effective address and
exactly four source bytes. Decoder `MemoryOperand::access_address_48` remains
the sole displacement and address-range implementation. RSP/SIB, address-size
override, disp32, segment or extra prefixes, extended registers, and pairs are
outside this slice.

Direct V2/shared IR/interpreter checks cover both operations and disp8
`-128/-1/0/127`, exact callback address/count, complete state/RAM parity, and
unchanged RBP/RSP. A noncanonical RBP start and an access whose last byte is
noncanonical both produce #SS (vector 12, error 0, no linear address); the
same checks with RBX produce #GP. Faults make no data callback and preserve
state/budget/retirement. AddressOverflow remains its host diagnostic, backend
errors remain host errors, entry STOP/budget zero make no access, and a store
that requests STOP after writing remains committed. Bounded dispatch composes
the singleton → bare NOP interpreter bridge → existing pair, matches
interpreter for budgets 0..4, resumes from the actual RIP without replay, and
keeps RBP/RSP unchanged. See the [S3-20u report](../reports/STAGE_3_S3-20u_2026-10-27.md).

#### S3-20v. RSP-base no-index SIB through shared IR and bounded dispatch

Admit only singleton `48 8B/89 /r sib` with ModRM `mod=00,r/m=100` and exact
SIB byte `0x24`: RSP base, no index, scale 1, no displacement, data RAX..RBX,
qword data, 64-bit effective address and exact four-byte length. The decoder's
`MemoryOperand` and `access_address_48` remain the sole address semantics; host
SP and frame ABI are unchanged. Other SIB fields, displacement modes, RSP as a
data register, `67`, other prefixes, extended registers, and pair changes are
excluded.

Direct V2/shared IR/interpreter load/store parity checks compare full state,
RAM, callback address/count and retirement, including unchanged RSP/RBP/flags.
Noncanonical RSP start and last-byte ranges produce #SS; identical RBX controls
produce #GP, before data access and with no architectural commit. AddressOverflow
and backend failures remain distinct host errors; entry STOP/zero budget do not
access memory and store+STOP retains its write. Bounded dispatch exercises the
four-byte singleton → bare NOP → existing pair, budgets 0..4, actual-RIP
continuation and partial byte windows. Invalid near-matches return Unsupported
without publishing a block or calling memory. See the
[S3-20v report](../reports/STAGE_3_S3-20v_2026-10-27.md).

#### S3-20w. RSP-base no-index SIB with signed disp8

Extend only the singleton forms `48 8B/89 /r sib disp8`, ModRM
`mod=01,r/m=100`, exact SIB `0x24`, data RAX..RBX, qword data and 64-bit EA.
The decoded `MemoryOperand` and `access_address_48` remain the sole source of
signed displacement and effective-address semantics. No disp32, indexed RSP,
address-size/segment/extra prefix, extended register, pair/V3, or emitter change
is included; the existing `[RSP]` form remains separately admitted.

Evidence covers disp8 -128/-1/0/127 for load/store across direct V2, shared IR
and interpreter, including callback address/count and full architectural/RAM
parity. RSP/RBP/flags stay unchanged. The shared fault matrix retains RSP #SS
versus RBX #GP, AddressOverflow versus backend errors, no-commit faults and
committed store+STOP. Bounded dispatch composes the five-byte singleton → bare
NOP interpreter bridge → existing pair across budgets 0..4, STOP-RIP and
actual-RIP continuation without replay; truncated windows and invalid shapes
have no memory effects. See the
[S3-20w report](../reports/STAGE_3_S3-20w_2026-10-27.md).

#### S3-20x. RSP-base no-index SIB with signed disp32

Extend only singleton `48 8B/89 /r sib disp32` with ModRM
`mod=10,r/m=100`, exact SIB `0x24`, RSP base/no index, data RAX..RBX,
qword data, 64-bit EA and length eight. The four displacement bytes begin
after the SIB; decoded `MemoryOperand` and `access_address_48` own signed
interpretation and address calculation. The source metadata capacity already
holds the eight-byte form; no data-buffer, pair, V3, or ABI change is needed.
Address32, other prefixes, indexed/special SIB forms, pair/V3 changes and
additional emitters remain excluded. Existing `[RSP]` and RSP+disp8 forms stay
separately covered.

Direct V2/shared IR/interpreter tests cover both operations, disp32
MIN/-1/0/MAX, contiguous qword access across 4 GiB and signed address wrap to
zero. Sources differing only in the high displacement byte produce distinct
expected callback targets. Fault tests check RSP #SS versus RBX #GP, diagnostic
AddressOverflow, backend error, no commit and successful store+STOP. Bounded
dispatch composes the eight-byte singleton → NOP → pair across budgets 0..4,
STOP and actual-RIP continuation; partial windows 1..7 and invalid forms make
no effects. See the
[S3-20x report](../reports/STAGE_3_S3-20x_2026-10-27.md).

#### S3-20y. RBP-base signed disp32 qword singleton

Extend only `48 8B/89 /r disp32`, ModRM `mod=10,r/m=101`, RBP base,
data RAX..RBX, qword data, 64-bit EA and exact length seven. The decoder's
`MemoryOperand` and `access_address_48` remain the sole source of signed
displacement and effective-address semantics. Keep `mod=00,r/m=101` as the
existing instruction-local RIP-relative form; no SIB, address override,
other prefix/base or pair/V3 changes are included.

Direct V2/shared IR/interpreter evidence covers both directions, disp32
MIN/-1/0/127/128/MAX, callback address/count and architectural/RAM parity.
The same displacement through RBP and RIP-relative encoding must resolve to
their distinct expected targets. Preserve RBP #SS versus RBX #GP at the
noncanonical start/end, diagnostic AddressOverflow and backend error
classification, fault atomicity, STOP/budget guards, and committed store+STOP.
Bounded dispatch composes RBP disp32 → bare NOP → existing pair across budgets
0..4, STOP-RIP and actual-RIP continuation without replay; windows 1..6 make
no effects. See the [S3-20y report](../reports/STAGE_3_S3-20y_2026-10-27.md).

#### S3-20ab. RBP as both qword MOV data and disp8 base

Admit only exact singleton `48 8B/89 6D ib`: ModRM `mod=01`, `reg=RBP`,
`r/m=RBP`, signed disp8, qword data, 64-bit effective address and four source
bytes. The decoded `MemoryOperand` and `access_address_48` remain the only
displacement/address implementation. No disp32, SIB, address-size or other
prefix, other data/base combinations, pairs, or frame/ABI changes are included;
existing RBP-data-only, RBP-base-only and RSP-data profiles remain supported.

Direct V2/shared IR/interpreter tests cover load/store at disp8 -128/-1/0/127.
The effective address and store payload use entry RBP before destination
commit; a load replaces RBP with a different value. A noncanonical current
access faults #SS before callback and leaves state/RAM/budget/retirement
unchanged. Bounded dispatch splits after a successful RBP-relative load,
resumes at the actual next RIP without repeating the read, and faults on a
subsequent RBP-based access while retaining the first commit. An ARM64/macOS
x19 ABI canary covers this alias profile's success, budget-guard and fault
returns; frame layout is unchanged. See the
[S3-20ab report](../reports/STAGE_3_S3-20ab_2026-10-27.md).

#### S3-20ac. RSP as both qword MOV data and no-index SIB base

Admit only exact singleton `48 8B/89 24 24`: ModRM `mod=00/rm=100`, data
register RSP, SIB exactly `0x24`, RSP base/no index/scale 1, no displacement,
qword data, 64-bit effective address and exact length four. The decoder's
`MemoryOperand` and `access_address_48` remain the sole address authority.
Other SIB values, displacement, index, prefix, data/base combinations, pairs,
and frame/ABI changes remain excluded; prior RSP data-only and RSP-base
RAX..RBX forms remain supported.

Direct V2/shared IR/interpreter checks compare full state/RAM, callback
address/count and retirement for load/store. Address and store payload use
entry RSP; a successful load commits a distinct new value. The current
noncanonical `[RSP]` access faults #SS before a data callback and commits
nothing. An ARM64/macOS x19 canary covers this exact profile's success, budget
guard and fault returns. Bounded dispatch pauses after the load on both budget
and callback STOP, resumes at the actual next RIP, then faults on a following
`[RSP]` access without repeating the first read. Invalid SIB and partial forms
stop before publication/helper calls. See the
[S3-20ac report](../reports/STAGE_3_S3-20ac_2026-10-27.md).

#### S3-20ad. RSP as both qword MOV data and signed-disp8 SIB base

Admit only exact singleton `48 8B/89 64 24 ib`: ModRM `mod=01/reg=4/rm=4`,
SIB exactly `0x24` (RSP base, no index, scale 1), signed disp8, qword data,
64-bit effective address and exact length five. The existing decoded
`MemoryOperand` and `access_address_48` remain the only address authority.
Generic GPR validation, frame/ABI, pair/V3 and other prefix/address forms do
not change.

Direct V2/shared IR/interpreter parity covers disp8 -128, -1, 0 and 127 for
load/store. Effective address and store payload use entry RSP; successful load
commits the loaded RSP only after the access succeeds. Fault, address-overflow,
backend-error, STOP and zero-budget cases preserve their existing
classification and commit rules. Dispatcher budget split and callback STOP
commit `MOV RSP,[RSP-1]`, resume at `ENTRY+5`, then fault #SS on the following
`[RSP]` access without repeating the first read. Partial SIB, disp32, changed
SIB and prefix variants stop without a data callback. See the
[S3-20ad report](../reports/STAGE_3_S3-20ad_2026-10-27.md).

#### S3-20ae. RSP as both qword MOV data and signed-disp32 SIB base

Admit only exact eight-byte singleton `48 8B/89 A4 24 id`: ModRM
`mod=10/reg=4/rm=4`, SIB exactly `0x24` (RSP base, no index, scale 1),
signed disp32 in source bytes 4..7, qword data and 64-bit effective address.
The existing decoder `MemoryOperand` and `access_address_48` remain the only
address authority; generic GPR validation, frame/storage/ABI, pair and V3
profiles remain unchanged.

Direct V2/shared IR/interpreter parity covers disp32 MIN/-1/0/MAX. Entry RSP
plus signed displacement is checked against the target, and load commits a
distinct new RSP only after success; store writes the full old RSP. Two sources
that differ only in byte 7 produce the independently expected callback targets
`0x01000040` and `0x02000040`. Fault and backend errors retain their existing
classification, current #SS has no data callback, and qword range overflow is
not converted to a guest fault. Budget/STOP split and exact store+STOP preserve
commit/accounting. Dispatcher composes this singleton with NOP and the existing
pair, resumes at actual RIP without replay, and rejects partial windows 1..7
and malformed SIB/prefix forms before effects. See the
[S3-20ae report](../reports/STAGE_3_S3-20ae_2026-10-27.md).

#### S3-20af. RBP as both qword MOV data and signed-disp32 base

Admit only exact seven-byte singleton `48 8B/89 AD id`: ModRM
`mod=10/reg=5/rm=5`, signed disp32 bytes 3..6, qword data and 64-bit EA.
The decoder's `MemoryOperand` and `access_address_48` remain the sole address
authority. This exact form does not widen generic register predicates; the
mod00/rm5 RIP-relative form remains separate and uses this instruction's
NextRip. Direct V2/shared IR/interpreter parity covers disp32 MIN/-1/0/MAX,
full CPU/RAM state, callback addresses/counts and retirement. The old RBP is
used as address and store data; a successful load commits the result into RBP.
Byte-six identity and the RBP-vs-RIP address distinction are explicit.
Partial windows 1..6 and malformed forms reject. Bounded continuation commits
the first RBP load, then faults #SS on the next RBP-relative access without
repeating that read. Existing data-only/base-only/RSP profiles remain covered.
See the [S3-20af report](../reports/STAGE_3_S3-20af_2026-10-27.md).

#### S3-20ag. RBP data with the exact RSP-base SIB

Admit only four-byte singleton `48 8B/89 2C 24`: ModRM
`mod=00/reg=5/rm=4`, SIB exactly `0x24` (RSP base, no index, scale 1, no
displacement), qword data and 64-bit EA. The decoded `MemoryOperand` remains
the address authority. This exact exception does not widen generic register
predicates; other RBP-data SIB forms, prefixes and displacement forms remain
unsupported.

Direct V2/shared IR/interpreter parity covers load/store with distinct RSP and
RBP values. The old RSP supplies the effective address and store leaves RBP
unchanged; load updates RBP only after a successful read. Noncanonical RSP
produces #SS before callback, while noncanonical RBP is accepted as store data
at canonical RSP. A successful store callback STOP preserves all eight bytes.
Dispatcher continuation after `MOV RBP,[RSP]` resumes at actual RIP and uses
the newly loaded RBP for the next instruction without replaying the first read.
See the [S3-20ag report](../reports/STAGE_3_S3-20ag_2026-10-27.md).

#### S3-20ah. RBP data with RSP-base SIB displacement

Add only exact qword singleton forms `48 8B/89 6C 24 ib` (signed disp8,
length 5) and `48 8B/89 AC 24 id` (signed disp32, length 8): ModRM
`mod=01/10`, `reg=5`, `rm=4`, SIB exactly `0x24` (RSP base, no index,
scale 1). The guest RSP supplies the effective-address base and RBP supplies
the load destination/store value. Use the decoder's `MemoryOperand` for all
address semantics; do not widen generic GPR predicates or change pair/V3,
source storage, frame layout or ABI.

Direct V2/shared IR/interpreter checks cover both widths, signed boundaries,
complete state/RAM and callback/retirement parity. They also verify #SS on
noncanonical RSP without a callback, noncanonical RBP as store payload,
address-overflow/backend error classification, and store+STOP commit. The
bounded dispatcher checks both widths in singleton→NOP→pair continuation and
uses a loaded RBP for the next RBP-relative access. Partial windows and
near-match forms stop before side effects. See the
[S3-20ah report](../reports/STAGE_3_S3-20ah_2026-10-27.md).

#### S3-20ai. RSP data with an RBP-base direct displacement

Add only singleton `48 8B/89 65 ib` (signed disp8, length 4) and
`48 8B/89 A5 id` (signed disp32, length 7): ModRM `mod=01/10`,
`reg=4` (RSP data), `rm=5` (RBP base), qword data and 64-bit EA. Do not
admit SIB, prefixes, other register combinations, pairs or V3. Decoded
`MemoryOperand` remains the only EA semantics; generic register validation,
frame and ABI do not change. Keep `mod=00/rm=5` RIP-relative and existing
RBP-data `mod=10/rm=5` profiles distinct.

Direct V2/shared IR/interpreter acceptance covers both widths and signed
boundaries, including disp32 source byte-six identity, independent
`RBP+displacement` target formulas, RSP/RBP alias semantics and full state/RAM
parity. Noncanonical RSP is valid data; a noncanonical RBP address raises #SS
before callback. Store+STOP keeps all old RSP bits. Dispatch resumes from
actual RIP and uses a successfully loaded RSP in the next `[RSP]` access;
budget splits and STOP do not repeat the first effect. Partial and forbidden
forms have no effects. See the
[S3-20ai report](../reports/STAGE_3_S3-20ai_2026-10-27.md).

#### S3-20aj. RSP/RBP data with a low-GPR direct displacement

Add singleton `48 8B/89 /r disp8` (length 4) and
`48 8B/89 /r disp32` (length 7), with ModRM `mod=01/10`, data field RSP or
RBP, and direct base RAX..RBX. SIB, prefixes, other data/base registers, pairs
and V3 remain outside this slice. Effective-address semantics continue to
come from the decoded `MemoryOperand`; generic guest-register admission is not
widened.

Direct V2, shared IR and interpreter parity cover all four bases, both data
registers and both widths. Noncanonical data remains valid until used as an
address; noncanonical low-GPR effective addresses raise #GP before callback.
Stores commit before callback STOP. Bounded dispatch covers actual-RIP budget
and STOP continuation, singleton→NOP→pair composition, and later #SS when a
loaded noncanonical RSP/RBP is used as the next memory base. Partial byte
windows and unsupported near matches stop without effects. The existing
RBP-base and RIP-relative profiles remain separate. See the
[S3-20aj report](../reports/STAGE_3_S3-20aj_2026-10-27.md).

#### S3-20ak. RIP-relative qword MOV with RSP/RBP data

Extend the exact singleton RIP-relative `48 8B/89 /r disp32` profile to
`reg=RSP|RBP` only (`48 8B/89 25/2D disp32`, seven bytes). Preserve the
decoded `MemoryOperand::NextRip` as the sole effective-address authority;
do not widen any generic register predicate. Prefixes, SIB, pairs and V3 stay
outside this slice.

Direct V2/shared-IR/interpreter host parity verifies both operations at a
CodeView interior entry and uses an independent entry `NextRip + disp32`
target. Noncanonical RIP-relative targets are exact #GP faults before a RAM
callback; a noncanonical value loaded into RSP/RBP is data and commits, while
using it as the next RSP/RBP address raises #SS without repeating the first
read. Bounded dispatch exercises NOP→RSP/RBP RIP-relative singleton→two
following singleton memory operations, budgets 0..4, and continuation from
the singleton's actual next RIP. See the
[S3-20ak report](../reports/STAGE_3_S3-20ak_2026-10-27.md).

#### S3-20al. RSP/RBP data through indexed 64-bit SIB

Only the current 64-bit indexed-SIB singleton subset is extended: no
displacement (`mod=00`), signed disp8 (`mod=01`) and signed disp32 (`mod=10`),
with data register RSP or RBP, base/index RAX..RBX and scale 1/2/4/8. The
existing decoder `MemoryOperand` remains the source of effective-address
semantics. Direct V2, shared IR and the bounded dispatcher must admit the same
exact profile; no pair, V3, address-size override or special SIB shape is
included. Evidence and remaining host-owner limits are in the
[S3-20al report](../reports/STAGE_3_S3-20al_2026-10-27.md).

#### S3-20am. RSP/RBP data through address32 direct-base qword MOV

This slice adds only direct address32 forms with qword data and RSP or RBP in
the ModRM data field. The memory base is RAX..RBX, there is no SIB or RIP
relative form, and the exact instruction lengths are 4, 5 and 8 bytes for
no-displacement, signed disp8 and signed disp32. Decoder metadata remains the
sole authority for address-size truncation and signed displacement. Tests must
prove the low-32-bit effective address independently, preserve all 64 data
bits, and treat a qword access crossing 4 GiB as one contiguous helper call.
See [S3-20am evidence](../reports/STAGE_3_S3-20am_2026-10-27.md).

#### S3-20an. RSP/RBP data through address32 indexed-SIB qword MOV

Admit only singleton `67 48 8B/89 /r sib [disp8|disp32]` forms with ModRM
`mod=00/01/10,r/m=100`, data RSP or RBP, and low-GPR base/index RAX..RBX.
SIB scales remain 1/2/4/8; special no-index/no-base SIB encodings and RSP/RBP
base/index are excluded. Exact lengths are 5/6/9 bytes for no displacement,
signed disp8 and signed disp32. Address size is Dword and operand size Qword;
the existing decoded `MemoryOperand` remains the only authority for EA
calculation. Keep direct V2, shared IR and dispatch admission aligned without
changing pairs or V3. Tests cover address32 wrapping, contiguous qword access,
stack-data commit/fault semantics, the ninth displacement byte, and dispatch
continuation. See [S3-20an evidence](../reports/STAGE_3_S3-20an_2026-10-27.md).

#### S3-20ao. RSP/RBP data through address32 no-index SIB

Admit only singleton `67 48 8B/89 /r 24 [disp8|disp32]` forms with ModRM
`mod=00/01/10,r/m=100` and SIB exactly `0x24`: guest RSP is the old
low-32-bit base, there is no index, scale is one, and the data field is one
of RAX..RBP. Address size is Dword and transfer size remains Qword; lengths
are 5/6/9 bytes. The decoder's `MemoryOperand` and
`access_address_48` remain the only effective-address semantics. No other SIB,
address-size, pair or V3 form is admitted.

Direct V2, shared IR, and the bounded dispatcher must preserve full-width
data while truncating only the old RSP used for address calculation. Test a
contiguous qword access crossing 4 GiB, signed displacement boundaries,
register aliases including RSP data, callback commit/fault behavior, guard
ordering, and continuation without repeating effects. See the
[S3-20ao report](../reports/STAGE_3_S3-20ao_2026-10-27.md).

#### S3-20ap. RSP/RBP data through address32 direct EBP-base

Admit only `67 48 8B/89 /r disp8|disp32` singleton forms with ModRM
`mod=01/10,r/m=101`. The effective-address base is old guest RBP's low 32
bits, data may be any register RAX..RBP, address size is Dword, and transfer
size remains Qword. Exact lengths are 5 and 8 bytes. `mod=00/rm=101` remains
excluded and is not reclassified as this EBP-based form; SIB, extra prefixes,
pairs and V3 remain outside scope. `MemoryOperand` and
`access_address_48` continue to supply the only EA semantics.

Direct V2, shared IR and dispatch must agree on signed displacement, low-32-bit
wrap, full-width data, and old-RBP alias behavior. Verify the qword crossing
4 GiB is one access, faults do not commit, and a loaded noncanonical RBP can
retire before a later 64-bit RBP-based access faults. See the
[S3-20ap report](../reports/STAGE_3_S3-20ap_2026-10-27.md).

#### S3-20aq. Stack data through indexed SIB with RSP base

Admit only 64-bit-address-size singleton `48 8B/89 /r sib [disp8|disp32]`
forms with ModRM `mod=00/01/10,r/m=100`, SIB base RSP, index RAX..RBX and
scale 1/2/4/8. Data may be RAX..RBP and remains a full qword. Exact lengths
are 4/5/8 bytes. The existing no-index SIB `0x24` profile remains separate;
67-prefixed, other base/index, pair and V3 forms remain outside scope.

Use the decoded `MemoryOperand` and `access_address_48` for EA behavior.
Verify old-value aliases for data=RSP/index, direct/shared-IR/interpreter
parity at signed displacement boundaries, #SS on noncanonical RSP start/end,
backend-fault atomicity, STOP/budget continuation and dispatcher resumption
without repeating the first memory access. See the
[S3-20aq report](../reports/STAGE_3_S3-20aq_2026-10-27.md).

#### S3-20ar. Stack data through indexed SIB with RBP base

Admit only 64-bit-address-size singleton `48 8B/89 /r sib disp8|disp32`
forms with ModRM `mod=01/10,r/m=100`, SIB base RBP, index RAX..RBX and
scale 1/2/4/8. Data may be RAX..RBP and remains qword. Exact lengths are 5/8
bytes. The `mod=00,SIB.base=101` encoding means no base and remains rejected;
the existing RSP-base indexed and no-index SIB profiles remain unchanged.

Use the decoded `MemoryOperand` and `access_address_48` for EA behavior.
Verify old-value aliases for data=RBP/index, signed displacement edges and
wrapping, #SS for noncanonical RBP independent of data/index, #GP through a
low-GPR control base, address/backend error atomicity, and STOP/budget
continuation without repeating effects. See the
[S3-20ar report](../reports/STAGE_3_S3-20ar_2026-10-27.md).

#### S3-20as. Address32 indexed SIB with RSP base

Admit only singleton `67 48 8B/89 /r sib [disp8|disp32]` forms with
ModRM `mod=00/01/10,r/m=100`, SIB base RSP, index RAX..RBX and scale
1/2/4/8. Data may be RAX..RBP; address calculation is Dword and transfer is
Qword. Exact lengths are 5/6/9 bytes. No-index `SIB=0x24` remains its existing
separate profile; no-base and RSP/RBP index encodings, extra prefixes, pairs
and V3 remain unsupported.

The decoder's `MemoryOperand` and `access_address_48` supply all EA semantics.
Verify upper-half truncation, signed modulo-2^32 addition, old-state base/index
and data aliases, a contiguous qword crossing 4 GiB, direct V2/shared IR/
interpreter parity, fault atomicity, committed store+STOP, and actual-RIP
dispatcher continuation. See the
[S3-20as report](../reports/STAGE_3_S3-20as_2026-10-27.md).

#### S3-20at. Address32 indexed SIB with RBP base

This slice admits singleton qword MOV forms `67 48 8B/89 /r sib disp8|disp32` with ModRM `mod=01/10,r/m=100`, SIB base RBP, an RAX..RBX index, scale 1/2/4/8, and data RAX..RBP. Address calculation uses Dword semantics while the transfer remains Qword; instruction lengths are exactly six or nine bytes. The existing decoded `MemoryOperand` and `access_address_48` remain the effective-address source. `mod=00,SIB.base=101` is the no-base form and remains excluded; no-index SIB, extra prefixes, pairs and V3 are unchanged.

The focused matrices cover source/decoded-profile validation, direct V2/shared IR/interpreter parity, full qword callbacks across the 4-GiB boundary, old-state data/base/index aliases, architectural and backend error atomicity, STOP after committed stores, dispatcher budget continuation from actual RIP, and partial-window/no-base rejection. Evidence and exact commands are in the [S3-20at report](../reports/STAGE_3_S3-20at_2026-10-27.md). Owner-backed RAM integration, physical x86, Mini and performance evidence remain open.

#### S3-20au. Absolute qword MOV через no-base/no-index SIB

Принимать только singleton `48 8B/89 /r 25 disp32`: ModRM `mod=00,r/m=100`, SIB ровно `0x25` (scale 1, index none, base none), data RAX..RBP, длина 8. EA — sign-extended disp32 в 64-битном адресном размере; decoder `MemoryOperand` и `access_address_48` остаются единственным источником семантики. Indexed no-base SIB, `67`, другие SIB, moffs, пары и V3 не добавляются.

Свидетельства включают direct V2/shared IR/interpreter parity для load/store и всех шести data-регистров, независимые абсолютные target значения при изменённых GPR/RIP, полный qword у `-8` до `u64::MAX`, overflow у `-1` до helper, архитектурные/backend ошибки без commit, store+STOP, значимость восьмого source byte и отличие от RIP-relative. Borrowed dispatcher проверяется на бюджете и продолжении с actual RIP; частичные окна 1..7 дают BoundaryReached без эффектов, полный addr32 neighbor — Unsupported. Результаты и границы: [S3-20au report](../reports/STAGE_3_S3-20au_2026-10-27.md). Production owner/MMU integration, physical x86, Mini и performance остаются открытыми.

#### S3-20av. Address32 absolute qword MOV через no-base/no-index SIB

Принимать только singleton `67 48 8B/89 /r 25 disp32`: ModRM `mod=00,r/m=100`, SIB ровно `0x25`, data RAX..RBP, длина 9. `MemoryOperand` и `access_address_48` вычисляют dword-адрес: signed disp32 складывается по модулю 2^32 и zero-extends в 64-битный адрес; qword access затем остаётся единым непрерывным диапазоном и не заворачивается побайтно через 4 GiB. Runtime matcher проверяет профиль bytes opcode/ModRM/SIB и использует displacement из immutable decoder-built artifact, не пере-декодируя raw disp32. 64-bit absolute 20au сохранён; indexed no-base, прямой `rm=101`, moffs, дополнительные префиксы, пары и V3 не добавляются.

Приёмка покрывает direct V2/shared IR/interpreter для обоих направлений и всех шести data-регистров, disp32 границы и независимый low-32-bit target, qword через `0xffff_ffff`, faults/backend без commit, store+STOP, только девятый байт source в независимых artifacts, dispatcher budget/resume без replay и partial windows 1..8. Отчёт: [S3-20av report](../reports/STAGE_3_S3-20av_2026-10-27.md); owner/MMU, physical x86, Mini и performance остаются отдельными открытыми gates.

#### S3-20aw. Address64 indexed no-base SIB disp32 qword MOV

Принимать только singleton `48 8B/89 /r sib disp32`: ModRM `mod=00,r/m=100`, SIB base field `101` (no base), index RAX..RBX, scale 1/2/4/8, data RAX..RBP, длина 8. No-index SIB `0x25` 20au остаётся отдельной формой. `MemoryOperand` и `access_address_48` задают полный 64-битный EA `index*scale + sign_extended_disp32` с адресной арифметикой по модулю `2^64`; проверка canonical/range и один непрерывный qword callback выполняются тем же helper. Runtime matcher проверяет фиксированный source profile и сравнивает decoded index/scale, используя displacement из immutable decoder-built artifact без повторного decode raw displacement.

Приёмка: direct V2/shared IR/interpreter сравниваются на обоих операциях, всех шести data-регистрах, четырёх index-регистрах, четырёх scale и границах disp32. Проверяются alias `data=index` из pre-state, EA без вклада base/RIP (`index*scale + signed disp32`), u64 scale/add wrap, high displacement byte identity, #GP для noncanonical computed address без callback даже с RSP/RBP data, AddressOverflow для qword end, faults/backend без commit, store+STOP и dispatcher budget/resume без replay. Partial windows 1..7 и недопустимые соседние SIB формы не публикуют блок и не вызывают memory helper. См. [S3-20aw report](../reports/STAGE_3_S3-20aw_2026-10-27.md); production owner/MMU integration, physical x86, Mini и performance остаются открытыми.

#### S3-20ax. Address32 indexed no-base SIB disp32 qword MOV

Принимать только singleton `67 48 8B/89 /r sib disp32`: ModRM `mod=00,r/m=100`, SIB base field `101` (no base), index RAX..RBX, scale 1/2/4/8, data RAX..RBP, длина ровно 9. Это Dword EA и Qword transfer; 64-bit address-size S3-20aw и no-index SIB `0x25` S3-20av остаются отдельными формами. Декодированный `MemoryOperand` и `access_address_48` единолично вычисляют `(low32(index) * scale + disp32) mod 2^32`, затем zero-extend; qword access остаётся непрерывным через границу 4 GiB. Runtime связывает opcode/ModRM/SIB с immutable decoder artifact, не пере-декодируя displacement bytes.

Приёмка сравнивает direct V2/shared IR/interpreter на обоих направлениях, шести data-регистрах, четырёх index-регистрах, scale 1/2/4/8 и disp32 MIN/-1/0/1/MAX. Проверяются независимая формула low32 EA, высокие биты index, `data=index` из pre-state, полный qword payload, значимость девятого байта, contiguous callback у `0xffff_ffff`, fault/backend no-commit, store+STOP и dispatcher actual-RIP continuation без replay. Partial windows 1..8 и unsupported SIB сосед автоматически effects не выполняют. Отчёт: [S3-20ax report](../reports/STAGE_3_S3-20ax_2026-10-27.md). Owner/MMU, Stage 2/ISA, Mini, физическое исполнение и performance остаются вне этого среза.

#### S3-20ay. Address64 indexed SIB без displacement с индексом RBP

Добавить отдельную exact ветку только для singleton `48 8B/89 /r sib`: ModRM `mod=00,r/m=100`, data RAX..RBP, base RAX..RBX, index RBP, scale 1/2/4/8, ровно 4 bytes. Не расширять общий whitelist индексов: индексное поле `100` при `REX.X=0` остаётся кодировкой no-index; не принимать RSP/RBP/no-base/RSI/RDI как base или индекс в этой новой ветке. Использовать decoder `MemoryOperand` и `access_address_48`; EA — `base + RBP*scale` с wrapping u64.

Приёмка сравнивает direct V2/shared IR/interpreter для load/store ×6 data ×4 base ×4 scale, с отдельной формулой target, high bits RBP, alias `data=RBP`/`data=base` из pre-state, полным qword store, wrap, #GP для start/end без callback, AddressOverflow и backend-classification. Успешный store+STOP остаётся committed один раз. Bounded dispatch проверяет split бюджета и возобновление по фактическому RIP без повторного singleton access; частичные окна и недопустимые соседние SIB формы не выполняют эффекты. Сохранить no-index `SIB=0x24` и no-base `SIB=0x2d` проверки. См. [S3-20ay report](../reports/STAGE_3_S3-20ay_2026-10-27.md); owner/MMU integration, Stage 2/ISA, Mini и performance остаются вне среза.

#### S3-20az. Address64 indexed SIB disp8/disp32 с индексом RBP

Добавить exact singleton-профиль `48 8B/89 /r sib disp8|disp32`: ModRM `mod=01/10,r/m=100`, data RAX..RBP, base RAX..RBX, index RBP, scale 1/2/4/8, длина 5/8 bytes. Не расширять общий whitelist индексов; профиль без displacement S3-20ay остаётся отдельным. Запрещены address-size override, stack/no-base base, другие index, pair и V3. `MemoryOperand` и `access_address_48` остаются единственным источником EA semantics: `base + full64(RBP)*scale + signed displacement` с wrapping u64.

Приёмка сравнивает direct V2/shared IR/interpreter для обоих операций, всех шести data-регистров, четырёх low bases, scales и signed disp8/disp32 границ. Проверяются независимая формула EA, высокие биты RBP, алиасы `data=RBP`/`data=base` из pre-state, qword callback и retirement. Noncanonical start/end должны дать #GP до callback; qword endpoint overflow и backend error сохраняют отдельную классификацию без commit. Store+STOP остаётся committed один раз. Runtime должен показать влияние последнего byte disp32, dispatcher — split и continuation по фактическому RIP без replay, а partial windows/invalid neighbors не дают effects. См. [S3-20az report](../reports/STAGE_3_S3-20az_2026-10-27.md). Production owner/MMU, Stage 2/ISA, Mini, physical x86 и performance остаются вне среза.

#### S3-20ba. Address32 indexed SIB с индексом RBP

Добавить exact singleton `67 48 8B/89 /r sib [disp8|disp32]`: ModRM `mod=00/01/10,r/m=100`, base RAX..RBX, индекс ровно RBP, scale 1/2/4/8, data RAX..RBP; длины 5/6/9, Dword EA и Qword transfer. Существующие 20ay/20az address64 profiles и address32 SIB profiles остаются отдельными; не расширять общий index whitelist и не принимать stack/no-base base, другие индексы, дополнительные prefixes, пары или V3. Decoder `MemoryOperand`/`access_address_48` остаётся единственным источником low32 base/index/displacement семантики.

Приёмка покрывает load/store ×6 data ×4 base ×4 scale и no-disp/disp8/disp32 с signed краями; сравнивает direct V2/shared IR/interpreter полное state/RAM/callback/retirement. Проверяются independent `(low32(base) + low32(RBP)*scale + signed displacement) mod 2^32`, high halves, aliases из pre-state, qword callback через 4 GiB, byte-nine effect, architectural/backend no-commit, store+STOP, partial windows и invalid соседние формы. Bounded dispatcher должен продолжить с фактического RIP после budget split без повторного эффекта. Отчёт: [S3-20ba report](../reports/STAGE_3_S3-20ba_2026-10-27.md). Owner/MMU, Stage 2/ISA, Mini, physical x86 и performance не входят в этот срез.

#### S3-20bb. Address64 indexed SIB с RSP base и RBP index

Добавить exact singleton `48 8B/89 /r sib [disp8|disp32]`: ModRM `mod=00/01/10,r/m=100`, SIB base=RSP/index=RBP, scale 1/2/4/8, data RAX..RBP; длины 4/5/8. Общий whitelist индексов не расширяется: добавляется только точная пара RSP base + RBP index, отдельно от ранее принятых RSP-base/low-index и low-base/RBP-index профилей. Нет `67`, RBP/no-base base, других новых индексов, дополнительных prefix, pair или V3. `MemoryOperand` и `access_address_48` остаются единственным источником EA semantics и wrapping u64.

Приёмка покрывает load/store ×6 data ×4 scale × no-disp/disp8/disp32 signed границы с direct V2/shared IR/interpreter parity и independent formula `RSP + RBP*scale + signed displacement mod 2^64`. Host evidence включает aliases из pre-state, u64 wrap, #SS для noncanonical qword начала/конца по RSP base без callback, #GP control по low-GPR base, AddressOverflow отдельно от guest fault, backend no-commit с exact callback, store+STOP commit, dispatcher NOP/pair continuation и partial windows без effects. Отчёт: [S3-20bb report](../reports/STAGE_3_S3-20bb_2026-10-27.md). Owner/MMU, Stage 2/ISA, Mini, physical x86 и performance вне этого среза.

#### S3-20bc. Address64 indexed SIB с RBP base и RBP index

Добавить только exact singleton `48 8B/89 /r sib disp8|disp32`: ModRM `mod=01/10,r/m=100`, SIB base=index=RBP, scale 1/2/4/8, data RAX..RBP, qword EA/data, lengths 5/8. `mod=00` с SIB base `101` означает no-base и остаётся вне этого профиля. Общий index whitelist не расширяется; pair, V3, `67` и другие новые base/index combinations не принимаются. Decoder `MemoryOperand` и `access_address_48` остаются единственным источником effective-address semantics.

Приёмка покрывает load/store ×6 data ×4 scale × disp8/disp32 signed края; direct V2/shared IR/interpreter parity сравнивает CPU/RAM/callback/retirement и независимую формулу `RBP + RBP*scale + signed displacement mod 2^64`, включая alias `data=base=index=RBP` из pre-state. Noncanonical start/end дают точный #SS без callback, low-base control даёт #GP; qword endpoint overflow остаётся AddressOverflow. Backend failures сохраняют host classification, callback target и no-commit; успешный store+STOP остаётся committed. Dispatcher продолжает после budget split с фактического RIP через NOP и существующую pair без повтора эффекта; partial/invalid формы не публикуются. Отчёт: [S3-20bc report](../reports/STAGE_3_S3-20bc_2026-10-27.md). Owner/MMU, Stage 2/ISA, Mini, physical x86 и performance вне этого среза.

#### S3-20bd. Address32 indexed SIB с RSP base и RBP index

Добавить exact singleton `67 48 8B/89 /r sib [disp8|disp32]`: ModRM `mod=00/01/10,r/m=100`, SIB base=RSP, index=RBP, scale 1/2/4/8, data RAX..RBP; Dword EA/Qword transfer и длины 5/6/9. Это отдельная точная комбинация; общий индексный whitelist не расширять. Существующие address32 no-index SIB, RSP-base/low-index, low-base/RBP-index и address64 RSP/RBP-index профили остаются самостоятельными. Не добавлять RBP base, no-base, другие индексы, дополнительные prefixes, пары или V3. Decoder `MemoryOperand`/`access_address_48` остаётся единственным источником адресной семантики.

Приёмка сравнивает direct V2/shared IR/interpreter для load/store, всех шести data GPR, четырёх scale и mod00/disp8/disp32 signed edges; адрес проверяется независимо как `(low32(RSP) + low32(RBP)*scale + signed displacement) mod 2^32`. Проверяются aliases из pre-state, high-half truncation, qword через 4 GiB единым callback, exact ninth-byte target identity, backend no-commit, store+STOP, partial windows 1..8 и unsupported neighbors. Dispatcher делает budget split после singleton и resumes с actual RIP через NOP и frozen pair без повтора первого memory effect. Отчёт: [S3-20bd report](../reports/STAGE_3_S3-20bd_2026-10-27.md). Owner/MMU, Stage 2/ISA, Mini, physical x86 и performance остаются вне среза.

#### S3-20be. Address32 indexed SIB с RBP base и RBP index

Добавить только singleton `67 48 8B/89 /r sib disp8|disp32`: ModRM `mod=01/10,r/m=100`, SIB base=index=RBP, scale 1/2/4/8, data RAX..RBP; Dword EA/Qword transfer и точные длины 6/9. `mod=00` с SIB base `101` означает no-base и остаётся запрещённым для этого профиля. Общий индексный whitelist не расширяется; no-displacement, другие base/index combinations, дополнительные prefixes, пары и V3 не добавляются. Decoder `MemoryOperand` и `access_address_48` остаются единственными источниками EA semantics.

Приёмка сравнивает direct V2/shared IR/interpreter для load/store, шести data GPR, четырёх scale и signed disp8/disp32 границ с независимой формулой `(low32(RBP) + low32(RBP)*scale + signed displacement) mod 2^32`. Включены pre-state alias `data=base=index=RBP`, игнорирование верхних половин RBP, полные qword payloads и единый callback через границу 4 GiB. Проверяются guest/backend faults без commit, callback STOP после committed store, budget split/resume без повтора, разные targets при изменении source byte 9, partial windows 1..5/1..8 и отклонение no-base neighbor. Для Dword EA старшие биты base не дают #SS: адрес усекается до проверки canonicality. Отчёт: [S3-20be report](../reports/STAGE_3_S3-20be_2026-10-27.md). Owner/MMU, Stage 2/ISA, Mini, physical x86 и performance вне среза.

#### S3-20bf. Address64 indexed no-base SIB с RBP index

Принимать только singleton `48 8B/89 /r sib disp32`: ModRM `mod=00,r/m=100`, SIB `base=101` (нет base), `index=RBP`, scale 1/2/4/8, data RAX..RBP, точная длина 8. EA равен `RBP*scale + signed disp32` по модулю 2^64; нет вклада base или RIP. `MemoryOperand::access_address_48` остаётся единственным источником проверки диапазона и canonicality: отсутствие base важно, поэтому неканонический результат faults как #GP, не #SS. Не добавлять `67`, другие индексы, mod01/10, пары, V3 или owner/MMU integration.

Приёмка сравнивает direct V2/shared IR/interpreter для обоих направлений, шести data GPR, четырёх scale и signed disp32 границ, проверяя независимую формулу, полный CPU/RAM/callback/retirement и pre-state alias `data=RBP`. Дополнительно проверить #GP start/end без callback, AddressOverflow отдельно, backend faults без commit, store+STOP, byte-8 target identity, окна 1..7 без эффектов, unsupported соседей и bounded dispatcher publication/continuation без повтора. Address32 sibling now has a separate exact S3-20bg profile; it does not broaden this Qword admission. Отчёт: [S3-20bf report](../reports/STAGE_3_S3-20bf_2026-10-27.md). Owner/MMU, Stage 2/ISA, Mini и performance вне среза.

#### S3-20bg. Address32 indexed no-base SIB с RBP index

Добавить только singleton `67 48 8B/89 /r sib disp32`: ModRM `mod=00,r/m=100`, SIB base `101` (нет base), index RBP, scale 1/2/4/8, data RAX..RBP; Dword EA/Qword transfer, точная длина 9. Это отдельный профиль от 20bf: EA равен `(low32(RBP)*scale + signed disp32) mod 2^32`, затем zero-extends в 64 бита; ни base, ни RIP не участвуют. Декодированный `MemoryOperand`/`access_address_48` остаётся единственным источником EA semantics. Не добавлять другие индексы, `67`-соседей, пары или V3.

Приёмка сравнивает direct V2/shared IR/interpreter для load/store, всех шести data-регистров, четырёх scale и MIN/-1/0/1/MAX disp32, включая pre-state `data=RBP` alias и полный 64-bit payload. Проверяются high-half truncation, independent low32 formula, qword callback через 4 GiB, byte-nine identity, arch/backend no-commit, store+STOP и budget/actual-RIP continuation без повтора; partial windows 1..8 и unsupported neighbors не дают effects. Отчёт: [S3-20bg report](../reports/STAGE_3_S3-20bg_2026-10-28.md). Owner/MMU, Stage 2/ISA, Mini, physical x86 и performance остаются вне среза.

#### S3-20bh. Address64 direct-base qword MOV через RSI/RDI

Допускаются только singleton `48 8B/89 /r` длины 3 с ModRM `mod=00`, base `rm=110` (RSI) или `rm=111` (RDI), data RAX..RBP; индекс отсутствует, displacement нулевой. RSI/RDI допустимы только как адресная база, не как data/index. Не добавлять SIB, displacement, `67`, дополнительные prefixes, pair или V3. EA, canonicality и qword range остаются decoder-owned.

Приёмка покрывает оба направления, обе базы и шесть data GPR direct V2/shared IR/interpreter parity с полным состоянием, RAM, callback адресом/count и retirement. Проверить, что полный старый RSI/RDI задаёт адрес независимо от RIP/RSP/RBP и сам не меняется; load меняет только destination, store записывает полный qword. #GP для noncanonical start/end не вызывает helper даже при RSP/RBP data; AddressOverflow отдельно; backend errors без commit; store+STOP committed once; entry STOP/stop-RIP/budget zero без доступа; continuation с budget=1 продолжает от actual RIP без повтора. Partial windows 1..2 и соседние SIB/displacement/addr32/data-RSI-RDI формы не дают effects. Отчёт: [S3-20bh report](../reports/STAGE_3_S3-20bh_2026-10-28.md). Owner/MMU, Stage 2/ISA, Mini и performance вне среза.

#### S3-20bi. Address64 direct-base qword MOV через RSI/RDI с displacement

Допускаются только singleton `48 8B/89 /r disp8|disp32`: ModRM `mod=01/10`, `rm=110` (RSI) или `rm=111` (RDI), data RAX..RBP, длины 4/7. RSI/RDI остаются только адресными базами. Нет SIB, `67`, дополнительных prefixes, новых data/index регистров, pair или V3. Signed displacement и EA вычисляет только decoder-owned `MemoryOperand::access_address_48`.

Приёмка: load/store × обе базы × шесть data GPR × signed края disp8 и disp32; независимое ожидание `old_base.wrapping_add_signed(displacement)`; CPU/RAM/callback/retirement parity direct V2/shared IR/interpreter. Проверить отсутствие влияния RIP/RSP/RBP на адрес, сохранность базы и остальных GPR, полный qword store, #GP start/end без callback, AddressOverflow отдельно, arch/backend atomicity, store+STOP once, entry guards и continuation от actual RIP без replay. Last byte disp32 должен менять target; partial windows 1..3 и 1..6, запрещённые SIB/prefix/addr32/data-RSI-RDI формы не имеют effects. Продолжить dispatcher через disp8 и disp32 singleton по budget=1 без повтора. Отчёт: [S3-20bi report](../reports/STAGE_3_S3-20bi_2026-10-28.md). Owner/MMU, Stage 2/ISA, Mini и performance вне среза.

#### S3-20bj. Address32 direct-base qword MOV через RSI/RDI

Допускаются только singleton `67 48 8B/89 /r [disp8|disp32]`: ModRM `mod=00/01/10`, `rm=110` (ESI) или `rm=111` (EDI), data RAX..RBP, длины 4/5/8. EA decoder-owned: `(low32(old RSI/RDI) + signed displacement) mod 2^32`, затем zero-extension; перенос данных и qword helper остаются 64-битными. Полные RSI/RDI сохраняются. SIB, дополнительные prefixes, новые data/index registers, пары и V3 не добавляются.

Приёмка сравнивает direct V2/shared IR/interpreter для load/store, обеих баз, всех шести data-регистров и границ displacement; проверяет полное состояние CPU/RAM, один callback `(EA, 8)`, retirement, неизменность полной базы и независимую low32-формулу. Верхние 32 бита базы не влияют на EA; qword callback у `0xffff_fffc` непрерывен через 4 GiB. Backend failure не коммитит состояние/RAM, STOP после успешного store оставляет запись закоммиченной, entry guards не вызывают helper, частичные окна и соседние SIB/data-RSI-RDI формы не имеют effects. Dispatcher продолжает со следующей singleton по actual RIP без повторного обращения. Отчёт: [S3-20bj report](../reports/STAGE_3_S3-20bj_2026-10-28.md). Production owner/MMU, Stage 2/ISA, Mini и performance остаются за пределами среза.

#### S3-20bk. Address64 indexed SIB с RSI/RDI base

Принимать только singleton `48 8B/89 /r sib` длиной 4: ModRM `mod=00/rm=100`, SIB base RSI/RDI, index RAX..RBX, scale 1/2/4/8 и data RAX..RBP. Address/data size — Qword. Не принимать 67, displacement, no-index/no-base, RBP/RSI/RDI как index, пары или V3. EA берётся только из decoded `MemoryOperand::access_address_48` и равна `old_full_base.wrapping_add(old_full_index.wrapping_mul(scale))`; чтение source registers происходит до commit destination.

Проверки direct V2/shared IR/interpreter покрывают load/store для двух bases, четырёх indices, четырёх scale и шести data GPR; сравнивают полное состояние, RAM, точный callback `(EA, 8)` и retirement. Включены pre-state aliases data=index и сохранение base (base RSI/RDI не может совпасть с data RAX..RBP), неканонический start и конец qword range без callback, AddressOverflow, entry STOP/zero budget без helper, store+STOP commit и budget continuation через NOP и существующую пару без повторного чтения. Descriptor rejects neighboring encodings и partial windows. Отчёт: [S3-20bk report](../reports/STAGE_3_S3-20bk_2026-10-28.md). Production owner/MMU, Stage 2/ISA, Mini, physical x86 и performance остаются вне среза.

#### S3-20bl. Address64 indexed SIB с RSI/RDI base и signed displacement

Поддержать singleton `48 8B/89 /r sib disp8|disp32`: ModRM `mod=01/10,r/m=100`, SIB base RSI/RDI, index RAX..RBX, scale 1/2/4/8, data RAX..RBP, длина 5/8. Effective address остаётся семантикой decoder `MemoryOperand::access_address_48`: old full base + old full index × scale + signed displacement с wrapping u64. Не добавлять address override, другие базы/индексы, no-index/no-base, prefixes, pairs или V3.

Descriptor и host direct V2/shared IR/interpreter проверяют обе операции, оба base, все индексы/scales/data GPR и signed границы disp8/disp32, включая pre-state data=index и точный target/callback. Проверяются также два disp32 artifacts с различием только в последнем displacement byte и high-index wrapping u64 адрес. Dispatcher покрывает disp8 и disp32 continuation по фактическому RIP через NOP и существующую пару без replay. Добавить #GP на неканонический start/end без callback, AddressOverflow отдельно, faults без commit, store+STOP commit once, STOP/zero budget guards и частичные окна/invalid neighbors без эффектов. Отчёт: [S3-20bl report](../reports/STAGE_3_S3-20bl_2026-10-28.md). Остальные production owner/MMU, Stage 2/ISA, Mini, physical x86 и performance requirements остаются открытыми.

#### S3-20bm. Address32 indexed SIB с RSI/RDI base

Поддержать только singleton `67 48 8B/89 /r sib [disp8|disp32]`: ModRM `mod=00/01/10,r/m=100`, SIB base RSI/RDI, index RAX..RBX, scale 1/2/4/8, data RAX..RBP, длины 5/6/9. EA задаёт decoder-owned `MemoryOperand::access_address_48`: `(low32(old base) + low32(old index) * scale + signed displacement) mod 2^32`, затем zero-extension; перенос qword остаётся 64-битным, полные RSI/RDI сохраняются.

JIT descriptor и host direct V2/shared IR/interpreter должны проверять load/store, оба base, все data/index/scale combinations для mod00 и signed disp8/disp32 boundaries. Подтвердить data=index pre-state, независимую low32 oracle, точные callback и retirement, contiguous qword callback на границе 4 GiB, изменение target при изменении ninth source byte, architectural/backend load и store fault atomicity, store+STOP commit, pre-entry STOP/stop-RIP/zero-budget без helper, actual-RIP budget continuation последовательно через длины 5/6/9 без replay, partial windows и forbidden neighbors. Результаты/команды: [S3-20bm report](../reports/STAGE_3_S3-20bm_2026-10-28.md). Production owner/MMU, Stage 2/ISA, Mini и performance остаются вне среза.

#### S3-20bn. Address64 indexed SIB с RSI/RDI base и RBP index

Поддержать только singleton `48 8B/89 /r sib [disp8|disp32]`: ModRM `mod=00/01/10,r/m=100`, SIB base RSI/RDI, index ровно RBP, scale 1/2/4/8, data RAX..RBP, длины 4/5/8. EA берётся из decoder-owned `MemoryOperand::access_address_48`: old full-width base + old full-width RBP × scale + signed displacement с wrapping u64. Не расширять общую whitelist индексов; no-index, RSP/RSI/RDI index, address override, другие базы, pair/V3 остаются вне профиля.

Проверить direct V2/shared IR/interpreter parity, data=RBP pre-state alias, все bases/scales/data/операции и mod00/disp8/disp32. Добавить независимую формулу адреса, wrap, фиксированный disp32 byte7 identity, точные callback/retirement, #GP на noncanonical start/end, отдельный AddressOverflow, fault no-commit, store+STOP, STOP/budget guards, continuation по фактическому RIP через формы 4/5/8 без повторных эффектов, partial windows и invalid neighbors. Результаты: [S3-20bn report](../reports/STAGE_3_S3-20bn_2026-10-28.md). Production owner/MMU, Stage 2/ISA, Mini и performance остаются вне среза.

#### S3-20bo. Address32 indexed SIB с RSI/RDI base и RBP index

Поддержать singleton `67 48 8B/89 /r sib [disp8|disp32]`: ModRM `mod=00/01/10,r/m=100`, SIB base RSI/RDI, index ровно RBP, scale 1/2/4/8, data RAX..RBP, длины 5/6/9. Effective address берётся только из decoder-owned `MemoryOperand::access_address_48`: `(low32(old base) + low32(old RBP) * scale + signed displacement) mod 2^32`, затем zero-extension; перенос qword остаётся 64-битным. RSI/RDI base сохраняются; индекс RBP читается из pre-state и меняется только если он является load destination после успешного доступа.

Проверки должны покрывать load/store × обе базы × все data registers/scales × mod00 и signed disp8/disp32 edges, direct V2/shared IR/interpreter parity, точные адрес/count/retirement и pre-state `data=RBP`. Добавить high-half truncation, contiguous qword callback на `0xffff_fffc`, architectural/backend no-commit, STOP после успешного store, pre-entry STOP/zero-budget, continuation по actual RIP между 5/6/9-byte forms без replay, ninth disp32 byte sensitivity, partial windows и invalid neighbors. Отчёт: [S3-20bo report](../reports/STAGE_3_S3-20bo_2026-10-28.md). Production owner/MMU, Stage 2/ISA, Mini и performance остаются вне среза.

#### S3-20bp. Address64 no-displacement SIB с RSI/RDI index

Поддержать только singleton `48 8B/89 /r sib`: ModRM `mod=00/rm=100`, SIB base RAX..RBX, index ровно RSI или RDI, scale 1/2/4/8, data RAX..RBP, длина 4. EA вычисляется только decoder-owned `MemoryOperand::access_address_48`: старые полные значения base + index×scale с wrapping u64. `data=base` использует pre-state base для EA и store payload; load в base меняет его только после успешного read. RSI/RDI остаются address-only indices и не изменяются; общая whitelist индексов и data-регистров не расширяется.

Проверить load/store × четыре low bases × RSI/RDI × все scales × шесть data regs через direct V2/shared IR/interpreter parity, включая полный CPU/RAM, независимый EA oracle, callback address/count и retirement. Добавить точную #GP проверку canonical start/noncanonical end, отдельный AddressOverflow без callback, architectural/backend fault без commit, successful store+STOP, entry STOP/zero budget, actual-RIP continuation singleton→NOP→pair без повторных эффектов и partial/invalid neighbors. Отчёт: [S3-20bp report](../reports/STAGE_3_S3-20bp_2026-10-28.md). Production owner/MMU, Stage 2/ISA, Mini и performance остаются вне среза.

#### S3-20bq. Address64 indexed SIB с low base, RSI/RDI index и displacement

Добавить только singleton `48 8B/89 /r sib disp8|disp32`: ModRM `mod=01/10,r/m=100`, SIB base RAX..RBX, index ровно RSI/RDI, scale 1/2/4/8, data RAX..RBP, длины 5/8. EA остаётся decoder-owned `MemoryOperand::access_address_48`: старый полный base + старый полный index×scale + signed displacement с wrapping u64. Не менять общие whitelist и не принимать address32, stack/no-base, другие indices, prefixes, pairs или V3.

Проверить обе операции × четыре base × RSI/RDI × четыре scale × шесть data GPR на signed disp8/disp32 границах: parity direct V2/shared IR/interpreter, полный CPU/RAM, независимая формула и callback/count/retirement. Подтвердить data=base из pre-state, full-width high-index bits и u64 wrap, RSI/RDI preservation, последнюю disp32 byte sensitivity, #GP start/end без callback, отдельный AddressOverflow, guest/backend no-commit, store+STOP, entry STOP/stop-RIP/zero budget, partial/unsupported без эффекта и actual-RIP budget continuation по disp8/disp32 без replay. Отчёт: [S3-20bq report](../reports/STAGE_3_S3-20bq_2026-10-28.md). Production owner/MMU, Stage 2/ISA, Mini и performance остаются вне среза.

#### S3-20br. Address32 indexed SIB с low base и RSI/RDI index

Поддержать только singleton `67 48 8B/89 /r sib [disp8|disp32]`: ModRM `mod=00/01/10,r/m=100`, SIB base RAX..RBX, index ровно RSI или RDI, scale 1/2/4/8, data RAX..RBP, длины 5/6/9. EA остаётся decoder-owned через `MemoryOperand::access_address_48`: `(low32(old base)+low32(old index)*scale+signed displacement) mod 2^32`, затем zero-extension; перенос и данные — полные qword. RSI/RDI используются только как индекс и сохраняются; data=base использует pre-state для адреса/store, а load commit происходит только после успешного чтения.

Приёмка: direct V2/shared IR/interpreter parity CPU/RAM/callback/retirement; все операции × low bases × RSI/RDI × scales × data GPR и формы mod00/disp8/disp32; high-half truncation и modulo-2^32 wrap, включая product/addition wrap; qword crossing 4 GiB одним contiguous callback; final byte 9 меняет decoded disp32 target; noncanonical high halves не порождают #GP после address-size truncation (Dword EA плюс qword length также не может переполнить u64); architectural/backend load/store no-commit; store+STOP и entry STOP/stop-RIP/zero-budget; actual-RIP continuation через 5/6/9-byte формы без replay; partial windows и invalid neighbors без эффектов. Отчёт: [S3-20br report](../reports/STAGE_3_S3-20br_2026-10-28.md). Production owner/MMU, Stage 2/ISA, Mini и performance вне среза.

#### S3-20bs. Address64 indexed SIB с RSP base и RSI/RDI index

Поддержать только singleton `48 8B/89 /r sib [disp8|disp32]`: ModRM `mod=00/01/10,r/m=100`, SIB base ровно RSP, index ровно RSI или RDI, scale 1/2/4/8, data RAX..RBP, длины 4/5/8. EA остаётся decoder-owned через `MemoryOperand::access_address_48`: старые полные RSP + старый полный RSI/RDI×scale + signed displacement с wrapping u64. `data=RSP` адресуется и сохраняется из pre-state; load меняет RSP только после успешного чтения. RSI/RDI используются только как индекс и сохраняются.

Приёмка: direct V2/shared IR/interpreter parity для обеих операций × 6 data GPR × RSI/RDI × 4 scale × mod00/disp8/disp32 signed edges; complete CPU/RAM, независимый EA oracle, callback address/count и retirement. Проверить pre-state `data=RSP`, full-width index bits, #SS при noncanonical start/end для RSP-base до callback, #GP для low-base control, отдельный AddressOverflow, architectural/backend no-commit load/store, store+STOP, entry STOP/stop-RIP/zero budget, byte8 disp32 target identity и actual-RIP continuation по 4/5/8-byte формам без replay. Partial windows и forbidden neighbors должны останавливаться без эффекта. Отчёт: [S3-20bs report](../reports/STAGE_3_S3-20bs_2026-10-28.md). Production owner/MMU, Stage 2/ISA, Mini и performance вне среза.

#### S3-20bt. Address64 indexed SIB с RBP base и RSI/RDI index, с displacement

Поддержать только singleton `48 8B/89 /r sib disp8|disp32`: ModRM `mod=01/10,r/m=100`, SIB base ровно RBP, index RSI или RDI, scale 1/2/4/8, data RAX..RBP, длины 5/8. `mod=00` с SIB base field 5 остаётся no-base и этим профилем не принимается. EA вычисляется исключительно decoder-owned `MemoryOperand::access_address_48`: старый полный RBP + старый полный RSI/RDI×scale + signed displacement с wrapping u64. Значение data-register берётся из pre-state для store и адресной базы; load коммитит destination только после успешного чтения.

Приёмка: direct V2/shared IR/interpreter parity обеих операций × шести data GPR × RSI/RDI × четырёх scale × signed disp8/disp32 краёв; independent EA oracle, full CPU/RAM, callback address/count и retirement. Подтвердить #SS при noncanonical start/end для RBP-base до callback, #GP control на low-base, отдельный AddressOverflow, architectural/backend no-commit load/store, committed store+STOP, entry stop-RIP/STOP/zero-budget, partial windows 1..7 и actual-RIP continuation across 5/8-byte forms без replay. Добавить отрицательные соседи, включая no-base и лишний prefix, сохранив предыдущие профили. Отчёт: [S3-20bt report](../reports/STAGE_3_S3-20bt_2026-10-28.md). Production owner/MMU, Stage 2/ISA, Mini и performance вне среза.

#### S3-20bu. Address32 indexed SIB с RSP base и RSI/RDI index

Поддержать только singleton `67 48 8B/89 /r sib [disp8|disp32]`: ModRM `mod=00/01/10,r/m=100`, SIB base ровно RSP, index ровно RSI или RDI, scale 1/2/4/8, data RAX..RBP, длины 5/6/9. EA остаётся исключительно decoder-owned `MemoryOperand::access_address_48`: `(low32(old RSP)+low32(old index)*scale+signed displacement) mod 2^32`, затем zero-extension; transfer и store payload остаются qword. Для data=RSP адрес и store source читаются из pre-state; load обновляет RSP только после successful read. RSI/RDI остаются address-only indices и сохраняются.

Приёмка: обе операции × data RAX..RBP × RSI/RDI × все scales × mod00/disp8/disp32, direct V2/shared IR/interpreter parity и полное CPU/RAM/callback/retirement сравнение. Подтвердить truncation high halves, modulo-2^32 product/add wrap, один contiguous qword callback на crossing 4 GiB, byte9 disp32 target identity, архитектурную/backend atomicity, store+STOP, entry stop-RIP/STOP/zero-budget, partial windows, invalid neighbors и actual-RIP continuation через 5/6/9-byte singleton без replay. Отчёт: [S3-20bu report](../reports/STAGE_3_S3-20bu_2026-10-28.md). Production owner/MMU, Stage 2/ISA, Mini и performance вне среза.

### S3-21. Подтвердить memory slice на физическом x86

1. Выполнить после локального завершения Stage 3 как финальный gate. Использовать завершённый S2-09 native memory profile. Проверить одинаковые входы, наблюдение всех заявленных GPR, нормализацию guest addresses и сравнение memory effects.
2. Добавить ручные и generated load/store cases, пересечения regions/pages, ошибки доступов и неполное наблюдение fault metadata.
3. Сравнить interpreter/JIT/native, повторить прогоны и сохранить полные результаты. Проверять helper и allocator clobbers вместе с памятью.

**Готово:** memory subset имеет независимое подтверждение. Если S2-09 не готов, задача остаётся заблокированной; synthetic snapshots, S0 oracle и локальное совпадение не заменяют её.

### S3-22. Учесть смену execution context

1. На изменениях режима, mapping identity/permissions, необходимых segment/control state и profile выполнить invalidate/relookup либо гарантированный exit.
2. Для отсутствующих production MMU/system features проверить contract через контролируемый fixture backend и обозначить интеграционную зависимость этапа 4.
3. Не считать неизменный RIP или одинаковые bytes доказательством пригодности старого compiled block.
4. Для reset и будущего restore/clone определить очистку или перепроверку cache. Архитектурные snapshots не содержат host code pointers; executable blocks создаются заново. Реализация полного VM snapshot относится к отдельному этапу.

**Готово:** context-sensitive cases не используют несовместимый перевод. Фактическое paging/TLB поведение не заявляется на основании fixture test.

#### S3-22a. Bounded fixture context generation

`ExecutionContextId` — caller-owned nonzero monotonic generation, а
`ExecutionContextSnapshot` combines it with the explicit JIT profile. Cached
single- and multi-entry APIs require this stamp and admit only the existing
`S0_09Fixture + Long64 + canonical48` profile; rejection occurs before cache
binding, compile, publish, or execute. The snapshot must remain frozen under an
owner lease for the whole synchronous dispatch. Mapping/backing IDs, versions,
and executable permission remain in the existing code dependency spans.

`TranslationKey` includes the context snapshot. A higher generation clears
resident entries and drops their published handles before a new lookup; a
same-generation profile change and generation regression are errors. Explicit
cache reset requires a higher generation. A reset/restore/clone therefore
causes fresh publication. GPRs, RFLAGS, budget, STOP-RIP and STOP pointers stay
out of the key. Tests cover same-context hit, changed-context miss at unchanged
RIP/source, old-handle drop, reset/republication, unsupported profiles with no
backend work, existing mapping/backing/permission invalidation, and state/control
changes without false misses.

This slice does not add a production MMU owner or live-owner candidate commit
for compilation that outlives/releases its snapshot. The caller's frozen
snapshot lease prevents owner mutation during the current synchronous
compile/publication path; revalidation is needed if a future async/unfrozen
path is added. Cached V2 memory load/store paths are not enabled. Physical Mini
checks remain deferred until final S3-21 after local Stage 3 work. Results are recorded in the
[S3-22a report](../reports/STAGE_3_S3-22a_2026-10-27.md).

### S3-23. Инвалидировать зависимости изменённого кода

#### S3-23a. Bounded fixture invalidation between dispatch calls

Реализован owner-to-cache notification path для bounded fixture cache между
завершёнными synchronous dispatch calls. `CodeInvalidation` покрывает
committed backing-content version change, mapping replacement/removal и
execute-permission change. `SingleEntryBlockCache` и `MultiEntryBlockCache`
снимают зависимые entries из lookup и сразу drop-ают опубликованные handles;
multi-entry counters освобождают resident code/metadata accounting. Поиск
backing ID учитывает aliases, а mapping events сопоставляют address-space и
mapping IDs.

Fixture owner сначала проверяет range/version increment, выполняет write,
продвигает backing version и передаёт notification до следующего snapshot.
Неуспешная запись не меняет bytes/version/cache. Cache borrow и API допускают
invalidation только между завершёнными calls, поэтому в этом срезе нет active
entry retirement protocol. Срез покрывает code owner fixture и register-only
S0_09 dispatch; unsafe V2 store для code writes не используется.

Локальные tests подтверждают changed bytes → old handle drop → новый compiled
code, alias-wide invalidation, failed-write preservation, remap на тот же guest
address, execute-permission revoke до повторного entry, unrelated backing hit и
reclamation resident handles/code/metadata. Owner mutation, racing publication,
interpreter/JIT/control writes, production MMU, DMA/device notifications и
active self-modifying code остаются вне среза и требуют последующей интеграции.
Результат: [S3-23a report](../reports/STAGE_3_S3-23a_2026-10-27.md).

#### S3-23c. Generated V2 store в другой executable backing — fixture partial

Typed V2 store slot/handle размещается в общей bounded executable arena рядом
с cached V1 block slots, но остаётся отдельным от V1 entry и store API для
private data RAM. После stop-RIP → STOP → budget guards fixture owner проверяет и связывает полный qword
range, executable/write permissions, следующую nonwrapping backing version, исходные bytes/RIP с активным source lease и
пересечение source backing, включая alias. Ошибка preflight выдаётся только если generated helper дошёл до write; Store в активный
source отвергается до mutation. Успешный generated helper write меняет
фиксированный executable guest backing один раз; после возврата native call и
host result decoding owner продвигает version, отправляет
CodeInvalidation, и cache удаляет только старый target entry до следующего
lookup. V2 store slot вычитается из доступной cache capacity.

Локальный end-to-end fixture прогревает B и unrelated C, выполняет compiled
store A через alias B, подтверждает little-endian byte change, один write,
late-STOP commit, B invalidation/recompile с новым результатом и C cache hit.
Дополнительные cases проверяют active overlap, range/permission/version
preflight и backend failure без частичной записи/version/cache mutation.
Срез ограничен одним vCPU и фиксированным RAM; нет active SMC/retirement,
production MMU, MMIO, DMA, SMP или автоматического retry. Существующий unsafe
execute_memory_store_v2 private-data contract не ослаблен и для этого code
write path не используется. Результат: [S3-23c report](../reports/STAGE_3_S3-23c_2026-10-27.md).

#### S3-23d. Общая fixture transaction для interpreter/JIT/control writes — partial

Bounded `TrackedCodeFixture` направляет три писателя через общий путь:
`prepare_write` проверяет полный range, write permission, требуемую executable
permission, следующий backing version и overlap с active source;
`write_preflighted` выполняет одну all-or-nothing запись. Generated V2 store
передаёт заранее проверенный план; interpreter использует существующий
`od_isa::execute::step` и `GuestMemory::write`; control adapter вызывает тот же
`GuestMemory::write` между execution calls. После возврата interpreter/JIT
adapter фиксирует версию и применяет notification к cache до следующего
snapshot/lookup. Control adapter делает это до возврата caller.

Differential tests запускают один MOV [base],reg через interpreter и
production V2 host entry на независимых owners, сравнивают полный CPU state,
RAM, backing version, cache invalidation и последующее выполнение B. Отдельный
control-write case использует alias, сохраняет unrelated C и проверяет
backend failure без mutation/cache eviction. Active-source overlap в
interpreter также отвергается до commit. Это только test fixture: production
MMU/control owner, shared/asynchronous RAM, MMIO, DMA, SMP, REP/RMW,
compile-publication races и active-code retirement не реализуются. Stage 2
файлы не менялись. См. [S3-23d report](../reports/STAGE_3_S3-23d_2026-10-27.md).

#### S3-23e. Region-scoped executable-write notification and redispatch — partial

Generated V2 store entry принимает отдельную bounded code-region stamp. Owner
требует, чтобы регион заканчивался ровно на `next_rip` store, отдельно
запрещает target overlap с source bytes store artifact (включая guest aliases),
затем сверяет регион с backing. После committed store/result decoding применяет
`CodeInvalidated` ко всем зависимым cache entries, включая оба guest aliases,
и возвращает этот exit для redispatch до следующего lookup. Тест кеширует prefix и
tail в двух aliases, выполняет отдельный store entry, проверяет exact-once
commit и interpreter state/RAM parity, затем заново dispatch-ит tail и source
по новым bytes. Компилируемый artifact store содержит только сам store; tail
не исполняется native во время этой записи. Следовательно, это prerequisite
между calls для coherence, а не S3-24 retirement активного native tail.
См. [S3-23e report](../reports/STAGE_3_S3-23e_2026-10-27.md).

#### S3-23f. Cross-mapping qword write within one backing — partial

`TrackedCodeFixture` resolves a range covered by one mapping or by exactly two
guest-adjacent mappings. A crossing range is accepted only when both mappings
refer to the same backing ID and the second backing offset immediately follows
the first; preflight validates the complete backing range and write/execute
permissions on every mapping before it reserves one next backing version. The
commit remains one all-or-nothing slice copy and emits one backing notification.
The target does not require the write bytes or the decoder-backed code view to
stop at the page boundary; cache dependencies retain each mapping identity in a
separate span even though the code view reads a contiguous backing slice.

The native test warms B (`MOV RDX,imm64`) at page-end−6 and an alias, plus C on
another backing. A separate checked V2 qword store writes the immediate at
page-end−4 across two adjacent mappings. It verifies exact little-endian bytes,
one version increment/notification, eviction of B and its alias, a C cache hit,
and fresh native B/alias results from the new immediate. Negative cases cover a
missing, read-only, or non-executable second mapping, exhausted version, backend error,
different/noncontiguous backing, and arithmetic overflow; failures preserve CPU,
both backing halves, versions, and cache entries. Writes ending exactly at or
starting exactly on the boundary remain valid single-mapping cases. Resolver is
bounded to two mappings and fixed fixture RAM; no production MMU, more-than-two
mapping operation, MMIO/DMA, or concurrent writer support is implied. See
[S3-23f report](../reports/STAGE_3_S3-23f_2026-10-27.md).

1. Ввести единый путь уведомления committed memory writes и смены backing/mapping. Подключить interpreter, JIT и управляющие writes доступного среза.
2. Изменить generation/identity нужных code regions, удалить зависимые entries из lookup и сделать невозможным stale entry по старой ссылке.
3. Проверить writable executable region, aliases, пересечение нескольких страниц, запись из другого block и повторное отображение нового backing по тому же адресу.
4. Проверить изменение code version между capture/decode и publication; устаревший результат компиляции не публиковать.

**Готово:** повторное исполнение наблюдает новые bytes. Уведомление future DMA/device writers описано как обязательная integration point, а не фиктивно реализованное устройство.

### S3-24. Обработать self-modifying code текущего блока

1. Создать case, в котором store изменяет ещё не исполненную guest instruction текущего compiled block, и case с изменением следующего блока.
2. После committed write, затрагивающего активные code dependencies, выйти с правильным guest state/RIP и продолжить через проверку актуального кода.
3. Проверить, что store выполнен ровно один раз, instruction count не удваивается, stale host tail не исполняется и переход через alias тоже учитывается.

**S3-24 остаётся открытым.** S3-24a добавляет один compiler-issued 23-byte native artifact с тремя decoder-validated instructions: `MOV RDX,imm64; MOV [RBX],RAX; MOV RCX,imm64`. Один typed V3 arena entry выполняет все три в одном generated call, но при успешной записи в уже исполненный prefix synchronously sets an invocation-local dirty marker, commits store RIP/budget/retirement once, and returns `CodeInvalidated` before stale native tail. The owner applies the reserved backing version/notification after result decoding and before the active handle is released. A later dispatch executes the tail from current bytes; re-entry observes the changed prefix. An unrelated destination completes the full three-instruction artifact. The store instruction itself may not overlap; future-tail targets are rejected, including aliases.

Fault after the prefix preserves RIP at the store and the committed prefix; budget/STOP boundaries, STOP raised during a successful write, alias writes, and backend failure are covered. This is a bounded already-executed-prefix case; it does not establish semantics for modifying a not-yet-executed instruction, general multi-instruction compilation, production MMU/writers, or full S3-24. See [S3-24a report](../reports/STAGE_3_S3-24a_2026-10-27.md).

### S3-25. Реализовать eviction и безопасное освобождение

#### S3-25a. Synchronous published-handle lifetime partial

Для текущего одного vCPU `run_cached` удерживает borrowed cache entry,
compiled metadata и published handle от вызова backend до проверки его
decoded result/counters. `PublishedArenaBackend` держит borrow arena и slot
metadata на всём протяжении generated call и host result decoding. Безопасная
точка для eviction/clear/slot reuse — после возврата backend и обработки его
результата, до следующего lookup; cache API и borrow checker не допускают
сброса/замены текущего entry изнутри этого синхронного вызова.

Проверки capacity 1/2 выполняют A→B→A churn, включая соседний живой слот,
STOP до entry, STOP после backend возврата, injected backend/executor error,
clear и освобождение code/metadata accounting. Existing arena lease test
проверяет, что уничтожение backend не снимает MAP_JIT mapping пока live handles
его удерживают, а drop последнего handle позволяет открыть arena заново.
Ошибка выполнения удаляет entry только после возврата backend; STOP сохраняет
resident mapping. Результат: [S3-25a report](../reports/STAGE_3_S3-25a_2026-10-27.md).

Это не active invalidation/SMC, SMP reclamation, deferred retirement queue или
полный S3-25. Эти механизмы требуют отдельного протокола и остаются открытыми.

1. Развести недоступность для новых входов, active execution, references и физическое освобождение code/metadata.
2. Для одного vCPU использовать простой проверяемый retirement path на безопасных точках; не внедрять SMP reclamation framework.
3. Проверить capacity pressure, invalidation активного блока, выход по helper fault/STOP, удаление cache и повторное создание mapping.
4. После освобождения никакая entry/PC-map/link reference не должна вести в повторно используемую память прежнего блока.

**Готово:** стресс с малыми capacity не вызывает use-after-free, утечки retired blocks или исполнения освобождённого кода. Доказательства относятся к одному vCPU; SMP остаётся будущей работой.

### S3-26. Сохранить порядок side effects

1. Разметить IR memory/helper operations по реальным effects: обычная память, потенциальный fault, atomic/ordering operation, device callback при его появлении.
2. Запретить перемещение/удаление accesses через fault/commit boundaries без доказанной эквивалентности. Не считать два одинаковых адреса разрешением объединить две MMIO reads.
3. Через контролируемый backend проверить порядок и число callbacks, отсутствие повторной записи после fallback и корректные exits для неизвестных memory classes.
4. Реальные MMIO/atomic модели подключать только вместе с готовой семантикой этапов 2/4. До этого явно ограничить исполняемый профиль fixture RAM.

**Готово:** IR и lowering сохраняют observable order, faults и однократность side effects. Test callback доказывает контракт, а не поддержку виртуального устройства.

### S3-27. Проверить точное состояние при faults

1. Для каждого способа выхода сопоставить host PC/state map с faulting guest instruction и последней завершённой инструкцией.
2. Проверить GPR, flags, RIP, память и budget при faulting load/store/helper и при переходе к faulting fetch следующего блока.
3. Добавить negative tests на ошибочный early commit и потерянные live/spilled values. Unexpected host fault завершает изолированный process и даёт диагностику, а не выдуманный guest vector.
4. Если планируется direct-memory fast path, сначала отдельно доказать fault classification/recovery и ownership. До этого сохранять checked helper path.

**Готово:** все предусмотренные guest faults имеют тот же определённый snapshot, что interpreter и доступный physical oracle; runtime failures однозначно отделены.

#### S3-27b. Faulting next fetch after a committed two-site memory pair — partial

Native execution of either exact pair returns its canonical `A+6` CPU/RAM
snapshot. A separate interpreter step then fetches at this valid guest address
and receives a controlled architectural fault. The fixture compares against
two successful interpreter steps followed by the same faulting fetch, checks
full payload/state/RAM and confirms the faulting fetch is not retired or a
reason to repeat either pair memory effect. Unmapped/backend fetch failures
remain host errors. Exhausted budget and STOP gate continuation before fetch;
source `CodeView` exhaustion remains compile/decode metadata, not a guest fault.
This is fixture-boundary evidence only: the production owner-backed MMU and
multi-block dispatcher remain open.

См. [S3-27b report](../reports/STAGE_3_S3-27b_2026-10-27.md) and
[JIT testing protocol](../testing/JIT.md#bounded-s3-27b-next-fetch-after-memory-pair).

### S3-28. Реализовать STOP и event safepoints

1. Проверять cooperative STOP/events на документированных instruction/block/helper boundaries, сохраняя ограниченный work interval.
2. Для длинного блока гарантировать конечную задержку проверки; для REP/сложного fallback использовать только готовый restart/partial-progress contract этапа 2.
3. Согласовать total budget между blocks и interpreter. Проверить STOP до entry, внутри loop, рядом с helper/fault и при исчерпании budget.
4. Снять latency на representative busy path; не обещать заданные миллисекунды без измерения. Hardware timeout не считать точным гостевым STOP.

**Готово:** остановка возвращает корректное state и не теряет/повторяет side effects; ограничение задержки описано и проверено. Установка APIC/timers этим не заявляется.

## 8. Измеряемые улучшения

Для каждой задачи этого раздела сначала сохранить корректный baseline и сравнить минимальные варианты в одинаковых условиях. Записывать compile time, warm execution, code size, spills/helper calls и ресурсы, если они меняются. Выбор «по теории должно быть быстрее» не закрывает задачу.

### S3-29. Вычислять flags по потребности

1. На ограниченном integer subset представить pending flags с width, operation и достаточными исходными values. Не терять значения после перезаписи host registers.
2. Материализовать нужные bits при Jcc/ADC/SBB и все наблюдаемые определённые bits перед exit/helper/fallback/snapshot. Учитывать операции, которые изменяют только часть flags.
3. Проверить цепочки arithmetic → partial flag writer → consumer, spills, branch exits, fault и STOP. Сравнить с eager reference path и физическим x86.
4. Измерить eager и demand-driven варианты, включая стоимость snapshots/helpers. Выбрать default по данным; eager режим может сохраняться как диагностический эталон.

**Готово:** работающая demand-driven стратегия восстанавливает точное состояние; выбранная конфигурация имеет замеры. Если отложена сама реализация flags по потребности, соответствующее обязательное требование roadmap остаётся открытым, даже при успешном eager baseline.

### S3-30. Выбрать register allocator

1. На измеренных горячих workloads определить цену spills/reloads и helper clobbers текущего простого allocator.
2. Сравнить минимальные варианты: текущий baseline и простое улучшение, например локальное удержание values или linear scan, если именно это оправдано профилем.
3. Проверить correctness при максимальном давлении на registers, разных paths, pending flags и helper calls. Сравнить compile time и code size вместе с warm throughput.
4. Если усложнение не даёт устойчивой пользы, сохранить простой вариант и записать результат эксперимента.

**Готово:** выбран простой allocator, удовлетворяющий измеренным требованиям; решение воспроизводимо. Наличие сложного allocator не является критерием успеха само по себе.

### S3-31. Оценить прямые связи блоков

1. Сначала измерить долю dispatcher/lookup в representative multi-block workload. Переходы через dispatcher уже должны корректно работать.
2. Если direct linking обоснован, реализовать один ограниченный способ связи с проверенными lifetime и invalidation dependencies. Не patch-ить active code произвольно.
3. Проверить удаление target, invalidation source/target, context change, code-memory reuse и STOP/budget на linked path.
4. Сравнить latency, compile/link cost, code size и цену invalidation. При отсутствии пользы оставить dispatcher-only и документировать решение.

**Готово:** выбранный способ переходов проверен и измерен. Direct linking необязателен; корректные межблочные переходы обязательны.

### S3-32. Оценить локальные IR-преобразования

1. Выбрать только подтверждённые профилем возможности: local constant propagation, устранение избыточного state load/store или dead pure temporary.
2. Для каждой трансформации определить допустимые types/widths и запрет на изменение faults, guest instruction accounting, flags definedness и side-effect order.
3. Выполнить A/B correctness и timing с включённой/выключенной трансформацией, включая compile cost.
4. Не добавлять speculative loads, глобальный SSA или aggressive motion через guest fault points без отдельной необходимости и доказательства.

**Готово:** внедрены только полезные и корректные преобразования либо сохранён baseline с результатами сравнения. «Меньше IR operations» не заменяет измеренную пользу.

### S3-33. Расширять lowering по готовности interpreter

1. По таблице S3-00 выбирать следующую готовую integer/branch/memory форму. Отдельно подтверждать widths, flags, faults и helper effects.
2. Для сложных поддержанных форм использовать собственный helper/interpreter fallback с точной materialization, если native lowering ещё не обоснован.
3. REP, atomics, system instructions, x87/SIMD и иные расширения не переводить приблизительно. Перед включением дождаться соответствующей semantics и oracle evidence этапа 2.
4. Для каждого profile публиковать counts native/helper/fallback/unsupported и не менять CPUID из-за одного наличия lowering.

**Готово:** выбранный JIT subset шире только за счёт реально проверенных форм; весь interpreter ISA не является обязательным native lowering этапа 3.

## 9. Проверка, производительность и интеграция

### S3-34. Собрать corpus и проверить сам harness

**Файлы:** существующие testkit, CPU harness/reducer, новые малые stage3 fixtures.

1. Объединить ручные boundary cases, фиксированные generated seeds, multi-block/fallback/memory/SMC/context/eviction cases. Не ограничивать проверку четырьмя S0 snippets.
2. Добавить reducer для JIT mismatch с сохранением profile, memory layout, code writes, branch targets и класса ошибки. Сохранять original/reduced inputs и все backend results.
3. Контролируемо проверить, что стенд ловит неправильный GPR/flag/RIP/memory byte, stale block, неверную generation, пропущенный fault и несоответствие digest/fingerprint.
4. Разделить physical-equivalence cases и implementation lifecycle tests. Тест eviction не требует физического аналога cache, но guest-visible результат обязан сохраняться.
5. Для принятого native subset запускать ручной набор и минимум 1000 уникальных допустимых generated cases по нескольким зафиксированным seeds, каждый минимум дважды. Специальные SMC/fault/process cases вести отдельным bounded набором с явным покрытием.
6. Недоступный обязательный oracle, timeout, host crash и unsupported обязательной формы дают ненулевой итог. Невыбранные suites маркируются `not_run`.

**Готово:** одна документированная команда воспроизводит correctness принятого среза; mismatch сохраняется и сокращается. 1000 cases — минимальный объём этого плана, а не доказательство всей ISA.

#### S3-34a. Двухоперационный IR memory differential corpus

Для двух уже принятых S3-20j/S3-20k последовательностей добавлен bounded
`od-host` integration corpus. Три фиксированных seed дают 2,400 уникальных
полных успешных входов (400 на последовательность и seed); каждый выполняется
дважды native entry и interpreter на независимых CPU/RAM копиях. Проверяются
полное состояние, RAM, exit/budget/retirement и порядок/число callbacks.
Входы варьируют qword state, alignment, совпадающие/разные ranges и безопасные
частичные overlaps там, где форма последовательности допускает два адреса.
Отдельные named случаи охватывают guest fault, STOP, budget и continuation;
comparator corruption check проверяет GPR, RIP, byte памяти, retirement и
пропущенный fault. Это локальное ARM64 evidence и частичный этап S3-34: reducer,
остальные native subsets, lifecycle suites и физическая проверка сюда не входят.
Физический S3-21 для этого corpus: `not_run`. См.
[S3-34a report](../reports/STAGE_3_S3-34a_2026-10-27.md).

#### S3-34b. Versioned replay of one saved two-site mismatch

The `od-host` corpus report now carries a strict version-1 case envelope with
the exact selected sequence/source bytes, initial supported CPU and private RAM
snapshots, instruction budget, STOP/stop-RIP request, typed site-0 fault
injection and trigger, and optional budget-then-continuation protocol. The
report records host/architecture/build revision, backend availability, and
result snapshots for each execution phase; continuation mismatch reports keep
the budget-limited prefix even when only the resumed phase differs. The ignored
`replay_saved_case_from_environment` test
requires `OD_JIT_REPLAY_CASE`; replay validates the report before publishing
native code, compares fresh native/interpreter results, and writes a new
create-only replay report beside the original. Unknown versions/profiles,
inconsistent source/state/addresses, malformed snapshots, unsupported protocol,
or missing native support are explicit failures. Round-trip, both successful
sequences, fault/STOP/budget/stop-RIP/continuation, comparator classes, and
malformed reports are covered locally; the physical S3-21 gate remains
`not_run`. См. [S3-34b report](../reports/STAGE_3_S3-34b_2026-10-27.md).

#### S3-34c. Bounded reducer for the two saved memory profiles

The ignored `reduce_saved_case_from_environment` entry uses
`OD_JIT_REDUCE_CASE` and accepts only S3-34b version-1 envelopes for the two
existing memory sequences. It first repeats the saved mismatch twice through
fresh native/interpreter execution; a nonreproducing mismatch stops with an
explicit report and zero reduction attempts. Candidate changes are limited to
non-address initial GPR values and private RAM bytes. Deterministic passes
reduce 8-byte groups, individual bytes, then bits, accepting a candidate only
when a simple popcount complexity strictly decreases and all original mismatch
classes remain phase by phase (including exact GPR/RAM indices); an identical
class moving from the first entry to continuation is rejected. The attempt limit is capped;
the saved result includes original/reduced envelopes, both initial replay
observations, reduced observations, classes, attempt count and stop reason.
The original file is create-only preserved, and a reduced envelope is checked
through ordinary S3-34b replay. A clearly marked input-dependent GPR[10]
corruption gated by nonzero GPR[11] is used only in reducer self-tests; it is
not evidence of a JIT defect. The generic reducer has no special handling for
the marker register and can reduce it when the mismatch predicate allows.
Mac mini and physical x86 validation remain
separate and `not_run`. См. [S3-34c report](../reports/STAGE_3_S3-34c_2026-10-27.md).

### S3-35. Измерить холодное исполнение

1. Измерить полный первый запуск при пустом code cache: необходимые decode/IR/lowering/allocation/publication/lookup/execution, с явно указанной стоимостью initialization.
2. Отдельно снять составляющие pipeline, не складывая несовместимые independently calibrated samples как точный end-to-end результат.
3. Для каждого cold-cache sample гарантировать отсутствие готового block; повторное использование process/runtime ресурсов явно обозначить. Cold process/VMHost launch измерять отдельно, если он входит в scenario.
4. Сравнить с interpreter и прежним fragment path там, где inputs/semantics сопоставимы. Сохранить расходы на compile и overhead fallback.

**Готово:** cold результат не замаскирован warm reuse. Замедление первого запуска допустимо как измеренный факт и должно быть отражено; обещание ускорения первого исполнения без данных запрещено.

### S3-36. Измерить прогретое исполнение

1. Прогреть cache вне timed samples и подтвердить hit/compile counters. Повторять одинаковую гостевую работу с корректным reset initial state.
2. Измерять полный steady-state runtime path с lookup/dispatcher/helpers/flags и exits, а не только одну ARM64 arithmetic instruction.
3. Чередовать варианты, применять повторные calibrated samples, сохранять raw elapsed/work counts, median и разброс; при пересекающемся шуме не утверждать speedup.
4. Отдельно показывать branch-heavy, helper-heavy и memory workloads, долю fallback, code size и resources. При осмысленности вычислить break-even числа повторов с учётом cold cost.

**Готово:** ускорение или его отсутствие установлено для конкретных workloads. Если подтверждённого ускорения целевого прогретого сценария нет, correctness slice может быть принят отдельно, а performance-критерий этапа остаётся незакрытым.

### S3-37. Проверить cache churn и ресурсы

1. Выполнить длинную серию unique blocks, повторных invalidations, capacity evictions, SMC и mixed hit/miss workloads.
2. Снимать executable bytes, metadata bytes, live/retired blocks, compile counts и доступные process memory показатели с описанием метода.
3. Проверить освобождение после STOP/fault/destroy и несколько независимых runtime instances. Не смешивать peaks разных процессов с памятью одной VM.
4. Измерить стоимость invalidation/recompile и отсутствие неограниченного накопления code/metadata. Единичный короткий smoke не выдавать за leak-free stress.

**Готово:** capacity/lifetime сохраняются при длительной нагрузке, ресурсные ограничения и рост/разброс объяснены. Полный budget работающей гостевой ОС не заявляется.

### S3-38. Интегрировать runtime с VMHost

1. Подключить новый путь через существующий host ownership/lifecycle, сохранив старые публичные ABI guarantees. Новая версия ABI нужна только при реальной несовместимости и обновляется вместе со всеми consumers.
2. Проверить RUN/RESULT/STOP, повторные вызовы, destroy после завершения и корректное владение cache между requests.
3. Обеспечить независимость двух экземпляров: guest context, code cache и resources не смешиваются. Не использовать глобальный cache без требуемого isolation contract.
4. Повторить существующие control lifecycle/fault/process regressions и новый busy-JIT STOP. Host crash остаётся аварией helper, не гостевым исключением.

**Готово:** реальный VMHost исполняет принятый JIT slice и корректно завершает/очищает его. Это не означает готовность GUI или release distribution.

### S3-39. Повторить ARM64 smoke на M1 Pro

1. Зафиксировать реально доступные OS/toolchain, способ сборки/передачи собственного artifact и выбранную ad-hoc/Hardened Runtime конфигурацию.
2. Выполнить bounded arithmetic/multi-block/helper/STOP/publication-lifecycle smoke вне debugger. Проверить отсутствие необъявленных M3-specific host instruction assumptions.
3. Сохранить конфигурацию и результат. Если M1 недоступен, указать непроверенную конфигурацию и не объявлять широкий host support.

**Готово:** повторён заявленный host smoke. M1 Pro не заменяет x86 oracle и не закрывает Developer ID/notarization вопрос ISSUE-004.

### S3-40. Зафиксировать решения по измерениям

1. Для allocator, flags, lookup/cache, linking и внедрённых IR improvements записать сравниваемые варианты, inputs, условия, samples, correctness и результат.
2. Сопоставить cold/warm/resource tradeoffs; выбрать самый простой вариант, удовлетворяющий измеренным требованиям.
3. При недостаточных данных назвать решение предварительным. Не переносить успешные данные одного short fixture на весь JIT или гостевую ОС.
4. Архитектурно значимые решения оформить ADR в существующем каталоге, остальные — в отчёте и BENCHMARKS. Сохранить нужные малые raw data и команды повторения.

**Готово:** окончательные решения критичных участков основаны на данных. Усложнения без подтверждённой пользы удалены либо не внедрены; ограничения производительности видимы.

### S3-41. Оформить приёмку этапа

1. Создать STAGE_3_REPORT: exact profiles/forms, native/helper/fallback границы, revision/fingerprint, машины/toolchains, correctness counts, commands/seeds, cold/warm/resource результаты.
2. Обновить этот план, JIT_CONTRACT, JIT.md, BENCHMARKS и README затронутых компонентов по фактическому состоянию. CPU contract менять только при действительно изменившемся архитектурном поведении.
3. Зарегистрировать нерешённые дефекты в KNOWN_ISSUES: компонент, последствия, воспроизведение, статус и следующий шаг. Решённые закрыть regression evidence. Планируемые возможности не выдавать за обнаруженные баги.
4. Сверить матрицу ниже. Незакрытый prerequisite этапа 2/4 или hardware criterion не превращать в выполненную задачу изменением формулировки.
5. Если готов только срез A/B, назвать именно его. Первый JIT не обязан нативно переводить всю ISA, но обязательные runtime/correctness/performance условия выбранного этапного профиля должны быть выполнены.

**Готово:** одинаковые программы дают одинаковое определённое состояние interpreter/JIT/physical x86; cold и warm исполнение измерены отдельно, подтверждена польза на оговорённых целевых сценариях и честно описаны регрессии/ограничения.

## 10. Матрица приёмки

| Область | Обязательное доказательство | Что не засчитывается |
|---|---|---|
| IR/lowering/emitter | Реальный pipeline и native instructions, widths/effects/verifier/relocations проверены | IR dump без исполнения, весь workload в interpreter |
| ABI/state | Layout, helper clobbers, spill state и публичные regressions | Успешный trivial call без state проверки |
| Blocks/dispatcher | Multi-block/branch/loop/budget/STOP и fallback с прогрессом | Один immutable fragment |
| Cache | Реальные hits, context/dependency validation, capacity | Lookup только по RIP без контекста |
| Code changes | Aliases, cross-page dependencies, публикация после изменения, активный SMC | Только удаление entry из lookup |
| Lifetime | Eviction/invalidation не освобождают active code/metadata | Smoke без pressure и retirement cases |
| Faults/state maps | Точный guest RIP/state на предусмотренных exits, internal errors отдельно | Любой host signal как guest exception |
| Flags | Demand-driven path с точной materialization и измеренным default | Потеря flags при helper/STOP или только NZCV |
| Memory | Нужный interpreter/native prerequisite, тройное сравнение и effects | Synthetic snapshots или register-only oracle |
| Correctness harness | Handwritten boundaries, generated corpus, repeat/replay/reducer, negative self-checks | Совпадение только собственных helpers |
| Производительность | Отдельные cold/warm samples, comparable work, разброс, подтверждённый полезный сценарий | Один лучший sample или warm число с названием cold |
| Ресурсы | Capacity/churn/retirement evidence | Неограниченный рост cache |
| Интеграция | VMHost lifecycle и заявленные host configurations | Unit tests вместо process execution |
| Документация | Фактические contracts/report/issues и повторяемые команды | Финальный ответ в чате вместо evidence |

Direct linking и сложный allocator не являются самостоятельными условиями приёмки: допустим измеренный простой вариант. Demand-driven flags и прочие явно обязательные функции roadmap остаются условиями; их перенос означает частичный результат, если требования не были отдельно изменены владельцем.

Stage 3 не доказывает полную ISA, guest boot, MMU/APIC, SMP ordering/reclamation, AMD compatibility, GPU или release signing. Поддержанные профили и границы должны следовать из отчёта, а не из названия JIT.

## 11. Проверки и команды

Существующие команды до начала реализации нового runtime:

```sh
# Локальная проверка текущего принятого стенда.
python3 tests/cpu/stage1_harness.py --suite local

# Обязательный physical oracle для текущего S0-профиля.
python3 tests/cpu/stage1_harness.py --suite oracle --generated-count 1000 --seed 0x5eedcafe --repeats 2

# Текущие fixture entry points; поддержка ограничена их фактическим профилем.
cargo run --offline -p od-runner -- --fixture CASE.json --engine interpreter --json
cargo run --offline -p od-runner -- --fixture CASE.json --engine jit --json
```

`CASE.json` заменить реальным входом. Текущий oracle/fragment CLI сам по себе не проверяет будущий block cache, memory JIT или расширенный v2 профиль.

После Rust-изменений:

```sh
cargo fmt --all -- --check
cargo check --workspace --all-targets --offline
cargo test -p od-jit --offline
```

Дополнительно запускать tests затронутых od-isa/od-host/od-testkit/runners, явные CPU/VMHost scenarios и physical subset соответствующей задачи. Если затронут executable code/ABI, нужны реальные ARM64 execution и process regressions. `--offline` требует доступных dependencies; отсутствие cache не считать успешной проверкой.

Для benchmarks собирать выбранные сравниваемые варианты с одинаковыми release parameters. Не включать SSH, JSON serialization и fixture generation в CPU timing, если они не являются предметом конкретного end-to-end scenario. Существующий fragment `--benchmark` расширять с явным именованием новых cold/warm режимов, а не менять смысл исторического вывода молча.

Новые stage3 CLI flags, suites и benchmark commands документировать одновременно с работающей реализацией в JIT.md. До этого не приводить их как существующий успешный путь. Автоматическая проверка должна отличать native lowering, helper и fallback, а также обязательный skip от успешного результата.

При изменении структуры/путей и для документационных задач:

```sh
python3 tools/check_layout.py
git diff --check
```

После изменения кода выполнять `graphify update .`. Временные логи/корпуса хранить в `work/`; в Git — малые собственные fixtures, необходимые raw measurements и отчёты без секретов. Не запускать CPU-тесты ради изменения только этого плана.

## 12. Первое задание для следующей модели

> Выполни только S3-01 из `docs/plans/STAGE_3.md`. Используй [JIT contract](../specs/JIT_CONTRACT.md) и проверь, что нужные fixtures/harness позволяют измерить один и тот же корректный register-only workload для существующего interpreter, текущего fragment JIT и нового block JIT. Зафиксируй воспроизводимые входы, холодное и прогретое исполнение, измеряемые границы и ограничения; не заявляй ускорение без результатов. Сохрани параллельные изменения этапа 2. Перед кодом/изменением harness зафиксируй baseline исходной реализации и сравниваемые варианты; обнови методику и отчет только по фактически полученным данным.

## 13. Статус реализации этапа

S3-00 выполнена как документационная задача. Зафиксированы точные формы S0-09; текущий прямой JIT отделён от будущего block pipeline; пути interpreter fallback помечены как будущие; этап 2 и его текущие незакоммиченные изменения не объявлены завершёнными. Записаны начальные block bounds и ARM64/ABI/macOS источники.

S3-02 выполнена: [JIT_CONTRACT](../specs/JIT_CONTRACT.md#5-runtime-boundary-s3-02) задаёт canonical state pointer/offset boundary, progress/budget outputs, stop-address и STOP/budget/fetch порядок, block exit tags, разделение compile/runtime/host ошибок, scalar helper ABI, AAPCS64 clobbers и executable mapping ownership. `od-jit::runtime` содержит `repr(C)` context, raw tags и same-build offsets; arithmetic helper semantics и legacy fragment path не менялись. GuestFault/EventPending tags зарезервированы для будущей версии. Block compiler, dispatcher и новый host invocation ещё не реализованы.

S3-03 выполнена: добавлен [линейный typed IR](../../crates/od-jit/src/ir.rs) и verifier до будущей публикации кода. IR различает temporaries и guest GPR, сохраняет guest RIP/next RIP и commit markers, описывает integer/flags effects и требует явный terminal с enum condition/kind. Verifier проверяет temp definitions/widths, GPR/flag references, последовательность commits и targets: branch target разрешается только на boundary из полного непрерывного списка guest records, branch fallthrough и exit target должны совпадать с последним `next_rip`. Эта проверка не подтверждает источник guest records; декодерная валидация принадлежит S3-04/S3-05. Целевые unit tests и cargo checks прошли; независимый diff review не выявил actionable замечаний. IR не подключён к legacy fragment compiler и не реализует lowering/runtime dispatch.

S3-04 выполнена: добавила [bounded block builder](../../crates/od-jit/src/block.rs). Он использует `long_decode::decode` по одной инструкции и read-only borrowed `CodeView`, проверяет адресные вычисления и допускает entry на любой позиции buffer/ровно code_end. Стандартные limits ограничены 32 инструкциями и 256 байтами; S0-09 forms принимаются по decoder metadata и exact encoded length. `BasicBlock` содержит decoded records и точные byte ranges, включая диапазон через границу страницы. Terminator завершает построение до декодирования хвоста. Unsupported form, limits, buffer exhaustion и deferred `DecodeBoundary` различаются; первый fetch/decode failure возвращает `NoBlock` с отложенной записью, не guest `#PF`. Mid-instruction branch target останавливает построение перед branch и сохраняет предыдущий prefix; branch target вычисляется 64-битным wrapping сложением, как в interpreter. `next_rip` текущей Jcc считается известной sequential boundary без декодирования следующих bytes; другие unparsed targets хранятся как `Unresolved`. S3-05 не передаёт unresolved target verifier как доказанную ветвь: он исключает Jcc и завершает prefix `NeedInterpreter`; dispatcher resolution остаётся S3-15. Guest ranges не содержат code identity/version: tracking относится к S3-17, cache invalidation — к S3-23. Целевые проверки прошли, независимый review не нашёл замечаний.

S3-05 выполнена: `lowering::lower` переиспользует decoded metadata для точных MOV/ADD/SUB/CMP/JZ/JNZ effects и проверяет каждый полученный Program. Arithmetic inputs фиксируются до write; CMP не пишет GPR, MOV не пишет flags. Resolved branches завершают линейные effects terminal `Branch`; unresolved Jcc исключается, а prefix передаётся с `NeedInterpreter`, без-prefix представлен `NoNativePrefix`. `DecodeBoundary` сохраняется отдельно. `CodeEnd` прокинут из `BasicBlock` и принят verifier только как точный code_end; commit+branch selection определены как одна retirement boundary. Проверки прошли: `cargo fmt --all -- --check`, `cargo check -p od-jit --all-targets --offline`, `cargo test -p od-jit --offline` (35/35), `git diff --check`; независимое review чистое.

S3-06 выполнена: существующий fragment translator использует выделенный [`arm64::Encoder`](../../crates/od-jit/src/arm64/encoder.rs) с typed GPR/SP/ZR operands, checked immediate/offset ranges, encoder-owned local labels/fixups и transactional `finish`. Encoder покрывает текущие использованные MOVZ/MOVK, arithmetic, memory, pair, conditional/unconditional branch, call и return forms; LE serialization проверяется отдельно. Writeback pair base overlap и duplicate LDP destinations отвергаются; full legacy MOV fragment имеет exact word-vector regression. [Local assembly fixture](../../crates/od-jit/tests/fixtures/arm64_encoder.s) собран Apple clang 21.0.0 для `arm64-apple-macosx15.0` и сверяется в unit test. Проверки: `cargo fmt --all -- --check`, `cargo check -p od-jit --all-targets --offline`, `cargo test -p od-jit --offline` (42/42), `git diff --check`; независимое ревью закрытых исправлений — clean, assembler fixture совпадает с 27 словами. Lowering/emission pipeline, publication, ABI и allocator не входят в этот срез.

S3-01 начата: в [BENCHMARKS](../testing/BENCHMARKS.md) описаны workload matrix и раздельные уровни cold process, cold code cache и warm execution. Preflight точной ревизии `9a1376b` описан в [отчёте baseline](../reports/STAGE_3_BASELINE_2026-10-26.md): локальные Cargo tests показали пять x87 failures; удалённый `od-isa --lib` preflight завершился ошибкой без сохранённого списка test names. Физические fixtures и timing не запускались. Из-за параллельной работы этапа 2 этот результат не переносится на последующие revisions. Повторить Stage 1 local/oracle после стабилизации соответствующих изменений; performance baseline остаётся открытым.

S3-00 прошла `python3 tools/check_layout.py`, проверку относительных Markdown links и `git diff --check`. Эти проверки подтверждают документацию/структуру, не реализацию JIT. В S3-00 код JIT, CPU/SSH-прогоны и benchmarks не изменялись и не запускались.

#### S3-20bv. Address32 indexed SIB с RBP base и RSI/RDI index

Поддержать только singleton `67 48 8B/89 /r sib disp8|disp32`: ModRM `mod=01/10,r/m=100`, SIB base ровно RBP, index ровно RSI или RDI, scale 1/2/4/8, data RAX..RBP, lengths 6/9. `mod=00` с SIB base field 5 — no-base и вне профиля. Effective address остаётся decoder-owned `MemoryOperand::access_address_48`: `(low32(old RBP)+low32(old RSI/RDI)*scale+signed displacement) mod 2^32`, затем zero-extension; transfer и store payload остаются qword. Data=RBP использует pre-state для адреса/store payload; load меняет RBP только после успешного read. RSI/RDI остаются index-only и неизменны.

Приёмка: direct V2/shared IR/interpreter parity обеих операций × шести data GPR × двух indices × четырёх scales × signed disp8/disp32 edges; full CPU/RAM, независимый EA oracle, callback address/length/count и retirement. Проверить high-half truncation/wrap, contiguous qword crossing 4 GiB, final disp32 byte target identity, architectural/backend no-commit для обоих направлений, store+STOP, dispatcher entry STOP/stop-RIP/zero-budget, partial windows 1..5 и 1..8, unsupported neighbors и actual-RIP continuation по 6/9-byte singleton без replay. Не расширять generic register whitelist и другие профили. Отчёт: [S3-20bv report](../reports/STAGE_3_S3-20bv_2026-10-28.md). Production owner/MMU, Stage 2/ISA, Mini и performance вне среза.
