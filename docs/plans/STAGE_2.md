# Этап 2. Эталонный интерпретатор x86-64 — пошаговый план реализации

Последнее локальное подзадание S2-19t добавляет REPNE CMPSW/D/Q с addr32 после S2-19s REPNE SCASW/D/Q; S2-19m ранее добавило REPE SCASB на точные
address-size-32 формы; общий REP/JIT/native profile
и финальный Mac mini/SSH/native/physical acceptance не расширены. Dual-fault
policy остаётся ограниченной и описана в ISSUE-020/021; hardware priority не
устанавливается.

**Дата:** 23 октября 2026 года. **Статус:** выполняется; S2-00–14 приняты в указанных границах. Baseline integer forms строки S2-15 в CPU contract локально покрыты срезами S2-15a–h и register-register XCHG из S2-10. S2-16a–e локально покрывают direct relative Jcc/JMP, SETcc, CMOVcc и ограниченные register/memory-target near indirect JMP; S2-17a–t покрывают ограниченные GPR и memory PUSH/POP, near CALL/RET, а также выбранные PUSHF/POPF forms — см. [отчёт](../reports/STAGE_2_REPORT.md). S2-15 onward through S2-21g remains open until the shared physical Mac mini acceptance substage. S2-22a–k add local-only FS/GS-base, selector, descriptor-load, VERW/GDT, exact FS/GS.base composition and null DS/ES selector cases; S2-22 itself remains open. Existing deferred-pool accounting remains 614 physically selected fixtures; the 11 S2-22h load fixtures, two S2-22i/j composition fixtures and four S2-22k null DS/ES fixtures are local-only and are not native-eligible. Across the string-operation slices, native admission includes 21 mapped cases and rejects 10 local-only fault cases; all S2-20a/S2-20c divide-error fixtures and the S2-20b/S2-20d UD2 fixtures are excluded from native acceptance selection and execution. Detailed accounting of the initial 558 and subsequent additions is retained in the Stage 2 report. S2-17c/d/e combined-fault priority for a noncanonical CALL target with invalid stack remains unresolved in ISSUE-013; no dual-fault fixture is included. Combined RSP commit policy for memory-destination POP store fault remains open in ISSUE-014. FS/GS forms outside the exact local-only profiles remain unsupported. S2-21a–g cover the documented bounded forms; other widths, LOCK forms and fences remain open in S2-21. F3-prefixed LZCNT/TZCNT remain outside baseline. Stage 2 remains open under broader roadmap criteria. ISSUE-011/012/013/014/015/016/017/018 remain open; acceptance criteria are unchanged.

План рассчитан на модель, которой нужны небольшие последовательные задания, явные ограничения и проверяемый результат. Выполнять одну задачу `S2-NN` за раз. Большую группу внутри задачи делить на перечисленные подшаги; не реализовывать весь этап одним изменением.

Нормативный объём — [этап 2 дорожной карты](../ROADMAP.md#этап-2-эталонный-интерпретатор-x86-64). Ответственность компонентов определяют [карта проекта](../PROJECT_LAYOUT.md) и [архитектура](../ARCHITECTURE.md). Этот план развивает существующий [CPU contract](../specs/CPU_CONTRACT.md), а не создаёт параллельную архитектуру.

## 1. Что получить

Собственный интерпретатор исполняет выбранный и явно описанный профиль x86, сохраняет точное архитектурное состояние на наблюдаемых выходах и служит диагностическим эталоном для JIT. Для каждой поддержанной формы проверяются результат, определённые флаги, память, исключения и границы исполнения.

Основной путь:

```text
CpuState + профиль + интерфейс гостевой памяти + budget/events
  → fetch только нужных байтов по гостевому RIP
  → decode одной инструкции
  → вычисление операндов и архитектурные проверки
  → исполнение с определённым порядком side effects
  → следующий RIP / точное исключение / остановка
  → snapshot и сравнение с независимым источником
```

Порядок срезов:

1. **A:** 64-bit integer, 16 GPR, память данных, обычные переходы и стек.
2. **B:** строковые операции, REP, точные выходы и исключения, необходимые операции одного CPU.
3. **C:** режимы, сегменты, системные инструкции и CPU-часть событий выбранного пути загрузки.
4. **D:** x87, MMX, SSE/SSE2 и дополнительные обязательные расширения выбранного профиля.

Успех A не означает завершение B–D. Поддержка отдельной инструкции не означает поддержку всей группы, объявляемой одним CPUID feature bit.

## 2. Исходное состояние

На дату подготовки плана в репозитории:

- [Этап 1 принят](../reports/STAGE_1_REPORT.md) для register-only S0-09 subset. [Повторное ревью](../reports/STAGE_1_REVIEW_2026-10-22.md) подтверждает 10 ручных и 1000 generated cases, каждый исполнен дважды на M3 Max и физическом Mac mini, без расхождений в заявленной области сравнения.
- `crates/od-isa/src/lib.rs` содержит ограниченные decoder/interpreter: RAX/RCX/RDX/RBX, RIP, арифметические flags, формы MOV/ADD/SUB/CMP/JZ/JNZ. Это не полный x86-64 CPU.
- Текущий `CpuState` имеет `gpr: [u64; 4]`, `rip: usize`; decoder разбирает ограниченный code buffer. Прототип проверяет заранее декодированные границы переходов и останавливается на конце buffer.
- `od-jit` опирается на существующее состояние, в emitter есть явные смещения RIP/RFLAGS 32/40. `od-host` экспортирует отдельный `OdCpuStateV0` из четырёх GPR через C ABI. Изменение внутреннего state требует проверки всех потребителей.
- `od-testkit` уже содержит fixture/result v1, comparator, генератор, reducer и fingerprint. `od-runner` поддерживает `--fixture`, `--compare-results`, `--generate-corpus`, `--minimize`. Не создавать эту инфраструктуру заново.
- [CPU oracle](../testing/CPU_ORACLE.md) пока не исполняет memory operands, SIMD/x87 или privileged instructions. Его host signal observations не являются точной моделью гостевых исключений.
- Целевой CPUID/ISA-профиль ОС ещё не принят. [Требования гостевых ОС](../specs/GUEST_REQUIREMENTS.md) содержат кандидаты и непроверенные условия; они не доказывают возможность загрузки.

**Единственный физический x86 host:** Mac mini с Ubuntu, SSH `senya@192.168.1.14`. По результатам этапа 1: Intel Core i5-3210M, Ubuntu 25.10, Linux `6.17.0-41-generic`, страницы 4096 bytes, Rust/Cargo 1.99.1. Существующий отдельный checkout: `/home/senya/projects/open_desktop-stage1`. Перед новым прогоном перепроверить конфигурацию и revision; не считать старый checkout синхронизированным автоматически. Основной ARM host — M3 Max со страницами 16384 bytes. Подробности — в [STAND.md](../testing/STAND.md).

Тесты на этом Intel не подтверждают AMD и другие CPU. Не предполагать наличие расширения по возрасту или названию машины: проверять CPUID и доступность OS state. Второй x86 host план не предполагает.

Из известных ограничений отдельно остаются ISSUE-004 (поставка/подпись) и ISSUE-010 (graphify не извлекает публичный C header) в [KNOWN_ISSUES](../KNOWN_ISSUES.md). При работе с ABI читать header непосредственно после graphify-запроса; отсутствие символов в графе не означает отсутствие ABI. Эти проблемы не закрываются составлением плана CPU.

## 3. Правила исполнения для модели

1. Перед задачей прочитать корневой `AGENTS.md`, этот план, карту проекта и все вложенные `AGENTS.md` целевых каталогов. До изменения архитектурной границы свериться с ARCHITECTURE.
2. Проверить `git status --short`; не перезаписывать изменения пользователя. При наличии `graphify-out/graph.json` выполнить `graphify query "<вопрос текущей задачи>"` до поиска/чтения исходников; использовать найденные пути. Ограничения графа не заменяют проверку реальных потребителей интерфейса.
3. Выбрать ближайшую незавершённую задачу с выполненными зависимостями. Сначала довести её до проверяемого результата, затем двигаться дальше.
4. Для новой формы до реализации записать источник, кодировку, operand/address sizes, затронутые поля, defined/undefined flags, ошибки и порядок side effects. Использовать [checklist новой ISA-группы](../testing/ADDING_ISA_GROUP.md).
5. Источники — публичные Intel SDM/AMD APM и спецификации выбранного пути загрузки. Для применённой семантики фиксировать версию и точные разделы. Не копировать чужие decoder tables, emulator code, SoftFloat, готовое x87-ядро или runtime-JIT.
6. Production CPU semantics находятся в `od-isa`. Testkit и runners управляют тестами и наблюдением, но не содержат второго вычислителя ожидаемой x86-семантики.
7. Неподдерживаемая корректная x86-инструкция и архитектурно недопустимая кодировка — разные случаи. Не превращать пробел реализации в `#UD`, NOP или успешный fallback.
8. При каждой новой группе сначала interpreter ↔ физический oracle, затем интеграция с JIT только в уже доступном объёме. Новый JIT относится к этапу 3; не задерживать interpreter ради полного lowering.
9. Производительность не заявлять без замеров. Начать с понятных eager flags и обычного dispatch; lazy flags, decode cache, SIMD host shortcuts и генераторы таблиц требуют отдельной подтверждённой необходимости.
10. Не закрывать задачу только сборкой или synthetic snapshots. Неизвестные поля, skipped hardware и отсутствие исключения в наблюдении — ограничения, а не успешная проверка.
11. После задачи обновить статус и документацию, добавить реальные дефекты в KNOWN_ISSUES, выполнить проверки раздела 11. После изменения кода выполнить `graphify update .`.
12. В отчёте задачи указать ID, изменённые файлы, поддержанные формы, выполненные команды, ограничения и следующий ID. Не называть этап завершённым до S2-44.

## 4. Границы и решения, обязательные до кода

### 4.1. Профиль поддержки

В S2-00 расширить CPU contract таблицей:

```text
группа / точные формы / режимы / operand sizes / address sizes
  / нужный feature bit / реализация / источник
  / ручные tests / generated tests / независимая проверка
  / обязательность для выбранного профиля / ограничения
```

Отдельно зафиксировать первый диагностический long-mode профиль и требуемый CPU-профиль раннего Linux boot. Не придумывать окончательную модель Windows CPU до исследования требований. Условные режимы/расширения можно исключить из текущей приёмки только с записанным основанием, что выбранный путь их не требует. Отсутствие функции на Mac mini само по себе не является таким основанием.

Если выбор профиля требует ещё не полученных данных, подготовить A с явно ограниченным scope; не выдавать его за весь этап 2. Задачи C/D, обязательные по зафиксированному профилю, остаются открытыми до реализации и проверки.

### 4.2. Владельцы компонентов

| Ответственность | Место |
|---|---|
| CPU state, decode, операнды, interpreter, CPU exceptions | `crates/od-isa/src/` |
| Перевод linear → physical, RAM/MMIO, paging/TLB и доставка machine events | `crates/od-machine/`, преимущественно этап 4 |
| Минимальный bounded memory backend тестовых fixtures | `crates/od-testkit/src/`; реализует интерфейс CPU, не production MMU |
| Fixture/result, генерация, сравнение, сокращение | существующий `crates/od-testkit/` |
| Headless CLI и адаптация профилей | существующий `tools/runner/` |
| Физическое исполнение, host signal/context observation | существующий `tools/x86-runner/` |
| Межкомпонентные проверки | `tests/cpu/`, при необходимости `tests/system/` |
| Малые собственные byte fixtures | предлагаемый `tests/fixtures/x86/stage2/` |
| Нормативный CPU-профиль | существующий `docs/specs/CPU_CONTRACT.md` |
| Методика interpreter-проверок | предлагаемый `docs/testing/INTERPRETER.md` со ссылками на существующие oracle/format документы |
| Реальные результаты | будущий `docs/reports/STAGE_2_REPORT.md`, малые evidence в `docs/reports/data/stage2/` |
| Временные корпуса и traces | `work/tmp/stage2/<run-id>/` |

Не создавать новый top-level crate, общий `utils/` или вторую ISA-реализацию. Разбивать `od-isa/src/lib.rs` на модули по мере появления реального кода, например `state.rs`, `decode/`, `execute/`, `memory.rs`, `exception.rs`, `float/`. Не создавать всё дерево пустым заранее.

### 4.3. Что относится к соседним этапам

- Расширение ARM64 JIT, блоки, dispatcher, cache, lazy flags и оптимизации — этап 3. В этапе 2 обязательны сохранение старого профиля и честный отказ JIT для новых форм.
- Production paging/TLB, APIC/IOAPIC, таймеры, физическая шина и загрузка Linux — этап 4. CPU-часть CR/MSR/mode transitions и exceptions делается здесь, её memory/event integration проверяется по мере появления machine backend.
- SMP ordering и конкурентные locked operations — этап 6. Однопроцессорные эффекты необходимых атомарных инструкций реализовать здесь; они не доказывают SMP.
- UEFI, firmware, устройства, Windows boot, GUI и release signing не добавлять ради CPU-теста.

Граница ответственности не даёт права оставить фиктивную функцию: требующий отсутствующего machine backend сценарий получает явную зависимость и статус blocked/not verified, а не успешный ответ.

### 4.4. Архитектурное состояние и совместимость

- Гостевые значения имеют фиксированные widths: GPR и RIP целевого 64-bit профиля — `u64`. `usize` используется для проверенных индексов host-буфера после преобразования и bounds checks.
- Реализовать 16 GPR с доступами 8/16/32/64, high-byte registers и правилами REX. Запись 32-bit subregister в long mode очищает верхние биты; остальные правила сверять по форме и режиму.
- Состояние сегментов, control registers, MSR, x87/SIMD появляется вместе с реальным поведением соответствующей задачи. Не выдавать заполненные нулями поля за поддержку режима.
- `OdCpuStateV0`, control protocol и fixture v1 не расширять молча. Явные adapters сохраняют прежний четырёхрегистровый профиль; неподходящее новое состояние ими отклоняется без потери старших регистров.
- Убирать магические JIT offsets только вместе с проверкой реального layout/adapter. Не менять 4 на 16 глобальным поиском без проверки emitter, native trampoline, JSON и ABI.
- Для новых fixtures ввести следующую версию схемы и профиль, сохранив strict v1 loading и исторические digests. Не переписывать старые evidence так, будто они проверяли новый CPU.

### 4.5. Fetch, память и точность

Декодировать одну инструкцию по текущему RIP; fetch запрашивает только необходимые байты через ограниченный интерфейс. Не читать 15 bytes заранее и не проверять недостижимый хвост программы. Переход на ранее не декодированный адрес разрешён, если там находится допустимая инструкция; старые ограничения S0 buffer остаются только в compatibility adapter.

В CPU-границе различать fetch/read/write, размер, адрес и контекст режима/привилегий. Интерфейс возвращает данные или определённый architectural fault; ошибка host mapping/ABI остаётся внутренней ошибкой. Тестовый flat mapping не доказывает x86 paging и не должен самовольно изобретать page-fault codes.

Для каждой инструкции фиксировать момент commit регистров, flags, RIP и памяти. Не применять общее «откатить весь state при любой ошибке»: REP и некоторые сложные операции имеют архитектурно наблюдаемый частичный прогресс. Не повторять MMIO side effects при восстановлении. На старте работать с fixture RAM; MMIO подключать только с реальным machine contract.

Исключения, завершение, HLT/wait, budget, cooperative STOP, unsupported и harness failure — разные выходы. Возврат из `step` не должен скрывать fault в успешном результате.

## 5. Таблица задач

Все флажки ниже относятся к реализации, а не к готовности плана. Для необязательной по S2-00 задачи допустима отдельная запись `not_required` с основанием; не ставить `[x]` за отсутствующую реализацию.

| Статус | ID | Задача | Зависимости |
|---|---|---|---|
| [x] | S2-00 | Зафиксировать профиль и нормативные источники; AMD fetch ограничен ISSUE-011 | Этап 1 |
| [x] | S2-01 | Закрепить baseline и границы совместимости | S2-00 |
| [x] | S2-02 | Расширить state и subregister access | S2-01 |
| [x] | S2-03 | Расширить fixture/result без поломки v1 | S2-02 |
| [x] | S2-04 | Ввести CPU memory/fault contract | S2-02 |
| [x] | S2-05 | Ввести step и явные выходы | S2-04 |
| [x] | S2-06 | Декодировать prefix/opcode/width в принятом decode-only срезе | S2-05 |
| [x] | S2-07 | Декодировать ModR/M/SIB/адреса в принятом decode-only срезе | S2-06 |
| [x] | S2-08 | Исполнить первый MOV через память; local working slice | S2-03, S2-07 |
| [x] | S2-09 | Расширить native oracle на GPR/память в проверенном положительном срезе | S2-08 |
| [x] | S2-10 | MOV extensions, LEA и register exchange в проверенном срезе | S2-09 |
| [x] | S2-11 | Арифметика и eager flags в проверенном срезе | S2-10 |
| [x] | S2-12 | Логические операции/TEST/NOT и semantic definedness в проверенном срезе | S2-11 |
| [x] | S2-13 | Сдвиги и вращения | S2-12 |
| [x] | S2-14 | Умножение и деление: успешные формы и физическое сравнение | S2-13 |
| [ ] | S2-15 | Bit operations и прочие integer формы профиля | S2-14 |
| [ ] | S2-16 | Условия, переходы, SETcc/CMOVcc | S2-11 |
| [ ] | S2-17 | Стек, CALL/RET и косвенные переходы | S2-16 |
| [ ] | S2-18 | Строковые операции без REP; S2-18a–n локально покрывают MOVS/STOS/LODS/SCAS/CMPS bounded формы и CLD/STD; общая приёмка открыта | S2-17 |
| [ ] | S2-19 | REP и точный частичный прогресс; S2-19a–m покрывают bounded формы и продолжение в локальных границах; остальные формы и общая приёмка открыты | S2-18 |
| [ ] | S2-20 | Исключения, приоритеты и fault state | S2-14, S2-19 |
| [ ] | S2-21 | Нужные однопроцессорные atomic/fence формы | S2-20 |
| [ ] | S2-22 | Сегменты и права доступа | S2-20 |
| [x] | S2-22a | Только `64 48 8B 00` FS-base load в новом local-only profile; источник MOV: Intel SDM rev.093 Vol.2B (253667-093US), pp.4-28–4-31; S2-22 остаётся открытым | S2-21g |
| [x] | S2-22b | Только `65 48 8B 00` GS-base load в отдельном local-only profile, тот же нормативный MOV source; S2-22 остаётся открытым | S2-22a |
| [x] | S2-22c | Только `48 8C E0` (`MOV RAX, FS`) в local-only profile с наблюдаемыми FS.base и selector; Intel SDM rev.093 Vol.2B MOV pp.4-28–4-31; descriptor/privilege semantics остаются открытыми | S2-22b |
| [x] | S2-22d | Только `48 8C E8` (`MOV RAX, GS`) в отдельном local-only profile с наблюдаемыми GS.base и selector; zero-extend, descriptor lookup/load и privilege checks не входят | S2-22c |
| [x] | S2-22e | Только bare `0F 00 E8` (`VERW AX`) на CPL3, GDT-only local profile; read-only 8-byte descriptor lookup, ZF rights check, table bounds and descriptor-origin memory context. S2-27 explicitly excludes this extracted form; S2-22 itself remains open | S2-22d, S2-20 |
| [x] | S2-22f | Только bare `8E E0` (`MOV FS, AX`) в Long64/CPL3, AX non-null GDT selector; descriptor-origin read и точные #GP/#NP priorities/codes; A=0 пока не меняет состояние | S2-22e |
| [x] | S2-22g | В том же local-only профиле для valid P=1/A=0 выполнить один byte write только по адресу access byte до CPU commit; A=1 не пишет; native profile остаётся fail-closed | S2-22f |
| [x] | S2-22h | Только bare `8E E8` (`MOV GS, AX`) в отдельном `s2-long-gs-load-v1`: Long64/CPL3, non-null GDT selector, тот же упорядоченный GDT validation/A-write контракт, наблюдаемый GS cache; S2-22 остаётся открытым | S2-22g |
| [x] | S2-22i | Один literal local-only composition case: exact `8E E8 65 48 8B 00`, loaded GS.base consumed by an already-supported GS qword load; bounded profile admits only existing `8E E8` and this exact two-instruction program | S2-22h |
| [x] | S2-22j | Один literal local-only composition case: exact `8E E0 64 48 8B 00`, loaded FS.base consumed by an already-supported FS qword load; bounded profile admits only existing `8E E0` and this exact two-instruction program | S2-22i |
| [x] | S2-22k | Exact bare `8E D8` (`MOV DS, AX`) and `8E C0` (`MOV ES, AX`) with AX=0 at Long64/CPL0; selector/unusable state only, no descriptor lookup, separate local-only profile; S2-22 remains open | S2-22j |
| [ ] | S2-23 | Real mode 16-bit, если нужен пути загрузки | S2-22 |
| [ ] | S2-24 | Protected/compatibility mode | S2-22; S2-23 при необходимости |
| [ ] | S2-25 | CR/MSR и переходы режима | S2-24 |
| [ ] | S2-26 | CPUID, системные инструкции и время | S2-25 |
| [ ] | S2-27 | Remaining descriptor/event/syscall CPU semantics; the bounded VERW/GDT check is extracted to S2-22e | S2-25, S2-26 |
| [ ] | S2-28 | Представить x87 state | S2-20 |
| [x] | S2-28a | Lossless Long64 x87 state and local NOP/FNINIT/FNCLEX/FNSTSW AX-only state profile; no general x87 execution or FPU feature claim | S2-28 |
| [ ] | S2-29 | Собственное ядро extended precision | S2-28 |
| [x] | S2-29a | Lossless binary80 raw fields and read-only raw encoding classification; no arithmetic or x87 execution | S2-28a |
| [x] | S2-29b | Integer-only finite binary80 normalization with source-class provenance and right-shift-jam/GRS bit helpers; no rounding or arithmetic | S2-29a |
| [x] | S2-29c | Generic `u128` right-shift-jam preserving 64-bit significands plus three low GRS bits; literal boundary and normalized-finite bridge vectors; no rounding or arithmetic | S2-29b |
| [x] | S2-29m | Bounded packing of already-rounded nonzero normal finite significands with one carry and finite normal exponent range; no subnormal/special packing or architectural effects | S2-29l |
| [x] | S2-29p | Round an exact signed `i128` coefficient once to finite binary80 through lossless normalization and direct normal/subnormal packing; no x87 architectural effects | S2-29e, S2-29f, S2-29k–n |
| [ ] | S2-30 | Основные x87 операции | S2-29 |
| [x] | S2-30a | Bare `DB E3` FNINIT with existing explicit x87 state; reset environment only, payloads preserved, native closed | S2-28a |
| [x] | S2-30b | Bare `DF E0` FNSTSW AX with existing x87 state; copy pre-operation raw FSW, preserve local x87 snapshot by deterministic policy, native closed | S2-28a |
| [x] | S2-30c | Bare `DB E2` FNCLEX with existing x87 state; clear FSW[0:7] and FSW[15], preserve C0–C3 by local policy, native closed | S2-28a |
| [x] | S2-30d | Bare `D8 C1` FADD ST(0), ST(1), exact same-exponent canonical normal results only; native closed | S2-29q, S2-28a |
| [x] | S2-30e | Bare `D8 C0+i` FADD ST(0), ST(i), i=0..7; same exact-only domain, native closed | S2-30d |
| [x] | S2-30f | Bare `DC C0+i` FADD ST(i), ST(0), same exact-only domain, native closed | S2-30e |
| [x] | S2-30g | Bare `DE C0+i` FADDP ST(i), ST(0), same exact-only domain plus physical TOP pop, native closed | S2-30f |
| [x] | S2-30h | Bare `D8 E0+i` FSUB ST(0), ST(i), exact-only same-exponent normal results; i=0 unsupported; native closed | S2-30e |
| [x] | S2-30i | Bare `DC E8+i` FSUB ST(i), ST(0), exact-only same-exponent normal results; i=0 alias unsupported; native closed | S2-30h |
| [x] | S2-30j | Bare `DE E8+i` FSUBP ST(i), ST(0), same exact-only domain plus physical TOP pop; i=0 alias unsupported; native closed | S2-30i |
| [x] | S2-30l | Bare `DC E0+i` FSUBR ST(i), ST(0), exact-only ST(0)−ST(i) into physical ST(i), no pop; i=0 alias unsupported; native closed | S2-30i |
| [x] | S2-30m | Bare `DE E0+i` FSUBRP ST(i), ST(0), exact-only ST(0)−ST(i) into old ST(i), then physical TOP pop; i=0 alias unsupported; native closed | S2-30l |
| [x] | S2-30n | Bare `D8 C8+i` FMUL ST(0), ST(i), exact binary80 result only, no pop; native closed | S2-30m, S2-29 |
| [x] | S2-30o | Bare `DC C8+i` FMUL ST(i), ST(0), exact binary80 result only, no pop; native closed | S2-30n |
| [x] | S2-30p | Bare `DE C8+i` FMULP ST(i), ST(0), exact binary80 result then physical TOP pop; native closed | S2-30o |
| [x] | S2-30q | Bare `D8 F0+i` FDIV ST(0), ST(i), exact canonical Normal quotient only; native closed | S2-30p, S2-29 |
| [x] | S2-30r | Bare `DE F8+i` FDIVP ST(i), ST(0), exact canonical Normal quotient then physical TOP pop; native closed | S2-30q |
| [x] | S2-30s | Bare `DE F0+i` FDIVRP ST(i), ST(0), exact `ST(0)/ST(i)` into old ST(i), then physical TOP pop; native closed | S2-30r |
| [x] | S2-30t | Bare `D8 F8+i` FDIVR ST(0), ST(i), exact `ST(i)/ST(0)` into old TOP without pop; native closed | S2-30s |
| [x] | S2-30u | Bare `DC F0+i` FDIVR ST(i), ST(0), exact `ST(0)/ST(i)` into physical ST(i) without pop; native closed | S2-30t |
| [x] | S2-30v | Bare `DC F8+i` FDIV ST(i), ST(0), exact `ST(i)/ST(0)` into physical ST(i) without pop; native closed | S2-30u |
| [x] | S2-30w | Bare `DD D8+i` FSTP ST(i), historical canonical-Normal source copy/pop; signed-zero limit superseded by S2-30as; native closed | S2-30v |
| [x] | S2-30x | Bare `DD D0+i` FST ST(i), canonical Normal raw copy without TOP change; native closed | S2-30w |
| [x] | S2-30y | Bare `D9 E0` FCHS, bounded canonical-Normal ST(0) sign-bit flip; native closed | S2-30x |
| [x] | S2-30z | Bare `D9 E1` FABS, bounded canonical-Normal ST(0) sign-bit clear; native closed | S2-30y |
| [x] | S2-30ap | Bare `D9 E1` FABS additionally admits canonical Zero-tagged signed zero; at that time FTST/FST gates remained Normal-only (superseded by S2-30aq/ar) | S2-30ao |
| [x] | S2-30aq | Bare `D9 E4` FTST additionally admits canonical Zero-tagged signed zero as equal; at that point FST/FSTP ST(i) were Normal-only (historical; superseded by S2-30ar/as); native/JIT closed | S2-30ap |
| [x] | S2-30at | Bare `D9 E4` FTST additionally admits Special-tagged canonical signed binary80 infinity; compares by sign; JIT/native closed | S2-30aq |
| [x] | S2-30ar | Bare `DD D0+i` FST ST(i) also copies exact Zero-tagged signed zero, retaining Zero without pop; FSTP's then-Normal-only scope is historical / superseded by S2-30as; native/JIT closed | S2-30aq |
| [x] | S2-30as | Bare `DD D8+i` FSTP ST(i) copies exact Zero-tagged signed zero before emptying old TOP and advancing TOP; native/JIT closed | S2-30ar |
| [x] | S2-30au | Bare `DD D0+i`/`DD D8+i` FST/FSTP ST(i) additionally copy Special-tagged canonical signed infinity; native/JIT closed | S2-30as |
| [x] | S2-30av | Bare `D9 E0` FCHS and `D9 E1` FABS additionally admit only Special-tagged canonical signed binary80 infinity; native/JIT closed | S2-30ap, S2-30at |
| [x] | S2-30aw | Bare `D9 E0` FCHS and `D9 E1` FABS additionally admit only Special-tagged canonical binary80 QuietNaN, preserving all payload bits; native/JIT closed | S2-30av, S2-31ah |
| [x] | S2-30ay | Bare `D9 E0` FCHS and `D9 E1` FABS additionally admit Special-tagged canonical binary80 subnormals with raw payload preservation; native/JIT closed | S2-30ax |
| [x] | S2-30bc | Bare D9 E0/E1 FCHS/FABS preserve historical sticky IE for six existing source classes; reject IE with other exception/summary bits; native/JIT closed | S2-30ay, S2-31ah |
| [x] | S2-30bd | Bare D9 E0/E1 FCHS/FABS additionally preserve historical DE independently or with IE; reject all other exception/summary bits; native/JIT closed | S2-30bc, S2-31ah |
| [x] | S2-30be | Bare D9 E0/E1 FCHS/FABS additionally preserve historical PE with IE/DE; only IE/DE/PE combinations admitted; native/JIT closed | S2-30bd, S2-31ah |
| [x] | S2-30bf | Bare D9 E0/E1 FCHS/FABS additionally preserve historical UE with IE/DE/PE; admit all IE/DE/UE/PE combinations, reject ZE/OE/SF/ES/B; local only, native/physical deferred | S2-30be |
| [x] | S2-30bg | Bare D9 E0/E1 FCHS/FABS additionally preserve historical OE with IE/DE/UE/PE; admit all 32 sticky exception combinations, reject ZE/SF/ES/B; local only, native/physical deferred | S2-30bf, S2-31bp, S2-31bq |
| [x] | S2-30bh | Bare D9 E0/E1 FCHS/FABS additionally preserve historical ZE; admit all 64 IE/DE/ZE/OE/UE/PE histories; reject SF/ES/B; local only, final acceptance deferred | S2-30bg |
| [x] | S2-30ba | Bare `DD D0+i`/`DD D8+i` FST/FSTP ST(i) additionally copy Special-tagged canonical binary80 SignalingNaN, preserving Q=0 and raw payload; native/JIT closed | S2-30az, S2-31ab |
| [x] | S2-30bb | Bare `DD D0+i`/`DD D8+i` FST/FSTP ST(i) additionally copy only Special-tagged canonical binary80 subnormals with unchanged bytes/tag; native/JIT closed | S2-30ba, S2-31x |
| [x] | S2-30az | Bare `DD D0+i`/`DD D8+i` FST/FSTP ST(i) additionally copy Special-tagged canonical binary80 QuietNaN without changing raw bytes; native/JIT closed | S2-30au, S2-30aw |
| [x] | S2-30aa | Bare `D9 E4` FTST, bounded canonical-Normal ST(0) comparison with zero; native closed | S2-30z |
| [x] | S2-30ab | Bare `D8 D1` FCOM ST(1), exact finite canonical-Normal compare with status-only commit; native closed | S2-30aa |
| [x] | S2-30ac | Bare `D8 D0+i` FCOM ST(i), exact finite canonical-Normal compare for all register indices; native closed | S2-30ab |
| [x] | S2-30ad | Bare `D8 D9` FCOMP ST(1) alias, exact compare then physical TOP pop; native closed | S2-30ac |
| [x] | S2-30ae | Bare `D8 D8+i` FCOMP ST(i), exact compare then physical TOP pop for all indices; native closed | S2-30ad |
| [x] | S2-30af | Bare `DE D9` FCOMPP, exact bounded ST(0)/ST(1) compare then two-entry physical TOP pop; native closed | S2-30ae |
| [x] | S2-30ag | Bare `DD E1` FUCOM ST(0), ST(1), bounded canonical-Normal compare without pop; native closed | S2-30af |
| [x] | S2-30ah | Bare `DD E9` FUCOMP ST(0), ST(1), bounded canonical-Normal compare then one physical TOP pop; native closed | S2-30ag |
| [x] | S2-30ai | Bare `DA E9` FUCOMPP, bounded canonical-Normal compare then two physical TOP pops; native closed | S2-30ah |
| [x] | S2-30aj | Bare `DB F1` FCOMI ST(0), ST(1), bounded canonical-Normal compare to RFLAGS; native closed | S2-30ai |
| [x] | S2-30ak | Bare `DF F1` FCOMIP ST(0), ST(1), bounded canonical-Normal compare to RFLAGS and one physical TOP pop; native closed | S2-30aj |
| [x] | S2-30al | Bare `DB E9` FUCOMI ST(0), ST(1), bounded canonical-Normal compare to RFLAGS; native closed | S2-30ak |
| [x] | S2-30am | Bare `DF E9` FUCOMIP ST(0), ST(1), bounded canonical-Normal compare to RFLAGS and one physical TOP pop; native closed | S2-30al |
| [x] | S2-30an | Bare indexed DB/DF register FCOMI-family forms for ST(i), bounded canonical-Normal compare and optional physical TOP pop; native closed | S2-30am |
| [x] | S2-30bu | Bare `DA C1` FCMOVB ST(0), ST(1), both participants preflighted; exact raw conditional copy, local C1=0 gate, native/JIT closed | S2-30an |
| [x] | S2-30bv | Bare `DA C0..C7` FCMOVB ST(0), ST(i), indexed source preflight, self-index checks only TOP; exact raw conditional copy, local C1=0 gate, native/JIT closed | S2-30bu |
| [x] | S2-30bw | Bare `DB C1` FCMOVNB ST(0), ST(1), same strict preflight gate, exact raw copy when CF=0, native/JIT closed | S2-30bv |
| [x] | S2-30bx | Bare `DB C0..C7` FCMOVNB ST(0), ST(i), indexed source preflight with self-index TOP-only, exact raw copy when CF=0, native/JIT closed | S2-30bw |
| [x] | S2-30ca | Historical bare `DB C9` FCMOVNE ST(0), ST(1) slice, superseded by indexed S2-30cb; native/JIT closed | S2-30bx |
| [x] | S2-30cb | Bare `DB C8..CF` FCMOVNE ST(0), ST(i), indexed preflight before ZF and self-index TOP-only, exact raw copy when ZF=0, native/JIT closed | S2-30ca |
| [x] | S2-30cc | Exact bare Long64 `DA D1` FCMOVBE ST(0), ST(1); preflight both participants before integer `CF || ZF`; full local matrices, six-stream schema and fixture runner; native/JIT closed | S2-30cb |
| [x] | S2-30cd | Bare Long64 `DA D0..D7` indexed FCMOVBE ST(0), ST(i); preflight before integer `CF || ZF`, TOP-only self-case, exact raw copy; indexed matrices, 20-stream schema and full-state runner; native/JIT closed | S2-30cc |
| [x] | S2-30ce | Historical exact bare Long64 `DB D1` FCMOVNBE ST(0), ST(1); preflight before integer CF/ZF, raw copy only when both clear; success/invalid matrices, six-stream schema and full-state runner; native/JIT closed | S2-30cd |
| [x] | S2-30cf | Bare Long64 `DB D0..D7` indexed FCMOVNBE ST(0), ST(i); validate TOP/source before CF/ZF, self-index TOP-only, raw copy when both clear; full matrices, 20-stream schema and full-state runner; native/JIT closed | S2-30ce |
| [x] | S2-30cg | Historical exact bare `DA D9` FCMOVU ST(0), ST(1) slice; 512 success and 768 invalid class/tag snapshots, superseded by indexed S2-30ch | S2-30cf |
| [x] | S2-30ch | Bare Long64 `DA D8..DF` indexed FCMOVU; preflight TOP/source before PF, self-index TOP-only, exact raw copy only when PF=1; full indexed matrices, 18-stream schema and literal full-state fixtures; unordered/NaN and native/JIT closed | S2-30cg |
| [x] | S2-30ci | Historical exact bare Long64 `DB D9` FCMOVNU ST(0), ST(1); superseded by indexed S2-30cj | S2-30ch |
| [x] | S2-30cj | Bare Long64 `DB D8..DF` indexed FCMOVNU; preflight TOP/source before PF, self-index TOP-only, raw copy when PF=0; 363,520 success and 161,280 reject snapshots, 18-stream schema and literal fixtures; native/physical acceptance deferred | S2-30ci |
| [x] | S2-30ao | Admit only canonical Zero-tagged signed zero to bare `D9 E0` FCHS; its then-Normal-only FTST/FST/FSTP scopes are historical and superseded by S2-30aq/ar/as; native/JIT closed | S2-30y, S2-31u, S2-31v |
| [ ] | S2-31 | x87 conversions, rounding и exceptions | S2-30 |
| [x] | S2-31dc | Admit canonical binary80 signed zero to bounded D9 FC FRNDINT with unchanged bytes and Zero tag; keep native/JIT closed | S2-31a, S2-31w |
| [x] | S2-31dd | Admit canonical Normal binary80 exact integers with e=63..16383 to D9 FC; preserve raw bytes/status except C1; native closed | S2-31dc |
| [x] | S2-31de | Admit canonical signed infinity with Special tag to D9 FC; preserve raw bytes/status except C1; native/JIT closed | S2-31dd, S2-31aa |
| [x] | S2-31df | Admit Special-tagged canonical QNaN to D9 FC as a payload-preserving identity; native/JIT closed | S2-31de |
| [x] | S2-31dg | Admit canonical Special-tagged SNaN to D9 FC; set IE and quiet bit while preserving payload; native/JIT closed | S2-31df |
| [x] | S2-31dh | Admit historical sticky IE at bounded D9 FC while preserving independent current IE/PE effects; native/JIT closed | S2-31dg |
| [x] | S2-31dj | Admit historical sticky DE at bounded D9 FC while preserving DE and current rounding/exception effects; native/JIT closed | S2-31dh |
| [x] | S2-31dk | Admit historical sticky UE at bounded D9 FC while preserving UE and independent current PE/IE effects; native/JIT closed | S2-31dj |
| [x] | S2-31dm | Admit historical sticky ZE at bounded D9 FC while preserving independent current PE/IE effects; native/JIT closed | S2-31dl |
| [x] | S2-31dn | Admit historical SF only together with IE at bounded D9 FC; preserve both while recomputing C1; native/JIT closed | S2-31dm |
| [x] | S2-31di | Admit canonical Normal binary80 fractions with e=-16382..-1 to D9 FC via comparison-based rounding; produce signed Zero or Valid ±1 with PE/C1; native/JIT closed | S2-31dh |
| [x] | S2-31dp | Admit Special-tagged binary80 pseudo-denormals through bounded D9 FC with subnormal rounding, DE+PE and canonical-Normal equivalence; native/JIT closed | S2-31do |
| [x] | S2-31dq | Admit Special-tagged binary80 unnormals through bounded D9 FC as masked invalid; native/JIT closed | S2-31dp |
| [x] | S2-31dr | Admit only Special-tagged binary80 pseudo-infinities through bounded D9 FC as masked invalid; native/JIT closed | S2-31dq |
| [x] | S2-31ds | Admit Special-tagged binary80 pseudo-NaNs (both Q-bit classes) through bounded D9 FC as masked invalid; native/JIT closed | S2-31dr |
| [x] | S2-31dt | Admit historical sticky UE only in exact bare D9 00/DD 00 FLD m32/m64 gates; keep other x87 gates and native/JIT closed | S2-31ak |
| [x] | S2-31du | Admit historical sticky OE only in exact bare D9 00/DD 00 FLD m32/m64 gates; keep other x87 gates and native/JIT closed | S2-31dt |
| [ ] | S2-31b | Bare bounded FISTP m32int [RAX] DB 18, partial slice | S2-31a |
| [x] | S2-31c | Bare bounded FIST m32int [RAX] DB 10, partial slice | S2-31b |
| [x] | S2-31d | Bare bounded FISTP m16int [RAX] DF 18, partial slice | S2-31c |
| [x] | S2-31e | Bare bounded FIST m16int [RAX] DF 10, partial slice | S2-31d |
| [x] | S2-31f | Bare bounded FISTP m64int [RAX] DF 38, partial slice | S2-31e |
| [x] | S2-31g | Bare bounded FNSTCW m2byte [RAX] D9 38, snapshot-preserving Word store; native closed | S2-31f |
| [x] | S2-31h | Bare bounded FNSTSW m2byte [RAX] DD 38, raw-FSW snapshot-preserving Word store; native closed | S2-31g |
| [x] | S2-31i | Bare bounded FLDCW m2byte [RAX] D9 28, validated Word load and local RC control; native closed | S2-31h |
| [x] | S2-31j | Bare bounded FSTP m32fp [RAX] D9 18, integer binary80-to-binary32 conversion and atomic Dword store; native closed | S2-31i |
| [x] | S2-31k | Bare bounded FST m32fp [RAX] D9 10, shared binary80-to-binary32 conversion and atomic Dword store without pop; native closed | S2-31j |
| [x] | S2-31l | Bare bounded FSTP m64fp [RAX] DD 18, shared binary80-to-binary64 conversion and atomic Qword store with pop; native closed | S2-31k |
| [x] | S2-31m | Bare bounded FST m64fp [RAX] DD 10, shared binary80-to-binary64 conversion and atomic Qword store without pop; native closed | S2-31l |
| [x] | S2-31q | Bare bounded FSTP m80fp [RAX] DB 38, exact ten-byte binary80 store and pop; native closed | S2-31p |
| [x] | S2-31r | Bare bounded FLD m80fp [RAX] DB 28, canonical-Normal ten-byte load and push; native closed | S2-31q |
| [x] | S2-31s | Bare bounded FLD m32fp [RAX] D9 00, exact normal binary32-to-binary80 expansion and push; native closed | S2-31r |
| [x] | S2-31t | Bare bounded FLD m64fp [RAX] DD 00, exact normal binary64-to-binary80 expansion and push; native closed | S2-31s |
| [x] | S2-31u | Extend S2-31t bare DD 00 to signed binary64 zero with Zero tag; keep nonzero subnormals and specials unsupported, native closed | S2-31t |
| [x] | S2-31x | Extend exact DB 28 FLD m80fp to canonical nonzero binary80 subnormals with Special tag; native closed | S2-31w |
| [x] | S2-31y | Extend exact D9 00 FLD m32fp to masked binary32 subnormal sources with exact Normal expansion and sticky DE; native closed | S2-31v |
| [x] | S2-31z | Extend exact DD 00 FLD m64fp to masked binary64 subnormal sources with exact Normal expansion and sticky DE; native closed | S2-31u |
| [x] | S2-31aa | Extend exact DB 28 FLD m80fp to canonical signed binary80 infinities with Special tag; native closed | S2-31x |
| [x] | S2-31ab | Extend exact DB 28 FLD m80fp to canonical quiet binary80 NaNs with Special tag; native closed | S2-31aa |
| [x] | S2-31ac | Extend exact DB 28 FLD m80fp to canonical signaling binary80 NaNs under all-masked FCWs; native closed | S2-31ab |
| [x] | S2-31ad | Extend exact DB 28 FLD m80fp to raw pseudo-denormals with Special tag and no DE; native closed | S2-31ac |
| [x] | S2-31ae | Extend exact DD 00 FLD m64fp to signed binary64 infinities with canonical binary80 Special; native closed | S2-31z |
| [x] | S2-31af | Extend exact D9 00 FLD m32fp to signed binary32 infinities with canonical binary80 Special; native closed | S2-31y |
| [x] | S2-31ag | Extend exact DD 00 FLD m64fp to canonical binary64 QNaNs with binary80 payload-preserving Special; native/JIT closed | S2-31ae |
| [x] | S2-31ah | Extend exact D9 00 FLD m32fp to canonical binary32 QNaNs with binary80 payload-preserving Special; native/JIT closed | S2-31af |
| [x] | S2-31ai | Extend exact DD 00 FLD m64fp to canonical binary64 SNaNs with integer quieting and sticky IE; native/JIT closed | S2-31ag |
| [x] | S2-31aj | Extend exact D9 00 FLD m32fp to canonical binary32 SNaNs with integer quieting and sticky IE; native/JIT closed | S2-31ah |
| [x] | S2-31ak | Permit sticky IE/DE/PE only on exact D900/DD00 FLD gates; retain all other status gates | S2-31aj |
| [x] | S2-31al | Extend exact D910/D918 m32 stores to canonical signed binary80 zero and atomic signed binary32 zero | S2-31ak |
| [x] | S2-31am | Extend exact DD10/DD18 m64 stores to canonical signed binary80 zero and atomic signed binary64 zero | S2-31al |
| [x] | S2-31an | Extend exact DB38 FSTP m80fp to canonical signed binary80 zero with raw ten-byte store | S2-31am |
| [x] | S2-31ao | Extend exact DB38 FSTP m80fp to canonical signed binary80 infinity with raw ten-byte store | S2-31an |
| [x] | S2-31ap | Extend exact DD10/DD18 m64 stores to canonical signed binary80 infinity and binary64 infinity | S2-31am |
| [x] | S2-31au | Convert Special-tagged canonical binary80 QuietNaN through exact DD10/DD18 stores with payload truncation | S2-31ap, S2-31at |
| [x] | S2-31av | Convert Special-tagged canonical binary80 QuietNaN through exact D910/D918 stores with binary32 payload truncation | S2-31aq, S2-31au |
| [x] | S2-31aw | Convert Special-tagged canonical binary80 SignalingNaN through exact DD10/DD18 stores, quieting binary64 and setting sticky IE | S2-31au |
| [x] | S2-31ax | Convert Special-tagged canonical binary80 SignalingNaN through exact D910/D918 stores, quieting binary32 and setting sticky IE | S2-31av, S2-31aw |
| [x] | S2-31ay | Admit masked sticky IE only in exact D910/D918 and DD10/DD18 stores; preserve sticky IE/PE and successful SNaN status | S2-31ax |
| [x] | S2-31az | Admit masked sticky DE in the same bounded m32/m64 FST/FSTP helpers; retain IE/DE/PE and keep subnormal store results unsupported | S2-31ay |
| [x] | S2-31ba | Admit masked sticky IE/DE in exact raw DB38 FSTP m80fp stores; preserve bytes and pop atomically | S2-31az |
| [x] | S2-31bb | Extend exact D910/D918 finite binary32 stores through exponent 127; reject rounded exponent 128 before access | S2-31az, S2-31av |
| [x] | S2-31bc | Extend exact DD10/DD18 finite binary64 stores through exponent 1023; reject rounded exponent 1024 before access | S2-31ba, S2-31bb |
| [x] | S2-31bd | Store exact m32 subnormal results from canonical Normal binary80 sources with integer downshift only | S2-31bb, S2-31bc |
| [x] | S2-31be | Store exact m64 subnormal results from canonical Normal binary80 sources with integer downshift only | S2-31bc, S2-31bd |
| [x] | S2-31bf | Round masked inexact D910/D918 m32 subnormal results; reject zero and minimum-normal promotion | S2-31bd |
| [x] | S2-31bg | Round masked inexact DD10/DD18 m64 subnormal results; reject zero and minimum-normal promotion | S2-31be |
| [x] | S2-31bh | Allow sticky UE in the bounded m32/m64 FST/FSTP gates without synthesizing PE from UE alone | S2-31bg |
| [x] | S2-31bi | Allow sticky UE in the raw DB38 FSTP m80fp gate without generating flags | S2-31bh |
| [x] | S2-31bj | Round extreme binary32 underflow directly to signed zero or minimum subnormal | S2-31bf, S2-31bh |
| [x] | S2-31bk | Round extreme binary64 underflow directly to signed zero or minimum subnormal | S2-31bg, S2-31bh |
| [x] | S2-31bl | Round Special-tagged canonical binary80 subnormals through D910/D918 to signed zero or minimum binary32 subnormal | S2-31bj, S2-31bh |
| [x] | S2-31bm | Round Special-tagged canonical binary80 subnormals through DD10/DD18 to signed zero or minimum binary64 subnormal | S2-31bk, S2-31bh |
| [x] | S2-31bn | Accept direct D910/D918 rounding from binary80 Normal exponent -127 to signed minimum binary32 normal with independent UE rounding | S2-31bf, S2-31bh |
| [x] | S2-31bo | Accept direct DD10/DD18 rounding from binary80 Normal exponent -1023 to signed minimum binary64 normal with independent UE rounding | S2-31bg, S2-31bh |
| [x] | S2-31bp | Accept D910/D918 exponent-127 rounding carry to signed binary32 infinity with sticky OE+PE and C1 | S2-31bb |
| [x] | S2-31bq | Accept DD10/DD18 exponent-1023 rounding carry to signed binary64 infinity with sticky OE+PE and C1 | S2-31bc, S2-31bh |
| [x] | S2-31br | Admit prior sticky IE/DE/OE/UE/PE on bounded m32/m64 stores while preserving conversion-specific flags | S2-31bq |
| [x] | S2-31bs | Admit prior sticky OE on exact raw DB38 FSTP m80fp stores without changing source bytes or generating flags | S2-31bi, S2-31bp, S2-31bq |
| [x] | S2-31bt | Admit canonical signed binary80 zero for DB10/DB18 FIST/FISTP m32int as exact integer zero | S2-31b, S2-31c |
| [x] | S2-31bu | Admit canonical signed binary80 zero for DF10/DF18 FIST/FISTP m16int as exact integer zero | S2-31d, S2-31e |
| [x] | S2-31bv | Admit canonical signed binary80 zero for DF38 FISTP m64int as exact integer zero | S2-31d, S2-31e |
| [x] | S2-31bw | Round canonical Normal binary80 fractions below one through DB10/DB18 m32 integer stores | S2-31bt |
| [x] | S2-31bx | Round canonical Normal binary80 fractions below one through DF10/DF18 m16 integer stores | S2-31bu |
| [x] | S2-31by | Round canonical Normal binary80 fractions below one through DF38 m64 FISTP | S2-31bv |
| [x] | S2-31bz | Convert canonical masked binary80 infinity to integer indefinite for DB10/DB18 m32 FIST | S2-31bw |
| [x] | S2-31ca | Convert canonical masked binary80 infinity to integer indefinite for DF10/DF18 m16 FIST | S2-31bx |
| [x] | S2-31cb | Convert canonical masked binary80 infinity to integer indefinite for DF38 m64 FISTP | S2-31by |
| [x] | S2-31cc | Convert canonical masked binary80 quiet NaN to integer indefinite for DB10/DB18 m32 FIST/FISTP | S2-31bz |
| [x] | S2-31cd | Convert canonical masked binary80 quiet NaN to integer indefinite for DF10/DF18 m16 FIST/FISTP | S2-31ca |
| [x] | S2-31ce | Convert canonical masked binary80 quiet NaN to integer indefinite for DF38 m64 FISTP | S2-31cb |
| [x] | S2-31cf | Convert canonical masked binary80 signaling NaN to integer indefinite for DB10/DB18 m32 FIST/FISTP | S2-31cc |
| [x] | S2-31cg | Convert canonical masked binary80 signaling NaN to integer indefinite for DF10/DF18 m16 FIST/FISTP | S2-31cd |
| [x] | S2-31ch | Convert canonical masked binary80 signaling NaN to integer indefinite for DF38 m64 FISTP | S2-31ce |
| [x] | S2-31ci | Admit sticky IE on bounded DB10/DB18/DF10/DF18/DF38 integer stores | S2-31br, S2-31ch |
| [x] | S2-31cj | Convert finite rounded i32 overflow to integer-indefinite for DB10/DB18 | S2-31ci |
| [x] | S2-31ck | Convert finite rounded i16 overflow to integer-indefinite for DF10/DF18 | S2-31ci, S2-31cj |
| [x] | S2-31cl | Convert finite rounded i64 overflow to integer-indefinite for DF38 | S2-31ci, S2-31cj, S2-31ck |
| [x] | S2-31cm | Admit sticky DE on bounded integer stores after masked FLD subnormal composition | S2-31ci, S2-31cl |
| [x] | S2-31cn | Admit sticky UE on the five bounded integer-store forms | S2-31cm |
| [x] | S2-31co | Admit sticky OE on the five bounded integer-store forms | S2-31cn |
| [x] | S2-31cp | Admit historical sticky ZE on the five bounded integer-store forms | S2-31co |
| [x] | S2-31cq | Admit Special-tagged canonical binary80 subnormals only through DB10/DB18 m32 FIST/FISTP | S2-31cp |
| [x] | S2-31cr | Admit Special-tagged canonical binary80 subnormals only through DF10/DF18 m16 FIST/FISTP | S2-31cq |
| [x] | S2-31cs | Admit Special-tagged canonical binary80 subnormals only through DF38 m64 FISTP | S2-31cr |
| [x] | S2-31ct | Convert Special-tagged binary80 pseudo-infinity through DB10/DB18 m32 FIST/FISTP | S2-31cs |
| [x] | S2-31cu | Convert Special-tagged binary80 pseudo-infinity through DF10/DF18 m16 FIST/FISTP | S2-31ct |
| [x] | S2-31cw | Convert Special-tagged binary80 pseudo-NaNs through DB10/DB18 m32 FIST/FISTP | S2-31cv |
| [x] | S2-31cx | Convert Special-tagged binary80 pseudo-NaNs through DF10/DF18 m16 FIST/FISTP | S2-31cw |
| [x] | S2-31cz | Convert Special-tagged binary80 unnormals through DB10/DB18 m32 FIST/FISTP | S2-31cy |
| [x] | S2-31da | Convert Special-tagged binary80 unnormals through DF10/DF18 m16 FIST/FISTP | S2-31cz |
| [x] | S2-31db | Convert Special-tagged binary80 unnormals through DF38 m64 FISTP | S2-31da |
| [ ] | S2-32 | Остальные обязательные x87 формы | S2-31 |
| [ ] | S2-33 | MMX и совместное состояние с x87 | S2-32 |
| [ ] | S2-34 | SSE state, moves и logic | S2-20 |
| [x] | S2-34.1 | Lossless local-only Long64 x87+SSE snapshot transfer: 16 raw XMM registers and raw MXCSR; no instruction semantics or hardware claim | S2-28a |
| [x] | S2-34.5 | Separate local-only legacy `MOVUPS [RAX],XMM` store profile; one exact 16-byte write with no CPU commit on backend failure | S2-34.4 |
| [x] | S2-34.6a | Shared local-only MOVUPS base+displacement profile for legacy `0F 10`/`0F 11`, no SIB/RIP-relative | S2-34.5 |
| [x] | S2-34.6b | Extend `s2-long-movups-mem-base-v1` with bounded legacy RIP-relative `0F 10`/`0F 11` disp32 forms | S2-34.6a |
| [x] | S2-34.6c | Extend `s2-long-movups-mem-base-v1` with shared SIB-addressed legacy `0F 10`/`0F 11` forms in address-size-64 mode | S2-34.6b |
| [x] | S2-34.7a | Separate local-only legacy XORPS register-to-register diagnostic profile; raw 128-bit XOR only | S2-34.6c |
| [x] | S2-34.7b | Separate local-only legacy ANDPS register-to-register diagnostic profile; raw 128-bit AND only | S2-34.7a |
| [x] | S2-34.7c | Separate local-only legacy ORPS register-to-register diagnostic profile; raw 128-bit OR only | S2-34.7b |
| [x] | S2-34.7d | Separate local-only legacy ANDNPS register-to-register diagnostic profile; raw 128-bit `(~old DEST) & SRC` | S2-34.7c |
| [x] | S2-34.8a | Separate local-only legacy MOVSS register-to-register profile; raw low dword copy with upper 96 destination bits preserved | S2-34.7d |
| [x] | S2-34.8b | Separate local-only legacy reverse-direction MOVSS register profile; ModR/M.reg source, r/m destination | S2-34.8a |
| [x] | S2-34.8c | Separate local-only legacy MOVSS m32 load from `[RAX]`; read exactly four bytes and clear destination bits 127:32 | S2-34.8b |
| [x] | S2-34.8d | Separate local-only legacy MOVSS m32 store to `[RAX]`; write four bytes from ModR/M.reg and commit RIP only on success | S2-34.8c |
| [x] | S2-34.9a | Separate local-only legacy scalar-double MOVSD register-to-register profile; copy raw low qword and preserve upper qword | S2-34.8d |
| [x] | S2-34.9b | Separate local-only reverse-direction legacy scalar-double MOVSD register profile; ModR/M.reg source, r/m destination, preserve upper qword | S2-34.9a |
| [x] | S2-34.9c | Separate local-only legacy scalar-double MOVSD m64 load from `[RAX]`; read exactly eight bytes and clear destination upper qword | S2-34.9b |
| [x] | S2-34.9d | Separate local-only legacy scalar-double MOVSD m64 store to `[RAX]`; write exactly eight bytes from source low qword | S2-34.9c |
| [ ] | S2-35 | Packed integer SSE/SSE2 | S2-34 |
| [x] | S2-35.3 | Local-only bounded register PSUBB XMM diagnostic slice; prerequisite S2-34.1 + S2-35.1 + reviewed S2-35.2 | S2-34.1, S2-35.1, S2-35.2 review |
| [x] | S2-35.4 | Local-only bounded register PADDUSB XMM diagnostic slice; prerequisite S2-34.1 + S2-35.1 + reviewed S2-35.2 | S2-34.1, S2-35.1, S2-35.2 review |
| [x] | S2-35.5 | Local-only bounded register PSUBUSB XMM diagnostic slice; prerequisite S2-34.1 + S2-35.1 + reviewed S2-35.3 | S2-34.1, S2-35.1, S2-35.3 review |
| [x] | S2-35.6 | Local-only bounded register PADDW XMM diagnostic slice; prerequisite S2-34.1 + S2-35.1 mandatory-66 decoder + reviewed S2-35.2 wrapping-add infrastructure | S2-34.1, S2-35.1, S2-35.2 review |
| [x] | S2-35.7 | Local-only bounded register PSUBW XMM diagnostic slice; prerequisite S2-34.1 + S2-35.1 mandatory-66 decoder + reviewed S2-35.6 word-lane representation + S2-35.3 wrapping-subtraction/order patterns | S2-34.1, S2-35.1, S2-35.6 review, S2-35.3 review |
| [x] | S2-35.8 | Local-only bounded register PADDUSW XMM diagnostic slice; prerequisite S2-34.1 + S2-35.1 mandatory-66 decoder + reviewed S2-35.6 word-lane representation + S2-35.4 unsigned saturation | S2-34.1, S2-35.1, S2-35.6 review, S2-35.4 review |
| [x] | S2-35.9 | Local-only bounded register PSUBUSW XMM diagnostic slice; prerequisite S2-34.1 + S2-35.1 mandatory-66 decoder + reviewed S2-35.7 word-lane subtraction/layout + S2-35.5 unsigned saturation-to-zero | S2-34.1, S2-35.1, S2-35.7 review, S2-35.5 review |
| [x] | S2-35.10 | Local-only bounded register PADDSB XMM diagnostic slice; prerequisite S2-34.1 + S2-35.1 + reviewed S2-35.2 byte-add state infrastructure + S2-35.4 saturation coverage | S2-34.1, S2-35.1, S2-35.2 review, S2-35.4 review |
| [x] | S2-35.11 | Local-only bounded register PSUBSB XMM diagnostic slice; prerequisite S2-34.1 + S2-35.1 + reviewed S2-35.3 byte-subtract order infrastructure + S2-35.10 signed saturation coverage | S2-34.1, S2-35.1, S2-35.3 review, S2-35.10 review |
| [x] | S2-35.12 | Local-only bounded register PADDSW XMM diagnostic slice; prerequisite S2-34.1 raw SSE state, S2-35.1 mandatory-66 decode and reviewed S2-35.6 word-lane representation | S2-34.1, S2-35.1, S2-35.6 review |
| [x] | S2-35.13 | Local-only bounded register PSUBSW XMM diagnostic slice; prerequisite S2-34.1 raw SSE state, S2-35.1 mandatory-66 decode and reviewed S2-35.6 word-lane representation | S2-34.1, S2-35.1, S2-35.6 review |
| [x] | S2-35.14 | Local-only bounded register PADDD XMM diagnostic slice; wrapping dword lanes, native/JIT closed | S2-34.1, S2-35.1 |
| [x] | S2-35.15 | Local-only bounded register PSUBD XMM diagnostic slice; wrapping dword subtraction, native/JIT closed | S2-34.1, S2-35.1 |
| [x] | S2-35.16 | Local-only bounded register PADDQ XMM diagnostic slice; wrapping qword lanes, native/JIT closed | S2-34.1, S2-35.1 |
| [x] | S2-35.17 | Local-only bounded register PSUBQ XMM diagnostic slice; destination-minus-source wrapping qword lanes, native/JIT closed | S2-34.1, S2-35.1, S2-35.16 review |
| [x] | S2-35.1 | Local-only PXOR XMM register diagnostic slice; child prerequisite S2-34.1 raw x87/SSE state only | S2-34.1 |
| [x] | S2-35.2 | Local-only PADDB XMM register diagnostic slice; requires S2-34.1 state and reviewed S2-35.1 mandatory-66 register decoder | S2-34.1, S2-35.1 review |
| [x] | S2-35.18 | Local-only bounded register PCMPEQB XMM diagnostic slice; byte equality masks, native/JIT closed | S2-34.1, S2-35.1 |
| [x] | S2-35.19 | Local-only bounded register PCMPEQW XMM diagnostic slice; word equality masks, native/JIT closed | S2-34.1, S2-35.1 |
| [x] | S2-35.20 | Local-only bounded register PCMPEQD XMM diagnostic slice; dword equality masks, native/JIT closed | S2-34.1, S2-35.1 |
| [x] | S2-35.21 | Local-only bounded register PCMPGTB XMM diagnostic slice; signed byte greater-than masks, native/JIT closed | S2-34.1, S2-35.1 |
| [x] | S2-35.22 | Local-only bounded register PCMPGTW XMM diagnostic slice; signed word greater-than masks, native/JIT closed | S2-34.1, S2-35.1 |
| [x] | S2-35.23 | Local-only bounded register PCMPGTD XMM diagnostic slice; signed dword greater-than masks, native/JIT closed | S2-34.1, S2-35.1 |
| [x] | S2-35.24 | Local-only bounded register PUNPCKLBW XMM diagnostic slice; low-byte interleave, native/JIT closed | S2-34.1, S2-35.1 |
| [x] | S2-35.25 | Local-only bounded register PUNPCKHBW XMM diagnostic slice; high-byte interleave, native/JIT closed | S2-34.1, S2-35.1 |
| [x] | S2-35.26 | Local-only bounded register PUNPCKLWD XMM diagnostic slice; low-word interleave, native/JIT closed | S2-34.1, S2-35.1 |
| [x] | S2-35.27 | Local-only bounded register PUNPCKHWD XMM diagnostic slice; high-word interleave, native/JIT closed | S2-34.1, S2-35.1 |
| [x] | S2-35.28 | Local-only bounded register PUNPCKLDQ XMM diagnostic slice; low-dword interleave, native/JIT closed | S2-34.1, S2-35.1; independent review |
| [x] | S2-35.29 | Local-only bounded register PUNPCKHDQ XMM diagnostic slice; high-dword interleave, native/JIT closed | S2-34.1, S2-35.1; independent review |
| [x] | S2-35.30 | Local-only bounded register PUNPCKLQDQ XMM diagnostic slice; low-qword interleave, native/JIT closed | S2-34.1, S2-35.1 |
| [x] | S2-35.31 | Local-only bounded register PUNPCKHQDQ XMM diagnostic slice; high-qword interleave, native/JIT closed | S2-34.1, S2-35.1; independent review |
| [x] | S2-35.32 | Local-only bounded register PACKSSWB XMM diagnostic slice; signed word-to-byte saturation, native/JIT closed | S2-34.1, S2-35.1; independent review |
| [x] | S2-35.33 | Local-only bounded register PACKUSWB XMM diagnostic slice; signed-word to unsigned-byte saturation, native/JIT closed | S2-34.1, S2-35.1; independent review |
| [x] | S2-35.34 | Local-only bounded register PACKSSDW XMM diagnostic slice; signed-dword to signed-word saturation, native/JIT closed | S2-34.1, S2-35.1; independent review |
| [x] | S2-35.35 | Local-only bounded register PSHUFD XMM diagnostic slice; imm8 dword shuffle, native/JIT closed | S2-34.1, S2-35.1; independent review |
| [x] | S2-35.36 | Packed-integer and qword logical shift coverage and acceptance inventory; no ISA/profile expansion | S2-34.10, existing S2-35 local evidence |
| [x] | S2-35.37 | Local-only bounded register PSLLW immediate profile; native/JIT closed | S2-34.1, S2-35.1; independent review |
| [x] | S2-35.38 | Local-only bounded register PSRLW immediate profile; native/JIT closed | S2-34.1, S2-35.1; independent review |
| [x] | S2-35.39 | Local-only bounded register PSRAW immediate profile; native/JIT closed | S2-34.1, S2-35.1; independent review |
| [x] | S2-35.41 | Local-only bounded register PSLLD immediate profile; native/JIT closed | S2-34.1, S2-35.1; independent review |
| [x] | S2-35.44 | Local-only bounded register PSLLQ immediate profile; native/JIT closed | S2-34.1, S2-35.1; independent review |
| [x] | S2-35.45 | Local-only bounded register PSRLQ immediate profile; native/JIT closed | S2-34.1, S2-35.1; independent review |
| [x] | S2-35.46 | Local-only bounded register PSLLDQ immediate profile; native/JIT closed | S2-34.1, S2-35.1; independent review |
| [x] | S2-35.47 | Local-only bounded register PSRLDQ immediate profile; native/JIT closed | S2-34.1, S2-35.1; independent review |
| [x] | S2-35.51 | Local-only bounded register POR profile; native/JIT closed | S2-34.1, S2-35.1 |
| [x] | S2-35.42 | Local-only bounded register PSRLD immediate profile; native/JIT closed | S2-34.1, S2-35.1; independent review |
| [x] | S2-35.43 | Local-only bounded register PSRAD immediate profile; native/JIT closed | S2-34.1, S2-35.1; independent review |
| [ ] | S2-36 | Собственная binary32/binary64 семантика | S2-29, S2-34 |
| [ ] | S2-37 | Scalar/packed SSE FP и conversions | S2-35, S2-36 |
| [ ] | S2-38 | MXCSR, FP exceptions и порядок faults | S2-37 |
| [ ] | S2-39 | FP/SIMD save/restore | S2-33, S2-38 |
| [ ] | S2-40 | Дополнительные обязательные расширения | S2-26, S2-39 |
| [ ] | S2-41 | Независимо проверить системный профиль | S2-27; machine backend по сценариям |
| [ ] | S2-42 | Расширить corpus/reducer/harness | S2-21, S2-39, S2-40 |
| [ ] | S2-43 | Диагностика и контракт для JIT | S2-42; S2-41 для системного профиля |
| [ ] | S2-44 | Приёмка и документация | Все обязательные задачи профиля |

Проверки и генерация добавляются с каждой группой; S2-42 объединяет готовые наборы, а не откладывает тесты до конца. Независимые ветки допустимо выполнять последовательно в удобном порядке после их prerequisites. Критический первый срез: `00 → 01 → 02 → 03/04 → 05 → 06 → 07 → 08 → 09`.

## 6. Задачи первого среза

### S2-00. Принять проверяемый профиль

**Статус:** выполнено для диагностического A; целевой Ubuntu-кандидат уточнён как Generic amd64. Нормативное прочтение его ABI/ISA floor — x86-64 psABI baseline (x86-64-v1); уровни v2/v3/v4 не принимаются. Это не утверждает полный CPU/CPUID/MSR/OS boot profile: C/D, точные source-to-binary требования и наблюдаемое исполнение остаются открытыми. См. [отчёт](../reports/STAGE_2_REPORT.md#s2-00--профиль) и [Linux profile evidence](../reports/STAGE_2_LINUX_PROFILE.md#решение-по-generic-amd64-baseline).

**Файлы:** CPU_CONTRACT, GUEST_REQUIREMENTS по фактическим уточнениям, будущий INTERPRETER.

1. Прочитать отчёт/ревью этапа 1 и текущий checklist новой ISA. Составить таблицу раздела 4.1 и список источников с версиями.
2. Определить обязательные режимы, widths, ISA-группы и system requirements раннего Linux пути; неопределённые данные пометить и указать способ получения. Не объявлять Windows profile готовым по одному ISO.
3. Разнести обязательное для A–D, условное по boot path и последующие этапы. Для каждого system case указать, что проверяется через CPU callback и что требует machine backend.
4. В INTERPRETER записать scope, порядок срезов и политику остановки/исключений. Источником поддерживаемых форм остаётся CPU_CONTRACT.

**Готово:** есть конкретный профиль и перечень условий приёмки. Если принят только A, оставшиеся обязательные решения видимы; весь этап не считается завершённым.

### S2-01. Закрепить baseline и контракт adapters

**Статус:** выполнено; local baseline и 1010 × 2 физических сравнений прошли до изменения state. См. [отчёт](../reports/STAGE_2_REPORT.md#s2-01--исходный-baseline-и-границы).

**Файлы:** od-isa, od-jit, od-host, testkit, runners только в части необходимых проверок/документации.

1. Выполнить существующий local и обязательный oracle наборы этапа 1; сохранить revision/fingerprint и результат. При недоступном SSH подготовить локальную часть, но не считать hardware baseline проверенным.
2. Найти всех потребителей CpuState/Instruction/Operation: JIT offsets, C ABI, Swift bridge, native trampoline, comparator, generator, reducer. Ничего не переносить ради предпочтения структуры.
3. Описать, как старый S0 entry, buffer-end exit и четырёхрегистровый ABI адаптируются к расширенному core. Сохранить один источник архитектурной семантики.

**Готово:** сохранён воспроизводимый baseline и известен список совместимых границ. Нет молчаливого изменения ABI/schema или заявленной поддержки новых инструкций старым JIT.

### S2-02. Расширить state и subregister access

**Статус:** реализовано состояние LongModeState и строгий fragment adapter; локальные проверки прошли. Новые формы исполнения не заявлены. См. [отчёт](../reports/STAGE_2_REPORT.md#s2-02--состояние-и-subregister-access).

**Предлагаемые модули:** `od-isa/src/state.rs`, локальные tests; adapters у существующих потребителей.

1. Ввести все 16 GPR и архитектурный RIP нужной ширины; определить именование регистров и обращения к subregisters без host-layout assumptions.
2. Реализовать read/write 8/16/32/64, AH/CH/DH/BH, запреты и изменения с REX, сохранение/очистку верхних bits.
3. Сохранить старые ABI и fixture adapters. Обновить необходимый внутренний JIT layout либо отдельное явное представление fragment state; проверить offsets тестами, а не комментариями.

**Готово:** tests покрывают все GPR, aliasing, крайние значения и REX/high-byte случаи. Старые interpreter/JIT/native fixtures продолжают совпадать. Новые недоступные состояния не обрезаются четырёхрегистровым adapter.

### S2-03. Расширить fixture и snapshot

**Статус:** выполнено для integer schema v2; comparator/conversion проверены, CPU execution нового формата не заявлено. См. [отчёт](../reports/STAGE_2_REPORT.md#s2-03--versioned-fixturesnapshot).

**Файлы:** существующие case/result/compare modules testkit, TEST_CASE_FORMAT, runner test CLI.

1. Ввести новую версию схемы для полного GPR set, mode, guest RIP, memory layout и flags definedness. Значения сохранять lossless; поля unavailable не подменять нулями.
2. Оставить strict v1 reader и старый профиль неизменными. Для преобразования старого case использовать явную операцию с новым digest; исторический результат не переписывать.
3. Расширить comparator на новые регистры и memory/fault fields; добавить faults/unsupported/budget/stop без смешения statuses. Проверить fingerprint и связь результата с точным входом.

**Готово:** v1 regression зелёная; новая схема отклоняет неизвестные поля/версии, переполнения и несовместимые profile/state. Подмена каждого нового наблюдаемого поля обнаруживается. Synthetic tests comparator не засчитываются за CPU execution.

### S2-04. Ввести узкую границу памяти

**Статус:** выполнено для bounded fixture RAM с fault injection; production MMU/MMIO не заявлены. См. [отчёт](../reports/STAGE_2_REPORT.md#s2-04--память).

**Файлы:** `od-isa/src/memory.rs`, `exception.rs`, bounded fixture backend в testkit.

1. Определить fetch/read/write и архитектурный контекст доступа. Развести guest linear, guest physical и host addresses; не передавать guest value как host pointer.
2. Ввести типизированные CPU faults и внутренние ошибки. Код ошибки и fault address брать из определённого контракта; проверка slice bounds не доказывает `#PF`.
3. Реализовать fixture-owned flat RAM с checked sizes, little-endian accesses и контролируемым fault injection. Он не становится второй production RAM/MMU.
4. Зафиксировать семантику пересекающих регионы обращений и commit для первого поддержанного вида доступа. Не строить универсальную транзакционную систему памяти.

**Готово:** unit tests различают fetch/read/write, проверяют переполнение guest address, недоступные ranges и injected fault. Нет host panic при невалидном гостевом адресе; внутренняя ошибка не маскируется гостевым исключением.

### S2-05. Ввести исполнение одной инструкции

**Файлы:** `od-isa/src/execute/`, существующий публичный entry как compatibility wrapper.

1. Определить `step` с явными normal/branch/fault/trap/wait/stop/unsupported выходами в нужном объёме. Run loop считает budget и проверяет события на документированных границах.
2. Декодировать по текущему RIP, не требуя предварительного декодирования всего buffer. Достигнутый конец S0 buffer обработать в adapter, а не как универсальное CPU-правило.
3. Сохранить точный faulting RIP и наблюдаемое состояние. Неподдерживаемая форма не должна частично исполниться.

**Готово:** прямой и обратный переход, разные entry RIP, недостижимый невалидный хвост, budget и STOP дают определённые результаты. Старый S0 профиль сохраняет свои документированные ограничения.

### S2-06. Реализовать prefix/opcode/size decoding

**Предлагаемые модули:** `od-isa/src/decode/`, собственные таблицы только для принятого профиля.

1. Реализовать byte-at-a-time fetch, prefix groups, REX, operand/address-size overrides и opcode maps по нужным режимам. Legacy SIMD mandatory prefixes трактовать по конкретной форме, а не универсальным правилом.
2. Возвращать нормализованную instruction description с длиной, widths и операндами. Различать malformed/architecturally invalid, unsupported и fetch fault.
3. Проверить лимит длины инструкции, каждый truncation point и правила повторных/недопустимых prefixes по нормативному источнику.

**Готово:** decoder не читает за необходимыми bytes, возвращает точную длину и не принимает соседние неподдерживаемые формы случайно. Граница страницы сама по себе не является ошибкой при доступных следующих bytes.

### S2-07. Реализовать ModR/M, SIB и effective address

1. Добавить register/memory operands, REX extensions, base/index/scale, displacement и immediate с точными sign/zero extension rules.
2. Реализовать RIP-relative формы, отсутствие base/index и address-size override; адрес вычислять от нужного RIP и с нужным wrap, не от host PC.
3. Для первого long-mode flat профиля явно задать segment bases. Canonical-address и segment checks привязать к CPU-профилю; полный segmentation path — S2-22.

**Готово:** ручные fixtures покрывают все выбранные combinations, отрицательные displacement, RSP/R12 SIB, RBP/R13 special cases, RIP-relative и arithmetic wrap. Байты fixtures имеют независимое происхождение, а не только output своего encoder.

### S2-08. Первый рабочий memory MOV

1. Начать с `MOV r64, [base+disp]` и `MOV [base+disp], r64` в fixture RAM. Затем добавить widths 8/16/32 с правильным subregister write.
2. Использовать одну и ту же нормализованную addressing semantics из decoder; не повторять её в runner. При injected fault проверить отсутствие неразрешённых side effects.
3. Выдать новый snapshot через существующий CLI; добавить реальное сохранение памяти до/после.

**Готово:** из CLI выполняется программа load → integer operation → store, result содержит GPR/RIP/flags/memory. Проверены unaligned, region boundary и fault случаи. До S2-09 это локальное подтверждение, а не физическая приёмка среза A.

### S2-09. Расширить физический oracle

**Файлы:** native runner/trampoline/supervisor, testkit adapters, existing SSH harness.

1. Сохранить существующие parent/child limits и validation. Расширять native allowlist только на конкретные новые безопасные формы.
2. Переделать захват полного GPR set, включая RSP и регистры, ранее служившие scratch/state pointers wrapper. Нельзя резервировать гостевой регистр и затем объявлять все 16 проверенными.
3. Создавать ограниченные fixture mappings с правами и guard pages, выделять тестовый stack. Начальные адреса и pointer-valued results должны однозначно соответствовать гостевой раскладке; нормализация не должна скрывать неверный адрес.
4. Проверить, что trampoline не портит flags/memory/GPR до snapshot, не даёт тесту менять служебные ranges и восстанавливает host ABI. Код без постоянного RWX.
5. Запустить первый memory slice и старый S0 corpus дважды на Mac mini. Обновить CPU_ORACLE с реальными capabilities.

**Готово:** одни входы дают одинаковое определённое состояние interpreter/native, включая память и все заявленные GPR. Host signal наблюдается отдельно; `SIGSEGV` не становится автоматически guest `#PF`. Если полное наблюдение недоступно, соответствующая строка покрытия остаётся непроверенной.

## 7. Целочисленные инструкции и точные выходы

Общие правила S2-10–S2-21: добавлять формы небольшими наборами; каждую проверить ручными boundaries и физическим corpus до следующей. Для memory operands учитывать порядок доступа и исключений. Неопределённые флаги исключать из сравнения до запуска согласно source, не после mismatch.

### S2-10. Передача данных, расширения и LEA

1. Добавить MOV register/immediate/memory формы принятого профиля, MOVZX/MOVSX/MOVSXD и LEA.
2. Проверить, что LEA вычисляет адрес без чтения памяти, а widths/extension не зависят от host casts.
3. Добавить XCHG сначала register-register; memory XCHG связать с atomic access contract S2-21, не реализовывать как незащищённые read+write с заявлением об атомарности.

**Готово:** exact-width fixtures и native comparison покрывают каждую добавленную форму; memory XCHG до S2-21 явно не объявлен готовым.

### S2-11. Арифметика и eager flags

1. Расширить ADD/SUB/CMP на принятые widths и формы; затем ADC/SBB с входным carry.
2. Отдельно добавить INC/DEC/NEG с их правилами сохранения/изменения flags. Использовать собственные width-aware helpers только там, где семантика действительно общая.
3. Проверить signed/unsigned overflow, carry/borrow, auxiliary carry, parity и неизменяемые bits. Не использовать host overflow как универсальную замену x86 semantics.

**Готово:** boundaries каждого width и carry-in проходят native oracle, включая memory destination/fault state. Независимые expected cases не рассчитываются тем же helper, что production CPU.

### S2-12. Логика и TEST

1. Добавить AND/OR/XOR/TEST, затем NOT в поддержанных widths/forms.
2. Явно зафиксировать definedness каждого flag; TEST не записывает вычисленный результат в operand.

**Готово:** register и memory формы, неизменяемые bits, undefined flags и fault cases проверены на физическом CPU.

### S2-13. Сдвиги и вращения

**Статус:** выполнено в положительных register и memory-RMW границах; физическая проверка fault-state snapshot для отказа доступа недоступна в текущем native harness. См. [отчёт](../reports/STAGE_2_REPORT.md#s2-13--сдвиги-и-вращения).

1. Реализованы SHL/SAL/SHR/SAR, ROL/ROR/RCL/RCR и SHLD/SHRD с immediate/CL counts в заявленных формах.
2. Проверены masking/modulo effective counts, нулевой effective count, carry-in, CF/OF definedness по count и форме, и undefined destination для SHLD/SHRD при count больше operand width. Rust shift count не используется без архитектурной проверки.
3. Локальные injected read/write faults проверяют отсутствие retirement и commit CPU/RIP/definedness/RAM для нулевого и ненулевого count. Physical native fixtures проверяют успешные register cases и две memory RMW формы; admission rejects покрывают запрещённые ranges/permissions, но не снимок state после host signal.
4. Две 16-bit SHLD/SHRD raw17 cases не участвуют в physical exact comparison: их destination architecturally undefined. Локальные `od-isa` tests сохраняют no-panic проверку и подтверждают undefined всех arithmetic flags для этих случаев.

**Готово:** effective count и per-flag definedness соответствуют выбранным формам, нулевой effective count не меняет destination/flags, host shift panic/UB исключён; native comparison подтверждает определённые выходы в успешных границах. Fault-before-commit подтверждён локальной fault injection, не physical host-signal snapshot.

### S2-14. Умножение, деление и `#DE`

**Статус:** завершён в границах успешных форм и положительного physical comparison 38 fixtures. Локальные #DE fault-state проверки пройдены; physical/native signal mapping для #DE не заявляется. См. [отчёт](../reports/STAGE_2_REPORT.md#s2-14--умножение-и-деление) и [evidence](../reports/data/stage2/multiply-divide-2026-10-22.json).

1. Локально реализованы MUL/IMUL формы с явной шириной промежуточного результата и implicit destination registers; IMUL двух- и трёхоперандных форм записывает усечённый результат и выставляет CF/OF при signed overflow.
2. Локально реализованы DIV/IDIV. Делитель-ноль и quotient вне диапазона возвращают #DE до destination/RIP/flags-definedness commit; host division panic не используется как механизм `#DE`.
3. Локальные `od-isa` unit tests проверяют divisor zero, unsigned quotient overflow, signed byte minimum / −1 и signed 128-bit minimum / −1 с сохранением состояния и без retirement. Все 38 literal v2 fixtures — успешные операции с нормальным stop outcome; они покрывают формы/widths, границы operands, quotient/remainder и defined flags, но не проверяют #DE. Ни #DE delivery, ни соответствие Linux host signal/context физическому `#DE` не подтверждены.
4. На момент завершения S2-14 faulting `#DE` cases оставались только в локальных `od-isa` unit tests; subsequent S2-20a и S2-20c добавили три local-only fixture-v2/CLI snapshots. Физический mini-run S2-14 сравнил все 38 успешных fixtures и repeat-2 snapshots; соответствие определённого состояния подтверждено для interpreter и native. Ни physical fault delivery, ни Linux signal/context mapping для этих трёх fault cases не проверялись.

**Готово:** positive cases совпадают с physical oracle по определённому состоянию; #DE до retire сохраняет точный локальный state. Это не подтверждает физическую доставку #DE или соответствие host signal/context архитектурному fault.

### S2-15. Остальные integer формы профиля

**S2-15a статус:** локально завершены opcode `98` (`CBW/CWDE/CDQE`) и `99`
(`CWD/CDQ/CQO`); физическая проверка шести fixtures отложена для единого
итогового mini-check substage. См. [отчёт](../reports/STAGE_2_REPORT.md#s2-15a--sign-extension-opcodes-98-и-99).

**S2-15b статус:** BSWAP `0F C8+rd` локально реализован для 32- и 64-bit GPR;
физическая проверка семи fixtures отложена для того же итогового mini-check.
См. [отчёт](../reports/STAGE_2_REPORT.md#s2-15b--bswap).

**S2-15c статус:** register-only BT/BTS/BTR/BTC локально реализованы для
16/32/64-bit registers; физическая проверка 12 fixtures отложена для общего
итогового mini-check. Memory forms с register bit index реализованы отдельно
в S2-15h.
См. [отчёт](../reports/STAGE_2_REPORT.md#s2-15c--btbtsbtrbtc-register-only).

**S2-15d статус:** register-only BSF/BSR локально реализованы для
16/32/64-bit widths; физическая проверка 17 fixtures отложена для общего
итогового mini-check. См. [отчёт](../reports/STAGE_2_REPORT.md#s2-15d--bsfbsr-register-only).

**S2-15e статус:** к обычным BSF/BSR формам добавлены memory-source варианты
`0F BC/BD r/m`; шесть новых fixtures проверяют успешное чтение и обработку
memory source, всего 23 scan fixtures. Отдельный injected
read-fault unit test проверяет no-commit. Их physical comparison отложен до
общего финального mini-check. F3-prefixed LZCNT/TZCNT остаются вне среза из-за
feature-dependent semantics. BT-family memory destination forms с обоими
способами задания bit index реализованы в S2-15g/h.
См. [отчёт](../reports/STAGE_2_REPORT.md#s2-15e--bsfbsr-memory-source).

**S2-15f статус:** обычные BT/BTS/BTR/BTC immediate формы `0F BA /4–/7 ib`
локально реализованы при `r/m=register`, widths 16/32/64. 48 literal fixtures
покрывают четыре immediate значения на каждую пару operation/width; физическая
проверка отложена до общего mini-check. Их memory-destination variants описаны
в S2-15g/h.
См. [отчёт](../reports/STAGE_2_REPORT.md#s2-15f--bt-family-immediate-register-forms).

**S2-15g статус:** добавлены non-LOCK memory-destination формы BT/BTS/BTR/BTC
`0F BA /4–/7 ib` с raw imm8 для operand widths 16/32/64 и exact-width memory
access. 48 literal fixtures локально проверены; физическая проверка ожидает
общий mini-check. Memory формы с register bit index описаны в S2-15h.
См. [отчёт](../reports/STAGE_2_REPORT.md#s2-15g--bt-family-immediate-memory-destination).

**S2-15h статус:** добавлены BT/BTS/BTR/BTC формы `0F A3/AB/B3/BB` с register
bit index и memory destination. Для 16/32/64-bit operand widths реализованы
signed operand-width Euclidean offsets, exact-width accesses и adjusted-range
native admission. 72 literal fixtures локально проверены; физическая проверка
отложена до общего mini-check. Независимое `gpt-6.1-sol` review завершилось
без замечаний.
См. [отчёт](../reports/STAGE_2_REPORT.md#s2-15h--bt-family-register-index-memory-destination).

Оставшиеся физические проверки на Mac mini пользователь попросил сгруппировать
в один общий финальный подэтап после завершения локальной реализации. Все 216
непроверенных S2-15 fixtures a–h входят в эту единую будущую проверку. Ранее
выполненные физические результаты этапа сохраняются как отдельные evidence и
не откладываются задним числом.

1. Выполнить общий physical mini-check всех baseline forms строки S2-15; register-register XCHG уже проверен в S2-10, memory XCHG и LOCK-контракт относятся к S2-21. Срезы a–h локально покрывают остальные формы этой строки: sign-extension, BSWAP, BT-family register/immediate/memory и BSF/BSR обычные register/memory sources. F3-prefixed BSF/BSR, включая feature-dependent LZCNT/TZCNT, находятся вне baseline. Не считать S2-15 принятым до общего physical mini-check.
2. Для каждого подшага сначала выписать opcode/feature requirements и обработку нулевого operand, counts и flags.
3. POPCNT/LZCNT/TZCNT/BMI и другие расширения не смешивать с внешне похожей legacy encoding без проверки feature-dependent semantics.

**Готово:** каждая обязательная строка профиля имеет реализацию и hardware/manual evidence нужного уровня. Название группы не скрывает недостающие формы.

### S2-16. Условия и переходы

1. Все 16 Jcc, direct relative JMP, SETcc и CMOVcc реализованы в S2-16a/b/c;
   S2-16d/e покрывают ограниченный near indirect JMP с register и ordinary
   memory r/m64. FS/GS and out-of-profile addressing, CALL, and far transfers
   remain open.
2. Проверить условие по guest flags, taken/not-taken, sign extension и целевой RIP; instruction pointer width зависит от режима.
3. CMOVcc читает memory source даже при false condition; source read и no-commit fault behavior покрыты в S2-16c.

Подшаг S2-16a локально реализует все 16 condition codes для short/near Jcc
(`70–7F`, `0F 80–8F`) и direct relative JMP (`EB`, `E9`) в long mode.
64 conditional fixtures проверяют taken/fallthrough для каждого condition code
и обе длины; четыре JMP fixtures проверяют forward/backward rel8/rel32.
Exact-length/fetch faults, полный CF/PF/ZF/SF/OF truth table, next-RIP
displacement, RIP wrap и taken noncanonical target покрыты локально. Prefixes
вне выбранного profile остаются Unsupported. SETcc и CMOVcc реализованы в
S2-16b/c; на тот момент indirect JMP оставался незавершённой частью S2-16.

Все 68 literal fixtures S2-16a входят в единый финальный Mac mini acceptance
substage вместе с 216 fixtures S2-15, 79 fixtures S2-16b SETcc и 70 fixtures
S2-16c CMOVcc; на момент завершения S2-16c было 433 fixtures. S2-17a добавил
8 cases, поэтому после S2-17a общий pool составлял 441; S2-17b увеличил его
до 449, S2-16d — до 452, а S2-16e добавил 7; после S2-16e было 459,
после S2-17c — 465, после S2-17d — 470, после S2-17e было 479, после S2-17f pool был 488, после S2-17g pool был 498, после S2-17h pool был 509, после S2-17i было 518, после S2-17j pool был 525; S2-17k добавил 8, после него pool был 533; S2-17l добавил 7, после него pool был 540; S2-17m добавил 4, теперь 544.
Независимое review S2-16a завершилось чисто без замечаний.

Подшаг S2-16b локально реализует SETcc `0F 90–9F /r` для byte register и
ordinary memory destinations. Literal fixtures проверяют taken/not-taken для
всех 16 conditions на register и memory, high-byte aliases без REX, low-byte
aliases с REX, extended byte registers, 66/67 и RIP-relative addressing.
Покрыты игнорирование ModRM.reg, unchanged flags/definedness, exact-byte
memory writes, unsupported F0/F2/F3 и FS/GS memory prefixes, fetch/write faults
и отсутствие commit при fault. Все 79 fixtures входят в общий финальный
physical mini substage; на момент завершения S2-16c pool составлял 433. SETcc допускает ordinary
write admission; JIT capability не расширена. На момент завершения S2-16c общий
pool был 433; после S2-16d было 452, после S2-16e — 459, после S2-17c —
465, после S2-17d — 470, после S2-17e — 479, после S2-17f — 488, после S2-17g — 498, после S2-17h — 509.

Подшаг S2-16c локально реализует CMOVcc `0F 40–4F /r` для всех 16 условий.
Source читается и при false condition; read fault не commit-ит CPU state,
flags, RIP или destination. После успешного чтения false 32-bit form всё равно
выполняет 32-bit destination write и очищает верхнюю половину регистра.
70 literal fixtures проверяют условия, widths, source reads, fault behavior и
полное ожидаемое состояние. Local checks прошли; независимое Sol review чистое.
Физическая проверка этих 70 fixtures отложена до общего Mac mini acceptance
substage; тогда общий пул составлял 433. S2-17a/b добавили 16 cases,
S2-16d — 3, S2-16e — 7, S2-17c — 6, S2-17d — 5, S2-17e — 9, S2-17f — 9, S2-17g — 10, S2-17h — 11, S2-17i — 9, S2-17j — 7, S2-17k — 8, S2-17l — 7, S2-17m — 4, S2-17n — 6, S2-17o — 2, S2-17p — 1, S2-17q — 1; текущий общий pool — 554.

Подшаг S2-16d локально реализует только near indirect JMP `FF /4` с
регистровым r/m64 и `REX.B`. Три fixtures проверяют taken targets через RAX и
R8 и literal `#GP` для noncanonical target; decode/budget и no-commit на
faulting target также проверены. Local workspace checks прошли, independent
Sol medium review чистое. Linux runner native tests не запускались: macOS ld
отвергает GNU-specific flags; Linux-target check прошёл. Физическая проверка
отложена в общий mini-check. S2-16e добавляет ограниченный ordinary memory
target; FS/GS and other out-of-profile addressing, CALL и far transfers остаются
открытыми.

Подшаг S2-16e добавляет ordinary memory r/m64 для near indirect JMP `FF /4`.
Target qword читается до проверки canonicality; read fault или `#GP` не
commit-ит состояние. Семь literal fixtures покрывают base, `REX.B`, SIB,
RIP-relative и address-size-32 effective addresses. FS/GS prefixes и другие
формы адресации вне profile остаются открытыми. Local checks прошли,
independent Sol medium review чистое. Linux runner native tests не запускались:
macOS linker не принимает GNU flags; Linux-target check прошёл. Физическая
проверка отложена в общий mini-check; после S2-16e пул составлял 459, после
S2-17c — 465, после S2-17d — 470, после S2-17e — 479, после S2-17f было 488, после S2-17g было 498, после S2-17h было 509, после S2-17i было 518, после S2-17j pool был 525, после S2-17k pool был 533; после S2-17l pool был 540; после S2-17m — 544, после S2-17n — 550, после S2-17o — 552, после S2-17p — 553, после S2-17q — 554 fixtures.

**Готово локально:** truth-table условий, conditional moves, direct transfers
и ограниченные register/memory-target indirect JMP покрыты tests; physical
acceptance остаётся в общем финальном mini substage. FS/GS and out-of-profile
addressing, CALL, far transfers и формы S2-17 остаются незавершёнными.

### S2-17. Стек, CALL/RET и косвенное управление

Подшаг S2-17a реализует только 64-bit GPR PUSH/POP opcode forms `50–5F` с `REX.B` и восемь literal fixtures. Проверены aliasing источника/назначения с RSP, bounded guest stack, полный canonical address range, `#SS(0)` и fault-before-commit. Local checks прошли; независимое Sol medium review чистое. Физическая проверка отложена.

Подшаг S2-17b добавляет только 64-bit immediate PUSH `6A ib` и `68 id`; оба encoded immediate sign-extend к qword. Восемь literal boundary fixtures проверяют формы/границы, faults, stack bounds, truncated fetch и no-commit. Local tests/checks прошли, независимое Sol medium review чистое. Все 16 fixtures S2-17a/b ждут общего финального mini-check; после этого среза pool был 449, после S2-16d/e — 459, после S2-17c — 465, после S2-17d пул был 470, после S2-17e pool был 479, после S2-17f pool был 488, после S2-17g pool был 498, после S2-17h pool был 509, после S2-17i pool был 518, после S2-17j pool был 525; после S2-17k общий pool был 533.

Подшаг S2-17c добавляет near relative `CALL rel32` с шестью literal fixtures; успешные вызовы и отдельные stack/target faults проходят локальные checks. Независимое Sol-medium review не нашло concrete defects в успешных CALL/RET, отдельных fault cases, fixture answers или native admission. Ограничение остаётся: executor проверяет canonical target до stack helper, но при одновременных noncanonical target и invalid/noncanonical stack winner не подтверждён. Intel SDM rev093 Vol.2A `CALL` p.3-125 описывает operation pseudocode и `#GP` отдельно, без однозначного combined-fault priority. Этот dual-fault case не покрыт шестью fixtures; отдельный fixture и expected physical snapshot ещё не созданы. Подробности — [ISSUE-013](../KNOWN_ISSUES.md#issue-013--не-подтверждён-приоритет-target-gp-и-stack-fault-для-near-call). S2-17c входит в общий отложенный pool, который тогда составлял 465.

Подшаг S2-17d добавляет только register near indirect CALL `FF /2 r/m64` с `REX.B`; пять literal fixtures покрывают формы RAX/R8, budget и отдельные `#GP`/stack-fault cases. Исправленный fixture stack sentinel — `A5`. Workspace tests/check/fmt/generators и Linux-target check прошли; Linux runner tests не удалось слинковать на macOS, потому что Apple ld отвергает GNU flags. Независимое Sol-medium review чистое. Физическая проверка отложена до общего финального Mac mini acceptance; после этого среза pool был 470 fixtures.

Подшаг S2-17e добавляет memory-source near indirect CALL `FF /2 r/m64`: executor читает qword operand, затем проверяет target canonicality, затем выполняет bounded push; fault не фиксирует состояние. Девять literal fixtures покрывают base, REX.B, SIB, RIP-relative и address-size 67 effective addresses, budget, а также отдельные source `#PF`, target `#GP` и stack `#SS` faults. Local workspace checks прошли; Linux native tests не исполнялись, физическая проверка всех fixtures остаётся отложенной до общего финального Mac mini acceptance. Независимое Sol-medium review чистое. Общий пул вырос с 470 до 479. ISSUE-013 теперь охватывает combined noncanonical-target + invalid-stack priority для relative, register и memory indirect CALL; dual-fault fixture/snapshot не создан.

Подшаг S2-17f добавляет только near `RET imm16` (`C2 iw`) в 64-bit operand mode. Unsigned imm16 прибавляется к RSP после pop 64-bit return address. Девять literal fixtures покрывают immediate boundaries, nested-call cleanup, budget, truncation и fault no-commit. Независимое Sol-medium review чистое; workspace, fmt, 21 generators, Python и Linux-target checks прошли. Физическая проверка отложена до общего финального Mac mini acceptance; текущий общий pool — 488 fixtures.

Подшаг S2-17g добавляет только 16-bit GPR PUSH/POP `50–5F` с `66` и optional `REX.B`; `REX.W` отсутствует, default operand size остаётся qword. PUSH читает word source до уменьшения RSP; POP сначала увеличивает RSP на 2, затем записывает low 16 bits, включая SP alias. Десять fixtures (7 success, 3 fault) покрывают extended registers, RSP/SP ordering, faults/truncated fetch и carry: RSP `0x30ffff` плюс word `0x2468` даёт `0x312468`. Независимое Sol review чистое; workspace/fmt/22 generators/Python/Linux-target checks прошли. Native Linux admission только cross-compiled; физический mini-check отложен. Общий pool вырос с 488 до 498.

Подшаг S2-17h добавляет только 16-bit immediate PUSH: `66 6A ib` sign-extends imm8 к word, а `66 68 iw` записывает literal word; оба уменьшают RSP на 2 и выполняют двухбайтовую запись. Восемь успешных и три fault fixtures покрывают immediate boundaries, stack bottom и truncated immediates с no-commit. Provenance: Intel SDM order 253667-093US, Vol.2B PUSH pp.4-522–4-525. Независимое Sol-medium review чистое; workspace/fmt, freshness всех generators, Python, Linux-target и native od-x86-runner checks прошли. Физический mini-check отложен; общий pool вырос с 498 до 509.

Подшаг S2-17i добавляет только qword memory-source PUSH `FF /6`: ordinary EA формы base, REX.B, SIB, RIP-relative и address32 читают 8-byte source до bounded stack write. Девять fixtures (7 успешных, включая budget и `[RSP]`, плюс source `#PF` и stack `#SS`) проверяют no-commit при fault. Независимое Sol-medium review чистое; workspace tests/check, fmt, generator freshness, Linux-target, Python, diff и graph checks прошли. Linux admission tests только cross-compiled; физическая проверка отложена до общего финального Mac mini substage. Общий pool вырос с 509 до 518.

Подшаг S2-17j добавляет только qword memory-destination POP `8F /0`: читает qword по старому RSP, затем увеличивает RSP, затем вычисляет destination EA по обновлённому состоянию. Шесть успешных fixtures покрывают base, REX.B, SIB, RIP-relative, address32 и `[RSP]` post-increment addressing; ещё один fixture проверяет destination-store `#PF` и неизменённую память. Fault fixture намеренно не сравнивает post-state CPU, поскольку нормативное commit поведение RSP при fault записи не установлено ([ISSUE-014](../KNOWN_ISSUES.md#issue-014--не-установлено-состояние-rsp-после-fault-у-memory-destination-pop)), а native admission отвергает этот fault-only case. Всего 7 fixtures. Независимое Sol-medium review чистое; workspace suite/check, fmt, freshness 25 generators и Linux-target checks прошли. Физическая проверка отложена; общий pool вырос с 518 до 525.

Подшаг S2-17k добавляет только memory-source PUSH `66 FF /6` (`r/m16`). Источник и stack destination читаются/записываются ровно по 2 байта; EA источника вычисляется до уменьшения RSP. `67` влияет только на EA, сохраняя 64-bit RSP. Восемь literal fixtures: шесть успешных (base, REX.B, SIB, RIP-relative, address32 и `[RSP]` source с высоким RSP) и два fault (`#PF` чтения источника и `#SS` stack), с no-commit. Независимое Sol-medium review чистое; workspace/fmt, 26 generators, Python, Linux-target, diff и graph checks прошли. Физический Mac mini acceptance отложен до общего финального подэтапа; pool вырос с 525 до 533.

Подшаг S2-17l добавляет memory-destination POP `66 8F /0` (`r/m16`). Сначала читается ровно 2 bytes со старого RSP, затем staging `RSP+2`, после чего вычисляется EA назначения (в том числе `[RSP]`) и записывается ровно 2 bytes. Семь fixtures: шесть успешных addressing cases и destination-store `#PF`; fault case проверяет exception/no memory write, но намеренно не сравнивает CPU post-state ([ISSUE-014](../KNOWN_ISSUES.md#issue-014--не-установлено-состояние-rsp-после-fault-у-memory-destination-pop)). Независимое Sol-medium review чистое; workspace/fmt, Linux-target, 22 generator tests и freshness всех generators, graph и diff checks прошли. Физическая проверка отложена до общего финального Mac mini acceptance; pool вырос с 533 до 540.

Подшаг S2-17m добавляет memory PUSH `66 48 FF /6` и POP `66 48 8F /0`: REX.W выбирает qword при наличии предшествующего 66. Четыре literal fixtures покрывают обычные memory source/destination и `[RSP]` для pre-decrement PUSH и post-increment POP. Decoder проверяет ширину обеих форм, testkit исполняет полное ожидаемое состояние, а native admission принимает все четыре fixtures и проверяет эффективный REX при повторных префиксах; LOCK и FS/GS forms остаются вне profile. Материализатор и freshness check добавлены. Первое независимое ревью нашло admission mismatch; после исправления focused повторное Sol-medium review чистое. `od-isa`/`od-testkit` executable tests прошли; runner admission test Linux x86-64-only и на текущем macOS host cross-checked, но не исполнялся. Физический Mac mini acceptance остаётся отложен. Общий pool вырос с 540 до 544. ISSUE-014 по destination-store fault post-state не меняется.

Подшаг S2-17n покрывает регистровые PUSH/POP с `66` и REX: effective `REX.W` выбирает qword вопреки `66`, а финальный REX с `W=0` выбирает word даже после более раннего REX с `W=1`. Шесть fixtures проверяют PUSH/POP, REX.B для R8/R9, оба размера и повторный REX; literal answers подтверждают изменение RSP и сохранение верхних битов word POP. Декодер и native admission проверяют формы, testkit сравнивает полное состояние. Нормативный источник — Intel SDM revision 093, Vol.2A §2.2.1.2 pp.2-8–2-9 и Vol.2B entries PUSH/POP. Физическая проверка отложена до общего mini acceptance; pool вырос с 544 до 550. `od-isa`/`od-testkit`, freshness всех 29 генераторов, fmt и workspace/Linux-target checks прошли; Linux-only admission test cross-compiled, но не исполнялся на macOS. Независимое Sol-medium review чистое. ISSUE-013/014 не меняются.


S2-17o локально реализует только bare `POPFQ` (`9D`) при штатных CPL3/IOPL0: CF/PF/AF/ZF/SF/DF/OF обновляются, bit 1 и все прочие RFLAGS bits сохраняются в interpreter projection; source/current TF/NT/AC даёт `Unsupported` до commit. AccessContext позволяет задать CPL, но IOPL отдельно не моделируется. Два literal fixtures покрывают успешное обновление/маскирование; stack fault и fixture-driven TF/AC no-commit проверяются на CPU и памяти; native admission отвергает TF/NT/AC. Независимое Sol review чистое. od-isa/testkit, CLI fixture, workspace check, Linux-target check, 28 generator tests, fmt, layout и diff проверки прошли. Linux-only native admission test cross-compiled, но не исполнялся на этом macOS host. Физическая проверка отложена до общего mini acceptance; pool составляет 552 fixtures.

S2-17p локально добавляет только bare `PUSHFQ` (`9C`) в long mode. По Intel SDM revision 093, Vol.2B PUSHF/PUSHFD/PUSHFQ pp.4-528–4-529, instruction сохраняет qword `RFLAGS AND 00000000_00FCFFFFH`, то есть очищает RF и VM в stack image и не меняет RFLAGS. Один literal fixture проверяет полный локальный образ, GPR/RSP/RIP и unchanged flags; physical memory comparison маскирует IF/IOPL и прочие неподмоделированные биты, но проверяет profile status flags, DF, bit 1 и нулевые RF/VM. Unit tests проверяют RF/VM clearing для произвольного state и no-commit при bounded write failure. Native admission проверяет только writable qword stack range; физическая проверка отложена до общего mini acceptance. Generator freshness, `od-isa`, fixture-driven testkit и Python generator tests прошли; независимое review и общие checks фиксируются в отчёте. Pool — 553 fixtures.

S2-17q локально добавляет только `66 9C` (`PUSHFW`) в long mode. Intel SDM revision 093, Vol.2B PUSHF/PUSHFD/PUSHFQ pp.4-528–4-529, задаёт 16-bit operation по 66H: сохраняются нижние 16 bits EFLAGS без RF/VM clearing, RSP уменьшается на два, RFLAGS остаётся неизменённым. Другие эффективные REX/segment/address/LOCK/REP prefix combinations остаются Unsupported; misplaced REX перед `66` игнорируется общим prefix decoder и поэтому разбирается как `66 9C`, тогда как REX непосредственно перед opcode не принимается этим срезом. Один literal fixture проверяет двухбайтовую запись, canaries, полный GPR/RSP/RIP и flags; unit tests проверяют bounded write no-commit. Native admission проверяет writable word range. Физическая проверка отложена до общего mini acceptance. Локальные проверки и независимое review записаны в отчёте. Pool вырос до 554 fixtures.

S2-17r добавляет только `66 9D` (`POPFW`) в long mode при CPL3/IOPL0. Из слова загружаются CF/PF/AF/ZF/SF/DF/OF; IF, IOPL, bit 1 и все другие непрофильные биты сохраняются. Source or current TF=1 returns `Unsupported` до commit; nonzero current IOPL returns `Unsupported` before stack access. Из-за конфликта Intel SDM 093 Vol.2B Table 4-15 и Vol.1 §7.3.13.2 p.7-22 source или initial NT=1 также остаётся Unsupported ([ISSUE-015](../KNOWN_ISSUES.md#issue-015--разногласие-sdm-по-nt-для-popf-word-на-cpl3iopl0)). Literal v2 fixture compares RFLAGS через маску `0xCD7`; отдельный local testkit assertion проверяет точное сохранение IF, reserved bit 3, ID и upper bit при source IOPL=3 и current IOPL=0. Fixture-driven no-commit checks отдельно покрывают source/current TF, source/current NT и nonzero current IOPL; native admission также отвергает nonzero current IOPL. Физическая проверка отложена до общего Mac mini acceptance; pool вырос до 555 fixtures. Результат локальных проверок указан в [отчёте](../reports/STAGE_2_REPORT.md#s2-17r--16-bit-popfw-at-cpl3iopl0-local).

S2-17t locally admits `66 C3` and `48 66 C3` after prefix normalization, retaining the existing qword return-target read and eight-byte RSP increment. Intel SDM revision 093 Vol.1 §6.4.7 pp.6-12–6-13 forces near-branch operation size to 64 bits in 64-bit mode and says operand-size prefixes are silently ignored. Vol.2A §2.2.1 p.2-8 says REX is ignored unless it immediately precedes the opcode or 0F, so the REX byte preceding 66 in the second form is ineffective. Vol.2B RET p.4-566 defines `C3` as near RET. Two literal fixtures check full GPR/RIP/RFLAGS state, qword target, and unchanged code/stack memory. Effective REX (`66 48 C3`), duplicate 66, and other effective prefix states remain Unsupported by this slice. The pool is now 558 fixtures; physical acceptance remains deferred.

Открыты другие memory POP forms, другие `FF` groups, FS/GS memory forms, прочие PUSHF width/prefix variants, другие privilege/IOPL combinations, другие профильно требуемые flags-stack forms, другие RET prefix variants, far returns и IRET. Результат S2-17a–t не закрывает подэтап S2-17; ISSUE-013 требует нормативного evidence или отдельной физической dual-fault проверки. ISSUE-014 остаётся открытым по memory-destination POP; ISSUE-015 фиксирует конфликт SDM для POPFW NT.

1. Добавить PUSH/POP и нужные flag-stack формы с правильным stack/address/operand size.
2. Добавить near CALL/RET и indirect JMP/CALL. Проверить return address, последовательность accesses, displacement и aliasing с RSP.
3. Расширить native harness только на ограниченный guest stack и валидируемые control transfers; host return/trampoline memory остаются недоступны тесту.

**Готово:** вложенные вызовы возвращаются с правильным RIP/RSP; stack faults сохраняют архитектурное состояние по конкретной форме. Нельзя принять несовпадение стековых pointers за допустимую нормализацию.

### S2-18. Одна итерация строковой инструкции

1. Реализовать MOVS/STOS/LODS/CMPS/SCAS без repeat для нужных widths/address sizes.
2. Учесть implicit source/destination segments, DF и wrap указателей; не заменять semantics произвольным host memcpy/memcmp.
3. Проверить перекрывающиеся области, DF=0/1 и fault на source/destination с нужным порядком доступа.

**Готово:** после одной итерации memory, pointers, accumulator и flags совпадают с физическим профилем.

Подшаг S2-18a реализует только bare `A4` MOVSB в long mode: байт читается по
RSI, затем записывается по RDI; после обеих успешных операций RSI/RDI
инкрементируются или декрементируются на один по DF, после чего общий путь
коммитит RIP. Flat DS/ES базы равны нулю в long mode. REP, адресный/сегментный
override, другие widths и остальные string instructions не принимаются.
Intel SDM 093 не определяет здесь CPU snapshot после destination `#PF`;
проверенный interpreter no-commit является только текущей локальной policy
([ISSUE-016](../KNOWN_ISSUES.md#issue-016--не-установлен-restart-state-movs-после-destination-fault)).
Три literal positive fixtures покрывают DF=0, DF=1 и alias source/destination
(RSI=RDI) для однобайтового self-copy; две fault fixtures задают source/destination fault sites и проходят
локальную injected-`#PF` проверку порядка и no-commit, но native admission их
отклоняет. Deferred physical acceptance pool S2-15…S2-18a содержит 563 cases:
прежние 558 fixtures плюс 3 новых MOVSB positive cases и 2 local-only
injected-fault cases. Остальные fixtures каталога здесь повторно не
классифицировались; native admission определяет исполняемое подмножество.
Физические проверки остаются в едином финальном Mac mini substage.

Подшаг S2-18b добавляет только MOVSW `66 A5` в long mode: word читается по
RSI до записи по RDI, а после обоих успешных access указатели изменяются на
`+2`/`−2` по DF. Допускается нормализованное состояние после misplaced REX
перед `66` (например, `48 66 A5`), поскольку общий decoder сбрасывает REX,
который не стоит непосредственно перед opcode. Effective REX, REP,
segment/address overrides, bare `A5` и sibling string forms остаются
Unsupported. Три mapped positive fixtures проверяют DF=0/1 и двухбайтовый
self-copy; две local-only injected-`#PF` fixtures проверяют порядок доступа и
локальную no-commit policy, без утверждения hardware restart snapshot
(ISSUE-016). Native admission принимает ровно три raw fixture cases и
отклоняет обе fault-only cases; regression также проверяет нормализованный
`48 66 A5`. Этот slice добавляет 5 cases к deferred physical acceptance
pool S2-15…S2-18a: 3 positive и 2 local-only, всего 568. Остальные fixtures
каталога повторно не классифицировались. Физические проверки остаются в
едином финальном Mac mini substage.

Подшаг S2-18c добавляет default-width MOVSD через bare `A5` в long mode:
dword читается по RSI до записи по RDI, затем после обоих успешных access
указатели меняются на `+4`/`−4` по DF. Для этой dword формы требуется пустой
prefix state; `66 A5` остаётся отдельной поддержанной word-формой S2-18b.
Effective REX (включая REX.W), REP, любой segment prefix и 67 не принимаются.
Общий decoder сбрасывает REX перед последующим legacy prefix; в частности,
`48 66 A5` нормализуется как существующая MOVSW форма, а не как MOVSD.
Три mapped positive fixtures
проверяют DF=0/1 и overlap; две local-only injected-`#PF` fixtures проверяют
порядок доступа и локальную no-commit policy, не утверждая hardware restart
snapshot (ISSUE-016). Native admission принимает только эти три positive cases
и отвергает fault-only cases. Новый срез добавляет 5 случаев к pool 568 после
S2-18b, итого 573. Физические проверки остаются в едином финальном Mac mini
substage.

Подшаг S2-18d добавляет MOVSQ для эффективного `REX.W + A5`: qword читается
по RSI, затем записывается по RDI, после чего указатели меняются на `+8`/`−8`
по DF. Intel SDM 093 Vol.2B указывает 8-байтовую операцию и этот шаг; Vol.2A
§2.2.1.2 указывает, что effective REX.W имеет приоритет над `66`, поэтому
`66 48 A5` также декодируется как MOVSQ. Misplaced REX сбрасывается перед
legacy prefix: `48 66 A5` остаётся MOVSW. Effective REX.W с REP, segment или
address-size prefix не входит в этот срез.
Семь handwritten literal fixtures проверяют DF=0/1, RSI=RDI alias, forward
overlap, `66 48 A5`, а также local-only source-read и destination-write `#PF`
с ровно восьмибайтовыми access-log entries. No-commit fault snapshot остаётся
локальной policy под ISSUE-016; native admission принимает пять mapped
positive cases и отвергает две unmapped fault cases. Срез добавляет 7 случаев
к deferred pool 573 после S2-18c, итого 580. Физические проверки выполняются
только в едином финальном Mac mini substage.

Подшаг S2-18e реализует только bare `AA` STOSB в long mode: AL записывается
в один байт по flat ES:[RDI], затем RDI увеличивается/уменьшается на единицу
по DF; RSI, AL и flags сохраняются. Любые legacy/REX/REP/address/segment
prefixes и прочие widths STOS остаются Unsupported. Четыре handwritten literal
fixtures покрывают DF=0/1, RSI=RDI с canaries вокруг единственного изменяемого
байта и destination-write `#PF`. Три mapped positives допускаются native
runner; fault fixture остаётся local-only и проверяет bounded-memory no-commit
policy без hardware restart-state claim (ISSUE-016). После S2-18d deferred
physical acceptance pool содержал 580 cases; S2-18e добавляет три native
positive cases и один local-only fault, итого 584. Mac mini execution остаётся
частью единого финального acceptance substage.

#### S2-18f — одна non-REP LODS итерация (локальный профиль)

Исполняются только `AC` LODSB, `66 AD` LODSW, bare `AD` LODSD и effective
`REX.W + AD` LODSQ в Long64 с address-size 64 и flat `DS:[RSI]`. Для AD
effective REX.W имеет приоритет над 66; misplaced REX перед 66 сбрасывается
общей нормализацией (`48 66 AD` остаётся word, `66 48 AD` — qword). Остальные
REX bits не меняют implicit operand. REP, LOCK, 67, segment overrides,
effective non-W REX на AD и любой prefix перед AC не принимаются; это граница
профиля, не заявление о #UD для каждой законной кодировки.

Интерпретатор выполняет ровно одно чтение ширины операнда из pre-state RSI.
После успешного чтения AL/AX заменяют только низ RAX, EAX обнуляет верх RAX,
а RAX записывается полностью. RSI затем wrapping-обновляется на ±1/2/4/8 по DF.
RIP продвигается по длине инструкции, одна инструкция retire-ится. Любой read
fault сохраняет полный CPU/RAM state и faulting RIP. Неканонический DS source
даёт #GP; отсутствующая fixture mapping остаётся harness error. Это не
добавляет MMU semantics или host-error mapping.

Локальные проверки содержат 6,144 полных успеха (width × DF × шесть значений ×
два начальных RAX × все 64 комбинации CF/PF/AF/ZF/SF/OF), 48 перечисленных
prefix-reject encodings, REX/66 ordering, read faults и 22 literal CLI fixtures:
8 width/direction positives, 4 unmapped-source outcomes, 4 noncanonical-source
#GPs, 4 zero-budget cases и 2 normalization cases. Также проверяются полные
снимки состояния и RAM, доступ только для чтения, fault/budget no-commit и
различие architectural #GP и backend `HarnessError`. Fixture schema/profile
shape сохранена; валидатор допускает instruction budget 0 для проверки этой
границы исполнения. S2-18 и S2-19 остаются открытыми; native/JIT исполнение
LODS не добавлено. ISSUE-016 и ISSUE-019 остаются открытыми. Физическая
проверка остаётся в едином финальном Mac mini substage.

#### S2-18k — non-REP LODS с address-size 32

Расширить только Long64 LODS без REP: `67 AC` byte, `67 66 AD` word,
`67 AD` dword и `67 48 AD` qword. Использовать нормализованные prefixes:
REX.W имеет приоритет над 66, legacy prefix после REX удаляет REX; поэтому
`66 67 AD` — word, `67 48 66 AD` — word, `48 67 AD` — dword, а
`67 66 48 AD` — qword. Повторный 67, REP67, все segment overrides (включая
FS/GS) и не-W REX AD не входят в LODS admission; admission прочих string
operations не меняется.
Отказы ограничивают профиль и не утверждают универсальную доставку #UD.

Начальный адрес чтения — zero-extended ESI, DS flat. Читается одна ширина
целиком по этому начальному адресу; 32-bit wrap применяется к ESI до чтения и
к обновлённому RSI, но не к каждому байту доступа. Поэтому qword от ESI
`ffffffff` охватывает `ffffffff..1_0000_0006`. Успех обновляет AL/AX/EAX/RAX
с обычным aliasing, zero-extends конечный ESI с DF-направленным шагом,
продвигает RIP и retire-ит одну инструкцию. Ошибка чтения не меняет CPU/RAM,
сохраняет исходные high RSI/RAX, faulting RIP и нулевой retirement.

Проверить 18,432 полных успешных snapshots, 96 boundary successes, 96 injected
read failures, 73 enumerated prefix rejects, fetch truncation, RCX=0,
budget/STOP/stop_rip, TEST→LODS undefined-AF composition, CLD/STD consumers и
регрессии address64/REP. Добавить ровно 39 schema-v2/profile literal cases;
JIT/native admission остаётся закрытой, физическое исполнение — в общем
финальном acceptance. ISSUE-016/020/021 остаются открытыми; не закрывать стадии.

#### S2-18g — одна non-REP STOSW/D/Q итерация (локальный профиль)

Оставить bare `AA` STOSB и точный `F3 AA` REP STOSB без изменений. Добавить
только `66 AB` STOSW, bare `AB` STOSD и effective `REX.W + AB` STOSQ в
Long64/address-size 64 с flat `ES:[RDI]`. Effective REX.W важнее 66; общий
prefix parser сбрасывает misplaced REX до legacy prefix, поэтому `48 66 AB`
остаётся STOSW, а `66 48 AB` становится STOSQ. Все REX.W bytes `48..4F`
принимаются. REP, LOCK, 67, любой segment override и effective non-W REX перед
AB остаются Unsupported; REP AB не добавляется.

Исполнение делает ровно один all-or-error `write_operand` низких 2/4/8 байт
RAX в pre-state RDI, затем wrapping-обновляет только RDI на ±width по DF,
продвигает RIP на декодированную длину и retire-ит одну инструкцию. RAX, RSI,
RCX и полный RFLAGS, включая DF, сохраняются. Не читать память через RSI и не
проверять его canonicality. Неканонический destination даёт #GP(0); mapping,
permission и backend failures остаются HarnessError, если backend явно не
вернул architectural fault. Не синтезировать #PF. При любом отказе CPU и RAM
не коммитятся. Это не задаёт LOCK/SMP atomicity.

Проверки: 6,144 полных success snapshot (четыре ширины × DF × шесть значений ×
64 комбинации арифметических flags × два RAM background), ровно одна write и
нет data reads; 35 перечисленных prefix rejects; все REX.W/66 порядки и fetch
граница; alignment residues, 4 KiB crossing, writable-region end, RDI wrap и
noncanonical post-index, четыре варианта RSI без доступа, unmapped/read-only,
injected architectural/backend errors, noncanonical destination, budget 0 и
no-commit. Добавлены ровно 20 handwritten literal schema-v2 fixtures: шесть
width/direction positives, по три unmapped/read-only/noncanonical destination,
три budget-zero и два prefix-normalization случая. Генератор сериализует
заданные byte/state/memory ответы и не вычисляет семантику. Schema/profile не
меняются; JIT/native остаются закрыты. ISSUE-016/019 остаются открытыми.
Физические проверки и Mac mini/SSH запускаются только в общем финальном
substage.

#### S2-18h — одна non-REP SCAS B/W/D/Q итерация (локальный профиль)

Добавить `AE` SCASB, `66 AF` SCASW, bare `AF` SCASD и effective `REX.W + AF`
SCASQ в Long64/address-size 64 с implicit flat `ES:[RDI]`. Принимать все
`48..4F`; effective REX.W переопределяет 66 (`66 48 AF` — qword), а misplaced
REX перед legacy 66 нормализуется (`48 66 AF` — word). AE остаётся bare-only.
REP/REPE/REPNE, LOCK, 67, все segment overrides, effective non-W REX на AF и
остальные prefixed AE forms Unsupported. JIT/native admission не расширять.

Исполнение делает ровно одно чтение 1/2/4/8 байт по pre-state RDI и сравнивает
AL/AX/EAX/RAX с memory через `arithmetic_result(size, Cmp, accumulator,
memory, old_flags)`. После успешного чтения коммитит только arithmetic flags,
DF-directed wrapping RDI и обычные RIP/retirement. Полный RAX сохраняется даже
для SCASD; RSI, RCX, RAM и DF не меняются и RSI не проверяется. Чтение
неканонического ES source даёт #GP(0); missing/permission/backend errors
остаются HarnessError, если backend не сообщил architectural fault. Fault
текущего SCAS не коммитит состояние; ранее retired TEST в literal composite
остаётся committed. ISSUE-016/019 не закрывать.

Проверки: ровно 6,144 полных состояния (4 widths × 2 DF × 12 пар × 64
арифметических флаговых входа) с независимым expected-flag calculator; одна
data read и ноль writes; все alignment residues, crossing/range end, RDI wrap,
noncanonical post-index, RSI sentinel/alias/unmapped/noncanonical без доступа,
все REX.W/66 порядки, fetch/budget boundary и 48 enumerated prefix rejects.
Fault matrix покрывает unmapped/permission/backend/injected architectural
fault, source #GP и TEST+SCAS success/fault definedness. Добавить ровно 24
literal fixtures: 8 width/DF successes, 4 unmapped, 4 noncanonical #GP, 4
budget-zero, 2 normalization и 2 TEST composition. Generator только
сериализует predetermined answers; profile/schema не менять. Физические и
Mac mini/SSH проверки остаются в общем финальном acceptance.

#### S2-18i — одна non-REP CMPS B/W/D/Q итерация (локальный профиль)

Добавить только `A6` CMPSB, `66 A7` CMPSW, bare `A7` CMPSD и effective
`REX.W + A7` CMPSQ в Long64/address-size 64. Все `48..4F` допустимы;
effective REX.W важнее 66 (`48 66 A7` — word, `66 48 A7` — qword).
CMPSB остаётся bare-only. REP, LOCK, 67, segment override и effective
non-W REX перед A7 остаются за пределами профиля.

Строгая локальная последовательность исполнения: прочитать ширину из
pre-state flat DS:[RSI], затем прочитать ту же ширину из pre-state flat
ES:[RDI], вычислить CMP flags для `memory[RSI] - memory[RDI]`, и только после
двух успешных чтений зафиксировать flags, DF-directed wrapping RSI/RDI, RIP и
одно retirement. Любой отказ первого чтения прекращает доступы; отказ второго
оставляет trace первого чтения, но не коммитит CPU. Canonical48 DS/ES fault —
#GP(0); unmapped/permission/backend остаются HarnessError, кроме явно
архитектурной ошибки backend. Не synthesize #PF. RSI=RDI всё равно даёт два
чтения. Указанный порядок — ограниченная локальная политика, а не свидетельство
hardware priority для двух одновременно ошибочных адресов; ISSUE-020 открыт.
Нет rollback claims для MMIO/side-effect backend. JIT/native не расширять.

Покрытие: 6,144 независимых полных snapshots (4 widths × 2 DF × 12 ordered
pairs × 64 arithmetic-flag inputs), 64 отказа чтений (4 widths × 2 DF × 2 sites
× 4 error kinds), ordered trace/no writes, overlap/alias, alignments/page/range
edges, both pointer wrap/post-index and canonical faults, zero budget and
truncation. 48 exact prefix rejects; все REX.W варианты и обе нормализации.
Ровно 33 literal fixtures: 8 successes, 8 unmapped, 8 noncanonical, 4 budget,
2 normalization, 3 `TEST+CMPSB` success/first-fault/second-fault. Schema 2 и
`s2-long-integer-v1` unchanged; generator serializes stated expected values.
Mac mini, SSH, native and physical checks remain reserved for the single final
acceptance substage.

### S2-19. REP и частичное выполнение

1. Добавить REP/REPE/REPNE только к разрешённым формам; обработать нулевой count без лишнего access.
2. Определить проверяемую границу остановки между итерациями. Обновлять count/pointers/flags и RIP так, чтобы restart после fault/stop был архитектурно корректен.
3. Документировать, как instruction budget относится к одной REP instruction и к iteration work limit; не менять смысл существующего счётчика молча.
4. Проверить fault/stop на первой, средней и последней итерации, затем повторный запуск из snapshot.

**Готово:** сохранённый partial progress продолжает работу без потери/повтора уже завершённых side effects. Timeout native не засчитывается за точный STOP на той же итерации; используются контролируемые fault cases и отдельные тесты CPU event boundary.

#### S2-19a — `F3 AA` REP STOSB (ограниченный локальный контракт)

Реализовать только точные байты `F3 AA` в long mode. `RCX=0` завершает
инструкцию без доступа к destination; иначе каждая успешная итерация записывает
`AL` в flat `ES:[RDI]`, затем обновляет `RDI` на ±1 по DF и уменьшает `RCX`.
Flags и остальные GPR не меняются. Отдельный iteration work limit равен 1024
успешным stores на один вызов исполнения одной инструкции; он не списывается
из instruction budget. Полное завершение одной REP-инструкции retire-ится
ровно один раз. При STOP callback либо исчерпании iteration work limit после
успешной итерации `RIP` остаётся на `F3 AA`, а snapshot содержит уже
уменьшенный `RCX` и следующий `RDI`; повторный запуск продолжает с этой пары.
При memory fault текущая итерация не меняет `RDI`/`RCX`, ранее завершённые
итерации сохраняются, `RIP` остаётся на инструкции и retirement отсутствует.
Это контракт выбранной модели/fixtures, а не утверждение об общих приоритетах
hardware exceptions. Literal fixtures покрывают нулевой и ненулевые counts,
DF=0/1 и local-only fault после успешной итерации; direct interpreter tests
проверяют STOP/replay и iteration work limit. Native admission допускает только
полностью mapped small-count cases, work-limit outcome остаётся local.

#### S2-19b — `F3 AB` REP STOSW/D/Q (ограниченный локальный контракт)

Расширить существующий `Operation::RepStos { size }` на `F3 66 AB` (word),
`F3 AB` (dword) и `F3 effective-REX.W AB` (qword); все `48..4F` допустимы.
Для AB разрешить только группу F3, отсутствие segment/address-size override,
и отсутствие REX либо effective REX.W. 66 может идти до или после F3 и сочетаться с REX.W; общий
normalizer сохраняет `F3 48 66 AB` как word и `F3 66 48 AB` как qword.
`F3 AA` остаётся строго byte-only; `F3 66 AA`, F2, LOCK, 67, segment,
non-W REX, дублирующие operand-size и прочие REP формы остаются Unsupported.
JIT/native admission не расширять.

Для `RCX=0` завершить без destination validation/access. Иначе выполнять одну
all-or-error write низкого выбранного RAX-width в текущий RDI; только после
успеха wrapping-adjust RDI на ±width и decrement RCX. Сначала проверить
completion; если count ещё не ноль, проверить STOP, затем лимит 1024 успешных
stores на один вызов, иначе продолжить. STOP и limit оставляют RIP на начале
REP, retirement=0; завершение продвигает RIP и retire-ит одну инструкцию.
Fault текущей записи не меняет её RDI/RCX, но оставляет прежние stores и
индексы/count; replay продолжает с сохранённой пары. Instruction budget
считается как одна инструкция. RAX/RSI/RFLAGS и flags definedness внутри run
не меняются; отдельный новый run заново инициализирует definedness, поэтому
lossless across-run claim не делается.

Добавить 12,288 полных нормальных состояний (4 widths × 2 DF × 6 значений ×
4 counts × 64 комбинации flags), 96 ошибок write, 16 STOP/replay, 24 limit
границы, точные 35 REP-prefix rejects и byte regression boundary. Проверить
STOP после 1024-й записи, RCX=`u64::MAX` bounded progress, нулевой count с
неканоническим RDI, width crossing, mapped page/alignment/wrap, budget zero,
malformed fetch, byte/word/dword/qword aliases, TEST→REP success и partial
definedness. Добавить ровно 28 literal schema-v2 fixtures: по восемь на новую
ширину, две normalization и два TEST→REP потока (полный и partial после одной
записи). Генератор лишь сериализует заданные bytes/state/memory; profile/schema
не меняются. ISSUE-016 и ISSUE-020 остаются открытыми; физические проверки,
Mac mini, SSH и native остаются в едином финальном acceptance.

#### S2-19c — `F3 AC/AD` REP LODSB/W/D/Q (ограниченный локальный контракт)

Добавить `Operation::RepLods { size }` для exact `F3 AC` byte, `F3 66 AD`
word, `F3 AD` dword и effective REX.W `AD` qword. Для `AD` разрешить F3,
необязательный 66 и отсутствующий REX или effective REX.W; все `48..4F`
допустимы. Нормализация принимает `66 F3 AD` как word, `F3 48 66 AD` как
word и `F3 66 48 AD` как qword. `F3 66 AC`, F2/LOCK/67/segment, не-W REX
`AD`, прочие prefixed `AC` и остальные неподдержанные комбинации остаются
admission failures. Native/JIT не исполняют REP LODS.

При `RCX=0` не читать и не валидировать RSI, сохранять RAX/RSI и завершать одну
инструкцию. Иначе одна ширинно-точная read из flat `DS:[RSI]` должна успешно
завершиться до любого commit; после неё записать AL/AX/EAX/RAX через
`RegisterView`, wrapping-adjust RSI по DF на ±width, уменьшить RCX. Затем
проверить completion, STOP, предел 1024 успешных reads и продолжить. Полное
completion retire-ит REP один раз. STOP/limit оставляют RIP на REP начале и
retire-ят ноль; fault сохраняет все ранее завершённые значения/индексы/count,
а текущая неудачная read не меняет регистры. Replay продолжает со следующего
элемента без повторного чтения завершённых элементов. Flags/DF/RDI/RAM не
меняются. Это локальный mapping/fault контракт, не синтезированный #PF и не
заявление об MMIO rollback. Новая `run()` заново инициализирует flag
definedness; across-run сохранение metadata не заявляется.

Покрыть 12,288 completion states, 96 injected read failures, 16 STOP/replay
states и initial STOP, 24 limit states, bounded `RCX=u64::MAX`, zero count,
zero instruction budget, alignment/page/short-range/wrap/noncanonical-next-pointer,
TEST→REP completion/partial definedness, все 48 prefix rejects, а также
non-REP LODS и REP STOS regression. Добавить ровно 36 literal schema-v2
fixtures: восемь на каждую ширину, две prefix-normalization и два TEST→REP
потока. Generator только сериализует литералы; profile/schema остаются
неизменными. ISSUE-016/020 остаются открытыми; весь batch в финальном substage.

#### S2-19d — `F3 A4/A5` REP MOVSB/W/D/Q (ограниченный локальный контракт)

Добавить `Operation::RepMovs { size }` только для Long64 и адресного размера
64: exact `F3 A4` byte, `F3 66 A5` word, `F3 A5` dword и effective REX.W
`A5` qword. Для A5 допускаются F3, отсутствие segment/address override,
отсутствующий REX или effective REX.W; все `48..4F` допустимы. 66 может
сочетаться с F3 и misplaced/effective REX: `66 F3 A5` и `F3 48 66 A5` — word,
`F3 66 48 A5` — qword. `F3 66 A4`, F2, LOCK, 67, segment overrides, non-W
REX на A5 и остальные prefixed A4 формы остаются Unsupported. Существующие
non-REP MOVS и REP STOS/LODS сохранить. JIT/native не расширять.

При `RCX=0` завершить без проверки/доступа к RSI или RDI. В каждой итерации
выполнять ровно одну width-sized read `DS:[RSI]`, затем одну all-or-error
write в `ES:[RDI]`. Только после обеих успешных операций wrapping-adjust RSI и
RDI на ±width согласно DF и уменьшить RCX. После commit проверять completion,
STOP, затем локальный предел 1024 успешных передач. Completion выигрывает у
STOP/limit; STOP выигрывает у limit. Лимит считается успешными передачами, а
instruction budget — одной инструкцией. Fault/STOP/limit оставляют RIP на REP
start и retirement=0; уже успешные передачи остаются. Ошибка чтения пропускает
write; ошибка записи фиксирует успешное чтение, но не меняет destination,
индексы или RCX. Replay повторно читает текущий source; hidden pending source
не хранится. В run сохраняются RAX, RFLAGS/definedness и DF. Новый `run()`
переинициализирует definedness, поэтому across-run metadata preservation не
заявляется.

Проверить 12,288 completion states, 192 injected failures, 16 STOP/replay,
24 work-limit boundaries, 36 overlap cases с последовательной семантикой:
same source/destination and destination ±width at every width/DF, plus ±1-byte
destination offsets for word/dword/qword in both directions,
width-cross failures обоих operands, zero-count с двумя invalid адресами,
`u64::MAX` bounded progress, wrap/page/alignment/STOP@1024/budget-zero/fetch и
TEST composition. 48 prefix rejects. Добавить 56 полных literal schema-v2
fixtures: 44 (11 per width), 8 overlap, 2 normalization и 2 TEST→REP; generator
только сериализует заранее заданные bytes/state/memory. Schema/profile не
менять. Source-first — local policy, не подтверждённый dual-invalid priority;
зарегистрировать отдельно. ISSUE-016/020 оставить открытыми. Mac mini, SSH,
native и physical проверки — только в общем финальном acceptance.

#### S2-19e — exact `F3 AE` REPE SCASB (ограниченный локальный контракт)

Добавить только точные `F3 AE` в Long64/address64. До любого data access,
включая `RCX=0`, admission требует arithmetic values exactly `PF|ZF` (`0x44`)
и full definedness для шести arithmetic flags (`0x8d5`); DF разрешён в обоих
значениях. Это узкое требование профиля для entry/restart, не архитектурное
требование к REPE SCAS. Остальные prefixes, REPNE, widths и CMPS остаются
Unsupported без доступа, state change или retirement.

Нулевой count завершает одну instruction без проверки RDI, сохраняет flags и
продвигает RIP на 2. Иначе читать byte из flat `ES:[RDI]`, считать CMP flags
для `AL-byte`, затем commit flags, wrapping RDI ±1 по DF и decrement RCX.
RCX=0 или ZF=0 завершает до STOP и лимита; matching continuation проверяет
STOP, затем предел 1024 successful reads. Faulting read не коммитит свою
итерацию; предыдущие равные итерации сохраняются в RDI/RCX/flags. Fault, STOP
и limit оставляют RIP на REP и не retire-ят instruction. Restart снова проходит
тот же gate: после равных сравнений arithmetic values остаются PF|ZF, а все
arithmetic flags defined, без скрытого состояния. JIT/native, Mini, SSH и
physical execution не входят в этот локальный substage.

Покрытие: 131,072 single-comparison states с independent CMP model, 160
termination paths, 16,380 отказов admission, 24 injected read failures, 4
STOP/replay checks, 6 limit states (1023/1024/1025 × DF), mismatch на итерации
1024, priority mismatch/STOP/limit, zero count, invalid RDI, budget zero,
truncated fetch и wrapping/page edges. Добавить ровно 24 literal schema-v2
fixtures: zero count × DF; count-one equal/mismatch × DF; mismatch positions
1/2/3 × DF; all-equal × DF; unmapped after 0/1/2 equal reads; noncanonical
first access; count1025 × DF; budget-zero; CMP→REP success/fault; TEST→REP
reject с undefined AF при concrete PF/ZF. Generator только сериализует
hand-authored bytes и snapshots; schema/profile не меняются. ISSUE-016/019/020/
021 остаются открытыми до общей финальной acceptance.

#### S2-19f — exact `F3 A6` REPE CMPSB (ограниченный локальный контракт)

Добавить только exact bytes `F3 A6` в Long64/address64. Это local profile
restriction, не claim что другие архитектурные CMPS формы invalid. До любого
data access, включая `RCX=0`, admission требует concrete arithmetic values
exactly `PF|ZF` (`0x44`) и full definedness `ARITHMETIC_FLAGS` (`0x8d5`). Оба
значения DF разрешены. Gate failure возвращает Unsupported без доступа,
state change или retirement. `F3 AE` и существующие REP операции сохранить.

При нулевом RCX не проверять RSI/RDI и не выполнять reads; сохранить flags,
прибавить два к RIP, retire один раз. Иначе читать один byte source
`DS:[RSI]`, затем один byte destination `ES:[RDI]`, и сравнивать source minus
destination существующим CMP. RSI-first выбран только как local policy; ISSUE-020
не подтверждает dual-invalid hardware priority. Только после обоих reads
commit flags, wrap-обновить RSI/RDI на ±1 по pre-iteration DF и decrement RCX.
RCX=0 или ZF=0 завершают до STOP и work limit; matching continuation проверяет
STOP перед пределом 1024 успешных итераций. Fault сохраняет первый read на
dual-site отказе, но не коммитит текущие flags/pointers/count; предыдущие
полностью завершённые равные итерации остаются. Fault/STOP/limit оставляют RIP
на REP и retire=0. Replay заново выполняет оба чтения, без hidden source state.
MMIO rollback не заявляется; ошибки map/backend не синтезируются в guest fault.

Покрытие включает 131,072 сравнений и независимую CMP oracle, 160 путей
завершения, 16,380 отказов value/definedness/count/DF, 48 injected read faults,
STOP/replay, 1023/1024/1025 limits, mismatch на шаге 1024, RCX max, same/overlap
aliases, zero count с invalid pointers, budget=0, fetch/page/canonical/wrap края.
Добавить ровно 31 literal schema-v2 fixture: zero count × DF; count-one
equal/mismatch × DF; mismatch positions 1/2/3 × DF; all equal × DF; source и
destination unmapped после 0/1/2 равных элементов; noncanonical source/dest;
limit 1025 × DF; budget zero; CMP RAX,RAX композиция success/first/second
fault; TEST RAX,RAX gate reject при concrete PF|ZF и undefined AF; same/overlap
alias positives. Generator сериализует только фиксированные expected values.
Не менять schema/profile, не расширять JIT/native и не использовать Mini, SSH
или physical execution в этом срезе.

Результат локальной реализации, fixture snapshots и выполненные проверки
зафиксированы в [отчёте этапа](../reports/STAGE_2_REPORT.md#s2-19f--exact-f3-a6-repe-cmpsb-bounded-local-profile).
Статус: локальная реализация и проверка завершены; 31 fixture сравнивается по
полному состоянию, JIT остаётся закрыт. Общая физическая acceptance не выполнялась.

#### S2-19g — ограниченные `F3 AF` REPE SCASW/D/Q

Расширить `RepScas` только на word/dword/qword формы в Long64/address64:
`F3 66 AF` (word), `F3 AF` (dword), effective `REX.W + F3 AF` (qword).
Разрешить 66 вместе с effective REX.W; учитывать порядок `F3 48 66 AF`
(word) и `F3 66 48 AF` (qword). Все `48..4F` REX.W байты допустимы. Для AF
принимать только normalized group1 F3, без segment/address override и без
не-W REX. Сохранить точную byte-форму `F3 AE`; `F3 66 AE`, F2, LOCK, 67,
segment prefixes и остальные перечисленные в задаче формы остаются
Unsupported. Это локальный profile slice, не общий REPNE или REP decoder.

До любого data read, включая `RCX=0`, entry/restart gate требует concrete
arithmetic values ровно `PF|ZF` (`0x44`) и все `ARITHMETIC_FLAGS` defined
(`0x8d5`). DF допускается в обоих значениях; gate является ограничением
профиля, а не архитектурным требованием. Нулевой count не проверяет RDI,
сохраняет flags и завершает одну инструкцию. Иначе сравнивать AX/EAX/RAX с
word/dword/qword `ES:[RDI]` существующей CMP семантикой; полностью сохранить
RAX (включая верхние 32 бита SCASD), RSI, RAM и non-arithmetic flags. После
успешного read коммитить flags, wrapping `RDI ± width` и `RCX--`; faulting
read не коммитит текущую итерацию. Завершение по count или ZF=0 раньше STOP и
лимита; продолжающее равное сравнение проверяет STOP, затем 1024 успешных
итерации. Partial RIP остаётся на REP, retirement=0; committed equal state
сохраняет PF|ZF и definedness, поэтому replay проходит тот же gate. Срез не
заявляет undefinedness continuation, general REPNE, JIT/native execution,
Mini/SSH или physical execution.

Покрытие: 864 одиночных сравнения (3 widths ×12 accumulator ×12 memory ×2 DF),
720 termination paths, 49,140 W/D/Q entry-gate rejects при сохранении
16,380-byte matrix, 72 injected read failures, 12 STOP/replay paths и 18
limit paths. Отдельно проверить mismatch на сравнении 1024, STOP-vs-limit,
initial STOP, budget zero, RCX max, zero count с invalid RDI, page/range,
alignment, index wrap и noncanonical post-index. Добавить ровно 59 literal
schema-v2 fixtures: 16 на каждую ширину, два prefix-order normalization и
три CMP/TEST compositions на ширину. Генератор только сериализует заданные
fixtures/answers; schema/profile не меняются. Сохранить JIT/native fail-closed.
ISSUE-016/019/020/021 остаются открытыми.

Локальная реализация и проверка завершены: 59 fixture snapshots, decoder/ISA
матрицы и CLI JIT-denial прошли. Полный workspace test/check/fmt и все 98 CPU
Python tests прошли. Graphify обновлён. Linux-only native admission test
добавлен, но не исполнялся на macOS; native execution и общая physical
acceptance не выполнялись. Фактические границы и проверки — в [отчёте этапа](../reports/STAGE_2_REPORT.md#s2-19g--bounded-f3-af-repe-scaswdq).

#### S2-19h — bounded `F3 A7` REPE CMPSW/D/Q

Extend exact `F3 A6` REPE CMPSB with `F3 66 A7` word, `F3 A7` dword, and
effective-REX.W `F3 A7` qword forms in Long64/address64. Accept both normalized
prefix orders (`66 F3 A7`, `F3 48 66 A7` as word; `F3 66 48 A7` as qword) and
all effective REX.W bytes `48..4F`. Keep `F3 66 A6`, F2/LOCK, address/segment
overrides, and non-W REX unsupported. Do not widen the JIT/native profile.

The CMPSB entry/restart gate remains in force before reads, including at RCX=0:
concrete arithmetic flags exactly `PF|ZF` (`0x44`) and all
`ARITHMETIC_FLAGS` defined (`0x8d5`). Each iteration reads the full-width
DS:[RSI] then ES:[RDI], compares source minus destination, and commits flags,
wrapping RSI/RDI by ±width, and RCX only after both reads. Preserve first-read
trace on second-read failure; prior complete equal iterations remain committed.
Completion/mismatch precedes STOP and the 1024-iteration limit. Partial RIP and
retirement behavior remain the S2-19f local contract. ISSUE-020 remains a local
RSI-first dual-fault limitation for CMPS B/W/D/Q; no hardware/MMIO claim.

Local verification covers 864 independent single states, 720 termination
paths, 49,140 gate rejects, 144 injected failures, 12 STOP/replay cases and 18
limit cases; alias/overlap/sub-width offsets, ordered reads, page/range/
alignment/wrap, mismatch-at-1024, zero invalid pointers, RCX maximum, budget
zero and truncated fetch. Exactly 71 literal schema-v2 fixtures cover 19 per
width, two prefix-normalization cases and four CMP/TEST compositions per width.
The generator serializes fixed literal expectations only. The CLI verifies full
state and JIT denial; Linux-only native admission remains explicitly denied.
No native execution, physical, Mini, or SSH acceptance is claimed.

#### S2-19i — явное продолжение REP после лимита итераций

Шесть schema-v2 fixtures покрывают `TEST RAX,RAX` перед REP STOSB, LODSB и
MOVSB с DF=0/1 и `RCX=1025`. Первый запуск выполняет TEST и 1024 итерации REP,
возвращая частичный snapshot с RIP на REP, `RCX=1` и сохранённой flag
definedness. Sidecar содержит дословные snapshots первого запуска и конечные
snapshots после второго запуска, передавшего definedness первого результата в
`run_with_rflags_defined`. Продолжение выполняет последнюю итерацию ровно один
раз, сохраняет undefined AF и завершает REP. Schema-v2/profile и стандартное
поведение свежего `run()` не меняются: обычный `run()` по-прежнему начинает с
полной integer-flag definedness; сохранение между вызовами доступно только
через новый явный tracked-entry API. Fixtures не являются доказательством
hardware restart state или fault priority. ISSUE-016/020/021 остаются открыты;
native/JIT/physical acceptance не выполнялась.

#### S2-19j — bounded REP LODS с address-size 32

Расширить только `Operation::RepLods` на Long64 с address-size 32: `F3 67 AC`
byte, `F3 67 66 AD` word, `F3 67 AD` dword и `F3 67 48 AD` qword. Для `AD`
сохранить существующий выбор ширины: effective REX.W выше 66; нормализованные
формы `F3 66 67 AD` и `F3 67 48 66 AD` остаются word, `F3 67 66 48 AD` —
qword. Для byte разрешён только F3 и необязательный 67. Другие REP-prefix
комбинации остаются вне локального admission; отказ не утверждает #UD для всех
законных кодировок. Не расширять JIT/native.

При каждой итерации брать начальный адрес из ESI, выполняя одну width-sized
читающую операцию из flat `DS:[ESI]`. Address-size 32 выбирает ECX для REP
count: нулевой ECX не читает и не валидирует RSI и сохраняет исходный RCX
целиком; после успешной итерации decrement записывает zero-extended ECX в RCX.
Только после успешного read записать AL/AX/EAX/RAX через существующий
`RegisterView`, изменить ESI на ±width modulo 2^32 с zero-extension и уменьшить
ECX. Fault сохраняет текущую итерацию без commit, а ранее успешный прогресс
остаётся продолжимым через существующий tracked-entry API.
Сохранить общие STOP/completion/1024-limit ordering, один retirement на полное
выполнение, ноль на частичное, неизменность flags/DF/RDI/RAM, и ошибку
отсутствующего mapping как harness error.

Покрыть четыре ширины, обе DF, нулевой/ненулевой count, high-half RSI,
частичный read failure, normalized prefix order, pointer wrap modulo 2^32,
work-limit/restart composition, регрессии S2-19c и address-size-64 LODS.
Добавить буквальные schema-v2 fixtures для всех widths, RCX high-half при
нулевом ECX, zero-budget и initial STOP без мутаций, и memory fault после
успешной итерации; генератор сериализует предзаданные ожидания. Fixture
schema/profile не менять. ISSUE-016,
ISSUE-020 и ISSUE-021 не закрывать по этому локальному результату; physical
acceptance остаётся в едином финальном Mac mini substage.

Исправление S2-19j от 2026-10-27: count selection/decrement теперь используют
ECX по Intel SDM REP pseudocode; ECX=0 сохраняет исходный RCX целиком. Целевые
проверки `od-isa` прошли 4/4, literal `od-testkit` fixture test — 1/1, полный
workspace test suite, all-target offline check, formatting, fixture freshness,
diff check и Graphify update прошли. См. [отчёт коррекции](../reports/STAGE_2_REPORT.md#s2-19j-correction--address-size-32-count-uses-ecx). Physical,
Mac mini, SSH и JIT/native проверки не выполнялись. Независимое gpt-6.1-sol high
ревью завершено без замечаний по коду и fixtures; найденное устаревшее описание
в `docs/testing/INTERPRETER.md` исправлено перед закрытием ревью.

#### S2-19k — bounded REP STOS с address-size 32

Расширить существующий `Operation::RepStos` только на Long64/address-size 32:
`F3 67 AA` STOSB, `F3 67 66 AB` STOSW, `F3 67 AB` STOSD и
`F3 67 48 AB` STOSQ. Сохранить выбор ширины и только уже установленные
normalization rules: `F3 66 67 AB` и `F3 67 48 66 AB` дают word,
`F3 67 66 48 AB` — qword, `F3 48 67 AB` — dword. Для каждого старта
использовать zero-extended EDI; одна итерация делает одну непрерывную
width-sized запись AL/AX/EAX/RAX, затем обновляет EDI на ±width modulo 2^32
по DF и уменьшает ECX. ECX=0 не обращается к destination и оставляет весь
RCX/RDI неизменным; после успешного decrement ECX zero-extends в RCX.

Текущая faulting запись не меняет CPU; предыдущие завершённые итерации
сохраняются. Использовать общие REP completion/STOP/1024-success limit,
tracked-definedness и retirement правила S2-19c. Отсутствующее отображение
остаётся harness error. Это локальная политика, не hardware restart,
приоритет fault или MMIO rollback claim. JIT/native не расширять.

Реализация добавила core matrices по четырём ширинам и DF, pointer wrap и high
halves, zero ECX, STOP/replay, budget, частичный store failure и iteration
limit; точную negative prefix matrix; 11 literal schema-v2 cases с полными
expected snapshots и генератором, который сериализует предзаданные значения.
Fixture schema/profile не менялись. Целевые и полные workspace проверки
пройдены; см. [отчёт](../reports/STAGE_2_REPORT.md#s2-19k--bounded-rep-stos-address-size-32).
ISSUE-016/020/021 остаются открытыми, physical/Mac mini/SSH проверки
выполняются только в общем финальном substage.

#### S2-19l — bounded REP MOVS с address-size 32

Расширить только Long64 REP MOVSB/W/D/Q формы `F3 67 A4`, `F3 67 66 A5`,
`F3 67 A5` и effective REX.W `F3 67 48 A5`. Сохранить только уже описанные
правила нормализации: misplaced/effective REX с 66 и порядок `66` относительно
67/REX выбирают ширину по действующему REX.W; иные префиксы остаются
Unsupported. Декодер и interpreter используют существующий `Operation::RepMovs`;
профиль/schema v2 не меняются. JIT/native admission остаются закрытыми.

Count берётся из ECX. При ECX=0 полностью сохраняются RCX, RSI и RDI и не
проверяются адреса. Иначе каждая итерация делает один непрерывный width-sized
read из zero-extended ESI, затем один all-or-error write в zero-extended EDI.
Размер доступа не переносится через 4 GiB по модулю. После успешного write оба
индекса обновляются modulo 2^32 по DF, а ECX decrement zero-extends в RCX.
Текущая faulting итерация не коммитится; завершённые ранее остаются видимыми,
overlap читает изменённую RAM каждой следующей итерацией. Completion выигрывает
у STOP и 1024-success limit; STOP выигрывает у limit. Частичный outcome оставляет
RIP на REP и retirement=0. Tracked-entry API сохраняет mask; обычный `run()`
остаётся fresh-entry. Отсутствие mapping остаётся harness error. Source-first —
local policy, не аппаратное утверждение приоритета; ISSUE-021 остаётся открытой.

Локальная проверка включает адрес-размерные decode/reject cases, width/DF/count
матрицы, high halves и aliases, overlap propagation, exact access order, 1023/
1024/1025/max ECX, limit resume и двенадцать literal schema-v2 fixtures с
полными snapshots. Генератор сериализует предзаданные значения и не вычисляет
ожидания из семантики инструкции. ISSUE-016/020/021 остаются открытыми; общий
workspace acceptance и физическая проверка — отдельные критерии.

#### S2-19m — bounded REPE SCASB с address-size 32

Расширить только Long64 byte SCASB на две точные prefix-последовательности:
`F3 67 AE` и `67 F3 AE` (длина 3), результат —
`Operation::RepScas { size: Byte }`. Сохранить S2-19e exact `F3 AE`.
Для нового адрес-размерного профиля не принимать `AF`, REPNE, дополнительные
`66`/REX/segment/LOCK или повторные prefixes; decoder и interpreter должны
ограничивать один и тот же набор. Адрес-размер-64 поведение остаётся прежним.

Entry/restart gate остаётся полным и проверяется до count/data path, в том числе
при ECX=0: `RFLAGS & 0x8d5 == 0x44` и definedness включает весь `0x8d5`.
Count выбирается из ECX. При ECX=0 полностью сохранить RCX/RDI и flags,
не проверять и не читать адрес данных, нормально продвинуть RIP на 3 и retire.
Для ненулевого ECX читать один byte по zero-extended EDI, сравнить AL-byte,
и только после успешного read коммитить CMP flags, EDI ±1 modulo 2^32 и
zero-extended ECX-1. Fault текущего чтения не коммитит текущую итерацию;
предыдущие успешные итерации сохраняются. Завершение по ECX=0 или ZF=0 имеет
приоритет над STOP, затем проверяется предел 1024 успешных чтений. Fault,
STOP и предел оставляют RIP на REP и retirement=0; tracked resume передаёт
предыдущую definedness mask. JIT/native остаются закрытыми.

Покрытие: независимая полная AL×byte×DF CMP-модель; gate negatives для bad и
undefined arithmetic flags, включая ECX=0; prefix positive/negative matrix;
zero-count high-half preservation, zero-extension EDI/ECX, partial read fault,
completion/STOP/1024 ordering, CMP→REP success и TEST→REP reject. Добавить
literal full-state schema-v2 fixtures с таблицей ожидаемых snapshots и
serialization-only generator; проверить CLI snapshots/JIT denial. Обновить
отчёт, CPU contract, interpreter coverage и fixture README. Физическую,
native/JIT, Mac mini и SSH проверки не выполнять.

#### S2-19n — bounded REPE CMPSB с address-size 32

Расширить только Long64 byte CMPS на точные трёхбайтовые prefix-порядки
`F3 67 A6` и `67 F3 A6`, оба декодируются как `Operation::RepCmps { size:
Byte }`. Сохранить существующий `F3 A6` address-size-64 и закрыть `A7`, REPNE,
добавочные 66/REX/segment/LOCK и повторные prefixes. Decoder и executor
admission должны принимать один и тот же набор.

Сохранить полный локальный entry/restart gate до чтения count, включая
ECX=0: `RFLAGS & 0x8d5 == 0x44`, а definedness содержит `0x8d5`. Count — ECX;
при нуле сохранить полный RCX/RSI/RDI и flags, не обращаться к памяти, перейти
на RIP+3 и retire ровно один раз. Иначе каждая итерация сначала читает byte
из zero-extended ESI, затем byte из zero-extended EDI. После обоих успешных
чтений сравнить source−destination через существующую модель CMPSB и только
затем обновить flags, ESI/EDI modulo 2^32 по DF и zero-extended ECX-1.
Текущая неуспешная итерация не коммитится; предшествующий read остаётся в trace
при ошибке destination. Source-first и dual-invalid результат — local ISSUE-020
policy, не аппаратный приоритет или MMIO side-effect rollback claim.

Завершение по ECX=0 или ZF=0 предшествует STOP, затем применяется limit 1024
успешных сравнений. Fault/STOP/limit оставляют RIP на CMPSB и retirement=0;
tracked resume передаёт definedness mask. Проверить независимую byte CMP-модель,
оба DF, высокие половины и wrap, count/mismatch, первый/второй/dual invalid,
retry, gate в том числе при ECX=0, zero budget, STOP, limit и tracked resume.
Добавить literal schema-v2 snapshots и serialization-only generator, проверить
runner full snapshot/JIT denial, затем обновить контракт, notes, issue и report.
Native/JIT/physical/Mac mini/SSH проверки не выполнять.

#### S2-19o — bounded REPE SCASW/D/Q с address-size 32

Расширить Long64 REPE SCAS только для широких `AF` форм с address-size 32.
Принимать normalized group1 F3, address override 67 и только W/D/Q ширину:
`F3 67 66 AF` — W, `F3 67 AF` — D, `F3 67 48 AF` — Q. Учитывать legacy
порядок: `F3 67 48 66 AF` — W, `F3 67 66 48 AF` — Q, а `F3 48 67 AF` — D,
поскольку последующий legacy prefix сбрасывает REX. Decoder и executor admission
должны принимать один профиль. Существующий `F3 AE` и addr32 `F3 67 AE` /
`67 F3 AE` остаются без изменений; addr32 CMPS `A7` и дополнительные F2,
LOCK, segment, repeated group, effective non-W REX и неподдержанные prefix
формы остаются закрыты.

Сохраняется полный локальный entry/restart gate до проверки ECX, включая ECX=0:
`RFLAGS & 0x8d5 == 0x44`, definedness содержит `0x8d5`. Count — ECX. При нуле
не читать память и полностью сохранить RCX/RDI/RAX/RSI и flags. Для каждой
успешной ненулевой итерации выполнить один width-sized read из zero-extended
EDI, сравнить AX/EAX/RAX с данными, затем commit CMP flags, modulo-32 EDI ±
width и zero-extended ECX-1. Весь read непрерывен при пересечении 4 GiB; память
после wrap не дочитывается. Fault текущего чтения не коммитит итерацию,
предыдущие успешные итерации сохраняются. Completion/mismatch предшествует
STOP, затем идёт предел 1024 успешных reads; partial RIP/retirement и tracked
resume сохраняют общий REP контракт. JIT/native остаются закрытыми.

Локальное покрытие: W/D/Q × DF с независимой CMP-моделью, zero/high-half и
noncanonical full RDI при корректном EDI, width read через границу 4 GiB,
частичные/unmapped tails, gates включая ECX=0, count/work-limit, mismatch,
fault/replay и CMP→SCAS success / TEST undefined-AF reject. Literal fixture-v2
snapshots используют serialization-only генератор. Обновить CPU contract,
interpreter notes, fixture README и report. ISSUE-020 остаётся открытым. Не
выполнять native/JIT/physical/Mac mini/SSH execution.

**Результат:** локальный decoder/interpreter срез реализован; executor loop
не требовал изменения, только admission согласован с decoder. Добавлены
1,350 независимых одноэлементных сравнения, addr32 count/mismatch/replay/gate
проверки и 14 полных literal fixture snapshots с CLI JIT denial. Итоговые
проверки и ограничения зафиксированы в [отчёте S2-19o](../reports/STAGE_2_REPORT.md#s2-19o--bounded-repe-scaswdq-address-size-32).

#### S2-19p — bounded REPE CMPSW/D/Q с address-size 32

Расширить только Long64 REPE CMPS `A7` на W/D/Q с address-size 32. Принимать
нормализованные F3 и 67: `F3 67 66 A7` — W, `F3 67 A7` — D,
`F3 67 48 A7` — Q. Prefix order учитывает reset REX последующим legacy
prefix: `F3 67 48 66 A7` — W, `F3 67 66 48 A7` — Q и `F3 48 67 A7` — D.
Все effective REX.W `48..4F` допустимы. Оставить без изменений addr64 CMPS
W/D/Q, addr64 CMPSB `F3 A6` и addr32 CMPSB `F3 67 A6`/`67 F3 A6`. F2, LOCK,
segments, повторные prefix groups, effective non-W REX и прочие формы остаются
закрыты. Decoder и executor admission должны совпадать; JIT/native не менять.

Сохранить local gate до чтения ECX и данных, включая ECX=0:
`RFLAGS & 0x8d5 == 0x44`, definedness содержит `0x8d5`. Count — ECX; ноль
полностью сохраняет RCX/RSI/RDI/flags и не обращается к памяти. Иначе выполнить
две отдельные непрерывные width-sized reads: сначала из zero-extended ESI,
затем из zero-extended EDI. Сравнивать source−destination. Только после обоих
успешных чтений коммитить flags, ESI/EDI modulo 2^32 по DF и zero-extended
ECX-1. Read остаётся contiguous при пересечении 4 GiB: truncate только
начальный адрес, не каждый байт. При source failure destination не читается;
при destination failure trace сохраняет source read, но текущая итерация
целиком не коммитится. Replay перечитывает оба операнда. Это source-first и
dual-invalid winner как local ISSUE-020 policy; hardware priority и MMIO
rollback не утверждать.

Completion/mismatch предшествует STOP, затем ограничение 1024 успешными
сравнениями. Fault/STOP/limit оставляют RIP на REP и retire=0; tracked resume
передаёт definedness mask. Проверить independent CMP model W/D/Q×DF, prefix
normalization/rejection, high halves и zero count, modulo wrap по обоим DF,
все width reads через 4-GiB boundary, first/second/dual-invalid source-first,
retry после destination fault с изменённым source, overlap, STOP/mismatch/limit,
и CMP→REP success / TEST→REP undefined-AF gate.
Добавить literal full-state schema-v2 snapshots и serialization-only
generator, testkit/CLI/JIT denial; обновить contract, interpreter notes,
fixture README, ISSUE-020 scope и report. Не выполнять native/JIT/physical,
Mac mini или SSH execution.

**Результат:** bounded decoder/interpreter admission реализован; существующий
общий `RepCmps` loop уже использует адресный размер для ECX/ESI/EDI и выполняет
две width-sized reads с commit только после обеих, поэтому его семантика не
дублировалась. Добавлены independent compare matrix, addr32 gates, prefix
матрицы, high-half/wrap/fault/replay/STOP проверки и 14 literal fixture-v2
snapshots с CLI JIT denial. Итоговые проверки и границы зафиксированы в
[отчёте S2-19p](../reports/STAGE_2_REPORT.md#s2-19p--bounded-repe-cmpswdq-address-size-32).

#### S2-19q — bounded REPNE SCASB с address-size 32

Добавить только Long64 `F2 67 AE` и `67 F2 AE`. Addr64 `F3 AE`, addr32
`F3 67 AE`/`67 F3 AE` и весь текущий F3 профиль остаются неизменными. Остальные
F2 SCAS/CMPS, AF/wide forms, дополнительные prefix, JIT и native закрыты.
Decoder/executor принимают единственный нормализованный профиль.

Определённость всех arithmetic flags обязательна до проверки ECX; concrete
RFLAGS значения не gate-ятся, включая ECX=0. Count — ECX. Нулевой count не
читает память и сохраняет RCX/RDI/RAX/flags/mask. Для каждой успешной итерации
один byte read по zero-extended EDI, CMP(AL, byte), commit flags, EDI ± 1 по DF
с modulo-32 wrap и zero-extended ECX-1. REPNE завершается при ECX=0 или ZF=1;
STOP и limit проверяются после completion. STOP/limit оставляют RIP и retirement
на instruction boundary; для переносимого возобновления нужен новый явный
`RepContinuation` API. Старые entry points fail closed для новой формы.

Token сохраняет original RFLAGS/definedness и ожидаемый checkpoint RAX/RCX/RDI/
RFLAGS/mask/RIP/AccessContext. Вход валидирует checkpoint до callback/fetch;
декодированное instruction identity проверяется до data read. Architectural
fault восстанавливает исходные flags/mask, оставляя уже выполненный count/index
прогресс, и очищает token; backend MemoryError сохраняет актуальный token.
Token хранится только вызывающей стороной: частичный schema-v2 snapshot не
восстанавливает continuation без token. Источник restart flags: Intel 64 and IA-32
Architectures Software Developer's Manual, Combined Vol. 2B, REP/REPE/REPNE,
§4-568 (rev. 093); это локальный execution profile, не полная модель #PF delivery.

Покрыть независимой CMP-моделью обе DF и начальные ZF, count 0/1/many, high
halves, ошибки state/instruction/mask/context до callbacks, faults first/after
progress, host MemoryError, STOP/1024 continuation, prefix order, old API и
literal schema-v2 snapshots. Testkit sidecar удерживает token между вызовами;
одиночные fixtures стартуют с `None`. Обновить CPU contract, interpreter notes,
fixture README и report. Без physical/Mac mini/SSH execution.

**Результат:** реализация и проверки описываются фактически в
[отчёте S2-19q](../reports/STAGE_2_REPORT.md#s2-19q--bounded-repne-scasb-addr32).

#### S2-19r — bounded REPNE CMPSB с address-size 32

Расширить только Long64 точными normalized prefix-порядками `F2 67 A6` и
`67 F2 A6`. Сохранить addr64 F3 и addr32 F3 профили; addr64 F2 и wide F2
CMPS, LOCK, segment, лишние/повторные prefixes остаются закрыты. JIT/native
admission не менять. Старые entry APIs должны fail closed; explicit
`run_with_rep_continuation` добавляет новую форму через discriminator opaque
token (`ScasbAddr32 { rax }` / `CmpsbAddr32 { rsi }`) при общей checkpoint
части. Schema v2 остаётся прежней; fixtures — fresh one-shot, а multi-run
testkit хранит token в sidecar.

До ECX и любых data reads требовать definedness всех arithmetic flags, в том
числе при ECX=0; конкретные значения flags не ограничивать. Нулевой ECX
сохраняет полные RCX/RSI/RDI/RFLAGS и завершает instruction без чтений. Для
ненулевого count читать byte из zero-extended ESI, затем byte из zero-extended
EDI; сравнить source−destination. Только после обоих успехов коммитить flags,
ESI/EDI modulo 32 по DF и zero-extended ECX−1. Architectural read fault
восстанавливает entry flags/mask, оставляет ранее выполненный progress и
очищает token; destination failure сохраняет source read в trace. Host backend
MemoryError сохраняет текущую token checkpoint пару; retry читает оба байта
заново и не хранит source byte в token. Завершение ECX=0 или ZF=1 предшествует
STOP и пределу 1024 успешных сравнений.

Проверить независимую byte-CMP модель, STOP/limit yields и resume, обе позиции
fault на entry и после progress, changed-source retry после backend failure,
stale state/context/mask/identity и AE↔A6 token cross-kind без data access,
prefix reorder, old APIs, initial stop/budget token preservation, zero-count
gate/retirement и fetched architectural fault. Добавить literal schema-v2
fixtures с serialization-only generator и runner full snapshots/JIT denial.
Обновить CPU contract, interpreter notes, fixture README, report и ISSUE-020
scope. ISSUE-020 остаётся открытым; source-first и dual-invalid outcome — local
policy, без hardware priority/MMIO claim. Не выполнять native/JIT/physical,
Mac mini или SSH проверки.

**Результат:** bounded decoder/interpreter profile реализован на общей
continuation API; имеются независимая модель, fault/replay, sidecar и две
literal fixture snapshots. Targeted проверка прошла; полная проверка и diff
review записываются в [отчёте S2-19r](../reports/STAGE_2_REPORT.md#s2-19r--bounded-repne-cmpsb-addr32).

### S2-20. Точность исключений и exit state

1. Свести для реализованных форм `#UD`, `#DE`, `#GP`, `#SS`, memory faults и traps только там, где они действительно определены профилем/адаптером.
2. Для конфликтующих условий проверить приоритет по точным разделам manual; не возвращать первую случайную Rust-ошибку.
3. Разделить faulting RIP и next RIP для соответствующих event classes, error-code presence и fault address availability.
4. Проверить commit flags/registers/memory, REP restart и отсутствие host panic. Нельзя объявлять `#PF` корректным без определённых translation/permissions semantics.

**Готово:** у каждой поддержанной faulting формы есть positive и negative case с состоянием до/после; общий catch-all не скрывает внутренние ошибки.

#### S2-20a — literal fixture-v2 для локального `#DE`

Два fixture-v2 cases поднимают уже реализованный byte `DIV BL` `#DE` contract
до публичного CLI результата: нулевой divisor и quotient overflow. Они
проверяют vector 0, unavailable error code/fault address, `faulting_rip` на
начале инструкции, нулевое retirement и полный no-commit GPR/RFLAGS/RIP/memory.
Проверяются только одиночные арифметические fault conditions; конфликтующие
fault priorities и native exception delivery не заявляются. Cases остаются
в deferred pool, но не включены в native acceptance selection и не проверялись
на native. Литералы сериализует
`tests/cpu/generate_stage2_divide_errors.py`; источник — Intel SDM revision
093 Vol.2A DIV. Физическая проверка остаётся в общем финальном Mac mini
acceptance. На момент завершения S2-20a CLI fixture suite прошёл 26/26, workspace offline check, all 42
Stage 2 generator freshness checks, Stage 2 harness (11/11), formatting,
layout и diff checks прошли. Graphify обновлён. Независимое gpt-6.1-sol
medium review нашло и помогло устранить неверное описание native selection и
устаревшие pool/contract строки; повторное ревью чистое. Native/physical
execution этих divide-error fixtures остаётся отложенным.

#### S2-20b — complete `UD2` (#UD) exit

Один local-only fixture-v2 case содержит полный двухбайтовый opcode `0F 0B`.
CLI test проверяет vector 6, unavailable error code/fault address, faulting RIP
на первом байте, retirement=0 и полный no-commit GPR/RFLAGS/RIP/code memory.
Приоритет против fetch faults и invalid prefixes не утверждается. Case не
включён в native acceptance selection/исполнение; generic decode/fault mapping
и physical delivery не проверяются. Литералы сериализует
`tests/cpu/generate_stage2_ud2.py` по Intel SDM revision 093 Vol.2A UD2.
CLI fixture suite прошёл 27/27, generator tests — 2/2, все 43 Stage 2
generator freshness checks, Stage 2 harness — 11/11, workspace offline check,
formatting, layout и diff checks прошли. Graphify обновлён. Независимое
gpt-6.1-sol medium review чистое. Native и физическое исполнение остаются
отложенными.

#### S2-20c — qword IDIV quotient-overflow `#DE` exit

Один local-only fixture-v2 case содержит literal `48 F7 F9` (`IDIV RCX`) с
signed 128-bit minimum dividend (`RDX:RAX`) и divisor −1. CLI проверяет
vector 0, unavailable error code/fault address, faulting RIP на инструкции,
retirement=0 и полный no-commit GPR/RFLAGS/RIP/code memory. Это одиночный
quotient-overflow case: приоритет конфликтующих faults, native exception
delivery и физический hardware result не заявляются. Case не входит в native
acceptance selection или execution; deferred S2-15-onward pool увеличивается
с 592 до 593, native-selected subset неизменен. Литеральные вход и expected
snapshot сериализует `tests/cpu/generate_stage2_idiv_qword_error.py`, с
provenance на Intel SDM revision 093 Vol.2A IDIV. Физическая проверка остаётся
в общем финальном Mac mini acceptance substage. CLI suite прошёл 28/28; новый
generator test — 2/2; все 44 Stage 2 generators свежие; Stage 2 harness —
11/11; workspace offline check, formatting, layout и diff checks прошли.
Graphify и независимое gpt-6.1-sol medium review указаны в отчёте.

#### S2-20d — bounded UD2 fetch boundary

Закрепить только локальный `0F 0B` boundary: после доступного executable
первого и второго байта полный UD2 сообщает `#UD` после ровно двух fetch;
смежные executable `FlatMemory` regions дают тот же результат без третьего
fetch. Отсутствующий или injected ошибочный второй fetch остаётся
`MemoryError`/`HarnessError` либо сохраняет исходный architectural fault
payload; первый fetch failure не декодирует инструкцию. Все fault paths
оставляют retirement равным нулю и сохраняют полное CPU состояние и RAM;
data access не выполняется. Это контракт `FlatMemory` и callback, без вывода
о физическом fetch или приоритете hardware `#UD`.

Сохранить existing complete-UD2 literal CLI fixture и добавить adjacent
split-region fixture-v2 snapshot. Truncated/backend-error сценарии проверить в
`od-testkit`: fixture-v2 не имеет `HarnessError` в `expected_exit`, поэтому не
подменять их `#UD` и не расширять fixture schema только для этой проверки.
Не добавлять другие illegal opcodes, trap delivery, MMU, generic priority,
ISA scope или Native/JIT admission. Final Mac mini/SSH/physical acceptance
остаётся отдельным общим подэтапом.

**Результат S2-20d:** выполнено в локальном FlatMemory/callback contract;
полные проверки и ограничения зафиксированы в [отчёте](../reports/STAGE_2_REPORT.md#s2-20d--complete-ud2-fetch-boundary-in-local-memory-callbacks).
Независимое `gpt-6.1-sol` medium review не нашло замечаний P0–P2 по fetch
trace, границам, переносу ошибок, faulting RIP и отсутствию commit CPU/RAM.

### S2-21. Однопроцессорные atomic/fence формы

1. По профилю реализовать XCHG memory, CMPXCHG/XADD и допустимые LOCK forms, по одной группе за изменение.
2. Проверить допустимость LOCK, alignment/width/fault ordering и единый доступ через memory contract. Неправильный prefix не игнорировать.
3. Для fences и прочих ordering instructions определить требуемое поведение serial interpreter и contract будущего memory backend. Не выдавать отсутствие потоков за доказанную x86 memory model.

**Готово:** результаты/flags/side effects одного vCPU проверены; SMP гарантии явно отложены до этапа 6 и не заявлены.

#### S2-21a — bare qword memory XCHG

Добавлен обязательный `GuestMemory::atomic_exchange_integer` boundary: один
backend вызов возвращает прежнее значение, выполняет замену и не меняет память
при error. Исполнитель записывает старое значение в register только после
успешного atomic call; отдельные read/write не используются. Поддерживается
только bare effective REX.W + `87 /r` qword memory form, без LOCK и overrides. Один local-only
fixture проверяет RAX как одновременно base/source, unaligned qword через две
смежные области и точные canaries. `FlatMemory` подтверждает atomic method и
no-mutation-on-error внутри fixture backend; это не production backend, SMP,
native admission или hardware claim. Fixture добавлена в deferred pool: 593 →
594; native-selected subset не меняется. Физическая проверка остаётся общему
финальному Mac mini acceptance substage.

#### S2-21b — qword register-register XADD

Добавлена только bare effective REX.W + `0F C1 /r` register-register qword
форма. Literal fixture `48 0F C1 C0` использует RAX в обоих полях ModR/M и
проверяет сумму, полный GPR snapshot, arithmetic flags, RIP и retire count.
Decoder rejects memory destination, default dword, `66`/`67`, segment and LOCK
forms. Fixture local-only; native admission and physical claims unchanged.
Deferred pool grows 594 → 595. Memory XADD, other widths, CMPXCHG and LOCK
forms remain open in S2-21.

#### S2-21c — qword register-register CMPXCHG

Added only bare effective REX.W + `0F B1 /r` register-register qword
CMPXCHG. Two local-only literal fixtures cover comparison equal (RAX equals
RCX; RBX is written to RCX) and comparison unequal (RCX is both source and
destination; its old value is copied to RAX). Both verify full GPR state,
comparison flags, RIP and retire count. Decoder rejects memory destination,
default dword, `66`/`67`, segment and LOCK forms. Native admission and
physical claims are unchanged. Deferred pool grows 595 → 597; the native
selected subset does not grow. Memory CMPXCHG, memory XADD, other widths,
LOCK and fences remain open in S2-21.

#### S2-21d — bare qword memory XADD

Added `GuestMemory::atomic_fetch_add_integer`: one backend operation returns
the old value, stores a width-masked wrapping sum, and guarantees no memory
mutation on error. Executor snapshots the source before address calculation,
then performs exactly one memory operation; only after success it writes the
old value to the source register and updates arithmetic flags. One local-only
literal fixture covers RAX source/base aliasing, an unaligned qword crossing
adjacent regions, wrapping, and canaries. Spy and fault tests prove one atomic
call and no CPU/RAM commit on errors. This boundary does not imply LOCK, SMP,
production backend, or hardware atomicity. The fixture increases the deferred
pool 597 → 598; the native-selected subset is unchanged. Physical validation
remains in the shared final Mac mini acceptance substage.

#### S2-21e — bare qword memory CMPXCHG

Added bare effective REX.W + `0F B1 /r` qword CMPXCHG with a memory
destination. `GuestMemory::atomic_compare_exchange_integer` compares, validates
write access even on mismatch, and conditionally commits as one backend call;
errors leave memory unchanged. The executor snapshots RAX/source and computes
the effective address from pre-instruction state, then updates RAX and
arithmetic flags only after success. Two local-only literal fixtures cover
equal and unequal comparison across adjacent memory regions; the unequal case
uses RAX as address base and source. Spy/fault tests verify one operation,
mismatch write-access validation and no CPU/RAM commit on errors. No LOCK, SMP,
production-backend, native-admission or physical-execution claim is made. The
deferred pool grows 598 → 600; the native-selected subset is unchanged.
Physical validation remains in the shared final Mac mini acceptance stage.

#### S2-21f — LOCK qword memory XADD (private serial RAM only)

Added only `F0 + effective REX.W + 0F C1 /r` with a qword memory destination.
It reuses `GuestMemory::atomic_fetch_add_integer`; the backend call returns the
old destination, commits the wrapping sum, and errors leave CPU/RAM uncommitted.
One local-only literal fixture checks RAX source/base aliasing, an unaligned
split-region access and canaries. Decoder keeps other LOCK forms, widths,
register-destination XADD, `66`/`67`, segment and REP forms unsupported. This
form is supported only when the interpreter accesses private, synchronous RAM
serially, with no concurrent observers/writers, DMA, shared aliases, delayed
writes, or MMIO. The generic GuestMemory boundary does not provide full x86
LOCK ordering or SMP semantics. The unaligned local fixture checks backend
bytes only; it does not establish hardware #AC or split-lock behavior.
Deferred pool grows 600 → 601; the
native-selected subset is unchanged. Physical validation remains in the shared
final Mac mini acceptance stage.

#### S2-21g — bare MFENCE with private synchronous RAM opt-in

Adds only unprefixed `0F AE F0`. The required `GuestMemory::mfence` backend
method has no default success implementation: success explicitly promises all
prior guest reads have returned and prior writes/RMWs are committed before
later guest memory operations begin. Only the private synchronous `FlatMemory`
fixture backend opts in, because it owns canonical bytes and commits each write
before returning; the code-only fragment backend explicitly rejects the call.
The executor performs no operand access, changes no GPR/RFLAGS/memory, and
advances RIP/retirement only after the backend succeeds. An error leaves the
fence unretired and preserves CPU prestate; already-retired prior instructions
remain committed.

One local-only literal fixture checks store → MFENCE → load and full relevant
state. Spy/failure and fetch-boundary tests check one completion call, no
operand/data access, exact three-byte fetch, and no fence retirement on backend
error. Other `0F AE` forms and prefix variants remain outside this profile; this
does not claim they are architecturally invalid. The slice does not advertise
SSE2 through CPUID, model feature-dependent `#UD`, add native admission, or
claim shared-memory, MMIO, SMP or physical ordering. ISSUE-017 remains open for
general LOCK ordering; MFENCE does not resolve it. Deferred pool grows 601 →
602, native-selected subset unchanged. Physical checks remain in the shared
final acceptance stage.

## 8. Режимы, системное состояние и события

### S2-22. Сегментация и privilege checks

1. Ввести selectors и необходимый hidden descriptor state; определить segment base/limit/attributes для каждого выбранного режима.
2. Реализовать CPU-side проверки типов, присутствия, privilege и stack accesses, включая FS/GS в long mode по профилю.
3. Не сводить сегменты всех режимов к нулевым bases. Подключать descriptor memory через существующую memory boundary.

**Готово:** адреса и fault priorities проверены таблицами из источника и fixture backend; long-mode user cases дополнительно проверены hardware там, где Linux позволяет. Это не подтверждает hardware privileged path.

S2-22e is a bounded local-only extraction: bare `0F 00 E8` (`VERW AX`) at
CPL3 with GDT only. It checks read-only 8-byte descriptors, selector null and
table bounds, writable-data type and CPL/RPL/DPL, with ZF as the only flag
effect. The GDT read carries descriptor-table origin and supervisor page
privilege without changing instruction CPL. This does not close S2-22: segment
loads, hidden descriptor caches, present faults, stack checks, LDT, other CPLs
and other modes remain open. S2-27 excludes this exact form and continues to
own the remaining descriptor/event/system-transition instructions.

S2-22f/g is a partial local-only Long64/CPL3 load slice: exactly bare `8E E0`
(`MOV FS, AX`) whose AX selector is a non-null GDT selector. It reads one
complete descriptor through descriptor-table origin and checks
type/readability, privilege and presence in architectural priority order.
For valid P=1/A=0 it makes exactly one `GuestMemory::write` of `access | 1` at
`descriptor_address + 5`, using the same descriptor context, before updating the
FS selector/cache and committing RIP/retirement. A=1 performs no write. The
byte-sized backend call defines the chosen semantic memory access, not physical
bus width. Descriptor writes require private synchronous RAM, serial execution,
and no external observer/writer, DMA, shared alias, delayed write or MMIO. A
write error preserves the current CPU state and retirement; generic memory
backends must honor the existing no-mutation-on-error contract. Native profile
admission remains fail-closed. Null and TI/LDT selectors are Unsupported. This
does not close S2-22: general accessed-bit writes, LDT, GS/SS loads, other
CPLs/modes and stack checks remain open.

Local verification results for S2-22f/g are recorded in the Stage 2 report.
No SSH, Mac mini or physical acceptance is performed here; those remain in the
single shared final acceptance substage. S2-22 itself remains open.

S2-22h adds the exact bare `8E E8` (`MOV GS, AX`) form for Long64/CPL3 and a
non-null GDT selector. Its separate `s2-long-gs-load-v1` wire profile exposes
the GS selector, canonical48 base and descriptor cache attributes while
preserving full FS state. It reuses the bounded FS descriptor validator and
executor mechanics: descriptor-origin read8, architectural CPL3/page Ring0
context, S/type/readability then privilege then P then A, and exactly one
access-byte write1 at descriptor+5 only when A=0. Selector/cache and RIP/
retirement commit only after the final memory operation; A=1 is read-only.
Literal fixtures strictly compare full relevant state and faults, and the
generator checks handwritten provenance/freshness. Null/TI, other CPLs/modes,
prefixes, other sources and Sreg remain Unsupported. Native profile admission
fails closed. The same private synchronous RAM, serial executor and no external
observer/writer, DMA, alias, delayed write or MMIO assumptions apply as to FS;
no physical bus-width or SMP guarantee is made. S2-22 and ISSUE-017/018 remain
open. Local verification and independent review are recorded in the Stage 2
report; physical checks remain in the shared final acceptance substage.

S2-22i adds one handwritten literal local-only composition fixture in the
existing GS-load profile. The program is exactly `8E E8 65 48 8B 00`:
`MOV GS, AX`, followed by the already-supported `MOV RAX, qword ptr GS:[RAX]`.
Profile admission accepts only the existing bare two-byte GS load or this
exact six-byte program, with matching instruction budget and stop RIP. The
fixture starts at CPL3/Long64 with RAX `0x0b`, old GS.base `0x00400000`, and a
GDT descriptor at `0x00200008` whose A=0 writable DPL3 data entry loads
base32 `0x80000000` and limit 0. The first instruction sets only GDT byte
`0x0020000d` from F2 to F3 and retires at RIP `0x00100002`; the second reads
`0x1122334455667788` at `0x8000000b`, then stops at RIP `0x00100006` with two
retirements. A canary at old-base address `0x0040000b` remains unchanged.
The offset intentionally exceeds cache limit zero; S2-22b already records
that Long64 FS/GS runtime limit checks are absent. Trace assertions prove one
descriptor read8 at CPL3/pageRing0, the optional A-byte write1, then an
ordinary CPL3 data read8 with no GDT reread. Unit faults verify later data
read failure preserves the first instruction's state/write/retirement and
that RAX `0x00007fff8000000b` still supplies selector `0x000b` before the
loaded base creates noncanonical `0x000080000000000b`; the latter #GP(0) skips
the data callback. Canonicality/no-limit evidence reuses S2-22a/b checks and
does not claim new independent manual verification. The case is local-only;
S2-22 and ISSUE-017/018 remain open.

Local verification results for S2-22i are recorded in the Stage 2 report.

S2-22j adds the exact FS counterpart in the existing local-only
`s2-long-fs-load-v1` profile. It uses bytes `8E E0 64 48 8B 00`, with the
same descriptor, offset, GDT A-bit update, old-base canary and loaded-base
qword as S2-22i. It admits only the original `8E E0` program or this exact
six-byte composition with matching instruction budget and stop RIP. The
fixture does not make FS or GS profiles native eligible. Unit trace and fault
coverage, fixture provenance, and local verification are recorded in the
Stage 2 report. S2-22, ISSUE-017 and ISSUE-018 remain open.

### S2-23. Real mode при необходимости

1. Выполнять только если S2-00 требует этот режим. Реализовать нужные defaults, 16-bit addressing, prefix overrides, segment arithmetic и control transfers.
2. Повторно использовать decoder/executor с mode context; не писать отдельную копию integer semantics.
3. A20 и machine-address policies не угадывать: это явная внешняя зависимость, если выбранный путь их использует.

**Готово:** небольшие собственные реальные программы и state transitions проверены в оговорённом окружении. Linux x86-64 user runner не объявлен oracle real mode. Если режим не нужен — записано основание, а не фиктивное прохождение.

### S2-24. Protected и compatibility mode

1. Реализовать нужные 16/32-bit defaults, segmentation и изменения addressing/operand sizes.
2. Добавить требуемые far transfers и CPL checks отдельными подшагами. Не считать `66`/`67` в long mode полноценной реализацией compatibility mode.
3. Сформировать cases для mode-sensitive decoding одинаковых bytes и границ stack/segment.

**Готово:** необходимые режимы профиля имеют фактическое исполнение и проверенные faults. Независимая системная проверка закрывается S2-41.

### S2-25. CR, MSR и переходы режима

1. Реализовать только перечисленные в профиле CR0/CR2/CR3/CR4, EFER и нужные MSR с privilege/reserved-bit/transition checks.
2. Неизвестный MSR не становится произвольным key-value storage с нулём по умолчанию. Поведение задаётся CPU model.
3. Изменение control state выдаёт необходимые memory/event effects наружу; translation invalidation принадлежит machine backend. Нельзя принять новую CR3 без наблюдаемого требования к translation context.
4. Проверить переходы режима и запрещённые комбинации, связывая paging-dependent сценарии с этапом 4.

**Готово:** CPU-side правила работают и проверены; интеграционно зависимые строки явно blocked до появления backend. Наличие полей CR/MSR само по себе не закрывает задачу.

### S2-26. CPUID, системные инструкции и время

1. Реализовать profile-driven CPUID с согласованными leaves, widths и feature gates. Не копировать host CPUID: гостевой профиль не равен M3 Max или Mac mini.
2. Добавлять RDMSR/WRMSR, RDTSC и другие нужные системные формы по одной с privilege/exception rules. Время поступает из внешнего contract, а не случайного host clock.
3. Проверить, что каждый объявляемый бит имеет весь обязательный instruction/state/exception/save-restore support. При частичной поддержке отключить feature advertisement и оставить диагностическую матрицу точных форм.

**Готово:** негативный test ловит ложное объявление capability; CPUID согласован с state и реализованным профилем. Это не заменяет проверку минимальных требований выбранной ОС.

### S2-27. Descriptors, события и системные переходы

The bounded CPL3 GDT-only `VERW AX` form is tracked in S2-22e and is excluded
from this task. S2-27 remains responsible for the other descriptor instructions
and transitions listed below.

1. Добавить необходимые LGDT/LIDT и прочие descriptor instructions; затем CPU semantics INT/IRET, SYSCALL/SYSRET или других нужных переходов строго по профилю.
2. Определить pending event, masking/inhibition, порядок доставки, stack frame и fault escalation в требуемом объёме. До добавления STI/MOV SS/HLT проверить их влияние на event boundaries, а не только значение flag.
3. CPU получает событие через узкую границу; не добавлять APIC/timer scheduler в `od-isa`. Stack/descriptor memory проверяется через выбранный backend.
4. Вне принятого профиля return `unsupported`; внутри обязательного профиля неподдержанное поведение оставляет задачу незавершённой.

**Готово:** обязательные переходы имеют исполняемые tests с корректным state; полноценная доставка при paging/privilege transitions подтверждается интеграционно вместе с machine backend, а не mock callbacks.

## 9. x87, MMX и SSE/SSE2

Здесь особенно запрещены shortcuts: host `double` вместо extended precision, сравнение только обычных конечных значений, потеря NaN payload/classification или blanket masking всех FP differences. Если нужны библиотеки для общей инфраструктуры, они не должны реализовывать за проект x86/FP semantics.

### S2-28. Представить архитектурное состояние x87

1. Ввести восемь физических 80-bit значений, logical stack mapping/TOP, tags, control/status words и требуемые instruction/data pointers/opcode fields.
2. Определить reset/init, empty/full stack и pending exception state. Хранить исходные bits без преобразования в f64.
3. Добавить snapshot/compare support с точной availability и definedness.

**Готово:** stack mapping, FNINIT transition и lossless serialization проверены. Это состояние без арифметики; CPUID FPU ещё не объявляется готовым.

### S2-28a. Lossless Long64 x87 state snapshot (bounded local slice)

Added raw physical-register storage (8 × 10 bytes), full physical tag word,
control/status words, derived TOP and logical ST(i) mapping, plus Long64 FIP,
FDP and 11-bit FOP fields. The local `s2-long-x87-state-v1` fixture profile
admits exactly one NOP, allowing the fixture and result adapters to carry the
complete x87 image through `LongModeState` and compare it without claiming x87
instruction execution. The fragment adapter rejects `Some(x87)` explicitly.
`None` means the profile does not include x87 state. A deterministic fixture
seed is separate from the FNINIT transition; FNINIT resets FCW/FSW/FTW and
pointer/opcode fields while retaining all register payload bytes. Raw FSW/FCW
are the only pending-exception representation; this slice adds no delivery.
Native admission remains closed and no CPUID feature is advertised.

The exact Long64 pointer contract and selector omission are recorded in
[CPU_CONTRACT](../specs/CPU_CONTRACT.md#local-profile-s2-28a-s2-long-x87-state-v1).
The umbrella S2-28 remains open for state consumers, hardware observation,
and integration with later x87 instruction slices.

### S2-29. Реализовать собственное extended-precision ядро

1. Представить sign/exponent/significand и классы значений, включая zero, normal, subnormal, infinity, NaN и специальные x87 encodings по manual.
2. Реализовать собственные integer-based primitives нормализации, достаточной промежуточной точности, guard/round/sticky и округления; не использовать host FP как источник x86 rounding/exceptions.
3. Проверить границы exponent/significand, ties, signed zero, NaN classification и overflow/underflow/inexact на независимых вручную выведенных vectors.

**Готово:** bit-level tests проверяют числа и flags. Этот слой не объявляется всей x87 реализацией; physical end-to-end comparisons добавляются с инструкциями S2-30.

### S2-29a. Lossless binary80 raw representation and classification (partial)

`od-isa::x87::X87Binary80` wraps exactly ten memory-order bytes and exposes
the sign, 15-bit exponent, full 64-bit significand, explicit integer bit,
63-bit fraction and Q bit without conversion through host floating point.
Classification distinguishes zero, subnormal, normal, infinity, quiet/signaling
NaN, pseudo-denormal, unnormal, pseudo-infinity and quiet/signaling pseudo-NaN.
It only reads the wrapper; it does not consult or update architectural x87 tags
or state. Literal hand-authored byte vectors include signed forms, payload
boundaries and unsupported encodings, with an 80-bit walking-one extraction
check.

This is only the encoding layer; finite normalization is separately bounded
in S2-29b. There are no arithmetic, rounding, exception or instruction
semantics, and no `X87Tag` inference from the raw payload. S2-29 and S2-28
remain open.

Normative basis: Intel SDM revision 093, Vol. 1 §4.2.2/Table 4-3 and
§8.2.2/Table 8-3 ([253665-093US](https://cdrdv2.intel.com/v1/dl/getContent/671436)).

### S2-29b. Finite normalization and shift-jam primitives (partial)

`X87Binary80::normalize_finite()` returns an integer-only finite view for
zero, subnormal, pseudo-denormal and normal encodings. Nonfinite and unsupported
classes return `None`. The view carries sign, a normalized 64-bit significand,
the exponent associated with its leading bit, and the original raw class.
Subnormals are shifted until their leading one reaches bit 63 with the exponent
adjusted by the same amount. Pseudo-denormals use the minimum normal exponent
while retaining `PseudoDenormal` provenance; raw bytes and `classify()` are
unchanged. Zero retains its sign and uses zero significand/exponent.

`shift_right_jam()` preserves discarded nonzero information in bit zero,
including shifts at and beyond the 64-bit width. `X87Grs` exposes low bits 2/1/0
as guard/round/sticky after callers reserve those bit positions. These helpers
do not round, mutate x87 status, execute instructions, or use host floating
point. Tests cover signed zero, smallest/largest subnormal, pseudo-denormal,
minimum normal, maximum finite, unsupported encodings, and shift distances
0/1/63/64/65 with sticky and GRS cases. This is a primitive slice only:
rounding modes, arithmetic, exception behavior, and the S2-29/S2-28 umbrellas
remain open.

### S2-29c. Generic u128 right-shift-jam for extended significands (partial)

`shift_right_jam_u128(value, distance)` is a standalone integer primitive:
distance zero is identity; a nonzero distance below 128 shifts logically and
ORs every discarded one into bit zero; distances at least 128 return one for
nonzero input and zero otherwise. Repeated right jams preserve a sticky one;
left shifts cannot reconstruct discarded bits. The bridge from a normalized
64-bit finite significand is exactly `(significand as u128) << 3`. After a
right jam, only the low three bits are cast to `X87Grs`, and the retained
significand is read from `aligned >> 3`. This defines no rounding or arithmetic.

Independent literal vectors cover zero, bit 127, `u128::MAX`, repeated sticky
propagation, distances 0/1/2/3/63/64/65/66/67/127/128/129/`u32::MAX`, and the
bridge for signed zero, negative subnormal, normal, and negative
pseudo-denormal while preserving source-class metadata. S2-29 and S2-28 remain
open.

### S2-29d. Ordered finite-pair alignment to a common exponent (partial)

`align_finite_pair(lhs, rhs)` returns a common `i32` exponent and two `u128`
significands in the same order as its inputs. For two nonzero values the
common exponent is the larger input exponent; with one zero it is the
nonzero exponent, and two zeros use exponent zero. A zero aligns to zero.
Each nonzero significand is extended by three low GRS positions and shifted
right with `shift_right_jam_u128` by the exponent difference. Sign, source
class, and source values are unchanged; the operation does not sort operands.

Independent literal vectors cover exponent gaps 0/1/2/3/4/63/64/65/66/67/127/
128/129, minimum subnormal against minimum normal and maximum finite, maximum
subnormal against minimum normal, pseudo-denormal against a matching normal
with distinct provenance, negative exponents, both operand orders, and all
signed-zero pairs. A jammed low bit means only that a discarded tail was
nonzero. It does not identify exact magnitude or architectural inexact status.
Any later subtraction or left renormalization needs a separate information-
retention proof. Addition/subtraction, result normalization, rounding,
precision control, exceptions/status, special values, guest instructions, and
native snapshot remain outside this partial slice; S2-29 and S2-28 remain open.

### S2-29e. Exact signed sum for matching finite exponents (partial)

`add_finite_same_exponent(lhs, rhs)` returns `Some((exponent, coefficient))`
only when every nonzero input has the same exponent. The exact value is
`coefficient * 2^(exponent - 63)`, where each normalized 64-bit significand is
converted to a signed `i128` and added without alignment, jam bits, or host
floating point. A zero is neutral and uses the other operand's exponent; two
zeros return `(0, 0)` without choosing a signed zero. The coefficient is not
normalized, so carry and cancellation remain represented exactly. Two
nonzero unequal exponents return `None`. The input values, raw encodings, and
source-class provenance are unchanged; the result carries no source class.

Manual literal vectors cover positive and negative carry, maximum signed
coefficient sums, adjacent-significand cancellation to `+1` and `-1`, exact
cancellation, negative exponents, zeros in both operand orders, a
pseudo-denormal plus its matching normal with distinct provenance, unequal
exponent rejection, and source immutability. Jammed alignment, binary80
packing, result normalization, unequal-exponent arithmetic, rounding/PC/RC,
signed-zero selection, exceptions/flags, and instruction behavior remain
outside this partial slice. S2-29 and the S2-28 umbrella remain open.

### S2-29f. Exact coefficient normalization (partial)

`normalize_exact_coefficient(exponent, coefficient)` interprets the exact value
`coefficient * 2^(exponent - 63)` and returns `(sign, normalized_exponent,
magnitude)` with the magnitude's leading one at bit 127. For nonzero input,
`unsigned_abs` preserves the full `i128::MIN` magnitude, and normalization shifts
only left, so no coefficient bits are discarded or jammed. The normalized
exponent is `exponent + (floor(log2(abs(coefficient))) - 63)`; `None` means that
this final exponent does not fit `i32`. Zero returns `(false, 0, 0)` for any
input exponent and has no architectural sign.

Literal vectors cover `+1`/`-1`, `2^63`, `2^64`, `2^64 + 1` with its low bit
preserved, `2^65 - 2`, `i128::MAX`/`i128::MIN`, exponent boundary success and
overflow, and zero at both exponent limits. Composition vectors use
S2-29e's exact same-exponent sum for positive and negative adjacent
cancellation, exact zero, the maximum-finite sum with normalized exponent
16384, and the difference between adjacent significands at the minimum-normal
exponent. Inputs, raw encodings, and provenance remain unchanged. This helper
does not define x87 arithmetic, binary80 packing, GRS/rounding, clipping,
exceptions/flags, instructions, or CPUID behavior. S2-29 and the S2-28 umbrella
remain open.

### S2-29g. Exact finite significand product (partial)

`multiply_finite_significands(lhs, rhs)` multiplies the original normalized
finite significands directly as unsigned `u128` values. For nonzero operands,
the result `(sign, exponent, magnitude)` represents
`(-1)^sign * magnitude * 2^(exponent - 127)`, where `sign` is the XOR and
`exponent = lhs.exponent + rhs.exponent + 1`. The exact product can have its
leading one at bit 126 or 127; it is not normalized or narrowed through `i128`.
Checked exponent overflow returns `None`. Any zero pair returns
`(false, 0, 0)` with no architectural signed-zero choice. Inputs, raw values,
and source-class provenance are unchanged; jammed alignment data is not used.

Independent literal vectors cover minimum-significand squared (`2^126`),
maximum-significand squared, mixed boundary patterns, signs, commutativity,
signed zeros, exponent underflow/overflow/success boundaries, and composition
with normalized raw finite values that have distinct provenance. This helper
does not normalize, round, pack, define exceptions/status, architectural
overflow/underflow, or execute guest instructions/native code. S2-29 and the
S2-28 umbrella remain open.

### S2-29h. Exact unsigned magnitude normalization (partial)

`normalize_exact_magnitude(sign, exponent, magnitude)` interprets
`(-1)^sign * magnitude * 2^(exponent - 127)`. For nonzero input it shifts
the exact unsigned magnitude left until its leading one is at bit 127 and
subtracts that shift from the exponent with checked `i32` arithmetic. Zero
returns `(false, 0, 0)` for any sign and exponent. This helper accepts an
exact `u128` magnitude directly and does not pass through the signed
coefficient path.

Independent literal vectors cover the bit-126 and bit-127 cases, `u128::MAX`,
preservation of a low bit in `(2^64 + 1) << 63`, both signs, idempotence,
zero at both exponent limits, and success/failure at the `i32` minimum for
magnitudes 1 and `2^126`. Composition with S2-29g covers minimum-times-minimum,
maximum-times-maximum, and minimum-times-maximum. Inputs and provenance remain
unchanged. Division, rounding, GRS, packing, architectural flags, guest
instructions, and native execution remain outside this partial slice.

### S2-29i. Exact normalized finite quotient and remainder (partial)

`divide_finite_significands(lhs, rhs)` returns an `X87FiniteQuotient` that
represents the exact value
`(-1)^sign * (quotient + remainder / denominator) * 2^(exponent - 63)`.
For nonzero normalized significands `a` and `d`, it chooses `k = 63` when
`a >= d`, otherwise `k = 64`, then computes `D = (a as u128) << k`,
`quotient = D / d`, and `remainder = D % d`. The result keeps the original
denominator, has `2^63 <= quotient < 2^64` and `remainder < denominator`, and
uses exponent `(lhs.exponent - rhs.exponent) + (63 - k)` with checked final
`i32` conversion. The sign is the input sign XOR.

A zero divisor returns `None`, including 0/0; zero numerator with a nonzero
divisor returns unsigned zero fields and retains the denominator. This does
not select architectural signed zero or define guest #Z behavior. Literal
vectors cover equal-exponent quotient/remainder boundaries, both sign
combinations, denominator retention, zero precedence, source/provenance
preservation, raw subnormal and pseudo-denormal composition, and synthetic
`i32` exponent boundaries. The calculation does not use alignment, jam, GRS,
rounding, packing, architectural flags/exceptions, guest instruction
execution, or native execution. S2-29 and S2-28 remain open.

### S2-29j. Pure unsigned rational nearest-integer ties-to-even (partial)

`round_quotient_ties_even(q, r, d)` rounds the unsigned rational value
`q + r / d` to its nearest integer, choosing an even result for an exact tie.
It returns `None` only when `d == 0` or `r >= d`; the result is widened to
`u128` so incrementing `u64::MAX` preserves the carry as `2^64`. The half-way
comparison widens `r` before doubling, so it is defined for the full `u64`
range.

Literal vectors cover below/at/above half, both quotient parities at a tie,
exact integers, invalid inputs, a `u64::MAX` carry, and a large remainder that
requires the widened comparison. Composition vectors feed exact S2-29i
quotient/remainder results into this helper. Exact ties and carry are synthetic
generic rational vectors: the normalized S2-29i divider outputs cannot reach
those cases. This helper does not implement x87 rounding modes/control word,
x87 RC/PC, binary80 formatting or packing, guest FDIV, signs, exponent
adjustment, architectural flags/exceptions, or signed zero. S2-29 and S2-28
remain open.

### S2-29k. Pure mathematical rational rounding modes (partial)

`round_quotient_magnitude(sign, q, r, d, mode)` rounds the exact coefficient
`(-1)^sign * (q + r/d)` to the integer lattice and returns its unsigned
magnitude. `RationalRoundingMode` contains nearest/ties-to-even, toward zero,
toward positive, and toward negative. The sign affects only whether either
directed mode increments the magnitude. The helper returns `None` only for a
zero denominator or `r >= d`; `r == 0` preserves `q` for every mode. The old
`round_quotient_ties_even` API remains as a positive nearest-even wrapper.

Literal vectors cover all modes for positive and negative fractions, both
nearest-even quotient parities, small values around zero, exact integers,
invalid inputs for every mode, carry beyond `u64::MAX`, and a widened half-way
comparison. Composition vectors use S2-29i quotient, remainder, denominator,
and sign getters for positive and negative results. Synthetic tie/carry cases
remain generic rational vectors unreachable from normalized S2-29i output.
These are mathematical modes only: no control-word encoding, precision
parameter, exponent adjustment, signed zero, x87 RC/PC, packing, status, or
instruction behavior is introduced. S2-29 and S2-28 remain open.

### S2-29l. Direct exact quotient rounding to significant precision (partial)

`round_quotient_to_precision(sign, q, r, d, precision, mode)` rounds the exact
normalized coefficient `(-1)^sign * (q + r/d)` directly to 1–64 significant
bits. It accepts only `d != 0`, `r < d`, and either the exact zero pair or a
normalized nonzero `q` with bit 63 set. The result is a widened unsigned
magnitude; a carry to `2^precision` remains visible and does not mutate an
exponent. For an S2-29i divider exponent `e`, the rounded value is represented
as `(-1)^sign * magnitude * 2^(e + 1 - precision)`. The caller owns that scale;
this helper does not adjust the exponent.

Nearest rounding compares the discarded quotient bits and exact remainder
without multiplication or an intermediate rounding. Directed modes use the
coefficient sign and whether any discarded information is nonzero. Literal
vectors cover all four modes and signs, p=1/2/24/53/63/64, tie parity, carry,
the `d = u64::MAX` half boundary, invalid inputs, and a direct-rounding case
that would change under double rounding. p=64 synthetic ties/carry are
unreachable from normalized divider output; reduced-precision ties remain
valid tested cases. Composition uses exact S2-29i results for 1/3 at p=24, 53,
and 64, including negative directed modes.

This is mathematical precision rounding only: it does not implement x87 PC/RC
or control-word values, exponent normalization/adjustment, subnormal/range
handling, binary80 packing, flags/exceptions, signed-zero selection, or guest
FDIV. S2-29 and S2-28 remain open.

### S2-29m. Bounded normal finite binary80 packing (partial)

`pack_normal_rounded_significand(sign, exponent, magnitude, precision)` packs
an already-rounded nonzero magnitude representing
`(-1)^sign * magnitude * 2^(exponent + 1 - precision)`. It accepts precision
1–64 and a magnitude in `[2^(precision-1), 2^precision]`. Values below the
carry boundary are shifted exactly into the explicit 64-bit significand; a
magnitude equal to `2^precision` becomes `0x8000000000000000` and increments
the exponent once. The final exponent must be in `[-16382, 16383]`; encoding is
the little-endian significand followed by the little-endian sign/exponent word.
No additional rounding occurs.

Literal byte vectors cover positive and negative values, p=1/2/24/53/63/64,
carry, the minimum normal exponent and carry into it, and the maximum finite
exponent. Composition packs exact S2-29i 1/3 results at p=24/53/64 and packs
negative p=24 directed-rounding results. Invalid precision, zero/out-of-range
magnitudes, exponent endpoints outside the finite normal range, checked carry
overflow, and integer exponent extremes return `None`.

This is a bounded normal packing helper only. Subnormal numeric packing is
covered by S2-29n; pseudo encodings, infinity, NaNs, architectural
overflow/underflow behavior, control-word rounding, flags, exceptions,
signed-zero selection, and guest instructions remain outside this slice.

### S2-29n. Direct exact quotient-to-subnormal binary80 rounding/packing (partial)

`round_pack_subnormal_quotient(sign, e, q, r, d, mode)` consumes the exact
nonzero value `(-1)^sign * (q + r/d) * 2^(e - 63)` for `q >= 2^63`, `d > 0`,
`r < d`, and `e <= -16383`. It computes `k = -16382 - e` in `i64` before any
narrowing. For `1 <= k <= 63`, it calls the existing exact quotient precision
rounder once at `p = 64-k`; `k=64` compares the exact coefficient with the
half-minimum-subnormal boundary and rounds to zero or one; `k>=65` rounds to
zero except for directed rounding away from zero. The final integer magnitude
is encoded with exponent field zero below `2^63`, or as minimum normal at
`2^63`. The input sign is retained even when rounding yields zero.

Literal byte vectors cover signs and all mathematical modes, exact subnormals,
half minimum subnormal, `k=1/63/64/65`, minimum/maximum subnormal, carry to
minimum normal, `i32::MIN`, invalid inputs, and a direct-rounding case that
would differ after rounding an intermediate magnitude. Composition uses exact
S2-29i divider getters for minimum normal divided by two and three.

This helper adds numerical normal/subnormal finite packing only. It does not
define architectural underflow/precision flags or exceptions, x87 RC/PC,
control-word behavior, pseudo/special values, signed-zero operation rules, or
guest FDIV. S2-29 and S2-28 remain open.

### S2-29o. Compose finite division with one-round binary80 packing (partial)

`divide_finite_to_binary80(lhs, rhs, mode)` rejects either zero operand before
calling the exact divider, since this bounded helper does not choose an
operation's signed-zero result. For nonzero operands it retains the exact
quotient/remainder and uses sign XOR. An unrounded quotient exponent at or
above `-16382` takes one 64-bit precision round followed by normal packing;
lower exponents go directly to subnormal quotient packing with the original
exact tail. `None` is limited-helper rejection, including zero operands and
values outside the finite binary80 exponent range, not guest #Z or overflow.

Tests cover exact normal and subnormal values, inexact positive/negative 1/3
under directed modes, minimum normal, largest subnormal and its reachable
carry to minimum normal, minimum subnormal, half/below/above-half minimum
subnormal, signed zero from tiny nonzero values, maximum finite and rejected
upper range, all sign-XOR combinations, zero operands, and source/provenance
immutability. The subnormal anti-double-rounding vector is checked through the
complete divider-to-pack composition. Normal p=64 synthetic ties/carry remain
unreachable from normalized divider outputs; subnormal carry is reachable.
This remains a numerical helper only: architectural control-word, flags,
exceptions, special-value and guest FDIV semantics remain open under ISSUE-019.

### S2-29p. Round an exact coefficient once to finite binary80 (partial)

`round_pack_exact_coefficient(exponent, coefficient, mode)` rounds
`coefficient * 2^(exponent - 63)` into finite normal or subnormal binary80.
It rejects coefficient zero because no sign is selected for an exact zero,
then calls `normalize_exact_coefficient` and decomposes its exact even `u128`
magnitude as `q=(magnitude >> 64)`, `r=(magnitude as u64) >> 1`,
`d=2^63`. Normal-range values use one p=64 quotient round then normal packing;
subnormal-range values pass the original `q/r/d` directly to subnormal packing.
Only bounded normalization and packer rejection propagates as `None`.

Literal vectors cover positive/negative `2^63`, normal ties-to-even with even
and odd retained significands, a synthetic normal carry to 4, minimum and
half-minimum subnormal, directed signed-zero results, the subnormal
anti-double-round case (`exponent=-16385`, `coefficient=2^64+11` gives
significand `0x2000000000000001`), largest subnormal, tie into minimum normal,
maximum finite, rejected upper range, `i128::MIN/MAX`, exponent edges, and zero
rejection. The synthetic inexact subnormal vectors test general coefficient
math; those tails are unreachable from exact binary80 same-exponent sums.
Composition with S2-29e checks positive/negative cancellation to ±1, carry,
one zero operand, unequal-exponent rejection, and immutable inputs. No jammed
alignment or pre-rounded subnormal coefficient is used. This numerical helper
does not implement x87 RC/PC, status, exceptions, instruction behavior,
architectural signed zero, or special values; S2-29, S2-28, and ISSUE-019 remain
open.

### S2-29q. Compose same-exponent finite addition with one-round binary80 packing (partial)

`add_finite_same_exponent_to_binary80(lhs, rhs, mode)` composes the exact
same-exponent signed coefficient with `round_pack_exact_coefficient`. One zero
operand is neutral in either order; two zeros and exact cancellation return
`None`, because this helper does not select an architectural zero sign.
Unequal nonzero normalized exponents return `None`; this slice does not align
or jam operands. Output is canonical finite binary80. `None` is bounded helper
rejection, including finite-range overflow, not a guest exception.

Raw-byte vectors normalized through the finite decoder cover positive and
negative `1+1`, positive and negative normal ties under all four mathematical
rounding modes, adjacent minimum normals subtracting to either signed minimum
subnormal, maximum subnormal, two half-minimum-normal values summing to minimum
normal, minimum-subnormal addition, zero with normal/subnormal/pseudo-denormal
in either order, maximum finite plus zero, upper-range rejection, cancellation,
all four zero-sign pairs, and unequal exponents. Input bytes and finite
provenance remain unchanged.

Reachability limits: two 64-bit significands have exact signed-sum magnitude at
most `2^65-2`; the synthetic `2^65-1` coefficient that can carry to four in a
generic coefficient rounder cannot arise here. Every finite binary80 value is
an integer multiple of minimum subnormal, so an exact sum in the subnormal
range is also a minimum-subnormal multiple. Inexact subnormal tails,
half-minimum-subnormal ties, and underflow-to-zero are therefore unreachable
from this addition slice, though covered by the general coefficient helper.
This does not implement FADD/FSUB decoding or execution, stack/control-word,
flags, exceptions, or architectural zero selection. S2-29, S2-28, and ISSUE-019
remain open.

### S2-30. Основные x87 операции

1. Добавлять отдельными подшагами FLD/FST/FSTP, stack operations/constants, basic arithmetic и comparisons из профиля.
2. Соблюдать source precision, control-word precision/rounding, stack pop timing и status effects конкретной формы.
3. Расширить native snapshot на x87 без изменения состояния до захвата; сравнивать 80-bit bits и архитектурно определённые fields.

**Готово:** обычные и граничные operands проходят native comparison, включая stack underflow/overflow и memory fault effects поддержанного профиля.

#### S2-30a. Bare FNINIT (bounded local slice)

Implement only the exact unprefixed bytes `DB E3` with an already-present
`Some(X87State)`. Apply the existing `X87State::fninit()` transition, advance
RIP by two and retire once. Preserve all GPRs, RFLAGS, and all eight data
payloads. Other `DB` forms, every prefix, and `9B DB E3` FINIT stay
unsupported. The local profile assumes x87 is available; CR0.EM/TS and #NM are
not modeled. Missing x87 state remains Unsupported. Native admission and FPU
feature advertisement remain closed. Tests cover decode neighbors/truncation,
arbitrary seeded payload preservation, full environment reset, and no commit
for fetch fault, STOP, budget exhaustion, or missing state.

#### S2-30b. Bare FNSTSW AX (bounded local slice)

Implement only the exact unprefixed bytes `DF E0` with an already-present
`Some(X87State)`. Copy the original raw 16-bit FSW to AX using the existing
word register write, preserving RAX[63:16], other GPRs, RFLAGS, and the local
x87 snapshot. The local snapshot's deterministic retention of C0/C1/C2/C3 is
an implementation policy; those four bits are architecturally undefined after
FNSTSW and are not architectural preservation claims. Reject every prefix,
neighboring `DF` encoding, memory form, and `9B DF E0`; missing state is
Unsupported. The local profile continues to assume x87 availability without
CR0.EM/TS checks, #NM delivery, or WAIT. Fetch fault, STOP and exhausted budget
must not commit. Native x87-profile execution and FNSTSW admission remain
closed. Do not synthesize x87 state or advertise an FPU feature.

#### S2-30c. Bare FNCLEX (bounded local slice)

Implement only exact unprefixed `DB E2` with an already-present
`Some(X87State)`. Clear FSW[0:7] and FSW[15], preserving TOP and the concrete
C0/C1/C2/C3 values only as a deterministic local policy; Intel leaves C0–C3
undefined. Preserve FCW, FTW, all data payloads and pointer/opcode fields,
GPRs, RFLAGS, and MXCSR. Advance RIP by two and retire once. The local profile
assumes x87 availability and adds no CR0.EM/TS, #NM, WAIT, pending-exception
check, or floating-point exception delivery. Other `DB` encodings, all
prefixes, `9B DB E2`, and missing state remain Unsupported. Fetch fault, STOP,
and budget exhaustion must not commit. Native x87-profile execution and FNCLEX
admission remain closed.

#### S2-30d. Bare exact-only FADD ST(0), ST(1) (bounded local slice)

Implement only bare `D8 C1` with an existing `Some(X87State)`. Require FCW
`0x037f`, a clean FSW exception/ES/B mask (`0x80ff`), Valid tags and canonical
Normal binary80 payloads at physical TOP and TOP+1, and equal normalized
exponents. Compute with `add_finite_same_exponent` and
`round_pack_exact_coefficient` only after proving exact representability:
coefficient magnitude at most `u64::MAX`, or a wider even 65-bit magnitude.
Require the packed result to remain canonical Normal. Reject zero, cancellation,
unequal exponents, specials, pseudo/denormal encodings, bad tags/environment,
inexact coefficients, overflow and zero/subnormal results before any mutation.

On success replace only the physical TOP payload, clear FSW.C1, set FIP to the
instruction RIP, advance RIP by two and retire once. Preserve TOP, tags, source
and other payloads, FCW, GPRs, RFLAGS and remaining FSW fields. C0/C2/C3 and FDP
are retained only as local deterministic policy because the SDM leaves them
undefined for this operation; FOP is unchanged on this no-exception path. The
profile still assumes x87 availability and does not model CR0.EM/TS, #NM, FPU
capability, general special values, or exception semantics. Tests use literal
binary80 vectors for exact carry and 1+1, inexact 65-bit sums, cancellation,
overflow and minimum-normal cancellation, plus TOP wrap, decode neighbors,
prefixes, truncation, full-state rejection and native fail-closed admission.

#### S2-30e. Extend exact-only FADD to every register source (bounded local slice)

Accept only bare `D8 C0+i`, `i=0..7`, as `FADD ST(0), ST(i)`. Keep every
S2-30d arithmetic and environment restriction unchanged: `Some(X87State)`, FCW
`0x037f`, clean FSW mask `0x80ff`, Valid tags for used operands, canonical
Normal payloads, equal normalized exponents, exact coefficient gate and Normal
output. The destination remains physical TOP and source is `(TOP+i)&7`. For
`i=0`, read TOP once as both operands and require no other tag. Read both
operands before mutation and commit the destination payload exactly once.

Success preserves S2-30d update behavior: clear C1, set FIP to RIP, retain
TOP/tags/GPRs/RFLAGS/FCW/other payloads and fields; C0/C2/C3 and FDP follow
the existing local deterministic policy, and FOP remains unchanged. Reject
unsupported forms and inputs without commit. Prefixes, other D8 ModR/M values,
memory/DC/DE/WAIT remain unsupported; native x87 admission stays closed.
Independent literal vectors cover every i and varied TOP values, including
`i=0` with empty unrelated tags, plus wrap coverage for `i=1, TOP=7` and `i=7, TOP=7`. Schema, runner and
native rejection checks cover all eight encodings. This extends only source
indexing, not the arithmetic domain or architectural x87 claims.

#### S2-30f. Bare exact-only FADD ST(i), ST(0) (bounded local slice)

Accept only bare `DC C0+i`, `i=0..7`, as `FADD ST(i), ST(0)`. Keep every
S2-30d arithmetic/environment condition and result restriction unchanged.
The source is physical TOP and the destination is `(TOP+i)&7`. For `i=0`,
read the shared register once and require only that tag; for other indices,
both used tags must be Valid. Read both operands and validate the canonical
Normal result before any mutation. Success writes only the destination payload,
clears C1, sets FIP to starting RIP, advances RIP by two and retires once;
there is no pop, and TOP/tags remain unchanged. Preserve other payloads,
GPRs/RFLAGS, FCW, remaining FSW fields, FDP and FOP under the S2-30d local
policy for undefined C0/C2/C3 and FDP. Prefixes, other DC ModR/M bytes,
memory forms, DE, WAIT and native execution remain unsupported.

Independent expected-byte vectors cover all destination indices, varied TOP
and wrap, the `i=0` alias with unrelated empty tags, and source preservation
for `i!=0`. No-commit rejects cover bad tags, specials, unequal exponents and
inexact coefficient input. Decoder tests enumerate the full DC ModR/M space,
prefixes and truncation; testkit schema and native fail-closed admission cover
all eight accepted byte encodings. This extends destination selection only;
the exact-only numerical domain does not widen.

#### S2-30g. Bare exact-only FADDP ST(i), ST(0) (bounded local slice)

Accept only bare `DE C0+i`, `i=0..7`, as `FADDP ST(i), ST(0)`. Reuse the
S2-30d exact FADD arithmetic and environment domain with logical destination
`i` and source zero. Capture both inputs and validate the canonical Normal
result before mutation. Success writes the result bytes to old physical
`(TOP+i)&7`, marks old physical TOP Empty, increments TOP modulo eight, clears
C1, records starting RIP as FIP, advances RIP by two, and retires once. Do not
rotate the physical register array or tag word. For `i=0`, result bytes are
written to old TOP before its tag is emptied and are no longer logically
accessible; for `i=1`, the result becomes new ST(0). Preserve all other raw
payloads, tags, FCW, other FSW bits, FDP/FOP and integer state under the local
policy. Native admission remains closed.

Independent vectors cover every `i` and TOP, physical payload sentinels,
`i=0`, `i=1` new-ST(0), TOP wrap, exact carry, and full-state rejection
including i=0 overflow. Decoder and fixture schema enumerate exactly the eight
bare encodings. This slice does not claim general FADDP, special values, stack
faults, exception delivery or CR0 behavior.

#### S2-30h. Bare exact-only FSUB ST(0), ST(i) (bounded local slice)

Accept only bare `D8 E0+i`, `i=0..7`, as `FSUB ST(0), ST(i)`. Retain the
S2-30d environment and canonical Normal input gates, Valid tags for physical
TOP and `(TOP+i)&7`, equal normalized exponents, exact coefficient gate, and
canonical Normal output. Compute `ST(0) - ST(i)` by inverting the sign in a
local copy of the already-validated source; never rewrite the source payload.
Reject `i=0` unconditionally because it aliases the destination and every
result is exact cancellation. Reject all unsupported values before mutation.

On success replace only physical TOP payload, clear C1, set FIP to old RIP,
advance RIP by two and retire once. Preserve TOP, FTW, FCW, FDP, FOP, other
FSW bits, all other payloads, GPRs and RFLAGS. Retaining undefined C0/C2/C3
and FDP is local policy. Native admission remains closed and CPUID/features do
not change.

Literal vectors cover both signs, positive and negative differences, the
smallest normal boundary, a 64-bit carry and exact `2^-63`; odd 65-bit exact
coefficients, exact subnormal output and overflow reject atomically. All TOP
and `i=1..7` pairs verify physical placement and sentinels. i=0 rejects for
all TOP. Decoder coverage includes complete D8 ModR/M space, prefixes, WAIT,
truncated fetch and reverse/memory subtraction neighbors; schema/runner cover
all seven successes and the i=0 Unsupported outcome. Native profile/admission
rejects all eight encodings.

#### S2-30i. Bare exact-only FSUB ST(i), ST(0) (bounded local slice)

Accept only bare `DC E8+i`, `i=0..7`, as `FSUB ST(i), ST(0)`. Retain the
S2-30d environment, used Valid tags, canonical Normal inputs, equal normalized
exponents, exact coefficient and canonical Normal result gates. Compute
`ST(i) - ST(0)` by changing the sign in a local copy of the validated source
ST(0); preserve its original bytes. Reject `i=0` because destination aliases
source and the result is exact cancellation. Reject missing/bad state, unequal
exponents, non-normal classes, rejected coefficients, zero/subnormal output
and overflow before mutation.

On success replace only physical `(TOP+i)&7`, clear C1, set FIP to old RIP,
advance RIP by two and retire once. Preserve source ST(0), TOP, tags, all other
payloads, FCW, other FSW bits, FDP/FOP and integer state under the established
local policy. Native admission stays closed. Literal arithmetic vectors,
every TOP×i placement, alias and unsupported-state atomicity, exhaustive DC
ModR/M and prefix/fetch boundaries, fixture schema, CLI runner, and native
denial are required. Full x87 architectural behavior remains outside this
bounded slice.

#### S2-30j. Bare exact-only FSUBP ST(i), ST(0) (bounded local slice)

Accept only bare `DE E8+i`, `i=0..7`, as `FSUBP ST(i), ST(0)`. Reuse the
S2-30i exact subtraction gate and result helper with old physical destination
`(TOP+i)&7` and source TOP; reject alias `i=0` as exact cancellation. Validate
the complete operation before mutation. On success write the result to the
destination first, mark old physical TOP Empty, increment TOP modulo eight,
clear C1, set FIP to old RIP, advance RIP by two and retire once. Do not rotate
payloads or FTW. Unsupported arithmetic, environment, operand, and fetch
conditions leave state unchanged. Native admission stays closed for all eight
codes.

Cover literal subtraction and boundary values, every TOP×i physical placement
and pop/wrap behavior, alias and numeric/domain rejects with full-state
equality, zero-budget/stop/fetch boundaries, exhaustive DE ModR/M and prefixes,
fixture schema, and native denial. Normative source: Intel SDM rev.093 Vol.2A
pp.3-394–3-396, FSUBP `DEST − SRC` and destination-before-pop semantics.

#### S2-30k. Bare exact-only FSUBR ST(0), ST(i) (bounded local slice)

Accept exactly bare `D8 E8+i`, `i=0..7`, as `FSUBR ST(0), ST(i)`. Reuse the
S2-30i exact subtraction gate as `ST(i) − ST(0)`; the helper's logical
destination/source indices do not change the architectural destination, which
is the old physical TOP. `i=0` is Unsupported because the operands alias and
the exact result cancels. Validate every condition before mutation.

Success replaces only old physical TOP payload, clears C1, sets FIP to the
starting RIP, advances RIP by two and retires once. Preserve TOP, FTW, FCW,
FDP, FOP, other FSW bits, other payloads, GPRs and RFLAGS. Cover literal
direction/sign vectors, ±2^-63, maximum even 65-bit carry, minimum Normal,
every TOP×i physical placement, full-state domain rejects, fetch/stop/budget,
exhaustive D8 ModR/M/prefix boundaries, fixture-schema alias (`stop_rip=None`,
budget 1), literal runner outputs and alias snapshot. Native profile/admission
remains closed. Normative source: Intel SDM rev.093 Vol.2A pp.3-397–3-399,
FSUBR computes `SRC − DEST`, with C1 clear on exact results and C0/C2/C3
undefined.

#### S2-30l. Bare exact-only FSUBR ST(i), ST(0) (bounded local slice)

Accept exactly bare `DC E0+i`, `i=0..7`, as `FSUBR ST(i), ST(0)`. Reuse
`exact_fsub_result(state, 0, i)` to compute `ST(0) − ST(i)` and store at old
physical `(TOP+i)&7`. The `i=0` alias is Unsupported because exact cancellation
is outside this slice. Preserve the S2-30d exact numeric/environment domain
and reject every unsupported input before commit.

Success changes only the old physical `ST(i)` payload, clears C1, sets FIP to
the starting RIP, advances RIP by two and retires once. Preserve TOP, FTW,
source physical TOP payload, all other x87 fields, GPRs and RFLAGS. Decoder
coverage includes the complete DC ModR/M and prefix matrix; schema accepts
DC E0..E7 and requires alias DC E0 to be Unsupported with no stop RIP and
budget one. Native admission stays closed. Normative source: Intel SDM
revision 093, Vol.2A pp.3-397–3-399; FSUBR computes SRC−DEST, C1 is cleared
for exact results, and C0/C2/C3 are undefined.

#### S2-30m. Bare exact-only FSUBRP ST(i), ST(0) (bounded local slice)

Accept exactly bare `DE E0+i`, `i=0..7`, as `FSUBRP ST(i), ST(0)`. Reuse
`exact_fsub_result(state, 0, i)` to compute old `ST(0) − ST(i)` and store the
result at old physical `(TOP+i)&7` before popping. The `i=0` self-alias is
Unsupported because it cancels exactly; the no-operand `DE E1` alias selects
`ST(1), ST(0)` (`i=1`). Preserve the S2-30d exact numeric
and environment domain; all rejections precede commit.

On success, mark the old physical TOP Empty while retaining its payload, set
TOP to `(old TOP+1)&7`, clear C1, set FIP to starting RIP, advance RIP by two
and retire once. Preserve every other payload/tag and x87/GPR/RFLAGS field.
The schema admits all eight codes and requires DE E0 alias Unsupported with
no stop RIP and budget one. Exhaustive DE decoding preserves DE C0..C7 FADDP
and DE E8..EF FSUBP. Native admission stays closed. Normative source: Intel
SDM revision 093, Vol.2A pp.3-397–3-399, “FSUBR/FSUBRP/FISUBR—Reverse
Subtract”; FSUBRP computes ST(0)−ST(i) before
pop; exact results clear C1 while C0/C2/C3 are undefined.

#### S2-30n. Bare exact-only FMUL ST(0), ST(i) (bounded local slice)

Accept exactly bare `D8 C8+i`, `i=0..7`, as `FMUL ST(0), ST(i)`. Require an
existing x87 state, FCW `0x037f`, `FSW & 0x80ff == 0`, Valid tags for used
physical registers, and canonical Normal raw operands. Exponents may differ.
Use `normalize_finite`, `multiply_finite_significands`,
`normalize_exact_magnitude`, and `pack_normal_rounded_significand`; accept only
when the normalized exact product's low 64 bits are zero and the packed result
is canonical Normal. Reject inexact results, zero/subnormal/overflow outputs,
special or noncanonical inputs, bad environment/tags, and missing state before
mutation. `i=0` is the valid `ST(0)*ST(0)` square and requires only one tag.

Success writes only the ten-byte result to old physical TOP, clears C1, sets
FIP to starting RIP, advances RIP by two, and retires once. Preserve TOP, tags,
all other payloads, FCW, other FSW fields, FDP/FOP, GPRs, and RFLAGS. Retention
of C0/C2/C3 is local deterministic policy because they are undefined. Decoder
tests cover all D8 ModR/M values, prefixes, truncation and neighboring forms;
schema/runner cover all eight indices and native x87 admission stays closed.
Normative source: Intel SDM revision 093, Vol.2A pp.3-360–3-362, “FMUL/FIMUL—
Multiply”.

#### S2-30o. Bare exact-only FMUL ST(i), ST(0) (bounded local slice)

Accept exactly bare `DC C8+i`, `i=0..7`, as `FMUL ST(i), ST(0)`. Reuse
S2-30n's exact multiplication gate, including differing exponents, canonical
Normal operands and the zero-low-64-bits exactness requirement. The destination
is physical `(TOP+i)&7`; source is physical TOP. `i=0` is the valid square and
requires only one Valid tag. Reject every inexact/range/environment/tag/raw
class failure before commit.

Success writes only the selected physical payload, clears C1, records starting
RIP as FIP, advances RIP by two and retires once. Preserve TOP, tags, every
other payload and x87/GPR/RFLAGS field. Decode must add only DC C8..CF while
preserving DC C0..C7 FADD, E0..E7 FSUBR and E8..EF FSUB. Fixture schema and
runner cover all eight indices; native x87 admission stays closed. Native
rejection also explicitly covers the existing DE E0..E7 FSUBRP form.
Normative source: Intel SDM revision 093, Vol.2A pp.3-360–3-362, “FMUL/FIMUL—
Multiply”.

#### S2-30p. Bare exact-only FMULP ST(i), ST(0) (bounded local slice)

Accept exactly bare `DE C8+i`, `i=0..7`, as `FMULP ST(i), ST(0)`. Reuse
S2-30n's exact multiplication gate: existing x87 state, FCW `0x037f`,
`FSW & 0x80ff == 0`, Valid tags for used physical operands, canonical Normal
binary80 inputs, exact normalized product with zero low 64 bits, and canonical
Normal packed output. Exponents may differ. `i=0` is a valid square and needs
one Valid tag; capture the product before marking that same old TOP tag Empty.
The no-operand `DE C9` encoding is the `i=1` form,
`FMULP ST(1), ST(0)`.
Reject inexact/range/environment/tag/raw-class failures before mutation.

Success writes the product to old physical `(TOP+i)&7`, marks old physical TOP
Empty while retaining its payload, increments TOP, clears C1, records starting
RIP as FIP, advances RIP by two and retires once. Preserve other tags,
payloads, x87 fields, GPRs and RFLAGS. Decoder coverage enumerates DE ModR/M,
prefixes, WAIT and truncation while preserving FADDP DE C0..C7, FSUBRP DE
E0..E7 and FSUBP DE E8..EF. The fixture schema and runner cover DE C8..CF,
including exact i=0 square success and an inexact `DE C8` Unsupported fixture
with no stop RIP and budget one. Native x87 admission stays closed. Normative
source: Intel SDM revision 093, Vol.2A pp.3-360–3-362, “FMUL/FIMUL—Multiply”;
DE C8+i computes old `ST(i)*ST(0)` before the pop, exact results clear C1 and
C0/C2/C3 are undefined.

#### S2-30q. Bare exact-only FDIV ST(0), ST(i) (bounded local slice)

Accept exactly bare `D8 F0+i`, `i=0..7`, as `FDIV ST(0), ST(i)`. Require
existing x87 state, FCW `0x037f`, `FSW & 0x80ff == 0`, Valid tags for the used
physical operands, canonical Normal binary80 inputs and an exactly divisible
normalized significand quotient. Require a canonical Normal precision-64
result; reject zero, specials, noncanonical raw inputs, nonzero quotient
remainder, overflow and subnormal/zero output before mutation. Exponents may
differ. `i=0` reads one Valid operand and yields exact `+1`, including for a
negative operand. Other D8 division directions, DC/DE register forms and
memory/integer forms remain unsupported.

Success writes only old physical TOP, clears C1, sets FIP to starting RIP,
advances RIP by two and retires once; preserve tags, TOP and every other field.
Enumerate all D8 ModR/M bytes, prefixes, WAIT and truncation; preserve adjacent
D8 groups. Fixture schema and runner cover the eight forms and atomic inexact
rejection. Native admission remains closed. Normative source: Intel SDM
revision 093, Vol.2A pp.3-333–3-335, “FDIV/FIDIV—Divide.”

#### S2-30r. Bare exact-only FDIVP ST(i), ST(0) (bounded local slice)

Accept exactly bare `DE F8+i`, `i=0..7`, as `FDIVP ST(i), ST(0)`, including
the no-operand `DE F9` alias for `i=1`. Require existing x87 state, FCW
`0x037f`, `FSW & 0x80ff == 0`, Valid tags for the used physical operands,
canonical Normal binary80 inputs and an exactly divisible normalized
significand quotient. Require a canonical Normal precision-64 result; reject
zero, specials, noncanonical raw inputs, nonzero quotient remainder, overflow
and subnormal/zero output before mutation. Exponents may differ. `i=0` uses
one Valid tag and computes self-division as exact `+1`, writes the payload to
old TOP, then marks that tag Empty and increments TOP. `i>0` computes old
`ST(i)/ST(0)`, writes old physical `(TOP+i)&7`, empties old TOP and increments
TOP. Other DE groups retain their meanings; `DE F0..F7` FDIVRP and all other
FDIV directions/memory/integer forms remain unsupported.

Exact success clears C1, sets FIP to starting RIP, advances RIP by two and
retires once. Enumerate every DE ModR/M byte, prefixes, WAIT and truncation;
preserve adjacent groups. Fixture schema and runner cover all eight forms and
atomic inexact `DE F9` rejection. Native admission remains closed. Normative
source: Intel SDM revision 093, Vol.2A pp.3-333–3-335, “FDIV/FIDIV—Divide.”

#### S2-30s. Bare exact-only FDIVRP ST(i), ST(0) (bounded local slice)

Accept exactly bare `DE F0+i`, `i=0..7`, as `FDIVRP ST(i), ST(0)`, including
the no-operand `DE F1` alias for `i=1`. Require existing x87 state, FCW
`0x037f`, `FSW & 0x80ff == 0`, Valid tags for used physical registers,
canonical Normal binary80 inputs and an exactly divisible normalized
significand quotient. Require a canonical Normal precision-64 result; reject
zero, specials, noncanonical raw inputs, nonzero quotient remainder, overflow
and subnormal/zero output before mutation. Exponents may differ. `i=0` uses
one Valid tag and computes self-division as exact `+1`, writes the payload to
old TOP, then marks that tag Empty and increments TOP. `i>0` computes old
`ST(0)/ST(i)`, writes old physical `(TOP+i)&7`, empties old TOP and increments
TOP. Other DE groups retain their meanings; `DE F8..FF` FDIVP remains
unchanged and other FDIV directions/memory/integer forms remain unsupported.

Exact success clears C1, sets FIP to starting RIP, advances RIP by two and
retires once. Enumerate every DE ModR/M byte, prefixes, REX, WAIT and truncation;
preserve adjacent groups. Fixture schema and runner cover all eight forms and
atomic inexact `DE F1` rejection with no stop RIP and budget one. Native
admission remains closed. Normative source: Intel SDM revision 093, Vol.2A
pp.3-336–3-338, “FDIVR/FDIVRP/FIDIVR—Reverse Divide.”

#### S2-30t. Bare exact-only FDIVR ST(0), ST(i) (bounded local slice)

Accept exactly bare `D8 F8+i`, `i=0..7`, as `FDIVR ST(0), ST(i)`; `D8 F9`
encodes `FDIVR ST(0), ST(1)`. Require existing x87 state, FCW
`0x037f`, `FSW & 0x80ff == 0`, Valid tags for used physical registers,
canonical Normal binary80 operands and an exactly divisible normalized
significand quotient. Compute old `ST(i)/ST(0)` with the shared ordered exact
divide gate. Require a canonical Normal precision-64 result; reject zero,
specials, noncanonical raw inputs, nonzero quotient remainder, overflow and
subnormal/zero output before mutation. Exponents may differ. `i=0` reads one
Valid tag and computes self-division as exact `+1`, writing old physical TOP
without a pop or tag/TOP changes. `D8 F0..F7` FDIV remains distinct; DC/DE
reverse divide forms and memory/integer forms remain outside this slice.

Exact success clears C1, sets FIP to starting RIP, advances RIP by two and
retires once. Enumerate every D8 ModR/M byte, prefixes, REX, WAIT and
truncation; preserve adjacent groups. Fixture schema and runner cover all
eight forms and atomic inexact `D8 F9` rejection with no stop RIP and budget
one. Native admission remains closed. Normative source: Intel SDM revision
093, Vol.2A pp.3-336–3-338, “FDIVR/FDIVRP/FIDIVR—Reverse Divide.”

#### S2-30u. Bare exact-only FDIVR ST(i), ST(0) (bounded local slice)

Accept exactly bare `DC F0+i`, `i=0..7`, as `FDIVR ST(i), ST(0)`. Require
existing x87 state, FCW `0x037f`, `FSW & 0x80ff == 0`, Valid tags for the used
physical operands, canonical Normal binary80 inputs and an exactly divisible
normalized significand quotient. Compute old `ST(0)/ST(i)` using the shared
ordered exact divide gate and require a canonical Normal precision-64 result.
Reject zero, specials, noncanonical inputs, nonzero quotient remainder,
overflow and subnormal/zero output before mutation. Exponents may differ. `i=0`
uses one Valid tag and yields exact `+1`, retaining its tag and TOP. `DC F0..F7`
is distinct from the existing `DC` add/multiply/subtract groups; memory forms,
prefixes, WAIT and all other unsupported encodings remain outside this slice.

Success writes the result to old physical `(TOP+i)&7`, clears C1, records
starting RIP as FIP, advances RIP by two and retires once. All other tags,
payloads, x87 fields, GPRs and RFLAGS remain unchanged. Exhaustively check DC
ModR/M, prefixes/REX/WAIT and truncation. Cover all TOP×i placements, signs,
normal edges and a non-power-of-two quotient; inexact `1/3` must return
Unsupported without state changes. The fixture schema and interpreter runner
admit the eight exact forms; native admission explicitly denies all eight
encodings. Native x87 admission remains closed.

Normative source: Intel SDM revision 093, Vol.2A pp.3-336–3-338,
“FDIVR/FDIVRP/FIDIVR—Reverse Divide.”

#### S2-30v. Bare exact-only FDIV ST(i), ST(0) (bounded local slice)

Accept exactly bare `DC F8+i`, `i=0..7`, as `FDIV ST(i), ST(0)`. Require an
existing x87 state, FCW `0x037f`, `FSW & 0x80ff == 0`, Valid tags for the used
physical operands, canonical Normal binary80 inputs and an exactly divisible
normalized significand quotient. Compute old `ST(i)/ST(0)` using the shared
ordered exact divide gate and require a canonical Normal precision-64 result.
Reject zero, specials, noncanonical inputs, nonzero quotient remainder,
overflow and subnormal/zero output before mutation. Exponents may differ. `i=0`
uses one Valid tag and yields exact `+1`, retaining its tag and TOP. `DC F8..FF`
remains distinct from `DC F0..F7` FDIVR; memory forms, prefixes, WAIT and all
other unsupported encodings remain outside this slice.

Success writes the result to old physical `(TOP+i)&7`, clears C1, records
starting RIP as FIP, advances RIP by two and retires once. All other tags,
payloads, x87 fields, GPRs and RFLAGS remain unchanged. Exhaustively check DC
ModR/M, prefixes/REX/WAIT and truncation. Cover all TOP×i placements, signs,
normal edges and ordered `15/3=5`; inexact `1/3` must return Unsupported
without state changes. The fixture schema and interpreter runner admit the eight
exact forms; the `DC F9` inexact rejection has no stop RIP and budget one.
Native admission explicitly denies all eight encodings and remains closed.

Normative source: Intel SDM revision 093, Vol.2A pp.3-333–3-335,
“FDIV/FIDIV—Divide” ([official PDF, Vol.2A p.3-333](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=451)).

#### S2-30w. Bare bounded `FSTP ST(i)` (local raw-copy slice)

Accept only bare `DD D8+i`, `i=0..7`, with an existing x87 state. Require
FCW `0x037f`, `FSW & 0x80ff == 0`, a Valid tag at old physical TOP, and a
canonical Normal binary80 source. Capture the source's ten raw bytes and old
TOP before mutation. Do not inspect the destination's old payload or tag.
Success writes the exact bytes to old physical `(TOP+i)&7`, marks that tag
Valid, then marks old TOP Empty and increments TOP modulo eight. This ordering
means the `i=0` alias retains the copied bytes while the old TOP tag ends Empty;
for `i=1`, the copied value is new ST(0). The physical register array is never
rotated.

FIP becomes the instruction-start RIP and RIP advances by two. C1 and every
other FSW bit outside TOP are retained as an explicit local policy; FCW, FDP,
FOP, GPRs, RFLAGS and all unrelated payloads/tags are unchanged. Missing state,
bad environment, invalid source tag/class, fetch fault/truncation, STOP and
exhausted budget preserve full state and do not retire. No memory form,
rounding/conversion, exception delivery, `#NM`, stack-fault handling or native
x87 admission is included.

Decoder coverage enumerates the complete DD ModR/M byte space and rejects
prefixes, REX, WAIT and truncation. Interpreter vectors cover every TOP/index
pair, the destination's four previous tag values for `i!=0`, raw signed and
Normal-edge copies, the `i=0`/`i=1` aliases, untouched sentinels and atomic
rejection. The schema admits all eight encodings plus one source-tag rejection
fixture (`DD D9`, no stop RIP, budget one). Native admission explicitly denies
all eight forms.

Normative source: Intel SDM revision 093, Vol.2A, “FST/FSTP—Store Floating-Point
Value,” pp.3-386–3-387 ([official PDF, Vol.2A p.3-386](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=504)).

#### S2-30x. Bare bounded `FST ST(i)` (local raw-copy slice)

Accept only bare `DD D0+i`, `i=0..7`, with an existing x87 state. Reuse the
bounded x87 store source gate: FCW `0x037f`, `FSW & 0x80ff == 0`, a Valid tag
at old physical TOP, and a canonical Normal binary80 source. Capture the
source's ten raw bytes and old TOP before mutation. Do not inspect the
destination's old payload or tag. Success writes those bytes to old physical
`(TOP+i)&7` and marks the destination Valid. TOP and all FSW bits are retained,
including C1; for `i=0`, source bytes and its Valid tag are retained. The
physical register array is never rotated.

FIP becomes the instruction-start RIP and RIP advances by two. FCW, FDP, FOP,
unrelated payloads/tags, GPRs and RFLAGS remain unchanged. Missing state, bad
environment, invalid source tag/class, fetch fault/truncation, STOP and
exhausted budget preserve full state and do not retire. No memory form,
rounding/conversion, exception delivery, `#NM`, stack-fault handling or native
x87 admission is included. This deterministic local policy is not a claim of
hardware validation for the bounded slice.

Decoder coverage enumerates the complete DD ModR/M byte space while retaining
`DD D8..DF` as FSTP, and rejects prefixes, REX, WAIT and truncation. Interpreter
vectors cover every TOP/index pair, each prior destination tag for `i!=0`, raw
signed/minimum/maximum Normal bytes, the `i=0` alias, full FSW preservation
and atomic rejection. The schema admits all eight encodings plus a source-tag
rejection fixture (`DD D1`, no stop RIP, budget one). Native admission
explicitly denies all eight forms.

Normative source: Intel SDM revision 093, Vol.2A, “FST/FSTP—Store Floating-Point
Value,” pp.3-386–3-387 ([official PDF, Vol.2A p.3-386](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=504)).

#### S2-30y. Bare bounded `FCHS` (sign-bit-only local slice)

Accept exactly bare `D9 E0` with an existing `Some(X87State)`. Reuse the
canonical-Normal ST(0) source gate: FCW `0x037f`, `FSW & 0x80ff == 0`, Valid
tag at old physical TOP and canonical Normal binary80 bytes. Snapshot all ten
source bytes, then flip only `bytes[9] ^= 0x80` in old physical TOP. Clear C1
while preserving TOP, tags and all other fields; C0/C2/C3 retention is the
deterministic local snapshot policy. FIP becomes instruction-start RIP and RIP
advances by two, with exactly one retirement.

Missing/bad state, tag, class or environment; unsupported D9 bytes, every
prefix/REX/WAIT, truncation, STOP and budget exhaustion leave state unchanged.
This slice excludes zero/subnormal/special values, memory forms, CR0/#NM,
exception delivery, native admission and FPU feature claims.

Decoder coverage checks all 256 second bytes with only E0 accepted, plus
prefix, REX, WAIT and truncation. Literal vectors cover all TOP positions,
positive and negative minimum/maximum/full-significand Normal encodings,
exactly one sign-bit change, C1 initially clear/set, and two FCHS executions
that restore the payload while C1 remains clear. Rejection vectors compare full
state across environment/tag/class, missing-state, STOP and budget boundaries.
The schema/CLI includes a successful case and a genuine source-tag rejection
with no stop RIP and budget one. Native admission explicitly denies `D9 E0`.

Normative source: Intel SDM revision 093, Vol.2A, “FCHS—Change Sign,” p.3-318
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=436)).

#### S2-30ao. Admit canonical signed zero to bounded `FCHS` (`D9 E0`)

Extend only FCHS's source gate: retain the existing FCW `0x037f`,
`FSW & 0x80ff == 0` and canonical-Normal Valid source behavior, and additionally
accept a Zero-tagged ST(0) only when all eight significand bytes are zero and
the sign/exponent word is exactly `0x0000` or `0x8000`. Flip only raw bit
`source[9] ^= 0x80`, retain Zero tag/TOP/other payloads, clear C1, retain
C0/C2/C3 by the existing snapshot policy, set FIP and advance RIP by two, then
retire once. At this historical point FTST and FST/FSTP ST(i) used the
Normal-only source gate; later S2-30aq/ar/as extend those forms respectively.
FCHS and FABS may share their narrow sign-operation gate.

Cover both signed zeros × all TOP × all 16 prior condition-code combinations,
full-state and double-operation restoration, malformed payload/tag/environment
and execution-boundary atomic rejection, Normal regressions, and FLD m32/m64 →
FCHS → FST → FNSTSW composition. CLI/schema fixtures remain project fixtures;
native/JIT stay closed. Physical and Mac mini acceptance remain under ISSUE-019.

#### S2-30ap. Admit canonical signed zero to bounded `FABS` (`D9 E1`)

Extend FABS's source admission to match FCHS's bounded domain: retain FCW
`0x037f`, `FSW & 0x80ff == 0`, and canonical-Normal Valid ST(0), and additionally
accept Zero-tagged ST(0) only for exact signed zero (zero significand and
sign/exponent word `0x0000` or `0x8000`). Clear only the raw sign bit, preserve
Zero tag/TOP/payload and unrelated state, clear C1, retain C0/C2/C3 by the
existing local snapshot policy, set FIP, advance RIP by two and retire once.
Share only the narrow FCHS/FABS sign-operation admission helper. At this
historical point FTST and FST/FSTP ST(i) remained Normal-only; S2-30aq and
S2-30ar supersede that statement for FTST and bare FST respectively.

Cover both signed zeros × all TOP × all 16 prior condition-code combinations,
full-state and repeated-FABS idempotence, malformed payload/tag/environment
and fetch/prefix/truncation/STOP/budget atomicity, Normal/FCHS regressions, and
FLD m32/m64 → FABS → FST → FNSTSW composition. The handwritten project fixture
uses exactly `d900d9e1d910dfe0` (four retirements); native/JIT and physical
acceptance remain closed under ISSUE-019.

#### S2-30aq. Admit canonical signed zero to bounded `FTST` (`D9 E4`)

Keep bare `D9 E4`, existing x87 state, FCW `0x037f`, and `FSW & 0x80ff == 0`.
Accept the existing canonical-Normal Valid source and exact Zero-tagged signed
zero only (zero significand and sign/exponent word `0x0000` or `0x8000`).
Normal results remain less/greater by sign; either signed zero compares equal
and sets C3 only using `(old_fsw & !0x4700) | 0x4000`. Preserve sign/raw bytes,
tag, TOP, FCW/FDP/FOP, other state, record FIP, advance RIP by two, and retire
once. At this historical point FST/FSTP ST(i) retained their Normal-only
source gate; S2-30ar later extends bare FST; the FSTP limit here is historical and S2-30as supersedes it.
Native/JIT remain closed.

Cover both signed zeros × all TOP × all 16 prior condition-code states,
repeated-operation idempotence, malformed zero/tag/environment rejection,
m32/m64 FLD→FTST→FNSTSW for both signs, Normal regressions, and
fetch/prefix/REX/WAIT/truncation/STOP/budget atomicity.
The exact handwritten fixture `d900d9e4dfe0` composes FLD m32fp, FTST, and
FNSTSW in three retirements. Physical behavior remains open under ISSUE-019.

#### S2-30at. Admit canonical signed infinity to bounded `FTST` (`D9 E4`)

Keep the existing exact bare encoding, explicit x87 state, FCW `0x037f`, and
`FSW & 0x80ff == 0` gate. In addition to canonical-Normal Valid values and
exact Zero-tagged signed zero, admit only Special-tagged canonical binary80
infinity: exponent `0x7fff`, integer bit `J=1`, and zero fraction. Either sign
is less/greater than zero respectively. Commit the status with the existing
`(old_fsw & !0x4700) | C0(if negative)` rule, clearing C1 and C2/C3 while
preserving the other FSW bits. Preserve TOP, tag, raw bytes and all other state;
record FIP, advance RIP by two, and retire once. Do not widen FCHS/FABS/FST/
FSTP or any other x87 source gate. Pseudo-infinity, NaNs, subnormal and other
unsupported/malformed values, wrong tags, and invalid environments reject
atomically. JIT/native admission stays closed.

Cover both signs × all TOP positions × all 16 old condition-code states,
repeated execution, Normal and signed-Zero regressions, wrong tags/classes,
invalid/missing x87 state, and fetch/prefix/REX/WAIT/truncation/STOP/budget
atomicity. Add only the exact existing `d900d9e4dfe0` project-fixture
composition for binary32 positive and negative infinity inputs; do not widen
the schema. Local fixture/interpreter checks do not establish physical x87
behavior; keep ISSUE-019 open.

#### S2-30ar. Admit canonical signed zero to bare `FST ST(i)` (`DD D0+i`)

Retain bare `DD D0+i`, existing x87 state, FCW `0x037f`, `FSW & 0x80ff == 0`,
and existing Valid-tag canonical-Normal source behavior. Additionally accept
only a Zero-tagged exact signed-zero source: the first nine bytes are zero and
byte 9 is `0x00` or `0x80`. Capture source bytes/tag before mutation, copy to
physical `(old_TOP+i)&7`, and set the destination tag to Zero for zero or Valid
for Normal. Ignore destination old payload/tag for `i!=0`. For `i=0`, preserve
the same bytes/tag; never pop or rotate the stack. Preserve TOP, all FSW bits,
and unrelated state; set FIP, advance RIP by two, and retire once. Retained
condition bits, including C1, remain the deterministic local policy.

Cover both signed zeros × all eight TOP values × all eight indices × all 16
prior condition-code states, destination tag/payload sentinels, i=0
idempotence, existing Normal vectors, and wrong-tag/malformed-zero/environment
and execution-boundary atomic rejection. FSTP's Normal-only scope here is
historical and is superseded by S2-30as. The local
m32/m64 composition and exact project fixture `d900ddd1dfe0` perform
FLD→FST ST(1)→FNSTSW in three retirements. Native/JIT remain closed and
physical acceptance remains deferred under ISSUE-019.

#### S2-30as. Admit canonical signed zero to bare `FSTP ST(i)` (`DD D8+i`)

Use the shared FST/FSTP register-store source gate: retain the existing
`0x037f` FCW, `FSW & 0x80ff == 0`, Valid-tag canonical-Normal behavior and add
only Zero-tagged exact signed zero (bytes 0..8 all zero and byte 9 `0x00` or
`0x80`). Do not widen unrelated Normal-only consumers. Before mutation capture
the source bytes/tag, physical old TOP, and index. Write bytes/tag to physical
`(old_TOP+i)&7`; then mark old physical TOP Empty; then set TOP to
`(old_TOP+1)&7`. Do not rotate registers. For `i=0`, payload stays while its
final tag is Empty; for `i=1`, the copy becomes new ST(0).

Preserve every FSW bit other than TOP, including C1, plus FCW/FDP/FOP, GPRs,
RFLAGS and unrelated payloads/tags. Set FIP to instruction RIP, advance RIP by
two, and retire once. Reject malformed/wrong-tag/special sources, bad controls,
missing state, prefixes/REX/WAIT, truncation/fetch fault, STOP and exhausted
budget atomically. Native/JIT remain closed.

Cover both zero signs × all TOP values × all indices × all 16 condition-code
states with full-state comparisons and prior destination tag/payload sentinels
for `i!=0`; retain Normal cases and explicitly cover `i=0`, `i=1`, and TOP=7
wrap. Add m32/m64 FLD→FSTP ST(1)→FNSTSW compositions for both signs and the exact
fixture `d900ddd9dfe0` (length six, three retirements, existing x87 fixture
profile). Preserve the earlier S2-30w Normal-only evidence as historical.
S2-30au later extends both register-store gates to Special-tagged canonical
signed infinity. Physical x87 acceptance remains open under ISSUE-019.

#### S2-30au. Admit canonical signed infinity to bounded `FST/FSTP ST(i)`

Extend only `bounded_x87_fst_sti_source`: retain Valid-tag canonical Normal and
exact Zero-tagged signed zero, and additionally admit the pair
`(Special, Infinity)` for canonical binary80 infinity (E=`0x7fff`, J=1,
fraction=0), either sign. Preserve FCW `0x037f`, `FSW & 0x80ff == 0`, source
bytes/tag, and existing C1 policy. Bare `DD D0+i` copies all ten bytes and
Special tag to physical `(old_TOP+i)&7`, without TOP change. Bare `DD D8+i`
performs the same copy, then empties old physical TOP and advances TOP modulo
eight. For `i=0` bytes persist with final Empty tag; for `i=1` the copied value
becomes new ST(0). Pseudo-infinity, NaNs, subnormal, malformed encodings and
wrong tag/class pairs remain atomic rejects. Do not alter JIT/native admission.

Cover both signs × both opcodes × all TOP × all indices × all 16 prior
condition-code states with complete-state and physical destination canaries;
explicitly check alias, `i=1`, TOP wrap, and FSTP ordering. Rejections cover
valid infinity under bad FCW/FSW, malformed and neighboring classes, wrong
tags, missing state, and fetch/prefix/REX/WAIT/truncation/STOP/budget boundaries.
Compose m32 and m64 FLD infinity with FST/FSTP ST(1) and FNSTSW for both signs.
Reuse only existing fixture byte patterns `d900ddd1dfe0` and
`d900ddd9dfe0`; the schema profile does not expand. Local interpreter, fixture,
JIT-denial, workspace and layout checks do not establish native or physical
behavior. Keep ISSUE-019 open; no Mac mini, SSH or physical x86 checks belong to
this substage.

#### S2-30av. Admit canonical signed infinity to bounded `FCHS/FABS`

Extend only `bounded_x87_sign_st0_source`: retain FCW `0x037f`,
`FSW & 0x80ff == 0`, canonical-Normal Valid and exact Zero-tagged signed-zero
behavior, and additionally admit `(Special, Infinity)` for canonical binary80
infinity (E=`0x7fff`, J=1, fraction=0), either sign. FCHS flips only the raw
sign bit; FABS clears only that bit. Preserve the Special tag, TOP, C0/C2/C3,
all other payload/state, and the existing C1-clear/FIP/RIP/retirement behavior.
Keep pseudo-infinity, NaNs, pseudo-NaNs, subnormal, pseudo-denormal, unnormal,
other malformed special values and wrong tags atomically unsupported. Do not
change JIT/native gates.

Cover both opcodes × both signs × all eight TOP positions × all 16 prior
condition-code values, with full-state expectations, idempotent FABS, double
FCHS restoration, neighboring-class/tag/environment rejection and canaries.
Check fetch, truncation, STOP and budget atomicity with admitted infinity and
compose FLD m32/m64 → each sign operation → FST → FNSTSW. Keep the exact
project-fixture instruction streams `d900d9e0d910dfe0` and
`d900d9e1d910dfe0`; add CLI success/rejection and JIT denial assertions without
changing the schema. Local evidence does not establish physical x87 behavior;
ISSUE-019 remains open and no Mac mini/SSH/physical x86 check belongs here.

#### S2-30aw. Admit canonical binary80 QuietNaN to bounded `FCHS/FABS`

Extend only `bounded_x87_sign_st0_source`: retain FCW `0x037f`,
`FSW & 0x80ff == 0`, canonical-Normal Valid, exact Zero-tagged signed zero,
and Special-tagged canonical infinity behavior. Additionally admit only
`(Special, QuietNaN)` where the canonical binary80 encoding has E=`0x7fff`,
J=1 and Q=1; accept either sign and every low-62-bit payload value, including
zero. FCHS changes only the sign bit and FABS clears only the sign bit. Keep
the Special tag, TOP, C0/C2/C3 policy, all payload bits, and other state; clear
C1 and add no exception flags. Reject SNaN, pseudo-NaN, pseudo-infinity,
subnormal, pseudo-denormal, unnormal and wrong tags atomically. Do not change
the classifier/schema or JIT/native gates.

The interpreter matrix covers both opcodes × both signs × all eight TOP values
× all 16 prior condition-code states × payloads `{0, 1, 1<<61, (1<<62)-1}`
with independent raw-byte full-state expectations, repeated FABS and double
FCHS. Add detailed malformed-class/tag/environment and fetch/truncation/STOP/
budget/fetch-fault rejection. Exact schema-v2 fixtures use the existing m32
FLD QNaN → each sign operation → FST → FNSTSW composition for both signs;
direct interpreter compositions cover both signs and both operations at m32
and m64 widths. CLI assertions include full state, JIT denial and a wrong-tag
fixture. Keep ISSUE-019 open; no Mac mini/SSH/native runtime/physical x86 checks
belong to this substage.

#### S2-30ax. Admit canonical binary80 SignalingNaN to bounded `FCHS/FABS`

Extend only `bounded_x87_sign_st0_source`: retain FCW `0x037f`,
`FSW & 0x80ff == 0`, canonical-Normal Valid, exact Zero-tagged signed zero,
Special-tagged canonical infinity and QuietNaN behavior. Additionally admit
only `(Special, SignalingNaN)` where binary80 has E=`0x7fff`, J=1, Q=0 and a
nonzero lower 62-bit payload; accept either sign. The zero-payload J=1/Q=0
encoding remains admitted through the existing canonical-infinity path. FCHS
toggles only the sign bit and FABS clears only it. Preserve Q=0, every payload
bit, Special tag, TOP and all other state, clear C1, add no IE/PE, and retain
the local C0/C2/C3 policy. Reject pseudo-NaN, pseudo-infinity, subnormal,
pseudo-denormal, unnormal, wrong tags and invalid environments atomically.
Do not change decode/classifier/schema or JIT/native admission.

The interpreter matrix covers both opcodes × signs × TOP × 16 prior condition
codes × payloads `{1, 1<<61, 0x0123456789abcdef, (1<<62)-1}`. Compare full
state and canaries, test double FCHS and repeated FABS, and reject malformed
classes/tags/environment and fetch/truncation/STOP/budget/fetch-fault
boundaries. A unit composition loads raw `FLD m80fp`, checks the SNaN bytes,
applies FCHS/FABS and checks unchanged Q/payload, stores with `FSTP m80fp`,
and observes status with `FNSTSW AX`; it covers both signs and operations
without converting through m32/m64. Four exact schema-v2 CLI fixtures perform
the bare sign operations across both signs/operations with a seeded SNaN and
budget one; a wrong-tag CLI fixture verifies atomic rejection. All four
success fixtures are denied by JIT. Keep ISSUE-019 open; defer Mac mini, SSH,
native runtime and physical
x86 acceptance to the combined final substage.

#### S2-30ay. Admit canonical binary80 subnormal to bounded `FCHS/FABS`

Extend only `bounded_x87_sign_st0_source`: preserve FCW `0x037f`,
`FSW & 0x80ff == 0`, canonical-Normal Valid, exact Zero-tagged signed zero,
and Special-tagged canonical infinity, QNaN and SNaN. Additionally admit only
`(Special, Subnormal)` for canonical binary80 E=0, J=0 and nonzero 63-bit
fraction, with either sign. FCHS toggles only the sign bit; FABS clears only
that bit. Preserve all fraction bits, Special tag, TOP, C0/C2/C3 and all other
state, clear C1 and add no DE/UE/PE/IE. Pseudo-denormal E=0,J=1,
unnormal, pseudo-special and wrong tags remain atomically unsupported. Do not
change the classifier, schema, decoder, other source gates or JIT/native
admission.

Cover both operations × both signs × all eight TOP values × 16 prior condition
codes × fractions `{1, 1<<62, 0x0123456789abcdef, (1<<63)-1}` with full-state
expectations and canaries. Include double-FCHS/repeated-FABS, adjacent classes,
wrong tags, invalid/missing x87 state and fetch/truncation/STOP/budget/fault
atomicity. A raw `FLD m80fp` → FCHS/FABS → `FSTP m80fp` → `FNSTSW AX`
composition inspects bytes after load, operation and store for both signs and
opcodes and confirms no DE. Four budget-one bare schema-v2 fixtures seed raw
subnormals in ST(0), check both operations/signs and JIT denial; a wrong-tag
fixture checks atomic rejection. Correct the prior malformed-vector comment:
`[0,0,0,0,0,0,0,0,1,0]` is E=1,J=0 unnormal, not subnormal. Keep ISSUE-019
open and defer Mac mini/SSH/native/physical acceptance to the combined final
substage.

#### S2-30bc. Admit historical sticky IE to bounded `FCHS/FABS`

Change only `bounded_x87_sign_st0_source`: retain exact FCW `0x037f` and
change its FSW filter from `0x80ff` to `0x80fe`, admitting historical IE
(bit 0) independently of the ST(0) class. Continue rejecting DE/ZE/OE/UE/PE/SF,
ES and B, including IE combined with any of them. Preserve exactly the six
existing source classes: Valid+Normal, Zero+exact signed zero, and
Special+Infinity/QNaN/SNaN/Subnormal. The operation preserves IE, does not
quiet a current SNaN or add flags, and keeps its sign-only raw-byte behavior,
C1 clear and existing CC/TOP/tag/payload policy. Leave all other gates, decoder,
schema, JIT/native and WAIT/#MF behavior unchanged.

Cover the 6,144-case matrix (two opcodes × six classes × two signs × eight TOPs
× 16 prior CC states × IE clear/set), full-state comparisons and canaries.
Reject IE combined with each forbidden status bit, bad FCW, malformed tags,
missing state and fetch/truncation/STOP/budget/fault boundaries atomically.
Check repeated FCHS/FABS and FNSTSW/FNCLEX status visibility. Four existing-schema
m32 producer fixtures load SNaNs, verify FLD produces QNaN+IE before FCHS/FABS,
then store at the same width and observe preserved IE. Add seeded IE-only Normal
success and IE+SF atomic-reject fixtures. Keep ISSUE-019 open; no Mac mini, SSH,
native runtime or physical x86 checks belong to this substage.

#### S2-30bd. Admit historical sticky DE to bounded `FCHS/FABS`

Change only `bounded_x87_sign_st0_source`: retain FCW `0x037f`, exact six
accepted tag/class pairs, and sign-only operation policy; change the FSW mask
from `0x80fe` to `0x80fc`. Historical IE and DE are admitted independently and
together; ZE/OE/UE/PE/SF/ES/B remain rejected with any IE/DE combination. Keep
C1 clearing and all other status, tag, TOP, raw payload, decoder, operation
and native/JIT gates unchanged. Raw binary80 subnormal sources do not create
DE, and the sign operation does not create IE for SNaN.

Cover the 12,288 full-state matrix (two operations × six classes × two signs ×
eight TOPs × 16 prior CC patterns × four IE/DE states). Reject each forbidden
status bit combined with all four IE/DE states, plus bad FCW, wrong tag,
missing state and fetch/truncation/STOP/budget/fault boundaries with DE set.
Check repeated operations and FNSTSW/FNCLEX. Add m32/m64 FLD subnormal
compositions for both signs and operations, minimum/interior/largest subnormal,
and seeded IE clear/set. Assert Normal+Valid+DE after FLD, unchanged binary80
except sign after FCHS/FABS, exact same-width subnormal FST output and no new
UE/PE. Four schema-v2 m32 producer fixtures cover operation/sign pairs; seeded
DE-only success and DE+ZE / IE+DE+SF rejection fixtures exercise the CLI and
JIT denial. Keep ISSUE-019 open; defer Mac mini/SSH/native/physical checks to
the combined final substage.

#### S2-30bf. Admit historical sticky UE to bounded `FCHS/FABS`

Change only `bounded_x87_sign_st0_source`: change the FSW mask from `0x80dc`
to `0x80cc`. Keep FCW `0x037f`, the same six tag/class pairs and all raw
sign, C1, CC/TOP, metadata and retirement behavior. Admit all 16 IE/DE/UE/PE
combinations; reject ZE/OE/SF/ES/B atomically with every admitted history
combination. Decoder/classifier/arithmetic/JIT/native gates stay unchanged; raw
binary80 subnormal sources do not set DE, and sign operations add no exception.

Cover 49,152 full-state cases and every forbidden bit crossed with all 16
history combinations. Extend malformed-state, FCW and execution-boundary checks
with UE. Under FCW `0x037f`, non-popping FST m32/m64 producers for ±2^-150 and
±3×2^-151, and ±2^-1075 and ±3×2^-1076, must establish UE|PE for signed zero
or minimum-subnormal outputs with C1 clear/set. Check the Normal+Valid raw
binary80 source is unchanged by the store and then only sign-transformed by
FCHS/FABS; assert that UE|PE is present after FST before the sign operation.
Cover both signs, both sign opcodes, both widths and outcomes, and status
overview/clearing/repeat behavior. Exact schema-v2 CLI streams include
`D9 10 D9 E0 DF E0` and `DD 10 D9 E1 DF E0`, plus UE-only and seeded
IE|DE|UE|PE success, UE|OE and IE|DE|UE|PE|SF rejection, with JIT denial.
Keep ISSUE-019 open and defer Mac mini/SSH/native/physical checks to the combined
final substage.

#### S2-30bg. Admit historical sticky OE to bounded `FCHS/FABS`

Change only `bounded_x87_sign_st0_source` from FSW mask `0x80cc` to
`0x80c4`. Keep FCW `0x037f`, the same six accepted tag/class pairs, all
sign/C1/TOP/tag/raw/payload/FIP/RIP/retirement invariants, and no generated
flags. Admit every combination of IE/DE/OE/UE/PE (32 histories); reject ZE,
SF, ES and B combined with each history. Decoder, schema, classifier,
arithmetic, JIT and native gates remain unchanged.

Cover 98,304 full-state cases (two operations × six pairs × two signs × eight
TOP × 16 CC × 32 histories) and 3,072 atomic forbidden-status cases (four
forbidden bits × all histories across both operations, pairs and signs).
Extend malformed-state, exact-FCW, missing-state and execution-boundary
rejections with OE. Under FCW `0x037f`, produce real OE using non-popping FST
m32/m64 at the round-to-nearest overflow midpoint: significands
`0xffffff8000000000` at exponent 127 and `0xfffffffffffffc00` at exponent
1023. Test midpoint−1, midpoint, midpoint+1 and maximum significand, both
signs. Only midpoint and above produce signed infinity with OE|PE and C1=1;
midpoint−1 does not set OE. Preserve the Normal+Valid source, assert that the
following sign operation changes only source sign, clears C1 and preserves
produced flags and memory, then observe FNSTSW/FNCLEX. Include write/range
atomicity and seeded IE/DE/UE/PE. CLI coverage uses exact m32/m64 streams,
seeded OE-only success and ZE-based rejects; JIT remains denied. Keep ISSUE-019
open and defer Mac mini/SSH/native/physical checks to combined final acceptance.

#### S2-30bh. Admit historical sticky ZE to bounded `FCHS/FABS`

Change only `bounded_x87_sign_st0_source` from FSW mask `0x80c4` to
`0x80c0`. Keep FCW `0x037f`, the same six accepted tag/class pairs,
sign-only transforms, C1 clearing, TOP/tag/payload/metadata/retirement and
all other execution behavior. Accept all 64 IE/DE/ZE/OE/UE/PE histories;
SF/ES/B remain atomic rejects. Sign operations generate no exception flags.
ZE is explicitly seeded in state/fixtures; no divide-by-zero producer is
claimed in this slice. No arithmetic/decode/JIT/native change is in scope.

Cover the 196,608 full-state product (two operations × six tag/class pairs ×
two signs × eight TOP × 16 CC × 64 histories) and the 4,608 forbidden product
(three bits × 64 histories × two operations × six pairs × two signs). Include
bad FCW/tag/raw/missing-state and fetch/truncation/STOP/budget/fault atomicity
with ZE, repeated sign operations, FNSTSW/FNCLEX, and seeded CLI ZE-only/all-six
success plus ZE|SF/all-six|ES rejection fixtures and JIT denial. FRNDINT
compositions with seeded ZE cover ±1.5→±2 and ±1.25→±1 (FRNDINT adds PE and
sets/clears C1 respectively), exact ±1/±0 (no new PE), and SNaN→QNaN where
FRNDINT performs the quieting and adds IE before FCHS/FABS preserves ZE|IE.
Check complete state at each stage, observe with FNSTSW and clear with FNCLEX.
Do not run Mini/SSH/native/physical checks; those remain in combined final
acceptance under ISSUE-019.

#### S2-30bi. Admit historical SF only with IE to bounded `FCHS/FABS`

Change only `bounded_x87_sign_st0_source` to accept the existing 64 IE/DE/ZE/OE/UE/PE
histories with SF clear and the 32 corresponding SF|IE histories. The exact
gate is equivalent to `FSW & 0x8080 == 0 && (FSW & 0x0040 == 0 || FSW & 0x0001 != 0)`.
Keep FCW `0x037f`, all six source tag/class pairs, raw sign/payload transform,
C1 clear, TOP/tag/metadata/FIP/RIP/retirement and atomicity unchanged. Reject
SF without IE (including SF-only and DE|SF), ES and B atomically. No status
flags are generated and no current SNaN is quieted to synthesize IE.

Retain the 196,608 SF=0 full-state cases and add 98,304 SF|IE states for
294,912 total (96 valid histories). Reject 768 no-IE SF combinations and 6,144
ES/B combinations crossed with all 128 low-seven-bit states. Include malformed
tags/raw values, Empty, missing x87 state, bad FCW and fetch/truncation/STOP/
budget/fault boundaries with IE|SF, repeated operations, FNSTSW/FNCLEX,
full-state and memory canaries, and JIT denial. Seeded bare D9 E0 uses +1 and
`0x0241`→`0x0041`; bare D9 E1 uses -1 and the same status transition. Exact
`D9 FC D9 E0 DF E0` and `D9 FC D9 E1 DF E0` compositions seed ±1.5 with IE|SF;
FRNDINT adds PE/C1 and the sign operation clears C1, with FNSTSW observing
`0x0061` at TOP=0. IE|SF is explicitly seeded in the initial project snapshot;
there is no guest stack-fault producer in scope. FRNDINT is composition only,
not an SF producer. No native/physical evidence is claimed.

Use Intel SDM Vol.1 revision 081 §8.1.3.4 p.8-6 and §8.5.1.1 p.8-26
([official PDF](https://cdrdv2-public.intel.com/789574/253665-sdm-vol-1.pdf#page=208)).
Keep ISSUE-019 open; do not relax ES/B acceptance further before a separate
pending-exception contract and final physical x86 acceptance. Do not run
Mac mini/SSH/native/physical checks in this substage.

#### S2-30be. Admit historical sticky PE to bounded `FCHS/FABS`

Change only `bounded_x87_sign_st0_source`: change its FSW mask from `0x80fc`
to `0x80dc`. Keep exact FCW `0x037f`, the six admitted tag/class pairs, and
all raw sign, C1, CC/TOP/tag, metadata and retirement behavior. Admit exactly
the eight IE/DE/PE combinations; reject ZE/OE/UE/SF/ES/B, alone and combined
with any admitted historical flags. Keep decoder/schema/JIT/native gates
unchanged; JIT remains denied.

Expand the full-state matrix to 24,576 cases (two operations × six classes ×
two signs × eight TOP × 16 CC × eight IE/DE/PE states), and combine each
forbidden status bit with every historical state. Check malformed class/tag/
raw values, FCW, missing x87 state and fetch/truncation/STOP/budget/fault
atomicity under PE. FRNDINT under FCW `0x037f` must produce PE and the results
±1.25→±1, ±1.5→±2 and ±0.5→±0; verify C1 clear and set producer cases before
FCHS/FABS clears C1 and preserves PE. Also check canonical subnormal FRNDINT to
Zero-tagged signed-zero raw bytes through both FCHS and FABS, with the complete
FSW retaining DE|PE after each, then
observe and clear status with FNSTSW/FNCLEX. Add two exact CLI
producer fixtures (`D9 FC D9 E0 DF E0` and `D9 FC D9 E1 DF E0`) seeded with
±1.5, plus PE-only and IE|DE|PE success and PE|UE and IE|DE|PE|SF atomic reject
fixtures. Keep ISSUE-019 open and defer Mac mini/SSH/native/physical checks.

#### S2-30az. Admit canonical binary80 QuietNaN to bounded `FST/FSTP ST(i)`

Extend only `bounded_x87_fst_sti_source`: retain Valid-tag canonical Normal,
exact Zero-tagged signed zero, and Special-tagged canonical infinity behavior.
Additionally admit `(Special, QuietNaN)` for canonical binary80 E=`0x7fff`,
J=1, Q=1; accept either sign and every low-62-bit payload, including zero.
Preserve FCW `0x037f`, `FSW & 0x80ff == 0`, all ten source bytes/tag, and the
current C1 policy. Bare `DD D0+i` copies to physical `(old_TOP+i)&7` without
changing TOP. Bare `DD D8+i` copies first, empties old physical TOP, then
advances TOP modulo eight. Thus `i=0` leaves copied bytes under an Empty tag,
and `i=1` places the copy at new ST(0). Do not change decoding, classifier,
schema, other gates or JIT/native admission. Reject SNaN, pseudo-NaNs,
pseudo-infinity, subnormal, pseudo-denormal, unnormal, malformed values and
wrong tag/class pairs atomically; add no exception flags.

Cover both opcodes × both signs × all TOP × all indices × all 16 prior
condition-code states × payloads `{0, 1, 1<<61, (1<<62)-1}` with full-state
expectations, destination tag/payload canaries, `i=0`/`i=1`, TOP wrap and
copy-before-pop checks. Include invalid neighboring classes/tags/environment,
missing state and fetch/truncation/prefix/REX/WAIT/STOP/budget atomicity.
Four existing-schema project fixtures use `d900ddd1dfe0` and
`d900ddd9dfe0` to load binary32 QNaNs of both signs, store to ST(1), and observe
status with FNSTSW. Direct interpreter compositions cover m32 and m64, both
signs and both store forms. Local checks do not establish physical x87 behavior;
keep ISSUE-019 open and defer Mac mini/SSH/native/physical acceptance to the
combined final substage.

#### S2-30ba. Admit canonical binary80 SignalingNaN to bounded `FST/FSTP ST(i)`

Extend only `bounded_x87_fst_sti_source`: retain Valid-tag canonical Normal,
exact Zero-tagged signed zero, Special-tagged canonical infinity and QNaN.
Additionally admit only `(Special, SignalingNaN)` with binary80 E=`0x7fff`,
J=1, Q=0 and a nonzero lower-62-bit payload, either sign. The J=1/Q=0
zero-payload encoding remains the canonical Infinity path. Preserve FCW `0x037f`,
`FSW & 0x80ff == 0`, all ten source bytes/tag and existing C1 behavior. Bare
`DD D0+i` copies to physical `(old_TOP+i)&7` without changing TOP; bare
`DD D8+i` copies first, empties old physical TOP and advances TOP modulo eight.
For `i=0` the copied bytes remain under Empty; for `i=1` the destination is
new ST(0). Add no IE/PE and do not change classifier, wire schema, decode,
other gates, JIT or native admission.

Canonical Infinity remains admitted through its existing path. Reject only
pseudo-infinity, pseudo-NaNs, subnormal, pseudo-denormal, unnormal, malformed
values, wrong tag/class pairs and bad environments atomically. Cover both
operations × both signs × all TOP × all indices × 16 condition-code states ×
payloads `{1, 1<<61, 0x0123456789abcdef, (1<<62)-1}` (16,384 cases), with full
state, physical canaries, destination tag/payload sentinels, alias/pop/wrap
checks and execution boundaries. Add raw FLD m80fp→FST/FSTP ST(1)→FNSTSW unit
compositions for both signs/forms, asserting Q=0 and no IE. Four seeded bare
schema-v2 fixtures run with budget one and stop RIP=start+2; the wrong-tag
fixture verifies no retirement, and successful fixtures remain denied by JIT. The
wire schema stays unchanged; `od-testkit` admits only bare `DD D0+i`/`DD D8+i`
streams for these seeded single-instruction cases. Do not use m32/m64 SNaN loads
because those quiet the value and raise IE. Keep
ISSUE-019 open; no Mac mini, SSH, native runtime or physical x86 checks belong
in this substage.


#### S2-30bb. Admit canonical binary80 subnormal to bounded `FST/FSTP ST(i)`

Extend only `bounded_x87_fst_sti_source`. Retain Valid-tag canonical Normal,
Zero-tagged exact signed zero, and Special-tagged canonical infinity, QNaN and
SNaN. Additionally admit only `(Special, Subnormal)` canonical binary80
E=0, J=0 with a nonzero 63-bit fraction, either sign. Preserve FCW `0x037f`,
`FSW & 0x80ff == 0`, all ten source bytes, Special tag and the existing C1
policy; add no DE/UE/PE/IE. Bare `DD D0+i` copies to physical `(old_TOP+i)&7`
without changing TOP. Bare `DD D8+i` copies first, empties old physical TOP,
then advances TOP modulo eight; alias `i=0` retains bytes under Empty and `i=1`
becomes new ST(0). Do not alter classifier, decoder, wire schema, other gates,
JIT or native admission. Reject zero under Special, wrong-tag subnormals,
pseudo-denormal, unnormal and pseudo-special values atomically.

Cover both opcodes × both signs × all TOP/index pairs × 16 prior condition-code
states × fractions `{1, 1<<62, 0x0123456789abcdef, (1<<63)-1}` (16,384 cases),
with full-state expectations, destination canaries/tags and alias/pop checks.
Add malformed neighbors, wrong tags, bad FCW/FSW, missing state and fetch,
truncation, prefix, WAIT, STOP, budget and fetch-fault atomicity. A raw FLD m80fp
→ FST/FSTP ST(1) → FNSTSW unit composition checks both signs, raw bytes and no
DE. Four existing-schema seeded bare fixtures run both forms/signs with
budget one and stop RIP=start+2; a wrong-tag fixture rejects atomically and
successful fixtures remain denied by JIT. Local checks do not establish
physical x86 acceptance; keep ISSUE-019 open and defer host/physical validation
to the combined final substage.

#### S2-30z. Bare bounded `FABS` (sign-bit-clear local slice)

Accept exactly bare `D9 E1` with an existing `Some(X87State)`, using the same
canonical-Normal ST(0) source gate as S2-30y: FCW `0x037f`, `FSW & 0x80ff == 0`,
Valid tag at old physical TOP and canonical Normal binary80 bytes. Snapshot all
ten source bytes and clear only `source[9] & 0x7f` in old physical TOP. Clear
C1 while preserving TOP, tags and every other state field; retaining C0/C2/C3
is the deterministic local snapshot policy. Set FIP to instruction-start RIP,
advance RIP by two and retire once.

Missing/bad state, environment, tag or class; every other D9 second byte,
prefix/REX/WAIT, truncation, STOP and exhausted budget reject without state
commit. Zero, subnormal and special values, memory, CR0/#NM, exception delivery,
native x87 and FPU feature claims remain outside this slice.

Coverage enumerates all 256 D9 second bytes, accepting E0 as FCHS and E1 as
FABS, and checks prefix/REX/WAIT/truncation. Literal interpreter vectors cover
all TOP values, positive and negative minimum/maximum/full-significand Normal
values, C1 clear/set, C0/C2/C3 retention and idempotent repeated FABS. Full-state
rejections cover invalid/missing environment, tag/class, fetch, STOP and budget.
The schema/CLI includes a literal success fixture and a real Empty-tag rejection
with no stop RIP and budget one. Native admission explicitly denies D9 E1.

Normative source: Intel SDM revision 093, Vol.2A, “FABS—Absolute Value,” p.3-309
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=427)).

#### S2-30aa. Bare bounded `FTST` (canonical-Normal comparison slice)

Accept exactly bare `D9 E4` with existing `Some(X87State)`, FCW `0x037f`,
`FSW & 0x80ff == 0`, Valid old physical TOP tag and canonical-Normal ST(0)
from `bounded_x87_normal_st0_source`. Positive Normal sets C3/C2/C0 to `000`;
negative Normal sets them to `001`. Clear C1 in both cases and replace the full
condition-code field using `(old_fsw & !0x4700) | (negative ? 0x0100 : 0)`.
Preserve TOP, all tags, physical register bytes and every other state field;
record instruction-start RIP in FIP, advance RIP by two and retire once.

Missing state, invalid environment/tag/class, every other D9 byte, prefixes,
REX, WAIT, truncation, fetch failure, STOP and exhausted budget must leave the
full state unchanged. Signed zero, special/subnormal values, exceptions, stack
faults, CR0/#NM, memory forms, native execution and FPU feature claims are
excluded. Exhaustive D9 decoding accepts exactly E0→FCHS, E1→FABS and E4→FTST.
Vectors cover all TOP positions, signs, Normal significand/exponent endpoints,
all initial condition-code combinations, sentinels, FTST→FNSTSW AX, atomic
rejections and CLI/schema fixtures. Native admission explicitly denies FTST.

Normative source: Intel SDM revision 093, Vol.2A, “FTST—Test,” pp.3-400–3-401
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

### S2-31. x87 conversions, rounding и exceptions

1. Добавить integer/float conversions, precision/rounding control и нужные control/status operations.
2. Реализовать masked/unmasked exceptions, pending/deferred delivery и WAIT/no-wait различия по источнику, без host signal shortcut.
3. Проверить invalid, denormal operand, divide-by-zero, overflow, underflow, precision, signed zero, NaN и крайние integer conversions.

**Готово:** value, status, stack и exception state совпадают в оговорённом профиле; неизвестные поля не скрыты общей маской.

#### S2-31a. Bare bounded `FRNDINT` (`D9 FC`), partial slice

Implement only register-only FRNDINT for a Valid physical TOP holding a raw
canonical Normal whose unbiased exponent is 0..62. Accept FCW exactly
`037f/077f/0b7f/0f7f` (PC64, all masks, each RC) and require
`FSW & 0x80df == 0`; sticky PE may be present. Use integer quotient/remainder
rounding and the existing normal packer, including the ±2^63 boundary without
host integer conversion. Commit only old physical TOP's payload, PE/C1, FIP,
RIP and one retirement; preserve tags, TOP and unrelated state. C0/C2/C3 are
retained by local policy. Reject unsupported values, environment, prefixes,
and execution boundaries atomically. Native admission remains closed. This
does not complete S2-31 or define general x87 exceptions.

#### S2-31dc. Admit signed zero to bounded `FRNDINT` (`D9 FC`)

Extend only the exact bare register-only S2-31a gate to accept the physical TOP
when it has `X87Tag::Zero` and canonical raw signed zero: significand bytes are
all zero and sign/exponent is exactly `0000` or `8000`. Preserve its exact ten
bytes, Zero tag and TOP; clear C1, add no exception flags, and retain PE and
C0/C2/C3. Keep the FCW whitelist and `FSW & 0x80df` unchanged. All existing
pointer, GPR/RFLAGS, retirement, and atomic fetch/truncation/STOP/budget
behavior remains unchanged. Mismatched tags/payloads and all other source
classes remain Unsupported. The 256-case matrix spans sign, FCW, TOP, PE and
C1; literal CLI coverage composes negative zero with FNSTSW AX. FNCLEX,
prefix/decode, and native/JIT denial remain covered by their existing gates.
Physical acceptance remains with the combined final S2-31 substage; this does
not complete S2-31 or expand x87 exception delivery.

#### S2-31dd. Admit exact-integer Normal values to bounded `FRNDINT` (`D9 FC`)

Extend the existing bare S2-31a register-only gate to canonical Normal values
with unbiased exponent `63..=16383` (biased exponent `0x403e..=0x7ffe`). With
the Valid physical TOP tag, J=1, either sign, and any fraction, the represented
value is already an integer: preserve its ten raw bytes, all tags and TOP;
clear C1, add no flags, and preserve PE and C0/C2/C3. Return this identity
before computing `63-e` or invoking the integer rounding/packing path. Keep the
FCW whitelist and `FSW & 0x80df` mask unchanged. Preserve FDP/FOP and unrelated
state, set FIP to old RIP, advance RIP by two and retire once. Reject malformed
or tag-mismatched values, other classes and invalid controls/status atomically.
The boundary coverage compares fractional exponent 62 (normal rounding and PE)
with exponent 63 (exact identity without new PE); matrix coverage spans literal
exponents 63, 64, 127, 1023, 16383; four significand patterns; both signs; all
TOPs/FCWs; and incoming PE/C1. Native/JIT denial and physical acceptance
remain unchanged; this does not complete S2-31 or define general exceptions.

#### S2-31de. Admit canonical signed infinity to bounded `FRNDINT` (`D9 FC`)

Extend only the exact bare S2-31a gate to accept physical TOP tagged Special
when the raw ten-byte value is canonical signed binary80 infinity: significand
`0x8000000000000000`, exponent/sign field exactly `0x7fff` or `0xffff`. Preserve
all ten bytes, Special tag, TOP, PE and C0/C2/C3; clear C1 and add no flags.
Keep the four-FCW whitelist and `FSW & 0x80df == 0` unchanged. Preserve FDP/FOP
and unrelated state, set FIP to the starting RIP, advance RIP by two, and
retire once without data or stack access.

Reject pseudo-infinity (J=0), NaNs and any nonzero fraction, wrong exponent,
wrong tag, invalid controls/status, prefixes and fetch/truncation/STOP/budget
boundaries atomically. Preserve existing zero and Normal behavior. A 256-case
matrix spans sign×FCW×TOP×PE×C1 with independent raw/status expectations. Add a
literal CLI composition that loads a supported signed infinity with `FLD
m80fp`, executes `FRNDINT`, observes FSW with `FNSTSW AX`, and checks payload,
tag/TOP, pointers and state. JIT/native admission remains closed. Physical x86
acceptance stays with the combined final S2-31 substage; this does not define
general x87 exception behavior.

Normative source: Intel SDM revision 093, Vol.1 §8.2.2/Table 8-3 (binary80
infinity encoding) and Vol.2A “FRNDINT—Round to Integer,” p.3-372, plus
“FLD—Load Floating-Point Value” for the existing m80 load form.

#### S2-31df. Admit canonical quiet NaN to bounded `FRNDINT` (`D9 FC`)

Extend only the bare register-only `D9 FC` gate to admit a Special-tagged
canonical binary80 QNaN: exponent `0x7fff`, J=1, bit 62=1, either sign, and any
lower 62 payload bits (including zero). Preserve all ten raw bytes, Special
tag, TOP, incoming PE and C0/C2/C3; clear C1 and add no exception flags. Keep
the exact four-FCW whitelist and `FSW & 0x80df == 0` gate. Preserve FDP/FOP and
unrelated state, set FIP to old RIP, advance RIP by two, and retire once
without memory or stack access.

Quiet-bit-clear SNaNs, J=0 pseudo-NaNs, pseudo-infinity, wrong tags, subnormals,
pseudo-denormals and other out-of-domain classes remain Unsupported; the
existing exponent 0..62 Normal rounding slice is unchanged.
The 1,024-case matrix covers four literal significands × both signs × four FCWs
× all TOP values × incoming PE × both old C1 values with independent raw and
status expectations. Add invalid source/tag/control/status and fetch,
truncation, STOP and budget atomicity checks, plus a literal `FLD m80fp` →
`FRNDINT` → `FNSTSW AX` fixture. JIT/native admission remains closed. Physical
x86 acceptance and general exception behavior remain deferred under ISSUE-019.

Normative sources: Intel SDM revision 093, Vol.1 §8.2.2/Table 8-3 and Vol.2A
“FLD—Load Floating-Point Value” and “FRNDINT—Round to Integer,” p.3-372.

#### S2-31dg. Quiet canonical signaling NaN in bounded `FRNDINT` (`D9 FC`)

Extend only bare `D9 FC` to admit a Special-tagged canonical binary80 SNaN:
exponent `0x7fff`, J=1, bit 62=0, and a nonzero lower 62-bit payload, either
sign. Quiet ST(0) by setting only bit 62; preserve sign, exponent, J, payload,
Special tag, TOP and all unrelated state. Add sticky IE, clear C1, add no PE
or other flags, and preserve prior PE and C0/C2/C3. Keep the FCW whitelist and
`FSW & 0x80df == 0` gate, including rejection of historical IE. Set FIP to old
RIP, advance RIP by two and retire once without memory or stack access.

Canonical infinity (zero lower payload) and QNaNs retain their existing
identity behavior; pseudo-NaNs, subnormals, pseudo-denormals, unnormals,
malformed encodings, wrong tags, invalid controls or status and
execution-boundary failures remain Unsupported. Cover the four
literal significands × both signs × four FCWs × eight TOPs × incoming PE × old
C1 matrix (1,024 cases) with independent quieted payload/status expectations,
plus boundaries, pre-FNCLEX re-entry rejection, `FLD m80fp`→`FRNDINT`→`FNSTSW`
→`FNCLEX`→`FRNDINT` composition and a literal CLI fixture. JIT/native
admission remains closed. Physical x86 acceptance remains with combined final
S2-31 acceptance under ISSUE-019.

Normative sources: Intel SDM revision 093, Vol.1 §8.2.2/Table 8-3 and Vol.2A
“FLD—Load Floating-Point Value” and “FRNDINT—Round to Integer,” p.3-372.

#### S2-31dh. Admit historical sticky IE to bounded `FRNDINT` (`D9 FC`)

Change only the existing bare register-only FRNDINT status gate from
`FSW & 0x80df == 0` to `FSW & 0x80de == 0`. Historical IE is admitted and
preserved independently of the current source: exact/identity sources add no
new exception, inexact Normal rounding still sets PE from the current
remainder, and a current SNaN still quiets and sets IE. Keep rejection of DE,
ZE, OE, UE, SF, ES, and B; the four-FCW whitelist, every source/tag domain,
rounding, quieting, metadata, atomic execution, and all other gates remain
unchanged.

Cover signed zero, exact and inexact Normal, large integral Normal, infinity,
QNaN and SNaN over sign, FCW, TOP, all IE/PE sticky combinations and prior C1.
Include IE with every forbidden status as an atomic reject, consecutive
SNaN→FRNDINT→FRNDINT execution without FNCLEX and the extended literal
FLD→FRNDINT→FRNDINT→FNSTSW→FNCLEX→FRNDINT CLI composition,
and the existing fetch/truncation/STOP/budget/no-memory and JIT/native denial
controls. This gate adjustment does not add a producer of IE and does not
complete S2-31 or define general x87 exception delivery.

#### S2-31dj. Admit historical sticky DE to bounded `FRNDINT` (`D9 FC`)

Change only the bare register-only FRNDINT status gate from
`FSW & 0x80de == 0` to `FSW & 0x80dc == 0`. Historical DE is admitted and
preserved along with IE and PE. Keep the four-FCW whitelist and current
rounding/exception behavior unchanged. Continue rejecting ZE, OE, UE, SF, ES
and B, including when combined with admitted IE/DE/PE; do not widen source or
tag domains, alter atomicity, or open JIT/native admission.

Cover the supported source classes and Normal exponent boundaries across
sign, RC, TOP, sticky IE/DE/PE combinations and prior C1. Literal CLI
compositions `FLD m32fp/m64fp subnormal → FRNDINT → FNSTSW AX` verify that
FLD's canonical-Normal result retains DE through rounding. Keep the all-masked
FCW gate unchanged; also cover seeded raw `D9 FC DF E0` with IE/DE/PE already
sticky under the all-masked FCW. Physical x86 and Mac mini checks belong to the single
combined final S2-31 acceptance substage under ISSUE-019.

#### S2-31dk. Admit historical sticky UE to bounded `FRNDINT` (`D9 FC`)

Change only the bounded bare register-only FRNDINT status gate from
`FSW & 0x80dc == 0` to `FSW & 0x80cc == 0`, admitting historical UE alongside
the existing IE/DE/PE set. Preserve UE. Do not generate PE solely because UE is
already set: exact and identity operations retain PE as-is, current inexact
rounding adds PE, and a current canonical SNaN adds IE without adding PE. Keep
the exact four FCWs, all source/tag domains, rounding, C1 and metadata policy;
ZE/OE/SF/ES/B and unsupported classes remain atomic rejects. Native/JIT remain
closed.

Add an independent interpreter matrix over incoming IE/DE/PE combinations,
prior C1, all four RCs, every TOP, and exact Normal, inexact Normal and
canonical SNaN sources. Include literal exact UE-only, UE+inexact PE, and
UE+SNaN IE cases, plus forbidden-status rejects with UE present. Add literal
seeded `D9 FC DF E0` CLI coverage and a composition
`FST m32fp underflow → FRNDINT → FNSTSW AX` proving the store-created UE flows
through FRNDINT. Keep JIT/native closed. Physical x86 and Mac mini checks
remain part of the combined final S2-31 acceptance substage under ISSUE-019.

#### S2-31dl. Admit historical sticky OE to bounded `FRNDINT` (`D9 FC`)

Change only the bounded bare register-only FRNDINT status gate from
`FSW & 0x80cc == 0` to `FSW & 0x80c4 == 0`, admitting and preserving historical
OE alongside IE/DE/UE/PE. OE itself does not add PE; exact and identity sources
retain existing PE, current inexact Normal rounding adds PE, and current SNaN
quieting adds IE without PE. Preserve the four FCWs, source/tag domains,
rounding, C1 and metadata. Continue rejecting ZE, SF, ES and B atomically.
Native/JIT admission remains closed.

Add an independent matrix over IE/DE/UE/PE, C1, four RCs and all TOPs for exact
and fractional Normal plus canonical SNaN. Include identity smoke for signed
zero, large integral Normal, infinity and QNaN, literal seeded
`D9 FC DF E0`, and forbidden OE+ZE/SF/ES/B rejects. Add the exact CLI pattern
`FST m32 overflow → FRNDINT → FNSTSW AX` starting from FSW=0: verify the store
producer's OE+PE+C1 and final sticky OE/PE with C1 cleared. Open only this
composition in the existing fixture schema. Physical x86/Mac mini checks
remain in the combined final S2-31 acceptance substage under ISSUE-019.

#### S2-31dm. Admit historical sticky ZE to bounded `FRNDINT` (`D9 FC`)

Change only the existing bounded register-only FRNDINT status gate from
`FSW & 0x80c4 == 0` to `FSW & 0x80c0 == 0`, admitting prior ZE while
continuing to reject SF, ES and B. ZE is preserved as historical sticky state;
FRNDINT itself does not generate ZE. Exact/identity sources add no PE,
fractional Normal inputs add PE, and current canonical SNaN adds IE without PE.
Sticky IE/DE/OE/UE/PE remain independently preserved. Keep the four-FCW
whitelist, source/tag domains, rounding, C1 and metadata unchanged; do not add
an FDIV-zero producer or widen its domain, and do not change JIT/native
admission.

Cross seeded ZE with all combinations of sticky IE/DE/OE/UE/PE over the
existing class/sign/RC/TOP/prior-C1 matrix. Keep SF/ES/B atomic rejects with
ZE and sticky flags present. Add an exact seeded `D9 FC DF E0` CLI fixture
whose FNSTSW expectation includes ZE; provenance must state that ZE is
explicitly seeded and no guest producer is present. Physical x86 and Mac mini
acceptance remain part of the combined final S2-31 substage under ISSUE-019.

#### S2-31dn. Admit historical SF together with IE to bounded `FRNDINT` (`D9 FC`)

Retain the exact FCW whitelist `0x037f/0x077f/0x0b7f/0x0f7f`. Use the base
status gate `FSW & 0x8080 == 0`, with the additional reject
`(FSW & SF != 0 && FSW & IE == 0)`. Thus historical SF is admitted only when
IE is also set; SF-only, ES and B reject atomically. Preserve both historical
SF and IE, along with DE/ZE/OE/UE/PE independently. FRNDINT continues to derive
C1 from the current result and never carries prior C1 forward. Empty, mismatched
and raw-malformed sources remain rejected; existing source/tag domains and
JIT/native denial stay unchanged.

Add an independent 32,768-case matrix: SF+IE crossed with all 32 DE/ZE/OE/UE/PE
combinations, eight source classes, both signs, four FCWs, all eight
TOP values and both prior C1 values. Include SF-only rejects for an ordinary
source and current SNaN, ES/B rejects, and retain existing empty/mismatch/raw
malformed rejection coverage. The literal `D9 FC DF E0` fixture starts with
source `1.0`, FCW `0x037f`, and initial FSW `0x0241`; it observes AX=`0x0041`
after C1 is recomputed. Fixture provenance must say the IE+SF state is seeded
in the initial snapshot, not produced by the guest, and is not native evidence.
The Intel SDM rev.093 basis is Vol.1 §8.1.3.4 p.8-6 and §8.5.1.1 p.8-26,
plus Vol.2A FRNDINT p.3-372. Re-entry/FNCLEX may use the existing SNaN flow.
No ES/B, pending/#MF/WAIT semantics are changed. Physical x86 acceptance remains
one final S2-31 substage under ISSUE-019.

#### S2-31di. Admit small canonical-Normal fractions to bounded `FRNDINT` (`D9 FC`)

Extend the bare register-only gate for a Valid canonical-Normal physical TOP
whose unbiased exponent is `-16382..=-1`, either sign. Keep FCW
`037f/077f/0b7f/0f7f` and `FSW & 0x80de == 0`. Use comparison-based rounding:
nearest-even selects magnitude one only for `e=-1` and significand greater
than `0x8000000000000000`; directed modes select signed magnitude one only
when their direction moves that sign away from zero. Otherwise return signed
zero. Every input adds sticky PE, and C1 is set exactly for magnitude one.
Encode zero as Zero-tagged signed binary80 zero and ±1 as canonical Valid
binary80. Do not form a denominator or perform an oversized shift.

Cover half and neighbors, near-one values, `e=-2,-63,-64,-65,-16382`, signs,
all RCs/TOPs, IE/PE sticky combinations, prior C1, Valid→Zero and Valid→Valid
tag transitions, repeated FRNDINT, status/payload preservation, atomic decode
and execution boundaries, no data memory, FNSTSW/FNCLEX, and an exact literal
CLI fixture. Keep JIT/native admission closed. Subnormal, pseudo-denormal,
malformed, mismatched-tag and unmasked cases remain unsupported; physical
acceptance belongs to the combined final S2-31 substage under ISSUE-019.

#### S2-31b. Bare bounded `FISTP m32int [RAX]` (`DB 18`), partial slice

Implement only bare `DB 18` in Long64 with a canonical 4-byte RAX destination
and an existing x87 snapshot. Preflight the old physical TOP tag and canonical
Normal source; accept unbiased exponent 0..31, the four existing all-masked
PC64 control words, and `FSW & 0x80df == 0`. Use integer quotient/remainder
rounding and accept only rounded values in signed i32 range. Out-of-domain and
overflow inputs are Unsupported before data access. Validate the entire
canonical48 destination before exactly one atomic Dword write; only after
success empty old TOP, advance TOP, update sticky PE/C1, FIP/FDP/RIP and retire
once. Preserve every x87 payload and unrelated state. Test exact `DB 18` and
`DB 18 DF E0` fixtures, overflow and store-fault atomicity, and the one-write,
no-read memory contract. Native admission remains closed. This is a local
partial slice and does not complete S2-31 or model x87 exception delivery.

#### S2-31c. Bare bounded `FIST m32int [RAX]` (`DB 10`), partial slice

Implement only bare `DB 10` in Long64 with canonical Dword destination `[RAX]`
and an existing x87 snapshot. Reuse S2-31b's canonical-Normal physical ST(0),
exponent 0..31, exact four FCW values, FSW gate, integer RC rounding and
post-rounded signed-i32 bounds. Validate the complete canonical48 destination
before one atomic Dword write. On success update sticky PE/C1, FIP/FDP/RIP and
retire once; preserve TOP, every tag, all payloads and unrelated state. Failure
before or during the write leaves architectural state unchanged. Accept only
literal `DB 10` and exact `DB 10 DF E0` fixtures, reject prefixes and nearby
forms, and explicitly deny native admission. This local slice does not complete
S2-31 or model general x87 exception delivery.

#### S2-31d. Bare bounded `FISTP m16int [RAX]` (`DF 18`), partial slice

Implement only bare `DF 18` in Long64 with a canonical Word destination
`[RAX]` and an existing x87 snapshot. Reuse S2-31b/c's canonical-Normal old
physical TOP, four exact all-masked PC64 FCW values, `FSW & 0x80df == 0`, and
integer quotient/remainder RC rounding, with exponent 0..15 and post-rounded
signed-i16 bounds. Validate the full canonical48 two-byte destination before
exactly one atomic Word write. Only after success empty old TOP, advance TOP,
update sticky PE/C1, record FIP/FDP, advance RIP and retire once; preserve raw
x87 payloads and unrelated state. Add literal fixtures for `DF 18`, exact
`DF 18 DF E0`, RC/sign/tie and signed-boundary cases, plus overflow, invalid
tag/status/configuration and no-commit rejection. Decoder/schema coverage must
reject prefixes and nearby forms, and native admission remains closed. This
local slice does not model general x87 exception delivery or complete S2-31.

#### S2-31e. Bare bounded `FIST m16int [RAX]` (`DF 10`), partial slice

Implement only bare `DF 10` in Long64 with a canonical Word destination
`[RAX]` and an existing x87 snapshot. Mirror S2-31d's canonical-Normal old
physical TOP, exponent 0..15, four exact all-masked PC64 FCW values,
`FSW & 0x80df == 0`, integer quotient/remainder RC rounding and post-rounded
signed-i16 bounds. Validate the complete canonical48 destination before one
atomic Word store with no data read. Only after that store succeeds, update
sticky PE/C1, FIP/FDP, RIP and retirement once. Preserve TOP, every physical
tag, all eight raw payloads and unrelated state. Failures before or during the
store preserve architectural state and memory. Add literal `DF 10` and exact
`DF 10 DF E0` (FNSTSW AX) fixtures for no-pop state, rounding/boundaries,
overflow and invalid tag/status/control/exponent cases; reject prefixes and
nearby forms, and explicitly deny native admission. This local slice does not
model general x87 exception delivery or complete S2-31.

#### S2-31f. Bare bounded `FISTP m64int [RAX]` (`DF 38`), partial slice

Implement only bare `DF 38` in Long64 with an eight-byte `[RAX]` destination
and existing x87 state. Require canonical-Normal old physical TOP, unbiased
exponent 0..63, FCW exactly `0x037f`, `0x077f`, `0x0b7f`, or `0x0f7f`, and
`FSW & 0x80df == 0` (sticky PE is allowed). Reuse integer quotient/remainder
rounding with `shift = 63 - exponent`; exponent 63 uses shift zero, divisor
one and zero remainder. Compare the rounded `u128` magnitude with the signed
i64 limit before narrowing, then encode the accepted result as unsigned
64-bit two's complement. Rounded overflow is Unsupported before data access;
this bounded profile does not write the ISA indefinite value or deliver x87
exceptions. Validate the complete canonical48 eight-byte range, perform one
atomic Qword write with no data read, then empty old TOP, advance TOP, update
sticky PE/C1, record FIP/FDP, advance RIP by two and retire once. Preserve all
other x87 payloads/tags and unrelated state. Add independent literal binary80
ties, signed boundaries, `2^63 - 0.5` RC cases, exponent 64 rejection,
canaries, canonical/access-fault and TOP-wrap coverage. Admit exact `DF 38`
and optionally `DF 38 DF E0`; reject prefixes/neighbors and deny native x87
admission. Exception delivery remains unmodeled; this does not complete
S2-31.

#### S2-31g. Bare bounded `FNSTCW m2byte [RAX]` (`D9 38`), partial slice

Implement only exact bare `D9 38` in Long64 with an existing x87 snapshot and
the fixed Word destination `[RAX]`. Store the raw FCW little-endian through one
atomic Word write; do not inspect ST(0), tags, TOP or status, and impose no FCW
or FSW gates. Validate the full canonical48 two-byte destination before the
write. On success, change only RIP and retire once; preserve the complete x87
snapshot, GPRs and RFLAGS. Address/backend/fetch/stop/budget failures must not
commit CPU state, and native execution stays closed. Add exact `D9 38` and
`D9 38 DF E0` schema forms, literal cases for all four standard FCWs and
arbitrary FCW/status with pending bits, empty stack and payload/pointer
sentinels, plus canaries, canonical boundaries and a second-byte store-failure
check. This slice does not complete S2-31.

**Результат:** реализовано и проверено на workspace fixtures/interpreter tests;
native execution remains explicitly denied. See the S2-31g report and CPU
contract for validation and scope.

#### S2-31h. Bare bounded `FNSTSW m2byte [RAX]` (`DD 38`), partial slice

Implement only exact bare `DD 38` in Long64 with an existing x87 snapshot and
the fixed Word destination `[RAX]`. Store the raw pre-operation FSW little-
endian through one atomic GuestMemory Word write; do not read destination data,
inspect stack tags/TOP, or gate on FCW/FSW values. Validate the full canonical48
two-byte destination before the store. On success, change only RIP and retire
once; preserve GPRs, RFLAGS and the entire x87 snapshot, including C0-C3 by
local policy without a native-equivalence claim. Address/store/fetch/missing-
state/stop/budget failures must not commit CPU state; propagate memory errors.
Admit exact `DD 38` and optionally exact `DD 38 DF E0` composition; reject
prefixes, waiting `9B DD 38`, and neighboring DD forms. Native execution stays
closed. Add literal FSW `0000`, mixed and `ffff` fixtures, arbitrary FCW/status,
Empty tags and sentinels, canaries, canonical and access failures, execution
boundaries, composition, and explicit native denial. This local slice does
not deliver `#MF` or `#NM`, model CR0, MMU, MMIO, or complete S2-31.

**Результат:** implemented and checked with workspace fixtures/interpreter
tests; native execution remains explicitly denied. See the S2-31h report and
CPU contract for validation and scope.

#### S2-31i. Bare bounded `FLDCW m2byte [RAX]` (`D9 28`), partial slice

Implement only exact bare `D9 28` in Long64 with an existing x87 snapshot and
fixed Word source `[RAX]`. Before data access require old FCW exactly
`037f/077f/0b7f/0f7f` and `FSW & 0x80df == 0`; sticky PE and condition codes
are permitted. Validate the complete canonical48 source range, issue one
GuestMemory Word read, then accept only one of the same four FCWs. Invalid old
state rejects before access; invalid loaded FCW rejects after that read and
before commit. Success changes only FCW and RIP by two and retires once,
preserving FSW (including undefined C0-C3 by local deterministic policy),
TOP/tags/payloads, FIP/FDP/FOP, GPRs and RFLAGS. Test all 16 old/new FCW pairs,
all four loaded RC modes through FRNDINT, `D9 28 D9 FC DF E0`, bad old/new
states, missing state, invalid address/read and execution boundaries. Reject
prefixes, WAIT and other D9 /5 addressing; keep native admission closed. This
does not implement pending/unmasked exception delivery, `#MF`/`#NM`, MMU, MMIO,
or complete S2-31.

**Результат:** implemented and checked with workspace fixtures/interpreter
tests; native execution remains explicitly denied. See the S2-31i report and
CPU contract for validation and scope.

#### S2-31j. Bare bounded `FSTP m32fp [RAX]` (`D9 18`), partial slice

Implement only exact bare `D9 18` in Long64 with an existing x87 snapshot and
fixed Dword destination `[RAX]`. Require a Valid old physical TOP with a
canonical Normal raw binary80 source whose unbiased exponent is -126..126;
accept only FCW `037f/077f/0b7f/0f7f` and `FSW & 0x80df == 0`. Convert using
integer bits (`q=significand>>40`, remainder over `2^40`) and the shared exact
quotient-rounding helper. Pack only a finite normal binary32 result; reject
subnormal/overflow exponents, including a rounding carry from exponent 126 to
127, before data access. No host floating-point conversion is permitted.

Validate the full canonical48 Dword range and issue exactly one atomic
GuestMemory write with no data read. Only after success empty the old physical
TOP tag, advance TOP, update sticky PE and C1 (C1 means rounded magnitude up;
exact results clear it under local policy), record FIP/FDP, advance RIP by two
and retire once. Preserve all payloads, other tags, FCW/FOP and unrelated
state; C0/C2/C3 are retained by deterministic local policy. Check literal
`D9 18` and exact `D9 18 DF E0` fixtures, ties, signed directed rounding,
normal exponent endpoints, carry/range rejects, every TOP, canaries,
atomic memory faults, canonical range, and fetch/truncation/STOP/budget
boundaries. Reject prefixes, WAIT and neighboring D9 forms; native admission
remains closed. This partial slice does not claim hardware exception priority
or general x87 exception delivery and does not complete S2-31.

**Результат:** implemented and checked with local fixtures/interpreter tests;
native execution remains explicitly denied. See the S2-31j report and CPU
contract for validation and scope.

#### S2-31k. Bare bounded `FST m32fp [RAX]` (`D9 10`), partial slice

Implement only exact bare `D9 10` in Long64 with an existing x87 snapshot and
fixed Dword destination `[RAX]`. Reuse S2-31j's integer-only binary80-to-
binary32 conversion and its source/environment gates: Valid old physical TOP,
canonical Normal raw binary80 source with unbiased exponent -126..126, FCW
`037f/077f/0b7f/0f7f`, and `FSW & 0x80df == 0`. Reject non-normal results and
any exponent carry outside that interval before data access.

Validate the full canonical48 Dword range and issue exactly one atomic
GuestMemory write with no data read. Only after success update sticky PE and C1
(C1 means rounded magnitude up; exact results clear it under local policy),
record FIP/FDP, advance RIP by two and retire once. Preserve TOP, every tag,
all 80-bit payloads, FCW/FOP and unrelated CPU state; C0/C2/C3 are retained by
local deterministic policy. Check literal `D9 10` and exact `D9 10 DF E0`
fixtures, RC/tie/sign and exponent boundaries, all TOP values, atomic memory
faults and canonical range checks. Reject prefixes, WAIT, truncation and
neighboring D9 forms; native admission remains closed. This partial slice does
not claim hardware exception priority or general x87 exception delivery and
does not complete S2-31.

**Результат:** реализовано и проверено локальными fixtures/interpreter tests;
native execution remains explicitly denied. See the S2-31k report and CPU
contract for validation and scope.

#### S2-31l. Bare bounded `FSTP m64fp [RAX]` (`DD 18`), partial slice

Implement only exact bare `DD 18` in Long64 with an existing x87 snapshot and
fixed Qword destination `[RAX]`. Require a Valid old physical TOP whose raw
binary80 payload is canonical Normal; accept FCW exactly `037f/077f/0b7f/0f7f`
and require `FSW & 0x80df == 0`. For significand `s`, calculate
`q=s>>11`, `r=s&0x7ff`, `d=0x800`, then reuse
`round_quotient_magnitude(sign,q,r,d,RC)`. On a carry to `2^53`, shift `q`
right and increment the unbiased exponent. This local profile admits only
exponents -1022..1022; exponent 1023 is intentionally excluded even though
finite binary64 values exist there. Pack binary64 sign, biased exponent and
fraction bits directly, without host floating-point casts.

Preflight conversion and the complete canonical48 Qword range before data
access, then perform exactly one atomic Qword write and no read. Only after
success empty old TOP, advance TOP modulo eight, set sticky PE iff `r != 0`,
and set C1 iff rounding increased magnitude (exact results clear C1 by local
policy). Preserve all payloads/tags except the old TOP tag, FCW/FOP, C0/C2/C3
and unrelated state. Record FIP/FDP, advance RIP by two and retire once.
Architectural store faults use the guest-fault reporting path; backend errors
remain diagnostics. Cover exact `DD 18` and `DD 18 DF E0` fixtures, both signs
under all RC modes, ties and discarded-bit boundaries, exponent/carry edges,
all TOPs including wrap, memory atomicity and execution boundaries. Reject
neighbors, ModR/M/addressing alternatives, prefixes, REX, WAIT and truncation;
native admission remains explicitly closed. This partial slice does not
complete S2-31 or claim physical x86 acceptance.

**Результат:** реализовано, проверено локальными fixture/interpreter tests;
native admission остаётся закрытым. Финальные Mac mini/physical checks остаются
в одном завершающем подэтапе S2-31. Подробности — в отчёте и CPU contract.

#### S2-31m. Bare bounded `FST m64fp [RAX]` (`DD 10`), partial slice

Implement only exact bare `DD 10` in Long64 with an existing x87 snapshot and
fixed Qword destination `[RAX]`. Reuse unchanged S2-31l
`bounded_x87_m64fp_result`: Valid old physical TOP, canonical-Normal raw
binary80 source, FCW exactly `037f/077f/0b7f/0f7f`, and `FSW & 0x80df == 0`.
The integer-only binary64 profile accepts original and rounded unbiased
exponents -1022..1022, intentionally excluding exponent 1023; carry into that
excluded range rejects before data access.

Preflight the converted value and complete canonical48 range `[RAX,RAX+7]`,
then issue exactly one atomic Qword write with no read. After success, set PE
sticky iff conversion discarded nonzero bits, set C1 iff rounding increased
magnitude (exact results clear C1 by local policy), record FIP/FDP, advance RIP
by two and retire once. Preserve TOP, every physical tag, all eight raw
payloads (including the unrounded old ST(0)), FCW/FOP, C0/C2/C3 and unrelated
state. Architectural store faults map to the guest-fault path; backend failures
remain diagnostic errors. Reject prefixes, REX, WAIT, neighbors and
truncation; native admission remains closed. Literal DD10 and DD10+FNSTSW AX
fixtures and independent rounding, preservation, canonical-range, store-fault,
fetch/STOP/budget checks define this partial local policy; it does not complete
S2-31 or claim physical x86 acceptance.

**Результат:** реализовано и проверено локальными fixtures/interpreter tests;
native admission остаётся закрытым. Финальные Mac mini/physical checks остаются
в одном завершающем подэтапе S2-31. Подробности — в отчёте и CPU contract.

#### S2-31n. Bare bounded `FILD m32int [RAX]` (`DB 00`), partial slice

Implement only exact bare `DB 00` in Long64 with existing x87 state and the
fixed Dword source `[RAX]`. Before any data access require FCW exactly
`037f/077f/0b7f/0f7f`, `FSW & 0x80df == 0`, and Empty tag at physical
`(old_TOP+7)&7`; do not validate old TOP's tag or any unrelated tags/payloads.
Validate the full canonical48 range `[RAX,RAX+3]`, then issue exactly one
typed Dword read and no writes. Convert signed i32 exactly to canonical
binary80 through the shared integer-only i64 packer: zero has an all-zero
payload and Zero tag; nonzero values use `unsigned_abs`,
`e=63-leading_zeros(magnitude)`,
`significand=magnitude << (63-e)`, and exponent `16383+e` with the signed sign
bit. After the read succeeds, write payload/tag to the physical destination,
set TOP to that destination, clear C1, set FIP/FDP, advance RIP by two and
retire once. Preserve FCW/FOP, GPR/RFLAGS and the other seven tags/payloads.
Retaining C0/C2/C3 is an explicit local policy because they are undefined here;
no hardware fault-priority claim is made. Architectural read faults use the
guest path; backend failures remain diagnostics. Reject prefixes, REX, WAIT,
other ModR/M forms and truncation; explicitly deny native admission.

Add independent literal binary80 answers for zero, ±1/2/3, `2^24±1`, i32
maximum/minimum, all TOP values including wrap, all four RC values, stale
Empty destination payloads, arbitrary unrelated tags/payloads, missing state,
invalid environment, nonempty destination before memory, canonical range and
read fault/backend failures, and fetch/truncation/STOP/zero-budget boundaries.
Schema/CLI fixtures cover exact `DB 00` and optionally exact `DB 00 DF E0`.
Normative source: Intel SDM revision 093, Vol.2A, “FILD—Load Integer,”
pp.3-342–3-343.

**Результат:** локальная реализация и проверки фиксируются в отчёте; S2-31
остаётся частичным. Финальные Mac mini/physical checks выполняются одним
общим подэтапом после стабилизации серии, не в этом срезе.

#### S2-31o. Bare bounded `FILD m16int [RAX]` (`DF 00`), partial slice

Add exactly bare `DF 00` in Long64, preserving existing `DF 10`, `DF 18`,
`DF 38`, `DF E0`, `DF E9`, `DF F1`, and all DB forms. Reuse the S2-31n
preflight and push commit: existing x87 state, FCW `037f/077f/0b7f/0f7f`,
`FSW & 0x80df == 0`, and Empty physical `(TOP+7)&7`; do not inspect old ST(0)
or other tags/payloads. Validate the complete canonical48 Word range
`[RAX,RAX+1]`, then issue one typed Word read and no writes. Sign-extend i16
to i64 and reuse integer-only `pack_i64_binary80`; zero is all-zero payload
with Zero tag. Commit only after a successful read, preserve PE and unrelated
state, clear C1, update FIP/FDP/RIP, and retire once. C0/C2/C3 retention is a
local deterministic policy. Guest faults use the guest path; backend failures
remain diagnostics. Native admission stays denied.

Literal vectors cover zero, ±1/2/3, 32767/-32768; all TOP × RC combinations,
sign extension of `0x8000`/`0xffff`, TOP wrap, stale Empty payload, arbitrary
unrelated tags/payloads, sticky PE/C1, old ST(0) Empty/occupied, unaligned
Word access with canaries and no third-byte access, canonical range/faults,
and fetch/truncation/STOP/zero-budget rejection. Exhaustive DF ModR/M and
prefix checks add only `DF 00`, retaining existing DF forms. Schema and CLI
fixtures cover exact `DF 00` and `DF 00 DF E0`; native x87 admission explicitly
denies FILD m16. S2-31 remains partial. Final Mac mini/physical acceptance is
reserved for the combined final substage.

Normative source: Intel SDM revision 093, Vol.2A, “FILD—Load Integer,”
pp.3-342–3-343 ([official PDF, Vol.2A p.3-342](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=460),
[official PDF, Vol.2A p.3-343](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=461)).

**Результат:** локальная реализация и проверки фиксируются в отчёте; S2-31
остаётся частичным. Финальные Mac mini/physical checks выполняются одним
общим подэтапом после стабилизации серии, не в этом срезе.

#### S2-31p. Bare bounded `FILD m64int [RAX]` (`DF 28`), partial slice

Add only exact bare `DF 28` in Long64 and preserve all existing DF and DB
forms. Reuse the bounded FILD preflight: existing x87 state, FCW
`037f/077f/0b7f/0f7f`, `FSW & 0x80df == 0`, and Empty physical
`(TOP+7)&7`; do not inspect unrelated tags or payloads. Validate the complete
canonical48 Qword range `[RAX,RAX+7]` before exactly one typed Qword read, with
no write. Sign-extend the loaded i64 and use the generalized integer-only
binary80 packer. Zero is ten zero bytes with a Zero tag. Commit only the new
payload/tag, TOP, C1, FIP/FDP/RIP and retirement after the read succeeds;
preserve PE, other status/control fields, GPR/RFLAGS and unrelated x87 state.
C0/C2/C3 retention follows the existing local FILD policy. Architectural read
faults use the guest path; backend failures remain diagnostics. Native
admission remains denied.

Independent literal answers cover zero, ±1/2/3, i64 extrema, ±(`2^53−1`),
`2^53`, ±(`2^53+1`),
and ±(`2^62−1`), ±`2^62`, ±(`2^62+1`) over every TOP×RC combination. Verify
old ST(0) both Empty and occupied, stale destination payload, other tags and
payloads, sticky PE/C1 policy, unaligned Qword canaries and exactly eight read
bytes with no ninth-byte access. Check noncanonical/crossing/overflow ranges,
architectural #PF/backend errors, invalid gates and fetch/truncation/STOP/zero
budget boundaries. Exhaust DF ModR/M and all legacy/REX prefixes while
preserving the existing DF and DB forms. Schema/CLI fixtures contain exact
`DF 28` and `DF 28 DF E0`; native denial is explicit. S2-31 remains partial;
final physical acceptance stays in the combined final S2-31 substage.

Normative source: Intel SDM revision 093, Vol.2A, “FILD—Load Integer,”
pp.3-342–3-343 ([official PDF, Vol.2A p.3-342](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=460),
[official PDF, Vol.2A p.3-343](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=461)).

### S2-32. Остальные обязательные x87 формы

1. По матрице профиля отдельно реализовать нужные remainder, scale, square root, transcendental и прочие оставшиеся формы. Одно семейство — отдельное изменение.
2. Для операций с разрешёнными архитектурой погрешностями определить допустимый диапазон из источника до запуска. Требовать побитового равенства там, где оно нормативно необходимо.
3. Не вызывать host libm как автоматическую замену x87 semantics и не заявлять полный FPU feature при пропущенных обязательных формах.

**Готово:** нет пустых строк обязательного x87 набора; различия моделей и допустимая accuracy документированы, проверены сложные значения и exceptions.

### S2-33. MMX и aliasing с x87

1. Реализовать MMX register view поверх архитектурно общего состояния с x87; не хранить несогласованные независимые копии.
2. По подгруппам добавить moves, packed arithmetic/logic/shifts, saturation и EMMS.
3. Проверить переход MMX ↔ x87 и влияние на tags/TOP по спецификации.

**Готово:** packed lanes и совместное состояние проходят physical comparison; MMX не считается готовым только по EMMS.

### S2-34. XMM/MXCSR, SSE moves и logic

#### S2-34.1 — local-only lossless x87/SSE state snapshot

**Выполнено:** `LongModeState` теперь независимо хранит optional raw `SseState`
рядом с optional `X87State`. Новый отдельный `s2-long-x87-sse-state-v1`
fixture profile требует оба snapshot object и допускает ровно один `90` NOP
при budget 1 и stop RIP сразу после него. Все 16 XMM регистров сохраняются как
ровно 16 bytes каждый; MXCSR переносится как raw u32, без применения mask к
core state. Диагностические metadata профиля фиксируют ограниченную mask
`0x0000ffbf` и поясняют default без optional DAZ; это не universal hardware
CPUID mask и не guest `#GP` semantics. Старые профили сохраняют прежнюю форму и
отвергают `sse`; fragment adapter и native admission закрыты для этого state.
Локальные schema/comparator/CLI проверки подтверждают round-trip и обнаружение
изменений каждого XMM/MXCSR. Это только snapshot/state preservation, не SSE
execution или physical verification. S2-34, S2-38 и S2-39 остаются открыты.

#### S2-34.2 — local-only register `MOVUPS`

**Локально завершён узкий срез:** отдельный versioned fixture profile
`s2-long-movups-reg-v1` выполняет только legacy `0F 10 /r` с
ModR/M.mod=3: XMM register → XMM register. Допускается отсутствие REX либо
ровно один эффективный REX непосредственно перед `0F 10`; только REX.R/B
участвуют в выборе XMM0–XMM15. REX.W/X в этом profile не принимаются.
`66`, F2/F3, segment/address, LOCK, duplicate prefixes/REX, memory ModR/M,
`0F 11`, SSE2 variants, VEX и EVEX остаются закрытыми. Основание формы —
Intel SDM version 093, Vol. 2B, MOVUPS, entry `0F 10 /r` (operand 1
ModR/M.reg, operand 2 ModR/M.r/m); `0F 11 /r` является отдельной store form.

Profile явно предполагает SSE доступность, CR0.EM=0, CR0.TS=0 и
CR4.OSFXSR=1; он не реализует control-register model и не моделирует
`#UD/#NM` или control faults. Успешное исполнение копирует ровно 128 raw bits,
включая self-copy, и сохраняет x87, MXCSR, GPR и RFLAGS; RIP/retirement
обновляются только после успеха. Memory/alignment semantics, MXCSR behavior,
FP arithmetic, native/JIT и физическая проверка не входят в срез. Profile
остается полностью вне native/JIT. State-only `s2-long-x87-sse-state-v1`
сохраняет прежний NOP-only контракт. S2-34/D/System profile остаются открыты.

1. Добавить XMM state нужного режима и MXCSR с supported/reserved bits. Snapshot фиксирует raw bits всех заявленных lanes.
2. Добавить moves и logical forms по небольшим группам; различать aligned/unaligned, scalar/packed и сохранение верхних bits для каждой encoding.
3. Проверить alignment faults, memory widths и режимные/control-register gates там, где они нужны.

**Готово:** реальные memory/register cases проходят native oracle; host SIMD type не определяет архитектурный layout.

#### S2-34.3 — local-only reverse-direction register `MOVUPS` (partial)

Расширен тот же `s2-long-movups-reg-v1`: добавлена только register-only
encoding `0F 11 /r` при ModR/M.mod=3. В этом направлении ModR/M.reg + REX.R —
источник, ModR/M.r/m + REX.B — назначение; `0F 11 C1` копирует XMM0→XMM1,
`45 0F 11 C1` — XMM8→XMM9. Переиспользуются `Operation::MovupsRegister` и
существующий executor. Профиль сохраняет прежнюю политику префиксов и явные
предусловия SSE, CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1. Отвергаются `66`, F2/F3,
legacy segment/address, LOCK, duplicate/ignored prefix forms, memory ModR/M,
REX.W/X, VEX/EVEX, некорректный и усечённый encoding. Literal fixture проверяет
полный x87/XMM/MXCSR snapshot; локальные проверки охватывают все 256
source/destination пар, self-copy, неизменность остальных state bits, отсутствие
data-memory access и native-deny. State-only S2-34.1 остаётся NOP-only.

Основание encoding: Intel SDM revision 093, Vol. 2B, MOVUPS table entry
`NP 0F 11 /r`, Op/En B, `MOVUPS xmm2/m128, xmm1`: `xmm1` source,
`xmm2/m128` destination. Это только локальный срез, без заявления о физической
проверке и без закрытия S2-34/D/System.

#### S2-34.4 — local-only legacy `MOVUPS XMM,[RAX]`

Добавлен отдельный versioned fixture profile `s2-long-movups-mem-load-v1`;
предыдущие NOP-only `s2-long-x87-sse-state-v1` и register-only
`s2-long-movups-reg-v1` не расширяются. Допускается ровно `0F 10 /r` с
ModR/M.mod=00 и r/m=000 (`[RAX]`), bare либо с одним эффективным REX.R для
назначения XMM0–XMM15. REX.B/X/W, SIB, displacement, `0F 11`, сегментные и
прочие запрещённые префиксы, VEX/EVEX и остальные memory forms отвергаются.
Основание: Intel SDM revision 093, Vol. 2B, MOVUPS `0F 10 /r`, Op/En RM;
ModR/M.reg — destination, ModR/M.r/m — source. MOVUPS unaligned; alignment
fault здесь не вводится.

Для `[RAX]` executor проверяет полный canonical48 диапазон из 16 байтов и
выполняет ровно один `GuestMemory::read` в 16-байтовый временный буфер перед
изменением CPU. `OperandSize::Qword` не используется для этого доступа. Raw
байты целиком заменяют только выбранный XMM; x87, MXCSR, source memory, GPR и
RFLAGS сохраняются. RIP/retirement меняются после успешного read. Диапазонные,
backend и injected fetch/read ошибки локально оставляют CPU без commit; это не
заявление об аппаратном fault-priority/restart state. Профиль предполагает
SSE available, CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1; control-register gates,
`#UD/#NM`, MXCSR, stores, native/JIT и физическое исполнение не реализованы.
Локальные decoder, schema, executor, literal fixture, runner snapshot и
native-deny проверки покрывают только этот срез. Выбор Generic amd64 baseline
не означает, что эта одна форма составляет или подтверждает полный baseline.
S2-34/D/System остаётся открытым.

#### S2-34.5 — local-only legacy `MOVUPS [RAX],XMM`

Добавлен отдельный versioned fixture profile `s2-long-movups-mem-store-v1`;
S2-34.1 state-only, S2-34.2/.3 register и S2-34.4 memory-load profiles не
расширяются. Принимается только legacy `NP 0F 11 /r`, ModR/M.mod=00 и r/m=000
(`MOVUPS [RAX],XMM`), bare либо с одним effective REX.R для source XMM0–XMM15.
REX.B/X/W, SIB/displacement, другие базы, `0F 10`, запрещённые legacy
prefixes, duplicate prefixes/REX, VEX и EVEX остаются отвергнуты. Intel SDM
revision 093 Vol. 2B, MOVUPS entry `NP 0F 11 /r`, Op/En B, задаёт operand
порядок `xmm2/m128, xmm1`; ModR/M.reg (с REX.R) — source XMM, r/m — memory
destination. SDM указывает, что memory operand MOVUPS может быть unaligned;
SIMD FP exception отсутствует, а general exception behavior отнесено к Type 4
с отдельно отмеченной вариацией #AC. Profile не реализует эти architectural
gates/exceptions.

Executor проверяет полный 16-byte canonical48 range, берёт raw `[u8;16]` из
XMM до backend вызова и делает ровно один `GuestMemory::write`; `read` и
`OperandSize::Qword` не используются. Только после успешного write продвигаются
RIP и retirement. Ошибка backend сохраняет полное CPU state. Локальный
`FlatMemory` документирует и тестирует all-or-error для своих частных
synchronous fixture-RAM записей; это только backend policy, без утверждений об
hardware 16-byte atomicity, MMIO или SMP. Верхняя граница u64 остаётся явным
ограничением ISSUE-012. Проверки включают literal full-state/memory fixture,
все 16 XMM source payloads, canaries, unaligned RAX, один write/нет read,
read-only/разрыв mapping, injected write error без partial bytes и CPU commit,
canonical/overflow, forbidden/truncated encodings, STOP/budget и native/JIT
deny. SSE доступность и CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1 остаются
предусловиями, не runtime gates. x86-64-v1 — только ABI floor; этот slice не
завершает generic CPU profile и не закрывает S2-34/D/System.

#### S2-34.6a — local-only legacy MOVUPS base+displacement forms

Добавлен отдельный profile `s2-long-movups-mem-base-v1` для legacy
`0F 10 /r` load и `0F 11 /r` store. Допускаются ModR/M memory modes 00/01/10
с обычной 64-bit base+displacement адресацией: без displacement, signed disp8
или signed disp32. REX.R расширяет XMM0–7 до XMM8–15; REX.B расширяет GPR
base. REX.W/X, `r/m=100` (SIB), `mod=00/rm=101` (RIP-relative), address-size
override, segment/other legacy prefixes, VEX и EVEX остаются Unsupported.
Старые state-only, register-only и две точные `[RAX]` memory profiles сохраняют
исходные формы и schema admission.

Декодирование повторно использует `rm_operand`/`MemoryOperand`; второго
effective-address parser нет. Кодировка проверена по Intel SDM revision 093:
Vol. 2B MOVUPS `0F 10/11 /r`, Vol. 2A §§2.1.3–2.1.5 и 2.2.1.1–2.2.1.3
([официальная страница SDM](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html),
[Vol. 2A PDF](https://cdrdv2.intel.com/v1/dl/getContent/671199),
[Vol. 2B PDF](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf)).

Общий range helper проверяет все 16 байт и сохраняет ISSUE-012 для upper-u64
переполнения. Load выполняет ровно один `GuestMemory::read` во временный буфер,
затем commit XMM/RIP; store сначала копирует raw XMM, выполняет один
`GuestMemory::write` и продвигает RIP только при успехе. Backend error оставляет
полное CPU state без изменений. FlatMemory all-or-error — только policy
локальной fixture RAM и не означает hardware atomicity. Default DS/SS semantics
и #SS/#GP attribution берутся из `MemoryOperand`: RBP остаётся SS-базой, R13 —
DS/data-базой. RSP и R12 требуют SIB и поэтому не представимы в заявленном
no-SIB срезе. Новый handwritten fixture исполняет `MOVUPS [R8-7],XMM8`.

#### S2-34.6b — bounded RIP-relative MOVUPS memory forms

The existing `s2-long-movups-mem-base-v1` profile is extended with legacy
`0F 10 /r` load and `0F 11 /r` store when ModR/M.mod=00 and r/m=101. The
four-byte displacement is sign-extended; `MemoryOperand::NextRip` evaluates
from the RIP after all instruction bytes, including optional REX, `0F`, opcode,
ModR/M and disp32. REX.R remains admitted for XMM8–15. Intel SDM revision 093,
Vol. 2A §2.2.1.6 says REX.B does not change r/m=101 in this encoding and the
form remains RIP-relative even when REX.B=1; this bounded profile deliberately
rejects REX.B on that form instead of treating it as R13. Other accepted REX
bits retain the S2-34.6a allowlist; REX.W/X, SIB, address/segment/mandatory
prefixes, VEX/EVEX, alignment/control gates and native/JIT remain excluded.

The implementation reuses the shared ModR/M parser, `MemoryOperand::NextRip`
and the existing 16-byte range/read/write path. Loads use one read into a
temporary; stores use one write from the captured XMM bytes. CPU state commits
only after successful access. Tests cover both directions with positive, zero
and negative disp32, 7/8-byte instruction lengths, REX.R high XMM, current-RIP
and unsigned-displacement discriminators, canaries, canonical-range faults,
injected failures, every truncated displacement byte with no data callback,
STOP/budget, SIB/prefix/REX.B rejection and literal schema-v2 fixtures. Prior
state/register/exact `[RAX]` fixture profiles are unchanged.

#### S2-34.6c — shared SIB-addressed MOVUPS memory forms

The existing `s2-long-movups-mem-base-v1` profile additionally admits SIB
memory addressing for legacy `0F 10 /r` loads and `0F 11 /r` stores with
address-size 64. The shared `rm_operand` decoder and `MemoryOperand` provide
the effective address; no second address decoder was added. SIB scale values
1/2/4/8 apply to an optional index. Index field 4 means no index when REX.X=0
and R12 when REX.X=1. For ModR/M.mod=00 with SIB base field 5, the base is
absent and disp32 is sign-extended regardless of REX.B. Mod=01/10 use the
extended base including R13. Default #SS attribution follows actual RSP/RBP
base references; R12/R13 bases and index-only/no-base forms are DS/#GP
attributed. These rules were checked against Intel SDM revision 093, Vol. 2A
§§2.1.3–2.1.5, 2.2.1.1–2.2.1.3 and Vol. 1 §§3.3.7.1, 3.7.4.1, 3.7.5.1,
plus Vol. 2B MOVUPS (`0F 10 /r`, `0F 11 /r`):
[official SDM page](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html),
[Vol. 2A PDF](https://cdrdv2.intel.com/v1/dl/getContent/671199),
[Vol. 2B PDF](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf),
[Vol. 1 PDF](https://cdrdv2.intel.com/v1/dl/getContent/671436).

The shared execution path validates all 16 bytes before exactly one read or
write, with temporary-before-commit loads and success-only RIP/state commit.
Backend errors do not commit. Flat fixture RAM behavior does not claim hardware
atomicity, MMIO behavior, or SMP ordering. Tests cover scale 1/2/4/8,
RSP/RBP/R12/R13 and no-base classes, absent/normal/R12 index, mod 00/01/10,
REX.B/X, all 16 XMM registers and both directions, exact callbacks, canaries,
canonical #SS/#GP attribution, injected errors, rejected prefixes and
truncation, and STOP/budget behavior. Literal load/store fixtures are included;
the runner continues to deny native/JIT execution. REX.W, address-size 32,
legacy segment/mandatory prefixes, VEX/EVEX, and alignment/control gates remain
unsupported. S2-34 remains open; x86-64-v1 remains only the ABI floor.

#### S2-34.7a — local legacy XORPS register-to-register

Добавлен отдельный versioned fixture profile `s2-long-xorps-reg-v1`. Он
принимает только `0F 57 /r` при ModRM.mod=3: ModRM.reg — destination,
ModRM.r/m — source. Допустимы голое encoding либо один effective REX с
REX.R/B для XMM0–XMM15; REX.W/X и остальные префиксы отвергаются. Выполняется
побайтовый XOR всех 128 сырых битов; host FP conversion отсутствует. MXCSR,
x87, GPR, RFLAGS и остальные XMM сохраняются. MOVUPS-профили остаются без
изменений. Основание — Intel SDM revision 093, Vol. 2B, XORPS `NP 0F 57 /r`,
Op/En RM; VEX/EVEX и memory forms не входят.

Decoder/executor/schema/runner tests покрывают 16×16 регистров, self-XOR,
высокие регистры, каждый байтовый шаблон, отсутствие data callbacks,
сохранение состояния, отказ memory/prefix/REX вариантов, fetch truncation,
stop/budget и атомарный отказ без SSE state. Профиль диагностический и
local-only: SSE availability, CR0.EM=0, CR0.TS=0 и CR4.OSFXSR=1 предполагаются;
control-register gates, #UD/#NM delivery, native/JIT admission и physical
verification не заявляются. S2-34 и D/System profile остаются открытыми.

#### S2-34.7b — local legacy ANDPS register-to-register

Добавлен отдельный exact fixture profile `s2-long-andps-reg-v1`. Он принимает
только legacy `NP 0F 54 /r` при ModRM.mod=3: ModRM.reg — destination,
ModRM.r/m — source. Допустимы голое encoding или один effective REX с REX.R/B
для XMM0–XMM15; REX.W/X и остальные префиксы отвергаются. Все 128 бит
destination побайтово AND-ятся с source без host FP; MXCSR, x87, GPR, RFLAGS и
остальные XMM сохраняются. Основание: Intel SDM revision 093, Vol. 2A,
ANDPS, `NP 0F 54 /r`, Op/En A, ModRM.reg destination и ModRM.r/m source.
VEX/EVEX и memory forms не входят.

Decoder/executor/schema/runner tests покрывают 16×16 register matrix, полный
байтовый AND truth table, alias/self, high registers, literal fixture,
preservation, отсутствие data callbacks, forbidden memory/prefix/REX forms,
truncated fetch, stop/budget и atomic failure without SSE state. Профиль
diagnostic/local-only: SSE availability, CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1
предполагаются; control-register gates, #UD/#NM delivery, native/JIT, CPUID,
boot profile и physical verification не заявляются. S2-34 и D/System остаются
открытыми.

#### S2-34.7c — local legacy ORPS register-to-register

Добавлен отдельный exact fixture profile `s2-long-orps-reg-v1`. Он принимает
только legacy `NP 0F 56 /r` при ModRM.mod=3: ModRM.reg — destination,
ModRM.r/m — source. Допустимы голое encoding или один effective REX с REX.R/B
для XMM0–XMM15; REX.W/X и остальные префиксы отвергаются. Все 128 raw bits
побайтово OR-ятся без host floating operations; MXCSR, x87, GPR, RFLAGS и
остальные XMM сохраняются. Основание — Intel SDM revision 093, Vol. 2B, ORPS
`NP 0F 56 /r`, Op/En RM, ModRM.reg destination и ModRM.r/m source.

Decoder/executor/schema/runner проверки покрывают 16×16 регистров, все 256
байтовых значений в шаблонах XMM, high-register literal fixture, alias/self,
полное сохранение unrelated state, отсутствие data callbacks, запрет memory/
prefix/неподдерживаемых REX вариантов, truncation, STOP/budget и атомарный
отказ без SSE state. Runner запрещает native/JIT. Профиль diagnostic/local-only:
SSE availability и CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1 предполагаются; gates,
#UD/#NM, CPUID, boot profile и physical verification не заявляются. S2-34 и
D/System остаются открытыми.

#### S2-34.7d — local legacy ANDNPS register-to-register

Добавлен отдельный exact fixture profile `s2-long-andnps-reg-v1`. Он принимает
только legacy `NP 0F 55 /r` при ModR/M.mod=3: ModR/M.reg — destination,
ModR/M.r/m — source. Допустимы голое encoding или один effective REX с REX.R/B
для XMM0–XMM15; REX.W/X и остальные префиксы отвергаются. Все 128 raw bits
получают `(~old destination) & source`, без host floating operations; MXCSR,
x87, GPR, RFLAGS и остальные XMM сохраняются. Основание — Intel SDM revision
093, Vol. 2A, ANDNPS `NP 0F 55 /r`, Op/En RM: ModRM.reg — read/write
destination, ModRM.r/m — read source. В legacy operation инвертируется
destination, не source. VEX/EVEX и memory forms не входят.

Decoder/executor/schema/runner проверки покрывают 16×16 register matrix,
асимметричное направление, alias/self, все байтовые пары, high-register
literal fixture, сохранение unrelated state, отсутствие data callbacks,
forbidden memory/prefix/REX forms, truncated fetch, STOP/budget и atomic
failure without x87/SSE state. Runner запрещает native/JIT. Профиль
diagnostic/local-only: SSE availability, CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1
предполагаются; control-register gates, #UD/#NM, CPUID, boot profile и physical
verification не заявляются. S2-34 и D/System остаются открытыми.

### S2-35. Packed integer SSE/SSE2

1. По одной группе добавить lane arithmetic/logic/comparison, shuffle/unpack/pack и shifts из принятого профиля.
2. Для каждого width проверить wrap/saturation, signedness, lane order и count semantics.

**Готово:** fixtures покрывают все lanes, крайние values и memory faults; одна удачная vector operation не закрывает SSE2 feature.

### S2-36. Binary32/binary64 без потери семантики

1. На основе собственных числовых primitives реализовать нужные binary32/binary64 operations, rounding и exception classification.
2. Учесть signed zero, subnormals, NaN propagation, invalid conversion и отсутствие непредусмотренного fused rounding.
3. Не считать ARM64 FP эквивалентным x86 по умолчанию. Любой host-assisted путь требует доказанного semantic equivalence; в этом этапе предпочитать понятный собственный reference path.

**Готово:** независимые boundary vectors проверяют точные bits и flags; FP core не зависит от текущего host rounding mode.

### S2-37. SSE/SSE2 FP arithmetic и conversions

1. Добавлять scalar/packed arithmetic и comparisons, затем conversions в порядке таблицы профиля.
2. Применять MXCSR rounding и правила сохранения destination lanes; не распространять scalar effect на весь XMM.
3. Для approximate instructions использовать нормативные error bounds, для exact-result instructions — точное сравнение.

**Готово:** NaNs, infinities, signed zeros, subnormals, boundary conversions и обычные cases сопоставлены с Mac mini в поддерживаемых формах.

### S2-38. MXCSR и FP fault ordering

1. Реализовать exception masks/status и DAZ/FTZ только согласно объявленным capabilities и точной семантике формы.
2. Проверить unmasked exceptions, suppression/commit destination и приоритет memory/alignment/FP faults.
3. Явно разграничить x87 exception state и SSE exception state; одинаковый host signal не объединяет их.

**Готово:** фиксируются правильные values, flags, RIP и доступное exception metadata. CPUID SSE/SSE2 не включён до готовности обязательных state/exceptions/operations.

### S2-39. Сохранение и восстановление FP/SIMD

1. Реализовать требуемые x87 environment save/restore и FXSAVE/FXRSTOR формы с нормативными layouts, modes, alignment и reserved-bit checks.
2. Проверить mixed x87/MMX/XMM state, tag representation, instruction/data pointer rules и round-trip без потери bits.
3. XSAVE/XRSTOR/XGETBV и расширенное состояние добавлять только при требовании профиля S2-40; не объявлять OSXSAVE/AVX заранее.

**Готово:** snapshot → save → modify → restore восстанавливает заданное архитектурой состояние, invalid image даёт нужный fault. Маскирование undefined/reserved fields обосновано отдельно.

### S2-40. Дополнительные обязательные расширения

1. Сопоставить принятый Linux/Windows candidate profile с оставшимися ISA requirements: не считать SSE2 автоматически достаточным для любого современного ПО.
2. Каждое действительно обязательное расширение оформить отдельным набором decoder/state/semantics/exceptions/save-restore/oracle задач. Не добавлять AVX/AVX2/BMI «на будущее».
3. Если Mac mini не исполняет требуемую форму, оставить hardware coverage непроверенным и определить независимый способ проверки. Не удалять требование профиля и не выдавать software self-comparison за физическое доказательство.

**Готово:** для каждого обязательного расширения есть реализация и доказательства нужного уровня либо явно незакрытый блокер. Наличие блокера не позволяет принять соответствующий полный профиль.

## 10. Системная проверка и приёмка

### S2-41. Независимо проверить privileged/mode сценарии

1. Разделить доступные Linux user-mode cases, CPU contract tests с fixture backend и сценарии, которым нужны аппаратные привилегии/переходы режима.
2. Для последних подготовить малые собственные входы, наблюдаемые state/fault fields и способ получения независимого результата. Не использовать root Linux process как автоматический oracle CPL0/real mode.
3. На единственном Mac mini запуск bare-metal потребует отдельного носителя/времени перезагрузки и прямого согласования такого действия с владельцем. План не разрешает менять установленную Ubuntu, диск, bootloader или проводить перезагрузку по SSH.
4. Системные интеграционные cases, которым нужны paging/GDT/IDT/stack mappings/events, прогонять с реальным machine backend этапа 4. Одни mock callbacks не подтверждают full-system semantics.

**Готово:** обязательные system/mode строки проверены нужным независимым способом и integrated backend, ограничения приложены к результатам. До этого можно принять срез A/B/D, но не весь профиль этапа 2, включающий непроверенные C requirements.

### S2-42. Собрать общий corpus, reducer и harness

**Файлы:** существующий testkit и CPU harness; новые stage2 entry points только при реальной необходимости.

1. Добавить profile-aware generation memory/stack/addressing/REP/FP случаев и несколько фиксированных seeds. Контрольные handwritten cases должны покрывать обязательные границы каждой формы независимо от random generation.
2. Расширить reducer с сохранением layout, branch targets, fault class, mode, feature requirements и ожидаемого partial progress. Invalid input/timeout не сохраняют класс архитектурного mismatch.
3. Сохранить original/reduced input, full results, сравниваемые маски, source/revision/toolchain/profile и точную replay command. Не переписывать evidence предыдущих этапов.
4. Для каждой поддержанной группы среза запустить ручной набор и минимум 1000 различных допустимых generated cases по нескольким seeds, по два повтора на interpreter/native. Для запрещённых или опасных комбинаций использовать отдельно определённый bounded набор и явно указать причину; число cases не заменяет coverage matrix.
5. Обязательные selected suites не могут silently skip hardware, неподдержанные обязательные формы или отсутствующий backend. Невыбранное маркируется `not_run`.

**Готово:** одна документированная команда проверяет выбранный срез, выдаёт machine-readable summary и ненулевой exit при недоступной обязательной проверке. Mismatch воспроизводится и сокращается без изменения его смысла.

### S2-43. Диагностический interpreter и граница JIT

1. Предоставить CLI single-step/trace/replay с ограниченным объёмом и архитектурным state до/после, faulting RIP и memory effects. Не превращать trace в обязательный production overhead.
2. Сохранить старые interpreter/JIT/native regressions и текущий C ABI. Для расширенного состояния определить узкий контракт материализации flags/registers на exits.
3. JIT явно отклоняет новые неподдержанные формы; если есть interpreter fallback, он вызывает собственный core с тем же state и точной границей передачи управления. Не называть fallback нативной JIT-поддержкой.
4. Не реализовывать новый block JIT/cache/оптимизатор в этой задаче. Общий helper не считается независимым эталоном; hardware comparison сохраняется.

**Готово:** ошибка воспроизводится через interpreter CLI; совместимость существующего вертикального среза подтверждена. Новые формы честно отделены от JIT capability.

### S2-44. Оформить приёмку

1. Обновить CPU_CONTRACT по фактически реализованным exact forms/modes, CPUID features и fault guarantees. Не заменять список поддержки одним словом «x86-64».
2. Создать STAGE_2_REPORT с revision, profiles, конфигурациями Mac/Mac mini, командами, seeds, числом исполненных/failed/unsupported/not-run cases и ссылками на evidence.
3. Обновить этот план, INTERPRETER, CPU_ORACLE, TEST_CASE_FORMAT, STAND и README затронутых компонентов по реальным изменениям.
4. Зарегистрировать найденные нерешённые дефекты в KNOWN_ISSUES; решённые закрыть с regression evidence. Планируемые функции и гипотезы не объявлять подтверждёнными дефектами.
5. Сверить таблицу ниже. Если готов только срез, назвать именно его; обязательные незакрытые задачи оставить открытыми.

**Готово:** поддерживаемый профиль корректно исполняется на boundaries с ожидаемыми exceptions; interpreter остаётся диагностическим эталоном, а доказательства и ограничения воспроизводимы.

### Матрица приёмки

| Область | Обязательное доказательство | Что не засчитывается |
|---|---|---|
| State/widths/совместимость | 16 GPR/subregister tests, v1 и ABI regressions, новые snapshots | Нулевая заглушка новых полей |
| Fetch/decode | Все нужные формы, truncation/overlength/prefix/address cases, отсутствие лишнего fetch | Совпадение encoder с собственным decoder |
| Integer/memory/stack | Ручные boundaries + physical cases, определённые flags и memory effects | Сравнение только interpreter/JIT |
| REP и STOP/fault | Частичный прогресс и корректный replay/restart | Timeout с неизвестной точкой останова |
| Exceptions | RIP, тип, приоритет, определённые error/address fields и commit state | Host panic или один номер Linux signal |
| Modes/system | Все обязательные режимы/формы, independent evidence и нужная machine integration | Поля CR/MSR, mock callbacks и отсутствие crash |
| x87 | Extended precision, rounding/status/stack, special values и required forms | Host double и только обычные operands |
| MMX/SSE/SSE2 | Lanes/aliasing/state/FP effects, MXCSR и save/restore | Один arithmetic smoke при включённом feature bit |
| CPUID/расширения | Profile соответствует всему объявляемому поведению и требованиям guest | Копирование host CPUID или сокрытие обязательного требования |
| Harness | Реальные counts, seeds, fingerprint, reducer/replay, fail при отказе обязательной проверки | Пропуски, записанные как success |
| Документы/проблемы | Актуальные contracts/report/KNOWN_ISSUES | Финальный ответ в чате вместо документации |

Полная приёмка этапа требует всех обязательных строк принятого профиля. Загрузка Linux/Windows, скорость JIT, SMP, AMD coverage и release signing не следуют из этой приёмки и здесь не заявляются.

## 11. Команды и проверки

Существующие команды, доступные до реализации этапа 2:

```sh
# Локальный baseline этапа 1.
python3 tests/cpu/stage1_harness.py --suite local

# Обязательное сравнение текущего профиля с физическим Mac mini.
python3 tests/cpu/stage1_harness.py --suite oracle --generated-count 1000 --seed 0x5eedcafe --repeats 2

# Текущий fixture CLI — профиль и schema пока ограничены этапом 1.
cargo run --offline -p od-runner -- --fixture CASE.json --engine interpreter --json
```

`CASE.json` заменить реальным входом. Эти команды сами по себе не проверяют будущие memory/system/FP возможности.

После Rust-изменений:

```sh
cargo fmt --all -- --check
cargo check --workspace --all-targets --offline
cargo test -p od-isa --offline
```

Дополнительно запускать tests реально затронутых testkit/JIT/host/runners и явные межкомпонентные сценарии. Native code проверять на Ubuntu package-targeted командами и реальным исполнением; cross-build на Mac не заменяет его. `--offline` требует доступного dependency cache; отсутствие пакетов фиксировать как проблему окружения, а не обходить проверку.

Новые stage2 flags/commands, profile selection, suite names и replay operations документировать в INTERPRETER одновременно с работающей реализацией. До этого не приводить их как существующие успешные команды. Использовать текущие инфраструктуру и exit-code policy, расширяя versioned contract там, где необходимо.

После изменения структуры/путей и для документационных задач:

```sh
python3 tools/check_layout.py
git diff --check
```

После изменения кода — `graphify update .`. В evidence сохранять revision/build/profile, входы, исходные результаты, definedness masks и причины исключённых полей. Большие корпуса/логи держать в `work/`; в Git — малые собственные regression fixtures и нужные доказательства без секретов.

## 12. Первое задание для следующей модели

> Продолжи ограниченные срезы S2-15, S2-16 и S2-17 по CPU contract и отчёту. S2-17a–n покрывают документированные register/memory PUSH/POP и near CALL/RET forms; S2-17o добавляет только bare POPFQ (`9D`) при CPL3/IOPL0; S2-17p добавляет bare PUSHFQ (`9C`) с qword образом `RFLAGS AND 00000000_00FCFFFFH`, сохраняя RFLAGS и коммитя RSP после успешной bounded write; S2-17q добавляет `66 9C` PUSHFW, который сохраняет lower FLAGS word и коммитит RSP после успешной word write. AccessContext моделирует CPL, но не IOPL; это не ограничивает PUSHFQ. Общий финальный Mac mini acceptance включает 554 fixtures; никаких физических checks до отдельного acceptance substage. ISSUE-013/014 остаются открытыми. Не расширяй заявленный профиль без нормативного evidence и отдельных тестов.

## 13. Статус подготовки документа

Создан план будущей реализации срезов A–D и задач S2-00–S2-44. Разработка CPU, SSH-прогоны и новые ISA-проверки при составлении этого документа не выполнялись.

22 октября 2026 года выполнены `python3 tools/check_layout.py` и `git diff --check`: ошибок структуры, относительных ссылок и whitespace не найдено. Дополнительно проверены последовательность 45 задач, соответствие таблицы заголовкам, наличие критерия готовности у каждой задачи и закрытие блоков команд. Это проверка плана, а не подтверждение реализации этапа.

#### S2-30ag. Bare bounded `FUCOM ST(0), ST(1)` (local canonical-Normal slice)

Accept exactly bare `DD E1` with existing `Some(X87State)`. Require FCW
`0x037f`, `FSW & 0x80ff == 0`, Valid tags at old physical TOP and
`(TOP+1)&7`, and canonical Normal binary80 operands. Reuse the exact integer
comparison in `bounded_x87_fcom_result(state, 1)`. Greater sets C0/C1/C2/C3 to
`0000`, less to `1000`, and equal to `0001` (FSW masks `0`, `0x0100`,
`0x4000`) through `(old_fsw & !0x4700) | relation`. FUCOM does not pop: TOP,
all tags and all raw register payloads remain unchanged. FIP becomes the
instruction-start RIP, RIP advances by two, and the instruction retires once.
Clearing C1 for this bounded normal domain is a deterministic local policy;
the SDM specifies C1=0 for stack underflow but does not define it for this
normal case.

Missing state, invalid environment/tag/class, prefixes, REX, WAIT, unsupported
DD bytes, fetch failure/truncation, STOP and exhausted budget reject without
state commit. This slice excludes zero, subnormal, infinity, NaN, noncanonical
encodings, memory forms, FUCOMP/FUCOMPP, exception delivery, stack faults,
`#NM`, and native execution. In particular, the unique QNaN handling that
distinguishes FUCOM from FCOM is outside the accepted Normal-only domain.

Literal vectors cover every TOP, all three relations, all 16 initial condition
fields, signed operands, exponent ordering and low-significand differences.
Independent invalid lhs/rhs tags and classes plus environment, fetch,
truncation, STOP and zero-budget rejects preserve full state. Exhaustive DD
ModR/M checks add only E1 while retaining D0..D7 FST and D8..DF FSTP. Schema
and CLI fixtures cover exact `dde1`, `dde1dfe0`, and an actual invalid-rhs-tag
no-stop/budget-one rejection. Native admission explicitly denies `DD E1`.

Normative source: Intel SDM revision 093, Vol.2A,
“FUCOM/FUCOMP/FUCOMPP—Unordered Compare Floating-Point Values,” pp.3-402–3-404
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

#### S2-31q. Bare bounded `FSTP m80fp [RAX]` (`DB 38`), partial slice

Admit only exact bare `DB 38` in Long64 with an existing x87 snapshot. Require
Valid physical TOP, canonical-Normal raw binary80, FCW `037f/077f/0b7f/0f7f`,
and `FSW & 0x80df == 0`; sticky PE and C1 are allowed. Preflight the complete
canonical48 ten-byte `[RAX,RAX+9]` range, including checked `u64` overflow,
then call `GuestMemory::write(address, context, &raw)` exactly once. Do not read
destination data or split the store into narrower writes. Only after success,
empty the old physical TOP and advance TOP modulo eight. Clear C1, preserving
PE, C0/C2/C3 and all other FSW bits; preserve all payloads, other tags, FCW/FOP
and unrelated CPU state. Record FIP/FDP, advance RIP by two and retire once.

Memory faults leave the CPU uncommitted; the memory backend's atomic write
contract leaves RAM unchanged. Architectural faults use the guest-fault path;
backend failures remain diagnostics. Missing/invalid x87 state, prefixes,
REX, WAIT and other unadmitted DB forms reject atomically, while existing DB
integer, status, control and register forms retain their current behavior.
Truncation, fetch failure, STOP and zero budget also reject atomically. The SDM
marks C0/C2/C3 undefined for this store, so retaining
them is an explicit deterministic local policy. No native x87 execution or
general exception delivery is added.

Interpreter tests cover all TOP values and wrap, raw sentinels, fractional
binary80, minimum/maximum Normal exponents, all supported FCW values, sticky PE
and C1 policy, retained condition codes, unaligned exact-width writes with
canaries and nonzero byte 11, canonical start/end and overflow, denied and
injected backend/#PF writes, invalid source/environment before data access,
and fetch/truncation/STOP/budget boundaries. Literal schema-v2 and CLI fixtures
use exact `DB 38` and `DB 38 DF E0`; fixture bytes determine instruction
identity independently of case names. Native admission explicitly denies
`Operation::FstpM80FpRax`. S2-31 remains partial.

Normative source: Intel SDM revision 093, Vol.2A, “FST/FSTP—Store Floating
Point Value,” pp.3-386–3-387 (`DB /7`), official PDF p.504
([Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=504));
Vol.1 §§8.1.8, 8.1.9 and §8.6 for x87 state and FIP/FDP.

#### S2-31r. Bare bounded `FLD m80fp [RAX]` (`DB 28`), partial slice

Add only exact bare `DB 28` in Long64, preserving all existing DB forms and
explicitly leaving `DB 30` unsupported. Require existing x87 state, FCW exactly
`037f/077f/0b7f/0f7f`, `FSW & 0x80df == 0`, and Empty physical destination
`(old_TOP+7)&7`; the old logical ST(0) tag/payload is irrelevant. Preflight the
full canonical48 range `[RAX,RAX+9]` and checked endpoint arithmetic before
exactly one `GuestMemory::read` into `[u8; 10]`. Accept only canonical Normal
binary80 after that read; preserve all ten bytes exactly and reject every other
class without CPU commit. Success writes payload and Valid tag to the physical
destination, sets TOP to destination, clears C1, preserves PE/other status,
FCW/FOP, other tags/payloads and unrelated state, records FIP/FDP, advances
RIP by two and retires once. C0/C2/C3 retention is the existing local FILD
policy. Architectural faults use the guest-fault path and backend failures stay
diagnostic. Native admission remains denied.

Literal tests cover all TOPs including wrap, all accepted FCWs, arbitrary
unrelated state, sticky PE and C1 clearing, retained condition codes, raw
positive/negative fractional and exponent-endpoint Normal values, one unaligned
ten-byte read with no writes, invalid source classes after exactly one read,
canonical start/end/crossing/overflow, read #PF/backend errors, missing state,
invalid FCW/FSW/destination, and fetch/truncation/STOP/zero-budget boundaries.
Exhaustive DB ModR/M and prefix checks admit only `DB 28` in addition to the
existing DB forms and explicitly reject `DB 30`. Schema/CLI fixtures use exact
`DB 28`, `DB 28 DF E0`, and an invalid-class no-commit case. S2-31 remains
partial; native and physical acceptance remain reserved for the combined final
S2-31 substage.

Normative source: Intel SDM revision 093, Vol.2A, “FLD—Load Floating-Point
Value,” pp.3-352–3-353, official PDF p.470; Vol.1 §§8.1.8, 8.1.9 and §8.6 for
x87 state and FIP/FDP.

#### S2-31s. Bare bounded `FLD m32fp [RAX]` (`D9 00`), partial slice

Add only exact bare `D9 00` in Long64, preserving every other D9 form. Require
the existing bounded push environment: FCW `037f/077f/0b7f/0f7f`,
`FSW & 0x80df == 0`, and Empty physical destination `(old_TOP+7)&7`; old ST(0)
is irrelevant. Preflight canonical48 `[RAX,RAX+3]` and checked addition before
one exact Dword read. After the read, accept only source exponent fields 1..254;
zero, subnormal, infinity and NaN reject without CPU commit. Convert by integer
field expansion: `((1<<23)|fraction)<<40` for the binary80 significand and
`source_exp - 127 + 16383` for its exponent, preserving sign. Include exponent
127 and the full Normal source range. No host floating-point conversion or
the narrower binary80-to-binary32 store helper is involved.

Success writes the exact expanded payload and Valid tag to the destination,
decrements TOP, clears C1 while retaining PE and other status, and retains
C0/C2/C3 under this local policy. It records FIP/FDP, advances RIP by two and
retires once. Memory faults use the guest-fault path; backend failures remain
diagnostics. Native admission explicitly denies FLD m32fp. This partial slice
does not model binary32 special-value loads, x87 exceptions, or the remaining
S2-31 umbrella.

Coverage uses independent raw binary80 literals for ±1, adjacent fractions,
minimum and maximum Normal values and exponent 127 over every TOP×RC pair.
It checks one unaligned exact Dword read/no writes and canaries; invalid values
after one read; invalid x87 gates before access; canonical crossing and checked
overflow; read #PF/backend failures; fetch/truncation/STOP/budget rejection;
exhaustive D9 ModR/M and prefix preservation; exact `D9 00`, `D9 00 DF E0`, and
invalid-class schema/CLI fixtures; and native denial. S2-31 remains partial.

Normative source: Intel SDM revision 093, Vol.2A, “FLD—Load Floating-Point
Value,” pp.3-352–3-353, official PDF p.470; Vol.1 §§8.1.8, 8.1.9 and §8.6 for
x87 state and FIP/FDP.

#### S2-31t. Bare bounded `FLD m64fp [RAX]` (`DD 00`), partial slice

Add only exact bare `DD 00` in Long64, preserving every other DD form and
prefix rejection. Require the existing bounded push environment: FCW
`037f/077f/0b7f/0f7f`, `FSW & 0x80df == 0`, and Empty physical destination
`(old_TOP+7)&7`; old ST(0) is irrelevant. Preflight checked canonical48
`[RAX,RAX+7]`, then issue exactly one Qword read. Accept only binary64 Normal
values with exponent field 1..2046; zero, subnormal, infinity, and NaN reject
after the read without CPU commit. Expand by integer fields:
`((1<<52)|(fraction))<<11` for the binary80 significand and
`source_exp - 1023 + 16383` for the exponent, preserving sign. Include the full
unbiased range -1022..=+1023, especially +1023; do not use the narrower
binary80-to-binary64 store helper.

Success writes the expanded payload and Valid tag to the destination,
decrements TOP, clears C1 while retaining PE and other status, and retains
C0/C2/C3 under the local policy. It records FIP/FDP, advances RIP by two and
retires once. Memory faults use the guest-fault path; backend failures remain
diagnostics. Native admission explicitly denies FLD m64fp. This partial slice
does not model special-value loads, general x87 exceptions, or the remaining
S2-31 umbrella.

Coverage uses independent raw binary80 literals for ±1, adjacent fractions,
minimum and maximum Normal and exponent +1023 over every TOP×RC pair. It also
checks sticky PE/C1 and preserved status/tags/slots, one unaligned exact Qword
read/no writes and canaries, invalid values after exactly one read, invalid
x87 gates before access, canonical start/end/crossing/overflow, read #PF/backend
errors, fetch/truncation/STOP/budget boundaries, exhaustive DD ModR/M and
prefix preservation, exact `DD 00`, `DD 00 DF E0`, and invalid-class CLI/schema
fixtures, plus native denial. S2-31 remains partial.

#### S2-31u. Extend bounded `FLD m64fp [RAX]` (`DD 00`) to signed zero

Retain the S2-31t byte encoding, FCW/FSW/Empty-destination gates, complete
canonical48 Qword range and exactly one Qword read. After that read, accept
binary64 signed zero in addition to Normal exponent fields 1..2046. Pack +0 as
ten zero bytes and -0 as ten zero bytes with the final sign byte `0x80`; both
receive `X87Tag::Zero`. Normal retains its exact integer expansion and Valid
tag. Nonzero subnormals, infinity and NaNs remain Unsupported after the read
with no CPU commit. Push/status/FIP/FDP/RIP and native-denial behavior otherwise
remain as in S2-31t.

Use independent +0/-0 literals across all TOP×RC combinations; retain Normal
vectors, sticky PE/C1 checks, old ST(0) independence, unrelated slot
preservation, TOP wrap, canonical-range and single-read checks. Verify
positive/negative minimum-subnormal rejection after one Qword read. Reclassify
the former zero rejection fixture to a positive
nonzero subnormal. The exact `DD 00` and `DD 00 DF E0` CLI fixtures cover
successful +0, with the composition observing the Zero tag and pushed TOP via
FNSTSW AX. The earlier S2-31t report remains a record of the Normal-only slice.
S2-31 stays partial; physical acceptance is deferred to its combined final
substage.

Normative source: Intel SDM revision 093, Vol.2A, “FLD—Load Floating-Point
Value,” pp.3-352–3-353, official PDF pp.470–471; Vol.1 §§8.1.8, 8.1.9 and §8.6
for x87 state and FIP/FDP. Local status/tag details are the project's bounded
policy.

Normative source: Intel SDM revision 093, Vol.2A, “FLD—Load Floating-Point
Value,” pp.3-352–3-353, official PDF p.470; Vol.1 §§8.1.8, 8.1.9 and §8.6 for
x87 state and FIP/FDP.

#### S2-31v. Extend bounded `FLD m32fp [RAX]` (`D9 00`) to signed zero

Keep the exact D9 00 encoding, existing x87 state and FCW/FSW/Empty-destination
gates, checked complete canonical48 Dword range, and exactly one Dword read.
After the read, accept either sign of binary32 zero: pack +0 as ten zero bytes,
-0 with the final sign byte `0x80`, and assign `X87Tag::Zero`. Preserve the
existing integer expansion and Valid tag for Normal exponent fields 1..254.
Nonzero subnormals, infinity and NaNs remain Unsupported after the read with
no CPU commit. Preserve push/TOP/C1/status/FIP/FDP/RIP and native-denial
policies; do not add reads or writes.

Add independent ±0 literals across all TOP×RC pairs and retain all Normal
vectors. Keep ±minimum-subnormal rejection after one read. Change the former
zero-rejection schema/CLI fixture to positive minimum subnormal and narrow its
exact exception predicate to nonzero binary32 subnormal under bare D9 00,
budget one and no stop RIP. Update success D9 00 and D9 00 DF E0 fixtures and
CLI assertions to cover Zero tag and signed payloads. Document the current
contract in the CPU contract, interpreter and fixture notes; preserve the
historical S2-31s report and add this result separately. S2-31 and ISSUE-019
remain open; physical acceptance stays in the combined final substage.

Normative source: Intel SDM revision 093, Vol.2A, “FLD—Load Floating-Point
Value,” pp.3-352–3-353, official PDF pp.470–471
([Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=470));
Vol.1 §§8.1.8, 8.1.9 and §8.6 for x87 state and FIP/FDP. Signed-zero packing
and tag details follow local policy.

#### S2-31w. Extend bounded `FLD m80fp [RAX]` (`DB 28`) to signed zero

Keep exact bare `DB 28`, the existing push gates, canonical48 ten-byte
preflight and exactly one `GuestMemory::read` into `[u8; 10]`. After the read,
accept canonical binary80 Normal unchanged with `X87Tag::Valid`, and accept
both raw signed binary80 zeros unchanged with `X87Tag::Zero`. Every other
class, including both signs of nonzero minimum subnormal, remains Unsupported
after one read and without CPU commit. Intel SDM revision 093 Vol.2A FLD
m80fp specifies no denormal exception for this format; do not import binary32/
binary64 #D behavior or broaden the class policy. Any future denormal and
unmasked exception behavior requires its own semantics slice.

Preserve TOP push/wrap, C1 clearing, PE and other local status policy, FCW/FOP,
other tags/payloads, FIP/FDP/RIP and one retirement. Keep native admission
denied. Literal +0/-0 vectors cover every TOP×FCW combination; minimum
subnormal ± cases and other specials reject after the one exact read with full
CPU state unchanged. Update the base fixture to +0 and the FNSTSW composition
to -0, asserting the exact raw bytes and Zero tag. Reclassify the old
zero-rejection fixture and its exact DB 28 / budget-one / no-stop schema
predicate to nonzero binary80 subnormal. Preserve the S2-31r report as the
historical Normal-only record and add this result separately. S2-31 remains
partial; physical acceptance stays in the combined final substage.

Normative source: Intel SDM revision 093, Vol.2A, “FLD—Load Floating-Point
Value,” pp.3-352–3-353, official PDF p.470
([Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=470));
Vol.1 §§8.1.8, 8.1.9 and §8.6 for x87 state and FIP/FDP. The no-#D statement
is specific to FLD m80fp. Broader denormal/unmasked policy is deferred.

#### S2-31x. Extend bounded `FLD m80fp [RAX]` (`DB 28`) to canonical subnormals

Preserve the exact `DB 28` instruction, S2-31w FCW/FSW/Empty-destination
gates, complete canonical48 ten-byte range check, and exactly one read. Keep
Normal as a byte-preserving Valid push and signed zero as a byte-preserving
Zero push. Accept canonical nonzero binary80 Subnormal only when exponent and
explicit integer bit are zero and the fraction is nonzero; copy all ten raw
bytes exactly and set the pushed tag to Special. Do not normalize the payload
or derive the tag from numeric magnitude. Preserve the existing push, C1, PE,
FIP/FDP/RIP and retirement behavior, with no added DE/UE/PE status effects.

After the single read, continue to reject pseudo-denormals, infinity, NaNs,
unnormals and pseudo-special classes atomically. Reclassify the narrow
DB 28/budget-one/no-stop Unsupported fixture and schema exception from positive
minimum subnormal to positive pseudo-denormal; reject the negative
pseudo-denormal and retain other special-class rejection vectors. Native x87
admission remains closed. Add literal raw vectors for signed minimum,
largest and interior subnormals across every TOP×FCW combination, plus exact
`DB 28` and `DB 28 DF E0` success fixtures. Preserve the S2-31w history.

Normative source: Intel SDM revision 093, Vol.2A, “FLD—Load Floating-Point
Value,” pp.3-352–3-353, official PDF p.470
([Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=470)).
The m80fp entry defines no `#D`; do not infer binary32/binary64 denormal
behavior from this entry. Broader denormal/unmasked exception semantics and
physical acceptance remain outside this slice.

#### S2-31y. Extend bounded `FLD m32fp [RAX]` (`D9 00`) to masked subnormals

Keep exact bare `D9 00`, x87 state, four all-masked FCWs, Empty
`(TOP-1)&7`, complete canonical48 Dword preflight, and one Dword read/no
writes. For source exponent zero and fraction `f != 0`, normalize exactly:
`k=floor(log2(f))`, binary80 significand `f << (63-k)`, unbiased exponent
`k-149` (biased `16383+k-149`), preserving sign. Store canonical Normal and
assign Valid. Set sticky FSW.DE only; do not add UE or PE, clear C1 under the
existing load policy, and preserve remaining flags, payloads, TOP and pointer
behavior. Permit pre-existing DE only in this D900 gate using `FSW & 0x80dd`;
leave the shared FILD/m64/m80 gate unchanged. Unmasked FCWs remain unsupported
and no exception delivery is implemented. Signed zero and Normal keep their
S2-31v behavior; infinity and NaN remain Unsupported after one read without
commit. Native admission stays closed.

Use independent literal min/interior/largest subnormals of both signs over
all TOP×FCW pairs, plus sticky-PE/C1/DE and repeated-load checks. Preserve
single-read/preflight and atomic reject coverage; reclassify the schema
exception fixture from minimum subnormal to binary32 infinity/NaN and retain
the exact D900, budget-one, no-stop predicate. Add successful positive D900
and negative D900/DFE0 fixtures and run both through the CLI. The S2-31v
report remains historical. Intel SDM rev.093 Vol.1 §8.5.2 describes masked
denormal-operand handling; Vol.2A FLD pp.3-352–3-353 (official PDF p.470)
specifies the load format. This bounded all-masked profile does not infer
unmasked push/exception behavior from that statement. Binary80 m80 subnormal
handling remains the separate S2-31x behavior.

#### S2-31z. Extend bounded `FLD m64fp [RAX]` (`DD 00`) to masked subnormals

Keep exact bare `DD 00`, existing x87 state, the four all-masked FCWs, Empty
`(TOP-1)&7`, complete canonical48 Qword preflight and one Qword read/no
writes. For source exponent zero and fraction `f != 0`, normalize exactly:
`k=floor(log2(f))`, binary80 significand `f << (63-k)`, unbiased exponent
`k-1074` (biased `16383+k-1074`), preserving sign. Store canonical Normal and
assign Valid. Set sticky FSW.DE only; do not add UE or PE, clear C1 under the
existing load policy and preserve other flags, payloads, TOP and pointer
behavior. Permit pre-existing DE only in this DD00 gate using
`FSW & 0x80dd`; leave FILD and m80 gates unchanged. Unmasked controls remain
unsupported and no exception delivery is implemented. Signed zero and Normal
keep their S2-31u behavior; infinity and NaN remain Unsupported after one read
without commit. Native admission stays closed.

Use independent signed minimum, interior and largest subnormal literals over
all TOP×FCW pairs, plus sticky-PE/C1/DE and repeated-load checks. Preserve
single-read and preflight atomicity coverage. Reclassify the schema exception
fixture from minimum subnormal to binary64 infinity/NaN while retaining exact
DD00, budget-one and no-stop constraints. Add successful positive DD00 and
negative DD00/DFE0 fixtures and run both through the CLI. S2-31u remains the
historical signed-zero/subnormal-rejection result. Intel SDM rev.093 Vol.1
§8.5.2 describes masked x87 denormal-operand handling; Vol.2A FLD
pp.3-352–3-353 (official PDF p.470) specifies the load format. This bounded
all-masked profile does not infer unmasked exception/push behavior. Binary80
m80 subnormal handling remains the separate S2-31x behavior.

#### S2-31aa. Extend bounded `FLD m80fp [RAX]` (`DB 28`) to canonical infinities

Keep exact bare `DB 28`, the S2-31x FCW/FSW gate `0x80df`, Empty
`(TOP-1)&7`, canonical48 ten-byte preflight, one raw read and no writes. After
the read, accept only canonical signed infinities: exponent `0x7fff`, explicit
integer bit set, and fraction zero. Preserve all ten source bytes and push with
the Special tag. Preserve the existing push/TOP, C1-clear, pointer, FOP and
retirement policy; add no status flags. Normal, signed zero and canonical
subnormal keep their existing S2-31t/w/x behavior. NaNs, pseudo-infinities,
pseudo-denormals, unnormals and pseudo-NaNs remain Unsupported after one read
without CPU commit. Do not model unmasked exception delivery. Native x87
admission stays closed.

Add positive/negative infinity literals over every TOP and supported FCW,
checking raw bytes, Special tag, unchanged status except C1/TOP, and one exact
read. Retain atomic rejection coverage for canonical quiet/signaling NaNs,
pseudo-infinities of both signs, pseudo-denormals, pseudo-NaNs and unnormals,
plus existing preflight, memory fault, truncation, STOP and budget cases. Move
the exact `DB 28`, budget-one, no-stop Unsupported schema/CLI fixture from
pseudo-denormal to canonical positive QNaN; adjacent classes must not use that
narrow exception. Add CLI success fixtures for bare positive infinity and
negative infinity composed with FNSTSW, and verify JIT/native denial remains
explicit. Intel SDM revision 093 Vol.1 Table 4-3 defines the canonical
double-extended infinity encoding and Table 8-3 distinguishes pseudo-infinity
and other unsupported encodings; Vol.2A FLD m80fp remains the instruction
reference. No physical or Mac mini acceptance is part of this slice.

#### S2-31ab. Extend bounded `FLD m80fp [RAX]` (`DB 28`) to canonical QNaNs

Keep the S2-31aa gates: exact bare `DB 28`, supported all-masked FCWs,
`FSW & 0x80df == 0`, Empty `(TOP-1)&7`, canonical48 ten-byte preflight and one
raw read. Accept only canonical QuietNaN encodings: exponent `0x7fff`, explicit
integer bit set, quiet bit set, and nonzero fraction. Preserve all ten source
bytes and push with Special tag. Preserve push/TOP, C1-clear, FSW and PE, pointer
and retirement behavior; do not set IE or any other new status bit. Signed zero,
Normal, subnormal and infinity paths keep their existing behavior. Signaling
NaNs, pseudo-NaNs, pseudo-denormals, pseudo-infinities, unnormals and all other
classes remain post-read Unsupported without commit. Native x87 admission stays
closed; do not add exception delivery or unmasked behavior.

Add independent QNaN payload/sign literals across every TOP×FCW pair, including
the canonical negative indefinite QNaN. Verify raw payload, Special tag,
preserved sticky PE, no IE, one ten-byte read and no writes. Convert the exact
`DB 28`, budget-one, no-stop Unsupported fixture/schema exception from positive
canonical QNaN to positive canonical SNaN; other QNaN signs/payloads and
neighbor classes must not match. Add bare positive QNaN and negative-indefinite
`DB 28 DF E0` CLI fixtures, and keep interpreter/JIT/native denial tests. The
Vol.1 binary80 special-value table is the encoding reference; no hardware or
physical acceptance is claimed.

#### S2-31ac. Extend bounded `FLD m80fp [RAX]` (`DB 28`) to canonical SNaNs

Keep the exact S2-31ab gates: bare `DB 28`, all-masked FCWs,
`FSW & 0x80df == 0`, Empty `(TOP-1)&7`, canonical48 ten-byte preflight and
one raw read. Accept canonical SignalingNaNs only: biased exponent `0x7fff`,
explicit integer bit set, quiet bit clear and remaining fraction nonzero.
Preserve all ten bytes including the signaling bit/payload and push with
Special tag; do not quiet the source or add exception flags, including IE.
Preserve PE and C0/C2/C3 under the existing local C1-clear/TOP/pointer/
retirement policy. Pseudo-SNaNs, pseudo-infinity, unnormals, pseudo-denormals
and pseudo-QNaNs remain Unsupported after one read without CPU commit. Native
and JIT admission and unmasked exception behavior remain outside scope.

Add independent minimum, middle and maximum SNaN payloads of both signs over
every TOP×FCW pair; verify exact raw payload, Special tag, status preservation,
one ten-byte read and no writes. Add positive bare DB28 and negative
DB28/DFE0 FNSTSW success fixtures. Move the current exact-DB28, budget-one,
no-stop schema/CLI rejection fixture from canonical positive SNaN to positive
pseudo-infinity; ensure canonical SNaNs and pseudo-SNaNs do not match the
narrow predicate. Keep explicit native/JIT denial. No physical acceptance is
claimed.

#### S2-30an. Bare indexed FCOMI-family register forms

Generalize the four existing `i=1` forms to exact bare DB/DF register bytes:
`DB E8..EF` FUCOMI, `DB F0..F7` FCOMI, `DF E8..EF` FUCOMIP, and `DF F0..F7`
FCOMIP. The low three ModR/M bits are the `source:u8` index. Preserve DB E2/E3
and DF E0; reject every other DB/DF byte, every prefix including REX and WAIT,
and memory ModR/M forms. The instruction remains exactly two bytes.

Preflight `bounded_x87_fcom_result(state, i)`: require existing x87 state,
FCW `0x037f`, `FSW & 0x80ff == 0`, and a Valid canonical-Normal old TOP. For
`i != 0`, require and read actual physical `(TOP+i)&7`, also Valid and
canonical Normal. For `i=0`, compare the old TOP to itself and do not read or
require ST(1). On success map greater/less/equal to RFLAGS relation bits zero,
CF, and ZF after clearing `0x8d5`; define all six affected flags and clear only
FSW.C1. Nonpop forms preserve TOP/tags/payloads. Pop forms empty only the old
TOP and increment TOP exactly once, including the self case. All physical
register payloads stay unchanged. Update FIP/RIP and retire once.

The schema admits exactly 32 comparison opcodes standalone or followed by DF
E0, for 64 exact program strings total. Its no-stop/budget-one reject profile
checks only the actual encoded physical source with a clean environment and a
Valid old TOP; the `i=0` class reject uses the exact all-zero payload. Existing
`i=1` rejection admission is preserved. Native profile and instruction
admission explicitly deny all indexed operations. No unordered/special values,
x87 exceptions, CR0/#NM, WAIT, memory forms, or native x87 execution are added.

Coverage includes exhaustive DB/DF second-byte and prefix decoding, every
TOP×source×ordered relation×16 FSW condition fields, all 64 initial affected
RFLAGS masks for each family and source, TOP wrap, self compare with unrelated
Empty tags, source-indexed physical slot distinctions, atomic invalid operand,
environment and execution-boundary rejection, and exact schema acceptance and
near-miss rejection. Literal CLI fixtures preserve existing `i=1` cases and
add self compare, indexed wrap/pop, ST(7) mapping, and actual-source reject
boundaries.

Normative source: Intel SDM revision 093, Vol.2A,
“FCOMI/FCOMIP/FUCOMI/FUCOMIP—Compare Floating-Point Values,” pp.3-327–3-329
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

#### S2-30aj. Bare bounded `FCOMI ST(0), ST(1)`

Accept exactly bare `DB F1`. Before commit require an existing x87 snapshot,
FCW `0x037f`, `FSW & 0x80ff == 0`, Valid tags at old physical TOP and
`(TOP+1)&7`, and canonical-Normal binary80 operands through
`bounded_x87_fcom_result(state, 1)`. Greater sets RFLAGS relation bits to zero,
less sets CF, and equal sets ZF. Clear CF/PF/AF/ZF/SF/OF before setting the
relation; decoded `FlagEffects` marks all six bits defined. Preserve every
other RFLAGS bit. Clear only FSW.C1; preserve C0/C2/C3, TOP, all tags and raw
payloads, FCW, FDP/FOP, GPRs and memory. Set FIP to instruction-start RIP,
advance RIP by two and retire once.

Reject missing state, invalid FCW/FSW/tags/classes and every other unadmitted
DB form; preserve the existing bare DB E2 FNCLEX and DB E3 FNINIT forms.
Prefixes, REX, WAIT, fetch/truncation, STOP and exhausted budget reject without
state commit. Exceptions and other FCOMI/FCOMIP/FUCOMI/FUCOMIP forms are excluded.
Coverage includes all TOP × relation × 16 initial FSW condition fields, all 64
initial six-flag masks with definedness, independent bad operand tags/classes,
environment and boundary atomicity, exhaustive DB decode/prefixes, exact
`dbf1` and `dbf1dfe0` fixtures, and explicit native denial of `Operation::Fcomi`
and DB F1.

Normative source: Intel SDM revision 093, Vol.2A,
“FCOMI/FCOMIP/FUCOMI/FUCOMIP—Compare Floating-Point Values,” pp.3-327–3-329.

#### S2-30ak. Bare bounded `FCOMIP ST(0), ST(1)`

Accept exactly bare `DF F1`. Before mutation, require `Some(X87State)`, FCW
`0x037f`, `FSW & 0x80ff == 0`, Valid tags at old physical TOP and
`(TOP+1)&7`, and canonical-Normal binary80 operands by
`bounded_x87_fcom_result(state, 1)`. Greater sets relation flags to zero, less
sets CF, and equal sets ZF. Clear CF/PF/AF/ZF/SF/OF before setting the relation;
`FlagEffects` defines all six bits (`0x8d5`) on successful retirement.

Clear only FSW.C1, empty the tag at the actual old physical TOP, and advance
TOP to `(old TOP+1)&7`. Preserve the old ST(1) Valid tag, every physical
register payload, C0/C2/C3, FCW, FDP/FOP, every other FSW bit, all GPRs and
memory. Set FIP to instruction-start RIP, advance RIP by two and retire once.
Only bare `DF F1` is admitted under DF; the existing `DF E0` FNSTSW AX and new
`DF F1` are the only accepted encodings. Existing DB E2 FNCLEX, DB E3 FNINIT
and DB F1 FCOMI remain admitted. Exceptions, other FCOMI-family forms,
prefixes, REX, WAIT, fetch/truncation, STOP and exhausted budget reject without
a state commit. Native admission explicitly denies `Operation::Fcomip` and
DF F1.

Coverage includes every TOP × ordered relation × 16 initial FSW condition
fields, all 64 initial subsets of the six changed RFLAGS bits with definedness,
including TOP wrap, payload/tag sentinels, independent bad operand tags and
classes, invalid environment and execution-boundary atomicity. Literal schema
and CLI fixtures use exact `dff1`/`dff1dfe0`; the no-stop/budget-one rejection
fixture has an Empty tag at actual old physical ST(1). Exhaustive DF decoding
admits only E0 and F1 and rejects every other second byte and all prefixes.

Normative source: Intel SDM revision 093, Vol.2A,
“FCOMI/FCOMIP/FUCOMI/FUCOMIP—Compare Floating-Point Values,” pp.3-327–3-329
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

#### S2-30al. Bare bounded `FUCOMI ST(0), ST(1)`

Accept exactly bare `DB E9`. Preflight `bounded_x87_fcom_result(state, 1)`
before mutation; it requires an existing x87 state, FCW `0x037f`,
`FSW & 0x80ff == 0`, Valid tags at old physical TOP and `(TOP+1)&7`, and
canonical-Normal binary80 operands. Greater sets the changed RFLAGS bits to
zero, less sets CF and equal sets ZF; `FlagEffects` defines all six bits in
mask `0x8d5`. Clear only FSW.C1. Preserve C0/C2/C3, TOP, both tags and raw
payloads, FCW, FDP/FOP, GPRs and memory. Set FIP to start RIP, advance RIP by
two and retire once. No pop, unordered QNaN handling, exception delivery or
native x87 execution is included.

Keep the DB allowlist to existing E2/E3/F1 plus E9; preserve the existing bare
DF E0 FNSTSW AX and DF F1 FCOMIP forms, and reject all other DB/DF forms,
prefixes, REX, WAIT and memory operands. Coverage includes exhaustive
DB/prefix decoding, all TOP × ordered relation × 16 initial FSW condition
fields, all 64 affected RFLAGS masks with definedness, invalid classes/tags/
environment and execution-boundary atomicity. Literal schema/CLI fixtures use
exact `dbe9dfe0` and `dbe9`; the no-stop/budget-one rejection has Empty actual
old ST(1). Reject the unsupported variant `dbf8dfe0`. Native admission
explicitly denies `Operation::Fucomi` and DB E9.

Normative source: Intel SDM revision 093, Vol.2A,
“FCOMI/FCOMIP/FUCOMI/FUCOMIP—Compare Floating-Point Values,” pp.3-327–3-329
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

#### S2-30am. Bare bounded `FUCOMIP ST(0), ST(1)`

Accept exactly bare `DF E9`. Before mutation, require `Some(X87State)`, FCW
`0x037f`, `FSW & 0x80ff == 0`, Valid tags at old physical TOP and
`(TOP+1)&7`, and canonical-Normal binary80 operands from
`bounded_x87_fcom_result(state, 1)`. Greater clears all six affected RFLAGS
bits, less sets CF, and equal sets ZF; `FlagEffects` defines CF/PF/AF/ZF/SF/OF
(`0x8d5`).

Clear only FSW.C1, empty the tag at actual old physical TOP, and set TOP to
`(old TOP+1)&7`. Preserve the old ST(1) Valid tag, every physical register
payload, C0/C2/C3, FCW, FDP/FOP, every other FSW bit, all GPRs and memory. Set
FIP to instruction-start RIP, advance RIP by two and retire once. Missing state,
invalid environment/tag/class, every other DB/DF form, prefixes, REX, WAIT,
fetch/truncation, STOP and exhausted budget reject without commit.

The exact DF allowlist is E0 FNSTSW AX, E9 FUCOMIP and F1 FCOMIP; DB continues
to admit only E2 FNCLEX, E3 FNINIT, E9 FUCOMI and F1 FCOMI. Schema/CLI fixtures
use exact `dfe9dfe0` and `dfe9`; the no-stop/budget-one rejection has an Empty
tag at actual old physical `(TOP+1)&7` with TOP=5. Schema and CLI reject
`dff8dfe0` and prefixed `66`, `48`, and `9b` variants. Coverage spans all TOP ×
ordered relations × 16 initial FSW condition fields, all 64 affected RFLAGS
masks with definedness, TOP wrap, payload preservation, invalid tags/classes,
environment and instruction-boundary atomicity. Exhaustive DF decoding admits
only E0/E9/F1 and rejects all other second bytes and prefixes. Native admission
explicitly denies `Operation::Fucomip` and DF E9.

Normative source: Intel SDM revision 093, Vol.2A,
“FCOMI/FCOMIP/FUCOMI/FUCOMIP—Compare Floating-Point Values,” pp.3-327–3-329
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

#### S2-30ah. Bare bounded `FUCOMP ST(0), ST(1)`

Accept exactly bare `DD E9` with existing `Some(X87State)`. Preflight FCW
`0x037f`, `FSW & 0x80ff == 0`, Valid tags at old physical TOP and
`(TOP+1)&7`, and canonical-Normal binary80 values by calling
`bounded_x87_fcom_result(state, 1)` before any status/tag/TOP mutation. Replace
the condition-code field with greater `0`, less `0x0100`, or equal `0x4000`
through `(old_fsw & !0x4700) | relation`; clear C1 as the bounded profile's
deterministic policy. Then mark only the old physical TOP Empty and advance TOP
to `(old+1)&7`. Preserve raw register payload bytes, old ST(1)'s Valid tag, and
all other state. Set FIP to the instruction-start RIP, advance RIP by two, and
retire exactly once.

Missing state, invalid environment/tag/class, prefixes, REX, WAIT, unsupported
DD bytes, fetch failure/truncation, STOP and exhausted budget reject atomically.
Only DD E9 is added: DD D0..D7 FST, DD D8..DF FSTP and DD E1 FUCOM remain
unchanged; DD E8, DD EA..EF and other unsupported forms remain rejected. The
slice excludes zero, subnormal, infinity, NaN, noncanonical encodings, exception
delivery, stack faults, `#NM` and native execution. FUCOM's QNaN behavior and
general FUCOMP semantics remain outside this canonical-Normal domain.

Literal schema and CLI fixtures cover `dde9`, `dde9dfe0`, and an actual
invalid-old-ST(1)-tag no-stop/budget-one rejection. Interpreter vectors cover
all TOP values, all three comparison relations, all 16 incoming condition
fields, wrap, physical payload sentinels, independent operand tag/class and
environment rejection, and fetch/truncation/STOP/budget atomicity. Exhaustive
DD ModR/M decoding retains neighboring forms; native admission explicitly
denies `Operation::Fucomp` and DD E9.

Normative source: Intel SDM revision 093, Vol.2A,
“FUCOM/FUCOMP/FUCOMPP—Unordered Compare Floating-Point Values,” pp.3-402–3-404
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

#### S2-30ai. Bare bounded `FUCOMPP`

Accept exactly bare `DA E9` with an existing x87 snapshot. Before any mutation,
require FCW `0x037f`, `FSW & 0x80ff == 0`, Valid tags at old physical TOP and
`(TOP+1)&7`, and canonical-Normal binary80 operands through
`bounded_x87_fcom_result(state, 1)`. Replace C0/C1/C2/C3 with greater `0`, less
`0x0100`, or equal `0x4000`; C1=0 for this normal domain is an explicit local
deterministic policy. Empty both old physical tags and set TOP to `(old+2)&7`.
Leave payload bytes and all other state unchanged, set FIP to instruction-start
RIP, advance RIP by two, and retire once.

Reject missing state, invalid environment/tags/classes, every other DA ModR/M,
prefixes, REX, WAIT, fetch failure, truncation, STOP and exhausted budget
atomically. Preserve DE D9 FCOMPP and all existing DA bytes outside E9.
Special values, exceptions, stack faults, `#NM`, and native x87 execution are
excluded.

Tests cover all TOP × relation × 16 initial condition combinations, TOP 6/7
wrap, Empty old ST(2) with arbitrary payload, independent operand tag/class
rejection, environment and execution-boundary rejection. Schema and literal
runner fixtures cover `dae9`, `dae9dfe0`, and a real invalid old physical
ST(1)-tag rejection with no stop RIP and budget one. Native admission denies
DA E9. The explicit FUCOMPP opcode table/prose on pp.3-402–3-404 says two
entries are popped; the pseudocode on p.3-403 depicts one. This slice follows
the opcode table/prose. C1 clearing remains a local policy.

Normative source: Intel SDM revision 093, Vol.2A,
“FUCOM/FUCOMP/FUCOMPP—Unordered Compare Floating-Point Values,” pp.3-402–3-404
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

#### S2-31ad. Extend bounded `FLD m80fp [RAX]` (`DB 28`) to pseudo-denormals

[x] Accept only DB28 pseudo-denormals (E=0,J=1) in the existing all-masked
profile. Preserve raw bytes and E=0, assign Special, add no DE/status flags,
and retain existing gates and single-read/push semantics. Other unsupported
classes remain rejected; native/JIT and arithmetic consumption remain out of
scope. Cover both signs and boundary fractions across TOP×FCW, exponent-one
Normal controls, schema/CLI fixtures and native denial; retain positive
pseudo-infinity as the narrow Unsupported fixture class. See the [S2-31ad
report](../reports/STAGE_2_REPORT.md#s2-31ad--bounded-fld-m80fp-pseudo-denormal-extension).

#### S2-31ae. Extend bounded `FLD m64fp [RAX]` (`DD 00`) to infinities

[x] Accept only signed binary64 infinities (exponent `0x7ff`, fraction zero)
after the existing one-Qword read, converting their sign and fields to
canonical binary80 infinity and assigning Special. Preserve the four FCWs,
`FSW & 0x80dd` gate, Empty destination, canonical48 eight-byte preflight,
sticky DE/PE, C1-clear, TOP, pointers and retirement behavior. Binary64 NaNs
remain post-read Unsupported without CPU commit; unmasked exception delivery
and native admission remain outside the slice. Add TOP×FCW signed vectors,
subnormal-then-infinity sticky-flag coverage, exact DD00 / DD00-DFE0 CLI
fixtures, and a narrow QNaN Unsupported fixture predicate at this stage;
S2-31ag supersedes it by accepting QNaNs and narrowing current rejection to
SNaN. Keep m80fp
subnormal semantics distinct (no DE). See the [S2-31ae report](../reports/STAGE_2_REPORT.md#s2-31ae--bounded-fld-m64fp-signed-infinity-extension).

#### S2-31af. Extend bounded `FLD m32fp [RAX]` (`D9 00`) to infinities

[x] Accept only signed binary32 infinities (exponent 255, fraction zero) after
the existing one-Dword read. Construct canonical binary80 infinity with
integer fields and Special tag; add no flags. Retain the all-masked FCWs,
`FSW & 0x80dd` gate, Empty destination, canonical48 four-byte preflight,
sticky DE/PE and existing push/C1/pointer policy. Keep normal, zero and masked
subnormal behavior unchanged, with NaNs post-read Unsupported and atomic.
Unmasked behavior and JIT/native admission remain outside the slice. Change
the D900-only Unsupported fixture/schema exception from infinity to canonical
QNaN, and add signed infinity TOP×FCW vectors, a subnormal→infinity sticky
status sequence, bare and D900/DFE0 CLI fixtures, schema/CLI rejection
neighbors, and JIT/native denial. See the [S2-31af report](../reports/STAGE_2_REPORT.md#s2-31af--bounded-fld-m32fp-signed-infinity-extension).

#### S2-31ag. Extend bounded `FLD m64fp [RAX]` (`DD 00`) to canonical QNaNs

[x] Accept canonical binary64 QNaNs only (exponent `0x7ff`, fraction bit 51
set, arbitrary remaining payload) after the existing one-Qword read. Convert
by integer fields to binary80 with significand `0x8000000000000000 | (fraction
<< 11)`, exponent `0x7fff`, preserved sign and Special tag. Add no status
flags, including IE, and preserve sticky DE/PE. Retain the four FCWs,
`FSW & 0x80dd` gate, Empty destination, canonical48 preflight and push/C1/TOP/
pointer/retirement behavior. SNaN remains post-read Unsupported with no CPU
commit; m32 NaNs, unmasked delivery and JIT/native execution remain outside
the slice. Switch the exact DD00/budget-one/no-stop Unsupported fixture and
schema predicate from canonical QNaN to canonical SNaN. Cover quiet-only,
low/middle/max payloads and both signs across TOP×FCW, sticky subnormal→QNaN
DE/PE, atomic SNaN rejection, positive bare and negative payload-bearing
DD00/DFE0+FNSTSW CLI fixtures, and explicit JIT/native denial. See the
[S2-31ag report](../reports/STAGE_2_REPORT.md#s2-31ag--bounded-fld-m64fp-canonical-qnan-extension).

#### S2-31ah. Extend bounded `FLD m32fp [RAX]` (`D9 00`) to canonical QNaNs

[x] Accept canonical binary32 QNaNs only (exponent `0xff`, fraction bit 22
set, arbitrary lower payload) after the existing one-Dword read. Construct
binary80 by integer fields with significand
`0x8000000000000000 | (fraction << 40)`, exponent `0x7fff`, preserved sign
and Special tag. Add no exception flags, including IE, and preserve sticky
DE/PE. Retain the four all-masked FCWs, `FSW & 0x80dd` gate, Empty destination,
canonical48 four-byte preflight/read-once, and C1/TOP/pointer/retirement
policy. SNaNs remain post-read Unsupported and atomic; m64 SNaNs, unmasked
delivery, and JIT/native execution remain outside the slice. Change the exact
D900/budget-one/no-stop Unsupported fixture and schema predicate from QNaN
to canonical SNaN. Cover quiet-only, low/middle/max payloads of both signs
across TOP×FCW, subnormal→QNaN sticky DE/PE, atomic SNaN rejection, positive
bare and negative payload-bearing D900/DFE0+FNSTSW CLI fixtures, and native/
JIT denial. See the [S2-31ah report](../reports/STAGE_2_REPORT.md#s2-31ah--bounded-fld-m32fp-canonical-qnan-extension).

#### S2-31ai. Extend bounded `FLD m64fp [RAX]` (`DD 00`) to canonical SNaNs

Accept binary64 signaling NaNs with exponent `0x7ff`, nonzero fraction and
quiet bit 51 clear, for either sign. After the existing canonical48 Qword
preflight and one Qword read, construct binary80 with significand
`0xc000000000000000 | (fraction << 11)`, exponent `0x7fff`, preserved sign,
and Special tag. Add sticky IE only; preserve DE/PE and other flags under the
existing local status policy. Retain the four all-masked FCWs, `FSW & 0x80dd`
entry gate, Empty destination, C1-clear/TOP/pointer/retirement behavior.
Pre-existing IE remains Unsupported before the operand read; subsequent
DD00 loads remain Unsupported while IE is set, and FNCLEX clears it. Keep
m32 SNaNs and raw m80 SNaNs unchanged; unmasked delivery and native/JIT
execution remain outside scope. Replace the exact DD00/budget-one/no-stop
SNaN schema rejection with an environment/boundary predicate. Cover signed
low/middle/maximum payloads across TOP×FCW, sticky DE/PE, exact FSW and
FNSTSW, FNCLEX recovery, memory faults, pre-read gates, CLI fixtures and
native/JIT denial. See the [S2-31ai report](../reports/STAGE_2_REPORT.md#s2-31ai--bounded-fld-m64fp-canonical-snan-extension).

#### S2-31aj. Extend bounded `FLD m32fp [RAX]` (`D9 00`) to canonical SNaNs

Accept binary32 signaling NaNs with exponent `0xff`, nonzero fraction and
quiet bit 22 clear, for either sign. After the existing canonical48 Dword
preflight and one Dword read, construct binary80 with significand
`0xc000000000000000 | (u64::from(fraction) << 40)`, exponent `0x7fff`,
preserved sign, and Special tag. Add sticky IE only; preserve DE/PE. Retain
the four all-masked FCWs, D900-local `FSW & 0x80dd` gate, Empty destination,
C1/TOP/pointer/retirement behavior. Existing zero, Normal, subnormal,
infinity and QNaN paths remain unchanged. Existing IE or an unmasked FCW
remains Unsupported before the read; a subsequent D900 load remains blocked
until FNCLEX clears status. Keep m64 and raw m80 behavior unchanged; unmasked
delivery and native/JIT execution remain outside scope. Replace the exact
D900/budget-one/no-stop SNaN-value exception with an environment/boundary
predicate. Cover signed low/middle/maximum SNaNs across TOP×FCW, exact
quieted-QNaN literals, sticky DE/PE, pre-read gates, memory faults, FNCLEX
recovery, CLI/FNSTSW fixtures and native/JIT denial. See the [S2-31aj
report](../reports/STAGE_2_REPORT.md#s2-31aj--bounded-fld-m32fp-canonical-snan-extension).

#### S2-31ak. Preserve sticky IE/DE/PE across exact m32/m64 FLD loads

Relax only the local D900 and DD00 status masks from `0x80dd` to `0x80dc`.
This permits all combinations of sticky IE, DE and PE while continuing to
reject ZE, OE, UE, SF, ES and B before the operand read. Keep the four admitted
FCWs, Empty destination, canonical48 range validation and one raw read. Loads
preserve admitted sticky flags; a signaling NaN adds IE and a nonzero
subnormal adds DE. Do not change FILD, m80, stores, arithmetic, or any other
x87 status gate. Native/JIT admission and unmasked delivery remain out of
scope.

Tests span all eight IE/DE/PE combinations across source classes, FCWs and
TOP values, and exercise same-width and cross-width sequences, repeated SNaNs,
second-load fault atomicity, each forbidden bit, occupied destinations,
unmasked FCWs and FNCLEX recovery. The exact D900/DD00 budget-one/no-stop
schema cases now require a forbidden status bit and use no source-class
exception. Existing single-load CLI/FNSTSW fixtures continue to observe IE.

#### S2-31al. Store signed binary80 zero through bounded m32 FST/FSTP

Extend only exact `D9 10` (`FST m32fp [RAX]`) and `D9 18`
(`FSTP m32fp [RAX]`) so the old physical TOP source may be canonical binary80
zero (`exponent=0`, `significand=0`) with tag Zero. Preserve its sign as
binary32 `+0`/`-0`. Retain the four admitted FCWs and `FSW & 0x80df == 0`
store gate; sticky PE may remain set. Exact zero adds no status flags and
clears C1 under the existing store policy.

Keep the full canonical48 Dword check and one atomic Dword write with no data
read. FST preserves TOP, all tags and payloads; FSTP empties old physical TOP
and advances TOP only after the write succeeds. Reject mismatched tags and
payloads before access; retain Normal conversion and other widths unchanged.
Cover both signs across TOP×FCW×incoming PE/C1, FLD-zero compositions, precise
source mismatches, store faults and range failures, literal CLI/FNSTSW fixtures,
and explicit native/JIT denial.

#### S2-31am. Store signed binary80 zero through bounded m64 FST/FSTP

Extend only exact `DD 10` (`FST m64fp [RAX]`) and `DD 18`
(`FSTP m64fp [RAX]`) so the old physical TOP source may be canonical binary80
zero (`exponent=0`, `significand=0`) with tag Zero. Preserve its sign as
binary64 `+0`/`-0`. Retain the four admitted FCWs and `FSW & 0x80df == 0`
store gate; sticky PE may remain set. Exact zero adds no flags and clears C1
under the existing exact-result policy.

Keep the full canonical48 Qword check and one atomic Qword write with no data
read. FST preserves TOP, all tags and payloads; FSTP empties old physical TOP
and advances TOP only after the write succeeds. Reject mismatched tags and
payloads before access; preserve Normal conversion and its local exponent
limits. Cover both signs across TOP×FCW×incoming PE/C1, cross-width FLD-zero
compositions, source/environment rejection, exact memory boundaries and store
faults, literal CLI/FNSTSW fixtures, and explicit native/JIT denial.


#### S2-31an. Store canonical signed binary80 zero through bounded FSTP m80fp

Extend only exact `DB 38` (`FSTP m80fp [RAX]`) so physical TOP may be either
the existing Valid + canonical-Normal pair or a Zero tag + canonical binary80
Zero (`exponent=0`, all 64 significand bits zero). Preserve all ten source
bytes, including the sign (`+0` `00000000000000000000`, `-0`
`00000000000000000080`). Do not round, normalize, or create status flags.
Retain the four FCWs and `FSW & 0x80df` gate; preserve PE and C0/C2/C3, and
clear C1 under the existing local policy.

Preflight the complete canonical48 ten-byte range and perform one atomic write
with no data read. Pop/empty old physical TOP and advance TOP only after a
successful write. Preserve all payloads and existing pointer/retirement
behavior. Reject malformed tag/payload pairs before memory access. Cover both
signs across TOP×FCW×PE/C1, canaries, cross-width zero-load compositions, wrap,
source/environment rejection, store faults and range edges, literal CLI/FNSTSW
fixtures, and native/JIT denial.


#### S2-31ao. Store canonical signed binary80 infinity through bounded FSTP m80fp

Extend only exact `DB 38` (`FSTP m80fp [RAX]`) so physical TOP may also be a
Special tag with canonical binary80 infinity (`exponent=0x7fff`, explicit
integer bit set, fraction zero), either sign. Preserve all ten bytes exactly
(`+∞` `0000000000000080ff7f`, `-∞` `0000000000000080ffff`). Keep the existing
Valid+Normal and Zero+Zero source pairs. Do not round, normalize, or set flags;
retain four FCWs, `FSW & 0x80df`, PE/C0/C2/C3 and C1-clear behavior.

Keep full canonical48 ten-byte preflight, one atomic write/no read, and pop only
after successful write. Cover signed infinity across TOP×FCW×PE/C1, canaries,
wrap, m32/m64/m80 infinity-load compositions, pseudo-infinity/NaN/other-class
and mismatched-tag rejection before access, every forbidden status gate, normal
and zero regressions, write/range atomicity, literal CLI/FNSTSW fixtures, and
native/JIT denial.


#### S2-31ap. Store canonical signed binary80 infinity through bounded m64 FST/FSTP

Extend only exact bare `DD 10` (`FST m64fp [RAX]`) and `DD 18` (`FSTP
m64fp [RAX]`) so physical TOP may be Special-tagged canonical binary80
infinity (exponent `0x7fff`, integer bit one, fraction zero), either sign.
Convert by integer fields to binary64 `+∞` (`000000000000f07f`) or `-∞`
(`000000000000f0ff`) with exact-result indicators false. Do not use host
floating point or set exception flags. Preserve PE, retain the existing FCWs and
`FSW & 0x80df` gate, clear C1, and preserve the existing C0/C2/C3 policy.

Preflight the full canonical48 Qword range; perform exactly one atomic write and
no data read. FST preserves TOP/tags/payloads; FSTP pops only after successful
write. Keep Normal conversion limits and signed-zero behavior unchanged. Cover
both signs/opcodes over TOP×FCW×PE/C1, sentinels/wrap, all-width FLD-to-store
compositions, invalid neighboring classes and tag mismatches, write/range
atomicity, normal/zero regressions, literal CLI/FNSTSW fixtures, and
native/JIT denial.

#### S2-31aq. Store canonical signed binary80 infinity through bounded m32 FST/FSTP

Extend only exact `D9 10` (`FST m32fp [RAX]`) and `D9 18`
(`FSTP m32fp [RAX]`) so physical TOP may also be a Special-tagged canonical
binary80 infinity (exponent `0x7fff`, explicit integer bit one, zero fraction).
Convert using integer fields to binary32 `+∞` (`0000807f`) or `-∞`
(`000080ff`), with exact-result indicators false and no new flags. Preserve PE
and the existing C1/C0/C2/C3 policy. Retain the four FCWs and
`FSW & 0x80df == 0` gate.

Preflight the complete canonical48 Dword range and perform one atomic Dword
write with no data read. FST preserves TOP, tags, and payloads; FSTP pops only
after successful write. Keep Normal and signed-zero paths unchanged. Reject
pseudo-infinity, NaNs, other Special classes, and mismatched tags before
access. Cover signed infinity across both opcodes and TOP×FCW×PE/C1,
cross-width FLD compositions, boundaries/faults, CLI/FNSTSW fixtures, and
native/JIT denial.

<a id="s2-31ar-store-canonical-binary80-quietnan-through-bounded-fstp-m80fp"></a>

#### S2-31ar. Store canonical binary80 QuietNaN through bounded FSTP m80fp

Extend only exact `DB 38` (`FSTP m80fp [RAX]`) so physical TOP may also be a
Special-tagged canonical binary80 QuietNaN: exponent `0x7fff`, explicit
integer bit one, quiet fraction bit one, and any remaining payload/sign. Copy
the original ten bytes unchanged; do not normalize, round, use host floating
point, or add status flags. Clear C1 and preserve PE and C0/C2/C3 under the
existing raw-store policy, retaining the four FCWs and `FSW & 0x80df == 0` gate.

Preflight the full canonical48 ten-byte range and perform one atomic write/no
read; pop and advance TOP only after successful write. Preserve all payloads,
pointers, and retirement behavior. Keep Valid+Normal, Zero+Zero, and
Special+Infinity paths. Reject SNaN, pseudo-QNaN, pseudo-SNaN, pseudo-infinity,
finite special classes, and mismatched tags before access. Cover both signs,
quiet-only/low/middle/max payloads including negative indefinite, TOP×FCW×PE/C1,
cross-width FLD compositions, faults/range, CLI/FNSTSW, and native/JIT denial.

<a id="s2-31as-store-canonical-binary80-signalingnan-through-bounded-fstp-m80fp"></a>

#### S2-31as. Store canonical binary80 SignalingNaN through bounded FSTP m80fp

Extend only exact `DB 38` so physical TOP may also be a Special-tagged
canonical binary80 SignalingNaN: exponent `0x7fff`, explicit integer bit one,
quiet bit zero, and a nonzero remaining fraction, with either sign. Copy all
ten bytes unchanged, retaining the signaling bit and payload; do not quiet,
use host floating point, or set IE or any other flag. Keep the four FCWs,
`FSW & 0x80df == 0`, C1-clear, and PE/C0/C2/C3-preserve store policy.

Preflight the full canonical48 ten-byte range and perform one atomic write/no
read; pop and advance TOP only after success. Keep existing Normal, Zero,
Infinity, and QuietNaN paths. Reject pseudo-SNaNs and other unsupported
classes or mismatched tags before access. Cover both signs and minimum/low/
middle/maximum payloads across TOP×FCW×PE/C1, raw m80 FLD-to-store composition,
failure atomicity, literal DB38/DB38 DFE0 fixtures, and native/JIT denial.
This does not implement general exception delivery or physical x86 acceptance.

<a id="s2-31at-store-canonical-binary80-subnormal-through-bounded-fstp-m80fp"></a>

#### S2-31at. Store canonical binary80 subnormal through bounded FSTP m80fp

Extend only exact `DB 38` so physical TOP may also be a Special-tagged
canonical binary80 subnormal: exponent zero, explicit integer bit zero, and a
nonzero fraction, with either sign. Copy all ten bytes unchanged; do not
normalize, flush, round, use host floating point, or set DE/UE/PE or other new
flags. Keep the four FCWs, `FSW & 0x80df == 0`, C1-clear, and PE/C0/C2/C3-
preserve store policy.

Preflight the full canonical48 ten-byte range and perform one atomic write/no
read; pop and advance TOP only after success. Preserve Valid+Normal,
Zero+Zero, and Special-tagged Infinity, QuietNaN, and SignalingNaN paths.
Reject pseudo-denormals and other unsupported encodings or mismatched tags
before access. Cover both signs and payload boundaries/interior across
TOP×FCW×PE/C1, m80 subnormal FLD-to-store composition, write/range atomicity,
literal DB38/DB38 DFE0 fixtures, and native/JIT denial. This does not implement
general exception delivery or physical x86 acceptance.


<a id="s2-31au-store-canonical-binary80-quietnan-through-bounded-fstfstp-m64fp"></a>

#### S2-31au. Store canonical binary80 QuietNaN through bounded m64 FST/FSTP

Extend only exact `DD 10` (`FST m64fp [RAX]`) and `DD 18` (`FSTP
m64fp [RAX]`) so a Special-tagged canonical binary80 QuietNaN is converted
by integer fields to binary64. Preserve sign and the upper 52 fraction bits;
truncate the lower 11 binary80 payload bits without rounding. Set no new flags,
preserve PE, and clear C1 under the existing exact-store policy. Keep four
FCWs and `FSW & 0x80df == 0`.

Check the complete canonical48 Qword range, issue exactly one atomic write/no
read, and pop only after a successful DD18 write. DD10 preserves TOP, tags and
payloads. Keep Normal, signed-zero and infinity paths unchanged; reject SNaNs,
pseudo-NaNs, other special classes and mismatched tags before access. Cover
both signs/opcodes across TOP×FCW×PE/C1 and payload boundaries, cross-width
FLD-to-store composition, fault/range atomicity, CLI/FNSTSW fixtures, and
native/JIT denial.


<a id="s2-31av-store-canonical-binary80-quietnan-through-bounded-fstfstp-m32fp"></a>

#### S2-31av. Store canonical binary80 QuietNaN through bounded m32 FST/FSTP

Extend only exact `D9 10` (`FST m32fp [RAX]`) and `D9 18` (`FSTP
m32fp [RAX]`) so a Special-tagged canonical binary80 QuietNaN converts to
binary32 by integer fields. Preserve sign and the upper 23 fraction bits;
truncate the lower 40 bits without rounding. Add no flags and preserve PE; clear
C1 under the existing exact-store policy. Keep the current four FCWs and
`FSW & 0x80df == 0` gate.

Preflight the complete canonical48 Dword range, then issue exactly one atomic
write/no read; D918 pops only after successful write while D910 preserves the
stack. Keep Normal, signed-zero and Infinity behavior. Reject SNaNs, pseudo-NaNs,
other special classes and mismatched tags before access. Cover signed values and
payload boundaries across TOP×FCW×PE/C1, cross-width FLD-to-store composition,
fault/range atomicity, CLI/FNSTSW fixtures, and native/JIT denial.


<a id="s2-31aw-store-canonical-binary80-signalingnan-through-bounded-fstfstp-m64fp"></a>

#### S2-31aw. Store canonical binary80 SignalingNaN through bounded m64 FST/FSTP

Extend only exact `DD 10` (`FST m64fp [RAX]`) and `DD 18` (`FSTP
m64fp [RAX]`) for a Special-tagged canonical binary80 SignalingNaN. Convert by
integer fields, preserving the sign and upper 51 payload bits while setting the
binary64 quiet bit; discard the lower 11 bits without PE. Set sticky IE only
after the single atomic Qword write succeeds; preserve PE and leave ES/B/SF
clear. Keep the current FCWs, `FSW & 0x80df == 0` entry gate, C1-clear policy,
and FST/FSTP state transition rules. The source's ten bytes remain unchanged;
DD18 pops only after a successful write.

Preflight the complete canonical48 Qword range and perform one write/no read.
Reject pseudo-SNaNs, QNaNs with mismatched tags, pseudo-infinities, other
unsupported encodings, occupied or Empty source tags, and forbidden environment
bits before memory access. Cover both signs/opcodes across TOP×FCW×PE/C1,
payload bits around the 11-bit truncation boundary, QNaN/SNaN differential,
write/range atomicity, IE/FNSTSW/FNCLEX recovery, CLI fixtures and native/JIT
denial. Intel SDM revision 093 Vol. 2A's FST/FSTP entry defines NaN identity
preservation on narrowing stores and the masked invalid-operation behavior is
described in Vol. 1 §8.5.1 ([official Vol. 2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=505),
[official Vol. 1 PDF](https://cdrdv2.intel.com/v1/dl/getContent/671436#page=232)).


<a id="s2-31ay-admit-sticky-ie-in-bounded-m32-m64-fst-fstp"></a>

#### S2-31ay. Admit sticky IE in bounded m32/m64 FST/FSTP

Relax only the status gate in the exact D910/D918 and DD10/DD18 conversion
helpers from `FSW & 0x80df == 0` to `FSW & 0x80de == 0`. This admits an
already-set masked IE while continuing to reject ZE, DE, OE, UE, SF, ES, and B;
the FCW whitelist, source classes/tags, range validation, and all other x87
instruction gates remain unchanged. Successful stores preserve incoming IE and
PE, set IE for a signaling NaN and PE for an inexact finite conversion only
after the atomic write succeeds, and keep ES/B clear. FSTP still pops only
after success. Add repeated-store and SNaN-load-to-quiet-store sequences with
FNSTSW/FNCLEX, plus fault, class, tag, FCW, forbidden-status, CLI, and JIT/native
admission checks. FNCLEX remains available to clear status flags but is no
longer required between otherwise admitted m32/m64 stores after SNaN.


<a id="s2-31az-admit-sticky-de-in-bounded-m32-m64-fst-fstp"></a>

#### S2-31az. Admit sticky DE in bounded m32/m64 FST/FSTP

Relax only the status predicates in D910/D918 and DD10/DD18 from
`FSW & 0x80de == 0` to `FSW & 0x80dc == 0`. This admits existing masked IE, DE,
and PE while continuing to reject ZE, OE, UE, SF, ES, and B. Keep the FCW
whitelist, source classes/tags, finite-result limits, addressing and every
unrelated x87 status gate unchanged. Preserve incoming IE/DE/PE; successful
SNaN conversion adds IE and an inexact finite conversion adds PE only after the
atomic write, with no newly generated DE. The full-range write must succeed
before status, pointers, RIP, or FSTP pop state commits.

Cover all eight incoming IE/DE/PE combinations, subnormal-result rejection,
SNaN and inexact-normal stores with DE already set, and exact binary32
subnormal FLD-to-binary64 FST/FSTP composition for both signs. Include
FNSTSW/FNCLEX, repeated stores, forbidden environment controls, full-state
fault atomicity, CLI fixtures, and JIT/native denial. Native runtime and
physical acceptance remain deferred to the final combined S2-31 substage under
ISSUE-019.

<a id="s2-31br-admit-sticky-status-on-bounded-m32m64-stores"></a>

#### S2-31br. Admit sticky IE/DE/OE/UE/PE on bounded m32/m64 stores

Only bounded D910/D918 and DD10/DD18 widen their status gate from
`FSW & 0x80cc == 0` to `FSW & 0x80c4 == 0`, admitting prior IE, DE, OE, UE and
PE while continuing to reject ZE, SF, ES and B. Preserve incoming sticky
flags; generate conversion-specific flags only after a successful write. OE
alone must not add PE, and old C1 is recomputed from the current conversion.
Keep FCW/source/addressing gates, FST/FSTP stack and pointer behavior, and all
other x87 paths unchanged. Cover all 32 sticky combinations across
representative result classes, signs, RC, TOP, both widths/opcodes, overflow
re-entry without FNCLEX, FNSTSW/FNCLEX, write/range faults, CLI fixtures, and
JIT/native denial.

<a id="s2-31ba-admit-sticky-ie-de-to-raw-binary80-fstp"></a>

#### S2-31ba. Admit sticky IE/DE on exact raw DB38 FSTP m80fp

Relax only `bounded_x87_m80fp_source` from `FSW & 0x80df == 0` to
`FSW & 0x80dc == 0`, admitting masked sticky IE and DE alongside PE while
continuing to reject ZE, OE, UE, SF, ES and B. Keep the four FCWs and all
existing tag/class pairs unchanged: Valid+Normal, Zero+Zero, and
Special+Infinity/QNaN/SNaN/Subnormal. Pseudo-denormals, unnormals and tag/class
mismatches remain Unsupported before data access.

DB38 copies all ten bytes unchanged, generates no flags, preserves incoming
IE/DE/PE and C0/C2/C3, and clears C1. Validate the full canonical48 ten-byte
range and complete the single atomic write before changing status, pointers,
RIP, or popping old TOP. Cover all eight sticky combinations across source
classes/signs, FCWs and TOP wrap; subnormal m32/m64 FLD→DB38 retains DE without
UE/PE, m32/m64 SNaN FLD→DB38 retains the quieted QNaN and IE, and raw m80
SNaN FLD→narrow FST→DB38 retains its signaling bytes while carrying IE.
Include forbidden environment gates, full-state write/range fault atomicity,
FNSTSW, FNCLEX, literal interpreter fixtures, and explicit JIT/native denial.
Native runtime and physical acceptance remain deferred under ISSUE-019.


<a id="s2-31bb-finite-exponent-127-bounded-m32-stores"></a>

#### S2-31bb. Extend finite binary32 stores through unbiased exponent 127

Extend only the finite Normal exponent interval in the exact D910/D918 m32
conversion helper from `-126..=126` to `-126..=127`. Keep the existing integer
significand rounding for all four FCWs. Normalize a rounded significand carry
before the final exponent check: carry from 126 to 127 is valid; carry from 127
to 128 returns Unsupported before memory access. A source already at exponent
128 also remains Unsupported. Do not synthesize masked overflow or set OE.

Keep the minimum exponent, subnormal-result rejection, FCW and FSW gates,
accepted source classes/tags, addressing, atomic write, status and FSTP pop
ordering unchanged. Cover maximum finite values of both signs, both opcodes,
all TOPs and FCWs, ties and directed rounding, carry to 127 versus 128, source
exponent 128, all incoming IE/DE/PE combinations, fault/range atomicity and
separate maximum-finite FLD and store literal fixtures. Keep native/JIT
admission closed; native runtime and physical acceptance remain deferred under
ISSUE-019. Validation passed: 16 focused `od-isa` m32 store tests, the CLI
fixture test, the JIT-denial fixture test, 786 workspace tests, workspace
`cargo check`, Linux x86_64 `cargo check --all-targets`, and `cargo fmt --all
-- --check`. Linux x86 test binaries could not be linked on this macOS host;
`tools/check_layout.py` reports the pre-existing unmapped `.serena` root, which
was left untouched.


<a id="s2-31ax-store-canonical-binary80-signalingnan-through-bounded-fstfstp-m32fp"></a>

#### S2-31ax. Store canonical binary80 SignalingNaN through bounded m32 FST/FSTP

Extend only exact `D9 10` (`FST m32fp [RAX]`) and `D9 18` (`FSTP
m32fp [RAX]`) for a Special-tagged canonical binary80 SignalingNaN. Convert by
integer fields: preserve sign, quiet the binary32 result, retain the upper 22
payload bits, and truncate lower 40 bits without rounding or PE. Add sticky IE
only after the atomic Dword write succeeds; preserve PE and source bytes. Keep
the existing four FCWs, `FSW & 0x80df == 0` gate, and C1-clear/status policy.

Preflight the complete canonical48 Dword range and perform one atomic write/no
read. D910 preserves TOP/tags/payloads; D918 pops only after successful write.
Keep Normal, Zero, Infinity and QNaN paths unchanged. Reject pseudo-SNaNs, other
special classes, mismatched tags, and forbidden environment bits before access.
Cover signs/opcodes across TOP×FCW×PE/C1, source payload bits 39/40, QNaN/SNaN
status differential, write/range atomicity, IE/FNSTSW/FNCLEX recovery, literal
CLI fixtures, and native/JIT denial. General unmasked exception delivery,
native runtime admission and physical acceptance remain outside this slice.
The masked invalid-operation result follows Intel SDM revision 093 Vol. 2A
FST/FSTP and Vol. 1 §8.5.1 ([official Vol. 2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=505),
[official Vol. 1 PDF](https://cdrdv2.intel.com/v1/dl/getContent/671436#page=232)).


<a id="s2-31bc-finite-exponent-1023-bounded-m64-stores"></a>

#### S2-31bc. Extend finite binary64 stores through unbiased exponent 1023

Extend only the finite Normal exponent interval in the exact DD10/DD18 m64
conversion helper from `-1022..=1022` to `-1022..=1023`. Keep integer
significand rounding and all four FCWs. Normalize a rounded significand carry
before the final exponent check: carry from 1022 to 1023 is valid; carry from
1023 to 1024 returns Unsupported before memory access. A source already at
exponent 1024 also remains Unsupported. Do not synthesize masked overflow or
set OE. At this S2-31bc stage, keep the minimum exponent and subnormal-result
rejection; S2-31be later adds exact subnormal outputs. Preserve status/control
gates, source classes/tags, addressing, status and FSTP pop ordering unchanged.

Cover both signs and opcodes across TOPs, all FCWs and incoming IE/DE/PE;
maximum finite, nearest/directed tie neighbors, carry to valid 1023 versus
rejected 1024, source exponent 1024, below-minimum rejection, and write/range
fault atomicity. Add separate maximum-finite m64 FLD, FST and FSTP/FNSTSW
fixtures, plus CLI/schema reachability and explicit JIT/native denial. Native
runtime and physical acceptance remain deferred under ISSUE-019. Validation
passed: 31 focused `od-isa` m64fp tests, the CLI fixture test, the JIT-denial
fixture test, 790 workspace tests, workspace `cargo check`, Linux x86_64
`cargo check --all-targets`, and `cargo fmt --all -- --check`. Linux x86 test
binaries could not be linked on this macOS host; layout checking still reports
the pre-existing unmapped `.serena` root, left untouched.


<a id="s2-31bd-exact-binary32-subnormal-results-through-m32-stores"></a>

#### S2-31bd. Store exact binary32 subnormal results through bounded m32 FST/FSTP

Extend only exact D910/D918 m32 stores for a Valid-tagged canonical binary80
Normal source with unbiased exponent `-149..=-127`. Convert the 64-bit
significand by integer downshift `shift = -exponent - 86` (41..63); accept
only when every discarded bit is zero and the resulting fraction is in
`1..=0x7fffff`. Preserve sign. This path is independent of the FCW rounding
mode and adds no DE, UE or PE. Inexact subnormal results, exponent below -149,
other tags/classes and malformed encodings remain Unsupported before memory
access. Keep normal conversion, minimum-normal behavior and all environment,
addressing, store/pop and native/JIT gates unchanged.

Cover minimum, adjacent and maximum subnormal fractions, both signs, both
opcodes, all TOPs/FCWs and incoming IE/DE/PE. Exercise every discarded-shift
boundary, 1.5 minimum subnormal, the half-step above maximum subnormal,
exponent -150 and minimum normal. Include D900 subnormal FLD→D910/FSTP
composition proving DE retention without PE, full-state write/range fault
atomicity, CLI fixtures with FNSTSW, and explicit JIT/native denial. Native
runtime and physical acceptance remain deferred under ISSUE-019.

S2-31bf extends this exact-only stage with masked, rounded inexact results
that remain nonzero binary32 subnormals.


<a id="s2-31be-exact-binary64-subnormal-results-through-m64-stores"></a>

#### S2-31be. Store exact binary64 subnormal results through bounded m64 FST/FSTP

Extend only exact DD10/DD18 m64 stores for a Valid-tagged canonical binary80
Normal source with unbiased exponent `-1074..=-1023`. Convert by integer
downshift `shift = -exponent - 1011` (12..63); accept only if every discarded
significand bit is zero and the result fraction is `1..=0x000fffffffffffff`.
Preserve sign. This exact path is independent of FCW rounding mode and adds no
DE, UE or PE. Inexact results, exponents below -1074, other tags/classes and
malformed encodings remain Unsupported before data access. Keep the existing
normal conversion, minimum-normal boundary, FCW/FSW gates, addressing, atomic
Qword write, status policy and FSTP pop-after-write ordering.

Cover minimum and adjacent subnormal, maximum subnormal, nontrivial fractions
and minimum-normal control across both signs/opcodes, all TOPs/FCWs and all
incoming IE/DE/PE combinations. Exercise every shift with nonzero discarded
bits, 1.5 minimum subnormal, half-step above maximum subnormal and exponent
-1075 rejection. Include DD00 subnormal FLD→DD10/DD18 compositions proving exact
recovery and DE retention without UE/PE, write/range-fault atomicity, signed
CLI fixtures including FNSTSW, and explicit JIT/native denial. `cargo test --workspace --offline` passed (801 tests);
workspace and Linux x86_64 all-target checks, and `cargo fmt --all -- --check`
passed. The Linux-only native admission test was compile-checked but not run on
macOS. Layout checking reports the pre-existing unmapped `.serena/` directory,
which was preserved. Native runtime and physical acceptance remain deferred
under ISSUE-019; independent review is pending.


<a id="s2-31bf-masked-inexact-binary32-subnormal-results-through-m32-stores"></a>

#### S2-31bf. Round masked inexact binary32 subnormal results through bounded m32 FST/FSTP

Extend only D9 10/D9 18 for Valid-tagged canonical Normal binary80 sources
with unbiased exponent `-149..=-127`. Convert with integer quotient/remainder
using `shift = -exponent - 86` (41..63), then round by FCW with sign-aware
directed modes. Accept only rounded fractions `1..=0x7fffff`; reject a result
rounded to zero, a source below -149, or a carry/promotion to minimum normal
before memory access. Preserve the existing exact-subnormal behavior.

For accepted inexact subnormal results, after the atomic Dword write add sticky
UE+PE (`0x30`), with no DE. C1 reports whether rounding increased magnitude.
Do not commit flags, instruction/data pointers, or the FSTP pop before storage
succeeds. Keep FCW and `FSW & 0x80dc == 0` gates; this rejects a pre-existing UE.
Other widths, operations, tags/classes and JIT/native admission remain closed.

Cover 1.5 and 2.5 times minimum subnormal, both signs, all RC modes and tie
neighbors; all shifts 41..63; maximum-subnormal directed rounding versus
minimum-normal promotion rejection; both opcodes/TOPs, incoming IE/DE/PE,
FNSTSW and FNCLEX, write/range-fault atomicity, forbidden classes/tags,
CLI fixtures, and JIT/native denial. Validation passed: 21 focused m32fp ISA
tests, CLI fixture and JIT-denial tests, 806 workspace tests, workspace and
Linux x86_64 all-target checks, and formatting. Layout checking reports the
pre-existing unmapped `.serena/` directory, left untouched. Linux-only native
admission was compile-checked but not executed. Independent review is pending;
native runtime and physical acceptance remain deferred under ISSUE-019.


<a id="s2-31bg-masked-inexact-binary64-subnormal-results-through-m64-stores"></a>

#### S2-31bg. Round masked inexact binary64 subnormal results through bounded m64 FST/FSTP

Extend only DD10/DD18 for Valid-tagged canonical Normal binary80 sources with
unbiased exponent `-1074..=-1023`. Compute `shift = -exponent - 1011` (12..63),
then use integer quotient/remainder and the accepted FCW's sign-aware rounding
mode. Preserve exact-subnormal status behavior. For inexact results, accept
only fractions `1..=0x000fffffffffffff`; reject zero, values below -1074 and
carry/promotion to minimum normal before data access.

After a successful atomic Qword write, accepted inexact stores add sticky UE+PE
(`0x0030`), add no DE, and set C1 only when magnitude increases. Retain the
FCW whitelist and `FSW & 0x80dc == 0` gate, including rejection of pre-existing
UE. Preserve the normal path and do not commit flags, FIP/DIP, RIP or FSTP pop
before the write succeeds. Other operations and JIT/native admission stay
closed.

Cover 1.5/2.5 minimum-subnormal ties and adjacent values across signs and RC;
every shift 12..63; maximum-subnormal directed rounding and min-normal
promotion rejection; both opcodes, every TOP and all incoming IE/DE/PE
combinations; below-minimum and UE-gate rejection; FNSTSW/FNCLEX; write/range
atomicity; literal CLI fixtures and JIT/native denial. Validation passed:
25 focused m64fp ISA tests, CLI fixture and JIT denial. Full workspace and
Linux x86_64 checks are recorded in the Stage 2 report. Physical acceptance
remains deferred under ISSUE-019.


#### S2-31bh. Admit sticky UE in bounded binary32/binary64 stores

Relax only the FSW gates in the bounded D9 10/D9 18 and DD 10/DD 18 result
helpers from `0x80dc` to `0x80cc`. This admits incoming masked IE/DE/UE/PE in any
combination while continuing to reject ZE/OE/SF/ES/B. UE alone does not create
PE; conversion-generated IE, PE, or UE+PE is accumulated only after a successful
atomic Dword/Qword write. Keep FCWs, sources, exact/inexact conversion behavior,
and the FLD/m80 gates unchanged.

Cover all 16 sticky-flag combinations across classes, exact/inexact results,
rounding modes, opcodes, and TOP values. Exercise repeated underflow, the m32
underflow-to-exact-m64 composition, FNSTSW/FNCLEX, write/range fault atomicity,
forbidden status/control values, and CLI/JIT/native admission. Preserve the
existing ISSUE-019 native-runtime and physical-acceptance deferral.


#### S2-31bi. Admit sticky UE in raw binary80 FSTP m80fp

Relax only `bounded_x87_m80fp_source` from `FSW & 0x80dc == 0` to
`FSW & 0x80cc == 0`, admitting masked IE/DE/UE/PE in any combination while
rejecting ZE/OE/SF/ES/B. Preserve the existing FCWs, source tag/class pairs,
ten raw bytes, and no-generated-flags behavior. In particular, UE alone adds
no IE/PE and the raw source is not normalized or quieted. Perform the single
atomic ten-byte write before status/pointer/RIP commit and FSTP pop.

Cover all 16 sticky combinations across classes/signs/FCWs/TOP, UE-only raw
SNaN preservation, m32/m64 underflow FST followed by raw m80 FSTP from the
unchanged source, FNSTSW/FNCLEX, forbidden statuses, FCW controls, fault/range
atomicity, CLI fixtures, and JIT/native denial. Native runtime and physical
acceptance remain deferred under ISSUE-019.


<a id="s2-31ci-admit-sticky-ie-on-bounded-integer-stores"></a>

#### S2-31ci. Admit sticky IE on bounded integer stores

Only the shared entry gate for bare DB 10, DB 18, DF 10, DF 18 and DF 38
changes from FSW & 0x80df == 0 to FSW & 0x80de == 0. This admits incoming IE
and PE together or independently while continuing to reject DE, ZE, OE, UE,
SF, ES and B. FCWs, source/tag classes, finite limits, addressing, and all
FILD/FRNDINT/floating-store gates remain unchanged. Incoming IE/PE stay sticky;
only the current conversion may add flags, and historical IE does not force
integer-indefinite or synthesize PE. C1 is recomputed from the current
operation. Preserve C0/C2/C3 and source bytes, and commit flags, pointers, RIP,
TOP and tag changes only after the atomic store succeeds.

Cover the four incoming IE/PE combinations across both signs, zero, exact and
inexact Normal, fractional, infinity, QNaN and SNaN sources, all five forms,
four RCs, every TOP and both incoming C1 states (17,920 combinations). Also
cover masked-invalid FIST followed by FISTP without FNCLEX, exact finite store
under IE with PE clear, IE+PE write/range atomicity and IE combined with each
forbidden status bit across all forms, FNSTSW/FNCLEX, CLI fixtures and
JIT/native denial. Native runtime and physical acceptance remain deferred under
ISSUE-019.


<a id="s2-31cj-masked-finite-overflow-to-m32-fistfstp"></a>

#### S2-31cj. Masked finite overflow to m32 `FIST`/`FISTP`

Only canonical Valid-tagged binary80 Normal inputs to bare `DB 10` and `DB 18`
gain signed-i32 out-of-range conversion. Round under the admitted RC first;
rounded values outside `[-2^31, 2^31-1]` store integer-indefinite
`0x80000000`, then add IE and clear C1 after a successful Dword write. Do not
add PE for an invalid conversion. For exponent `e >= 32`, classify directly as
invalid. For `0 <= e <= 31`, use the exact significand quotient, guard/sticky
tail and RC before testing signed bounds; negative-exponent fractions retain
their comparison-based rounding path. This preserves out-of-range real values
that round back into range. Keep m16, subnormal, malformed and unmasked paths
unchanged in this m32 slice; S2-31ck later extends m16 and S2-31cl later adds
the m64 overflow path.

Cover signed limits, representable neighbors around both half-integer
thresholds and ties, all RCs/signs/opcodes/TOP and incoming IE/PE, prior C1,
large exponents 32/63/64/16383 with minimum/mixed/maximum significands, and
exact `-2^31` versus the same-bit integer-indefinite result. Check invalid
fractional input adds IE without PE; atomic denied-write/range faults and raw
source preservation; existing zero/fraction/special regressions; FNSTSW/FNCLEX,
literal CLI fixtures, and JIT/native denial. Native runtime and physical
acceptance remain deferred under ISSUE-019.


<a id="s2-31ck-masked-finite-overflow-to-m16-fistfstp"></a>

#### S2-31ck. Masked finite overflow to m16 `FIST`/`FISTP`

Only canonical Valid-tagged binary80 Normal inputs to bare `DF 10` and `DF 18`
gain signed-i16 out-of-range conversion. Round under the admitted RC first;
rounded values outside `[-2^15, 2^15-1]` store Word integer-indefinite
`0x8000`, then add IE and clear C1 after a successful Word write. Do not add PE
for an invalid conversion. For exponent `e >= 16`, classify directly as
invalid; for `0 <= e <= 15`, round the exact significand quotient/remainder
before testing signed bounds. Negative-exponent fractions retain their
comparison-based path. Values outside the real interval that round back into
range remain finite. Keep m32 overflow, subnormal, malformed and unmasked
paths unchanged; S2-31cl later adds m64 finite overflow.

Cover signed limits, binary80 neighbors and values around both half-integer
thresholds, ties under every RC/sign/opcode/TOP, incoming IE/PE and prior C1;
large exponents 16/31/32/63/64/16383 with minimum/mixed/maximum significands;
and exact `-32768` versus the same-bit indefinite result. Check invalid
fractions add IE without PE, failed-write/range atomicity for both opcodes,
raw source preservation, FNSTSW/FNCLEX, CLI fixtures, and JIT/native denial.
Native runtime and physical acceptance remain deferred under ISSUE-019.

<a id="s2-31bs-admit-sticky-oe-to-raw-binary80-fstp"></a>

#### S2-31bs. Admit sticky OE in raw binary80 FSTP m80fp

Relax only `bounded_x87_m80fp_source` from `FSW & 0x80cc == 0` to
`FSW & 0x80c4 == 0`, admitting masked sticky OE with IE/DE/UE/PE while still
rejecting ZE, SF, ES and B. Keep the FCW whitelist, source tag/class pairs,
address preflight and exact `DB 38` form unchanged. Perform no conversion,
normalization, quieting or flag generation: preserve all ten source bytes and
incoming exception flags, leave C0/C2/C3 unchanged, and clear C1 as before.
Only after the single atomic ten-byte write succeeds may status/pointers/RIP
and the FSTP TOP/tag update commit.

Cover all 32 sticky IE/DE/OE/UE/PE combinations across accepted classes,
signs, FCWs and TOPs. Include overflow from m32/m64 FST followed by DB38 without
FNCLEX, OE-only signaling-NaN byte preservation with no IE/PE, incoming-OE
write/range fault atomicity, FNSTSW/FNCLEX, literal CLI coverage, and explicit
JIT/native denial. Native runtime and physical acceptance remain deferred
under ISSUE-019.

<a id="s2-31bt-exact-signed-zero-fist-m32int"></a>

#### S2-31bt. Exact signed-zero `FIST`/`FISTP m32int [RAX]`

Extend only exact bare `DB 10` and `DB 18` for `X87Tag::Zero` paired with a
canonical binary80 zero payload: zero significand and sign/exponent word
`0x0000` or `0x8000`. Both signs and every RC store integer zero exactly, add
no flags, preserve incoming PE, clear C1, and retain the current C0/C2/C3
policy. Keep the FCW whitelist, `FSW & 0x80df == 0`, bare Long64 RAX addressing,
and full canonical48 Dword range check. Perform one atomic Dword write with no
read; preserve source bytes. FIST keeps the physical tag and TOP; FISTP empties
the old TOP tag and increments TOP modulo eight only after a successful write.
Do not extend m16/m64 or nonzero values with magnitude below one.

Cover sign, RC, opcode, TOP, prior PE/C1, canonical/tag mismatch and unsupported
controls/classes. Check write and range atomicity, literal CLI/FNSTSW fixtures,
and JIT/native denial. Native runtime and physical checks remain deferred under
ISSUE-019.

<a id="s2-31bu-exact-signed-zero-fist-m16int"></a>

#### S2-31bu. Exact signed-zero `FIST`/`FISTP m16int [RAX]`

Extend only exact bare `DF 10` and `DF 18` for `X87Tag::Zero` paired with a
canonical binary80 zero payload: zero significand and sign/exponent word
`0x0000` or `0x8000`. Both signs and every RC store Word zero exactly, add no
flags, preserve incoming PE, clear C1, and retain the current C0/C2/C3 policy.
Keep the FCW whitelist, `FSW & 0x80df == 0`, bare Long64 RAX addressing, and
complete canonical48 Word range check. Perform one atomic Word write with no
read; preserve source bytes. FIST keeps the physical tag and TOP; FISTP empties
the old TOP tag and increments TOP modulo eight only after successful write.
Do not widen the existing m32 behavior or admit m64, nonzero magnitudes below
one, masked invalid values, or additional forms.

Cover sign, RC, opcode, TOP, prior PE/C1, tag/payload mismatches, prohibited
controls/prefixes, and write/range atomicity. Add a literal CLI/FNSTSW fixture
and JIT/native denial coverage. Native runtime and physical checks remain
deferred under ISSUE-019.

<a id="s2-31bv-exact-signed-zero-fistp-m64int"></a>

#### S2-31bv. Exact signed-zero `FISTP m64int [RAX]`

Extend only exact bare `DF 38` for `X87Tag::Zero` paired with a canonical
binary80 zero payload: zero significand and sign/exponent word `0x0000` or
`0x8000`. Both signs and all four admitted RC modes store Qword zero exactly,
generate no new flags, preserve incoming PE, clear C1, and retain the current
C0/C2/C3 policy. Keep the FCW whitelist, `FSW & 0x80df == 0`, bare Long64 RAX
addressing, and full canonical48 Qword range check. Perform one atomic Qword
write with no read and preserve the source bytes. FISTP empties the old TOP tag
and advances TOP modulo eight only after write success. Do not add FIST m64,
other forms, nonzero magnitudes below one, or masked-invalid behavior.

Cover sign, RC, TOP, incoming PE/C1, canonical/tag/class rejection, controls
and prefixes, no-read/single-write behavior, write/range atomicity, signed-i64
regressions, CLI/FNSTSW fixture, and JIT/native denial. Native runtime and
physical acceptance remain deferred under ISSUE-019.

<a id="s2-31bw-round-binary80-fractions-through-bounded-m32-fist"></a>

#### S2-31bw. Round Normal binary80 fractions through bounded m32 `FIST`/`FISTP`

Extend only bare `DB 10` and `DB 18` for Valid-tagged canonical binary80
Normal sources with unbiased exponent -16382 through -1. Every such nonzero
fraction stores signed integer zero or one according to FCW rounding: nearest
ties-even returns zero through exact 0.5 and one above it; toward-zero returns
zero; toward-positive returns one only for positive inputs; toward-negative
returns minus one only for negative inputs. Every accepted value is inexact
and adds sticky PE after the atomic Dword write. C1 is set iff magnitude one
is selected; no other exception flags are generated. Preserve incoming PE,
source bytes, prior C0/C2/C3, the existing FCW/FSW/address gates, and FIST vs
FISTP state/pop ordering. Keep existing signed-zero behavior unchanged; this
new branch does not admit subnormal, pseudo, malformed, or Special-tagged
sources, and adds no fractional m16/m64 forms.

Use only exponent/significand comparisons for this range; do not construct
large shifts or divisors. Cover both signs, four RCs, both opcodes, every TOP,
incoming PE/C1, half and adjacent encodings, values below one, minimum Normal,
and exponents -2/-63/-64/-65. Preserve zero and ±1 regressions; test forbidden
classes/environments, range and write atomicity, CLI/FNSTSW, and JIT/native
denial. Native runtime and physical acceptance remain deferred under ISSUE-019.

<a id="s2-31bx-round-binary80-fractions-through-bounded-m16-fist"></a>

#### S2-31bx. Round Normal binary80 fractions through bounded m16 `FIST`/`FISTP`

Extend only bare `DF 10` and `DF 18` for Valid-tagged canonical binary80
Normal sources with unbiased exponent -16382 through -1. Use the same
comparison-only rounding rule as the bounded m32 fraction slice: nearest
ties-even returns zero through exact 0.5 and signed one above it; toward-zero
returns zero; directed rounding returns one only when the sign and direction
round away from zero. Every accepted input adds sticky PE after the atomic Word
write; C1 is set iff the result magnitude is one, and no IE/DE/OE/UE is added.
Preserve source bytes, incoming PE, C0/C2/C3, existing FCW/FSW/address gates,
and FIST/FISTP state/pop ordering. Do not change m32 or admit m64 fractions.

Cover both signs, four RCs, both opcodes, every TOP, incoming PE/C1, exact half
and adjacent encodings, the largest fraction below one, minimum Normal and
exponents -2/-63/-64/-65. Keep signed-zero, ±1, and i16-boundary behavior
unchanged. Test tag/class/environment rejection, Word canaries and write/range
atomicity, CLI/FNSTSW, and JIT/native denial. Native runtime and physical
acceptance remain deferred under ISSUE-019.

<a id="s2-31by-round-normal-binary80-fractions-through-bounded-m64-fistp"></a>

#### S2-31by. Round Normal binary80 fractions through bounded m64 `FISTP`

Extend only existing bare `DF 38` (`FISTP m64int [RAX]`) for Valid-tagged
canonical binary80 Normal sources with unbiased exponent -16382 through -1.
Use the same comparison-only rule as m16/m32: nearest-ties-even stores zero
through exact 0.5 and signed one above; toward-zero stores zero; directed
rounding stores signed one only when it rounds away from zero. Every admitted
nonzero fraction adds sticky PE after the atomic Qword write; C1 is set iff the
result magnitude is one, with no IE/DE/OE/UE. Preserve source bytes, incoming
PE, C0/C2/C3, FCW/FSW/address gates and post-write pop ordering. Do not add an
FIST m64 form or alter m16/m32 behavior.

Cover both signs, all RC modes and TOP values, incoming PE/C1, exact half and
neighbors, largest fraction below one, minimum Normal and exponents
-2/-63/-64/-65. Preserve signed-zero, ±1 and i64-boundary regressions. Test
tag/class/environment rejection, Qword canaries, failed-write and canonical48
range atomicity, CLI/FNSTSW, and JIT/native denial. Native runtime and physical
acceptance remain deferred under ISSUE-019.

<a id="s2-31bz-masked-binary80-infinity-through-bounded-m32-fist"></a>

#### S2-31bz. Convert masked binary80 infinity through bounded m32 `FIST`/`FISTP`

Extend only bare `DB 10` and `DB 18` for Special-tagged canonical binary80
positive or negative infinity (exact significand `0x8000000000000000`,
sign/exponent `0x7fff` or `0xffff`). Under the existing masked FCW whitelist,
store the integer-indefinite Dword `0x80000000` regardless of sign or RC. After
the successful atomic Dword write, add sticky IE, clear C1, preserve incoming
PE and C0/C2/C3, and add no PE/DE/OE/UE/SF or ES/B. Keep the FSW gate, bare
Long64 RAX addressing, full canonical48 range preflight, raw source payload,
and post-write FIST/FISTP state commit. Existing IE rejection remains.

Cover signs, all admitted FCWs, both opcodes, every TOP, incoming PE/C1,
canonical/tag/class/FCW/FSW/prefix rejection, finite overflow and exact -2^31
distinction, Dword canaries and write/range atomicity, FNSTSW/FNCLEX, and
finite/fraction/zero regressions. JIT/native denial remains explicit; native
runtime and physical acceptance remain deferred under ISSUE-019.

<a id="s2-31ca-masked-binary80-infinity-through-bounded-m16-fist"></a>

#### S2-31ca. Convert masked binary80 infinity through bounded m16 `FIST`/`FISTP`

Extend only bare `DF 10` and `DF 18` for Special-tagged canonical binary80
positive or negative infinity (exact significand `0x8000000000000000`,
sign/exponent `0x7fff` or `0xffff`). Under the existing masked FCW whitelist,
store the Word integer-indefinite `0x8000` regardless of sign or RC. After the
successful atomic Word write, add sticky IE, clear C1, preserve incoming PE and
C0/C2/C3, and add no PE/DE/OE/UE/SF or ES/B. Keep the FSW gate, bare Long64 RAX
addressing, full canonical48 range preflight, raw source payload, and
post-write FIST/FISTP state commit. Existing IE rejection remains; m32 stays
unchanged and m64 is not admitted.

Cover both signs, all admitted FCWs, both opcodes, every TOP, incoming PE/C1,
exact -32768 distinction, malformed/tag/class/control/prefix rejection,
Word canaries and write/range atomicity, FNSTSW/FNCLEX, and finite/fraction/zero
regressions. Add a literal FISTP/FNSTSW fixture and explicit JIT/native denial.
Native runtime and physical acceptance remain deferred under ISSUE-019.

<a id="s2-31cb-masked-binary80-infinity-through-bounded-m64-fistp"></a>

#### S2-31cb. Convert masked binary80 infinity through bounded m64 `FISTP`

Extend only bare `DF 38` for Special-tagged canonical binary80 positive or
negative infinity (exact significand `0x8000000000000000`, sign/exponent
`0x7fff` or `0xffff`). Under the existing admitted masked FCWs, either sign
and every RC mode store Qword integer indefinite `0x8000000000000000`. After
the successful atomic Qword write, add sticky IE, clear C1, preserve incoming
PE and C0/C2/C3, and add no PE/DE/OE/UE/SF or ES/B. Keep the FSW gate, bare
Long64 RAX addressing, full canonical48 range preflight, raw source payload,
and post-write FISTP state/pop commit. Existing IE rejection remains; no FIST
m64 form is added.

Cover both signs, all admitted FCWs, every TOP and incoming PE/C1; distinguish
exact -2^63 from infinity (same Qword bits, only infinity adds IE); reject
pseudo-infinity, NaNs, malformed encodings, mismatched tags, forbidden
controls and prefixes before access. Test Qword canaries and write/range
atomicity, FNSTSW/FNCLEX, finite/fraction/zero regressions, and a literal
FISTP/FNSTSW fixture with JIT/native denial. Native runtime and physical
acceptance remain deferred under ISSUE-019.

#### S2-31bj. Round extreme underflow through binary32 stores

Extend only the bounded `D9 10`/`D9 18` m32 conversion for canonical Normal
binary80 sources with unbiased exponents -16382 through -150. Round directly to
signed zero or signed minimum subnormal under the existing four FCWs; at -150,
compare the significand with the exact half-way value without an oversized
shift. Every admitted result is inexact and commits UE+PE after the atomic Dword
write. Set C1 iff the minimum subnormal is selected. Preserve source bytes and
sign, existing FCW/FSW gates, exact behavior, pointer/pop atomicity, and all
other conversion bounds.

Cover below/equal/above half, the lowest source exponent, both signs and RCs,
signed zero, exact-minimum control, both opcodes and every TOP, all 16 sticky
IE/DE/UE/PE states, FNSTSW/FNCLEX, m32-underflow to raw-m80 preservation,
write/range atomicity, forbidden classes/tags/gates, and CLI/JIT/native denial.
Native runtime and physical acceptance remain deferred under ISSUE-019.


#### S2-31bk. Round extreme underflow through binary64 stores

Extend only bounded `DD 10`/`DD 18` for canonical Normal binary80 sources with
unbiased exponents -16382 through -1075. Round directly to signed zero or
signed minimum subnormal under the existing FCWs; at -1075 compare the
significand with the exact half-way value without an oversized shift. Every
admitted result is inexact and commits UE+PE after the atomic Qword write. Set C1
iff minimum subnormal is selected. Preserve source bytes/sign, FCW/FSW gates,
exact behavior, and atomic status/pointer/pop commit.

Cover below/equal/above half, lowest source exponent, both signs and RCs, signed
zero, exact-minimum control, both opcodes and all TOPs, all 16 sticky IE/DE/UE/PE
states, FNSTSW/FNCLEX, m64-underflow to raw-m80 preservation, Qword write/range
atomicity, forbidden classes/tags/gates, and CLI/JIT/native denial. Native runtime
and physical acceptance remain deferred under ISSUE-019.

#### S2-31bl. Round binary80 subnormals through binary32 stores

Extend only bounded D9 10/D9 18 for Special-tagged canonical binary80 Subnormal
sources (E=0, J=0, nonzero fraction), for either sign. Their magnitudes are below
half of the minimum binary32 subnormal: nearest and toward-zero produce signed
zero; toward-positive produces positive minimum subnormal or negative zero;
toward-negative produces negative minimum subnormal or positive zero. Every
accepted result is inexact and commits UE+PE after the atomic Dword write. C1 is
set iff minimum subnormal is selected. Preserve the raw source, existing masks,
gates, exact behavior and atomic status/pointer/pop commit. Reject all other
subnormal encodings/tags before access.

Cover fractions 1, 2, 0x4000... and 0x7fff..., both signs/RCs/opcodes/TOPs and
all 16 sticky IE/DE/UE/PE states, C1 replacement, raw-m80 source preservation,
zero/minimum-normal controls, forbidden classes/tags/gates, write/range
atomicity, FNSTSW/FNCLEX and CLI/JIT/native denial. Native runtime and physical
acceptance remain deferred under ISSUE-019.

#### S2-31bm. Round binary80 subnormals through binary64 stores

Extend only bounded `DD 10`/`DD 18` for Special-tagged canonical binary80
Subnormal sources (`E=0`, `J=0`, nonzero fraction). Since these values are below
half the minimum binary64 subnormal, nearest and toward-zero produce signed
zero; toward-positive produces positive minimum subnormal or negative zero;
toward-negative produces negative minimum subnormal or positive zero. Accepted
conversions always add UE+PE after a successful Qword write, never DE, and set
C1 iff minimum subnormal is selected. Preserve raw source bytes and the current
FCW/FSW gates, with status, pointer, RIP, and FSTP pop committed atomically.
Reject other tags and encodings before access.

Cover fractions 1, 2, `0x4000000000000000`, and
`0x7fffffffffffffff`; both signs, opcodes, FCWs, TOPs and all 16 sticky
IE/DE/UE/PE combinations; directed negative outcomes; raw FLD m80 → FST m64 →
FNCLEX → raw FSTP m80 preservation; invalid classes/tags and gates; write/range
atomicity; FNSTSW/FNCLEX; literal CLI fixtures and JIT/native denial. Native
runtime and physical acceptance remain deferred under ISSUE-019. See the
[S2-31bm report](../reports/STAGE_2_REPORT.md#s2-31bm--binary80-subnormal-to-binary64-underflow-rounding).

#### S2-31bn. Round exponent -127 binary80 normals directly to binary32 minimum normal

Extend only bounded `D9 10`/`D9 18` for Valid-tagged canonical binary80 Normal
sources at unbiased exponent -127 when direct rounding of `S / 2^41` selects
exactly `0x800000`, the binary32 minimum normal significand. Store the minimum
normal directly from that 23-bit destination rounding; do not derive stored
bits from the separate 24-bit rounding used to classify underflow. Set PE and
C1 for the increment. Decide new UE independently: round `S >> 40` and its low
40-bit remainder with the same sign-aware RC, and set UE iff the rounded
24-bit significand remains below `1 << 24`. Preserve any prior UE when this
calculation does not add it. Keep the existing FCW/FSW gates, source bytes and
all other m32/m64 paths unchanged. Commit flags, FIP/FDP, RIP and FSTP pop only
after the atomic Dword write succeeds.

Cover exact maximum-subnormal and minimum-normal controls; nearest-even and
directed-rounding thresholds and neighbors for both signs; every RC, opcode,
TOP and all 16 sticky IE/DE/UE/PE combinations; the independent 24-bit UE
threshold; and an exact-integer oracle for both thresholds. Also cover write
and range fault atomicity, FNSTSW/FNCLEX, literal CLI fixtures and JIT/native
denial. Native runtime and physical acceptance remain deferred under
ISSUE-019.

#### S2-31bo. Round exponent -1023 binary80 normals directly to binary64 minimum normal

Extend only bounded `DD 10`/`DD 18` for Valid-tagged canonical binary80 Normal
sources at unbiased exponent -1023 when direct destination rounding of
`S / 2^12` selects exactly `1 << 52`. Store signed binary64 minimum normal
from that destination rounding and add PE+C1. Independently classify UE by
rounding `S >> 11` with its low 11-bit remainder under the same sign-aware RC;
add UE iff that unbounded 53-bit significand remains below `1 << 53`. Never use
the auxiliary value to form stored bits. Preserve existing IE/DE/UE/PE and keep
all gates, source bytes and non-target m64 paths unchanged. Commit flags,
pointers, RIP and FSTP pop only after a successful atomic Qword write.

Cover nearest and directed destination thresholds with neighbors, the
independent wide UE threshold, both signs, all RCs/opcodes/TOPs and 16 sticky
flag combinations; exact maximum-subnormal and minimum-normal controls; an
exact integer oracle; write/range fault atomicity; FNSTSW/FNCLEX; and literal
CLI fixtures with JIT/native denial. Native runtime and physical acceptance
remain deferred under ISSUE-019.

#### S2-31bp. Round exponent-127 binary80 normals to binary32 infinity on overflow

Extend only bounded `D9 10`/`D9 18` for Valid-tagged canonical binary80 Normal
sources at unbiased exponent 127 when existing sign-aware rounding of
`S / 2^40` carries the 24-bit significand to `1 << 24`. Store signed binary32
infinity after a successful atomic Dword write, then add sticky OE+PE and set
C1. Do not add IE, DE or UE. Keep the current exact FCW set and
`FSW & 0x80cc == 0` gate, so a later bounded store remains rejected until
FNCLEX clears OE. Preserve source bytes, all other FST/FSTP behavior and
write-before-pop/status/pointer commit ordering. Exponent-128 sources remain
unsupported.

Cover the maximum-finite value, nearest-even midpoint and neighbors, directed
rounding thresholds, both signs, all RCs/opcodes/TOPs and all 16 sticky
IE/DE/UE/PE combinations; exponent-128 rejection; Dword write/range atomicity;
FNSTSW/FNCLEX and sticky-OE gate behavior; and CLI fixtures with JIT/native
denial. Native runtime and physical acceptance remain deferred under ISSUE-019.


#### S2-31bq. Round exponent-1023 binary80 normals to binary64 infinity on overflow

Extend only bounded `DD 10` and `DD 18` for Valid-tagged canonical binary80
Normal sources at unbiased exponent 1023. When sign-aware rounding of
`S / 2^11` carries the 53-bit significand to `1 << 53`, store signed binary64
infinity. A successful atomic Qword write then commits sticky OE+PE and C1; do
not add IE, DE or UE. Keep the exact FCW whitelist and `FSW & 0x80cc == 0`
gate, so sticky OE denies the next bounded store until FNCLEX. Preserve source
bytes and all existing FST/FSTP pointer, stack, and retirement behavior.
Exponent-1024 sources, malformed encodings, and unmasked/unsupported
environments reject before data access.

Use an independent exact-integer oracle for `0xfffffffffffff800`,
`0xfffffffffffff801`, `0xfffffffffffffbff`, `0xfffffffffffffc00`,
`0xfffffffffffffc01`, and `0xffffffffffffffff`, across both signs, all RCs,
both opcodes, every TOP and all 16 sticky IE/DE/UE/PE combinations. Cover
finite versus infinity choices under directed rounding, exponent-1024 rejection,
Qword write/range fault atomicity, FNSTSW/FNCLEX, and interpreter fixtures with
JIT/native denial. Native runtime and physical acceptance remain deferred under
ISSUE-019.


<a id="s2-31cc-masked-binary80-qnan-through-bounded-m32-fist"></a>

#### S2-31cc. Convert masked binary80 quiet NaN through bounded m32 `FIST`/`FISTP`

Extend only bare `DB 10` and `DB 18` for Special-tagged canonical binary80
quiet NaNs: exponent `0x7fff`, either sign, explicit integer bit J=1, quiet
bit=1, and arbitrary lower payload. Under the existing admitted masked FCWs,
store Dword integer indefinite `0x80000000` for every payload, sign and RC.
After the atomic Dword write, add sticky IE, clear C1, preserve incoming PE and
C0/C2/C3, and add no PE/DE/OE/UE/SF or ES/B. Preserve the raw source and
existing addressing, gates, and post-write FIST/FISTP commit behavior. SNaN,
J=0 pseudo-NaNs, mismatched tags, finite overflow, m16/m64 QNaNs, and unmasked
delivery remain outside this slice.

Coverage crosses four payload shapes, both signs, all admitted FCWs, both
opcodes, all TOP values, and incoming PE/C1. Rejections include J=0 quiet-like
encodings, SNaN, tag mismatch, range and write failures; a literal FISTP/FNSTSW
fixture plus FNSTSW/FNCLEX unit sequence observes and clears IE. JIT/native
admission remains closed.


<a id="s2-31cd-masked-binary80-qnan-through-bounded-m16-fist"></a>

#### S2-31cd. Convert masked binary80 quiet NaN through bounded m16 `FIST`/`FISTP`

Extend only bare `DF 10` and `DF 18` for Special-tagged canonical binary80
quiet NaNs: exponent `0x7fff`, either sign, explicit integer bit J=1, quiet
bit=1, and arbitrary lower payload. Under the existing admitted masked FCWs,
store Word integer indefinite `0x8000` for every payload, sign and RC. After
the atomic Word write, add sticky IE, clear C1, preserve incoming PE and C0/C2/C3,
and add no PE/DE/OE/UE/SF or ES/B. Preserve the raw source, addressing and
post-write FIST/FISTP state/pop ordering. SNaN, J=0 pseudo-NaNs, malformed or
mismatched tags, m64 QNaNs and unmasked delivery remain outside this slice;
m32 behavior is unchanged.

Coverage crosses four payload patterns, both signs, all admitted FCWs, both
opcodes, every TOP and incoming PE/C1. Rejections include J=0 quiet-like
encodings, SNaN, tag/class/control/prefix and m64 width; Word canaries, write and
range faults prove atomicity. A literal FISTP/FNSTSW fixture and a direct
FNSTSW/FNCLEX sequence observe and clear IE. JIT/native admission remains
closed.


<a id="s2-31ce-masked-binary80-qnan-through-bounded-m64-fistp"></a>

#### S2-31ce. Convert masked binary80 quiet NaN through bounded m64 `FISTP`

Extend only bare `DF 38` for Special-tagged canonical binary80 quiet NaNs:
exponent `0x7fff`, either sign, explicit integer bit J=1, quiet bit=1, and
arbitrary lower payload. Under the existing admitted masked FCWs, store Qword
integer indefinite `0x8000000000000000` for every payload, sign and RC. After
the atomic Qword write, add sticky IE, clear C1, preserve incoming PE and
C0/C2/C3, and add no PE/DE/OE/UE/SF or ES/B. Preserve the raw source and
post-write FISTP state/pop ordering. SNaN, J=0 pseudo-NaNs, malformed or
mismatched tags, finite overflow, non-pop m64 and unmasked delivery remain
outside this slice.

Coverage crosses four payload patterns, both signs, all admitted FCWs, every
TOP and incoming PE/C1. Rejections include J=0 quiet-like encodings, SNaN, tag,
control and prefix; Qword canaries and write/range faults prove atomicity. A
literal FISTP/FNSTSW fixture and direct FNSTSW/FNCLEX sequence observe and
clear IE. JIT/native admission remains closed.


<a id="s2-31cf-masked-binary80-snan-through-bounded-m32-fist"></a>

#### S2-31cf. Convert masked binary80 signaling NaN through bounded m32 `FIST`/`FISTP`

Extend only bare `DB 10` and `DB 18` for Special-tagged canonical binary80
signaling NaNs: exponent `0x7fff`, either sign, explicit integer bit J=1,
quiet bit=0, and nonzero lower 62-bit payload. Under the existing admitted
masked FCWs, store Dword integer indefinite `0x80000000` for every payload,
sign and RC. After the atomic Dword write, add sticky IE, clear C1, preserve
incoming PE and C0/C2/C3, and add no PE/DE/OE/UE/SF or ES/B. Preserve raw
signaling source bytes for both FIST and FISTP. Canonical infinity and QNaN
remain admitted by earlier slices; J=0 pseudo-NaNs, mismatched tags, finite
overflow, m16/m64 SNaNs, and unmasked delivery remain outside this slice.

Coverage crosses four payload patterns, both signs, all admitted FCWs, both
opcodes, every TOP and incoming PE/C1 (2,048 cases). Rejections include J=0
pseudo-SNaN/pseudo-QNaN, mismatched tags, controls, prefixes, unsupported
widths, and range/write faults with full-state atomicity. A literal
FISTP/FNSTSW fixture plus direct FNSTSW/FNCLEX observes and clears IE; JIT and
native admission remain closed. Native runtime and physical acceptance remain
deferred under ISSUE-019.


<a id="s2-31cg-masked-binary80-snan-through-bounded-m16-fist"></a>

#### S2-31cg. Convert masked binary80 signaling NaN through bounded m16 `FIST`/`FISTP`

Extend only bare `DF 10` and `DF 18` for Special-tagged canonical binary80
signaling NaNs: exponent `0x7fff`, either sign, J=1, quiet bit=0, and
nonzero lower 62-bit payload. Under the existing admitted masked FCWs, store
Word integer indefinite `0x8000` for every payload, sign and RC. After the
atomic Word write, add sticky IE, clear C1, preserve incoming PE and C0/C2/C3,
and add no PE/DE/OE/UE/SF or ES/B. Preserve raw SNaN bytes through FIST and
FISTP; the m32 SNaN slice is unchanged and m64 is handled separately in
S2-31ch.

Coverage crosses four payload patterns, both signs, all admitted FCWs, both
opcodes, every TOP and incoming PE/C1 (2,048 cases). Reject J=0 pseudo-NaNs,
mismatched tags, invalid controls, prefixes, and canonical-range/write faults
atomically. The former m64 `DF 38` rejection was superseded when S2-31ch
admitted that form. A literal FISTP/FNSTSW fixture and direct FNSTSW/FNCLEX
sequence observe and clear IE. JIT/native admission remains closed; native
runtime and physical acceptance remain deferred under ISSUE-019.


<a id="s2-31ch-masked-binary80-snan-through-bounded-m64-fistp"></a>

#### S2-31ch. Convert masked binary80 signaling NaN through bounded m64 `FISTP`

Extend only bare `DF 38` for Special-tagged canonical binary80 signaling NaNs:
exponent `0x7fff`, either sign, J=1, quiet bit=0, and nonzero lower 62-bit
payload. Under the existing admitted masked FCWs, store Qword integer
indefinite `0x8000000000000000` for every payload, sign and RC. After the
atomic Qword write, add sticky IE, clear C1, preserve incoming PE and C0/C2/C3,
and add no PE/DE/OE/UE/SF or ES/B. Preserve raw SNaN bytes through FISTP;
m16/m32 SNaN behavior stays unchanged.

Coverage crosses four payload patterns × both signs × four FCWs × every TOP
× incoming PE/C1 (1,024 cases). Reject J=0 pseudo-NaNs, mismatched tags,
invalid controls, prefixes and canonical-range/write faults atomically. A
literal FISTP/FNSTSW fixture and direct FNSTSW/FNCLEX sequence observe and
clear IE. JIT/native admission remains closed; native runtime and physical
acceptance remain deferred under ISSUE-019.


<a id="s2-31cl-masked-finite-overflow-to-m64-fistp"></a>

#### S2-31cl. Masked finite overflow and roundback to m64 `FISTP`

Extend only bare `DF 38` for Valid-tagged canonical binary80 Normal values.
Round the exact significand quotient/remainder under the current FCW mode
before checking signed i64 bounds. If the rounded result is outside
`[-2^63, 2^63-1]`, write Qword integer-indefinite
`0x8000000000000000`, then add sticky IE and clear C1 after the atomic Qword
write. Do not add PE/OE/UE/DE/SF/ES/B; preserve incoming IE/PE and C0/C2/C3.
Values outside the real-number limits that round back into range remain finite
results. At exponent 63, positive magnitudes overflow; only negative
significand `0x8000000000000000` is the exact valid i64 minimum. At exponent 64
or greater, classify invalid without a wide shift. Existing fractions below
one retain their comparison-only behavior.

Keep the FCW whitelist, `FSW & 0x80de` gate, bare Long64 RAX addressing,
canonical48 Qword preflight, one atomic write/no read, raw source preservation,
and post-write FISTP state/pop commit. Subnormals, malformed sources and
unmasked delivery remain outside this slice. Coverage includes signed limits
and adjacent binary80 values, half-integer roundback, exponents 64, 65 and
16383 with minimum/mixed/maximum significands, all RC/TOP/IE/PE/C1 states,
write/range atomicity, FNSTSW/FNCLEX, CLI, and JIT/native denial. See the
[CPU contract](../specs/CPU_CONTRACT.md#local-s2-31cl-masked-finite-overflow-m64-fistp),
[report](../reports/STAGE_2_REPORT.md#s2-31cl--masked-finite-overflow-to-m64-fistp),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


<a id="s2-31cv-pseudo-infinity-m64-fistp"></a>

#### S2-31cv. Convert Special-tagged pseudo-infinity through m64 `FISTP`

Only bare `DF 38` admits a Special-tagged binary80 pseudo-infinity with raw
sign/exponent `0x7fff` or `0xffff` and zero significand (J=0). Store Qword
integer-indefinite `0x8000000000000000`, add current IE, clear C1, and add no
PE or other current flags. Preserve historical sticky flags and C0/C2/C3.
Retain the FCW whitelist, `FSW & 0x80c0` gate, bare Long64 RAX form,
canonical48 8-byte preflight, and one atomic Qword write without a read.
Commit status, FIP/FDP/RIP and the single FISTP TOP pop only after the write
succeeds. Preserve all ten source bytes and raw payload.

Canonical signed zero and canonical J=1 infinity retain their distinct
existing status behavior. Pseudo-NaNs, unnormals, pseudo-denormals, mismatched
tags, unmasked or otherwise disallowed controls/status, prefixes, m16/m32
behavior changes, `FIST m64`, and other forms remain unsupported. FLD m80
composition is excluded because FLD rejects pseudo-infinity. JIT/native
admission remains denied; physical x86 acceptance and native runtime remain
deferred to the combined final S2-31 substage under ISSUE-019.

Coverage crosses both signs × four FCWs × eight TOP positions × 64 sticky
combinations × two previous C1 values (8,192 cases), checking literal Qword
bytes/status, IE without PE, C1 clearing, source preservation, tag/pop and TOP
wrap, canaries, and one-write/no-read access. Rejection and fault cases cover
malformed neighboring encodings, tag/control gates, denied writes and
canonical48 crossing. FNSTSW/FNCLEX, a direct seeded CLI fixture, and JIT/native
denial are included. See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-31cv-pseudo-infinity-m64-fistp),
[report](../reports/STAGE_2_REPORT.md#s2-31cv--special-tagged-pseudo-infinity-through-m64-fistp),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).







<a id="s2-31cm-admit-sticky-de-on-bounded-integer-stores"></a>

#### S2-31cm. Admit sticky DE on bounded integer stores

Admit historical DE only on the existing DB 10, DB 18, DF 10, DF 18 and DF 38
integer-store forms by changing their shared entry mask from `0x80de` to
`0x80dc`. This adds no new conversion classes or widths. Preserve incoming
IE/DE/PE and C0/C2/C3; DE alone adds no current-operation flags. Current
conversion PE/IE and C1 are committed only after the existing atomic write.
Continue rejecting ZE/OE/UE/SF/ES/B, malformed source encodings, binary80
subnormals and pseudo-denormals. Other x87 gates remain unchanged.

The 35,840-case sticky matrix spans seven admitted source classes, both signs,
all five forms, all four FCWs, eight TOP values, incoming IE/DE/PE, and both
prior C1 values. An additional 40-case composition test loads positive and
negative minimum/maximum binary32 and binary64 subnormals, then stores to all
five integer forms without FNCLEX; DE remains sticky, inexact integer rounding
adds PE, and C1 reflects the store. Five-form denied-write and canonical-range
tests carry IE|DE|PE; DE combined with each still-forbidden FSW bit rejects
before memory access. Literal FISTP/FNSTSW fixtures seed the canonical
binary80 intermediate that FLD creates, because the current fixture schema
accepts only the exact instruction pair; the full FLD-to-store sequence is
covered by the ISA test. JIT/native admission and physical execution remain
closed/deferred. See the
[CPU contract](../specs/CPU_CONTRACT.md#local-s2-31cm-admit-sticky-de-bounded-integer-stores),
[report](../reports/STAGE_2_REPORT.md#s2-31cm--sticky-de-on-bounded-integer-stores),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


<a id="s2-31co-admit-sticky-oe-on-bounded-integer-stores"></a>

#### S2-31co. Admit sticky OE on bounded integer stores

Admit incoming OE only on the existing DB10, DB18, DF10, DF18 and DF38
integer-store forms by narrowing the shared FSW gate from `0x80cc` to
`0x80c4`. Preserve IE/DE/OE/UE/PE independently; continue rejecting ZE/SF/ES/B.
Historical OE must not affect the current integer result, add IE/PE, or retain
prior C1. Other x87 gates, source forms, FCW and range behavior stay unchanged;
raw binary80 subnormal and pseudo-denormal integer sources remain unsupported.

Coverage is a 143,360-case matrix across seven admitted classes, signs, five
forms, four FCWs, eight TOP values, all 32 IE/DE/OE/UE/PE combinations and
prior C1. Twenty actual FST m32→integer-store cases use `(E,S)=(0x407e,
0xffffffffffffffff)` where the selected sign/rounding mode produces infinity;
FST m32 stores signed infinity and OE|PE, then integer overflow stores
indefinite and adds IE while retaining OE|PE and clearing C1. Five-form
write/range atomicity, OE plus each forbidden status, invalid re-entry,
OE-only exact conversion, FNSTSW/FNCLEX, CLI fixtures, and JIT/native admission
denial are required. Native execution and physical x86 acceptance remain
closed/deferred under ISSUE-019.

<a id="s2-31cn-admit-sticky-ue-on-bounded-integer-stores"></a>

#### S2-31cn. Admit sticky UE on bounded integer stores

Admit incoming UE only on the existing DB 10, DB 18, DF 10, DF 18 and DF 38
integer-store forms by changing their shared FSW mask from `0x80dc` to
`0x80cc`. This admits and preserves IE/DE/UE/PE independently while ZE/OE/
SF/ES/B remain rejected. Historical UE does not alter the current conversion
or synthesize IE/PE; C1 and new conversion flags remain operation-specific and
commit only after the atomic destination write. FCW, source classes, range,
addressing and stack behavior remain unchanged, including rejection of raw
binary80 subnormal and pseudo-denormal integer operands.

The sticky matrix covers 71,680 cases: seven admitted source classes, both
signs, five forms, four FCWs, eight TOP values, all 16 IE/DE/UE/PE combinations
and both prior C1 states. A 40-case FST m32→integer-store composition matrix
creates sticky UE|PE from a 1.5-minimum-binary32-subnormal source, then checks
that integer conversion preserves those flags and recomputes C1 for each sign,
rounding mode and store form. Five-form write/range faults preserve full state
and memory with IE|DE|UE|PE incoming; UE combined with each remaining forbidden
FSW bit rejects before memory access. Two literal FISTP/FNSTSW fixtures seed
the canonical binary80 source and incoming UE because the fixture schema allows
only this instruction pair. JIT/native admission and physical execution remain
closed/deferred. See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-31cn-admit-sticky-ue-bounded-integer-stores),
[report](../reports/STAGE_2_REPORT.md#s2-31cn--sticky-ue-on-bounded-integer-stores),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


<a id="s2-31cp-admit-historical-sticky-ze-on-bounded-integer-stores"></a>

#### S2-31cp. Admit historical sticky ZE on bounded integer stores

Admit incoming ZE only on the existing DB 10, DB 18, DF 10, DF 18 and DF 38
integer-store forms by changing the shared FSW gate from `0x80c4` to
`0x80c0`. Preserve IE/DE/ZE/OE/UE/PE independently; continue rejecting SF/ES/B.
Historical ZE alone does not change the current result, add flags, or preserve
prior C1. Other x87 gates, current exception generation, source classes,
addressing, FCW, rounding, range handling and atomic write-before-state-commit
remain unchanged. No ZE-producing division form, unmasked delivery, or binary80
subnormal/pseudo-denormal integer source is included.

Coverage is a 286,720-case matrix across seven admitted source classes, both
signs, five forms, four FCWs, eight TOP values, all 64 sticky exception
combinations and both prior C1 values. ZE-only exact, ZE+current PE/inexact and
ZE+current IE/invalid behavior, two stores without FNCLEX, and FNSTSW/FNCLEX
observation are checked. Five-form write and canonical-range faults preserve
all six sticky flags, complete CPU state and memory; ZE combined with SF, ES or
B rejects before access. Unsupported FCW and source-class controls remain
rejected. Literal CLI fixtures seed only historical ZE; JIT/native admission
stays denied. Physical x86 and native runtime acceptance remain deferred to
the combined final S2-31 substage. See the
[CPU contract](../specs/CPU_CONTRACT.md#local-s2-31cp-admit-historical-sticky-ze-bounded-integer-stores),
[report](../reports/STAGE_2_REPORT.md#s2-31cp--historical-sticky-ze-on-bounded-integer-stores),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


<a id="s2-31cq-canonical-subnormal-m32-fist"></a>

#### S2-31cq. Admit canonical binary80 subnormals through m32 `FIST`/`FISTP`

Only bare DB 10 and DB 18 admit a Special-tagged canonical nonzero binary80
subnormal: exponent zero, explicit integer bit J zero, and nonzero fraction.
Such values are strictly below one half. RN/TZ store zero; RD stores -1 only
for a negative source; RU stores +1 only for a positive source. Every admitted
conversion adds PE, and C1 is set exactly when the stored magnitude is one.
No other exception/status flags are generated. Preserve the six incoming
sticky flags under the existing `0x80c0` FSW mask and preserve C0/C2/C3. Keep
the FCW whitelist, full canonical48 Dword preflight, one write/no read,
write-before-commit ordering, source payload preservation and FIST/FISTP TOP
rules. m16/m64, pseudo-denormals, malformed/tag-mismatched inputs and unmasked
delivery remain unsupported.

Cover four literal fraction payloads, both signs, all RC values/opcodes/TOPs,
64 incoming sticky-flag combinations and both old C1 values (65,536 cases),
plus FLD m80 composition, invalid classes/tags, write/range atomicity,
FNSTSW/FNCLEX composition, literal CLI observation and JIT/native denial. This local
extension does not complete S2-31 or ISSUE-019. See the
[CPU contract](../specs/CPU_CONTRACT.md#local-s2-31cq-canonical-subnormal-m32-fist),
[report](../reports/STAGE_2_REPORT.md#s2-31cq--canonical-subnormal-m32-fist),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).

<a id="s2-31cr-canonical-subnormal-m16-fist"></a>

#### S2-31cr. Admit canonical binary80 subnormals through m16 `FIST`/`FISTP`

Extend only bare DF 10 and DF 18 so a Special-tagged canonical nonzero
binary80 subnormal (exponent zero, J=0, nonzero fraction) follows the existing
exact integer rounding path. RN/TZ store zero; RD stores -1 only for a
negative source; RU stores +1 only for a positive source. Each conversion
adds PE and sets C1 exactly when the result magnitude is one. Keep the FCW
whitelist, `FSW & 0x80c0` gate, status preservation, canonical48 Word
preflight, one atomic write/no read, raw source payload, and post-write state
and pop commit. The corresponding m32 path is unchanged; m64 subnormal,
pseudo-denormal, malformed/tag-mismatched, and unmasked cases remain closed.

Cover the same 65,536-case fraction/sign/RC/opcode/TOP/sticky/C1 matrix,
FLD m80 composition, source/control rejection, Word canaries, write/range
atomicity, FNSTSW/FNCLEX, and literal CLI/JIT/native admission checks. Physical
x86 acceptance remains part of the combined final S2-31 substage under
ISSUE-019.

<a id="s2-31cs-canonical-subnormal-m64-fistp"></a>

#### S2-31cs. Admit canonical binary80 subnormals through m64 `FISTP`

Extend only bare `DF 38` (`FISTP m64int [RAX]`) for a Special-tagged
canonical binary80 nonzero subnormal (exponent zero, explicit integer bit
J=0, nonzero fraction). RN/TZ store zero; RD stores all-ones only for a
negative source; RU stores one only for a positive source. Every admitted
conversion adds sticky PE and sets C1 exactly when the rounded magnitude is
one. Preserve incoming sticky flags and C0/C2/C3 under the existing
`FSW & 0x80c0` gate. Keep the FCW whitelist, Long64 bare RAX form, canonical48
eight-byte preflight, one atomic Qword write/no read, and post-write state/RIP/
pop commit. Preserve the old physical TOP's raw ten-byte payload when popping.
No `FIST m64` form is added. The m16/m32 paths are unchanged; pseudo-
denormals, malformed or tag-mismatched sources, unmasked cases, and other
forms remain unsupported.

Cover four literal fractions × both signs × four RC values × eight TOP values
× 64 incoming sticky-flag combinations × both old C1 values (32,768 cases),
plus FLD m80→FISTP→FNSTSW/FNCLEX composition, source/control/status rejects,
Qword canaries, write/range atomicity, wrap and raw-source preservation,
m16/m32 regressions, literal CLI observation, and JIT/native denial. Physical
x86 acceptance remains part of the combined final S2-31 substage under
ISSUE-019. See the
[CPU contract](../specs/CPU_CONTRACT.md#local-s2-31cs-canonical-subnormal-m64-fistp),
[report](../reports/STAGE_2_REPORT.md#s2-31cs--canonical-subnormal-m64-fistp),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


<a id="s2-31ct-pseudo-infinity-m32-fist"></a>

#### S2-31ct. Convert Special-tagged pseudo-infinity through m32 `FIST`/`FISTP`

Only bare DB 10 and DB 18 admit a Special-tagged binary80 pseudo-infinity:
raw sign/exponent `0x7fff` or `0xffff`, zero significand, all-one exponent,
and explicit integer bit J=0. Store the Dword integer-indefinite value
`0x80000000`, add current IE, and clear C1. Do not add PE or other current
flags; preserve existing sticky flags and C0/C2/C3. Keep the existing FCW
whitelist, `FSW & 0x80c0` gate, bare Long64 RAX form, complete canonical48
Dword preflight, atomic one-write/no-read behavior, and write-before-commit
ordering. Preserve raw source bytes; FIST retains TOP and FISTP pops once,
including TOP wrap.

Canonical signed zero, canonical infinity with J=1, and pseudo-NaNs remain
distinct. m16/m64, wrong tags, unmasked/status rejects, prefixes, and all
other gates remain unchanged. FLD m80 composition is not claimed because FLD
does not admit pseudo-infinity.

Coverage crosses both raw signs × four FCWs × both forms × all TOP values ×
64 sticky combinations × both previous C1 values (16,384 cases), with literal
expected store/status values. Additional checks cover adjacent pseudo-NaNs,
wrong tags, m16/m64 exclusion, source preservation, TOP wrap, canaries,
write/range atomicity, FNSTSW/FNCLEX, the literal CLI fixture, and JIT/native
denial. Physical x86 acceptance remains deferred to the combined final S2-31
substage under ISSUE-019. See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-31ct-pseudo-infinity-m32-fist),
[report](../reports/STAGE_2_REPORT.md#s2-31ct--special-tagged-pseudo-infinity-through-m32-fist),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


<a id="s2-31cu-pseudo-infinity-m16-fist"></a>

#### S2-31cu. Convert Special-tagged pseudo-infinity through m16 `FIST`/`FISTP`

Only bare DF 10 and DF 18 admit a Special-tagged binary80 pseudo-infinity:
raw sign/exponent `0x7fff` or `0xffff`, zero significand, and explicit integer
bit J=0. Store Word integer-indefinite `0x8000`, add current IE, and clear C1;
add no PE or other current flags. Preserve historical sticky flags and C0/C2/C3.
Keep the FCW whitelist, `FSW & 0x80c0` gate, bare Long64 RAX form, complete
canonical48 Word preflight, atomic one-write/no-read behavior, and write-before-
commit ordering. Preserve raw source bytes; FIST retains TOP and FISTP pops once,
including TOP wrap.

Canonical signed zero and canonical infinity (J=1) remain distinct, as do
pseudo-NaNs. Wrong tags, m32/m64 width-specific behavior outside its existing
profile, unmasked/status rejects, prefixes, and other gates remain unchanged.
FLD m80 composition is not claimed because FLD rejects pseudo-infinity.

Coverage crosses both raw signs × four FCWs × both forms × all TOP values ×
64 sticky combinations × both previous C1 values (16,384 cases). Independent
literal expectations cover Word bytes/status, IE without PE, C1 clearing, flag
preservation, source bytes, TOP/tag behavior and wrap. Rejections include
pseudo-NaNs, wrong tags, forbidden controls/status and DF38 m64; fault checks
cover denied writes and canonical-range crossing. FNSTSW/FNCLEX, a direct
literal CLI fixture, and JIT/native denial are covered. Native runtime and
physical x86 checks remain deferred to the combined final S2-31 substage under
ISSUE-019. See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-31cu-pseudo-infinity-m16-fist),
[report](../reports/STAGE_2_REPORT.md#s2-31cu--special-tagged-pseudo-infinity-through-m16-fist),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


<a id="s2-31cw-special-pseudo-nan-m32-fist"></a>

#### S2-31cw. Convert Special-tagged pseudo-NaNs through m32 `FIST`/`FISTP`

Only bare DB 10 and DB 18 admit Special-tagged binary80 pseudo-NaNs with
exponent `0x7fff`, explicit integer bit J=0, and a nonzero fraction. Both the
Q-bit-clear and Q-bit-set variants, of either sign, store Dword
integer-indefinite `0x80000000`, add IE, and clear C1. Add no PE or other
current flags; preserve historical sticky flags and C0/C2/C3. Retain the FCW
whitelist, `FSW & 0x80c0` gate, bare Long64 RAX form, complete canonical48
Dword preflight, one atomic write/no read, write-before-commit ordering and
raw ten-byte source preservation. FIST retains TOP; FISTP pops exactly once.

Pseudo-infinity (zero fraction), canonical J=1 QNaN/SNaN, and finite
non-all-one exponents remain distinct. Pseudo-denormals, unnormals, wrong tags, m16/m64 pseudo-NaNs were unsupported
at the original S2-31cw boundary; S2-31cx below later admits only the bounded
m16 DF10/DF18 cases. m64 pseudo-NaNs, FLD and unmasked delivery remain
unsupported; no generic special-class fallback is added. JIT/native admission remains denied, and
physical x86 acceptance remains part of the combined final S2-31 substage under
ISSUE-019.

Coverage crosses four literal fractions (`1`, `0x3fff_ffff_ffff_ffff`,
`0x4000_0000_0000_0000`, `0x7fff_ffff_ffff_ffff`) × both signs × four FCWs ×
both forms × all TOP values × 64 sticky combinations × both old C1 values
(65,536 cases). Independent expectations check Dword bytes, IE without PE,
C1 clearing, status preservation, source payload, canaries, one write/no read,
and TOP/tag rules. Controls cover zero-fraction pseudo-infinity, J=1 canonical
QNaN/SNaN, non-all-one exponent, wrong tags, m16/m64 at the original S2-31cw
boundary, m64 under the current S2-31cx scope, FCW/status, Dword write
and canonical-range atomicity. FNSTSW/FNCLEX and a direct seeded CLI fixture
are included. FLD composition is excluded because FLD does not admit J=0
pseudo-NaNs. See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-31cw-special-pseudo-nan-m32-fist),
[report](../reports/STAGE_2_REPORT.md#s2-31cw--special-tagged-pseudo-nans-through-m32-fist),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


<a id="s2-31cx-special-pseudo-nan-m16-fist"></a>

#### S2-31cx. Convert Special-tagged pseudo-NaNs through m16 `FIST`/`FISTP`

Only bare DF 10 and DF 18 admit a Special-tagged binary80 pseudo-NaN with
exponent `0x7fff`, J=0, and a nonzero fraction. Both bit-62 variants and
either sign store Word integer-indefinite `0x8000`, add IE, and clear C1; no
PE or other current flags are added. Preserve historical sticky flags and
C0/C2/C3. Retain the FCW whitelist, `FSW & 0x80c0` gate, bare Long64 RAX
form, complete canonical48 Word preflight, atomic one-write/no-read behavior,
write-before-commit order, and raw ten-byte source. FIST retains TOP; FISTP
pops once after a successful write.

Zero-fraction pseudo-infinity and canonical J=1 QNaN/SNaN retain their existing
behavior. Wrong tags, pseudo-denormals, unnormals, other exponents, m64
pseudo-NaNs, FLD, unmasked delivery and other controls/status remain
unsupported. The existing m32 pseudo-NaN slice is unchanged.

Coverage crosses four nonzero fractions, both signs, four FCWs, both forms,
all TOP values, 64 sticky combinations and both prior C1 values (65,536
cases), with independent Word/status expectations. Rejections cover malformed
classes/tags, excluded width, forbidden controls/status, and atomic denied-write
and canonical-range failures. FNSTSW/FNCLEX and a seeded CLI fixture observe
and clear IE; JIT/native admission stays denied. Physical x86 acceptance remains
deferred to the combined final S2-31 substage under ISSUE-019. See the
[CPU contract](../specs/CPU_CONTRACT.md#local-s2-31cx-special-pseudo-nan-m16-fist),
[report](../reports/STAGE_2_REPORT.md#s2-31cx--special-tagged-pseudo-nans-through-m16-fist),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


<a id="s2-31cy-special-pseudo-nan-m64-fistp"></a>

#### S2-31cy. Convert Special-tagged pseudo-NaN through m64 `FISTP`

Only bare `DF 38` admits a Special-tagged binary80 pseudo-NaN with exponent
`0x7fff`, J=0, and a nonzero fraction. Both bit-62 variants and either sign
store Qword integer-indefinite `0x8000000000000000`, add IE, and clear C1. Add
no PE or other current flags; preserve historical sticky flags and C0/C2/C3.
Retain FCW values `0x037f`, `0x077f`, `0x0b7f`, `0x0f7f`, `FSW & 0x80c0 == 0`,
bare Long64 RAX addressing, canonical48 eight-byte preflight, one atomic
Qword write/no read, raw ten-byte source preservation, and write-before-commit
FISTP pop ordering.

Zero-fraction pseudo-infinity and canonical J=1 infinity/QNaN/SNaN keep their
existing behavior. Other exponents, pseudo-denormals, unnormals, wrong tags,
m16/m32 behavior, FLD and unmasked delivery remain outside this extension; no
generic special-class fallback is added. JIT/native admission remains denied.

Coverage crosses four literal nonzero fractions × both signs × four FCWs × all
eight TOP values × 64 sticky combinations × both old C1 values (32,768 cases)
with independent Qword and status literals. Also cover bit-62/raw-byte
preservation, TOP wrap, canaries, write/range atomicity, zero/pseudo-infinity and
canonical J=1 NaN controls, other exponents/wrong tags, m16/m32 regression,
FNSTSW/FNCLEX, a directly seeded fixture, and JIT/native denial. Mac mini and
physical x86 acceptance remain part of the combined final S2-31 substage under
ISSUE-019. See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-31cy-special-pseudo-nan-m64-fistp),
[report](../reports/STAGE_2_REPORT.md#s2-31cy--special-tagged-pseudo-nan-through-m64-fistp),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


<a id="s2-31cz-special-unnormals-m32-fist"></a>

#### S2-31cz. Convert Special-tagged binary80 unnormals through m32 `FIST`/`FISTP`

Only bare `DB 10` and `DB 18` admit a Special-tagged binary80 unnormal with
biased exponent 1 through `0x7ffe`, J=0, either sign, and any 64-bit
significand fraction. Store Dword integer-indefinite `0x80000000`, add IE,
clear C1, and add no PE/DE/UE/OE/ZE/SF/ES/B. Preserve incoming sticky flags
and C0/C2/C3. Retain FCWs `0x037f`, `0x077f`, `0x0b7f`, `0x0f7f`,
`FSW & 0x80c0 == 0`, bare Long64 RAX addressing, full canonical48 Dword
preflight, one atomic write/no read, raw ten-byte source, and write-before-
commit ordering. FIST retains TOP; FISTP pops only after a successful write.

Exponent zero and `0x7fff` paths remain unchanged, as do J=1 encodings,
pseudo-denormals, m16/m64, wrong tags and FLD. Unmasked delivery and JIT/native
admission remain unsupported. Coverage crosses exponents 1/`0x3fff`/`0x7ffe`,
four significands, both signs, all four FCWs, both forms, every TOP, all 64
sticky combinations and both prior C1 values (196,608 cases), with independent
Dword/status expectations. Also cover boundaries, other tags/classes and widths,
IE without PE/DE, raw payload/pop/TOP, canaries, denied-write/canonical-range
atomicity, FNSTSW/FNCLEX, a seeded CLI fixture and JIT/native denial. Physical
x86 and Mac mini acceptance remain part of the combined final S2-31 substage
under ISSUE-019. See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-31cz-special-unnormals-m32-fist),
[report](../reports/STAGE_2_REPORT.md#s2-31cz--special-tagged-unnormals-through-m32-fist),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).

<a id="s2-31da-special-unnormals-m16-fist"></a>

#### S2-31da. Convert Special-tagged unnormals through m16 `FIST`/`FISTP`

Only bare `DF 10` and `DF 18` admit a Special-tagged binary80 unnormal with
biased exponent 1 through `0x7ffe`, J=0, either sign, and any fraction. Store
Word integer-indefinite `0x8000`, add IE, clear C1, and add no PE/DE/UE/OE/ZE/
SF/ES/B. Preserve incoming sticky flags and C0/C2/C3. Keep the existing FCW/FSW
gates, bare Long64 RAX addressing, canonical48 Word preflight, atomic write/no
read, raw source bytes, and write-before-commit ordering. FIST retains TOP; FISTP
pops only after a successful write.

Coverage mirrors S2-31cz: 196,608 independent cases across three exponents,
four fractions, both signs, four FCWs, both forms, all TOP values, 64 sticky
combinations, and both prior C1 values. It also checks zero-significand
unnormals, boundaries, J=1, other tags/classes and widths, controls, IE without
PE/DE, status controls, raw bytes, canaries, pop behavior, denied writes,
canonical-range atomicity, FNSTSW/FNCLEX, the seeded CLI fixture and JIT/native
denial. m32 behavior remains covered by S2-31cz. No FLD composition, unmasked
delivery, physical x86 or Mac mini acceptance is claimed; those remain deferred
to the combined S2-31 acceptance under ISSUE-019. See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-31da-special-unnormals-m16-fist),
[report](../reports/STAGE_2_REPORT.md#s2-31da--special-tagged-unnormals-through-m16-fist),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).

<a id="s2-31db-special-unnormals-m64-fistp"></a>

#### S2-31db. Convert Special-tagged unnormals through m64 `FISTP`

Only bare `DF 38` admits a Special-tagged binary80 unnormal with biased
exponent 1 through `0x7ffe`, J=0, either sign, and any fraction. Store Qword
integer-indefinite `0x8000000000000000`, add IE, clear C1, and add no
PE/DE/UE/OE/ZE/SF/ES/B. Preserve incoming sticky flags and C0/C2/C3, raw source
bytes, FCW/FSW gates, bare Long64 RAX addressing and canonical48 Qword
preflight. A successful conversion performs one atomic write without a read,
then empties the old physical TOP, increments TOP and commits FIP/FDP/RIP.

Coverage has 98,304 independent cases across three exponents, four fractions,
both signs, four FCWs, all TOP values, 64 sticky combinations and both prior C1
values. Additional checks cover zero-significand unnormals, exponent/J/tag
boundaries, other widths and forbidden controls/status, fault atomicity,
FNSTSW/FNCLEX, a seeded CLI fixture, and JIT/native denial. Non-pop m64,
FLD composition, unmasked delivery, physical x86 and Mac mini acceptance are
outside this slice; final host acceptance remains under ISSUE-019. See the
[CPU contract](../specs/CPU_CONTRACT.md#local-s2-31db-special-unnormals-m64-fistp),
[report](../reports/STAGE_2_REPORT.md#s2-31db--special-tagged-unnormals-through-m64-fistp),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).
## S2-31do — canonical binary80 subnormal through bounded FRNDINT

For Long64 bare `D9 FC`, admit only a Special-tagged old physical ST(0) whose
raw encoding is a canonical nonzero binary80 subnormal (biased exponent zero,
J=0, nonzero fraction). Preserve existing pseudo-denormal, malformed, class,
tag, control and status rejection boundaries. All admitted magnitudes are
below one-half: RN/RZ yield signed zero; RD yields -1 for negative and +0 for
positive; RU yields +1 for positive and -0 for negative. Each operation adds
sticky DE and PE, with C1 set only for ±1. Zero results become Zero-tagged;
one results become Valid-tagged. Preserve C0/C2/C3, TOP, unrelated state,
metadata and the existing all-masked/status gates. Propagate current DE only
through the local FRNDINT preflight/commit tuple.

Add an independent min/max/two-interior fraction × sign × RC × TOP × prior
DE/PE/C1 matrix, legal sticky IE/SF pairings, full canaries, repeat FRNDINT,
FNSTSW/FNCLEX and atomic malformed/pseudo-denormal/tag/control/status rejects.
Add one literal seeded `D9 FC DF E0` fixture with explicit project-fixture
provenance. Keep JIT/native admission closed; physical x86 and Mac mini checks
remain for the combined final S2-31 acceptance substage and must not be run as
part of this implementation substage.

## S2-31dp — Special-tagged binary80 pseudo-denormal through bounded FRNDINT

Extend the exact Special-tagged finite input branch for bare Long64 `D9 FC` to
accept pseudo-denormals (biased exponent zero, J=1, any fraction including
zero). Use Intel's effective exponent -16382. Match the tiny-value arithmetic
behavior already implemented for canonical subnormals: RN/RZ signed zero, RD
negative→-1 otherwise +0, RU positive→+1 otherwise -0; add sticky DE and PE,
set C1 only for ±1, and assign Zero/Valid tags. Preserve the existing FCW,
status and atomic commit gates. Do not alter decoder/classification or admit
pseudo-infinity/NaN, malformed encodings or wrong tags. Unnormal inputs were
outside this stage and receive a separate masked-invalid slice in S2-31dq
below.

Compare raw pseudo-denormals against equivalent E=1 canonical Normals across
fraction 0/1/interior/max, sign, all RC modes, TOP, sticky flags and prior C1:
result bytes, result tags, PE and C1 must match, with DE as the only new
difference. Include atomic neighboring rejects and repeat/FNSTSW/FNCLEX
coverage. Add a literal project-fixture `DB 28 D9 FC DF E0` sequence if the
already bounded x87-state fixture profile accepts it; verify FLD preserves raw
E=0/J=1/Special and that DE first appears at FRNDINT. Keep JIT/native closed.
Physical x86 and Mac mini acceptance stay in the combined final S2-31 substage
under ISSUE-019.

#### S2-31dq. Special-tagged binary80 unnormal through bounded `FRNDINT`

For exact bare Long64 `D9 FC`, accept only a Special-tagged binary80 unnormal:
biased exponent 1..=`0x7ffe`, J=0, either sign, any fraction. Under the
existing four all-masked FCWs, return the fixed negative floating-point
indefinite `00 00 00 00 00 00 00 c0 ff ff`, retain Special tag, add sticky IE,
clear C1, and add no DE/PE/SF or other current exception. Preserve previously
admitted sticky IE/DE/ZE/OE/UE/PE and SF only paired with IE. Leave FCW/FSW
admission gates, TOP, other registers, FIP/RIP/retirement and atomic preflight
unchanged. Do not widen FLD, the binary80 classifier, or JIT/native admission;
wrong tags/classes, controls, fetch, STOP and budget rejects remain atomic.

Independent expected-value coverage crosses exponent 1/interior/max, fraction
0/1/bit62/max, sign, RC, TOP, legal historical sticky flags and prior C1. It
also checks fixed-result QNaN re-entry, FNSTSW/FNCLEX and neighboring class/tag
rejects. A literal seeded `D9 FC DF E0 DB E2` fixture begins with raw
Special-tagged unnormal bytes in project x87 state and is explicitly not native
evidence. Keep the distinct FISTP DE ambiguity documented under ISSUE-019;
physical x86 and Mac mini checks remain for the combined final S2-31 acceptance
substage and must not be run here.

#### S2-31dr. Special-tagged binary80 pseudo-infinity through bounded `FRNDINT`

Extend only exact bare Long64 `D9 FC`: admit physical TOP tagged Special when
binary80 has E=`0x7fff`, J=0, zero fraction and either sign. Reuse S2-31dq's
masked-invalid result: fixed negative floating-point indefinite
`00 00 00 00 00 00 00 c0 ff ff`, Special tag, sticky IE, C1 cleared, and no
new DE/PE/SF/ZE/OE/UE. Preserve prior IE/DE/ZE/OE/UE/PE and SF only with IE.
Keep the existing FCW/status/prefix/fetch/stop/budget gates, preflight,
metadata, TOP and physical-register canaries unchanged. Do not alter FLD,
binary80 classification or JIT/native admission. At this substage, pseudo-NaNs
(nonzero fraction) remained Unsupported; S2-31ds below adds them separately.
Canonical J=1 infinity/QNaN/SNaN keep existing behavior;
E=0/J=0/zero-fraction Special remains a zero/tag-mismatch reject.

Add the independent two-sign × four-RC × eight-TOP × three legal IE/SF × 32
DE/ZE/OE/UE/PE sticky × two prior-C1 matrix with complete state and memory
canaries, plus repeat FRNDINT and exact neighboring rejects. Add a literal
seeded `D9 FC DF E0 DB E2` `project_fixture` and observe/clear IE through
FNSTSW/FNCLEX; it is not hardware evidence. Run only local validation: no Mac
mini, SSH or physical x86. Keep ISSUE-019 open for remaining x87 and host
acceptance.

#### S2-31ds. Special-tagged binary80 pseudo-NaN through bounded `FRNDINT`

Extend only exact bare Long64 `D9 FC`: accept physical TOP with Special tag,
E=`0x7fff`, J=0 and any nonzero fraction, covering Q=0 and Q=1 and either
sign. Use the same masked-invalid response as S2-31dq/dr: fixed negative
binary80 indefinite `00 00 00 00 00 00 00 c0 ff ff`, Special tag, sticky IE,
C1 cleared, and no new DE/PE/SF/ZE/OE/UE. Preserve previously admitted IE,
DE/ZE/OE/UE/PE and SF only paired with IE. Canonical J=1 infinity/QNaN/SNaN
retain their existing identity or quieting behavior. Preserve FCW/status gates,
preflight atomicity, TOP, metadata and physical-register state. Do not widen
FLD, the classifier or JIT/native admission; tag mismatches and other rejected
classes remain Unsupported.

Independent literal coverage crosses fractions `1`, `0x3fff_ffff_ffff_ffff`,
`0x4000_0000_0000_0000`, `0x7fff_ffff_ffff_ffff` (Q=0 and Q=1), sign, all
four RCs, eight TOPs, three legal IE/SF combinations, 32 DE/ZE/OE/UE/PE sticky
combinations and prior C1 (49,152 full-state cases). Seeded Q=0 and Q=1
`project_fixture` cases run `D9 FC DF E0 DB E2`, observing then clearing IE;
they are project fixture evidence, not native evidence. Keep ISSUE-019 open and
do not run Mac mini, SSH or physical x86 checks for this task. Normative basis:
Intel SDM revision 093 Vol.2A FRNDINT p.3-372 and Vol.1 §8.2.2 Tables 8-3/
8-10 and Table 4-3, as detailed in the CPU contract.

#### S2-31dt. Admit historical sticky UE through exact binary32/binary64 FLD

Only exact bare `D9 00` (`FLD m32fp [RAX]`) and `DD 00` (`FLD m64fp [RAX]`)
change their source gates from `0x80dc` to `0x80cc`. This admits a pre-existing
UE and preserves it; source-only ordinary loads add no IE/DE/PE, current
nonzero subnormals add DE, and current SNaNs add IE while retaining the existing
quieting and source mapping. ZE/OE/SF/ES/B remain pre-read rejects. Preserve the
four FCWs, Empty push destination, one raw read, canonical48 preflight, C1,
TOP/tag, pointers, RIP, retirement and atomicity. FILD, m80 loads, stores,
FRNDINT, JIT and native admission are unchanged.

Validation evidence crosses zero, normal boundaries/interior, infinity, QNaN,
SNaN and nonzero subnormal sources with both widths, all four FCWs, all TOPs,
prior C1 and all 16 IE/DE/UE/PE combinations. It also covers exact UE-only
`D9 00`/`DD 00` plus FNSTSW project fixtures, current IE/DE independence,
underflow-store-to-load compositions, repeated observation/clearing, denied
reads, bad controls, occupied destinations, forbidden statuses, canonical
range and instruction-boundary atomicity. Existing v2 unsupported predicates
must use the `0x80cc` forbidden-status mask, and their rejection fixtures now
use OE rather than UE. See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-31dt-historical-sticky-ue-for-bounded-m32m64-fld), [report](../reports/STAGE_2_REPORT.md#s2-31dt--historical-sticky-ue-for-bounded-m32m64-fld), and [fixture notes](../../tests/fixtures/x86/stage2/README.md).

#### S2-31du. Admit historical sticky OE through exact binary32/binary64 FLD

Only exact bare `D9 00` and `DD 00` lower their local FSW mask from `0x80cc`
to `0x80c4`, admitting and preserving pre-existing OE together with IE/DE/UE/PE.
Keep ZE/SF/ES/B rejected before read. Preserve source mapping, current DE for
subnormals, current IE and quieting for SNaNs, FCWs, destination, one-read
behavior, canonical range preflight, C1, metadata, retirement and atomicity.
Leave every other x87 gate and JIT/native admission unchanged.

Expand the independent matrix to all 32 IE/DE/OE/UE/PE combinations for both
widths and source classes; ordinary sources add no flags, current subnormals
add DE, SNaNs add IE, and C1 clears. Check complete state, memory, read history
and canaries. Add OE-only FNSTSW fixtures and same-width FST-overflow → FLD
infinity → FNSTSW/FNCLEX compositions under nearest and directed-away rounding.
Do not claim an OE producer under toward-zero-directed modes. Add a schema
regression for the bare budget-one/no-stop Normal Unsupported case mutated to
OE-only, and change old reject fixtures to ZE. Keep ISSUE-019 open; no Mac mini,
SSH or physical x86 checks are part of this substage.

#### S2-30bj. Narrow bare `FXCH ST(1)` (`D9 C9`)

Implement only exact no-prefix Long64 `D9 C9`; adjacent D9 encodings and memory
forms remain unsupported. Require present x87 state, FCW `0x037f`,
`FSW & 0x80ff == 0`, and both physical `(TOP)` and `(TOP+1)&7` entries to have
Valid tags and raw canonical-Normal binary80 payloads. Validate both operands
before mutation, then exchange all ten bytes. Tags and TOP stay fixed; C1 clears;
no push, pop, rounding, exception, or stack-underflow response is added. Local
deterministic policy preserves C0/C2/C3, without a hardware-equality claim.
FIP records old RIP; FDP/FOP follow the existing register-only policy; RIP
advances by two and the instruction retires once. GPR, RFLAGS, and memory stay
unchanged. Missing state, bad FCW, any status bit in the rejection mask, wrong
tags, and non-Normal or malformed raw values reject atomically. Do not add
#MF/#NM/WAIT behavior or IE/SF handling.

Cover 2,048 independent full-state successes (four positive-magnitude pairs,
four sign combinations, eight TOP values, sixteen prior CC patterns), six-cell
canaries, TOP wrap, C1 clearing and CC preservation. Check double FXCH byte/tag
roundtrip with fixed TOP and per-step metadata/RIP. Reject all nine status bits
individually and together, both operand positions for Empty/Zero/Special/wrong
tags and Valid-tagged non-Normal classes, plus missing state and instruction
boundaries. The schema-v2 allowlist is exactly `d9c9`, `d9c9dfe0`, and
`d9c9d9c9dfe0`; add seeded TOP=7/C1 and wrong-tag Unsupported fixture coverage.
JIT/native remain denied. Keep ISSUE-019 open for non-Normal/Empty/stack
exceptions, wider FXCH, undefined condition codes, and final host/physical
acceptance. No Mac mini, SSH, native runtime, or physical x86 checks in this
slice.

#### S2-30bk. Expand bounded FXCH from `i=1` to all register indices

Generalize the existing helper and operation to exact no-prefix Long64
`D9 C8+i`, `i=0..7`. Use physical `(TOP+i)&7`; preflight the unique
participating physical operands before mutation and swap all ten raw bytes.
For `i=0`, inspect only TOP, retain its payload and tag, clear C1, update FIP,
advance RIP and retire once. Unused cells must not affect self-exchange
admission. Keep FCW=`0x037f`, `FSW & 0x80ff == 0`, Valid-tagged canonical-Normal
participants, the prior deterministic C0/C2/C3 and FDP/FOP policy, and the
existing absence of exception/stack/native/JIT semantics.

Cover the existing cross-product for `i=1..7` (14,336 full states), plus
1,024 self states (four magnitude choices × two signs × eight TOP × sixteen
prior CC patterns). Include malformed/Empty unused-cell canaries, source and
indexed target tags/raw rejects, all nine forbidden FSW bits, FCW and missing
state rejection, TOP/index wrap, exact decoder bytes/prefixes/neighbors/fetch,
STOP/budget/fault boundaries, self-exchange and same-index roundtrip. Schema
allowlists are limited to one indexed exchange, optional `DF E0`, or the same
indexed exchange repeated twice followed by `DF E0`; no arbitrary sequences.
Add literal self, `i=7` wrap, nonadjacent roundtrip, wrong-target-tag, and
JIT-denial fixtures. Keep ISSUE-019 open; no Mac mini, SSH, native runtime or
physical x86 checks.

#### S2-30bl. Admit canonical signed zero to bounded FXCH

Keep the exact `D9 C8+i`, FCW=`0x037f`, and `FSW & 0x80ff == 0` gates from
S2-30bk. Each participant may be either Valid-tagged canonical Normal or
Zero-tagged exact binary80 signed zero (sign arbitrary, significand and exponent
zero). For `i!=0`, preflight both participants before atomically exchanging
the complete ten-byte payloads and physical two-bit tags. For `i=0`, validate
TOP only and preserve its payload/tag; malformed unused cells remain ignored.
TOP, C0/C2/C3 local undefined policy, metadata, no-new-flags behavior, and C1
clear stay fixed. Do not expand allowed instruction streams, decoder, native or
JIT acceptance.

Retain the 15,360 Normal-only S2-30bk matrix. Add 32,256 nonzero-index mixed
states: nine ordered magnitude pairs (one Zero/Zero, four Zero/Normal, four
Normal/Zero), four sign combinations, seven indices, eight TOP values, and
sixteen prior CC patterns; add 256 self-Zero states. Assert the complete state,
raw bytes, full tag word, Zero/Normal transfers in both orders, opposite signed
zeros, wrap, double exchange, and untouched canaries. Reject malformed Zero,
Empty/Special, Valid-tagged zero, subnormal/pseudo/unnormal participants, and
invalid participants in either position atomically. For `i=0`, unrelated cells
remain unvalidated. Update only the existing schema source-rejection predicate
to admit canonical Zero and reject malformed Zero; retain the existing stream
allowlist and explicit JIT denial.

Update fixture notes, CPU contract, report, and ISSUE-019 evidence. ISSUE-019
remains open for Empty/stack faults, special classes, physical acceptance, and
other excluded behavior. No Mini, SSH, native runtime, or physical x86 checks.

#### S2-30bm. Bounded register-only `FINCSTP` / `FDECSTP`

Implement exact bare Long64 `D9 F7` FINCSTP and `D9 F6` FDECSTP only. Require
present x87 state, FCW `0x037f`, and `FSW & 0x80ff == 0`. FINCSTP changes
TOP to `(TOP+1)&7`; FDECSTP changes it to `(TOP+7)&7`. Both clear C1 and
preserve C0/C2/C3 by deterministic local policy, with no hardware-equality
claim for architecturally undefined condition codes. Preserve all eight raw
ten-byte registers and the complete packed tag word exactly; do not inspect,
reclassify, reorder or empty any entry. Add no floating exception or status
flags. FIP records old RIP, RIP advances by two and one instruction retires;
FDP/FOP retain register-only policy and GPR/RFLAGS/memory remain unchanged.

Reject unsupported prefixes/neighbors, absent state, bad FCW, all nine FSW
bits in `0x80ff`, and fetch/truncation/STOP/budget/fault boundaries atomically.
Do not add ES/B acceptance, #MF/#NM/WAIT/pending-exception or IE/SF handling.
Add 1,536 full-state success cases (two directions × eight TOPs × sixteen
prior CC patterns × six opaque tag/raw snapshots), TOP wrap, eight repeats,
inverse pairs, per-step raw/tag assertions, and composition with FXCH after a
TOP change. The schema stream allowlist adds only `d9f7`, `d9f6`,
`d9f7dfe0`, `d9f6dfe0`, `d9f7d9f6dfe0`, and `d9f6d9f7dfe0`; use literal seeded
C1/status-observation fixtures with mixed and Empty-tagged canaries and
round-trip coverage. JIT/native remain denied. Keep ISSUE-019 open for broader
x87 exceptions and physical/native acceptance; do not run Mac mini, SSH or
physical x86 checks.

#### S2-30bn. Bare FLD1 and FLDZ (bounded local constant-push slice)

Admit only bare Long64 `D9 E8` (FLD1) and `D9 EE` (FLDZ), with present x87 state, FCW exactly `0x037f`, `FSW & 0x80ff == 0`, and physical destination `(old_TOP+7)&7` tagged Empty. Preflight all conditions before mutation and treat the old Empty payload as opaque. FLD1 stores exact binary80 `0000000000000080ff3f` with Valid tag; FLDZ stores ten zero bytes with Zero tag. Set TOP to the destination, clear C1, set FIP to old RIP, advance RIP by two and retire once. Preserve all other payloads, tags, FCW, FSW bits and pointer/opcode fields under the local policy; C0/C2/C3 retention is deterministic and makes no hardware-equality claim. Reject an occupied destination atomically for every tag, with no stack-overflow flags or indefinite value. Do not model rounding, memory, flags, exceptions, WAIT, #NM, JIT or native admission. Tests cover the 1,536 success-state matrix, opaque Empty raw payloads, occupied tags, environment rejects, fill-eight/ninth-reject, compositions, decode/fetch/budget/STOP boundaries, and exact schema-v2 allowlist fixtures. ISSUE-019 remains open for Empty sources, arithmetic/Special behavior, stack exceptions and host/physical acceptance.


#### S2-30bo. Bare FLD ST(0) (bounded raw-copy push slice)

Admit only exact unprefixed Long64 `D9 C0`; keep `D9 C1..C7` unsupported. Require present x87 state, FCW exactly `0x037f`, `FSW & 0x80ff == 0`, old physical TOP tagged Valid with canonical Normal binary80 or Zero with exact signed zero, and physical `(old_TOP+7)&7` tagged Empty. Preflight source and destination before mutation. Capture and copy all ten source bytes and its tag to the destination; the old source remains intact and becomes logical ST(1) after setting TOP to the destination. Destination bytes under Empty are opaque. Clear C1, preserve C0/C2/C3 under deterministic local policy without hardware-equality claims, add no flags, set FIP to old RIP, advance RIP by two and retire once. Preserve other state and pointer/opcode fields under register-only local policy.

Reject Empty/Special/malformed/wrong-tag sources, occupied destination tags, invalid FCW/status/missing state, prefixes, neighbors, truncation/fetch, STOP, budget and faults atomically. Do not model IE/SF, indefinite or stack exceptions, #MF/#NM/WAIT, native or JIT behavior. Verify 5,120 full-state successes (five magnitudes, both signs, eight TOPs, sixteen prior CC patterns, four snapshots), source/destination preflight rejection, seven copies from one source followed by an eighth atomic rejection, FLD1/FLDZ compositions and raw store correctness. Schema streams are exactly `d9c0`, `d9c0dfe0`, `d9e8d9c0dfe0`, and `d9eed9c0dfe0`; add literal signed-zero/TOP-wrap, producer, occupied-destination, and Empty-source fixtures. Keep ISSUE-019 open; no Mini, SSH, native runtime, or physical x86 checks.


#### S2-30bp. Bare FLD ST(i), i=0..6 (bounded raw-copy push slice)

Generalize S2-30bo without duplicating its execution gate: admit only exact unprefixed Long64 `D9 C0..C6`. Source is physical `(old_TOP+i)&7`; destination remains `(old_TOP+7)&7`. Require present x87 state, FCW exactly `0x037f`, `FSW & 0x80ff == 0`, a source tagged Valid with canonical-Normal binary80 or Zero with exact signed binary80 zero, and an Empty destination. Preflight both before mutation. Capture and copy the source's exact ten bytes and tag; retain the old source and destination bytes under Empty remain opaque. Set TOP to destination, clear C1, preserve C0/C2/C3 under the established deterministic local policy, record FIP from old RIP, advance two and retire once. Keep C7, prefixes, other x87 classes, ES/B/status gates, stack exceptions and native/JIT execution denied.

Verify 35,840 full-state successes (seven indices × five magnitudes × two signs × eight TOPs × sixteen prior CC patterns × four snapshots), with source selection before TOP changes, wrap, source retention, and indices above zero independent of old ST(0). Preserve source/destination/tag/environment rejection and boundary atomicity. Schema admits only `D9 C0..C6`, each optionally followed by `FNSTSW AX`, plus the existing `FLD1`/`FLDZ` producer streams with one admitted indexed load and `FNSTSW AX`. Add one literal success fixture per new index and signed-zero/wrap, nonadjacent-source-with-Empty-ST(0), wrong-source-tag and occupied-destination cases. JIT/native and physical x86 checks remain deferred to the final shared physical substage; ISSUE-019 remains open.


#### S2-30bq. Bare FFREE ST(0) (bounded tag-only free slice)

Admit exactly bare unprefixed Long64 `DD C0`; `DD C1..C7`, prefixes, memory forms, WAIT/FWAIT and aliases stay unsupported. Require present x87 state, FCW `0x037f`, and `FSW & 0x80ff == 0`. Prior tag and all ten payload bytes at physical old TOP are opaque, including already Empty, SNaN and malformed values. Success changes only that physical tag to Empty; preserve TOP, all raw payloads, other tags, all C0..C3 (including C1 under the local undefined-field policy), GPR/RFLAGS/memory, FCW/FSW, FDP/FOP; set FIP to old RIP, advance RIP by two and retire once. The operation is idempotent and produces no exception or pending state.

Verify 3,072 full-state successes (eight TOPs × sixteen prior CC patterns × four old tags × six opaque payload canaries), full snapshot and memory preservation, nine forbidden FSW bits individually/together, bad FCW/missing state, decoder neighbors/prefix/truncation and STOP/budget/fault no-commit. Composition must show `FFREE ST(0) → FINCSTP → FLD1` reuses the freed physical cell while preserving its raw bytes until FLD1 commits. Schema streams are exactly `ddc0`, `ddc0dfe0`, `ddc0d9c0dfe0`, and `ddc0d9f7d9e8dfe0`; add literal fixtures for TOP=7/C1=1 mixed canaries, already Empty, opaque SNaN/malformed, composition, and environment rejection. Keep JIT/native/physical acceptance denied/deferred and ISSUE-019 open.

#### S2-30br. Generalize bounded FFREE to all `ST(i)` indices

Generalize the tag-only operation to exact unprefixed Long64 `DD C0..C7` and
carry `i=0..7` in the decoded operation. Select physical `(old_TOP+i)&7`;
require present x87 state, FCW `0x037f`, and `FSW & 0x80ff == 0`. The selected
tag and all ten payload bytes are opaque. Success sets only that physical
two-bit tag to Empty, preserving TOP, every payload and other tag, full FCW/FSW
including C0..C3, FDP/FOP and all non-x87 state; record FIP=old RIP, advance
RIP by two and retire once. Repeated FFREE is idempotent. Do not classify or
mutate payload, pop, raise exceptions, create pending state or admit JIT/native
execution.

Cover the full 24,576-state matrix (eight indices × eight TOPs × sixteen prior
CC combinations × four tags × six opaque payloads), retaining the earlier
3,072 `ST(0)` cases. Include wrap for index/TOP, one-tag-only mutation and
repeated operations. Reject each forbidden FSW bit individually and together,
bad FCW and missing state atomically; retain decoder, fetch, prefix, memory,
WAIT/FFREEP, budget and stop-boundary denials. Schema admits exactly nineteen
streams: each bare `DD C0..C7` at budget one and each followed by `DF E0` at
budget two, plus `DD C0 D9 C0 DF E0` (FFREE commits then FLD rejects),
`DD C0 D9 F7 D9 E8 DF E0`, and `DD C7 D9 E8 DF E0`. Add seven indexed literal
fixtures covering TOP wrap, C1=1, mixed tags, opaque SNaN/malformed payloads,
and `ST(7)→FLD1` physical reuse, plus indexed environment rejection. The CLI
checks snapshots and JIT denial. Keep ISSUE-019 open; no Mini, SSH, native or
physical x86 tests are included.

#### S2-30bs. Bare FNOP with a bounded environment gate

Admit only unprefixed Long64 `D9 D0`, requiring present x87 state, FCW exactly
`0x037f`, and `FSW & 0x80ff == 0`. FNOP is a local state-preserving no-op except
FIP=old RIP, RIP+=2, and one retirement. Do not inspect TOP, tags, or any raw
payload; retain TOP, FTW, raw values, FCW/FSW including C0..C3 and C1, FDP/FOP,
GPR, RFLAGS, MXCSR and pending state. Do not introduce exception, pending, data
memory, JIT or native behavior.

Cover 4,608 full snapshots (eight TOPs × sixteen CC patterns × six tag patterns
× six raw layouts), all nine forbidden FSW bits individually and together,
invalid FCW variants, missing x87 state, decode/prefix/WAIT neighbors, fetch,
truncation, STOP and budget atomicity, repeated FNOP and FFREE→FNOP→FLD1
composition. Schema allows only `d9d0`, `d9d0dfe0`, `d9d0d9d0dfe0`, and
`ddc7d9d0d9e8dfe0`; bad-environment fixtures use only bare `d9d0` at budget
one. Add local CLI fixtures for TOP=7/C1=1 opaque values, all Empty, repeat,
composition, ES|B and invalid FCW; explicitly deny JIT. Physical x86 acceptance
remains deferred under ISSUE-019.
#### S2-30bt. Bounded bare FXAM (Normal and signed-zero classification)

Admit only exact unprefixed Long64 `D9 E5` with present x87 state, FCW `0x037f`, and `FSW & 0x80ff == 0`. Physical old TOP may be Valid-tagged canonical Normal or Zero-tagged exact signed binary80 zero. Normal maps to `C3:C2:C0 = 010` (`FSW` mask `0x0400`), zero maps to `100` (`0x4000`), and C1 is replaced with the source sign. Replace all C0/C1/C2/C3 using `(old_FSW & !0x4700) | result`; preserve TOP and every other x87 byte/field and machine state. Set FIP to old RIP, advance RIP by two and retire once. Reject other tags/classes, malformed payloads, invalid FCW/status, missing state, prefixes, neighbors, truncation/fetch and execution boundaries atomically. Keep FTST's admission gate unchanged; do not model Empty/Special FXAM outputs, exceptions, native execution or JIT.

Verify 5,120 full-state successes (four canonical Normal representatives: minimum normal, 1, 1+ULP and maximum finite; plus exact Zero; both signs, eight TOP values, sixteen old CC combinations, four non-source canary layouts) and 10,752 rejected class/tag combinations (42 × two signs × eight TOPs × sixteen old CC patterns), plus environment and boundary rejects. Schema allows exactly `d9e5`, `d9e5dfe0`, `d9e5d9e5dfe0`, `d9e8d9e5dfe0`, `d9eed9e5dfe0`, `d9e8d9e0d9e5dfe0`, and `d9eed9e0d9e5dfe0`. Literal fixtures cover signed Normal/TOP wrap, FNSTSW, repeat, FLD1/FLDZ producer composition with and without FCHS, wrong tag, Empty, Special, ES|B and invalid FCW. JIT/native remain denied. ISSUE-019 stays open; do not run Mac mini, SSH or physical x86 checks.

#### S2-30bu. Bare `FCMOVB ST(0), ST(1)`

Implement only `DA C1` in exact unprefixed Long64 form. Preflight Valid+canonical
Normal or Zero+exact signed zero for both old physical TOP and `(TOP+1)&7`
before reading CF, even on the untaken path. CF=1 copies raw binary80 bytes and
tag; CF=0 leaves both operands untouched. Preserve TOP, complete FSW under a
local incoming `C1=0` restriction, and all unrelated state; update FIP/RIP and
retirement only. Preserve existing `DA E9` behavior. No memory, pop, exception,
JIT, native, or physical claim.

Acceptance: 51,200 full-state successes using minimum Normal, 1, 1+ULP and
maximum finite in both signs plus exact ±0, and 21,504 invalid class/tag rejects,
plus environment and boundary rejects. Schema allows only the five exact streams
and budgets recorded in the CPU contract. Literal CLI fixtures cover both CF
outcomes, tags, signed zero, TOP wrap, repeat, both real FLD1/FCHS/FCOMI
producers, invalid source with CF=0, invalid destination, C1 and FCW rejection.
Keep JIT/native closed and record remaining x87 limitations under ISSUE-019.
This S2-30bu ST(1)-only register extent is historical and is superseded by
S2-30bv for source indices `i=0..7`.

#### S2-30bv. Indexed bare `FCMOVB ST(0), ST(i)`

Extend S2-30bu only to exact unprefixed Long64 `DA C0..C7`; the low three
ModR/M bits select source `i`. Preserve the S2-30bu gate: x87 state present,
FCW exactly `0x037f`, `FSW & 0x82ff == 0`, and each inspected participant is
Valid-tagged canonical Normal or Zero-tagged exact signed zero. For `i>0`,
preflight physical old TOP and `(old_TOP+i)&7` before reading CF, including
when CF is clear. For `i=0`, inspect only old TOP; never inspect ST(1) or any
other tag/payload. Both self-index outcomes preserve that slot's bytes/tag.
When CF is set for `i>0`, copy the source's exact ten bytes and tag to old
TOP; otherwise preserve both operands. Preserve all other state except FIP,
RIP and retirement. Keep `DA E9` unchanged and C1=1 unsupported.

Acceptance: 363,520 success snapshots (51,200 each for `i=1..7`, plus 5,120
self-index cases) and 161,280 rejected class/tag snapshots (21,504 each for
`i=1..7`, plus 10,752 self-index cases), plus per-index environment and
execution-boundary checks. Successes cover the four canonical Normal values
in both signs and exact ±0, all TOP values, CF, C0/C2/C3, and PF/ZF. Rejections
cover malformed encodings, Empty/Special and mismatched tags even when CF=0.
The exact schema admits the eight bare bytes and eight corresponding
FNSTSW-terminated streams, plus the S2-30bu repeat and two producer streams
(19 exact streams total). Literal CLI fixtures exercise every index, TOP wrap,
self-index with opaque unrelated registers, signed-zero/raw-tag copying,
invalid source under CF=0 and indexed environment rejection. Local JIT admission
remains denied; no native or physical acceptance is claimed. See
[CPU contract](../specs/CPU_CONTRACT.md#local-s2-30bv-bounded-indexed-fcmovb-sti-da-c0c7),
[report](../reports/STAGE_2_REPORT.md#s2-30bv--bounded-indexed-fcmovb-sti), and
[fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30bv-bounded-indexed-fcmovb-sti).

#### S2-30bw. Bare `FCMOVNB ST(0), ST(1)`

Admit exactly unprefixed Long64 `DB C1`. Reuse the bounded FCMOV operand
preflight: x87 state must exist, FCW is `0x037f`, `FSW & 0x82ff == 0`, and
both physical old TOP and `(old_TOP+1)&7` must be Valid-tagged canonical
Normal or Zero-tagged exact signed binary80 zero. Preflight both slots before
reading CF, including the untaken path. If CF=0, copy the source's ten raw
bytes and tag to TOP; if CF=1, preserve both operands. Preserve all remaining
x87 state, RFLAGS and machine state; set FIP to old RIP, advance RIP by two,
and retire once. Keep FCMOVB and all other DB forms unchanged. Exclude memory,
exceptions, native execution and JIT.

Acceptance includes 51,200 successful state pairs and 21,504 rejected class/tag
cases, plus environment, decode/prefix, fetch, STOP and budget boundaries. The
property test compares FCMOVNB at CF=x to FCMOVB at CF=!x over the complete
success matrix. Schema allows only `dbc1`, `dbc1dfe0`, `dbc1dbc1dfe0`,
`d9e8d9e8d9e0dbf1dbc1dfe0`, and `d9e8d9e0d9e8dbf1dbc1dfe0` with budgets
1/2/3/6/6. Literal fixtures cover taken/untaken, distinct tags, signed zero,
TOP wrap, repeat, both producer streams, invalid source under CF=1, invalid
destination, C1 and bad FCW. JIT remains denied; ISSUE-019 remains open.

#### S2-30bx. Indexed bare `FCMOVNB ST(0), ST(i)`

Admit exactly unprefixed Long64 `DB C0..C7`, carrying the low ModR/M bits as
`i`. Require present x87 state, FCW `0x037f`, and `FSW & 0x82ff == 0`.
Preflight old physical TOP and `(old_TOP+i)&7` before reading CF for `i>0`;
for `i=0`, validate only old TOP and do not inspect any other physical slot.
Each required operand must be Valid-tagged canonical Normal or Zero-tagged
exact signed binary80 zero, even when CF=1. CF=0 copies all ten source bytes
and its tag to TOP; CF=1 leaves operands unchanged. Preserve all state except
FIP, RIP and retirement. Keep C1=0 and C0/C2/C3 preservation policy. No
memory form, exception delivery, native execution or JIT lowering is included.

Acceptance is 363,520 success snapshots comparing full x87 state, RFLAGS and
RIP (51,200 per `i=1..7`, 5,120 for self-index `i=0`), plus 161,280 invalid
class/tag snapshots comparing complete machine state (21,504 per nonzero
index, 10,752 for self-index). The complement property compares
each indexed FCMOVNB form with the matching FCMOVB form under inverted CF,
and an independent expected-state snapshot is checked. Per-index environment,
missing-x87, fetch, prefix, STOP and budget boundaries are covered. The schema
admits exactly 19 streams: each `dbc0..dbc7` with budget 1 and each followed
by `dfe0` with budget 2, plus `dbc1dbc1dfe0` and the two existing producer
streams with budgets 3/6/6. Literal CLI fixtures cover all eight indices,
including both self-index CF outcomes, TOP wrap, Normal↔Zero copies, signed
zero, invalid-source CF=1 and indexed environment rejection. Runner snapshots
check interpreter results and deny JIT. The former S2-30bw DB C1-only limit
is historical and superseded here. ISSUE-019 remains open.

#### S2-30bz. Bare `FCMOVE ST(0), ST(i)` (`DA C8..CF`)

Admit exact unprefixed Long64 `DA C8..CF`; the low ModR/M bits select `i`.
Require x87 state, FCW `0x037f`, and `FSW & 0x82ff == 0`. For `i>0`, preflight
physical old TOP and `(old_TOP+i)&7` before reading integer RFLAGS.ZF, including
when ZF is clear. For `i=0`, validate only old TOP and leave every other slot
opaque. Each required operand must be Valid-tagged canonical Normal or
Zero-tagged exact signed binary80 zero. When ZF=1 copy the source's exact ten
bytes and tag to old TOP; when ZF=0 preserve operands. Preserve TOP, full
FSW/FCW, integer flags and all other state except FIP=old RIP, RIP+2 and one
retirement. C1=1 remains unsupported. Keep FCMOVB and FUCOMPP unchanged. No
memory, exceptions, JIT or native execution is included.

Cover 363,520 success snapshots (51,200 for each `i=1..7`, 5,120 for `i=0`)
and 161,280 invalid class/tag snapshots (21,504 for each nonzero index,
10,752 for self-index), plus per-index environment and execution boundaries.
The schema allows exactly 19 streams: all eight bare encodings and their
`DF E0` observation forms, plus the existing repeat and producer streams.
Literal fixtures cover all indices, both self-index ZF outcomes, TOP wrap,
tag/raw payload copying, negative zero, invalid source with ZF=0 and indexed
environment rejection. Runner checks complete x87 snapshots and explicitly
checks the repeat's second transfer; JIT remains denied. The former ST(1)-only
limit is historical. ISSUE-019 remains open.

#### S2-30ca. Bare `FCMOVNE ST(0), ST(1)` (`DB C9`)

Historical single-index scope; superseded by S2-30cb below.

Admit only exact unprefixed Long64 `DB C9`. Reuse the bounded FCMOV operand
preflight for physical TOP and TOP+1, before reading ZF even when the move is
untaken. When ZF=0, copy the source's exact binary80 bytes and tag to TOP; when
ZF=1, preserve both operands. Keep FCW `0x037f`, `FSW & 0x82ff == 0`, C1=1
rejection, full-state preservation, FIP/RIP/retirement, no memory, no exceptions,
and no JIT/native execution. Validate the 51,200-success and 21,504-rejection
matrices, exact schema streams, literal fixtures, runner snapshots and JIT
denial before marking complete. ISSUE-019 remains open.

#### S2-30cb. Indexed bare `FCMOVNE ST(0), ST(i)` (`DB C8..CF`)

Admit only the exact unprefixed Long64 `DB C8..CF` forms and decode `i` from
the low ModR/M bits. Reuse the indexed FCMOV preflight: for `i>0`, validate
old physical TOP and `(TOP+i)&7` before consulting integer ZF, including when
ZF=1; for `i=0`, validate only TOP. Both cells must have the exact canonical
Valid/Normal or Zero/exact-signed-zero pairing. Keep FCW `0x037f`,
`FSW & 0x82ff == 0`, C1=1 rejection, and atomic failure. ZF=0 copies the
source's raw ten bytes and tag to TOP; ZF=1 preserves both. All state other
than FIP/RIP/retirement remains unchanged; JIT/native stay closed.

Validate 363,520 successes and 161,280 invalid class/tag rejects, the exact
19-stream schema, all source indices including self-index and wrap, repeat and
FCOMI producer streams, literal state snapshots, environment/status/boundary
rejections, complement behavior against indexed FCMOVE, and per-index JIT
denial. Local results do not close ISSUE-019 or establish physical behavior.

#### S2-30cc. Historical exact bare `FCMOVBE ST(0), ST(1)` (`DA D1`)

Admit only the unprefixed Long64 two-byte `DA D1` encoding. Reuse the bounded
FCMOV physical TOP/TOP+1 preflight and require both participants to be
Valid-tagged canonical Normal or Zero-tagged exact signed binary80 zero before
reading integer RFLAGS, even when `CF=ZF=0`. Take the raw ten-byte/tag copy
exactly when integer `CF || ZF`; otherwise preserve both participants. Require
FCW `0x037f` and `FSW & 0x82ff == 0`; preserve all other state except FIP/RIP
and retirement. No FSW condition bits, memory form, exception delivery, JIT,
native execution or physical acceptance are included.

Validate 51,200 complete success snapshots (10×10 values, four CF/ZF rows,
both PF values, eight TOP values and eight C0/C2/C3 combinations) and 86,016
invalid class/tag snapshots across all four conditions. Also check missing
state, all FCW/FSW gates, execution boundaries and exact prefix/neighbor
rejection. The fixture schema admits exactly six streams: bare, FNSTSW
observation, repeat, and three FCOMI producers for greater, equal-then-FCHS,
and less. Literal full-state fixtures cover all truth-table rows, signed-zero
raw/tag transfer, TOP wrap, both repeated steps, all three integer-flag
producers, invalid participants and environment rejection. The fixture runner
checks complete state/memory and denies JIT. Local evidence does not resolve
ISSUE-019 or establish physical behavior.

#### S2-30cd. Indexed bare `FCMOVBE ST(0), ST(i)` (`DA D0..D7`)

Extend the prior S2-30cc form to exact unprefixed Long64 `DA D0..D7` and carry
`ModR/M & 7` in `Operation`. Keep the same present-x87, FCW `0x037f`, and
`FSW & 0x82ff == 0` gates. Before reading integer flags, preflight old TOP and
`(TOP+i)&7` for i>0; for i=0 preflight only TOP and treat every other stack
slot as opaque. Each used value is Valid-tagged canonical Normal or
Zero-tagged exact signed binary80 zero, even when untaken. Copy raw ten bytes
and tag to TOP exactly when `CF || ZF`; otherwise preserve the participants.
All state except FIP/RIP/retirement remains unchanged. C1=1, memory forms,
exception delivery, JIT, native and physical acceptance remain excluded.

The local matrices contain 363,520 complete success snapshots (7×51,200
indexed cases plus 5,120 self-index cases) and 645,120 invalid class/tag
snapshots (7×86,016 indexed cases plus 43,008 self-index cases). The exact
schema admits twenty streams: each bare DA D0..D7 and its FNSTSW AX form,
the existing DAD1 repeat, and the three existing FCOMI producers. Literal
fixtures cover each index, all four self-condition rows, wraparound, signed
zero/tag transfer, untaken invalid operands, bad environment, repeat, and
the retained producers. Runner assertions compare full machine state and
memory and deny JIT. Local evidence does not resolve ISSUE-019 or establish
physical x86 behavior.

#### S2-30ce. Exact bare `FCMOVNBE ST(0), ST(1)` (`DB D1`)

Admit only the exact unprefixed Long64 `DB D1` form. Require present x87
state, FCW `0x037f`, and `FSW & 0x82ff == 0`. Preflight physical TOP and
`(TOP+1)&7` before reading integer flags, whether the instruction is taken or
not. Each participant must be Valid-tagged canonical Normal binary80 or
Zero-tagged exact signed zero. Copy the source's raw ten bytes and tag to TOP
only when both integer CF and ZF are clear (`!(CF || ZF)`); otherwise preserve
the destination. Preserve all other state except FIP=old RIP, RIP+=2, and one
retirement. Rejections are atomic. C1=1, all other DB D0..D7 forms, prefixes, memory forms, x87 exception
delivery, JIT, native execution and physical acceptance were excluded from
this historical ST(1)-only slice; S2-30cf extends it to D0..D7.

The interpreter matrix contains 51,200 complete success states (10×10 values,
four CF/ZF rows, both PF values, eight TOP values and eight C0/C2/C3 patterns),
including 12,800 taken and 38,400 untaken cases. An additional matrix checks
86,016 invalid class/tag operand snapshots, including untaken conditions. The
test suite explicitly verifies the complementary truth table against FCMOVBE
at identical CF/ZF values. Schema v2 admits exactly six streams: `dbd1` (1),
`dbd1dfe0` (2), `dbd1dbd1dfe0` (3), and the three six-instruction FCOMI
producer streams for greater, equal-then-FCHS, and less. Literal fixtures cover
the CF/ZF rows, signed-zero/tag transfer with TOP wrap, both repeat steps,
producer truth cases, invalid untaken source, and bad environment. The runner
compares full machine and memory snapshots and denies JIT. Local evidence does
not resolve ISSUE-019 or establish physical x86 behavior.


#### S2-30cf. Indexed bare `FCMOVNBE ST(0), ST(i)` (`DB D0..D7`)

Extend the bounded S2-30ce behavior to exact unprefixed Long64 `DB D0..D7`.
Keep the existing x87 gates: x87 state present, FCW `0x037f`, and
`FSW & 0x82ff == 0`. Preflight old physical TOP and `(TOP+i)&7` before reading
integer CF/ZF when i>0; self-index i=0 validates only TOP and leaves all other
slots opaque. Participants are Valid canonical Normal or Zero exact signed
zero. Copy raw ten-byte payload and tag to TOP only when CF=ZF=0. Each valid
instruction preserves all state except FIP=old RIP, RIP+=2, and one retirement;
invalid operands, environment, decode, and execution boundaries remain atomic.
C1=1, exception delivery, JIT/native execution and physical acceptance remain
outside this slice.

Retain the S2-30ce ST(1) matrices, repeat, and three FCOMI producer streams.
Add 363,520 complete success snapshots (7×51,200 indexed plus 5,120 self)
and 645,120 invalid class/tag snapshots (7×86,016 indexed plus 43,008 self).
Require the 20 exact schema streams: DB D0..D7 alone (budget 1), each followed
by FNSTSW AX (budget 2), the DB D1 repeat (budget 3), and the three existing
producer streams (budget 6). Literal fixtures cover indices 0 and 2..7, all
self CF/ZF rows, wraparound, Normal/Zero and signed-zero transfer, invalid
source under taken and untaken conditions, and environment/status/decode and
boundary rejection. Compare full machine/memory snapshots and deny JIT for
each index. This remains local interpreter evidence under ISSUE-019.


#### S2-30cg. Historical exact bare `FCMOVU ST(0), ST(1)` (`DA D9`)

Admit only exact unprefixed Long64 `DA D9`. Reuse the existing FCMOV gate:
require present x87 state, FCW `0x037f`, `FSW & 0x82ff == 0`, and Valid
canonical-Normal or Zero exact-signed-zero operands at physical TOP and
TOP+1. Preflight both participants before reading integer PF, including when
PF=0. Copy the source's raw ten bytes and tag to TOP only when PF=1; otherwise
preserve the destination. Preserve all other state except FIP=old RIP, RIP+=2
and one retirement. Invalid state/environment/operands and execution boundaries
remain atomic. DA D8 and DA DA..DF, every prefix, memory form, unordered/NaN
input, x87 exception delivery and physical x86 acceptance remain outside scope;
native and JIT admission stay closed.

The original local implementation covered 512 success and 768 invalid
class/tag snapshots, and admitted only the five-instruction producer stream.
That ST(1)-only slice is historical and is superseded by S2-30ch. Its two
literal TEST producer fixtures remain compatibility coverage; unordered x87
values and physical hardware were not covered.

#### S2-30ch. Indexed bare `FCMOVU ST(0), ST(i)` (`DA D8..DF`)

Extend the bounded FCMOVU operation to every exact unprefixed Long64 encoding
`DA D8..DF`, carrying `i` in the decoded operation. Reuse the strict FCMOV
preflight gate: require x87 state, FCW `0x037f`, `FSW & 0x82ff == 0`, and Valid
canonical-Normal or Zero exact-signed-zero participants. For `i>0`, validate
old physical TOP and `(old_TOP+i)&7` before reading PF, even when untaken; for
`i=0`, validate only TOP and preserve that cell without inspecting other
registers. PF=1 copies the raw ten bytes and tag; PF=0 preserves the destination.
Keep C1 closed, reject Special/NaN values, and preserve all state except
FIP=old RIP, RIP+=2 and retirement. Rejections are atomic.

Cover exactly 363,520 success and 161,280 invalid class/tag snapshots across
all indices, values/signs, PF, CF/ZF, TOP and x87 CC patterns. Check missing
state, FCW/FSW, decode and execution boundaries. The exact 18-stream schema
allows eight bare forms (budget 1), each followed by FNSTSW (budget 2), the
DA D9 repeat (budget 3), and the existing TEST producer (budget 5). Add nine
bare success fixtures, a repeat, two retained producer fixtures, invalid
sources under PF=0/1, invalid destination and C1/ES/B/FCW rejects. Compare
full state and memory and deny JIT. This substage excludes Mac mini, SSH,
native and physical checks and leaves ISSUE-019 open.

#### S2-30ci. Historical exact bare `FCMOVNU ST(0), ST(1)` (`DB D9`)

Admit only exact unprefixed Long64 `DB D9`. Reuse the bounded FCMOV preflight:
require x87 state, FCW `0x037f`, `FSW & 0x82ff == 0`, and both old physical
TOP and TOP+1 participants tagged Valid canonical Normal or Zero exact signed
zero. Preflight both operands before PF even when untaken. PF=0 raw-copies the
source's ten bytes and tag into TOP; PF=1 preserves the destination. Reject C1=1 through the existing gate; preserve C1=0 and all other machine/x87
state except FIP, RIP and retirement. Reject atomically; add no other FCMOVNU encoding or
unordered/NaN semantics.

Cover exactly 51,200 success snapshots (10 destination values × 10 source
values × 2 PF × 4 CF/ZF patterns × 8 TOP × 8 x87 CC patterns), split evenly
between taken and untaken, and 21,504 invalid participant class/tag snapshots
(42 pairs × 2 signs × 2 PF × 2 participant positions × 8 TOP × 8 CC patterns).
Also cover each forbidden status bit and combinations, invalid source under
PF=1, both invalid participants, missing state, bad FCW, wrapping TOP, raw
signed-zero/tag transfer, repeat, prefixes/neighbors/truncation, STOP/budget
and no-commit boundaries, and no data-memory access. Compare FCMOVNU PF=x with
FCMOVU PF=!x using independently built complete expected snapshots.

The schema admits exactly `dbd9` (budget 1), `dbd9dfe0` (2),
`dbd9dbd9dfe0` (3), and `d9e8d9ee4885c0dbd9dfe0` (5). Add literal success,
invalid-source, repeat, and both TEST producer fixtures. The producer cases use
RAX=1 (PF=0, ZF=0, taken) and RAX=3 (PF=1, ZF=0, untaken); check each
intermediate state, FCMOV at RIP+7 and final RIP+11. Deny JIT. Update ISSUE-019
with local results; the shared Mac mini/SSH/native/physical validation substage
remains deferred and ISSUE-019 stays open. S2-30cj supersedes the one-index
extent; the original fixtures remain historical compatibility evidence.

#### S2-30cj. Indexed bare `FCMOVNU ST(0), ST(i)` (`DB D8..DF`)

Admit only exact unprefixed Long64 `DB D8..DF` and retain `ModR/M & 7` as the
source index. Require present x87, FCW `0x037f`, `FSW & 0x82ff == 0`, and each
used physical participant to be Valid with canonical Normal binary80 or Zero
with exact signed zero. For `i>0`, validate old TOP and `(TOP+i)&7` before PF,
including untaken executions. For `i=0`, validate TOP alone and do not inspect
other cells; the self operation never changes payload or tag. PF=0 copies the
source's ten raw bytes and tag into TOP; PF=1 preserves the destination. Keep
C1 closed and exclude Special/NaN semantics. Preserve all other state/memory
except FIP=old RIP, RIP+=2 and retirement; rejects are atomic.

Cover exactly 363,520 success snapshots (7×51,200 for `i>0`, plus 5,120
self-index) and 161,280 invalid participant class/tag snapshots (150,528 for
`i>0`, plus 10,752 self-index). Check all PF, CF/ZF, TOP and x87 CC patterns,
environment/status gates, missing x87, index wrap, signed-zero and raw tag
copy, repeat, no data access, execution boundaries and independently built
FCMOVU complement snapshots.

The schema admits exactly 18 streams: eight bare DB D8..DF forms (budget 1),
each followed by FNSTSW AX (budget 2), DB D9 repeat plus FNSTSW (budget 3),
and the existing TEST producer (budget 5). Add nine bare success fixtures
covering indices 1..7 and self PF=0/1, plus invalid source under PF=0/1,
invalid destination, C1/ES/B/FCW rejects. Preserve the historical repeat and
both TEST producers. Compare full state and memory, deny JIT, and update
ISSUE-019 with local results. Mac mini, SSH, native and physical checks remain
for the shared final acceptance substage.


#### S2-18j — bare CLD/STD (локальный профиль)

Только Long64 primary-map bare FC/FD, длина 1, без префиксов, ModR/M и
immediate. CLD очищает только DF, STD устанавливает только DF; каждый
успешный шаг увеличивает RIP на один и retire на один. Не обращается к
данным, GPR, segment/x87 или guest pointers. `FlagEffects` определяет только
DF и сохраняет остальные definedness bits. Все одиночные префиксы отвергаются
как вне профиля; это admission boundary, не утверждение о #UD или precedence.
`D9 FC` остаётся FRNDINT. Native/JIT/physical acceptance не входят; S2-18
остаётся открытой.

#### S2-18l — non-REP STOS с address-size 32

Добавить только Long64 non-REP `67 AA`, `67 66 AB`, `67 AB`, `67 48 AB`.
Стартовый адрес — zero-extend(EDI), запись выполняется одной непрерывной
width-sized операцией без побайтного wrap на 4 GiB; после успеха RDI получает
zero-extended `(EDI ± width) mod 2^32` согласно DF. RAX целиком сохраняется,
как и остальные GPR/flags/definedness/segment/x87. Ошибки записи атомарны в
локальной модели: полный CPU/RAM pre-state сохраняется, retirement равен 0;
архитектурный fault остаётся гостевым, backend error остаётся HarnessError.
Это bounded local policy под ISSUE-016, не hardware restart claim.

Покрытие: полная матрица core состояний, границы, store failures, prefix
admission и ровно 43 literal schema-v2 fixtures с полными expected snapshots;
runner проверяет snapshots/RAM, retirements и закрытый JIT, Linux-only native
test отвергает четыре формы до tracee. Mac mini/native физическая проверка
остаётся объединённой в общем финальном substage. ISSUE-016/020/021 остаются
открытыми; S2-18 и Stage 2 в целом остаются открытыми.

#### S2-18m — non-REP SCAS с address-size 32

Добавить только Long64 non-REP `67 AE`, `67 66 AF`, `67 AF`, `67 48 AF`.
Чтение выполняется одним непрерывным width-sized доступом из zero-extend(EDI);
побайтного wrap на 4 GiB нет. После успешного сравнения с аккумулятором
обновить шесть арифметических flags/definedness по CMP, затем zero-extend RDI
после `(EDI ± width) mod 2^32` по DF. RAX и прочие регистры сохраняются.
Произвольные входные arithmetic flags/definedness для non-REP SCAS допустимы;
REPE entry gate сюда не переносится. Ошибка чтения сохраняет полный state,
flags-definedness и RAM; backend errors остаются HarnessError. Это один
read-only профиль и он не закрывает ISSUE-016/020/021.

Покрытие: 18,432 полных success states, 512 definedness вариантов, 96
address-boundary cases, 96 ошибок чтения, 73 prefix rejects и 39 literal
schema-v2 fixtures. Проверить TEST→SCAS success/fault, CLD/STD, бюджет/fetch,
normalization, RCX=0 и прежние SCAS/REPE regressions. JIT остаётся закрытым;
Linux-only native admission отклоняет четыре формы до tracee. Mac mini/native
физическая проверка остаётся единой в общем финальном substage; S2-18 и Stage
2 остаются открытыми до acceptance.

#### S2-18n — non-REP CMPS с address-size 32

Добавить только Long64 non-REP `67 A6`, `67 66 A7`, `67 A7` и `67 48 A7`.
Начальные адреса — zero-extend(ESI) и zero-extend(EDI); выполнить два
непрерывных width-sized чтения в порядке flat DS:[ESI], затем flat ES:[EDI].
Отдельные байты операнда не оборачиваются на границе 4 GiB. После двух
успешных чтений вычислить `memory[ESI] - memory[EDI]`, зафиксировать CMP flags
и zero-extended `(ESI/EDI +/- width) mod 2^32` по DF. Ошибка любого чтения не
коммитит CPU state; ошибка второго сохраняет первое чтение в локальном trace.
Missing mapping и backend errors остаются HarnessError. ESI-first — только
локальная политика; dual-invalid hardware priority и MMIO rollback не
утверждаются (ISSUE-020).

Покрытие: 63 literal schema-v2 fixtures: 8 success, 8 unmapped, 4 budget-zero,
16 pointer wraps, 6 непрерывных чтений через 4 GiB, 2 prefix normalizations,
3 `TEST+CMPSB`, 8 `CLD/STD`, 4 alias и 4 dual-unmapped cases. Независимо
зафиксировать полный GPR/RFLAGS/definedness/RIP/memory/retirement snapshot и
адрес первой ошибки. Проверить fixture generation/freshness и runner snapshots;
не использовать interpreter как oracle. Core regression matrices дополнительно
проверяют 55,296 success states, 512 flag-definedness states, 576 read failures,
192 boundaries, 72 aliases, 36 overlaps, 128 dual-invalid error combinations
(четыре пары ошибок × четыре ширины × два значения DF × две старшие части RSI ×
две старшие части RDI), and retries that reread changed source RAM after a second-read
failure. JIT/native admission остаётся закрытой.
Physical/Mac mini acceptance не выполняется здесь; ISSUE-016/020/021 и S2-18
остаются открытыми.

#### S2-18o — non-REP MOVS с address-size 32

Добавить только bounded Long64 non-REP `67 A4`, `67 66 A5`, `67 A5` и
`67 48 A5` MOVSB/W/D/Q, с принятыми нормализациями `48 67 A5` (dword),
`67 48 66 A5` (word) и `67 66 48 A5` (qword). Начальные адреса независимо
zero-extend ESI/EDI. Выполнить source read, затем ровно одну width-sized
destination write; диапазоны остаются непрерывными через 4 GiB, а overlap
использует исходные source bytes. Только после успешной записи обновить оба
индекса по DF modulo 2^32. Сохранить RAX, RCX, flags/definedness и unrelated
state; RCX=0 не подавляет non-REP MOVS.

Source fault запрещает destination access. Destination fault сохраняет source
read в trace, но не коммитит CPU/RAM/retirement согласно локальной
all-or-error модели; это не hardware restart, MMIO rollback или SMP atomicity
утверждение (ISSUE-016). Источник имеет локальный приоритет при dual-invalid
(ISSUE-021), без hardware exception-priority утверждения.

Покрытие: 73 literal schema-v2 fixtures; core matrices — 55,296 success,
512 definedness, 192 pointer boundaries, 576 failures, 72 aliases, 36 overlaps
и 32 dual-invalid. End-to-end `Unsupported` проверяет 73 непокрытых prefix
варианта, включая REP и сегментные/repeated-address формы. Также проверить
fetch truncation, budget 0, STOP/stop_rip, TEST/CLD/STD compositions, readonly
source/write-only destination, destination partial-fault no-prefix-mutation,
retry after source mutation, MOVS64/REP regressions. Native admission остаётся
addr64-only, JIT closed. Перегенерацию и runner snapshots проверить локально;
физические/Mac mini прогоны выполнять только в общем финальном substage.


#### S2-19s — bounded REPNE SCASW/D/Q с address-size 32

Добавить только Long64 `AF` с normalized group1 F2, address override 67 и
шириной W/D/Q: `F2 67 66 AF` — W, `F2 67 AF` и `67 F2 AF` — D,
`F2 67 48 AF` — Q. Сохранять legacy REX reset: `F2 67 48 66 AF` — W,
`F2 67 66 48 AF` — Q, `F2 48 67 AF` — D. Допускается каждый effective
REX.W `48..4F`. Addr32 F2 AE и существующий F3 профиль остаются неизменны;
F2 A7, addr64 F2, byte F2 для AF, LOCK, segment, duplicate prefix groups
и effective non-W REX остаются unsupported. Старые `run`,
`run_with_rflags_defined` и single-step entry points остаются fail-closed.

До ECX/data read требовать definedness всех arithmetic flags, не ограничивая
их concrete values. ECX=0 не читает и сохраняет полные RCX/RDI/RAX/flags/mask,
затем продвигает RIP и retires once. Иначе выполнить один contiguous width
read из zero-extended EDI; после success сравнить accumulator width minus data,
зафиксировать CMP flags, modulo-32 EDI ± width и zero-extended ECX-1.
Операнд не делится на два чтения на границе 4 GiB. REPNE завершается при
ECX=0 или ZF=1 до STOP, затем применяется лимит 1024 успешных сравнений.
Архитектурный fetch/data fault восстанавливает entry RFLAGS/mask, оставляет
счётчик/индекс после предыдущих успешных сравнений и очищает token; backend
ошибка оставляет checkpoint/token для retry.

Оpaque `RepContinuation` расширить kind, operand size, address size и
instruction length плюс существующие semantic checkpoint/context/original flags.
Проверять полное состояние до callback; после decode сверять kind/width/address/
length до data read. Equivalent prefix order той же длины допустим; другая
длина и cross-kind/width/address tokens отклоняются. Token остаётся caller-side
only и не сериализуется schema v2.

Покрытие этого среза включает exact prefix matrix, независимую W/D/Q CMP
модель по DF/начальному ZF/равенству/count, high halves, zero count, zero-extended
starting EDI, 4-GiB contiguous Qword/tail behavior, fault after progress, retry
checkpoint, STOP/limit resume and changed-length rejection. Literal fixtures
содержат serialization-only W/D/Q complete snapshots. ISSUE-020 остаётся
CMPS-only открытой политикой; этот SCAS профиль не расширяет её. Native/JIT,
physical x86, Mac mini и SSH проверки не выполнять.

#### S2-19t — bounded REPNE CMPSW/D/Q с address-size 32

Добавить только Long64 `A7` с normalized F2, address override 67 и widths
W/D/Q по той же exact prefix allowlist, что addr32 REPNE SCASW/D/Q:
`F2 67 66 A7` / `67 F2 66 A7` — W; `F2 67 A7` / `67 F2 A7` — D;
`F2 67 REX.W A7` / `67 F2 REX.W A7` — Q для REX `48..4F`. Сохранить
REX reset: `F2 REX 67 A7` — D, `F2 67 REX 66 A7` — W и
`F2 67 66 REX A7` — Q. Другие префиксы, repeat/segment/LOCK,
effective non-W REX, addr64 F2 и прочие CMPS формы остаются закрыты;
byte/F3 профили неизменны. Добавить `Operation::RepneCmpsWdqAddr32{size}`;
старые APIs остаются fail-closed, explicit continuation API — единственная
точка исполнения.

Continuation token сохраняет CMPS kind и width, address size, instruction
length, RSI и общее REP checkpoint state. До доступа проверять kind/width/
address/length; equivalent same-length prefix reorder допустим. Чтения за один
шаг: width bytes из zero-extended ESI, затем width bytes из EDI; arithmetic —
source−destination. Оба индекса обновляются modulo 32 на ±width по DF, без
wrap внутри операнда. До обоих успешных чтений не менять flags/index/count.
Архитектурный fault восстанавливает entry flags/definedness, оставляя прошлый
прогресс; backend error сохраняет checkpoint/token, retry повторяет оба
чтения. Source-first и dual-invalid остаются local ISSUE-020 policy, без
hardware-priority/MMIO rollback утверждения. Завершение ZF=1 или ECX=0
предшествует STOP и лимиту 1024 успешных сравнений.

Coverage includes exact prefix tests, independent W/D/Q CMP modeling, token
identity and reordered-prefix resume, architectural source/destination faults
after progress, changed-operand backend retry, source-first dual-invalid trace,
overlap, contiguous 4-GiB accesses and width-exact tail failures, three literal
schema-v2 fixtures with a serialization-only generator, testkit STOP/1024/
multiple-resume sidecar, and CLI/JIT denial. `cargo test --workspace
--all-targets --offline`, `cargo check --workspace --all-targets --offline`,
`cargo fmt --all -- --check`, all 131 CPU Python tests, generator freshness,
targeted runner snapshot/JIT denial, `git diff --check`, and Graphify update
passed. Independent `gpt-6.1-sol` medium code review found no actionable
defects in decoder admission, continuation identity, index/count wrap, operand
ordering, fault/retry handling, fixtures or documentation. ISSUE-020 remains open. See [CPU
contract](../specs/CPU_CONTRACT.md#local-s2-19t-bounded-repne-cmpswdq-address-size-32-contract), [interpreter notes](../testing/INTERPRETER.md#repne-cmpswdq-address-size-32--s2-19t), [fixture README](../../tests/fixtures/x86/stage2/README.md#bounded-repne-cmpswdq-address-size-32-fixtures-s2-19t), and [report](../reports/STAGE_2_REPORT.md#s2-19t--bounded-repne-cmpswdq-addr32).

#### S2-34.8a — local legacy MOVSS register-to-register

Добавлен exact local profile `s2-long-movss-reg-v1`. Допускается только
legacy `F3 0F 10 /r`, ModR/M.mod=3, при одном эффективном mandatory `F3` и
необязательном одном REX с R/B, расположенном после F3 и перед `0F`; индексы
XMM ограничены 0–15. `DEST[31:0]` получает raw `SRC[31:0]`, прежние
`DEST[127:32]` сохраняются. Это подтверждено Intel SDM revision 093, Vol. 2B,
MOVSS: register-source форма сохраняет биты 127:32, тогда как отдельная
memory-load форма очищает их. Использованы официальные [SDM revision 093
PDF](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf) и
[страница Intel SDM](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html).
F3 здесь обязательный opcode-prefix и не задаёт REP; регистровая форма не
читает/пишет память. VEX/EVEX, memory, F2/66, 0F 11, другие перестановки
префиксов, feature gates, #UD/#NM delivery и native/JIT остаются вне профиля.

Проверки покрывают 16×16 XMM, literal high-register fixture, alias/self,
сохранение остальных XMM/x87/MXCSR/GPR/RFLAGS при RCX и DF, отсутствие data
callbacks, prefix/memory rejection, truncation, 4/5-byte длины, STOP/budget и
отсутствующее SSE/x87 state. S2-34 и D/System остаются открытыми.

#### S2-34.8b — local legacy reverse-direction MOVSS register-to-register

Добавлен отдельный exact local fixture profile `s2-long-movss-store-reg-v1`:
только legacy `F3 0F 11 /r`, ModR/M.mod=3, с обязательным F3 и максимум одним
effective REX.R/B после F3 непосредственно перед `0F`; REX.W/X запрещены.
Для этой opcode direction `ModR/M.reg` — source, а `ModR/M.r/m` — destination.
Исполняется только raw copy `DEST[31:0] = SRC[31:0]`, остальные 96 бит
destination сохраняются. XMM индексы ограничены 0–15. Отдельный versioned
profile сохраняет точный контракт ранее существовавшего `0F 10` profile.

Основание: Intel SDM revision 093, Vol. 2B, MOVSS entry `F3 0F 11 /r`,
Op/En C (`xmm2/m32, xmm1`): для register operand ModR/M.r/m задаёт xmm2
destination, а ModR/M.reg — xmm1 source; legacy register result сохраняет
destination bits 127:32. Используются [официальный SDM revision 093 Volume 2B
PDF](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf) и
[страница Intel SDM](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html).
F3 для MOVSS — обязательный opcode prefix, не REP: эта форма не потребляет
RCX, не меняет направление по DF и не делает data-memory callbacks.

Тесты включают все 16×16 operand mappings, high XMM registers, alias/self,
асимметричный источник/назначение, полное сохранение x87/SSE/MXCSR/GPR/RFLAGS,
отсутствие memory callbacks, strict fixture admission (включая отказ `0F 10`
и memory forms), prefix/VEX/EVEX rejection, truncation, missing state,
STOP/budget и literal XMM8→XMM9 fixture. Native/JIT, feature gates,
`#UD`/`#NM` delivery и physical CPU execution не проверяются; S2-34 и
D/System остаются открытыми.

#### S2-34.8c — local legacy MOVSS m32 load from `[RAX]`

Добавлен отдельный точный profile `s2-long-movss-mem-load-v1` для legacy
`F3 0F 10 /r` с ModR/M.mod=00 и r/m=000 (`[RAX]`); разрешён optional REX.R
для выбора XMM8–XMM15, но REX.B/X/W и SIB/displacement отклоняются. При
успехе один 4-byte `GuestMemory::read` загружает low dword XMM и обнуляет
bits 127:32; прочее состояние сохраняется, RIP фиксируется после успешного
чтения. Ошибка backend сохраняет CPU state. Fault priority и hardware guest
exception delivery этим срезом не подтверждаются.

Intel SDM revision 093, Vol. 2B, Order Number 253667-093US (September 2026),
MOVSS table/operation: `F3 0F 10 /r`, Op/En A, source m32; legacy memory-load
pseudocode clears `DEST[127:32]` (pp. 4-117–4-119). See [official PDF]
(https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf). The profile assumes SSE and
CR0/CR4 prerequisites; it does not implement CPUID/CPU gates or #UD/#NM
delivery. Native/JIT and physical execution remain excluded. Tests cover the
exact profile, all sixteen XMM destinations, malformed code, 4-byte range
boundaries, injected read failure, no-commit/no-extra access, truncation,
STOP/budget and the literal runner fixture.

#### S2-34.8d — local legacy MOVSS m32 store to `[RAX]`

Added exact local profile `s2-long-movss-mem-store-v1` for legacy
`F3 0F 11 /r`, ModR/M.mod=00/rm=000 (`[RAX]`). One optional REX.R directly
after F3 selects source XMM8–XMM15 through ModR/M.reg. REX.B/X/W, SIB,
displacement, other bases, register destination, other prefixes, and VEX/EVEX
are rejected. The shared `MemoryOperand` validates a four-byte range; execution
performs exactly one `GuestMemory::write` of the source low dword and advances
RIP only after success. No XMM or unrelated state changes; backend write
failure leaves CPU state unchanged.

Intel SDM revision 093, Vol. 2B, Order Number 253667-093US (September 2026),
MOVSS `F3 0F 11 /r`, Op/En C, specifies `xmm2/m32, xmm1`: ModR/M.r/m is the
destination and ModR/M.reg is the source; the memory-store operation copies
`SRC[31:0]` to `DEST[31:0]` (pp. 4-117–4-119). Sources: [official Intel PDF]
(https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf) and [Intel SDM page]
(https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html).
Tests cover all source XMM indices, exact write/no-read callback counts,
canaries, range boundaries, injected write failure, truncated fetch, STOP and
budget, missing SSE state, malformed encodings, a handwritten literal fixture,
and native/JIT denial. This local diagnostic profile assumes SSE and CR0/CR4
prerequisites; fault priority, guest exception delivery, alignment guarantees,
feature gates, native/JIT execution, and physical verification are not claimed.

#### S2-34.9a — local legacy scalar-double MOVSD register-to-register

Добавлен отдельный exact profile `s2-long-movsd-reg-v1`: только legacy
`F2 0F 10 /r`, ModR/M.mod=3, обязательный F2 и максимум один REX.R/B после
F2 перед `0F`; REX.W/X запрещены, XMM индексы 0–15. ModR/M.reg выбирает
destination, ModR/M.r/m — source. Выполняется raw copy `DEST[63:0] =
SRC[63:0]`; прежние `DEST[127:64]` и всё прочее состояние сохраняются.
Используется отдельная exact schema, поскольку существующие MOVSS/MOVUPS
profiles имеют другую ширину и prefix contract.

Основание: Intel SDM revision 093, Vol. 2B, Order Number 253667-093US
(September 2026), MOVSD `F2 0F 10 /r`, Op/En A, назначает ModR/M.reg как
destination и ModR/M.r/m как source; legacy operation копирует low qword и
оставляет `DEST[MAXVL:64]` неизменным (pp. 4-108–4-110). Использованы
[официальный PDF](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf) и
[страница Intel SDM](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html).
F2 здесь обязательный opcode prefix, не REPNE; opcode A5 для string MOVSD
является отдельной primary-map инструкцией и отвергается этим профилем.

Покрытие включает 16×16 mappings, REX.R/B combinations, alias/self, zeros,
ones, alternating и NaN-like raw patterns, sentinels старших битов destination,
сохранение XMM/x87/MXCSR/GPR/RFLAGS при RCX=0 и DF=1, отсутствие data-memory
callbacks, missing state, truncation, STOP/budget, strict prefix/memory/
string/VEX/EVEX rejection и literal XMM8←XMM9 fixture. Native/JIT, CPUID,
`#UD`/`#NM`, physical x86 и широкая готовность SSE2 не заявляются. Плановый
срез предполагает явно заданное SSE2 state и architectural preconditions.

#### S2-34.9b — local legacy reverse-direction scalar-double MOVSD register

Добавлен отдельный exact profile `s2-long-movsd-store-reg-v1`: только legacy
`F2 0F 11 /r` с ModR/M.mod=3, optional one REX.R/B непосредственно после F2
перед `0F`; REX.W/X и остальные префиксы запрещены. ModR/M.reg — source,
ModR/M.r/m — destination, оба XMM0–XMM15. Копируются исходные биты
`SRC[63:0]` в `DEST[63:0]`; `DEST[127:64]` и прочее CPU state сохраняются.
F2 здесь mandatory opcode prefix, не REPNE; RCX/DF не участвуют, data-memory
callback отсутствует. Строковый `A5` остаётся отдельной инструкцией.

Intel SDM revision 093 Vol. 2B, entry MOVSD, `F2 0F 11 /r`, Op/En C,
описывает `MOVSD xmm2/m64, xmm1`: ModR/M.r/m — destination, ModR/M.reg —
source; для XMM destination обновляются только младшие 64 бита (pp. 4-108–
4-110). Source: [official Intel PDF](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf)
и [страница SDM](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html).
Литеральный XMM9←XMM8 fixture проверяет асимметричные старшие sentinel bits.
Полная matrix 16×16, prefix/neighbor rejection, missing state, truncation,
STOP/budget и отсутствие data callbacks покрыты локально; native/JIT и
физическое исполнение не заявляются. S2-34/D/System остаются открытыми.

#### S2-34.9c — local legacy scalar-double MOVSD m64 load from `[RAX]`

Добавлен отдельный exact profile `s2-long-movsd-mem-load-v1`. Он принимает
только legacy `F2 0F 10 /r` с ModR/M.mod=00, r/m=000 (`[RAX]`), обязательным F2
и максимум одним REX непосредственно после F2. У REX допускается только
REX.R; REX.W/X/B, SIB, displacement, другие базы, segment/address/operand
prefixes, LOCK, duplicate prefix/REX, reverse `0F 11`, VEX/EVEX и string MOVSD
`A5` отвергаются. ModR/M.reg с REX.R выбирает XMM0–XMM15. Этот профиль не
расширяет отдельные register-only контракты S2-34.9a/.9b.

Основание: Intel SDM revision 093, Vol. 2B, Order Number 253667-093US,
MOVSD `F2 0F 10 /r`, Op/En A: ModR/M.reg задаёт XMM destination, ModR/M.r/m —
m64 source. Legacy memory-load operation копирует `SRC[63:0]` в
`DEST[63:0]`, обнуляет `DEST[127:64]`, а биты `DEST[MAXVL:128]` оставляет без
изменения (pp. 4-108–4-110). См. [официальный Intel PDF]
(https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf) и
[страницу Intel SDM](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html).
F2 является mandatory opcode prefix, а не REPNE; primary-map string MOVSD
`A5` остаётся отдельной инструкцией.

Для `[RAX..RAX+7]` executor сначала валидирует весь canonical48 byte range,
затем выполняет ровно один `GuestMemory::read` в 8-byte временный буфер. После
успешного чтения он копирует low qword, очищает destination high qword и
обновляет RIP; x87, MXCSR, GPR, RFLAGS и прочие XMM не меняются. Диапазонные,
backend и injected fetch/read ошибки не фиксируют XMM/RIP; это local rollback,
а не утверждение об аппаратном fault-priority или guest #PF delivery.
Handwritten literal fixture содержит XMM8 load, data canaries и полные
before/after snapshots. Decoder/schema/executor проверки охватывают все 16
назначений, REX.R, partial-lane patterns, canonical границы, один 8-byte read,
отсутствие writes, rollback, STOP/budget, missing SSE, усечение/fetch failure,
strict negative encoding matrix и native/JIT denial. Control-register gates,
alignment/fault-order claims, CPUID, native/JIT execution и physical x86 не
входят в срез; предполагаются явно заданные SSE2 state и architectural
preconditions. S2-34/D/System остаётся открытым.

#### S2-34.9d — local legacy scalar-double MOVSD m64 store to `[RAX]`

Add separate profile `s2-long-movsd-mem-store-v1`. Admit only `F2 0F 11 /r`
with ModR/M.mod=00 and r/m=000 (`[RAX]`), mandatory F2 and at most one
immediately following REX with only REX.R permitted. ModR/M.reg plus REX.R
selects source XMM0–XMM15. Keep this distinct from reverse register `0F 11`
and string `A5`; reject other address modes, prefixes, REX bits, VEX and EVEX.

Intel SDM revision 093, Vol. 2B, MOVSD `F2 0F 11 /r`, Op/En C, defines
ModR/M.r/m as the memory destination and ModR/M.reg as the XMM source. Write
exactly source bits 63:0 through one shared `MemoryOperand` write. Do not alter
the source or any other CPU state. Validate the full canonical48 eight-byte
range before the callback and commit RIP only after the write succeeds. Add a
high-register literal fixture, canaries, all 16 source indices, boundaries,
fault/no-commit, fetch/STOP/budget and negative-form coverage, and explicit
native/JIT denial. This remains local interpreter evidence only; hardware
fault ordering/delivery, CPUID, native, and physical execution are excluded.

#### S2-34.10 — closure inventory and remaining acceptance

This inventory records bounded implementation already present and the open
work needed to close parent S2-34. It does not select the final CPU profile or
mark S2-34 complete.

| Group | State | Fault/alignment boundary | Evidence still needed | Dependency and next slice |
|---|---|---|---|---|
| Implemented-local: `MOVUPS` | Exact legacy `0F 10 /r` XMM←XMM and `0F 11 /r` XMM→XMM (`mod=3`); memory forms are `[RAX]` (`mod=00, r/m=000`), 64-bit base+disp (`mod=00/01/10` except RIP/SIB selectors), RIP-relative (`mod=00, r/m=101`) and SIB, via the separate exact fixture profiles. Bare or one profile-allowed REX; XMM0–15. | Register forms do not access data memory. Memory forms use one 16-byte callback and local no-commit on backend failure. No alignment-fault, guest-delivery, hardware fault-priority or atomicity claim; control/feature gates are assumed. | Local fixtures, decoder/executor matrices, fault injection and strict negative allowlists are evidence for these forms. Parent acceptance still needs real register and memory native-oracle cases; current physical harness admits integer profiles only, so SSE physical status is `not_run`. | No new local slice required. Keep native/JIT admission closed pending parent acceptance/profile decision. |
| Implemented-local: packed register logic | Legacy `0F 57` `XORPS`, `0F 54` `ANDPS`, `0F 56` `ORPS`, `0F 55` `ANDNPS`, all `/r, mod=3`; bare or one REX with R/B extension, no other legacy prefixes. Raw bitwise operations preserve unrelated state. | Register-only; no data fault or alignment access. SSE and CR0/CR4 preconditions assumed; no `#UD/#NM` delivery. | Local semantic/fixture evidence exists. Native-oracle confirmation and profile-wide feature/state consistency remain open. Physical SSE is `not_run` with the current integer-only harness. | No new local slice required; parent profile/acceptance work remains. |
| Implemented-local: `MOVSS` | Legacy `F3 0F 10 /r` and `F3 0F 11 /r`: both register directions (`mod=3`) plus m32 `[RAX]` load/store (`mod=00, r/m=000`); mandatory F3, at most one immediate REX as each profile allows, XMM0–15 (8a–8d). | Register forms preserve destination high 96 bits; memory load clears them. Memory forms issue one 4-byte callback and commit locally on success. Alignment, guest fault delivery/order and control gates are not established. | Local fixtures and callback/failure evidence exist; native-oracle memory/register confirmation remains part of parent completion. Physical SSE is `not_run`. | No new local slice required; retain exact profile allowlists. |
| Implemented-local: `MOVSD` | Legacy `F2 0F 10 /r` and `F2 0F 11 /r`: both register directions (`mod=3`) plus m64 `[RAX]` load/store (`mod=00, r/m=000`); mandatory F2, at most one immediate REX as each profile allows, XMM0–15 (9a–9d). Scalar-double SIMD, not string opcode `A5`. | Register forms preserve destination high 64 bits; memory load clears them. Memory forms issue one 8-byte callback and local no-commit on failure. Alignment, guest fault delivery/order and control gates are not established. | Local fixtures and callback/failure evidence exist; native-oracle memory/register confirmation remains part of parent completion. Physical SSE is `not_run`. | No new local slice required; retain exact profile allowlists. |
| Planned/unimplemented SSE moves and logic | Parent plan names moves and logical forms and calls out aligned/unaligned, scalar/packed and upper-lane behavior. Forms beyond the exact rows above are not implemented by this inventory; derive candidates from the selected profile rather than widening an existing fixture profile. | For each selected form, define source/destination state validation and commit point; determine architectural alignment and fault requirements from the applicable encoding. Local callback rollback is not guest exception delivery. | Per-form decode/negative matrix, lane and full-state preservation, boundary/alignment/fault tests; independent native oracle for required register and memory cases. Physical evidence only if a supported harness/profile can execute SSE. | Depends on explicit parent-profile scope. Next bounded slice: select one named missing form, document exact opcode/prefix/mode/operand allowlist and preconditions, then add local semantic and fixture evidence. |
| Excluded from current bounded profiles | VEX/EVEX, other mandatory/legacy prefixes and address forms, REX bits/forms outside each profile, feature/control-register fault delivery, FP arithmetic/MXCSR instruction behavior, and native/JIT execution remain excluded where stated by individual profiles. This does not decide global exclusions for the eventual CPU. | No behavior is promised for excluded encodings or unavailable-state conditions. Do not infer alignment or exception behavior from local results. | Negative tests establish only each versioned profile's closed allowlist. | Keep excluded until parent profile scope explicitly selects a requirement; do not broaden allowlists as part of this inventory. |
| Profile-undecided / system integration | Exact generic CPU profile, CPUID advertisement, SSE/SSE2 feature gates, MXCSR supported/reserved mask, and system/save-restore requirements remain undecided or owned by existing parent work. | Local slices assume relevant SSE/SSE2 and CR0/CR4 conditions; they do not implement `#UD`, `#NM`, `#GP`, or general state-transition delivery. No universal MXCSR mask is implied. | Decide a coherent profile and prove each advertised capability has matching state, instruction, gate and save/restore support; hardware observation remains separate from local evidence. | Preserve existing S2-35, S2-36, S2-38 and S2-39 ownership/dependencies. Next step is a profile decision in its owning parent task, not speculative local opcode expansion. |

**Start of a bounded diagnostic slice:** name one exact opcode/prefix/mode/
operand form, keep it in a versioned local profile, state SSE/SSE2 and CR0/CR4
assumptions, define affected state and local failure/no-commit behavior, and
provide local semantic plus literal fixture coverage. This does not require
settling generic CPUID/MSR/boot policy or make native, JIT or physical claims.

**Full parent S2-34 completion:** keep the parent open until the required
profile's instruction/state scope is explicit, every required form has
architectural state, gate, fault and alignment requirements accounted for,
and real register and memory cases pass the native oracle. Local interpreter
results are not native/JIT or physical evidence. The current physical harness
admits integer profiles only; therefore physical SSE evidence is `not_run`,
not a pass. Existing Mac mini/SSH batch validation remains at the end-of-stage
acceptance batch; this inventory does not move or execute that batch.

#### S2-35.1 — local-only bounded register `PXOR`

Prerequisite is only the local S2-34.1 raw x87/SSE state representation, not
completion of parent S2-34. Add exact fixture profile `s2-long-pxor-reg-v1` for `66 [REX.R/B] 0F EF /r`
with ModR/M.mod=3. Mandatory 66 distinguishes XMM/SSE2 from unprefixed MMX
PXOR; only one optional effective REX with R/B is admitted. Implement raw
128-bit XOR and deny memory, unsupported prefixes/REX bits, VEX/EVEX and
native/JIT. This child slice does not close parent S2-35, parent S2-34, or the
Stage 2 dependency `S2-35 → S2-34`; those remain open.

#### S2-35.2 — local-only bounded register `PADDB`

Requires S2-34.1 raw x87/SSE state and independent review of the S2-35.1
mandatory-`66` register decoder. Add exact profile
`s2-long-paddb-reg-v1` for `66 [one optional REX.R/B] 0F FC /r`, ModR/M.mod=3.
The `66` is mandatory because bare `0F FC` selects MMX; ModR/M.reg is the XMM
destination, and ModR/M.r/m is the source. Each of the sixteen byte lanes adds
independently modulo 256. Test all 16×16 register pairs, full-lane sentinels,
all 65,536 byte operand pairs on fixed registers, wrap/no-carry and
non-saturating vectors, and state preservation. Keep memory, PADDW/PADDD/PADDQ,
saturating operations, other prefixes, REX.W/X, VEX/EVEX, native/JIT and
physical execution excluded. Assume SSE2 available, CR0.EM=0, CR0.TS=0 and
CR4.OSFXSR=1 without CPUID claims. This local child task does not close S2-34,
S2-35 or the global dependency `S2-35 → S2-34`.


#### S2-35.3 — local-only bounded register `PSUBB`

Requires the raw state snapshot S2-34.1, S2-35.1, and independent review of
S2-35.2. The exact profile `s2-long-psubb-reg-v1` admits only `66 [one
optional REX.R/B] 0F F8 /r` with ModR/M.mod=3: ModR/M.reg is destination and
ModR/M.r/m is source. Each byte computes `(DEST - SRC) mod 256`, independently
with no lane borrow or saturation. The complete local check matrix tests all
16×16 XMM pairs and all 65,536 byte pairs on fixed registers, order, lane
isolation, wrap, self-subtraction and preserved unrelated state. Bare MMX,
memory, other packed widths and saturating variants, other prefixes, REX.W/X,
VEX/EVEX, native/JIT, and physical execution remain excluded. SSE2 availability
and CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1 are assumptions only; there are no CPUID
claims. This child task leaves S2-34, S2-35, and global S2-35 → S2-34 open.

#### S2-35.4 — local-only bounded register `PADDUSB`

Requires the raw state snapshot S2-34.1, mandatory-66 XMM decoding from
S2-35.1, and the byte-operation/state pattern of reviewed S2-35.2. The exact
profile `s2-long-paddusb-reg-v1` admits only `66 [one optional REX.R/B] 0F DC
/r` with ModR/M.mod=3: ModR/M.reg is destination and ModR/M.r/m is source.
Each unsigned byte lane computes `min(DEST + SRC, 255)` independently. Verify
all 16×16 XMM pairs, all 65,536 byte operand pairs on fixed registers,
unsigned-vs-signed boundaries, lane independence, self-add, state preservation,
no callbacks, and strict profile deny paths. Bare MMX, memory, PADDUSW/other
widths, signed saturation, other prefixes, REX.W/X, VEX/EVEX, native/JIT and
physical execution remain excluded. SSE2 availability, CR0.EM=0, CR0.TS=0 and
CR4.OSFXSR=1 are assumptions only; there are no CPUID claims. This child leaves
S2-34, S2-35 and global S2-35 → S2-34 open.


#### S2-35.5 — local-only bounded register `PSUBUSB`

Requires raw state snapshot S2-34.1, mandatory-66 XMM decoding from S2-35.1, and the reviewed byte-operation/state pattern from S2-35.3. Exact profile `s2-long-psubusb-reg-v1` admits `66 [one optional REX.R/B] 0F D8 /r` with ModR/M.mod=3: ModR/M.reg is destination, ModR/M.r/m is source. Each unsigned byte lane computes `max(DEST - SRC, 0)` independently. Check every 16×16 XMM pair and all 65,536 byte pairs on fixed registers, the order-sensitive vectors `00-01=00`, `02-01=01`, `01-02=00`, `FF-01=FE`, `80-7F=01`, `7F-80=00`, lane isolation, self-subtraction, state preservation, no memory callbacks, and strict profile rejection. Bare MMX, memory, other widths, signed saturation, other prefixes, REX.W/X, VEX/EVEX, native/JIT and physical execution remain excluded. SSE2 availability and CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1 are assumptions only; there are no CPUID claims. This child does not close parent S2-34, S2-35 or global S2-35 → S2-34.

#### S2-35.6 — local-only bounded register `PADDW`

Requires S2-34.1 raw x87/SSE state, mandatory-66 XMM decoding from S2-35.1,
and reviewed wrapping-add infrastructure from S2-35.2. Exact profile
`s2-long-paddw-reg-v1` admits only `66 [one optional REX.R/B] 0F FD /r` with
ModR/M.mod=3: ModR/M.reg is destination and ModR/M.r/m is source. Add eight
independent little-endian 16-bit lanes modulo 65536; carry stays within each
word and never crosses lanes. Check all 16×16 XMM pairs, lane sentinels,
little-endian raw-byte mapping, wraparound and signed-boundary values, self-add,
high registers and REX.R/B axes, selected boundary addends for every 16-bit lhs,
state preservation, no memory callbacks and strict profile rejection. Bare MMX,
PSUBW/PADDD/PADDQ, saturating variants, memory operands, other prefixes,
REX.W/X, VEX/EVEX, native/JIT and physical execution are excluded. SSE2
availability, CR0.EM=0, CR0.TS=0 and CR4.OSFXSR=1 are assumptions only; there
are no CPUID claims. This child leaves S2-34, S2-35 and global S2-35 → S2-34
open.

#### S2-35.7 — local-only bounded register `PSUBW`

Requires S2-34.1 raw x87/SSE state, mandatory-66 XMM decoding from S2-35.1,
reviewed little-endian word-lane representation from S2-35.6, and reviewed
wrapping subtraction/order patterns from S2-35.3. The exact profile
`s2-long-psubw-reg-v1` admits only `66 [one optional REX.R/B] 0F F9 /r` with
ModR/M.mod=3: ModR/M.reg is destination and ModR/M.r/m is source. Each of eight
little-endian u16 lanes computes `(DEST - SRC) mod 65536`, with borrow confined
to its word and no borrow crossing lanes. Check all 16×16 register pairs and
every u16 lhs against selected rhs boundaries, including direction, wrap,
no-saturation, lane sentinels, self-subtraction, full state preservation and no
data callbacks. Missing state, truncation, STOP/budget, strict decode/fixture
rejections, literal execution, and JIT/native denial are covered. Bare MMX,
PSUBD/Q, saturation, memory, extra prefixes, REX.W/X, VEX/EVEX and physical
execution remain excluded. SSE2 and CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1 are
assumptions only; no CPUID or exception-delivery claim is made. This child
does not close S2-34, S2-35 or global `S2-35 → S2-34`.


#### S2-35.8 — local-only bounded register `PADDUSW`

Requires S2-34.1 raw x87/SSE state, S2-35.1 mandatory-66 XMM decoding, the
reviewed little-endian word-lane representation from S2-35.6, and unsigned
saturation from S2-35.4. Exact profile `s2-long-paddusw-reg-v1` admits only
`66 [one optional REX.R/B] 0F DD /r` with ModR/M.mod=3: ModR/M.reg is the XMM
destination and ModR/M.r/m the source. Eight independent unsigned little-endian
u16 lanes compute `min(DEST + SRC, 65535)`; saturation is per lane. Check all
16×16 register pairs and each u16 lhs against selected rhs boundaries, including
65534/65535/65536 sums, signed-boundary and byte-order sentinels, lane
isolation, self-add thresholds, state preservation and zero data callbacks.
Missing state, truncation, STOP/budget behavior, strict schema/decoder rejections,
literal fixture execution, and JIT/native denial are covered. Bare MMX, memory,
other widths, signed saturation, extra prefixes, REX.W/X, VEX/EVEX and physical
execution remain excluded. SSE2 and CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1 are
assumptions only; no CPUID or exception-delivery claim is made. Intel SDM Rev.
093 Vol. 2B specifies `0F DD /r` as MMX and mandatory-66 `66 0F DD /r` as
`PADDUSW xmm1, xmm2/m128`, with ModR/M.reg destination, r/m source and
unsigned saturation, Intel SDM Rev. 093, Vol. 2B pp. 4-212–4-214
([official PDF](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf)).
This child leaves S2-34, S2-35 and global S2-35 → S2-34 open.


#### S2-35.9 — local-only bounded register `PSUBUSW`

Requires S2-34.1 raw x87/SSE state, S2-35.1 mandatory-66 XMM decoding,
reviewed little-endian word-lane subtraction/layout from S2-35.7, and the
unsigned saturation-to-zero pattern from S2-35.5. Exact profile
`s2-long-psubusw-reg-v1` admits only `66 [one optional REX.R/B] 0F D9 /r` with
ModR/M.mod=3: ModR/M.reg is destination and ModR/M.r/m source, extended only to
XMM0–XMM15. Each of eight little-endian u16 lanes computes
`max(unsigned(DEST) - unsigned(SRC), 0)` independently. Check all 16×16 register
pairs, every u16 lhs against selected rhs boundaries, underflow/equality/no-
underflow values, lane isolation and little-endian layout, full-state
preservation and no data callbacks. Missing state, truncation, STOP/budget,
strict schema/decoder rejection, literal execution and JIT/native denial are
covered. Bare MMX, memory, other widths, signed saturation, other prefixes,
REX.W/X, VEX/EVEX and physical execution remain excluded. SSE2 and
CR0.EM=0/CR0.TS=0/CR4.OSFXSR=1 are assumptions only; no CPUID or
exception-delivery claim is made. Intel SDM Rev. 093 Vol. 2B defines bare
`0F D9 /r` as MMX and `66 0F D9 /r` as `PSUBUSW xmm1, xmm2/m128`, with
ModR/M.reg destination, r/m source and unsigned saturation to zero, pp. 4-494–4-497
([official instruction reference](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf),
[Intel SDM index](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html)).
This child leaves S2-34, S2-35 and global `S2-35 → S2-34` open.


#### S2-35.10 — local-only bounded register `PADDSB`

Requires S2-34.1 raw x87/SSE state, S2-35.1 mandatory-66 XMM decoding,
reviewed S2-35.2 byte-lane state infrastructure and S2-35.4 saturation
coverage. Exact profile `s2-long-paddsb-reg-v1` admits only legacy
`66 [one optional REX.R/B] 0F EC /r` with ModR/M.mod=3. ModR/M.reg is the XMM
destination; ModR/M.r/m is the source, extended through XMM15. For each of
16 lanes, reinterpret the bytes as signed i8, add, clamp to [-128,127], and
write the result's raw byte. Verify all 65,536 byte-pair inputs, all 16×16
register pairs, lane sentinels and saturation/unsigned-distinguishing cases,
full-state preservation, unchanged flags/MXCSR and no data callbacks. Cover
missing state, truncation, STOP/budget, strict-profile negatives, a literal
fixture, and JIT/native denial. Bare MMX, PADDSW, subtraction, memory, extra
prefixes, REX.W/X, VEX/EVEX and physical execution remain excluded. SSE2 and
CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1 are assumptions only; no CPUID or
exception-delivery claim is made. Intel SDM Rev. 093 Vol. 2B specifies bare
`0F EC /r` as MMX PADDSB and mandatory-66 `66 0F EC /r` as
`PADDSB xmm1, xmm2/m128`, signed-byte addition with signed saturation
([official Intel PDF](https://cdrdv2-public.intel.com/929354/253667-093-sdm-vol-2b.pdf)).
This child leaves S2-34, S2-35 and global `S2-35 → S2-34` open.

#### S2-35.11 — local-only bounded register `PSUBSB`

Requires S2-34.1 raw x87/SSE state, mandatory-66 XMM decoding from S2-35.1,
reviewed byte-subtraction/order infrastructure from S2-35.3 and signed
saturation coverage from S2-35.10. The exact profile
`s2-long-psubsb-reg-v1` admits only legacy `66 [one optional REX.R/B] 0F E8
/r` with ModR/M.mod=3. ModR/M.reg is the XMM destination and ModR/M.r/m is the
source, extended through XMM15. Each of 16 lanes reinterprets both operands as
signed i8, computes DEST minus SRC, clamps to [-128, 127], and stores the raw
result byte. Check all 16×16 register pairs, all 65,536 byte pairs on fixed
registers, lane isolation, saturation boundaries and asymmetry including
`00-80=7f`, complete architectural-state preservation except destination XMM
and RIP, unchanged flags/MXCSR, and zero data callbacks. Missing x87/SSE state,
truncation, STOP/budget, strict schema and decoder rejection, literal fixture
execution, and JIT/native denial are covered. Bare MMX, memory operands, other
packed widths, addition and unsigned saturation, other prefixes, REX.W/X,
VEX/EVEX, native/JIT execution and physical execution are excluded. SSE2 and
CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1 are assumptions only; no CPUID or exception
delivery claim is made. This child leaves parent S2-34, parent S2-35, and
global `S2-35 → S2-34` open.

#### S2-35.12 — local-only bounded register `PADDSW`

Requires S2-34.1 raw x87/SSE state, mandatory-66 XMM decoding from S2-35.1,
and reviewed little-endian word-lane representation from S2-35.6. The exact
profile `s2-long-paddsw-reg-v1` admits only legacy
`66 [one optional REX.R/B] 0F ED /r` with ModR/M.mod=3. ModR/M.reg is the XMM
destination and ModR/M.r/m is the source, extended through XMM15. Each of eight
little-endian signed i16 lanes is added in widened i32 and clamped to
[-32768, 32767]. Check all 16×16 register pairs; all 65,536 signed lhs values
against rhs boundaries `0000, 0001, ffff, 7fff, 8000, 7ffe, 8001`; saturation
boundaries, signedness and lane isolation, full state preservation, unchanged
flags/MXCSR and no data callbacks. Missing state, truncation, STOP/budget,
strict schema/decoder rejection, literal fixture execution, and JIT/native
denial are covered. Bare MMX, memory operands, other packed widths, subtraction,
extra prefixes, REX.W/X, VEX/EVEX and physical execution remain excluded. SSE2
and CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1 are assumptions only; no CPUID or exception
delivery claim is made. This child leaves S2-34, S2-35, and global
`S2-35 → S2-34` open. Mac mini/SSH tests remain deferred to the final combined
substage.

#### S2-35.13 — local-only bounded register `PSUBSW`

Requires S2-34.1 raw x87/SSE state, mandatory-66 XMM decoding from S2-35.1,
and reviewed little-endian word-lane representation from S2-35.6. Profile
`s2-long-psubsw-reg-v1` admits only legacy `66 [one optional REX.R/B] 0F E9 /r`
with ModR/M.mod=3. ModR/M.reg is the XMM destination and ModR/M.r/m the source,
extended through XMM15. For each of eight little-endian signed i16 lanes,
subtract source from destination in widened i32 and clamp to [-32768, 32767].
Coverage includes all 16×16 register pairs and all 65,536 signed lhs values
against rhs boundaries `0000, 0001, ffff, 7fff, 8000, 7ffe, 8001`, plus
saturation/asymmetry, lane isolation, full state/flags/MXCSR preservation, and
no data callbacks. Missing state, truncation, STOP/budget, strict schema and
decoder rejection, a literal fixture, and JIT/native denial are covered. Bare
MMX, memory, other widths, extra prefixes, REX.W/X, VEX/EVEX, and physical
execution remain outside scope. SSE2 and CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1 are
assumptions only; there is no CPUID or exception-delivery claim. S2-34, S2-35,
and global `S2-35 → S2-34` remain open. Mac mini/SSH acceptance remains deferred
to the final combined substage.

#### S2-35.14 — local-only bounded register `PADDD`

Requires S2-34.1 raw x87/SSE state, mandatory-66 XMM decoding from S2-35.1,
and reviewed little-endian word-lane representation from S2-35.6. Exact profile
`s2-long-paddd-reg-v1` admits only legacy `66 [one optional REX.R/B] 0F FE
/r` with ModR/M.mod=3. ModR/M.reg is the XMM destination and ModR/M.r/m is the
source, extended to XMM0–15. Four little-endian u32 lanes add modulo 2^32
independently; carry is confined to its dword. Check all 16×16 register pairs,
lane sentinels, no-cross-lane carry, wrap and carry-chain values, walking bits,
and reproducible raw-u32 samples against an independent u64 masked reference.
Preserve all state except destination XMM and RIP and invoke no data callback.
Missing state, truncation, STOP/budget, strict schema and decoder rejection,
literal high-XMM fixture execution, and JIT/native denial are covered. Bare
MMX, PSUBD/PADDQ, saturation, memory, extra prefixes, REX.W/X, VEX/EVEX and
physical execution remain excluded. SSE2 and CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1
are assumptions only; there is no CPUID or exception-delivery claim. S2-34,
S2-35, and global `S2-35 → S2-34` remain open. Mac mini/SSH acceptance remains
deferred to the final combined substage.

S2-35.14 implementation check (2026-10-27): focused PADDD tests passed for
`od-isa` (3), `od-testkit` schema validation (1), and the literal `od-runner`
fixture (1). `cargo test --workspace --offline`, `cargo check --workspace
--all-targets --offline`, `cargo check --workspace --all-targets --target
x86_64-unknown-linux-gnu --offline`, and `cargo fmt --all -- --check` passed.
The Linux-target check compiled the Linux-gated native-denial test; execution of
that gate remains unavailable on the macOS host. The local fixture runner
confirmed interpreter execution and JIT denial. These results are local
interpreter/profile evidence only and do not close S2-34, S2-35, global
S2-35 → S2-34, or the deferred Mac mini/SSH acceptance batch.

#### S2-35.15 — local-only bounded register `PSUBD`

Requires S2-34.1 raw x87/SSE state, mandatory-66 XMM decoding from S2-35.1,
and the reviewed dword-lane representation from S2-35.14. Exact profile
`s2-long-psubd-reg-v1` admits only legacy `66 [one optional REX.R/B] 0F FA
/r` with ModR/M.mod=3. ModR/M.reg is destination and r/m source, extended to
XMM0–15. Four little-endian u32 lanes compute `(DEST-SRC) mod 2^32`
independently. Coverage includes the full 16×16 XMM/REX matrix, reproducible
raw-u32 vectors checked against an independent u64 modulo reference,
walking-one/zero patterns, and borrow chains including `00010000-1=0000ffff`.
The full state except destination XMM and RIP is preserved, and no guest data
memory callback occurs. Missing state, truncation, STOP/budget, strict schema
and decoder rejection, literal high-XMM execution, and JIT/native denial are
covered. Bare MMX, memory, PADDQ/PSUBQ, other widths, saturation, extra
prefixes, REX.W/X, VEX/EVEX and physical execution remain excluded. SSE2 and
CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1 are assumptions only; no CPUID or exception
delivery claim is made. S2-34, S2-35 and global `S2-35 → S2-34` remain open;
Mac mini/SSH acceptance remains deferred to the final combined substage.

S2-35.15 implementation check (2026-10-27): focused PSUBD tests passed for
`od-isa` (3), `od-testkit` schema (1), and `od-runner` literal fixture (1).
`cargo test --workspace --offline` passed; workspace all-target offline check,
Linux x86_64 `od-x86-runner` all-target compile check, formatting, and
`git diff --check` passed. The Linux-gated native-denial test compiled but was
not run on Linux. `graphify update .` completed. These results establish local
interpreter/profile behavior only; they do not close S2-34, S2-35, global
S2-35 → S2-34, or deferred Mac mini/SSH acceptance.

The S2-35.16 independent review found that the PSUBD profile reached native
trace setup before it was rejected. The early local-only guard now rejects
PSUBD before tracee setup; its Linux-gated assertion was compiled by the
S2-35.16 x86_64 Linux all-target check.

#### S2-35.16 — local-only bounded register `PADDQ`

Requires S2-34.1 raw x87/SSE state, mandatory-66 XMM decoding from S2-35.1,
and the reviewed qword-independent layout pattern established by S2-35.14.
Exact profile `s2-long-paddq-reg-v1` admits only legacy
`66 [one optional REX.R/B] 0F D4 /r` with ModR/M.mod=3. ModR/M.reg is the XMM
destination and ModR/M.r/m is the source, extended to XMM0–15. Each of two
little-endian u64 lanes computes `(DEST + SRC) mod 2^64` independently; carry
cannot cross between lanes. Cover all 16×16 register pairs, deterministic raw
u64 vectors against an independent u128 masked reference, walking-one/zero
patterns, carry chains through bit 63 and between-lane isolation, complete
state preservation except destination XMM and RIP, and no guest data-memory
callbacks. Missing x87/SSE state, truncation, STOP/budget atomicity, strict
schema/decoder rejection, a handwritten literal XMM8/XMM9 fixture, and
JIT/native denial are covered. Bare MMX, PSUBQ/other widths, saturation,
memory operands, extra prefixes, REX.W/X, VEX/EVEX and physical execution
remain excluded. SSE2 and CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1 are assumptions only;
there is no CPUID or exception-delivery claim. S2-34, S2-35 and global
`S2-35 → S2-34` remain open. Mac mini/SSH acceptance remains deferred to the
final combined substage.

S2-35.16 implementation check (2026-10-27): focused PADDQ interpreter and
strict-decoder tests passed (3); the handwritten XMM8/XMM9 fixture passed its
CLI interpreter/JIT-denial check (1). The sibling PADDQ/PSUBD/PADDD fixture
runner checks passed (11). `cargo test --workspace --offline`, workspace
all-target offline check, Linux x86_64 workspace all-target compile check,
`cargo fmt --all -- --check`, `git diff --check`, and `graphify update .`
passed. PADDQ and PSUBD Linux-only early native-denial assertions compiled in
the cross-target check but were not executed on this macOS host. The evidence
is local interpreter/profile coverage only; it does not close S2-34, S2-35,
global `S2-35 → S2-34`, physical x86 validation, or deferred Mac mini/SSH
acceptance.

#### S2-35.17 — local-only bounded register `PSUBQ`

Requires S2-34.1 raw x87/SSE state, mandatory-66 XMM decoding from S2-35.1,
and the reviewed qword lane representation established by S2-35.16. Exact
profile `s2-long-psubq-reg-v1` admits only legacy `66 [one optional REX.R/B]
0F FB /r` with ModR/M.mod=3. ModR/M.reg is destination and ModR/M.r/m source,
extended to XMM0–15. Each of two little-endian u64 lanes computes
`(DEST-SRC) mod 2^64` independently. The direction follows Intel SDM Rev. 093,
Vol. 2B's PSUBQ description and RM operand encoding; opcode table wording that
reverses the operands is not used. Coverage includes all 16×16 register pairs,
reproducible raw-u64 samples checked against an independent u128 masked
subtraction reference, walking-one/zero patterns, borrow chains, and explicit
`0-1`, `8000000000000000-1`, and `0000000100000000-1` cases. State other than
destination XMM and RIP is preserved, and no guest data-memory callback occurs.
Missing state, truncation, STOP/budget atomicity, strict schema/decoder
rejection, a literal XMM8/XMM9 fixture, and JIT/native denial are covered. Bare
MMX, memory operands, saturation, other widths/operations, extra prefixes,
REX.W/X, VEX/EVEX, native/physical execution, SSE feature gating, and exception
delivery remain excluded. SSE2 and CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1 are
assumptions only. S2-34, S2-35, and global `S2-35 → S2-34` remain open; Mac
mini/SSH acceptance remains deferred to the final combined substage.

S2-35.17 implementation check (2026-10-27): focused `cargo test -p od-isa psubq --offline` passed (3); the literal `od-runner` fixture test passed (1),
including strict schema-neighbor rejection and explicit JIT denial. All
workspace tests passed (1,695 total: `od-host` 6, `od-isa` 982, `od-jit` 5,
`od-runner` 374, `od-testkit` 319, `od-x86-runner` 9; `od-machine`,
`od-storage`, and doc tests had 0).
Workspace all-target offline check and x86_64 Linux all-target cross-check
passed. The Linux-only native-denial assertion compiled in that cross-check;
it could not be executed on this macOS host. Formatting, diff whitespace, and
Graphify update passed. Independent gpt-6.1-sol medium review passed with no findings on 2026-10-27;
these results establish local interpreter/profile coverage only and do not close S2-34, S2-35, global
`S2-35 → S2-34`, physical x86 validation, or deferred Mac mini/SSH acceptance.
#### S2-35.18 — local-only bounded register `PCMPEQB`

Requires S2-34.1 raw x87/SSE state and mandatory-66 XMM decoding from S2-35.1.
Exact profile `s2-long-pcmpeqb-reg-v1` admits legacy
`66 [one optional REX.R/B] 0F 74 /r` with ModR/M.mod=3 only. ModR/M.reg is the
destination and ModR/M.r/m is the source, extended through XMM15. Each byte
lane independently becomes `FF` for exact equality and `00` for inequality.
Coverage includes all 65,536 byte pairs, all 16×16 XMM register pairs and REX
axes, mixed lanes, one mismatch at every lane position, all-FF self-compare,
signed-byte boundaries (`7F`, `80`, `FF`) and lane permutations. State outside destination
XMM/RIP is preserved and no guest data-memory callback occurs. Missing state,
truncation, STOP/budget atomicity, strict schema/decode negatives, a handwritten
literal high-XMM fixture, and JIT/native denial are covered. PCMPEQW/D/Q,
PCMPGT*, and PCMPEQQ (a separate SSE4.1 instruction) are excluded, as are MMX,
memory, extra prefixes, REX.W/X, VEX/EVEX, feature gates,
exceptions and physical execution remain excluded. SSE2 and CR0/CR4 requirements
are assumptions only. S2-34, S2-35 and global `S2-35 → S2-34` remain open; Mac
mini/SSH acceptance remains deferred.

S2-35.18 implementation check (2026-10-27): focused PCMPEQB tests passed (4);
the handwritten fixture test passed (1), and all 319 `od-testkit` tests passed.
The full workspace suite passed 1,700 tests. Workspace and x86_64 Linux
all-target offline checks passed. The Linux-gated native-denial assertion was
compile-checked only on macOS. Formatting and diff whitespace checks passed;
layout validation reports the pre-existing ISSUE-023 checkout/link findings.
Graphify update completed. Independent gpt-6.1-sol medium review passed with
no remaining findings after correcting the test total. This remains local
interpreter/profile evidence.


#### S2-35.19 — local-only bounded register `PCMPEQW`

Requires S2-34.1 raw x87/SSE state and mandatory-66 XMM decoding from S2-35.1.
Exact profile `s2-long-pcmpeqw-reg-v1` admits only legacy
`66 [one optional REX.R/B] 0F 75 /r` with ModR/M.mod=3. ModR/M.reg is the
destination and ModR/M.r/m the source, extended to XMM0–15. Each of eight
little-endian u16 lanes independently becomes `FFFF` for exact equality and
`0000` otherwise. Coverage requires all 65,536 lhs values against self, each of
16 single-bit toggles, and boundary rhs values; all 16×16 XMM/REX pairs; mixed
lanes, one mismatch per word, byte-swapped sentinels, all-FF self compare, full
state preservation and zero data callbacks. Missing state, truncation, STOP/
budget atomicity, strict schema/decode negatives, handwritten literal high-XMM
fixture execution, and JIT/native denial are covered. PCMPEQD/Q, PCMPGT*,
PCMPEQQ (separate SSE4.1), MMX, memory, extra prefixes, REX.W/X, VEX/EVEX,
gates, exception delivery and physical execution remain excluded. SSE2 and
CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1 are assumptions only. S2-34, S2-35 and global
`S2-35 → S2-34` remain open; Mac mini/SSH acceptance remains deferred.

S2-35.19 implementation check (2026-10-27): focused PCMPEQW interpreter and strict decoder tests passed (4); the handwritten XMM8/XMM9 fixture CLI test passed (1), including strict schema-neighbor rejection and JIT denial. `cargo test --workspace --offline` passed 1,705 tests. Workspace all-target offline check, x86_64 Linux all-target compile check, formatting, and diff whitespace checks passed. The Linux-gated native-denial assertion compiled in the cross-target check but was not executed on this macOS host. `python3 tools/check_layout.py` reports the pre-existing ISSUE-023 checkout pointer and stale oracle links. `graphify update .` completed. Independent `gpt-6.1-sol` medium review found a schema-negative opcode typo; it was corrected to PCMPEQW's `C5 F9 75 C1`, the neighboring PCMPEQB case was preserved as `C5 F9 74 C1`, and final review/re-review passed with no findings. The results are local interpreter/profile evidence only; S2-34, S2-35, global `S2-35 → S2-34`, physical x86 validation, and deferred Mac mini/SSH acceptance remain open.

S2-35.20 implementation check (2026-10-27): three focused `od-isa` PCMPEQD
tests passed (decode/negative matrix, dword boundary/bit/register semantics,
and atomicity); the handwritten XMM8/XMM9 CLI fixture passed (1), including
mixed equal/unequal lane masks, strict VEX rejection, and JIT denial. The
neighboring PCMPEQB fixture test passed after independent review caught and
corrected an accidental expected-output edit. `cargo test --workspace --offline`
passed 1,709 tests. Workspace all-target offline check and x86_64 Linux
all-target compile check passed; the Linux-only native-denial test is compile-
checked but not run on this macOS host. Formatting and diff whitespace checks
passed. Layout validation failed on the pre-existing ISSUE-023 checkout
pointer and two missing historical oracle manifests. Graphify update
completed. Review by `gpt-6.1-sol` medium
found no remaining findings. These are local interpreter/profile results;
S2-34, S2-35, global `S2-35 → S2-34`, physical x86 validation, and deferred
Mac mini/SSH acceptance remain open.

#### S2-35.20 — local-only bounded register `PCMPEQD`

Requires S2-34.1 raw x87/SSE state and mandatory-66 XMM decoding from S2-35.1.
Exact profile `s2-long-pcmpeqd-reg-v1` admits only legacy
`66 [one optional REX.R/B] 0F 76 /r` with ModR/M.mod=3. ModR/M.reg is the
destination and ModR/M.r/m the source, extended to XMM0–15. Each of four
little-endian u32 lanes independently becomes `FFFFFFFF` for exact whole-dword
equality and `00000000` otherwise. Coverage includes 0, 1, 7fffffff, 80000000,
ffffffff boundaries; all 32 single-bit mismatches; walking patterns; deterministic
mixed raw dwords; one mismatch at each lane; distinctions from word/byte equality;
self compare, full state, all 16×16 register/REX combinations, no data callbacks,
atomic failure paths, strict negative encodings and a literal XMM8/XMM9 fixture.
PCMPEQQ, PCMPGT*, MMX, memory, extra prefixes, REX.W/X, VEX/EVEX, gates,
exceptions and native/JIT/physical execution remain excluded. SSE2 and
CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1 are assumptions only. S2-34, S2-35 and global
`S2-35 → S2-34` remain open; Mac mini/SSH acceptance remains deferred.

#### S2-35.21 — local-only bounded register `PCMPGTB`

Requires S2-34.1 raw x87/SSE state and mandatory-66 XMM decoding from S2-35.1.
Exact profile `s2-long-pcmpgtb-reg-v1` admits only legacy
`66 [one optional REX.R/B] 0F 64 /r` with ModR/M.mod=3. ModR/M.reg is the
destination and ModR/M.r/m the source, extended to XMM0–15. Each byte lane
becomes `FF` iff signed_i8(destination) is strictly greater than
signed_i8(source), otherwise `00`. Coverage includes all 65,536 raw byte pairs
against an independent signed mapping, strict/equal and asymmetric cases,
`7f>80`, `80>7f`, `00>ff`, and `ff>00`, mixed lane values, the full 16×16
XMM/REX matrix, complete state/flags/MXCSR preservation, and zero data-memory
callbacks. Missing state, truncation, STOP/budget atomicity, exact schema and
decoder negatives, the literal XMM8/XMM9 fixture, and JIT/native denial are
covered. Bare MMX, PCMPGTW/D/Q, other comparisons, memory, extra prefixes,
REX.W/X, VEX/EVEX, feature gates, exception delivery and physical execution
are excluded. PCMPGTQ requires SSE4.2 and remains outside this SSE2 slice. SSE2
and CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1 are assumptions only; there is no CPUID
or exception-delivery claim. S2-34, S2-35 and global `S2-35 → S2-34` remain
open; Mac mini/SSH acceptance remains deferred.

S2-35.21 implementation check (2026-10-27): the focused `od-isa` decoder,
exhaustive byte-pair/register matrix, and atomicity tests passed (3); the
handwritten literal XMM8/XMM9 runner fixture passed (1), including strict VEX
rejection and JIT denial. `cargo test --workspace --offline` passed 1,713 tests.
Workspace all-target offline check and x86_64 Linux all-target compile check
passed; the Linux-gated native-denial test was compiled but not executed on the
macOS host. `cargo fmt --all -- --check` and `git diff --check` passed.
`graphify update .` completed; it reports the existing header extractor warning
for `include/open_desktop/host_abi.h`. The cross-target check reports existing
dead-code warnings in `od-host` and `od-runner`. Independent `gpt-6.1-sol`
medium review passed with no findings. These are local interpreter/profile results only and do not close S2-34, S2-35, global `S2-35 → S2-34`,
physical x86 validation, or deferred Mac mini/SSH acceptance.

#### S2-35.22 — local-only bounded register `PCMPGTW`

Requires S2-34.1 raw x87/SSE state and mandatory-66 XMM decoding from S2-35.1.
Exact profile `s2-long-pcmpgtw-reg-v1` admits only legacy
`66 [one optional REX.R/B] 0F 65 /r` with ModR/M.mod=3. ModR/M.reg is the
destination and ModR/M.r/m the source, extended to XMM0–15. Each of eight
little-endian word lanes becomes `FFFF` iff signed_i16(destination) is
strictly greater than signed_i16(source), otherwise `0000`. Coverage includes
all 65,536 lhs words against signed boundary rhs values and their in-range
neighbors, plus each lhs against its signed predecessor, itself, and successor
using an independent i32 oracle. Explicit cases test operand direction,
strictness/equality and word-vs-byte distinctions (`0100>00ff`, `0080>007f`),
mixed lanes, the complete 16×16 XMM/REX matrix, full-state/flags/MXCSR
preservation, and zero data callbacks. Missing state, truncation, STOP/budget
atomicity, strict schema/decoder negatives including VEX `C5 F9 65 C1`, and a
literal XMM8/XMM9 fixture with JIT/native denial are covered. PCMPGTD/Q, other
comparisons, MMX, memory, extra prefixes, REX.W/X, VEX/EVEX, gates, exceptions,
and native/JIT/physical execution remain out of scope. SSE2 and CR0/CR4
preconditions are assumptions only. S2-34, S2-35 and global `S2-35 → S2-34`
remain open; Mac mini/SSH acceptance remains deferred.

S2-35.22 implementation check (2026-10-27): focused `od-isa` PCMPGTW tests
passed (3), including decoder boundaries, per-lhs signed-neighbor oracle,
mixed lanes, register matrix, preservation and atomic failures. The literal
high-XMM runner fixture/schema/JIT-denial test passed (1). Full
`cargo test --workspace --offline --quiet` passed 1,717 tests. Workspace
all-target offline check and `x86_64-unknown-linux-gnu` all-target compile
check passed; the Linux-gated native-denial test was compile-checked but not
executed on the Darwin host. `cargo fmt --all -- --check` and `git diff --check`
passed. `graphify update .` completed and reported the existing
`include/open_desktop/host_abi.h` extractor warning. Independent
`gpt-6.1-sol` medium review and re-review passed with no remaining findings.
These are local interpreter/profile results only; they do not close S2-34,
S2-35, global `S2-35 → S2-34`, physical x86 validation, or deferred combined
Mac mini/SSH acceptance.

#### S2-35.23 — local-only bounded register `PCMPGTD`

Requires S2-34.1 raw x87/SSE state and mandatory-66 XMM decoding from S2-35.1.
Exact profile `s2-long-pcmpgtd-reg-v1` admits only legacy
`66 [one optional REX.R/B] 0F 66 /r` with ModR/M.mod=3. The opcode byte is
the second `66`, after `0F`; the first is the mandatory prefix. ModR/M.reg is
the destination and ModR/M.r/m the source, extended to XMM0–15. Each of four
little-endian dword lanes becomes `FFFFFFFF` iff signed_i32(destination) is
strictly greater than signed_i32(source), otherwise `00000000`.

Coverage includes a raw-u32 domain built from every walking bit and complement,
signed boundary values and in-range neighbors, compared through an independent
i64 signed oracle around i32 boundaries. Explicit cases test direction,
strictness/equality, dword-vs-word/byte sentinels (`00010000>0000ffff`,
`00008000>00007fff`), signed extrema, mixed masks, the full 16×16 XMM/REX
matrix, complete state/flags/MXCSR preservation, and zero data callbacks.
Missing state, truncation, STOP/budget atomicity, strict schema/decoder
negatives including VEX `C5 F9 66 C1`, and a literal XMM8/XMM9 fixture with
JIT/native denial are covered. PCMPGTQ/other compares, MMX, memory, other
prefixes, REX.W/X, VEX/EVEX execution, feature gates, exception delivery, and
native/JIT/physical execution remain excluded. SSE2 and CR0/CR4 preconditions
are assumptions only. S2-34, S2-35 and global `S2-35 → S2-34` remain open;
Mac mini/SSH acceptance remains deferred.

S2-35.23 implementation check (2026-10-27): focused `od-isa` PCMPGTD
coverage passed (3 tests), including decoder boundaries, signed-i32 boundary
and walking-bit oracles, mixed lanes, all 16×16 XMM pairs, preservation and
atomic failures. The handwritten XMM8/XMM9 runner fixture/schema/JIT-denial
test passed (1). `cargo test --workspace --offline --quiet` passed 1,721
workspace tests. Workspace all-target offline check and the
`x86_64-unknown-linux-gnu` all-target compile check passed; the Linux-gated
native-denial test was compile-checked but not executed on the Darwin host.
`cargo fmt --all -- --check` and `git diff --check` passed. Layout validation
still fails on the linked worktree `.git` pointer and two missing historical
oracle manifests. `graphify update .` completed and reported the existing
`include/open_desktop/host_abi.h` extractor warning. Independent `gpt-6.1-sol`
medium review and re-review passed with no remaining findings. These are local
interpreter/profile results only and
do not close S2-34, S2-35, global `S2-35 → S2-34`, physical x86 validation, or
deferred combined Mac mini/SSH acceptance.

#### S2-35.24 — local-only bounded register `PUNPCKLBW`

Requires S2-34.1 raw x87/SSE state and mandatory-66 XMM decoding from S2-35.1.
Exact profile `s2-long-punpcklbw-reg-v1` admits only legacy
`66 [one optional REX.R/B] 0F 60 /r` with ModR/M.mod=3. ModR/M.reg is the
destination and ModR/M.r/m the source, extended to XMM0–15. For each of eight
low byte lanes, the result is `DEST[2*i] = old_DEST[i]` and
`DEST[2*i+1] = SRC[i]`; byte 0 is least significant. All 16 destination bytes
are written. Source and destination bytes 8–15 are ignored.

Coverage includes the handwritten `00..07` / `80..87` interleave sentinel,
self-alias `d0..d7`, every byte value at every lane position, walking bits,
all 16×16 XMM pairs/REX extensions, upper-half independence, complete state
preservation and zero data callbacks. Missing state, all truncated prefixes,
STOP/budget atomicity, strict schema/decoder negatives including VEX
`C5 F9 60 C1`, and a literal XMM8/XMM9 fixture with JIT/native denial are
covered. High-half unpack, other widths, packing, shuffles, shifts, memory,
MMX, extra prefixes, REX.W/X, VEX/EVEX, feature gates, exception delivery and
physical execution remain excluded. SSE2 and CR0.EM=0, CR0.TS=0,
CR4.OSFXSR=1 are assumptions only. S2-34, S2-35 and global `S2-35 → S2-34`
remain open; Mac mini/SSH acceptance remains deferred.

S2-35.24 implementation check (2026-10-27): focused `od-isa` PUNPCKLBW
decoder/semantics/atomicity tests passed (3), including every byte value at
each low lane, walking bits, all 16×16 XMM/REX combinations, self-alias,
upper-half independence, full architectural state preservation, and zero data
callbacks. The handwritten literal XMM8/XMM9 fixture/schema/JIT-denial test
passed (1), including strict VEX rejection; all 319 `od-testkit` tests passed.
`cargo test --workspace --offline --quiet` passed 1,725 tests. Workspace
all-target offline check and `x86_64-unknown-linux-gnu` all-target compile
check passed. The Linux-only native-denial test was compile-checked on this
Darwin host, not executed. Formatting and diff whitespace checks passed.
Graphify update completed with the existing header extractor warning for
`include/open_desktop/host_abi.h`. Independent `gpt-6.1-sol` medium review and
re-review passed with no remaining findings. Results remain local
interpreter/profile evidence and do not close
S2-34, S2-35, global `S2-35 → S2-34`, physical x86 validation, or deferred
combined Mac mini/SSH acceptance.

#### S2-35.25 — local-only bounded register `PUNPCKHBW`

Requires S2-34.1 raw x87/SSE state and mandatory-66 XMM decoding from S2-35.1.
Exact profile `s2-long-punpckhbw-reg-v1` admits only legacy
`66 [one optional REX.R/B] 0F 68 /r` with ModR/M.mod=3. ModR/M.reg is the
destination and ModR/M.r/m the source, extended through XMM15. For each of
eight byte lanes, `DEST[2*i] = old_DEST[8+i]` and
`DEST[2*i+1] = SRC[8+i]`; byte 0 is least significant. All destination bytes
are overwritten and the low halves of both inputs are ignored.

Coverage includes the literal high-half sentinel `08..0f` / `88..8f`,
self-alias `d8..df`, every byte value at each consumed position, low-half
independence, walking bits, all 16×16 XMM pairs and REX extensions, complete
architectural state preservation, and zero guest data-memory callbacks.
Missing state, truncation, STOP/budget atomicity, strict schema and decoder
negatives including VEX `C5 F9 68 C1`, and a handwritten literal XMM8/XMM9
fixture with JIT/native denial are covered. Other unpack widths such as
PUNPCKLWD, PUNPCKHWD, PUNPCKLQDQ and PUNPCKHQDQ, packing, shuffles, shifts,
memory, MMX, extra prefixes, REX.W/X,
VEX/EVEX, feature gates, exception delivery and physical execution remain
excluded. SSE2 and CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1 are assumptions only.
S2-34, S2-35 and global `S2-35 → S2-34` remain open; Mac mini/SSH acceptance
remains deferred.

S2-35.25 implementation check (2026-10-27): focused `od-isa` PUNPCKHBW
decoder, semantics and atomicity tests passed (3); all 319 `od-testkit` tests
passed, and the handwritten literal XMM8/XMM9 runner fixture test passed (1),
including strict VEX rejection and explicit JIT denial. The fixture's high
halves `08..0f` and `88..8f` produce `08 88 09 89 0a 8a 0b 8b 0c 8c 0d 8d
0e 8e 0f 8f`. `cargo test --workspace --offline --quiet` passed all 1,729
workspace tests. Workspace all-target offline check and
`x86_64-unknown-linux-gnu` all-target compile check passed; the Linux-gated
native-denial assertion was compile-checked but not executed on this Darwin
host. Formatting, diff whitespace checks and `graphify update .` passed; the
graph update reported its existing `include/open_desktop/host_abi.h`
extractor warning. `python3 tools/check_layout.py` still reports the linked
worktree `.git` pointer and two missing historical oracle manifest links.
Independent `gpt-6.1-sol` medium review found one documentation scope typo;
the exclusion wording was corrected and review/re-review passed with no
remaining findings. These are local interpreter/profile results only and do
not close S2-34, S2-35, global
`S2-35 → S2-34`, physical x86 validation, or deferred combined Mac mini/SSH
acceptance.

#### S2-35.26 — local-only bounded register `PUNPCKLWD`

Requires S2-34.1 raw x87/SSE state and mandatory-66 XMM decoding from S2-35.1.
Exact profile `s2-long-punpcklwd-reg-v1` admits only legacy
`66 [one optional REX.R/B] 0F 61 /r` with ModR/M.mod=3. ModR/M.reg is the
destination and ModR/M.r/m the source, extended through XMM15. For `i=0..3`,
the result words are `DEST[2*i] = old_DEST[i]` and
`DEST[2*i+1] = SRC[i]`; words are little-endian u16 values, byte order within
each word is preserved, all eight destination words are overwritten, and the
high four words of both inputs are ignored.

Coverage includes the literal word sentinel `DEST [0100,0302,0504,0706]` /
`SRC [8180,8382,8584,8786]`, self-alias, every byte value at each consumed
position, high-word independence, walking bits, all 16×16 XMM/REX combinations,
complete state preservation and zero guest data callbacks. Missing-state,
truncation and STOP/budget atomicity, strict schema/decoder negatives including
VEX `C5 F9 61 C1`, and a handwritten literal XMM8/XMM9 fixture cover boundary
and admission behavior. JIT and native execution remain denied. PUNPCKHWD,
other widths, packing, shuffles, shifts, memory, MMX, extra prefixes, REX.W/X,
VEX/EVEX execution, feature gates, exception delivery and physical execution
remain excluded. SSE2 and CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1 are assumptions only.
S2-34, S2-35, and global `S2-35 → S2-34` remain open; combined Mac mini/SSH
acceptance remains deferred.

S2-35.26 implementation checks and review outcomes are recorded in
[STAGE_2_REPORT.md](../reports/STAGE_2_REPORT.md).

#### S2-35.29 — local-only bounded register `PUNPCKHDQ`

Requires S2-34.1 raw x87/SSE state and mandatory-66 XMM decoding from S2-35.1.
The bounded profile `s2-long-punpckhdq-reg-v1` admits legacy
`66 [one optional REX.R/B] 0F 6A /r`, ModR/M.mod=3 and XMM0–15. ModR/M.reg is
the destination and ModR/M.r/m the source. The four little-endian u32 result
elements are `[old_DEST[2], SRC[2], old_DEST[3], SRC[3]]`; all destination
dwords are overwritten and both low halves are ignored.

Coverage includes the high-dword sentinel, self-alias, low-half independence,
all consumed byte values at each position, walking bits, all 16×16 XMM/REX.R/B
combinations, full-state preservation and zero data-memory callbacks. Missing
state, truncation and STOP/budget atomicity, strict decoder/schema negatives
including VEX `C5 F9 6A C1`, and a handwritten literal XMM8/XMM9 fixture cover
the boundaries. JIT and native execution remain denied. Other widths, packing,
shuffles, shifts, memory, MMX, extra prefixes, REX.W/X, VEX/EVEX execution,
feature gates, exception delivery and physical execution remain excluded.
SSE2 and CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1 are assumptions only. S2-34, S2-35,
global `S2-35 → S2-34`, and combined Mac mini/SSH acceptance remain open.
Evidence is recorded in [STAGE_2_REPORT.md](../reports/STAGE_2_REPORT.md).

#### S2-35.27 — local-only bounded register `PUNPCKHWD`

Requires S2-34.1 raw x87/SSE state and mandatory-66 XMM decoding from S2-35.1.
The bounded profile `s2-long-punpckhwd-reg-v1` admits legacy
`66 [one optional REX.R/B] 0F 69 /r` with ModR/M.mod=3 and XMM0–15. ModR/M.reg
is the destination and ModR/M.r/m the source. For `i=0..3`, result words are
`DEST[2*i] = old_DEST[4+i]` and `DEST[2*i+1] = SRC[4+i]`. Little-endian u16
ordering is preserved; all destination words are overwritten and input low
halves are ignored.

Coverage includes the high-word sentinel, self-alias, low-half independence,
all byte values at each consumed byte, walking bits, all 16×16 XMM/REX.R/B
combinations, complete state preservation, and zero guest data callbacks.
Missing state, truncation, STOP/budget atomicity, strict decoder/schema
negatives including VEX `C5 F9 69 C1`, and a literal XMM8/XMM9 fixture cover
the boundaries. JIT and native execution remain denied. Other widths, packing,
shuffles, shifts, memory, MMX, extra prefixes, REX.W/X, VEX/EVEX, feature gates,
exception delivery and physical execution remain excluded. SSE2 and
CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1 are assumptions only. S2-34, S2-35, global
`S2-35 → S2-34`, and combined Mac mini/SSH acceptance remain open. Evidence is
recorded in [STAGE_2_REPORT.md](../reports/STAGE_2_REPORT.md).

#### S2-35.28 — local-only bounded register `PUNPCKLDQ`

Requires S2-34.1 raw x87/SSE state and mandatory-66 XMM decoding from S2-35.1.
The bounded profile `s2-long-punpckldq-reg-v1` admits legacy
`66 [one optional REX.R/B] 0F 62 /r`, ModR/M.mod=3 and XMM0–15. ModR/M.reg
is the destination and ModR/M.r/m is the source. The result's four
little-endian u32 elements are `[old_DEST[0], SRC[0], old_DEST[1], SRC[1]]`.
The destination is fully overwritten; elements 2 and 3 of both inputs are
ignored; byte order within each dword is unchanged.

Coverage includes the literal dword sentinel, self-alias, upper-half
independence, all byte values at each consumed position, walking bits, all
16×16 XMM/REX.R/B combinations, full-state preservation and zero data-memory
callbacks. Missing-state, truncation and STOP/budget atomicity, strict
schema/decoder negatives including VEX `C5 F9 62 C1`, and a handwritten
literal XMM8/XMM9 fixture cover boundaries. JIT and native execution remain
denied. PUNPCKHDQ/qword, other unpack/pack/shuffle/shift operations, memory,
MMX, extra prefixes, REX.W/X, VEX/EVEX execution, feature gates, exception
delivery and physical execution remain excluded. SSE2 and CR0.EM=0, CR0.TS=0,
CR4.OSFXSR=1 are assumptions only. S2-34, S2-35, global `S2-35 → S2-34`, and
combined Mac mini/SSH acceptance remain open. Evidence is in
[STAGE_2_REPORT.md](../reports/STAGE_2_REPORT.md).

#### S2-35.30 — local-only bounded register `PUNPCKLQDQ`

Requires S2-34.1 raw x87/SSE state and mandatory-66 XMM decoding from S2-35.1.
The bounded profile `s2-long-punpcklqdq-reg-v1` admits only legacy
`66 [one optional REX.R/B] 0F 6C /r` with ModR/M.mod=3 and XMM0–15.
ModR/M.reg is the destination and ModR/M.r/m the source. The destination is
fully overwritten with the two little-endian qwords `[old_DEST[0], SRC[0]]`;
the original upper qwords are ignored. PUNPCKLQDQ has no MMX form, so missing
mandatory `66` is rejected.

Coverage includes the sentinel DEST low qword `0706050403020100` and SRC low
qword `8786858483828180`, self-aliasing, all 256 byte values in each consumed
position, walking bits, distinct qwords, ignored-upper-half independence, all
16×16 XMM/REX.R/B combinations, full-state preservation, and zero data-memory
callbacks. Missing state, truncation, STOP/budget atomicity, strict schema and
decoder negatives including missing `66` and VEX `C5 F9 6C C1`, and a literal
XMM8/XMM9 fixture cover the boundaries. JIT and native execution remain denied.
PUNPCKHQDQ, other widths, packing, shuffles, shifts, memory, extra prefixes,
REX.W/X, VEX/EVEX execution, feature gates, exception delivery and physical
execution remain excluded. SSE2 and CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1 are
assumptions only. S2-34, S2-35, global `S2-35 → S2-34`, and combined Mac
mini/SSH acceptance remain open. Evidence is recorded in
[STAGE_2_REPORT.md](../reports/STAGE_2_REPORT.md).


#### S2-35.31 — local-only bounded register `PUNPCKHQDQ`

Requires S2-34.1 raw x87/SSE state and mandatory-66 XMM decoding from S2-35.1.
The bounded profile `s2-long-punpckhqdq-reg-v1` admits legacy
`66 [one optional REX.R/B] 0F 6D /r`, ModR/M.mod=3 and XMM0–15. ModR/M.reg
is the destination and ModR/M.r/m the source. The complete destination becomes
the little-endian qwords `[old_DEST[1], SRC[1]]`; both input low qwords are
ignored. PUNPCKHQDQ has no MMX form, so missing mandatory `66` is rejected.

Coverage includes the high-qword sentinel, self-aliasing, low-half independence,
all 256 byte values at each consumed position, all walking bits, distinct qwords,
all 16×16 XMM/REX.R/B combinations, complete state/source preservation and
zero guest data callbacks. Missing state, truncation, STOP/budget atomicity,
strict schema/decoder negatives including missing `66` and VEX
`C5 F9 6D C1`, and a literal XMM8/XMM9 fixture cover boundaries. JIT and
native execution remain denied. Other unpack/pack/shuffle/shift operations,
memory, MMX, extra prefixes, REX.W/X, VEX/EVEX execution, feature gates,
exception delivery and physical execution remain excluded. SSE2 and
CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1 are assumptions only. S2-34, S2-35, global
`S2-35 → S2-34`, and combined Mac mini/SSH acceptance remain open. Evidence is
recorded in [STAGE_2_REPORT.md](../reports/STAGE_2_REPORT.md).


#### S2-35.32 — local-only bounded register `PACKSSWB`

The bounded profile `s2-long-packsswb-reg-v1` admits only legacy
`66 [one optional REX.R/B] 0F 63 /r`, ModR/M.mod=3 and XMM0–15.
ModR/M.reg is destination and ModR/M.r/m is source. Read eight signed
little-endian i16 words from the original destination, then eight from the
original source; clamp each independently to [-128, 127] and write the eight
destination results into bytes 0–7 and eight source results into bytes 8–15.
This output is concatenated in operand order, not interleaved. Source is
preserved. SSE2 and CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1 are assumptions only.

Coverage includes -129/-128/-127, 127/128, 0100 and ff00 sentinels; every raw
i16 value at each of the eight positions in each operand against a widened
i32 reference; all 16×16 XMM/REX.R/B pairs; distinct order sentinels,
self-aliasing, full architectural state and source preservation, and zero data
callbacks. Truncation, missing-state, strict decoder negatives including
VEX `C5 F9 63 C1`, and a handwritten literal XMM8/XMM9 fixture are covered.
JIT and native execution are denied. PACKUSWB, PACKSSDW, other packing or
shuffle/shift operations, memory, MMX, extra prefixes, REX.W/X, VEX/EVEX,
feature gates, exception delivery, and physical execution remain excluded.
S2-34, S2-35, global `S2-35 → S2-34`, and combined Mac mini/SSH acceptance
remain open. Evidence is recorded in [STAGE_2_REPORT.md](../reports/STAGE_2_REPORT.md).


#### S2-35.35 — local-only bounded register `PSHUFD`

The bounded profile `s2-long-pshufd-reg-v1` admits exactly
`66 [one optional REX.R/B] 0F 70 /r ib`, with ModR/M.mod=3 and XMM0–15.
ModR/M.reg is the destination and ModR/M.r/m is the source. For each of four
destination dwords, the corresponding 2-bit field in imm8 selects one of the
four dwords from the original source value; all selections are simultaneous.
This also defines destination/source aliasing. Test all 256 control bytes for
all 16×16 register pairs, strict sibling negatives, state/source preservation,
missing state, truncated imm8 fetch/IP atomicity, stop/budget, no data callback,
and literal fixture admission. Keep native/JIT and physical execution denied;
SSE2 and CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1 remain assumptions only. Other
shuffle opcodes, memory, prefixes beyond the exact form, REX.W/X, VEX/EVEX,
feature gates, and guest exception delivery are excluded. This child leaves
S2-34, S2-35, global `S2-35 → S2-34`, and combined Mac mini/SSH acceptance
open.

#### S2-35.37 — local-only bounded register immediate `PSLLW`

The profile `s2-long-psllw-imm-reg-v1` admits exactly
`66 [optional REX.B] 0F 71 /6 ib`, ModR/M.mod=3, with XMM0–15. ModR/M.r/m
selects the destination; `/6` is mandatory. Each of eight little-endian u16
lanes is shifted logically left by the unsigned imm8. Counts 0–15 retain the
low 16 result bits; counts >=16 clear all lanes without masking the count.
There is no lane carry, and flags/MXCSR are unchanged. SSE2 and CR0.EM=0,
CR0.TS=0, CR4.OSFXSR=1 remain assumptions only.

Local coverage checks all 256 counts with lane sentinels on every XMM destination,
every 65,536 lane words for counts 0–15 against a widened reference,
strict encoding/schema negatives, full-state/source preservation, no data
callbacks, and STOP/zero-budget atomicity. The handwritten XMM9 fixture is
checked through interpreter execution with JIT denied; a Linux-only native
admission denial guard is compiled for the Linux target on this Darwin host, but
not executed there. Tests also verify the missing immediate fetch at 0x1000
after four instruction bytes at 0xffc, plus successful five-byte fetch across
that boundary. The native runner rejects this local-only profile before mapping
or process setup. Formatting, focused PSLLW interpreter and fixture tests,
host/Linux-target test compilation, and Graphify passed; independent review
found no remaining issues. Other shifts, memory/MMX forms, REX.R/X/W, VEX/EVEX,
feature gates, exception delivery and physical execution remain excluded.
S2-34, S2-35, global `S2-35 → S2-34`, and combined Mac mini/SSH acceptance
remain open.

#### S2-35.38 — local-only bounded register immediate `PSRLW`

The profile `s2-long-psrlw-imm-reg-v1` admits exactly
`66 [optional REX.B] 0F 71 /2 ib`, ModR/M.mod=3, with XMM0–15. ModR/M.r/m
selects the destination and `/2` is mandatory; `/6` belongs to PSLLW and is
rejected by this profile. Each of eight little-endian u16 lanes shifts
logically right by the unsigned imm8. Counts 0–15 shift and zero-fill; counts
>=16 clear every lane without masking the count. No lane carry occurs, and
flags/MXCSR are unchanged. SSE2 and CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1 remain
assumptions only.

Local checks cover every word value for counts 0–15 against an independent
widened reference, all 256 counts on every XMM destination (including the
0x8000/0xffff sentinels), strict selector/encoding negatives, full-state
preservation, no data callbacks, and STOP/zero-budget atomicity. The handwritten
XMM9 fixture exercises interpreter execution with JIT denied. Native denial is
checked before mapping/process setup; Linux-only guard is compile-only on Darwin.
Instruction-fetch truncation/page-crossing and complete cross-boundary fetch
are covered. Other shifts, memory/MMX forms, REX.R/X/W, VEX/EVEX, feature gates,
exception delivery and physical execution remain excluded. S2-34, S2-35, global
`S2-35 → S2-34`, and combined Mac mini/SSH acceptance remain open.

#### S2-35.39 — local-only bounded register immediate `PSRAW`

The profile `s2-long-psraw-imm-reg-v1` admits exactly
`66 [no REX or optional 41] 0F 71 /4 ib`, ModR/M.mod=3, with XMM0–15.
ModR/M.r/m selects the destination and `/4` is mandatory; `/2` PSRLW and `/6`
PSLLW remain distinct sibling operations and are rejected by the PSRAW schema.
Each of eight signed i16 lanes shifts arithmetically right by the unsigned,
unmasked imm8. Counts 0–15 sign-fill; counts >=16 produce `ffff` for negative
input lanes and `0000` otherwise. Flags and MXCSR remain unchanged. SSE2 and
CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1 are assumptions only.

Local checks cover all 65,536 lane values for counts 0–15 against an independent
widened floor/sign-fill reference, all 256 counts on each of 16 XMM destinations,
the `8000>>1=c000`, `ffff>>1=ffff`, `fffd>>1=fffe`, and `7fff>>1=3fff`
sentinels, full-state preservation, no data callbacks, missing SSE state,
STOP/zero-budget atomicity, strict selector/encoding negatives, and immediate
fetch truncation/page crossing. The handwritten XMM9 fixture checks interpreter
execution with JIT denied. Native denial is checked before mapping/process
setup; Linux-only guard is compile-only on Darwin. Other shifts, memory/MMX,
REX.R/X/W and bare REX, VEX/EVEX, feature gates, exception delivery and physical
execution remain excluded. S2-34, S2-35, global `S2-35 → S2-34`, and combined
Mac mini/SSH acceptance remain open.

#### S2-35.41 — local-only bounded register immediate `PSLLD`

The profile `s2-long-pslld-imm-reg-v1` admits exactly
`66 [no REX or optional 41] 0F 72 /6 ib`, ModR/M.mod=3. ModR/M.r/m
selects destination XMM0–15; `/6` is required. Four independent little-endian
u32 lanes shift logically left by unsigned imm8. Counts 0–31 retain the low
32 result bits; counts >=32 clear every lane without masking the count. There
is no cross-lane carry; flags and MXCSR are unchanged. SSE2 and CR0.EM=0,
CR0.TS=0, CR4.OSFXSR=1 remain assumptions only.

Local checks cover all 256 counts on each of 16 destinations and compare the
sub-32 results with a u64 reference across boundary values, walking bits and a
carry-chain vector. They also cover strict /6 and 0F 72 sibling negatives,
full-state/source preservation, no data callbacks, missing SSE state,
STOP/zero-budget atomicity, immediate truncation/page crossing, and complete
cross-boundary fetch. The handwritten XMM9 fixture checks interpreter execution
with JIT denied. Native denial is before mapping/process setup; Linux-only guard
is compile-only on Darwin. Other shifts, memory/MMX, REX.R/X/W, extra prefixes,
VEX/EVEX, feature gates, exception delivery and physical execution remain
excluded.

S2-35.41 implementation check (2026-10-27): `cargo test -p od-isa pslld
--offline` passed (5 tests), including all 256 counts on all 16 destinations,
u64 reference vectors, strict decode negatives, page-crossing fetch, state and
atomicity checks. `cargo test -p od-runner --test fixture_v2 pslld --offline`
passed (1 test), verifying the handwritten fixture, profile rejection matrix,
and JIT denial. Formatting, workspace all-target offline check, Linux-target
x86-runner test compilation, `git diff --check`, and Graphify update passed.
Linux native denial
is compile-only on this Darwin host. Independent `gpt-6.1-sol` medium review
found no remaining findings and independently reran the PSLLD interpreter and
fixture tests.

#### S2-35.45 — local-only bounded register immediate `PSRLQ`

The profile `s2-long-psrlq-imm-reg-v1` admits exactly
`66 [no REX or optional 41] 0F 73 /2 ib`, ModR/M.mod=3. ModR/M.r/m
selects destination XMM0–15; `/2` is required, `/6` PSLLQ and `/3` PSRLDQ
are rejected. Two independent little-endian u64 lanes shift logically right
by unmasked unsigned imm8. Counts 0–63 zero-fill; counts >=64 clear both
lanes. There is no cross-qword carry; flags and MXCSR are unchanged. SSE2 and
CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1 remain assumptions only.

Local checks cover all 256 counts on all 16 destinations, an independent u128
oracle with walking-bit and mixed-qword inputs, right-shift boundary sentinels,
strict selector and prefix negatives, full-state/source preservation, no data
callbacks, missing SSE state, STOP/zero-budget atomicity, and immediate
truncation/page crossing with successful complete fetch. The handwritten XMM9
fixture checks interpreter execution with JIT denied. Native denial is before
mapping/process setup; Linux-only guard compilation is not physical execution.
Other shifts, memory/MMX, REX.R/X/W, extra prefixes, VEX/EVEX, feature gates,
exception delivery and physical execution remain excluded.

S2-35.45 implementation check (2026-10-27): `cargo test -p od-isa psrlq
--offline` passed (5 focused tests), and `cargo test -p od-runner --test
fixture_v2 psrlq --offline` passed (1 fixture test). Formatting, workspace
all-target offline check, Linux-target `od-x86-runner --tests` compilation,
`git diff --check`, and Graphify update passed. Independent `gpt-6.1-sol`
medium review found no remaining findings and independently reran the focused
interpreter and fixture tests. Linux native denial remains compile-only on this
Darwin host; physical SSE/SSE2 execution remains `not_run`.

#### S2-35.44 — local-only bounded register immediate `PSLLQ`

The profile `s2-long-psllq-imm-reg-v1` admits exactly
`66 [no REX or optional 41] 0F 73 /6 ib`, ModR/M.mod=3. ModR/M.r/m
selects destination XMM0–15; `/6` is required and `/7` PSLLDQ is rejected.
Two independent little-endian u64 lanes shift logically left by unsigned imm8.
Counts 0–63 retain the low 64 result bits; counts >=64 clear both lanes
without masking the count. There is no cross-qword carry; flags and MXCSR are
unchanged. SSE2 and CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1 remain assumptions only.

Local checks cover all 256 counts on each of 16 destinations, an independent
u128 widened oracle for counts below 64 with raw boundary, walking-bit and
carry-chain vectors in both lanes, explicit `0000000080000000<<1`, `1<<63`,
and `8000000000000000<<1` sentinels, strict `/6` and `/7` encoding, state and
no-callback preservation, missing SSE state, STOP/zero-budget atomicity, and
immediate truncation/page crossing with successful complete fetch. The
handwritten XMM9 fixture checks interpreter execution with JIT denied. Native
denial is before mapping/process setup; Linux-only guard compilation is not
physical execution. Other shifts, memory/MMX, REX.R/X/W, bare REX, extra
prefixes, VEX/EVEX, feature gates, exception delivery and physical execution
remain excluded.

S2-35.44 implementation check (2026-10-27): `cargo test -p od-isa psllq
--offline` passed (5 tests), covering all counts/destinations, u128 widened and
walking-bit vectors, strict decode and `/7` sibling rejection, fetch boundaries,
state preservation, missing SSE, and atomicity. `cargo test -p od-runner
--test fixture_v2 psllq --offline` passed (1 test), verifying the handwritten
XMM9 fixture, profile rejection matrix, and JIT denial. `cargo fmt --all
-- --check`, `cargo check --workspace --all-targets --offline`, and
`cargo check -p od-x86-runner --tests --target x86_64-unknown-linux-gnu
--offline` passed. `cargo test ... --no-run` for that Linux target compiled the
runner but could not link on this Darwin host because the configured `cc`/`ld`
is a Darwin linker and rejects Linux GNU linker flags; physical native denial
therefore remains compile-only here. Graphify update and `git diff --check`
passed. Independent `gpt-6.1-sol` medium review found no remaining findings and
independently reran the PSLLQ interpreter and fixture tests.

#### S2-35.46 — local-only bounded register immediate `PSLLDQ`

The profile `s2-long-pslldq-imm-reg-v1` admits exactly
`66 [no REX or optional 41] 0F 73 /7 ib`, ModR/M.mod=3. ModR/M.r/m
selects destination XMM0–15; `/7` is required, while `/6` PSLLQ and `/3`
PSRLDQ remain distinct selectors. The unsigned imm8 is an unmasked byte count:
for counts below 16, output byte `i` receives the original destination byte
`i-count` when `i >= count`, otherwise zero. Counts >=16 clear the full XMM.
The snapshot-based operation handles movement across the qword boundary;
flags and MXCSR are unchanged. SSE2 and CR0.EM=0, CR0.TS=0,
CR4.OSFXSR=1 remain assumptions only.

Local checks cover every count on all 16 destinations against an independent
byte-index oracle, all 128 walking-one bits, distinct bytes 1–16, count 1
across byte 7/8, count 8 across the qword boundary, and counts 0/15/16/17/32/255.
They also cover exact `/7` and PSLLQ `/6` separation, strict prefixes and
register-only form, full-state/no-callback preservation, missing SSE,
STOP/zero-budget atomicity, and immediate truncation/page crossing followed by
complete fetch. The handwritten XMM9 fixture checks interpreter execution
with JIT denied; native admission remains denied before native execution.
Physical SSE/SSE2 execution remains `not_run`.

S2-35.46 implementation check (2026-10-27): focused checks and independent
review are recorded in [the stage report](../reports/STAGE_2_REPORT.md#s2-3546--local-pslldq-immediate-register-profile).

#### S2-35.47 — local-only bounded register immediate `PSRLDQ`

The profile `s2-long-psrldq-imm-reg-v1` admits exactly
`66 [no REX or optional 41] 0F 73 /3 ib`, ModR/M.mod=3. ModR/M.r/m selects
XMM0–15, with REX.B extending the destination. `/3` is required; `/2` PSRLQ
and `/7` PSLLDQ are separate selectors. The unsigned imm8 is an unmasked byte
count: for counts below 16, output byte `i` receives original destination byte
`i+count` when the source index is below 16, else zero. Counts >=16 clear all
bytes. The operation uses the original XMM snapshot and preserves flags/MXCSR.
SSE2 preconditions remain assumptions; memory/MMX, other prefixes and JIT/native
execution are outside this profile.

S2-35.47 implementation check (2026-10-28): focused PSRLDQ interpreter tests
passed (4), and the literal XMM9 fixture passed (1), including the independent
byte-index oracle, all walking-one positions, qword-boundary sentinels, strict
selector negatives, and x87-seeded missing-SSE/STOP/zero-budget atomicity.
Workspace all-target checks, formatting, Linux-target x86-runner test
compilation, `git diff --check`, and Graphify passed. Independent Sol review
found no remaining findings and independently reran the neighboring ISA suites
and all fixture tests. Native denial is compile-only on Darwin; physical
SSE/SSE2 execution is `not_run`. S2-34, S2-35, and global `S2-35 → S2-34` remain
open.

#### S2-35.42 — local-only bounded register immediate `PSRLD`

The profile `s2-long-psrld-imm-reg-v1` admits exactly
`66 [no REX or optional 41] 0F 72 /2 ib`, ModR/M.mod=3. ModR/M.r/m
selects destination XMM0–15; `/2` is required and `/6` is the distinct PSLLD
sibling. Four independent little-endian u32 lanes shift logically right by
unsigned imm8. Counts 0–31 shift with zero-fill; counts >=32 clear every lane
without masking the count. There is no cross-lane carry; flags and MXCSR are
unchanged. SSE2 and CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1 remain assumptions only.

Local checks cover all 256 counts on each of 16 destinations against an
independent u64 reference for sub-32 counts, the `80000000>>1=40000000`,
`ffffffff>>1=7fffffff`, `00010000>>1=00008000`, and `80000000>>31=1`
sentinels, walking bits in each lane, mixed lanes, and no cross-lane carry.
Strict selector/encoding negatives, full-state preservation, no data
callbacks, missing SSE state, STOP/zero-budget atomicity, immediate
truncation/page crossing, and complete cross-boundary fetch are covered. A
handwritten XMM9 fixture checks interpreter execution with JIT denied. Native
denial is before mapping/process setup; Linux-only guard compilation is not
physical execution. Other shifts, memory/MMX, REX.R/X/W, extra prefixes,
VEX/EVEX, feature gates, exception delivery, and physical execution remain
excluded.

S2-35.42 implementation check (2026-10-27): `cargo test -p od-isa psrld
--offline` passed (6 focused tests); `cargo test -p od-runner --test fixture_v2
psrld --offline` passed (1 fixture test), verifying the handwritten fixture,
strict profile rejection matrix, and JIT denial. `cargo fmt --all -- --check`,
`cargo check --workspace --all-targets --offline`, x86_64 Linux-target
`od-x86-runner` test compilation, `git diff --check`, and `graphify update .`
passed. The Linux native-denial test was compile-checked only on this Darwin
host. Independent `gpt-6.1-sol` medium review is complete: the reviewer reran
the focused PSRLD interpreter tests (6/6) and fixture test (1/1), and confirmed
the workspace all-target check. Native runtime and physical SSE/SSE2 execution
were not run. S2-34, S2-35, global `S2-35 → S2-34`, physical SSE/SSE2
validation, and combined Mac mini/SSH acceptance remain open.

S2-35.39 implementation check (2026-10-27): focused PSRAW interpreter tests
passed (5), including exhaustive counts/word values, every count on all XMM
destinations, sentinels, strict encoding, page crossing, state/no-callback, and
atomicity; the literal fixture CLI test passed (1), including sibling-selector
schema negatives and JIT denial. PSRLW/PSLLW reciprocal interpreter tests (6
each) and fixture tests (1 each) passed. Workspace all-target offline check,
x86_64 Linux runner test compilation, formatting, `git diff --check`, and
Graphify update passed. Linux native denial is compile-only on this Darwin host;
`tools/check_layout.py` still reports the pre-existing unmapped `.git` root and
two absent oracle manifests in the stage report. Independent
`gpt-6.1-sol` medium review found no remaining issues; the reviewer reran the
focused PSRAW interpreter and fixture tests plus the diff whitespace check.
This is local interpreter/profile evidence only; parent S2-34, S2-35, global
`S2-35 → S2-34`, physical x86 validation, and combined Mac mini/SSH acceptance
remain open.


#### S2-35.43 — local-only bounded register immediate `PSRAD`

The profile `s2-long-psrad-imm-reg-v1` admits exactly
`66 [no REX or optional 41] 0F 72 /4 ib`, ModR/M.mod=3. ModR/M.r/m selects
destination XMM0–15; `/4` is required and `/2` PSRLD and `/6` PSLLD are
distinct siblings. Four independent little-endian signed i32 lanes shift
arithmetically right by unsigned, unmasked imm8. Count 0 preserves the lanes;
counts 0–31 sign-fill, and counts >=32 yield `ffffffff` for negative input
lanes and zero for nonnegative lanes. Flags and MXCSR are unchanged. SSE2 and
CR0.EM=0, CR0.TS=0, CR4.OSFXSR=1 remain assumptions only.

Focused checks cover every unsigned count on all 16 XMM destinations against
an independent i64 floor-division/sign-fill oracle, including reproducible raw
i32 and boundary inputs; walking bits in every lane/destination; the
`80000000>>1=c0000000`, `ffffffff>>1=ffffffff`, `fffffffd>>1=fffffffe`,
`7fffffff>>1=3fffffff`, and `00008000>>1=00004000` sentinels; strict sibling,
prefix and memory-form negatives; full-state preservation and no data
callbacks; missing SSE state and STOP/zero-budget atomicity; and immediate
fetch truncation/page crossing with successful cross-boundary fetch. The
handwritten XMM9 fixture exercises the interpreter with JIT denied. Native
admission is denied before mapping/process setup; its Linux guard is compile
only on Darwin. Other shifts, memory/MMX, REX.R/X/W, extra prefixes, VEX/EVEX,
feature gates, guest exception delivery and physical execution are excluded.

S2-35.43 implementation check (2026-10-27): `cargo test -p od-isa psrad
--offline` passed (5 focused tests), and `cargo test -p od-runner --test
fixture_v2 psrad --offline` passed (1 fixture test). `cargo fmt --all --
--check`, `cargo check --workspace --all-targets --offline`, Linux-target
`od-x86-runner` test compilation, `git diff --check`, and `graphify update .`
passed. The Linux-only native-denial guard is compile-checked, not executed on
this Darwin host. Independent `gpt-6.1-sol` medium review found no remaining
findings and independently reran the PSRAD interpreter and fixture tests.
Physical SSE/SSE2 execution is not claimed. S2-34, S2-35, global
`S2-35 → S2-34`, and combined Mac mini/SSH acceptance remain open.


#### S2-35.33 — local-only bounded register `PACKUSWB`

The bounded profile `s2-long-packuswb-reg-v1` admits legacy
`66 [one optional REX.R/B] 0F 67 /r`, ModR/M.mod=3 and XMM0–15. The `67`
after `0F` is the opcode byte, not the address-size prefix. ModR/M.reg is the
destination and ModR/M.r/m the source. Read eight signed little-endian i16
words from the original destination, then eight from the original source; clamp
each independently to [0,255] and write the destination results into bytes 0–7
and source results into bytes 8–15. This is concatenated operand order, not
interleaved; a distinct source is preserved, and self-pack reads the original
aliased value before writing. SSE2 and CR0.EM=0, CR0.TS=0,
CR4.OSFXSR=1 are assumptions only.

Coverage includes `ffff` (-1), `8000` and `ff00` signedness sentinels;
0000/0001/007f/0080/00ff/0100/7fff clamp boundaries; all 65,536 i16 values in
each of eight positions for DEST and SRC against a widened reference; all
16×16 XMM/REX.R/B pairs, distinct order sentinels, self-aliasing, full-state
and source preservation, and zero data callbacks. Truncation, missing SSE state,
STOP/budget atomicity, strict decoder/schema negatives including VEX
`C5 F9 67 C1`, and a handwritten XMM8/XMM9 fixture cover boundaries. JIT and
native execution are denied. PACKSSDW, other pack/shuffle/shift operations,
memory, MMX, extra prefixes, REX.W/X, VEX/EVEX, feature gates, exception
delivery and physical execution remain excluded. S2-34, S2-35, global
`S2-35 → S2-34`, and combined Mac mini/SSH acceptance remain open. Evidence is
recorded in [STAGE_2_REPORT.md](../reports/STAGE_2_REPORT.md).


#### S2-35.34 — local-only bounded register `PACKSSDW`

The bounded profile `s2-long-packssdw-reg-v1` admits legacy
`66 [one optional REX.R/B] 0F 6B /r`, ModR/M.mod=3 and XMM0–15. ModR/M.reg is
the destination and ModR/M.r/m the source. Read four signed little-endian i32
lanes from the original destination, then four from the original source;
clamp each to [-32768,32767] before narrowing to little-endian i16. The eight
results are concatenated in operand order into destination words 0–3 and 4–7.
A distinct source and all non-destination architectural state are preserved;
self-pack reads the original aliased value. SSE2 and CR0.EM=0, CR0.TS=0,
CR4.OSFXSR=1 are assumptions only.

Coverage includes signed limits and adjacent values, signed dword extrema,
exhaustive all 65,536 sign-extended i16 inputs at each of four positions in
both operands against an independent widened reference, all 16×16 XMM/REX.R/B
pairs, full-state/source preservation and zero data callbacks. Truncation,
missing state, STOP/budget atomicity, strict decoder/schema negatives
including VEX `C5 F9 6B C1`, and a handwritten literal XMM8/XMM9 fixture cover
the profile boundary. JIT and native execution are denied. Other packing,
shuffles/shifts, memory, MMX, extra prefixes, REX.W/X, VEX/EVEX execution,
feature gates, exception delivery and physical execution remain excluded.
S2-34, S2-35, global `S2-35 → S2-34`, and combined Mac mini/SSH acceptance
remain open. Evidence is recorded in [STAGE_2_REPORT.md](../reports/STAGE_2_REPORT.md).

#### S2-35.36 — bounded coverage and acceptance inventory

This inventory reconciles implemented local slices with the task list; it
does not define the eventual CPU profile or imply that every x86 form is
required. “Implemented” below means only the exact local diagnostic forms
named here. Exact encodings and profile names are listed in each numbered
subsection; all profiles are register-only and reject forms outside their
allowlist. The ordinary packed arithmetic, logic, comparison, unpack and pack
forms use `66 [REX.R/B] 0F opcode /r` with ModR/M.mod=3 and XMM0–15. The
immediate shift profiles use grouped selectors (`/6`, `/2`, `/4`, `/7` or `/3`)
with ModR/M.r/m selecting the XMM destination plus imm8; `/7` PSLLDQ and `/3`
PSRLDQ shift the whole XMM, unlike the independent qword lanes in PSLLQ/PSRLQ.
`PSHUFD` uses `/r` plus imm8. The detailed literal fixtures are indexed in the
[Stage 2 fixture README](../../tests/fixtures/x86/stage2/README.md).

| Operation group | Local forms evidenced in S2-35 | Local evidence and next technical dependency |
|---|---|---|
| Arithmetic | `PADDB` FC, `PSUBB` F8, `PADDUSB` DC, `PSUBUSB` D8, `PADDW` FD, `PSUBW` F9, `PADDUSW` DD, `PSUBUSW` D9, `PADDSB` EC, `PSUBSB` E8, `PADDSW` ED, `PSUBSW` E9, `PADDD` FE, `PSUBD` FA, `PADDQ` D4, `PSUBQ` FB | Each byte is the final opcode in `66 [REX.R/B] 0F xx /r`, ModR/M.mod=3. Per-profile interpreter/decoder coverage and literal XMM fixtures exercise lane order, wrap or saturation, state preservation, and profile rejection; several entries record exhaustive lane/register matrices. Other arithmetic mnemonics or memory/MMX forms are not selected. Any extension first depends on the parent profile choosing exact forms; reviewed byte/word/dword/qword operations are local semantic precedents, not evidence of parent acceptance. |
| Logic | `PXOR` EF, `PAND` DB, `PANDN` DF, `POR` EB | Each is a distinct exact `66 [REX.R/B] 0F opcode /r` register-only profile with ModR/M.mod=3. Local decoder/interpreter tests, handwritten fixtures, full-state preservation and strict profile negatives cover each named form. These bounded local slices do not settle parent S2-35/S2-34 selection, profile-wide acceptance, or native-oracle acceptance. No other logical forms or memory/MMX variants are implied. |
| Comparison | `PCMPEQB` 74, `PCMPEQW` 75, `PCMPEQD` 76, `PCMPGTB` 64, `PCMPGTW` 65, `PCMPGTD` 66 | Each byte is the final opcode in `66 [REX.R/B] 0F xx /r`, ModR/M.mod=3. Local fixtures and per-profile lane/equality/signed-order checks; S2-35.21 records exhaustive raw byte-pair and 16×16 register-pair coverage. `PCMPEQQ` (SSE4.1) and `PCMPGTQ` (SSE4.2) are explicitly outside these SSE2 slices. More forms require explicit profile selection and parent acceptance. |
| Unpack | `PUNPCKLBW` 60, `PUNPCKHBW` 68, `PUNPCKLWD` 61, `PUNPCKHWD` 69, `PUNPCKLDQ` 62, `PUNPCKHDQ` 6A, `PUNPCKLQDQ` 6C, `PUNPCKHQDQ` 6D | Each byte is the final opcode in `66 [REX.R/B] 0F xx /r`, ModR/M.mod=3. Local register fixtures and lane-selection/state checks exist for all eight listed forms. No new local opcode dependency is evidenced; parent native-oracle acceptance remains outstanding. |
| Pack | `PACKSSWB` 63, `PACKUSWB` 67, `PACKSSDW` 6B | Each byte is the final opcode in `66 [REX.R/B] 0F xx /r`, ModR/M.mod=3. Local fixtures and boundary/extrema checks; S2-35.33/.34 additionally record exhaustive sign-extended input-position checks and register-pair coverage. These are the only pack forms inventoried here. |
| Shuffle | `PSHUFD` 70 + imm8 | `66 [REX.R/B] 0F 70 /r ib`, ModR/M.mod=3. Exact register-only profile and fixture; local tests cover all 256 controls and all 16×16 register pairs, aliasing, fetch truncation and strict negatives. Other shuffle forms have no selected S2-35 profile here. |
| Shifts | Word forms: `PSLLW` `0F 71 /6` + imm8 (`s2-long-psllw-imm-reg-v1`); `PSRLW` `0F 71 /2` + imm8 (`s2-long-psrlw-imm-reg-v1`); `PSRAW` `0F 71 /4` + imm8 (`s2-long-psraw-imm-reg-v1`). Dword forms: `PSLLD` `0F 72 /6` + imm8 (`s2-long-pslld-imm-reg-v1`); `PSRLD` `0F 72 /2` + imm8 (`s2-long-psrld-imm-reg-v1`); `PSRAD` `0F 72 /4` + imm8 (`s2-long-psrad-imm-reg-v1`). Qword-lane logical pair: `PSLLQ` `0F 73 /6` + imm8 (`s2-long-psllq-imm-reg-v1`); `PSRLQ` `0F 73 /2` + imm8 (`s2-long-psrlq-imm-reg-v1`). Whole-XMM byte shifts: `PSLLDQ` `0F 73 /7` + imm8 (`s2-long-pslldq-imm-reg-v1`); `PSRLDQ` `0F 73 /3` + imm8 (`s2-long-psrldq-imm-reg-v1`). | All forms are exact register-only profiles, `66 [no REX or optional 41]`, ModR/M.mod=3, with ModR/M.r/m selecting XMM0–15 and one unsigned, unmasked imm8 byte. For the earlier profiles: word forms use selectors `/6`, `/2`, `/4` respectively; dword forms use `/6`, `/2`, `/4`; qword-lane PSLLQ/PSRLQ use `/6` and `/2` at `0F 73`. Word shifts clear lanes at counts >=16 except PSRAW, which sign-fills negative lanes and clears nonnegative lanes; logical dword shifts clear lanes at counts >=32 while PSRAD sign-fills negative lanes and clears nonnegative lanes; each independent qword lane in PSLLQ/PSRLQ clears at counts >=64. PSLLDQ `/7` shifts the entire 16-byte XMM toward higher byte indices: result byte `i` is original byte `i-count` when `i >= count`, else zero. PSRLDQ `/3` shifts the entire XMM toward lower byte indices: result byte `i` is original byte `i+count` when that index is below 16, else zero. Both whole-register forms clear all bytes at counts >=16 and cross the qword boundary; they are distinct operations from the two independent qword-lane shifts. Focused local interpreter tests passed: `cargo test -p od-isa psllw --offline` (6), `psrlw` (6), `psraw` (5), `pslld` (5), `psrld` (6), `psrad` (5), `psllq` (5), `psrlq` (5), `pslldq` (5), and `psrldq` (4). Handwritten XMM9 fixtures `s2_35_37_psllw_imm_register.case.json`, `s2_35_38_psrlw_imm_register.case.json`, `s2_35_39_psraw_imm_register.case.json`, `s2_35_41_pslld_imm_register.case.json`, `s2_35_42_psrld_imm_register.case.json`, `s2_35_43_psrad_imm_register.case.json`, `s2_35_44_psllq_imm_register.case.json`, `s2_35_45_psrlq_imm_register.case.json`, `s2_35_46_pslldq_imm_register.case.json`, and `s2_35_47_psrldq_imm_register.case.json` each passed their `fixture_v2` test (one per form), confirming interpreter execution with JIT denied. Native admission remains denied before execution; Linux-target x86-runner test compilation for the reviewed immediate-shift slices is compile-only on Darwin, not an executed Linux denial test. Physical SSE/SSE2 execution is `not_run`. Other unselected shift forms and qword arithmetic are not implied as inventory requirements. |

For every implemented row, the local contract is register-only and assumes
SSE2 plus CR0.EM=0, CR0.TS=0 and CR4.OSFXSR=1 where the profile says so.
Memory operands, memory-fault delivery, alignment behavior, MMX, unlisted
prefix/REX encodings, VEX/EVEX, control-state transitions and general CPU
feature gating are not established by these slices. Local implementation
presence is recorded by the named profiles and focused interpreter/fixture
evidence in the plan and report. Native admission remains closed for these
diagnostic profiles; where Linux-only denial guards exist, their compile-only
result on macOS is not an executed admission test. This is distinct from
physical execution. Physical SSE/SSE2 execution is `not_run`: the current oracle
harness admits integer profiles only, as recorded by [S2-34.10](#s2-3410--closure-inventory-and-remaining-acceptance).

S2-34.10 remains the acceptance boundary: completion still requires an explicit
parent profile and real native-oracle register and memory cases for required
forms, with faults, alignment and feature/state requirements accounted for.
Native denial checks and local interpreter PASS do not satisfy that boundary.
The cross-cutting dependencies include raw SIMD state and gates from S2-34,
profile/CPUID decisions, and required save/restore or extended-state capture
from S2-39/S2-40; this inventory makes no CPUID, boot, allowlist, or extended
state decision. S2-34, S2-35, and global `S2-35 → S2-34` remain open; the
combined Mac mini/SSH acceptance remains deferred.

#### S2-35.49 — local-only bounded register `PAND`

Add exact profile `s2-long-pand-reg-v1` for `66 [one optional REX.R/B] 0F DB /r`,
with ModR/M.mod=3. Mandatory `66` selects XMM/SSE2; bare `0F DB` is MMX and
remains rejected. ModR/M.reg is the XMM destination and ModR/M.r/m is the
source; REX.R and REX.B independently extend them through XMM15. Execution
computes `DEST = original_DEST & original_SRC` for all 128 raw bits and
preserves flags, MXCSR, x87, GPRs and other XMM registers. Memory, REX.W/X,
other/duplicate prefixes, VEX/EVEX, JIT/native admission and physical execution
remain outside this local slice. SSE2 availability and CR0/CR4 preconditions
are assumed; control-register gates and `#UD/#NM` delivery are not modeled.

Local verification covers every 128-bit truth-table position, all-zero,
all-one, alternating and upper/lower sentinels, identity/zero behavior, all
16×16 register pairs and independent REX.R/B extension, source preservation and
aliasing, full-state/no-data-callback behavior, missing SSE with seeded x87,
STOP/zero-budget/truncated-fetch atomicity, and strict MMX/sibling/memory/prefix
rejection. A handwritten high-XMM fixture exercises the interpreter and
remains denied by JIT and native execution. This child does not close S2-35,
S2-34, or their global dependency.

S2-35.49 implementation check (2026-10-28): the PAND profile, interpreter
semantics, schema-v2 fixture and fail-closed JIT/native boundaries are in place.
`cargo test -p od-isa pand --offline` passed (6 focused tests),
`cargo test -p od-testkit pand --offline` passed (2 tests), and
`cargo test -p od-runner --test fixture_v2 pand --offline` passed (1 test).
Workspace format/check, Linux-target x86-runner test compilation, Graphify
update and `git diff --check` passed. The Linux-only native-denial test was
compile-checked but not executed on this Darwin host; physical SSE2 remains
`not_run`. Independent review is pending. Parent S2-35/S2-34 remain open.
See the [report](../reports/STAGE_2_REPORT.md#s2-3549--local-pand-register-profile),
[CPU contract](../specs/CPU_CONTRACT.md#local-register-only-pand-profile--s2-3549),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-3549-local-register-pand-diagnostic-profile).

#### S2-35.50 — local-only bounded register `PANDN`

Add the distinct profile `s2-long-pandn-reg-v1` for exact `66 [one optional
REX.R/B] 0F DF /r`, ModR/M.mod=3. ModR/M.reg is the XMM destination and r/m
is the source, independently extended to XMM0–XMM15. Each bit computes
`DEST = ~old_DEST & SRC` over the full 128-bit register; flags, MXCSR, x87,
GPRs, and all other XMM registers are preserved. PAND (`0F DB`), POR, PXOR,
MMX, memory, REX.W/X, other or duplicate prefixes, VEX/EVEX, JIT/native
admission, and physical execution are outside this local diagnostic slice.
SSE2 and CR0/CR4 preconditions are assumptions; control gates and
`#UD/#NM` delivery are not modeled.

Verification covers all 16×16 register pairs and independent REX.R/B mapping,
all 128 bit positions with four truth rows, asymmetric operand order, zero/all
one and alternating masks, aliasing, full-state preservation and no data
callbacks, missing SSE with seeded x87, STOP/zero-budget/truncated-fetch
atomicity, and strict decoder/profile negatives. The handwritten XMM8/XMM9
fixture is exercised by the interpreter and denied by JIT and native paths.
Verification (2026-10-28): `cargo test -p od-isa pandn --offline` passed (3),
`cargo test -p od-testkit pandn --offline` passed (2), and
`cargo test -p od-runner --test fixture_v2 pandn --offline` passed (1). The
handwritten fixture produced the literal XMM8 result `10101010101010101010101010101010`;
its JIT-denial check passed. `cargo fmt --all -- --check`,
`cargo check --workspace --all-targets --offline`,
`cargo check -p od-x86-runner --all-targets --target x86_64-unknown-linux-gnu
--offline`, `graphify update .`, and `git diff --check` passed. The Linux-only
native-denial test was compile-checked, not executed on this Darwin host.
Physical SSE2 validation remains `not_run`; independent Sol review completed
with no remaining findings. Parent S2-35, S2-34, and global `S2-35 → S2-34`
remain open; combined Mac mini/SSH
acceptance remains deferred.
See the [report](../reports/STAGE_2_REPORT.md#s2-3550--local-pandn-register-profile),
[CPU contract](../specs/CPU_CONTRACT.md#local-register-only-pandn-profile--s2-3550),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-3550-local-register-pandn-diagnostic-profile).

#### S2-35.51 — local-only bounded register `POR`

Add `s2-long-por-reg-v1` for exact long-mode `66 [one optional REX.R/B]
0F EB /r`, with ModR/M.mod=3. ModR/M.reg selects the XMM destination and
ModR/M.r/m selects the XMM source; REX.R and REX.B independently extend each
index through XMM15. Snapshot both source and destination before writing
`DEST = old_DEST | old_SRC` across all 128 raw bits. Aliasing is supported.
Preserve flags, MXCSR, x87, GPRs, and every non-destination XMM register; do
not access data memory.

This local diagnostic profile excludes bare MMX, memory, REX.W/X, other or
duplicate prefixes, VEX/EVEX, other opcodes/forms, modeled SSE2/CR0/CR4 gates
or exception delivery, JIT/native execution, and physical execution. Its
assumed preconditions are SSE2 availability, CR0.EM=0, CR0.TS=0, and
CR4.OSFXSR=1. Verification covers all XMM register pairs and independent REX
axes, all truth rows at all bit positions (including `1 | 1 = 1`), masks in
both halves, aliasing/full-state preservation/no callbacks, atomic missing
state and incomplete/unstarted execution, and strict encoding/profile
rejection. A handwritten schema-v2 high-XMM fixture executes in the
interpreter and remains denied by JIT/native admission. Parent/global
acceptance remains open.
