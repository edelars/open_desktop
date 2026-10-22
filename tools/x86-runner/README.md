# od-x86-runner

`od-x86-runner --fixture CASE.json --json` executes the S0-09 register-only profile on Linux x86-64 in a supervised child process. Input bytes must pass the project's `od-isa` decoder. Code, state, and stack use guarded mappings; code is writable only while populated, then changed to RX. The wrapper saves the host ABI callee-saved registers and captures GPR/RFLAGS after the fixture returns.

The fixture command caps wall time at the fixture maximum (30 s), CPU time at 2 s, address space at 256 MiB, result stdout at `MAX_RESULT_JSON_BYTES`, and diagnostics stderr at 64 KiB. The reusable supervisor accepts a CPU limit up to 30 s. It kills the whole child process group when limits are exceeded and waits for children. On Linux, `setrlimit` applies CPU, address-space, and core-dump limits before `exec`. Signals remain host harness failures, not guest exceptions.

Native instruction count is reported unavailable because the hardware run is not single-stepped or instrumented. Results remain comparable by the fixture's declared architectural scope. Unsupported instruction forms and branch targets are rejected before execution.

## Dependency

- `libc` 0.2.190 from crates.io (`MIT OR Apache-2.0`, default `std` feature): Linux `setrlimit` bindings for the child process. This package does not depend on `od-host`.
- `od-isa` from this workspace: strict allowlist decoding of fixture instruction forms and branch targets; the runner does not implement a second decoder.
