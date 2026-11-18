# Этап 9: воспроизводимый набор проверок (S9-42)

Статус: **этап 9 НЕ принят.** Набор фиксирует, что реально выполнено локально на модели отказов и реальных файлах macOS, и явно хранит всё, что не выполнено. Локальный успех не заполняет hardware, Windows и ревью-клетки.

Связанные документы: [план этапа 9](../plans/STAGE_9.md), [отчёт](../reports/STAGE_9_REPORT.md), [STORAGE_RECOVERY.md](STORAGE_RECOVERY.md), [S9_40_COMMIT_LATENCY.md](S9_40_COMMIT_LATENCY.md), [S9_40_CHAIN_EXPORT_COMPACT.md](S9_40_CHAIN_EXPORT_COMPACT.md), [реестр проблем](../KNOWN_ISSUES.md).

## 1. Как запустить

```sh
tools/stage9/run_checks.sh
```

Скрипт выполняет команды по порядку, не останавливаясь на первой ошибке. Переменные: `STAGE9_RUN_ID` (по умолчанию дата-время), `STAGE9_BUDGET_SECONDS` (общий бюджет, 840), `STAGE9_CASE_TIMEOUT` (на case, 600), `STAGE9_LAYOUT_ERROR_BASELINE` (45). Код выхода 1 только при `failed`; `blocked` и `not_run` код выхода не меняют, поэтому «код 0» не означает принятия этапа — смотреть `summary.txt`.

## 2. Где лежат результаты

Всё в игнорируемом Git каталоге `work/stage9/<run-id>/` (правило `/work/**` в `.gitignore`; скрипт проверяет `git check-ignore` и пишет результат в `environment.txt`):

- `results.tsv` — `case`, `command`, `status`, `exit_code`, `seconds`, `evidence`, `note`; `evidence` — путь к логу внутри run-каталога, для статичных строк — документ со статусом (см. §5);
- `environment.txt` — `git rev-parse HEAD`, ветка, `git status --porcelain`, `sw_vers`, `uname`, `rustc --version`, `cargo --version`, версия python3, параметры бюджета;
- `frozen.txt` — sha256 `Cargo.toml`, `Cargo.lock`, `STORAGE_FORMAT.md`, `STAGE_9.md`, самого скрипта и `check_layout.py` (отсутствующий файл помечается `missing`);
- `logs/<case>.log` — полный вывод каждой команды; `summary.txt` — счётчики статусов.

В Git попадают только скрипт и этот документ. Большие образы и логи в Git не добавлять; перенос долговечного результата в `docs/reports/data/` — отдельным решением и без секретов.

## 3. Команды и ожидаемый результат

| Case | Команда | Ожидание |
|---|---|---|
| `fmt_check` | `cargo fmt --all -- --check` | код 0 |
| `od_storage` | `cargo test -p od-storage` | 219 passed, 1 ignored (`write_fixture`), 0 failed (2026-11-18, с матрицей S9-38 уровня 0) |
| `od_host_storage_policy` | `cargo test -p od-host --lib storage_policy` | 24 passed |
| `od_host_storage_io` | `cargo test -p od-host --lib storage_io` | 36 passed |
| `od_host_raw_file_tests` | `cargo test -p od-host --lib raw_file_tests` | 9 passed |
| `od_host_persistent_nvme` | `cargo test -p od-host --lib persistent_nvme` | 19 passed (в т. ч. S9-04: два процесса, SIGKILL); тесты с файлами сериализованы `process_spawn_guard` ([ISSUE-518](../KNOWN_ISSUES.md#issue-518-s5-persistent-nvme-reopen-test-flaky-in-parallel), закрыто) |
| `od_host_stage9_storage_kill` | `cargo test -p od-host --test stage9_storage_kill` | 2 passed, 1 ignored (дочерний режим, запускается родителем): kill процесса на реальных файлах для 9 операций (S9-38 уровень 1, [STORAGE_RECOVERY.md §4.2](STORAGE_RECOVERY.md#42-матрица-s9-38-на-уровне-1-kill-процесса-на-реальных-файлах-2026-11-18)) и порядок вызовов commit на host-файле (S9-06). Только macOS, иначе `not_run` |
| `od_image_tool` | `cargo test -p od-image-tool` | 21 passed (`tests/cli.rs`, в т. ч. `import`) |
| `workspace_check` | `cargo check --workspace --all-targets --offline` | код 0 |
| `git_diff_check` | `git diff --check` | код 0; проверяет только отслеживаемые изменения |
| `s9_40_bench_once` | `cargo build -p od-bench --release --offline --bin s9_40_storage` и `s9_40_storage <run>/s9_40_bench 1` | код 0: код замера S9-40 собирается, все сверки с моделью проходят; это не замер, цифры одного прогона не публикуются. Только macOS/aarch64, иначе `not_run` |
| `layout_check` | `python3 tools/check_layout.py` | код 1 ожидаем: известный baseline **45 ERROR** (в основном ссылки на `work/stage3-harness/...`, которых нет в чистом worktree). Число не должно расти; `passed` означает «не больше baseline», а не чистую структуру. Новые документы не добавляют markdown-ссылок на `work/` |

Не входит в набор: полный `cargo test -p od-host` — в нём известны 21 падение (`s3_20*`, `native_register_v1_corpus::reduce_saved_native_register_v1_case`), существовавшие до этапа 9 (см. отчёт §6).

## 4. Case и статусы

Словарь: `passed`, `failed`, `not_run`, `blocked`. Подсостояния `not_started` и `partial` пишутся в `note`; в `results.tsv` статус при этом `not_run`, а если остаток заблокирован зависимостью — `blocked`.

Последний полный запуск: run-id `20261118-s9-38c`, HEAD `46829f5` с изменениями среза S9-38 уровня 1 в рабочем дереве, macOS 26.7.1 (25G241, Apple Silicon), rustc/cargo 1.99.0, бюджет по умолчанию 840 с, 113 с (до правок ревью — `20261118-s9-38b`, тот же итог). Итог: 12 passed, 0 failed, 1 not_run, 7 blocked (добавлен case `od_host_stage9_storage_kill`, статичная строка storage-матрицы S9-38 убрана — её заменяют исполняемые case). Предыдущие: `20261118-s9-38a` — 11 / 0 / 2 / 7; `20261118-s9-43` — 11 / 0 / 2 / 6.

Промежуточные запуски того же дня:
- `20261118-s9-42` — бюджет 1800, до добавления S9-38 и смены статусов: 11 / 0 / 3 / 4;
- `20261118-s9-42b` — `od_host_persistent_nvme` `failed` из-за ISSUE-518;
- `20261118-s9-42c` — HEAD `3719cc9`, тот же case в один поток: 11 / 0 / 2 / 6; холодная пересборка бенчмарка в `20261118-s9-42` заняла около 43 с.

Предыдущий запуск `20261105-final` (HEAD `7c68b98`): 9 passed, 0 failed, 3 not_run, 4 blocked.

| Case | Статус | Причина / ограничение |
|---|---|---|
| `fmt_check`, `od_storage`, `od_host_storage_policy`, `od_host_storage_io`, `od_host_raw_file_tests`, `od_host_persistent_nvme`, `od_host_stage9_storage_kill`, `od_image_tool`, `workspace_check`, `git_diff_check`, `s9_40_bench_once` | `passed` | Выполнено реально, код 0. Это тесты модели отказов (`FaultBackend`), реальных локальных файлов и двух процессов; не power-loss и не guest |
| `layout_check` | `passed` (с baseline) | 45 ERROR = baseline, рост отсутствует; код выхода 1 ожидаем |
| `S9-38_vm_matrix` | `blocked` | VM-часть S9-38 заблокирована S9-37, S9-29…31, S9-13 |
| `S9-39_power_loss` | `blocked` | Испытания выполнены вне скрипта: 30 прогонов `verify ok` на USB-накопителе ([S9_39_POWER_LOSS_PLAN.md §7, §8.7](S9_39_POWER_LOSS_PLAN.md)), в том числе точки между барьерами; S9-39 принята владельцем 2026-11-18 в объёме этого носителя. Строка остаётся `blocked` для целевой комбинации (внутренний SSD не испытан, барьерная семантика не доказана). Kill процесса и fault-инъекция стенд не заменяют (G5) |
| `windows_vtpm_uefi` | `blocked` | Нет Windows guest, UEFI variables store (этап 7, S7-09/S7-10) и vTPM (S9-26/27/28) |
| `linux_mini_guest` | `blocked` | Реальный Linux guest с диском этого этапа не запускался: образ этапа 9 гостю не подключён (S9-13 `blocked`) |
| `real_guest_lifecycle` | `blocked` | S9-28…S9-37: зависят от компонентов этапов 4–8 и [ISSUE-056](../KNOWN_ISSUES.md#issue-056-s9-cpu-state-not-single-owner) |
| `S9-22_migration` | `not_run` | Закрыта в объёме v1 (решение владельца 2026-11-18): путь RAW → CoW v1 через `import` покрыт `od_storage` и `od_image_tool`; миграции между версиями CoW нет — второй версии нет |
| `S9-40_full` | `blocked` (`partial`) | Storage-замеры сделаны и воспроизводимы (`s9_40_bench_once`, [S9_40_CHAIN_EXPORT_COMPACT.md](S9_40_CHAIN_EXPORT_COMPACT.md)); пауза/длительность checkpoint и restore заблокированы S9-29…31; `not_run`: фрагментация, реальные размеры диска, независимый повтор |
| `S9-43_independent_review` | `blocked` | Итоговое ревью storage-scope выполнено Claude Opus high и засчитано владельцем 2026-11-18 (отчёт §10); строка остаётся `blocked`, потому что ревью полной VM невозможно: S9-38 (VM-часть), S9-39 (целевой носитель) и S9-23…37 не закрыты. Итоговое ревью записывается в [отчёт](../reports/STAGE_9_REPORT.md); скрипт его не заменяет и `passed` не ставит |

Не включено в скрипт как отдельные case: длинный soak S9-41 (8 seeds × 12000 циклов, `OD_SOAK_CYCLES`/`OD_SOAK_SEED`) — выполнен отдельно, лог в [docs/reports/data/stage9](../reports/data/stage9/S9_41_soak_2026-11-18.log); в `od_storage` входит только короткий soak по умолчанию; optional S9-34 (guest coordination) не имеет выполненных provider tests и из claim исключается.

## 5. Правила чтения результата

- `passed` относится только к команде в строке. Он не закрывает gate G1–G6.
- Любой `blocked`/`not_run` mandatory case остаётся видимым в `results.tsv` и в этом документе; статичные строки скрипт не переопределяет по результату локальных команд.
- Для воспроизведения сохранять `frozen.txt` и `environment.txt` рядом с результатом; сравнение прогонов без совпадающего HEAD и `Cargo.lock` недействительно.
- Повторный запуск создаёт новый run-id; старые каталоги `work/stage9/` не удалять и не перезаписывать. Скрипт отказывает (код 2), если каталог run-id уже существует.
- У статичных строк поле `evidence` указывает документ, где записан статус.
- Пределы механики: бюджет проверяется только перед стартом case (фактический предел — бюджет плюс таймаут одного case); SIGALRM завершает только процесс, запущенный через `perl exec`, — дочерние процессы `cargo` или бенчмарка после таймаута могут продолжить работу.
