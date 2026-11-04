# S2-42 Linux five-suite acceptance

**Status:** the combined command passed its fixed selection on 2026-11-04,
using the local macOS interpreter and the Linux x86_64 native backend. This is a
bounded S2-42 acceptance slice; parent S2-42 remains open.

## Command and selection

Run from the repository root:

```sh
python3 tests/cpu/run_stage2_42_linux_five_suites.py \
  --host HOST \
  --user USER \
  --remote-root /absolute/linux/checkout
```

The command runs existing strict generators and saved-run verifiers in order
for S2-42c/e/h/i/r. It writes a fail-closed `combined-summary.json` under
`work/stage2-42-linux-five-suites/<run-id>/`. The summary records selected and
unselected IDs, each child summary and manifest, counts, two-repeat status and
the shared source fingerprint. The command exits nonzero if a required child
fails, is missing, has stale evidence or differs in count/repeats/fingerprint.

## Linux result

Native host `192.168.1.39` (`senya`), checkout
`/home/senya/projects/open_desktop`; the interpreter ran locally on macOS.
Aggregate run `20261104T143258Z-ff9f411e` passed with source fingerprint
`5f710e209f2772ba9df37d8e232dc0736fb086015a7b74d4d1e9b5f30a9d7f9d`.

| Suite | Cases | Repeats | Results per backend | Saved run |
|---|---:|---:|---:|---|
| S2-42c XORPD | 1,030 | 2 | 2,060 | `stage2-20261104-143258-0c60f6a2` |
| S2-42e MOV memory | 1,080 | 2 | 2,160 | `stage2-20261104-143733-c9bef217` |
| S2-42h PUSH/POP | 1,056 | 2 | 2,112 | `stage2-20261104-144235-bfca2724` |
| S2-42i REP STOSB | 1,060 | 2 | 2,120 | `stage2-20261104-144732-7af9a091` |
| S2-42r JZ/JNZ rel8 | 1,056 | 2 | 2,112 | `stage2-20261104-145236-6bf65238` |
| **Total** | **5,282** | | **10,564 per backend** | |

Combined summary and child evidence are under
`work/stage2-42-linux-five-suites/20261104T143258Z-ff9f411e/`. All five child
statuses are `passed`; all five child manifests share the fingerprint above.

## Boundaries

The other 27 lettered S2-42 suites are listed as unselected in the combined
summary; they are not covered by this run. Physical Mac acceptance remains
`not_run`. This result does not close generic reducer requirements or parent
S2-42 / S2-21 / S2-39 / S2-40.
