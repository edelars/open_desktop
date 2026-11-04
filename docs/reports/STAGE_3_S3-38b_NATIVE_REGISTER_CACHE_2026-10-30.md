# S3-38 bounded NativeRegisterV1 cache slice

## Scope

Added an opt-in `run_cached_native_register_v1_controlled` entrypoint backed
by the existing one-entry `SingleEntryBlockCache`. It compiles the composed
NativeRegisterV1 profile eagerly on miss and retains the published host handle
across calls. The demand-flags candidate and default runtime are unchanged.

The cache key carries both a distinct NativeRegisterV1 semantic profile and
compile flavor. It also retains the full fixture `CodeView`, block/source
metadata, caller dependency identities and versions, and frozen execution
context. Capacity remains one and uses the existing 512-KiB logical bound.
Entry replacement releases the prior published handle before new publication;
STOP and event are rechecked after publication and immediately before entry.
Exact bare NOP remains an interpreter step with normal budget/retirement
accounting. This slice provides no owner/MMU invalidation, SMC, persistent
VMHost/session cache, Mini, or performance claim.

## Focused evidence

- `cargo test -p od-jit --offline native_cached_ -- --test-threads=1` — 2/2
  focused dispatch tests passed. They cover profile/key isolation, source,
  dependency and context changes, capacity-one replacement, STOP/event guard
  priority, failed publication and execution, and handle release after an
  execution error.
- `cargo test -p od-host --offline --lib tests::cached_native_register_v1_reuses_eager_block_with_full_state_parity -- --exact --test-threads=1` — passed. It compares full CPU state and retirement against the interpreter and uncached eager NativeRegisterV1, verifies RIP-relative LEA on both hit and generation-triggered recompile, confirms a repeat hit without another compile/publication, checks backward-branch loop reuse, partial-budget exhaustion, a STOP boundary, and NOP interpreter handoff with capacity-one handle release.
- Harness gate manifest selfcheck — passed with 77 exact Rust gate records and unique gate step names (74 existing plus 3 new S3-38 gates); the complete harness Python selfchecks passed 31/31. Some pre-existing step names intentionally share a Rust test identity.
- `cargo fmt --all -- --check` and `cargo check --workspace --all-targets --offline` — passed.
- `python3 tools/check_layout.py` — reported only already registered ISSUE-042 (the linked-worktree `.git` pointer appears as an unmapped root file) and ISSUE-043 (two links in the Stage 2 report point to absent ignored oracle manifests under `work/`). Neither issue is part of this cache slice; no Stage 2 evidence was changed.

## Full local harness

Command: `python3 tests/cpu/stage3_harness.py --suite local`.

- Run: `stage3-20261031-020824-b7d49a25ce`; status `local_only`; 81/81
  executed steps passed, zero failed. The 114-step manifest includes 33
  unselected/not-run steps. `local_lifecycle`, `local_acceptance`, and
  `memory_corpus` passed; Stage 1 dispatch is `local_only`.
- Host: macOS 26.7.1 ARM64, `aarch64-apple-darwin`; Python 3.15.0;
  Rust/Cargo 1.99.0.
- Run window: 2026-10-31 02:08:24–02:30:59 UTC.
- Source revision at start: `974290a20673dc22f12a8ea1c7a47f0b7203acad`.
  Harness source fingerprint matched at start and end:
  `605163f73904c6097c39d884a1ff8bb8dd7e92e415f9a25acc2453a1d7132117`.
- Input-manifest SHA-256:
  `5e1a440db4d2489ed6b8605488295bbca2e21641c131b9e81865ad271b859376`.
  Summary SHA-256:
  `a9a391c53a5f19f33c7ae76f47bcb297dab0115fa388a39e3505affc08c75347`.
- New exact-gate log hashes:
  - Profile/dependency/source/context: `5f32aa650d1fae71c3405a0766af0d9617735549ca9f8a84a854bc161653b336`.
  - Guard/capacity/failure lifecycle: `9e946a3614466939b307e744b964452d815ddc59894347750b783febfbdd0a17`.
  - Host reuse/lifetime/parity: `6bc1a6bb604b1a07c64fbb5f026d4c701adb800d3389ff0941491fda17f99d1b`.
- Evidence remains in ignored local files: [summary](../../work/stage3-harness/stage3-20261031-020824-b7d49a25ce/summary.json) and [input manifest](../../work/stage3-harness/stage3-20261031-020824-b7d49a25ce/input_manifest.json). Mini, physical x86, performance, owner-backed, Stage 2, and VMHost process suites remain `not_run`.

The report was finalized after the harness run to record its result, so its
final evidence text is not part of the run's captured source fingerprint. The
fingerprint did match at both harness boundaries. The final Graphify refresh
after the run found no code-graph topology changes and left the graph outputs
untouched.
