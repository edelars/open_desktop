# S2-38ai — unmasked DIVSS m32 memory profile

## Result

Implemented immutable schema-v2 interpreter-only profile
`s2-long-divss-memory-unmasked-rax-v1` for exact Long64 legacy `F3 0F 5E 00`
(`DIVSS XMM0,m32 [RAX]`). It requires SSE bit 25, completes full fetch before
gate faults, accepts CPL0–3, refuses AC before span/read work, validates
canonical48 `[RAX, RAX+3]`, and performs exactly one m32 read. AC=0 permits
unaligned RAX. Gate, span and read failures preserve state and retirement.

The profile uses its own immutable `divss_memory_rax_unmasked_controls` echo,
read-only data mapping, budget one and stop RIP=entry+4. It requires supported
MXCSR bits `0xffbf`, reserved bits clear, DAZ=FTZ=0 and OM=UM=1, while
permitting all RC modes, all 64 histories of the six sticky flags and every IM/DM/ZM/PM
mask combination. It preserves existing masked-memory S2-37do and register
S2-37ay profiles.

Execution reads the actual RHS once, then classifies IE before ZE before DE.
Original SNaN raises IE; QNaN alone does not. `0/0` and `∞/∞` raise IE;
finite or subnormal nonzero divided by signed zero raises ZE; `∞/0` does not.
NaN suppresses same-lane ZE/DE. Unmasked IE/ZE/DE commit generated pre-event
flags and suppress later OE/UE/PE, result, RIP and retirement. Otherwise raw
DIVSS computation uses MXCSR.RC, WPR tininess-after-rounding and PE handling;
unmasked PE suppresses destination/RIP/retirement. OSXMMEXCPT selects #XM
vector 19 or SIMD-generated #UD vector 6, both with no error code. Independent
expected values use the testkit's integer-only raw-u32 rational oracle and the
fixture's actual memory RHS.

## Coverage

- 221,184 actual interpreter executions: 27 event vectors × 4 RC × 64 prior
  sticky histories × 16 IM/DM/ZM/PM combinations × both OSXMMEXCPT values.
  Each result is checked by the independent oracle; this is runtime execution
  coverage, not a synthetic-validator-only matrix.
- Runtime cases include signed SNaN in both operand positions, QNaN-only and
  QNaN suppression of ZE/DE, `0/0`, `∞/∞`, finite/subnormal divided by signed
  zero, `∞/0`, zero/subnormal, `∞/subnormal`, other DE inputs, exact/inexact
  ratios including `1/3`, exact/inexact tiny outputs, half-minsub, a
  min-normal WPR boundary, overflow, unmasked pre-event suppression of later
  events, and masked pre-events followed by unmasked PE.
- Three handwritten literal fixtures exercise IE, ZE and PE delivery.
  Runner checks cover fixture/result/status/control/exception tampering,
  strict unknown-field rejection, JIT/native result denial, JIT compile denial,
  and the prior S2-37do read-only mapping contract.
- Direct executor probes cover full-fetch-before-gates, #UD/#NM gates, CPL0–3,
  AC-before-noncanonical-span, unaligned single read, canonical-span fault,
  #XM/#UD selection, pre- and post-event state suppression, and read-failure
  rollback.

## Validation and review

- Full package suites passed: `od-isa` 1,962; `od-testkit` 461;
  `od-runner` 141 unit tests and 562 fixture tests.
- Workspace all-target offline check, formatting, Graphify refresh (12,545
  nodes, 27,114 edges, 401 communities), and `git diff --check` passed.
- `tools/check_layout.py` reports the isolated worktree `.git` marker and two
  pre-existing aggregate `STAGE_2_REPORT.md` links to absent `work/` oracle
  manifests; no S2-38ai layout issue was reported.
- Independent Sol/high review found the fetch-span validator mismatch and
  provenance wording issues. Both were fixed, covered by regressions, and
  independently re-reviewed with no remaining findings.

This is local interpreter fixture evidence. It does not establish physical x86
acceptance. Native, JIT, C ABI, generic and fragment routes remain closed.
