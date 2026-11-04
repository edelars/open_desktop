# S2-42j — bounded Linux native PUSH/POP acceptance, 2026-11-04

## Result

The existing native-v2 stack reducer ran against two exact S2-17a fixtures on
Linux x86_64:

| Fixture | Native callbacks | Result | Saved original replay |
|---|---:|---|---|
| `s2_17a_push_rax.case.json` | 2, identical | `no_initial_mismatch` | `native_x86` / `stop_address`, exit 0 |
| `s2_17a_pop_rsp.case.json` | 2, identical | `no_initial_mismatch` | `native_x86` / `stop_address`, exit 0 |

Both reducers recorded zero candidate proposals/pairs, zero accepted candidates,
and no reduced fixture. This confirms bounded native execution and saved replay
for these literal cases; it does not exercise mismatch reduction or establish a
CPU defect.

## Provenance

- Host: `senya@192.168.1.39`, Linux x86_64, Intel Core i5-3210M,
  kernel `7.0.0-38-generic`.
- `execution_kind` is `unknown`; separate physical Mac acceptance remains
  `not_run`.
- Remote checkout HEAD was `0a0640b5a7d54b13b3024146b5c9068f0f15bb5d` and was
  dirty. The relevant `od-testkit` reducer and native runner source files
  matched the local main source hashes listed in the verification record.
- The debug runner binary SHA-256 was
  `d58d1d4e89921515bf69e6cdcdc9618f188d6f920233b341613d202ed72689d4`.
  Its embedded build revision/compiler were unavailable; the evidence is bound
  to the exact binary and source hashes, not a clean historical build claim.

## Commands and evidence

The reducer command for each fixture was:

```sh
cargo run --offline -p od-x86-runner -- \
  --reduce-native-push-pop tests/fixtures/x86/stage2/CASE.json \
  --limit 4 \
  --output-dir work/stage2/s2-42j-native-stack/acceptance-20261104-RUN
```

Each saved original was then replayed with the report's recorded `--fixture
<original.case.json> --json` command to a separate result file. The two
complete reports, callback journals and replay outputs are under ignored
`work/stage2/s2-42j-native-stack/acceptance-20261104-push-rax/` and
`work/stage2/s2-42j-native-stack/acceptance-20261104-pop-rsp/`. The compact
[verification record](../../work/stage2/s2-42j-native-stack/verification.json)
contains exact commands, source/binary hashes, report hashes, callback counts,
and replay results.

This bounded result does not close S2-42j reducer mismatch behavior, parent
S2-42, or the wider S2-21/S2-39/S2-40 requirements.
