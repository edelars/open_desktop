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

Bounded S2-30cj `FCMOVNU ST(0), ST(i)` (`DB D8..DF`) remains interpreter-only;
the native runner explicitly rejects its decoded operation for every source
index, including the historically supported `DB D9` encoding.

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

#### S3-20cq: exact aliased Dword store

The shared typed-IR Dword-store entry built for S3-20co additionally admits
only exact unprefixed `89 00` (`MOV [RAX],EAX`) alongside its existing `89 03`
form. The decoder supplies full-width old RAX as the address, and the same
old value's low 32 bits are the Dword payload; successful execution preserves
RAX and commits exactly one four-byte write before RIP/retirement and STOP
observation. The source/site descriptor is bound to the exact two bytes, RAX
base, Dword operand width and `A+2` continuation. Prefix, SIB, displacement,
other ModRM values, tail bytes and partial sources remain rejected.

The host fixture compares complete CPU/RAM state with the interpreter for an
address above 4 GiB and a four-byte transfer crossing `0xffff_ffff`. It checks
the exact callback payload, adjacent bytes, entry STOP/budget guards, callback
STOP after commit, architectural/backend write errors and noncanonical #GP
without a callback. The PC map disallows snapshots in HelperCall, AfterHelper
and commit phases; replay is permitted only on the helper's all-or-nothing
error exit. This remains private synchronous ordinary RAM and adds no owner,
MMU, cache, SMC or dispatcher integration. See the
[S3-20cq report](../reports/STAGE_3_S3-20cq_2026-11-01.md).

#### S3-20cr: exact dword load with RBX-base disp8

The shared typed-IR Dword-load artifact additionally admits exactly the
three-byte singleton `8B 43 ib` (`MOV EAX,[RBX+disp8]`). The decoded
`MemoryOperand` is authoritative for the signed displacement and effective
address; runtime code does not reconstruct the address from raw instruction
bytes. A successful checked read transfers four bytes and zero-extends EAX into
RAX. The descriptor binds the exact three source bytes and the one committed
guest boundary is `A+3`; all 256 disp8 values retain distinct source/site
identity. Read failures leave the architectural state uncommitted.

This is one additional exact encoding alongside the existing `8B 03` and
`8B 00` fixtures. It does not admit other dword widths, bases, address forms,
prefixes, SIB, displacement widths, multi-instruction blocks, or a general
dword memory family. It adds no owner/MMU, cache, dispatcher, SMC, or
performance conclusion. See the [S3-20cr report](../reports/STAGE_3_S3-20cr_2026-11-01.md).

#### S3-20cs: exact dword load with RBX-base disp32

The shared typed-IR Dword-load artifact additionally admits exactly the
six-byte singleton `8B 83 id` (`MOV EAX,[RBX+disp32]`). The decoded
`MemoryOperand` supplies the signed disp32 effective address from the full-width
RBX value. A successful checked read transfers four bytes and zero-extends EAX
into RAX. The descriptor binds all six source bytes and the single guest
instruction boundary is `A+6`; the PC map uses that same boundary. The bounded
host fixture checks signed displacement cases, exact four-byte callback,
zero-extension, and no architectural commit on failed reads.

This is a separate exact encoding from S3-20cr `8B 43 ib`; it does not admit
other dword bases/addressing modes, prefixes, SIB, arbitrary multi-instruction
blocks, or a general dword memory family. It adds no owner/MMU, cache,
dispatcher, SMC, Mini, M1, Stage 2, physical-x86, or performance conclusion.
See the [S3-20cs report](../reports/STAGE_3_S3-20cs_2026-11-01.md).

#### S3-20ct: exact Dword store with RBX-base disp8

The shared typed-IR Dword-store path additionally admits exactly the
three-byte singleton `89 43 ib` (`MOV [RBX+disp8],EAX`) alongside the existing
exact S3-20co `89 03` and S3-20cq `89 00` forms. The decoder-owned memory operand supplies
the signed displacement and effective address from full-width RBX. The helper
captures the low 32-bit EAX payload and performs one checked four-byte write;
RIP and retirement commit only after a successful write, before STOP is
observed. The source/site binding is exact and the single guest boundary is
`A+3`; no dword store encodings beyond those existing exact forms and this
additional singleton are admitted.

This is separate from exact S3-20co `89 03` and S3-20cq `89 00`. It does not
admit other bases, SIB, prefixes, additional displacement widths or a general
dword store family. Production owner/MMU, SMC, cache, dispatcher, Mini, M1,
Stage 2, physical-x86 and performance remain outside this fixture scope. See
the [S3-20ct report](../reports/STAGE_3_S3-20ct_2026-11-01.md).

#### S3-20cu: exact Dword store with RBX-base disp32

The shared typed-IR Dword-store path additionally admits the six-byte
singleton `89 83 id` (`MOV [RBX+disp32],EAX`) alongside the existing exact
`89 03`, `89 00`, and `89 43 ib` forms. The decoded memory operand supplies
the signed disp32 effective address from full-width RBX. The helper performs
one checked four-byte write from the low 32 bits of EAX; RIP and retirement
commit after a successful write, then STOP is observed. Exact source/site
binding and one committed guest boundary at `A+6` preserve the singleton's
instruction boundary.

This does not admit other dword bases/addressing modes, prefixes, SIB, or
additional displacement forms beyond the enumerated exact encodings. The
S3-20cu host fixture is complemented by a separate S3-27i regression test for
canonical-end and range-overflow behavior through the same decoder-owned
`access_address_48` path. This is bounded test coverage, not a reported runtime
defect. No production
owner/MMU, SMC, cache, dispatcher, Mini, M1, Stage 2, physical-x86 or
performance conclusion follows. See the
[S3-20cu report](../reports/STAGE_3_S3-20cu_2026-11-01.md).

#### S3-20cv: exact RIP-relative Dword load with disp32

The shared typed-IR Dword-load path admits the six-byte singleton
`8B 05 id` (`MOV EAX,[RIP+disp32]`). The decoder-owned `NextRip` is `A+6`;
the effective guest address uses this next-instruction address and the signed
disp32. Source bytes bind to the site and the PC map has a single boundary at
`A+6`. This adds no other RIP-relative memory form, dword address form,
production owner/MMU, or performance conclusion. See the
[S3-20cv report](../reports/STAGE_3_S3-20cv_2026-11-01.md).

#### S3-20cw: exact RIP-relative Dword store with disp32

The shared typed-IR Dword-store path additionally admits the six-byte
singleton `89 05 id` (`MOV [RIP+disp32],EAX`). Effective-address calculation
uses the decoder-owned `NextRip` at `A+6` with the signed disp32; source bytes
bind to the site and the PC map has a single boundary at `A+6`. The host
fixture uses split code/data backing: its `(ENTRY,-1)` write changes only the
BTreeMap data RAM. It supplies no SMC or backing-alias evidence. No other
RIP-relative or dword store form is admitted by this slice. See the
[S3-20cw report](../reports/STAGE_3_S3-20cw_2026-11-01.md).

#### S3-20cx: exact address-size-32 Dword load

The shared typed-IR Dword-load path additionally admits the exact three-byte
singleton `67 8B 03` (`MOV EAX,[EBX]`). Address calculation follows the
decoder-owned address-size-32 operand: low32 RBX is zero-extended before the
canonical-address check. Exact source/site binding and one committed guest
boundary at `A+3` preserve this fixture's instruction boundary; the helper
performs a checked four-byte load with EAX zero-extension. Other addr32
forms, prefixes beyond this exact address-size override, and broader dword
memory lowering are not admitted by this slice. See the
[S3-20cx report](../reports/STAGE_3_S3-20cx_2026-11-02.md).

#### S3-20cy: exact address-size-32 Dword store

The shared typed-IR Dword-store path additionally admits only the exact
three-byte singleton `67 89 03` (`MOV [EBX],EAX`). The decoder-owned
address-size-32 access calculation zero-extends low32 RBX before canonical
checking; the store writes the low 32 bits of RAX as one checked four-byte
write. This does not admit other addr32 bases, SIB/displacement forms, or a
general Dword-store family. See the
[S3-20cy report](../reports/STAGE_3_S3-20cy_2026-11-02.md).

#### S3-20cz: exact address-size-32 Dword load with signed disp8

The shared typed-IR Dword-load path additionally admits only the exact
four-byte singleton `67 8B 43 ib` (`MOV EAX,[EBX+disp8]`). The signed disp8
and address-size-32 wrapping/zero-extension semantics come from the decoded
operand; the checked load reads four bytes and zero-extends into EAX. Exact
source bytes bind to the instruction site, and the PC map has one guest
boundary at `A+4`. This adds no other addr32 base, displacement width,
prefix combination, or general Dword-load family. See the
[S3-20cz report](../reports/STAGE_3_S3-20cz_2026-11-02.md).

#### S3-20da: exact address-size-32 Dword load with signed disp32

The shared typed-IR Dword-load path additionally admits only the exact
seven-byte singleton `67 8B 83 id` (`MOV EAX,[EBX+disp32]`) with instruction
boundary `A+7`. Effective-address
calculation uses the decoder-owned address-size-32 operand and signed disp32,
including its 32-bit wrapping and zero-extension behavior. The checked load
reads four bytes and zero-extends into EAX; all source bytes bind to the
instruction site. This does not admit other addr32 bases, displacement widths,
or prefix combinations. See the
[S3-20da report](../reports/STAGE_3_S3-20da_2026-11-02.md).

#### S3-20dl: exact address-size-32 SIB Dword loads

The shared typed-IR Dword-load path additionally admits exactly the four
four-byte singleton forms `67 8B 04 0B`, `67 8B 04 4B`, `67 8B 04 8B`, and
`67 8B 04 CB` (`MOV EAX,[EBX+ECX*1/2/4/8]`). The decoder-owned address-size-32
effective address adds low32 EBX to low32 ECX multiplied by the SIB scale,
wraps the result to 32 bits, and zero-extends it before canonical-address
checking. Each operation performs one checked contiguous four-byte read; the
recorded boundary fixture covers the 4-GiB address boundary. Writing EAX
zero-extends into RAX. Source/site binding distinguishes the prefix and scale;
the PC map has the `A+4` guest boundary. Recorded fixtures cover fault, STOP,
and budget behavior. Other addr32 SIB and displacement forms remain outside
this profile. See the [S3-20dl report](../reports/STAGE_3_S3-20dl_2026-11-02.md).

#### S3-20dm: exact address-size-32 SIB Dword stores

The shared typed-IR Dword-store path additionally admits exactly the four
four-byte singleton forms `67 89 04 0B`, `67 89 04 4B`, `67 89 04 8B`, and
`67 89 04 CB` (`MOV [EBX+ECX*1/2/4/8],EAX`). The decoder-owned address-size-32
effective address sums low32 EBX and low32 ECX times the selected SIB scale,
wraps the result to 32 bits, and zero-extends it before canonical-address
checking; the high halves of RBX and RCX do not participate. Each store
performs one contiguous checked four-byte write of low32 RAX. The guest
boundary is `A+4`; source binding and PC-map coverage distinguish the prefix
and scale. Recorded fixtures cover errors, STOP, and budget behavior. Other
addr32 SIB or displacement-store forms remain outside this profile. See the
[S3-20dm report](../reports/STAGE_3_S3-20dm_2026-11-02.md).

#### S3-20dn: exact address-size-32 SIB Dword loads with signed disp8

The shared typed-IR Dword-load path additionally admits exactly the four
five-byte singleton forms `67 8B 44 0B ib`, `67 8B 44 4B ib`,
`67 8B 44 8B ib`, and `67 8B 44 CB ib`
(`MOV EAX,[EBX+ECX*1/2/4/8+disp8]`). The decoder-owned address-size-32
effective address adds low32 EBX and low32 ECX times the SIB scale plus signed
disp8, wraps the sum to 32 bits, and zero-extends before canonical checking.
Each checked operation is one contiguous four-byte read. Writing EAX
zero-extends into RAX; the guest boundary is `A+5`. Source/site binding covers
all four scales × all 256 signed disp8 values, and PC-map, atomic-fault, STOP,
and budget fixtures remain bounded to these exact forms. See the
[S3-20dn report](../reports/STAGE_3_S3-20dn_2026-11-02.md).

#### S3-20do: exact address-size-32 SIB Dword stores with signed disp8

The shared typed-IR Dword-store path additionally admits exactly the four
five-byte singleton forms `67 89 44 0B ib`, `67 89 44 4B ib`,
`67 89 44 8B ib`, and `67 89 44 CB ib`
(`MOV [EBX+ECX*1/2/4/8+disp8],EAX`). The decoder-owned address-size-32
effective address adds low32 EBX, low32 ECX times the selected scale, and
signed disp8; it wraps to 32 bits and zero-extends before canonical checking.
Each store performs one contiguous checked four-byte write of low32 RAX. The
guest boundary is `A+5`; source/site binding covers all four scales × all
256 signed disp8 values, with PC-map coverage at that boundary. Fixtures cover
atomic faults, STOP, and budget behavior. See the
[S3-20do report](../reports/STAGE_3_S3-20do_2026-11-02.md).

#### S3-20dp: exact address-size-32 SIB Dword loads with signed disp32

The shared typed-IR Dword-load path admits exactly the four eight-byte
singleton forms `67 8B 84 0B id`, `67 8B 84 4B id`,
`67 8B 84 8B id`, and `67 8B 84 CB id`
(`MOV EAX,[EBX+ECX*1/2/4/8+signed disp32]`). The decoder-owned addr32
effective address adds low32 EBX, low32 ECX times the SIB scale, and signed
disp32, wraps the result to 32 bits, and zero-extends before canonical
checking; high halves of RBX and RCX are ignored. Each load is one contiguous
checked four-byte read, writes EAX and zero-extends into RAX, and has guest
boundary `A+8`. Source/site binding distinguishes all four scales and every
disp32 source byte; PC-map, fault, STOP, and budget fixtures remain bounded to
these exact forms. See the
[S3-20dp report](../reports/STAGE_3_S3-20dp_2026-11-02.md).

#### S3-20dq: exact address-size-32 SIB Dword stores with signed disp32

The shared typed-IR Dword-store path admits exactly the four eight-byte
singleton forms `67 89 84 0B id`, `67 89 84 4B id`, `67 89 84 8B id`, and
`67 89 84 CB id` (`MOV [EBX+ECX*1/2/4/8+signed disp32],EAX`). The
decoder-owned address-size-32 effective address adds low32 EBX, low32 ECX
times the SIB scale, and signed disp32, wraps to 32 bits, and zero-extends
before canonical checking; high halves of RBX and RCX are ignored. The store
writes low32 RAX in one contiguous checked four-byte access with guest
boundary `A+8`. Source/site binding distinguishes all four scales and every
disp32 source byte; PC-map, fault, STOP, and budget fixtures remain bounded to
these exact forms. Host semantic checks use private synchronous RAM and do not
establish MMIO, SMC, owner notification, or MMU integration. See the
[S3-20dq report](../reports/STAGE_3_S3-20dq_2026-11-02.md).

#### S3-20dr: exact RBP-base Dword load with signed disp8

The shared typed-IR Dword-load path admits the exact three-byte singleton
`8B 45 ib` (`MOV EAX,[RBP+disp8]`). The decoder-owned `MemoryOperand` supplies
full-width RBP plus signed disp8 with wrapping; raw source bytes are not used to
reconstruct the effective address. A successful checked read transfers exactly
four bytes, writes EAX, and zero-extends into RAX. The immutable site binds all
256 displacement-byte values and has the single guest boundary `A+3`.

RBP-based noncanonical start or four-byte range-end failures report `#SS`;
the separate RBX-based S3-20cr host profile retains `#GP` as a control. An
effective-address wrap from `u64::MAX+1` to zero
is valid and performs one four-byte read, while a four-byte range beginning at
`u64::MAX-1` returns `AddressOverflow` before callback or architectural commit.
Read faults likewise leave state uncommitted. The bounded host cases cover
budget and STOP guards and successful callback STOP after the load commits.
Admission remains limited to this exact encoding: no other base, addressing
form, prefix, SIB, or displacement width is implied. The fixture does not
establish MMIO, SMC, owner/MMU, or hardware integration. See the
[S3-20dr report](../reports/STAGE_3_S3-20dr_2026-11-02.md).

#### S3-20ds: exact RBP-base Dword store with signed disp8

The shared typed-IR Dword-store path admits the exact three-byte singleton
`89 45 ib` (`MOV [RBP+disp8],EAX`). The decoder-owned `MemoryOperand` supplies
full-width RBP plus signed disp8 with wrapping. The helper writes low32 RAX as
one checked four-byte access; guest RIP and retirement commit only after a
successful write. The immutable site binds all 256 displacement-byte values.
The PC map classifies both `HelperCall` and `AfterHelper` as
`InadmissibleAfterEffect`, and `CommitInProgress` as
`InadmissibleDuringCommit`. The canonical successor boundary `A+3` is a
separate post-commit boundary.

RBP noncanonical start and four-byte range-end faults report `#SS`; the
neighboring RBX-base control retains `#GP`. An effective-address wrap from
`u64::MAX+1` to zero is valid. A checked range beginning at `u64::MAX-1`
returns `AddressOverflow` before write or architectural commit. Architectural
faults do not call the writer; a backend-error attempt leaves CPU state and RAM
bytes unchanged. Zero budget and entry STOP do not write.
If STOP is raised by a successful write callback, that one write and the
`A+3` state commit remain visible, and a second call at that RIP is rejected
without replay. Admission remains limited to this exact encoding; no other
base, addressing form, prefix, SIB, or displacement width is implied. The
bounded fixture does not establish MMIO, SMC, owner/MMU, or hardware
integration. See the [S3-20ds report](../reports/STAGE_3_S3-20ds_2026-11-02.md).

#### S3-20dt: exact RBP-base Dword load with signed disp32

The shared typed-IR Dword-load path admits only the exact six-byte singleton
`8B 85 id` (`MOV EAX,[RBP+disp32]`). The decoded memory operand supplies
full-width RBP plus signed disp32 with wrapping. The source/site test exercises
twelve representative signed disp32 values and binds every encoded
displacement byte to its site; this is not an exhaustive sweep of all 2^32
byte patterns. A successful checked read transfers exactly four bytes and
zero-extends EAX into RAX. The checked guest boundary is `A+6`.

The host fixture checks interpreter parity, one width-four read, RBP
noncanonical-start/end `#SS` versus the retained RBX disp32 `#GP` control,
effective-address wrap from `u64::MAX+1` to zero, and width-four
`AddressOverflow` before callback or architectural commit. Read faults leave
state uncommitted; budget and STOP guards and successful callback STOP are
covered. These checks make no claim about other bases, address forms, prefixes,
SIB, or displacement widths, and no MMIO, SMC, owner/MMU, hardware, Mini, M1,
or Stage 2 behavior is established. See the
[S3-20dt report](../reports/STAGE_3_S3-20dt_2026-11-02.md).

#### S3-20du: exact RBP-base Dword store with signed disp32

The shared typed-IR Dword-store path admits only the exact six-byte singleton
`89 85 id` (`MOV [RBP+disp32],EAX`). The decoded memory operand supplies
full-width RBP plus signed disp32 with wrapping. The JIT source/site test
checks twelve representative displacement values and changes each of the
four displacement bytes to verify that it remains bound to its site; it does
not enumerate all 2^32 possible encodings. The host parity fixture's
successful-value loop covers ten displacement values. A successful checked
write transfers exactly four bytes from low32 RAX and advances the guest
boundary to `A+6`.

The host fixture checks effective-address wrap from `u64::MAX+1` to zero,
width-four range overflow before callback or architectural commit, RBP
noncanonical-start/end `#SS`, and fault atomicity. Its RBX `#GP` control is the
disp8 store `89 43 01`; separate S3-20cu tests cover RBX disp32. Budget, STOP,
and no-replay behavior are covered. This slice makes no separate PC-map claim
and does not admit other bases, address forms, prefixes, SIB, or displacement
widths. No MMIO, SMC, owner/MMU, hardware, Mini, M1, or Stage 2 behavior is
established. See the [S3-20du report](../reports/STAGE_3_S3-20du_2026-11-02.md).

#### S3-20dv: exact address-size-32 RBP-base Dword load with signed disp8

The shared typed-IR Dword-load path admits only the exact four-byte singleton
`67 8B 45 ib` (`MOV EAX,[EBP+disp8]`). Its decoded operand uses the low 32 bits
of RBP plus signed disp8, wraps the sum modulo 2^32, and zero-extends the
effective address. The JIT source/site test binds all 256 disp8 bytes to
distinct identities and checks the single guest boundary at `A+4`. A
successful read transfers exactly four contiguous bytes and writes EAX with
zero-extension into RAX.

The host fixture compares native and interpreter state and RAM over two signed
displacement edges (`-128` and `+127`), each tested with low/high full-width
RBP values. It verifies a single width-four read spanning the 4-GiB boundary
from address `0xffffffff`; address-size truncation applies to the starting
effective address, while the access remains contiguous across the boundary.
Read errors leave CPU/RAM state uncommitted; budget, stop-address, and STOP
before/during the successful read are covered. This is address-size-32 EBP
addressing; it makes no address-size-64 RBP `#SS` claim and adds no other base,
address form, prefix, SIB, or displacement width. No MMIO, SMC, owner/MMU,
hardware, Mini, M1, or Stage 2 behavior is established. See the
[S3-20dv report](../reports/STAGE_3_S3-20dv_2026-11-02.md).

#### S3-20dw: exact address-size-32 RBP-base Dword store with signed disp8

The shared typed-IR Dword-store path admits only the exact four-byte singleton
`67 89 45 ib` (`MOV [EBP+disp8],EAX`). The decoder-owned address-size-32
operand adds signed disp8 to low32 EBP modulo 2^32, then zero-extends the
starting effective address. A successful operation performs one contiguous
checked four-byte write of low32 EAX and advances to `A+4`; address-size
wrapping applies to the starting address, so bytes of a write beginning at
`0xffffffff` continue contiguously beyond the 4-GiB boundary.

The JIT source/site test covers all 256 encoded disp8 values, each bound to a
distinct site. The bounded host fixture exercises six displacement/base pairs
with both low and high full-width RBP values. It compares native and
interpreter state and RAM, including the four-byte boundary-crossing write.
Budget and stop guards leave the operation uncommitted; callback STOP follows
the committed write. On injected architectural or unmapped write errors, the
host callback is attempted once and CPU state and RAM bytes remain unchanged.
This address-size-32 profile makes no address-size-64 RBP `#SS` claim and does
not establish other bases/forms, SIB, MMIO, SMC, owner/MMU integration,
hardware behavior, Mini, M1, or Stage 2. See the
[S3-20dw report](../reports/STAGE_3_S3-20dw_2026-11-02.md).

#### S3-20dx: exact address-size-32 RBP-base Dword load with signed disp32

The shared typed-IR Dword-load path admits only the exact seven-byte singleton
`67 8B 85 id` (`MOV EAX,[EBP+disp32]`). The address-size-32 operand uses
low32 EBP plus signed disp32; the sum wraps modulo 2^32 and the starting
effective address is then zero-extended. A successful load reads exactly four
contiguous bytes, writes EAX with zero-extension into RAX, and advances to
`A+7`. The starting address is truncated, while a transfer beginning at
`0xffffffff` continues contiguously across the 4-GiB boundary.

The JIT source/site test covers eight representative signed disp32 values
and mutates each of the four encoded displacement bytes; this is not an
exhaustive sweep of all 2^32 encodings. The bounded host fixture covers eight
`(disp32, low32 EBP)` tuples, each with low and high full-width RBP values,
and compares native/interpreter state and RAM. This address-size-32 EBP
profile makes no address-size-64 RBP `#SS` claim and does not admit other
bases/forms, prefixes, SIB, or displacement widths. See the
[S3-20dx report](../reports/STAGE_3_S3-20dx_2026-11-02.md).

#### S3-20dy: exact address-size-32 RBP-base Dword store with signed disp32

The shared typed-IR Dword-store path admits only the exact seven-byte singleton
`67 89 85 id` (`MOV [EBP+disp32],EAX`). The decoder-owned address-size-32
effective address adds signed disp32 to low32 EBP modulo 2^32, then zero-
extends the starting address. A successful checked write transfers exactly
four contiguous bytes from low32 EAX and advances to `A+7`; a write beginning
at `0xffffffff` continues contiguously across the 4-GiB boundary.

The JIT source/site test covers eleven representative signed disp32 values
and mutates all four encoded displacement bytes at each tested site; it does
not exhaust all 2^32 encodings. The host parity fixture covers ten
displacement/base tuples, each with low and high full-width RBP values, and
compares native/interpreter state and RAM. On an injected callback error the
callback is attempted once and CPU/RAM state remains uncommitted. This slice
makes no separate PC-map phase claim and does not admit other bases/forms,
prefixes, SIB, or displacement widths. No address-size-64 RBP `#SS`, MMIO,
SMC, owner/MMU, hardware, Mini, M1, or Stage 2 behavior is established. See
the [S3-20dy report](../reports/STAGE_3_S3-20dy_2026-11-02.md).

#### S3-20dz: exact RSP-base Dword load through SIB

The shared typed-IR Dword-load path admits only the exact three-byte singleton
`8B 04 24` (`MOV EAX,[RSP]`). Its decoded address64 operand uses full-width
RSP as the base and no index. A successful checked load reads exactly four
bytes, writes EAX with zero-extension into RAX, and advances the guest
instruction boundary to `A+3`.

The host fixture checks RSP noncanonical-start and range-end `#SS` faults
against a noncanonical RBX `#GP` control, four-byte range overflow before the
read callback, and no architectural commit after injected read errors. A
callback STOP after a successful read preserves the completed load. Retrying
the same published block with committed guest RIP `A+3` returns
`EntryRipMismatch { expected: A, actual: A+3 }`, leaves state unchanged, and
does not read again. This slice does not claim a separate PC-map phase or establish other SIB shapes,
MMIO, SMC, owner/MMU integration, hardware behavior, Mini, M1, or Stage 2. See
the [S3-20dz report](../reports/STAGE_3_S3-20dz_2026-11-02.md).

#### S3-20ea: exact RSP-base Dword store through SIB

The shared typed-IR Dword-store path admits only the exact three-byte
singleton `89 04 24` (`MOV [RSP],EAX`): address64 SIB with full-width RSP
base, no index, and scale 1. The decoded operand supplies the effective
address; the checked helper writes one contiguous four-byte value from low32
RAX. A successful write commits guest RIP to `A+3` and preserves registers
and flags. This bounded fixture does not make a separate PC-map phase claim.

The host test checks noncanonical RSP start and width-four range-end faults as
`#SS`, with the corresponding RBX-base `#GP` control; width-four
`AddressOverflow` is returned before the writer callback. Entry STOP and zero
budget do not write. On a backend write error, exactly one width-four attempt
is recorded while CPU state and RAM remain unchanged. If the successful
callback raises STOP, its write and the `A+3` architectural commit remain
visible. Retrying the same published block with committed guest RIP `A+3`
returns `EntryRipMismatch { expected: A, actual: A+3 }`, preserves state, and
does not issue a second write. No other base, addressing form, prefix, SIB,
or displacement width is implied. The fixture does not establish MMIO, SMC,
owner/MMU integration, or hardware behavior. See the
[S3-20ea report](../reports/STAGE_3_S3-20ea_2026-11-02.md).

#### S3-20eb: exact RSP-base Dword load with signed disp8 through SIB

The shared typed-IR Dword-load path admits only the exact four-byte singleton
`8B 44 24 ib` (`MOV EAX,[RSP+disp8]`): address64 SIB, full-width RSP base,
no index, scale 1, and a signed 8-bit displacement. The decoder-owned memory
operand computes full-width RSP plus signed disp8 with wrapping u64
arithmetic. A successful checked load performs one contiguous four-byte read,
writes EAX with zero-extension into RAX, and commits guest RIP to `A+4`.
The JIT source/site test binds all 256 displacement-byte values to their exact
sites; this does not make a separate PC-map phase claim.

The host fixture compares native execution with the interpreter and verifies
one width-four read. It checks `+1` and `-1` displacement cases, including
RSP `0x1001` with `-1` reaching `0x1000`, and RSP `u64::MAX` with `+1`
wrapping the starting address to zero. Noncanonical stack addresses at the
start and width-four range end fault as `#SS`, contrasted with an RBX-base
`#GP` control. Width-four range overflow and backend read errors do not commit
guest state. Budget/STOP guards avoid reads; callback STOP after a successful
read leaves the load and `A+4` commit visible. Retrying the same published
block with committed RIP `A+4` returns
`EntryRipMismatch { expected: A, actual: A+4 }`, preserves state, and does not
perform another read. Other bases, SIB forms, prefixes, or displacement
widths are not implied. The fixture does not establish MMIO, SMC, owner/MMU,
or hardware integration. The S3-34bm harness pins the exact JIT source/site
and host checked-state identities and requires the no-replay host evidence in
the final local-acceptance summary; see the
[S3-20eb report](../reports/STAGE_3_S3-20eb_2026-11-02.md).

#### S3-20ec: exact RSP-base Dword store with signed disp8 through SIB

The typed-IR Dword-store path admits only the exact four-byte singleton
`89 44 24 ib` (`MOV [RSP+disp8],EAX`): address64 SIB, full-width RSP base,
no index, scale 1, and signed disp8. The decoder-owned effective address is
full-width RSP plus signed displacement with wrapping u64 arithmetic. The
checked helper writes one contiguous width-four value from low32 RAX and
commits guest RIP to `A+4`. The JIT test binds all 256 displacement bytes to
their decoded signed values, source/site identities, and `A+4` boundary;
forged metadata and neighboring unsupported forms remain rejected. This does
not make a separate PC-map phase claim.

The host regression checks signed edges, wrap to effective address zero,
interpreter parity, and exactly one width-four callback. It also verifies
RSP `#SS` against an RBX `#GP` control, overflow/backend-error atomicity,
preserved RAX/RSP/flags, entry STOP/budget, callback STOP commit, and retry at
committed `A+4` without a second write. The fixture does not establish MMIO,
SMC, owner/MMU integration, general address forms, hardware behavior, or
performance. See the [S3-20ec report](../reports/STAGE_3_S3-20ec_2026-11-02.md).

#### S3-20ed: exact RSP-base Dword load with signed disp32 through SIB

The typed-IR Dword-load path admits only the exact seven-byte singleton
`8B 84 24 id` (`MOV EAX,[RSP+disp32]`): address64 SIB, full-width RSP base,
no index, scale 1, and signed disp32. The decoder-owned effective address
adds sign-extended disp32 to RSP with wrapping u64 arithmetic. A successful
checked load performs one contiguous four-byte read, zero-extends EAX into
RAX, and commits guest RIP to `A+7`.

The JIT fixture covers nine representative signed disp32 values, mutates each
of the four displacement bytes, and verifies source/site identity and the
single-instruction PC-map boundary; it does not enumerate all 2^32 encodings
or claim a separate published-map integration gate. The host fixture checks
native/interpreter parity, one width-four read, negative displacement and
wrap-to-zero cases, RSP `#SS` versus an RBX `#GP` control, width-four overflow
and backend-read-error atomicity, STOP/budget behavior, and retry at committed
`A+7` without a second read. This does not establish production owner/MMU,
MMIO/SMC, physical-hardware, or performance behavior. See the
[S3-20ed report](../reports/STAGE_3_S3-20ed_2026-11-02.md).

#### S3-20ee: exact RSP-base Dword store with signed disp32 through SIB

The typed-IR Dword-store path admits only the exact seven-byte singleton
`89 84 24 id` (`MOV [RSP+disp32],EAX`): address64 SIB, full-width RSP base,
no index, scale 1, and signed disp32. The decoder-owned effective address
adds sign-extended disp32 to RSP with wrapping u64 arithmetic. A successful
checked store writes low32 RAX in one contiguous four-byte operation and
commits guest RIP to `A+7`.

The JIT fixture covers nine representative signed disp32 values, mutates each
of the four displacement bytes, and verifies source/site identity and the
single-instruction PC-map boundary; it does not enumerate all 2^32 encodings
or claim a separate published-map integration gate. The host fixture checks
interpreter parity, one width-four write, wrap-to-zero, RSP `#SS` versus an
RBX `#GP` control, overflow and backend-write-error atomicity, STOP/budget
behavior, callback STOP commit, and retry at committed `A+7` without a second
write. This does not establish production owner/MMU, MMIO/SMC,
physical-hardware, or performance behavior. See the
[S3-20ee report](../reports/STAGE_3_S3-20ee_2026-11-02.md).

#### S3-20ef: exact address-size-32 ESP-base Dword load through SIB

The typed-IR Dword-load path admits only the exact four-byte singleton
`67 8B 04 24` (`MOV EAX,[ESP]`): address-size override, SIB, ESP base, no
index, scale 1, and zero displacement. Effective-address calculation uses
`u64(RSP as u32)`, so high RSP bits are ignored and ESP is zero-extended to
the guest address. A successful checked load performs one contiguous
four-byte read, zero-extends EAX into RAX, and commits RIP to `A+4`. If ESP is
`0xffff_fffe`, the span continues through `0x1_0000_0001`; the four bytes do
not individually wrap at 4 GiB.

The fixture checks source/site identity and rejects neighboring encodings,
including indexed SIB; the existing address64 RSP-base negative JIT coverage
remains in place. Host cases compare native/interpreter state and memory,
exercise nonzero/poisoned high RSP bits, a contiguous span across 4 GiB,
injected read-error atomicity, STOP/budget, and callback STOP followed by
retry rejection without a second read. An independent runtime review required
restoring the existing indexed-base guard; the final admission remains limited
to this no-index ESP form. This slice makes no RSP `#SS` or other
address-derived fault claim, and establishes no production owner/MMU,
MMIO/SMC, physical-hardware, or performance behavior. See the
[S3-20ef report](../reports/STAGE_3_S3-20ef_2026-11-02.md).

#### S3-20eg: exact address-size-32 ESP-base Dword store through SIB

The typed-IR Dword-store path admits only the exact four-byte singleton
`67 89 04 24` (`MOV [ESP],EAX`): address-size override, SIB, ESP base, no
index, scale 1, and zero displacement. Effective address is `u64(RSP as u32)`;
high RSP bits are ignored. A successful checked store writes low32 RAX in one
contiguous four-byte operation and commits RIP to `A+4`. If ESP is
`0xffff_fffe`, the store span continues through `0x1_0000_0001`; individual
bytes do not wrap at 4 GiB.

The host fixture compares interpreter state and memory, checks high32-poisoned
RSP values, one width-four callback, and callback STOP followed by retry
rejection with no second write. Backend-error and entry STOP/budget cases
assert unchanged state/memory and no unintended write, but do not explicitly
retry after those outcomes. This is a nonblocking test-coverage limitation,
not an identified runtime defect; see [ISSUE-045](../KNOWN_ISSUES.md). No
production owner/MMU, MMIO/SMC, physical-hardware, or performance behavior is
established. See the [S3-20eg report](../reports/STAGE_3_S3-20eg_2026-11-02.md).

Runtime follow-up `525e310` adds successful retry assertions after injected
backend write errors and each entry STOP/budget guard. Independent Sol/high
static review passed, but the changed test has not been executed. The
retryability acceptance evidence therefore remains pending in
[ISSUE-045](../KNOWN_ISSUES.md); these assertions do not change the store
contract above.

#### S3-20eh: exact address-size-32 ESP-base Dword load with signed disp8

The typed-IR Dword-load path admits only the exact five-byte singleton
`67 8B 44 24 ib` (`MOV EAX,[ESP+disp8]`): address-size override, SIB, ESP
base, no index, scale 1, and signed 8-bit displacement. The effective address
is `(low32(RSP) + sign_extend(disp8)) mod 2^32`, zero-extended to the guest
address space. The checked operation reads four contiguous bytes; crossing
4 GiB does not wrap individual bytes. A successful load zero-extends EAX into
RAX and advances RIP to `A+5`. High RSP bits do not participate. JIT source
and site identity bind all signed disp8 encodings; other forms remain outside
this admission profile.

The bounded host fixture checks interpreter parity, poisoned high32 RSP,
address wrap and a contiguous span crossing 4 GiB, one width-four read, and
callback STOP commit followed by retry rejection without a second read. No
production owner/MMU, physical-hardware, or performance behavior is
established. See the [S3-20eh report](../reports/STAGE_3_S3-20eh_2026-11-02.md).

#### S3-20ei: exact address-size-32 ESP-base Dword store with signed disp8

The typed-IR Dword-store path admits only the exact five-byte singleton
`67 89 44 24 ib` (`MOV [ESP+disp8],EAX`): address-size override, SIB, ESP
base, no index, scale 1, and signed 8-bit displacement. Effective address is
`(low32(RSP) + sign_extend(disp8)) mod 2^32`, zero-extended. The checked
operation writes low32 RAX in one contiguous four-byte access and advances RIP
to `A+5`; the bytes in a span crossing 4 GiB do not individually wrap. High
RSP bits are ignored. JIT source/site identity binds all 256 disp8 encodings.

The bounded host fixture checks interpreter CPU/RAM parity, address wrap,
poisoned-high32 RSP values, entry STOP/budget no-write plus successful retry,
backend-write-error no-commit plus successful retry, and callback STOP after a
committed write with no second write on retry. The separate S3-20eg coverage
limitation remains tracked in [ISSUE-045](../KNOWN_ISSUES.md); this slice does
not close it. No production owner/MMU, physical-hardware, or performance
behavior is established. See the [S3-20ei report](../reports/STAGE_3_S3-20ei_2026-11-02.md).

#### S3-20ej: exact address-size-32 ESP-base Dword load with signed disp32

The typed-IR Dword-load path admits only the exact eight-byte singleton
`67 8B 84 24 id` (`MOV EAX,[ESP+disp32]`): address-size override, SIB, ESP
base, no index, scale 1, and signed 32-bit displacement. The effective address
is `(low32(RSP) + sign_extend(disp32)) mod 2^32`, zero-extended. The checked
operation reads four contiguous bytes; if the start is `0xffff_ffff`, later
bytes continue above 4 GiB rather than wrapping individually. A successful
load zero-extends EAX into RAX and advances RIP to `A+8`; high RSP bits are
ignored. Source/site identity covers representative signed disp32 values and
all four encoded displacement bytes. Other address forms remain outside this
admission profile.


The host fixture checks interpreter parity, poisoned high32 RSP values,
negative boundaries including `-128` and `+127`, wrap-to-zero, cross-4GiB
contiguous reads, and entry STOP/budget guards with successful retries. It
also checks callback STOP commit and retry rejection without a second read.
This bounded slice makes no production owner/MMU or performance claim. The
separate S3-20eg coverage gap remains open in [ISSUE-045](../KNOWN_ISSUES.md);
S3-20ej does not close it. See the [S3-20ej report](../reports/STAGE_3_S3-20ej_2026-11-02.md).

#### S3-20ek: exact address-size-32 ESP-base Dword store with signed disp32

The typed-IR Dword-store path admits only the exact eight-byte singleton
`67 89 84 24 id` (`MOV [ESP+disp32],EAX`): address-size override, SIB, ESP
base, no index, scale 1, and signed 32-bit displacement. Effective address is
`(low32(RSP) + sign_extend(disp32)) mod 2^32`, zero-extended. The checked
operation writes low32 RAX in one contiguous four-byte access and advances RIP
to `A+8`; a span crossing 4 GiB does not wrap individual bytes. High RSP bits
are ignored. The bounded JIT identity fixture binds representative disp32
encodings and each source byte to the site. No other address-size-32 store
form is admitted by this slice.

The related host fixture status is recorded in [S3-20ek testing notes](../testing/JIT.md#s3-20ek-exact-address-size-32-esp-base-dword-checked-store-with-signed-disp32).

#### S3-20el: exact absolute SIB Dword load with signed disp32

The shared typed-IR Dword-load path admits only the exact seven-byte singleton
`8B 04 25 id` (`MOV EAX,[disp32]`): unprefixed, SIB with no base or index,
scale 1, Qword address size, and Dword transfer. The decoder-owned signed
disp32 is sign-extended to form the absolute 64-bit effective address; for
example, `-4` maps to `u64::MAX-3`. The operation performs one checked
four-byte read, writes EAX (zero-extending to RAX), and has the guest boundary
`A+7`. Other prefixes and neighboring encodings remain outside this exact
admission. The fixtures are bounded and do not establish physical memory,
MMU, or broad host behavior. See the
[S3-20el report](../reports/STAGE_3_S3-20el_2026-11-02.md).

#### S3-20em: exact absolute SIB Dword store with signed disp32

The shared typed-IR Dword-store path admits only the exact seven-byte
singleton `89 04 25 id` (`MOV [disp32],EAX`): unprefixed, SIB with no base
or index, scale 1, Qword address size, and Dword transfer. The decoder-owned
signed disp32 forms the absolute 64-bit effective address; `-4` resolves to
`u64::MAX-3`, where one four-byte checked write ends exactly at `u64::MAX`.
The store payload is low32 RAX and the guest boundary is `A+7`. Signed
displacements `-3`, `-2`, and `-1` fault the checked range before the
memory callback. Other prefixes and neighboring encodings remain outside this
admission. Fixtures are bounded and do not establish physical memory, MMU,
or broad host behavior. See the
[S3-20em report](../reports/STAGE_3_S3-20em_2026-11-02.md).

#### S3-20en: exact address-size-32 absolute SIB Dword load

The shared typed-IR Dword-load path admits only the exact eight-byte
singleton `67 8B 04 25 id` (`MOV EAX,[disp32]`): address-size override,
SIB with no base or index, scale 1, Dword address size, and Dword transfer.
The decoder-owned disp32 is reduced modulo 2^32 and zero-extended to form the
absolute effective address. One checked four-byte read remains contiguous
even across 4 GiB; `EAX` zero-extends into `RAX`, and the instruction boundary
is `A+8`. Neighboring prefixes and encodings remain outside the exact
admission. The bounded fixtures do not establish physical memory, MMU, or
broad host behavior. See the
[S3-20en report](../reports/STAGE_3_S3-20en_2026-11-02.md).

#### S3-20eo: exact address-size-32 absolute SIB Dword store

The shared typed-IR Dword-store path admits only the exact eight-byte
singleton `67 89 04 25 id` (`MOV [disp32],EAX`): address-size override, SIB
with no base or index, scale 1, Dword address size, and Dword transfer. The
decoder-owned disp32 is reduced modulo 2^32 and zero-extended to form the
absolute effective address. One checked contiguous four-byte write stores
low32 RAX; it can cross 4 GiB without per-byte wrap, and the instruction
boundary is `A+8`. Neighboring prefixes and encodings remain outside the exact
admission. The bounded fixtures do not establish physical memory, MMU, or
broad host behavior. See the
[S3-20eo report](../reports/STAGE_3_S3-20eo_2026-11-02.md).
Exact harness gate identities and their unexecuted status are recorded in the
[S3-34bz report](../reports/STAGE_3_S3-34bz_2026-11-02.md); they do not add
runtime evidence.

#### S3-20ep: exact RCX-indexed no-base SIB Dword load with disp32

The shared typed-IR Dword-load path admits only the exact unprefixed seven-byte
singleton `8B 04 0D disp32` (`MOV EAX,[RCX+disp32]`): address64, SIB without a
base, RCX index at scale 1, and Dword transfer. The decoder-owned effective
address is full RCX plus sign-extended disp32 with wrapping u64 arithmetic.
One checked four-byte read is performed, EAX zero-extends into RAX, and the
instruction boundary is `A+7`. Other prefixes and neighboring SIB forms
remain outside the exact admission. Runtime tests remain pending; the
[S3-20ep report](../reports/STAGE_3_S3-20ep_2026-11-02.md) records the current
evidence and limit.

#### S3-20eq: exact RCX-indexed no-base SIB Dword store with disp32

The shared typed-IR Dword-store path admits only the exact unprefixed seven-byte
singleton `89 04 0D disp32` (`MOV [RCX+disp32],EAX`): address64, SIB without a
base, RCX index at scale 1, and Dword transfer. The decoder-owned effective
address is the full-width RCX plus sign-extended disp32 with wrapping u64
arithmetic. One checked contiguous four-byte write stores low32 RAX, and the
guest instruction boundary is `A+7`. Other prefixes and neighboring SIB
encodings remain outside this exact admission. Runtime gates remain pending;
see the [S3-20eq report](../reports/STAGE_3_S3-20eq_2026-11-02.md).

#### S3-20er: exact scaled RCX-indexed no-base SIB disp32 Dword loads

The shared typed-IR Dword-load path admits only the exact seven-byte
unprefixed singletons `8B 04 4D disp32`, `8B 04 8D disp32`, and
`8B 04 CD disp32`: address64 SIB without a base, RCX index at scale 2, 4, or
8, and Dword transfer. The decoder-owned effective address is full-width
`RCX*scale + sign-extended disp32` with wrapping u64 arithmetic. Each checked
read is one contiguous four-byte access; EAX zero-extends into RAX, and the
guarded instruction boundary is `A+7`. Other prefixes, scales, and
neighboring encodings remain outside this exact admission. Runtime tests remain
pending; see the [S3-20er report](../reports/STAGE_3_S3-20er_2026-11-02.md).

#### S3-20es: exact scaled RCX-indexed no-base SIB disp32 Dword stores

The shared typed-IR Dword-store path admits only the exact seven-byte
unprefixed singletons `89 04 4D disp32`, `89 04 8D disp32`, and
`89 04 CD disp32`: address64 SIB without a base, RCX index at scale 2, 4, or
8, and Dword transfer. The decoder-owned effective address is full-width
`RCX*scale + sign-extended disp32` with wrapping u64 arithmetic. Each checked
write is one contiguous four-byte access containing low32 RAX, and the
instruction boundary is `A+7`. Other prefixes, scales, and neighboring
encodings remain outside this exact admission. Runtime gates remain pending;
see the [S3-20es report](../reports/STAGE_3_S3-20es_2026-11-02.md).

#### S3-20et: exact addr32 RCX-indexed no-base SIB disp32 Dword load

The shared typed-IR Dword-load path admits only the exact eight-byte
singleton `67 8B 04 0D disp32`: address-size-32 SIB without a base, ECX
index at scale 1, and Dword transfer. The effective address is
`zero_extend((low32(RCX) + signed_disp32) mod 2^32)`. One contiguous checked
four-byte read is performed, including the recorded 4-GiB crossing; EAX
zero-extends into RAX, and the instruction boundary is `A+8`. Other prefixes,
scales, SIB forms, and neighboring encodings remain outside this exact
admission. Runtime gates remain pending; see the
[S3-20et report](../reports/STAGE_3_S3-20et_2026-11-02.md).

#### S3-20eu: exact addr32 RCX-indexed no-base SIB disp32 Dword store

The shared typed-IR Dword-store path admits only the exact eight-byte
singleton with exactly the `67` address-size override prefix and no others:
`67 89 04 0D disp32`. The address-size-32 SIB has no base and uses ECX as
its scale-1 index. The effective address is
`zero_extend((low32(RCX) + sign-extended disp32) mod 2^32)`. One contiguous
checked four-byte write stores low32 RAX, including the recorded 4-GiB
crossing. GPRs and RFLAGS are preserved, and the instruction boundary is
`A+8`. Other prefixes, scales, and neighboring SIB encodings remain outside
this exact admission. Runtime gates remain pending; see the
[S3-20eu report](../reports/STAGE_3_S3-20eu_2026-11-02.md).

#### S3-20ev: exact addr32 scaled RCX-indexed no-base SIB disp32 Dword loads

The shared typed-IR Dword-load path admits only the exact eight-byte
singletons with exactly the `67` address-size override prefix and no other
prefixes: `67 8B 04 4D disp32`, `67 8B 04 8D disp32`, and
`67 8B 04 CD disp32`. The address-size-32 SIB has no base and uses ECX as
its index at scale 2, 4, or 8. The effective address is
`zero_extend((low32(RCX)*scale + signed disp32) mod 2^32)`. Each checked read
is one contiguous four-byte access, including the recorded 4-GiB crossing.
EAX zero-extends into RAX; other GPRs and RFLAGS are preserved, and the
instruction boundary is `A+8`. Other prefixes, scales, and neighboring
encodings remain outside this exact admission. Runtime gates remain pending;
see the [S3-20ev report](../reports/STAGE_3_S3-20ev_2026-11-02.md).

#### S3-20ew: exact addr32 scaled RCX-indexed no-base SIB disp32 Dword stores

The shared typed-IR Dword-store path admits only the exact eight-byte
singletons with exactly the `67` address-size override prefix and no other
prefixes: `67 89 04 4D disp32`, `67 89 04 8D disp32`, and
`67 89 04 CD disp32`. The address-size-32 SIB has no base and uses ECX as
its index at scale 2, 4, or 8. The effective address is
`zero_extend((low32(RCX)*scale + signed disp32) mod 2^32)`. Each checked
write is one contiguous four-byte access storing low32 RAX, including the
recorded 4-GiB crossing. GPRs and RFLAGS are preserved, and the instruction
boundary is `A+8`. Other prefixes, scales, and neighboring encodings remain
outside this exact admission. Runtime gates remain pending; see the
[S3-20ew report](../reports/STAGE_3_S3-20ew_2026-11-02.md).

#### S3-20ex: exact address64 RAX-indexed no-base SIB Dword load

The shared typed-IR Dword-load path admits only the exact seven-byte,
unprefixed singleton `8B 04 05 disp32` (`MOV EAX,[RAX+signed disp32]`). Its
SIB has no base, RAX as index, and scale 1; address size is Qword and transfer
width is Dword. The effective address is computed from the original full64
RAX plus sign-extended disp32 using wrapping `u64` arithmetic, before EAX
writeback. One contiguous checked four-byte read writes EAX and zero-extends
RAX; other GPRs and RFLAGS are preserved, and successful retirement commits
`A+7`. Entry guards, architectural faults, and backend errors before commit
preserve CPU state and RIP; a retry uses the original RAX. Noncanonical
addresses produce guest `#GP`; a checked range overflow remains a memory
operation error rather than a guest fault. Other prefixes, scales, indexes,
base forms, transfer widths, and neighboring encodings remain outside this
exact admission. See the [S3-20ex report](../reports/STAGE_3_S3-20ex_2026-11-02.md).

#### S3-20ey: exact address64 RAX-indexed no-base SIB Dword store

The shared typed-IR Dword-store path admits only the exact seven-byte,
unprefixed singleton `89 04 05 disp32` (`MOV [RAX+signed disp32],EAX`). Its SIB
has no base, RAX as index, and scale 1; address size is Qword and transfer
width is Dword. The effective address is computed from the original full64
RAX plus sign-extended disp32 using wrapping `u64` arithmetic. The store
writes low32 of that original RAX through one contiguous checked four-byte
write; RAX and all other GPRs/RFLAGS remain unchanged, and successful
retirement commits `A+7`. Entry guards, architectural faults, and backend
errors before commit preserve CPU state, RIP, and RAM; successful retry uses
the same original address and data. Noncanonical addresses produce guest
`#GP`; checked range overflow remains a memory-operation error rather than a
guest fault. A callback STOP is observed after the committed store and does
not replay it on re-entry at `A+7`. Other prefixes, scales, indexes, base
forms, transfer widths, and neighboring encodings remain outside this exact
admission. See the [S3-20ey report](../reports/STAGE_3_S3-20ey_2026-11-02.md).

#### S3-20ez: exact address64 scaled RAX-indexed no-base SIB Dword loads

The shared typed-IR Dword-load path admits only the exact seven-byte,
unprefixed forms `8B 04 45/85/C5 disp32` (`MOV EAX,[RAX*scale+signed
disp32]`), with no base, RAX index, and scale 2, 4, or 8. Address size is
Qword and transfer width is Dword. The effective address is
`(original_full64_RAX * scale + sign_extend(disp32)) mod 2^64`; both
multiplication and addition wrap, and the calculation uses the original RAX
before EAX writeback. One contiguous checked four-byte read writes EAX and
zero-extends RAX; other GPRs and RFLAGS are preserved, and successful
retirement commits `A+7`. Entry guards, architectural faults, and backend
errors before commit preserve CPU state and RIP; a retry uses the original
RAX. Noncanonical addresses produce guest `#GP`, while checked range overflow
remains a memory-operation error. Other scales, indexes, base forms, prefixes,
transfer widths, and neighboring encodings remain outside this exact
admission. See the [S3-20ez report](../reports/STAGE_3_S3-20ez_2026-11-02.md).

#### S3-20fa: exact address64 scaled RAX-indexed no-base SIB Dword store

The shared typed-IR Dword-store path admits only the exact seven-byte,
unprefixed forms `89 04 45/85/C5 disp32` (`MOV
[RAX*scale+sign-extended disp32],EAX`) for scales 2, 4, and 8. SIB has no
base and RAX as the index; address size is Qword and transfer width is Dword.
The effective address is `(original_full64_RAX * scale +
sign_extend(disp32)) mod 2^64`, with wrapping multiplication and addition.
Both address and payload use pre-instruction RAX: one contiguous checked
four-byte write stores its low32, and all GPRs/RFLAGS remain unchanged.
Successful retirement commits `A+7`.

Exact source bytes and decoded site metadata remain bound, including scale and
all four displacement bytes; trailing bytes, partial windows, forged operand
fields, and other prefixes/scales/index/base/transfer forms are rejected.
Entry guards do not access memory. A pre-commit architectural or backend error
preserves CPU state, RIP, and RAM; retry uses the original address/data.
Noncanonical addresses produce guest `#GP`, while a checked range overflow is
a memory-operation error. A callback STOP follows the committed write and
cannot replay it when re-entering at `A+7`. See the
[S3-20fa report](../reports/STAGE_3_S3-20fa_2026-11-02.md).

#### S3-20fb: exact address32 RAX-indexed no-base SIB Dword load

The shared typed-IR Dword-load path admits only the exact eight-byte,
unprefixed-except-for-address-size form `67 8B 04 05 disp32`
(`MOV EAX,[low32(RAX)+sign-extended disp32]`). SIB has no base, RAX as index,
scale 1, Dword address size, and Dword transfer width. The effective address
is `zero_extend_64((low32(original_RAX) + sign_extend(disp32)) mod 2^32)`;
calculation uses pre-instruction RAX. One contiguous checked four-byte read
writes EAX and zero-extends the result into RAX. Other GPRs and RFLAGS remain
unchanged, and successful retirement commits `A+8`.

The exact source bytes and decoded site metadata stay bound, including the
address-size prefix and all four displacement bytes. Partial windows,
trailing bytes, forged fields, extra prefixes, other indexes/scales, base
forms, and transfer widths are not admitted. Entry guards do not read memory;
a backend error before commit preserves CPU state and RIP, and a retry uses
original RAX. This address calculation always yields a zero-extended 32-bit
address; this bounded slice does not claim production MMU integration. See
the [S3-20fb report](../reports/STAGE_3_S3-20fb_2026-11-02.md).

#### S3-20fc: exact address32 RAX-indexed no-base SIB Dword store

The shared typed-IR Dword-store path admits only the exact eight-byte form
`67 89 04 05 disp32` (`MOV [low32(RAX)+sign-extended disp32],EAX`). Address
size is Dword; SIB has no base, RAX index and scale 1; transfer width is
Dword. The effective address is
`zero_extend_64((low32(original_RAX) + sign_extend(disp32)) mod 2^32)`. The
store payload is low32 of that same pre-instruction RAX. High RAX bits do not
affect the address, and RAX, other GPRs, and RFLAGS remain unchanged. A single
contiguous checked four-byte write commits at the computed address and
successful retirement advances to `A+8`. If the start is near `0xffff_ffff`,
the transfer remains contiguous into the following linear addresses; only
effective-address calculation wraps at 32 bits.

Exact source bytes and decoder-owned site metadata remain bound, including
prefix, opcode, SIB, and displacement bytes. Partial/trailing instructions,
extra prefixes, other SIB fields, transfer widths, and neighboring load/store
forms are not admitted. Entry guards perform no write. A backend error before
commit preserves state and RAM; retry uses the same original address and
payload. Callback STOP follows the committed store and re-entry at `A+8` does
not replay it. This is bounded borrowed-memory evidence, not production
owner/MMU integration. See the
[S3-20fc report](../reports/STAGE_3_S3-20fc_2026-11-02.md).

#### S3-20fd: exact address32 scaled RAX-indexed no-base SIB Dword load

The shared typed-IR Dword-load path admits only the eight-byte forms
`67 8B 04 45/85/C5 disp32` (`MOV EAX,[low32(RAX)*scale+sign-extended
disp32]`) for scales 2, 4, and 8. Address size and transfer width are both
Dword; SIB has no base and RAX index. Effective address calculation uses
pre-instruction RAX:
`zero_extend_64((low32(original_RAX) * scale + sign_extend(disp32)) mod
2^32)`, with both multiplication and addition wrapping in 32 bits. One
contiguous checked four-byte read writes EAX and zero-extends it to RAX;
other GPRs and RFLAGS remain unchanged, and successful retirement commits
`A+8`. Effective-address wrap does not wrap the bytes of a transfer that
crosses the 4-GiB boundary.

Source bytes and decoded site fields remain exactly bound, including scale
and each displacement byte; partial/trailing instructions, changed scale,
other indexes/base forms, extra prefixes, and other transfer widths are not
admitted. Entry STOP, stop-address, and exhausted-budget guards perform no
read. A backend error before commit preserves state and RIP; retry succeeds
from the same architectural input. Callback STOP follows a committed load,
and re-entry at `A+8` does not repeat the read. This remains bounded
borrowed-memory fixture coverage, not production owner/MMU integration. See
the [S3-20fd report](../reports/STAGE_3_S3-20fd_2026-11-02.md).

#### S3-20fe: exact address32 scaled RAX-indexed no-base SIB Dword store

The shared typed-IR Dword-store path admits only the eight-byte forms
`67 89 04 45/85/C5 disp32` (`MOV [low32(RAX)*scale+sign-extended disp32],EAX`)
for scales 2, 4, and 8. Address size and transfer width are both Dword; the
SIB has no base and RAX index. Effective address calculation uses the
pre-instruction RAX:
`zero_extend_64((low32(original_RAX) * scale + sign_extend(disp32)) mod
2^32)`, with scale multiplication and displacement addition wrapping in 32
bits. The store writes low32 of that same original RAX in one contiguous
checked four-byte transfer; other GPRs and RFLAGS remain unchanged, and
successful retirement commits `A+8`. Effective-address wrap does not wrap
the bytes of a transfer crossing the 4-GiB boundary.

The complete source byte sequence and decoded site fields, including scale
and all displacement bytes, remain bound. Partial/trailing instructions,
other scales/index/base forms, extra prefixes, and other transfer widths are
not admitted. Entry guards perform no write. A backend error before commit
preserves state and RAM; retry uses the same architectural input. Callback
STOP follows a committed store, and re-entry at `A+8` does not replay it.
This is bounded borrowed-memory fixture coverage, not production owner/MMU
integration. See the
[S3-20fe report](../reports/STAGE_3_S3-20fe_2026-11-02.md).

#### S3-20ff: exact address64 RAX-base Dword load with signed disp8

The shared typed-IR Dword-load path admits only the three-byte unprefixed
form `8B 40 ib` (`MOV EAX,[RAX+sign-extended disp8]`). Address and transfer
sizes are Qword and Dword respectively. Effective address calculation uses
pre-instruction full-width RAX plus signed disp8 with wrapping u64 semantics;
the resulting four-byte access is checked for canonicality and range. One
contiguous checked four-byte read writes EAX and zero-extends it to RAX. The
old RAX is used for the address before destination writeback; other GPRs and
RFLAGS remain unchanged, and successful retirement commits `A+3`.

The exact three source bytes and decoded site fields remain bound. Partial or
trailing instructions, prefixes, other ModRM fields, other data registers,
or other transfer widths are not admitted. Entry STOP, stop-address, and
exhausted-budget guards perform no read. A backend error before commit leaves
state unchanged and permits retry from the same original RAX. Canonical
address faults occur before the helper; access-range overflow is a distinct
memory error. Callback STOP follows the committed load, and re-entry at
`A+3` does not repeat the read. This remains bounded borrowed-memory fixture
coverage, not production owner/MMU integration. See the
[S3-20ff report](../reports/STAGE_3_S3-20ff_2026-11-02.md).

#### S3-20fh: exact address64 RAX-base Dword load with signed disp32

The shared typed-IR Dword-load path admits only the six-byte unprefixed form
`8B 80 id` (`MOV EAX,[RAX+sign-extended disp32]`). Address size is Qword,
transfer width is Dword, and the sole effective-address input is the
pre-instruction full-width RAX plus signed disp32 with wrapping u64
semantics. One contiguous checked four-byte read writes EAX and zero-extends
it into RAX; other GPRs and RFLAGS remain unchanged, and successful
retirement commits `A+6`.

The exact source bytes and decoded site metadata remain bound, including all
four displacement bytes. Partial/trailing instructions, prefixes, other
opcodes, bases, data registers, address sizes, and transfer widths are not
admitted. Entry STOP, stop-address, and exhausted-budget guards perform no
read. A backend error before commit leaves state unchanged and permits retry
from the same input. Noncanonical start/end addresses fault before the
helper; address-range overflow remains a distinct memory error. Callback
STOP follows a committed load, and re-entry at `A+6` does not repeat the
read. This is bounded borrowed-memory fixture coverage, not production
owner/MMU integration. See the
[S3-20fh report](../reports/STAGE_3_S3-20fh_2026-11-02.md).

#### S3-20fi: exact address64 RAX-base Dword store with signed disp32

The shared typed-IR Dword-store path admits only the six-byte unprefixed
form `89 80 id` (`MOV [RAX+sign-extended disp32],EAX`). Address size is
Qword, transfer width is Dword, and the sole effective-address input is the
pre-instruction full-width RAX plus signed disp32 with wrapping u64
semantics. The payload is low32 of that same original RAX. One contiguous
checked four-byte write is performed; all GPRs and RFLAGS remain unchanged,
and successful retirement commits `A+6`.

The exact six source bytes and decoded site metadata remain bound, including
all four displacement bytes. Partial/trailing instructions, prefixes, other
opcodes, bases, data registers, address sizes, and transfer widths are not
admitted. Entry STOP, stop-address, and exhausted-budget guards perform no
write. Architectural or backend failure before commit leaves state and RAM
unchanged; retry uses the same original address and payload. A noncanonical
access faults before the helper; address-range overflow is a distinct
memory error. Callback STOP follows the committed store, and re-entry at
`A+6` does not replay it. This is bounded borrowed-memory fixture coverage,
not production owner/MMU integration. See the
[S3-20fi report](../reports/STAGE_3_S3-20fi_2026-11-02.md).

#### S3-20fj: exact address32 RAX-alias Dword load

The shared typed-IR Dword-load path admits only the exact three-byte
singleton `67 8B 00` (`MOV EAX,[EAX]`): address size Dword, transfer width
Dword, RAX selected as both the address base and load destination. The
effective address is the zero-extended low32 of pre-instruction RAX. The
load uses that original low32 value before EAX is replaced; one contiguous
checked four-byte read zero-extends the loaded value into RAX. Other GPRs and
RFLAGS remain unchanged, and successful retirement commits `A+3`.

The exact three source bytes and decoded site metadata remain bound. Prefixes,
other opcodes, ModRM forms, address sizes, transfer widths, partial/trailing
instructions, and non-alias data/base forms are outside admission. Entry
STOP, stop-address, and exhausted-budget guards perform no read. Architectural
faults and backend errors leave CPU state unchanged; retry begins at the same
RIP/address. A callback STOP follows the committed load, and re-entry at
`A+3` does not read again. This is borrowed-memory fixture evidence, not
production owner/MMU integration. See the
[S3-20fj report](../reports/STAGE_3_S3-20fj_2026-11-02.md).

#### S3-20fk: exact address32 RAX-alias Dword store

The shared typed-IR Dword-store path admits only the three-byte singleton
`67 89 00` (`MOV [EAX],EAX`): address size Dword, transfer width Dword, and
RAX selected as both the memory base and store source. Both the effective
address and payload come from the original RAX: the address is zero-extended
low32 RAX and the payload is low32 RAX. One contiguous checked four-byte
write is performed; all GPRs and RFLAGS remain unchanged, and successful
retirement commits `A+3`.

The exact three source bytes and decoded site metadata remain bound. RAX-based
addr32 displacement or SIB forms, other bases, prefixes, opcodes, partial or
trailing encodings, address sizes, and transfer widths are not admitted by
this slice. Entry STOP, stop-address, and exhausted-budget guards perform no
write. Guest and backend failures before commit preserve architectural state
and RAM and can be retried at the same instruction. A callback STOP follows
the committed store without replay. EA `0xffff_fffd`, `0xffff_fffe`, and
`0xffff_ffff` each use one contiguous four-byte transfer across the 4-GiB
boundary; individual transfer bytes do not wrap. This is bounded
borrowed-memory fixture coverage, not production owner/MMU integration. See
the [S3-20fk report](../reports/STAGE_3_S3-20fk_2026-11-02.md).

#### S3-20db: exact address-size-32 Dword store with signed disp8

The shared typed-IR Dword-store path additionally admits only the exact
four-byte singleton `67 89 43 ib` (`MOV [EBX+disp8],EAX`). Effective-address
calculation follows the decoder-owned address-size-32 operand, including
wrapped 32-bit addition and zero-extension. The store writes low32 RAX in one
contiguous checked four-byte access. All 256 signed-disp8 source/site values
are bound exactly, with the instruction boundary at `A+4`. The associated
STOP, budget, fault, PC-map, and no-replay fixtures do not broaden admission
to other addr32 store forms. See the
[S3-20db report](../reports/STAGE_3_S3-20db_2026-11-02.md).

#### S3-20dc: exact address-size-32 Dword store with signed disp32

The shared typed-IR Dword-store path additionally admits only the exact
seven-byte singleton `67 89 83 id` (`MOV [EBX+disp32],EAX`). Effective-address
calculation follows the decoder-owned address-size-32 operand: signed disp32
addition wraps to 32 bits and the resulting EA is zero-extended. The store
writes low32 RAX through one contiguous checked four-byte access. Exact source
bytes bind to the instruction site and the PC map has the `A+7` guest boundary.
Recorded fixtures cover STOP, budget, fault, and no-replay behavior; other
addr32 dword-store forms remain outside this admission. See the
[S3-20dc report](../reports/STAGE_3_S3-20dc_2026-11-02.md).

#### S3-20dd: exact address64 SIB Dword load

The shared typed-IR Dword-load path additionally admits only the exact
three-byte singleton `8B 04 0B` (`MOV EAX,[RBX+RCX]`): ModRM selects EAX and
SIB selects scale 1, full-width RBX base, and full-width RCX index. Effective
address calculation follows the decoder-owned address64 operand. The checked
load reads four bytes and writes the result to EAX, zero-extending into RAX
and clearing `RAX[63:32]`; other registers and flags are preserved. Exact
source/site binding and one PC-map guest
boundary at `A+3` are retained. Recorded fixtures cover faults, no replay,
STOP, and budget handling. This entry is limited to the no-displacement
scale-1 singleton; distinct scale and displacement profiles are documented
separately in S3-20df, S3-20dh, and S3-20dj. See the
[S3-20dd report](../reports/STAGE_3_S3-20dd_2026-11-02.md).

#### S3-20df: exact address64 SIB Dword loads with RCX scales 2, 4, and 8

Alongside scale-1 `8B 04 0B` from S3-20dd, the shared typed-IR Dword-load
path additionally admits exactly `8B 04 4B`, `8B 04 8B`, and `8B 04 CB`
(`MOV EAX,[RBX+RCX*2|4|8]`). Each is a three-byte singleton with full-width
RBX base, full-width RCX index, and the SIB-selected scale. The effective
address follows decoder-owned `access_address_48`; the checked read is four
bytes and writes EAX, zero-extending into RAX and clearing `RAX[63:32]`.
Other registers and flags are preserved. Strict source/site binding and the
single PC-map boundary at `A+3` are retained; recorded fixtures cover faults
and guards. No separate scale-specific canonical-end/range-overflow fixture
was added, and the shared preflight/helper are unchanged; no runtime defect is
claimed. See the [S3-20df report](../reports/STAGE_3_S3-20df_2026-11-02.md).

#### S3-20dg: exact address64 SIB Dword stores with RCX scales 2, 4, and 8

The shared typed-IR Dword-store path additionally admits exactly `89 04 4B`,
`89 04 8B`, and `89 04 CB` (`MOV [RBX+RCX*2|4|8],EAX`). Each is a three-byte
singleton with full-width RBX base, full-width RCX index, and its encoded SIB
scale. Effective address calculation follows decoder-owned address48
`access_address_48`. The store writes low32 EAX through one contiguous checked
four-byte access; source registers and flags are preserved. Exact source/site
scale binding and the single PC-map guest boundary at `A+3` are retained;
recorded fixtures cover no replay, fault atomicity, STOP, and budget behavior.
A noncanonical crossing fixture asserts the decoded EA ends in `...fffe` and
the access raises `#GP`. The scale-1 `89 04 0B` form remains separately
covered by S3-20de; this does not claim separate 64-bit range-overflow tests or
a runtime defect. See the [S3-20dg report](../reports/STAGE_3_S3-20dg_2026-11-02.md).

#### S3-20dh: exact address64 SIB Dword loads with signed disp8

The shared typed-IR Dword-load path additionally admits exactly
`8B 44 0B ib`, `8B 44 4B ib`, `8B 44 8B ib`, and `8B 44 CB ib`
(`MOV EAX,[RBX+RCX*1/2/4/8+disp8]`). Each is a four-byte singleton: ModRM
selects EAX and SIB selects RBX base, RCX index, and scale 1/2/4/8. The signed
disp8 and full-width effective-address calculation follow decoder-owned
`access_address_48`. Each checked read is four bytes and writes EAX,
zero-extending into RAX and clearing `RAX[63:32]`. All four scales × all 256
disp8 values bind source bytes to their exact site; the PC map has the `A+4`
boundary. Recorded fixtures cover atomic faults, STOP, and budget behavior.
No separate canonical-end/range-overflow fixture was added for these forms;
the shared preflight is unchanged and no runtime defect is claimed. See the
[S3-20dh report](../reports/STAGE_3_S3-20dh_2026-11-02.md).

#### S3-20di: exact address64 SIB Dword stores with signed disp8

The shared typed-IR Dword-store path additionally admits exactly
`89 44 0B ib`, `89 44 4B ib`, `89 44 8B ib`, and `89 44 CB ib`
(`MOV [RBX+RCX*1/2/4/8+disp8],EAX`). Each is a four-byte singleton with
full-width RBX base, full-width RCX index, the encoded scale, and signed
disp8. Effective address calculation follows decoder-owned
`access_address_48`. One contiguous checked four-byte write commits low32
RAX; source registers and flags are preserved. All four scales × all 256 disp8 values bind the source bytes to their
exact site; the PC map has the `A+4` boundary. The host fixture distinguishes
a four-byte noncanonical-end crossing (`#GP`) from u64 range overflow
(`AddressOverflow { address, length: 4 }`); neither invokes the writer or
commits guest state. A valid adjacent write ending at the canonical boundary
is also covered. Recorded fixtures cover STOP and budget behavior. This
profile does not add other SIB/store forms. See the
[S3-20di report](../reports/STAGE_3_S3-20di_2026-11-02.md).

#### S3-20dj: exact address64 SIB Dword loads with signed disp32

The shared typed-IR Dword-load path additionally admits exactly
`8B 84 0B id`, `8B 84 4B id`, `8B 84 8B id`, and `8B 84 CB id`
(`MOV EAX,[RBX+RCX*1/2/4/8+disp32]`). Each is a seven-byte singleton with
full-width RBX base, full-width RCX index, the encoded scale, and signed
disp32. The decoder-owned `access_address_48` computes the effective address.
Each checked read is four bytes and writes EAX, zero-extending into RAX and
clearing `RAX[63:32]`. The PC map has the `A+7` boundary; the JIT fixture
checks source/site identity across all four scales and all four encoded
disp32 bytes. The host fixture covers canonical-end `#GP`, u64 range overflow,
and a valid adjacent read ending at the canonical boundary, without guest
state commit or a memory callback on either fault. See the
[S3-20dj report](../reports/STAGE_3_S3-20dj_2026-11-02.md).

#### S3-20dk: exact address64 SIB Dword stores with signed disp32

The shared typed-IR Dword-store path additionally admits exactly
`89 84 0B id`, `89 84 4B id`, `89 84 8B id`, and `89 84 CB id`
(`MOV [RBX+RCX*1/2/4/8+signed disp32],EAX`). Each is a seven-byte singleton
with full-width RBX base, full-width RCX index, encoded SIB scale, and signed
disp32. The decoder-owned `access_address_48` computes the effective address.
Each store performs one contiguous checked four-byte write of low32 RAX; the
guest boundary is `A+7`. Source/site binding covers all four scales and each
encoded displacement byte. The PC map uses the seven-byte boundary. Recorded
fixtures cover canonical-end `#GP`, u64 range overflow, fault atomicity, no
replay, STOP, and budget behavior. See the
[S3-20dk report](../reports/STAGE_3_S3-20dk_2026-11-02.md).

#### S3-20de: exact address64 SIB Dword store

The shared typed-IR Dword-store path additionally admits only the exact
three-byte singleton `89 04 0B` (`MOV [RBX+RCX],EAX`): ModRM selects EAX and
SIB selects scale 1, full-width RBX base, and full-width RCX index. The
effective address follows the decoder-owned address64 operand. The store
writes low32 RAX through one contiguous checked four-byte access; source
registers, other registers, and flags are preserved. Exact source/site binding
and one PC-map guest boundary at `A+3` are retained. Recorded fixtures cover
fault atomicity, no replay, STOP, and budget handling. This entry is limited
to the scale-1/no-displacement singleton; the distinct disp8 store is covered
by S3-20di. See the [S3-20de report](../reports/STAGE_3_S3-20de_2026-11-02.md).

#### S3-27i: disp32 Dword-store canonical-end and range-overflow checks

The host-side regression `s3_27i_disp32_dword_store_checks_canonical_end_and_range_overflow`
checks canonical-end and range-overflow behavior for the bounded S3-20cu
`89 83 id` store through the shared decoded-address path. It supplements the
original host fixture; it does not broaden instruction admission or establish
production owner/MMU integration or complete S3-27 fault recovery. See the
[S3-27i report](../reports/STAGE_3_S3-27i_2026-11-01.md).

#### S3-27j: published dword-store PC-map parity and lifetime

The host integration test
`published_dword_disp8_and_disp32_store_pc_maps_match_artifacts_survive_drop_and_republish`
checks map/artifact parity, map retention after artifact drop, and
republishing for the bounded disp8/disp32 dword-store artifacts. S3-34af pins
this identity as a harness gate. This is test-only publication/lifetime
evidence; it does not establish host-fault recovery or complete S3-27. See the
[S3-27j report](../reports/STAGE_3_S3-27j_2026-11-01.md).

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
profile. At that slice, the source buffer was fixed at eight bytes and carries an explicit
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

#### S3-20h / S3-26a: one checked qword load through shared typed IR

The opt-in profile accepts only `48 8B /r`, ModRM `mod=00`, data and base
registers RAX..RBX, without SIB or displacement. It follows
`BasicBlock → typed IR → allocator → runtime emitter`; it does not call the
direct V2 compiler as a hidden code-generation fallback. `CheckedLoad` carries
the decoder's qword `MemoryOperand`, defines a W64 success-only temporary, and
must be followed by `WriteGpr` of that temporary before the instruction commit.
The verifier rejects malformed descriptors, unavailable values, and a commit
that precedes publication of the load result.

The allocator records a helper boundary at the actual checked helper call and
emits its live-value saves/restores and spill homes. The runtime emitter calls
`memory_site_helper_v2` using site index 0. The qword remains staged until the
shared GPR/RIP/budget/retirement commit. The emitted PC map identifies the
helper-call and commit ranges. The compiler-issued IR artifact has a distinct
compile flavor, publication metadata, and V2 entry/context path. V1 block
execution and direct V2 load execution reject it before entering generated
code; the existing direct V2 compiler remains unchanged as a baseline.

Local native ARM64 differential coverage runs `[RAX] → RAX` through this path,
the direct V2 compiler, and the interpreter; it compares full state/RAM and
checks success retirement, architectural fault preservation, backend-error
classification/payload, STOP, stop-RIP, and zero-budget no-read behavior.
Verifier negatives cover malformed memory descriptors, undefined result
temporaries, missing writes, and early commit. This does not claim general
memory lowering, multiblock execution, MMIO, stores, atomics, or automatic host
fault recovery. Results are recorded in the
[S3-20h/S3-26a report](../reports/STAGE_3_S3-20h_2026-10-27.md).

#### S3-20i / S3-26b: one checked qword store through shared typed IR

The separate opt-in profile accepts exactly one `48 89 /r` instruction with
ModRM `mod=00`, source and base RAX..RBX, and no SIB or displacement.
`CheckedStore` records the canonical source GPR and decoder-provided qword
`MemoryOperand`; it does not invent a temporary that the emitter would ignore.
The verifier requires one store and its immediately following instruction
commit. It rejects duplicate stores, missing/early commit, or an intervening
effect. The artifact has a distinct compile flavor and typed publication and
execution path. V1, direct V2 load, shared-IR load, and direct V2 store entries
reject it before native entry.

The generated entry performs the existing STOP-RIP, STOP, and budget guards,
calls the checked helper once, then commits RIP, budget, and retirement before
checking STOP again. A successful write followed by STOP is retained and
reported as `StopRequested`. Architectural or backend write errors leave CPU
state and RAM unchanged and retire nothing. The typed host entry carries the
same unsafe private synchronous ordinary-RAM ownership preconditions as the
direct V2 store: no MMIO, DMA, concurrent/shared or executable/code backing,
and all-or-nothing writes.

PC-map ranges for the helper call and generated return-to-commit interval are
`InadmissibleAfterEffect`; an unknown host fault there cannot be recovered by
replaying the guest store. Only the planned helper error branch remains
replay-admissible because the helper guarantees no write on error. Local native
ARM64 tests compare IR store, direct V2 store, and interpreter on independent
CPU/RAM snapshots for `MOV [RAX], RAX`; they also cover the callback count,
successful retirement, STOP raised during write, architectural/backend errors,
pre-entry guards, verifier negatives, PC-map admissibility, and foreign-entry
rejection. This is one instruction over private synchronous ordinary data RAM.
It does not add MMIO, DMA, concurrency, atomics, executable aliases, active
SMC, multiblock ordering, or performance claims. See the
[S3-20i/S3-26b report](../reports/STAGE_3_S3-20i_2026-10-27.md).

#### S3-20j / S3-26c / S3-20k / S3-26d: exact two-site checked-memory entries

The opt-in profile accepts exactly two six-byte sequences: `48 89 03 48 8B 0B`
(`MOV [RBX], RAX; MOV RCX, [RBX]`) and `48 8B 03 48 89 02`
(`MOV RAX, [RBX]; MOV [RDX], RAX`). Builder, typed IR, allocator, shared
emitter, and host publication produce one entry with two ordered memory sites.
The typed metadata records the selected sequence and validates each real
operation, operand and instruction boundary. The existing V2 single-site APIs
retain their exact index-zero checks and reject this artifact family.

The entry accepts canonical RIP at the first or second instruction. Store→load
commits the first store before the load; load→store commits loaded RAX before
the store. A second-site fault may occur on continuation before retiring in
that invocation; it does not repeat the first site. STOP and budget are checked
at the second boundary in stop-RIP → STOP → budget order. The host result
decoder validates helper site, fault payload, RIP, budget, and retirement for
both sequences. A store commit/helper PC range remains non-replayable.

This contract applies to one vCPU and caller-guaranteed private synchronous
ordinary RAM with all-or-nothing qword writes. It excludes branches, executable
aliases, MMIO, atomic operations, concurrent access, and arbitrary multi-site
blocks. No performance claim is made. Evidence is in the
[two-sequence report](../reports/STAGE_3_S3-20j_2026-10-27.md).

#### S3-26e: exact two-load pair

One additional exact two-site sequence is admitted: `48 8B 03 48 8B 0B`
(`MOV RAX, [RBX]; MOV RCX, [RBX]`). Each site performs a separate checked read
from the same effective address; a callback or memory read may not be merged
because the addresses compare equal. Each instruction has its own helper site,
GPR commit, RIP boundary and retirement. A second-site fault/backend error
preserves the first load and resumes at `A+3`; the ordinary singleton load may
retry that second instruction without replaying the first read. STOP and budget
guards retain their existing second-boundary order. An event raised by the
first callback is sampled only after the bounded pair returns, unless an
earlier STOP/fault/backend outcome wins. This is a private synchronous ordinary
RAM fixture; the callback trace is not an MMIO/device model. Other load-load or
multi-site sequences remain unsupported. Full S3-26/S3-27 and physical x86
acceptance remain open. See the [S3-26e report](../reports/STAGE_3_S3-26e_2026-10-30.md).

#### S3-26f: exact store→store pair

The additional accepted source is exactly `48 89 03 48 89 02`
(`MOV [RBX],RAX; MOV [RDX],RAX`). Site 0 binds instruction index 0, RIP `A`,
base RBX and source RAX; site 1 binds index 1, RIP `A+3`, base RDX and the
same source. Lowering retains two `CheckedStore` operations and separate
instruction commits. The emitter uses helper operation index 4 for the second
store. The sites remain separate callbacks even when RBX and RDX resolve to
the same address. A successful first write commits at `A+3`; the second
commits at `A+6`. A second-site architectural fault preserves the first write,
leaves the second unchanged, and exposes state at `A+3`; the pair is not
transactional. Each write requires private synchronous ordinary RAM whose
individual eight-byte write is all-or-nothing. Cached pair publication
explicitly rejects this variant. Other source, operand, metadata and pair
shapes remain unsupported. Owner/MMU integration and full S3-26/S3-27
acceptance remain open. See the
[S3-26f report](../reports/STAGE_3_S3-26f_2026-11-03.md).

#### S3-20l: RIP-relative qword MOV through shared IR and bounded dispatch

The admitted singleton is exactly seven bytes: `48 8B /r disp32` or
`48 89 /r disp32`, ModRM `mod=00,r/m=101`, qword data register RAX..RBX. No
SIB, address override, extra prefix, trailing byte, or multi-instruction
extension is admitted. Decode supplies the `MemoryOperand`; execution uses
`access_address_48` with that instruction's `next_rip`. IR verification,
typed host publication/execution, and bounded dispatcher compile admission
must preserve this same profile. Existing direct-base and two-pair profiles
retain their prior checks.

The bounded dispatcher treats an explicit fixture byte-window end separately
from guest termination: a 1..6-byte truncated RIP-relative source returns
`BoundaryReached` without publishing or touching data memory. It may continue
from any actual RIP produced by a completed singleton or NOP interpreter step.
Native ARM64 tests cover negative/zero/positive displacement, interior
CodeView entry, parity with direct V2 and the interpreter, exact architectural
fault versus host-error behavior, and STOP after a committed IR store. The
dispatcher tests both RIP-relative singleton → bare NOP → existing pair and
bare NOP → RIP-relative singleton → pair, verifying `next_rip` from the latter
entry. This remains a fixture profile and does not provide `od-machine`
ownership, MMU translation, or a performance result. See the
[S3-20l report](../reports/STAGE_3_S3-20l_2026-10-27.md).

#### S3-20m: direct-base signed disp8/disp32 in shared IR and dispatch

The checked singleton profile adds only `48 8B/89 /r disp8` with ModRM
`mod=01`, exact four-byte source, or `48 8B/89 /r disp32` with ModRM `mod=10`,
exact seven-byte source. The data/destination/source register and direct base
register each must be RAX..RBX. SIB (`r/m=100`), RIP-relative, `67`, any extra
prefix, register form and trailing bytes remain outside this profile. The
decoder's `MemoryOperand` carries the signed displacement unchanged; the host
uses the existing `access_address_48` and checked helper, without duplicating
EA arithmetic or address validation.

Builder raw-byte checks bind ModRM mode/length to the decoded operand range;
IR descriptor, compiler-issued source/site metadata, host validation and
`memory_dispatch::compile_at` enforce the same singleton shape. Native tests
compare direct V2/shared IR/interpreter over disp8 and disp32 endpoints,
including destination/base and source/base aliasing, canonical start/end
faults, backend errors, wraparound, STOP after successful store and truncated
windows. The bounded dispatcher also composes a disp8 singleton → bare NOP
interpreter fallback → existing pair. See the
[S3-20m report](../reports/STAGE_3_S3-20m_2026-10-27.md).

#### S3-20n: no-displacement SIB in shared IR and bounded dispatch

The shared checked singleton profile additionally accepts exactly four-byte
`48 8B/89 /r sib`: ModRM `mod=00,r/m=100`; data, base and index encodings
RAX..RBX; qword operand and address sizes; and scale 1/2/4/8. SIB index
`100` (no index), base `101` (no base), RSP/RBP/R8+, displacement forms,
address override, other prefixes, and trailing bytes remain rejected. This is
a separate raw-byte admission from direct-base disp8 (`mod=01`, also four
bytes); the existing `MemorySiteV2` validator binds bytes to decoded base,
index and scale, while the IR descriptor requires a four-byte instruction.

The decoder-provided `MemoryOperand` and `access_address_48` remain the only
effective-address source. No SIB address math or emitter was added. Native
direct V2/shared IR/interpreter tests cover both directions across all data,
base, index and scale encodings, including aliases and a zero-valued index.
SIB cases check wrapping multiply/add, noncanonical start/end faults with no
state or RAM commit, backend load/store errors as host errors, and exact
architectural store-fault payloads without CPU/RAM/budget/retirement commit.
STOP after a successful store preserves its committed bytes and one retirement.
Bounded dispatch composes the
SIB singleton with bare-NOP interpreter fallback and the existing pair across
budgets 0..4. STOP after the SIB load returns at its actual RIP before NOP;
clearing STOP resumes without repeating the access. Truncated source windows before the SIB byte return
`BoundaryReached` without effects. Pair admissions and the unsafe private
synchronous RAM contract are unchanged. See the
[S3-20n report](../reports/STAGE_3_S3-20n_2026-10-27.md).

#### S3-20o: address32 direct-base qword MOV in shared IR and dispatch

The shared checked singleton additionally accepts exactly four-byte
`67 48 8B/89 /r`, ModRM `mod=00`, with data and base encodings RAX..RBX.
Descriptors preserve qword operand width and dword address width as separate
decoded fields. This is an exact admission branch distinct from the 64-bit
address SIB and disp8 forms that also have four-byte encodings. SIB,
displacement, extended registers, reordered or additional prefixes, and tail
bytes remain rejected. The decoder's `MemoryOperand::access_address_48` is
still the only address computation and canonical/range checker; runtime does
not add an address mask or split the qword access.

Native direct V2/shared IR/interpreter parity covers both operations and all
data/base register pairs, including load destination/store source equal to the
base. It checks noncanonical u64 GPR values are interpreted using the decoded
dword address size, while a qword starting at `0xffff_fffc` accesses bytes
through `0x1_0000_0003` in one callback. Architectural load/store faults
preserve full CPU/RAM/budget/retirement with exact payload; backend store
failure remains a host error; budget, stop-RIP and STOP guards return before
access; successful store+STOP retains its commit. The bounded dispatcher
composes this singleton → bare NOP fallback → existing pair across budgets
0..4 and resumes from actual RIP after STOP. Partial prefix/opcode/ModRM
windows do not retire or access guest data. Pair profiles are unchanged. See
the [S3-20o report](../reports/STAGE_3_S3-20o_2026-10-27.md).

#### S3-20p: address32 signed-displacement qword MOV in shared IR and dispatch

The shared checked singleton additionally admits only
`67 48 8B/89 /r disp8|disp32`: ModRM `mod=01/10`, data/base RAX..RBX, dword
address size, qword operand size, and exact lengths five/eight. Its typed
descriptor retains the decoded signed displacement. Exact source validation
binds opcode, prefix order, ModRM mode/registers, and length to that decoded
site; it does not derive displacement semantics from bytes. The decoder's
`MemoryOperand::access_address_48` remains the only effective-address,
wrapping, canonicality, and complete qword-range implementation. No SIB,
RIP-relative/base-less form, extra prefix, extended register, arbitrary block,
or pair-profile change is admitted.

Direct V2/shared IR/interpreter parity covers disp8 -128/-1/0/1/127 and disp32
MIN/MIN+1/-1/0/1/MAX-1/MAX for both MOV directions, base aliases, high base
bits, modulo-2^32 signed addition, and the contiguous qword callback across
`0xffff_ffff`. Shared IR fault payloads preserve CPU/RAM/budget/retirement;
backend errors remain host errors; store+STOP preserves the successful write.
Bounded dispatch composes the disp8 singleton → bare NOP fallback → existing
pair and tests partial disp8 and disp32 windows. Pair admissions are unchanged.
See the [S3-20p report](../reports/STAGE_3_S3-20p_2026-10-27.md).

#### S3-20q: 64-bit SIB signed-displacement qword MOV in shared IR and dispatch

The shared checked singleton additionally admits exactly
`48 8B/89 /r sib disp8|disp32`: ModRM `mod=01/10,r/m=100`, RAX..RBX
data/base/index encodings, scale 1/2/4/8, qword data and 64-bit address size,
with exact lengths five/eight. SIB index 4 (no index), the mod00 base-5
no-base special form, RBP/RSP and extended registers, `67`, and any extra
prefix remain unsupported. In the admitted mod01/mod10 forms, base 5 means
RBP and is rejected by the RAX..RBX limit. Exact
source validation checks prefix/opcode, ModRM mode and SIB selection, register
fields, and length against the decoded `MemorySiteV2`; signed displacement
meaning remains decoder-owned. The existing `MemoryOperand::access_address_48`
is still the sole EA and qword-range implementation. The descriptor is
distinct from same-length address32 direct displacement and direct-base
64-bit forms. Pair profiles are unchanged.

Native direct V2/shared IR/interpreter parity covers both operations across
all scales and disp8 -128/-1/0/127 and disp32 MIN/-1/0/MAX, including data/base,
data/index, and base/index aliases. Callback logs confirm one access at the
decoded target with length eight. Exact architectural fault payloads preserve
CPU/RAM/budget/retirement; backend failures stay host errors; successful
store+STOP retains its write. Bounded dispatch composes SIB disp8 → bare NOP
fallback → existing pair across budgets 0..4, pauses before NOP on STOP and
resumes from actual RIP, rejects invalid SIB fields without helper access,
and classifies partial disp8/disp32 windows as `BoundaryReached`. See the
[S3-20q report](../reports/STAGE_3_S3-20q_2026-10-27.md).

#### S3-20r: address32 no-displacement SIB qword MOV in shared IR and dispatch

The exact additional singleton is `67 48 8B/89 /r sib`, with ModRM
`mod=00,r/m=100`, RAX..RBX data/base/index encodings, scale 1/2/4/8, qword
data, dword address size, and five source bytes. `MemorySiteV2` validates the
decoded base, index, scale, zero displacement and source bytes together.
Address32 direct disp8 has the same total length but uses ModRM `mod=01` and a
direct base; the validator keeps those shapes separate. Ordinary 64-bit SIB
uses no `67` prefix. Address32 SIB displacement, including a 9-byte disp32
form beyond the site's eight-byte source storage, is unsupported. No-index
and no-base specials, RSP/RBP and extended fields, extra prefixes and trailing
bytes are rejected. The decoder's `MemoryOperand::access_address_48` remains
the sole source for 32-bit effective-address truncation, scale/add wrapping,
canonicality and contiguous qword range checks. Existing pair profiles are
unchanged.

Native direct V2/shared IR/interpreter cases cover both operations and all
data/base/index/scale combinations, aliases, high register halves, wraparound,
and a contiguous qword access beginning at `0xffff_fffc`. Fault, backend error
and successful store+STOP checks preserve the established commit contract.
Bounded dispatch composes this singleton → bare NOP interpreter fallback →
existing pair across budgets 0..4, with actual-RIP continuation and no repeated
access; partial byte windows stop without publishing or calling data memory.
See the [S3-20r report](../reports/STAGE_3_S3-20r_2026-10-27.md).

#### S3-20s: address32 SIB disp8 qword MOV in shared IR and dispatch

The checked singleton additionally admits `67 48 8B/89 /r sib disp8` with
ModRM `mod=01,r/m=100`, RAX..RBX data/base/index encodings, scale 1/2/4/8,
qword data, dword address size and exact six-byte source. It remains distinct
from the five-byte `mod=00` addr32 SIB form, the same-length addr32 direct
disp8 form, and 64-bit SIB forms. The decoder supplies the signed displacement
and `MemoryOperand::access_address_48` supplies all address/truncation,
wrapping, canonicality and qword-range semantics. The fixed site source
storage remained eight bytes, so addr32 SIB disp32 was not yet admitted. S3-20t
extends singleton source storage for its exact nine-byte form. Extra prefixes,
no-index/no-base specials, unsupported registers,
and trailing bytes are rejected. Existing pair profiles are unchanged; the
six-byte singleton is selected by exact bytes and metadata, not length alone.

Native direct V2/shared IR/interpreter parity covers both MOV operations at
disp8 `-128/-1/0/127`, all scales, aliases, high base/index halves, wrapping,
and qword access crossing 4 GiB. Faults preserve exact payload and do not
commit CPU, RAM, budget or retirement; backend errors remain host errors, and
successful store+STOP keeps the write. Bounded dispatch composes this
singleton → bare NOP fallback → existing pair across budgets 0..4, including
six-byte standalone admission and actual-RIP continuation without replay.
See the [S3-20s report](../reports/STAGE_3_S3-20s_2026-10-27.md).

#### S3-20t: address32 SIB disp32 qword MOV in shared IR and dispatch

The checked singleton admits exactly `67 48 8B/89 /r sib disp32`, ModRM
`mod=10,r/m=100`, RAX..RBX data/base/index encodings, scale 1/2/4/8, qword
data, dword address size and nine bytes. The named singleton source bound is
`MEMORY_SITE_SOURCE_BYTES_MAX = 9`; `source_bytes_storage()` now returns
`([u8; 9], u8)` instead of `([u8; 8], u8)`. Callers validate decoded
instruction length before slicing/copying and preserve the ninth byte in
compiler, IR, host publication, and dispatcher metadata. This bound does not
change qword data buffers, pair source `[u8; 6]`, ActiveCodeV3 source
`[u8; 23]`, or RuntimeContext/C ABI layout. `MemoryOperand` and
`access_address_48` remain the sole displacement and effective-address source.

The descriptor matrix covers both MOV directions, all allowed GPR encodings
and scales, and disp32 MIN/-1/0/MAX. Native direct V2/shared IR/interpreter
parity checks callback address/count and complete CPU/RAM state, including a
qword callback crossing 4 GiB and two inputs differing only in byte nine.
Architectural faults preserve payload and no-commit accounting; backend errors
stay host errors; successful store+STOP retains its write. Bounded dispatch
composes the nine-byte singleton → bare NOP interpreter bridge → existing
pair, with budget, stop-RIP, callback STOP, and continuation coverage. Partial
windows 1..8 and malformed profiles reject before data callbacks. See the
[S3-20t report](../reports/STAGE_3_S3-20t_2026-10-27.md).

#### S3-20u: RBP-base signed disp8 qword MOV in shared IR and dispatch

The checked singleton admits exactly `48 8B/89 /r disp8`, ModRM
`mod=01,r/m=101`, RBP base, data register RAX..RBX, qword operand and exact
length four. The decoder's `MemoryOperand` and `access_address_48` remain the
only displacement, effective-address and qword canonical-range semantics.
No SIB, RSP, address-size override, disp32, segment/extra prefix, extended
register or pair form is admitted.

Descriptor and direct V2/shared IR/interpreter evidence covers both operations
at disp8 -128/-1/0/127, callback address/count, full state/RAM and unchanged
RBP/RSP. Noncanonical RBP start and last-byte ranges produce #SS
(vector 12/error 0/no linear address); equivalent RBX checks produce #GP
(vector 13/error 0/no linear address), before a memory callback or commit.
AddressOverflow remains the explicit host diagnostic; ordinary backend errors
remain host errors. Entry STOP and zero budget do not access memory, and a
successful store+STOP remains committed. Bounded dispatch composes the
singleton → bare NOP interpreter bridge → existing pair across budgets 0..4,
and resumes from the actual RIP without replay. See the
[S3-20u report](../reports/STAGE_3_S3-20u_2026-10-27.md).

#### S3-20v: RSP-base no-index SIB qword MOV in shared IR and dispatch

Admit exactly `48 8B/89 /r sib`, ModRM `mod=00,r/m=100`, SIB byte `0x24`,
RSP base, no index, scale 1, no displacement, data register RAX..RBX, qword
data, 64-bit effective address and four-byte instruction length. No SIB
variation, displacement, RSP data operand, address-size or extra prefix,
extended register or pair change is admitted. Decoder `MemoryOperand` and
`access_address_48` remain the sole effective-address and range semantics; host
SP/frame ABI is not involved.

Direct V2/shared IR/interpreter evidence covers both operations, complete
state/RAM parity, callback address/count and retirement with RSP/RBP/flags
unchanged. Noncanonical RSP start and end-of-qword checks produce #SS
(vector 12/error 0/no linear address); the same RBX checks produce #GP
(vector 13/error 0/no linear address), with no backend callback or commit.
AddressOverflow remains its host diagnostic and ordinary backend failures stay
host errors. Entry STOP and zero budget cause no access; successful store+STOP
remains committed. Bounded dispatch composes the exact singleton → bare NOP
interpreter bridge → existing pair across budgets 0..4 and continuation from
the actual RIP. Partial byte windows return BoundaryReached; malformed SIB,
displacement, prefix and data-register variants return Unsupported without
execution. See the
[S3-20v report](../reports/STAGE_3_S3-20v_2026-10-27.md).

#### S3-20w: RSP-base no-index SIB qword MOV with disp8

Admit exactly the singleton forms `48 8B/89 /r sib disp8`, ModRM
`mod=01,r/m=100`, SIB `0x24`: RSP base, no index, scale 1, signed disp8,
data RAX..RBX, qword and 64-bit effective address, exact length five. The
decoder's `MemoryOperand` and `access_address_48` remain the sole addressing
semantics. The prior no-displacement `[RSP]` form remains separate. Disp32,
index/scale variation, `67`, segment/extra prefixes, extended registers and
pair changes are unsupported.

Direct V2/shared IR/interpreter parity covers load/store and disp8 -128/-1/0/127,
full state/RAM, callback address/count and retirement, with RSP/RBP/flags
unchanged. Architectural faults retain RSP #SS versus RBX #GP payloads and no
commit; AddressOverflow and backend failures remain host errors. Entry guards
make no access, while successful store+STOP remains committed. Bounded dispatch
composes the five-byte singleton → bare NOP bridge → existing pair with exact
budget accounting, STOP-RIP and actual-RIP continuation. Partial windows stop
as `BoundaryReached`; unsupported variants have no effect. See the
[S3-20w report](../reports/STAGE_3_S3-20w_2026-10-27.md).

#### S3-20x: RSP-base no-index SIB qword MOV with disp32

Admit exactly singleton `48 8B/89 /r sib disp32`, ModRM `mod=10,r/m=100`,
SIB `0x24`: RSP base, no index, scale 1, signed disp32, data RAX..RBX, qword,
64-bit effective address and exact length eight. The displacement bytes are
bytes 4..7 after the SIB. Decoder `MemoryOperand` and `access_address_48` are
the only signed displacement and address semantics. The existing source
metadata capacity is sufficient. Address-size override, other prefixes,
indexed/no-base SIB forms, pair/V3 and data-buffer changes are excluded; the
separate `[RSP]` and RSP+disp8 profiles remain unchanged.

Direct V2/shared IR/interpreter parity covers load/store, disp32
MIN/-1/0/MAX, full state/RAM, callback address/count and retirement. A qword
callback crossing `0xffff_ffff`, positive u64 wrap to address zero and sources
that differ only in the high displacement byte are checked at runtime. RSP
faults retain #SS while RBX controls retain #GP; faults do not commit, and
AddressOverflow remains distinct from backend errors. Entry STOP/budget zero
make no callback, successful store+STOP remains committed. Bounded dispatch
composes the eight-byte singleton → bare NOP → existing pair with budgets 0..4
and actual-RIP continuation; partial windows return `BoundaryReached` and
invalid forms have no effects. See the
[S3-20x report](../reports/STAGE_3_S3-20x_2026-10-27.md).

#### S3-20y: RBP-base signed disp32 qword MOV

Admit only singleton `48 8B/89 /r disp32`, ModRM `mod=10,r/m=101`, RBP base,
data RAX..RBX, qword data, 64-bit EA and exact length seven. The decoder's
`MemoryOperand` and `access_address_48` own displacement and effective-address
semantics. The raw-byte gate distinguishes this from the existing
RIP-relative `mod=00,r/m=101` form, whose `NextRip` base and `next_rip` remain
unchanged. SIB, 67, other prefixes/bases, pairs and V3 are excluded.

Direct V2/shared IR/interpreter coverage includes both operations, disp32
MIN/-1/0/127/128/MAX, exact callback address/count, state/RAM parity and
retirement. Equal displacement under RBP versus RIP-relative encoding must
produce addresses based on RBP versus that instruction's `next_rip`. RBP
noncanonical start/end is #SS, matching RBX control is #GP; these and backend
failures do not commit architectural state. AddressOverflow remains a host
diagnostic. Entry STOP/budget-zero make no callback; successful store+STOP
retains its commit. Dispatcher composes the singleton, bare-NOP interpreter
bridge and existing pair; budgets 0..4 and continuation from actual RIP do not
repeat effects, and source windows 1..6 stop without effects. See the
[S3-20y report](../reports/STAGE_3_S3-20y_2026-10-27.md).

S3-34a adds a bounded host-side differential corpus for these two exact
sequences. It varies full initial GPR/flags/RAM snapshots, qword values,
alignment, same/different ranges, and safe partial overlaps for load→store;
store→load retains its artifact-defined same-range accesses. Each generated
input runs twice through the native entry and interpreter on independent
snapshots. Comparison includes complete `LongModeState`, RAM bytes, retirement,
budget, exit, and data callback order/count. Fixed seeds, uniqueness checks,
JSON mismatch capture, and comparator corruption checks are local ARM64 test
evidence only. Named fault, STOP, budget, and continuation cases remain outside
the generated-success count. This does not widen either two-site artifact
shape, establish physical Mac mini equivalence, or imply performance; S3-21
remains `not_run` for this slice. See the
[S3-34a report](../reports/STAGE_3_S3-34a_2026-10-27.md).

## 3a. Единые правила допуска регистрового набора

Этот раздел задаёт **действующее** множество для регистровых admission-профилей S0-09, S3-32a и S3-33a–m одним описанием. Он не добавляет инструкции или кодировки и не делает все строки доступными каждому существующему API: профиль выбирается явно по таблице ниже. Ранее описанные ограниченные срезы и их условия исполнения сохраняются. Здесь определяется только допуск; наличие правила само по себе не доказывает готовность JIT или этапа 3.

### Вход и привязка свидетельства

Вход — неизменяемый `CodeView { base_rip, code_end, bytes }`: допустимый диапазон байтов — `[base_rip, code_end)`, а `code_end` указывает на адрес сразу после последнего байта и вычисляется проверенно при создании view. Для каждой инструкции декодер `od_isa::long_decode` получает **эти же** байты на гостевом `rip` и возвращает `Instruction`: `operation` с классом и операндами, `opcode.map`/`opcode.byte`, `operand_size`, `address_size`, поля префиксов (`group1`, `segment`, `operand_override`, `address_override`, конкретный `rex`) и точную `length`. Свидетельство фиксирует байты, `base_rip`, `code_end`, начала всех декодированных инструкций и исключительный `code_end`; оно применимо только к совпадающему view. Для каждой инструкции проверяется `[rip, next_rip)`, где `next_rip = rip + length` без переполнения; цели допустимых переходов проверяются после полного сканирования view, не только построенного впоследствии блока.

Обозначения в таблицах: **P48** — `group1 == None`, `segment == None`, `operand_override == false`, `address_override == false`, `rex == Some(0x48)`, `opcode.map == Primary`, `operand_size == Qword`, `address_size == Qword`. Это условия на поля результата декодирования, включая конкретный последний REX, а не разрешение пропускать приведённые ниже byte guards. **P0** — `prefixes == Default::default()` и `opcode.map == Primary`. `R0`…`R3` — qword `RAX`, `RCX`, `RDX`, `RBX`; `ib` — один байт, `id` — четыре байта little-endian. Указанная длина точная. В каждой строке с byte guard все перечисленные байты и их связь с декодированным операндом обязательны для существующего профиля.

### Положительные правила форм

| Правило | Декодированная операция, поля и длина | Дополнительная сверка исходных байтов |
|---|---|---|
| `B-MOV` | P48; `MovImmediate`, destination — qword `R0`…`R3`; opcode `B8`…`BB`; imm64; длина 10 | Отдельной сверки формы нет. |
| `B-ALU` | P48; `Arithmetic` с двумя qword register operands `R0`…`R3`: opcode `03` только для `Add`, `2B` только для `Sub`, `3B` только для `Cmp`; длина 3 | Отдельной сверки формы нет. |
| `B-JCC` | P0; `Branch`: opcode `74` с `Equal` или `75` с `NotEqual`; `operand_size` и `address_size` — Qword; signed rel8; длина 2 | Отдельной сверки формы нет; цель проверяется по границам ниже. |
| `NOP` | P0; `Operation::Nop`, opcode `90`, длина 1. Helper не вводит отдельной проверки `operand_size` или `address_size`: текущий decoder для `90` возвращает Dword/Qword. | Первый байт равен `90`. |
| `INC` | P48; `Arithmetic::Inc`, dst qword RAX, src `Immediate(1)`; opcode `FF`; длина 3 | `48 FF C0`. |
| `DEC` | P48; `Arithmetic::Dec`, dst qword RAX, src `Immediate(1)`; opcode `FF`; длина 3 | `48 FF C8`. |
| `NEG` | P48; `Arithmetic::Neg`, dst qword RAX, src `Immediate(0)`; opcode `F7`; длина 3 | `48 F7 D8`. |
| `NOT` | P48; `Logic::Not`, dst qword RAX, src `Immediate(0)`; opcode `F7`; длина 3 | `48 F7 D0`. |
| `LEA-RBX` | P48; `Lea`, dst qword RAX, src: base RBX, нет index, scale 1, displacement 0, src size/address size Qword; opcode `8D`; длина 3 | `48 8D 03`. |
| `LEA-D8` | P48; `Lea`, dst qword RAX, src: base RBX, нет index, scale 1, displacement равен `i64(ib as i8)`, src size/address size Qword; opcode `8D`; длина 4 | `48 8D 43 ib`; byte guard обязателен для различения disp8 от других кодировок с теми же decoded-полями и длиной. |
| `LEA-SIB` | P48; `Lea`, dst qword RAX, src: base RBX, index RCX, scale 1, displacement 0, src size/address size Qword; opcode `8D`; длина 4 | `48 8D 04 0B`. |
| `LEA-RIP` | P48; `Lea`, dst qword RAX, src: base `NextRip`, нет index, scale 1, displacement равен sign-extended `id` как `i32` в `i64`, src size/address size Qword; opcode `8D`; длина 7 | `48 8D 05 id`. |
| `XCHG` | P48; `Xchg` с decoded left qword RBX и right qword RAX; opcode `87`; длина 3 | `48 87 D8`. |
| `ADD-I8` | P48; `Arithmetic::Add`, dst qword RAX, src `Immediate(value)`; opcode `83`; длина 4 | `48 83 C0 ib`; `value` равен sign-extension `ib` из `i8` до `u64`. |
| `CMP-I8` | P48; `Arithmetic::Cmp`, dst qword RAX, src `Immediate(value)`; opcode `83`; длина 4 | `48 83 F8 ib`; `value` равен sign-extension `ib` из `i8` до `u64`. |
| `SUB-I8` | P48; `Arithmetic::Sub`, dst qword RAX, src `Immediate(value)`; opcode `83`; длина 4 | `48 83 E8 ib`; `value` равен sign-extension `ib` из `i8` до `u64`. |
| `MOV-I32` | P48; `MovImmediate`, dst qword RAX; opcode `C7`; длина 7 | `48 C7 C0 id`; decoded value равен sign-extension little-endian `id` из `i32` до `u64`. |
| `ADD-RAX` | P48; `Arithmetic::Add`, dst и src — qword RAX; opcode `03`; длина 3 | `48 03 C0`. Форма уже входит в `B-ALU`; это дополнительное точное свидетельство S3-32a. |

Сохранение всех указанных byte guards требуется для эквивалентности существующим helper-предикатам, в том числе если вызывающий код передаёт несогласованные `Instruction` и байты. Для пары, полученной декодированием тех же байтов, S3-50 установил различающую необходимость `LEA-D8`: `48 8D 43 00`, `48 8D 04 23` и `40 48 8D 03` дают одинаковые decoded-поля и длину, но только первая форма допускается как disp8. Для остальных перечисленных exact helpers и bare NOP анализ S3-50 установил избыточность guard для такой связанной пары; этот вывод не разрешает удалять действующие проверки. Основания и граница вывода приведены в [инвентаре S3-50](../reports/STAGE_3_CORE_REPORT.md).

### Явный выбор профиля

`Base = {B-MOV, B-ALU, B-JCC, NOP}`. Для каждого из 13 отдельных `S0_09Nop<Form>Admission` допустим `Base` и **только** одна указанная строка:

| Admission / профиль сборки | Дополнительное правило |
|---|---|
| `S0_09NopIncRaxAdmission` / `S0_09NativeIncRax` | `INC` |
| `S0_09NopDecRaxAdmission` / `S0_09NativeDecRax` | `DEC` |
| `S0_09NopNegRaxAdmission` / `S0_09NativeNegRax` | `NEG` |
| `S0_09NopNotRaxAdmission` / `S0_09NativeNotRax` | `NOT` |
| `S0_09NopLeaRaxRbxAdmission` / `S0_09NativeLeaRaxRbx` | `LEA-RBX` |
| `S0_09NopLeaRaxRbxDisp8Admission` / `S0_09NativeLeaRaxRbxDisp8` | `LEA-D8` |
| `S0_09NopLeaRaxRbxRcxAdmission` / `S0_09NativeLeaRaxRbxRcx` | `LEA-SIB` |
| `S0_09NopLeaRaxRipDisp32Admission` / `S0_09NativeLeaRaxRipDisp32` | `LEA-RIP` |
| `S0_09NopXchgRaxRbxAdmission` / `S0_09NativeXchgRaxRbx` | `XCHG` |
| `S0_09NopAddRaxImm8Admission` / `S0_09NativeAddRaxImm8` | `ADD-I8` |
| `S0_09NopCmpRaxImm8Admission` / `S0_09NativeCmpRaxImm8` | `CMP-I8` |
| `S0_09NopSubRaxImm8Admission` / `S0_09NativeSubRaxImm8` | `SUB-I8` |
| `S0_09NopMovRaxImm32Admission` / `S0_09NativeMovRaxImm32` | `MOV-I32` |

`S0_09NopAdmission` использует только `Base`; `NativeRegisterV1Admission` использует `Base` и объединение всех 13 дополнительных правил. В таблице каждому расширенному свидетельству сопоставлен **native opt-in** `BlockBuildProfile`; выбор свидетельства сам по себе не выбирает builder. Например, legacy `run_with_nop_inc_rax_fallback` использует `Base + INC` admission, но компилирует через обычный `S0_09` builder и при передаче управления fallback исполняет допущенный `INC` интерпретатором. Только `run_with_native_inc_rax` выбирает `S0_09NativeIncRax` builder для native INC. Ни один из этих builder-профилей не превращает `NOP` в native-форму: builder допускает `B-MOV`, `B-ALU`, `B-JCC` и дополнительные строки выбранного native-профиля, а на bare NOP останавливается с `UnsupportedDecoded`; при продолжении исполнения допущенный NOP передаётся interpreter fallback с действующими проверками STOP и budget. `S3_32aAddRaxRaxAdmission` сначала получает полное `S0_09NopAdmission`-свидетельство и затем требует хотя бы один `ADD-RAX` на декодированной границе; при отсутствии такой формы возвращается `UnsupportedInstruction { rip: code.base_rip() }`. Этот тест наличия не разрешает инструкции вне `Base`.

### Отказы, границы и лимиты

Для выбранного профиля применяется отказ по умолчанию. Полный immutable view сканируется по порядку RIP: декодирование, проверка длины, диапазона и правила формы; затем по собранным инструкциям проверяются цели переходов. Ошибки admission с указанными payload и порядком обнаружения сохраняются:

| `S0_09NopAdmissionError` | Условие |
|---|---|
| `Decode { rip, error }` | Декодер не возвращает инструкцию; исходный `DecodeError` сохраняется. |
| `InvalidLength { rip, length }` | Декодированная длина вне `1..=15`. |
| `RangeOverflow { rip, length }` | Невозможно преобразовать offset (тогда `length: 0`), `rip + length` переполняется либо `next_rip > code_end`. |
| `UnsupportedInstruction { rip }` | Успешно декодированная форма не соответствует выбранному профилю; для S3-32a также отсутствие обязательного `ADD-RAX` после base-проверки. |
| `BranchTargetNotBoundary { rip, target }` | После полного scan цель `next_rip.wrapping_add_signed(rel8)` не совпадает с началом декодированной инструкции или `code_end`. |

Сегментные префиксы, `LOCK`, `REP` и `0x66`/`0x67` вне положительных правил, любая не перечисленная комбинация операции, операндов, размера, REX, длины или байтов отвергаются. Регистровый набор не допускает data-memory `MOV` и не содержит data-memory sites: один такой site, а значит и несколько, вне данного допуска. `LEA` вычисляет адрес и не считается data-memory site. Существующий exact fallback `NOT RCX` относится к отдельному memory-dispatch пути; ни `NativeRegisterV1`, ни этот раздел не добавляют ему native register admission. Memory-формы `MOV`, `Checked*` и ограниченные двухсайтовые memory-профили остаются в своих разделах и отдельном механизме; данный отказ не меняет их правила.

`BlockLimits { instructions, bytes }` применяется отдельно к построению одного блока, а не к полному admission view. Каждый предел должен быть положительным: `instructions <= 32`, `bytes <= 256`; иначе builder возвращает `BuildError::InvalidLimits { limits }`. Достижение допустимого предела завершает корректный префикс как `BlockStopReason::InstructionLimit` или `ByteLimit` (в том числе когда байтовый предел прерывает fetch инструкции), а не как `S0_09NopAdmissionError` или гостевое исключение. Проверенное создание `CodeView` может отдельно вернуть `BuildError::AddressOverflow`; entry вне view и другие ошибки builder сохраняют собственные существующие типы. Полное свидетельство границ нельзя заменять ограниченным одним блоком scan.

### Обязательная эквивалентность

Единый механизм, который впоследствии реализует эти правила, обязан при каждом соответствующем профиле принимать ровно действующее множество форм и кодировок, отказывать на тех же входах с той же причиной и payload и сохранять IR, машинный код, состояние регистров и флагов и retirement. Параметризованный корпус должен проверить каждую форму и каждое правило отказа; одна новая разрешённая кодировка или одно расхождение означает невыполнение инварианта. Правила положительные и не грубее существующих предикатов; память и отдельный memory-path сохраняют своё поведение. Этот раздел не является расширением поддержки.

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

### S3-18e: capacity-one checked-memory cache fixture partial

The implemented bounded S3-18e fixture is a separate opt-in cache path for
only the two frozen S3-34a profiles: `StoreThenLoad` and `LoadThenStore`. Its
typed memory profile is distinct from S0_09 and register-only cache profiles. The
complete key includes the selected memory profile, exact full source bytes and
CodeView base/end/entry RIP, validated executable dependency spans (address
space, mapping and backing identities, their nonzero generations/versions and
permissions), and a nonzero execution-context generation plus the explicit
fixture profile. The caller holds the immutable source/dependency/context
snapshot for the entire synchronous lookup, compile, publication and execution
call; runtime CPU state, budget and invocation-local STOP/event controls are not
key inputs.

The cache retains at most one compiled artifact and one opaque typed published
handle. A hit revalidates the complete key before entry. A miss or explicit
clear drops the prior inactive handle before publishing its successor; the
retained handle remains alive through native execution and result validation.
The handle owns only executable code and immutable artifact metadata: it must
not retain pointers to invocation-local CPU state, RAM, helper context, STOP or
event controls. Each call supplies fresh state, memory and controls, and the
backend borrows them only for that synchronous execution and result validation.
`MemoryBlockExecutor` and `PublishedMemoryBlockExecutor` keep their existing
release-before-return contract for uncached dispatch. The opt-in cache uses
a separate typed backend/handle contract rather than weakening those one-shot
semantics. Its private synchronous ordinary-RAM safety preconditions continue
to apply on every call: code bytes do not alias mutable data RAM; there is no
MMIO, DMA, concurrency, re-entry or unwind through the helper; and writes are
all-or-nothing. Retaining a code mapping does not relax those requirements.
Publish or execution errors are terminal and do not replay committed memory
effects.

This section records the bounded fixture implementation, not production memory
ownership or completed invalidation. The bounded fixture generations let
tests prove source/dependency/context changes miss or invalidate; they do not
provide an MMU, live-owner mutation protocol, SMC/DMA coherence, asynchronous
compilation, multi-entry policy, or active-code retirement. The implementation
uses the existing typed two-site publish/execute APIs and does not route through
S0_09. Focused implementation evidence is in the
[S3-18e report](../reports/STAGE_3_S3-18e_2026-11-01.md). No benchmark,
default-policy, or performance conclusion follows. See
[ADR-0004](../decisions/ADR-0004-capacity-one-memory-cache.md). S3-18 and
S3-22/23/25 remain open.

### S3-18f: opt-in retained exact load-then-load cache profile

The capacity-one checked-memory cache also admits one exact profile,
`MemoryPairProfile::LoadThenLoad`, with only the six source bytes
`48 8B 03 48 8B 0B`. It is opt-in through the existing typed cache API; cache
capacity, complete source/dependency/context keying, replacement lifetime, and
the ordinary-RAM synchronous safety preconditions remain unchanged. The
published handle and cache key contain immutable code/artifact identity only.
Each invocation supplies current CPU state, RAM/helper boundary, access
context, STOP and event controls, budget, and stop-RIP anew.

The two checked reads remain independent helper calls, including when both
decode to the same address. Each successful load commits at its own guest
boundary; if the second read faults, the first load and RIP `A+3` remain
committed. The cache API continues to reject entry at internal RIP `A+3`; the
caller resumes through the established continuation path without replaying the
first load. `StoreThenStore` and all other byte sequences remain rejected by
cache admission. Existing guard order is preserved. This optional profile
does not change lowering, dispatcher routing, benchmark defaults, or any
performance claim; see the S3-18f implementation evidence when recorded.

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

### S3-25c: block lifecycle for one vCPU (normative)

This section is normative for the one-vCPU, synchronous profile only. It states
one lifecycle scheme and maps both implementations onto it; it adds no code
path. Reports: [S3-25cd report](../reports/STAGE_3_S3-25cd_2026-11-18.md).

**States.** A compiled block is in exactly one of four states:

1. **Unavailable for new entries.** No lookup can return it: it was removed from
   the lookup structure, or its key is stale (the backing/page generation or the
   fetched bytes no longer match). The block may still be physically owned.
2. **Active execution.** A native call of the block is in progress. The block,
   its metadata and its slot are borrowed for the whole call and for decoding
   its result; nothing can remove, replace or reuse them from inside the call.
3. **Referenced.** Not executing and not reachable from lookup, but something
   still holds it: an `Rc` handle to the arena slot, or a PC-map copy bound to
   the slot generation. There are no references through block links (no block
   linking exists; S3-31 is open), so a block is referenced only by handles
   and generation-bound PC-map copies.
4. **Physically freed.** The arena slot is returned to the free stack, or the
   production MAP_JIT region is unmapped. In the fixture arena this state is
   reached only at a mutating arena access (see "Safe point"), never under an
   arena borrow. The arena mapping itself is unmapped only after the last handle
   to the arena has dropped.

Two steps are kept apart. (a) *Destroying the last reference* (dropping the
handle) can happen at any moment, including from an owner callback while the
arena is borrowed; in the fixture arena that step only *records* the release
(`pending_release`). (b) *State 4* (the slot returned) happens only at the next
mutating arena access: `drain_pending_releases` at the start of `publish_*` or
in a handle drop that finds the arena free, never under a borrow.
`pending_release` and `drain_pending_releases` are a mechanism of the fixture
arena only (`MultiEntryBlockCache` + `PublishedArenaBackend`).
`PagedFetchedLoadSession` has no such path: the session takes `&mut self` and
has no external handles.

**Safe point.** A transition between states 1-4 that the cache, backend or
session performs itself (removal, clear, replacement, reset, retirement) is
allowed only at the safe point "after the native call returned and its
result/counters were checked, before the next lookup". Mechanism: the execution
entries take `&mut` on the cache/backend/session, so none of these can be called
while a block executes. Destroying the last handle is not such a transition and
is not bound to the safe point (step (a) above): a handle that drops while the
arena is shared-borrowed (an owner callback during the native call or during
preflight) only records its release in the slot's `pending_release` cell; the
slot is returned (state 4) at the next mutable access (`drain_pending_releases`
in `publish_*` or in a later drop), never under the borrow. Reaching state 4 may
therefore be delayed past the safe point (to the next publication or drop), but
a block in state 1 or later is never entered.

**Reuse and stale handles.** Every publication into a slot raises the slot
generation (`checked_add`, `GenerationExhausted` on overflow). A handle or
PC-map copy of an older generation is rejected before any CPU or memory
effect; a handle whose slot was freed but not yet reused finds no metadata. The
error name depends on the handle type: a V1 cached handle gives `StaleHandle`
for both a reused and a freed slot; a V2 `pc_map()` copy gives `StaleHandle` or
`InvalidBlockMetadata`; at execution V2 and V3 handles give `InvalidHandle`
(older generation) or `InvalidMetadata` (freed, not reused). Releasing a free
slot, a duplicate release and a release of a stale generation are no-ops, so a
slot is never on the free stack twice. Result: no cache entry, PC-map or handle
leads into a reused slot.

**Normative implementation.** `PagedFetchedLoadSession` (production owner,
S4-15/S4-16) is the normative implementation. The fixture
`MultiEntryBlockCache` + `PublishedArenaBackend` is a bounded model of the same
scheme with several slots and typed handles; its evidence covers the arena
mechanisms (generation, `pending_release`, lease) and does not by itself prove
properties of the production session. The two realize the states as follows.

| State / transition | Fixture: `MultiEntryBlockCache` + `PublishedArenaBackend` | Production: `PagedFetchedLoadSession` |
| --- | --- | --- |
| 1 Unavailable | `MultiEntryBlockCache::invalidate`, `clear`, `evict_oldest` (via `prepare_install`), `reset_for_execution_context`, removal after executor error; stale key never found by `find`. A typed V3 artifact is not in the cache: after `CodeInvalidated` its handle stays alive with the current generation, and unavailability for new entries is enforced by `validate_active_code_source_v3` returning `Preflight(ActiveSourceMismatch)` before the native entry (tests `active_code_v3_invalidates_exact_not_yet_executed_tail_immediate`, `active_code_v3_pc_map_survives_artifact_drop_invalidation_and_safe_reuse`); the slot is freed only when the owner drops the handle. | Reuse check in `execute_form`/`execute_forms` (not `retired`, same linear base, physical page, page generation, bytes) fails: the session sets `admitted = None` before compiling; `retire_after_code_invalidation` marks the block `retired` after its own store reported `CodeInvalidated`. |
| 2 Active | `PublishedArenaBackend::execute_cached_block`, `PublishedArenaMemoryStore::execute_tracked_code_store_v2`, `PublishedArenaActiveCodeV3::execute_tracked_v3` hold `arena.borrow()` through the call and result decoding; the dispatcher holds the cache entry. | `execute` (load only) and `execute_with_store` go through `execute_form`; `execute_forms` and `execute_or_interpret` are the other entries. All take `&mut self`; the admitted block is alive until the call returns. |
| 2 to 1 or 3 (safe point) | After the native return and result decoding, the notification (`CodeInvalidation`) is applied after the arena borrow was dropped (`execute_tracked_code_store_v2`, `execute_tracked_v3`); for cached entries `MultiEntryBlockCache::invalidate` then runs; for a V3 artifact the handle stays in state 3 and re-entry is rejected by `ActiveSourceMismatch` (row 1); STOP/EventPending leave the entry resident. | `retire_after_code_invalidation` runs after the call returned; nothing is released there. |
| 1 to 3 (outliving handle) | A typed handle or PC-map copy outlives the cache entry (`pc_map()` retained across artifact drop, invalidation and clear). | None: the session owns its block alone; there is no external handle. |
| 3 to 4 (physical free) | `Drop for PublishedArenaBlock` calls `ArenaInner::release_slot` (arena free) or only records the release in `pending_release` (arena borrowed; no state 4 yet); `drain_pending_releases` returns the slot on the next `publish_*` and on the next drop; the mapping is unmapped after the last handle. | `PagedFetchedLoadSession::reset`, the stale-block drop in `execute_form`/`execute_forms`, or dropping the session drops `PublishedCode`: munmap and the process-wide lease (`map_jit_region_live()`). A block with `retired` set is freed at the next entry that reaches the reuse check; an entry that aborts earlier (context, CR3, operand range, fetch error; in `execute_forms` the range is checked before the reuse check) leaves the block owned, but it cannot be entered because of `retired`. |
| 4 to reuse | New publication: slot generation + 1; old handles rejected by generation. | New admission maps a fresh region; the retired one is already gone or is dropped before it. |

**Tests per transition.** All names exist in the code.

- 1 (fixture): od-jit `dispatch::tests`
  `s3_54_dependency_invalidation_and_context_reset_prevent_stale_hits`,
  `backing_write_notification_invalidates_aliases_but_not_unrelated_backings`,
  `multi_entry_cache_reuses_sixteen_keys_and_evicts_exact_fifo_victim`,
  `post_publish_stale_candidate_is_dropped_before_entry_or_execution`; od-host
  `arena_cache_a_b_a_churn_reuses_only_safe_slots_at_capacity_one_and_two`,
  `tracked_v2_store_invalidates_alias_after_return_and_preserves_late_stop_commit`.
- 1 (production): `tests/system/stage4_memory_fault.rs`
  `s4_15f_generation_invalidation_replaces_the_block_without_leaking_the_region`,
  `s4_16i_after_the_exit_the_old_bytes_never_run_and_new_bytes_are_admitted`,
  `s4_16i_the_retired_block_stays_owned_until_the_next_admission_reset_or_drop`.
- 2 (fixture): od-jit `cached_stop_and_executor_error_release_only_at_safe_return_boundaries`
  (live handle during execute); od-host
  `s4_15f_handle_dropped_by_the_owner_during_a_native_call_is_freed_after_return`
  (the owner hook fires in `preflight_code_store`, under the arena borrow but
  before the native code is entered; the release is recorded and the slot
  returns at the next mutable access).
  2 (production): `s4_16i_store_into_the_executing_code_page_commits_and_exits_like_the_interpreter`,
  `s4_16i_store_fault_on_the_executing_page_precedes_the_write_and_keeps_the_block`.
- 2 to 1/3: od-host `active_code_v3_exits_on_dirty_prefix_and_finishes_unrelated_store_in_one_entry`,
  `active_code_v3_guards_faults_aliases_and_committed_stop_are_precise`,
  `active_code_v3_architectural_write_fault_matches_interpreter_and_is_atomic`;
  production `s4_15f_the_block_stays_owned_after_its_execution_until_reset_or_drop`.
- 1 to 3: od-host `active_code_v3_pc_map_survives_artifact_drop_invalidation_and_safe_reuse`,
  `direct_memory_v2_pc_map_survives_artifact_drop_and_arena_slot_reuse`.
- 3 to 4: od-host `s4_15f_arena_release_is_exact_and_a_duplicate_release_is_a_no_op`,
  `s4_15f_handle_dropped_under_a_borrow_is_deferred_not_leaked`,
  `arena_lease_outlives_backend_until_last_handle_and_can_reopen`; production
  `s4_15f_reset_between_two_entries_readmits_and_equals_the_interpreter`,
  `s4_15f_free_slots_return_to_the_initial_count_after_reset_and_after_drop`,
  `s4_15f_repeated_reset_and_late_drop_never_release_another_sessions_region`.
- 4 to reuse (stale generation rejected): od-host
  `direct_memory_v2_pc_map_survives_artifact_drop_and_arena_slot_reuse`,
  `active_code_v3_pc_map_survives_artifact_drop_invalidation_and_safe_reuse`.
- All transitions together at capacity 1 and 2 (S3-25d): od-host
  `s3_25d_small_capacity_lifecycle_stress_matches_interpreter_and_frees_everything`
  (about 530 steps: publish, execute, active-block invalidation, STOP and
  EventPending, injected helper fault, deferred release under a native call,
  stale-handle probes, cache clear, backend drop with live handles, reopen; each
  native execution compared with the interpreter; the end state has all slots
  free and `pending_release_count() == 0`). Comparison scope: in the cached
  runs the CPU state and `retired_instructions` are compared (exit is not; the
  interpreter continues on the same owner after the native run, which is sound
  because those blocks are register-only moves and `run_multi_cached` borrows
  the owner); in the V3 runs `backings[0]` (bytes and version) and `backings[2]`
  (variant 3) are compared fully, `backings[1]` (variant 1) only in
  `bytes[..8]`, without its version. Stale attempts of the last run (84) split
  into 48 forged handles of an old generation (`InvalidHandle` if the slot was
  reused, `InvalidMetadata` if it was freed) and 36 handles with the live
  generation rejected by the source check (`Preflight(ActiveSourceMismatch)`);
  `StaleHandle` does not occur in the stress.

**Rule for any future linking or backward transition (a rule, not a premise).**
A direct link between blocks, or any transition from one block into another
that bypasses lookup (return-address reuse, backward jump, chained entry), MUST
check the target's slot generation and key/page generation at the transition, or
else be unlinked at the same safe point at which its target leaves state 1.
Without that check the delay before a retired target is detected is unbounded
(S6-40 item 3). This holds today only because no such links exist; S3-31
(direct block links) must satisfy this rule before it can close.

**Limits.** Evidence is for one vCPU only. It holds for the current block
profile (at most 32 instructions per block, no linking, memory blocks with at
most two memory sites). There is no cross-thread release or retirement with a
grace period; SMP is future work. Writers other than the session (NVMe DMA,
display, loader/delivery paths not wired to the session) are outside this
section; for them safety rests on the key check at the next lookup (S4-16h).
Self-modifying code in shared multi-instruction blocks containing a store stays
with S3-24 (open); S3-25c/d cover only the single-instruction store exit of the
production session (S4-16i) and the exact V3 fixture artifact (S3-24a/b). The
S3-25d stress checks only the fixture arena and only on macOS aarch64; on other
platforms the test returns at once and passes without checks. The production
evidence is `s4_15f_*` / `s4_16i_*` (`tests/system/stage4_memory_fault.rs`,
macOS aarch64). No performance claim is made.

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
invalidation/lifecycle и проверки capacity (см. ISSUE-038).

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
Both `lookup` and `snapshot_admissibility_at` classify only aligned ARM64
instruction PCs within emitted code. A misaligned offset, the exclusive
code end or trailing padding has no snapshot classification.
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

### S3-29a: bounded demand-driven flags correctness candidate

`block_compile::compile_block_demand_flags_candidate`,
`block_compile::compile_block_for_nop_fallback_demand_flags_candidate` and
`block_emitter::emit_runtime_block_demand_flags_candidate` expose a separate,
opt-in uncached path beside the eager production/reference path. Callers must
not put this artifact into the shared translation cache under the eager
compile identity. The API does not select a default or claim a speedup.

The candidate is limited to the existing W64 register-only full arithmetic
writers ADD/SUB/CMP and the already admitted flag-preserving operations and
JZ/JNZ. It does not admit ADC/SBB, partial flag writers, or new guest forms.
Each full writer saves its original lhs/rhs to dedicated frame slots at
`SP+176` and `SP+184` and replaces the pending descriptor `{operation, W64,
operand slots}`. The slots survive later guest GPR writes and allocator spills.
The descriptor remains pending across MOV and other admitted non-writers; a
later full writer replaces it without calling the flags helper for the older
writer. The candidate frame prefix is 192 bytes; the eager 176-byte frame and
its code path are unchanged.

The guard fast path only checks stop-RIP, STOP, and budget. An out-of-line
guard/terminal return stub materializes pending flags only if that exit is
taken. A Jcc materializes before reading ZF and selecting its RIP. The shared
`integer_flags` helper receives the last writer's saved operands and the
unchanged staged pre-write RFLAGS, so it computes all defined arithmetic bits
and preserves the other bits. Successful materialization updates staged and
canonical RFLAGS. The guest commit omits RFLAGS while the descriptor is
pending; `PcMap` ranges and committed boundaries use
`InadmissiblePendingFlags` until the canonical RFLAGS store has completed.
That inadmissible span includes the store instruction itself; a map point is
canonical only at the following host PC. Pending helper-failure epilogues also
remain inadmissible because the helper did not materialize the stale flags.
Helper failure remains a mapped host-contract error and does not become a
guest exit. STOP, budget, stop-RIP, block completion, unsupported tail and
deferred decode exits all return canonical RFLAGS on success.

Correctness checks currently cover native candidate/eager/interpreter parity
for the tested bounded sequences, actual helper-call laziness, flag-preserving
MOV with source-register clobber, replacement of ADD/CMP descriptors, alias
inputs, both JZ/JNZ outcomes, register budgets 0..4, budget/stop-RIP exits,
unsupported/deferred exits and mapped helper failure. This is a correctness
candidate only. Partial writers, ADC/SBB consumers, memory/helper handoff,
physical-x86 validation, the final eager-vs-candidate performance comparison,
policy selection and overall S3-29 acceptance remain open.

### S3-29b: uncached demand-flags multi-block NOP dispatcher

`dispatch::run_with_nop_fallback_demand_flags_candidate` is an explicit opt-in
entry point over the same bounded dispatcher loop as
`dispatch::run_with_nop_fallback`. It recompiles each reached native block
with `compile_block_for_nop_fallback_demand_flags_candidate`; it does not use
the shared translation cache. The existing admission, `DispatchControl`,
`ControlledBlockExecutor`, counter validation, bare-NOP interpreter bridge,
event ordering, and terminal/error handling remain shared. The eager entry
point continues compiling eager blocks and remains the default/reference.

Native coverage includes taken and non-taken JNZ back-edge iterations followed
by bare NOP, native CMP/JZ with both branch outcomes, and a following native
MOV on the non-taken path. The executor witness requires an actual
`InadmissiblePendingFlags` candidate PC-map range. Budget 0 through full
completion and every reachable instruction stop-RIP boundary compare complete
CPU state with the interpreter; resuming from each boundary consumes only the
remaining instructions. Events injected between blocks, including at the NOP
entry, return `EventPending` with the event retained; clearing the event and
resuming reaches the same final state without replay. STOP-RIP, STOP and budget
remain ahead of a simultaneous event as in S3-28a. Published executable code
is dropped by the existing executor before the shared loop compiles or steps
the next instruction.

This remains an uncached correctness candidate only. It does not select a
default, expand the ISA/admission or memory profile, integrate a production
owner, or claim performance. Full S3-29 and the final eager/candidate comparison
remain open.

### S3-29c: partial-flags interpreter handoff

Separate `S0_09NopIncRaxAdmission` and `run_with_nop_inc_rax_fallback*` APIs
admit S0-09 forms, exact bare NOP, and only exact `48 FF C0` (`INC RAX`). The
existing NOP-only admission and entry points continue rejecting INC. Branch
targets must be decoded instruction boundaries; malformed/truncated bytes,
prefixes, other widths/registers, DEC and branches into the INC bytes are not
admitted.

Native eager and demand-flags candidate prefixes stop at the admitted INC
boundary. The candidate materializes canonical RFLAGS before returning
`NeedInterpreter`; the dispatcher then drops the completed native mapping and
uses `od_isa::execute::step` with an immutable three-byte fetch window. The
bridge retires exactly one instruction and requires RIP to advance by three.
In this bridge path, INC itself is not lowered natively. Its existing interpreter semantics
preserve CF and compute OF/SF/ZF/AF/PF; the next native Jcc/suffix block begins
from the resulting state. Event requests are sampled before the synchronous
INC step and remain pending on yield; there is no deterministic callback point
between that one interpreter step and the following dispatcher guard. STOP-RIP
and STOP are checked before the step and may stop at either side of INC.

Native host coverage compares full CPU state and accounting with eager and
interpreter paths for CMP→MOV RAX,imm64→INC→JZ/JNZ→native suffix, both CF inputs,
both branch outcomes, budgets, and stop/resume at every reachable instruction
boundary. Literal
flags cover zero, signed qword extrema, `u64::MAX`, nibble and parity edges;
the candidate PC-map witness requires pending flags before handoff. S3-29
remains open pending remaining correctness gates, final strategy comparison,
and deferred Mini/performance work.

### S3-33a: opt-in exact native INC RAX

The separately named uncached `compile_block_for_native_inc_rax*` and
`dispatch::run_with_native_inc_rax*` paths reuse
`S0_09NopIncRaxAdmission`. They may lower only the admitted exact bare
`48 FF C0`: W64 integer Add with constant one and a `WriteMask` for PF/AF/ZF/SF/OF.
The staged RFLAGS merge preserves CF and every bit outside that mask. A pending
full arithmetic writer in the demand-flags candidate is materialized before
INC; its live allocator homes are preserved across the helper call. INC itself
does not become a pending writer. Other registers, DEC, memory operands, and
prefixed encodings remain outside this native opt-in path. Existing S3-29c
interpreter-bridge APIs remain unchanged and continue to serve as the baseline.

Host coverage compares complete state, flags and retirement with the interpreter
for CF 0/1, arithmetic edges, and following native JZ/JNZ consumers. It also
covers candidate budget and STOP-RIP boundaries and a pending full writer before
INC. This is bounded correctness evidence only; it changes no shared cache or
default policy and makes no performance claim. Mini/performance remain deferred
until Stage 3 ends. See [S3-33a report](../reports/STAGE_3_S3-33a_2026-10-30.md).

### S3-33b: opt-in exact native DEC RAX

The separately named uncached `compile_block_for_native_dec_rax*` and
`dispatch::run_with_native_dec_rax*` paths use their own
`S0_09NopDecRaxAdmission`; the S3-33a INC admission and APIs remain INC-only.
They accept only exact bare `48 FF C8` (`DEC RAX`) in addition to S0-09 forms
and bare NOP. DEC lowers as W64 Subtract by constant one with the existing
INC/DEC partial RFLAGS mask PF/AF/ZF/SF/OF, so CF and every other bit are
preserved. The demand-flags candidate materializes any pending full writer
before DEC and preserves live allocator homes across that helper boundary;
DEC flags are merged eagerly, with no new pending descriptor or helper ABI.
The path remains uncached and opt-in.

Compiler and host differential coverage checks exact negative admissions,
complete CPU state and retirement against `od_isa::execute::step` for both CF
inputs and zero, one, `u64::MAX`, signed extrema, AF/PF edges, and native JZ/JNZ
consumers. Candidate budgets and STOP at entry and immediately before DEC are covered. This is partial
Stage 3 correctness evidence only; no performance claim is made, and Mini and
performance checks remain deferred to the end of Stage 3. See
[S3-33b report](../reports/STAGE_3_S3-33b_2026-10-30.md).

### S3-33c: opt-in exact native NEG RAX

The separate uncached `compile_block_for_native_neg_rax*` and
`dispatch::run_with_native_neg_rax*` APIs use their own
`S0_09NopNegRaxAdmission`. They admit only exact bare `48 F7 D8` in addition
to S0-09 forms and bare NOP; INC-only and DEC-only admissions remain unchanged.
Lowering emits Read RAX, W64 constant zero, W64 `Subtract(0, RAX)` with full
`FlagEffect::Arithmetic`, Write RAX, then Commit. NEG does not consume old CF,
and the demand-flags candidate replaces any pending full flags result without
adding a descriptor or helper ABI. The path is uncached and opt-in; it changes
no shared cache, default policy, IR kind, emitter ABI, or flags mask.

Compiler and host coverage checks exact bytes and rejected neighboring forms,
full-state and retirement parity for both incoming CF values, arithmetic and
AF/PF edges, JZ/JNZ consumers, candidate budget/STOP boundaries, terminal NEG,
and NEG followed by arithmetic. The PC-map witness checks a pending statepoint
at the branch consumer. This is partial correctness evidence only; Mini and
performance checks remain deferred until Stage 3 ends. See
[S3-33c report](../reports/STAGE_3_S3-33c_2026-10-30.md).

The separate uncached `compile_block_for_native_add_rax_imm8*` and
`dispatch::run_with_native_add_rax_imm8*` APIs use
`S0_09NopAddRaxImm8Admission` for exact bare `48 83 C0 ib` only. The decoder's
sign-extended immediate is carried into W64 `Add` lowering with full arithmetic
flags. Admission cross-checks each raw immediate byte against the decoder's
sign-extended value; lowering uses that decoded value without recalculating it.
INC, DEC, NEG, and generic admission contracts remain
operation-specific. The path is opt-in and uncached, with no performance claim;
Mini and performance checks remain deferred until the Stage 3 gate. See
[S3-33d report](../reports/STAGE_3_S3-33d_2026-10-30.md).

### S3-33e: opt-in exact native CMP RAX, sign-extended imm8

The separate uncached `compile_block_for_native_cmp_rax_imm8*` and
`dispatch::run_with_native_cmp_rax_imm8*` APIs use their own
`S0_09NopCmpRaxImm8Admission` for exact bare `48 83 F8 ib` only (W64 RAX,
opcode `83 /7`, length four). Admission cross-checks each source immediate
byte against the decoder-owned sign-extended value; lowering consumes that
decoded value as a W64 constant. It emits `Compare` with full
`FlagEffect::Arithmetic` and `result=None`, then commits without any GPR
write. Existing generic, INC, DEC, NEG, and ADD-immediate admissions remain
separate. The path is opt-in and uncached; Mini and performance checks remain
deferred until the Stage 3 gate. See
[S3-33e report](../reports/STAGE_3_S3-33e_2026-10-30.md).

### S3-33f: opt-in exact native SUB RAX, sign-extended imm8

The separate `compile_block_for_native_sub_rax_imm8*` and
`dispatch::run_with_native_sub_rax_imm8*` APIs use their own
`S0_09NopSubRaxImm8Admission` for exact bare `48 83 E8 ib` only (W64 RAX,
opcode `83 /5`, length four). Admission checks every source immediate byte
against the decoder-owned sign-extended value, which lowering consumes as a W64
constant. The lowering is Read RAX → Const → W64 `Subtract` with
`FlagEffect::Arithmetic` and `result=Some` → Write RAX → one Commit. This
updates all arithmetic flags even for immediate zero; `FF` means subtract
negative one and is not INC semantics. Existing generic, INC, DEC, NEG, ADD,
and CMP admissions remain separate. The path is opt-in and uncached; Mini and
performance checks remain deferred until the Stage 3 gate. See
[S3-33f report](../reports/STAGE_3_S3-33f_2026-10-30.md).

### S3-33g: opt-in exact native NOT RAX

The separate `compile_block_for_native_not_rax*` and
`dispatch::run_with_native_not_rax*` APIs use `S0_09NopNotRaxAdmission` for
bare `48 F7 D0` only. Other group selectors, registers, memory operands,
widths, prefixes, truncated instructions, and branch targets inside the
encoding are rejected. Lowering computes `u64::MAX - RAX` as W64
`Subtract + FlagEffect::Preserve + result=Some`, writes RAX, and commits once.
Runtime emitter acceptance for this preserve operation is limited to this
shape. It performs no flags-helper call or flags staging and leaves full
RFLAGS, flag definedness, live allocator homes, and any prior pending-flags
descriptor and saved operands intact. Its PC-map range does not claim
`AfterHelper`. Full-state comparisons cover pending ADD/CMP followed by NOT and
JZ/JNZ, repeated NOT, STOP/budget boundaries, and a successor flags writer.
This is an opt-in uncached correctness slice; Mini and performance checks are
deferred until Stage 3 ends. See the
[S3-33g report](../reports/STAGE_3_S3-33g_2026-10-30.md).

### S3-33h: opt-in exact native LEA RAX,[RBX]

Separate `compile_block_for_native_lea_rax_rbx*` and
`dispatch::run_with_native_lea_rax_rbx*` entry points use an uncached profile
for bare `48 8D 03` only. Admission requires exact three-byte source identity
and decoded W64 destination RAX, 64-bit address size, base RBX, no index,
scale 1, and zero displacement. Alternate encodings with SIB or displacement,
other registers, RIP-relative forms, address32, prefixes, truncation, and a
branch into the instruction are rejected. Lowering contains only
`Begin`, `ReadGpr(RBX,W64)`, `WriteGpr(RAX,W64)`, and one `Commit`. No checked
load, data-memory access, canonicality check, helper, arithmetic, or flags
operation is involved. Full RFLAGS/definedness and any pending-flags descriptor
and saved operands are preserved. Eager and demand-flags candidate entry points
are separate and opt-in. An admitted NOP boundary may use the existing bounded
interpreter fallback; it fetches only the exact LEA's three source bytes and
retirement accounting remains shared with native execution. Mini and
performance checks remain deferred until Stage 3 ends. See the
[S3-33h report](../reports/STAGE_3_S3-33h_2026-10-30.md).

### S3-33i: opt-in exact native XCHG RAX,RBX

Separate `compile_block_for_native_xchg_rax_rbx*` and
`dispatch::run_with_native_xchg_rax_rbx*` entry points use an uncached profile
for exact bare bytes `48 87 D8` only. Admission also requires the decoded W64
register exchange RAX/RBX; reject equivalent `48 87 C3`/`48 93`, self-exchange,
other register pairs or widths, memory forms, LOCK/other prefixes, truncation,
and branches into the instruction. Lowering reads old RAX and RBX before either
write, writes RAX from old RBX and RBX from old RAX, then commits once. The
runtime emitter permits two writes only for this exact ordered IR window, with
two distinct temporaries and no mixed operation or third write. Intermediate
statepoints must replay the XCHG or be inadmissible; no canonical snapshot may
publish one exchanged register. XCHG has no flags/helper effect and preserves
full flags/definedness and pending-writer descriptor/operands. This is register
semantics only and makes no memory-atomicity claim. Eager and demand-flags
candidate APIs are separate and uncached. Mini and performance remain deferred
until Stage 3 ends. See [S3-33i report](../reports/STAGE_3_S3-33i_2026-10-30.md).

### S3-33j: opt-in exact native LEA RAX,[RBX+disp8]

Separate uncached compiler/dispatcher entry points admit only bytes
`48 8D 43 db`, with all 256 raw displacement bytes. Admission binds exact bytes
to decoder-owned W64 RAX destination, address64, RBX base, no index, scale 1,
and `i64::from(db as i8)` displacement. The existing no-displacement
`48 8D 03` profile is unchanged and does not accept this encoding. Lowering
reads RBX, adds the sign-extended displacement as W64 wrapping arithmetic with
`FlagEffect::Preserve`, writes RAX, and commits once. It has no memory operation
or data access, and performs no canonicality/fault check or helper call. Full
RFLAGS/definedness and any pending-flags descriptor/operands remain unchanged.
The preserving emitter path accepts only W64 Add/Subtract with a result;
Compare, no-result, other widths, and other flag effects remain unsupported.
Eager and demand-flags candidate APIs remain separate and uncached. Local
differential coverage is specific to these exact bytes; no physical exact-byte
confirmation is claimed. Mini and performance remain deferred until Stage 3
ends. See [S3-33j report](../reports/STAGE_3_S3-33j_2026-10-30.md).

### S3-33k: opt-in exact native MOV RAX,sign-extended imm32

Separate uncached compiler and dispatcher entry points admit only the seven
bare bytes `48 C7 C0 id`, where `id` is the little-endian immediate. Admission
binds these bytes to decoder-owned W64 `MovImmediate` with destination RAX,
ModRM `/0`, and length 7; the decoder-owned signed-extended value must agree
with the four source immediate bytes. No other opcode extension, register,
memory destination, operand/address width, prefix, alternate `48 B8 imm64`
encoding, or truncated source is admitted. The existing `48 B8 imm64`
profile is unchanged.

Lowering uses the decoder-owned signed-extended value as a W64 constant and
performs `WriteGpr(RAX)` with one instruction commit. Every other GPR,
incoming full RFLAGS/definedness, and pending flag-writer descriptor and
operands are preserved. Local differential coverage includes preceding
register ADD/CMP writers followed by MOV and JZ/JNZ consumers. The operation
contains no arithmetic, data-memory access, or helper call.
Eager and demand-flags candidate compile/dispatch APIs remain separate and
uncached. Local differential coverage is specific to the exact bytes; no exact
physical-x86 source-byte confirmation is claimed. Mini/performance checks
remain deferred until Stage 3 ends. See
[S3-33k report](../reports/STAGE_3_S3-33k_2026-10-30.md).

### S3-33l: opt-in exact native LEA RAX,[RBX+RCX]

The separate uncached profile admits only exact bytes `48 8D 04 0B`: W64 and
address64, destination RAX, base RBX, index RCX, scale 1, displacement 0,
length 4. It keeps the earlier no-displacement and disp8 LEA profiles
independent. Lowering reads RBX and RCX before any write, adds them as W64 with
`FlagEffect::Preserve`, writes RAX, then commits once. Full RFLAGS, definedness,
and pending flag-writer state are preserved. LEA performs no data access,
canonicality check, flags helper, or memory helper. Eager and demand-flags
candidate compile/dispatch remain uncached opt-in APIs.

Local negative admission covers neighboring ModR/M/SIB encodings, register
swaps, scales, no index, displacement forms, RIP-relative addressing, prefixes,
truncation, and a branch into the instruction body. Host differential checks
cover representative operands, full state, flags, pending CMP→LEA→JNZ with
opposite predecessor/sum ZF, zero data callbacks, budget exhaustion, and STOP
before/after retirement. Exact bytes have local differential evidence only;
no physical fixture confirmation is available. Mini/performance remain
deferred until Stage 3 ends. See
[S3-33l report](../reports/STAGE_3_S3-33l_2026-10-30.md).

### S3-33m: opt-in exact native LEA RAX,[RIP+disp32]

The separate uncached profile admits only `48 8D 05 dd dd dd dd`, length 7,
with W64/address64, destination RAX, `AddressBase::NextRip`, no index, scale
1, and displacement exactly bound to the decoded signed disp32. Lowering
computes `instruction.next_rip.wrapping_add_signed(displacement)` from the
specific guest instruction, emits a W64 constant, writes RAX, and commits once.
Flags, other GPRs, and pending flags are preserved. The result is not treated
as a memory address; no data access, canonicality check, branch, or helper is
performed. Exact profile remains separate from other LEA forms.

Admission/compiler/host checks cover exact-form negatives, variable-length
preceding instructions, signed displacement edges, different guest RIPs,
wrapping result arithmetic, pending CMP→LEA→JNZ, full state parity and zero
data callbacks. Evidence is local differential only; no physical fixture is
claimed. Mini/performance remain deferred until Stage 3 ends. See
[S3-33m report](../reports/STAGE_3_S3-33m_2026-10-30.md).

### S3-33n: opt-in composed exact `NativeRegisterV1`

`NativeRegisterV1Admission` is a separate uncached whole-code witness for the
union of restricted S0-09, the already admitted bare NOP interpreter fallback,
and only the exact native forms committed in S3-33a–m: `INC/DEC/NEG/NOT RAX`,
`ADD/CMP/SUB RAX,sign-extended imm8`, `MOV RAX,sign-extended imm32`,
`XCHG RAX,RBX`, and the four exact RAX LEA forms (`[RBX]`, `[RBX+disp8]`,
`[RBX+RCX]`, `[RIP+disp32]`). Their byte patterns and operand constraints
remain those recorded by the individual S3-33a–m sections. No other widths,
registers, prefixes, memory data operations or encoding alternatives are
implied. Existing standalone admission APIs remain unchanged.

Admission decodes the complete immutable code view once, records exact
instruction starts and binds each additional form to the existing exact
predicate plus decoder-owned operands and source bytes. All branch targets
must be exact decoded starts or the exclusive code end; an interior target is
rejected across all admitted variable instruction lengths. Only exact bare
NOPs can take the already admitted interpreter fallback. Eager and
demand-flags-candidate compile/dispatch APIs remain explicitly selected and
uncached. Lowering uses existing per-form semantic handlers; this profile adds
no new x86 semantics and does not change the default profile.

Mixed-block parity, pending-statepoint, ordered-pair corpus, exact local gate,
and full-suite results are recorded in the [S3-33n report](../reports/STAGE_3_S3-33n_2026-10-30.md).

### S3-38b runtime cache slice: eager NativeRegisterV1

`run_cached_native_register_v1_controlled` is a separate opt-in path over a
fixture-owned immutable `CodeView`, complete caller-owned dependency spans and
a frozen NativeRegisterV1 execution-context generation. It reuses
`SingleEntryBlockCache` and eagerly compiles with
`compile_block_for_native_register_v1`; the demand-flags candidate remains
uncached. Its `NativeRegisterV1Fixture` semantic profile and
`S3_38bNativeRegisterV1Cached` compile flavor are distinct from the S0-09
fixture dispatcher, so identical source bytes and RIP do not share an entry
across profiles. The generic S0-09 dispatch admission is unchanged.

The one resident entry is keyed and revalidated against the complete code
view, block boundaries/terminal, dependency identity and versions, and frozen
execution context. A replacement drops the previous opaque published handle
before publishing its successor; execution borrows the retained handle through
result validation. STOP and event guards are checked before lookup and again
after publication immediately before entry. Exact bare NOP remains a one-step
interpreter fallback and consumes the same guest budget. This path makes no
SMC, owner/MMU, Mini or speed claim. Host/unit parity, branch-loop reuse,
invalidation, capacity, guard priority and publication lifetime are recorded in
the [bounded runtime cache report](../reports/STAGE_3_S3-38b_NATIVE_REGISTER_CACHE_2026-10-30.md).

### S3-38c runtime cache slice: demand-flags NativeRegisterV1 candidate

`run_cached_native_register_v1_demand_flags_candidate_controlled` exposes the
existing demand-flags compiler through the same one-entry fixture cache loop.
It keeps the `NativeRegisterV1Fixture` semantic profile but uses the distinct
`S3_38cNativeRegisterV1DemandFlagsCachedCandidate` compile flavor, preventing
cross-hits with eager artifacts. The API remains opt-in and experimental;
pending flag descriptors/operands are execution-local and are not cache state.
No default dispatch choice or performance conclusion follows from this slice.
S0-09, memory, and multi-entry admissions are unchanged. See the S3-38c report.

### S3-29d: uncached arithmetic-to-checked-load handoff

The bounded host fixture composes existing eager and demand-flags native
compilation with the existing V2 memory dispatcher and
`PublishedMemoryBlockExecutor` for one or two exact W64 arithmetic encodings:
`48 03 C3` (ADD RAX,RBX), `48 2B C3` (SUB RAX,RBX), or `48 3B C3`
(CMP RAX,RBX), followed by
`48 8B 03` (MOV RAX,[RBX]) and a native JZ/JNZ suffix. It uses one original
immutable `CodeView`, passes the same remaining budget across phases, and
starts each next phase at actual guest RIP. Candidate native entry samples
stop/event controls again immediately before entering the artifact. The memory
executor completes and drops its mapping before the suffix is compiled and
entered. This is fixture-scoped integration evidence; it adds no general
dispatcher, cache route, public runtime API, owner/MMU integration, or new
instruction form.

The host test compares full state, RAM, callback count, and separate native,
checked-helper, and interpreter retirement counts for candidate/eager/interpreter
paths, both JZ and JNZ outcomes, individual ADD/SUB/CMP operations, and
two-writer ADD→SUB and ADD→CMP chains. The candidate
PC-map witness shows pending flags in the native prefix, while the interpreter
comparison checks canonical RFLAGS at the load handoff. A successful load
retires once and performs one qword callback. #PF and noncanonical #GP preserve
the prefix and load RIP without committing RAX or entering the suffix;
noncanonical #GP occurs before a callback. Host backend errors remain executor
errors. Fault/error/retry injection uses the demand-flags candidate path;
eager/interpreter fault parity is not claimed. Repaired fixtures retry the load
and suffix without replaying the prefix. A successful callback event commits the load and yields before native
suffix entry; simultaneous event and guest fault returns the fault. Budget,
STOP-RIP, STOP, and event precedence are exercised at the handoff, and branch
targets inside the three-byte load are rejected by the fixture boundary check.
Malformed native reports are not injected in this fixture; existing checked
memory dispatcher tests retain invalid-accounting and zero-progress coverage.
S3-29 remains open; eager stays the default, candidate remains uncached, and
Mini/performance comparison remains deferred until the end of Stage 3.

### S3-29e / S3-15p: bounded production coordinator

`od_jit::memory_dispatch::run_bounded_arithmetic_load` replaces fixture-side
phase orchestration for this exact slice. It accepts one or two exact W64
writers (`48 03 C3`, `48 2B C3`, `48 3B C3`), the exact checked `48 8B 03`
load, JZ/JNZ with a target at the full-view code end, and the existing
fallthrough `ADD RAX,RBX`. It validates the complete byte sequence and decoder
boundaries against one immutable full `CodeView`; phase views never rebase
guest RIPs, and the prefix end is not treated as the global code end. A caller
may restart at the load RIP after a fault or at any other decoded instruction
boundary after a budget/STOP handoff without replaying completed work.

The API carries one budget and STOP, stop-RIP, and event controls across
phases. Eager compilation is the default; demand-driven flags are explicit and
uncached. The existing `ControlledBlockExecutor` hook resamples control after
publication and before native entry for adapters that separate those steps.
Native reports and phase RIPs are validated, including budget-exit
consistency and retirement returned by terminal exits; the exact V2 load is dispatched
through `MemoryBlockExecutor`, whose mapping is released before native suffix
compilation. Typed failures preserve compile, host-executor, memory-dispatch,
accounting, progress, and admission distinctions. This does not add a cache,
new ISA form, `od-machine` owner/MMU path, full S3-15 corpus acceptance, or
performance result. Mini and performance checks remain deferred until Stage 3
ends; eager remains the default and S3-29 stays open.

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
known ISSUE-040 x87 failures. A local-only run on this harness revision passed
for 10 manual plus 1000 generated inputs, seed `0x5eedcafe`, and two repeats
(4040 local engine results; 2020 cross-engine and 2020 repeat comparisons); its
manifest marks `physical_x86=not_run`. Physical triple comparison remains
pending.

#### S3-15 checked-memory borrowed dispatcher partial

`od_jit::memory_dispatch::run` is a separate uncached synchronous dispatcher
for only the two exact checked qword memory pairs and their admitted singleton
load/store forms. It borrows an immutable `CodeView` and takes an explicit
`FixtureExecutionBoundary`, which is a byte-window limit and may split an
instruction; supported pairs compile as one artifact only when all six bytes
fit, otherwise the current complete instruction compiles as a singleton. It
continues from actual
`LongModeState.rip` and forwards the remaining instruction budget to every
host execution. Stop-RIP and cooperative STOP are checked before boundary
handling, and a zero budget does not compile or publish another artifact.
Compile/lowering rejection is `Unsupported`, never a guest fault. Reaching a
byte-window end or encountering a partial instruction returns
`BoundaryReached { rip }` without compiling or executing past the limit. This
is not a guest fetch fault or guest halt. Complete one-byte unsupported
instructions and complete invalid encodings retain separate `Unsupported` and
`Invalid` outcomes even when they end the byte window. Production code-fetch
fault delivery requires the future owner-supplied fetch integration.

`od_host::PublishedMemoryBlockExecutor` publishes, runs, decodes, and drops
exactly one mapping per artifact before returning to the dispatcher. The
dispatcher accepts only exact compiler-issued artifact types, preserves a
committed prefix on later architectural or host errors, and never caches code.
Both the uncached dispatcher and the retained memory-pair cache accept a
`GuestFault` report only when the remaining budget permits the faulting
instruction and the canonical RIP matches the artifact site indexed by the
number of retired instructions. A report outside those bounds fails the host
executor contract. The cache drops the affected mapping at the synchronous
return boundary without retrying the artifact. The uncached dispatcher retains
the rejected report in its error so the arithmetic/load coordinator can keep
its existing phase RIP and invalid-memory-report diagnostics.
Its unsafe caller contract requires immutable non-aliasing code bytes and
private synchronous ordinary data RAM with all-or-nothing writes, no MMIO,
executable/code aliases, DMA, concurrency, re-entry, or unwind. `GuestMemory`
does not prove those ownership properties. There is no production RAM/MMU
owner in `od-machine` yet; VMHost owner integration and owner-supplied code
fetch remain open. This does not cover a guest fault on the next code fetch,
general S3-15, interpreter fallback,
physical x86 comparison, cache/invalidation, or performance acceptance. See
the [bounded memory dispatcher report](../reports/STAGE_3_S3-15_2026-10-27.md#bounded-checked-memory-dispatcher-partial).

#### Bounded arithmetic/checked-load coordinator (S3-15p)

The public `run_bounded_arithmetic_load` entry composes the register compiler,
V2 checked-load dispatcher, and existing executor contracts for the exact
arithmetic/load/Jcc/fallthrough slice described in S3-29e above. It checks the
whole plan before execution, forwards remaining budget and controls, and
accepts restart at each decoded arithmetic boundary, the load RIP, the Jcc
instruction RIP (the suffix start, for example after an event yield), its
decoded fallthrough, or code end. These points are derived from the exact
validated plan; this does not admit arbitrary branch targets. Native and
memory phase outcomes are checked against their exact decoded boundaries
before terminal exits or handoffs are accepted. Valid native control exits
honor stop-RIP, STOP, global code end, budget, then event priority; guest
faults and host errors retain their original outcome.
The check is uncached and borrowed. Mapping release is sequential: register
publication ends before the memory callback starts, and memory publication
ends before suffix native compilation. The API does not prove private RAM
ownership, code-fetch fault delivery, alias invalidation, or VMHost/MMU
integration.

#### Production memory-owner integration prerequisites (S3-15n)

The borrowed dispatcher above is a bounded API, not production VMHost memory
integration. `od-machine` currently has no production RAM/MMU owner. The target
one-vCPU dependency boundary is:

| Existing API/evidence | Missing guarantee | Owner component | Acceptance evidence required |
|---|---|---|---|
| `GuestMemory` fetch/read/write and fixture `CodeView` | Private synchronous ordinary RAM capability, backing identity, aliases, mapping/permission identity, revocation | `od-machine` | Owner-issued access capability; reject MMIO/shared/DMA/concurrent or revocable views for this path; test permission and mapping revocation |
| Borrowed immutable `CodeView`; caller holds frozen bytes | Owner-backed code snapshot/lease binding guest byte ranges to code/backing identity, mapping identity/generation, execute permission and bytes for the full compile/publish/execute interval; explicit release before conflicting publication | `od-machine` supplies lease; `od-jit` consumes it | Change/remap/revoke cannot race an active lease; lease released on every normal/error exit before next publication |
| Fixture byte-window and `BoundaryReached` | Incremental guest fetch/decode with MMU permissions and precise deferred fault at the first reached RIP; byte-window end is not guest termination | `od-machine` fetch; `od-jit` continuation policy | Fetch across pages; prefix commits before later fetch fault; fault payload appears only when RIP is reached; stop-RIP → STOP → budget precedes fetch |
| Fixture tracked writes and region-version invalidation | Notifications from every committed writer path, including interpreter, JIT, loader/debugger/control and future device/DMA writers; alias-aware target version updates and active-code dirty signal | `od-machine` central writer/mapping owner; `od-jit` dependency invalidation | Each transaction emits one committed batch with backing/mapping identities and changed ranges/versions; test aliases, remap/protection change, failure without notification, invalidation before next lookup |
| `PublishedMemoryBlockExecutor` and one-artifact-at-a-time fixture test | Production owner lease and JIT dirty/invalidation safe point composed with executable mapping lifetime | `od-host` publication; `od-jit` dispatch; `od-machine` owner | Native end-to-end test proves mapping dropped after return/result decode, owner lease released, notifications applied before lookup/publication, and no stale code executes |

The owner contract needs observable operations and lifetime guarantees; it does
not prescribe a new trait hierarchy. A code lease identifies guest source
ranges and their backing/mapping identities and generations, keeps source bytes
immutable with execute permission for its lifetime, and is explicitly released
before the owner revokes or mutates those mappings. If writers may run during
compilation, the candidate must be revalidated against the same identities and
versions before publication; otherwise the owner keeps the lease frozen through
native return and result decode.

Fetch is owner-mediated and instruction-sized: the decoder may request the
remaining architectural prefix across mapping boundaries, while the owner
reports fetch permission/fault at the exact guest address. A fetch/decode error
after a valid prefix is deferred until continuation reaches that RIP. A host
compile/publication error remains a host error. Dispatcher checks are ordered
stop-RIP → cooperative STOP → remaining budget → fetch/decode. Reaching a
fixture source window is only `BoundaryReached`; only an owner-supplied guest
execution boundary or architectural fetch result can establish guest progress
or termination.

Committed-write notifications describe the validated write batch after commit,
including each affected backing identity/range and its new version. All paths
that can write guest code use this owner path. The owner marks overlapping
active code dirty, and dispatch observes that signal at a safe point before its
next cache lookup or artifact publication. Alias mappings resolve to the same
backing dependency even when guest virtual ranges differ. Permission changes,
unmap/remap, and backing replacement also advance the relevant mapping/code
identity and invalidate affected dependencies. Notification does not replace
the write transaction or all-or-nothing validation.

Generic `GuestMemory` alone is insufficient evidence for the unsafe private
synchronous RAM contract: it does not establish exclusive ordinary-RAM
ownership, exclude MMIO/DMA/concurrent aliases, or prove revocation and mapping
lifetime. The future production path must receive a verifiable owner-issued
capability/lease or retain equivalent explicit caller obligations at its unsafe
boundary; it must not silently treat arbitrary `GuestMemory` as private RAM.

Prerequisites are ordered: implement `od-machine` backing/mapping/MMU ownership
and permission/revocation tests; connect all committed writers and fetch to
owner identities, versions and notifications; then connect `od-jit` dispatch/
cache invalidation and `od-host` publication lifetime and run the end-to-end
matrix above. Final Mac mini physical-oracle acceptance and representative
cold/warm performance measurements remain later Stage 3 gates; fixture
differential tests do not satisfy them. See [ISSUE-038](../KNOWN_ISSUES.md).

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

#### S3-16: paged publication refusal

The single-vCPU paged loop falls back for a supported `MOV RAX,[RBX]`
when the MAP_JIT lease is busy before native entry. The attempted fetch
can commit page-walk Accessed bits. The interpreter then consumes one
instruction and one unit of budget. No block was published for that step.
After the lease is released, the test resumes the same session at the
next instruction and checks exactly one native retirement at the next
visit to the load. The combined write log, registers, RIP, flags, RAM
and retirement agree with the interpreter. The local Stage 3 selector
requires the exact `s3_16_publication_refusal` gate.

#### S3-16m: exact bare-NOP continuation for the bounded memory dispatcher

`od_jit::memory_dispatch::run_with_nop_fallback` is a separate opt-in entry;
the existing strict `run` is unchanged and reports `Unsupported` at bare
`0x90`. The opt-in repeatedly invokes strict dispatch using the remaining total
budget and intercepts only an `Unsupported` result at the current RIP whose
single-byte view decodes as exact unprefixed primary-map `Operation::Nop`
(`90`, length 1, no prefixes). It calls the existing
`od_isa::execute::step` on `CodeMemory::one_instruction`; only `Advanced` with
RIP advancing by one retires. Prefix NOP, `0F 1F`, PAUSE and every other
unsupported/invalid form remain terminal strict results.

The returned `BoundedMemoryFallbackResult` reports `retired_native` and
`retired_interpreter` separately; after each native segment and interpreter
step their sum is checked against `total_budget - remaining_budget`. STOP-RIP
and cooperative STOP are sampled by strict dispatch before reporting the NOP
and again before stepping; the next strict iteration checks them before
compiling/publishing the next memory artifact. A budget exit after NOP returns
the actual next RIP, so later continuation does not repeat the committed pair.
The `MemoryBlockExecutor` contract requires executable mapping release before
callback return. `PublishedMemoryBlockExecutor` explicitly drops the published
mapping after result decoding and before returning to the dispatcher, which
then steps NOP. This remains an unsafe caller-guaranteed private synchronous RAM
and fixture byte-window API, not a production owner integration or general
Stage 2 fallback.

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

The existing generic cached fixture-dispatch and multi-entry APIs require a
caller-owned, nonzero `ExecutionContextId` generation and an explicit
`ExecutionProfile`. Those APIs admit only `S0_09Fixture + Long64 + canonical48`;
other modes, canonical widths, or semantic profiles are rejected before cache
binding, compilation, publication, or execution. The separate S3-38b
`run_cached_native_register_v1_controlled` API admits only
`NativeRegisterV1Fixture + Long64 + canonical48` in its one-entry
`SingleEntryBlockCache`. That profile is not admitted by the generic dispatcher
or the multi-entry cache. `TranslationKey` includes the context stamp/profile
in addition to existing mapping/backing identities, versions, and executable
permissions. No second MMU identity layer is added.

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

### S3-23b: memory-pair fixture cache invalidation

`MemoryPairCache::invalidate` accepts the existing `CodeInvalidation` owner
notification and uses the same dependency matcher as the register-only
single-entry and multi-entry caches. A matching inactive resident is removed
and its published handle is dropped. Backing-content changes match by backing
identity/version across aliases; mapping replacement/removal and
execute-permission changes match by address-space and mapping identity.
The notification API is callable only between synchronous calls because it
requires mutable access to the cache. After invalidation, the caller provides
the updated dependency snapshot to the next lookup.

Local tests cover each notification variant, unrelated backing preservation,
backing-alias matching, drop, and replacement. An exact macOS ARM64 `od-host`
acceptance test publishes and executes the memory pair, invalidates it after
return, then publishes again from a changed dependency version with fresh CPU
state and fresh RAM. Successful republish verifies that the old single-live
MAP_JIT mapping was released first.

This connects no production owner or MMU and provides no active SMC retirement,
concurrency, DMA, or device-writer behavior. S3-23, S3-24, and S3-25 remain
open. See the [S3-23b report](../reports/STAGE_3_S3-23b_2026-11-03.md).

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
bounded fixture scenario; it does not replace the architectural
store/control-transfer requirement for general self-modifying code. The
initial S3-24a implementation rejected all future-tail overlap; S3-24b adds
only the exact immediate replacement described below. General blocks,
production MMU/writer integration, and complete S3-24 remain open. See the
[S3-24a report](../reports/STAGE_3_S3-24a_2026-10-27.md).

### S3-24b: exact not-yet-executed tail immediate invalidation

The exact V3 artifact remains the 23-byte sequence
`48 BA imm64; 48 89 03; 48 B9 imm64`. A fixture owner may commit one qword
store only when the resolved write plan is a single segment covering exactly
the final `MOV RCX,imm64` immediate at artifact offsets `[15,23)`. The opcode
and instruction length remain unchanged. Admission also requires the complete
23-byte source lease to match the backing before generated entry, the expected
V3 opcode profile, and a target with the same backing identity and backing
offset as the leased tail. Direct and aliased guest addresses therefore share
one versioned backing notification. The qword must not be split across guest
mapping segments, even when both segments name contiguous bytes of one backing.

The owner sets the existing invocation-local dirty flag on that exact staged
write plan before mutation. A successful checked write commits one store,
advances RIP to `entry+13`, retires the prefix plus store, and exits
`CodeInvalidated` before the old native tail can execute. After return and
result decoding, the owner advances the backing generation and emits its
notification while the V3 handle remains live. The dispatcher invalidates all
cache dependencies on that backing, including direct and alias entries, but
keeps unrelated backing entries resident. A later dispatch fetches the updated
immediate and executes it once. Failure before commit leaves bytes, generation,
and dirty outcome unchanged; STOP raised by a successful store does not
suppress invalidation.

This is fixture-level coherence and redispatch evidence for one decoded tail
instruction. It is not a physical x86 SMC oracle and does not establish
general instruction rewriting, production MMU/writer integration, arbitrary
mapping spans, or complete S3-24. See the
[S3-24b report](../reports/STAGE_3_S3-24b_2026-10-29.md).

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

### S3-27b: next-instruction fetch fault after a two-site memory pair — fixture partial

After either exact two-site artifact retires both guest instructions, its
returned CPU state is at `A+6` and its memory effects are complete. A subsequent
interpreter step uses that returned state and RAM directly. A fixture supplies
a valid fetch mapping at `A+6` whose `GuestMemory::fetch` returns a chosen
architectural fault. Interpreter-only `execute::run` over the pair plus the
faulting third fetch reports that fault with `instructions_executed == 2`; the
run from native-returned state reports the same payload with zero additional
instructions. Full CPU state/RAM match and retries do not repeat either pair
memory callback. Unmapped/backend fetch errors for both pair sequences remain
host `MemoryError`s with zero continuation instructions. `RunConfig` with zero
budget and a raised STOP closure suppresses the fetch; replenishing budget or
clearing STOP then exposes the architectural fault with zero continuation
instructions. Compiler `CodeView` exhaustion is not reclassified as a guest
fetch fault.

This is bounded fixture-boundary evidence, not a production dispatcher or
owner-backed MMU integration. Production cross-block fetch/fault delivery and
S3-27's general PC-map coverage remain open. See the
[S3-27b report](../reports/STAGE_3_S3-27b_2026-10-27.md).

### S3-27c: diagnostic host-PC attribution for the exact V3 fixture

The exact three-instruction `ActiveCodeBlockV3` carries a validated `PcMap`
whose byte ranges cover the emitted ARM64 words contiguously. Boundaries record
the committed guest RIPs `A+10`, `A+13`, and `A+23` after 1, 2, and 3 retired
instructions; map instruction indices are zero-based (`0`, `1`, `2`). Canonical state writes and retirement accounting are marked
`CommitInProgress`; the checked helper and post-write/pre-commit ranges are
marked inadmissible after an effect. Helper failure is attributed to store RIP
`A+10`, while its typed helper outcome distinguishes architectural faults from
backend failures. CodeInvalidated and its stub are fixed to `A+13`; Complete and
STOP-after-tail are fixed to `A+23`. Shared stop-RIP, STOP, and budget stubs use
canonical state. This map is diagnostic attribution only: it does not install
host-fault recovery or authorize replay after an effect.

The map is cloned into the published arena slot metadata and remains available
after compiler artifact drop and cache invalidation while that generation is
live. Generation reuse rejects an old handle. This bounded exact-source fixture
does not map direct V2 blocks, provide owner/MMU integration, or close S3-27.
See the [S3-27c report](../reports/STAGE_3_S3-27c_2026-10-30.md).

### S3-27d: diagnostic host-PC attribution for direct singleton V2 memory blocks

Direct `MemoryLoadBlockV2` and `MemoryStoreBlockV2` artifacts own a validated
ARM64 `PcMap` separately from the shared `MemoryArtifactV2`, which remains
shared with the typed-IR compiler. The map covers every emitted byte in
contiguous aligned ranges and records one committed boundary at the next guest
RIP. The direct emitter captures byte offsets from checked encoder word
positions; it does not alter code emission, admission, labels, order, or ABI.

Entry guards map to the fixed guest RIP. The checked load helper-call range is
classified `ReplayCurrentInstruction` as generated-state attribution only.
Replay additionally requires the caller to establish synchronous ordinary
RAM independently; the safe direct load API accepts arbitrary `GuestMemory`,
so this map never authorizes replay for arbitrary implementations, MMIO, or
unknown memory classes. No host-fault recovery is installed. A store
helper-call range and its failure path are non-replayable after the possible
write. Commit writes are inadmissible until the committed boundary. The normal
return and STOP-after-commit use the fixed next RIP; pre-entry exits use the
fixed entry RIP, while stop-address, budget, and helper-failure stubs have
their explicit ranges and RIP sources. Helper guest/backend results remain
distinct in the existing execution result contract.

`PublishedCode::pc_map()` and a generation-checked
`PublishedArenaMemoryStore::pc_map()` expose owned copies retained after the
compiler artifact is dropped. Arena metadata remains attached to the store
generation across notification/cache invalidation; a handle from a reused
generation is rejected. The existing `MemoryOperationMetadataV2` remains a
`Copy` execution descriptor, and shared-IR/V3 maps remain unchanged. This
diagnostic mapping does not add host-fault recovery, physical/MMU/unknown
memory classes, Stage2 integration, Mini/performance claims, or close full
S3-27/Stage3. See the
[S3-27d report](../reports/STAGE_3_S3-27d_2026-10-30.md).

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

#### S3-20z: guest RSP as an exact qword MOV data operand

Admit only `48 8B/89 /r` with ModRM `mod=00`, `reg=4` (guest RSP data), and
`r/m=0..3` (direct base RAX..RBX), exact length three. This exception is
scoped to checked memory data: generic GPR reads/writes remain RAX..RBX, and a
load may stage/write guest RSP only after its matching successful checked
load. No SIB, displacement, address-size override, prefix, other base or pair
profile is included. Decoded `MemoryOperand` remains the sole address
semantics; full-width noncanonical data is not itself an address fault.

At this slice the private IR frame staged the five admitted data-state GPRs
at `SP+56..95`, flags at `SP+96`, RIP at `SP+104`, and saved X19/X20 and
X21/X22 at `SP+112` and `SP+128`; its minimum prefix was 144 bytes. S3-20aa
extends this private staging area uniformly to all six GPRs; see below. This
frame is not the host SP and does not change the public runtime/C ABI. A sequential
load to guest RSP commits only on successful memory read; a following use of
that RSP as an address then receives the normal architectural fault at its own
RIP without replaying the prior effect. See the
[S3-20z report](../reports/STAGE_3_S3-20z_2026-10-27.md).

#### S3-20aa: guest RBP as an exact qword MOV data operand

Admit only three-byte `48 8B/89 /r`, ModRM `mod=00`, data RBP (`reg=5`) and
direct base RAX..RBX (`r/m=0..3`). RBP is allowed only as this exact checked
memory data register; generic GPR read/write admission, pairs, and V3 remain
unchanged. A successful load commits RBP only after the read succeeds. Stores
transmit the full 64-bit RBP payload even when that value is noncanonical;
address canonicality continues to come from the decoder's memory helper.

The private frame uniformly stages six GPRs RAX..RBP at `SP+56..103`, flags at
`SP+104`, RIP at `SP+112`, and X19/X20 and X21/X22 save pairs at `SP+128` and
`SP+144`. The minimum reserved prefix is 160 bytes; allocator spills remain
beyond it. RuntimeContext and C ABI layouts are unchanged. See the
[S3-20aa report](../reports/STAGE_3_S3-20aa_2026-10-27.md).

#### S3-20ab: RBP as qword MOV data and signed disp8 base

Admit only exact singleton `48 8B/89 6D ib`: ModRM `mod=01`, `reg=RBP`,
`r/m=RBP`, signed disp8, qword data, 64-bit effective address and four source
bytes. The existing decoder `MemoryOperand` and `access_address_48` remain the
sole source of address semantics. Generic GPR validation is unchanged, and no
disp32, SIB, prefix, other register combination, pair, V3 or frame-layout
change is included. Existing RBP-data-only, RBP-base-only and RSP-data forms
remain exact separate profiles.

For `MOV RBP,[RBP+disp8]`, address calculation reads entry RBP and commits the
loaded destination only after a successful access. For `MOV [RBP+disp8],RBP`,
both effective address and full qword payload come from old RBP. Direct V2,
shared IR, interpreter and bounded-dispatch tests cover signed displacement
boundaries and complete state/RAM/callback/retirement parity. A current
noncanonical address faults as #SS without callback or architectural commit.
After a successful load of a noncanonical RBP, a later RBP-based instruction
faults at its own RIP while retaining the earlier commit; budget and callback
STOP continuation do not repeat the first access. See the
[S3-20ab report](../reports/STAGE_3_S3-20ab_2026-10-27.md).

#### S3-20ac: RSP as qword MOV data and exact SIB base

Admit only singleton `48 8B/89 24 24`: ModRM `mod=00/rm=100`, reg/data RSP,
SIB exactly `0x24`, RSP base, no index, scale 1, no displacement, qword data,
64-bit effective address and length four. `MemoryOperand` and
`access_address_48` remain the only address semantics. Generic GPR validation,
frame layout, and public C ABI do not change. Other SIB values, displacement,
prefix, register combination, pair and V3 forms remain excluded; existing
RSP-data-only and RSP-base forms remain separate.

For `MOV RSP,[RSP]`, the address uses entry RSP; successful read commits a
distinct loaded value only afterward. `MOV [RSP],RSP` writes the full old RSP
payload to the address based on that old value. Direct V2, shared IR,
interpreter and dispatcher parity cover CPU/RAM/callback/retirement. A current
noncanonical RSP faults as #SS before callback and commits nothing. After
loading a noncanonical value, a subsequent `[RSP]` instruction faults at its
own RIP while preserving the prior commit. Budget and callback STOP resume
without replay. The ARM64/macOS x19 canary covers success, guard and fault
returns. See the
[S3-20ac report](../reports/STAGE_3_S3-20ac_2026-10-27.md).

#### S3-20ad: RSP as qword MOV data and signed-disp8 SIB base

Admit only exact five-byte singleton `48 8B/89 64 24 ib`: ModRM `mod=01`,
reg/data RSP, r/m SIB, SIB exactly `0x24`, signed disp8, qword data and 64-bit
effective address. The decoded `MemoryOperand` and `access_address_48` remain
the sole address semantics. Generic GPR validation and frame/public ABI remain
unchanged; no other SIB, displacement width, prefix, pair or V3 form is added.

Address and store payload use entry RSP. Successful load commits RSP only
after the read succeeds; a later instruction observes the committed value.
Architectural faults leave the instruction uncommitted, address overflow and
backend errors retain their existing classification, and a successful store
remains committed if STOP is requested after its callback. Budget and STOP
continuation resumes from actual RIP without replay. The bounded dispatcher
test loads a noncanonical RSP through `[RSP-1]`, commits the first instruction,
then faults #SS on the next `[RSP]` without repeating the first access. Partial
source windows and non-profile forms stop before data access. See the
[S3-20ad report](../reports/STAGE_3_S3-20ad_2026-10-27.md).

#### S3-20ae: RSP as qword MOV data and signed-disp32 SIB base

Admit only exact eight-byte singleton `48 8B/89 A4 24 id`: ModRM `mod=10`,
reg/data RSP, r/m SIB, SIB exactly `0x24`, signed disp32 in bytes 4..7, qword
data and 64-bit effective address. The decoder's `MemoryOperand` and
`access_address_48` remain the sole address semantics. Generic GPR validation,
frame layout, source storage, public ABI, pair and V3 profiles do not change.

The effective address and store payload use entry RSP. A successful load
commits the loaded RSP only after the read; subsequent instructions use that
committed value. Architectural faults preserve the instruction state and
retirement; address-overflow and backend errors remain host errors. STOP after
a successful store leaves its full 64-bit payload committed. The dispatcher
continues through the bare NOP bridge into the existing pair with exact
native/interpreter retirement accounting and actual-RIP continuation. Partial
source windows and non-profile forms stop before data effects. See the
[S3-20ae report](../reports/STAGE_3_S3-20ae_2026-10-27.md).

#### S3-20af: RBP as both qword MOV data and signed-disp32 base

Admit only exact seven-byte singleton `48 8B/89 AD id`: ModRM
`mod=10/reg=5/rm=5`, signed disp32 bytes 3..6, qword data and 64-bit EA.
The decoded `MemoryOperand` and `access_address_48` remain the only address
authority; the exact-data exception does not widen generic GPR predicates.
The mod00/rm5 RIP-relative form remains distinct and computes from this
instruction's NextRip. Pair, V3, frame layout and source storage are unchanged.

Direct V2/shared IR/interpreter checks compare both operations and disp32
MIN/-1/0/MAX against full state and RAM, callback address/count, and retirement.
The address and store payload use entry RBP; load commits the returned value
only after a successful access. Descriptor checks verify byte-six identity,
decoder-derived displacement, RIP-next-RIP differentiation, exact instruction
length, and reject partial windows 1..6 and malformed SIB/prefix variants.
Actual decoded RBP-relative and NextRip operands are passed through
`access_address_48`; both results are compared with independent formulas and
asserted distinct. Host callbacks for byte-six values 1 and 2 target
`0x01000040` and `0x02000040` from initial RBP 64.
Bounded dispatcher continuation commits the first RBP load at its actual next
RIP; a subsequent RBP-relative access to the loaded noncanonical value faults
#SS without repeating the first read. Existing RBP-data-only, RBP-base-only,
RSP-data and prior dual-register profiles remain covered. Exact-profile checks
also preserve AddressOverflow and backend-error classification, and the
macOS/ARM64 x19 canary covers success, zero budget and fault. A mixed
singleton→NOP→pair sequence checks separate native/interpreter/native counts
1/1/2 with actual-RIP continuation and no repeated first read. See the
[S3-20af report](../reports/STAGE_3_S3-20af_2026-10-27.md).

#### S3-20ag: RBP data with the exact RSP-base SIB

Admit only exact four-byte singleton `48 8B/89 2C 24`: ModRM
`mod=00/reg=5/rm=4`, SIB `0x24` (RSP base, no index, scale 1, no
displacement), qword data and 64-bit EA. Decoded `MemoryOperand` remains the
only effective-address source. The exception is exact; generic register
predicates, other RBP-data SIB forms, prefixes, displacement variants, pairs,
V3 and frame/ABI are unchanged.

Direct V2/shared IR/interpreter parity compares complete CPU/RAM state,
callback and retirement for both operations with distinct RSP/RBP values.
Load writes RBP only after success; store uses all 64 bits of old RBP at the
old-RSP address. Noncanonical RSP produces #SS without a callback, while
noncanonical RBP remains valid store data for canonical RSP. Store callback
STOP preserves the write. Dispatcher continuation after `MOV RBP,[RSP]`
uses the newly loaded RBP in the next instruction, resumes from actual RIP,
and does not repeat the first read. Existing RSP/RBP data profiles and canary
lifecycle tests remain green. See the
[S3-20ag report](../reports/STAGE_3_S3-20ag_2026-10-27.md).

#### S3-20ah: RBP data with signed RSP-base SIB displacement

The exact added singleton forms are `48 8B/89 6C 24 ib` (signed disp8,
length 5) and `48 8B/89 AC 24 id` (signed disp32, length 8). ModRM is
`mod=01/10`, `reg=5`, `rm=4`; SIB is exactly `0x24`, selecting RSP as base,
with no index and scale 1. The decoded `MemoryOperand` remains the sole
effective-address authority. This exact data-register exception does not
widen generic GPR validation or alter pair/V3 profiles, source storage, frame
layout or ABI.

Both direct V2 and shared IR are compared with the interpreter for load and
store. The address uses entry RSP plus signed displacement; store data uses
all 64 bits of entry RBP; a load updates only RBP after success. Faults retain
the exact #SS payload and make no data callback, while address overflow and
backend failures remain host errors. A successful store callback STOP leaves
the write committed. Bounded dispatch continues by actual RIP, including
singleton→NOP→pair and a subsequent RBP-relative access through the newly
loaded RBP, with no replay. A fixture byte-window ending inside an instruction
returns `BoundaryReached` before publication or memory effects; it is not a
guest fetch fault. See the
[S3-20ah report](../reports/STAGE_3_S3-20ah_2026-10-27.md).

#### S3-20ai: RSP data with an RBP-base direct displacement

The added exact singleton spellings are `48 8B/89 65 ib` (ModRM
`mod=01/reg=4/rm=5`, signed disp8 at byte 3, length 4) and `48 8B/89 A5 id`
(`mod=10/reg=4/rm=5`, signed disp32 at bytes 3..6, length 7). The effective
address is computed only by the decoder's `MemoryOperand`; this does not
widen generic GPR predicates or change SIB, pair, V3, frame or ABI profiles.
The exact ModRM distinguishes the forms from RBP-data encodings. The
neighboring `mod=00/rm=5` form remains RIP-relative and uses instruction-local
NextRip.

Both operations are compared through direct V2, shared IR and interpreter.
The address uses entry RBP plus signed displacement; load writes RSP only
after a successful access, while store writes all 64 bits of old RSP. A
noncanonical RSP value is valid data, while noncanonical RBP as EA returns
exact #SS before callback and preserves state/accounting. Successful store
STOP retains the write. Dispatcher continuation from actual RIP uses the
loaded RSP in a following `[RSP]`, with budget split/STOP and no replay. The
disp32 byte-six host check ties differing source bytes to independently
expected addresses. See the
[S3-20ai report](../reports/STAGE_3_S3-20ai_2026-10-27.md).

#### S3-20aj: RSP/RBP data with a low-GPR direct displacement

The exact singleton forms are `48 8B/89 /r disp8` (length 4) and
`48 8B/89 /r disp32` (length 7), with ModRM `mod=01/10`, data register RSP or
RBP (`reg=4/5`), and base RAX..RBX (`rm=0..3`). SIB, address/prefix variants,
other bases/data registers, pairs and V3 are not admitted. The decoder's
`MemoryOperand` remains the effective-address authority; shared IR and runtime
metadata must bind the exact bytes and decoded operand without broadening
generic GPR validation.

Loads update only the selected stack data register after a successful read;
stores use the old data-register value and commit before a callback STOP.
Noncanonical values are valid data, while using one later as an RSP/RBP base
produces #SS at that next instruction. Noncanonical low-GPR effective
addresses produce #GP before a backend callback. The bounded fixture dispatcher
continues from actual RIP through budget/STOP splits and the opt-in bare-NOP
interpreter step, without repeating effects. Partial byte windows return the
fixture boundary result without publication or memory access. Existing
RBP-base, RSP-base and RIP-relative spellings remain distinct.

See [S3-20aj evidence](../reports/STAGE_3_S3-20aj_2026-10-27.md).

#### S3-20ak: RIP-relative qword MOV with RSP/RBP data

The bounded shared-IR profile admits only singleton
`48 8B/89 25 disp32` (RSP data) and `48 8B/89 2D disp32` (RBP data), exact
length seven. ModRM is `mod=00,r/m=101`; SIB, prefixes, other data registers,
pairs and V3 remain unsupported. The decoder's `MemoryOperand` and
instruction-local `NextRip` continue to define the effective address.

Direct V2, shared IR and interpreter agree on load/store state, RAM callbacks
and retirement for a CodeView interior entry. Exact-profile noncanonical
NextRip targets produce #GP before a memory callback. Loaded noncanonical
stack-register values are valid data; if a later instruction uses that value
as RSP/RBP base, #SS occurs on that instruction while the preceding load
remains committed. The bounded dispatcher composes a bare NOP, either exact
stack-data singleton and two following singleton memory operations, checks
budgets 0..4, and resumes from the actual RIP without repeating a callback. See
[S3-20ak evidence](../reports/STAGE_3_S3-20ak_2026-10-27.md).

#### S3-20al: stack-register data through indexed 64-bit SIB

Shared IR, direct V2 and bounded dispatch admit RSP/RBP as the qword MOV data
register only for existing indexed SIB forms with ModRM `r/m=100`, base and
index RAX..RBX, and scale 1/2/4/8. The exact lengths are four bytes for no
displacement, five for signed disp8, and eight for signed disp32; data register
is RSP or RBP. The decoder's `MemoryOperand` and `access_address_48` remain the
only effective-address semantics. No address-size override, special
no-index/no-base SIB, extra prefix, pair or V3 form is added. Stack-register
loads commit only after a successful access; stores consume the old full
64-bit value. Noncanonical payloads are permitted as data, while later use as
an RSP/RBP memory base faults at that later instruction. See the
[S3-20al evidence](../reports/STAGE_3_S3-20al_2026-10-27.md).

#### S3-20am: address32 qword MOV with RSP/RBP data

Direct V2, shared IR and bounded dispatch admit exact singleton
`67 48 8B/89 /r [disp8|disp32]` forms with ModRM `mod=00/01/10`, data
register RSP or RBP, and direct base RAX..RBX. Address size is Dword while
operand size remains Qword; lengths are 4/5/8 and displacement is zero, signed
disp8 or signed disp32 respectively. SIB, RIP-relative and extra-prefix forms
are excluded. The decoded `MemoryOperand` remains authoritative for low-32-bit
wrapping EA semantics. Data transfer and stack-register commits remain full
64-bit. A qword access beginning at `0xffff_fffc` is one contiguous access
through `0x1_0000_0003`; upper base bits do not participate in effective-address
calculation. See [S3-20am evidence](../reports/STAGE_3_S3-20am_2026-10-27.md).

#### S3-20an: address32 indexed-SIB qword MOV with RSP/RBP data

The added singleton forms are exactly `67 48 8B/89 /r sib` with optional
disp8 or disp32: ModRM `mod=00/01/10,r/m=100`, data RSP or RBP, SIB base and
index RAX..RBX, scale 1/2/4/8. Address size is Dword, data size Qword, and
instruction lengths are 5/6/9 bytes. No-index/no-base SIB specials, RSP/RBP
base or index, additional prefixes, pair and V3 forms remain unsupported.
Decoder `MemoryOperand` plus `access_address_48` remain the only EA semantics;
low-32-bit base/index/displacement arithmetic does not truncate the qword data.
Direct V2, shared IR and bounded dispatch use the same profile. A stack-data
load commits only on success; later noncanonical RSP/RBP stack-base uses raise
#SS after that first retirement. The [S3-20an report](../reports/STAGE_3_S3-20an_2026-10-27.md)
records the current fixture evidence and remaining owner/MMU limitation.

#### S3-20ao: address32 no-index SIB qword MOV with stack data

Admit only singleton `67 48 8B/89 /r 24 [disp8|disp32]` with ModRM
`mod=00/01/10,r/m=100` and SIB exactly `0x24`. The old guest RSP supplies
the low-32-bit effective-address base, with no index and scale one. The data
register may be RAX through RBP. Instruction lengths are 5/6/9 bytes, address
size is Dword, and transfer size is Qword. Direct V2, shared IR and bounded
dispatch use the same exact byte/decoded-operand profile; other SIB encodings,
prefixes, pairs and V3 stay outside this slice. The decoder's `MemoryOperand`
and `access_address_48` remain the sole authority for address calculation.
Full qword values, including RSP data, are preserved independently of the
low-32-bit address calculation. See
[S3-20ao evidence](../reports/STAGE_3_S3-20ao_2026-10-27.md).

#### S3-20ap: address32 direct EBP-base qword MOV with stack data

Admit only singleton `67 48 8B/89 /r disp8|disp32` forms with ModRM
`mod=01/10,r/m=101`, data RAX..RBP, Dword address size and Qword transfer
size. Lengths are 5/8 bytes; the old guest RBP low 32 bits supply the base.
`mod=00/rm=101` is not part of this form, and no SIB, extra prefix, pair or V3
profile is added. Existing decoded `MemoryOperand` and `access_address_48`
remain the sole EA semantics. Data remains full-width even when the data and
base register alias. See
[S3-20ap evidence](../reports/STAGE_3_S3-20ap_2026-10-27.md).

#### S3-20aq: indexed SIB with RSP base and stack-register data

Admit only singleton 64-bit-address-size `48 8B/89 /r sib
[disp8|disp32]` forms with ModRM `mod=00/01/10,r/m=100`, SIB base RSP,
index RAX..RBX, scale 1/2/4/8 and data RAX..RBP. Lengths are 4/5/8 bytes;
transfer width stays Qword. The existing no-index SIB `0x24` remains a
separate profile. No address-size override, other base/index, pair or V3 form
is added. `MemoryOperand` and `access_address_48` remain the sole EA semantics.
The base and index are read from the instruction's pre-state before any load
destination commit; stores use the old data register value. See
[S3-20aq evidence](../reports/STAGE_3_S3-20aq_2026-10-27.md).

#### S3-20ar: indexed SIB with RBP base and stack-register data

Admit only singleton 64-bit-address-size `48 8B/89 /r sib disp8|disp32`
forms with ModRM `mod=01/10,r/m=100`, SIB base RBP, index RAX..RBX, scale
1/2/4/8 and data RAX..RBP. Instruction lengths are 5/8 bytes; transfer width
is Qword. `mod=00,SIB.base=101` decodes as a no-base operand and remains
unsupported. The existing indexed RSP-base and no-index SIB profiles remain
distinct. `MemoryOperand` and `access_address_48` remain the sole EA semantics.
Base, index and store data are read from the instruction pre-state before any
destination commit. See [S3-20ar evidence](../reports/STAGE_3_S3-20ar_2026-10-27.md).

#### S3-20as: address32 indexed SIB with RSP base

Admit singleton `67 48 8B/89 /r sib [disp8|disp32]` forms with ModRM
`mod=00/01/10,r/m=100`, SIB base RSP, index RAX..RBX, scale 1/2/4/8 and data
RAX..RBP. Address size is Dword while transfer size remains Qword; lengths are
5/6/9 bytes. The existing `SIB=0x24` no-index profile remains separate. No
no-base, RSP/RBP index, extra-prefix, pair or V3 form is admitted.

The decoded `MemoryOperand` and `access_address_48` remain the only source of
effective-address semantics: truncate RSP/index inputs to 32 bits, scale/add
with address-size wrapping, then perform one contiguous qword access. The
load/store source and address inputs use the pre-instruction state. Publication
and the borrowed bounded dispatcher validate exact source bytes and decoded
metadata before execution. See
[S3-20as evidence](../reports/STAGE_3_S3-20as_2026-10-27.md).

#### S3-20at: address32 indexed SIB with RBP base

The bounded singleton profile admits exactly `67 48 8B/89 /r sib disp8|disp32`, with ModRM `mod=01/10,r/m=100`, SIB base RBP, index RAX..RBX, scale 1/2/4/8, and data register RAX..RBP. Address size is Dword and memory data size is Qword; the exact instruction lengths are 6 and 9 bytes. The decoder-owned `MemoryOperand` and `access_address_48` define signed displacement, scaling, truncation, wrapping, and the final contiguous qword range. Host/source validation binds all bytes, decoded base/index/scale, operation, size, and instruction length. ModRM `mod=00` with SIB base field 5 remains the no-base encoding and is rejected. No-index, additional-prefix, pair, and V3 admission is unchanged.

Descriptor and host tests cover all six data registers, all low index registers and scale values, both directions, signed disp8/disp32 edges, data=RBP/index and data=RBP/base aliases from pre-instruction state, high-half truncation, ninth-byte displacement identity, contiguous access across 4 GiB, fault/backend classification and no guest commit, store+STOP commit, dispatcher budgets and resume without replay, partial windows, and no-base rejection. See the [S3-20at evidence](../reports/STAGE_3_S3-20at_2026-10-27.md). The evidence remains borrowed-fixture coverage; production memory-owner/MMU integration, physical x86, Mini, and performance are open.

#### S3-20au: absolute qword access through no-base/no-index SIB

Admit only singleton `48 8B/89 /r 25 disp32`: ModRM `mod=00,r/m=100`, exact SIB `0x25`, data RAX..RBP, and length eight. The address is the sign-extended disp32 in 64-bit address mode. The source-profile matcher verifies the opcode, ModRM fields, exact SIB, data register, and decoded instruction length. The displacement comes from the decoder-produced `MemoryOperand` retained in the immutable compiled artifact; the runtime matcher does not re-decode or byte-compare the raw disp32. `MemoryOperand` and `access_address_48` remain the sole effective-address semantics. Indexed no-base, address32, other SIB encodings, moffs, pairs, and V3 remain unsupported.

Direct V2, shared IR, interpreter, and borrowed dispatcher evidence covers both directions and all six data registers; GPR/RIP-independent target calculation; contiguous qword at `-8`; pre-helper overflow for `-1`; architectural/backend no-commit behavior; store+STOP; byte-eight identity; and distinction from RIP-relative addressing. The byte-eight runtime case compiles two immutable decoder-built artifacts whose final disp32 byte differs, then verifies each artifact's independently expected callback target. Partial byte windows return fixture `BoundaryReached` without effects, while a complete address32 neighbor is Unsupported. See the [S3-20au evidence](../reports/STAGE_3_S3-20au_2026-10-27.md). Production owner/MMU integration, physical x86, Mini, and performance remain open.

#### S3-20av: address32 absolute qword access through no-base/no-index SIB

Admit only singleton `67 48 8B/89 /r 25 disp32`: ModRM `mod=00,r/m=100`, exact SIB `0x25`, data RAX..RBP, length nine, Dword address size and Qword data size. The decoded `MemoryOperand`/`access_address_48` computes the low-32-bit absolute target; the qword transfer is then one contiguous 64-bit callback and does not wrap each byte at 4 GiB. Runtime profile validation checks the fixed opcode/ModRM/SIB shape and takes displacement from the immutable decoder-built artifact; it does not re-decode raw displacement bytes. The 64-bit absolute 20au profile remains admitted. Indexed no-base, direct `rm=101`, extra prefixes, moffs, pairs, and V3 remain unsupported.

Direct V2/shared IR/interpreter and borrowed-dispatcher evidence covers both directions and all six data registers, signed displacement edges, GPR/RIP-independent low-32-bit target, qword access across 4 GiB, precise fault/backend no-commit behavior, committed store+STOP, byte-nine identity across independently compiled immutable artifacts, and budget/actual-RIP continuation. Partial windows return `BoundaryReached`; complete invalid neighbors remain Unsupported. See the [S3-20av report](../reports/STAGE_3_S3-20av_2026-10-27.md). Production owner/MMU integration, physical x86, Mini, and performance remain open.

#### S3-20aw: indexed no-base SIB qword access in 64-bit address mode

Admit only singleton `48 8B/89 /r sib disp32`: ModRM `mod=00,r/m=100`, SIB base field `101` (no base), index RAX..RBX, scale 1/2/4/8, data register RAX..RBP, and length eight. The no-index SIB `0x25` remains a separately matched 20au form. The decoded `MemoryOperand` and `access_address_48` are the sole EA semantics: full-width index scaling and signed disp32 addition wrap modulo `2^64`, followed by canonical/full-qword-range validation. Runtime matching binds fixed opcode/ModRM/SIB identity and compares decoded index, scale, width, and length; displacement is carried by the immutable decoder-built artifact and is not re-decoded from raw bytes.

Direct V2, shared IR, interpreter, and bounded dispatcher evidence covers load/store × six data registers × four index registers × four scales × signed disp32 boundaries, including data=index pre-state behavior, independently computed/helper targets, u64 wrap, high displacement-byte identity, noncanonical address #GP before callback with stack data registers, AddressOverflow, architectural and backend faults without commit, successful store+STOP, budget/actual-RIP resume without replay, partial windows, and rejected neighboring SIB shapes. See [S3-20aw report](../reports/STAGE_3_S3-20aw_2026-10-27.md). Production owner/MMU integration, physical x86, Mini, and performance remain open.

#### S3-20ax: indexed no-base SIB qword access in address32 mode

Admit only singleton `67 48 8B/89 /r sib disp32`: ModRM `mod=00,r/m=100`, SIB base field `101` (no base), index RAX..RBX, scale 1/2/4/8, data RAX..RBP, and length nine. Transfer width is Qword; effective-address size is Dword. The decoded `MemoryOperand` and `access_address_48` remain the sole source of EA semantics: `(low32(index) * scale + signed_disp32) mod 2^32`, zero-extended to 64 bits before one contiguous qword access. The separate 64-bit indexed no-base 20aw form and 20av no-index SIB `0x25` remain unchanged. Runtime matching checks the exact prefix/opcode/ModRM/SIB shape and decoded metadata from the immutable decoder-built artifact; it does not re-decode displacement bytes.

Evidence compares direct V2, shared IR, interpreter, and borrowed dispatcher for both operations, six data registers, four indices, scales, and signed disp32 edges. It includes independent low32 EA formulas with nonzero index high halves, data=index pre-state, ninth-byte artifact identity, qword crossing at `0xffff_ffff`, exact callback identity and retirement, guest/backend no-commit, store+STOP, budgets and actual-RIP continuation without replay, partial windows 1..8, and rejected SIB neighbors. Production owner/MMU integration, Stage 2/ISA, Mini, physical x86, and performance remain open.

#### S3-20ay: qword SIB access with RBP index

Admit only singleton `48 8B/89 /r sib`: ModRM `mod=00,r/m=100`, base RAX..RBX, index RBP, scale 1/2/4/8, data RAX..RBP, exact length four. This is a narrow separate index=RBP branch; generic index admission stays RAX..RBX. SIB index field `100` with `REX.X=0` remains no-index and never aliases RBP. Other SIB combinations, displacement/prefix forms, and special base encodings are not admitted by this branch; previously bounded no-index, RSP-base, RBP-base, and no-base profiles keep their separate validators. The decoded `MemoryOperand` and `access_address_48` alone compute `base + RBP*scale` with u64 wrapping and canonical/range checks.

Descriptor and host evidence cover load/store × six data registers × four low bases × four scales, with independent target formulas, high RBP bits, `data=RBP`/`data=base` pre-state aliases, full qword stores, and direct V2/shared IR/interpreter state/RAM/callback/retirement parity. Fault coverage includes #GP for noncanonical start/end before callback, qword endpoint AddressOverflow, backend error classification and callback target, and committed store+STOP. The dispatcher resumes from the actual RIP after a budget split without repeating the SIB access. Partial windows and disallowed RSP/no-base bases with RBP index, RSI/RDI index, displacement, and prefix forms are rejected. The existing no-index SIB `0x24` and no-base `0x2d` profiles remain separately gated. See [S3-20ay report](../reports/STAGE_3_S3-20ay_2026-10-27.md). Production owner/MMU integration, Stage 2/ISA, Mini, physical x86, and performance remain open.

#### S3-20az: qword SIB access with RBP index and signed displacement

Admit only singleton `48 8B/89 /r sib disp8|disp32` with ModRM `mod=01/10,r/m=100`, base RAX..RBX, index RBP, scale 1/2/4/8, data RAX..RBP, and exact lengths 5/8. This branch is separate from no-displacement S3-20ay; the generic index whitelist remains unchanged. No address-size override, RSP/RBP/no-base base, other index, pair, or V3 shape is added. The decoder's `MemoryOperand` and `access_address_48` alone compute the full-width `base + RBP*scale + signed displacement` EA with u64 wrapping. Runtime validates the fixed source profile and decoded operand metadata carried by the immutable artifact.

Descriptor and host evidence cover load/store × six data registers × four bases × four scales × signed disp8/disp32 boundaries. It checks an independent EA formula, high RBP bits, `data=RBP`/`data=base` pre-state aliases, full qword payloads, and direct V2/shared IR/interpreter state/RAM/callback/retirement parity. Noncanonical start/end are #GP before callback; endpoint AddressOverflow and backend failures remain distinct and do not commit. Store+STOP retains its write. Two source artifacts differing only in the final disp32 byte execute at their distinct expected targets. The dispatcher resumes at actual RIP from the eight-byte singleton, continues through NOP and the existing pair without replay, and rejects partial windows. See [S3-20az report](../reports/STAGE_3_S3-20az_2026-10-27.md). Production owner/MMU integration, Stage 2/ISA, Mini, physical x86, and performance remain open.

#### S3-20ba: address32 SIB access with RBP index

Admit only singleton `67 48 8B/89 /r sib [disp8|disp32]`: ModRM `mod=00/01/10,r/m=100`, SIB base RAX..RBX and index exactly RBP, scale 1/2/4/8, data RAX..RBP, lengths 5/6/9. Address size is Dword while transfer size remains Qword. This is a separate scoped index exception; the generic RAX..RBX index gate stays unchanged, and existing 20ay/20az and address32 SIB forms remain independently validated. Stack/no-base bases, other indices, extra prefixes, pair, and V3 forms are unsupported. The decoded `MemoryOperand` and `access_address_48` own low32 base/index scaling/addition and signed displacement semantics; runtime does not re-decode displacement bytes.

The descriptor matrix covers both operations, all six data registers, four low bases, four scales, all three lengths, and signed disp8/disp32 edges. Host parity compares direct V2/shared IR/interpreter CPU/RAM/callback/retirement and the independent `(low32(base) + low32(RBP)*scale + signed displacement) mod 2^32` formula. It checks high-half truncation, pre-state data/index aliases, a contiguous qword callback across 4 GiB, and two immutable disp32 artifacts differing only in byte nine with distinct exact callback targets. Injected architectural and backend errors preserve CPU/RAM state and classification; the callback address/count remains exact. Successful store+STOP commits once. Bounded dispatch splits after the RBP-index singleton, resumes at actual RIP through NOP and the established pair without replay, and returns fixture `BoundaryReached` for partial windows 1..8 without effects. See [S3-20ba report](../reports/STAGE_3_S3-20ba_2026-10-27.md). Production owner/MMU integration, Stage 2/ISA, Mini, physical x86, and performance remain open.

#### S3-20bb: address64 SIB access with RSP base and RBP index

Admit only singleton `48 8B/89 /r sib [disp8|disp32]`: ModRM `mod=00/01/10,r/m=100`, SIB base exactly RSP, index exactly RBP, scale 1/2/4/8, data RAX..RBP, lengths 4/5/8. This is an exact base/index exception; generic index admission remains RAX..RBX. Address size and transfer size are both Qword. No address override, RBP/no-base base, other new index, extra prefix, pair, or V3 profile is admitted. The decoder `MemoryOperand` and `access_address_48` remain the only effective-address semantics: `RSP + RBP*scale + signed displacement` with wrapping 64-bit arithmetic and normal canonical/range checks.

Descriptor and host evidence covers load/store, all six data registers, scales, and no-displacement/disp8/disp32 signed edges. Direct V2/shared IR/interpreter parity compares complete CPU/RAM state, callback identity, and retirement, including data=RSP/RBP aliases from pre-state and a wrapping effective address. Exact fault checks cover #SS at noncanonical qword start/end for the RSP base before callback, a low-GPR-base #GP control, AddressOverflow, backend errors without commit and with exact callbacks, and store+STOP commit. The borrowed dispatcher resumes from actual RIP after the singleton and NOP through the existing pair; partial windows stop with fixture `BoundaryReached` without effects. See [S3-20bb report](../reports/STAGE_3_S3-20bb_2026-10-27.md). Production owner/MMU integration, Stage 2/ISA, Mini, physical x86, and performance remain open.

#### S3-20bc: address64 SIB access with RBP base and RBP index

Admit only singleton `48 8B/89 /r sib disp8|disp32` with ModRM `mod=01/10,r/m=100`, SIB base=RBP and index=RBP, scale 1/2/4/8, data RAX..RBP, and instruction length 5/8. Both address and operand sizes are Qword. `mod=00`/SIB base `101` remains the no-base encoding and is not admitted by this form. This is one exact base/index exception; the global index whitelist remains unchanged. No address override, other index/base, extra prefix, pair, or V3 form is admitted. Decoded `MemoryOperand::access_address_48` remains the sole EA implementation, including signed displacement and wrapping arithmetic.

The descriptor/host matrices compare load and store for all six data registers, four scales, and signed disp8/disp32 edges. Direct V2/shared IR/interpreter runs compare CPU/RAM/callback/retirement against independent `RBP + RBP*scale + signed displacement` calculations; `data=base=index=RBP` uses the pre-instruction value. Fault tests check #SS at noncanonical qword start/end without callbacks, a low-base #GP control, qword-end AddressOverflow, and backend failure classification/callback identity without commit. Successful store+STOP commits exactly once. Bounded dispatch splits after the singleton and continues from actual RIP through NOP and the established pair without repeating the read; unsupported neighbors and partial windows stop before effects. See [S3-20bc report](../reports/STAGE_3_S3-20bc_2026-10-27.md). Production owner/MMU integration, Stage 2/ISA, Mini, physical x86, and performance remain open.

#### S3-20bd: address32 SIB access with RSP base and RBP index

Admit only singleton `67 48 8B/89 /r sib [disp8|disp32]`: ModRM `mod=00/01/10,r/m=100`, SIB base RSP/index RBP, scale 1/2/4/8, data RAX..RBP, Dword address and Qword transfer, exact lengths 5/6/9. This adds a single explicit base/index pair; it does not broaden the generic index whitelist. Existing no-index, RSP-base/low-index, low-base/RBP-index and address64 RSP/RBP-index forms remain distinct. No RBP base, no-base, other index, extra prefix, pair, or V3 form is included. The decoded `MemoryOperand` and `access_address_48` remain the only EA semantics.

The descriptor matrix checks both directions, all data registers/scales and signed displacement edges. Host parity compares direct V2/shared IR/interpreter CPU/RAM/callback/retirement against `(low32(RSP) + low32(RBP)*scale + signed displacement) mod 2^32`, including aliases and ignored high halves. It confirms a contiguous qword callback across 4 GiB, distinct targets for immutable source artifacts differing in the ninth byte, backend no-commit, store+STOP, and dispatcher continuation through NOP and the existing pair without replay. Partial windows 1..8 and invalid SIB neighbors stop before effects. See [S3-20bd report](../reports/STAGE_3_S3-20bd_2026-10-27.md). Production owner/MMU integration, Stage 2/ISA, Mini, physical x86, and performance remain open.

#### S3-20be: address32 SIB access with RBP base and RBP index

Admit only singleton `67 48 8B/89 /r sib disp8|disp32`: ModRM `mod=01/10,r/m=100`, SIB base=index=RBP, scale 1/2/4/8, data RAX..RBP, Dword effective address and Qword transfer, exact lengths 6/9. The `mod=00,SIB.base=101` form remains no-base and is excluded. This scoped pair does not widen the generic index whitelist and does not add other bases/indices, no-displacement, prefixes, pairs, or V3. The decoder-owned `MemoryOperand` and `access_address_48` remain the only effective-address semantics.

The descriptor and host matrices cover both directions, all six data registers, four scales, and signed disp8/disp32 boundaries. Direct V2/shared IR/interpreter parity checks complete CPU/RAM/callback/retirement state against `(low32(RBP) + low32(RBP)*scale + signed displacement) mod 2^32`, including `data=base=index=RBP` pre-state aliasing and full 64-bit payloads. A qword at `0xffff_fffc` uses one contiguous callback across 4 GiB. Guest callback faults preserve their payload and do not commit; backend failures remain host errors without commit. Since Dword EA truncates before canonicality, noncanonical high RBP bits do not produce #SS. Store+STOP commits once; budget continuation resumes at actual RIP through NOP and the existing pair without replay. The ninth displacement byte changes the independently checked target; partial windows 1..5/1..8 and the no-base neighbor stop before effects. See [S3-20be report](../reports/STAGE_3_S3-20be_2026-10-27.md). Production owner/MMU integration, Stage 2/ISA, Mini, physical x86, and performance remain open.

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

#### S3-20bf: address64 indexed no-base SIB with RBP index

Admit only singleton `48 8B/89 /r sib disp32`, ModRM `mod=00,r/m=100`, SIB base field `101` (no base), index RBP, scale 1/2/4/8, data RAX..RBP, exact length eight. This is the address64 Qword profile: its effective address is `RBP*scale + signed disp32` modulo 2^64; there is no base or RIP contribution. `MemoryOperand::access_address_48` remains the only EA/range/canonicality implementation. Since the operand has no stack base, noncanonical start/end faults are #GP rather than #SS. The separate address32 Dword profile is specified in S3-20bg; this section does not add prefixes, other indices, disp8/base forms, pairs or V3.

The descriptor and host matrices compare both directions, six data registers, four scales and signed disp32 edges. Host parity checks direct V2/shared IR/interpreter CPU/RAM/callback/retirement against an independent wrapping-u64 formula and pre-state `data=RBP` aliasing. It also checks #GP without callback, AddressOverflow independently, store+STOP commit, exact byte-8 displacement identity, windows 1..7 and invalid neighbors, plus bounded publication at the actual RIP. The address32 sibling has a separate exact profile in S3-20bg; it does not widen this Qword profile. See [S3-20bf report](../reports/STAGE_3_S3-20bf_2026-10-27.md). Owner/MMU, Stage 2/ISA, Mini, physical x86 and performance remain open.

#### S3-20bg: address32 indexed no-base SIB with RBP index

Admit only singleton `67 48 8B/89 /r sib disp32`: ModRM `mod=00,r/m=100`, SIB base field `101` (no base), index RBP, scale 1/2/4/8, data RAX..RBP, Dword EA/Qword transfer, exact length nine. This is distinct from the address64 S3-20bf profile. The effective address is `(low32(RBP)*scale + signed disp32) mod 2^32`, zero-extended to 64 bits; neither a base nor RIP contributes. The decoded `MemoryOperand` and `access_address_48` remain the only EA implementation. Do not widen the generic index whitelist or admit other no-base indices, extra prefixes, pairs, or V3.

Descriptor and host matrices cover load/store, six data registers, four scales, and disp32 MIN/-1/0/1/MAX. Direct V2/shared IR/interpreter parity checks complete CPU/RAM/callback/retirement and the independent low32 address formula, including `data=RBP` pre-state aliasing and full 64-bit payloads. High RBP bits are ignored for the address; a qword crossing 4 GiB remains one contiguous callback. Byte nine changes the target. Architectural/backend failures preserve no-commit behavior, store+STOP commits once, and bounded dispatch resumes from actual RIP without repeating the access. Partial windows 1..8 and unsupported neighbors produce no effects. See [S3-20bg report](../reports/STAGE_3_S3-20bg_2026-10-28.md). Owner/MMU integration, Stage 2/ISA, Mini, physical x86 and performance remain outside this slice.

#### S3-20bh: address64 direct-base qword MOV with RSI/RDI base

Admit only singleton `48 8B/89 /r`, length 3, ModRM `mod=00`, `rm=110` (RSI) or `rm=111` (RDI), and data RAX..RBP. The decoded operand must have Qword address/data sizes, the selected RSI/RDI base, no index, scale 1, and displacement zero. RSI/RDI are base-only in this slice: they are not data registers or indexes. No SIB, displacement, address override, extra prefix, pair or V3 form is added. `MemoryOperand::access_address_48` remains the only EA/range/canonicality implementation.

Host parity covers direct V2/shared IR/interpreter load/store for both bases and all six data registers, including exact callback address/count and full-state preservation. The base value is the full old RSI/RDI and is independent of RIP/RSP/RBP; loads update only the destination and stores send the full qword. Noncanonical start/end gives #GP with no callback even when RSP/RBP is the data register; AddressOverflow remains separate. Backend errors do not commit. Store+STOP commits once; entry STOP, stop-RIP and budget zero do not access memory. Budget-split dispatch resumes at actual RIP without replay. Partial byte windows and SIB/displacement/address32/data-RSI/RDI neighbors are rejected before effects. See [S3-20bh report](../reports/STAGE_3_S3-20bh_2026-10-28.md). Production owner/MMU integration, Stage 2/ISA, Mini, hardware and performance remain outside this slice.

#### S3-20bi: address64 direct-base qword MOV with RSI/RDI and displacement

Admit only singleton `48 8B/89 /r disp8|disp32`: ModRM `mod=01/10`, `rm=110` (RSI) or `rm=111` (RDI), data RAX..RBP, Qword address/data, exact lengths four/seven. RSI/RDI are base-only: no SIB, address override, additional prefixes, data/index use, pair or V3 forms. The decoded `MemoryOperand::access_address_48` remains the only signed-displacement/effective-address implementation.

Descriptor and host matrices cover load/store × both bases × six data registers × disp8/disp32 signed boundaries. Direct V2/shared IR/interpreter parity checks complete CPU/RAM/callback/retirement state and the independent `old_base.wrapping_add_signed(displacement)` target. Noncanonical start/end produces #GP before callback; canonical qword overflow remains a distinct AddressOverflow diagnostic. Architectural/backend failures do not commit. Store+STOP writes once; entry STOP, stop-RIP and zero budget perform no access. Disp32's final byte changes the observed target. Partial windows 1..3 and 1..6 and unsupported SIB/prefix/address32/data-RSI/RDI neighbors have no effects. Budget-split dispatch resumes at actual RIP across disp8 and disp32 instructions without replay. See [S3-20bi report](../reports/STAGE_3_S3-20bi_2026-10-28.md). Production owner/MMU integration, Stage 2/ISA, Mini, physical x86 and performance remain outside this slice.

#### S3-20bj: address32 direct-base qword MOV with RSI/RDI

Admit only singleton `67 48 8B/89 /r [disp8|disp32]`, ModRM `mod=00/01/10`, `rm=110/111`, and data RAX..RBP. Instruction lengths are 4, 5, and 8 bytes. The decoded memory operand remains authoritative: EA is `(low32(old_base) + signed_displacement) mod 2^32`; the qword transfer remains eight contiguous bytes, and full RSI/RDI values are preserved. Do not infer this profile from instruction length or widen SIB, prefixes, bases, or data registers.

Descriptor and host matrices cover loads/stores, both bases, six data registers, and signed displacement boundaries. Direct V2/shared IR/interpreter parity checks CPU/RAM/callback/retirement state, an independently computed low32 target, and upper-base-bit truncation. A qword access at `0xffff_fffc` stays one contiguous `(EA, 8)` callback across 4 GiB. Backend faults do not commit; successful store+STOP remains committed. Entry guards prevent helper access, partial windows and unsupported neighbors have no effects, and bounded dispatch resumes from actual RIP without replay. See [S3-20bj report](../reports/STAGE_3_S3-20bj_2026-10-28.md). Production owner/MMU integration, Stage 2/ISA, Mini, physical x86 and performance remain outside this slice.

#### S3-20bk: address64 indexed SIB qword MOV with RSI/RDI base

Admit only the four-byte singleton `48 8B/89 /r sib`: ModRM `mod=00/rm=100`, SIB base RSI or RDI, index RAX..RBX, scale 1/2/4/8, data RAX..RBP, and Qword address/data sizes. Displacement, address override, no-index/no-base, RBP/RSI/RDI index, additional prefixes, pairs and V3 forms remain unsupported. Effective-address semantics come from the decoded memory operand's `access_address_48`; the target is the old full base plus the old full index times scale, wrapping in u64.

The descriptor matrix covers both directions, both bases, four indices, four scales and all six data registers. Host direct V2/shared IR/interpreter parity checks CPU/RAM/callback/retirement and the independent wrapping-u64 target; `data=index` aliases use pre-instruction values and the base is preserved. Noncanonical start/end faults happen before any memory callback, and AddressOverflow is distinct. Entry STOP and zero budget prevent helper access. A budget-split dispatcher sequence resumes from the actual RIP through the SIB singleton, explicit NOP fallback and existing pair without repeating the first read. The exact-profile store+STOP callback commits once. Partial windows and unsupported neighboring forms are rejected. See [S3-20bk report](../reports/STAGE_3_S3-20bk_2026-10-28.md). Production owner/MMU integration, Stage 2/ISA, Mini, physical x86 and performance remain outside this slice.

#### S3-20bl: address64 indexed SIB qword MOV with RSI/RDI base and displacement

The exact singleton profile is `48 8B/89 /r sib disp8|disp32`: ModRM `mod=01/10,r/m=100`, SIB base RSI/RDI, index RAX..RBX, scale 1/2/4/8, data RAX..RBP, instruction length 5/8. The decoder-owned `MemoryOperand::access_address_48` computes `old_base + old_index*scale + signed displacement` with u64 wrapping. The fixture matcher checks the exact opcode/ModRM/SIB profile, consumed length, and decoded displacement width against the immutable decoder-built artifact. It does not re-decode raw displacement bytes or calculate an effective address; the code bytes and operand metadata share the compile-time immutable source snapshot. This does not admit address32, other base/index fields, no-index/no-base, extra prefixes, pairs or V3.

The JIT descriptor matrix exercises both directions and all data/base/index/scale choices at signed displacement boundaries. Host direct V2/shared IR/interpreter parity checks full CPU/RAM state, independent callback targets/counts and retirement; two immutable disp32 artifacts differing only in the last encoded displacement byte assert targets `0x1000` and `0x0100_1000`, and a displaced high-index case checks scale-8 wrapping u64 arithmetic. Dispatcher evidence covers continuation through disp8 and disp32 singletons, a NOP, and the existing pair, pre-entry STOP/budget guards, successful store+STOP, partial source windows, #GP start/end without callbacks, distinct AddressOverflow, and backend error atomicity. See the [S3-20bl report](../reports/STAGE_3_S3-20bl_2026-10-28.md). This bounded borrowed-memory fixture profile does not implement production owner/MMU integration.

#### S3-20bm: address32 indexed SIB qword MOV with RSI/RDI base

Admit only singleton `67 48 8B/89 /r sib [disp8|disp32]`: ModRM `mod=00/01/10,r/m=100`, SIB base RSI or RDI, index RAX..RBX, scale 1/2/4/8, data RAX..RBP, instruction lengths 5/6/9. Address size is Dword and transfer size is Qword. `MemoryOperand::access_address_48` remains the sole effective-address implementation: `(low32(old base) + low32(old index) * scale + signed displacement) mod 2^32`, zero-extended to u64. Guest base registers are preserved and the qword access is contiguous across the 4 GiB boundary. No additional prefixes, other bases/indices, pairs or V3 shapes are admitted.

Descriptor and host matrices compare direct V2/shared IR/interpreter for both operations and all base/data/index/scale combinations, including a mod00 case for every combination and signed displacement boundaries, exact callback address/count and retirement. They check pre-state `data=index`, high-half truncation, ninth-byte disp32 target identity, qword crossing, architectural/backend load and store fault atomicity with RAM snapshots, store+STOP commit, entry STOP/stop-RIP/zero-budget guards, partial byte windows, unsupported neighbors and dispatcher continuation from actual RIP across 5/6/9-byte forms without replay. See the [S3-20bm report](../reports/STAGE_3_S3-20bm_2026-10-28.md). Production owner/MMU integration remains out of scope.

#### S3-20bn: address64 indexed SIB qword MOV with RSI/RDI base and RBP index

Admit only singleton `48 8B/89 /r sib [disp8|disp32]`: ModRM `mod=00/01/10,r/m=100`; SIB base RSI/RDI, index exactly RBP, scale 1/2/4/8; data register RAX..RBP; exact instruction lengths 4/5/8. Effective address semantics remain solely in the decoder's `MemoryOperand::access_address_48`: old full-width RSI/RDI base + old full-width RBP × scale + signed displacement, wrapping as u64. RBP as a destination/source uses its pre-instruction value for index and store data; successful loads commit the destination only after the read succeeds.

This is a local exact-profile exception. It does not widen the generic index whitelist. Address32, no-index, no-base, other index/base registers, extra prefixes, pairs and V3 remain unsupported.

The descriptor matrix and host direct V2/shared IR/interpreter matrix cover load/store, RSI/RDI bases, all four scales, six data registers, and mod00/disp8/disp32 signed edges. Evidence includes independent u64 target calculations, pre-state `data=RBP`, wrapping arithmetic, fixed-register disp32 artifacts differing at byte 7, full CPU/RAM parity, callback addresses/counts and retirement. Dispatcher evidence covers actual-RIP budget continuation across 4/5/8-byte forms without replay, entry STOP/budget zero, store+STOP commit, partial windows, invalid neighbors, noncanonical start/end #GP without callbacks, separate AddressOverflow, and backend no-commit. See the [S3-20bn report](../reports/STAGE_3_S3-20bn_2026-10-28.md). Production owner/MMU integration remains out of scope.

#### S3-20bo: address32 indexed SIB qword MOV with RSI/RDI base and RBP index

Admit singleton `67 48 8B/89 /r sib [disp8|disp32]` only: ModRM `mod=00/01/10,r/m=100`, SIB base RSI or RDI, index exactly RBP, scale 1/2/4/8, data RAX..RBP, lengths 5/6/9. The decoded `MemoryOperand::access_address_48` is the sole EA authority: `(low32(old_base) + low32(old_RBP) * scale + signed_disp) mod 2^32`, zero-extended. Transfer width remains Qword and the source GPR value remains full 64-bit. RSI/RDI bases are preserved; RBP is read from pre-state as the index and is only changed by a successful load when it is the destination.

The exact profile is a scoped admission exception; it does not widen the generic index/base whitelist and does not add other bases/indices, address64 alternatives, extra prefixes, pairs, or V3. Descriptor and host evidence compare direct V2/shared IR/interpreter for both operations, both bases, all six data registers, all four scales, mod00, and signed disp8/disp32 boundaries. Host assertions cover independent low32 EA, high-half truncation, pre-state `data=RBP`, callback address/count and retirement, qword crossing at 4 GiB, ninth displacement byte sensitivity, load/store architectural and backend no-commit, store+STOP, entry STOP/budget0, partial windows and budget continuation across actual 5/6/9-byte RIPs. See the [S3-20bo report](../reports/STAGE_3_S3-20bo_2026-10-28.md).

#### S3-20bp: address64 no-displacement SIB qword MOV with RSI/RDI index

Admit only singleton `48 8B/89 /r sib`: ModRM `mod=00/rm=100`, SIB base RAX..RBX, index exactly RSI or RDI, scale 1/2/4/8, data RAX..RBP, length 4. The decoded `MemoryOperand::access_address_48` remains the sole EA authority: old full-width base plus old full-width RSI/RDI times scale, wrapping u64. When data=base, the pre-state base supplies the EA and store payload; a load to base commits only after the read succeeds. RSI/RDI are address-only index registers and remain unchanged. Generic register whitelists remain unchanged.

Descriptor and host direct V2/shared IR/interpreter parity cover load/store × four bases × RSI/RDI × four scales × six data registers, checking complete CPU/RAM, independent target, callback address/count, and retirement. Exact-profile cases cover the data=base pre-state alias, unchanged RSI/RDI indices, canonical-start/noncanonical-end #GP with no callback, separate AddressOverflow, architectural/backend no-commit, store+STOP, entry STOP/zero budget, and actual-RIP continuation through this singleton, NOP and the existing pair without replay. Unsupported neighboring encodings and partial source windows are rejected. See the [S3-20bp report](../reports/STAGE_3_S3-20bp_2026-10-28.md). This is a bounded borrowed-memory fixture profile, not production owner/MMU integration.

#### S3-20bq: address64 indexed SIB qword MOV with low base, RSI/RDI index, and displacement

Admit only singleton `48 8B/89 /r sib disp8|disp32`: ModRM `mod=01/10,r/m=100`, SIB base RAX..RBX, index exactly RSI or RDI, scale 1/2/4/8, data RAX..RBP, lengths 5/8. `MemoryOperand::access_address_48` remains the only EA authority: old full-width base + old full-width RSI/RDI × scale + signed displacement, wrapping u64. A load to the base uses the pre-state base for the address and commits only after a successful read; RSI/RDI are address-only indices and remain unchanged. No generic register whitelist is widened.

Descriptor and host direct V2/shared IR/interpreter parity cover both operations × four bases × two indices × four scales × six data registers at signed disp8/disp32 boundaries. Evidence checks complete CPU/RAM state, independent EA oracle including full-width high index bits and u64 wrap, exact callback address/count, retirement, final disp32 byte target identity, and data=base pre-state behavior. Exact-profile checks cover #GP at noncanonical start/end before callback, separate AddressOverflow, architectural/backend no-commit, successful store+STOP, entry STOP/stop-RIP/zero-budget guards, partial/unsupported neighbors without effects, and actual-RIP budget continuation through disp8/disp32 singleton, NOP and the existing pair without replay. See the [S3-20bq report](../reports/STAGE_3_S3-20bq_2026-10-28.md). Production owner/MMU integration remains out of scope.

#### S3-20br: address32 indexed SIB qword MOV with low base and RSI/RDI index

Admit only singleton `67 48 8B/89 /r sib [disp8|disp32]`: ModRM `mod=00/01/10,r/m=100`, SIB base RAX..RBX, index RSI or RDI, scale 1/2/4/8, data RAX..RBP, exact lengths 5/6/9. The decoded `MemoryOperand::access_address_48` remains the sole EA implementation: `(low32(old_base)+low32(old_index)*scale+signed displacement) mod 2^32`, zero-extended. Data and memory transfer remain qword; RSI/RDI are address-only indices and remain full-width/preserved. When the data register aliases the low base, the old base supplies the EA and store payload; a load commits only after the read succeeds.

The JIT descriptor and host direct V2/shared IR/interpreter matrix cover loads and stores, all low bases, RSI/RDI, scales, data registers and the no-displacement/disp8/disp32 forms. Evidence includes independent low32 address calculation, high-half truncation and wrapping, pre-state aliases, complete CPU/RAM parity, exact callback address/length/count and retirement, contiguous qword crossing 4 GiB, and distinct immutable artifacts whose ninth byte changes the disp32 target. Address-size truncation keeps the effective address canonical even when source registers have noncanonical high halves; a Dword effective address plus qword length cannot overflow u64. Host checks also cover architectural and backend no-commit behavior for both directions, store+STOP, entry STOP/stop-RIP/zero-budget, partial windows, invalid neighbors, and bounded actual-RIP continuation across 5/6/9-byte forms without replay. The generic register whitelist is unchanged. See the [S3-20br report](../reports/STAGE_3_S3-20br_2026-10-28.md). Production memory owner/MMU integration, Stage 2/ISA, Mini and performance remain out of scope.

#### S3-20bs: address64 indexed SIB qword MOV with RSP base and RSI/RDI index

Admit only singleton `48 8B/89 /r sib [disp8|disp32]`: ModRM `mod=00/01/10,r/m=100`, SIB base RSP, index RSI or RDI, scale 1/2/4/8, data RAX..RBP, exact lengths 4/5/8. Effective addresses remain decoder-owned through `MemoryOperand::access_address_48`: old full RSP + old full RSI/RDI×scale + signed displacement with u64 wrapping. `data=RSP` uses the pre-instruction base for EA and store payload; a load commits RSP only after a successful read. RSI/RDI are index-only and remain unchanged.

The descriptor and host direct V2/shared IR/interpreter matrix covers both operations, six data registers, both indices, four scales and mod00/disp8/disp32 signed edges. Evidence includes full CPU/RAM parity, an independent full-width EA oracle, callback address/count and retirement, pre-state data=RSP, RSP-base #SS at noncanonical start/end before callbacks, a low-base #GP control, separate AddressOverflow, architectural/backend no-commit for loads/stores, committed store+STOP, entry stop-RIP/STOP/zero-budget, byte8 disp32 target identity and actual-RIP continuation across 4/5/8-byte forms without replay. Partial windows and unsupported neighbors have no effects. See the [S3-20bs report](../reports/STAGE_3_S3-20bs_2026-10-28.md). Production owner/MMU integration remains out of scope.

#### S3-20bt: address64 indexed SIB qword MOV with RBP base, RSI/RDI index, and displacement

Admit only singleton `48 8B/89 /r sib disp8|disp32`: ModRM `mod=01/10,r/m=100`, SIB base RBP, index RSI or RDI, scale 1/2/4/8, data RAX..RBP, lengths 5/8. ModRM `mod=00` with SIB base field 5 remains the no-base form and is not part of this profile. `MemoryOperand::access_address_48` owns effective-address semantics: old full RBP + old full RSI/RDI×scale + signed displacement, wrapping u64. Data source/address values are read from pre-instruction state; a load commits its destination only after a successful memory read.

The checked block builder, shared IR validator, direct V2 artifact, runtime source matcher, and bounded dispatcher use the same exact profile. Tests cover direct V2/shared IR/interpreter parity for loads and stores across six data registers, both indices, four scales and signed disp8/disp32 boundaries, with complete CPU/RAM parity, independent target, callback address/count and retirement. Host cases cover RBP-base #SS on noncanonical start/end without callback, low-base #GP, separate AddressOverflow, architectural/backend no-commit for both directions, successful store+STOP, entry guards, partial byte windows, and actual-RIP dispatcher continuation across both lengths without replay. Extra prefixes, no-base, other indices, pairs, and V3 remain unsupported. See the [S3-20bt report](../reports/STAGE_3_S3-20bt_2026-10-28.md). This remains borrowed fixture evidence; production owner/MMU integration is open.

#### S3-20bu: address32 indexed SIB qword MOV with RSP base, RSI/RDI index

The bounded fixture profile admits only singleton `67 48 8B/89 /r sib [disp8|disp32]`: ModRM `mod=00/01/10,r/m=100`, SIB base RSP, index RSI or RDI, scale 1/2/4/8, data RAX..RBP and lengths 5/6/9. Address calculation remains solely in the decoder's `MemoryOperand::access_address_48`: `(low32(old RSP)+low32(old index)*scale+signed displacement) mod 2^32`, zero-extended for the qword transfer. Data=RSP uses pre-state for the address and store payload; loads update RSP only after a successful read. RSI/RDI are index-only and remain unchanged. The generic register whitelist is unchanged.

The checked block builder, shared IR validator, direct V2 artifact, runtime source matcher, and bounded dispatcher admit the same exact shape. Descriptor and host direct V2/shared IR/interpreter evidence covers both operations, six data registers, both indices, four scales, and no-displacement/disp8/disp32 forms; it compares full CPU/RAM state, independently calculated EA, callback address/width/count, and retirement. Additional coverage verifies high-half truncation, modulo-32 product/add wrap, a contiguous qword callback across 4 GiB, final disp32 byte target identity, architectural/backend no-commit behavior, store+STOP, entry guards, partial windows and unsupported neighbors, plus actual-RIP continuation across 5/6/9-byte forms without replay. No extra prefix, other base/index, pair, or V3 form is added. See the [S3-20bu report](../reports/STAGE_3_S3-20bu_2026-10-28.md). This is borrowed fixture evidence; production owner/MMU integration remains open.

#### S3-20bv: address32 indexed SIB qword MOV with RBP base and RSI/RDI index

Admit only singleton `67 48 8B/89 /r sib disp8|disp32`: ModRM `mod=01/10,r/m=100`, SIB base RBP, index RSI or RDI, scale 1/2/4/8, data RAX..RBP, lengths 6/9. ModRM `mod=00` with SIB base field 5 denotes no-base and is excluded. Decoder-owned `MemoryOperand::access_address_48` computes `(low32(old RBP)+low32(old RSI/RDI)*scale+signed displacement) mod 2^32`, zero-extended for the qword transfer. Data=RBP uses the pre-instruction RBP for EA/store source; a load commits RBP only after a successful read. RSI/RDI are address-only indices and remain unchanged.

The checked block builder, shared IR validator, direct V2 artifact, runtime source matcher, and bounded dispatcher use this exact profile. Descriptor and host direct V2/shared IR/interpreter cases cover both directions, six data registers, both indices, four scales and signed disp8/disp32 edges with complete state/RAM parity, independent EA, callback address/width/count, and retirement. Host evidence includes high-half truncation/wrap, contiguous qword crossing 4 GiB, ninth-byte disp32 target identity, architectural/backend no-commit for both operations, store+STOP, entry guards, partial windows and unsupported neighbors, plus actual-RIP dispatcher continuation from the 6-byte form to the 9-byte form without replay. No generic register whitelist, pair, V3 or owner/MMU scope is changed. See [S3-20bv report](../reports/STAGE_3_S3-20bv_2026-10-28.md). This remains borrowed fixture evidence; production owner/MMU integration is open.

#### S3-20bw: address64 indexed no-base SIB qword MOV with RSI/RDI index and disp32

Admit only singleton `48 8B/89 /r sib disp32`: ModRM `mod=00/rm=100`, SIB base field `101` (no base), index RSI or RDI, scale 1/2/4/8, data RAX..RBP, exact length 8. Effective address is decoder-owned through `MemoryOperand::access_address_48`: old full-width index×scale plus signed disp32, wrapping u64; no base or RIP contribution. RSI/RDI are index-only and remain unchanged. The exact source/profile path does not re-decode the displacement; it consumes the immutable decoder-built operand.

The block builder, shared IR verifier, direct V2 artifact, runtime matcher, and bounded dispatcher must agree on this exact shape. Acceptance compares direct V2/shared IR/interpreter for load/store × six data registers × RSI/RDI × four scales × signed disp32 edges, with independent full-u64 EA, complete CPU/RAM, callback address/width/count and retirement. Host checks include full-width high-index participation and u64 wrap, canonical-start/noncanonical-end #GP before callback, end-of-u64 overflow separately, architectural/backend no-commit, committed store+STOP, stop-RIP/STOP/budget0 entry guards, windows 1..7 without effects, final source-byte target identity, and actual-RIP continuation without replay. Preserve the separate no-base/no-index `0x25` profile and the addr32 no-base sibling. See the [S3-20bw report](../reports/STAGE_3_S3-20bw_2026-10-28.md). Owner/MMU integration remains open; Stage 2/ISA, Mini, and performance work are excluded.

#### S3-20bx: address32 indexed no-base SIB qword MOV with RSI/RDI index and disp32

The exact singleton is `67 48 8B/89 /r sib disp32`: ModRM `mod=00/rm=100`, SIB base field `101` (no base), index RSI or RDI, scale 1/2/4/8, data RAX..RBP, length 9. `MemoryOperand::access_address_48` remains the sole EA implementation: `(low32(index)*scale + signed disp32) mod 2^32`, then zero-extension for the qword transfer. There is no base or RIP contribution; full RSI/RDI values remain unchanged. The exact profile does not widen other prefixes, registers, pair or V3 forms.

The checked block builder, direct V2 site, shared IR verifier, runtime source matcher, and bounded dispatcher admit this profile. Descriptor and host direct V2/shared IR/interpreter evidence checks load/store × six data registers × RSI/RDI × four scales × signed disp32 edges, full CPU/RAM state, independent EA, callback address/width/count, and retirement. Host evidence also covers high-half truncation and modulo-32 product/add wrap, contiguous qword access across 4 GiB, ninth-byte source identity, architectural/backend no-commit, store+STOP payload, entry guards, partial windows 1..8, invalid neighbors, and continuation from actual RIP without replay. See the [S3-20bx report](../reports/STAGE_3_S3-20bx_2026-10-28.md). This remains borrowed fixture evidence; production owner/MMU integration is open.

### S3-20by: address64 indexed SIB with RSI/RDI base and RSI/RDI index

Admit only singleton `48 8B/89 /r sib` with ModRM `mod=00/01/10,r/m=100`, SIB base RSI/RDI and index RSI/RDI, scale 1/2/4/8, data RAX..RBP, and exact lengths 4/5/8. The sole EA source is decoder-built `MemoryOperand::access_address_48`: full-width old base + full-width old index × scale + signed displacement, modulo u64. If base and index are the same register, both contributions read that register's pre-instruction value. RSI/RDI are address-only here and remain unchanged; generic register whitelist is unchanged.

JIT descriptor and host matrices cover both directions, all base/index pairs, scales, data registers, three lengths, and signed displacement boundaries. Direct V2/shared IR/interpreter comparisons check full CPU/RAM state, independent EA, exact callback address/count and retirement. Additional cases cover same-register alias and wrapping, last disp32 byte target identity, #GP start/end without helper calls, separate AddressOverflow, architectural/backend no-commit, full-payload store+STOP, entry guards, partial windows, and actual-RIP dispatcher continuation across the new singleton, NOP and frozen pair without replay. Invalid neighbors stay unsupported. See the [S3-20by report](../reports/STAGE_3_S3-20by_2026-10-28.md). This bounded borrowed-memory profile does not integrate a production owner/MMU.


### S3-20bz: address32 indexed SIB with RSI/RDI base and RSI/RDI index

Admit only singleton `67 48 8B/89 /r sib [disp8|disp32]`: ModRM `mod=00/01/10,r/m=100`; SIB base and index each RSI/RDI; scale 1/2/4/8; data RAX..RBP; lengths 5/6/9. Effective address remains exclusively decoder-owned through `MemoryOperand::access_address_48`: `(low32(old_base)+low32(old_index)*scale+signed displacement) mod 2^32`, zero-extended for the qword transfer. Same-register base/index uses the same pre-state value for both contributions, and full RSI/RDI values remain preserved.

Descriptor and host direct V2/shared IR/interpreter matrices cover both operations, all four base/index pairs, scales, data registers, three instruction forms and signed displacement boundaries, checking full CPU/RAM, independent EA, callback address/width/count and retirement. Exact cases verify high-half truncation, same-register/product/add wrapping, contiguous qword crossing 4 GiB, ninth-byte target identity, architectural/backend no-commit, store+STOP, entry guards, partial windows and actual-RIP continuation through 5/6/9-byte singleton→NOP→frozen pair without replay. Invalid neighbors remain unsupported. See [S3-20bz report](../reports/STAGE_3_S3-20bz_2026-10-28.md). This is bounded borrowed-memory fixture coverage; production owner/MMU integration remains open.

### S3-20ca: address64 low-base SIB with no index

Admit only singleton `48 8B/89 /r sib [disp8|disp32]`, ModRM `mod=00/01/10,r/m=100`, SIB byte `0x20..=0x23`, data RAX..RBP, Qword transfer/address size, lengths 4/5/8. The SIB index field `100` with extension clear means no index; it is not guest RSP. The scale is exactly one and the base is RAX..RBX. No address32 form, other base/index/SIB shape, extra prefix, additional data register, pair, or V3 admission is implied.

The decoder-owned `MemoryOperand::access_address_48` remains the only effective-address implementation: old base plus signed displacement with existing 64-bit semantics. Checked block construction, direct V2/shared IR site validation, immutable runtime source matching, and bounded dispatcher compilation must agree on this exact profile. Direct-base encodings with the same base/displacement are a semantic reference in the host matrix, not an alias in the admission profile. See the [S3-20ca report](../reports/STAGE_3_S3-20ca_2026-10-28.md).

### S3-20cb: address32 low-base SIB with no index

Admit only singleton `67 48 8B/89 /r sib [disp8|disp32]`: ModRM `mod=00/01/10,r/m=100`, SIB exactly `0x20..=0x23`, data RAX..RBP, lengths 5/6/9. SIB scale is one, index field `100` is the absent-index sentinel, and base is RAX..RBX. Address calculation is Dword while data remains Qword. `MemoryOperand::access_address_48` supplies `(low32(old base)+signed displacement) mod 2^32`, zero-extended. The base GPR remains unchanged when it is not the load destination; for data=base, load uses its old value for EA and commits the full qword destination only after a successful read. The exact admission is shared by checked block construction, direct V2/shared IR metadata, immutable runtime source matching, and bounded dispatcher compilation. The host matrix compares with address32 direct-base interpreter behavior while accounting for its shorter instruction length. Other SIB shapes, address size, prefixes, data registers, pairs and V3 remain outside this profile. See the [S3-20cb report](../reports/STAGE_3_S3-20cb_2026-10-28.md).

### S3-20cd: address32 RBP-base SIB with no index and displacement

Admit only singleton `67 48 8B/89 /r sib disp8|disp32`: ModRM `mod=01/10,r/m=100`, SIB exactly `0x25`, data RAX..RBP, Dword effective address/Qword transfer, lengths 6/9. `mod=00` with SIB `0x25` is the distinct absolute/no-base S3-20av profile. Effective address comes only from decoder-owned `MemoryOperand::access_address_48`: `(low32(old RBP)+signed displacement) mod 2^32`; high RBP bits do not contribute. Data=RBP uses pre-state for EA and store payload; a successful load commits its qword destination after the access.

The descriptor matrix covers both MOV directions, all six data registers, signed disp8/disp32 boundaries, exact source/operand metadata, direct-base reference, partial windows 1..5/1..8 and invalid neighbors. Host direct V2/shared IR/interpreter compares complete CPU/RAM, callback address/width/count and retirement; includes 4-GiB crossing, architectural/backend no-commit, committed store+STOP, entry guards, ninth disp32 byte target identity and dispatcher continuation through 6/9-byte forms without replay. See [S3-20cd report](../reports/STAGE_3_S3-20cd_2026-10-28.md). Production owner/MMU, Stage 2/ISA, Mini and performance remain outside this slice.

### S3-20ce: address64 RSI/RDI-base SIB with no index

Admit only singleton `48 8B/89 /r sib [disp8|disp32]`: ModRM `mod=00/01/10,r/m=100`, SIB exactly `0x26` or `0x27` (scale 1, index absent, base RSI or RDI), data RAX..RBP, Qword EA/data and lengths 4/5/8. The sole EA implementation remains decoder-owned `MemoryOperand::access_address_48`: old full-width base plus signed displacement with wrapping u64. RSP is never an address input and may be the data register; full RSI/RDI values are preserved. No address32/`67`, SIB scale aliases, other bases/indices, extra data registers, pairs, V3, or owner/MMU integration is implied.

Checked block construction, direct V2/shared IR metadata, immutable runtime source matching and bounded dispatcher compilation admit the same exact shapes. Descriptor and host matrices cover both directions, RSI/RDI, all six data registers, all three length forms and signed displacement edges. Host direct V2/shared IR/interpreter comparisons check complete CPU/RAM, independent EA, callback address/width/count and retirement; equivalent direct-base interpreter cases normalize only RIP for differing instruction lengths. Additional checks cover RSP independence, #GP at noncanonical start/end without callbacks including RSP/RBP data, AddressOverflow, architectural/backend no-commit, exact committed store+STOP payload, entry guards, final disp32-byte target identity, partial windows, invalid neighbors and actual-RIP continuation through 4/5/8-byte forms without replay. See [S3-20ce report](../reports/STAGE_3_S3-20ce_2026-10-28.md). Mini and performance checks are deferred until the end of Stage 3.

### S3-20cf: address32 RSI/RDI-base SIB with no index

Admit only singleton `67 48 8B/89 /r sib [disp8|disp32]`: ModRM `mod=00/01/10,r/m=100`, SIB exactly `0x26` or `0x27` (scale one, index absent, RSI/RDI base), data RAX..RBP, Dword effective address/Qword transfer and lengths 5/6/9. Effective address comes only from decoder-owned `MemoryOperand::access_address_48`: `(low32(old_base)+signed displacement) mod 2^32`, zero-extended. The source GPR and transferred qword remain full width; RSI/RDI are preserved and RSP never contributes. No scale aliases, additional prefixes, other data registers, pairs, or V3 forms are included. The separate Qword-address S3-20ce descriptor remains restricted to lengths 4/5/8.

Checked block construction, direct V2/shared IR metadata, immutable runtime source matching, and bounded dispatcher compilation accept this same byte profile. Descriptor tests cover load/store, both bases, all six data registers, mod00/disp8/disp32 and signed edges, with direct addr32 operand comparison, exact next RIP, wrap at `low32(base)=0xffffffff,disp=2`, partial windows and rejected neighbors. Host tests compare direct V2 and shared IR with the interpreter across the full matrix, including complete CPU/RAM, independent EA, callback address/width/count, retirement, high-half truncation and direct-base interpreter cases with their own length-adjusted RIP. Dispatcher checks cover 5/6/9-byte continuation without replay, incomplete windows, entry guards, one qword store followed by STOP, memory-fault no-commit, and final disp32-byte target identity. See [S3-20cf report](../reports/STAGE_3_S3-20cf_2026-10-28.md). Mini and performance checks are deferred until the end of Stage 3.

### S3-20cc: address64 RBP-base SIB with no index and displacement

Admit only singleton `48 8B/89 /r sib disp8|disp32`: ModRM `mod=01/10,r/m=100`, SIB exactly `0x25` (scale one, no index, RBP base), data RAX..RBP, Qword EA/data and lengths 5/8. The same SIB byte under `mod=00` is the separate absolute/no-base form; it does not enter this profile. Effective address comes only from decoder-owned `MemoryOperand::access_address_48`: old RBP plus signed displacement with u64 wrapping. RSP is not an index. Checked block construction, direct V2/shared IR site validation, immutable runtime matching, and bounded dispatcher compilation share this exact profile.

Host direct V2/shared IR/interpreter parity covers both directions, all six data registers and signed disp8/disp32 edges, including data=RBP pre-state semantics and full CPU/RAM/callback/retirement. A direct ModRM RBP-base encoding is used as an independent semantic comparison with its own next RIP. Tests verify last-disp32-byte target identity, #SS at noncanonical start/end before callback, separate AddressOverflow, architectural/backend no-commit, successful store+STOP, entry guards, partial windows 1..4/1..7, rejected neighboring encodings, and actual-RIP dispatcher continuation through 5/8-byte forms without replay. The addr32 `67` sibling with mod01/mod10 is handled by the separate S3-20cd profile; mod00 remains the absolute/no-base form. See the [S3-20cc report](../reports/STAGE_3_S3-20cc_2026-10-28.md). Production owner/MMU, Stage 2/ISA, Mini and performance remain outside this slice.

### S3-20ch: address64 direct-base qword load into RSI/RDI

Admit only singleton `48 8B /r`, length 3, ModRM `mod=00`, destination RSI/RDI (`reg=6|7`), and base RAX/RCX/RDX/RBX (`r/m=0..3`). No displacement, SIB, address override, extra prefix/REX, R8+ register or non-qword transfer is included. RSI/RDI `WriteGpr` is accepted only immediately after a `CheckedLoad` with the same temporary and exact qword low-base direct operand at length 3. `ReadGpr` and the generic GPR whitelist remain restricted to RAX..RBX.

The staged frame is coordinated across runtime emitters and allocation: qword GPR staging occupies `[56,120)`, flags is at 120, RIP at 128, saved X19/X20 at 144, saved X21/X22 at 160, and the reserved prefix is 176 bytes aligned to 16. Spill homes begin after that prefix; the emitted-block limit remains 4096 bytes. Direct V2, shared IR, runtime MemorySite/source matching, and block admission must accept the same eight encodings. Host checks compare full CPU/RAM and flags against the interpreter, including high-bit qwords, one read and one retirement, fault/budget guards with no mutation or unintended read, and rejection of arbitrary RSI/RDI reads/writes and neighboring encodings. Frame regressions and existing arithmetic/store/two-memory cases remain mandatory. See [S3-20ch report](../reports/STAGE_3_S3-20ch_2026-10-28.md). Mini and performance checks are deferred to the end of Stage 3; owner/MMU and Stage 2 remain outside this slice.

### S3-20ci: address64 direct low-base qword MOV with RSI/RDI data and displacement

Admit only singleton `48 8B/89 /r disp8|disp32` instructions with ModRM `mod=01/10`, data RSI/RDI (`reg=6|7`), base RAX/RCX/RDX/RBX (`r/m=0..3`), qword address/data, and exact lengths 4/7. Address calculation remains decoder-owned: full pre-instruction base plus signed displacement with wrapping u64. Do not admit SIB, address override, additional prefixes, other base/data registers, pair profiles, or V3.

The scoped IR permission for RSI/RDI sources applies only to these `CheckedStore` forms. RSI/RDI `WriteGpr` remains valid only immediately after the matching `CheckedLoad` temporary and a matching low-base qword operand; generic `ReadGpr` and the general GPR validator do not expand. Direct V2, shared IR, block construction, runtime site validation and immutable byte matching must agree, including byte-for-byte signed displacement identity.

Coverage includes all 32 opcode/data/base/displacement-width combinations, signed displacement edges, full-width wrap, noncanonical start/end #GP before callback, access endpoint overflow, architectural/backend atomicity, one full-width store before STOP, budget entry guards, actual-RIP continuation at 4/7 bytes without replay, changed final disp32 byte, partial windows 1..3/1..6 and forbidden neighbors. The staged frame remains 176 bytes, emitted code remains capped at 4096 bytes, and the 10-MOV regression remains 3868 bytes. See [S3-20ci report](../reports/STAGE_3_S3-20ci_2026-10-28.md). Mini and performance are deferred until Stage 3 ends; owner/MMU and Stage 2 are outside this slice.

### S3-20cj: address32 direct low-base qword MOV with RSI/RDI data

Admit exactly singleton `67 48 8B/89 /r [disp8|disp32]` forms with ModRM `mod=00/01/10`, data RSI/RDI (`reg=6|7`), base RAX/RCX/RDX/RBX (`r/m=0..3`), Dword address size, Qword transfer and lengths 4/5/8. This is a 48-form opcode/data/base/form matrix. The decoder-owned effective address is `(low32(old_base) + signed displacement) mod 2^32`; the qword payload/result and complete base register remain full width. Exclude SIB, extra prefixes/REX extensions, other base/data registers, pair and V3 forms.

RSI/RDI `WriteGpr` is permitted only directly after its matching `CheckedLoad` with the same temporary and exact Dword operand. Generic `ReadGpr`, arithmetic and the general GPR validator remain unchanged. Direct V2, shared IR, block admission, runtime descriptors and source-byte matching must agree; Dword lengths 4/5/8 remain distinct from address64 lengths 3/4/7.

Acceptance covers full CPU/RAM/callback address and width/count/RIP/retirement parity against the interpreter, signed displacement limits, high-half truncation and modulo-2^32 wrap, qword access crossing 4 GiB, fault/backend atomicity, store+STOP, entry guards without memory effects, actual-RIP continuation through 4/5/8 without replay, final disp32-byte identity, partial windows 1..3/1..4/1..7, forged descriptors and invalid neighbors. Preserve the 176-byte frame, 4096-byte emitted-code cap and 3868-byte 10-MOV regression. Mini and performance are deferred until Stage 3 ends; owner/MMU and Stage 2 are outside this slice. See [S3-20cj report](../reports/STAGE_3_S3-20cj_2026-10-28.md).

### S3-20ck: RIP-relative qword MOV with RSI/RDI data

Admit only singleton `48 8B/89 /r disp32`: ModRM `mod=00,r/m=101`, data RSI/RDI (`reg=6|7`), Qword address/data sizes and exact length seven. The decoder-owned `AddressBase::NextRip` and signed disp32 define the effective address from this instruction's next RIP; host runtime does not independently derive the target. RSI/RDI `WriteGpr` remains scoped to the immediately preceding matching `CheckedLoad` temporary and exact operand; generic RSI/RDI reads/arithmetic remain closed. No SIB, address override, extra prefix, other data register, pair or V3 form is admitted.

Acceptance covers all four load/store encodings in direct V2/shared IR/interpreter parity for complete CPU/RAM/flags, callback address/width/count, RIP and retirement; signed disp32 boundaries; identical bytes at distinct guest RIPs; and final disp32-byte source identity. Reject forged displacement/data/base/index/scale/size/length descriptors, invalid source variants, and partial windows 1..6 without admission or effects. Verify noncanonical #GP before callbacks, architectural/backend no-commit, entry guards, one committed full-qword store before STOP, and dispatcher continuation from actual RIP+7 without replay. Keep frame 176, emitted-code cap 4096 and 10-MOV size 3868. Mini/performance remain deferred to Stage 3 end; owner/MMU and Stage 2 are outside this slice. See [S3-20ck report](../reports/STAGE_3_S3-20ck_2026-10-28.md).

### S3-20cl: Address64 RSP-base no-index SIB with RSI/RDI data

Admit only `48 8B/89 /r sib [disp8|disp32]` with ModRM `mod=00/01/10,r/m=100`, SIB exactly `0x24`, RSI/RDI data (`reg=6|7`), qword effective address/transfer, and lengths 4/5/8. The decoder-owned `MemoryOperand::access_address_48` uses full old guest RSP plus signed displacement; host SP is unrelated. Preserve all 64 data bits. Scoped `WriteGpr` remains tied to the immediately preceding matching `CheckedLoad`; generic `ReadGpr`, arithmetic, address32, other SIB values, prefixes, pairs, and V3 stay outside this profile.

Verify all 12 forms in direct V2/shared IR/interpreter parity for complete CPU/RAM, callbacks, RIP and retirement; signed displacement limits and final disp32-byte target identity; RSP noncanonical start/end #SS before callbacks and RBX #GP control; overflow and architectural/backend atomicity; one full-width store before STOP; entry guards; partial windows 1..3/1..4/1..7; and actual-RIP continuation through 4/5/8-byte forms without replay. Preserve frame 176 bytes, emitted-code cap 4096 and 10-MOV size 3868. Mini/performance are deferred until Stage 3 ends; owner/MMU and Stage 2 are outside scope. See [S3-20cl report](../reports/STAGE_3_S3-20cl_2026-10-28.md).

### S3-20cm: Address64 direct RBP-base qword MOV with RSI/RDI data

Accept only eight singleton forms `48 8B/89 /r disp8|disp32`: ModRM `mod=01/10`, data RSI/RDI (`reg=6|7`), RBP base (`r/m=5`), qword EA/data and lengths 4/7. `[RBP]` uses `disp8=0`; `mod=00,r/m=5` remains RIP-relative. Effective address and signed displacement stay decoder-owned. Exclude SIB, address32, other data/base registers, pairs and V3. RSI/RDI `WriteGpr` remains immediately matched to its `CheckedLoad`; generic reads/arithmetic stay closed.

Compare direct V2/shared IR/interpreter for both operations and data registers with full CPU/RAM/callback/retirement parity, signed displacement edges, unaligned qword accesses and load destination aliasing the original RBP. The bounded dispatcher must admit the same eight forms; verify STOP from the successful callback, single retirement, partial windows 1..3/1..6, actual-RIP +4/+7 continuation without replay, final disp32-byte target identity, entry guards, architectural/backend atomicity, full-qword store exactly once, RBP noncanonical start/end #SS and RBX #GP control. Keep frame 176, emitted-code cap 4096 and 10-MOV size 3868. Mini/performance stay deferred until Stage 3 ends. This optional profile is not a Stage 3 completion gate (S3-33); see [S3-20cm report](../reports/STAGE_3_S3-20cm_2026-10-29.md).

### S3-28a: Cooperative host event yield at register-dispatch boundaries

`DispatchControl` may carry an invocation-local optional `AtomicBool` event request. This request is host-only: generated entry code does not read it, raw `RuntimeExitTag` values are unchanged, and `RuntimeContextV1` is unchanged. The host dispatcher returns the separate `DispatchExit::EventPending` after sampling the request at a safe boundary. Guard priority is stop-RIP → STOP → CodeEnd → budget → event. Thus terminal guards and runtime exits, backend failures, and invalid report accounting retain their existing result.

For cached register dispatch, a pre-entry request returns before cache binding, lookup, compilation, publication, or execution. After publication and cache validation, the dispatcher rechecks the full guard priority immediately before native entry. Thus a STOP raised during publication wins over an event raised at the same boundary. A request raised while a bounded native block runs is observed only after that block returns and its validated state/counters are committed; the dispatcher does not enter the next block. In the NOP-fallback dispatcher, the event is checked before each interpreter step, so at most one bare NOP step is performed between checks. Event state remains set on return. The host caller clears it explicitly and resumes using the actual guest RIP; the dispatcher does not rewind or replay committed work. The guest-work bound is one currently executing bounded native block or one bare NOP step; this contract makes no wall-time latency or performance claim. See [S3-28a evidence](../reports/STAGE_3_S3-28a_2026-10-29.md).

### S3-28b: Cooperative host event yield at bounded checked-memory boundaries

The checked-memory fixture dispatcher offers controlled `run_controlled` and `run_with_nop_fallback_controlled` entrypoints with an invocation-local optional `AtomicBool`. Existing public entrypoints remain event-disabled wrappers. `BoundedMemoryExit::EventPending` is host-only; it does not change raw V2/V3 ABI tags, `RuntimeContextV1`, generated code, or helper semantics.

Before compilation/executor entry and before each exact bare-NOP interpreter step, the dispatcher applies stop-RIP → STOP → fixture boundary → budget → event. The published executor repeats stop-RIP → STOP → budget → event after publication and immediately before native entry; the fixture boundary was checked by the dispatcher before compilation, and state remains unchanged until native entry. The published executor drops its just-published mapping before returning any pre-entry outcome. For callbacks during a running artifact the event is sampled only after that artifact returns. In particular, absent a higher-priority terminal exit, a two-site artifact completes both helper sites before event sampling; it may commit at most the pair's two retirements. STOP and architectural/backend failure can end a pair at their own established helper boundary, but the event is never polled between helpers. A valid returned report commits accounting and preserves actual RIP before the yield is returned. Architectural faults, `CodeInvalidated`, STOP, backend failures and malformed accounting are not hidden by an event. The request remains set; its caller clears it and resumes at actual RIP, without replaying committed work. This defines only a guest-work bound of one in-flight artifact (up to two instructions for a pair) or one bare NOP step; no wall-time latency/performance claim is made. Mini checks remain deferred through Stage 3. See [S3-28b report](../reports/STAGE_3_S3-28b_2026-10-29.md).

### S3-28c / S3-28d: STOP and event matrix, latency bound, shared budget, local STOP latency

Scope: one vCPU and the JIT path without SMP. The `od-machine` `StopAck`/`SafePoint` protocol is interpreter-only and is not claimed here. S3-28 accepted with limitations (Opus gate 2026-11-18). Nothing in this section changes product code; it records and tests what the code already does. New `unsafe` appears only in test code of `od-host` `src/lib.rs` (block emit, `transmute` to the entry pointer, the entry call carries a SAFETY comment); the integration tests use the safe production API.

**Mechanism.** Generated register blocks poll `stop_rip` → `stop_requested` → budget at every guest instruction boundary (`emit_instruction_guards`); V2/V3 memory artifacts poll STOP after a committed helper. A block that completes does not poll again: `BlockComplete` returns with the flag still set and the dispatcher's `control_exit` (stop-RIP → STOP → CodeEnd → budget) reports it before the next lookup, compile or entry. The event flag is host-only and is sampled by the dispatcher after `control_exit` (S3-28a/b), never by generated code.

**Case → test matrix.** Paths: *cached register* (`run_cached*`, `run_multi_cached*`, generated register blocks), *checked-memory* (`memory_dispatch`, one or two instruction artifacts), *NOP fallback* (`run_with_nop_fallback`, one bare NOP interpreter step), *paged session* (`PagedFetchedLoadSession`). Tests are in `od-jit` (`dispatch.rs`, `memory_dispatch.rs`, `memory_cache.rs`) and `od-host` (`src/lib.rs`, `tests/s3_28c_stop_budget.rs`). **New** marks tests added by S3-28c.

| Case | Cached register | Checked-memory | NOP fallback | Paged session |
| --- | --- | --- | --- | --- |
| (a) STOP before entry | `cached_stop_and_executor_error_release_only_at_safe_return_boundaries` (no backend call, resident handle kept), `cached_dispatch_samples_stop_at_guest_boundary_and_does_not_leak_default_stop`, `native_cached_guards_and_failures_preserve_publication_and_entry_counts`; real native: `runtime_boundary_checks_stop_address_stop_and_budget_in_order`, `s3_25d_small_capacity_lifecycle_stress_matches_interpreter_and_frees_everything` | `controlled_event_respects_stop_boundary_and_budget_priority`, `event_after_publication_skips_entry_and_stop_wins`, `entry_guards_do_not_publish_and_event_during_publish_does_not_execute` | `shared_control_stops_before_entry_or_before_interpreter_fallback` | **New (S3-38o)**, through the host loop `od_host::paged_host_loop`: `s3_38o_flag_raised_before_the_reread_runs_no_step` (flag raised before the loop rereads: no step, retired 0, no write, STOP beats finish), `s3_38o_stop_after_the_reread_is_a_normal_exit_without_replay` (byte raised after the reread, steps 0 and 1 native: the block exits `StopRequested` before its instruction, retired 0, no instruction effect; the next safe point sees the flag), `s3_38o_stop_before_a_native_store_with_a_new_value_leaves_no_data_trace` (native store of a new value, STOP before it: retired 0, data qword, data PTE Accessed/Dirty bits and write list untouched apart from the Accessed bits of the fetch walk; the resume stores once). The STOP byte belongs to the caller (`PagedHostLoopControl`) and is passed as `RuntimeContextV1::stop_requested` of every native call |
| (b) STOP at the k-th instruction of a 32-instruction block, exit exactly at boundary k | **New** `s3_28c_stop_during_instruction_k_of_a_32_instruction_block_exits_at_boundary_k` (k = 1, 2, 16, 31, 32 and an uninterrupted control): this path only (generated register block, helper raises STOP during instruction k; instructions 1..k committed, no helper of k+1 runs, RIP/RAX/flags/retired/budget exact; k = 32 completes the block with the flag still set) | not applicable: an artifact has at most two instructions; STOP inside a pair: `bounded_memory_dispatch_opt_in_bare_nop_bridge_preserves_budget_and_effects` (real native, STOP from the first read callback of the pair, retired (1, 0), RIP = BASE+3, no write of the second site, resume without replay); singleton instruction: `rbp_direct_rsi_rdi_dispatch_handles_partial_stop_and_actual_rip` | one step is the bound: `native_prefix_stop_is_observed_before_interpreter_nop` | **New (S3-38o)** one instruction per block: `s3_38o_stop_after_the_reread_is_a_normal_exit_without_replay` (STOP raised at step 0, 1, 2, 3 of the mixed loop: native steps stop before the instruction, interpreter steps (the interpreter never polls the byte) retire exactly one more; state equals the interpreter at that retired count, resume equals the interpreter run to the total, nothing repeated or skipped) |
| (c) STOP in a guest loop (several dispatcher entries) | **New** `s3_28c_stop_in_a_guest_loop_exits_after_the_current_block_and_resumes_exactly` (real native cached loop, STOP after the n-th block, n = 1, 2, 5: exactly n blocks, no next block, STOP retained, resume equals the interpreter, one publication); `cached_native_register_v1_branch_loop_and_nop_handoff_retire_exactly` (stop-RIP inside the loop) | `rbp_direct_rsi_rdi_dispatch_handles_partial_stop_and_actual_rip` (singleton: STOP, resume at actual RIP), `bounded_memory_dispatch_opt_in_bare_nop_bridge_preserves_budget_and_effects` (pair: STOP, resume at actual RIP without replay); `uncached_dispatch_selects_store_store_pair_and_resumes_at_second_site` (pair selection and resume at the second site, no STOP) | `native_prefix_nop_native_suffix_accounts_each_retirement_once`, `native_prefix_stop_is_observed_before_interpreter_nop` | **New (S3-38o)** `s3_38o_external_stop_from_another_thread_in_an_endless_guest_loop` (real native, STOP from another thread in an endless mixed guest loop, three rounds: exit `StopRequested`, retired within 2 of the progress the requester read right after `request_stop` (a coarse sanity bound), state equals the interpreter at the retired count, resume equals the interpreter). The exact bound, no instruction after the iteration that took the byte down, is proved by the protocol and by the deterministic hook tests of columns (a) and (b) (`s3_38o_stop_after_the_reread_*`), not by this test |
| (d) STOP next to a helper or fault | STOP next to a helper: `runtime_samples_stop_set_by_successful_helper_after_commit`; fault of a helper (STOP stays 0): `helper_failure_after_prefix_keeps_exact_commit_rip_and_map_site`; `s3_55_helper_only_budget_stop_event_and_zero_progress` | `s3_25d_*` (STOP from a committed store compared with the interpreter), `tracked_v2_store_invalidates_alias_after_return_and_preserves_late_stop_commit`, `active_code_v3_guards_faults_aliases_and_committed_stop_are_precise` | not applicable (no helper) | since S3-38o `StopRequested` of the single-instruction block is an ordinary result of the loop, not `JitHostFailure(InvalidOutcome)`: retired 0 (byte seen before the instruction, no effect, the instruction runs once after the flag is withdrawn: `s3_38o_a_byte_without_a_flag_is_harmless`, `s3_38o_stop_after_the_reread_*`) or retired 1 (byte seen after the instruction, never repeated; accounted by `decode_memory_operation_v2_result` and by the loop counters). No deterministic loop test raises the byte inside the helper, so the retired-1 variant has no loop test of its own; an architectural fault of the operand ends the loop with `GuestFault` and equals the interpreter (`s3_38o_guest_fault_of_the_native_form_equals_the_interpreters`); recovery after a host fault is S3-27 (open) |
| (e) budget exhaustion | `cache_config_and_zero_budget_do_not_publish`, `code_end_precedes_zero_budget_and_zero_budget_does_not_execute`, `one_entry_cache_reuses_same_key_and_self_loop_consumes_budget`; real native: `cached_native_register_v1_branch_loop_and_nop_handoff_retire_exactly` | `invalid_unsupported_truncated_and_zero_budget_boundaries_remain_distinct` | `all_nop_fallback_preserves_flags_and_counts_and_obeys_budget_guards`; **New** `s3_28c_native_blocks_and_interpreter_steps_share_one_budget` | **New (S3-38o)** `s3_38o_budgets_zero_to_n_on_the_mixed_program_equal_the_interpreter` (budgets 0..13 on a mixed native/interpreter loop equal the pure interpreter: state, RAM, write list; `retired_jit + retired_interpreter + remaining == incoming`), `s3_38o_mixed_program_with_a_guest_loop_matches_the_interpreter_on_both_paths` (103 instructions: 52 native, 51 interpreter, both shares recorded and nonzero) |
| (f) STOP wins over an event | `cached_publication_boundary_rechecks_stop_priority_before_event`, `event_request_obeys_stop_code_end_budget_and_error_priority` | `event_after_publication_skips_entry_and_stop_wins`, `completed_artifact_terminal_exit_and_stop_are_not_masked_by_event` | `event_request_obeys_stop_code_end_budget_and_error_priority` (uses `run_with_nop_fallback`) | **New (S3-38o)** `s3_38o_event_exits_on_the_safe_point_before_the_next_step_and_stop_wins` (event raised during step 3: exit after exactly three retired instructions, before the fourth; the flag is left raised and the next call exits at once; budget 0 is reported before the event; STOP is reported before the event) |

The *paged session* column was filled by S3-38o (accepted with limits after the Opus high gate review, 2026-11-18): the production host loop of one vCPU, `od_host::paged_host_loop::run_paged_host_loop`, wires the session to an external STOP byte, request flags, a shared budget and the shared paged interpreter. `PagedFetchedLoadSession` itself still executes one instruction per call; the caller is the loop.

**S3-38o: the caller of `PagedFetchedLoadSession` (one vCPU, one thread).**
- *Owner of the byte.* `PagedHostLoopControl` (caller-owned, `Sync`: atomics only) holds the STOP `AtomicU8` and the request flags `stop`, `finish`, `event`. The loop passes `control.stop_byte()` to every native call as `RuntimeContextV1::stop_requested` (`PagedFetchedLoadSession::execute_forms_with_stop`); the session never owns, sets or clears it.
- *Iteration* starts on the S3-25c safe point (native call returned, result and counters checked, the walk writes of the step delivered to the caller's `committed_write`, no lookup started): (1) take the byte down (`swap(0, SeqCst)`) and reread the flags (`SeqCst`), once per iteration (a JIT step or the fallback interpreter step): the flags are not reread between a JIT refusal and the interpreter step that follows it, which stays within the bound of one instruction; (2) exit by priority `StopRequested`, `FinishRequested`, `BudgetExhausted`, `EventPending`; (3) one step: the session's native path for the admitted forms; if the session refused without effects of the instruction (unsupported form, CPU context, operand range, fetch fault, compile/publish refusal, non-RAM operand), exactly one step of the shared paged interpreter, `od_machine::run_paged_probe` with a budget of one (the interpreter `FirmwareBoard::run_quantum` uses). The loop does not clear a flag; only the loop takes the byte down.
- *Protocol* (S6-40 section 5, S3, steps 1-2, single-thread form). Requester: flag (`SeqCst`), then byte (`SeqCst`) (`request_stop`, `request_event`, `request_finish`); the requester withdraws its own flag (`clear_*_request`). A flag raised before the reread means no step runs. A byte raised later is seen by the native block at its guard: `StopRequested` with retired 0 (before the instruction, no effect) or retired 1 (after it); the instruction is never repeated, the next safe point sees the flag. A byte without a flag is harmless. S6-42 adds `stop_all` and `split_owner` to the reread; the order does not change.
- *Budget.* One counter: `retired_jit + retired_interpreter + remaining == incoming` (`PagedHostLoopReport::budget_is_consistent`).
- *Writes.* Both paths write guest RAM only through `commit_write` inside the bus (the page generation grows); the loop delivers the walk writes of the fetch and the operand, native and interpreter alike, in order.
- *Executable memory.* The loop owns no region and no lease; the `MapJitLease` lives in the session's `PublishedCode` and ends when the session is reset or dropped (M6); a new session starts a new loop (`s3_38o_the_map_jit_lease_ends_with_the_session_and_a_new_loop_runs`).
- *Effects of a refused or stopped attempt.* A native attempt that exits before its instruction has already performed the tracked instruction fetch: the Accessed bits of the fetch walk are committed (idempotent, reported in the write list). The interpreter reference that retired nothing has not fetched; the tests clear only the Accessed bit (0x20) of the four fetch-walk qwords, and only for a stop at step 0, and check the write list explicitly (fetch-walk Accessed writes and nothing else); from step 1 on no normalisation is applied.
- *Limits (not extended by S3-38o).* One vCPU, one thread; the session, bus and CPU state are not `Send`/`Sync` and no thread is introduced by the loop. JIT profile: one instruction per block; a block is dropped by every unsupported form, so a mixed loop re-admits each native form (no block reuse across interpreter steps). The admission still maps one region per session admission (slots of a region: S6-42). The shared interpreter's instruction set is the board's narrow profile (`MOV RAX,[RBX]`, `MOV [RBX],RCX`, stack forms, `RET`, `HLT`, port I/O and a few exact firmware forms), so the forms admitted only by the JIT (`48 89 03`, `8B 03`, displacement and RIP-relative loads) have no interpreter fallback and no oracle: an unsupported form ends the loop with `Interpreter(Unsupported)`. S3-27 (host fault recovery in generated code) is open. S3-38 is not closed by this slice.
- *S4-16i with an observed STOP (closed at the S3-38o gate, 2026-11-18).* A store block reads the STOP byte after its commit, so `StopRequested` with retired 1 (the byte raised by another thread during the helper) is reachable. The exit of a store into the active code page is rewritten to `CodeInvalidated` (and the admitted block is marked retired) when the store retired: `Complete`, or `StopRequested` with retired 1 (`store_invalidates_active_code` in `lib.rs`, unit test `a_retired_store_into_the_code_page_invalidates_even_when_stop_was_observed`). The STOP is not lost: native code only reads the byte, and the loop clears it and re-reads the request flags at the next safe point (S6-40 §5 S3); only the diagnostic counter `native_stop_exits` misses that one exit. Stale reuse is in any case closed by the generation in the admission key. No deterministic loop test raises the byte inside the helper.

**Latency bound in guest work.** STOP: exactly ONE instruction, also inside a pair: for a register block the next generated boundary guard (at most one instruction retires after the flag is raised, tested by `s3_28c_stop_during_instruction_k_*` with exactly `k` retired); for a two-site memory artifact STOP is polled after the first site, on the boundary before the second site (never between the two helpers inside one site; tested by `bounded_memory_dispatch_opt_in_bare_nop_bridge_preserves_budget_and_effects`, retired (1, 0)). The "pair (at most 2 instructions)" bound applies only to the EVENT, not to STOP. A completed block that ends with the flag set is stopped before the next block (`s3_28c_stop_in_a_guest_loop_*`; the register state is the same at every block boundary because the native register profile admits only RAX-based forms and `JZ` needs ZF set, so the exactness of the stop point rests on the block, retirement and publication counters and on the interpreter comparison). Event: one bounded block (at most 32 instructions by `MAX_BLOCK_INSTRUCTIONS`), one memory artifact (a pair, at most two instructions), or one bare NOP step (`event_pending_after_native_cache_miss_and_hit_preserves_request_until_cleared`, `event_after_jcc_yields_before_nop_and_resumes_without_replaying_prefix`, `controlled_event_yields_before_work_and_after_one_committed_artifact`). Limit found by S3-28d: the compile pipeline rejects emitted code above `MAX_EMITTED_BLOCK_BYTES = 4096` (`EmittedCodeTooLarge`); the number of admitted instructions depends on the instruction forms and is bounded by `MAX_EMITTED_BLOCK_BYTES = 4096`; for example ten MOV give 3868 bytes (`ten_mov_block_remains_inside_the_unchanged_code_size_limit`, `block_compile.rs` ~2150), and the measured form (`NOT RAX` plus the guards) stopped fitting above 9 instructions. The 32-instruction block of case (b) is exercised through the emitter directly. The event bound therefore stays "one block", at most 32 instructions by the contract limit and fewer when the emitted code reaches the byte limit first. Hardware timeouts are not an exact guest STOP.

**Budget shared by blocks and interpreter.** Generated code decrements the budget once per retired instruction (`remaining = incoming - retired`, no other budget exists); the dispatcher checks every report with `validate_counters` (`RetiredExceedsBudget`, `RetiredExceedsBlock`, `InconsistentRemaining`) and passes `report.remaining_budget` to the next block or interpreter step; each bare NOP step decrements it by one. Tests: `forwards_remaining_budget_and_dispatches_iteratively_to_code_end`, `rejects_invalid_counters_and_zero_progress_continuation`; the end-to-end check is **new** `s3_28c_native_blocks_and_interpreter_steps_share_one_budget` (real native blocks and NOP interpreter steps, budgets 0..7 on `ADD; NOP; ADD; NOP; ADD`, equal to the instruction-by-instruction interpreter, `retired_native + retired_interpreter + remaining == incoming`, cached and uncached results equal).

**S3-28d: local STOP wall-time latency (LOCAL, not Mini, no promise of milliseconds).** Measured on the development machine (Apple M3 Max, macOS aarch64, `uname -srm`: Darwin 25.6.0 arm64), one worker thread running the cached register dispatcher over a guest loop that never ends, a second thread storing `stop_requested = 1` after a pseudo-random 0 to 200 microsecond delay once the worker had run at least 1000 blocks; latency = time from just before the store to just after the dispatcher returned `StopRequested`. 1000 runs per configuration; raw values and summaries are saved in `docs/reports/data/stage3/s3-28d/`. Command: `cargo test -p od-host [--release] --offline --test s3_28d_stop_latency -- --ignored --nocapture --test-threads=1` (ignored test, not part of the normal suite). The clock tick observed is about 42 ns (smallest non-zero sample, monotonic `Instant`).

| Configuration | Profile | median | p95 | p99 | max |
| --- | --- | --- | --- | --- | --- |
| `loop2`: 2-instruction block (ADD; JZ) | release | 459 ns | 833 ns | 959 ns | 21458 ns |
| `blockmax`: 9-instruction block (8 NOT RAX + JZ) | release | 1208 ns | 2416 ns | 5334 ns | 22709 ns |
| `loop2` | debug (default test profile) | 3084 ns | 6000 ns | 18583 ns | 29333 ns |
| `blockmax` | debug | 8500 ns | 15625 ns | 21666 ns | 54458 ns |

The numbers describe this machine and this build only. They include thread scheduling noise in the maxima, exclude any wake-up of an idle worker, and are not a Mini result or a latency guarantee. They are an observation that the bounded-work design stops a busy worker within microseconds here, nothing more.

Limits: one vCPU; JIT path without SMP; no timer, APIC or event delivery is claimed; paged session has no cooperative STOP/event input; the measured guest forms are register-only.

### S3-20ga: exact address32 RCX-base Dword store with signed disp32

Runtime commit `b7b361b` and harness commit `c2a3e8f` are integrated and passed
independent Sol/high static review. Behavior execution remains unrun. The
profile admits only seven-byte `67 89 81 id`
(`MOV [ECX+signext(disp32)],EAX`): `67` is the sole prefix, data and address
sizes are Dword, RCX is the addr32 alias base, there is no index, and
displacement is signed little-endian i32. The starting effective address is
`zero_extend((low32(RCX) + signed_i32_disp32) mod 2^32)`. Perform one
contiguous checked four-byte write of low32 pre-state RAX; preserve every GPR
and RFLAGS and commit successful RIP `A+7`. Only the starting EA wraps at
32 bits; transfer bytes after `0xffffffff` continue contiguously above 4 GiB.

Do not include the matching addr32 RCX disp32 load, singleton or disp8 store,
other bases, SIB/index, additional prefixes or REX, or any other encoding.
Supported neighboring forms keep distinct source/site identities. Existing
memory/backend fault behavior, entry guards, retry, and committed callback
STOP without replay remain required. Published IR uses immutable
compiler-issued source bytes and does not fetch guest instruction bytes at
runtime; instruction-fetch errors are checked through the demand interpreter
path. See the [S3-20ga report](../reports/STAGE_3_S3-20ga_2026-11-02.md).

### S4-01b: LOCK MOV CR0 decode boundary in JIT consumers

The exact decoder error `InvalidMovCr0Lock` (`F0 0F 22 C0`, length 4) remains
architecturally invalid. The block builder stops at that source RIP and keeps
the typed decode error in `UnsupportedEncoding`; previously decoded
instructions and their exact guest ranges are preserved. Deferred decode
boundaries retain the consumed encoded length as well. The bounded memory
dispatcher returns `Unsupported` at the current RIP before invoking its memory
executor, and does not continue into subsequent source bytes. This is an
integration classification only: it does not admit LOCK MOV CR0 to block/JIT
execution or change the CPU profile's #UD behavior.

The exact LOCK MOV CR3 boundary remains unchanged. Ordinary invalid/unsupported
encodings, truncated source, and byte/instruction budget boundaries retain
their existing classifications. CPU #UD behavior is covered by the existing
`od-isa::execute::paged_mov_profile_tests::lock_prefixed_mov_cr0_is_ud_after_complete_fetch`
and `od-isa::execute::paged_cr3_probe_tests::paged_cr3_profile_keeps_lock_mov_cr0_as_ud`;
this JIT slice does not alter CPU semantics. See
[S4-01b evidence](../reports/STAGE_4_S4-01B_2026-11-06.md).

### S4-15: JIT on the production memory owner (closure, S4-15g)

This section is the single contract for the JIT on the production memory owner
(`od_host::PagedFetchedLoadSession` over `od_machine::MachineBus`). The
S4-15b-f subsections that follow are slice-level detail and must not be read as
extending or narrowing it. Scope: one vCPU, macOS arm64 (MAP_JIT), Long64,
CPL0/CPL3, 4 KiB pages, one instruction per block, synchronous release.

**What the JIT compiles and executes.** One instruction per block, compiled from
the bytes fetched from guest RAM (never from a caller constant) and published
into the process's single MAP_JIT region. The memory operand is never reached
through a host pointer: it goes through a scoped `GuestMemory` over the tracked
RAM walker (`with_paged_mov_memory`, `with_paged_mov_dword_memory`,
`with_paged_mov_store_memory`), which commits Accessed/Dirty bits and data as
effects and refuses MMIO, ROM, framebuffer, cross-page and 2 MiB/1 GiB-leaf
operands before any committing walk.

| Form | Bytes | Operand | JIT (`execute_forms`) | JIT in `execute_or_interpret` | Interpreter `step_paged_mov` | Tests |
|---|---|---|---|---|---|---|
| `MOV RAX,[RBX]` | `48 8B 03` | qword | yes | yes | yes (allowlist) | `s4_15a`, `s4_15b_*`, `s4_15c_*`, `s4_15e_the_existing_forms_*` |
| `MOV [RBX],RAX` | `48 89 03` | qword | yes | yes (no fallback guarantee) | no in `step_paged_mov`: explicit `Unsupported` (allowlist has `48 89 0B` only) | `s4_15d_*`, `s4_15e_the_existing_forms_*` (oracle `48 89 0B`, RCX=RAX) |
| `MOV EAX,[RBX]` | `8B 03` | dword, zero-extended | yes | no (`UnsupportedForm`) | no (a separate dword profile exists; the dispatcher does not call it) | `s4_15e_dword_*` |
| `MOV RAX,[RBX+disp8]` | `48 8B 43 ib` | qword | yes | no | no | `s4_15e_qword_displacement_*` |
| `MOV RAX,[RBX+disp32]` | `48 8B 83 id` | qword | yes | no | no | `s4_15e_qword_displacement_*` |
| `MOV RAX,[RIP+disp32]` | `48 8B 05 id` | qword, from the next RIP | yes | no | no | `s4_15e_rip_relative_*`, `s4_15e_reuse_key_*` |
| `67`-prefixed forms, store with displacement, dword with displacement | | | `UnsupportedForm` | `UnsupportedForm` | no | `s4_15e_forms_outside_the_allowlist_*` |
| `48 89 0B` (store RCX) | | qword | `UnsupportedForm` | `UnsupportedForm`, then interpreted | yes (allowlist) | `s4_15c_unsupported_forms_*` |

The two JIT-only entries differ: `execute` admits only `48 8B 03`;
`execute_with_store` and `execute_or_interpret` admit `48 8B 03` and `48 89 03`;
`execute_forms` admits every JIT row above. Of these only `execute_or_interpret`
has the interpreter fallback; a form that the JIT refuses there and that the
interpreter does not execute ends in the interpreter's explicit
`RunExit::Unsupported` (0 retired, RIP/RAX untouched, reported, never a silent
success). That is the case for `48 89 03` after a refused store (non-RAM target,
untranslatable operand; a store into the block's own code page is not refused
since S4-16i, see below). For the forms that only
`execute_forms` admits this follows from `execute_or_interpret` calling
`execute_with_store`; there is no dedicated dispatcher test for them.

**How the path is chosen (`execute_or_interpret`).** In order:
(1) preflight without a page walk: CPU context, CR3, paging controls, operand
range for the legacy forms (`execute_forms` checks the operand range after the
fetch); (2) tracked instruction fetch at RIP (execute/NX/U/S for the CPL; code
Accessed bits are committed and reported) giving bytes, physical page and
`PhysicalMemory::code_page_generation`; (3) admission: the block is reused only
while RIP, physical page, generation and bytes all match, otherwise the stale
block is dropped first and the form is compiled again, and bytes outside the
allowlist are `UnsupportedForm` and never run a stale or constant block;
(4) for a store, the read-only probe of the operand translation (no A/D);
(5) the data walk (A bits, then for a store the leaf D bit) and execution of
the operand bytes. A refusal before step (5) commits nothing but the fetch's
and, at most, a data walk's Accessed bits, leaves RAX, RIP, RFLAGS and guest
data untouched and reaches no device, so the step goes once to
`od_machine::step_paged_mov` and the reported write list equals a pure
interpreter step. An executed JIT step (completed, or a guest `#PF` with its
committed Accessed bits and, for a store, before the Dirty bit and the write)
is final and is never repeated by the interpreter. Any other JIT helper failure
is `PagedStepError::JitHostFailure` and is not retried.

**Release.** The session owns its block as `PublishedCode`, i.e. the process's
single MAP_JIT region. It is released by `reset(&mut self)` at a safe point
between calls, by a later call that drops a stale block, or by dropping the
session; a call's block lives until it returns; after a reset the next entry is
`Admitted`. `reset` on an empty session is a no-op and cannot release a region
owned by another session. A bounded-arena handle dropped under a shared borrow
defers its release to the next mutable arena access instead of leaking the slot.

**Verified.** Integration tests `tests/system/stage4_memory_fault.rs`: 39
`s4_15{b,c,d,e,f}_*` tests (6 + 7 + 8 + 13 + 5) plus `s4_17a`, 40 in the file;
lib tests in `od-host`: `s4_15a_*` (1) and `s4_15f_*` (3), 4 in total. Every
executed JIT step is compared with the interpreter (or, where it has no form,
with an equivalent interpreter instruction: `48 89 0B` with RCX=RAX for the
store, `48 8B 03` with RBX set to the effective address for displacement and
RIP-relative loads) on 16 GPRs, RIP, masked RFLAGS, `PageFaultRecord`, the
ordered committed writes and the whole RAM image. Covered: CPL0/CPL3, guest
`#PF` before and after allowed effects, MMIO operand reaching the device exactly
once through the interpreter, refused stores without effects, bytes/page/
generation re-admission, executable-region release at reset/drop/invalidation.
Source reports: [S4-15a](../reports/STAGE_4_S4-15A_2026-11-06.md) and the
S4-15b-f sections of `docs/reports/STAGE_4_REPORT.md`.

**Not done (limits of S4-15).**

- Multi-instruction blocks; one instruction per call.
- `67`-prefixed forms, store with displacement, dword with displacement: not in
  the `step_paged_mov` allowlist, so the JIT is not extended on its own
  authority; `execute_forms` is not wired into `execute_or_interpret`.
- `48 89 03` (`MOV [RBX],RAX`): not in the one-step paged profile
  `step_paged_mov` (allowlist `is_paged_mov_instruction`: `48 8B 03` and
  `48 89 0B`). The forms `48 89 03` and `8B 03` do execute in od-isa (the flat
  s2 interpreter, the dword profile `step_paged_mov_dword_read`, the paged CPL0
  set on the board), but the fallback to them is not wired from the dispatcher.
  The slice with a fallback guarantee is `48 8B 03` (and `48 89 0B` is executed
  by the interpreter); `48 89 03` is a JIT extension WITHOUT a fallback
  guarantee on refusal (the refusal is explicit, `Unsupported`, no silent
  effect). Owner of the remainder: S4-42 (fallback to the paged CPL0 set on the
  Linux trace) or a separate od-isa/CPU_CONTRACT sub-slice if a one-step paged
  profile with `48 89 03` is needed (outside the "Where" of S4-15).
- 2 MiB (and 1 GiB) code pages: the tracked fetch goes through the 4 KiB walker
  and refuses `Fetch(Access(UnsupportedPageSize))`; code crossing a 4 KiB page
  is `Fetch(InvalidLength)`; cross-page and 2 MiB-leaf operands are refused.
- MMIO, ROM and framebuffer operands through the JIT (MMIO only through the
  interpreter, and a refused `48 89 03` is not executed by the dispatcher's interpreter, see the `48 89 03` limit above).
- SMP, deferred retirement with a grace period (S3-25, G7); release is
  immediate on one vCPU. One MAP_JIT region per process: a live block of another
  session makes `Publish` refuse (`MappingAlreadyLive`).
- Integration into the runner and the Linux trace; `reset` is called only
  explicitly.
- Performance: no speed claim; the cost of the repeated tracked fetch and of
  the probe plus the walk on a store was not measured.
- Store into the active block (S4-16i, done for the single-store block of
  `PagedFetchedLoadSession`: see the S4-16i section); aliases and cache
  invalidation are S4-16.
- `JitHostFailure` branches have no test (not reachable from the public API).

### S4-15b: fetch-snapshot admission from guest RAM

`od_host::PagedFetchedLoadSession` admits the exact `48 8B 03` (`MOV RAX,[RBX]`)
from bytes read out of guest RAM instead of a caller-supplied constant. Each
call performs a tracked instruction fetch of the 3 bytes at RIP through the
machine bus page walker (execute/NX/U/S for the caller's CPL; code Accessed
writes commit as effects and are reported), recording the physical page and its
`PhysicalMemory::code_page_generation`. The block is compiled and published
from the snapshot bytes with `entry_rip` = RIP. A cached block is reused only
while RIP, physical page, generation and bytes all match; otherwise it is
dropped before the new admission (one executable region) and any bytes other
than `48 8B 03` are refused without running a stale block. Fetch rights are
re-checked on every call. The data operand still runs through
`with_paged_mov_memory` with scoped `GuestMemory`; no host pointer to guest RAM.
Fetch faults return the exact `#PF` record before any operand walk. Stores,
other forms, reset/retirement and interpreter fallback were outside this slice
and were added in S4-15c-f; cross-page code was not. The contract for the
production owner as a whole is the section "S4-15: JIT on the production
memory owner" above (S4-15g).

Limit: re-admission and fetch work only for 4 KiB code pages.
`tracked_code_snapshot_with_callback` goes through the 4 KiB walker
(`access_long64_4k_ram`); a PDE with PS=1 yields the typed refusal
`Fetch(Access(UnsupportedPageSize))`, possibly with Accessed bits of the upper
levels already committed. The Linux kernel text is usually mapped with 2 MiB
pages, so on a real kernel this path will almost certainly refuse. Support for
2 MiB code pages was not added by S4-15; it remains open (see the limits in
the S4-15 section above).

### S4-15c: one-step fallback dispatcher

`PagedFetchedLoadSession::execute_or_interpret` runs the S4-15b JIT path and,
when that path refuses without having executed the instruction, runs
`od_machine::step_paged_mov` exactly once on the same state and bus; the result
names the path (`PagedStepPath::{Jit, Interpreter}`). An executed JIT step
(completed, or a guest `#PF` with its committed Accessed bits) is final and is
never repeated. A refusal before execution (CPU context, operand range,
instruction fetch including a fetch `#PF`, unsupported form, compile or publish
refusal, or a RAM-only adapter refusal of the data operand such as MMIO) leaves
RAX, RIP, RFLAGS and guest data untouched and reaches no device; its only
possible effects are page-walk Accessed bits, which a second walk does not
commit again, and the refused attempt's writes are prepended to the reported
list so it equals a pure interpreter step. Any other JIT helper failure is
returned as `PagedStepError::JitHostFailure` and is not retried. The
interpreter's own allowlist (`48 8B 03`, `48 89 0B`) still applies: other forms
end in its explicit `Unsupported` exit. Store JIT, new forms and reset/retirement
were added in S4-15d-f; the consolidated contract is the S4-15 section above.

### S4-15d: tracked store `48 89 03` on the production owner

`PagedFetchedLoadSession::execute_with_store` (and `execute_or_interpret`) also
admits exactly `MOV [RBX],RAX` (`48 89 03`) from the same tracked fetch snapshot
and reuse key as the load. The store runs through `with_paged_mov_store_memory`,
a store-only `GuestMemory` over `MachineBus::access_tracked_ram`: the page walk
checks Present, Write (CR0.WP at CPL0) and User (CPL3), NX does not apply, then
Accessed bits, the leaf Dirty bit and the 8 operand bytes are committed in that
order through the committed-write callback. The `unsafe` contract of
`execute_memory_store_v2` holds because the adapter writes only ordinary RAM
(a read-only probe refuses MMIO, ROM, framebuffer and any operand the probe
cannot translate, e.g. a 2 MiB leaf, before any walk that commits), never reaches a device, runs under an exclusive `&mut MachineBus`,
relies on `PhysicalMemory::write_bytes` validating the whole range first, never
calls back into the runtime (the compiled block itself lives in the host
MAP_JIT mapping, so memory safety never depended on the page the store lands
on). S4-15d refused an operand on the guest page the block was fetched from;
**S4-16i replaced that refusal**: such a store now commits and ends in the exit
`CodeInvalidated` (section S4-16i). The host helper is crate-private; the load-only
`execute` entry refuses `48 89 03` even when a store block was admitted through
`execute_with_store` on the same session.
A guest `#PF` precedes the Dirty bit and the write and is a final JIT result. A
refusal (`StoreTargetRefused`: non-RAM target or an
operand the probe cannot translate) commits no data-walk bit, no Dirty bit and
no data; only the fetch's Accessed bits remain, so the merged write list and the
RAM image equal a pure interpreter step. It falls back to one `step_paged_mov`,
whose allowlist lacks `48 89 03` and reports `Unsupported`. A store into another code page is allowed:
the next entry re-fetches and compares bytes, page and generation. A store into
the active block was not part of this slice and is S4-16i (it replaced the
refusal described in the S4-15d report); more forms, reset/handle
and the closing contract text were added in S4-15e-g.

### S4-15e: wider operand forms through `execute_forms`

`PagedFetchedLoadSession::execute_forms` admits, from the same tracked fetch
snapshot and the same reuse key (RIP, physical page, generation, bytes), the
forms below; the S4-15b/c/d entries are unchanged. The instruction length is
not fixed: 2 bytes are fetched first, then 3, then 4 or 7 as the opcode needs
(code that does not fit the 4 KiB page of RIP is a `Fetch(InvalidLength)`
refusal, not a fault). The effective address is the full 64-bit base (RBX, or
the RIP of the *next* instruction) plus the sign-extended displacement,
computed on every call from the CPU state, never stored in the block; the host
executor checks the published metadata against the form and cross-checks the
decoder's address. The operand must lie inside one 4 KiB page (checked after
the fetch, before any data walk).

| Form | Bytes | Operand | JIT | Interpreter paged allowlist |
|---|---|---|---|---|
| `MOV RAX,[RBX]` | `48 8B 03` | qword | yes | yes |
| `MOV [RBX],RAX` | `48 89 03` | qword | yes (S4-15d) | no (`48 89 0B` only) |
| `MOV EAX,[RBX]` | `8B 03` | dword, zero-extended into RAX | yes (`with_paged_mov_dword_memory`) | yes (dword profile) |
| `MOV RAX,[RBX+disp8]` | `48 8B 43 ib` | qword | yes | no |
| `MOV RAX,[RBX+disp32]` | `48 8B 83 id` | qword | yes | no |
| `MOV RAX,[RIP+disp32]` | `48 8B 05 id` | qword | yes | no |
| `67`-prefixed forms, stores with a displacement, dword with a displacement | | | `UnsupportedForm` | no (wait for `od-isa`) |

There is no interpreter fallback on this entry: the interpreter has no
displacement or RIP-relative form. Multi-instruction blocks, 2 MiB code pages
and the dispatcher integration (`execute_or_interpret` still uses only
`48 8B 03` / `48 89 03`) are not part of this slice or of S4-15; reset/handle
and the closing contract text were added in S4-15f-g.

### S4-15f: session reset and release of the executable region

`PagedFetchedLoadSession` owns its admitted block through `PublishedCode`, i.e.
the process's single MAP_JIT region, not through an arena slot. The region is
released only (1) by `PagedFetchedLoadSession::reset(&mut self)`, (2) when a
later call drops a stale block (changed bytes, physical page or write
generation) before compiling, or (3) when the session drops. Dropping the block
unmaps the region and ends the process-wide lease. `reset` is a safe-point
operation between calls: like every execute entry it takes `&mut self`, so it
cannot run inside an executing block, and a call's block stays alive until that
call returns. After a reset the next entry admits the form again (`Admitted`).
`reset` on an empty session does nothing and cannot release a region owned by
another session. Guest CPU state and RAM are not touched. `map_jit_region_live()`
exposes the lease. Scope: one vCPU, synchronous release; SMP and deferred
retirement with a grace period (S3-25, G7) are not part of this slice.

For the bounded arena, a `PublishedArenaBlock` that drops while the arena is
borrowed (for example from an owner callback during a native call, where the
executing handle holds the shared borrow) never frees its slot under that
borrow and never leaks it: the release is recorded and applied at the next
mutable arena access, and `available_slots()` counts it as free. Releasing a
slot that is already free, or of a stale generation, is a no-op.

### S4-16a: production tracked loader patch for two inactive code aliases

`od-host::PagedCodeSession` borrows one `MachineBus` exclusively and fixes one
Long64 paging snapshot for its lifetime. It accepts at most two linear aliases
of one physical RAM page and one exact `NativeRegisterV1` code range contained
in that guest 4 KiB page. It owns two one-entry caches and a two-slot published
arena. The cache uses the existing backing-content invalidation contract; no
new general cache policy is introduced. Backing and mapping IDs are local to
the bus-bound session, and the backing version is the `PhysicalMemory` version
of the physical guest page.

Before lookup, each alias gets a fresh owned code snapshot from
`MachineBus::tracked_code_snapshot`. Its first byte is fetched through the
tracked Long64 walker with `AccessOrigin::Explicit`, which checks execute
permissions at the supplied CPL and commits actual A updates; descriptor-table
origin is rejected because it would apply supervisor page permissions. The
snapshot preserves the range's page offset. Both aliases must map the same
physical byte range, and a loader patch must lie in
`[range_offset, range_offset + code_length)`. The session rejects another
physical page, third alias, changed source length/offset, or non-register-only
admission. It binds backing, range and alias only after admission succeeds.
The selected cache entry is invalidated against the fresh backing version
before dispatch. The loader patch API writes one byte through a selected
writable guest alias after a tracked mapping preflight; a committed write
advances that physical page's version and invalidates both alias caches.

A failed operand write does not commit the requested patch byte. Either
tracked walk may commit Accessed effects; only the write walk may commit Dirty
effects before a later failure. If page-table storage physically overlaps the
code backing, those effects can change code bytes and their page generation even
when the patch is rejected. That overlap is outside this slice's fail-with-unchanged-code
guarantee. Every later lookup captures fresh bytes and generation and
invalidates the selected alias entry before dispatch, preventing execution of
an obsolete cached block. Before S4-16d, another idle alias entry could remain
resident after a page-table A/D write until its next lookup. S4-16d forwards
tracked snapshot and patch callbacks into synchronous invalidation of both idle
aliases when the committed write overlaps their code backing.

The existing synchronous NativeRegisterV1 dispatcher is fixture-profiled and
rejects a populated CR3. This bounded host adapter runs it against a local copy
of `LongModeState` with CR3 cleared only for the admitted register-only call,
then restores the original CR3 into the returned state. Thus an unwind leaves
the caller's state untouched. Published block allocation and PC map remain
owned by the selected cache through native execution and result decoding.

The macOS ARM64 host test warms both aliases, rejects a read-only patch without
dropping either entry, changes one immediate byte through alias A, verifies
both entries are removed, then executes through B and A and observes the new
RAX value. This is one-vCPU loader-patch coverage only. Guest JIT stores,
general writer notifications, remap/TLB changes, cross-page code, MMIO/DMA,
SMP and active-block mutation remain outside this slice; parent S4-16 stays
open. Per-page generation adds 8 bytes per RAM guest page and updates touched
pages on committed writes. It is provisional: no performance measurements or
speed claim are available. See the [S4-16a report](../reports/STAGE_4_S4-16A_2026-11-06.md).

### S4-16b: one interpreted guest store invalidates inactive aliased code

`PagedCodeSession::step_guest_mov` runs the existing one-instruction
`od-machine::step_paged_mov` profile synchronously between native calls. The
host session requires its fixed Long64 mode, CR0/CR3/CR4 snapshot and preserves
the caller's CPL. CR3 is passed separately to the machine adapter, while the
interpreter receives a local clone of `LongModeState` with CR3 cleared, because
the profile takes CR3 as a separate paging argument. After a successful machine
input result, the original CR3 is restored before the clone is copied back to the
caller. A machine input error leaves caller CPU state unchanged; an architectural
fault follows the interpreter profile's normal state-commit semantics. The
interpreter profile remains limited to its exact admitted MOV encodings; this
slice tests `48 89 0B` (`MOV [RBX],RCX`).

The adapter collects physical ranges from actual committed-write callbacks,
including page-walker Accessed/Dirty updates. It reconciles those ranges with
the captured physical code page before propagating a machine-step error or
returning its guest report. If any committed write overlaps the code page, it
reads the current per-page generation and invalidates both alias cache entries
synchronously. Thus a later page fault does not discard an earlier committed
page-table update or leave stale code resident. A write confined to another
RAM page leaves the code generation and both entries unchanged.

The existing machine step may also dispatch an operand access to registered
MMIO. This slice validates RAM-backed stores only; MMIO behavior is not tested.
Cache reconciliation consumes committed physical RAM write callbacks and does
not represent device-side writes.

Acceptance covers: successful code mutation through the guest store and fresh
execution through both aliases; a CPL3 store whose PML4 Accessed update
overlaps the code backing and precedes an operand #PF, with zero retirement,
unchanged CPU state, no operand write, and both caches invalidated; and a
successful unrelated-page store that preserves the code generation and cache
entries. The code range remains within one page under S4-16a's snapshot
contract. This slice does not test cross-page data operands or assert their
behavior. General writer integration, remap/TLB coupling, active-block
mutation, MMIO behavior, DMA, SMP, and broader instruction admission remain open under
parent S4-16. Performance is not measured here and no speed claim is made. See the
[S4-16b report](../reports/STAGE_4_S4-16B_2026-11-06.md).

### S4-16c: synchronous physical RAM loader write invalidates inactive aliases

`MachineBus::patch_physical_ram_with_callback` preserves the existing checked
`PhysicalMemory::write_bytes` behavior and reports the exact physical range only
after commit. The existing no-callback convenience method remains available.
`PagedCodeSession::write_physical_ram` is a narrow privileged operation for the
one-vCPU fixture; it accepts a typed physical address and byte slice between
native calls, and exposes neither mutable RAM nor an arbitrary writer callback.

The session collects actual committed notifications and runs the same physical
code-page generation reconciliation used by the guest MOV writer before
returning success or an error. Any overlapping committed write invalidates both
inactive alias cache entries, including writes outside the admitted instruction
range. A write confined to another guest page preserves the code generation and
entries. The test uses neighboring pages at `0x6000` and `0x7000`, exact callback
ranges, an immediate replacement followed by execution through both aliases,
and failed RAM/ROM boundary and ROM writes that preserve the cache.

This does not connect every possible physical-memory writer and does not cover
remap/TLB, cross-page code, MMIO/DMA, SMP or mutation of an active native block.
The path is synchronous and only admits writes while native code is inactive;
performance is not measured. Parent S4-16 remains open. See the
[S4-16c report](../reports/STAGE_4_S4-16C_2026-11-06.md).

### S4-16d: tracked snapshot and loader-patch A/D notifications

The callback-bearing machine APIs report only physical writes that actually
committed, in operation order. A snapshot may report page-walker Accessed
updates before a later translation fault. A tracked alias patch may report
Accessed/Dirty updates committed by its read preflight/write walk before an
error; the requested code byte is reported only when that byte commits.

`PagedCodeSession::execute_alias` and `patch_alias_byte` collect these ranges
and invoke the shared code-page reconciliation before propagating a snapshot,
mapping/admission, or patch result. An overlapping committed A/D write updates
the backing generation view and invalidates both idle alias entries before the
next lookup, even when the guest operation returns a fault. A failed patch does
not imply that its operand byte committed. The callback-free machine methods
remain wrappers around the same tracked behavior.

The S4-16d tests cover ordered A/D and operand callbacks on success. In the
CPL3 write-protection case, the read preflight sets the alternate PDE A bit at
`0x7010` in the code page before the write walk faults. In the snapshot case,
the walk sets the unused PML4 A bit at `0x7400` in the code page before a later
missing-PDE fault. They do not imply TLB
invalidation, mapping-change handling, cross-page code, or active-block
retirement; parent S4-16 remains open. See the
[S4-16d report](../reports/STAGE_4_S4-16D_2026-11-06.md).

### S4-16e: retired INVLPG releases bounded aliased-code admission

The host session can execute one exact `INVLPG [RBX]` between native calls
through the tracked Long64 RAM fetch adapter. It reconciles fetch-time A/D
writes before returning. Instruction faults and ISA rejections are carried in
`Ok(result).step.cpu.exit`; tracked fetch backend failures are carried in
`Ok(result).step.access_error`; `Err` is reserved for configuration errors. A
retired canonical invalidation clears both inactive alias cache entries and
releases the fixed backing, code-range, and alias binding so remapped code can
be admitted. The session intentionally over-invalidates its two-entry cache
for any canonical retired INVLPG effect; the guest still executes a separate
architectural invalidation for each linear alias.

The bounded S4-16e result has no production TLB, general remap notification,
cross-page code, arbitrary writer, MMIO/DMA, SMP, or active-block retirement
semantics. Parent S4-16 and Stage 4 remain open; no performance claim is made.
See the [S4-16e report](../reports/STAGE_4_S4-16E_2026-11-06.md).

### S4-16f: translation context in the code-alias key

`PagedCodeSession` derives its admission identity from a
`PagedCodeTranslationContext` instead of a fixed fixture: CR3 root (bits
35:12), CR0.WP, CR4.PSE, CR4.PGE, EFER.NXE and CPL form the key; its injective
id is the `CodeIdentity::address_space`, while `ExecutionContextId` is a
monotonic generation advanced on every key change (the dispatch caches require
a strictly advancing id). Any key change (observed in CPU state, retired
`MOV CR0` WP toggle, `set_privilege`, `set_efer`), every retired `MOV CR3`
(including a same-value write), every retired canonical `INVLPG` and every
delivered `#PF` (`note_delivered_page_fault`) conservatively resets the whole
session: both cache entries and the backing/range/alias binding. A block
admitted under another key is never reused; returning to an earlier key needs a
fresh admission. Bits outside the key (CR0.TS/EM, CR4.OSFXSR, EFER.SCE) do not
reset. `PagingControlsMismatch` remains only for missing or invalid controls.
Not covered: active-block retirement (S4-16i), arbitrary writers (S4-16h),
cross-page code (S4-16g), PCID/global pages, a shared invalidation contract with
`FetchTranslationCache`; the consolidated S4-16 invalidation contract is still
to be written. No performance claim is made.

Host obligations and key boundary (S4-16f):

- A delivered `#PF` is reported by the host through
  `note_delivered_page_fault`. If the host does not call it, the session is not
  reset, but a walk over the new PTEs gives `UnsupportedAlias` (or a snapshot
  fault), never execution of stale code: the snapshot is recomputed on every
  `execute_alias`.
- A CPL change is reported by the host through `set_privilege`. A state that
  carries a CS selector must agree with the key: if `(cs_selector & 3)` differs
  from the CPL of the key, `execute_alias`, `step_guest_mov`, `execute_invlpg`,
  `step_guest_mov_wp`, `execute_mov_cr3` and `execute_mov_cr0` return
  `ContextRejected(ProbeConfigError::CplMismatch)` before any effect: no reset,
  no change of the state. Limitation: a state without a CS selector
  (`cs_selector == None`) is not checked and keeps the session's CPL, so the
  host must call `set_privilege` itself in that case.
- SMEP, SMAP, PKE, PKS, LA57, PCIDE, CR0.CD and CR0.NW are neither in the key
  nor admitted: the CR0/CR4 validation of `Long64ProbeConfig` rejects them, so
  "not in the key" is safe (a session can never hold a snapshot in which they
  would change translation or access rules).
- `execute_mov_cr3` / `execute_mov_cr0` modify the caller's `state` in place. On
  `ContextEffectMismatch` or on `ContextRejected` after the instruction retired
  the state may already hold the written CR and the advanced RIP; these branches
  are unreachable with the current od-isa profiles. If such a path appears, the
  caller must treat `state` as corrupted. `ContextRejected` does not reset the
  session when the rejection happens before any effect is applied (`set_efer`,
  the CPL check, the pre-step configuration of `execute_mov_cr0`); only the
  rejection of an already retired write resets it.

### S4-16g: code across a 4 KiB page boundary

A `PagedCodeSession` block may start in one guest 4 KiB page and continue into
the next one (at most two pages; a longer range is `InvalidLength` before the walk (length outside `0 < length <= 0x2000 - offset`); the exception is an address overflow in the last page of the address space: there the rejection happens after the walk of the first page, its Accessed bits are committed, the block is not bound and not executed; recorded as a limitation, the interpreter performs the same first-page walk; otherwise the state, the session binding and the page tables are left untouched). The first page is walked as an instruction fetch at the block RIP;
the second page gets its own fetch walk at the page boundary with the block RIP
as the instruction RIP, so a fault there carries the same `PageFaultRecord` as
a byte-wise interpreter fetch (saved RIP = block RIP, CR2 = boundary address).
Each page keeps its own physical base and generation; the physical pages need
not be adjacent. The block has two dependency spans, `[RIP, boundary)` and
`[boundary, end)`, with distinct mapping ids, backings and backing versions.
A committed physical write to either backing page advances that page's
generation and invalidates the entry (for a write reported through an empty
callback, invalidation is reached by the key mismatch at the next lookup, not by
removing the entry immediately); a write to any other page, including one
inside the same 16 KiB host page, does not (generations are per 4 KiB page).
Page granularity is conservative: a write elsewhere on a dependency page still
invalidates, as in S4-16c. The snapshot is taken before every lookup, so a
resident block is not executed when a page walk now faults. A fault on either
page returns no snapshot, binds nothing and executes nothing; the interpreter
fallback of S4-15c is the caller's step. If the second walk writes Accessed bits
into the first code page, the first page is re-read so both parts describe
memory as of the return from the snapshot (this is not byte-wise oracle
equivalence). Limitation: for a layout where the page tables live in the first
code page, the JIT may decode the bytes after the Accessed bit of the second
walk is set, while the interpreter reads them before; such a layout is not
covered by tests; the variant that matches the oracle exactly is not to re-read
the first page and to rely on the subsequent version check (recorded as an open
question). Not covered: active-block mutation (S4-16i), arbitrary
writers (S4-16h), 2 MiB code pages, multi-instruction blocks, patching the
second page through `patch_alias_byte` (`UnsupportedAlias`). No performance
claim is made.
### S4-16h: writers that do not notify the session
Every committed RAM write advances the generation of each written 4 KiB page in
`PhysicalMemory::write_bytes`, whichever component writes (NVMe DMA, the Linux
loader, interrupt/exception frame pushes, a JIT store, a loader patch). Only the
session's own methods hand committed ranges to `reconcile_code_writes`, which
releases a stale cache entry immediately. A writer that reports through an empty
callback does not: correctness then rests on `execute_alias` retaking the tracked
snapshot before each lookup (current bytes and generations of both pages in the
translation key), so a stale block is never found and never executed; the
explicit pre-dispatch `invalidate` and the version in the identity are redundant
layers on top of the byte key. Display stores reach only MMIO registers and the
framebuffer region, which has no generations and cannot be admitted as code
(`Snapshot(_)`). `PagedFetchedLoadSession` and `PagedCodeSession` cannot hold
blocks at once (one process-wide MAP_JIT region), so a JIT store never runs
while a cached alias block exists. Covered by `tests::s4_16h_*`. Not covered: a
host that actually forwards NVMe/loader/delivery ranges to the session, active
blocks (S4-16i). No performance claim is made.

### S4-16i: store into the active block
`PagedFetchedLoadSession` (`execute_with_store`, `execute_forms`,
`execute_or_interpret`): a `48 89 03` whose operand lies on the physical page the
block was fetched from is no longer refused (`PagedStoreRefusal::ActiveCodePage`
and the `active_code_page` argument of `with_paged_mov_store_memory` are gone).
It is walked, checked (Present, Write, User, WP) and committed like any store:
Accessed bits, leaf Dirty bit, the 8 bytes. A `#PF` precedes the commit exactly
as before (`GuestFault`, no write, block kept). On a committed write the
instruction retires with RAX/RFLAGS/RIP as the interpreter's equivalent store
(RIP = `next_rip`, never inside the rewritten bytes), and the host helper turns
the exit `Complete` into `MemoryOperationExecutionExitV2::CodeInvalidated` when
a committed write range intersects the active physical page (the data bytes or,
rarely, a table entry on that page). `execute_or_interpret` returns it as a final
`PagedStepOutcome::Jit` (never `JitHostFailure`, never repeated on the
interpreter). Observation point: the end of the retired store. The block is this
one instruction, so no later instruction of the old bytes can run; the next
entry fetches again.
The admitted block is marked `retired`: it is never reused. Re-admission is
ensured by the page generation, which grows on any committed write to RAM (a
store of the same three bytes included); `retired` is an additional
defence in depth. `retired` is not caught by a single-mutation test (the
generation alone rejects the block), and a test of `retired` with an unchanged
generation is impossible without a code change, since a committed write to the
active page always raises the generation. The next entry drops it and admits from the new bytes
(`Admitted`, `admissions` + 1). Its executable memory is borrowed (alive) for
the whole native call, is not released by the call that retires it, and is freed
at the next entry that reaches the key check (even if it then ends in
`UnsupportedForm`, `Compile` or `Publish`; an entry refused earlier - context,
CR3, operand range, fetch `#PF` - does not free it), `reset()` or drop of the session (S4-15f mechanism;
`jit_free_slots()` stays 0 after the exit and returns to all-free after reset
or drop). A store into another code page is unchanged (exit `Complete`, block
reused, key re-check on the next entry).
Not covered: SMP and deferred retirement with a grace period (S3-25), blocks of
more than one instruction (a store in the middle of a block, mid-block exit and
re-entry), 2 MiB code pages, writes by other writers into the active page while
the block runs (single vCPU, synchronous only), `PagedCodeSession` active
blocks. No performance claim is made. Covered by `s4_16i_*` in
`tests/system/stage4_memory_fault.rs`.

### S4-16: code aliases and invalidation (closure, S4-16j)
Summary of S4-16a-i; the tests-to-plan table and the limits list are in the
"S4-16 — итог" section of `docs/reports/STAGE_4_REPORT.md`. No code changed in S4-16j.
- Key: a block depends on physical code pages (backing page, per-4 KiB-page
  generation from `PhysicalMemory`, the code bytes) and on the translation context
  (CR3 root, CR0.WP, CR4.PSE, CR4.PGE, EFER.NXE, CPL). Two linear aliases of one
  physical page are two cache entries sharing one backing. Code across a 4 KiB
  boundary has two dependency spans (two pages, at most).
- Every lookup re-takes a tracked snapshot (fetch walk, bytes, generations) and builds
  the key from it, so a stale block is never found; the explicit invalidation after a
  committed write only frees the entry at once (defence in depth).
- Reset rules (whole session: both entries, backing, range, alias): a change of any key
  component, any retired `MOV CR3` (same value too), a retired canonical `INVLPG`, a
  delivered `#PF` reported by the host. Bits outside the key do not reset.
- Tracked: committed writes by the session itself (guest store, loader patch, A/D
  bits of the walker) reconcile against the dependency pages before the result is
  returned, also on faults. A store into the active block page commits, the block exits
  with `CodeInvalidated` at the end of the retired instruction, is marked `retired`
  and is re-admitted from the new bytes; its allocation is freed at a later entry,
  `reset()` or drop (S4-15f).
- Not covered: writers are not wired to the session in production (NVMe DMA, loader,
  display, interrupt frames, JIT store: safety rests on the key at the next lookup);
  one vCPU, no SMP or deferred retirement (S3-25); 2 MiB code pages; multi-instruction
  blocks; three or more pages; store/dword forms with a displacement outside the
  fallback; `retired` is defence in depth only; first-page re-read in the "page tables
  in the first code page" layout differs from the byte-wise oracle (S4-16g); a
  non-canonical boundary address (`#GP`) is not modelled. No performance claim.

### S6-40: кэш кода при нескольких vCPU (контракт)

Статус: документ-контракт, написан 2026-11-18, принят после архитектурного ревью 2026-11-18 (Claude Opus, high; условия выполнены в `4c3182c`); пометки «по памяти» сверяются до приёмки S6-42. Кода JIT этот раздел не добавляет и не меняет. Раздел нормативен для `S6-41`, `S6-42` и `S6-43`. Он расширяет однопроцессорный жизненный цикл [S3-25c](#s3-25c-block-lifecycle-for-one-vcpu-normative) и границы STOP [S3-28c/d](#s3-28c--s3-28d-stop-and-event-matrix-latency-bound-shared-budget-local-stop-latency) на несколько vCPU. Опирается на `S4-15`/`S4-16` (ключ блока и поколения страниц), на `S6-17` (согласование адресных пространств), `S6-30b` (атомарные поколения), `S6-32` (порядок по режимам), `S6-33` (порядок блокировок), `S6-34` (потоки vCPU, `stop_all`, kick, шлюз split `LOCK`) и `S6-36` (litmus параллельного режима); их тексты — в [MACHINE_CONTRACT.md](MACHINE_CONTRACT.md) и [MEMORY_MODEL.md](MEMORY_MODEL.md). Пометка «по памяти» означает, что формулировка источника приведена не по сверенному тексту. Её надо сверить до приёмки S6-42; правила раздела от дословного текста не зависят, но ссылка без сверки доказательством не считается.

**Отправные факты кода (2026-11-18).**
- Поколения. `Region::page_generations` — `Vec<AtomicU64>`, по счётчику на 4 KiB страницу RAM. `PhysicalMemory::commit_write` пишет данные и затем делает `fetch_add(1, Release)` по каждой затронутой странице. `code_page_generation` читает с `Acquire`. Правило читателя: поколение читается до байт и хранится вместе с ними (MACHINE_CONTRACT, S6-30b). Поколение повышает любая зафиксированная запись в RAM, кто бы ни писал: гость, JIT-store через helper, DMA, загрузчик, кадры прерываний (S4-16h).
- Уведомление `committed_write` вызывается только в потоке писателя (S6-30b п. 3). Кеш другого потока узнаёт о записи только по поколению.
- Кеш кода и арена — однопоточные. `PagedFetchedLoadSession` держит один допущенный блок, все входы берут `&mut self`, и на каждом входе заново снимается tracked snapshot. Ручки `PublishedArenaBackend`/`MultiEntryBlockCache` построены на `Rc`/`RefCell`, поэтому они `!Send` и `!Sync`.
- Исполняемая память. В процессе одна аренда `MAP_JIT_REGION_LIVE` (`MapJitLease::acquire`, `compare_exchange`). Вторая живая region получает `MappingAlreadyLive` (ISSUE-038). `PublishedCode` под production-сессией отображает свою region на каждый допуск. Окно записи — `JitWriteAccess` (`pthread_jit_write_protect_np(0)`, затем `(1)`) вокруг копирования. После него в том же потоке вызывается `sys_icache_invalidate` (`write_mapping_bytes`, `crates/od-host/src/lib.rs`).
- Блок. Не больше 32 гостевых инструкций (`MAX_BLOCK_INSTRUCTIONS`) и не больше 4096 байт кода (`MAX_EMITTED_BLOCK_BYTES`). Блок кончается на первом переходе (`BlockStopReason::Branch`), обратный переход тоже кончает блок. Управление возвращается диспетчеру, и следующий вход идёт через lookup. Прямых связей блоков нет (S3-31 открыт).
- STOP и события. `RuntimeContextV1::stop_requested` — указатель на выровненный атомарный байт. Сгенерированный код опрашивает его на каждой границе гостевой инструкции (S3-28c). Флаг события читает только диспетчер после `control_exit`.
- Потоки vCPU (S6-34, `parallel_run.rs`): один поток `std` на vCPU внутри `std::thread::scope`, `ParallelControl::stop_all`/`resume`, флаги `safe` и `quiet`, kick — флаг плюс `unpark`. JIT к параллельному циклу пока не подключён.

#### 1. Владение кэшем (пункт 1 плана)

Кандидаты:

| | (а) Отдельный кэш у каждого vCPU | (б) Общий кэш с отложенным освобождением |
| --- | --- | --- |
| Владелец блока | Поток своего vCPU. Блок, ручка, PC-map и указатель входа не покидают этот поток | Все потоки, ручки `Send + Sync` (`Arc`) |
| Инвалидация | По общим поколениям страниц при каждом lookup своего vCPU. Своя запись дополнительно сразу делает свою запись кэша недоступной (освобождение на safe point) | Так же по поколениям, плюс удаление записи из общей структуры под синхронизацией |
| Освобождение | Только владелец, на своём safe point (S3-25c). Другой поток войти в блок не может, поэтому период ожидания не нужен | Блок освобождается, только когда каждый vCPU прошёл свой safe point после того, как блок стал недоступен для новых входов (quiescent state). Нужны учёт эпох и межпоточная синхронизация кода (п. 4.3) |
| Исполняемая память | Одна region на процесс, разделённая на непересекающиеся части по vCPU (п. 4) | Одна region, общий распределитель слотов |
| Цена | Один и тот же код компилируется до N раз, исполняемой памяти до N раз больше | Синхронизация на lookup и вставке, сложное освобождение |
| Совместимость с текущим кодом | Совпадает с S3-25c для одного vCPU: тот же автомат, `&mut`/`Rc` остаются | Нужны новые типы ручек без `Rc`, новый жизненный цикл, `!Send`-инвариант снимается |

**Решение: (а).** Обоснования:
1. Автомат S3-25c (четыре состояния, переходы только на safe point, поколение слота при переиспользовании) переносится на каждый vCPU без изменений. Условие S6-42 «блок освобождается только когда ни один vCPU не может его исполнять» выполняется по построению: исполнять блок может только владелец, а освобождает его владелец после возврата из native-вызова.
2. Текущие ручки `!Send` (`Rc`). Вариант (а) не требует ни `unsafe impl Send/Sync`, ни переноса ручек между потоками; это то же правило, что в ADR-0005.
3. Межпоточного кода нет: каждый поток исполняет только то, что сам записал. Правило Arm ARM о синхронизации на исполняющем ядре сводится к однопоточному случаю (п. 4.2).
4. План запрещает выбирать (б) без замера.

Нормативные правила (а):
- **A1.** У каждого vCPU ровно один кэш кода. Он создаётся, используется, очищается и уничтожается в потоке этого vCPU. Блок, ручка, PC-map или указатель входа другому потоку не передаются; `unsafe impl Send/Sync` для них не добавляется. В режиме чередования все vCPU исполняет один поток, но кэши всё равно раздельные: по одному на vCPU, ключ без номера vCPU в общую структуру не попадает (по аналогии с кешами перевода, которые MEMORY_MODEL:243 делает частными для vCPU).
- **A2.** Ключ блока — ключ S4-16: физические страницы зависимости, их поколения, байты кода, контекст трансляции своего vCPU (CR3, CR0.WP, CR4.PSE, CR4.PGE, EFER.NXE, CPL). Номер vCPU в ключ не входит, потому что кэш частный.
- **A3.** Между vCPU инвалидация идёт только через общие поколения страниц. Межпоточных сообщений об инвалидации нет: колбэк `committed_write` в чужой поток не передаётся. Собственная запись vCPU через свой колбэк (`reconcile_code_writes`) сразу делает зависимые записи недоступными для новых входов (выход из состояния 1 S3-25c); освобождение — на safe point (состояние 4, A4). Это защита в глубину, корректность от неё не зависит.
- **A4.** Своя запись удаляется только владельцем и только на своём safe point (S3-25c, «Safe point»). Чужой поток запись не удаляет и не освобождает. Механизм `pending_release` остаётся однопоточным, как в S3-25c.
- **A5.** В параллельном режиме допускается только production-путь lookup: на каждом входе заново снимаются обход выборки, поколения и байты, как в `PagedFetchedLoadSession`. Фикстурные кэши (`MultiEntryBlockCache` с фикстурными ключами, профили ADR-0004, S3-18*) работают только в одном потоке и в параллельном режиме не используются.
- **A6.** Исполняемая память — по п. 4 (одна region, части по vCPU).
- **A7.** Операции кэша (lookup, вставка, удаление) и native-вызов не выполняются под замками S6-33: ни под замком шины или устройств, ни под замком APIC, источника платы или владельца часов. Слот своего vCPU поток держит так же, как при шаге интерпретатора в S6-34. Замок устройств helper берёт на одно обращение, как интерпретатор. Своих замков кэш не вводит, потому что он частный.

**Когда пересматривать в пользу (б).** Только по замеру `S6-37`/`S6-38`, не здесь. Записать долю времени компиляции, повторные компиляции одного кода разными vCPU, вытеснения из своей части region и рост памяти процесса на 1, 2 и 4 vCPU. Переход к (б) требует нового раздела этого контракта и нового ADR. Обязательно: освобождение через quiescent state всех vCPU, межпоточная синхронизация кода по п. 4.3, ручки без `Rc` и обоснование, что `stop_all` не нужен для освобождения.

#### 2. Cross-modifying code (пункт 2 плана)

**Источник.** В Intel SDM 253668-093US, сентябрь 2026, cross-modifying code описан в §11.1.3, а список сериализующих инструкций в §11.3. Модифицирующий процессор записывает код и подаёт сигнал. Исполняющий процессор ждёт сигнал, выполняет сериализующую инструкцию, например `CPUID`, и затем исполняет обновлённый код. Гарантия протокола действует после сериализации. [Intel SDM Vol. 3A, §11.1.3, §11.3](https://cdrdv2.intel.com/v1/dl/getContent/671190).

**Что делает машина.** Пусть vCPU A изменил код, который vCPU B исполняет сейчас или будет исполнять.
1. A фиксирует запись через `commit_write`: данные, затем `fetch_add(Release)` поколения страницы. Если записывает JIT-код A, то тоже через этот путь (обязательство S6-41 (1) ниже).
2. B не получает уведомления. Он узнаёт о записи на ближайшем lookup: перед входом в блок читаются поколения всех страниц зависимости (`Acquire`), и при расхождении с ключом блок недоступен для новых входов (состояние 1 S3-25c), потом компилируется заново.
3. **Допустимая задержка обнаружения.** Когда повышенное поколение видно потоку B, B не входит ни в один блок со старыми байтами. Текущий блок B может дорабатывать: не больше оставшихся инструкций блока, то есть не больше 31 инструкции после текущей (блок не длиннее 32 инструкций, `MAX_BLOCK_INSTRUCTIONS`). Граница — ближайшая проверка ключа при lookup или входе в блок, а когда появятся связи — при переходе между блоками (п. 3). Сколько времени повышение поколения идёт до другого ядра, задаёт аппаратная когерентность host. Предела в инструкциях для этого нет, и он не заявляется.
4. **Гарантия при протоколе SDM (G-XMC).** Пусть гость на B прочитал значение, которое A записал после изменения кода, затем исполнил сериализующую инструкцию или любую инструкцию, которая кончает блок, и прошёл следующий за этим lookup. Тогда B больше не исполняет старый код. Цепочка: изменение кода и `fetch_add(Release)` у A по порядку программы идут раньше сигнальной записи A. Сигнальная запись в параллельном режиме — `SeqCst` (S6-32). Чтение сигнала на B — `SeqCst` (S6-32) либо, в JIT-коде, не слабее `Acquire` (S6-41). Значит, `fetch_add` A происходит раньше чтения поколения на B (happens-before), и по когерентности B читает новое поколение на ближайшем lookup. Сериализующая инструкция в середине блока не компилируется: она последняя в блоке или идёт в интерпретатор (обязательство S6-41). Поэтому после неё всегда следует lookup. Машина даёт больше, чем требует SDM: и без сериализации B перестаёт входить в старые блоки, как только видит поколение.
5. **Разорванный снимок.** Пусть запись A попала между чтением поколения и чтением байт при компиляции на B. Тогда блок B собран из смеси старых и новых байт и записан со старым поколением. Модель не гарантирует, что такой блок исполнится не больше одного раза: B может прочитать новые байты данных, записанные A до `fetch_add`, но старое поколение (от `fetch_add` A к следующему чтению поколения на B нет happens-before), и войти в блок ещё раз. Верная граница: до первого lookup на B, который увидел новое поколение (чтение `Acquire`). После такого чтения повторных входов в блок из смеси нет; при протоколе G-XMC такое чтение гарантировано на ближайшем lookup после сигнала, то есть повторных входов после сигнала ноль. Байты читаются через атомарные функции доступа `PhysicalMemory` (S6-30a/S6-32), никогда через сырой срез, поэтому гонки данных в Rust нет. Декодер принимает любые байты, отказ декодера — обычный отказ блока. Архитектурная гарантия G-XMC относится к исполнению после сигнала и сериализации из §11.1.3.
6. Самоизменяющийся код внутри своего vCPU — отдельный случай S3-24 (п. «Ограничения»). Запись самого B в страницу блока, который B исполняет, обрабатывает S4-16i (одноинструкционный блок, выход `CodeInvalidated`). Для многоинструкционных блоков со store это обязательство S3-24.

#### 3. Обнаружение устаревшего блока (пункт 3 плана)

- **D1 (переход в обход lookup).** Каждый вход в блок, будь то из диспетчера, по прямой связи, по обратному переходу, по кешу адресов возврата или цепочкой, сверяет поколения всех страниц зависимости и поколение слота цели. Это повторение правила S3-25c («Rule for any future linking or backward transition») для нескольких vCPU.
- **D2 (отвязки на safe point мало).** S3-25c допускает и второй вариант: отвязать связь на том safe point, где цель уходит из состояния 1. При нескольких vCPU этого мало: запись другого vCPU владелец не замечает, пока сам не проверит поколение, и до этой проверки отвязка не произойдёт. Поэтому в параллельном режиме для S3-31 допустим только вариант с проверкой на переходе. Проверять можно поколения страниц цели или консервативный суммарный счётчик, который растёт не реже, чем любое из этих поколений. Выбор между ними — S3-31, по замеру.
- **D3 (циклы внутри блока).** Обратная дуга внутри одного native-блока без такой проверки запрещена. Сейчас её нет: блок кончается на первом переходе.
- **D4.** Сторожа STOP, stop-RIP и бюджета проверку поколения не заменяют.
- **Текущая граница.** Связей нет, циклов внутри блока нет, каждый переход возвращает управление диспетчеру, и тот делает lookup. Значит, D1 выполнено сейчас, задержка — по п. 2.3. D1–D3 — предусловие закрытия `S3-31` и приёмки `S6-42`. Без них задержка обнаружения не ограничена.

#### 4. Исполняемая память: несколько потоков и один MAP_JIT (пункт 4 плана)

**4.1 Правила платформы.**
- При поддержке thread-local защиты `pthread_jit_write_protect_np` меняет W/X для вызывающего потока. Ноль разрешает запись, ненулевое значение разрешает исполнение. Перед исполнением обновлённой страницы Apple требует `sys_icache_invalidate`. Проект использует прямое переключение защиты. Вариант allowlist использует `pthread_jit_write_with_callback_np`. [Руководство Apple](https://developer.apple.com/documentation/apple-silicon/porting-just-in-time-compilers-to-apple-silicon), [man page Apple libpthread](https://raw.githubusercontent.com/apple-oss-distributions/libpthread/main/man/pthread_jit_write_protect_np.3).
- Apple указывает предел в одну `MAP_JIT` region для Hardened Runtime вместе с `com.apple.security.cs.allow-jit`. Проект обеспечивает собственную единственную аренду во всех сборках. [Руководство Apple](https://developer.apple.com/documentation/apple-silicon/porting-just-in-time-compilers-to-apple-silicon).
- Arm описывает полную синхронизацию кода в DDI0487L.a B2.7.4.2 и правила параллельного изменения в B2.2.5. Типовая последовательность на пишущем ядре использует `DC CVAU`, `DSB ISH`, `IC IVAU` и `DSB ISH`. Исполняющий поток выполняет context synchronization после публикации, например `ISB`. При `IDC` или `DIC` допускается опустить соответствующую операцию с кешем. [Сопровождение кешей Arm](https://developer.arm.com/community/arm-community-blogs/b/architectures-and-processors-blog/posts/caches-self-modifying-code-implementing-clear-cache), [Межпоточное исполнение Arm](https://developer.arm.com/community/arm-community-blogs/b/architectures-and-processors-blog/posts/caches-self-modifying-code-working-with-threads).

**4.2 Правила проекта (вариант (а)).**
- **M1.** На процесс одна region. Её создаёт один объект `od-host` для всей машины до запуска потоков vCPU, и он же держит аренду `MapJitLease`. Объект `Sync`: потоки получают на него общую ссылку внутри `thread::scope` или `Arc`. Уничтожается он после `join` всех потоков vCPU; аренда снимается вместе с ним. Жизненный цикл region: части между vCPU разрешено перераспределять только между прогонами, когда ни один поток vCPU не работает (все потоки присоединены, кэши очищены); пока потоки работают, перераспределения нет (M2). Отображение на блок или на vCPU запрещено (ISSUE-038). В параллельном режиме не допускается и отображение на каждый допуск, как сейчас у `PublishedCode` под `PagedFetchedLoadSession`: S6-42 переводит допуск на слот своей части region.
- **M2.** Region при старте делится на N непересекающихся частей, по одной на vCPU; размеры — параметр S6-42, проверяется замером S6-38. Часть передаётся своему потоку один раз как `Send`-значение без исполняемых ручек. В потоке она становится `!Send`-ареной кэша этого vCPU. Пока потоки работают, части между vCPU не перераспределяются и чужие слоты не забираются. Перераспределение означало бы межпоточный код (п. 4.3) и новое ревью. Если части не хватает, владелец вытесняет свои записи на своём safe point (FIFO, S3-18c).
- **M3.** В свою часть пишет только её поток. Окно записи (`JitWriteAccess`) действует только для этого потока и никогда не охватывает native-вызов в нём. Исполнение в других частях region другими потоками от этого окна не зависит (правило Apple выше).
- **M4.** После записи и при переиспользовании слота поток восстанавливает защиту, вызывает `sys_icache_invalidate` на записанном диапазоне и выполняет явный `ISB` перед первым входом. В ARM64-реализации Apple libplatform-375.120.2 непустой диапазон обрабатывается последовательностью `DSB ISH`, `IC IVAU`, `DSB ISH`, `ISB`. Последовательность инструкций относится к этой версии библиотеки. Миграция потока относится к ответственности macOS. В Arm с FEAT_ExS контекстная синхронизация при входе и выходе из исключения зависит от `SCTLR_EL1.EIS` и `EOS`. В опубликованной версии XNU есть компенсационный `ISB` для несинхронизирующего возврата. [libplatform-375.120.2](https://raw.githubusercontent.com/apple-oss-distributions/libplatform/libplatform-375.120.2/src/cachecontrol/arm64/cache.s), [Arm SCTLR_EL1](https://support.arm.com/documentation/ddi0601/2026-09/AArch64-Registers/SCTLR-EL1--System-Control-Register--EL1-), [XNU xnu-12377.121.6](https://raw.githubusercontent.com/apple-oss-distributions/xnu/xnu-12377.121.6/osfmk/arm64/locore.s).
- **M5.** Если пишущий и исполняющий потоки разные (в (а) такого нет; это (б) или перераспределение частей), правила такие. Пишущий: запись, восстановление защиты, `sys_icache_invalidate`, затем публикация указателя входа с `Release`. Исполняющий: чтение указателя с `Acquire`, `ISB`, затем вход (синхронизация на исполняющем потоке по Arm, см. источники выше).
- **M6.** Пока region машины живёт, публикация через v0 fragment ABI или сессию v1 получает прежний BUSY/`MappingAlreadyLive`, и наоборот (ADR-0005 не меняется).

#### 5. Safe point блока (пункт 5 плана)

- **S1.** Safe point vCPU с JIT — точка S3-25c: native-вызов вернулся, результат и счётчики проверены (`validate_counters`), уведомления применены, следующий lookup ещё не начался. Только здесь поток vCPU поднимает `safe` (подтверждение `stop_all`, S6-34); внутри native-вызова `safe` не поднимается никогда. `quiet` (подтверждение шлюзу split `LOCK`) поднимается на safe point, с одним исключением: helper ждёт шлюз split `LOCK` до первого эффекта инструкции (проигравший, S6-34, шаг 8) — тогда `quiet` поднимается внутри native-вызова. Это безопасно, потому что до эффекта инструкция не начала менять состояние (S6-34).
- **S2.** Скрытых живых регистров нет. На safe point архитектурное состояние гостя целиком лежит в `LongModeState`, на который указывает `RuntimeContextV1.state`: выходы сгенерированного кода — канонические границы PC-map (ABI S3-08, состояние S3-12, матрица выходов S3-28c). В host-регистрах после возврата значений гостя нет. `RuntimeContextV1` принадлежит одному вызову и после возврата не используется. Снимок остановленной машины (`VmSnapshot::take_parallel`, S6-35) читает только состояние vCPU и LAPIC; кэш кода в снимок не входит.
- **S3.** Kick, `stop_all` и шлюз split `LOCK` при включённом JIT: протокол STOP-байта (`stop_requested`) в порядке шагов.
  1. Запрашивающий (`stop_all`, шлюз split `LOCK`, kick для IPI, `finish`) сначала ставит свой флаг запроса (`SeqCst`): флаг барьера, `split_owner`, флаг события, `finish`. Затем поднимает STOP-байт целевого vCPU (`AtomicU8`, `SeqCst`).
  2. Поток vCPU перед КАЖДЫМ входом в блок снимает STOP-байт (`SeqCst`), затем перечитывает все флаги запросов (`SeqCst`): `stop_all`, `split_owner` и запрос «стань тихим», событие, `finish`. Если любой флаг поднят, в блок поток не входит и подтверждает `safe`/`quiet` по S6-34 (S1). Если ни один не поднят, входит.
  3. Какие kick поднимают байт. Остановка (`stop_all`): да. Шлюз split `LOCK` («стань тихим»): да, и запрос доходит до native-кода тем же опросом. IPI и прерывание для поднятия события: да, граница события — блок. `finish`: да. Kick без байта (только флаг и `unpark`, как сейчас в S6-34 до подключения JIT) для работающего в native-коде vCPU недопустим как единственный механизм остановки; если такой kick останется, граница у него — конец блока, не больше 32 инструкций. Спящий vCPU (`HLT`, ожидание SIPI) вне native-кода и подтверждает сразу, как в S6-34.
  4. Снятие. Байт снимает сам поток на входе (шаг 2), контроллер его не снимает. `resume` снимает свой флаг запроса, но не байт. Гонка «новый запрос во время снятия» исключена: перечитывание флагов идёт после снятия байта. Запрос, поднявший флаг до перечитывания, виден перечитыванием, и вход не происходит. Запрос, поднявший флаг после перечитывания, ставит байт позже снятия (флаг идёт раньше байта в порядке `SeqCst`), байт остаётся поднятым, и первый же опрос в блоке выходит.

  Граница при этом: запрос, поднятый до перечитывания, — блок не исполняется вовсе; поднятый позже или во время блока — одна гостевая инструкция в регистровом блоке, для пары памяти — граница перед вторым местом (S3-28c). Событие (IPI) для диспетчера наблюдается в конце блока: не больше одного блока, до 32 инструкций.
- **S4.** STOP-байт пишет другой поток. В Rust это атомарный байт (`AtomicU8`), гонки данных нет. Сгенерированный код читает его однобайтовой загрузкой; Arm DDI0487H.a, §B2.2.1, с. B2-152–153, гарантирует single-copy atomicity такого доступа. Однобайтовая загрузка в один GPR выровнена на размер доступа при любом адресе байта. Порядок для самой загрузки в сгенерированном коде не нужен, он влияет только на задержку. Порядок `SeqCst` нужен на стороне Rust для пары «снятие байта, перечитывание флагов» и «флаг, затем байт» (S3, шаги 1–2). Выход по STOP возобновляется без повтора (S3-28c). Лишний байт (запрос уже снят) приводит к безвредному выходу: поток на safe point не видит флагов и входит снова.
- **S5.** Асинхронного вытеснения внутри JIT-кода нет: сигналов, `thread_suspend` или переписывания PC работающего потока нет. Host fault в сгенерированном коде — отказ host (S3-27 открыт; ADR-0005: «generated-code memory faults remain host failures»). Задержку остановки ограничивает только кооперативный опрос S3-28c.
- **S6.** Если helper ждёт внутри native-вызова (проигравший за шлюз split `LOCK`, S6-34), поток считается тихим (`quiet`) по протоколу S6-34; это исключение из S1 для `quiet`. `safe` он подтверждает только после конца своей инструкции и выхода из блока (S1). Ожидание ограничено временем удержания шлюза победителем.

#### Сценарии

| Сценарий | Поведение | Источник |
| --- | --- | --- |
| Инвалидация: своя запись vCPU в страницу своего блока | Запись фиксируется, поколение растёт, свой колбэк сразу делает зависимые записи недоступными, освобождение на safe point (A3, A4). Store в активный одноинструкционный блок завершается выходом `CodeInvalidated` и `retired` (S4-16i) | S4-16, S4-16i, S6-30b |
| Инвалидация: запись другого vCPU, DMA, загрузчика или кадра прерывания | Поколение растёт в `commit_write`. Каждый vCPU не входит в устаревший блок на ближайшем своём lookup; текущий блок дорабатывает (п. 2.3) | S4-16h, S6-30b, п. 2 |
| Инвалидация: другой vCPU пишет в таблицу страниц | Обход выборки повторяется на каждом lookup. До своей инвалидации допустим старый или новый перевод (S6-17). Если появится кэш перевода, он строгий: поколения кадров таблиц берутся при `insert` до чтения записей обхода (S6-34, условие 9) | S6-17, S6-34 |
| Инвалидация: свой `MOV CR3`, `INVLPG`, доставленный `#PF` | Сброс своей сессии по правилам S4-16. На чужие кэши не влияет | S4-16 |
| Освобождение | Только владелец, на своём safe point (A4). Слот переиспользуется с ростом поколения слота, старые ручки отвергаются (S3-25c). Region снимается после `join` всех потоков (M1). Периода ожидания нет: другие потоки в чужую часть не входят | S3-25c, A1–A4, M1–M2 |
| Остановка всех vCPU | `stop_all` ставит флаг запроса, затем STOP-байт (S3, шаг 1). Поток снимает байт и перечитывает флаги перед каждым входом (шаг 2): запрос до перечитывания — блок не исполняется; позже — выход не позже чем через одну инструкцию регистрового блока (пара памяти — перед вторым местом). `safe` подтверждается только на safe point (S1). Событие IPI — граница блок, до 32 инструкций. Состояние целиком в `LongModeState` (S2). Асинхронного вытеснения нет (S5) | S6-34, S3-28c, S3-08, S3-12 |
| Cross-modifying code | B перестаёт входить в старый блок после первого lookup, увидевшего новое поколение (чтение `Acquire`). При протоколе SDM (сигнал и сериализация) такое чтение гарантировано, повторных входов после сигнала нет (G-XMC). Без сериализации текущий блок дорабатывает не больше 31 инструкции после текущей | Intel SDM 093US §11.1.3, п. 2 |
| Одновременная запись в код с двух vCPU | Обе записи фиксируются, поколения растут по разу на запись. Итоговые байты — по модели памяти гостя для гонки записей. Блок, собранный из смеси, может быть исполнен повторно, пока B не сделает первый lookup, увидевший новое поколение (`Acquire`); после него повторных входов нет (п. 2.5). Архитектурная гарантия G-XMC действует после сигнала и сериализации | п. 2.5, MEMORY_MODEL |
| Самоизменяющийся код внутри многоинструкционного блока со store | Не входит в этот контракт: обязательство S3-24 (открыт). Сейчас покрыт только одноинструкционный выход S4-16i | S3-24, S4-16i |
| Сброс комплекса CPU или восстановление снимка | Кэш vCPU очищается его потоком на safe point или уничтожается вместе с потоком. Восстановление S6-35 RAM не пишет (RAM вне снимка). Корректность даёт ключ: контекст, поколения и байты проверяются на каждом входе. Обязательство S9-24: восстановление RAM идёт через `commit_write` (повышение поколений) или сбрасывает все кэши; поколение не убывает | S6-35, S6-30b, S9-24 |

#### Ограничения (обязательные)

- Одна `MAP_JIT` region на процесс (ISSUE-038). Отображения на блок или на vCPU нет (M1).
- Ручки на `Rc`, `!Send`. Поэтому кэш per-vCPU (A1). Общий кэш (б) без замера не выбирается.
- S3-24 (SMC в общих многоинструкционных блоках со store) открыт. Для S6-40 это обязательство S3-24: такой блок после зафиксированного store, задевшего страницу его зависимости, обязан выйти в конце этой инструкции. До закрытия S3-24 многоинструкционные блоки со store в параллельный режим не допускаются.
- S3-27 открыт: асинхронное вытеснение и восстановление после host fault в JIT-коде запрещены (S5).
- S3-31: связей блоков нет. Правила D1–D3 — предусловие его закрытия.
- Замеры (цена (а), размеры частей region, задержка остановки при N потоках) — S6-37/S6-38, не этот раздел. Задержка в миллисекундах не обещается.

#### ADR-0004, ADR-0005 и предложение ADR-0010

- **ADR-0004** (фикстурный кэш ёмкости один с retained handle) не нарушается. Его профили остаются однопоточными фикстурами (A5), production-кэш vCPU их ключ не обобщает. Триггер пересмотра ADR-0004 — «production owner/MMU integration, more than one resident entry» — относится к его фикстуре. Этот контракт фикстуру не меняет.
- **ADR-0005** (постоянная сессия на отдельном потоке-владельце) не нарушается. Правило «объекты JIT, привязанные к потоку, не пересекают потоков, `unsafe impl Send/Sync` нет» совпадает с A1. Правило «одна аренда `MAP_JIT` на процесс, второй владелец получает busy» сохраняется (M6). Одновременно сессии v1 и JIT машины в одном процессе нет.
- **Новое решение.** Одна region на процесс разделена между несколькими потоками vCPU: `Sync`-владелец region и части, привязанные к потокам (M1–M2). Это новая форма владения исполняемой памятью в `od-host`, которой ADR-0004 и ADR-0005 не описывают. Её нужно оформить как **ADR-0010 (предложение; файл не создан)**: «Process MAP_JIT region partitioned between vCPU threads», содержание — правила A1, M1–M6, вариант (б) отклонён до замера. Номер 0010 — потому что ADR-0006 и ADR-0009 существуют, а ADR-0007 и ADR-0008 зарезервированы планом этапа 11. Уточнение ADR-0005 (строка 20 которого: «process-wide MAP_JIT lease admits only one executable-memory owner at a time»): один держатель аренды, исполняемые части привязаны к потокам. Файл создаёт координатор после архитектурного ревью S6-40. Архитектурного вопроса для S6-42 это не оставляет: содержание решения записано здесь.

#### Обязательства для S6-41 и S6-42

- **S6-41.** (1) Любой store в сгенерированном коде повышает поколение затронутых страниц. Сейчас это путь helper через `commit_write`. Inline-store без helper допустим, только если делает то же: данные, затем `fetch_add(Release)` поколения каждой затронутой страницы, затем отчёт `committed_write`. (2) В параллельном режиме loads и stores не слабее схемы MEMORY_MODEL и S6-32 для параллельного режима; для цепочки G-XMC достаточно `Acquire` у loads и `Release` у stores. (3) Сериализующая инструкция в середине блока не компилируется (п. 2.4). (4) Сторожа STOP на каждой границе инструкции сохраняются (S1, S3). (5) Формы `LOCK` идут через helper и шлюз split `LOCK` S6-34, пока для выровненной формы не доказан host-атомарный lowering.
- **S6-42.** Реализовать A1–A6, D1–D4 (для будущих связей — проверку на переходе), M1–M6 и S1–S6. Тесты плана: (1) «vCPU A меняет код, vCPU B его исполняет»: после записи A и синхронизации гостя (сигнал и сериализующая инструкция или конец блока) следующий блок B собран из новых байт; (1b) после первого `Acquire`-чтения поколения на B, увидевшего новое значение, B не входит в блок с устаревшими байтами (без синхронизации гостя до этого чтения возможны повторные входы, текущий блок доисполняется не больше 31 инструкции после текущей); (2) освобождение под нагрузкой: каждый поток освобождает только свои слоты, обращений к освобождённой памяти нет, поколение слота растёт; (3) `stop_all` с включённым JIT: подтверждение `safe` только после возврата, снимок совпадает с интерпретатором. ISSUE-038 обновить.

#### Источники и оставшаяся сверка

- Intel SDM 253668-093US, §11.1.3 и §11.3. [Intel Volume 3A](https://cdrdv2.intel.com/v1/dl/getContent/671190).
- Синхронизация кешей и context synchronization сверены по материалам Arm с указанием DDI0487L.a B2.7.4.2 и B2.2.5 и по описанию полей EIS/EOS в SCTLR_EL1. Норма для S4 сверена по DDI0487H.a, §B2.2.1, с. B2-152–153. Она определяет атомарность однобайтового доступа. Время наблюдения STOP и порядок операций Rust определены отдельно. [Arm ARM DDI0487H.a, архивная копия](https://kib.kiev.ua/x86docs/ARM/ARMARMv8/DDI0487H_a_a-profile_architecture_reference_manual.pdf). [Архитектурное руководство Arm](https://developer.arm.com/community/arm-community-blogs/b/architectures-and-processors-blog/posts/caches-self-modifying-code-working-with-threads), [Arm SCTLR_EL1](https://support.arm.com/documentation/ddi0601/2026-09/AArch64-Registers/SCTLR-EL1--System-Control-Register--EL1-).
- Правило одной region, thread-local W/X и требование cache invalidation сверены по руководству, man page из macOS SDK 27.0 и публичному исходнику Apple. [Руководство Apple](https://developer.apple.com/documentation/apple-silicon/porting-just-in-time-compilers-to-apple-silicon), [libpthread](https://raw.githubusercontent.com/apple-oss-distributions/libpthread/main/man/pthread_jit_write_protect_np.3), [libplatform-375.120.2](https://raw.githubusercontent.com/apple-oss-distributions/libplatform/libplatform-375.120.2/src/cachecontrol/arm64/cache.s).
- Проектные источники: S3-25c, S3-28c/d, S4-15, S4-16 (этот файл); S6-17, S6-30b, S6-32, S6-33, S6-34, S6-35, S6-36 ([MACHINE_CONTRACT.md](MACHINE_CONTRACT.md), [MEMORY_MODEL.md](MEMORY_MODEL.md)); [ADR-0004](../decisions/ADR-0004-capacity-one-memory-cache.md), [ADR-0005](../decisions/ADR-0005-persistent-same-code-session.md); ISSUE-038 ([KNOWN_ISSUES.md](../KNOWN_ISSUES.md)).

### S6-41: атомарные операции и барьеры в lowering ARM64

Дата: 2026-11-18. S6-41 принят по результатам независимого ревью `gpt-6.1-sol/high` для реализованного ARM64-профиля. Проверены кодировки AArch64, lowering трёх классов эффектов, дифференциал LOCK-форм и litmus с JIT в обоих режимах. В параллельном режиме обычные production-доступы проходят через существующие helper с семантикой RAM. [Отчёт](../reports/STAGE_6_S6-41_2026-11-18.md).

**Что добавлено.**
- Энкодер: `LDR/STR{B,H,W,X}` с беззнаковым смещением (`ldr_sized`, `str_sized`), `LDAR/STLR{B,H,W,X}`, `LDADD`, `SWP`, `CAS` в формах `{B,H,W,X}` и двух порядках (`AtomicOrder::Relaxed` — без бит acquire/release, `AcquireRelease` — `…AL`), `DMB ISH` и `DMB ISHLD`. Других инструкций нет (`DMB ISHST`, `LDSET/LDCLR`, `CASP`, `LDAXP/STLXP` не добавлены: норма fence отображает `SFENCE` в `DMB ISH`, а формы `OR/AND/BTS/BTR` и 16-байтный примитив S6-31 не реализовала). `rt` у `LDADD`/`SWP` — только `XReg`: при `XZR` форма без `A` у `LDADD` — алиас `STADD` с другим порядком, поэтому такой операнд не выражается. Алиаса `STSWP` нет; довод в пользу `SWP` как самостоятельной формы — `XZR` в роли `rt` теряет acquire.
- IR: три вида эффекта, ни один не сводится к другому. Обычный доступ — прежние `CheckedLoad`/`CheckedStore`. Атомарная операция — новый `Operation::AtomicRmw { kind: Add | Exchange | CompareExchange, width, operand, value, expected, dst }` (`dst` — прежнее значение памяти; флаги выводит отдельный `Integer` из `dst`, они не часть атомарной операции). Fence — новый `Operation::Fence { kind: Full | Store | Load }` (`MFENCE`/`SFENCE`/`LFENCE`). `Operation::memory_class()` возвращает `Ordinary | Atomic | Fence`. `verify` проверяет: эффект внутри инструкции; `expected` есть ровно у `CompareExchange`; ширина равна размеру операнда; временные определены и нужной ширины; `dst` свежий (`VerifyError::InvalidAtomicRmw` и прежние ошибки). Оптимизаций IR нет: ни один проход не удаляет и не сливает `Fence`/`AtomicRmw`. Регистровые эмиттеры (`emitter.rs`, `block_emitter.rs`) явно отказывают на новых эффектах (`InvalidOperation`/`UnsupportedShape`), а не пропускают их.
- Lowering: `arm64::memory_order::lower_memory_effect(encoder, mode, &Operation, &HostOperands)`; режим — параметр `MemoryOrderMode { Interleaved, Parallel }` каждого вызова, не глобал.

**Отображение IR → инструкции** (ширина `B/H/W/X` по размеру операнда; регистры подбирает вызывающий):

| Эффект IR | `Interleaved` | `Parallel` |
|---|---|---|
| `CheckedLoad` | `LDR{B,H,W,X}` | `LDAR{B,H,W,X}` |
| `CheckedStore` | `STR{B,H,W,X}` | `STLR{B,H,W,X}` |
| `AtomicRmw::Add` (`LOCK ADD/INC/DEC/XADD`) | `LDADD{B,H,}` | `LDADDAL{B,H,}` |
| `AtomicRmw::Exchange` (`XCHG` с памятью) | `SWP{B,H,}` | `SWPAL{B,H,}` |
| `AtomicRmw::CompareExchange` (`LOCK CMPXCHG`) | `MOV dst, expected` (если регистры разные), `CAS` | то же, `CASAL` |
| `Fence::Full` (`MFENCE`) | ничего | `DMB ISH` |
| `Fence::Store` (`SFENCE`) | ничего | `DMB ISH` |
| `Fence::Load` (`LFENCE`) | ничего | `DMB ISHLD` |

`INC`/`DEC` — `Add` с константой `1` и `-1`; `ADD m, imm` — `Add` с константой. В режиме чередования нет ни бит порядка (в том числе у `LDADD`/`SWP`/`CAS`), ни `DMB` (тест `interleaved_mode_never_emits_an_ordering_bit_or_a_barrier`); `Fence` в этом режиме остаётся эффектом IR и выдаёт ноль слов. В параллельном режиме обычные доступы — `LDAR`/`STLR`, то есть `SeqCst` по AArch64 (RCsc), что не слабее нормы S6-32 и достаточно для цепочки G-XMC (`Acquire` у loads и `Release` у stores, S6-40, обязательство 2). Вывод для существующих программ: прежний код не меняется, потому что ни один адаптер декодера не выдаёт новые эффекты (проверено полным `cargo test -p od-jit`: 469 пройдено).

**Предусловия вызывающего** (inline-эмиттера памяти, которого пока нет): адрес в host-регистре уже переведён, естественно выровнен по ширине (`LDAR`/`STLR`/LSE иначе fault; split `LOCK` идёт через helper и шлюз S6-34), права проверены, перед записью выполнены правила обязательства 1 S6-40 (поколение страницы, `committed_write`). Нужна FEAT_LSE (Apple M1 и новее; хост S6-06). Для `CompareExchange` регистр результата не может совпадать с регистром адреса или значения (`MemoryOrderError::CompareExchangeOverlap`); регистр `expected` не портится.

**Проверки.** Байты: 47 слов в `tests/fixtures/arm64_memory_order.s` собраны Apple clang 21.0.0 (`-target arm64-apple-macosx15.0 -march=armv8.4-a`), прочитаны `otool -tV`/`otool -t`, сравнены с энкодером (`s6_41_memory_order_forms_match_local_apple_clang_fixture`); побочно — `s6_41_sized_loads_and_stores_scale_and_reject_bad_offsets`. Кодировки сверены с ассемблером, а не с текстом Arm ARM; номера разделов Arm ARM здесь не приводятся. Lowering: 8 тестов `arm64::memory_order::tests`, 3 теста `ir::tests::s6_41_*`, 1 тест эмиттера. Исполнение на хосте (`crates/od-host/tests/s6_41_atomic_lowering.rs`, Apple M3 Max): (а) дифференциал `od_isa::execute::run_with_cpl0_profile_set` против сгенерированного кода в обоих режимах на 11 формах (`LOCK XADD m32/m64`, `LOCK ADD m32/m64 reg`, `LOCK ADD m8 imm`, `LOCK INC m32`, `LOCK DEC m64`, `LOCK CMPXCHG m32/m64`, `XCHG m32/m64`), 96 входов на форму (края и случайные; у `CMPXCHG` половина — совпадающее сравнение), 2112 сравнений: байты операнда и охранной области (8 до, 16 после), прежнее значение, ZF и RAX у `CMPXCHG`; (б) четыре потока по 100 000 раз на `LDADDAL` W/X, `CASAL` W-цикл и `SWPAL` X — без потерянных обновлений; (в) litmus MP и SB на сгенерированном коде, 2 000 000 раундов на форму: `LDAR/STLR`, обычные `LDR/STR` с `DMB ISH`/`DMB ISHLD` из `lower_fence` — запрещённых исходов 0; контроль без барьеров (печатается, не проверяется) показал слабые исходы (MP 38, SB 65 977), то есть проверка способна заметить нарушение.

**Отступление от таблицы S6-32 (решение по M1 ревью, принято).** В таблице S6-32 ([MACHINE_CONTRACT.md](MACHINE_CONTRACT.md), строка «`LOCK`-операции, `XCHG`, `compare_replace_u64`») для режима чередования записано `SeqCst` («не менялось»). Здесь в режиме чередования `LDADD`/`SWP`/`CAS` идут БЕЗ бит порядка — это отступление от этой строки. Обоснование: в режиме чередования все vCPU и все источники записи устройств исполняются на одном host-потоке ([MEMORY_MODEL.md](MEMORY_MODEL.md), строка 17), биты порядка там не наблюдаемы; план S6-32 требует отсутствия барьеров и порядка для чередования. Обязательство: привести к паритету с таблицей (биты `AL`), если появится многопоточное исполнение режима чередования (не ожидается). Тест `interleaved_mode_never_emits_an_ordering_bit_or_a_barrier` закрепляет текущее поведение и при таком изменении правится вместе с этим абзацем.

**Норма fence и JIT-код (m2).** Норма S6-32 «только `std::sync::atomic::fence` (не inline-asm `DMB`)» относится к Rust-коду (хост-примитивы, интерпретатор). В сгенерированном JIT-коде действует строка «Отображение в хост: aarch64» той же нормы: `MFENCE` — `DMB ISH`, `SFENCE` — `DMB ISH`, `LFENCE` — `DMB ISHLD` (таблица выше). `DMB` в JIT-коде — не нарушение нормы Rust-кода.

**Предусловие S6-41b и S6-42 (m4).** Код, собранный в режиме Interleaved, не исполняется в режиме Parallel: либо ключ кэша блоков включает `ExecutionMode`, либо кэш очищается при смене режима. Режим фиксируется до выдачи адаптеров (S6-32, шаг 2). Это предусловие допуска любых атомарных форм в блок; в S6-41 его нет, потому что ни один блок их не содержит.

**Политика без FEAT_LSE (m7).** На `aarch64-apple-darwin` LSE — базовая (Apple M1 и новее). На ином aarch64-хосте перед параллельным JIT проверять `is_aarch64_feature_detected!("lse")`; без LSE параллельный JIT отказывает (остаётся интерпретатор и helper), LL/SC-вариант — отдельный срез, не часть S6-41/S6-41b.

**Допуск `LOCK`-форм и флаги (M3).** Допуск `LOCK`-форм в блок требует сравнения полного набора флагов (CF/PF/AF/ZF/SF/OF) с интерпретатором — S6-41b. Сейчас сравнивается только ZF у `CMPXCHG`, и он выводится из прежнего значения теста (`s6_41_atomic_lowering.rs`, ветка `Returns::CmpxchgRax`: `zf == (old == expected & width_mask)`, где `zf` — флаг интерпретатора), то есть проверяет прежнее значение и RAX, а не JIT-флаг. JIT-код ZF у `CMPXCHG` не порождает (в IR флаги выводит отдельный `Integer`, в ARM-эмиттере этот путь не собран).

**Замечания по формулировкам (nits).**
- Тест `atomic_program` в `ir.rs` строит `Integer Compare` с `lhs = old`, `rhs = expected`; слова «`LOCK CMPXCHG`-shaped» в старом комментарии вводили в заблуждение: у x86 флаги `CMPXCHG` — от аккумулятора минус операнд памяти (`expected - old`), а не `old - expected`. Комментарий в коде исправлен; настоящая форма флагов — S6-41b (г).
- «Единственное отображение `SeqCst`» в отчёте читать как: те же инструкции, что Rust `SeqCst` в интерпретаторе, а не доказанную единственность.
- Litmus «plain + parallel fences» (обычные `LDR/STR` с `DMB ISH`/`DMB ISHLD`) — не производственная форма; производственная — `LDAR`/`STLR` и `DMB`, строго сильнее. Числа контроля без барьеров (MP 38, SB 65 977) — пример одного прогона, а не норма.

**Границы первого среза S6-41 и продолжение.**
- Адаптер, inline-эмиттер атомарного доступа и допуск форм реализованы в S6-41b/c/d. Обычные load/store в атомарном блоке остаются вне профиля.
- Флаги `LOCK XADD`/`ADD`/`CMPXCHG` в дифференциале не сравниваются, кроме ZF у `CMPXCHG` (он выведен из прежнего значения, см. «Допуск `LOCK`-форм и флаги (M3)»): lowering их не считает (IR выводит их `Integer`), этот путь в ARM-эмиттере не собран — S6-41b (г).
- Litmus S6-19/S6-36 с JIT принят в S6-41e для атомарных форм в обоих режимах. Обычные MOV корпуса выполняет интерпретатор. Чувствительность гостевого SB остаётся открытой. [ISSUE-516](../KNOWN_ISSUES.md#issue-516-s6-41e-sb-observer-ordering).
- Измерения только на Apple M3 Max, не на Mac mini. Слабые модели вне aarch64-apple-darwin не рассматривались.
- В S6-06 `LDAR`/`STLR` «измерены, но не выбраны» как inline-asm вариант к Rust `SeqCst`; для сгенерированного кода Rust-атомиков нет, и `LDAR`/`STLR` — отображение `SeqCst` на AArch64. Это решение исполнителя; ревьюеру подтвердить.

**Не закрыто (явный список; всё — S6-41b, `STAGE_6.md`).**
- (а) адаптер декодера, выдающий `AtomicRmw`/`Fence`; (б) inline-эмиттер памяти по обязательствам 1 и 5 S6-40; (в) допуск `LOCK`/`XCHG`/`MFENCE` в блок.
- (г) полный набор флагов CF/PF/AF/ZF/SF/OF в дифференциале (ZF у `CMPXCHG` JIT-кодом не порождается).
- (д), (м) `LOCK OR/AND/BTS/BTR` — `LDSET`/`LDCLR` либо цикл на `CAS`, с тестами на байты.
- (е) аллокатор (`allocator.rs`): `AtomicRmw` молча пропускается через `_ => {}` — явный учёт временных или явный отказ.
- (ж) правило `verify` для `AtomicRmw`, аналогичное `CheckedStoreCommitOrder`.
- (з) ключ кэша по `ExecutionMode` либо очистка при смене режима (предусловие, см. выше).
- (и) поправка к инварианту (в) S6-30a «`unsafe` только в макросах `ram_storage.rs`», если inline-доступ идёт по сырому host-указателю.
- (к) litmus S6-19/S6-36 с включённым JIT в обоих режимах.
- (л) FEAT_LSE на не-Apple aarch64 (политика выше).

Состояние этого списка после S6-41b — в следующем разделе.

### S6-41b: inline-эмиттер атомарных операций и барьеров в блоке JIT

Дата: 2026-11-18. S6-41b принят после независимого ревью `gpt-6.1-sol/high`. Поддержаны выровненные атомарные операции над RAM шириной 8, 32 и 64 бита и fences. Компилятор V4 допускает 1–16 атомарных инструкций на блок. Обычные production-доступы используют существующие helpers. Смешанные блоки с обычными inline load/store, ширина 16 бит, FS/GS, регистровый индекс BTS/BTR и LL/SC для хостов без LSE остаются вне профиля. Пункты (б), (в), (з) и (к) подтверждены срезами S6-41c/d/e. [Отчёт](../reports/STAGE_6_S6-41b_2026-11-18.md).

**Результат исходного среза до S6-41c/d/e.**
- **(а) Адаптер декодера — сделано.** Декодер `od_isa::long_decode::decode` отдаёт готовые формы публично (`Instruction.operation`: `LockAddMemReg`, `LockAddMem`, `LockIncMem`, `LockDecMem`, `LockXaddMem`/`XaddMemory`/`XaddDsMem`, `XchgMemory`, `LockCmpxchgMem`/`CmpxchgMemory`, `LockOrMem`, `LockAndMem`, `LockBitMemImm`, `Mfence`/`Lfence`/`Sfence`), `od-isa` не менялся, блокера нет. `atomic_adapter::lower_atomic_instruction(&Instruction, guest_rip)` строит проверенную `verify` одноинструкционную `Program`. Новые IR-эффекты: `AtomicRmwKind::{Or, ClearBits}` (`LDSET`, `LDCLR`), `Operation::AtomicFlags { kind, width, lhs, rhs }` (флаги по старому значению памяти и операнду; `AtomicFlagsKind::{Add, AddKeepCarry, Subtract, SubtractKeepCarry, And, Or, BitTest}` с `written_mask`), `Operation::WriteGprIfNotEqual` (аккумулятор `CMPXCHG` получает наблюдаемое значение только при несовпадении; при совпадении не трогается, в том числе старшие 32 бита при `m32`). Отображение: `ADD`/`XADD` — `Add`; `INC` — `Add` с 1; `DEC` — `Add` с -1 и флаги вычитания 1 без CF; `CMPXCHG` — `CompareExchange`, флаги `RAX − память`; `XCHG` — `Exchange`; `OR`, `BTS` — `Or`; `AND` — `ClearBits` с дополнением маски; `BTR` — `ClearBits`; `MFENCE`/`SFENCE`/`LFENCE` — `Fence`. Явные отказы (`AtomicAdapterError`): `LOCK BTS/BTR m, r` с регистровым смещением (адрес элемента зависит от регистра, а `MemoryOperand` статический), регистр данных вне RAX..RBX или не полный, не атомарная операция. Формы FS/GS и `XADD m8/m16` в эти операции не декодируются.
- **(г) Полный набор флагов — сделано (в дифференциале).** `arm64::atomic_sequence::lower_atomic_flags` считает CF/PF/AF/ZF/SF/OF обычными целочисленными инструкциями без вызова helper, для ширин 8, 16, 32 и 64 (в дифференциале проверены 8, 32 и 64; **ширина 16 не проверена**: декодер `od_isa` не выдаёт `LOCK ADD/OR/INC/DEC m16` с префиксом 0x66 — проверено формами `66 f0 ff /0`, `f0 66 ff /0`, `f0 66 81 /0`, их отвергает декодер; поэтому приём `OperandSize::Word` адаптером (`ANY_SIZE`) недостижим из декодированного кода и остаётся непроверенным до S6-41c/d), по формулам `arithmetic_result`/`logic_result` интерпретатора (сложение: перенос и знаковое переполнение; вычитание: заём; логика: CF = OF = 0, AF не пишется; `INC`/`DEC` не пишут CF; `BTS`/`BTR` пишут только CF). `emit_atomic_program` сливает их в RFLAGS под `written_mask`. ZF у `CMPXCHG` теперь настоящий флаг JIT-кода (флаги вычитания `RAX − память`), а не вывод из прежнего значения. Дифференциал `s6_41b_adapted_atomic_sequences_equal_the_interpreter_with_all_flags_in_both_modes`: 58 форм, 400 раундов на форму (100 из них — перебор краевых пар, остальные случайные; начальные CF/PF/AF/ZF/SF/OF случайные), 2 режима, 46 400 сравнений; сверяются байты операнда и охранной области, все 16 GPR-ячеек состояния и весь RFLAGS. Проверка на зубы (assert): учитываются только флаги, которые форма пишет (CF у `INC`/`DEC` сохраняется и в счёт не идёт); все шесть флагов принимали оба значения у add-подобных и у sub-подобных форм (CF — от форм, которые его пишут), PF/ZF/SF — у `OR`/`AND`, CF — у `BTS`/`BTR`; ручная мутация (сдвиг OF с 11 на 10) тест роняет. Формы: S6-41 (`XADD`, `ADD`, `INC`, `DEC`, `CMPXCHG`, `XCHG`) с источником RBX и RCX, `ADD m8/m32/m64` с непосредственным, `OR m8/m32/m64`, `AND m8`, `BTS`/`BTR m32/m64` с индексами 0, 5, 31, 33, 63, 200, три fence. Этот ARM64-код **не** заменяет и не использует IR-эффект `Integer` с helper флагов блока (тот остаётся только `W64`).
- **(д), (м) `LOCK OR/AND/BTS/BTR` — сделано на уровне кодирования, IR, lowering, адаптера и дифференциала, в блок не допущено (в).** Энкодер: `LDSET`/`LDCLR` `{B,H,W,X}` в двух порядках; байты сверены с Apple clang 21.0.0 (`-target arm64-apple-macosx15.0 -march=armv8.4-a`, `otool -tV`/`otool -t`) по фикстуре `arm64_atomic_logic.s` (тест `s6_41b_atomic_logic_and_flag_forms_match_local_apple_clang_fixture`: 17 слов `LDSET*`/`LDCLR*` и все новые логические, битовые формы и `CSET` — `ORR`, `EOR`, `ORN`, `LSL`, `LSR`, `UBFX`, `CSET`). Выбран `LDSET`/`LDCLR`, а не цикл на `CAS` (одна атомарная инструкция, как у остальных форм). Многопоточная проверка: `s6_41b_or_and_clear_bits_claim_each_bit_exactly_once_across_threads` (пул из 4 потоков на весь тест, старт каждой фазы по `std::sync::Barrier`; 5000 раундов, раунд — цикл на одной ячейке: все 64 бита выставляет `LDSETAL`, барьер, все 64 бита сбрасывает `LDCLRAL`; каждый бит в каждой фазе захвачен ровно одним потоком, ячейка после раунда пуста). Конкуренция — по барьеру, не гарантирована на всех прогонах: поток может опоздать на загруженном host; тест проверяет кодирование и семантику атомарного захвата бита, а на частоту пересечений не претендует. (Первая версия порождала потоки заново каждый раунд: 64 операции ≈ 1 мкс против десятков мкс на порождение, потоки почти не пересекались; исправлено по ревью.)
- **(е) Аллокатор — сделано (явный отказ).** `Allocation::build` возвращает `AllocatorError::UnsupportedEffect { operation }` на `AtomicRmw`, `AtomicFlags`, `WriteGprIfNotEqual`: аллокатор не моделирует их временные (`dst`, `value`, `expected`, `lhs`, `rhs`). Прежние `_ => {}` в `collect_definitions` и `compute_liveness` заменены перечнем вариантов, поэтому новый вариант `Operation` не пройдёт молча. Потребитель этих эффектов — `atomic_sequence`, у него свой план регистров (`x2..x8` временные, `x9..x16` флаги, `state`/`address` вне `x2..x16`; больше семи временных — `TooManyTemporaries`). Тесты: `atomic_adapter::tests::atomic_programs_are_refused_by_the_block_emitters`, `emitter::tests::s6_41_data_effect_emitter_refuses_atomic_and_fence_effects` (обновлён: для `AtomicRmw` отказывает аллокатор, для `Fence` — эмиттер).
- **(ж) Правило `verify` — сделано.** `VerifyError::AtomicRmwOrder`: после `AtomicRmw` в той же гостевой инструкции нет `CheckedLoad`, `CheckedStore`, другого `AtomicRmw`, и `CheckedStore` не делит с ним инструкцию (store до атомарной операции уже запрещён `CheckedStoreCommitOrder`, правило служит страховкой). Флаги, запись регистра и `Fence` после неё допустимы. Новые ошибки `InvalidAtomicFlags`, `InvalidConditionalWrite` проверяют форму `AtomicFlags` и `WriteGprIfNotEqual`. Тесты `ir::tests::s6_41b_*` (5).
- **(з) Ключ кэша по режиму — сделано для фикстурных кэшей, для production-сессии не требуется до (в).** `ExecutionProfile::memory_order: MemoryOrderMode` входит в `ExecutionContextSnapshot`, а через него в каждый `TranslationKey`; `MemoryOrderMode` стал `Hash`. Константы профилей — `Interleaved`; `with_memory_order` строит профиль другого режима. Фикстурные кэши (`SingleEntryBlockCache`, `MultiEntryBlockCache`) принимают только `Interleaved` (`is_supported_cached_fixture`), `Parallel`-профиль отвергают `UnsupportedProfile`, что совпадает с A5 S6-40; код `Interleaved` из такого кэша в `Parallel`-контексте не выдаётся ни по ключу, ни по допуску профиля. Тесты: `translation::tests::s6_41b_translation_key_distinguishes_the_memory_order_mode`, расширен перечень `unsupported_profiles` в тесте `dispatch`. **Граница:** `PagedFetchedLoadSession` (production) ключует допуск по байтам и поколению, режима не несёт, и пока не нужно: её блоки содержат только helper-формы, байты которых от режима не зависят. Поле режима и сброс допуска при его смене надо добавить в сессию вместе с (в); теста «код Interleaved не исполняется в Parallel» на production-пути нет и быть не может, пока нет различия кода.
- **(и) Поправка к инварианту (в) S6-30a — записана текстом** в [MACHINE_CONTRACT.md, S6-30a](MACHINE_CONTRACT.md#s6-30a--хранение-ram-и-функции-доступа) («Поправка S6-41b к инварианту (в)»): вступает в силу с inline-эмиттером; код не менялся.
- **(л) FEAT_LSE — сделано (политика и проверка).** `memory_order::host_has_lse()`: на `aarch64-apple-darwin` `true` без пробы; на ином aarch64 — `is_aarch64_feature_detected!("lse")`; вне aarch64 — `false`. `mode_admitted_on_host(mode)` равна ей для обоих режимов: все атомарные формы (в том числе чередования) — инструкции LSE, поэтому без LSE отказывают и параллельный JIT, и inline-атомарные формы чередования (остаётся интерпретатор и helper); LL/SC — отдельный срез. Вызывающего нет (нет допуска в блок): проверка и тест `s6_41b_lse_policy_matches_the_host` готовы к подключению в (в).

**Последовательность одной гостевой инструкции.** `emit_atomic_program(encoder, &SequenceEnv { mode, state, address }, &Program)` проверяет программу (`verify`), требует ровно одну гостевую инструкцию и выдаёт: чтение регистров из `LongModeState`, атомарную операцию по `lower_memory_effect` (`mode` выбирает биты порядка и `DMB`), флаги, запись регистров, `RFLAGS`. Предусловия вызывающего (то, что и есть (б)): `address` — уже переведённый, естественно выровненный, проверенный по правам host-указатель на записываемую память; после последовательности вызывающий повышает поколение страницы и сообщает `committed_write`; после атомарной операции ничего не может завершиться отказом (правило (ж)). Тесты: 4 в `atomic_sequence::tests` (режимы отличаются только словом атомарной операции, `Fence` отдельно, отказы, форма флагов), 5 в `atomic_adapter::tests`.

**Litmus на сгенерированном коде (не S6-19/S6-36).** `s6_41b_store_buffering_with_xchg_in_parallel_mode_has_no_forbidden_outcome`: два потока, каждый `XCHG [своя], 1` (адаптер, IR, последовательность, режим `Parallel`) и затем `LDAR` чужой переменной; 1 000 000 раундов, запрещённых исходов (0, 0) — 0. Это доказательство, что `SWPAL` + `LDAR` на этом host порядок даёт; это не заменяет litmus S6-19/S6-36. **Что тест изолирует.** `STLR` + `LDAR` уже даёт 0 запрещённых исходов в SB на этом host (2 млн раундов, S6-41, `s6_41_ordering_litmus_on_generated_code`), поэтому тест с `XCHG` изолирует только бит release у `SWPAL` (барьерная роль самого `SWP` без него не проверена). Контрольная форма — relaxed `SWP` (без бит acquire/release) + обычный `LDR` с ожиданием, что исход (0, 0) наблюдается (доказательство, что тест способен ловить нарушение) — **желательна и не выполнена**.

**Ограничения исходного среза до S6-41c/d/e.**
- **(б) inline-эмиттер памяти — не сделан.** Нужно, и этого нет: (1) host-указатель на байты RAM для гостевого диапазона. Сегодня блок обращается к памяти только через helper с копированием (`CheckedMemoryHelperV1`, `GuestMemory::read`/`write`), а `PhysicalMemory`/`GuestMemory` указателя не отдают; (2) перевод линейного адреса и проверка прав и выравнивания внутри блока (обход страниц гостя в сгенерированном коде не пишется; значит, нужен helper «перевести и проверить», который возвращает указатель или исход fault, и новый вид выхода блока); (3) расширение `RuntimeContext` (новая версия ABI: указатель на helper, указатель на массив поколений страниц, буфер отчёта `committed_write`) и точки PC-map для fault до атомарной операции; (4) код `fetch_add(Release)` по поколению каждой затронутой страницы и отчёт `committed_write` после записи (обязательство 1 S6-40); (5) поправка (и) становится нормативной. Это отдельный срез эмиттера — **S6-41c** (номер назначен при ревью S6-41b, координатор может переименовать). Последовательность и флаги из этого среза — его готовая часть. Обязательства среза — в абзаце «Обязательства среза (б)» ниже.
- **(в) допуск в блок — не сделан.** До (б) отказ явный: аллокатор (`UnsupportedEffect`) и эмиттеры (`InvalidOperation`, `UnsupportedShape`); обязательство 5 S6-40 остаётся в силе (формы `LOCK` идут через helper и шлюз split `LOCK` S6-34). Условия допуска: (б), (з) для production-сессии, проверка (л) при создании кэша параллельного режима, повтор дифференциала через блок, а не через последовательность. Срез — **S6-41d** (зависит от S6-41c); в него входят аллокатор и блок-эмиттеры, принимающие атомарные эффекты (или блок из одной атомарной инструкции), режим в ключе допуска `PagedFetchedLoadSession` со сбросом при смене режима и тест «код Interleaved не исполняется в Parallel», вызов `host_has_lse`/`mode_admitted_on_host` при создании кэша `Parallel`, повтор дифференциала через блок, включая fault-пути.
- **(к) litmus S6-19/S6-36 с включённым JIT — не запущен.** Причины: (1) `od-machine` исполняет litmus интерпретатором (`parallel_run.rs`); JIT-сессия живёт в `od-host`, который зависит от `od-machine`, обратной зависимости нет; (2) paged-путь JIT берёт `&mut MachineBus` (шина под `Mutex` в тестах S6-42), поэтому параллельное исполнение гостевых программ litmus через JIT над общей RAM сегодня невозможно (S6-42b); (3) формы MP/SB с `LOCK`/`XCHG`/`MFENCE` не допущены в блок (в) и ушли бы в интерпретатор, то есть JIT litmus не проверял бы. Не обойдено: тест на сгенерированном коде выше и MP/SB S6-41 — дополнительные доказательства. Срез — **S6-41e** (зависит от S6-41d и S6-42b): точка подключения JIT к прогону litmus на стороне `od-host` (`od-machine` от `od-host` не зависит).
- Не изменялись и открыты: `LOCK BTS/BTR m, r` с регистровым смещением (адаптер отказывает), LL/SC без LSE, блоки из нескольких атомарных инструкций (адаптер выдаёт одну инструкцию на программу), пометки «по памяти» S6-40.

**Следствия для S6-42 и S6-43a.** «Inline-формы — после S6-41b» в ограничении приёмки S6-42 остаётся в силе: параллельный режим допускает только memory-формы через helper и `commit_write`. S6-43a (через S6-42b) не разблокирован: ему нужны S6-41c ((б)), S6-41d ((в)) и S6-41e ((к)).

**Обязательства среза (б) (S6-41c), записаны при ревью.**
- (а) **Ветка неравенства `CMPXCHG` и права на запись.** `CMPXCHG` на x86 при неравенстве всё равно пишет назначение (прежнее значение): на странице только для чтения это #PF, у страницы выставляется бит D. ARM `CAS` при неравенстве память не пишет. Поэтому перевод адреса с намерением записи и выставление бита D должны покрывать и ветку неравенства (проверка прав и D-бит до атомарной операции, независимо от исхода сравнения); иначе JIT расходится с интерпретатором по #PF и по D-биту.
- (б) **`CASAL` при неравенстве и барьер.** По памяти (Arm ARM, правила `CAS`, не сверено по тексту): при неравенстве `CASAL` не имеет release-семантики для записи (записи нет). Свойство «неудачный `LOCK CMPXCHG` — полный барьер» держится, пока обычные доступы в `Parallel` — `LDAR`/`STLR` (RCsc): acquire-чтение у `CASAL` плюс последующие `LDAR` его сохраняют. Если обычные доступы ослабят (`LDAPR`, `LDR`), после `CAS` нужен `DMB ISH`. Условие записать в S6-41c рядом с выбором форм обычных доступов.

**Итог ревью Opus (2026-11-18): «принять частично», блокеров нет.** Исправлено по ревью: (1) тест `LDSET`/`LDCLR` — барьер старта и пул потоков (см. (д)); (2) абзац «Litmus на сгенерированном коде» — что тест изолирует; (3) формулировки о зубах дифференциала — только записываемые флаги, добавлены проверки PF/ZF/SF для `OR`/`AND` и CF для `BTS`/`BTR`; (4) ширина 16 — записана как непроверенная, декодер её не выдаёт (список «Не сделано», пункт (г)); (5) комментарий к `LOCK AND` в `atomic_adapter.rs` — декодер выдаёт только байт; (6) обязательства среза (б) — выше; (7) граница (з) записана и в открытые пункты S6-42b (раздел S6-42). Новые срезы (номера назначены при ревью, координатор может переименовать): **S6-41c** — (б) inline-эмиттер памяти; **S6-41d** — (в) допуск в блок; **S6-41e** — (к) litmus с JIT. Планы — `docs/plans/STAGE_6.md`. Семантика адаптера и флагов при ревью не менялась.

### S6-41c: inline-эмиттер памяти для атомарных операций

Дата: 2026-11-18. Статус: **принято** после независимого ревью `gpt-6.1-sol/high` для блока из одной поддержанной атомарной инструкции. Закрывает пункт (б) S6-41b (обязательства 1 и 5 S6-40 и «Обязательства среза (б)» выше) для блока из одной атомарной гостевой инструкции; допуск такого блока в `PagedFetchedLoadSession` и в блоки из нескольких инструкций — S6-41d, litmus с JIT — S6-41e. Код: `crates/od-machine/src/ram_storage.rs`, `crates/od-machine/src/physical_memory.rs`, `crates/od-machine/src/paged_memory_adapter.rs` (указатель, окно, обход), `crates/od-jit/src/runtime.rs` (`RuntimeContextV4`), `crates/od-jit/src/atomic_block.rs` (новый, блок), `crates/od-host/src/atomic_inline.rs` (новый, helper и запуск); тесты — там же и `crates/od-host/tests/s6_41c_atomic_inline.rs` (новый). Поправка (в′) к S6-30a — [MACHINE_CONTRACT.md, S6-41c](MACHINE_CONTRACT.md#s6-41c--указатель-на-операнд-ram-и-поправка-в-к-s6-30a-нормативно) (нормативна). Отчёт — [STAGE_6_S6-41c_2026-11-18.md](../reports/STAGE_6_S6-41c_2026-11-18.md).

**Что сделано по пунктам среза.**
1. **Указатель на выровненный диапазон RAM.** `RamStorage::operand_ptr(offset, width)` (`ram_storage.rs`): единственная функция вне макросов с `unsafe` над хранением; ширины 1/2/4/8, выравнивание проверяется по host-указателю; срок действия — жизнь `PhysicalMemory`, указатель не выдаётся за пределы вызова (`PhysicalMemory::atomic_operand_window` возвращает `AtomicOperandWindow<'m>`, заимствующее память). Поправка (в′) записана в MACHINE_CONTRACT как нормативная.
2. **Helper «перевести и проверить» с намерением записи.** `od_machine::locate_paged_atomic_operand` (обход `locked_target`: чтение, затем запись, права, dirty-бит, региональная проверка; невыровненный — до обхода) и `od_host::atomic_inline::locate_helper` (`extern "C"`, адрес — из состояния гостя и `MemoryOperand::access_address_48`, затем обход, затем окно). Результат — указатель и счётчик поколения либо исход: `RUNTIME_EXIT_V4_ATOMIC_FAULT` (#PF/#GP/#SS, детали в `RuntimeContextV4::outcome`, запись `PageFaultRecord` у хоста), `RUNTIME_EXIT_V4_ATOMIC_DECLINED`, `RUNTIME_EXIT_V4_HELPER_FAILURE`. Ветка неравенства `CMPXCHG`: обход с намерением записи выполняется всегда, до сравнения, поэтому #PF на странице только для чтения и dirty-бит на записываемой те же, что у интерпретатора (тест `s6_41c_an_unequal_cmpxchg_takes_the_write_intent_like_the_interpreter`).
3. **`RuntimeContextV4`** (`runtime.rs`, рядом с V1–V3, их определения не менялись; `RuntimeContextV1` — первый член, префикс V1 сохранён): указатель на helper (`locate_helper`), `host`/`site` (непрозрачные указатели helper), выходные поля helper (`operand_host`, `generation: [*const AtomicU64; 2]`, `generation_count`, диапазон `committed_start/len`, `outcome: CheckedMemoryOutcomeV1`), выходное поле блока `committed_valid`. Отдельный тип входа `BlockEntryV4`; V1, V2, V3 и все пути S3/S4 не затронуты.
4. **Точка PC-map до атомарной операции.** `AtomicBlockLayout::pc_map` (`atomic_block.rs`): `BeforeEffects` от слова 0 до `effects_start`, `InInstruction`, `AfterInstruction`; helper вызывается и его статус проверяется до `effects_start`, поэтому любой выход по fault или отказу оставляет CPU, RAM, RIP и retired нетронутыми. Структурная проверка — слова до `effects_start` не содержат записи через `x20` (состояние) или `x1` (операнд) и ни одной атомарной инструкции (`the_exits_of_the_helper_lie_before_every_effect_of_the_instruction`); поведенческая — `s6_41c_a_fault_before_the_atomic_operation_leaves_everything_untouched`.
5. **Поколение и `committed_write`.** После последовательности блок делает `LDADDAL` (acquire-release, не слабее `Release`) по каждому из первых `generation_count` счётчиков (`LDADDAL` и в режиме чередования: `Release` повышения в Rust-пути `commit_write` тоже не зависит от режима), затем `committed_valid = 1`; хост после возврата блока вызывает `committed_write(operand)`. Порядок: данные, `Release`-поколение, отчёт (S6-30b). Поколение повышается безусловно, в том числе при неравенстве `CMPXCHG` и при `LOCK OR` с нулевой маской: так делает и интерпретатор (`PhysicalMemory::atomic_compare_exchange` сообщает запись всегда), лишнее повышение безопасно.
6. **Невыровненный и split-доступ.** Выбор: helper **отказывает** (`DECLINED`) до обхода страниц, без эффекта и без счёта в `split_lock_refusals`; такую инструкцию исполняет шаг интерпретатора со шлюзом split `LOCK` S6-34 (шаг 8). Inline-пути в шлюз нет. Тест `s6_41c_a_misaligned_or_split_operand_is_declined_and_the_interpreter_owns_it` (внутри страницы и через границу страницы; после отказа интерпретатор даёт то же состояние, что и без JIT).
7. **Поправка (и) S6-41b = (в′) нормативна:** `unsafe`, касающийся байт RAM, — только `ram_storage.rs` (макросы и `operand_ptr`); в эмиттере блока (`atomic_block.rs`, `atomic_sequence.rs`) `unsafe` нет, он выдаёт слова команд. `unsafe` в `od-host/atomic_inline.rs` — вызов опубликованного кода и разыменование записи вызова в `extern "C"` helper (той же категории, что `memory_site_helper_v2`); байтов RAM он не трогает.

**Форма блока** (`compile_atomic_block(&Program, MemoryOrderMode)`): пролог (`x19` — запись вызова, `x20` — состояние), сторожа бюджета и STOP-байта до любого эффекта, вызов helper (`blr`, статус `!= 0` возвращается блоком как есть), `emit_atomic_program` с `address = x1` (host-указатель из helper), подъём поколения, RIP, `remaining_budget`, `retired_instructions`, эпилог. Блок без операнда (`MFENCE`/`LFENCE`/`SFENCE`) helper не вызывает. Отказ компиляции без FEAT_LSE (`mode_admitted_on_host`, политика (л) S6-41b). `stop_rip` блок V4 не поддерживает: хост-обёртка всегда ставит `stop_rip_enabled = 0`.

**Тесты (по именам).**
- `od-machine --lib` (5): `ram_storage::tests::operand_ptr_is_naturally_aligned_in_range_and_refuses_everything_else`, `ram_storage::tests::a_host_atomic_through_the_operand_pointer_is_seen_by_the_safe_loads`, `physical_memory::tests::the_atomic_operand_window_serves_aligned_ram_and_refuses_the_rest`, `paged_memory_adapter::tests::locating_an_atomic_operand_leaves_what_the_interpreters_unequal_cmpxchg_left`, `paged_memory_adapter::tests::a_misaligned_atomic_operand_is_declined_before_any_walk`.
- `od-jit --lib` (4): `atomic_block::tests::the_exits_of_the_helper_lie_before_every_effect_of_the_instruction`, `…the_generation_is_raised_after_the_data_word_with_release_in_both_modes`, `…a_fence_block_has_no_helper_and_no_generation_raise`, `…programs_outside_the_profile_are_refused`.
- `od-host --test s6_41c_atomic_inline` (9): (в) `s6_41c_inline_atomic_blocks_equal_the_interpreter_with_the_walk_and_the_generation` — 27 форм (ADD/XADD/INC/DEC/CMPXCHG/XCHG/OR/AND/BTS/BTR, ширины 8/32/64) × 2 режима × 48 раундов = 2592 сравнения блока (операнд в RAM по линейному адресу через таблицы страниц) с `od_isa` (набор CPL0 над тем же адаптером): GPR, RIP, RFLAGS, 8 КиБ данных (две страницы), записи таблиц, поколения страниц данных и таблиц, последовательность `committed_write` (dirty-бит обхода, затем операнд); (а) `s6_41c_a_fault_before_the_atomic_operation_leaves_everything_untouched` — страница только для чтения (все формы, #PF с кодом P|W) и страница без present (каждая третья форма), оба режима: CPU, RAM, RIP, retired, поколение нетронуты, таблицы и #PF (CR2, код) равны интерпретатору; (б) `s6_41c_another_vcpu_sees_the_generation_and_the_committed_write_after_the_atomic_write` — 3000 раундов `LOCK XADD m64`: поток B дожидается счётчика через `Acquire`, читает ячейку (она уже содержит сумму раунда), получает отчёт по каналу (колбэк потока A читает счётчик внутри себя и видит значение раунда: отчёт после поколения); (г) `s6_41c_an_unequal_cmpxchg_takes_the_write_intent_like_the_interpreter` — несовпадение на странице только для чтения: #PF и таблицы равны интерпретатору, dirty-бита нет; на записываемой странице без dirty: dirty-бит поставлен, память не изменена, поколение +1 как у интерпретатора; (д) `s6_41c_a_misaligned_or_split_operand_is_declined_and_the_interpreter_owns_it`; дополнительно `s6_41c_a_fence_block_runs_without_the_helper_and_changes_only_rip_and_the_budget`, `s6_41c_the_guards_leave_before_the_helper_with_no_effect` (бюджет 0, поднятый STOP-байт: helper не вызван), `s6_41c_the_block_raises_every_generation_the_helper_returns` (синтетический helper с двумя счётчиками), `s6_41c_a_declining_helper_status_is_returned_before_the_atomic_operation`.

**Ограничения (записаны).**
- Профиль блока — **одна атомарная инструкция** (S6-41d снимает ограничение). `PagedFetchedLoadSession` не допускает `LOCK`/`XCHG`/`MFENCE` в блок до S6-41d: `AtomicInlineBlock` — отдельный тип с вызовом `run`, в сессию и в цикл S3-38o не подключён; ключ допуска по режиму и вызов `mode_admitted_on_host` при создании кэша `Parallel` — там же (здесь отказ компиляции без LSE).
- Запись на страницу кода активного блока не проверяется helper: сессия (S6-41d) обязана вывести `CodeInvalidated` из сообщённой `committed_write` (S4-16i), как для store.
- Litmus S6-19/S6-36 с JIT — S6-41e; FEAT_LSE — политика (л) S6-41b без изменений (на ином aarch64 без LSE блок не строится).
- A7 S6-40: блок и helper замков S6-33 не берут, `PhysicalMemory` в тестах лежит вне шины (общая ссылка); production-путь под `Mutex<MachineBus>` остаётся S6-42b. Параллельность проверена двумя потоками (тест (б)) и не заменяет S6-42b.
- Ширина 16 не проверена (декодер `od_isa` не выдаёт `LOCK … m16`); `LOCK BTS/BTR m, r` с регистровым индексом адаптер отказывает; FS/GS-формы не декодируются; `stop_rip` в V4 нет.
- Обязательство (б) S6-41b «`CASAL` при неравенстве и барьер»: форма обычных доступов не менялась (в блоке нет обычных доступов, только атомарные инструкции и `LDADDAL` поколения); условие «если обычные доступы ослабят до `LDAPR`/`LDR`, нужен `DMB ISH` после `CAS`» остаётся в силе, S6-41c его не снимает.
- Предел покрытия A-битов в host-тесте (а): тест идёт на фикстуре, где A-биты стоят на всех уровнях таблиц; частичный обход с установкой A покрыт только `od-machine`-тестом `locating_an_atomic_operand_leaves_what_the_interpreters_unequal_cmpxchg_left` (`paged_memory_adapter.rs`), не host-тестом.
- Ордеринг в тесте (б) подтверждён поведением на M3 Max (3000 раундов), на уровне инструкций — порядком слов блока; контрольной мутации «поколение раньше данных» на уровне теста не сделано. Одна контрольная мутация проведена: шаг счётчика поколения 1 заменён на 2 в `atomic_block.rs` (исходный код восстановлен) — `s6_41c_an_unequal_cmpxchg_takes_the_write_intent_like_the_interpreter` падает.
- Измерения только на Apple M3 Max.

### S6-41d: допуск LOCK/XCHG/MFENCE в блок JIT

Дата: 2026-11-18. Статус: **принято** после независимого ревью `gpt-6.1-sol/high` для атомарных блоков и fence. Закрывает пункт (в) S6-41b и открытый пункт «поле режима в ключе допуска `PagedFetchedLoadSession`» из S6-42 (граница (з) S6-41b): блок из нескольких атомарных инструкций, допуск такого блока в сессию с ключом по режиму, вызов политики LSE при создании блока, `CodeInvalidated` атомарного пути и дифференциал через блоки. **Не сделаны (ниже): обычные load/store и целочисленные эффекты внутри блока с атомарными, привязка `execute_atomic` к циклу S3-38o и к интерпретаторному fallback, A7 под `Mutex<MachineBus>` (S6-42b), litmus с JIT (S6-41e).** Код: `crates/od-jit/src/atomic_block.rs`, `crates/od-jit/src/atomic_adapter.rs`, `crates/od-jit/src/runtime.rs` (`RuntimeContextV4`), `crates/od-jit/src/ir.rs` (только тест и комментарий), `crates/od-host/src/atomic_inline.rs`, `crates/od-host/src/lib.rs` (`PagedFetchedLoadSession::execute_atomic`), `crates/od-machine/src/bus.rs` (`MachineBus::atomic_inline_memory`); тесты — там же и `crates/od-host/tests/s6_41d_atomic_blocks.rs` (новый). Отчёт — [STAGE_6_S6-41d_2026-11-18.md](../reports/STAGE_6_S6-41d_2026-11-18.md).

**Что сделано по пунктам плана.**
1. **Блок из нескольких атомарных инструкций.** `compile_atomic_block(&Program, MemoryOrderMode)` принимает от 1 до `MAX_ATOMIC_BLOCK_INSTRUCTIONS` (16, граница профиля, не замер) подряд идущих гостевых инструкций; каждая — атомарная форма или fence, не больше одного `AtomicRmw` на инструкцию (`MoreThanOneMemoryEffect`). Инструкция без `AtomicRmw` и `Fence` отклоняется с `MissingAtomicEffect` и её RIP. Программа делится на программы по инструкциям, каждая идёт через прежний `emit_atomic_program` (свой план регистров, `x2..x16` временные). **Каждая инструкция закрывается сама:** перед ней сторожа (оставшийся бюджет, STOP-байт), helper, последовательность, повышение поколения, затем RIP, `remaining_budget -= 1`, `retired_instructions += 1`. Fault, отказ helper, бюджет и STOP посреди блока оставляют инструкции до него завершёнными (RIP, счётчики, данные, поколения) и ничего от неё самой (точка PC-map `BeforeEffects` у каждой инструкции: `AtomicInstructionLayout::{guard_start, helper_call_word, effects_start}`; поля `helper_call_word`/`effects_start` в `AtomicBlockLayout` заменены списком `instructions`). `RuntimeContextV4`: `site` — база массива `site_count` описаний (по одному на инструкцию с памятью), блок пишет `site_index` перед каждым вызовом helper, helper отказывает по индексу за границей (`HELPER_FAILURE`); `AtomicBlock::sites` вместо `site`. Адаптер: `lower_atomic_sequence(&[&Instruction], first_rip)` — одна проверенная многоинструкционная `Program` (сквозные временные, пара `Begin`/`Commit` на инструкцию); `lower_atomic_instruction` — её частный случай. Хост: `AtomicInlineBlock::{publish_in (в слот части S6-42), run_guarded}`; отчёт `AtomicInlineReport::committed_writes` (по порядку программы), `committed` — последний. **Отклонение (записано):** отчёты `committed_write` операндов блок отдаёт после возврата, то есть после записей таблиц страниц обходов **следующих** инструкций; интерпретатор чередует «обход, операнд, обход, операнд». Множество записей то же (тест сравнивает как множество, когда операнды на двух страницах, и точно, когда на одной); порядок «данные, `Release` поколения, отчёт» (S6-30b) соблюдён для каждой инструкции. **Аллокатор и блок-эмиттеры общего пути (`allocator.rs`, `block_emitter.rs`, `emitter.rs`) сохраняют явный отказ `UnsupportedEffect`/`InvalidOperation` по `AtomicRmw`/`AtomicFlags`/`WriteGprIfNotEqual`/`Fence` (решение):** блок с атомарными эффектами исполняет `atomic_block` со своим планом регистров и ABI V4, а у общего пути другой ABI доступа (V2/V3, helper обычных load/store); принимать эффекты там значило бы выдавать код, которого этот путь не исполняет. Обычные `CheckedLoad`/`CheckedStore` и целочисленные эффекты в блоке `atomic_block` по-прежнему отказ (`SequenceError::UnsupportedEffect`); обычная инструкция рядом с атомарной остаётся интерпретатору, блок кончается перед ней. **Явные отказы:** ширина 16 (`AtomicBlockError::UnsupportedWidth16`; декодер её не выдаёт), `LOCK BTS/BTR m, r` (`AtomicAdapterError::RegisterBitOffset`), FS/GS-формы (не атомарная форма адаптера: `NotAnAtomicForm`), регистр данных вне RAX..RBX (`UnsupportedRegister`), больше 16 инструкций (`InstructionCount`).
2. **Допуск в `PagedFetchedLoadSession`.** `execute_atomic(state, bus, cr3, efer, access_context, budget)` (и `pub(crate) execute_atomic_with_stop`): контекст как у `execute_forms`; трекаемая выборка окна до `PAGED_ATOMIC_FETCH_WINDOW` (240) байт в странице RIP (права исполнения/NX/U/S, бит Accessed кода); декодер `od_isa::long_decode` по окну, ведущий ряд инструкций, которые адаптер переводит; первая иная (обычный load/store, ветвление, FS/GS) заканчивает блок или, если она первая, даёт отказ `PagedFetchedLoadErrorKind::AtomicForm(..)` без исполнения; блок публикуется (`publish_in` — в слот части сессии, если есть); исполнение — `run_guarded` над памятью шины (`MachineBus::atomic_inline_memory`, общая ссылка на ту же `PhysicalMemory`, что берут блокировочные операции адаптеров). Новые виды отказа: `AtomicForm`, `AtomicModeUnavailable(ExecutionMode)`, `AtomicCompile(AtomicBlockError)`, `AtomicRun(AtomicInlineError)`. Сессия держит один опубликованный блок: допуск атомарного блока сбрасывает блок форм S4-15 и наоборот; `reset()` освобождает любой. Отчёт `PagedFetchedAtomicReport` (код блока, число инструкций, режим, `AtomicInlineReport`, все committed writes вызова). **Отказ helper (`Declined`: невыровненный или split-операнд, не RAM) исполняет вызывающий интерпретатором**; у сессии нет fallback для этих форм (`execute_or_interpret` идёт через `step_paged_mov`, которому `LOCK` неизвестен), цикл S3-38o `execute_atomic` не использует.
3. **Ключ режима (з).** Ключ допуска атомарного блока: линейный адрес, физическая страница кода, поколение страницы, байты инструкций блока и **режим памяти шины** (`MachineBus::execution_mode` → `MemoryOrderMode`). Смена режима сбрасывает допуск (`atomic_mode_drops`) и следующий вызов допускает блок заново, уже для нового режима (`PagedFetchAdmission::Admitted`); код `Interleaved` не исполняется в `Parallel` и наоборот. Для блоков форм S4-15 (`execute_forms`) режим не нужен: их код от режима не зависит.
4. **Политика LSE.** `compile_atomic_block_with(program, mode, &dyn Fn(MemoryOrderMode) -> bool)` спрашивает политику один раз до генерации кода; `compile_atomic_block` передаёт `mode_admitted_on_host`. Сессия спрашивает политику до выборки (нет обхода страниц, `fetch_committed_writes` пуст, состояние и таблицы не тронуты) и ещё раз при компиляции; отказ — `AtomicModeUnavailable(mode)`. Подмена предиката для теста: `PagedFetchedLoadSession::set_atomic_mode_policy` (`#[doc(hidden)]`). На этом хосте (Apple M3 Max) политика допускает оба режима.
5. **Порядок `verify`.** Правило `AtomicRmwOrder` и так действует по гостевой инструкции (`rposition` на `BeginInstruction`, `take_while` до `CommitInstruction`); тест фиксирует: между инструкциями блока за атомарным эффектом может идти что угодно, внутри одной инструкции — ничего fallible (ошибка указывает на операцию той инструкции, а не первой).
6. **`CodeInvalidated`.** Helper отмечает (`RuntimeContextV4::code_dirty`), что запись обхода или операнда пересекает охраняемый физический диапазон; блок проверяет флаг после того, как инструкция завершилась (RIP, счётчики), и выходит `RUNTIME_EXIT_V4_CODE_INVALIDATED` (14): записавшая инструкция и все до неё завершены, следующие не исполняются. Диапазон — **4 КиБ страница кода блока** (правило store S4-16i: запись в страницу, а не в байты блока). Сессия помечает блок `retired` (никогда не используется повторно, какой бы ни был ключ), следующий вызов читает новые байты.
7. **Дифференциал через блоки** (тесты ниже).

**Тесты (по именам).**
- `od-jit --lib` (6 новых): `atomic_block::tests::{s6_41d_a_block_of_three_instructions_has_one_site_and_one_layout_entry_each, s6_41d_the_profile_refuses_ordinary_memory_effects_and_oversized_blocks, s6_41d_the_host_policy_is_asked_when_the_block_is_created, s6_41d_a_sixteen_bit_atomic_operand_is_refused_explicitly}`, `ir::tests::s6_41d_the_atomic_order_rules_hold_per_instruction_in_a_multi_instruction_block`, `atomic_adapter::tests::s6_41d_a_run_of_atomic_forms_lowers_to_one_verified_multi_instruction_program`; обновлены `the_exits_of_the_helper_lie_before_every_effect_of_the_instruction` и `a_fence_block_has_no_helper_and_no_generation_raise` (список `instructions`).
- `od-host --test s6_41d_atomic_blocks` (8): (7) `s6_41d_blocks_of_two_and_three_atomic_instructions_equal_the_interpreter` — 70 последовательностей из 2–3 инструкций (LOCK-формы, `XCHG`, fence; 18 различных форм; 29 последовательностей с двумя инструкциями на одном операнде; одна страница и две страницы) × 2 режима × 10 раундов = 1400 сравнений блока с интерпретатором `od_isa` на двойнике памяти: GPR, RIP, RFLAGS, обе страницы данных (операнд и охранные байты), записи таблиц, поколения трёх страниц, committed writes; (7) `s6_41d_a_fault_between_the_instructions_of_a_block_leaves_the_earlier_ones_retired` — страница только для чтения (P|W) и страница без present на инструкции с номером 0, 1 или 2, 768 случаев (464 — с завершёнными инструкциями до fault): состояние, RIP на faulting инструкции, `retired`, `remaining_budget`, одно повышение поколения на завершённую инструкцию с памятью, таблицы, committed writes (как множество), запись `PageFaultRecord` (CR2, код, сохранённый RIP) равны интерпретатору (интерпретатор шагается по одной инструкции, как плата: у адаптера на запуск свой `instruction_rip`); `s6_41d_the_guards_between_instructions_leave_the_earlier_ones_retired` — бюджет 0…3 и поднятый STOP-байт; (3, 8) `s6_41d_code_built_for_interleaved_does_not_run_in_parallel` — расписание Interleaved, Interleaved (Reused), Parallel (Admitted), Parallel (Reused), Interleaved (Admitted): слова допущенного блока равны словам блока того режима, `atomic_mode_drops == 2`, 3 допуска, результат равен интерпретатору в обоих режимах; (4, 8) `s6_41d_a_host_without_lse_refuses_the_atomic_block_before_any_walk` — подменённая политика (только `Parallel`; все режимы), отказ без обхода; (6, 8) `s6_41d_a_write_into_the_active_code_page_ends_the_block_with_code_invalidated` — блок из `lock add [стр. 1]`, `xchg [в код]`, `lock inc`: выход `CodeInvalidated`, `retired == 2`, третья не исполнена, состояние и память равны интерпретатору после двух инструкций, следующий вызов видит NOP на месте третьей (отказ `AtomicForm`, блок сброшен), запись в другую страницу не инвалидирует (повторное использование); (2, 8) `s6_41d_atomic_blocks_run_next_to_ordinary_instructions_through_the_session` — `lock add; xchg; mov rax,[rbx]; lock inc; mfence`: блок из двух, шаг интерпретатора для `mov`, блок из двух; итог равен чистому интерпретатору в обоих режимах; обычный load первым — отказ `AtomicForm(NotAnAtomicForm)`; `s6_41d_unsupported_forms_and_the_block_bounds_are_explicit` — `LOCK BTS m64, rcx`, `LOCK XADD m64, rsi`, FS-форма: отказ без исполнения; ряд длиннее границы блока режется на 16 + остаток.
- Контрольные мутации (проведены, код восстановлен): снятие проверки `code_dirty` после инструкции — падает тест (6); снятие режима из ключа допуска и сброса при смене — падает тест режима.
- Повтор S6-41c: `s6_41c_atomic_inline` 9 тестов, `s6_41_atomic_lowering` 6 тестов зелёные; регрессия `od-jit --lib` (atomic arm64 encoder memory_order ir allocator emitter translation) 253 теста; полный `cargo test -p od-jit --no-fail-fast` 498 passed, 2 ignored.

**Ограничения (записаны).**
- **Обычные load/store и целочисленные эффекты внутри блока с атомарными не допущены.** V4-helper обходит страницы только с намерением записи (`AtomicTarget`); у обычного чтения и записи нужен свой вид `Located` в `page_walk.rs`/`bus.rs`/`paged_memory_adapter.rs` и вопрос порядка (`LDAR`/`STLR`) — отдельный срез. «Обычные инструкции рядом» значит: блок кончается перед ними, исполняет интерпретатор (тест `..._next_to_ordinary_instructions_through_the_session`).
- FS/GS-формы, ширина 16 и `LOCK BTS/BTR m, r` — явный отказ (выше). Ширина 16 не сравнивалась ни разу.
- `execute_atomic` не подключён к циклу S3-38o (`paged_host_loop`) и к `execute_or_interpret`; отказ helper исполняет вызывающий. Бюджет — параметр вызова.
- Окно выборки (240 байт, страница RIP) и граница 16 инструкций — границы профиля, не замеры (S6-38). Ряд, пересекающий границу страницы, кончается на последней инструкции, целиком лежащей в окне; первая инструкция, не умещающаяся в страницу, — отказ.
- Допуск привязан к режиму шины (`execution_mode`), а не к числу потоков: привязка к `ParallelControl` и потокам S6-34 — S6-42b.
- **A7 S6-40 не выполнен:** шина под `Mutex`, `execute_atomic` берёт `&mut MachineBus`; параллельное исполнение двух native-вызовов над общей `&MachineBus` — S6-42b. Litmus S6-19/S6-36 с JIT — S6-41e.
- Задержка отчёта `committed_write` (после возврата блока, не после каждой инструкции) записана выше как отклонение; потребителю, которому нужен порядок записей по инструкциям, он не годится; многоинструкционные блоки с обычным store в параллельный режим по-прежнему не допускаются (S3-24), в этом блоке обычного store нет.
- Обязательство (б) S6-41b («`CASAL` при неравенстве и барьер»): обычных доступов в блоке по-прежнему нет, условие остаётся в силе.
- **Отказ `AtomicForm` сбрасывает блок форм S4-15 (ограничение, сессия держит один блок).** При неудачном повторном использовании (ключ не совпал) `admitted` (блок форм S4-15) и `admitted_atomic` сбрасываются до декодирования, поэтому отказ `PagedFetchedLoadErrorKind::AtomicForm` уничтожает и блок форм; допустимый к повторному использованию атомарный блок отказ не затрагивает. Перенос сброса после успешного `lower_atomic_sequence` не делался.
- **STOP и #PF внутри блока.** При записи Dirty первой инструкции тест поднимает STOP-байт. Следующий сторож возвращает управление с одним завершённым шагом. Тест ошибки доступа для read-only PTE с изначально сброшенным Accessed сверяет установку бита и её уведомление с интерпретатором.
- **Расширение API `od-machine`:** публичный `MachineBus::atomic_inline_memory() -> &PhysicalMemory` дублирует `pub(crate)` `physical_memory()` и отдаёт ссылку на память наружу крейта (только чтение ссылки; смену карты памяти и режима она не даёт, `set_execution_mode` требует `&mut`). Обоснование: нужна сессии `od-host` (helper работает на общей памяти) и host-тестам; записано как расширение API (также в `MACHINE_CONTRACT.md`). Сужение до `pub(crate)`/feature не делалось.
- Измерений нет (S6-38), проверки только на Apple M3 Max.

### S6-42: межпроцессорная инвалидация и безопасное освобождение блоков

Дата: 2026-11-18. Статус: реализовано частично (чекбокс S6-42 в `STAGE_6.md` не отмечен, gate-ревью отдельно). Профиль приёмки — production-профиль «одна инструкция на блок» (формы `PagedFetchedLoadSession`, решение 2026-11-18); граница «текущий блок дорабатывает ≤31 инструкции» на нём не проверяется, многоинструкционные блоки со store в параллельный режим не допускаются до S3-24. Код: `crates/od-host/src/jit_region.rs` (новый), `crates/od-host/src/lib.rs` (`CodeRegion`, `publish_*_in`, `PagedFetchedLoadSession::with_region_part`), `crates/od-host/src/paged_host_loop.rs` (`PagedVcpuGroup`, вызов safe point сессии); тесты — `crates/od-host/src/paged_host_loop/tests/s6_42.rs`. Решение о форме владения region — [ADR-0010](../decisions/ADR-0010-process-map-jit-region-partitioned-between-vcpu-threads.md) (предложение). Отчёт — [STAGE_6_S6-42_2026-11-18.md](../reports/STAGE_6_S6-42_2026-11-18.md).

**Что реализовано.**
- `SharedJitRegion` (`Sync`) — единственный владелец process-wide `MAP_JIT` region и `MapJitLease` (M1). Region делится на `parts` непересекающихся частей по `slots_per_part` слотов `ARENA_SLOT_BYTES`, каждая часть начинается на границе системной страницы. Часть выдаётся один раз (`take_part`) как `JitRegionPart` (`Send`, без исполняемых ручек), в потоке vCPU становится `ActiveJitPart` (`Rc`, `!Send`): таблица слотов, список свободных и поколения слотов живут только в этом потоке (A1, M2). Перераспределения частей нет. Mapping и аренда живут, пока жив владелец или любая часть (`Arc<RegionShared>`), а не «аренда снимается после `join`»: это уточнение M1 (безопаснее, use-after-unmap исключён). Уточнение ADR-0005: «один держатель исполняемой памяти» из ADR-0005 здесь — один держатель аренды (`SharedJitRegion`); части — арены внутри него, привязанные к потокам.
- **Отклонение от M2 (не реализовано).** Контракт S6-40 предусматривал «если части не хватает, владелец вытесняет свои записи (FIFO)». Вытеснения нет: когда часть полна, публикация отказывает (`jit_region.rs`, `allocate_and_write`) и сессия уходит в интерпретатор. Для сессии с одним блоком состояние недостижимо; для нескольких блоков на часть — открыто.
- Допуск сессии на слот: `PagedFetchedLoadSession::with_region_part(part)`; блок каждого допуска живёт в слоте своей части (`CodeRegion::Slot`), отображения на допуск нет, аренда за допуск не берётся. Сессия без части получает `MappingAlreadyLive`, пока region жива (M6), и по правилам S3-38o уходит в интерпретатор.
- Запись слота (M3, M4): окно записи `pthread_jit_write_protect_np` — потока-владельца; затем `sys_icache_invalidate` диапазона и явный `ISB` в том же потоке. M4 относится к слотам: явный `ISB` есть только у слотового пути (`publish_*` с частью); ветка `Owned` в `publish_with_metadata_in` (`lib.rs`) идёт без `ISB`, как и раньше. Поток пишет и исполняет сам, M5 не применяется; наличие `ISB` внутри `sys_icache_invalidate` не сверялось, явный барьер делает вопрос несущественным).
- Освобождение (A4): слот возвращается, когда владелец роняет `PublishedCode`; тип `!Send`, поэтому другой поток освободить блок не может. Для освобождения, отложенного из-за заимствования таблицы, реализован механизм: оно применяется на safe point сессии (`PagedFetchedLoadSession::safe_point`, вызывается циклом S3-38o в начале каждой итерации). Путь отложенного освобождения (`jit_region.rs`, `try_borrow_mut` с ошибкой) защитный: в текущем коде недостижим и тестами не покрыт, проверки `pending_release == 0` в тестах тривиальны; «применяется на safe point» — реализованный механизм, не проверенное поведение. Поколение слота растёт при каждом занятии. В тестовой сборке (только `cfg(test)`) освобождённый слот заполняется `BRK #0`, но лишь первые 64 байта слота из 4096: исполнение освобождённого блока было бы ловушкой, если бы управление вошло в эти 64 байта; это ограничение доказательной силы.
- Инвалидация между vCPU (A3, D1): только через общие поколения `commit_write`; на каждом входе (каждый шаг профиля «одна инструкция на блок») сессия заново снимает tracked snapshot и сверяет байты, страницу и поколение с ключом допуска блока (внутри сессии). Задержка: первый вход после записи видит новое поколение при синхронизации через замок (шина под `Mutex`) или канал; без замка не проверено (S6-42b). **A7 не выполнен (шина под `Mutex`)**: шаг JIT в межпроцессорных тестах идёт под `Mutex<MachineBus>`, native-вызов и операции кэша не вне замка шины; задержка обнаружения без замка не проверялась — S6-42b. Поэтому сценарий A/B, свободный прогон и `stop_all` проверяют режим чередования с гранулярностью замка, а D1 и G-XMC в параллельном режиме не доказаны.
- `PagedVcpuGroup` (`paged_host_loop.rs`) — `stop_all`/`resume` над набором `PagedHostLoopControl` по S6-40, раздел 5 (флаг, затем STOP-байт; поток подтверждает `safe` только после возврата цикла, опускает `safe` до перечитывания флагов — порядок Дейкстры S6-34).

**Правила S6-40 и чем выполнены.**

| Правило | Тест |
|---|---|
| A1 (кэш, блок, ручка, PC-map не пересекают потоки; нет `unsafe impl Send/Sync` для них) | `s6_42_region_parts_are_disjoint_handed_out_once_and_hold_the_process_lease` (проверки `!Send` на этапе компиляции для `PublishedCode`, `ActiveJitPart`, `PagedFetchedLoadSession`, `PublishedArenaBackend`; `Send` для `JitRegionPart`, `Send + Sync` для `SharedJitRegion`) |
| A3, D1 (инвалидация только по поколениям; проверка на каждом входе; **частично**: A7 не выполнен, шина под `Mutex`, режим чередования с гранулярностью замка) | `s6_42_vcpu_a_changes_code_vcpu_b_executes_it_and_leaves_the_old_code` (A и B — потоки со своими сессиями и частями; после записи A первый шаг B идёт по новым байтам, итог равен прогону интерпретатора по тому же расписанию, список committed writes совпадает); `s6_42_free_running_vcpus_never_execute_stale_code` (на каждом шаге B: эффект шага равен эффекту байт, которые хранит страница в этот момент; ключ допуска блока (`admitted.generation`, `admitted.bytes`) напрямую не сверяется — `report.fetched_code`/`fetched_generation` берутся из свежего снимка под тем же замком, поэтому сверка байт и поколения почти тавтологична; доказательная сила — эффекты (RAX, qword данных для load и store при пути только JIT); поколение не старше уже прочитанного). Закэшированная отбраковка блока по поколению сценарием A/B не проверяется: каждый отказ (`push`, `ret`) обнуляет `admitted`, и каждый native-шаг B компилирует блок заново (admissions 2 и 4); отбраковку по ключу проверяет только свободный прогон, случай «поколение выросло, байты те же» отдельно не проверен |
| A4 (освобождает только владелец, на своём safe point) | `s6_42_slot_is_released_by_the_owner_and_reused_with_a_new_generation`, `s6_42_release_under_load_never_runs_a_released_slot` (353 блока опубликованы и освобождены одним vCPU, второй меняет код и крутит 1600 сырых блоков в своей части — числа примера одного недетерминированного прогона; `allocations == releases + live`, `pending_release == 0`, поколение слота = числу занятий) |
| M1, M2, M6 | `s6_42_region_parts_are_disjoint_handed_out_once_and_hold_the_process_lease`, `s6_42_session_publishes_into_a_slot_and_a_plain_session_falls_back_to_the_interpreter` |
| M3, M4 | `s6_42_threads_write_and_run_their_own_parts_concurrently` (4 потока по 4000 публикаций и вызовов в своих частях одновременно, плюс по одному резидентному блоку; ни один поток не споткнулся) |
| S1, S3 (подтверждение `safe` только после возврата, без потерянных подтверждений) | `s6_42_stop_all_with_jit_vcpus_acknowledges_on_safe_points_and_resumes` (3 vCPU с JIT-циклом, 6 остановок: во время остановки `retired` не растёт, состояние каждого vCPU равно состоянию интерпретатора при том же числе инструкций, после `resume` все идут дальше, на каждый `stop_all` ровно одно подтверждение от каждого), `s6_42_stop_all_names_the_vcpu_that_does_not_acknowledge` |
| Повтор S3-38o на слотовом backend | 11 тестов `s6_42_slot_backend_*` (те же тела, что `s3_38o_*`, сессии публикуют в слоты одной общей region) |

Доля исполнения интерпретатором записана в результат (stderr тестов): сценарий записи кода — 8 из 12 шагов B (67%, шаги `push`/`ret` цикла; JIT исполнял изменяемую инструкцию до и после записи, по два шага); свободный прогон — 0% (путь только JIT, отказ — паника; числа 290/273 — пример одного недетерминированного прогона), остановка — 50% (`push`/`ret`). Прогон, в котором JIT не исполнял изменяемый код, тесты не засчитывают (`steps_jit` ровно 2 до и после записи; `native_old > 0 && native_new > 0`).

**Обновление S6-42b (2026-11-18):** пункты «нет привязки к `ParallelControl`» и «шина под `Mutex`» ниже закрыты частично, см. [S6-42b](#s6-42b-привязка-к-parallelcontrol-и-a7-над-machinebus-без-замка); текст ниже — состояние на конец S6-42.

**Ограничения (не закрыто).**
- **Нет привязки к `ParallelControl` и потокам S6-34.** `parallel_run` — `pub(crate)` в `od-machine` и исполняет кванты интерпретатора; `od-host` до него не дотягивается. Цикл S3-38o и сессия с частью запускаются в потоках теста; `stop_all` — свой `PagedVcpuGroup` с тем же протоколом, а не `ParallelControl::stop_all`. Привязка — S6-42b.
- **Шина под `Mutex`.** Сессия JIT и общий интерпретатор берут `&mut MachineBus`; варианта над `&MachineBus` (как у интерпретатора S6-34) для paged-пути JIT нет. Потоки vCPU делят одну `MachineBus` (одну `PhysicalMemory` с общими поколениями) под замком, шаги разных vCPU чередуются, а параллельны только записи слотов, `ISB` и native-вызовы разных частей (тест M3). Одновременного исполнения гостевой RAM из двух native-вызовов этот срез не доказывает. **A7 не выполнен (шина под `Mutex`); задержка обнаружения без замка не проверялась — S6-42b** (`s6_42.rs`: шаг JIT под `Mutex<MachineBus>`).
- **Открытый пункт S6-42b (граница (з) S6-41b):** код `Interleaved` не исполняется в `Parallel` — поле режима в ключе допуска `PagedFetchedLoadSession` и сброс допуска при смене режима. Пока в блоках нет атомарных форм, код от режима не зависит и это не требуется; становится обязательным с допуском атомарных форм (S6-41d), тест «код Interleaved не исполняется в Parallel» — там же.
- Фикстурные бэкенды (`PublishedArenaBackend`, `MultiEntryBlockCache`) на слоты не переведены (A5): пока region жива, они получают `MappingAlreadyLive`.
- Закрыто на gate S3-38o (2026-11-18): выход `StopRequested` с `retired == 1` для store в собственную страницу кода переводится в `CodeInvalidated`, как `Complete` (`store_invalidates_active_code`); атомарный путь (`AtomicInlineExit::CodeInvalidated`) этим не затронут.
- Размеры частей (параметры `SharedJitRegion::new`) — параметр теста, не замер; замер — S6-38.
- Пометки «по памяти» контракта S6-40 (Intel SDM 9.1.3 и 9.3, Arm ARM о сопровождении кеша инструкций и single-copy atomicity, правило Apple о `pthread_jit_write_protect_np`, `ISB` в `sys_icache_invalidate`) в этом срезе не сверялись по нормативным текстам.
- Многоинструкционные блоки со store (S3-24), связи блоков (S3-31, D2/D3), inline-формы `LOCK`/`XCHG`/`MFENCE` (S6-41b), восстановление после host fault (S3-27) не затронуты.

### S6-42b: привязка к ParallelControl и A7 над `&MachineBus` без замка

Дата: 2026-11-18. S6-42b принят независимым ревью `gpt-6.1-sol/high` для привязки к `ParallelControl`, A7 и порядка D1 на одноинструкционном профиле. Замеры задержки и итоговая приёмка S6-42 остаются открытыми. [Отчёт](../reports/STAGE_6_S6-42b_2026-11-18.md).

**Что сделано по пунктам задачи.**
1. **Привязка к потокам S6-34 через `ParallelControl`.** `parallel_run` остался `pub(crate)`; узкий публичный фасад — `od_machine::host_parallel` (`run_host_threads`, `HostParallelControl`, `HostQuantumCall`/`HostQuantumFn`/`HostThreadExitFn`, `host_parallel_board`, `host_install_vcpu`): те же потоки (по одному на vCPU), барьер, kick, `stop_all`/`resume` и начисление часов владельцем часов, но квант потока — функция вызывающего. **Шлюз split `LOCK` квант хоста НЕ получает:** `HostQuantumCall` не несёт gate, `split_for_set_step_shared` отдаёт `gate: None`, `AdapterBus::Shared::split_gate()` — `None`; гостевой split `LOCK` на этом пути — отказ `MisalignedAtomic` и ошибка run, а не проход через шлюз (опрос чужого `other_split` в `stop` работает; запись в «Открыто», S6-42c). Квант JIT-цикла — `od_host::host_threads::JitHostQuantum` (macOS/aarch64): сессия и часть region создаются в потоке vCPU и **роняются в нём же явным хуком выхода потока** (`run_host_threads(.., on_thread_exit, ..)`: хук вызывается в замыкании потока после последнего кванта и до учёта `threads_exited`; `JitHostQuantum::thread_exit` берёт слот `thread_local!`, так что аренда слота и отображение `MAP_JIT` освобождены до возврата `run_host_threads`; деструктор `thread_local!` для этого не годится: `thread::scope` ждёт конца замыкания, а не деструкторов TLS, и аренда могла бы пережить run — `MappingAlreadyLive` у следующей region) (A1, A4), каждый вызов — `run_paged_host_loop_shared`. `stop_all` run доходит до цикла опросом `stop` в начале каждой итерации (флаги, затем опрос; STOP возвращается как `StopRequested` без шага после него), поток подтверждает `safe` на барьере после возврата кванта, не внутри native-вызова. `PagedVcpuGroup` остаётся протоколом для хостов со своими потоками.
2. **A7 для paged-пути.** Методы шины, которым `&mut` был не нужен (`access_tracked_ram`, `access_tracked_2m_ram`, `tracked_code_snapshot*`, `capture_code_page`, `read_tracked_bounded_ram_read*`, `read_virtual_ram_diagnostic`), стали `&self` (обходы страниц и так принимали `&PhysicalMemory`). Адаптеры JIT (`with_paged_mov_memory`, `with_paged_mov_dword_memory`, `with_paged_mov_store_memory`) и вся `PagedFetchedLoadSession` (`execute_forms*`, `execute_atomic*`, `execute_paged_*`) принимают `&MachineBus`; для адаптера добавлен вид `AdapterBus::Shared`. Интерпретатор fallback — `od_machine::run_paged_probe_shared` (тот же цикл и профиль, что `run_paged_probe`; порты через `split_for_set_step_shared`: замок устройств берётся на одно обращение). Замка шины в цикле нет. Замок источника платы у загрузочного процессора раньше держался через весь вызов кванта (`Worker::call`); для кванта, который источник не использует, он берётся только на расчёт бюджета (`run_threads_with(.., source_free = true, ..)`); замок владельца часов и APIC на вызов не держатся. Комментарий SAFETY store-пути (`execute_paged_mov_rbx_rax_store_v2_with_stop`) переписан: довод «эксклюзивная `&mut`» заменён доводом «единственный эффект на RAM блока — вызов helper в адаптер, чья запись — `PhysicalMemory::store_bytes` (`&self`, один host-атомарный доступ на выровненный операнд, `SeqCst` в параллельном режиме, поколение `Release` после данных)».
3. **Задержка обнаружения кода без замка (D1).** Тест `s6_42b_b_runs_the_new_code_on_the_first_step_after_the_acquire_read_without_a_bus_lock`: потоки A и B (свои сессии и части), общая `&MachineBus` в `ExecutionMode::Parallel`, **замка шины нет**. A 31 раз переписывает инструкцию по `CODE_RIP` B нативным store на 8 байт (`mov rax,[rbx+disp8]` с 32 различными `disp8`; ячейка `k` содержит `0xC0DE_0000 + k`, поэтому RAX называет исполненную инструкцию) и после каждой записи публикует счётчик `Release`. B на каждом шаге читает счётчик `Acquire` (`seen`) и исполняет один шаг по пути `PagedStepPath::Jit` (шаг без JIT — падение): первый шаг после чтения должен исполнить инструкцию с номером не меньше `seen`, и номер не убывает. Два режима: свободный (A уходит вперёд, B видит 3–4 из 31) и в лок-степе (A ждёт два шага B между записями; B видит 31 из 31, 64 native-шага, 32 допуска, интерпретатор 0%). **Мутация (проведена, код восстановлен):** снятие сверки поколения и байт в `reusable` (`execute_forms_with_stop`) — тест падает («D1: after the Acquire read of 4 published writes the step ran the instruction of write 0»). Ограничения доказательной силы: проверяется порядок «публикация → `Acquire` → шаг», а не время; D1 доказан для профиля «одна инструкция на блок» и для одной ARM64-машины; G-XMC (litmus с JIT) — S6-41e, S6-43a.
4. **Фикстурные бэкенды** (`PublishedArenaBackend`, `MultiEntryBlockCache`) на слоты **не переведены** (A5): по-прежнему только однопоточные, пока region жива, получают `MappingAlreadyLive`; запись оставлена, перевод недёшев и в параллельном режиме не нужен (S6-42c, если понадобится).

**Тесты (по именам).**
- `od-host --lib`, модуль `paged_host_loop::tests::s6_42b` (5, пятый — `s6_42b_the_thread_exit_releases_the_region_before_the_run_returns`: 24 раунда «region, два потока, run, drop(quantum), drop(region), новая region сразу» без `MappingAlreadyLive`; хук видит сессию в слоте потока B до вызова `thread_exit` и пустой слот после, в каждом раунде): `s6_42b_the_shared_loop_equals_the_interpreter_on_the_mixed_program` (бюджеты 0…13 и 103: регистры, RAM, список записей и счётчики равны чистому интерпретатору; на 103 — 52 native и 51 интерпретатор); `s6_42b_the_external_stop_poll_ends_the_call_before_the_next_step` (7 шагов, затем `StopRequested`); D1 (выше); `s6_42b_parallel_control_stops_jit_vcpus_and_a_changes_code_b_executes` — два потока `run_host_threads` над платой с 2 vCPU: B (vCPU 0) крутит цикл «store, `push`, `ret`» нативно; первый `stop_all` (оба подтвердили, за 40 мс не растут `retired` и RAM, состояние B и RAM равны интерпретатору при том же числе инструкций — 3028 в примере прогона); во время остановки контроллер будит A (vCPU 1: `mov [rbx],rax; hlt`), после `resume` A нативным store пишет новый код (load вместо store) в страницу B и засыпает; второй `stop_all`, контроллер кладёт маркер в ячейку данных B, `resume`; третий `stop_all`: RAX B равен маркеру и ячейка не перезаписана (старый store затёр бы её) — B исполняет новый код нативно (число native-шагов B 1371 → 2744, интерпретатор 5486 = 66% — шаги `push`/`ret` цикла, native-шаг A 1), `stop_exits >= 3` (каждая остановка пришла опросом внутри вызова: квант 4096 длиннее ожидания остановки), потоки соединены. Мутация: отключение опроса `stop` — падает проверка `stop_exits`; сам `stop_all` без опроса проходит на границе кванта. Мутация reusable этим тестом **не** ловится (в цикле между native-шагами идут шаги интерпретатора, сессия сбрасывает допуск при каждом отказе и компилирует блок заново), её ловит D1.
- `od-machine --lib` (3): `host_parallel::tests::the_shared_probe_equals_the_exclusive_probe_with_port_io` (`in`/`out` на COM1: одинаковые отчёт, состояние и устройство), `host_parallel::tests::the_shared_probe_polls_stop_between_instructions`, `parallel_run::tests::a_source_free_run_holds_no_board_lock_across_the_quantum_call` (замок источника платы и замок владельца часов свободны внутри кванта при `source_free`; контрольный прогон без него показывает занятый замок источника).

**Открыто (S6-42c).**
- Шлюз split `LOCK` (`SplitLockGate`) на пути кванта хоста не подключён: гостевой split `LOCK` — `MisalignedAtomic`, ошибка run. Доставка NMI потоком (`deliver_pending_nmi_of` в `Worker::run`) идёт через `HOST_BOARD_ROOT = 0x1000` для vCPU без CR3 и в тестах S6-42b не проверялась.
- Квант JIT-цикла исполняет **базовый профиль** интерпретатора (`run_paged_probe_shared`: `MOV`, `PUSH`/`POP`, `IN`/`OUT`, `HLT`…): нет доставки прерываний и исключений гостю, нет профиля CPL0-set; `GuestFault` и отказ интерпретатора — ошибка run. Перенос цикла на `run_cpl0_set_loop` над `LoopBus::Shared` с JIT-попыткой — отдельный срез.
- STOP-байт, который читает native-код (`RuntimeContextV1::stop_requested`), этой привязкой не поднимается: нужен только блокам из нескольких инструкций (S3-24); у блока в одну инструкцию опрос между инструкциями цикла даёт ту же границу.
- `execute_atomic` принимает `&MachineBus`, но не подключён к циклу S3-38o и к фасаду кванта (как в S6-41d); litmus с JIT (S6-41e) и probe S6-43a — впереди; G-XMC в параллельном режиме не доказан.
- Задержка D1 не измерена во времени (только порядок); замеры — S6-37, S6-38.
- Фикстурные бэкенды предназначены для одного потока по A5. Требование о слотах относится к production host-циклу.
- Результат последующей сверки источников приведён в S6-40. Точная норма Arm для однобайтовой single-copy atomicity приведена в S4 раздела S6-40.
- Размеры частей region остаются параметром теста до замеров на профильной нагрузке (M2). Production-профиль с одной записью освобождает её перед новой публикацией.
- Новая публичная поверхность `od-machine` (`host_parallel`, `run_paged_probe_shared`) — расширение API под нужды `od-host`; запись в `MACHINE_CONTRACT.md` (S6-42b).
- Контракт `# Safety` у `execute_memory_store_v2` переписан после ревью (M2): разрешена общая RAM, каждый доступ к которой атомарен (`PhysicalMemory::store_bytes(&self)`); гарантия «полная 8-байтная запись или ничего» относится к записи этого vCPU. Код поведения не менялся.
- Контроль Linux после правок S6-42b (3+3 прогона до/после): побайтно совпали, код выхода 0, stderr пуст, JSON `b2898c991ad0f79b11a12c57f34eafaaa0e6b9264365db02136bd5354f02224f`, stdout `5104cf02696cc822bfa4b2b0e6be48c2321ef000e0258aba9c4e5f5ec0c80fd3` у всех шести; wall до 128/126/126 с, после 143/127/125 с (первый «после» шёл параллельно с clippy, время не показательно); логи `/private/tmp/s642b-ctl/`.

### S6-42c: прерывания, исключения и шлюз split `LOCK` в квантах хоста

Дата: 2026-11-18. Статус: **реализовано частично** (чекбоксы S6-42 и S6-42b/c в `STAGE_6.md` не отмечены, gate-ревью отдельно): сделаны A (прерывания и исключения гостю), B (шлюз split `LOCK`), C (корень NMI), D (тесты); открыто для S6-42d: источник платы, насос устройств, `execute_atomic` в цикле, перенос `MOV CR3` и прочее (ниже). Контракт S6-40 не менялся. Код: `crates/od-machine/src/{host_parallel.rs, parallel_run.rs, firmware_board.rs, paged_memory_adapter.rs, lib.rs}`, `crates/od-host/src/{paged_host_loop.rs, host_threads.rs}`; тесты — `crates/od-host/src/paged_host_loop/tests/s6_42c.rs` (новый), `crates/od-machine/src/host_parallel.rs` (`mod tests`). Отчёт — [STAGE_6_S6-42c_2026-11-18.md](../reports/STAGE_6_S6-42c_2026-11-18.md).

**Что сделано по пунктам задачи.**
1. **(A) Прерывания и исключения гостю.** Опт-ин режим кванта: `JitHostQuantum::with_cpl0_set()` (по умолчанию квант — базовый профиль S6-42b, побайтно прежний). В этом режиме цикл (`run_paged_host_loop_shared_set`, `PagedHostSetEnv`) перед каждым шагом (и перед native-попыткой) спрашивает границу прерывания своего vCPU: `od_machine::host_deliver_pending_irq` — тот же `LapicIrqSource` (старший вектор IRR выше PPR) и та же доставка `deliver_pending_irq` через IDT гостя, что у цикла CPL0-набора (`run_cpl0_set_loop`; IF = 0 и тень STI удерживают вектор, доставка ничего не ретайрит, вектор уходит из IRR в ISR только после кадра, EOI — запись гостя в LAPIC своего vCPU). Шаг интерпретатора в этом режиме — одна инструкция **профиля CPL0-набора** (`od_machine::run_host_set_step`: `run_cpl0_set_loop` над `LoopBus::Shared`, бюджет 1, без хука прерывания): `IRETQ`, `STI`/`CLI`, `LOCK`-формы, `INC`, переходы, MMIO LAPIC и остальное, чего нет в базовом профиле (`CLD`, `IN`/`OUT`, `HLT`, формы `MOV`). Гостевые `#PF` и `#GP(0)` доставляются через IDT гостя так же, как в цикле набора (кадр, CR2, обработчик, `IRETQ` перезапускает инструкцию). **Гостевая ошибка native-пути** (`GuestFault` блока с нулём завершённых инструкций) доставляется через IDT из исходной записи после подключения атомарных шагов S6-42d (`jit_faults_delivered`). Если доставка невозможна, исходный `GuestFault` завершает прогон с диагностикой в `JitHostQuantum::last_failure`. Недоставленный вектор — `IrqDeliveryFailed`, ошибка run (не тихий сброс). Запись в регистры таймера LAPIC (`0x320`, `0x380`, `0x3E0`) заканчивает вызов (`DeviceBoundary`, как у кванта интерпретатора: срок часов владельца мог устареть). Таймер LAPIC поднимает вектор в IRR по начислению часов владельцем (`account_retired`), доставка — на следующей границе (первый шаг следующего вызова, не позже кванта).
2. **(B) Шлюз split `LOCK`.** `HostQuantumCall` получил `gate: HostSplitGate` (непрозрачная обёртка над `SplitLockGate` потока; `HostSplitGate::none()` — «без шлюза») и `lapic`. `run_host_set_step` передаёт шлюз в `run_cpl0_set_loop`, который кладёт его в вид устройств шага (`with_split_gate`), так что гостевой split `LOCK` в кванте хоста идёт как у потоков S6-34: остановка остальных vCPU до первого видимого эффекта, ошибка `MisalignedAtomic` пропала. Путь `ProbeBus::Shared`/`AdapterBus::Shared` базового профиля остаётся с `gate: None` (базовый профиль без `LOCK`).
3. **(C) NMI.** `deliver_pending_nmi_of` берёт корень как `state.cr3`, затем явный корень, названный для vCPU, и больше ничего: без обоих — ошибка run `deliver_pending_nmi: the vCPU has no page-table root`, чтения по адресу 0x1000 (`HOST_BOARD_ROOT`) нет. `run_host_threads` принимает `nmi_roots: &[Option<u64>]`; `JitHostQuantum::nmi_roots()` даёт резервный `config.cr3` каждого vCPU. Пути интерпретатора (`deliver_pending_nmi` платы, `run_threads`) передают `Some(page_table_root)` платы и ведут себя как раньше.
4. **Опубликованная поверхность `od-machine`** (запись в `MACHINE_CONTRACT.md`, S6-42c): `HostQuantumCall::{lapic, gate}`, `HostSplitGate`, `HostCpl0Step`, `HostSetStepReport`, `HostIrqBoundary`, `host_deliver_pending_irq`, `run_host_set_step`, `HostParallelControl::lapic`, поля `nmi_delivered` и `split_locks` у `HostParallelReport`, аргумент `nmi_roots` у `run_host_threads` (изменение сигнатуры, вызовы в тестах обновлены).

**Не менялось (проверено тестами).** Пути чередования (`run_paged_probe`, `run_quantum`, `run_cpl0_set_loop` без хоста): изменения общего кода — видимость `deliver_pending_irq`, `IrqBoundaryOutcome`, `set_exit_to_probe_exit` (`pub(crate)`) и тип корня в `deliver_pending_nmi_of` (`Option<u64>`, платы передают `Some`, выражение `cr3.or(root)` равно прежнему `unwrap_or`); регрессии `stage6_smp_probe` и вся библиотека `od-machine` — в таблице отчёта.

**Тесты (по именам).** `od-host --lib`, модуль `paged_host_loop::tests::s6_42c` (6): `s6_42c_a_fixed_vector_is_delivered_to_a_guest_that_runs_native_steps` (12 векторов, вектор постится после того, как обработчик предыдущего завершён: счётчик обработчика ровно 12, `irqs_delivered` 12, IRR и ISR пусты, стек цикла уравновешен, IF восстановлен `IRETQ`, цикл остаётся native-интерпретаторным), `s6_42c_a_one_shot_timer_is_delivered_once` (гость программирует таймер LAPIC записями MMIO, три записи — три `DeviceBoundary`, одно срабатывание — один вызов обработчика, `next_timer_deadline` пуст), `s6_42c_a_guest_page_fault_is_delivered_through_the_guest_idt` (`#PF` native-инструкции доставлен, обработчик убирает код ошибки, чинит RBX, `IRETQ` перезапускает загрузку, она идёт native; `exceptions_delivered = 1`, `jit_faults_delivered = 1`, CR2 равен адресу; контроль — базовый квант с той же ошибкой: ошибка run), `s6_42c_a_guest_split_lock_passes_the_gate_of_the_host_quantum` (два vCPU по 400 `LOCK INC` через границу страницы: сумма точна три раза подряд, `split_locks` по 400 у каждого; контроль без шлюза — ошибка run с текстом «not naturally aligned», `MisalignedAtomic`), `s6_42c_an_nmi_is_delivered_through_the_root_named_for_the_vcpu` (vCPU 1 шлёт NMI через ICR, vCPU 0 принимает по `config.cr3`: один обработчик, `nmi_delivered = [1, 0]`, `nmi_blocked` снят `IRETQ`, `state.cr3` пуст), `s6_42c_an_nmi_without_a_root_fails_the_run_explicitly` (`nmi_roots = [None, None]`: `Error { cpu: 0 }` с текстом «no page-table root», обработчик не работал, потоки соединены). `od-machine --lib`: `host_parallel::tests::an_nmi_needs_a_root_named_for_the_vcpu` (без корня — явная ошибка; с названным корнем ошибка другого рода: корень используется).

**Чувствительность.** Мутация (код восстановлен): граница прерывания выключена (`host_deliver_pending_irq` не вызывается) — тесты вектора и таймера **падают** (ожидание доставки истекло, `irqs_delivered: 0`, 1,9 млн шагов цикла). Контроли (внутри тестов, код не менялся): без шлюза (`HostSplitGate::none()`) тест split `LOCK` даёт ожидаемую ошибку run; без корня NMI — ошибка run (тест). Каждая проверка — по тесту выше.

**STOP-байт S6-42d.** `Kicks` в параллельном режиме host-кванта публикует флаг, затем индивидуальный байт vCPU. `stop_all`, `finish` и шлюз split `LOCK` публикуют свой флаг запроса до kick. Цикл снимает байт перед опросом флагов, передаёт байт в нативный вызов и учитывает результаты до подтверждения остановки. Kick завершает квант для проверки INIT/SIPI и NMI на границе потока. Проверки с запросом после опроса приведены в [отчёте S6-42d](../reports/STAGE_6_S6-42c_2026-11-18.md#s6-42d--stop-байт-нативного-вызова).

- После доставки `LapicIrqSource` и источник IOAPIC подтверждают выбранный вектор. Квант BSP сохраняет выбранный источник до подтверждения. См. [ISSUE-513](../KNOWN_ISSUES.md#issue-513-s6-42d-bsp-ioapic-ack).
- `JitHostQuantum::nmi_paging` передаёт корень и EFER из одной конфигурации. Проверка прав чтения IDT использует тот же EFER, что квант. См. [контракт машины](MACHINE_CONTRACT.md#s6-42c--расширение-api-host_parallel-граница-прерывания-шаг-набора-cpl0-шлюз-и-корни-nmi).

**Атомарный шаг S6-42d.** Общий CPL0-цикл ограничивает V4-блок одной инструкцией и передаёт ему STOP-байт vCPU. Ключ допуска включает режим шины и предел числа инструкций. В Parallel требуется LSE. При отказе до входа или отказе помощника до выполнения операции используется шаг CPL0 со шлюзом split `LOCK`. Хостовая ошибка нативного вызова завершает квант.

Завершённая атомарная инструкция расходует единицу бюджета, очищает RF и применяет маску определённости флагов инструкции. Для native `#PF` и `#GP(0)`, в том числе от загрузки `MOV`, кадр строится из исходной ошибки и записи обхода страниц. RIP указывает на ошибочную инструкцию до записи кадра. Лимит составляет 256 последовательных native-доставок без завершённой инструкции. Наблюдатель получает диапазоны записей обхода страниц, операнда и кадра. См. [проверки](../reports/STAGE_6_S6-42c_2026-11-18.md#s6-42d--атомарные-инструкции-в-кванте-хоста).

**Смена CR3 S6-42d.** Опт-ин CPL0-квант заполняет отсутствующий `state.cr3` из конфигурации на первой границе инструкции. Для `MOV CR3, r64` используется профиль `MovCr3Long64V1` интерпретатора. Он проверяет 36-битное значение и завершает инструкцию со сменой контекста трансляции. PCID вне профиля, запись с установленным битом 63 поднимает `#GP(0)`. Следующая граница берёт новое полное значение CR3 из состояния. Обходчик выделяет физический адрес корня.

Нативный вызов получает текущий CR3 явным параметром. Только внутри вызова поле скрыто для профилей V2 и V4, поддержанные формы которых не обращаются к CR3. При возврате поле восстанавливается до публикации записей и доставки событий. Каждый допуск заново проверяет физическую страницу кода, её поколение и байты. Для операнда используется новый обход с текущим CR3. IRQ, гостевые исключения и NMI используют собственный CR3 vCPU при его наличии. [Проверки](../reports/STAGE_6_S6-42c_2026-11-18.md#s6-42d--смена-cr3-в-кванте-хоста).

**Частичный `REP` S6-42d.** После `RepIterationBudgetExhausted` CPL0 host-цикл пропускает один опрос IRQ перед продолжением той же инструкции. При STOP вызывающая сторона сохраняет этот признак для следующего вызова. Регрессия проверяет нулевой расход бюджета промежуточной порцией, завершение `REP MOVSB` после снятия STOP и доставку ожидающего IRQ на следующей границе. [Проверка](../reports/STAGE_6_S6-42c_2026-11-18.md#s6-42d--частичный-rep-и-ожидающий-irq).

**Источник платы S6-42d.** В CPL0-кванте BSP сначала опрашивается источник платы, затем LAPIC. AP опрашивают свои LAPIC. Адаптер блокирует источник на время отдельной операции. После записи кадра подтверждение получает выбранный источник. Запись в порт PIT завершает квант, если срок источника зависит от PIT. Следующий вызов получает бюджет по пересчитанному сроку. [Проверки](../reports/STAGE_6_S6-42c_2026-11-18.md#s6-42d--источник-прерываний-платы-в-кванте-хоста).

**COM1 и тики портов S6-42d.** CPL0-квант даёт потоку обслужить очередь COM1 и тики PIT/PM на границе вызова. После успешного доступа к порту шаг проверяет наличие этой работы и завершает квант при ожидающем обслуживании. Уже поставленная в очередь работа может завершить вызов с нулём инструкций. Общий насос сохраняет байты COM1 по порядку завершения. Собранные тики передаются в `HostQuantumResult::device_ns` и начисляются владельцем часов. [Проверки](../reports/STAGE_6_S6-42c_2026-11-18.md#s6-42d--com1-и-тики-портов-в-кванте-хоста).

**Остановка по ACPI S6-42d.** CPL0-квант наблюдает защёлкнутый запрос ACPI перед первой гостевой инструкцией и после успешного PIO. Инструкция OUT, поставившая запрос, учитывается вместе с собранными тиками. Рабочий поток публикует `GuestStop(Shutdown)` или `GuestStop(Reset)` после начисления времени и подаёт STOP остальным vCPU. Reset сохраняет запрос для отдельного жизненного цикла машины. [Проверки](../reports/STAGE_6_S6-42c_2026-11-18.md#s6-42d--остановка-host-run-по-acpi).

**NVMe S6-42d.** Квант обнаруживает ожидающий doorbell перед первым шагом и после успешной записи MMIO. Рабочий поток выполняет обслуживание между гостевыми инструкциями через `PhysicalMemory`. Запись DMA публикует поколения страниц сразу, наблюдатели получают диапазоны после освобождения замка NVMe. STOP и защёлкнутый запрос ACPI проверяются перед началом обслуживания. Гостевой бюджет учитывает инструкции, синхронный DMA использует существующую модель времени контроллера. [Проверки](../reports/STAGE_6_S6-42c_2026-11-18.md#s6-42d--nvme-doorbell-и-dma-в-кванте-хоста).

**Слоты в Parallel S6-42d.** Production host-цикл требует выделенную часть region перед lookup в Parallel. Без части цикл освобождает ранее опубликованный собственный блок и исполняет шаг интерпретатора. Допуск V2 и V4 одинаково зависит от наличия части. Текущий production-кеш держит одну запись и освобождает её перед заменой. Правило FIFO задаёт порядок для нескольких собственных записей при расширении кеша. Однопоточные fixture-бэкенды остаются в профиле A5. [Проверки](../reports/STAGE_6_S6-42c_2026-11-18.md#s6-42d--допуск-параллельного-блока-только-в-слот-vcpu).

**NVMe INTx S6-42d.** При настроенном маршруте платы завершение CQE поднимает PCI INTx через IOAPIC. CPL0-квант доставляет вектор через IDT до следующего шага. Запись гостем CQ head снимает запрос устройства, а LAPIC EOI завершает обслуживание level-вектора. [Проверки](../reports/STAGE_6_S6-42c_2026-11-18.md#s6-42d--доставка-nvme-intx).

**Открыто (S6-42d).**
- **NVMe.** Задержка STOP при синхронном вызове provider остаётся открытой. [ISSUE-515](../KNOWN_ISSUES.md#issue-515-s6-42d-nvme-stop-latency).
- **NMI** доставляется на границе вызова кванта (не между инструкциями внутри вызова), как S6-34; границы кванта ≤ `quantum` инструкций.
- **NMI и маскируемое прерывание внутри вызова:** NMI ждёт только границы вызова; внутри вызова при ожидающем NMI может уйти маскируемое прерывание (как записано для S6-34).
- **Стоимость границы:** вопрос `eligible()` берёт замок APIC на каждом шаге (в режиме набора); не измерена (S6-37, S6-38). Задержка таймера до границы кванта не измерена.
- Размеры частей region требуют замеров (M2). Точная норма Arm для STOP-байта приведена в S4 раздела S6-40.
- Linux-контроль (прогон образа) для S6-42c не запускался: пути чередования не менялись (выше), проверено регрессиями; пометить для ревью.

### S6-41e: litmus S6-19/S6-36 с JIT в обоих режимах

Дата: 2026-11-18. S6-41e принят для атомарных форм профиля S6-41d. Оба litmus-драйвера используют `LitmusStepHook` и `execute_atomic`. JIT исполняет все атомарные формы корпуса, включая восемь fixture из пятнадцати и `LOCK-INC`. Обычные MOV и INC без LOCK проходят через интерпретатор. [Отчёт](../reports/STAGE_6_S6-41e_2026-11-18.md), [данные](../reports/data/stage6/s6-41e/).

**Что подключено.**
1. **Точка подключения — хук исполнения инструкции.** `od-machine` от `od-host` не зависит, а `parallel_run` — `pub(crate)`; поэтому `od-machine` отдаёт трейт `LitmusStepHook` (`step(LitmusStep) -> Result<LitmusStepOutcome, String>`, `thread_exit(cpu)`; `Sync`), а `od-host` его реализует (`JitLitmusHook`). Драйвер предлагает хуку каждую инструкцию до интерпретатора; хук возвращает `Executed { retired }` или `Declined` (тогда инструкцию исполняет интерпретатор по одной за вызов). Сами прогоны, барьеры (стартовый барьер пересекается до хука, как до интерпретатора), размещения, гистограмма и эталон — код S6-36/S6-19 без изменений; `run_litmus_parallel` без хука идёт тем же кодом (`hook: None`, бюджет интерпретатора прежний). Хук получает `&MachineBus`, состояние своего vCPU, `cr3` и `efer` стенда.
2. **Параллельный режим — `od_host::litmus_jit::run_litmus_jit_parallel(case, vcpus, rounds)`**: `od_machine::run_litmus_parallel_hooked` — потоки `run_threads_with` (те же потоки S6-34, барьер, kick, часы владельца; фасад `run_host_threads` S6-42b — тонкая обёртка над тем же `run_threads_with`, но квант litmus использует интерпретатор платы и источник платы, которых у `HostQuantumCall` нет, поэтому драйвер S6-36 остаётся хозяином потока, а хук выходит из потока явным `thread_exit`). **Отклонение от формулировки плана:** `run_host_threads` для litmus не используется (его квант — базовый профиль интерпретатора `run_paged_probe_shared`, который не знает `MOV [disp32], imm32` корпуса и `LOCK`-форм).
3. **Режим чередования — `run_litmus_jit_interleaved(case, max_exhaustive, seeds)`**: `od_machine::run_litmus_interleaved_hooked` — тот же перебор S6-19 (все порядко-сохраняющие расписания при числе расписаний не больше порога, иначе расписания по seed), но **на шине платы**, не на плоском `LitmusRam` S6-19 (на нём JIT исполняться не может): `SmpScheduler` с `QuantumPolicy::Schedule`, `BoardSmpHost`, один vCPU на поток litmus, один шаг планировщика на инструкцию, `ExecutionMode::Interleaved`, интерпретатор — CPL0-набор платы. Эталон — множество `interleaving_instruction_atomic` fixture (как S6-19), прочитанное из файла; в отчёте и множество x86-TSO.
4. **Хук JIT.** Инструкция fixture классифицируется до прогона (`jit_classification`): форма, которую переводит `lower_atomic_instruction`, идёт через `PagedFetchedLoadSession::execute_atomic` (блок до 16 подряд атомарных форм из окна 240 байт, helper, последовательность в режиме памяти шины); иначе хук отказывает, интерпретатор исполняет инструкцию. Сессия — на пару (vCPU, поток litmus), на своей части общего `MAP_JIT` region (`SharedJitRegion`, S6-42): vCPU-поток исполняет разные потоки litmus по размещениям, а сессия держит один блок, поэтому сессия на vCPU перекомпилировала бы блок каждый раунд. Сессии создаются и роняются на потоке vCPU (`thread_exit` в `run_threads_with`; A1, A4, M1 S6-42b). Состояние `cr3` (CPU-собственный CR3 у vCPU litmus) на время вызова изымается и возвращается: сессия отказывает при `state.cr3 = Some`.
5. **Страницы стенда.** Для прогонов с JIT первые 2 МиБ отображаются 512 тождественными страницами 4 КиБ (таблица страниц `TABLES + 0x3000`, листья P|W|A|D = `0x63`), а не одной страницей 2 МиБ: окно выборки и helper обходят только 4 КиБ (`UnsupportedPageSize` для листа PD, выяснено первым прогоном). Прогон S6-36 без хука по-прежнему на странице 2 МиБ. Это отклонение стенда JIT-прогонов от S6-36: путь обхода страниц интерпретатора здесь тоже 4 КиБ.

**Какие fixture идут через JIT.** Через `execute_atomic` исполняются: `LOCK-CMPXCHG-MUTEX`, `LOCK-XADD`, `SB-LOCK-LOCAL` (`LOCK INC`), `SB-LOCK-XADD`, `SB-LOCK`, `SB-MFENCE` (`MFENCE`), `SB-XCHG`, `XCHG-MUTEX` и счётчик `LOCK-INC` (N = 2 и N = 4, K = 2; подряд идущие `LOCK INC` образуют блок из нескольких инструкций, в параллельном режиме блок повторно используется, в чередовании бюджет шага 1 и блок допускается заново). Атомарная инструкция исполняется JIT; остальные инструкции тех же fixture (`MOV dword [disp32], imm32`, `MOV r32, [disp32]`) — интерпретатор. **Не через JIT:** `2+2W`, `IRIW`, `LB`, `MP`, `SB`, `STORE-FORWARDING`, `UNLOCKED-INC`, счётчики `UNLOCKED-INC` (N = 2, 4) и `THREE-INSN-COUNTER`: в них нет ни одной формы, которую допускает блок S6-41d (только обычные `MOV` с абсолютным адресом и `INC` без `LOCK`); обычные load/store внутри атомарного блока не допущены (JIT_CONTRACT, S6-41d), а формы `execute_forms` (S4-15) — `RBX`/`RIP`-относительные, не `[disp32]` корпуса. Запуск такого fixture с JIT отказывает (`not counted`) и не засчитывается; корпусный прогон записывает их в `not_via_jit` с классификацией инструкций. Доля интерпретатора записана по каждому прогону: `interpreted_retired` (обычные `MOV` и `INC`) против `hooked_retired`; у JIT-fixture откат JIT-форм в интерпретатор (`fallback_of_jit_forms`) — 0.

**Результаты.** Release, Apple M3 Max, три процесса по 1 000 000 раундов на тест и конфигурацию (параллельный режим, 2 и 4 vCPU; 3 000 000 раундов на конфигурацию), чередование — все порядко-сохраняющие расписания (перебор полный: 2…2520 расписаний на случай, а не seed). **Запрещённых исходов 0** во всех 29 прогонах каждого из трёх процессов; чередование: наблюдаемое множество **равно** множеству fixture во всех 10 случаях; `late_barrier_rounds = 0`. Через JIT исполнено 100% ожидаемых атомарных инструкций каждого раунда (например `SB-XCHG`, 2 vCPU: 6 000 000 из 6 000 000; `LOCK-INC` N = 4: 24 000 000 из 24 000 000), блоков допущено 6…48 на конфигурацию за три процесса (по одному на пару vCPU–поток в процессе, остальные раунды используют блок повторно). Время процесса — 230, 232 и 210 с (сумма 11,2 мин). Сводка — `summary.md`, сырые данные — `process-{1,2,3}.json`, журнал — `run.log`.

**Чувствительность («JIT реально исполнял»).** Прогон засчитывается только при `JitLitmusCounters::counted()`: `jit_retired > 0`, `jit_calls > 0` и откат JIT-форм равен 0 (то есть JIT исполнил все формы fixture каждого раунда; число = `jit_forms_per_round × rounds`); тест `s6_41e_a_run_without_jit_execution_or_with_fallback_is_not_counted` проверяет саму проверку, тест корпуса падает при незасчитанном прогоне. **Контрольная мутация проведена** (код мутаций откатан, данные корпуса не менялись). A. Неатомарный `LOCK INC` (load/пауза/store +1 без JIT), параллельный режим, 100 000 раундов: **ловится** (`LOCK-INC` N = 2, K = 2: 2 запрещённых исхода; `SB-LOCK`: 1); чередование не ловится (инструкции неделимы по построению). B. `MFENCE` выброшен (только `RIP += 3`), параллельный режим, 2 и 4 vCPU, по 300 000 раундов: **не ловится** (`SB-MFENCE`: 0 запрещённых); чередование не ловится (последовательная согласованность, барьер проверить нельзя). Причина B: стенд S6-36 ни разу не дал слабый исход SB (0,0) — SB 3 из 4 исходов, `SB-LOCK-LOCAL` 3 из 4 (`s6-36/litmus-parallel/summary.md`). Итог: атомарность JIT-форм проверяют `LOCK-INC` и `SB-LOCK`; барьерная семантика JIT litmus не проверена.

**Ограничения (записаны).**
- Атомарность JIT-форм проверяют `LOCK-INC` и `SB-LOCK`. Обычные доступы в корпусе идут через интерпретатор. Мутация, удаляющая `MFENCE` в гостевом стенде, не вызвала слабого исхода SB.
- **Не закрыто (открытый пункт).** Проверка барьеров JIT требует стенда, где наблюдается SB (0,0): например, сборка `weak-host-ordering` вместе с JIT (отдельный feature, только для тестов) или отдельный тест на сгенерированном коде (как S6-41 MP/SB) с контрольным исходом.
- Парный контроль сгенерированного кода декодирует гостевой `MFENCE`, компилирует блок V4 и выполняет его машинное слово `DMB ISH` между нативными `STR` и `LDR`. За 6 млн раундов получено 0 исходов SB `(0,0)`. Замена слова на `NOP` дала 3 193 062 исхода за 6 млн. Проверка выполняется отдельно от обоих гостевых драйверов. [Команда и исходы](../reports/STAGE_6_S6-41e_barrier_2026-11-18.md#8-парный-контроль-машинного-слова-гостевой-mfence).
- Хук исполняет по одной инструкции или блоку подряд идущих атомарных форм за вызов; split `LOCK` и невыровненный операнд в корпусе нет (шлюз split `LOCK` хосту-кванту не передан, S6-42c).
- Стенд JIT — страницы 4 КиБ (выше), одна `MAP_JIT` region на процесс: прогон берёт блокировку теста и свою region.
- Чередование — собственный драйвер на шине платы, а не драйвер S6-19 на `LitmusRam`; расписания и эталон те же, равенство множеств проверено, прогон интерпретатора на этом же драйвере (без хука) не делался; интерпретатор на `LitmusRam` — прогон S6-19.
- Размеры: 1 000 000 раундов на тест, три процесса (как S6-36); статистическая сила та же (правило трёх: около 1·10^-6 на раунд); гостевой `LOCK INC` на 20 000 инкрементов S6-36 (стенд guest) с JIT не прогонялся.
- Квант и интерпретатор: хук не доставляет прерывания (в litmus их нет); STOP-байт native-кода не поднимается (блок завершается сам); G-XMC (cross-modifying code) этим прогоном не доказан (S6-43a).
- Результаты получены на одной ARM64-машине; замеров производительности JIT нет (скорость прогона не показатель, S6-38).

### S6-43a: probe cross-modifying code на двух vCPU с включённым JIT

Дата: 2026-11-18. Статус: **реализовано частично** (чекбокс S6-43a в `STAGE_6.md` не отмечен, ревью отдельно). Контракт S6-40 не менялся. Код: `crates/od-host/src/paged_host_loop/tests/s6_43a.rs` (новый, тесты и сценарий), `crates/od-host/src/host_threads.rs` (`JitHostQuantum::with_write_log`, `committed_writes` — необязательный журнал зафиксированных записей по vCPU, по умолчанию выключен), `crates/od-host/src/paged_host_loop.rs` (подключение модуля). Продуктовый код JIT не менялся. Отчёт — [STAGE_6_S6-43a_2026-11-18.md](../reports/STAGE_6_S6-43a_2026-11-18.md), данные — [`docs/reports/data/stage6/s6-43a/`](../reports/data/stage6/s6-43a/summary.md). Сценарий живёт в `od-host`, а не в `stage6_smp_probe.rs` (probe S6-16): `od-machine` не зависит от `od-host`, а гостевой код probe S6-16 исполняется интерпретатором платы.

**Сценарий.** vCPU A и vCPU B, общая `&MachineBus`, у каждого своя сессия JIT на своей части одного `SharedJitRegion` (A1, M1, M2). Страница кода B (физический 0x8000, линейный `CODE_RIP`) содержит изменяемую инструкцию X, ожидание сигнала W (`mov rax,[rbx]`) и шаг C, кончающий блок (`CLD`; по SDM не сериализующая). Две версии X — формы `QwordLoadRbxDisp8` одного допущенного JIT вида `PagedFetchedLoadSession`, но с разными байтами: `48 8B 43 00` (загрузка первой ячейки, версии 0, 2, …) и `48 8B 43 10` (загрузка второй ячейки, версии 1, 3, …); результат запуска (RAX = `0x0123456789ABCDEF` или `0x5555666677778888`, RAX перед шагом — `0xAAAA…`) называет исполненную версию. Цикл `k` (1…N):
1. B исполняет X (старая версия `k − 1`) и допускает её блок; результат проверяется.
2. A исполняет нативный `mov [rbx],rax` с RBX = X: пишет байты версии `k` в страницу кода B (`commit_write`: данные, затем `fetch_add(Release)` поколения), затем нативный `mov [rbx],rax` с RAX = `k` и RBX = ячейка сигнала (сигнальная запись `SeqCst` в параллельном режиме, S6-32).
3. B ждёт сигнал (читает ячейку, пока значение не станет не меньше `k`), исполняет шаг, кончающий блок (`CLD`), и дважды исполняет X; оба запуска обязаны дать результат версии `k`; результат версии `k − 1` — исполнение устаревшего кода (`stale`), другой — `wrong`.
4. Следующий цикл A начинает после конца цикла B (в параллельном режиме — по атомарным счётчикам хоста: B опускает `old_done` после запуска старой версии, `cycle_done` после цикла; ожидания A и B — вращающиеся циклы с тайм-аутом).

**Три протокола** (`PROTOCOLS`). (1) `guest-wait+serialize` — B ждёт сигнал guest-загрузкой W (нативный шаг), затем `CLD` (шаг интерпретатора), затем X: вторая ветвь G-XMC п. 2.4 (конец блока); SDM-форма с сериализующей инструкцией не проверена. В профиле одной инструкции на блок (бюджет 1) протокол (1) механически совпадает с (2) — отдельного доказательства с сериализацией он не даёт; реальная сила — свежая компиляция после сигнала (2) и сверка ключа у сохранённого блока (3). (2) `guest-wait` — без шага, кончающего блок: у машины гарантия сильнее SDM (запуск после увиденного поколения не входит в старый блок). (3) `host-wait (block kept)` — сигнал читает хост загрузкой памяти режима (`SeqCst`) без шага guest, поэтому блок X, допущенный на шаге 1, остаётся допущенным до шага 3 и единственное, что его сбрасывает, — сверка ключа на входе (D1): этот протокол видит мутацию сверки. В протоколах (1) и (2) между шагом 1 и шагом 3 исполняется другая инструкция, сессия держит один блок и сбрасывает допуск, поэтому там каждый запуск X компилируется заново и сверка поколения в `reusable` не проверяется (проверяется только то, что компиляция читает свежие байты).

**Три исполнителя одного кода циклов.** Функции цикла (`b_before_write`, `a_write_and_signal`, `b_after_signal`) работают с трейтом `Stepper` (один шаг = одна инструкция: RIP, RAX = RCX и RBX заданы драйвером, дальше исполнение): (а) интерпретатор od-isa (`run_paged_probe`, шина под `RefCell`) — эталон; (б) режим чередования: один поток, две сессии JIT (по одной на vCPU, A1), расписание «B старая версия, A пишет и подаёт сигнал, B ждёт, шаг конца блока, исполняет»; бюджет шага 1, `ExecutionMode::Interleaved`; (в) параллельный режим: `od_machine::run_host_threads` (по потоку на vCPU), квант — `JitHostQuantum::run` с бюджетом 1 на каждый шаг цикла, замка шины нет, `ExecutionMode::Parallel`. **Ограничение формы:** профиль интерпретатора не имеет ни сравнения, ни перехода, ни `CPUID`, ни форм с смещением, поэтому циклы (ожидание сигнала, счётчик циклов) ведёт драйвер хоста, а шаги — инструкции гостя; интерпретатор исполняет те же версии как `mov rax,[rbx]` (`48 8B 03`, четвёртый, не исполняемый байт отличает версии в RAM) с RBX, перенесённым на вторую ячейку в нечётных версиях, и store A как `mov [rbx],rcx` (RAX = RCX у обоих); поэтому RIP B на конце на байт короче у эталона (проверяется явно), а слово X в RAM сверяется отдельно по кодировке каждой стороны. `CLD` — шаг, кончающий блок (S6-40 п. 2.4); по Intel SDM он **не** сериализующая (сериализующие непривилегированные: `CPUID`, `IRET`, `RSM`, `SERIALIZE`; по памяти). Отступление от `CPUID`: базовый профиль `run_paged_probe_shared` отвечает на `0F` отказом `Unsupported` (`cpuid_boundary_does_not_apply_on_the_paged_probe_path`), и `CPUID` в нём не исполняется. **Эталон даёт ожидаемые значения, но не проверяет cross-modifying code:** у версий X интерпретатор исполняет одни и те же байты `48 8B 03`, результат задаёт RBX (`version_rbx`); конечный RBX B сверяется с `version_rbx` каждой стороны.

**Проверки.** (1) Внутри цикла: результат запуска до записи — версия `k − 1`, после сигнала — версия `k`, оба раза; иначе `stale`/`wrong` и отказ теста. (2) По окончании: регистры обоих vCPU, вся RAM ниже страницы кода A (кроме слова X), зафиксированные записи. В чередовании сравниваются полные списки зафиксированных записей каждого vCPU (включая биты Accessed/Dirty таблиц страниц); в параллельном режиме — только записи данных (адрес не ниже 0x8000, в порядке программы vCPU): биты таблиц, общих для обоих vCPU, фиксирует тот, кто первым их тронул, и их порядок зависит от времени. (3) Прогон засчитывается (`BRecord::counted`), только если B исполнил X нативно один раз до записи и два раза после сигнала в каждом цикле (`x_old_jit == N`, `x_new_jit == 2N`) и ни один запуск X не ушёл в интерпретатор (`x_not_jit == 0`); store A проверяются нативными в каждом цикле. Тест `s6_43a_a_run_without_native_execution_of_the_changed_code_is_not_counted` показывает, что прогон интерпретатора и прогон на единицу короче правила не засчитываются. Доля интерпретатора записывается в каждый результат: она целиком из шага, кончающего блок (`CLD`; протокол (1): 2 000 000 интерпретаторных шагов из 11 875 239 шагов B, 16,8%; в остальных протоколах 0%); у запусков X доля 0%.

**Тесты (по именам).** `od-host --lib`, модуль `paged_host_loop::tests::s6_43a`: `s6_43a_the_interpreter_reference_gives_the_expected_values_around_the_signal` (эталон, 40 циклов, три протокола), `s6_43a_interleaved_b_runs_only_the_new_bytes_after_the_signal_and_equals_the_interpreter` (чередование: 600 сигналов на протокол, 1 800 запусков X на протокол, сверка с интерпретатором, в том числе полных списков записей), `s6_43a_parallel_b_runs_only_the_new_bytes_after_the_signal_and_equals_the_interpreter` (`run_host_threads`: 2000 сигналов на протокол), `s6_43a_a_run_without_native_execution_of_the_changed_code_is_not_counted` (правило учёта), `s6_43a_long_parallel_run` (`#[ignore]`, release, `OD_S6_43A_CYCLES`, по умолчанию 2 000 000 сигналов на протокол).

**Результаты** (Apple M3 Max, ARM64, одна машина).
- Быстрая форма (debug): 3 протокола × (600 чередование + 2000 параллельных) сигналов: `stale 0`, `wrong 0`, X нативно 100% (3 запуска на цикл), регистры, RAM и записи равны интерпретатору.
- Длинная форма (release, параллельный режим, `run_host_threads`): **2 000 000 сигналов на протокол, 6 000 000 всего, 18 000 000 запусков X, все нативные, `stale 0`, `wrong 0`**, результат равен интерпретатору (регистры, RAM, записи данных) в каждом из трёх прогонов; 81 с всего (23,9 / 25,2 / 19,6 с, около 79–102 тыс. сигналов в секунду). Протокол (1): шагов B нативных 9 875 239, интерпретатора 2 000 000 (16,8%, все — `CLD`), шагов ожидания 3 875 239 (до 1 283 на цикл), допусков 5 401 473; протокол (2): нативных 10 822 682, интерпретатор 0%, шагов ожидания 4 822 682 (до 13 616 на цикл), допусков 5 499 346; протокол (3): нативных 6 000 000, интерпретатор 0%, допусков 2 000 001 (по одному на цикл: блок меняется записью A, второй запуск после сигнала использует его заново). A: нативных 4 000 000, интерпретатор 0 в каждом прогоне.
- Число повторов: `SMP.md` не задавал число для cross-modifying code; принято и записано в `SMP.md`: быстрая форма — 600 (чередование) и 2000 (параллельный) сигналов на протокол, длинная — 2 000 000 сигналов на протокол (не меньше 10 000); числа 600 / 2000 / 2 000 000 / не меньше 10 000 выбраны исполнителем S6-43a, требуют подтверждения ревью (в плане их нет).

**Чувствительность (контрольные мутации).**
- **Мутация сверки поколения и байт на входе** (из `reusable` в `execute_forms_with_stop`, `crates/od-host/src/lib.rs`, убраны `admitted.generation == snapshot.backing_generation` и `admitted.bytes == snapshot.bytes`; код восстановлен): **ловится** протоколом (3) в обоих режимах — чередование: `stale 600`, `wrong 300` из 1800 запусков X, допусков 1 (блок не перекомпилируется); параллельный: `stale 2000`, `wrong 1000` из 6000, допусков 1; протоколы (1) и (2) мутацию **не ловят** (сессия сбрасывает допуск на шаге между запусками). Вывод — `mutation-reusable-debug.txt`.
- **Ослабление порядка** (`--features od-machine/weak-host-ordering`, release: обычные доступы и сигнал `Relaxed`, поколение без `Release`; 200 000 сигналов на протокол): **не ловится** (`stale 0`, `wrong 0`; `weak-host-ordering-release-200000.txt`). Причина та же, что у S6-36 и S6-41e (мутация B): сценарий строит цепочку «запись кода, запись поколения, сигнал, чтение сигнала, чтение поколения», и на этой машине слабый порядок в ней не проявился за 600 000 запусков; отсутствие исхода порядок не доказывает (SMP.md, 5.1). Корректность порядка здесь держится на документированной цепочке G-XMC (S6-40 п. 2.4), а не на этом прогоне.

**Ограничения (записаны честно).**
1. Одна ARM64-машина (Apple M3 Max); других хостов и производительности нет.
2. Профиль «одна инструкция на блок» (формы `PagedFetchedLoadSession`, `QwordLoadRbxDisp8` и `QwordStoreRbx`); граница «текущий блок дорабатывает ≤31 инструкции» не проверялась; многоинструкционные блоки со store (S3-24) не затронуты.
3. Проба S6-43a проверяет публикацию изменённого кода и инвалидацию. Барьер `MFENCE` проверен отдельным контролем S6-41e. Цепочка G-XMC использует `SeqCst` памяти режима и `Release`/`Acquire` поколений. Экспериментальное ослабление порядка в S6-43a слабого исхода не выявило.
4. `CPUID` как сериализующая инструкция не исполнялась: база профиля интерпретатора её не допускает, JIT тоже; взят `CLD` (конец блока и следующий lookup, S6-40 п. 2.4), он по SDM не сериализующая; SDM-форма протокола (093US §11.1.3) с сериализующей инструкцией не проверена, пометки «по памяти» S6-40 не сверялись по нормативным текстам.
5. Циклы (ожидание сигнала, счётчик циклов, рукопожатия `old_done`/`cycle_done`) ведёт драйвер хоста, не гостевой код; рукопожатие хоста между A и B не часть проверяемого протокола (оно только не даёт A переписать код во время запусков B, чтобы ожидаемый результат был определён); сигнал в протоколах (1) и (2) читается и пишется гостевыми инструкциями, в протоколе (3) читается хостом (`SeqCst`-загрузка той же памяти).
6. Сверка сессии с мутацией «поколение выросло, байты те же» отдельно не проводилась: версии отличаются байтами.
7. Эталон — интерпретатор в чередовании на одной шине; для нечётных версий X его кодировка и RBX другие (профиль интерпретатора не имеет формы со смещением), поэтому «те же байты программы» у JIT и эталона только по смыслу, а байты слова X сверяются по кодировке каждой стороны.
8. Доставка прерываний и исключений гостю, шлюз split `LOCK`, STOP-байт native-кода — открытые пункты S6-42c; в этом probe их нет.
9. Доля интерпретатора 16,8% в протоколе (1) — только шаг `CLD`, он не нативен по построению; запуски X — 0%.
