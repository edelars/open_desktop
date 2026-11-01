# S2-29aa — direct finite binary80 square-root precision helper

## Scope

Added `sqrt_finite_to_binary80_precision(value, mode, precision)` in
`od-isa::x87`. It accepts positive, nonzero normalized finite binary80 values
at significand precision 24, 53, or 64 and returns a normal binary80 encoding
with binary80's exponent range. Unsupported precision and the existing p64
helper's rejected input domain return `None`.

The helper uses the existing exact square-root decomposition `(E,q,R)`. At
p<64, with `h=64-p`, `T=q>>h`, `L=q&((1<<h)-1)`, and `H=1<<(h-1)`, it increments
toward-positive iff `L!=0 || R!=0`, never increments toward-zero or
toward-negative, and increments nearest-even iff `L>H || (L==H && (R!=0 || T
is odd))`. At p64 the legacy nearest condition `R>q` is retained. The widened
rounded significand goes directly to the existing normal packer at precision
p. The existing `sqrt_finite_to_binary80(value, mode)` is the p64 projection;
its acceptance/rejection behavior is unchanged.

## Literal coverage

Independent expected ten-byte arrays cover all four rounding modes at p=24,
53, and 64, exact and irrational roots, p24 exact midpoint ties for even and
odd retained significands, adjacent p24 and p53 rounding boundaries, maximum
finite input and exponent carry, minimum-subnormal and negative-exponent inputs,
pseudo-denormal equivalence, p64 wrapper projection and input domain, invalid
precisions and inputs, input immutability, and a p24 result proving that direct
rounding differs from p64-then-narrow. The p53 boundary examples have nonzero
exact root remainder; no exact p53 midpoint root is asserted.

Normative basis for the x87 precision choices is Intel SDM Rev. 093, Vol. 1
§8.1.5.2 and Table 8-2, pp. 8-7–8-8 ([official Vol. 1 PDF](https://cdrdv2-public.intel.com/929351/253665-093-sdm-vol-1.pdf)). This helper does not read or parse FCW and does not claim instruction behavior.

## Validation

- `cargo test -p od-isa finite_sqrt --offline`: 7 passed.
- `cargo test -p od-isa --offline`: 1,745 passed.
- `cargo fmt --all -- --check`: passed.
- `cargo check -p od-isa --all-targets --offline`: passed.
- `cargo check --workspace --all-targets --offline`: passed.
- `python3 tools/check_layout.py`: reports only the known worktree `.git`
  pointer and missing ignored Stage 1/2 oracle manifests tracked by ISSUE-023;
  all S2-29aa local links resolve.
- Graphify query ran in the main checkout because this isolated worktree had
  no initial index. After seeding the existing index, `graphify update .`
  succeeded in this worktree: 10,961 nodes, 23,607 edges, 328 communities;
  seven uncached files were extracted.
- Independent `gpt-6.1-sol/high` review of implementation commit
  `9706ed9f008c7fea78a18cd29248b03c576ed219` found no P0–P2 findings. No
  native/JIT, Mac mini, SSH, or physical x86 verification was performed.

## Boundaries

No FSQRT decode or execution, FCW parsing/state, architectural flags or
exceptions, native/JIT/C ABI admission, physical x86 acceptance, or performance
claim was added. S2-29 and ISSUE-019 remain open.

See the [S2-29aa plan](../plans/STAGE_2.md#s2-29aa-direct-finite-binary80-square-root-precision-rounding-bounded-helper), [CPU contract](../specs/CPU_CONTRACT.md#local-s2-29aa-direct-finite-binary80-square-root-precision-helper), and [shared Stage 2 report](STAGE_2_REPORT.md#s2-29aa--direct-finite-binary80-square-root-precision-helper).
