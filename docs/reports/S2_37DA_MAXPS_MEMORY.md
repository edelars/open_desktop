# S2-37da — MAXPS XMM0,m128 [RAX]

**Статус:** локальный interpreter-only срез завершён. Физическая x86-приёмка
не заявляется.

Реализована только точная legacy Long64 форма `0F 5F 00` под schema-v2
профилем `s2-long-maxps-memory-rax-v1`. Профиль имеет отдельные SSE gates
(`CPUID.01H:EDX[25]`); SSE2 не требуется. Другие memory-адресации, префиксы,
REX, VEX и EVEX формы остаются вне профиля.

Полный fetch длины 3 байта предшествует gate checks. Отсутствие SSE,
`CR0.EM=1` или `CR4.OSFXSR=0` дают `#UD`; только после прохождения этих gates
`CR0.TS=1` даёт `#NM`. Требуется присутствующее XMM/MXCSR состояние. MXCSR
допускает только биты `0xffbf`, требует установленных всех шести exception
mask bits, `DAZ=0`, `FTZ=0` и нулевые reserved bits; RC и накопленные sticky
flags разрешены.

Перед чтением проверяется 16-байтовое выравнивание RAX, затем весь canonical48
span `[RAX,RAX+15]`. Misalignment, noncanonical span и переполнение endpoint
дают `#GP(0)` без memory callback. Выравнивание обязательно независимо от
`RFLAGS.AC`; оба значения AC допускаются при выровненном адресе. Успешный путь
выполняет ровно один `read16`.

Каждая из четырёх 32-битных lanes использует
`Binary32::max_sse_raw_result`. При tie, включая оба направления signed-zero
пары, и при любом NaN выбираются исходные raw bits правого операнда; SNaN
payload не quiet-ится. IE объединяется по lanes. DE учитывается отдельно для
каждой lane и подавляется только если NaN находится в этой же lane. Поэтому
NaN в другой lane не подавляет DE. IE/DE объединяются с прежними sticky bits;
успех атомарно заменяет все 128 бит XMM0 и увеличивает RIP на 3. Ошибка чтения
не меняет состояние CPU, MXCSR, RIP или retirement.

Schema-v2 и независимый результатный validator требуют точные байты, одну
исполняемую code mapping и одну читаемую 16-байтовую data mapping по RAX,
budget 1 и stop RIP `entry+3`. Для oracle используются отдельные integer-bit
классификация и порядок, без host floating-point и без production helper.
Интерпретаторный runner исполняет профиль; JIT явно отклоняет его. Native
denial assertion добавлен в Linux/x86-64 runner, однако этот модуль исключён
на текущем macOS хосте и здесь отдельно не исполнялся. C ABI, generic и
fragment execution не открыты этим профилем.

## Проверки

- Focused `od-isa maxps_memory_rax`: **3 passed**; включены точный decode и
  invalid neighbors, четыре lanes, одиночные QNaN/SNaN в обеих позициях,
  raw payloads, ±0 ties, infinities, subnormals, отрицательное сравнение,
  весь RC × 64 sticky combinations, gate order, truncated fetch-before-gates,
  MXCSR, alignment/canonical/overflow/AC, ровно один read16 и no-commit faults.
- Focused `od-testkit maxps_m128_schema`: **1 passed**; схема и tampering.
- `cargo test -p od-isa -p od-testkit -p od-runner --offline`: **1723 od-isa,
  454 od-testkit, 112 runner unit и 482 fixture_v2 tests passed**.
- `cargo check --workspace --all-targets --offline`: passed.
- `cargo fmt --all -- --check` и `git diff --check`: passed.
- `graphify update .`: **10,877 nodes / 23,255 edges / 336 communities**.
- Independent review `gpt-6.1-sol` / high: **approved, no remaining findings**.
  Review requested sole-NaN and reverse-zero tie cases plus a corrected `entry+3`
  diagnostic; all were added/fixed and re-reviewed.
- Native denial test is present but not run here: `native_v2` is Linux/x86-64
  only. No physical Intel/AMD MAXPS execution is claimed.
- `python3 tools/check_layout.py` in this worktree reports the regular `.git`
  pointer file as an unmapped root file and missing ignored work-manifest links
  from pre-existing reports. This task does not change project structure.

**Источники:** Intel SDM Rev. 093, Vol. 2B `MAXPS` pp. 4-8–4-10; Vol. 2A
Type 2/Table 2-19 p. 2-27; Vol. 1 §4.9.2, §§11.5.3.1–3 и Appendix D;
Vol. 3A §16.1.3/Table 16-1.
