# S2-38t — MAXPS m128 unmasked IM/DM

Статус: реализация завершена, ограниченный профиль интерпретатора.

## Реализация

- Добавлен отдельный schema-v2 профиль `s2-long-maxps-memory-unmasked-rax-v1` для точного legacy Long64 `0F 5F 00` (`MAXPS XMM0, m128 [RAX]`). Профиль использует SSE bit 25, выполняет ровно один 16-байтовый memory read, требует 16-байтовое выравнивание и канонический диапазон адреса.
- Исполнитель агрегирует IE и DE из всех четырёх binary32 lanes до unmasked decision. Для unmasked события записывает sticky IE/DE, завершает `#XM` или арифметическим `#UD`, не меняя XMM0 и RIP и не retire-ит инструкцию. Masked профиль S2-37da остаётся отдельным.
- Добавлены независимая schema-v2 проверка результатов, echo controls и запрет JIT/native. Fixtures используют реальные memory RHS и poisoned XMM1.
- Генератор создаёт 2 048 комбинаций четырёх lane-классов и IM×DM×OSXMMEXCPT, 256 комбинаций RC×64 sticky histories и 12 специальных lane/приоритетных случаев.

## Проверки

- `cargo test -p od-isa --offline` — 1 923 passed.
- `cargo test -p od-testkit --offline` — 455 passed.
- `cargo test -p od-runner --test fixture_v2 --offline` — 533 passed.
- S2-38t focused runner fixture test — 1 passed; проверяет все 2 316 fixtures, контрольный echo, schema tampering и JIT/native denial.
- S2-38t native-local-only denial test — 1 passed.
- Graphify index обновлён командой `graphify update .`.

## Границы

Профиль не включает JIT, native execution или расширение общего CPU runner. Аппаратное поведение не измерялось; физический x86 backend не допускается.
