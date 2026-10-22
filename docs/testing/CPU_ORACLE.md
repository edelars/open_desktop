# CPU oracle: первый профиль проверки

**Статус:** этап 1 принят для ограниченного register-only профиля S0-09. Ручной набор из 10 fixtures и фиксированный корпус из 1000 сгенерированных cases прошли по два повтора на Mac mini и M3 Max. Полные границы подтверждения перечислены в [отчёте приёмки Stage 1](../reports/STAGE_1_REPORT.md).

## Цель и границы

Этот документ задаёт начальный Linux x86-64 user-mode профиль для сравнения уже реализованного подмножества S0-09 с исполнением на физическом x86 CPU. Oracle исполняет те же байты программы и начальные значения состояния, что и локальные interpreter/JIT. Oracle не вычисляет ожидаемый результат через Open Desktop и не запускает чужой эмулятор.

Профиль не является обещанием поддержки x86-64 ОС или приложений. В нём нет guest OS, firmware, виртуальной платы, privileged mode, системных вызовов из тестового кода, data memory, MMU, исключений гостевой машины, SIMD, x87, CPUID/MSR, SMP или инструкций, отсутствующих в таблице ниже. Системные средства Linux могут запускать дочерний процесс и наблюдать его завершение, но не добавляют гостевую CPU-семантику.

## Допустимые входные формы

Байтовые формы и их эффекты нормативно заданы в [CPU contract v0.1](../specs/CPU_CONTRACT.md), разделы «Предлагаемый профиль тестового прототипа S0-09» и «Формы инструкций». Первый набор oracle принимает только:

| Инструкция | Разрешённая форма профиля |
|---|---|
| `MOV r64, imm64` | `REX.W + B8+rd + imm64`, назначения RAX/RCX/RDX/RBX |
| `ADD r64, r64` | `REX.W + 03 /r`, ModR/M задаёт только регистры RAX/RCX/RDX/RBX |
| `SUB r64, r64` | `REX.W + 2B /r`, ModR/M задаёт только регистры RAX/RCX/RDX/RBX |
| `CMP r64, r64` | `REX.W + 3B /r`, ModR/M задаёт только регистры RAX/RCX/RDX/RBX |
| `JZ rel8` | `74 cb`, signed смещение от конца инструкции |
| `JNZ rel8` | `75 cb`, signed смещение от конца инструкции |

Другие префиксы и opcode, memory operands, дополнительные GPR и переходы вне границ fixture отклоняются как неподдерживаемый или некорректный вход. Успешное native исполнение более широкой формы не расширяет профиль автоматически.

Начальное и конечное состояние первого профиля: RAX, RCX, RDX, RBX, RIP как byte offset в code buffer, и моделируемые биты RFLAGS. Для `ADD`, `SUB`, `CMP` сравниваются CF, PF, AF, ZF, SF и OF; `JZ`/`JNZ` читают только ZF. Неопределённые биты и биты вне CPU contract не сравниваются. Выходом является достижение конца кода либо исчерпание заданного instruction budget; timeout, сигнал, wrapper fault и неподдерживаемая форма — отдельные исходы harness, не guest exceptions.

## Требования к физическому исполнителю

- Исполнение проходит на подтверждённой машине Linux x86-64, непосредственно на CPU. Эмуляторы и результаты локального interpreter/JIT не засчитываются как oracle.
- Для каждого run сохраняются точные байты и digest fixture, начальное состояние, профиль, seed при наличии, revision/build, machine-readable snapshot и процессный exit status.
- Внешний wall-clock timeout и сбор сигналов изолируют аварийные входы. Timeout, сигнал, ошибка wrapper или потеря результата никогда не преобразуются в успешный CPU result.
- Runner проверяет вход на ограниченный allowlist до передачи исполняемых байтов нативному пути. Этот decoder не становится источником ожидаемой семантики: значения регистров и определённых flags наблюдаются из native execution.
- На этапе первого профиля все разрешённые входы должны быть прямыми, небольшими fixtures или воспроизводиться фиксированным seed. Нативная инструкция не должна читать за пределами выделенной code region или менять состояние процесса за пределами выделенных входных/выходных данных.
- CPU family/model/stepping, microcode (если доступен), feature bits, дистрибутив, ядро, toolchain и page size записываются до приёмочного прогона. Один Intel CPU подтверждает только эту конфигурацию; поведение AMD и других моделей не выводится из него.

## Источники семантики

- Проектный контракт и выбранные байтовые формы: [CPU contract v0.1](../specs/CPU_CONTRACT.md), разделы о формах инструкций, flags и ошибках.
- Intel, [Intel 64 and IA-32 Architectures Software Developer Manuals](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html), версия 093, указанная в CPU contract. Для каждой формы сверять instruction reference и режим исполнения.
- AMD, [AMD64 Architecture Programmer’s Manual, Volumes 1–5](https://docs.amd.com/v/u/en-US/40332_4.09_APM_PUB), публикация 4.09. Отдельно учитывать возможные различия Intel/AMD, не обобщать профиль по одной машине.

## Инвентаризация физического профиля

22 октября 2026 года выполнена read-only инвентаризация единственного обозначенного владельцем x86 host — Mac mini с Ubuntu по адресу `192.168.1.14`, пользователь `senya`. Соединение прошло с обычной проверкой host key после того, как запись ключа была добавлена в пользовательскую `known_hosts` из локальной консоли.

Наблюдаемые параметры: Ubuntu 25.10 (Questing Quokka), kernel `6.17.0-41-generic`, `x86_64`, Intel Core i5-3210M, family 6/model 58/stepping 9, microcode `0x21`, page size 4096 bytes. `lscpu` сообщает 4 логических CPU и флаг `lm`; профиль S0-09 не требует AVX. Доступны GCC/`cc` 15.2.0 с target `x86_64-linux-gnu`, GNU Make 4.4.1, Git 2.51.0, Python 3.13.7. `clang`, `rustc` и `cargo` не установлены. В проверенных путях домашней директории нет checkout Open Desktop.

Rust stable 1.99.1 установлен в пользовательский каталог по разрешению владельца; текущий checkout скопирован в отдельный `~/projects/open_desktop-stage1`. Linux workspace check, тесты целевых crates, Linux `setrlimit` smoke и сборка `od-x86-runner` прошли. Десять fixture snapshots дважды получены с физического CPU и совпали с interpreter и JIT на M3 Max по заявленной области сравнения. Полные данные этой проверки находятся в `work/stage1-oracle/stage1-20261022-190848-eb81896a/`. Это подтверждает только перечисленные register-only формы на одном Intel CPU; не подтверждает весь S0 ISA.

## Автоматическая проверка S1-06

Команда `python3 tests/cpu/stage1_oracle.py` выполняет обязательный oracle: сверяет fingerprint источников двух checkout, собирает runners с одинаковой меткой revision, загружает только выбранные JSON fixtures, дважды запускает interpreter/JIT/native, проверяет каждый result и сравнивает снимки через `od-runner --compare-results`. По умолчанию выбранные 10 fixtures исполняются дважды. Параметры `--host`, `--user`, `--remote-root` и `--repeats` задают стенд; недоступность SSH, ошибка сборки, несовпадение источников или отсутствие JSON result завершают запуск ошибкой.

До SSH входы проверяются на безопасные уникальные ID и basename, затем точные JSON bytes сохраняются под `fixtures/` в каталоге запуска. Именно эти копии передаются всем engines. Manifest записывает пути, SHA-256, seed и `reproduce_argv`. При ошибке native batch сохраняются исходный ответ SSH, JSON/stdout, stderr и exit code каждого процесса; ненулевой exit остаётся ошибкой всего запуска. Для локальных engines также сохраняются stderr и process exit code. Таким образом failed run оставляет данные для диагностики и повторения, а редактирование исходной fixture не меняет уже начавшийся run.

Проверочный запуск `stage1-20261022-190848-eb81896a` завершился `passed`: 10/10 cases × 2 повтора, ноль расхождений interpreter/native, JIT/native и между повторами. Результаты, сравнения, сведения о машинах и командах сохранены в локальном manifest и соседних JSON-файлах. Физически подтверждён один Intel Core i5-3210M; результат не обобщается на другие x86 модели или полную ISA.

## Детерминированная генерация

`od-testkit::generate::generate_fixture(seed, sequence)` использует версию `od-testkit-cpu-v1` и SplitMix64. Одинаковые `seed` и `sequence` дают те же JSON bytes, digest, байты программы и начальное состояние. Генератор включает 0, 1, all-ones, signed boundaries и псевдослучайные значения; строит только допустимые MOV/ADD/SUB/CMP и forward JZ/JNZ с целью на границе конца кода. Циклы не генерируются. `od-runner --generate-corpus SEED COUNT OUTPUT_DIR` сохраняет каждый fixture отдельным JSON; максимальный COUNT ограничен 100000.

Финальный физический запуск `stage1-20261022-193143-3244fc20` использовал seed `0x5eedcafe`, 1000 case IDs и 2 повтора (2000 native results). Все interpreter/native, JIT/native и повторные сравнения прошли без различий. Совокупный SHA-256 сериализованных fixture bytes: `9356e916b2037a3f5a082e8366fe33bf4277ab00812489809bfc586210744d35`. Корпус сохранён в `work/stage1-harness-generated-stage1-harness-20261022-193139-96d1916a/`, результаты — в `work/stage1-oracle/stage1-20261022-193143-3244fc20/`. Это один воспроизводимый corpus текущих форм, а не полное покрытие ISA.

## Границы fetch и наблюдаемость

На Linux x86-64 Mac mini выполнены guard-page tests для decoder: доступная область заканчивалась усечённым `REX` или двумя байтами `REX.W + MOV opcode`; следующая страница была `PROT_NONE`. Оба случая дали `DecodeError::Truncated` без чтения за границей и без host signal. Эти проверки подтверждают контракт декодера над slice. Они не реализуют guest virtual memory и не заявляют architectural page fault handling.

## Матрица возможностей памяти и исключений

| Возможность | Стенд сейчас | Что это подтверждает |
|---|---|---|
| GPR/RIP/RFLAGS register-only execution | Поддерживается для таблицы S0-09 выше | Снимки физических Intel execution сопоставимы с interpreter/JIT |
| Fixture memory regions и byte masks | Сравнение synthetic snapshots тестируется; S0-09 fixture ставит `unsupported` | Comparator использует заявленные region ID/address/mask, но CPU runner не исполняет memory operands |
| Exception metadata в comparator | Сравнение synthetic snapshots тестируется; S0-09 fixture ставит `unsupported` | Сравниваются доступные тип/vector/error-code/RIP/address поля, не утверждается их guest availability |
| Linux user-mode SIGSEGV/SIGILL observer | Test-only child сохраняет raw `si_code`, `ucontext` RIP и доступный `si_addr`, отдельно нормализует OS event | Проверен сбор host diagnostic metadata; это не guest exception semantics |
| Guest virtual memory, cross-page load/store, permissions | Не поддерживается | Требует guest address translation, memory model и точности faulting instruction |
| Guest architectural exception vectors / privileged faults | Не поддерживается | Нельзя выводить vector/error code из одного host signal |
| Device callback memory observations | Не поддерживается; требования для будущего adapter описаны в `docs/testing/CONTROL_PROTOCOL_TESTING.md` | Контракт подключается, когда появится реальная модель устройства |

Для следующего этапа с памятью понадобятся адресные формы ModR/M/SIB/displacement, fixture-owned guest memory image, доступ до/после fault, нормализованный guest faulting RIP и сопоставление страниц; будущие device tests дополнительно потребуют callbacks с явным guest address-space/context и ограниченными наблюдаемыми side effects. Не объявлять такую поддержку по результату Linux host signal smoke.

## Наборы запуска и MCP

`python3 tests/cpu/stage1_harness.py --suite local` выполняет локальные fmt/workspace check/целевые tests/fixture digests. `--suite oracle` запускает обязательный физический профиль и ошибается, если SSH, build или сравнение недоступны. `--suite vmhost` запускает реальные VMHost process/socket scenarios; `--suite all` выбирает все три. Невыбранные наборы остаются `not_run` в summary. `--generated-count N --seed 0x...` добавляет воспроизводимый CPU corpus в oracle suite. Summary и полные step logs сохраняются под `work/stage1-harness/<run-id>/`.

После [ревью этапа 1](../reports/STAGE_1_REVIEW_2026-10-22.md) local suite также исполняет `tests/cpu/test_stage1_harness.py`, а generated corpus всегда добавляется к 10 ручным fixtures. Повторная команда `--suite all --generated-count 1000 --seed 0x5eedcafe --repeats 2` прошла 1010 cases × 2 повтора на обеих машинах без расхождений; актуальные ссылки и fingerprint находятся в [review evidence](../reports/data/stage1/review-2026-10-22.json).

MCP отложен: CLI с JSON и Python orchestration уже поддерживают fixture execution, comparison, seeded batches, manifests и process scenarios. Пересмотр нужен только при конкретном interactive IDE use case или внешнем workflow, которому нужен структурированный вызов без CLI.

Порядок подключения следующей группы инструкций описан в [ADDING_ISA_GROUP.md](ADDING_ISA_GROUP.md); проверенный пример ADD/SUB/CMP остаётся ограничен одним Intel host и текущими register-only forms.
