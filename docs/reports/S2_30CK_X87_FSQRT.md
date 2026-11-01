# S2-30ck — bounded bare FSQRT ST(0)

## Implemented slice

The Long64 interpreter accepts only exact unprefixed `D9 FA`. The existing x87
state is required, with FCS/FDS selector values absent as required by the
shared D8–DF admission gate. FCW must be one of the twelve all-exception-masked
values for PC24, PC53, or PC64 and the four rounding modes. `FSW & 0x80df` must
be zero; this permits historical PE and all TOP/C0/C2/C3 patterns.

The source is the Valid-tagged canonical positive nonzero Normal value in the
physical old TOP. Every other class, malformed representation, sign, zero or
tag is refused before commit. The interpreter maps PC and RC directly and
calls `sqrt_finite_to_binary80_precision_with_diagnostics` once at the target
precision. It writes only that physical register payload, ORs PE when
inexact, replaces C1 from `incremented`, and retains C0/C2/C3 under a
deterministic local policy. TOP, tags, FCW, FDP, FOP and unrelated state are
preserved; FIP is set to old RIP and RIP advances by two. The instruction
performs no data-memory access. Fetch, stop, budget and other failures are
atomic.

This profile assumes x87 availability, CR0.EM=CR0.TS=0, and no pending x87
exception. CPUID/FPU availability, EM/TS behavior, #NM/#MF, pending exception
delivery, exception flags/masked responses and general x87 exception behavior
are not implemented. Intel SDM Rev. 093 Vol. 1 §8.1.8 makes
CPUID.07H.00H:EBX[6] a condition on FDP (the data pointer), not FIP. Because
this is a register-only instruction, FDP is preserved under the local policy;
this is not a claim of hardware equality. C0/C2/C3 retention is also a local
policy for undefined fields. Native and JIT execution and physical x86
acceptance are not claimed.

## Coverage

- 1,536 full-state success snapshots: 12 FCWs × 8 TOP values × both old PE
  states × 8 C0/C2/C3 patterns, including canaries for unrelated registers,
  x87 pointers and raw stack cells.
- Independent literal binary80 results for sqrt(1), sqrt(4), sqrt(2), p24
  midpoint even/odd cases, adjacent p53 rounding cases, minimum and maximum
  normal exponent boundaries, p64 significand carry, and a direct p24 result
  that differs from p64-then-narrow.
- Atomic refusals for positive/negative zero, negative Normal, subnormal,
  pseudo-denormal, NaN, infinity, unnormal/malformed data and wrong tags;
  missing x87, all reserved PC=01 forms, each cleared exception mask, each
  forbidden FSW bit, prefixes, WAIT/FWAIT, D9 neighbors, truncation/fetch
  fault, STOP and zero instruction budget.
- JIT denial is covered by a fragment-JIT unit test. Native admission contains
  an explicit x87 deny case, but its Linux-only test could not run on this
  macOS host. Cross-target verification is blocked by pre-existing Linux
  runner compile errors documented in [ISSUE-029](../KNOWN_ISSUES.md#issue-029--linux-x86-runner-target-does-not-compile-all-tests).
- No schema or CLI fixture was added.

## Validation

- Focused `od-isa` FSQRT tests: 4/4 passed.
- Full `cargo test -p od-isa --offline`: 1,750 passed.
- Full `cargo test -p od-jit --offline`: 7 passed, including FSQRT denial.
- `cargo check --workspace --all-targets --offline`: passed on the configured
  `aarch64-apple-darwin` host.
- `cargo fmt --all -- --check`: passed.
- `tools/check_layout.py`: reports the known isolated-worktree `.git` pointer
  and two links from `STAGE_2_REPORT.md` to local oracle manifests absent from
  this worktree; the new S2-30ck links point to existing files.
- Native Linux admission test: not run. The same x86_64 Linux all-target
  blockers were reproduced on the untouched base commit `bf1230c` and are
  tracked in ISSUE-029.

## Sources

- Intel, *64 and IA-32 Architectures Software Developer’s Manual*, Rev. 093,
  Vol. 2A, `FSQRT` pp. 3-384–3-385:
  [official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf).
- Intel SDM Rev. 093, Vol. 1 §§8.1.5.2–8.1.5.3, pp. 8-7–8-8, and §8.1.8,
  pp. 8-9–8-10, for x87 state and instruction/data pointers:
  [official PDF](https://cdrdv2-public.intel.com/929351/253665-093-sdm-vol-1.pdf).

See the [plan](../plans/STAGE_2.md#s2-30ck-bare-register-only-fsqrt-st0-d9-fa),
[CPU contract](../specs/CPU_CONTRACT.md#local-s2-30ck-bounded-register-only-fsqrt-st0-d9-fa),
[shared Stage 2 report](STAGE_2_REPORT.md#s2-30ck--bounded-bare-fsqrt-st0), and
[ISSUE-019](../KNOWN_ISSUES.md#issue-019--семантика-арифметики-и-инструкций-x87-остаётся-открытой).
