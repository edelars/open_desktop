# S6-06: host memory primitive measurements

Status: three correctness-checked release runs completed on one Apple Silicon
host. These measurements select candidates for later implementation; they do
not describe an implemented parallel RAM backend.

## Host and reproducibility

- MacBook Pro Mac15,11, Apple M3 Max, 14 cores; macOS 26.7.1 build 25G241.
- The host reports a 128-byte cache line (`sysctl hw.cachelinesize`). The
  benchmark process cannot query `sysctl` in this sandbox, so the observed host
  value and query provenance are included in each raw fingerprint.
- `rustc 1.99.0 (b940084d7 2026-09-28)`, target `aarch64-apple-darwin`, release
  profile, default Cargo release flags (`RUSTFLAGS` unset).
- Benchmark-module file SHA-256 (`tools/bench/src/stage6_host_memory.rs`):
  `58fe77309fbc107bc09876386e8553398580d11d9783f3af5fc1870865e530aa`.
- Build-input source-tree SHA-256: `7e74d189e48fee89068299ede23eb9d8106db0a0eb51103969425e9fd92f2dbc`
  (source revision `391e123ee6a8593a5ed92607df38fd2bf03d5c59`; working tree dirty,
  as recorded in every raw run).
- Benchmark binary SHA-256: `edd9d696afd0545cb0221f2b600de4a36e0c6f1a8274f15e2196005a80c81c48`.
- Each run retained 20 alternating paired samples for every comparison. Warm-up
  and calibration were discarded. The formal protocol floor is 200 ms per side;
  the harness required at least 300 ms per side and scaled retries toward 375 ms.
  Minimum retained side totals were 301,715,542 ns, 300,010,582 ns, and
  301,684,207 ns. Three release processes ran sequentially.
- The placement preflight verified that packed words shared one 128-byte cache
  line and each separated word was 128-byte aligned with a 128-byte stride.
  Correctness preflight checked every ordinary load/store candidate and width,
  uncontended and four-thread RMW totals for widths 1/2/4/8, and 16-byte carry
  and contention behavior for CASP and LL/SC. Both preflights passed in all
  three processes.
- Each process contains 138 comparisons: 120 ordinary load/store rows across
  widths 1/2/4/8 and one, two, or four threads, plus 18 RMW rows for widths
  1/2/4/8/16, one-thread uncontended or four-thread contended.

Exact reproduction commands:

```sh
cargo build --release --offline -p od-bench
target/release/od-bench stage6-host-memory --process-run 1 --output docs/reports/data/stage6/s6-06-host-1.json
target/release/od-bench stage6-host-memory --process-run 2 --output docs/reports/data/stage6/s6-06-host-2.json
target/release/od-bench stage6-host-memory --process-run 3 --output docs/reports/data/stage6/s6-06-host-3.json
```

The benchmark protocol requires 20 samples and a 200 ms per-variant floor:
[BENCHMARKS.md](../../../testing/BENCHMARKS.md). Raw values, per-row spread,
host/build fingerprints, cache-line size, and preflight status are in [run 1](s6-06-host-1.json),
[run 2](s6-06-host-2.json), and [run 3](s6-06-host-3.json). The separate
[instruction disassembly](s6-06-atomic-disassembly.txt) is tied to the binary
hash above. Failed and superseded attempts are indexed in
[failed-attempts.json](s6-06-failed-attempts.json); they are excluded from all
aggregates.

## Results

Ordinary access ratios compare each ordering candidate with its relaxed atomic
load/store baseline. Each table entry pools 1,200 paired sample ratios:
3 processes × 20 retained pairs × 4 widths × 5 thread/layout profiles
(single-thread, two-thread same/separate line, four-thread same/separate line).
This is 60 run/width/profile rows with 20 paired observations each. The table
reports the median and interquartile range (IQR); the relaxed baseline is not a
candidate choice.

| Operation and candidate | Paired ratios | Median candidate/baseline | Pooled IQR | Interpretation |
|---|---:|---:|---:|---|
| Load: Rust `SeqCst` atomic | 1,200 | 2.237 | 1.855–2.260 | Higher cost than relaxed; eligible same-width API candidate. |
| Load: `LDAR` | 1,200 | 2.862 | 2.678–2.877 | Slower in this harness; assembly is not a Rust synchronization contract. |
| Load: `LDR; DMB ISHLD` | 1,200 | 4.270 | 3.075–4.314 | Highest measured load cost. |
| Store: Rust `SeqCst` atomic | 1,200 | 1.047 | 1.000–2.276 | Within broad spread; no improvement claim. |
| Store: `STLR` | 1,200 | 1.039 | 1.000–2.267 | Overlaps the Rust atomic candidate; no improvement claim. |
| Store: `STR; DMB ISHST` | 1,200 | 2.004 | 1.993–6.620 | Higher measured cost, especially on shared lines. |

Same-width 1/2/4/8-byte RMW ratios pool 240 paired observations per
candidate/profile: 3 processes × 20 pairs × 4 widths. For uncontended RMW,
Rust `SeqCst fetch_add` was 0.541× LL/SC (IQR 0.539–0.544); explicit LSE was
0.542× (0.539–0.544). Under four-thread contention, Rust `fetch_add` was
0.870× (0.740–1.015); the pooled IQR includes 1.0, and the per-width IQRs
include 1.0 for three of four widths. The explicit LSE probe, which brackets
`LDADDAL` with full `DMB ISH` instructions, was 1.588× LL/SC (1.352–1.843).

For the 16-byte pair, 60 paired observations per profile were collected
(3 processes × 20 pairs). `CASPAL` took 1.265× LL/SC time when uncontended
(IQR 1.259–1.271) and 3.672× with four threads (3.014–4.176). LL/SC was faster
in both profiles; the IQRs are entirely above 1.

The disassembly records the actual Rust standard atomic code: `AtomicU8`
`fetch_add(SeqCst)` uses `LDADDALB`, `AtomicU16` uses `LDADDALH`, `AtomicU32`
uses `LDADDAL W`, and `AtomicU64` uses `LDADDAL X`. The 16-byte pair is not a
Rust standard atomic type. The explicit pair probes use `CASPAL` or `LDAXP` /
`STLXP` retry loops with `DMB ISH` ordering around the operation.

## Measured candidate selection and limits

- For aligned, homogeneous-width ordinary loads and stores, use Rust
  `SeqCst` atomics as the selected safe host API candidate. Stores are within
  measurement spread of `STLR`; loads are measurably more expensive than the
  relaxed baseline. Raw `LDAR`/`STLR` assembly was not selected because the
  Rust atomic API supplies the language-level synchronization contract.
- For same-width 1/2/4/8-byte RMW, use the Rust standard `SeqCst` atomic API.
  On this build it compiled to LSE instructions. Uncontended results matched
  explicit LSE; contended results were within LL/SC noise overall, while the
  explicit `DMB ISH` LSE probe was slower. This is the simpler memory-safe
  candidate and does not assert a general contention speedup.
- For aligned 16-byte RMW, use the measured LL/SC pair candidate rather than
  CASP. Rust std has no 128-bit atomic type; the eventual `UnsafeCell`/ownership
  wrapper still needs its own audited implementation.

These choices are constrained to the homogeneous-width access and synchronization
conditions listed in S6-04. `SeqCst` is stronger than x86-TSO and forbids the
guest-allowed SB `0,0` outcome; Rust atomics of different widths cannot safely
overlap. Therefore this measurement does not select a complete ordinary-RAM
backend, solve mixed-width aliasing, establish a global RAM ownership protocol,
or implement the instruction-specific page-walk, DMA, descriptor, and MMIO
transactions. Those limitations remain requirements for the later Stage 6
implementation slices.
