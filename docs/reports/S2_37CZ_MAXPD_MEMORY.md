# S2-37cz — MAXPD XMM0,m128 [RAX]

**Статус:** локальный interpreter-only срез завершён. Физическая x86-приёмка не заявляется.

Реализована только точная legacy Long64 форма `66 0F 5F 00` под schema-v2 профилем
`s2-long-maxpd-memory-rax-v1`. Декодер допускает её как отдельную память-форму;
прочие memory-формы, префиксные варианты и VEX/EVEX-формы остаются вне профиля.

Перед исполнением требуется SSE2 (`CPUID.01H:EDX[26]`). Полный fetch инструкции
предшествует локальным gate checks. Отсутствие SSE2, `CR0.EM=1` или
`CR4.OSFXSR=0` дают `#UD`; только при пройденном `#UD` gate `CR0.TS=1` даёт
`#NM`. Требуется присутствующее XMM/MXCSR состояние. MXCSR допускает только
биты маски `0xffbf`, требует установленных всех шести exception mask bits,
`DAZ=0`, `FTZ=0` и нулевые reserved bits; RC и ранее накопленные IE/DE
разрешены.

Перед чтением данных проверяется 16-байтовое выравнивание RAX, затем весь
canonical48 span `[RAX,RAX+15]`. Misalignment, noncanonical span или
непредставимый endpoint дают `#GP(0)` без memory callback. Проверка выравнивания
не зависит от `RFLAGS.AC`: оба значения AC допускаются для уже выровненного
адреса. Успешный путь выполняет ровно один `read16`.

Каждая 64-битная lane использует собственную операцию `Binary64::max_sse_raw_result`.
На tie, включая пару signed zeros, и на любом NaN выбирается исходное raw-значение
правого операнда; payload SNaN не quiet-ится. IE агрегируется OR по двум lanes.
DE агрегируется по lanes отдельно: subnormal операнд добавляет DE только если
в той же lane нет NaN. Поэтому NaN в одной lane не маскирует DE в другой.
IE/DE объединяются с прежними sticky bits. Успех заменяет обе lanes XMM0;
другие регистры и состояние, RFLAGS и определённость flags сохраняются.
Ошибка чтения оставляет полный CPU state и RIP без изменений и не retire-ит
инструкцию.

Schema и результатный validator ограничивают fixture точными байтами, одной
исполняемой code mapping, одной 16-байтовой читаемой data mapping по RAX,
budget 1 и stop RIP `entry+4`. Добавлен fixture
`s2_37cz_maxpd_m128_qnan_and_other_lane_denormal.case.json`. JIT, native,
C ABI, generic interpreter и fragment execution остаются закрытыми для этого
профиля.

## Проверки

- Focused decoder/direct ISA tests: **3/3**; direct проверки покрывают exact
  encoding, gate order и fetch-before-gates, aligned/unaligned AC=0/1,
  canonical span, один `read16`, отсутствие чтения при fault и atomic read error.
- `od-isa`: **1717 passed**.
- `od-testkit`: **453 passed**.
- `od-runner` `fixture_v2`: **482 passed** (`RUST_TEST_THREADS=1` для исключения
  коллизий временных файлов в параллельных тестах).
- `od-x86-runner`: **8 library + 4 binary tests passed**. Это проверка
  launcher-инфраструктуры, не физическое исполнение MAXPD.
- `cargo check --workspace --all-targets --offline`: passed.
- `cargo fmt --all -- --check` and `git diff --check`: passed.
- `graphify update .`: **10,843 nodes / 23,187 edges / 327 communities**.
- Независимое code/schema review моделью Sol/high: MAXPD семантика и контракты
  подтверждены; найденные при ревью unrelated schema edits и неполная direct
  vector table исправлены, повторная проверка затронутой таблицы пройдена.
- Layout checker прошёл в основном checkout (**48 directories, 6 crate
  scaffolds, 14 instruction scopes**). В изолированном worktree его локальный
  запуск не проходит из-за `.git` pointer file и отсутствующих ignored `work/`
  manifests; layout в этой задаче не менялся.

Отдельных физических Intel/AMD запусков и аппаратных acceptance claims нет.

**Источники:** Intel SDM Rev. 093, Vol. 2B `MAXPD` pp. 4-5–4-7; Vol. 2A
Type 2/Table 2-19 p. 2-27; Vol. 1 §4.9.2, §§11.5.3.1–3 и Appendix D
D-9/D-13/D-15; Vol. 3A §16.1.3/Table 16-1.
