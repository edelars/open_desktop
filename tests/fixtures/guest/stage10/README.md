# Эталоны дисплея этапа 10

Происхождение: файлы созданы собственным генератором [tools/build/display_pattern.py](../../../../tools/build/display_pattern.py) без внешних изображений и без скриншотов ОС. Размер первых двух — 12 288 байт, третьего — 19 200 байт.

## Формат

- Raw, без заголовка: 4 байта на пиксель, строки сверху вниз, без паддинга; stride = width × 4.
- Порядок байтов пикселя в памяти: `B, G, R, X`; `X` всегда `0x00`.
- Все числа целые; вычисления в формулах ниже выполняются без округления с плавающей точкой.

## Формулы узоров

Обозначения: `x` ∈ [0, width), `y` ∈ [0, height), `s` — seed (0…2³²−1). Значения `R, G, B` ∈ [0, 255].

- `gradient`: `R = (x·256 // width) XOR (s & 0xFF)`; `G = (y·256 // height) XOR ((s >> 8) & 0xFF)`; `B = ((x + y) & 0xFF) XOR ((s >> 16) & 0xFF)`.
- `bars`: `i = (x·8 // width + s) mod 8`; цвет `i` берётся из таблицы (R,G,B): 0 белый FFFFFF, 1 жёлтый FFFF00, 2 голубой 00FFFF, 3 зелёный 00FF00, 4 пурпурный FF00FF, 5 красный FF0000, 6 синий 0000FF, 7 чёрный 000000.
- `checker`: ячейки 8×8 пикселей; если `((x // 8) + (y // 8)) & 1 == 0`, то чёрный 000000; иначе `(0xFF XOR (s & 0xFF), 0xFF XOR ((s >> 8) & 0xFF), 0xFF XOR ((s >> 16) & 0xFF))`.

Гостевой probe (S10-08) реализует те же формулы; при расхождении эталон считается правильным, а probe — ошибочным, пока не доказано обратное.

## Файлы

| Файл | Команда генерации | SHA-256 |
|---|---|---|
| `bars_64x48_seed0.raw` | `python3 tools/build/display_pattern.py --width 64 --height 48 --pattern bars --seed 0 --output tests/fixtures/guest/stage10/bars_64x48_seed0.raw` | `a738f2f35ccf34d9bca698c40fcca5cc8ab4bda80b58004f6b42a71228a5a3e2` |
| `checker_64x48_seed0x204080.raw` | `python3 tools/build/display_pattern.py --width 64 --height 48 --pattern checker --seed 0x204080 --output tests/fixtures/guest/stage10/checker_64x48_seed0x204080.raw` | `12779eb36b1949960129604b0f81f19cc8ec5c58d70c43240b3f744c0884d529` |
| `checker_80x60_seed0x204080.raw` | `python3 tools/build/display_pattern.py --width 80 --height 60 --pattern checker --seed 0x204080 --output tests/fixtures/guest/stage10/checker_80x60_seed0x204080.raw` | `9d7970a72f5c37a0c4035d948b307dacd0b09763717aff81658bec7b0d0eccdb` |

Проверка: `python3 -m unittest tools/build/test_display_pattern.py` сравнивает эти хеши с повторной генерацией.

## Гостевой probe дисплея (S10-08)

Образ собственного гостевого кода, который сам программирует режим, рисует `bars` 64 × 48, seed 0, и выполняет `HLT`. Он собран генератором [tools/build/build_stage10_display_probe.py](../../../../tools/build/build_stage10_display_probe.py) вручную (без внешнего ассемблера) и лежит в контейнере `ODFWIMG1` платы firmware с обрезанными хвостовыми нулями (288 байт; загрузчик дополняет нулями до 4 МиБ, CRC32 в заголовке посчитан по полному образу). Гость считает узор по формуле `bars` выше без деления (счётчик 8 на пиксель и индекс бара по модулю 8); результат совпадает с `bars_64x48_seed0.raw` байт в байт.

| Файл | Команда генерации | SHA-256 |
|---|---|---|
| `display_probe_bars_64x48_seed0.img` | `python3 tools/build/build_stage10_display_probe.py --pattern bars --width 64 --height 48 --seed 0 --output-dir tests/fixtures/guest/stage10` | `4ebe326b92f544575cc2d3ffbe0874c91ef6e8c37ae8070dcda86f03a2cb6a4a` |
| `display_probe_bars_64x48_seed0.json` | та же команда | манифест: хеш кадра, размеры, число инструкций (`expected_retired` = 25 983), STATUS гостя (`0x1` после `MODE_COMMIT`, `0x3` после doorbell) |

Проверки: `python3 -m unittest tools/build/test_build_stage10_display_probe.py` (повторная генерация совпадает с файлами); `cargo test -p od-machine --offline --lib stage10_display_probe` (загрузчик на плате firmware сравнивает хеш кадра из `read_frame_into` и `expected_retired` с манифестом); `od-runner display-probe --image tests/fixtures/guest/stage10/display_probe_bars_64x48_seed0.img --frame-out FILE`.

Сравнение кадра с эталоном: `python3 tests/system/display_compare.py EXPECTED ACTUAL --width 64 --height 48 [--tolerance N]`.
