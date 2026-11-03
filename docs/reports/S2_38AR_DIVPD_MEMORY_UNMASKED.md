# S2-38ar — DIVPD memory unmasked profile

Implemented exact Long64 legacy `66 0F 5E 00` (`DIVPD XMM0,m128 [RAX]`) as
schema-v2 profile `s2-long-divpd-memory-unmasked-rax-v1`. The slice is
interpreter-only and keeps the masked m128 and register profiles separate.

The profile requires SSE2 CPUID bit 26, CR0.EM=0, CR4.OSFXSR=1 and CR0.TS=0.
It checks the complete four-byte instruction before feature-gate delivery,
using the inclusive final byte (`RIP+3`) for the canonical-span decision. The
literal at RIP `0x0000_7fff_ffff_fffd` faults with #GP(0)
before disabled SSE2/CR0.TS delivery; RIP `0x7ffffffffffc` has a canonical
four-byte span and proceeds to #UD from those gates. Fetch faults do not
generate SIMD status or update architectural state. The profile requires
supported MXCSR mask `0xffbf`, DAZ=FTZ=0 and OM=UM=1, and admits all
RC, IM/DM/ZM/PM, and prior sticky settings. The memory operand requires
16-byte alignment independent of AC, checked canonical48 `[RAX,RAX+15]`, and
one read16. Address and memory failures preserve state and retirement.

The two lanes use XMM0 and bytes read from the actual memory RHS. Each lane
uses IE→ZE→DE priority, including SNaN IE despite QNaN selection; only after
lane classification are surviving IE/ZE/DE flags ORed across lanes. An
unmasked aggregate pre-event commits those status flags and suppresses
post-event status, destination, RIP and retirement. An unmasked PE commits
applicable status and suppresses the whole destination. The expected-result
oracle uses integer-only raw-u64 rational arithmetic.

Checked-in literal cases cover success, IE, ZE, DE, PE, UE+PE and SSE2 gate
precedence. Runner tests cover cross-lane IE+ZE, IE+DE and ZE+DE, same-lane
SNaN/zero precedence, masked IE followed by unmasked PE, immutable-control
and result tampering, and interpreter-only backend denial. A full
4 RC ×64 sticky histories ×16 IM/DM/ZM/PM settings ×2 OSXMMEXCPT delivery
matrix runs against memory-backed `1/3` and exact `1/1` lanes (8,192 cases).
Eight independent quotient/event boundary vectors run the same product
(65,536 cases): exact subnormal, inexact tiny, min-normal vicinity,
half-minsub, overflow+PE, large exponent gap, infinity/zero without ZE, and
finite/zero with ZE. Two further 8,192-case witnesses cover directed rounding
to min-normal and the exact WPR half-minsub tie; the latter asserts RN/RU
min-normal, RD/RZ max-subnormal and UE+PE for every RC. Direct ISA coverage
checks aggregate IE+ZE, atomic PE delivery, one read16, alignment/AC,
canonical-address rejection, read rollback and CPL0–3.

## Verification

- Direct ISA test `divpd_memory_rax_unmasked_aggregates_pre_events_and_suppresses_destination` passed. It checks cross-lane IE+ZE, atomic PE, one read16, alignment with AC both clear/set, noncanonical address rejection, read rollback and CPL0–3.
- Runner fixture test `divpd_m128_unmasked_memory_commits_events_atomically_and_keeps_lane_precedence` passed: 8,192 RC/sticky/mask/delivery combinations, cross-lane IE+ZE/IE+DE/ZE+DE, same-lane SNaN+zero priority, masked IE with unmasked PE, literal fetch-span/event/gate cases, wrong-vector tampering and backend denial.
- Boundary product passed: 65,536 cases over eight vectors. Two extra all-control matrices passed (8,192 cases each) for directed rounding across min-normal and the WPR half-minsub tie. The exact tie asserts UE+PE for every RC; RN/RU produce min-normal, RD/RZ max-subnormal. Total interpreter comparisons: 90,112.
- Full affected suites passed: od-isa 1,972; od-runner unit tests 159; `fixture_v2` 570; od-testkit 461.
- `cargo check --workspace --all-targets --offline`, `cargo fmt --all -- --check`, and `git diff --check` passed. `graphify update .` completed (12,757 nodes, 29,508 edges, 393 communities).
- `python3 tools/check_layout.py` reports the isolated worktree `.git` marker and two existing broken links from `docs/reports/STAGE_2_REPORT.md` to historical oracle manifests. No source-layout violation from this slice was reported.
- Independent Sol/high review passed cleanly, including the `RIP+3` fetch boundary, fffc/fffd gate behavior, literal validation, WPR half-minsub rounding and result tampering.

No native/JIT/C ABI/generic/fragment route or physical x86 acceptance is
claimed.
