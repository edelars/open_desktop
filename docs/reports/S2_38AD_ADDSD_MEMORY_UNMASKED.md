# S2-38ad — ADDSD m64 memory with unmasked exceptions

## Delivered

Added a distinct schema-v2 interpreter-only profile, `s2-long-addsd-memory-unmasked-rax-v1`, for exact Long64 `F2 0F 58 00` (`ADDSD XMM0,m64 [RAX]`). The profile has independent controls and control echo; SSE2 bit 26 and OS gates are checked before AC handling and memory access. AC with the gate open is reported as local Unsupported before span validation or a read. AC-clear access is unaligned and performs one read8. The schema requires a read-only m64 mapping, exact instruction bytes, one-instruction budget, canonical span, supported MXCSR `0xffbf`, reserved bits clear, DAZ/FTZ clear, and OM/UM set.

The interpreter records IE/DE before PE. An unmasked new IE/DE event raises #XM(19), or SIMD #UD(6) without OSXMMEXCPT, before post-event PE processing. Otherwise the operation records masked exception flags, uses MXCSR rounding control, and delivers unmasked PE after the result. SIMD exceptions preserve XMM0 and RIP and do not retire; success updates only XMM0 low64 and preserves high64. API tests cover fetch/gate priority, local AC refusal before span/read, unaligned read with AC clear, canonical-span and read-failure rollback, and successful execution at CPL0–3. Existing masked ADDSD dispatch remains a separate profile variant and retains its controls.

The independent testkit oracle validates complete result state and exceptions. A runner matrix covers 20 operand classes—including positive/negative QNaN and SNaN paired with a subnormal in both operand positions—× 4 RC values × 64 old sticky histories × 8 IM/DM/PM patterns × 2 OSXMMEXCPT settings (81,920 cases). The NaN/subnormal cross-product verifies that NaN handling suppresses DE independently of operand position and sign. Four hand-authored fixtures cover success, IE/#XM, DE/#UD and PE/#XM. Tests deny JIT/native backends and reject control echo, low/high XMM changes, MXCSR IE/DE/PE sticky-status changes, RIP, exception vector and #GP(0) error-code tampering, retirement, permissions and malformed MXCSR.

## Verification

- `cargo fmt --all` — passed.
- `cargo check -p od-testkit -p od-runner --all-targets --offline` — passed.
- `cargo test -p od-isa addsd_memory_rax_unmasked --offline` — 2 passed.
- `cargo test -p od-runner addsd_m64_unmasked_literal_fixtures_validate_ie_de_pe_and_denials --offline -- --nocapture` — 1 passed.
- `cargo test -p od-runner addsd_unmasked_memory_covers_rounding_sticky_masks_and_operand_classes --offline -- --nocapture` — 1 passed; 81,920 executions, 34.81 s.
- `cargo fmt --all -- --check` — passed.
- `cargo check --workspace --all-targets --offline` — passed without warnings after removing the test-only unused `mut`.
- `cargo test -p od-isa --offline -- --quiet` — 1,954 passed.
- `cargo test --workspace --offline --quiet` — passed: od-host 10, od-isa 1,954, od-machine 9, od-runner binary 135, runner fixture integration 558, od-testkit 456, od-x86-runner library 8 and binary 6; other crates/doc-tests had no tests.
- `graphify update .` — passed; graph regenerated to 12,402 nodes, 26,776 edges and 382 communities.

## Review follow-up

Sol/high review found two coverage gaps and no production-semantic defect. The matrix now includes signed QNaN/SNaN paired with a subnormal in both operand positions over every control combination; fixture tampering now covers low/high XMM, IE/DE/PE sticky status and #GP(0)'s error code. Focused fixture and matrix tests, formatting, affected-crate check and Graphify refresh passed after these additions. Independent Sol/high re-review is clean.

## Main integration

The profile and review-fix commits were transferred to main. Combined main checks
passed: `cargo fmt --all -- --check`, `cargo check --workspace --all-targets
--offline`, `python3 tools/check_layout.py`, and `git diff --check`. Directly
affected package suites passed: od-isa 1,956, od-testkit 456, runner 136 unit
and 559 fixture tests. Graphify after both slices: 12,382 nodes, 26,891 edges,
381 communities. A full-workspace test attempt exceeded the context executor's
300-second response limit without a captured final exit status; no physical x86
acceptance is claimed.

## Boundaries

Preserve S2-37dj masked-memory and S2-37bb register profiles. This slice does not add unmasked OE/UE delivery, native/JIT/C ABI support, generic or fragmented decoding, or physical x86 acceptance. No new known issue was confirmed during implementation.
