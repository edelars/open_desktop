# S2-38s — MINPS XMM0,m128 [RAX] with unmasked IM/DM exits

The dedicated schema-v2 profile `s2-long-minps-memory-unmasked-rax-v1`
implements the exact three-byte Long64 form `0F 5D 00`. Admission requires SSE,
CR0.EM=0, CR4.OSFXSR=1, and explicit CR0.TS/CR4.OSXMMEXCPT controls; SSE2 is not
required. The profile is separate from masked S2-37dc memory MINPS and unmasked
S2-38k register MINPS. Only interpreter execution is supported; JIT and native
backend paths deny the profile.

The interpreter rejects prefixed or otherwise non-exact forms before the SSE
prerequisite #UD and TS #NM gates. It accepts supported MXCSR bits `0xffbf`,
requires reserved bits clear, DAZ/FTZ off and ZE/OE/UE/PE masked, while IM, DM,
RC and prior sticky bits vary. It checks 16-byte alignment and the canonical48
`[RAX,RAX+15]` span before one all-or-error 16-byte memory read. Alignment,
canonical-span and read faults leave CPU state unchanged.

All four lanes use raw MINPS selection with the memory operand as source 2;
ties and NaNs select the raw memory bits. Invalid and denormal events are
collected across lanes. An unmasked event sets all raw IE/DE sticky flags and
reports #XM or arithmetic #UD according to OSXMMEXCPT while preserving the
entire XMM0, RIP and retirement count. A masked completion commits all lanes;
DE status is suppressed only for a lane that also raises IE.

The independent fixture oracle consumes the literal memory RHS rather than
reusing XMM1 or interpreter results. Three named fixtures cover raw NaN and
signed-zero ties, upper-lane IE+DE #XM, and SNaN arithmetic #UD. Executor tests
cross all four lane classes with IM, DM and OSXMMEXCPT (2,048 cases), plus each
RC with all 64 prior sticky histories. Fixture checks cover exact encoding,
strict controls and snapshots, output tampering, and JIT/native denial.

## Verification

- `cargo test -p od-isa --offline`: 1,918 passed.
- `cargo test -p od-testkit --offline`: 455 passed.
- `cargo test -p od-runner --test fixture_v2 --offline`: 529 passed.
- `cargo test -p od-x86-runner --offline`: 8 library and 5 binary tests passed.
- `cargo check --workspace --all-targets --offline`: passed.
- `cargo fmt --all -- --check`: passed.
- `git diff --check`: passed.
- `graphify update .`: completed; 12,012 nodes, 25,949 edges and 397 communities.
- `python3 tools/check_layout.py`: did not pass in the isolated worktree because
  it treats the worktree `.git` pointer as an unmapped root file and the existing
  shared report links two oracle manifests ignored in this checkout. No layout
  or shared report files were changed for this slice.

The Linux-only native execution preflight was not available on this macOS host.
Physical x86 acceptance is not claimed.
