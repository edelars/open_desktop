# S2-37de — SQRTPD m128 memory profile

Added the schema-v2 interpreter-only profile `s2-long-sqrtpd-memory-rax-v1` for the exact Long64 instruction `SQRTPD XMM0,m128 [RAX]`, bytes `66 0F 51 00`, length 4. This profile accepts only the exact memory ModRM and refuses other ModRM values (including register operands), extra prefixes, REX, SIB/displacement addressing, VEX/EVEX, and truncated forms; generic SQRTPD register decoding remains a separate profile. The profile gates on SSE2 CPUID EDX[26], CR0.EM, CR0.TS, and CR4.OSFXSR after a complete four-byte fetch: missing SSE2, EM, or !OSFXSR produces #UD; TS produces #NM. It requires XMM/MXCSR state. JIT and native execution remain denied.

The masked-only local MXCSR subset admits `mxcsr & !0xffbf == 0`, DAZ=0, FTZ=0, and all six exception masks set. Every RC mode and prior sticky-flag combination is accepted. Unmasked exceptions and #XM delivery are outside this profile.

The Type 2 legacy memory form requires 16-byte aligned RAX regardless of AC. Alignment is checked before `RAX+15`, then both endpoints are checked for canonical48 before any data access. A misaligned or noncanonical span raises #GP(0) without invoking the data callback. An aligned `u64::MAX-15` endpoint is representable and reaches the one 16-byte read callback; its final byte is `u64::MAX`. A memory read failure leaves CPU/SSE state and retirement unchanged.

After one successful read, the two binary64 lanes come only from memory; poisoned old XMM0 bytes do not affect the result. Each lane uses `Binary64::sqrt_raw_result(MXCSR.RC)`. Positive and negative zero preserve their signs, +infinity passes through, QNaN passes through, and SNaN is quieted with IE according to the existing helper policy. Negative nonzero non-NaNs, including negative subnormals and −infinity, produce the negative indefinite QNaN `0xfff8000000000000` with IE and no same-lane DE. Positive subnormals raise DE; inexact positive finite results raise PE. IE, DE, and PE are ORed across lanes and into prior MXCSR sticky flags. On success XMM0 is replaced in full; unrelated state is preserved and RIP advances by four.

The schema validator uses an independent integer-only `independent_sqrtpd` oracle, without host floating point or the production square-root helper. The handwritten fixture poisons the prior XMM0 and supplies memory lanes `[4,2]`. Runner checks cover four RC modes crossed with all 64 prior sticky histories, plus signed zeros, infinities, QNaNs and SNaNs with both signs/payloads in either lane, negative finite/subnormal/infinity, positive subnormal, and mixed-lane status. Literal checks include `[4,2]` with sqrt(2) rounding (`RN/RU = 0x3ff6a09e667f3bcd`; `RD/RZ = 0x3ff6a09e667f3bcc`), `[-1,min-subnormal]` yielding `[0xfff8000000000000,0x1e60000000000000]` with IE|DE, `[-min-subnormal,4]` with IE only, `[positive-subnormal 3,2]` with DE|PE, and `[positive-subnormal 3,SNaN]` with aggregate IE|DE|PE.

Focused decoder, executor, fixture-validator, CLI backend-denial, and tampering checks cover exact encoding, fetch-before-gate and gate-before-data order, AC-independent alignment, canonical span, read rollback, one read16 on success, full-XMM replacement, control/result/retirement comparison, and refusal by JIT/native. No physical x86 acceptance is claimed.

Normative references: Intel SDM Rev. 093, Vol. 2B, `SQRTPD`, pp. 4-661–4-663; Vol. 2A §2.5.2, Table 2-19; Vol. 1 §§11.5.3.1–.3; Vol. 3A, Table 7-8 and §16.1.3/Table 16-1. This report records the supplied revision/page references and does not claim a new PDF download or physical-hardware run.

## Local verification

- Focused exact decoder refusal: 1 passed.
- Focused executor gates, alignment/span, success one-read and read rollback: 1 passed.
- Focused SQRTPD schema/CLI runner matrix: 1 passed (256 RC/sticky combinations plus 19 literal lane vectors).
- `cargo test -p od-isa --offline`: 1,738 passed.
- `cargo test -p od-testkit --offline`: 454 passed.
- `cargo test -p od-runner --offline`: 112 unit tests and 486 fixture tests passed.
- `cargo check --workspace --all-targets --offline`: passed.
- `cargo fmt --all -- --check` and `git diff --check`: passed.
- Graphify code-only AST update: 10,918 nodes, 23,516 edges, 314 communities.
- `tools/check_layout.py` in the isolated worktree reports its `.git` worktree marker as an unmapped root file and two pre-existing links from `STAGE_2_REPORT.md` into unavailable `work/` manifests. The same layout check in the main worktree passes (48 required directories, 6 crate scaffolds, 14 instruction scopes); no layout files were changed here.
- Independent Sol/high code review passed after adding both `[3,SNaN]` and `[SNaN,3]` aggregate IE|DE|PE cases and correcting the profile boundary description. No review findings remain.
