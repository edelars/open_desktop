# S2-31eb — historical IE and DE for raw FLD m80fp

The bounded interpreter admits historical IE and DE for exact bare DB 28
`FLD m80fp [RAX]` using its local `FSW & 0x80dc == 0` gate. The shared FILD
gate remains `0x80df`. Existing FCW, Empty destination, binary80 source-class,
raw-byte, single-read, rollback, push, TOP, C1, and pointer behavior remain in
force. No physical x86 acceptance is claimed; native and JIT execution remain
closed.

Coverage includes the accepted binary80 class matrix across TOP and FCW values,
and IE/DE combinations with and without sticky PE, unsupported status gates,
strict schema-v2 Unsupported validation, literal fixtures, and native/JIT
denial. For each accepted raw class, the ISA matrix covers all 8 TOP values, 4
admitted FCWs, and the 8 IE/DE/PE combinations. It checks raw destination bytes,
tag, TOP/C1, full sticky status preservation, FIP/FDP, FCW, opcode, GPRs,
RFLAGS, RIP, one 10-byte read, and surrounding canaries. The ISA matrix builds
the expected complete `LongModeState` and compares it after each push, including
selectors and SSE state. Read/span rollback checks start with IE+DE+PE already
set (`FSW=0x0423`) and verify that failures commit no CPU state.
A repeated DB28/DD38/DBE2/DB28/DD38 sequence checks IE/DE observation through
FNSTSW, clearing through FNCLEX, and re-entry into FLD.

Verification on the isolated worktree:

- `cargo test -p od-isa fld_m80fp --lib`: 5 passed.
- `cargo test -p od-testkit --lib`: 455 passed.
- `cargo test -p od-runner --test fixture_v2 v2_interpreter_preserves_historical_ie_de_and_pe_for_raw_fld_m80`: 1 passed.
- `cargo test -p od-runner --test fixture_v2 v2_native_and_jit_deny_fld_m80_historical_status_profile`: 1 passed.
- `cargo fmt --all -- --check`: passed.
- `cargo check --workspace --all-targets --offline`: passed.
- `graphify update .`: passed.

Fixture inputs are handwritten project evidence and do not claim native
hardware results. Native/JIT denial was verified through the runner profile;
physical x86 execution was not run.
