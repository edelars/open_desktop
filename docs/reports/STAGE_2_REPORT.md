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
its adapter cannot observe x87 state. S2-30a below adds one exact FNINIT form;
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

The existing `s2-long-x87-state-v1` profile now admits exactly one NOP or one
bare FNINIT, with budget 1 and stop RIP immediately after the instruction. The
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
