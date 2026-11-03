# S2-31ef — historical SF with prior IE on exact bounded FILD forms

S2-31ef widens only the shared pre-read FILD environment gate for exact bare
Long64 `DF 00` (m16), `DB 00` (m32) and `DF 28` (m64). Historical SF is
accepted only when IE was already set, with ES/B still rejected. The FCW
allowlist, required x87 state, Empty physical push destination, exact typed
read and integer-only signed conversion are unchanged. Successful pushes
preserve SF and all six sticky flags, clear C1, and retain the existing
TOP/tag/payload/FIP/FDP/RIP commit behavior. FLD/FST gates are unchanged.

The independent literal matrix covers 30,720 states: three widths, zero, +1,
−1, signed minimum and signed maximum, all 32 combinations of DE/ZE/OE/UE/PE
with SF+IE, eight TOP values, four FCWs and both previous C1 values. A separate
S2-31ee matrix retains coverage of its SF=0 sticky histories. Additional ISA
checks cover SF-only/ES/B no-read rejection, canonical span faults, typed read
widths and complete state rollback with both C1 values. Four handwritten
schema-v2 fixtures cover all three widths and FILD/FNSTSW composition; runner
checks literal results and denies JIT/native backends. Unaligned source
addresses remain represented in the success matrix.

## Verification

- `cargo test -p od-isa --offline`: 1,941 passed
- `cargo test -p od-testkit --offline`: 455 passed
- `cargo test -p od-runner --offline`: 130 unit and 544 fixture tests passed
- `cargo check --workspace --all-targets --offline`: passed
- `cargo fmt --all -- --check` and `git diff --check`: passed
- `graphify update .`: 12,217 nodes, 26,351 edges and 382 communities
- `python3 tools/check_layout.py`: reports the linked worktree `.git` pointer
  as an unmapped root file and two historical Stage 2 report links into absent
  ignored `work/` manifests. No S2-31ef fixture or report link is broken.
- Independent code review: pending after commit

No physical x86 acceptance is claimed. The SF=>IE condition is a local
interpreter-profile admission rule.
