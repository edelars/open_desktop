# Stage 3 S3-17 reviewer acceptance

**Reviewed revision:** `629b1b14a341e47758e7dd9f1da11f7abf5a68cd`  
**Review role:** independent `gpt-6.1-sol`, high reasoning  
**Result:** clean sign-off for the fixture-level key, dependency snapshot and
historical lookup-measurement evidence. S3-17 remains open for production
owner/MMU guarantees and production cache-policy decisions.

## Contract review

The reviewer checked `TranslationKey`, `dependencies_match_snapshot`, the
dispatcher lookup/publication/entry path, their unit tests, and the frozen
benchmark protocol. The key contains compile flavor, execution context, entry,
full code bytes/base/end, fetch ranges and terminal classification. Equality
compares full content after hashing, including the forced-collision test.
Dependency spans cover the full `CodeView` without gaps or overlap and validate
execute permission, mapping/backing identity, generation, aliases and address
reuse. The synchronous fixture validates source and dependencies at lookup,
before and after publication, and before entry.

Independent correctness checks passed: 11 translation tests and 34 dispatch
tests. No runtime changes or new benchmark runs were made for this review.

## Frozen measurement provenance

The three committed `measure-N.json` SHA-256 values match the measurement
report:

| File | SHA-256 |
| --- | --- |
| `measure-1.json` | `d8ba7ddc79d94e0baa2f83bb6d71478bc0e668e90964710ec647969d4daa7282` |
| `measure-2.json` | `50c5d96ebe232163098313725d9602da8634455237cc8bc8f80960f7177aaf31` |
| `measure-3.json` | `c1770591cb210d6c7951843276a71d642167aa6ddc073978ede9a8123bca51f1` |

The reviewer also confirmed the preserved trace SHA-256
`bf8d5efe0c52a47cbb4682f67ea73a82e684d463733aff3be60fed0b280bd5b3`, release
binary SHA-256
`79ba483306f0cad8f56013e50db61e0cc06c3651e724f595b55a022228d4c29d`, and
source fingerprint SHA-256
`a2de6c274d51ce81f836bd99d0cd1413761192ddc8288a54216b374f2d61e5dd` for the
recorded source revision `85afb316869971e80a0891b6cbdc849d89960c95`.
All 1,010 fixture hashes/lengths and the recorded program hash were checked.
The trace contains 1,322 dispatcher visits and 1,320 unique input/entry pairs.
The sample count is `18 scenarios × 20 samples × 4 kernels × 3 runs = 4,320`.
Scenario distributions, operation counts and the 18 median rows agree with the
raw reports. The measured cold/warm boundaries match the documented protocol.

Claims remain limited to the frozen prototype workloads: prepared-key Vec and
HashMap lookup/construction. They exclude current-key construction, decode,
publication, execution, cache-miss handling, invalidation and eviction. They do not choose a
production cache structure or establish end-to-end JIT speedup.

## Remaining conditions

- Owner/MMU integration must provide truthful mapping/backing identities and
  generations and guarantee notification/invalidation completeness.
- Full production cache/invalidation and the production lookup-policy choice
  remain open; this review covers the synchronous fixture contract only.
- Historical measurements target source revision `85afb316…`. They do not
  measure the later `ExecutionContextSnapshot` key or other hardware.
- Mini, physical x86 and performance gates remain deferred until the end of
  Stage 3.

See the [S3-17 plan](../plans/STAGE_3.md#s3-17-определить-cache-key-и-зависимости),
[measurement report](STAGE_3_S3-17-lookup-measurements-2026-10-26.md), and
[benchmark protocol](../testing/BENCHMARKS.md#s3-17-translation-key-lookup-prototype).
