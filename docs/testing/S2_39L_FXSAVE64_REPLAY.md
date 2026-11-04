# S2-39l — offline interpreter replay against saved FXSAVE64 evidence

**Status:** implemented as an opt-in, offline replay. It reuses the validated
S2-39k Linux A/B/A/B evidence and does not run native instructions. The evidence
remains raw and unchanged at its explicit input path. This slice does not change
S2-39j's native observation contract or close parent S2-39.

## Input and opt-in behavior

Set `OD_S2_39L_EVIDENCE_PATH` to the saved S2-39k
`raw-evidence.json`. The focused integration test reads this file in place and
first parses the strict evidence schema and calls the independent
`od_testkit::fxsave64::compare_fxsave64` comparator. It proceeds only when the
comparator accepts an `observed` record with four A/B/A/B runs. Missing, invalid,
rejected, or incomplete evidence fails the opt-in test.

With the variable unset the test returns without opening an artifact. No normal
test or build requires ignored files under `work/`. Example invocation:

```sh
OD_S2_39L_EVIDENCE_PATH=/absolute/path/to/raw-evidence.json \
  cargo test -p od-runner --test fixture_v2 \
  s2_39l_opt_in_offline_interpreter_replay_matches_saved_linux_evidence --offline
```

## Replay and comparisons

Each repetition creates fresh input from the existing exact
`s2-long-fxsave64-rax-disp8-v1` fixture. A uses MXCSR `0x1f80`, XMM bytes
`00..ff`, and an `A5` image canary; B uses MXCSR `0x7f80`, reversed literal
XMM bytes, and a `5A` image canary. The replay order is A/B/A/B. The interpreter
must report exactly one completed instruction and reach the fixture's E+5 stop
address; all modeled CPU state must equal its initial state except RIP.

For the interpreter-produced 512-byte image, compare against the matching
validated native run only at these half-open ranges:

- MXCSR `[24,28)`;
- XMM0–XMM15 `[160,416)`;
- software tail `[464,512)`.

The existing independent comparator separately checks the native run's
unchanged left/right guards, pre/post XMM and MXCSR observations, image
canaries, four-run ordering, successful exit, and marker progression. It
validates raw INT3 RIP `E+6` and the protocol's one-byte adjustment to
architectural E+5. The replay does not compare host code addresses because the
interpreter and native runs execute in different processes.

Never compare all 512 image bytes. Host MXCSR_MASK `[28,32)` is retained and
validated as raw evidence but is not compared to the interpreter's synthetic
target mask. x87 fields/pointers, reserved and unused ranges (including
`[416,464)`), byte 5, and x87 slot padding are excluded. Excluded-byte changes
must not create a replay mismatch.

## Failure coverage and limits

The independent comparator regression corpus covers changed compared bytes,
changed guards, incorrect completion/RIP, malformed or incomplete evidence,
and excluded ranges including a raw host mask of `0xffff`. The replay uses the
saved S2-39k evidence, not a new native run. A passing local replay establishes
only that these existing interpreter results agree with the selected defined
bytes and progression in that saved evidence. It does not establish physical
execution, Linux virtualization status beyond the raw evidence metadata, x87
behavior, fault behavior, FXRSTOR, JIT support, or full-system integration.

See [S2-39k's native protocol](S2_39J_FXSAVE64_ORACLE.md), the
[S2-39k acceptance report](../reports/S2_39K_LINUX_ACCEPTANCE.md), and the
[S2-39l implementation report](../reports/S2_39L_INTERPRETER_REPLAY.md).
