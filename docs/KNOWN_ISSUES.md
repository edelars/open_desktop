# Известные недоделки и проблемы

Реестр проблем, обнаруженных в ходе работы и не устранённых к завершению задачи. Правила ведения — в [AGENTS.md](../AGENTS.md#документирование-результатов-и-открытых-проблем). Планируемые возможности сами по себе не являются найденными проблемами и описываются в [ROADMAP.md](ROADMAP.md) и [планах этапов](plans/).

Для каждой записи указывать:

- Идентификатор, дату обнаружения и затронутый компонент.
- Суть недоделки или проблемы и её последствия.
- Условия воспроизведения или основание обнаружения, ссылки на исходники и результаты проверок при наличии.
- Статус: открыта, в работе или решена; необходимый следующий шаг.
- При решении — результат проверки и ссылку на обновлённую документацию реализации.

## Записи

### ISSUE-017 — Нет полного x86 LOCK ordering для общего memory backend

- **Дата обнаружения:** 2026-10-23.
- **Компонент:** `od-isa::GuestMemory`, S2-21f `LOCK XADD qword [memory], register`.
- **Статус:** открыта; S2-21f ограничен private synchronous RAM при последовательном исполнении. Shared/asynchronous memory, DMA, MMIO и SMP use остаются вне контракта.
- **Суть и последствия:** `atomic_fetch_add_integer` задаёт один backend вызов, wrapping update и no-mutation-on-error, но не задаёт полного x86 LOCK ordering, alignment/split-lock policy и не проверяет capability backend. Это достаточно только при отсутствии внешних конкурентных читателей/писателей и отложенных эффектов; generic `GuestMemory` не может динамически гарантировать это precondition. Unaligned local fixture проверяет backend bytes, но не hardware `#AC`/split-lock behavior.
- **Уточнение S2-21g:** обязательный `GuestMemory::mfence` completion call покрывает только явный MFENCE для opt-in private synchronous RAM. Он не служит ordering boundary для LOCK XADD и не закрывает эту проблему; S2-21f остаётся под тем же integration precondition.
- **Уточнение S2-22g:** valid A=0 descriptor для локального `MOV FS, AX` профиля обновляется одним `GuestMemory::write` длиной 1 по access byte после чтения/проверок. Это семантический byte-sized backend access, а не утверждение о физической ширине bus transaction. Он допускается только для private synchronous RAM при serial interpreter execution без внешних наблюдателей/писателей, DMA, shared aliases, delayed writes или MMIO. Срез полагается на общий контракт no-mutation-on-error и сам по себе не задаёт полного x86 LOCK ordering или SMP semantics.
- **Уточнение S2-22h/i:** тот же узкий A=0 byte write добавлен только для bare `MOV GS, AX` на CPL3 в отдельном local-only GDT profile; S2-22i добавил один local-only последовательный data read через загруженную GS.base. Оба сохраняют private synchronous RAM и serial-execution preconditions и не закрывают общий LOCK ordering или SMP semantics.
- **Основание:** архитектурный анализ S2-21f сопоставил [memory trait](../crates/od-isa/src/memory.rs), [архитектурный контракт памяти](ARCHITECTURE.md) и Intel SDM revision 093 LOCK/XADD semantics. `FlatMemory` тестирует local split-region behavior, но не подтверждает межпоточную, DMA, MMIO или hardware атомарность.
- **Воспроизведение/условие:** использовать S2-21f `F0 48 0F C1 /r` через `GuestMemory`, который имеет concurrent readers/writers, shared aliases, DMA, MMIO или отложенные writes; interpreter не проверит backend capability и не обеспечивает требуемый ordering.
- **Следующий шаг:** до подключения такого backend определить memory ownership/capability и complete x86 LOCK ordering contract, затем добавить concurrent/ordering tests и отдельную native/physical acceptance. До этого сохранять S2-21f и S2-22g/h/i memory ownership preconditions; не расширять LOCK формы или descriptor-write profile на shared, asynchronous, MMIO или SMP memory.

### ISSUE-015 — Разногласие SDM по NT для POPF word на CPL3/IOPL0

- **Дата обнаружения:** 2026-10-23.
- **Компонент:** `od-isa` S2-17r, long-mode word `POPF` (`66 9D`) в CPL3/IOPL0.
- **Статус:** открыта; входное значение NT=1 возвращает `Unsupported`, архитектурное поведение не заявлено.
- **Суть и последствия:** Intel SDM revision 093 Vol.2B Table 4-15 и Vol.1 §7.3.13.2 p.7-22 расходятся относительно загрузки NT bit для 16-bit POPF при CPL3/IOPL0. Чтобы не закреплять семантику на основании противоречащих нормативных разделов, реализация отклоняет до commit как source NT=1, так и начальное NT=1; NT=0 поддерживается. Это сужает принятый профиль и исключает физическое утверждение для NT=1.
- **Основание:** сверка Intel SDM order number 253667-093US, revision 093, Vol.2B Table 4-15 с Vol.1 §7.3.13.2 p.7-22 при реализации S2-17r. Literal fixture `s2_17r_popfw_updates_allowed_flags` использует TF=0 и NT=0 и не разрешает разногласие.
- **Следующий шаг:** найти авторитетную errata/уточнение Intel либо надёжное дополнительное нормативное свидетельство. До этого сохранять `Unsupported` для NT=1 и не расширять fixture/native profile.

### ISSUE-016 — Не установлен restart state MOVS после destination fault

- **Дата обнаружения:** 2026-10-23.
- **Компонент:** `od-isa` S2-18a/S2-18b/S2-18c/S2-18d, long-mode non-REP `MOVSB` (`A4`), `MOVSW` (`66 A5`), `MOVSD` (`A5`) и `MOVSQ` (`REX.W A5`) с implicit `[RSI]` source и `[RDI]` destination; S2-18e non-REP `STOSB` (`AA`) with AL source and implicit `[RDI]` destination.
- **Статус:** открыта только для non-REP destination-fault snapshots; это непроверенная архитектурная граница, а не подтверждённый дефект runtime.
- **Основание и последствия:** Intel SDM revision 093, Vol.2B MOVS задаёт `DEST ← SRC`, RSI/RDI и шаг по DF (1/2/4/8), но не говорит, обязаны ли индексы/RIP оставаться прежними при non-REP destination `#PF` после успешного source read. STOSB задаёт `DEST ← AL` и шаг RDI на один, но не устанавливает post-fault snapshot для non-REP destination `#PF`. Executors commit the relevant index register(s) and common RIP only after a successful destination write; this bounded-memory no-commit behavior is local policy, not a hardware restart guarantee. REP suspension is separately specified in the SDM as preserving the next element, remaining count, and instruction RIP after completed iterations; S2-19a follows and tests that between-iteration restart contract. It does not infer fault priority. Fault-only fixtures remain local and are rejected by native admission.
- **Воспроизведение/условие:** execute a locally supported non-REP MOVS form with readable `[RSI]` and unmapped `[RDI]`, or bare `AA` STOSB with unmapped `[RDI]`, then observe the `#PF` snapshot. Local replay: S2-18a–d `*_write_fault` fixtures or `s2_18e_stosb_write_fault` using an injected write fault in `FlatMemory`; the test result is not a physical expectation.
- **Следующий шаг:** find an unambiguous normative/errata source or obtain a reliable post-fault snapshot during the shared final Mac mini acceptance substage. Until then, keep the local commit policy and do not claim the hardware RSI/RDI/RIP state after a string destination `#PF`.

### ISSUE-014 — Не установлено состояние RSP после fault у memory-destination POP

- **Дата обнаружения:** 2026-10-23.
- **Компонент:** `od-isa` S2-17j/S2-17l, long-mode qword `POP r/m64` and word `POP r/m16` (`8F /0`) с ordinary memory destination.
- **Статус:** открыта; нормативный post-fault CPU state не подтверждён, не установленный дефект runtime.
- **Основание и последствия:** Intel SDM order number 253667-093US, revision 093, Vol.2B, `POP` pp. 4-398–4-402 описывает чтение значения со старого stack top, увеличение RSP на размер операнда до записи в memory destination и вычисление адреса destination после увеличения, если RSP используется как base. Раздел перечисляет `#PF` для memory destination, но не устанавливает, обязано ли увеличенное RSP сохраняться при fault destination store. Executor вычисляет EA на staged state после `RSP += 8` для qword и `RSP += 2` для word, публикуя staged state только после успешной записи; это локальная fault policy, не подтверждённое архитектурное postcondition. Store-`#PF` regressions намеренно проверяют exception и неизменившуюся RAM, но не сравнивают GPR/RIP; native admission отвергает fault-only fixtures.
- **Воспроизведение/условие:** `8F /0` или `66 8F /0` с читаемым значением на старом `[RSP]` и memory destination, который вызывает store `#PF`. Вызвать `step` с инъекцией architectural write fault в `FlatMemory`; проверить #PF и неизменившуюся память. CPU post-state не сравнивать до уточнения нормативного поведения.
- **Следующий шаг:** найти однозначное Intel/AMD normative или errata evidence либо получить надёжный физический x86 snapshot в общем финальном Mac mini substage. До проверки не закреплять архитектурное ожидание для RSP/RIP/GPR при destination-store fault. Успешный `[RSP]` путь использует post-increment EA согласно SDM; эта успешная семантика не затронута проблемой. Контекст реализации: [S2-17j report](reports/STAGE_2_REPORT.md#s2-17j--qword-memory-destination-pop-local), [S2-17l report](reports/STAGE_2_REPORT.md#s2-17l--16-bit-memory-destination-pop-local), [CPU contract](specs/CPU_CONTRACT.md) и [plan](plans/STAGE_2.md).

### ISSUE-013 — Не подтверждён приоритет target `#GP` и stack fault для near CALL

- **Дата обнаружения:** 2026-10-22.
- **Компонент:** `od-isa` S2-17c/d/e, near relative `CALL rel32` (`E8`) и near indirect `CALL r/m64` (`FF /2`) с register или ordinary memory source, включая порядок source/target/stack fault checks.
- **Статус:** открыта; непроверенный приоритет combined fault, не подтверждённый дефект.
- **Основание и последствия:** executor проверяет canonicality target перед вызовом stack helper для `E8 rel32` и `FF /2`; memory `FF /2` перед этим читает qword source. Отдельные source-read fault, noncanonical-target `#GP` и stack-fault cases проходят. Для случая, когда near CALL получил noncanonical target и одновременно invalid/noncanonical guest stack, архитектурный победивший fault не установлен. Intel SDM revision 093, Vol.2A, `CALL` p.3-125 описывает operation pseudocode через временный RIP, stack capacity, push и обновление RIP, а `#GP` для target перечисляет отдельно; эта подача сама по себе не подтверждает приоритет combined fault для direct, register-indirect или memory-indirect формы. Independent Sol-medium reviews для S2-17d/e чистые; S2-17e локально проверяет отдельные source/target/stack faults без commit; combined dual-fault fixture и expected physical snapshot пока не созданы.
- **Воспроизведение/условие:** выполнить `E8 rel32`, register-only `FF /2` или memory `FF /2` (читаемый source содержит noncanonical qword) при одновременно invalid/noncanonical guest stack. Для memory CALL текущий executor сначала читает source, затем проверяет target canonicality и stack helper; наблюдаемый порядок реализации не устанавливает нормативный expected winner.
- **Следующий шаг:** найти нормативное или errata evidence; иначе проверить возможность надёжно захватить combined-fault result для direct/register/memory indirect CALL в harness, создать dual-fault fixture с expected state и включить её в общий финальный physical Mac mini acceptance evidence. До подтверждения не менять runtime по догадке. Контекст — [S2-17e report](reports/STAGE_2_REPORT.md#s2-17e--memory-near-indirect-call-local), [CPU contract](specs/CPU_CONTRACT.md) и [plan](plans/STAGE_2.md).

### ISSUE-012 — Не подтверждена семантика operand range через верх u64

- **Дата обнаружения:** 2026-10-22.
- **Компонент:** `od-isa` S2-08, full-width address validation диагностического long-mode профиля.
- **Статус:** открыта; непроверенная semantic boundary, не подтверждённый hardware bug.
- **Основание и последствия:** проверка Intel SDM093 Vol.3A, §§4.5.1 и 6.15, не дала однозначного нормативного ответа для многобайтного доступа, у которого endpoint переполняет u64. Поэтому реализация возвращает `MemoryError::AddressOverflow` до backend и без commit; она не выдаёт неподтверждённый #GP. Это harness/memory-boundary error, не установленная x86 exception semantics.
- **Воспроизведение:** в диагностическом long-mode профиле задать `RAX = 0xffffffffffffffff` и исполнить `MOV r64, [RAX]` (например `48 8b 08`, destination RCX); старт canonical, endpoint 8-byte operand непредставим u64. Проверить AddressOverflow, сохранённые RIP/GPR/flags и отсутствие обращения к backend.
- **Следующий шаг:** найти точный нормативный источник или независимое подтверждение на физическом x86 для этого диапазона, затем закрепить architectural outcome и regression. Закрыть до полной приёмки этапа 2; локальная проверка выбранного boundary policy не заменяет hardware confirmation. Контекст — [CPU contract](specs/CPU_CONTRACT.md#исполнение-mov--s2-08).

### ISSUE-011 — AMD APM 4.09 не прочитан через web fetch

- **Дата обнаружения:** 2026-10-22.
- **Компонент:** нормативные источники CPU, S2-00; проверка AMD semantics.
- **Статус:** открыта; ограничение доступа к источнику в текущей среде.
- **Основание и последствия:** повторный `web.run.open` вернул `Failed to fetch … (404) Not Found` для [страницы 40332 revision 4.09](https://docs.amd.com/v/u/en-US/40332_4.09_APM_PUB), [официального PDF](https://www.amd.com/content/dam/amd/en/documents/processor-tech-docs/programmer-references/40332.pdf) и [indexed content API](https://docs.amd.com/api/khub/documents/68GKiN0gMEd6bMddsmhPwg/content). Поиск показывает metadata/выдержки, но тело manual через этот инструмент не прочитано; они не считаются проверкой нормативной семантики. Наблюдение не доказывает общую недоступность сайта или отсутствие документа.
- **Повторная ограниченная проверка:** 22 октября `web.run.open` снова вернул 404 для landing 4.09, khub content, отдельного официального Volume 3 `24594.pdf` и Volume 1 khub. Другой HTTP fetch получил для landing только SPA shell «Loading application…»; после загрузки в Chrome официальный портал показал «Content not found» с сообщением о перемещённом/удалённом документе именно по этому URL. Поиск портала `40332` результата в наблюдаемой странице не дал. Отдельный `urllib.request.urlopen` GET официального [24594.pdf](https://www.amd.com/content/dam/amd/en/documents/processor-tech-docs/programmer-references/24594.pdf), предназначенный для обработки bytes в памяти и `pdftotext - -`, завершился `TimeoutError: The read operation timed out` с лимитом 25 s до получения PDF. PDF bytes/hash и текст разделов не получены; search snippets не использованы вместо manual. Это разные наблюдаемые границы инструментов/URL, не вывод об универсальной недоступности AMD.
- **Следующий шаг:** получить официальный AMD PDF revision 4.09 через доступный официальный канал, проверить титул/version и необходимые разделы, сохранить provenance/hash и обновить [CPU contract](specs/CPU_CONTRACT.md) и [отчёт](reports/STAGE_2_REPORT.md). До этого новые vendor-specific решения считать непроверенными по AMD; уже реализованный GPR contract подтверждён Intel SDM.

### ISSUE-010 — graphify не извлекает символы публичного C ABI header

- **Дата обнаружения:** 2026-10-22.
- **Компонент:** производный граф `graphify-out/`, C extractor инструмента graphify.
- **Статус:** открыта; ограничение инструмента навигации, не подтверждённый дефект C header.
- **Основание и последствия:** `graphify update .` завершился успешно, но сообщил syntax error на строке 68 `include/open_desktop/host_abi.h` и отсутствие извлечённых символов этого файла. Поэтому граф недостаточен для полного анализа связей публичного ABI. Успешный workspace/VMHost build не даёт оснований считать эту диагностику ошибкой компиляции исходника.
- **Воспроизведение:** выполнить `graphify update .` на текущем checkout. Граф после ревью содержит 1184 nodes и 2363 edges; предупреждение о header сохранилось.
- **Следующий шаг:** проверить поддержку используемых C macros/атрибутов в extractor и исправить или обновить инструмент. До этого для ABI-ревью дополнительно читать нормативный header и проверять штатным компилятором. Не менять ABI только ради устранения предупреждения graphify.

### ISSUE-009 — Дефекты валидации, сохранения результатов и reducer этапа 1

- **Дата обнаружения:** 2026-10-22.
- **Компоненты:** `od-testkit::{case,result,minimize}`, `tests/cpu/stage1_oracle.py`, `stage1_harness.py`.
- **Статус:** решена 22 октября 2026 года.
- **Суть и последствия:** oracle использовал непроверенные ID как пути и не отклонял конфликтующие ID/basename; batch терял вывод при native failure, а входы не фиксировались перед запуском. Generated corpus заменял ручные cases. Tagged JSON enums игнорировали лишние поля. Reducer принимал разовый flaky mismatch, не переносил ненулевой entry RIP и превышал лимит callback при значениях 0/1.
- **Воспроизведение и исправления:** семь находок и отдельные regression tests описаны в [ревью этапа 1](reports/STAGE_1_REVIEW_2026-10-22.md). Первые четыре Rust regressions воспроизведены до исправления. Точные входы и process outputs теперь сохраняются; валидация выполняется до использования ID в путях; reducer подтверждает каждого кандидата дважды и переносит entry.
- **Результат проверки:** 31 testkit test на обоих hosts, 6 Python regressions, local/workspace/fixture checks и VMHost smoke прошли. Объединённый физический прогон: 1010 уникальных cases × 2 повтора, 2020 native snapshots, ноль расхождений; [evidence](reports/data/stage1/review-2026-10-22.json).
- **Следующий шаг:** отсутствует для перечисленных дефектов; ограничения register-only профиля сохранены в [CPU_ORACLE](testing/CPU_ORACLE.md).

### ISSUE-008 — Нет Rust toolchain и checkout на физическом x86 host

- **Дата обнаружения:** 2026-10-22.
- **Компонент:** `tools/x86-runner`, физическая проверка S1-05/S1-06 и подтверждение Linux-сборки S1-04.
- **Статус:** решена 22 октября 2026 года.
- **Суть и последствия:** исходное отсутствие toolchain и checkout мешало проверить Linux `setrlimit`/watchdog и запускать нативные fixtures. Владелец разрешил установить toolchain и скопировать checkout в отдельный каталог.
- **Основание:** Rust stable 1.99.1 установлен в пользовательский каталог; checkout скопирован в `~/projects/open_desktop-stage1`. На Mac mini прошли Linux workspace check, тесты целевых crates, Linux `setrlimit` regression test и сборка `od-x86-runner`. Затем S1-06 автоматически исполнил 10 cases × 2 повтора на физическом Intel CPU; подробности и доказательства находятся в [CPU_ORACLE](testing/CPU_ORACLE.md), [STAND](testing/STAND.md) и [плане этапа 1](plans/STAGE_1.md).
- **Следующий шаг:** отсутствует; устранённое условие закрыто. Ответственный: инженер runner.

### ISSUE-005 — RUN_ACK опережает готовность VMHost к STOP

- **Дата обнаружения:** 2026-10-22.
- **Компонент:** `apps/vmhost`, запуск `FragmentWorker` и C ABI lifecycle.
- **Статус:** решена 22 октября 2026 года.
- **Суть и последствия:** успешный RUN_ACK отправляется сразу после постановки worker в DispatchQueue. C ABI теперь сохраняет STOP, пришедший до начала исполнения, и сбрасывает флаг после завершения run, поэтому остановка не зависит от планирования worker.
- **Воспроизведение:** штатный launcher с `--hex 75fe --budget 18446744073709551615 --stop-after-ms 0`; на M3 Max в 5 из 100 запусков получен `VMHost returned status 6`. Подробности и сборка — в [ревью S0](reports/STAGE_0_REVIEW_2026-10-22.md).
- **Результат:** `od_fragment_request_stop` сохраняет ранний запрос для следующего run; [control-race smoke](../tests/system/vmhost_control_race_smoke.py) прошёл 100/100 запусков немедленного STOP с reason 2. Отключение канала после RUN_ACK также проверено: VMHost остановил worker и завершил процесс.

### ISSUE-006 — Неполный control frame блокирует готовый RESULT

- **Дата обнаружения:** 2026-10-22.
- **Компонент:** `apps/vmhost/Sources/main.swift`, `ControlFrame.read`.
- **Статус:** решена 22 октября 2026 года.
- **Суть и последствия:** POLLIN означает наличие хотя бы части кадра; блокирующий read всего кадра переставал обслуживать completion pipe. VMHost теперь неблокирующе накапливает данные и декодирует только полные кадры, продолжая опрашивать completion.
- **Воспроизведение:** после RUN_ACK передать один байт `O` следующего header и держать socket открытым. Для конечного budget=2000000 RESULT не пришёл за 2 s, тогда как контроль без частичного кадра завершился за 0.021 s со стартом процесса. Полные входы — в [ревью S0](reports/STAGE_0_REVIEW_2026-10-22.md).
- **Результат:** [control-race smoke](../tests/system/vmhost_control_race_smoke.py) подтверждает доставку RESULT при незавершённых header и payload; проверка отключения канала также проходит.

### ISSUE-007 — Launcher теряет результат при завершении до отложенного STOP

- **Дата обнаружения:** 2026-10-22.
- **Компонент:** `apps/macos/Sources/launcher.swift`, ожидание `--stop-after-ms`.
- **Статус:** решена 22 октября 2026 года.
- **Суть и последствия:** launcher раньше спал до таймера STOP и затем безусловно отправлял запрос, теряя RESULT, если helper естественно завершился первым. Launcher теперь ожидает socket и срок STOP одновременно, затем принимает RESULT без ACK или STOP_ACK перед RESULT.
- **Воспроизведение:** `--hex 48b80100000000000000 --stop-after-ms 25` возвращает exit=1 и `control frame write failed`. Подтверждено на свежих release smoke-бинарниках; [ревью S0](reports/STAGE_0_REVIEW_2026-10-22.md).
- **Результат:** точное воспроизведение с задержкой 25 ms теперь завершается reason 0; 20 запусков с нулевой задержкой завершаются корректно с reason 0 либо reason 2. Допустимые порядки описаны в [CONTROL_PROTOCOL_V0.md](../protocols/control/CONTROL_PROTOCOL_V0.md).

### ISSUE-001 — Корневые пользовательские каталоги и Finder metadata

- **Дата обнаружения:** 2026-10-22.
- **Компонент:** структура репозитория, [карта проекта](PROJECT_LAYOUT.md).
- **Статус:** решена 22 октября 2026 года.
- **Проблема и последствия:** проверка структуры обнаруживает не описанные в нормативной карте объекты корня: файлы `.DS_Store` и каталоги `.codex/`, `.opencode/`, `graphify-out/`, `iso/`. Из-за этого `check_layout.py` завершается ошибкой, хотя часть объектов может быть локальной конфигурацией или временными данными.
- **Воспроизведение:** из корня проекта запустить `python3 tools/check_layout.py`. На 22 октября 2026 года получены `ERROR: Unmapped root file: .DS_Store` и `ERROR: Unmapped root directory` для `.codex`, `.opencode`, `graphify-out`, `iso`.
- **Результат:** в [карте проекта](PROJECT_LAYOUT.md) перечислены допустимые необязательные каталоги `.codex/`, `.opencode/`, `graphify-out/` и `iso/`; layout checker учитывает их как локальные/производные данные и пропускает `.DS_Store`. Файлы и каталоги не перемещались и не удалялись. `python3 tools/check_layout.py` после изменения прошёл.

### ISSUE-002 — Задержка VMHost RUN→RESULT из-за polling worker completion

- **Дата обнаружения:** 2026-10-22.
- **Компонент:** `apps/vmhost` control loop и VMHost completion signaling.
- **Статус:** решена 22 октября 2026 года для короткого VMHost RUN→RESULT path; длительная нагрузка и отдельный STOP latency требуют дальнейшей проверки.
- **Проблема и последствия:** первоначальный loop ожидал worker completion через `poll()` control socket с timeout 50 ms. Один короткий `MOV RAX,1` integration run дал медианный RUN→RESULT 51.33 ms [51.16–51.61] на 20 fresh VMHost processes.
- **Основание:** [S0-13 CPU baseline report](reports/STAGE_0_CPU_BASELINE_2026-10-22.md), раздел VMHost integration pilot; raw samples до/после в локальном `work/stage-0/2026-10-22-e04-vmhost-abi-ipc-resource/`. Все 40 процессов вернули ожидаемые `RAX=1`, `RIP=10`, exit reason 0 и instruction count 1. Источник timeout и реализация completion signal: `apps/vmhost/Sources/main.swift`, `serviceStopRequests()` и `FragmentWorker`.
- **Результат:** VMHost теперь одновременно ждёт control socket и completion pipe без периодического timeout. В повторе из 20 запусков медиана RUN→RESULT составила 138.1 μs [114.5–479.7], примерно в 372 раза меньше исходного пилота. Повторные launcher и isolation smoke прошли. Изменение и ограничения измерения описаны в [S0-13 baseline report](reports/STAGE_0_CPU_BASELINE_2026-10-22.md).
- **Ограничения и следующий шаг:** сравнение было одним коротким сеансом при неконтролируемых условиях и не является изолированной мерой IPC или результатом длительной нагрузки. После исправления отдельный busy-helper pilot измерил STOP_ACK и STOP→RESULT; значения и ограничения описаны в S0-13 report. Для финального решения повторить с контролируемым фоном и длительной нагрузкой.

### ISSUE-003 — Временный SSH-доступ к M1 Pro для JIT smoke

- **Дата обнаружения:** 2026-10-22.
- **Компонент:** внешний контрольный Mac M1 Pro, проверка S0-08/E01.
- **Статус:** решена 2026-10-22 для разрешённого smoke-прогона.
- **Суть и последствия:** исходный SSH-вход без ключа завершился отказом аутентификации, поэтому запуск на M1 Pro зависел от настройки временного ключа.
- **Основание:** по запросу владельца SSH-хост достигнут 22 октября; после явного согласия владельца на передачу трёх smoke-файлов использован временный Ed25519 ключ. Приватный ключ не попадал в репозиторий.
- **Результат:** на M1 Pro (macOS 14.6.1, MacBookPro18,3) прошли arithmetic JIT, cooperative STOP и process-isolation smoke. SHA-256 и ограничения опыта записаны в [STAND.md](testing/STAND.md); статус S0-08 — в [плане](plans/STAGE_0.md) и [отчёте](reports/STAGE_0_REPORT.md).
- **Очистка доступа:** после проверок приватный ключ на основном Mac и каталог с тремя переданными smoke-файлами на M1 удалены. Владельцу остаётся удалить строку с комментарием `open-desktop-stage0-m1-temp` из `~/.ssh/authorized_keys` на M1.

### ISSUE-004 — S0-08: Developer ID distribution и проверка конечного bundle

- **Дата обнаружения:** 2026-10-22.
- **Компонент:** `od-host`, VMHost supervisor и путь поставки, S0-08/E01.
- **Статус:** открыта; локальные fault-diagnostics закрыты, но этап 0 не принят до проверки пути выпуска.
- **Суть и последствия:** unit test подтверждает fault в generated code внутри дочернего процесса, а protocol-level test helper подтверждает `SIGSEGV` диагностику launcher-а. Временная ad-hoc `.app` smoke-упаковка валидна и исполняет JIT, но не подтверждает Developer ID distribution или notarization. Владелец сообщает о наличии Developer ID certificate, но соответствующая signing identity (сертификат с доступным private key) не найдена в keychains проверенных M3 Max и M1 Pro. Членство Apple Developer Program не активно; Apple notary service недоступен до его возобновления.
- **Основание:** проверки `generated_code_fault_is_confined_to_child_process`, `vmhost_generated_fault_smoke.py` и `vmhost_app_bundle_smoke.sh` прошли на M3 Max. Инвентаризация подписи и пределы теста записаны в [STAND.md](testing/STAND.md) и [отчёте этапа 0](reports/STAGE_0_REPORT.md).
- **Следующий шаг:** установить на M3 Max действующую Developer ID Application identity вместе с соответствующим private key и проверить подписание конечного Open Desktop bundle. Если требуется проверить notarization, владелец должен возобновить членство Apple Developer Program и предоставить авторизацию для notary service; учетные данные не передавать через чат. Затем повторить S0-14 review. До появления доступной identity Developer ID signing не проверено, а без активного членства notarization недоступна. Ответственный: владелец конфигурации подписи совместно с инженером macOS.

### ISSUE-018 — S2-22 покрывает только ограниченные selector/descriptor slices

- **Дата обнаружения:** 2026-10-23.
- **Компонент:** `od-isa` long-mode сегментная модель и S2-22.
- **Статус:** открыта; это зарегистрированный остаток этапа, не подтверждённый дефект S2-22f/g/h/i.
- **Суть и последствия:** S2-22a–i задают локальные профили для отдельных FS/GS base/selector операций, CPL3 VERW, ограниченных `MOV FS, AX` / `MOV GS, AX` загрузок из GDT с A-bit update только в private synchronous RAM и одного точного GS.base composition case. S2-22i показывает потребление загруженной базы уже поддержанной GS memory формой; он не расширяет профиль на другие последовательности. Эти срезы не реализуют LDT, null segment loads, общие accessed-bit updates, остальные GS/SS loads, stack checks, другие CPL/modes или применение protected-mode limit/type checks. Поэтому Stage 2 не может объявить полную сегментную архитектуру или завершить S2-22 по результатам этих профилей.
- **Основание:** ограничения зафиксированы в [плане S2-22](plans/STAGE_2.md), [CPU contract](specs/CPU_CONTRACT.md) и [отчёте этапа 2](reports/STAGE_2_REPORT.md). Локальные fixture/unit checks подтверждают только перечисленные формы.
- **Следующий шаг:** после утверждения обязательного guest profile в S2-00 выбрать отдельные bounded slices с собственным fault/state matrix и review. Не расширять текущие профили автоматически на перечисленные формы.

### ISSUE-019 — Семантика арифметики и инструкций x87 остаётся открытой

- **Дата обнаружения:** 2026-10-23.
- **Компонент:** `od-isa::x87`, S2-29–S2-39.
- **Статус:** открыта. S2-29a–q наращивают bounded raw/finite helpers: representation/classification, finite normalization, exact alignment/sum/product/quotient, mathematical rounding, normal/subnormal packing и композиции arithmetic→one-round result. S2-30a теперь исполняет только bare `DB E3` FNINIT при заранее существующем x87 state; это не закрывает общую x87 instruction semantics.
- **Суть и последствия:** `divide_finite_to_binary80`, `round_pack_exact_coefficient` и `add_finite_same_exponent_to_binary80` завершают только числовое округление и кодирование конечных binary80 результатов. Division отклоняет нулевые входы; coefficient/add helpers отклоняют точный ноль, а add также отклоняет два нуля, unequal exponents и finite-range overflow. Signed zero сохраняется только для ненулевого крошечного результата division. Ни один helper не выбирает архитектурный знак нулевого результата. Helpers не реализуют x87 RC/PC/control-word semantics, architectural underflow/precision effects, special values, гостевые FDIV/FADD/FSUB, flags/exceptions. Исполнение FNINIT не реализует проверку CR0.EM/TS/#NM, предшествующий WAIT FINIT, pending-exception delivery или другие x87 инструкции. Совместное состояние с MMX/SSE, native-state observation и полная save/restore integration также остаются открытыми. Наличие helpers не подтверждает общее исполнение x87 или наличие FPU feature; x87 snapshot остаётся ограниченным локальным профилем.
- **Основание:** локальные literal vectors покрывают exact finite intermediates, mathematical rounding, normal/subnormal bytes и композиции finite division/coefficient/addition с one-round pack. Для S2-29q проверяются ±1+±1, signed normal ties в четырёх режимах, минимум/максимум subnormal границы, zero-neutral для normal/subnormal/pseudo-denormal в обоих порядках, max-finite plus zero, overflow rejection, cancellation, все zero-sign pairs, unequal exponents и неизменность input/provenance. Сумма двух significands ограничена `2^65-2`, поэтому synthetic carry-to-four недостижим; exact subnormal sums кратны minimum subnormal, поэтому inexact subnormal tails и округление в underflow zero недостижимы. Full x87 architectural behavior не проверялся. См. [отчёт этапа 2 S2-29q](reports/STAGE_2_REPORT.md#s2-29q--compose-same-exponent-finite-addition-with-one-round-binary80-packing-partial), [CPU contract S2-29q](specs/CPU_CONTRACT.md#local-s2-29q-same-exponent-finite-addition-to-one-rounded-binary80-result), [план S2-29q](plans/STAGE_2.md#s2-29q-compose-same-exponent-finite-addition-with-one-round-binary80-packing-partial), предыдущие [S2-29p](reports/STAGE_2_REPORT.md#s2-29p--round-exact-coefficient-to-one-binary80-result-partial) и [S2-29o](reports/STAGE_2_REPORT.md#s2-29o--compose-finite-division-with-single-round-binary80-packing-partial), и [исходник x87.rs](../crates/od-isa/src/x87.rs).
- **Следующий шаг:** выбирать дальнейшую конечную арифметику, architectural effects и ограниченные x87 instruction forms отдельными срезами, каждый с independent vectors и доступным native state observation. См. [локальный контракт S2-30a](specs/CPU_CONTRACT.md#local-s2-30a-bare-fninit-db-e3), [отчёт S2-30a](reports/STAGE_2_REPORT.md#s2-30a--bare-fninit-bounded-local-slice) и [план этапа](plans/STAGE_2.md#s2-30a-bare-fninit-bounded-local-slice). S2-29 и umbrella S2-28/S2-30 остаются открытыми.
