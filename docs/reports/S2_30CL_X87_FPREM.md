# S2-30cl — bounded x87 FPREM

Implemented the exact unprefixed Long64 `D9 F8` register form in the `od-isa`
interpreter. Admission requires an existing x87 snapshot with absent FCS/FDS,
one of the twelve all-masked PC24/53/64 FCWs, `FSW & 0x80df == 0`, and Valid
canonical positive nonzero Normal operands at physical TOP and TOP+1.

The local input-only cutoff requires divisor exponent `e1 >= -16319` and
`D=e0-e1 < 64`, checked before invoking the shared mathematical helper. The
cutoff guarantees every nonzero result is normal or zero. Negative gaps retain
the exact raw ST(0) value and report quotient zero; complete nonnegative gaps
use the existing truncating helper and use the quotient's low three bits for
the FPREM condition codes. Exact multiples write canonical `+0` and the Zero
tag; nonzero results preserve Valid. C0=Q2, C3=Q1, C1=Q0, C2=0. Only old
physical TOP's payload/tag, FSW condition codes, FIP and RIP change. PE and
other exception bits, all other state, and memory are preserved. No data
memory access occurs.

The implementation covers 3,072 admitted snapshots across all 12 FCWs, eight
TOP values, both PE histories, and 16 initial condition-code patterns. Literal
raw vectors verify quotient bits 0–7 plus q=8 wrap, 5.5/2, exact-multiple
positive zero, D<0 passthrough, D=63 with quotient `2^63-1` and remainder
`2^-63`, and the minimum-normal result at `e1=-16319`. Atomic rejections cover
e1 below the cutoff including valid minimum-normal inputs whose mathematical
remainder is the minimum binary80 subnormal `2^-16445`, D=64/65, every
forbidden FSW field, invalid FCW, selectors, absent x87 state, participant
tags/classes and raw encodings. Decode checks cover only bare D9 F8, prefixes,
WAIT/FWAIT, neighboring D9 encodings and truncation; execution checks cover
callback STOP, stop-RIP, zero budget, one-instruction retirement, canaries and
fetch-only memory traces.

Validation passed: full `cargo test -p od-isa --offline` (1,753/1,753),
full `cargo test -p od-jit --offline` (8/8), `cargo fmt --all -- --check`,
`cargo check --workspace --all-targets --offline`, and `git diff --check`.
The focused FPREM decoder/executor selection passed 3/3 and the fragment-JIT
denial test passed 1/1. `graphify update .` completed with 11,046 nodes,
23,639 edges and 339 communities.

`python3 tools/check_layout.py` did not pass in this linked worktree: it reports
the `.git` worktree pointer and two absent historical oracle manifest links.
Both are the pre-existing checkout/report issue tracked in
[ISSUE-023](../KNOWN_ISSUES.md#issue-023--layout-checker-reports-worktree-metadata-and-stale-oracle-links); no layout was changed.

The native runner has a regression denial row for `D9 F8`, reusing the existing
x87 deny gate. It was not executable on this host: the Linux x86-64 all-target
check is blocked by the pre-existing snapshot/test dependency errors tracked
in [ISSUE-029](../KNOWN_ISSUES.md#issue-029--linux-x86-runner-target-does-not-compile-all-tests).
This report makes no native execution or physical x86 acceptance claim.
General x87 exception delivery, #NM/#MF, CPUID/CR0 gates, JIT/native support,
schema/CLI fixtures and physical acceptance remain outside scope; ISSUE-019
stays open.

Independent `gpt-6.1-sol` high review is clean. The reviewer confirmed the gap
and minimum-exponent denial rows reach their intended gates and the STOP and
one-retirement assertions are explicit. No further concrete defect was found.

Normative sources: Intel SDM Rev. 093, Vol. 2A, `FPREM` pp. 3-366–3-367; Vol. 1
§4.9.1.5 (underflow threshold) and §§8.1.5.2–8.1.5.3 (precision/rounding
control). See the [official Intel SDM page](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html).

See the [S2-30cl plan](../plans/STAGE_2.md#s2-30cl-bare-register-only-fprem-st0-st1-d9-f8),
[CPU contract](../specs/CPU_CONTRACT.md#local-s2-30cl-bounded-register-only-fprem-st0-st1-d9-f8),
and [shared Stage 2 report](STAGE_2_REPORT.md#s2-30cl--bounded-bare-fprem-st0-st1).
