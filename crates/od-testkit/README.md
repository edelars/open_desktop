# od-testkit

Test-only infrastructure for versioned CPU fixtures and machine-readable execution results. Production runtime crates must not depend on this crate.

The v1 format uses strict JSON parsing, fixed-width hexadecimal strings for 64-bit architectural values, SHA-256 digests, explicit observation availability, and bounded program/result sizes. The initial profile covers only the S0-09 register-only subset; memory and exception comparison are represented as unsupported, not as passing empty results.

`case::Fixture::from_json` validates a fixture and returns `LoadedFixture` with the digest of the exact JSON input bytes. `case::Fixture::to_json` validates before writing. `result::RunResult` provides equivalent bounded load/save operations and can check that its case ID, input digest, and profile match a supplied fixture. `compare::compare_results` rejects incompatible or incomplete runs before comparing fixture-scoped registers, RIP, masked RFLAGS, bounded memory regions, and supported exception metadata. The current S0-09 profile deliberately leaves memory and exception comparison unsupported.

`generate::generate_fixture(seed, sequence)` emits deterministic, versioned S0-09 cases. `minimize::minimize_fixture` reduces a fixture using `od-isa` instruction boundaries and a caller supplied mismatch-class callback; callers keep process and hardware execution outside testkit. Attempts count actual callback calls and require a limit of at least two. The initial mismatch and every accepted candidate must reproduce twice. Branch targets and the entry RIP are remapped when instructions are removed, and every candidate is validated again.

See [TEST_CASE_FORMAT.md](../../docs/testing/TEST_CASE_FORMAT.md) for fields, limits, dependency versions, provenance and sample files.

Schema v2 (`s2-long-integer-v1`) provides full 16-GPR/RIP/RFLAGS state,
bounded fixture RAM, fault observations and strict comparison. The locally accepted S2-08 slice
connects `od-runner --fixture --engine interpreter` to actual MOV execution
and full snapshots; v2 JIT remains explicitly Unsupported. Architectural faults,
mapping/backend errors and cooperative wall timeout have distinct outcomes.
The three [stage2 fixtures](../../tests/fixtures/x86/stage2/) exercise real
programs locally. S2-09 separately passed three hardware-layout MOV/memory
programs twice on one Intel CPU; see [CPU_ORACLE](../../docs/testing/CPU_ORACLE.md#native-v2-s2-09-границы-физического-подтверждения)
for evidence and limits. This does not verify the full stage 2 profile.
S2-10 adds five hand-authored transfer programs with canaries and intermediate
result snapshots. The eight-case v2 suite passed twice on one Intel CPU,
alongside the 1010-case legacy regression; [transfer evidence](../../docs/reports/data/stage2/transfers-2026-10-22.json)
records exact inputs and scope. Memory XCHG, privileged faults and v2 JIT remain outside that confirmation.

S2-11 adds 313 one-instruction arithmetic fixtures and a literal full-state,
flags and memory sidecar. The materializer packages handwritten templates;
it does not evaluate x86 arithmetic. The 321-case v2 suite passed twice on one
Intel CPU; [arithmetic evidence](../../docs/reports/data/stage2/arithmetic-2026-10-22.json)
links the matching legacy regression. Injected read/write faults verify local
no-commit behavior; they do not establish native guest #PF delivery.

S2-12 adds 197 literal logic cases and validates dynamic semantic flags masks:
AF becomes undefined after logic/TEST, while NOT preserves definedness. Both
materializers pass `--check`; 518 v2 cases passed twice on one Intel CPU.
[Logic evidence](../../docs/reports/data/stage2/logic-2026-10-22.json) preserves
the manifests and scope separately from accepted S2-11 evidence. Mask updates
follow shared decoded FlagEffects after retirement, without a result evaluator.

## Dependencies

- `serde` 1.0.229 with `derive`: general data model serialization.
- `serde_json` 1.0.151 with default `std`: JSON input/output.
- `sha2` 0.10.9 with default `std`: SHA-256 digests.
- `od-isa` workspace path dependency: reuse the project's decoder to locate valid instruction boundaries while reducing fixtures; testkit does not copy decoder semantics.

All are general-purpose infrastructure dependencies. None implement CPU semantics. The resolved transitive graph is pinned in the workspace `Cargo.lock`.

S2-18c adds five handwritten schema v2 fixtures for bare `A5` MOVSD: three
mapped positive cases and two local-only injected access faults. Their literal
state and full data-memory answers are serialized by
`tests/cpu/generate_stage2_movsd.py`; the generator does not calculate x86
semantics. The five cases extend the deferred physical pool from 568 to 573.

S2-30by adds five exact `s2-long-x87-state-v1` FCMOVE streams with budgets
1/2/3/6/6: bare, status observation, repeat, and two FCOMI/FCHS producers.
Operand and environment rejection cases are restricted to bare `dac9`; prefixed
and neighboring DA encodings remain outside the schema. This is fixture-format
coverage only and does not claim native or physical x86 behavior.

S2-30cb admits exactly 19 FCMOVNE schema streams: each bare `DB C8..CF` form
alone and followed by FNSTSW, the repeated C9 stream, and two FCOMI producer
programs. Operand/environment rejects cover indexed source selection and
prefixes remain rejected. This schema allowlist is local fixture-format
coverage only; JIT/native and physical x86 behavior are not implied.

S2-30cc recorded the historical ST(1)-only `DA D1` FCMOVBE fixture schema.
S2-30cd admits exactly twenty streams: each bare `DA D0..D7` form alone and
followed by FNSTSW, plus the retained repeat and three FCOMI producer
sequences. The schema rejects prefixes, neighboring encodings, invalid
operands and forbidden FCW/FSW. It describes local fixture format only and
does not establish JIT, native or physical behavior.

S2-30ce adds exactly six `s2-long-x87-state-v1` FCMOVNBE streams for bare
`DB D1`, FNSTSW observation, repeat, and three FCOMI producer compositions.
Other DB D0..D7 forms, prefixes, and memory operands remain outside the
schema. Local fixture validation is not native or physical x86 evidence.

S2-30cg historically admitted only its exact five-instruction TEST producer
stream, with 512 success and 768 invalid class/tag interpreter snapshots.
S2-30ch admits exactly 18 FCMOVU streams: bare DA D8..DF and their FNSTSW
forms, the DA D9 repeat, and the retained producer. The schema still rejects
prefixes, neighboring encodings and all other sequences; these are local
fixture-format rules, not native or physical x86 evidence.
