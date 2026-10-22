# od-runner

Headless runner for the limited S0-09 x86 fragment profile. It calls `od-isa` and `od-host`; instruction semantics and ARM64 translation remain in their owning libraries.

Example on macOS ARM64:

```sh
cargo run --offline -p od-runner -- --hex '48 2b c1 75 fb' --rax 3 --rcx 1 --budget 32 --engine both
```

Use `--hex-file PATH` to run a whitespace-separated project fixture, for example:

```sh
cargo run --offline -p od-runner -- --hex-file tests/fixtures/x86/s0_09_sub_jnz_loop.hex --rax 3 --rcx 1 --budget 32 --engine both
```

Specify exactly one code source, `--hex` or `--hex-file`. The command prints interpreter and JIT state and exits with an error if they differ. The runner does not boot a guest OS or provide a VMHost/Swift supervisor.

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
