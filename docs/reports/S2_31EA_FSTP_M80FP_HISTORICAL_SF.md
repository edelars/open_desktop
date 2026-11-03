# S2-31ea: historical SF admission for raw FSTP m80fp

## Scope

The bounded interpreter admits historical x87 stack-fault status (SF) for the exact bare Long64 `DB 38` raw `FSTP m80fp [RAX]` path only when invalid-operation (IE) was already set. ES and B still reject the path. SF without IE rejects before a memory callback, including when the current source is an SNaN. The existing control-word, tag/source encoding, and full-range write checks remain in force.

An admitted store copies the source's ten bytes unchanged and raises no conversion flags. The existing C1 policy is retained. TOP, tag, x87 pointers, and RIP commit only after a complete successful write. The strict schema-v2 Unsupported predicate uses the same SF/IE rule. The bounded m32/m64 paths and native/JIT support are unchanged.

## Evidence

- Interpreter matrix: six admitted tag/class and raw-encoding pairs, both signs, all eight TOP values, four admitted FCWs, both C1 values, and every DE/ZE/OE/UE/PE history with fixed historical SF+IE (24,576 cases). The test compares the complete resulting `LongModeState`, memory bytes, and write callback.
- Refusal/rollback: SF-only SNaN, ES/B with SF+IE, invalid FCWs, tag/source mismatches with SF+IE, and write/range failures with SF+IE.
- Status sequence: repeated raw stores preserve SF+IE and ZE; FNSTSW observes them; FNCLEX clears them; a subsequent raw store succeeds with the cleared state.
- Fixture coverage: accepted SF+IE raw-copy plus FNSTSW fixture and SF-only SNaN Unsupported fixture; interpreter behavior is checked and native/JIT remain denied.
- Schema coverage: the raw DB38 Unsupported reason admits SF without IE but rejects SF+IE and ZE alone as reasons for Unsupported.

## Verification

Focused checks completed during implementation:

- `cargo test -p od-isa fstp_m80fp --offline`: 22 passed.
- `cargo test -p od-testkit --offline`: 455 passed.
- `cargo test -p od-runner --test fixture_v2 fstp_m80fp --offline`: 3 passed.
- `cargo test -p od-runner --test fixture_v2 x87_store_schema --offline`: 2 passed.
- Full `cargo test -p od-isa --offline`: 1,925 passed.
- Full `cargo test -p od-runner --test fixture_v2 --offline`: 534 passed.
- `cargo check --workspace --all-targets --offline` and `cargo fmt --all -- --check` passed.
- `graphify update .` completed.

`python3 tools/check_layout.py` reports the existing unmapped root `.git` pointer and two missing historical oracle manifests linked by `docs/reports/STAGE_2_REPORT.md`; it reports no issue in the files added for this slice. Independent Sol/high review found no remaining correctness or scope findings.

## Limits

This is a bounded local interpreter admission rule based on historical state. It does not implement x87 exception delivery, add SF producers, or claim physical-hardware conformance. Raw-copy admission remains restricted to the exact DB38 source path and does not expand m32/m64 behavior.
