# S2-38p — MAXSD XMM0,m64 [RAX] with unmasked IM/DM exits

## Result

Added the interpreter-only schema-v2 profile
`s2-long-maxsd-memory-unmasked-rax-v1` for exact Long64 `F2 0F 5F 00`.
The profile keeps masked S2-37cx memory MAXSD and register S2-38h MAXSD
separate, fixes CR0.MP to one, and uses its own memory control selector. The
memory path checks complete fetch and feature/control gates before TS, rejects
AC locally before checked canonical span validation and one read8, then
classifies the raw operands. It uses MAXSD's raw source-2 selection and
unmasked IE/DE sticky priority, preserving XMM0 and retirement on an arithmetic
exit. JIT and native execution remain denied.

A strict raw-u64 schema oracle independently determines MAXSD selection,
exception status, expected state, retirement, and the selected architectural
exception. The handwritten fixture
`s2_38p_maxsd_qnan_im_unmasked_xm.case.json` exercises an unmasked QNaN/IE #XM
exit and verifies XMM0 suppression plus the dedicated MAXSD control echo.

## Verification

The runner test crosses IM×DM×OSXMMEXCPT for the QNaN case and all four RC
values with 64 prior sticky histories. It does not yet cover the full
NaN/subnormal/tie space or injected memory/fetch rollback cases. The complete
affected suites passed: od-isa 1,901; od-testkit 455; runner 125 unit + 523
fixture tests. Workspace all-target offline check, formatting, diff check, and
Graphify refresh passed. Layout validation reports the unmapped root `.git`
entry and two broken manifest links in the shared stage report; no physical
x86 acceptance is claimed.

See the [CPU contract](../specs/CPU_CONTRACT.md#s2-38p--exact-maxsd-unmasked-memory-exit-for-m64-rax), [plan](../plans/STAGE_2.md#s2-38p-exact-long64-maxsd-xmm0m64-rax-memory-profile-with-mixed-imdm-exit), and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-38p-maxsd-m64-unmasked-invalid-and-denormal).
