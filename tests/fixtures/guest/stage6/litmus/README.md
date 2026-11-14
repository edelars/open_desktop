# Stage 6 litmus fixtures

These are project-authored finite litmus descriptions in `od.smp-litmus.v1`.
The builder is `tools/build/build_stage6_litmus.py`; run it from any directory
with `python3 tools/build/build_stage6_litmus.py`. It writes canonical UTF-8
JSON with sorted object keys, stable fixture order, sorted outcome vectors,
and prints a SHA-256 over the generated fixture bytes. No assembler, emulator,
external package, or host-x86 execution is involved.

## Provenance and meaning

The principal source is Sewell et al., *x86-TSO: A Rigorous and Usable
Programmer’s Model for x86 Multiprocessors*, CACM 2010, §§1, 3, 3.2–3.3:
[paper and author materials](https://www.cl.cam.ac.uk/~pes20/weakmemory/index3.html),
[PDF](https://www.cl.cam.ac.uk/~pes20/weakmemory/cacm.pdf). Instruction
semantics and encodings are taken from Intel, *64 and IA-32 Architectures
Software Developer’s Manual*, v093, Vol. 3A §8.2 and the instruction entries
for `MFENCE`, `XCHG`, `XADD`, `CMPXCHG`:
[Intel SDM](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html).
The fixture files cite the source used for each instruction/test family. They
are written from the descriptions in those sources; no external litmus suite
files, tables, or generated data were copied.

`sources` identifies the normative/public basis. Each full enumerated model
set is explicitly marked with `derived_by: build_stage6_litmus.py/1`: it is
the result of this project's exhaustive finite-state enumerator, not a
physical-machine observation. A source which gives a representative outcome
does not by itself establish a complete outcome set. In particular, SB's
four complete TSO outcomes are confirmed by the paper's `00` allowance plus
SC witnesses for the other three; the own-model SC result excludes `00`.
For MP and IRIW the known forbidden outcomes are absent in the generated TSO
set. Every other finite set is enumerated from the fixture operations.
Each model entry also records the enumerator revision, unique explored-state
count, and SHA-256 of the abstract fixture input before outcome fields are
added.

## Corpus

| Fixture | Operation shape | Expected outcome summary |
|---|---|---|
| `sb.json` | Two store-then-load threads | TSO has four outcomes including both loads reading zero; memory-event SC and instruction-atomic interleaving each have three and exclude it. |
| `mp.json` | Payload store, publication store; publication load, payload load | TSO excludes seeing publication `1` with stale payload `0`. |
| `lb.json` | Load then store in each of two threads | TSO permits the two-zero load result. |
| `iriw.json` | Two independent writers and two readers in opposite orders | TSO excludes the classic result where both readers observe `1` then `0` in opposite location orders. Four CPU identities are represented. |
| `2+2w.json` | Two threads store two locations in opposite order | TSO preserves each thread's FIFO store order; all reported finals have explicit model witnesses. |
| `sb-mfence.json` | SB with an `MFENCE` after each store | Both-zero is absent from all three sets. |
| `sb-lock.json` | SB with a locked increment of a shared dummy cell before each load | Locked operation drains pending TSO stores and makes both-zero absent. |
| `sb-lock-local.json` | One SB thread has a locked increment; the other retains an ordinary store buffer | TSO allows both loads to read zero while final memory is `(x=1,y=1)`: the lock drains only its issuing CPU's queue. |
| `sb-xchg.json` | SB with implicit-LOCK memory `XCHG` before each load | Both-zero is absent from all three sets. |
| `sb-lock-xadd.json` | SB with `LOCK XADD` before each load | Both-zero is absent from all three sets. |
| `lock-cmpxchg-mutex.json` | Two contenders perform one `LOCK CMPXCHG` attempt | Exactly one compare-exchange changes the initial zero value; the failed attempt observes the winner's value. |
| `xchg-mutex.json` | Two contenders exchange a nonzero token with a zero shared word | Exactly one observes zero; the other observes the token. This is one atomic arbitration attempt, not a retry loop. |
| `lock-xadd.json` | Two locked adds to one cell | Final value is the sum; each register records its old value in serialization order. |
| `store-forwarding.json` | One thread stores then reads the same location; another reads it | The local read may forward the buffered value before global visibility. |
| `unlocked-inc.json` | Two unprefixed `INC [x]` read-modify-write instructions | Lost update is allowed by memory-event SC and TSO; it is impossible when the harness makes each whole instruction indivisible. |

`counters-formula.json` records the unenumerated counter group as runnable
parameterized x86 code templates. Each case gives initial RAM/registers,
per-CPU code-base formula, instruction asm/bytes/operands/events, and an
`RCX=K` loop with a checked relative branch back to the body. Parameters
require `N >= 2`, `K >= 1`, and `N*K <= 2^32-1`. For `N` threads with `K`
operations, `LOCK INC` ends at exactly `N*K`. Unprefixed `INC [x]` has the
exact TSO/SC integer range `1..N*K` when `K=1`, and `2..N*K` when `K>=2`; the
whole-instruction interleaver always gives exactly `N*K`. The separate
load/add/store loop has the same exact range in all three models. These
closed forms were checked against full enumeration for N=1,2,3 and K=1,2,3;
N=1 is an extra regression beyond the fixture parameter domain. The group
intentionally has no enumerated outcome arrays. Its machine-readable oracle
uses `exact_singleton` or `exact_integer_range` with `complete: true`, so a
consumer can distinguish a full allowed set from bounds.

Each finite fixture records its exact order-preserving schedule count. Every
finite count is below the calibrated ceiling, so all three model outcome sets
are exhaustive and use no seeds. The ceiling is **1,000,000 complete
order-preserving schedules**, not expanded-state count. Above it, S6-19 owns
runtime schedule sampling with deterministic seeds `0..9,999` inclusive;
`random.Random(seed)` shuffles available instruction-step successors. This
seed range is a reproducibility budget, not a statistical confidence claim.
S6-05 measured the cost of this range; S6-19 owns its runtime use and result
recording.

Calibration used the actual instruction-atomic enumerator on 2 threads with
12 shared loads each (2,704,156 possible complete schedules), with state
deduplication disabled for path counting and the unique-state set plus DFS
frontier retained. It stopped at exactly 1,000,000 completed schedules in
each of three runs: 3,851,199 expanded states, 149 unique states, 12 pending
states, 27,607,040–27,705,344 bytes peak RSS, and 10.279–10.564 seconds
(2.72% relative time range). The 0–9,999 seed range was run three times after
an excluded 0–99 warm-up: each run completed 10,000 paths / 250,000 expanded
states, used 27,607,040–27,770,880 bytes peak RSS, and took 1.097–1.116
seconds (1.77% range).
Both measured gates are below the 2 GiB / 120 second limit. No lower schedule
candidate was needed. Raw repetitions, environment, workload hashes, and the
older state-expansion-only runs (explicitly non-basis) are in
`docs/reports/data/stage6/s6-05-enumerator-calibration.json`. S6-05 remains
unaccepted pending independent review.

## Model and validation boundary

The TSO enumerator gives every CPU a FIFO store buffer, forwards a read from
the newest matching local buffered write, and chooses explicit one-entry FIFO
flushes. `MFENCE` waits for its own buffer to empty. A `LOCK` RMW or implicit
memory `XCHG` waits for the issuing CPU's queue to empty and updates shared
memory as one atomic event; other CPUs' buffers remain pending. This permits
the locked-SB witness where T1 reads `x=0`, T0 drains `x=1` and reads still
buffered `y=0`, then T1's `y=1` flushes. SC enumerates memory events in per-thread order; an ordinary
memory RMW has a distinct read and write event, so another CPU may interleave.
The third scheduler executes each entire instruction as one indivisible step,
including unprefixed RMW instructions. State deduplication merges identical
states only; no transition or outcome is removed because of its expected
answer.

The tool checks exact byte length and operation-specific encodings, assembly,
operands, memory-event cardinality and values, and that the encoded disp32
names the event's symbolic cell. The encodings are handwritten fixture data,
not assembled at generation time. Code/data/register regions are represented
as disjoint address/register namespaces in each fixture; all shared data
cells are aligned 32-bit ordinary write-back RAM. This corpus is a model
input and does not claim execution by a guest, emulator, or physical x86.

`python3 tools/build/test_build_stage6_litmus.py` checks reproducibility,
schema/encoding consistency, SB/MP/IRIW model properties, and the unlocked
`INC` distinction, counter formula ranges, event/encoding mutation rejection,
and locked local-buffer behavior. Calibration locks the numeric schedule and
seed budgets; corpus acceptance and S6-19 runtime sampling acceptance remain
pending independent review and implementation, respectively.
