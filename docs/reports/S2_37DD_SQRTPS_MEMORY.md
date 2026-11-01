# S2-37dd — SQRTPS m128 memory profile

Реализован отдельный schema-v2 профиль `s2-long-sqrtps-memory-rax-v1` для
строгой формы Long64 `SQRTPS XMM0,m128 [RAX]` (`0F 51 00`, длина 3).
Декодер отвергает любые префиксы и REX, другой ModR/M, SIB, displacement,
усечённый код, VEX и EVEX. Новый маршрут использует SSE availability bit 25;
SSE2 bit 26 не требуется. JIT и native admission остаются закрыты.

Перед проверкой gate-параметров декодируются все три байта инструкции. Затем
проверяются SSE, CR0.EM, CR0.TS и CR4.OSFXSR в порядке #UD, затем #NM. Профиль
принимает только MXCSR с нулевыми reserved bits 16–31, DAZ=0, FTZ=0 и всеми
шестью exception masks, установленными в 1; все RC и исходные sticky flags
принимаются. Неподдерживаемый MXCSR и отсутствующий SSE state закрывают
локальный профиль без изменения состояния. Путь unmasked/#XM не реализован.

Эффективный RAX должен быть выровнен на 16 байт независимо от AC и CPL. До
callback проверяется canonical48 для начала и конца диапазона `[RAX,RAX+15]`;
ошибка выравнивания или span возвращает #GP(0). В частности, тестовое значение
`u64::MAX-7` отклоняется из-за выравнивания; оно не обозначает переполнение.
После проверок вызывается ровно одно чтение 16 байт. При ошибке чтения CPU,
MXCSR, RIP и retirement остаются без изменений.

Результат строится только из четырёх little-endian lanes прочитанной памяти;
прежнее содержимое XMM0 в расчёте не участвует. Каждая lane передаётся в
`Binary32::sqrt_raw_result(MXCSR.RC)`. Отрицательное ненулевое значение,
включая отрицательную subnormal и −∞, даёт IE и `0xffc00000`; SNaN тихо
квалифицируется существующей helper-политикой и даёт IE; QNaN проходит без IE.
Положительная subnormal даёт DE. Inexact корень даёт PE. Эти флаги объединяются
между lanes и добавляются через OR к прежним sticky flags. После успешного
чтения XMM0 заменяется целиком, RIP увеличивается на 3. GPR, RFLAGS, остальные
XMM, RAM и прочее состояние сохраняются.

Ручной fixture `tests/fixtures/x86/stage2/s2_37dd_sqrtps_m128_memory_source_lanes_rounding.case.json`
использует poisoned XMM0 и memory lanes `[4, 2, 1, 0.25]`. Independent testkit
oracle `independent_sqrtps` отдельно рассчитывает четыре результата и IE/DE/PE;
он не использует production helper или host floating point. Runner покрывает
все RC, signed zeros, +∞, нормальные числа, положительные и отрицательные
subnormal, отрицательное нормальное значение, QNaN/SNaN, SNaN quieting и смешанный
вектор `[-1, min-positive-subnormal, 2, SNaN]`, который агрегирует IE|DE|PE.
Дополнительно проверены poisoned old XMM0, backend denial, результат/control
metadata tampering, exact/neighbor encoding, gate-before-data-callback,
alignment при AC=0/1, canonical fault и rollback при ошибке чтения.

Нормативные источники: Intel SDM Rev. 093 Vol. 2B `SQRTPS` pp. 4-664–4-666;
Vol. 2A p. 2-27; Vol. 1 §§11.5.2.1–.2 и 11.5.3.1–.3; Vol. 3A Event 19 и
Table 7-8. Реализация и fixtures подтверждают локальный interpreter profile;
физическое исполнение на x86 и поддержка за пределами точной формы не заявляются.

Проверки: полный `od-isa` suite (1736 passed), `od-testkit` suite (454 passed),
`od-runner --test fixture_v2` (485 passed), `cargo check --workspace --all-targets
--offline`, `cargo fmt --all -- --check`, `git diff --check`, Graphify update и
`tools/check_layout.py` (из основного worktree) и независимое Sol/high review без замечаний прошли. Первый полный
`cargo test --workspace --all-targets --offline` был остановлен ОС из-за
`No space left on device`; после очистки только локального `target/` тесты
релевантных crates прошли полностью. Физический x86 acceptance не выполнялся.
