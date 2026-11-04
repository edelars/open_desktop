# S2-42e: deterministic MOV memory corpus

## Scope

S2-42e adds a deterministic manifest-backed input corpus for existing schema-v2
profile `s2-long-integer-v1`. It uses only the native-admitted one-instruction
MOV loads and stores already covered by S2-09/S2-10. It does not change CPU
semantics, fixture schema, runner CLI, JIT, or native admission.

The fixed corpus has 1,024 cases: four fixed 64-bit seeds and 256 sequence
values per seed. The complete matrix is:

| Dimension | Values |
|---|---|
| Width | 8, 16, 32, 64 bits |
| Direction | register load from memory, memory store from register |
| Effective address | base, signed disp8, signed disp32, SIB, RIP-relative, address-size-32, REX.B extended base |

All cases execute one instruction, stop at the decoded next RIP, and map a
read/write data region with full-byte comparison. Fixed vectors include the
first and last valid data spans, unaligned multi-byte accesses, disp8 values
−128/−17/−1/0/1/17/127, and disp32 values −0x12345/−0x10000/0x10000/0x12345.
Address-size-32 cases use a low mapped effective address while the full RBX
contains nonzero upper bits. The manifest records the selected dimensions and
effective address for each input.

## Generation and verification

The generator writes fixtures, `expected.json`, and a strict `manifest.json`
under `work/stage2/s2-42e-mov-memory/`. The manifest binds each exact fixture
byte sequence, the complete ordered case list, and the independent expected
results by SHA-256. Verification regenerates all expected bytes, rejects
missing/extra files and symlinks, and compares the complete set byte-for-byte.
The exact CPU fixture validator remains the existing Rust runner.

When execution or saved-evidence verification is requested, the command first
writes a failed machine summary. A corpus preflight failure (including a
missing or tampered fixture or manifest) replaces any prior success summary
before returning nonzero and before starting a backend. Since the corpus did
not pass verification, that failure summary contains no `corpus_digest`.

The expected-result calculation is a separate one-instruction byte-transfer
oracle. Loads update only AL/AX, zero-extend EAX, or replace RAX according to
width. Stores copy the low source-register bytes to the effective address.
The oracle records all GPRs, next RIP, unchanged RFLAGS and definedness, exact
one-instruction retirement, both complete memory regions, and their canaries.
It uses the generator's chosen effective byte offset and does not decode or
execute the instruction.

## Native comparison command

Run the single orchestration command below from the repository root on a
development machine that can reach a supported Linux x86_64 host over SSH:

```sh
python3 tests/cpu/generate_stage2_mov_memory_corpus.py --run-oracle \
  --host HOST --user USER --remote-root /absolute/remote/checkout
```

The command generates and verifies the corpus, then calls the existing
`tests/cpu/stage2_oracle.py` with all 1,024 case files and exactly two repeats.
It compares each interpreter and remote native result with the independent
byte-transfer oracle. Any missing input/result, nonzero native process, native
host other than Linux x86_64, mismatch, or missing repeat returns nonzero. The
machine-readable run evidence remains under `work/stage2-42e-oracle/`; the
wrapper writes `s2-42e-summary.json` there on both success and failure.

To verify a previously saved run without executing either backend, use the
same independent expected results and strict corpus verification:

```sh
python3 tests/cpu/generate_stage2_mov_memory_corpus.py \
  --verify work/stage2/s2-42e-mov-memory \
  --verify-oracle-run work/stage2-42e-oracle/RUN_ID \
  --oracle-output-root work/stage2-42e-oracle/saved-check
```

The saved run manifest must bind the selected profile and exactly two repeats.
Each input record must identify schema version 2, profile, case ID, and exact
fixture SHA-256. Each repeat record must bind schema version, fixture digest,
zero interpreter/native differences, and the expected repeat-stability bit; it
also supplies build and machine identities for both backends. Every result JSON
must match its repeat's case ID, raw input digest, schema, selected profile,
backend, build, and machine. The native result is accepted only when its
recorded backend machine is Linux x86_64. The verifier also compares the full
architectural result against the independent MOV oracle. Malformed JSON,
missing results, duplicate identities, provenance mismatches, or result
mismatches produce a failed summary and nonzero exit.

Current acceptance verification requires `suite=mov-memory-corpus`; historical
generated-only `integer` evidence is available only through the explicitly
named legacy verifier and cannot satisfy this command. The saved run must also
contain exact `fixtures/` and `hand-controls/` directories with all 1,080 frozen
case files as regular non-symlink files. Each frozen byte sequence is compared
with its verified corpus or pinned control source and digest. Every result
record and backend result binds to `sha256:<source_fingerprint>` and to the
interpreter/local and native/remote Rust compiler versions recorded in the run
machine provenance.

To check only generated bytes and manifest without CPU execution:

```sh
python3 tests/cpu/generate_stage2_mov_memory_corpus.py --verify \
  work/stage2/s2-42e-mov-memory
```

## Evidence boundary

Python tests cover deterministic output, 1,024 unique inputs, the complete
width/direction/address matrix, fixed boundary vectors, mapped access spans,
manifest tampering, fail-closed preflight summaries, strict saved-evidence
identity binding, malformed JSON, and a positive saved-result roundtrip. Tests
also reject missing, extra, symlinked, or changed frozen inputs and legacy
suite fallback. The
existing interpreter and Linux x86 oracle comparison command must pass
separately before claiming native execution evidence. The corpus-generator
implementation itself does not claim Linux native execution or physical Mac
acceptance. It adds no JIT claim and does not close parent task S2-42.

## S2-42q handwritten boundary controls

S2-42q extends the same acceptance command with 56 checked-in controls before
the unchanged 1,024 generated inputs. The order is width `08,16,32,64`, then
direction `load,store`, then `base,disp8,disp32,sib,rip_relative,address32,rex_b`.
Each ID is `s2_42q_mov_{width}_{direction}_{form}`. Each fixture and complete
expected snapshot is a static JSON file under
[`s2_42q_mov_hand/`](../../tests/fixtures/x86/stage2/s2_42q_mov_hand/). Runtime
selection verifies the version-1 control manifest, exact directory file set,
literal hashes, pinned control-set digest, fixed encodings, effective
addresses, mappings, masks, and permissions. The execute-only code mapping is
exactly `read=false, write=false, execute=true`. The S2-42e generated manifest,
fixtures, and digest remain unchanged.

The control vectors use the fixed 128-byte data pattern and RAX value recorded
in the fixtures. They include first and last valid spans, unaligned accesses,
both displacement signs, RIP-relative length-dependent displacements, address
size 32 truncation with high RBX bits set, and REX.B through R8. Ordinary SIB
uses scale 4 with RCX=3 (`SIB=8b`); address32 uses scale 1 with RCX=3
(`SIB=0b`). Every expected snapshot independently pins all GPRs, RIP, RFLAGS
and definedness, one-instruction retirement, both complete memory regions,
and store canaries. Each interpreter and native result is checked against
that snapshot before backend comparison, so identical wrong results cannot
pass.

`--run-oracle` now selects the ordered 56+1,024 set and requires exactly two
interpreter and two Linux x86_64 native results per case. Evidence distinguishes
`generated_count=1024`, `handwritten_count=56`, and `total_count=1080`; its
selected-run ID binds both manifest digests and all ordered IDs. Saved-run
verification requires all 2,160 case/repeat records, both backend result files
for every record, successful process evidence, full expected snapshots, and
build/machine provenance bound to the run-level source fingerprint and runtime
compiler versions. All 1,080 frozen input files are checked byte-for-byte and
the current verifier rejects legacy `integer` runs. A backend timeout or missing native execution fails
the command. The failed summary is written before input preflight, so stale
success cannot survive a failed control check.

The local control run executed all 56 literal fixtures twice through the
existing interpreter and matched all 112 full snapshots. Python tests mock
remote orchestration; they do not claim native execution. Linux native
comparison and physical hardware acceptance remain `not_run`; full S2-42 and
its S2-21/S2-39/S2-40 dependencies remain open.
