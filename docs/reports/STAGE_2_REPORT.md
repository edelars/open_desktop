# Этап 2 — ход реализации

**Дата:** 23 октября 2026 года. Этап не принят; S2-00–14 приняты в указанных
границах, включая положительный physical slice S2-14.
ISSUE-011/012 остаются открытыми; наличие нового состояния или local instruction
slice не означает завершение этапа.

## S2-22e — CPL3 GDT-only VERW

Added the local-only `s2-long-verw-gdt-v1` profile and one exact decoder form,
bare `0F 00 E8` (`VERW AX`). GDTR base/limit are explicit, validated and
compared state. The interpreter performs null/TI/table-bound checks, reads one
complete 8-byte descriptor through descriptor-table access origin, and updates
only ZF based on writable-data type and CPL/RPL/DPL rights. It does not write
the descriptor or check present. A descriptor-table access retains CPL3 for
rights checks while using supervisor page privilege. Memory faults propagate
with no instruction commit or retirement. Native admission fails closed.

Eight literal fixtures cover writable, read-only, code, system, privilege
mismatch, exact-fit and short table limits, null selector, and RPL0 success.
The runner asserts full GPR preservation, RIP+3, one retirement, ZF, GDTR and
unchanged descriptor bytes. Unit tests additionally cover one exact 8-byte
read and context, no-read selector cases, no-commit backend/page faults,
profile required/null/width/canonical validation, legacy adapter rejection,
decoder neighbors/truncation and native admission guards.

Verification passed: `cargo test --workspace --offline` (344 tests),
`cargo check --workspace --all-targets --offline`, Python CPU suite and all
generator freshness tests (83 tests), the targeted VERW generator tests (2),
and `python3 tools/check_layout.py`. Linux x86_64 `cargo check --workspace
--all-targets --target x86_64-unknown-linux-gnu --offline` also passed as a
compile-only check; it emitted existing dead-code warnings in `od-host` and
`od-runner`. `cargo fmt --all -- --check` and `git diff --check` passed. No
physical/native execution was performed; Linux-gated native tests were not
executed on macOS. Independent Sol-medium review found and then verified a fix
for VERW ZF-definedness metadata; the re-review reported no remaining findings.
This slice does not complete S2-22 or S2-27.

Normative sources: Intel SDM revision 093, Vol.2C (326018-093US),
“VERR/VERW—Verify a Segment for Reading or Writing,” pp.5-162–5-163,
[official PDF](https://cdrdv2.intel.com/v1/dl/getContent/671072); Vol.3A
(325384-093US), §§3.5.1, 6.10.2, and 6.11.5,
[official combined PDF](https://cdrdv2.intel.com/v1/dl/getContent/671447).

```sh
python3 tests/cpu/generate_stage2_verw_gdt.py --check
python3 tests/cpu/test_stage2_verw_gdt_generator.py
cargo test -p od-isa --offline verw_
cargo test -p od-testkit --offline verw_gdt_profile
cargo test -p od-runner --test fixture_v2 verw_gdt_fixtures_check_literal_rights_flags_state_and_descriptor_bytes --offline
```

## S2-22f — CPL3 GDT-only `MOV FS, AX` (partial)

Added the local-only `s2-long-fs-load-v1` profile for exactly bare `8E E0`
(`MOV FS, AX`) at Long64/CPL3. It reads a non-null GDT selector from AX,
checks the entire 8-byte descriptor against GDTR limit, and makes exactly one
descriptor-origin read with CPL3 retained and Ring0 page privilege. Descriptor
type/readability and CPL/RPL/DPL are checked before P; type/privilege faults are
`#GP(selector)`, a valid non-present descriptor raises `#NP(selector)`, and a
valid present descriptor succeeds only when A=1. A=0, null and TI/LDT remain
Unsupported without descriptor writes. Successful loads expose the visible FS
selector, zero-extended descriptor base, effective limit and cache attributes;
other GPRs, RFLAGS, GDTR, GS and memory are retained.

The profile carries explicit initial hidden cache state and does not require it
to match the current GDT bytes. Old profiles omit the new cache fields. The
legacy adapter rejects modeled cache state, and native profile admission fails
before mapping/tracee creation; decoded legacy native admission rejects `8E E0`.
This slice does not implement A-bit writes, null loads, LDT, other CPLs, GS/SS,
stack checks, long-mode FS limit/type checks, or complete S2-22 semantics.

Literal fixtures cover writable data G=0, read-only data G=1, readable and
conforming code, system and execute-only #GP, CPL3/RPL0/DPL0 #GP, valid P=0
#NP, and a descriptor crossing the GDTR limit. Runtime tests separately cover
the high bit of base32, G expansion, A=0 no-write, null/TI/CPL boundaries,
truncated fetch, and injected page/backend read faults without state or
retirement commit. The fixture runner checks every GPR, flags, RIP, base,
selector, cache fields, exception vector/error code, and unchanged descriptor
bytes. The writable G=0 literal encodes base32 `0x89abcdef` in the actual
descriptor bytes and verifies the loaded base, rather than describing a high
base while encoding zero.

A review follow-up changed result validation to use observed retired-instruction
count when deciding whether the FS cache may have changed. A later Exception,
Unsupported or HarnessError outcome no longer erases a prior retired `MOV FS`;
zero retired instructions still require the initial FS state. A CLI regression
executes `8ee00f0b` with budget 2, observes one retired load followed by #UD,
and runs the result through `--compare-results` against itself. Validator tests
also cover the later Unsupported and HarnessError outcomes. Conforming-code
DPL0 privilege behavior and FS-cache persistence across an external GDT edit
are not separately tested by this partial slice. The S2-22 umbrella remains
open.

Local verification passed: `cargo test --workspace --offline` (353 tests),
`cargo check --workspace --all-targets --offline`,
`cargo fmt --all -- --check`, the Python CPU suite (85 tests), all 57
`generate_stage2_*.py --check` freshness checks, `python3 tools/check_layout.py`,
and `git diff --check`. Linux-only native code and tests compiled with
`cargo check -p od-x86-runner --all-targets --target x86_64-unknown-linux-gnu
--offline`; this was compile-only, and no Linux-native test executed. No SSH,
physical Mac mini, or hardware acceptance was run; these remain in the shared
final acceptance substage. The review follow-up's focused tests also passed:
`cargo test -p od-testkit --offline fs_load_state_change_is_allowed_after_retirement_independent_of_terminal_outcome`,
`cargo test -p od-runner --test fixture_v2 --offline fs_load_commit_is_preserved_when_a_later_instruction_faults_and_self_compares`,
and `python3 tests/cpu/test_stage2_fs_load_generator.py`. The later S2-22g
independent Sol-medium review rechecked this path and reported no remaining
findings.

Source: Intel SDM revision 093, Vol.2B (253667-093US), MOV pp.4-28–4-31,
[official PDF](https://cdrdv2.intel.com/v1/dl/getContent/671241); Vol.3A
(325384-093US), §§3.4.3–3.4.5.1, 3.5.1, 6.4.1.1, 6.6–6.6.1, 6.11.5 and
7.13 plus Event 11, [official combined PDF](https://cdrdv2.intel.com/v1/dl/getContent/671447).

## S2-22g — CPL3 GDT-only `MOV FS, AX` accessed-bit update

Extended the existing `s2-long-fs-load-v1` local-only profile for exactly bare
`8E E0` at Long64/CPL3. The interpreter reads the complete descriptor once
through `DescriptorTable` origin, preserving architectural Ring3 in the
context while page privilege is Ring0. It preserves validation order
S/type/readability → CPL/RPL/DPL → P → A, including conforming-code privilege
exemption. A valid P=1/A=0 descriptor causes exactly one
`GuestMemory::write(descriptor_address + 5, same_descriptor_context,
&[access | 1])`; A=1 does no write. The FS selector/base/cache, RIP and
retirement commit only after a successful write. Architectural write faults
retain their payload; backend errors are returned unchanged; both leave the
current instruction uncommitted. No CPU-state operation that can fail follows
the successful backend write.

The one-byte backend call is the selected semantic memory access size; it does
not assert a physical bus transaction width. The operation is bounded to
private synchronous RAM under serial interpreter execution with no external
observer/writer, DMA, shared alias, delayed write or MMIO. Generic
`GuestMemory` must retain its existing no-mutation-on-error contract. Native
profile admission remains fail-closed, and S2-22 remains open.

Coverage includes writable and read-only data descriptors with A=0, A=1 from
a read-only mapping, exact read8 then write1 trace and matching descriptor
context, read errors without a write, injected architectural/backend write
errors without CPU/RIP/retirement commit, invalid type/rights/P=0 before an
armed write error, adjacent GDT regions where only the access-byte region is
writable, repeated selector loads that write once, and successful load/write
state preserved before a later #UD. Existing null/TI/CPL/truncation/canonical/
limit/high-base/G=0/G=1/conforming-code/native-admission cases remain covered.
Two generated fixtures add A=0 writable/read-only descriptor successes and
the fixture runner checks access-byte-only mutation.

Local verification passed: `cargo test --workspace --offline` (360 tests),
`cargo check --workspace --all-targets --offline`, the focused `od-isa`
`mov_fs_ax` tests (10), the adjacent-region `od-testkit` test (1), the
`fixture_v2` literal descriptor test (1), the Python CPU suite (85), all 57
Stage 2 fixture-generator freshness checks, `cargo fmt --all -- --check`,
`python3 tools/check_layout.py`, and `git diff --check`. No SSH, Mac mini,
native execution or physical acceptance was run; those remain in the shared
final acceptance substage. Independent Sol-medium code review completed with
no remaining code or fixture findings after the descriptor-origin wording fix.

Sources: Intel SDM revision 093, Vol.2B MOV pp.4-28–4-31, [official PDF](https://cdrdv2.intel.com/v1/dl/getContent/671241);
Vol.3A §§3.4.5.1 pp.3-12–3-13, 5.6.1 pp.5-33–5-35, 5.7 pp.5-37–5-38,
11.1.2.1 p.11-3 Automatic Locking, [official combined PDF](https://cdrdv2.intel.com/v1/dl/getContent/671447).

```sh
python3 tests/cpu/generate_stage2_fs_load.py --check
python3 tests/cpu/test_stage2_fs_load_generator.py
cargo test -p od-isa --offline mov_fs_ax
cargo test -p od-testkit --offline fs_load_accessed_byte_write_spans_adjacent_gdt_regions_without_full_descriptor_writeback
cargo test -p od-runner --test fixture_v2 --offline fs_load_fixtures_check_literal_descriptor_results_fault_codes_and_full_state
```

## S2-22h — CPL3 GDT-only `MOV GS, AX`

Added the separate local-only `s2-long-gs-load-v1` profile for exactly bare
`8E E8` at Long64/CPL3. The visible GS selector, canonical48 GS base, GDTR and
GS descriptor cache have independent schema-v2 fields; the FS profile wire
shape remains unchanged. The interpreter reuses the bounded FS validator and
descriptor executor, committing only GS selector/base/cache after successful
memory operations. It reads one 8-byte descriptor with descriptor-table origin
and architectural CPL3/page privilege Ring0, checks bounds/canonicality and
S/type/readability → privilege → P → A, writes `access | 1` to descriptor byte
5 only for A=0, and commits RIP+2/retirement only after success. Full FS state,
all GPRs, RFLAGS and GDTR remain unchanged. Null/TI, other CPLs, other source
registers, prefixes and other Sreg destinations are Unsupported. Native profile
admission remains fail-closed; S2-22, ISSUE-017 and ISSUE-018 remain open.

Eleven handwritten literal fixtures cover A=1 success with a read-only GDT,
A=0 writable/read-only descriptor paths, high-bit base32, G=0/G=1 limits and
attributes, descriptor type/privilege/presence faults, and GDTR bounds. The
runner checks literal full GPR/RFLAGS/RIP/GDTR results, GS base/selector/cache,
fault codes and descriptor bytes. Unit tests additionally check CPL/null/TI/
missing-state no-read cases, exact read8 and read8/write1 traces and contexts,
architectural/backend/permission errors without CPU commit, split GDT regions,
repeat-selector behavior, later-fault persistence, and an FS-state canary. The
fixture generator serializes handwritten bytes/state and does not calculate
instruction semantics.

Local verification passed: `cargo test -p od-isa -p od-testkit -p od-runner
-p od-x86-runner --offline` (all targets; 364 tests passed),
`cargo check --workspace --all-targets --offline`, all 87 Python CPU tests,
GS fixture generation/freshness and provenance tests, `cargo fmt --all -- --check`,
`python3 tools/check_layout.py`, and `git diff --check`. Independent Sol-medium
review found no remaining issues after adding explicit missing-state no-read
coverage and a DPL0 conforming-code fixture that exercises the privilege rule.
No SSH, Mac mini, native execution or physical check was performed; those remain
in the shared final acceptance substage. This work uses the CPU_CONTRACT rev093
references already checked in; no new independent PDF verification is claimed.

## S2-22i — loaded GS.base composition

Added one handwritten literal fixture to the existing local-only
`s2-long-gs-load-v1` profile. It admits exactly the existing `8E E8` load or
the new fixed two-instruction program `8E E8 65 48 8B 00` with budget and stop
RIP matched to that program. The fixture loads GS.base `0x80000000` from the
GDT selector `0x0b` at CPL3, applies the A=0 update only at byte
`0x20000d`, and then reads qword `0x1122334455667788` from `0x8000000b` using
the already-supported GS memory form. A canary at old-base address
`0x40000b` remains unchanged. The operand offset `0x0b` exceeds descriptor
limit zero intentionally; the existing S2-22b contract states that Long64
FS/GS runtime limits are not checked. The second instruction uses the cached
GS base and does not reread the descriptor.

Trace tests assert descriptor read8 at architectural CPL3/page Ring0, optional
access-byte write1, and one explicit CPL3 data read8. They also cover a later
data-read #PF, which retains the first instruction's GS/cache, A-bit write and
retirement, and a noncanonical result from RAX `0x00007fff8000000b`, which
raises #GP(0) without a data callback while preserving the first instruction's
effects. These canonicality and no-limit properties reuse the checks already
recorded in the CPU contract; no independent manual verification is claimed.
The fixture generator serializes the handwritten program, initial state,
descriptor/data/canary bytes and harness fields. It does not put the expected
post-state into the fixture: Rust unit and runner tests hold the expected
register, cache, retirement and memory results as literals.
S2-22 and ISSUE-017/018 remain open; native/physical execution and Linux
readiness are not claimed.

Local verification passed: `cargo check --workspace --all-targets --offline`,
`cargo test --workspace --all-targets --offline` (377 tests passed),
`cargo check -p od-x86-runner --all-targets --target x86_64-unknown-linux-gnu
--offline` (compile-only), all 88 Python CPU tests, GS fixture freshness and
generator provenance, `cargo fmt --all -- --check`,
`python3 tools/check_layout.py`, and `git diff --check`. The listed S2-22h
focused commands and the S2-22i unit/runner tests also passed.
Independent `gpt-6.1-sol` medium review found no issues. The Linux-only native
runner test was compiled for its target but not executed; no Mac mini, SSH,
native or physical acceptance was performed.

```sh
python3 tests/cpu/generate_stage2_gs_load.py --check
python3 tests/cpu/test_stage2_gs_load_generator.py
cargo test -p od-isa --offline mov_gs_ax
cargo test -p od-testkit --offline gs_load_accessed_byte_write_spans_adjacent_gdt_regions_without_full_descriptor_writeback
cargo test -p od-runner --test fixture_v2 --offline gs_load_fixtures_check_literal_descriptor_results_fault_codes_and_full_state
cargo test -p od-testkit --offline s2_22i
cargo test -p od-runner --test fixture_v2 --offline s2_22i_gs_base_composition_runs_the_literal_two_instruction_profile
```

## S2-22j — loaded FS.base composition

Added the literal local-only counterpart using exact bytes
`8E E0 64 48 8B 00` under the existing `s2-long-fs-load-v1` profile. The
profile now accepts only the existing exact `8E E0` load or this exact
two-instruction sequence with a matching code mapping, budget and stop RIP.
The fixture loads FS.base `0x80000000` through selector `0x0b`, updates only
GDT access byte `0x20000d` from F2 to F3, then reads the literal qword
`0x1122334455667788` at `0x8000000b`; a canary at the previous FS.base remains
unchanged. The generator serializes handwritten input bytes/state/descriptor
and data; Rust tests hold expected registers, hidden cache, memory and
retirement values as literals.

Unit traces assert descriptor read8 and optional A-byte write1 in descriptor
context at architectural CPL3/pageRing0, then one explicit CPL3 qword data
read with no GDT reread. A later data-read #PF keeps the load, descriptor
update and first retirement. A noncanonical address raises #GP(0) before the
data callback while preserving those earlier effects. This reuses the existing
FS/GS canonicality and Long64 no-limit contract; no new independent manual
verification is claimed. The physical x86 runner still returns Unsupported
before tracee creation for FS/GS profiles, and the fixture is not in native
selection. The Mac-only execution of the Linux-gated native rejection test is
deferred to the shared final Mac mini acceptance substage.

Local verification passed: `cargo test --offline -p od-testkit -p od-runner
-p od-x86-runner` (141, 55, and 9 tests respectively),
`cargo check --workspace --all-targets --offline`, and Linux target
`cargo check -p od-x86-runner --all-targets --target x86_64-unknown-linux-gnu
--offline` (compile-only). The GS/FS load fixture generator freshness check
and all three provenance tests passed. `cargo fmt --all -- --check`,
`python3 tools/check_layout.py`, and `git diff --check` passed. A cross-target
`cargo test --no-run` attempt reached the linker but could not link Linux ELF
tests with this macOS host linker; the cross-target `cargo check --all-targets`
above verifies compilation only. No Mac mini, SSH or physical test was run.
S2-22 and ISSUE-017/018 remain open.

```sh
python3 tests/cpu/generate_stage2_gs_load.py --check
python3 tests/cpu/test_stage2_gs_load_generator.py
cargo test -p od-testkit --offline s2_22j
cargo test -p od-runner --test fixture_v2 --offline s2_22j_fs_base_composition_runs_the_literal_two_instruction_profile
cargo test --offline -p od-testkit -p od-runner -p od-x86-runner
```

## S2-00 — профиль

Принят диагностический `s2-long-integer-v1`, объём A и обязательные решения для
последующих B–D отражены в [CPU_CONTRACT](../specs/CPU_CONTRACT.md). Создана
[методика interpreter](../testing/INTERPRETER.md). Проверены публичная страница
Intel SDM (revision 093) и Linux boot protocol §1.14. Прямой 64-bit entry
выбран для раннего Linux; требования к CPU и machine mapping разделены.
Позднее статически исследованы exact Ubuntu amd64 packages `7.0.0-38.38`,
image `7.0.0-38-generic` и buildinfo config: измерены hashes и header fields
(protocol 2.15, `xloadflags=0x007f`). Метод, provenance, hashes и required
capabilities — в [Linux profile report](STAGE_2_LINUX_PROFILE.md) и
[GUEST_REQUIREMENTS](../specs/GUEST_REQUIREMENTS.md). Это чтение bytes в памяти
по официальным источникам; staging, boot и проверка APT/PE signatures не
выполнены. Upstream v7.0 `configure_5level_paging` использует 32-bit trampoline
при mismatch текущего и требуемого LA57, поэтому direct64 не исключает
дальнейший 32-bit путь. Сопоставление Ubuntu source/build, reachable ISA,
полный C/D contract и Windows profile остаются открытыми.
Через web fetch этой сессии страница AMD APM 4.09 и два официальных пути PDF
возвращают HTTP 404; содержание manual не прочитано. Это ограничение проверки
источника в данной среде, [ISSUE-011](../KNOWN_ISSUES.md#issue-011--amd-apm-409-не-прочитан-через-web-fetch).
Реализация GPR опирается на Intel SDM, а не на поисковые выдержки AMD.
Повторная bounded попытка не закрыла ISSUE-011: web fetch вернул 404,
браузер по официальному landing показал «Content not found», прямой GET
официального `24594.pdf` в память завершился read timeout через 25 s до
получения PDF. Revision/sections по телу AMD manual не перепроверены;
новые AMD capabilities или hardware confirmation из этой проверки не следуют.

## S2-01 — исходный baseline и границы

Локальный baseline `stage1-harness-20261022-200549-6f8867b7` прошёл:
fmt, workspace check, targeted tests, Python orchestration tests и fixture digests.
Физический baseline `stage1-20261022-200905-e2c490b4` прошёл: 10 ручных + 1000
generated cases, seed `0x5eedcafe`, два повтора, ноль расхождений
interpreter/native, JIT/native и повторных результатов. Source fingerprint:
`0a7a30de39bf7e53c864c6a057508e405b5e1363790d9354748fa10a4e1889fe`.

Основной host — Apple M3 Max, macOS 26.7.1, pages 16384, Rust/Cargo 1.99.0;
oracle — Intel i5-3210M, Ubuntu, Linux 6.17.0-41-generic, pages 4096,
Rust/Cargo 1.99.1. Debug builds и точный fingerprint сохранены в manifest.
Полные данные: `work/stage1-oracle/stage1-20261022-200905-e2c490b4/`;
summary: `work/stage1-harness/stage1-harness-20261022-200903-f0fafd47/summary.json`.
Первый запуск был прерван внешним timeout; он не засчитывается за проверку.

Потребители state/instructions найдены в od-jit, od-host, od-testkit reducer,
od-runner main/test_cli и native trampoline. Карта adapters записана в
[INTERPRETER](../testing/INTERPRETER.md#карта-совместимости-s2-01).
Четырёхрегистровые fixture/result v1 и C ABI v0 сохраняются; исторические
evidence и digests не переписываются. Новый JIT в объём этапа 2 не включён.

## S2-02 — состояние и subregister access

`od-isa::LongModeState` содержит 16 GPR и RIP u64. `RegisterView` задаёт
low8/16/32/64 и AH/CH/DH/BH; `decode_byte` различает отсутствие REX,
REX low bytes SPL/BPL/SIL/DIL и расширенные R8B…R15B. High-byte view с REX
отклоняется. 8/16-bit writes сохраняют остальные биты, 32-bit write очищает
верхнюю половину, 64-bit write заменяет всё значение. Flags/RIP и другие
регистры при этих доступах не меняются.

Существующий тип переименован в `FragmentState` с `repr(C)`; `CpuState`
сохранён как source-compatible type alias старого API. Новый core не зависит
от его host layout. `TryFrom<&LongModeState>` отклоняет ненулевые дополнительные
регистры, неподдерживаемые flags, отсутствие reserved bit 1 и непредставимый
host-индекс. Для fragment register query вне четырёх регистров возвращается None.
JIT использует `offset_of!` для RIP/RFLAGS; тест проверяет layout и реальные
generated loads/stores. Decoder/JIT по-прежнему отклоняют новые формы.

Локальные tests проверяют все 16 регистров, widths, крайние значения,
aliasing/REX/high bytes и отсутствие потерь при adapters. Сборка workspace и
tests od-isa/od-jit/od-host/od-testkit/od-runner прошли после изменения state.
Это не физическое доказательство исполнения новых subregister-инструкций:
их decoder/execute/oracle ещё предстоит реализовать.

После изменения state повторный физический прогон
`stage1-20261022-201334-89ff8bca` прошёл: 1010 cases × 2 повтора, ноль
расхождений. Fingerprint `1c94edb7017ccf6c161a82579b8cc837e11b4a29a504e703158e3eb91c220ce2`.
Это регрессия старого профиля, не проверка новых ISA-форм.

## S2-03 — versioned fixture/snapshot

Добавлены строгие `od_testkit::v2` fixture/result, полное integer state,
memory layout/masks, definedness и раздельные outcomes. Существующие v1
readers/digests сохранены. CLI выполняет явное преобразование с новым digest,
не перезаписывает output и сравнивает v2 results по исходному case.
Tests выявляют подмену каждого GPR, RIP, flags/definedness, memory/fault fields,
outcome, input digest и memory layout; unavailable/unsupported не дают успех.
`cargo test -p od-testkit --offline` прошёл с 36 tests после S2-03.
Реальное CLI-преобразование `s0_09_mov_imm64` в
`work/tmp/s2-converted-check.case.json` прошло; новый SHA-256:
`8cf162f24cf31927a33c1400dcbb8f7b0571772a41a04d903537e849fa794159`.
Synthetic snapshots проверяют comparator, не CPU. Исполнение v2 пока не включено.

## S2-04 — память

Добавлен CPU-интерфейс GuestMemory с mode/CPL, отдельными fetch/read/write,
little-endian integer helpers и типизированным разделением CPU faults и
mapping/backend errors. FlatMemory находится в testkit и не заменяет MMU.
Он проверяет весь доступ до mutation; поддерживает смежные regions и
однократную fault injection. Tests покрывают разные permissions, переполнение
u64, недоступные ranges, пересечение 4 KiB/16 KiB, widths и injected CPU/backend
errors без RAM/destination side effects. После S2-04 od-testkit: 40 tests,
od-isa: 12 tests, все прошли. Подробный контракт —
[INTERPRETER](../testing/INTERPRETER.md#новые-fixture-api-и-память).

## S2-05 — пошаговое исполнение

`od_isa::execute::step/run` выполняет побайтный on-demand fetch с текущего RIP.
Поддержаны прежние S0 instruction forms; переходы декодируются при достижении,
недостижимый невалидный хвост не проверяется. Отдельны advanced/branched,
architectural fault, unsupported и mapping/backend error. Run проверяет явный
stop address, STOP, budget и только затем fetch; fault не retire-ится и
сохраняет faulting RIP/state. Конец code buffer не является неявным выходом
нового core. Старый `interpret` сохраняет предварительный S0 decode/границы
и делегирует исполнение этому core. Полный контракт —
[INTERPRETER](../testing/INTERPRETER.md#реализованный-steprun--s2-05).

До последующих изменений decoder/comparator прошли `cargo test --workspace
--offline`: od-host 6, od-isa 12, od-jit 4, od-runner 5, od-testkit 44 и
od-x86-runner library 8 tests. Scaffold/doc-test targets с нулём tests
не подтверждают функциональность. Format, workspace check и local harness
`stage1-harness-20261022-203013-6c1423c8` также прошли.

Физическая регрессия `stage1-20261022-203239-2d50e862` прошла 22 октября,
20:32:39–20:33:47 UTC: 1010 cases × 2 повтора, 2020 native results,
ноль interpreter/native, JIT/native и repeat расхождений. Fingerprint:
`39e0a6c8ac6de17594556e5923526d79bfef89bfe0c8fa38a68b474a6131a259`.
Summary: `work/stage1-harness/stage1-harness-20261022-203238-0e101c26/summary.json`.
Малое долговечное доказательство трёх baseline-прогонов:
[baseline-2026-10-22.json](data/stage2/baseline-2026-10-22.json).
Это проверка старого четырёхрегистрового S0 профиля и его adapters, не новые
widths/memory instructions, не весь этап 2. Этот fingerprint получен до
описанных ниже v2 review fixes и изменений S2-06; он не подтверждает их
физическую регрессию.

## Исправления по review v2 и памяти

После checkpoint исправлены обнаруженные дефекты проверки результатов:
CLI теперь вызывает `validate_for` для обоих results до разбора status;
unsupported/failed status не обходит validation case/profile/digest/snapshot.
`UnexpectedCoreExit` валидированного S0 adapter классифицируется как harness
failure. Comparator fault observations игнорирует только текст объяснения
unavailable reason; availability, доступные значения и обязательные поля
по-прежнему проверяются. Regression tests подтверждают эти границы.

Добавлены memory regressions: отказ прав во втором из двух смежных regions
оставляет RAM/read destination неизменными; injected error срабатывает ровно
один раз. После интеграции этих исправлений прошли 47 tests od-testkit и
9 tests od-runner. Это локальные проверки, без нового физического run.

## S2-06 — decoder

Добавлен собственный `long_decode` с нормализованными instruction description,
длиной, widths и операндами: legacy prefixes/REX, 66/67 и opcode maps.
Разобраны MOV immediate 8–64, register-register MOV, register ADD/SUB/CMP
соответствующих widths и short/near JZ/JNZ. Это decode coverage; execution
allowlist пока сохраняет S2-05: исходные S0 формы с первыми четырьмя r64
регистрами и short JZ/JNZ. Новое исполнение subregister/near Jcc не заявлено.
В checkpoint S2-06 memory operands остаются Unsupported; duplicate legacy groups, SIMD и REP
не поддержаны в этом срезе. Для REX влияет только properly located prefix;
ранние и misplaced REX игнорируются по SDM. Duplicate legacy groups отклоняются
как граница среза, без универсального утверждения об их архитектурной семантике.
LOCK консервативно Unsupported: неподтверждённый приоритет #UD/fetch fault
не заявляется. Invalid/#UD возвращается только для полностью полученного UD2.

После точечного review и интеграции всех изменений прошли format, workspace
check и `cargo test --workspace --offline`: od-host 6, od-isa 19, od-jit 4,
od-runner 9, od-testkit 47 и od-x86-runner library 8 tests. Layout и whitespace
checks также прошли. Нормативные источники — Intel
SDM 093 [Vol.2A](https://cdrdv2.intel.com/v1/dl/getContent/671199), §§2.1.1–2.2.1,
ADD/CMP/Jcc/LOCK, и [Vol.2B](https://cdrdv2.intel.com/v1/dl/getContent/671241),
MOV/SUB. S2-06 принят в перечисленных decode-only границах; новых execution
форм и их проверки на физическом oracle это не подтверждает.

## S2-07 — ModRM/SIB и effective address

Добавлены decode-only ModR/M/SIB, displacement, RIP-relative и 67 address-size
forms. Локально проверены 6228 raw-byte combinations; FS/GS memory forms
остаются Unsupported. Execution сохраняет S0 allowlist до S2-08: новые memory
описания сами по себе не означают чтение или запись гостевой памяти.

Effective-address helper проверяет только canonical start address. Проверка
полной ширины operand требуется в следующем S2-08. При noncanonical start #SS
относится к actual base RSP/RBP; #GP — к другим bases, включая R12/R13, а не
выбирается по совпадению младших encoding bits с RSP/RBP.

После review прошли format, workspace check, od-isa 26, od-testkit 47,
od-runner 9, od-host 6 и od-jit 4 tests. Нормативные источники: SDM093
[Vol.2A](https://cdrdv2.intel.com/v1/dl/getContent/671199) §§2.1.3–2.1.5,
§2.2.1.2 Tables 2-4/2-5, §2.2.1.3, §2.2.1.6 Table 2-7 и
[Vol.1](https://cdrdv2.intel.com/v1/dl/getContent/671436) §§3.3.7.1,
3.4.2.1, 3.7.4.1. S2-07 принят в decode-only границах; новых физических
execution результатов нет. Граф обновлён до 1584 nodes; известное ограничение
C header extractor [ISSUE-010](../KNOWN_ISSUES.md#issue-010--graphify-не-извлекает-символы-публичного-c-abi-header)
сохраняется.

## S2-08 — MOV через память и v2 CLI

Реализовано исполнение MOV B0–BF immediate register и 88–8B reg/reg/load/store
с widths 8/16/32/64, всеми 16 GPR, high-byte/REX aliases, сохранением flags.
Проверяется canonical диапазон всей ширины до backend; #SS относится к actual
RSP/RBP base, #GP — к остальным, включая R12/R13. 67 оборачивает только
effective start, а не последующие bytes operand на границе 4 GiB.
Fault/error сохраняет faulting RIP/state и не делает частичный commit.
API не изменён. Arithmetic execution остаётся S0 first-four r64 ADD/SUB/CMP,
branches — S0 short JZ/JNZ; остальные decoded forms не объявлены исполняемыми.

Для непредставимого u64 endpoint возвращается MemoryError::AddressOverflow
до backend и без commit, а не неподтверждённый #GP. В проверенных Intel
SDM093 Vol.3A §§4.5.1/6.15 однозначного ответа не найдено; это непроверенная
semantic boundary [ISSUE-012](../KNOWN_ISSUES.md#issue-012--не-подтверждена-семантика-operand-range-через-верх-u64),
не доказанный hardware bug. Ограничение AMD source fetch ISSUE-011 сохраняется.

CLI подключает actual v2 interpreter execution, full-state/memory/fault
snapshots; v2 JIT даёт Unsupported/exit 4. Mapping/backend errors —
Harness/exit 5, architectural fault — отдельный outcome. Cooperative deadline
на instruction boundary использует отдельный признак и даёт TimedOut/exit 5.
Три [stage2 fixtures](../../tests/fixtures/x86/stage2/) — реальные программы:
load/add/store, width aliasing и REX/RIP-relative addressing.

После совместной интеграции прошли `cargo fmt --all -- --check`,
`cargo check --workspace --all-targets --offline` и
`cargo test --workspace --offline`: od-host 6, od-isa 31, od-jit 4,
od-runner 10 unit и 6 actual CLI integration, od-testkit 49,
od-x86-runner library 8 tests. Layout и whitespace checks прошли.
S2-08 принят как локальный working MOV/v2 slice. Это не аппаратная проверка
новых memory форм и не приёмка полного CPU. Физический fingerprint
`39e0a6c8ac6de17594556e5923526d79bfef89bfe0c8fa38a68b474a6131a259`
остаётся только checkpoint S2-05; новых hardware memory результатов до S2-09 нет.

## S2-09 — native v2 и физическая проверка

Подготовлен отдельный Linux/x86-64 ptrace path с сохранением прежнего v1
trampoline. Supervised child выступает tracer, disposable tracee получает
fixture mappings, TRACEME/initial SIGSTOP и full GPR/RSP/RIP/flags через
GETREGS/SETREGS. SINGLESTEP и подтверждённый TRAP_TRACE дают exact retired
count; memory снимается через process_vm_readv. EXITKILL, process-group
watchdog и kill/reap ограничивают lifecycle. Host signal не превращается
в architectural exception.

Native profile ограничивает readable low mappings endpoint ≤2 GiB,
envelope с guard pages ≤64 MiB, исключает W+X и несовместимые permissions
logical regions одной host page. Allowlist проверяет actual native registers,
fetch и memory ranges перед каждым шагом. Код не резервирует guest register
для host pointer. Реализация собрана и проверена на Linux Mac mini.

Добавлены Python harness `tests/cpu/stage2_oracle.py` и три отдельных
[hw fixtures](../../tests/fixtures/x86/stage2/README.md#отдельные-входы-для-физического-linux-oracle-s2-09)
с новыми IDs/digests и literal low-address layout. Старые S2-08 inputs не
перезаписаны. Harness сохраняет точные inputs, fingerprints, outputs и manifest,
требует два повтора interpreter/native, не запускает v2 JIT; полный S2-09
дополнительно требует legacy 1010 cases ×2 на совпадающих исходниках.

Локальные format, workspace check и workspace tests прошли; 17 Python harness
tests прошли без SSH. Затем на Linux прошли workspace check all-targets
и od-x86-runner tests: library 9, binary 22 (9 native v2 +4 fetch +9 supervisor).
Предупреждения unused существовали ранее; новых build/test failures нет.

Физическая legacy регрессия `stage1-20261022-211911-7d05dd0b` прошла
21:19:11–21:20:17 UTC: 1010 inputs ×2, 2020 native results, ноль расхождений
interpreter/native и JIT/native, стабильные повторы. Native v2 run
`stage2-20261022-212031-027fe280` прошёл 21:20:31–21:20:36 UTC: 3 inputs ×2,
6 native results; совпали все 16 GPR, включая RSP, RIP, flags с defined mask
`0xcd7`, вся fixture memory и exact instruction counts. Повторы стабильны.
Общий fingerprint `bcade2e1c1282285f923e43e89dde2ffff7a70d900922022c3368f9aaf04cf5d`;
linked legacy manifest SHA-256
`2f22cc1fb7614c02ce497caac4d39821542c471c094b403103f1b0991a29d1ef`.
Малое долговечное [evidence](data/stage2/native-2026-10-22.json) содержит
hashes/times/hosts/build/limits/reproduction; большие raw outputs остаются в
`work/stage1-oracle/` и `work/stage2-oracle/` по указанным run IDs.

S2-09 принят для этого положительного MOV/memory среза и legacy регрессии
на одном Intel i5-3210M. Privileged/faulting instructions, AMD, v2 JIT и полный
этап 2 не подтверждены. ISSUE-011/012 остаются открытыми. Старый fingerprint
`39e0a6c8…` относится только к S2-05. Методика —
[CPU_ORACLE](../testing/CPU_ORACLE.md#native-v2-s2-09-границы-физического-подтверждения).

## S2-10 — transfer extensions

Подготовлен core для C6/C7 immediate r/m, A0–A3 moffs, MOVSX/MOVSXD/MOVZX,
LEA, register XCHG и NOP90. Реализатор перед code прочитал Intel SDM093
Vol.2B MOV 4-28–31, MOVSX/MOVSXD 4-120, MOVZX 4-130; Vol.2A LEA 3-547–548;
Vol.2D XCHG 6-32–33. Независимая проверка повторно прочитала official PDFs
253666-093US/253667-093US/334569-093US (September 2026) через системный curl
с проверкой TLS и pdftotext в памяти; web.open для этих URL был недоступен,
а Python urllib не доверял локальной цепочке сертификатов, поэтому insecure
TLS не использовался. Подтверждены MOVSXD 16→16/32→32/32→64, independent
LEA operand/address widths и NOP alias 90 независимо от data-size prefix.
Точные формы/widths/effects/exceptions и ограничения —
в [CPU contract](../specs/CPU_CONTRACT.md#передача-данных-и-lea--s2-10).

Независимое source review текущего core не выявило конкретных дефектов в
fault/commit timing, LEA без memory access/canonical check, C7 next RIP с
immediate, NOP90 против `87 c0` и обмене AL/AH. Это code review, не physical
oracle. Пять новых fixtures включают все заявленные extension widths при
address32/64, C6/C7 stores всех widths, moffs load/store matrix, LEA widths и
FS/GS/no-read cases, register XCHG aliases. Промежуточные результаты сохранены
в memory, canaries выявляют неверную ширину записи, flags mask равен `0xcd7`.
Независимый coverage review после дополнения 67/LEA16×32 gaps не выявил
оставшихся конкретных пробелов. Memory XCHG/atomicity, LOCK и новые
arithmetic/system forms не включены.

Отдельно прошёл `cargo check --workspace --all-targets --target
x86_64-unknown-linux-gnu --offline` на Mac после установки стандартного Rust
target: cfg Linux/x86-64 native v2 typechecked, остались прежние unused warnings.
Это cross-check, не Linux runtime или hardware execution. После явного согласия
владельца конкретный S2-10 payload (5 Rust sources и 5 fixtures) передан на
Mac mini. Actual Linux `cargo test --workspace --offline -- --test-threads=1`
прошёл: od-isa 39, od-jit 4, od-runner 10 unit +6 CLI integration,
od-testkit 54, od-x86-runner library 9 и binary 23 tests; прочие targets — 0.
На Mac full workspace прошёл с od-host 6, od-isa 39, od-jit 4, od-runner
10+6, od-testkit 54 и native library 8. Format, Linux-target workspace check,
layout, whitespace и 17 Python harness tests также прошли. Граф обновлён:
1811 nodes, 3782 edges, 118 communities; известное предупреждение C header
extractor ISSUE-010 сохраняется.

Legacy run `stage1-20261022-213920-13e82953` прошёл 21:39:20–21:40:16 UTC:
1010 cases ×2, 2020 native results, ноль interpreter/native и JIT/native
расхождений, стабильные повторы. V2 run `stage2-20261022-214049-874a0bf0`
прошёл 21:40:49–21:40:57 UTC: восемь cases ×2, 16 native results, включая
пять новых transfer fixtures. Совпали все 16 GPR/RSP/RIP, flags defined mask
`0xcd7`, вся fixture memory и exact instruction counts; повторы стабильны.
Общий fingerprint `93ac5dfd316316cfc76f27d83fad993e3148ba6349ca77feede314da43e10d2c`;
linked legacy manifest SHA-256
`d3e8845dd420422385328551cab06c345fe3260fcf761ce6b0e89e275cb9a603`.
Отдельное долговечное [evidence](data/stage2/transfers-2026-10-22.json)
содержит точные hashes/times/hosts/build/limits/inputs/reproduction; прежнее
S2-09 evidence сохранено. S2-10 принят для этих положительных программ на
одном Intel CPU; полный этап, AMD, privileged faults и v2 JIT не подтверждены.
ISSUE-011/012 остаются открытыми.

## S2-11 — арифметика и eager flags

Реализованы ADD/ADC/SUB/SBB/CMP widths 8/16/32/64, оба ModR/M направления
reg↔r/m, dedicated accumulator immediate и groups 80/81/83; INC/DEC FE/FF
и NEG F6/F7. Sign-extension immediates, aliases/32-bit zero-extension и
carry/borrow соблюдают [CPU contract](../specs/CPU_CONTRACT.md#арифметика-и-eager-flags--s2-11).
CF/PF/AF/ZF/SF/OF материализуются eager; INC/DEC сохраняют CF, CMP не пишет
destination/memory, DF сохраняется. LOCK/REP, FS/GS data arithmetic,
logic/TEST и будущие system/FP формы не заявлены.

Нормативные sources независимо сверены по official Intel SDM093:
Vol.2A ADC 3-9–11, ADD 3-14–15, CMP 3-161–162, DEC 3-262–263,
INC 3-457–458; Vol.2B NEG 4-158–159, SBB 4-613–615, SUB 4-685–686;
Vol.1 §3.4.3.1 и Appendix A Table A-2 flags. Подробные ссылки — в контракте.
313 one-instruction fixtures и literal sidecar проверяют full16/flags/memory,
формы, widths, sign extension и boundaries; материализатор только упаковывает
handwritten templates/буквальные ответы. Независимое review не нашло
конкретных дефектов в таблицах/coverage; `--check` подтвердил точные bytes.

На Mac `cargo test --workspace --offline` прошёл: host 6, isa 45, jit 4,
runner 10+6 CLI integration, testkit 56, native library 8, всего 135 tests.
Linux target all-target workspace check, format, 17 Python harness tests,
materializer `--check`, layout и whitespace checks прошли. На Mac mini actual
`cargo test --workspace --offline --quiet -- --test-threads=1` прошёл:
isa 45, jit 4, runner 10+6, testkit 56, native library 9 + binary 24,
всего 154 tests; предупреждения только прежние dead_code. Local injected
read/write #PF/backend failures во втором region подтверждают no-commit:
faulting instruction не retire, RIP/GPR/flags/RAM сохраняются. Это не physical
guest #PF: native positive admission исключает недоступные ranges/permissions,
а host signal остаётся отдельным наблюдением.

Legacy run `stage1-20261022-215743-751a2dd1` прошёл 21:57:43–21:58:43 UTC:
1010 cases ×2, 2020 native snapshots, ноль interpreter/native и JIT/native
расхождений, стабильные повторы. V2 run `stage2-20261022-215943-adeb1874`
прошёл 21:59:43–22:00:21 UTC: 321 cases (313 S2-11 +5 S2-10 +3 S2-09) ×2,
642 native snapshots. Расхождений interpreter/native нет; все 321 repeat2
стабильны. Доступны full16/RIP/RFLAGS mask `0xcd7`, вся memory, exception
observation и exact counts. Fingerprint:
`db01a6db7b28c41c9fd21761f308a3ef8c60a61a808530bc73ac36525bd391ba`.
Linked legacy SHA-256:
`006820aee22457fc54909c6fa5b7c8f6b699050eee688756d1eb5b27042df1fe`.
Новое [arithmetic evidence](data/stage2/arithmetic-2026-10-22.json) сохраняет
manifest hashes, times, machines/build, input-index digest, literal-source
hashes и reproduction. Подтверждён один Intel i5-3210M/Linux
6.17.0-41-generic; AMD, privileged faults, v2 JIT и весь этап не подтверждены.
S2-11 принят в описанных положительных границах; ISSUE-011/012 открыты.

## S2-12 — logic, TEST/NOT и semantic definedness

Реализованы AND/OR/XOR reg/memory/accumulator/group-immediate, TEST только
реальными 84/85/A8/A9/F6/F7 /0 forms и NOT F6/F7 /2, widths8/16/32/64,
address32/64 и aliases. TEST не пишет destination, NOT сохраняет flags.
CF/OF после AND/OR/XOR/TEST равны нулю, SF/ZF/PF определены, AF undefined;
его значение не сравнивается. Источники Intel SDM093: Vol.2A AND 3-60–61;
Vol.2B OR 4-163–164, NOT 4-161–162, TEST 4-721–722; Vol.2D XOR 6-40–41.
Точные формы и ссылки — в [контракте](../specs/CPU_CONTRACT.md#логика-test-и-definedness--s2-12).

Semantic mask берётся из общей decoded `FlagEffects`, не из expected evaluator,
и применяется только после успешного retirement; native — после TRAP_TRACE.
Logic/TEST снимает AF из `rflags_defined`, MOV/NOT сохраняют definedness,
arithmetic с defined AF восстанавливает его. Последовательности `XOR;MOV;NOT`
и `XOR;ADD` проверены отдельными cases. Fault/unsupported не обновляет mask.
197 новых literal fixtures и sidecar материализуются без x86 evaluator;
старые S2-11 inputs/expected/evidence не переписаны. Local injected read/write
architectural/backend faults проверяют no-retirement/no-commit/no-mask-update,
включая вторую logical region. Native positive profile не доказывает guest #PF:
недоступные ranges/permissions отклоняются admission, host signals отдельны.

Локальный Mac workspace: 146 tests passed (host6, isa52, jit4, runner10,
fixture_v2 integration8, testkit58, native library8). Actual Linux workspace:
167 passed (isa52, jit4, runner10, integration8, testkit58, native library9
и binary26). Python harness17, оба materializer `--check`, Linux-target
workspace check, format/layout/whitespace прошли. Предупреждения dead_code
не расширяют функциональные claims.

Legacy run `stage1-20261022-221121-5c3dfa43` прошёл 22:11:21–22:12:22 UTC:
1010 inputs ×2, 2020 native snapshots, ноль interpreter/native и JIT/native
различий. V2 run `stage2-20261022-221233-96c6116b` прошёл
22:12:33–22:13:51 UTC: 518 inputs (3 S2-09 +5 S2-10 +313 S2-11 +197 S2-12)
×2, 1036 native snapshots, ноль различий; все 518 repeat2 стабильны.
Full16/RIP/RFLAGS/exception/count observations доступны. В обоих backend
684 snapshots имеют semantic mask `0xcd7`, 352 — `0xcc7`.
Fingerprint `5d964b8ba56fc641ff86f51088daafe3a1799052f0b31f68350d23447f6a14d5`;
linked legacy SHA-256
`f97d13301aea75a9f87c9b656ad451824a52cf60b0f45cdd7dca794cde99d63b`.
Новое [logic evidence](data/stage2/logic-2026-10-22.json) сохраняет actual
manifest hashes/times/machines/build/input-index/literal hashes/reproduction.
S2-12 принят для этого положительного профиля на одном Intel i5-3210M,
kernel 6.17.0-41-generic. AMD, privileged exceptions, v2 JIT и полный этап 2
не подтверждены; ISSUE-011/012 остаются открытыми.

## S2-13 — сдвиги и вращения

Поддержаны SHL/SAL/SHR/SAR, ROL/ROR/RCL/RCR и SHLD/SHRD с immediate/CL counts,
masking/modulo effective-count semantics, carry-in и definedness flags по
форме/count. Effective count zero сохраняет destination и flags; 32-bit
destination write zero-extends согласно long-mode правилу. Для SHLD/SHRD при
count больше operand width destination undefined и не сравнивается exact.
Точные формы и источники Intel SDM093 перечислены в
[CPU contract](../specs/CPU_CONTRACT.md#сдвиги-и-вращения--s2-13). Две 16-bit
SHLD/SHRD raw17 cases исключены из physical exact comparison как undefined;
локальные `od-isa` tests оставляют no-panic и all-arithmetic-flags-undefined
проверки. `od-isa` README документирует dword count-zero zero-extension.

Локальные injected read/write faults проверяют нулевой и ненулевой effective
count: инструкция не retire, CPU/RIP/definedness/RAM не commit-ятся. Native
harness успешно проверил 142 register fixtures и 2 memory RMW fixtures; он
также проверяет admission reject для запрещённых ranges/permissions. Его native
процесс не может сохранить post-host-signal snapshot, поэтому physical
fault-state preservation для этих memory cases не заявляется.

Проверки прошли: Mac `cargo test --workspace --offline`; Mac
`cargo check --workspace --all-targets --offline`; `cargo fmt --all -- --check`;
generators arithmetic/logic/shifts `--check`; `python3 -m unittest discover
-s tests/cpu` (17 passed); `python3 tools/check_layout.py`; `git diff --check`.
Linux mini full workspace `cargo test --workspace --offline --quiet --
--test-threads=1` прошёл: 172 passed, 0 failed; targeted
`cargo test -p od-x86-runner --offline`: 9 library + 28 binary tests passed.
Один первый Linux-only run выявил ошибку setup mask length; test setup исправлен,
повторный targeted run и полный workspace run прошли. Локальные tests также
подтверждают отсутствие panic и undefined arithmetic flags у исключённых
raw17 boundaries.

Legacy физический run `stage1-20261022-223439-2d67338b` прошёл 22:34:39–22:35:44
UTC: 1010 inputs ×2, без diff/error; fingerprint
`e21944ca03541871fe6973bb7ccfb47307dd4963f91a1c7dee9262d1be46cfa2`, manifest
SHA-256 `05b75f7874458a46138663d91d7892e8cbbeecb0a8550546dea1380e29ba91f2`.
Stage2 run `stage2-20261022-223610-81d63d71` прошёл 22:36:10–22:37:59 UTC:
662 cases ×2 = 1324 snapshots, suite split S2-09 3 / S2-10 5 / S2-11 313 /
S2-12 197 / S2-13 144; ноль interpreter/native diffs, все 662 cases стабильны
на repeat 2. Fingerprint совпадает с legacy; manifest SHA-256
`1962b53007d190bab22ee97037248f692224f84a43e486a859c8c69821ce23c3`, linked
legacy manifest SHA выше. Каждый backend наблюдал все 16 GPR, RIP и RFLAGS.
Per-backend definedness masks: `0x04c6` — 16, `0x04c7` — 12, `0x04d7` — 32,
`0x0cc7` — 412, `0x0cd7` — 852 snapshots. Физическая машина — Intel Core
i5-3210M, Linux 6.17.0-41-generic, x86_64; local Apple M3 Max/Rust 1.99.0,
remote rustc 1.99.1. [Долговечное evidence](data/stage2/shifts-2026-10-22.json)
содержит полные hashes, времена, сборки, input digest и reproduction; raw
manifests остаются в `work/stage1-oracle/` и `work/stage2-oracle/` по указанным
run IDs.

S2-13 принят в этих успешных interpreter/native границах на одном Intel CPU.
Это не подтверждает AMD, privileged faults, v2 JIT или полный этап 2;
ISSUE-011/012 остаются открытыми.

## S2-14 — умножение и деление

Реализованы one-operand MUL/IMUL (`F6/F7 /4,/5`), двухоперандный IMUL
(`0F AF`) и трёхоперандный IMUL с immediate (`69/6B`), а также DIV/IDIV
(`F6/F7 /6,/7`). Implicit product/dividend registers зависят от operand width:
AX для byte, иначе AX/DX, EAX/EDX или RAX/RDX. MUL/однопараметрический IMUL
материализуют полный результат в accumulator pair; двух/трёхоперандный IMUL
записывает destination-width результат. Immediate sign-extends согласно форме.
CF/OF определены для multiply по signed/unsigned overflow, SF/ZF/AF/PF
undefined; после успешного DIV/IDIV все арифметические flags undefined.

Локальный DIV/IDIV выдаёт архитектурный fault для divisor zero и quotient,
который не помещается в destination. `#DE` обнаруживается до destination write
и retirement; локальный `od-isa` unit test
`divide_errors_preserve_exact_state_and_report_de_before_retirement` проверяет
точный неизменившийся state и отсутствие retirement для divisor zero,
unsigned quotient overflow, signed byte minimum / −1 и signed 128-bit minimum /
−1. At S2-14 completion this was unit-test evidence only; S2-20a and S2-20c
later added three local-only fixture-v2/CLI divide-error snapshots. Signed
division округляет quotient к нулю. Все 38 v2 fixture cases проверяют только успешные
операции с нормальным stop outcome и без exception; они покрывают формы,
widths, успешные quotient/remainder и defined flags. Эти проверки подтверждают
только interpreter-side behavior, не hardware `#DE` delivery или соответствие
host signal/context архитектурному vector.

Точные Intel SDM revision 093 references сверены по официальным PDF volumes:
[Vol.2B](https://cdrdv2.intel.com/v1/dl/getContent/671241) MUL pp.4-140–4-141;
[Vol.2A](https://cdrdv2.intel.com/v1/dl/getContent/671199) IMUL pp.3-451–3-454,
DIV pp.3-264–3-266, IDIV pp.3-448–3-450; Vol.1 §3.4.1.1 — GPR alias/32-bit
write rules. The fixture metadata currently identifies the four mnemonics
under Vol.2B collectively; the verified manual split above corrects that
attribution for this report.

`python3 tests/cpu/generate_stage2_multiply_divide.py --check` reports 38 current
successful-operation fixture/expected pairs. Runner integration test
`multiply_divide_cli_fixtures_match_literal_full_state_and_flag_definedness`
checks each case exits at its stop address with one retired instruction, no
exception, and literal full GPR/RIP/flags-definedness/memory state. The separate
local `od-isa` unit test above contains the `#DE` faulting inputs; fixture-v2/CLI
coverage for three of those inputs was added later in S2-20a and S2-20c. Локальные `cargo test --workspace --offline` и
`cargo check --workspace --all-targets --offline` прошли; генератор сообщает 38
актуальных fixtures. Полный итоговый workspace suite также прошёл на Intel mini
и локальном host.

Физический прогон `stage2-20261022-230853-3684f650` завершился со статусом
passed: 700 inputs ×2 = 1400 comparisons, включая 38 новых S2-14 successful
fixtures ×2; различий interpreter/native нет, все 700 cases стабильны на
повторе 2. Оба backend снимали все 16 GPR, RIP и RFLAGS. Распределение
per-backend definedness masks: `0x0402` — 18, `0x04c6` — 16, `0x04c7` — 12,
`0x04d7` — 32, `0x0c03` — 58, `0x0cc7` — 412, `0x0cd7` — 852 snapshots.
Целевой host: Intel Core i5-3210M, Ubuntu x86_64, Linux
`6.17.0-41-generic`, Rust/Cargo 1.99.1. Сравнение охватывает выбранные v2 CPU
fixtures для interpreter и native физического oracle; v2 JIT не участвовал.

Итоговый manifest:
[`work/stage2-oracle/stage2-20261022-230853-3684f650/manifest.json`](../../work/stage2-oracle/stage2-20261022-230853-3684f650/manifest.json),
SHA-256 `1ce8d6ac41ca76ac8746dd60bb1618539af10ef270559c761d9adc790ad46e6b`;
source fingerprint `f9c90e6246c3bd40c9c51bdb2c0d62389d585cbe99f05a8bebca4963d37277fc`.
Он связан с legacy manifest
[`work/stage1-oracle/stage1-20261022-230241-3b73e810/manifest.json`](../../work/stage1-oracle/stage1-20261022-230241-3b73e810/manifest.json),
SHA-256 `cf2483245be2dd71cf956043c76cd7981cead0c654a01d85469ee543c0c471c8`;
тот прошёл 1010 inputs ×2 с тем же fingerprint. Компактные параметры прогона:
[evidence JSON](data/stage2/multiply-divide-2026-10-22.json).

Положительные fixture results не проверяют faulting divide. На момент этого
S2-14 прогона fault inputs существовали только в local `od-isa` unit test;
последующие S2-20a/S2-20c добавили три local-only fixture-v2/CLI snapshots.
Физический запуск не содержал faulting inputs. Поэтому результат не
подтверждает физическую доставку `#DE` или отображение Linux host
signal/context на архитектурный `#DE`.

## S2-15a — sign-extension opcodes 98 и 99

Локально реализованы `CBW`, `CWDE`, `CDQE` (opcode `98`) и `CWD`, `CDQ`,
`CQO` (opcode `99`) с выбором ширины operand-size, правилами alias writes,
zero-extension для 32-bit writes и сохранением flags. Шесть handwritten v2
fixtures используют literal expected state; генератор не использует assembler
или emulator.

Локальная проверка на Mac прошла: `cargo test -p od-isa --offline` (63 tests),
`cargo test -p od-testkit --offline sign_extension_fixtures_match_literal_full_state_answers`
(6 fixtures), `cargo test -p od-runner --offline --test fixture_v2
sign_extension_cli_fixtures` (6 fixtures) и `cargo test -p od-x86-runner
--offline` (8 tests). Также прошли macOS и Linux-target `cargo check --workspace
--all-targets --offline`, `cargo fmt --all -- --check`,
`python3 tests/cpu/generate_stage2_sign_extension.py --check` и `git diff
--check`. Независимое ревью после исправлений завершилось без замечаний.
Linux-only admission test компилируется, но не может быть слинкован/исполнен на
macOS.

Шесть S2-15a fixtures ещё не запускались на физическом Mac mini. После передачи
на mini `cargo test --workspace --offline` прошёл; stage1 harness запускался с
настройками по умолчанию и проверил только 10 S0/S1 fixtures ×2. Это не
подтверждает эти шесть fixtures, stage2 corpus или legacy corpus из 1010 cases.
Пользователь попросил объединить оставшиеся physical checks в один итоговый
mini-check substage; до него hardware comparison для opcode 98/99 остаётся
отложенным. Поэтому S2-15a принят только как локально проверенный срез, а не как
физически подтверждённая возможность.

## S2-15b — BSWAP

Локально реализован BSWAP (`0F C8+rd`) для 32- и 64-bit GPR с корректным
выбором регистра по REX.B, REX.W и operand-size override; dword writes
обнуляют верхние 32 бита регистра, flags сохраняются. Архитектурно
неопределённый 16-bit result не объявляется поддержанным. Семь handwritten
fixtures содержат literal expected state и покрывают legacy/extended
registers и width selection; generator не использует assembler или emulator.

Локальная проверка и независимое ревью после исправлений прошли без замечаний:
`bswap_decodes_opcode_register_and_long_mode_operand_size_rules` и
`bswap_register_forms_reverse_bytes_and_preserve_flags` в `od-isa`,
`cargo test -p od-testkit --offline bswap_fixtures_match_literal_full_state_answers`
(7 fixtures), `cargo test -p od-runner --offline --test fixture_v2
bswap_cli_fixtures` (7 fixtures) и
`python3 tests/cpu/generate_stage2_bswap.py --check` (7 current). macOS
workspace check и fmt прошли; Linux-target `od-x86-runner` all-target check
прошёл. Target-gated native test
`bswap_register_fixtures_are_admitted_without_data_memory_or_flag_effects`
скомпилирован, но не исполнялся на Mac; он проверяет native admission/filter
без исполнения физического fixture.

Семь BSWAP fixtures ещё не запускались на физическом Mac mini. Их hardware
comparison отложен для единого итогового mini-check substage; локальные
fixture, admission/filter и unit-test результаты не являются physical oracle.
Поэтому S2-15b подтверждён только локально. Вместе с локально завершённым
S2-15a это не закрывает S2-15: оставшиеся integer forms и физическая проверка
13 fixtures из S2-15a/b остаются открытыми.

## S2-15c — BT/BTS/BTR/BTC, register-only

Локально реализованы register/register формы BT, BTS, BTR и BTC с opcode
`0F A3`, `0F AB`, `0F B3` и `0F BB` соответственно. Срез ограничен
16/32/64-bit регистрами: offset выбирает бит modulo operand width; BT только
проверяет бит, а BTS/BTR/BTC устанавливают, сбрасывают или инвертируют его.
CF получает исходное значение выбранного бита, ZF сохраняется, OF/SF/AF/PF
не определены. Destination и offset читаются до возможной записи, включая
случай aliases; 32-bit write обнуляет верхние 32 бита. Memory destination и
LOCK не входят в этот register-offset substep. Immediate register forms
`0F BA /4–/7 ib` реализованы отдельно в S2-15f; memory forms с register bit
index и memory destination реализованы в S2-15h.

Семантика сверена с Intel SDM revision 093, Vol.2A: BT pp. 3-112–3-113,
BTC pp. 3-114–3-115, BTR pp. 3-116–3-117, BTS pp. 3-118–3-119. Правило
zero-extension 32-bit GPR write — Vol.1 §3.4.1.1. Двенадцать handwritten
fixtures содержат literal full-state answers для всех четырёх операций и
widths 16/32/64; их байты и ответы не вычисляются assembler/emulator.

Локально прошли focused `od-isa` decode, rejection-boundary, execution и alias
tests, `od-testkit` test
`bit_operation_fixtures_match_literal_full_state_and_definedness_answers`
(12 fixtures), runner test
`bt_family_cli_fixtures_match_literal_state_and_flag_definedness` (12 fixtures),
`cargo test --workspace --offline`, `cargo fmt --all -- --check`,
`python3 tests/cpu/generate_stage2_bitops.py --check` (12 current),
Linux-target all-target compilation и `graphify update .`. Независимое ревью
`gpt-6.1-sol` / medium завершилось без замечаний.

Все 12 S2-15c fixtures ещё не запускались на физическом Mac mini; их physical
comparison отложен для единого итогового mini-check substage. Unit, fixture,
CLI и graphify проверки остаются локальными evidence и не являются hardware
oracle. S2-15c подтверждён только локально.

## S2-15d — BSF/BSR, register-only

Локально реализованы register-only BSF (`0F BC`) и BSR (`0F BD`) для
16/32/64-bit widths. BSF находит индекс младшего установленного source bit,
BSR — старшего. При нулевом source устанавливается ZF и destination
сохраняется; при ненулевом source записывается найденный индекс и ZF
сбрасывается. Остальные арифметические flags undefined; 32-bit destination
write обнуляет верхние 32 бита. Memory и F3-prefixed forms остаются вне этого
среза.

Семантика сверена с Intel SDM revision 093, Vol.2A: BSF pp. 3-107–3-108,
BSR pp. 3-109–3-110. Семнадцать handwritten fixtures содержат literal
full-state answers и покрывают zero/nonzero inputs, края каждого width,
extended registers и aliases. После review замечания generator изменён так,
что ожидаемые destination и RFLAGS заданы явными independently hand-calculated
literal значениями; materializer не вычисляет их из source fixtures. Это
устранило P2 provenance finding.

Локально прошли focused tests `bit_scan_register_forms_decode_sizes_and_rex_registers`,
`bit_scan_memory_and_f3_prefixed_forms_remain_unsupported` и
`bit_scans_cover_width_edges_zero_sources_aliasing_and_flags`, `od-testkit`
test `bit_scan_fixtures_match_literal_full_state_and_definedness_answers`
(17 fixtures), runner test
`bsf_bsr_cli_fixtures_match_literal_state_and_flag_definedness` (17 fixtures),
`cargo test --workspace --offline`, `cargo fmt --all -- --check`,
`python3 tests/cpu/generate_stage2_scans.py --check` (17 current),
Linux-target all-target compilation и `graphify update .`. Linux native admission
tests компилируются, но не исполнялись на macOS из-за отсутствующего Linux
linker. Независимое ревью `gpt-6.1-sol` / medium завершилось чисто после правки
generator; P2 provenance finding закрыт.

Все 17 S2-15d fixtures ещё не запускались на физическом Mac mini; их physical
comparison отложен до единого итогового mini-check substage. Локальные tests,
fixtures и compile не являются hardware oracle. Поэтому S2-15d подтверждён
локально и независимо reviewed, но физическая проверка остаётся открытой.

## S2-15e — BSF/BSR memory-source

К обычным BSF/BSR register forms добавлен memory-source вариант `0F BC/BD
r/m` для 16/32/64-bit operand widths. Шесть literal fixtures (три BSF и три
BSR) проверяют memory source: zero, selected-bit position и destination
preservation/update, flags и полное состояние. Memory-destination forms
BT/BTS/BTR/BTC находятся вне этого среза и реализованы в S2-15g/h. BSF/BSR с
F3 prefix также не принимаются, поскольку
LZCNT/TZCNT имеют feature-dependent semantics.

При execution source полностью читается до записи destination, flags и RIP.
Локальный test `memory_bit_scan_read_fault_does_not_commit_cpu_or_definedness`
подтверждает, что injected read fault сохраняет CPU/definedness и не retire-ит
инструкцию. Native-v2 admission требует, чтобы memory source находился в
mapped readable region; это проверка admission, не физическое исполнение.
Шесть новых cases дополняют 17 register-only cases: literal runner test и
generator покрывают 23 BSF/BSR scan fixtures всего.

Локальные проверки: `cargo test -p od-isa --offline` — 73 passed;
`cargo test -p od-runner --offline --test fixture_v2
bsf_bsr_cli_fixtures_match_literal_state_and_flag_definedness` — 23 literal
fixtures; `python3 tests/cpu/generate_stage2_scans.py --check` — 23 current;
`cargo fmt --all -- --check`; workspace all-targets check for Linux target.
Native Linux binary не удалось слинковать на macOS из-за Apple `ld` вместо
Linux linker; Linux admission tests не исполнялись локально на Mac.

Ни одна из шести новых S2-15e fixtures ещё не прошла физический mini-check.
Пользователь попросил отложенные physical comparisons на Mac mini собрать в
один общий финальный подэтап после локальной реализации оставшихся work items.
Ранее выполненные physical runs (включая S2-14) остаются подтверждёнными
отдельными результатами; это решение не переносит и не отменяет их.

## S2-15f — BT-family immediate register forms

Реализованы обычные BT/BTS/BTR/BTC immediate формы `0F BA /4`, `/5`, `/6`,
`/7 ib` только при `r/m=register`, с widths 16/32/64. В register form
immediate offset выбирает бит внутри operand width; CF содержит прежнее
значение этого бита, ZF сохраняется, OF/SF/AF/PF undefined. BT сохраняет
destination; BTS/BTR/BTC устанавливают, сбрасывают или инвертируют выбранный
бит. 32-bit writes обнуляют верхние 32 бита. Memory-encoded `0F BA` описаны
в S2-15g; memory forms с register bit index — в S2-15h.

48 handwritten literal fixtures охватывают четыре операции, три widths и
immediate values `0`, `width−1`, `width`, `255`. Fixtures имеют literal
full-state expected answers; materializer перечисляет ответы и не вычисляет
семантику instructions. Срез S2-15f расширяет register-only S2-15c на
immediate register forms, не добавляя BT-family memory support.

Локальные проверки прошли: `cargo test -p od-isa --offline` (75 passed),
`cargo test -p od-runner --offline --test fixture_v2` (14 passed),
`cargo test -p od-testkit --offline` (64 passed),
`cargo test -p od-x86-runner --offline` (8 passed), Linux-target workspace
all-targets check, `cargo fmt --all -- --check`,
`python3 tests/cpu/generate_stage2_bitops_immediate.py --check` (48 current),
`graphify update .` и `git diff --check`. Physical исполнения новых fixtures
не было; `od-x86-runner` admission в этой задаче ограничен register forms.

Ни одна из 48 S2-15f fixtures не запускалась физически. Оставшиеся физические
проверки пользователь попросил объединить в один финальный Mac mini substage
после завершения локальной реализации работ этапа. Ранее выполненные physical
results остаются зафиксированными в своих разделах и evidence.

## S2-15g — BT-family immediate memory destination

Добавлены non-LOCK memory-destination forms BT/BTS/BTR/BTC с encoding
`0F BA /4`, `/5`, `/6`, `/7 ib`, raw imm8 и operand widths 16/32/64. Memory
operand имеет exact width. `BT` только читает и не записывает memory; BTS/BTR/BTC
принудительно выполняют полную-width запись. Для read и write faults side effects
не commit-ятся: CPU state, flags, RAM и retirement остаются неизменными.
Register bit-index memory forms `0F A3/AB/B3/BB` описаны отдельно в S2-15h.

В raw encoding processor игнорирует ненулевые high bits imm8; low bits
выбирают бит внутри operand. При сборке memory bit-string инструкции assembler
может перенести high bits желаемого bit offset в effective-address displacement.
Это различие между raw instruction bytes и assembler encoding указано в Intel
SDM revision 093, Vol.2A, BT pp. 3-112–3-113. Fixtures фиксируют raw bytes и
не полагаются на assembler.

48 literal fixtures покрывают четыре операции и widths 16/32/64; raw imm8 и
полные expected state записаны явно. Вместе с 48 register-immediate fixtures
S2-15f и 12 register-offset fixtures S2-15c это 108 BT-family fixtures.

Локально прошли `cargo test --workspace --offline`, Linux-target workspace
all-targets check, `cargo fmt --all -- --check`, генераторы S2-15 с `--check`
(включая `generate_stage2_bitops_immediate_memory.py`, 48 current),
`graphify update .` и `git diff --check`. Новые fixtures не исполнялись
физически; их проверка ожидает общего финального Mac mini mini-check.
Независимое code/SDM review `gpt-6.1-sol` / medium завершилось без замечаний;
provenance refs перепроверены по Intel SDM revision 093.

## S2-15h — BT-family register-index memory destination

Добавлены BT/BTS/BTR/BTC memory-destination forms `0F A3`, `0F AB`, `0F B3`,
`0F BB`, где bit offset задаётся register. Поддержаны operand widths 16/32/64.
Offset интерпретируется как signed значение ширины operand; номер memory word
вычисляется делением с округлением вниз, а номер бита — по Euclidean modulo.
Эффективный byte delta равен `floor(offset / width) * (width / 8)`. Доступ
точно ограничен operand width. `BT` читает и не пишет; BTS/BTR/BTC всегда
выполняют полную-width запись. Read/write faults не commit-ят CPU, flags, RAM
или retirement.

Native-v2 admission проверяет adjusted memory range и permissions для адреса,
полученного после signed bit-offset adjustment; admission не является
физическим исполнением. LOCK-префикс не относится к этому non-LOCK срезу;
LOCK/atomic contract остаётся в S2-21. FS/GS-prefixed BitScan memory forms
остаются Unsupported, поскольку их bases не моделируются; FS/GS prefixes на
register-source BitScan forms принимаются, так как сегментный override не
меняет register operand. Regression `canonical48_checks_select_ss_only_for_rsp_rbp_bases`
проверяет rejection FS/GS-prefixed memory BitScan operands и acceptance тех же
prefixes с register sources.

72 literal fixtures покрывают четыре операции × три widths × offsets
`0`, `width−1`, `width`, `−1`, `−width`, `−width−1`. Intel SDM revision 093,
Vol.2A bit-index general rule — pp. 3-11–3-12; per-instruction pages: BT
3-112–3-113, BTC 3-114–3-115, BTR 3-116–3-117, BTS 3-118–3-119. Fixtures
содержат literal raw bytes и full-state answers.

Локально прошли `cargo test --workspace --offline`, Linux-target workspace
all-targets check, `cargo fmt --all -- --check`, generators S2-15 с `--check`,
`graphify update .` и `git diff --check`. Физическая проверка 72 fixtures
отложена до общего финального Mac mini mini-check. Независимое
`gpt-6.1-sol` review S2-15h завершилось чисто без замечаний; provenance refs
перепроверены.

## Следующая задача

## S2-16a — direct relative control flow (local)

The long-mode decoder/interpreter now supports the 16 legacy condition codes,
short/near Jcc (`70–7F`, `0F 80–8F`), and direct relative JMP (`EB`, `E9`).
Relative offsets are sign-extended and added to the complete instruction's
next RIP with 64-bit wrapping. A taken noncanonical target faults before RIP
commit; a not-taken conditional branch advances normally. Unsupported-prefix
handling remains bounded to the current default-prefix profile.

The 68 literal cases in `s2_16a_control_flow_expected.json` cover true and false
conditions for every code/encoding length, plus forward/backward short/near
JMP. The exhaustive 32×16 flag truth table, exact byte-fetch failures, canonical
boundary, unsupported prefix, and RIP wrap checks are local tests. Workspace
tests, host and Linux-target all-target checks, formatting, all Stage 2 fixture
generator checks, graph refresh, and diff whitespace check passed. Physical
Mac mini validation remains deferred to the shared final acceptance stage.
Independent `gpt-6.1-sol` review completed cleanly with no findings. These 68
fixtures join the 216 accumulated S2-15 fixtures in the same final Mac mini
acceptance substage; at S2-16c completion the combined total was 433. S2-17a
and S2-17b later added 16 cases, bringing the pool to 449 before S2-16d/e
added 10 more; at that point the total was 459. S2-17c later raised it to 465;
S2-17d added five, bringing the pool to 470; S2-17e later added nine to reach 479, S2-17f raised it to 488, S2-17g to 498, S2-17h to 509, S2-17i brought it to 518; S2-17j added seven to reach 525, S2-17k added eight to reach 533, S2-17l added seven to reach 540, S2-17m added four to reach 544, S2-17n added six to reach 550, and S2-17o adds two to reach 552.
S2-16 remains open for remaining indirect JMP forms and other planned control-flow; this
local slice does not claim completion of the stage.

## S2-16b — SETcc (local)

The long-mode decoder/interpreter now supports all 16 SETcc encodings
(`0F 90–9F /r`) with byte-register and ordinary byte-memory destinations.
`ConditionCode` supplies the same CF/PF/ZF/SF/OF predicates used by Jcc;
ModRM.reg is ignored, REX selects the low-byte aliases and extended registers,
and 66 does not widen the destination. SETcc preserves flags and their
definedness. Memory uses the existing exact-byte operand write; write faults
leave CPU state, RAM, flags, definedness, and RIP uncommitted.

The 79 independent literal cases cover true/false for every condition to both
AL and memory, AH/CH/DH/BH, SPL/BPL/SIL/DIL, R8B/R15B, ignored ModRM.reg,
REX.R/W, 66, address-size override, and RIP-relative addressing. Local tests
also cover exact fetch boundaries, F0/F2/F3 and FS/GS memory rejection,
one-byte writes, and injected/noncanonical write faults. The testkit and CLI
runner validate the full expected CPU state, unchanged flags/definedness, and
all memory regions; the native-v2 admission test enumerates these forms and
checks the empty flag-effect mask (Linux-target compile checked). Independent
`gpt-6.1-sol` review completed cleanly with no findings. Physical Mac mini
validation remains deferred to the shared acceptance stage. The combined
physical pool at S2-16b completion contained 433 fixtures:
216 S2-15, 68 S2-16a, 79 S2-16b, and 70 S2-16c. Indirect JMP and other planned
forms remained open at S2-16d completion; S2-16e later adds the ordinary
memory-target subset. This does not claim completion of S2-16.

## S2-16c — CMOVcc (local)

The long-mode decoder/interpreter now supports all 16 CMOVcc conditions
(`0F 40–4F /r`) with register or ordinary memory sources. The source is read
even when the condition is false; a read fault leaves CPU state, flags, RIP,
and destination uncommitted. After a successful read, a false 32-bit form still
performs the 32-bit destination write, clearing the register's upper dword.

Seventy literal fixtures cover all conditions and the defined state outcomes.
Local workspace tests/checks passed. Independent `gpt-6.1-sol` review completed
cleanly with no findings. Physical Mac mini acceptance remains deferred; these
70 cases join the shared pool of 216 S2-15, 68 S2-16a, and 79 S2-16b fixtures,
for 433 total at S2-16c completion. S2-16 remains open for indirect JMP and
other planned forms.

## S2-16d — register-target near indirect JMP (local)

The decoder/interpreter supports only near indirect JMP `FF /4` with a
register r/m64 target and `REX.B`. Three literal fixtures cover taken targets
through RAX and R8 plus a literal `#GP` case for a noncanonical target. Local
tests also cover decode/budget behavior and fault no-commit. Workspace checks
passed; independent `gpt-6.1-sol` medium review completed cleanly. Linux runner
native tests were not executed because macOS `ld` rejects the GNU-specific
flags; the Linux-target check passed. Physical Mac mini validation remains
deferred to the shared acceptance substage. At S2-16d completion, the pool was
452 fixtures:
216 S2-15, 68 S2-16a, 79 S2-16b, 70 S2-16c, 3 S2-16d, and 16 S2-17a/b.

At S2-16d completion, memory indirect JMP, CALL, and far transfers remained
open. S2-16e later covers ordinary memory r/m64; FS/GS and out-of-profile
addressing, CALL, and far transfers remain open. S2-16 is not complete.

## S2-16e — ordinary memory-target near indirect JMP (local)

The decoder/interpreter supports ordinary memory r/m64 for near indirect JMP
`FF /4`. It reads the target qword before checking whether the target is
canonical; a read fault or noncanonical-target `#GP` leaves state uncommitted.
Seven literal fixtures cover base addressing, `REX.B`, SIB, RIP-relative, and
address-size-32 effective addresses. FS/GS prefixes and addressing outside the
selected profile remain open. Local checks passed, and independent
`gpt-6.1-sol` medium review completed cleanly. Linux runner tests could not be
linked on macOS; the Linux-target check passed. Physical Mac mini acceptance
remains deferred. At S2-16e completion the pool was 459 fixtures: 216 S2-15,
68 S2-16a, 79 S2-16b, 70 S2-16c, 3 S2-16d, 7 S2-16e, and 16 S2-17a/b. The
later S2-17c addition brings the current total to 465.

CALL, far transfers, FS/GS-prefixed memory targets, and other out-of-profile
addressing forms remain open in S2-16.

## S2-17a — 64-bit GPR PUSH/POP opcodes (local)

The decoder/interpreter supports only the 64-bit GPR opcode forms `50–5F` with
`REX.B`. The slice covers RSP source/destination aliasing and executes against
a bounded guest stack; stack addresses are checked across the full canonical
range. Stack faults report `#SS(0)` and faulting operations leave architectural
state uncommitted. Eight literal fixtures cover the supported opcode slice,
with local fault and aliasing tests. Local checks passed, and independent
`gpt-6.1-sol` medium review completed cleanly. Physical Mac mini validation is
deferred to the shared final acceptance substage. At S2-17a completion, the pool was 441
fixtures: 216 S2-15, 68 S2-16a, 79 S2-16b, 70 S2-16c, and 8 S2-17a.

This does not complete S2-17. At this point immediate and 16-bit PUSH/POP,
flags-stack, memory PUSH/POP, CALL/RET, and indirect control-transfer forms
remained open at S2-17a; later local slices cover only the specific forms listed below.

## S2-17b — 64-bit immediate PUSH (local)

The decoder/interpreter supports only the 64-bit immediate PUSH encodings
`6A ib` and `68 id`, sign-extending the encoded byte or dword to a qword before
the stack write. Eight literal boundary fixtures cover the supported forms.
Local tests exercise immediate boundaries, bounded-stack faults, truncated
fetches, and fault-before-commit behavior. Local checks passed, and independent
`gpt-6.1-sol` medium review completed cleanly. Physical Mac mini validation is
deferred to the shared final acceptance substage. At S2-17b completion, the
pool was 449 fixtures: 216 S2-15, 68 S2-16a, 79 S2-16b, 70 S2-16c, and 16
S2-17a/b. S2-16d/e later add 10 fixtures; at that point total is 459. The
later S2-17c addition brings the pool to 465.

Other immediate and 16-bit PUSH/POP forms, PUSHF/POPF, memory PUSH/POP,
CALL/RET, and indirect JMP/CALL remain open; S2-17 is not complete.

## S2-17c — near relative CALL rel32 (local)

Six literal fixtures cover successful near relative `CALL rel32` and separate
target and stack fault cases. Local tests/checks passed. Independent
`gpt-6.1-sol` medium review found no concrete defects in successful CALL/RET,
the separate fault cases, fixture answers, or native admission. One combined
fault boundary remains unverified: when the target is noncanonical and the
guest stack is invalid/noncanonical at the same time, the winning fault is not
established. The current executor checks target canonicality before entering
the stack helper, but this implementation order is not evidence of the
architectural priority. Intel SDM revision 093, Vol.2A, `CALL` p.3-125 presents
the operation pseudocode and lists target `#GP` separately without settling
this combined-fault case. The dual-fault fixture and expected physical snapshot
have not been created; see [ISSUE-013](../KNOWN_ISSUES.md#issue-013--не-подтверждён-приоритет-target-gp-и-stack-fault-для-near-call).
Physical Mac mini acceptance for the six existing fixtures remains deferred.
The shared pool at S2-17c completion was 465 fixtures; S2-17d later brings it
to 470. The future dual-fault case is not included.

This is an unverified priority limitation, not a confirmed defect. Do not
change runtime behavior based on an assumed winner. The next step is normative
or errata evidence, or a harness-captured dual-fault fixture with its expected
physical outcome included in final acceptance evidence.

## S2-17d — register near indirect CALL (local)

This slice implements only register-source near indirect CALL `FF /2 r/m64`
with `REX.B`. Five literal fixtures cover RAX/R8 targets, budget behavior, and
separate noncanonical-target and stack-fault cases; the corrected fixture
stack sentinel is `A5`. The dual-fault target-plus-invalid-stack case remains
unverified and is not among these fixtures (see [ISSUE-013](../KNOWN_ISSUES.md#issue-013--не-подтверждён-приоритет-target-gp-и-stack-fault-для-near-call)).

Local workspace tests, formatting, generators, and Linux-target checks passed.
Linux runner tests could not be linked on macOS because Apple ld rejects GNU
linker flags. Independent Sol-medium review completed cleanly. Physical
validation is deferred to the shared final Mac mini acceptance substage. The
pool is now 470 fixtures: 216 S2-15, 68 S2-16a, 79 S2-16b, 70 S2-16c, 3
S2-16d, 7 S2-16e, 16 S2-17a/b, 6 S2-17c, and 5 S2-17d. Other 16-bit,
flags-stack, memory PUSH/POP, memory indirect CALL, RET, and far forms
remained open at S2-17d; S2-17e later adds only ordinary memory-source near CALL.

## S2-17e — memory near indirect CALL (local)

This slice implements ordinary memory-source near indirect CALL `FF /2
r/m64`. Execution reads the qword source, validates the target, and then
performs a bounded stack push; faults leave architectural state uncommitted.
Nine literal fixtures cover base, `REX.B`, SIB, RIP-relative and address-size
`67` addressing, budget exhaustion, and separate source `#PF`, target `#GP`,
and stack `#SS` faults. No combined target-plus-invalid-stack fixture or
expected snapshot exists; ISSUE-013 tracks that unresolved priority across
relative, register-indirect, and memory-indirect CALL.

Independent Sol-medium review completed cleanly, and local workspace checks
passed. Linux native tests were not executed. Physical validation is deferred
to the shared final Mac mini acceptance substage. The corpus is now 479
fixtures: 216 S2-15, 68 S2-16a, 79 S2-16b, 70 S2-16c, 3 S2-16d, 7 S2-16e,
16 S2-17a/b, 6 S2-17c, 5 S2-17d, and 9 S2-17e. Other 16-bit, flags-stack,
memory PUSH/POP, RET, and far forms remain open in S2-17.


## S2-17f — near RET imm16 (local)

This slice implements only near `RET imm16` (`C2 iw`) in 64-bit operand mode.
The immediate is added to RSP after the qword return-address pop. Nine literal
fixtures cover immediate boundaries, nested-call cleanup, budget exhaustion,
truncated fetch, and separate target, stack, and read faults with no-commit.
Independent Sol-medium review completed cleanly. Workspace checks, formatting,
all 21 Stage 2 generators, Python checks, and Linux-target checks passed.
Physical Mac mini validation is deferred to the shared final acceptance
substage. The pool is now 488 fixtures: 216 S2-15, 68 S2-16a, 79 S2-16b,
70 S2-16c, 3 S2-16d, 7 S2-16e, 16 S2-17a/b, 6 S2-17c, 5 S2-17d, 9
S2-17e, and 9 S2-17f. Far returns, IRET, 16-bit operand width, and prefix
variants remain open.


## S2-17g — 16-bit GPR PUSH/POP (local)

This slice implements 16-bit GPR PUSH/POP opcodes `50–5F` with `66` and
optional `REX.B`; `REX.W` is absent, so forms without `66` retain the default
qword operand size. PUSH reads the word source before decrementing RSP. POP
post-increments RSP by two before writing the destination low word, including
SP aliasing. Ten literal fixtures (7 success, 3 fault) cover AX and extended
registers, stack alias ordering, and faults/truncated fetch. The carry case
starts with RSP `0x30ffff` and word `0x2468`, producing RSP `0x312468`.
Independent Sol review completed cleanly. Local workspace, formatting, all 22
generators, Python checks, and Linux-target checks passed; Linux native admission
was cross-compiled only. Physical mini validation remains deferred to the shared
final acceptance substage. The pool is now 498 fixtures. Remaining immediate
and memory PUSH/POP forms, flags-stack forms, and other planned stack/control
forms remain open.


## S2-17h — 16-bit immediate PUSH (local)

This slice implements only 16-bit immediate PUSH encodings `66 6A ib` and
`66 68 iw`. The first sign-extends imm8 to a 16-bit value; the second pushes
the literal word. Each operation decrements RSP by two and writes exactly two
stack bytes. Eight successful and three fault fixtures cover signed immediate
boundaries, stack-bottom failure, and truncated immediates with no-commit.
Fixture provenance cites Intel SDM order 253667-093US, Vol.2B PUSH
pp.4-522–4-525. Independent Sol-medium review completed cleanly. Workspace
tests/checks, formatting, generator freshness, Python checks, Linux-target
checks, and native `od-x86-runner` checks passed. Physical Mac mini validation
remains deferred to the shared final acceptance substage. The pool is now 509
fixtures. Memory PUSH/POP, flags, and other planned forms remain open.


## S2-17i — qword memory-source PUSH (local)

This slice implements ordinary memory-source `PUSH r/m64` (`FF /6`). Covered
effective-address forms include base, `REX.B`, SIB, RIP-relative, and address
size 32. The executor reads the qword source before performing the bounded
stack write; faults leave state uncommitted. Nine literal fixtures cover seven
success cases, including budget exhaustion and `[RSP]` as source, plus source
`#PF` and stack `#SS`. Independent Sol-medium review completed cleanly.
Workspace tests/checks, formatting, generator freshness, Linux-target, Python,
diff, and graph checks passed. Linux admission tests were cross-compiled only.
Physical Mac mini validation remains deferred to the shared final acceptance
substage. The corpus is now 518 fixtures. Memory POP, FS/GS-prefixed memory
forms, `66` memory forms, flags, and other groups remain open.


## S2-17j — qword memory-destination POP (local)

This slice implements ordinary qword memory-destination `POP r/m64` (`8F /0`).
The operation reads the qword at old RSP, increments RSP, then computes the
destination effective address using the post-increment state and writes the
value. Six successful fixtures cover base, `REX.B`, SIB, RIP-relative, address
size 32, and `[RSP]` post-increment addressing. A seventh fixture covers a
destination-store `#PF` and asserts unchanged memory, but intentionally omits
CPU post-state assertions: whether the RSP increment commits on this fault is
unresolved in [ISSUE-014](../KNOWN_ISSUES.md#issue-014--не-установлено-состояние-rsp-после-fault-у-memory-destination-pop),
not a confirmed runtime defect. Native admission rejects this fault-only case.
Independent Sol-medium review completed cleanly. Workspace suite/checks,
formatting, 25-generator freshness, and Linux-target checks passed. Physical
validation is deferred to the shared final Mac mini acceptance; the total pool
is now 525 fixtures. Other memory POP forms, including FS/GS-prefixed and
`66` forms, remain open.

## S2-17k — 16-bit memory-source PUSH (local)

This slice implements memory-source PUSH `66 FF /6` (`r/m16`). It reads the
source and writes the stack destination at exactly two bytes each; source EA is
computed before RSP is decremented. Address-size `67` affects only EA
calculation, while RSP remains 64-bit. Eight literal fixtures comprise six
success cases (base, `REX.B`, SIB, RIP-relative, address32, and `[RSP]` source
with high RSP) plus injected source-read `#PF` and stack `#SS` cases, checking
no-commit on faults. Independent Sol-medium review completed cleanly. Workspace
suite/checks, formatting, freshness of 26 generators, Python tests,
Linux-target checks, diff, and graph checks passed. Physical Mac mini validation
is deferred to the shared final acceptance substage; the pool is now 533
fixtures. Other memory POP forms, other `FF` groups, FS/GS memory forms, and memory
`66+REX.W` forms remain open; ISSUE-014 now also covers the word form.

## S2-17l — 16-bit memory-destination POP (local)

This slice implements memory-destination POP `66 8F /0` (`r/m16`). It reads
exactly two bytes at old RSP, stages `RSP + 2`, computes the destination EA from
the incremented state (including `[RSP]`), and writes exactly two bytes. Seven
literal fixtures include six successful addressing cases and one destination-
store `#PF` case. The fault fixture checks the exception and unchanged memory
but intentionally makes no CPU post-state assertion because RSP commit on a
store fault remains unresolved in [ISSUE-014](../KNOWN_ISSUES.md#issue-014--не-установлено-состояние-rsp-после-fault-у-memory-destination-pop); it is not a confirmed runtime defect. Independent Sol-medium review completed cleanly. Full workspace/fmt/Linux-target checks, 22 generator tests, all generator freshness checks, graph, and diff checks passed. Physical Mac mini validation remains deferred to the shared final acceptance; the pool was 540 fixtures after S2-17l. Other memory POP forms, other `FF` groups, and FS/GS memory forms remain open.

## S2-17m — memory stack operands with `66` + `REX.W` (local)

This slice accepts memory-source PUSH `66 48 FF /6` and memory-destination POP
`66 48 8F /0` as qword operations: `REX.W` selects the 64-bit operand despite
the earlier operand-size override. Four literal fixtures cover source memory,
destination memory, PUSH `[RSP]` using the pre-decrement address, and POP
`[RSP]` using the post-increment address. Decoder tests check both selected
widths; the testkit executes each full-state fixture. No memory faults are
asserted by this slice, and ISSUE-014 continues to track POP destination-store
fault post-state for the existing qword and word forms.

The width rule is explicit in Intel SDM revision 093, Vol.2A §2.2.1.2
pp.2-8–2-9: for non-byte operations `66H` is ignored when `REX.W=1`, and Table
2-4 defines `W=1` as 64-bit operand size. PUSH and POP operand tables list the
memory widths at Vol.2B pp.4-522 and 4-398. The decoder accepts the unique
`66` + effective `REX` combinations here; duplicate legacy prefix groups stay
Unsupported under the existing profile. With multiple REX bytes, only the
REX immediately before the opcode is effective, as checked by decoder tests.

Native admission accepts all four fixtures and applies the selected operand
width to bounded source, stack, and post-increment destination ranges. A focused
admission test also checks repeated REX ordering and continued rejection of
LOCK and FS/GS forms. The fixture materializer freshness check and local
executable `od-isa`, `od-testkit`, and host `od-x86-runner` tests passed. The
first independent review found the admission mismatch; after the fix, the
focused repeat review was clean. The
native admission test is Linux x86-64-only; its target all-target check passed,
but this macOS host omits it from executable tests, so it remains for the shared
final acceptance substage. Physical Mac mini validation remains deferred; the
pool was 544 fixtures at S2-17m completion. Other memory POP forms, other `FF` groups, and FS/GS
memory forms remain open.

Все baseline integer forms строки S2-15 в CPU contract локально покрыты
подшагами a–h, кроме register-register XCHG, подтверждённого S2-10. S2-15
остаётся открытым до общего физического mini-check 216 fixtures. F3-prefixed
BSF/BSR forms, включая feature-dependent LZCNT/TZCNT, находятся вне baseline;
memory XCHG и LOCK/atomic contract относятся к S2-21.
Срезы a–h локально покрывают sign-extension, BSWAP, register/immediate/memory
BT-family forms и ordinary BSF/BSR register/memory sources. Все 216 fixtures
S2-15a/b/c/d/e/f/g/h ждут одного общего финального mini-check. Сам S2-15 и
этап 2 остаются открытыми; ISSUE-011/012/013 остаются открытыми. Для S2-14 отдельно не проверялись physical
`#DE` delivery и signal/context mapping. Общий физический acceptance substage
теперь включает 68 fixtures S2-16a, 79 S2-16b и 70 S2-16c: на тот момент 216 + 68 + 79 + 70 = 433 cases. S2-17a/b добавили 16, S2-16d/e — 10, S2-17c — 6, S2-17d — 5, S2-17e — 9, S2-17f — 9, S2-17g — 10, S2-17h — 11, S2-17i — 9, S2-17j — 7, S2-17k — 8, S2-17l — 7 и S2-17m — 4, S2-17n — 6, S2-17o — 2, S2-17p — 1, S2-17q — 1, S2-17r — 1, S2-17s — 1, S2-17t — 2; текущий pool — 558. Dual-fault cases S2-17c/d/e в эти 558 fixtures не входят.


## S2-17n — register stack operands with `66` + REX (local)

This slice covers register PUSH/POP opcodes `50–5F` with operand-size override
`66` and effective REX prefixes. Six literal fixtures check qword selection by
`REX.W`, word selection when the final REX has `W=0` despite an earlier REX.W,
REX.B selection of R8/R9, and preservation of upper register bits on word POP.
Intel SDM revision 093 Vol.2A §2.2.1.2 pp.2-8–2-9 defines the width precedence
and effective final REX; Vol.2B PUSH pp.4-522–4-523 and POP pp.4-398–4-399
define the stack operations. The decoder checks selected widths/registers,
testkit compares complete literal post-state, and native admission validates
the bounded stack accesses. Local `od-isa` decode and `od-testkit` full-state tests
passed; freshness of all 29 generators, formatting, workspace offline check, and
Linux-target all-target check passed. The Linux-only native admission test is
cross-compiled here but cannot execute on this macOS host. Independent Sol-medium
review found no defects and independently recomputed GPR, RSP, flags, and stack
memory answers. Physical Mac mini work remains deferred to the shared final
acceptance substage; the pool was 550 fixtures at S2-17n completion.

## S2-17o — bare POPFQ at CPL3/IOPL0 (local)

This slice accepts only bare long-mode `POPFQ` (`9D`) in the runner's default
Ring3 context. `AccessContext` has CPL but no IOPL selector, so this implementation
does not claim semantics for other CPL/IOPL combinations. Intel SDM revision 093,
Vol.2B POPF/POPFQ pp.4-407–4-410 permits the status flags and DF to be changed
at CPL3/IOPL0 while IF/IOPL remain privileged; this interpreter projection updates
only CF/PF/AF/ZF/SF/DF/OF and preserves bit 1 and every other RFLAGS bit. Source or current
TF, NT, or AC is out of this slice: execution returns `Unsupported` before any CPU,
RIP, RSP, or RFLAGS commit and performs no memory write. A stack read fault also
leaves CPU state and flag definedness unchanged. The stack read itself may appear
in the memory backend trace; this read is documented as out-of-profile observation,
not guest exception delivery.

Two literal fixtures cover updating and clearing the allowed flags, preservation
of bit 1, and exact RSP/stack memory. Unit and fixture-driven tests cover
IF/IOPL/out-of-mask preservation, stack-fault no-commit, and TF/NT/AC Unsupported
without CPU or memory commit. Native admission validates the bounded stack read
and rejects TF/NT/AC fixture inputs. `cargo test -p od-isa -p od-testkit --offline`
passed all 112 and 97 tests; the CLI fixture test passed; generator freshness
passed 28 tests; `cargo fmt --all -- --check`, workspace offline check,
Linux-target all-target check, layout check, and `git diff --check` passed. The
Linux-only native admission test was cross-compiled but not executed on this
macOS host. No Mac mini, SSH, or file-transfer checks were run. The shared pool
is now 552 fixtures. PUSHF and other privilege combinations remain open.


## S2-17p — bare PUSHFQ in long mode (local)

This slice accepts only unprefixed `PUSHFQ` (`9C`) in long mode. Intel SDM
revision 093, Vol.2B PUSHF/PUSHFD/PUSHFQ pp.4-528–4-529 specifies a qword
stack write of `RFLAGS AND 00000000_00FCFFFFH`; RF and VM are cleared in the
stack image, and the instruction does not modify RFLAGS. The interpreter
reuses the checked qword stack write, committing RSP only after the write
succeeds. Unit tests verify the exact mask and unchanged CPU state on a
bounded write failure.

One literal fixture checks the profile flags, DF, fixed bit 1, RF/VM clearing,
full GPR/RSP/RIP, flags definedness, and the entire local memory result. Its
physical comparison mask excludes IF/IOPL and other unmodeled bits that the
ring-3 fixture harness cannot initialize as arbitrary values. Native admission
checks the bounded writable qword stack range. The physical mini-run is deferred
to the shared final acceptance; no SSH or transfer was run. Local full
`od-isa`/`od-testkit` tests, the fixture-driven `od-runner` CLI test, 41 Python
Stage 2 tests (including generator freshness), formatting, workspace offline
check, Linux-target `od-x86-runner` all-target check, layout, graph, and diff
checks passed. The native-admission unit test is Linux-only: it was included in
the target check but could not execute on this macOS host. Independent Sol-medium
review found one incorrect manual page citation (P3); the citation and generated
provenance hash were corrected, and focused repeat review was clean. The common
pool is now 553 fixtures.

## S2-17q — 16-bit PUSHFW in long mode (local)

This slice implements exact `66 9C` (`PUSHFW`) in long mode. Intel SDM
revision 093, Vol.2B PUSHF/PUSHFD/PUSHFQ pp.4-528–4-529 says that the 66H
operand-size override selects the lower 16-bit FLAGS image in 64-bit mode:
RSP is decremented by two, the lower 16 bits are stored unchanged, and RFLAGS
is not modified. Unlike the qword path, this word path does not apply the
RF/VM clearing mask because those bits are above bit 15. Other prefix
combinations remain Unsupported; a REX before 66 is ignored by the shared
prefix decoder, while an effective REX is outside this slice.

One handwritten fixture checks the `66 9C` bytes, two-byte stack write,
canaries, all GPRs, RSP/RIP, flags and flags-definedness. Unit tests cover
decoder boundaries, arbitrary high RFLAGS bits being truncated by the word
write, unchanged RFLAGS, and no CPU commit on bounded write failure. Native
admission requires a writable two-byte range. The common physical mini-check
remains deferred and the pool is now 554 fixtures. No Mac mini, SSH, or
transfer checks were run.

Local verification passed: full offline `cargo test -p od-isa -p od-testkit
-p od-runner`, `cargo check --workspace --all-targets --offline`, Linux-target
`cargo check -p od-x86-runner --all-targets`, `cargo fmt --all -- --check`, all
43 Stage 2 Python tests, freshness checks for every `generate_stage2_*.py`,
`python3 tools/check_layout.py`, and `git diff --check`. The Linux-only
native-admission test was cross-compiled, not executed: attempting to run the
Linux target test on macOS fails at link time because Apple `ld` rejects the
GNU linker flags. Independent gpt-6.1-sol medium review first found one stale
PUSHFQ decoder assertion that still rejected `66 9C`; it was removed, and the
focused repeat review plus a final review of the exact-width native admission
test reported no remaining findings. `graphify update .` completed after the
code change. No physical Mac mini, SSH, or transfer checks were run.

## S2-17r — 16-bit POPFW at CPL3/IOPL0 (local)

This slice implements only exact long-mode `66 9D` POPFW under the runner's
default CPL3/IOPL0 context. Intel SDM revision 093 Vol.2B Table 4-15
pp.4-395–4-396 permits
the documented lower status flags and DF to be loaded while IF/IOPL remain
privileged. The interpreter loads CF/PF/AF/ZF/SF/DF/OF from the word, advances
RSP by two after a successful read, and preserves bit 1 and every other
modeled-out RFLAGS bit. Prefixes other than the exact operand-size override
remain Unsupported.

Source or current TF=1 returns `Unsupported` before CPU commit. Nonzero current IOPL also returns `Unsupported` before stack access, in both interpreter and native admission. NT=1 source or
initial state also returns `Unsupported`: Vol.2B Table 4-15 conflicts with
Vol.1 §7.3.13.2 p.7-22 on 16-bit POPF's NT effect at CPL3/IOPL0. [ISSUE-015](../KNOWN_ISSUES.md#issue-015--разногласие-sdm-по-nt-для-popf-word-на-cpl3iopl0)
records this unresolved profile boundary. The single handwritten fixture uses
TF=NT=0, loads every supported flag, provides IOPL=3 in the source word to
check privilege preservation. The v2 literal comparison checks all GPRs, RIP,
flag-definedness and memory, but compares RFLAGS only through fixture mask
`0xCD7`. A separate local testkit assertion starts with nonzero IF, reserved
bit 3, ID and upper bit 63 (current IOPL remains zero), then checks exact full
RFLAGS preservation while the source IOPL is 3. The initial-IOPL no-commit test
injects a stack-read fault and confirms Unsupported takes precedence. The fixture was
materialized by
[`generate_stage2_popfw.py`](../../tests/cpu/generate_stage2_popfw.py), with the
script hash and Intel SDM revision in fixture provenance. Testkit checks
separately verify TF-source, initial-TF, NT-source, initial-NT, and nonzero-current-IOPL `Unsupported` results leave full CPU state and memory unchanged. Native admission validates a
readable two-byte stack range and rejects these unsupported flag conditions.

Local verification passed: all 118 `od-isa` and 106 `od-testkit` tests,
`cargo check --workspace --all-targets --offline`, Linux-target
`cargo check -p od-x86-runner --all-targets`, `cargo fmt --all -- --check`,
the fixture generator freshness and its 2 Python structural tests, a fixture
run through the interpreter CLI, `python3 tools/check_layout.py`,
`git diff --check`, and `graphify update .`. The Linux-only native admission
test was cross-compiled, not executed on this macOS host; no physical CPU,
Mac mini, SSH, or transfer checks were run. Independent review is pending in
the parent task. The shared pool is now 555 fixtures.



## S2-17s — PUSHFQ/POPFQ effective REX.W overrides 66 (local)

Intel SDM revision 093 Vol.2A §2.2.1.2 specifies that effective REX.W overrides 66H for non-byte operations. S2-17s applies that rule narrowly to 9C/9D: `66 48 9C` and `66 48 9D` use qword semantics; effective REX.W=0 with 66 remains word. No segment/address/group prefixes are admitted, and S2-17r's POPFW NT boundary remains unchanged.

One literal fixture executes `66 48 9C; 66 48 9D` with TF/NT/IOPL=0. It checks full architectural state, final RIP, unchanged RFLAGS, and the qword stack image with an 8-byte comparison mask and surrounding canaries. The testkit additionally seeds unmasked IF/ID for exact POPFQ state checks; RF, TF, NT, AC and IOPL remain clear on this allowed-source path. Native stack admission includes this fixture. The common Mac mini pool is now 556 fixtures; physical checks remain for the unified final acceptance stage.

Local verification passed: targeted `od-isa` PUSHF/POPF tests (6), both fixture-driven `od-testkit` tests (2), Python generator tests (4), Linux-target `od-x86-runner` cross-check, `cargo fmt --all -- --check`, workspace check, fixture/generator freshness, CPU Python suite, layout/diff checks and graphify update. The Linux-only native-admission test could not run on macOS because Apple `ld` rejects Linux linker flags; it was cross-compiled. Independent gpt-6.1-sol medium review is pending. No Mac mini, SSH, or transfer checks were run.


## S2-17t — operand-size-prefixed near RET (`66 C3`, `48 66 C3`) (local)

Intel SDM revision 093 Vol.1 §6.4.7 pp.6-12–6-13 forces near-branch operation size to 64 bits in 64-bit mode and says operand-size prefixes are silently ignored. Vol.2A §2.2.1 p.2-8 says a REX byte is ignored unless it immediately precedes the opcode or 0F; therefore the leading 48 in `48 66 C3` is ineffective and prefix normalization yields the same 66-only state as `66 C3`. Vol.2B RET p.4-566 defines `C3` near RET. The decoder and interpreter admit both normalized forms alongside bare RET; the RET path reads an eight-byte target and commits RSP+8 only after a successful read and canonical-target check. Effective REX (`66 48 C3`), duplicate 66, and other effective prefix states remain Unsupported.

Two handwritten fixtures with literal full-state expected output check the qword target, RSP increment, unchanged RFLAGS and memory, and final RIP. The generator and focused fixture checks validate bytes/provenance; the runner CLI executes both through the interpreter, and native-v2 admission checks only the bounded stack/target path. No native execution, SSH, transfer, or physical CPU check was run. Physical acceptance remains deferred; the shared fixture pool is 558.


## S2-18a — one bare MOVSB iteration (local)

The interpreter supports only bare `A4` in long mode with 64-bit implicit
addresses: read one byte from `[RSI]`, then write it to `[RDI]`. After both
accesses succeed, DF selects wrapping RSI/RDI `+1` or `-1`; flags remain
unchanged and the shared retirement path advances RIP. Segment/address/group
prefixes, REP, A5 and all sibling string instructions remain unsupported.
Architectural references are Intel SDM revision 093 (253667-093US), Vol.2B
MOVS/MOVSB/MOVSW/MOVSD/MOVSQ pp.4-107–4-109.

Five handwritten fixtures use literal state/memory answers. The three
positive cases cover DF=0, DF=1, and exact RSI=RDI source/destination aliasing.
Two fault-only cases identify the implicit read/write addresses; local tests
inject architectural #PF at those addresses and observe Fetch→Read, or
Fetch→Read→Write, with zero retired instructions and unchanged CPU/RAM.
This verifies the interpreter's current local no-commit policy. SDM 093 does
not define whether RSI/RDI/RIP must retain their pre-instruction snapshot
after destination #PF; [ISSUE-016](../KNOWN_ISSUES.md#issue-016--не-установлен-restart-state-movs-после-destination-fault)
records this unresolved hardware boundary. Native admission accepts the
three mapped positive cases and rejects both fault-only inputs. No fault
snapshot is claimed as physical evidence.

The fixture/materializer provenance is handwritten byte and expected-value
constants, serialized without an assembler, emulator, or semantic answer
generator. Generator SHA-256:
`2adc5c9a3507d2ca1f2c88e75bc688c992884c69bd69e01597aadb7c76d2337a`.
The deferred physical acceptance pool S2-15 onward contains 563 cases at the
S2-18a boundary: the prior 558 fixtures plus 3 new MOVSB positive cases and 2
local-only injected-fault cases. Other fixture-directory contents were not
reclassified in this slice; native admission determines the executable subset,
and no total native-positive count is asserted here. All Mac mini execution
remains deferred to the single final acceptance substage.

Local checks passed: `cargo test --offline -p od-isa -p od-testkit` (118 and
110 tests), workspace all-target check, Linux `od-x86-runner` all-target
cross-check, `cargo fmt --all -- --check`, generator freshness,
`python3 tools/check_layout.py`, `git diff --check`, and `graphify update .`.
The Linux native-admission test compiled as part of the cross-check but was
not executed on macOS. Focused post-review verification also passed both
MOVSB testkit tests and fixture freshness. Independent gpt-6.1-sol medium
review found that the initial overlap fixture used adjacent disjoint bytes;
the fixture now aliases RSI=RDI, with unchanged memory and both indices
advancing by one. Focused re-review passed with no remaining findings. No SSH,
transfer, or physical Mac mini checks ran.


## S2-18b — one `66 A5` MOVSW iteration (local)

The interpreter admits the word form `66 A5` in long mode. It reads two bytes
from flat `[RSI]` before writing two bytes to `[RDI]`; after both accesses
succeed, DF selects wrapping RSI/RDI `+2` or `−2`. Flags remain unchanged and
the shared retirement path advances RIP. The accepted decoder state also
includes `48 66 A5`: the general prefix parser discards a REX byte that is not
immediately before the opcode. Effective REX, REP, segment/address overrides,
bare `A5`, and sibling string forms remain Unsupported.

Five handwritten fixtures use literal word bytes and expected state/memory
answers. Three mapped positive cases cover DF=0, DF=1, and RSI=RDI self-copy.
Two local-only cases inject source-read or destination-write `#PF`, checking
Fetch→Read(2 bytes) or Fetch→Read→Write(2 bytes), no retirement, and the local
no-commit policy. Intel SDM revision 093, Vol.2B pp.4-107–4-109 specifies the
copy and ±2 pointer step but does not establish the destination-`#PF` restart
snapshot; ISSUE-016 tracks that open boundary. Native admission accepts the
three mapped fixtures and rejects both fault-only fixtures. A separate
regression executes normalized `48 66 A5` with the same expected word result.

The fixtures and expected values are handwritten constants serialized by
`tests/cpu/generate_stage2_movsw.py`, without an assembler, emulator, or
semantic answer generator. Generator SHA-256:
`b5f2702bbf3206870474e26f9814d42e7cf58069038035a1b6433c67d9eda4bc`. The
deferred S2-15-through-S2-18a pool contained 563 cases; this slice adds 5
(3 positive and 2 local-only fault cases), bringing that pool to 568. Earlier
fixtures were not reclassified, and no total native-positive count is asserted.
Physical Mac mini checks remain deferred to the single final acceptance
substage.

Local verification passed focused `od-testkit` MOVSW tests (2, including the
normalized-prefix regression) and MOVSB regressions (2), Stage 2 Python
harness tests (11), workspace all-target check, Linux-target `od-x86-runner`
all-target check, generator freshness, `cargo fmt --all -- --check`, layout,
diff checks, and `graphify update .`. The native-admission unit test is gated
to Linux and was not executable on this macOS host; it compiled in the Linux
target all-target check. Independent gpt-6.1-sol medium review initially
identified that the first generator draft calculated final data bytes; the
generator now takes explicit handwritten `final_data` literals for every
case. Re-review found no remaining issues and confirmed the updated SHA-256
and fixture freshness. No SSH, transfer, or physical Mac mini checks were
run.


## S2-18c — one bare `A5` MOVSD iteration (local)

The interpreter now admits the default-width dword form, bare `A5`, in long
mode. It reads four bytes from `[RSI]`, then writes four bytes to `[RDI]`;
after both accesses succeed, DF selects wrapping RSI/RDI `+4` or `−4`. Flags
remain unchanged and the shared retirement path advances RIP. The form requires
empty prefix state: effective REX (including REX.W), REP, segment prefixes,
and address-size override are outside this slice. `66 A5` remains the separate
MOVSW form from S2-18b. The shared prefix parser clears a REX byte before a
later legacy prefix; `48 66 A5` therefore remains normalized MOVSW and is not
admitted as the MOVSD form.

Five handwritten fixtures use literal `A5` bytes, state and expected memory
answers. Three mapped positives cover DF=0, DF=1, and a four-byte overlapping
copy. Two local-only injected-`#PF` cases check a four-byte source read or
destination write, the access order, and the executor's no-commit policy.
Intel's current official manual page reports downloadable SDM PDFs at version
093; its Vol.2B MOVS entry defines A5 MOVSD, default 64-bit addressing through
RSI/RDI, `67` addressing through ESI/EDI, and REX.W promotion to MOVSQ. The
manual does not establish the MOVSD destination-fault restart snapshot;
[ISSUE-016](../KNOWN_ISSUES.md#issue-016--не-установлен-restart-state-movs-после-destination-fault)
tracks that boundary. Native admission accepts the three mapped positives and
rejects the two fault-only fixtures. The deferred pool was 568 after S2-18b;
this slice adds 5 cases, bringing it to 573. Mac mini physical execution
remains deferred to the shared final acceptance substage.

The literal values are serialized by `tests/cpu/generate_stage2_movsd.py`,
without an assembler, emulator, or semantic expected-value generator. Its
SHA-256 is
`105cf287a1d247dd144d835ab799d6a9d2c3de10f1549ef055977584f1eca9f5`.
The reference is Intel's [SDM download page](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html)
and its revision-093 Vol.2B MOVS/MOVSD entry.

Local verification passed focused MOVSD tests and the complete `od-isa` and
`od-testkit` unit suites (119 and 114 tests), workspace all-target check,
Linux-target `od-x86-runner` all-target check, generator freshness,
`cargo fmt --all -- --check`, layout check, diff check, and `graphify update .`.
The native-admission test is Linux-gated and compiled in the Linux-target check;
it was not executed on this macOS host. Independent gpt-6.1-sol medium review
found no findings and confirmed the decoder/executor/native prefix boundaries,
four-byte access/DF behavior, literal expected answers, overlap case, and
local-only fault documentation. No SSH, transfer, or physical Mac mini checks
were run.


## S2-18d — one effective `REX.W + A5` MOVSQ iteration (local)

The interpreter admits effective `REX.W + A5` as one qword MOVSQ iteration in
long mode. It reads eight bytes from `[RSI]`, then writes eight bytes to
`[RDI]`; after both accesses succeed, DF selects wrapping RSI/RDI `+8` or
`−8`. Flags remain unchanged and shared retirement advances RIP. Intel SDM
revision 093 Vol.2B, MOVS/MOVSB/MOVSW/MOVSD/MOVSQ pp.4-107–4-110 identifies
`REX.W + A5` as MOVSQ, sets the qword step to eight, and says REX.W promotes
the dword operation to 64 bits. Vol.2A §2.2.1.2 p.2-9 states that effective
REX.W overrides 66 for non-byte instructions; therefore `66 48 A5` is also
MOVSQ. Prefix normalization discards a REX before a later legacy prefix, so
`48 66 A5` continues to normalize to the separately supported MOVSW form.

Seven handwritten fixtures cover DF=0, DF=1, exact RSI=RDI aliasing, a
forward-overlapping eight-byte copy, the `66 48 A5` width override, and local-
only source-read/destination-write `#PF` injections. The fixture-driven test
checks exact eight-byte read/write log entries and complete literal state and
memory answers. Both fault cases assert only the bounded executor's local
no-commit policy; Intel SDM does not define the destination-fault restart
snapshot, which remains tracked by [ISSUE-016](../KNOWN_ISSUES.md#issue-016--не-установлен-restart-state-movs-после-destination-fault).
Native admission accepts the five mapped positive fixtures and rejects the
two unmapped fault fixtures. Effective REX.W with REP, segment, or address-size
override remains outside this slice.

The fixture generator is
[`generate_stage2_movsq.py`](../../tests/cpu/generate_stage2_movsq.py); its
provenance names the handwritten input/output constants and Intel SDM 093.
Generator SHA-256: `0b5ee4f7a936ae972be2778bbe37c35fe4850668c11f8ee9970853915af4012e`.
The source references are Intel's [SDM version page](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html),
[Vol.2A](https://cdrdv2.intel.com/v1/dl/getContent/671199), and
[Vol.2B](https://www.intel.com/content/dam/www/public/us/en/documents/manuals/64-ia-32-architectures-software-developer-vol-2b-manual.pdf).
This slice adds seven cases to the deferred pool of 573 after S2-18c, bringing
it to 580. Mac mini execution remains deferred to the shared final acceptance
substage; no SSH, transfer, or physical checks were run here.

Local verification passed `cargo test --offline -p od-isa -p od-testkit` (119
and 115 tests), workspace all-target check, Linux-target
`od-x86-runner --all-targets` check, `cargo fmt --all -- --check`, fixture
generator freshness, the 11 Stage 2 Python harness tests, layout/diff checks,
and `graphify update .`. The native-admission test compiled for Linux but could
not execute on this macOS host. Independent gpt-6.1-sol medium re-review
confirmed the generator SHA-256 and found no remaining issues.

## S2-18e — one bare `AA` STOSB iteration (local)

The long-mode interpreter admits only bare `AA` STOSB in this slice. It stores
the low byte of RAX (AL) to flat ES:`[RDI]`; after a successful one-byte write,
DF selects wrapping RDI `+1` or `−1`. RSI, RAX, and RFLAGS remain unchanged,
and normal retirement advances RIP. REP, all prefixes, and other STOS widths
remain Unsupported. Intel SDM revision 093 Vol.2B STOS/STOSB/STOSW/STOSD/STOSQ
defines the accumulator source, ES destination and DF-controlled index step.

Four handwritten literal fixtures cover forward and backward movement, an
RSI=RDI alias/canary case, and one injected destination-write `#PF`. The
fixture test checks complete architectural state, exact data bytes, and that
the only data access is a one-byte write. The fault case checks local bounded
memory no-commit behavior only; the restart snapshot after a hardware STOSB
destination fault remains unconfirmed in
[ISSUE-016](../KNOWN_ISSUES.md#issue-016--не-установлен-restart-state-movs-после-destination-fault).
Native admission accepts the three mapped successes and rejects the local-only
fault case and prefixed forms.

`tests/cpu/generate_stage2_stosb.py` serializes handwritten bytes and literal
expected state without an assembler, emulator, or x86-result calculator. Its
SHA-256 is `236e03b683dd637d61cb9fee9702e0ef2530c621caa8ea71d7f4762e2fe2d832`.
The 580-case deferred physical pool after S2-18d gains three natively admitted
positive fixtures and one local-only fault fixture, reaching 584. Across
S2-18a–e the 584-case pool contains 17 mapped string-operation cases admitted
by the native profile and 9 local-only fault cases rejected by admission.
Mac mini, SSH, and physical checks remain deferred to the shared final
acceptance substage.

Local verification passed focused STOSB regressions (2), all `od-isa` and
`od-testkit` tests (119 and 117), workspace all-target offline check,
`od-x86-runner` all-target Linux cross-check, all 40 Stage 2 generator freshness
checks, formatting, layout check, and `graphify update .`. The native admission
test compiled for Linux but was not run on this macOS host. Independent review
by gpt-6.1-sol medium found no findings. The reviewer confirmed the bare-AA
boundary, one-byte AL store, post-success RDI-only update, preserved RSI/RAX/
RFLAGS, fault-only local policy, fixture/canary answers, native admission,
generator hash, and pool counts. Review was static; Linux native execution,
SSH, and physical checks were not performed.

## S2-18f — non-REP LODS B/W/D/Q (local)

The interpreter now admits only bare `AC` LODSB, bare `AD` LODSD, `66 AD`
LODSW, and effective REX.W + `AD` LODSQ in Long64 with implicit flat
DS:[RSI] and address-size 64. For `AD`, effective REX.W overrides 66;
normalization preserves the existing prefix order rule, so `48 66 AD` is word
and `66 48 AD` is qword. Other REX bits do not affect the implicit operands.
REP, LOCK, 67, segment overrides, effective non-W REX for `AD`, and every
prefixed `AC` form are outside the admitted profile. This is an implementation
boundary, not a claim that every rejected legal form raises #UD.

Each successful instruction makes one read of the element width at pre-state
RSI, updates the corresponding low RAX view (byte/word preserve upper bits,
dword zero-extends, qword fully replaces), then wrapping-updates RSI by the
width according to DF. It preserves all other GPRs, flags and definedness,
segments, x87 state, and RAM; RIP advances by decoded length and retirement is
one. Read failure commits no CPU or RAM state. Canonicality uses the existing
DS data-address helper, so a noncanonical source reports #GP(0); missing
fixture mappings remain backend harness errors rather than being converted to
guest exceptions.

Local tests cover 6,144 full-state successful inputs: four widths, both DF
directions, six values (zero, one, all ones, sign bit, `55`, `AA`), two initial
RAX sentinels and all 64 CF/PF/AF/ZF/SF/OF combinations. They assert one width-
exact read, no writes, unchanged RAM and complete expected state. Decoder tests
enumerate 48 rejected prefix cases, all eight REX.W values with/without 66,
both REX/66 orders, and truncated fetch boundaries. Address tests cover eight
alignment residues per width, a qword crossing 4 KiB, the exact readable end,
canonical-range crossing #GP for word/dword/qword DS reads, forward/backward
RSI wrap, and a successful read whose final RSI is noncanonical. Four true
unmapped source reads, four noncanonical sources, injected read faults, and
zero-budget execution preserve the pre-state. The 22
literal v2 fixtures contain 8 positive width/DF cases,
4 unmapped source outcomes, 4 noncanonical #GP cases, 4 budget-zero cases, and
2 prefix-normalization cases. Their generator serializes stated byte/state/
memory constants and does not derive expected values from execution.

Verification passed: all 745 `od-isa` tests, all 189 `od-testkit` tests, all
203 v2 runner CLI fixture tests, targeted LODS and existing MOVS/STOSB/REP
STOSB regressions, formatter check, workspace all-target offline check, all
59 Stage 2 generator freshness checks, LODS generator freshness and `git diff
--check`. `tools/check_layout.py` failed because the managed worktree's
`.git` marker being reported as an unmapped root file and two existing report
links to ignored `work/stage*` manifests; this task did not alter those paths.
The fixture JSON field/profile shape is unchanged; the existing `expected_exit`
string now accepts `harness_error` for that existing backend outcome, and v2
validation permits a zero instruction budget while retaining the 10,000,000
maximum. `compare` still rejects nonarchitectural harness-error results. JIT
and native execution remain unsupported; physical checks remain deferred to
the single final Mac mini acceptance substage. S2-18 remains partial, and
ISSUE-016/019 remain open. Independent gpt-6.1-sol high review found no
remaining issues in decoder/executor admission, commit ordering, fault behavior,
fixture shape, or the revised prefix matrix. Review was static; no native,
SSH, or physical execution was performed.

## S2-18i — non-REP CMPS B/W/D/Q (local)

The Long64 interpreter now admits bare `A6`, `66 A7`, bare `A7`, and effective
REX.W + `A7`, including all `48..4F` variants and the shared REX/66 ordering
normalization. CMPS reads pre-state DS:[RSI] first and ES:[RDI] second, compares
the first value minus the second, then commits arithmetic flags, both
DF-directed wrapping pointers, RIP, and retirement together. A first-read error
prevents the second access. A second-read error preserves the first access in
the local trace but commits no CPU state. Noncanonical addresses on either
operand produce #GP(0); unmapped, permission, and backend errors retain the
HarnessError path unless the backend explicitly returns an architectural fault.
The RSI-first choice is a local bounded-policy decision and is not hardware
priority evidence for two invalid addresses. It makes no MMIO rollback claim.
JIT/native admission remains closed.

Coverage adds the 6,144-case full-state/independent-flag matrix; 64 injected
fault combinations; 48 exact prefix rejects; REX.W normalization; same-address
double reads and overlapping regions; alignment/page/range ends; pointer wrap;
canonical start/crossing and no-commit behavior. Exactly 33 literal schema-v2
fixtures cover eight positives, eight unmapped errors, eight noncanonical
faults, four zero-budget cases, two normalization forms, and three
TEST+CMPSB success/first-fault/second-fault streams. The generator contains
literal expected outcomes. Final local verification passed all 747 `od-isa`
tests, all 206 `od-testkit` tests, all 206 v2 runner fixture tests, workspace
all-target offline check, `cargo fmt --all -- --check`, CMPS generator freshness
and its two Python checks, and `git diff --check`. `tools/check_layout.py`
reported the managed worktree's `.git` marker as an unmapped root file and two
existing report links to ignored Stage 1/2 manifests under `work/`; this change
did not alter those paths. Graphify was updated.
Independent gpt-6.1-sol high review found no findings in decoder/executor
admission, ordered reads, commit behavior, fixtures, or the dual-fault caveat.
Review was static. Mac mini/SSH/native and physical checks were not run and
remain reserved for the single final acceptance substage. The unknown
dual-invalid winner is open as ISSUE-020; ISSUE-016/019 remain open.

## S2-19a — bounded `F3 AA` REP STOSB (local)

This slice admits only exact `F3 AA` in long mode. `RCX=0` completes without a
destination access. For nonzero count, each successful byte store writes AL to
flat ES:`[RDI]`, then commits the DF-directed RDI step and RCX decrement; flags
and other GPRs remain unchanged. One invocation handles at most 1024 successful
iterations, independently of the one-instruction retirement budget. Full
completion advances RIP and retires once. A STOP callback or work-limit exit
between iterations retains partial memory/count/index progress, leaves RIP at
the REP instruction, and retires zero instructions. A memory fault preserves
prior successful iterations while current-iteration RDI/RCX remain unchanged.
This is the documented executor boundary; no general exception-priority claim
is made. Intel SDM revision 093 Vol.2B REP and STOS entries describe the count,
implicit AL/ES:[RDI], DF step, and restart state after completed iterations.

Five handwritten literal fixtures cover zero count, count one, forward and
backward count-three repeats, and an injected middle write fault after one
successful store. The fault case remains local-only. Separate direct tests
stop after two writes and replay from saved RCX/RDI, and exhaust then resume the
1024-iteration work limit. A runner CLI test checks exact complete state and
memory for all four mapped fixtures. The native profile admits those four
mapped small-count inputs, including zero count; it rejects the injected fault,
counts above eight, and extra prefixes. Native admission bounds are not
interpreter semantics.

The fixture generator is
[`generate_stage2_rep_stosb.py`](../../tests/cpu/generate_stage2_rep_stosb.py);
its SHA-256 is `5285aad6ece888856f43fb83c961fcf1930f7db748f418fcbe2c5837a1d747bc`.
The deferred S2-15-onward physical pool grows from 584 to 589: 558 earlier
S2-15–S2-17 cases, 26 prior MOVS/STOS cases, and four native-admitted REP
positives plus one local-only REP fault. Across all string-operation slices,
native admission now includes 21 mapped cases and rejects 10 local-only fault
cases. Physical execution remains deferred to the single final Mac mini
acceptance substage.

Post-fix local checks passed: REP testkit cases (3/3), all `od-runner` v2
fixture tests (25/25), the target-independent synthetic REP ptrace accounting
test (1/1), all `od-isa`/`od-testkit` tests (119/120), workspace all-target
offline check, Linux-target `od-x86-runner --all-targets` check, formatting,
layout, all 41 Stage 2 generator freshness checks, the Stage 2 Python harness
(11/11), and graph refresh. A Linux-gated physical REP test verifies count
three with instruction budget one; it compiled in the Linux-target check but
was not executed on this macOS host. Independent gpt-6.1-sol medium review
found and prompted correction of one native retirement-accounting defect;
follow-up review found no remaining findings. No SSH, transfer, or physical
Mac mini checks were run.

## S2-20a — DIV `#DE` exit snapshots

Two local-only fixture-v2 cases expose the existing byte `DIV BL` divide-error
behavior through the public CLI: a zero divisor and an unrepresentable byte
quotient. Literal expectations check vector 0, unavailable error code and fault
address, faulting RIP at the instruction, zero retired instructions, and
unchanged full GPR/RFLAGS/RIP and code memory. They make no claim about
conflicting-fault priority or native exception delivery. The two cases raise
the deferred S2-15-onward pool from 589 to 591; native acceptance selection is
unchanged, including the string subset of 21 mapped and 10 local-only fault
fixtures. The two divide-error cases were not included in native acceptance
selection or execution; they remain for the shared final Mac mini acceptance stage.

`cargo test -p od-runner --test fixture_v2
divide_error_fixtures_preserve_state_and_report_exact_de_metadata --offline`
passed; the full CLI fixture suite passed 26/26, workspace offline check,
all 42 Stage 2 generator freshness checks, Stage 2 harness (11/11), formatting,
layout, and diff checks also passed. Independent gpt-6.1-sol medium review
found stale pool/contract text and an ambiguous native-selection statement;
both were corrected and re-review found no remaining substantive findings.
The generator is
[`generate_stage2_divide_errors.py`](../../tests/cpu/generate_stage2_divide_errors.py);
native and physical execution were intentionally not run in this slice; the
cases were not selected for native acceptance.

## S2-20b — complete UD2 `#UD` exit

One local-only fixture-v2 case contains the complete `0F 0B` UD2 encoding.
The public CLI test checks vector 6, unavailable error code and fault address,
faulting RIP at the first opcode byte, zero retirement, and unchanged full
GPR/RFLAGS/RIP and code memory. It makes no claim about fetch-fault or invalid-
prefix priority, native exception delivery, or physical execution. The case
was not included in native acceptance selection; the deferred S2-15-onward
pool increases from 591 to 592 while native selection remains unchanged.

[`generate_stage2_ud2.py`](../../tests/cpu/generate_stage2_ud2.py) serializes
handwritten bytes and expected state from Intel SDM revision 093 Vol.2A UD2.
The CLI fixture suite passed 27/27, generator tests 2/2, all 43 Stage 2
generator freshness checks, Stage 2 harness 11/11, workspace offline check,
formatting, layout, and diff checks. Independent gpt-6.1-sol medium review
found no issues. Native and physical execution remain deferred to the shared
final acceptance stage. Result: S2-20b is complete within this local-only
boundary.

## S2-20c — qword IDIV quotient-overflow `#DE` exit

One local-only fixture-v2 case contains literal `48 F7 F9` (`IDIV RCX`) with
`RDX:RAX` set to signed 128-bit minimum and `RCX=-1`, producing an
unrepresentable signed qword quotient. The public CLI test verifies vector 0,
unavailable error code and fault address, faulting RIP at the instruction,
zero retirements, and unchanged full GPR/RFLAGS/RIP and code memory. It makes
no claim about fault priority, native exception delivery, or hardware behavior.
The case was not included in native acceptance selection or execution. The
deferred S2-15-onward pool increases from 592 to 593; the native-selected subset
is unchanged.

[`generate_stage2_idiv_qword_error.py`](../../tests/cpu/generate_stage2_idiv_qword_error.py)
serializes handwritten encoding, register values, and expected state with
provenance to Intel SDM revision 093 Vol.2A IDIV; it does not calculate CPU
semantics. The focused CLI test passed, the full fixture-v2 CLI suite passed
28/28, generator structural tests passed 2/2, all 44 Stage 2 generators were
fresh, and the Stage 2 harness passed 11/11. Workspace offline check,
formatting, layout, and diff checks passed. Graphify rebuilt the code graph
(2589 nodes, 5467 edges, 157 communities). Independent gpt-6.1-sol medium
review found no concrete defects. Native and physical execution remain deferred
to the shared final acceptance stage. Result: S2-20c is complete within this
local-only boundary.

## S2-21a — bare qword memory XCHG

Added only bare REX.W + `87 /r` memory qword XCHG. The required
`GuestMemory::atomic_exchange_integer` API is one backend operation returning
the previous value; the contract requires unchanged memory on error. The
executor writes that old value to the source register only after the operation
succeeds. `FlatMemory` validates the complete read and write spans before any
mutation. This does not claim production-backend semantics, SMP ordering,
native admission, or hardware atomicity.

One literal fixture-v2 case checks `48 87 00` with RAX as both address base and
replacement, using an unaligned qword that crosses adjacent memory regions.
The CLI verifies exact full-register/flags/RIP state, one retired instruction,
the old qword in RAX, and exact post-memory bytes including canaries. Executor
spy coverage verifies one atomic method call with no separate data read/write;
architectural #PF and backend errors preserve CPU state and RAM and retire no
instruction. A FlatMemory test covers adjacent-range validation and
no-mutation-on-error. The case remains local-only and was not added to native
selection. The deferred S2-15-onward pool increases from 593 to 594; the
native-selected subset is unchanged.

The focused CLI, executor, memory, and generator tests passed. The complete
fixture-v2 CLI suite passed 29/29, the Stage 2 harness passed 11/11, all 45
Stage 2 generators were fresh, workspace offline check, formatting, layout,
and diff checks passed. Graphify rebuilt the code graph (2615 nodes, 5536 edges,
142 communities). Independent gpt-6.1-sol medium review found no concrete
defects; follow-up confirmed the REX.W-family wording and scope exclusions.
Native/physical checks remain in the shared final acceptance stage. Result:
S2-21a is complete within this local-only boundary.

## S2-21b — qword register-register XADD

Added only bare effective REX.W + `0F C1 /r` register-register qword XADD.
The executor captures both operand values, calculates the qword sum and ADD
flags with the existing arithmetic helper, writes the old destination value
to the source register, then writes the sum to the destination. This preserves
the sum when both ModR/M fields name RAX. Memory destinations, LOCK, other
widths, CMPXCHG and native/physical claims remain outside the slice.

One literal schema-v2 fixture checks `48 0F C1 C0` (`XADD RAX, RAX`) from
`RAX=0x7fff_ffff_ffff_ffff`, expecting `RAX=0xffff_ffff_ffff_fffe`, flags
`0xc92`, unchanged remaining GPRs and one retired instruction. Its provenance
points to Intel SDM revision 093 Vol.2D XADD p.6-23. The fixture is local-only;
the deferred S2-15-onward pool increases from 594 to 595 and native selection
is unchanged.

Focused decoder/executor tests passed 2/2, the public fixture CLI test passed,
generator structural/freshness tests passed 2/2, full `fixture_v2` passed
30/30, `od-isa` tests passed 123/123, `od-testkit` passed 121/121, and the
workspace offline check, all 46 Stage 2 generator checks, Stage 2 harness
11/11, formatting, layout and diff checks passed. Independent gpt-6.1-sol
medium review found two CPU-contract wording contradictions: XADD was included
under a general RFLAGS-preservation statement, and an S2-10 historical
exclusion did not distinguish later support. Both passages were corrected;
re-review found no remaining findings. Graphify rebuilt the graph (2634 nodes,
5572 edges, 149 communities). Physical checks remain in the shared final
acceptance stage. Result: S2-21b is complete within this local-only boundary.

## S2-21c — qword register-register CMPXCHG

Added only bare effective REX.W + `0F B1 /r` register-register qword
CMPXCHG. The interpreter compares RAX with the destination using the existing
qword CMP flag calculation. Equality writes the source to the destination;
inequality copies the old destination to RAX. All other GPRs and non-arithmetic
RFLAGS are preserved. Memory destinations, LOCK and prefix variants are
Unsupported in this slice.

Two handwritten local-only fixtures cover equal `RAX == RCX` with `RBX` copied
to `RCX`, and unequal `RAX != RCX` with source/destination alias (`RCX, RCX`)
and the old RCX copied to RAX. Both check complete GPR snapshots, defined
arithmetic flags, RIP, one retired instruction and code bytes. Their provenance
references Intel SDM revision 093 Vol.2A CMPXCHG. Deferred S2-15-onward pool
increases 595 → 597; the native-selected subset is unchanged.

Focused decoder and CLI fixture tests passed, as did generator structural and
freshness checks. Full local validation passed: `od-isa` 124/124,
`od-testkit` 121/121, fixture-v2 CLI 31/31, workspace offline check, fmt,
layout, diff check, Stage 2 harness 11/11, and all 47 Stage 2 generators.
Independent gpt-6.1-sol medium review found a stale historical CPU-contract
sentence and an incorrect Vol.2A document number in the fixture provenance;
both were corrected and re-review found no remaining defects. Graphify was
refreshed after implementation. This remains local interpreter evidence only:
native admission, SMP behavior and physical Mac mini execution are not
claimed; physical checks remain in the common final acceptance substage.
Result: S2-21c is complete within this local-only boundary.

## S2-21d — bare qword memory XADD

Added bare effective REX.W + `0F C1 /r` qword XADD with a memory destination.
The decoder rejects non-profile width/prefix forms. `GuestMemory` now exposes
`atomic_fetch_add_integer`, which returns the old value, commits a width-masked
wrapping sum as one backend operation, and leaves memory unchanged on error.
The executor snapshots the source before address calculation, calls this
method once, and writes the old value to the source plus arithmetic flags only
after success. The common commit path updates flag-definedness, RIP and retire
count. On faults/backend errors the spy verifies no source/flags/RIP/retirement
or RAM commit and no separate read/write operation.

One literal local-only fixture uses RAX as both source and address base, with
an unaligned qword split across adjacent regions. It checks wrapping addition,
old-value alias result, arithmetic flags, full register state, and all split
region bytes including canaries. `FlatMemory` validates the whole access before
mutation; its tests cover cross-region success and write-error no-commit. The
API makes no LOCK, SMP ordering, production-backend or hardware atomicity claim.
The fixture is excluded from native admission. Deferred S2-15-onward pool
increases 597 → 598; the native-selected subset remains unchanged.

Focused decoder, executor spy/fault, FlatMemory boundary, generator and fixture
tests passed. Full local validation passed: `od-isa` 125/125, `od-testkit`
122/122, runner `fixture_v2` 32/32, workspace offline check, `cargo fmt`, 48
generator freshness checks, 25 generator tests, Stage 2 harness 11/11, layout
and diff checks. Independent gpt-6.1-sol medium review found no defects.
Graphify refreshed to 2679 nodes, 5673 edges and 147 communities. Physical Mac
mini checks remain deferred to the shared final acceptance substage. Result:
S2-21d is locally implemented within this bounded contract.

## S2-21e — bare qword memory CMPXCHG

Added bare effective REX.W + `0F B1 /r` qword CMPXCHG with a memory
destination. `GuestMemory::atomic_compare_exchange_integer` returns the
observed value and performs compare-and-store as one backend operation. The
contract validates write access even on mismatch and leaves memory unchanged
on any error. The executor snapshots RAX and the source, resolves the
effective address from pre-instruction state, and commits RAX/flags only after
the memory operation succeeds.

Two local-only literal fixtures cover equal and unequal comparisons with an
unaligned qword split over adjacent regions. The unequal fixture aliases RAX
as source and address base. Spy and fault tests check the one-call boundary,
mismatch write validation, and no CPU/RAM commit on errors. The fixtures are
excluded from native admission; no LOCK, SMP, production-backend, or physical
execution claim is made. Deferred pool increases 598 → 600; the native-selected
subset remains unchanged.

Focused decoder/executor/FlatMemory tests, both CLI fixture cases and generator
checks passed. Full local validation passed: `od-isa` 126/126, `od-testkit`
123/123, runner `fixture_v2` 33/33, workspace offline check, `cargo fmt`, all
49 Stage 2 generator freshness checks, 52 generator tests, Stage 2 harness
11/11, layout and diff checks. Independent gpt-6.1-sol medium review found no
defects. Graphify refreshed to 2706 nodes, 5741 edges and 170 communities; a
second refresh after a clarity-only edit found no topology changes. Physical
Mac mini checks remain deferred to the shared final acceptance stage. Result:
S2-21e is locally implemented within this bounded contract.

## S2-21f — LOCK qword memory XADD (private serial RAM only)

Added only `F0 + effective REX.W + 0F C1 /r` with a qword memory destination,
using the existing `GuestMemory::atomic_fetch_add_integer` boundary. The local
fixture covers source/base aliasing, an unaligned qword split over adjacent
regions, wrapping arithmetic, flags and canaries. Decoder rejects LOCK register
XADD and all other locked forms in this slice. The profile is limited to
private, synchronous ordinary RAM with serial access and no concurrent
observers/writers, DMA, shared aliases, delayed writes or MMIO; this API does
not establish full x86 LOCK ordering or SMP semantics. Native selection remains
unchanged; deferred pool increases 600 → 601. The unaligned local fixture
does not establish hardware #AC or split-lock policy. Full local validation
passed: `od-isa` 128/128, `od-testkit` 123/123, runner `fixture_v2` 34/34,
workspace offline check, `cargo fmt`, 50 generator freshness checks, 54
generator tests, Stage 2 harness 11/11, layout and diff checks. Independent
gpt-6.1-sol medium review found no defects. Graphify AST refresh completed at
2725 nodes, 5778 edges and 171 communities. The CLI semantic documentation
refresh attempt failed because no LLM API key is configured; semantic doc
refresh remains pending. The code graph is current. Physical checks remain in
the shared final acceptance stage.

## S2-21g — bare MFENCE on opted-in private synchronous RAM

Added only unprefixed `0F AE F0`. The required `GuestMemory::mfence` call
defines completion of prior loads/stores before following accesses; it has no
default implementation. `FlatMemory` opts in because it owns canonical bytes
and commits writes synchronously, while the code-only fragment backend returns
a backend error. The interpreter performs no data access and changes no GPR,
RFLAGS or memory. RIP and retirement advance only after successful completion.
If the backend errors, the fence remains unretired and earlier retired stores
remain committed.

One local-only fixture executes store → MFENCE → load and checks registers,
flags, RIP, retirement and bytes. Spy/failure tests verify one backend call,
fetch-only accesses for an isolated fence and no retirement on backend error;
decoder tests accept only the bare `F0` ModR/M form in this profile and verify
three-byte fetch boundaries. Other `0F AE` forms and prefixes remain unsupported
in this profile without an architectural invalid-encoding claim. This does not
advertise SSE2 in CPUID, expand native admission, or claim shared-memory, MMIO,
DMA, SMP or physical ordering. The deferred pool grows 601 → 602; native
selection is unchanged. ISSUE-017 remains open for general LOCK ordering.

Normative reference: Intel SDM revision 093 (order 253667-093US), Vol.2B
MFENCE p.4-15, [official Intel PDF](https://cdrdv2.intel.com/v1/dl/getContent/671241).

Local validation passed: `od-isa` 129/129, `od-testkit` 124/124, runner
`fixture_v2` 35/35, focused generator freshness/provenance 2/2. Physical checks
remain in the shared final acceptance stage.

## S2-22a — one local-only FS-base load form

Added `s2-long-fs-base-v1`, a schema-v2 profile whose state requires canonical48
`fs_base`. The existing `s2-long-integer-v1` input and result field sets remain
unchanged; an existing S2-21g fixture retains its pinned input SHA-256 and exact
old-profile JSON round trip. The interpreter accepts only `64 48 8B 00`
(`MOV RAX, qword ptr FS:[RAX]`): it adds FS.base to the pre-instruction RAX
offset, validates the full qword linear range, reads through the existing flat
GuestMemory API, then commits RAX and RIP. LEA remains offset-only. Canonicality
faults report `#GP(0)` before a data access; backend read errors and truncated
fetch preserve CPU state and FS.base. Tests cover the alias, FS-base observation
and comparison, adjacent FS/GS decode rejections, profile validation and native
fail-closed behavior. The new profile and the exact FS load are excluded from
native admission; no memory model, selection or shared hardware semantics were
added. Pool count is 602 → 603; native-selected subset is unchanged.

Sources: Intel SDM revision 093 (253665-093US), Vol.1 §§3.3.7–3.3.7.1
pp.3-10–3-11 and §3.4.2.1 p.3-15; Vol.2B MOV pp.4-28–4-31 (253667-093US),
[official Vol.2B PDF](https://cdrdv2.intel.com/v1/dl/getContent/671241). See
[CPU contract](../specs/CPU_CONTRACT.md#s2-22a--единственная-fs-base-форма-в-отдельном-локальном-профиле)
and [fixture format](../testing/TEST_CASE_FORMAT.md#version-2-long-mode-integer-states).

Validation passed: `cargo fmt --all -- --check`, full workspace check and tests
offline (`od-isa` 130, `od-testkit` 127, runner unit tests 11 and fixture-v2
integration tests 35), Linux x86-64 target `cargo check --all-targets`, all 75
Python CPU tests, fixture freshness, layout and diff checks. The Linux-target
native test binary cannot be linked by the macOS host linker, so this is a
compile check only; no Linux native runtime or physical Mac mini test was run.
Independent gpt-6.1-sol medium review and re-review are clean. Graphify updated
the code graph to 2777 nodes; the semantic documentation refresh was not run.
The deferred pool is 603 fixtures and native-selected membership is unchanged;
physical checks remain in the shared final acceptance stage.

## S2-22b — one local-only GS-base load form

Added `s2-long-gs-base-v1` while preserving the integer and FS-base profile wire
shapes. The interpreter accepts only `65 48 8B 00` (`MOV RAX, qword ptr
GS:[RAX]`), adds the full GS base to the pre-instruction offset, validates the
complete qword canonical range, then reads from flat fixture memory and commits
RAX/RIP. Literal coverage checks aliasing and data, `#GP(0)` before data access,
unmapped data `#PF` without commit, injected backend read error, truncated
fetch, and nearby unsupported forms. Native execution returns Unsupported
before mapping or spawning a tracee for the new profile; legacy-profile native
admission rejects the GS-prefixed instruction after decode. This does not
model selectors, descriptors, privilege, paging delivery, or guest GS capture.
The deferred pool is 603 → 604; native-selected membership is unchanged.

Provenance correction: earlier S2-22a/b text identified the MOV source as
Vol.2A. The cited opcode pages are in Intel SDM revision 093 Vol.2B,
order 253667-093US, pp.4-28–4-31; both fixture generators and serialized
fixture provenance now use the official Vol.2B PDF.

Source: Intel SDM revision 093 (253665-093US), Vol.1 §§3.3.7–3.3.7.1
pp.3-10–3-11 and §3.4.2.1 p.3-15; Vol.2B MOV pp.4-28–4-31 (253667-093US),
[official Vol.2B PDF](https://cdrdv2.intel.com/v1/dl/getContent/671241). See the
[CPU contract](../specs/CPU_CONTRACT.md#s2-22b--единственная-gs-base-форма-в-отдельном-локальном-профиле)
and [fixture format](../testing/TEST_CASE_FORMAT.md#version-2-long-mode-integer-states).

Validation passed: `cargo fmt --all -- --check`; local offline tests for the
four affected Rust packages (od-isa 132, od-testkit 129, od-runner unit 11 and
fixture integration 36, od-x86-runner supervisor 8 and trace 1); Linux
x86-64-target `cargo check --all-targets`; all 77 Python CPU tests; GS fixture
freshness; layout check; and graphify update. Linux native tests and physical
Mac mini execution were not run; the Linux target was compile-only because the
host is macOS. Independent gpt-6.1-sol review found no confirmed issues.

## S2-22c — one local-only FS-selector read

Added `s2-long-fs-selector-v1` with required, observable FS.base and 16-bit
FS.selector fields. The interpreter accepts only `48 8C E0` (`MOV RAX, FS`),
zero-extends the selector into RAX, advances RIP by three and retires one
instruction while preserving flags, FS.base, selector and all other GPRs.
This reads the selector already present in state; it does not fetch or validate
a descriptor, load a selector, or model privilege/segment checks. Existing
integer, FS-base and GS-base profile wire shapes remain unchanged. The legacy
fragment adapter rejects nonzero FS selectors.

The literal fixture checks the zero-extension, full GPR snapshot, flags, RIP,
FS.base and selector. Decoder tests accept only this encoding and check truncated
fetches; interpreter tests verify state preservation. New selector profile
execution fails closed before native mapping/tracee creation, and legacy native
admission rejects the decoded instruction. The deferred pool grows 604 → 605;
native-selected membership is unchanged. S2-22 remains open for descriptor,
privilege, stack and mode-specific segmentation behavior.

Source: Intel SDM revision 093, Vol.1 §3.4.2 p.3-13 (253665-093US; segment
registers contain selectors), [official Vol.1 PDF](https://cdrdv2.intel.com/v1/dl/getContent/671436);
Vol.2B MOV pp.4-28–4-31 (253667-093US; REX.W + 8C /r zero-extends the selector
into r64), [official Vol.2B PDF](https://cdrdv2.intel.com/v1/dl/getContent/671241).

Validation passed: `cargo fmt --all -- --check`; workspace
`cargo check --workspace --all-targets --offline`; local tests for od-isa (134),
od-testkit (130), od-runner (11 unit + 37 fixture integration), and the
host-runnable x86-runner tests (8 supervisor + 1 trace); all 79 Python CPU tests;
fixture freshness, layout and diff checks. A Linux-target `--all-targets` compile
check also passed, which type-checks the Linux-only native guard tests but does
not execute them. The macOS test run does
not include those Linux-only tests. No Linux native runtime or physical Mac mini
execution was performed; physical checks remain in the shared final acceptance
substage. Independent gpt-6.1-sol medium review of S2-22a/b provenance and
S2-22c found no confirmed defects.

## S2-22d — one local-only GS-selector read

Added `s2-long-gs-selector-v1` with required, observable canonical48 GS.base
and 16-bit GS.selector. The interpreter accepts only `48 8C E8`
(`MOV RAX, GS`), zero-extends the selector into RAX, advances RIP by three and
retires one instruction while preserving flags, GS state and all other GPRs.
Direct core-state coverage also checks that FS base/selector remain independent.
No descriptor lookup/load/validation, privilege checks, or data-memory access
is modeled. Older profiles continue to omit `gs_selector`, and the FS-selector
profile wire shape is unchanged. The legacy fragment adapter rejects nonzero
GS selectors.

The literal fixture checks RAX, full GPR state, flags, RIP, GS.base and
GS.selector. Profile tests check required fields, explicit null, 16-bit range,
result mismatch, and unchanged legacy shapes. Decoder tests accept only the
exact FS and GS selector forms, reject nearby encodings, and check truncated
fetch. Native execution rejects the new profile before mapping/tracee creation;
legacy native admission rejects the decoded GS-selector instruction. The
deferred pool grows 605 → 606; native-selected membership is unchanged. S2-22
remains open for descriptor loads, privilege, stack and mode-specific
segmentation behavior.

Source: Intel SDM revision 093, Vol.1 §3.4.2 p.3-13 (253665-093US; segment
registers contain selectors), [official Vol.1 PDF](https://cdrdv2.intel.com/v1/dl/getContent/671436);
Vol.2B MOV pp.4-28–4-31 (253667-093US; REX.W + 8C /r zero-extends the selector
into r64, ModR/M E8 selects GS:RAX), [official Vol.2B PDF](https://cdrdv2.intel.com/v1/dl/getContent/671241).

Validation passed: `cargo fmt --all -- --check`, `cargo check --workspace
--all-targets --offline`, `cargo test --workspace --offline`, all 81 Python
CPU tests, all 55 stage2 generator freshness checks, `tools/check_layout.py`,
and diff checks. The GS fixture integration test asserts the complete 16-GPR
array against the literal initial array with only RAX replaced. Linux-target `cargo check -p od-x86-runner --all-targets
--target x86_64-unknown-linux-gnu --offline` passed, compiling Linux-only native
tests but not executing them. Host-runnable x86-runner tests passed. Independent
gpt-6.1-sol medium review found no remaining findings, including after the
full-GPR assertion was added. No Linux native runtime, SSH, or physical Mac mini
execution was performed; physical checks remain in the shared final acceptance
substage.

## S2-28a — lossless Long64 x87 state snapshot (partial)

Added `od-isa::state::X87State` as an isolated raw state model: eight physical
10-byte registers, full 16-bit physical tag word, control/status words, 64-bit
FIP/FDP offsets, and the 11-bit FOP. Logical TOP is derived from FSW bits
13:11; ST(i) maps to `(TOP+i)&7`; tags remain indexed by physical register.
The snapshot wire object serializes each register as exactly 20 lowercase hex
digits in increasing memory-address order and rejects out-of-width words or
FOP. Empty tag values do not erase payload bytes.

The new `s2-long-x87-state-v1` profile is isolated from existing integer
profiles; `None` means the profile excludes x87, and old profiles continue to
omit the x87 key. In the original S2-28a slice, this profile admitted only a
one-byte NOP with budget 1 and a stop RIP immediately afterward. The fixture
adapter loaded the x87 image into `LongModeState`; the local interpreter
executed the admitted NOP with that state, and the result adapter serialized
the executed state. Comparison reports any x87 difference. The fragment
adapter rejects `Some(x87)`, and native execution remains unsupported because
its adapter cannot observe x87 state. S2-30a and S2-30b below add one exact
FNINIT form and one exact FNSTSW AX form;
no general x87 or FPU feature claim was added.

`X87State::fixture_seed()` is deterministic non-architectural fixture data.
`fninit()` is the separately named architectural transition: FCW `0x037f`, FSW
`0`, FTW `0xffff`, FIP/FDP/FOP `0`, with all register payloads preserved. Raw
FSW/FCW bits are the only pending-exception representation; delivery is not
implemented. The Long64 wire contract omits FCS/FDS, following the SDM's
64-bit save form with REX.W. INIT is kept distinct: Intel Vol.3A §11.1.1
Table 11-1 note 5 says INIT does not change x87 FPU/MMX state.

Local verification passed: TOP mapping across all eight values, tag encodings,
FNINIT payload preservation, raw-state wire roundtrip and malformed rejection,
mutation-sensitive comparison, `IntegerState`/`LongModeState` adapter
roundtrip, explicit fragment rejection, legacy profile wire/digest
compatibility, and one-NOP runner preservation through the executed
`LongModeState`. The S2-30a extension is recorded below.

## S2-30a — bare FNINIT (bounded local slice)

The local long-mode decoder accepts only the exact bare bytes `DB E3`; prefixes,
other `DB` forms, and `9B DB E3` FINIT remain Unsupported. The interpreter
requires an existing `Some(X87State)`, calls the existing `X87State::fninit()`
transition, advances RIP by two, and retires one instruction. FCW, FSW, full
tag word and FIP/FDP/FOP reset to `037F/0/FFFF/0/0/0`; all eight register
payloads, GPRs and RFLAGS remain unchanged. Missing x87 state is Unsupported;
this local profile assumes x87 is available and does not inspect CR0.EM/TS or
deliver #NM. Native profile execution and instruction admission both remain
closed. No FPU feature is advertised.

The existing `s2-long-x87-state-v1` profile admits exactly one NOP, one bare
FNINIT, or one bare FNSTSW AX, with budget 1 and stop RIP immediately after the instruction. The
runner compares the entire raw x87 state. Local tests cover arbitrary seeded
payload preservation, environment reset, decode neighbors and prefixes,
truncated/faulting fetch without commit, missing state, STOP and budget
boundaries, fixture validation, and native rejection. These checks do not
provide physical CPU or native FPU observation.

Normative source: Intel SDM revision 093, Vol.2A, “FINIT/FNINIT—Initialize
Floating-Point Unit,” pp.3-345–3-346, [official Vol.2A PDF](https://cdrdv2.intel.com/v1/dl/getContent/671199).
It identifies `DB E3` as FNINIT and `9B DB E3` as FINIT with WAIT; FNINIT
resets FCW to `037F`, clears FSW, empties all tags, clears instruction/data
pointers and opcode, leaves data-register payloads unchanged and generates no
x87 floating-point exceptions.

Validation passed: `cargo test -p od-isa fninit --offline`,
`cargo test -p od-testkit x87_state_profile --offline`,
`cargo test -p od-testkit x87_state_profile_admits --offline`, and
`cargo test -p od-runner --test fixture_v2
local_x87_state_profile_runs_fninit_with_full_environment_reset --offline`.
The host macOS target does not include the Linux-only `od-x86-runner` tests;
the FNINIT native-rejection test was added for that target but remains a
compile-only check. `cargo test --workspace --offline`,
`cargo fmt --all -- --check`, `cargo check --workspace --all-targets --offline`,
Linux-target `cargo check -p od-x86-runner --all-targets --target
x86_64-unknown-linux-gnu --offline`, and `git diff --check` passed. The layout
checker reports the pre-existing untracked `.serena/` directory as an unmapped
root directory; it was left untouched. Independent gpt-6.1-sol medium review
found no confirmed defects. No physical/Mac mini check was performed. S2-28,
S2-29 and umbrella S2-30 remain open.

```sh
cargo test -p od-isa x87 --offline
cargo test -p od-isa fninit_resets --offline
cargo test -p od-testkit x87 --offline
cargo test -p od-testkit fs_base_profile_is_required_and_old_wire_shape_stays_unchanged --offline
cargo test -p od-runner --test fixture_v2 local_x87_state_profile_preserves_all_raw_state_across_one_nop --offline
cargo check -p od-x86-runner --all-targets --offline
cargo fmt --all -- --check
cargo check --workspace --all-targets --offline
cargo test --workspace --offline
python3 tools/check_layout.py
git diff --check
```

Normative basis: Intel SDM revision 093, Vol.1 §§8.1.2, 8.1.3.1, 8.1.5,
8.1.7–8.1.9 and 8.5; Vol.2A “FINIT/FNINIT—Initialize Floating-Point Unit,”
pp.3-345–3-346; Vol.3A §11.1.1 Table 11-1 note 5. Official manuals are linked
in [CPU_CONTRACT](../specs/CPU_CONTRACT.md#local-profile-s2-28a-s2-long-x87-state-v1).
The x87 umbrella remains open: the profile does not execute x87 operations,
observe native FPU state, implement exception delivery, or integrate x87 with
the fragment/JIT state contract. No physical or Mac mini check was performed.

## S2-29a — lossless binary80 raw representation and classification (partial)

Added immutable `od-isa::x87::X87Binary80`, a ten-byte memory-order wrapper
with exact accessors for sign, exponent, significand, J/integer bit, fraction,
and Q bit. The classifier distinguishes zero, subnormal, normal, infinity,
quiet/signaling NaN, pseudo-denormal, unnormal, pseudo-infinity, and
quiet/signaling pseudo-NaN. It does not interpret `X87Tag`, normalize values,
or modify any payload/state. No arithmetic, rounding, exception handling,
instruction execution, wire changes, dependencies, or host `f64` conversion
were introduced.

Hand-authored literal vectors assert fields and classes for signed zero,
subnormal and normal boundaries, infinities, NaN payload boundaries,
negative QNaN indefinite, pseudo-denormals, unnormal exponent edges,
pseudo-infinity, and both pseudo-NaN classes. An 80-case walking-one check
checks every memory bit's extracted field; a separate regression classifies
register payloads and asserts complete `X87State` and physical tags remain
unchanged. Local verification results follow.

Local checks passed: `cargo test -p od-isa x87 --offline` (6 tests), the
existing `od-testkit` raw x87 round-trip test (1), and the one-NOP runner
snapshot-preservation regression (1). `cargo check --workspace --all-targets
--offline`, `cargo fmt --all -- --check`, `python3 tools/check_layout.py`,
`git diff --check`, and `graphify update .` also passed. No physical CPU or
performance measurement was needed or claimed for raw classification.

Normative basis: Intel SDM revision 093, Vol. 1 §4.2.2/Table 4-3 and
§8.2.2/Table 8-3 ([253665-093US](https://cdrdv2.intel.com/v1/dl/getContent/671436)).
S2-29 and the S2-28 umbrella remain open; this slice makes no physical or
performance claim. The follow-up scope is recorded as [ISSUE-019](../KNOWN_ISSUES.md).

## S2-29b — finite normalization and shift-jam primitives (partial)

Added `X87Finite`, returned only for raw zero, subnormal, pseudo-denormal and
normal classes. Nonzero significands have their leading bit at bit 63, with
the associated exponent adjusted during subnormal normalization. The view
retains sign and original raw class, so pseudo-denormal and subnormal
provenance remains available while the `X87Binary80` bytes and classifier stay
unchanged. Zero retains its sign and uses zero exponent/significand. Added
integer-only `shift_right_jam()` and low-bit `X87Grs` extraction. No arithmetic,
rounding, exception/status mutation, instruction execution, host floating
point, dependencies, or performance claim was added.

Literal unit tests cover signed zero, smallest/largest subnormal,
pseudo-denormal, minimum normal, maximum finite, rejected nonfinite/unnormal
classes, shift distances 0/1/63/64/65, sticky behavior and GRS bit extraction.
`cargo test -p od-isa x87 --offline` passed (8 tests).
`cargo check --workspace --all-targets --offline`,
`cargo fmt --all -- --check`, `python3 tools/check_layout.py`, and
`git diff --check` passed. `graphify update .` completed after the source and
documentation updates. No physical CPU check was required or performed; no
performance result is claimed. The umbrella S2-29/S2-28 remain open, and
ISSUE-019 tracks the remaining work.

## S2-29c — generic u128 right-shift-jam (partial)

Added standalone `shift_right_jam_u128(value, distance)`, preserving jammed
sticky state across right shifts and returning only the sticky bit for
distances at least 128. Its documented normalized-significand bridge is
`(significand as u128) << 3`; callers extract GRS from only the low three bits
and retain the aligned significand with `aligned >> 3`. No left-shift recovery,
rounding, arithmetic, exceptions, instruction execution, result framework,
or precision claim is provided.

Independent literal vectors cover zero, bit 127, `u128::MAX`, all required
distances 0/1/2/3/63/64/65/66/67/127/128/129/`u32::MAX`, repeated sticky
propagation, and bridge cases for signed zero, negative subnormal, normal and
negative pseudo-denormal with source-class metadata. The focused
`cargo test -p od-isa x87 --offline` command passed (11 tests),
`cargo check --workspace --all-targets --offline`,
`cargo fmt --all -- --check`, `python3 tools/check_layout.py`, and
`git diff --check` passed. No physical Mac mini check was performed or
required for this integer helper. ISSUE-019 remains open for arithmetic,
rounding, exceptions, instructions, and state integration.

## S2-29d — ordered finite-pair alignment (partial)

Added `align_finite_pair(lhs, rhs)`, returning a shared exponent and two
aligned `u128` significands in input order. Nonzero pairs use the larger
exponent; one zero uses the other operand's exponent; two zeros use exponent
zero. Zero contributes a zero significand. Nonzero operands use the existing
three-bit significand bridge and `shift_right_jam_u128`. The helper preserves
the input objects' signs, source classes, and source values; it neither sorts
by magnitude nor changes either sign.

Literal local vectors cover gaps 0/1/2/3/4/63/64/65/66/67/127/128/129,
minimum subnormal against minimum normal and maximum finite, maximum subnormal
against minimum normal, pseudo-denormal against its matching normal with
distinct provenance, both operand orders, negative exponents, and signed-zero
pairs. The jammed bit is only lost-tail information; it is not exact magnitude
or architectural inexact status. Future subtraction or left renormalization
requires a separate information-retention proof. Addition/subtraction,
result normalization, rounding, precision control, exceptions/status, special
values, guest instructions, and native snapshot remain excluded.

Local verification passed: `cargo test -p od-isa x87 --offline`,
`cargo fmt --all -- --check`, `cargo check --workspace --all-targets --offline`,
`python3 tools/check_layout.py`, `git diff --check`, and `graphify update .`.
No physical Mac mini check was performed or required for this integer helper.
S2-29 and the S2-28 umbrella remain open under ISSUE-019.

## S2-29e — exact same-exponent finite sum (partial)

Added `add_finite_same_exponent(lhs, rhs)`, which returns the exact signed
`i128` coefficient when nonzero finite normalized inputs have matching
exponents, including exponent zero.
Its represented value is `coefficient * 2^(exponent - 63)`. It reads the
original significands directly and does not consume jammed alignment data.
Zeros are neutral; two zeros return `(0, 0)`. Carry and cancellation remain
unnormalized, and no result source class is produced. Unequal nonzero
exponents return `None`.

Manual literal vectors cover positive/negative carry and maximum sums,
adjacent-significand cancellation to `+1` and `-1`, exact cancellation, negative
exponents, zeros in both orders, matching pseudo-denormal/normal values with
distinct provenance, unequal exponents, and source immutability. Binary80
packing, result normalization, unequal-exponent arithmetic, rounding/PC/RC,
signed-zero selection, exceptions/flags, and instruction execution remain
outside this slice. No performance claim is made.

Local verification passed: `cargo test -p od-isa x87 --offline` (16 tests),
`cargo fmt --all -- --check`, `cargo check --workspace --all-targets --offline`,
`git diff --check`, and `graphify update .`. `python3 tools/check_layout.py`
could not complete because the pre-existing untracked root directory `.serena/`
is unmapped. No physical Mac mini check was performed or required for this
integer helper. S2-29 and the S2-28 umbrella remain open under ISSUE-019.

## S2-29f — exact coefficient normalization (partial)

Added `normalize_exact_coefficient(exponent, coefficient)` for the exact value
`coefficient * 2^(exponent - 63)`. A nonzero coefficient returns its sign,
normalized exponent, and an exact `u128` magnitude whose leading one is at bit
127. The helper uses the unsigned magnitude, including for `i128::MIN`, and
only shifts left; `None` reports only final `i32` exponent overflow. Zero at
any input exponent returns `(false, 0, 0)`.

Literal vectors cover `+1`/`-1`, `2^63`, `2^64`, `2^64 + 1` with the low bit
preserved, `2^65 - 2`, `i128::MAX`/`i128::MIN`, exponent-edge success and
overflow, and zero at both exponent limits. Composition with S2-29e covers
positive and negative adjacent cancellation, exact zero, the maximum-finite sum
normalizing to exponent 16384, and adjacent-significand difference at the
minimum-normal exponent. Source values, raw encodings, and provenance remain
unchanged. Binary80 packing, GRS/rounding/clipping, flags, instructions, and
CPUID remain outside this helper; ISSUE-019 remains open.

Local verification passed: `cargo test -p od-isa x87 --offline` (19 tests),
`cargo fmt --all -- --check`, `cargo check --workspace --all-targets --offline`,
`git diff --check`, and `graphify update .`. `python3 tools/check_layout.py`
stopped at the pre-existing unmapped root directory `.serena/`; it was left
untouched. No physical Mac mini check was performed or required for this exact
integer helper. S2-29 and the S2-28 umbrella remain open under ISSUE-019.

## S2-29g — exact finite significand product (partial)

Added `multiply_finite_significands(lhs, rhs)`, which multiplies the original
finite significands as unsigned `u128` values and returns the XOR sign, checked
scale exponent `lhs.exponent + rhs.exponent + 1`, and the full exact product.
The result represents `(-1)^sign * magnitude * 2^(exponent - 127)`; its leading
one may be at bit 126 or 127, and it is neither normalized nor narrowed
through `i128`. A zero input returns `(false, 0, 0)`; `None` reports only scale
exponent overflow. Jammed alignment is not consumed and source values,
encodings, and provenance remain unchanged.

Independent literal vectors cover minimum significand squared, maximum squared,
mixed boundary products, sign combinations, commutativity, signed zero pairs,
exponent min/max overflow and successful cancellation of the exponents, plus
products from normalized raw pseudo-denormal and normal values with preserved
provenance. No normalization, rounding, packing, architectural overflow or
underflow, exceptions/status, instructions, or native execution is included.

Local verification passed: `cargo test -p od-isa x87 --offline` (21 tests),
`cargo fmt --all -- --check`, and `cargo check --workspace --all-targets
--offline`. `git diff --check` and `graphify update .` are recorded after the
documentation update. `python3 tools/check_layout.py` remains subject to the
pre-existing unmapped root `.serena/` directory, which was left untouched. No
physical Mac mini check was performed or required for this integer helper.
S2-29 and the S2-28 umbrella remain open under ISSUE-019.

## S2-29h — exact unsigned magnitude normalization (partial)

Added `normalize_exact_magnitude(sign, exponent, magnitude)` for exact values
of `(-1)^sign * magnitude * 2^(exponent - 127)`. Nonzero magnitudes are shifted
left only until the leading one reaches bit 127; the same shift is subtracted
from the exponent with checked `i32` arithmetic. Zero returns `(false, 0, 0)`
for any sign and exponent. The helper consumes the unsigned `u128` directly,
preserves all bits, and does not use the signed coefficient normalization
path.

Literal vectors cover magnitude 1, `2^126`, `2^127`, `u128::MAX`, a preserved
low bit in `(2^64 + 1) << 63`, both signs, idempotence, signed-zero
canonicalization at both exponent limits, and success/failure at the `i32`
minimum for magnitudes 1 and `2^126`. Composition with S2-29g checks
minimum-times-minimum (one-bit left normalization), maximum-times-maximum
(already normalized), and minimum-times-maximum, including sign, exponent,
and unchanged source-class provenance. Division, rounding, GRS, binary80
packing, architectural flags/exceptions, instructions, and native execution
remain outside this partial slice.

`cargo test -p od-isa x87 --offline` passed (24 tests); `cargo fmt --all
-- --check` and `cargo check --workspace --all-targets --offline` passed.
`python3 tools/check_layout.py` stopped on the pre-existing unmapped root
directory `.serena/`, which was left untouched. No physical Mac mini check was
performed or required for this integer helper. S2-29 and the S2-28 umbrella
remain open under ISSUE-019.

## S2-29i — exact normalized finite quotient and remainder (partial)

Added `X87FiniteQuotient` and `divide_finite_significands(lhs, rhs)`. For
nonzero normalized finite significands, it scales the numerator by 63 or 64
bits to produce a quotient in `[2^63, 2^64)`, then retains the exact unsigned
remainder and original denominator. The returned fields represent
`(-1)^sign * (quotient + remainder / denominator) * 2^(exponent - 63)`; the
exponent uses checked final `i32` conversion and the sign is XOR. A zero
denominator, including 0/0, returns `None`; zero numerator returns unsigned
zero fields and keeps the denominator. This does not choose architectural
signed zero or define guest #Z behavior.

Literal vectors cover the eight normalized quotient/remainder cases, sign
combinations, denominator retention, zero precedence, raw subnormal and
pseudo-denormal composition, source/input preservation, and synthetic
exponent boundaries. The helper consumes no jammed alignment or GRS data and
does not round, pack, set flags/exceptions, execute guest instructions, or
claim native behavior.

`cargo test -p od-isa x87 --offline` passed (27 tests), as did
`cargo check --workspace --all-targets --offline`, `cargo fmt --all -- --check`,
`git diff --check`, and `graphify update .`. `python3 tools/check_layout.py`
stopped on the pre-existing unmapped root `.serena/` directory, which was left
untouched. Graphify reported its existing `include/open_desktop/host_abi.h`
parse warning (tracked by ISSUE-010). No physical Mac mini check was performed
or required for this integer helper. S2-29 and the S2-28 umbrella remain open
under ISSUE-019.


<a id="s2-31bx--round-binary80-fractions-through-m16-fistfstp"></a>

## S2-31bx — round binary80 fractions through m16 FIST/FISTP

Only bare `DF 10` and `DF 18` now admit Valid-tagged canonical binary80 Normal
sources with unbiased exponent -16382 through -1. The comparison-only rounding
path matches the bounded m32 behavior: nearest-ties-even stores zero through
exact 0.5 and signed one above; toward-zero stores zero; directed rounding
stores signed one only when rounding away from zero. Every admitted nonzero
fraction adds sticky PE after the atomic Word write, C1 is set iff magnitude
one is selected, and no IE/DE/OE/UE is added. Existing m32 behavior and m64
fraction rejection are unchanged.

The independent ISA matrix checks literal Word results for both signs and
every RC mode, both opcodes, all TOP values, incoming PE/C1, exact half and
neighbors, largest fraction below one, minimum Normal, and exponents
-2/-63/-64/-65. Rejection tests cover invalid tag/payload/class and forbidden
environment controls; fault tests cover denied writes and canonical48 range
crossing without CPU or memory commit. The negative `-0.25` FISTP CLI fixture
composes with FNSTSW to observe `0xffff`, PE+C1, and the popped TOP/tag. JIT and
native admission remain denied.

Focused `od-isa` fraction, source/environment, and fault/range tests passed
1/1 each; the CLI/FNSTSW fixture and JIT-denial tests passed 1/1 each. The full
`cargo test --workspace --offline` suite passed 871 tests: 506 `od-isa`, 4
`od-jit`, 6 host, 60 runner CLI, 120 fixture/JIT, 166 testkit, 8 x86-runner
library, and 1 x86-runner main test; all doc-tests passed. Host and Linux
x86_64 `cargo check --workspace --all-targets --offline` passed; the Linux
target compiles native-admission denial coverage but does not execute it. The
Linux-target build emitted existing dead-code warnings in `od-host` and
`od-runner`. `cargo fmt --all -- --check` and `git diff --check` passed.
`python3 tools/check_layout.py` reports the preserved root `.serena/` directory
as unmapped; it is excluded from the change. `graphify update .` rebuilt the
graph to 4,369 nodes, 8,901 edges, and 178 communities. Native runtime and
physical x86 acceptance remain deferred to the combined final substage under
ISSUE-019.




<a id="s2-31by--round-binary80-fractions-through-m64-fistp"></a>

## S2-31by — round binary80 fractions through m64 FISTP

Only the existing bare `DF 38` (`FISTP m64int [RAX]`) path now admits
Valid-tagged canonical binary80 Normal values with unbiased exponent -16382
through -1. It reuses the comparison-only rule already used for m16/m32:
nearest-ties-even stores zero through exact 0.5 and signed one above; toward-
zero stores zero; directed rounding stores signed one only when rounding away
from zero. Every admitted nonzero fraction adds sticky PE after the atomic
Qword write, C1 reflects whether magnitude one was selected, and no IE/DE/OE/UE
is added. Source bytes and incoming PE/C0/C2/C3 remain preserved; FISTP tag/TOP
and pointer state commit only after a successful write. No FIST m64 form was
added, and m16/m32 behavior is unchanged.

The independent ISA matrix checks literal Qword values for both signs, all
four RCs, all TOP values, incoming PE/C1, exact half and adjacent values,
largest fraction below one, minimum Normal, and exponents -2/-63/-64/-65.
Rejections cover tag/raw mismatches, subnormal, pseudo-denormal, pseudo-normal,
malformed special, unsupported FCW/FSW, prefix, and the nonexistent DF30 FIST
form. Qword canaries and denied-write/backend/range-fault atomicity are checked.
A negative `-0.25` FISTP CLI fixture composes with FNSTSW and observes Qword -1,
PE+C1, and the popped TOP/tag. JIT denial is asserted; native admission denial
is retained in the Linux-only test list.

Focused `cargo test -p od-isa fistp_m64int --offline` passed 9/9. The CLI and
JIT-denial tests each passed 1/1. The full `cargo test --workspace --offline`
suite passed 876 tests: 509 `od-isa`, 4 `od-jit`, 6 host, 61 runner CLI, 121
fixture/JIT, 166 testkit, 8 x86-runner library, and 1 x86-runner main; all
doc-tests passed. Host and Linux x86_64 `cargo check --workspace --all-targets
--offline` passed; Linux compiled native-admission test code but did not run it.
The Linux cross-check emitted existing dead-code warnings in `od-host` and
non-x86 `od-runner` benchmark helpers. Formatting and `git diff --check` passed.
`python3 tools/check_layout.py` reports the preserved root `.serena/` directory
as unmapped; it remains excluded from this change. `graphify update .` rebuilt
the graph to 4,378 nodes, 8,918 edges, and 181 communities. Native runtime and physical x86
acceptance remain deferred to the combined final substage under ISSUE-019.


<a id="s2-31bz--masked-binary80-infinity-through-m32-fistfstp"></a>

## S2-31bz — masked binary80 infinity through m32 FIST/FISTP

Only bare `DB 10` and `DB 18` now admit Special-tagged canonical binary80
positive or negative infinity (significand `0x8000000000000000`,
sign/exponent `0x7fff` or `0xffff`). Under the existing masked FCW whitelist,
all signs and RC modes store integer indefinite `0x80000000`. After successful
atomic Dword write, IE is sticky, C1 clears, incoming PE and C0/C2/C3 remain
preserved, and no PE/DE/OE/UE/SF or ES/B is added. Source bytes, addressing,
FSW gate, and post-write FIST/FISTP commit/pop behavior remain bounded; no other
width, NaN, pseudo-infinity, unmasked delivery, or finite overflow admission
was added.

The ISA matrix crosses sign, all four admitted FCWs, both opcodes, all TOP
values, and incoming PE/C1. Rejection tests cover tag/class/malformed special,
positive finite +2^31, unsupported FCW/FSW and prefixes, plus the DF38 m64 form.
Write/range fault tests assert complete CPU/memory atomicity. Existing finite
negative -2^31 remains accepted without IE. The literal negative-infinity
FISTP/FNSTSW fixture observes IE and the popped TOP/tag; a direct FISTP,
FNSTSW, FNCLEX sequence observes IE (`0x4901`) and then clears it (`0x4900`).
JIT and native admission denial remain explicit.

Focused `fist_m32int_` tests passed 10/10; `fistp_m32int_` tests passed 7/7.
The CLI fixture and JIT-denial tests each passed 1/1. The full
`cargo test --workspace --offline` suite passed 882 tests: 513 `od-isa`, 4
`od-jit`, 6 host, 62 runner CLI, 122 fixture/JIT, 166 testkit, 8 x86-runner
library, and 1 x86-runner main; all doc-tests passed. Host and Linux x86_64
`cargo check --workspace --all-targets --offline` passed; Linux cross-check
compiled native-denial coverage but did not execute it. Existing dead-code
warnings remain in `od-host` and non-x86 runner benchmark helpers.
`cargo fmt --all -- --check` and `git diff --check` passed. The layout checker
reports only the preserved root `.serena/` directory as unmapped; it is
excluded from the change. `graphify update .` rebuilt the graph to 4,388 nodes,
8,938 edges, and 212 communities. Native runtime and physical x86 acceptance
remain deferred under ISSUE-019.


## S2-29j — pure unsigned rational nearest-integer ties-to-even (partial)

Added `round_quotient_ties_even(q, r, d)`, a pure unsigned helper for rounding
`q + r / d` to nearest with ties to even. It rejects zero denominators and
remainders at or above the denominator, widens before the half comparison, and
returns `u128` so a carry beyond `u64::MAX` is preserved. Literal tests cover
below, exact, and above half; both quotient parities at a tie; exact integers;
invalid inputs; a large remainder; and the widened carry. Composition vectors
feed S2-29i division results through the helper. Exact ties and carry use
synthetic generic rational values because the normalized divider cannot
produce them.

The helper does not implement x87 RC/PC, binary80 formatting or packing, guest
FDIV, signs, exponent adjustment, architectural flags/exceptions, or signed
zero. It does not execute guest instructions or claim architectural rounding
behavior.

`cargo test -p od-isa x87 --offline` passed (29 tests), as did
`cargo fmt --all -- --check`, `cargo check --workspace --all-targets --offline`,
`git diff --check`, and `graphify update .`. `python3 tools/check_layout.py`
stopped on the pre-existing unmapped root `.serena/` directory, which was left
untouched. No physical Mac mini check was performed or required for this pure
integer helper. S2-29 and the S2-28 umbrella remain open under ISSUE-019.

## S2-29k — pure mathematical rational rounding modes (partial)

Added `RationalRoundingMode` and `round_quotient_magnitude(sign, q, r, d,
mode)`, which rounds the exact rational coefficient
`(-1)^sign * (q + r / d)` to the integer lattice and returns its unsigned
magnitude. Nearest mode uses ties-to-even; directed modes increment magnitude
only when their direction and the coefficient sign require it. The helper
returns `None` only for `d == 0` or `r >= d`, and preserves `q` when `r == 0`.
The existing `round_quotient_ties_even` remains a positive nearest-even wrapper.

Literal tests cover each mode across positive and negative quarter/three-quarter
fractions, even and odd ties, small signed fractions, exact integers, invalid
inputs in all modes, `u64::MAX` tie carry, and a widened half comparison.
Composition vectors use S2-29i quotient/remainder/denominator/sign getters for
positive and negative divisions. Tie and carry are synthetic generic rational
inputs; the normalized S2-29i divider cannot produce them.

The modes are mathematical only. They define no control-word values,
precision parameter, exponent adjustment, signed zero, x87 RC/PC, binary80
packing, status effects, or guest instruction behavior. Verification for this
change: `cargo test -p od-isa x87 --offline` passed (32 tests),
`cargo fmt --all -- --check`, `cargo check --workspace --all-targets --offline`,
and `git diff --check` passed. `graphify update .` completed; it reported the
existing parser warning for `include/open_desktop/host_abi.h`. The layout check
stopped on the pre-existing unmapped `.serena/` directory, which was left
untouched. No physical Mac mini check is required for this pure helper. S2-29
and S2-28 remain open under ISSUE-019.

## S2-29l — direct exact quotient rounding to significant precision (partial)

Added `round_quotient_to_precision(sign, q, r, d, precision, mode)` for exact
normalized quotient decompositions. It accepts 1–64 significant bits, rejects
invalid denominator/remainder/precision inputs and nonzero unnormalized
quotients, and returns zero for the exact zero pair. The implementation keeps
`t = q >> (64 - precision)` and the discarded `lo` bits, then compares those
bits and the exact rational remainder together. It uses no multiplication,
intermediate rounding, host floating point, or exponent mutation. A carry to
`2^precision` remains in the returned `u128`. With divider exponent `e`, the
rounded result represents `(-1)^sign * magnitude * 2^(e + 1 - precision)`;
the caller owns this scale.

Literal tests cover all four mathematical modes and signs, reduced precision
ties and directed tails, p=1 boundaries, exact zero, invalid input metadata,
the maximum-denominator half boundary, and a counterexample to double
rounding. Synthetic p=64 ties and carry are explicitly labeled unreachable
from normalized divider output; reduced-precision ties remain valid. The exact
S2-29i 1/3 quotient composes at p=24 (`0xaaaaab`), p=53
(`0x15555555555555`), and p=64 (`0xaaaaaaaaaaaaaaab`), with negative directed
modes checked at p=24.

This remains a mathematical helper. x87 PC/RC/control-word behavior, exponent
adjustment/normalization, subnormal/range handling, binary80 packing,
architectural flags/exceptions, signed-zero choice, and guest FDIV remain
outside this slice. Verification: `cargo test -p od-isa x87 --offline` passed
(36 tests); `cargo fmt --all -- --check`, `cargo check --workspace
--all-targets --offline`, and `git diff --check` passed. `python3
tools/check_layout.py` stopped on the pre-existing unmapped root `.serena/`
directory, which was left untouched. `graphify update .` completed with the
existing parser warning for `include/open_desktop/host_abi.h`. No physical Mac
mini check was performed or required for this pure integer helper. S2-29 and
S2-28 remain open under ISSUE-019.

## S2-29m — bounded normal finite binary80 packing (partial)

Added `pack_normal_rounded_significand(sign, exponent, magnitude, precision)`
to encode an already-rounded nonzero magnitude as a normal finite binary80
value. It validates precision 1–64 and the normalized magnitude interval
`[2^(precision-1), 2^precision]`, shifts the significand exactly into the
explicit integer bit, and handles the upper endpoint by setting the significand
to `0x8000000000000000` and incrementing the exponent once. It accepts final
exponents only in `[-16382, 16383]` and writes the eight significand bytes and
two sign/exponent bytes in memory order. It performs no rounding.

Literal raw-byte arrays cover positive one, negative 1.5 (the expected encoding
is `[00,00,00,00,00,00,00,c0,ff,bf]`), a p=1 carry, p=24/53/63/64,
negative p=64 carry, minimum normal, carry from `e=-16383` into minimum normal,
and maximum finite. Composition vectors round exact 1/3 S2-29i results and pack
them at p=24/53/64; negative p=24 nearest and directed modes are also passed
through packing. Invalid precision, zero/out-of-range magnitudes, exponent
range endpoints, carry overflow, and `i32` extremes return `None`.

Verification: `cargo test -p od-isa x87 --offline` passed (39 tests),
`cargo fmt --all -- --check`, `cargo check --workspace --all-targets --offline`,
and `git diff --check` passed. `python3 tools/check_layout.py` stopped on the
pre-existing unmapped `.serena/` directory; it was left untouched. `graphify
update .` completed. No physical Mac mini check was required for this integer
helper. Subnormals, NaNs/infinities, architectural overflow/underflow,
architectural rounding/flags/exceptions, signed-zero selection, and guest x87
instructions remain open; S2-29 and S2-28 remain open under ISSUE-019.

## S2-29n — direct exact quotient-to-subnormal binary80 rounding/packing (partial)

Added `round_pack_subnormal_quotient(sign, e, q, r, d, mode)` for exact
normalized quotient inputs. It preserves the original quotient and rational
remainder through one precision-rounding call for `k=1..63`, uses an exact
half-minimum-subnormal comparison at `k=64`, and handles `k>=65` as strictly
below half a minimum-subnormal unit. The exponent distance is computed in
`i64`, so `i32::MIN` remains safe. The final significand is encoded with
exponent field zero below `2^63`, with `2^63` encoded as minimum normal; a
rounded zero retains the exact input sign.

Literal vectors cover both signs and all four mathematical rounding modes,
exact subnormals, half minimum subnormal, `k=1/63/64/65`, minimum and maximum
subnormal, rounding to minimum normal, `i32::MIN`, invalid domains, and the
anti-double-rounding input `e=-16383, q=2^63+1, r=1, d=3`. Composition uses
exact S2-29i quotient getters for minimum normal divided by two and by three.
The helper provides numerical finite-value rounding and encoding only; it does
not implement architectural underflow/precision effects, x87 RC/PC, special
values, operation signed-zero rules, flags/exceptions, or guest FDIV.

Verification: `cargo test -p od-isa x87 --offline` passed (42 tests), as did
`cargo fmt --all -- --check`, `cargo check --workspace --all-targets --offline`,
and `git diff --check`. `python3 tools/check_layout.py` stopped on the existing
unmapped root `.serena/` directory, which was left untouched. `graphify update .`
was run after the implementation. No physical Mac mini check was performed or
required for this integer helper. S2-29 and S2-28 remain open under ISSUE-019.

## S2-30b — bare FNSTSW AX (bounded local slice)

The local decoder and interpreter accept only exact unprefixed `DF E0` with an
existing `Some(X87State)`. Execution reads the original raw 16-bit FSW, writes
it to AX through the existing word-sized register path, advances RIP by two,
and retires once. RAX[63:16], other GPRs, and RFLAGS are preserved. The local
snapshot remains unchanged by deterministic implementation policy; this is not
an architectural preservation claim for C0/C1/C2/C3, which Intel defines as
undefined after FNSTSW. AX contains the raw pre-operation word, including the
local concrete values of those four fields.

The existing local x87 fixture profile admits one NOP, FNINIT, or FNSTSW AX
instruction, with budget 1 and stop RIP immediately afterward. The interpreter
does not synthesize x87 state or inspect CR0.EM/TS, deliver #NM, or insert WAIT.
Every prefix, all other `DF` second bytes and memory forms, and `9B DF E0` stay
Unsupported. Missing state, fetch fault, STOP, and budget exhaustion do not
commit. Native execution still rejects the x87 profile and direct FNSTSW
admission. No FPU feature is advertised.

Local tests cover all 256 `DF` second bytes, prefix classes, truncated fetch,
multiple raw FSW values including different TOP and exception/status bits,
AX partial write, unchanged remaining GPR/RFLAGS and the deterministic local
x87 snapshot, missing state, fetch fault, STOP, budget exhaustion, fixture
schema admission, runner observation, and native rejection. The x87 snapshot
check documents only the current local implementation policy for undefined
condition codes.

Normative source: Intel SDM revision 089 Vol.2A, “FNSTSW—Store x87 FPU Status
Word,” p.3-385, [official Vol.2A PDF](https://cdrdv2-public.intel.com/868140/253666-089-sdm-vol-2a.pdf).
The reference specifies `DF E0` writing the status word to AX, C0/C1/C2/C3 as
undefined, and no x87 floating-point exceptions.

Validation passed: `cargo test -p od-isa fnstsw --offline`,
`cargo test -p od-testkit x87_state_profile --offline`, the focused runner
fixture test, `cargo test --workspace --offline`, `cargo fmt --all -- --check`,
`cargo check --workspace --all-targets --offline`, and
`cargo check -p od-x86-runner --all-targets --target
x86_64-unknown-linux-gnu --offline`. `python3 tools/check_layout.py` stopped
on the pre-existing untracked `.serena/` root directory, which was left
untouched. An attempted Linux-target `cargo test` could not link because this
macOS host's Darwin linker does not accept the Linux GNU linker flags; native
rejection tests therefore have compile-only Linux-target coverage here.
`graphify update .` completed and reported its existing partial extraction
warning for `include/open_desktop/host_abi.h` (ISSUE-010). No physical CPU or
Mac mini acceptance was run; those checks remain deferred to the shared final
acceptance substage.

## S2-29o — compose finite division with single-round binary80 packing (partial)

Added `divide_finite_to_binary80(lhs, rhs, mode)`. It rejects either zero input
before division, computes the exact significand quotient/remainder for
nonzero inputs, applies sign XOR, and branches on the exact unrounded quotient
exponent. Normal-range results use one p=64 rational round and normal packing;
smaller results go directly to subnormal packing with the original exact
remainder. Tiny nonzero values retain their result sign if they round to zero.
`None` denotes bounded-helper rejection, not a guest #Z or overflow result.

Literal tests include exact normal/subnormal values, signed directed 1/3,
minimum normal, largest subnormal and the reachable carry to minimum normal,
minimum subnormal, half/below/above-half minimum subnormal, signed zero,
maximum finite, unsupported upper range, every sign-XOR combination, zero
operand rejection, immutability/provenance, and the reachable anti-double-round
vector (`e=-16383, q=0xaaaaaaaaaaaaaaad, r=0x4000000000000000,
d=0xc000000000000000` → `57 55 55 55 55 55 55 55 00 00`). Normal p=64
synthetic ties/carry remain unreachable from normalized divider outputs;
subnormal carry is reachable.

Verification: `cargo test -p od-isa x87 --offline` passed (45 tests), as did
`cargo fmt --all -- --check`, `cargo check --workspace --all-targets --offline`,
and `git diff --check`. `python3 tools/check_layout.py` stopped on the existing
unmapped root `.serena/` directory, which was left untouched. `graphify update .`
was run after the implementation. No physical Mac mini check was required for
this numeric helper. Architectural x87 behavior remains open under ISSUE-019.

## S2-29p — round exact coefficient to one binary80 result (partial)

Added `round_pack_exact_coefficient(exponent, coefficient, mode)`. It rejects
zero, losslessly normalizes the signed `i128` coefficient, and decomposes the
even `u128` magnitude into the exact quotient `q + r/2^63`. Normal results use
one p=64 rounding call followed by normal packing. Subnormal results pass the
original quotient and remainder directly to the subnormal packer, with no
intermediate rounding. `None` means bounded normalization or packer rejection.

Literal vectors cover both signs of `2^63`, normal ties with even and odd
retained significands, synthetic carry to 4, minsub and half-minsub signed
results under all mathematical modes, the anti-double-round target
`(e=-16385, coefficient=2^64+11) -> significand 0x2000000000000001`, largest
subnormal, tie into minimum normal, max finite, upper-range rejection,
`i128::MIN/MAX`, exponent boundaries, and zero. S2-29e composition covers
positive/negative cancellation to ±1, carry, a zero operand, unequal exponent
rejection, and unchanged inputs. Inexact synthetic subnormal coefficient
vectors are explicitly unreachable from exact binary80 same-exponent sums.
This is numerical encoding only; it does not add guest operations or
architectural x87 state effects.

Verification: `cargo test -p od-isa x87 --offline` passed (48 tests), as did
`cargo fmt --all -- --check`, `cargo check --workspace --all-targets --offline`,
and `git diff --check`. `python3 tools/check_layout.py` stopped on the existing
unmapped root `.serena/` directory, which was left untouched. `graphify update .`
was run after the implementation. No physical Mac mini check was required for
this integer helper. S2-29, S2-28, and architectural x87 behavior remain open
under ISSUE-019.

## S2-30c — bare FNCLEX (bounded local slice)

The decoder and local interpreter accept only exact unprefixed `DB E2` with
an existing `Some(X87State)`. Execution clears status bits 0–7 and 15 using
the `0x80ff` mask, advances RIP by two, and retires once. TOP and the current
concrete C0/C1/C2/C3 values remain as a deterministic local policy; Intel
defines those condition bits as undefined after FNCLEX. FCW, FTW, all raw
register payloads, instruction/data pointers, last opcode, GPRs, RFLAGS and
MXCSR are unchanged. The profile assumes x87 availability and performs no
CR0.EM/TS check, #NM delivery, WAIT, pending-exception check, or floating-point
exception delivery. Missing state and every other/prefixed form are
Unsupported; fetch fault, STOP and budget exhaustion commit no state. The
testkit fixture profile accepts one FNCLEX instruction. Native profile and
instruction admission remain closed and no FPU feature is advertised.

Normative source: Intel SDM revision 093 Vol.2A, “FCLEX/FNCLEX—Clear
Exceptions,” pp.3-320–3-321, [official Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf).
The reference specifies the no-wait `DB E2` form, clears PE/UE/OE/ZE/DE/IE,
ES, SF and B, marks C0–C3 undefined, and has no floating-point exception or
MXCSR effect.

Validation passed: the focused FNCLEX interpreter tests, exhaustive DB-second-
byte decoder test, testkit x87-profile tests, and local fixture-runner FNCLEX
case passed. `cargo test --workspace --offline`, `cargo fmt --all -- --check`,
`cargo check --workspace --all-targets --offline`,
`cargo check -p od-x86-runner --all-targets --target
x86_64-unknown-linux-gnu --offline`, and `git diff --check` passed. The
Linux-only native rejection test was not executed on this macOS host; its
target was included in the Linux-target all-target compile check. Graphify
update completed. `python3 tools/check_layout.py` reported the pre-existing
unmapped `.serena/` directory, which remains untouched. Physical acceptance
remains deferred to the shared final acceptance substage. S2-29, S2-28, and
architectural x87 behavior remain open under ISSUE-019.

## S2-30d — exact-only FADD ST(0), ST(1) (bounded local slice)

The decoder and interpreter accept only bare `D8 C1` with an existing
`Some(X87State)`. Execution requires FCW `0x037f`, clear FSW exception/ES/B
bits (`0x80ff`), Valid tags at physical TOP and TOP+1, two canonical Normal
binary80 payloads and equal normalized exponents. It obtains the exact signed
coefficient through `add_finite_same_exponent`, rejects coefficients wider
than 64 bits unless their 65-bit magnitude is even, then calls
`round_pack_exact_coefficient` once and requires a canonical Normal output.
This proves exact representability before using the numeric helper. Inexact
odd 65-bit results, zero/cancellation, unequal exponents, invalid encodings or
tags, unsupported environment, overflow and zero/subnormal results return
Unsupported without state commit.

Success updates only the physical TOP payload, clears FSW.C1, sets FIP to the
instruction RIP and advances RIP by two. GPRs, RFLAGS, FCW, TOP, tags, other
payloads and all other FSW bits remain as before. C0/C2/C3 and FDP are retained
as local policy because they are undefined after this register-only FADD form;
these fields carry no native equality claim. FOP is unchanged on the exact
no-exception path. The profile assumes x87 availability and does not model
CR0.EM/TS, #NM, FPU capability, general special values, or exception behavior.
Native x87 profile and instruction admission remain closed.

Independent literal tests cover exact 1+1, a 65-bit even carry, TOP wrap
(TOP=7), FIP update, C1 clearing, undefined-field retention policy, and
no-commit rejection for inexact sums, zero, cancellation, unequal exponents,
special/pseudo/denormal/unnormal input, invalid tags, unsupported FCW/FSW,
subnormal output and overflow. Decoder tests cover all D8 ModR/M neighbors,
prefixes, `WAIT`, and truncation. Fixture schema, runner and native rejection
checks cover the exact form. Normative sources are Intel SDM revision 093
Vol.2A, “FADD/FADDP/FIADD—Add,” pp.3-311–3-313, and Vol.1
§§8.1.5.2–8.1.9.1, pp.8-7–8-11 ([Vol.2A](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf),
[Vol.1](https://cdrdv2-public.intel.com/929351/253665-093-sdm-vol-1.pdf)).

Validation passed: focused `od-isa` FADD tests, testkit exact-form profile
tests, the fixture-runner success/rejection case, and
`cargo test --workspace --offline` passed. `cargo fmt --all -- --check`,
`cargo check --workspace --all-targets --offline`,
`cargo check -p od-x86-runner --all-targets --target
x86_64-unknown-linux-gnu --offline`, and `git diff --check` passed. The
Linux-specific native rejection test is compiled for that target but could not
be executed on this macOS host; no physical hardware check was run. Graphify
update completed. `python3 tools/check_layout.py` reported the pre-existing
unmapped root `.serena/` directory, which remains untouched. Physical
acceptance remains deferred to the shared final acceptance substage. S2-29,
S2-28 and general x87 semantics remain open under ISSUE-019.

## S2-30e — exact-only FADD ST(0), ST(i) register sources (bounded local slice)

Extended the exact-only FADD decoder from bare `D8 C1` to bare `D8 C0`–`D8
C7`. The decoded operation retains `i`; execution uses physical TOP as the
destination and `(TOP+i)&7` as the source. The S2-30d arithmetic domain and
environment checks remain unchanged. Both operands are loaded before the
pre-commit result validation. For `i=0`, TOP is used as both operands and the
result is committed exactly once. The tag check covers only the operands used,
so the alias case accepts empty tags in unrelated physical registers. At `i=7, TOP=7`, the source wraps to physical register 6; this is not an alias.

Literal expected-byte success vectors cover all eight indices and varied TOP
values, including `i=0` with empty unrelated tags and wrap cases `i=1, TOP=7` and `i=7, TOP=7`. Reject vectors
compare the full state after bad-tag rejection for each `i`, and retain the
existing matrix for unsupported FCW/FSW, zero/cancellation, unequal exponents,
special encodings, inexact coefficients, overflow and non-Normal results.
Decoder checks enumerate all ModR/M bytes, every prefix for each accepted
encoding, and truncation. Testkit schema accepts precisely the eight encodings;
the native runner test checks all eight stay fail-closed. Native admission is
unchanged.

Success preserves the prior update contract: write only physical TOP, clear
FSW.C1, set FIP to the instruction RIP, advance RIP by two, and preserve TOP,
tags, GPRs, RFLAGS, FCW, remaining FSW fields, other payloads, FDP and FOP. The
retention of undefined C0/C2/C3 and FDP remains local deterministic policy.
Normative sources remain Intel SDM revision 093, Vol.2A pp.3-311–3-313 and
Vol.1 §§8.1.5.2–8.1.9.1 ([Vol.2A](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf),
[Vol.1](https://cdrdv2-public.intel.com/929351/253665-093-sdm-vol-1.pdf)).

Validation passed: focused `od-isa` decoder/executor FADD tests, the testkit
x87 schema test, and the fixture-runner test for all eight source indices plus
bad-tag no-commit. `cargo test --workspace --offline`, `cargo fmt --all --
--check`, `cargo check --workspace --all-targets --offline`, the Linux-target
`cargo check -p od-x86-runner --all-targets --target x86_64-unknown-linux-gnu
--offline`, and `git diff --check` passed. Linux-only native rejection tests
were compiled for their target but not executed on this macOS host. Graphify
update completed with the existing `include/open_desktop/host_abi.h` parser
warning (tracked by ISSUE-010). `python3 tools/check_layout.py` reports the
pre-existing unmapped root `.serena/` directory, left untouched. Physical
acceptance remains deferred to the shared final acceptance substage; no Mac
mini or SSH check was run.

## S2-30f — exact-only FADD ST(i), ST(0) destination forms (bounded local slice)

Added a separate decoded operation for bare `DC C0+i`, `i=0..7`, implementing
`FADD ST(i), ST(0)`. Execution retains the S2-30d arithmetic and environment
gates. Source is physical TOP and destination is `(TOP+i)&7`; `i=0` reads the
shared physical register once and checks only that tag. Both used payloads are
captured and the result is validated before commit. Success writes only the
destination payload, clears C1, records FIP, advances RIP by two, and preserves
TOP, tags, other payloads, environment fields and integer state. There is no
pop. Unsupported cases return before state mutation, and native admission
remains closed.

Literal expected-byte vectors exercise all destination indices, varied TOP,
wrap, the alias case with unrelated Empty tags and source preservation for
`i!=0`. Full-state reject vectors include bad used tags, special operands,
unequal exponents and an inexact coefficient. The alias case cannot produce an
inexact sum because a value added to itself has an even coefficient. Decoder
tests enumerate every DC ModR/M value, all legacy/REX prefixes and truncation;
testkit schema accepts all eight bare forms, and native profile/admission tests
reject them. Existing D8 decoding and exact-only arithmetic remain unchanged.

Normative basis: Intel SDM revision 093, Vol.2A pp.3-311–3-313 and Vol.1
§§8.1.8–8.1.9.1 ([Vol.2A](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf),
[Vol.1](https://cdrdv2-public.intel.com/929351/253665-093-sdm-vol-1.pdf)).
Focused `cargo test -p od-isa fadd_sti_st0 --offline` (3 tests), focused
testkit x87 schema coverage, and the full `cargo test --workspace --offline`
passed. `cargo fmt --all -- --check`,
`cargo check --workspace --all-targets --offline`, the Linux-target
`cargo check -p od-x86-runner --all-targets --target x86_64-unknown-linux-gnu
--offline`, and `git diff --check` passed. The native rejection test is
platform-gated and was not executed on this macOS host. The Linux target
`cargo check` passed; an additional `cargo test --no-run` cross-link attempt
could not use the host macOS linker, which rejected Linux linker flags.
`python3 tools/check_layout.py` reports the existing
unmapped root `.serena/` directory, left untouched. `graphify update .`
completed after source and documentation updates. No Mac mini or SSH
acceptance was run; the consolidated acceptance substage remains responsible
for physical checks. Independent `gpt-6.1-sol` medium review found no code
defects; it identified one contradictory phrase in the new contract, which was
corrected to “other D8 forms.”

## S2-30g — exact-only FADDP ST(i), ST(0) (bounded local slice)

Added bare `DE C0+i`, `i=0..7`, as exact-only `FADDP ST(i), ST(0)`. It reuses
the S2-30d exact result helper with destination `i` and source zero, so all
arithmetic and environment preconditions are checked before the first mutation.
Success writes to old physical `(TOP+i)&7`, empties old physical TOP, increments
TOP modulo eight, clears C1, records starting RIP in FIP, advances RIP by two,
and retires once. The physical register array and tag word are not shifted. At
`i=0`, the just-written bytes remain under Empty; at `i=1`, the result is new
ST(0). Other payloads, tags, FCW, remaining FSW fields, FDP/FOP and integer
state follow the specified preservation policy. Native admission stays closed.

Independent byte/state vectors cover all 64 `(TOP,i)` pairs with unique physical
payload sentinels, alias/pop behavior at `i=0`, new ST(0) at `i=1`, TOP wrap and
the exact 1+1 carry result. Reject vectors compare full state for used bad tags,
specials, unequal exponents, inexact coefficients and i=0 overflow. Additional
checks cover missing x87 state, fetch fault, stop-before-execution and zero
budget. Decoder exhaustively checks DE ModR/M, prefixes and truncation; the
testkit schema accepts all eight bare encodings, and the native runner rejects
all eight forms.

Normative basis: Intel SDM revision 093, Vol.2A pp.3-311–3-313 and Vol.1
§§8.1.7–8.1.9 ([Vol.2A](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf),
[Vol.1](https://cdrdv2-public.intel.com/929351/253665-093-sdm-vol-1.pdf)).
This does not claim general FADDP, special values, stack faults, exception
delivery, CR0 behavior or native x87 execution.

Focused `cargo test -p od-isa faddp_sti_st0 --offline` (4 tests), the testkit
x87 schema test, and the fixture-runner matrix covering all 64 destination/TOP
pairs passed. `cargo test --workspace --offline`, `cargo fmt --all -- --check`,
`cargo check --workspace --all-targets --offline`, the Linux-target
`cargo check -p od-x86-runner --all-targets --target x86_64-unknown-linux-gnu
--offline`, and `git diff --check` passed. The Linux-only native rejection test
was not executed on this macOS host; physical acceptance remains in the final
acceptance substage. `python3 tools/check_layout.py` reports the existing
unmapped root `.serena/` directory, left untouched. `graphify update .`
completed and refreshed the code graph. Independent `gpt-6.1-sol` medium
review found no concrete defects; no hardware or SSH check was run.


## S2-30h — exact-only FSUB ST(0), ST(i) (bounded local slice)

Added bare `D8 E0+i`, `i=0..7`, as `FSUB ST(0), ST(i)`. Execution shares
S2-30d's exact same-exponent addition/packing path after validating both raw
operands; it toggles the sign only in a local copy of the Normal-validated
source. `i=0` always returns Unsupported because it aliases TOP and cancels.
Success updates only physical TOP payload, clears C1, records the old RIP in
FIP, advances RIP by two and retires once. Native/CPUID feature admission is
unchanged and remains closed.

Literal vectors cover all eight TOP values and i=1..7, both subtraction
orders and signs, ±2^-63, minimum Normal and maximum-coefficient carry. Full
state is compared on rejects for every i=0/TOP pair, odd 65-bit coefficient,
unequal exponent, invalid environment/tags/classes, exact subnormal and
overflow. Decoder checks all D8 ModR/M bytes, prefixes, WAIT, truncation and
reverse/memory/DC/DE neighbors. The v2 schema admits all eight encodings with
i=0 represented as Unsupported; the CLI runner confirms i=1..7 success and
full-state i=0 rejection. The native test names all eight new encodings in its
fail-closed profile/admission set.

Normative source: Intel SDM revision 093, Vol.2A, “FSUB/FSUBP/FISUB—Subtract,”
pp.3-394–3-396 ([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=512)).
It specifies `DEST − SRC`, `D8 E0+i` as FSUB ST(0), ST(i), C1 cleared when no
round-up occurred, and C0/C2/C3 undefined. Undefined condition-code and FDP
retention is explicitly local deterministic policy.

Focused `od-isa` FSUB tests (4), the testkit x87 schema test, and the runner
fixture test passed. `cargo test --workspace --offline` passed (all workspace
tests); `cargo fmt --all -- --check`, `cargo check --workspace --all-targets
--offline`, the Linux-target `cargo check -p od-x86-runner --all-targets --target
x86_64-unknown-linux-gnu --offline`, and `git diff --check` passed. The
Linux-only native rejection test is compiled by the target check but was not
executed on this macOS host. `python3 tools/check_layout.py` reports only the
pre-existing unmapped root `.serena/` directory; it was left untouched.
`graphify update .` refreshed the graph after source edits. No Mac mini/SSH
check or commit was performed; physical acceptance remains for the shared
final acceptance substage.

## S2-29q — compose same-exponent finite addition with one-round binary80 packing (partial)

Added `add_finite_same_exponent_to_binary80(lhs, rhs, mode)`. It composes the
exact signed coefficient returned by `add_finite_same_exponent` with one call
to `round_pack_exact_coefficient`, so the sum is rounded and encoded once.
Either single zero is neutral in either operand position. Two zeros and exact
cancellation return `None` because no architectural zero sign is selected;
unequal nonzero normalized exponents are rejected without alignment/jamming.
The output is canonical finite binary80. `None` means only bounded helper
rejection, including upper finite-range rejection, not a guest exception.

Raw binary80 byte vectors normalized with `normalize_finite()` cover positive
and negative `1+1`, positive/negative normal ties with even/odd retained bits
in all four mathematical modes, adjacent minimum normals subtracting to ±minsub,
maximum subnormal, two half-minimum-normal values summing to minimum normal,
minsub+minsub, zero plus normal/subnormal/pseudo-denormal in either order,
maxfinite plus zero, overflow rejection, exact cancellation, all four zero
pairs, and unequal exponents. Input encodings and source-class provenance are
preserved.

Reachability is narrower than the generic coefficient rounder: two 64-bit
significands give `abs(sum) <= 2^65-2`, so its synthetic `2^65-1` carry-to-four
case is unreachable. Binary80 finite values are minimum-subnormal multiples;
their exact sums in the subnormal range remain such multiples. Inexact
subnormal tails, half-minsub ties and underflow-to-zero therefore cannot arise
from this addition helper. No guest FADD/FSUB decode/execute, x87 stack or
control-word behavior, flags, exceptions, or architectural zero selection was
added.

Verification: `cargo test -p od-isa x87 --offline` passed (50 tests), as did
`cargo check --workspace --all-targets --offline`, `cargo fmt --all -- --check`,
`git diff --check`, and `graphify update .`. Graphify reported its existing
parser warning for `include/open_desktop/host_abi.h` (tracked by ISSUE-010).
`python3 tools/check_layout.py` stopped on the pre-existing unmapped root
`.serena/` directory, which was left untouched. No physical Mac mini check is
required for this integer helper. S2-29 and S2-28 remain open under ISSUE-019.

## S2-30i — exact-only FSUB ST(i), ST(0) (bounded local slice)

Added bare `DC E8+i`, `i=0..7`, as `FSUB ST(i), ST(0)`. The operation shares
the S2-30d exact same-exponent arithmetic path, validates both original
canonical Normal operands and their Valid tags before changing state, and
toggles the sign only in a local copy of the ST(0) source. The result replaces
only physical `(TOP+i)&7`; source ST(0), TOP, tags and other payloads are
preserved. Alias `i=0` is Unsupported atomically. Native admission remains
closed for all eight encodings.

Literal sign/order vectors cover positive and negative results including
`1.5−1`, `1−1.5`, negative permutations and `±2`. Additional literal vectors
cover `±2^-63`, maximum even 65-bit carry and minimum Normal. All TOP×i=1..7
execution pairs verify physical result placement and successful retirement;
source bytes and all unrelated state remain unchanged. Full-state atomic reject
cases cover alias, missing state, FCW/FSW, tags, unequal exponents, every raw
non-Normal class, cancellation, odd 65-bit coefficient, exact subnormal output
and overflow. Separate run checks verify fetch-fault, stop-address and zero-
budget atomicity. Decoder, fixture schema, runner matrix and native fail-closed
source tests cover the new encoding range.

`cargo test --workspace --offline`, `cargo fmt --all -- --check`,
`cargo check --workspace --all-targets --offline`,
`cargo check -p od-x86-runner --all-targets --target
x86_64-unknown-linux-gnu --offline`, `git diff --check`, and `graphify update .`
passed. `python3 tools/check_layout.py` reports the pre-existing unmapped root
`.serena/` directory, which was left untouched. The Linux-only native rejection
test was not executed on this macOS host; the Linux-target runner check compiled
that target. No hardware or SSH test was run; physical acceptance remains in
the shared final acceptance substage.

## S2-30j — exact-only FSUBP ST(i), ST(0) (bounded local slice)

Added exactly bare `DE E8+i`, `i=0..7`, as exact-only `FSUBP ST(i), ST(0)`.
Execution reuses the S2-30i `exact_fsub_result` gate for old physical
destination `(TOP+i)&7` and old ST(0). It validates before mutation, writes
the result, marks old physical TOP Empty, increments TOP modulo eight, clears
C1, records FIP, advances RIP by two and retires once. Physical payloads and
FTW are not rotated. Alias `i=0` is Unsupported atomically. Native admission
remains closed for all eight encodings.

Checks cover literal ±0.5/±2 outcomes, both `±2^-63` boundaries, maximum even
65-bit carry and minimum Normal output, every TOP×i=1..7 physical placement,
TOP wrap, tag/pop status, preserved payloads and unrelated CPU state. Alias
for every TOP and bad environment/tag/exponent/class/coefficient/subnormal/
overflow cases compare the complete state. Fetch fault, stop address, zero
budget, exhaustive DE ModR/M and prefix decoding, fixture schema and native
denial are covered. The testkit execution runner has independent literal
expected binary80 outputs for sign/order cases and successful retirement.

Normative source: Intel SDM revision 093, Vol.2A pp.3-394–3-396
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=512)); FSUBP computes `DEST − SRC` before popping old TOP.

Verification passed: focused FSUBP tests, `cargo test --workspace --offline`,
`cargo fmt --all -- --check`, `cargo check --workspace --all-targets --offline`,
and `cargo check --workspace --all-targets --target x86_64-unknown-linux-gnu
--offline`. `git diff --check` and `graphify update .` completed. The layout
check reports only the pre-existing unmapped root `.serena/` directory, which
was left untouched. Linux-only native tests were compile-checked but not
executed on macOS. No Mac mini, SSH or hardware test was run; physical
acceptance remains the final substage.

## S2-30k — exact-only FSUBR ST(0), ST(i) (bounded local slice)

Added exactly bare `D8 E8+i`, `i=0..7`, as `FSUBR ST(0), ST(i)`. Execution
reuses the exact S2-30i subtraction gate with logical helper arguments
`(i, 0)` to calculate `ST(i) − ST(0)`, then writes only the result to old
physical TOP. Alias `i=0` returns Unsupported before commit. Success clears
C1, records starting RIP in FIP, advances RIP by two and retires once; TOP,
FTW, all other x87 payloads/state, GPRs and RFLAGS are preserved. Native
profile/admission remains closed.

Checks include literal sign/order cases, `±2^-63`, maximum even 65-bit carry,
minimum Normal, every TOP×i=1..7 destination placement with source/sentinel
preservation, full-state rejects for alias and arithmetic/environment/tag/
exponent/class/coefficient/subnormal/overflow conditions, and fetch/stop/zero-
budget no-commit. Decoder coverage is exhaustive over D8 ModR/M and checks
prefixes, WAIT, truncation and neighboring D8/DC/DE forms. The schema accepts
all eight bare codes and specifically validates D8 E8 as Unsupported with no
stop RIP and budget one. Literal runner cases check successful state and the
unchanged alias snapshot. Normative source: Intel SDM revision 093, Vol.2A
pp.3-397–3-399 ([official PDF, pp.3-397–3-399](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=515));
the operation is `SRC − DEST`, C1 is clear for exact results, and C0/C2/C3 are
undefined.

`cargo test --workspace --offline`, `cargo fmt --all -- --check`,
`cargo check --workspace --all-targets --offline`,
`cargo check -p od-x86-runner --all-targets --target x86_64-unknown-linux-gnu
--offline`, and `git diff --check` passed. The Linux-target check compiled the
native runner tests but did not execute them on this macOS host.
`python3 tools/check_layout.py` reports only the pre-existing unmapped root
`.serena/` directory; it was left untouched. No Mac mini, SSH, hardware or
native execution check was run; physical acceptance remains the shared final
substage. `graphify update .` refreshed the project graph. Independent
gpt-6.1-sol medium review found no concrete defects or missing required
coverage; the review was static and made no changes.

## S2-30l — exact-only FSUBR ST(i), ST(0) (bounded local slice)

Implemented bare `DC E0+i`, `i=0..7`, as the separately decoded `FSUBR ST(i),
ST(0)` operation. It computes `ST(0)−ST(i)` through the exact subtraction gate,
commits only to old physical `(TOP+i)&7`, clears C1, records FIP, advances RIP
by two and retires once. TOP/FTW and the source payload remain unchanged; alias
`i=0` is Unsupported before commit. Native admission remains closed.

Local literals verify every TOP×i physical placement, signs and operand order,
`±2^-63`, maximum even carry, minimum Normal, alias atomicity, and fetch/stop/
zero-budget atomicity. Decoder tests cover every DC ModR/M byte and rejected
prefixes, fixture schema accepts DC E0..E7 with the alias unsupported, and
native admission denies all eight encodings. `cargo test --workspace --offline`,
`cargo fmt --all -- --check`, `cargo check --workspace --all-targets --offline`,
the Linux-target runner compile check, and `git diff --check` passed. The layout
check reports the pre-existing unmapped `.serena/` directory, which was left
untouched. Independent gpt-6.1-sol medium review found no remaining concrete
defects. This report does not claim general x87 semantics or physical oracle
acceptance; Mac mini/SSH/hardware execution remains reserved for the shared
final acceptance gate.

## S2-30m — exact-only FSUBRP ST(i), ST(0) (bounded local slice)

Added exact bare `DE E0+i`, `i=0..7`, as `FSUBRP ST(i), ST(0)`. The decoder
keeps the existing `DE C0..C7` FADDP and `DE E8..EF` FSUBP forms and now
recognizes the intervening E0..E7 range. Execution reuses
`exact_fsub_result(state, 0, i)` for old `ST(0)−ST(i)`, writes the result to
old physical `(TOP+i)&7`, then marks old physical TOP Empty and increments
TOP. `i=0` decodes as the self-alias but is Unsupported before commit because
the result is cancellation. The no-operand `DE E1` alias represents
`FSUBRP ST(1), ST(0)` (`i=1`) and uses the same supported form.

The S2-30i exact gate remains unchanged: existing x87 state, FCW `0x037f`,
`FSW & 0x80ff == 0`, Valid used tags, canonical Normal operands with equal
normalized exponents, accepted coefficient bound and canonical Normal output.
Success clears C1, records FIP, advances RIP by two and retires once. The
popped TOP payload remains present; other tags, payloads, FSW fields, FCW,
FDP/FOP, GPRs and RFLAGS are preserved. The operation adds no CR0/#NM,
exception, or native feature behavior.

Literal tests cover every TOP×i pair, physical destination and pop, positive
and negative operand orders, ±2^-63, the maximum accepted even 65-bit carry,
minimum Normal output, alias and numeric/environment/tag/domain rejection,
plus fetch/truncation/STOP/zero-budget atomicity. Testkit accepts DE E0..EF
and represents DE E0 as Unsupported with no stop address and budget one. The
fixture runner checks all 56 non-alias TOP/index pairs and all eight alias
TOPs. Exhaustive DE ModR/M tests retain FADDP and FSUBP decoding; prefixes,
REX and WAIT stay rejected. Native admission denies all eight new encodings.

Normative source: Intel SDM revision 093, Vol.2A, “FSUBR/FSUBRP/FISUBR—Reverse
Subtract,” pp.3-397–3-399 ([official PDF,
pp.3-397–3-399](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=515)).
It specifies old `ST(0)−ST(i)` before pop, C1 clear for exact results and
C0/C2/C3 undefined.

`cargo test --workspace --offline` passed before the final isolated fetch-fault
assertion was added; after that test-only edit, focused `od-isa` FSUBRP and
fixture-runner FSUBRP tests passed again. The testkit x87 schema test passed,
and native fail-closed cases compile for `x86_64-unknown-linux-gnu`; the
architecture-specific native test did not execute on this arm64 macOS host.
`cargo fmt --all -- --check`, workspace all-target check, Linux-target runner
all-target check, and `git diff --check` passed. `python3 tools/check_layout.py`
reported only the pre-existing unmapped `.serena/` directory, left untouched.
Graphify update completed. Independent gpt-6.1-sol medium review found no
remaining defects or missing requested coverage. Physical acceptance remains
reserved for the shared final acceptance substage; this report makes no
hardware x87 claim.

## S2-30n — exact-only FMUL ST(0), ST(i) (bounded local slice)

Added exact bare `D8 C8+i`, `i=0..7`, as `FMUL ST(0), ST(i)`. Execution
requires existing x87 state, FCW `0x037f`, a clean `FSW & 0x80ff` mask, Valid
tags for used physical registers, and canonical Normal raw operands. It
normalizes each operand, multiplies the full significands with integer
arithmetic, normalizes the exact 128-bit magnitude, and accepts only a zero
low 64-bit tail before packing the high 64 bits at precision 64. Unequal
exponents are supported. Inexact results, zero/subnormal/overflow outputs,
invalid input classes, bad environment/tags, and missing state reject before
commit. `i=0` is a valid square and needs only the TOP tag.

Success writes only the result into old physical TOP, clears C1, records the
starting RIP in FIP, advances RIP by two and retires once. TOP, tags, other
payloads, other x87 fields, GPRs and RFLAGS remain unchanged. C0/C2/C3 retain
their concrete values only by local policy because the SDM marks them
undefined. Native x87 profile and instruction admission stay closed.

Literal execution tests cover every TOP×i pair with physical sentinels,
including the i=0 `1.5*1.5 = 2.25` square, all four sign combinations,
different exponents, a full-width 64-bit significand multiplied by a power of
two, and full-state atomic rejection for `(1+2^-63)^2`, maximum-Normal
overflow, minimum-Normal times one-half, raw special/noncanonical encodings,
invalid tags/environment, and missing x87 state. Fetch fault, stop address,
stop request and zero budget preserve state. Decoder tests enumerate all D8
ModR/M bytes and prefixes; the fixture schema accepts all eight encodings, the
runner checks all TOP×i state results, and the native test rejects every new
encoding.

Normative source: Intel SDM revision 093, Vol.2A, “FMUL/FIMUL—Multiply,”
pp.3-360–3-362 ([official PDF, Vol.2A p.3-360](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=478)).
`D8 C8+i` computes old `ST(0) * ST(i)` without popping; exact results clear C1,
and C0/C2/C3 are undefined.

Focused `od-isa` FMUL tests, the testkit x87-profile schema test, and the
fixture-runner TOP×i matrix passed. `cargo test --workspace --offline`,
`cargo fmt --all -- --check`, `cargo check --workspace --all-targets --offline`,
the Linux-target `od-x86-runner` all-target check, and `git diff --check` also
passed. `python3 tools/check_layout.py` reports only the pre-existing unmapped
root `.serena/` directory, which was left untouched. The Linux-target check
compiled native rejection code; architecture-specific native tests did not
execute on this arm64 macOS host. `graphify update .` completed and refreshed
the code graph. No Mac mini, SSH, hardware execution or commit was performed;
physical acceptance remains reserved for the shared final acceptance stage.
Independent gpt-6.1-sol medium review found no production defect; it identified
two test-coverage gaps (the baseline tag mask and an injected fetch fault that
did not reach decode), both corrected and re-reviewed with no findings.

## S2-30o — exact-only FMUL ST(i), ST(0) (bounded local slice)

Added bare `DC C8+i`, `i=0..7`, as `FMUL ST(i), ST(0)`. The implementation
reuses the S2-30n exact-result gate, including differing exponents, and writes
only old physical `(TOP+i)&7`; it clears C1, records FIP and retires without
popping. Literal unit coverage checks every TOP×i pair, the `i=0` square,
physical payload sentinels, sign XOR, full-width-significand multiplication,
inexact and range rejection, and fetch/stop/budget atomicity. Decoder expectations preserve
the neighboring DC FADD, FSUBR and FSUB groups. Fixture schema accepts all
eight bytes and native admission rejects all eight.

The native deny list also now explicitly names `FsubrpStSt0`, correcting an
existing mismatch in the test that expects an x87-specific denial for `DE E0`
through `DE E7`. `docs/KNOWN_ISSUES.md` records the narrow resolution; broad
x87 semantics and physical x87 comparison remain open.

Validation passed: `cargo test --workspace --offline`,
`cargo fmt --all -- --check`, `cargo check --workspace --all-targets --offline`,
`cargo check -p od-x86-runner --all-targets --target x86_64-unknown-linux-gnu --offline`,
and `git diff --check`. The Linux-target check compiles native rejection tests;
they were not executed on this arm64 macOS host. `python3 tools/check_layout.py`
reports only the pre-existing unmapped root `.serena/` directory, left
untouched. `graphify update .` completed. Independent gpt-6.1-sol medium review
found no production defects or remaining coverage findings after the final
prefix, missing-state and alias-wording corrections. No Mac mini, SSH, hardware
execution or commit was performed; physical acceptance remains reserved for
the shared final acceptance stage.

## S2-30q — exact-only FDIV ST(0), ST(i) (bounded local slice)

Added bare `D8 F0+i`, `i=0..7`, as exact-only `FDIV ST(0), ST(i)`. The
pre-commit gate requires an existing x87 state, FCW `0x037f`, `FSW & 0x80ff ==
0`, Valid tags for the used physical registers, canonical Normal inputs and
zero remainder from `divide_finite_significands`. It packs the normalized
quotient at precision 64 and accepts only canonical Normal output. The gate
rejects zero operands, specials, noncanonical raw values, inexact quotients,
overflow and subnormal/zero results before changing state. It permits unequal
exponents and the quotient path where the lhs significand is smaller. `i=0`
reads a single Valid physical TOP operand and produces exact `+1`, including
for negative inputs.

Success changes only the old physical TOP payload, clears C1, records starting
RIP as FIP, advances RIP by two and retires once. TOP, tags and the rest of
machine/x87 state are preserved. Literal checks cover all 64 TOP×i placements,
`15/3 = 5`, `1/3` rejection, all signs, `lhs_significand < rhs_significand`,
full-width significand divided by one, minimum/maximum Normal self-division to
`+1`, exact overflow (`2^16383 / 0.5`), and exact subnormal (`2^-16382 / 2`).
Full-state equality is checked for every numeric/environment/tag/class reject
and for absent x87 state. Fetch fault, stop address and zero budget do not
commit. Decoder coverage enumerates every D8 ModR/M byte and prefixes, WAIT
and truncation while retaining neighbor groups. Fixture schema and runner
cover all eight encodings, the TOP×i matrix and inexact Unsupported behavior.
Native x87 admission remains closed.

Normative source: Intel SDM revision 093, Vol.2A pp.3-333–3-335,
“FDIV/FIDIV—Divide” ([official PDF, Vol.2A p.3-333](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=451)).
It specifies the old `ST(0)/ST(i)` quotient without a pop, C1 clearing for an
exact result, and undefined C0/C2/C3. General rounding, exceptions, special
values, CR0/#NM and native x87 behavior remain outside this profile.

Validation passed: `cargo test --workspace --offline`,
`cargo fmt --all -- --check`, `cargo check --workspace --all-targets --offline`,
`cargo check -p od-x86-runner --all-targets --target x86_64-unknown-linux-gnu --offline`,
and `git diff --check`. `python3 tools/check_layout.py` reports only the
pre-existing unmapped `.serena/` directory, left untouched. Linux native deny
tests were compile-only on this arm64 macOS host. `graphify update .` completed.
No physical hardware, SSH or commit was used; final acceptance remains reserved
for the shared acceptance stage.
Independent gpt-6.1-sol medium review found two test-isolation/schema-fixture
gaps; both were corrected and the affected tests reran successfully. Re-review
found no remaining findings.

## S2-30p — exact-only FMULP ST(i), ST(0) (bounded local slice)

Added bare `DE C8+i`, `i=0..7`, as exact-only `FMULP ST(i), ST(0)`. Execution
reuses the S2-30n exact multiplication gate, calculates old `ST(i)*ST(0)`,
writes old physical `(TOP+i)&7`, then marks old physical TOP Empty and
increments TOP. For `i=0`, the valid square is calculated and written before
the same old TOP tag is emptied. Exact success clears C1, records starting RIP
as FIP, advances RIP by two and retires once. Native x87 admission remains
closed. The no-operand `DE C9` encoding is the supported `i=1` form,
`FMULP ST(1), ST(0)`.

Interpreter literal vectors cover all 64 TOP×i pairs, including the i=0
`1.5*1.5 = 2.25` square, physical placement, TOP wrap, payload retention and
all unrelated state. Separate vectors cover all sign combinations, differing
exponents, and a full-width significand multiplied by a power of two. A
full-state rejection matrix covers inexact products, overflow, subnormal
results, bad FCW/FSW, invalid tags, non-Normal raw classes and missing state;
i=0 inexact and exact-overflow squares are rejected atomically. Truncation,
injected fetch fault, stop address and zero budget preserve state. Decoder
enumeration checks all DE ModR/M bytes, bare
prefix rules, WAIT and truncation while preserving DE C0..C7 FADDP, E0..E7
FSUBRP and E8..EF FSUBP. Fixture schema and runner cover all eight FMULP
encodings, every TOP×i success and an inexact `DE C8` Unsupported case with no
stop RIP and budget one. Native admission remains closed for all eight bytes.

Normative source: Intel SDM revision 093, Vol.2A, “FMUL/FIMUL—Multiply,”
pp.3-360–3-362 ([official PDF, Vol.2A p.3-360](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=478)).
It specifies the old `ST(i)*ST(0)` product before pop; exact results clear C1,
and C0/C2/C3 are undefined. General rounding, exceptions, special values,
CR0/#NM, native x87 execution and hardware state comparison remain outside
this local profile.

Focused interpreter, decoder, fixture-schema and runner tests passed during
implementation. Full workspace tests, workspace check, Linux-target runner
check, formatting and diff check passed. The layout check reports only the
pre-existing unmapped `.serena/` root directory, left untouched. Linux native
rejection tests are compile-only on this macOS host. Independent review found
one initial i=0 overflow vector that was inexact before reaching range packing;
it was corrected to use an exact power-of-two significand at exponent `0x7ffe`
and re-reviewed. Graphify update completed. No Mac mini, SSH, hardware
execution or commit was performed; physical acceptance remains reserved for
the shared final acceptance stage.

## S2-30r — exact-only FDIVP ST(i), ST(0) (bounded local slice)

Added exact-only bare `DE F8+i`, `i=0..7`, as `FDIVP ST(i), ST(0)` using the
shared ordered FDIV result gate. It computes old `ST(i)/ST(0)`, requires zero
normalized quotient remainder before precision-64 packing, and accepts only a
canonical Normal result. The gate requires x87 state, FCW `0x037f`,
`FSW & 0x80ff == 0`, Valid tags for used operands and canonical Normal input
encodings. Unequal exponents are supported. Zero, special/noncanonical inputs,
inexact quotient, overflow and subnormal/zero output reject before mutation.
For `i=0`, one Valid tag is sufficient and self-division yields exact `+1`.

Success writes to old physical `(TOP+i)&7`, empties old physical TOP, advances
TOP, clears C1, records starting RIP as FIP, advances RIP by two and retires
once. In the i=0 alias case, `+1` is written into old TOP before its tag is
emptied. All other x87 state, GPRs and RFLAGS are preserved. The decoder accepts
all DE F8..FF bytes, leaves DE F0..F7 unsupported and preserves the existing
FADDP/FSUBRP/FSUBP neighbors. Prefixes, REX, WAIT and truncation remain
unsupported. The fixture schema and runner cover all TOP×i successful forms
plus an atomic inexact `DE F9` Unsupported case with no stop RIP and budget one.
Native x87 admission remains closed.

Literal interpreter coverage checks every TOP×i physical result and complete
post-state using `15/3 = 5` to catch operand reversal. Additional vectors cover
all sign combinations, unequal exponents, full-width significands, alias
payload/tag order, inexact `1/3` and `2/3`, exact overflow, subnormal output,
bad FCW/FSW, bad tags, raw non-Normal operands, missing state, and fetch/STOP/
budget boundaries. D8 FDIV acceptance and rejection tests still pass after
generalizing the shared helper to ordered logical indices.

Normative source: Intel SDM revision 093, Vol.2A pp.3-333–3-335,
“FDIV/FIDIV—Divide” ([official PDF, Vol.2A p.3-333](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=451)).
It specifies old `ST(i)/ST(0)` before popping old `ST(0)`, clears C1 for an
exact result, and leaves C0/C2/C3 undefined. General rounding, exceptions,
special values, CR0/#NM, hardware state observation and native x87 behavior
remain outside this local profile.

Validation passed: `cargo test --workspace --offline`,
`cargo fmt --all -- --check`, `cargo check --workspace --all-targets --offline`,
`cargo check -p od-x86-runner --all-targets --target x86_64-unknown-linux-gnu --offline`,
and `git diff --check`. The Linux target compiled native deny coverage; it could
not run on this arm64 macOS host. `python3 tools/check_layout.py` reports only
the pre-existing unmapped `.serena/` root directory, left untouched.
`graphify update .` completed. Independent gpt-6.1-sol medium review reported
no findings. No Mac mini, SSH, physical hardware execution or commit was used;
hardware acceptance remains reserved for the shared acceptance stage.

## S2-30s — exact-only FDIVRP ST(i), ST(0) (bounded local slice)

Added bare `DE F0+i`, `i=0..7`, as exact-only `FDIVRP ST(i), ST(0)`. The
operation reuses the ordered divide gate with old `ST(0)` as numerator and old
`ST(i)` as denominator. It requires zero normalized quotient remainder before
precision-64 packing and accepts only a canonical Normal result. The gate
requires an existing x87 state, FCW `0x037f`, `FSW & 0x80ff == 0`, Valid tags
for used operands and canonical Normal input encodings; unequal exponents are
allowed. Zero, special/noncanonical inputs, inexact quotient, overflow and
subnormal/zero output reject before mutation. The `i=0` alias uses one Valid
tag and yields exact `+1` for either operand sign.

Success writes old physical `(TOP+i)&7`, empties old physical TOP, increments
TOP, clears C1, records starting RIP as FIP, advances RIP by two and retires
once. In the alias case it writes `+1` before emptying the same tag. All other
x87 fields, tags, payloads, GPRs and RFLAGS remain unchanged. Exhaustive DE
ModR/M decoding preserves FADDP, FMULP, FSUBRP, FSUBP and FDIVP; prefixes,
REX, WAIT and truncation remain unsupported. Schema admits all eight forms and
represents an inexact `DE F1` as Unsupported with no stop RIP and budget one.
Native x87 admission remains closed.

Interpreter vectors cover every TOP×i placement, `15/3 = 5` to catch operand
reversal, all result sign combinations, differing exponents, a full-width
significand divided by one, and positive/negative i=0 self-division. Full-state
atomic rejection covers inexact `1/3` and `2/3`, overflow, subnormal output,
bad FCW/FSW, Empty/Special tags, raw zero/unnormal/pseudo-denormal/infinity/NaN
inputs and missing x87 state. Truncated fetch, fetch fault, stop and zero budget
do not commit. The testkit runner checks the all-index matrix and the inexact
Unsupported path.

Normative source: Intel SDM revision 093, Vol.2A pp.3-336–3-338,
“FDIVR/FDIVRP/FIDIVR—Reverse Divide” ([official PDF, Vol.2A p.3-336](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=454)).
It specifies old `ST(0)/ST(i)` before popping old `ST(0)`, clears C1 for exact
results and leaves C0/C2/C3 undefined. General rounding, exceptions, special
values, CR0/#NM, native x87 execution and hardware state observation remain
outside this local profile.

Validation passed: `cargo test --workspace --offline`,
`cargo fmt --all -- --check`, `cargo check --workspace --all-targets --offline`,
`cargo check -p od-x86-runner --all-targets --target x86_64-unknown-linux-gnu --offline`,
`git diff --check`, and `graphify update .`. Linux-target compilation includes
the native x87 denial coverage; it was compile-only on this arm64 macOS host.
`python3 tools/check_layout.py` reports only the pre-existing unmapped
`.serena/` root directory, left untouched. Independent gpt-6.1-sol medium
review found two test/documentation issues, both corrected; re-review found no
remaining findings. No Mac mini, SSH, physical hardware execution or commit
was used; physical acceptance remains reserved for the shared acceptance
stage.

## S2-30t — exact-only FDIVR ST(0), ST(i) (bounded local slice)

Added bare `D8 F8+i`, `i=0..7`, as exact-only `FDIVR ST(0), ST(i)`. It reuses
the ordered exact-division gate with old `ST(i)` as numerator and old `ST(0)`
as denominator. The gate requires zero normalized quotient remainder before
precision-64 packing and accepts only a canonical Normal result. It retains
the established environment, tag and canonical Normal input requirements;
unequal exponents are allowed. Zero, special/noncanonical inputs, inexact
quotients, overflow and subnormal/zero outputs reject before mutation.

Success overwrites old physical TOP without changing tags or TOP, clears C1,
records starting RIP as FIP, advances RIP by two and retires once. The `i=0`
alias computes exact `+1` into the same payload while retaining its Valid tag.
The decoder covers all D8 ModR/M bytes and preserves neighboring groups;
prefixes, REX, WAIT and truncated fetch remain unsupported. Schema and runner
coverage admits all eight encodings and represents inexact `D8 F9` as
Unsupported with no stop RIP and budget one. Native admission stays closed.

Interpreter and testkit vectors cover every TOP×i placement, ordered
`ST(i)=15 / ST(0)=3 => 5`, signs, differing exponents, a full-width
significand, i=0, and full-state atomic rejection of actual inexact `1/3`,
overflow, subnormal output, invalid environment/tags/classes and missing x87
state. Truncated and faulted fetches, stop-at-RIP and zero-budget boundaries
preserve state. Schema tests admit all eight encodings and select the no-stop
Unsupported fixture shape; testkit execution confirms the `D8 F9` operands
really are inexact. Native x87 denial was compiled for the Linux runner target
but was not executed on this arm64 macOS host.

Normative source: Intel SDM revision 093, Vol.2A pp.3-336–3-338,
“FDIVR/FDIVRP/FIDIVR—Reverse Divide” ([official PDF, Vol.2A p.3-336](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=454)).
General rounding, architectural exception effects, special values, CR0/#NM,
native x87 state observation and full x87 behavior remain outside this slice.

Validation passed: `cargo test --workspace --offline`,
`cargo fmt --all -- --check`, `cargo check --workspace --all-targets --offline`,
`cargo check -p od-x86-runner --all-targets --target x86_64-unknown-linux-gnu --offline`,
`git diff --check`, and `graphify update .`. The layout checker reports only
the pre-existing unmapped `.serena/` root directory; it was left untouched.
The Linux native-denial tests compiled but were not executed on this arm64
macOS host. No Mac mini, SSH, physical hardware execution or commit was used.
Independent gpt-6.1-sol medium review found and corrected D8 F9 terminology in
the plan, contract and fixture notes; the updated wording is under re-review.

## S2-30u — exact-only FDIVR ST(i), ST(0) (bounded local slice)

Added bare `DC F0+i`, `i=0..7`, as exact-only `FDIVR ST(i), ST(0)`. The
existing ordered exact-division gate computes old `ST(0)/ST(i)`, requires zero
normalized quotient remainder before precision-64 packing and accepts only a
canonical Normal result. Unequal exponents are allowed; zero, special or
noncanonical operands, inexact quotients, overflow and subnormal/zero outputs
reject before state changes.

Success writes old physical `(TOP+i)&7`, clears C1, records starting RIP as
FIP, advances RIP by two and retires once without popping or changing tags/TOP.
The `i=0` alias produces exact +1 in place. DC F0..F7 now decode while the
existing FADD/FMUL/FSUBR/FSUB groups remain distinct; prefixed forms, WAIT and
truncation remain unsupported. Native x87 admission explicitly rejects all
eight new encodings.

Local vectors cover all 64 TOP/index pairs, asymmetric `6/3=2` ordering,
`15/3=5`, all four result sign pairs, differing exponents, lower/upper Normal
edges, and full-state atomic rejection of inexact `1/3`, bad FCW/FSW/tags,
zero/special/noncanonical operands, overflow, subnormal output and missing
x87 state. Fetch truncation/fault, STOP and exhausted budget preserve state.
Decoder tests enumerate all DC ModR/M values, reject prefixes/REX/WAIT, and
check truncation. The v2 fixture schema accepts all eight interpreter forms
and permits the inexact `DC F1` Unsupported/no-stop shape; testkit runner
coverage executes all TOP×i placements and verifies full-state inexact
rejection. Native admission rejects every DC F0..F7 form.

Normative source: Intel SDM revision 093, Vol.2A pp.3-336–3-338,
“FDIVR/FDIVRP/FIDIVR—Reverse Divide” ([official PDF, Vol.2A p.3-336](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=454)).
It specifies old `ST(0)/ST(i)` into old `ST(i)` with no pop; exact results
clear C1, while C0/C2/C3 are undefined. General rounding, architectural
exceptions, special values, CR0/#NM, native x87 state observation and full x87
behavior remain outside this slice.

Validation passed: focused `od-isa` S2-30u tests and `od-testkit` x87 schema/
runner tests, `cargo test --workspace --offline`, `cargo fmt --all -- --check`,
`cargo check --workspace --all-targets --offline`, Linux x86 runner target
compile, `git diff --check`, and `graphify update .`. `python3
tools/check_layout.py` reports only the pre-existing unmapped `.serena/` root
directory; it was left untouched. Linux native-denial tests compiled but were
not executed on this arm64 macOS host. No Mac mini, SSH, physical hardware
execution or commit was used; the independent review is pending.

## S2-30v — exact-only FDIV ST(i), ST(0) (bounded local slice)

Added bare `DC F8+i`, `i=0..7`, as exact-only `FDIV ST(i), ST(0)`. The
existing ordered exact-division gate computes old `ST(i)/ST(0)`, requires zero
normalized quotient remainder before precision-64 packing and accepts only a
canonical Normal result. Unequal exponents are allowed; zero, special or
noncanonical operands, inexact quotients, overflow and subnormal/zero outputs
reject before state changes.

Success writes old physical `(TOP+i)&7`, clears C1, records starting RIP as
FIP, advances RIP by two and retires once without popping or changing tags/TOP.
The `i=0` alias produces exact +1 in place. DC F8..FF now decode while
preserving DC F0..F7 FDIVR and the existing FADD/FMUL/FSUB groups; prefixed
forms, WAIT and truncation remain unsupported. Native x87 admission explicitly
rejects all eight new encodings.

Local vectors cover all 64 TOP/index pairs, ordered `15/3=5`, the i=0 +1
alias, sign combinations, differing exponents, lower/upper Normal edges and
full-state atomic rejection of inexact `1/3`, invalid environment/tags,
zero/special/noncanonical operands, overflow, subnormal output and missing
x87 state. Fetch truncation, STOP and exhausted budget preserve state. Decoder
tests enumerate all DC ModR/M values, reject prefixes/REX/WAIT and check
truncation. The v2 fixture schema accepts all eight forms and permits the
inexact `DC F9` Unsupported/no-stop shape; testkit runner coverage executes all
TOP×i placements and verifies full-state inexact rejection. Native admission
rejects every DC F8..FF form.

Normative source: Intel SDM revision 093, Vol.2A pp.3-333–3-335,
“FDIV/FIDIV—Divide” ([official PDF, Vol.2A p.3-333](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=451)).
It specifies old `ST(i)/ST(0)` into old `ST(i)` with no pop; exact results
clear C1, while C0/C2/C3 are undefined. General rounding, architectural
exceptions, special values, CR0/#NM, native x87 state observation and full x87
behavior remain outside this slice.

Validation passed: the focused `od-isa` S2-30v tests, `od-testkit` schema and
runner tests, `cargo test --workspace --offline`, `cargo fmt --all -- --check`,
`cargo check --workspace --all-targets --offline`, Linux x86 runner target
compile (`cargo check -p od-x86-runner --all-targets --target
x86_64-unknown-linux-gnu --offline`), `git diff --check` and `graphify update .`.
`python3 tools/check_layout.py` reports the unmapped root `.serena/` directory;
it was left untouched. Linux native-denial tests compiled for the x86_64 target
but were not run on this arm64 macOS host. No Mac mini, SSH or physical hardware
checks were run, as those remain in the shared acceptance stage. Independent
gpt-6.1-sol high review found one documentation mismatch: signs and Normal
range edges were attributed to the testkit runner instead of the `od-isa`
literal vectors. The fixture notes were corrected and the re-review found no
remaining findings.

## S2-30w — bounded FSTP ST(i) local raw-copy slice

Added only bare `DD D8+i`, `i=0..7`, as a bounded register-register
`FSTP ST(i)` form. It requires an existing x87 snapshot, FCW `0x037f`,
`FSW & 0x80ff == 0`, and a Valid canonical-Normal old ST(0). The interpreter
captures old TOP and exact ten source bytes before mutation, copies those bytes
to old physical `(TOP+i)&7`, marks that destination Valid, empties old TOP and
advances TOP. It never rotates the physical register array and does not read
the destination's previous payload or tag. For `i=0` the copied payload remains
with an Empty tag; for `i=1` the copy is the new ST(0). FIP receives starting
RIP, RIP advances by two, and the instruction retires once. C1 is preserved as
the local deterministic policy; unrelated physical payloads/tags and FCW, FDP,
FOP, GPRs and RFLAGS remain unchanged. Native execution admission denies all
eight encodings.

`od-isa` vectors cover all 64 TOP/index pairs and each old destination tag for
`i!=0`, raw signed/minimum/maximum Normal payloads, unchanged sentinels, and
alias/pop behavior. Invalid source tags/classes, FCW/FSW, missing x87, fetch
faults/truncation, STOP and budget exhaustion reject atomically. Decoder tests
enumerate DD ModR/M values and cover prefix, REX, WAIT and truncation rejection.
The fixture schema admits all eight instructions; runner tests execute all
indices and verify the full result. A dedicated no-stop/budget-one fixture
rejects an Empty-tagged old ST(0), confirming source validation rather than
destination validation.

Normative source: Intel SDM revision 093, Vol.2A pp.3-386–3-387,
“FST/FSTP—Store Floating-Point Value” ([official PDF, Vol.2A p.3-386](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=504)).
This bounded slice does not implement memory forms, arithmetic, #NM/stack-fault
delivery or broader x87 semantics.

Validation passed: `cargo test --workspace --offline`, focused `od-isa`,
`od-testkit` schema, and CLI runner tests, `cargo fmt --all -- --check`,
`cargo check --workspace --all-targets --offline`, Linux x86_64 runner target
compile (`cargo check -p od-x86-runner --all-targets --target
x86_64-unknown-linux-gnu --offline`), `git diff --check`, and `graphify update .`.
`python3 tools/check_layout.py` reports the pre-existing unmapped root
`.serena/` directory; it was left untouched per task instructions. Linux-gated
native-denial tests compiled but were not run on this arm64 macOS host. No Mac
mini, SSH or physical-hardware checks were run; these remain in the shared
acceptance stage. Independent review is pending.

## S2-30x — bounded FST ST(i) local raw-copy slice

Added only bare `DD D0+i`, `i=0..7`, as a bounded register-register
`FST ST(i)` form. It shares the source validation used by S2-30w: existing
x87 state, FCW `0x037f`, `FSW & 0x80ff == 0`, and a Valid canonical-Normal old
ST(0). The interpreter captures the exact ten source bytes before mutation,
writes them to old physical `(TOP+i)&7`, and marks that destination Valid
without rotating the register array. It does not inspect the destination's
previous payload or tag. TOP and the complete FSW remain unchanged, including
for `i=0`; FIP receives starting RIP, RIP advances by two and one instruction
retires. FCW, FDP, FOP, unrelated payloads/tags, GPRs and RFLAGS are preserved.
Native admission explicitly denies all eight encodings.

`od-isa` vectors cover all 64 TOP/index pairs, each previous destination tag
for `i!=0`, raw signed/minimum/maximum Normal values, full FSW preservation,
and complete-state rejection for invalid source/environment, missing x87,
fetch/truncation, STOP and budget boundaries. Exhaustive DD ModR/M checks
preserve D8..DF as FSTP; prefix, REX, WAIT and truncation are rejected. The
fixture schema admits all eight forms and the literal DD D1 source-tag reject
fixture. CLI runner coverage verifies all destination indices and complete
x87 snapshots.

Normative source: Intel SDM revision 093, Vol.2A pp.3-386–3-387,
“FST/FSTP—Store Floating-Point Value” ([official PDF, Vol.2A p.3-386](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=504)).
This bounded slice preserves all FSW bits as a deterministic local policy and
does not claim hardware validation. Memory forms, arithmetic, #NM/stack-fault
delivery and broader x87 semantics remain unsupported.

Validation passed: `cargo fmt --all -- --check`, `cargo test --workspace --offline`
(all workspace tests), `cargo check --workspace --all-targets --offline`, the
focused fixture schema and CLI runner tests, `cargo check -p od-x86-runner
--all-targets --target x86_64-unknown-linux-gnu --offline`, `git diff --check`,
and `graphify update .`. `python3 tools/check_layout.py` reports the pre-existing
unmapped root `.serena/` directory; it was left untouched. The x86_64 Linux
runner target compiled, but its native-denial test was not run on this arm64
macOS host. No Mac mini, SSH or physical-hardware checks were run; those remain
in the shared acceptance stage. Independent gpt-6.1-sol medium review completed
with no findings.

## S2-30y — bounded FCHS sign-bit slice

Added only bare `D9 E0` for an existing x87 snapshot with FCW `0x037f`,
`FSW & 0x80ff == 0`, Valid old physical TOP tag and canonical-Normal ST(0).
The interpreter snapshots the ten source bytes, flips only byte 9's sign bit,
clears C1, records starting RIP as FIP and retires once. TOP, tags, C0/C2/C3
and other state are preserved; retaining C0/C2/C3 is the local deterministic
snapshot policy. The shared bounded source gate now has an operation-neutral
name and is reused by FST/FSTP and FCHS.

Literal interpreter vectors cover all TOP values, positive and negative
minimum/maximum/full-significand Normal payloads, initial C1 clear/set and two
FCHS operations restoring the payload while C1 stays cleared. Full-state rejects
cover zero, subnormal and special encodings, non-Valid tags, invalid control or
status, absent state, truncation, fetch fault, STOP and exhausted budget. The
decoder enumerates all 256 D9 second bytes, accepting only E0, and checks prefix,
REX, WAIT and truncation rejection. Schema and CLI fixtures cover successful
sign-flip behavior and a genuine Empty-tag rejection with no stop RIP and budget
one. Native x87 admission explicitly denies `D9 E0`.

This local slice excludes zero/subnormal/special FCHS semantics, all other D9
forms, memory, CR0/#NM, exception delivery, native execution and FPU feature
claims. It is not hardware validation of general x87 behavior.

Validation passed: `cargo test --workspace --offline`, focused FCHS decoder and
interpreter vectors, schema/CLI success and rejection fixtures, `cargo check
--workspace --all-targets --offline`, Linux x86_64 runner compile-only check,
`cargo fmt --all -- --check`, `git diff --check`, and `graphify update .`.
`python3 tools/check_layout.py` reports the pre-existing unmapped root
`.serena/` directory; it was left untouched. The Linux runner target compiled
but was not executed. No Mac mini, SSH or physical-hardware checks were run.
The first gpt-6.1-sol medium review found that the new `D9 E0` test vector was
missing `Operation::Fchs` from the native backend's explicit x87-denial arm.
That guard now rejects FCHS with the same x87-specific admission error as the
other bounded x87 operations. The Linux x86_64 all-target compile passed after
the fix. The Linux-only native-denial test could not run on this arm64 macOS
host; no `qemu-x86_64` is installed. Independent gpt-6.1-sol medium re-review
verified the explicit native x87 denial for `Operation::Fchs` and reported no
remaining findings.

## S2-30z — bounded FABS sign-clear slice

Added exactly bare `D9 E1` for an existing x87 snapshot with FCW `0x037f`,
`FSW & 0x80ff == 0`, Valid old physical TOP tag and canonical-Normal ST(0).
The shared bounded source gate validates before commit; success snapshots all
ten bytes and clears only `source[9]` bit 7, clears C1, preserves TOP/tags and
all remaining x87/CPU state, records starting RIP as FIP and retires once.
C0/C2/C3 retention is the deterministic local snapshot policy.

Literal interpreter vectors cover all eight TOPs, positive/negative minimum,
maximum and full-significand Normal values, C1 initially clear/set, retention of
C0/C2/C3 sentinels, and repeated FABS idempotence. Full-state rejects cover
missing state, invalid FCW/FSW, invalid tag/class, truncation, fetch fault, STOP
and zero budget. Exhaustive D9 second-byte decode admits E0→FCHS and E1→FABS
only, with prefix/REX/WAIT/truncation rejection. The fixture schema admits the
positive literal and a no-stop/budget-one invalid-source case; CLI checks the
exact post-state and zero-retirement atomic rejection. Native admission
explicitly denies `Operation::Fabs` and D9 E1.

This bounded slice excludes zero/subnormal/special values, memory, CR0/#NM,
exception delivery, native execution and FPU feature claims. General x87
architectural behavior remains open.

Normative source: Intel SDM revision 093, Vol.2A, “FABS—Absolute Value,” p.3-309
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=427)).

Validation passed: `cargo test --workspace --offline`, `cargo check --workspace
--all-targets --offline`, focused fixture schema/CLI tests, Linux x86_64 runner
all-targets compile-only check, `cargo fmt --all -- --check`, `git diff --check`
and `graphify update .`. `python3 tools/check_layout.py` reports the pre-existing
unmapped root `.serena/` directory; it was left untouched. The Linux native
denial test was not run on this arm64 macOS host. No Mac mini, SSH or physical
hardware checks were run. Independent gpt-6.1-sol review found no defects.

## S2-30aa — bounded FTST canonical-Normal comparison slice

Added bare `D9 E4` to the exhaustive D9 table as the third and only new accepted
second byte after E0/FCHS and E1/FABS. The interpreter requires an existing
x87 snapshot, FCW `0x037f`, `FSW & 0x80ff == 0`, a Valid physical TOP tag and
canonical-Normal ST(0) via the shared bounded source gate. It derives C0 from
the sign bit, clears C1, clears C2/C3, preserves TOP/tags/all raw physical
payloads and other state, records FIP and retires once. This deliberately leaves
signed zero, special/subnormal values, exceptions, stack faults, CR0/#NM, memory
forms and full architectural x87 behavior out of scope.

Literal vectors cover all eight TOP values, positive/negative minimum and
maximum Normal endpoints plus signed full-width-significand examples, each of
all 16 initial C3/C2/C1/C0 combinations, complete payload/tag sentinels,
missing state, invalid FCW/FSW/tag/class, truncation, fetch failure, STOP and
zero budget with full-state no-commit checks. FTST followed by FNSTSW AX confirms
the status update is visible. Two literal success fixtures cover positive and
negative comparisons; a third has canonical-Normal bytes with an Empty physical
TOP tag, `stop_rip: null`, and instruction budget one. The fixture schema and
CLI verify the success state, zero-retirement rejection and atomicity. Native
admission explicitly denies `Operation::Ftst`, and its x87 deny regression
vector includes `D9 E4`; the prior explicit `FCHS` denial finding remains
covered by its existing `D9 E0` vector and arm.

Normative source: Intel SDM revision 093, Vol.2A, “FTST—Test,” pp.3-400–3-401
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).


Validation passed: the full `cargo test --workspace --offline` suite completed
successfully three times, including after the final shared FCOMI/FUCOMI commit
arm; `cargo check --workspace --all-targets --offline` and Linux x86_64 compile-only
`cargo check -p od-x86-runner --target x86_64-unknown-linux-gnu --all-targets
--offline`, focused FTST interpreter/decode/schema/CLI tests,
`cargo fmt --all -- --check`, `git diff --check` and `graphify update .`.
`python3 tools/check_layout.py` still reports the pre-existing unmapped root `.serena/`
directory; it was left untouched. Linux native admission tests were not run on
the arm64 macOS host. No Mac mini, SSH or physical-hardware checks were run.
Independent gpt-6.1-sol medium review completed with no findings.

## S2-31a — bounded register-only FRNDINT partial slice

Implemented bare `D9 FC` for canonical Normal binary80 values in physical ST(0)
with unbiased exponent `0..62`, the four PC64/all-masked FCW values, and all four
rounding modes. The result is computed with integer-only rounding and packed
before commit. Unsupported encodings, invalid state/environment, special
values, exponent boundaries, and execution boundaries reject atomically. The
native backend remains explicitly closed. Literal fixtures cover +1.5 ties,
FNSTSW observation, and an Empty physical-TOP rejection. Interpreter tests also
use hand-calculated raw binary80/status tables for exact +1.0, sticky PE, every
rounding direction for signed +1.25/-1.25, even/odd half ties, adjacent values
around +1.5, and signed values immediately below 2^63. These expected bytes do
not call the production rounding or packing helpers. S2-31 remains partial; this
does not implement full FRNDINT architectural behavior.

Validation passed: focused `cargo test -p od-isa --offline frndint` (5 tests),
full `cargo test --workspace --offline` (329 `od-isa` tests and all workspace
suites),
`cargo check --workspace --all-targets --offline`, Linux x86_64 compile-only
`cargo check -p od-x86-runner --target x86_64-unknown-linux-gnu --all-targets
--offline`, `cargo fmt --all -- --check`, and `git diff --check`. The Linux-only
native admission regression was compile-checked but could not be executed on
this macOS host. `python3 tools/check_layout.py` reports only the pre-existing
unmapped root `.serena/` directory, which was left untouched. `graphify update .`
completed with 3603 nodes, 7689 edges, and 209 communities; the existing
extractor warning for `include/open_desktop/host_abi.h` remains. Physical x86
execution, native x87 state comparison, exception delivery, and out-of-slice
inputs were not tested.

Normative source: Intel SDM revision 093, Vol.2A, “FRNDINT—Round to Integer,”
p.3-372 ([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

## S2-31dc — canonical signed zero through bounded FRNDINT

The existing bare `D9 FC` gate now accepts either canonical signed binary80
zero only when physical TOP is tagged Zero and the raw significand and
sign/exponent exactly match +0 or -0. It preserves all source bytes, Zero/TOP,
PE and C0/C2/C3, clears C1, updates FIP/RIP and retires once. The existing FCW
whitelist and `FSW & 0x80df` mask are unchanged. Mismatched tags/payloads and
all prior unsupported classes remain atomic; no memory or stack changes were
introduced. The Normal-only independent rounding vectors remain unchanged.

The local 256-case matrix and literal negative-zero/FNSTSW fixture validate
the bounded interpreter profile. Native/JIT remain denied. Physical x86
behavior and broader exception semantics are deferred to the combined final
S2-31 substage; S2-31 remains partial under ISSUE-019.

Validation passed: `cargo test -p od-isa --offline frndint` (6 tests), the
literal fixture runner test
`cargo test -p od-runner --offline frndint_signed_zero_literal_fnstsw_fixture_preserves_state_and_sign`,
`cargo check --workspace --all-targets --offline`, and Linux x86_64 compile-only
`cargo check -p od-x86-runner --target x86_64-unknown-linux-gnu --all-targets
--offline`. `cargo fmt --all -- --check`, `git diff --check`, and
`python3 tools/check_layout.py` passed. `graphify update .` completed with 4659
nodes, 9415 edges, and 172 communities. Physical x86/native execution and Linux
runtime denial tests were not run on the arm64 macOS host; the final combined
S2-31 substage remains responsible for physical acceptance.

## S2-31dd — exact-integer Normal values through bounded FRNDINT

Extended bare `D9 FC` to admit Valid canonical-Normal binary80 values with
unbiased exponent `63..=16383`. Since these values are exact integers, the
preflight returns their original ten bytes before computing `63-e`, rounding,
or packing. Success preserves every tag, TOP, PE, C0/C2/C3, FDP/FOP and
unrelated state, clears C1, records FIP, advances RIP by two and retires once.
The FCW whitelist and `FSW & 0x80df` mask are unchanged. Invalid tags, J=0,
exponent `0x7fff`, forbidden controls/status and execution boundaries remain
atomic rejects. Native/JIT admission remains closed and no data or stack access
was added.

The focused matrix spans exponents 63, 64, 127, 1023, 16383; four literal
significand patterns; both signs; all eight TOP values; all four FCWs; and PE/C1
combinations. The explicit e=62 fractional versus e=63 identity boundary
confirms rounding/PE versus exact preservation. A literal FNSTSW fixture checks
the CLI-visible result and status. Physical x87 comparison and general
exception delivery remain unverified under ISSUE-019.

Validation passed: focused `cargo test -p od-isa --offline frndint` (8 tests),
literal CLI fixture test
`cargo test -p od-runner --offline frndint_large_exact_integer_literal_fnstsw_fixture_preserves_raw_value` (1 test),
`cargo check --workspace --all-targets --offline`, and Linux x86_64 compile-only
`cargo check -p od-x86-runner --target x86_64-unknown-linux-gnu --all-targets --offline`.
`cargo fmt --all -- --check`, `git diff --check`, and
`python3 tools/check_layout.py` passed. `graphify update .` rebuilt 4666 nodes,
9427 edges, and 182 communities; its existing extractor warning for
`include/open_desktop/host_abi.h` remains. Physical x86/native execution and
Linux runtime denial tests were not run on this arm64 macOS host.

## S2-31de — canonical signed infinity through bounded FRNDINT

Bare `D9 FC` now accepts canonical positive or negative binary80 infinity only
with a Special tag, exact explicit integer bit, exponent `0x7fff`, and zero
fraction. It preserves the raw value, tag, TOP, sticky PE and C0/C2/C3; clears
C1; records FIP; advances RIP by two; and retires once. The FCW whitelist and
`FSW & 0x80df` gate are unchanged. Pseudo-infinities, NaNs, other exponent or
fraction patterns, tag mismatches, and invalid controls/status remain atomic
rejections. Existing zero and Normal paths are unchanged.

The 256-case test matrix spans sign, all four FCWs, all eight TOP values, PE,
and prior C1 while comparing literal raw bytes and the full expected x87
snapshot. Additional rejects cover pseudo-infinity, NaN payloads, malformed
Special values, mismatched tags and environment gates. The literal CLI fixture
loads negative infinity from m80 memory, executes `FRNDINT`, and observes the
status with `FNSTSW AX`; it checks the pushed raw payload, Special tag, TOP,
pointers, canary state and status. The corresponding JIT request returns
Unsupported without interpreter fallback. Native x87 admission remains
explicitly denied. Physical x86 acceptance and general exception behavior
remain deferred to the combined final S2-31 substage under ISSUE-019.

Validation passed: all 9 focused `cargo test -p od-isa --offline frndint`
tests, the literal CLI fixture test
`cargo test -p od-runner --offline frndint_loaded_negative_infinity_literal_fixture_preserves_raw_value`,
and the JIT denial test
`cargo test -p od-runner --offline v2_jit_denies_loaded_infinity_frndint_without_interpreter_fallback`.
Host `cargo check --workspace --all-targets --offline` and Linux x86_64
`cargo check --workspace --all-targets --target x86_64-unknown-linux-gnu --offline`
passed; the cross-check reports existing dead-code warnings in `od-host` and
`od-runner`. `cargo fmt --all -- --check`, `git diff --check`, and
`python3 tools/check_layout.py` passed. `graphify update .` refreshed the graph
to 4674 nodes, 9439 edges, and 180 communities; its existing extractor warning
for `include/open_desktop/host_abi.h` remains. The Linux native-denial test was
compile-checked but not run on this arm64 macOS host. Independent gpt-6.1-sol
review reported no code, fixture, or contract findings. Physical x86 execution
and Mac mini acceptance remain deferred to the combined final S2-31 substage.

## S2-31df — canonical quiet NaNs through bounded FRNDINT

Bare `D9 FC` admits Special-tagged canonical binary80 QNaNs only when the
exponent is `0x7fff`, J and bit 62 are set, either sign is used, and lower 62
payload bits are unrestricted. It preserves all ten bytes, Special/TOP, sticky
PE, C0/C2/C3, FDP/FOP and unrelated state; clears C1; adds no flags; updates
FIP/RIP; and retires once. The existing FCW and status gates remain intact.

Interpreter coverage includes four significands, both signs, all four FCWs,
all TOP values, incoming PE and both old C1 values (1,024 cases), plus malformed
class/tag, control/status and execution-boundary atomic rejects. The literal
`FLD m80fp`→`FRNDINT`→`FNSTSW AX` fixture checks the full payload and status.
JIT compilation and native x87 admission remain denied. Physical x86 behavior
and general exception delivery remain deferred under ISSUE-019.

Validation passed: `cargo test -p od-isa --offline frndint` (10 tests), the literal CLI fixture test `cargo test -p od-runner --offline frndint_loaded_quiet_nan_literal_fixture_preserves_payload_and_status` (1 test), host `cargo check --workspace --all-targets --offline`, Linux x86_64 `cargo check --workspace --all-targets --target x86_64-unknown-linux-gnu --offline`, `cargo fmt --all -- --check`, `git diff --check`, `python3 tools/check_layout.py`, and `graphify update .` (4,680 nodes, 9,449 edges, 168 communities). The macOS-filtered native admission regression had no matching tests; the native profile's explicit `Frndint` rejection remains present in `native_v2.rs`. Physical x86/native runtime acceptance remains deferred under ISSUE-019.

## S2-31dg — canonical signaling NaNs through bounded FRNDINT

Bare `D9 FC` now admits Special-tagged canonical binary80 SNaNs with exponent
`0x7fff`, J=1, bit 62 clear and a nonzero lower 62-bit payload. It sets only
bit 62 in the significand, preserving sign, exponent, J, lower payload, tag,
TOP and unrelated state. Commit adds sticky IE, clears C1, preserves PE and
C0/C2/C3, updates FIP/RIP and retires once without memory or stack access.
The existing FCW whitelist and `FSW & 0x80df` gate remain unchanged, so
historical IE rejects re-entry until FNCLEX.

Interpreter coverage includes four literal significands × both signs × all
four FCWs × every TOP × incoming PE × prior C1 (1,024 cases), with independent
result and status assertions. Boundary/source tests confirm the existing
zero-payload infinity and QNaN identities; rejects include J=0 and
wrong tags. SNaN-specific invalid controls/status,
prefix, fetch, truncation, stop and budget cases are atomic. The local sequence
checks IE observation by FNSTSW, re-entry rejection, FNCLEX, and a successful
second FRNDINT. A seeded literal CLI fixture executes `FLD m80fp`→`FRNDINT`→
`FNSTSW AX`→`FNCLEX`→`FRNDINT`. JIT/native admission remains denied. Physical
x86/native runtime acceptance remains deferred to combined S2-31 acceptance
under ISSUE-019.

Validation: `cargo test -p od-isa --offline` passed all 603 tests; `cargo test -p od-runner --offline` passed 95 unit tests and 145 fixture tests, including the S2-31dg literal case. Host and Linux x86_64 `cargo check --workspace --all-targets --offline` passed (the Linux-target check reports existing dead-code warnings in `od-host` and `od-runner`). `cargo fmt --all -- --check`, `git diff --check`, and `python3 tools/check_layout.py` passed. `graphify update .` completed at 4,688 nodes, 9,466 edges and 180 communities; a final update after the last test edits found no graph topology changes. Physical x86/Mac mini acceptance remains deferred under ISSUE-019.
## S2-31dh — historical sticky IE through bounded FRNDINT

Changed only the bare `D9 FC` interpreter status gate to `FSW & 0x80de == 0`.
Historical IE is now admitted and retained independently of the current
source's effects: exact identities remain identities, inexact Normal rounding
adds PE, and a current SNaN is quieted and adds IE. DE/ZE/OE/UE/SF/ES/B remain
rejected, including when combined with IE. The four-FCW whitelist, all source
and tag domains, value paths, metadata, atomic execution, and JIT/native denial
remain unchanged.

The interpreter matrix covers signed zero, exact and inexact Normal, large
integral Normal, infinity, QNaN and SNaN across signs, all FCWs and TOPs, IE/PE
combinations, and both prior C1 values. It also checks IE plus each forbidden
status atomically and verifies SNaN→FRNDINT→FRNDINT succeeds without FNCLEX;
the second operation sees the quiet NaN, retains IE, adds no PE, and clears C1.
Existing fetch/truncation/STOP/budget/no-memory atomicity and the literal
FNSTSW/FNCLEX fixture sequence remain covered. Physical x86 behavior and
native runtime acceptance remain deferred under ISSUE-019.

Validation passed: `cargo test --workspace --offline` passed all 1,029 tests,
including 604 `od-isa`, 95 runner unit, 145 fixture integration, 166 testkit,
8 x86-runner supervisor, 4 JIT, 6 host and 1 REP trace tests. Focused
`cargo test -p od-isa --offline frndint` passed all 14 matching tests, and the
extended SNaN CLI fixture test passed. Host and Linux x86_64
`cargo check --workspace --all-targets --offline`, `cargo fmt --all -- --check`,
`git diff --check`, and `python3 tools/check_layout.py` passed. The Linux
cross-check reports existing dead-code warnings in `od-host` and `od-runner`;
the Linux-only native-denial runtime test could not run on this Apple Silicon
host, though the denial code was compile-checked. `graphify update .` rebuilt
4,693 nodes, 9,474 edges and 185 communities; its existing parser warning for
`include/open_desktop/host_abi.h` remains. Physical x86 and Mac mini acceptance
remain deferred under ISSUE-019.

## S2-30ab — bounded FCOM ST(1) exact Normal compare

Added exact bare `D8 D1` decode as `FcomSt1`; the existing D8 arithmetic ranges
remain unchanged. Before commit, the interpreter requires existing x87 state,
FCW `0x037f`, clear `FSW & 0x80ff`, Valid tags for both old physical TOP and
`(TOP+1)&7`, and canonical-Normal binary80 values. Numeric ordering uses sign,
finite exponent and all 64 significand bits, without host-float conversion or
subtraction. Commit replaces only FSW condition bits with greater=`0`,
less=`0x0100`, equal=`0x4000`, records instruction-start RIP in FIP, advances
RIP by two, and retires once. No register, tag, environment, other status,
GPR, RFLAGS or unrelated x87 payload changes. Both operands are checked before
mutation; invalid state rejects atomically.

Local vectors cover all TOP values, all 16 initial condition-code combinations,
all order relations, positive/negative same-sign order, opposite signs in both
orders, exponent and low-significand-bit distinctions, and minimum/maximum
Normal values. Full-state rejection vectors exercise each operand's invalid
tag/class and bad FCW/FSW, plus missing x87 state and fetch/truncation/STOP/
budget boundaries. Three handwritten fixtures cover greater/equal/less; a
fourth has Normal ST(1) payload bytes with an Empty tag and uses no stop address
and budget one. The CLI checks all four and the exact `FCOM ST(1)`→`FNSTSW AX`
sequence. The fixture schema admits the exact instruction, its real rejection
case, and only that bounded two-instruction observation sequence. Native admission
has an explicit `FcomSt1` deny arm, and the Linux-only native deny regression passes
in an amd64 Rust 1.99 Docker container. No physical x86 guest execution, SSH or Mac
mini run was made.
General x87 comparison semantics, exceptions, special values, stack effects,
native execution and FPU feature claims remain unverified; ISSUE-019 remains
open.

Normative source: Intel SDM revision 093, Vol.2A,
“FCOM/FCOMP/FCOMPP—Compare Floating-Point Values,” pp.3-324–3-326
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).


Validation passed: `cargo test --workspace --offline`,
`cargo check --workspace --all-targets --offline`,
`cargo check -p od-x86-runner --target x86_64-unknown-linux-gnu --all-targets
--offline`, focused D8 decoder/schema/interpreter/CLI tests,
Docker `cargo test -p od-x86-runner --target x86_64-unknown-linux-gnu
x87_instructions_are_rejected_by_native_profile_and_instruction_admission`,
`cargo fmt --all -- --check`, `git diff --check`, and `graphify update .`.
`python3 tools/check_layout.py` reports the pre-existing unmapped root
`.serena/` directory; it was left untouched. Independent review findings for
the native deny arm, D8 arithmetic wording, and a stale plan link were corrected;
follow-up review reported no remaining findings.

## S2-30ad — bounded FCOMP ST(1) compare and physical TOP pop

Added exact bare `D8 D9` as the zero-operand FCOMP ST(1) alias. The interpreter
reuses `bounded_x87_fcom_result(state, 1)` to validate and compare the old
physical TOP and old `(TOP+1)&7` values before mutation. It then replaces the
condition field with the exact greater/less/equal result, empties the old
physical TOP tag, advances TOP by one, records FIP, and retires once. Raw
payloads remain in their physical registers without rotation or clearing;
neighboring D8 D0..D7 FCOM and arithmetic forms retain their existing decode.

Interpreter vectors cover all eight TOP positions including wrap at seven,
all 16 initial condition-code fields, all three relations, sign/exponent/low
significand ordering, independent tag and class rejection for both operands,
missing state/environment and fetch/truncation/STOP/budget atomicity. Literal
fixtures add equal success and invalid ST(1)-tag rejection with no stop RIP and
budget one. Schema and CLI cover only D8 D9 and exact D8 D9 DF E0 status
observation. Native admission explicitly denies D8 D9.

Validation passed: `cargo test --workspace --offline` (297 `od-isa` tests,
155 `od-testkit` tests, 69 runner fixture cases and all remaining workspace
suites), `cargo check --workspace --all-targets --offline`, Linux x86_64
`cargo check -p od-x86-runner --target x86_64-unknown-linux-gnu --all-targets
--offline`, `cargo fmt --all -- --check`, `git diff --check`, and
`graphify update .`. Focused FCOMP decoder, interpreter, schema and CLI tests
also passed. The Linux-only native-deny regression was compiled but not
executed on this arm64 macOS host; no SSH or physical-hardware checks were run.
`python3 tools/check_layout.py` reports the existing unmapped root `.serena/`
directory, which was left untouched.
Independent gpt-6.1-sol medium follow-up review confirmed the fixture tag and
report ordering corrections with no remaining findings.

Full x87 comparison behavior, special values, exceptions, native execution and
FPU feature claims remain unverified; ISSUE-019 remains open.

Normative source: Intel SDM revision 093, Vol.2A,
“FCOM/FCOMP/FCOMPP—Compare Floating-Point Values,” pp.3-324–3-326
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

## S2-30ac — bounded FCOM ST(i) exact Normal compare

Generalized the operation to `FcomSt { source }` and admitted exactly bare
`D8 D0..D7`. The interpreter compares old physical TOP against old physical
`(TOP+i)&7` using sign, exponent and all 64 significand bits. For `i=0`, it
checks the shared Valid tag and canonical-Normal payload once and returns
equality. For other indices it preflights both tags and both canonical-Normal
payloads before any commit. Commit changes only FSW condition bits, writes
instruction-start RIP to FIP, advances RIP by two, and retires once. The D8
arithmetic ranges remain unchanged; `D8 D8..DF` were unsupported at this stage
and are added by S2-30ae below.

Interpreter vectors cover all 64 TOP×i placements, including alias and wrap,
all 16 initial condition-code combinations, all three relations, signed order,
exponent/low-significand distinctions, Normal endpoints, independent tag and
class rejects for both non-alias operands, alias failure, environment failure,
sentinels, and fetch/truncation/STOP/budget atomicity. Existing S2-30ab literal
fixtures remain unchanged; S2-30ac adds literal D8 D7 equality and canonical-
Normal ST(7) payload with an Empty tag. Runner vectors cover all eight source
indices. The schema admits only D8 D0..D7 and corresponding D8 D0+i DF E0
observation sequences; no-stop, budget-one invalid-tag rejection is exercised.
Native admission explicitly denies all eight encodings; its regression source
contains a separate vector for each encoding.

Validation passed: `cargo test --workspace --offline` (294 `od-isa` tests,
155 `od-testkit` tests, 67 runner fixture tests and all remaining workspace
suites), `cargo check --workspace --all-targets --offline`, Linux x86_64
`cargo check -p od-x86-runner --target x86_64-unknown-linux-gnu --all-targets
--offline`, `cargo fmt --all -- --check`, and `git diff --check`. The focused
interpreter, schema, runner CLI and decoder tests also passed. The Linux-only
native deny test was compiled but not executed on this arm64 macOS host. Independent
review found a stale S2-30ac plan anchor; it now links to the actual plan document,
and follow-up review confirmed the fix with no remaining findings. No
Mac mini, SSH or physical-hardware checks were run. `python3 tools/check_layout.py` reports the pre-existing unmapped root `.serena/`
directory; it was left untouched. Graphify regeneration completed; it rebuilt the graph after detecting changed community
labels. Its parser warning for `include/open_desktop/host_abi.h` remains.

Full x87 comparison behavior, special values, exceptions, native execution and
FPU feature claims remain unverified; ISSUE-019 remains open.

Normative source: Intel SDM revision 093, Vol.2A,
“FCOM/FCOMP/FCOMPP—Compare Floating-Point Values,” pp.3-324–3-326
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

## S2-30ae — bounded FCOMP ST(i) compare and physical TOP pop

Generalized `FcompSt1` to `FcompSt { source }` and admits exactly bare
`D8 D8+i` for i=0..7. The interpreter preflights both old physical operands
with the existing exact canonical-Normal compare helper, replaces FSW condition
bits, empties the old physical TOP tag, advances TOP with wrap and leaves all
raw register payloads in place. The i=0 alias validates its shared operand
before taking the equality fast path and still pops once. FIP records start RIP;
RIP advances by two and retirement is once. Existing D8 D0..D7 FCOM and D8
arithmetic decoding are preserved.

Interpreter vectors cover all 64 TOP×i combinations, all 16 initial condition
fields, greater/less/equal and exact signed/exponent/significand ordering,
independent lhs/rhs tag and class rejects, environment rejects, payload/tag
sentinels, and fetch/truncation/STOP/budget atomicity. Eight literal success
fixtures exercise the CLI at every i. Schema tests admit all eight codes and
each exact `D8 D8+i DF E0` observation sequence with budgets one and two; a
no-stop/budget-one invalid-tag case remains covered. Native admission explicitly
denies all eight `FcompSt { .. }` encodings.

Validation passed: `cargo test --workspace --offline` (297 `od-isa` tests,
155 `od-testkit` tests, 69 runner fixture tests, 8 x86-runner tests and all
remaining workspace suites), `cargo check --workspace --all-targets --offline`,
Linux x86_64 `cargo check -p od-x86-runner --target
x86_64-unknown-linux-gnu --all-targets --offline`, `cargo fmt --all -- --check`,
and `git diff --check`. `python3 tools/check_layout.py` reports the pre-existing
unmapped root `.serena/` directory; it was left untouched. `graphify update .`
rebuilt the graph (3475 nodes, 7438 edges); its existing parser warning for
`include/open_desktop/host_abi.h` remains. Native deny vectors compile on this
arm64 macOS host but were not executed on Linux; no SSH or physical-hardware
checks were run.
Independent gpt-6.1-sol medium review completed with no findings.

Full x87 architectural behavior, special values, exception delivery, native
execution and FPU feature claims remain unverified; ISSUE-019 remains open.

Normative source: Intel SDM revision 093, Vol.2A,
“FCOM/FCOMP/FCOMPP—Compare Floating-Point Values,” pp.3-324–3-326
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

## S2-30af — bounded FCOMPP compare and two-entry physical TOP pop

Bare `DE D9` compares old physical ST(0) with old ST(1) using the bounded
canonical-Normal comparison helper. After complete preflight it replaces the
FSW condition field, empties both source tags, advances TOP by two with wrap,
and leaves the physical register byte array untouched. The operation records
FIP, advances RIP by two and retires once; invalid operands and execution
boundaries reject atomically. Existing DE arithmetic maps remain intact and
other DE compare encodings remain unsupported.

Interpreter vectors cover all eight TOP positions, all three relations and all
16 initial condition fields; TOP=6 and TOP=7 verify wrap to 0 and 1. Tests use
raw-register sentinels and an Empty unused old ST(2), independent invalid lhs/rhs
cases, invalid environment, fetch failure, truncation, STOP and zero budget.
The schema accepts exact `ded9` and `ded9dfe0`, and requires the separate
no-stop/budget-one rejection case to contain a bad actual old ST(1) tag. Two
handwritten literal fixtures cover the FNSTSW AX observation sequence and
atomic invalid-rhs rejection. Native admission explicitly denies FCOMPP.

Validation passed: `cargo test --workspace --offline` (300 `od-isa` tests,
70 runner fixture integration tests, 156 `od-testkit` tests, 8 x86-runner
library tests and all remaining workspace suites), `cargo check --workspace
--all-targets --offline`, Linux x86_64 `cargo check -p od-x86-runner --target
x86_64-unknown-linux-gnu --all-targets --offline`, `cargo fmt --all -- --check`,
and `git diff --check`. The new literal CLI success/rejection test and the schema
rule tests passed. `python3 tools/check_layout.py` reports the pre-existing
unmapped root `.serena/` directory; it was left untouched. `graphify update .`
rebuilt the graph (3487 nodes, 7462 edges); its existing parser warning for
`include/open_desktop/host_abi.h` remains. Native deny vectors compile on this
arm64 macOS host but were not executed on Linux; no SSH or physical-hardware
checks were run. Independent review caught a provenance description mismatch in the
invalid-RHS fixture; its source text now describes the actual `DE D9` program.
The focused CLI fixture and schema tests passed again after that correction;
follow-up independent review confirmed the provenance fix with no findings.

Full x87 architectural behavior, special values, exception delivery, native
execution and FPU feature claims remain unverified; ISSUE-019 remains open.

Normative source: Intel SDM revision 093, Vol.2A,
“FCOM/FCOMP/FCOMPP—Compare Floating-Point Values,” pp.3-324–3-326
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

## S2-30ag — bounded FUCOM ST(0), ST(1) compare without pop

Added bare `DD E1` with the shared exact integer canonical-Normal comparison
gate for old physical TOP and TOP+1. Success replaces C0/C1/C2/C3 with greater
`0`, less `0x0100`, or equal `0x4000`; it clears C1 by the bounded profile's
explicit local policy, records FIP, advances RIP by two, and retires once.
TOP, all physical tags and all raw register payloads are preserved. The local
contract notes that Intel defines C1=0 for stack underflow but does not define
C1 for this normal case. FUCOM's unique QNaN semantics remain outside the
Normal-only domain.

Coverage includes every TOP × three relations × 16 initial condition fields,
signed operands, same-sign/different-exponent greater and less cases for both
positive and negative values, low-significand distinctions, independent
lhs/rhs invalid-tag and invalid-class rejection, invalid environment, and
fetch/truncation/STOP/budget atomicity. Exhaustive DD ModR/M coverage adds E1
and retains FST/FSTP neighbors. Schema and literal interpreter fixtures cover
`dde1`, `dde1dfe0`, and an actual invalid-RHS-tag no-stop/budget-one rejection.
Native admission explicitly denies FUCOM and DD E1. Full x87 architectural
behavior remains unverified.

Independent Sol-medium review confirmed the condition-code tuple and exponent
ordering coverage corrections; no remaining findings.

Normative source: Intel SDM revision 093, Vol.2A,
“FUCOM/FUCOMP/FUCOMPP—Unordered Compare Floating-Point Values,” pp.3-402–3-404
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

## S2-30ah — bounded FUCOMP ST(0), ST(1) compare and one-entry pop

Added exact bare `DD E9`, reusing `bounded_x87_fcom_result(state, 1)` for
preflight and canonical-Normal ordering. Success commits greater `0`, less
`0x0100`, or equal `0x4000`, clears C1 by the local deterministic policy,
empties only old physical TOP, advances TOP once with wrap, records FIP and
retires once. The raw physical register array and old ST(1) tag remain intact;
all other state is preserved.

Interpreter vectors cover every TOP × three relations × 16 incoming condition
fields, TOP wrap, payload sentinels, invalid tags/classes/environment and
fetch/truncation/STOP/budget atomicity. Schema and literal CLI fixtures cover
`dde9`, `dde9dfe0`, and an actual invalid-old-ST(1)-tag no-stop/budget-one
rejection. Exhaustive DD decode preserves FST/FSTP/FUCOM neighbors and leaves
DD E8, DD EA..EF and other unsupported forms rejected. Native admission
explicitly denies DD E9. FUCOMP QNaN semantics, exception delivery, stack
faults and native x87 execution remain outside the bounded Normal-only slice.

Validation passed: `cargo test --workspace --offline`,
`cargo fmt --all -- --check`, `cargo check --workspace --all-targets --offline`,
and compile-only `cargo check -p od-x86-runner --all-targets --target
x86_64-unknown-linux-gnu --offline`, plus `git diff --check` and graphify update.
`python3 tools/check_layout.py` did not pass because the pre-existing untracked
root directory `.serena/` is outside the allowed root-directory list; it was
left untouched. No physical x86 run, SSH, or Mac mini acceptance was performed.
Independent gpt-6.1-sol medium review completed with no findings.

Normative source: Intel SDM revision 093, Vol.2A,
“FUCOM/FUCOMP/FUCOMPP—Unordered Compare Floating-Point Values,” pp.3-402–3-404
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

## S2-30ai — bounded FUCOMPP compare and two-entry physical TOP pop

Added exact bare `DA E9` as a distinct `Fucompp` operation. It preflights the
existing x87 snapshot and both old physical operands with
`bounded_x87_fcom_result(state, 1)`, replaces the condition field with greater
`0`, less `0x0100`, or equal `0x4000`, clears C1 by local deterministic policy,
empties both old physical tags and advances TOP by two with wrap. Physical
payload bytes are untouched. FIP/RIP and retirement update once; invalid domain
and execution-boundary cases preserve full state. The DA decoder accepts only
ModR/M E9 without prefixes, and native admission explicitly denies the
operation.

Interpreter vectors cover all eight TOP values × three relations × 16 incoming
condition fields. They verify TOP=6/7 wrap, an unused Empty old ST(2) with an
arbitrary payload, independent invalid operand tags/classes, invalid
environment and fetch/truncation/STOP/budget rejection. Decoder tests enumerate
all 256 DA ModR/M values and prefixes. Schema and literal CLI fixtures cover
`dae9`, `dae9dfe0`, and an actual invalid old physical ST(1)-tag no-stop/budget-
one rejection. The equal-plus-FNSTSW fixture observes FSW and AX `0x5000`, both
old tags Empty, and two retired instructions.

Intel SDM revision 093 Vol.2A pp.3-402–3-404 says FUCOMPP pops both stack
entries in the explicit opcode table and prose; its p.3-403 pseudocode depicts
one pop. This slice follows the explicit table/prose. C1 clearing is local
policy. General FUCOMPP special-value, exception, stack-fault, `#NM` and native
semantics remain unverified.

Validation passed: full `cargo test --workspace --offline`,
`cargo check --workspace --all-targets --offline`, Linux x86_64 runner target
check, formatting, and `git diff --check`. `python3 tools/check_layout.py`
reports the pre-existing unmapped root `.serena/` directory; it was left
untouched. `graphify update .` rebuilt the graph (3518 nodes, 7523 edges); its
existing parser warning for `include/open_desktop/host_abi.h` remains. No SSH or
hardware acceptance was performed.

Normative source: Intel SDM revision 093, Vol.2A,
“FUCOM/FUCOMP/FUCOMPP—Unordered Compare Floating-Point Values,” pp.3-402–3-404
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

## S2-30aj — bounded FCOMI ST(0), ST(1) to RFLAGS

Added exactly bare `DB F1` as `Operation::Fcomi`. Before commit it requires an
existing x87 state, FCW `0x037f`, `FSW & 0x80ff == 0`, Valid tags at old
physical TOP and `(TOP+1)&7`, and canonical-Normal binary80 operands via
`bounded_x87_fcom_result(state, 1)`. Greater maps to cleared relation bits,
less to CF, and equal to ZF. CF/PF/AF/ZF/SF/OF are all defined and set from the
ordered result (AF/SF/OF/PF are zero for this bounded domain). Only FSW.C1 is
cleared; C0/C2/C3/TOP, tags, physical register payloads, FCW/FDP/FOP and all
unrelated state remain unchanged. FIP records instruction-start RIP; RIP
advances by two and the instruction retires once. Exceptions and all other
FCOMI-family forms remain unsupported.

Coverage includes every TOP × relation × 16 initial FSW condition field,
positive and negative ordering by sign/exponent/significand, all 64 initial
subsets of the six changed RFLAGS bits with definedness, bad tags and classes
on both operands, invalid environment, and fetch/truncation/STOP/budget
atomicity. The DB decoder enumerates all second bytes and rejects prefixes,
REX and WAIT. Literal schema/CLI fixtures use `dbf1` and `dbf1dfe0`; the
no-stop/budget-one rejection fixture has an Empty tag at actual old physical
ST(1). Native admission explicitly denies both `Operation::Fcomi` and DB F1.
Independent gpt-6.1-sol review found no remaining findings after correcting
two documentation wording issues. `graphify update .` completed with 3530
nodes and 7545 edges; its existing parser warning for
`include/open_desktop/host_abi.h` remains.

Validation passed: `cargo test --workspace --offline`,
`cargo check --workspace --all-targets --offline`, Linux x86_64 compile-only
`cargo check -p od-x86-runner --target x86_64-unknown-linux-gnu --all-targets
--offline`, `cargo fmt --all -- --check`, `git diff --check`, and focused FCOMI
interpreter/schema/runner tests. The full workspace suite also passed the
explicit native admission denial regression. `python3 tools/check_layout.py`
reports the pre-existing unmapped root `.serena/` directory; it was present
before this task and remains untouched. Linux native x87 execution, physical
x86 execution, SSH and Mac mini acceptance were not performed.

Normative source: Intel SDM revision 093, Vol.2A,
“FCOMI/FCOMIP/FUCOMI/FUCOMIP—Compare Floating-Point Values,” pp.3-327–3-329
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

## S2-31b — bounded `FISTP m32int [RAX]` partial slice

Added only bare `DB 18` as a fixed Dword store through RAX in Long64. The
interpreter preflights existing x87 state, all-masked PC64 FCW, the bounded
FSW mask, old physical TOP Valid tag, canonical-Normal binary80 input, and
unbiased exponent 0..31. It rounds with integer quotient/remainder arithmetic
under the four admitted RC values, rejects signed-i32 overflow before data
access, and encodes negative results with unsigned two's-complement arithmetic.

After the conversion succeeds, the executor validates the entire canonical48
four-byte range, then issues one `write_integer` Dword access with no data read.
The CPU remains unchanged until that write succeeds. Store errors retain their
existing types; an injected architectural #PF remains the memory backend's
reported error, and flat mapping/permission failures are not converted into
guest faults. Successful commit empties old physical TOP, advances TOP, keeps
PE sticky and sets it for a remainder, updates C1 from magnitude increment,
records FIP/FDP, and advances RIP. All x87 raw payload bytes survive.

Four literal fixtures cover `db18`, `db18dfe0`, invalid-TOP rejection, and
signed-i32 overflow rejection without data access. Interpreter tests cover the
unaligned exact four-byte store and canaries, no data read, one write, overflow
before access, permission failure atomicity, canonical-range rejection, the
negative i32 minimum, and an injected #PF with unchanged CPU and RAM. The
literal runner test checks split-region output bytes, FSW/TOP/tag/payload/FIP/FDP
state, and no-commit snapshots for invalid tag and overflow. Decoder
enumeration, schema validation, and explicit native denial are included.
The follow-up literal-oracle test runs exact and inexact positive/negative
values through all four RC modes, then checks raw `+2147483647.5` and
`-2147483648.5` binary80 inputs against hand-stated signed-boundary results.
Each success observes FSW in AX through FNSTSW and checks memory bytes; each
rounding-induced overflow verifies zero retirement and no store.

Validation passed: `cargo test --workspace --offline`,
`cargo check --workspace --all-targets --offline`, Linux x86_64 compile-only
`cargo check -p od-x86-runner --target x86_64-unknown-linux-gnu --all-targets
--offline`, `cargo fmt --all -- --check`, and `git diff --check`. The native
admission regression is Linux-only and was compile-checked, not executed on
this macOS host. `python3 tools/check_layout.py` reports the pre-existing
unmapped root `.serena/` directory; it was left untouched. `graphify update .`
rebuilt the graph to 3614 nodes, 7710 edges and 211 communities; the existing
parser warning for `include/open_desktop/host_abi.h` remains. A Sol follow-up
review of the literal RC/boundary coverage and report found no actionable
issues. Physical x86 execution and MMU/page-walk behavior were not tested.
This partial slice does not complete S2-31 or general x87 exception semantics;
final Mac mini acceptance remains deferred to the last S2-31 slice.

Normative source: Intel SDM revision 093, Vol.2A, “FIST/FISTP—Store Integer”
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

## S2-31c — bounded `FIST m32int [RAX]` partial slice

Added only bare `DB 10` as a fixed Dword store through RAX in Long64. The
bounded preflight and integer quotient/remainder rounding reuse the FISTP
conversion domain: canonical-Normal old physical ST(0), unbiased exponent
0..31, one of the four all-masked PC64 control words, `FSW & 0x80df == 0`, and
a post-rounded signed-i32 result. Overflow and unsupported state reject before
data access.

The executor validates the complete canonical48 Dword destination and issues
one atomic `write_integer` call without a data read. CPU state remains unchanged
until the store succeeds. Successful commit updates sticky PE/C1, FIP/FDP,
RIP and retirement while preserving TOP, all physical tags, every raw x87
payload and unrelated state. Store errors retain existing types and do not
commit CPU/x87 state. Native x87 admission explicitly denies `FIST`.

Literal fixtures cover exact `db10` and `db10dfe0` (FNSTSW AX) programs with an
unaligned Dword store split across adjacent memory regions, plus Empty-TOP and
+2^32 overflow rejections. Interpreter checks cover all four RC modes on
positive/negative half-way values, exact signed-i32 endpoints, PE/C1 behavior,
TOP/tag/payload preservation, overflow before access, canonical-range and
permission-fault atomicity, the FIST→FISTP sequence, and the
single-write/no-read contract. Decoder enumeration confirms only ModR/M 10 is
added for DB and prefixes remain rejected. `cargo test -p od-isa --offline`,
the literal runner schema/CLI test, workspace all-target check, formatting and
diff checks passed. A Linux-only native admission regression was added but not
run on this macOS host; physical x86 execution was not run.
`graphify update .` refreshed the graph to 3624 nodes, 7728 edges and 205
communities; its existing warning for `include/open_desktop/host_abi.h` remains.

Flat fixture RAM does not establish MMU/page-walk behavior, MMIO, SMP, physical
x86 execution or architectural exception delivery. This partial slice does
not complete S2-31.

Normative source: Intel SDM revision 093, Vol.2A, “FIST/FISTP—Store Integer”
([official Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

## S2-31d — bounded `FISTP m16int [RAX]` partial slice

Added only bare `DF 18` as a fixed Word store through RAX in Long64. The
executor reuses S2-31b/c's integer quotient/remainder RC conversion, with a
canonical-Normal old physical TOP, exponent 0..15, exact all-masked PC64 FCW
values, `FSW & 0x80df == 0`, and post-rounded signed-i16 bounds. The full
canonical48 two-byte range is checked before one atomic Word write with no data
read. Success empties old TOP and commits PE/C1, FIP/FDP, RIP and retirement
only after the store; raw payloads and unrelated state remain unchanged.

Literal fixtures cover all four RC modes with positive and negative ties,
exact/rounded signed-word limits, overflow, invalid tag/status/control word and
out-of-domain exponent, plus exact `DF 18 DF E0` status observation. The CLI
checks written bytes, status/pop state and complete payload preservation, and
checks zero-retirement rejections preserve CPU and memory. Interpreter tests
also cover exact one-Word-write/no-read tracing, all RC/sign results, write
permission failure, injected #PF, canonical-range rejection and failure
atomicity. Exhaustive DF ModR/M tests and prefix rejection include `DF 18`;
native admission remains fail-closed.

Verification passed: `cargo test --workspace --offline`,
`cargo check --workspace --all-targets --offline`, `cargo fmt --all -- --check`,
and `git diff --check`. The CLI fixture test covers all 18 literal inputs. The
native admission code explicitly denies DF 18, but its platform-gated test did
not execute on this macOS host. `graphify update .` rebuilt the graph to 3633
nodes, 7745 edges and 177 communities; the existing parser warning for
`include/open_desktop/host_abi.h` remains. No physical x86, MMU/page-walk,
MMIO, SMP or general x87 exception behavior was tested. This does not complete
S2-31.

Normative source: Intel SDM revision 093, Vol.2A, “FIST/FISTP—Store Integer,”
pp.3-347–3-349 ([official Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

S2-31ck later supersedes this initial finite-overflow rejection: rounded overflow
now stores integer-indefinite and adds IE after a successful DF 18/DF 10 store.
The former overflow and exponent-16 fixtures now execute as successful cases.


## S2-31e — bounded `FIST m16int [RAX]` partial slice

Added bare `DF 10` as a fixed Word store through RAX in Long64. The decoder
uses the existing integer quotient/remainder conversion with the S2-31d
canonical-Normal source, exponent 0..15, exact PC64/all-masked FCW values,
`FSW & 0x80df == 0`, and post-rounded signed-i16 bounds. The full canonical48
two-byte destination is checked before one atomic Word store with no data
read. Only a successful store commits PE/C1, FIP/FDP, RIP and retirement. The
non-pop operation preserves TOP, every physical tag, all raw x87 payloads and
unrelated state.

Literal fixtures admit exact `DF 10` and `DF 10 DF E0` (FNSTSW AX), and cover
no-pop behavior, a tie, exact signed-word limits, overflow and invalid
tag/status/control/exponent state. Interpreter checks additionally exercise
all RC modes and signs, rounded boundaries, one-write/no-read behavior,
permission and injected #PF failure atomicity, and canonical-range rejection.
Native admission explicitly denies DF 10.

Verification passed: `cargo test --workspace --offline`,
`cargo check --workspace --all-targets --offline`, `cargo fmt --all -- --check`,
and `git diff --check`. The local CLI fixture test covers nine literal S2-31e
cases; the full workspace test includes the new decoder, conversion, store
atomicity and schema/fixture checks. Native admission explicitly denies DF 10,
but platform-gated native execution was not run. No Mac mini or physical x86
tests were run. MMU/page-walk, MMIO, SMP and general x87 exception behavior
remain outside this local slice.

This bounded local slice does not establish physical x86 behavior, MMU/page
walks, MMIO, SMP, architectural exception delivery, unmasked x87 exceptions,
or general x87 semantics. It does not complete S2-31.

Normative source: Intel SDM revision 093, Vol.2A, “FIST/FISTP—Store Integer,”
pp.3-347–3-349 ([official Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

S2-31ck later extends this initial finite-overflow rejection: rounded overflow
now stores integer-indefinite and adds IE after a successful DF 10/DF 18 store.
The former overflow and exponent-16 fixtures now execute as successful cases.


## S2-31f — bounded `FISTP m64int [RAX]` partial slice

Added only bare `DF 38` as a fixed Qword store through RAX in Long64. The
shared quotient/remainder conversion now supports signed i16, i32 and i64
outputs. For `DF 38`, preflight accepts canonical-Normal old physical TOP,
unbiased exponent 0..63, exact all-masked PC64 FCW values, and
`FSW & 0x80df == 0`; exponent 63 takes the zero-shift, denominator-one exact
path. Rounded magnitude is checked in `u128` against asymmetric signed-i64
limits before narrowing, then represented by unsigned two's-complement Qword
encoding. Unsupported inputs and rounded overflow return Unsupported before
memory access.

Execution checks the complete canonical48 Qword range and issues one atomic
Qword `write_integer` without a data read. Only a successful store empties old
physical TOP, advances TOP once with wrap, updates sticky PE/C1, records
FIP/FDP, advances RIP and retires. Raw x87 payloads and all unrelated state
remain preserved. Literal fixture and interpreter coverage includes the exact
signed-i64 boundaries, positive overflow, `2^63 - 0.5` under all four RC
modes, all RCs for negative `2^63`, all four sign/tie RC combinations,
exponent 64 rejection, canaries, canonical-range rejection and write
permission/#PF atomicity. The schema and decoder admit only exact `DF 38`
(plus `DF 38 DF E0` status observation); prefixes and neighboring encodings
remain unsupported. Native x87 admission explicitly denies this operation.

Verification passed: `cargo test -p od-isa fistp_m64int --offline`,
`cargo test -p od-testkit --offline`, the focused runner literal-fixture test,
the full workspace suite, workspace all-target check, formatting, diff checks
and `graphify update .` (3651 nodes, 7780 edges, 213 communities; the existing
parser warning for `include/open_desktop/host_abi.h` remains).
`python3 tools/check_layout.py` reports the pre-existing unmapped `.serena/`
directory, which was left untouched. This host did not run physical x86 or Mac
mini checks. Flat fixture RAM does not establish MMU/page
walks, MMIO, SMP or architectural exception delivery. In this bounded profile
overflow is Unsupported rather than the architectural indefinite integer
result, and x87 exception delivery remains unmodeled. This does not complete
S2-31.

Normative source: Intel SDM revision 093, Vol.2A, “FIST/FISTP—Store Integer,”
pp.3-347–3-349 ([official Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

## S2-30ak — bounded FCOMIP ST(0), ST(1) to RFLAGS and pop

Added exactly bare `DF F1` as `Operation::Fcomip`. DF decoding now admits
only existing `DF E0` FNSTSW AX and FCOMIP `DF F1`; every other ModR/M byte,
prefix, REX and WAIT form remains rejected. Before commit, FCOMIP requires
existing x87 state, FCW `0x037f`, `FSW & 0x80ff == 0`, Valid tags at old
physical TOP and `(TOP+1)&7`, and canonical-Normal operands from
`bounded_x87_fcom_result(state, 1)`. Greater sets relation flags to zero, less
sets CF, and equal sets ZF; all six affected RFLAGS bits (`0x8d5`) are defined.
It clears only FSW.C1, empties the tag at old physical TOP, advances TOP once
with wrap, and preserves old physical ST(1)'s Valid tag, all register
payloads, C0/C2/C3, FCW/FDP/FOP and unrelated state. FIP records instruction
start, RIP advances two bytes, and retirement occurs once. Exceptions and
non-Normal classes remain unsupported and atomic.

Coverage spans all eight TOP values × all three ordered relations × 16 initial
FSW condition fields, all 64 affected RFLAGS masks with definedness, used-tag
and raw-class/environment rejection, missing x87 state, and fetch/truncation/
STOP/budget atomicity. DF decoder tests enumerate all 256 second bytes and
reject every prefix. Literal schema/CLI fixtures cover exact `dff1dfe0` with
FNSTSW result AX/FSW `0x4d00` and tag word `0xfff3`, plus no-stop/budget-one
`dff1` rejection on Empty actual old physical ST(1). Native admission
explicitly denies `Operation::Fcomip` and DF F1.

Validation passed: `cargo test --workspace --offline` (all workspace tests),
`cargo check --workspace --all-targets --offline`, Linux x86_64 compile-only
`cargo check -p od-x86-runner --target x86_64-unknown-linux-gnu --all-targets
--offline`, `cargo fmt --all -- --check`, and `git diff --check`. Focused
FCOMIP interpreter and literal runner tests passed. Native admission explicitly
denies `Operation::Fcomip` and DF F1 in code; the Linux-only native admission
regression was not executed on this macOS host. `python3 tools/check_layout.py`
still reports the pre-existing unmapped root `.serena/` directory, which was
present before this task and left untouched. `graphify update .` rebuilt the
graph with 3542 nodes and 7569 edges; the existing parser warning for
`include/open_desktop/host_abi.h` remains. Linux x86_64 compile-only check
passed; Linux native x87 execution, the Linux-only admission test and
physical-x86 acceptance were not performed.

Normative source: Intel SDM revision 093, Vol.2A,
“FCOMI/FCOMIP/FUCOMI/FUCOMIP—Compare Floating-Point Values,” pp.3-327–3-329
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

## S2-30al — bounded FUCOMI ST(0), ST(1) to RFLAGS

Added exact bare `DB E9` as separate `Operation::Fucomi`. Before mutation it
calls `bounded_x87_fcom_result(state, 1)`, requiring an existing x87 snapshot,
FCW `0x037f`, `FSW & 0x80ff == 0`, Valid tags at old physical TOP and TOP+1,
and canonical-Normal binary80 operands. Ordered greater maps the `0x8d5`
RFLAGS mask to zero, less to CF, and equal to ZF; decoded `FlagEffects` defines
all six affected bits. It clears only FSW.C1, records FIP, advances RIP by two
and retires once. It preserves C0/C2/C3, TOP, tags, payloads, the rest of the
x87 environment, GPRs and memory. It does not pop or process unordered
QNaN/special values, deliver x87 exceptions, or enter native x87 execution.

DB now admits only existing E2 FNCLEX, E3 FNINIT, F1 FCOMI and E9 FUCOMI.
Exhaustive second-byte and prefix tests reject every other DB form; REX, WAIT,
memory forms and all prefixes remain unsupported. Interpreter tests cover all
TOP × three ordered relations × 16 initial FSW condition fields, all 64
affected RFLAGS masks with definedness, both operands' invalid tags and raw
classes, bad FCW/FSW, missing state, and fetch/truncation/STOP/budget
atomicity. Literal schema/CLI fixtures use exact `dbe9dfe0` and `dbe9`, with
the latter carrying an Empty actual old physical ST(1) tag and budget one/no
stop; the S2-30an extension now admits `dbeadfe0` as FUCOMI ST(2); the exact indexed
classifier and schema coverage are documented below. Native admission explicitly
denies `Operation::Fucomi` and DB E9.

Validation passed: `cargo test --workspace --offline`,
`cargo check --workspace --all-targets --offline`, Linux x86_64 compile-only
`cargo check -p od-x86-runner --target x86_64-unknown-linux-gnu --all-targets
--offline`, `cargo fmt --all -- --check`, and `git diff --check`.
`python3 tools/check_layout.py` reports the existing unmapped root `.serena/`
directory; it was present before this task and remains untouched. The native
admission regression is compile-covered but Linux-only and was not executed on
this macOS host. Physical x86/native x87 acceptance was not performed.
Independent Sol-medium review found no implementation defects; it identified a
documentation ambiguity about neighboring DF forms, which was corrected and
re-reviewed clean. `graphify update .` completed with 3555 nodes, 7592 edges
and 183 communities; its existing parser warning for
`include/open_desktop/host_abi.h` remains.

Normative source: Intel SDM revision 093, Vol.2A,
“FCOMI/FCOMIP/FUCOMI/FUCOMIP—Compare Floating-Point Values,” pp.3-327–3-329
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

## S2-30am — bounded FUCOMIP ST(0), ST(1) to RFLAGS and pop

Added exact bare `DF E9` as a distinct `Operation::Fucomip`. The decoder keeps
DF limited to E0 FNSTSW AX, E9 FUCOMIP and F1 FCOMIP, and leaves the DB allowlist
E2 FNCLEX, E3 FNINIT, E9 FUCOMI and F1 FCOMI. Prefixes, REX, WAIT, memory and all
other DB/DF bytes remain unsupported. Execution preflights the existing
`bounded_x87_fcom_result(state, 1)` before mutation: x87 state must exist, FCW
must be `0x037f`, `FSW & 0x80ff` zero, both actual physical operand tags Valid,
and both binary80 values canonical Normal. Commit maps greater/less/equal to
cleared relation flags/CF/ZF, defines all six bits in `0x8d5`, clears only C1,
empties old physical TOP and increments TOP with wrap. It preserves all raw
payloads, the old ST(1) Valid tag, remaining x87 state, GPRs and memory, records
FIP, advances RIP by two and retires once. Unordered/special values, exception
delivery, `#NM` and native x87 execution remain excluded.

Interpreter tests cover TOP × three ordered relations × 16 initial FSW
condition fields, all 64 initial affected-RFLAGS masks with definedness, bad
operand tags/classes, FCW/FSW, missing state, and fetch/truncation/STOP/budget
atomicity. Literal fixtures use exact `dfe9dfe0` and `dfe9`; the latter sets
TOP=5 and the actual physical RHS (slot 6) tag Empty with no stop RIP and budget
one. Schema/CLI tests reject `dff8dfe0` (DF F8 remains outside the indexed group) and `66`/`48`/`9b` prefixes. Native
admission explicitly rejects `Operation::Fucomip`; the Linux-only native
admission regression was compile-checked but not executed on this macOS host.

Validation passed: `cargo test --workspace --offline`, `cargo check --workspace
--all-targets --offline`, Linux x86_64 compile-only
`cargo check -p od-x86-runner --target x86_64-unknown-linux-gnu --all-targets
--offline`, `cargo fmt --all -- --check`, and `git diff --check`. Focused
FUCOMIP tests passed (4 interpreter/decode tests and the literal CLI fixture
test). `python3 tools/check_layout.py` reports the pre-existing unmapped root
`.serena/` directory; it was present before this task and remains untouched.
`graphify update .` completed with 3567 nodes, 7616 edges and 177 communities;
the existing extractor warning for `include/open_desktop/host_abi.h` remains.
Physical x86 execution and native x87 state comparison were not performed.

Normative source: Intel SDM revision 093, Vol.2A,
“FCOMI/FCOMIP/FUCOMI/FUCOMIP—Compare Floating-Point Values,” pp.3-327–3-329
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

## S2-30an — indexed FCOMI-family register forms

Generalized the four existing `i=1` instructions to all `i=0..7` encodings:
`DB E8+i` FUCOMI, `DB F0+i` FCOMI, `DF E8+i` FUCOMIP, and `DF F0+i` FCOMIP.
The exact-byte decoder carries the low ModR/M index in `source:u8`; DB E2/E3
and DF E0 retain their previous operations. Every other DB/DF byte, all legacy
and REX prefixes, WAIT, and memory forms remain rejected. Native execution is
explicitly closed for all four indexed operation variants and all 32 encodings.

Execution reuses `bounded_x87_fcom_result(state, i)`. It validates the x87
snapshot, FCW/FSW, old physical TOP tag and canonical-Normal value. For nonzero
i it also validates the actual physical `(TOP+i)&7` source; for `i=0` it returns
Equal after validating old TOP without reading ST(1). The commit clears RFLAGS
mask `0x8d5`, writes greater/less/equal as zero/CF/ZF, defines all six affected
flags, and clears only FSW.C1. Nonpop forms preserve all tags and TOP. Pop
forms empty old TOP and advance once with wrap, including self compare. Raw
register payloads are preserved; FIP/RIP and retirement update normally.
Rejects remain atomic.

The x87 schema admits exactly 32 comparison instructions, alone or followed
by DF E0 FNSTSW AX: 64 exact strings. It rejects near-miss bytes, prefixes,
memory forms, and excess trailing instructions. Its no-stop/budget-one reject
profile is restricted to a bad actual source with valid old TOP and environment;
i=0 uses the exact all-zero payload class reject. The six new literal CLI
fixtures cover FCOMI self compare at TOP=3 with only TOP Valid; FUCOMIP self
compare/pop at TOP=7 with wrap and only TOP Valid; FUCOMI ST(7) at TOP=7 using
physical slot 6 with old ST(1) Empty; FCOMIP ST(6) at TOP=6 using physical
slot 4 while popping only old TOP; and standalone invalid-source cases for i=0
and i=7 with unrelated ST(1) Valid. Existing i=1 fixtures are unchanged.

Interpreter tests cover all TOP × source × three ordered relations × 16 initial
FSW condition fields and all 64 initial affected-RFLAGS masks for each family
and source. Additional vectors cover every invalid actual source, environment
rejection, source physical placement, self aliases, Empty unrelated tags, pop
wrap, payload/tag preservation, decode bytes/prefixes, and execution boundaries.
Schema tests exhaust all 64 exact program strings and near misses. Runner CLI
checks all six new fixtures.

Validation passed: `cargo test --workspace --offline`,
`cargo check --workspace --all-targets --offline`, Linux x86_64 compile-only
`cargo check -p od-x86-runner --target x86_64-unknown-linux-gnu --all-targets
--offline`, `cargo fmt --all -- --check`, and `git diff --check`. The Linux-only
native admission regression was compile-checked but could not be executed on
this macOS host. `python3 tools/check_layout.py` reports only the pre-existing
unmapped root `.serena/` directory, which was left untouched. `graphify update .`
completed with 3586 nodes, 7653 edges and 174 communities; its existing
extractor warning for `include/open_desktop/host_abi.h` remains. Physical x86
execution, native x87 state comparison, special values, and exception delivery
were not performed.

Normative source: Intel SDM revision 093, Vol.2A,
“FCOMI/FCOMIP/FUCOMI/FUCOMIP—Compare Floating-Point Values,” pp.3-327–3-329
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf)).

## S2-31g — bounded `FNSTCW m2byte [RAX]` partial slice

Added exact bare `D9 38` in Long64 with an existing x87 snapshot. It stores the
raw FCW as one little-endian Word write to `[RAX]`, with a full canonical48
range check and no data read. It accepts any FCW and schema-valid FSW, including
pending/status bits and an empty stack, without inspecting ST(0), tags or TOP.
After the write succeeds, only RIP advances by two and the instruction retires
once; the complete FPU snapshot, GPRs and RFLAGS remain unchanged. Write errors
propagate unchanged and preserve CPU state. Prefixes and neighboring D9 forms
remain unsupported, and native admission explicitly denies FNSTCW.
The no-wait instruction does not check pending x87 exceptions; the waiting
FSTCW `#MF` case is not applied here. Undefined C0-C3 bits are preserved
deterministically by local policy without claiming native equality.

Literal schema fixtures cover the four standard FCWs, arbitrary `0xa55a` FCW
with `FSW=0xffff` and Empty tags, payload and pointer sentinels, unaligned Word
canaries, exact `D9 38 DF E0` observation, and a canonical-boundary crossing
reject. Interpreter tests cover second-byte write failure, canonical
boundaries, no data reads, full snapshot preservation, and fetch/truncation,
STOP and budget boundaries. The existing x87 schema admits only 16-bit FCW/FSW
values; arbitrary values within that schema are accepted by this operation.

Validation passed: `cargo test --workspace --offline`,
`cargo check --workspace --all-targets --offline`, `cargo fmt --all -- --check`,
and `git diff --check`. The graph was refreshed with `graphify update .`.
`python3 tools/check_layout.py` reports the pre-existing unmapped root
directory `.serena`; that directory was left untouched. No physical x86 or Mac
mini checks were run. This does not complete S2-31 or establish general x87
exception behavior, MMU translation, MMIO or SMP.

Normative source: Intel SDM revision 093, Vol.2A, “FSTCW/FNSTCW—Store x87 FPU
Control Word,” pp.3-388–3-389 (`D9 /7`); Vol.1 §§8.1.8, 8.1.9 and §8.6 for
x87 state and FIP/FDP context ([official Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=506),
[official Vol.1 PDF](https://cdrdv2-public.intel.com/929351/253665-093-sdm-vol-1.pdf)).

## S2-31h — bounded `FNSTSW m2byte [RAX]` partial slice

Added exact bare `DD 38` in Long64 with an existing x87 snapshot. The
interpreter stores the raw pre-operation FSW through one atomic little-endian
Word write to canonical `[RAX, RAX+1]`; it issues no data read and imposes no
FCW/FSW or stack/tag/TOP gate. A successful store changes only RIP by two and
retires once, preserving GPRs, RFLAGS and every x87 field. Local policy also
preserves C0-C3 without claiming native equivalence. Missing state, canonical
range failures, store errors, and fetch/stop/budget boundaries leave CPU state
uncommitted; memory errors propagate. Exact `DD 38` and `DD 38 DF E0` schema
coverage is present, and native admission explicitly denies FNSTSW m2byte.

Literal fixtures cover FSW `0000`, mixed `45a5`, `ffff`, arbitrary FCW/status,
Empty tags, pointer and payload sentinels, unaligned canaries, FNSTSW AX
composition and canonical48 crossing. Interpreter vectors additionally check
single Word/no-read access, missing state, write permission/backend failure,
canonical lower and upper boundaries, fetch/truncation, STOP and budget
atomicity. Waiting `9B DD 38`, `#MF`, `#NM`/CR0, MMU translation, MMIO, SMP and
physical x86 behavior remain outside the slice; this does not complete S2-31.

Validation passed: `cargo test --workspace --offline`,
`cargo check --workspace --all-targets --offline`, `cargo fmt --all -- --check`,
and `git diff --check`. `graphify update .` refreshed the graph. The native
runner's Linux x86_64-only unit tests and physical x86/Mac mini checks were not
run on this host; native execution remains explicitly denied by the code path.

Normative source: Intel SDM revision 093, Vol.2A,
“FSTSW/FNSTSW—Store x87 FPU Status Word,” pp.3-392–3-393 (`DD /7`); Vol.1
§§8.1.8, 8.1.9 and 8.3.12 ([official Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=510),
[official Vol.1 PDF](https://cdrdv2-public.intel.com/929351/253665-093-sdm-vol-1.pdf)).

## S2-31i — bounded `FLDCW m2byte [RAX]` partial slice

Added exact bare `D9 28` in Long64 with an existing x87 snapshot. The old
environment gate runs before address or data access: FCW must be one of
`037f/077f/0b7f/0f7f` and `FSW & 0x80df` must be zero. The implementation
validates canonical48 for the complete Word range, makes one GuestMemory Word
read, then accepts only those same four loaded FCWs. Invalid old state causes
zero data accesses; invalid loaded state causes one read and no state commit.
Success changes only FCW and RIP and retires once, preserving FSW, tags, TOP,
all payloads, pointers, GPRs and RFLAGS. C0-C3 preservation is local policy.

Interpreter tests cover all 16 old/new pairs, all four RC modes via FRNDINT,
invalid/missing state, a second-byte read fault, canonical crossing, fetch,
truncation, STOP and budget boundaries. The literal CLI fixture composes
`D9 28 D9 FC DF E0` and observes the rounding result and status through AX.
The schema rejects prefixes, WAIT and neighbor forms. Native admission remains
closed. Pending/unmasked exception delivery, `#MF`, `#NM`/CR0, MMU, MMIO and
physical execution remain unverified; this does not complete S2-31.

Validation passed: `cargo test --workspace --offline` (all 639 unit, fixture,
runner and doctest cases); `cargo check --workspace --all-targets --offline`,
`cargo fmt --all -- --check`, and `git diff --check`. `python3 tools/check_layout.py`
was run but reports the existing untracked `.serena/` root directory as
unmapped; that user data was left untouched. `graphify update .` refreshed the
code graph. Physical x86/Mac mini checks were not run on this host; native x87
admission remains explicitly denied.

Normative source: Intel SDM revision 093, Vol.2A,
“FLDCW—Load x87 FPU Control Word,” pp.3-356–3-357 (`D9 /5`); Vol.1 §§8.1.8,
8.1.9 and §8.6 ([official Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=474),
[official Vol.1 PDF](https://cdrdv2-public.intel.com/929351/253665-093-sdm-vol-1.pdf)).

## S2-31j — bounded `FSTP m32fp` partial slice

Added exact bare `D9 18` as `FSTP m32fp [RAX]` in Long64. The implementation
preflights only the Valid old physical TOP and canonical-Normal raw binary80
payload, accepts unbiased exponents -126..126, the four exact all-masked PC64
FCWs and `FSW & 0x80df == 0`, then rounds entirely with integer significand,
remainder and the shared quotient-rounding helper. It directly encodes finite
normal binary32 bits; input exponents -127/127 and a carry from exponent 126
to 127 reject before data access. No host floating-point conversion is used.

On success, the executor validates the complete canonical48 Dword destination,
performs one atomic Dword write with no data read, then empties old TOP, advances
TOP, sets sticky PE for an inexact result and sets C1 iff magnitude rounded up.
It records old RIP in FIP and the destination in FDP, advances RIP by two and
retires once. Raw payloads, other tags, FCW/FOP and unrelated state are
preserved; C0/C2/C3 follow the local retain policy. The schema accepts exact
`D9 18` and exact `D9 18 DF E0`; fixtures observe `+1.5` stored as
`0x3fc00000` and the post-pop FSW through AX.

Independent literal interpreter tests cover all TOPs, even/odd ties, positive
and negative directed modes, exact and inexact PE/C1 state, minimum/maximum
normal exponents, exponent/carry rejections, canaries, one-write/no-read,
canonical crossing, store failure, invalid source/environment, missing state,
fetch truncation, STOP and budget boundaries. The runner CLI fixture test also
checks the exact two byte strings, payload/tag state and FIP/FDP composition.
Native x87 admission remains closed.

Validation passed: `cargo test --workspace --offline`, `cargo check --workspace
--all-targets --offline`, `cargo fmt --all -- --check`, and `git diff --check`.
`graphify update .` refreshed the code graph. `python3 tools/check_layout.py`
still reports the pre-existing unmapped root `.serena/` directory, which was
present before this task and left untouched. Physical x86 and Mac mini checks
were not run; they remain for the final combined stage. This local slice does
not establish hardware exception priority, general x87 exception delivery,
MMU/page-walk behavior, MMIO or SMP, and does not complete S2-31.

Normative source: Intel SDM revision 093, Vol.2A, “FST/FSTP—Store Floating
Point Value,” pp.3-386–3-387 (`D9 /2`/`D9 /3`); Vol.1 §§8.1.8, 8.1.9 and §8.6
([official Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=504),
[official Vol.1 PDF](https://cdrdv2-public.intel.com/929351/253665-093-sdm-vol-1.pdf)).

## S2-31k — bounded `FST m32fp` partial slice

Added exact bare `D9 10` as `FST m32fp [RAX]` in Long64. It reuses S2-31j's
integer-only binary80-to-binary32 conversion, source/environment gates and
canonical48 Dword preflight. Only finite normal binary32 results are admitted;
out-of-range exponents and a carry outside the local exponent range reject
before data access.

Success performs one atomic Dword write with no data read, then updates PE/C1,
FIP/FDP and RIP. TOP, every tag and all 80-bit payloads remain unchanged. C0,
C2 and C3 are retained by local deterministic policy; C1 is set when rounding
increases magnitude and cleared for exact results by local policy. Backend
store diagnostics are preserved, and architectural write faults are reported
as guest faults.

The schema admits exact `D9 10` and `D9 10 DF E0`. CLI fixtures verify the
stored `0x3fc00000`, full no-pop state preservation, FIP/FDP and FNSTSW AX
composition. Focused interpreter tests cover all TOP values and tag/payload
sentinels, tie and sign-sensitive rounding modes, exponent endpoints/carry,
sticky PE/C1, canaries, one-write/no-read behavior, canonical range, denied
backend writes and architectural `#PF` atomicity. Decoder/schema checks cover
prefixes, WAIT, neighboring encodings and truncation. Native x87 admission
explicitly denies FST.

Validation passed: `cargo test --workspace --offline`, `cargo check --workspace
--all-targets --offline`, `cargo fmt --all -- --check`, and `git diff --check`.
`graphify update .` refreshed the code graph. `python3 tools/check_layout.py`
still reports the pre-existing unmapped root `.serena/` directory, which was
left untouched. Physical x86 and Mac mini checks were not run; they remain for
the final combined stage. This local slice does not establish hardware
exception priority, general x87 exception delivery, MMU/page-walk behavior,
MMIO or SMP, and does not complete S2-31.

Normative source: Intel SDM revision 093, Vol.2A, “FST/FSTP—Store Floating
Point Value,” pp.3-386–3-387 (`D9 /2`/`D9 /3`); Vol.1 §§8.1.8, 8.1.9 and §8.6
([official Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=504),
[official Vol.1 PDF](https://cdrdv2-public.intel.com/929351/253665-093-sdm-vol-1.pdf)).

## S2-31l — bounded `FSTP m64fp` partial slice

Added exact bare `DD 18` as `FSTP m64fp [RAX]` in Long64. The conversion
preflights a Valid old physical TOP with a canonical-Normal binary80 source,
the four exact all-masked PC64 FCWs, and `FSW & 0x80df == 0`. It computes
`q=s>>11`, `r=s&0x7ff`, `d=0x800` and reuses the integer quotient-rounding
helper. Binary64 bits are packed directly; the local unbiased exponent range
is -1022..1022, intentionally excluding exponent 1023. No host floating-point
cast is used.

The executor validates the complete canonical48 Qword range, performs one
atomic Qword write with no data read, then empties old TOP, advances TOP,
updates sticky PE/C1, records FIP/FDP, advances RIP by two and retires once.
Architectural store faults map through the guest-fault reporting helper;
backend failures remain diagnostic errors. Other payloads/tags, FCW/FOP and
unrelated state are preserved, with C0/C2/C3 retained by local policy.

Two handwritten schema-v2 fixtures cover exact `DD 18` and `DD 18 DF E0`
composition. Independent interpreter literals cover signs and RC, exact and
tie boundaries, discarded bits, exponent endpoints/carry, all TOP values
including wrap, PE/C1, memory canaries, one-write/no-read, noncanonical and
crossing ranges, backend write failure, injected architectural `#PF`, invalid
environment/source, and fetch/truncation/STOP/budget boundaries. Exhaustive
DD decoder tests preserve existing register/status operations and reject
neighbors, tested prefixes, REX and truncation. Native admission explicitly
denies `FSTP m64fp`.

Validation passed: `cargo test --workspace --offline`, `cargo check --workspace
--all-targets --offline`, `cargo fmt --all -- --check`, and `git diff --check`.
`graphify update .` refreshed the graph. `python3 tools/check_layout.py` still
reports the pre-existing unmapped root `.serena/` directory, which was present
before this task and left untouched. Independent review is pending. Physical
x86/Mac mini checks were not run and remain part of the single final S2-31
acceptance substage. This partial slice does not establish hardware exception
priority, general x87 exception delivery, MMU/page-walk behavior, MMIO or SMP,
and does not complete S2-31.

Normative source: Intel SDM revision 093, Vol.2A, “FST/FSTP—Store Floating
Point Value,” pp.3-386–3-387 (`DD /2`/`DD /3`)
([official PDF, Vol.2A p.3-386](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=504));
Vol.1 §§8.1.8, 8.1.9 and §8.6 for x87 state and FIP/FDP.

## S2-31s — bounded `FLD m32fp` partial slice

Added exact bare `D9 00` for Long64 `FLD m32fp [RAX]`, preserving all existing
D9 forms. The operation uses the established FCW/FSW/Empty-destination gates,
preflights the complete canonical48 Dword range, and makes one exact Dword
read. Only binary32 Normal exponent fields 1..254 are accepted; integer field
expansion preserves sign and maps directly to canonical binary80, including
the full source exponent range. Zero, subnormal, infinity and NaN reject after
the single read without CPU commit. Success writes the new physical slot,
decrements TOP, clears C1 under the local push policy, retains PE and other
condition bits, records FIP/FDP, advances RIP and retires once. Native admission
explicitly denies the operation.

Independent raw binary80 literals cover ±1, neighboring fractions, minimum
and maximum Normal inputs, and exponent 127 for all TOP×RC combinations.
Additional checks cover sticky PE and C1, unrelated tags/payloads, unaligned
exact Dword read/canaries/no writes, invalid classes after one read, invalid
gates before access, canonical range crossing and arithmetic overflow, read
#PF/backend errors, and fetch/truncation/STOP/zero-budget boundaries. Exact
`D9 00`, `D9 00 DF E0`, and invalid-class schema/CLI fixtures were added.

Normative source: Intel SDM revision 093, Vol.2A, “FLD—Load Floating-Point
Value,” pp.3-352–3-353, official PDF p.470. The local slice excludes special
source classes and general x87 exception behavior; native or physical x86
acceptance was not performed. S2-31 and ISSUE-019 remain open.

## S2-31q — bounded FSTP m80fp partial slice

Added exact bare `DB 38` (`FSTP m80fp [RAX]`) in Long64. The new path accepts
only an existing x87 state with Valid physical TOP, canonical-Normal binary80,
one of the four all-masked FCWs and `FSW & 0x80df == 0`. It validates the
complete canonical48 ten-byte destination and checked endpoint before issuing
one `GuestMemory::write` with the original raw bytes. After success it empties
old TOP, advances TOP modulo eight, clears C1, records FIP/FDP and advances RIP.
PE and all remaining status bits, all register payloads and unrelated state are
preserved. Retention of undefined C0/C2/C3 is documented as local policy.

Two schema-v2 literal fixtures cover exact `DB 38` and `DB 38 DF E0`; CLI
assertions verify the raw ten-byte result, pop state, status observation and
retirement counts. Interpreter tests cover all TOPs and wrap, raw fractional
and minimum/maximum Normal payloads, all accepted RC settings, PE/C1 and
condition retention, unaligned canaries including byte 11, exactly one write
and zero reads, bad environment/tag/class before access, canonical start/end
and overflow, denied/backend/#PF write errors, and fetch/truncation/STOP/budget
atomicity. Exhaustive DB ModR/M and prefix checks preserve existing forms.
Native instruction admission explicitly denies the operation.

Validation passed: full `cargo test --workspace --offline`,
`cargo check --workspace --all-targets --offline`, focused S2-31q interpreter,
decoder and CLI/schema tests, all 84 `fixture_v2` tests,
`cargo check --target x86_64-unknown-linux-gnu -p od-x86-runner --all-targets
--offline`, formatting and `git diff --check`. The cross-target `cargo test
--no-run` attempt could compile objects but could not link with the host macOS
linker; the Linux all-target `cargo check` successfully compiled the
Linux-gated native admission tests without linking. `tools/check_layout.py`
continues to report the pre-existing unmapped `.serena/` directory; it was left
untouched. The graph was refreshed with `graphify update .`. No native
instruction or physical x86 acceptance was run. Independent gpt-6.1-sol review
found no functional defects; it identified overly broad wording about DB forms
in the contract, which was narrowed to the unadmitted DB /7 store forms.

Normative source: Intel SDM revision 093, Vol.2A, “FST/FSTP—Store Floating
Point Value,” pp.3-386–3-387 (`DB /7`), official PDF p.504
([Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=504));
Vol.1 §§8.1.8, 8.1.9 and §8.6 for x87 state and FIP/FDP.

## S2-31m — bounded `FST m64fp` partial slice

Added exact bare `DD 10` as `FST m64fp [RAX]` in Long64. It reuses the
unchanged S2-31l `bounded_x87_m64fp_result` conversion and the same Valid
physical TOP, canonical-Normal source, four exact PC64/all-masked FCWs and
`FSW & 0x80df == 0` gates. Integer-only conversion packs binary64 bits and
keeps the local original/rounded exponent interval at -1022..1022, excluding
1023 even though finite binary64 values exist there.

The executor validates the full canonical48 Qword range and makes one atomic
Qword write without a data read. After success it commits sticky PE/C1,
FIP/FDP, RIP+2 and one retirement while preserving TOP, every tag, every raw
payload including unrounded ST(0), FCW/FOP, C0/C2/C3 and unrelated state.
Condition-code retention and exact-result C1 clearing are deterministic local
policies. Architectural store faults map to guest faults; backend write
failures remain diagnostic errors. Native admission explicitly denies FST.

Literal interpreter vectors use independently stated binary64 results for
signs, all RC modes, exact and half-adjacent values, discarded-bit boundaries,
exponent endpoints and carry rejection. Additional checks cover all eight TOP
positions, no-pop payload/tag sentinels, the unrounded inexact source, Qword
bytes/canaries, one-write/no-read, separate noncanonical-start and
canonical-crossing cases, backend failure, injected `#PF`, invalid
source/environment, and fetch/truncation/STOP/budget boundaries. Two
handwritten schema-v2 fixtures cover exact `DD 10` and `DD 10 DF E0` CLI
composition. Existing DD decoder forms remain preserved.

Validation passed: focused S2-31m tests, `cargo test --workspace --offline`,
`cargo check --workspace --all-targets --offline`, `cargo fmt --all -- --check`,
and `git diff --check`. `graphify update .` refreshed the graph. The layout
checker reports the pre-existing unmapped root `.serena/` directory; it was
left untouched. No Mac mini or physical acceptance was run. Flat fixture RAM
does not establish MMU, MMIO, SMP, physical x86 behavior, exception delivery or
hardware exception priority. Independent `gpt-6.1-sol` review found no
remaining findings after two test/comment observations were corrected and
re-reviewed. This partial slice does not complete S2-31.

Normative source: Intel SDM revision 093, Vol.2A, “FST/FSTP—Store Floating
Point Value,” pp.3-386–3-387 (`DD /2` FST, `DD /3` FSTP)
([official PDF, Vol.2A p.3-386](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=504),
[official PDF, Vol.2A p.3-387](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=505));
Vol.1 §§8.1.8, 8.1.9 and §8.6 for x87 state and FIP/FDP.

## S2-31n — bounded `FILD m32int` partial slice

Added exact bare `DB 00` in Long64. Preflight requires an existing x87
snapshot, FCW exactly `037f/077f/0b7f/0f7f`, `FSW & 0x80df == 0`, and an Empty
tag at physical `(old_TOP+7)&7`; it deliberately ignores old TOP's tag and
all unrelated tags/payloads. The complete canonical48 Dword source range is
validated before one typed Dword read. After that read succeeds, signed i32
values are widened to i64 and normalized to exact canonical binary80 through
the shared integer-only i64 packer,
then the payload/tag and TOP are committed together with C1 clear and FIP/FDP/
RIP updates. Zero is ten zero bytes with a Zero tag. C0/C2/C3 retention is an
explicit local policy because these bits are undefined for this form.

Independent literal vectors cover zero, ±1/2/3, `2^24±1`, i32 maximum and
minimum. Interpreter matrices cover all TOP values including 0→7 wrap, all
four RC settings, an Empty old ST(0), stale destination payload replacement,
and preservation of the other seven tags/payloads and unrelated CPU state.
Atomic reject checks cover missing state, each environment gate, nonempty
destination before memory, canonical-start/crossing errors, architectural
read fault, backend failure, fetch/truncation/STOP/zero-budget boundaries.
Exhaustive DB ModR/M and prefix checks accept only `DB 00`; two schema-v2
fixtures cover exact `DB 00` and `DB 00 DF E0` composition. Native x87
admission explicitly denies FILD.

Validation passed: focused FILD interpreter, decoder, fixture/schema, CLI and
JIT-denial checks; `cargo test --workspace --offline`,
`cargo check --workspace --all-targets --offline`, `cargo fmt --all -- --check`,
and `git diff --check`. `graphify update .` refreshed the graph. The layout
checker reports only the pre-existing unmapped root `.serena/` directory, which
was left untouched. No Mac mini or physical acceptance was run; all physical
x86 acceptance remains reserved for one combined final S2-31 substage. This
partial local implementation does not complete S2-31 or establish hardware
exception delivery or fault priority.

Normative source: Intel SDM revision 093, Vol.2A, “FILD—Load Integer,”
pp.3-342–3-343 ([official PDF, Vol.2A p.3-342](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=460),
[official PDF, Vol.2A p.3-343](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=461));
Vol.1 §§8.1.8, 8.1.9 and §8.6 for x87 state and FIP/FDP.

## S2-31o — bounded `FILD m16int` partial slice

Added exact bare `DF 00` in Long64. It reuses the existing FILD push gates:
existing x87 state, one of the four bounded FCW values, clear `FSW & 0x80df`,
and Empty physical `(TOP+7)&7`. The executor validates the complete
canonical48 Word range before one typed Word read. After success it sign-
extends i16 to i64 and calls the shared integer-only `pack_i64_binary80`, then
commits the payload/tag, TOP, C1, FIP/FDP/RIP and retirement. PE and every
unrelated CPU/x87 field remain unchanged. C0/C2/C3 retention is a local policy.

Literal expected payloads cover zero, ±1/2/3 and 32767/-32768 across every TOP
and RC, including explicit `0x8000` and `0xffff` sign extension. State checks
cover wrap, stale Empty payload, arbitrary unrelated tags/payloads, sticky PE,
C1 clearing and old ST(0) both Empty and occupied. The unaligned Word test
records exactly one two-byte read and checks byte canaries. Canonical-start/
crossing, injected architectural read fault/backend failure, fetch/truncation,
STOP and zero-budget checks preserve state. Exhaustive DF decoding admits
only the new DF 00 while preserving all pre-existing DF and DB forms. Two
schema/CLI fixtures exercise exact `DF 00` and `DF 00 DF E0`; native admission
explicitly denies FILD m16.

Validation passed: focused S2-31o interpreter/decoder/schema/CLI checks, existing
S2-31n regressions, `cargo test --workspace --offline`, `cargo check --workspace
--all-targets --offline`, `cargo fmt --all -- --check`, and `git diff --check`.
The Linux-gated x86 runner test code also passed `cargo check -p od-x86-runner
--tests --target x86_64-unknown-linux-gnu --offline`; the native-deny test itself
was not run on this macOS host.
`graphify update .` refreshed the graph. `python3 tools/check_layout.py` reports
only the pre-existing unmapped root `.serena/` directory; it was left untouched.
Independent `gpt-6.1-sol` review caught incorrect DB00 bytes in the new fixtures
and missing negative two/three vectors. Both fixtures now encode DF00/DF00DFE0,
their adjacent third byte distinguishes Word from Dword, and the missing
negative vectors are present; re-review found no remaining findings. No Mac
mini or physical acceptance was run; it remains in the combined final S2-31
substage.

Normative source: Intel SDM revision 093, Vol.2A, “FILD—Load Integer,”
pp.3-342–3-343 ([official PDF, Vol.2A p.3-342](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=460),
[official PDF, Vol.2A p.3-343](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=461));
Vol.1 §§8.1.8, 8.1.9 and §8.6 for x87 state and FIP/FDP.

## S2-31p — bounded `FILD m64int` partial slice

Added exact bare `DF 28` in Long64 while retaining the existing DF and DB
forms. The executor uses the bounded FILD state gates, validates the complete
canonical48 Qword range before one typed Qword read, and commits the signed
integer push only after the read succeeds. The shared binary80 packer now
accepts i64 and uses integer arithmetic plus `unsigned_abs`; it handles
`i64::MIN`, represents every i64 exactly, emits ten zero bytes/Zero tag for
zero, and has no RC-dependent rounding or new PE effect. Existing m16/m32
FILD paths reuse this packer.

Independent literals cover zero, ±1/2/3, i64 minimum/maximum, ±(`2^53−1`),
`2^53`, ±(`2^53+1`), and ±(`2^62−1`), ±`2^62`, ±(`2^62+1`) across all
TOP×RC combinations. State
coverage includes wrap, stale Empty payload, unrelated tags/payloads, old
ST(0) Empty/occupied, PE preservation and C1 clearing. An unaligned Qword
with canaries records exactly one eight-byte read. Atomic rejects cover bad
state/environment/destination gates, noncanonical and crossing canonical
ranges, arithmetic overflow, architectural #PF and backend read failure,
plus fetch fault/truncation, STOP and zero budget. Exhaustive DF ModR/M and
legacy/REX prefix checks admit only `DF 28` as the new form while preserving
DF 00/10/18/38/E0/E9/F1 and all existing DB forms.

Two handwritten schema-v2 fixtures contain exact `DF 28` and
`DF 28 DF E0`; interpreter/CLI checks validate their code bytes from fixture
content, observe the pushed binary80 state and status, and confirm no RAM
writes. Native admission explicitly denies FILD m64. General x87 exception
delivery/priority, `#NM`, MMU/MMIO, SMP and physical x86 behavior remain
unverified. S2-31 remains partial and final physical acceptance remains
reserved for the combined S2-31 substage.

Validation passed: `cargo test --workspace --offline`,
`cargo check --workspace --all-targets --offline`, target-specific native
denial test compilation for `x86_64-unknown-linux-gnu`, and focused FILD,
decoder, fixture/schema and CLI tests. `cargo fmt --all -- --check` and
`git diff --check` passed. Independent Sol-medium review found no correctness
issues; its stale i32 helper comment was corrected. `python3
tools/check_layout.py` reports only the pre-existing unmapped `.serena/`
directory, which was left untouched. `graphify update .` was run after the
code changes. No native execution or physical x86 acceptance was performed.

Normative source: Intel SDM revision 093, Vol.2A, “FILD—Load Integer,”
pp.3-342–3-343 ([official PDF, Vol.2A p.3-342](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=460),
[official PDF, Vol.2A p.3-343](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=461));
Vol.1 §§8.1.8, 8.1.9 and §8.6 for x87 state and FIP/FDP.

## S2-31r — bounded `FLD m80fp` partial slice

Added exact bare `DB 28` for Long64 `FLD m80fp [RAX]`; existing DB forms remain
unchanged and `DB 30` remains Unsupported. The operation reuses the bounded
FILD push preflight, validates the entire canonical48 ten-byte address before
memory access, and performs exactly one `GuestMemory::read` into a local
`[u8; 10]`. It accepts canonical-Normal binary80 only, preserving all ten raw
source bytes verbatim. Zero, subnormal, infinity, NaN and noncanonical classes
are rejected after that single read without CPU commit. Success writes the
new physical destination, sets its tag Valid, decrements TOP, clears C1 under
the existing local policy, records FIP/FDP, advances RIP by two and retires
once; all other payloads, tags, FSW bits and FCW are preserved. Native
admission explicitly denies this operation.

Tests cover all TOP values and FCW variants, every C0/C2/C3 retention case,
sticky PE and incoming-C1 clearing, arbitrary unrelated tags/payloads, an
irrelevant old ST(0), literal positive/negative Normal encodings and exponent
endpoints, exact unaligned ten-byte read/no writes with byte-11 canary, and a
read-only memory fixture. Atomic rejection coverage includes occupied physical
destination, missing state/invalid environment, full-range noncanonicality,
crossing and arithmetic overflow, backend read failure and guest #PF, all
non-Normal classes, instruction fetch/truncation, STOP and zero budget.
Exhaustive DB ModR/M and prefix checks admit only this exact new form while
preserving all pre-existing DB cases and rejecting DB 30. Three handwritten
literal fixtures check `db28`, `db28dfe0` composition, and invalid-class
no-commit behavior.

Validation passed: `cargo test --workspace --offline`,
`cargo check --workspace --all-targets --offline`,
`cargo check -p od-x86-runner --all-targets --target
x86_64-unknown-linux-gnu --offline` (compile-only), the focused interpreter
and CLI fixture tests, all 88 Python CPU/generator tests,
`cargo fmt --all -- --check`, and `git diff --check`. Independent
`gpt-6.1-sol` review found no remaining findings after the fixture source
regions were made read-only. `python3 tools/check_layout.py` reports only the
pre-existing unmapped `.serena/` directory; it was left untouched.
`graphify update .` was run after code changes. No Mac mini, native execution,
or physical acceptance was performed; those remain in the combined final
S2-31 substage. The implemented Normal-only domain and C1 policy are local
scope decisions; other binary80 classes, general x87 exception behavior, and
native/physical behavior remain deferred.

Normative source: Intel SDM revision 093, Vol.2A, “FLD—Load Floating-Point
Value,” pp.3-352–3-353 ([official PDF, Vol.2A p.3-352](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=470),
[official PDF, Vol.2A p.3-353](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=471));
Vol.1 §§8.1.8, 8.1.9 and §8.6 for x87 state and FIP/FDP.

## S2-31t — bounded `FLD m64fp` partial slice

Added exact bare `DD 00` in Long64. The interpreter validates the bounded x87
push environment and complete canonical48 Qword range before one Qword read,
then expands only binary64 Normal values into binary80 with integer field
operations. Unbiased exponent +1023 is accepted. Success updates the destination
payload/tag, TOP, C1, FIP/FDP and RIP while retaining PE and unrelated state;
special source classes reject after the read without commit. Exact `DD 00`,
`DD 00 DF E0`, and invalid-class fixtures are schema-checked and run by the
headless CLI. Native admission explicitly denies all three.

Interpreter coverage uses independent raw binary80 literals for signed one, an
adjacent fraction, minimum and maximum Normal, and exponent +1023 over every
TOP×FCW pair. Invalid-class and x87-gate cases verify read ordering and no
commit; exact access, canaries, fault mapping, fetch/truncation/STOP/budget
atomicity, and exhaustive DD decode are covered. `cargo test --workspace
--all-targets --offline` passed all 687 tests. `cargo check --workspace
--all-targets --offline` and the same workspace check for
`x86_64-unknown-linux-gnu` pass. The final focused rerun of the two new FLD m64
interpreter tests also passed. Formatting and `git diff --check` pass.
`tools/check_layout.py` reports the existing unmapped `.serena/` directory; it
remains untouched. Graphify was refreshed after the final test change. General
x87 exception delivery, MMU/MMIO and physical x86 acceptance remain outside
this partial S2-31 slice.

## S2-31u — bounded `FLD m64fp` signed-zero extension

Extended exact bare `DD 00` to accept binary64 +0 and -0 after the existing
single Qword read. Signed zero expands to the literal ten-byte binary80 zero
payload, preserving -0 in the final sign byte, and receives `X87Tag::Zero`.
Normal exponent fields 1..2046 keep the previous exact integer expansion and
Valid tag. Positive/negative nonzero subnormals, infinity and NaNs still return
Unsupported after the read without CPU commit. Push gates, complete
canonical48-range preflight, TOP update/wrap, C1 clearing, PE and other status
retention, FIP/FDP/RIP and native denial are unchanged.

Independent +0/-0 literals run over all eight TOP values and four supported
FCW settings. Existing Normal vectors also run over that matrix. The tests
check signed raw payloads/tags, sticky PE and C1 clearing, old ST(0)
independence, unrelated slots, one exact unaligned Qword read, and no commit
for positive/negative minimum-subnormal and other unsupported classes. Exact
`DD 00` and `DD 00 DF E0` schema/CLI fixtures now exercise successful +0; the
composition asserts `FNSTSW AX` observed the updated status. The former
zero-rejection fixture now uses positive minimum subnormal and remains an
Unsupported no-commit case. The S2-31t report above remains the historical
Normal-only record.

Validation passed: `cargo test --workspace --all-targets --offline` (687
tests), `cargo check --workspace --all-targets --offline`,
`cargo check --workspace --all-targets --target x86_64-unknown-linux-gnu
--offline` (compile-only), `cargo fmt --all -- --check`, and `git diff --check`.
The Linux compile emitted existing dead-code warnings in `od-host` and
`od-runner`. `python3 tools/check_layout.py` reports the pre-existing unmapped
`.serena/` directory; it was left untouched. Independent review found no
remaining functional defect after its requested fixture-link, composition
assertion, and coverage-wording corrections. Graphify is refreshed after the
final code edits. No Mac mini, native execution, or physical x86 acceptance was
performed; those remain in the combined final S2-31 substage. General x87
exception delivery, MMU/MMIO/SMP and physical behavior remain unverified.

Normative source: Intel SDM revision 093, Vol.2A, “FLD—Load Floating-Point
Value,” pp.3-352–3-353, official PDF pp.470–471
([Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=470));
Vol.1 §§8.1.8, 8.1.9 and §8.6 for x87 state and FIP/FDP. Signed-zero tag and
packing details are local project policy.

## S2-31v — bounded `FLD m32fp` signed-zero extension

Extended exact bare `D9 00` to accept binary32 +0 and -0 after its existing
single Dword read. Signed zero expands to the exact ten-byte binary80 payload,
including sign byte `0x80` for -0, and receives `X87Tag::Zero`. Normal exponent
fields 1..254 retain the prior integer expansion and Valid tag. Nonzero
subnormals, infinity and NaNs still return Unsupported after the read without
CPU commit. The x87 gates, full canonical48 Dword preflight, single access,
push/TOP/status/FIP/FDP/RIP behavior and native denial remain unchanged.

Independent signed-zero literals cover all eight TOP values and four supported
FCW settings alongside the preserved Normal vectors. Both signs of minimum
subnormal are rejected after one Dword read in separate single-setup cases.
The `D9 00` and `D9 00 DF E0` schema/CLI fixtures cover +0 and -0, and assert
the resulting Zero tag and signed binary80 payload. The invalid-class fixture
now uses positive minimum subnormal. Schema admission of Unsupported is
narrowed to a nonzero binary32 subnormal for exact `D9 00`, budget one and no
stop RIP; zero, Normal and infinity near-misses are rejected by tests.

Validation passed: `cargo test --workspace --all-targets --offline` (687
tests), `cargo check --workspace --all-targets --offline`, and the same
workspace check for `x86_64-unknown-linux-gnu` (compile-only). Focused FLD m32
interpreter and runner tests passed. `cargo fmt --all -- --check` and
`git diff --check` passed. `graphify update .` refreshed the graph. The layout
check exits with the pre-existing unmapped root `.serena/` directory; it was
left untouched. The Linux compile emitted existing
dead-code warnings in `od-host` and `od-runner`. Independent review found no
remaining defects after correcting the contract anchor and qualifying the
minimum-subnormal coverage wording. No
Mac mini, native execution, or physical x86 acceptance was performed; those
remain in the combined final S2-31 substage. General x87 exception delivery,
MMU/MMIO/SMP and physical behavior remain unverified.

Normative source: Intel SDM revision 093, Vol.2A, “FLD—Load Floating-Point
Value,” pp.3-352–3-353, official PDF pp.470–471
([Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=470));
Vol.1 §§8.1.8, 8.1.9 and §8.6 for x87 state and FIP/FDP. Signed-zero tag and
packing details are local project policy.


## S2-31w — bounded `FLD m80fp` signed-zero extension

Extended exact bare `DB 28` to accept raw binary80 +0 and -0 after the
existing single ten-byte read. Both sign encodings are preserved byte-for-byte
and assigned `X87Tag::Zero`; canonical Normal values retain their exact raw
payload and Valid tag. Positive/negative nonzero subnormals, infinity, NaNs and
other unsupported encodings still return Unsupported after exactly one read
with no CPU commit. No binary32/binary64 denormal `#D` policy is imported: the
Intel SDM revision 093 FLD m80fp entry specifies no denormal exception for
this source format. Broader denormal and unmasked exception handling is
deferred to a dedicated slice.

Literal interpreter checks cover both signed zeros across all eight TOP values
and four FCW settings while retaining Normal vectors. Both signs of minimum
nonzero subnormal and other special classes are checked for one-read,
full-state atomic rejection. The base schema/CLI fixture now loads +0 and the
`DB 28 DF E0` composition fixture loads -0; CLI assertions verify the exact
signed ten-byte payload and Zero tag. The invalid-class fixture now uses
positive minimum subnormal, with schema admission limited to nonzero binary80
subnormal data for exact `DB 28`, budget one and no stop RIP. The S2-31r report
above remains the historical Normal-only result.

Validation passed: `cargo test --workspace --all-targets --offline` (687 tests),
`cargo check --workspace --all-targets --offline`,
`cargo check --workspace --all-targets --target x86_64-unknown-linux-gnu
--offline` (compile-only), `cargo fmt --all -- --check`, and `git diff --check`.
The Linux compile emitted existing dead-code warnings in `od-host` and
`od-runner`. `python3 tools/check_layout.py` reports the pre-existing unmapped
root `.serena/` directory; it was left untouched. Independent `gpt-6.1-sol`
review found no concrete defects. `graphify update .` refreshed the code graph.
No Mac mini or physical x86 checks were performed; physical acceptance remains
in the combined final S2-31 substage. S2-31 remains partial, native admission
stays denied, and ISSUE-019 remains open for broader x87 and exception
behavior.

Normative source: Intel SDM revision 093, Vol.2A, “FLD—Load Floating-Point
Value,” pp.3-352–3-353, official PDF p.470
([Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=470));
Vol.1 §§8.1.8, 8.1.9 and §8.6 for x87 state and FIP/FDP.

## S2-31x — bounded `FLD m80fp` canonical-subnormal extension

Extended the existing exact bare `DB 28` load to accept canonical nonzero
binary80 subnormal inputs. The same FCW/FSW/Empty-destination gates, checked
canonical48 ten-byte range and exactly one raw read remain in force. Normal
and signed-zero values preserve their ten bytes with Valid and Zero tags;
subnormals preserve all ten bytes and receive `X87Tag::Special`. The existing
push, TOP, C1, sticky PE, FIP/FDP/RIP and retirement behavior remains
unchanged. No DE/UE/PE status bits are added. Pseudo-denormal, infinity, NaN,
unnormal and pseudo-special values remain Unsupported after the read without
CPU commit. Native x87 admission stays denied.

Literal subnormal vectors cover positive/negative minimum, largest and
interior values for every TOP and accepted FCW, with unrelated tags and
payloads. Assertions verify byte preservation, the Special tag, one exact
ten-byte read, preserved FSW and absence of new DE/UE. Positive-minimum and
negative-largest literals cover `DB 28` and `DB 28 DF E0` fixture/CLI
composition. The invalid fixture is reclassified as positive pseudo-denormal;
the narrow Unsupported schema profile remains exact DB 28, budget one and no
stop RIP. The corresponding predicate admits that pseudo-denormal only, and
rejects negative pseudo-denormal, canonical subnormal and other near misses.
S2-31w remains the historical signed-zero result.

Validation passed: `cargo test --workspace --all-targets --offline`,
`cargo check --workspace --all-targets --offline`, and the same all-target
workspace check for `x86_64-unknown-linux-gnu` (compile-only). The Linux check
reported existing dead-code warnings in `od-host` and `od-runner`.
`cargo fmt --all -- --check` and `git diff --check` passed. The actual runner
CLI produced the expected raw payload, Special tag, FSW, and retirement count
for both success fixtures; the pseudo-denormal fixture returned Unsupported
with zero retirements and unchanged CPU/memory. `python3 tools/check_layout.py`
reports the pre-existing unmapped `.serena/` directory; it was left untouched.
Independent `gpt-6.1-sol` medium review and targeted re-review found no
remaining defects. `graphify update .` refreshed the graph after code edits.
No Mac mini or physical x86 tests were run; physical acceptance remains in the
combined final S2-31 substage. The SDM revision 093 Vol.2A FLD m80fp entry,
pp.3-352–3-353 (official PDF p.470), specifies no `#D` for this source format.
No binary32/binary64 denormal behavior is inferred. Broader exception delivery
remains open under ISSUE-019.

## S2-31y — bounded `FLD m32fp` masked-subnormal extension

Extended exact bare `D9 00` to accept nonzero binary32 subnormal operands
under the four all-masked FCWs. Integer normalization maps fraction `f` to
significand `f << (63-k)` and unbiased exponent `k-149`, where
`k=floor(log2(f))`; sign is preserved, the pushed binary80 class is canonical
Normal, and its tag is Valid. The D900-only FSW preflight now uses
`FSW & 0x80dd`, allowing pre-existing sticky DE while the shared FILD/m64/m80
gate remains unchanged. Successful subnormal loads set DE and preserve its
sticky state, add no UE or PE, clear C1, and retain the established push,
payload, pointer and retirement behavior. Infinity and NaN still reject after
one Dword read without CPU commit. Unmasked exception delivery and push
behavior are not claimed; native admission remains denied. The binary80
`DB 28` subnormal path remains the distinct S2-31x behavior with no DE.

Independent literal vectors cover positive and negative minimum, interior
and largest binary32 subnormals across all eight TOP and four FCW settings.
The matrix also checks PE and C1 policy; a repeated-load case checks that DE
stays sticky. Positive exact `D9 00` and negative `D9 00 DF E0` fixtures are
admitted through the schema and run in the CLI; FNSTSW observes DE and the
pushed TOP. At that stage, the invalid-class fixture/schema predicate used binary32
infinity/NaN, exact `D9 00`, budget one and no stop RIP; S2-31af now accepts
infinity and narrows the current schema exception to QNaN. The S2-31v report
above remains the historical signed-zero/reject result.

Normative source: Intel SDM revision 093, Vol.1 §8.5.2 for masked denormal
operand handling; Vol.2A, “FLD—Load Floating-Point Value,” pp.3-352–3-353,
official PDF p.470
([Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=470)).
This local profile is limited to masked controls and does not infer unmasked
exception behavior.

Validation: focused `od-isa` D900 tests (3), focused runner literal fixture
tests (3), and the exact x87 schema predicate test (1) passed. The full
`cargo test --workspace --all-targets --offline`,
`cargo check --workspace --all-targets --offline`,
`cargo check --target x86_64-unknown-linux-gnu -p od-x86-runner --all-targets
--offline`, `cargo fmt --all -- --check`, and `git diff --check` passed.
`python3 tools/check_layout.py` reports the existing unmapped `.serena/`
directory; it was present before this change and left untouched. Graphify was
updated. No Mac mini or physical x86 tests were run; those remain deferred to
the combined final S2-31 substage.

## S2-31z — bounded `FLD m64fp` masked-subnormal extension

Extended exact bare `DD 00` to accept nonzero binary64 subnormal operands
under the four all-masked FCWs. Integer normalization maps fraction `f` to
significand `f << (63-k)` and unbiased exponent `k-1074`, where
`k=floor(log2(f))`; sign is preserved, the pushed binary80 class is canonical
Normal, and its tag is Valid. The DD00-only FSW preflight uses `FSW & 0x80dd`,
allowing pre-existing sticky DE while FILD and m80 gates remain unchanged.
Successful subnormal loads set sticky DE, add no UE or PE, clear C1, and retain
the existing push, payload, pointer and retirement behavior. Infinity and NaN
remain Unsupported after one Qword read without CPU commit. Unmasked exception
delivery and push behavior are not claimed; native admission remains denied.
The binary80 `DB 28` subnormal behavior remains distinct and does not set DE.

Independent literal vectors cover positive and negative minimum, interior and
largest binary64 subnormals across every TOP and supported FCW setting. The
matrix checks sticky PE/C1 behavior, and repeated loads check sticky DE.
Positive exact `DD 00` and negative `DD 00 DF E0` fixtures are admitted by the
schema and run through the CLI; FNSTSW observes DE and the pushed TOP. The
invalid-class fixture/schema predicate now uses binary64 infinity/NaN, exact
`DD 00`, budget one and no stop RIP. The S2-31u report above remains the
historical signed-zero/reject result.

Normative source: Intel SDM revision 093, Vol.1 §8.5.2 for masked denormal
operand handling; Vol.2A, “FLD—Load Floating-Point Value,” pp.3-352–3-353,
official PDF p.470 ([Vol.1 PDF](https://cdrdv2-public.intel.com/671436/253665-sdm-vol-1.pdf),
[Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=470)).
This local profile is limited to masked controls and does not infer unmasked
exception behavior.

Validation: focused `od-isa` binary64 FLD tests (2), exact x87 schema predicate
test (1), runner literal fixture test, and runner positive/negative subnormal
CLI fixture test passed. `cargo test --workspace --all-targets --offline`,
`cargo check --workspace --all-targets --offline`,
`cargo check --target x86_64-unknown-linux-gnu -p od-x86-runner --all-targets
--offline`, `cargo fmt --all -- --check`, and `git diff --check` passed.
`graphify update .` completed. `python3 tools/check_layout.py` reports the
existing unmapped `.serena/` directory; it was present before this change and
left untouched. Independent gpt-6.1-sol review found no functional defects or
remaining link issues; the inherited S2-31u contract anchors were corrected.
Mac mini and physical x86 tests were not run; physical acceptance remains
deferred to the combined final S2-31 substage.

## S2-31aa — bounded `FLD m80fp` canonical-infinity extension

Extended exact bare `DB 28` to accept only canonical positive and negative
binary80 infinities: exponent `0x7fff`, explicit integer bit set, and fraction
zero. The path retains the S2-31x gates (`FSW & 0x80df == 0`, the supported
masked FCWs, Empty destination, canonical48 ten-byte preflight and one raw
read). It preserves the ten source bytes verbatim, pushes with Special tag,
clears C1, and adds no status flags. Existing TOP, pointer, payload and
retirement behavior remains unchanged. No exception delivery is inferred;
native x87 admission remains closed.

Signed-infinity literal vectors cover every TOP×FCW pair and verify exact raw
payload, tag, status preservation, read count, and no writes. Canonical
quiet/signaling NaNs, pseudo-infinities, pseudo-denormals, pseudo-NaNs and
unnormals remain post-read Unsupported without CPU commit. The narrow
Unsupported fixture/schema exception now uses canonical positive QNaN for
exact `DB 28`, budget one and no stop RIP. Positive bare and negative
`DB 28 DF E0` fixtures run through the CLI; an integration case checks that
JIT/native admission denies the infinity load. The historical S2-31x report
above records its original pseudo-denormal schema exception and subnormal-only
extension.

The canonical infinity encoding follows Intel SDM revision 093, Vol.1 Table
4-3; Table 8-3 distinguishes pseudo-infinity and unsupported encodings
([Vol.1 PDF](https://cdrdv2.intel.com/v1/dl/getContent/671436#page=92)). The
Vol.2A FLD m80fp entry, pp.3-352–3-353 (official PDF p.470), is retained as
the instruction-form reference. See the [Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=470),
[CPU contract](../specs/CPU_CONTRACT.md#local-s2-31aa-bounded-fld-m80fp-rax-db-28-canonical-infinities),
[plan](../plans/STAGE_2.md#s2-31aa-extend-bounded-fld-m80fp-rax-db-28-to-canonical-infinities),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).




Validation: focused `od-isa` DB28 tests (2), the exact schema predicate (1),
the CLI fixture test (1), and the JIT-denial integration test (1) passed.
`cargo test --workspace --all-targets --offline` passed (692 tests across the
workspace); workspace and Linux x86 runner checks, `cargo fmt --all -- --check`,
and `git diff --check` passed. `graphify update .` completed. The layout check
reports the pre-existing unmapped `.serena/` directory; it was left untouched.
Independent gpt-6.1-sol medium review found no remaining defects. It identified
missing pseudo-signaling-NaN neighbor vectors; signed cases were added to the
interpreter and schema/CLI checks, and the focused tests plus formatting/diff
checks passed again. Mac mini and physical x86 tests were not run; physical
acceptance remains deferred to the combined final S2-31 substage.

## S2-31ab — bounded `FLD m80fp` canonical QuietNaN extension

Extended exact bare `DB 28` to accept canonical binary80 QuietNaN values:
biased exponent `0x7fff`, explicit integer bit set, quiet bit set, and nonzero
fraction. Under the unchanged all-masked FCWs, `FSW & 0x80df` gate, Empty
destination and canonical48 ten-byte one-read preflight, the load preserves all
source bytes and pushes with Special tag. It adds no status flags, including
IE, and retains sticky PE; the existing C1-clear, TOP, pointer and retirement
policy remains unchanged. Signed zero, Normal, subnormal and infinity behavior
is unchanged. Signaling NaNs, pseudo-NaNs, pseudo-denormals, pseudo-infinities,
unnormals and other classes remain post-read Unsupported without CPU commit.
Native x87/JIT admission remains closed; unmasked delivery is not implemented.

Independent signed/payload QNaN literals cover every TOP×FCW pair, including
negative indefinite. They check exact raw payload, Special tag, preserved PE,
IE clear, one ten-byte read, no writes and unchanged unrelated state. Bare
positive QNaN and negative-indefinite `DB 28 DF E0` fixtures run through the
CLI, with FNSTSW observing the pushed TOP. The narrow exact-DB28, budget-one,
no-stop Unsupported schema/CLI fixture changes from positive QNaN to positive
canonical SNaN; signed QNaNs and other classes are rejected by the exception
predicate. Native admission explicitly denies DB28.

Intel SDM revision 093 Vol.1 Table 8-3 describes canonical and pseudo-NaN
encodings ([Vol.1 PDF](https://cdrdv2.intel.com/v1/dl/getContent/671436));
Vol.2A FLD m80fp remains the instruction-form reference
([Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=470)).
See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-31ab-bounded-fld-m80fp-rax-db-28-canonical-quietnans),
[plan](../plans/STAGE_2.md#s2-31ab-extend-bounded-fld-m80fp-rax-db-28-to-canonical-qnans),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).

Validation passed: focused schema, CLI fixture, and JIT/native-denial tests;
`cargo test --workspace --all-targets --offline`;
`cargo check --workspace --all-targets --offline`;
`cargo check --target x86_64-unknown-linux-gnu -p od-x86-runner --all-targets --offline`;
`cargo fmt --all -- --check`; and `git diff --check`. The native denial test is
compiled for Linux x86; it is architecture-gated and did not execute on this
local arm64 host. `python3 tools/check_layout.py` reports the pre-existing
unmapped root `.serena` directory; it was left untouched. Independent
gpt-6.1-sol medium review found no defects in classification, gates, status
preservation, atomicity, fixture/schema precision, native denial, or docs.

## S2-31ac — bounded `FLD m80fp` canonical SignalingNaN extension

Extended exact bare `DB 28` to accept canonical binary80 SignalingNaNs only:
biased exponent `0x7fff`, explicit integer bit set, quiet bit clear and
nonzero remaining fraction. The operation preserves all ten raw bytes, keeps
the clear quiet bit and source payload, and pushes with Special tag. The
all-masked FCWs, `FSW & 0x80df` gate, Empty destination, canonical48 ten-byte
preflight and one-read/no-write commit boundary are unchanged.

The load adds no exception flags, including IE, and preserves PE/C0/C2/C3
under the existing C1-clear, TOP, pointer and retirement policy. Minimum,
middle and maximum SNaN payloads of both signs cover every TOP×FCW pair and
verify exact source bytes, tag, status, read count and no writes. Positive bare
and negative maximum-payload DB28/DFE0-with-FNSTSW CLI fixtures run. Canonical
SNaNs and pseudo-SNaNs reject against the exact no-stop/budget-one schema
exception, which now uses positive pseudo-infinity. JIT/native admission and
unmasked exception delivery remain outside scope.

Intel SDM revision 093 Vol.1 Table 4-3 describes canonical signaling and quiet
NaN encodings; §8.2.2/Table 8-3 describes pseudo-NaNs and other unsupported
encodings. Vol.2A Table 3-47 reconstructs E=0,J=1 as Special
([Vol.1 PDF](https://cdrdv2.intel.com/v1/dl/getContent/671436)); Vol.2A FLD
m80fp remains the instruction-form reference
([Vol.2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=470)).
See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-31ac-bounded-fld-m80fp-rax-db-28-canonical-signalingnans),
[plan](../plans/STAGE_2.md#s2-31ac-extend-bounded-fld-m80fp-rax-db-28-to-canonical-snans),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).

Validation passed: focused od-isa FLD m80, schema-admission, CLI fixture, and
JIT-denial tests; `cargo test --workspace --all-targets --offline`;
`cargo check --workspace --all-targets --offline`;
`cargo check --target x86_64-unknown-linux-gnu -p od-x86-runner --all-targets --offline`;
`cargo fmt --all -- --check`; and `git diff --check`. The architecture-gated
native-denial test compiles for Linux x86 but does not execute on this local
arm64 host. `python3 tools/check_layout.py` reports the pre-existing unmapped
root `.serena` directory; it was left untouched. Independent
gpt-6.1-sol medium review found no actionable defects in classification,
gates, raw bytes/status, neighboring-class rejection, schema scope, CLI
fixtures, or native/JIT denial.

## S2-31ad — bounded `FLD m80fp` pseudo-denormal extension

Exact bare `DB 28` now accepts pseudo-denormals only (exponent zero, explicit
integer bit set) under the unchanged all-masked FCWs, `FSW & 0x80df == 0`, Empty
destination, canonical48 ten-byte preflight and single raw read. It preserves
all ten bytes including exponent zero, pushes Special, and adds no DE or other
status flags. No arithmetic consumption or unmasked exception delivery is
implemented; pseudo-infinity/NaNs and unnormals remain Unsupported after one
read without CPU commit. Native/JIT admission remains closed.

Independent literal coverage spans both signs, zero/minimum/maximum fractions,
matching exponent-one Normal controls, every TOP×FCW pair, raw payload/tag/
status invariants, and existing read/fault/atomicity behavior. Positive bare
and negative DB28/DFE0 FNSTSW fixtures run through the CLI. The schema exception
remains narrow to positive pseudo-infinity with exact DB28, budget one and no
stop RIP; pseudo-denormal neighbors are excluded. Intel SDM revision 093 Vol.1
§8.2.2/Table 8-3 describes pseudo-denormal handling; Vol.2A FLD m80fp states
that `#D` does not occur for the m80fp form. Table 3-47 reconstructs E=0,J=1
as Special. See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-31ad-bounded-fld-m80fp-rax-db-28-pseudo-denormals),
[plan](../plans/STAGE_2.md#s2-31ad-extend-bounded-fld-m80fp-rax-db-28-to-pseudo-denormals),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).

## S2-31ae — bounded `FLD m64fp` signed-infinity extension

Exact bare `DD 00` now accepts signed binary64 infinities only (exponent
`0x7ff`, zero fraction), converting them by integer fields into canonical
binary80 infinities with Special tag. The four all-masked FCWs, DD00-local
`FSW & 0x80dd` gate, Empty destination, canonical48 eight-byte preflight and
one Qword read remain unchanged. Sticky DE/PE, existing C1-clear/TOP/pointer
behavior and retirement are preserved. QNaN/SNaN remain Unsupported after the
read without CPU commit. Unmasked delivery and JIT/native admission remain
outside scope; m80fp subnormal loads retain their no-DE behavior.

Independent signed infinity literals cover every TOP×FCW pair. A subnormal
followed by infinity checks sticky DE and PE. The positive bare and negative
DD00/DFE0+FNSTSW fixtures verify exact output bytes/status through CLI. The
invalid-class fixture and exact DD00/budget-one/no-stop schema predicate now
use QNaN, while signed infinities and SNaNs are excluded from rejection
fixtures. Intel SDM revision 093 Vol.1 Table 4-3 describes binary64 special
encodings; Vol.2A FLD m64fp is the instruction reference. The reconstructed
x87 tag classification uses Vol.2A Table 3-47, “Recreating FSAVE Format.”

Validation passed: focused FLD literal/status, rejection, schema, CLI fixture,
JIT denial and native-profile checks; full workspace suite (695 tests);
`cargo check --workspace --all-targets --offline`; Linux x86 runner check;
`cargo fmt --all -- --check`; `git diff --check`; and graphify update. The
architecture-gated native-denial test compiles for Linux x86 but does not
execute on this local arm64 host. `python3 tools/check_layout.py` reports the
pre-existing unmapped root `.serena` directory, which was left untouched.
Independent gpt-6.1-sol medium review found no concrete defects in conversion,
gates, status preservation, NaN rejection, fixtures, denial paths or docs. See
the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-31ae-bounded-fld-m64fp-rax-dd-00-signed-infinities),
[plan](../plans/STAGE_2.md#s2-31ae-extend-bounded-fld-m64fp-rax-dd-00-to-infinities),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).

## S2-31af — bounded `FLD m32fp` signed-infinity extension

Exact bare `D9 00` now accepts signed binary32 infinities only (exponent 255,
zero fraction), converting by integer fields to canonical binary80 infinity
with Special tag. The four all-masked FCWs, D900-local `FSW & 0x80dd` gate,
Empty destination, canonical48 four-byte preflight and one Dword read remain
unchanged. No status flags are added; sticky DE/PE, C1-clear/TOP/pointer policy
and retirement are preserved. Existing zero, Normal and masked-subnormal
paths are unchanged. QNaN/SNaN remain post-read Unsupported with no CPU commit.
Unmasked delivery and JIT/native execution remain outside scope; binary80
m80fp subnormals retain their separate no-DE behavior.

Independent signed infinity literals cover every TOP×FCW pair. A minimum
subnormal followed by infinity verifies sticky DE and PE. Bare positive D900
and negative D900/DFE0+FNSTSW fixtures verify the canonical bytes, Special tag,
status and retirement through CLI. The exact D900/budget-one/no-stop schema
exception now uses canonical QNaN, while infinity and SNaN neighbors are
excluded. JIT and native admission remain closed.

Intel SDM revision 093 Vol.1 §8.5.2 describes masked denormal-operand
handling, Table 4-3 gives floating point encodings, and Vol.2A FLD m32fp,
pp.3-352–3-353 (official PDF p.470), specifies the load form. Table 3-47,
“Recreating FSAVE Format,” supplies Special tag classification (official PDF
p.532). This all-masked profile does not infer unmasked exception delivery.
See the [Vol.1 PDF](https://cdrdv2.intel.com/v1/dl/getContent/671436),
[Vol.2A PDF, FLD](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=470),
[Vol.2A PDF, Table 3-47](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=533),
[CPU contract](../specs/CPU_CONTRACT.md#local-s2-31af-bounded-fld-m32fp-rax-d9-00-signed-infinities),
[plan](../plans/STAGE_2.md#s2-31af-extend-bounded-fld-m32fp-rax-d9-00-to-infinities),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).

Validation passed: focused D900 literal/status/rejection tests, schema
admission, CLI fixtures, invalid-class atomicity and JIT denial; full workspace
suite (697 tests); `cargo check --workspace --all-targets --offline`; Linux x86
runner check; `cargo fmt --all -- --check`; `git diff --check`; and graphify
update. The architecture-gated native-denial test compiles for Linux x86 but is
not executed on this local arm64 host. `python3 tools/check_layout.py` reports
the pre-existing unmapped root `.serena` directory, which was left untouched.
Independent gpt-6.1-sol medium review found no concrete findings. All nine
focused ISA/schema/runner review checks passed; native runtime denial remains
unexecuted on this local arm64 host.

## S2-31ag — bounded `FLD m64fp` canonical QNaN extension

Exact bare `DD 00` now accepts canonical binary64 QNaNs (exponent `0x7ff`,
quiet bit set) of either sign and any lower payload. After the existing
canonical48 eight-byte preflight and one Qword read, integer conversion forms
binary80 significand `0x8000000000000000 | (fraction << 11)`, exponent
`0x7fff`, preserves the sign, and assigns Special. No status flags are added,
including IE; sticky DE/PE and existing C1-clear/TOP/pointer/retirement policy
remain. FCWs, `FSW & 0x80dd`, Empty destination, and zero/Normal/subnormal/
infinity behavior remain fixed. SNaNs still reject after one read without CPU
commit. Native and JIT admission remain denied; unmasked behavior is deferred.

Independent quiet-only, low, middle and max payload literals of both signs
cover TOP×FCW. A subnormal followed by a negative payload-bearing QNaN checks
sticky DE/PE. Positive bare DD00 and negative DD00/DFE0+FNSTSW fixtures assert
payload preservation through CLI. The exact DD00/budget-one/no-stop schema
exception now admits canonical SNaN only. The native runtime-denial check is
architecture-gated and was not executed on the local arm64 host.

Validation passed: focused DD00 conversion/rejection tests, the exact
od-testkit schema test (and all 166 schema-crate tests), CLI fixtures, and JIT
denial; full offline workspace suite (697 tests);
`cargo check --workspace --all-targets --offline`; Linux x86 `od-x86-runner`
check; `cargo fmt --all -- --check`; and `git diff --check`. Graphify refreshed
to 3903 nodes and 8185 edges. The architecture-gated native runtime denial
compiles for Linux x86 but is not executed on this local arm64 host. The layout
checker reports the pre-existing unmapped `.serena/` directory, which remains
untouched.

Independent gpt-6.1-sol medium review found no concrete defects. All six
focused ISA/schema/CLI/JIT review checks passed; native runtime denial was not
run on the local arm64 host.

## S2-31ah — bounded `FLD m32fp` canonical QNaN extension

Exact bare `D9 00` accepts canonical binary32 QNaNs (exponent `0xff`, quiet
bit 22 set) of either sign and any lower payload. After the existing
canonical48 four-byte preflight and one Dword read, integer conversion forms
binary80 significand `0x8000000000000000 | (fraction << 40)`, exponent
`0x7fff`, preserves the sign and assigns Special. No exception flags are
added, including IE; sticky DE/PE and existing C1-clear/TOP/pointer/retirement
policy remain. The four FCWs, `FSW & 0x80dd`, Empty destination, and
zero/Normal/subnormal/infinity behavior remain fixed. SNaNs still reject after
one read without CPU commit. Native and JIT admission remain denied; unmasked
behavior and m64 SNaN behavior are deferred.

Independent quiet-only, low, middle and maximum payload literals of both
signs cover TOP×FCW. A subnormal followed by a negative payload-bearing QNaN
checks sticky DE/PE. Positive bare D900 and negative payload-bearing
D900/DFE0+FNSTSW fixtures verify exact bytes and retirement through CLI. The
exact D900/budget-one/no-stop schema exception now admits canonical SNaN only;
the invalid-class CLI fixture was updated accordingly. Native/JIT denial
coverage includes both QNaN fixtures.

Validation passed: focused ISA, schema, CLI, SNaN atomic-rejection and
JIT-denial checks; full offline workspace suite (697 tests);
`cargo check --workspace --all-targets --offline`; Linux x86
`od-x86-runner` all-target check; `cargo fmt --all -- --check`; and
`git diff --check`. Graphify refreshed to 3911 nodes and 8193 edges. The
architecture-gated native runtime denial compiles for Linux x86 but is not
executed on this local arm64 host. `python3 tools/check_layout.py` reports the
pre-existing unmapped `.serena/` directory, which remains untouched.

## S2-31ai — bounded `FLD m64fp` canonical SNaN extension

Exact bare `DD 00` now accepts canonical binary64 SNaNs after one Qword read.
Integer-only conversion sets the binary80 explicit integer and quiet bits,
maps the source payload by eleven places, preserves sign, and assigns Special.
The implementation sets sticky IE only; DE/PE remain sticky, and ES/SF/B and
other flags are not introduced. Existing all-masked FCWs, `FSW & 0x80dd`
gate, Empty destination, C1/TOP/pointers, and retirement behavior remain.
Pre-existing IE is rejected before memory access. A later DD00 load is blocked
while IE remains set; FNCLEX clears it. Binary32 SNaN and raw m80 SNaN behavior
are unchanged. Native/JIT and unmasked delivery remain outside scope.

Signed minimum, middle and maximum SNaN payload vectors cover every TOP×FCW
pair; sticky DE/PE, exact FSW/FNSTSW, one-read fault atomicity, IM=0 and
pre-existing-IE pre-read rejection, and FNCLEX recovery are covered. Positive
bare and negative DD00/DFE0+FNSTSW CLI fixtures are included. The exact
DD00/budget-one/no-stop Unsupported schema exception now describes an invalid
FCW or pre-existing status gate, independent of source value. JIT/native
denial fixture coverage includes both new fixtures.

Validation passed: focused DD00 ISA/schema/CLI/JIT checks; the full offline
workspace suite (699 tests); `cargo check --workspace --all-targets --offline`;
Linux x86 `od-x86-runner` all-target check; `cargo fmt --all -- --check`;
`git diff --check`; and relative Markdown link validation. Graphify refreshed
to 3921 nodes and 8207 edges. `python3 tools/check_layout.py` reports the
pre-existing unmapped `.serena/` directory, which remains untouched. The
architecture-gated native denial compiles for Linux x86 but was not executed
on this local arm64 host. Independent gpt-6.1-sol medium review found no
remaining defects after correcting the report anchor; it rechecked conversion,
gates, fixture/schema reachability, FNCLEX recovery, and native/JIT denial.

Intel SDM revision 093 Vol.1 §4.8.3.5/Table 4-8 describe setting the SNaN
quiet bit on masked invalid operation and §8.1.3.3 describes x87 status
reporting; Vol.2A FLD m64fp (PDF p.470) identifies SNaN as #IA, and Table
3-47 “Recreating FSAVE Format” (PDF p.533) supports the Special tag. See
[Vol.1 §4.8.3.5/Table 4-8](https://cdrdv2-public.intel.com/929351/253665-093-sdm-vol-1.pdf#page=105),
[Vol.1 §8.1.3.3](https://cdrdv2-public.intel.com/929351/253665-093-sdm-vol-1.pdf#page=211),
[Vol.2A FLD](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=470), and
[Vol.2A Table 3-47](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=533).
See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-31ai-bounded-fld-m64fp-rax-dd-00-canonical-snans),
[plan](../plans/STAGE_2.md#s2-31ai-extend-bounded-fld-m64fp-rax-dd-00-to-canonical-snans),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).

## S2-31aj — bounded `FLD m32fp` canonical SNaN extension

Exact bare `D9 00` now accepts canonical binary32 signaling NaNs after one
Dword read. Integer-only conversion sets binary80's explicit integer and
quiet bits, shifts the source fraction by 40, preserves sign, and assigns
Special. Sticky IE alone is added; DE/PE and other admitted status remain
sticky. Existing FCWs, the D900-local `FSW & 0x80dd` gate, Empty destination,
C1/TOP/pointer/retirement behavior, canonical48 preflight and one-read access
remain unchanged. Subsequent D900 loads are rejected while IE is set until
FNCLEX clears status. Binary64 and raw binary80 SNaNs retain their operation-
specific behavior; unmasked delivery and native/JIT execution remain outside
scope.

Independent signed low, middle and maximum payload vectors cover every
TOP×FCW pair, with exact quieted binary80 literals. Tests verify clean TOP=0
FSW `0x3801`, sticky DE/PE, SNaN→quieted-QNaN composition, one-Dword-read
fault behavior, pre-read unmasked-FCW/pre-existing-IE gates, subsequent-load
rejection, and FNCLEX clearing exception flags while preserving TOP, tags and
payloads before another load. Positive bare and negative payload-bearing
D900/DFE0+FNSTSW CLI fixtures are included. The exact D900/budget-one/no-stop
Unsupported schema exception now represents an invalid environment gate,
independent of source class. Native/JIT deny fixtures include both SNaN cases.

Focused validation passed: `od-isa` D900 matrices/recovery (5 tests), the
D900 schema predicate (1), successful CLI fixtures (1), JIT denial (1), and
invalid-environment pre-read rejection (1). The full offline workspace suite
passed (700 tests); workspace and Linux x86 runner all-target checks,
`cargo fmt --all -- --check`, and `git diff --check` passed. `graphify update .`
completed with 3930 nodes and 8217 edges. The layout check reports the existing
unmapped `.serena/` directory, which remains untouched. Independent
`gpt-6.1-sol` medium review was clean after restoring the prior S2-31aa
validation paragraph and adding a clean TOP=0 assertion for SNaN FSW `0x3801`.
The reviewer confirmed conversion/status semantics, pre-read gates, atomicity,
fixture/schema/native/JIT reachability and scope boundaries. No Mac mini or
physical x86 test was run; physical acceptance remains deferred to the
combined final S2-31 substage.

Intel SDM revision 093 Vol.1 §4.8.3.5/Table 4-8 describes SNaN quieting by
setting the most-significant fraction bit; §8.1.3.3 describes x87 invalid
operation status. Vol.2A FLD m32fp identifies SNaN as #IA; reconstructed tag
classification follows Table 3-47, “Recreating FSAVE Format” (PDF p.533). The
local profile covers all-masked FCWs only. See [Vol.1 §4.8.3.5/Table 4-8](https://cdrdv2-public.intel.com/929351/253665-093-sdm-vol-1.pdf#page=105),
[Vol.1 §8.1.3.3](https://cdrdv2-public.intel.com/929351/253665-093-sdm-vol-1.pdf#page=211),
[Vol.2A FLD](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=470), and
[Vol.2A Table 3-47](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=533).
See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-31aj-bounded-fld-m32fp-rax-d9-00-canonical-snans),
[plan](../plans/STAGE_2.md#s2-31aj-extend-bounded-fld-m32fp-rax-d9-00-to-canonical-snans),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).

## S2-31ak — sticky IE/DE/PE across bounded m32/m64 FLD

S2-31ak changes only the local status gates for exact D900 and DD00 from
`FSW & 0x80dd == 0` to `FSW & 0x80dc == 0`. Either load now accepts all eight
combinations of sticky IE, DE and PE, preserves those flags, and continues to
reject ZE, OE, UE, SF, ES and B before reading memory. A signaling NaN adds IE;
a nonzero subnormal adds DE. FCWs, Empty destination, memory preflight/read
discipline and pointer/push policy remain unchanged. Other x87 operation gates
are unchanged.

Coverage includes the full sticky-flag × FCW × TOP × source-class matrix,
same-width and mixed-width sequences, repeated SNaNs, second-load fault
atomicity, per-bit forbidden-status rejection, occupied destination and
unmasked-FCW controls, FNCLEX recovery, and the existing CLI/FNSTSW fixtures.
The exact D900/DD00 budget-one/no-stop schema predicate now requires forbidden
status rather than a source-class exception. Native/JIT remain closed, and no
unmasked behavior is implied by the all-masked profile.

## S2-31al — signed-zero stores for bounded m32 FST/FSTP

Exact `D9 10` and `D9 18` now store a canonical binary80 Zero from physical
TOP, preserving its sign as binary32 `+0` or `-0`. The source must have both
the Zero tag and canonical zero payload; mismatched tags and nonzero or
pseudo-zero payloads remain pre-access Unsupported. The existing four FCWs,
`FSW & 0x80df` gate, PE preservation, and C1-clear policy are unchanged.
D910 preserves the full stack. D918 advances TOP and empties the old physical
TOP only after its one atomic Dword write succeeds; no data read is performed.

Coverage includes both signs across TOP×FCW×PE×C1, FLD-zero roundtrips,
literal output bytes, tag/payload mismatches, denied and architectural write
faults, and canonical48 range failure. Positive bare and negative DFE0/FNSTSW
fixtures cover each opcode through the CLI; JIT/native denial checks include
both forms. Focused interpreter, runner/schema, CLI, and JIT tests passed.
The full offline workspace suite passed (709 tests), as did
`cargo check --workspace --all-targets --offline`, Linux x86 `od-x86-runner`
all-target check, `cargo fmt --all -- --check`, and `git diff --check`.
Graphify rebuilt with 3956 nodes and 8254 edges. The layout checker reports
the existing unmapped `.serena/` directory, which remains untouched. The
architecture-gated native denial compiles for Linux x86 but was not executed
on this local arm64 host; physical x86 testing remains deferred to the
combined final S2-31 substage. Independent `gpt-6.1-sol` medium review was
clean; it verified zero eligibility, gates, atomic write order, FST/FSTP
status and stack handling, CLI fixture reachability, and native/JIT denial.

See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-31al-signed-zero-fst-m32fp-and-fstp-m32fp),
[plan](../plans/STAGE_2.md#s2-31al-store-signed-binary80-zero-through-bounded-m32-fstfstp),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).

## S2-31am — signed-zero stores for bounded m64 FST/FSTP

Exact `DD 10` and `DD 18` now store a canonical binary80 Zero from physical
TOP, preserving its sign as binary64 `+0` or `-0`. Both the Zero tag and the
canonical exponent-zero/significand-zero payload are required; a mismatched
tag or nonzero/pseudo-zero payload remains pre-access Unsupported. Existing
FCWs and `FSW & 0x80df` store gates remain fixed. Exact zero adds no flags,
preserves PE, and clears C1.

The full canonical48 Qword range is checked before one atomic Qword write;
there is no data read. DD10 preserves TOP, all tags and raw payloads. DD18
empties old physical TOP and advances TOP only after the write succeeds.
Normal rounding behavior and its local exponent limits remain unchanged.

Tests cover both signs across TOP×FCW×PE×C1, m32↔m64 FLD-zero compositions,
strict source/status rejection, denied and architectural write failures, and
the final canonical Qword boundary. Four literal positive-bare/negative
DFE0+FNSTSW fixtures run through schema/CLI; JIT and native admission stay
closed. Focused checks and the full offline workspace suite passed (715 tests).
`cargo check --workspace --all-targets --offline`, Linux x86
`od-x86-runner` all-target check, `cargo fmt --all -- --check`, and
`git diff --check` passed. Graphify rebuilt with 3972 nodes and 8280 edges.
The layout check reports the existing unmapped `.serena/` directory, which
remains untouched. The local arm64 host cannot execute the architecture-gated
native denial path; physical x86 validation remains deferred to the combined
final S2-31 substage.

See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-31am-signed-zero-fst-m64fp-and-fstp-m64fp),
[plan](../plans/STAGE_2.md#s2-31am-store-signed-binary80-zero-through-bounded-m64-fstfstp),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


## S2-31an — signed-zero binary80 stores through bounded FSTP m80fp

Exact bare `DB 38` now accepts a Zero tag only when the raw binary80 payload is
canonical zero (zero exponent and all 64 significand bits clear), alongside
the existing Valid-tagged canonical Normal source. It writes all ten bytes
unchanged, preserving the sign and representation. Existing FCW and
`FSW & 0x80df` gates remain; no status flags are added, PE and C0/C2/C3 are
preserved, and C1 clears. The one atomic ten-byte write occurs after complete
canonical48 preflight and without a data read; TOP/tag pop happens only after
success.

Coverage includes both signs across TOP×FCW×PE/C1, canaries, exact one-write
accounting, wrap, m32/m64/m80 load-to-store compositions, mismatched source
rejection, canonical-range and overflow atomicity, literal positive bare and
negative DFE0/FNSTSW fixtures, and native/JIT denial. The full offline workspace
suite passed (720 tests); workspace all-target checks, Linux x86 runner check,
formatting and diff checks passed. The layout checker reports the existing
unmapped `.serena/` directory, which remains untouched. Graphify was updated;
its final extraction reported no topology changes. The explicitly requested independent `gpt-6.1-sol` medium review was clean
after the report-link correction in commit `4c9aba19`.


## S2-31ao — signed-infinity binary80 store through bounded FSTP m80fp

Exact bare `DB 38` now also accepts a Special-tagged canonical binary80
infinity (`exp=0x7fff`, integer bit one, zero fraction), preserving all ten raw
bytes and sign. The existing Valid+Normal and Zero+Zero pairs remain.
Pseudo-infinity, NaNs, other classes, and mismatched tags reject before access.
The FCW/FSW gates, exact no-flag policy, one atomic ten-byte write/no read, and
post-success pop remain unchanged.

Coverage includes both signs across TOP×FCW×PE/C1, canaries, wrap, m32/m64/m80
load-to-store compositions, every forbidden FSW gate, source-class/tag rejects,
write/range atomicity, Normal/zero regressions, literal CLI/FNSTSW fixtures,
and native/JIT denial. The full offline workspace suite passed (725 tests);
workspace all-target checks, Linux x86 runner check, formatting, and diff checks
passed. Graphify was updated. The layout checker reports the existing unmapped
`.serena/` directory, left untouched. Physical x86 testing remains deferred to
the combined final S2-31 substage. Independent `gpt-6.1-sol` review at medium
reasoning found no concrete findings. The native admission test is excluded by
this host configuration;
native admission remains closed in the tested contract.


## S2-31ap — signed-infinity stores for bounded m64 FST/FSTP

Exact `DD 10` and `DD 18` now also accept physical TOP with Special tag and
canonical binary80 infinity, converting by integer fields to the exact signed
binary64 infinity bytes. Normal and signed-zero behavior remains unchanged. The
existing FCW and `FSW & 0x80df` gates stay fixed; infinity creates no flags,
preserves PE, and clears C1. FST preserves the source stack; FSTP pops only
after its successful Qword write.

Tests cover both signs and opcodes across TOP×FCW×PE/C1, sentinels, wrap,
invalid classes/tags and forbidden-status pre-access rejection, Qword write and
canonical-boundary atomicity, m32/m64/m80 FLD-to-store sequences, CLI/FNSTSW
fixtures, and native/JIT denial. The full offline workspace suite passed (730
tests); workspace all-target checks, Linux x86 runner all-target check,
formatting, diff, and local documentation-link checks passed. Graphify rebuilt
with 4005 nodes and 8332 edges. The layout checker reports the existing
unmapped `.serena/` directory, left untouched. Native admission tests are
architecture-gated and were not executed on this arm64 host; the x86 runner
target compiled successfully. Physical validation remains deferred to the
combined final S2-31 substage. Independent `gpt-6.1-sol` review at medium
reasoning found no concrete findings.


<a id="s2-31aq--signed-infinity-stores-for-bounded-m32-fstfstp"></a>

## S2-31aq — signed-infinity stores for bounded m32 FST/FSTP

Exact D910/D918 now accept only canonical binary80 infinity with the physical
TOP tagged Special, converting sign and exponent by integer fields to literal
binary32 infinity bytes. The path is exact, adds no flags, preserves PE, and
retains existing store status policy. A full Dword range check precedes one
atomic write/no read; D910 preserves stack state and D918 pops only after the
write succeeds. Normal and signed-zero stores are unchanged. Tests cover both
signs/opcodes across TOP×FCW×PE/C1, all-width FLD composition, malformed
neighbors and mismatched tags, atomic write/range failures, CLI/FNSTSW fixtures,
and native/JIT denial. Intel SDM revision 093 Vol.1 §8.2.2/Table 8-3 and Vol.2A
pp.3-386–3-387 support the bounded encoding and instruction contract.

Focused `od-isa` coverage passed (10 selected tests), including signed
TOP×FCW×PE/C1 matrices, class/tag and forbidden-status pre-access rejection,
fault/boundary atomicity, and m32/m64/m80 load-to-store compositions. The new
runner CLI test passed with the positive bare and negative DFE0/FNSTSW fixtures;
it exercises the normal fixture schema and execution path. The full offline
workspace suite passed (735 tests, 0 failures), all-target workspace check,
Linux x86 `od-x86-runner` all-target check, formatting, and diff checks passed.
Graphify rebuilt to 4016 nodes and 8349 edges. The layout checker reports only
the pre-existing unmapped `.serena/` directory, left untouched. Architecture-
gated native runtime denial was not executed on this local arm64 host; the
Linux x86 runner target compiled. Physical validation remains deferred to the
combined final S2-31 substage. Independent `gpt-6.1-sol` review at medium
reasoning found no remaining findings after the explicit report-anchor fix in
commit `79808a5`.


<a id="s2-31ar--canonical-quietnan-binary80-store-through-bounded-fstp-m80fp"></a>

## S2-31ar — canonical QuietNaN binary80 store through bounded FSTP m80fp

Exact DB38 now preserves the raw ten bytes of a canonical binary80 QuietNaN
only when physical TOP is tagged Special. It adds no flags, preserves PE and
the existing condition-code policy, and performs one atomic ten-byte write
with no data read before popping. Existing Normal, signed-zero, and infinity
paths remain. Tests cover both signs, negative indefinite, quiet-only/low/
middle/maximum payloads across TOP×FCW×PE/C1, cross-width FLD composition,
neighbor-class/tag rejection, write/range atomicity, CLI/FNSTSW fixtures, and
native/JIT denial. Intel SDM Vol.1 Table 4-3 and revision 093 Vol.2A
pp.3-386–3-387 support the narrow value and instruction contract.

Focused `od-isa` store matrices, source/tag rejection, write/range atomicity,
and cross-width composition tests passed. The runner CLI/FNSTSW fixture test and
JIT-denial test passed. The full offline workspace suite passed (740 tests,
0 failures); workspace all-target check, Linux x86 runner all-target check,
formatting, and diff checks passed. Graphify update completed successfully
with no topology changes. The layout checker reports the pre-existing unmapped
`.serena/` directory, left untouched. Architecture-gated native runtime denial
was not executed on this arm64 host; the Linux x86 runner target compiled.
Physical validation remains deferred to the combined final S2-31 substage.
Independent `gpt-6.1-sol` medium review found no code or coverage findings. It
identified a plan wording issue about C1; the plan now correctly states that
C1 clears while PE and C0/C2/C3 are preserved.

<a id="s2-31as--canonical-signalingnan-binary80-store-through-bounded-fstp-m80fp"></a>

## S2-31as — canonical SignalingNaN binary80 store through bounded FSTP m80fp

Exact DB38 now preserves the full ten-byte encoding of a canonical binary80
SignalingNaN only when physical TOP is tagged Special. The signaling bit,
sign, and payload remain unchanged; the store adds no IE or other flags. It
retains the existing environment gates, atomic one-write/no-read behavior,
and pop-after-success semantics. Existing Normal, Zero, Infinity, and
QuietNaN paths remain supported; pseudo-SNaNs and mismatched tags reject
before access.

The focused `od-isa` SNaN matrix, raw m80 composition, pre-access rejection,
and write-fault atomicity tests passed. The positive bare and negative
DFE0/FNSTSW fixtures pass the runner CLI and JIT-denial tests. The full offline
workspace suite passed (745 tests, 0 failures); workspace all-target check,
Linux x86 runner all-target check, formatting, and diff checks passed.
Graphify updated to 4042 nodes and 8386 edges. The layout checker reports the
pre-existing unmapped `.serena/` directory, left untouched. Architecture-gated
native runtime denial was not executed on this arm64 host; the Linux x86 runner
target compiled. Physical validation remains deferred to the combined final
S2-31 substage. Independent `gpt-6.1-sol` medium review of commit `52dc10c`
found no concrete correctness, regression, contract, or fixture-reachability
findings. Targeted verification passed 13 m80 store tests, one SNaN load/store
composition test, one CLI fixture test, and two relevant JIT-denial tests. The
Intel FST/FSTP entry explicitly exempts double extended-precision stores from
the SNaN invalid-operation exception ([Intel SDM revision 093, Vol.2A, p.3-387,
official PDF p.505](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=505)).

<a id="s2-31at--canonical-subnormal-binary80-store-through-bounded-fstp-m80fp"></a>

## S2-31at — canonical subnormal binary80 store through bounded FSTP m80fp

Exact DB38 now also preserves the full ten-byte encoding of canonical binary80
subnormals only when physical TOP is tagged Special. Sign and fraction remain
unchanged; the store adds no DE/UE/PE or other flags. It retains the existing
environment gates, atomic one-write/no-read behavior, and pop-after-success
semantics. Existing Normal, Zero, Infinity, QuietNaN, and SignalingNaN paths
remain supported; pseudo-denormals and mismatched tags reject before access.

The focused `od-isa` subnormal matrix, m80 FLD-to-store composition,
pre-access class/tag rejection, and write/range atomicity tests passed. The
positive bare and negative DFE0/FNSTSW fixtures pass the runner CLI and JIT-
denial tests. The full offline workspace suite passed (750 tests, 0 failures);
workspace all-target check, Linux x86 runner all-target check, formatting, and
diff checks passed. Graphify updated to 4055 nodes and 8404 edges. The layout
checker reports the pre-existing unmapped `.serena/` directory, left untouched.
Independent `gpt-6.1-sol` medium review of commit `40fbc43` found no concrete
findings. The independent review run passed 10 `od-isa` subnormal tests and
four runner tests; positive fixtures exercised the interpreter and JIT/native
denial paths. Native runtime denial was not executed on this arm64 host; the
Linux x86 runner target compiled. Physical validation remains pending for the
combined final S2-31 substage.


<a id="s2-31au--canonical-quietnan-binary80-to-binary64-stores"></a>

## S2-31au — canonical QuietNaN binary80 to binary64 stores

Exact DD10/DD18 now accept Special-tagged canonical binary80 QuietNaNs and
convert by integer fields, preserving sign and the upper 52 fraction bits while
truncating the lower 11. The path adds no flags, preserves PE and clears C1.
Existing FCW/FSW gates, one-write/no-read Qword behavior, DD10 preservation and
post-success DD18 pop remain unchanged. SNaNs and malformed/tag-mismatched
classes reject before access; Normal, zero, infinity and native/JIT gates stay
unchanged.

Focused verification passed: conversion matrices across both opcodes, signs, all
TOPs and FCWs, PE/C1, and quiet-only/low/intermediate/maximum payloads;
cross-width FLD-to-store composition; rejection and Qword fault/range atomicity;
positive bare and negative DFE0/FNSTSW CLI fixtures; and JIT-denial fixtures.
The full offline workspace suite passed (755 tests, 0 failures), workspace
all-target check passed, Linux x86_64 runner all-target check passed, and
formatting passed. Graphify update completed (4068 nodes, 8423 edges, 207
communities). The layout check reports the pre-existing unmapped `.serena/`
directory, left untouched. Independent `gpt-6.1-sol` medium review of `e3f4257` was clean. The reviewer
confirmed the narrow Special-tagged canonical QuietNaN admission, exact payload
truncation, unchanged gates and store/pop atomicity, with no fallback. Independent
evidence comprised 17 targeted tests: 14 m64fp tests, cross-width FLD-to-FST
composition, interpreter fixture execution, and JIT denial. Native runtime and
physical behavior were not verified on this macOS arm64 host and remain recorded
under ISSUE-019; physical acceptance is deferred to the final combined S2-31
substage.


<a id="s2-31av--canonical-quietnan-binary80-to-binary32-stores"></a>

## S2-31av — canonical QuietNaN binary80 to binary32 stores

Exact D910/D918 now accept Special-tagged canonical binary80 QuietNaNs and
convert by integer fields, preserving sign and the upper 23 fraction bits while
truncating the lower 40. The path adds no flags, preserves PE, and clears C1.
Existing FCW/FSW gates, one-write/no-read Dword access, D910 preservation and
post-success D918 pop remain fixed. SNaNs and malformed/tag-mismatched classes
reject before access; Normal, zero, infinity and native/JIT gates are unchanged.

Focused QNaN conversion, cross-width FLD-to-store composition, rejection,
fault/range atomicity, CLI/FNSTSW fixture, and JIT-denial tests passed. The full
offline workspace suite passed (760 tests, zero failures); workspace all-target
check and Linux x86_64 runner all-target check passed. Formatting, diff and
fixture-link checks passed. Graphify updated successfully (4,081 nodes, 8,442
edges, 174 communities); the layout checker reports only the pre-existing
unmapped `.serena/` directory, which was left untouched. Native runtime and
physical behavior remain unverified on this arm64 host and are deferred to the
final combined acceptance substage (ISSUE-019). Independent Sol-medium review
of commit `57f93d5` was clean: six QuietNaN ISA tests plus CLI fixture and JIT
denial tests passed. The review confirmed source-class/tag gates, canonical48
Dword prevalidation, and that the write precedes status and pop changes. Native
runtime was not verified; physical acceptance remains deferred under ISSUE-019.


<a id="s2-31aw--canonical-signalingnan-binary80-to-binary64-stores"></a>

## S2-31aw — canonical SignalingNaN binary80 stores through m64

Exact DD10/DD18 now accept only a Special-tagged canonical binary80 SNaN. The
binary64 result preserves sign and the upper 51 payload bits, sets the quiet
bit, and truncates the lower 11 bits without PE. Sticky IE is committed only
after the atomic Qword write succeeds; PE is preserved, ES/B/SF remain clear,
the raw source remains unchanged, and DD18 pops only after the write. FCW/FSW
gates and normal/zero/infinity/QNaN paths are unchanged.

Six focused test functions passed: the signed payload matrix, fault/range
atomicity, IE plus FNCLEX recovery, malformed-class/tag/environment rejection,
CLI/FNSTSW fixtures, and JIT denial. The full offline workspace suite passed
(765 tests, zero failures); workspace all-target check and Linux x86_64 runner
all-target check passed. Formatting, diff and fixture-link checks passed.
Graphify rebuilt successfully (4,094 nodes, 8,461 edges, 208 communities).
The layout check reports only the pre-existing unmapped `.serena/` directory,
which was left untouched. Native runtime and physical behavior remain
unverified on this arm64 host and are deferred to the final combined acceptance
substage under ISSUE-019. Independent gpt-6.1-sol medium review of
`50bbd25` was clean: no concrete correctness, regression, contract, or
fixture-reachability findings. Review evidence confirmed the canonical
class/tag gate, quieting without PE, unchanged source, IE commit only after a
successful full-range atomic write, and correct FST/FSTP state transitions
under the existing IE/FNCLEX gate. The independent review exercised 17 m64
ISA tests, the CLI fixture, and JIT denial. Native runtime was not executed on
the arm64 host; physical validation remains deferred under ISSUE-019.


<a id="s2-31ax--canonical-signalingnan-binary80-to-binary32-stores"></a>

## S2-31ax — canonical SignalingNaN binary80 to binary32 stores

Exact D910/D918 now accept Special-tagged canonical binary80 SNaNs and convert
by integer fields to quiet binary32 values. The sign and upper 22 payload bits
are retained, lower 40 bits truncate without rounding or PE, and sticky IE is
committed only after a successful atomic Dword write. PE and all ten source
bytes are preserved; D910 keeps TOP/tags and D918 pops only after success. The
existing four FCWs and `FSW & 0x80df == 0` gate remain, so a later store is
blocked until FNCLEX clears IE at this stage. S2-31ay below supersedes that
restriction for the exact m32/m64 FST/FSTP conversion helpers. QNaN conversion
remains a no-new-flags path.

Focused coverage includes both signs/opcodes over TOP×FCW×PE/C1 and payload
bits 39/40, QNaN/SNaN status differentiation, write/range atomicity, and
FNSTSW/FNCLEX recovery. The focused m32 ISA suite passed (14 tests), the runner
m32 unit/CLI batch passed (4 tests), and fixture/JIT denial passed (2 tests).
Positive D910 and negative D918/DFE0 fixtures run through the CLI; both are
explicitly denied by JIT/native admission. The full offline workspace suite
passed (769 tests, zero failures); workspace all-target check, Linux x86_64
runner all-target check, formatting, and diff checks passed. Graphify updated
successfully. The layout check reports only the pre-existing unmapped
`.serena/` directory, which was left untouched. Independent review is pending.
Native runtime and physical acceptance remain deferred under ISSUE-019.


<a id="s2-31ay--sticky-ie-m32-m64-stores"></a>

## S2-31ay — sticky IE in bounded m32/m64 stores

The exact D910/D918 and DD10/DD18 conversion helpers now admit masked sticky IE
by changing only their FSW predicate from `0x80df` to `0x80de`. ZE/DE/OE/UE/SF/ES/B
remain rejected; FCWs, source classes/tags, address validation, memory shape,
and all unrelated x87 gates remain unchanged. Successful SNaN stores preserve
IE and PE and OR IE after the atomic write; inexact finite stores similarly
OR PE after success. FSTP still pops only after the write, and repeated stores
can proceed with IE set. `FNSTSW` observes sticky IE and `FNCLEX` remains the
way to clear status, but no longer needs to run between otherwise admitted
m32/m64 stores.

Validation passed locally: the od-isa suite passed 443 tests, both CLI
fixture tests passed, both m32/m64 JIT-denial integration tests passed, and the
full offline workspace suite passed 771 tests with zero failures. Workspace
all-target checking and Linux x86_64 runner all-target checking passed;
formatting and diff checks passed. Graphify rebuilt successfully (4,116 nodes,
8,491 edges, 186 communities). The layout check reports only the pre-existing
unmapped `.serena/` directory, which was left untouched. The Linux runner check
reports existing unused calibration-helper warnings. Independent review is
pending. Native runtime and physical acceptance remain deferred under
ISSUE-019.


<a id="s2-31az--sticky-de-in-bounded-m32-m64-stores"></a>

## S2-31az — sticky DE in bounded m32/m64 stores

The exact D910/D918 and DD10/DD18 conversion helpers now admit sticky masked
DE by changing only their local status mask from `0x80de` to `0x80dc`. IE, DE,
and PE are preserved; SNaN conversion adds IE and inexact finite conversion
adds PE only after a successful full-range atomic write. The store path does
not generate DE. ZE/OE/UE/SF/ES/B, FCWs, source classes/tags, finite-result
limits and all unrelated x87 gates remain unchanged. Store results that would
be binary32/64 subnormals continue to reject before access.

Coverage includes all eight incoming IE/DE/PE combinations, TOP×FCW and source
classes, forbidden-status controls, fault atomicity, FNSTSW/FNCLEX, repeated
stores, and binary32 subnormal FLD followed by binary64 FST/FSTP for both signs.
Two literal sequence fixtures execute through the runner's interpreter path
and are explicitly denied by JIT without fallback. The native-admission unit
test now checks both fixtures on the x86 runner target; it cross-compiles on
this arm64 host but cannot execute here. Focused ISA, fixture, schema, and JIT
checks passed. The full offline workspace suite passed 775 tests with zero
failures; workspace all-target checking and Linux x86_64 runner all-target
checking passed. Formatting and diff checks passed. Graphify updated
successfully (4,129 nodes, 8,510 edges, 210 communities). The layout checker
reports only the pre-existing unmapped `.serena/` directory, which was left
untouched. Independent read-only Sol-medium review found no blocking correctness,
atomicity or scope defects. Native runtime and physical
acceptance remain deferred to the final combined S2-31 substage under
ISSUE-019.


<a id="s2-31ba--sticky-ie-de-on-raw-binary80-fstp-m80fp"></a>

## S2-31ba — sticky IE/DE on raw binary80 FSTP m80fp

Only the exact DB38 raw-store helper now admits masked sticky IE and DE
alongside PE, by changing its local `FSW & 0x80df == 0` predicate to
`FSW & 0x80dc == 0`. The four FCWs, accepted tag/class pairs, full canonical48
range check, one atomic ten-byte write and post-success pop remain unchanged.
The operation copies raw source bytes exactly, adds no status flags, preserves
IE/DE/PE and C0/C2/C3, and clears C1. ZE/OE/UE/SF/ES/B and malformed class/tag
pairs still reject before data access.

Unit coverage exercises all eight incoming IE/DE/PE combinations across every
accepted class/sign, FCW and TOP, plus forbidden bits and full-state write/range
failure atomicity. Cross-width tests cover binary32/64 subnormal loads followed
by DB38 with DE and no UE/PE, binary32/64 SNaN loads followed by raw stores with
quieted payload and IE, and raw m80 SNaN load→m32 FST→DB38 preserving the
signaling source bytes with IE. Two literal fixtures run through the
interpreter; explicit JIT and native-admission denial are checked. The clean
independent review passed 17 FSTP m80 tests and 8 raw-m80 composition tests; its
runner test timed out, while the parent validation passed the full workspace
and Linux x86_64 checks below. Focused
ISA/CLI/JIT checks passed. The offline workspace suite passed 782 tests with
zero failures; workspace all-target checking and Linux x86_64 all-target checks
for `od-runner` and `od-x86-runner` passed. Formatting and diff checks passed.
Graphify updated successfully (4,143 nodes, 8,532 edges, 209 communities). The
layout checker reports only the pre-existing
unmapped `.serena/` directory, which was left untouched. Independent review is
clean. Native runtime and physical acceptance remain deferred under ISSUE-019.


<a id="s2-31bb--finite-exponent-127-bounded-m32-stores"></a>

## S2-31bb — finite exponent 127 for bounded binary32 stores

Exact D910/D918 finite m32 stores now accept canonical Normal binary80 sources
through unbiased exponent 127. Existing integer rounding is retained and a
significand carry is normalized before the final exponent check, so carry from
126 to 127 is accepted while carry to 128 and an input at exponent 128 reject
before memory access without setting OE. Minimum exponent, subnormal-result
rejection, status/control gates, source classes, addressing and atomic store/pop
semantics remain unchanged.

Separate maximum-finite FLD and store fixtures exercise the interpreter/schema
path; explicit JIT denial remains covered. Validation passed: 16 focused
`od-isa` m32 store tests, the CLI fixture test, JIT-denial fixture test, 786
workspace tests, workspace `cargo check`, Linux x86_64 `cargo check
--all-targets`, and `cargo fmt --all -- --check`. Linux x86 test binaries could
not be linked on this macOS host; `tools/check_layout.py` reports the pre-existing
unmapped `.serena` root, which was left untouched. Native runtime and physical
acceptance remain deferred under ISSUE-019.

Independent gpt-6.1-sol medium review of commit `aaf0d6f` was clean. Reviewer
verification covered 26 m32fp ISA cases, the maximum-finite interpreter fixture,
JIT denial and the Linux x86 target check. Native runtime and physical behavior
remain unverified and deferred under ISSUE-019.


<a id="s2-31bc--finite-exponent-1023-bounded-m64-stores"></a>

## S2-31bc — finite exponent 1023 for bounded binary64 stores

Exact DD10/DD18 finite m64 stores now accept canonical Normal binary80 sources
through unbiased exponent 1023. Integer significand rounding normalizes a carry
before the final check, so 1022→1023 is supported while 1023→1024 and an
exponent-1024 source reject before memory access without setting OE. The
minimum exponent, subnormal-result rejection, environment gates, source
classes, and atomic store/pop behavior remain unchanged.

Separate maximum-finite FLD/store fixtures, TOP×FCW signed rounding vectors,
sticky IE/DE/PE combinations and write/range fault tests are covered here.
Validation passed: 31 focused `od-isa` m64fp tests, the CLI fixture test, the
JIT-denial fixture test, 790 workspace tests, workspace `cargo check`, Linux
x86_64 `cargo check --all-targets`, and `cargo fmt --all -- --check`. Linux
x86 test binaries could not be linked on this macOS host; layout checking still
reports the pre-existing unmapped `.serena` root, left untouched. Native runtime
and physical acceptance remain deferred under ISSUE-019.




Independent gpt-6.1-sol medium review of commit `3d29eb1` was clean. Reviewer
verification covered 31 m64fp ISA cases, the maximum-finite CLI fixtures and
JIT denial. Linux x86_64 target compilation passed; native runtime and physical
behavior remain unverified and deferred under ISSUE-019. Executable Linux x86
tests could not be linked on this macOS host.


<a id="s2-31bd--exact-binary32-subnormal-results-through-m32-stores"></a>

## S2-31bd — exact binary32 subnormal results through bounded m32 stores

Exact D910/D918 stores now pack exactly representable binary32 subnormal
results from canonical Normal binary80 sources with exponents -149 through
-127. The integer downshift is independent of RC and requires every discarded
bit to be zero. Inexact tiny results and values below the minimum subnormal
remain Unsupported before memory access. No new DE, UE or PE is generated;
sticky status, gates, normal rounding, addressing and store/pop ordering remain
unchanged.

Boundary vectors, D900 FLD→D910 composition, two CLI fixtures including DFE0
FNSTSW, JIT/native denial, and write/range atomicity are covered. Focused
verification passed 28 `od-isa` m32fp tests plus composition, CLI and JIT
denial tests. The full offline workspace suite passed 795 tests; workspace
all-target checks, Linux x86_64 runner cross-check and formatting passed.
The layout checker reports the existing unmapped `.serena/` directory, which
was left untouched. Native runtime and physical acceptance remain deferred
under ISSUE-019.


<a id="s2-31be--exact-binary64-subnormal-results-through-m64-stores"></a>

## S2-31be — exact binary64 subnormal results through bounded m64 stores

Exact DD10/DD18 stores now pack exactly representable binary64 subnormal
results from canonical Normal binary80 sources with exponents -1074 through
-1023. The integer downshift is independent of RC and requires every
discarded bit to be zero. Inexact tiny results and values below the minimum
subnormal remain Unsupported before memory access. No new DE, UE or PE is
generated; sticky status, gates, normal conversion, addressing and store/pop
ordering remain unchanged.

Coverage includes signed minimum/adjacent/maximum subnormal values, nontrivial
fractions, all shifts and their low-bit rejection neighbors, 1.5 minimum
subnormal, half-step above maximum subnormal, exponent -1075 and minimum normal.
DD00 FLD→DD10/DD18 composition proves DE retention without UE/PE. Signed CLI
fixtures include bare FST and FSTP with FNSTSW; JIT/native denial and write/
range-fault atomicity are covered. `cargo test --workspace --offline` passed
(801 tests); workspace and Linux x86_64 all-target checks and
`cargo fmt --all -- --check` passed. The Linux-only native admission test was
compile-checked but not executed on this macOS host. `tools/check_layout.py`
reports the pre-existing unmapped `.serena/` directory, which was preserved;
`git diff --check` passed. Independent gpt-6.1-sol review was clean: the 34
focused m64fp ISA tests, FLD/store roundtrip, CLI fixtures and JIT denial were
reviewed; native and physical runtime remain deferred under ISSUE-019.


<a id="s2-31bf--masked-inexact-binary32-subnormal-results-through-m32-stores"></a>

## S2-31bf — masked inexact binary32 subnormal results through bounded m32 stores

The bounded D9 10/D9 18 m32 store path now rounds canonical Normal binary80
sources with exponents -149 through -127 using integer quotient/remainder and
the sign-aware FCW mode. It accepts only rounded nonzero binary32 subnormal
fractions. Sources below -149, results rounded to zero, and promotion to
minimum normal remain Unsupported before access. Exact subnormal results retain
their previous status behavior.

Accepted inexact tiny results add sticky UE+PE (`0x30`) and no DE after the
atomic Dword write; C1 records a magnitude increment. Pointer updates, flags
and FSTP pop remain uncommitted on write or range faults. Coverage includes
1.5/2.5 minimum-subnormal ties and neighbors, every shift 41..63, signed RC
matrices, maximum-subnormal directed rounding and promotion rejection,
incoming IE/DE/PE, UE gate and class/tag controls, and FNSTSW/FNCLEX behavior.
Signed literal CLI fixtures expose UE+PE; JIT/native admission remains closed.

Validation passed: 21 focused m32fp ISA tests, the CLI fixture test, the
JIT-denial test, and the full `cargo test --workspace --offline` run (806 passed,
0 failed). Workspace and Linux x86_64 all-target checks, formatting, and diff
checks passed. The independent reviewer's separate broad runner filter timed out
after its CLI unit test; that timeout is distinct from the passing implementation
suite reported above.
`graphify update .` completed its AST-only pass and found no code-graph topology
changes. The Linux-native admission test was compile-checked but not executed
on this macOS host. The layout checker reports the pre-existing
unmapped `.serena/` directory, preserved. Independent gpt-6.1-sol review was
clean, covering the focused ISA, CLI, and JIT-denial evidence. Native runtime
and physical acceptance remain deferred under ISSUE-019.


<a id="s2-31bg--masked-inexact-binary64-subnormal-results-through-m64-stores"></a>

## S2-31bg — masked inexact binary64 subnormal results through bounded m64 stores

Only `DD 10` and `DD 18` now round Valid-tagged canonical Normal binary80
sources with exponents -1074 through -1023 using integer quotient/remainder,
shift 12 through 63, and sign-aware FCW rounding. Nonzero subnormal results are
accepted; zero, below-minimum inputs, and promotion to minimum normal remain
Unsupported before access. Exact subnormal behavior is unchanged.

Accepted inexact stores add sticky UE+PE (`0x0030`), no DE, and C1 only when
rounding increases magnitude. The FCW whitelist and `FSW & 0x80dc == 0` gate
remain in force, including pre-existing UE rejection. Status/pointers/RIP and
FSTP pop commit only after the atomic Qword write. Coverage includes tie and
neighbor vectors, every shift, both opcodes/all TOPs/all incoming IE/DE/PE
combinations, upper-bound rejection, below-range/UE rejection, FNSTSW/FNCLEX,
fault atomicity, CLI fixtures, and JIT/native denial.

Validation passed: 25 focused m64fp ISA tests, the CLI fixture and JIT-denial
tests, and the full offline workspace suite (811 passed, 0 failed). Workspace
and Linux x86_64 all-target checks, formatting, and diff checks passed.
`graphify update .` completed its AST-only pass and rebuilt the code graph
(4216 nodes, 8649 edges, 172 communities). The Linux-native admission test was
compile-checked but not executed on this macOS host. The layout checker reports
the pre-existing unmapped `.serena/` directory, preserved. Independent
gpt-6.1-sol medium review was clean, covering the 25 focused ISA cases, two CLI
subnormal fixtures, and one JIT-denial test. Native runtime and physical
acceptance remain deferred under ISSUE-019.


<a id="s2-31bh--sticky-ue-reentry-for-bounded-m32-m64-stores"></a>

## S2-31bh — allow sticky UE in bounded binary32/binary64 stores

The bounded m32/m64 result helpers now use `FSW & 0x80cc`, admitting masked
sticky UE alongside IE/DE/PE for only D9 10/D9 18/DD 10/DD 18. The four FCWs,
FLD and m80 gates, source classes, and conversion rules remain unchanged. UE
alone does not add PE. Conversion-generated status and FSTP/TOP/pointer updates
remain deferred until after a successful atomic data write.

Coverage now spans all 16 IE/DE/UE/PE combinations for admitted classes and
exact/inexact paths, repeated masked underflow, exact m64 storage after m32
underflow from the unchanged source, FNSTSW/FNCLEX, write/range atomicity,
forbidden status/control values, and CLI/JIT/native denial.

Validation passed: all 468 `od-isa` unit tests; the sticky-UE CLI fixture;
the JIT-denial fixture test; and the full offline workspace suite (813 passed,
0 failed). `cargo fmt --all -- --check`, host workspace all-target checks, and
Linux x86_64 workspace all-target checks passed. Linux x86_64 native-runner
unit-test code passed `cargo check -p od-x86-runner --tests --target
x86_64-unknown-linux-gnu --offline`. The Linux-target test binary could not be
linked or executed here: the macOS linker rejects Linux `--as-needed`,
`-Bstatic`, and `-z` options. `graphify update .` completed its AST-only update
(4224 nodes, 8660 edges, 179 communities). The layout checker reports only the
pre-existing unmapped `.serena/` directory, preserved; `git diff --check` and
changed-document anchor checks passed. Native runtime and physical acceptance
remain deferred under ISSUE-019. Independent gpt-6.1-sol medium review was
clean; it verified all 468 `od-isa` tests, the CLI fixture group, sticky-UE JIT
denial, and the Linux native-runner test typecheck. The linker failure above was
reproduced on macOS when attempting to link the Linux test binary; native test
execution remains unavailable here. Physical acceptance is still deferred.


<a id="s2-31bi--sticky-ue-on-raw-binary80-fstp-m80fp"></a>

## S2-31bi — allow sticky UE on raw binary80 FSTP m80fp

The bounded `DB 38` helper now uses `FSW & 0x80cc`, admitting incoming masked
UE alongside IE/DE/PE while continuing to reject ZE/OE/SF/ES/B. FCWs, accepted
tag/class pairs, raw ten-byte source preservation, and no-generated-flags
behavior remain unchanged. UE alone does not add IE or PE; signaling NaNs remain
raw. Status, pointers, RIP, and pop commit only after the atomic ten-byte write.

Coverage includes all 16 IE/DE/UE/PE combinations across accepted classes,
signs, FCWs, and TOP; UE-only signaling-NaN preservation; m32/m64 underflow
followed by raw m80 FSTP from the unchanged source; FNSTSW/FNCLEX; forbidden
status/control gates; write/range atomicity; and CLI/JIT/native denial.

Validation passed: all 469 `od-isa` unit tests and the full offline workspace
suite (814 passed, 0 failed). The UE-only CLI fixture group and JIT-denial
fixture test passed. Host workspace all-target checks and Linux x86_64 workspace
all-target checks passed, as did Linux x86_64 native-runner test-code checking
with `cargo check -p od-x86-runner --tests --target x86_64-unknown-linux-gnu
--offline`. `cargo fmt --all -- --check` and `git diff --check` passed. `graphify update .`
completed its AST-only pass and rebuilt the graph (4230 nodes, 8668 edges, 215
communities). The Linux-only native denial test is typechecked but not executed
on this macOS host; physical acceptance remains deferred under ISSUE-019. The
layout checker reports the pre-existing unmapped `.serena/` directory,
preserved. Independent Sol-medium review was clean: the 12,288-case sticky-flag
matrix, 17 raw m80 ISA cases, CLI fixture group, JIT denial, and native test-code
check were accepted; native runtime and physical acceptance remain deferred.


<a id="s2-31bj--extreme-binary32-underflow-to-zero-or-minimum-subnormal"></a>

## S2-31bj — round extreme binary32 underflow to zero or minimum subnormal

The bounded D9 10/D9 18 conversion now accepts canonical Normal binary80
exponents -16382 through -150 and rounds directly to signed zero or signed
minimum subnormal under RC. Exponent -150 uses an explicit significand
comparison at the exact half-way point, avoiding oversized shifts. Accepted
results always add sticky UE+PE after successful atomic Dword storage; C1 tracks
selection of the minimum subnormal, including sign-preserving zero results.
Exact conversion behavior, source bytes, FCW/FSW gates and atomic state/pop
commit are preserved.

Coverage includes below/equal/above half, lowest exponent, both signs and RCs,
all 16 sticky IE/DE/UE/PE states, both opcodes/TOPs, signed zero, exact minimum
control, FNSTSW/FNCLEX, m32-to-raw-m80 composition, write/range atomicity,
forbidden source/gate cases, and CLI/JIT/native denial. Validation passed: the
full offline workspace suite (815 passed, 0 failed), including all 470 `od-isa`
tests; the literal CLI fixture group (50 passed); and the schema-v2 JIT-denial
group (110 passed). Host workspace and Linux x86_64 workspace all-target checks
passed, as did Linux x86_64 native-runner test-code typechecking. Formatting,
fixture JSON, documentation anchors, and `git diff --check` passed.
`graphify update .` completed with 4236 nodes, 8676 edges, and 192 communities.
The layout checker reports only the pre-existing unmapped `.serena/` directory,
preserved. Native runtime and physical acceptance remain deferred under
ISSUE-019. Independent Sol-medium review was clean, including all 470 `od-isa`
tests, CLI and JIT checks, and 519,456 exact-integer rounding oracle combinations; native runtime
and physical acceptance remain deferred under ISSUE-019.


<a id="s2-31bk--extreme-binary64-underflow-to-zero-or-minimum-subnormal"></a>

## S2-31bk — round extreme binary64 underflow to zero or minimum subnormal

The bounded DD 10/DD 18 conversion now accepts canonical Normal binary80
exponents -16382 through -1075 and rounds directly to signed zero or signed
minimum subnormal under RC. Exponent -1075 uses an explicit significand
comparison at the exact half-way point, avoiding oversized shifts. Accepted
results always add sticky UE+PE after successful atomic Qword storage; C1 tracks
selection of the minimum subnormal. Exact conversion behavior, source bytes,
FCW/FSW gates and atomic state/pop commit are preserved.

Coverage includes below/equal/above half, lowest exponent, both signs and RCs,
all 16 sticky IE/DE/UE/PE states, both opcodes/TOPs, signed zero, exact minimum
control, FNSTSW/FNCLEX, m64-to-raw-m80 composition, write/range atomicity,
forbidden source/gate cases, and CLI/JIT/native denial. Validation passed: the
full offline workspace suite (816 passed, 0 failed), including all 471 `od-isa`
tests; the CLI fixture group (50 passed); and schema-v2 fixture/JIT denial
tests (110 passed). Host and Linux x86_64 workspace all-target checks passed,
as did Linux x86_64 native-runner test-code typechecking. Formatting, fixture
JSON, documentation anchors, and `git diff --check` passed. `graphify update .`
completed with 4242 nodes, 8684 edges, and 181 communities. The layout checker
reports only the pre-existing unmapped `.serena/` directory, preserved. A clean
independent review confirmed the 471 ISA tests, CLI fixture, and JIT denial;
the exact-integer rounding oracle covered 489,856 combinations. Native runtime
and physical acceptance remain deferred under ISSUE-019.

## S2-31bl — binary80 subnormal to binary32 underflow rounding

The bounded D9 10/D9 18 conversion accepts Special-tagged canonical binary80
Subnormal inputs (E=0, J=0, nonzero fraction) and selects signed zero or signed
minimum binary32 subnormal using sign and RC. Every accepted conversion is
inexact and commits UE+PE after atomic storage; C1 records minimum-subnormal
selection. The raw source remains unchanged. Coverage includes four payloads,
both signs, all four FCWs, both opcodes, every TOP, all 16 sticky IE/DE/UE/PE
combinations, invalid encodings/tags, C1 replacement, zero/minimum-normal
controls, raw-m80 preservation, FNSTSW/FNCLEX, and write/range atomicity. The
interpreter CLI fixture is explicitly denied by JIT and native admission.

Validation passed: the full offline workspace suite (820 passed, 0 failed),
including 475 `od-isa` tests; the CLI unit group (50 passed); and all 110
schema-v2 fixture/JIT tests. Host and Linux x86_64 all-target workspace checks
passed, as did Linux x86_64 native-runner test-code typechecking. Formatting,
fixture JSON, documentation anchors, and `git diff --check` passed. The
`graphify update .` AST refresh completed with 4252 nodes, 8703 edges, and 171
communities. Layout checking reports only the preserved unmapped
`.serena/` directory. Independent Sol-medium review was clean. The review
confirmed 25 focused `fst_m32fp` ISA tests, the FLD→FST→FNCLEX→FSTP raw-m80
composition, CLI/FNSTSW fixture coverage, and JIT denial. Native runtime and
physical acceptance remain deferred under ISSUE-019.

<a id="s2-31bm--binary80-subnormal-to-binary64-underflow-rounding"></a>

## S2-31bm — binary80 subnormal to binary64 underflow rounding

The bounded DD 10/DD 18 conversion now accepts only Special-tagged canonical
binary80 Subnormal sources (E=0, J=0, nonzero fraction). These values are below
half the minimum binary64 subnormal. Existing FCWs select signed zero or the
signed minimum subnormal by sign and directed rounding. Every accepted
conversion adds UE+PE and no DE after the Qword write; C1 is set exactly when
minimum subnormal is selected. The raw source remains unchanged, and
status/pointer/RIP/FSTP-pop state commits only after a successful write.

The ISA matrix covers fractions 1, 2, `0x4000000000000000`, and
`0x7fffffffffffffff`, both signs, all four FCWs, both opcodes, every TOP, and
all 16 sticky IE/DE/UE/PE combinations. Rejection and failure tests cover bad
tags/classes/FCWs/FSWs, write and canonical-range failures, FNSTSW/FNCLEX, and
FLD m80 → FST m64 → FNCLEX → raw FSTP m80 source preservation. Literal
interpreter fixtures observe a minimum-subnormal FST with C1/UE/PE and a
negative-zero FSTP with the updated TOP. The fixtures are denied by JIT and the
native-admission test records the corresponding denial.

Validation passed: the full offline workspace suite (825 passed, 0 failed),
including 478 `od-isa` tests, 51 runner unit tests and 111 schema-v2 fixture
checks; focused m64 x87 coverage (28 passed); host and Linux x86_64 all-target
workspace checks; formatting and fixture validation. `tools/check_layout.py` reports the existing
unmapped `.serena/` directory, which was preserved. The Linux x86_64 check
typechecks the native-admission test; it was not executed on this Apple Silicon
host. Native runtime and physical acceptance remain deferred under ISSUE-019.
`graphify update .` rebuilt the graph with 4264 nodes, 8724 edges, and 188
communities.

<a id="s2-31bn--binary80-normal-to-binary32-minimum-normal-rounding"></a>

## S2-31bn — binary80 Normal to binary32 minimum-normal rounding

The bounded D910/D918 path now admits Valid-tagged canonical Normal binary80
sources at exponent -127 when direct destination rounding of `S/2^41` yields
exactly binary32 minimum normal. It stores the destination-rounded value
directly, adds PE+C1, and determines UE independently by rounding the
unbounded 24-bit significand from `S/2^40` with the same sign-aware RC. UE is
added only when that rounded significand remains below `1<<24`; prior UE remains
sticky otherwise. The other m32/m64 conversion behavior and existing FCW/FSW
gates are unchanged. Architectural state and FSTP pop commit only after the
Dword write succeeds.

The exact-integer oracle and TOP×RC×sign×opcode×sticky matrix cover the nearest
boundary, directed-rounding boundary, neighbors, maximum-subnormal and
minimum-normal controls, and the distinct wide-significand UE threshold. The
ISA tests include Dword write/range atomicity and FNSTSW/FNCLEX composition.
Literal positive FST and negative FSTP fixtures observe status through FNSTSW;
the latter demonstrates minimum normal with PE+C1 and no newly raised UE. JIT
and native admission explicitly deny both fixtures.

Validation passed: the focused m32 FST/FSTP ISA group (27 tests) and the full
offline workspace suite (830 tests) passed. The two interpreter CLI fixtures
and the JIT-denial fixture test passed. Host and Linux x86_64
`cargo check --workspace --all-targets --offline` checks passed; the Linux
cross-target check typechecked native-admission coverage but was not executed
as a guest binary on this Apple Silicon host. Formatting, diff and both fixture
JSON checks passed. Graphify updated successfully to 4,276 nodes, 8,745 edges
and 184 communities. The cross-target check emitted existing dead-code warnings
in `od-host`/`od-runner`; no errors. `tools/check_layout.py` reported only the
preserved unmapped `.serena/` directory; it was left untouched. Physical x86
acceptance remains deferred to the combined S2-31 substage under ISSUE-019.

Independent gpt-6.1-sol medium review of commit `eda15b9` was clean. The review
confirmed 481 `od-isa` tests, both minimum-normal interpreter fixtures and their
JIT denial. Native runtime and physical x86 acceptance remain deferred to the
combined S2-31 acceptance substage.

<a id="s2-31bo--binary80-normal-to-binary64-minimum-normal-rounding"></a>

## S2-31bo — binary80 Normal to binary64 minimum-normal rounding

The bounded DD10/DD18 path now admits Valid-tagged canonical binary80 Normal
sources at exponent -1023 when direct rounding of `S/2^12` selects binary64
minimum normal. Stored bits come only from that destination rounding. PE+C1 are
added, while UE is independently decided by rounding the unbounded 53-bit
significand from `S>>11` under the same sign-aware RC. Existing UE is preserved
when the wide rounding carries; source state, FCW/FSW gates, and other m64 paths
remain unchanged. The atomic Qword write precedes flags, pointer/RIP updates and
FSTP pop.

An exact-integer oracle covers nearest-even and directed destination thresholds
and neighbors, the distinct wide UE threshold, both signs, four RCs, both
opcodes, every TOP and all 16 sticky IE/DE/UE/PE combinations. Maximum-subnormal
and minimum-normal controls, write/range failures, FNSTSW/FNCLEX, and status
atomicity are covered. Positive nearest FST and negative directed FSTP fixtures
show UE+PE+C1 and PE+C1 without new UE, respectively. Both run in the
interpreter; the implementation's CLI fixture and JIT-denial tests passed.
Native admission coverage was typechecked for Linux x86_64, but native
execution was not performed.

Validation passed: 31 focused m64 FST/FSTP ISA tests and the full offline
workspace suite (835 tests), including the two CLI fixtures and JIT denial.
`cargo check --workspace --all-targets --offline` passed on the Apple Silicon
host; the same all-target check passed for `x86_64-unknown-linux-gnu`, including
native-admission test code, but was compile-only on this host. `cargo fmt --all
-- --check` passed. `python3 tools/check_layout.py` reported the existing
unmapped `.serena/` directory, which was preserved. `graphify update .` rebuilt
the graph to 4,289 nodes, 8,769 edges and 178 communities. `git diff --check`
passed after final whitespace cleanup. Native runtime and physical x86
acceptance remain deferred to the combined S2-31 acceptance substage under
ISSUE-019. Independent gpt-6.1-sol medium review of `e8e097e` was clean. The
reviewer independently ran 31 m64 ISA tests and checked the destination and UE
thresholds. Its additional CLI, JIT and native reruns timed out; those reviewer
reruns are not counted as passing evidence. The implementation's own full
workspace suite (835 tests), CLI fixtures and JIT denial passed as listed above.

<a id="s2-31bp--binary80-normal-to-binary32-infinity-on-overflow"></a>

## S2-31bp — binary80 Normal to binary32 infinity on overflow

The bounded D910/D918 path now admits Valid-tagged canonical binary80 Normal
sources at exponent 127 when sign-aware rounding of `S/2^40` carries the 24-bit
significand to `1<<24`. It stores signed binary32 infinity, then commits sticky
OE+PE and C1 after a successful atomic Dword write. No IE, DE or UE is added.
Existing source, FCW/FSW gates, and FST/FSTP state/pointer/pop behavior remain
unchanged; exponent-128 sources continue to reject before data access.

The exact-integer matrix covers the maximum-finite value and successor, nearest
predecessor/midpoint/successor, and maximum significand for both signs, all RCs,
both opcodes, every TOP and all 16 sticky IE/DE/UE/PE combinations. It also
covers exponent-128 rejection, Dword write/range atomicity, FNSTSW/FNCLEX, and
sticky-OE re-entry after FNCLEX. Positive nearest FST and negative directed
FSTP CLI fixtures observe OE+PE+C1; interpreter execution and JIT denial pass.
Native denial is typechecked in Linux x86_64 all-target validation; native
execution was not performed.

Validation passed: 30 focused m32 FST/FSTP ISA tests and the full offline
workspace suite (840 tests). Both CLI fixtures and the new JIT-denial test
passed. Host and Linux x86_64 `cargo check --workspace --all-targets --offline`
checks passed; the Linux cross-target check includes native-admission test code
but is compile-only on this host. `cargo fmt --all -- --check` and
`git diff --check` passed. `python3 tools/check_layout.py` reports the existing
unmapped `.serena/` directory, which was preserved. The final `graphify update .`
rebuilt the graph to 4,301 nodes, 8,789 edges and 212 communities. Native runtime
and physical x86 acceptance remain deferred to the combined S2-31 acceptance
substage under ISSUE-019.


Independent gpt-6.1-sol medium review of `6b2bb69` was clean. The reviewer
independently confirmed 30 m32 interpreter tests, the CLI fixture and JIT
denial. Native-test filtering was unavailable in the review environment, and
the combined reviewer command timed out; these are not counted as independent
native evidence. Native runtime and physical x86 acceptance remain deferred.


<a id="s2-31bq--binary80-normal-to-binary64-infinity-on-overflow"></a>

## S2-31bq — binary80 Normal to binary64 infinity on overflow

The bounded DD10/DD18 path now admits Valid-tagged canonical binary80 Normal
sources at exponent 1023 when sign-aware rounding of `S/2^11` carries the
53-bit significand to `1<<53`. It stores signed binary64 infinity; after a
successful atomic Qword write it commits sticky OE+PE and C1. No IE, DE or UE is
added. Source bytes, FCW/FSW gates, FST/FSTP state and pointer behavior remain
unchanged; exponent-1024 sources continue to reject before data access.

An independent exact-integer oracle covers `0xfffffffffffff800`,
`0xfffffffffffff801`, `0xfffffffffffffbff`, `0xfffffffffffffc00`,
`0xfffffffffffffc01`, and `0xffffffffffffffff`, across both signs, all RCs,
both opcodes, every TOP and all 16 sticky IE/DE/UE/PE combinations. It also
covers finite versus infinity under directed rounding, exponent-1024 rejection,
Qword write/range atomicity, and FNSTSW/FNCLEX sticky-OE re-entry. Positive
nearest FST and negative directed FSTP CLI fixtures observe OE+PE+C1; JIT and
native admission deny both fixtures.

Validation passed: 34 focused m64 FST/FSTP ISA tests and the full offline
workspace suite (845 tests). Both CLI fixtures, the JIT-denial test, host
`cargo check --workspace --all-targets --offline`, and the Linux x86_64
all-target cross-check passed. The cross-target check compiled native-admission
test code but was not executed on this Apple Silicon host. Formatting and
`git diff --check` passed. The layout checker reports only the preserved
unmapped `.serena/` directory. `graphify update .` rebuilt the graph to 4,313
nodes, 8,809 edges and 180 communities. Native runtime and physical x86
acceptance remain deferred to the combined S2-31 acceptance substage under
ISSUE-019.


Independent gpt-6.1-sol medium review of `962a6b6` was clean. The reviewer
independently passed 46 m64fp ISA tests and three runner unit plus three
integration overflow tests. Its native-run command timed out twice; no native
runtime pass is claimed. Physical x86 acceptance remains deferred to the
combined S2-31 acceptance substage under ISSUE-019.


<a id="s2-31bu--exact-signed-zero-fist-m16int"></a>

## S2-31bu — exact signed-zero FIST/FISTP m16int

Only exact bare `DF 10`/`DF 18` now accept `X87Tag::Zero` paired with canonical
binary80 zero payloads: zero significand and sign/exponent word `0x0000` or
`0x8000`. Both signs and every admitted RC mode store Word zero with no new
exceptions, preserve PE and source bytes, and clear C1. FIST retains tag/TOP;
FISTP pops only after the atomic Word write. FCW/FSW, addressing, range, and
other width gates remain unchanged; m32 behavior is unchanged and m64 zero is
still unsupported.

Coverage spans sign × RC × opcode × TOP × PE × C1, tag/payload/control/prefix
rejection, Word write/no-read behavior, fault/range atomicity, and normal-value
regressions. One literal negative-zero FISTP fixture composes with FNSTSW. JIT
and native denial are registered.

Validation passed: the focused m16 FIST/FISTP unit suite passed 6/6; the CLI
fixture and JIT-denial tests each passed 1/1. The full `cargo test --workspace
--offline` suite passed 856 tests, including 497 `od-isa`, 57 runner CLI, 117
fixture/JIT, 166 testkit, 6 host, 4 jit, and 9 x86-runner tests. Host and Linux
x86_64 `cargo check --workspace --all-targets --offline` passed. The Linux
target compiles native-denial tests but does not execute them; existing
dead-code warnings remain. `cargo fmt --all -- --check` and `git diff --check`
passed. `python3 tools/check_layout.py` reports the preserved unmapped root
`.serena/` directory, excluded from this change. `graphify update .` rebuilt
the graph to 4,342 nodes, 8,852 edges and 180 communities. Native runtime and
physical x86 behavior remain deferred to the combined S2-31 acceptance
substage under ISSUE-019.


<a id="s2-31br--sticky-status-admission-for-bounded-m32m64-stores"></a>

## S2-31br — sticky status admission for bounded m32/m64 stores

The bounded D910/D918 m32 and DD10/DD18 m64 conversion gates now admit prior
sticky IE, DE, OE, UE and PE (`FSW & 0x80c4 == 0`) while continuing to reject
ZE, SF, ES and B. The exact FCW whitelist, sources, and addressing remain
unchanged. Incoming sticky flags are preserved; current conversion flags and
C1 are committed only after successful writes. OE alone remains OE without
adding PE. A subsequent bounded store is admitted with OE already set; the CLI
fixture stores exact binary80 1.0 and FNSTSW verifies `0x4508`, with PE/C1 clear.

The independent m32/m64 status matrix covers all 32 IE/DE/OE/UE/PE combinations
across both widths/opcodes, both signs, four RC modes, every TOP, and zero,
normal exact/inexact, infinity, QNaN and SNaN sources. Existing focused
conversion matrices cover exact/inexact subnormals, minimum-normal promotion,
and rounding-carry overflow with incoming sticky combinations. Additional checks
cover ZE/SF/ES/B and FCW rejection before access, overflow FST followed by
FSTP without FNCLEX, FNSTSW/FNCLEX, and write/range fault atomicity. CLI, JIT
denial, and native-denial fixture definitions are updated.

Validation passed: 490/490 `od-isa` tests and 845/845 tests in
`cargo test --workspace --offline`. The updated CLI fixture test and the JIT
denial integration test passed. Host `cargo check --workspace --all-targets --offline` passed. The Linux
x86_64-target all-target check compiles native-admission tests on this macOS
host; it does not run them. `cargo fmt --all -- --check` and `git diff --check`
passed. `graphify update .` rebuilt the graph to 4,318 nodes, 8,814 edges and
171 communities. `python3 tools/check_layout.py` reports the existing unmapped
root directory `.serena/`, which was preserved and is not part of the change.
Native runtime and physical x86 acceptance remain deferred to the combined
S2-31 acceptance substage under ISSUE-019.


<a id="s2-31bs--sticky-oe-on-raw-binary80-fstp-m80fp"></a>

## S2-31bs — sticky OE on raw binary80 FSTP m80fp

Only the DB38 m80 source gate now admits sticky OE, changing its mask from
`0x80cc` to `0x80c4`. The existing FCW whitelist, exact instruction, source
tag/class pairs, address checks, raw ten-byte write and post-success pop remain
unchanged. No conversion or exception flags are generated; an OE-only SNaN is
stored byte-for-byte without IE/PE. C1 clears as before, while C0/C2/C3 and
incoming exception bits remain unchanged. Faults leave state and memory
uncommitted.

The source matrix covers all 32 IE/DE/OE/UE/PE combinations across accepted
classes, signs, all four FCWs, every TOP, and C1 set/clear. Composition tests
exercise m32 and m64 overflow followed by raw DB38 without FNCLEX, checking the
original source bytes, preserved OE+PE, and cleared C1. An OE-only negative SNaN
CLI fixture verifies raw bytes and FNSTSW `0x0808`; JIT denial is tested, and
the native-denial fixture test compiles for Linux x86_64.

Validation passed: the focused `od-isa` m80fp suite (18/18), updated CLI
fixture test, and JIT-denial integration test passed. The full
`cargo test --workspace --offline` suite passed, including 491 `od-isa` tests.
Host and Linux x86_64 `cargo check --workspace --all-targets --offline`
passed. The Linux-target check compiles native-admission test code on this
macOS host; it does not run it. `cargo fmt --all -- --check` and
`git diff --check` passed. `graphify update .` rebuilt the graph to 4,324
nodes, 8,822 edges and 179 communities.
`python3 tools/check_layout.py` reports the preserved, pre-existing unmapped
root `.serena/` directory; it is excluded from this change. Native runtime and
physical acceptance remain deferred to the combined S2-31 acceptance substage
under ISSUE-019.


<a id="s2-31bt--exact-signed-zero-fist-m32int"></a>

## S2-31bt — exact signed-zero FIST/FISTP m32int

Only bare `DB 10` and `DB 18` now accept `X87Tag::Zero` paired with a canonical
binary80 zero payload (zero significand and sign/exponent word `0x0000` or
`0x8000`). Both signs and all four admitted RC modes store integer zero; no new
flags are generated, incoming PE and source payloads are preserved, and C1 is
cleared. FIST retains tag/TOP; FISTP updates the old TOP tag and advances TOP
only after the atomic Dword write. FCW, FSW, addressing, range, and other
conversion gates are unchanged. m16/m64 and nonzero magnitudes below one remain
unsupported.

The independent interpreter matrix covers sign × RC × opcode × TOP × PE × C1,
canonical/tag/class rejection, unsupported FCW/FSW, and write/range atomicity.
Two CLI fixtures compose negative-zero FIST and positive-zero FISTP with
FNSTSW. JIT and native denial fixtures are included.

Validation passed: the focused m32 FIST unit suite passed 6/6, the CLI/FNSTSW
fixture test passed 1/1, and JIT denial passed 1/1. The full
`cargo test --workspace --offline` suite passed 851 tests, including 494
`od-isa`, 56 runner CLI, 116 fixture/JIT, and 166 testkit tests. Host and Linux
x86_64 `cargo check --workspace --all-targets --offline` passed; the Linux
target compiles native-denial tests but does not execute them. `cargo fmt
--all -- --check` and `git diff --check` passed. `python3 tools/check_layout.py`
reports the preserved unmapped root `.serena/` directory, excluded from this
change. `graphify update .` rebuilt the graph to 4,333 nodes, 8,837 edges and
189 communities. Native runtime and physical x86 behavior remain deferred to
the combined S2-31 acceptance substage under ISSUE-019.


<a id="s2-31bv--exact-signed-zero-fistp-m64int"></a>

## S2-31bv — exact signed-zero FISTP m64int

Only bare `DF 38` now admits `X87Tag::Zero` paired with canonical binary80
zero (zero significand; sign/exponent word `0x0000` or `0x8000`). Both signs
and all four admitted RC modes store Qword zero without adding flags, preserve
incoming PE and the source payload, and clear C1. FISTP empties the old TOP tag
and increments TOP modulo eight only after one atomic Qword write succeeds.
The existing FCW, FSW, bare Long64 RAX and full canonical48 Qword-range gates
remain in force; other forms, nonzero values below one and FIST m64 remain
unsupported.

The ISA matrix crosses sign, RC, TOP, incoming PE and C1, with tag/payload/
class/control/prefix rejection, single-write/no-read behavior, and write/range
fault atomicity. A negative-zero CLI fixture composes `DF 38` with FNSTSW;
JIT and native admission remain explicitly denied.

Focused `od-isa` FISTP m64 tests passed 6/6, the CLI fixture test passed 1/1,
and JIT denial passed 1/1. The native-denial test is compiled only for Linux
x86_64 and was not executable on this macOS host; native runtime and physical
acceptance remain deferred under ISSUE-019.

The full `cargo test --workspace --offline` suite passed 861 tests, including
500 `od-isa`, 58 runner CLI, 118 fixture/JIT, 166 testkit, 6 host, 4 JIT, and
9 host x86-runner tests. Host and Linux x86_64
`cargo check --workspace --all-targets --offline` passed; the Linux target
compiles the Linux-only native admission tests but does not run them. Formatting
and `git diff --check` passed. `python3 tools/check_layout.py` reports the
preserved root `.serena/` directory as unmapped; it remains excluded from the
change. `graphify update .` rebuilt the graph to 4,351 nodes, 8,867 edges and
175 communities. The physical x86 acceptance batch remains deferred under
ISSUE-019.


<a id="s2-31bw--round-binary80-fractions-through-m32-fistfstp"></a>

## S2-31bw — round binary80 fractions through m32 FIST/FISTP

Only bare `DB 10` and `DB 18` now accept a Valid-tagged canonical binary80
Normal source with unbiased exponent -16382 through -1. The comparison-only
fraction path stores signed integer zero or one under the four admitted RC
modes, adds sticky PE for every accepted nonzero fraction, and sets C1 iff the
rounded magnitude is one. It uses the exact e=-1 half threshold without
building large shifts or divisors. Existing signed-zero and integer paths,
source payloads, FCW/FSW/address gates, and FIST/FISTP commit/pop behavior are
unchanged.

The ISA matrix uses hand-authored literal expected Dword values for both signs
under RN/RD/RU/RZ and crosses both opcodes, all TOP values, incoming PE/C1,
exact half and its adjacent encodings, values below one, minimum Normal and
exponents -2/-63/-64/-65. Rejected tags/classes and environment gates are
checked before access; separate tests cover write/range atomicity. A negative
`-0.25` FISTP CLI fixture composes with FNSTSW to observe PE+C1 and TOP pop.
JIT and native admission remain explicitly denied.

Focused `cargo test -p od-isa fist_m32int_ --offline` passed 8/8; the separate
fraction write/range atomicity test passed 1/1. The CLI/FNSTSW fixture and JIT
denial each passed 1/1. The full `cargo test --workspace --offline` suite
passed 866 tests, including 503 `od-isa`, 59 runner CLI, 119 fixture/JIT, 166
testkit, 6 host, 4 JIT, and 9 host x86-runner tests. Host and Linux x86_64
`cargo check --workspace --all-targets --offline` passed; the Linux target
compiles native-denial tests but does not execute them. `cargo fmt --all --
--check` and `git diff --check` passed. `python3 tools/check_layout.py` reports
the preserved root `.serena/` directory as unmapped; it remains excluded from
this change. `graphify update .` rebuilt the graph to 4,360 nodes, 8,884 edges
and 182 communities. Native runtime and physical x86 behavior remain deferred
under ISSUE-019.

<a id="s2-31ca--masked-binary80-infinity-through-m16-fistfstp"></a>

## S2-31ca — masked binary80 infinity through m16 FIST/FISTP

Only bare `DF 10` and `DF 18` now admit Special-tagged canonical binary80
positive or negative infinity (significand `0x8000000000000000`,
sign/exponent `0x7fff` or `0xffff`). Under the existing masked FCW whitelist,
both signs and every RC mode store Word integer indefinite `0x8000`. After a
successful atomic Word write, IE is sticky, C1 clears, incoming PE and
C0/C2/C3 remain preserved, and no PE/DE/OE/UE/SF or ES/B is added. Raw source
bytes, addressing, FSW gate, and post-write FIST/FISTP commit/pop remain
unchanged; m32 behavior is unchanged, and m64 is outside this slice. The
separate DF38 infinity case is covered by S2-31cb below.

The ISA matrix crosses both signs, four FCWs, both opcodes, all TOP values,
and incoming PE/C1. Rejections cover mismatched tags, noncanonical special
classes, finite values paired with Special, controls, FSW, prefixes, and the
m64 form. Fault tests check denied and backend Word writes plus a canonical48
range crossing, asserting complete CPU/memory atomicity. Existing exact
-32768 finite coverage remains in the signed-boundary tests. A literal
negative-infinity FISTP/FNSTSW fixture observes IE and the popped TOP/tag; an
ISA FISTP/FNSTSW/FNCLEX sequence verifies IE can be observed then cleared. JIT
and native admission remain closed.

Focused `fist_m16int_` tests passed 10/10 and `fistp_m16int_` tests passed 6/6;
the corresponding m32 groups passed 10/10 and 7/7 after the helper extension.
The CLI fixture and JIT-denial tests passed 1/1 each. The full
`cargo test --workspace --offline` suite passed 888 tests: 517 `od-isa`, 4
`od-jit`, 6 host, 63 runner CLI, 123 fixture/JIT, 166 testkit, 8 x86-runner
library and 1 x86-runner main; all doc-tests passed. Host and Linux x86_64
all-target checks passed; Linux compiled native-denial coverage but did not
execute it. `cargo fmt --all -- --check` and `git diff --check` passed.
`python3 tools/check_layout.py` reports only the preserved root `.serena/`
directory as unmapped; it remains untracked and excluded. Changed S2-31ca plan,
contract, report and README links were manually cross-checked. `graphify update
.` rebuilt the graph to 4,398 nodes, 8,958 edges and 181 communities. Native
runtime and physical x86 acceptance remain deferred under ISSUE-019.



<a id="s2-31cb--masked-binary80-infinity-through-m64-fistp"></a>

## S2-31cb — masked binary80 infinity through m64 FISTP

Only bare `DF 38` now admits Special-tagged canonical binary80 positive or
negative infinity (significand `0x8000000000000000`, sign/exponent `0x7fff` or
`0xffff`). Under the existing masked FCW whitelist, both signs and every RC
mode store Qword integer indefinite `0x8000000000000000`. After a successful
atomic Qword write, IE is sticky, C1 clears, incoming PE and C0/C2/C3 remain
preserved, and no PE/DE/OE/UE/SF or ES/B is added. Raw source bytes, addressing,
FSW gate, and post-write FISTP commit/pop behavior remain unchanged; no FIST
m64 form is added.

The ISA matrix crosses signs, all admitted FCWs, TOP values, and incoming
PE/C1. A paired exact -2^63 case verifies that finite minimum and infinity
store the same Qword bits while only infinity sets IE. Rejection tests cover
mismatched tags, NaNs, pseudo-infinity, malformed specials, FCW/FSW gates and
prefixes. Fault tests assert complete state/memory atomicity for denied and
backend Qword writes and canonical48 range failures. A negative-infinity
FISTP/FNSTSW CLI fixture observes IE and the popped TOP/tag; a direct
FISTP/FNSTSW/FNCLEX sequence verifies observation and clearing. JIT/native
admission remains closed.

Focused `cargo test -p od-isa fistp_m64int_ --offline` passed 14/14, including
the infinity matrix, finite -2^63 distinction, rejection, fault and FNSTSW/
FNCLEX tests. The m16 and m32 infinity rejection regressions each passed 1/1
after removing their obsolete DF38 rejection. The CLI fixture and JIT denial
passed 1/1 each. The full `cargo test --workspace --offline` suite passed 895
tests: 522 `od-isa`, 4 `od-jit`, 6 host, 64 runner CLI, 124 fixture/JIT, 166
testkit, 8 x86-runner library and 1 x86-runner main; all doc-tests passed.
Host and Linux x86_64 all-target checks passed; Linux compiled native-denial
coverage but did not execute it. Formatting, diff and S2-31cb anchor checks
passed. `python3 tools/check_layout.py` reports only the preserved root
`.serena/` directory as unmapped; it remains excluded. `graphify update .`
rebuilt the graph to 4,409 nodes, 8,981 edges and 175 communities. Native
runtime and physical x86 acceptance remain deferred under ISSUE-019.

<a id="s2-31cc--masked-binary80-qnan-through-m32-fistfstp"></a>

## S2-31cc — masked binary80 quiet NaN through m32 FIST/FISTP

Only bare `DB 10` and `DB 18` now admit Special-tagged canonical binary80
quiet NaNs: exponent `0x7fff`, either sign, J=1, quiet bit=1, and arbitrary
lower payload. Under the existing FCW whitelist, every sign, payload and RC
stores Dword integer indefinite `0x80000000`. After a successful atomic write,
IE is set, C1 clears, incoming PE and C0/C2/C3 are preserved, and no PE/DE/OE/UE/
SF or ES/B is added. The source remains raw and state/pointers/TOP/RIP commit only
after the Dword write. SNaN, J=0 pseudo-NaNs, mismatched tags, finite overflow,
m16/m64 QNaNs and unmasked delivery remain outside this slice.

The ISA matrix crosses four payload patterns, both signs, all four admitted
FCWs, DB10/DB18, all TOP values and incoming PE/C1. Rejections cover J=0
quiet-like encodings, SNaN, tags, denied FCW/FSW, a complete operand-size
prefix, and DF38 m64. Fault coverage checks denied Dword writes and canonical48
range failures without state or memory changes. Focused tests passed: the 2,048
combination success matrix 1/1; quiet-NaN rejection/FNSTSW/FNCLEX/fault tests
2/2; the stale m32-infinity rejection test 1/1 after removing the newly
admitted canonical QNaN; CLI fixture 1/1; JIT denial 1/1.

`cargo test --workspace --offline` passed 900 tests: 525 `od-isa`, 4 `od-jit`,
6 host, 65 runner CLI, 125 fixture/JIT, 166 testkit, 8 x86-runner library and
1 x86-runner main; all doc-tests passed. Host and Linux x86_64
`cargo check --workspace --all-targets --offline` passed; Linux compiled native
denial coverage but did not execute it. `cargo fmt --all -- --check` and
`git diff --check` passed. `graphify update .` rebuilt the graph to 4,418 nodes,
8,998 edges and 166 communities. Native runtime and physical x86 acceptance
remain deferred under ISSUE-019.

<a id="s2-31cd--masked-binary80-qnan-through-m16-fistfstp"></a>

## S2-31cd — masked binary80 quiet NaN through m16 FIST/FISTP

Only bare `DF 10` and `DF 18` now admit Special-tagged canonical binary80 quiet
NaNs: exponent `0x7fff`, either sign, J=1, quiet bit=1, and arbitrary lower
payload. Under the existing FCW whitelist, every sign, payload and RC stores
Word integer indefinite `0x8000`. After a successful atomic Word write, IE is
set, C1 clears, incoming PE and C0/C2/C3 are preserved, and no PE/DE/OE/UE/SF or
ES/B is added. Raw source bytes and post-write state/pop order remain unchanged.
SNaN, J=0 pseudo-NaNs, malformed/mismatched tags, m64 QNaNs and unmasked
delivery remain outside the slice; m32 behavior is unchanged.

The ISA matrix crosses four payload patterns, both signs, all four admitted
FCWs, DF10/DF18, every TOP and incoming PE/C1. Rejections cover J=0
quiet-like encodings, SNaN, mismatched tags, forbidden FCW/FSW, operand/address
prefixes and DF38 m64. Fault coverage verifies denied Word writes and
canonical48 range failures leave full state and memory unchanged. Focused
results: the 2,048-combination success matrix 1/1; rejection/fault/FNSTSW/FNCLEX
2/2; existing m16-infinity malformed-source regression 1/1; literal CLI fixture
1/1; JIT denial 1/1. After correcting the fixture ID to accurately name its
FISTP/FNSTSW sequence, CLI and JIT fixture tests passed again 1/1 each.

`cargo test --workspace --offline` passed 905 tests: 528 `od-isa`, 4 `od-jit`,
6 host, 66 runner CLI, 126 fixture/JIT, 166 testkit, 8 x86-runner library and
1 x86-runner main; all doc-tests passed. Host and Linux x86_64
`cargo check --workspace --all-targets --offline` passed; Linux compiled native
denial coverage but did not execute it. `cargo fmt --all -- --check` and
`git diff --check` passed. `graphify update .` rebuilt the graph to 4,427 nodes,
9,015 edges and 178 communities; contract, plan, report and fixture README
anchors were checked. Native runtime and physical x86 acceptance remain deferred
under ISSUE-019.

<a id="s2-31ce--masked-binary80-qnan-through-m64-fistp"></a>

## S2-31ce — masked binary80 quiet NaN through m64 FISTP

Only bare `DF 38` now admits Special-tagged canonical binary80 quiet NaNs:
exponent `0x7fff`, either sign, J=1, quiet bit=1, and arbitrary lower payload.
Under the existing FCW whitelist, every sign, payload and RC stores Qword
integer indefinite `0x8000000000000000`. After a successful atomic write, IE is
set, C1 clears, incoming PE and C0/C2/C3 are preserved, and no PE/DE/OE/UE/SF
or ES/B is added. Raw source bytes and post-write FISTP state/pop ordering
remain unchanged. SNaN, J=0 pseudo-NaNs, malformed/mismatched tags, finite
overflow, non-pop m64 and unmasked delivery remain outside this slice.

The ISA matrix crosses four payload patterns, both signs, all four admitted
FCWs, all TOP values and incoming PE/C1 (1,024 cases). Rejections cover J=0
quiet-like encodings, SNaN, tag mismatch, FCW/FSW, prefixes and canonical48
range failures; Qword canaries, denied writes and full state equality verify
atomicity. Focused tests passed: matrix 1/1; QNaN rejection/fault/FNSTSW/FNCLEX
2/2; prior m64-infinity rejection 1/1; stale m16 and m32 width-rejection
regressions 1/1 each after removing the now-admitted DF38 QNaN cases; CLI
fixture 1/1; JIT denial 1/1.

`cargo test --workspace --offline` passed 910 tests: 531 `od-isa`, 4 `od-jit`,
6 host, 67 runner CLI, 127 fixture/JIT, 166 testkit, 8 x86-runner library and
1 x86-runner main; all doc-tests passed. Host and Linux x86_64
`cargo check --workspace --all-targets --offline` passed; Linux compiled native
denial coverage but did not execute it. `cargo fmt --all -- --check` and
`git diff --check` passed. `graphify update .` rebuilt the graph to 4,436 nodes,
9,032 edges and 183 communities; contract/plan/report anchors were checked.
Native runtime and physical x86 acceptance remain deferred under ISSUE-019.


<a id="s2-31cl--masked-finite-overflow-to-m64-fistp"></a>

## S2-31cl — masked finite overflow to m64 `FISTP`

Only bare `DF 38` now converts Valid-tagged canonical binary80 Normal inputs
that round outside signed i64. The shared integer converter rounds quotient and
remainder under FCW before comparing the signed bounds. Out-of-range results
store Qword integer-indefinite and add IE only after successful write, clear
C1, and add no PE/OE/UE/DE/SF/ES/B. Values that round back into range retain
finite storage and PE/C1 behavior. Exponent 64+ takes the invalid path without
shifting by a large amount; exponent 63 accepts only exact negative i64
minimum at the lower boundary. Existing fractional behavior below one is
unchanged.

The independent table contains 26 signed literal inputs × four RC modes ×
eight TOPs × incoming IE × incoming PE × incoming C1 (6,656 executions).
Boundary vectors include positive i64 max, its adjacent half-integer and its
neighbor, the negative counterpart, exact i64 min and the next negative
integer. High-exponent vectors cover 64, 65 and 16383 with minimum, mixed and
maximum significands and both signs. Tests also distinguish exact i64 min from
indefinite by IE, prove invalid fractional conversions do not add PE, preserve
raw source bytes and verify denied-write/canonical-range atomicity plus
FNSTSW/FNCLEX.

The formerly rejecting S2-31f positive-overflow fixture is now a literal
`DF 38; DF E0` CLI case: it commits indefinite, pops, observes IE via FNSTSW,
and preserves the source bytes. The CLI fixture and JIT-denial tests passed
1/1 each. The native-denial test is architecture-gated and ran 0 tests on the
local arm64 host; its Linux x86_64 all-target compile succeeded.

The 22 focused `fistp_m64int_` ISA tests passed. `cargo test --workspace
--offline --quiet` passed 948 tests: 6 `od-host`, 550 `od-isa`, 4 `od-jit`, 77
runner CLI, 136 fixture/JIT, 166 `od-testkit`, 8 x86-runner library, and 1
x86-runner main; all doc-test invocations passed. Host
`cargo check --workspace --all-targets --offline` and Linux x86_64
`cargo check --workspace --all-targets --offline --target
x86_64-unknown-linux-gnu` passed. The Linux check emitted existing dead-code
warnings in `od-host` and `od-runner`; it compiled architecture-gated native
denial code but did not execute that test on the arm64 host. `cargo fmt --all
-- --check` and `git diff --check` passed. `python3 tools/check_layout.py` could
not complete because the preserved untracked `.serena/` root directory is not
mapped by the repository checker; no layout was changed. `graphify update .`
rebuilt the graph to 4,511 nodes, 9,164 edges and 183 communities. The S2-31cl
anchors in the plan, contract, report and fixture README resolve to their
matching sections. A separate read-only final review found no remaining code,
fixture or documentation findings.

Native runtime and physical x86 acceptance remain deferred under ISSUE-019.


<a id="s2-31cj--masked-finite-overflow-to-m32-fistfstp"></a>

## S2-31cj — masked finite overflow to m32 `FIST`/`FISTP`

Canonical Valid-tagged binary80 Normal inputs to bare DB 10/DB 18 now round
under the existing RC before signed-i32 range testing. If the rounded integer
is out of range, the Dword result is integer-indefinite `0x80000000` and IE is
added after a successful write; invalid conversion adds no PE and clears C1.
Finite values just outside the real range that round into range remain finite.
Exponent `e >= 32` takes the invalid path before shifting; exponents 0 through
31 use exact quotient/remainder rounding, while negative-exponent fractions
retain their comparison-based path. Other widths/classes remain unchanged by
this m32 slice; S2-31ck later extends m16 and S2-31cl later adds m64 finite
overflow.

The literal oracle crosses 42 vectors (signed limits, neighbors/tie-adjacent
values and exponents 32/63/64/16383 × minimum/mixed/maximum significands and
both signs) with four RCs, both m32 opcodes, every TOP, incoming IE/PE and both
incoming C1 states (21,504 combinations). It checks status flags, current C1,
source/pointer/tag preservation, one Dword write/no read, and both exact
`-2^31` and the same-bit invalid-indefinite result. Overflow write-denial,
injected write fault and canonical-range tests seed sticky IE|PE and verify
full state/memory atomicity. Focused m32 tests passed 13/13; the overflow
FNSTSW/FNCLEX sequence passed 1/1. The two new literal CLI fixtures and the
two prior exponent-32 CLI fixtures passed; the JIT denials passed 2/2.

`cargo test --workspace --offline --quiet` passed 938 tests: 545 `od-isa`,
4 `od-jit`, 6 `od-host`, 74 runner CLI, 134 fixture/JIT, 166 `od-testkit`,
8 x86-runner library, and 1 x86-runner main; all doc-tests passed. Host
`cargo check --workspace --all-targets --offline` and Linux x86_64
`cargo check --workspace --all-targets --offline --target x86_64-unknown-linux-gnu`
passed. The Linux cross-check compiled architecture-gated native denial code;
it did not execute on the local arm64 host. Existing dead-code warnings in
`od-host` and `od-runner` appeared on the Linux check. `cargo fmt --all --
--check` and `git diff --check` passed. `graphify update .` rebuilt the graph
to 4,491 nodes, 9,129 edges and 170 communities. The S2-31cj contract, plan,
report and fixture README anchors were checked against their targets.

Native runtime and physical x86 acceptance remain deferred under ISSUE-019.


<a id="s2-31ck--masked-finite-overflow-to-m16-fistfstp"></a>

## S2-31ck — masked finite overflow to m16 `FIST`/`FISTP`

Canonical Valid-tagged binary80 Normal inputs to bare DF 10/DF 18 now round
under the existing RC before signed-i16 range testing. Rounded values outside
`[-2^15, 2^15-1]` store Word integer-indefinite `0x8000`, then add IE and clear
C1 after successful write; invalid conversion adds no PE. Values just beyond
the real bounds that round back into range retain the finite integer result.
Exponent `e >= 16` is classified before a wide shift; exponents 0 through 15
use exact quotient/remainder rounding; negative-exponent fractions keep their
comparison path. m32, subnormal, malformed, and unmasked behavior stays
unchanged; m64 finite overflow is added later by S2-31cl.

The literal oracle includes 18 signed-boundary/neighbor/tie vectors and six
exponents (16/31/32/63/64/16383) × three significands × both signs, crossed with
four RCs, both opcodes, all TOP values, incoming IE/PE and both prior C1 states
(27,648 combinations). It distinguishes exact -32768 from same-bit invalid
indefinite, checks current C1 and exception flags, source and stack state, and
one Word write/no read. Both DF10 and DF18 denied-write and canonical-range
cases preserve full state and memory. The overflow FNSTSW/FNCLEX sequence,
two new literal CLI fixtures, four migrated historical overflow/exponent-16
fixtures, and JIT denial are checked; native denial remains architecture-gated.

Focused `cargo test -p od-isa fist_m16int --offline` passed 14/14. The new
overflow and roundback CLI fixtures each passed 1/1; the migrated historical
31d/31e overflow and exponent-16 cases passed 1/1; historical 31d/31e
boundary/rejection suites passed 1/1 each. The JIT denial test passed 1/1.

`cargo test --workspace --offline --quiet` passed 945 tests: 548 `od-isa`,
4 `od-jit`, 6 `od-host`, 77 runner CLI, 135 fixture/JIT, 166 `od-testkit`,
8 x86-runner library, and 1 x86-runner main; all doc-tests passed. Host and
Linux x86_64 `cargo check --workspace --all-targets --offline` passed. The
Linux cross-check compiled architecture-gated native denial code; it did not
execute on the local arm64 host. Existing Linux dead-code warnings in `od-host`
and `od-runner` remain. `cargo fmt --all -- --check` and `git diff --check`
passed. `graphify update .` rebuilt the graph to 4,503 nodes, 9,151 edges and
184 communities. The S2-31ck contract, plan, report, known-issues and fixture
README anchors were checked against their targets.

Native runtime and physical x86 acceptance remain deferred under ISSUE-019.


<a id="s2-31ci--sticky-ie-on-bounded-integer-stores"></a>

## S2-31ci — sticky IE on bounded integer stores

The entry gate for the five existing bounded integer stores (DB 10, DB 18,
DF 10, DF 18, and DF 38) now masks `0x80de` instead of `0x80df`. Incoming IE
and PE are admitted and preserved; DE, ZE, OE, UE, SF, ES, and B remain
rejected. Conversion-specific PE/C1 behavior, source bytes, FCW, classes,
addressing, and post-write state/pop ordering are unchanged. The 17,920-case
matrix crosses seven source classes, both signs, five forms, four FCWs, eight
TOP values, four IE/PE states, and both incoming C1 states. All five forms also
have incoming IE|PE denied-write and canonical-range atomicity checks, plus
IE|each-forbidden-status pre-access rejection.

Composition tests cover masked SNaN FIST followed by FISTP without FNCLEX,
then exact finite FISTP while IE remains sticky and PE stays clear; FNSTSW
observes status, and FNCLEX clears it. Two literal CLI fixtures cover SNaN
FIST/FNSTSW and exact finite FISTP/FNSTSW under incoming IE. Their CLI and JIT
denial tests passed, as did focused matrix (1/1), atomicity/forbidden-status
(1/1), composition (1/1), and both fixture CLI tests (1/1 each). Historical
m16 FIST/FISTP invalid-status fixtures were corrected from IE-only to DE
status and their CLI tests passed. `cargo test --workspace --offline` passed
932 tests: 543 `od-isa`, 4 `od-jit`, 6 `od-host`, 72 runner CLI, 132
fixture/JIT, 166 `od-testkit`, 8 x86-runner library, and 1 x86-runner main;
all doc-tests passed. Host `cargo check --workspace --all-targets --offline`
and Linux x86_64 `cargo check --workspace --all-targets --offline --target
x86_64-unknown-linux-gnu` passed. Linux emitted existing dead-code warnings in
od-host and od-runner. `cargo fmt --all -- --check` and `git diff --check`
passed. `graphify update .` rebuilt the graph to 4,479 nodes, 9,109 edges,
and 179 communities; documentation anchors were checked.
The native-denial test compiled in Linux all-target checks but is not executed
on this arm64 host; native runtime and physical x86 acceptance remain deferred
under ISSUE-019.

Native runtime and physical x86 acceptance remain deferred under ISSUE-019.


<a id="s2-31ch--masked-binary80-snan-through-m64-fistp"></a>

## S2-31ch — masked binary80 signaling NaN through m64 FISTP

Only bare `DF 38` admits Special-tagged canonical binary80 SNaNs: exponent
`0x7fff`, either sign, J=1, quiet bit=0, and nonzero lower 62-bit payload.
Every admitted payload, sign and FCW stores Qword integer indefinite
`0x8000000000000000`. After the successful atomic write, IE is set and C1 is
cleared; incoming PE/C0/C2/C3 are preserved, and no other flags are added.
The raw signaling source remains unchanged through FISTP. The existing FSW
gate continues to reject incoming IE; no non-pop m64 form is admitted.

The matrix crosses four payload patterns × both signs × four FCWs × every TOP
× incoming PE/C1 (1,024 cases). Tests also cover FNSTSW/FNCLEX observation,
J=0 pseudo-NaNs, tag/class/control/prefix rejection, Qword write/range
atomicity, and raw source preservation. Focused ISA SNaN tests passed 15/15;
the CLI fixture and JIT-denial tests passed 1/1 each. `cargo test --workspace
--offline` passed 925 tests: 6 `od-host`, 540 `od-isa`, 4 `od-jit`, 70 runner
CLI, 130 fixture/JIT, 166 `od-testkit`, 8 x86-runner library, and 1 x86-runner
main; all doc-tests passed. Host `cargo check --workspace --all-targets
--offline` and Linux x86_64 `cargo check --workspace --all-targets --offline
--target x86_64-unknown-linux-gnu` passed. The Linux check emitted existing
dead-code warnings in host/runner. The native-denial test is architecture-
gated: its test code compiled in the Linux all-target build, but the local
arm64 runner did not execute it. No native runtime or physical x86 acceptance
is claimed. `cargo fmt --all -- --check` passed, and `graphify update .`
rebuilt the graph to 4,466 nodes, 9,086 edges, and 172 communities.

Native runtime and physical x86 acceptance remain deferred under ISSUE-019.


<a id="s2-31cf--masked-binary80-snan-through-m32-fistfstp"></a>

## S2-31cf — masked binary80 signaling NaN through m32 FIST/FISTP

Only bare `DB 10` and `DB 18` admit Special-tagged canonical binary80 SNaNs:
exponent `0x7fff`, either sign, J=1, quiet bit=0, and nonzero lower 62-bit
payload. Every admitted sign, payload and RC stores Dword integer indefinite
`0x80000000`; after successful write, sticky IE is set and C1 cleared, with
incoming PE/C0/C2/C3 preserved and no other flags added. Raw source bytes stay
unchanged through FIST/FISTP. Existing infinity and QNaN classes stay admitted;
pseudo-NaNs, mismatched tags, finite overflow, other widths, and unmasked
delivery remain outside this slice.

The matrix covers four payload shapes × both signs × four FCWs × both opcodes ×
eight TOP values × PE/C1 (2,048 cases). Tests cover raw source preservation,
FIST/FISTP tags/TOP and pointer commit, J=0 pseudo-SNaN/pseudo-QNaN, tag/class,
control, prefix, width, and range/write atomic rejection. A literal FISTP/FNSTSW
fixture and a direct FNSTSW/FNCLEX sequence observe and clear IE. Focused
evidence: SNaN matrix 1/1; SNaN observation/rejection/fault tests 2/2; previous
m32 QNaN and infinity stale-rejection regressions 1/1 each; CLI fixture 1/1;
JIT denial 1/1. The final `cargo test --workspace --offline` passed 915 tests:
534 `od-isa`, 4 `od-jit`, 6 `od-host`, 68 runner CLI, 128 fixture/JIT, 166
`od-testkit`, 8 x86-runner library and 1 x86-runner main; all doc-tests passed.
Host and Linux x86_64 `cargo check --workspace --all-targets --offline` passed.
The native-denial test is architecture-gated; it compiled in the Linux
x86_64 check but did not run on the local arm64 host. `cargo fmt --all --
--check` and `git diff --check` passed. `graphify update .` rebuilt the graph
to 4,446 nodes, 9,050 edges and 180 communities; documentation anchors were
checked.

Native runtime and physical x86 acceptance remain deferred under ISSUE-019.


<a id="s2-31cg--masked-binary80-snan-through-m16-fistfstp"></a>

## S2-31cg — masked binary80 signaling NaN through m16 FIST/FISTP

Only bare `DF 10` and `DF 18` admit Special-tagged canonical binary80 SNaNs:
exponent `0x7fff`, either sign, J=1, quiet bit=0, and nonzero lower 62-bit
payload. Every admitted sign, payload and RC stores Word integer indefinite
`0x8000`; after the atomic Word write, IE is set and C1 cleared, incoming
PE/C0/C2/C3 are preserved, and no other flags are added. Raw SNaN bytes remain
unchanged for FIST and FISTP. The m32 SNaN path remains unchanged, and m64 stays
outside this m16 slice; its exact `DF 38` FISTP form is added separately in
S2-31ch.

The matrix crosses four payload shapes × both signs × four FCWs × both opcodes
× eight TOP values × PE/C1 (2,048 cases). Rejections cover J=0 pseudo-NaNs,
tag mismatches, all FSW gate bits, invalid FCW, prefixes, m64, canonical-range
faults and denied writes with full-state/memory preservation. A literal
FISTP/FNSTSW fixture and direct FNSTSW/FNCLEX composition observe and clear IE.
Focused tests passed: m16 matrix 1/1; m16 observation/rejection/fault 2/2;
stale m16 QNaN and infinity rejection regressions 1/1 each; prior m32 SNaN
matrix 1/1 and observation/rejection/fault 2/2; CLI fixture 1/1; JIT denial
1/1. The m32 width fence was
updated to keep only DF38 outside the m16/m32 admitted SNaN widths. The
native-denial test is architecture-gated, was compiled for Linux x86_64, and
did not run on the local arm64 host.

`cargo test --workspace --offline` passed 920 tests: 537 `od-isa`, 4 `od-jit`,
6 `od-host`, 69 runner CLI, 129 fixture/JIT, 166 `od-testkit`, 8 x86-runner
library and 1 x86-runner main; all doc-tests passed. Host and Linux x86_64
`cargo check --workspace --all-targets --offline` passed. `cargo fmt --all --
--check` and `git diff --check` passed. `graphify update .` confirmed the graph
to 4,456 nodes, 9,068 edges and 165 communities; the native-denial test
compiled for Linux x86_64 and remains unexecuted on the arm64 host.

Native runtime and physical x86 acceptance remain deferred under ISSUE-019.


<a id="s2-31cm--sticky-de-on-bounded-integer-stores"></a>

## S2-31cm — sticky DE on bounded integer stores

The shared bounded-integer entry mask changes only from `0x80de` to
`0x80dc`, admitting sticky DE across the existing five forms DB10, DB18,
DF10, DF18 and DF38. Incoming IE/DE/PE are preserved independently; DE alone
adds no current-operation exception. ZE/OE/UE/SF/ES/B still reject, and no
other x87 gates, source classes or widths changed. Current flags/C1 and stack
state remain committed after the atomic destination write. Binary80 Subnormal
and pseudo-denormal operands remain unsupported; supported FLD m32/m64
subnormals become canonical Normal binary80 values and can then use the
existing integer fraction conversion.

Focused checks passed: the 35,840-combination seven-class/sign/form/FCW/TOP/
IE/DE/PE/prior-C1 matrix (1 test), 40 FLD min/max subnormal-to-five-form
composition cases (1 test), five-form IE|DE|PE write/range atomicity and
DE-plus-forbidden-status rejection (1 test), both stale m64 DE-gate regressions
(1 each), two m16 legacy invalid-status CLI controls (1 each), two new CLI
fixtures (1 test), and JIT denial (1 test). The new fixtures use the schema
accepted FISTP/FNSTSW pair and seed the canonical binary80 Normal intermediate
that FLD produces; the actual FLD→store sequences are exercised directly in
the ISA composition matrix. A scan found no other bounded-integer fixture
using DE as a negative control.

`cargo test --workspace --offline` passed 951 tests: 6 `od-host`, 551 `od-isa`,
4 `od-jit`, 78 runner CLI, 137 fixture/JIT, 166 `od-testkit`, 8 x86-runner
library and 1 x86-runner main; all doc-tests passed. Host
`cargo check --workspace --all-targets --offline` and Linux x86_64
`cargo check --workspace --all-targets --offline --target
x86_64-unknown-linux-gnu` passed. No native runtime or physical x86 execution
is claimed. `cargo fmt --all -- --check` and `git diff --check` passed.
`graphify update .` rebuilt the graph to 4,519 nodes, 9,175 edges and 185
communities. The native-denial test is architecture-gated and filtered out on
the local arm64 host (0 tests run by the specific filter); it compiled as part
of the Linux all-target check.

Native runtime and physical x86 acceptance remain deferred under ISSUE-019.


<a id="s2-31cn--sticky-ue-on-bounded-integer-stores"></a>

## S2-31cn — sticky UE on bounded integer stores

Only the shared bounded-integer entry mask changed, from `0x80dc` to
`0x80cc`, for DB10, DB18, DF10, DF18 and DF38. This admits and preserves
incoming UE independently with IE/DE/PE, while ZE/OE/SF/ES/B remain denied.
UE by itself does not alter the integer value or synthesize IE/PE; C1 remains
specific to the current conversion. FCW/source/address/range gates, atomic
write-before-state commit, FIST/FISTP stack behavior, and the separate FLD
gates are unchanged. Raw binary80 subnormal and pseudo-denormal sources remain
unsupported.

Focused checks passed: the 71,680-case matrix spans seven existing admitted
source classes, both signs, five forms, four FCWs, eight TOP values, all 16
IE/DE/UE/PE combinations and both incoming C1 states; 40 actual FST m32→integer
composition cases cover both signs and all FCWs/forms; five-form UE-bearing
write/range atomicity and UE|forbidden-status rejection pass; the stale m32
infinity rejection test passes with UE removed from its negative environment
controls. Two literal m32/m64 FISTP/FNSTSW fixtures pass CLI execution and JIT
denial. The native admission-denial filter matched zero tests on the local arm64 host;
the Linux x86_64 all-target check compiled that test but did not execute it.

Native runtime and physical x86 acceptance remain deferred under ISSUE-019.

`cargo test --workspace --offline` passed 953 tests: 6 `od-host`, 552 `od-isa`,
4 `od-jit`, 79 runner CLI, 137 fixture/JIT, 166 `od-testkit`, 8 x86-runner
library and 1 x86-runner main; all doc-tests passed. Host
`cargo check --workspace --all-targets --offline` and Linux x86_64
`cargo check --workspace --all-targets --offline --target
x86_64-unknown-linux-gnu` passed. `cargo fmt --all -- --check` and
`git diff --check` passed. `graphify update .` rebuilt the graph to 4,525 nodes, 9,184 edges and 181
communities. Documentation anchors and fixture references were checked. Native
runtime and physical x86 acceptance remain deferred under ISSUE-019.


<a id="s2-31co--sticky-oe-on-bounded-integer-stores"></a>

## S2-31co — sticky OE on bounded integer stores

The shared gate for only DB10/DB18/DF10/DF18/DF38 changed from `0x80cc` to
`0x80c4`. It admits historical OE with IE/DE/UE/PE and still rejects ZE/SF/ES/B.
Historical OE does not change the current result, add IE/PE, or preserve the
previous C1. Other x87 gates and raw binary80 subnormal/pseudo-denormal
rejection remain unchanged.

The full 143,360-case matrix passed across seven admitted source classes,
signs, all five forms, FCW/TOP combinations, 32 sticky flag combinations and
prior C1. Twenty actual FST m32 overflow→integer-store compositions passed for
the sign/rounding cases that produce infinity: OE|PE survives the integer
indefinite result and current IE, and C1 clears. Five-form write/range
atomicity and OE plus each forbidden-status control pass. OE-only exact finite
conversion and invalid re-entry followed by FNSTSW/FNCLEX are covered. CLI
fixtures verify sticky OE and FNSTSW; JIT/native fixture admission remains
denied.

`cargo test --workspace --offline` passed 956 tests (553 `od-isa`, 80 runner
CLI, 138 fixture/JIT, 6 `od-host`, 4 `od-jit`, 166 `od-testkit`, 8 x86-runner
library, and 1 x86-runner binary); all doc-tests passed. Host
`cargo check --workspace --all-targets --offline`, Linux x86_64
`cargo check --workspace --all-targets --offline --target
x86_64-unknown-linux-gnu`, and `cargo fmt --all -- --check` passed. Linux check
reported existing dead-code warnings in `od-host` and the runner. The focused
native-denial test filter matched zero tests on the local arm64 host; its
x86_64 Linux test target compiled in the all-target check. `graphify update .`
refreshed the graph to 4,532 nodes, 9,195 links and 182 communities. Native
runtime and physical x86 acceptance remain deferred under ISSUE-019.


<a id="s2-31cp--historical-sticky-ze-on-bounded-integer-stores"></a>

## S2-31cp — historical sticky ZE on bounded integer stores

The shared gate for only DB10/DB18/DF10/DF18/DF38 changed from `0x80c4` to
`0x80c0`. It admits historical ZE alongside IE/DE/OE/UE/PE and continues to
reject SF/ES/B. ZE alone leaves the current integer result and exception flags
unchanged; C1 remains derived from the current conversion. Other x87 gates,
classes, addressing, FCW, rounding and atomic store ordering are unchanged.

The full 286,720-case matrix spans seven admitted source classes, signs, all
five forms, FCWs, TOP values, the 64 combinations of IE/DE/ZE/OE/UE/PE and
prior C1. Specific coverage checks ZE-only exact conversion, ZE plus current
PE/inexact and ZE plus current IE/invalid conversion. Consecutive stores retain
ZE without FNCLEX; FNSTSW observes it and FNCLEX clears it. Five-form write and
canonical-range fault checks preserve all six sticky flags, complete CPU state,
and memory. ZE plus each SF/ES/B reject is checked before access. Literal CLI
fixtures seed ZE alone. Unsupported FCW/source controls stay rejected, and
JIT/native admission remains denied.

`cargo test --workspace --offline` passed 958 unit/integration tests across
`od-host` (6), `od-isa` (554), `od-jit` (4), `od-runner` CLI (81), fixture/JIT
(138), `od-testkit` (166), `od-x86-runner` library (8), and binary (1); all
doc-tests passed. Host `cargo check --workspace --all-targets --offline` and
Linux x86_64 `cargo check --workspace --all-targets --offline --target
x86_64-unknown-linux-gnu` passed, as did `cargo fmt --all -- --check` and
`git diff --check`.
Linux check emitted the existing dead-code warnings in `od-host` and
`od-runner`. `tools/check_layout.py` reports the pre-existing unmapped `.serena`
directory, which is excluded from this task. The native-denial test is
architecture-gated and did not run on this arm64 host; the Linux x86_64
all-target check compiled it. `graphify update .` refreshed the graph to 4,538
nodes, 9,204 edges and 183 communities. Physical x86 checks remain deferred to
the combined final S2-31 substage under ISSUE-019. See the
[CPU contract](../specs/CPU_CONTRACT.md#local-s2-31cp-admit-historical-sticky-ze-bounded-integer-stores),
[plan](../plans/STAGE_2.md#s2-31cp-admit-historical-sticky-ze-on-bounded-integer-stores),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


<a id="s2-31cq--canonical-subnormal-m32-fist"></a>

## S2-31cq — canonical binary80 subnormals through m32 FIST/FISTP

The bounded DB10/DB18 m32 integer-store path now admits Special-tagged
canonical nonzero binary80 subnormals (exponent zero, J=0, nonzero fraction).
All such magnitudes are below one half. RN/TZ produce zero; RD produces -1
only for negative inputs; RU produces +1 only for positive inputs. Each
conversion sets PE, and current C1 is set exactly for a result magnitude of
one. No other current exception flags are added. The existing FCW whitelist,
FSW mask `0x80c0`, six historical sticky flags, Dword range preflight,
atomic one-write/no-read contract, and post-write FIST/FISTP commit behavior
remain in force. m16/m64 and pseudo-denormal/malformed sources remain closed.

The 65,536-case matrix spans the four requested fraction payloads, both signs,
four RC settings, both opcodes, all eight TOP positions, 64 combinations of
incoming IE/DE/ZE/OE/UE/PE, and both prior C1 values. It checks output bytes,
all FSW bits, raw payload/tag preservation, TOP behavior, and one Dword write
with no read. A literal FLD m80→FIST→FISTP unit composition checks status and
pop effects. Wrong tag/class and m16/m64 controls reject before access; write
and canonical-range faults preserve full state and memory. The new literal CLI
fixture executes FISTP/FNSTSW on a negative subnormal; the FLD m80 composition
also checks FNCLEX after status observation. JIT/native admission remains
denied.

`cargo test --workspace --offline` passed 962 tests: `od-host` (6), `od-isa`
(556), `od-jit` (4), `od-runner` CLI (81), fixture/JIT (140), `od-testkit`
(166), and `od-x86-runner` (8 library, 1 binary); all doc-tests passed.
Host and Linux x86_64 `cargo check --workspace --all-targets --offline`,
`cargo fmt --all -- --check`, and `git diff --check` passed. The graph refreshed
to 4,546 nodes, 9,217 edges and 181 communities. `tools/check_layout.py` reports
the unmapped `.serena/` session directory; it was preserved and excluded from
the commit. The native x87-denial test is
Linux-only and did not execute on this macOS arm64 host; the Linux all-target
check compiled the changed path. Physical x86 validation and native runtime
support remain deferred under ISSUE-019; S2-31 remains partial. See
the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-31cq-canonical-subnormal-m32-fist),
[plan](../plans/STAGE_2.md#s2-31cq-canonical-subnormal-m32-fist), and
[fixture notes](../../tests/fixtures/x86/stage2/README.md).

<a id="s2-31cr--canonical-subnormal-m16-fist"></a>

## S2-31cr — canonical binary80 subnormals through m16 FIST/FISTP

The bounded DF10/DF18 m16 integer-store path now admits Special-tagged
canonical nonzero binary80 subnormals. It reuses the m32 exact rounding rule:
RN/TZ produce zero; RD produces -1 only for negative input; RU produces +1
only for positive input. Every conversion adds PE and recomputes C1 from the
stored magnitude. The implementation leaves m64 subnormal sources closed and
retains the established FCW/FSW gates, canonical48 Word preflight, one
atomic write/no read, raw binary80 source bytes, and write-before-commit
ordering.

Coverage includes the 65,536-case matrix over four fractions, signs, RC,
FIST/FISTP, TOP, all six incoming sticky flags, and prior C1; FLD m80
composition through both stores with FNSTSW/FNCLEX; malformed/tag/class and
width controls; Word sentinels; and write/range atomicity. A literal
FISTP/FNSTSW fixture verifies negative round-down storage and status
observation. Interpreter success and JIT/native denial are asserted by the
fixture harnesses.

`cargo test --workspace --offline` passed all 967 unit, fixture, and doc-test
cases on the macOS arm64 host. This includes the 65,536-case m16 matrix, the
CLI fixture interpreter path, and the JIT denial check. The native x86
admission test is compiled only for Linux and did not run on this host.
`cargo check --workspace --all-targets --offline` passed on macOS arm64 and
for `x86_64-unknown-linux-gnu`; the Linux cross-check emitted existing
dead-code warnings in `od-host` and `od-runner`. `cargo fmt --all -- --check`
and `git diff --check` passed. Graphify refreshed to 4,556 nodes, 9,233 edges,
and 175 communities. Independent Sol-medium review found no defects and
passed the three new ISA tests and fixture/JIT checks. No Mac mini or physical
x86 acceptance was run; that remains deferred to the combined S2-31 substage
under ISSUE-019. See the
[CPU contract](../specs/CPU_CONTRACT.md#local-s2-31cr-canonical-subnormal-m16-fist),
[plan](../plans/STAGE_2.md#s2-31cr-canonical-subnormal-m16-fist), and
[fixture notes](../../tests/fixtures/x86/stage2/README.md).


<a id="s2-31cs--canonical-subnormal-m64-fistp"></a>

## S2-31cs — canonical binary80 subnormal through m64 FISTP

Only exact bare `DF 38` now admits Special-tagged canonical binary80
nonzero subnormals (exponent zero, J=0, nonzero fraction). The existing
integer-only conversion helper applies the bounded subnormal result: RN/TZ
store zero, RD stores all-ones only for negative input, and RU stores one only
for positive input. PE is always ORed after a successful Qword write; C1 is
recomputed from whether the rounded magnitude is one. Existing FCW and
`FSW & 0x80c0` gates remain in force, including preservation of all six sticky
flags and C0/C2/C3. `FIST m64` is not introduced. Pseudo-denormals, malformed
encodings, mismatched tags, and unmasked cases remain unsupported.

The 32,768-case ISA matrix uses four literal fractions, both signs, all four
RC values, all eight TOP positions, all 64 incoming IE/DE/ZE/OE/UE/PE
combinations, and both incoming C1 values. It checks literal Qwords, eight-byte
canaries, exactly one Qword write and no read, sticky/condition status
preservation, C1, TOP wrap/pop, FIP/FDP/RIP, and preservation of all raw x87
payloads. Additional checks reject wrong tags, pseudo-denormal, malformed and
zero operands plus disallowed controls/status. Denied writes and canonical48
range failures leave state and RAM unchanged. Existing m16/m32 subnormal
matrix and composition tests remain green.

The literal CLI fixture performs an unaligned `FLD m80`, changes RAX, executes
`FISTP m64`, observes PE/C1 with `FNSTSW AX`, then clears exception flags with
`FNCLEX`. It checks the all-ones Qword between sentinels, the raw popped
subnormal payload, the final TOP/tag/status, and unchanged input bytes. The
fixture schema recognizes only this exact composed byte sequence. JIT
translation and native x86 admission both fail closed for this fixture.

Validation on the local macOS arm64 host: `cargo test --workspace --offline`
passed all 970 tests, including 561 `od-isa`, 82 runner unit, 142 fixture
integration, and the other workspace targets. The S2-31cs matrix and CLI tests
also passed independently. `cargo check --workspace --all-targets --offline`,
the same check for `x86_64-unknown-linux-gnu`, and `cargo fmt --all -- --check`
passed. The Linux cross-check reports existing dead-code warnings in `od-host`
and `od-runner`. Linux native-denial tests were compile-checked but not run;
physical x86 and Mac mini acceptance were not run and remain owned by the
combined final S2-31 substage under ISSUE-019. No known issue was closed by
this local interpreter extension. See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-31cs-canonical-subnormal-m64-fistp),
[plan](../plans/STAGE_2.md#s2-31cs-canonical-subnormal-m64-fistp), and
[fixture notes](../../tests/fixtures/x86/stage2/README.md).

<a id="s2-31ct--special-tagged-pseudo-infinity-through-m32-fist"></a>

## S2-31ct — Special-tagged pseudo-infinity through m32 FIST/FISTP

The bounded DB10/DB18 m32 integer-store path now admits only Special-tagged
binary80 pseudo-infinity encodings with zero significand and all-one exponent
(raw sign/exponent `0x7fff` or `0xffff`, J=0). A successful conversion writes
Dword integer-indefinite `0x80000000`, adds current IE, clears C1, and adds no
current PE or other exception flags. Existing sticky flags and C0/C2/C3 are
preserved. The FCW whitelist, `FSW & 0x80c0` gate, Long64 bare RAX addressing,
canonical48 Dword range preflight, one atomic write/no read, raw source
preservation, and post-write commit/FISTP pop behavior remain unchanged.
Canonical signed zero, canonical infinity with J=1 and pseudo-NaNs stay
separate; m16/m64 and other unlisted classes remain unsupported. FLD m80
pseudo-infinity composition is excluded because FLD rejects this encoding.

The literal 16,384-case ISA matrix spans both raw sign/exponent values, all four
FCWs, DB10/DB18, all TOP values, all 64 incoming sticky-flag combinations and
both prior C1 values. It checks literal Dword bytes, IE without added PE,
condition/sticky status preservation, source payloads, canaries and TOP/tag
behavior. Additional ISA tests reject adjacent pseudo-NaNs and wrong tags,
prove m16/m64 remain excluded, exercise FNSTSW/FNCLEX, and verify denied-write
and canonical-range atomicity. The literal CLI fixture executes FISTP and
FNSTSW on a negative pseudo-infinity, checks the Dword canaries and status,
and confirms JIT denial. Native x86 admission remains fail-closed for bounded
x87; the native-denial test was compile-checked on Linux but not run on this
macOS arm64 host.

Validation: `cargo test --workspace --offline` passed all workspace unit,
integration, and doc-test targets. Focused ISA and CLI tests passed.
Host and `x86_64-unknown-linux-gnu` `cargo check --workspace --all-targets
--offline`, `cargo fmt --all -- --check`, and `git diff --check` passed. The
Linux cross-check emitted the existing dead-code warnings in `od-host` and
`od-runner`. Graphify refreshed to 4,572 nodes, 9,259 edges and 179
communities. Physical x86 and Mac mini acceptance remain deferred to the
combined final S2-31 substage under ISSUE-019; this local extension does not
close ISSUE-019. See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-31ct-pseudo-infinity-m32-fist),
[plan](../plans/STAGE_2.md#s2-31ct-pseudo-infinity-m32-fist), and
[fixture notes](../../tests/fixtures/x86/stage2/README.md).


<a id="s2-31cu--special-tagged-pseudo-infinity-through-m16-fist"></a>

## S2-31cu — Special-tagged pseudo-infinity through m16 FIST/FISTP

Only bare DF10/DF18 m16 integer stores now admit Special-tagged binary80
pseudo-infinity with raw sign/exponent `0x7fff` or `0xffff` and zero
significand. Success writes Word integer-indefinite `0x8000`, adds IE, clears
C1, and adds no PE or other current exception flag. Historical sticky flags
and C0/C2/C3 remain preserved. The FCW whitelist, `FSW & 0x80c0` gate, bare
Long64 RAX form, canonical48 Word preflight, one atomic write/no read, raw
source preservation, and write-before-commit/FISTP pop behavior are unchanged.
The shared preflight explicitly retains m32 behavior while m64 pseudo-infinity
remains unsupported. Canonical signed zero and J=1 infinity remain distinct;
FLD pseudo-infinity composition is excluded.

The literal 16,384-case ISA matrix spans both sign/exponent encodings, all four
FCWs, DF10/DF18, all TOP values, 64 incoming sticky combinations and both
prior C1 values. Tests verify literal Word bytes/status, IE without PE, C1
clearing, preservation of sticky/C0/C2/C3, source bytes, canaries, FIST/FISTP
tag behavior and TOP wrap. Additional ISA coverage rejects pseudo-NaNs, wrong
tags, m64, and forbidden controls/status; denied-write and canonical-range
tests prove atomicity. FNSTSW/FNCLEX and a direct literal CLI fixture cover
observation and clearing. The CLI checks the two-byte store/canaries and JIT
denial; native admission denial is compile-checked on Linux x86_64 but was not
run on this macOS arm64 host.

Validation: focused ISA and CLI runtime tests and
`cargo test --workspace --offline` (including all unit, integration, and
documentation tests) passed. Host and
`x86_64-unknown-linux-gnu` `cargo check --workspace --all-targets --offline`,
`cargo fmt --all -- --check`, and `git diff --check` passed. The Linux
cross-check emitted the existing dead-code warnings in `od-host` and
`od-runner`. `cargo test -p od-x86-runner --target
x86_64-unknown-linux-gnu --offline --no-run` could not link on this macOS host:
its `cc`/Darwin `ld` reject Linux linker options. The cross-target all-targets
check compile-checked the native-denial test, but Linux execution remains
unverified here. `graphify update .` refreshed the graph to 4,584 nodes,
9,276 edges, and 173 communities. Physical x86 runtime and Mac mini acceptance
remain deferred to the combined final S2-31 substage under ISSUE-019; this
extension does not close ISSUE-019. See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-31cu-pseudo-infinity-m16-fist),
[plan](../plans/STAGE_2.md#s2-31cu-pseudo-infinity-m16-fist), and
[fixture notes](../../tests/fixtures/x86/stage2/README.md).


<a id="s2-31cv--special-tagged-pseudo-infinity-through-m64-fistp"></a>

## S2-31cv — Special-tagged pseudo-infinity through m64 FISTP

Only bare `DF 38` now admits a Special-tagged binary80 pseudo-infinity with
zero significand and raw sign/exponent `0x7fff` or `0xffff`. It stores Qword
integer-indefinite `0x8000000000000000`, adds IE, clears C1, adds no new PE or
other current flags, and preserves historical sticky/condition flags. The
existing FCW and FSW gates, bare Long64 RAX form, canonical48 eight-byte
preflight, one atomic Qword write/no read, and write-before-commit order are
unchanged. FISTP preserves the source's ten raw bytes, empties old physical
TOP, and advances TOP once, including wrap.

The 8,192-case matrix crosses both signs, four FCWs, all eight TOP positions,
64 sticky combinations, and both incoming C1 values. Assertions check the
literal Qword output/status, PE preservation without adding it, IE, C1 clearing,
source preservation, tag/pop behavior, TOP wrap, canaries and exactly one
Qword write without a read. Rejection tests cover pseudo-NaNs, unnormals,
pseudo-denormals, mismatched tags and denied FCW/FSW controls. Canonical signed
zero and J=1 infinity retain their distinct existing result/status behavior.
Write-denial and canonical48-crossing cases remain atomic. A direct seeded CLI
fixture composes `DF 38`, `FNSTSW AX`, and `FNCLEX`; it checks the Qword canaries,
sticky IE observation, clearing and popped tag. JIT compilation and native
admission remain denied. FLD composition is excluded.

Validation: the two focused ISA tests and seeded CLI fixture test passed;
`cargo test --workspace --offline` passed. Host and
`x86_64-unknown-linux-gnu` `cargo check --workspace --all-targets --offline`,
`cargo fmt --all -- --check`, and `git diff --check` passed. The Linux
all-target check compile-checks the native-denial test; Linux execution was not
performed on this macOS arm64 host. `python3 tools/check_layout.py` reports the
pre-existing unmapped `.serena/` directory; it was left untouched. `graphify
update .` refreshed the graph to 4,592 nodes, 9,288 edges and 181 communities.
Mac mini validation, physical x86 acceptance and native runtime remain deferred
to the combined final S2-31 substage under ISSUE-019. See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-31cv-pseudo-infinity-m64-fistp),
[plan](../plans/STAGE_2.md#s2-31cv-pseudo-infinity-m64-fistp), and
[fixture notes](../../tests/fixtures/x86/stage2/README.md).


<a id="s2-31cw--special-tagged-pseudo-nans-through-m32-fist"></a>

## S2-31cw — Special-tagged pseudo-NaNs through m32 FIST/FISTP

Only bare DB 10 and DB 18 now admit Special-tagged binary80 pseudo-NaNs with
exponent `0x7fff`, J=0 and nonzero fraction. Both Q-bit variants and either
sign store Dword integer-indefinite `0x80000000`, add IE without PE or other
current flags, and clear C1. Historical sticky flags and C0/C2/C3 remain
preserved. The existing FCW/FSW gates, bare RAX form, canonical48 Dword
preflight, one-write/no-read ordering, raw ten-byte source and FIST/FISTP
TOP behavior are unchanged. m16/m64 were closed at the S2-31cw boundary;
S2-31cx below later admits the bounded m16 cases. m64, FLD and unmasked
behavior remain closed.

The independent expected-value matrix crosses four literal fractions, both
signs, four FCWs, both forms, all TOP values, 64 sticky combinations and both
prior C1 values (65,536 cases). Additional tests preserve the existing
pseudo-infinity and canonical J=1 QNaN/SNaN behavior, reject wrong tags,
unnormals, pseudo-denormals, unsupported FCW/FSW and m16/m64 pseudo-NaNs at
the original S2-31cw boundary, and check Dword write/canonical-range atomicity.
An ISA DB18/FNSTSW/FNCLEX
composition observes IE then clears it; the direct CLI fixture is limited to
DB18/FNSTSW because the fixture schema admits one x87 instruction or the exact
two-instruction status pair. JIT compilation and native admission remain
denied. FLD composition is not claimed because FLD rejects pseudo-NaNs.

Validation: all 19 m32 non-pop FIST tests and 16 m32 FISTP tests passed, along
with the direct CLI fixture and ISA FNSTSW/FNCLEX composition. The complete
offline workspace suite passed (988 unit/integration tests plus doc-test
targets). Host and `x86_64-unknown-linux-gnu` workspace all-targets checks,
`cargo fmt --all -- --check`, and `git diff --check` passed. The Linux
cross-check reports existing dead-code warnings in `od-host` and `od-runner`.
`python3 tools/check_layout.py` still reports the pre-existing unmapped
`.serena/` directory, which was left untouched. `graphify update .` refreshed
the graph to 4,602 nodes, 9,307 edges and 181 communities. A Linux-target test
binary could not be linked using the macOS host linker; Linux native-denial
runtime remains unverified on this macOS arm64 host. Physical x86 and Mac mini
acceptance remain deferred to the combined final S2-31 substage under ISSUE-019.
See the
[CPU contract](../specs/CPU_CONTRACT.md#local-s2-31cw-special-pseudo-nan-m32-fist),
[plan](../plans/STAGE_2.md#s2-31cw-special-pseudo-nan-m32-fist), and
[fixture notes](../../tests/fixtures/x86/stage2/README.md).


<a id="s2-31cx--special-tagged-pseudo-nans-through-m16-fist"></a>

## S2-31cx — Special-tagged pseudo-NaNs through m16 FIST/FISTP

Only bare `DF 10` and `DF 18` now admit Special-tagged binary80 pseudo-NaNs
(exponent `0x7fff`, J=0, nonzero fraction). Either sign and both bit-62
variants store Word integer-indefinite `0x8000`, add IE without PE or other
new flags, and clear C1. Historical sticky flags and C0/C2/C3 remain preserved.
The existing FCW/FSW gates, bare RAX form, canonical48 two-byte preflight,
one-write/no-read order, raw source bytes and FIST/FISTP TOP behavior remain.
The m32 pseudo-NaN path is unchanged. Wrong tags/classes, non-all-one
exponents, m64 pseudo-NaNs, FLD and unmasked delivery remain outside the slice.

The independently expected 65,536-case matrix crosses four nonzero fractions,
both signs, four FCWs, both forms, all TOP values, 64 sticky combinations and
both old C1 states. Additional checks cover pseudo-infinity and canonical
J=1 QNaN/SNaN controls, tag/class/width/control rejection, write/range
atomicity, Word canaries, FNSTSW/FNCLEX composition, a literal seeded CLI
fixture, and JIT/native denial. Physical x86 acceptance and Mac mini validation
remain deferred to the combined final S2-31 substage under ISSUE-019.

Validation: focused ISA matrix/rejection and status-composition tests passed.
`cargo test --workspace --all-targets --offline --quiet` passed all 993
unit/integration tests. `cargo test --workspace --doc --offline --quiet` passed
all 7 doc-test targets; they contain zero examples. Host and
`x86_64-unknown-linux-gnu` `cargo check --workspace --all-targets --offline`
passed; Linux emitted existing dead-code warnings in `od-host` and `od-runner`.
Linux native-denial runtime execution remains unverified on this macOS arm64
host; only the Linux cross-target compile was run. `cargo fmt --all -- --check`,
`git diff --check`, and
`python3 tools/check_layout.py` passed. `graphify update .` completed with
4,612 nodes, 9,326 edges and 183 communities; its extractor reported the
existing parse warning for `include/open_desktop/host_abi.h` (no symbols from
that header). `.serena/` remains a local-tooling exception and no such paths
were changed or staged. Independent review completed with no findings. Physical Mac mini
validation remains deferred to the combined final S2-31 substage under
ISSUE-019.


<a id="s2-31cy--special-tagged-pseudo-nan-through-m64-fistp"></a>

## S2-31cy — Special-tagged pseudo-NaN through m64 FISTP

Only bare `DF 38` now accepts a Special-tagged binary80 pseudo-NaN with
exponent `0x7fff`, J=0, and nonzero fraction. Either bit-62 variant and sign
stores Qword integer-indefinite `0x8000000000000000`, adds sticky IE, clears C1,
and adds no PE or other new flags. Historical sticky flags and C0/C2/C3 remain
preserved. The existing FCW/FSW admission gate, bare RAX addressing, full
canonical48 eight-byte preflight, one atomic Qword write/no read, raw source
retention, and write-before-commit FISTP pop behavior remain in force. Existing
zero, pseudo-infinity and canonical J=1 infinity/QNaN/SNaN paths retain their
separate behavior; m16/m32 behavior is unchanged. FLD, unmasked delivery, and
generic unsupported-class fallback are not admitted.

The independent literal matrix crosses four fractions, both signs, four FCWs,
all eight TOP values, 64 sticky combinations and both prior C1 values (32,768
cases). It checks literal Qword bytes and status, source payload, canaries,
one write/no read, IE without newly set PE, C1 clearing and TOP/tag pop wrap.
Malformed classes/tags, other exponents, controls, canonical-range and denied
write atomicity are covered. The seeded negative pseudo-QNaN fixture observes
IE using FNSTSW, clears it using FNCLEX, and preserves the source payload and
memory canaries. The JIT fixture check and native admission check remain
fail-closed.

Validation: `cargo test --workspace --offline` passed (998 unit/integration
tests plus doc-test targets). Host and
`x86_64-unknown-linux-gnu` `cargo check --workspace --all-targets --offline`
passed. The Linux cross-check reports existing dead-code warnings in `od-host`
and `od-runner`; the native-denial test is Linux x86_64 gated and did not run on
this Mac host. The focused ISA matrix, status-composition sequence, seeded CLI
fixture, JIT denial check, `cargo fmt --all -- --check`, `git diff --check`, and
`python3 tools/check_layout.py` passed. Graphify was refreshed to 4,622 nodes,
9,345 edges and 180 communities. Mac mini validation, physical x86 acceptance,
and native runtime execution remain deferred to the combined final S2-31
substage under ISSUE-019.
See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-31cy-special-pseudo-nan-m64-fistp),
[plan](../plans/STAGE_2.md#s2-31cy-special-pseudo-nan-m64-fistp), and
[fixture notes](../../tests/fixtures/x86/stage2/README.md).


<a id="s2-31cz--special-tagged-unnormals-through-m32-fist"></a>

## S2-31cz — Special-tagged unnormals through m32 FIST/FISTP

Only bare `DB 10` and `DB 18` now admit a Special-tagged binary80 unnormal
with biased exponent 1 through `0x7ffe`, J=0, either sign, and any significand
fraction including zero. The conversion stores Dword integer-indefinite
`0x80000000`, adds IE, clears C1, and adds no PE/DE/UE/OE/ZE/SF/ES/B. Incoming
sticky flags and C0/C2/C3 are preserved. Existing FCW/FSW gates, bare RAX
addressing, canonical48 Dword preflight, one atomic write/no read, raw source
bytes, write-before-commit, and FIST/FISTP TOP behavior remain in force.

The independent matrix covers 3 exponents × 4 fractions × both signs × 4 FCWs
× both forms × 8 TOP values × 64 sticky combinations × both old C1 values
(196,608 cases). Explicit controls cover E=0 subnormal, E=0/J=1
pseudo-denormal rejection, J=1 finite Normal, and E=0x7fff canonical infinity,
plus malformed tags, excluded m16/m64 forms, controls/status, write denial and
canonical-range crossing. A seeded `DB 18` CLI fixture checks store bytes,
canaries, IE, original raw bytes and TOP pop. A separate ISA sequence checks
FNSTSW observation and FNCLEX clearing; the fixture intentionally stays a
single exact x87 operation under the existing fixture schema. `od_jit::compile`
and native admission remain denied. No FLD composition is claimed.

Validation passed: focused unnormal ISA tests (matrix, boundaries, rejections,
fault atomicity and FNSTSW/FNCLEX), the seeded CLI fixture, and
`cargo test --workspace --offline`. Host
`cargo check --workspace --all-targets --offline` passed. Linux x86_64
`cargo check --workspace --all-targets --offline --target x86_64-unknown-linux-gnu`
passed with existing dead-code warnings in `od-host` and `od-runner`; the
Linux-gated native admission test was compiled but not executed on this Mac
host. `cargo fmt --all -- --check`, `git diff --check`, and
`python3 tools/check_layout.py` passed. Graphify refreshed to 4,632 nodes,
9,366 edges and 183 communities; its existing parse warning for
`include/open_desktop/host_abi.h` remained. Physical x86/Mac mini acceptance and
native runtime execution remain deferred to the combined final S2-31 substage
under ISSUE-019. See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-31cz-special-unnormals-m32-fist),
[plan](../plans/STAGE_2.md#s2-31cz-special-unnormals-m32-fist), and
[fixture notes](../../tests/fixtures/x86/stage2/README.md).

<a id="s2-31da--special-tagged-unnormals-through-m16-fist"></a>

## S2-31da — Special-tagged unnormals through m16 FIST/FISTP

Only bare `DF 10`/`DF 18` now admit Special-tagged binary80 unnormals with
biased exponent 1..=`0x7ffe`, J=0, either sign and any fraction. Masked
unsupported-format conversion stores Word integer-indefinite `0x8000`, adds IE,
clears C1, and adds no PE/DE/UE/OE/ZE/SF/ES/B. Incoming sticky status and
C0/C2/C3, raw ten-byte source and the established write-before-commit behavior
are preserved; C1 is cleared. FIST retains TOP; FISTP pops after a successful
store.

The independent matrix covers 196,608 inputs across exponents 1/`0x3fff`/
`0x7ffe`, four fractions including zero, signs, all FCWs, both forms, TOP,
sticky combinations and prior C1. Checks also cover boundaries and adjacent
encodings, malformed tags, excluded widths, control/status gates, IE without
PE/DE, raw bytes, canaries, FNSTSW/FNCLEX, denied-write and canonical48 atomicity,
a seeded `DF 18` CLI fixture, and JIT/native denial. No FLD composition is
claimed.

Validation passed: all 1,010 workspace tests, including the 592 `od-isa`
tests and 89 CLI tests; focused unnormal coverage; host and Linux
`cargo check --workspace --all-targets --offline`; the seeded CLI fixture; fmt,
diff and layout checks. The native-admission regression is platform-gated and
compiled but was not run on this macOS host. Independent gpt-6.1-sol review was
clean. Physical x86 and Mac mini acceptance remain deferred to the combined
final S2-31 substage under ISSUE-019. See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-31da-special-unnormals-m16-fist),
[plan](../plans/STAGE_2.md#s2-31da-special-unnormals-m16-fist), and
[fixture notes](../../tests/fixtures/x86/stage2/README.md).
<a id="s2-31db--special-tagged-unnormals-through-m64-fistp"></a>

## S2-31db — Special-tagged unnormals through m64 FISTP

The interpreter's bounded m64 integer-store conversion now admits only
Special-tagged binary80 unnormals with finite nonzero exponent and J=0 for bare
`DF 38`. It stores Qword integer-indefinite, sets IE, clears C1, and keeps the
raw x87 payload. The existing preflight and write-before-commit path remains in
force: a successful single Qword write precedes FIP/FDP/RIP, TOP and tag commit.
The independent matrix covers 98,304 exponent/fraction/sign/FCW/TOP/sticky/C1
states; a literal CLI fixture checks status, payload, memory and pop behavior.

Validation passed: `cargo test --workspace --offline` ran 1,014 tests with no
failures, including 595 `od-isa`, 90 runner, 144 fixture integration, 166
`od-testkit`, 8 x86-runner supervisor and 1 REP trace tests. The focused
unnormal matrix/rejection/status tests and seeded CLI fixture also passed.
Host and `x86_64-unknown-linux-gnu` `cargo check --workspace --all-targets
--offline`, `cargo fmt --all -- --check`, `python3 tools/check_layout.py`, and
`git diff --check` passed. The native-admission regression compiled, but its
x86-only test was platform-gated and ran zero tests on this Apple Silicon host.
Graphify was refreshed to 4,653 nodes, 9,406 edges and 182 communities; its
existing parser warning for `include/open_desktop/host_abi.h` remains.
JIT/native execution remain denied.
Physical x86 and Mac mini acceptance are deferred to the combined final S2-31
substage under ISSUE-019. See the [CPU
contract](../specs/CPU_CONTRACT.md#local-s2-31db-special-unnormals-m64-fistp),
[plan](../plans/STAGE_2.md#s2-31db-special-unnormals-m64-fistp), and [fixture
notes](../../tests/fixtures/x86/stage2/README.md).

## S2-31di — small canonical-Normal values through bounded FRNDINT

Bare register-only `D9 FC` now admits a Valid canonical-Normal old physical
TOP with unbiased exponent `-16382..=-1`. The comparison-based branch avoids
oversized shifts: nearest-even returns magnitude one only above the exact
halfway significand at `e=-1`; directed modes return one only when sign and
direction move away from zero. Every admitted value adds PE, C1 reports the
magnitude-one choice, and signed zero outputs receive `X87Tag::Zero`. Existing
control/status gates, sticky IE/PE behavior and all unrelated state remain
unchanged.

The local matrix crosses both signs, all four FCWs, all TOPs, `e=-1,-2,-63,
-64,-65,-16382`, three significand patterns, sticky IE/PE combinations and
prior C1. It checks nearest half and adjacent values, values below one,
Valid→Zero/Valid tags, full state preservation, atomic invalid-tag/environment
and execution-boundary rejects, and repeated FRNDINT after rounding to zero.
The literal `s2_31di_frndint_min_normal_to_negative_zero_fnstsw.case.json`
checks minimum-normal input, negative-zero bytes/tag, FNSTSW status, pointer
preservation and retirement. JIT/native denial and no-data-memory behavior
remain covered by existing admission/execution checks. This slice does not
admit subnormal/pseudo-denormal/malformed/tag-mismatched sources or unmasked
exceptions.

Validation passed: focused `cargo test -p od-isa --offline frndint` ran 16
tests, and the new literal CLI test passed. `cargo test --workspace --offline`
passed all 1,032 tests: 6 host, 606 od-isa, 4 JIT, 96 runner unit, 145 fixture,
166 testkit, 8 x86-runner supervisor and 1 REP trace tests. Host and
`x86_64-unknown-linux-gnu` `cargo check --workspace --all-targets --offline`
passed; the Linux cross-check reports existing dead-code warnings in od-host
and od-runner. `cargo fmt --all -- --check`, `git diff --check`, and
`python3 tools/check_layout.py` passed. The x86-only native-denial runtime test
was not executed on this Apple Silicon host; its target compiled in the Linux
cross-check. Graphify refreshed the code graph to 4,700 nodes, 9,486 edges and
174 communities; existing extractor warnings remain for
`include/open_desktop/host_abi.h` and files with no supported symbols.
Physical x86/Mac mini acceptance remains deferred to the combined final S2-31
substage under ISSUE-019. See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-31di-small-canonical-normal-frndint-d9-fc),
[plan](../plans/STAGE_2.md), and [fixture notes](../../tests/fixtures/x86/stage2/README.md).

## S2-31dl — historical sticky OE through bounded FRNDINT

Changed only the bare `D9 FC` status gate from `FSW & 0x80cc == 0` to
`FSW & 0x80c4 == 0`. OE is admitted and preserved with IE/DE/UE/PE, without
creating PE by itself. The independent interpreter matrix spans exact and
fractional Normal plus canonical SNaN, both signs, all four FCWs, every TOP,
all IE/DE/UE/PE combinations and old C1. It verifies exact/identity PE
preservation, current inexact PE, SNaN IE without PE, metadata and result
payloads; ZE/SF/ES/B rejects remain atomic. A separate ISA sequence checks
FST m32 overflow produces OE+PE+C1 from FSW=0, then FRNDINT preserves OE+PE
and clears C1. Literal CLI fixtures cover seeded OE and
`FST m32fp overflow → FRNDINT → FNSTSW AX` over `(2 − 2^-63) × 2^127`
with final OE+PE visible. Identity
smoke covers signed zero, large integral Normal, infinity and QNaN.

Focused `cargo test -p od-isa --offline frndint` passed all 20 matching tests;
the seeded/composition CLI test and sticky-OE JIT-denial test also passed.
`cargo fmt --all -- --check`, `cargo check --workspace --all-targets --offline`,
`python3 tools/check_layout.py`, and `git diff --check` passed. Graphify refreshed
the repository graph to 4,730 nodes, 9,533 edges and 176 communities; the
existing extractor warnings remain for the public ABI header and unsupported
or symbol-free files. Independent Sol-medium review found the initial fixture
provenance misstated the source value; it was corrected to `(2 − 2^-63) × 2^127`.
The existing `d910d9fcdfe0` whitelist entry is reused without widening schema.
The follow-up Sol-medium review found no remaining issues. Physical x86 and Mac
mini acceptance remain deferred to the combined final S2-31 substage under
ISSUE-019; native/JIT remain closed.
See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-31dl-historical-sticky-oe-in-bounded-frndint-d9-fc),
[plan](../plans/STAGE_2.md#s2-31dl-admit-historical-sticky-oe-to-bounded-frndint-d9-fc),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).

## S2-31dj — historical sticky DE through bounded FRNDINT

Changed only the bare `D9 FC` status gate from `FSW & 0x80de == 0` to
`FSW & 0x80dc == 0`. Historical DE now passes through and remains sticky;
the four accepted all-masked FCW values and current rounding, PE generation,
SNaN quieting/IE, C1 and metadata behavior are unchanged. ZE/OE/UE/SF/ES/B
remain rejected when combined with admitted IE/DE/PE. JIT/native admission
remains closed.

The historical-status source-class matrix now crosses IE/DE/PE independently
with sign, FCW, TOP and prior C1 over zero, exact/inexact and large Normal,
infinity, QNaN and SNaN. A separate exponent matrix covers minimum Normal,
the e=-1 and e=0 rounding boundaries, e=1, e=63, and maximum finite Normal
across sign/FCW/TOP/sticky flags/C1; current inexact sources continue to add
PE. Every forbidden status bit is tested with IE and DE combinations and
must reject atomically. Two literal CLI fixtures run minimum-subnormal
`FLD m32fp` or `FLD m64fp`, `FRNDINT`, and `FNSTSW AX`: FLD first creates a
canonical-Normal binary80 stack value and DE, and FRNDINT returns positive
zero while preserving DE and setting PE; `FNSTSW` observes `0x7d22`.
The seeded `D9 FC DF E0` CLI fixture separately starts with IE/DE/PE set and
the all-masked `0x037f` FCW, confirming DE survives and prior C1 is
recomputed. The x87-state profile was extended only for the two exact FLD
compositions.

Focused validation passed: `cargo test -p od-isa --offline frndint` ran 17
tests, and `cargo test -p od-runner --offline local_x87_frndint_` passed all
four matching tests, including both FLD fixture variants and the seeded raw
fixture. Formatting, workspace check, layout and diff checks passed; the graph
was refreshed to 4,710 nodes, 9,502 edges and 183 communities (the existing
header parser warning remains). Independent `gpt-6.1-sol` medium review
approved with no concrete defects. Physical x86 and
Mac mini acceptance remain deferred to the combined final S2-31 substage under
ISSUE-019. See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-31dj-historical-sticky-de-in-bounded-frndint-d9-fc),
[plan](../plans/STAGE_2.md), and [fixture notes](../../tests/fixtures/x86/stage2/README.md).

## S2-31dk — historical sticky UE through bounded FRNDINT

Changed only the bare `D9 FC` status gate from `FSW & 0x80dc == 0` to
`FSW & 0x80cc == 0`. Historical UE now passes through and remains sticky with
IE/DE/PE. It does not itself generate PE: exact and identity sources retain
prior PE, current inexact Normal rounding sets PE, and current canonical SNaN
quieting sets IE without PE. The all-masked FCWs, source/tag domains, rounding,
C1, state transitions and metadata are unchanged. ZE/OE/SF/ES/B and unsupported
classes remain atomic rejects; JIT/native stay closed.

The independent interpreter matrix covers exact Normal, inexact Normal and
canonical SNaN across all TOPs, FCWs, incoming IE/DE/PE combinations and prior
C1. Forbidden status rejects are paired with UE and admitted sticky bits. The
literal seeded `D9 FC DF E0` fixture starts with exact `1.0` and UE only, then
observes UE without PE. The literal `D9 10 D9 FC DF E0` fixture begins with
1.5 times the minimum binary32 subnormal: FST m32 creates UE+PE, FRNDINT rounds
the same value to zero and preserves both, and FNSTSW observes `0x0030`.

Focused `cargo test -p od-isa --offline frndint` passed all 18 matching tests,
and `cargo test -p od-runner --offline
local_x87_frndint_admits_seeded_ue_and_underflow_composition` passed its CLI
fixture test. `cargo fmt --all -- --check`,
`cargo check --workspace --all-targets --offline`,
`python3 tools/check_layout.py`, and `git diff --check` passed. Graphify refreshed
the graph to 4,719 nodes, 9,515 edges and 182 communities; existing extractor
warnings remain for the public ABI header and files with no supported symbols.
Independent Sol-medium review found no remaining concrete defects after
correcting the composition fixture to begin with FSW=0; its focused CLI test was
rerun and passed. Physical x86 and Mac mini acceptance remain deferred to the
combined final S2-31 substage under ISSUE-019. See the
[CPU contract](../specs/CPU_CONTRACT.md#local-s2-31dk-historical-sticky-ue-in-bounded-frndint-d9-fc),
[plan](../plans/STAGE_2.md), and [fixture notes](../../tests/fixtures/x86/stage2/README.md).

## S2-31dm — historical sticky ZE through bounded FRNDINT

Changed only the bare `D9 FC` interpreter gate from `FSW & 0x80c4 == 0` to
`FSW & 0x80c0 == 0`. Historical ZE now passes through and remains sticky;
FRNDINT itself does not produce ZE. Exact input adds no PE, fractional input
adds PE, and current SNaN quieting adds IE without PE. Existing IE/DE/OE/UE/PE
stickies remain independent. The four-FCW whitelist, source/tag domains,
rounding, metadata, C1 behavior and JIT/native admission are unchanged. No
FDIV-zero producer or producer-domain change was made.

The existing FRNDINT matrix now crosses ZE with all 32 combinations of
IE/DE/OE/UE/PE across source class, sign, all four RCs, every TOP and prior C1.
SF/ES/B remain atomic rejects. A literal seeded `D9 FC DF E0` CLI fixture
starts with canonical Normal `1.0` and ZE+OE (`FSW=0x000c`), and asserts exact
`FNSTSW AX=0x000c`; its provenance explicitly says ZE is seeded and no guest
instruction produces it.

Verification passed: `cargo test -p od-isa --offline frndint` (20 matching
tests), the exact seeded CLI fixture test,
`cargo fmt --all -- --check`,
`cargo check --workspace --all-targets --offline`,
`python3 tools/check_layout.py`, `git diff --check`, and `graphify update .`.
Independent `gpt-6.1-sol` medium review found two gaps: the SF/ES/B reject
inputs did not include ZE, and fixture specification metadata still named
S2-31dl. Both were corrected; targeted re-review found no remaining concrete
issues. Physical x86 and Mac mini acceptance remain deferred to the combined
final S2-31 substage under ISSUE-019. See the
[CPU contract](../specs/CPU_CONTRACT.md#local-s2-31dm-historical-sticky-ze-in-bounded-frndint-d9-fc),
[plan](../plans/STAGE_2.md#s2-31dm-admit-historical-sticky-ze-to-bounded-frndint-d9-fc),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).


## S2-31dn — historical sticky SF with IE through bounded FRNDINT

The bare register-only `D9 FC` gate now uses the base predicate
`FSW & 0x8080 == 0` plus a guard that rejects SF when IE is absent. This admits
historical SF only together with IE; both remain sticky. Historical DE/ZE/OE/UE/PE
are independently admitted. Current rounding and current exception behavior,
source/tag domains, exact FCWs, and metadata are unchanged; C1 is recomputed from
the current result. SF-only, ES and B remain atomic rejects, including SF-only
with current SNaN. Empty/mismatched/raw-malformed sources remain rejected.
JIT/native and pending/#MF/WAIT behavior are unchanged.

The independent matrix covers 32,768 combinations of SF+IE, 32 DE/ZE/OE/UE/PE
subsets, eight source classes, both signs, four FCWs, all TOP values and prior
C1. The seeded project_fixture `D9 FC DF E0` case starts from source `1.0`,
FCW `0x037f`, and FSW `0x0241`; FRNDINT clears prior C1 but preserves SF+IE,
so FNSTSW AX returns `0x0041`. Its provenance explicitly records that IE+SF
are set in the initial snapshot, not produced by the guest, and it is not native
evidence. Existing SNaN re-entry/FNCLEX coverage is retained. The normative
basis is Intel SDM revision 093 Vol.1 §8.1.3.4 p.8-6 and §8.5.1.1 p.8-26,
and Vol.2A FRNDINT p.3-372.

Focused verification passed: `cargo test -p od-isa --offline frndint` ran 21
matching tests; `cargo test -p od-runner --offline frndint` ran 17 matching
tests (including the seeded CLI and JIT-denial fixtures). Formatting,
`cargo check --workspace --all-targets --offline`, `python3 tools/check_layout.py`
and `git diff --check` passed. `graphify update .` refreshed the graph to 4,743
nodes, 9,552 edges and 176 communities; existing extraction warnings remain for
the public ABI header and files without supported symbols. Independent
Sol-medium review found three coverage/documentation issues; all were corrected
and the follow-up review found no remaining issues. The x86-only native-admission
runtime test was not run on this Apple Silicon host. Physical x86/Mac mini
acceptance remains deferred to the combined final S2-31 substage under ISSUE-019.
## S2-31do — canonical binary80 subnormal through bounded FRNDINT

Bare Long64 `D9 FC` now admits only a Special-tagged physical ST(0) with
canonical binary80 subnormal bytes (exponent zero, J=0, nonzero fraction).
All inputs are below one-half: RN/RZ return signed zero, RD returns -1 only
for negative inputs, and RU returns +1 only for positive inputs. Each admitted
operation sets sticky DE and PE; C1 is set only for ±1. Commits set the result
tag to Zero or Valid as appropriate while preserving other flags, tags,
registers, TOP, FDP/FOP and unrelated state; FIP receives the instruction's
old RIP. Current DE passes through the
existing FRNDINT preflight/commit tuple. Pseudo-denormal, malformed, mismatch,
control, status and execution-boundary rejects remain atomic; JIT/native remain
denied.

Local coverage crosses minimum/maximum and two interior fractions with sign,
four RC modes, all TOP values, prior DE/PE/C1 and legal IE/SF combinations,
checking full state canaries and no memory access. Repeat FRNDINT, FNSTSW AX,
FNCLEX and zero re-entry are covered. The seeded literal
`s2_31do_frndint_seeded_subnormal_fnstsw.case.json` is explicitly a
`project_fixture`; its initial state is not native evidence.

Validation passed: focused `cargo test -p od-isa --offline frndint` (23 tests),
the new 6,144-case subnormal matrix/reject test, and the focused
`od-runner` seeded-fixture test. A literal CLI interpreter run retired two
instructions and observed AX/FSW `0x6d23`, positive zero, and Zero tag.
`cargo fmt --all -- --check`, `cargo check --workspace --all-targets --offline`,
`python3 tools/check_layout.py`, `git diff --check`, and `graphify update .`
passed. No Mac mini, SSH or physical x86 run was performed; those remain
part of the combined final S2-31 acceptance substage under ISSUE-019. See the
[CPU contract](../specs/CPU_CONTRACT.md#local-s2-31do-canonical-binary80-subnormal-frndint-d9-fc),
[plan](../plans/STAGE_2.md#s2-31do-canonical-binary80-subnormal-through-bounded-frndint),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-31do-frndint-canonical-subnormal-fixture).

## S2-31dp — Special-tagged binary80 pseudo-denormal through bounded FRNDINT

The exact Special-tagged finite branch of bare Long64 `D9 FC` now also admits
binary80 pseudo-denormals (biased exponent zero, J=1, any fraction including
zero). This uses the Intel-defined effective exponent -16382 and the existing
tiny arithmetic behavior: RN/RZ return signed zero; RD returns -1 for negative
and +0 for positive; RU returns +1 for positive and -0 for negative. Each
operation adds sticky DE+PE, sets C1 only for ±1, and writes a Zero/Valid result
tag. The existing FCW/status gates, decoder/classifier, preflight/commit tuple,
metadata and register-only boundary are unchanged. Pseudo-infinity/NaN,
malformed bytes, tag mismatches and wrong tags remain atomic rejects. At the
S2-31dp stage unnormal was also rejected; S2-31dq below now handles that class
separately as masked invalid. JIT/native admission stays closed.

The independent 6,144-pair matrix compares pseudo-denormal and E=1 canonical
Normal encodings across fractions 0/1/interior/max, sign, all RCs, every TOP,
prior DE/PE/C1 and legal IE/SF sticky combinations. It asserts identical full
post-x87 state after accounting for current DE, including result bytes/tags,
PE, C1 and all canaries. Neighbor rejects include pseudo-infinity/NaN,
malformed encodings and Valid/Zero tags. At this stage the matrix also checked
unnormal rejection; the later S2-31dq matrix supersedes that one boundary. A
local two-step `FLD m80fp` then
`FRNDINT` test inspects the intermediate state: FLD preserves raw E=0/J=1 bytes
and Special tag with clean DE/PE; FRNDINT then returns Zero and adds DE+PE.
Another sequence covers repeat FRNDINT, FNSTSW, FNCLEX and zero re-entry.
The literal `s2_31dp_frndint_fld_pseudo_denormal_fnstsw.case.json` runs the
already schema-allowed `DB 28 D9 FC DF E0` composition from a clean seeded
project fixture and observes AX/FSW `0x3822`; this is not native evidence.

Focused verification passed: `cargo test -p od-isa --offline frndint` (27
matching tests) and `cargo test -p od-runner --offline frndint` (8 unit and 11
fixture integration tests). `cargo fmt --all -- --check`,
`cargo check --workspace --all-targets --offline`,
`python3 tools/check_layout.py`, and `git diff --check` passed. Graphify updated
the code graph to 4,759 nodes, 9,583 edges and 185 communities; the pre-existing
`include/open_desktop/host_abi.h` parser warning remains. Independent
`gpt-6.1-sol` medium review found no concrete defects and independently passed
the 27 matching `od-isa` tests. The reviewer's repeated runner command did not
finish within its 60-second limit; the author run above completed with 8 unit
and 11 fixture integration tests passing. Physical x86 and Mac mini acceptance
remain deferred to the combined final S2-31 substage under ISSUE-019.
Physical x86 and Mac mini acceptance remain deferred to the combined final
S2-31 substage under ISSUE-019. Normative basis: Intel SDM revision 093 Vol.1
§8.2.2/Table 8-3 and the Vol.2A FRNDINT entry (#D for denormal source, #P for
nonintegral result). In this revision, Vol.2A Table 3-47 is “Recreating FSAVE
Format,” so the pseudo-denormal encoding classification is cited to Vol.1
Table 8-3. See the
[CPU contract](../specs/CPU_CONTRACT.md#local-s2-31dp-special-tagged-binary80-pseudo-denormal-frndint-d9-fc),
[plan](../plans/STAGE_2.md#s2-31dp-special-tagged-binary80-pseudo-denormal-through-bounded-frndint),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-31dp-frndint-pseudo-denormal-fixture).

## S2-31dq — Special-tagged binary80 unnormal through bounded FRNDINT

The exact Special-tagged branch of bare Long64 `D9 FC` now accepts binary80
unnormals with biased exponent 1..`0x7ffe`, J=0, either sign and any fraction.
Under the unchanged four all-masked FCWs and status gate, it returns fixed
negative binary80 QNaN indefinite bytes `00 00 00 00 00 00 00 c0 ff ff`, keeps
the Special tag, adds sticky IE and clears C1. It does not add current DE, PE,
SF, ZE, OE or UE. Previously admitted sticky IE/DE/ZE/OE/UE/PE and SF paired
with IE remain preserved. TOP, other registers, FIP/RIP/retirement and
preflight remain unchanged. The result re-enters as the existing QNaN identity;
SNaN quieting remains a distinct path. FLD, classification and JIT/native
admission were not changed.

The independent 147,456-case matrix crosses exponent 1/interior/max, fraction
0/1/bit62/max, sign, all RC modes and TOPs, each legal historical IE/SF
pairing, all prior DE/ZE/OE/UE/PE combinations and C1. It checks full x87/GPR/
RFLAGS canaries and no memory access. Neighbor class/tag, invalid-control and
status rejects plus fetch/truncation/STOP/budget atomicity are covered. A
seeded literal `D9 FC DF E0 DB E2` fixture uses raw Special unnormal input in
the project x87 snapshot, observes AX/FSW `0x4501`, then FNCLEX leaves FSW
`0x4500` and retains the fixed result. This is project-fixture evidence, not
native execution.

Normative basis: Intel SDM revision 093 Vol.2A “FRNDINT—Round to Integer,”
p.3-372, lists unsupported format as #IA. Vol.1 Table 8-10 specifies the
masked QNaN floating-point indefinite for an arithmetic operation on an
unsupported format; Table 4-3 and §4.8.3.7 define its binary80 encoding. Table
8-10 gives SNaNs a separate masked response; that behavior is not reused here.
The separate FIST/FISTP denormal #D ambiguity under ISSUE-019 remains unchanged.

Validation passed: focused `cargo test -p od-isa --offline frndint` (28 tests),
the S2-31dq CLI fixture test `cargo test -p od-runner --offline
local_x87_frndint_seeded_unnormal_fnstsw_fnclex_fixture` (1 test),
`cargo fmt --all -- --check`, `cargo check --workspace --all-targets --offline`,
`python3 tools/check_layout.py`, and `git diff --check`. The independent
gpt-6.1-sol medium review found no code or behavior defects; it noted the
contract section's placement/reference, which was corrected by placing S2-31dq
immediately after S2-31dp. `graphify update .` completed at 4,767 nodes, 9,596
edges and 181 communities; the existing parser warning for
`include/open_desktop/host_abi.h` remains. No Mac mini, SSH or physical x86
execution was performed; physical acceptance remains deferred to the combined
final S2-31 substage under ISSUE-019. See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-31dq-special-unnormal-frndint-d9-fc), [plan](../plans/STAGE_2.md#s2-31dq-special-tagged-binary80-unnormal-through-bounded-frndint), and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-31dq-frndint-unnormal-fixture).

## S2-31dr — Special-tagged binary80 pseudo-infinity through bounded FRNDINT

Exact bare Long64 `D9 FC` now also accepts only a Special-tagged binary80
pseudo-infinity: E=`0x7fff`, J=0, zero fraction, either sign. This exact
encoding uses S2-31dq's existing masked-invalid response: fixed negative
binary80 QNaN indefinite `00 00 00 00 00 00 00 c0 ff ff`, Special tag, sticky
IE and cleared C1, with no new DE/PE/SF/ZE/OE/UE. Prior IE/DE/ZE/OE/UE/PE and
SF paired with IE remain preserved. Canonical J=1 infinity/QNaN/SNaN retain
their existing identity/quieting behavior. E=0 zero with Special tag remains a
tag mismatch; nonzero-fraction pseudo-NaNs and other neighboring classes remain
Unsupported. FLD, the classifier and JIT/native admission are unchanged.

The independent 12,288-case matrix crosses both signs, four RC controls, all
TOP values, three legal historical IE/SF pairings, all 32 prior DE/ZE/OE/UE/PE
combinations and both prior C1 values. It compares complete state, including
metadata and all non-TOP registers, and confirms no memory accesses. Repeat
FRNDINT preserves the indefinite and sticky IE. The literal seeded
`D9 FC DF E0 DB E2` fixture starts from a negative Special pseudo-infinity,
observes IE using FNSTSW, then clears exception flags with FNCLEX while keeping
the indefinite. It is `project_fixture` evidence and was not run on native x86.

Validation: focused `cargo test -p od-isa --offline frndint` and
`cargo test -p od-runner --offline local_x87_frndint_seeded_pseudo_infinity_fnstsw_fnclex_fixture`,
formatting, workspace check and layout checks passed. No Mac mini, SSH or
physical x86 execution was performed. ISSUE-019 remains open for combined host
acceptance and broader x87 semantics. See [contract](../specs/CPU_CONTRACT.md#local-s2-31dr-special-pseudo-infinity-frndint-d9-fc), [plan](../plans/STAGE_2.md#s2-31dr-special-tagged-binary80-pseudo-infinity-through-bounded-frndint), and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-31dr-frndint-pseudo-infinity-fixture).

## S2-31ds — Special-tagged binary80 pseudo-NaNs through bounded FRNDINT

Exact bare Long64 `D9 FC` accepts Special-tagged physical TOP with E=`0x7fff`,
J=0 and nonzero fraction for both signs and both Q-bit classes. It uses the
same fixed negative binary80 indefinite `00 00 00 00 00 00 00 c0 ff ff` as
the unsupported-format path, sets sticky IE, clears C1, and creates no current
DE/PE/SF/ZE/OE/UE. Existing admitted historical flags are retained. Pseudo-NaN
sign and payload are discarded; canonical J=1 infinity/QNaN/SNaN behavior,
FLD/classifier scope, gates and JIT/native denial remain unchanged.

The independent literal matrix covers 49,152 complete-state vectors:
four fractions spanning Q=0/Q=1, both signs, four RCs, eight TOPs, three legal
IE/SF pairings, all 32 combinations of historical DE/ZE/OE/UE/PE and prior
C1. Seeded Q=0 and Q=1 `D9 FC DF E0 DB E2` project fixtures observe IE with
FNSTSW and clear it with FNCLEX; neither is native evidence. Old pseudo-NaN
rejection expectations were updated while tag-mismatch rejects remain.

Normative basis: Intel SDM revision 093 Vol.2A FRNDINT p.3-372 and Vol.1
§8.2.2 Tables 8-3/8-10 and Table 4-3. The separate FIST/FISTP #D ambiguity
under ISSUE-019 is unchanged.

Validation passed: the 49,152-vector S2-31ds matrix, dq/dr regression matrices, FRNDINT canonical neighbor/environment controls, canonical J=1 infinity/QNaN/SNaN behavior, FLD invalid-class rejection, and both Q-class project fixtures. `cargo fmt --all -- --check`, `cargo check --workspace --all-targets --offline`, `python3 tools/check_layout.py`, and `git diff --check` passed. Independent gpt-6.1-sol medium review found no code or contract defects, including in the final reject-list and fixture-byte assertions. `graphify update .` completed at 4,782 nodes, 9,620 edges and 211 communities; the pre-existing parser warning for `include/open_desktop/host_abi.h` remains. No Mac mini, SSH or physical x86 execution was performed; ISSUE-019 remains open. See [CPU contract](../specs/CPU_CONTRACT.md#local-s2-31ds-special-pseudo-nan-frndint-d9-fc), [plan](../plans/STAGE_2.md#s2-31ds-special-tagged-binary80-pseudo-nan-through-bounded-frndint), and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-31ds-frndint-pseudo-nan-fixtures).

### Open x87 denormal question for integer stores

The current canonical-subnormal integer-store slices (`S2-31cq/cr/cs`) expect
PE without newly setting DE. Intel SDM revision 093 Vol.1 §§8.2.2, 8.4.1 and
8.5.2 say pseudo-denormals act as denormal operands, classify FIST/FISTP as
arithmetic, and require #D for an arithmetic denormal operand; the dedicated
Vol.2A FIST/FISTP entry on pp. 3-347–348 lists #IS/#IA/#P but omits #D. The
new-DE behavior is therefore unresolved, not a confirmed implementation bug.
Do not infer hardware behavior from the local PE-only fixtures. ISSUE-019
records a targeted final combined Mac mini check using canonical-subnormal and
pseudo-denormal sources with initial DE=0 through m16/m32 FIST/FISTP and m64
FISTP across RC; compare DE, PE, result, TOP and destination. Defer any FISTP
pseudo-denormal implementation until that question has evidence and a reviewed
contract.

## S2-31dt — historical sticky UE for bounded m32/m64 FLD

Only exact bare `D9 00` (`FLD m32fp [RAX]`) and `DD 00`
(`FLD m64fp [RAX]`) now admit pre-existing UE, by changing the local source
and v2 rejection-predicate masks from `0x80dc` to `0x80cc`. Historical UE is
preserved unchanged. Ordinary zero/Normal/infinity/QNaN loads add no IE/DE/PE;
current nonzero subnormal loads add DE, and SNaN loads add IE while retaining
the existing quieted payload mapping. ZE/OE/SF/ES/B remain rejected before
memory access. Four FCWs, Empty destination, one raw read, canonical48 range
preflight, C1, source mapping, push/TOP/tag, pointers, RIP, retirement and
atomicity remain in force. FILD, m80 loads, stores, FRNDINT, JIT and native
admission are unchanged.

The independent interpreter matrix has 16,384 cases: both widths, signed zero,
minimum/interior Normal, infinity, QNaN, SNaN and nonzero subnormal sources,
all four FCWs, all TOP values, all 16 IE/DE/UE/PE histories and both prior C1
values. It checks independently expected binary80 payloads and compares complete
`LongModeState`, one-read access history, canary bytes and memory contents.
The literal UE-only D900/DD00 project fixtures execute normal loads followed by
FNSTSW AX and observe UE without a current IE/DE/PE. Real FST m32/m64 underflow
producers feed both same-width and cross-width FLD consumers for all FCWs;
repeated loads retain UE/DE/PE, FNSTSW observes them, and FNCLEX clears the
exception flags. Invalid FCW, occupied destination, ZE/OE/SF/ES/B with UE,
canonical range, denied read/backend and instruction-boundary atomicity are
covered. The two prior S2-31ak Unsupported fixtures now seed ZE, since OE is
admitted, and retain their source-independent pre-read rejection. Separate m32/m64 schema
regressions mutate those exact bare, budget-one/no-stop, legal-FCW,
Normal-source Unsupported inputs to UE-only and require `Fixture::from_json`
to reject them.

Focused checks passed: the two new `od-isa` matrix/composition tests,
`fld_m32fp_rejects_invalid_values_gates_ranges_and_execution_boundaries_atomically`,
`fld_m64fp_rejects_invalid_gates_before_commit`, both seeded-UE and both
invalid-status runner fixture tests, and the m32/m64 `test_cli` literal fixture
schema tests. Formatting, workspace check, layout and diff checks passed;
`graphify update .` completed. The full `od-testkit` unit suite was interrupted
before completion; its focused runner schema/fixture paths passed. No Mac mini,
SSH or physical x86 execution was performed. Native runtime, physical
acceptance and broader x87 exception behavior remain open under ISSUE-019. See
the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-31dt-historical-sticky-ue-for-bounded-m32m64-fld),
[plan](../plans/STAGE_2.md#s2-31dt-admit-historical-sticky-ue-through-exact-binary32binary64-fld),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).

## S2-31du — historical sticky OE for bounded m32/m64 FLD

Only exact bare `D9 00`/`DD 00` changed their runtime and duplicated v2 schema
status masks from `0x80cc` to `0x80c4`. Historical OE now passes through and is
preserved alongside IE/DE/UE/PE. ZE/SF/ES/B remain pre-read rejects, and
unrelated x87, JIT and native gates are unchanged. The independent full-state
interpreter matrix covers 131,072 cases: both widths, eight signed-zero,
Normal-edge/interior, infinity, QNaN, SNaN and subnormal source vectors, all
four FCWs, all eight TOPs, all 32 IE/DE/OE/UE/PE histories and both prior-C1
values. Ordinary sources add no flags, subnormals add DE, SNaNs add IE and C1
clears. Complete `LongModeState`, one-read history, bytes and canaries are
checked.

Two seeded OE-only Normal→FNSTSW project fixtures were added. Four actual
same-width `FST overflow → FLD infinity → FNSTSW → FNCLEX` project fixtures
cover m32/m64 under nearest and positive-directed RC. The local composition
test checks state after each instruction and verifies OE+PE survive the
infinity load, are visible in AX, then clear through FNCLEX. It makes no OE
producer claim under toward-zero-directed modes. The S2-31ak pre-read reject
fixtures now use ZE. A schema regression verifies that a bare budget-one,
no-stop Normal Unsupported input is rejected with OE-only status. These are
local tests and `project_fixture` inputs; no Mac mini, SSH, physical x86 or
native-runtime checks were run. Broader x87 semantics remain open under
ISSUE-019. Focused checks passed: the interpreter matrix, same-width
composition, m32/m64 pre-read rejection/atomicity tests, the two-width OE-only
schema regression, the six OE project fixtures, and the updated S2-31ak fixture
checks. `cargo fmt --all -- --check`,
`cargo check --workspace --all-targets --offline`, and
`python3 tools/check_layout.py` passed. Independent gpt-6.1-sol medium review
found two coverage gaps in the first draft; both were fixed and final review
was clean. No full test corpus was run. See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-31du-historical-sticky-oe-for-bounded-m32m64-fld), [plan](../plans/STAGE_2.md#s2-31du-admit-historical-sticky-oe-through-exact-binary32binary64-fld), and [fixture notes](../../tests/fixtures/x86/stage2/README.md).

## S2-30ao — canonical signed zero through bounded FCHS

Bare `D9 E0` now admits Zero-tagged ST(0) only for the exact binary80 signed
zero encodings (zero significand and exponent word `0x0000` or `0x8000`). It
toggles only the raw sign bit, preserves the Zero tag, TOP and other state,
clears C1, and records FIP/RIP with one retirement. The existing Normal FCHS
path is unchanged; at that historical point FABS, FTST and FST/FSTP ST(i)
used the Normal-only gate. S2-30ap extends FABS, S2-30aq extends FTST, and
S2-30ar/as supersede the FST/FSTP limits. Native/JIT remain closed.

The local matrix covers both signs × all eight TOP positions × all 16 prior
condition-code values and compares full state after each step and after a
second FCHS. Rejection tests cover malformed Zero payloads, mismatched tag,
control/status failures, fetch/prefix/truncation/STOP/budget boundaries, and
confirm no data access. A four-instruction FLD m32/m64 → FCHS → FST → FNSTSW
composition is checked step by step for both widths. Handwritten project
fixtures exercise positive Zero admission and canonical-zero/wrong-tag atomic
Unsupported with no stop RIP and budget one. These are local interpreter and
fixture checks, not physical x86 or native evidence; broader x87 behavior and
physical acceptance remain open under ISSUE-019. Focused checks passed:
`od-isa` FCHS and composition tests, plus all FCHS runner fixture CLI tests.
`cargo fmt --all -- --check`, `cargo check --workspace --all-targets --offline`,
`python3 tools/check_layout.py`, and `graphify update .` passed. See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-30ao-signed-zero-fchs-admission-d9-e0), [plan](../plans/STAGE_2.md#s2-30ao-admit-canonical-signed-zero-to-bounded-fchs-d9-e0), and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30ao-signed-zero-fchs).

Independent gpt-6.1-sol high-effort re-review was clean after the first review's
coverage findings were fixed. No full workspace test suite or physical/native
execution was performed.
The required independent gpt-6.1-sol medium-effort review of commit `3ae0e0a`
was also clean, with no remaining concrete findings.

## S2-30ap — canonical signed zero through bounded FABS

Bare `D9 E1` now admits exact Zero-tagged signed zero in addition to the
existing canonical-Normal Valid source. FABS clears only the raw sign bit,
preserves Zero tag/TOP/payload and unrelated state, clears C1, retains C0/C2/C3
under the local snapshot policy, records FIP, advances RIP by two and retires
once. The narrow source gate is shared with FCHS. At that point FTST and
FST/FSTP ST(i) kept their Normal-only gate; S2-30aq and S2-30ar later extend
FTST and bare FST respectively. Native and JIT admission remain closed.

The literal interpreter matrix covers both zero signs × all eight TOP values ×
all 16 initial condition-code combinations, compares complete state after each
operation, and checks repeated-FABS idempotence. Rejection vectors cover
malformed Zero significands and exponent/special encodings, canonical zero
bytes under the wrong tag, bad FCW/FSW while the source remains canonical ±0,
missing state, and fetch/prefix/truncation/STOP/budget boundaries. The
unchanged-state guard then checked FTST and all eight FST plus all eight FSTP
ST(i) forms as Normal-only. S2-30aq/ar supersede that scope for FTST/FST; the
FSTP Normal-only result is historical and superseded by S2-30as. FLD m32/m64 → FABS →
FST → FNSTSW is checked locally. The new handwritten project fixture uses the
exact allowed stream `d900d9e1d910dfe0`, with four retirements; the schema admits
that exact composition only.

Focused checks passed: FABS Zero matrix, FABS bad-source/boundary atomicity,
FTST/FST/FSTP ST(i) Normal-only guard as of S2-30ap (FST/FSTP zero scope later superseded by S2-30ar/as), FCHS Zero matrix regression, both-width local
composition, the exact-composition schema positive/near-miss test, and the
four-retirement runner fixture CLI check. Final formatting, workspace check,
`cargo fmt --all -- --check`, `cargo check --workspace --all-targets --offline`,
`python3 tools/check_layout.py`, and `graphify update .` passed.
Evidence is local interpreter/project-fixture coverage, not physical or native
x87 evidence; ISSUE-019 remains open.

The first independent gpt-6.1-sol medium review found two coverage gaps; both
were fixed. The follow-up review was clean with no remaining concrete findings.

## S2-30ar — canonical signed zero through FST ST(i)

Bare `DD D0+i` now admits exact Zero-tagged signed zero under the existing
`0x037f` FCW and `FSW & 0x80ff == 0` gates. It copies the captured ten bytes and
Zero tag to physical `(old_TOP+i)&7` without popping or rotating. Existing
canonical-Normal Valid-source behavior remains, and bare FSTP ST(i) continues
to reject Zero. TOP, FSW and unrelated state are preserved except FIP; retained
condition bits, including C1, remain the documented deterministic local policy.

Tests exercise both signs × eight TOP positions × all eight destinations × all
16 condition-code states, previous destination tag/payload sentinels, alias
and repeated-operation idempotence, Normal regression vectors, malformed zero,
wrong tags, environment and execution-boundary atomic rejection. Local m32/m64
FLD→FST ST(1)→FNSTSW compositions run, and the schema/CLI fixture
`d900ddd1dfe0` checks the negative m32-zero sequence in three retirements.

Focused `od-isa`, `od-testkit`, and fixture CLI checks passed. Independent
gpt-6.1-sol medium review was clean after the rejection-boundary coverage was
extended. Formatting, workspace checks, layout validation and Graphify refresh
passed. Native/JIT and physical x86 behavior were not tested; ISSUE-019 remains open. S2-30ap's
and S2-30aq's earlier statements that FST ST(i) is Normal-only are historical;
S2-30ar supersedes them. S2-30ar's FSTP Normal-only scope is historical and
S2-30as supersedes it.

## S2-30aq — canonical signed zero through bounded FTST

Bare `D9 E4` additionally admits only exact Zero-tagged signed zero alongside
the previous canonical-Normal Valid source. Both zero signs compare equal and
set C3 alone with `(old_fsw & !0x4700) | 0x4000`; raw bytes/sign, Zero tag, TOP,
FCW/FDP/FOP, GPRs and RFLAGS are preserved, FIP records instruction RIP, and
RIP advances by two. At that historical point FST/FSTP ST(i) remained
Normal-only; S2-30ar/as later extend them. Native and JIT remain closed.

Local unit coverage spans both signed zeros × all TOP × all 16 old condition
fields with repeated execution and complete-state comparisons. Malformed zero
payloads, wrong tags, invalid environment, missing state and decode/fetch/STOP/
budget boundaries reject atomically. Existing Normal FTST cases remain; local
FLD m32/m64→FTST→FNSTSW vectors cover both zero signs. The handwritten fixture
admits only `d900d9e4dfe0` (FLD m32 → FTST → FNSTSW, three
retirements) and checks full state/memory through the interpreter CLI. This is
local evidence only; physical x87 behavior remains open under ISSUE-019.


## S2-30as — canonical signed zero through FSTP ST(i)

The shared bounded FST/FSTP register-store source gate now admits either a
Valid-tag canonical Normal or exact Zero-tagged signed zero under FCW `0x037f`
and `FSW & 0x80ff == 0`. For bare `DD D8+i`, the interpreter captures source
bytes/tag and old TOP/index, writes the physical destination, marks old TOP
Empty, then advances TOP modulo eight. It does not rotate the register array.
The `i=0` payload remains with final Empty tag; `i=1` becomes new ST(0).
All FSW bits except TOP (including C1), FCW/FDP/FOP, GPRs, RFLAGS and unrelated
payloads/tags are preserved. FIP/RIP and retirement update once. Other
Normal-only consumers remain unchanged; native/JIT admission stays closed.

Coverage checks both signs × all eight TOP positions × all indices × all 16
condition-code combinations with full-state comparisons and destination
payload/tag sentinels. It retains Normal cases and covers alias, `i=1`, TOP wrap,
malformed Zero, Zero-tagged Normal, wrong tags, special classes, bad controls,
missing state and atomic prefix/REX/WAIT/truncation/fetch/STOP/budget boundaries.
Both m32 and m64 FLD→FSTP ST(1)→FNSTSW compositions run for both signs. The exact
handwritten fixture `d900ddd9dfe0` has six bytes and three retirements in the
existing x87 fixture profile. Focused interpreter, schema, and fixture checks
passed, along with formatting, workspace check, layout validation and Graphify
refresh. No Mac mini, SSH, physical x86, native runtime or JIT execution was
run. ISSUE-019 remains open; local evidence does not establish physical x87
behavior.

## S2-30au — canonical signed infinity through FST/FSTP ST(i)

The shared bounded register-store source gate now also accepts only
Special-tagged canonical binary80 infinity (E=`0x7fff`, J=1, fraction=0) in
either sign. The exact `0x037f` FCW and `FSW & 0x80ff == 0` gates remain. FST
copies all ten bytes and Special tag to `(old_TOP+i)&7` while preserving TOP and
all status bits. FSTP copies before emptying old TOP and advancing it modulo
eight; `i=0` leaves the bytes with Empty tag, and `i=1` becomes new ST(0). The
existing C1 policy is unchanged. Pseudo-infinity, NaNs, subnormal, malformed
encodings and wrong tag/class pairs remain atomic rejects. JIT/native admission
is unchanged.

Interpreter coverage crosses both opcodes, signs, all TOP/index pairs and 16
condition-code combinations with full-state comparisons, destination canaries,
alias, `i=1`, and wrap. Valid infinities are also rejected atomically under bad
FCW/FSW; malformed neighboring classes, missing x87 state and execution
boundaries remain covered. m32/m64 FLD → FST/FSTP ST(1) → FNSTSW compositions
cover both signs. Four literal `project_fixture` cases reuse the existing
`d900ddd1dfe0` and `d900ddd9dfe0` streams; the schema did not expand. Focused
interpreter, fixture, formatting, workspace, layout and Graphify checks passed.
No Mac mini, SSH, physical x86 or native runtime check was performed. JIT
requests remain explicitly unsupported. ISSUE-019 stays open.

## S2-30at — canonical signed infinity through bounded FTST

Bare `D9 E4` additionally admits canonical Special-tagged binary80 infinity
(E=`0x7fff`, J=1, fraction=0) for either sign. Negative infinity compares Less
and sets C0; positive infinity compares Greater. The existing commit clears the
whole condition-code field before setting C0 when negative, so C1/C2/C3 clear;
all unrelated state, TOP, source tag and bytes remain unchanged. FCW `0x037f`
and `FSW & 0x80ff == 0` remain exact gates. FCHS/FABS source gates were not
changed; the historical FST/FSTP limit was superseded by S2-30au.

The interpreter matrix covers both signs × all eight TOP values × all 16 prior
condition-code values with full-state comparison and repeated execution. The
reject matrix covers absent x87 state, wrong tags, malformed Special values,
pseudo-infinity, subnormal, pseudo-denormal, unnormal, NaNs, bad FCW/FSW, and
prefix/REX/WAIT/truncation/fetch/STOP/budget atomicity using the newly admitted
infinity source for the environment and execution-boundary cases. Existing
Normal and signed-zero FTST regressions remain covered.

Two handwritten `project_fixture` cases reuse only the existing exact
`d900d9e4dfe0` sequence and load binary32 positive/negative infinity. Both
interpreter fixture runs retired three instructions, preserved all memory, and
matched complete literal CPU/x87 snapshots; FNSTSW observed `0x3800` and
`0x3900` respectively. The schema did not widen. A JIT request returned
`Unsupported` because schema-v2 execution is interpreter-only; no JIT ran.
Native x87 admission remains closed; its Linux-only admission test was not
executed on this macOS host. No Mac mini, SSH or physical x86 run was performed.

Focused `od-isa ftst_`, project-fixture execution, and schema fixture tests
passed, along with `cargo fmt --all -- --check`,
`cargo check --workspace --all-targets --offline`, and
`python3 tools/check_layout.py`. Independent gpt-6.1-sol medium review was
clean after adding the malformed-class/environment and literal-fixture checks.
Local interpreter/project-fixture evidence does not establish physical x87
behavior; ISSUE-019 remains open.

## S2-30av — canonical signed infinity through bounded FCHS/FABS

The shared bounded FCHS/FABS source gate now additionally accepts only the
`(Special, Infinity)` pair for canonical signed binary80 infinity. FCW remains
`0x037f`, the existing `FSW & 0x80ff == 0` mask is unchanged, and Normal and
Zero-tagged exact signed-zero behavior is unchanged. FCHS flips the sign bit;
FABS clears it. Both retain the Special tag and TOP, clear C1, and preserve the
rest of the local snapshot state. Pseudo-infinity, NaNs and pseudo-NaNs,
subnormal, pseudo-denormal, unnormal, wrong tags, invalid controls/status and
execution-boundary cases reject atomically.

The full-state interpreter matrix covers both opcodes × both signs × all eight
TOP values × all 16 prior condition-code states, with repeated operations and
malformed-source/environment/boundary rejection. m32/m64 FLD→FCHS/FABS→FST→
FNSTSW compositions cover both signs. The two literal project fixtures use
exactly `d900d9e0d910dfe0` and `d900d9e1d910dfe0`; the no-stop, budget-one
wrong-tag fixture confirms CLI rejection. JIT requests remain unsupported and
the schema was not widened. These checks passed:

- `cargo test -p od-isa --offline fchs_and_fabs` (acceptance, malformed-source
  and execution-boundary tests).
- `cargo test -p od-isa --offline fld_signed_infinity_sign_operations_fst_fnstsw_compose_for_both_source_widths`.
- `cargo test -p od-isa --offline fchs_flips_only_the_raw_sign_byte_for_normal_st0_and_clears_c1`,
  `... fchs_toggles_canonical_zero_for_every_top_and_condition_code`,
  `... fabs_clears_only_the_raw_sign_byte_for_normal_st0_and_clears_c1`, and
  `... fabs_clears_canonical_zero_sign_for_every_top_and_condition_code`.
- `cargo test -p od-runner --offline --test fixture_v2 local_x87_fld_sign_operation_fst_fnstsw_infinity_fixtures_check_state_memory_and_jit_denial`
  and `... local_x87_fchs_infinity_wrong_tag_fixture_is_atomic_in_the_cli_runner`.
- `cargo fmt --all -- --check`, `cargo check --workspace --all-targets --offline`,
  `python3 tools/check_layout.py`, and `git diff --check`.
- `graphify update .` completed; it refreshed the graph and reported its
  existing extractor warning for `include/open_desktop/host_abi.h`.

No Mac mini, SSH, native runtime or physical x86 checks were performed; ISSUE-019
remains open.

## S2-30aw — canonical binary80 QuietNaN through bounded FCHS/FABS

The shared FCHS/FABS source gate now admits `(Special, QuietNaN)` in addition
to the established canonical-Normal, exact Zero-tagged signed-zero and
Special-tagged canonical-infinity pairs. The canonical binary80 classifier
defines QuietNaN as E=`0x7fff`, J=1, Q=1; both signs and all low 62 payload bits
are retained. FCHS toggles only the raw sign bit and FABS clears only it. Both
preserve Special tag, TOP and existing C0/C2/C3 policy, clear C1, retain all
exception flags, and keep the existing FIP/RIP/retirement behavior. SNaN,
pseudo-NaNs, pseudo-infinity, subnormal, pseudo-denormal, unnormal and wrong
tags remain rejected before commit. No classifier, schema or JIT/native gate
changed.

The od-isa full-state matrix contains 2,048 distinct combinations:
2 opcodes × 2 signs × 8 TOP values × 16 condition-code patterns × 4 payloads
(`0`, `1`, `1<<61`, `(1<<62)-1`). Expectations mutate a cloned raw-byte snapshot
directly, check all state/canaries, then repeat FABS or execute FCHS twice.
Negative tests cover malformed neighbors, wrong tags, invalid FCW/FSW, absent
x87 state and truncation, STOP, zero budget and fetch faults. Four exact
schema-v2 `project_fixture` programs use the existing m32 FLD payload QNaN →
FCHS or FABS → FST → FNSTSW compositions across both source signs. Direct
interpreter compositions additionally cover m64 FLD/FST for both operations
and signs. The CLI verifies full state and memory bytes and confirms JIT
denial; a no-stop wrong-tag fixture confirms zero retirement and unchanged
state. The schema is unchanged.

Validation passed: `cargo test -p od-isa --offline
fchs_and_fabs_admit_canonical_quiet_nan_and_preserve_every_payload_bit`,
the focused FCHS/FABS reject/boundary/legacy matrices, the runner QNaN fixture
and wrong-tag tests, `cargo fmt --all -- --check`,
`cargo check --workspace --all-targets --offline`,
`python3 tools/check_layout.py`, and `git diff --check`. `graphify update .`
refreshed the code graph. No Mac mini, SSH, native runtime or physical x86
checks were performed; ISSUE-019 remains open pending combined host/physical
acceptance.

## S2-30ax — canonical binary80 SignalingNaN through bounded FCHS/FABS

The shared `bounded_x87_sign_st0_source` gate additionally admits only
`(Special, SignalingNaN)` with canonical binary80 exponent `0x7fff`, J=1, Q=0,
and nonzero low-62 payload, for either sign. Existing canonical infinity
remains admitted through its separate class path; in particular, the J=1,
Q=0 zero-payload encoding retains the classifier's established Infinity
classification. FCHS toggles only the sign bit, FABS clears only the sign bit,
and both preserve Q=0, payload, Special tag, TOP, C0/C2/C3 and all other state
while clearing C1 and adding no IE/PE. No decoder, classifier, schema, JIT or
native gate changed.

The 2,048-case full-state unit matrix crosses both operations, both signs, all
eight TOP values, 16 prior condition-code combinations and payloads `{1,
1<<61, 0x0123456789abcdef, (1<<62)-1}`. It checks full state and canaries,
double-FCHS restoration, repeated-FABS idempotence and continued SNaN
classification with IE/PE clear. Atomic rejection coverage includes
pseudo-NaNs/infinities, subnormal, pseudo-denormal, unnormal, wrong tags,
invalid control/status, absent x87 state, truncation, STOP, exhausted budget
and fetch fault. A raw `FLD m80fp` → FCHS/FABS → `FSTP m80fp` → `FNSTSW AX`
unit composition checks bytes after load and after the operation as well as
the stored ten bytes, for both signs/operations. Four bare schema-v2 fixtures
seed the SNaN directly in ST(0), each execute one operation with budget one,
check full state and are denied by JIT; the separate wrong-tag CLI fixture
rejects atomically. The fixture sequence was restricted to bare operations
because the schema-v2 CLI contract for this substage requires one instruction.

Validation passed: `cargo test -p od-isa --offline fchs_and_fabs` (five
admission, malformed-source, boundary and prior infinity/QuietNaN tests),
`cargo test -p od-isa --offline
fld_m80_fchs_fabs_fstp_m80_preserves_raw_signaling_nan`, both targeted
`od-runner --test fixture_v2` SNaN success/wrong-tag cases,
`cargo fmt --all -- --check`, `cargo check --workspace --all-targets --offline`,
`python3 tools/check_layout.py`, and `git diff --check`. `graphify update .`
completed at 4,904 nodes, 9,827 edges and 195 communities; its existing
`include/open_desktop/host_abi.h` extraction warning remains. No Mac mini, SSH,
native runtime or physical x86 checks were performed; ISSUE-019 remains open.
Independent gpt-6.1-sol medium follow-up review found no remaining defects in
the SNaN gate, fixtures or contract documentation.

## S2-30ay — canonical binary80 subnormal through bounded FCHS/FABS

Only `bounded_x87_sign_st0_source` was extended: `(Special, Subnormal)` is
accepted when the existing binary80 classifier reports canonical subnormal
(E=0, J=0, nonzero fraction). Existing FCW `0x037f`, `FSW & 0x80ff == 0`,
Normal/zero/infinity/QNaN/SNaN handling and all other gates remain unchanged.
FCHS flips only the sign bit; FABS clears only it. The full fraction, Special
tag, TOP, C0/C2/C3 and other state remain unchanged, C1 clears, and no DE/UE/
PE/IE is added. Pseudo-denormal, unnormal, pseudo-special and wrong tag/class
inputs continue to reject before commit. The pre-existing vector with bytes
`[0,0,0,0,0,0,0,0,1,0]` is correctly labeled as unnormal (E=1,J=0).

The full-state 2,048-case matrix crosses both operations, both signs, all eight
TOP values, all 16 prior condition-code combinations and fractions `{1,
1<<62, 0x0123456789abcdef, (1<<63)-1}`. It checks complete state, exact raw
payload, no added exception flags, repeated FABS, double-FCHS, neighboring
malformed classes, wrong tags, invalid/missing state and execution-boundary
atomicity. The raw m80 composition executes FLD m80fp → FCHS/FABS → FSTP m80fp
→ FNSTSW AX, checking bytes after load, sign operation and store for both
source signs/opcodes, no DE and memory canaries. Four hand-authored bare
schema-v2 fixtures seed the raw subnormal in ST(0), run with budget one and
check exact state plus JIT denial; the additional wrong-tag fixture rejects
atomically. No schema, decoder, classifier or JIT/native admission was changed.

Validation passed: `cargo fmt --all -- --check`; all six
`cargo test -p od-isa --offline fchs_and_fabs` cases; the focused raw m80
composition; both focused `od-runner --test fixture_v2` success/JIT-denial and
wrong-tag tests; `cargo check --workspace --all-targets --offline`;
`python3 tools/check_layout.py`; `git diff --check`; and `graphify update .`
(4,913 nodes, 9,842 edges, 182 communities). Graphify reported its existing
`include/open_desktop/host_abi.h` extraction warning and skipped unsupported
file types. No Mac mini, SSH, native runtime or physical x86 checks were
performed. ISSUE-019 remains open for combined host/physical acceptance.

## S2-30az — canonical binary80 QuietNaN through bounded FST/FSTP ST(i)

Only `bounded_x87_fst_sti_source` changed. The source gate now also admits
`(Special, QuietNaN)` canonical binary80 values (E=`0x7fff`, J=1, Q=1) for
either sign and all low-62 payloads, including zero. FCW `0x037f`,
`FSW & 0x80ff == 0`, existing Normal/zero/infinity paths and the current C1
policy remain. FST copies the raw bytes and tag without TOP change; FSTP copies
first, empties old TOP and advances TOP modulo eight. No exception flags,
decoder, classifier, schema, other source gate, JIT admission or native
admission changed.

The 16,384-case interpreter matrix crosses both opcodes, both signs, every TOP
and index, all 16 condition-code patterns and payloads `{0, 1, 1<<61,
(1<<62)-1}`. It compares complete state and raw bytes, overwrites prior
destination tag/payload sentinels, and exercises alias `i=0`, `i=1`, TOP wrap
and copy-before-pop. Reject cases cover SNaN and malformed neighboring
classes, wrong tags, FCW/FSW gates, absent x87 state and fetch/truncation/
prefix/REX/WAIT/STOP/budget boundaries. Direct FLD→FST/FSTP ST(1)→FNSTSW
compositions cover m32 and m64 QNaNs of both signs. Four existing-schema
project fixtures run the exact `d900ddd1dfe0` and `d900ddd9dfe0` streams with
both binary32 signs; they check full state, payload, memory, canary and JIT
denial.

Validation passed: `cargo test -p od-isa --offline qnan` (8 tests),
`cargo test -p od-isa --offline fst_sti` (6 tests),
`cargo test -p od-isa --offline fstp_sti` (7 tests), and the focused
`cargo test -p od-runner --offline --test fixture_v2
local_x87_fld_fst_and_fstp_sti_qnan_fixtures_check_state_memory_and_jit_denial`
(1 test). `cargo fmt --all -- --check`,
`cargo check --workspace --all-targets --offline`,
`python3 tools/check_layout.py`, and `git diff --check` passed. `graphify update
.` rebuilt the graph at 4,926 nodes, 9,864 edges and 182 communities. Its
existing `include/open_desktop/host_abi.h` syntax warning and skipped
unsupported file types remain. Independent gpt-6.1-sol medium follow-up review
found no remaining defects. No Mac mini, SSH, native runtime or physical x86
checks were performed; ISSUE-019 remains open for combined host/physical
acceptance.

## S2-30ba — canonical binary80 SignalingNaN through bounded FST/FSTP ST(i)

Only `bounded_x87_fst_sti_source` was extended. It now admits canonical
`(Special, SignalingNaN)` binary80 encodings with E=`0x7fff`, J=1, Q=0 and a
nonzero lower-62-bit payload, for either sign. The zero-payload J=1/Q=0 value
continues through the existing canonical Infinity path. FCW `0x037f`,
`FSW & 0x80ff == 0`, prior Normal/zero/infinity/QNaN cases, and existing C1
handling are unchanged. FST copies raw bytes/tag without TOP movement; FSTP
copies before emptying old physical TOP and incrementing TOP modulo eight. No
Q bit, IE or PE is introduced.

The 16,384-case state matrix crosses both forms/signs, all TOP/index pairs,
16 condition-code states and payloads `{1, 1<<61, 0x0123456789abcdef,
(1<<62)-1}`. It compares full state, payload bytes, tag canaries and physical
register sentinels, including alias, ST(1), TOP wrap and copy-before-pop. The
old canonical SNaN positive reject vectors were removed; pseudo-SNaN and other
invalid neighbor/tag/environment/boundary rejects remain covered. A raw
FLD m80fp→FST/FSTP ST(1)→FNSTSW unit composition covers both signs and forms
and verifies Q=0/no IE. Four seeded bare schema-v2 cases run `DD D1`/`DD D9`
with budget one and stop RIP +2; wrong-tag input returns Unsupported without
retirement/RIP movement. Successful runner fixtures are denied by JIT.

The v2 wire schema is unchanged. To support the requested bare CLI cases, the
`od-testkit` profile now recognizes only exact single-op `DD D0+i` and
`DD D8+i` register-store streams. Two stale invalid-environment test witnesses
were corrected from admitted UE to rejected ZE; forbidden-status lists also
exclude admitted OE and UE under the shared `0x80c4` gate. This did not change
production execution gates.

Focused checks passed: the full `od-isa` unit suite (652 tests), including the
16,384-case SNaN matrix, raw m80 composition, canonical neighbor/tag rejects
and special-class/environment rejects; the seeded runner fixture test including
JIT denial; and `cargo test -p od-testkit --offline` (173 tests).
`cargo fmt --all -- --check`, `cargo check --workspace --all-targets --offline`,
`python3 tools/check_layout.py` and `git diff --check` passed. Graphify rebuilt
with 4,938 nodes, 9,883 edges and 186 communities; the existing header syntax
warning and unsupported-file skips remain. No Mac mini, SSH, native runtime or
physical x86 checks were performed. ISSUE-019 remains open for combined
host/physical acceptance.

<a id="s2-30bb-canonical-binary80-subnormal-through-bounded-fstfstp-sti"></a>

## S2-30bb — canonical binary80 subnormal through bounded FST/FSTP ST(i)

Only `bounded_x87_fst_sti_source` changed: it now additionally admits the
`(Special, Subnormal)` pair for canonical binary80 E=0, J=0 and nonzero 63-bit
fraction. The raw ten bytes and Special tag are copied unchanged for either
sign. Existing FCW `0x037f`, `FSW & 0x80ff == 0`, C1, FIP/RIP and retirement
policies remain. FST preserves TOP; FSTP copies before emptying old TOP and
advancing TOP. No DE/UE/PE/IE is added. Classifier, decode, schema, other source
gates and JIT/native admission are unchanged.

The 16,384-case interpreter matrix crosses both opcodes/signs, every TOP and
index, all 16 condition-code combinations, and fractions `{1, 1<<62,
0x0123456789abcdef, (1<<63)-1}`. It compares full state and checks raw payloads,
destination tag canaries, alias behavior and TOP wrap/copy-before-pop. Atomic
rejection checks cover zero under Special, wrong-tag subnormals, malformed
neighbor classes, bad FCW/FSW, missing x87 state, fetch/truncation/prefix/WAIT,
STOP, budget and fetch fault. Raw FLD m80fp→FST/FSTP ST(1)→FNSTSW unit
compositions verify both signs/forms preserve exact bytes and do not set DE.
Four seeded bare schema-v2 fixtures exercise FST/FSTP for both signs with
budget one and stop RIP +2; a wrong-tag fixture rejects without retirement and
successful fixtures are denied by JIT. The profile schema and allowlist were
not changed; the existing bare DD D0+i/DD D8+i fixtures already establish that
stream profile.

Focused checks passed: the 16,384-case matrix, raw m80 composition, neighbor/
wrong-tag rejects, subnormal environment/execution-boundary atomicity, the
runner fixture test (including JIT denial) and `cargo test -p od-testkit
--offline` (173 tests). `cargo fmt --all -- --check`,
`cargo check --workspace --all-targets --offline`,
`python3 tools/check_layout.py` and `git diff --check` passed. Graphify updated
to 4,951 nodes, 9,906 edges and 183 communities; it reported the existing
`include/open_desktop/host_abi.h` syntax warning and skipped unsupported file
types. No Mac mini, SSH, native runtime or physical x86 checks were performed.
ISSUE-019 remains open for combined host/physical acceptance.

## S2-30bc — historical sticky IE through bounded FCHS/FABS

Only `bounded_x87_sign_st0_source` changed its FSW acceptance mask from
`0x80ff` to `0x80fe`; FCW `0x037f`, the six accepted tag/class pairs, the
remaining exception/summary rejects and every other operation/JIT/native gate
remain unchanged. IE is preserved independently of source class. FCHS/FABS
retain their sign-only raw update, clear C1 and add no exception flags; current
SNaN bytes remain signaling and unchanged apart from the sign transform.

The interpreter matrix contains 6,144 cases (both opcodes × six admitted
classes × signs × TOP × 16 condition-code states × IE clear/set), compares full
state and canaries, and repeats each operation to verify IE persistence. Atomic
rejects cover IE plus each forbidden FSW bit, invalid FCW with IE, malformed
tags with IE, missing x87 state and fetch/truncation/STOP/budget/fault
boundaries. FNSTSW observes IE and FNCLEX clears it. Direct m32/m64 SNaN
compositions assert that FLD quiets the SNaN and sets IE before FCHS/FABS, then
check the same-width stored QNaN sign/payload and FNSTSW-observed IE. Four
schema-v2 m32 producer fixtures cover both signs and operations; separate
fixtures check IE-only Normal success and IE+SF atomic rejection. All fixture
success cases are denied by JIT.

Validation passed: the eight focused `od-isa` FCHS/FABS tests (including the
6,144-case matrix and forbidden-bit atomic rejects), direct m32/m64 SNaN
composition, and FNSTSW/FNCLEX check; the four m32 producer CLI fixtures and
IE-only/IE+SF CLI fixtures; `cargo fmt --all -- --check`,
`cargo check --workspace --all-targets --offline`, `python3 tools/check_layout.py`,
and `git diff --check`. `graphify update .` rebuilt 4,964 nodes, 9,929 edges
and 177 communities. It reported the existing `host_abi.h` syntax warning,
unsupported-file skips, and that community labels need refresh. No Mac mini,
SSH, native runtime or physical x86 checks were performed. ISSUE-019 remains
open for combined host/physical acceptance.


## S2-30bd — historical sticky DE through bounded FCHS/FABS

Only `bounded_x87_sign_st0_source` changed its FSW mask from `0x80fe` to
`0x80fc`; FCW `0x037f`, the six exact source class/tag pairs and all other gates
remain unchanged. Historical IE and DE are accepted independently or together.
All ZE/OE/UE/PE/SF/ES/B combinations remain unsupported. The sign-only raw
operation, C1 clearing and existing CC/TOP/tag behavior remain intact. Raw
binary80 subnormals still add no DE; current SNaNs still add no IE.

The 12,288-case full-state matrix spans both instructions, six classes, signs,
TOP, all 16 CC states and four IE/DE combinations. Atomic rejects combine each
forbidden status bit with every IE/DE combination, and separately cover invalid
FCW/tag, missing state and execution boundaries with DE set. Unit compositions
run m32/m64 minimum, interior and largest subnormals for both signs/operations
with initial IE clear/set. They check Normal+Valid+DE immediately after FLD,
exact binary80 sign-only changes, same-width subnormal FST output and no new
UE/PE. Four m32 project fixtures cover opcode/sign CLI compositions; seeded
DE-only success and DE+ZE/IE+DE+SF rejection fixtures check atomicity and JIT
deny-by-default behavior.

Validation passed: focused `od-isa` FCHS/FABS matrix, forbidden-status and
binary-subnormal composition tests; runner CLI tests for the four producer and
three seeded-status fixtures; `cargo fmt --all -- --check`,
`cargo check --workspace --all-targets --offline`, `python3 tools/check_layout.py`,
`git diff --check`, and `graphify update .`. No Mac mini, SSH, native runtime
or physical x86 checks were performed. ISSUE-019 remains open for combined
acceptance.

## S2-30be — historical sticky PE through bounded FCHS/FABS

Only `bounded_x87_sign_st0_source` changed its FSW mask from `0x80fc` to
`0x80dc`. FCW `0x037f`, the six accepted tag/class pairs, sign/payload
semantics, C1 clearing, CC/TOP/tags, metadata/RIP/retirement behavior and
unrelated gates remain unchanged. Exactly all eight IE/DE/PE combinations are
admitted; ZE/OE/UE/SF/ES/B remain forbidden with every such combination.
Decoder/schema/native gates were not widened and the JIT still denies these
forms.

The full-state interpreter matrix covers 24,576 states: two operations × six
classes × two signs × eight TOPs × 16 CC states × eight IE/DE/PE combinations.
Full-state assertions include every forbidden status bit crossed with all eight
historical states, malformed class/tag/raw and FCW, missing state, and
fetch/truncation/STOP/budget/fault atomic rejection with PE. FRNDINT at FCW
`0x037f` produces PE for ±1.25→±1, ±1.5→±2, and ±0.5→±0. Tests observe C1
clear/set after FRNDINT and verify both FCHS/FABS clear C1 while preserving PE.
Canonical subnormal→FRNDINT produces Zero-tagged signed-zero raw bytes and full
FSW `0x0022`; both FCHS and FABS then preserve the Zero tag, exact raw bytes and
full FSW `0x0022` after their respective sign transforms before FNSTSW/FNCLEX
observes and clears status.
Six exact schema-v2 CLI fixtures cover FRNDINT→FCHS/FABS
from seeded ±1.5, PE-only and IE|DE|PE success, and PE|UE and IE|DE|PE|SF
atomic rejects. Runner checks retain JIT denial.

Focused checks passed: 18 filtered `od-isa` FCHS/FABS tests, the canonical
subnormal FRNDINT status test, the historical PE fixture CLI test, and
`cargo fmt --all -- --check`. `cargo check --workspace --all-targets --offline`,
`python3 tools/check_layout.py` (48 required directories, 6 crate scaffolds,
14 instruction scopes), `git diff --check`, and `graphify update .` also passed.
Graphify rebuilt 4,989 nodes, 9,970 edges and 183 communities; it reported the
existing `host_abi.h` syntax warning, unsupported-file skips, and stale saved
community labels. No Mac mini, SSH, native runtime or physical x86 checks were
performed. ISSUE-019 remains open.

## S2-30bf — historical sticky UE through bounded FCHS/FABS

At S2-30bf, the earlier S2-30be PE|UE rejection fixture was renamed to
`s2_30be_fchs_normal_pe_oe` and seeded PE|OE (`0x0028`), since UE was admitted
while OE remained forbidden. S2-30bg later admits OE and used a ZE|PE
(`0x0024`) reject fixture. S2-30bh admits ZE, so the current repository
fixtures are rebased to `s2_30be_fchs_normal_pe_sf` and
`s2_30bf_fchs_normal_ue_es` to retain forbidden-status coverage.


Only `bounded_x87_sign_st0_source` changed its FSW mask from `0x80dc` to
`0x80cc`. FCW `0x037f`, the six source tag/class pairs, sign/payload behavior,
C1 clearing, CC/TOP/tags, metadata and retirement behavior remain unchanged.
Exactly the 16 IE/DE/UE/PE combinations are admitted; ZE/OE/SF/ES/B are
rejected with each history combination. Decode/classifier/arithmetic/JIT/native
gates are unchanged.

The interpreter matrix covers 49,152 states: two operations × six classes ×
two signs × eight TOP × 16 CC patterns × 16 IE/DE/UE/PE combinations. The
forbidden-status matrix covers 1,920 atomic rejects: two operations × six
source classes × two signs × 16 histories × five forbidden bits. Under FCW
`0x037f`, an independent FST producer test covers m32 and m64 signed zero and
minimum-subnormal outcomes for both signs and both sign operations. It checks
written bytes, the unchanged Normal+Valid binary80 source and tag, then checks
that the sign operation changes only sign and preserves the UE|PE already
observed after FST (with C1 clear/set at producer time). CLI fixtures use the
exact streams `d910d9e0dfe0` and `dd10d9e1dfe0`, plus forbidden UE|OE at that
stage (renamed to ZE|UE in S2-30bg) and IE|DE|UE|PE|SF cases; JIT remains
denied.

No Mac mini, SSH, native runtime or physical x86 checks were performed.
ISSUE-019 remains open for the combined final substage.


Focused checks passed: eight filtered FCHS/FABS ISA tests (including the
49,152-case matrix and 1,920 forbidden-status rejects), the m32/m64 FST UE|PE
producer test with FNSTSW/FNCLEX, and the schema-v2 producer CLI test.
`cargo fmt --all -- --check`, `cargo check --workspace --all-targets --offline`,
`python3 tools/check_layout.py` (48 required directories, six crate scaffolds,
14 instruction scopes), `git diff --check`, and `graphify update .` passed.
Graphify rebuilt 4,999 nodes, 9,985 edges and 180 communities; it reported the
existing `host_abi.h` syntax warning, unsupported-file skips and stale saved
community labels. No Mac mini, SSH, native runtime or physical x86 checks were
performed. ISSUE-019 remains open.

## S2-30bg — historical sticky OE through bounded FCHS/FABS

Only `bounded_x87_sign_st0_source` changed its FSW mask from `0x80cc` to
`0x80c4`. FCW `0x037f`, the six accepted tag/class pairs and all existing
sign/C1/TOP/tag/raw/payload/FIP/RIP/retirement invariants remain unchanged.
Exactly all 32 IE/DE/OE/UE/PE combinations are admitted. ZE/SF/ES/B reject
atomically in every history combination. Sign operations generate no flags;
decoder, schema, classifier, arithmetic and JIT/native gates remain unchanged.

The full-state interpreter matrix covers 98,304 states (two operations × six
class/tag pairs × two signs × eight TOP × 16 CC patterns × 32 historical
combinations). The four forbidden-status products cover 3,072 atomic rejects.
Bad FCW/tag/raw, missing x87 state and fetch/truncation/STOP/budget/fault
boundaries are checked with OE history. Under FCW `0x037f`, non-popping FST
m32/m64 producer cases test round-to-nearest overflow midpoint−1, midpoint,
midpoint+1 and maximum significand for both signs. Midpoint values are
`0xffffff8000000000` at exponent 127 and `0xfffffffffffffc00` at exponent
1023. Midpoint−1 produces no OE; midpoint and higher produce signed infinity,
OE|PE and C1=1. Seeded IE/DE/UE/PE remain set. Full state and canary snapshots
check store bytes, unchanged Normal+Valid source, sign-only source
transformation, C1 clear, retained OE|PE, unchanged memory, and FNSTSW/FNCLEX
visibility. Failed and out-of-range writes preserve full state and memory.
CLI fixtures execute `d910d9e0dfe0` and `dd10d9e1dfe0`, include seeded
OE-only success and the then-forbidden ZE|PE/ZE|UE rejects, later replaced
with PE|SF and UE|ES when S2-30bh admitted historical ZE, and retain JIT denial.

Focused checks passed: the 98,304-case matrix, 3,072 forbidden-status cases,
OE boundary rejection, the m32/m64 producer and atomic-write unit test, and
schema-v2 CLI tests for OE-only success, both producers, renamed rejects and
JIT denial. `cargo fmt --all -- --check`,
`cargo check --workspace --all-targets --offline`,
`python3 tools/check_layout.py` (48 required directories, six crate scaffolds,
14 instruction scopes), and `git diff --check` passed. `graphify update .` was
run after these changes. No Mac mini, SSH, native runtime or physical x86
checks were performed; ISSUE-019 remains open.


## S2-30bh — historical sticky ZE through bounded FCHS/FABS

Only `bounded_x87_sign_st0_source` changed its local FSW mask from `0x80c4` to
`0x80c0`. The exact FCW and six tag/class pairs remain unchanged. The 196,608
case full-state matrix spans both operations, all class/tag pairs, signs, TOP,
CC and 64 IE/DE/ZE/OE/UE/PE histories. The 4,608 forbidden product checks SF,
ES and B against each history. The operation continues to clear C1 and preserves
all other status, raw payload, TOP/tag, metadata and retirement; it adds no
exception flags.

ZE was seeded in the initial x87 state. No guest divide-by-zero producer is
claimed. Seeded CLI fixtures cover ZE-only and all-six-flags success and ZE|SF
and all-six-flags|ES rejection, with JIT denial. Former DE|ZE, PE|ZE and UE|ZE
rejection fixtures were renamed/rebased to DE|SF, PE|SF and UE|ES.

The focused FRNDINT compositions seed ZE and check ±1.5→±2 and ±1.25→±1
with newly generated PE, exact ±1/±0 without new PE, and SNaN→QNaN where
FRNDINT performs quieting and adds IE; FCHS/FABS preserves the resulting
history. FNSTSW/FNCLEX observe and clear state. Documentation and fixture
provenance explicitly distinguish seeded ZE from a producer.

Local validation is limited to focused ISA/CLI coverage, formatting, offline
workspace check, layout validation, diff checks and Graphify update. Mac mini,
SSH, native runtime and physical x86 checks were not run; ISSUE-019 remains open
for the combined final acceptance substage.

## S2-30bi — historical SF only with IE through bounded FCHS/FABS

The shared FCHS/FABS source gate now admits all 64 IE/DE/ZE/OE/UE/PE histories
with SF clear and the corresponding 32 histories with SF|IE. FCW `0x037f`, the
six accepted tag/class pairs, sign/payload transform, C1 clear, TOP/tag,
metadata, FIP/RIP and retirement behavior remain unchanged. The operations do
not generate flags or quiet SNaN. SF without IE, ES and B reject atomically;
decoder/schema/JIT/native admission is unchanged, and JIT remains denied.

The full-state matrix passes 294,912 cases (196,608 SF-clear plus 98,304 SF|IE).
Atomic rejection passes for 768 SF-without-IE states and 6,144 ES/B states over
all 128 low-seven-bit histories. The boundary checks seed IE|SF for fetch,
truncation, STOP, budget and fetch-fault paths; invalid FCW, malformed/Empty
source, missing state, repeated operations and FNSTSW/FNCLEX are also covered.
Basic schema-v2 CLI fixtures check bare FCHS +1 and FABS -1 with initial FSW
`0x0241`; the operation preserves IE|SF and clears C1 to `0x0041`. Two exact
FRNDINT compositions use `d9fcd9e0dfe0` and `d9fcd9e1dfe0`, seed ±1.5 with
IE|SF, then check FRNDINT's PE/C1 and FCHS/FABS C1 clearing before FNSTSW reads
`0x0061` at TOP=0. IE|SF is seeded in the initial project snapshot; no guest
stack-fault producer exists in scope, and FRNDINT is composition, not an SF
producer. Fixtures are local evidence and do not establish native or physical
x86 behavior. A stepwise ISA composition compares complete state after seeded
IE|SF FRNDINT, FCHS/FABS, FNSTSW and FNCLEX operations; it covers both finite
ties and SNaN→QNaN where FRNDINT performs quieting, and confirms FNCLEX clears
SF and all exception/summary bits before a final FNSTSW observes zero. Intel
SDM Vol.1 revision 081 §8.1.3.4 p.8-6 and §8.5.1.1 p.8-26 is the normative
reference. Further ES/B relaxation is deferred until a separate
pending-exception contract and final physical x86 acceptance.

Local checks passed: focused ISA full-state and rejection matrices; schema-v2
runner fixtures; formatting; offline workspace check; layout validation;
fixture validation and diff checks; Graphify update. No Mac mini, SSH, native
runtime or physical x86 checks were performed. ISSUE-019 remains open.

## S2-30bj — narrow bare `FXCH ST(1)`

Implemented exact no-prefix `D9 C9` decode and interpreter semantics. The gate
requires FCW `0x037f`, no FSW bits in `0x80ff`, and Valid-tagged canonical-Normal
values in both physical TOP slots before committing. Success swaps the complete
raw values, preserves their tags and TOP, preserves C0/C2/C3 as a local policy,
clears C1, records FIP, advances RIP by two, and retires once. GPR/RFLAGS/memory
and unrelated x87 payloads remain untouched. No aliases, memory form, stack
underflow, exception delivery, WAIT, JIT lowering, or native acceptance was
added. C0/C2/C3 preservation is deterministic local behavior; no hardware
comparison is claimed for those architecturally undefined fields.

Validation passed: `cargo test -p od-isa fxch_st1 --offline` (3 tests, including
truncated/faulting fetch, stop callback/address and budget-zero atomicity), the
exhaustive D9 decoder test (1), the FXCH schema test (1), and the added CLI
fixture test (1). The complete `fixture_v2` suite passed 184/184;
`cargo fmt --all -- --check`, `cargo check --workspace --all-targets --offline`,
and `python3 tools/check_layout.py` passed. `graphify update .` completed.
Graphify reported an existing C-header extraction warning for
`include/open_desktop/host_abi.h` and refreshed the graph to 5,035 nodes; these
do not affect the FXCH implementation. No Mac mini, SSH, native runtime, or
physical x86 checks were run. ISSUE-019 remains open for the explicitly
excluded classes and final host/physical acceptance.

## S2-30bk — expand bounded FXCH to all indices

Generalized exact no-prefix `D9 C8+i` (`i=0..7`) decode and interpreter behavior
through one indexed operation and the existing parameterized preflight helper.
The physical target is `(TOP+i)&7`. The self form inspects TOP only, preserves
its raw payload and tag, clears C1, updates FIP/RIP and retires. Nonzero forms
preflight both participants and swap all ten bytes. FCW/status gates and local
C0/C2/C3/FDP/FOP policies remain fixed; no hardware-equivalence claim was
added. No stack exception, new status behavior, JIT or native x87 admission was
introduced.

Coverage includes 14,336 success states for `i=1..7`, 1,024 self-exchange
states with deliberately malformed/Empty nonparticipant canaries, participating
tag/raw/environment rejection, all forbidden FSW bits, all eight D9 encodings,
prefix/neighbor/truncation/fetch checks, and literal schema/runner fixtures for
self-exchange, `i=7` wrap, nonadjacent repeated-index roundtrip, wrong target
tag, and JIT denial. `cargo test -p od-isa fxch --offline` passed 6/6;
`cargo test -p od-testkit x87_fxch_st1_schema --offline` passed 1/1; focused
runner FXCH tests passed 2/2; and `cargo test -p od-runner --test fixture_v2
--offline` passed 185/185. `cargo fmt --all -- --check`,
`cargo check --workspace --all-targets --offline`, `python3
tools/check_layout.py`, and `git diff --check` passed. `graphify update .`
completed with 5,043 nodes and 10,061 edges. It reported the existing C-header
extractor warning for `include/open_desktop/host_abi.h` and skipped unsupported
file types; neither affects this FXCH implementation. No Mac mini, SSH, native
runtime, or physical x86 checks were run. ISSUE-019 remains open for excluded
x87 classes/exceptions and physical/native acceptance.

## S2-30bl — canonical signed zero through bounded FXCH

Extended the local exact bare `D9 C8+i` FXCH profile to accept a participant
only when it is either a Valid-tagged canonical-Normal binary80 value or a
Zero-tagged exact signed zero. For nonzero indices, the interpreter preflights
both wrapped physical entries and commits a swap of both ten-byte payloads and
their physical tags. The self form validates and preserves only TOP. FCW,
FSW-mask, TOP, C1, condition-code retention, metadata, and explicit JIT/native
denial remain unchanged; no decoder or schema stream expansion was made.

The Normal-only matrix remains 15,360 full-state cases. Added 32,256 mixed
indexed states from nine ordered magnitude pairs, four sign combinations,
seven indices, eight TOP values, and sixteen prior CC patterns, plus 256
self-Zero states. Rejections include malformed Zero, Valid-tagged zero,
Empty/Special tags, unsupported binary80 classes, either invalid indexed
participant, and the existing environment/fetch/STOP/budget cases. A literal
CLI fixture asserts raw Zero/Normal wrap transfer and full tag-word exchange;
another performs opposite signed-zero transfer twice and checks byte/tag
roundtrip. JIT denial remains explicit.

Validation passed: `cargo test -p od-isa fxch --offline` (7/7),
`cargo test -p od-testkit x87_fxch_st1_schema --offline` (1/1), the targeted
signed-zero runner fixture (1/1), and full `cargo test -p od-runner --test
fixture_v2 --offline` (186/186). `cargo fmt --all -- --check`, `cargo check
--workspace --all-targets --offline`, `python3 tools/check_layout.py`, and
`git diff --check` passed. No Mini, SSH, native execution, or physical x86
check was run. The CPU contract records the verified FXCH source as Intel SDM
rev.093 Vol.2A p.3-407 and tag classes as rev.081 Vol.1 §8.1.7. ISSUE-019
remains open for Empty/stack faults, special classes, physical acceptance, and
the broader x87 contract. See the [plan](../plans/STAGE_2.md#s2-30bl-admit-canonical-signed-zero-to-bounded-fxch),
[CPU contract](../specs/CPU_CONTRACT.md#local-s2-30bl-canonical-signed-zero-through-bounded-fxch-sti),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30bl-canonical-signed-zero-fxch).
`graphify update .` completed at 5,049 nodes and 10,073 edges. It reported the
existing `include/open_desktop/host_abi.h` extraction warning and skipped
unsupported file types; neither affects this change.

## S2-30bm — bounded register-only FINCSTP and FDECSTP

Added exact bare Long64 `D9 F7`/`D9 F6` operations. They update TOP by +1/-1
modulo eight, clear C1, preserve C0/C2/C3 under a deterministic local policy,
and leave every raw x87 register byte and the complete tag word unchanged.
All register contents and tags are opaque: no tag/class reads, reclassification,
exchange, or Empty marking occurs. FIP captures old RIP; FDP/FOP, GPR, RFLAGS,
and memory retain their register-only behavior. The execution gate is present
state, FCW `0x037f`, and `FSW & 0x80ff == 0`.

Coverage includes 1,536 full-state successes (2 directions × 8 TOP values × 16
prior CC states × 6 opaque snapshots), separate TOP wrap, eight repeated
operations, both inverse pairs, all nine forbidden FSW bits, invalid FCW,
missing state, truncated fetch, STOP, zero budget, and adjusted-TOP FXCH
composition. Decoder and schema accept only the exact six requested streams;
four literal runner fixtures seed TOP=7/C1 with distinct raw canaries and mixed
including Empty tags, observe status after each direction, and observe both
inverse orders. JIT denial is asserted. These fixtures and matrices are local
interpreter/schema evidence, not native or physical x86 evidence.

Validation passed: focused `od-isa` FINCSTP/FDECSTP tests (4/4), adjusted-TOP
FXCH composition (1/1), focused schema allowlist test (1/1), and full
`cargo test -p od-runner --test fixture_v2 --offline` (187/187). The targeted
runner fixture also passed with explicit JIT denial. `cargo fmt --all
-- --check`, `cargo check --workspace --all-targets --offline`,
`python3 tools/check_layout.py`, and `git diff --check` passed. No Mac mini,
SSH, native runtime, or physical x86 checks were performed. ISSUE-019 remains
open for broader exception/native/physical behavior. See the [plan](../plans/STAGE_2.md#s2-30bm-bounded-register-only-fincstp--fdecstp),
[CPU contract](../specs/CPU_CONTRACT.md#local-s2-30bm-bounded-register-only-fincstp--fdecstp),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30bm-bounded-fincstpfdecstp).

## S2-30bn — bounded FLD1 and FLDZ

The exact bare Long64 forms `D9 E8` and `D9 EE` now push their specified constants only when x87 state is present, FCW is `0x037f`, the masked exception/status environment is clean (`FSW & 0x80ff == 0`), and `(old_TOP+7)&7` is Empty. FLD1 stores exact binary80 one with Valid tag; FLDZ stores exact zero with Zero tag. The implementation preflights before mutation, never reads old Empty payload bytes, updates TOP and clears C1, and records FIP. Other cells/tags and local deterministic undefined fields are preserved. The dedicated 1,536-case full-state matrix, atomic rejections, eight-push/ninth-reject, composition, four exact schema-success streams and occupied-destination zero-retirement fixture provide local evidence. JIT/native/physical behavior and broader stack/Special semantics remain outside scope; ISSUE-019 stays open. See [plan](../plans/STAGE_2.md#s2-30bn-bare-fld1-and-fldz-bounded-local-constant-push-slice), [CPU contract](../specs/CPU_CONTRACT.md#local-s2-30bn-bounded-fld1-and-fldz-constant-pushes), and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30bn-fld1-and-fldz-constant-pushes).


## S2-30bo — bounded FLD ST(0)

Implemented exact bare Long64 `D9 C0`. Preflight now requires canonical x87 state (FCW `0x037f`, `FSW & 0x80ff == 0`), a Valid/canonical-Normal or Zero/exact-signed-zero old physical TOP, and an Empty physical push slot `(old_TOP+7)&7`. It captures the old source bytes/tag before updating TOP, copies all ten bytes and the tag to the destination, leaves the source in place, clears C1, records FIP, and retires once. Empty destination bytes remain opaque. C0/C2/C3 retention is a local policy without a hardware-equality claim. `D9 C1..C7` remain rejected; no JIT/native acceptance was added.

The 5,120-state full-state interpreter matrix (five magnitudes × two signs × eight TOP values × sixteen prior CC patterns × four snapshots) checks raw bytes, full tags, TOP, C1, pointers, GPR/RFLAGS and canaries. Additional checks cover all occupied destination tags, Empty/Special/wrong-tag/malformed sources, combined invalid source plus occupied destination, nine forbidden FSW bits, FCW/missing state, seven duplications followed by atomic eighth rejection, FLD1/FLDZ→FLD ST(0), m80 store correctness, decoder prefixes/neighbors/truncation/fetch, and STOP/budget/fault boundaries. Schema admits exactly `d9c0`, `d9c0dfe0`, `d9e8d9c0dfe0`, and `d9eed9c0dfe0`; six literal fixtures seed signed-zero wrap, Normal wrap/status observation, both producers, Empty source rejection and occupied destination rejection. JIT denial is asserted by the CLI regression.

Validation passed: `cargo test -p od-isa fld_st0 --offline` (5/5), `cargo test -p od-testkit x87_fld_st0_schema --offline` (1/1), and `cargo test -p od-runner --test fixture_v2 --offline` (188/188). `cargo fmt --all -- --check`, `cargo check --workspace --all-targets --offline`, `python3 tools/check_layout.py`, and `git diff --check` passed. After the review correction, the focused S2-30bo CLI regression (1/1) and schema test (1/1) passed again, as did formatting, workspace check, layout and diff check. Review follow-up added `Operation::FldSt0` to the native runner's explicit x87-denial match. Its regression test is compiled only for Linux x86-64 and remains unexecuted on this macOS host; the denial assertion was therefore not independently run here. No Mac mini, SSH, native execution, or physical x86 checks were performed. ISSUE-019 remains open for Empty/Special sources, broader indices, stack exceptions and native/physical acceptance. See the [plan](../plans/STAGE_2.md#s2-30bo-bare-fld-st0-bounded-raw-copy-push-slice), [CPU contract](../specs/CPU_CONTRACT.md#local-s2-30bo-bounded-bare-fld-st0-d9-c0), and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30bo-bounded-fld-st0).


## S2-30bp — bounded FLD ST(i)

Generalized the S2-30bo raw-copy push gate to exact bare Long64 `D9 C0..C6`. The source is selected as physical `(old_TOP+i)&7`; the push destination remains `(old_TOP+7)&7`. The shared preflight requires the existing strict x87 environment, canonical-Normal/Valid or exact signed-Zero/Zero source and Empty destination before any mutation. The executor copies captured raw bytes/tag, retains source, updates TOP, clears C1, records FIP, retires once, and preserves destination canaries until successful commit. `D9 C7`, prefixed forms, unsupported source classes, JIT and native execution remain denied.

Added 35,840 full-state success cases (seven indices × five magnitudes × two signs × eight TOP values × sixteen prior CC patterns × four canary snapshots). The matrix checks nonzero-index source selection independently of old ST(0), TOP wrap, source retention, exact raw/tag copy, status update, pointer/GPR/RFLAGS canaries and no memory access. Existing atomic rejection/boundary coverage remains; the schema source predicate now computes source `(TOP+i)&7` separately from destination `(TOP+7)&7`. Nine literal fixtures cover every new index, indexed signed-zero/TOP wrap with Empty ST(0), wrong source tag and occupied destination. The runner checks snapshots, no retirement for rejects and JIT denial. The schema permits only the seven exact indexed instructions, each optionally followed by FNSTSW AX, plus the existing FLD1/FLDZ producer streams.

Local validation passed: `cargo test -p od-isa fld_sti --offline` (2/2), `cargo test -p od-isa fld_st0 --offline` (3/3), `cargo test -p od-testkit x87_fld_sti_schema --offline` (1/1), focused `cargo test -p od-runner --test fixture_v2 local_x87_fld_sti --offline` (1/1), and full `cargo test -p od-runner --test fixture_v2 --offline` (189/189). `cargo fmt --all -- --check`, `cargo check --workspace --all-targets --offline`, `python3 tools/check_layout.py`, and `git diff --check` passed. No Mac mini, SSH, native execution, or physical x86 checks were performed; those are reserved for the final physical substage. ISSUE-019 remains open for broader x87 behavior and physical acceptance. See the [plan](../plans/STAGE_2.md), [CPU contract](../specs/CPU_CONTRACT.md#local-s2-30bp-bounded-bare-fld-sti-d9-c0c6), and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30bp-bounded-fld-sti).


## S2-30bq — bounded bare FFREE ST(0)

Implemented exact unprefixed Long64 `DD C0`; all `DD C1..C7` aliases and prefixed forms remain unsupported. The strict gate requires present x87 state, FCW `0x037f`, and `FSW & 0x80ff == 0`. The physical old-TOP tag and raw payload are never inspected; success sets only that tag to Empty and records FIP. TOP, all payload bytes, other tags, C0..C3 including C1, GPR/RFLAGS/memory and other x87 fields remain unchanged; RIP advances two and retirement is one. Repeating FFREE on Empty is accepted. No exception/pending behavior or JIT/native admission was introduced.

Added 3,072 full-state successes across eight TOPs, sixteen prior CC patterns, four source tags and six opaque payload patterns. Atomic rejects cover each of nine forbidden FSW bits and their combined mask, bad FCW with otherwise-valid FSW, missing x87, decoder neighbors/prefix/truncation, and stop/budget/fetch boundaries. A separate `DD C0 90` execution confirms that `90` is fetched as the next NOP after FFREE retires. Schema is limited to `ddc0`, `ddc0dfe0`, `ddc0d9c0dfe0`, and `ddc0d9f7d9e8dfe0`. Literal fixtures cover TOP=7/C1=1 mixed canaries with an already-Empty entry, opaque SNaN, the FLD ST(0) Empty rejection, bad environment and `FFREE→FINCSTP→FLD1→FNSTSW` physical-cell reuse. The runner also asserts JIT denial.

Local validation passed: `cargo test -p od-isa bounded_ffree_st0 --offline` (2/2), exact DD decoder test (1/1), `cargo test -p od-testkit x87_ffree_st0_schema --offline` (1/1), and `cargo test -p od-runner --test fixture_v2 local_x87_ffree_st0 --offline` (1/1). `cargo fmt --all -- --check`, `cargo check --workspace --all-targets --offline`, `python3 tools/check_layout.py`, and `git diff --check` passed. Graphify was refreshed after the implementation and documentation edits. No Mac mini, SSH, native runtime or physical x86 checks were performed; ISSUE-019 remains open. See the [plan](../plans/STAGE_2.md#s2-30bq-bare-ffree-st0-bounded-tag-only-free-slice), [CPU contract](../specs/CPU_CONTRACT.md#local-s2-30bq-bounded-bare-ffree-st0-dd-c0), and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30bq-bounded-ffree-st0).

## S2-30br — bounded `FFREE ST(i)` for all indices

Generalized exact bare Long64 `DD C0..C7` to indexed `FfreeSti`. The selected
physical slot is `(old_TOP+i)&7`; present x87 state, FCW `0x037f`, and
`FSW & 0x80ff == 0` are required. The operation does not inspect the selected
tag or payload and changes only that tag to Empty. TOP, all raw payloads and
other tags, full FCW/FSW including C0..C3, FDP/FOP, GPR/RFLAGS/memory remain
unchanged; FIP records old RIP, RIP advances two, and retirement is one.
Repeated execution is idempotent. Stack exceptions/pending state, JIT/native
execution and physical x86 claims remain outside scope.

The full-state matrix covers 24,576 cases across eight indices, eight TOPs,
sixteen prior CC combinations, four tags and six opaque payload patterns,
including the original 3,072 `ST(0)` states. Atomic environment rejects cover
each forbidden FSW bit and their combined mask, invalid FCW and absent x87
state; decoder checks cover exact forms, prefix/neighbor/WAIT/memory denials.
The exact nineteen-stream schema includes bare indexed forms, FNSTSW forms,
the existing `ST(0)→FLD ST(0)` rejection, and `ST(7)→FLD1` reuse. Its fixture
starts the selected physical slot 6 with a Special tag and opaque payload, so
the successful FLD1 depends on FFREE emptying that occupied slot. Seven indexed
literal fixtures plus indexed environment rejection exercise CLI snapshots and
JIT denial.

Validation passed: `cargo test -p od-isa bounded_ffree_sti --offline` (2/2),
`cargo test -p od-isa exact_bare_dd_c0_to_c7 --offline` (1/1),
`cargo test -p od-testkit x87_ffree_sti_schema --offline` (1/1), focused indexed
CLI regression (1/1), full `fixture_v2` suite (191/191),
`cargo fmt --all -- --check`, `cargo check --workspace --all-targets --offline`,
`python3 tools/check_layout.py`, and `git diff --check`. No Mac mini, SSH,
native runtime or physical x86 checks were run. ISSUE-019 remains open. See
[plan](../plans/STAGE_2.md#s2-30br-generalize-bounded-ffree-to-all-sti-indices),
[CPU contract](../specs/CPU_CONTRACT.md#local-s2-30br-bounded-bare-ffree-sti-dd-c0c7),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30br-bounded-ffree-sti).

## S2-30bs — bounded bare FNOP

Implemented exact unprefixed Long64 `D9 D0`. Execution requires present x87
state, FCW `0x037f`, and `FSW & 0x80ff == 0`. It does not inspect TOP, FTW,
register payloads or memory. Success updates only FIP to old RIP, advances RIP
by two and retires once; all x87 fields other than FIP, GPR, RFLAGS, MXCSR and
pending state are preserved. C0..C3 including C1 are retained under the
existing local undefined-field policy. No exception or pending behavior,
native admission or JIT lowering was added.

The interpreter matrix covers 4,608 snapshots across TOP, all sixteen
condition-code combinations, six tag patterns and six payload layouts. Reject
coverage includes each forbidden FSW bit individually and together, invalid
FCW variants, missing x87 state, all prefixes, `9B D9 D0`, D9 D1..D7,
fetch/truncation, STOP and budget boundaries. Two consecutive FNOPs update FIP
at each instruction while retaining all other state; memory tracing confirms no
data access. Six literal fixtures cover opaque TOP=7/C1 state, all Empty tags,
repeated FNOP, `FFREE ST(7)→FNOP→FLD1`, ES|B and invalid FCW. The schema allows
only the four requested success streams and budget-one bare environment
rejections; the runner checks interpreter snapshots and JIT denial.

Local checks passed: `cargo test -p od-isa fnop_ --offline` (3/3),
`cargo test -p od-testkit x87_fnop_schema --offline` (1/1), focused runner CLI
fixture (`cargo test -p od-runner --test fixture_v2 local_x87_fnop_preserves_full_snapshot --offline`, 1/1), `cargo fmt --all -- --check`,
`cargo check --workspace --all-targets --offline`, `python3 tools/check_layout.py`,
and `git diff --check`. The full `cargo test -p od-isa --offline` run has 691
passes and three x87 reject-vector expectation mismatches recorded under
ISSUE-019: two canonical-Normal FABS/FCHS cases with historical SF|IE, and one
Special-tagged canonical subnormal FST/FSTP ST(i) case. They are unrelated to
FNOP. No Mac mini, SSH, native runtime, or physical x86 check was run. ISSUE-019
remains open. See the [plan](../plans/STAGE_2.md#s2-30bs-bare-fnop-with-a-bounded-environment-gate), [CPU contract](../specs/CPU_CONTRACT.md#local-s2-30bs-bounded-bare-fnop-d9-d0), and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30bs-bounded-fnop).
## S2-30bt — bounded bare FXAM

The local interpreter now admits exact bare Long64 `D9 E5` for Valid-tagged
canonical Normal or Zero-tagged exact signed zero at physical old TOP, under
FCW `0x037f` and clean `FSW & 0x80ff`. It replaces C0/C1/C2/C3 with Normal
`0x0400` or Zero `0x4000`, plus C1 for a negative source; TOP and all other
state are retained and FIP records old RIP. FTST admission was not widened.
The 5,120-state success matrix (four Normal representatives: minimum normal,
1, 1+ULP and maximum finite, plus exact Zero) and 10,752 distinct class/tag
rejection product, seven
exact schema streams, seven producer/repeat/signed-result fixtures, five
literal rejects, and explicit JIT denial provide local evidence only.

Validation passed: `cargo test -p od-isa fxam_ --offline` (three focused
tests, including the 10,752-case rejection matrix), `cargo test -p od-testkit
x87_fxam_schema --offline`, and `cargo test -p od-runner --test fixture_v2
local_x87_fxam --offline`. Formatting, workspace check, layout validation and
`git diff --check` passed. No Mac mini, SSH, native runtime, or physical x86 checks were
performed; ISSUE-019 remains open. See [plan](../plans/STAGE_2.md#s2-30bt-bounded-bare-fxam-normal-and-signed-zero-classification),
[CPU contract](../specs/CPU_CONTRACT.md#local-s2-30bt-bounded-bare-fxam-d9-e5),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30bt-bounded-fxam).

## S2-30bu — bounded `FCMOVB ST(0), ST(1)`

Implemented exact bare Long64 `DA C1`. Both old physical TOP and its next
physical slot are preflighted as Valid canonical Normal or Zero exact signed
zero before CF is observed. A taken move copies ten raw bytes and tag; an
untaken move preserves both operands. The executor preserves all status bits
and requires `FSW & 0x82ff == 0`, which is a deliberately local C1=0 condition;
no physical value is claimed for FSW.C1. TOP and all other fields are preserved
except FIP, RIP and retirement. Existing `DA E9` is unchanged. No memory, pop,
exception, JIT, native or physical behavior was added.

The local interpreter matrices cover 51,200 success snapshots using four
canonical Normal values (minimum, 1, 1+ULP and maximum finite) in both signs
plus exact ±0, and 21,504 invalid class/tag snapshots, plus FCW/status,
missing-x87 and decode checks.
Five exact schema streams and ten literal fixture inputs cover taken/untaken,
distinct tags, negative zero, TOP wrap, repeat, FLD1/FCHS/FCOMI producers,
invalid source with CF=0, invalid destination, C1=1 and bad FCW. Results are
local only; native/JIT remain closed and ISSUE-019 stays open.

The ST(1)-only register extent is historical and is superseded by S2-30bv for
the indexed register forms `DA C0..C7`.

## S2-30bv — bounded indexed `FCMOVB ST(0), ST(i)`

The decoder and interpreter now admit exact bare Long64 `DA C0..C7` and use
the ModR/M low bits as `i`. For nonzero indices the implementation preflights
both old physical TOP and `(TOP+i)&7` before observing CF, including the
untaken path. Self-index `i=0` checks only TOP and preserves its bytes/tag for
both CF outcomes; unrelated physical slots are not inspected. The existing
FCW `0x037f`, `FSW & 0x82ff == 0`, Valid canonical-Normal or Zero exact-signed-
zero domain remains. Taken moves copy exact bytes and tag. `DA E9` remains
unchanged, and JIT/native remain closed.

Local interpreter coverage passed 363,520 full-state successes (51,200 per
`i=1..7` plus 5,120 for `i=0`) and 161,280 invalid class/tag cases (21,504 per
nonzero index plus 10,752 for `i=0`). The matrix includes both CF values,
TOP, all C0/C2/C3 and PF/ZF combinations, signed Normal/Zero values, and
invalid operands even when CF is clear. Per-index environment and execution
boundary checks passed. The exact schema test admits 19 streams; 8 literal
CLI success fixtures cover DA C0..C7 and the runner checks interpreter
snapshots plus JIT denial. Additional indexed rejects cover an invalid source
under CF=0, C1 and FCW gates. `cargo test -p od-isa --lib fcmovb --offline`,
`cargo test -p od-testkit --lib
x87_fcmovb_schema_admits_only_indexed_exact_streams_and_operand_rejects
--offline`, and the focused runner fixture test passed. No physical/native
checks were run; the remaining x87 limitations stay under ISSUE-019. See
[contract](../specs/CPU_CONTRACT.md#local-s2-30bv-bounded-indexed-fcmovb-sti-da-c0c7),
[plan](../plans/STAGE_2.md#s2-30bv-indexed-bare-fcmovb-st0-sti), and
[fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30bv-bounded-indexed-fcmovb-sti).

## S2-30bw — bounded `FCMOVNB ST(0), ST(1)`

Implemented exact unprefixed Long64 `DB C1`. It uses the same strict bounded
preflight gate as indexed FCMOVB, including preflight of both old physical TOP
and TOP+1 before CF is inspected. CF=0 copies the source's ten raw bytes and
tag; CF=1 preserves both operands. FIP/RIP/retirement update as specified;
other state is preserved. Existing FCMOVB and DB instructions remain intact.

Local coverage includes 51,200 CF-complement success comparisons against
FCMOVB, 21,504 invalid class/tag rejection snapshots, environment and boundary
rejection, exact bare/prefix/neighbor decoding, five schema streams, and ten
literal CLI fixtures with JIT denial. These are local checks only. No native
or physical x86 acceptance is claimed; ISSUE-019 remains open. See [plan](../plans/STAGE_2.md#s2-30bw-bare-fcmovnb-st0-st1), [CPU contract](../specs/CPU_CONTRACT.md#local-s2-30bw-bounded-fcmovnb-st0-st1), and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30bw-bounded-fcmovnb-st0-st1).

Focused validation passed: `cargo test -p od-isa fcmovnb --offline` (3/3),
`cargo test -p od-isa fcmovb --offline` (6/6), both FCMOV testkit schema cases
(1/1 each), and both FCMOV runner fixture tests (1/1 each). Workspace check,
formatting, layout validation, `git diff --check`, and the Linux x86_64
`od-x86-runner` cross-target check passed. The latter confirms the native
admission table compiles with FCMOVB/FCMOVNB denied. No physical/native checks
were run.

## S2-30bx — bounded indexed `FCMOVNB ST(0), ST(i)`

The exact bare Long64 decoder form is now `DB C0..C7`; its ModR/M low bits
select the source. This reuses the bounded FCMOV preflight and raw-copy
executor. Indices 1–7 validate physical old TOP and `(TOP+i)&7` before CF,
including CF=1. Self-index validates TOP only, preserves its payload and tag
for either CF value, and ignores unrelated physical slots. The existing
FCW/FSW gates and Normal-or-exact-signed-Zero operand domain remain unchanged.
FIP, RIP and retirement are the only machine state updates. Memory forms,
exceptions, native execution and JIT remain outside the profile.

Local interpreter coverage asserts 363,520 success snapshots comparing full
x87 state, RFLAGS and RIP (51,200 for each nonzero index and 5,120 for
self-index), and 161,280 invalid class/tag snapshots comparing complete
machine state (21,504 per nonzero index and 10,752 for self-index). It compares each
FCMOVNB index against matching FCMOVB with inverted CF and checks independent
expected snapshots. Tests cover every indexed status pattern, FCW/FSW rejects,
missing x87 state, prefixes, fetch/truncation, STOP and budget boundaries.
The schema admits exactly 19 streams: `dbc0..dbc7` with budget 1, each followed
by `dfe0` with budget 2, `dbc1dbc1dfe0` with budget 3, and the existing two
producer streams with budget 6. Eight new literal fixtures cover DB C0 and
DB C2..C7, including both self-index CF paths, TOP wrap, signed Zero and both
Normal/Zero transfer directions. Together with the existing C1 fixture, the
runner checks all indices, full x87 snapshots and JIT denial. The former
DB C1-only S2-30bw scope is historical and superseded.

Validation passed: `cargo test -p od-isa --lib fcmovnb --offline` (3/3),
`cargo test -p od-testkit --lib
x87_fcmovnb_schema_admits_only_nineteen_exact_streams_and_reject_profiles
--offline` (1/1), `cargo test -p od-testkit --offline` (181/181), and
`cargo test -p od-runner --test fixture_v2
local_x87_fcmovnb_snapshots_and_denies_jit --offline` (1/1). `cargo check
--workspace --all-targets --offline`, `cargo fmt --all -- --check`,
`python3 tools/check_layout.py`, and `git diff --check` also passed. No physical
x86, native runtime or JIT execution was performed; ISSUE-019 remains open.
Independent review identified and corrected the success-snapshot scope wording;
no code or runtime finding remained. Documentation links and diff checks passed,
and Graphify was refreshed after the final source and document edits.

## S2-30bz — bounded indexed `FCMOVE ST(0), ST(i)`

The Long64 interpreter now admits exact unprefixed `DA C8..CF`, with the
ModR/M low bits selecting the source. For `i>0`, it preflights physical old
TOP and `(TOP+i)&7` before reading integer RFLAGS.ZF, including when ZF is
clear; self-index validates only TOP and leaves unrelated slots opaque. ZF=1
copies all ten raw source bytes and its tag to TOP; ZF=0 preserves the cells.
FSW.C3 is not the condition. The existing gates remain FCW `0x037f` and
`FSW & 0x82ff == 0`; remaining x87 state, RFLAGS, GPRs and memory are
preserved, while FIP, RIP and retirement update. Existing FCMOVB and FUCOMPP
decode paths remain intact.

Unit coverage checks 363,520 success snapshots and 161,280 invalid class/tag
rejections across all indices. It also checks each forbidden FSW bit, bad FCW,
missing x87, per-index decode and execution boundaries. Two real producer
sequences inspect state after FCOMI, after FCHS and after FCMOVE. The schema
admits exactly 19 streams: all eight bare encodings, all eight with `DF E0`,
repeat and two producer streams. New literal fixtures cover indices 0 and
2..7; existing ST(1) fixtures cover index 1. Runner assertions compare full
x87 state for every success fixture, including an independently calculated
two-step expectation for the repeat fixture, and deny JIT. The two producer
cases use hand-calculated complete x87 snapshots rather than an interpreter
prefix as their expected result. Rejection fixtures cover invalid
source with ZF=0 and indexed environment failure in addition to the prior
destination/status cases. All evidence is local; no physical x86, native or
JIT execution was performed. ISSUE-019 remains open.

Validation passed: `cargo test -p od-isa fcmove --offline` (4/4),
`cargo test -p od-testkit x87_fcmove_schema --offline` (1/1), and
`cargo test -p od-runner --test fixture_v2
local_x87_fcmove_snapshots_and_denies_jit --offline` (1/1). `cargo fmt --all
-- --check`, `cargo check --workspace --all-targets --offline`,
`python3 tools/check_layout.py`, and `git diff --check` also passed. No physical
x86, native runtime or JIT execution was performed; ISSUE-019 remains open.
Graphify was refreshed after source and documentation edits. Independent
review is handled by the parent task.

Review follow-up corrected three stale denial expectations without changing
runtime behavior: the FCMOVB schema rejects unsupported `DA E8` instead of
supported indexed `DA C8`; the FNINIT decoder test explicitly allows the
supported FCMOVNB range `DB C0..C7`; and the FST/FSTP rejection vector now uses
a pseudo-denormal rather than a canonical subnormal admitted by the bounded
profile. The focused tests and testkit schema pass. A full
`cargo test -p od-isa --offline` run reports 706 passed and only the two
previously documented FABS/FCHS status-vector mismatches; see ISSUE-019.

## S2-30ca — bounded bare `FCMOVNE ST(0), ST(1)`

Historical ST(1)-only slice; superseded by S2-30cb.

Added the exact bare Long64 `DB C9` decoder and interpreter path. It reuses the
existing FCMOV preflight, so TOP and TOP+1 are checked before ZF is read,
including ZF=1. ZF=0 copies the source's raw ten-byte payload and tag to TOP;
ZF=1 leaves it unchanged. FCW/FSW gates, C1=1 rejection, full-state
preservation and FIP/RIP/retirement use existing bounded execution contracts.

Local unit checks cover exact bare/prefix/neighbor decoding, 51,200 full-state
successes and 21,504 atomic class/tag rejections. Testkit admits exactly five
streams: `dbc9`, `dbc9dfe0`, `dbc9dbc9dfe0`, and the two required producer
sequences. Two literal success fixtures check complete x87 payload/tag outcomes
for ZF=0 and ZF=1; a third rejects an invalid source with ZF=1. All deny JIT.
Two producer sequences also verify the
ZF values after FCOMI and the resulting negative-one preservation or positive-
one copy. Targeted tests and workspace offline check pass. `tools/check_layout.py`
reports that `.git` is unmapped because it is a worktree pointer file, and
reports the two historical oracle manifest links above as broken because their
ignored `work/` artifacts are not copied into this isolated worktree; both
manifests exist in the shared checkout. The report links were left intact. No
native or physical x86 acceptance is claimed. ISSUE-019 remains open.

## S2-30cb — indexed bare `FCMOVNE ST(0), ST(i)`

Expanded the exact unprefixed Long64 FCMOVNE form from DB C9 to DB C8..CF.
The low ModR/M bits now select the physical source index, while the established
FCMOV gate remains unchanged. Nonzero indices preflight old TOP and
`(TOP+i)&7` before ZF; self-index preflights TOP only. ZF=0 performs an exact
raw ten-byte/tag transfer, and ZF=1 preserves the cells. Existing indexed
FCMOVNE shares the bounded FCMOV operand validation and remains JIT/native
closed.

Local evidence includes 363,520 complete success states, 161,280 invalid
class/tag rejection snapshots, exact 19-stream schema coverage, source-indexed
environment/status/boundary checks, and the complementary indexed FCMOVE
property. Handwritten fixtures cover indices 0 and 2..7, self-index both ZF
outcomes, wrap, Normal/Zero and negative-zero transfer, repeats, both FCOMI
producer streams, invalid source with ZF=1, and forbidden environment. Runner
checks full x87 snapshots and RFLAGS, FIP/RIP, retirement, memory preservation,
and per-fixture JIT denial. Verification passed for `cargo fmt --all -- --check`, `cargo check --workspace
--all-targets --offline`, all 87 `od-isa` decoder tests, all 7 focused FCMOVNE
interpreter tests (including the 363,520/161,280 matrices), all 183 `od-testkit` tests, the exact schema
test, and the complete runner fixture suite (197 tests). The broader combined
`od-isa`/`od-testkit` run still has two previously recorded unrelated FABS/FCHS
status-vector failures (`rejection=3` and `rejection=18` under SF|IE); the
remaining 713 `od-isa` tests passed. `tools/check_layout.py` reports the known
worktree pointer `.git` as an unmapped root file and two broken links to ignored
oracle manifests absent from this isolated worktree; those artifacts and links
exist in the shared checkout. No native or physical execution is claimed;
ISSUE-019 remains open.

## S2-30cc — historical bounded bare `FCMOVBE ST(0), ST(1)`

Added exact unprefixed Long64 `DA D1`. The interpreter reuses the existing
bounded FCMOV participant preflight for physical TOP and TOP+1 and performs the
raw ten-byte/tag copy exactly when integer CF or ZF is set. It preflights both
cells even when both flags are clear, requires FCW `0x037f` and
`FSW & 0x82ff == 0`, and otherwise preserves x87 and integer state except FIP,
RIP and retirement. All other DA encodings and prefixes remain outside this
admission; JIT is denied.

Local ISA tests cover 51,200 success snapshots over ten operand values, all
four CF/ZF rows, both PF values, all eight TOP values and eight status C0/C2/C3
combinations. The 86,016 invalid class/tag snapshots cover both participant
positions, signs, every condition/PF row, TOP and condition-code combination.
Missing x87 state, FCW/FSW gates and fetch/budget/stop boundaries are also
checked. The schema admits only the six requested byte streams. Ten literal
fixtures cover the truth table, negative-zero raw/tag transfer, TOP wrap,
FNSTSW, both repeats, three FCOMI producers (greater, equal then FCHS, less),
invalid participant and bad environment. The runner compares complete machine
state and memory and confirms JIT denial for successes and rejects.

Verification: `cargo check --workspace --all-targets --offline`,
`cargo fmt --all -- --check`, all 184 `od-testkit` tests, and all 198
`od-runner` fixture tests pass. The full `od-isa` run has 718 passed and the
two pre-existing FABS/FCHS status expectation failures recorded under
ISSUE-019 (`rejection=3` and `rejection=18`); the new FCMOVBE matrices pass.
`tools/check_layout.py` reports the known worktree pointer `.git` as an
unmapped root file and two broken links to ignored oracle manifests absent
from this isolated worktree; those files exist in the shared checkout, so the
links were not changed. No native or physical x86 execution is claimed, and
ISSUE-019 remains open.

## S2-30cd — indexed bare `FCMOVBE ST(0), ST(i)`

Extended the local bounded form to exact bare Long64 `DA D0..D7`. The decoder
retains `ModR/M & 7` in `Operation::Fcmovbe { source }`. The interpreter
preflights physical TOP and `(TOP+i)&7` before integer CF/ZF for i>0; i=0
preflights only TOP. Present x87 state, FCW `0x037f`, `FSW & 0x82ff == 0`, and
Valid canonical Normal or Zero exact signed-zero tags remain required for
every used operand, even when the condition is false. When CF or ZF is set,
the source raw bytes and tag are copied to TOP; otherwise it is preserved.
Only FIP, RIP and retirement change. C1=1, exception delivery, memory forms,
JIT, native execution and physical x86 acceptance remain outside the slice.

The interpreter matrices cover 363,520 complete success states and 645,120
atomic invalid class/tag rejects across indices, all CF/ZF rows, PF, TOP and
C0/C2/C3. The exact fixture schema admits twenty streams: eight bare indexed
forms, each optionally followed by FNSTSW AX, the prior repeat, and three
prior FCOMI producers. Literal fixtures cover each index, all four self-index
rows, wrap, signed-zero/tag transfer, untaken and taken invalid participants,
and an indexed bad environment. Runner checks include complete machine state
and memory, RFLAGS, RIP/FIP, retirement, repeat execution, and JIT denial.

Verification: formatting, `cargo check --workspace --all-targets --offline`,
all 184 `od-testkit` tests, and all 198 `od-runner` fixture tests pass. The
full `od-isa` suite has 718 passed and the two previously recorded unrelated
FABS/FCHS expectation failures (`rejection=3` and `rejection=18` under SF|IE);
all S2-30cd focused tests pass. `tools/check_layout.py` reports the known
worktree `.git` pointer as an unmapped root file and two missing ignored oracle
manifest links in this isolated worktree; those artifacts exist in the
shared checkout. No native or physical execution is claimed; ISSUE-019
remains open.

## S2-30ce — bare `FCMOVNBE ST(0), ST(1)`

Implemented exact unprefixed Long64 `DB D1` only. The interpreter requires a
present x87 state, FCW `0x037f`, `FSW & 0x82ff == 0`, and Valid canonical-Normal
or Zero exact-signed-zero participants at physical TOP and TOP+1. Both cells
are checked before integer CF/ZF; raw ten-byte/tag transfer occurs only when
both flags are clear. C1=1, other encodings, prefixes, memory, exception
delivery, JIT, native and physical execution remain unsupported.

Local checks cover 51,200 success states, split into 12,800 taken and 38,400
untaken cases, plus 86,016 invalid participant snapshots. They include a
same-CF/ZF complement property against FCMOVBE, all environment gates,
decode/prefix and execution boundaries, producer checkpoints after FCOMI/FCHS,
and complete intermediate snapshots for both steps of a repeat. The exact
six-stream fixture schema contains the
bare and FNSTSW forms, repeat, and greater/equal-then-FCHS/less FCOMI producer
streams. Eight literal success fixtures and two atomic rejects pass full
machine/memory runner checks and deny JIT. This is local interpreter evidence;
it does not establish native or physical x86 behavior, and ISSUE-019 remains
open.

Verification passed for `cargo fmt --all -- --check`,
`cargo check --workspace --all-targets --offline`, all 185 `od-testkit` tests,
and all 199 `od-runner` fixture tests. The full `od-isa` suite reports 725
passed and two previously documented FABS/FCHS expectation failures at
`rejection=3` and `rejection=18` under SF|IE (ISSUE-019); all seven focused
FCMOVNBE tests pass. `git diff --check` passes. `tools/check_layout.py` still
reports the known isolated-worktree `.git` pointer and two broken links to
ignored oracle manifests absent from this worktree. No directory structure
changed. No native or physical x86 execution was performed.

## S2-30cf — indexed bare `FCMOVNBE ST(0), ST(i)`

Extended exact unprefixed Long64 `DB D1` to `DB D0..D7`. The indexed
`Operation::Fcmovnbe { source }` flows through the existing shared preflight:
FCW `0x037f`, `FSW & 0x82ff == 0`, validate TOP/source before CF/ZF for i>0,
and validate only TOP for self-index. Valid/Zero signed-zero participants are
copied byte-for-byte with their tag only when CF=ZF=0; invalid state rejects
atomically. Existing S2-30ce ST(1) repeat and producer streams remain admitted.

The local matrices cover 363,520 successes (358,400 i>0 and 5,120 self) and
645,120 invalid class/tag snapshots (602,112 i>0 and 43,008 self). The
indexed FCMOVBE complement check covers every index; for i=0 both preserve
the same cell. The schema has
exactly 20 streams: eight bare DB D0..D7 forms at budget 1, their eight FNSTSW
AX forms at budget 2, D1 repeat at budget 3, and three producer streams at
budget 6. Handwritten fixtures cover self-condition rows, indices 2..7,
wrap, both Normal/Zero transfer directions and signed-zero copy, taken and
untaken invalid source, status observation, repeat, producers and indexed
environment rejection. Runner
snapshots compare full state and memory and deny JIT for every index.

Verification passed: 10 focused `od-isa` tests, all 185 `od-testkit` tests, all 101 `od-runner`
unit tests and 199 fixture integration tests, `cargo check --workspace --all-targets --offline`, `cargo fmt --all -- --check`, and `git diff --check`. `tools/check_layout.py` still reports the known isolated-worktree `.git` pointer and two links to ignored oracle manifests absent from this worktree. No directory structure changed. No Mini, SSH, native runtime or physical x86 verification was performed; ISSUE-019 remains open.

## S2-30ch — indexed bare `FCMOVU ST(0), ST(i)`

Expanded exact unprefixed Long64 `DA D9` to `DA D8..DF`, storing each source
index in `Operation::Fcmovu`. Execution reuses the existing bounded FCMOV
preflight: FCW `0x037f`, `FSW & 0x82ff == 0`, and Valid canonical-Normal or
Zero exact-signed-zero operands. Both participants are checked before PF for
`i>0`, including untaken moves; self-index checks TOP only and retains its cell
without inspecting other registers. PF=1 copies the ten raw bytes and tag;
PF=0 preserves the destination. C1 remains closed and Special/NaN inputs are
not modeled.

Interpreter tests cover 363,520 successes (358,400 for `i>0`, 5,120 self) and
161,280 invalid class/tag rejects (150,528 for `i>0`, 10,752 self), including
both signs, PF, all TOP values and x87 condition-code patterns. The exact
schema admits 18 streams: eight bare forms at budget 1, their FNSTSW forms at
budget 2, DA D9 repeat at budget 3, and the retained TEST producer at budget 5.
Nine new bare successes, a repeat, two retained producer fixtures,
invalid-source PF=0/1, invalid destination, and C1/ES/B/FCW rejects have
full-state and memory runner assertions with JIT denial.

S2-30cg's earlier ST(1)-only evidence is retained as 512 success and 768
invalid class/tag snapshots; S2-30ch is the indexed acceptance claim. The four
focused `od-isa` decoder/interpreter tests, the exact-schema `od-testkit` test,
the retained producer fixture test and the indexed success/reject runner test
all pass. `cargo fmt --all -- --check`,
`cargo check --workspace --all-targets --offline`, and `git diff --check` pass.
`graphify update .` completed; it reports the existing `host_abi.h` parser
warning and files without extractable symbols. No layout changed. This
substage does not run Mac mini, SSH, native runtime or physical x86 checks;
ISSUE-019 remains open.

## S2-30ci — historical exact bare `FCMOVNU ST(0), ST(1)`

The original exact unprefixed Long64 `DB D9` one-index implementation used the
strict FCMOV environment and participant gates. This historical extent is now
superseded by indexed S2-30cj; its fixtures remain compatibility evidence.
Unordered/NaN behavior remains unsupported and C1=1 remains rejected.

The historical focused interpreter tests passed 51,200 complete success
snapshots and 21,504 invalid class/tag rejects. The four-stream schema and
producer checks describe only the original DB D9 extent.

## S2-30cj — indexed bare `FCMOVNU ST(0), ST(i)`

Implemented exact unprefixed Long64 `DB D8..DF`, carrying `ModR/M & 7` into the
operation. For `i>0`, the interpreter preflights physical TOP and
`(TOP+i)&7` before PF even when untaken; self-index validates TOP only and
does not inspect other cells. Valid participants are Valid canonical Normal
or Zero exact signed zero under FCW `0x037f` and `FSW & 0x82ff == 0`. PF=0
raw-copies the source bytes and tag; PF=1 keeps the destination. C1 stays
closed, Special/NaN semantics are unsupported, and the only other state
changes are FIP=old RIP, RIP advancement and retirement. Rejections are atomic.

Local matrices passed for 363,520 success snapshots (358,400 `i>0`, 5,120
self) and 161,280 invalid class/tag snapshots (150,528 `i>0`, 10,752 self).
The 18-stream schema, decode neighbor/prefix/truncation and execution-boundary
checks for all indices, testkit full-state/memory literal interpreter rows, wrap and both
signed-zero/tag transfer directions, repeat, both TEST producer intermediates,
independent indexed FCMOVU complement snapshots, and explicit native/JIT
denial passed.

Validation passed: focused `od-isa` FCMOVNU tests (5/5), exact `od-testkit`
schema and full-state literal testkit tests; the full
`cargo test -p od-runner --test fixture_v2 --offline` suite passed 202/202. No Mac mini, SSH, native runtime
or physical x86 checks were run; those remain in the shared final physical
substage. ISSUE-019 remains open.

## S2-18g — non-REP STOSW/D/Q interpreter slice

The interpreter now accepts `66 AB` STOSW, bare `AB` STOSD, and effective
`REX.W + AB` STOSQ. All `48..4F` REX.W forms are admitted; REX.W wins over 66,
while a REX before later legacy prefix is normalized away (`48 66 AB` is word,
`66 48 AB` is qword). Bare `AA` STOSB and exact `F3 AA` REP STOSB retain their
previous behavior. REP AB, LOCK, address/segment overrides, and effective
non-W REX remain unsupported.

Each successful instruction issues one width-sized write of low RAX to the
pre-state RDI, then updates RDI with DF-directed wrapping arithmetic, advances
RIP, and retires once. RAX, RSI, RCX, and RFLAGS remain unchanged. Tests confirm
no RSI read/validation for ordinary, alias, unmapped, and noncanonical RSI
values. Noncanonical destination is #GP(0); bounded mapping and permission
errors remain harness errors, while an explicitly injected architectural
backend fault is preserved. Write, mapping, permission, injected-fault, and
fetch-boundary failures leave CPU state and RAM unchanged. No LOCK/SMP
atomicity or native/JIT behavior is claimed.

Local evidence passed: 6,144 full-state successes across all widths, DF values,
six RAX patterns, 64 arithmetic-flag combinations, and two RAM backgrounds;
every successful path records exactly one data write and no data reads. The 35
prefix-reject matrix, all REX/66 order variants, alignment residues, page
crossing, writable-region end, RDI wrapping/noncanonical post-index, injected
architectural/backend errors, and truncated fetch also pass. Exactly 20 new
literal schema-v2 fixtures cover positives, unmapped/read-only/noncanonical
destinations, zero budget, and both normalization orders. Their fixture-v2 CLI
test verifies complete state and RAM answers and atomic failures.

Validation passed: `cargo test -p od-isa --offline` (747/747),
`cargo test -p od-testkit --offline` (195/195), `cargo test -p od-runner --test fixture_v2 --offline` (204/204), including the
new literal fixture CLI test, `cargo fmt --all -- --check`, `cargo check --workspace --all-targets
--offline`, generator freshness, and `git diff --check`. `tools/check_layout.py`
still reports the worktree `.git` pointer as an unmapped root file and two
pre-existing report links to ignored oracle manifests absent from this checkout.
Graphify was updated; it reported its existing parser warning for
`include/open_desktop/host_abi.h`. No directory layout changed. No Mac mini, SSH,
native runtime, or physical x86 checks were run. Those checks remain in the
shared final acceptance substage; ISSUE-016 and ISSUE-019 remain open.

## S2-18h — non-REP SCAS B/W/D/Q interpreter slice

The local Long64 interpreter accepts bare `AE` SCASB, `66 AF` SCASW, bare
`AF` SCASD, and effective `REX.W + AF` SCASQ with address-size 64 and flat
`ES:[RDI]`. All `48..4F` REX.W variants are supported; REX.W overrides 66,
while misplaced REX before a later legacy prefix is normalized away. REP,
LOCK, 67, segment overrides, prefixed AE, and effective non-W REX on AF remain
unsupported. Existing `F3 AA` REP STOSB behavior is unchanged. No JIT/native
admission was added.

SCAS performs one width-sized source read from pre-state RDI, compares
AL/AX/EAX/RAX with memory using CMP semantics, and only then commits the six
arithmetic flags and DF-directed wrapping RDI update. It preserves all of RAX,
RSI, RCX, RAM, DF and other state. Successful retirement advances RIP once and
defines `ARITHMETIC_FLAGS`. A noncanonical source produces #GP(0); missing
mapping and permissions remain harness errors. A faulting SCAS commits neither
state nor memory, while a preceding retired TEST remains visible.

Evidence includes an independent 6,144-case state/flags matrix, exact single
read/no-write traces, 48 unsupported-prefix cases, every REX.W byte and 66/REX
ordering, fault/budget boundaries, and TEST→SCAS AF-definedness on success and
fault. Exactly 24 literal schema-v2 fixture cases were added; the generator
serializes predetermined values only. Schema/profile are unchanged. The 24
fixtures have local interpreter CLI coverage; they are not native/JIT evidence.

Validation passed: `cargo test -p od-isa --offline` (747/747),
`cargo test -p od-testkit --offline` (201/201), `cargo test -p od-runner
--offline` (101 unit + 205 fixture-v2 tests), and
`cargo check --workspace --all-targets --offline`. Focused SCAS testkit
coverage passed (6/6), and the SCAS CLI fixture test passed (1/1). All 61
stage2 fixture generators passed `--check`; the 73 generator unittest checks
passed. `cargo fmt --all -- --check` and `git diff --check` passed. Graphify
was updated. Independent gpt-6.1-sol high review found and verified one
SCAS test-fixture mistake (an STOSB opcode in the noncanonical post-index
case); the case was changed to `AE` and its exact read trace now passes. The
review found no remaining issues and independently checked all 24 fixture
snapshots. `tools/check_layout.py` reports the existing worktree `.git`
pointer as unmapped and two pre-existing report links to ignored oracle
manifests absent from this checkout. No layout change was made. No Mac mini,
SSH, native runtime, or physical x86 checks were run. ISSUE-016 and ISSUE-019
remain open.

## S2-19b — bounded `F3 AB` REP STOSW/D/Q (local)

Extended the existing REP STOS operation to word, dword, and qword forms while
keeping exact `F3 AA` byte behavior. Accepted AB prefix forms are F3 with
optional 66, no segment/address override, and absent REX or effective REX.W;
all REX.W values are admitted. Normalization covers `F3 48 66 AB` as word and
`F3 66 48 AB` as qword. All other requested prefix combinations remain
Unsupported, and the native/JIT gate was not expanded.

The loop writes the low accumulator width in one all-or-error store, then
commits DF-directed wrapping RDI and RCX decrement. Zero count avoids access.
Completion is checked before STOP, which is checked before the 1024-successful
store work limit. Partial STOP/limit/fault leaves RIP at the REP start and
retirement zero; earlier successful stores persist. Instruction budget still
counts the completed REP as one instruction. RAX, RSI, RFLAGS and definedness
remain unchanged during one `run()`; definedness resets on a new run.

Added the 12,288-state normal completion matrix, exact prefix and byte boundary
checks, and 28 literal schema-v2 fixtures with full expected-state sidecar.
Fixture generation writes the fixed case constants. ISSUE-016 and ISSUE-020
remain open. Mac mini, SSH, native and physical checks were not run and remain
reserved for final acceptance.

Final local validation for S2-19b: `cargo fmt --all -- --check`,
`cargo check --workspace --all-targets --offline`, the complete offline test
suites for `od-isa`, `od-testkit`, and `od-runner`, and the S2-19b fixture
freshness check passed. Focused assertions cover the complete 12,288-state
matrix, 96 write failures, 16 STOP/replay paths with exact access sequences,
24 work-limit boundaries, STOP after store 1024, bounded `RCX=u64::MAX`,
zero-count invalid RDI, fetch/budget edges, page/alignment crossing, pointer
wrap, byte regressions, TEST definedness, and CLI JIT denial. Native admission
remains byte-only; its fail-closed condition was checked by static review, but
the native runner test was not executed on this host. An independent static
review found no remaining concrete defects.

`python3 tools/check_layout.py` was attempted but reports the managed worktree's
`.git` marker as an unmapped root file and two absent ignored oracle manifests
referenced by the existing report. No structure was added by S2-19b. No
Mac mini, SSH, native execution, or physical x86 checks were run.

## S2-19c — bounded `F3 AC/AD` REP LODSB/W/D/Q (local)

Added `Operation::RepLods { size }` for exact REP LODSB/W/D/Q profile forms.
Admission accepts exact `F3 AC`, F3 plus optional 66 on `AD`, no segment or
address-size override, and no REX or effective REX.W; every `48..4F` REX.W is
admitted. Normalization yields word for `66 F3 AD` and `F3 48 66 AD`, and qword
for `F3 66 48 AD`. `F3 66 AC`, F2/LOCK/67/segment, non-W REX `AD`, and other
prefixed `AC` forms remain unsupported. Native and JIT do not execute REP
LODS and do not fall back to the interpreter.

Each nonzero iteration reads one width from flat `DS:[RSI]`. Only successful
reads commit the width-selected accumulator alias, DF-directed wrapping RSI
and RCX decrement. Zero count does not read or validate RSI. Completion wins
over STOP; STOP wins over the 1024 successful-read work limit. Partial
STOP/limit leaves RIP on REP and retires zero; read failure preserves prior
completed elements and commits no current-element state. A replay resumes from
that exact RSI/RCX without rereading prior elements. Flags, DF, RDI and RAM do
not change. Missing mapping remains a harness error, no #PF is synthesized,
and no MMIO rollback claim is made. Definedness is checked within a run only;
`run()` reinitializes it on a new invocation.

Added a 12,288-state full completion matrix (4 widths × 2 DF × 6 data seeds ×
4 counts × 64 incoming flag combinations), 96 injected read errors, 16
STOP/replay cases plus initial STOP, 24 limit cases, `RCX=u64::MAX`, zero-count
and zero-budget checks, alignment/page/short-range/wrap/noncanonical-next-pointer
coverage, TEST→REP completion/partial definedness, 48 prefix rejects, and
non-REP LODS/REP STOS regressions. Exactly 36 literal schema-v2 fixtures were
added: eight per width, two normalization cases and two TEST→REP streams. The
generator serializes constants only; no profile/schema fields were added.
ISSUE-016 and ISSUE-020 remain open.

Final local validation in the Stage2 managed worktree passed: `cargo fmt --all
-- --check`, `cargo check --workspace --all-targets --offline`, complete offline
suites for `od-isa` (749 tests), `od-testkit` (218), `od-runner` (101 unit and
208 fixture tests), and `od-x86-runner` (9 host-available tests), plus
`python3 tests/cpu/generate_stage2_rep_lods.py --check`. Focused assertions
cover full snapshots and exact read sequences. The host-available x86-runner
tests omit its Linux-only native admission module. No Mac mini, SSH, native
runtime or physical x86 checks were run; all batch evidence remains in the
final substage.

Independent review initially found one `48 F3 AC` decoder-admission leak and
missing full-state/fault-metadata assertions in STOP, limit, and literal cases.
The decoder now requires raw exact two-byte `F3 AC`; the 48 prescribed prefix
rejects and the discarded-REX `48 F3 AC` case are checked without data access
or state changes. STOP/replay, work-limit and `RCX=u64::MAX` cases now compare
full state and retirement; page crossing, short range, wrapped RSI and
noncanonical next-pointer cases are covered. Fixture tests check expected read
fault address/type and architectural exception fields. The independent
re-review found no remaining concrete defects. No native/physical testing was
performed.

## S2-19d — bounded `F3 A4/A5` REP MOVSB/W/D/Q (local)

Added `Operation::RepMovs { size }` for exact Long64 forms `F3 A4` byte,
`F3 66 A5` word, `F3 A5` dword and effective REX.W A5 qword. The decoder accepts
all REX.W bytes 48..4F and the specified 66/REX orderings. Segment/address-size
overrides, non-W REX A5, F3 66 A4, conflicting/repeated group prefixes and
other unsupported prefixed A4 forms remain admission failures. Existing
non-REP MOVS, REP STOS and REP LODS remain covered. Native/JIT admission is not
expanded.

The executor reads one width from DS:[RSI], then writes one width to ES:[RDI].
It commits both DF-directed wrapping pointer steps and RCX decrement only
after both accesses succeed. A failed read skips the write; a failed write
follows the successful read without committing the current destination or
register progress. Previously completed transfers persist; replay rereads the
current source. Completion takes priority over STOP and the 1024-success local
limit, and STOP takes priority over that limit. A full REP retires once;
partial fault/STOP/limit keeps the original RIP and retires zero. Zero count
performs no data access even for invalid pointers. This remains a local policy:
no synthetic #PF, MMIO rollback or SMP atomicity claim is made. Flag
definedness is preserved within one run only because a new run reinitializes
the metadata.

Local verification added 12,288 full-state completions with exact alternating
read/write access logs; 192 injected read/write failures across architectural,
unmapped, permission and backend errors, including RAM no-commit checks; 16
STOP/replay states; 24 count/limit boundaries; 36 overlap transfer cases;
prefix admission/rejection checks; and bounded u64::MAX count progress. It
also checks zero count with two invalid pointers, page-edge success and
crossing failures for each operand/width, pointer wrap, and truncated fetch.
Fifty-six schema-v2 fixtures cover each width's zero, direction, fault,
noncanonical, budget and limit cases, overlap, normalization and TEST
composition. Expected overlap snapshots and pointer progress are handwritten
literal tables; fixture generation only serializes those values. The profile
and schema are unchanged. ISSUE-021 records that source-first ordering is not
established as physical dual-invalid exception priority; ISSUE-016 and ISSUE-020
remain open.

Focused local checks passed: `cargo test -p od-isa --offline rep_movs`,
`cargo test -p od-testkit --offline rep_movs`,
`cargo test -p od-runner --test fixture_v2 --offline rep_movs_cli_fixtures_check_full_state_and_jit_denial`, and all four REP fixture generators passed `--check`. Final validation passed `cargo fmt --all -- --check`, `cargo check --workspace --all-targets --offline`, and `cargo test --workspace --offline` (all workspace suites, including 750 od-isa, 225 od-testkit, 101 runner unit tests and 209 runner fixture tests). The 56 CLI fixtures compare full state and memory even on harness errors, check fault details and retirement, and deny JIT execution for every new fixture. Independent review found three test-evidence issues; all were fixed and re-reviewed with no remaining concrete findings. No Mac mini, SSH, native or physical tests were run.

## S2-19e — exact `F3 AE` REPE SCASB (bounded local profile)

The interpreter now admits only exact `F3 AE` in Long64/address64. Before
data access, even for zero count, it requires arithmetic flag values
`PF|ZF` (`0x44`) and all six arithmetic flags defined (`0x8d5`); DF=0/1 is
allowed. This is a local entry/restart restriction, not an x86 architectural
requirement. Zero count avoids RDI access and preserves flags. For nonzero
count, each read is followed by the existing byte CMP flag calculation, a
wrapping DF-directed RDI update, and RCX decrement. Completion and mismatch
precede STOP and the 1024-success limit; a continuing match checks STOP before
the limit. Faulting iteration state is not committed; prior equal progress
stays in RDI/RCX/flags with RIP at the REP instruction, permitting replay
without hidden state.

The exact reject set, operation semantics and definedness metadata are covered
by 131,072 single-comparison cases against an independent CMP model, 160
termination paths, 16,380 admission rejects, 24 injected read failures,
STOP/replay, six count/limit boundaries, mismatch at iteration 1024, limit/STOP
priority, zero count, u64::MAX bounded progress, budget-zero, truncated fetch,
page-edge, wrap and next-pointer boundaries. Twenty-four schema-v2 fixtures
and complete literal expected snapshots cover the requested composition,
failure and direction cases. JIT decode and native admission remain closed.

Verification passed: all eight focused `od-isa` REP SCASB tests, the 24-fixture
literal full-state comparison, all five `od-jit` tests, `cargo fmt --all --
--check`, `cargo check --workspace --all-targets --offline`, the full
`cargo test --workspace --offline` suite (758 od-isa, 226 od-testkit, 101
runner unit, 209 runner fixture and the remaining workspace tests), and all 77
Stage 2 generator freshness tests. Graphify was updated. Native admission has
an explicit reject branch, but its Linux-only target is not compiled on this
macOS host. No native, physical, Mini, SSH or JIT execution was performed.
Independent gpt-6.1-sol review found three evidence gaps: mismatch-vs-STOP
observation at iteration 1024, actual #GP linear-address comparison, and a
TEST fixture whose AF value also failed the concrete value gate. All were
corrected; re-review found no remaining concrete findings. The eight focused
ISA tests, 24-fixture test, five JIT denial tests, two new generator tests, and
the final full workspace checks passed. ISSUE-016/019/020/021 remain open.

## S2-19f — exact `F3 A6` REPE CMPSB (bounded local profile)

The Long64/address64 interpreter now admits only exact `F3 A6`. Before any
data read, including at RCX zero, entry requires arithmetic values `PF|ZF`
(`0x44`) and full arithmetic definedness (`0x8d5`); DF is accepted either way.
This is a local entry/restart restriction, not an architectural REPE CMPS
requirement. With zero count it avoids RSI/RDI validation and data access,
preserves flags, advances RIP by two, and retires once.

Each nonzero iteration reads source DS:[RSI], then destination ES:[RDI], and
uses the existing CMP calculation for source minus destination. Only after both
reads succeed does it commit flags, wrapping RSI/RDI by DF and RCX decrement.
Completion/mismatch precedes STOP and the 1024-success limit; STOP precedes
the limit. A first-read failure skips the second access. A second-read failure
leaves its first read visible but commits none of the current iteration. Prior
equal iterations persist and retry rereads both values; no pending source is
stored. This source-first order is local policy only, recorded in ISSUE-020;
MMIO rollback is not claimed. Mappings/backend errors remain harness errors.

The independent Rust matrix covers 131,072 source×destination×DF results,
160 count/mismatch termination paths, 16,380 value/definedness/count/DF
rejections, 48 injected read failures, four STOP/replay cases, six limit cases,
mismatch at iteration 1024, STOP/limit priority, RCX max, aliases/overlap,
zero count with invalid pointers, budget zero, page edge, wrap, noncanonical
first operands and truncated fetch. Exactly 31 schema-v2 fixtures carry
handwritten complete literal answers; the generator serializes them without
computing CPU results. Fixture tests compare complete interpreter snapshots and
require JIT `Unsupported` without fallback. Native admission explicitly rejects
REP comparisons. No native, Mac mini, SSH, or physical check was run.

Focused local verification passed: the six `od-isa` REP CMPSB tests, the
31-case testkit full-state fixture comparison, the CLI full-state/JIT-denial
comparison, five `od-jit` tests, both generator tests, and the 96-test Python
CPU harness/generator suite. `cargo fmt --all -- --check`,
`cargo check --workspace --all-targets --offline`, and
`cargo test --workspace --offline -- --quiet` passed: 1,322 Rust tests passed,
zero failed (764 od-isa, 227 od-testkit, 101 runner unit, 210 runner fixture,
and all remaining workspace tests). Graphify was updated after the source and
documentation changes. The Linux native path was not run; no native, Mini, SSH
or physical execution was performed. See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-19f-bounded-f3-a6-repe-cmpsb-contract),
[plan](../plans/STAGE_2.md#s2-19f--exact-f3-a6-repe-cmpsb-ограниченный-локальный-контракт),
[interpreter coverage](../testing/INTERPRETER.md#repe-cmpsb--s2-19f-local-interpreter-coverage),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#bounded-repe-cmpsb-fixtures-s2-19f).
