# Этап 2. Эталонный интерпретатор x86-64 — пошаговый план реализации

**Дата:** 23 октября 2026 года. **Статус:** выполняется; S2-00–14 приняты в указанных границах. Baseline integer forms строки S2-15 в CPU contract локально покрыты срезами S2-15a–h и register-register XCHG из S2-10. S2-16a–e локально покрывают direct relative Jcc/JMP, SETcc, CMOVcc и ограниченные register/memory-target near indirect JMP; S2-17a–t покрывают ограниченные GPR и memory PUSH/POP, near CALL/RET, а также выбранные PUSHF/POPF forms — см. [отчёт](../reports/STAGE_2_REPORT.md). S2-15 onward through S2-21g remains open until the shared physical Mac mini acceptance substage. S2-22a–j add local-only FS/GS-base, selector, descriptor-load, VERW/GDT and exact GS/FS.base composition cases; S2-22 itself remains open. Existing deferred-pool accounting remains 614 physically selected fixtures; the 11 S2-22h load fixtures and two S2-22i/j composition fixtures are local-only and are not native-eligible. Across the string-operation slices, native admission includes 21 mapped cases and rejects 10 local-only fault cases; all S2-20a/S2-20c divide-error fixtures and the S2-20b UD2 fixture are excluded from native acceptance selection and execution. Detailed accounting of the initial 558 and subsequent additions is retained in the Stage 2 report. S2-17c/d/e combined-fault priority for a noncanonical CALL target with invalid stack remains unresolved in ISSUE-013; no dual-fault fixture is included. Combined RSP commit policy for memory-destination POP store fault remains open in ISSUE-014. FS/GS forms outside the exact local-only profiles remain unsupported. S2-21a–g cover the documented bounded forms; other widths, LOCK forms and fences remain open in S2-21. F3-prefixed LZCNT/TZCNT remain outside baseline. Stage 2 remains open under broader roadmap criteria. ISSUE-011/012/013/014/015/016/017/018 remain open; acceptance criteria are unchanged.

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
| [ ] | S2-18 | Строковые операции без REP | S2-17 |
| [ ] | S2-19 | REP и точный частичный прогресс | S2-18 |
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
| [x] | S2-30w | Bare `DD D8+i` FSTP ST(i), canonical Normal source copy then physical TOP pop; native closed | S2-30v |
| [ ] | S2-31 | x87 conversions, rounding и exceptions | S2-30 |
| [ ] | S2-32 | Остальные обязательные x87 формы | S2-31 |
| [ ] | S2-33 | MMX и совместное состояние с x87 | S2-32 |
| [ ] | S2-34 | SSE state, moves и logic | S2-20 |
| [ ] | S2-35 | Packed integer SSE/SSE2 | S2-34 |
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

**Статус:** выполнено для диагностического A; дополнительные решения C/D и kernel image явно остаются открытыми. См. [отчёт](../reports/STAGE_2_REPORT.md#s2-00--профиль).

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

### S2-31. x87 conversions, rounding и exceptions

1. Добавить integer/float conversions, precision/rounding control и нужные control/status operations.
2. Реализовать masked/unmasked exceptions, pending/deferred delivery и WAIT/no-wait различия по источнику, без host signal shortcut.
3. Проверить invalid, denormal operand, divide-by-zero, overflow, underflow, precision, signed zero, NaN и крайние integer conversions.

**Готово:** value, status, stack и exception state совпадают в оговорённом профиле; неизвестные поля не скрыты общей маской.

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

1. Добавить XMM state нужного режима и MXCSR с supported/reserved bits. Snapshot фиксирует raw bits всех заявленных lanes.
2. Добавить moves и logical forms по небольшим группам; различать aligned/unaligned, scalar/packed и сохранение верхних bits для каждой encoding.
3. Проверить alignment faults, memory widths и режимные/control-register gates там, где они нужны.

**Готово:** реальные memory/register cases проходят native oracle; host SIMD type не определяет архитектурный layout.

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
