# S3-56b. FIFO and generic dispatch on the same S3-14 corpus

`od-bench cold-first-run` accepts the tracked S3-14 branch/helper case list
and paired trace for the legacy S0-09 FIFO path. It selects this corpus by
the case-list digest and checks the trace digest, five inputs and five
visits before fixture preparation. The frozen S3-17 pair still selects
the existing full-corpus profile. `--generic-register` remains bound
to the S3-14 pair. The executable rejected four tests covering changed
case-list or trace bytes with each cold dispatch mode.

One signed release executable ran four modes on the five S3-14 fixtures.
Cold compared `cold-first-run` with and without `--generic-register`.
Warm used `warm-branch-helper` for FIFO and
`warm-first-run --generic-register` for generic. Both cold modes time
initialization of a fresh FIFO16 runtime and the dispatch calls. Both
warm modes time calls on a resident cache. Each process alternates JIT
and interpreter calls over the same inputs and sums the call intervals.
Fixture parsing, admission, state reset, parity checks, teardown and
process launch are outside the scored intervals.

The archived audit checks 14 retired instructions and five calls per
sweep in both engines. Each cold JIT sweep records five misses, compiles
and publications. Warm JIT sweeps record five hits and no publication.
Both JIT routes record five native block executions per sweep and no
evictions. The generic report separately accounts for zero interpreter
steps in native sweeps. [Frozen warm reference](STAGE_3_S3-36b_2026-10-30.md)

## Results

Two process rounds used opposite execution orders. Each of the eight
processes retained 20 pairs. All 320 timed sides met the 200 ms minimum.
The shortest lasted 200.028 ms. The reports preserve calibration and
any complete-series restarts from the existing benchmark. All four modes
reached a high-water mark of five entries, 7,500 code bytes and 17,641
logical metadata bytes.

Times are microseconds per sweep, shown as median [min, max]. The last
column contains the median and range of paired JIT/interpreter ratios.

| Mode | Round | JIT (µs) | Interpreter (µs) | JIT / interpreter |
| --- | ---: | ---: | ---: | ---: |
| Cold FIFO | 1 | 37.575 [33.053, 47.908] | 3.454 [3.220, 5.590] | 10.473 [6.862, 14.452] |
| Cold FIFO | 2 | 34.012 [32.450, 47.332] | 3.293 [3.151, 4.219] | 10.496 [10.006, 12.361] |
| Cold generic | 1 | 52.988 [38.268, 74.205] | 4.574 [3.320, 7.158] | 11.593 [7.866, 14.242] |
| Cold generic | 2 | 38.523 [36.774, 39.257] | 3.295 [3.247, 3.342] | 11.689 [11.296, 11.939] |
| Warm FIFO | 1 | 4.648 [4.589, 4.709] | 3.327 [3.219, 3.401] | 1.396 [1.372, 1.426] |
| Warm FIFO | 2 | 4.495 [4.426, 4.579] | 3.240 [3.192, 3.367] | 1.387 [1.344, 1.424] |
| Warm generic | 1 | 5.270 [4.447, 6.667] | 3.770 [3.202, 4.744] | 1.388 [1.323, 1.622] |
| Warm generic | 2 | 4.536 [4.444, 4.710] | 3.281 [3.207, 3.429] | 1.376 [1.336, 1.413] |

Cold generic/FIFO ratios of JIT medians were 1.410 and 1.133 in the two
rounds. Dividing the per-process median of paired JIT/interpreter ratios
gives 1.107 and 1.114. Warm raw ratios were 1.134 and 1.009. Warm normalized
ratios were 0.995 and 0.992, and the paired ranges overlap in each round.
Cold generic costs more on this corpus. Both JIT routes were slower than
the paired interpreter in every measured sample.

The historical S3-36b warm FIFO used the same fixture and trace digests and
reported 3.736–3.959 µs/sweep for FIFO and 0.726–0.751 µs/sweep for the
interpreter. Both absolute ranges are lower than the corresponding
process medians here. The matched round ratios above provide the
comparison between dispatch routes at the same revision. The separately
frozen 1,010-case S3-17 inputs are used in the historical cold comparison
with S3-35a. The five admitted fixtures exercise native
execution. Physical x86 and Mac mini checks remain deferred.
[Historical warm evidence](STAGE_3_S3-36b_2026-10-30.md)
[Historical cold comparison on S3-17](STAGE_3_S3-56_S3-17_2026-11-19.md)
[Issue and historical cold input](../KNOWN_ISSUES.md#issue-075-s3-56-historical-corpus-comparability)

## Reproduction and evidence

The host was Apple M3 Max with 14 cores, 36 GB RAM, macOS 26.7.1
(25G241) and rustc 1.99.0. The release build uses the archived patch on
`7664491`. Its resolved build source SHA-256 is
`b6e11097ec97b34e68afa9604bd859ecf1d146f17f9e49980460b7192d95a66e`.
The signed executable SHA-256 is
`29c4b8ba448bae0bf4c3bfff909ddb6ff6b2ab69fd618b1d9c3752f782ed5a95`.
The archive contains the input digests, exact patch, invocation order,
eight raw reports, rejected-input logs and the independent calculation.

Extract the archive's `files` into a fresh scratch directory. Run
`audit.py` with Python to recompute the saved results. For a fresh
measurement, start from a clean checkout of `7664491` and apply the
extracted `dispatch-cache.patch` with `git apply`. Then build `od-bench` with
`cargo build --release --offline --locked -p od-bench`. Sign a copy with
ad-hoc Hardened Runtime and
`apps/vmhost/Configuration/vmhost-smoke.entitlements`. Copy `run.py`
to a separate empty scratch directory and invoke it from the repository
root with the signed executable path as its only argument.
The archive SHA-256 is
`84c943ee1aef9f95bba49775edd7c0677dbbf1e91fe398a38bfd0cfed699ba64`.
[Raw evidence and scripts](data/stage3/s3-56-matched-20261119/s3-14-fifo-generic-matched-evidence.json.gz)
