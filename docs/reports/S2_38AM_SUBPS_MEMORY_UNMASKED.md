# S2-38am — SUBPS m128 unmasked memory profile

## Implemented

Added immutable schema-v2 interpreter-only profile
`s2-long-subps-memory-unmasked-rax-v1` for exact Long64 `0F 5C 00`
(`SUBPS XMM0,m128 [RAX]`). It uses its own
`subps_memory_rax_unmasked_controls` echo and preserves the S2-37dr masked
memory and register SUBPS profiles. It requires SSE bit 25, one executable
mapping, one read-only m128 mapping, budget one and stop RIP=entry+3.

The executor fetches all three bytes and checks the exact form before SSE,
CR0.EM, CR4.OSFXSR and CR0.TS gates; a noncanonical instruction-fetch span
faults `#GP(0)` first. It requires MXCSR supported bits `0xffbf`, reserved bits
clear, DAZ=FTZ=0 and OM=UM=1, while admitting every RC, IM/DM/ZM/PM mask and
prior sticky history. The operand requires 16-byte alignment independent of
AC. The checked `[RAX,RAX+15]` span must be canonical at both ends, followed by
exactly one read16.

All four lanes use pre-execution XMM0 and the actual memory RHS. The raw-u32
oracle classifies original operands before the subtraction sign transform,
selects the first original NaN when both are NaNs, and preserves/quietens the
selected value as required. SNaN and same-sign infinity subtraction raise IE;
opposite-sign infinities remain valid. NaN suppresses DE only in its own lane.
IE and DE aggregate before mask handling. An unmasked pre-event commits all
applicable new IE/DE flags and suppresses post flags, XMM0, RIP and retirement.
Otherwise post flags are aggregated and unmasked PE suppresses the whole
destination and retirement. Exact tiny results add no UE/PE. OSXMMEXCPT selects
`#XM` vector 19 or SIMD-generated `#UD` vector 6. JIT/native/C ABI/generic and
fragment routes remain closed.

## Verification

- The actual interpreter representative matrix ran **41,472** cases: all
  `6^4` four-lane event products × 16 IM/DM/ZM/PM masks × both OSXMMEXCPT
  mappings.
- The actual interpreter cross-lane/boundary matrix ran **393,216** cases:
  48 selected vectors × 4 RC modes × 64 old sticky histories × 16 masks × both
  delivery mappings. Together, the two independent-oracle matrices ran
  **434,688** actual interpreter cases using the memory RHS.
- Six literal fixtures cover IE, DE, PE, pre-event suppression of PE, successful
  prior-sticky preservation, and noncanonical-fetch precedence. Direct ISA
  coverage checks fetch/form/gate order, CPL0–3, alignment at AC=0/1, canonical
  span and read rollback, one-read behavior, unsupported register-form
  admission, and fault-state preservation.
- Runner checks reject altered destination/status/retirement/control echo/
  exception vector, non-interpreter backend claims, missing or unknown schema
  fields, non-exact encodings, writable source mappings, and reserved MXCSR
  bits. The six literal fixtures execute through the interpreter.
- `cargo test -p od-isa --offline --quiet`: **1,967 passed**.
- `cargo test -p od-testkit --offline --quiet`: **461 passed**.
- `cargo test -p od-runner --offline --quiet`: **152 unit tests and 564 fixture
  tests passed**.
- `cargo check --workspace --all-targets --offline`,
  `cargo fmt --all -- --check`, and `git diff --check` passed. `graphify update .`
  passed with 12,642 nodes, 27,363 edges, and 350 communities; semantic labels
  were not refreshed because no LLM key is configured.
- `python3 tools/check_layout.py` reported only the linked-worktree `.git` file
  and two pre-existing missing manifest links in `STAGE_2_REPORT.md`; it found
  no new S2-38am layout issue.
- Independent Sol/high review completed cleanly, including fetch-span
  precedence, schema/result validation, and the full representative and
  boundary matrices. The `S2-38am` plan row is complete.

No physical x86 acceptance is claimed. Sources: Intel SDM Rev. 093 Vol. 2B
SUBPS pp. 4-690–4-692; Vol. 2A §2.5.2/Table 2-19 p. 2-27; Vol. 1
§4.8.3.5/Table 4-8, §§4.9.2 and 11.5.3.1–3; Vol. 3A Event 19/Table 7-8.

See the [CPU contract](../specs/CPU_CONTRACT.md#s2-38am--exact-subps-memory-profile-with-unmasked-exceptions),
[stage plan](../plans/STAGE_2.md), and
[fixture notes](../../tests/fixtures/x86/stage2/README.md).
