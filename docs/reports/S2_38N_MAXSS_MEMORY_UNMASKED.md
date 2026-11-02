# S2-38n — MAXSS XMM0,m32 [RAX] with unmasked IM/DM

The local schema-v2 interpreter profile `s2-long-maxss-memory-unmasked-rax-v1`
admits only Long64 `F3 0F 5F 00`. Its control echo includes SSE, CR0.EM/TS/MP,
CR4.OSFXSR and CR4.OSXMMEXCPT, with CR0.MP fixed to one. The pre-existing
S2-37cy masked MAXSS memory path, S2-38f register MAXSS path and S2-38m MINSS
memory path remain distinct.

Execution follows full fetch, exact-form selection, SSE/EM/OSFXSR #UD before TS
#NM, state/MXCSR validation, AC=1 local Unsupported, checked canonical48 span
`[RAX,RAX+3]`, one read4 and then MAXSS classification. AC=0 allows unaligned
data. A bad span or failed read precedes FP event generation and preserves
architectural state and retirement.

The profile uses the MAXSS behavior documented for S2-38f: QNaN and SNaN in
either operand raise IE; subnormal operands raise DE. The raw result selects
source 2 for NaNs, equal values and signed-zero ties, preserving a source-2
SNaN payload. NaN+subnormal reports IE only with IM=DM=1; opening either mask
commits IE|DE on one arithmetic exit. A newly unmasked event updates sticky
bits, suppresses all of XMM0 and retirement, and chooses #XM or arithmetic #UD
through OSXMMEXCPT. Prior sticky IE/DE alone does not trap.

Direct ISA tests use an independent raw integer ordering oracle and cross
IM×DM×OSXMMEXCPT with QNaN/SNaN, both subnormal signs and operand orders,
mixed NaN/subnormal, finite/infinite/equal inputs, and signed zero. They also
cross all RC modes with 64 prior sticky histories and verify XMM0.high96,
XMM1, MXCSR, RIP and retirement. Fault tests cover wrong-form bytes before
disabled gates, SSE/EM/OSFXSR versus TS priority, fixed-MP validation, AC before
bad span, span overflow, truncated fetch, failed read with a QNaN source,
callback count and rollback. Runner tests verify the literal QNaN #XM result,
masked completion, strict schema/result echo, backend relabel rejection and
JIT/native refusal.

## Verification

Integrated main verification passed: combined `od-isa` (1,895 tests),
`od-testkit` (455), and `od-runner` (125 unit + 520 fixture tests);
`od-x86-runner` (8 library + 4 binary tests), workspace all-target offline
check, formatting, layout check, and `git diff --check`. The focused S2-38n
runner fixture verifies #XM, masked completion, strict controls/result echo,
JIT/native denial and backend relabel rejection; direct ISA tests cover the
arithmetic matrix and fault ordering described above. `graphify update .`
completed with 11,828 nodes, 25,656 edges and 376 communities. Independent
Sol/high review is clean.

Physical x86 execution was not run. ISSUE-025 remains open and concerns the
unrelated reciprocal tiny-result threshold. The profile does not claim complete
SSE/MMX support or guest exception-handler continuation.

Normative sources: Intel SDM Rev. 093, Vol. 2B `MAXSS`, pp. 4-13–4-14; Vol. 1
§§4.9.2, 11.5.2.1–.2, 11.5.3.2–.3 and Appendix D Table D-9; Vol. 3A Tables
7-8 and 16-1. See the [plan](../plans/STAGE_2.md#s2-38n-exact-long64-maxss-xmm0m32-rax-memory-profile-with-mixed-imdm-exit),
[CPU contract](../specs/CPU_CONTRACT.md#s2-38n--exact-maxss-unmasked-memory-exit-for-m32-rax),
[shared report](STAGE_2_REPORT.md#s2-38n--exact-maxss-unmasked-memory-profile-at-m32-rax),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-38n--maxss-xmm0m32-rax-with-unmasked-imdm).
