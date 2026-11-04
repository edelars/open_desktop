# S3-38b — current-source local VMHost process regression addendum

## Recorded run result

The independently reviewed run record `stage3-20261101-180016-5fb4c7afec` is scoped to
`vmhost_process_only`. Its summary records the selected `vmhost_process` suite
as passed with 33 steps passed, 0 failed, and 120 not run. Independent review
passed for the summary, manifest, selected argv and logs, hashes, signatures,
entitlements, process-group cleanup, and artifact bindings. This does not claim
that ISSUE-044 is fixed or closed. It is a local macOS ARM64 process-gate
observation only.

The evidence bundle includes the summary, input manifest, all 133 run logs
(including per-step stdout/stderr and child diagnostics), and a SHA-256/size
receipt for every copied file:

- [Run summary](data/stage3/s3-38b-20261101/summary.json)
- [Input manifest](data/stage3/s3-38b-20261101/input_manifest.json)
- [Evidence SHA-256 receipt](data/stage3/s3-38b-20261101/evidence_sha256.json)
- [All process logs](data/stage3/s3-38b-20261101/logs/)

No executable, app bundle, static library, module cache, or other build output
was copied into the evidence bundle. Artifact identity is recorded by hashes
in the run summary.

## Harness self-check and process suite are separate results

Before the accepted process-only run, one Python harness self-check invocation
ran 40 checks and failed 6 stale-fixture checks. A fixture-only correction was
recorded in commit `621e805`; the subsequent 40-check harness self-check passed
40/40. These are harness consistency checks and are not included in the process
suite count below. The 40/40 result was observed in command output; its raw
stdout was not archived. The evidence bundle contains only the separate
process-suite invocation, whose archived summary records 33/0/120.

## Process coverage observed

The selected suite built the host library, VMHost, launcher, generated-fault
helper, and signed app bundle in run-scoped paths. It recorded the resolved
`codesign` path/hash and inspected VMHost, helper, launcher, and bundle
signatures/entitlements. Process scenarios included control-race behavior,
isolation, execution of the preserved signed app bundle, and generated-code
fault handling.

For the generated-code fault scenario, the smoke expected the guest-generated
fault to terminate VMHost with SIGSEGV. The launcher returned 1 and reported
`VMHost terminated by SIGSEGV`; the smoke step itself returned 0 because the
expected failure mode was observed. Its process-group cleanup field was
`empty`. This expected-fault observation does not establish that the
intermittent ISSUE-044 behavior is resolved.

## Run and artifact bindings

- Run: `stage3-20261101-180016-5fb4c7afec`, 2026-11-01 18:00:16–18:11:51 UTC.
- Source revision: `621e805a463d36aa1677fa7ca882f33c738e5b32`.
- Source fingerprint at start and end:
  `3e831696a8edcd5551df9b517a0ed871fbffa06111e5379f8ae8a8a8fe6f36ed`.
- Summary SHA-256:
  `6a9432420bca7c535354231d1f78f905b9ae3fcdd3bb023dc6d0613fa57ec5d5`.
- Input-manifest SHA-256:
  `29a703a6bee33c51f5a6ca9e527fd178d2ed7d966e87f24c355a7f29c47a5073`.
- Host: M3 Max, macOS 26.7.1 build 25G241, ARM64; Rust/Cargo 1.99.0,
  Swift 6.4, Xcode 27.1. The run resolved `/usr/bin/codesign`, SHA-256
  `3635f18db517033a4786ea7ddb0acc1801e642eaf6171e208e24cb9a1922f518`.
- Artifact hashes from the run summary: app bundle tree
  `3055e2ece7d9004a64bdc9af8cd5475aa38fdbf15553edb704e9fc381735f457`;
  bundle launcher `7910a9208cddda4787623fa34cf9e84e5d7e24a19d5273b025a57d4f11665560`;
  bundle VMHost `486b78046a1b778e4dd25942848658032efadd3f864cf98c28795dae0132b5c7`;
  standalone launcher `7657d726f33a3a25462dd7b768600dec86821af7da254e66744f4340591b9d2d`;
  fault helper `bf14974fdd6569ed344a46149d326a8a57d971c6ef17e617d98fef66b29bbbde`;
  `od_host` static library
  `66a2ccbd85e01730f54d23572287e029c38bfedd17195bc4342c6f6863baf04e`.
  The binaries and app bundle are not part of the committed evidence bundle.

## Scope limits

All ten other suites in the accepted summary are `not_run`, including local
acceptance/lifecycle/Stage 1/memory, RESULT-only, physical x86, Mac mini,
performance, owner-backed, and Stage 2. The run does not replace S3-38d's
RESULT-state transport slice, does not establish Mini or physical-x86 behavior,
and does not resolve ISSUE-044. S3-38 and Stage 3 remain open.

The earlier S3-38b process-only evidence and ISSUE-044 history remain in the
[original S3-38b report](STAGE_3_S3-38b_2026-10-30.md) and
[diagnostic report](STAGE_3_ISSUE-044_DIAGNOSTIC_2026-10-30.md). This addendum does not
erase prior timeout observations.
