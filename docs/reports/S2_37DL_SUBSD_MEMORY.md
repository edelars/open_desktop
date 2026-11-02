# S2-37dl — legacy Long64 SUBSD XMM0,m64 [RAX]

This bounded slice implements one strict schema-v2 local interpreter route for exact legacy `F2 0F 5C 00`. It fetches four bytes, retires once, and stops at entry+4. Long64/SSE2 (`CPUID.01H:EDX[26]`), CR0.EM=0, CR4.OSFXSR=1, CR0.TS=0, and XMM/MXCSR state are required. Fetch completes before gates; SSE2=0, CR0.EM=1, or CR4.OSFXSR=0 produces #UD before CR0.TS=1 produces #NM. Other encodings, lengths, addressing forms, destinations, JIT/native/C ABI/generic/fragment routes and physical acceptance remain closed.

MXCSR is restricted to supported mask `0xffbf`, reserved bits clear, all six exception masks set, DAZ=FTZ=0. Every rounding control and prior sticky history is accepted. AC=1 is local Unsupported before address validation or memory access; AC=0 permits unaligned m64. Checked `RAX+7` and both canonical48 endpoints precede exactly one read8. Read failure preserves all CPU/SSE state and retirement.

Execution subtracts the loaded memory RHS from old XMM0.low64 using the existing raw binary64 subtraction path. The original operands are classified and NaNs selected before RHS sign treatment; first original NaN wins, SNaNs are quieted and raise IE, and generated IE/DE/OE/UE/PE bits are merged with prior stickies. Only XMM0.low64 is written; high64, XMM1, unrelated registers, flags, and RAM are preserved.

The handwritten fixture `s2_37dl_subsd_m64_halfway.case.json` uses 1.0 minus 2^-54, with XMM0.high64, XMM1, and x87 canaries. The raw-u64 `independent_subsd` oracle receives memory RHS and uses neither host floating point nor production arithmetic helpers. The runner matrix covers all 256 pairs drawn from 16 raw operand classes × four RC values × 64 prior sticky states (65,536 executions). It asserts StopAddress, snapshot availability and retired=1 before oracle validation. Additional tests cover both NaN orders, ties/tiny/overflow, fetch/gate ordering, invalid MXCSR runtime and schema, AC, unaligned access, canonical endpoint and checked overflow, exactly one read, read rollback, provenance/control/result tampering, backend denial, and SUBSD/SUBSS/ADDSD register regressions.

## Verification

- Focused decoder/direct ISA tests: 4 passed.
- Focused runner matrix: 1 passed, covering 65,536 independent comparisons in 27.56 seconds.
- Literal fixture/schema/provenance/backend-denial test: 1 passed.
- Full `od-isa`: 1,806 passed; `od-testkit`: 455 passed.
- Full runner: 115 unit tests and 493 fixture tests passed.
- Full `od-x86-runner`: 8 library tests and 4 binary tests passed.
- `cargo check --workspace --all-targets --offline`, `cargo fmt --all -- --check`, and `git diff --check` passed.
- `graphify update .` passed and rebuilt 11,315 nodes / 24,334 edges / 346 communities.
- `python3 tools/check_layout.py` reported only the isolated-worktree `.git` unmapped root marker and two pre-existing absent `work/` oracle manifests; documentation links for this slice resolve.
- Independent Sol/high review completed with no remaining findings. No physical x86 acceptance is claimed.

Normative references: Intel SDM Rev. 093 Vol. 2B `SUBSD` p. 4-693; Vol. 2A legacy SSE2 rows in Table 2-27; Vol. 1 §§4.9.1.4–.6, 11.5.2–11.5.3 and DAZ/FTZ; Vol. 3A Event 19/Table 7-8. See the [official Intel SDM downloads](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html).
