# Stage 3 S3-38h: VMHost Control V1 repeated RUN process smoke

Status: partial; the process smoke, build/sign path, and fail-closed process-harness gate pass.

## Scope

`tests/system/vmhost_control_v1_same_code_smoke.py` launches one built VMHost
and sends two sequential RUN frames over one local socket pair. Both invocations
use the same immutable two-instruction fragment `48 03 C3 48 03 C3` (`ADD
RAX,RBX` twice) but different CPU snapshots, budgets, and increasing request
IDs. The first one-instruction run returns RAX `0x55` at `A+3` with RFLAGS
`0x206`; the second two-instruction run starts from fresh input and returns RAX
`0x1E9` at `A+6` with RFLAGS `0x202`. Other GPRs are preserved. The smoke checks
READY, exact RUN_ACK and full RESULT payloads in order, then closes the control
connection while idle and requires a successful VMHost exit.

The Stage3 process suite pins the command, the smoke and ABI source paths, the
VMHost binary inputs, timeout, and success marker. Its Swift compilation uses
the VMHost bridging header that imports both public host and session ABI
headers. Two private typed inline accessors bridge the public `UINT32_C` and
`UINT64_C` ABI macros into Swift without changing public header values or
runtime behavior. The smoke fixture uses decoder-supported ADD instructions;
an initial MOV fixture was correctly rejected by bounded session admission as
`INVALID_CODE` and was replaced without widening admission.

## Review and execution

Independent Sol/high review passed for the typed bridging accessors and the
supported-instruction smoke fixture. `cargo build --release -p od-host --offline
--target-dir /tmp/s3-38h-run/build-target` and the Swift VMHost compile passed.
Ad-hoc runtime signing, strict signature verification, signature inspection,
and entitlement inspection passed; `com.apple.security.cs.allow-jit` was true.
The exact process smoke printed
`VMHost control V1 smoke passed: same-code sequential RUNs with fresh state and budgets`.
The pinned `test_vmhost_process_manifest_covers_every_gate_once` self-test
passed 1/1. `graphify update .` timed out after 120 seconds; graph refresh is
unconfirmed. SSH to the documented Mini endpoint timed out before any remote
commands/tests; M1 remains deferred by the user.

The full `python3 tests/cpu/stage3_harness.py --suite vmhost-process` suite was
also attempted. The context-mode tool call timed out after 300 seconds with no
suite summary and an empty captured log; the outcome is unconfirmed, so this is
neither a pass nor a test failure.

## Remaining acceptance

The smoke currently covers repeated same-code RUN with fresh state/budget,
monotonic IDs, ACK/RESULT ordering, and idle EOF teardown. STOP during RUN, EOF
during RUN, malformed/partial frames, completion races, backpressure, descriptor
and lease teardown, and unchanged V0 process regressions still require runtime
acceptance on a supported macOS ARM host. The full 39-step VMHost process suite,
C-client layout/linking, pipe-failure injection, physical x86/Mini, full Stage
3, owner/MMU integration, and performance remain unverified. This bounded
smoke makes no broader runtime or performance claim.
