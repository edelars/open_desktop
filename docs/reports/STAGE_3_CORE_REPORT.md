# Этап 3 Core — промежуточный отчёт

Статус: **промежуточный. Этап 3 НЕ принят; линия S3 Core выполняется.** Этот отчёт фиксирует отправную точку на `b496d5e` и результаты S3-50 в ветке `codex`. Отчёт не подтверждает приёмку этапа 3.

**Текущий статус (2026-11-11):** S3-50 прошёл независимое ревью и закоммичен в ветке `codex` как `80a0fe6`; Graphify обновлён. [Отчёт и инвентарь](STAGE_3_CORE_REPORT.md): необходимость byte guards классифицирована по формам. `od-jit` — 412 passed; `od-host` — 358 passed и 3 failed из-за отсутствующих локальных corpus artifacts; layout-check — 45 ошибок baseline.

Связанные документы: [план S3 Core](../plans/STAGE_3_CORE.md), [основной план этапа 3](../plans/STAGE_3.md), [контракт JIT](../specs/JIT_CONTRACT.md), [карта проекта](../PROJECT_LAYOUT.md).

## 1. Сводка линии

| Задача | Статус на этой ревизии |
|---|---|
| S3-50 — отчёт и инвентарь дублирования | Ревью пройдено; коммит `80a0fe6`, Graphify обновлён |
| S3-51 — единые правила допуска | Независимое архитектурное ревью Sol пройдено; коммит будет зафиксирован следующим шагом |
| S3-52…S3-57 | not_started |

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

Разница `31` против `32` в плане и коде оставлена открытой для согласования: код содержит по два NOP fallback API и два NOP+INC fallback API. Из исходника нельзя заключить, является ли ожидаемое `31` устаревшим числом или арифметической ошибкой. Полный `pub fn run*` до `#[cfg(test)]` — 43, включая эти 32, cached и multi-cached API.

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

Статус: **независимое архитектурное ревью Sol пройдено, замечания исправлены**; коммит будет зафиксирован следующим шагом. В [§3a JIT_CONTRACT](../specs/JIT_CONTRACT.md#3a-единые-правила-допуска-регистрового-набора) записаны положительные правила выбранного профиля, привязка к полному `CodeView`, действующие byte guards, все пять `S0_09NopAdmissionError` и оба предела `BlockLimits`. Memory-path и exact fallback `NOT RCX` оставлены отдельно; NOP в admission не объявлен native-формой builder. Код и действующее множество форм не менялись.

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

Проверки и ограничения: `python3 tools/check_layout.py` из worktree `codex` после изменений завершился с 45 `ERROR` (baseline S3-50 — 45), без ошибок для изменённых документов; `git diff --check` чист. CPU-тесты для документационного среза не запускались. Независимое архитектурное ревью выполнено Sol; коммит ещё не создан.

## 5. S3-52 — один механизм допуска

Статус: not_started.

## 6. S3-53 — единая точка входа диспетчера

Статус: not_started.

## 7. S3-54 — generic путь и multi-entry cache

Статус: not_started.

## 8. S3-55 — interpreter fallback

Статус: not_started.

## 9. S3-56 — измерения generic пути

Статус: not_started.

## 10. S3-57 — финальное независимое ревью линии

Статус: not_started.
