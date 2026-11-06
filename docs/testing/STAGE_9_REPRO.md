# Этап 9: воспроизводимый набор проверок (S9-42)

Статус: **этап 9 НЕ принят.** Набор фиксирует, что реально выполнено локально на модели отказов и реальных файлах macOS, и явно хранит всё, что не выполнено. Локальный успех не заполняет hardware, Windows и ревью-клетки.

Связанные документы: [план этапа 9](../plans/STAGE_9.md), [отчёт](../reports/STAGE_9_REPORT.md), [STORAGE_RECOVERY.md](STORAGE_RECOVERY.md), [S9_40_COMMIT_LATENCY.md](S9_40_COMMIT_LATENCY.md), [реестр проблем](../KNOWN_ISSUES.md).

## 1. Как запустить

```sh
tools/stage9/run_checks.sh
```

Скрипт выполняет команды по порядку, не останавливаясь на первой ошибке. Переменные: `STAGE9_RUN_ID` (по умолчанию дата-время), `STAGE9_BUDGET_SECONDS` (общий бюджет, 840), `STAGE9_CASE_TIMEOUT` (на case, 600), `STAGE9_LAYOUT_ERROR_BASELINE` (45). Код выхода 1 только при `failed`; `blocked` и `not_run` код выхода не меняют, поэтому «код 0» не означает принятия этапа — смотреть `summary.txt`.

## 2. Где лежат результаты

Всё в игнорируемом Git каталоге `work/stage9/<run-id>/` (правило `/work/**` в `.gitignore`; скрипт проверяет `git check-ignore` и пишет результат в `environment.txt`):

- `results.tsv` — `case`, `command`, `status`, `exit_code`, `seconds`, `evidence`, `note`; `evidence` — путь к логу внутри run-каталога;
- `environment.txt` — `git rev-parse HEAD`, ветка, `git status --porcelain`, `sw_vers`, `uname`, `rustc --version`, `cargo --version`, версия python3, параметры бюджета;
- `frozen.txt` — sha256 `Cargo.toml`, `Cargo.lock`, `STORAGE_FORMAT.md`, `STAGE_9.md`, самого скрипта и `check_layout.py` (отсутствующий файл помечается `missing`);
- `logs/<case>.log` — полный вывод каждой команды; `summary.txt` — счётчики статусов.

В Git попадают только скрипт и этот документ. Большие образы и логи в Git не добавлять; перенос долговечного результата в `docs/reports/data/` — отдельным решением и без секретов.

## 3. Команды и ожидаемый результат

| Case | Команда | Ожидание |
|---|---|---|
| `fmt_check` | `cargo fmt --all -- --check` | код 0 |
| `od_storage` | `cargo test -p od-storage` | 201 passed, 0 failed (на `7c68b98`) |
| `od_host_storage_policy` | `cargo test -p od-host --lib storage_policy` | 20 passed |
| `od_host_storage_io` | `cargo test -p od-host --lib storage_io` | 30 passed |
| `od_host_raw_file_tests` | `cargo test -p od-host --lib raw_file_tests` | 9 passed |
| `od_image_tool` | `cargo test -p od-image-tool` | 16 passed (`tests/cli.rs`) |
| `workspace_check` | `cargo check --workspace --all-targets --offline` | код 0 |
| `git_diff_check` | `git diff --check` | код 0; проверяет только отслеживаемые изменения |
| `layout_check` | `python3 tools/check_layout.py` | код 1 ожидаем: известный baseline **45 ERROR** (в основном ссылки на `work/stage3-harness/...`, которых нет в чистом worktree). Число не должно расти; `passed` означает «не больше baseline», а не чистую структуру. Новые документы не добавляют markdown-ссылок на `work/` |

Не входит в набор: полный `cargo test -p od-host` — в нём известны 21 падение (`s3_20*`, `native_register_v1_corpus::reduce_saved_native_register_v1_case`), существовавшие до этапа 9 (см. отчёт §6).

## 4. Case и статусы

Словарь: `passed`, `failed`, `not_run`, `blocked`. Подсостояния `not_started` и `partial` пишутся в `note`, в `results.tsv` статус при этом `not_run`.

Первый полный запуск: run-id `20261105-final`, HEAD `7c68b98`, macOS 26.7.1 (Apple Silicon), rustc/cargo 1.99.0, 18 с. Итог: 9 passed, 0 failed, 3 not_run, 4 blocked. Рабочее дерево в момент запуска содержало только неотслеживаемые `docs/plans/.claude/`, `docs/plans/CLAUDE.md` и `tools/stage9/`.

| Case | Статус | Причина / ограничение |
|---|---|---|
| `fmt_check`, `od_storage`, `od_host_storage_policy`, `od_host_storage_io`, `od_host_raw_file_tests`, `od_image_tool`, `workspace_check`, `git_diff_check` | `passed` | Выполнено реально, код 0. Это тесты модели отказов (`FaultBackend`) и реальных локальных файлов; не power-loss и не guest |
| `layout_check` | `passed` (с baseline) | 45 ERROR = baseline, рост отсутствует; код выхода 1 ожидаем |
| `S9-39_power_loss` | `blocked` | Нет разрешения на стенд power-loss. Kill процесса и fault-инъекция его не заменяют (G5) |
| `windows_vtpm_uefi` | `blocked` | Windows guest, UEFI firmware и vTPM отсутствуют (S9-26/27/28) |
| `linux_mini_guest` | `not_run` | Реальный Linux guest с диском этого этапа не запускался; нет устройства хранения гостя (S9-13) |
| `real_guest_lifecycle` | `blocked` | S9-28…S9-37: зависят от компонентов этапов 4–8 и [ISSUE-056](../KNOWN_ISSUES.md#issue-056-s9-cpu-state-not-single-owner) |
| `S9-22_migration` | `not_run` (`not_started`) | Нет второй версии формата; формат не принят как нормативный |
| `S9-40_full` | `not_run` (`partial`) | Выполнена только commit-latency ([S9_40_COMMIT_LATENCY.md](S9_40_COMMIT_LATENCY.md)); throughput, чтение, write amplification, recovery и память RAW против CoW не измерены |
| `S9-43_independent_review` | `blocked` | Независимое ревью через субагент заблокировано классификатором; `passed` не ставится |

Не включено в скрипт как отдельные case: seeded soak и repeated recovery (S9-41) входят в `od_storage` только как тесты на модели; optional S9-34 (guest coordination) не имеет выполненных provider tests и из claim исключается.

## 5. Правила чтения результата

- `passed` относится только к команде в строке. Он не закрывает gate G1–G6.
- Любой `blocked`/`not_run` mandatory case остаётся видимым в `results.tsv` и в этом документе; статичные строки скрипт не переопределяет по результату локальных команд.
- Для воспроизведения сохранять `frozen.txt` и `environment.txt` рядом с результатом; сравнение прогонов без совпадающего HEAD и `Cargo.lock` недействительно.
- Повторный запуск создаёт новый run-id; старые каталоги `work/stage9/` не удалять и не перезаписывать.
