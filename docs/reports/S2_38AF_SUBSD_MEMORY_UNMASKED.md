# S2-38af — SUBSD m64 memory with unmasked exceptions

## Delivered

Added the immutable schema-v2 interpreter-only profile `s2-long-subsd-memory-unmasked-rax-v1` for exact Long64 `F2 0F 5C 00` (`SUBSD XMM0,m64 [RAX]`). It has a separate control id and result echo. SSE2 bit 26 and OS gates precede AC handling and memory access. With AC set, the profile returns local Unsupported before span validation/read; with AC clear it permits unaligned access and performs exactly one read8. The schema requires one executable code mapping, one read-only m64 data mapping, budget one, stop RIP entry+4, canonical span, MXCSR supported mask `0xffbf`, reserved bits clear, DAZ/FTZ clear and OM/UM set. Every RC, IM/DM/ZM/PM combination and old sticky history is admitted. S2-37dl masked memory and S2-37ap register profiles remain separate.

The interpreter classifies original raw operands before computation. It selects the first original NaN, quiets a selected SNaN, suppresses DE when either operand is NaN, raises IE for SNaN or same-sign infinity subtraction, and uses the subtraction indefinite for invalid infinity pairs. Unmasked IE/DE commits applicable IE/DE status then suppresses PE, XMM0, RIP and retirement. Otherwise binary64 subtraction uses MXCSR.RC; unmasked PE commits status and suppresses the destination and retirement. OSXMMEXCPT maps SIMD delivery to #XM(19), or arithmetic #UD(6) when clear. Success changes low64 only and preserves high64 and unrelated state.

The direct interpreter matrix contains 22 raw operand vectors × 4 RC modes × 64 old sticky histories × 16 IM/DM/ZM/PM patterns × 2 OSXMMEXCPT settings (180,224 runs). It includes signed QNaN/SNaN in both operand positions, NaN+subnormal, invalid and valid infinity subtraction, finite rounding, signed zero, exact tiny and overflow values. The independent testkit raw-u64 oracle uses the actual fixture memory RHS and validates complete state, status, retirement and exception outcomes. Four literal fixtures cover success, IE/#XM, DE/#UD and PE/#XM. Focused checks cover STOP and zero-budget suppression before fetch/read, gate priority, AC-before-read, canonical-span #GP(0), one-read behavior and read-failure rollback. Runner tests deny JIT/native backends and reject control echo, low/high XMM, MXCSR status, RIP, exception vector, #GP error-code and retirement tampering, as well as writable mappings and missing controls. A regression test preserves the pre-existing masked S2-37dl schema and execution behavior for a writable data mapping while keeping S2-38af writable mappings invalid.

## Verification

- `cargo fmt --all -- --check` — passed.
- `git diff --check` — passed.
- `cargo check --workspace --all-targets --offline` — passed.
- `cargo test -p od-isa --offline` — 1,957 passed, including gate/span/read rollback tests.
- `cargo test -p od-testkit --offline` — 458 passed, including the independent 180,224-case raw-u64 matrix.
- `cargo test -p od-runner --offline --quiet` — 136 unit and 560 fixture tests passed.
- `graphify update .` — passed; graph has 12,457 nodes, 26,895 edges and 397 communities.

Independent Sol/high review is pending. Physical x86 execution was not run and is not claimed.

## Boundaries

Unmasked OE/UE delivery, native/JIT/C ABI execution, generic/fragment decoding, and physical x86 acceptance remain outside this slice. No new confirmed known issue was found during implementation.
