# S3-38c cached demand-flags NativeRegisterV1 candidate

## Scope

This slice adds the separately named opt-in
`run_cached_native_register_v1_demand_flags_candidate_controlled` entrypoint.
It shares the bounded `SingleEntryBlockCache` lifecycle with cached eager
NativeRegisterV1, while `S3_38cNativeRegisterV1DemandFlagsCachedCandidate`
keeps candidate and eager compiled artifacts in distinct cache keys. The
semantic profile remains `NativeRegisterV1Fixture`; pending descriptors and
operands are produced per execution and are not cached. Capacity, default
selection, S0/memory/multi-entry admissibility, and runtime policy are
unchanged. This is experimental correctness/reuse evidence only, with no
performance conclusion or default promotion.

## Focused evidence

- `cargo test -p od-jit --offline dispatch::tests::native_cached_demand_candidate_strategy_key_isolated -- --exact --test-threads=1` — passed. Eager → candidate → candidate → eager yields three compiles/publications, one hit and three misses. The cached candidate artifact contains the pending-flags inadmissible PC-map evidence.
- `cargo test -p od-jit --offline dispatch::tests::native_cached_demand_candidate_guards_and_lifetime -- --exact --test-threads=1` — passed. It covers STOP before lookup, candidate publication/execution failures, entry eviction and published handle release, and NOP interpreter handoff.
- `cargo test -p od-host --offline --lib tests::cached_native_register_v1_demand_candidate_cold_hit_full_state_parity -- --exact --test-threads=1` — passed. The candidate cold path matches the uncached candidate; a cache hit with a different initial RAX and RFLAGS matches the expected full state and confirms no additional compile/publication.
- `python3 -m unittest test_stage3_harness` from `tests/cpu/` — passed 31/31. The harness pins three additional exact gates; the total is 80.
- `cargo fmt --all -- --check` and `cargo check --workspace --all-targets --offline` — passed.
- `python3 tools/check_layout.py` — reports only existing ISSUE-042 (`.git` linked-worktree pointer) and ISSUE-043 (absent ignored Stage 1/2 manifests).

## Full local harness

Command: `python3 tests/cpu/stage3_harness.py --suite local`.

- Run `stage3-20261031-025712-17744b3142`: `local_only`; 84/84 selected steps passed, zero failed. The 117-step manifest contains 33 unselected/not-run steps. `local_lifecycle`, `local_acceptance`, and `memory_corpus` passed; Stage 1 dispatch is `local_only`.
- Host: macOS 26.7.1 ARM64, `aarch64-apple-darwin`; Python 3.15.0; Rust/Cargo 1.99.0.
- Run window: 2026-10-31 02:57:12–03:21:13 UTC.
- Source revision at start: `90f6386fd3931702dff30eb9807ab430c2af02de`. Captured source fingerprint matched at start and end: `ac1556a76b2d4ff6da099f669814598df9a37c76323a9a656354f3d86137a331`.
- Input-manifest SHA-256: `63288b371c0884684accc36026d0ece94d7cbfa25c4941ccdbb23e327839952b`. Summary SHA-256: `cf6215c2c0b3149bd5130c511cbaab49dc124452ee2178c48bd94140822803b6`.
- S3-38c gate log hashes: strategy-key isolation `c227d19185dfb7843d3c4ad12c77bba68b43455e2588a6d72feb238f63d8f5c0`; lifecycle/guards `03a9cf947bb66c0e21169ff31cbd1eade7d04f9f444bed7b0e275cbbdbba8249`; cold/hit parity `e0c0b48d45bead5b57f8d462b5e015f1ee810f8034267035b5c29889ff6f6d0d`.
- Evidence remains in ignored local files: summary (`work/stage3-harness/stage3-20261031-025712-17744b3142/summary.json`) and input manifest (`work/stage3-harness/stage3-20261031-025712-17744b3142/input_manifest.json`).

The report was updated after the harness completed to capture its result, so
the final evidence text is not part of the run's source fingerprint. The
fingerprint did match at both harness boundaries.

Mini, physical x86, performance, owner/MMU, Stage 2, and VMHost process suites remain deferred or not run for this slice.
