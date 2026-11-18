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

## Повтор на исходниках 3476780, 18 ноября 2026

Проверяемая ревизия `3476780`. Source fingerprint официального oracle:
`f3a3a351d5729f362a670a6893fd43a8a149d775c989d8d8c333892135e1d1a1`.
Локальный harness `stage1-harness-20261118-215920-57e8d0e1` завершился
успешно на macOS 26.7.1 ARM64 с Rust 1.99.0. Прошли форматирование,
workspace check, 7 586 Rust tests (5 ignored), 25 Python tests harness и
проверка SHA-256 16 фикстур.

```sh
python3 tests/cpu/stage1_harness.py --suite local \
  --generated-count 1000 --seed 0x5eedcafe --repeats 2
python3 tests/cpu/stage1_harness.py --suite oracle \
  --remote-root /path/to/matching/checkout \
  --generated-count 1000 --seed 0x5eedcafe --repeats 2
```

Локальный preflight на `s0_09_add_overflow`
(`input_sha256=12e43c2a15e5b41e868092fd9223c1e4ff888da49d6b39a1474ce8e362d2a729`)
сверил interpreter, legacy fragment, one-block JIT и dispatcher. Во всех
случаях совпали полный snapshot, `reached_code_end` и одна выполненная
инструкция. Команды и границы измерений описаны в [BENCHMARKS](../testing/BENCHMARKS.md#jit-этапа-3-workload-и-уровни-прогрева).
Раздельные cold и warm серии для одного пятислучайного register-only корпуса
сохранены в [S3-56](STAGE_3_CORE_REPORT.md#9-s3-56--измерения-generic-пути).
Каждая из шести серий содержит 20 пар с одинаковым sweep count внутри пары
и минимумом 200 ms измеренного времени для каждой стороны. Это замеры на
исторических исходниках, указанных в отчёте линии.

[S2-09](STAGE_2_REPORT.md#s2-09--native-v2-и-физическая-проверка) принят
для трёх положительных v2 MOV/memory cases с физическим сравнением
interpreter/native на своём fingerprint. В нём подтверждены 16 GPR, RIP,
определённые флаги и fixture memory. Тройное сравнение memory JIT относится
к S3-21 и остаётся отдельной проверкой.

### Физический preflight

Исходники на Linux x86_64 совпали по fingerprint. На всех попытках
использовались seed `0x5eedcafe`, 1 000 generated cases и два
запрошенных повтора. Ни одна из перечисленных попыток не дошла до
физического сравнения.

| Oracle run | Наблюдение |
| --- | --- |
| `stage1-20261118-215524-3c8bf74d` | Первый минимальный архив не включал 192 compile-time test fixtures. Preflight остановился на их отсутствии. |
| `stage1-20261118-215739-b41e8f80` | Дополненный архив собрал тесты. Preflight достиг лимита 1 800 с во время `od-runner` tests. |
| `stage1-20261118-222826-c967f904` | Повтор с `opt-level=1` только для профиля Cargo test достиг того же лимита на холодной компиляции. |

Предел и возобновление проверки отслеживаются в
[ISSUE-519](../KNOWN_ISSUES.md#issue-519). S3-01 остаётся открытой до
результата физического oracle. Идентификаторы соответствуют каталогам
`work/stage1-oracle/`, где сохранены manifest и логи preflight.
