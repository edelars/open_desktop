mcp__plugin_context-mode_context-mode__ctx_search({queries:["/Users/senya/projects/open_desktop/.claude/worktrees/agent-a8a74a0231f63a7cd/docs/reports/STAGE_7_REPORT.md","devices finished firmware_"], source:"shk5"}) searches the whole output (1211785 B).

## Регрессия SETcc (2026-11-21)

**Симптом.** ROM, собранный из main (`48fe2f8`), останавливается на `Unsupported { opcode: 148 }` по RIP `0x227cae`; `tests/system/firmware_boot_smoke.py` падает на `boot_core #1`: в транскрипте нет строк `runtime events registered`, `variable image retained`, `exit boot services verified`. ROM на `0ffc506^` и `0ffc506` проходит, `0e5aeb1` (S7-31b, ревью адресов) и main — нет.

**Причина.** `0e5aeb1` заменил в `plan_image` параметр `bool` на перечисление `ImagePlacement` (`Pinned`, `Highest`, `LowestAligned`). В `bs::image::load` ROM после этого содержит `cmpl $1, 0x70(%rsp)` и `sete 0x70(%rsp)` (`0F 94 44 24 70`): компилятор сравнивает значение перечисления и сохраняет результат в тот же слот стека. Профиль прошивки `PagedCpl0FirmwareBootV1` принимал `SETcc` в память только по списку точных форм (в нём был `SETNE [RSP+0x70]`, но не `SETE`). Выбор формы зависит от компилятора и пути каталога сборки, поэтому код прошивки менять нельзя как решение: любая правка раскладки вернёт такую форму.

**Решение.** Корректная семантика в od-isa, а не обход в прошивке: профиль прошивки принимает любой `SETcc r/m8` с приёмником в памяти (Intel SDM Vol. 2, SETcc): `0F 90..9F /r`, все 16 условий, любая адресация, один байт 0/1, флаги не меняются, RIP фиксируется после записи, при сбое записи и неканоничном адресе состояние не меняется. Список точных форм удалён из `step_paged_probe_mode`; базовый профиль не тронут. Прошивка не менялась.

**Проверки.** `cargo test -p od-isa --offline --test paged_probe_profile` — 1715 passed (включая два новых теста и исправленный список соседних форм); `cargo test -p od-isa --offline --lib setcc` — 5 passed; `python3 tests/system/firmware_boot_smoke.py` — exit 0, все случаи манифеста (boot_core, hello, ebs, vars, rt, acpi, gop, blk, blk_backup, blk_no_gpt, fs) и проверка отсутствующего приложения; `cargo fmt --all -- --check`; `cargo check --workspace --all-targets --offline`. Полные наборы od-machine и od-runner не запускались; 50 циклов сброса не повторялись.

**Не проверено.** Инвентарь `s7-31b-rom-isa-inventory.json` не пересобирался (снимок до исправления, три формы `SETcc` в нём числятся неподдержанными). ROM на другом пути каталога может содержать другие формы вне набора: ISSUE-631.
