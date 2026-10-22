# Этап 1 — отчёт приёмки основы проверки корректности

**Дата:** 22 октября 2026 года. **Статус:** S1-00–S1-16 выполнены для заявленного register-only профиля S0-09. Этап подтверждён на M3 Max и единственном доступном физическом x86 host — Mac mini с Ubuntu.

**Повторное ревью:** исправлены семь дефектов проверки входов, сохранения результатов, состава corpus, строгого JSON и reducer. Обновлённая единая команда прошла 10 ручных + 1000 generated cases × 2 повтора, 2020 native results, без расхождений. Подробности и актуальный fingerprint — в [отчёте ревью](STAGE_1_REVIEW_2026-10-22.md) и [review evidence](data/stage1/review-2026-10-22.json). Данные ниже сохраняют исходную приёмку; её generated-прогон выполнялся отдельно от ручного набора.

## Объём доказательства

Проверен ограниченный профиль из `MOV r64, imm64`, `ADD/SUB/CMP r64, r64`, `JZ/JNZ rel8` для RAX/RCX/RDX/RBX и определённых CF/PF/AF/ZF/SF/OF. В него входят четыре исходных S0 fixtures и шесть вручную составленных граничных cases. Генератор `od-testkit-cpu-v1` добавил ещё 1000 уникальных cases по seed `0x5eedcafe`; программы используют линейные операции и forward conditional branches без loops. Каждый из этих cases исполнен дважды на обеих машинах.

Физическая конфигурация: Ubuntu 25.10, Linux `6.17.0-41-generic`, x86_64, Intel Core i5-3210M (family 6/model 58/stepping 9, microcode `0x21`), page size 4096 bytes, Rust/Cargo 1.99.1. ARM host: macOS 26.7.1, Darwin 25.6.0, Apple M3 Max, page size 16384 bytes, Rust/Cargo 1.99.0. Единственный физический x86 host проверен; на AMD и иных Intel microarchitectures результат не распространяется.

## Результат запуска

Итоговая команда:

```sh
python3 tests/cpu/stage1_harness.py \
  --suite all --generated-count 1000 --seed 0x5eedcafe --repeats 2
```

Итог `passed`: локальный fmt/workspace check/целевые tests/SHA-256 fixtures, удалённые Linux tests, обязательный CPU oracle и VMHost process/socket scenarios. Oracle сохранил 2000 records для 1000 уникальных generated case IDs (1000 на каждый повтор); суммарно выполнено 6000 engine runs (interpreter, JIT, native x86), ноль mismatches, ноль failed/unsupported/skipped cases. Все повторные native snapshots совпали.

Fixture/result schemas — v1/v1. Оба runner были собраны по одному source fingerprint: `sha256:e40d446ba553031165bd8bfab414ef4343b864d1bf59eb4d7bb507107ed925f2`. Seeded corpus содержит 1000 валидных JSON fixtures; SHA-256 конкатенации их точных JSON bytes: `9356e916b2037a3f5a082e8366fe33bf4277ab00812489809bfc586210744d35`.

Итоговый harness summary: `work/stage1-harness/stage1-harness-20261022-193139-96d1916a/summary.json` (SHA-256 `af3aea0bc282d14da21ebf2332fedd6073bbd924486088e33f2818eed5b39b78`). Полный oracle manifest и отдельные snapshots/comparator reports: `work/stage1-oracle/stage1-20261022-193143-3244fc20/manifest.json` (SHA-256 `5c12b9bae4bf76777468b5dbde4a5618ef54435bd190921f15a9dba70db96a5a`). Компактные долговечные поля проверки продублированы в [evidence JSON](data/stage1/acceptance.json).

## Выполненные проверки

- На M3 Max: `cargo fmt --all -- --check`, `cargo check --workspace --all-targets --offline`, `cargo test --offline -p od-isa -p od-testkit -p od-runner -p od-x86-runner`, SHA-256 fixture manifest. Целевые crates прошли все 48 локальных unit tests.
- На Mac mini: тот же набор Cargo tests на Linux/x86-64 — 53 tests, включая RLIMIT/watchdog/process supervision, guard-page decode boundary, отдельные SIGSEGV/SIGILL probes и generator encoder. Установка Rust и отдельный checkout выполнены по разрешению владельца.
- Физический oracle: 10 вручную составленных fixtures × 2 повтора и 1000 generated fixtures × 2 повтора. Для каждого результата проверены digest, schema, profile и build fingerprint; сравнивались объявленные GPR, RIP, masked RFLAGS и exit reason. Все comparisons и повторные native snapshots совпали.
- Отрицательный путь: comparator tests обнаруживают подменённый RAX и некорректные schema/digest/profile, проверяют undefined flags и недоступные обязательные поля; supervisor tests ловят timeout, signal, повреждённый JSON, закрытые каналы и потомка с унаследованным pipe. Отсутствующий fixture дал exit 1 и failed manifest; CLI minimizer для корректного fixture без реального mismatch завершился exit 2, не выдавая ложный успех.
- Редуктор проверен на test-only подмене GPR в `RunResult`: comparator сохраняет класс `gpr.rax`, reducer удаляет инструкции и возвращает валидный меньший fixture. Реального CPU mismatch в корпусе не найдено.
- VMHost regression: control-race smoke прошёл сценарии раннего STOP, partial/chunked frames, malformed requests и disconnect на реальном process/socket path.
- Единая команда `tests/cpu/stage1_harness.py` создаёт machine-readable summary; обязательный oracle suite возвращает ошибку при проблеме окружения, build или сравнений. Невыбранные наборы явно указаны как `not_run`.

## Границы и следующий объём

Stage 1 не реализует полную x86 ISA, guest OS, guest virtual memory/MMU, memory operands, cross-page guest loads/stores, architectural exception vectors, privileged mode, SIMD/x87, AMD compatibility или boot Linux/Windows. Linux guard-page fault и SIGILL проверки валидируют средства host observation; они не выдаются за guest exception semantics. Native retired-instruction count остаётся unavailable. Generated corpus использует один seed и текущий allowlist, не является статистическим покрытием ISA. MCP отложен: реальные сценарии покрывают CLI и JSON harness.

Stage 0 всё ещё не принят по отдельной проверке режима поставки: Developer ID signing/notarization остаются открытыми в ISSUE-004. S0-12/E03 перенесён из Stage 0 и теперь закрыт для оговорённого register-only профиля; это не закрывает проверку поставки. Для будущей memory/exception coverage потребуется отдельный этап с guest address translation и архитектурно точными fault observations.
