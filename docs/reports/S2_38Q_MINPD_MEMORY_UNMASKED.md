# S2-38q — MINPD XMM0,m128 [RAX] with unmasked IM/DM exits

The dedicated schema-v2 profile `s2-long-minpd-memory-unmasked-rax-v1`
implements the exact Long64 encoding `66 0F 5D 00`. It remains separate from
masked memory S2-37db and register S2-38i. Admission requires SSE2, CR0.EM=0,
CR4.OSFXSR=1, and explicit CR0.TS/CR4.OSXMMEXCPT controls. The strict fixture
schema requires the unmasked MINPD controls and rejects the masked-memory
control block. Execution is interpreter-only; JIT/native, C ABI, generic, and
fragment paths remain closed.

The interpreter fetches all four instruction bytes, rejects other forms before
feature gates, checks SSE2/EM/OSFXSR before TS, admits supported MXCSR bits
`0xffbf` with DAZ/FTZ clear and ZE/OE/UE/PE masked, then validates mandatory
16-byte alignment and the canonical48 16-byte span before one all-or-error
read16. Address and read faults precede operand classification. Both lanes use
raw MINPD source-2 selection. IE and DE are classified across both lanes before
the unmasked decision. An arithmetic exit commits all raw sticky events while
preserving XMM0, RIP, and retirement. Masked completion suppresses DE only for
the lane that also raised IE.

The independent schema oracle uses raw integer bit classification and ordering
to validate the expected lane results, sticky state, selected exception,
retirement, and XMM/RIP atomicity. The handwritten fixture covers QNaN/IM
producing #XM while a second lane contributes DE. Tests also reject foreign
controls, wrong encodings, forged JIT/native result labels, missing control
echoes, and tampered sticky state.

## Verification

Focused checks passed: three ISA tests cover event aggregation and atomic exits,
512 RC/sticky-history/AC combinations, and exact-form/fetch/gate/alignment/
canonical-span/read-fault ordering. The focused runner fixture test passed,
including strict-schema and backend-tampering checks. Full affected suites
passed: `od-isa` (1,910), `od-testkit` (455), and `od-runner` (125 unit + 524
fixture tests). Workspace all-target offline check, formatting, diff check, and
Graphify refresh passed (11,946 nodes, 25,795 edges, 381 communities). A separate
Sol/high review found and verified fixes for exact-form-before-gate admission,
foreign masked-memory control rejection, and interpreter-only result validation;
the follow-up review was clean. Physical x86 acceptance is not claimed.
