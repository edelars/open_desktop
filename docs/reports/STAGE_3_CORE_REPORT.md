# Этап 3 Core — промежуточный отчёт

Статус: **промежуточный. Этап 3 НЕ принят; линия S3 Core выполняется.** Этот отчёт фиксирует отправную точку на `b496d5e` и результаты S3-50…S3-57 в ветке `codex`. Отчёт не подтверждает приёмку этапа 3.

**Текущий статус (2026-11-11):** реализация и ограниченные измерения линии проверены и закоммичены в `codex`; независимое итоговое ревью кода не выявило дефектов. S3-56 и S3-57 остаются частичными до разрешения ISSUE-075. Исходный baseline S3-50: `od-jit` — 412 passed; `od-host` — 358 passed и 3 failed из-за отсутствующих локальных corpus artifacts; layout-check — 45 ошибок.

Связанные документы: [план S3 Core](../plans/STAGE_3_CORE.md), [основной план этапа 3](../plans/STAGE_3.md), [контракт JIT](../specs/JIT_CONTRACT.md), [карта проекта](../PROJECT_LAYOUT.md).

## 1. Сводка линии

| Задача | Статус на этой ревизии |
|---|---|
| S3-50 — отчёт и инвентарь дублирования | Ревью пройдено; коммит `80a0fe6`, Graphify обновлён |
| S3-51 — единые правила допуска | Независимое архитектурное ревью Sol пройдено; коммит `8dcaac8`, Graphify обновлён |
| S3-52 — один механизм допуска | Независимое ревью Sol одобрено; коммит `dd9c9bb`, Graphify обновлён |
| S3-53 — единая точка входа диспетчера | Независимое ревью Sol одобрено; коммит `8aab6f3`, Graphify обновлён |
| S3-54 — generic путь и multi-entry cache | Проверки выполнены, независимое ревью Sol одобрено; коммит `6c438a8`, Graphify обновлён |
| S3-55 — interpreter fallback | Ограниченный mixed маршрут реализован и проверен; независимое Sol-ревью пройдено, коммит `31380a5` |
| S3-56 — измерения generic пути | Частично: Sol-ревью выполненного объёма пройдено; задача не принята из-за ISSUE-075 и отсутствующего S3-17 corpus; коммит `795be59` |
| S3-57 — финальное ревью линии | Частично: независимое Sol-ревью кода пройдено; приёмка ждёт S3-56 и ISSUE-075 |

## 2. S3-50 — инвентарь дублирования

Инвентарь снят с `crates/od-jit/src/block.rs` и `dispatch.rs` в worktree `codex`. Под «байтовой проверкой» ниже понимается дополнительная сверка уже декодированного кода с точной кодировкой; декодирование и семантические условия остаются источником операндов и размеров.

### 2.1 Admission-структуры

У всех структур есть общий `AdmittedInstructionBoundaries` (`block.rs:139–145`): `code_base`, `code_end`, `code_bytes` и список границ инструкций. В таблице перечислены собственные поля, привязанные к RIP. Проверки источника кода также ограничивают длину, диапазон и цели переходов.

| Структура | Форма / срез-источник | Свои RIP/значения | Условие декодированной формы и дополнительная сверка байтов |
|---|---|---|---|
| `S0_09NopAdmission` (`149–152`) | S0-09 + bare NOP | `nop_rips` | S0-09: MOV imm64 в RAX–RBX (без legacy-префиксов, REX `48`, qword, len 10); ADD/SUB/CMP qword reg-reg для регистров RAX–RBX (без legacy-префиксов, REX `48`, len 3); короткие JE/JNE (opcode `74`/`75`, без префиксов, qword sizes, len 2) (`is_s0_09_form`, `5122–5178`). NOP: `Operation::Nop`, без префикса, длина 1 и байт `90` (`2168–2174`). Для S0-09 байтовой сверки формы нет; для NOP сверка байта `90` избыточна для различения при условии, что opcode получен декодированием этих же байтов; она сохраняется как проверка согласованности. |
| `S3_32aAddRaxRaxAdmission` (`245–248`) | ADD RAX,RAX / S3-32a | `add_rax_rax_rips` | Qword ADD RAX,RAX, opcode 03, REX `48`, без LOCK/сегментных/66/67, длина 3; guard сверяет `48 03 C0`; избыточна для связанной пары: opcode `03`, register RAX/RAX и mod=11 задают ModRM `C0`. (`1922–1945`) |
| `S0_09NopIncRaxAdmission` (`157–161`) | INC RAX / S3-33a | `nop_rips`, `inc_rax_rips` | Qword INC RAX, FF, REX `48`, без запрещённых префиксов, длина 3; guard сверяет `48 FF C0`; избыточна для связанной пары: `FF /0`, mod=11 и RAX задают `C0`. (`2134–2150`) |
| `S0_09NopDecRaxAdmission` (`166–170`) | DEC RAX / S3-33b | `nop_rips`, `dec_rax_rips` | Qword DEC RAX, FF, REX `48`, без запрещённых префиксов, длина 3; guard сверяет `48 FF C8`; избыточна для связанной пары: `FF /1`, mod=11 и RAX задают `C8`. (`2151–2167`) |
| `S0_09NopNegRaxAdmission` (`175–179`) | NEG RAX / S3-33c | `nop_rips`, `neg_rax_rips` | Qword NEG RAX, F7, REX `48`, без запрещённых префиксов, длина 3; guard сверяет `48 F7 D8`; избыточна для связанной пары: `F7 /3`, mod=11 и RAX задают `D8`. (`1973–1989`) |
| `S0_09NopNotRaxAdmission` (`184–188`) | NOT RAX / S3-33g | `nop_rips`, `not_rax_rips` | Qword NOT RAX, F7, REX `48`, без запрещённых префиксов, длина 3; guard сверяет `48 F7 D0`; избыточна для связанной пары: `F7 /2`, mod=11 и RAX задают `D0`. (`1990–2006`) |
| `S0_09NopLeaRaxRbxAdmission` (`193–197`) | LEA RAX,[RBX] / S3-33h | `nop_rips`, `lea_rips` | Qword LEA base RBX, без index/disp, 8D, REX `48`, длина 3; guard сверяет `48 8D 03`; избыточна для связанной пары: len 3 исключает SIB/disp, base RBX и dst RAX задают ModRM `03`. (`2007–2030`) |
| `S0_09NopLeaRaxRbxDisp8Admission` (`202–206`) | LEA RAX,[RBX+disp8] / S3-33j | `nop_rips`, `lea_rips: (rip,i64)` | Qword LEA base RBX и signed disp8, 8D, REX `48`, длина 4; guard сверяет `48 8D 43` и payload с декодированным disp; необходима для различения: SIB без disp `48 8D 04 23` и два REX без disp `40 48 8D 03` дают те же decoded-поля, что disp8=0 `48 8D 43 00`. (`2031–2058`) |
| `S0_09NopLeaRaxRbxRcxAdmission` (`211–215`) | LEA RAX,[RBX+RCX] / S3-33l | `nop_rips`, `lea_rips` | Qword LEA base RBX, index RCX, scale 1, disp 0; guard сверяет `48 8D 04 0B`; избыточна для связанной пары: index RCX требует SIB, len 4 исключает disp; dst RAX и base RBX задают `04 0B`. (`2059–2082`) |
| `S0_09NopLeaRaxRipDisp32Admission` (`220–224`) | LEA RAX,[RIP+disp32] / S3-33m | `nop_rips`, `lea_displacements: (rip,i64)` | Qword LEA с базой NextRip, без index, signed disp32; guard сверяет `48 8D 05` и disp32 payload с декодированным значением; избыточна для связанной пары: NextRip задаёт mod=00/rm=101 и `05`, signed disp32 однозначно восстанавливается из decoded displacement. (`2083–2113`) |
| `S0_09NopXchgRaxRbxAdmission` (`229–233`) | XCHG RAX,RBX / S3-33i | `nop_rips`, `xchg_rips` | Qword XCHG RAX,RBX, 87, REX `48`, длина 3; guard сверяет `48 87 D8`; избыточна для связанной пары: opcode `87`, left RBX/right RAX и mod=11 задают `D8`. (`2114–2133`) |
| `S0_09NopAddRaxImm8Admission` (`237–241`) | ADD RAX,sign-ext imm8 / S3-33d | `nop_rips`, `add_rax_imm8_values: (rip,u64)` | Qword ADD immediate, 83, REX `48`, длина 4; guard сверяет `48 83 C0` и sign-extended imm8 с декодированным значением; избыточна для связанной пары: `83 /0`, mod=11 и RAX задают `C0`; sign extension однозначно задаёт imm8. (`1895–1921`) |
| `S0_09NopCmpRaxImm8Admission` (`252–256`) | CMP RAX,sign-ext imm8 / S3-33e | `nop_rips`, `cmp_rax_imm8_values: (rip,u64)` | Qword CMP immediate, 83, REX `48`, длина 4; guard сверяет `48 83 F8` и sign-extended payload с декодированным значением; избыточна для связанной пары: `83 /7`, mod=11 и RAX задают `F8`; sign extension однозначно задаёт imm8. (`1868–1894`) |
| `S0_09NopSubRaxImm8Admission` (`260–264`) | SUB RAX,sign-ext imm8 / S3-33f | `nop_rips`, `sub_rax_imm8_values: (rip,u64)` | Qword SUB immediate, 83, REX `48`, длина 4; guard сверяет `48 83 E8` и sign-extended payload с декодированным значением; избыточна для связанной пары: `83 /5`, mod=11 и RAX задают `E8`; sign extension однозначно задаёт imm8. (`1946–1972`) |
| `S0_09NopMovRaxImm32Admission` (`268–272`) | MOV RAX,sign-ext imm32 / S3-33k | `nop_rips`, `mov_rax_imm32_values: (rip,u64)` | Qword MOV immediate в RAX, C7, REX `48`, длина 7; guard сверяет `48 C7 C0` и sign-extended imm32 с декодированным значением; избыточна для связанной пары: `C7 /0`, mod=11 и RAX задают `C0`; sign extension однозначно задаёт imm32. (`1843–1867`) |
| `NativeRegisterV1Admission` (`277–281`) | Union S0-09, NOP и S3-33a–m | записи `{rip,next_rip,decoded,source_bytes,native}`, плюс `nop_rips` | Для записи выбирает общий S0-09 предикат или один из точных native helpers; байты сохраняются и используются в проверках 13 точных native-форм; guard LEA disp8 необходим для различения, остальные 12 избыточны при связанной паре (подробности в строках форм). Конструктор `1658+`, выбор `1775+`. |

**Основание классификации.** «Связанная пара» означает, что `Instruction` получен штатным `long_decode::decode` из тех же bytes, которые переданы helper: так конструкторы admission читают `CodeMemory` и исходный срез (`block.rs:346–366`, `block.rs:1670–1695`). Это не утверждение для произвольно составленного `Instruction` или несовпадающего с ним среза: guards тогда проверяют согласованность и сохраняются для эквивалентности всех вызовов. Все 13 расширенных форм фиксируют REX `48`, qword operand/address size и отсутствие LOCK, segment, 66 и 67. У каждой формы, кроме LEA disp8, обязательные opcode, ModRM/SIB и payload уже занимают точную длину, поэтому дополнительный префикс в ней не помещается. У LEA disp8 кодирование без displacement может освободить байт для дополнительного REX; декодер сохраняет последний REX (`long_decode.rs:2311–2353`). Декодер `rm_operand` извлекает mod/reg/rm и SIB, а также sign-extended displacement (`long_decode.rs:2253–2306`); его opcode cases задают направление и поля (`long_decode.rs:7978–8060`, `8127–8134`, `8310–8357`, `8387–8416`). В таблице приведено, какие поля фиксируют ModRM/SIB. Sign extension i8/i32 инъективна в допустимом диапазоне, поэтому decoded immediate/displacement восстанавливает соответствующий payload.

Для LEA disp8 есть конкретные коллизии: `48 8D 43 00` (mod=01, disp8=0), `48 8D 04 23` (mod=00, SIB без index и displacement) и `40 48 8D 03` (два REX, без displacement) декодируются в одинаковый `Instruction`: длина 4, RAX, base RBX, index отсутствует, scale 1, displacement 0. Локальный Rust-пробник, вызвавший `od_isa::long_decode::decode` из worktree `codex`, подтвердил равенство структур. Helper `is_exact_lea_rax_rbx_disp8` (`block.rs:2031–2058`) сравнивает raw byte 3 с decoded displacement и проверяет `48 8D 43`: первая кодировка проходит, оба других варианта отвергаются. Эти byte checks нужны сверх decoded-полей; они отделяют допущенную форму с disp8 от альтернативных вариантов.

Вывод об избыточности остальных guards ограничен указанным свойством связанной пары и существующим декодером. Он не является разрешением удалить сверки или расширить допуск.

### 2.2 BlockBuildProfile

`BlockBuildProfile` (`block.rs:17–39`) содержит 20 вариантов: 15 регистровых и 5 memory `Checked*`.

| Регистровый набор (15) | Checked* (5) |
|---|---|
| `S0_09`; `S0_09NativeIncRax`; `S0_09NativeDecRax`; `S0_09NativeNegRax`; `S0_09NativeNotRax`; `S0_09NativeLeaRaxRbx`; `S0_09NativeLeaRaxRbxDisp8`; `S0_09NativeLeaRaxRbxRcx`; `S0_09NativeLeaRaxRipDisp32`; `S0_09NativeXchgRaxRbx`; `S0_09NativeAddRaxImm8`; `S0_09NativeCmpRaxImm8`; `S0_09NativeSubRaxImm8`; `S0_09NativeMovRaxImm32`; `S0_09NativeRegisterV1` | `CheckedQwordLoad`; `CheckedDwordLoad`; `CheckedDwordStore`; `CheckedQwordStore`; `CheckedQwordMemoryPair` |

### 2.3 Функции регистрового пути в dispatch.rs

В текущем `dispatch.rs` обнаружено 32 публичных `run_with*` функции: 28 native и 4 interpreter fallback. Для каждой группы demand-вариант — имя с суффиксом `_demand_flags_candidate`.

| Группа | Eager | Demand flags |
|---|---|---|
| Bare NOP fallback | `run_with_nop_fallback` | `run_with_nop_fallback_demand_flags_candidate` |
| NOP + INC RAX interpreter handoff | `run_with_nop_inc_rax_fallback` | `run_with_nop_inc_rax_fallback_demand_flags_candidate` |
| `NativeRegisterV1` | `run_with_native_register_v1` | `run_with_native_register_v1_demand_flags_candidate` |
| INC RAX | `run_with_native_inc_rax` | `run_with_native_inc_rax_demand_flags_candidate` |
| DEC RAX | `run_with_native_dec_rax` | `run_with_native_dec_rax_demand_flags_candidate` |
| NEG RAX | `run_with_native_neg_rax` | `run_with_native_neg_rax_demand_flags_candidate` |
| NOT RAX | `run_with_native_not_rax` | `run_with_native_not_rax_demand_flags_candidate` |
| LEA RAX,[RBX] | `run_with_native_lea_rax_rbx` | `run_with_native_lea_rax_rbx_demand_flags_candidate` |
| LEA RAX,[RBX+disp8] | `run_with_native_lea_rax_rbx_disp8` | `run_with_native_lea_rax_rbx_disp8_demand_flags_candidate` |
| LEA RAX,[RBX+RCX] | `run_with_native_lea_rax_rbx_rcx` | `run_with_native_lea_rax_rbx_rcx_demand_flags_candidate` |
| LEA RAX,[RIP+disp32] | `run_with_native_lea_rax_rip_disp32` | `run_with_native_lea_rax_rip_disp32_demand_flags_candidate` |
| XCHG RAX,RBX | `run_with_native_xchg_rax_rbx` | `run_with_native_xchg_rax_rbx_demand_flags_candidate` |
| ADD RAX,imm8 | `run_with_native_add_rax_imm8` | `run_with_native_add_rax_imm8_demand_flags_candidate` |
| CMP RAX,imm8 | `run_with_native_cmp_rax_imm8` | `run_with_native_cmp_rax_imm8_demand_flags_candidate` |
| SUB RAX,imm8 | `run_with_native_sub_rax_imm8` | `run_with_native_sub_rax_imm8_demand_flags_candidate` |
| MOV RAX,imm32 | `run_with_native_mov_rax_imm32` | `run_with_native_mov_rax_imm32_demand_flags_candidate` |

Числа `31` и `32` описывают разные множества. S3-53 охватывает 28 `run_with_native_*`, два demand-варианта NOP/NOP+INC fallback и `run_cached_native_register_v1_demand_flags_candidate_controlled` — всего 31. Таблица выше содержит 32 `run_with*`: в неё входят также два eager fallback, но не входит cached demand. Полный `pub fn run*` до `#[cfg(test)]` — 43, включая cached и multi-cached API.

### 2.4 Методы admits_* и вызовы по файлам

В `block.rs` есть 29 объявлений с 15 различными именами: 15 реализаций `admits_nop` и 14 других методов. В последнем столбце дана одна и та же метрика для всех строк: число текстовых receiver call sites `.admits_<name>(` по написанию имени, включая тесты и вызовы через enum-wrapper. Порядок файлов: `block.rs / dispatch.rs / lowering.rs / block_compile.rs`. Это не распределение по типам получателя: 15 строк `admits_nop` повторяют общий счётчик имени `19/23/14/0`; суммировать его по строкам нельзя. Например, из четырёх вызовов по имени `admits_inc_rax` в `dispatch.rs` один вызывает метод конкретного admission, три вызывают `FallbackAdmission` wrapper. Скрипт ниже воспроизводит числа и перечень объявлений.

Каждый метод сам проверяет membership RIP в заранее построенном списке. Столбец «Правило отбора» связывает каждое объявление с decoded-условием и byte guard в таблице 2.1. Guard сохраняется в текущем множестве допуска; его необходимость для связанной пары классифицирована по форме в таблице 2.1.

| Объявление в block.rs | Правило отбора RIP: decoded-условие и byte guard | Call sites по файлам |
|---|---|---:|
| `S0_09NopAdmission::admits_nop` (413) | `is_exact_bare_nop`: decoded NOP, opcode `90`, без префиксов, len 1; byte `90`. При согласованном декодировании byte guard не нужен для различения, но сохраняется. | 19/23/14/0 |
| `S0_09NopIncRaxAdmission::admits_nop` (544) | `is_exact_bare_nop`: decoded NOP, opcode `90`, без префиксов, len 1; byte `90`. При согласованном декодировании byte guard не нужен для различения, но сохраняется. | 19/23/14/0 |
| `S0_09NopDecRaxAdmission::admits_nop` (631) | `is_exact_bare_nop`: decoded NOP, opcode `90`, без префиксов, len 1; byte `90`. При согласованном декодировании byte guard не нужен для различения, но сохраняется. | 19/23/14/0 |
| `S0_09NopNegRaxAdmission::admits_nop` (716) | `is_exact_bare_nop`: decoded NOP, opcode `90`, без префиксов, len 1; byte `90`. При согласованном декодировании byte guard не нужен для различения, но сохраняется. | 19/23/14/0 |
| `S0_09NopNotRaxAdmission::admits_nop` (802) | `is_exact_bare_nop`: decoded NOP, opcode `90`, без префиксов, len 1; byte `90`. При согласованном декодировании byte guard не нужен для различения, но сохраняется. | 19/23/14/0 |
| `S0_09NopLeaRaxRbxAdmission::admits_nop` (888) | `is_exact_bare_nop`: decoded NOP, opcode `90`, без префиксов, len 1; byte `90`. При согласованном декодировании byte guard не нужен для различения, но сохраняется. | 19/23/14/0 |
| `S0_09NopLeaRaxRbxDisp8Admission::admits_nop` (977) | `is_exact_bare_nop`: decoded NOP, opcode `90`, без префиксов, len 1; byte `90`. При согласованном декодировании byte guard не нужен для различения, но сохраняется. | 19/23/14/0 |
| `S0_09NopLeaRaxRbxRcxAdmission::admits_nop` (1070) | `is_exact_bare_nop`: decoded NOP, opcode `90`, без префиксов, len 1; byte `90`. При согласованном декодировании byte guard не нужен для различения, но сохраняется. | 19/23/14/0 |
| `S0_09NopLeaRaxRipDisp32Admission::admits_nop` (1159) | `is_exact_bare_nop`: decoded NOP, opcode `90`, без префиксов, len 1; byte `90`. При согласованном декодировании byte guard не нужен для различения, но сохраняется. | 19/23/14/0 |
| `S0_09NopXchgRaxRbxAdmission::admits_nop` (1252) | `is_exact_bare_nop`: decoded NOP, opcode `90`, без префиксов, len 1; byte `90`. При согласованном декодировании byte guard не нужен для различения, но сохраняется. | 19/23/14/0 |
| `S0_09NopAddRaxImm8Admission::admits_nop` (1344) | `is_exact_bare_nop`: decoded NOP, opcode `90`, без префиксов, len 1; byte `90`. При согласованном декодировании byte guard не нужен для различения, но сохраняется. | 19/23/14/0 |
| `S0_09NopCmpRaxImm8Admission::admits_nop` (1443) | `is_exact_bare_nop`: decoded NOP, opcode `90`, без префиксов, len 1; byte `90`. При согласованном декодировании byte guard не нужен для различения, но сохраняется. | 19/23/14/0 |
| `S0_09NopSubRaxImm8Admission::admits_nop` (1542) | `is_exact_bare_nop`: decoded NOP, opcode `90`, без префиксов, len 1; byte `90`. При согласованном декодировании byte guard не нужен для различения, но сохраняется. | 19/23/14/0 |
| `S0_09NopMovRaxImm32Admission::admits_nop` (1641) | `is_exact_bare_nop`: decoded NOP, opcode `90`, без префиксов, len 1; byte `90`. При согласованном декодировании byte guard не нужен для различения, но сохраняется. | 19/23/14/0 |
| `NativeRegisterV1Admission::admits_nop` (1752) | `is_exact_bare_nop`: decoded NOP, opcode `90`, без префиксов, len 1; byte `90`. При согласованном декодировании byte guard не нужен для различения, но сохраняется. | 19/23/14/0 |
| `admits_add_rax_rax` (460) | S3-32a ADD RAX,RAX: decoded-условие и byte guard строки формы в §2.1; для связанной пары byte guard избыточен согласно обоснованию в §2.1. | 0/0/2/1 |
| `admits_inc_rax` (547) | INC RAX: decoded-условие и byte guard строки формы в §2.1; для связанной пары byte guard избыточен согласно обоснованию в §2.1. | 1/4/2/0 |
| `admits_dec_rax` (634) | DEC RAX: decoded-условие и byte guard строки формы в §2.1; для связанной пары byte guard избыточен согласно обоснованию в §2.1. | 1/4/2/0 |
| `admits_neg_rax` (719) | NEG RAX: decoded-условие и byte guard строки формы в §2.1; для связанной пары byte guard избыточен согласно обоснованию в §2.1. | 1/4/2/0 |
| `admits_not_rax` (805) | NOT RAX: decoded-условие и byte guard строки формы в §2.1; для связанной пары byte guard избыточен согласно обоснованию в §2.1. | 1/3/2/0 |
| `admits_lea_rax_rbx` (891) | LEA RAX,[RBX]: decoded-условие и byte guard строки формы в §2.1; для связанной пары byte guard избыточен согласно обоснованию в §2.1. | 2/4/2/0 |
| `admits_lea_rax_rbx_disp8` (980) | LEA RAX,[RBX+disp8]: decoded-условие и byte guard строки формы в §2.1; byte guard необходим для различения disp8 от SIB без disp. | 2/4/1/0 |
| `admits_lea_rax_rbx_rcx` (1073) | LEA RAX,[RBX+RCX]: decoded-условие и byte guard строки формы в §2.1; для связанной пары byte guard избыточен согласно обоснованию в §2.1. | 2/4/2/0 |
| `admits_lea_rax_rip_disp32` (1169) | LEA RAX,[RIP+disp32]: decoded-условие и byte guard строки формы в §2.1; для связанной пары byte guard избыточен согласно обоснованию в §2.1. | 1/4/0/0 |
| `admits_xchg_rax_rbx` (1255) | XCHG RAX,RBX: decoded-условие и byte guard строки формы в §2.1; для связанной пары byte guard избыточен согласно обоснованию в §2.1. | 2/2/2/0 |
| `admits_add_rax_imm8` (1347) | ADD RAX,imm8: decoded-условие и byte guard строки формы в §2.1; для связанной пары byte guard избыточен согласно обоснованию в §2.1. | 1/4/1/0 |
| `admits_cmp_rax_imm8` (1446) | CMP RAX,imm8: decoded-условие и byte guard строки формы в §2.1; для связанной пары byte guard избыточен согласно обоснованию в §2.1. | 1/3/1/0 |
| `admits_sub_rax_imm8` (1545) | SUB RAX,imm8: decoded-условие и byte guard строки формы в §2.1; для связанной пары byte guard избыточен согласно обоснованию в §2.1. | 1/2/1/0 |
| `admits_mov_rax_imm32` (1645) | MOV RAX,imm32: decoded-условие и byte guard строки формы в §2.1; для связанной пары byte guard избыточен согласно обоснованию в §2.1. | 1/3/0/0 |

Количество вхождений считается по имени, а не по динамическому типу receiver. Поэтому таблица показывает и прямые вызовы метода, и forwarding через wrapper; ссылки на строки объявлений дают отдельный перечень всех 29 методов.

## 3. Baseline и перепроверка

Проверки выполнены в worktree `codex` на `b496d5e`; тесты запускались offline без изменения исходников.

| Команда | Фактический результат |
|---|---|
| `cargo test -p od-jit --lib --offline` | Успешно: 412 passed, 0 failed; предупреждение о неиспользуемом `one_instruction`. |
| `cargo test -p od-host --lib --offline` | 358 passed, 3 failed. Все три падения требуют отсутствующие локальные данные: source observations и `work/stage3-native-register-v1/manual/native-register-v1-cache-trace.json`; это не проверка кода с полным набором корпусных артефактов. |
| `python3 tools/check_layout.py` | Команда завершилась с 45 `ERROR` для отсутствующих локальных/игнорируемых артефактов и ссылок в отчётах; количество совпадает с baseline, указанным в плане. |

Числа, перепроверенные по текущим исходникам:

| Счётчик | Ожидалось в плане | Фактически | Основание |
|---|---:|---:|---|
| Admission-структуры в `block.rs` | 16 | 16 | `rg -n '^pub struct .*Admission' crates/od-jit/src/block.rs` |
| `admits_*` объявления | 29 | 29 | `rg -F -n 'pub(crate) fn admits_' crates/od-jit/src/block.rs` |
| `BlockBuildProfile` | 20 | 20 | варианты enum `block.rs:17–39` |
| `run*` до тестового модуля | 43 | 43 | `rg -n '^pub fn run' crates/od-jit/src/dispatch.rs` в production-секции |
| `run_with_native_*` | 28 | 28 | `rg -n '^pub fn run_with_native_' crates/od-jit/src/dispatch.rs` |
| Регистровые `run_with*` включая fallback | 31 | 32 | `rg -n '^pub fn run_with' crates/od-jit/src/dispatch.rs` |
| Demand native variants | 14 | 14 | суффикс `_demand_flags_candidate` у 14 native пар |
| `FallbackAdmission` variants | 15 | 15 | brace-aware count of enum variants in `dispatch.rs:2920–2936` |
| `Operation` variants | 402 | 406 | brace-aware подсчёт top-level variant declarations в `long_decode.rs`; число в плане расходится с текущей веткой |

В таблице фиксируются расхождения плана и проверенного состояния, а не исправляются нормативные числа без ревью. Три теста `od-host` нельзя считать проверенными без соответствующих локальных corpus artifacts. Layout-check показывает те же 45 ошибок, которые план задаёт как текущий baseline; задача требует не больше 45.

### Команды воспроизведения основных счётчиков

```sh
rg -c '^pub struct .*Admission' crates/od-jit/src/block.rs
rg -c 'pub\(crate\) fn admits_' crates/od-jit/src/block.rs
sed -n '18,37p' crates/od-jit/src/block.rs | wc -l
sed -n '2921,2935p' crates/od-jit/src/dispatch.rs | wc -l
python3 - <<'PY'
from pathlib import Path
import re
production = Path('crates/od-jit/src/dispatch.rs').read_text().split('#[cfg(test)]', 1)[0]
for pattern in (r'^pub fn run\w*', r'^pub fn run_with_native_\w*',
                r'^pub fn run_with_native_\w*demand_flags_candidate', r'^pub fn run_with\w*'):
    print(pattern, len(re.findall(pattern, production, re.M)))
PY
```

Подсчёт объявлений и receiver call sites из таблицы 2.4 (включая tests и wrapper calls):

```sh
python3 - <<'PY'
from collections import Counter
from pathlib import Path
import re
files = {name: Path(f'crates/od-jit/src/{name}.rs').read_text()
         for name in ('block', 'dispatch', 'lowering', 'block_compile')}
declarations = Counter(re.findall(r'pub\(crate\) fn (admits_\w+)\(', files['block']))
for name, count in declarations.items():
    calls = [len(re.findall(r'\.\s*' + re.escape(name) + r'\s*\(', text))
             for text in files.values()]
    print(name, 'declarations=', count, 'calls=', '/'.join(map(str, calls)))
PY
```

Brace-aware подсчёт вариантов `Operation` и `FallbackAdmission` можно воспроизвести в одном Python-фрагменте:

```sh
python3 - <<'PY'
from pathlib import Path
import re
def variants(path, name):
    source = Path(path).read_text()
    match = re.search(r'\benum\s+' + re.escape(name) + r'\b', source)
    start = source.index('{', match.end())
    depth = 0
    for end in range(start, len(source)):
        if source[end] == '{': depth += 1
        elif source[end] == '}':
            depth -= 1
            if depth == 0: break
    body = source[start + 1:end]
    return sum(bool(re.match(r'\s{4}[A-Z]\w*\b', line)) for line in body.splitlines())
print('Operation', variants('crates/od-isa/src/long_decode.rs', 'Operation'))
print('FallbackAdmission', variants('crates/od-jit/src/dispatch.rs', 'FallbackAdmission'))
PY
```

Скрипт считает только top-level строки enum-вариантов с отступом четыре пробела и uppercase identifier: в `codex` это 406 `Operation` (план указывает 402) и 15 `FallbackAdmission`.

## 4. S3-51 — единые правила допуска

Статус: **независимое архитектурное ревью Sol пройдено, замечания исправлены**; коммит `8dcaac8` в `codex`, Graphify обновлён. В [§3a JIT_CONTRACT](../specs/JIT_CONTRACT.md#3a-единые-правила-допуска-регистрового-набора) записаны положительные правила выбранного профиля, привязка к полному `CodeView`, действующие byte guards, все пять `S0_09NopAdmissionError` и оба предела `BlockLimits`. Memory-path и exact fallback `NOT RCX` оставлены отдельно; NOP в admission не объявлен native-формой builder. Код и действующее множество форм не менялись.

### Построчная сверка с инвентарём S3-50

В следующей таблице `Base` означает `B-MOV`, `B-ALU`, `B-JCC`, `NOP` из §3a контракта. Для колонок dispatcher запись `f{, demand}` обозначает **обе** строки таблицы S3-50: `f` и `f_demand_flags_candidate`. Для каждой из 15 строк `Type::admits_nop` правило — `NOP`; второй метод строки ссылается на указанное дополнительное правило. Одна admission-структура INC показана двумя строками из-за разных builder-путей. Таким образом перечислены все 16 admission-структур, 15 register `BlockBuildProfile`, 32 функции register dispatcher (16 пар) и 29 объявлений `admits_*`; последний столбец показывает правило или границу, а не заявляет нового допуска.

| Строка admission S3-50 | Register build profile | Dispatcher S3-50 | `admits_*` S3-50 | Правило §3a |
|---|---|---|---|---|
| `S0_09NopAdmission` | `S0_09` | `run_with_nop_fallback{, demand}` | `S0_09NopAdmission::admits_nop` | `Base`; builder останавливается на `NOP` для fallback |
| `S3_32aAddRaxRaxAdmission` | собственной разновидности нет; `S0_09` | отдельной `run_with_native_*` пары нет | `admits_add_rax_rax` | `Base` и обязательное наличие `ADD-RAX` |
| `S0_09NopIncRaxAdmission` (legacy fallback) | `S0_09` | `run_with_nop_inc_rax_fallback{, demand}` | `S0_09NopIncRaxAdmission::admits_nop`; `admits_inc_rax` | `Base` + `INC` admission; обычный builder, допущенный INC исполняется интерпретатором при переходе в fallback |
| `S0_09NopIncRaxAdmission` (native opt-in) | `S0_09NativeIncRax` | `run_with_native_inc_rax{, demand}` | те же `admits_nop`; `admits_inc_rax` | `Base` + `INC` admission; native INC builder |
| `S0_09NopDecRaxAdmission` | `S0_09NativeDecRax` | `run_with_native_dec_rax{, demand}` | `S0_09NopDecRaxAdmission::admits_nop`; `admits_dec_rax` | `Base` + `DEC` |
| `S0_09NopNegRaxAdmission` | `S0_09NativeNegRax` | `run_with_native_neg_rax{, demand}` | `S0_09NopNegRaxAdmission::admits_nop`; `admits_neg_rax` | `Base` + `NEG` |
| `S0_09NopNotRaxAdmission` | `S0_09NativeNotRax` | `run_with_native_not_rax{, demand}` | `S0_09NopNotRaxAdmission::admits_nop`; `admits_not_rax` | `Base` + `NOT` |
| `S0_09NopLeaRaxRbxAdmission` | `S0_09NativeLeaRaxRbx` | `run_with_native_lea_rax_rbx{, demand}` | `S0_09NopLeaRaxRbxAdmission::admits_nop`; `admits_lea_rax_rbx` | `Base` + `LEA-RBX` |
| `S0_09NopLeaRaxRbxDisp8Admission` | `S0_09NativeLeaRaxRbxDisp8` | `run_with_native_lea_rax_rbx_disp8{, demand}` | `S0_09NopLeaRaxRbxDisp8Admission::admits_nop`; `admits_lea_rax_rbx_disp8` | `Base` + `LEA-D8`, в том числе обязательный byte guard |
| `S0_09NopLeaRaxRbxRcxAdmission` | `S0_09NativeLeaRaxRbxRcx` | `run_with_native_lea_rax_rbx_rcx{, demand}` | `S0_09NopLeaRaxRbxRcxAdmission::admits_nop`; `admits_lea_rax_rbx_rcx` | `Base` + `LEA-SIB` |
| `S0_09NopLeaRaxRipDisp32Admission` | `S0_09NativeLeaRaxRipDisp32` | `run_with_native_lea_rax_rip_disp32{, demand}` | `S0_09NopLeaRaxRipDisp32Admission::admits_nop`; `admits_lea_rax_rip_disp32` | `Base` + `LEA-RIP` |
| `S0_09NopXchgRaxRbxAdmission` | `S0_09NativeXchgRaxRbx` | `run_with_native_xchg_rax_rbx{, demand}` | `S0_09NopXchgRaxRbxAdmission::admits_nop`; `admits_xchg_rax_rbx` | `Base` + `XCHG` |
| `S0_09NopAddRaxImm8Admission` | `S0_09NativeAddRaxImm8` | `run_with_native_add_rax_imm8{, demand}` | `S0_09NopAddRaxImm8Admission::admits_nop`; `admits_add_rax_imm8` | `Base` + `ADD-I8` |
| `S0_09NopCmpRaxImm8Admission` | `S0_09NativeCmpRaxImm8` | `run_with_native_cmp_rax_imm8{, demand}` | `S0_09NopCmpRaxImm8Admission::admits_nop`; `admits_cmp_rax_imm8` | `Base` + `CMP-I8` |
| `S0_09NopSubRaxImm8Admission` | `S0_09NativeSubRaxImm8` | `run_with_native_sub_rax_imm8{, demand}` | `S0_09NopSubRaxImm8Admission::admits_nop`; `admits_sub_rax_imm8` | `Base` + `SUB-I8` |
| `S0_09NopMovRaxImm32Admission` | `S0_09NativeMovRaxImm32` | `run_with_native_mov_rax_imm32{, demand}` | `S0_09NopMovRaxImm32Admission::admits_nop`; `admits_mov_rax_imm32` | `Base` + `MOV-I32` |
| `NativeRegisterV1Admission` | `S0_09NativeRegisterV1` | `run_with_native_register_v1{, demand}` | `NativeRegisterV1Admission::admits_nop` | `Base` + все 13 дополнительных правил |

Остальные пять строк `BlockBuildProfile` из таблицы S3-50 сверены отдельно: `CheckedQwordLoad`, `CheckedDwordLoad`, `CheckedDwordStore`, `CheckedQwordStore` и `CheckedQwordMemoryPair` относятся к отдельному memory-path и не принимаются этим register-контрактом. Это не отказ их собственных memory API. Таблица положительных форм контракта перечисляет только имевшиеся в S3-50 predicates с теми же условиями и byte guards; остальные формы, в том числе `NOT RCX` на native register пути и data-memory sites, получают явный отказ. По именам механически сверены 16/16 admission-структур, 20/20 профилей, 32/32 функций и 29/29 методов без пропусков; затем условия строк сопоставлены с predicates. Это документальная сверка, а не параметризованный корпус эквивалентности, который остаётся задачей S3-52.

Проверки и ограничения: `python3 tools/check_layout.py` из worktree `codex` после изменений завершился с 45 `ERROR` (baseline S3-50 — 45), без ошибок для изменённых документов; `git diff --check` чист. CPU-тесты для документационного среза не запускались. Независимое архитектурное ревью выполнено Sol; коммит `8dcaac8`, Graphify обновлён.

## 5. S3-52 — один механизм допуска

Статус: реализация и корпус проверены; независимое ревью Sol одобрено, коммит `dd9c9bb` в `codex`, Graphify обновлён. Публичные admission-структуры и их `admits_*` методы сохранены. Один full-CodeView scanner декодирует и классифицирует регистровые формы, проверяет полный набор branch boundaries и проецирует записи в прежние RIP/value списки; builder register path использует тот же classifier с отдельной политикой NOP handoff. `Checked*` memory branches оставлены отдельными. Декодированная запись хранит диапазон исходных байтов внутри witness, без отдельной копии на инструкцию.

Корпус [S3-52](../testing/JIT.md#s3-52-register-admission-equivalence) сохраняет под `cfg(test)` прежние 16 admission loops и register builder decision как независимый oracle; точные поформенные предикаты остаются общими и дополнительно проверяются фиксированными byte/class ожиданиями. Выполненный целевой прогон дал `comparisons=2452 mismatches=0`. Сопоставляются полные wrapper results и значения, source binding, builder outcomes и отказы; defensive `InvalidLength`/`RangeOverflow` проверены через synthetic validation seam. Формы и кодировки намеренно не расширялись.

Проверки на `codex`: `cargo fmt --all -- --check` и `cargo check -p od-jit -p od-host --all-targets --offline` прошли; `od-jit` — 413 passed, один отдельный microbenchmark проигнорирован в обычном наборе; `od-host` — 361 passed после появления локальных corpus artifacts. В базовом прогоне S3-50 три теста `od-host` не прошли из-за отсутствовавших artifacts. `python3 tools/check_layout.py` показывает прежние 45 ошибок отсутствующих ссылок, новых ошибок изменённых документов нет; `git diff --check` чист. Корпус эквивалентности дал 2452 сравнения и ноль расхождений. Независимое ревью Sol одобрено; коммит `dd9c9bb` в `codex`, Graphify обновлён. Измерение ниже использует адаптированный legacy proxy и не воспроизводит исходный allocation/lifetime baseline для NativeRegisterV1.

### Измерение full-CodeView scanner

Сравнивались адаптированный legacy scanner и новый scanner в одном release binary на одинаковых immutable code views; результаты проверялись на равенство перед замером. Legacy сохраняет pre-S3-52 циклы допуска, но запись `NativeRegisterV1` использует новый `source_range`: временный `source_bytes` освобождается после обработки инструкции, тогда как исходная реализация сохраняла отдельный `Vec` в каждой записи до освобождения witness. Поэтому legacy в benchmark — proxy прежнего scanner, а не точная копия исходного распределения аллокаций и времени жизни данных. MacBook Pro, Apple M3 Max (10 performance + 4 efficiency cores), 36 GB, macOS 26.7.1, arm64, Rust/Cargo 1.99.0, стандартный `--release`. Команда и состав входов приведены в [методике](../testing/JIT.md#s3-52-register-admission-equivalence). В каждом запуске — девять пар с чередованием порядка, по 200 сканирований на пару.

Приведены все восемь запусков в миллисекундах на 200 сканирований. Серия A — первоначальная release-сборка: A1 session `85081`, A2–A4 session `19918`. Серия B — session `86110`, после добавления только test-only проверок baseline-class и пересборки test binary; production-код scanner, benchmark-функция и входы не менялись. В таблице указаны медиана и наблюдаемые min–max девяти выборок для каждого варианта, не доверительные интервалы. В исходном выводе парные дельты отдельно не сохранялись.

| Запуск | Вход | Legacy proxy: медиана [min–max] | Новый scanner: медиана [min–max] |
|---|---|---|---|
| A1 | `base_128` | 10.384 [5.128–12.590] | 10.825 [4.917–12.521] |
| A1 | `base_inc_128` | 6.085 [5.871–11.820] | 5.499 [5.109–8.181] |
| A1 | `base_lea_disp8_128` | 8.064 [6.396–10.027] | 8.048 [6.484–12.361] |
| A1 | `union_130` | 10.349 [7.698–15.625] | 12.510 [7.063–14.610] |
| A2 | `base_128` | 6.655 [5.159–10.006] | 7.341 [4.988–10.956] |
| A2 | `base_inc_128` | 8.231 [6.728–12.737] | 8.462 [7.177–13.330] |
| A2 | `base_lea_disp8_128` | 6.090 [5.904–8.607] | 6.281 [5.796–7.107] |
| A2 | `union_130` | 7.831 [7.537–16.776] | 6.036 [5.819–8.952] |
| A3 | `base_128` | 5.107 [4.895–5.794] | 5.074 [4.928–5.348] |
| A3 | `base_inc_128` | 11.604 [5.772–12.721] | 11.605 [5.262–12.417] |
| A3 | `base_lea_disp8_128` | 6.049 [5.721–12.465] | 5.864 [5.506–10.075] |
| A3 | `union_130` | 9.329 [7.887–14.853] | 7.137 [6.119–10.855] |
| A4 | `base_128` | 5.181 [5.027–8.910] | 5.140 [4.825–10.986] |
| A4 | `base_inc_128` | 6.020 [5.652–6.891] | 5.460 [5.161–6.523] |
| A4 | `base_lea_disp8_128` | 6.897 [5.713–12.026] | 5.970 [5.506–10.896] |
| A4 | `union_130` | 7.888 [7.582–41.476] | 6.182 [5.804–12.163] |
| B1 | `base_128` | 5.175 [4.987–5.576] | 5.182 [4.804–6.506] |
| B1 | `base_inc_128` | 7.070 [5.953–11.113] | 7.516 [5.465–12.432] |
| B1 | `base_lea_disp8_128` | 7.029 [5.690–11.875] | 6.795 [5.477–10.636] |
| B1 | `union_130` | 8.874 [7.719–10.688] | 6.521 [6.140–16.676] |
| B2 | `base_128` | 5.489 [4.989–6.159] | 5.385 [4.847–6.106] |
| B2 | `base_inc_128` | 6.250 [5.847–7.325] | 5.790 [5.358–7.105] |
| B2 | `base_lea_disp8_128` | 6.073 [5.752–7.671] | 5.953 [5.518–7.024] |
| B2 | `union_130` | 7.817 [7.589–8.005] | 6.220 [5.791–6.604] |
| B3 | `base_128` | 5.092 [4.870–5.809] | 5.093 [4.926–5.365] |
| B3 | `base_inc_128` | 5.932 [5.682–6.766] | 5.371 [5.100–6.008] |
| B3 | `base_lea_disp8_128` | 6.835 [6.305–9.305] | 6.590 [5.841–7.500] |
| B3 | `union_130` | 7.990 [7.567–9.155] | 6.399 [6.053–6.780] |
| B4 | `base_128` | 5.496 [5.204–5.927] | 5.505 [5.006–6.093] |
| B4 | `base_inc_128` | 6.340 [5.668–6.999] | 5.636 [5.329–6.862] |
| B4 | `base_lea_disp8_128` | 5.927 [5.633–6.019] | 5.654 [5.516–5.850] |
| B4 | `union_130` | 9.212 [7.695–12.406] | 6.969 [6.074–11.240] |

Медиана `union_130` в A1 выросла с 10.349 до 12.510 мс, а в A2–A4 и B1–B4 была ниже у нового scanner. В большинстве пар наблюдаемые диапазоны пересекаются; для `union_130` B2 и B3 они не пересекаются. Из-за адаптированного legacy proxy, отсутствия сохранённых парных дельт и неконтролируемой нагрузки эти наблюдения не устанавливают разницу с исходной реализацией, общий выигрыш или отсутствие регрессии. Замер относится только к синтетическому scanner с прогретыми исполняемыми и входными данными; память, реальное JIT-исполнение и cold/warm cache не измерялись. Полное измерение generic пути остаётся задачей S3-56.

## 6. S3-53 — единая точка входа диспетчера

Статус: независимое ревью Sol одобрено; коммит `8aab6f3` в `codex`, Graphify обновлён. Проверены все 31 wrapper, 33 целевых теста, форматирование и сборка; полный `od-jit` — 446 passed/2 ignored, `od-host` — 361 passed при наличии локальных artifacts, layout — 45 baseline ошибок. Измерение с mock backend не является проверкой производительности полного JIT.

В `dispatch.rs` частная `run_register_dispatch` принимает профиль и режим flags отдельно. `RegisterDispatchProfile` связывает существующий тип admission с режимом исполнения формы; `InterpretedIncRax` сохраняет обычный S0-09 builder и interpreter fallback, а `NativeIncRax` выбирает native builder. `UncachedRegisterPath` передаёт профиль и mode существующему циклу; `SingleEntryRegisterPath` типизированно передаёт `NativeRegisterV1` в прежний single-entry cache loop. Публичные сигнатуры 31 обёртки сохранены. Оставшиеся cached/multi/memory точки входа не переводились на этот путь.

Для 28 native обёрток ниже каждый столбец — отдельный тест `dispatch::tests::s3_53_<имя>`, сравнивающий публичный вызов с прямым `run_register_dispatch` и прежней стратегией на той же admitted форме и следующем базовом ADD. Сравниваются результат, всё состояние, emitted bytes, входные бюджеты и число вызовов executor; дополнительно фиксируются ожидаемые retirement и NOP handoff.

| Профиль | Eager test suffix | Demand test suffix |
|---|---|---|
| NativeRegisterV1 | `native_register_v1` | `native_register_v1_demand_flags_candidate` |
| INC RAX | `native_inc_rax` | `native_inc_rax_demand_flags_candidate` |
| DEC RAX | `native_dec_rax` | `native_dec_rax_demand_flags_candidate` |
| NEG RAX | `native_neg_rax` | `native_neg_rax_demand_flags_candidate` |
| NOT RAX | `native_not_rax` | `native_not_rax_demand_flags_candidate` |
| LEA RAX,[RBX] | `native_lea_rax_rbx` | `native_lea_rax_rbx_demand_flags_candidate` |
| LEA RAX,[RBX+disp8] | `native_lea_rax_rbx_disp8` | `native_lea_rax_rbx_disp8_demand_flags_candidate` |
| LEA RAX,[RBX+RCX] | `native_lea_rax_rbx_rcx` | `native_lea_rax_rbx_rcx_demand_flags_candidate` |
| LEA RAX,[RIP+disp32] | `native_lea_rax_rip_disp32` | `native_lea_rax_rip_disp32_demand_flags_candidate` |
| XCHG RAX,RBX | `native_xchg_rax_rbx` | `native_xchg_rax_rbx_demand_flags_candidate` |
| ADD RAX,imm8 | `native_add_rax_imm8` | `native_add_rax_imm8_demand_flags_candidate` |
| CMP RAX,imm8 | `native_cmp_rax_imm8` | `native_cmp_rax_imm8_demand_flags_candidate` |
| SUB RAX,imm8 | `native_sub_rax_imm8` | `native_sub_rax_imm8_demand_flags_candidate` |
| MOV RAX,imm32 | `native_mov_rax_imm32` | `native_mov_rax_imm32_demand_flags_candidate` |

Остальные три wrapper-теста — `s3_53_nop_fallback_demand`, `s3_53_nop_inc_fallback_demand` и `s3_53_cached_native_register_v1_demand_wrapper`. Cached-тест сравнивает cold/hit и замену eager/demand по состоянию, результату, counters, publish/execute/drop и compile-flavor isolation. `s3_53_direct_entry_preserves_control_and_source_binding` отдельно проверяет STOP address/request, нулевой budget, event и отказ на изменённый CodeView; `s3_53_direct_entry_preserves_branch_and_nop_handoff` проверяет ветвление с последующим NOP.

Целевой прогон `cargo test -p od-jit --lib --offline s3_53_` — 33 passed, 0 failed: 31 тест обёрток и два общих сценария. `cargo fmt --all -- --check` и `cargo check -p od-jit -p od-host --all-targets --offline` прошли; полный `od-jit` — 446 passed, 2 ignored (два отдельных microbenchmark), 0 failed. `od-host` — 361 passed, 0 failed, при наличии локальных corpus artifacts; его прогон выполнен до добавления test-only benchmark, после которого повторный `cargo check --all-targets` прошёл. `python3 tools/check_layout.py` завершился с 45 прежними `ERROR`, новых ошибок в изменённых документах и данных нет. `git diff --check` чист.

### Измерение издержек маршрутизации

Для решения на критичном dispatch-пути выполнен ограниченный paired microbenchmark: прежний прямой вызов существующего strategy-loop и новый `run_register_dispatch` используют одинаковые admitted bytes и один и тот же loop в одном release binary. Сценарии — interpreter NOP, uncached native INC с mock executor и прогретый single-entry demand cache с mock backend. Это не измерение исполнения ARM64/JIT и не сравнение полного приложения. В каждой из трёх отдельных сессий выполнялись 9 пар, порядок old/new чередовался; числа включают сброс тестового состояния. NOP — 10 000 вызовов на выборку, uncached INC — 250, cached demand — 1 000. Команда воспроизведения:

```sh
cargo test -p od-jit --lib --release --offline s3_53_dispatch_entry_microbench -- --ignored --nocapture
```

Конфигурация: Apple M3 Max, 36 GiB RAM, macOS 26.7.1/Darwin 25.6.0 arm64, rustc 1.99.0 (`b940084d7`), cargo 1.99.0, стандартный Cargo release profile. Полные парные значения наносекунд приведены в [CSV](data/stage3/s3-53-dispatch-entry-2026-11-11.csv). Ниже — медиана и наблюдаемый min–max за 9 выборок; delta — медиана и min–max парных `(unified − legacy) / legacy`, не доверительный интервал.

| Сессия / сценарий | Legacy, мс | Unified, мс | Парная delta, % |
|---|---:|---:|---:|
| A / NOP | 3.389 [3.359–3.481] | 3.408 [3.360–3.499] | +0.58 [−1.74…+2.78] |
| A / uncached INC | 0.738 [0.716–0.843] | 0.726 [0.716–0.837] | −1.03 [−9.64…+13.43] |
| A / cached demand | 0.581 [0.569–0.688] | 0.584 [0.569–0.679] | +0.41 [−5.59…+2.13] |
| B / NOP | 3.396 [3.348–3.459] | 3.384 [3.326–3.405] | −0.33 [−2.17…+0.24] |
| B / uncached INC | 0.762 [0.730–0.849] | 0.756 [0.737–0.807] | +0.34 [−8.54…+1.72] |
| B / cached demand | 0.587 [0.567–0.667] | 0.574 [0.571–0.658] | +0.03 [−6.03…+3.75] |
| C / NOP | 3.330 [3.300–3.361] | 3.320 [3.309–3.355] | −0.13 [−0.69…+0.63] |
| C / uncached INC | 0.728 [0.717–0.790] | 0.727 [0.718–0.747] | −0.57 [−5.81…+1.28] |
| C / cached demand | 0.578 [0.569–0.661] | 0.578 [0.568–0.664] | −0.23 [−5.68…+4.04] |

Парные изменения малы относительно разброса и меняют знак между сессиями; эти данные не подтверждают ни ускорение, ни отсутствие регрессии на реальной нагрузке. Память отдельно не измерялась. Полное сравнение cold/warm JIT остаётся задачей S3-56.

## 7. S3-54 — generic путь и multi-entry cache

Статус: независимое ревью Sol одобрено; коммит `6c438a8` в `codex`, Graphify обновлён. Добавлен `run_multi_cached_native_register_v1_controlled` через общую точку `run_register_dispatch`, `MultiEntryRegisterPath` и существующие `MultiEntryBlockCache`/`MultiBlockCachedBackend`. Состав `TranslationKey`, алгоритмы FIFO/LRU и существующие memory/fixture entrypoints не изменены. Существующий native cached loop обобщён на `CachedEntryStore`; после `record_hit` он заново находит ключ, поскольку LRU может переместить запись. `reset_for_execution_context` принимает native cached fixture; legacy fixture dispatcher по-прежнему отвергает native profile.

Тест `s3_54_register_corpus_cold_warm_counters_and_flag_isolation` прогоняет 15 отдельных bounded code views (baseline, S3-32a и 13 native форм) в каждом режиме флагов; mock backend отслеживает опубликованные handles. Счётчики на каждый режим:

| Прогон | hits | misses | successful_compiles | successful_publications | evictions |
|---|---:|---:|---:|---:|---:|
| Cold | 0 | 15 | 15 | 15 | 0 |
| После warm | 15 | 15 | 15 | 15 | 0 |

Отдельные тесты проверяют изоляцию eager/demand ключей; FIFO/LRU последовательность A→B→A→C и идентичность исполненного handle после перемещения LRU; capacity по entries, code bytes и metadata, oversized candidate, backend capacity mismatch и отказ публикации; смену dependency version, `CodeInvalidation::BackingContentChanged` и `reset_for_execution_context`. Новая версия dependency даёт другой ключ и miss; старая запись остаётся resident до явной invalidation или eviction. Удаление и reset вызываются между синхронными dispatch-вызовами.

ARM64 host-тест `s3_54_native_multi_cache_reuses_published_handle` выполняется на macOS arm64 с реальным `PublishedArenaBackend`: для FIFO/LRU и eager/demand cold вызов публикует handle, warm исполняет тот же `(slot_index, generation)` без новой публикации, а полное состояние сверяется с `od_isa::execute::step`. Для каждого из четырёх сочетаний cold/warm дают суммарно 1 hit, 1 miss, 1 compile, 1 publication, 0 evictions. Целевой host-тест прошёл.

Проверки в worktree `codex`: `cargo fmt --all -- --check` и `cargo check -p od-jit -p od-host --all-targets --offline` прошли; `cargo test -p od-jit --lib --offline` — 450 passed, 2 ignored; целевые четыре S3-54 unit tests — 4 passed; `cargo test -p od-host --lib --offline` — 362 passed; `python3 tools/check_layout.py` — 45 прежних ошибок локальных ссылок. Hit-rate не является оценкой ускорения; производительность и cold/warm измерения полного generic JIT остаются задачей S3-56. Production owner и MMU integration остаются открыты в S3-18.

## 8. S3-55 — interpreter fallback

**Статус (2026-11-11): ограниченный mixed маршрут реализован и прошёл независимое Sol-ревью; коммит `31380a5`.** Opt-in `MixedRegisterV1Admission` хранит полные границы и native-классы прежнего сканера, отдельно разрешая только точные однобайтовые CLD (`FC`) и STD (`FD`) как interpreter sites. Строгий `NativeRegisterV1Admission` по-прежнему их отвергает. Существующий NOP handoff сохранён. Для этих helper выполняется `od_isa::execute::step` с fetch-only видом в один байт; memory/profile-required формы не заявлены. После шага проверяются привязка кода, CR3, контекст cache и зависимости. Нулевой прогресс и отказы возвращаются типизированно; helper entry исполняется без попытки компиляции. Metadata отдельно считает native-инструкции при компиляции, вызовы helper и успешные шаги интерпретатора. Два новых compile flavor изолируют mixed eager/demand от строгого пути.

Контракт описывает ограниченный маршрут, не общий Stage 2 fallback и не production owner/MMU integration. ARM64 host-сравнение полностью проверяет состояние и retirement для ADD → NOP → CLD → STD → ADD и ADD → CLD → ADD → STD → ADD на FIFO/LRU, eager/demand и cold/warm. Второй поток проверяет native terminal непосредственно перед обоими helper. Тесты проверяют strict reject и точность mixed witness, отсутствие компиляции helper entry, budget в том числе ноль, STOP/event, типизированный нулевой прогресс native и interpreter, injected отказ interpreter и memory error, пост-helper отказ при изменении source, invalid dependencies, CR3 и устаревшем context generation. `cargo test -p od-jit --lib --offline`: 455 passed, 2 ignored; `cargo test -p od-host --lib --offline`: 362 passed; `cargo fmt --all -- --check`, `cargo check -p od-jit -p od-host --all-targets --offline` и `git diff --check` прошли. `python3 tools/check_layout.py` возвращает 45 существующих ошибок ссылок на локальные отсутствующие артефакты, как baseline S3-50. Независимое Sol-ревью кода, тестов и документации одобрено; подтверждённых дефектов и незакрытых замечаний нет.

## 9. S3-56 — измерения generic пути

**Статус (2026-11-11): частично; независимое Sol-ревью выполненного объёма пройдено, коммит `795be59`.** Расширены существующие `cold-first-run` и `warm-first-run` флагом `--generic-register`; default поведение сохранено. Измерены interpreter, generic cold и generic warm на одном resident native-only корпусе. S3-56 не принят: для сравнения cold на исходном корпусе S3-35a отсутствуют замороженные inputs, а исторические S3-35a и S3-36b использовали разные корпуса ([ISSUE-075](../KNOWN_ISSUES.md#issue-075-s3-56-historical-corpus-comparability)). Это измерение выполнено до реализации ограниченного mixed fallback S3-55.

**Методика и вход.** Пять S3-14 branch/helper fixtures, 5 visits и 14 retired guest instructions на sweep; список SHA-256 `dd590c6c8f8ae1a09f2192e47c1d019f7449c814212c87e06194bf7425dd0881`, trace SHA-256 `272eb80c3283e71962f1bc2a2e6d847723164d48f56dd3970134a814df57d2c8`. Это те же hashes и case IDs, что у S3-36b. Фикстуры, запись trace, outcome, retirement и NativeRegisterV1 admission проверяются до тайминга. Native-only preflight отвергает bare NOP — единственный текущий interpreter handoff этого профиля — и каждый успешный generic dispatch проверяет `retired_interpreter == 0`. Поэтому fallback helper calls = 0 выведено из проверенной eligibility и результата, а не снято отдельным инструментированным счётчиком; машинные helpers реализации флагов эта величина не считает.

Измерены три независимых release процесса для каждого режима на Apple M3 Max, arm64, macOS 26.7.1, `rustc 1.99.0 (b940084d7 2026-09-28)`, target `aarch64-apple-darwin`. Модель CPU подтверждена отдельным `sysctl -n machdep.cpu.brand_string`; JSON процесса фиксирует `unavailable` из-за sandbox sysctl. Build fingerprint `ba466b09bce22ed2e0f3edcfab93cbb9c155b235147392d8c328df24352e588c`, исходный HEAD `b41755065c7817215903aefc2b487aae976b2f96`, дерево во время сборки dirty из-за S3-56. Каждая серия имеет 20 чередующихся пар с общим sweep count и минимумом 200 ms на сторону; короткая серия сбрасывается и повторяется существующим калибратором. Cold включает fresh arena/cache initialization и dispatch, warm использует один предварительно прогретый cache; загрузка фикстур/admission, клонирование и проверки результатов вне временных интервалов. В каждом измеренном generic `Variant::call` также находятся выбор benchmark-ветки, создание `CodeView`/dependency/context и stop byte, поэтому это не latency одного лишь dispatch. Полная методика и команда — в [BENCHMARKS.md](../testing/BENCHMARKS.md).

| Режим / процесс | Generic median [min–max], IQR, µs/sweep | Interpreter median [min–max], IQR, µs/sweep |
|---|---:|---:|
| Cold 1 | 36.995 [35.963–40.037], 1.450 | 2.964 [2.918–3.050], 0.060 |
| Cold 2 | 39.009 [36.659–42.530], 2.540 | 3.077 [2.968–3.680], 0.089 |
| Cold 3 | 36.319 [35.985–36.701], 0.288 | 2.993 [2.968–3.052], 0.041 |
| Warm 1 | 5.024 [4.909–7.926], 0.040 | 2.982 [2.912–3.040], 0.029 |
| Warm 2 | 5.019 [4.987–5.080], 0.040 | 2.978 [2.963–3.038], 0.012 |
| Warm 3 | 5.024 [4.964–5.142], 0.056 | 2.968 [2.958–3.020], 0.020 |

Диапазоны min–max трёх процессов пересекаются в каждом режиме; отдельные медианы различаются, а min–max не являются доверительным интервалом. В каждом generic cold sweep: 14 native retired, 0 interpreter steps/fallback helper calls, 0 hits и 5 misses/compiles/publications, 0 evictions. Warm prewarm: 5 misses/compiles/publications; каждый измеренный sweep: 14 native retired, 0 interpreter steps/fallback helper calls, 5 hits и 0 misses/compiles/publications/evictions, hit-rate 1.0. Interpreter выполняет 14 steps на sweep. Это подтверждено для всех записанных samples.

На этой машине и этих пяти случаях generic медленнее interpreter: cold медианы примерно 12.1–12.7×, warm примерно 1.68–1.69×. Возможные причины — стоимость lookup/key, публикации и переходов через JIT; это гипотеза без фазовой атрибуции. Исторический S3-36b использовал тот же list/trace и те же единицы: warm FIFO медианы 3.736–3.959 µs против нынешних 5.019–5.024 µs; interpreter тогда 0.726–0.751 µs против нынешних 2.968–2.982 µs. Тела измерительных `interpreter_sweep`, `measure_interpreter_sample`, `summarize_first_run` идентичны historical revision `836fbfee`, но между ревизиями существенно изменились `od-isa/src/execute.rs` и decoder. Данные получены на одном CPU model/OS/toolchain, но разные revisions и текущая сборка dirty; сравнение фиксирует различие наблюдений, не устанавливает причину или регрессию самого dispatch. Исторический cold S3-35a — 6.640–6.694 ms и interpreter 0.164–0.166 ms на 1 010 случаях/1 322 visits — **несопоставим** с этими пятью случаями и не используется для вывода об изменении скорости.

Сырые данные: [cold 1](data/stage3/s3-56/cold-1.json), [cold 2](data/stage3/s3-56/cold-2.json), [cold 3](data/stage3/s3-56/cold-3.json), [warm 1](data/stage3/s3-56/warm-1.json), [warm 2](data/stage3/s3-56/warm-2.json), [warm 3](data/stage3/s3-56/warm-3.json). `cargo test -p od-bench --offline`: 67 lib + 6 CLI passed; `cargo fmt --all -- --check` и `cargo check -p od-bench --all-targets --offline` прошли. `python3 tools/check_layout.py` сообщает прежние 45 ошибок и не сообщает новых ошибок изменённых документов; `git diff --check` чист. Эти результаты не принимают S3-35/S3-36 или этап 3.

## 10. S3-57 — финальное независимое ревью линии

**Статус: частично.** Независимый `gpt-6.1-sol` high reviewer проверил diff
линии `b496d5e..31380a5`: точность допуска, отказов и границ, register
dispatch, cache/invalidation, fallback/progress и раздельный учёт, а также
сырые данные и сопоставимость измерений. Подтверждённых дефектов не найдено.
S3-57 зависит от S3-56 и не принимается до закрытия ISSUE-075 и недоступного
исторического корпуса. Этап 3 остаётся открытым до S3-41.

Повтор на чистом `31380a5` в `codex`: `cargo fmt --all -- --check`,
`cargo check -p od-jit -p od-host --all-targets --offline` прошли; `od-jit`
455 passed / 2 ignored, `od-host` 362 passed, `od-bench` 67 lib и 6 CLI
passed. Отдельный корпус admission дал 2 452 сравнения и 0 расхождений.
`python3 tools/check_layout.py` вернул 45 ранее зафиксированных ошибок ссылок
на отсутствующие локальные артефакты; новых ошибок не обнаружено. Исторические
основные счётчики S3-50 повторно проверены на `b496d5e`: 16 admission,
29 объявлений `admits_*`, 43 `run*`, 28 native, 14 demand и 32 `run_with*`.
После рефакторинга часть счётчиков на текущей ревизии ожидаемо изменилась.

На том же пятислучайном корпусе S3-56 повторены по два независимых release
процесса каждого режима на чистом `31380a5`, с методикой раздела 9. В таблице
медиана [min–max], IQR в мкс на sweep; raw JSON фиксирует ревизию, fingerprint,
20 пар измерений и счётчики:

| Режим / процесс | Generic | Interpreter |
|---|---:|---:|
| Cold 1 | 36.979 [36.319–38.235], 0.911 | 2.995 [2.972–3.100], 0.058 |
| Cold 2 | 37.339 [36.592–39.277], 1.738 | 3.003 [2.972–3.081], 0.050 |
| Warm 1 | 5.118 [5.067–5.200], 0.060 | 2.991 [2.969–3.070], 0.049 |
| Warm 2 | 5.065 [5.019–5.130], 0.035 | 2.981 [2.965–3.032], 0.018 |

Сырые данные: [cold 1](data/stage3/s3-57/cold-1.json),
[cold 2](data/stage3/s3-57/cold-2.json), [warm 1](data/stage3/s3-57/warm-1.json),
[warm 2](data/stage3/s3-57/warm-2.json). Диапазоны повторов пересекаются с
записанными в разделе 9; это не доказывает равенство производительности между
ревизиями. Generic остаётся медленнее interpreter на этом корпусе. Повтор не
восстанавливает inputs S3-35a и не закрывает требуемое историческое сравнение.
