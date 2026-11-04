# S2-42ae — exact qword BSF/BSR register corpus

## Scope and selected contract

This local corpus covers four exact Long64 encodings, each with RAX as the
destination and either RCX or RAX as the source:

| Form | Bytes | Source |
| --- | --- | --- |
| BSF RAX, RCX | `48 0F BC C1` | RCX |
| BSF RAX, RAX | `48 0F BC C0` | RAX (alias) |
| BSR RAX, RCX | `48 0F BD C1` | RCX |
| BSR RAX, RAX | `48 0F BD C0` | RAX (alias) |

The selected architectural reference is Intel's [64 and IA-32 Architectures
Software Developer's Manual, revision 093, Volume 2A](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html),
BSF pages 3-107–3-108 and BSR pages 3-109–3-110. A zero source preserves the
destination and sets ZF. A nonzero source writes the least- or most-significant
set-bit index and clears ZF. PF reports even parity of the source; CF, OF, SF,
and AF are cleared. All arithmetic status bits in mask `0x8d5` are defined;
undefined arithmetic mask is zero. The remaining RFLAGS bits are preserved.

The repository source agrees with this contract: `Operation::BitScan` in
`crates/od-isa/src/execute.rs` computes the result and flags, while
`crates/od-isa/src/long_decode.rs` declares the defined and undefined flag
masks. Native admission in `tools/x86-runner/src/native_v2.rs` admits a bit-scan
source read and its existing admission test asserts `defined=0x8d5` and
`undefined=0`.

There is a historical documentation mismatch: the S2-15d section in
`docs/reports/STAGE_2_REPORT.md` says the other arithmetic flags are undefined,
but also cites Intel SDM revision 093. Its own S2-15 fixture provenance records
PF, clears CF/OF/SF/AF, and uses a full `0xcd7` definedness mask. This slice uses
the Rev.093 defined-flags contract corroborated by the current decoder,
interpreter and native-admission code. The old report paragraph should be
corrected when the shared Stage 2 report is next integrated; no undefined flag
semantics are inferred here.

## Inputs and independent oracle

Each exact form has eight pinned controls: zero, one, bit 63, bit 31, all ones,
both alternating patterns, and a mixed multi-bit value. The two alias forms
exercise the source-before-destination read behavior because their source and
destination are the same RAX. Controls vary incoming defined flags so the
`0x8d5` update and preservation of other RFLAGS bits are observable.

Four fixed seeds generate 256 cases per seed per form (1,024 unique full input
states per form). The versioned generator uses only Python integer operations
as its oracle: the trailing set-bit index for BSF, the bit length minus one for
BSR, zero-source destination preservation, source parity, and the specified
flag update. It imports no decoder, interpreter, native executor or production
bit-scan implementation.

Expected snapshots include all 16 GPRs, full RFLAGS and definedness, RIP, code
bytes, normal stop outcome, one retired instruction and no exception. The
manifest fixes the ordered exact case list and SHA-256 of each input and
expected snapshot. Strict verification rejects duplicate JSON keys, altered or
missing controls, unexpected files, incomplete generated sets and digest
mismatches.

## Local execution and replay

```sh
python3 tests/cpu/generate_stage2_bsf_bsr_qword_corpus.py --generate
python3 tests/cpu/generate_stage2_bsf_bsr_qword_corpus.py --verify
python3 tests/cpu/test_stage2_bsf_bsr_qword_corpus.py
python3 tests/cpu/generate_stage2_bsf_bsr_qword_corpus.py --run-interpreter
python3 tests/cpu/generate_stage2_bsf_bsr_qword_corpus.py --verify-results
```

Generated corpus and two-repeat interpreter evidence are saved under
`work/stage2/s2-42ae-bsf-bsr-qword-corpus/`. Records bind fixture digests,
runner hash, source revision, build and host provenance, exact command and
working directory, selected environment, stdout, snapshot and repeat identity.
Interrupted execution saves completed attempts and lists missing repeats.
Native evidence is `not_run/not_selected`; this slice does not add a native
suite or make a hardware-acceptance claim.

## Local result

The generated bundle has 4,128 cases: eight pinned controls and 1,024
generated cases for each of the four exact forms. All 8,256 interpreter
executions passed, with zero failures or missing attempts. Strict bundle and
saved-result replay verification passed. The corpus digest is
`f0f54741d86ebfa0e00ebb3b270f5bbb7e3d5ab94d79ced13498261346a9477f`;
the ordered manifest SHA-256 is
`b43c00db779980a22f8e287249843da37657d6a408d441b710789a093ff6f9dc`.
Saved interpreter evidence is at
`work/stage2/s2-42ae-bsf-bsr-qword-corpus/interpreter-results.json`; it records
the source revision and runner SHA-256 and is accepted only when both match the
saved replay context. Native execution is `not_run/not_selected`; physical
execution is `not_run`.

Focused generator tests (six tests), runner offline build, corpus verification,
saved-result replay verification and `git diff --check` passed. The isolated
assigned worktree made `tools/check_layout.py` report its `.git` file as
unmapped; after transfer, `python3 tools/check_layout.py` passed on main
(`Layout OK: 48 required directories, 6 crate scaffolds, 14 instruction scopes`).
No root-level files or layout entries were added. `graphify update .` completed
(14,597 nodes, 34,883 edges, 371 communities); its output is ignored by Git.

This corpus is limited to the four exact forms above and does not close
S2-15d, parent S2-42, physical acceptance or broader BSF/BSR coverage.
