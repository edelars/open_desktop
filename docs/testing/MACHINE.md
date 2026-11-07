# Методика проверки машины: текущий срез

S4-00a evidence capture 2026-11-05 сверяет binary hashes по
[S2-40d](../reports/S2_40D_ARTIFACT_DIGEST_RECHECK.md), фиксирует recovered
Ubuntu 7.0.0-38.38 source/config hashes, совпадающие Build IDs и равенство 38
общих allocated file-backed ELF sections. Это не аутентифицирует mirror download и не
связывает source/config с binary; полный список ограничений и команд находится
в [S4-00a report](../reports/STAGE_4_S4-00A_2026-11-05.md). S4-00/S4-01 и
полная source provenance остаются открытыми.

## Область

Эта методика покрывает S4-02—S4-10a и bounded S4-06b/S4-06c/S4-11b: валидируемую physical
map, production RAM owner, committed-write notification, синхронную MMIO/PIO
bus, диагностический CPU memory bridge, uncached translation probe, bounded
tracked RAM access и два CPU-connected paged MOV forms. Она не проверяет полную
CPU paging matrix, guest exception delivery, APIC, JIT, firmware или Linux.

## Локальный набор

Команды выполняются из корня checkout:

```text
cargo fmt --all -- --check
cargo check -p od-machine --all-targets --offline
cargo test -p od-machine --offline -- --test-threads=1
cargo test -p od-isa --locked --offline page_fault
git diff --check
```

Тесты `od-machine` должны подтверждать:

1. checked range overflow, overlap и physical-width boundary;
2. width 64 с последним адресом `u64::MAX`;
3. общий RAM budget до allocation;
4. zeroed backing и little-endian scalar accesses;
5. hole/ROM/reserved rejection;
6. cross-region access без частичной записи и без callback;
7. ровно одно уведомление после успешной полной RAM-записи;
8. обычные guest-byte accesses, пересекающие детерминированную 16 KiB границу,
   без предположения о host page size.
9. MMIO widths 1/2/4/8, PIO widths 1/2/4 и отказ PIO Qword;
10. PIO endpoint `65536`, oversized range rejection и overlap/binding checks;
11. unknown/misaligned/read-only device accesses без device side effects;
12. отказ до dispatch для cross-window access без partial RAM/device effects.
13. execution regression через `od_isa::execute::run` и
    `IdentityMemoryAdapter`: fetch/load/store, unmapped fault, CPU state/RIP,
    retirement, RAM и committed callback;
14. explicit rejection atomic/fence operations в diagnostic flat mode.
15. four-level 4 KiB literal page tables: geometry, canonicality before reads,
    physical36/CR3, present/reserved/PS, cumulative permissions, WP/NXE,
    literal PFEC, trace, table-read errors and no A/D mutation;
16. PTE mutation and CR3 changes are observed on the next call, proving no TLB
    or translation cache in the probe.
17. `od-isa` PFEC helper matches literal nonpresent, write, reserved, NX-fetch
    and descriptor-table cases; record vector/error/address/RIP fields stay
    distinct and retain full `AccessContext`.
18. The `od-machine` orchestration converts only probe page faults; translated,
    noncanonical, unsupported-page-size and table-read results remain distinct.
    Page tables (including A/D bits) remain byte-for-byte unchanged.
19. Tracked RAM access commits missing A bits immediately during a valid walk,
    preserves those effects on later nonpresent/reserved/protection/table-read
    failures, commits D only for an allowed write after full RAM operand
    validation, and commits the operand last.
20. The tracked API accepts Read/Write widths 1/2/4/8 and Fetch remains
    byte-only; current success cases cover Read Dword, Write Byte/Word and
    Fetch Byte. Already-set A/D bits do not cause duplicate writes or callbacks,
    ROM/reserved/MMIO errors remain distinct, and cross-page/unsupported
    RAM-only accesses have no operand effect.
21. Present PS entries at PDPT/PD return the exact unsupported-page-size result
    without descendant or operand access; pure probes remain mutation-free, and
    tracked ancestor A updates are retained in order while the rejected entry
    remains unchanged.
22. Nonpresent+PS keeps nonpresent precedence; PML4 bit 7 remains a reserved
    bit, PT bit 7 remains admitted for 4 KiB, and reserved-address/NXE cases do
    not invent huge-page support or change the admission boundary.
23. Same-page Read/Write widths 1/2/4/8 cover the last fitting offsets and exact
    bytes; crossing requests return `CrossesPage` before any walk with empty
    trace/journal, zero A updates, no callback and unchanged RAM.
24. Crossing admission wins over noncanonical or inaccessible-table candidates
    and is independent of prospective RAM/RAM, RAM/hole and RAM/MMIO targets;
    it never splits a device or operand access.
25. A successful same-page call remains committed when a later crossing call is
    rejected. Adjacent caller-issued Fetch bytes use separate reports; a later
    fault does not erase the earlier byte or its A/D/callback effects.
26. `Long64ProbeConfig::from_cpu_state` requires CR0 before CR4, preserves raw
    CR0/CR4/CR3/EFER inputs through existing validation, and returns exact
    missing/invalid configuration errors without changing CPU state or memory.
27. Rebuilding snapshots after CR0/CR4 or explicit CR3 changes observes the new
    translation context while the old snapshot remains unchanged; explicit EFER
    inputs are preserved and validated; distinct roots produce distinct literal
    physical translations.
28. The experimental pure-translation cache passes preflight parity against the
    uncached probe, caches only successful 4 KiB walks, reports Hit versus Walk,
    and leaves pure-probe RAM unchanged; release performance evidence belongs
    to the separate [S4-13a report](../reports/STAGE_4_S4-13a_2026-11-05.md).
29. S4-14a verifies that `TranslationProbeCache::clear` is idempotent and does
    not write page-table RAM or A/D bits; a successful entry follows
    `Walk -> Hit -> clear -> Walk -> Hit` with literal translation metadata.
30. Failed replacement is not retained: the failed request returns `Walk`, the
    cache is empty, and the next matching request walks before becoming a hit.
31. Ending the cache scope permits a page-table remap and construction of a new
    cache, which observes the new physical page; recreated caches also observe
    leaf RW tighten/relax and NX Fetch rights with PFEC `0x07` and `0x15`.
32. Explicit page-table writes report their committed `(address, length)` range;
    cache probe itself produces no callbacks or page-table A/D mutation.
33. S4-21a runs real fetch/load/store bytes through `IdentityMemoryAdapter`,
    preserves CPU state and the `rflags_defined` mask across one budget versus
    multiple quanta, and returns `WorkLimit` without automatic continuation.
34. Pause/resume leaves CPU state unchanged; STOP is idempotent, wins before
    fetch and at an execution poll point, and prevents later `run` calls.
35. S4-21a keeps halted, architectural fault, unsupported instruction, backend
    memory error and stop-address outcomes distinct; it does not claim HLT
    wakeup, reset, IRQ, MMU or JIT lifecycle.
36. S4-22a checks zero/rounding/large virtual-clock conversions, atomic time/TSC
    overflow rejection and split-advance equivalence; time does not advance from
    execution, budgets, polling or host calls.
37. Bounded RDTSC uses the caller-driven clock and preserves EDX:EAX, RIP and
    `rflags_defined` across split budgets. TSD/CPL denial, fetch-boundary failure,
    zero budget and invalid defined masks remain explicit outcomes.
38. S4-23a verifies fixed-capacity one-shot deadlines: equal deadlines preserve
    insertion order, reschedule gets a new position, `deadline <= now` is due,
    and `pop_due` removes at most one event.
39. Duplicate/full/not-found operations, cancellation of due events, `u64`
    extremes and sequence rebasing leave the queue unchanged or ordered by the
    documented contract; VirtualClock readiness does not advance time.
40. S4-23b checks that `EventGeneration` starts at zero, every queue operation
    rejects a stale generation before lookup/capacity checks or mutation, and
    extracted events are validated before use. Successful reset clears pending
    events and sequence while retaining capacity; stale reset and generation
    exhaustion preserve entries, sequence, capacity and clock. Deadline/order
    semantics remain unchanged, and reset never advances `VirtualClock`.
41. S4-23c checks bounded UART RX payload capacity and dispatch ordering:
    generation, STOP, event budget, then readiness. Deadlines and equal-deadline
    order remain owned by `EventQueue`. Successful injection removes exactly one
    event/payload; `ReceiverBusy` preserves the earliest event, payload,
    deadline and order for retry. Reset invalidates stale pending work without
    changing `VirtualClock`, CPU state or HLT state.

## Интерпретация

Успешные локальные тесты подтверждают только перечисленный Rust-контракт,
flat identity integration, tracked RAM-only effects и targeted `od-isa`
PFEC/record tests, pure translation probe, S4-10a rejection boundary, S4-11a
cross-page admission and bounded S4-11b qword read, S4-12a snapshot bridge, S4-13a pure-cache experiment,
S4-14a cache lifecycle boundary, S4-21a diagnostic vCPU session и S4-22a
caller-driven clock/RDTSC boundary, S4-23a bounded deadline queue, S4-23b
reset-generation boundary and S4-23c bounded UART RX event delivery.
Они не являются physical x86 evidence и не
подтверждают, что guest 4 KiB
страницы уже имеют независимые paging permissions. Отдельные failure values не
являются автоматически архитектурными `#PF`.

После изменения кода Graphify обновляется командой `graphify update .`. При
переходе к MMU/CPU-срезам каждый тест должен дополнительно сохранять fault state,
CR2, A/D effects и source of expected result.

## S4-01a: diagnostic CPU-to-RAM baseline

The [S4-01a report](../reports/STAGE_4_S4-01A_2026-11-05.md) records a fresh
run of the exact CPU-through-RAM integration test and a separate immutable
page-walk snapshot control. Run both exact commands from that report and
require one executed test per command. The integration contract covers
`od_isa::execute::run → IdentityMemoryAdapter → MachineBus → PhysicalMemory`
in diagnostic flat identity mode (`linear == physical`); it is not a Linux
machine manifest. This baseline does not establish CPU-integrated paging,
`#PF` delivery/retry, JIT, privileged hardware behavior or Linux boot. The
captured host page size is metadata only; it does not parameterize guest test
memory.

## S4-06b: bounded CPU-connected paged MOV

Baseline revision: `b2a093c`. The reviewed code files are listed in the
[S4-06b report](../reports/STAGE_4_S4-06B_2026-11-05.md). From the repository
root, reproduce the package-level checks with:

```text
cargo test -p od-isa --locked --offline
cargo test -p od-machine --locked --offline
cargo check --workspace --all-targets --locked --offline
cargo fmt --all -- --check
python3 tools/check_layout.py
git diff --check
```

The independent `gpt-6.1-sol/high` review reran the two package test commands:
`od-isa` passed 2,014 unit tests and `od-machine` passed 85 tests. The author
reported 8 `PagedMov` CPU tests, 3 legacy CR0 tests, 2 legacy CR4 tests, and
the `paged_mov_step` integration target's 8 tests passing; the package-level
runs include these regressions. Workspace check, formatting, layout and diff
checks passed. The review approved the complete code diff with no P0–P3
findings.

Coverage includes both exact encodings, one-step retirement, CR0/CR4 admission,
`Explicit` origin with preserved CPL, `rflags_defined`, literal non-identity
translations, PFEC and saved RIP, A/D and callback ordering, aliasing,
unchanged read buffers on error, non-partial operand writes, and distinct
architectural/backend/unsupported outcomes.

This does not establish full paging, TLB/invalidation, CR2 commit or exception
delivery/retry, cross-page operands, REP/stack/atomic semantics, JIT, IRQ/devices,
SMP or Linux boot. ISSUE-054 remains open for cross-page/partial progress,
REP/RMW and broader rollback consumers.

## S4-06c: CPU-connected paged MOV to MMIO

The [S4-06c report](../reports/STAGE_4_S4-06C_2026-11-05.md) and manifest
record the focused device-dispatch and ordering evidence. The tests use the
existing `step_paged_mov` path with a literal guest mapping from the operand
GVA to a bound MMIO range. The device double records `DeviceAccess`, payload
and event order alongside committed-write callbacks.

The positive cases require a nonzero device read value to reach the MOV
destination, a write to carry the exact qword and device offset/width, exactly
one device operation, and A/D callbacks before MMIO dispatch. Negative cases
require permission failure to avoid the device call and device backend failure
to preserve CPU state and retirement count while retaining prior A/D effects.
The MMIO device contract requires failure before visible device-side effects.

Focused reproduction, check results, reviewer findings, and open boundaries
are recorded in the S4-06c report and JSON manifest. This does not test cross-page
device transactions, retries, device read/modify/write, exception delivery,
TLB/JIT, IRQs or the complete paging matrix.

## S4-11b: CPU-connected cross-page qword load

The [S4-11b report](../reports/STAGE_4_S4-11B_2026-11-05.md) and evidence
manifest record all seven offsets of an 8-byte operand across a 4 KiB boundary.
The fixture maps the two virtual pages to distinct, non-contiguous RAM pages.
Positive cases check the assembled little-endian qword, RAX/RIP, retirement,
preserved flags, no D-bit update and the ordered A-update journal.

Negative cases fault on the second page as not-present or supervisor-only,
verify the exact `PageFaultRecord`/PFEC/address/RIP/context, preserve the CPU
destination and caller buffer, and retain A updates committed by earlier walks.
RAM+hole and RAM+MMIO combinations must fail without device dispatch or a
partial operand value. A page-table alias case confirms both walks and A
updates precede operand sampling. Cross-page stores and device splitting remain
outside this method.

## S4-11c: CPU-connected cross-page dword read

`od_machine::step_paged_mov_dword_read` admits a separate exact Long64 `8B 03`
(`MOV EAX,[RBX]`) profile. Reproduce with
`cargo test -p od-machine --test paged_mov_step --locked --offline s4_11c`.
The same-page RAM case is the control; crossing offsets `0xffd`, `0xffe` and
`0xfff` use two non-contiguous RAM mappings. Tests check zero-extension, RIP+2,
single retirement, unchanged qword profile neighbors, alias sampling after
both A-bit updates, no D update, exact second-page #PF data and refusal of
cross-page MMIO dispatch. This is a synthetic installed-Long64 profile, not a
full guest or hardware acceptance.

## S4-11d: CPU-connected cross-page instruction fetch

Reproduce with `cargo test -p od-machine --test paged_mov_step --locked --offline s4_11d`.
The exact `8B 03` profile places the opcode at linear page offset `0xfff`, the
ModRM byte on a second non-contiguous RAM code page and the dword operand on a
separate RAM page. Success checks the normal single retirement. Second-fetch
nonpresent and NX faults check PFEC `0x10`/`0x11`, the faulting linear byte,
saved RIP, full CPU rollback, no retirement and no operand-page walk. A hole or
MMIO second code page must fail without device callback. A decoder unit control
places one-byte HLT at the last byte of a page and verifies no next-page fetch;
HLT is not admitted by the CPU profile. This does not establish general
cross-page instruction-fetch support.

## S4-11e: CPU-connected cross-page word read

Reproduce with `cargo test -p od-machine --test paged_mov_step --locked --offline s4_11e`
and `cargo test -p od-machine --lib --locked --offline failed_second_cross_page_walk_keeps_caller_buffer_unchanged`.
The exact `66 8B 03` (`MOV AX,[RBX]`) profile checks same-page RAM and an operand
starting at offset `0xfff` across two non-contiguous RAM pages. Success preserves
RAX[63:16], updates AX, advances RIP by 3 and retires once. CPL0 non-present and
CPL3 supervisor-only second-page faults verify exact PFEC, fault byte, saved RIP,
no CPU/buffer commit or retirement, prior A updates and no D update. RAM holes,
same-page/cross-page MMIO, alias walk-then-sample and profile exclusions are
covered. CPL0 and CPL3 require AC=0. This is a synthetic bounded profile, not
general cross-page memory or hardware acceptance.

## S4-11f: CPU-connected cross-page word instruction fetch

Reproduce with `cargo test -p od-machine --test paged_mov_step --locked --offline s4_11f`.
The exact `66 8B 03` profile executes only at CPL0 with AC clear and NXE enabled.
Starts at offsets `0xffe` and `0xfff` fetch from non-contiguous RAM code pages;
the operand remains within a separate RAM page. Offset `0xffd` is the same-page
control with the next page absent. Check exact second-page non-present/NX fetch
faults (`0x10`/`0x11`), original RIP and linear fault byte, saved CPU state, A
updates, no D, and no operand walk or retirement. A missing or MMIO code page
must not dispatch a device. A case with both a fetch fault and unavailable
operand verifies fetch priority. The existing S4-11e profile remains restricted
to same-page instruction fetch. Reject other encodings, CPL3 and AC=1 through
both the specialized and generic profile entries. Reject a word operand at
data offset `0xfff` after decode and before any operand walk; reject CR0-only
profile entry through both run and step APIs before fetch. This is not general
cross-page fetch support.

## S4-24a: bounded COM1 polling TX

Baseline revision: `6aeb2dd` (the reviewed UART changes were present in the
shared worktree after this baseline). The [S4-24a report](../reports/STAGE_4_S4-24A_2026-11-05.md)
records the bounded profile and review result. Reproduce the focused checks
from the repository root:

```text
cargo test -p od-machine --lib uart16550::tests --locked --offline
cargo test -p od-machine --test uart16550_polling --locked --offline
cargo test -p od-machine --lib bus::tests --locked --offline
cargo check -p od-machine --all-targets --locked --offline
cargo fmt --all -- --check
python3 tools/check_layout.py
git diff --check
```

Recorded results: 7 UART unit tests, 2 UART PIO integration tests and 4
existing `MachineBus` tests passed; the package all-targets check, formatting,
layout and diff checks passed. Independent `gpt-6.1-sol/high` review approved
the code diff with no P0–P3 findings.

The integration path binds the device to COM1 and checks byte PIO through the
production bus, DLAB divisor selection without accidental TX, the literal
`THRE/TEMT` transitions, raw byte completion, busy/unsupported access atomicity,
and reset through a handle that aliases the bus-owned state. This verifies only
the synchronous bounded TX endpoint. Baud timing, FIFO, IRQ4, Linux serial
compatibility, guest boot and scheduler behavior are not covered. Parent
S4-24 remains open.

## S4-24b: bounded CPU PIO and COM1 RX

The [S4-24b report](../reports/STAGE_4_S4-24B_2026-11-05.md) records the
accepted CPU-to-PIO slice and independent review. The bounded profile is
`DiagnosticCpl0PioU8V1`: only bare `EC`/`EE` in Long64 CPL0, with CR0/CR4
`None`; fetch uses flat identity RAM. Each successful call performs one byte
PIO callback and one retirement. `IN AL,DX` preserves upper RAX and consumes
the single injected UART RX byte; `OUT DX,AL` uses the existing bounded TX
holding/shift path. Failed fetch or PIO operations do not retire or mutate CPU
state.

Recorded verification from the implementation and independent review:

```text
cargo test -p od-isa --locked --offline                  # 2014 unit + 7 integration passed
cargo test -p od-machine --locked --offline              # 98 passed
cargo check -p od-isa --all-targets --locked --offline   # passed
cargo check --workspace --all-targets --locked --offline # passed
cargo fmt --all -- --check                              # passed
git diff --check                                         # passed
```

The targeted PIO integration tests cover successful byte IN/OUT, exactly one
callback and retirement, upper-RAX preservation, RBR read-clear, `LSR.DR`,
unmapped-port errors, fetch errors, unsupported instructions, CPL rejection
and unchanged CPU state on failures. UART RX accepts one injected byte,
reports data ready while it is pending, rejects a second injection until the
guest reads the first, and returns `DeviceError::WriteOnly` when the empty RBR
is read. The full `od-machine` suite and `od-isa` package tests were reported
successful; the independent `gpt-6.1-sol/high` review approved the complete
code slice with no P0–P3 findings.

These results do not establish IOPL/TSS checks, other I/O widths, immediate or
REP/string forms, IRQ delivery, APIC, Linux serial-driver compatibility or
Linux boot. Parent S4-24 remains open.

## S4-24c: bounded paged CPU PIO

The [S4-24c report](../reports/STAGE_4_S4-24C_2026-11-05.md) records the
accepted paged CPU-to-PIO slice and independent review. `step_paged_pio_u8`
uses profile `PagedCpl0PioU8V1`: bare `EC`/`EE`, Long64 CPL0, with CR3/EFER
validated by the existing `Long64ProbeConfig` four-level 4 KiB configuration.
Fetch follows guest page tables with `AccessOrigin::Explicit`. Literal fault
coverage expects PFEC `0x10` for not-present and `0x11` for NX protection.

The CPU entry accepts an already-selected `GuestMemory` and performs only
CPU-side CR0/CR4 and profile admission. The machine entry validates CR3/EFER
through the existing `Long64ProbeConfig` and supplies the real page-aware
memory adapter. The machine acceptance executes `MOV RAX,[RBX]` then `OUT DX,AL` with no host
mutation between steps and passes the first result's definedness into the
second. UART output is checked against the guest-loaded byte. On a PIO failure,
CPU/definedness are unchanged while already committed A/D writes and callbacks
remain. Full tests passed: `od-isa --all-targets` — 2,014 unit, 6 paged and 7
legacy tests; `od-machine --all-targets` — 105 tests, including 7 paged.

Recorded additional checks: package checks, formatting, diff check and layout
check passed; workspace check, JSON/link validation and independent
`gpt-6.1-sol/high` review also passed with no P0–P3 findings. These checks do not establish a mixed-instruction runner,
exception delivery/CR2 update, IOPL/TSS, IRQ/APIC, scheduler, baud timing, JIT
or Linux behavior. Parent S4-24 remains open.

## S4-32a: reproducible flat probe

The [S4-32a report](../reports/STAGE_4_S4-32A_2026-11-05.md) records the
artifact-to-runner acceptance path. The freestanding fixture is assembled and
linked into ELF64, then converted to a fixed raw image by the active Rust
toolchain's `ld.lld` and `rust-objcopy`. The build validator checks the entry,
three PT_LOAD segments, exact page-table entries, code bytes and zero padding,
payload bytes, raw size and hashes. Two clean builds with payload
`0x1122334455667788` produced the same raw SHA-256
`e78a7af7306e31fa316f1d64332ea4bd64abf0e854883106b1933f68bef97204`.

`od_machine::load_stage4_flat_probe` accepts only the exact image layout,
copies it verbatim to GPA `0x1000`, installs the fixed Long64/CPL0 handoff and
binds the bounded COM1 polling UART. It rejects short/oversized images, aliases,
NX code and unexpected present page-table entries. `od-runner --stage4-probe`
passes the loaded image to the existing `run_paged_probe` quantum; it does not
inject an expected byte or dispatch the guest instructions itself. A real CLI
run retired `MOV`, `OUT` and `HLT`, reported the payload's low byte on serial,
and a budget of `2` returned a nonzero `budget_exhausted` result after two
retirements with the partial UART byte visible.

Focused verification passed: loader integration **4/4**, runner CLI tests
**5/5**, runner package check, formatting and diff check. The full runner
all-targets suite was started but not completed in this slice, so no aggregate
pass count is claimed. This remains a bounded interpreter probe: no Linux boot
protocol, general kernel loader, exception delivery, IRQ/APIC, scheduler,
physical hardware or full S4-32 acceptance is established.

## S4-32b: freestanding store/load probe

The [S4-32b report](../reports/STAGE_4_S4-32B_2026-11-05.md) records a second
real-artifact path after S4-32a. The builder's explicit `store-load-probe`
profile produces the eight-byte guest sequence
`MOV [RBX],RCX; MOV RAX,[RBX]; OUT DX,AL; HLT` and stores distinct initial and
store-value qwords in the image. `od-machine` reads the artifact-owned
store-value into the documented `RCX` handoff, while the runner only invokes
the existing `run_paged_probe` quantum.

The permanent `stage4_store_load_build_cli` integration test builds two
identical inputs and one different input, compares same-input raw bytes,
executes both real `flat_probe.bin` outputs through
`--stage4-store-load-probe`, and checks the full data/RAX/UART/HLT result. It
also checks budget 1/2/3 partial state, a corrupted code artifact and an
equal-data artifact rejected by the loader. The focused machine test checks
the same handoff and committed store independently, including the exact page
table A/D and target-data committed-write callback sequence. The builder's
negative test rejects an ELF/raw pair with a substituted opcode, and the CLI
rejects repeated budget options.

This slice remains bounded interpreter evidence. It does not establish stack
execution, exception delivery/repair, alias or cross-page behavior, IRQ/APIC,
HLT wakeup, JIT integration, Linux boot or full S4-32/S4-33 acceptance.

## S4-21b: bounded paged probe quantum

The [S4-21b report](../reports/STAGE_4_S4-21B_2026-11-05.md) and
[machine contract](../specs/MACHINE_CONTRACT.md#bounded-paged-probe-quantum-s4-21b)
record the accepted `PagedCpl0ProbeV1` boundary. `step_paged_probe` accepts
only `48 8B 03` (`MOV RAX,[RBX]`), `48 89 0B` (`MOV [RBX],RCX`), bare `EC`
(`IN AL,DX`), bare `EE` (`OUT DX,AL`) and bare `F4` (`HLT`). CPU admission
checks CR0/CR4; `run_paged_probe` validates CR3/EFER and uses page-aware memory.

The quantum owns a finite instruction budget and decodes once at each boundary.
STOP wins before budget exhaustion; an exhausted budget wins before fetch.
The store→load→OUT→HLT probe is checked both in one quantum and across
single-instruction quanta. STOP/resume checks that committed store/PIO effects
are not repeated. HLT retires and enters the halted state; a repeated call is a
no-op with no fetch. It is not interpreted as guest success.

Recorded verification: `od-isa` has 2,014 unit plus 25 integration tests;
`od-machine` has 116 passing tests. Workspace all-targets check, formatting,
layout and diff passed. `graphify update .` refreshed the code graph; the
docs-aware semantic extraction was not completed. The tests cover page-aware instruction
fetch/data, literal PFEC and fault records, saved RIP/access context, prior
retirements, tracked A/D bits and callback ranges, plus distinct backend and
PIO errors. No exception delivery or CR2 update is claimed. The evidence is
local Rust testing; it does not establish kernel/Linux boot, IRQ/APIC, or full
S4-21/S4-24/S4-32 acceptance. Stage 4 remains open.

## S4-32c: bounded freestanding stack probe

The `stack-probe` builder profile assembles a real ELF64 fixture and converts it
to the exact `0x9000`-byte raw image used by the S4-32b loader. Its code is
`51 58 EE F4`; the payload qword is artifact-owned at raw offset `0x8000`.
`od-machine::load_stage4_stack_probe` validates the same fixed page-table
layout, code bytes and image size, then hands the payload to `RCX`. The runner
invokes the opt-in `run_paged_stack_probe` path and reports stack qword, `RAX`,
`RSP`, retirement count, UART bytes and exit/activity as JSON.

The focused machine test checks budgets 0…4, split PUSH/POP/OUT/HLT calls,
stack canaries, qword handoff, `RAX/RSP/RIP`, `rflags_defined`, UART output and
an inert repeated call after HLT. The builder-to-runner test builds two equal
payloads and one different payload, verifies reproducibility, runs every budget,
and rejects corrupted code and page-table artifacts. Direct ISA regression
also verifies that the legacy `PagedCpl0ProbeV1` still rejects `PUSH RCX` and
`POP RAX`. Stack-fault tests cover nonpresent PUSH/POP and read-only PUSH
PFEC/CPU-commit boundaries. This is bounded local
Long64/CPL0 interpreter evidence only: stack exception delivery/repair, CR2,
cross-page transactions, IRQ/APIC, HLT wakeup, JIT, Linux, privileged hardware
and parent S4-32/S4-33 remain unclaimed until separate evidence exists.

## S4-33a: guest RAM alias

The strict `alias-probe` builder profile places a distinct initial qword and
store value in its image and maps data GVA `0x800000` plus stack GVA `0xa00000`
to GPA `0x9000`. Machine tests check that budget zero preserves the initial
value, guest store changes the shared physical qword, POP reads the new value
through the stack alias, each leaf PTE receives the expected A/D bits, and
committed callback ranges occur in exact order. Builder/runner tests compare
multiple payloads and reject an injected remapping that still reaches HLT but
does not satisfy the alias-result oracle. The manifest's `mappings` and `stack`
fields must agree. This is uncached interpreter evidence; TLB/JIT, Linux and
physical hardware claims are outside the case.

## S4-17a: paged MOV differential harness

Run the cross-component harness with:

```text
cargo test -p od-host --test stage4_memory_fault --offline -- --test-threads=1
cargo test -p od-testkit --lib --offline memory_fault::tests -- --test-threads=1
```

The integration target runs on Apple Silicon macOS, the platform supported by
`PublishedCode` executable-memory publication. On other hosts this target is
compiled out; the testkit comparator unit tests remain portable.

The deterministic Long64/CPL0 case is exact `48 8B 03` (`MOV RAX,[RBX]`),
with code linear address `0x00401000` and operand linear address
`0x8000000120` mapping to a separate physical RAM page. The harness creates an
independent `MachineBus` from the same case descriptor for the uncached
interpreter and published JIT. It checks both a successful qword load and a
non-present leaf PTE fault. Code-side page-table A bits are initialized before
execution so interpreter instruction fetch does not add effects absent from
the published JIT path; data-side A updates remain observable in physical RAM
deltas and ordered callbacks. The test verifies that each changed entry differs
by exactly the Accessed bit, and that each backend's complete #PF exit agrees
with its `PageFaultRecord`.

`od_testkit::memory_fault::MemoryFaultObservation` records the case digest,
all GPRs, RIP, RFLAGS plus its definedness mask, retirement, outcome, intended
fault CR2 value, actual architectural CR2 availability, table-byte deltas,
ordered physical write ranges, and device-event availability. This bounded
RAM-only case declares an observed empty device-event list. The page-fault
record supplies a CR2 value but these step APIs do not commit architectural
CR2, so that state is recorded unavailable. A post-execution injected
missing-A-bit result must compare unequal at its physical address and bit.
Cached interpreter execution is explicitly unsupported: the current cached
translation probe is not a cached instruction interpreter and does not carry
tracked A/D effects. This compares two local paths that share the page walker;
it is not independent hardware evidence. Reducer coverage and broader
S4-17 cases remain open. See the [S4-17a report](../reports/STAGE_4_S4-17A_2026-11-06.md).

## S4-18c: target descriptor Accessed update

Reproduce the bounded delivery checks with:

```text
cargo test -p od-isa --lib interrupt::tests:: --offline -- --test-threads=1
cargo test -p od-machine --lib paged_exception_delivery::tests --offline -- --test-threads=1
cargo test -p od-host --test stage4_exception_delivery --offline -- --test-threads=1
```

The system target covers guest `LIDT → LGDT → MOV [RBX],RCX → #PF` delivery
through the guest-loaded tables with target descriptor A=0, exact six-qword
frame, tracked GDT PTE Accessed/Dirty transitions, a one-byte descriptor update
and handler load. Negative controls cover an A=1 descriptor on a read-only GDT,
an invalid A=0 target, and a read-only A=0 update fault with its secondary PFEC,
CR2, retained frame and no CPU entry commit. A distinct-linear-address
GDT/stack physical alias is rejected before frame stores. Additional cases
make the saved RIP overwrite the GDT leaf PTE with a read-only mapping and a
redirect to another RAM page; preflight preserves the PTE and redirected
target, with no frame write committed. Another case makes an earlier frame
slot rewrite a stack PTE used by a later slot, which is rejected before any
frame write. Missing-page and physical-hole cases confirm the first stack
failure keeps its original fault/backend classification even when an
unreachable later slot aliases the descriptor. The emulator's chosen store order is not claimed as
universal hardware fault ordering; a secondary #PF would require #DF, which
remains unsupported here. This does not demonstrate recovery, restartability,
physical-hardware equivalence, or the rest of Stage 4. Results are recorded in
the [S4-18c report](../reports/STAGE_4_S4-18C_2026-11-06.md).

<a id="s4-19c-secondary-pf-double-fault-delivery-and-shutdown"></a>

## S4-19c: secondary #PF, double-fault delivery, and shutdown

Run the bounded delivery checks with:

```text
cargo test -p od-isa --lib interrupt::tests:: --offline -- --test-threads=1
cargo test -p od-machine --lib paged_exception_delivery::tests --offline -- --test-threads=1
cargo test -p od-host --test stage4_exception_delivery --offline -- --test-threads=1
```

The positive system case makes the target #PF code descriptor's Accessed-bit
write fault on a read-only GDT. It checks the original and secondary page-fault
records, exactly one vector-8 delivery, zero #DF error code, no forced RF in the
#DF frame, preserved CR2, all six frame slots, and execution of the first
handler MOV. The shutdown case faults while writing the #DF frame to an absent
stack page; it checks the newest CR2, retained frame-prefix writes, shutdown
activity, no handler step, and a subsequent paged step with zero fetches,
accesses, A/D updates, or writes. Existing IDT/GDT/stack failure cases also
check their final attempt outcomes. The focused ISA group covers #DF frame
construction and failure ordering.

This is local emulator evidence for #PF → #DF and #DF-delivery #PF → shutdown
inside the bounded profile. It does not establish other exception-pair
precedence, hardware ordering, handler repair, INVLPG, IRETQ, or retry. Results
are recorded in the [S4-19c report](../reports/STAGE_4_S4-19C_2026-11-06.md).

<a id="s4-19d-guest-pte-repair-and-invlpg"></a>

## S4-19d: guest PTE repair and INVLPG

The system scenario raises a real write-protection #PF, delivers it through
the IDTR/GDTR values set in its initial fixture state, then executes guest
`MOV [RDI],RSI` to change the selected writable alias PTE-page mapping's leaf
data PTE from read-only to read-write while retaining A and supervisor-only
U/S=0. It next executes guest `INVLPG [RBX]` for the
original faulting address. Checks cover unchanged faulting data, unchanged
RBX/RCX, CR2, retired instruction count, exact PTE value, invalidation address,
and tracked A/D and callback order. No host PTE patch occurs after the initial
fixture setup.

This stops before IRETQ and retry. Since the tracked paged path does not keep a
translation cache, the scenario demonstrates guest invalidation execution but
not stale-TLB eviction or that the invalidation is needed. Results are recorded
in the [S4-19d report](../reports/STAGE_4_S4-19D_2026-11-06.md).

<a id="s4-19e1-guest-error-code-discard-and-paged-iretq"></a>

## S4-19e1: guest error-code discard and paged IRETQ

The system scenario extends the actual #PF → guest PTE repair → INVLPG path.
Guest `LEA RSP,[RSP+8]` removes the error-code slot and guest `IRETQ` reads the
five-qword frame plus real CS/SS descriptors through the tracked Long64 walker.
It checks return to the original faulting RIP, exact restoration of the
unaligned RSP and RF=1, unchanged CR2/control registers/GPR and unchanged data.
The fixture provides a real pre-accessed writable CPL0 SS descriptor. No host
instruction patches the frame or return state.

Read page faults update CR2 and retain already completed tracked A/D effects;
backend failures remain separate. Paged IRETQ validates CS before checking
saved SS selector/bounds and does not commit partial CPU state. This stops at
the original MOV before retry. The tracked path has no translation cache.
Results are recorded in the
[S4-19e1 report](../reports/STAGE_4_S4-19E1_2026-11-06.md).

<a id="s4-19e2-faulting-mov-retry-and-data-commit"></a>

## S4-19e2: faulting MOV retry and data commit

After the guest return in S4-19e1, the exact original `MOV [RBX],RCX` is
executed again from its faulting RIP. The test checks one retirement, one
qword data write, the preceding tracked D-bit update (`0xe023 → 0xe063`), RF=0
after the retried instruction, and a following guest `MOV RAX,[RBX]` readback.
The adjacent qword canary is unchanged before and after the readback. No host
instruction patches RIP, RSP or PTE after the guest fault begins.

RF-clear tests cover successful MOV, operand #GP, data #PF/backend error and
pre-start exits. `rflags_defined` remains unchanged. This is the bounded
Long64 CPL0/WP=1 private-RAM profile; the tracked path has no translation
cache, so stale-TLB behavior is not claimed. See the
[S4-19e2 report](../reports/STAGE_4_S4-19E2_2026-11-06.md).

<a id="s4-20a-synthetic-paged-iretq-outer-return"></a>

## S4-20a: synthetic paged IRETQ outer return to CPL3

This CPU-only profile checks the outer-privilege return path for exact paged
`48 CF`. The initial state is the synthetic four-level Long64 profile with
`CR4.LA57=0`, CPUID leaf 7 zero, a CPL0 CS cache and null DS/ES/FS/GS selectors.
The guest frame contains RIP, CS, RFLAGS, RSP and SS. User CS/SS descriptors
are the bounded flat, already-Accessed GDT forms recorded in the
[machine contract](../specs/MACHINE_CONTRACT.md#synthetic-paged-iretq-outer-return-to-cpl3-s4-20a).

Unit tests verify exact CPU state after success, atomic rejection of malformed
selectors/descriptors/addresses/flags, separate guest paging faults and backend
read errors, and first-fault ordering across the frame and CS/SS descriptor
reads. Run:

```sh
cargo test -p od-isa --lib iretq::tests --offline -- --test-threads=1
```

This slice does not execute the next user instruction or prove machine
`AccessContext` derives CPL from CS; that integration is S4-20b. It does not
establish Linux boot or implement #GP/#SS classification for rejected return
state. See the [S4-20a report](../reports/STAGE_4_S4-20A_2026-11-06.md).

<a id="s4-20b-machine-iretq-and-cpl3-load"></a>

## S4-20b: machine IRETQ followed by one CPL3 load

The machine wrapper returns from CPL0 to CPL3 and executes exactly
`48 8B 03` (`MOV RAX,[RBX]`) using CR3 from CPU state and CPL derived from the
committed CS selector/cache. It does not accept caller-supplied privilege or
CR3 values. The bounded instruction fetch must fit in one guest page; offset
`0xffd` is covered, while cross-page instruction fetch is outside this slice.

System tests cover successful user fetch/data read, supervisor-only fetch and
data mappings at every page-table level, PFEC `0x15`/`0x05`, exact CR2 and saved
RIP, Accessed updates, no Dirty or operand write, a physical-hole backend abort,
and a late SS-descriptor page fault before IRETQ state commit. A second
instruction fault preserves the already completed CPL3 transition; an IRETQ
fault does not run the load. ISA profile tests reject nearby encodings without
operand reads or retirement; instruction fetch may already have occurred
during bounded decoding. The MOV clears RF before operand access, so that
defined flags effect remains if the later data access aborts.

Run the focused admission test and the complete machine system target:

```sh
cargo test -p od-isa --lib cpl3_load_ --offline -- --test-threads=1
cargo test -p od-host --test stage4_exception_delivery --offline -- --test-threads=1
```

The profile does not support cross-page instruction fetch, user stores,
exceptions delivered from CPL3, syscalls, or Linux boot. See the
[S4-20b report](../reports/STAGE_4_S4-20B_2026-11-06.md) and
[machine contract](../specs/MACHINE_CONTRACT.md#machine-iretq-followed-by-one-cpl3-load-s4-20b).

## S4-20c: paged SYSCALL → kernel store → SYSRETQ → user load

The four-step machine test exercises the exact instruction bytes through
tracked paging, with a supervisor RW mapping and a user RO alias to one data
qword. It verifies the nonce written by the kernel in CPL0 is observed by the
final CPL3 load, the CPL transitions and selector caches, RCX/R11/FMASK and
RFLAGS definedness, and unchanged RSP/CR3. STAR selector derivation, SYSRETQ's
`0x3c7fd7` flags mask, reserved-bit handling and definedness are covered by ISA
tests.

Failure tests cover SCE-disabled #UD, invalid SYSRETQ CPL/canonical address,
saved TF rejection, live TF/RF admission, page permissions and fetch/data
faults, physical backend aborts, and instruction/operand page boundaries. The
ISA test checks that saved-TF rejection occurs after required fetch with no CPU
commit. A separate machine-level probe during independent review (outside the
33-test target) confirmed the exact fetch A-bit PTE update and
`committed_write` callback on that path. Exact A/D PTE values and callback
order are checked by the system target for success, kernel-store #PF/backend
abort, and late user fetch/read failures; already committed SYSCALL state and
kernel-store writes remain visible.

Run the focused ISA, complete machine-system, and affected machine checks:

```sh
cargo test -p od-isa --lib syscall --offline -- --test-threads=1
cargo test -p od-host --test stage4_exception_delivery --offline -- --test-threads=1
cargo check -p od-machine --all-targets --offline
```

All 5 focused ISA tests and all 33 system tests passed. Formatting and the
machine all-target check passed; the build reports the existing unrelated
`od-jit::block::CodeMemory::one_instruction` dead-code warning. Graphify was
updated after code changes (27,525 nodes, 69,045 edges). Independent
Sol/high review accepted the slice with no P0–P3 findings. Cross-page accesses,
single-step/debug behavior, asynchronous events, stack switching, exception
delivery from CPL3 and Linux boot remain outside this slice. See the
[S4-20c report](../reports/STAGE_4_S4-20C_2026-11-06.md),
[plan](../plans/STAGE_4.md#s4-20-cpl3-и-системные-вызовывозвраты), and
[machine contract](../specs/MACHINE_CONTRACT.md#paged-syscall-kernel-store-sysretq-and-user-load-s4-20c).

<a id="s4-20d-paged-cpl3-mov-store-load-after-iretq"></a>

## S4-20d: paged CPL3 MOV store/load after IRETQ

The end-to-end sequence performs paged IRETQ, the accepted S4-20b user load,
then exact `48 89 0B` store and `48 8B 03` reload. The nonce is different from
initial data. Checks cover separate retirement and RIP progression, CPL3
selector caches, unchanged RSP/CR3, one operand write, and the nonce in RAX.
The legacy load-only entry still rejects the store encoding.

The system target tests user RO and supervisor-only stores with PFEC `0x07`,
supervisor-only read `0x05`, supervisor-only fetch on every page-table level
and NX fetch `0x15`, non-present instruction fetch `0x14`, and non-present
read/store `0x04`/`0x06`. It checks exact CR2/saved RIP, no retirement or
operand/D write on faults, and physical backend failure as `OperandPhysical`.
A clean first user fetch-fault fixture independently checks expected A-bit PTE
values and ordered callback ranges. Exact A/D readbacks and callback order are
also checked for non-present data and store-fault paths. Supervisor-only read
and late non-present fetch cases verify fault records and retention of earlier
committed effects without a separate exact A/D/callback assertion.

Run the focused ISA group, full machine-system target and affected machine
check:

```sh
cargo test -p od-isa --lib cpl3_ --offline -- --test-threads=1
cargo test -p od-host --test stage4_exception_delivery --offline -- --test-threads=1
cargo check -p od-machine --all-targets --offline
```

All 9 matching ISA tests and 35 system tests passed, as did formatting, the
machine all-target check and `git diff --check`. Graphify was updated after
code changes (27,540 nodes, 69,099 edges). Independent Sol/high review accepted
the slice with no P0–P3 findings. Live TF/RF/AC, cross-page accesses, privileged
instructions, invalid-selector IRETQ and CPL3 exception delivery remain outside
this slice. See the [S4-20d report](../reports/STAGE_4_S4-20D_2026-11-06.md),
[plan](../plans/STAGE_4.md#s4-20-cpl3-и-системные-вызовывозвраты), and
[machine contract](../specs/MACHINE_CONTRACT.md#paged-cpl3-mov-store-load-after-iretq-s4-20d).
