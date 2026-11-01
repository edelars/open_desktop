# S2-37df — masked-only Long64 SQRTSD XMM0,m64 [RAX]

## Scope

This isolated slice implements only the exact legacy encoding `F2 0F 51 00` (four bytes), exposed by schema-v2 profile `s2-long-sqrtsd-memory-rax-v1`. It requires Long64, SSE2 (`CPUID.01H:EDX[26]`), clear CR0.EM, CR4.OSFXSR=1 and XMM/MXCSR state. Full four-byte instruction fetch precedes the gates; missing SSE2, EM=1 or OSFXSR=0 produces #UD before TS=1 produces #NM. CPL0–3 are admitted.

The profile accepts only MXCSR bits in supported mask `0xffbf`, all six exception masks set, DAZ=FTZ=0, and any RC mode or prior sticky history. Unmasked exception delivery and #XM remain unsupported. AC=1 returns local Unsupported before address validation or a data callback. With AC=0, the scalar Type 3 form imposes no 16-byte alignment requirement, and an unaligned RAX is admitted. This is a profile boundary, not an architectural #AC result.

Before reading, the interpreter checks `RAX + 7` for overflow and verifies both endpoints of `[RAX, RAX+7]` are canonical 48-bit addresses. A successful execution issues exactly one `read8`. A test address at `u64::MAX-7` is canonical through `u64::MAX` and reaches one read8 callback; the local test memory rejects that unmapped address. Overflow and noncanonical spans are rejected before any callback. A read failure leaves all CPU/SSE state, MXCSR, RIP and retirement unchanged.

The result uses only the memory m64 source and `Binary64::sqrt_raw_result(MXCSR.RC)`. It replaces XMM0.low64, preserves XMM0.high64 and unrelated CPU state, and advances RIP by four. Signed zero and +infinity pass through; negative nonzero values, including negative subnormal and −infinity, yield `0xfff8000000000000` with IE only; SNaN is quieted and raises IE; QNaN passes; positive subnormal raises DE and may also raise PE; inexact square root raises PE. Newly generated IE/DE/PE bits are ORed into prior sticky flags.

## Independent checking and coverage

The strict schema-v2 validator independently computes the scalar result with integer raw-bit arithmetic through the one-lane SQRT operation oracle; it does not call the production `Binary64::sqrt_raw_result` helper or host floating point. The literal runner fixture uses bytes `f2 0f 51 00`, a poisoned XMM0 low half, an XMM0.high64 canary and an unaligned source address.

Focused tests cover exact decode and malformed forms, fetch-before-gate order, all SSE2/EM/OSFXSR/TS combinations at CPL0–3, absent XMM/MXCSR state, AC=1 local refusal, AC=0 unaligned success, canonical endpoint and overflow refusal, one read8, read rollback, high64 preservation, all RC modes and old sticky flags, raw-u64 values for 4, 0.25, sqrt(2), subnormals 1/2/3, signed zeros, infinities, negative inputs and signed QNaN/SNaN, strict metadata/control/result tampering (including fabricated fault address and faulting RIP for #UD/#NM/#GP), and JIT/native denial.

## Evidence and limits

Focused results: `od-isa` 1740 tests passed; `od-testkit` 454 passed; `od-runner` 112 unit tests and 487 schema-v2 fixture tests passed. `cargo check --workspace --all-targets --offline`, `cargo fmt --all -- --check`, and `git diff --check` passed. Graphify was refreshed after source edits (10,985 nodes, 23,522 edges, 328 communities).

`python3 tools/check_layout.py` was also run in the isolated worktree. It reports the worktree `.git` marker as an unmapped root file and two existing `STAGE_2_REPORT.md` links into absent `work/` manifests (`stage1-oracle` and `stage2-oracle`). This is a worktree validation caveat; no project layout files were changed. Independent Sol/high review passed after one finding was fixed: result validation now rejects fabricated fault addresses and faulting RIPs for #UD/#NM/#GP; the corresponding tampering cases pass. The change does not claim physical x86 acceptance. Native, JIT, C ABI, generic interpreter and fragment admission remain closed; unmasked/#XM behavior and other encodings/addressing forms are out of scope.

Sources: Intel SDM Rev. 093 Vol. 2A §2.5.3/Table 2-20; Vol. 2B SQRTSD pp. 4-667–4-668; Vol. 1 §§11.5.2.1–.2 and 11.5.3.1–.3; Vol. 3A Event 19 and Table 7-8.
