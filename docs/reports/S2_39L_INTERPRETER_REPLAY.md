# S2-39l — offline interpreter replay against saved Linux evidence

**Result:** the opt-in A/B/A/B interpreter replay passed against the unchanged
S2-39k raw evidence. Before replay, the strict schema and existing independent
`compare_fxsave64` comparator accepted the four Linux observations. The evidence
SHA-256 was
`fb323ed37f8be6a10d593284e579c4db79aa2fa12fa3f5e5d70264e4ac1b0208`, matching
the S2-39k acceptance report.

For each A/B/A/B repetition, the existing
`s2-long-fxsave64-rax-disp8-v1` fixture ran once in the interpreter and reached
E+5. Modeled CPU state matched its input except for RIP. The interpreter output
matched the corresponding saved native image at MXCSR `[24,28)`, XMM
`[160,416)`, and software tail `[464,512)`. The independent native comparator
also validated its own pre/post observations, unchanged guards, completion,
A/B/A/B order, and raw-to-architectural INT3 RIP normalization.

Raw MXCSR_MASK, x87/pointer data, reserved/unused bytes, and slot padding were
not compared. In particular, no equality assertion was made between host
MXCSR_MASK and synthetic target mask. The raw evidence was read from its
explicit path in the main checkout and not changed or copied. With
`OD_S2_39L_EVIDENCE_PATH` unset, the test reads no ignored artifact; default
tests therefore remain self-contained.

Checks run in the isolated worktree:

- Graphify query against the main checkout graph before source reads;
- `cargo fmt --all`;
- `cargo test -p od-testkit s2_39l_comparator_regression_corpus_enforces_protocol_mask_and_completion --offline` (1 passed);
- `OD_S2_39L_EVIDENCE_PATH=/Users/senya/projects/open_desktop/work/stage2/s2-39k-acceptance-20261104-01/raw-evidence.json cargo test -p od-runner --test fixture_v2 s2_39l_opt_in_offline_interpreter_replay_matches_saved_linux_evidence --offline` (1 passed);
- the same opt-in test with `OD_S2_39L_EVIDENCE_PATH` unset (1 passed without reading an artifact);
- `cargo fmt --all -- --check` and `git diff --check` passed; all new S2-39l document links and anchors resolved;
- `tools/check_layout.py` reported the isolated worktree's unmapped `.git` pointer and pre-existing missing ignored `work/` artifact links; it reported no S2-39l link;
- Graphify update completed in the isolated checkout (14,926 nodes, 35,823 edges);
- independent `gpt-6.1-sol` high review found no concrete defects.

This is a local interpreter-to-saved-evidence cross-check, not additional
hardware execution or physical-host acceptance. `execution_kind=unknown` is
preserved and parent S2-39 remains open. See the [protocol](../testing/S2_39L_FXSAVE64_REPLAY.md),
[Stage 2 plan](../plans/STAGE_2.md#s2-39l-offline-interpreter-replay-against-the-saved-native-evidence),
and [S2-39k acceptance](S2_39K_LINUX_ACCEPTANCE.md).
