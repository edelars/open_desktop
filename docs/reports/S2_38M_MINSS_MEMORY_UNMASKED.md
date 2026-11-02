# S2-38m — MINSS XMM0,m32 [RAX] with unmasked IM/DM

The local interpreter profile `s2-long-minss-memory-unmasked-rax-v1` adds an
unmasked memory source for exact Long64 `F3 0F 5D 00`. The existing masked
`s2-long-minss-memory-rax-v1` profile remains unchanged. Admission requires
SSE bit 25, CR0.EM=0, fixed CR0.MP=1, CR4.OSFXSR=1, explicit CR0.TS and
CR4.OSXMMEXCPT controls, and present XMM/MXCSR state. The strict schema echoes
all five immutable controls. Other encodings, addressing forms, and JIT,
native, C ABI, generic and fragment paths remain closed.

The local order is stop/budget, full fetch, exact-form and feature gates,
MXCSR/state admission, AC=1 local Unsupported, checked `RAX+3` and canonical48
endpoints, exactly one read4, then operand classification and result. SSE
absence, CR0.EM or OSFXSR failures produce #UD before TS produces #NM. AC=0
allows unaligned m32. Address and read failures happen before FP classification;
they create no new IE/DE and preserve CPU state and retirement.

MXCSR admits supported bits `0xffbf`, requires reserved bits and DAZ/FTZ clear,
and requires ZM/OM/UM/PM set. IM and DM independently accept either value; all
RC modes and prior sticky histories are preserved. After a successful read,
the S2-38d mixed IM/DM MINSS policy applies. NaNs set IE. A subnormal operand
sets DE unless the both-masked invalid-priority case suppresses it. NaN plus
subnormal with IM=DM=1 sets IE only and completes; if either mask is clear,
IE|DE are committed and the instruction makes one arithmetic exit. A new
unmasked event commits sticky flags, suppresses all of XMM0, and leaves RIP and
retirement unchanged. OSXMMEXCPT selects #XM or arithmetic #UD. Prior IE/DE
alone does not trap. Success changes only XMM0.low32 and new sticky bits;
numeric ties, signed-zero ties and NaNs select raw RHS, including an unquieted
SNaN payload.

The handwritten fixture covers QNaN with an unmasked IE/#XM result. An
independent raw-integer MINSS oracle validates schema-v2 results and does not
call production floating-point helpers. Direct ISA tests cross IM×DM×OSXMMEXCPT
over QNaN/SNaN in both operands, both subnormal signs, mixed NaN/subnormal in
both orders, signed zero and finite values; they also cross all four RC modes
with all 64 old sticky histories. Fault tests cover AC plus a noncanonical
address, span overflow, feature-gate priority, truncated fetch, failed read
with an unmasked NaN, and a completed read followed by an arithmetic exit.
Assertions verify zero or one data callbacks, atomicity, XMM canaries, sticky
history, retirement and backend denial. Result-control tampering is rejected.

**Verification:** focused ISA checks passed (2 tests) and the focused literal
runner check passed (1 test). Full affected suites passed: `od-isa` (1,889),
`od-testkit` (455), and runner `fixture_v2` (517). Workspace check, formatting,
and `git diff --check` passed; the workspace all-target offline check took
9.74 s. `graphify update .` refreshed the worktree graph to 11,829 nodes,
25,501 edges and 384 communities. `tools/check_layout.py` returned exactly the
three ISSUE-023 errors: `Unmapped root file: .git` (the isolated worktree's
65-byte `.git` pointer file), and broken local links from
`docs/reports/STAGE_2_REPORT.md` to
`../../work/stage2-oracle/stage2-20261022-230853-3684f650/manifest.json` and
`../../work/stage1-oracle/stage1-20261022-230241-3b73e810/manifest.json`.
Independent Sol/high review is pending after the author commit. No physical
x86 acceptance, complete SSE/MMX support, or guest exception-handler
continuation is claimed. ISSUE-025 remains open and concerns the unrelated
RCPSS/RCPPS tiny-result threshold.

See the [Stage 2 plan](../plans/STAGE_2.md#s2-38m-exact-long64-minss-xmm0m32-rax-memory-profile-with-mixed-imdm-exit),
[CPU contract](../specs/CPU_CONTRACT.md#s2-38m--exact-minss-unmasked-memory-exit-for-m32-rax),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-38m--minss-xmm0m32-rax-with-unmasked-imdm).

Normative sources: Intel SDM Rev. 093, Vol. 2B `MINSS` pp. 4-24–4-25; Vol. 1
§4.9.2 and §§11.5.2.1–.2, 11.5.3.2–.3; Vol. 3A Tables 7-8 and 16-1.
