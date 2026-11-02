# S2-30cn — bounded precision-aware x87 FDIV

The interpreter now admits only bare Long64 `D8 F0..F7` (`FDIV ST(0), ST(i)`).
This adds a local precision-aware execution profile for this one register
direction. It does not change `FDIVR`, the `DC`/`DE` directions, the decoder,
schema, JIT, native runner, or the exact-only domains of other arithmetic forms.

Admission requires an x87 snapshot with absent FCS/FDS selectors; one of the
twelve all-exception-masked PC24/53/64 control words; and
`FSW & 0x80df == 0`. Every distinct participant must have a Valid tag and a
canonical nonzero Normal binary80 encoding. Either sign is accepted. For
`i=0`, only the physical TOP is validated and the self-alias commits exact
canonical `+1`.

For distinct operands, the existing S2-29ac direct division helper rounds the
exact quotient once at PC24, PC53, or PC64 using RC. The unrounded quotient
exponent and rounded result must be finite and Normal; zero, special,
subnormal, malformed, underflow, overflow, and helper-domain failures refuse
atomically before architectural mutation. These are local profile boundaries,
not general x87 exception behavior.

Success replaces physical TOP, sets `PE = old_PE || inexact` and `C1` to the
helper's `incremented` result, and preserves C0/C2/C3, TOP, tags, remaining
registers and x87 fields under the local policy. FIP receives old RIP, RIP
advances by two, the instruction retires once, and no data-memory callback is
performed. Prefixes, REX, WAIT/FWAIT, neighboring opcodes, truncation, STOP,
stop-RIP and zero-budget boundaries remain rejected or atomic as covered by
the existing decoder/executor gates. JIT and native execution remain closed.

The tests include independent literal byte encodings and 1/3 rounding results
across all twelve FCWs, all TOP/index pairs and operand-sign combinations;
self-aliasing; sticky PE and C1 replacement; midpoint parity and neighbors;
discarded quotient tails, carry, exponent boundaries, malformed/special and
zero inputs, state preservation, fetch-only behavior and atomic refusals.

Verification: focused FDIV decoder/executor passed 4/4; full `od-isa` passed
1,762/1,762, `od-testkit` passed 454/454, and `od-runner` passed 112 unit plus
487 fixture tests. `cargo fmt --all -- --check`, workspace all-target offline
check, `git diff --check`, and `graphify update .` passed; Graphify reports
11,093 nodes, 23,765 edges and 340 communities. Independent Sol/high review
found no concrete defects and reran the focused FDIV checks (od-isa 16/16;
od-testkit 6/6). Native denial is unchanged and its Linux-only regression was
not run on this macOS host.

Sources: Intel SDM Rev. 093, Vol. 2A, `FDIV` pp. 3-333–3-335 (operation and
flags p. 3-334),
[official Vol. 2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=451);
Vol. 1 §8.1.5.2/Table 8-2 and §8.1.5.3, and §8.5.6 p. 8-30,
[official Vol. 1 PDF](https://cdrdv2-public.intel.com/929351/253665-093-sdm-vol-1.pdf#page=236).
No physical x86 or native acceptance is claimed. ISSUE-019 remains open.
