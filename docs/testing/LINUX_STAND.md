# Методика стенда Linux (этап 5)

Статус: **Method gate принят как отдельный subgate после независимого ревью (2026-11-12); Numeric calibration gate blocked; полный S5-01 blocked.** Срез задаёт формат и критерии будущих доказательств, но не сообщает о принятой загрузке или гостевом сценарии. Число повторов S5-15 и длительность S5-16 не назначены; см. [ISSUE-076](../KNOWN_ISSUES.md#issue-076-s5-01-run-duration-inputs-missing).

Связанные документы: [план этапа 5](../plans/STAGE_5.md), [методика машины](MACHINE.md), [инвентаризация стенда](STAND.md), [базовая методика измерений](BENCHMARKS.md), [отчёт этапа 5](../reports/STAGE_5_REPORT.md).

## 1. Область и стенд

Эта методика описывает сбор evidence для Linux guest workloads этапа 5: уровни unit/contract/scenario/repeatability/soak/measurement, формат одного прогона, правила сравнения результатов и границы временных оценок. Она не утверждает, что сценарий выполнен, и не переносит проверки с одного host или build на другой.

### Два gates S5-01 и порядок работ

**Method gate** содержит проверяемую evidence schema, pass/fail правила, workload/profile определения, критерии расхождений, формулы и порядок калибровки. Он принят после независимого ревью как отдельный subgate; это разрешает использовать методику для подготовки и функциональных проверок только при выполненных независимых prerequisites. Одобрение не меняет checkbox или overall status S5-01.

Принятый Method gate фиксирует, что: шесть уровней evidence имеют однозначные pass criteria; один run record задаёт все идентификаторы, конфигурацию, markers, observations, hashes, stop и артефакты; каждый workload имеет именованный профиль/входы; измерения `I_boot`, `W_boot`, `I_cycle`, `W_cycle` имеют определённые границы начала/конца и host/build fingerprint; формулы используют budgets, объявленные до калибровки; отсутствующее поле или prerequisite даёт `blocked`/`not_run`, а не pass. Этого достаточно для отдельных функциональных проверок и подготовки, но не для числового acceptance.

**Numeric calibration gate** остаётся blocked до появления всех фактических входов: `I_boot`/`W_boot` из уже принятого S5-08 calibration boot path; `I_cycle`/`W_cycle` из уже принятого повторяемого C+D+G cycle после S5-10/11/14; и заранее заданных, обоснованных `B_series`/`B_soak`. Gate вычисляет `N`/`D_soak_estimate`; только его принятие вместе с Method gate позволяет принять полный S5-01.

S5-06/07 могут готовить guest сценарии и evidence orchestration по Method gate. Одиночный функциональный сценарий можно выполнить по той же методике только после собственных prerequisites: S5-08 принят на этапе функциональной проверки с ограничениями S4-39/S4-31; S5-09 — до принятия S7-16, PCI binding и IRQ route; C/D/G — до принятия S5-09 и относящихся к ним ISA/profile gates S5-21. Numeric gate не требуется для этих functional checks. Ограниченные повторы, требуемые собственными критериями задач (например, второй boot S5-08, повторная сборка S5-11 или явно заданный S5-07 smoke count), могут закрывать критерии именно этих задач, если полностью проверены. Они не задают приёмочный `N` S5-15 или soak target S5-16. S5-15 и S5-16 начинаются только после принятия Numeric gate и полного S5-01; их итоговые серии не используются для ретроактивного вычисления собственных бюджетов.

По [инвентаризации стенда](STAND.md#основной-host) основной host — MacBook Pro с Apple M3 Max под macOS 26.7.1, arm64. Физический x86 oracle — отдельный Mac mini с Intel Core i5-3210M под Ubuntu 25.10. Это разные машины, ОС и цели проверки: результаты нельзя объединять в одну временную baseline. Каждая запись evidence фиксирует реальный host, ОС, архитектуру CPU, сборку runner, исходную ревизию и параметры guest machine.

До готовности полного Linux userspace `S4-39` guest-scenario уровень остаётся заблокированным. Измерение ISA-разрыва `S5-21` отдельно определяет, какие workloads могут быть запущены. Предварительный runner load/diagnostic stop не является успешной загрузкой или завершением гостевого сценария.

Сырые runner JSON, UART logs и гостевые artifacts хранятся в игнорируемом `work/stage5/<run_id>/`; в Git попадают только компактный отчёт и необходимые малые manifests. Evidence не должен содержать секреты, серийные номера, учётные данные или пользовательские файлы.

## 2. Шесть уровней доказательств

| Уровень | Что проверяется | Условие pass |
|---|---|---|
| Unit | Локальные unit tests `od-host`, `od-storage`, `od-machine` и их отрицательные ветви | Все применимые tests проходят; ожидаемые отказы проверяются конкретными значениями и отсутствием запрещённых побочных эффектов. Каркас или тест отсутствующей функции не считается pass. |
| Контрактные проверки диска | Host backend и профиль NVMe без гостя: write/readback, write-through/durability contract, close/reopen, повторный read, FaultBackend и исчерпание места | Каждый вызов имеет ожидаемый completion/error; повторное открытие возвращает точные bytes; failure не превращён в успешный ack. Эти проверки сами по себе не доказывают Linux driver или guest filesystem. |
| Один гостевой сценарий | Один run ID и nonce, ожидаемые guest markers, machine observations, UART transcript и хеши файлов | Гость запущен, точная последовательность markers и сценарные assertions совпали, input/guest/host hashes проверены; `linux-direct.status = loaded` отдельно не считается гостевым pass. |
| Повторяемость | Один и тот же сценарий и конфигурация в N отдельных прогонах; nonce новый в каждом прогоне | Серия завершена с заранее выведенным N; все semantic fields совпали или каждое различие заранее классифицировано и объяснено. Неудачные прогоны не усредняются и не отбрасываются. Пока N заблокировано; формула в §5. |
| Длительный прогон | Смешанный цикл сценариев C, D и G с контролем данных и ресурсов процесса | Прогон достигает заранее обоснованной длительности; hashes проверены до, во время и после; каждый рост памяти/дескрипторов измерен и укладывается в заранее заданный предел либо записан как дефект. Сейчас цикл и целевая длительность не измерены. |
| Измерения | Скорость/задержка на явно описанном workload и одной host/build конфигурации | Зафиксированы interpreter/JIT profile, cache mode, workload и host/build. Отдельно записаны disjoint числа retired instructions для interpreter, JIT и fallback; доля fallback вычислена относительно общего числа либо отмечена `not_applicable` с причиной. Их сумма согласуется с общим числом по контракту счётчиков; повторы и user-CPU/wall time приведены раздельно. Пропущенные счётчики дают `blocked`, а не успешный замер. Само измерение не означает ускорение или поддержку. |

Для каждой операции применимы одновременно уровни, относящиеся к её границам. Например, успешный гостевой сценарий с диском требует предварительно проверенного host disk contract, но положительный unit test не доказывает guest discovery или filesystem behavior.

## 3. Запись одного прогона

Одна запись связывает run ID, входы, фактически запущенную конфигурацию и результат. `run_id` уникален для одного запуска; `series_id` при наличии связывает несколько таких записей серии. `nonce` уникален для гостевого прогона и должен быть виден в guest marker. Для `failed`, `not_run` и `blocked` поле `reason` обязательно.

| Поле записи | Источник и требование |
|---|---|
| `run_id`, `series_id`, `scenario_id`, `nonce` | Orchestrator/evidence manifest. `nonce` точно соответствует `--nonce` и проверяется в гостевом выводе. Не помещать секреты. |
| `host` | Host name/model, ОС/version, архитектура, CPU, RAM/page size если доступны, Rust/toolchain, runner build/profile, git revision и dirty state. Не переносить данные M3 Max на Mac mini или наоборот. |
| `configuration` | Профиль board, RAM MiB, command line, ACPI manifest hash и остальные явно используемые параметры сценария. Сохранять фактические значения из `linux-direct`, а не только ожидаемые. |
| `artifacts` | Path и SHA-256 kernel, initramfs, ACPI blob/manifest, guest script/rootfs и disk image до запуска, где применимо. Runner JSON сохраняется без редактирования и хешируется отдельно. |
| `runner` | Команда и аргументы, код завершения, путь и SHA-256 JSON от `--out`; ключевые наблюдения: `status`, `guest_started`, `retired`, `entry`, `stop.exit_kind`, `stop.rip`, registers, `stop.com1_hex/text`, а также `inputs`, `memory_sha256`, `cmdline_in_memory_matches`, `acpi` и `e820`. У runner нет отдельных полей для run ID, guest markers, полного host inventory или wall/user-CPU elapsed; они записываются в manifest вокруг сырого JSON. |
| `execution_measurement` | Interpreter/JIT/mixed profile и конфигурация cache. Для измерительного сценария: общее число retired instructions и раздельные, неперекрывающиеся `interpreter_retired`, `jit_retired`, `fallback_retired`; `fallback_share = fallback_retired / total_retired` либо `not_applicable` с причиной, когда fallback невозможен или не используется. Записать доступные elapsed значения отдельно. Если профиль или счётчики не наблюдаются, outcome не может быть `passed` для measurement level. |
| `guest_markers` | Упорядоченные строки гостя с run ID/nonce и ожидаемыми scenario markers; приложить путь и SHA-256 исходного UART transcript. Runner COM1 bytes в JSON не заменяют полный transcript. |
| `guest_files` / `host_verification` | Для каждого проверяемого файла: guest path, byte length, guest SHA-256, способ получения тех же bytes host-стороной, host byte length/SHA-256 и `matches`. Если host не может получить тот же byte stream, результат — `blocked`, а не inferred pass. |
| `timing` | Отдельно user-CPU seconds и wall elapsed, способ измерения, start/end и build. Текущий `linux-direct` JSON не записывает эти elapsed values. |
| `outcome` | Ровно `passed`, `failed`, `not_run` или `blocked`; для последних трёх требуется проверяемая причина и ссылка на точное наблюдение/предусловие. Это поле manifest, а не `linux-direct.status`. |

`linux-direct` уже пишет JSON в `--out` и stdout. Успешная загрузка возвращает top-level `status: "loaded"`; ранний отказ — `status: "refused"` с `error.code`, `error.stage`, `error.detail`. Запуск гостя дополнительно отражается в `guest_started` и `retired`; `--run` добавляет `stop`. Входные hashes находятся в `inputs.vmlinuz`, `inputs.initramfs`, `inputs.acpi_blob`; machine-side readback kernel/initramfs — в `memory_sha256`. `ram_mib`, `cmdline`, `acpi.manifest_hash`, `e820`, `entry` и `stop` являются источником machine observations. `status: loaded` означает успешную загрузку образа в board, но не успешное завершение guest workload.

Stage 5 series writer uses schema v2: `host.source_revision` records the Git HEAD at runner build time and `host.source_dirty` records whether the runner's production source inputs were dirty. The checked input paths are declared in `tools/runner/build.rs`, including the runner and its workspace dependencies. This boolean is state metadata, not a source-content fingerprint. The reader accepts v1 and v2; v1 records without `source_dirty` preserve it as unobserved (`null`). A dirty or unavailable source identity does not establish artifact equivalence or a passing guest run.

Подготовленный Stage 5 path принимает два `series.json` через `od-runner --compare-results <series-a> <series-b> --linux-stage5 --json`; существующий fixture path `od-runner --compare-results <result-a> <result-b> --case <fixture> --json` сохранён. Raw JSON `linux-direct` не является непосредственным input ни для Stage 5 comparator, ни для fixture comparator: Stage 5 comparator читает run manifests и сверяет ссылки/хеши исходных JSON и runner-recorded COM1 bytes. Его stop record содержит байты COM1 только из `run_boot` для данного stop; это stop-scoped capture, а не полный независимый UART transcript, который требуется на scenario level. Результат `matches: true` означает лишь совпадение сравниваемых series evidence; сам по себе он не доказывает pass guest scenario или acceptance. Для pass всё ещё нужны scenario-specific expected markers/assertions, корректные machine observations, полный UART transcript и guest/host file evidence по критериям раздела 2.


### Дисковый профиль не требуется для S5-07

Для проверки механизма повторов используется существующий сценарий `stage4-init-nonce`.
Подготовьте pin inputs из [рецепта Stage 4](MACHINE.md#воспроизведение-linux-direct-boot-s4-39j).
Используйте `--nonce-bootconfig` с сохранённым ядром Ubuntu 7.0.0-38.38 и исходным Stage 4 initramfs. Один вызов выполняет два запуска, выдаёт отдельный nonce каждому и сохраняет сравнение:

```sh
target/release/od-runner linux-direct \
  --test-runs 2 --series-id s5-07-diskless-n2 --scenario-id stage4-init-nonce \
  --expected-stop-kind budget_exhausted --out work/stage5/s5-07-diskless-n2/series.json \
  --vmlinuz "$VMLINUX" --initramfs "$INITRAMFS" --acpi-blob "$ACPI_BLOB" \
  --expected-vmlinuz-sha256 9f55ef7253055a9cf68f8a04323c93dead2fca617386d842fbde9ca7b178b27e \
  --expected-initramfs-sha256 ee9183a4eded3c16a8c6a7a81c6d5bb79be8bb023f793f7cd61694ea2bd3e588 \
  --expected-acpi-manifest-hash 2bd690ae0817705b1b128675f9628dd7212fee74ab97c88da3599a1ae8e398f2 \
  --ram-mib 128 --run --budget 3000000000 --set-cpl0 --cpuid-profile stage4a --user-mode \
  --nonce-bootconfig
```

Для ACPI используйте 1,072-байтовый blob с SHA-256 `b1ba6078a4d9bc7a8d224980713d8242e4a708743ce4f56093b00a07c631ffaf` из рецепта Stage 4.
Для следующей серии выберите новый `--series-id` и соответствующий ему `--out`.
Оба запуска должны завершить сценарий, а сохранённый `comparison.json` должен содержать `matches: true` и пустой `unexplained_differences`. Эта серия проверяет S5-07 и используется с профилем `smoke_not_acceptance`.

Перед загрузкой runner проверяет SHA-256 файла initramfs, затем добавляет Linux bootconfig с `kernel.od.nonce` в памяти. Linux передаёт этот параметр в `/proc/cmdline`, который читает Stage 4 `/init`. Ранняя boot command line остаётся одинаковой, что исключает nonce из конкретного источника энтропии `random_init_early`. Исходный файл, профиль CPU и настройки ASLR сохраняются. Per-run raw JSON отдельно фиксирует хеш и длину загруженного initrd и хеш трейлера. Comparator проверяет совпадение исходных artifact hashes, сверяет дублирующие хеши загруженного initrd в `stop.inputs` с проверенным readback и относит их к входным данным. Остальные машинные наблюдения сравниваются напрямую. Допустимое изменение в UART ограничено заменой собственного nonce.

Для повторной сверки сохранённой серии вызовите `od-runner --compare-results <series.json> --linux-stage5 --json`. Он использует тот же comparator, что и завершение `--test-runs`. Вариант команды с двумя путями сравнивает две серии между собой.



### S5-08 загрузка и ACPI-выключение без диска

Соберите вариант Stage 4 `/init` для выключения в отдельный каталог.

```sh
python3 tools/build/build_stage4_initramfs.py --poweroff \
  --output-dir work/stage4/s5-08-poweroff \
  --config work/stage4/linux-7.0.0-38.38/source/extracted/config --no-evidence
cargo build --release --locked --offline -p od-runner
```

Команда использует зафиксированное ядро Stage 4 и 1 072-байтовый ACPI blob с SHA-256 `b1ba6078a4d9bc7a8d224980713d8242e4a708743ce4f56093b00a07c631ffaf`. Его можно извлечь из Git как `git show c09f62d:tests/fixtures/guest/stage4/acpi_blob/stage4_acpi_blob.bin` в новый файл и передать как `$ACPI_BLOB`.

```sh
series=s5-08-stage4a-poweroff-n2
target/release/od-runner linux-direct --test-runs 2 \
  --scenario-id s5-08 --expected-stop-kind stopped --series-id "$series" \
  --out "work/stage5/$series/series.json" \
  --vmlinuz work/stage4/linux-7.0.0-38.38/vmlinuz-7.0.0-38-generic \
  --initramfs work/stage4/s5-08-poweroff/initramfs.cpio --acpi-blob "$ACPI_BLOB" \
  --expected-vmlinuz-sha256 9f55ef7253055a9cf68f8a04323c93dead2fca617386d842fbde9ca7b178b27e \
  --expected-initramfs-sha256 dea0fcb2a0c4401a99f342413567a0a9b4aa5d8929cd0abdaf687e5042567d03 \
  --expected-acpi-manifest-hash 2bd690ae0817705b1b128675f9628dd7212fee74ab97c88da3599a1ae8e398f2 \
  --ram-mib 128 --run --budget 3000000000 --set-cpl0 --cpuid-profile stage4a \
  --user-mode --nonce-bootconfig
python3 tests/system/s5_08_shutdown_smoke.py "work/stage5/$series/series.json"
```

Каждый новый запуск команды требует нового `series`; runner создаёт свежий guest nonce для каждого прогона. `/init` выводит `OD_NONCE=<nonce>` и `OD_S5_A_SHUTDOWN_REQUEST nonce=<nonce> method=poweroff`, затем вызывает Linux `reboot(POWER_OFF)`. Зачёт требует точных полных строк в этом порядке, отсутствия kernel panic и `exit_kind=stopped` с типизированной причиной `machine_stop_reason=shutdown`. Сравнение проверяет UART, машинные наблюдения и хеши исходных артефактов по тем же правилам, что и серия с bootconfig выше. Профиль `stage5_functional` относится к функциональной проверке S5-08. Приёмочный `N` и калибровка S5-01 описаны отдельно.


## 4. Расхождения, pass/fail и целостность

До сравнения фиксируются неизменные входы: artifact hashes, runner/build revision, board profile, RAM, command line, scenario script и профиль CPU. В одной series намеренно различаются `run_id`, `nonce`, пути каталога evidence и host wall-time/resource observations. Если guest marker или создаваемый файл включает nonce, ожидаемое различие объявляется заранее и проверяется внутри соответствующего прогона; его нельзя молча исключать из сравнения.

При одинаковых входах необъяснённое расхождение — любое отличие в status/guest_started, ожидаемом exit/stop, retired count, semantic machine state, guest markers, ожидаемых file bytes/hashes или повторно читаемых durable data, если оно не перечислено как допустимое для данного сценария. Каждому иному отличию нужны причина и evidence; иначе прогон `failed`, а известный дефект записывается в `KNOWN_ISSUES.md`. Нельзя вводить допуск «в пределах разумного».

Повреждение данных подтверждено, если длина/хеш проверяемого файла не совпадает с ожидаемым значением, гостевой и host SHA-256 одного и того же byte stream расходятся, либо последующий ожидаемо clean mount сообщает, что нужна filesystem recovery. До/после теста сравниваются именно одинаковые файлы по их bytes. SHA-256 файла из гостя нельзя сравнивать с SHA-256 всего raw disk image: это разные объекты. Изменение whole-image hash после ожидаемых guest writes само по себе не доказывает порчу или целостность файла.

При `VWC = 0` успешный write проверяется согласно write-through контракту. Остановка процесса и host-side fault injection не называются доказательством потери питания; power-loss claim остаётся отдельной областью этапа 9/S9-39.

## 5. Временные оценки, повторы и длительный прогон

### Измеренный темп и текущий frontier

По [плану этапа 4, записи S4-13](../plans/STAGE_4.md#s4-13-выбрать-простой-программный-tlb-по-замерам) 200,000,000 retired instructions заняли 554 seconds **user CPU** на ускоренном профиле; исходный runner result — [S4-13 200M evidence](../reports/data/stage4/s4-13-fetch-cache-200m-run.json). Владелец подтвердил, что прогон выполнен на «этом PC». Текущая проверка окружения идентифицирует этот PC как Mac15,11/arm64 с macOS 26.7.1 build 25G241. Сохранённые JSON и строка отчёта не фиксируют host/build на момент измерения: текущий build — только наблюдение текущего окружения, и нет evidence, что замер выполнялся именно под 25G241. Rate относится к подтверждённому владельцем этому PC, но её нельзя переносить на физический Intel Mac mini/Ubuntu oracle или приписывать историческому OS build. Воспроизводимая арифметика для отдельной исторической контрольной точки C5:

```text
R = 200,000,000 instructions / 554 user-CPU-s
  = 361,010.8303 retired instructions per user-CPU-second

I_C5_historical = 1,137,474,216 retired instructions
T_C5_projection_at_R = I_C5_historical / R
                     = 3,150.8036 user-CPU-s
                     = 52 min 30.8 s user CPU (about 52 min 31 s)
```

Число C5 — **историческая** baseline checkpoint, не текущий frontier. Stage 4 позднее достиг C7 на настоящем ядре; затем диагностический S4-38L7w прогон (DiagnosticV1, не засчитывается) достиг retired 1,185,015,387 и остановился на `IN AL, DX` ([план S4-38L7w](../plans/STAGE_4.md) и [raw evidence](../reports/data/stage4/s4-38l7w-diagnostic-run.json)). Это diagnostic progress, не подтверждённая загрузка Linux userspace. Отдельный release sample в Stage 4 сообщает около 2.8M instructions/s на 1.1B run; это другая сборка/измерение и его нельзя смешивать с S4-13 user-CPU rate или использовать как замену измерению целевого workload. C5 projection выше — только user-CPU estimate на historical checkpoint, не измеренный wall time и не время завершённой загрузки. Для масштаба уменьшенного diagnostic profile checkpoint 200,000,000 retired instructions соответствует 554 user-CPU-s (9 min 14 s). Он проверяет только bounded diagnostic evidence; **не** удовлетворяет S5-15, не подтверждает полный boot и не является soak для S5-16.

Число одной завершённой загрузки не выводится из historical C5 checkpoint или более позднего diagnostic run. После подтверждения и принятия целевого S5-08 guest path выполнить отдельный calibration run: измерить `I_boot` до заранее заданного гостевого success marker и wall time `W_boot` на зафиксированном host/build. Этот run — калибровочный вход, а не одна из повторных загрузок S5-15. `I_boot / R` даёт лишь user-CPU estimate при сопоставимой скорости; runner обязан отдельно записать фактический wall elapsed.

### Обязательная запись calibration run (Method gate)

Каждая будущая boot/cycle calibration записывает один self-contained record со следующими полями и проверками. Record может быть заполнен до измерения идентификаторами и профилем; числовые результаты остаются отсутствующими до фактического run.

| Группа | Обязательные данные и правило |
|---|---|
| Идентичность и метод | `calibration_id`, версия/revision этой методики, gate (`boot` или `cycle`), имя и hash workload/profile; для cycle — точный порядок C, D, G и scenario IDs. |
| Среда и inputs | Host model/OS/build/architecture/CPU, runner/toolchain revision и dirty state; SHA-256 каждого kernel/initramfs/rootfs/disk/script/fixture artifact, board/memory profile, command line и полный configuration fingerprint. Все fingerprints относятся к тому же run, что и измерения. |
| Границы и время | Именованные `start_event`/`end_event`, монотонные `start_ns`/`end_ns`, clock source и единицы; `W_boot`/`W_cycle` вычисляются из этих границ. Та же самая операция и интервал должны покрывать instruction-counter delta: `I_boot`/`I_cycle` нельзя брать из другого phase/run или смешивать с wall time по иным границам. |
| Boot operation | Указать фазы до/после boot, которые входят в профиль. Границы должны повторять ту же полную boot operation, которую позже проверяет S5-15: от заранее названного operation-start event до принятого S5-08 guest success marker. Любые исключённые preparation/teardown phases перечислить заранее. |
| Mixed C+D+G cycle | Зафиксировать исходное состояние/reset, C→D→G order, input/profile hashes, completion marker каждого component run и ссылки на их run records; указать shutdown/restart/kill boundaries. `I_cycle` — сумма только валидных per-run instruction deltas для объявленного цикла через resets; отсутствующий post-kill counter остаётся unavailable и не восстанавливается догадкой. `W_cycle` покрывает объявленные cycle boundaries, включая указанные restart/shutdown интервалы. |
| Budgets | `B_series` и `B_soak` задать с единицами, rationale, доступным временем стенда и ссылкой на решение/evidence до начала соответствующей calibration. Записывать budget fingerprint/revision и время его фиксации; не выбирать budget под уже увиденный результат. |
| Проверка полноты | Проверить наличие всех обязательных идентификаторов, hashes, событий, границ, inputs и счётчиков; `end_ns > start_ns`, положительные wall durations, сопоставимость host/build/profile и целостность component run references. Отсутствующий/несопоставимый input означает Numeric `blocked`; не заполнять его inferred value. Если формула даёт `N < 2`, не округлять до двух и не назначать меньший произвольный N — Numeric остаётся blocked до обоснованного изменения budget или measurement profile с новым record. |

Эта схема определяет форму будущего measurement, а не сообщает значения, принятый calibration result или guest pass. Функциональные повторы S5-08/S5-11 сохраняют собственные acceptance criteria; они не становятся автоматически калибровочным record, если не используют заранее утверждённый метод и полный набор полей выше.

### S5-15: число повторов

Пусть `B_series` — заранее выделенный и обоснованный budget wall time серии, а `W_boot` — измеренный calibration wall time принятого S5-08 boot path при зафиксированных host/build/guest inputs. Число `N` можно вычислить до S5-15 из этого отдельного калибровочного прогона:

```text
N = floor(B_series / W_boot), with N >= 2 for a comparison
```

На дату этого документа числовой N не назначается: `I_boot`/`W_boot` отсутствуют, потому что S4-39 ещё не подтвердил целевой userspace marker, и `B_series` ещё не задан. Поэтому Numeric gate и полный S5-01 остаются blocked; S5-15 не стартует до их принятия. Сокращать N до произвольного числа нельзя.

### S5-16: длительность soak

После принятия S5-10/S5-11/S5-14 их зафиксированные workload profiles составляют калибровочный mixed C+D+G cycle; порядок, начальное состояние, inputs, completion markers и граница цикла должны быть перечислены в calibration record, без изменений сценарных критериев. До запуска S5-16 отдельный calibration run измеряет `I_cycle` (retired guest instructions) и `W_cycle` (wall time одной полной итерации) на выбранных host/build/inputs. Отдельно заранее задаётся целевой soak budget `B_soak` с обоснованием и временным окном стенда. Эти входы позволяют пройти Numeric gate до самого soak:

```text
K_estimate = ceil(B_soak / W_cycle)
D_soak_estimate = K_estimate * W_cycle
T_cycle_user_estimate = I_cycle / R   # только user-CPU projection, не wall time
```

`K_estimate` и `D_soak_estimate` — плановые оценки, а не фактические elapsed. Во время принятого soak time gate проверяет фактический wall elapsed и прекращает прогон после завершения текущего цикла, когда `elapsed >= B_soak`; записываются фактические `K_actual`, `elapsed_actual` и завершённые/незавершённые циклы. Они могут отличаться от estimate из-за разброса `W_cycle`; estimate нельзя выдавать за runtime evidence.

Числовые `I_cycle`, `W_cycle`, `B_soak`, `K_estimate` и `D_soak_estimate` отсутствуют: принятый C+D+G cycle и target budget ещё не заданы. Поэтому Numeric gate, полный S5-01 и S5-16 остаются blocked; длительность нельзя назначать как круглое «overnight» число.

Точный блокер и следующий порядок шагов занесены в [ISSUE-076](../KNOWN_ISSUES.md#issue-076-s5-01-run-duration-inputs-missing). `I_boot`/`W_boot` берутся из уже принятого S5-08 calibration path, а не из серии S5-15; `I_cycle`/`W_cycle` — из уже принятого C+D+G cycle, а не из S5-16. Предварительные числа не объявляются приёмкой.

## 6. Сопоставление guest и host hashes

Для input artifacts host фиксирует SHA-256 файла до запуска. `linux-direct` уже пишет фактические hashes `vmlinuz`, initramfs и ACPI blob; `memory_sha256` и поля `*_matches_file` подтверждают, что kernel protected-mode bytes и initrd в guest memory совпадают с прочитанными host inputs. Эти поля подтверждают load/readback boundary и не заменяют guest workload.

Для guest-created file гость после завершения записи сообщает path, exact byte length и SHA-256 файла с конкретным nonce/run ID. После clean shutdown и повторного открытия storage host получает те же file bytes документированным способом, считает длину и SHA-256 независимо, затем сравнивает pair `(length, digest)`. Метод получения guest file на host и путь к сырым bytes записываются в manifest. Если такого пути пока нет, результат не подменяется сравнением всего disk image и остаётся `blocked` до предусмотренного этапом канала/инструмента.

Disk-image before/after hashes могут быть полезным дополнительным наблюдением для точного RAW artifact, но guest file SHA-256 относится к одному файлу внутри filesystem. Не выводить равенство или целостность гостевого файла из сравнения этих разных уровней.
