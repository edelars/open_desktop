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

Run `cargo test -p od-jit --lib ir_checked_direct_disp8_disp32_sites_match_decoded_metadata`, `cargo test -p od-jit --lib ir_checked_direct_displacements_reject_sib_prefix_register_and_trailing_forms`, `cargo test -p od-host v2_rip_relative_mov_success_targets_follow_guest_rip`, and `cargo test -p od-host bounded_dispatch_composes_rip_relative_nop_and_pair_with_actual_rip_continuation`. Direct V2/shared IR/interpreter comparisons cover disp8 -128/-1/0/127 and disp32 min/-1/0/1/max for both operations; load destination equals base and store source equals base. Cases cover signed wraparound into canonical memory, noncanonical start and end faults with no callback, backend errors, and STOP after committed store. The dispatcher test checks disp8 singleton → bare NOP → existing pair at budgets 0..4, pre-entry STOP, continuation from the returned RIP without repeated accesses, and truncated disp8 windows 1..3 plus disp32 windows 1..6 without publication or data access. These remain local macOS ARM64 fixtures, not owner/MMU or performance evidence.
