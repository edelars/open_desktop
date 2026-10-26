# Stage 3 one-block comparison fixtures

These small schema-v1 programs are local additions for S3-14 and use only the
existing register-only S0-09 forms. Their `.hex` files are the exact byte
sources; the adjacent `.case.json` records bind the bytes by SHA-256.

- `s3_14_budget_at_linear_boundary`: MOV reaches the budget boundary before a
  following ADD, so the one-block result must stop at RIP 10 with one retired
  instruction and `instruction_budget_exhausted`.
- `s3_14_jnz_to_end_skips_tail`: a taken JNZ targets `code_end` over a valid
  but unreachable MOV after CMP makes ZF clear. The one-block result must
  retire CMP and JNZ only, then leave RAX unchanged.
- `s3_14_jz_to_end_skips_tail`: a taken JZ targets `code_end` over a valid MOV
  after CMP makes ZF set.
- `s3_14_jz_not_taken_backward_target` and
  `s3_14_jnz_not_taken_backward_target`: the taken edge targets the decoded
  entry while the selected not-taken fallthrough is `code_end`.
- `s3_14_live_through_helpers`: five arithmetic helper operations keep the
  first ADD result live through subsequent helper boundaries. It also covers
  aliased ADD of the signed minimum (CF and OF), aliased SUB to zero, and a
  terminal taken JZ after CMP.

`s3_14_budget_at_linear_boundary` is a local-only control case: it stops at
the budget boundary before `code_end`, so it is not part of triple physical
terminal acceptance.

The S3-14 physical comparison selection is
[`tests/cpu/manifests/stage3_s3_14_jit_block.json`](../../../cpu/manifests/stage3_s3_14_jit_block.json).
It contains eight historical single-block cases and five boundary cases;
it also records one local-only budget case. The two historical cases needing
a dispatcher are listed there with reasons.
