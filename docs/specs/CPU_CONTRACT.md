# CPU contract v0.1 — ограниченный профиль этапа 0

**Дата:** 22 октября 2026 года. **Статус:** контракт минимального прототипа S0-09 принят для сравнения interpreter/JIT; целевой CPUID/ISA-профиль для Windows и Linux не утверждён и не выводится из этого документа.

Этот документ разделяет будущую системную модель CPU и минимальную семантику вертикального среза. Прототип не эмулирует загрузочный CPU, ОС, привилегии, MMU, SIMD или полный x86-64. Все детали кодировок и вычисления флагов должны соответствовать Intel/AMD manuals из раздела источников.

## Архитектурный контракт целевого движка

| Область | Контракт / статус |
|---|---|
| Идентичность архитектуры | Гостевая архитектура — x86-64; host ISA — ARM64. Представление гостевого состояния не зависит от host-регистров и ABI. |
| Состояние CPU | В полном `CpuState` потребуются RIP, GPR, RFLAGS, режим, сегменты, control registers, MSR, векторное состояние и pending events. Поля появляются вместе с поддерживаемыми режимами; прототип ниже использует только подмножество. |
| CPUID и MSR | Нужны для будущих ОС/ПО, но конкретные листья, биты, значения и MSR не выбраны. Нельзя объявлять возможности, не реализованные и не проверенные. |
| Режимы и привилегии | Для Windows boot потребуется определить long mode, CPL, сегменты, control registers, paging и переходы firmware/loader. Точный минимум ожидает анализа выбранного ISO и S0-05. |
| Адреса | В целевой модели различать guest virtual/linear, guest physical и host address. Translation и MMIO принадлежат machine/MMU boundaries. Их ширины и canonical-address правила для профиля ОС не зафиксированы. |
| Исключения/events | Каждый реализованный instruction form должен задавать decode faults, architectural faults и состояние на exit. Отображение в guest exception нельзя заменять host panic или общим перехватом. Приоритеты для незаданного набора инструкций не утверждаются этим черновиком. |
| Память и атомарность | SMP/атомарные операции, fences и memory ordering вне прототипа. Для будущего многопоточного профиля нужен отдельный контракт и проверки соответствующей модели памяти x86. |
| JIT state | На каждом наблюдаемом exit должны материализоваться определённые архитектурные регистры/флаги и guest instruction offset. Восстановление после исключений и асинхронных событий остаётся отдельной задачей. |

Никакой CPU capability profile для запуска Windows/Linux пока не выбран. Таблицы Windows minimums и reference VM в [GUEST_REQUIREMENTS.md](GUEST_REQUIREMENTS.md) не подменяют исследование конкретного install media, CPU feature checks и драйверов.

## Предлагаемый профиль тестового прототипа S0-09

Прототип — изолированный пользовательский fragment runner с единым vCPU, только immutable code buffer и register-only вычислениями. Это набор для сравнения decoder → interpreter/JIT, а не профиль x86-64 машины.

### Состояние и запуск

- Режим: 64-bit instruction mode; никаких переходов режима, CPL transitions, сегментных обращений или privilege checks.
- Состояние: `RAX`, `RCX`, `RDX`, `RBX`, `RIP` и подмножество `RFLAGS`. Host-регистры не считаются архитектурным состоянием.
- Начальные значения GPR и `RIP` задаются входным manifest программы; `RIP` — byte offset внутри code buffer. Начальные арифметические flags очищены, reserved bit 1 RFLAGS установлен.
- Операции используют 64-bit width. Конкретные разрешённые prefix/opcode/ModR/M формы перечисляются в таблице ниже; прочие формы не поддерживаются.
- Исполнение останавливается при достижении `RIP == code_length`, исчерпании instruction budget или явной ошибке. Итог включает перечисленные регистры и определённые flags.

### Формы инструкций

| Инструкция | Минимальная форма S0-09 | Архитектурный эффект |
|---|---|---|
| `MOV r64, imm64` | `REX.W + B8+rd + imm64`, назначения только RAX/RCX/RDX/RBX | Записывает 64-bit immediate в назначенный GPR; flags не меняет. |
| `ADD r64, r/m64` | `REX.W + 03 /r`; оба операнда — один из четырёх разрешённых GPR, без памяти | Складывает source с destination; устанавливает CF/PF/AF/ZF/SF/OF по x86-64 semantics. |
| `SUB r64, r/m64` | `REX.W + 2B /r`; register-register | Вычитает source из destination; устанавливает CF/PF/AF/ZF/SF/OF. |
| `CMP r64, r/m64` | `REX.W + 3B /r`; register-register | Вычисляет destination − source, не записывает результат; устанавливает CF/PF/AF/ZF/SF/OF. |
| `JZ rel8` | `74 cb` | Если ZF=1, переход на signed rel8 от конца инструкции; иначе продолжение. |
| `JNZ rel8` | `75 cb` | Если ZF=0, переход на signed rel8 от конца инструкции; иначе продолжение. |

Условный переход может вести только на начало декодированной инструкции внутри code buffer либо ровно на `code_length` (остановка). Переход в середину инструкции, за границу, назад к недекодируемому смещению и за budget завершается явным результатом runner. Нет `CALL/RET`, стека, памяти данных, системных инструкций, CPUID/MSR, x87/SSE/AVX, interrupt delivery или MMU.

Декодер читает байты через ограниченный интерфейс; один запрос не читает за концом buffer. Максимальная архитектурная длина x86 инструкции — 15 bytes; усечённая и неразрешённая форма возвращает отдельную ошибку без частичного исполнения. Перед реализацией зафиксировать детальную таблицу принимаемых байтовых форм/REX/ModR/M, чтобы decoder не принимал более широкие формы случайно.

### Flag contract

Для `ADD`, `SUB` и `CMP` результат флагов CF/PF/AF/ZF/SF/OF определяется соответствующими x86 operation semantics. `JZ/JNZ` читают только ZF. На каждом сравнении interpreter/JIT проверяются все flags, определённые данной инструкцией. Флаги или состояния, которые архитектура определяет как undefined, не используются тестами и не считаются обязательным совпадением. Другие RFLAGS биты не моделируются и в CPUID не рекламируются.

### Ошибки и корректность

- Неизвестные, неподдерживаемые и усечённые формы дают различимые decode errors; они не исполняются приблизительно.
- Некорректная цель ветвления, instruction budget exhaustion и отсутствие доступного байта — различимые причины остановки.
- Host panic, memory safety failure и JIT code fault — дефекты реализации, не guest exception.
- Interpreter и JIT используют одинаковый набор входных байтов и сравниваются по exit reason, guest RIP, GPR и flags, определённым этой версией контракта.
- Собственные hex-fixtures для этапа 0 находятся в [`tests/fixtures/x86/`](../../tests/fixtures/x86/README.md): арифметика с переполнением, зависимый `SUB/JNZ` цикл, `CMP/JZ`, загрузка immediate и неподдерживаемый opcode. Команды сравнивают interpreter и JIT; источником семантики остаётся этот контракт и Intel/AMD manuals.

## Не включено и будущие решения

| Возможность | Статус | Что нужно до объявления |
|---|---|---|
| Windows/Linux CPU profile | Не определён | Exact media/build, сбор boot requirements, обязательные CPUID/MSR/ISA и драйверные проверки |
| x86-64 application baseline | Не определён | Результаты выбранных приложений и минимальные ABI/runtime requirements |
| 32-bit compatibility | Не включена в S0-09 | Guest mode transitions, segmentation, syscall/exception behavior и тестовый набор |
| x87/SSE/AVX | Не включены в S0-09 | State layout, exception/masking semantics, feature gating и отдельная проверка каждого расширения |
| Paging/MMU и TLB | Не включены в S0-09 | GVA→GPA translation, permissions, page faults, address-width profile и invalidation contract |
| SMP, atomics, fences | Не включены в S0-09 | x86 memory ordering, locked operations, interrupt/scheduler model и stress tests |
| Privileged/system instructions, APIC/timers, exceptions | Не включены в S0-09 | Выбранный board/boot profile, точная архитектурная семантика и приоритеты событий |

## Источники архитектурной семантики

- Intel, [Intel 64 and IA-32 Architectures Software Developer Manuals](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html), версия 093 на дату документа: базовая архитектура и instruction reference. Для реализации сверять конкретную инструкцию и выбранный режим по manual.
- AMD, [AMD64 Architecture Programmer’s Manual, Volumes 1–5](https://docs.amd.com/v/u/en-US/40332_4.09_APM_PUB), публикация 4.09: независимое описание AMD64 semantics; расхождения Intel/AMD нужно фиксировать до расширения общего профиля.
- [План S0-09](../plans/STAGE_0.md#s0-09-реализовать-ограниченный-сквозной-x86--arm64-прототип) задаёт границу вертикального среза и требования к сравнению interpreter/JIT.
