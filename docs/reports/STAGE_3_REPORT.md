# Stage 3 — локальная bounded acceptance сводка

**Статус:** локальные bounded slices имеют отдельные evidence; Stage 3 не
завершён. Этот документ фиксирует доступный локальный результат и оставшиеся
обязательные gates, но не заменяет финальную приёмку S3-41.

## Зафиксированное локальное evidence

Последний report-inclusive локальный harness run:

- Run: `stage3-20261030-054347-8d2c65aebe`, `local_only`, macOS 26.7.1 ARM64.
- Исходная ревизия harness evidence: `fa80ab0da77b3de268c6adf649dc2a92de9dde20`.
- Fingerprint начала и конца: `caa5eec7590595b66910ce855e6c6eba4b476b59c1f7704619b273eb10e970c9`.
- Результат: 31 обязательный шаг прошёл, 0 завершился ошибкой; 33 шага других
  suites остались `not_run`. Это локальный результат на указанном fingerprint,
  а не проверка более поздних изменений.
- Stage 1 dispatcher child: 10 ручных + 1,000 generated input, 2 повтора
  (2,020 результатов); evidence остаётся `local_only`.
- Memory corpus: 3,600 уникальных входов ×2; отдельные named fault, STOP,
  budget и continuation случаи; точный S3-34h gate прошёл.
- Lifecycle/acceptance включает выбранные регрессии для cache/lifetime,
  invalidation, V3 и direct V2 PC maps, event/STOP, demand flags, coordinator
  и V0 ABI. Перечень точных identities и argv находится в исходном run summary.
- Input manifest SHA-256:
  `9860c7e0c453fe98dee086866ab086cdc840cee1fa632f133f1077d857339048`.
- Summary SHA-256:
  `b9c26b9eed6d42791e6ebfa26a8e3a8347fe6785eb137f2be3cc63d16a44fbdc`.
- Команда: `python3 tests/cpu/stage3_harness.py --suite local`.
- Игнорируемые локальные evidence хранятся в
  `work/stage3-harness/stage3-20261030-054347-8d2c65aebe/`; run manifest и
  summary намеренно не копируются в Git.

## Карта требований и границ

| Область | Локально подтверждено | Статус и граница |
|---|---|---|
| Block dispatch и bounded execution | S3-34f local acceptance и S3-34h report-inclusive harness; dispatcher corpus запускается отдельно как `local_only` | Это ARM64/local correctness evidence; не physical x86 equivalence и не полный Stage 3 workload. См. [S3-34f](STAGE_3_S3-34f_2026-10-30.md), [S3-34h](STAGE_3_S3-34h_2026-10-30.md). |
| Checked memory lowering и ordered callbacks | Узкие принятые V2 MOV singleton/pair profiles; три sequence в S3-34h generated corpus и replay/reducer | Не общий memory compiler, MMIO/atomic path или production MMU/owner integration. Текущий профиль описан в [JIT contract](../specs/JIT_CONTRACT.md), [S3-20cl](STAGE_3_S3-20cl_2026-10-28.md) и [S3-20cm](STAGE_3_S3-20cm_2026-10-29.md). |
| Code invalidation, lifetime и state attribution | Fixture-bounded S3-23/24/25; S3-27c V3 и S3-27d direct singleton V2 PC maps в acceptance harness | Не общий active-code retirement/SMP reclamation, произвольное host-fault recovery или production committed-writer ownership. См. [S3-27d](STAGE_3_S3-27d_2026-10-30.md) и разделы S3-27 в [JIT contract](../specs/JIT_CONTRACT.md). |
| Events, STOP, flags и coordinator | Точные локальные S3-28 event/STOP, demand-flags и S3-29e coordinator tests прошли в harness | Latency/default choice и измеренная польза не установлены. См. [S3-34f](STAGE_3_S3-34f_2026-10-30.md). |
| VMHost/C ABI | S3-38a register-only V0 ABI integration и S3-38b process-suite evidence | Сценарии partial; ISSUE-027 остаётся открытой из-за наблюдавшегося timeout. Новый process suite после установленного основания не запускался. См. [S3-38b](STAGE_3_S3-38b_2026-10-30.md), [реестр проблем](../KNOWN_ISSUES.md) и [диагностику](ISSUE_027_DIAGNOSTIC_2026-10-30.md). |
| Harness/replay/reducer | Fixed seeds, repeated corpus, callback-aware comparison, saved-case validation, reducer preservation и negative self-checks | Подтверждает только внесённые профили и локальный ARM64 runner. Полный S3-34 и physical comparison остаются открытыми. См. [S3-34h](STAGE_3_S3-34h_2026-10-30.md). |

## Оставшиеся gates

| Gate | Статус |
|---|---|
| Тройное сравнение interpreter/JIT/physical x86 для обязательных принятых профилей (S3-21) | Не выполнено; Mac mini проверка отложена до конца локальной работы Stage 3. |
| S3-18d1 dispatch/cache cold-sweep observation | Выполнены три локальных release runs на одном frozen corpus; это partial evidence, не full cold compile+first-execution comparison. См. [отчёт](STAGE_3_S3-18d1_2026-10-30.md). |
| Полные cold/warm measurements, churn/resources и решения о default (S3-35…37, S3-40) | Не выполнены. `od-bench cold-first-run` реализован; две попытки завершились до отчёта из-за interpreter batch короче 200 ms, вторая — после paired calibration. Теперь short measured pair сбрасывает partial series и увеличивает общий sweep count. Валидных результатов и основания для speedup/default выбора нет. |
| Production RAM/backing/mapping/MMU owner и committed-writer integration | Не входит в текущий local fixture scope; не реализован этим отчётом и не засчитывается как локальное доказательство. |
| ISSUE-027 | Открыта: прежний timeout не локализован; три диагностических наблюдения не установили причину или исправление. Не повторять suite без нового основания. |
| M1 Pro (S3-39) | Не подтверждён этим run; проверка остаётся отдельной. |
| Итоговая приёмка S3-41 | Открыта. Stage 3 нельзя объявлять завершённым по этой локальной сводке. |

## Источники

- [План Stage 3](../plans/STAGE_3.md) и [дорожная карта](../ROADMAP.md#этап-3-первый-jit-x86--arm64).
- [JIT contract](../specs/JIT_CONTRACT.md) и [JIT testing](../testing/JIT.md).
- [S3-27d direct V2 PC maps](STAGE_3_S3-27d_2026-10-30.md),
  [S3-34f local gates](STAGE_3_S3-34f_2026-10-30.md),
  [S3-34h memory corpus](STAGE_3_S3-34h_2026-10-30.md),
  [S3-38b VMHost process suite](STAGE_3_S3-38b_2026-10-30.md) и
  [ISSUE-027 diagnostics](ISSUE_027_DIAGNOSTIC_2026-10-30.md).
- [Открытые проблемы](../KNOWN_ISSUES.md).
