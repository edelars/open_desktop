# Ревью этапа 1 — 22 октября 2026 года

Проверены текущие незакоммиченные изменения этапа 1, план S1-00–S1-16, формат fixture/result, comparator, generator/reducer, CLI, Linux native trampoline/supervisor и Python/control orchestration. Исходные изменения сохранены. Производственная CPU-семантика и JIT в ходе ревью не менялись.

## Найденные и исправленные дефекты

| Приоритет | Компонент | Дефект и последствия | Исправление и проверка |
|---|---|---|---|
| P1 | `stage1_oracle.py`, идентичность входов | `case_id` использовался в пути до проверки Rust loader. Некорректный ID мог создать каталог вне каталога результатов. Совпавшие basename при упаковке перезаписывали удалённый вход; одинаковые ID конфликтовали в результатах. | До SSH проверяются допустимые символы ID, уникальность ID/basename, размер JSON и расположение файла. Отдельные regression tests отклоняют traversal и оба вида дубликатов. Некорректная структура `program` становится диагностируемой ошибкой. |
| P1 | `stage1_oracle.py`, доказательства сбоя | Удалённый batch прекращался на первом ненулевом exit и терял JSON/диагностику, включая уже выполненные cases. Локальные сравнения читали исходные файлы повторно, без сохранения точного неизменного входа. | Входы копируются побайтно до сборки; manifest получает digest, seed и команду повторения. Batch возвращает stdout/stderr/exit каждого процесса; все результаты сохраняются до сообщения об ошибке. Сохраняется и сырой ответ SSH. Regression с test-only timeout и следующим успешным case подтверждает сохранение обоих. |
| P2 | `stage1_harness.py` | `--generated-count` заменял 10 ручных fixtures сгенерированным корпусом, хотя документирован как добавление. Единая команда приёмки теряла ручные границы и цикл SUB/JNZ. | Ручные fixtures включаются вместе с corpus. Regression проверяет аргументы запуска; реальный прогон подтвердил 1010 уникальных cases. |
| P2 | `od-testkit`, JSON scopes/observations | Tagged enums молча игнорировали неизвестные поля: например, `unsupported` с `regions` или `available` с `reason`. Это нарушало строгий v1 contract и скрывало противоречивые данные. | Добавлен строгий разбор observations; scopes используют пустые struct-варианты при десериализации, поскольку tagged unit-варианты serde игнорируют добавочные поля. Два regression tests воспроизвели дефект до исправления и проходят после него. |
| P2 | `od-testkit::minimize` | Исходный mismatch проверялся дважды, но каждый меньший кандидат — только один раз. Reducer мог вернуть flaky пример вместо воспроизводимой ошибки. | Каждый принимаемый кандидат должен дважды воспроизвести тот же класс. Тест, в котором кандидаты дают mismatch только при первом вызове, теперь сохраняет исходный стабильный пример. |
| P2 | `od-testkit::minimize` | При удалении инструкций до ненулевого entry RIP переносились branch targets, но не entry. Новое значение RIP указывало на другую инструкцию или внутрь инструкции. | Entry переносится по тем же сохранённым границам; regression из трёх MOV проверяет удаление первого MOV при входе во второй. |
| P2 | `od-testkit::minimize` | Публичная функция вызывала callback дважды даже при `max_attempts=0` или `1`, нарушая свой предел. | Такие лимиты отклоняются до callback. Подтверждение кандидата начинается только при наличии бюджета на оба запуска; счётчик отражает реальные вызовы. |

Также исправлено описание CPU limit: fixture CLI применяет 2 s, тогда как reusable supervisor допускает максимум 30 s. Уточнены текущий статус формата и поведение reducer.

## Проверки

Четыре первоначальных Rust regressions упали на исходной реализации: скрытые поля scopes, противоречивые observations, flaky кандидат и слишком малый attempt limit. После исправления `od-testkit` проходит 31 test. Локальный набор теперь также явно запускает Python orchestration regressions; финальный локальный прогон прошёл все 6 Python tests.

Полная команда:

```sh
python3 tests/cpu/stage1_harness.py \
  --suite all --generated-count 1000 --seed 0x5eedcafe --repeats 2
```

Итоговый run `stage1-harness-20261022-194809-4ea0d620` прошёл `local`, `oracle`, `vmhost`. Физический manifest `stage1-20261022-194814-8fc2b763` содержит 1010 уникальных входов: 10 ручных + 1000 generated, 2 повтора, 2020 native snapshots и 6060 исполнений трёх backends. Все interpreter/native, JIT/native и повторные native comparisons совпали; failed/unsupported/skipped cases отсутствуют. Оба runner собраны с fingerprint `sha256:0a7a30de39bf7e53c864c6a057508e405b5e1363790d9354748fa10a4e1889fe`.

Конфигурация та же, что в [отчёте приёмки](STAGE_1_REPORT.md): Apple M3 Max/macOS 26.7.1/Rust 1.99.0 и физический Intel i5-3210M/Ubuntu 25.10/Rust 1.99.1. На мини переданы только три исправленных файла testkit после явного разрешения владельца. Все Linux Cargo tests и реальные process/socket сценарии VMHost прошли. Проверены форматирование, workspace check, локальные целевые Rust tests (53), SHA-256 fixtures и `tools/check_layout.py`. После последнего уточнения обработки некорректного JSON отдельно повторён local suite: `stage1-harness-20261022-194937-9d4955e2`.

Малое долговечное доказательство: [review evidence](data/stage1/review-2026-10-22.json). Полные входы, snapshots, diagnostics, comparison reports и manifests находятся в `work/stage1-oracle/stage1-20261022-194814-8fc2b763/`; логи полного harness — в `work/stage1-harness/stage1-harness-20261022-194809-4ea0d620/`.

`graphify update .` выполнен: 1184 nodes, 2363 edges. Инструмент сообщил о неполном разборе `include/open_desktop/host_abi.h`; это ограничение графа зарегистрировано как ISSUE-010 в [реестре](../KNOWN_ISSUES.md), а не как доказанная ошибка C header. Файлы manifests и пустые scaffolds без извлечённых символов сами по себе не являются дефектами реализации. Локальные Markdown-ссылки изменённой документации, SHA-256 evidence и `git diff --check` проверены.

## Границы вывода

Ревью и повторный прогон подтверждают только register-only профиль S0-09 на одном Intel CPU и M3 Max. Полная ISA, guest memory/MMU, guest exceptions, другие x86 microarchitectures и Developer ID distribution не проверены этим результатом. Реальный CPU mismatch не обнаружен; негативные regressions используют явно тестовые данные. Автоматический CLI reducer остаётся локальным interpreter/JIT reducer; физическое сравнение воспроизводится отдельно. Это существующая граница первого профиля, а не доказательство автоматического сокращения physical-only mismatch.
