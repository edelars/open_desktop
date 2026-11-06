# od-isa

Архитектурное состояние x86, собственный декодер и эталонный интерпретатор.

Не зависит от JIT, машины, host API и дискового формата. Данные CPU и семантика инструкций — здесь; исполнение ARM64 — в od-jit.

Статус на 22 октября 2026 года: decoder и reference interpreter исполняют ограниченный S0-09 из `docs/specs/CPU_CONTRACT.md` (`MOV r64, imm64`, register-only `ADD/SUB/CMP`, `JZ/JNZ`). Есть проверки branch boundaries, flags и instruction budget; этот профиль независимо сравнивается с физическим Intel и ARM64 JIT.

`integer_flags::updated_arithmetic_flags` — общий pure helper для определённых
W64 ADD/SUB/CMP flags, используемый JIT helper adapter; он не исполняет guest
instructions и не расширяет поддерживаемый interpreter профиль. Literal
vectors, сохранение невычисляемых RFLAGS bits и cross-check с reference
interpreter покрывают текущий S3-11 slice.

В S2-02 добавлены `LongModeState` (16 GPR, RIP u64), `OperandSize` и проверяемые `RegisterView`: low aliases 8/16/32/64, high bytes и различия REX. Это состояние для следующего integer core, а не подтверждение исполнения расширенного ISA. `FragmentState` с C layout и прежнее имя `CpuState` сохраняют четырёхрегистровый S0 API. Явный `TryFrom<&LongModeState>` отклоняет дополнительные регистры, несовместимые flags и непредставимый RIP. Fixture v1 и C ABI v0 не расширены. Подробности и проверки — [отчёт этапа 2](../../docs/reports/STAGE_2_REPORT.md). Выполнять `cargo test -p od-isa --offline`.

Локально принятый S2-08 добавляет реальное исполнение MOV B0–BF immediate register
и 88–8B reg/reg/load/store, widths 8/16/32/64, все 16 GPR и high-byte/REX
aliases через `execute::step/run`. MOV сохраняет flags; memory range проверяется
до backend и commit. Arithmetic и short branches сохраняют S0 execution
ограничения; другие decoder descriptions не означают исполнение. Точный
контракт и непроверенная u64 endpoint boundary ISSUE-012 — в
[CPU contract](../../docs/specs/CPU_CONTRACT.md#исполнение-mov--s2-08).
S2-09 физически подтвердил три положительные MOV/memory программы и прежний
S0 corpus на одном Intel CPU; это не полное ISA покрытие. Evidence — в
[CPU_ORACLE](../../docs/testing/CPU_ORACLE.md#native-v2-s2-09-границы-физического-подтверждения).

S2-37a/b add two separate local-only scalar approximation instructions:
register `RSQRTSS` and `RCPSS`. They reuse deterministic integer-only
`Binary32` references and update only the destination low dword; the rest of
XMM and CPU state is preserved. MXCSR flags/exceptions and CPUID/control gates
are outside these profiles. The exact helper bits are not physical CPU claims;
RCPSS's tiny-output hardware threshold remains unresolved in ISSUE-025. JIT
and native admission stay closed. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#local-s2-37b-register-only-rcpss).

S2-37c adds local-only register `RSQRTPS`: four binary32 lanes use the
existing deterministic value reference and replace all 128 destination bits.
All unrelated state, including MXCSR, is preserved. Literal mixed-lane tests
cover result routing; hardware checks use the Intel `3/8192` error envelope,
not exact output bits. SSE/control gates are assumed, while CPUID, FP flags,
exceptions, JIT and native execution remain outside the profile. See the
[CPU contract](../../docs/specs/CPU_CONTRACT.md#local-s2-37c-register-only-rsqrtps).

S2-37d adds local-only packed register `RCPPS`, applying the existing
deterministic binary32 reciprocal reference to four copied source lanes before
replacing the destination XMM. Raw MXCSR and all unrelated state are preserved.
ISSUE-025 leaves the hardware tiny-result boundary ambiguous; the local helper
does not promise hardware-exact result bits. JIT/native remain closed, and
physical validation is reserved for the combined Mac mini substage. See the
[CPU contract](../../docs/specs/CPU_CONTRACT.md#local-s2-37d-register-only-rcpps).

S2-37dy adds exact memory-only `0F 53 00` RCPPS from `[RAX]` as a separate
profile. It requires an aligned 16-byte read and checked canonical48 endpoints,
uses only the memory lanes, replaces all of XMM0, and preserves MXCSR for every
accepted RC/sticky state. The deterministic helper and ISSUE-025 limitations
remain the same as register RCPPS. JIT/native/generic routes stay closed. See
the [CPU contract](../../docs/specs/CPU_CONTRACT.md#local-s2-37dy-memory-rcpps-at-rax).

S2-37m adds only exact Long64 `F3 0F E6 C1` CVTDQ2PD XMM0,XMM1 under a strict
schema-v2 profile with explicit SSE2/CR0/CR4 gates. It converts two signed
dwords to exact binary64, preserves MXCSR and unrelated state, and performs no
data-memory access. The independent fixture validator uses integer-bit
construction rather than the production conversion helper. Other encodings,
generic CPUID, JIT/native, and physical acceptance remain out of scope. See the
[CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37m--exact-long64-cvtdq2pd-xmm0xmm1).

S2-10 добавляет C6/C7 immediate r/m, A0–A3 moffs, MOVSX/MOVSXD/MOVZX,
LEA и register-only XCHG/NOP90 с сохранением flags. Пять дополнительных
transfer программ и три прежних MOV cases прошли physical interpreter/native
сравнение дважды на одном Intel вместе с legacy corpus. Memory XCHG/atomicity,
LOCK and a complete CPU are not claimed. S2-19a adds only a bounded REP STOSB
slice. Exact forms and evidence are in
[CPU contract](../../docs/specs/CPU_CONTRACT.md#передача-данных-и-lea--s2-10).

S2-11 реализует ADD/ADC/SUB/SBB/CMP, INC/DEC/NEG widths8/16/32/64,
reg/memory/immediate forms и eager flags. 313 новых literal cases и восемь
прежних v2 cases прошли physical сравнение дважды, вместе с legacy corpus.
Injected local no-commit faults не доказывают native guest #PF. Точные формы,
flags и ограничения — в [контракте](../../docs/specs/CPU_CONTRACT.md#арифметика-и-eager-flags--s2-11).

S2-12 добавляет AND/OR/XOR/TEST/NOT и decoded `FlagEffects` metadata.
AF undefined после logic/TEST, NOT сохраняет values/definedness; mask меняется
только после успешного retirement. 197 новых cases и прежний corpus прошли
518×2 physical сравнения без различий. Точные границы — в
[контракте](../../docs/specs/CPU_CONTRACT.md#логика-test-и-definedness--s2-12).

S2-18a/b/c locally add bare MOVSB (`A4`), word MOVSW (`66 A5`), and default
dword MOVSD (bare `A5`) single iterations in long mode. The shared executor
reads `[RSI]` then writes `[RDI]`, adjusts both indices by the element width
according to DF after success, and preserves flags. REP, segment/address
overrides and unsupported widths remain outside the slice. Literal local
fixtures cover positive movement/overlap and injected access faults; post-fault
hardware restart state remains open in [ISSUE-016](../../docs/KNOWN_ISSUES.md#issue-016--не-установлен-restart-state-movs-после-destination-fault).
See [CPU contract](../../docs/specs/CPU_CONTRACT.md) and
[Stage 2 report](../../docs/reports/STAGE_2_REPORT.md) for exact boundaries.

S2-21g adds only bare `0F AE F0` MFENCE through the required
`GuestMemory::mfence` completion call. `FlatMemory` explicitly opts in for its
private synchronous RAM model; a fence backend error prevents retirement while
preserving effects from earlier completed instructions. This does not establish
shared-memory/SMP ordering, advertise SSE2, or implement other fence forms.

S2-21h adds only explicit `F0 48 87 /r` qword memory XCHG in the same
private synchronous serial `FlatMemory` profile. It reuses the existing
`XchgMemory` operation and one `GuestMemory::atomic_exchange_integer` call;
native admission stays closed and ISSUE-017 remains open.


S2-18e adds one bare `AA` STOSB iteration in long mode: AL is stored to flat
`[RDI]`, then RDI advances by one according to DF; RSI and flags are preserved.
Three mapped literal fixtures cover both DF directions and a pointer-alias/
canary case; one local-only injected destination fault checks no-commit
behavior. The hardware fault restart snapshot remains open in ISSUE-016.

S2-19a adds only `F3 AA` REP STOSB. RCX=0 performs no data access; each
successful byte store commits RDI and RCX, while STOP/fault/work-limit exits
preserve partial progress with RIP at the REP instruction. A separate
1024-iteration work limit does not affect instruction retirement accounting.
Other REP string forms and additional prefixes remain unsupported. See the
[CPU contract](../../docs/specs/CPU_CONTRACT.md) and
[fixture notes](../../tests/fixtures/x86/stage2/README.md) for the bounded
profile and local fault policy.

S2-22e adds a local-only CPL3 `VERW AX` profile with explicit GDTR state. The
interpreter checks writable-data descriptor rights from one read-only GDT
descriptor read; descriptor-table origin selects supervisor page privilege
without changing architectural CPL. Null/bounds failures clear ZF without a
read, while memory backend faults leave the instruction unretired. LDTs, other
privilege levels, descriptor loads, native execution and hardware validation
remain outside this slice. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md)
and [Stage 2 report](../../docs/reports/STAGE_2_REPORT.md).

S2-30ck adds exact bare Long64 `D9 FA` FSQRT to the interpreter for canonical
positive nonzero Normal binary80 at physical TOP and the twelve all-masked
PC24/PC53/PC64 FCW values. It rounds directly to target precision, updates only
TOP payload, sticky PE and C1 under a local undefined-condition-code policy,
then records FIP and retires once. The existing x87 selector gate remains in
force; availability, EM/TS, pending exception delivery, general exceptions,
native/JIT and physical acceptance are outside this slice. See the
[CPU contract](../../docs/specs/CPU_CONTRACT.md#local-s2-30ck-bounded-register-only-fsqrt-st0-d9-fa)
and [Stage 2 report](../../docs/reports/STAGE_2_REPORT.md#s2-30ck--bounded-bare-fsqrt-st0).

S2-22f/g adds one local-only Long64/CPL3 `MOV FS, AX` form. It performs a
single 8-byte descriptor-origin GDT read, checks type/readability and
privilege before presence, then handles A. A=1 needs no write; valid P=1/A=0
uses one one-byte `GuestMemory::write` at the descriptor access byte before
the FS cache and instruction commit. The backend call width is the chosen
semantic access size, not a claim about physical bus width. Descriptor writes
are limited to private synchronous RAM under serial execution without external
observers, DMA, shared aliases, delayed writes or MMIO. Null/TI, other
modes/CPLs, LDT, GS/SS and general accessed-bit updates remain unsupported. The
profile makes initial FS cache state explicit; older wire shapes are
unchanged, and native execution fails closed. Sources: Intel SDM rev.093
Vol.2B MOV pp.4-28–4-31 and Vol.3A §3.4.5.1 pp.3-12–3-13, §5.6.1
pp.5-33–5-35, §5.7 pp.5-37–5-38, §11.1.2.1 p.11-3 Automatic Locking. See
the [CPU contract](../../docs/specs/CPU_CONTRACT.md) and
[Stage 2 report](../../docs/reports/STAGE_2_REPORT.md).

S2-22h adds one local-only Long64/CPL3 `MOV GS, AX` form for exact bare
`8E E8` and a non-null GDT selector. It shares the bounded descriptor
validation and A-bit update semantics above, but maps only GS state in the
new `s2-long-gs-load-v1` profile; FS state remains a canary and existing wire
profiles are unchanged. Native admission fails closed. No physical execution
or general GS/SS, LDT, other-CPL or prefixed form is claimed. See the
[CPU contract](../../docs/specs/CPU_CONTRACT.md), [fixtures](../../tests/fixtures/x86/stage2/README.md)
and [Stage 2 report](../../docs/reports/STAGE_2_REPORT.md).

S2-30bz extends bounded bare FCMOVE from `DA C9` to indexed `DA C8..CF`.
It validates physical TOP and `(TOP+i)&7` before consulting integer ZF, except
self-index validates TOP only; taken moves copy the source's raw binary80 bytes
and tag. Full local state matrices and CLI assertions cover all indices. The
FCW/FSW gates are unchanged, and JIT/native/physical x86 support is not claimed.
See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#local-s2-30bz-bounded-indexed-bare-fcmove-st0-sti-da-c8cf),
[fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30bz-bounded-indexed-fcmove-st0-sti),
and [Stage 2 report](../../docs/reports/STAGE_2_REPORT.md#s2-30bz--bounded-indexed-fcmove-st0-sti).

S2-30bp extends the local raw-copy push slice from bare `D9 C0` to `D9 C0..C6`:
the selected source is physical `(old_TOP+i)&7`, and the Empty push destination
is `(old_TOP+7)&7`. Only canonical-Normal Valid or exact signed-Zero Zero sources
are admitted, with the same strict environment and atomic preflight. C7, JIT,
native and physical x86 behavior remain unsupported or unverified. See the
[CPU contract](../../docs/specs/CPU_CONTRACT.md#local-s2-30bp-bounded-bare-fld-sti-d9-c0c6),
[fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30bp-bounded-fld-sti),
and [Stage 2 report](../../docs/reports/STAGE_2_REPORT.md#s2-30bp--bounded-fld-sti).

S2-30br extends the tag-only FFREE slice from bare `DD C0` to exact bare
`DD C0..C7`. It empties physical `(TOP+i)&7` while preserving every payload,
other tag and status bit under the strict FCW/FSW gate. The 24,576-state matrix,
19-stream schema, indexed literal fixtures, local checks and exclusions are
documented in the [CPU contract](../../docs/specs/CPU_CONTRACT.md#local-s2-30br-bounded-bare-ffree-sti-dd-c0c7),
[fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30br-bounded-ffree-sti),
and [Stage 2 report](../../docs/reports/STAGE_2_REPORT.md#s2-30br--bounded-ffree-sti).

S2-30m adds bare `DE E0+i` FSUBRP through the existing exact same-exponent
subtraction gate: write old `ST(0)−ST(i)` to old physical `(TOP+i)&7`, then
empty old TOP and increment it. The decoded `i=0` alias remains Unsupported
for exact cancellation. Native x87 admission and general x87 semantics remain
closed; details and validation limits are in the [CPU contract](../../docs/specs/CPU_CONTRACT.md),
[fixture notes](../../tests/fixtures/x86/stage2/README.md), and
[Stage 2 report](../../docs/reports/STAGE_2_REPORT.md).

S2-30s adds exact-only `DE F0+i` FDIVRP: old `ST(0)/ST(i)` is written to old
physical `(TOP+i)&7`, then old TOP is emptied and TOP advances. The `i=0`
alias writes exact `+1` before emptying its only tag. Inexact quotients,
non-Normal inputs/results and invalid x87 environment/tags reject atomically;
native x87 admission remains closed. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#local-s2-30s-exact-only-fdivrp-sti-st0-de-f0i),
[fixture notes](../../tests/fixtures/x86/stage2/README.md) and
[Stage 2 report](../../docs/reports/STAGE_2_REPORT.md).

S2-30t adds exact-only bare `D8 F8+i` FDIVR: old `ST(i)/ST(0)` overwrites
old physical TOP without popping. The `i=0` alias writes exact `+1` while
retaining its tag and TOP. Inexact quotients, non-Normal inputs/results and
invalid x87 environment/tags reject atomically; native x87 admission remains
closed. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#local-s2-30t-exact-only-fdivr-st0-sti-d8-f8i),
[fixture notes](../../tests/fixtures/x86/stage2/README.md) and
[Stage 2 report](../../docs/reports/STAGE_2_REPORT.md).

S2-30u adds exact-only bare `DC F0+i` FDIVR: old `ST(0)/ST(i)` overwrites old
physical `(TOP+i)&7` without popping. The `i=0` alias writes exact `+1` while
retaining its tag and TOP. Inexact quotients, non-Normal inputs/results and
invalid x87 environment/tags reject atomically; native x87 admission remains
closed. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#local-s2-30u-exact-only-fdivr-sti-st0-dc-f0i),
[fixture notes](../../tests/fixtures/x86/stage2/README.md) and
[Stage 2 report](../../docs/reports/STAGE_2_REPORT.md).

S2-30v adds exact-only bare `DC F8+i` FDIV: old `ST(i)/ST(0)` overwrites old
physical `(TOP+i)&7` without popping. The `i=0` alias writes exact `+1` while
retaining its tag and TOP. Inexact quotients, non-Normal inputs/results and
invalid x87 environment/tags reject atomically; native x87 admission remains
closed. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#local-s2-30v-exact-only-fdiv-sti-st0-dc-f8i),
[fixture notes](../../tests/fixtures/x86/stage2/README.md) and
[Stage 2 report](../../docs/reports/STAGE_2_REPORT.md).


S2-30w adds only bare `DD D8+i` FSTP ST(i): a Valid canonical-Normal old
ST(0) is copied byte-for-byte to old physical `(TOP+i)&7`, its destination tag
is set Valid, old TOP is emptied, and TOP advances. Old destination tag/payload
are ignored. The `i=0` alias ends with the copied payload and an Empty old TOP
tag; `i=1` makes the copied value new ST(0). C1 is retained by local policy.
Invalid source/environment and fetch/stop/budget boundaries preserve full state.
Memory forms, exception delivery and native x87 admission remain outside this
slice. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#local-s2-30w-bounded-fstp-sti-dd-d8i), [Stage 2 plan](../../docs/plans/STAGE_2.md#s2-30w-bare-bounded-fstp-sti-local-raw-copy-slice), and [fixture notes](../../tests/fixtures/x86/stage2/README.md).

S2-30x adds only bare `DD D0+i` FST ST(i): a Valid canonical-Normal old
ST(0) is copied byte-for-byte to old physical `(TOP+i)&7`, and that destination
tag is set Valid. TOP and the complete FSW are preserved, including for the
`i=0` alias; the old destination tag/payload is ignored. Invalid source or
environment and fetch/stop/budget boundaries preserve full state. Memory
forms, exception delivery and native x87 admission remain outside this slice.
See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#local-s2-30x-bounded-fst-sti-dd-d0i), [Stage 2 plan](../../docs/plans/STAGE_2.md#s2-30x-bare-bounded-fst-sti-local-raw-copy-slice), and [fixture notes](../../tests/fixtures/x86/stage2/README.md).

S2-30ar extends bare `DD D0+i` FST ST(i) to accept exact Zero-tagged signed
zero alongside the existing canonical-Normal Valid source. It copies the raw
bytes and preserves Zero at the physical destination without popping or
rotating; FSTP remains Normal-only. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#local-s2-30ar-canonical-signed-zero-fst-sti-dd-d0i), [Stage 2 plan](../../docs/plans/STAGE_2.md#s2-30ar-admit-canonical-signed-zero-to-bare-fst-sti-dd-d0i), and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30ar-signed-zero-fst-sti).

S2-30z adds bare `D9 E1` FABS through the same bounded canonical-Normal ST(0)
source gate: it clears only the raw sign bit, clears C1, records FIP and retires
once. Literal vectors cover every TOP, positive/negative Normal boundaries, C1
and retained status sentinels, idempotence, and atomic state/fetch/stop/budget
rejection. The schema and CLI include a success fixture and a real
no-stop/budget-one source-tag rejection. Native x87 admission explicitly denies
FABS; full x87 semantics remain open. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#local-s2-30z-bounded-fabs-d9-e1), [fixture notes](../../tests/fixtures/x86/stage2/README.md), and [Stage 2 report](../../docs/reports/STAGE_2_REPORT.md).

S2-30ak adds bare `DF F1` FCOMIP for canonical-Normal ST(0)/ST(1): it maps
greater/less/equal to RFLAGS, defines all six affected bits, clears only C1,
empties the actual old physical TOP tag and advances TOP once. It preserves
the old ST(1) tag and every register payload. Exact `dff1`/`dff1dfe0` fixtures,
an invalid old ST(1)-tag reject, exhaustive DF decoding, TOP/relation/status
coverage and all 64 RFLAGS masks are covered; native x87 admission explicitly
denies FCOMIP. Exceptions and non-Normal operands remain outside the bounded
profile. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#local-s2-30ak-bounded-fcomip-st0-st1-df-f1), [fixture notes](../../tests/fixtures/x86/stage2/README.md), and [Stage 2 report](../../docs/reports/STAGE_2_REPORT.md).

S2-30am adds bare `DF E9` FUCOMIP for canonical-Normal ST(0)/ST(1), with the
same ordered RFLAGS mapping and definedness as FCOMIP, clearing only C1,
emptying old physical TOP and advancing TOP once. Matrices cover all TOP,
relations, initial condition fields and 64 affected-flag masks; exact
`dfe9dfe0`/`dfe9` fixtures include a TOP=5 invalid actual-ST(1) tag reject.
Native x87 admission explicitly denies FUCOMIP; unordered/special inputs and
exception delivery remain outside this bounded profile. See the [CPU
contract](../../docs/specs/CPU_CONTRACT.md#local-s2-30am-bounded-fucomip-st0-st1-df-e9), [fixture notes](../../tests/fixtures/x86/stage2/README.md), and [Stage 2 report](../../docs/reports/STAGE_2_REPORT.md#s2-30am--bounded-fucomip-st0-st1-to-rflags-and-pop).

S2-30an generalizes FCOMI, FUCOMI, FCOMIP and FUCOMIP from `i=1` to all
indexed register forms. The decoder carries `source:u8`; self compares only
require/read old TOP, while nonzero indices use physical `(TOP+i)&7`. All four
families define six RFLAGS bits and clear C1; the pop forms empty actual old
TOP and advance once, including `i=0`. Exact DB/DF, prefix, schema, native
deny and literal boundary coverage are recorded in the [CPU contract](../../docs/specs/CPU_CONTRACT.md#local-s2-30an-indexed-bounded-fcomi-family-register-forms), [fixture notes](../../tests/fixtures/x86/stage2/README.md), and [Stage 2 report](../../docs/reports/STAGE_2_REPORT.md).

S2-31p adds exact bare `DF 28` FILD m64int `[RAX]` in the bounded Long64 x87
profile. It performs one canonical48-checked Qword read, converts signed i64
exactly to binary80 using integer operations, then commits the push after a
successful read. The shared packer also serves the existing m16/m32 FILD
forms. Native x87 admission and broader architectural exception behavior remain
outside this slice; see the [CPU contract](../../docs/specs/CPU_CONTRACT.md#local-s2-31p-bounded-fild-m64int-rax-df-28),
[plan](../../docs/plans/STAGE_2.md), and [report](../../docs/reports/STAGE_2_REPORT.md).

S2-31ee extends only the existing exact bare `DF 00`, `DB 00` and `DF 28` FILD memory gates to admit any history of IE/DE/ZE/OE/UE/PE. The shared preflight still rejects SF (including SF+IE), ES and B before reading, while successful integer reads preserve all six sticky flags and clear C1. Independent literal matrices cover all 64 histories × 8 TOP × 4 FCW × both previous C1 states at every width; native/JIT admission stays closed. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-31ee--historical-sticky-flags-on-exact-bounded-fild-memory-forms), [plan](../../docs/plans/STAGE_2.md#s2-31ee--exact-fild-historical-sticky-flag-admission), and [report](../../docs/reports/S2_31EE_FILD_HISTORICAL_STICKY_FLAGS.md).

S2-31q adds exact bare `DB 38` (`FSTP m80fp [RAX]`) for a Valid
canonical-Normal binary80 ST(0). It validates the complete ten-byte
canonical48 range, performs one raw slice write without a data read, then pops
TOP and clears C1 only after success. Exact `DB 38`/`DB 38 DF E0` fixtures,
atomicity checks and explicit native denial are documented in the [CPU
contract](../../docs/specs/CPU_CONTRACT.md#local-s2-31q-bounded-fstp-m80fp-rax-db-38).


S2-31r adds exact bare `DB 28` (`FLD m80fp [RAX]`) in Long64. It checks an
Empty physical destination and bounded x87 environment before one canonical48
checked ten-byte read, then accepts only canonical-Normal binary80 and copies
its raw bytes exactly while pushing TOP. Invalid classes reject after that read
without CPU commit. PE and unrelated state are preserved, C1 clears, and C0/C2/C3
retention follows the existing local FILD policy. `DB 30` and native x87
admission remain unsupported; the broader S2-31 exception and physical scopes
remain open. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#local-s2-31r-bounded-fld-m80fp-rax-db-28),
[fixtures](../../tests/fixtures/x86/stage2/README.md), and
[Stage 2 report](../../docs/reports/STAGE_2_REPORT.md).

S2-31x extends that same exact `DB 28` path to canonical nonzero binary80
subnormals. It copies all ten bytes unchanged and pushes with `X87Tag::Special`;
Normal and signed-zero behavior retain their Valid and Zero tags. Pseudo-
denormals and other unsupported classes reject after the single read without
commit. The current bounded gates and status policy remain unchanged, native
x87 admission stays closed, and the SDM m80fp form specifies no `#D`. See the
[CPU contract](../../docs/specs/CPU_CONTRACT.md#local-s2-31x-bounded-fld-m80fp-rax-db-28-canonical-subnormal-extension),
[fixtures](../../tests/fixtures/x86/stage2/README.md), and
[Stage 2 report](../../docs/reports/STAGE_2_REPORT.md#s2-31x--bounded-fld-m80fp-canonical-subnormal-extension).

S2-30bk generalizes the earlier bounded bare `D9 C9` exchange to exact
`D9 C8+i` for `i=0..7`. It preflights physical TOP and `(TOP+i)&7`, except
self-exchange checks TOP once, then swaps the complete ten-byte values while
preserving tags/TOP and the local condition-code policy. The unit matrix covers
14,336 indexed and 1,024 self-exchange states plus atomic rejects and execution
boundaries. Empty/non-Normal participants, stack exceptions, and JIT/native
acceptance remain outside this slice; see the [CPU contract](../../docs/specs/CPU_CONTRACT.md#local-s2-30bk-bounded-bare-fxch-sti-d9-c8i).

S2-31t added exact bare `DD 00` (`FLD m64fp [RAX]`) for Normal binary64 and
expanded it to binary80 using integer fields. S2-31u extends that same bounded
load to signed zero: +0 becomes ten zero bytes, -0 keeps its sign in the last
byte, and both receive the Zero tag. Normal remains unchanged; nonzero
subnormals, infinity and NaNs still reject after one Qword read without CPU
commit. Native x87 admission remains closed, and physical acceptance remains
part of the combined final S2-31 substage. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#local-s2-31u-bounded-fld-m64fp-rax-dd-00-signed-zero-extension), [fixtures](../../tests/fixtures/x86/stage2/README.md), and [Stage 2 report](../../docs/reports/STAGE_2_REPORT.md#s2-31u--bounded-fld-m64fp-signed-zero-extension).


S2-31an extends exact `DB 38` to a Zero-tagged canonical binary80 zero. It
preserves all ten bytes (including sign), keeps the one-write/no-read and
post-success pop rules, and leaves native/JIT admission closed; see the [CPU
contract](../../docs/specs/CPU_CONTRACT.md#local-s2-31an-signed-zero-fstp-m80fp-rax-db-38).


S2-31ao also admits canonical binary80 infinity with the Special tag for exact
`DB 38`, preserving its raw sign and ten-byte payload. The prior Normal and
signed-zero pairs are unchanged; malformed classes reject before access. See the
[CPU contract](../../docs/specs/CPU_CONTRACT.md#local-s2-31ao-signed-infinity-fstp-m80fp-rax-db-38).


S2-31ap extends exact DD10/DD18 m64 stores to Special-tagged canonical
binary80 infinity, using integer fields to emit signed binary64 infinity with
no new flags. Qword write ordering, FST/FSTP stack behavior, and native/JIT
denial remain bounded. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#local-s2-31ap-signed-infinity-fstfstp-m64fp-rax).


Bounded `FST/FSTP m32fp` (`D9 10`/`D9 18`) also stores canonical signed
binary80 infinity from a Special-tagged TOP as exact binary32 infinity. The
one-write/no-read and post-success-pop rules remain; native/JIT admission stays
closed.


Bounded `FSTP m80fp` (`DB 38`) also preserves canonical binary80 QuietNaNs
byte-for-byte when physical TOP is Special-tagged. It adds no flags and pops
only after the atomic ten-byte write; native/JIT admission stays closed.

Exact `DB 38` also accepts Special-tagged canonical binary80 SignalingNaNs in
the bounded S2-31as raw-store path. It preserves all ten bytes, including the
signaling bit and payload, and adds no status flags. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#local-s2-31as-canonical-signalingnan-fstp-m80fp-rax-db-38).

Exact `DB 38` also accepts Special-tagged canonical binary80 subnormals in the
bounded S2-31at raw-store path. It preserves sign and fraction bytes and adds
no flags. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#local-s2-31at-canonical-subnormal-fstp-m80fp-rax-db-38).

S2-30bs adds exact unprefixed Long64 `D9 D0` FNOP to the local x87 interpreter
profile. With present x87 state, FCW `0x037f`, and clean `FSW & 0x80ff`, it
updates only FIP and retires; stack tags and payloads are opaque. JIT/native
and physical x86 execution remain unsupported. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#local-s2-30bs-bounded-bare-fnop-d9-d0), [plan](../../docs/plans/STAGE_2.md#s2-30bs-bare-fnop-with-a-bounded-environment-gate), and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30bs-bounded-fnop).

S2-30bw adds exact bare `DB C1` FCMOVNB ST(0), ST(1), using the existing
bounded FCMOV operand gate and raw 10-byte/tag copy when CF is clear. Both
operands are preflighted before CF is observed. The 51,200 success matrix, the
21,504 invalid class/tag matrix, exact decoder neighbors and execution-boundary
checks are local interpreter evidence. Native/JIT remain closed.

S2-30bx extends FCMOVNB to exact bare `DB C0..C7`, matching the indexed source
and self-index rules already used by FCMOVB. Nonzero indices preflight TOP and
the indexed source before CF; self-index checks TOP only. CF=0 copies raw bytes
and tag, while CF=1 preserves them. The 363,520 success and 161,280 reject
snapshots, indexed schema streams, literal CLI fixtures and JIT denial are
local-only evidence. The former C1-only extent is historical; native/JIT remain
closed. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#local-s2-30bx-bounded-indexed-fcmovnb-st0-sti-db-c0c7),
[plan](../../docs/plans/STAGE_2.md#s2-30bx-indexed-bare-fcmovnb-st0-sti), and
[fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30bx-bounded-indexed-fcmovnb-sti).

Historical S2-30by adds exact bare `DA C9` FCMOVE ST(0), ST(1), now included
in the indexed `DA C8..CF` slice above. It reuses the strict
bounded operand preflight and copies raw binary80 bytes plus tag only when
integer RFLAGS.ZF is set. Both operands are checked before consulting ZF;
FCW/status gates and C1 policy match the neighboring bounded FCMOV slices.
The 51,200 success and 21,504 invalid class/tag matrices, actual FCOMI/FCHS
producer steps, exact-stream decoding and rejection boundaries remain local
interpreter evidence. JIT/native and physical x86 execution remain closed.

S2-30cb extends FCMOVNE to exact bare `DB C8..CF`, using `ModR/M & 7` as the
source index and the same strict canonical-Normal/Zero operand gate. Nonzero
indices preflight TOP and `(TOP+i)&7` before ZF; self-index checks TOP only.
ZF=0 copies the source's exact raw bytes and tag. The 363,520 success and
161,280 invalid-pair matrices, self-index canaries, environment/status/boundary
checks, and FCMOVE complement property pass locally. JIT/native remain closed.

S2-30cc recorded the historical exact bare Long64 `DA D1` FCMOVBE slice; its
ST(1)-only scope is superseded by indexed S2-30cd. The indexed decoder admits
only bare `DA D0..D7` and carries `ModR/M & 7` as the source index. It preflights
TOP and `(TOP+i)&7` before integer flags when i>0, and checks only TOP for the
self-index case. Exact raw bytes/tag transfer occurs when `CF || ZF`. Local
full-state matrices cover 363,520 successes and 645,120 invalid class/tag
rejects, plus environment, boundary, indexed fixture, and exact-schema checks.
JIT, native and physical x86 execution remain unverified.

S2-30ce implements only bare Long64 `DB D1` FCMOVNBE ST(0), ST(1). It
preflights canonical Normal/Zero operands before integer CF/ZF and copies raw
bytes plus tag only when both flags are clear. The 51,200-success and
86,016-invalid matrices, complement property, producer checkpoints and repeat
intermediate states are local interpreter evidence; JIT/native and physical
x86 remain outside the slice.

S2-30ci adds only exact bare `DB D9` FCMOVNU ST(0), ST(1) in the local
Long64 interpreter. Its strict environment and both-operand preflight copy raw
binary80 bytes/tag only when integer PF is clear. Other FCMOVNU indices and
unordered/NaN behavior remain unsupported; native and physical behavior are
not claimed. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#local-s2-30ci-exact-bare-fcmovnu-st0-st1-db-d9) and [Stage 2 report](../../docs/reports/STAGE_2_REPORT.md#s2-30ci--exact-bare-fcmovnu-st0-st1).

S2-19d adds bounded REP MOVSB/W/D/Q with exact F3-prefix admission and
source-read-then-destination-write progress. Both indices and RCX advance only
after a complete transfer; STOP, errors and the local 1024-transfer limit
preserve earlier successful pairs and leave the REP-start RIP. This is local
interpreter policy; JIT/native and physical execution remain unverified. See
the [CPU contract](../../docs/specs/CPU_CONTRACT.md#local-s2-19d-bounded-rep-movsbwdq-contract),
[plan](../../docs/plans/STAGE_2.md#s2-19d--f3-a4a5-rep-movsbwdq-ограниченный-локальный-контракт),
and [fixtures](../../tests/fixtures/x86/stage2/README.md#bounded-rep-movs-bwdq-fixtures-s2-19d).

S2-19g extends the existing bounded REPE SCASB interpreter slice to SCASW/D/Q
with normalized F3/66/REX.W prefix handling. The arithmetic value/definedness
entry gate, read-then-commit progress, STOP/replay and 1024-iteration cap stay
local profile conditions; JIT/native and physical execution remain unverified.
See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#local-s2-19g-bounded-f3-af-repe-scaswdq-contract),
[interpreter coverage](../../docs/testing/INTERPRETER.md#repe-scaswdq--s2-19g-local-interpreter-coverage),
and [fixtures](../../tests/fixtures/x86/stage2/README.md#bounded-repe-scaswdq-fixtures-s2-19g).


S2-19s adds a bounded Long64 REPNE SCASW/D/Q addr32 slice through the explicit
`run_with_rep_continuation` API. It performs one contiguous width read from
zero-extended EDI, computes accumulator-minus-memory CMP, and commits flags,
32-bit EDI and ECX only after successful reads. The opaque token validates kind,
width, address size and instruction length across resumes. Existing F2 SCASB and
F3 behavior remain unchanged; old entry APIs and JIT/native admission stay
closed. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md), [interpreter
notes](../../docs/testing/INTERPRETER.md), and [Stage 2 report](../../docs/reports/STAGE_2_REPORT.md).

S2-34.1 adds an independent optional `SseState` alongside `X87State` in
`LongModeState`: 16 XMM registers of raw 16-byte values and raw u32 MXCSR. The
C fragment adapter rejects `Some(sse)`. This is local snapshot/state storage
only; SSE execution and native/JIT support are not claimed. See the
[CPU contract](../../docs/specs/CPU_CONTRACT.md#local-profile-s2-341-s2-long-x87-sse-state-v1).

S2-34.2 locally adds only legacy register-only `0F 10 /r` MOVUPS under the
separate `s2-long-movups-reg-v1` profile. One optional effective REX can extend
ModR/M.reg/rm to XMM0–XMM15; memory forms, extra prefixes, `0F 11`, SSE2
variants and native/JIT are excluded. The operation copies raw 128-bit state
and preserves x87/MXCSR/GPR/RFLAGS. It assumes SSE availability and the stated
CR0/CR4 preconditions without implementing control state. See the [CPU
contract](../../docs/specs/CPU_CONTRACT.md#local-profile-s2-342-s2-long-movups-reg-v1).

S2-34.4 adds a separate local-only legacy MOVUPS load profile for exact
`0F 10 /r`, ModR/M.mod=00 and r/m=000 (`[RAX]`), with optional REX.R for
XMM8–15. It validates the canonical 16-byte range and issues one raw
`GuestMemory::read` before committing the destination XMM and RIP. Unaligned
access is allowed; stores, other addressing forms, native/JIT and physical
execution remain outside the slice. The profile's SSE and CR0/CR4 conditions
are assumptions, not implemented gates, and it is one form rather than a full
Generic amd64 profile. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#local-profile-s2-344-s2-long-movups-mem-load-v1).

S2-34.6a adds separate local-only `s2-long-movups-mem-base-v1` coverage for
both legacy memory directions with ModR/M modes 00/01/10, base plus signed
disp8/disp32, REX.R XMM extension and REX.B GPR-base extension. Decoding reuses
the existing `rm_operand` and `MemoryOperand`; execution validates a raw
16-byte range without widening `OperandSize`. SIB, RIP-relative, REX.W/X,
address-size 32, prefixes, VEX/EVEX and native/JIT remain excluded. The prior
state, register and `[RAX]` memory profiles keep their exact admissions. See
the [CPU contract](../../docs/specs/CPU_CONTRACT.md#local-profile-s2-346a-s2-long-movups-mem-base-v1).

S2-34.6b extends the same profile with legacy `0F 10 /r` and `0F 11 /r`
ModR/M.mod=00/rm=101 RIP-relative disp32 forms. It sign-extends disp32 and
uses complete next RIP through `MemoryOperand::NextRip`; REX.R still selects
XMM8–15. Intel SDM 093 Vol. 2A §2.2.1.6 confirms REX.B does not select R13
for this encoding. The bounded profile rejects REX.B=1 on this form. SIB,
segment/address/mandatory prefixes, VEX/EVEX and native/JIT remain excluded.
The earlier base+displacement forms and other profile admissions stay intact.
See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-346b-rip-relative-extension).

S2-35.5 adds local-only register `PSUBUSB` for `66 [one optional REX.R/B] 0F D8 /r` with ModR/M.mod=3. Each XMM byte lane independently computes `max(unsigned(DEST) - unsigned(SRC), 0)`. This is a narrow SSE2 diagnostic slice: no MMX, memory, other widths, prefixes, REX.W/X, VEX/EVEX, native/JIT, or physical execution. SSE2 and CR0/CR4 conditions are assumptions only. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#local-register-only-psubusb-profile--s2-355).

S2-35.6 adds local-only register `PADDW` for `66 [one optional REX.R/B] 0F FD /r` with ModR/M.mod=3. Eight independent little-endian u16 lanes add modulo 65536. MMX, memory, other widths, saturation, other prefixes, REX.W/X, VEX/EVEX, JIT/native and physical execution remain excluded. SSE2 and CR0/CR4 conditions are assumptions only. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#local-register-only-paddw-profile--s2-356).

S2-35.7 adds local-only register `PSUBW` for `66 [one optional REX.R/B] 0F F9 /r` with ModR/M.mod=3. Eight independent little-endian u16 lanes subtract modulo 65536. MMX, memory, other widths, saturation, other prefixes, REX.W/X, VEX/EVEX, JIT/native and physical execution remain excluded. SSE2 and CR0/CR4 conditions are assumptions only. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#local-register-only-psubw-profile--s2-357).

S2-35.8 adds local-only register `PADDUSW` for `66 [one optional REX.R/B] 0F DD /r` with ModR/M.mod=3. Eight independent little-endian u16 lanes add with unsigned saturation at 65535. MMX, memory, other widths, signed saturation, other prefixes, REX.W/X, VEX/EVEX, JIT/native and physical execution remain excluded. SSE2 and CR0/CR4 conditions are assumptions only. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#local-register-only-paddusw-profile--s2-358).

S2-35.9 adds local-only register `PSUBUSW` for `66 [one optional REX.R/B] 0F D9 /r` with ModR/M.mod=3. Eight independent little-endian u16 lanes compute `max(DEST - SRC, 0)`. MMX, memory, other widths, signed saturation, other prefixes, REX.W/X, VEX/EVEX, JIT/native and physical execution remain excluded. SSE2 and CR0/CR4 conditions are assumptions only. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#local-register-only-psubusw-profile--s2-359).

S2-35.10 adds local-only register `PADDSB` for `66 [one optional REX.R/B] 0F EC /r` with ModR/M.mod=3. Sixteen independent signed i8 lanes add with saturation to [-128, 127], storing the raw result bytes. MMX, wider forms, subtraction, memory, other prefixes, REX.W/X, VEX/EVEX, JIT/native and physical execution remain excluded. S2-35.12 adds local-only register `PADDSW` for `66 [one optional REX.R/B] 0F ED /r`, ModR/M.mod=3; eight independent signed i16 lanes add using widened intermediates and saturate to [-32768, 32767]. SSE2 and CR0/CR4 conditions are assumptions only. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#local-register-only-paddsb-profile--s2-3510).

S2-35.14 adds local-only register PADDD for `66 [one optional REX.R/B] 0F FE /r`, ModR/M.mod=3. Four little-endian u32 lanes add modulo 2^32 independently; the bounded profile excludes MMX, other widths, saturation, memory, extra prefixes, REX.W/X, VEX/EVEX and native/JIT execution.

S2-37e adds local-only register MINSS profile `s2-long-minss-reg-masked-v1`.
With all MXCSR exception masks set, DAZ/FTZ clear and reserved bits clear, it
selects operand 2 for either NaN and accumulates masked IE/DE sticky bits for
NaN/subnormal inputs. It preserves upper destination bits and unrelated state;
JIT/native remain closed. Full unmasked FP exception delivery and fault ordering
remain open. Normative source: Intel SDM Rev. 093 Vol. 1 §11.5, Appendix D
Tables D-9/D-15, and Vol. 2B MINSS. See [CPU contract](../../docs/specs/CPU_CONTRACT.md#local-s2-37e-register-only-minss-with-masked-status).

S2-37f adds local-only register MAXSS profile
`s2-long-maxss-reg-masked-v1`, reusing the same masked-status constraints.
Either NaN selects the raw RHS and sets IE; any subnormal operand sets DE;
these sticky bits accumulate independently. The result and non-status state
follow `Binary32::max_sse_raw_result`; unmasked exceptions, fault ordering,
JIT/native and physical admission remain open. Normative source: Intel SDM
Rev. 093 Vol. 1 §11.5, Appendix D Tables D-9/D-13/D-15 and Vol. 2B MAXSS.
See [CPU contract](../../docs/specs/CPU_CONTRACT.md#local-s2-37f-register-only-maxss-with-masked-status).

S2-38n adds the separate interpreter-only profile
`s2-long-maxss-memory-unmasked-rax-v1` for exact `F3 0F 5F 00`, MAXSS
XMM0,m32 `[RAX]`. It reads exactly four bytes after canonical48 span checks,
then applies MAXSS-specific QNaN/SNaN, subnormal, source-2, and mixed IM/DM
semantics. New unmasked exceptions suppress XMM0 and retirement while
committing required sticky flags. The S2-37cy masked memory profile, S2-38f
register profile, and S2-38m MINSS memory profile remain unchanged. Native,
JIT, generic, C ABI and fragment paths remain closed. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-38n--exact-maxss-unmasked-memory-exit-for-m32-rax).

S2-37g adds local-only packed register MINPS profile
`s2-long-minps-reg-masked-v1` for bare or optional-REX `0F 5D /r` in register
form. It snapshots both XMM operands and updates four little-endian binary32
lanes; masked IE/DE conditions are ORed across lanes before the full destination
and sticky-status commit. It uses the same all-masks-set, DAZ/FTZ-clear,
reserved-bits-clear preconditions as MINSS/MAXSS. Unmasked exceptions, memory,
JIT/native and physical execution remain closed. See [CPU contract](../../docs/specs/CPU_CONTRACT.md#local-s2-37g-register-only-minps-with-masked-status).

S2-37h adds local-only packed register MAXPS profile
`s2-long-maxps-reg-masked-v1` for bare or optional-REX `0F 5F /r` in register
form. It snapshots both XMM operands and updates four little-endian binary32
lanes; masked IE/DE conditions are ORed across lanes before the full destination
and sticky-status commit. It uses the same all-masks-set, DAZ/FTZ-clear,
reserved-bits-clear preconditions as MINPS. Unmasked exceptions, memory,
JIT/native and physical execution remain closed. See [CPU contract](../../docs/specs/CPU_CONTRACT.md#local-s2-37h-register-only-maxps-with-masked-status).


S2-37i adds local-only register MINSD operation `MinsdRegisterMasked` for
`F2 [optional adjacent REX.R/B] 0F 5D /r`. Execution uses raw Binary64 MIN
selection on the low qwords only, accumulates masked IE/DE, and commits only
the destination low qword plus sticky status while preserving the upper qword
and unrelated state. Upper-qword sentinels do not contribute flags. F2 is now
globally decoded as MINSD; MINSS/MINPS profile validation remains prefix-exact.
JIT/native/CPUID remain closed. The bounded behavior is documented in the
[CPU contract](../../docs/specs/CPU_CONTRACT.md#local-s2-37i-register-only-minsd-with-masked-status).

S2-37aw adds bounded interpreter-only Long64 `DIVPS XMM,XMM` register pairs
(`NP [REX.R/B] 0F 5E /r`). It executes four raw Binary32 lanes with MXCSR
rounding, WPR/FTZ handling, and aggregate pre/post exception flags. The strict
schema-v2 profile, independent raw-rational oracle, tests, and scope limits are
recorded in the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37aw--bounded-masked-range-long64-divps-xmm-register-pairs)
and [Stage 2 report](../../docs/reports/STAGE_2_REPORT.md#s2-37aw--divps-register-pair-interpreter-profile).
Memory forms, JIT/native execution, and physical acceptance remain outside this
profile.

S2-37ax adds bounded interpreter-only Long64 `DIVSD XMM,XMM` register pairs
(`F2 [REX.R/B] 0F 5E /r`). It computes the low binary64 lane from operand
snapshots while preserving destination.high64 and ignoring source.high64. The
strict schema-v2 profile, independent raw-rational oracle, tests and scope
limits are recorded in the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37ax--bounded-masked-range-long64-divsd-xmm-register-pairs),
[Stage 2 plan](../../docs/plans/STAGE_2.md#s2-37ax--canonical-long64-divsd-xmm-register-pairs)
and [report](../../docs/reports/STAGE_2_REPORT.md#s2-37ax--divsd-register-pair-interpreter-profile).

S2-37bb adds strict interpreter-only Long64 register ADDSD under the
schema-v2 `s2-long-addsd-registers-v1` profile. It snapshots both XMM sources,
adds only low64, preserves destination high64, and processes MXCSR rounding and
SIMD exceptions with full-fetch SSE2/CR0/CR4 gates. Memory forms and JIT/native
execution remain closed. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37bb--canonical-long64-addsd-xmm-register-pairs)
and [Stage 2 report](../../docs/reports/STAGE_2_REPORT.md#s2-37bb--addsd-register-pair-interpreter-profile).

S2-37bd adds the strict interpreter-only Long64 CVTPD2PS register-pair profile
`s2-long-cvtpd2ps-registers-v1`. It narrows two snapshotted binary64 source lanes,
zeroes the destination upper qword only on success, and applies MXCSR pre/post
event ordering with explicit SSE2/CR0/CR4 gates. The schema oracle is independent
integer code. Memory forms and JIT/native/physical acceptance remain excluded.
See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37bd--canonical-long64-cvtpd2ps-xmm-register-pairs)
and [Stage 2 report](../../docs/reports/STAGE_2_REPORT.md#s2-37bd--cvtpd2ps-register-pair-interpreter-profile).


S2-37be adds local interpreter-only legacy `CVTSD2SS` register pairs. It
snapshots source.low64, narrows using MXCSR.RC, writes only destination.low32,
and preserves high96. The profile implements bounded IE/DE/OE/UE/PE status, FTZ,
SSE2 gates and atomic SIMD faults; memory/JIT/native/physical execution remain
outside scope. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37be--canonical-long64-cvtsd2ss-xmm-register-pairs)
and [stage report](../../docs/reports/STAGE_2_REPORT.md#s2-37be--cvtsd2ss-register-pair-interpreter-profile).

S2-37bh adds a bounded Long64 `0F 2C C1` CVTTPS2PI MM0,XMM1 interpreter
profile. It truncates two binary32 lanes independently, records masked IE/PE,
and writes the result into physical x87 register 0 with the documented MMX tag/TOP
transition. The profile requires supported MXCSR with IM=PM=1 and an x87 snapshot
without pending exceptions; unrelated XMM/x87 state is preserved. Other encodings
and JIT/native paths remain closed. See the Stage 2 plan, CPU contract and report.


S2-37bj adds the exact Long64 `0F 2D C1` CVTPS2PI MM0,XMM1 interpreter
profile. It rounds two binary32 lanes using MXCSR.RC, aggregates masked IE/PE,
and performs the x87/MMX physical slot-0/TOP/tag transition. The profile has
strict SSE/CR0/CR4 gates and requires supported MXCSR plus a clear x87 pending
exception state. Other encodings and JIT/native paths remain closed. See the
[Stage 2 plan](../../docs/plans/STAGE_2.md#s2-37bj--bounded-long64-cvtps2pi-mm0xmm1-masked-register-profile),
[CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37bj--bounded-long64-cvtps2pi-mm0xmm1-masked-register-profile),
and [report](../../docs/reports/STAGE_2_REPORT.md#s2-37bj--cvtps2pi-mm0xmm1-masked-register-profile).

S2-37bk adds the exact Long64 `66 0F 2D C1` CVTPD2PI MM0,XMM1 interpreter
profile. It rounds two binary64 lanes with MXCSR.RC and an integer-only
conversion, aggregates masked IE/PE, and performs the physical x87/MMX
slot-0/TOP/tag transition. It uses a separate SSE2 availability gate; native,
JIT and broader operand forms remain closed. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37bk--bounded-long64-cvtpd2pi-mm0xmm1-masked-register-profile)
and [Stage 2 report](../../docs/reports/STAGE_2_REPORT.md#s2-37bk--cvtpd2pi-mm0xmm1-masked-register-profile).

S2-37bl adds the exact Long64 `66 0F 2C C1` CVTTPD2PI MM0,XMM1
interpreter profile. It truncates both binary64 lanes toward zero regardless
of MXCSR.RC, records masked IE and valid-lane PE, and performs the x87/MMX
slot-0/TOP/tag transition. SSE2/CR0/CR4 gates and the no-pending-x87
precondition are explicit; JIT/native and wider forms remain closed. See the
[CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37bl--bounded-long64-cvttpd2pi-mm0xmm1-masked-register-profile)
and [Stage 2 report](../../docs/reports/STAGE_2_REPORT.md#s2-37bl--cvttpd2pi-mm0xmm1-masked-register-profile).

S2-37bm adds exact Long64 register-source `66 0F 2A C1` CVTPI2PD XMM0,MM1.
It reads signed i32 lanes from physical x87 slot 1/MM1 independent of TOP,
writes exact binary64 XMM0, clears TOP and sets all tags valid while preserving
x87 payloads and MXCSR. Pending x87 states are refused. JIT/native and broader
forms remain closed; the memory-source profile stays separate.

S2-37bn adds exact `0F 2A C1` CVTPI2PS XMM0,MM1 with an SSE (not SSE2) gate.
The interpreter converts physical x87 slot 1 signed i32 lanes to binary32
under MXCSR.RC, writes XMM0.low64, ORs sticky PE for inexact lanes, and
transitions TOP/tags while preserving all x87 payloads and XMM0.high64.
Pending x87 state is refused; JIT/native and broader forms remain closed. See
the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37bn--bounded-long64-cvtpi2ps-xmm0mm1-register-profile).

S2-37bo adds exact Long64 `0F 2C 00` CVTTPS2PI MM0,[RAX] as a bounded
interpreter profile. It reads one canonical 8-byte source span with AC=0,
truncates binary32 lanes independent of MXCSR.RC, aggregates sticky IE/PE, and
performs the physical MM0/TOP/tag transition only after a successful read.
Pending x87 states and unsupported encodings are refused; JIT/native remain
closed. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37bo--bounded-long64-cvttps2pi-mm0rax-memory-profile)
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-37bo-cvttps2pi-mm0rax-memory-profile).

S2-37bp adds exact Long64 `0F 2D 00` CVTPS2PI MM0,[RAX] as a bounded
interpreter profile. It validates one canonical m64 source span with AC=0,
then performs one 8-byte read before any state mutation; unaligned reads are
accepted. Rounding follows MXCSR.RC; masked invalid/inexact lanes update IE/PE.
The SDM table and Operation pseudocode specify m64/SRC[63:0], which governs
despite the entry's conflicting 128-bit prose. Other backends remain closed.
See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37bp--bounded-long64-cvtps2pi-mm0rax-memory-profile)
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-37bp-cvtps2pi-mm0rax-memory-profile).

S2-37bq adds exact Long64 `66 0F 2C 00` CVTTPD2PI MM0,[RAX] as a bounded
interpreter profile. It validates canonical48 and 16-byte alignment, then
issues one m128 read before any CPU mutation. Misalignment raises #GP(0)
independent of CPL and AC; aligned AC=0/1 both work. Conversion truncates raw
binary64 lanes independent of RC and updates masked IE/PE. Successful reads
write physical MM0 and transition TOP/tags; read failures roll back state.
JIT/native and broader forms remain closed. See the [CPU
contract](../../docs/specs/CPU_CONTRACT.md#s2-37bq--bounded-long64-cvttpd2pi-mm0rax-aligned-m128-memory-profile)
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-37bq-cvttpd2pi-mm0rax-aligned-m128-memory-profile).

S2-37br adds exact interpreter-only Long64 `CVTPD2PI MM0,[RAX]` (`66 0F 2D 00`): one aligned canonical m128 read, MXCSR.RC rounding followed by signed-i32 range validation, masked IE/PE, and the MMX state transition. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37br--bounded-long64-cvtpd2pi-mm0rax-aligned-m128-memory-profile), [Stage 2 report](../../docs/reports/STAGE_2_REPORT.md#s2-37br--cvtpd2pi-mm0rax-aligned-m128-memory-profile), and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-37br-cvtpd2pi-mm0rax-aligned-m128-memory-profile).

S2-37bs adds exact interpreter-only legacy `CVTDQ2PD XMM0,[RAX]`
(`F3 0F E6 00`) under a separate profile. It reads one unaligned-capable m64
with AC=0 after canonical48 span validation, converts two signed i32 values to
binary64, replaces all XMM0 bits, and preserves MXCSR plus every x87 field,
including pending-state canaries. Read/gate failures do not partially update
CPU state. JIT, native and C ABI paths remain closed. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37bs--bounded-long64-cvtdq2pd-xmm0rax-m64-memory-profile), [Stage 2 report](../../docs/reports/STAGE_2_REPORT.md#s2-37bs--cvtdq2pd-xmm0rax-m64-memory-profile), and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-37bs-cvtdq2pd-xmm0rax-m64-memory-profile).

S2-37bt adds exact interpreter-only legacy `CVTDQ2PS XMM0,[RAX]`
(`0F 5B 00`) for one aligned 16-byte memory operand. It rounds four signed
integer lanes to binary32 under MXCSR.RC, aggregates masked PE, and preserves
prior MXCSR status plus all x87 state. The local profile requires PM=1 and
keeps JIT/native/C ABI closed. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37bt--bounded-long64-cvtdq2ps-xmm0rax-aligned-m128-memory-profile), [plan](../../docs/plans/STAGE_2.md#s2-37bt--bounded-long64-cvtdq2ps-xmm0rax-aligned-m128-memory-profile), and [report](../../docs/reports/STAGE_2_REPORT.md#s2-37bt--cvtdq2ps-xmm0rax-aligned-m128-memory-profile).

S2-37bu adds the exact interpreter-only legacy `CVTTPS2DQ XMM0,[RAX]`
(`F3 0F 5B 00`) m128 form. It requires an aligned 16-byte read, truncates four
raw binary32 lanes toward zero independent of MXCSR.RC, and preserves sticky
MXCSR flags while adding masked IE/PE. Pending x87 canaries pass through
unchanged. JIT/native/C ABI remain closed. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37bu--bounded-long64-cvttps2dq-xmm0rax-aligned-m128-memory-profile) and [report](../../docs/reports/STAGE_2_REPORT.md#s2-37bu--cvttps2dq-xmm0rax-aligned-m128-memory-profile).

### S2-37bv — CVTPS2DQ m128 memory profile

The exact legacy `66 0F 5B 00` `CVTPS2DQ XMM0,[RAX]` encoding has a separate
schema-v2 interpreter profile. It fetches four bytes before the local SSE2,
EM/OSFXSR and TS checks, then reads one aligned 16-byte span. It rounds binary32
lanes according to MXCSR.RC before range checking, aggregates masked IE/PE, and
commits XMM0/MXCSR only after the read succeeds. No x87 state is gated or
changed. JIT/native/C ABI remain closed; no physical x86 execution is claimed.

### S2-37bw — CVTTPD2DQ m128 memory profile

The exact legacy `66 0F E6 00` `CVTTPD2DQ XMM0,[RAX]` form has a separate
schema-v2 interpreter control path. It performs one checked aligned 16-byte
read, truncates two binary64 inputs toward zero, then checks signed-i32 range.
Masked IE/PE are sticky-ORed, and successful execution replaces all XMM0 bits
with two i32 lanes and a zero upper half. x87 state is not gated or mutated.
Native/JIT/C ABI remain closed. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37bw--bounded-long64-cvttpd2dq-xmm0rax-aligned-m128-profile).

### S2-37bx — CVTPD2DQ m128 memory profile

The exact legacy `F2 0F E6 00` Long64 profile performs one aligned 16-byte read,
then rounds two binary64 values with MXCSR.RC before signed-i32 range checking.
Masked invalid contributes IE, valid inexact contributes PE; native/JIT/C ABI
remain closed. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37bx--bounded-long64-cvtpd2dq-xmm0rax-aligned-m128-profile).

### S2-37by — CVTPS2PD m64 memory profile

The exact legacy `0F 5A 00` RAX form reads one checked m64 span, then widens
two binary32 lanes into all of XMM0. It records masked IE for SNaN quieting and
DE for nonzero subnormal widening; MXCSR RC/FTZ do not affect the result. x87
state is untouched and does not gate the operation. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37by--bounded-long64-cvtps2pd-xmm0rax-m64-memory-profile).


### S2-37bz — CVTSS2SD m32 memory profile

The exact `F3 0F 5A 00` scalar form reads four bytes from RAX once and writes
only the low 64 bits of XMM0. It widens binary32 exactly, adding masked IE for
SNaN quieting and DE for nonzero subnormal input; it preserves all upper XMM0
bits, MXCSR control/sticky state, and x87 canaries. RC/FTZ do not affect the
result. Native/JIT/C ABI remain closed. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37bz--bounded-long64-cvtss2sd-xmm0rax-m32-memory-profile).

### S2-37ca — CVTSD2SS m64 memory profile

The exact legacy `F2 0F 5A 00` form reads one unaligned-allowed m64 at RAX
under local AC=0 and writes only XMM0.low32. It rounds with MXCSR.RC and uses
WPR tininess before final binary32 exponent encoding; FTZ flushes all WPR-tiny
values with UE|PE, including exact tiny values. The bounded masked profile
requires IM/DM/OM/UM/PM=1 and DAZ=0. It preserves upper XMM0 and x87 state.
Missing SSE is refused after prerequisite gates and before data access.
Native/JIT/C ABI remain closed; see the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37ca--bounded-long64-cvtsd2ss-xmm0rax-m64-memory-profile).


### S2-37cb — CVTPD2PS m128 memory profile

The exact `66 0F 5A 00` interpreter profile reads aligned m128 once and narrows
two binary64 lanes with WPR tininess and FTZ, aggregating masked SIMD status.
It zeroes XMM0.high64 on success and leaves x87 unchanged; non-interpreter
backends remain closed. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37cb--bounded-long64-cvtpd2ps-xmm0rax-aligned-m128-memory-profile).

### S2-37cc — CVTSI2SD m32 memory form

The exact `F2 0F 2A 00` local profile reads four bytes at RAX and exactly widens
signed i32 into XMM0.low64. It preserves MXCSR and upper XMM0; the bounded
interpreter profile allows RC/FTZ/sticky/mask variation, requires DAZ=0 and AC=0,
and keeps native/JIT/C ABI admission closed. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37cc--bounded-long64-cvtsi2sd-xmm0dword-ptr-rax-m32-memory-profile).


### S2-37cd — CVTSI2SD m64 memory form

The exact `F2 48 0F 2A 00` interpreter profile reads one unaligned-allowed
eight-byte signed i64 at RAX, rounds directly to binary64 under MXCSR.RC, and
writes only XMM0.low64. It preserves XMM0.high64 and unrelated state; inexact
conversions set sticky PE. The bounded profile requires PM=1, DAZ=0, and AC=0,
and keeps native/JIT/C ABI admission closed. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37cd--bounded-long64-cvtsi2sd-xmm0qword-ptr-rax-m64-memory-profile).


### S2-37ce — CVTSI2SS m32 memory form

The exact legacy `F3 0F 2A 00` profile reads one signed i32 from `[RAX]` and
converts directly to binary32 under MXCSR.RC. It writes XMM0.low32, preserves
high96 and unrelated CPU/x87 state, and sets sticky PE only for inexact
conversion. It uses CPUID.SSE bit 25, a single m32 read, and local AC=0
admission. The strict schema-v2 profile remains interpreter-only. See the [CPU
contract](../../docs/specs/CPU_CONTRACT.md#s2-37ce--bounded-long64-cvtsi2ss-xmm0dword-ptr-rax-m32-memory-profile).

### S2-37cf — CVTSI2SS m64 memory form

The exact legacy `F3 48 0F 2A 00` interpreter profile reads one signed i64 and
converts directly to binary32 under MXCSR.RC. It writes XMM0.low32, preserves
high96 and unrelated state, and adds sticky PE only for inexact conversion.
The direct integer path preserves low source bits that an intermediate binary64
would lose. It uses SSE feature bit 25, allows unaligned reads at local AC=0,
and stays closed to generic/JIT/native/C execution. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37cf--bounded-long64-cvtsi2ss-xmm0qword-ptr-rax-m64-memory-profile).

### S2-37cj — CVTTSD2SI EAX,m64

The bounded executor profile accepts only exact legacy `F2 0F 2C 00`, performs
one eight-byte `[RAX]` read, truncates binary64 toward zero, then checks i32
range and commits EAX with zero-extension only after a successful read. It
preserves unrelated state and sticky MXCSR flags. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37cj--bounded-long64-cvttsd2si-eaxqword-ptr-rax-m64-to-r32-memory-profile).

### S2-37ck — CVTSD2SI RAX,m64 memory form

Exact `F2 48 0F 2D 00` reads one binary64 from the captured original RAX, rounds
with MXCSR.RC, and checks signed-i64 range before committing full-width RAX.
The local profile requires IM=PM=1 and DAZ=0, preserves sticky flags, permits
unaligned m64 only under AC=0, and remains closed to native/JIT/C ABI paths.
See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37ck--bounded-long64-cvtsd2si-raxqword-ptr-rax-m64-to-r64-memory-profile).


### S2-37cl — CVTSS2SI RAX,m32 memory profile

Exact `F3 48 0F 2D 00` is interpreter-only. It reads one dword at captured RAX, rounds binary32 under MXCSR.RC, range-checks signed i64, and writes full RAX; masked invalid adds IE, valid inexact adds PE. It requires SSE bit 25 and local IM=PM=1/DAZ=0 admission, and leaves native/JIT/generic routes closed.

### S2-37cm — CVTTSS2SI RAX,m32 memory profile

The bounded interpreter accepts exact legacy `F3 48 0F 2C 00`, captures the
original RAX, reads exactly four bytes, truncates binary32 toward zero, checks
signed-i64 range, and writes full RAX. Invalid inputs set sticky IE; valid
inexact inputs set PE. The profile uses SSE bit 25, requires IM=PM=1 and DAZ=0,
and keeps generic/JIT/native/C routes closed. Focused ISA checks passed 2/2
execution tests and 1/1 decoder test; no physical x86 acceptance is claimed.

### S2-37cn — CVTTSD2SI RAX,m64 memory profile

Exact legacy `F2 48 0F 2C 00` is a separate interpreter-only profile. It reads
one qword from captured RAX, truncates binary64 toward zero, checks signed-i64
range, then writes full RAX. Masked invalid adds sticky IE; valid inexact adds
PE. SSE2 bit 26, IM=PM=1, and DAZ=0 are local admission requirements. Native,
JIT, C, and generic execution remain closed.

S2-30cx extends exact bare `DC F8+i` FDIV to bounded FCW precision: old
`ST(i)/ST(0)` is rounded directly at PC24/53/64 and written to old physical
`(TOP+i)&7` without popping. Division has no exponent-gap cutoff; `i=0`
produces exact +1 after validating the sole used register. PE is sticky, C1
tracks the rounded magnitude increment, and the prior exact p64 quotient set
remains supported, including differing exponents. Invalid environment,
operand, and result states refuse atomically; native x87 admission remains
closed. See [CPU contract](../../docs/specs/CPU_CONTRACT.md#local-s2-30cx-bounded-fdiv-sti-st0-dc-f8-ff) and
[Stage 2 report](../../docs/reports/STAGE_2_REPORT.md#s2-30cx--bounded-fdiv-sti-st0).

S2-30cy extends exact bare `DC F0+i` FDIVR to bounded FCW precision: old
`ST(0)/ST(i)` is rounded directly at PC24/53/64 and written to old physical
`(TOP+i)&7` without popping. Division has no exponent-gap cutoff; `i=0`
produces exact +1 after validating the sole used register. PE is sticky, C1
tracks the rounded magnitude increment, and the prior exact p64 quotient set
remains supported, including differing exponents. Invalid environment,
operand, and result states refuse atomically; native x87 admission remains
closed. See [CPU contract](../../docs/specs/CPU_CONTRACT.md#local-s2-30cy-bounded-fdivr-sti-st0-dc-f0-f7) and
[Stage 2 report](../../docs/reports/STAGE_2_REPORT.md#s2-30cy--bounded-fdivr-sti-st0).

S2-30dj extends only bare `D8 E8+i` FSUBR to normalized Normal gaps through
32765. It computes `ST(i)-ST(0)` at requested FCW precision by flipping a
validated local ST(0) copy and calling the existing direct-addition helper.
The `DC` reverse and `DE` pop forms remain gap-capped; alias cancellation and
unsupported results refuse atomically. JIT/native and physical x86 acceptance
remain closed. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#local-s2-30dj-large-gap-fsubr-st0-sti-d8-e8-ef) and
[Stage 2 report](../../docs/reports/STAGE_2_REPORT.md#s2-30dj--large-gap-fsubr-st0-sti).

Stage 4 adds a separate `MovCr0WpLong64V1` CPU entry for exact bare
`MOV CR0,RAX` WP toggles. It shares the validated S4-12d commit logic but
excludes that profile's memory store form; it emits a typed context-change
effect and leaves machine cache invalidation to `od-machine`.

S4-14f adds the opt-in `InvlpgWpLong64V1` / `step_invlpg_wp` entry. It shares
the exact bare `0F 01 3B` decoder, privilege/fetch ordering and invalidation
effect with `step_invlpg`, while accepting CR0.WP=0 or 1. The original
`InvlpgLong64V1` keeps its WP=1 requirement.

S4-14g adds a separate `MovCr3WpLong64V1` / `step_mov_cr3_wp` entry for the
exact bare `0F 22 D8` form under CR0.WP=0 or 1. It reuses the same fetch,
privilege, reserved-bit, commit and `Cr3Write` behavior. The existing
`MovCr3Long64V1` / `step_mov_cr3` remains WP=1-only; other profiles are
unchanged.
