# S2-30cv — bounded reverse `FSUB ST(i), ST(0)` precision

The Long64 interpreter admits exact bare `DC E8..EF`, writing old physical
`(TOP+i)&7` with `ST(i) - ST(0)`. It applies direct FCW precision 24/53/64 and
all four rounding modes by validating both original operands, flipping the
sign bit only on a local copy of ST(0), then reusing the S2-29ae direct-addition
precision helper. No public arithmetic API or guest source mutation was added.

Admission requires present x87 state, absent FCS/FDS selectors, one of twelve
all-exception-masked FCWs, `FSW & 0x80df == 0`, Valid tags, and canonical
nonzero Normal binary80 operands. Normalized exponent gap must be at most 64
before the arithmetic helper is called. The `i=0` alias cancels to zero and
remains atomically Unsupported. Only finite Normal exact or rounded results
are accepted. The earlier S2-30i exact-result set is included by the precision
64, equal-exponent path; tests compare its bytes and diagnostics against the
unchanged exact helper.

Success changes only the selected physical destination, accumulates PE from
`inexact`, replaces C1 from the helper's pre-carry `incremented` diagnostic,
preserves C0/C2/C3 by local policy, stores old RIP in FIP, advances RIP by two
and retires once. TOP, tags, source bytes, FCW/FDP/FOP, GPRs, RFLAGS and
unrelated state remain unchanged. Register execution performs no data access.
Operand, environment, numeric-domain and execution-boundary refusals remain
atomic. Other D8/DC directions and DE pop forms retain their previous paths.
JIT/native execution, general x87 exception delivery and physical x86
acceptance are not claimed.

Coverage includes a 12,288-case matrix over all TOP/index pairs, twelve FCWs,
four sign combinations and every prior PE/C1 combination; midpoint neighbors,
negative directed rounding and a direct-versus-double-rounding discriminator;
gaps 0/1/63/64 and 65+; alias/cancellation, malformed, zero, subnormal and
overflow refusals; the full prior exact-result domain across TOP/index, signs,
PE/C1 and condition-code histories; max carry, minimum Normal, ±2^-63 and
upper exponent boundary vectors; unsupported FCW/FSW, selector and tag gates
tested from noncancelling baselines; fetch, STOP, budget and no-data-access
checks; and D8/DC/DE decoder and runtime regressions.

Validation passed: focused reverse-FSUB/decoder tests (10), full `od-isa`
(1,813), `od-testkit` (455), `od-runner` (115 unit plus 493 fixture tests),
workspace all-target offline check, formatting and diff check. Layout reports
the existing worktree `.git` marker and two missing historical oracle manifests
tracked by ISSUE-023; no new broken documentation links were found. Graphify
update completed with 11,335 nodes, 24,389 edges and 351 communities.
The Sol/high re-review is clean; its focused `fsub` filter passed 35 tests.
No physical x86, JIT or native acceptance was performed.

Normative references: Intel SDM Rev. 093 Vol. 2A `FSUB/FSUBP/FISUB`, including
`DC E8+i`, pp. 3-394–3-396
([official Vol. 2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=512));
Vol. 1 §§8.1.5.1–.3, Table 8-2, and §8.5.6, pp. 8-7–8-8 and 8-30
([official Vol. 1 PDF](https://cdrdv2-public.intel.com/929351/253665-093-sdm-vol-1.pdf#page=236)).
See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-30cv-bounded-fsub-sti-st0-dc-e8-ef)
and [Stage 2 plan](../plans/STAGE_2.md#s2-30cv-bare-reverse-fsub-sti-st0-with-bounded-fcw-precision).
