# S3-35b clean run environment record

Recorded from the command/process-check outputs of the clean three-run
sequence on 2026-10-31. The cargo commands were executed directly in separate
shell invocations, without redirection. Each command returned exit code 0 and
produced the matching `process_run` JSON report. The exact build/test commands
are repeated in [the S3-35b report](../../../STAGE_3_S3-35b_2026-10-31.md).

| Run | Before run | During run | Command exit |
|---|---|---|---:|
| 1 | Process scan found no other cargo/rustc, Stage 2 runner, machine, or bench process. `top -l 3` at 10:42:25: 90.86% idle; load 1.70/2.41/3.05. | Process scan about 20 seconds into the sample found no other cargo/rustc or Stage 2 process. | 0 |
| 2 | Process scan found no other cargo/rustc, Stage 2 runner, machine, or bench process. | Process scan about 20 seconds into the sample found no other cargo/rustc or Stage 2 process. | 0 |
| 3 | Process scan found no other cargo/rustc, Stage 2 runner, machine, or bench process. | Process scan about 20 seconds into the sample found no other cargo/rustc or Stage 2 process. | 0 |

The process scans filtered for `cargo`, `rustc`, `od-x86-runner`, `od-runner`,
`od-machine`, and `od-bench`; only the process-list probe and its `rg` filter
appeared. The shell tool's returned process exit status was 0 for all three
commands. The process-list stdout and command logs were not separately
redirected at run time; this file records their returned observations after the
run, rather than claiming to be a raw OS log.

An earlier provisional set was kept as
`work/tmp/stage3/s3-35/pipeline-initial-inconclusive-{1,2,3}.json` and excluded.
A Stage 2 test was noticed in that earlier window; it was not stopped. The
clean set above was run only after that process exited.
