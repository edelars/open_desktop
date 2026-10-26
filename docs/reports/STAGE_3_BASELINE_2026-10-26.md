# Этап 3 — baseline preflight, 26 октября 2026 года

**Статус:** S3-01 не завершена. Методика и workload matrix подготовлены; обязательная проверка Stage 1 остановилась на локальном preflight. Performance baseline не измерялся.

## Проверенная ревизия

Проверялся точный снимок `9a1376be294e7c626a62ca5ae50d29f9afdd3bd0` (`9a1376b`), source fingerprint `sha256:25a71e913d6084be9f805be39b77e56563a766e706c1215a12ae9c78b9fb6cfa`. Изолированный worktree не содержал незакоммиченных изменений. Этот результат относится только к данной ревизии; параллельные и последующие изменения этапа 2 в нём не проверены.

## Локальный preflight

Команда:

```sh
python3 tests/cpu/stage1_harness.py --suite local --generated-count 1000 --seed 0x5eedcafe --repeats 2
```

Harness run `stage1-harness-20261026-174338-1ac20fc0` завершился со статусом `failed`. `cargo fmt --all -- --check` и `cargo check --workspace --all-targets --offline` прошли. Целевые Cargo tests сообщили 703 passed и 5 failed:

- `execute::tests::fabs_rejects_missing_or_invalid_source_and_boundaries_atomically`
- `execute::tests::fchs_rejects_invalid_source_and_execution_boundaries_atomically`
- `execute::tests::fst_and_fstp_sti_reject_noncanonical_specials_and_bad_infinity_environment_atomically`
- `long_decode::tests::fcomi_family_decodes_only_the_32_bare_indexed_register_forms`
- `long_decode::tests::fninit_accepts_only_bare_db_e3`

В фактическом выводе новые x87 формы были приняты там, где эти тесты ожидали `Unsupported`. Это наблюдение о проверенной ревизии, а не вывод о некорректности новой семантики: тестовые ожидания и реализации этапа 2 находятся в параллельной работе.

## Oracle preflight

Использовались те же seed, generated count и repeats. Первый запуск против старого checkout `/home/senya/projects/open_desktop-stage1` остановился до выполнения тестов из-за несовпадения fingerprint: локальный `25a71e…` против удалённого `811d78c15ee5392bac914fde5d1069c8f18b64fbd1acf7ac264e2baa448e19f7`.

Чтобы не изменять старый checkout, для второго запуска в отдельном каталоге Mac mini была распакована архивная копия ровно проверенной ревизии. Fingerprint совпал; generated corpus имел SHA-256 `9356e916b2037a3f5a082e8366fe33bf4277ab00812489809bfc586210744d35`. Удалённые `od-isa --lib` tests завершились ошибкой до запуска native fixtures. Сохранённый manifest не содержит имён удалённых failing tests, поэтому совпадение с локальным набором пяти случаев не подтверждено. Временная копия удалена. Oracle results для этого baseline отсутствуют.

## Ограничения и следующий шаг

- Физическое исполнение fixtures не состоялось; сравнения interpreter/native и VMHost suite не выполнены.
- Cold process, cold compile-plus-first-execution и warm performance samples не измерялись. Нельзя заявлять ускорение или использовать legacy fragment numbers как baseline block JIT.
- После того как параллельные изменения этапа 2 стабилизируют поведение и соответствующие ожидания тестов, повторить local и oracle suites на одном зафиксированном fingerprint. Отдельно сохранить успешные локальные результаты и physical oracle evidence; затем измерять только доступные на этой ревизии workloads.

Сырые harness outputs и manifest сохранены локально вне Git в `work/stage1-harness/` и `work/stage1-oracle/`; они не являются долговременным доказательством успешного oracle.
