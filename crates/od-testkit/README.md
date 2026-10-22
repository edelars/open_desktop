# od-testkit

Test-only infrastructure for versioned CPU fixtures and machine-readable execution results. Production runtime crates must not depend on this crate.

The v1 format uses strict JSON parsing, fixed-width hexadecimal strings for 64-bit architectural values, SHA-256 digests, explicit observation availability, and bounded program/result sizes. The initial profile covers only the S0-09 register-only subset; memory and exception comparison are represented as unsupported, not as passing empty results.

`case::Fixture::from_json` validates a fixture and returns `LoadedFixture` with the digest of the exact JSON input bytes. `case::Fixture::to_json` validates before writing. `result::RunResult` provides equivalent bounded load/save operations and can check that its case ID, input digest, and profile match a supplied fixture. `compare::compare_results` rejects incompatible or incomplete runs before comparing fixture-scoped registers, RIP, masked RFLAGS, bounded memory regions, and supported exception metadata. The current S0-09 profile deliberately leaves memory and exception comparison unsupported.

`generate::generate_fixture(seed, sequence)` emits deterministic, versioned S0-09 cases. `minimize::minimize_fixture` reduces a fixture using `od-isa` instruction boundaries and a caller supplied mismatch-class callback; callers keep process and hardware execution outside testkit. Attempts count actual callback calls and require a limit of at least two. The initial mismatch and every accepted candidate must reproduce twice. Branch targets and the entry RIP are remapped when instructions are removed, and every candidate is validated again.

See [TEST_CASE_FORMAT.md](../../docs/testing/TEST_CASE_FORMAT.md) for fields, limits, dependency versions, provenance and sample files.

## Dependencies

- `serde` 1.0.229 with `derive`: general data model serialization.
- `serde_json` 1.0.151 with default `std`: JSON input/output.
- `sha2` 0.10.9 with default `std`: SHA-256 digests.
- `od-isa` workspace path dependency: reuse the project's decoder to locate valid instruction boundaries while reducing fixtures; testkit does not copy decoder semantics.

All are general-purpose infrastructure dependencies. None implement CPU semantics. The resolved transitive graph is pinned in the workspace `Cargo.lock`.
