# S2-37db — MINPD XMM0,m128 [RAX]

**Статус:** локальный interpreter-only срез завершён. Физическая x86-приёмка не заявляется.

Реализована точная legacy Long64 форма `66 0F 5D 00` под schema-v2 профилем
`s2-long-minpd-memory-rax-v1`. Декодер допускает эту memory-форму отдельно;
другие ModR/M адресации, дополнительные префиксы, VEX и EVEX остаются вне
профиля. Выполняется полный четырёхбайтовый fetch до проверок исполнения.

Требуется SSE2 (`CPUID.01H:EDX[26]`). После точного декодирования отсутствие
SSE2, `CR0.EM=1` или `CR4.OSFXSR=0` дают `#UD`; при пройденных этих проверках
`CR0.TS=1` даёт `#NM`. Затем требуется присутствующее XMM/MXCSR состояние и
локально допустимое MXCSR: supported mask `0xffbf`, все шесть exception mask
bits установлены, `DAZ=0`, `FTZ=0`, reserved bits равны нулю. RC и прежние IE/DE
sticky bits разрешены.

Далее проверяются 16-байтовое выравнивание RAX и canonical48 span
`[RAX,RAX+15]`, включая переполнение endpoint. `#GP(0)` при misalignment,
noncanonical span или переполнении возникает до memory callback. `RFLAGS.AC`
не влияет на допуск: выровненный адрес допустим с AC=0 и AC=1. Успешный путь
выполняет ровно один `read16`. Ошибка чтения не меняет полный CPU state, RIP,
MXCSR или retirement.

Для каждой из двух binary64 lanes применяется `Binary64::min_sse_raw_result`.
На tie (включая противоположные signed zeros) и при любом NaN выбирается raw
правый операнд из памяти; SNaN payload сохраняется без quieting. IE объединяется
по обоим lanes, а DE накапливается отдельно для каждой lane только когда в ней
есть subnormal operand и нет NaN. NaN в lane 0 поэтому не подавляет DE из
subnormal в lane 1. Новые IE/DE объединяются с прежними sticky bits. Успех
заменяет все 128 бит XMM0; остальное CPU state, RFLAGS, RAM и определённость
flags сохраняются.

Schema/result validation требует точный профиль и control echo, байты,
единственную исполняемую code mapping и одну читаемую 16-байтовую data mapping
по RAX, budget 1 и stop RIP `entry+4`. Raw-u64 oracle независим от production
helper и host floating point. Добавлен fixture
`s2_37db_minpd_m128_qnan_and_other_lane_denormal.case.json`. JIT, native, C ABI,
generic interpreter и fragment execution для этого профиля остаются закрытыми.

## Проверки

- Focused decoder/direct ISA tests: **3/3**. Проверены exact decode и соседи,
  fetch/gate order, aligned AC=0/1, alignment, noncanonical span, and the high
  misaligned `u64::MAX-7` boundary faulting on alignment before endpoint
  arithmetic; one `read16` and atomic read failure.
- `od-isa`: **1,730 passed**.
- `od-testkit`: **454 passed**.
- `od-runner`: **112 unit + 483 fixture tests passed**. MINPD fixture matrix
  выполнила 128 raw-u64 сравнений по vectors, четырём RC и четырём sticky
  histories; включены cross-lane IE+DE, QNaN на обеих сторонах, SNaN в левом
  операнде, ties, signed zero, infinity и subnormal. Прямой ISA oracle также
  проверяет SNaN в memory-операнде.
- `cargo check -p od-isa -p od-testkit -p od-runner --all-targets --offline`:
  passed.
- `cargo check --workspace --all-targets --offline`: passed.
- `od-x86-runner`: **8 library + 4 binary tests passed**; this validates runner
  infrastructure, not physical x86 execution.
- `cargo fmt --all -- --check`: passed.
- `graphify update .`: **10,859 nodes / 23,366 edges / 321 communities**.
- Independent code/schema review моделью Sol/high: **clean**; замечания о
  случайной duplicate MAXPS ветке, truncation bytes и формулировках покрытия
  исправлены, итоговая повторная проверка пройдена.

Аппаратное исполнение на x86 и физическая acceptance этого профиля не заявляются.

**Источники:** Intel SDM Rev. 093, Vol. 2B `MINPD` pp. 4-16–4-18; Vol. 2A
Type 2/Table 2-19 p. 2-27; Vol. 1 §4.9.2, §§11.5.3.1–3, Appendix D D-9/D-13/D-15;
Vol. 3A §16.1.3/Table 16-1.
