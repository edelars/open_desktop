# od-runner

Headless runner for the limited S0-09 x86 fragment profile. It calls `od-isa` for interpretation and uses the macOS ARM64 host path for JIT execution. Instruction semantics and ARM64 translation remain in their owning libraries. Linux builds use the interpreter without linking `od-host`; JIT requests return an explicit `unsupported` JSON result.

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

Comparison JSON contains `field`, optional `mask`, `expected`, `actual`, and fixture/backend `context` for every difference. Exit codes are `0` for a match, `2` for a mismatch, `3` for invalid input, `4` for an unsupported operation or host, and `5` for a harness failure. Unsupported engine results remain valid JSON and never appear as completed runs. Unknown CPU model, kernel, page size, compiler version, and source revision are represented as unavailable metadata; the CLI does not guess them.

The fixture command accepts `interpreter` or `jit` per invocation. The current S0-09 fixture contract does not capture RAM or exceptions. This in-process path has no wall-time watchdog; the native x86 runner enforces its wall-time limit in an isolated process.

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
