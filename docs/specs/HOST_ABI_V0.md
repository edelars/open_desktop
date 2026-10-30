# Публичный host ABI v0

Состояние на 22 октября 2026 года. Это минимальный C ABI для исполнения ограниченного CPU-фрагмента S0-09; он не является ABI готовой виртуальной машины.

Нормативные объявления находятся в [`include/open_desktop/host_abi.h`](../../include/open_desktop/host_abi.h), реализация — в `crates/od-host`. Не дублировать объявления в Swift вручную: Swift должен импортировать этот заголовок.

## Версия и данные

- `od_host_abi_version()` возвращает монотонный идентификатор `0x00000001` для этого draft v0.
- `od_cpu_state_v0` содержит четыре 64-bit GPR в порядке RAX, RCX, RDX, RBX, byte-offset RIP и RFLAGS. Значимы только CF/PF/AF/ZF/SF/OF; зарезервированный бит 1 выставляется движком.
- `od_execution_result_v0` возвращает итоговое CPU-состояние, причину выхода и число исполненных гостевых инструкций. `reserved` равен нулю.
- Максимальный размер входного фрагмента — 4096 байт. Формы инструкций и ветвлений проверяются тем же собственным decoder, что использует interpreter/JIT.

## Операции и владение

| Операция | Контракт |
|---|---|
| `od_fragment_create` | Синхронно проверяет и копирует неизменяемые байты. После успеха handle принадлежит вызывающей стороне; входной буфер можно освободить. При ошибке `*out_handle` остаётся NULL. |
| `od_fragment_execute` | Синхронно исполняет handle с instruction budget и копирует результат в предоставленную структуру. Сейчас register-only S0-09 проходит через eager block dispatcher с bounded cache/arena, созданными только на время этого вызова. Один handle не запускается параллельно: повторный run возвращает `BUSY`. |
| `od_fragment_request_stop` | Потокобезопасный запрос кооперативной остановки. Запрос до начала исполнения сохраняется и останавливает следующий run, в том числе когда входной RIP равен `code_end`; запрос во время исполнения проверяется на границах guest instructions/blocks. Флаг сбрасывается после завершения run. |
| `od_fragment_destroy` | Освобождает idle handle; во время run возвращает `BUSY`. Вызывающий обязан исключить новые обращения к handle перед уничтожением и не использовать его после успеха. |

Все указатели, кроме нулевого code pointer при длине 0, должны быть корректными и оставаться доступными на протяжении вызова. Результат не возвращается при ненулевом status. ABI не принимает владение памятью вызывающего кода или состояния.

## Коды результата

Status: `OK`, `INVALID_ARGUMENT`, `INVALID_CODE`, `BUSY`, `UNSUPPORTED_HOST`, `ALLOCATION_FAILED`, `INVALID_STATE`. Причины гостевого выхода: `CODE_END`, `INSTRUCTION_BUDGET`, `STOP_REQUESTED`. Status описывает host/API ошибку, exit reason — штатный результат исполнения.

Вызовы create/execute/request-stop/destroy требуют одного действительного handle. `request_stop` разрешён с другого host-потока одновременно с синхронным execute. Destroy требует внешней синхронизации с началом новых вызовов.

Rust panic не переводится в guest status и не перехватывается этим ABI; он завершает VMHost. Нарушение памяти или аварийный fault — дефект процесса. Отдельный helper/supervisor и IPC lifecycle ещё не реализованы, поэтому API пока не подтверждает изоляцию.

Проверки исходной версии: Rust-side lifecycle unit tests `c_abi_create_execute_stop_destroy_lifecycle` и `c_abi_stop_requested_before_execute_is_honored_once`; C smoke `tests/system/host_abi_smoke.c` собран и исполнен на основном M3 Max. Точная воспроизводимая команда линковки сохранена в [плане S0](../plans/STAGE_0.md#s0-10-проверить-abi-и-минимальный-ipc).

S3-38a integration evidence: `cargo test -p od-host --lib --offline` (98 passed),
workspace all-target check, C ABI repeat/two-handle/STOP/destroy tests, loop
differential against legacy path and interpreter with cache-hit counters, and
signed local VMHost arithmetic/control-race/isolation/app-bundle smokes passed
on the local macOS ARM64 host. The generated-fault process smoke was intermittent:
one of three runs timed out, while two retries reported SIGSEGV; its cause is
unlocalized and it is not counted as a clean gate. See the
[S3-38a report](../reports/STAGE_3_S3-38a_2026-10-29.md). This does not close
full S3-38 or establish persistent cache reuse between calls, memory execution
through the public ABI, Mini support, or a performance benefit. The legacy
fragment path returned `CODE_END` before observing pre-run STOP when RIP already
equaled `code_end`; the integrated dispatcher follows the documented V0 STOP
contract at that boundary.
