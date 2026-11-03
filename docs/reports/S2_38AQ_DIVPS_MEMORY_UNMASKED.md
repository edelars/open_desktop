# S2-38aq — unmasked DIVPS m128 memory profile

## Scope

Implemented a separate schema-v2 interpreter profile
`s2-long-divps-memory-unmasked-rax-v1` for exact Long64 `0F 5E 00`
(`DIVPS XMM0,m128 [RAX]`). It requires SSE bit 25, one read-only m128 RHS
mapping, budget one, and stop RIP at entry+3. Existing masked-memory S2-37dt,
register S2-37aw, and scalar DIVSS profiles remain distinct. Native and JIT
execution are denied for this profile.

The profile requires MXCSR mask `0xffbf`, reserved bits clear, `DAZ=FTZ=0`,
and `OM=UM=1`. All four RC modes, prior sticky histories, IM/DM/ZM/PM masks,
and OSXMMEXCPT delivery are admitted through a separate five-control snapshot.
Instruction fetch and exact-form admission precede the SSE/CR0/CR4 gates;
CR0.TS follows #UD gates. The data operand must be 16-byte aligned regardless
of AC and its complete span must be canonical. A successful instruction makes
exactly one read16 from the actual RAX mapping.

## Arithmetic and exception behavior

The runtime and independent raw-u32 rational oracle classify original lane
operands before computing a quotient. Each lane settles NaN suppression and
its IE→ZE→DE candidates first; surviving pre-event flags are then ORed across
lanes. Any original SNaN raises IE, including when DIVPS selects a QNaN from
the other operand. QNaN-only lanes do not raise IE. `0/0` and `∞/∞` raise IE;
finite nonzero or subnormal numerators divided by signed zero raise ZE, while
`∞/0` does not. A NaN suppresses that lane's ZE and DE candidates.

An unmasked aggregate pre-event commits all applicable IE/ZE/DE status and
suppresses OE/UE/PE, XMM0, RIP, and retirement. With pre-events masked, the
integer-only raw-u32 oracle uses actual fixture memory bytes and checks
rounding, WPR tininess, underflow and precision status. Exact tiny results do
not raise UE; inexact tiny results raise UE+PE. The minimum-normal witnesses
are distinct: `0x00800000 / 0x3f800001` rounds to max-subnormal with UE+PE under
RN, while `0x3fffffff / 0x7f000000` is exactly halfway in WPR and rounds to
min-normal under RN/RU and max-subnormal under RD/RZ, with UE+PE in every RC
mode. Overflow produces OE+PE. An unmasked PE commits applicable status but
suppresses the full XMM0 result, RIP and retirement. OSXMMEXCPT selects #XM
vector 19 or SIMD-generated #UD vector 6.

Seven handwritten arithmetic literals cover SNaN IE, finite/zero ZE,
cross-lane IE+ZE, cross-lane ZE+DE, masked IE followed by unmasked PE, QNaN
suppression of same-lane ZE, and unmasked PE on an inexact tiny result. Four
more literals cover the SSE #UD gate, misaligned RAX, noncanonical data span,
and noncanonical fetch span. Runner checks also
cover `0/0`, `∞/∞`, subnormal/negative-zero, `∞/0`, exact tiny, half-minsub,
the minimum-normal boundaries, overflow+PE and a large exponent gap. Result,
status, exception, control echo, destination, retirement, and backend
tampering are rejected.

The direct ISA control matrix executes 73,728 DIVPS cases: one four-lane
IE+ZE+DE+PE vector plus eight boundary vectors, each across 4 RC modes, 64
prior sticky states, 16 IM/DM/ZM/PM combinations, and both OSXMMEXCPT values.
This matrix checks control/event behavior against the interpreter's arithmetic
helper; it is reported separately and does not count as independent-oracle
coverage.

A second in-process runner matrix executes the same 73,728 vector/control
product through the actual fixture interpreter. `RunResult::validate_for`
compares each complete state, MXCSR status, exception delivery, destination,
RIP and retirement against testkit's independent integer-only rational
`independent_divps` oracle using the actual memory RHS. The product contains
the four-lane IE+ZE+DE+PE event vector and eight boundary vectors, including
the WPR-tiny halfway that rounds to minimum-normal under RN/RU and max-subnormal
under RD/RZ. Three additional oracle-validated directed cases cover a positive
SNaN in XMM0, a negative SNaN in memory, and a QNaN-selected result with an
original SNaN operand still raising IE. These directed cases are outside the
73,728 Cartesian product.

## Validation

Passed in the isolated worktree:

- `cargo test -p od-isa --offline`: 1,973 passed.
- `cargo test -p od-testkit --offline`: 461 passed.
- `cargo test -p od-runner --bin od-runner --offline`: 160 unit tests passed.
- `cargo test -p od-runner --test fixture_v2 --offline`: 568 fixture tests passed.
- The `od-runner` unit test
  `divps_memory_unmasked_full_controls_match_independent_rational_oracle`
  passes 73,728 actual-interpreter comparisons plus three directed NaN cases.
- `cargo check --workspace --all-targets --offline`.
- `cargo fmt --all -- --check`.
- `graphify update .` refreshed the worktree graph to 12,753 nodes.

Independent Sol/high re-review passed, including the oracle-backed 73,728-case
runner product and directed SNaN/QNaN cases. `python3 tools/check_layout.py`
still exits 1 in this linked worktree because the worktree `.git` pointer is
unmapped and `STAGE_2_REPORT.md` contains two links to ignored `work/`
manifests; it reports no S2-38aq path mapping issue. This report records
host-side interpreter evidence only and does not claim physical x86
acceptance.
