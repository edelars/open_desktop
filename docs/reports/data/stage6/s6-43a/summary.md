# S6-43a: probe cross-modifying code на двух vCPU с JIT (данные)

Дата: 2026-11-18. Host: Apple M3 Max (ARM64), одна машина. Тесты: `crates/od-host/src/paged_host_loop/tests/s6_43a.rs`.

Файлы (текст, вывод `eprintln!` тестов и строки `test`):

- `fast-debug.txt` — быстрая форма (debug, `cargo test -p od-host --offline --lib -- s6_43a --nocapture`): чередование 600 сигналов и параллельный режим 2000 сигналов на каждый из трёх протоколов, сверка с интерпретатором.
- `long-parallel-release.txt` — длинная форма (release, `#[ignore]`, 2 000 000 сигналов на протокол, 6 000 000 всего, 81 с): `cargo test --release -p od-host --offline --lib -- --ignored s6_43a_long_parallel_run --nocapture` (переменная `OD_S6_43A_CYCLES` меняет число).
- `mutation-reusable-debug.txt` — контрольная мутация: из `reusable` (`PagedFetchedLoadSession::execute_forms_with_stop`, `crates/od-host/src/lib.rs`) убраны сверка поколения и сверка байт. Ловится протоколом `host-wait (block kept)` в обоих режимах (чередование: stale 600, wrong 300 из 1800 запусков X; параллельный режим: stale 2000, wrong 1000 из 6000; блок компилируется один раз вместо одного на цикл; stale — запуск после сигнала исполнил код до записи, wrong — запуск до записи дал результат не той версии, потому что застрявший блок не менялся). Код восстановлен.
- `weak-host-ordering-release-200000.txt` — прогон параллельного режима с `--features od-machine/weak-host-ordering` (порядок `Relaxed` вместо `SeqCst`, поколение страницы без `Release`), 200 000 сигналов на протокол: устаревших запусков 0 (мутация не ловится).

Протоколы (`PROTOCOLS`): `guest-wait+serialize` (B ждёт сигнал guest-загрузкой, затем шаг, кончающий блок (`CLD`; по SDM не сериализующая), затем изменённый код), `guest-wait` (без шага, кончающего блок), `host-wait (block kept)` (сигнал читается `SeqCst`-загрузкой хоста без шага guest, блок изменяемой инструкции остаётся допущенным).
