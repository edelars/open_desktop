# S2-42n — bounded MOV qword memory reducer

## Scope

This slice adds an interpreter callback reducer for one exact schema-v2
`s2-long-integer-v1` Long64 instruction: bytes `48 89 03`, `MOV qword ptr
[RBX], RAX`. The code mapping is one RX region of exactly three bytes. A single
non-executable RW data region is fully observed, and RBX points at an eight-byte
destination with at least one canary byte on both sides. The fixture has a
one-instruction budget and stops at `entry+3`.

The reducer may change RAX only. Code, mapping bases and lengths, permissions,
EA/RBX, masks, flags and definedness, all other initial state, profile, budget,
and stop address remain fixed. It does not change CPU behavior or schema and
does not broaden native admission.

## Oracle and mismatch preservation

The independent oracle copies the original data bytes and writes the candidate
RAX value to the eight bytes at `RBX - data_base` in little-endian order. It
predicts unchanged GPRs and RFLAGS, RIP at `entry+3`, one retired instruction,
the complete code region and complete data region including canaries. It does
not invoke the decoder or interpreter.

A callback preserves a mismatch only when the result identifies the exact
fixture digest, interpreter backend and profile; reports successful stop at
the fixed boundary, retirement count one, an explicit complete snapshot and
no exception; and matches the independent result in every observed field
except one or more data-byte offsets inside the destination qword. The
fingerprint is that ordered set of destination-relative byte offsets. A code,
canary, register, defined-flag, layout, exit, retirement or observation-scope
mismatch is rejected. Each candidate gets a newly calculated oracle result
and must preserve the same fingerprint.

The original, every candidate and the final accepted value each require two
identical full callback results. RAX bits are proposed from bit 63 down to bit
0, clearing set bits only. The actual callback cap is 132: two initial calls,
at most 64 proposals with two calls each, and two final calls. A lower user
limit is allowed from 4 to 132. If the full proposal schedule cannot fit, the
run records budget exhaustion and publishes no reduced fixture. Callback
errors, unsupported/fault outcomes, wrong backend, missing snapshots, drift,
flaky pairs, and failed final confirmation likewise cannot publish a reduced
fixture.

## Invocation and evidence

```sh
cargo run -p od-runner --offline -- \
  --reduce-mov-memory \
  tests/fixtures/x86/stage2/s2_42n_mov_qword_rbx_rax.case.json \
  [--limit N] [--output-dir work/stage2/s2-42n-mov-memory-reducer/<run-id>]
```

The default budget is 132. The output path must be a fresh child under
`work/stage2/s2-42n-mov-memory-reducer/`. Evidence includes the exact original
input bytes and digest, independent expected results for every distinct
candidate, every callback result and journal record, candidate files and
fixture/result/expected digests, mismatch fingerprints, budget counters, provenance, runner
revision/toolchain, and original/reduced/reducer replay argument vectors and
working directory. Native and physical execution are explicitly `not_run`.

The literal fixture passes local interpreter execution and the reducer reports
`no_initial_mismatch` after two calls. Synthetic stable-mismatch tests prove
the RAX-only reduction path and exact destination-offset fingerprint; drift,
callback failure, flaky initial results, budget exhaustion, and saved original
replay are covered separately. Synthetic callbacks are orchestration evidence,
not evidence of a CPU defect. This slice does not close the larger S2-42
acceptance or the S2-21, S2-39, S2-40, native, or physical requirements.
