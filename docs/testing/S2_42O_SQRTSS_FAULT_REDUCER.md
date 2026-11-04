# S2-42o — bounded SQRTSS SNaN fault reducer

## Scope

This slice adds one local interpreter reducer for schema-v2 profile
`s2-long-sqrtss-registers-v1`, Long64, and exact bytes `F3 0F 51 C1`
(`SQRTSS XMM0, XMM1`). It requires one RX code mapping, budget 1, stop at
`entry+4`, and expected architectural #XM vector 19. SSE must be available,
CR0.EM/TS clear, and CR4.OSFXSR/OSXMMEXCPT set. MXCSR uses only supported bits
`0xffbf`, DAZ/FTZ are clear, IM is clear, and the other exception masks are set.
The input XMM1.low32 is a positive binary32 SNaN with a nonzero 22-bit payload.

The reducer changes only payload bits, proposing set bits from 21 down to 0.
Sign, exponent, quiet bit, masks, sticky bits, code/mapping/masks, and all other
fixture fields stay fixed. It skips a proposal that would clear the final
payload bit and turn the value into infinity. The independent expected result
sets MXCSR.IE, preserves all other source state and prior sticky bits, and
expects zero retirement, unchanged RIP, #XM(19), no architectural error code,
and no fault address. Every candidate expects its current source payload.

## Callback predicate and bound

The reducer intentionally calls schema shape validation plus explicit fixture
digest/case/profile/backend checks. It does not call `validate_for`: that shared
validator compares the exact expected state and rejects the intended
XMM0/MXCSR discrepancy. The reducer checks controls, complete snapshots,
unchanged GPR/RFLAGS/RIP/code/XMM source/high bits/other XMM state, exact
exception metadata, build and machine provenance, and unchanged MXCSR except
IE. Its fingerprint lists only differing bit locations in XMM0.low32 and/or
MXCSR.IE. Any other mismatch, unsupported outcome, #UD/#NM, changed fault,
missing snapshot, timeout, invalid result, flaky pair, or callback error fails
closed.

Original, each candidate and final confirmation each run twice. The maximum is
48 callbacks: two initial, 22 paired payload proposals, and two final. The
runner saves full callback results and journals, fixture/result digests,
independent original/candidate/reduced expected states, provenance, source and
toolchain/build/machine metadata, and exact replay argv. Replay uses the
preserved input copy and a new output directory under
`work/stage2/s2-42o-sqrtss-fault/`.

## Invocation and evidence

```sh
cargo run --offline -p od-runner -- \
  --reduce-sqrtss-fault \
  tests/fixtures/x86/stage2/s2_42o_sqrtss_snan_fault.case.json \
  [--limit N] \
  [--output-dir work/stage2/s2-42o-sqrtss-fault/<run-id>]
```

The default callback budget is 48. Output must be a new child directory in the
stated work area. The literal fixture is expected to report
`no_initial_mismatch` on the local interpreter. Synthetic stable IE-omission
and XMM0 destination corruption exercise reduction and exact fingerprints;
strict-validator rejection, fault drift, flakiness, callback budget, and replay
are regression coverage. Synthetic defects are orchestration checks, not CPU
defect evidence. Native and physical execution remain `not_run`; broader FP
acceptance and parent S2-42 remain open.
