# S6-08 native Linux calibration

## Run

- Date: 2026-11-13.
- Source: tracked repository snapshot from Codex commit `fa74b1c`; the isolated Mini checkout records that origin in `S6_SOURCE_COMMIT`. Because `git archive` does not carry `.git`, the checkout has a synthetic clean snapshot commit `63e5512ac4bea98618ec1784da0e6f446e0ee7f0`; this is the revision embedded in the binary metadata.
- Transfer archive: 188,252,160 bytes uncompressed; 26,043,977 bytes gzip-compressed; SHA-256 `a5f4483a585497f82908fab3a7c6a96c69cc7eff199ef1fca88c2f4d5ea08e4c`.
- Host: Mac mini, Intel Core i5-3210M, Linux x86_64, kernel `7.0.0-42-generic`; 2 physical cores / 4 logical CPUs. All two-worker cases selected logical CPUs 0 and 1 on separate physical cores (`worker_smt_sharing=false`). Four-worker IRIW selected CPUs 0, 1, 2, and 3 and reported SMT sharing (`worker_smt_sharing=true`), since the host has only two physical cores.
- Toolchain/build: rustc `1.99.1 (58cf8f9c3 2026-10-13)`, target `x86_64-unknown-linux-gnu`, release profile, Cargo profile defaults, no extra rustflags.
- Executable SHA-256: `85493676f6121d492c91e6805ac72ae6bff0430fe8241b6a12ab363404fc09ff`.
- Source SHA-256 from the runner: `606493597635615b07f332b22b120f3fba5515823777ff17b520aff020fe7bad`.
- Corpus SHA-256: `54fbd672bef0780273208fcb60c0749f81c87cef7444bb34dcacf64b186b8af8`.
- Command: `target/release/od-x86-runner --litmus-corpus tests/fixtures/guest/stage6/litmus --iterations 10000 --counter-n 2 --counter-k 2 --json --output work/stage6/s6-08-calibration.json`.

## Results

All 18 execution cases ran in three fresh child processes each. Each of the 54 process histograms summed to 10,000 observations; aggregate work was 540,000 rounds. Every observed outcome was in its declared x86-TSO set. The SB `00` outcome (`0,0,1,1`) appeared once in 30,000 rounds. `LOCK-INC` produced only 4 across 30,000 rounds; the unlocked increment produced 2 and 4, both in the model's `{2,3,4}` set. This is a calibration run, not the S6-09 full budget.

Per-process throughput samples: minimum 47,160, median 83,592, maximum 98,115 rounds/s; pooled rate 79,023 rounds/s. The 10,000,000-round candidate would take at least 212 seconds for one process at the slowest observed rate, exceeding the runner's 30-second CPU and wall limits. The proposed S6-09 budget is therefore 500,000 rounds/test/process × three processes = 1,500,000 rounds/test. It projects to 10.6 seconds for the slowest observed process and 341.7 seconds for the complete 18-case corpus using the measured per-process samples; using the slowest sample for every process gives a conservative 572.5 seconds. The full S6-09 run is still pending.

Linux `cargo test -p od-x86-runner --locked`: **9 library tests and 258 runner tests passed, 0 failed**. The existing ordinary `--fixture` path also completed `s0_09_mov_imm64` with `RAX=0x8877665544332211`, `RIP=0x0a`, and status `completed`.

## Raw evidence

- [Full calibration JSON and per-process histograms](s6-08-native-calibration.json)
- [Compressed raw Linux runner test log](s6-08-linux-cargo-test.log.gz)
- [Ordinary fixture result](s6-08-ordinary-fixture.json)

SHA-256: calibration JSON `4112925614d5ba8ad202c2651c1f5ead5ef4ed601c34d10122d0dd9d03e883fc`; compressed test log `dad766a32ee231cc1246dde1c373839202c173f473e9cad6abbb788d0110ac39`; ordinary fixture JSON `7cfee3585aea67338c3c8b967b49b15172a895e2a9936fe56d26d163d083e5e2`.
