# S6-09 full physical x86 run

## Run

- Date: 2026-11-13.
- Source: tracked snapshot of Codex commit `fa74b1c`, origin marker `S6_SOURCE_COMMIT=fa74b1c`; synthetic clean snapshot commit embedded in runner metadata: `63e5512ac4bea98618ec1784da0e6f446e0ee7f0`.
- Host: Mac mini, Intel Core i5-3210M, Linux x86_64 kernel `7.0.0-42-generic`, 2 physical cores / 4 logical CPUs.
- Affinity: two-worker cases selected logical CPUs 0/1 on distinct physical cores with SMT sharing false; IRIW selected CPUs 0–3 and reported SMT sharing true.
- Toolchain: rustc `1.99.1 (58cf8f9c3 2026-10-13)`, release profile, target `x86_64-unknown-linux-gnu`, no extra rustflags.
- Executable SHA-256: `85493676f6121d492c91e6805ac72ae6bff0430fe8241b6a12ab363404fc09ff`.
- Source SHA-256: `606493597635615b07f332b22b120f3fba5515823777ff17b520aff020fe7bad`.
- Corpus SHA-256: `54fbd672bef0780273208fcb60c0749f81c87cef7444bb34dcacf64b186b8af8`.
- Command: `target/release/od-x86-runner --litmus-corpus tests/fixtures/guest/stage6/litmus --iterations 500000 --counter-n 2 --counter-k 2 --json --output work/stage6/s6-09-native-physical.json`.
- Child limits: 30 seconds CPU, 30 seconds wall time, 256 MiB address space. The command exited 0; no stderr was emitted.

## Results

All 18 execution cases ran in three fresh processes each: 500,000 iterations/process, 1,500,000/test, 27,000,000 rounds total. All 54 process histograms and all 18 aggregate histograms conserved their requested counts. Every observed outcome was in the corresponding expected x86-TSO set; there were no timed-out or failed children. Child elapsed times were 7.83–12.29 seconds, below the 30-second CPU/wall caps. Summed child elapsed was 519.592934988 seconds (about 8m40s), excluding build and parent overhead.

The 54 process-throughput samples ranged 40,678–63,885 rounds/s (median 52,440; pooled 51,964). IRIW used four workers on logical CPUs 0–3 and reported SMT sharing; two-worker cases used CPUs 0 and 1 on distinct physical cores. The SB `00` outcome (`0,0,1,1`) appeared **19 times** in 1,500,000 SB rounds. The 1,500,000-round `LOCK-INC` result was exactly `{4}`. Formula unlocked INC produced 17 outcomes of 2 and 1,499,983 of 4; the three-instruction counter produced 26 outcomes of 2, one of 3, and 1,499,973 of 4. All are within their formula oracle sets.

## Raw evidence

- [Full JSON with process-level and aggregate histograms](s6-09-native-physical.json)
- [Compressed runner stdout](s6-09-native-physical.stdout.json.gz)

SHA-256: result JSON `60a4b3650f3e6c6058727e56b5f1814e405a3fd81c47141403cb888613a0bf81`; compressed stdout `3460b93acee1f4dc77b23928412a627e061532502b0521563904f352c1c44cbb`; stderr was empty (`e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855`).
