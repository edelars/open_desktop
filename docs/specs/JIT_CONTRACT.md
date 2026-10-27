# Контракт первого JIT

**Статус:** начальный профиль этапа 3; зафиксирован в S3-00. Это контракт
первого проверяемого JIT-среза, а не описание полной x86-64 совместимости.

## 1. Источники и предел доказательств

Первый профиль берёт только S0-09 из [CPU contract](CPU_CONTRACT.md) и
принимает ровно формы, подтверждённые [отчётом этапа 1](../reports/STAGE_1_REPORT.md).
Проверка этапа 1 закрыта для своего старого register-only исполнителя; она не
подтверждает новый block pipeline этапа 3. Полный corpus этапа 1 подключается
к новому pipeline после dispatcher и переходов между блоками (S3-15).

Legacy `od-jit` fragment translator по-прежнему существует отдельно от нового
typed block path и сохраняет свой `FragmentState` interface. S3-13 добавляет
первый opt-in fixture-v1 путь через builder, typed IR, allocator, runtime
emitter и `PublishedCode`; это один block без dispatcher/cache. Schema v2 JIT
по-прежнему отклоняется. Для новых форм нельзя считать lowering завершённым
только потому, что декодер или интерпретатор этапа 2 уже принимает их.

Для кодирования A64 используется [Arm A-profile A64 Instruction Set
Architecture, DDI 0602, версия 2026-06](https://developer.arm.com/documentation/ddi0602/2026-06/Base-Instructions).
Для общих правил вызовов используется [AAPCS64, выпуск 2025Q4](https://github.com/ARM-software/abi-aa/blob/2025Q4/aapcs64/aapcs64.rst).
Платформенные отличия Apple задаёт документ [Writing ARM64 code for Apple
platforms](https://developer.apple.com/documentation/xcode/writing-arm64-code-for-apple-platforms)
(доступ 2026-10-26); в частности, x18 зарезервирован, а SP сохраняет
16-байтовое выравнивание. Для executable memory и синхронизации instruction
cache применяются указания Apple [Porting just-in-time compilers to Apple
silicon](https://developer.apple.com/documentation/apple-silicon/porting-just-in-time-compilers-to-apple-silicon)
(доступ 2026-10-26). До реализации host path зафиксировать реально
используемые macOS, Xcode/SDK и entitlements; документация не заменяет проверку
целевой конфигурации.

## 2. Матрица начального среза

Ограничение кодировок намеренно точное: REX byte `48`, только регистры
RAX/RCX/RDX/RBX, без high-byte registers и памяти. Для ModR/M `mod` обязан быть
`11b`, а оба индекса `reg` и `r/m` — от 0 до 3 включительно.

| Guest form | Начальный путь | Состояние и сравнение | Внешняя зависимость |
|---|---|---|---|
| `48 B8..BB imm64` — `MOV r64, imm64` | Native ARM64 lowering; flags не меняются | Четыре разрешённых GPR, RIP, сохранение определённых входных flags и exit reason | S0-09 CPU contract; точная форма уже принята decoder/interpreter этапа 2 |
| `48 03 /r`, `mod=11b` — `ADD r64, r/m64` | Native ARM64 arithmetic; собственный host helper материализует CF/PF/AF/ZF/SF/OF | Четыре разрешённых GPR и все определённые flags | Точные operand order/flags из CPU contract; A64 encoding из DDI 0602 |
| `48 2B /r`, `mod=11b` — `SUB r64, r/m64` | Native ARM64 arithmetic; helper материализует определённые x86 flags | Четыре разрешённых GPR и все определённые flags | То же; арифметическая семантика принадлежит `od-isa` |
| `48 3B /r`, `mod=11b` — `CMP r64, r/m64` | Native ARM64 subtraction без записи результата; helper материализует определённые x86 flags | GPR не меняются; сравниваются определённые flags и RIP | То же |
| `74 cb` — `JZ rel8` | Native ARM64 conditional control flow по материализованному ZF | RIP/exit reason и заданное состояние GPR/flags | Цель должна быть границей инструкции внутри входного буфера либо его концом |
| `75 cb` — `JNZ rel8` | Native ARM64 conditional control flow по материализованному ZF | То же | То же |

Формы таблицы образуют единственное начальное множество native JIT lowering.
Для `MOV` принимаются только четыре назначения из таблицы. Для арифметики оба
регистра operand должны иметь индекс 0–3. Другие REX bytes, ширины, opcode
варианты, addressing modes, префиксы и состояния в этот профиль не входят.
Интерпретатор может поддерживать больше форм; это не расширяет таблицу.

Для каждого исполнения сравнивать RAX/RCX/RDX/RBX, RIP, причину выхода и все
определённые на наблюдаемом exit flags из CF/PF/AF/ZF/SF/OF. Это включает
флаги, определённые предыдущей инструкцией и сохранённые последующими `MOV` или
ветвлением. Не сравнивать флаги, оставленные undefined последним flag-writing
оператором, и прочие неоговорённые биты RFLAGS. Память и архитектурные faults в
данном профиле отсутствуют.

## 3. Формы за пределами среза

| Класс | Путь на текущем шаге | Условие дальнейшего включения |
|---|---|---|
| Любые memory operands вне точно перечисленных V2 load/store профилей ниже | Runner и V1 block/cache path не выбирают memory code. Для перечисленных форм доступны отдельные compiler-issued V2 artifacts и host entries; прямой host pointer access запрещён | Каждая новая форма отдельно проходит S2 semantic confirmation, checked access/commit implementation и собственную проверку |
| Остальные формы, уже принятые interpreter этапа 2 | Runner не допускает их к текущему legacy JIT. Interpreter fallback запланирован в S3-16 и не реализован текущим contract | Завершённая interpreter semantics конкретной формы и fallback с точным progress/state |
| x87, SIMD, privilege/system, interrupts, unsupported/invalid decode | Runner не допускает эти формы к текущему legacy JIT; interpreter путь разрешён только если соответствующая форма уже реализована и запрошен явный fallback после S3-16 | Отдельные S2 contracts, CPU state mapping и сопоставимые внешние evidence для каждой формы |

Не вводить один общий `supported-v2` переключатель. Каждая новая форма требует
отдельного решения о lowering, fallback, сравнении состояния и prerequisites.
Ни native execution, ни память, ни исключения этапа 2 не наследуются этапом 3
автоматически.

### S3-19b / S3-20: узкие production V2 MOV load/store slices

S3-19a adds a standalone checked helper transport. S3-19b admits one production
V2 block ending in one exact qword `MOV r64,[base]`: bytes `48 8B /r`, ModRM
`mod=00`, `reg` and `r/m` in RAX..RBX, with no SIB, displacement, segment, or
address-size override. This profile uses the existing `long_decode` result and
`MemoryOperand::access_address_48`; generated ARM64 passes immutable site index
0 to the trusted Rust helper and performs no effective-address calculation.
The V2 artifact and entry type are distinct from V1. The V1 host path rejects a
V2 artifact before pointer conversion, and V1 tag 6 remains invalid.

The load result remains staged until the checked read succeeds. On success the
entry commits the destination, next RIP, and one retirement; flags are
preserved. On an architectural fault or checked-memory boundary/backend error,
the destination, flags, RIP, and retirement remain unchanged. The host returns
the complete fixed-width architectural fault payload separately from
non-architectural `MemoryError` outcomes. `MOV RAX,[RAX]` verifies that address
calculation reads old canonical RAX before the load commits.

S3-20b adds only exact qword `MOV [base],r64`: bytes `48 89 /r`, ModRM
`mod=00`, source and base RAX..RBX, with no SIB, displacement, segment, or
address-size override. It shares the V2 decoder/artifact/emitter/helper path,
but has a distinct compile flavor, site direction metadata, publication type,
and unsafe store entry; the load entry rejects a store artifact before native
entry. The checked helper captures the source value and address before one
all-or-nothing `GuestMemory::write`. A write error leaves CPU state and RAM
unchanged; success commits RIP and retirement only after the write returns, then
observes STOP. `MOV [RAX],RAX` verifies source/address aliasing. Native success,
architectural faults, and boundary/backend errors are compared with
`od_isa::execute::step` on independent CPU/RAM copies, including full state and
RAM bytes. Entry STOP, stop-RIP, and zero-budget guards are also verified to
return before the store has any memory effect.

The store entry is unsafe and may only be used with private synchronous
ordinary RAM whose writes are all-or-nothing, with no MMIO, DMA, concurrent or
delayed effects. Its backing must not be executable or alias cached guest code,
and cannot become executable while the compiled block exists. No blanket
`GuestMemory` implementation grants this capability. Code writes remain
unsupported pending S3-23 invalidation. The V2 context, adapter, and site table
are invocation-local; pointers and RAM contents do not enter a translation
key. These one-instruction compiler paths are not cached and have no general
IR, multi-instruction block, or PC-map integration. Other memory forms remain
unsupported. The physical Mac mini check is reserved for final S3-21 after
local Stage 3 work is complete.

#### S3-20c: one-byte SIB extension

S3-20c extends both existing qword operations to exactly one SIB byte:
`48 8B/89 /r`, ModRM `mod=00,r/m=100`, followed by SIB, with no displacement,
prefix, or tail bytes. The data register, SIB base, and SIB index encodings must
be RAX..RBX (0..3); index `100` (no-index) and base `101` (no-base plus
disp32), RSP/RBP, and extended registers remain rejected. Scale bits select
only 1, 2, 4, or 8. The original three-byte direct-base forms remain admitted.

The decoder remains the sole source of `MemoryOperand`; raw ModRM/SIB bits are
checked only against decoded instruction length, operation, register, base,
index, and scale. The artifact binds a four-byte bounded source buffer and an
explicit source length so source identity includes the SIB when present. Site
validation accepts decoded instruction lengths three or four and checks the
same allowed base/index/scale profile. The emitter still passes the immutable
site index to Rust; effective-address wrapping and canonical48 validation
continue exclusively through `MemoryOperand::access_address_48`. Helper ABI,
V1 tags, operation-specific V2 artifacts, store safety boundary, and commit
ordering are unchanged.

Local ARM64 native differential tests cover all 256 valid combinations per
operation (data register, base, index, and scale), full CPU/RAM comparisons
against committed `od_isa::execute::step`, little-endian store bytes, aliases,
wrap-to-canonical addressing, noncanonical faults, backend boundary failures,
and STOP/stop-RIP/zero-budget no-effect guards. Invalid truncated/tail,
displacement, no-index, no-base, extended-register, and extra-prefix forms are
rejected. The physical Mac mini check remains deferred to the final S3-21 gate;
see [the S3-20c report](../reports/STAGE_3_S3-20c_2026-10-27.md).

#### S3-20d: bounded signed displacement forms

S3-20d extends the same two qword MOV operations to ModRM `mod=01` signed
disp8 and `mod=10` signed disp32, both with or without the supported SIB byte.
For direct-base addressing, exact lengths are 3/4/7 bytes for mod00/01/10;
with SIB they are 4/5/8 bytes. Mod00 remains the existing no-displacement
profile. The source buffer is fixed at eight bytes and carries an explicit
length; the next RIP is derived from `long_decode::decode`'s instruction length.

A shared `MemorySiteV2` shape/source validator is used by compilation, host
metadata admission, and the trusted helper. It checks operation, width,
register/base/index/scale, ModRM/SIB shape, instruction length, and that the
decoded signed displacement fits the selected width. It does not re-sign-extend
raw bytes or compute an effective address. The decoded `MemoryOperand` remains
the sole source for displacement semantics, wrapping, canonical48, and full
access-range checks through `access_address_48`. Mod11, RIP-relative and
base-less addressing, no-index SIB, RSP/RBP/R8+, other prefixes/overrides, and
any trailing bytes remain rejected. Load staging, store unsafe private-RAM
requirements, helper ABI, fault payloads, commit ordering, and STOP handling
are unchanged.

Local ARM64 differential tests enumerate all valid addressing/register/scale
combinations for both operations and all three ModRM modes. They cover disp8
`-128/-1/0/1/127`, disp32 min/max and neighbors, equivalent disp8/disp32
addresses with distinct next RIPs, canonical wrap and noncanonical/endpoint
faults, backend/permission errors, guard no-effects, and late STOP after one
successful store. See [the S3-20d report](../reports/STAGE_3_S3-20d_2026-10-27.md).
Physical Mac mini execution remains reserved for the final S3-21 gate.

#### S3-20e: bounded RIP-relative disp32 forms

The existing V2 qword MOV load/store compiler additionally admits only
`48 8B/89 /r` with ModRM `mod=00, r/m=101`, a RAX..RBX data register, exactly
four displacement bytes, and a total instruction length of seven bytes.
`long_decode::decode` provides `AddressBase::NextRip`, signed displacement,
and the instruction's `next_rip`; the source bytes and decoded site remain
bound in immutable artifact metadata. Other prefixes, truncation, trailing
bytes, and other register encodings are rejected.

The trusted helper requires `state.rip == site.guest_rip` and obtains the
address only through `MemoryOperand::access_address_48(state, site.next_rip)`.
It does not rebase the site or independently calculate a RIP-relative target.
Load staging and store's private synchronous ordinary-RAM unsafe contract,
including the prohibition on executable/code aliases, are unchanged. Local
native/interpreter differential coverage includes all four data registers,
eight displacement boundaries, and identical source bytes at two guest RIPs.
Physical Mac mini execution remains reserved for the final S3-21 gate; see the
[S3-20e report](../reports/STAGE_3_S3-20e_2026-10-27.md).

#### S3-20f: bounded address-size override MOV forms

The V2 compiler separately admits only `67 48 8B/89 /r` with ModRM `mod=00`,
data and base register encodings RAX..RBX, and exactly four instruction bytes.
The decoder must report qword data width, dword address size, register base,
no index, scale 1, and displacement 0. The byte admission branch is kept
separate from the existing direct/SIB/displacement/RIP-relative shapes. The
compiler does not apply its own masks or zero extension: the runtime uses the
decoded `MemoryOperand::access_address_48` behavior for the starting address,
full qword access range, and fault checks. Source identity is exactly four
bytes and `next_rip` is decoder-derived as entry RIP plus four.

Local native/interpreter differential cases cover all 16 data/base register
pairs for load and store, full-qword data, high address-register bits ignored
by the decoded dword address size, and `MOV RAX,[EAX]` using the old base
before destination commit. Address-size truncation applies only to the starting
effective address; the qword's subsequent byte addresses advance linearly and
do not wrap at 32 bits. A sparse mapping test proves this by successfully
reading and writing bytes at `0xffffffff` through `0x100000006`, while separate
unmapped cases verify fault atomicity. Other prefixes,
ModRM modes, SIB, displacement, truncation, and trailing bytes remain rejected.
Physical Mac mini execution remains reserved for the final S3-21 gate; see the
[S3-20f report](../reports/STAGE_3_S3-20f_2026-10-27.md).

#### S3-20g: bounded signed displacement with address-size override

The existing V2 qword MOV profile additionally admits only
`67 48 8B/89 /r disp8|disp32`: ModRM `mod=01` or `mod=10`, data/base register
encodings RAX..RBX, no SIB, no RIP-relative/base-less form, and exact total
lengths of five or eight bytes. The existing `mod=00` four-byte form remains
admitted. The decoder supplies the dword-address `MemoryOperand`, signed
displacement, and `next_rip`; raw-byte validation checks the prefix/opcode,
ModRM mode/registers, source length, and agreement with that metadata without
re-decoding displacement bytes. Runtime validates the selected displacement
width and obtains the start address and complete qword range only through
`MemoryOperand::access_address_48`.

Local native/interpreter differential cases cover disp8 `-128/-1/0/1/127`,
disp32 minimum/maximum and adjacent/zero values, upper base bits ignored,
32-bit effective-address wrap, aliasing load destination/store source with the
base, and a qword beginning at `0xffff_fffc` whose later bytes continue through
`0x1_0000_0003` without 32-bit wrap. The boundary fault preserves CPU and RAM
for both operations. STOP, stop-RIP, and zero-budget guards return before
memory access for both directions. Truncated and trailing bytes, unsupported
registers, SIB, extra/reordered prefixes, and other ModRM modes remain
rejected. Other address-size-override forms stay outside this slice. Physical Mac mini
execution remains reserved for the final S3-21 gate; see the
[S3-20g report](../reports/STAGE_3_S3-20g_2026-10-27.md).

## 4. Начальные runtime bounds

- Один гостевой vCPU; компиляция блоков последовательная.
- Вход компилятора — неизменяемый bounded code buffer. Компилятор не выполняет
  гостевой код и не должен читать за границу буфера.
- Начальный предел блока: 32 guest instructions, 256 guest bytes и 4096 bytes
  emitted ARM64 code; срабатывает первый достигнутый предел.
- Блок заканчивается на управляющей инструкции, неподдержанной форме, конце
  доступного буфера или достигнутом пределе. Fault/Unsupported за уже собранным
  корректным префиксом должен наблюдаться только при достижении соответствующего
  RIP исполнителем.
- Это явные начальные ограничения, не результат измерения оптимальности.
  Переходы между блоками и cache принадлежат следующим задачам, а cache появится
  не раньше S3-18.

S3-04 реализует только decoder-backed block builder поверх immutable borrowed
`CodeView`. `base_rip + len` и entry offset проверяются без переполнения; entry
может быть любой позицией в view или ровно `code_end` и считается новой decode
boundary. Настраиваемые limits должны быть положительными и не превышать 32
instructions/256 bytes. Блок сохраняет точные `[guest_rip, next_rip)` ranges для
успешно декодированных instructions; range остаётся одной записью и при
пересечении страницы, без предположений о guest page mapping.

Decoder получает read-only `GuestMemory`, читающий только внутри view. Fetch за
концом доступного буфера создаёт `DecodeBoundary` stop metadata; это deferred
decode/fetch result, не guest `#PF`. Ошибка на entry возвращается как `NoBlock`
с тем же deferred record, чтобы dispatcher/interpreter наблюдал её в нужный
момент. Buffer exhaustion и block limits сами по себе не являются guest faults.
Unsupported decoded form не включается в block; builder останавливается перед
ней. S0-09 admission использует decoder opcode/prefix/operand metadata и точную
длину известных форм, поэтому лишние и повторные prefixes не проходят как
поддержанная форма.

Условная branch завершает построение немедленно, до чтения недостижимого хвоста
или target. Target в уже декодированном range должен совпадать с decoded
instruction start или `code_end`; если он попадает внутрь instruction, builder
останавливается перед Jcc и сохраняет ранее собранный prefix. Target вычисляется
64-битным wrapping сложением `next_rip` и sign-extended displacement, как в
interpreter. Если target совпадает с `code_end`, он классифицируется как
`CodeEnd`; если совпадает с `next_rip` текущей branch, он классифицируется как
`NextRipBoundary`, даже когда за ним есть неразобранные bytes. Это доказывает
только sequential boundary по длине декодированной branch; будущие bytes после
него не декодируются и не объявляются валидной инструкцией. Targets на уже
декодированные starts сохраняют `KnownInstructionStart`. Другие forward/unparsed
и внешние targets имеют статус `Unresolved`. Такой target пока нельзя передать
в `ir::verify` как проверенную ветвь: verifier принимает только guest-record
starts, последний `next_rip` или точный `code_end` из block metadata. S3-05
направляет unresolved branch на interpreter до Jcc, не включая и не retiring
её; дальнейшее разрешение target для dispatch остаётся S3-15.

`BasicBlock.guest_ranges` хранит точные ranges декодированного блока. Они не
содержат идентичность или версию executable code и сами не являются cache
dependencies. Code identity/version tracking принадлежит S3-17, а проверка
изменений и cache invalidation — S3-23.

### S3-05: decoded effects to typed IR

`lowering::lower` переиспользует уже decoded `long_decode::Instruction` и
builder admission для точных форм MOV immediate qword, ADD/SUB/CMP qword
register-register и JZ/JNZ rel8. IR хранит effects и операнды decoder output;
он не повторяет x86 decode или CPU semantics. Arithmetic lowerer читает оба
источника до destination write, CMP не пишет GPR, MOV не выдаёт flags write,
а все значения имеют W64. Каждый включённый instruction record имеет ровно
один commit.

Для разрешённой Jcc commit marker и следующий terminal `Branch` — единая
граница retirement и выбора RIP. Между ними запрещены helper calls,
safepoints и любые наблюдаемые exits. Target `CodeEnd` принимается verifier
только при точном совпадении с `Program.code_end`, скопированным из
`BasicBlock`/валидированного `CodeView`; все guest records обязаны лежать до
этого exclusive end. Это не произвольный список разрешённых адресов.

Unresolved forward/external Jcc не включается в IR records и не коммитится.
Непустой prefix завершается `NeedInterpreter` на RIP этой branch; если prefix
пуст, lowering возвращает `NoNativePrefix`. Mid-instruction target завершается
`Unsupported` до Jcc. Unsupported form также даёт явный Unsupported exit на
problem RIP. Buffer/limit completion использует последний `next_rip`. Любой
`DecodeBoundary` остаётся отдельным deferred metadata; lowering не объявляет
его guest fault или успешным code end. Публикация и runtime dispatch ещё не
подключены.

### S3-06: checked ARM64 encoder

`arm64::Encoder` — project-owned word buffer для текущих используемых A64
форм: MOVZ/MOVK, register ADD/SUB/AND, immediate ADDS/SUBS, LDR/STR X,
LDARB W, STP/LDP X, B/BL/B.cond, CBZ/CBNZ W/X, BLR и RET. Это encoder, а не
native-code publisher; он не управляет executable memory.

GPR operands — typed registers 0–30. Encoded register 31 доступен через явные
`Zero` operands либо `Sp` base/immediate source operands, чтобы не смешивать
XZR/WZR и SP. Для ADDS immediate destination 31 означает ZR, а source 31 — SP;
типизированный API отражает это разделение. MOVZ/MOVK принимают halfword index
0–3 и u16 value; unshifted arithmetic immediate — 0–4095. LDR/STR X принимают unsigned byte offset 0–32760, кратный
8 (imm12 scaled by 8). STP/LDP X принимают signed byte offset −512…504,
кратный 8 (signed imm7 scaled by 8). LDP отвергает одинаковые destination
registers; writeback STP/LDP отвергает base, совпадающий с pair register.
LDARB имеет только register/SP base.

Текущий typed B.cond subset — EQ и LO, использованные fragment translator.
Local labels связаны с owner ID encoder; label другого encoder отвергается при
bind и добавлении branch/call fixup. Labels создают fixups для B/BL (signed imm26 words:
−2^25…2^25−1) и B.cond/CBZ/CBNZ (signed imm19 words:
−2^18…2^18−1). Word displacement считается от адреса самой branch
instruction; кодовый буфер состоит из выровненных 32-bit words. `finish`
возвращает код только после успешного разрешения всех использованных labels и
проверки диапазонов; unresolved/out-of-range fixup не выдаёт partial buffer.
`EncodedCode::to_le_bytes` сериализует каждое слово little-endian. Direct BL
сохраняет link opcode `0x94…`, отдельно от B `0x14…`.

Точные слова для обеих сторон сверены с локальным assembler fixture
[`arm64_encoder.s`](../../crates/od-jit/tests/fixtures/arm64_encoder.s),
собранным Apple clang 21.0.0 (`-target arm64-apple-macosx15.0`) и прочитанным
через `otool -s __TEXT __text`; encoder unit test фиксирует соответствующие
слова, включая BL/B relocation, register-31 roles и LE byte order. Fixture не
является runtime dependency. Эта проверка подтверждает только перечисленные
формы и operand vectors; out-of-range cases проверяются отдельно unit tests.
S3-06 не добавляет relocations к внешним symbol addresses, veneers, relaxation,
register allocation, helper ABI или executable memory.

## 5. Runtime boundary (S3-02)

Legacy fragment entry продолжает использовать `FragmentState` и собственную
сигнатуру `(*mut FragmentState, u64, *mut u64, *const u8) -> u32`. Этот путь
не является новым runtime ABI и не преобразуется неявно в architectural state.
Переход к нему из `LongModeState` возможен только через существующий
fallible `FragmentState::try_from(&LongModeState)`; потеря дополнительных
регистров, flags и режима запрещена.

Будущий block entry внутри одной linked Rust build принимает non-null
`*mut RuntimeContextV1` и возвращает raw `u32` exit tag. `RuntimeContextV1` —
versioned `repr(C)` record: `state` — opaque non-null `*mut LongModeState`,
`initial_budget` — входной неизменяемый лимит этого вызова, `remaining_budget`
и `retired_instructions` — выходы, `stop_requested` — обязательный non-null
указатель на живой выровненный atomic byte. `stop_rip_enabled` задаётся raw
значением 0 (disabled) или 1 (enabled); иные значения и ненулевой `reserved`
являются host contract failure. При disabled `stop_rip` игнорируется. Ни один
из этих полей не имеет Rust `Option` encoding.

При любом определённом runtime exit `remaining_budget` равен начальному
лимиту минус число retired guest instructions, а `retired_instructions`
содержит именно число инструкций, уже committed этим вызовом. Их сумма
сохраняет бюджет при переходе dispatcher между blocks; `NeedInterpreter`
возвращает RIP неподдержанной инструкции после ранее committed префикса и
только committed state, поэтому interpreter может продолжить с остатком
бюджета без двойного retirement. Инструкция
retired только после полного commit её effects и next RIP. `initial_budget` не
меняется. `LongModeState` не является C record: native code получает `gpr`,
`rip` и `rflags` по `offset_of!` значениям, вычисленным в том же `od-jit`
build, и не читает другие поля. Эти offsets не являются стабильным on-disk,
IPC или cross-version ABI. Внешние raw ABI используют только примитивы,
указатели и явно `repr(C)` records; Rust enum, `Result`, `Option` и trait
objects через такую границу не передаются.

Host wrapper формирует `context` из валидных Rust references и удерживает их
заимствования вместе с `LongModeState` и `AtomicU8` на весь call; pointers
должны быть non-null, aligned и доступны по назначению. Raw ABI не позволяет
достоверно проверить lifetime или доступность произвольного address. Их
нарушение — ошибка вызывающей стороны с неопределённым поведением, не
диагностируемый runtime exit и не guest fault. Перед generated call host
wrapper проверяет scalar fields `reserved == 0` и `stop_rip_enabled ∈ {0, 1}`;
невалидные значения являются host-side contract failure. Stop byte читается
атомарно с acquire порядком; host публикует запрос с release порядком. Ноль
означает отсутствие запроса. При отсутствии cooperative-stop источника host
передаёт адрес собственного нулевого atomic byte, не null.

Block exit tags (`repr(u32)`; остальные значения — host contract failure):

| Tag | Значение | Действие dispatcher |
|---|---:|---|
| `BlockComplete` | 0 | Продолжить dispatch с committed state/RIP |
| `BudgetExhausted` | 1 | Вернуть budget exit |
| `NeedInterpreter` | 2 | Передать interpreter на том же committed RIP/state |
| `StopRequested` | 3 | Вернуть cooperative stop |
| `StopAddress` | 4 | Остановиться на `stop_rip`, не исполняя инструкцию по этому RIP |
| `Unsupported` | 5 | Не исполнять форму; interpreter допускается только при явном fallback policy |
| `DispatchCandidate` | 8 | Jcc committed один раз; вернуть выбранный RIP и counters будущему dispatcher |

`GuestFault` (tag 6) и `EventPending` (tag 7) зарезервированы и недопустимы в
runtime contract v1: начальный S0-09 профиль не содержит faulting forms или
event input/model. Возврат этих или любого неизвестного tag — host contract
failure. В будущей версии fault exit потребует отдельного raw fault record с
явными полями, до его появления гостевой fault не заявляется.

Локальный срез S3-15 вводит typed `Operation::DispatchBranch` и runtime tag
`DispatchCandidate` (8). Terminal выбирает Jcc edge и выполняет единый guest
commit, затем возвращает выбранный RIP в canonical state с точными
`remaining_budget`/`retired_instructions`; он не исполняет следующий block и
не повторяет branch. Target и fallthrough допускаются только как boundaries
полного непрерывного fixture-v1 decode, переданного через
`AdmittedInstructionBoundaries`; verifier также требует, чтобы IR records
принадлежали этому admission и `code_end` совпадал. Dispatch compiler сверяет
полный decode с теми же immutable bytes, отвергает entry и все builder ranges,
которые не входят в fixture boundaries. Bounded builder по-прежнему
останавливается сразу после branch и не читает tail. Legacy `compile_block`
не получает admission и сохраняет прежний `BlockComplete`/`NeedInterpreter`
контракт. Итеративный `od_jit::dispatch::run` продолжает только к admitted RIP
после положительного retirement и сверяет `retired`/`remaining` с входным
budget и размером emitted блока. Начальный code_end предшествует budget=0;
при нулевом budget блок не компилируется. `od_host::PublishedBlockExecutor`
держит mapping только внутри callback publish/execute/drop, затем разрешает
следующую итерацию. Unsupported/runtime exits возвращаются вызывающему без
fallback. Сам S3-15 path не добавляет cache, CLI multi-block cache use или полную
S3-15 corpus/native-oracle acceptance.

### S3-18a: one-entry fixture-dispatch cache partial

`od_jit::dispatch::run_cached` is a separate opt-in Rust path over the same
fixture-v1 `AdmittedInstructionBoundaries`. It owns exactly one compiled block
and an opaque published handle supplied by a generic backend; `od-jit` does not
depend on `od-host`. `od-host::PublishedBlockExecutor` implements the cached
backend separately from its existing publish/execute/drop callbacks. On a hit,
the bounded decoder rebuilds the block record and compares the complete
`TranslationKey` (including full CodeView bytes, terminal, fetch ranges and
caller-owned dependency identity/version spans); lowering, allocation,
emission and publication are skipped. Source bytes and dependency snapshots
are checked before publication and again before each entry. A stale snapshot
returns an error without retry. On a miss, the previous owner is dropped
before publication to respect the current single-live-MAP_JIT lease.
An early lower-bound check rejects source/dependency payloads that cannot fit
before copying them into a `TranslationKey`.

The cache constructor rejects any capacity other than one and enforces a
512-KiB maximum logical entry budget. Accounting includes key contents,
compiled code and state-map contents, plus an equivalent code/map publication
copy. It excludes allocator capacity, page rounding and opaque host-handle
internals, so it is not an RSS bound. Runtime budget and STOP guards retain the
existing ordering, including a fresh invocation-local STOP flag and boundary
resampling; zero-budget and code-end checks do not compile or publish.
The native fixture test compares cached and uncached results and reuses the
resident owner. This subsection documents only the one-entry register-only
path. A separate multi-entry cache partial is described below; it does not
change the `SingleEntryBlockCache` API or memory-lowering/production MMU
identity/invalidation scope. A read-only one-entry telemetry snapshot reports
hits, misses, successful compiles/publications and owner replacements; it does
not change the existing constructor or execution behavior. S3-18 remains open, as do
context/invalidation/retirement work in S3-22/23/25.

### S3-18b: bounded host publication arena partial

`od-host::PublishedArenaBackend` owns one `MAP_JIT` region divided into a
caller-selected 1..=16 fixed slots of 4096 bytes, matching
`MAX_EMITTED_BLOCK_BYTES`. Arena length is checked and rounded to the runtime
page size. A preallocated free-slot stack and per-slot generation/PC-map
metadata allow up to the configured number of independently owned
`PublishedArenaBlock` handles. Each handle keeps the arena and global
`MapJitLease` alive; releasing the final backend/handle unmaps once and frees
the lease. Existing `PublishedCode` still owns one standalone mapping and its
release-on-return executor is unchanged. A live arena blocks standalone
publication and another arena with typed `MappingAlreadyLive` errors.

Publication reserves a free slot only after checked size/metadata validation,
copies only that slot while the current thread has JIT write access, restores
write protection, then invalidates the written instruction-cache range before
returning the handle. Full capacity does not consume a slot. No per-slot
mapping, compaction, relocation, or `od-jit` multi-entry cache policy is added.
Arena entry uses the same verified call/result decoder as `PublishedCode`;
owned slot metadata remains live throughout execution. The native test keeps A
and B live together, executes both, replaces dropped A's slot with C, and
re-executes B to check isolation. The 16-slot limit is a bounded provisional
choice only; it has no performance evidence. This host-only subsection adds
the arena, not an `od-jit` eviction policy; the separate FIFO cache partial
below covers that S3-18 slice. Memory lowering, S3-25 retirement,
SMP/concurrent MAP_JIT policy, and S3-22/23 remain open.

### S3-18c: bounded multi-entry cache partial

`od_jit::dispatch::MultiEntryBlockCache` adds a separate opt-in FIFO policy
over the shared cache-dispatch control loop. It keeps an insertion-ordered
`Vec` of full `TranslationKey`, compiled blocks and opaque published handles;
hits do not refresh order. `MultiBlockCachedBackend` exposes simultaneous
publication capacity without changing the existing release-on-return
executors. `PublishedArenaBackend` implements this capacity contract, and the
runner rejects a configured cache larger than the arena before work starts.

Limits are at most 16 entries, 65,536 actual emitted bytes total, 512 KiB
aggregate logical resident metadata, and 4096 bytes per compiled block. The
metadata budget counts each cache-entry's inline key/block/handle storage,
owned source/dependency/fetch payloads, and both compiled and published PC-map
payloads. Emitted instruction bytes are accounted separately. It excludes
allocator bookkeeping, arena page rounding and opaque host handle internals,
so it is not an RSS cap. An individually oversized candidate
is rejected before eviction. For an eligible miss, the candidate is compiled
and source/dependency validated before oldest inactive entries are evicted;
their published handles are dropped before the backend publishes the new block.
If publication fails, prior evictions stay applied. The post-publication stale
check drops the candidate without execution and never retries. Execution and
compile errors remain terminal, with no interpreter fallback. Counters report
hits, misses, successful compiles/publications, evictions, resident emitted
bytes and resident logical metadata.

Unit tests cover A→B→A reuse, self-loop budget accounting, 16-key reuse, the
exact FIFO victim on insertion 17, metadata-triggered multiple eviction,
oversized-candidate retention, malformed dependency rejection, stale
post-publication handle drop, publish-failure eviction semantics, backend
capacity checks and resident accounting. Native
ARM64 od-host tests compare arena-cached execution to the uncached path and
verify reuse. FIFO and the 16-entry ceiling are provisional. No optimality,
speedup, production-MMU invalidation, or memory-lowering claim is made. An
end-to-end repeat measurement must compare FIFO with LRU for 16-key and full
execution workloads before selecting a final policy/capacity. S3-18 remains
open; S3-22/23/25 and physical S3-14/15 acceptance remain separate work.

### S3-25a: synchronous published-entry lifetime partial

For the bounded single-vCPU fixture path, the cache entry's compiled metadata
and published handle remain borrowed for the complete synchronous backend call
and result/counter validation. `PublishedArenaBackend` holds the arena slot
metadata borrow while generated code runs and while the host decodes its raw
result. Safe points for cache eviction, clear, or slot reuse occur after the
backend has returned and the dispatch result has been checked, before the next
lookup. On execution error, the dispatcher removes the entry only after the
backend returns the error. STOP before entry does not execute or release a
resident entry; STOP returned after execution keeps its cache mapping resident.

Local tests exercise capacity-one and capacity-two A→B→A churn, neighbor-slot
isolation, STOP before/after execution, execution-error drop ordering, clear
accounting, and the existing backend-before-handle / last-handle-reopens-arena
lifecycle. This proves only synchronous inactive-entry reuse/drop behavior. It
does not add active invalidation, self-modifying-code handling, SMP
reclamation, a deferred retirement queue, or full S3-25. See the
[S3-25a report](../reports/STAGE_3_S3-25a_2026-10-27.md).

### S3-25b: capacity-two V3 lifetime partial

With an arena of exactly two slots, one compiler-issued V3 active-code handle
and one cached prefix can remain live simultaneously. A third publication
returns `CapacityFull` without dropping either. Clearing the cache while V3 is
live releases only the cached slot; after refilling it with the prefix, a
committed self-write invalidates that prefix while the V3 slot remains owned.
The freed slot can publish the fresh tail, which executes the sentinel once
without replaying the store. Re-entry through the stale V3 handle rejects the
changed source before CPU or memory effects. Dropping the old V3 handle frees
its slot for a newly compiled source-bound artifact.

The same capacity-two fixture covers STOP after the new artifact's committed
store and confirms the tail remains unexecuted. Existing V3 fault tests verify
that a store failure after the register prefix leaves that prefix committed and
does not retire the store. A backend-drop/last-handle/reopen case verifies the
arena mapping lease. This remains a synchronous one-vCPU lifetime fixture; it
does not implement concurrent/reentrant eviction, active native retirement
beyond this V3 artifact, a deferred queue, SMP reclamation, or complete S3-25.
See the [S3-25b report](../reports/STAGE_3_S3-25b_2026-10-27.md).

An S3-18d1 benchmark-only partial adds `od-bench dispatch-cache` on native
Apple Silicon. It compares the uncached publisher, one-entry cache over a
16-slot arena, FIFO(16), and opt-in LRU(16); the existing multi-entry
constructor remains FIFO. The harness accepts only exact frozen S3-17 corpus
and trace fingerprints, prepares decoder admission and interpreter oracles
before timing, and records deterministic cold/repeat, hot-set, actual
multi-block, capacity-edge, and cold-insertion scenarios. It uses a shared
calibrated repetition count so variants in each paired sample execute equal
operation counts, requires at least 200 ms per sample, validates the full
observable register state/RIP/compared RFLAGS and dispatch counters, and writes
JSON atomically. This harness has not been run; no cache policy or performance
conclusion follows from its existence. S3-18 remains open.

Compile result остаётся Rust-side: успешная компиляция даёт непубликованный
код/metadata; неподдержанная форма классифицируется как `Unsupported`, а
ошибка декодирования, размера или внутреннего контракта — как compilation
failure. Только runtime `NeedInterpreter` является запросом продолжить через
interpreter, причём с неизменённого RIP и корректно committed state.
`Unsupported` не превращается в fallback сам по себе. Guest fault будет guest
результатом только после добавления fault record в следующую версию; сейчас tag
зарезервирован. Неизвестный raw tag, invalid pointer, mapping/publication
failure, helper ABI violation и panic/unwind — ошибка host/runtime, не guest
exit.
`StopAddress`, `StopRequested`, `BudgetExhausted`, `Unsupported` и
`NeedInterpreter` не retire текущую инструкцию; `remaining_budget` и
`retired_instructions` всё же отражают ранее committed инструкции текущего
вызова. `BudgetExhausted` возвращается только с нулевым remaining budget.

На каждой границе guest instruction entry проверяет включённый `stop_rip`,
затем cooperative STOP, затем оставшийся budget, и только потом fetch/decode.
Таким образом, STOP и budget не скрывают stop-at-RIP, а fetch не выполняется
после любого из этих выходов. Commit RIP/state для уже завершённой инструкции
сохраняется.

Scalar helper ABI alias: `(operation: u32, lhs: u64, rhs: u64, old_rflags: u64,
output_rflags: *mut u64) -> u32`. Operations: Add=1, Subtract=2, Compare=3;
reserved status tags: Success=0, Unsupported=1, InvalidArgument=2. Реализация
`od-isa::integer_flags` и тонкий `od-jit` adapter добавлены в S3-11. Helper не
меняет state, RIP или budget, пишет только `output_rflags` при Success и не
unwinds через ABI.

Generated ARM64 code следует AAPCS64: после helper call считать volatile
`X0–X17`, `X30`, `NZCV` и caller-clobbered SIMD registers уничтоженными; не
оставлять в них live значения. Сохранять `X19–X29`, держать `SP` выровненным на
16 байт и не использовать зарезервированный `X18`. Callee обязан сохранить
`X19–X29`; это правило не разрешает emitter временно держать live значения в
callee-saved регистрах без корректного prologue/epilogue.

Executable handle владеет только `od-host`: он публикует код, синхронизирует
instruction cache и удерживает mapping живым весь generated call. `od-jit`
возвращает code bytes/metadata и не зависит от `od-host`; host failure при
allocate/publish/invoke остаётся host-side. Старые C ABI и fragment mapping
остаются совместимыми.

S3-07 добавляет `od_host::PublishedCode`: конструктор `publish(&[u8])` создаёт
private `MAP_JIT` region, копирует bytes с выключенной write protection,
восстанавливает protection через RAII guard, вызывает `sys_icache_invalidate` и
только затем отдаёт owning handle. Любая ошибка до handle publication возвращает
host error; успешный handle освобождает mapping при Drop. Generic `PublishedCode`
не предоставляет safe invocation arbitrary bytes; только узкие verified entry
API могут исполнять generated code, удерживая owner на весь вызов.

Apple разрешает Hardened Runtime процессу с
`com.apple.security.cs.allow-jit` только одну `MAP_JIT` region. `od-host`
резервирует не более одной живой region: standalone `PublishedCode` или один
`PublishedArenaBackend`; вторая отдельная region получает `MappingAlreadyLive`,
а legacy C ABI использует существующий `STATUS_BUSY`. S3-18b добавляет
предварительный фиксированный slot allocator внутри этой region для нескольких
verified blocks. Он не создаёт per-block mappings и не подключает multi-entry
cache/eviction. Это ограничение касается mappings, создаваемых `od-host`; OS
остаётся последним контролем, если другая библиотека уже заняла разрешённую
process JIT region. Полный multi-block cache всё ещё требует od-jit integration,
invalidation/lifecycle и проверки capacity (см. ISSUE-021).

`pthread_jit_write_protect_np` меняет доступ текущего потока. Publisher
восстанавливает protection для своего потока до публикации и не предоставляет
post-publication writes. Это не process-wide synchronization с посторонним JIT
кодом. Внутри `od-host` generated execution начинается только после полного
copy/cache-sync; конкурентная публикация второго собственного mapping
отвергается. Выбор `pthread_jit_write_protect_np` несовместим с JIT write
callback allowlist entitlement, который его запрещает.

S3-08 фиксирует entry ABI в `runtime::BlockEntry`: `x0 = *mut RuntimeContextV1`,
raw exit в `w0`; `x19-x29`, LR и SP сохраняются, SP остаётся 16-byte aligned,
frame chain строится через `x29/x30`; `x18` не используется. Остальные caller-
clobbered GPR, NZCV и volatile SIMD нельзя считать живыми через helper call.
Context offsets экспортируются только как `offset_of!` constants.

Scalar helper получает `(operation: w0, input1: x1, input2: x2, old_flags: x3,
output: x4)` и возвращает raw status в `w0`. Успех — 0; ненулевые и неизвестные
status кодируются entry как `RAW_HELPER_CONTRACT_FAILURE_V1`, который host
отвергает отдельным `BlockEntryError::HelperContractFailure` до разбора guest
exit tag. Это host-contract failure, не `RuntimeExitTag`, guest exception или
interpreter fallback. Entry staging output в локальном aligned stack slot;
только status 0 позволяет прочитать его и обновить test probe outputs. Для error
entry не читает staging output, не retire-ит instruction и не трогает guest
state/outputs, даже если helper записал staging slot до возврата ошибки.

Native ARM64 probe в `od-host` tests сгенерирован checked `arm64::Encoder` и
исполнялся только через raw `BlockEntry` function pointer в unsafe test boundary,
пока `PublishedCode` owner жив. Test wrapper проверяет `x19-x29`, а inline-asm
canaries — сохранение `D8-D15`; helper делает вложенный host call и принудительно
портит volatile `x1-x17`. Это ABI-only probe: x86 arithmetic/helper semantics,
generic safe invocation arbitrary bytes, IR lowering и dispatcher в нём
отсутствуют. Сгенерированные frames не поддерживают unwind; host helpers не
должны panic/unwind через generated code. В production safe entry boundary может
вызвать только code object, чьё происхождение/verification задаёт будущий
compiler; этот срез не добавляет general raw-pointer invocation API.

Локально проверено на macOS 26.7.1 ARM64 / Xcode macOS SDK 27.0; SDK помечает
`pthread_jit_write_protect_np` как доступный с macOS 11.0; Cargo test binary
имеет `LC_BUILD_VERSION minos 11.0`. Подписанный Swift smoke binary имеет
`minos 26.0`, потому что его локальная команда сборки не задаёт deployment target.
В проекте не найден явный deployment target конечного приложения. Ad-hoc подписанный Hardened Runtime
VMHost smoke с allow-jit entitlement исполнил fragment через launcher на этой
машине. Developer ID/notarization, packaged application и другой Mac остаются
непроверенными под ISSUE-004.

S3-09 добавляет узкий fixed-home allocator baseline для verified linear `ir::Program`.
В порядке определения temps выбираются permanent homes x9..x12 (настраиваемый
бюджет 0..=4); остальные W64 temps получают уникальные SP-relative 8-byte
slots. Входной `TempId` остаётся sparse identity и не используется как индекс
контейнера. W8/W16/W32 отклоняются. Eviction, slot reuse, coalescing,
rematerialization и register reuse не выполняются.

Helper calls пока отсутствуют в IR. Компилятор явно передаёт allocator boundary
как `BeforeOperation` или `AfterOperation`; boundary у terminal или после него
отклоняется, чтобы не вставлять вызов после последнего commit перед branch/exit.
Liveness берётся из `Constant`/`ReadGpr` definitions,
`Integer` input/result и `WriteGpr` uses. Каждое live-through значение в x9..x12
сохраняется в собственный capture slot до helper и восстанавливается после;
spill homes переживают вызов непосредственно в frame. Этот контракт не
вставляет observable operation или call между guest commit и terminal branch.
Frame layout резервирует caller-provided prefix, выравнивает полный размер до
16 bytes и проверяет overflow, instruction offset bounds, slot containment и
одновременные live-home conflicts. x13..x15 доступны только как transfer
scratch; x18 запрещён и allocator-owned code не меняет x19..x29/LR/SP.
Counters дают статическое число spill slots, spill loads/stores, helper
saves/reloads и соответствующих transfer words/bytes; это baseline cost
estimate, не benchmark.

`od-host` native ARM64 test исполняет transfer sequence на текущем allow-jit
host: четыре register homes и два stack spills переживают helper, который
пишет в volatile x1..x17, после чего generated entry возвращает сумму 21.
Test wrapper дополнительно проверяет callee-saved GPR и D8-D15 canaries.
Проверка не является guest instruction lowering или производительным
сравнением. Allocator baseline реализован; потребитель и дополнительные
проверки liveness описываются следующими этапами.

### S3-10: staged integer data effects

`emitter::emit_data_effects` принимает только verifier-checked `Program` и
`Allocation`, созданную для того же объекта Program. На границе S3-10 начальная
версия emitter принимала W64 `Constant`, `ReadGpr`,
`Integer(Add|Subtract|Compare)` и `WriteGpr`; она
отвергает другие widths, `FlagEffect` кроме `Arithmetic` у integer operations,
Jcc/branch terminals и exits кроме `BlockComplete`. Для arithmetic оба входа
считываются до записи результата, поэтому aliasing и `dst == src` сохраняют
правильный operand order. Add/Sub записывают только во временное значение;
`CMP` вычисляет subtraction result в отдельный scratch output и не делает
guest-GPR write. Generated sequence использует allocation homes/spills и
caller-saved scratch registers, сохраняет SP и не вызывает helper.

Entry принимает test/staging context с отдельной копией четырёх GPR и output
buffer. `WriteGpr` изменяет только staged GPR copy; outputs экспонируют temps,
кандидаты GPR и CMP scratch. Возвращаемый
`DATA_EFFECTS_UNCOMMITTED_V1` — host-contract sentinel, не `RuntimeExitTag`.
Emitter не изменяет canonical `LongModeState`, flags, guest RIP, budget или
retirement count; результат нельзя считать committed guest instruction или
`BlockComplete`. На границе реализации S3-10 arithmetic flags были явно
помечены `ArithmeticIncomplete`, а terminal Branch не исполнялся. S3-11 ниже
добавляет eager flags и staged branch selection; архитектурный commit и state
recovery остаются отдельной задачей S3-12.

Native od-host ARM64 test проходит decoder → bounded block → typed IR → allocator
→ production emitter → `PublishedCode` → execution при budgets 0..=4. Он
проверяет alias/equal operands, overflow patterns, multi-lane constants,
spill transport, CMP scratch и неизменность canonical state. Это локальное
staging evidence, не физический x86 comparison и не performance benchmark.

### S3-11: eager flags и staged branch selection

`od-isa::integer_flags::updated_arithmetic_flags` задаёт один источник W64
ADD/SUB/CMP semantics для CF/PF/AF/ZF/SF/OF. Он заменяет только mask `0x8d5`,
сохраняет все остальные RFLAGS bits (включая bit 1), считает carry/borrow,
signed overflow, auxiliary carry и parity явно и не использует host NZCV.
`od-jit::runtime::arithmetic_flags_helper_adapter` — тонкий ABI adapter;
неизвестные operation tags и null output возвращают ненулевой status, а
output pointer записывается только при успешном вычислении.

Data-effect emitter вызывает helper до materialизации результата операции,
передаёт оба входа и прежние staged flags, проверяет status прежде чем читать
helper output или публиковать candidate результата. Ошибка возвращает отдельный
`RAW_HELPER_CONTRACT_FAILURE_V1`; она не меняет canonical state и не выбирает
guest exit. После успеха emitter вычисляет transport result, фиксирует
staged flags и снимки output при определении temp/GPR candidate, чтобы helper
clobber не уничтожил уже мёртвый allocator home. MOV не вызывает helper и
сохраняет flags.

JZ/JNZ теперь читают staged guest ZF и выбирают target/fallthrough RIP после
последнего guest instruction. Flags, GPR candidates и выбранный RIP остаются
в отдельном staging context; generated entry возвращает
`DATA_EFFECTS_UNCOMMITTED_V1`, не retire'ит инструкцию и не обновляет
canonical `LongModeState`, guest budget или успешный `BlockComplete`. Helper
call и branch selection не создают commit boundary между ними.

Проверки включают literal flag vectors, comparison с interpreter, все бюджеты
allocator 0..=4, helper-clobber/failed-status cases, CMP без GPR write, MOV
preservation и native taken/not-taken JZ/JNZ. Это evidence для текущего W64
slice, не полное RFLAGS/x86 equivalence и не физический x86 comparison.

### S3-12: committed runtime state and host-PC metadata

`block_emitter::emit_runtime_block` consumes a verifier-checked `Program` and
the matching allocator `Allocation`. It emits the current W64 register-only
profile into a runtime entry; unsupported operation shapes are rejected during
emission. A private aligned frame stages four guest GPRs, flags and candidate
RIP. Canonical `LongModeState` is updated only after the instruction's effects
have succeeded. The commit sequence copies staged GPRs/flags/RIP and then
updates `remaining_budget` and `retired_instructions`; its host range is marked
`CommitInProgress` and inadmissible for snapshots. The following zero-length
guest-boundary point records the completed commit separately.

Before every included instruction, generated code checks enabled `stop_rip`,
then acquire-loads cooperative STOP, then checks remaining budget. No fetch or
instruction effect runs after one of these exits. Unsupported/NeedInterpreter
terminals sample the same guards before returning at their target RIP;
BlockComplete returns at the already committed final RIP. A Jcc selects its
target/fallthrough into staged RIP as part of its final instruction commit;
there is no observable exit between that selection and publication of the
state. These are planned exits for this emitter, not proof of recovery at every
possible host PC.

`PcMap` stores checked, contiguous, aligned half-open host byte ranges, plus
separate aligned guest commit-boundary points. Each range names either a fixed
guest RIP or the canonical RIP at that runtime point, optional instruction
index, phase and snapshot admissibility. The Jcc terminal transfer, its return
stub and shared epilogue use canonical-RIP metadata because taken, fallthrough,
and earlier planned exits can reach those ranges with different committed RIPs.
Per-instruction guard/helper failure stubs retain their fixed faulting guest
RIP. Helper call
failure has an instruction-specific return stub and map range; it returns a
host-only raw sentinel before current-instruction commit. The host wrapper uses
the unchanged canonical RIP and map site to report the failure at the exact
instruction, while earlier committed instructions and counts remain visible.
The map is cloned into `PublishedCode` metadata and remains owned with the
executable mapping. Safe `execute_block` requires production emitter
provenance and matching entry RIP; test/custom-helper code stays unsafe.

The metadata supports lookup of emitted bytes and attribution of planned
helper failures. It does not make arbitrary host crashes, SIGSEGV/#PF, Rust
panic/unwind, ABI violations or asynchronous/concurrent snapshots recoverable
guest events. There are no memory effects or fault replay in this slice. Native
ARM64 tests validate the generated register-only path and planned exits; they
are not the S3-14 physical-x86 comparison.

### S3-13: opt-in one-block fixture CLI

Fixture schema v1 accepts `--engine jit-block`, producing the distinct result
backend tag `jit_ir_block`. Example:

```sh
target/debug/od-runner --fixture CASE_V1.json --engine jit-block --json
```

The legacy `--engine jit`, direct `--hex` path and C fragment ABI retain their
existing behavior. The compatibility layer applies the existing whole-fixture
v1 static decoder admission and checks that the initial RIP is one of its
instruction boundaries before invoking
`od-jit::block_compile::compile_block`; the bounded compiler itself decodes
from the fixture's initial RIP and stops at its branch/limit without consuming
unreachable tail bytes. It reuses the
S3-04 builder, S3-05 lowering, S3-09 allocator and S3-12 production emitter,
with runtime reserved frame prefix 112 bytes and a 4096-byte emitted-code cap
checked before publication. `NoNativePrefix` remains an unsupported outcome;
compile errors do not become fallback success.

The runner publishes and executes one block exactly once with
`stop_rip = code_end`. `BlockComplete` is normalized in this order: actual RIP
at code end becomes `ReachedCodeEnd`; otherwise zero remaining budget becomes
`InstructionBudgetExhausted`; otherwise the result is unsupported because a
next block/dispatcher is required. `NeedInterpreter` and `Unsupported` are
reported as unsupported without running the interpreter. Invalid fixture/input
uses exit 3, unsupported profile/form/host uses exit 4, and compile, publication,
runtime-contract or output failures use exit 5. Schema v2 remains unsupported
for JIT execution. Completed results expose the v1 comparison snapshot and
retired count and round-trip through the existing `RunResult` schema. This is
one-block ARM64 smoke evidence, not multi-block execution, physical-x86
comparison or a performance claim.

### S3-15: opt-in fixture dispatcher CLI partial

Fixture schema v1 accepts `--engine jit-dispatch`, producing backend tag
`jit_ir_dispatch`. It validates the complete fixture-v1 decode against the
same immutable bytes and requires initial RIP to be an admitted instruction
boundary or `code_end`; the runner then uses `od_jit::dispatch::run` with
`od_host::PublishedBlockExecutor`. Each iteration publishes, executes, and
drops one block before dispatching the next. The CLI reports `ReachedCodeEnd`
before `InstructionBudgetExhausted` when a committed branch reaches `code_end`
on the final budget unit. Zero budget is exercised through the dispatcher API
because fixture-v1 validation requires a positive budget. Unsupported decode,
target, or lowering produces the unsupported result; compile, publication,
helper, counter, and runtime contract failures remain harness failures. No
interpreter fallback is attempted. Legacy `jit` and one-block `jit-block`
retain their existing backend tags and behavior. This local CLI does not
complete the S3-15 hand-case/corpus acceptance or S3-14 physical x86 comparison.

`tests/cpu/stage1_harness.py --suite oracle --engine jit-dispatch` selects the
full acceptance input set by default: the 10 unchanged manual cases plus 1000
generated cases using seed `0x5eedcafe`, with two repeats. `--local-only`
selects the same cases and repeat count but runs interpreter versus
`jit-dispatch` on the local host only. Its saved manifest and summary identify
`local_only` evidence and explicitly record that physical x86 was not run; it
cannot satisfy either hardware acceptance. The full physical path still records
interpreter/JIT/native results and host/build metadata. Dispatch remote test
preflight runs `od-testkit`, `od-runner`, and `od-x86-runner`; `od-isa` is built
as a dependency, while its unrelated full unit suite is excluded due to the
known ISSUE-023 x87 failures. A local-only run on this harness revision passed
for 10 manual plus 1000 generated inputs, seed `0x5eedcafe`, and two repeats
(4040 local engine results; 2020 cross-engine and 2020 repeat comparisons); its
manifest marks `physical_x86=not_run`. Physical triple comparison remains
pending.

### S3-16: exact bare NOP fallback partial

The local Rust API `S0_09NopAdmission` / `run_with_nop_fallback` admits only a
complete immutable decode of existing S0-09 forms plus unprefixed one-byte
`0x90`. It derives sequential and branch-target boundaries from the long-mode
decoder; it is not fixture-v1 admission and does not widen `jit-dispatch`.
At an admitted NOP, native code has already returned and its mapping has been
dropped. The dispatcher calls the existing `od_isa::execute::step` once using a
read-only fetch adapter limited to that one byte. Only `StepOutcome::Advanced`
at `rip + 1` retires the interpreter instruction; decode, step, executor and
counter errors are terminal and are never retried or converted to guest
fallback success.

One `DispatchControl` shares `stop_rip` and the same `AtomicU8 STOP` reference
with generated block guards and interpreter pre-step checks. Boundary checks
sample stop address, then STOP, then code end, then budget. Native and
interpreter retirements are reported separately and must sum to initial budget
minus remaining budget. The current implementation covers only exact NOP;
other decoded operations, memory instructions, general Stage 2 fallback,
dispatcher CLI changes, faults on writable/mapped guest memory and generic
fallback are outside this partial API.

### S3-17: typed translation identity partial

`od_jit::translation::TranslationKey::for_block` builds an immutable,
fully comparable compile identity from the complete `CodeView` bytes and
base/end, entry RIP, typed compile flavor, terminal classification, the
block's exact instruction fetch ranges, and caller-supplied dependency spans.
The bounded cached fixture path also includes the explicit
`ExecutionContextSnapshot` described in S3-22a.
Dependency spans must exactly and contiguously cover the complete code view,
including unread tail bytes that can affect admission or terminal behavior.
Because `BasicBlock` is public data, the constructor re-decodes each claimed
instruction against the same bytes, with fetch bounded to that record's exact
range, before accepting it as a source dependency.
Each span carries a nonzero address-space ID, mapping ID/version, backing
ID/version, and executable permission. Multiple mapping IDs may share one
backing ID/version to represent aliases; inconsistent versions for one
identity are rejected. `TranslationKey` equality compares all fields and all
code bytes after hashing.

The caller owns the meaning and lifetime of these stable IDs and versions and
must advance versions without wrap when mappings or backing contents change.
The current `CodeView` has no production MMU owner, so this module does not
establish production invalidation. Runtime budget, STOP, GPRs, and RFLAGS are
not key inputs. The keys are used by the opt-in partial fixture caches in
S3-18; production dispatch, MMU ownership, and complete invalidation remain
open under S3-17/18/22/23.

### S3-22a: bounded fixture execution-context identity

The cached fixture-dispatch APIs require a caller-owned, nonzero
`ExecutionContextId` generation and an explicit `ExecutionProfile`. The only
admitted profile is `S0_09Fixture + Long64 + canonical48`; other modes,
canonical widths, or semantic profiles are rejected before cache binding,
compilation, publication, or execution. `TranslationKey` includes this
context stamp/profile in addition to existing mapping/backing identities,
versions, and executable permissions. No second MMU identity layer is added.

The owner must hold a frozen execution/mapping snapshot or lease for the full
synchronous dispatch call and advance the generation without wrapping on
reset, restore, clone, or profile change. Reusing the same snapshot permits a
hit. A higher generation clears the cache and drops old published handles
before lookup/compile, so old code cannot run under the new context. Explicit
cache reset also clears entries and requires a higher generation. A profile
change under the same ID is rejected. Restore/clone therefore compiles fresh
code artifacts. GPRs, RFLAGS, budget, STOP-RIP and invocation-local STOP
pointers are not key inputs. Existing mapping/backing IDs, versions, and
permissions continue to detect their own changes.

This is a fixture contract, not production mode/MMU/TLB integration. V2 memory
load/store paths remain uncached. The current synchronous API relies on the
caller to hold the immutable owner snapshot through decode, compilation, and
publication, which excludes an intervening owner mutation. A live-owner
candidate commit is still required if future background/asynchronous
compilation can outlive or release that snapshot. Local tests cover
same-context hits, changed-generation misses and handle drop, unsupported
profile rejection before work, reset/recompile, mapping/backing/permission
changes, and exclusion of architectural state and invocation controls from the
key. Physical Mac mini execution remains reserved for final S3-21 after local
Stage 3 is complete. Local evidence is recorded in
[the S3-22a report](../reports/STAGE_3_S3-22a_2026-10-27.md).

### S3-23a: bounded fixture cache invalidation

Fixture owners may report `CodeInvalidation` only between completed
synchronous dispatch calls. Backing-content notifications identify the backing
and current version, so entries using an older version are removed across all
mapping aliases. Mapping replacement, removal, and execute-permission changes
identify the address-space/mapping pair; affected entries are removed from
lookup and their published handles are dropped before the owner supplies its
next snapshot. The invalidation APIs exist for both single-entry and bounded
multi-entry caches.

The fixture owner validates a write range and the next nonwrapping version
before mutation. A successful all-or-nothing code-byte write is followed by a
version advance and cache notification before any subsequent snapshot is
issued. A failed write leaves bytes, version, and cache untouched. This path
assumes mutation occurs only after dispatch returns; cache borrowing prevents
the fixture caller from invalidating an active entry through the safe API.

This is not complete S3-23 integration. It does not connect the interpreter,
V2 memory stores, control-plane writers, production MMU, DMA, or device
callbacks. In the current synchronous path, the caller keeps the owner snapshot
frozen through compile and publication, so an intervening owner write cannot
race that path. There is no separate live-owner candidate commit for compilation
that outlives/releases that snapshot; add one if background/asynchronous
compilation or a production owner permits such mutation. Active/self-modifying
code retirement remains separate S3-24 lifecycle work. No physical Mac mini
check was run; see [S3-23a report](../reports/STAGE_3_S3-23a_2026-10-27.md).

### S3-23c: tracked executable-code store fixture partial

The bounded host arena can reserve a typed compiler-issued V2 store slot next
to cached V1 blocks. The store handle has its own metadata/entry path and does
not enter the V1 cached-block executor. A fixture-only tracked code owner
after the entry stop-RIP → STOP → budget guards preflights the full qword
destination, executable and write permissions, nonwrapping backing-version
increment, and binds the artifact RIP/source bytes to the owner-held active
source lease before the helper can write. It also checks overlap against that
source backing, including guest aliases. It owns fixed
ordinary host RAM; guest-executable backing is never the host MAP_JIT arena.

The verified generated helper performs one all-or-nothing write. After the
native entry returns and the host decodes its result, the owner commits the
backing version and emits CodeInvalidation; the caller applies that event to
the cache before another lookup. This ordering also preserves an already
committed write when the generated result is STOP or host decoding fails.
Cache capacity excludes currently reserved typed store slots.

Native fixture coverage warms target block B and unrelated C, stores through an
alias of B's backing, observes exactly one little-endian qword write and late
STOP commit, drops B while retaining C, then recompiles/runs changed B bytes.
Other cases prove stop-RIP/STOP/budget exits do not touch the owner, reject an
unrelated artifact/source lease, active-source overlap and
range/permission/version preflight failures before mutation, and verify a backend write error leaves
bytes, version, and cache unchanged. This is one-vCPU fixed-RAM fixture work
only: it does not enable MMIO, DMA, production MMU/writer integration, SMP,
active-code invalidation, or general self-modifying-code retirement. The
existing unsafe private-data V2 store API remains unchanged and is not used
for this executable-code path. See the
[S3-23c report](../reports/STAGE_3_S3-23c_2026-10-27.md).

### S3-23d: one fixture transaction for interpreter, generated, and control writes

The one-vCPU fixture routes the existing interpreter `GuestMemory::write`,
tracked V2 generated code store, and between-call control writes through one
owner transaction. Shared preflight resolves the complete guest range to a
backing span, checks write permission, optional executable permission, the
next nonwrapping backing version, and backing-relative overlap with the active
source lease. The generated V2 path supplies its already verified site plan;
the interpreter still uses `od_isa::execute::step` and its existing MOV
semantics, without copying or re-decoding instruction semantics in the owner.

The common commit performs a single all-or-nothing byte copy and stages one
version notification. Interpreter and generated-call adapters drain and apply
that notification only after the current call returns, before another snapshot
or cache lookup. The control adapter does the same synchronously before it
returns to its caller. Failed preflight or backend writes leave bytes, version,
CPU state, and cache entries unchanged. Differential fixture tests compare
interpreter MOV store and native V2 store over independent owners, including
full CPU state/RAM, version, invalidated target, retained unrelated entry, and
the next execution of changed target code. Control writes test alias-based
invalidation and backend-failure preservation.

This is still fixture-only. The current synchronous adapter holds a frozen
owner snapshot through compile/publication; it does not provide a live-owner
candidate commit for future background/unfrozen compilation. It also does not
provide a production MMU/control owner, multi-writer synchronization,
asynchronous/shared RAM, MMIO, DMA, SMP, REP/RMW, or active-code retirement.
No Stage 2 files were changed. See the
[S3-23d report](../reports/STAGE_3_S3-23d_2026-10-27.md).

### S3-23e: region-scoped executable-write notification and redispatch

The fixture may separately identify a guest code region while a standalone
verified V2 store entry writes executable backing. The store artifact still
contains only its own instruction; the region stamp is owner metadata used to
detect that the committed write changed bytes covered by cached entries. After
the generated store call returns and its result is decoded, the owner advances
the backing version, applies `CodeInvalidated` to dependent entries (including
guest aliases), and returns the distinct `CodeInvalidated` exit. The dispatch
owner treats that as `RedispatchRequired` before another cached lookup. The
committed store is not replayed.

The supplied region must end exactly at the store's `next_rip`, so it covers
only the already executed prefix and the committing store, never a future
tail. Before allowing a write to overlap that region, the owner separately
rejects overlap with the store artifact's own source bytes using backing-relative
ranges, including through guest aliases.

The bounded test runs cached prefix and tail entries at two aliases, then a
separate V2 store entry changes bytes in the already completed prefix. It checks
that all four dependent entries are removed, the store result matches the
interpreter checkpoint, and redispatch at the tail and source observes fresh
bytes. These are separate compiled entries: no native tail is active during
the store, and this does not implement S3-24 current-block retirement. Intel
SDM Vol. 3A (Order No. 253668-082US, December 2023) §9.1.3 describes store, transfer of control, then execution as the
defined self-modifying-code sequence; the fixture demonstrates the bounded
notification/redispatch prerequisite only. See the
[S3-23e report](../reports/STAGE_3_S3-23e_2026-10-27.md).

### S3-23f: qword writes across adjacent fixture mappings

The fixed-RAM `TrackedCodeFixture` resolves ranges through one mapping or at
most two guest-adjacent mappings. A range crossing the boundary is valid only
when both mappings refer to the same backing ID and the second mapping's
backing offset equals the first offset plus its length. Preflight validates
checked guest/backing ranges and write/required execute permission on both
segments before it reserves one next backing version. The commit performs one
all-or-nothing slice copy into the contiguous backing and emits one content
notification/version change. A range ending exactly at or starting exactly on
the boundary uses its single covering mapping. A decoder-backed `CodeView` may
borrow the contiguous backing bytes across the boundary, but its cache
dependency spans still carry each guest mapping's independent identity and
version.

The native fixture maps B's `48 BA imm64` across two adjacent 4096-byte guest
pages: instruction RIP is page-end−6, immediate begins at page-end−4. A
separate checked V2 qword store replaces all eight immediate bytes across the
boundary. The test warms B, its alias, and unrelated C; it checks exact
little-endian bytes, one version increment and notification, invalidates B and
the alias, retains C as a hit, then dispatches fresh B and alias and observes
the new immediate. Failure cases (missing, read-only, or non-executable second
mapping; version exhaustion; backend failure; different backing; noncontiguous
offset; and checked arithmetic overflow) preserve CPU state, both byte ranges,
version, and cache residents. This resolver is fixture-only and capped at two
mappings; it does not implement production MMU, arbitrarily segmented accesses,
MMIO, DMA, or concurrent writers. See the
[S3-23f report](../reports/STAGE_3_S3-23f_2026-10-27.md).

### S3-23g: bounded qword writes across two different backings

The fixed-RAM fixture write owner also accepts one qword spanning at most two
guest-adjacent writable mappings backed by distinct ordinary RAM identities.
Before copying any byte it builds a fixed two-segment plan with backing index,
offset, segment length, source offset and precomputed next version. Both guest
and backing ranges, mapping permissions, backing versions, aliases and active
source overlap are checked before commit. A fixed notification batch is
prepared without allocation; the exclusive fixture transaction then copies
both segments without callbacks or re-entry (coalescing them into one slice
copy when they are contiguous in the same backing) and advances each distinct
backing version once. The generated instruction retires once. Result decoding completes
before the host applies every notification and permits another lookup.

Native, interpreter and control-write fixtures cover the split qword. The
native case warms code entries for both backing IDs and their aliases plus
unrelated C; one store changes both backing versions, invalidates all dependent
entries, retains C as a hit, and fresh dispatch observes the new MOV immediate
in the second backing. Missing, read-only, or non-executable second mapping,
version exhaustion on either backing, backend failure, active-source alias
overlap, canonical endpoint overflow, and a range that would require three
mappings preserve CPU state, RAM, versions, cache contents and retirement. This
is a single-vCPU fixture transaction; it does not claim
hardware atomicity, SMP synchronization, production MMU behavior, or support
for arbitrary segmentation. See the
[S3-23g report](../reports/STAGE_3_S3-23g_2026-10-27.md).

### S3-24a: active native block prefix invalidation partial

`ActiveCodeBlockV3` is a separate compiler-issued artifact, compile flavor,
arena metadata/slot handle, and `BlockEntryV3` ABI. It admits exactly 23 bytes
containing three instructions decoded individually by `long_decode::decode`:
`48 BA imm64; 48 89 03; 48 B9 imm64`. V3 does not wrap or call a V2 entry;
it owns the instruction guards, sequential RIP/budget/retirement commits,
dirty exit, and tail. Only the lower-level checked-store helper call primitive
is shared. Load/store V2 entries and the V1 runtime/tag 6 remain isolated.

After stop-RIP, STOP, and initial-budget entry guards, the host validates the
complete 23-byte source lease before executing any generated instruction. A
stale source returns a boundary error without changing CPU state or RAM,
including when budget would later stop before the store. The store owner then
preflights the full qword destination, active source, tail, permissions, and
next backing version before mutation. Its invocation-local
dirty output is bound for the synchronous call and set immediately after a
successful all-or-nothing write when the destination intersects the already
executed prefix. No fallible step follows byte mutation. On success the store
commits RIP `entry+13`, decrements budget, retires once, and returns
`CodeInvalidated` before executing the native `MOV RCX,imm64` tail. The host
decodes the result, releases its arena borrow, then applies the committed
notification while the typed V3 handle still pins the mapping. Cache entry
drops can therefore reclaim their arena slots before the next lookup. STOP
raised during the write does not suppress this dirty exit. An unrelated store
completes all three instructions. A store into the store instruction or the
not-yet-executed tail is rejected before writing, including through fixture
aliases.

If the store faults, the first instruction remains committed and state stays
at the store RIP; no bytes/version change. Entry guards preserve stop-RIP,
STOP, then budget priority. A change to the already executed prefix is a
bounded fixture scenario; it does not define behavior for modifying a future
instruction or replace the architectural store/control-transfer requirement
for general self-modifying code. General blocks, production MMU/writer
integration, and complete S3-24 remain open. See the
[S3-24a report](../reports/STAGE_3_S3-24a_2026-10-27.md).

### S3-27a: precise architectural fault for the V3 active-code store partial

For the exact V3 artifact above, a fixture can inject an existing
`MemoryError::Architectural` from `GuestMemory::write` at a valid mapped
destination. The checked helper transports the complete optional
`ArchitecturalFault` payload through `CheckedMemoryOutcomeV1`; the host returns
`GuestFault` while preserving the committed `MOV RDX,imm64` prefix, the store
RIP, budget/retirement, all registers and flags, and unchanged RAM/backing
versions. No notification is emitted. A STOP raised as the helper returns
does not replace that fault. Budget and stop-RIP at the store boundary prevent
the write helper from being called. Backend memory failures and helper
boundary failures remain host errors; malformed exit tags, accounting, or
fault presence flags are rejected by the host decoder without invoking native
code.

Independent fixture copies run the same prefix and faulting store through
`od_isa::execute::step`; the fault payload, full CPU state, and RAM/backing
versions are compared. Optional fault error-code and linear-address fields may
be absent because `ArchitecturalFault` models each as `Option`. This is a
bounded checked-store regression, not full S3-27 fault/PC-map coverage or a
physical exception-delivery claim. Mac mini execution remains reserved for
the final S3-21 gate. See the [S3-27a report](../reports/STAGE_3_S3-27a_2026-10-27.md).

### S3-19a: invocation-local checked memory helper ABI partial

`od_jit::memory_bridge::CheckedMemoryAdapter` borrows one `&mut dyn
GuestMemory` and its `AccessContext` for one synchronous invocation. Generated
code receives only an opaque adapter pointer; no Rust enum, `String`, or trait
object crosses the C boundary. `CheckedMemoryHelperV1` has fixed-width
arguments and a `#[repr(C)] CheckedMemoryOutcomeV1`. Its raw status values are
local to this helper: `0 Success`, `1 ArchitecturalFault`, `2 MemoryError`,
`3 InvalidArgument`. They are not `RuntimeExitTag` values and must not be sent
to `decode_block_result`. `RuntimeContextV1` is unchanged; tag 6 remains
reserved-invalid.

The admitted helper operations are fetch width 1 and integer read/write widths
1, 2, 4, and 8. For read/fetch, caller-provided staging bytes change only on
success, so a generated caller can publish a load candidate at its own commit
point. Write invokes exactly one `GuestMemory::write`; architectural errors
carry fixed vector/error-code/linear-address fields. `MemoryError` variants
that are not architectural are status 2, with fixed `error_kind` values for
unmapped, permission denied, address overflow, and backend failure; backend
text is intentionally not transported. The caller is responsible for
validating an operand address with the committed `MemoryOperand::access_address_48`
contract before the helper call.

An od-host native ARM64 test emits a standalone helper-call probe with the
checked ARM64 encoder and invokes it through its own probe entry type. The
generated probe commits a staged load candidate and retirement sentinel only
after Success, and verifies both remain untouched on backend error. Bridge unit
tests verify a single backend write and distinct architectural/boundary/backend
error classes. The probe is not a generated guest block and does not validate
guest state commit, architectural RIP/counters, guest-fault exit mapping, or
production runtime integration. The follow-on V2 production slice is specified
below. All other memory forms, including stores, RMW/atomic, MMIO, REP,
descriptor access, and direct host-memory access, remain outside the contract.

## 6. Host и этапная приёмка

Основной ARM64 host для первого JIT-среза — Apple Silicon M3 Max/macOS,
дополнительный ARM64 host — M1 Pro. Единственный физический x86 oracle,
зафиксированный в [STAND](../testing/STAND.md) и [отчёте этапа
1](../reports/STAGE_1_REPORT.md), — Mac mini с Ubuntu. Это указывает, где
проводить нужные проверки, но не утверждает, что хосты повторно проверены для
текущей ревизии.

S3-14 требует тройного сравнения нового JIT, interpreter и физического x86 для
допущенных одно-блочных программ. Полный Stage 1 corpus с несколькими блоками
откладывается до S3-15, когда появится dispatcher. Проверка только interpreter
vs JIT, старый S0 JIT, сборка или ARM64 smoke не заменяют этот критерий.

Измерения скорости относятся к S3-35 и S3-36 и проводятся раздельно для
холодной компиляции/исполнения и прогретого cache. До этих данных contract не
заявляет ускорения.
