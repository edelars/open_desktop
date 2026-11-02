# S2-31dy — historical ZE on raw FSTP m80fp

## Result

The exact bare `DB 38` (`FSTP m80fp [RAX]`) interpreter path now admits a
historical ZE bit by changing its status preflight mask from `0x80c4` to
`0x80c0`. SF, ES and B still reject before data access. FCW and source/tag/class
checks, canonical ten-byte range validation, raw source bytes, and the single
all-or-error write remain unchanged. Pop/TOP, tag, status, x87 pointers, RIP and
retirement still commit only after that write succeeds. The m32/m64 store gates
and ZE producers are unchanged.

Schema-v2 now treats ZE-only status as insufficient reason for an Unsupported
DB38 fixture. Explicit SF, ES or B status and unsupported FCWs remain valid
preflight-refusal reasons. JIT and native execution remain denied.

## Coverage

- Interpreter matrix: ZE plus all 32 historical IE/DE/OE/UE/PE combinations,
  across Normal, Zero, Infinity, QNaN, SNaN and Subnormal sources, both signs,
  all four admitted FCWs, every TOP, and both incoming C1 states. It verifies
  exact raw bytes, condition-code and sticky-bit preservation, C1 clearing, and
  post-write stack/pointer/RIP state.
- ZE-only SNaN history confirms raw signaling payload storage adds no IE.
- Refusal checks cover SF/ES/B together with ZE, malformed FCWs, and malformed
  source/tag/class pairs with ZE already admitted, all before memory access.
  Existing canonical-range,
  denied-write, backend-write-failure, architectural-write-fault, fetch,
  truncation, STOP and budget atomicity coverage remains in place.
- Repeated raw stores preserve both payloads; FNSTSW observes ZE, and FNCLEX
  clears it while preserving TOP.
- A handwritten literal DB38/FNSTSW fixture verifies ZE+OE state, raw SNaN
  payload preservation and retirement. Runner checks deny both JIT and native
  engines without fallback.

## Verification

- `cargo test -p od-isa --offline fstp_m80fp_` — 20 passed.
- `cargo test -p od-runner --offline --test fixture_v2 fstp_m80fp` — 1 passed.
- `cargo test -p od-runner --offline --test fixture_v2 x87_store_schema_does_not_accept_ze_only_as_an_unsupported_reason` — 1 passed.
- `cargo check -p od-isa -p od-testkit -p od-runner --all-targets --offline` — passed.
- `cargo fmt --all -- --check` — passed.

These are interpreter, schema and runner checks; they do not claim physical x86
acceptance or JIT/native support.
