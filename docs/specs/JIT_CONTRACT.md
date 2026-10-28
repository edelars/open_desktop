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
Its unsafe caller contract requires immutable non-aliasing code bytes and
private synchronous ordinary data RAM with all-or-nothing writes, no MMIO,
executable/code aliases, DMA, concurrency, re-entry, or unwind. `GuestMemory`
does not prove those ownership properties. There is no production RAM/MMU
owner in `od-machine` yet; VMHost owner integration and owner-supplied code
fetch remain open. This does not cover a guest fault on the next code fetch,
general S3-15, interpreter fallback,
physical x86 comparison, cache/invalidation, or performance acceptance. See
the [bounded memory dispatcher report](../reports/STAGE_3_S3-15_2026-10-27.md#bounded-checked-memory-dispatcher-partial).

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
differential tests do not satisfy them. See [ISSUE-021](../KNOWN_ISSUES.md).

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
