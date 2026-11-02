# S2-37dq — ADDPS m128 memory profile

Реализован отдельный schema-v2 profile `s2-long-addps-memory-rax-v1` для
точной инструкции Long64 `ADDPS XMM0,m128 [RAX]`, код `0F 58 00` длиной три
байта. Профиль interpreter-only; JIT и NativeX86 явно отказывают запуску, а
`RunResult::validate_for` отклоняет результат с успешным exit, если его
переименовать в любой из этих backend.

Декодирование выполняется для полного кода до проверки feature и execution
gates. Требуется только SSE CPUID bit 25; SSE2 не требуется. Проверяются
CR0.EM, CR0.TS и CR4.OSFXSR, при этом профилем поддержаны все четыре MXCSR.RC,
любые исходные sticky flags, reserved bits=0, DAZ=FTZ=0 и все шесть exception
mask bits=1. При таких масках unmasked exception delivery не возникает и
`#XM` не реализуется.

Эффективный адрес берётся только из RAX. Профиль требует 16-byte alignment
даже при AC=0 и CPL=3; сначала проверяет canonical48 для начала и конца
`[RAX,RAX+15]`, после чего выполняет ровно один `read16`. До успешного чтения
никакие архитектурные изменения не фиксируются. Ошибка alignment/span
возвращает `#GP(0)`, ошибка чтения откатывает весь instruction state. Записей
в память нет.

Четыре binary32 RHS lane из прочитанного блока складываются с соответствующими
lane старого XMM0 с помощью `Binary32::add_raw_result` и выбранного RC.
Независимый oracle testkit вычисляет те же lanes через `independent_addps`.
IE/DE/OE/UE/PE агрегируются по четырём lanes и добавляются к прежнему MXCSR
через OR. ZE, reserved/high bits (которые для валидного входа равны нулю),
GPR, остальные XMM/x87 и RFLAGS сохраняются; успешный RIP продвигается на 3.

Фикстура `tests/fixtures/x86/stage2/s2_37dq_addps_m128_memory_source_and_destination_lanes.case.json`
использует независимые старые XMM0 и memory значения, что выявляет неверное
использование только одного операнда. ISA tests проверяют точное декодирование,
все RC и sticky histories, mixed invalid/denormal/overflow/underflow/precision,
ties-to-even и directed rounding, отсутствие SSE2, порядок полного fetch и
gates, обязательную alignment при AC=0/1, canonical-span faults, единственный
read callback и rollback. Runner test проверяет профиль, oracle result,
tampering, SSE gating и отрицательное backend admission/relabel.

Вне этого среза остаются физическая проверка на x86, иные ADDPS encodings и
операнды, generic exception delivery и unmasked SIMD floating-point exceptions.

**Проверки:** `cargo test -p od-isa --offline` прошёл (1834 tests),
`cargo test -p od-testkit --offline` прошёл (455 tests),
`cargo test -p od-runner --offline --test fixture_v2` прошёл (497 tests),
`cargo check --workspace --all-targets --offline`,
`cargo fmt --all -- --check`, `git diff --check` и `graphify update .` прошли.
Workspace check показывает два существовавших unused-variable warning в
`independent_mulss`. `python3 tools/check_layout.py` завершился ошибками из-за
`.git` worktree marker и двух ранее сломанных ссылок на отсутствующие oracle
manifests; изменений layout в этом slice нет. Independent Sol/high review
будет выполнен после commit. Физическая x86 проверка не запускалась.
