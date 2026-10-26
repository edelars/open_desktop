# od-runner

Headless runner for the limited S0-09 x86 fragment profile and the experimental S2 long-mode integer fixture profile. It calls `od-isa` for interpretation and uses the macOS ARM64 host path for S0 JIT execution. Instruction semantics and ARM64 translation remain in their owning libraries. Linux builds use the interpreter without linking `od-host`; JIT requests return an explicit `unsupported` JSON result. Schema v2 JIT execution is unsupported on every host.

Example on macOS ARM64:

```sh
cargo run --offline -p od-runner -- --hex '48 2b c1 75 fb' --rax 3 --rcx 1 --budget 32 --engine both
```

Use `--hex-file PATH` to run a whitespace-separated project fixture, for example:

```sh
cargo run --offline -p od-runner -- --hex-file tests/fixtures/x86/s0_09_sub_jnz_loop.hex --rax 3 --rcx 1 --budget 32 --engine both
```

Specify exactly one code source, `--hex` or `--hex-file`. The command prints interpreter and JIT state and exits with an error if they differ. The runner does not boot a guest OS or provide a VMHost/Swift supervisor.

## Versioned fixture results

Run a JSON fixture once per engine. The result is the only stdout content; diagnostics go to stderr.

```sh
cargo run --offline -p od-runner -- \
  --fixture tests/fixtures/x86/s0_09_add_overflow.case.json \
  --engine interpreter --json
cargo run --offline -p od-runner -- \
  --fixture tests/fixtures/x86/s0_09_add_overflow.case.json \
  --engine jit --json
```

Compare two saved results against the exact fixture used to produce them:

```sh
target/debug/od-runner --compare-results interpreter.json jit.json \
  --case tests/fixtures/x86/s0_09_add_overflow.case.json --json
```

Schema v1 comparison JSON contains `field`, optional `mask`, `expected`, `actual`, and fixture/backend `context` for every difference. Schema v2 comparison reports contain `schema_version`, `case_id`, `input_sha256`, `matches`, and differing field names. Exit codes are `0` for a completed run or comparison match, `2` for a comparison mismatch, `3` for invalid input, `4` for an unsupported operation or host, and `5` for a harness failure or deadline. Unsupported engine results remain valid JSON and never appear as completed runs. Unknown CPU model, kernel, page size, compiler version, and source revision are represented as unavailable metadata; the CLI does not guess them.

The fixture command accepts `interpreter` or `jit` per invocation and dispatches strictly by schema version. The S0-09 v1 fixture contract and digests remain unchanged; it does not capture RAM or exceptions, and its in-process path has no wall-time watchdog. The native x86 runner enforces its wall-time limit in an isolated process.

### Schema v2 memory execution

```sh
target/debug/od-runner --fixture tests/fixtures/x86/stage2/mov_load_add_store.case.json \
  --engine interpreter --json
```

The three [hand-authored S2 fixtures](../../tests/fixtures/x86/stage2/README.md)
exercise MOV load/store and register aliases, including a load → qword ADD →
store across an unaligned 4 KiB/adjacent-region boundary. The v2 interpreter
executes on-demand MOV B0–BF/88–8B byte/word/dword/qword forms with the full
GPR set, plus the earlier S0 qword arithmetic and short JZ/JNZ slice.
Unsupported forms remain explicit outcomes.

Results bind to the exact fixture bytes and contain all 16 GPRs, architectural
RIP, flags and definedness, retired instruction count, complete final memory
regions, observed exception metadata, and build/host metadata. Initial memory
is in the exact input fixture, so final memory is a real before/after pair.
MOV/branches preserve the known input flags; the supported ADD/SUB/CMP forms
define every arithmetic flag. The result therefore marks the current integer
profile flags as defined. New instruction families must establish their own
definedness before execution is enabled.

Architectural exceptions retain faulting RIP and available error code/address;
they are completed CPU outcomes, not host failures. Missing fixture mappings,
permissions and backend errors produce `harness_error` rather than a fabricated
guest page fault. A data access endpoint beyond `u64::MAX` is currently reported
as a diagnostic address-overflow harness error; its architectural boundary is
not yet established. Unsupported v2 JIT requests emit schema v2 `unsupported`
with unavailable state/count and exit 4; they do not execute the interpreter.

The v2 `wall_timeout_ms` is a cooperative execution deadline checked at each
instruction boundary, before budget/fetch and after the stop-address check.
Its expiry produces `timed_out` with the precise current snapshot/count and
exit 5. This is not an isolated watchdog and cannot interrupt a stalled memory
backend call. Budget exhaustion and stop address are separate architectural
outcomes. These local execution checks do not establish physical x86 equivalence;
the S2 oracle expansion remains a separate task.

Generate a deterministic CPU corpus with a fixed seed and sequence count:

```sh
target/debug/od-runner --generate-corpus 0x5eedcafe 1000 work/stage1-generated-5eedcafe
```

The command records each fixture as JSON with generator provenance, seed, and byte digest. The corpus generator only emits the current register-only profile.

Reduce a stable local interpreter/JIT mismatch:

```sh
target/debug/od-runner --minimize CASE.json --output work/reduced.case.json --max-attempts 1000
```

The reducer confirms the initial mismatch and each accepted candidate twice, keeping only candidates where the same comparison field still differs. Instruction removal remaps branches and the entry RIP. It fails when no stable local mismatch exists. Use `python3 tests/cpu/stage1_oracle.py CASE.json` to reproduce and compare one fixture with the physical x86 runner before reducing an oracle mismatch; automatic reduction of physical-only differences is not currently wired into this CLI.

## Reproducible CPU benchmark

Build the release runner, then run at least 20 alternating interpreter/JIT pairs on a fixed fixture:

```sh
cargo build --release --offline -p od-runner
target/release/od-runner \
  --hex-file tests/fixtures/x86/s0_09_sub_jnz_loop.hex \
  --rax 3 --rcx 1 --budget 32 --benchmark
```

`--benchmark` first checks that both engines produce the same result, then calibrates every measurement batch to at least 200 ms and records 20 samples by default. `--samples N` accepts 20–1000 samples. Each pair alternates which engine runs first; the three cold-build measurements rotate their order. Since each engine has a different calibrated iteration count, the CSV records both batch counts and elapsed times plus normalized nanoseconds per operation. Summary lines report median, minimum, and maximum.

The output separates `decode_ns_per_op`, `translation_ns_per_op` (decode plus lowering/emission), and `compile_publish_ns_per_op` (translation, executable mapping, code copy, instruction-cache invalidation, and mapping drop). It compares repeated interpreter calls with a `CompiledFragment` whose executable mapping is reused. Each invocation starts from the same guest state and uses the same instruction budget; correctness is checked before timing and after every measured batch. Interpreter time includes decode on each invocation. The benchmark is limited to this fragment and host path; it does not measure VMHost/IPC, process launch, a guest OS, or application workloads.

Save raw output under `work/stage-0/<run-id>/` with the exact command, source revision and dirty-tree state, host model/OS/build, toolchain, and observed power/thermal/background conditions. The benchmark records every measured sample but does not collect those host metadata itself. Do not treat the result as a product speed claim or a final E04 decision before the remaining E04 scenarios and independent x86 comparison.

S2-30bz's indexed FCMOVE fixtures verify complete x87 register payloads, tags,
environment and instruction-pointer state in the interpreter runner, including
the second move in the repeat stream. Schema v2 JIT remains unsupported.
S2-30ca's `DB C9` FCMOVNE fixtures assert the full x87 result for taken and
untaken ZF outcomes, reject an invalid source with ZF=1, and explicitly deny
JIT.

S2-30cb extends those fixtures to indexed `DB C8..CF`. Runner assertions cover
self-index ZF outcomes, indices 2..7, raw payload/tag copies, repeat and FCOMI
producer streams, invalid source with ZF=1, forbidden environment, unchanged
memory and full x87 snapshots. Every fixture explicitly denies JIT. This is
local interpreter evidence; no native or physical x86 execution is claimed.

S2-30cc's exact bare `DA D1` FCMOVBE fixture runner compares full machine
state and memory for the truth table, TOP wrap, both repeated transfers and
three FCOMI producers; invalid participant and environment fixtures remain
atomic. Every success and rejection fixture denies JIT. No native or physical
x86 execution is claimed.

S2-30ce fixture runs compare full machine and memory snapshots for bare
FCMOVNBE, signed-zero transfer, repeat, and FCOMI-produced conditions. Both
repeat instructions are checked by interpreter state tests; the runner denies
JIT for all success and rejection fixtures.
