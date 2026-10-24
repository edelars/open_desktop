# Собственные schema v2 fixtures этапа 2

Байты написаны вручную по Intel SDM revision 093: Vol.2B `MOV—Move`,
Vol.2A `ADD—Add`, Vol.2A §§2.1.5 и 2.2.1.2 (ModR/M/SIB и REX),
Vol.1 §3.4.1.1 (GPR/subregister aliases). Нормативная ссылка:
[Intel SDM](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html).
Это собственные короткие программы, а не вывод assembler/encoder или
копия чужой CPU-реализации. Provenance записан также в каждом fixture.

Базовые fixtures ниже используют strict schema v2 / `s2-long-integer-v1`;
последующие local-only segment profiles описаны в отдельных разделах ниже.
V1 fixtures не меняются.
Начальные GPR/RIP/flags, memory bytes, permissions, comparison masks и
stop address записаны lossless в `.case.json`. Digest вычисляется по
точным JSON bytes при чтении; исторические v1 файлы и `SHA256SUMS` не
изменены. Fixtures не являются результатами физического oracle.

| Файл | Программа и ручной ожидаемый результат |
|---|---|
| [mov_load_add_store.case.json](mov_load_add_store.case.json) | `MOV RAX,[RBX+1]; ADD RAX,RCX; MOV [RBX+1],RAX`. При RBX=`0xffd` qword по `0xffe` пересекает 4 KiB и два смежных RAM regions. Из `7 + 5` получаются RAX=`12`, data=`0c00000000000000`, RFLAGS=`0x406`, RIP=`0x400b`, retired=`3`; остальные GPR не меняются. |
| [mov_width_aliasing.case.json](mov_width_aliasing.case.json) | Loads/stores через AL/AH/AX/EAX. До EAX верхние биты RAX сохраняются; EAX zero-extends. Последовательные load values: RAX=`0x1122334455667711`, `0x1122334455662211`, `0x1122334455664433`, `0x0000000088776655`. Destination bytes `1122334455667788`, flags=`0x402`, RIP=`0x4019`, retired=`8`. |
| [mov_rex_rip_relative.case.json](mov_rex_rip_relative.case.json) | RIP-relative load в R8D; register MOV в R9D; store через R12+disp; immediate в R10B; `MOV SPL,AL` с REX. Результат: R8=R9=`0x44332211`, R10=`0x112233445566777f`, RSP=`0x7034`, destination bytes=`0011223344`, flags=`0x402`, RIP=`0x4015`, retired=`5`. |

Для каждого файла:

```sh
target/debug/od-runner --fixture tests/fixtures/x86/stage2/mov_load_add_store.case.json --engine interpreter --json
```

`tools/runner/tests/fixture_v2.rs` запускает настоящий CLI и проверяет
перечисленные ручные результаты, все 16 GPR, digest и final memory.
`crates/od-testkit/src/execution_tests.rs` отдельно проверяет промежуточные
alias values и injected read/store faults: faulting инструкция не
retire, предшествующие инструкции сохраняются, частичная запись в первый
region при ошибке второго не допускается. Это локальные CPU/fixture-RAM
проверки S2-08; физическая приёмка среза A требует S2-09.

## Отдельные входы для физического Linux oracle S2-09

`hw_*.case.json` — новые входы с новыми case IDs и digest. Они сохраняют
темы S2-08, но явно задают пригодный для Linux layout: RX/readable code
по `0x100000`, readable data от `0x200000`, RW stack по `0x300000`
длиной 256 bytes и начальный RSP=`0x3000f8`. Старые fixtures выше не
переписаны. Адреса и pointer-valued GPR/results сравниваются буквально;
normalization и замена входов после исполнения не допускаются.

| Новый fixture / case ID | Явные отличия layout и ручные ожидаемые значения |
|---|---|
| [hw_mov_load_add_store.case.json](hw_mov_load_add_store.case.json) / `s2_09_hw_mov_load_add_store` | RBX=`0x200ffd`, qword по `0x200ffe` пересекает 4 KiB и logical regions `data_left`/`data_right`. RAX=`12`, flags=`0x406`, RIP=`0x10000b`, retired=`3`; data bytes=`0c00000000000000`, stack не меняется. |
| [hw_mov_width_aliasing.case.json](hw_mov_width_aliasing.case.json) / `s2_09_hw_mov_width_aliasing` | RBX=`0x200000`; destination по `0x200008`. RAX=`0x88776655`, flags=`0x402`, RIP=`0x100019`, retired=`8`; destination bytes=`1122334455667788`. |
| [hw_mov_rex_rip_relative.case.json](hw_mov_rex_rip_relative.case.json) / `s2_09_hw_mov_rex_rip_relative` | RIP-relative displacement буквально `0x000ffff9` ведёт от следующего RIP=`0x100007` к source=`0x200000`; R12=`0x210000`. R8=R9=`0x44332211`, R10=`0x112233445566777f`, RSP после SPL write=`0x300034`, destination bytes=`0011223344`, flags=`0x402`, RIP=`0x100015`, retired=`5`. |

Native profile ограничивает mappings low canonical addresses, readable
regions и отсутствие W+X. Страничное округление не расширяет logical
fixture ranges; snapshot содержит только записанные region bytes.
Разные permissions у logical regions одной host page дают явный
unsupported. Полный 16-GPR snapshot включает настоящий гостевой RSP.

Для двух interpreter/native прогонов трёх новых cases:

```sh
python3 tests/cpu/stage2_oracle.py --host 192.168.1.14 --user senya \
  --remote-root /home/senya/projects/open_desktop-stage1 --repeats 2
```

Harness требует одинаковые source fingerprints локально и на oracle;
исходники сам не синхронизирует. Он замораживает точные JSON bytes до
сборки, собирает local interpreter и remote native runner, передаёт
только архив выбранных fixtures и сравнивает через существующий Rust
v2 comparator. Повторы сравниваются отдельно для обоих engines. Raw
stdout/stderr/return codes, batch payload, inputs/digests/seeds, build
metadata, machine metadata, hashes harness/helper и reproduction argv
сохраняются в `work/stage2-oracle/<run-id>/manifest.json` и соседних
файлах. Любой SSH/build/unsupported/missing-result/mismatch проваливает
выбранный suite с сохранённым failed manifest. JIT нового профиля не
запускается. Запуск этой команды требует разрешённой передачи новых
fixture payload на выбранный oracle.

Старый v1 corpus (1010 cases дважды) по-прежнему запускается существующим
stage1 harness. Его прошедший oracle manifest можно привязать через
`--stage1-manifest work/.../manifest.json`; обязательны тот же Rust source
fingerprint, ≥1010 уникальных inputs и все interpreter/JIT/native
сравнения минимум двух повторов. Без этой опции результат обозначает
только memory suite и явно записывает отсутствие legacy evidence;
полная приёмка S2-09 всё равно требует старый corpus. Наличие команды и
локальных unit tests не подтверждает физический прогон.

Проверки самой orchestration без SSH:

```sh
python3 -m unittest discover -s tests/cpu -p 'test_stage*_harness.py' -v
```

## Data-transfer fixtures S2-10

Пять новых `s2_10_*.case.json` — самостоятельные ручные входы с layout
code=`0x100000`, data/results от `0x200000`, stack=`0x300000`.
Intel SDM revision 093: Vol.2B MOV pp.4-28–4-31, MOVSX/MOVSXD
pp.4-120–4-123 и MOVZX pp.4-130–4-131; Vol.2A LEA
pp.3-547–3-548; Vol.2D XCHG pp.6-32–6-33. Точные ссылки на формы
есть в provenance каждого входа. Все инструкции сохраняют исходные
проверяемые flags=`0xcd7`. Ручные expected значения проверяются реальным
interpreter в `crates/od-testkit/src/execution_tests.rs`; сами fixtures
ещё не являются свидетельством физического прохождения.

| Fixture | Наблюдаемая проверка |
|---|---|
| [s2_10_immediate_moffs.case.json](s2_10_immediate_moffs.case.json) | 34 инструкции, stop=`0x1000dc`. C6/C7 memory stores 8/16/32/64, AH/word/dword/qword register immediates и C7 RIP-relative с immediate внутри next RIP. Moffs A0–A3: все widths 8/16/32/64 при address32 и address64. Каждый load сохраняет полный RAX в отдельный qword results, каждый store пишет отдельный 16-byte slot с нетронутыми canaries `5a`/`a5`, поэтому последующая инструкция не скрывает ошибочную ширину записи. Итог RAX=R9=`0xffffffff80000001`, R8=`0x80000000`, RCX=`0xaabbccddeeff8000`; RIP-relative destination содержит `78563412`. |
| [s2_10_extend.case.json](s2_10_extend.case.json) | 34 инструкции, stop=`0x100089`. MOVZX/MOVSX: source8→dest16/32/64 и source16→dest32/64; MOVSXD: 16→16, 32→32 и 32→64. Memory и register sources включают AH, REX SPL и R8B. После каждой формы полный destination сохраняется отдельным qword, включая верхние биты после narrow writes. |
| [s2_10_addr32_transfers.case.json](s2_10_addr32_transfers.case.json) | 38 инструкций, stop=`0x1000b3`. Все memory extension формы предыдущего входа и четыре C6/C7 widths используют `67` с RBX=`0x1122334400200000`, то есть усечение до EBX=`0x200000` необходимо для успеха. Отдельные 16-byte canary slots различают ширину каждого store; промежуточные результаты extension сохраняются. |
| [s2_10_lea_no_read.case.json](s2_10_lea_no_read.case.json) | 12 инструкций, stop=`0x10003b`. Все шесть destination16/32/64 × address32/64 сочетаний, SIB, signed displacement, RIP-relative unmapped result=`0x101020`. Noncanonical RBX=`0x0000800000000000` и отсутствие data mappings проверяют отсутствие чтения памяти. FS/GS-prefixed LEA оставляют R12=R15=`0x0000800000000000` без прибавления segment base. Каждый результат остаётся в отдельном GPR. |
| [s2_10_xchg_aliases.case.json](s2_10_xchg_aliases.case.json) | 18 инструкций, stop=`0x10002c`. Register XCHG byte/word/dword/qword, AL/AH simultaneous old values, REX SPL, extended registers; `90`, `40 90`, `48 90` сохраняют верх RAX (checkpoint R10=`0x1122334455667788`), `41 90` и `87 c0` дают dword zero-extension. Промежуточные обмены наблюдаемы через RSP/RDX/R9–R13. Memory XCHG в этот срез не входит. |

Для отдельного S2-10 suite (после разрешённой синхронизации исходников
и передачи этих пяти fixtures):

```sh
python3 tests/cpu/stage2_oracle.py --host 192.168.1.14 --user senya \
  --remote-root /home/senya/projects/open_desktop-stage1 --repeats 2 \
  tests/fixtures/x86/stage2/s2_10_*.case.json
```

Default suite S2-09 остаётся прежним: три `hw_*` входа. Старые fixture
bytes и исторический `SHA256SUMS` не изменяются. Для S2-10 нужно новое
физическое evidence на точных новых JSON bytes и актуальном source
fingerprint; прежний S2-09 manifest его не заменяет.

## Arithmetic fixtures S2-11

`s2_11_*.case.json` — 313 новых bounded входов. Каждый исполняет ровно
одну арифметическую инструкцию и останавливается на следующем RIP:
результат и все шесть arithmetic flags остаются наблюдаемы в final
snapshot. Начальные DF=1 и reserved bit1=1 также проверяются; прежние
OF/SF/ZF/AF/PF/CF намеренно задаются отличающимися от многих результатов.
Expected literal sidecar [s2_11_expected.json](s2_11_expected.json)
содержит полные 16 GPR, flags и точные изменяемые memory regions.
Он не является альтернативным fixture format и не подаётся в runner.

| Семья | Проверяемые формы и границы |
|---|---|
| ADD/ADC/SUB/SBB/CMP | widths8/16/32/64; оба ModR/M направления register/register, memory/register и register/memory; dedicated accumulator immediate; group80/81 register и memory; group83 register и memory для16/32/64. |
| Carry и flags | Signed overflow каждого width, unsigned carry/borrow, ADC/SBB с обоими входными CF, auxiliary carry отдельно от whole-width carry, zero/nonzero, odd/even low-byte parity, sign. Negative imm8 проверяет sign-extension для16/32/64, negative imm32 — group81 и accumulator64. |
| INC/DEC/NEG | FE/FF register и memory widths8/16/32/64, INC/DEC сохраняют оба входных CF, signed overflow и unsigned wrap/borrow. F6/F7 NEG проверяет zero, one и minimum signed operand каждого width. |
| Aliases и адреса | AL/AH общего RAX, REX SPL и R8D/R9D с dword zero-extension. Каждая семья/ширина дополнительно использует address32 с RBX=`0x1122334400200ffe`, реально усечённым до EBX=`0x200ffe`. Это ограниченная явная матрица форм, а не исчерпывающий cross-product всех16 GPR и addressing encodings. |
| Memory | Data access по `0x200ffe`, logical regions `data_left`/`data_right` стыкуются по `0x201000`: widths32/64 пересекают и logical границу, и host page. Вокруг operand находятся неизменяемые `a5` canaries. Memory sources и CMP destinations read-only; CMP не должен записывать даже прежнее значение. Full-byte masks покрывают весь logical region. |

Это ручные opcode/ModR/M templates и literal input/output/flags tables,
материализуемые [generate_stage2_arithmetic.py](../../../cpu/generate_stage2_arithmetic.py).
Генератор упаковывает числа и JSON, не вычисляет арифметику, flags или
CPU semantics и не использует assembler/encoder/emulator. Provenance
каждого входа содержит SHA256 генератора. Нормативные формы: Intel SDM
revision093, [Vol.2A](https://cdrdv2.intel.com/v1/dl/getContent/671199)
ADC pp.3-9–3-11, ADD pp.3-14–3-15, CMP pp.3-161–3-162,
DEC pp.3-262–3-263, INC pp.3-457–3-458;
[Vol.2B](https://cdrdv2.intel.com/v1/dl/getContent/671241)
NEG pp.4-158–4-159, SBB pp.4-613–4-615, SUB pp.4-685–4-686;
Vol.1 AppendixA TableA-2 определяет affected flags.

Локальная проверка точных materialized bytes и независимых expected:

```sh
python3 tests/cpu/generate_stage2_arithmetic.py --check
cargo test -p od-testkit --offline arithmetic_
```

Testkit исполняет каждый checked-in input настоящим interpreter и
сравнивает full state/RIP/count/flags/memory с literal sidecar. Отдельный
тест инъецирует architectural read/write #PF и backend failures во
второй logical region для ADD/ADC/SUB/SBB/INC/DEC/NEG: faulting instruction
не retire, GPR/flags/RIP и весь RAM остаются прежними. Это локальный
точный fault-state test. Native positive profile отклоняет недопустимые
ranges/permissions до физической инструкции, а host signal не считается
guest exception; физическое подтверждение guest #PF этим набором не
заявляется. LOCK/REP и FS/GS data arithmetic остаются Unsupported.

После отдельного разрешения передачи нового source/fixture payload:

```sh
python3 tests/cpu/stage2_oracle.py --host 192.168.1.14 --user senya \
  --remote-root /home/senya/projects/open_desktop-stage1 --repeats 2 \
  tests/fixtures/x86/stage2/s2_11_*.case.json
```

Runner использует реальные hardware GPR для admission адресов; arithmetic
expected evaluator в physical runner отсутствует. Последующий manifest
с точными digest/source fingerprint должен отдельно подтвердить
физический результат; наличие fixtures и локальный interpreter PASS
его не заменяют. Старые S2-08–10 inputs/default suite не изменяются.

## Logic fixtures S2-12

`s2_12_*.case.json` — 197 новых входов с отдельным literal sidecar
[s2_12_expected.json](s2_12_expected.json). Все граничные формы имеют
одну логическую инструкцию и stop на следующем RIP; два дополнительных
сценария проверяют последовательное состояние definedness.

| Семья | Матрица и наблюдаемый результат |
|---|---|
| AND/OR/XOR | Widths8/16/32/64, оба ModR/M направления для register/register и register/memory, memory destinations, accumulator immediate, group80/81 register/memory, group83 register/memory для16/32/64. Negative imm8 и64-bit negative imm32 проверяют sign-extension. Zero/nonzero, positive/negative и low-byte parity имеют независимые literal ответы; CF/OF из исходных1 сбрасываются в0. |
| TEST | Только реальные формы84/85, A8/A9 иF6/F7 /0; register/register, read-only memory, register/memory immediate, address32. Destinations, включая полные верхние32 бита RAX после TEST EAX, остаются прежними. Negative imm32 с minimum qword различает sign-extension от zero-extension. |
| NOT | F6/F7 /2 register/memory/address32 для8/16/32/64. Все flags сохраняются при исходных all-set modeled flags, CF0 и all-zero modeled flags (reserved bit1 остаётся1). 8/16-bit destination сохраняет верх GPR,32-bit destination zero-extends. |
| Aliases/layout | Явные AL/AH общего RAX, REX SPL и R8D/R9D. Address32 требует усечения noncanonical RBX до EBX=`0x200ffe`. Widths32/64 пересекают logical regions и4KiB; `a5` canaries и полные memory masks делают ошибочную ширину записи наблюдаемой. Memory source/TEST read-only, изменяемые destinations R+W. |
| Definedness | Для AND/OR/XOR/TEST AF архитектурно undefined: fixture value mask и final semantic `rflags_defined` равны `0xcc7` вместо `0xcd7`. Значение AF не является expected ответом. `XOR; MOV; NOT` сохраняет undefined AF; `XOR; ADD` вновь определяет AF и возвращает mask=`0xcd7`. NOT сохраняет mask. |

[generate_stage2_logic.py](../../../cpu/generate_stage2_logic.py)
упаковывает ручные byte templates и literal answers. Проверенные S2-11
входы используются только как явный layout baseline; SHA256 baseline и
самого генератора записаны в каждом новом provenance. Ни старые входы,
ни их generator/expected bytes не изменяются. Logic/reference evaluator,
assembler или сторонняя CPU реализация не используются. Нормативные
формы и flags: Intel SDM revision093, Vol.2 instruction entries
AND/OR/XOR/TEST/NOT и Vol.1 AppendixA TableA-2.

```sh
python3 tests/cpu/generate_stage2_logic.py --check
cargo test -p od-testkit --offline logic_
```

Локальные tests сравнивают все197 actual interpreter runs с полными
literal GPR/memory/RIP/count, semantic flags mask и только определёнными
flags values. Injected read/write architectural/backend faults для
AND/OR/XOR/NOT и read faults TEST подтверждают отсутствие retirement,
изменения состояния, partial RAM write или definedness при ошибке.
Native admission возвращает shared instruction flag effects без
вычисления результата; tracer применяет их только после подтверждённого
TRAP_TRACE. Linux native tests отдельно проверяют TEST readonly и
preservation/restoration definedness. Они требуют реального Linux x86
исполнения; host signals не заменяют guest exceptions.

После отдельного разрешения передачи конкретных новых файлов:

```sh
python3 tests/cpu/stage2_oracle.py --host 192.168.1.14 --user senya \
  --remote-root /home/senya/projects/open_desktop-stage1 --repeats 2 \
  tests/fixtures/x86/stage2/s2_12_*.case.json
```

Это ограниченная явная матрица форм, не cross-product всех GPR/addressing
encodings. Физическое evidence появляется только после реального
native/interpreter сравнения exact bytes/source fingerprint. Ограничение
positive native profile по проверке guest #PF остаётся тем же, что у
S2-11; локальный injected fault test не выдаётся за физическое #PF.

## Shift and rotate fixtures S2-13

`s2_13_*.case.json` — 144 однокомандных сценария. Литеральная таблица
[s2_13_answers.json](s2_13_answers.json) задаёт входы, результаты и defined
flags; [generate_stage2_shifts.py](../../../cpu/generate_stage2_shifts.py)
только упаковывает эти строки в schema-v2 fixtures и
[s2_13_expected.json](s2_13_expected.json), не вычисляя сдвиги, вращения или
flags. SHL/SAL, SHR, SAR, ROL, ROR, RCL и RCR покрывают widths8/16/32/64;
SHLD/SHRD — widths16/32/64. Каждая пара kind/width проверяет raw counts 0,
1, width и width+1, с immediate и CL формами, кроме SHLD/SHRD word count17.
При этом count17 после маскирования превышает ширину слова, а результат
назначения архитектурно неопределён; v2 сравнивает полный GPR и не может
исключить эти биты. Эта пара остаётся в локальной no-panic/definedness проверке
без сравнения destination. RCL/RCR count=1 проверяется с CF-in=0 и1 для каждой
ширины. Для32/64 raw width и width+1 после архитектурной маски могут стать
zero или one; эти случаи проверяют именно masked behavior.

Две дополнительные memory fixtures выполняют SHL qword и SHLD qword с
destination по `0x200ffe`, пересекая logical regions `data_left`/`data_right`.
Native admission unit checks подтверждают, что RMW отклоняется без write
permission или вне logical range. Это проверка границы harness, не результат
физического guest page fault. Count-zero dword cases начинают с ненулевой
верхней половиной RAX и проверяют, что запись в EAX zero-extends в RAX[63:32],
даже когда эффективный count равен нулю; flags при этом остаются прежними.

Нормативные формы и flags: Intel SDM revision093, Vol.2A SAL/SAR/SHL/SHR
pp.4-582–4-585, Vol.2B RCL/RCR/ROL/ROR pp.4-521–4-524, SHLD pp.4-640–4-641,
SHRD pp.4-643–4-644 и Vol.1 AppendixA TableA-2.

```sh
python3 tests/cpu/generate_stage2_shifts.py --check
cargo test -p od-testkit --offline shift_rotate_fixtures
```

Физическое evidence требует native/interpreter сравнения этих точных JSON
bytes и source fingerprint на Linux x86-64.

## Direct control flow fixtures S2-16a

`s2_16a_*.case.json` содержит 68 буквальных long-mode сценариев: все 16
legacy condition codes для short и near Jcc в taken/fallthrough вариантах,
плюс четыре direct JMP с forward/backward rel8/rel32. Generator
[`generate_stage2_control_flow.py`](../../../cpu/generate_stage2_control_flow.py)
упаковывает ручные opcode bytes, входные flags и literal full-state answers;
условия и targets не вычисляются общей библиотекой или assembler. Provenance
ссылается на Intel SDM revision 093 Vol.2A Jcc pp. 3-499–3-502 и JMP
pp. 3-504–3-505.

`od-isa` проверяет все 32 сочетания CF/PF/ZF/SF/OF против literal truth table,
signed displacement от next RIP, 64-bit RIP wrap, canonical target fault,
exact fetch boundaries и Unsupported prefixes. Testkit и CLI runner сверяют
полное состояние и sidecar. Native-v2 admission проверяет только выбранный
taken/fallthrough target. Физический mini-check отложен до общего финального
подэтапа.

```sh
python3 tests/cpu/generate_stage2_control_flow.py --check
cargo test -p od-isa -p od-testkit -p od-runner --offline direct_control_flow
```

## Memory stack `66` + `REX.W` fixtures S2-17m

Four `.case.json` files cover `PUSH 66 48 FF /6` and `POP 66 48 8F /0`.
`REX.W` selects qword width over the earlier `66` prefix. The inputs check
ordinary memory source/destination and `[RSP]` alias ordering: PUSH reads from
the pre-decrement address; POP computes its destination from post-increment
RSP. Literal full-state answers are in
[`s2_17m_memory_stack_rexw_expected.json`](s2_17m_memory_stack_rexw_expected.json)
and are materialized by
[`generate_stage2_stack_rexw_override.py`](../../../cpu/generate_stage2_stack_rexw_override.py).
The native runner admission test accepts all four fixtures and checks that
effective REX ordering is honored while LOCK and FS/GS-prefixed forms remain
outside this profile. The fixtures await the shared final physical mini-check.

```sh
python3 tests/cpu/generate_stage2_stack_rexw_override.py --check
cargo test -p od-isa -p od-testkit --offline memory_stack_rexw
cargo test -p od-x86-runner --offline rexw_overrides_operand_override
```

## Register stack `66` + REX.W fixtures S2-17n

Six literal fixtures cover register PUSH/POP with an operand-size override and
an effective REX prefix. Qword forms confirm that REX.W overrides `66`; word
forms place a REX prefix with W=1 before the final REX with W=0, confirming
that only the final REX immediately before the opcode is effective. Extended
R8/R9 forms cover REX.B, and POP fixtures check preservation of upper register
bits for word destinations. Literal full-state answers are stored in
[`s2_17n_register_stack_rexw_expected.json`](s2_17n_register_stack_rexw_expected.json)
and materialized by
[`generate_stage2_stack_rexw_register.py`](../../../cpu/generate_stage2_stack_rexw_register.py).
The native admission test validates all bounded stack reads and writes. These
fixtures await the shared final physical mini-check.

Normative source: Intel SDM revision 093, Vol.2A §2.2.1.2 pp.2-8–2-9 and
Vol.2B PUSH pp.4-522–4-523, POP pp.4-398–4-399.

```sh
python3 tests/cpu/generate_stage2_stack_rexw_register.py --check
cargo test -p od-isa -p od-testkit --offline register_stack_rexw
cargo test -p od-x86-runner --offline mixed_prefix_register_stack
```
## POPFQ fixtures S2-17o

Two literal fixtures cover bare `POPFQ` (`9D`) under the native runner's
default CPL3/IOPL0 context. Only CF/PF/AF/ZF/SF/DF/OF are writable in this
profile; bit 1 is preserved by the literal fixtures. IF/IOPL and other
out-of-mask preservation are asserted by unit tests. Inputs or initial state
with TF, NT, or AC set are outside this slice and return `Unsupported` without committing CPU state. A stack access
fault also leaves CPU state and RFLAGS definedness unchanged. The literal
answers are materialized by
[`generate_stage2_popfq.py`](../../../cpu/generate_stage2_popfq.py).

Normative source: Intel SDM revision 093, Vol.2B POPF/POPFQ pp.4-407–4-410.
The implemented CPL3/IOPL0 privilege contract is deliberately limited; other
IOPL/CPL combinations, other POPF forms, and other PUSHF width/prefix variants remain open.

## PUSHFQ fixture S2-17p

One literal fixture covers bare long-mode `PUSHFQ` (`9C`). The instruction
writes a qword `RFLAGS AND 00000000_00FCFFFFH` at `RSP - 8`, then commits RSP;
RFLAGS is unchanged. The local full-state answer is literal. The memory
comparison mask checks the profile status flags, DF, fixed bit 1, and cleared
RF/VM bits, while ignoring IF/IOPL and other bits that the CPL3 fixture harness
cannot set to arbitrary values. Unit tests separately exercise the exact mask
for arbitrary interpreter state and no-commit on a bounded write failure.

The answer is materialized by
[`generate_stage2_pushfq.py`](../../../cpu/generate_stage2_pushfq.py).
Normative source: Intel SDM revision 093, Vol.2B
PUSHF/PUSHFD/PUSHFQ pp.4-528–4-529.

```sh
python3 tests/cpu/generate_stage2_pushfq.py --check
cargo test -p od-isa -p od-testkit --offline pushfq
cargo test -p od-x86-runner --offline pushfq_fixture_admits_only_a_bounded_writable_stack_slot
```

```sh
python3 tests/cpu/generate_stage2_popfq.py --check
cargo test -p od-isa -p od-testkit --offline popfq
cargo test -p od-runner --offline popfq_cli_fixtures
cargo test -p od-x86-runner --offline popfq_fixtures_admit
```

## POPFW fixture S2-17r

One literal fixture covers exact long-mode `66 9D` POPFW at CPL3/IOPL0.
The word loads CF/PF/AF/ZF/SF/DF/OF, while IF, IOPL, bit 1 and other
unmodeled RFLAGS bits remain unchanged. A source/current TF bit or source/current NT
bit returns `Unsupported` before CPU commit. Nonzero current IOPL also returns
`Unsupported` before the stack read. NT=1 is kept outside the profile because Intel SDM revision 093 Vol.2B Table 4-15 and Vol.1 §7.3.13.2 p.7-22 conflict;
see [ISSUE-015](../../../../docs/KNOWN_ISSUES.md#issue-015--разногласие-sdm-по-nt-для-popf-word-на-cpl3iopl0).
The fixture uses TF=NT=0; its v2 literal comparison checks RFLAGS through mask
`0xCD7`. A separate local testkit assertion checks exact preservation of
unmasked IF, reserved, ID, and upper bits with current IOPL=0 and source IOPL=3.
Native admission accepts only the bounded readable word stack input and rejects
TF/NT profile violations and nonzero current IOPL.

The answer is materialized by
[`generate_stage2_popfw.py`](../../../cpu/generate_stage2_popfw.py).
Normative source: Intel SDM revision 093, Vol.2B Table 4-15 pp.4-395–4-396
and Vol.1 §7.3.13.2 p.7-22. The physical comparison remains deferred to common Mac mini
acceptance.

```sh
python3 tests/cpu/generate_stage2_popfw.py --check
cargo test -p od-isa -p od-testkit --offline popfw
cargo test -p od-x86-runner --offline popfw_fixtures_admit
```

## PUSHFW fixture S2-17q

One literal fixture covers the long-mode `66 9C` word-width form. It writes
the low 16 bits of RFLAGS at `RSP - 2`, leaves RFLAGS unchanged, and commits
the two-byte stack-pointer decrement only after the bounded write succeeds.
The full local answer checks every register and byte; the physical memory mask
checks the integer-profile flags and fixed bit 1. A misplaced REX before 66 is
ignored by the shared decoder, while an effective REX and other prefixes are
outside this slice. Unit tests cover a bounded write failure without CPU
commit, and native admission requires a writable two-byte stack slot.

The handwritten fixture and literal answer are materialized by
[`generate_stage2_pushfw.py`](../../../cpu/generate_stage2_pushfw.py).
Normative source: Intel SDM revision 093 (253667-093US), Vol.2B
PUSHF/PUSHFD/PUSHFQ pp.4-528–4-529.

```sh
python3 tests/cpu/generate_stage2_pushfw.py --check
cargo test -p od-isa -p od-testkit --offline pushfw
cargo test -p od-runner --offline pushfw_cli_fixture
cargo test -p od-x86-runner --offline pushf_fixtures_admit_only_a_bounded_writable_stack_slot
```


## PUSHFQ/POPFQ REX.W-over-66 fixture S2-17s

One literal fixture executes `66 48 9C` followed by `66 48 9D`. Effective REX.W selects qword PUSHFQ/POPFQ semantics despite 66H; with effective REX.W clear, the existing word interpretation remains. PUSHFQ writes `RFLAGS AND 00000000_00FCFFFFH`; POPFQ restores the modeled flags. Input TF, NT and IOPL are zero to avoid combining this decode case with separate POPFQ restrictions. The fixture checks full RSP/RIP/flags state and the entire 8-byte stack slot with a canary mask.

Generated by [`generate_stage2_stack_flags_rexw.py`](../../../cpu/generate_stage2_stack_flags_rexw.py). Normative source: Intel SDM revision 093, Vol.2A §2.2.1.2 pp.2-8–2-9 and Vol.2B PUSHF/PUSHFD/PUSHFQ, POPF/POPFD/POPFQ.

```sh
python3 tests/cpu/generate_stage2_stack_flags_rexw.py --check
cargo test -p od-isa --offline pushf
cargo test -p od-testkit --offline pushf_rexw
```

## Near RET with ignored operand-size prefix S2-17t

The two literal fixtures execute `66 C3` and `48 66 C3` in long mode. Intel
SDM revision 093 Vol.1 §6.4.7 pp.6-12–6-13 forces near-branch operation
size to 64 bits and silently ignores operand-size prefixes. Vol.2A §2.2.1
p.2-8 says REX is ignored unless it immediately precedes the opcode or 0F,
so the REX before 66 is ineffective and is removed by prefix normalization.
Vol.2B RET p.4-566 defines `C3` as near RET. Both fixtures check the qword
return target, eight-byte RSP increment, full GPR/RIP/RFLAGS state, and
unchanged code and stack memory. Effective REX, duplicate 66, and other
effective prefix states remain outside this slice. Physical acceptance remains
deferred.

Generated by [`generate_stage2_ret66.py`](../../../cpu/generate_stage2_ret66.py)
from handwritten opcode bytes and a literal expected result, without an
assembler or emulator.

```sh
python3 tests/cpu/generate_stage2_ret66.py --check
python3 -m unittest tests.cpu.test_stage2_ret66_generator
cargo test -p od-isa --offline direct_call_and_ret_reject_prefixes_and_truncated_immediates
cargo test -p od-runner --offline operand_size_prefixed_near_ret_keeps_qword_stack_semantics
```

## Single-iteration MOVSB fixtures S2-18a

Three literal positive fixtures execute only bare `A4` MOVSB in long mode.
They check RSI/RDI movement for DF=0 and DF=1, exact RSI=RDI source/destination
aliasing for a one-byte self-copy, full GPR/RFLAGS/RIP, and all bytes in the data region.
The interpreter reads `[RSI]`, writes `[RDI]`, and commits both indices only
after the write succeeds. Flat DS/ES bases are zero in the modeled long-mode
profile. REP, all prefixes, address-size changes, widths other than byte, and
other string instructions remain unsupported.

Two fault-only fixtures place the selected source or destination just beyond
the bounded data mapping. The local test injects architectural #PF at each
site and checks access order and the executor's current no-commit policy.
Intel SDM 093 does not establish the CPU's index/RIP snapshot after a
destination #PF; see [ISSUE-016](../../../../docs/KNOWN_ISSUES.md#issue-016--не-установлен-restart-state-movs-после-destination-fault).
Native admission accepts the three fully mapped positive cases and rejects
both fault-only cases. The deferred physical acceptance pool S2-15 onward
contains 563 cases at the S2-18a boundary: 558 prior fixtures, these 3
positive cases, and 2 local-only injected-fault cases. Other files in the
fixture directory were not reclassified here; native admission determines
the executable subset. No Mac mini execution has been performed for this
slice.

The handwritten bytes and expected state/memory constants are serialized by
[`generate_stage2_movsb.py`](../../../cpu/generate_stage2_movsb.py). The script
does not assemble instructions or calculate x86 results. Its SHA-256 is
`2adc5c9a3507d2ca1f2c88e75bc688c992884c69bd69e01597aadb7c76d2337a`.
Normative source: Intel SDM revision 093
(253667-093US), Vol.2B MOVS/MOVSB/MOVSW/MOVSD/MOVSQ pp.4-107–4-109.

```sh
python3 tests/cpu/generate_stage2_movsb.py --check
cargo test -p od-isa -p od-testkit --offline movsb
cargo test -p od-x86-runner --offline movsb_native_admission
```

## Single-iteration MOVSW fixtures S2-18b

Three literal positive fixtures execute `66 A5` MOVSW in long mode. They check
RSI/RDI movement by two for DF=0 and DF=1, exact RSI=RDI aliasing for a
two-byte self-copy, full GPR/RFLAGS/RIP, and the complete data region. The
interpreter reads the word at `[RSI]`, writes it to `[RDI]`, and commits both
indices only after the write succeeds. Three mapped raw fixtures pass native
admission; the prior fixture pool is not reclassified in this slice.

Two fault-only fixtures place the word source or destination outside the
bounded mapping. Local injected-#PF checks verify Fetch→Read(2 bytes), or
Fetch→Read→Write(2 bytes), and the executor's current no-commit policy. SDM
093 does not establish the destination-fault restart snapshot; see
[ISSUE-016](../../../../docs/KNOWN_ISSUES.md#issue-016--не-установлен-restart-state-movs-после-destination-fault).
Native admission rejects both fault-only inputs. S2-18a's deferred pool was
563 cases; this slice adds 5 cases (3 mapped positive, 2 local-only fault),
bringing that pool to 568. No Mac mini execution has been performed for this
slice.

The handwritten bytes and expected answers are serialized by
[`generate_stage2_movsw.py`](../../../cpu/generate_stage2_movsw.py). The
script does not assemble instructions or calculate x86 results. Its SHA-256 is
`b5f2702bbf3206870474e26f9814d42e7cf58069038035a1b6433c67d9eda4bc`.
Normative source: Intel SDM revision 093 (253667-093US), Vol.2B
MOVS/MOVSB/MOVSW/MOVSD/MOVSQ pp.4-107–4-109. The decoder normalizes away a
misplaced REX before `66`; a local regression verifies `48 66 A5` has the same
accepted word semantics as `66 A5`.

```sh
python3 tests/cpu/generate_stage2_movsw.py --check
cargo test -p od-isa -p od-testkit --offline movsw
cargo test -p od-x86-runner --offline movsw_native_admission
```

## Single-iteration MOVSD fixtures S2-18c

Three literal positive fixtures execute bare `A5` MOVSD in long mode. They
check four-byte RSI/RDI movement for DF=0 and DF=1, a four-byte overlapping
copy, full GPR/RFLAGS/RIP, and every byte in the data region. This slice admits
only the unprefixed dword form. Effective REX, REP, segment prefixes, and 67
address-size override remain unsupported. `66 A5` selects the separate MOVSW
slice. The decoder normalizes away a
REX byte followed by a legacy prefix; the surviving legacy prefix still keeps
such a sequence outside bare-A5 admission.

Two fault-only fixtures place the dword source or destination outside the
bounded mapping. Local injected-#PF checks verify Fetch→Read(4 bytes), or
Fetch→Read→Write(4 bytes), and the executor's current no-commit policy. SDM
093 does not establish the destination-fault restart snapshot; see
[ISSUE-016](../../../../docs/KNOWN_ISSUES.md#issue-016--не-установлен-restart-state-movs-после-destination-fault).
Native admission accepts the three mapped positive fixtures and rejects both
fault-only cases. S2-18b's deferred pool was 568 cases; these five bring it to
573. No Mac mini execution has been performed for this slice.

Handwritten bytes and literal expected answers are serialized by
[`generate_stage2_movsd.py`](../../../cpu/generate_stage2_movsd.py), which does
not assemble instructions or calculate x86 results. Normative source: Intel
SDM revision 093, Vol.2B MOVS/MOVSB/MOVSW/MOVSD/MOVSQ; bare `A5` is MOVSD in
long mode, default address size uses RSI/RDI, and `67` selects ESI/EDI.

```sh
python3 tests/cpu/generate_stage2_movsd.py --check
cargo test -p od-isa -p od-testkit --offline movsd
cargo test -p od-x86-runner --offline movsd_native_admission
```

## Single-iteration STOSB fixtures S2-18e

Four handwritten schema v2 cases use only bare `AA` STOSB in long mode. Three
mapped cases cover DF=0, DF=1, and RSI=RDI aliasing with canary bytes around the
single destination byte. The alias case also proves that STOSB performs no
source memory read and leaves RSI unchanged. A fourth case points RDI exactly
past the mapped data region; its injected write `#PF` is local-only and checks
the executor's current no-commit policy. No hardware restart snapshot is
asserted; see [ISSUE-016](../../../../docs/KNOWN_ISSUES.md#issue-016--не-установлен-restart-state-movs-после-destination-fault).

Native admission accepts the three mapped cases and rejects the fault-only
fixture because the implicit destination is not mapped. Prefixes, REP, and
other STOS widths remain outside this slice. Literal bytes, full state, flags,
and memory images are materialized by
[`generate_stage2_stosb.py`](../../../cpu/generate_stage2_stosb.py),
which contains no encoder or semantic expected-value calculation.

The deferred S2-15-through-S2-18 physical pool was 580 cases after MOVSQ.
These four cases bring it to 584: three native-admissible positive fixtures
and one local-only fault fixture. Physical execution remains deferred to the
single final Mac mini acceptance substage.

```sh
python3 tests/cpu/generate_stage2_stosb.py --check
cargo test -p od-isa -p od-testkit --offline stosb
cargo test -p od-x86-runner --offline stosb_native_admission
```

## Bounded REP STOSB fixtures S2-19a

Five handwritten schema v2 fixtures cover exact `F3 AA`: zero count without a
destination access, one forward store, three forward/backward stores, and an
injected middle write fault after one committed iteration. The fault fixture is
local-only and checks that the successful store remains visible while the
faulting iteration leaves RCX/RDI at the next byte and RIP at `F3 AA`; this is
the chosen executor contract and does not assert exception-priority behavior.
Focused interpreter tests also stop after two stores, resume from the saved
RCX/RDI state, and exercise the separate 1024-iteration work limit. That limit
does not consume the one-instruction retirement budget.

Native admission accepts the four complete mapped small-count fixtures,
including zero count, and rejects the local-only fault, counts above eight,
and extra prefixes. These are fixture-profile bounds, not interpreter
semantics. Literal answers are serialized by
[`generate_stage2_rep_stosb.py`](../../../cpu/generate_stage2_rep_stosb.py);
the script does not assemble instructions or calculate expected results.

The deferred S2-15-onward physical pool increases from 584 to 589 fixtures.
Across string-operation slices S2-18a–S2-19a it now contains 21 mapped cases
admitted by native profile and 10 local-only fault cases rejected by admission.
Physical execution stays deferred to the single final Mac mini acceptance
substage.

```sh
python3 tests/cpu/generate_stage2_rep_stosb.py --check
cargo test -p od-testkit --offline rep_stosb
cargo test -p od-x86-runner --offline rep_stosb_native_admission
```

## Divide-error exit fixtures S2-20a

Two local-only schema v2 fixtures use literal `F6 F3` (`DIV BL`) bytes and
exercise its zero-divisor and byte-quotient-overflow `#DE` cases. The runner
CLI checks vector 0, unavailable error code and fault address, faulting RIP at
the instruction, zero retirements, and unchanged full GPR/RFLAGS/RIP and code
memory. They do not establish conflicting-fault priority or native exception
delivery. At S2-20a completion these two cases increased the deferred
S2-15-onward pool from 589 to 591; the native acceptance-selected subset was
unchanged. Across string-operation slices, native admission remains 21 mapped
fixtures and rejects 10 local-only fault fixtures. Physical runs stay in the
shared final Mac mini acceptance stage.

Literal values are serialized by
[`generate_stage2_divide_errors.py`](../../../cpu/generate_stage2_divide_errors.py).

```sh
python3 tests/cpu/generate_stage2_divide_errors.py --check
cargo test -p od-runner --test fixture_v2 divide_error_fixtures_preserve_state_and_report_exact_de_metadata --offline
```

## Complete UD2 exit fixture S2-20b

One local-only schema v2 fixture contains the complete `0F 0B` UD2 opcode. Its
CLI test checks vector 6, unavailable error code and fault address, faulting
RIP at the first byte, zero retired instructions, and unchanged full GPR,
RFLAGS, RIP and code memory. This does not assert priority against an
instruction-fetch fault, invalid-prefix behavior, native exception delivery,
or a physical hardware result. The case was not included in native acceptance
selection or execution. It brings the deferred S2-15-onward pool to 592 while
the native acceptance-selected subset remains unchanged.

The handwritten bytes and expected snapshot are serialized by
[`generate_stage2_ud2.py`](../../../cpu/generate_stage2_ud2.py), using Intel SDM
revision 093 Vol.2A UD2.

```sh
python3 tests/cpu/generate_stage2_ud2.py --check
python3 tests/cpu/test_stage2_ud2_generator.py
cargo test -p od-runner --test fixture_v2 complete_ud2_cli_fixture_reports_ud_without_state_commit --offline
```

## Qword IDIV divide-error exit fixture S2-20c

One local-only schema v2 fixture contains literal `48 F7 F9` (`IDIV RCX`)
with `RDX:RAX` equal to signed 128-bit minimum and `RCX=-1`, producing a
signed quotient overflow. The CLI test checks vector 0, unavailable error code
and fault address, faulting RIP at the instruction, zero retirements, and
unchanged full GPR/RFLAGS/RIP and code memory. This is an isolated divide-error
case; it does not claim fault-priority or native exception semantics. It is
excluded from native acceptance selection and execution. The deferred
S2-15-onward pool increases from 592 to 593 while the native-selected subset
remains unchanged. Physical checks remain in the shared final acceptance stage.

[`generate_stage2_idiv_qword_error.py`](../../../cpu/generate_stage2_idiv_qword_error.py)
serializes the literal bytes and expected state from Intel SDM revision 093
Vol.2A IDIV.

```sh
python3 tests/cpu/generate_stage2_idiv_qword_error.py --check
python3 tests/cpu/test_stage2_idiv_qword_error_generator.py
cargo test -p od-runner --test fixture_v2 qword_idiv_overflow_cli_fixture_reports_de_without_state_commit --offline
```

## Bare qword memory XCHG fixture S2-21a

One local-only schema v2 case runs handwritten `48 87 00` (`XCHG [RAX], RAX`).
RAX is both the memory base and source value; its unaligned qword crosses two
adjacent data regions, whose exact post-state and canary bytes are checked by
the public CLI test. This checks executor behavior through one
`GuestMemory::atomic_exchange_integer` call and verifies that the old value is
committed to RAX only on success. The GuestMemory contract requires the call to
leave memory unchanged on error; this fixture does not claim SMP ordering,
production-backend guarantees, native selection, or physical atomicity.

The single case increases the deferred S2-15-onward pool from 593 to 594; the
native-selected subset is unchanged. Physical checks remain in the common final
Mac mini acceptance stage.

[`generate_stage2_xchg_qword_memory.py`](../../../cpu/generate_stage2_xchg_qword_memory.py)
serializes the handwritten bytes and literal initial memory with Intel SDM
revision 093 Vol.2D XCHG provenance.

```sh
python3 tests/cpu/generate_stage2_xchg_qword_memory.py --check
python3 tests/cpu/test_stage2_xchg_qword_memory_generator.py
cargo test -p od-runner --test fixture_v2 qword_memory_xchg_fixture_matches_literal_alias_and_split_memory_result --offline
```

## Qword register-register XADD fixture S2-21b

One local-only schema v2 fixture runs literal `48 0F C1 C0` (`XADD RAX, RAX`).
With initial RAX `0x7fff_ffff_ffff_ffff`, the result is
`0xffff_ffff_ffff_fffe`; the CLI test checks all GPRs, RIP, full arithmetic
flags and one instruction retirement. The same register in both ModR/M fields
checks alias behavior. The case does not cover memory XADD, LOCK, other widths,
CMPXCHG, native admission or physical execution.

The case increases the deferred S2-15-onward pool from 594 to 595; native
selection is unchanged. Physical checks remain in the common final Mac mini
acceptance stage.

[`generate_stage2_xadd_qword_register.py`](../../../cpu/generate_stage2_xadd_qword_register.py)
serializes handwritten bytes and literal state with provenance to Intel SDM
revision 093 Vol.2D XADD p.6-23.

```sh
python3 tests/cpu/generate_stage2_xadd_qword_register.py --check
python3 tests/cpu/test_stage2_xadd_qword_register_generator.py
cargo test -p od-runner --test fixture_v2 qword_register_xadd_fixture_matches_alias_sum_and_arithmetic_flags --offline
```

## Qword register-register CMPXCHG fixtures S2-21c

Two local-only schema-v2 fixtures cover bare `48 0F B1 /r` qword register
CMPXCHG: the equal case compares RAX with RCX and copies RBX to RCX; the
unequal case compares RAX with RCX while source and destination both name RCX,
then copies the old RCX to RAX. Both include complete expected register and
flag snapshots in the CLI oracle. Memory destinations, LOCK, other widths,
native admission and physical execution are outside this slice.

These cases increase the deferred S2-15-onward pool from 595 to 597. The native
selection remains unchanged; physical checks remain in the common final Mac
mini acceptance stage.

[`generate_stage2_cmpxchg_qword_register.py`](../../../cpu/generate_stage2_cmpxchg_qword_register.py)
serializes handwritten bytes and initial state with provenance to Intel SDM
revision 093 Vol.2A CMPXCHG.

```sh
python3 tests/cpu/generate_stage2_cmpxchg_qword_register.py --check
python3 tests/cpu/test_stage2_cmpxchg_qword_register_generator.py
cargo test -p od-runner --test fixture_v2 qword_register_cmpxchg_fixtures_match_both_compare_branches --offline
```

## Qword memory XADD fixture S2-21d

One local-only schema-v2 fixture executes literal `48 0F C1 00` (`XADD
qword [RAX], RAX`). RAX is both the address base and source value. The unaligned
qword spans two adjacent regions; adding it to `0xffffffffffffffff` wraps to
`0x00000000003000fa`. The expected state checks the old memory value in RAX,
arithmetic flags, RIP/retirement and every byte of both regions, including
canaries. The generator provenance identifies Intel SDM revision 093 Vol.2D
XADD p.6-23.

The interpreter makes one `GuestMemory::atomic_fetch_add_integer` call. Local
spy and fault tests prove one backend operation and no CPU/RAM commit on an
error. This fixture is excluded from native admission; no production backend,
LOCK/SMP, or hardware-atomicity claim is made. The deferred S2-15-onward pool
increases from 597 to 598; the native-selected subset is unchanged. Physical
validation remains in the shared final Mac mini acceptance stage.

[`generate_stage2_xadd_qword_memory.py`](../../../cpu/generate_stage2_xadd_qword_memory.py)
serializes literal bytes and expected state with Intel SDM provenance.

```sh
python3 tests/cpu/generate_stage2_xadd_qword_memory.py --check
python3 tests/cpu/test_stage2_xadd_qword_memory_generator.py
cargo test -p od-runner --test fixture_v2 qword_memory_xadd_fixture_matches_alias_sum_flags_and_split_canaries --offline
```

## Qword memory CMPXCHG fixtures S2-21e

Two local-only schema-v2 fixtures execute bare effective REX.W `0F B1 /r`
CMPXCHG with qword memory destinations. The equal and unequal cases both use an
unaligned qword split across adjacent regions and check the complete register,
flag, RIP, retirement, and region-byte snapshots. The unequal case uses RAX as
the source and effective-address base; it verifies that the observed memory
value replaces RAX while memory remains unchanged. The generator records Intel
SDM revision 093 Vol.2A CMPXCHG provenance.

The `GuestMemory::atomic_compare_exchange_integer` boundary is one operation,
validates write access even on mismatch, and does not mutate memory on error.
Spy/fault tests cover backend call count and commit behavior. Fixtures remain
excluded from native admission; LOCK/SMP/production backend/physical claims
are outside scope. Deferred S2-15-onward pool increases from 598 to 600; the
native-selected subset is unchanged. Physical validation remains in the shared
final Mac mini acceptance stage.

[`generate_stage2_cmpxchg_qword_memory.py`](../../../cpu/generate_stage2_cmpxchg_qword_memory.py)
serializes literal bytes and state with Intel SDM provenance.

```sh
python3 tests/cpu/generate_stage2_cmpxchg_qword_memory.py --check
python3 tests/cpu/test_stage2_cmpxchg_qword_memory_generator.py
cargo test -p od-runner --test fixture_v2 qword_memory_cmpxchg_fixtures_match_both_compare_branches_and_split_canaries --offline
```

## LOCK qword memory XADD fixture S2-21f

One local-only schema-v2 fixture executes literal `F0 48 0F C1 00` (`LOCK XADD
qword [RAX], RAX`). It checks the old destination in the aliased RAX source,
wrapping sum, arithmetic flags, RIP/retirement, and all bytes/canaries across an
unaligned qword spanning adjacent regions. Generator provenance references
Intel SDM revision 093 Vol.2D XADD and Vol.2A LOCK prefix rules.

This operation uses the existing `GuestMemory::atomic_fetch_add_integer` call.
Its use is restricted to serial execution over private synchronous ordinary
RAM with no concurrent readers/writers, DMA, shared aliases, delayed writes,
or MMIO. No full x86 LOCK ordering, SMP, native admission, or physical claim is
made. Other LOCK forms remain unsupported. Deferred S2-15-onward pool grows
from 600 to 601; the native-selected subset is unchanged. Physical validation
remains in the shared final Mac mini acceptance stage. The unaligned fixture
checks backend bytes only and does not establish hardware #AC or split-lock
behavior.

[`generate_stage2_lock_xadd_qword_memory.py`](../../../cpu/generate_stage2_lock_xadd_qword_memory.py)
serializes the literal code and state with Intel SDM provenance.

```sh
python3 tests/cpu/generate_stage2_lock_xadd_qword_memory.py --check
python3 tests/cpu/test_stage2_lock_xadd_qword_memory_generator.py
cargo test -p od-runner --test fixture_v2 locked_qword_memory_xadd_fixture_matches_alias_sum_flags_and_split_canaries --offline
```

## Bare MFENCE fixture S2-21g

The local-only schema-v2 fixture executes `48 89 18 0F AE F0 48 8B 08`
(`MOV [RAX], RBX; MFENCE; MOV RCX, [RAX]`). It verifies the store is committed
before the later load, with GPRs, flags, RIP, retirement and the data bytes
observed. `GuestMemory::mfence` is required and has no default no-op; only
private synchronous `FlatMemory` opts in. The contract does not cover shared
memory, asynchronous writes, MMIO, DMA, SMP or physical ordering. Alternate
`0F AE` encodings and prefixes remain unsupported by this profile, without an
architectural invalid-encoding claim. The native-selected subset is unchanged;
the deferred S2-15-onward pool grows from 601 to 602. Physical validation
remains in the shared final Mac mini acceptance stage.

[`generate_stage2_mfence.py`](../../../cpu/generate_stage2_mfence.py) serializes
the literal fixture with Intel SDM provenance.

```sh
python3 tests/cpu/generate_stage2_mfence.py --check
python3 tests/cpu/test_stage2_mfence_generator.py
cargo test -p od-runner --test fixture_v2 mfence_store_fence_load --offline
```

## FS selector MOV fixture S2-22c

The local-only `s2-long-fs-selector-v1` fixture runs exactly `48 8C E0`
(`MOV RAX, FS`). It checks zero-extension of `FS.selector=0xF123` into RAX,
three-byte RIP advance, one retirement, and preservation of FS.base, selector,
flags and all other GPRs. The selector profile includes both FS.base and
FS.selector in initial/result state so independence and preservation are
observable. This reads the selector only; it performs no descriptor lookup or
validation and makes no claim about privilege or segment-load behavior.
Native execution rejects the profile before mapping or tracee creation, and
legacy native admission rejects this decoded form. The shared deferred pool
increases by one; native-selected membership is unchanged, and physical
validation remains in the shared final acceptance stage.

[`generate_stage2_fs_selector.py`](../../../cpu/generate_stage2_fs_selector.py)
serializes literal bytes and expected state from Intel SDM revision 093 Vol.1
§3.4.2 and Vol.2B MOV pp.4-28–4-31 provenance.

```sh
python3 tests/cpu/generate_stage2_fs_selector.py --check
python3 tests/cpu/test_stage2_fs_selector_generator.py
cargo test -p od-runner --test fixture_v2 fs_selector_fixture_zero_extends_and_preserves_segment_state --offline
```

## GS selector MOV fixture S2-22d

The local-only `s2-long-gs-selector-v1` fixture runs exactly `48 8C E8`
(`MOV RAX, GS`). It checks zero-extension of `GS.selector=0xF123` into RAX,
three-byte RIP advance, one retirement, and preservation of GS.base, selector,
flags and all other GPRs. The profile exposes only GS.base and GS.selector;
descriptor lookup/load and privilege checks are outside the slice. Native
execution fails closed before mapping or tracee creation, and legacy native
admission rejects the decoded form. The shared deferred pool grows by one;
native-selected membership is unchanged, and physical validation remains in
the shared final acceptance stage.

[`generate_stage2_gs_selector.py`](../../../cpu/generate_stage2_gs_selector.py)
serializes literal bytes and expected state with Intel SDM revision 093 Vol.1
§3.4.2 and Vol.2B MOV pp.4-28–4-31 provenance.

```sh
python3 tests/cpu/generate_stage2_gs_selector.py --check
python3 tests/cpu/test_stage2_gs_selector_generator.py
cargo test -p od-runner --test fixture_v2 gs_selector_fixture_zero_extends_and_preserves_segment_state --offline
```

## CPL3 GDT-only VERW fixture S2-22e

The local-only `s2-long-verw-gdt-v1` profile runs exactly bare `0F 00 E8`
(`VERW AX`) at CPL3. Its GDTR base and 16-bit limit are explicit state. Literal
cases cover writable data, read-only data, code, system, DPL/RPL mismatch,
exact/short table bounds, null selector and RPL0 success. Each case checks the
complete GPR state, flags, RIP/retirement, GDTR and unchanged descriptor bytes.
The interpreter performs one read-only 8-byte GDT read for an in-bounds
non-null selector; null and out-of-bounds selectors do not read. Descriptor
table reads use supervisor page privilege while architectural CPL remains 3.
Backend faults propagate without retirement or architectural commit. LDT/TI,
other privilege levels, descriptor loading, hidden caches, native execution
and physical validation remain outside this slice; physical validation is
deferred to the shared final acceptance stage.

Sources: Intel SDM revision 093, Vol.2C (326018-093US), VERR/VERW
pp.5-162–5-163; Vol.3A (325384-093US), §§3.5.1, 6.10.2 and 6.11.5. The
generator records these official sources alongside each literal case.

```sh
python3 tests/cpu/generate_stage2_verw_gdt.py --check
python3 tests/cpu/test_stage2_verw_gdt_generator.py
cargo test -p od-runner --test fixture_v2 verw_gdt_fixtures_check_literal_rights_flags_state_and_descriptor_bytes --offline
```

## CPL3 GDT-only `MOV FS, AX` fixture S2-22f/g

The partial local-only `s2-long-fs-load-v1` profile runs only bare `8E E0`
(`MOV FS, AX`) at Long64/CPL3. Literal inputs cover accessed writable/read-only
data, readable conforming/nonconforming code, invalid system/execute-only and
privilege cases, P=0, a short GDTR limit, and writable/read-only data with A=0.
The runner checks the complete GPR array, RIP, FS base/selector/cache,
unchanged GDTR/RFLAGS, the access-byte-only A update and exact #GP/#NP
vectors/error codes. Unit coverage separately checks nonzero base32, G=0/G=1
limit expansion, A=1 with no write permission, exact read8/write1 trace and
descriptor context, write-fault no-commit behavior, validation priority,
repeated-selector write suppression, and a split adjacent GDT where only the
access byte is writable. These writes require private synchronous RAM and
serial interpreter execution with no external observer/writer, DMA, shared
alias, delayed write or MMIO. The byte-sized backend call specifies semantic
access width only, not physical bus transaction width. This is a local
interpreter profile; native and physical execution are not claimed.

Sources: Intel SDM revision 093, Vol.2B (253667-093US), MOV pp.4-28–4-31;
Vol.3A (325384-093US), §3.4.5.1 pp.3-12–3-13, §5.6.1 pp.5-33–5-35,
§5.7 pp.5-37–5-38, §11.1.2.1 p.11-3 Automatic Locking. The generator records
official PDF links and literal provenance with each case.

```sh
python3 tests/cpu/generate_stage2_fs_load.py --check
python3 tests/cpu/test_stage2_fs_load_generator.py
cargo test -p od-runner --test fixture_v2 fs_load_fixtures_check_literal_descriptor_results_fault_codes_and_full_state --offline
```

## CPL3 GDT-only `MOV GS, AX` fixture S2-22h

The partial local-only `s2-long-gs-load-v1` profile runs only bare `8E E8`
(`MOV GS, AX`) at Long64/CPL3. The 11 bounded literal fixtures cover accessed
writable/read-only data, readable nonconforming code and a DPL0 conforming
code descriptor loadable from CPL3, invalid
system/execute-only and privilege cases, P=0, a short GDTR limit, and writable
and read-only data with A=0. They compare full GPR/RFLAGS/RIP/GDTR and FS
canaries, GS selector/base/cache, descriptor bytes and exact #GP/#NP
error codes. Unit tests additionally cover upper base32 bits, limit expansion,
A=1 without write permission, descriptor access ordering and context, read and
write faults without CPU commit, repeated selectors, and split adjacent GDT
regions. A=0 writes are one semantic byte write at descriptor+5 after the
8-byte read; the backend width does not assert physical bus width. Like the FS
slice, writes require private synchronous RAM and serial execution without
external observers/writers, DMA, shared aliases, delayed writes or MMIO. This
is a local interpreter profile; native and physical execution are not claimed.

The generator takes the opcode bytes and selector/access/flags/base fields as
handwritten literals, then serializes descriptor fields into the architectural
eight-byte layout. It does not derive expected CPU results. Sources: Intel
SDM revision 093, Vol.2B (253667-093US), MOV pp.4-28–4-31;
Vol.3A (325384-093US), §3.4.3–§3.4.5.1, §5.6.1, §5.7, and §11.1.2.1
Automatic Locking, as referenced by the checked-in CPU contract. The generator
records the same official PDF links and literal field provenance.

```sh
python3 tests/cpu/generate_stage2_gs_load.py --check
python3 tests/cpu/test_stage2_gs_load_generator.py
cargo test -p od-runner --test fixture_v2 gs_load_fixtures_check_literal_descriptor_results_fault_codes_and_full_state --offline
```

## Loaded GS.base composition fixture S2-22i

The one local-only composition case runs exactly `8E E8 65 48 8B 00` under
`s2-long-gs-load-v1`: CPL3 loads GS from a literal GDT descriptor, then the
existing `MOV RAX, qword ptr GS:[RAX]` consumes that cached base. The starting
GS.base points at a contrasting canary; the loaded base `0x80000000` selects
the qword at `0x8000000b`. Its offset `0x0b` exceeds the cached limit 0 by
design, consistent with the existing Long64 GS memory contract. The fixture
checks the one A-bit byte update, full state, both memory regions and final
retirement count. Unit trace assertions prove descriptor-origin read8 and
optional write1 at CPL3/pageRing0, followed by the explicit CPL3 data read8
without a GDT reread. Separate unit cases verify later data-read fault
persistence and noncanonical-result #GP(0) with no data callback.

The profile admits the existing exact `8E E8` case and this exact
two-instruction byte sequence with fixed budget/stop boundary; other sequences
are rejected. Instruction bytes, initial state, descriptor/data/canary bytes
and harness fields are handwritten literals. The generator serializes those
fixture inputs; the expected post-state and memory answers are hard-coded in
Rust unit and runner tests, not generated into the fixture. Canonicality and
long-mode no-limit behavior reuse the checks already documented in
CPU_CONTRACT, with no independent manual verification claimed. S2-22 and
ISSUE-017/018 remain open.

```sh
python3 tests/cpu/generate_stage2_gs_load.py --check
python3 tests/cpu/test_stage2_gs_load_generator.py
cargo test -p od-testkit --offline s2_22i
cargo test -p od-runner --test fixture_v2 --offline s2_22i_gs_base_composition_runs_the_literal_two_instruction_profile
```

## Loaded FS.base composition fixture S2-22j

The one local-only composition case runs exactly `8E E0 64 48 8B 00` under
`s2-long-fs-load-v1`: CPL3 loads FS from a literal GDT descriptor, then the
existing `MOV RAX, qword ptr FS:[RAX]` consumes that cached base. The original
FS.base points to a contrasting canary; the descriptor-loaded base
`0x80000000` selects the qword at `0x8000000b`. The fixture checks the one
A-bit byte update, full state, both data regions and retirement count. Unit
trace assertions prove descriptor read8 and optional write1 at CPL3/pageRing0,
then an explicit CPL3 data read8 without a GDT reread. Separate cases verify
that a later data-read fault retains the first instruction and that a
noncanonical result raises #GP(0) before any data callback.

The profile admits the existing exact `8E E0` case and this exact two-
instruction program with fixed budget and stop boundary; other sequences are
rejected. Fixture bytes, initial state, descriptor/data/canary bytes and
harness fields are handwritten literals. The generator serializes inputs;
expected CPU state and memory answers are literals in Rust tests. The existing
CPU_CONTRACT entries support descriptor and canonicality semantics; no
independent manual verification is claimed. S2-22 and ISSUE-017/018 remain
open. This fixture is local-only and is excluded from native selection and
execution.

```sh
python3 tests/cpu/generate_stage2_gs_load.py --check
python3 tests/cpu/test_stage2_gs_load_generator.py
cargo test -p od-testkit --offline s2_22j
cargo test -p od-runner --test fixture_v2 --offline s2_22j_fs_base_composition_runs_the_literal_two_instruction_profile
```

### S2-30k x87 schema coverage

The schema test admits all eight bare `D8 E8+i` FSUBR encodings. Alias
`D8 E8` is represented with expected `Unsupported`, `stop_rip: null`, and an
instruction budget of one. Local execution tests supply literal success
vectors and compare the unchanged complete CPU/x87 snapshot on that alias.
These cases remain local-only; native x87 admission stays closed.

### S2-30l x87 schema coverage

The schema validator admits all eight bare `DC E0+i` FSUBR encodings.
`DC E0` is represented as expected `Unsupported` with `stop_rip: null` and
instruction budget one. Local literal execution checks ST(0)−ST(i) output,
physical destination placement and preservation of TOP/source state. These
cases are local-only; native x87 admission stays closed.

### S2-30m x87 schema coverage

The schema validator admits all eight bare `DE E0+i` FSUBRP encodings.
`DE E0` is expected to remain Unsupported with `stop_rip: null` and instruction
budget one. Local execution and fixture-runner tests check old ST(i)
destination placement, retention of the popped payload under its Empty tag,
TOP increment and alias atomicity. Native x87 admission stays closed.

### S2-30n x87 schema coverage

The schema validator admits all eight bare `D8 C8+i` FMUL encodings. Literal
runner cases cover every TOP×i pair, including the valid i=0 square, differing
exponents, physical payload sentinels, C1 clearing and preservation of tags and
other state. Exact-only domain failures remain Unsupported atomically; native
x87 admission stays closed.

### S2-30o x87 schema coverage

The schema validator admits all eight bare `DC C8+i` FMUL encodings. Local
runner checks cover every TOP×i physical destination, the valid i=0 square,
different exponents, payload sentinels and atomic rejection of inexact/range
inputs. DC C0..C7 remain FADD, E0..E7 remain FSUBR, and E8..EF remain FSUB.
The forms are local-only; native x87 admission stays closed.

### S2-30p x87 schema coverage

The schema validator admits all eight bare `DE C8+i` FMULP encodings. Runner
cases check every TOP×i pair, including the valid i=0 square written into old
physical TOP before that tag is emptied, TOP wrap, C1 clearing and preservation
of other raw state. An inexact `DE C8` fixture is Unsupported with
`stop_rip: null` and budget one. `DE C9` is the supported no-operand
`FMULP ST(1), ST(0)` form. FADDP DE C0..C7, FSUBRP E0..E7 and FSUBP E8..EF
remain distinct; native x87 admission stays closed.

### S2-30q x87 schema coverage

The schema validator admits all eight bare `D8 F0+i` FDIV encodings. The
runner checks each TOP×i placement, exact `+1` alias behavior, C1 clearing,
tag/TOP preservation and atomic rejection of an inexact quotient. `D8 F1`
with no stop RIP represents the inexact Unsupported case. Other D8 division
directions, DC/DE forms and memory/integer forms remain unsupported; native
x87 admission stays closed.

### S2-30r x87 schema coverage

The schema validator admits all eight bare `DE F8+i` FDIVP encodings, including
the no-operand `DE F9` alias. Runner cases check every TOP×i destination,
ordered `15/3 = 5`, exact `+1` alias payload before the old TOP tag is emptied,
TOP wrap, C1 clearing and full x87 snapshot preservation. An inexact `DE F9`
fixture uses `stop_rip: null` and budget one, and verifies that the full state
is unchanged. `DE F0..F7` FDIVRP remains unsupported; native x87 admission
stays closed.

### S2-30s x87 schema coverage

The schema validator admits all eight bare `DE F0+i` FDIVRP encodings,
including the no-operand `DE F1` alias. Runner cases check every TOP×i
destination, ordered `15/3 = 5`, exact `+1` alias payload before the old TOP
tag is emptied, TOP wrap, C1 clearing and unchanged unrelated state. An
inexact `DE F1` case uses `stop_rip: null` and budget one and verifies that
the full x87 state is unchanged. `DE F8..FF` FDIVP and adjacent DE groups keep
their existing meanings; native x87 admission stays closed.


### S2-30t x87 schema coverage

The schema validator admits all eight bare `D8 F8+i` FDIVR encodings;
`D8 F9` is `FDIVR ST(0), ST(1)`. Runner cases check every TOP×i
destination, ordered `15/3 = 5`, exact `+1` alias behavior, C1 clearing and
preservation of tags/TOP and unrelated raw state. An inexact `D8 F9` case uses
`stop_rip: null` and budget one and verifies the full state is unchanged.
`D8 F0..F7` FDIV remains distinct; native x87 admission stays closed.

### S2-30u x87 schema coverage

The instruction schema accepts the eight bare `DC F0+i` forms in the local
interpreter profile. The runner tests every TOP×i destination, `ST(0)=6` over
`ST(i)=3`, the exact `i=0` +1 alias, sign combinations, normal-range edges and
`15/3=5` to verify operand order. An inexact `DC F1` case is Unsupported with
no stop RIP and budget one, and compares the complete state. Exhaustive DC
ModR/M and prefix checks preserve existing neighboring groups. Native x87
admission explicitly rejects all eight encodings.

### S2-30v x87 schema coverage

The instruction schema accepts the eight bare `DC F8+i` FDIV forms in the
local interpreter profile. The runner tests every TOP×i destination, ordered
`15/3=5`, the exact `i=0` +1 alias, and payload/TOP/tag preservation. Its
inexact `DC F9` case is Unsupported with no stop RIP and budget one, and
compares the complete state. `od-isa` literal vectors additionally cover sign
combinations and Normal-range edges. Exhaustive DC ModR/M and prefix checks
preserve `DC F0..F7` FDIVR and adjacent groups. Native x87 admission explicitly
rejects all eight new encodings.


### S2-30w x87 schema and fixture coverage

The schema admits all eight bare `DD D8+i` FSTP ST(i) encodings in the local
interpreter profile. `s2_30w_fstp_invalid_source.case.json` is a literal
`DD D9` rejection case with canonical-Normal ST(0) tagged Empty; it uses
`stop_rip: null` and budget one and checks zero retirement plus
full-state preservation through the CLI runner. Interpreter unit vectors cover
all TOP×i pairs, each old destination tag state for `i!=0`, exact ten-byte
source copies including signed and Normal-edge payloads, `i=0` and `i=1`, and
atomic environment/source rejection. All DD ModR/M bytes and prefix/truncation
cases are checked. Native x87 admission explicitly denies all eight forms.

Normative source: Intel SDM revision 093, Vol.2A, “FST/FSTP—Store Floating-Point
Value,” pp.3-386–3-387 ([official PDF, Vol.2A p.3-386](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=504)).

### S2-30aa x87 schema and fixture coverage

The schema admits exactly the D9 E0 FCHS, D9 E1 FABS and bare D9 E4 FTST
forms. Two literal FTST success fixtures cover positive and negative
canonical-Normal ST(0), including a condition-code field initially set to all
ones. The Empty-tag rejection fixture has `stop_rip: null` and instruction
budget one; the CLI verifies full state preservation and zero retirement.
Local interpreter vectors additionally cover every TOP, source sign and
Normal endpoints, all initial condition-code combinations, unchanged raw
payload/tag sentinels, FTST followed by FNSTSW AX, and fetch/truncation/STOP/
budget atomicity. Native x87 admission explicitly denies D9 E4.

Normative source: Intel SDM revision 093, Vol.2A, “FTST—Test,” pp.3-400–3-401
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

### S2-30ab x87 schema and fixture coverage

The schema admits bare `D8 D1` FCOM ST(1) and the exact four-byte
`D8 D1 DF E0` sequence used to observe its status through FNSTSW AX. Three
literal success fixtures cover ST(0)>ST(1), equality, and ST(0)<ST(1), with
physical register/tag sentinels. `s2_30ab_fcom_st1_invalid_rhs_tag.case.json`
has canonical-Normal ST(1) bytes but an Empty tag, `stop_rip: null`, and budget
one; the CLI confirms unchanged full state and zero retirement. Interpreter
vectors cover all TOP values and condition-code inputs, sign/exponent/all-bit
significand ordering, both operands' independent invalid tags/classes,
environment rejection, and fetch/truncation/STOP/budget atomicity. Exhaustive
D8 ModR/M checks keep arithmetic neighbors intact; native admission explicitly
denies `D8 D1`.

Normative source: Intel SDM revision 093, Vol.2A,
“FCOM/FCOMP/FCOMPP—Compare Floating-Point Values,” pp.3-324–3-326
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

### S2-30ac x87 schema and fixture coverage

The schema admits exactly the eight bare `D8 D0+i` FCOM register forms and
`D8 D0+i DF E0` for status observation. The S2-30ab three-relation fixtures
remain unchanged. `s2_30ac_fcom_st7_equal.case.json` adds a literal index-7
success case; `s2_30ac_fcom_st7_invalid_rhs_tag.case.json` has canonical-Normal
ST(7) bytes with an Empty tag, `stop_rip: null`, and budget one, and the CLI
checks full-state atomic rejection. Additional runner vectors execute every
source index. Interpreter vectors cover every TOP×i pair, including i=0 alias
and TOP wrap. The schema rejects D8 D8..DF and every prefixed or extended form.
Native admission explicitly denies all eight D8 D0..D7 encodings.

Normative source: Intel SDM revision 093, Vol.2A,
“FCOM/FCOMP/FCOMPP—Compare Floating-Point Values,” pp.3-324–3-326
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

### S2-30ad x87 schema and fixture coverage

The schema admits bare `D8 D9` FCOMP ST(1) and the exact four-byte
`D8 D9 DF E0` sequence for status observation. The literal success fixture
compares equal canonical-Normal ST(0)/ST(1), empties the old physical TOP tag,
and advances TOP without changing register payload bytes. The literal reject
fixture has an Empty ST(1) tag, `stop_rip: null`, and budget one; the CLI checks
full-state atomic rejection. Interpreter vectors cover every TOP including
TOP=7 wrap, all initial condition-code combinations, relation/sign/exponent/
low-significand ordering, independent lhs/rhs tag and class rejection, and
fetch/truncation/STOP/budget atomicity. The schema rejects D8 D8 and D8 DA..DF
along with prefixes, REX and WAIT. Native admission explicitly denies D8 D9.

Normative source: Intel SDM revision 093, Vol.2A,
“FCOM/FCOMP/FCOMPP—Compare Floating-Point Values,” pp.3-324–3-326
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

### S2-30x x87 schema and fixture coverage

The schema admits all eight bare `DD D0+i` FST ST(i) encodings. The literal
`s2_30x_fst_invalid_source.case.json` uses `DD D1` with a canonical-Normal
ST(0) tagged Empty, `stop_rip: null`, and budget one to verify atomic source-tag
rejection through the CLI runner. Runner checks cover each destination index
and verify raw payload placement, Valid destination tagging, unchanged TOP and
FSW, and full x87 state. Local vectors additionally exercise every TOP×i pair
and all old destination tags for i!=0. All DD ModR/M values and prefix,
REX, WAIT and truncation behavior are checked; `DD D8..DF` remains FSTP.
Native x87 admission explicitly denies all eight FST encodings.

### S2-30y x87 schema and fixture coverage

The schema admits the bare `D9 E0` FCHS success fixture with canonical-Normal
ST(0), C1 set, and a stop address after one instruction. The CLI verifies that
only the raw ST(0) sign bit changes, C1 clears, FIP/RIP update, and the rest of
the snapshot stays intact. `s2_30y_fchs_invalid_source.case.json` contains a
canonical-Normal payload with an Empty source tag; it has `stop_rip: null` and
budget one, so the CLI confirms a real zero-retirement atomic rejection.
Interpreter vectors separately cover all TOPs, both signs, Normal range edges,
double FCHS, all D9 second bytes, prefixes and execution boundaries. Native x87
admission explicitly denies `D9 E0`.

Normative source: Intel SDM revision 093, Vol.2A, “FCHS—Change Sign,” p.3-318 ([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=436)).

### S2-30z x87 schema and fixture coverage

The schema admits only bare `D9 E1` for the FABS slice. The positive literal
fixture starts with a negative canonical-Normal ST(0), C1 set and C0/C2/C3 set;
the CLI checks that only the raw sign bit clears, C1 clears, FIP/RIP update,
and all other snapshot fields remain unchanged. The invalid-source fixture has
the same Normal bytes with the physical TOP tag Empty, `stop_rip: null`, and
budget one, so the CLI checks a genuine zero-retirement atomic rejection.
Interpreter vectors cover all TOP values, both source signs, Normal boundaries,
idempotence, undefined-condition-bit retention, exhaustive D9 second-byte
decoding, prefixes and execution boundaries. Native x87 admission explicitly
denies `D9 E1`.

Normative source: Intel SDM revision 093, Vol.2A, “FABS—Absolute Value,” p.3-309 ([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=427)).

### S2-31a FRNDINT schema and fixture coverage

The schema admits bare `D9 FC` and the exact four-byte composition
`D9 FC DF E0` (FNSTSW AX). The success fixture rounds a positive 1.5 tie to
2.0 under nearest-even, setting sticky PE and C1 while preserving
C0/C2/C3; the composition fixture makes the resulting FSW observable in AX.
The no-stop, budget-one fixture has an Empty old physical TOP tag and confirms
atomic Unsupported. Interpreter vectors cover all TOP×RC×sign×exact/tie/
below/above combinations, prior sticky PE, condition-bit retention, both signed
2^63 results, rejected exponent endpoints and raw classes, invalid environment,
and fetch/truncation/STOP/budget boundaries. RC values and pre-existing PE are
accepted only by the FRNDINT interpreter gate; native admission explicitly
denies D9 FC.

Normative source: Intel SDM revision 093, Vol.2A, “FRNDINT—Round to Integer,”
p.3-372 ([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=490)).

### S2-31b FISTP m32int schema and literal coverage

The schema admits only bare `DB 18` and exact `DB 18 DF E0` (FNSTSW AX).
Literal cases cover a successful unaligned Dword write split across adjacent
fixture regions, the exact FNSTSW composition, an Empty old physical TOP tag,
and a signed-i32 overflow. The two success cases preserve all raw x87 payloads,
record the destination linear address in FDP, pop exactly one physical slot,
and make the stored bytes observable. No-stop/budget-one unsupported cases
preserve their full CPU and memory snapshots. Injected store permission and
architectural faults are exercised by interpreter tests. Native admission
explicitly rejects FISTP; flat fixture RAM does not establish MMU, page-walk,
MMIO, SMP, or physical x86 behavior.

Normative source: Intel SDM revision 093, Vol.2A, “FIST/FISTP—Store Integer”
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

### S2-31d FISTP m16int schema and literal coverage

The schema admits only bare `DF 18` and exact `DF 18 DF E0` (FNSTSW AX).
Handwritten fixtures cover positive and negative half-way inputs under all four
RC modes, exact signed-word limits, rounded values at both limits, and the
FNSTSW composition. Rejection fixtures cover signed-i16 overflow, invalid old
physical TOP tag, invalid FSW, unsupported FCW and an out-of-domain exponent.
The CLI verifies one Word mutation across the split fixture regions, unchanged
raw x87 payloads, pop/status/FIP/FDP state, and zero-retirement failures with
unchanged state and memory. Interpreter tests additionally exercise the exact
one-write/no-read contract, permission and injected #PF atomicity, and full
canonical48-range rejection. Native x87 admission explicitly denies FISTP.
Flat fixture RAM does not establish MMU translation, page walks, MMIO, SMP, or
physical x86 behavior.

Normative source: Intel SDM revision 093, Vol.2A, “FIST/FISTP—Store Integer,”
pp.3-347–3-349 ([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

### S2-31g FNSTCW m2byte schema and literal coverage

The schema admits exact bare `D9 38` and exact `D9 38 DF E0` (FNSTSW AX);
prefixed and neighboring D9 forms remain unsupported. Literal fixtures cover
the four standard FCWs, arbitrary schema-valid FCW `0xa55a` with `FSW=0xffff`,
Empty tags, payload and FPU pointer sentinels, an unaligned split-region Word
store with canaries, and canonical48 crossing rejection. The composition case
observes unchanged FSW through AX. Interpreter tests check that the store makes
one Word write and no data read, preserves FCW/FSW/tags/TOP/payloads/FIP/FDP/FOP,
GPRs and RFLAGS, and does not commit on second-byte write failure, fetch
truncation, STOP or budget exhaustion. Native admission explicitly denies
FNSTCW. The local profile preserves undefined C0-C3 deterministically but does
not claim native equality; it does not model general x87 exception delivery,
MMU translation, MMIO, SMP or physical x86 behavior.

Normative source: Intel SDM revision 093, Vol.2A, “FSTCW/FNSTCW—Store x87 FPU
Control Word,” pp.3-388–3-389 (`D9 /7`); Vol.1 §§8.1.8, 8.1.9 and §8.6 for
x87 state and FIP/FDP context ([official Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=506),
[official Vol.1 PDF](https://cdrdv2-public.intel.com/929351/253665-093-sdm-vol-1.pdf)).

### S2-31h FNSTSW m2byte schema and literal coverage

The schema admits exact bare `DD 38` and `DD 38 DF E0` (FNSTSW AX), while
prefixes—including waiting `9B DD 38`—and neighboring DD forms remain
unsupported. Literal fixtures cover FSW `0000`, mixed `45a5`, `ffff`, arbitrary
FCW/status with Empty tags and payload/pointer sentinels, an unaligned
split-region Word store with canaries, and canonical48 crossing rejection.
The composition case observes the same FSW through AX after storing it. Local
interpreter checks cover exactly one Word write with no data read, full x87/GPR/
RFLAGS preservation, write permission/backend failure and fetch/stop/budget
boundaries. Native x87 admission explicitly denies `DD 38`. C0-C3 preservation
is local deterministic policy without native-equivalence evidence; the slice
does not establish `#MF`, `#NM`/CR0, MMU, MMIO, SMP, or physical x86 behavior.

Normative source: Intel SDM revision 093, Vol.2A, “FSTSW/FNSTSW—Store x87 FPU
Status Word,” pp.3-392–3-393 (`DD /7`); Vol.1 §§8.1.8, 8.1.9 and 8.3.12
([official Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=510),
[official Vol.1 PDF](https://cdrdv2-public.intel.com/929351/253665-093-sdm-vol-1.pdf)).

### S2-31i FLDCW m2byte schema and literal coverage

The schema admits exact bare `D9 28` and the composition
`D9 28 D9 FC DF E0`; prefixes, including waiting `9B D9 28`, and other D9
forms remain unsupported. The literal composition fixture loads `0x0b7f`
across the existing stack region, rounds 1.5 upward through FRNDINT, and
observes resulting FSW `0x4720` through FNSTSW AX. Interpreter vectors cover
all 16 old/new standard FCW pairs and each loaded rounding-control setting.
Invalid old FCW/FSW rejects before data access; invalid loaded FCW performs
one Word read and rejects without committing. Missing x87 state, second-byte
permission failure, canonical48 crossing, fetch/truncation/STOP/budget and
exhaustive D9 ModR/M/prefix decoding are covered. Native x87 admission denies
FLDCW. The bounded implementation does not deliver pending/unmasked x87
exceptions, `#MF`, or `#NM`/CR0 and makes no physical execution claim.

Normative source: Intel SDM revision 093, Vol.2A, “FLDCW—Load x87 FPU Control
Word,” pp.3-356–3-357 (`D9 /5`); Vol.1 §§8.1.8, 8.1.9 and §8.6 ([official
Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=474),
[official Vol.1 PDF](https://cdrdv2-public.intel.com/929351/253665-093-sdm-vol-1.pdf)).

### S2-31j FSTP m32fp schema and literal CLI coverage

The schema admits exact bare `D9 18` and exact `D9 18 DF E0` (FNSTSW AX);
prefixes, WAIT and neighboring D9 bytes remain unsupported. The two handwritten
fixtures use an aligned, mapped Dword destination, a +1.5 binary80 source,
preserved payload sentinels and an observable post-pop status. The CLI test
checks the stored binary32 bits `0x3fc00000`, TOP/tag update, preserved payloads, FIP/FDP,
and FNSTSW AX composition. Interpreter vectors independently cover all TOPs,
even/odd ties, sign-sensitive directed rounding, minimum/maximum normal
exponents, carry from exponent 126, rejected exponents -127/127, C1/PE state,
canaries, one atomic write with no data read, canonical crossing, store fault,
and rejection before data access. Native x87 admission remains closed. These
fixtures and flat-memory checks do not establish hardware exception priority,
general x87 exception delivery, MMU, MMIO, SMP, or physical x86 behavior.

Normative source: Intel SDM revision 093, Vol.2A, “FST/FSTP—Store Floating
Point Value,” pp.3-386–3-387 (`D9 /2`/`D9 /3`); see also the local bounded
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31j-bounded-fstp-m32fp-rax-d9-18).

### S2-31k FST m32fp schema and literal CLI coverage

The schema admits exact bare `D9 10` and exact `D9 10 DF E0` (FNSTSW AX);
prefixes, WAIT and neighboring D9 bytes remain unsupported. These two
handwritten fixtures reuse the +1.5 binary80 source and mapped Dword destination
from S2-31j. The CLI verifies stored `0x3fc00000`, unchanged TOP/tag/payload
state, retained C0/C2/C3, exact-result C1 clearing, FIP/FDP and status-word
composition through AX. Independent interpreter tests cover all TOPs with tag
and payload sentinels, RC/tie/sign behavior, normal exponent endpoints and
carry rejection, canaries, one-write/no-read, canonical range, backend store
failure and architectural #PF atomicity. Native x87 admission stays closed.
No hardware exception-priority, MMU, MMIO, SMP or physical x86 behavior is
claimed.

Normative source: Intel SDM revision 093, Vol.2A, “FST/FSTP—Store Floating
Point Value,” pp.3-386–3-387 (`D9 /2`/`D9 /3`); see also the local bounded
[CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31k-bounded-fst-m32fp-rax-d9-10).

### S2-31l FSTP m64fp schema and literal CLI coverage

The schema admits exact bare `DD 18` and exact `DD 18 DF E0` (FNSTSW AX);
other FSTP m64fp ModR/M/addressing forms, prefixes, REX and WAIT remain
unsupported for this operation; existing DD register/status forms retain
their current behavior. Two
handwritten schema-v2 fixtures use a canonical-Normal 1.5 source and a Qword
destination. The CLI verifies stored binary64 bits `0x3ff8000000000000`, the
old TOP tag becoming Empty, TOP advance, preserved payloads, FIP/FDP and the
FNSTSW-observed status. Independent interpreter vectors cover both signs and
all RC modes, ties and discarded-bit boundaries, exponent endpoints and
carry rejection, all TOP values including wrap, PE/C1, canaries, one atomic
Qword write with no read, canonical range checks, backend errors and
architectural `#PF` atomicity. Native x87 admission remains closed. These
local checks do not establish hardware exception priority, MMU, MMIO, SMP or
physical x86 behavior.

Normative source: Intel SDM revision 093, Vol.2A, “FST/FSTP—Store Floating
Point Value,” pp.3-386–3-387 (`DD /2`/`DD /3`)
([official PDF, Vol.2A p.3-386](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=504));
see the local bounded [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31l-bounded-fstp-m64fp-rax-dd-18).

### S2-31m FST m64fp schema and literal CLI coverage

The schema admits exact bare `DD 10` and exact `DD 10 DF E0` (FNSTSW AX);
other DD ModR/M/addressing forms, prefixes, REX and WAIT remain unsupported
for this operation, while existing DD register/status forms retain their
behavior. Two handwritten schema-v2 fixtures use a canonical-Normal 1.5
source and Qword destination. CLI checks verify binary64 bytes
`0x3ff8000000000000`, unchanged TOP/tag word/raw registers and the
FNSTSW-observed status. Interpreter checks use independent rounding literals
for both signs and four RC modes, exact/tie-adjacent/discarded-bit values,
exponent endpoints and carry rejection. They also cover every TOP including
TOP 7, no-pop source preservation after inexact conversion, canaries, one
Qword write/no read, separate noncanonical-start and canonical-crossing
ranges, backend and architectural store failures, invalid source/environment,
and fetch/truncation/STOP/budget boundaries. Native x87 admission explicitly
denies this operation. These local tests do not establish hardware exception
priority, MMU, MMIO, SMP or physical x86 behavior.

Normative source: Intel SDM revision 093, Vol.2A, “FST/FSTP—Store Floating
Point Value,” pp.3-386–3-387 (`DD /2` FST, `DD /3` FSTP)
([official PDF, Vol.2A p.3-386](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=504),
[official PDF, Vol.2A p.3-387](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=505));
see the local bounded [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31m-bounded-fst-m64fp-rax-dd-10).

### S2-31n FILD m32int schema and literal CLI coverage

The schema admits exact bare `DB 00` and exact `DB 00 DF E0` (FNSTSW AX),
while exhaustive decoder checks admit only `DB 00` for FILD and reject its other
ModR/M values, prefixes, REX and WAIT. Existing DB instruction forms remain
supported. Two handwritten fixtures load signed integer 1 into physical
register 7 from an unaligned Dword crossing adjacent regions; the composition
fixture observes the pushed status through AX. CLI/interpreter checks compare
the literal binary80 payload/tag, TOP, FIP/FDP, status and preserved CPU/x87
state. Unit vectors independently cover zero, ±1/2/3, `2^24±1`, i32 maximum
and minimum, every TOP including wrap, and all four RC values. Failure checks
cover nonempty destination before memory, missing x87 state, FCW/FSW gates,
canonical start/crossing, architectural read fault, backend error, fetch and
truncation, STOP and zero budget. Native x87 admission remains closed.

Normative source: Intel SDM revision 093, Vol.2A, “FILD—Load Integer,”
pp.3-342–3-343 ([official PDF, Vol.2A p.3-342](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=460),
[official PDF, Vol.2A p.3-343](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=461));
see the local bounded [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31n-bounded-fild-m32int-rax-db-00).

### S2-31o FILD m16int schema and literal CLI coverage

The schema admits exact bare `DF 00` and exact `DF 00 DF E0` (FNSTSW AX).
Exhaustive decoder checks add only `DF 00`, while retaining `DF 10`, `DF 18`,
`DF 38`, `DF E0`, `DF E9`, `DF F1`, and the existing DB forms; all prefixes
remain rejected. Two handwritten fixtures read signed Word value 1 from an
unaligned address crossing adjacent regions and observe the pushed status in
the composition case. Runner checks compare binary80 payload/tag, TOP, FIP/FDP,
status, and preserved state. Interpreter vectors cover zero, ±1/2/3,
32767/-32768, all TOP×RC combinations, sign extension, stale Empty payload,
unrelated tags/payloads, C1/PE policy, exact two-byte reads with canaries,
canonical range/read faults, and fetch/truncation/STOP/budget boundaries.
Native x87 admission remains closed.

Normative source: Intel SDM revision 093, Vol.2A, “FILD—Load Integer,”
pp.3-342–3-343 ([official PDF, Vol.2A p.3-342](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=460),
[official PDF, Vol.2A p.3-343](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=461));
see the local bounded [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31o-bounded-fild-m16int-rax-df-00).

### S2-31f FISTP m64int schema and literal coverage

The schema admits exact `DF 38` and `DF 38 DF E0` (FNSTSW AX), with prefixes
and neighboring encodings rejected. Literal CLI fixtures cover an unaligned
Qword crossing adjacent regions, exact signed-i64 maximum and minimum, TOP
pop, the FNSTSW-observed status, and positive 2^63 rejection before any data
write. Interpreter vectors separately cover positive/negative ties under all
four RC modes, negative 2^63 under every RC, `2^63 - 0.5` under all four RC
modes, negative `2^63+1`, exponent 64, canaries, canonical48 rejection,
permission failure and injected #PF atomicity. Native x87 admission explicitly
denies `DF 38`. Overflow uses this bounded profile's Unsupported result rather
than architectural indefinite integer encoding; x87 exception delivery,
MMU/page walks, MMIO, SMP and physical execution remain unmodeled.

Normative source: Intel SDM revision 093, Vol.2A, “FIST/FISTP—Store Integer,”
pp.3-347–3-349 ([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

### S2-31e FIST m16int schema and literal coverage

The schema admits only bare `DF 10` and exact `DF 10 DF E0` (FNSTSW AX).
Handwritten cases cover a positive tie, FNSTSW composition, exact signed-word
limits, signed-i16 overflow, invalid old TOP tag, invalid FSW, unsupported FCW
and an out-of-domain exponent. The CLI verifies one Word mutation and complete
no-pop state preservation, including TOP, tags and raw payloads; rejection
cases preserve CPU and memory with zero retirement. Interpreter unit tests also
cover every rounding control and sign, rounding at the signed limits, complete
canonical48 range validation, the one-write/no-read contract, permission
failure and injected architectural-fault atomicity. Native x87 admission
explicitly denies FIST. Flat fixture RAM does not establish MMU translation,
page walks, MMIO, SMP or physical x86 behavior.

Normative source: Intel SDM revision 093, Vol.2A, “FIST/FISTP—Store Integer,”
pp.3-347–3-349 ([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

### S2-31c FIST m32int schema and literal coverage

The schema admits only bare `DB 10` and exact `DB 10 DF E0` (FNSTSW AX).
Literal cases execute the same unaligned split-region Dword write as S2-31b
and then optionally observe FSW through AX. They verify that FIST retains the
old TOP, tag word and raw payloads while recording FIP/FDP and setting sticky
PE/C1. Additional literal cases reject an Empty old physical TOP tag and
signed-i32 overflow without changing the snapshot or data memory. Interpreter
checks cover all four rounding controls, signed values, signed-i32 overflow
before access, the one-write/no-read contract, and store fault atomicity. Native
x87 admission explicitly denies FIST. Flat fixture RAM
does not establish MMU, page-walk, MMIO, SMP, or physical x86 behavior.

Normative source: Intel SDM revision 093, Vol.2A, “FIST/FISTP—Store Integer”
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

Normative source: Intel SDM revision 093, Vol.2A, “FST/FSTP—Store Floating-Point
Value,” pp.3-386–3-387 ([official PDF, Vol.2A p.3-386](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=504)).

### S2-30ae FCOMP ST(i) schema and literal CLI coverage

The schema admits every bare `D8 D8+i` form and each exact four-byte
`D8 D8+i DF E0` status-observation sequence with instruction budgets one and
two. Eight handwritten equality fixtures execute each FCOMP index through the
CLI and verify that the old TOP tag becomes Empty, TOP advances once, the
condition field reports equality, and all physical register bytes remain
unchanged. The existing invalid-ST(1)-tag case rejects atomically with
`stop_rip: null` and budget one. Interpreter vectors independently cover every
TOP×i pair and lhs/rhs tag/class failures. Native admission explicitly denies
all eight encodings.

Normative source: Intel SDM revision 093, Vol.2A,
“FCOM/FCOMP/FCOMPP—Compare Floating-Point Values,” pp.3-324–3-326.

### S2-30af FCOMPP schema and literal CLI coverage

The schema admits exactly bare `DE D9` and the exact `DE D9 DF E0` FNSTSW AX
observation sequence. The literal success fixture checks the compare status and
two-entry physical TOP pop through the CLI. A separate `DE D9` fixture has an
invalid actual old ST(1) tag, no stop RIP and budget one, proving atomic
rejection. Schema validation ties that rejection rule to the physical tag
selected by the input TOP. Interpreter vectors cover every TOP, relation and
initial condition field; native admission denies `DE D9`.

Normative source: Intel SDM revision 093, Vol.2A,
“FCOM/FCOMP/FCOMPP—Compare Floating-Point Values,” pp.3-324–3-326.

### S2-30ag FUCOM schema and literal CLI coverage

[s2_30ag_fucom_equal_fnstsw.case.json](s2_30ag_fucom_equal_fnstsw.case.json)
runs bare `DD E1` followed by `DF E0` with equal canonical-Normal operands;
FUCOM replaces the condition codes with equality and FNSTSW copies that value
to AX. TOP, tags and raw register payloads remain unchanged. The paired
[s2_30ag_fucom_invalid_rhs_tag.case.json](s2_30ag_fucom_invalid_rhs_tag.case.json)
has a Valid lhs and Empty old physical ST(1), no stop RIP, and budget one; it
must return Unsupported without retiring. Schema and interpreter-runner tests
load both literal files. The unique QNaN comparison behavior of FUCOM is
outside this Normal-only slice.

Normative source: Intel SDM revision 093, Vol.2A,
“FUCOM/FUCOMP/FUCOMPP—Unordered Compare Floating-Point Values,” pp.3-402–3-404
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

### S2-30aj FCOMI schema and literal CLI coverage

[`s2_30aj_fcomi_equal_fnstsw.case.json`](s2_30aj_fcomi_equal_fnstsw.case.json)
uses exact `dbf1dfe0`: equal canonical-Normal operands set ZF, clear the other
five affected RFLAGS bits, clear only FSW.C1, and let FNSTSW copy `0x4500` to
AX. The fixture compares the six defined FCOMI flag bits. The paired
[`s2_30aj_fcomi_invalid_rhs_tag.case.json`](s2_30aj_fcomi_invalid_rhs_tag.case.json)
uses exact `dbf1`, no stop RIP and budget one; the actual old physical ST(1)
tag is Empty, so the instruction must reject atomically without retirement.
Schema loading and interpreter CLI coverage assert both exact byte strings.
Native admission explicitly denies `Operation::Fcomi` and DB F1.

Normative source: Intel SDM revision 093, Vol.2A,
“FCOMI/FCOMIP/FUCOMI/FUCOMIP—Compare Floating-Point Values,” pp.3-327–3-329.

### S2-30ak FCOMIP schema and literal CLI coverage

[`s2_30ak_fcomip_equal_fnstsw.case.json`](s2_30ak_fcomip_equal_fnstsw.case.json)
uses exact `dff1dfe0`: equal canonical-Normal operands set ZF, clear the other
five affected RFLAGS bits, clear only FSW.C1, empty physical old TOP, advance
TOP once, and let FNSTSW copy `0x4d00` to AX. Physical old ST(1) remains Valid
and its ten-byte payload is preserved. The paired
[`s2_30ak_fcomip_invalid_rhs_tag.case.json`](s2_30ak_fcomip_invalid_rhs_tag.case.json)
uses exact `dff1`, no stop RIP and budget one; the actual old physical ST(1)
tag is Empty, so FCOMIP rejects atomically without retirement. Schema loading
and interpreter CLI coverage assert the exact byte strings and resulting
state. Native admission explicitly denies `Operation::Fcomip` and DF F1.

Normative source: Intel SDM revision 093, Vol.2A,
“FCOMI/FCOMIP/FUCOMI/FUCOMIP—Compare Floating-Point Values,” pp.3-327–3-329
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

### S2-30al FUCOMI schema and literal CLI coverage

[`s2_30al_fucomi_equal_fnstsw.case.json`](s2_30al_fucomi_equal_fnstsw.case.json)
uses exact `dbe9dfe0`: ordered equal canonical-Normal operands set ZF, clear
the other five affected RFLAGS bits and only FSW.C1, then FNSTSW copies
`0x4500` to AX. FUCOMI leaves TOP, tags and payloads unchanged. The paired
[`s2_30al_fucomi_invalid_rhs_tag.case.json`](s2_30al_fucomi_invalid_rhs_tag.case.json)
uses exact `dbe9`, no stop RIP and budget one; the actual old physical ST(1)
tag is Empty, so execution rejects without retirement or state mutation.
Schema/CLI coverage rejects `dbf8dfe0`. Native admission explicitly denies
`Operation::Fucomi` and DB E9.

Normative source: Intel SDM revision 093, Vol.2A,
“FCOMI/FCOMIP/FUCOMI/FUCOMIP—Compare Floating-Point Values,” pp.3-327–3-329.

### S2-30ah FUCOMP schema and literal CLI coverage

[s2_30ah_fucomp_equal_fnstsw.case.json](s2_30ah_fucomp_equal_fnstsw.case.json)
runs bare `DD E9` followed by `DF E0` with equal canonical-Normal operands.
FUCOMP reports equality, empties only old physical TOP, advances TOP once, and
FNSTSW copies the resulting status to AX. Raw physical register payloads and
the old ST(1) Valid tag remain unchanged.
[s2_30ah_fucomp_invalid_rhs_tag.case.json](s2_30ah_fucomp_invalid_rhs_tag.case.json)
has a Valid lhs and Empty old physical ST(1), no stop RIP, and budget one; the
instruction must reject without retiring or changing state. Schema and
interpreter CLI tests load both literal files. FUCOMP's QNaN behavior remains
outside the canonical-Normal-only slice.

Normative source: Intel SDM revision 093, Vol.2A,
“FUCOM/FUCOMP/FUCOMPP—Unordered Compare Floating-Point Values,” pp.3-402–3-404
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

### S2-30ai FUCOMPP schema and literal CLI coverage

[s2_30ai_fucompp_equal_fnstsw.case.json](s2_30ai_fucompp_equal_fnstsw.case.json)
runs bare `DA E9` then `DF E0` with equal canonical-Normal operands. It observes
FSW/AX `0x5000`, both old physical tags Empty, the TOP advance by two and two
retired instructions; physical payloads remain unchanged.
[s2_30ai_fucompp_invalid_rhs_tag.case.json](s2_30ai_fucompp_invalid_rhs_tag.case.json)
uses a Valid lhs and invalid actual old physical ST(1) tag with no stop RIP and
budget one, so execution must reject without retiring or changing state. Schema
tests admit only exact `dae9` and `dae9dfe0` for this FUCOMPP slice, and decoder
tests enumerate all DA ModR/M values and prefix forms. Native admission denies
DA E9. C1=0 is an explicit local policy. Intel SDM revision 093 Vol.2A
pp.3-402–3-404's opcode table/prose says two entries are popped while p.3-403
pseudocode depicts one; the local contract follows the explicit table/prose.

Normative source: Intel SDM revision 093, Vol.2A,
“FUCOM/FUCOMP/FUCOMPP—Unordered Compare Floating-Point Values,” pp.3-402–3-404
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

### S2-30am FUCOMIP schema and literal CLI coverage

[s2_30am_fucomip_equal_fnstsw.case.json](s2_30am_fucomip_equal_fnstsw.case.json)
uses exact `dfe9dfe0`: equal canonical-Normal operands set ZF, clear the other
five affected flags and C1, empty old physical TOP, advance TOP once, and let
FNSTSW copy `0x4d00` to AX. All raw payloads and the old physical ST(1) Valid
tag remain preserved. The paired
[s2_30am_fucomip_invalid_rhs_tag.case.json](s2_30am_fucomip_invalid_rhs_tag.case.json)
uses exact `dfe9`, TOP=5, and Empty at actual physical `(TOP+1)&7`; with no stop
RIP and budget one, it rejects without retirement or mutation. Schema/CLI
reject `dff8dfe0` and 66/48/9B-prefixed variants. Native admission explicitly
denies `Operation::Fucomip` and DF E9.

Normative source: Intel SDM revision 093, Vol.2A,
“FCOMI/FCOMIP/FUCOMI/FUCOMIP—Compare Floating-Point Values,” pp.3-327–3-329
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

### S2-30an indexed FCOMI-family schema and literal CLI coverage

The schema accepts the exact 32 bare forms `DB E8..EF` FUCOMI, `DB F0..F7`
FCOMI, `DF E8..EF` FUCOMIP and `DF F0..F7` FCOMIP, either alone or followed
by `DF E0` FNSTSW AX. Exhaustive schema tests cover all 64 strings and reject
neighboring bytes, prefixes, memory forms and extra trailing bytes.

Four literal success fixtures cover `DB F0` self-compare at TOP=3 with only
old TOP Valid; `DF E8` self-compare/pop at TOP=7 with wrap and only old TOP
Valid; `DB EF` source ST(7) at TOP=7 using physical slot 6 while old ST(1) is
Empty; and `DF F6` source ST(6) at TOP=6 using physical slot 4, then popping
only old TOP. Standalone budget-one rejects cover `i=0` with a Valid old TOP
but all-zero payload and `i=7` with Empty actual physical `(TOP+7)&7` while the
unrelated old ST(1) remains Valid. Every reject has no stop RIP and preserves
full state. The existing `i=1` fixtures remain unchanged. Native admission
explicitly denies all 32 indexed instruction encodings and all four operation
families.

Normative source: Intel SDM revision 093, Vol.2A,
“FCOMI/FCOMIP/FUCOMI/FUCOMIP—Compare Floating-Point Values,” pp.3-327–3-329
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

### S2-31p FILD m64int schema and literal CLI coverage

The schema and CLI execute only the exact bare bytes `DF 28` and exact
composition `DF 28 DF E0` (FNSTSW AX). Each handwritten fixture places signed
Qword value 1 at an unaligned address that crosses two adjacent RAM regions;
the composition case observes the pushed status through AX. The CLI checks
the binary80 payload/tag, TOP, FIP/FDP, status and preserved state. Decoder
coverage exhausts every DF ModR/M byte and all tested legacy and REX prefixes,
while retaining DF 00/10/18/38/E0/E9/F1 and all DB forms.

Interpreter literals cover zero, ±1/2/3, i64 extrema, values around `2^53`
and signed values around `2^62` across every TOP and RC. The shared i64 packer
is also exercised by the existing m16/m32 FILD suites. An unaligned Qword
read checks canaries and one exact eight-byte access. Invalid x87 gates,
noncanonical/crossing/overflow ranges, architectural read #PF, backend errors,
fetch/truncation/STOP/zero-budget boundaries and full state atomicity are
covered. Native x87 admission explicitly denies the instruction. This remains
a local partial slice; general exception delivery, MMU/MMIO/SMP and physical
x86 behavior are not established.

Normative source: Intel SDM revision 093, Vol.2A, “FILD—Load Integer,”
pp.3-342–3-343 ([official PDF, Vol.2A p.3-342](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=460),
[official PDF, Vol.2A p.3-343](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=461));
see the local bounded [CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31p-bounded-fild-m64int-rax-df-28).

### S2-31q FSTP m80fp schema and literal CLI coverage

The schema admits exact bare `DB 38` and exact `DB 38 DF E0` (FNSTSW AX).
Fixtures contain independent literal code bytes, raw binary80 ST(0) state and
the ten-byte destination; CLI checks verify all ten bytes, TOP/tag retirement,
preserved payloads and FNSTSW-visible C1 clearing. Interpreter tests add all TOP
values, minimum/maximum Normal raw exponents, accepted FCW variants, sticky PE,
condition retention, unaligned canaries and a nonzero byte 11, one write/no
reads, full canonical48 range/overflow, denied and injected backend/#PF stores,
and source/environment/fetch/execution-boundary atomicity. Native admission
explicitly denies the decoded operation. This local profile does not establish
physical x86 behavior or general x87 exception delivery.

Normative source: Intel SDM revision 093, Vol.2A, “FST/FSTP—Store Floating
Point Value,” pp.3-386–3-387 (`DB /7`), official PDF p.504
([Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=504));
Vol.1 §§8.1.8, 8.1.9 and §8.6 for x87 state and FIP/FDP.


### S2-31r FLD m80fp schema and literal CLI coverage

Fixtures contain exact literal `DB 28` and `DB 28 DF E0` bytes, a raw
canonical-Normal ten-byte read-only memory region, full x87 state, and a
non-readable adjacent byte-11 canary; independent code lengths/stop addresses
keep both boundaries explicit. The status-composition case observes the pushed TOP and
C1 policy through FNSTSW AX. A third fixture supplies binary80 zero, expects
Unsupported, and verifies the CPU snapshot remains uncommitted after the single
source read. CLI results check the expected exit and one/two retired
instructions. Interpreter tests cover the full TOP × FCW matrix, literal raw
sign/fraction/exponent vectors, unaligned exact-width access/no writes, invalid
classes, canonical ranges, #PF/backend errors, and execution boundaries.
DB ModR/M/prefix tests preserve all existing forms and reject DB 30. Native
x87 admission remains closed.

Normative source: Intel SDM revision 093, Vol.2A, “FLD—Load Floating-Point
Value,” pp.3-352–3-353, official PDF p.470
([Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=470)).

### S2-31t FLD m64fp schema and literal CLI coverage

The original S2-31t fixtures contained exact literal `DD 00` and
`DD 00 DF E0` instruction bytes, an unaligned eight-byte binary64 source,
complete x87 state and an adjacent read-only canary. The composition fixture
observes pushed TOP and C1 clearing through FNSTSW AX. At that stage the third
fixture used zero as an unsupported source. The historical S2-31t report
records that Normal-only behavior.

Normative source: Intel SDM revision 093, Vol.2A, “FLD—Load Floating-Point
Value,” pp.3-352–3-353, official PDF p.470
([Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=470)).

### S2-31u FLD m64fp signed-zero fixture coverage (historical)

The exact `DD 00` and `DD 00 DF E0` fixtures now use a literal positive-zero
binary64 source and expect a successful push with the Zero tag; the composition
observes the updated TOP and C1 through FNSTSW AX. The invalid-class fixture
supplied positive minimum subnormal in the S2-31u slice; S2-31z reclassifies it
to positive infinity while preserving the S2-31u report as historical
evidence. Interpreter literals additionally cover negative zero and,
currently, negative minimum subnormal. Native admission continues to deny all
three fixtures. See the [S2-31u CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31u-bounded-fld-m64fp-rax-signed-zero-extension), [plan](../../../../docs/plans/STAGE_2.md#s2-31u-extend-bounded-fld-m64fp-rax-dd-00-to-signed-zero), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31u--bounded-fld-m64fp-signed-zero-extension).

Normative source: Intel SDM revision 093, Vol.2A, “FLD—Load Floating-Point
Value,” pp.3-352–3-353, official PDF pp.470–471
([Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=470)).

### S2-31w FLD m80fp signed-zero fixture coverage

The current `DB 28` success fixture supplies literal binary80 +0 and asserts
the pushed Zero tag and ten zero bytes. The `DB 28 DF E0` composition fixture
supplies -0 and checks the preserved sign byte (`0x80`) and Zero tag while
FNSTSW observes TOP/C1. The invalid-class fixture now supplies positive
minimum nonzero binary80 subnormal; schema admission is limited to nonzero
binary80 subnormal values for exact DB 28 with budget one and no stop RIP.
Both signs of minimum subnormal remain Unsupported after the one ten-byte read
with no CPU commit. The S2-31r report and fixture history above document its
original Normal-only state.

Intel SDM revision 093, Vol.2A FLD m80fp (pp.3-352–3-353, official PDF p.470)
does not define #D for this source format. Binary32/binary64 denormal policy is
not carried over; broader denormal/unmasked handling requires a dedicated
semantics slice. See the [S2-31w CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31w-bounded-fld-m80fp-rax-db-28-signed-zero-extension),
[plan](../../../../docs/plans/STAGE_2.md#s2-31w-extend-bounded-fld-m80fp-rax-db-28-to-signed-zero),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31w--bounded-fld-m80fp-signed-zero-extension).

### S2-31x FLD m80fp canonical-subnormal coverage

The S2-31x exact `DB 28` fixture supplies positive minimum binary80 subnormal;
the `DB 28 DF E0` composition fixture supplies negative largest subnormal and
checks that FNSTSW observes the pushed TOP/C1 result. Both assert preserved
raw ten-byte payloads and the Special tag. Interpreter literals cover both
signs of minimum, largest and interior subnormals for every TOP and accepted
FCW, with one exact ten-byte read and preserved unrelated state. The invalid
class fixture used positive pseudo-denormal at the S2-31x stage; S2-31aa
replaces it with positive canonical QNaN for the narrow exact-DB28,
budget-one, no-stop schema exception. Negative pseudo-denormal and other
special encodings remain rejected. Intel SDM
revision 093, Vol.2A FLD m80fp (pp.3-352–3-353, official PDF p.470) defines no
`#D` for this source format. See the [S2-31x CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31x-bounded-fld-m80fp-rax-db-28-canonical-subnormal-extension),
[plan](../../../../docs/plans/STAGE_2.md#s2-31x-extend-bounded-fld-m80fp-rax-db-28-to-canonical-subnormals),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31x--bounded-fld-m80fp-canonical-subnormal-extension).

### S2-31aa FLD m80fp canonical-infinity coverage

The positive bare `DB 28` fixture supplies canonical +infinity and expects the
unchanged ten-byte payload with the Special tag. The negative infinity fixture
executes `DB 28 DF E0`, preserving the sign and observing TOP/C1 through
FNSTSW. Interpreter literals cover both signs across every TOP and supported
FCW. Canonical NaNs, pseudo-infinities of both signs, pseudo-denormals,
pseudo-NaNs and unnormals reject after one ten-byte read without CPU commit;
memory preflight, read faults and execution-boundary atomicity remain covered.
At the S2-31aa stage the narrow Unsupported fixture supplied positive
canonical QNaN; S2-31ab replaces it with positive canonical SNaN. JIT/native admission
continues to deny the operation. Intel SDM revision 093 Vol.1 Table 4-3 gives
the canonical extended infinity encoding, while Table 8-3 distinguishes
pseudo-infinity and unsupported encodings; Vol.2A FLD m80fp is the instruction
reference. See the [S2-31aa CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31aa-bounded-fld-m80fp-rax-db-28-canonical-infinities),
[plan](../../../../docs/plans/STAGE_2.md#s2-31aa-extend-bounded-fld-m80fp-rax-db-28-to-canonical-infinities),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31aa--bounded-fld-m80fp-canonical-infinity-extension).

### S2-31ab FLD m80fp canonical-QuietNaN coverage

The positive bare `DB 28` fixture supplies a canonical QNaN with an independent
payload; the negative-indefinite fixture executes `DB 28 DF E0`, preserves its
raw bytes and observes the resulting TOP through FNSTSW. Interpreter literals
cover both signs/payloads across every TOP and supported FCW, retaining sticky
PE without setting IE. The exact `DB 28`, budget-one, no-stop Unsupported
fixture supplied canonical positive SNaN at that stage; S2-31ac replaces it
with positive pseudo-infinity. Schema/CLI neighbors reject other classes from
that narrow exception. The JIT/native profile continues to deny `DB 28`. See
the [S2-31ab CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31ab-bounded-fld-m80fp-rax-db-28-canonical-quietnans),
[plan](../../../../docs/plans/STAGE_2.md#s2-31ab-extend-bounded-fld-m80fp-rax-db-28-to-canonical-qnans),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31ab--bounded-fld-m80fp-canonical-quietnan-extension).

### S2-31ac FLD m80fp canonical-SNaN coverage

The positive bare `DB 28` fixture supplies a minimum-payload canonical SNaN;
the negative maximum-payload fixture executes `DB 28 DF E0`, preserves all ten
raw bytes including the clear quiet bit and observes the pushed TOP through
FNSTSW. Interpreter literals cover minimum, middle and maximum payloads of
both signs across every TOP and supported FCW, preserving PE and C0/C2/C3 with
IE clear. The exact `DB 28`, budget-one, no-stop Unsupported fixture now uses
positive pseudo-infinity. Canonical SNaNs, pseudo-SNaNs and other neighboring
classes do not match that exception. Native and JIT profiles continue to deny
`DB 28`. See the [S2-31ac CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31ac-bounded-fld-m80fp-rax-db-28-canonical-signalingnans),
[plan](../../../../docs/plans/STAGE_2.md#s2-31ac-extend-bounded-fld-m80fp-rax-db-28-to-canonical-snans),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31ac--bounded-fld-m80fp-canonical-signalingnan-extension).

### S2-31s FLD m32fp schema and literal CLI coverage

Fixtures contain exact literal `D9 00` and `D9 00 DF E0` instruction bytes,
an unaligned four-byte binary32 source, full x87 state and an adjacent
read-only canary. The composition case observes the pushed TOP and C1 clearing
through FNSTSW AX. At the original S2-31s slice, the third fixture supplied
zero and expected Unsupported; S2-31v reclassifies it to positive minimum
subnormal. The native admission
tests explicitly deny all three fixtures. Interpreter checks cover the full
TOP × FCW matrix using independent binary80 literals for signed one, adjacent
fractions, minimum/maximum Normal values and exponent 127; they also cover
special-class rejection, access width, canonical range failures, read
#PF/backend errors, invalid push gates and execution boundaries. Exhaustive D9
decode checks retain the existing forms and reject all prefix variants.

Normative source: Intel SDM revision 093, Vol.2A, “FLD—Load Floating-Point
Value,” pp.3-352–3-353, official PDF p.470
([Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=470)).

### S2-31v FLD m32fp signed-zero fixture coverage

The exact `D9 00` and `D9 00 DF E0` fixtures load positive and negative
binary32 zero respectively. CLI assertions check the pushed Zero tag and raw
signed binary80 payload; the composition observes TOP and C1 through FNSTSW
AX. In the original S2-31v result, the invalid-class fixture supplied positive
minimum subnormal and expected Unsupported. S2-31y supersedes that current
fixture/profile while keeping the S2-31v report historical. Native admission
continues to deny all fixtures. See the [S2-31v CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31v-bounded-fld-m32fp-rax-d9-00-signed-zero-extension), [plan](../../../../docs/plans/STAGE_2.md#s2-31v-extend-bounded-fld-m32fp-rax-d9-00-to-signed-zero), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31v--bounded-fld-m32fp-signed-zero-extension).

### S2-31y FLD m32fp masked-subnormal fixture coverage

The new exact `D9 00` fixture supplies positive minimum binary32 subnormal;
the `D9 00 DF E0` composition fixture supplies negative minimum subnormal and
observes sticky DE through FNSTSW AX. CLI checks verify canonical Normal
binary80 payloads, Valid tags, and the sticky DE status bit. Interpreter
literal vectors cover positive/negative minimum, interior and largest
subnormals for every TOP and accepted FCW, plus sticky PE/C1 and repeated-load
behavior. At the S2-31y stage, the invalid fixture supplied positive infinity;
S2-31af now supplies canonical positive QNaN because signed infinities are
supported. The current schema exception admits only QNaN for exact `D9 00`,
budget one and no stop RIP. Native admission remains closed. See the [S2-31y CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31y-bounded-fld-m32fp-rax-d9-00-masked-subnormal-extension), [plan](../../../../docs/plans/STAGE_2.md#s2-31y-extend-bounded-fld-m32fp-rax-d9-00-to-masked-subnormals), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31y--bounded-fld-m32fp-masked-subnormal-extension).

### S2-31z FLD m64fp masked-subnormal fixture coverage

The positive exact `DD 00` fixture supplies the minimum binary64 subnormal;
the negative `DD 00 DF E0` fixture supplies its negative and observes sticky
DE through FNSTSW AX. CLI checks verify canonical Normal binary80 payloads,
Valid tags and DE. Interpreter literals cover signed minimum, interior and
largest subnormals for every TOP and accepted FCW, plus sticky PE/C1 and
repeated-load behavior. At the S2-31z stage, the invalid-class fixture supplied positive infinity;
S2-31ae changed it to canonical positive QNaN because infinities became
supported. S2-31ag supersedes that predicate by accepting QNaNs; the current
schema exception requires canonical binary64 SNaN with exact `DD 00`, budget
one and no stop RIP. Native admission remains closed. See the [S2-31z CPU
contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31z-bounded-fld-m64fp-rax-dd-00-masked-subnormal-extension), [plan](../../../../docs/plans/STAGE_2.md#s2-31z-extend-bounded-fld-m64fp-rax-dd-00-to-masked-subnormals), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31z--bounded-fld-m64fp-masked-subnormal-extension).

### S2-31ad FLD m80fp pseudo-denormal fixture coverage

The positive bare `DB 28` fixture supplies a pseudo-denormal with zero
fraction; the negative `DB 28 DF E0` fixture supplies maximum fraction and
observes TOP/status through FNSTSW. Both preserve all ten bytes, including the
zero exponent, and push Special without new status flags. Interpreter literal
coverage includes both signs, zero/minimum/maximum fraction, all TOP×FCW pairs,
and equivalent exponent-one Normal controls. The unsupported schema fixture
remains positive pseudo-infinity; pseudo-denormals do not match its exact
DB28/budget-one/no-stop predicate. Arithmetic consumption, unmasked delivery,
native/JIT admission and physical behavior remain outside the slice. See the
[S2-31ad CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31ad-bounded-fld-m80fp-rax-db-28-pseudo-denormals),
[plan](../../../../docs/plans/STAGE_2.md#s2-31ad-extend-bounded-fld-m80fp-rax-db-28-to-pseudo-denormals),
and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31ad--bounded-fld-m80fp-pseudo-denormal-extension).

### S2-31ae FLD m64fp signed-infinity fixture coverage

The positive bare `DD 00` fixture supplies +infinity; the negative
`DD 00 DF E0` fixture supplies -infinity and observes the pushed state through
FNSTSW. Both produce the exact canonical binary80 infinity and Special tag,
with no new flags. Interpreter literals cover both signs across every TOP and
accepted FCW; a minimum-subnormal load followed by infinity verifies sticky DE
and PE. Signed QNaN/SNaN neighbors remained Unsupported at this stage after
one Qword read. Its narrow schema exception was exact DD00, budget one, no
stop RIP and QNaN only; S2-31ag supersedes it. Native/JIT admission remains
closed. See the [S2-31ae CPU
contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31ae-bounded-fld-m64fp-rax-dd-00-signed-infinities), [plan](../../../../docs/plans/STAGE_2.md#s2-31ae-extend-bounded-fld-m64fp-rax-dd-00-to-infinities), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31ae--bounded-fld-m64fp-signed-infinity-extension).


### S2-31af FLD m32fp signed-infinity fixture coverage

The positive bare `D9 00` fixture supplies +infinity; the negative
`D9 00 DF E0` fixture supplies -infinity and observes the pushed state through
FNSTSW. Both produce canonical binary80 infinity and Special tag without new
flags. Interpreter literals cover both signs across all TOP×FCW pairs; a
minimum-subnormal load followed by infinity verifies sticky DE and PE. Signed
QNaN/SNaN neighbors remain Unsupported after one Dword read. The narrow
schema exception is exact D900, budget one, no stop RIP and canonical QNaN
only. Native/JIT admission remains closed. See the [S2-31af CPU
contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31af-bounded-fld-m32fp-rax-d9-00-signed-infinities), [plan](../../../../docs/plans/STAGE_2.md#s2-31af-extend-bounded-fld-m32fp-rax-d9-00-to-infinities), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31af--bounded-fld-m32fp-signed-infinity-extension).


### S2-31ag FLD m64fp canonical QNaN fixture coverage

The positive bare `DD 00` fixture supplies a quiet-only positive QNaN; the
negative `DD 00 DF E0` fixture supplies a payload-bearing QNaN and observes
the pushed state through FNSTSW. Both preserve sign and payload in canonical
binary80 with Special tag and add no status flags. Interpreter literals cover
quiet-only, low, middle and maximum payloads of both signs across TOP×FCW; a
minimum-subnormal load followed by QNaN verifies sticky DE/PE. The current
schema exception is exact DD00, budget one, no stop RIP and canonical SNaN
only. SNaNs remain post-read Unsupported after one Qword read. JIT/native
admission remains closed. See the [S2-31ag CPU
contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31ag-bounded-fld-m64fp-rax-dd-00-canonical-qnans), [plan](../../../../docs/plans/STAGE_2.md#s2-31ag-extend-bounded-fld-m64fp-rax-dd-00-to-canonical-qnans), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31ag--bounded-fld-m64fp-canonical-qnan-extension).

### S2-31ah FLD m32fp canonical QNaN fixture coverage

The positive bare `D9 00` fixture supplies a quiet-only QNaN; the negative
`D9 00 DF E0` fixture supplies a payload-bearing QNaN and observes the pushed
state through FNSTSW. Both preserve sign/payload in canonical binary80 with
Special tag and add no exception flags. Interpreter literals cover
quiet-only, low, middle and maximum payloads of both signs across TOP×FCW; a
minimum-subnormal load followed by QNaN verifies sticky DE/PE. The current
At S2-31ah, the schema exception was keyed to canonical SNaN and SNaNs
remained post-read Unsupported; S2-31aj accepts them and now uses an invalid-
environment fixture. JIT/native admission remains closed. See the [S2-31ah CPU
contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31ah-bounded-fld-m32fp-rax-d9-00-canonical-qnans), [plan](../../../../docs/plans/STAGE_2.md#s2-31ah-extend-bounded-fld-m32fp-rax-d9-00-to-canonical-qnans), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31ah--bounded-fld-m32fp-canonical-qnan-extension).

### S2-31ai FLD m64fp canonical SNaN fixture coverage

The positive bare `DD 00` fixture loads a low-payload SNaN; the negative
`DD 00 DF E0` fixture loads a maximum-payload negative SNaN and observes IE
through FNSTSW. Both quiet the source by integer bit mapping, preserve sign and
payload, produce binary80 Special, and set sticky IE only. Interpreter vectors
cover signed low/middle/maximum payloads across TOP×FCW, sticky DE/PE, exact
FSW, one-read fault atomicity, pre-read IM=0/pre-existing-IE rejection, and
FNCLEX recovery. Pre-existing IE blocks another DD00 until FNCLEX. The exact
schema exception is DD00, budget one, no stop RIP, and an invalid FCW or
`FSW & 0x80dd != 0`, independent of operand value. Native/JIT admission
remains closed. See the [S2-31ai CPU
contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31ai-bounded-fld-m64fp-rax-dd-00-canonical-snans), [plan](../../../../docs/plans/STAGE_2.md#s2-31ai-extend-bounded-fld-m64fp-rax-dd-00-to-canonical-snans), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31ai--bounded-fld-m64fp-canonical-snan-extension).

### S2-31aj FLD m32fp canonical SNaN fixture coverage

The positive bare `D9 00` fixture loads a minimum-payload canonical binary32
SNaN. The negative payload-bearing `D9 00 DF E0` fixture observes IE through
FNSTSW. Integer conversion sets binary80's quiet bit, preserves sign/payload,
and assigns Special. Interpreter literal vectors cover signed low, middle and
maximum payloads across TOP×FCW, sticky DE/PE, exact FSW, and FNCLEX recovery.
The latter clears exception flags while preserving TOP, tags and loaded bytes,
then permits another D900 load. Memory errors, unmasked/pre-existing-IE
pre-read gates and subsequent-load rejection are covered. The exact
D900/budget-one/no-stop Unsupported schema fixture is environment-driven
(invalid FCW or `FSW & 0x80dd != 0`) and uses an ordinary Normal source to show
operand class is irrelevant. Native/JIT admission remains closed. See the
[S2-31aj CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31aj-bounded-fld-m32fp-rax-d9-00-canonical-snans), [plan](../../../../docs/plans/STAGE_2.md#s2-31aj-extend-bounded-fld-m32fp-rax-d9-00-to-canonical-snans), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31aj--bounded-fld-m32fp-canonical-snan-extension).

### S2-31ak bounded m32/m64 FLD sticky-status fixtures

The prior D900 environment fixture and DD00 rejected-class fixture now carry
forbidden ZE status and use exact `D900`/`DD00`, budget-one, no-stop predicates.
The schema no longer treats IE, DE or PE as an automatic rejection for these
loads. The existing bare and DFE0/FNSTSW signaling-NaN fixtures verify the
sticky IE result; interpreter tests cover repeated and cross-width sequences,
all sticky IE/DE/PE combinations, and preserve the separate FILD/m80 gates.
Native/JIT admission remains denied. See [S2-31ak CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31ak-sticky-exception-flags-for-bounded-m32m64-fld), [plan](../../../../docs/plans/STAGE_2.md#s2-31ak-preserve-sticky-ie-de-pe-across-exact-m32m64-fld-loads), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31ak--sticky-ie-de-pe-across-bounded-m32m64-fld).

### S2-31al signed-zero m32 store fixtures

Four literal fixtures cover positive and negative binary80 Zero through exact
`D9 10` and `D9 18`; the negative cases compose with `DF E0` so FNSTSW exposes
the final status. The source physical TOP tag is Zero, the ten-byte payload is
canonical zero with the requested sign, and the destination bytes are literal
`00000000` or `00000080`. Runner tests verify FST stack preservation and FSTP
pop/tag updates after the write. Interpreter coverage spans all TOP×FCW values,
incoming PE/C1 combinations, FLD-zero compositions, bad tag/payload pairs,
store faults and canonical range rejection. JIT/native admission remains
closed. See [S2-31al CPU contract](../../../../docs/specs/CPU_CONTRACT.md#local-s2-31al-signed-zero-fst-m32fp-and-fstp-m32fp), [plan](../../../../docs/plans/STAGE_2.md#s2-31al-store-signed-binary80-zero-through-bounded-m32-fstfstp), and [report](../../../../docs/reports/STAGE_2_REPORT.md#s2-31al--signed-zero-stores-for-bounded-m32-fstfstp).
