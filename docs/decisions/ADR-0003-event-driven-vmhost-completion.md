# ADR-0003: Wake VMHost on worker completion

**Date:** 2026-10-22. **Status:** accepted for the one-fragment Stage 0 VMHost.

## Question

How should the VMHost control loop notice that its guest fragment worker has completed while remaining responsive to a STOP request?

## Context and measurements

The first VMHost implementation polled the control socket with a 50 ms timeout and checked worker completion after each wake. On the primary MacBook Pro with Apple M3 Max, 36 GB RAM, macOS 26.7.1 build 25G241, 20 fresh ad-hoc signed VMHost processes each ran the same one-instruction `MOV RAX,1` fragment. All results were correct. Median RUN→RESULT was 51.334 ms [51.159–51.609].

After the completion-signal change, the same 20-run pilot returned the same architectural state and measured 138.1 μs [114.5–479.7] median RUN→RESULT. This is about 372 times lower in this pilot. A separate busy-helper test delivered cooperative STOP in 24.6 μs [18.5–44.8] to STOP_ACK and 32.1 μs [22.5–67.3] to RESULT across 20 fresh helpers; every result reported `STOP_REQUESTED` and retained its initial RAX. The launcher smoke and two-process isolation smoke also passed after the change.

The timings are short single-session pilots with uncontrolled background load. RUN_ACK includes C ABI fragment creation, decode/compile/publication and worker startup; RUN→RESULT includes guest execution and control-loop wakeup. These are integration timings, not isolated IPC costs. The post-change spawn-to-ready series contained one 574 ms outlier and is not used for a decision. Raw samples and environment metadata are in the ignored local directory `work/stage-0/2026-10-22-e04-vmhost-abi-ipc-resource/`.

## Options

1. Keep periodic 50 ms socket polling. It is simple, but the measured short-run completion waited for the next poll.
2. Wait for worker completion only. This would not service STOP on the control socket while the worker is running.
3. Wait on both the control socket and a worker completion signal.

## Decision

Use a completion pipe per fragment worker. `poll()` waits on both the inherited control socket and the pipe's read descriptor without a periodic timeout. The worker writes one notification byte after publishing its final result. The control socket continues to carry STOP while execution is active.

## Consequences

- Short completed runs do not wait for a fixed poll interval.
- STOP and worker completion share the existing blocking control loop, without polling the worker state periodically.
- The change adds one small pipe to the one-shot VMHost fragment lifecycle.
- These measurements do not establish latency, memory or throughput budgets for persistent or multi-VM execution.
- Revisit the completion mechanism when VMHost supports multiple fragments or a persistent machine lifecycle; benchmark under controlled load before changing hot-path policy.

## Verification

- Swift VMHost rebuilt and ad-hoc Hardened Runtime signature verified.
- 20/20 post-change normal runs returned `RAX=1`, `RIP=10`, exit reason 0 and one instruction.
- 20/20 post-change busy-helper STOP runs returned the original `RAX` and `STOP_REQUESTED`.
- Launcher smoke returned the expected result; the two-process isolation smoke confirmed that the surviving helper still accepts STOP after the other is killed.
- Full report: [Stage 0 CPU and integration baseline](../reports/STAGE_0_CPU_BASELINE_2026-10-22.md).
