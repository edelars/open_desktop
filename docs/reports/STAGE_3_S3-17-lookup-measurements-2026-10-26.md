# S3-17 lookup prototype measurements

Date: 2026-10-26

## Scope and protocol

Three independent release-process runs compared `Vec<TranslationKey>` and
`std::collections::HashMap<TranslationKey, ()>` for the current full typed key
and dependency-snapshot validator. Each run replayed the same recorded fixture
trace, used 18 scenarios, and collected 20 paired samples for each of four
kernels per scenario: cold construction (`Vec`, `HashMap`) and warm lookup plus
dependency validation (`Vec`, `HashMap`). This is 4,320 raw kernel samples
across the three runs. Keys, trace replay and validation were outside timing.

Cold construction samples sum table-construction timer intervals across bounded
micro-batches. Destruction and between-batch pauses are excluded from sample
`elapsed_ns` but included in the report's wall time. Each kernel's calibration
must observe at least 200 ms before fixing its repetition count. Ordinary
variation means some later samples were shorter: 27, 23 and 35 samples across
the four kernels were below 200 ms in process runs 1, 2 and 3 respectively;
the lowest was 175.8 ms. No scenario failed calibration or produced a partial
report.

The 128 MiB limit is a logical-storage budget for resident prepared suite data,
execution tables and one cold micro-batch. It includes inline key/table slots
and owned key buffers. It is not a process-RSS limit: HashMap control bytes,
allocator metadata, and runtime overhead are not measured. During periodic
`ps` snapshots, observed RSS reached approximately 1.03 GiB, 1.09 GiB and
1.05 GiB in runs 1, 2 and 3. System-wide free memory remained 73–74%; this was
not a peak-RSS instrumented measurement.

## Reproducibility

- Input: ignored local artifact `work/tmp/stage3/s3-17/dispatch-trace.json`
  (574 KiB), SHA-256
  `bf8d5efe0c52a47cbb4682f67ea73a82e684d463733aff3be60fed0b280bd5b3`.
- Trace: 1,010 fixture inputs, 1,322 recorded dispatcher visits, and 1,320
  observed unique block keys.
- Host: Apple M3 Max, macOS 26.7.1, `aarch64-apple-darwin`, Rust
  `1.99.0 (b940084d7 2026-09-28)`, optimized release profile.
- Source revision: `85afb316869971e80a0891b6cbdc849d89960c95`; source SHA-256
  `a2de6c274d51ce81f836bd99d0cd1413761192ddc8288a54216b374f2d61e5dd`;
  build and execution tree clean for all three runs.
- Release binary SHA-256: `79ba483306f0cad8f56013e50db61e0cc06c3651e724f595b55a022228d4c29d`.
- Runs were sequential and single-threaded; power/thermal telemetry was not
  collected.
- Command per run:
  `target/release/od-bench measure --trace work/tmp/stage3/s3-17/dispatch-trace.json --process-run N --output work/tmp/stage3/s3-17/measure-N.json`.
- All JSON reports use schema 3, contain 18 scenarios, 20 samples per kernel
  and scenario, and validate identical Vec/HashMap outcomes.

| Run | Total wall time | Raw report SHA-256 | Output |
| --- | ---: | --- | --- |
| 1 | 592.832 s | `d8ba7ddc79d94e0baa2f83bb6d71478bc0e668e90964710ec647969d4daa7282` | [measure-1.json](data/stage3/s3-17-translation-lookup-2026-10-26/measure-1.json) |
| 2 | 591.542 s | `50c5d96ebe232163098313725d9602da8634455237cc8bc8f80960f7177aaf31` | [measure-2.json](data/stage3/s3-17-translation-lookup-2026-10-26/measure-2.json) |
| 3 | 596.055 s | `c1770591cb210d6c7951843276a71d642167aa6ddc073978ede9a8123bca51f1` | [measure-3.json](data/stage3/s3-17-translation-lookup-2026-10-26/measure-3.json) |

The values below are the median across the three per-run medians, in
nanoseconds per key construction or lookup operation. Raw samples, min/max/IQR,
operation counts, calibration repetitions, capacities and hashes are preserved
in the three local JSON reports above.

| Scenario | Cold Vec | Cold HashMap | Warm Vec | Warm HashMap |
| --- | ---: | ---: | ---: | ---: |
| Recorded corpus trace | 43 | 165 | 980 | 205 |
| Observed unique count (1,320) | 43 | 168 | 997 | 211 |
| Synthetic size 1 | 52 | 137 | 103 | 184 |
| Synthetic size 8 | 41 | 119 | 122 | 184 |
| Synthetic size 64 | 39 | 144 | 233 | 181 |
| Synthetic size 512 | 38 | 150 | 1,150 | 192 |
| Synthetic size 4,096 | 38 | 153 | 8,363 | 189 |
| Hot loop, 1 key | 50 | 135 | 103 | 183 |
| Hot loop, 8 keys | 41 | 119 | 122 | 183 |
| No-locality unique sequence | 38 | 150 | 8,317 | 188 |
| Mixed hit/miss | 39 | 152 | 3,180 | 142 |
| Stale mapping | 40 | 118 | 119 | 179 |
| Stale backing | 40 | 121 | 122 | 183 |
| Stale execute permission | 41 | 122 | 48 | 107 |
| Alias with shared backing | 46 | 125 | 106 | 185 |
| One dependency span | 51 | 135 | 103 | 181 |
| Two dependency spans | 52 | 168 | 130 | 232 |
| Four dependency spans | 56 | 227 | 189 | 347 |

## Limits

On this trace and host, warm HashMap lookup had lower median cost in the
recorded corpus and larger synthetic tables, while cold HashMap construction
cost more than Vec construction in every scenario. For one- and eight-key hot
loops, warm Vec was faster. These measurements cover prepared full keys only;
they exclude decode, key construction, native publication/execution, cache
miss handling, invalidation and eviction. They do not establish full dispatcher
or JIT speedup, production cache policy, or behavior on other hardware.

The three raw measurement JSON reports are committed beside this report.
Production caching and end-to-end performance remain open.

## Exact input recovery, 2026-11-19

The original signed ARM64 recorder was rebuilt from
`80b318a6293da499c7d9287da4d9db8c4e3ff4f6`. Two retained October Stage 1
manifests supplied the same 1,010 fixture files in different orders.
Each file matched its manifest digest. Both actual captures returned 1,322
visits. The case-ID-sorted capture matched the historical trace when its
capture host and elapsed time were restored from the archived measurement.
The recorder's input and visit payload bytes were preserved. The restored
`trace_elapsed_ns` value of 593184292 is historical metadata.

The recovered trace is 587,555 bytes, with 1,010 inputs, 1,322 visits,
and 1,320 distinct input-index/entry-RIP pairs. Its SHA-256 equals the
historical pin
`bf8d5efe0c52a47cbb4682f67ea73a82e684d463733aff3be60fed0b280bd5b3`.
All 1,010 archived fixture files match the trace's ordered case IDs,
byte lengths and raw SHA-256 values.

The [recovery bundle](data/stage3/s3-17-recovery-20261119/historical-trace-and-fixtures.json.gz)
contains 1,020 files and has SHA-256
`a6c8db14570d5420d540401c466cc3ac85186458fb3e881618b403df7bf1d969`.
It retains the recovered trace, raw sorted capture, exact fixtures,
recovery scripts and build metadata. The UTF-8 contents are in `files`
and their SHA-256 values are in `file_sha256`.

The original `cases.txt` remains unavailable. The archive's
`verified/relocated-cases.txt` lists the recovered files in trace order.
Its locator strings give it a separate SHA-256
`e72bb9c57da22b65f14aa8028621021cf171d793267d874330c2e6ca3d2ba074`.
The default historical list pin stays
`b9276b52556b3692552de074e83cd8aef0c777f29861bd19ab09c188e41615d3`.
S3-35a historical comparison remains in [ISSUE-075](../KNOWN_ISSUES.md#issue-075-s3-56-historical-corpus-comparability).
