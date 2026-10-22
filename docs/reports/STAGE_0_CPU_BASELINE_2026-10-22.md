# Предварительный CPU baseline S0-13

**Дата:** 22 октября 2026 года. **Статус:** пилотный прогон на основном M3 Max; не является приёмкой E04 и не подтверждает производительность полной VM.

## Конфигурация и вход

- Host: MacBook Pro, Apple M3 Max, 36 GB RAM; macOS Tahoe 26.7.1, build 25G241, arm64.
- Toolchain: rustc 1.99.0, cargo 1.99.0; release build.
- Исходники: HEAD `92f59b12a912ace5f262d0df9c652b63b9439de0`, рабочее дерево было изменено. Снимок `git status --short` сохранён вместе с сырыми данными.
- Вход: `tests/fixtures/x86/s0_09_sub_jnz_loop.hex`, SHA-256 `34f5d65b6ae73fcf4516e0e72daa6753be67603d8f82d16fe69724773269aea2`.
- Начальное состояние: `RAX=3`, `RCX=1`, остальные GPR и RIP равны нулю; budget 32. Оба движка на каждой проверке возвращали `RAX=0`, `RCX=1`, `RIP=5`, RFLAGS `0x46`, 6 выполненных инструкций.
- Команда: `cargo build --release --offline -p od-runner`, затем `target/release/od-runner --hex-file tests/fixtures/x86/s0_09_sub_jnz_loop.hex --rax 3 --rcx 1 --budget 32 --benchmark`.
- Harness калибровал каждую серию до batch не короче 200 ms, записал 20 сырых образцов и чередовал порядок измерений. Для decode, translation, compile/publish/release, interpreter и JIT были отдельные counts на sample; точные counts находятся в сырых данных.

## Результаты

Медиана и полный диапазон по 20 samples, наносекунд на одну операцию:

| Операция | Медиана | Min–max |
|---|---:|---:|
| Decode | 76 | 76–78 |
| Decode + JIT translation/lowering | 568 | 552–586 |
| Translation + executable mapping/publication + release | 4 538 | 4 398–5 064 |
| Interpreter, один вызов fixture | 227 | 213–238 |
| Повторное исполнение ранее скомпилированного JIT fragment | 11 | 10–12 |

Interpreter вызывает decode на каждом запуске; JIT использует ранее опубликованный `CompiledFragment`. Оба пути сбрасывают гостевое состояние перед каждым invocation, исполняют тот же вход с тем же instruction budget и проверяются на равенство результата до и после batch.

## Дополнительный четырёх-fixture пилот

Позже в тот же день release harness выполнен на четырёх поддерживаемых fixtures: 20 samples на fixture, все calibrated batches не короче 200 ms. Для каждого fixture `od-runner` сначала подтвердил совпадение interpreter/JIT; сырые sample-строки и точные команды сохранены в локальном каталоге ниже. Значения — наносекунды на один вызов соответствующей операции, median [min–max]:

| Fixture (инструкций в invocation) | Decode | Translation | Compile/publish/release | Interpreter | Reused JIT |
|---|---:|---:|---:|---:|---:|
| `add_overflow` (1) | 59 [58–62] | 449 [440–467] | 4 389 [4 330–5 144] | 104 [102–109] | 7 [6–7] |
| `cmp_jz` (2) | 75 [74–88] | 554 [541–590] | 4 533 [4 123–5 984] | 147 [146–152] | 7 [7–7] |
| `mov_imm64` (1) | 58 [58–60] | 401 [398–410] | 3 604 [3 410–4 891] | 106 [105–108] | 7 [7–7] |
| `sub_jnz_loop` (6) | 76 [75–78] | 531 [507–542] | 4 479 [4 380–5 074] | 213 [212–235] | 11 [10–11] |

Это расширяет набор входов, но остаётся одним коротким пилотным сеансом на register-only fixtures. При повторе `sub_jnz_loop` медианы translation и interpreter отличались от предыдущего прогона; условия питания, температуры и фона не контролировались, поэтому межпрогонную стабильность не установили. По этим цифрам не выбирается архитектура и не заявляется преимущество для полной VM.

## Ограничения

CPU fixtures — короткие register-only программы от одной до шести инструкций. Питание, thermal throttling и фоновые процессы не контролировались и не измерялись. Отдельный VMHost pilot включает process launch, handshake, ABI fragment creation и control round-trip, но не разделяет цену IPC и C ABI batching; память процесса не измерена. Наблюдаемое различие между CPU fixtures и сессиями не обосновывает решения для полной VM. Для E04 остаются контролируемые повторы, изолированные ABI/IPC сценарии и resource measurements; физический x86 oracle S0-12/E03 выполняется после этапа и не входит в зависимость этого отчёта.

## VMHost integration pilot

После CPU pilot выполнены 20 свежих запусков подписанного ad-hoc VMHost с одним корректным `MOV RAX,1` фрагментом. В каждом прогоне проверены handshake, RUN_ACK, RESULT и выход процесса; все 20 результатов совпали: `RAX=1`, `RIP=10`, exit reason 0, одна инструкция. Медиана [min–max] на основном M3 Max: процесс + HELLO/READY 11.65 ms [5.62–14.02]; RUN до RUN_ACK 160.6 μs [109.9–305.6]; RUN до RESULT 51.33 ms [51.16–51.61].

Это интеграционная задержка, а не изолированная цена IPC: RUN_ACK включает C ABI fragment creation, decode/compile и запуск worker; полный RUN round-trip дополнительно включает выполнение и пробуждение VMHost. Первоначальный control loop проверял завершение worker через `poll()` с timeout 50 ms. По этому замеру он заменён на ожидание control socket и worker completion pipe; оба дескриптора пробуждают один `poll()` без периодического тайм-аута. Повтор того же 20-run сценария после изменения дал RUN_ACK 107.6 μs [89.8–400.9] и RUN→RESULT 138.1 μs [114.5–479.7], медиана примерно в 372 раза меньше исходного пилота. Все 20 результатов корректны. Это подтверждает устранение измеренного 50 ms ожидания для данного пути; фоновая нагрузка не контролировалась, а отдельные затраты IPC/C ABI и пригодность для длительной нагрузки ещё не измерены. Spawn-to-ready имел выброс до 574 ms, поэтому не используется для вывода. RSS не снимался в timing pass; отдельная peak-RSS серия описана ниже. Raw samples и manifest до/после: `work/stage-0/2026-10-22-e04-vmhost-abi-ipc-resource/`.

## Public C ABI execute pilot

Дополнительно слинкованный C harness вызвал публичный `od_fragment_execute` 938,087 раз на том же `MOV RAX,1` fixture: 20 samples, каждый не короче 200 ms. После каждого вызова проверялись `RAX=1`, `RIP=10`, exit reason 0 и одна инструкция. Медиана составила 4,170.64 ns/call, диапазон по sample medians — 4,023.54–5,248.32 ns/call. Handle создавался один раз на sample, но ABI execute внутри заново проходит decoder/JIT compile/publication/release и исполняет фрагмент. Поэтому это стоимость полного C ABI execute path, а не чистый FFI overhead. В публичном ABI пока нет пакетного вызова для прямого A/B сравнения; решение о batch API не принималось. Raw CSV и harness source сохранены в том же локальном `work/` каталоге.

## Busy-helper STOP and resource pilot

На post-change VMHost выполнены 20 свежих запусков с busy helper (`JNZ -2`), после RUN_ACK передавался cooperative STOP. Все 20 вернули отдельный STOP_ACK, исходный `RAX=0x222` и exit reason `STOP_REQUESTED`; допустимо, что остановка пришла до первой гостевой инструкции. Медиана STOP_ACK составила 24.6 μs [18.5–44.8], а STOP→RESULT — 32.1 μs [22.5–67.3]. Это включает Python framing и планирование процесса и остаётся одним неконтролируемым pilot.

Peak RSS измерен отдельно через `wait4()` child usage для 20 новых signed VMHost процессов, каждый выполнил тот же короткий `MOV RAX,1` и завершился: `ru_maxrss` составил 5.94 MiB во всех 20 выборках. Затем измерены 20 пар одновременно работающих одноразовых VMHost с разными immediate/result: каждый процесс имел peak RSS 5.94 MiB, сумма двух независимых per-process peaks — 11.88 MiB во всех выборках; результаты и каналы не смешивались. Это baseline двух smoke helpers, не бюджет длительных VM или больших JIT code arenas. Размер executable mapping самого JIT fragment не раскрывается текущим ABI и остаётся неизвестным. `ps` sandbox запрещает, однако `wait4()` для собственных дочерних процессов доступен. Raw CSV и metadata записаны в локальном `work/stage-0/2026-10-22-e04-vmhost-abi-ipc-resource/`.

## Сырые данные

Исходный прогон и параметры сохранены в `work/stage-0/2026-10-22-cpu-s0-13.IqRQzS/`; дополнительная матрица из четырёх fixtures, metadata и raw output — в `work/stage-0/2026-10-22-cpu-s0-13-matrix-c_qpcam_/`. Каталоги не версионируются; здесь приведены агрегаты и точные входы для объяснения результатов.
