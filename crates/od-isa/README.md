# od-isa

Архитектурное состояние x86, собственный декодер и эталонный интерпретатор.

Не зависит от JIT, машины, host API и дискового формата. Данные CPU и семантика инструкций — здесь; исполнение ARM64 — в od-jit.

Статус на 22 октября 2026 года: decoder и reference interpreter исполняют ограниченный S0-09 из `docs/specs/CPU_CONTRACT.md` (`MOV r64, imm64`, register-only `ADD/SUB/CMP`, `JZ/JNZ`). Есть проверки branch boundaries, flags и instruction budget; этот профиль независимо сравнивается с физическим Intel и ARM64 JIT.

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
