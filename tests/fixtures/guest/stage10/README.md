# Эталоны дисплея этапа 10

Происхождение: оба файла созданы собственным генератором [tools/build/display_pattern.py](../../../../tools/build/display_pattern.py) без внешних изображений и без скриншотов ОС. Размер каждого — 12 288 байт.

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

Проверка: `python3 -m unittest tools/build/test_display_pattern.py` сравнивает эти хеши с повторной генерацией.

Сравнение кадра с эталоном: `python3 tests/system/display_compare.py EXPECTED ACTUAL --width 64 --height 48 [--tolerance N]`.
