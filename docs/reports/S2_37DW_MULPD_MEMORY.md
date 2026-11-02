# S2-37dw MULPD m128 memory profile

## Result

Implemented the interpreter-only schema-v2 profile `s2-long-mulpd-memory-rax-v1`
for exact legacy Long64 `66 0F 59 00` (`MULPD XMM0,m128 [RAX]`). It requires SSE2
and XMM/MXCSR state. Complete four-byte instruction fetch precedes the SSE2,
CR0.EM, CR4.OSFXSR, and CR0.TS gates. The profile admits supported MXCSR bits
`0xffbf`, all six exception masks set, reserved bits clear, DAZ and FTZ clear,
and any RC and prior sticky status.

The source span must be aligned to 16 bytes and canonical48 through RAX+15.
After those checks, execution performs exactly one atomic 16-byte read and
multiplies each old XMM0 binary64 lane by the matching lane from memory. It
aggregates IE/DE/OE/UE/PE, preserves old ZE and sticky status, and commits XMM0,
MXCSR, and RIP together. Gate, alignment, span, and memory read failures leave
state and retirement unchanged. Non-interpreter routes and unmasked/#XM
behavior remain unsupported.

## Evidence

The handwritten fixture's old XMM0 lanes (1.0, 2.0) and memory lanes (2.0, 5.0)
produce (2.0, 10.0), distinguishing its memory RHS from the register profile.
The result validator reads both RHS lanes from fixture data and uses the
independent raw-bit `independent_mulpd` oracle. Runner coverage exercises the
four RC modes crossed with 64 prior sticky combinations, NaN/invalid priority,
zero-times-infinity, signed zero, subnormal input/output, rounding, overflow,
tiny/minnormal behavior, mixed-lane IE/DE, schema/result/control tampering,
gate priority, alignment, and backend denial. Direct ISA tests verify exact
decode neighbors, complete fetch before gates, no read on gate/address faults,
one `read16`, and state rollback on read error.

Checks: od-isa 1,861/1,861; od-testkit 455/455; od-runner 119 unit +
508 fixture_v2; od-x86-runner 8 library + 4 binary; targeted MULPD direct ISA
3/3 and runner fixture 1/1; `cargo check --workspace --all-targets --offline`;
`cargo fmt --all -- --check`; `git diff --check`; Graphify AST update (11,628
nodes, 25,045 edges, 370 communities). `tools/check_layout.py` reported only the
known linked-worktree `.git` marker and two absent oracle manifests linked by
the shared stage report. Independent Sol/high review is pending.

No physical x86 acceptance was run. The admission intentionally excludes
unmasked arithmetic exceptions and #XM.
