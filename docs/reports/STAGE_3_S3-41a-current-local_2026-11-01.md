# S3-41a — current full-local bounded harness run

## Run record

The accepted run `stage3-20261101-190932-58293bb642` is a full invocation of
the local harness selector, with `result_scope=local_only`. Its summary records
120 passed, 0 failed, and 33 `not_run`; the evidence bundle and bindings are
independently reviewed and passed with no findings. This is a local macOS
ARM64 harness result, not Stage 3 completion or final S3-41 acceptance.

The copied evidence is under
[`data/stage3/s3-41a-local-20261101/`](data/stage3/s3-41a-local-20261101/):

- [Accepted run summary](data/stage3/s3-41a-local-20261101/accepted/summary.json)
- [Accepted input manifest](data/stage3/s3-41a-local-20261101/accepted/input_manifest.json)
- [All 172 accepted-run logs](data/stage3/s3-41a-local-20261101/accepted/logs/)
- [Stage 1 child summary, manifest, and logs](data/stage3/s3-41a-local-20261101/accepted/stage1-child/)
- [Preceding failed-run summary and manifest](data/stage3/s3-41a-local-20261101/preceding-failure/)
- [SHA-256 and size receipt](data/stage3/s3-41a-local-20261101/sha256_receipt.json)

The bundle contains JSON manifests and text logs only; compiled binaries, app
bundles, Cargo output, and other build artifacts were not copied.

## Local harness and Stage 1 child scope

The accepted summary records `local_lifecycle`, `local_acceptance`,
`memory_corpus`, and `local_vmhost_result_state` as passed. The
`stage1_dispatch` suite is `local_only`; `vmhost_process`, `physical_x86`,
`mac_mini`, `performance`, `owner_backed`, and `stage2` are `not_run`.

The Stage 1 child is separately bound by its summary and manifest hashes in the
parent summary. It used 10 manual inputs and 1,000 generated inputs, ran each
twice, and emitted 2,020 local results. Its source fingerprint is
`b96906f28c43a2b2a47e995da23e177eafee2585a1df75443ad1501db9368ef3`; its
evidence scope is `local_only`. It checks local interpreter/JIT-dispatch
agreement for that frozen corpus and does not replace physical x86 comparison
or broader Stage 1 acceptance.

## Preceding failed attempt and correction

The preceding local invocation, `stage3-20261101-182017-d88d51f20b`, stopped
with `FileExistsError` when creating its run's `artifacts` root. S3-34p had
already created that real directory and populated it; the harness attempted
exclusive creation. Its summary records 106 passed, 0 failed, and 47
`not_run`; S3-38d had not run. The [failed summary](data/stage3/s3-41a-local-20261101/preceding-failure/summary.json)
and [input manifest](data/stage3/s3-41a-local-20261101/preceding-failure/input_manifest.json)
are preserved separately from the accepted retry.

Commit `020f79d` changed artifact-root handling to reuse an existing real
directory safely and reject symlinks, and added a 41st self-check. A separate
post-fix Python harness self-check reported 41 tests, 92.130 seconds, `OK`; its
standalone raw stdout was not saved. The accepted local run also records its
own `harness_self_tests` step as passed; the copied log reports 41 tests in
86.198 seconds (104.964 seconds for the whole self-test step). That single
self-check step is included in the accepted run's 120 passed steps; its 41
individual tests do not add 41 more harness steps.

## Reproducibility bindings

- Accepted run: 2026-11-01 19:09:32–19:51:56 UTC.
- Source revision: `020f79d74566ad6abdc8d2fa4ad8d6e0bfaf65ea`.
- Source fingerprint at start and end:
  `8dfe432b2903d5b62c7de353665b0efd51597280a6cf86ca7223f7e18e8175f9`.
- Accepted summary SHA-256:
  `1d41db29e5c63a08d477c6d5787ea93d28e5df290fab9e36ae76ee8a62923509`.
- Accepted input-manifest SHA-256:
  `b9295da476d67df9fadc3040be158bea9390d53fa8ed73a7e82d45e3425389b9`.
- Host/toolchain: Apple M3 Max, macOS 26.7.1, ARM64; Python 3.15.0,
  rustc/cargo 1.99.0.
- Stage 1 child summary SHA-256:
  `b7a9313cac2bad8c34f89f66c94bb29aeeb788de525f5f7c97c65b88d782a486`.
- Stage 1 child manifest SHA-256:
  `53478d3018e52f6b4514c9b41c5a6983c1aca704abb66434c177762a059d9cc6`.
- Preceding failed run revision: `c9e8b73573286966f5aec77381a10beca994ff17`;
  source fingerprint start=end `8c2bb1fe7157e7cb7622484f447c74c4b0751aebe893bf61966227aa5a9101c2`.
  Its summary SHA-256 is
  `649cc466799d92b3c7041299b083f199cae797f39602333209b31ac339e6949e`;
  its input manifest SHA-256 is
  `29a703a6bee33c51f5a6ca9e527fd178d2ed7d966e87f24c355a7f29c47a5073`.

## Limits and open work

This run is local-only evidence. It does not close Stage 3, S3-41, ISSUE-027,
Stage 2, Mini, performance, physical x86, or production owner/MMU gates. The
run's local RESULT-state suite does not replace the separate S3-38d report; the
process-only S3-38b result remains a separate run. The failed artifact-root
attempt is retained in the record even though the accepted retry completed.
