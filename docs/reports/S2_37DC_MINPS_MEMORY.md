# S2-37dc — MINPS XMM0,m128 [RAX]

**Статус:** локальный interpreter-only профиль реализован. Физическая x86-приёмка не заявляется.

Добавлена только точная legacy Long64 форма `0F 5D 00`, длина 3 байта, под schema-v2 профилем `s2-long-minps-memory-rax-v1`. Граница профиля использует отдельный SSE feature bit `CPUID.01H:EDX[25]`; состояние SSE2 отражается отдельно, но не требуется. Другие memory addressing forms, префиксы, REX, VEX, EVEX и прочие формы остаются закрыты.

Весь fetch инструкции предшествует проверкам состояния. Отсутствие SSE, `CR0.EM=1` или `CR4.OSFXSR=0` дают `#UD`; только после прохождения этих условий `CR0.TS=1` даёт `#NM`. Требуется присутствующее XMM/MXCSR состояние. MXCSR допускает только биты `0xffbf`, требует все шесть exception mask bits, `DAZ=0`, `FTZ=0` и нулевые reserved bits; любое RC и ранее накопленные sticky flags разрешены.

Перед callback проверяются 16-байтовое выравнивание и весь canonical48 span `[RAX,RAX+15]`. Misalignment, noncanonical span и переполнение checked endpoint дают `#GP(0)` без memory callback. Alignment обязательно независимо от `RFLAGS.AC`; выровненный operand принимается при AC=0 и AC=1. Успешный путь выполняет ровно один `read16`. Сбой чтения не меняет CPU/XMM/MXCSR/RIP и retirement.

Четыре 32-битные lane вычисляются через `Binary32::min_sse_raw_result`. Для численного сравнения выбирается меньший операнд; при равенстве, включая оба направления signed-zero пары, и при любом QNaN/SNaN выбираются исходные raw bits правого операнда, без quiet-изменения SNaN payload. IE объединяется по всем lanes. DE возникает для subnormal operand только в той же lane, где нет NaN; NaN lane не подавляет DE в другой lane. Новые IE/DE добавляются к старым sticky bits. Успешная операция атомарно заменяет все 128 бит XMM0 и увеличивает RIP на 3; остальные CPU state, flags и память сохраняются.

Schema-v2 и независимый result validator ограничивают точные байты, code/data mappings, длины, бюджет и stop RIP `entry+3`; backend tests подтверждают отказ JIT/native. Oracle использует отдельную integer-bit классификацию/порядок, literal vectors и не использует host floating point или production helper. Generic, fragment, C ABI и native execution не открыты этим профилем.

## Проверки

- Focused `od-isa` MINPS tests: **3 passed** — точная кодировка и invalid neighbors, четыре raw-bit lanes, 16 комбинаций RC/sticky, SSE=1 с SSE2=0, fetch-before-gates, порядок `#UD`/`#NM`, AC×alignment, canonical-span fault и read-failure rollback.
- Focused `fixture_v2` MINPS runner test: **1 passed**, 6,144 integer-oracle comparisons (24 literal vectors × 4 RC × all 64 prior sticky histories). Coverage includes both signed-zero tie directions; sole QNaN and SNaN from RHS; same-lane NaN+subnormal in both source orders with no DE; pure DE; and a lane0×lane1 4×4 product of `{none, I, D, I+D}` with remaining lanes fixed to normal values. The product exercises every cross-lane IE/DE combination while keeping the event source local and visible. Schema tampering, misalignment with AC=0/1, snapshot preservation and JIT/native denial are also checked.
- Final targeted cross-profile reruns passed: `minps_m128_memory` **1**, `minpd_m128_memory` **1**, `maxps_unmasked_four_lane_anchors_validate_independent_results_and_denials` **1**, and `od-testkit` **454** tests. This confirms the MINPS validator changes did not broaden or regress MAXPS/MINPD admission.
- Полные relevant suites passed: `od-isa` **1733**, `od-testkit` **454**, `od-runner` **112 unit + 484 fixture_v2**, `od-x86-runner` **8 library + 4 binary tests**.
- `cargo check --workspace --all-targets --offline`, `cargo fmt --all -- --check` и `git diff --check`: passed.
- `python3 tools/check_layout.py` сообщает только worktree-baseline проблемы: `.git` pointer как unmapped root file и две pre-existing отсутствующие ссылки из `STAGE_2_REPORT.md` на work manifests. Структура проекта этой задачей не менялась.
- Graphify update after final test changes: **10,886 nodes / 23,430 edges / 316 communities**.
- Native-denial assertion added to Linux/x86-64 runner; этот target исключён на текущем macOS хосте, поэтому локально native_v2 assertion не исполнялся. Physical x86 acceptance не проверялась.
- Independent `gpt-6.1-sol` high code/schema review: **clean after corrections**, no remaining actionable findings. Review covered MAXPS/MINPS snapshot guard scope, canonical span fault boundary, lane0×lane1 raw class matrix, NaN precedence and source sides, both signed-zero ties, full sticky history, and existing SSE/alignment/read atomicity checks.

**Источники:** Intel SDM Rev. 093 Vol. 2B `MINPS` pp. 4-19–4-21; Vol. 2A §2.5.2/Table 2-19 p. 2-27; Vol. 1 §4.9.2, §§11.5.3.1–3, Appendix D; Vol. 3A §16.1.3/Table 16-1. Эти ссылки соответствуют официальным источникам, уже указанным в проекте; PDFs в рамках этой задачи повторно не открывались.
