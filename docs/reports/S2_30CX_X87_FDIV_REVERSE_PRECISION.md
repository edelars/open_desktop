# S2-30cx — bounded reverse x87 `FDIV ST(i), ST(0)`

Added FCW precision execution for exact bare Long64 `DC F8..FF`. The existing
bounded reverse-division helper computes old `ST(i)/ST(0)` directly at PC24,
PC53 or PC64 with the selected RC mode. Division does not use an exponent-gap
cutoff. The profile requires one of twelve all-masked FCWs, no saved instruction
or data selectors, `FSW & 0x80df == 0`, Valid tags for used operands, canonical
nonzero Normal binary80 inputs and a finite Normal result. The alias `i=0`
validates only old ST(0) and yields exact +1.

Success writes only old physical `(TOP+i)&7`, does not pop, preserves TOP,
tags, sources and unrelated state, makes PE sticky, replaces C1 from the
helper's `incremented`, records FIP, advances RIP by two and retires once.
Refusals are atomic. The former exact-only S2-30v p64 result domain remains
supported, including exact quotients whose operands have different exponents.
Other division directions, pop forms, general x87 exception delivery and
native/JIT acceptance remain out of scope.

Focused tests passed 5/5. They cover every TOP/index pair across twelve FCWs,
four sign combinations and prior PE/C1, literal 1/3 results at all three
precisions and rounding modes, exact p64 compatibility through the previous
exact helper, alias +1 with one Valid tag, invalid control/status/selectors,
tags, zero divisor, malformed inputs, non-Normal results, and fetch/STOP/budget
atomicity. A quotient with exponent gap 65 succeeds under every FCW, confirming
that division has no addition/subtraction gap cutoff. Full `od-isa` passed
1,823/1,823. Workspace all-target offline
check, formatting and diff checks passed. Graphify rebuilt 11,388 nodes,
24,531 edges and 352 communities. Layout reports only the isolated worktree
`.git` marker and two unavailable historical oracle manifests tracked by
ISSUE-023; no new documentation path or layout violation was found.

Independent Sol/high review is pending. Physical x86, JIT and native x87
acceptance were not performed. Sources: Intel SDM Rev. 093 Vol. 2A
`FDIV/FIDIV`, pp. 3-333–3-335, and Vol. 1 §8.1.5.2/Table 8-2, §8.1.5.3,
§8.5.6 ([Vol. 2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=451),
[Vol. 1 PDF](https://cdrdv2-public.intel.com/929351/253665-093-sdm-vol-1.pdf#page=236)).
See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-30cx-bounded-fdiv-sti-st0-dc-f8-ff),
[Stage 2 plan](../plans/STAGE_2.md#s2-30cx-bare-fdiv-sti-st0-with-bounded-fcw-precision),
and [ISSUE-019](../KNOWN_ISSUES.md#issue-019--семантика-арифметики-и-инструкций-x87-остаётся-открытой).
