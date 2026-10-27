# Проверка JIT этапа 3

Эта методика дополняет [контракт JIT](../specs/JIT_CONTRACT.md) и общие правила
из [BENCHMARKS](BENCHMARKS.md). Она описывает проверки, которые будут включаться
по мере готовности задач S3; не все пути реализованы сейчас.

## Срез A: один register-only блок

1. Использовать только точные S0-09 формы из JIT contract и fixtures с
   зафиксированными bytes, начальным состоянием и ожидаемым результатом.
2. Сравнивать одинаковый fixture на interpreter и новом block JIT, затем на
   физическом x86 oracle. Сверять RAX/RCX/RDX/RBX, RIP, определённые flags и
   exit reason. Не трактовать undefined flags или недоступное наблюдение как
   совпадение.
3. До появления dispatcher (S3-15) ограничивать corpus целыми программами,
   исполняемыми одним блоком, включая допустимую terminal branch. Multi-block
   corpus и полный Stage 1 harness остаются отложенными до S3-15.
4. Проверять отрицательные входы: неразрешённый opcode/prefix/register,
   некорректная branch target, усечённая инструкция, branch в середину
   инструкции и предел блока. Не публиковать и не исполнять неполный или
   невалидный host output. Если следующая guest instruction за валидным
   префиксом блока даёт fault/Unsupported, сохранить этот корректный префикс и
   наблюдать исход только при достижении соответствующего RIP; правила этого
   случая фиксируются в S3-04.
5. Явно подтверждать, что arithmetic flags вычисляет собственный x86 helper,
   если lowering ещё не materializes их напрямую; совпадение host NZCV с x86
   flags не предполагается.

## Срез B и последующие зависимости

- Проверять budget, STOP, dispatcher, точный progress и interpreter fallback
  только после S3-15/S3-16 и с точной семантикой нужной формы этапа 2.
- Memory forms не принимать до checked bridge S3-19, соответствующего
  interpreter slice, fault cases и условий ISSUE-012. Сверять адресное
  пространство, access origin, fault result и порядок side effects.
- Проверять инвалидирование code dependencies и освобождение compiled blocks
  после реализации соответствующих S3 tasks. Не считать изменение fixture
  buffer заменой проверки guest code writes/mapping changes.
- Host execution, executable-memory publication и I-cache synchronization
  проверять на поддержанном Apple Silicon/macOS configuration. Отдельно
  фиксировать signing/Hardened Runtime entitlements и SDK; не переносить
  результаты одной машины на другую.

## Протокол и доказательства

### Bounded S3-27b next fetch after memory pair

The native fixture runs either exact two-site pair through the generated entry,
then applies a controlled `GuestMemory::fetch` architectural fault at valid
guest RIP `A+6` by calling the interpreter on the returned state and RAM. A
separate `od_isa::execute::run` runs the same pair and faulting third fetch,
reporting the fault with two `instructions_executed`. Run the interpreter
again from the native-returned state/RAM; it reports the same fault with zero
additional instructions. Compare the complete CPU state including flags/RIP
and all RAM; check the full fault payload and no repeated pair data callbacks
on fault or retry. Inject unmapped/backend fetch errors for both pair forms and
require host-error classification with zero continuation instructions. Real
`RunConfig` zero-budget and STOP closure exits suppress the next fetch; after
replenishing budget or clearing STOP, continuation reports the architectural
fault with zero instructions. The six-byte
compiler `CodeView` ending at `A+6` is independent of the guest mapping/fetch
result. This test proves a fixture boundary only; production dispatcher and
owner-backed MMU behavior remain untested.

Production acceptance additionally needs the `od-machine` owner contract:
private synchronous RAM capability, code lease across compile/publication/
execution, MMU-aware fetch with deferred next-RIP fault delivery, and
committed-write batches from every writer with alias-aware code invalidation.
The dispatcher must check stop-RIP → STOP → budget before fetch, treat fixture
window end as distinct from guest termination, and release both owner lease
and published mapping at their documented safe points. See the [S3-15n
contract](../specs/JIT_CONTRACT.md#production-memory-owner-integration-prerequisites-s3-15n).
These are prerequisites to owner-backed end-to-end testing, not claims made by
the fixture cases above; physical Mac mini comparison and performance
measurements remain separate final gates.

### S3-16m bounded-memory bare-NOP fallback

Run `cargo test -p od-host --offline bounded_memory_dispatch`. The native
fixture uses pair → exact bare `90` → pair and compares complete CPU state,
data RAM and callback counts with independent full-interpreter execution at
budgets 0 through 5. It checks native/interpreter retired counts, strict
`run` rejection at `90`, STOP and stop-RIP on both sides of the NOP, and
continuation from the post-NOP RIP without repeating the first pair's memory
accesses. An unsupported instruction after NOP preserves prior commits.
Prefixed NOP, `0F 1F` and PAUSE must not increment interpreter retirement.
Successful publication of the trailing pair exercises mapping release before
the next native publication; the executor contract and explicit host drop
place that release before the interpreter step. This is fixture-only evidence;
it adds neither general interpreter fallback nor production owner/MMU tests.

### Bounded S3-34a two-site memory corpus

For the two exact V2 memory sequences in the JIT contract, run
`cargo test -p od-host --test memory_corpus`. On local macOS ARM64 this runs
2,400 unique successful inputs (three fixed seeds × 400 per sequence), with
two native-entry and interpreter executions per input on independent full
CPU/RAM snapshots. Inputs vary qword state, destination registers, address
alignment, same/different regions, and safe partial overlap in load→store.
Checks include full state and RAM, exit/budget/retirement, and memory callback
order/count. Four separately named tests cover comparator corruption, mismatch
diagnostic serialization, guest fault, STOP, budget exhaustion, and second-site
continuation from the actual committed CPU/RAM result of the first
budget-limited entry. The test is
platform-gated and is not a physical x86 oracle or Mac mini check; those remain
separate suites and must be reported as `not_run` until executed.

On mismatch the test emits seed and case ID and writes JSON containing the
original sequence/register/flags/address/RAM input and both serialized
observations under the system temporary directory. A failed file write is
included in the panic output.

To replay a saved report explicitly, run
`OD_JIT_REPLAY_CASE=/absolute/path/case.json cargo test -p od-host --test memory_corpus replay_saved_case_from_environment -- --ignored --exact`.
The ignored test requires this variable; if it is missing, an explicit replay
fails with a diagnostic. It validates the version/profile, exact source bytes,
supported CPU/RAM snapshot, and execution protocol before native publication.
It replays each backend independently and compares each phase, including
continuation from the prior phase's returned state and memory. A fresh
`.replay-<pid>.json` report is created beside the original without overwriting
it; that report includes the current environment, build revision, backend
availability, result snapshots, and whether the saved mismatch classes were
reproduced. Unsupported native hosts and malformed or unsupported reports
cannot produce an equivalence-success report.
Continuation mismatch reports include the first budget-limited result and the
resumed result as separate paired snapshots, even when only the latter differs;
the decoder requires the snapshot count to match the saved protocol.

To run the bounded reducer on a saved mismatch report, run
`OD_JIT_REDUCE_CASE=/absolute/path/case.json cargo test -p od-host --test memory_corpus reduce_saved_case_from_environment -- --ignored --exact`.
The reducer repeats the original mismatch twice before attempting candidates,
then accepts only strictly simpler inputs with exactly matching recorded
mismatch classes at every execution phase. It modifies non-address initial GPR values and RAM contents
only, and preserves the sequence, source bytes, addresses and protocol. A fresh
`.reduced-<pid>.json` report records both envelopes, the fresh observations,
attempt count and stop reason; it is created without replacing the original.
The reducer has a 512-attempt maximum and runs only on macOS ARM64. Its positive
self-test injects a clearly labeled GPR[10] corruption when the input GPR[11]
marker is nonzero; clearing that marker removes the synthetic mismatch and is
rejected by the normal comparator. The generic reducer does not reserve GPR[11]
and can reduce it when the mismatch predicate permits. This synthetic case
validates reducer behavior and is not a JIT defect report.

Каждая новая строка support matrix получает literal positive/negative cases,
точный сравниваемый state и внешний prerequisite. В отчёте сохранять git
revision, build fingerprint, host/OS/toolchain, fixtures/corpus fingerprint,
seed и повторы. Физический x86 oracle обязателен там, где его требует план;
его ошибка или отсутствие соединения не могут превращаться в пропуск с green
status.

Performance checks описаны отдельно в S3-35/S3-36: cold path включает
перевод и первое исполнение, warm path использует уже опубликованный cache.
Сравнивать одинаковые входы и требования к корректности, делать повторные
прогоны, показывать распределение и ресурсы. Пилот Stage 0 не является
baseline block JIT; не утверждать ускорение без новых измерений.

### S3-20l RIP-relative shared-IR and dispatcher extension

Run `cargo test -p od-jit --lib ir_checked_rip_relative_load_store_keep_decoded_next_rip_and_displacement`, `cargo test -p od-jit --lib ir_rip_relative_singletons_reject_wrong_register_prefix_and_trailing_bytes`, and `cargo test -p od-host bounded_dispatch_composes_rip_relative_nop_and_pair_with_actual_rip_continuation`. The bounded dispatcher test exercises both RIP-relative singleton → NOP → pair and NOP → RIP-relative singleton → pair. Distinct adjacent data sentinels verify the second order uses the singleton's instruction-local `next_rip`; budgets 0..4 compare full CPU state and data RAM against interpreter steps, and continuation resumes from the actual returned RIP. Partial byte windows 1..6 return `BoundaryReached` with zero retirement, publication, or data callbacks. The RIP-relative native host test compares direct V2 and shared IR against interpreter for negative, zero and positive disp32 at an entry inside CodeView. Shared-IR load/store additionally check architectural fault payload and atomic CPU/RAM/budget state, backend errors as host errors, and STOP after successful store commit. These tests are local macOS ARM64 fixture evidence; owner/MMU integration, physical x86, and performance remain open.

### S3-20m signed direct-base displacement singleton

Run `cargo test -p od-jit --lib ir_checked_direct_disp8_disp32_sites_match_decoded_metadata`, `cargo test -p od-jit --lib ir_checked_direct_displacements_reject_prefix_register_and_trailing_forms`, `cargo test -p od-host v2_rip_relative_mov_success_targets_follow_guest_rip`, and `cargo test -p od-host bounded_dispatch_composes_rip_relative_nop_and_pair_with_actual_rip_continuation`. Direct V2/shared IR/interpreter comparisons cover disp8 -128/-1/0/127 and disp32 min/-1/0/1/max for both operations; load destination equals base and store source equals base. Cases cover signed wraparound into canonical memory, noncanonical start and end faults with no callback, backend errors, and STOP after committed store. The dispatcher test checks disp8 singleton → bare NOP → existing pair at budgets 0..4, pre-entry STOP, continuation from the returned RIP without repeated accesses, and truncated disp8 windows 1..3 plus disp32 windows 1..6 without publication or data access. These remain local macOS ARM64 fixtures, not owner/MMU or performance evidence.

### S3-20n no-displacement SIB singleton

Run `cargo test -p od-jit --lib ir_checked_sib_singletons_match_decoded_metadata_for_all_registers_and_scales`, `cargo test -p od-host v2_sib_mov_all_register_scale_combinations_match_shared_ir_and_interpreter`, and `cargo test -p od-host bounded_dispatch_composes_rip_relative_nop_and_pair_with_actual_rip_continuation`. Compiler metadata and native differential cases enumerate both MOV directions, all RAX..RBX data/base/index encodings, and scales 1/2/4/8; aliases and zero-valued index are included. Descriptor-driven native cases cover multiply/add wrapping to canonical address zero, noncanonical start/end fault atomicity, load/store backend error classification, exact architectural store fault payload with unchanged CPU/RAM/budget/retirement, and committed SIB store followed by STOP. Dispatcher budgets 0..4 compare full CPU/RAM to interpreter for SIB → NOP → pair; STOP after the SIB load pauses before NOP and resumes from actual RIP without repeating the access. Byte windows ending before the SIB byte return `BoundaryReached` without retirements/data callbacks. Negative cases reject truncated SIB, no-index/no-base, RSP/RBP, extension bits, displacement, `67`, and trailing bytes. The direct disp8 `mod=01` four-byte form remains separately tested. Evidence is local ARM64 fixture only; owner/MMU integration, physical x86, and performance remain open.

### S3-20o address32 direct-base qword singleton

Run `cargo test -p od-jit --lib ir_address32_direct_base_mov_has_exact_qword_descriptor`, `cargo test -p od-host ir_address32_mov_matches_v2_and_interpreter_with_qword_access_semantics`, and `cargo test -p od-host bounded_dispatch_composes_rip_relative_nop_and_pair_with_actual_rip_continuation`. The accepted source is exactly `67 48 8B/89 /r` with ModRM `mod=00`, base/data RAX..RBX, qword data width and dword address width. Native direct V2/shared IR/interpreter comparisons cover all base/data pairs and both operations, including destination/source alias with base and noncanonical full u64 base values. The base `0xffff_fffc` case checks one contiguous qword callback spanning through `0x1_0000_0003`. Load and store architectural faults check exact payload and no CPU/RAM/budget/retirement commit; backend store errors stay host errors; zero-budget, stop-RIP and STOP guards make no callback; successful store+STOP preserves bytes and retirement. Mixed dispatch exercises address32 singleton → NOP → existing pair with budgets 0..4, stop-before-NOP and continuation without repeated access. Partial prefix/opcode/ModRM windows and wrong prefix order, SIB, displacements, extra bytes and unsupported registers are rejected. Evidence is local ARM64 fixture only; owner/MMU integration, physical x86, and performance remain open.

### S3-20p address32 signed-displacement qword singleton

Run `cargo test -p od-jit --lib address_size_override_signed_displacements_are_admitted_by_decoded_shape`, `cargo test -p od-host v2_address32_signed_displacements_match_interpreter_and_cross_32bit_boundary_linearly`, and `cargo test -p od-host bounded_dispatch_composes_rip_relative_nop_and_pair_with_actual_rip_continuation`. The shared checked source is exactly `67 48 8B/89 /r disp8|disp32`, ModRM `mod=01/10`, data/base RAX..RBX, lengths 5/8, qword data and dword EA. Descriptor and native cases cover disp8 -128/-1/0/1/127 and disp32 MIN/MIN+1/-1/0/1/MAX-1/MAX, both operations, base alias, noncanonical u64 base bits, and modulo-2^32 wrapping. Callback records assert exactly one qword access at the decoded target, including a contiguous access beginning at `0xffff_fffc`. Shared IR exact architectural fault payloads leave CPU/RAM/budget/retirement unchanged; backend errors remain host errors; store+STOP preserves the committed qword. Dispatcher composes disp8 → bare NOP → existing pair across budgets 0..4 and resumes at the returned RIP; partial disp8 and disp32 windows return `BoundaryReached` without publication/access. Incorrect prefix, SIB, unsupported registers, trailing bytes, and wrong lengths remain rejected. Evidence is local ARM64 fixture only; owner/MMU integration, physical x86, and performance remain open.

### S3-20q 64-bit SIB signed-displacement qword singleton

Run `cargo test -p od-jit --lib ir_checked_sib_displacements_match_decoded_metadata_and_reject_other_shapes`, `cargo test -p od-host v2_sib_mov_all_register_scale_combinations_match_shared_ir_and_interpreter`, and `cargo test -p od-host bounded_dispatch_composes_rip_relative_nop_and_pair_with_actual_rip_continuation`. Exact accepted sources are `48 8B/89 /r sib disp8|disp32`, ModRM `mod=01/10,r/m=100`, base/index/data RAX..RBX, scale 1/2/4/8, lengths 5/8. Descriptor and host matrices cover disp8 -128/-1/0/127 and disp32 MIN/-1/0/MAX across all scales and both operations; alias cases include data/base, data/index, and base/index for both displacement widths. Native direct V2/shared IR/interpreter state and RAM parity, callback address and count are checked. Canonical EA wrap, noncanonical start/end, full architectural fault payload/no commit, backend host errors, and committed store+STOP are covered. Dispatcher composes SIB disp8 → NOP → existing pair for budgets 0..4; STOP resumes from the actual RIP without repeating the SIB access. Partial disp8/disp32 and invalid no-index SIB windows do not publish or call memory. Evidence is local ARM64 fixture only; owner/MMU integration, physical x86, and performance remain open.

### S3-20r address32 no-displacement SIB qword singleton

Run `cargo test -p od-jit --lib ir_address32_sib_no_displacement_has_exact_qword_descriptor`, `cargo test -p od-host v2_address32_sib_mov_matches_direct_ir_and_interpreter`, and `cargo test -p od-host bounded_dispatch_composes_rip_relative_nop_and_pair_with_actual_rip_continuation`. The exact shape is `67 48 8B/89 /r sib`, ModRM `mod=00,r/m=100`, data/base/index RAX..RBX, scale 1/2/4/8, five bytes, qword data and dword EA. Descriptor cases enumerate both operations and every supported data/base/index/scale combination; forbidden no-index/no-base, displaced SIB, extra-prefix and trailing forms reject. Direct V2/shared IR/interpreter parity checks CPU/RAM, callback address/count, aliases, ignored high halves, modulo-2^32 wrap and contiguous qword access at `0xffff_fffc`. Architectural faults preserve exact payload and no state/RAM/budget/retirement commit, backend errors stay host errors, and successful SIB store+STOP remains committed. Dispatcher composes addr32 SIB → NOP → pair at budgets 0..4, tests no-effect guards and partial windows, and resumes from actual RIP without repeating the first access. Evidence is local ARM64 fixture only; owner/MMU integration, physical x86, and performance remain open.

### S3-20s address32 signed disp8 SIB qword singleton

Run `cargo test -p od-jit --lib ir_address32_sib_disp8_has_exact_qword_descriptor`, `cargo test -p od-host v2_address32_sib_mov_matches_direct_ir_and_interpreter`, and `cargo test -p od-host bounded_dispatch_composes_rip_relative_nop_and_pair_with_actual_rip_continuation`. The accepted source is exactly `67 48 8B/89 /r sib disp8`, ModRM `mod=01,r/m=100`, data/base/index RAX..RBX, scale 1/2/4/8, six bytes, qword data and dword EA. Descriptor tests cover both directions, every supported register/scale combination, disp8 `-128/-1/0/127`, and reject truncated bytes, wrong ModRM/SIB fields, wrong prefix, trailing bytes and disp32. Native direct V2/shared IR/interpreter parity verifies state/RAM, one callback at the decoder-derived qword target, aliases, upper-half ignore, signed modulo-2^32 arithmetic, and a contiguous access across `0xffff_ffff`. Fault payloads preserve state/RAM/budget/retirement, backend errors remain host errors, and store+STOP preserves the committed write. Dispatcher budgets 0..4 compare against interpreter; a fixture ending exactly after the six-byte singleton proves it is not treated as a pair by length. NOP/pair continuation resumes without repeating the first access. Evidence is local ARM64 fixture only; owner/MMU integration, physical x86, and performance remain open.

### S3-20t address32 signed disp32 SIB qword singleton

Run `cargo test -p od-jit --lib ir_address32_sib_disp32_has_exact_nine_byte_qword_descriptor`, `cargo test -p od-host v2_address32_sib_mov_matches_direct_ir_and_interpreter`, and `cargo test -p od-host bounded_dispatch_runs_address32_sib_disp32_nop_and_pair_continuously`. The exact source is `67 48 8B/89 /r sib disp32`, ModRM `mod=10,r/m=100`, data/base/index RAX..RBX, scale 1/2/4/8, qword data and dword EA. Descriptor coverage checks both directions, every supported register/scale combination and disp32 MIN/-1/0/MAX. Host direct V2/shared IR/interpreter parity checks callback address/count, complete state/RAM, RAX base/data alias, high-half truncation and contiguous qword access at `0xffff_fffc`; sources differing only in byte nine retain distinct metadata. Architectural faults preserve exact payload and no CPU/RAM/budget/retirement commit, backend errors remain host errors, and successful store+STOP preserves the committed write. Dispatcher composes the nine-byte singleton → bare NOP → existing pair at budgets 0..4, checks stop-RIP and callback STOP, then resumes at the actual RIP without repeating the first access. Partial windows 1..8 and invalid SIB/ModRM/prefix/tail encodings reject before a data callback. `source_bytes_storage()` now returns `([u8; 9], u8)`; qword data buffers, pair `[u8; 6]`, ActiveCodeV3 `[u8; 23]` and RuntimeContext/C ABI remain unchanged. Evidence is local ARM64 fixture only; owner/MMU integration, physical x86, and performance remain open.

### S3-20u RBP-base signed disp8 qword singleton

Run `cargo test -p od-jit --lib ir_rbp_disp8_has_exact_qword_descriptor_and_rejects_other_rbp_forms`, `cargo test -p od-host v2_address32_sib_mov_matches_direct_ir_and_interpreter`, and `cargo test -p od-host bounded_dispatch_runs_address32_sib_disp32_nop_and_pair_continuously`. The exact source is `48 8B/89 /r disp8`, ModRM `mod=01,r/m=101`, RBP base, data RAX..RBX, qword data, 64-bit EA and exact length four. Descriptor/direct V2/shared IR/interpreter cases cover both directions and disp8 -128/-1/0/127 with callback address/count and full state/RAM parity; RBP and RSP are unchanged. Noncanonical RBP start and last-byte crossing return #SS (12/0/no linear address), while matching RBX cases return #GP (13/0/no linear address), all before callbacks and with no state/budget/retirement commit. AddressOverflow retains the host diagnostic kind, backend failures remain host errors, entry STOP/budget zero do not access memory, and a successful store+STOP stays committed. Bounded dispatch composes RBP disp8 → bare NOP → existing pair for budgets 0..4 and continuation from the actual RIP without duplicate callbacks. Truncation and unsupported prefix/SIB/disp32/extended forms reject. Evidence is local ARM64 fixture only; owner/MMU integration, physical x86, and performance remain open.

### S3-20v RSP-base exact no-index SIB qword singleton

Run `cargo test -p od-jit --lib ir_rsp_base_no_index_sib_is_exactly_the_0x24_form`, `cargo test -p od-host v2_address32_sib_mov_matches_direct_ir_and_interpreter`, and `cargo test -p od-host bounded_dispatch_runs_address32_sib_disp32_nop_and_pair_continuously`. The only accepted encoding is `48 8B/89 /r sib`, ModRM `mod=00,r/m=100`, exact SIB `0x24`; RSP is the base, index is absent, scale is 1, displacement is zero, data is RAX..RBX, qword/64-bit EA, and length is four. Direct V2/shared IR/interpreter comparisons cover both operations, all data registers, callback target/count, full state/RAM and retirement with RSP/RBP/flags unchanged. Noncanonical RSP start and last-byte range produce #SS; the corresponding RBX controls produce #GP, with exact vector/error/linear payload and no callback/state/RIP/RAM/budget/retirement commit. AddressOverflow and ordinary backend error remain separate host classifications. Entry STOP/budget zero make no callback and successful store+STOP retains its write. Dispatcher compares budgets 0..4, split continuation after the RSP singleton and partial windows 1..3; invalid SIB/index/scale/displacement/prefix/RSP-data variants return Unsupported before publication or memory callbacks. Evidence is local ARM64 fixture only; owner/MMU integration, physical x86, and performance remain open.

### S3-20w RSP-base no-index SIB with signed disp8

Run `cargo test -p od-jit --lib ir_rsp_base_no_index_sib_disp8_is_exact_and_signed`, `cargo test -p od-host v2_address32_sib_mov_matches_direct_ir_and_interpreter`, and `cargo test -p od-host bounded_dispatch_runs_address32_sib_disp32_nop_and_pair_continuously`. The exact singleton profile is `48 8B/89 /r sib disp8`, ModRM `mod=01,r/m=100`, SIB `0x24`, RSP base, no index/scale 1, signed disp8, data RAX..RBX, qword and 64-bit EA, length five. The existing no-displacement `[RSP]` form stays separate. Direct V2/shared IR/interpreter tests cover both operations and -128/-1/0/127 with complete state/RAM, callback address/count, retirement and unchanged RSP/RBP/flags. Fault tests retain #SS for RSP and #GP controls, no memory callback/commit, distinct AddressOverflow/backend errors, entry STOP/budget-zero guards, and committed store+STOP. Dispatcher checks budgets 0..4, STOP-RIP before the NOP bridge, continuation from the singleton's actual next RIP without replay, pair execution, partial windows 1..4 and invalid index/disp32/prefix variants without callbacks. Evidence is local ARM64 fixture only; owner/MMU integration, physical x86 and performance remain open.

### S3-20x RSP-base no-index SIB with signed disp32

Run `cargo test -p od-jit --lib ir_rsp_base_no_index_sib_disp32_is_exact_and_keeps_all_source_bytes`, `cargo test -p od-host v2_address32_sib_mov_matches_direct_ir_and_interpreter`, and `cargo test -p od-host bounded_dispatch_runs_address32_sib_disp32_nop_and_pair_continuously`. The exact singleton is `48 8B/89 /r sib disp32`, ModRM `mod=10,r/m=100`, SIB exactly `0x24`, RSP base/no index/scale 1, disp32 at source offsets 4..7, data RAX..RBX, qword/64-bit EA, length eight. Descriptor checks both operations, all data registers, signed MIN/-1/0/MAX and exact all-byte source identity; two sources differing only at byte 7 decode to distinct offsets. Host direct V2/shared IR/interpreter checks both operations at signed boundaries with one contiguous callback across `0xffff_ffff`, plus a positive address sum wrapping to canonical zero. Fixed-RSP sources differing only in byte 7 yield the explicit different callback addresses `0x0000_7000_0100_5020` and `0x0000_7000_0200_5020`. RSP #SS/RBX #GP start/end controls, AddressOverflow and backend classification, no commit, entry STOP/budget-zero and successful store+STOP are checked. Dispatcher composes singleton→NOP→pair at budgets 0..4, STOP-RIP and continuation without replay; windows 1..7 and invalid no-index/profile/prefix shapes have no data effects. Existing `[RSP]` and disp8 tests remain. Evidence is local ARM64 fixture only; owner/MMU integration, physical x86 and performance remain open.

### S3-20y RBP-base signed disp32 qword singleton

Run `cargo test -p od-jit --lib ir_checked_rbp_base_disp32_is_distinct_from_rip_relative_disp32`, `cargo test -p od-host v2_address32_sib_mov_matches_direct_ir_and_interpreter`, and `cargo test -p od-host bounded_dispatch_runs_address32_sib_disp32_nop_and_pair_continuously`. The exact source is `48 8B/89 /r disp32`, ModRM `mod=10/rm=101`, RBP base, data RAX..RBX, qword/64-bit EA and length seven. Descriptor/direct V2/shared IR/interpreter tests cover both operations and disp32 MIN/-1/0/127/128/MAX, full state/RAM parity, callback address/count and retirement. The same displacement under `mod=00/rm=101` is explicitly checked to use instruction-local NextRip and yield a different expected target. RBP noncanonical start/end gives exact #SS payload without callback while RBX controls give #GP; AddressOverflow and backend failures remain distinct host errors with no commit. STOP/budget-zero suppress callbacks; successful store+STOP remains committed. Dispatcher composes RBP disp32→bare NOP→pair across budgets 0..4, STOP-RIP and continuation from actual RIP without replay; partial windows 1..6 have no effects. Evidence is local ARM64 fixture only; owner/MMU integration, physical x86 and performance remain open.


### S3-20z guest RSP as qword MOV data register

Run `cargo test -p od-jit --lib rsp_is_admitted_only_as_qword_memory_data_for_exact_direct_forms` and `cargo test -p od-host rsp_is_an_exact_qword_memory_data_register_and_commits_before_next_use`. Only exact three-byte `48 8B/89 /r` with ModRM `mod=00`, data RSP and base RAX..RBX is admitted; generic GPR read/write validation remains limited to RAX..RBX. Direct V2/shared IR/interpreter parity covers both load and store, full-width RSP values, state/RAM, callback target/count and retirement. A loaded noncanonical value commits to guest RSP only after successful read; store parity proves all 64 bits of `0x0000_8000_0000_0000` reach RAM, since canonicality applies to the effective address. The IR frame stages GPRs at `SP+56..95`, flags at `SP+96`, RIP at `SP+104`, and preserves X19/X20 and X21/X22 at `SP+112` and `SP+128`; the required reserved prefix is 144 bytes. A native x19 canary covers successful return, budget guard and architectural fault. Entry STOP and budget zero perform no callback; a budget-one split resumes at the actual RIP without repeating the load. Negative encoding variants remain unsupported. The sequential dispatcher case commits `MOV RSP,[RAX]`, then faults with #SS on `MOV RCX,[RSP]` at the second RIP, preserving the first effect and not repeating its read on callback STOP/resume. Evidence is local ARM64 fixture only; owner/MMU integration, physical x86 and performance remain open.


### S3-20aa guest RBP as qword MOV data register

Run `cargo test -p od-jit --lib rbp_is_admitted_only_as_qword_memory_data_for_exact_direct_forms`, `cargo test -p od-jit --lib rbp_memory_data_spill_stays_after_six_gpr_runtime_prefix`, and `cargo test -p od-host rsp_is_an_exact_qword_memory_data_register_and_commits_before_next_use`. Admit only exact three-byte `48 8B/89 /r`, ModRM `mod=00`, data RBP and base RAX..RBX. Generic GPR validation stays restricted, and the existing pair-profile compiler rejects RBP-data pair sequences. Direct V2/shared IR/interpreter tests compare full CPU/RAM state, callback address/count and retirement for load/store; a noncanonical RBP payload is stored as all eight bytes. Failed accesses preserve old RBP. The six-slot frame places RAX..RBP at SP+56..103, flags +104, RIP +112, X19/X20 at +128 and X21/X22 at +144, with 160-byte prefix; forced spill and emitted PC-map regression covers allocator storage after the prefix. x19 canary covers repeated success, budget guard and fault returns. Dispatcher checks entry STOP/budget-zero, budget-one and callback-STOP split/resume, then `MOV RBP,[RAX]; MOV RCX,[RBP+0]` faults #SS on the second instruction while preserving the first commit and avoiding repeated reads. Evidence is local ARM64 fixture only; owner/MMU integration, physical x86 and performance remain open.

### S3-20ab guest RBP as both qword MOV data and disp8 base

Run `cargo test -p od-jit --lib ir_rbp_rbp_disp8_has_exact_dual_register_descriptor`, `cargo test -p od-jit --lib ir_rbp_disp8_has_exact_qword_descriptor_and_rejects_other_rbp_forms`, and `cargo test -p od-host rbp_data_disp8_uses_old_rbp_for_address_and_dispatch_continuation`. Only exact four-byte `48 8B/89 6D ib` is admitted: ModRM `mod=01/reg=5/rm=5`, both operand data register and decoded address base RBP, signed disp8, qword data and 64-bit EA. Direct V2/shared IR/interpreter compare both operations at -128/-1/0/127, complete CPU/RAM state, callback target/count and retirement. The load callback target is calculated from the old RBP even though the instruction commits a different value into RBP; stores use old RBP both as address base and full 64-bit payload. A current noncanonical RBP access returns #SS before the backend callback and preserves state/RAM/budget/retirement. The dispatcher budget split and callback STOP both commit the first load, resume at its actual next RIP, fault on the subsequent RBP-relative access, and never repeat the first read. Existing RBP-data-only, RBP-base-only and RSP-data tests remain covered. Evidence is local ARM64 fixture only; production owner/MMU integration, physical x86, Mini and performance remain open.
