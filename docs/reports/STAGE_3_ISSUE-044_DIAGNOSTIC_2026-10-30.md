# ISSUE-044 — bounded generated-fault diagnostics

## Result

Three diagnostic attempts on one frozen, instrumented launcher/helper/library
set all followed the expected generated-fault path. Each completed within the
existing deadlines, observed the helper terminate by SIGSEGV, and left no child
process. The earlier timeout did not recur. The timeout cause is still unknown;
no fix or stable pass is claimed, and ISSUE-044 remains open.

## Compare of the preserved S3-38b runs

The completed pass was `stage3-20261030-022418-2f31426694`; the timeout was
`stage3-20261030-023436-6452c1b474`. Their preserved summaries are the pass
summary (`work/stage3-harness/stage3-20261030-022418-2f31426694/summary.json`)
and timeout summary (`work/stage3-harness/stage3-20261030-023436-6452c1b474/summary.json`).
Both record revision
`8e0a2b289869a224917c8188cbb04cd5edbdfc93`, the same macOS ARM64 host,
Rust/Cargo 1.99.0, Swift 6.4, Xcode 27.1, `/usr/bin/codesign` hash
`3635f18db517033a4786ea7ddb0acc1801e642eaf6171e208e24cb9a1922f518`, and
entitlements source hash
`958648f799e436860b51eaf55ec8f92d2c62da17001e23d96bc05ffc748f2a2a`. Their
source fingerprints were different: `09b0c2e526b63ad1e13365c8cbbfa136235988a8a6a4508a2af14d6ce11a45b2`
for the pass and `798b7b1af26171a367f5d4b7d0fcf96b3c12ade58cf455f7e13bcea90173eaeb`
for the timeout.

Both runs used the same release Cargo build, isolated target directory, and
Swift `-parse-as-library -O` configuration. The exact commands and their
per-step durations are retained in each run's `summary.json`.

| Artifact | Passing run SHA-256 | Timeout run SHA-256 | Comparison |
|---|---|---|---|
| `libod_host.a` | `9ea908b5e663f6487a240146b677d89a7dc5f3f88e333ab032ed3016bfaab2f5` | same | identical |
| Standalone launcher | `a278885efbe72e32b902b7bfc5f6c6617a703c4b4fda1bb86be915d25de3546d` | same | identical bytes and UUID |
| VMHost smoke | `3f53b6c629e395ab3bc9a2bba054dfc4690ce6f7854df7ab10070dd3b4246347` | `13053ebc0bc632b8a8d7189120864720facf3ec9449fb55b0fbaf34da395a3d8` | whole signed files differ |
| Generated-fault helper | `a2beba8610fba927b782ca55a6a6248830606a3609518339ff0ff82f8861f2ca` | `2aaa1cec47bbc2b4264f980406ff1baea152cb6bc254561e7f2d32a2ecec5546` | whole signed files differ |
| Preserved app bundle tree | `c8dc0ad3dbe469b61c5a6e1aba0cff11738432d5bd14be627d1ea2cc156c0356` | `0193e45afeca872148b524632993d57d46814a96367fbfaba9a433c002550153` | differs |

The VMHost and fault-helper binaries had the same Mach-O UUIDs and matching
hashes for all named sections. Their whole-file hashes still differ, including
link-edit/signature bytes, so these historical attempts did not run the same
binary bytes. The app-bundle launcher was re-signed during bundle creation and
also had a different hash; the standalone launcher used by generated-fault
smoke was identical.

The same codesign tool and entitlements policy were used in both runs. VMHost
and the generated-fault helper had `com.apple.security.cs.allow-jit=true`; the
launcher had no entitlements. Ad-hoc signature inspection passed. The generated
fault step command used the standalone launcher and generated-fault helper
paths. Its passing duration was 9.314 s and produced launcher exit 1 with
`VMHost terminated by SIGSEGV`; its timeout duration was 13.961 s with
`generated_fault_inner_timeout`. In the timeout run, launcher stdout, stderr,
and both child log files were empty; the recorded process group was empty after
cleanup.

## Frozen diagnostic set and attempts

The diagnostic build added only environment-gated trace output. It did not
change control frames, signal handling, the generated instructions, or either
deadline. Markers go to stderr, include monotonic uptime nanoseconds, and flush
before execution proceeds. They cover helper spawn, HELLO/READY/RUN, RUN_ACK
and result waits, JIT mapping/code readiness, generated entry, socket error,
and child wait/signal classification. Without `OD_ISSUE027_TRACE=1`, they emit
nothing.

Frozen-set manifest:
`work/stage3-harness/issue027-diag-20261030T032010Z-24c79bbd/frozen-set.json`.
The static library was copied from the preserved pass build after confirming it
matched the timeout build; its original revision was
`8e0a2b289869a224917c8188cbb04cd5edbdfc93` with source fingerprint
`09b0c2e526b63ad1e13365c8cbbfa136235988a8a6a4508a2af14d6ce11a45b2`. Only the
diagnostic launcher and generated-fault helper were rebuilt for this set. They
were compiled once with these source and toolchain inputs:

- Revision `b842d261f115f160845b55d4b3d59c326aa18203`.
- Source fingerprint before and after build and diagnostic attempts:
  `af4e1ff8b07a80ba96366647fe5c4918a1be8b85abe117bc894e6d3e4ce1479c`.
- Library `libod_host.a` SHA-256:
  `9ea908b5e663f6487a240146b677d89a7dc5f3f88e333ab032ed3016bfaab2f5`.
- Diagnostic launcher SHA-256:
  `a2b7eae2ea842264b953601c7d0e29ac750878662d5417c94d923223a2ca8323`.
- Diagnostic generated-fault helper SHA-256:
  `cf8b7692f33812ea1a67827ec17dcd01c1a51a2d28c1fa03d733a43a0f110901`.
- `/usr/bin/codesign` SHA-256:
  `3635f18db517033a4786ea7ddb0acc1801e642eaf6171e208e24cb9a1922f518`.
- Entitlements source SHA-256:
  `958648f799e436860b51eaf55ec8f92d2c62da17001e23d96bc05ffc748f2a2a`.

The exact build/sign/verify argv, outputs, durations, signature details, and
entitlement dumps are in the frozen-set manifest and its `logs/`. The launcher
was signed with runtime options and no entitlements; the helper was signed with
the existing VMHost smoke entitlements. Both were made read-only before the
attempts, and hashes were checked before and after each attempt.

Each diagnostic command used the same frozen binary paths and the existing
5-second smoke deadline and 30-second outer process-step budget:

```sh
/usr/bin/env OD_ISSUE027_TRACE=1 python3 tests/system/vmhost_generated_fault_smoke.py \
  <frozen-launcher> <frozen-generated-fault-helper> --diagnostics-dir <attempt-child-logs>
```

All three attempt records are separate and immutable:

| Attempt | Result | Exit | Duration | Record |
|---|---|---:|---:|---|
| 1 | expected SIGSEGV observation | 0 | 9.980 s | `work/stage3-harness/issue027-diag-20261030T032010Z-24c79bbd/attempts/01/attempt.json` |
| 2 | expected SIGSEGV observation | 0 | 9.371 s | `work/stage3-harness/issue027-diag-20261030T032010Z-24c79bbd/attempts/02/attempt.json` |
| 3 | expected SIGSEGV observation | 0 | 9.468 s | `work/stage3-harness/issue027-diag-20261030T032010Z-24c79bbd/attempts/03/attempt.json` |

Aggregate record:
`work/stage3-harness/issue027-diag-20261030T032010Z-24c79bbd/diagnostic-attempts.json`.
Every attempt saved stdout/stderr, launcher child stdout/stderr, full command,
exit status, duration, source fingerprint after execution, cleanup status, and
the frozen artifact hashes. No run was skipped after a pass; no build occurred
between attempts.

In all attempts the helper received HELLO, sent READY, received RUN, mapped and
prepared the JIT page, and reached `generated_call_begin`. No
`generated_call_returned` marker appeared. The launcher was waiting for RUN_ACK
when the helper closed the control socket; `waitpid` then reported signal 11,
and the launcher classified it as `VMHost terminated by SIGSEGV`. Each trace
therefore localizes the expected path through handshake and generated entry,
then through control-socket EOF and child reaping. It does not localize the
earlier timeout: that run predates markers, had empty logs, and was not
reproduced by these three attempts. The suite remains intermittent by history;
three diagnostic passes do not establish stability or a fix.

## Validation and scope

- Independent review found no control-flow or signal-handling changes in the
  opt-in markers.
- Stage 3 harness self-tests passed 26/26; Swift launcher/helper compiled and
  were signature/entitlement verified in the frozen-set build.
- No full VMHost process suite, Mini, Stage 2, physical, or performance run was
  performed for this diagnosis.
