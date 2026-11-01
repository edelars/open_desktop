# Проверка JIT этапа 3

Эта методика дополняет [контракт JIT](../specs/JIT_CONTRACT.md) и общие правила
из [BENCHMARKS](BENCHMARKS.md). Она описывает проверки, которые будут включаться
по мере готовности задач S3; не все пути реализованы сейчас.

## S3-24b: exact future-tail immediate rewrite in the V3 fixture

Run the exact `od-host` cases
`tests::active_code_v3_invalidates_exact_not_yet_executed_tail_immediate` and
`tests::active_code_v3_guards_faults_aliases_and_committed_stop_are_precise`
on macOS ARM64, then run `python3 tests/cpu/stage3_harness.py --suite local`.
The harness pins the new lifecycle test identity alongside the existing SMC
and cache-lifecycle cases. Mini, physical-x86, performance, owner-backed, and
Stage 2 suites remain unselected here.

The first case changes only bytes `[entry+15, entry+23)` of the exact
23-byte V3 artifact `MOV RDX,imm64; MOV [RBX],RAX; MOV RCX,imm64`. It checks
direct and guest-alias destinations against one backing, one write and one
version increment, retirement/budget/RIP at the committed store boundary,
full interpreter CPU/RAM checkpoint parity, invalidation of all warmed direct
and alias prefix/tail dependencies while an unrelated entry remains resident,
and fresh dispatch through both mappings. A captured native result from the
old tail is rejected by the comparator after the backing update. Negative
cases reject writes into the store instruction, the tail opcode, qwords shifted
by either one byte, a same-backing target split across adjacent mappings, and
a stale full-source lease. Backend and architectural write failures retain
atomicity; STOP raised during a successful write still yields the committed
invalidation boundary.

This is fixture-level identity/invalidation and redispatch evidence. It does
not claim physical Intel/AMD self-modifying-code behavior, arbitrary code
rewrites, production MMU/writer integration, or completion of S3-24.

## S3-18e: bounded checked-memory cache

Run the focused `od-jit` memory-cache unit tests and, on macOS ARM64, the two
`od-host` `memory_corpus` tests named
`capacity_one_memory_cache_matches_uncached_dispatch_for_frozen_profiles`
and `capacity_one_memory_cache_preserves_control_and_committed_prefix_behavior`.
They cover capacity-one same-key reuse, dependency/context key changes,
replacement/drop ordering, failed publication, entry guards, executor-error
cleanup, and parity with uncached execution for the two frozen profiles. The
host corpus checks full state/memory and control behavior, including committed
prefix and fault outcomes.

This opt-in fixture cache retains one executable handle across synchronous
calls, while CPU state, RAM, access context, budget, STOP and event controls
remain invocation-local. The test coverage does not establish production
owner/MMU or SMC/DMA invalidation, concurrent/asynchronous execution, other
memory profiles, Mini support, physical-x86 equivalence, or performance.
See the [implementation report](../reports/STAGE_3_S3-18e_2026-11-01.md) and
[ADR-0004](../decisions/ADR-0004-capacity-one-memory-cache.md).

## S3-38b: cached eager NativeRegisterV1 fixture

Run the three pinned `s3_38b_native_register_v1_*` gates in
`python3 tests/cpu/stage3_harness.py --suite local`. They cover separate
NativeRegisterV1 profile/key identity, dependency/source/context changes,
capacity-one replacement, STOP/event guard ordering, publish/execute failures,
and host parity/lifetime. The host scenario compares full CPU state and
retirement against both the interpreter and uncached eager NativeRegisterV1,
checks a backward branch re-entering the same block, exact NOP interpreter
handoff, RIP-relative LEA, and hit counters with no repeated compile or
publication. Counters document reuse and lifecycle only; they are not timing
evidence.

The cache is fixture-owned and bounded to one entry. These checks do not
exercise owner/MMU invalidation, SMC, a VMHost-wide cache lifetime, Mini,
physical x86, Stage 2, or performance acceptance. Those suites remain separate.

## S3-38c: cached demand-flags NativeRegisterV1 candidate

Run the three pinned `s3_38c_native_register_v1_*` gates in
`python3 tests/cpu/stage3_harness.py --suite local`. They pin strategy-key
isolation, candidate lifecycle/guards, and cold/hit full-state parity. The
candidate is opt-in and experimental; these checks establish correctness and
cache reuse only. Eager remains the default. Mini, physical x86, Stage 2,
owner/MMU, and performance suites are not covered by this slice.

## S3-29a: bounded demand-driven flags correctness candidate

Run `cargo test -p od-jit --lib demand_flags_candidate --offline` and
`cargo test -p od-host --lib demand_flags_candidate --offline`. The opt-in
candidate compile API is separate from the eager/default compile path and is
not inserted into the shared translation cache. It only covers the existing
W64 register-only ADD/SUB/CMP profile with flag-preserving MOV and JZ/JNZ; it
does not add instructions or select a production policy.

The native host tests compare candidate and eager CPU state, RIP, defined
arithmetic flags, preserved non-arithmetic RFLAGS, retirement and remaining
budget. One arithmetic chain is also stepped through `od_isa::execute::step`.
Cases include ADD→SUB, CMP→ADD, alias/equal operands, ADD→MOV that overwrites
the original destination, arithmetic→MOV→JZ/JNZ with both outcomes, register
homes 0..4 (including forced spills), zero/exhausted budgets, stop-RIP before
the next instruction, unsupported and deferred-decode exits, and a mapped
helper contract failure, plus the canonical `NeedInterpreter` boundary before
the existing bare-NOP fallback. The instrumented helper must run once at the
final observer/exit for two uninterrupted full writers (with the call operands from
the last writer), and once for an exit between writers (with the earlier
writer's saved operands); budget zero runs it zero times. PC-map tests require
pending ranges/boundaries to remain inadmissible until the PC after the final
RFLAGS store. The ADD→MOV map case checks all non-exit MOV ranges, and helper
failure epilogues must remain inadmissible.

These tests establish a bounded correctness candidate only. They do not cover
partial flag writers, ADC/SBB, memory/helper handoff, physical x86, Mini, or
performance. Eager remains the reference/default; S3-29 remains open until the
remaining correctness gates and final performance comparison determine policy.

## S3-29b: uncached multi-block NOP dispatch

Run `cargo test -p od-host --lib native_nop_fallback_demand_flags_candidate_dispatches_uncached_blocks --offline` on macOS ARM64. This executes the explicit opt-in dispatcher with `PublishedBlockExecutor`; its witness requires a candidate `InadmissiblePendingFlags` PC-map range and observes native entries across loop and post-NOP blocks. The eager `run_with_nop_fallback` regression entry remains separately exercised.

The fixture runs taken and non-taken back-edge JNZ, arithmetic → bare NOP → CMP/JZ, both JZ outcomes, and the non-taken path's following MOV. Compare full CPU state and retirement/budget counts with `od_isa::execute::step` for budgets 0..full completion. Stop at every reachable instruction boundary, compare exact prefix state and accounting, clear stop-RIP, and resume to the same final state without replay. Inject an event after blocks at loop re-entry and at NOP entry; require `EventPending` before the NOP interpreter bridge, then clear and resume. Check simultaneous STOP-RIP/STOP and budget exhaustion retain S3-28a priority over an event. Each block goes through the existing publish/execute/drop callback, so the mapping is released before the dispatcher continues.

Mini and performance checks are intentionally deferred to the end of Stage 3. The candidate remains uncached and unselected; this is local correctness evidence, not physical-x86 or performance evidence.

## S3-29c: partial-flags INC RAX handoff

Run `cargo test -p od-jit --lib inc_rax_admission_is_exact_and_separate_from_nop_profile --offline` and `cargo test -p od-host --lib native_inc_rax_fallback_carries_canonical_flags_into_native_branches --offline` on macOS ARM64. Only the separate `S0_09NopIncRaxAdmission` and eager/candidate `run_with_nop_inc_rax_fallback*` APIs admit exact `48 FF C0`; the legacy NOP-only admission still rejects it. Admission rejects truncation, extra prefixes, wrong width/register/opcode, DEC and branches into the INC encoding.

The host test runs `CMP→MOV RAX,imm64→INC RAX→JZ→MOV/JNZ→suffix` through candidate and eager dispatch and compares full CPU state/accounting with `od_isa::execute::step`. It checks both CF inputs and literal OF/SF/ZF/AF/PF results for zero, signed qword extrema, `u64::MAX`, nibble and parity edges, with both JZ/JNZ outcomes and unchanged GPR/non-arithmetic flags. Budgets 0..full and stop/resume at every reachable instruction boundary must not replay an instruction. The PC-map witness requires pending-flags state before fallback; interpreter retirement counts only INC. An event raised after the native prefix remains pending and yields before INC; after clearing, resume continues from the same RIP. Simultaneous STOP-RIP or atomic STOP and event retain STOP priority. Events are sampled before the synchronous one-step interpreter call; there is no deterministic executor hook between INC and the following dispatcher guard.

Mini and performance checks remain deferred to the end of Stage 3. This is uncached local correctness evidence; it does not select a default or extend the ISA beyond exact INC RAX fallback.

## S3-33a: exact native INC RAX

Run the compiler regression `cargo test -p od-jit native_inc_rax_is_opt_in_and_compiles_following_branch_consumers --offline` and the focused host regression above. The additional `compile_block_for_native_inc_rax*` and `dispatch::run_with_native_inc_rax*` APIs reuse the same full-code exact admission, but lower bare `48 FF C0` as W64 Add+1 with a partial `WriteMask`. The demand-flags candidate must materialize a pending full writer and preserve its live allocator homes before the INC helper call. It then merges PF/AF/ZF/SF/OF while preserving CF.

The host regression checks eager and candidate full-state parity for CF=0/1 and the arithmetic edge values, followed by native JZ/JNZ consumers. The native path must retire all selected instructions natively; candidate budget boundaries and STOP-RIP at the pre-INC boundary must match interpreter state without replay. The compiler test also proves that the existing bridge still returns `NoNativePrefix` at INC and that both native strategies include the branch in their PC map. Prefixes, DEC and other registers remain negative admission cases from S3-29c. Do not run Mini or performance checks until the final Stage 3 gate.

## S3-33b: exact native DEC RAX

Run `cargo test -p od-jit --lib dec_rax_admission_is_exact_and_separate_from_inc_profile --offline`, `cargo test -p od-jit --lib native_dec_rax_is_opt_in_and_compiles_following_branch_consumers --offline`, and `cargo test -p od-host --lib native_dec_rax_preserves_cf_and_matches_interpreter_branches --offline`. Separate `compile_block_for_native_dec_rax*` and `dispatch::run_with_native_dec_rax*` APIs use `S0_09NopDecRaxAdmission` and only exact bare `48 FF C8`. W64 Subtract+1 writes PF/AF/ZF/SF/OF while preserving CF and other bits. The demand candidate materializes pending full flags before DEC and preserves live allocator homes across the helper boundary; DEC flags are eager and introduce no pending descriptor/helper ABI.

The host differential test compares complete CPU state and retirement with `od_isa::execute::step` for both CF values, 0/1/`u64::MAX`/signed extrema/AF-PF edges, and both JZ/JNZ outcomes. It covers eager/candidate execution, candidate budgets 0..6, STOP at entry and immediately before DEC after the pending CMP, pending-flags PC-map evidence, and negative exact-form admission (truncation, prefixes, INC, other registers, immediate SUB, memory DEC, and branches into DEC). This remains partial uncached correctness evidence with no performance claim; Mini and performance checks remain deferred until Stage 3 ends.

## S3-33c: exact native NEG RAX

Run `cargo test -p od-jit --offline neg_rax_admission_is_exact_and_separate_from_inc_dec_profiles`, `cargo test -p od-jit --offline native_neg_rax_is_opt_in_and_compiles_following_branch_consumers`, `cargo test -p od-jit --offline native_neg_rax_lowers_to_zero_minus_rax_with_full_arithmetic_flags`, and `cargo test -p od-host --offline native_neg_rax_replaces_full_pending_flags_and_matches_interpreter`. Only `S0_09NopNegRaxAdmission` and the separate eager/demand-candidate APIs admit exact bare `48 F7 D8`; S0-09 NOP behavior and INC/DEC admissions remain unchanged. Reject wrong registers (including R8), memory, narrow width, extra prefixes, truncation, NOT, and branches into the encoding.

The lowering test asserts the exact Read RAX -> Const 0 -> W64 Subtract(0,RAX) with full `FlagEffect::Arithmetic` -> Write RAX -> Commit sequence. The compiler test covers eager and demand-candidate compilation with the following branch mapped in the PC map. Host parity covers `0`, `1`, `u64::MAX`, signed extrema and `0x10/0x11/0x100`, both incoming CF values, CF=0 only for zero, OF at `i64::MIN`, DF/non-arithmetic preservation, both JZ/JNZ paths, and complete state/retirement parity. Exercise pending CMP->NEG->JZ/JNZ, NEG->ADD, terminal NEG, candidate budgets 0..5, STOP before NEG after CMP, and budget/STOP immediately after NEG. The candidate PC-map witness must see pending flags before the branch. Generic helper-failure behavior remains covered by `demand_flags_candidate_materializes_on_stop_unsupported_and_helper_failure_exits`; host failures must not become guest exits. Mini and performance checks remain deferred to the end of Stage 3. See the [S3-33c report](../reports/STAGE_3_S3-33c_2026-10-30.md).

## S3-33d: exact native ADD RAX, sign-extended imm8

Run `cargo test -p od-jit --offline add_rax_imm8_admission_covers_signed_byte_range_and_rejects_neighbors`, `cargo test -p od-jit --offline native_add_rax_imm8_lowers_decoded_sign_extended_value`, `cargo test -p od-jit --offline native_add_rax_imm8_compiles_eager_and_candidate_branch_consumers`, and `cargo test -p od-host --offline native_add_rax_imm8_matches_interpreter_full_state_and_pending_branch`. Only `S0_09NopAddRaxImm8Admission` and its eager/demand-candidate APIs admit exact bare `48 83 C0 ib`; admission cross-checks all 256 source bytes against the decoder's sign-extended immediate, which lowering then uses without recalculating. Existing generic and INC/DEC/NEG-only admissions continue to reject this encoding.

The lowering test checks Read RAX → decoded W64 Const → W64 Add with full `FlagEffect::Arithmetic` → Write RAX → Commit for `00`, `01`, `7F`, `80`, and `FF`. The compiler test checks eager and candidate PC maps through a following JZ. Host differential coverage compares complete `LongModeState` and retirement against the interpreter for zero, one, `u64::MAX`, signed extrema, AF/PF edges, both incoming CF values, DF/non-arithmetic preservation, each immediate boundary, and CMP→ADD→JZ/JNZ with distinct taken/fallthrough state changes. CMP's ZF is opposite to the ADD result in both branch cases; a candidate PC-map witness checks pending flags at the JZ consumer, and a successor ADD checks the next full writer. Budgets and STOP before ADD, before JZ, and before the successor writer are compared with the interpreter. Negative admission covers other registers including R8, `/5` SUB, `/7` CMP, `81`, accumulator form, memory, byte/narrow widths, prefixes, truncation, and a branch into the immediate byte. Mini and performance checks remain deferred until the end of Stage 3. See the [S3-33d report](../reports/STAGE_3_S3-33d_2026-10-30.md).

## S3-33e: exact native CMP RAX, sign-extended imm8

Run `cargo test -p od-jit --offline cmp_rax_imm8_admission_covers_signed_byte_range_and_rejects_neighbors`, `cargo test -p od-jit --offline native_cmp_rax_imm8_lowers_to_flags_only_compare`, `cargo test -p od-jit --offline native_cmp_rax_imm8_compiles_eager_and_candidate_branch_consumers`, and `cargo test -p od-host --offline native_cmp_rax_imm8_matches_interpreter_and_preserves_gprs`. Only `S0_09NopCmpRaxImm8Admission` and its eager/demand-candidate APIs admit exact bare `48 83 F8 ib`; all 256 immediates are checked against decoder-owned sign extension. Other generic, INC, DEC, NEG, and ADD-immediate admissions reject CMP-immediate.

Lowering is Read RAX → decoded W64 Const → W64 Compare with `result=None` and full `FlagEffect::Arithmetic` → Commit; no GPR write is permitted. Host parity compares complete state and retirement for equal/unequal cases, `CMP ...,0`, `MAX` versus `FF`, and `...80` versus `80`; it checks DF/non-arithmetic preservation, both initial CF values, varied old arithmetic flags, JZ/JNZ distinct paths, candidate pending-statepoint evidence, a successor full writer, budgets 0..5, and STOP before CMP and at branch boundaries. Negative admission covers `/0` ADD, `/5` SUB, other GPR/R8, `81`, accumulator encoding, memory, narrow widths, prefixes, truncation, and a branch into the immediate. Mini and performance checks remain deferred until the end of Stage 3. See the [S3-33e report](../reports/STAGE_3_S3-33e_2026-10-30.md).

## S3-33f: exact native SUB RAX, sign-extended imm8

Run the exact S3-33f admission, lowering, compiler, and host tests, plus the
three local harness gates `s3_33f_native_sub_rax_imm8_admission`,
`s3_33f_native_sub_rax_imm8_compiler`, and
`s3_33f_native_sub_rax_imm8_host`. Only its dedicated admission and eager/
demand-candidate APIs accept bare `48 83 E8 ib` (`83 /5`, W64 RAX); all 256
immediate bytes are checked against decoder-owned sign extension. The IR is
Read RAX → decoded W64 Const → W64 Subtract with a result and full arithmetic
flags → Write RAX → Commit.

Coverage includes SUB zero's flag update, `0-1`, `0-(-1)`, MIN−1, MAX−(-1),
both old CF values, AF/PF boundaries, opposing incoming flags, DF and
non-arithmetic preservation, full state/RIP/retirement, and both eager and
candidate execution. Branch tests cover pending CMP→SUB and ADD→SUB with
distinguishable JZ/JNZ paths, candidate PC-map evidence, terminal SUB, a
successor full writer, budgets and STOP before SUB, after SUB, and before the
successor. Negative cases include other `/digit` values, GPRs including R8,
memory/width variants, prefixes, `81`, accumulator encoding, truncation, and a
branch into the immediate; prior arithmetic admissions reject this profile.
Mini and performance checks remain deferred until the end of Stage 3. See the
[S3-33f report](../reports/STAGE_3_S3-33f_2026-10-30.md).

## S3-33g: exact native NOT RAX

Run the exact NOT RAX admission, emitter-shape, compiler, and host gates from
`python3 tests/cpu/stage3_harness.py --suite local`. The admission gate must
accept only bare `48 F7 D0`, reject nearby NOT/NEG/register/memory/width/prefix
forms, truncation, and a branch into the encoding, and remain separate from
the NEG profile. Eager and candidate compilation must use the exact profile.
The emitter gate limits `FlagEffect::Preserve` support to W64 Subtract with a
result; the NOT compiler gate confirms that NOT itself has no helper call and
is not mapped as `AfterHelper`.

Host comparisons cover complete CPU state and retirement, full RFLAGS and
definedness, input values `0`, `u64::MAX`, signed extremes and alternating
bits, NOT twice, NOT without a pending writer, and a new writer after NOT.
Pending ADD/CMP → NOT → JZ/JNZ cases use branch outcomes that distinguish the
old writer's flags from flags incorrectly produced by NOT. Also check an old
pending ADD/CMP writers through NOT, a pending writer through two NOTs,
candidate statepoints, all non-RAX GPRs, STOP/budget boundaries, end-of-code,
branch exits, and actual RIP. The compiler gate counts emitted `BLR` helper
calls and requires none for NOT.
Mini and performance checks remain deferred until the end of Stage 3. See the
[S3-33g report](../reports/STAGE_3_S3-33g_2026-10-30.md).

## S3-33h: exact native LEA RAX,[RBX]

Run the exact admission, compiler, and host gates from
`python3 tests/cpu/stage3_harness.py --suite local`. Admission accepts only
bare `48 8D 03` with decoded W64 RAX destination and 64-bit `[RBX]` address;
it rejects other registers, SIB/disp alternatives, RIP-relative, address32,
prefix/truncated forms, branch into the instruction, and remains separate
from existing profiles. The IR test requires a single GPR read, GPR write,
and commit without memory, flags, or arithmetic operations. The compiler gate
checks eager/candidate entry points, PC map, and absence of data helper calls.

Host differential coverage compares complete state and retirement for RBX
zero, small values, signed extrema, unmapped and noncanonical values, with
all other GPRs as canaries and full flags/definedness preserved. A rejecting
interpreter memory probe proves no data callback occurs. Pending ADD/CMP → LEA
→ JZ/JNZ cases have distinguishable branch outcomes and candidate statepoint
evidence. LEA → NOP interpreter fallback → JZ/JNZ checks pending flags across
the fallback, native/interpreter retirement accounting, and STOP/budget
boundaries around it. Also cover consecutive LEAs, a successor flags writer,
and terminal execution. Mini and performance checks remain
deferred until the end of Stage 3. See the
[S3-33h report](../reports/STAGE_3_S3-33h_2026-10-30.md).

## S3-33i: exact native XCHG RAX,RBX

Run the admission, emitter, compiler, and host parity gates from
`python3 tests/cpu/stage3_harness.py --suite local`. Admit only bare `48 87 D8`
with decoded W64 RAX/RBX operands. Reject `48 87 C3`, `48 93`, self-exchange,
other registers/widths, memory, LOCK and other prefixes, truncation, and branch
targets inside the instruction. The lowering and emitter checks require both
old register reads before either write, exact swapped destinations, one commit,
two distinct live temporaries, and rejection of wrong mapping, read-after-write,
duplicate/third destinations, and mixed operations. PC-map ranges during
staging writes must be replay-only or inadmissible, never canonical; the guest
retires the exchange once.

Host differential cases compare full state and retirement for equal/unequal
register pairs, zero, `u64::MAX`, signed extrema and alternating patterns,
including all other GPRs, full flags/definedness, two exchanges restoring the
initial state, and eager/candidate execution. Pending CMP → XCHG → JZ followed
by a SUB checks that prior flags still select the branch and later arithmetic
uses swapped registers. Include terminal execution, STOP/budget boundaries and
admitted interpreter fallback. The native/interpreter evidence is local and
specific to these exact bytes; the S2-10 reviewed physical fixture confirms
XCHG semantics but does not establish exact physical bytes `48 87 D8`. This
register operation makes no memory-atomicity claim. Mini/performance checks
remain deferred until the end of Stage 3. See the
[S3-33i report](../reports/STAGE_3_S3-33i_2026-10-30.md).

## S3-33j: exact native LEA RAX,[RBX+disp8]

Run the admission, preserving-emitter, eager/candidate compiler, and host
parity gates from `python3 tests/cpu/stage3_harness.py --suite local`. The
profile admits only bare `48 8D 43 db` and all 256 displacement bytes, with
decoder-owned W64 RAX, address64, RBX base, no index, scale 1, and exact
sign-extended i8 displacement. It remains separate from the existing
`48 8D 03` profile. Lowering uses `ReadGpr(RBX)`, a signed displacement
constant, W64 `Add`/`Preserve` with a result, `WriteGpr(RAX)`, and one commit.
No memory access, canonicality check, fault, helper, or flags update is part of
LEA; full flags/definedness and pending-writer state stay intact.

Admission rejects other registers, SIB, disp32, RIP-relative, width/address
changes, prefixes, truncation, the old no-displacement encoding, and branches
into the instruction. Host parity covers signed byte edges, wrapping, unmapped
and noncanonical values, complete state, and retirement. Pending ADD/CMP → LEA
→ JZ/JNZ, multiple LEAs, a following writer, terminal/fallback, STOP/budget,
and PC-map cases are part of the bounded acceptance target. Exact local
differential evidence does not confirm physical source bytes. Mini and
performance checks remain deferred until Stage 3 ends; see the
[S3-33j report](../reports/STAGE_3_S3-33j_2026-10-30.md).

## S3-33k: exact native MOV RAX,sign-extended imm32

Run the three exact admission, compiler and host parity gates from
`python3 tests/cpu/stage3_harness.py --suite local`. The uncached opt-in profile
admits only bare `48 C7 C0 id` (length 7), and verifies decoder-owned W64 RAX,
`/0`, and agreement between the decoded signed-extended constant and source
bytes. The existing `48 B8 imm64` baseline profile remains unchanged.

The local cases sample immediate values and byte/bit patterns, verify
little-endian decoding and sign extension, compare full state against the
interpreter, and preserve architectural flags and pending ADD/CMP writer state
across MOV. Distinguishable JZ/JNZ outcomes verify the previous writer's flags
survive the MOV. Compiler
checks cover eager and demand-flags candidate lowering, following branch
consumers, PC-map entries, and `MOV;NOP` lowering to a `NeedInterpreter` exit.
Host tests compare native `MOV;NOP` fallback retirement with the interpreter
and cover budget/STOP before entry and immediately after the seven-byte MOV.
Negative admission cases cover alternate
register/group/width/prefix/C6/truncation and branch-interior forms. These are
representative local differential checks, not an exhaustive enumeration of
2^32 immediates or an exact physical-x86 oracle. Mini and performance checks
remain deferred until Stage 3 ends; Stage 2 and owner/MMU are outside this
slice. See the [S3-33k report](../reports/STAGE_3_S3-33k_2026-10-30.md).

## S3-33l: exact native LEA RAX,[RBX+RCX]

Run the three exact admission, compiler, and host parity gates from
`python3 tests/cpu/stage3_harness.py --suite local`. The opt-in uncached
profile admits only `48 8D 04 0B`: W64/address64, RAX destination, RBX base,
RCX index, scale 1, zero displacement, and length 4. It remains separate from
the earlier no-displacement and disp8 LEA profiles. Lowering reads both sources
before writing RAX, adds as W64 with `FlagEffect::Preserve`, and commits once;
compiled code must not contain a guest data-memory operation or helper call.

Admission negatives cover adjacent ModR/M/SIB encodings, swapped registers,
other scales, no index, displacement-bearing and RIP-relative forms, prefixes,
truncation, and branch targets inside the instruction. Host parity checks full
state across representative values and flags, zero data callbacks, and
CMP→LEA→JNZ where predecessor ZF opposes the sum's ZF. Distinguishable branch
paths plus budget=1 and STOP at entry/after LEA check retirement and RIP
boundaries. These are local differential checks; there is no dedicated
physical fixture evidence for the exact bytes. Mini/performance remain deferred
until Stage 3 ends. See the
[S3-33l report](../reports/STAGE_3_S3-33l_2026-10-30.md).

## S3-33m: exact native LEA RAX,[RIP+disp32]

Run the three exact admission, compiler, and host parity gates through
`python3 tests/cpu/stage3_harness.py --suite local`. The uncached profile
admits only `48 8D 05 dd dd dd dd`: W64/address64, RAX destination, RIP-relative
base, no index, scale 1, signed decoder-owned disp32, and length 7. The result
is this instruction's guest `next_rip` plus signed displacement with wrapping
W64 arithmetic; it is not used as a memory address. Flags, other GPRs, and
pending flags remain unchanged, with no data callbacks or helpers.

Cases cover 0, ±1, ±7, `i32::MIN/MAX`, distinct guest RIPs, a variable-length
preceding instruction, valid code near the guest address limit, CMP→LEA→JNZ,
and exact-form/branch-boundary negatives. The current evidence is local
differential only and has no physical fixture claim. Mini/performance remain
deferred until Stage 3 ends. See
[S3-33m report](../reports/STAGE_3_S3-33m_2026-10-30.md).

## S3-33n: composed exact native register profile

Run the three S3-33n exact gates through `python3
tests/cpu/stage3_harness.py --suite local`: union admission,
mixed compiler/statepoints, and host full-state parity/corpus. The admission
gate checks the exact union of restricted S0-09, bare NOP fallback, and the
already committed S3-33a–m forms; it also reruns the old isolated profile
negative witnesses to ensure foreign extras remain rejected there. Check all
variable-length boundary starts, branch targets at starts and instruction
interiors, exact-code binding, truncation, and disallowed register/width/prefix
and memory-data variants.

The compiler gate mixes many or all admitted forms, branches across their exact
boundaries, and places RIP-relative LEA after a differently sized predecessor.
Compare eager and demand-flags-candidate snapshots and PC maps, including a
pending full writer → partial writer → flags-preserving operation → XCHG →
branch chain. Exercise STOP/budget immediately before and after every opcode
boundary, plus bare-NOP fallback and control behavior.

The host gate compares complete interpreter/native state, flags and definedness,
RIP, and total/native/interpreter retirement. It includes decoder-owned
immediates/displacements, pending/saved operands across the XCHG/branch path,
and a bounded deterministic ordered-pair corpus over the 13 exact extra forms.
The targeted host test passed exactly 169 ordered pairs with seed
`0x5333_336e_7265_6731`; failures print seed, pair, bytes, and initial state.
The mixed compiler case passed with five instructions under the existing
4096-byte cap. Eager and candidate paths cover pending-ZF taken and fallthrough
through distinct MOV paths, STOP/budget boundaries, and bare-NOP fallback /
control. The three targeted Rust identities passed, as did 28/28 harness
self-checks. Independent review and the full local suite remain pending. Mini,
performance, Stage 2, and owner/MMU checks remain `not_run` or outside this
slice. See
[S3-33n report](../reports/STAGE_3_S3-33n_2026-10-30.md).

## Срез A: один register-only блок

1. Использовать только точные S0-09 формы из JIT contract и fixtures с
   зафиксированными bytes, начальным состоянием и ожидаемым результатом.
2. Сравнивать одинаковый fixture на interpreter и новом block JIT, затем на
   физическом x86 oracle. Сверять RAX/RCX/RDX/RBX, RIP, определённые flags и
   exit reason. Не трактовать undefined flags или недоступное наблюдение как
   совпадение.
3. До появления dispatcher (S3-15) ограничивать corpus целыми программами,
   исполняемыми одним блоком, включая допустимую terminal branch. Multi-block
   corpus и полный Stage 1 harness остаются отложенными до S3-15.
4. Проверять отрицательные входы: неразрешённый opcode/prefix/register,
   некорректная branch target, усечённая инструкция, branch в середину
   инструкции и предел блока. Не публиковать и не исполнять неполный или
   невалидный host output. Если следующая guest instruction за валидным
   префиксом блока даёт fault/Unsupported, сохранить этот корректный префикс и
   наблюдать исход только при достижении соответствующего RIP; правила этого
   случая фиксируются в S3-04.
5. Явно подтверждать, что arithmetic flags вычисляет собственный x86 helper,
   если lowering ещё не materializes их напрямую; совпадение host NZCV с x86
   flags не предполагается.

## Срез B и последующие зависимости

- Проверять budget, STOP, dispatcher, точный progress и interpreter fallback
  только после S3-15/S3-16 и с точной семантикой нужной формы этапа 2.
- Memory forms не принимать до checked bridge S3-19, соответствующего
  interpreter slice, fault cases и условий ISSUE-012. Сверять адресное
  пространство, access origin, fault result и порядок side effects.
- Проверять инвалидирование code dependencies и освобождение compiled blocks
  после реализации соответствующих S3 tasks. Не считать изменение fixture
  buffer заменой проверки guest code writes/mapping changes.
- Host execution, executable-memory publication и I-cache synchronization
  проверять на поддержанном Apple Silicon/macOS configuration. Отдельно
  фиксировать signing/Hardened Runtime entitlements и SDK; не переносить
  результаты одной машины на другую.

## Протокол и доказательства

### Bounded S3-26e repeated load pair

Run `cargo test -p od-jit --lib ir_load_then_load_is_one_typed_two_site_entry`
and `cargo test -p od-host --lib
ir_two_load_pair_preserves_callbacks_faults_and_continuation`. The only added
pair is `48 8B 03 48 8B 0B` (`MOV RAX,[RBX]; MOV RCX,[RBX]`); its native callback
trace must contain exactly two qword reads at the same address, and full CPU
state/RAM must match two independent interpreter steps. Check first-site and
second-site architectural faults, a second-site backend error, partial RAX
commit, singleton load retry at the second RIP without repeating site 0,
budgets 0/1/2, pre-entry STOP, stop-RIP at site 1, callback STOP, and an event
raised by the first read. The event case uses remaining budget and a following
instruction; both reads commit before `EventPending` is returned, without
clearing the event. The exact host test is also required by the S3-34e local
harness. This is fixture-only private ordinary RAM: the trace proves two helper
calls, not device/MMIO behavior, full S3-26/S3-27, or physical x86 semantics.

### Bounded S3-27b next fetch after memory pair

The native fixture runs either exact two-site pair through the generated entry,
then applies a controlled `GuestMemory::fetch` architectural fault at valid
guest RIP `A+6` by calling the interpreter on the returned state and RAM. A
separate `od_isa::execute::run` runs the same pair and faulting third fetch,
reporting the fault with two `instructions_executed`. Run the interpreter
again from the native-returned state/RAM; it reports the same fault with zero
additional instructions. Compare the complete CPU state including flags/RIP
and all RAM; check the full fault payload and no repeated pair data callbacks
on fault or retry. Inject unmapped/backend fetch errors for both pair forms and
require host-error classification with zero continuation instructions. Real
`RunConfig` zero-budget and STOP closure exits suppress the next fetch; after
replenishing budget or clearing STOP, continuation reports the architectural
fault with zero instructions. The six-byte
compiler `CodeView` ending at `A+6` is independent of the guest mapping/fetch
result. This test proves a fixture boundary only; production dispatcher and
owner-backed MMU behavior remain untested.

### Bounded S3-27c V3 host-PC attribution

The exact `ActiveCodeBlockV3` compiler test requires a validated `PcMap` over
all emitted ARM64 bytes, aligned contiguous ranges, exact commit boundaries at
`A+10`, `A+13`, and `A+23`, and the expected commit/helper/exit admissibility
classes. The native lifecycle test drops the compiler artifact, invalidates the
cache, and then reuses the slot; the live handle retains its map and the stale
generation is rejected. Helper failure attribution remains at store RIP `A+10`
for both typed architectural-fault and backend-error outcomes. This is
diagnostic attribution and does not provide host-fault recovery or replay
authorization. Direct V2 maps and full S3-27 remain open.

Production acceptance additionally needs the `od-machine` owner contract:
private synchronous RAM capability, code lease across compile/publication/
execution, MMU-aware fetch with deferred next-RIP fault delivery, and
committed-write batches from every writer with alias-aware code invalidation.
The dispatcher must check stop-RIP → STOP → budget before fetch, treat fixture
window end as distinct from guest termination, and release both owner lease
and published mapping at their documented safe points. See the [S3-15n
contract](../specs/JIT_CONTRACT.md#production-memory-owner-integration-prerequisites-s3-15n).
These are prerequisites to owner-backed end-to-end testing, not claims made by
the fixture cases above; physical Mac mini comparison and performance
measurements remain separate final gates.

### S3-16m bounded-memory bare-NOP fallback

Run `cargo test -p od-host --offline bounded_memory_dispatch`. The native
fixture uses pair → exact bare `90` → pair and compares complete CPU state,
data RAM and callback counts with independent full-interpreter execution at
budgets 0 through 5. It checks native/interpreter retired counts, strict
`run` rejection at `90`, STOP and stop-RIP on both sides of the NOP, and
continuation from the post-NOP RIP without repeating the first pair's memory
accesses. An unsupported instruction after NOP preserves prior commits.
Prefixed NOP, `0F 1F` and PAUSE must not increment interpreter retirement.
Successful publication of the trailing pair exercises mapping release before
the next native publication; the executor contract and explicit host drop
place that release before the interpreter step. This is fixture-only evidence;
it adds neither general interpreter fallback nor production owner/MMU tests.

### S3-28b cooperative events in bounded checked-memory dispatch

Run focused dispatcher tests with
`cargo test -p od-jit --offline --lib event` and
`cargo test -p od-host --offline bounded_memory_dispatch_opt_in_bare_nop_bridge_preserves_budget_and_effects --lib`.
The `od-jit` cases cover a request before work, event after artifact completion,
post-publication pre-entry yield, STOP priority, stop-RIP/fixture-boundary/
budget priority, malformed `EventPending` accounting, and pair→NOP→pair
continuation without replay. The host fixture covers published-artifact drop
before entry, singleton load/store callback events, complete pair callback
ordering before sampling, first-read STOP+event with a committed one-instruction
prefix, second-site architectural/backend failures, and interpreter parity
after clearing the request. Event remains set on yield and the caller resumes
at the returned guest RIP. These tests exercise only the
bounded fixture dispatcher; they make no Stage 2/owner/MMU, Mini, or performance
claim.

### Bounded S3-34a two-site memory corpus

For the two exact V2 memory sequences in the JIT contract, run
`cargo test -p od-host --test memory_corpus`. On local macOS ARM64 this runs
2,400 unique successful inputs (three fixed seeds × 400 per sequence), with
two native-entry and interpreter executions per input on independent full
CPU/RAM snapshots. Inputs vary qword state, destination registers, address
alignment, same/different regions, and safe partial overlap in load→store.
Checks include full state and RAM, exit/budget/retirement, and memory callback
order/count. Four separately named tests cover comparator corruption, mismatch
diagnostic serialization, guest fault, STOP, budget exhaustion, and second-site
continuation from the actual committed CPU/RAM result of the first
budget-limited entry. The test is
platform-gated and is not a physical x86 oracle or Mac mini check; those remain
separate suites and must be reported as `not_run` until executed.

On mismatch the test emits seed and case ID and writes JSON containing the
original sequence/register/flags/address/RAM input and both serialized
observations under the system temporary directory. A failed file write is
included in the panic output.

To replay a saved report explicitly, run
`OD_JIT_REPLAY_CASE=/absolute/path/case.json cargo test -p od-host --test memory_corpus replay_saved_case_from_environment -- --ignored --exact`.
The ignored test requires this variable; if it is missing, an explicit replay
fails with a diagnostic. It validates the version/profile, exact source bytes,
supported CPU/RAM snapshot, and execution protocol before native publication.
It replays each backend independently and compares each phase, including
continuation from the prior phase's returned state and memory. A fresh
`.replay-<pid>.json` report is created beside the original without overwriting
it; that report includes the current environment, build revision, backend
availability, result snapshots, and whether the saved mismatch classes were
reproduced. Unsupported native hosts and malformed or unsupported reports
cannot produce an equivalence-success report.
Continuation mismatch reports include the first budget-limited result and the
resumed result as separate paired snapshots, even when only the latter differs;
the decoder requires the snapshot count to match the saved protocol.

To run the bounded reducer on a saved mismatch report, run
`OD_JIT_REDUCE_CASE=/absolute/path/case.json cargo test -p od-host --test memory_corpus reduce_saved_case_from_environment -- --ignored --exact`.
The reducer repeats the original mismatch twice before attempting candidates,
then accepts only strictly simpler inputs with exactly matching recorded
mismatch classes at every execution phase. It modifies non-address initial GPR values and RAM contents
only, and preserves the sequence, source bytes, addresses and protocol. A fresh
`.reduced-<pid>.json` report records both envelopes, the fresh observations,
attempt count and stop reason; it is created without replacing the original.
The reducer has a 512-attempt maximum and runs only on macOS ARM64. Its positive
self-test injects a clearly labeled GPR[10] corruption when the input GPR[11]
marker is nonzero; clearing that marker removes the synthetic mismatch and is
rejected by the normal comparator. The generic reducer does not reserve GPR[11]
and can reduce it when the mismatch predicate permits. This synthetic case
validates reducer behavior and is not a JIT defect report.

### S3-34d SMC lifecycle comparator self-check

Run `cargo test -p od-host --offline code_region_store_invalidates_cached_prefix_and_tail_before_redispatch`.
The existing tracked-code fixture warms a `MOV RDX,imm64` prefix and its guest
alias, commits a store that changes the immediate, and applies the backing
notification. It checks both alias prefix/tail cache entries are removed while
an unrelated entry remains reusable, then compares fresh native re-entry with
the interpreter over full CPU state and all backing bytes/generations. The
test-only lifecycle comparator must classify a controlled old-immediate
observation as `StaleCodeResult` and a corrupted backing generation as
`BackingGenerationMismatch`. This does not disable production validation or
claim physical x86 equivalence.

### S3-34e unified local Stage 3 correctness harness

Run `python3 tests/cpu/stage3_harness.py --suite local`. It first runs its own
validation self-tests, then the existing Stage 1 local dispatcher harness with
the manual cases plus 1000 seeded generated cases twice, the complete
`od-host/tests/memory_corpus.rs` integration suite, and nine exact `od-host`
lifecycle test identities. S3-34f also adds twelve exact acceptance gates for
the bounded coordinator/demand-flags, event priority, and V0 ABI lifecycle/status
contracts. The memory corpus includes its 2400-unique-input
two-repeat run, named fault/STOP/budget/continuation cases, comparator self-check,
versioned replay and reducer checks. Two environment-only saved-case replay and
reducer tests remain ignored by design; all mandatory named tests must pass.

The parent input manifest pins the expected Stage 1 source fingerprint and each
manual fixture's case ID and SHA-256. The validator requires the child manifest
and both interpreter/JIT build revisions to use that fingerprint, then binds
each frozen manual input and result path/digest to the parent manifest. Harness
self-tests require an exact list and count of passing test identities; zero
matches, skipped tests and failures are rejected.

The harness requires macOS ARM64, rejects zero-match/ignored mandatory tests,
malformed child evidence, timeouts, signals, nonzero exits, partial suites and
source-tree fingerprint changes. Every run creates a unique
`work/stage3-harness/<run-id>/` with summary, input-manifest digest and logs;
Stage 1 summary/manifest paths and hashes are linked from it. The result is
`local_only`; physical x86/Mini, performance, Stage 2 and owner-backed suites
are explicitly `not_run`. It does not claim complete Stage 3 acceptance. See
[S3-34e report](../reports/STAGE_3_S3-34e_2026-10-29.md).

### S3-34f local acceptance audit

The local harness adds these exact tests to its nine lifecycle identities:

| Contract | Bounded acceptance form | Exact Rust test identity | Proof and residual boundary |
|---|---|---|---|
| S3-29e coordinator | Existing arithmetic → checked load → branch/suffix API | `od-host::tests::s3_29e_bounded_arithmetic_load_coordinator` | Full state/RAM parity, faults and retries, phase accounting/priority; no general memory, cache or owner/MMU |
| S3-29 pending flags | Exact arithmetic writers, preserving MOV, JZ/JNZ and defined exits | `od-host::tests::demand_flags_candidate_defers_replaced_writers_and_survives_clobbers_and_spills`; `od-host::tests::demand_flags_candidate_materializes_before_jz_and_jnz_after_preserving_mov`; `od-host::tests::demand_flags_candidate_materializes_on_stop_unsupported_and_helper_failure_exits` | Replacement, spills/clobbers, branch consumers, STOP/unsupported/helper failures; opt-in candidate only, eager remains default, no performance policy |
| S3-28 control | Event before register-dispatch work; STOP priority after publication | `od-jit::dispatch::tests::event_pending_before_entry_preserves_state_budget_cache_and_nop_fallback`; `od-jit::dispatch::tests::cached_publication_boundary_rechecks_stop_priority_before_event` | No work before pre-entry event; STOP beats event after publish; bounded local boundary, no latency claim |
| S3-38a V0 ABI | Existing register-only V0 fragment and handle lifecycle | `od-host::tests::c_abi_create_execute_stop_destroy_lifecycle`; `od-host::tests::c_abi_stop_requested_before_execute_is_honored_once`; `od-host::tests::c_abi_register_dispatch_matches_interpreter_and_reuses_loop_block`; `od-host::tests::c_abi_pre_stop_at_code_end_honors_v0_stop_contract_once`; `od-host::tests::c_abi_repeated_and_interleaved_handles_keep_state_independent`; `od-host::tests::c_abi_reports_live_process_mapping_as_busy` | STOP/destroy, loop reuse, code-end contract, independent handles, BUSY/invalid-state and output preservation. VMHost process regressions remain a separate gate; persistent cache/ownership and full S3-38 remain open. |

The input manifest pins these gate records (step, package and full test
identity), plus hashes of the harness and test source files. Each test runs in
its own Cargo `--exact` lane. The run summary records exact argv, status,
identity, output tail and a digest bound to that step's log in the same run
directory. Final validation requires a generated-format run ID, rejects
sibling-run aliases and run/log/step-log symlinks, and requires each log's
canonical lexical run-local path. It checks each log digest/tail and requires
exactly one matching non-ignored `... ok` line. Missing steps, wrong
crate/identity or argv, zero/duplicate matches, ignored results, nonzero/
timeout runs and forged `passed` summaries fail validation. Self-tests cover
omitted manifest/summary gates, invalid paths and forged evidence.

This audit proves only the bounded test lanes listed above. Full S3-28, S3-29,
S3-34 and S3-38 remain open. ISSUE-027 remains open pending process-level
reproduction; S3-23/24 general writer/MMU and physical SMC, unknown memory
classes, performance policy, physical x86/Mini and Stage 2 remain outside this
local profile. See the
[S3-34f report](../reports/STAGE_3_S3-34f_2026-10-30.md).

### S3-34j exact gates for S3-33e CMP imm8

The local harness adds three exact acceptance gates for S3-33e: host
full-state parity, eager/candidate compiler plus branch-consumer coverage, and
exhaustive admission/rejection coverage. The input manifest, planned steps,
and final summary each pin the three gates exactly once. Self-checks reject
omission of every CMP gate from both manifest and final evidence; the existing
validator continues to reject incorrect crate/identity/argv, zero or duplicate
matches, ignored/failed/timed-out results, and forged `passed` evidence. These
gates extend the S3-34i set to 22 exact gate identities across S3-33a–h; the
local run and any unselected suites are documented in the
[S3-34j report](../reports/STAGE_3_S3-34j_2026-10-30.md). This does not close
S3-34 or Stage 3.

### S3-34k exact gates for S3-33i XCHG RAX,RBX

Four mandatory exact acceptance gates pin XCHG admission, dual-write emitter
validation, compiler/statepoint behavior, and host parity. The manifest,
planned steps, and final local summary contain each identity exactly once;
self-checks pin the crate and test identity, reject a missing gate and forged
or incomplete passing evidence, and retain the prior 22 exact gates for S3-33a–h.
The resulting set has 26 gates across S3-33a–i. See the
[S3-33i report](../reports/STAGE_3_S3-33i_2026-10-30.md).

### S3-34l exact gates for S3-33j disp8 LEA

Four exact Cargo identities pin exhaustive admission, the W64 preserving
integer emitter window, eager/candidate compilation, and host/interpreter
parity. The manifest, planned steps, final summary, and self-checks require all
four once while retaining every earlier gate identity; the NOT emitter
identity is renamed to reflect that it remains the NOT regression. Exact
package, test identity, argv, and passing result are validated. The resulting
set has 30 gates across S3-33a–j. See the
[S3-33j report](../reports/STAGE_3_S3-33j_2026-10-30.md).

### S3-34n reproducible NativeRegisterV1 generated corpus

The `NativeRegisterV1` local corpus uses three fixed seeds and 400 unique
complete inputs per seed (1,200 total). Each input is reset and run twice
through interpreter, eager NativeRegisterV1 and demand-candidate backends.
Coverage includes the 13 committed extra forms, NOP fallback, baseline S0-09
instructions, variable-length predecessors to RIP-relative LEA, branch
boundaries/outcomes, STOP and budget boundaries. Full state, exit/RIP,
native-versus-interpreter retirement and remaining budget are compared, and
data access callbacks are rejected. Canonical full-input bytes feed the
uniqueness check and the reported corpus digest.

The versioned saved-case envelope includes exact bytes, base RIP, full
initial-state fields, controls, profile/version, seed/index/digest, observations
from both repeats for all backends, and a reproduction command. The exact
round-trip gate reads this saved envelope and replays both native paths. Three
mandatory exact gates cover generated repeated execution, saved replay, and
fail-closed comparator/envelope checks. All 69 existing local exact gates are
retained, including the 42 S3-33 identities; the complete harness now requires
72. A mismatch is an error and
preserves the original input and observations in the run artifact directory.
This is bounded local ARM64 correctness evidence only; Mini, physical x86,
performance, Stage 2, and owner/MMU remain unrun/out of scope. See the
[S3-34n report](../reports/STAGE_3_S3-34n_2026-10-30.md).

### S3-34o bounded saved-case value reduction

The exact reducer gate takes `OD_JIT_NATIVE_REGISTER_V1_REDUCE_CASE`, a path to
an existing v1 `SavedCase`. It replays the original first. Reduction starts
only when the exact nonempty saved signature reproduces as a stable semantic
mismatch across both repeats of interpreter, eager, and demand-candidate
backends. Every candidate is admitted and digest-checked again, then run twice
through all three backends. Accept only the exact whole-signature match and a
strictly lower popcount metric over initial GPR, arithmetic flags/DF, and
existing immediate/displacement values.

The reducer cannot remove or reorder instructions, alter opcodes or lengths,
change branch bytes/targets, guest base/entry RIP, auxiliary state, STOP, or
budget. Branch displacement is immutable code structure; no relocation occurs.
This is value reduction, not instruction minimization. Candidate order is
deterministic and exact-input deduplicated, with a 256 evaluation cap. A cap
stop is recorded as `evaluation_limit` and is not a minimality claim. The
original envelope is create-only/preserved; reduced envelope and digest are
written only when the source reproduces and a strict reduction preserves the
signature. Run artifacts include all repeated backend observations and replay
commands for both source and reduced cases. The positive mechanical self-check
uses explicitly labeled local fault injection in observations; it is not
ordinary replay data or evidence of a JIT bug. Two exact gates add to the
existing 72 Rust gates (74 total), preserving the 42 S3-33 identity set and all
three S3-34n gates. Mini remains deferred until Stage 3 ends. This slice makes
no physical x86, Stage 2, owner/MMU, performance, global-minimality, or full
S3-34/Stage 3 completion claim. See the
[S3-34o report](../reports/STAGE_3_S3-34o_2026-10-30.md).

### S3-34p reproducible cached NativeRegisterV1 corpus and replay

`native-register-v1-observations.json` is a separate complete source artifact
containing all 1,200 S3-34n inputs and their repeated interpreter, eager, and
demand-candidate observations. It does not change the S3-34n `SavedCase` v1
envelope consumed by the reducer. The cached-corpus gate runs each source input
through a retained single-entry eager cache sequence and a separate
demand-flags-candidate sequence, comparing every full state, exit, native and
interpreter retirement, and remaining budget with both the saved interpreter
and matching uncached observations. Per-step cache counters and execution
context/dependency bindings are recorded; no hit is assumed for multi-block
inputs.

`native-register-v1-cache-trace.json` is a version-1 ordered trace with a full
digest. It includes each input, baseline/cached observations, strategy,
bindings and cumulative counters for the generated sequences, plus one
two-call single-block ADD sequence for each strategy. The latter changes RAX
and RFLAGS while reusing identical bytes/context/dependencies, and requires
one miss/compile/publication followed by one hit without another compile or
publication. The replay gate verifies the input order against the generated
corpus, then executes every sequence from a fresh empty cache and compares the
full observations and exact counters. It rejects tampered state/digests,
unknown fields, counter changes and missing sequence steps. The harness binds
the trace and source-observation locations, SHA-256 values and envelope digests
to the producer/replay gates; it recomputes the source envelope digest and
checks that the producer's binding remains unchanged through final validation.
If a generated-corpus or retained-hit step mismatches, the producer saves a
separate versioned failure envelope with its successful prefix, failing input,
full observations, and cache counters before/after the failure. Its bounded
replay reader checks the source corpus identity and digest, replays the saved
prefix on a fresh cache, then verifies the failing input, observation, counters
and failure signature. A third exact gate runs explicitly labeled local
observation injections for both segment kinds to check save/read/replay, tamper
rejection, and rejection when the failure does not recur; this is a consumer
self-check, not evidence of a JIT defect. The local harness pins all 83 Rust identities, retaining all 42 S3-33
identities.
Mini, physical x86, performance, Stage 2 and owner/MMU remain deferred or
outside this local slice. See the
[S3-34p report](../reports/STAGE_3_S3-34p_2026-10-31.md).

Каждая новая строка support matrix получает literal positive/negative cases,
точный сравниваемый state и внешний prerequisite. В отчёте сохранять git
revision, build fingerprint, host/OS/toolchain, fixtures/corpus fingerprint,
seed и повторы. Физический x86 oracle обязателен там, где его требует план;
его ошибка или отсутствие соединения не могут превращаться в пропуск с green
status.

Performance checks описаны отдельно в S3-35/S3-36: cold path включает
перевод и первое исполнение, warm path использует уже опубликованный cache.
Сравнивать одинаковые входы и требования к корректности, делать повторные
прогоны, показывать распределение и ресурсы. Пилот Stage 0 не является
baseline block JIT; не утверждать ускорение без новых измерений.

S3-35b adds opt-in phase observation on the real FIFO16 dispatch/compiler
path. The ordinary od-jit path uses a statically selected no-op observer; the
bench-only feature exposes clock-based phase timings. Decode/admission, key/
snapshot/lookup, lowering, allocation, emission/finalization, publication and
execution are recorded at their actual boundaries. Hits still pay the first
two phases; guards and accounting are residual dispatch work. Three reviewed
release process runs measured the same frozen S3-17 corpus: the paired
observed/ordinary dispatch ratio median was 1.0301 across 60 pairs (IQR
1.0261–1.0342). The component intervals include the timestamp boundary transit
documented in [BENCHMARKS](BENCHMARKS.md#s3-35b-cold-dispatcher-pipeline-attribution);
they do not select a default or optimization. See the
[S3-35b report](../reports/STAGE_3_S3-35b_2026-10-31.md). Mini remains deferred
until the end of Stage 3.

### S3-20l RIP-relative shared-IR and dispatcher extension

Run `cargo test -p od-jit --lib ir_checked_rip_relative_load_store_keep_decoded_next_rip_and_displacement`, `cargo test -p od-jit --lib ir_rip_relative_singletons_reject_wrong_register_prefix_and_trailing_bytes`, and `cargo test -p od-host bounded_dispatch_composes_rip_relative_nop_and_pair_with_actual_rip_continuation`. The bounded dispatcher test exercises both RIP-relative singleton → NOP → pair and NOP → RIP-relative singleton → pair. Distinct adjacent data sentinels verify the second order uses the singleton's instruction-local `next_rip`; budgets 0..4 compare full CPU state and data RAM against interpreter steps, and continuation resumes from the actual returned RIP. Partial byte windows 1..6 return `BoundaryReached` with zero retirement, publication, or data callbacks. The RIP-relative native host test compares direct V2 and shared IR against interpreter for negative, zero and positive disp32 at an entry inside CodeView. Shared-IR load/store additionally check architectural fault payload and atomic CPU/RAM/budget state, backend errors as host errors, and STOP after successful store commit. These tests are local macOS ARM64 fixture evidence; owner/MMU integration, physical x86, and performance remain open.

### S3-20m signed direct-base displacement singleton

Run `cargo test -p od-jit --lib ir_checked_direct_disp8_disp32_sites_match_decoded_metadata`, `cargo test -p od-jit --lib ir_checked_direct_displacements_reject_prefix_register_and_trailing_forms`, `cargo test -p od-host v2_rip_relative_mov_success_targets_follow_guest_rip`, and `cargo test -p od-host bounded_dispatch_composes_rip_relative_nop_and_pair_with_actual_rip_continuation`. Direct V2/shared IR/interpreter comparisons cover disp8 -128/-1/0/127 and disp32 min/-1/0/1/max for both operations; load destination equals base and store source equals base. Cases cover signed wraparound into canonical memory, noncanonical start and end faults with no callback, backend errors, and STOP after committed store. The dispatcher test checks disp8 singleton → bare NOP → existing pair at budgets 0..4, pre-entry STOP, continuation from the returned RIP without repeated accesses, and truncated disp8 windows 1..3 plus disp32 windows 1..6 without publication or data access. These remain local macOS ARM64 fixtures, not owner/MMU or performance evidence.

### S3-20n no-displacement SIB singleton

Run `cargo test -p od-jit --lib ir_checked_sib_singletons_match_decoded_metadata_for_all_registers_and_scales`, `cargo test -p od-host v2_sib_mov_all_register_scale_combinations_match_shared_ir_and_interpreter`, and `cargo test -p od-host bounded_dispatch_composes_rip_relative_nop_and_pair_with_actual_rip_continuation`. Compiler metadata and native differential cases enumerate both MOV directions, all RAX..RBX data/base/index encodings, and scales 1/2/4/8; aliases and zero-valued index are included. Descriptor-driven native cases cover multiply/add wrapping to canonical address zero, noncanonical start/end fault atomicity, load/store backend error classification, exact architectural store fault payload with unchanged CPU/RAM/budget/retirement, and committed SIB store followed by STOP. Dispatcher budgets 0..4 compare full CPU/RAM to interpreter for SIB → NOP → pair; STOP after the SIB load pauses before NOP and resumes from actual RIP without repeating the access. Byte windows ending before the SIB byte return `BoundaryReached` without retirements/data callbacks. Negative cases reject truncated SIB, no-index/no-base, RSP/RBP, extension bits, displacement, `67`, and trailing bytes. The direct disp8 `mod=01` four-byte form remains separately tested. Evidence is local ARM64 fixture only; owner/MMU integration, physical x86, and performance remain open.

### S3-20o address32 direct-base qword singleton

Run `cargo test -p od-jit --lib ir_address32_direct_base_mov_has_exact_qword_descriptor`, `cargo test -p od-host ir_address32_mov_matches_v2_and_interpreter_with_qword_access_semantics`, and `cargo test -p od-host bounded_dispatch_composes_rip_relative_nop_and_pair_with_actual_rip_continuation`. The accepted source is exactly `67 48 8B/89 /r` with ModRM `mod=00`, base/data RAX..RBX, qword data width and dword address width. Native direct V2/shared IR/interpreter comparisons cover all base/data pairs and both operations, including destination/source alias with base and noncanonical full u64 base values. The base `0xffff_fffc` case checks one contiguous qword callback spanning through `0x1_0000_0003`. Load and store architectural faults check exact payload and no CPU/RAM/budget/retirement commit; backend store errors stay host errors; zero-budget, stop-RIP and STOP guards make no callback; successful store+STOP preserves bytes and retirement. Mixed dispatch exercises address32 singleton → NOP → existing pair with budgets 0..4, stop-before-NOP and continuation without repeated access. Partial prefix/opcode/ModRM windows and wrong prefix order, SIB, displacements, extra bytes and unsupported registers are rejected. Evidence is local ARM64 fixture only; owner/MMU integration, physical x86, and performance remain open.

### S3-20p address32 signed-displacement qword singleton

Run `cargo test -p od-jit --lib address_size_override_signed_displacements_are_admitted_by_decoded_shape`, `cargo test -p od-host v2_address32_signed_displacements_match_interpreter_and_cross_32bit_boundary_linearly`, and `cargo test -p od-host bounded_dispatch_composes_rip_relative_nop_and_pair_with_actual_rip_continuation`. The shared checked source is exactly `67 48 8B/89 /r disp8|disp32`, ModRM `mod=01/10`, data/base RAX..RBX, lengths 5/8, qword data and dword EA. Descriptor and native cases cover disp8 -128/-1/0/1/127 and disp32 MIN/MIN+1/-1/0/1/MAX-1/MAX, both operations, base alias, noncanonical u64 base bits, and modulo-2^32 wrapping. Callback records assert exactly one qword access at the decoded target, including a contiguous access beginning at `0xffff_fffc`. Shared IR exact architectural fault payloads leave CPU/RAM/budget/retirement unchanged; backend errors remain host errors; store+STOP preserves the committed qword. Dispatcher composes disp8 → bare NOP → existing pair across budgets 0..4 and resumes at the returned RIP; partial disp8 and disp32 windows return `BoundaryReached` without publication/access. Incorrect prefix, SIB, unsupported registers, trailing bytes, and wrong lengths remain rejected. Evidence is local ARM64 fixture only; owner/MMU integration, physical x86, and performance remain open.

### S3-20q 64-bit SIB signed-displacement qword singleton

Run `cargo test -p od-jit --lib ir_checked_sib_displacements_match_decoded_metadata_and_reject_other_shapes`, `cargo test -p od-host v2_sib_mov_all_register_scale_combinations_match_shared_ir_and_interpreter`, and `cargo test -p od-host bounded_dispatch_composes_rip_relative_nop_and_pair_with_actual_rip_continuation`. Exact accepted sources are `48 8B/89 /r sib disp8|disp32`, ModRM `mod=01/10,r/m=100`, base/index/data RAX..RBX, scale 1/2/4/8, lengths 5/8. Descriptor and host matrices cover disp8 -128/-1/0/127 and disp32 MIN/-1/0/MAX across all scales and both operations; alias cases include data/base, data/index, and base/index for both displacement widths. Native direct V2/shared IR/interpreter state and RAM parity, callback address and count are checked. Canonical EA wrap, noncanonical start/end, full architectural fault payload/no commit, backend host errors, and committed store+STOP are covered. Dispatcher composes SIB disp8 → NOP → existing pair for budgets 0..4; STOP resumes from the actual RIP without repeating the SIB access. Partial disp8/disp32 and invalid no-index SIB windows do not publish or call memory. Evidence is local ARM64 fixture only; owner/MMU integration, physical x86, and performance remain open.

### S3-20r address32 no-displacement SIB qword singleton

Run `cargo test -p od-jit --lib ir_address32_sib_no_displacement_has_exact_qword_descriptor`, `cargo test -p od-host v2_address32_sib_mov_matches_direct_ir_and_interpreter`, and `cargo test -p od-host bounded_dispatch_composes_rip_relative_nop_and_pair_with_actual_rip_continuation`. The exact shape is `67 48 8B/89 /r sib`, ModRM `mod=00,r/m=100`, data/base/index RAX..RBX, scale 1/2/4/8, five bytes, qword data and dword EA. Descriptor cases enumerate both operations and every supported data/base/index/scale combination; forbidden no-index/no-base, displaced SIB, extra-prefix and trailing forms reject. Direct V2/shared IR/interpreter parity checks CPU/RAM, callback address/count, aliases, ignored high halves, modulo-2^32 wrap and contiguous qword access at `0xffff_fffc`. Architectural faults preserve exact payload and no state/RAM/budget/retirement commit, backend errors stay host errors, and successful SIB store+STOP remains committed. Dispatcher composes addr32 SIB → NOP → pair at budgets 0..4, tests no-effect guards and partial windows, and resumes from actual RIP without repeating the first access. Evidence is local ARM64 fixture only; owner/MMU integration, physical x86, and performance remain open.

### S3-20s address32 signed disp8 SIB qword singleton

Run `cargo test -p od-jit --lib ir_address32_sib_disp8_has_exact_qword_descriptor`, `cargo test -p od-host v2_address32_sib_mov_matches_direct_ir_and_interpreter`, and `cargo test -p od-host bounded_dispatch_composes_rip_relative_nop_and_pair_with_actual_rip_continuation`. The accepted source is exactly `67 48 8B/89 /r sib disp8`, ModRM `mod=01,r/m=100`, data/base/index RAX..RBX, scale 1/2/4/8, six bytes, qword data and dword EA. Descriptor tests cover both directions, every supported register/scale combination, disp8 `-128/-1/0/127`, and reject truncated bytes, wrong ModRM/SIB fields, wrong prefix, trailing bytes and disp32. Native direct V2/shared IR/interpreter parity verifies state/RAM, one callback at the decoder-derived qword target, aliases, upper-half ignore, signed modulo-2^32 arithmetic, and a contiguous access across `0xffff_ffff`. Fault payloads preserve state/RAM/budget/retirement, backend errors remain host errors, and store+STOP preserves the committed write. Dispatcher budgets 0..4 compare against interpreter; a fixture ending exactly after the six-byte singleton proves it is not treated as a pair by length. NOP/pair continuation resumes without repeating the first access. Evidence is local ARM64 fixture only; owner/MMU integration, physical x86, and performance remain open.

### S3-20t address32 signed disp32 SIB qword singleton

Run `cargo test -p od-jit --lib ir_address32_sib_disp32_has_exact_nine_byte_qword_descriptor`, `cargo test -p od-host v2_address32_sib_mov_matches_direct_ir_and_interpreter`, and `cargo test -p od-host bounded_dispatch_runs_address32_sib_disp32_nop_and_pair_continuously`. The exact source is `67 48 8B/89 /r sib disp32`, ModRM `mod=10,r/m=100`, data/base/index RAX..RBX, scale 1/2/4/8, qword data and dword EA. Descriptor coverage checks both directions, every supported register/scale combination and disp32 MIN/-1/0/MAX. Host direct V2/shared IR/interpreter parity checks callback address/count, complete state/RAM, RAX base/data alias, high-half truncation and contiguous qword access at `0xffff_fffc`; sources differing only in byte nine retain distinct metadata. Architectural faults preserve exact payload and no CPU/RAM/budget/retirement commit, backend errors remain host errors, and successful store+STOP preserves the committed write. Dispatcher composes the nine-byte singleton → bare NOP → existing pair at budgets 0..4, checks stop-RIP and callback STOP, then resumes at the actual RIP without repeating the first access. Partial windows 1..8 and invalid SIB/ModRM/prefix/tail encodings reject before a data callback. `source_bytes_storage()` now returns `([u8; 9], u8)`; qword data buffers, pair `[u8; 6]`, ActiveCodeV3 `[u8; 23]` and RuntimeContext/C ABI remain unchanged. Evidence is local ARM64 fixture only; owner/MMU integration, physical x86, and performance remain open.

### S3-20u RBP-base signed disp8 qword singleton

Run `cargo test -p od-jit --lib ir_rbp_disp8_has_exact_qword_descriptor_and_rejects_other_rbp_forms`, `cargo test -p od-host v2_address32_sib_mov_matches_direct_ir_and_interpreter`, and `cargo test -p od-host bounded_dispatch_runs_address32_sib_disp32_nop_and_pair_continuously`. The exact source is `48 8B/89 /r disp8`, ModRM `mod=01,r/m=101`, RBP base, data RAX..RBX, qword data, 64-bit EA and exact length four. Descriptor/direct V2/shared IR/interpreter cases cover both directions and disp8 -128/-1/0/127 with callback address/count and full state/RAM parity; RBP and RSP are unchanged. Noncanonical RBP start and last-byte crossing return #SS (12/0/no linear address), while matching RBX cases return #GP (13/0/no linear address), all before callbacks and with no state/budget/retirement commit. AddressOverflow retains the host diagnostic kind, backend failures remain host errors, entry STOP/budget zero do not access memory, and a successful store+STOP stays committed. Bounded dispatch composes RBP disp8 → bare NOP → existing pair for budgets 0..4 and continuation from the actual RIP without duplicate callbacks. Truncation and unsupported prefix/SIB/disp32/extended forms reject. Evidence is local ARM64 fixture only; owner/MMU integration, physical x86, and performance remain open.

### S3-20v RSP-base exact no-index SIB qword singleton

Run `cargo test -p od-jit --lib ir_rsp_base_no_index_sib_is_exactly_the_0x24_form`, `cargo test -p od-host v2_address32_sib_mov_matches_direct_ir_and_interpreter`, and `cargo test -p od-host bounded_dispatch_runs_address32_sib_disp32_nop_and_pair_continuously`. The only accepted encoding is `48 8B/89 /r sib`, ModRM `mod=00,r/m=100`, exact SIB `0x24`; RSP is the base, index is absent, scale is 1, displacement is zero, data is RAX..RBX, qword/64-bit EA, and length is four. Direct V2/shared IR/interpreter comparisons cover both operations, all data registers, callback target/count, full state/RAM and retirement with RSP/RBP/flags unchanged. Noncanonical RSP start and last-byte range produce #SS; the corresponding RBX controls produce #GP, with exact vector/error/linear payload and no callback/state/RIP/RAM/budget/retirement commit. AddressOverflow and ordinary backend error remain separate host classifications. Entry STOP/budget zero make no callback and successful store+STOP retains its write. Dispatcher compares budgets 0..4, split continuation after the RSP singleton and partial windows 1..3; invalid SIB/index/scale/displacement/prefix/RSP-data variants return Unsupported before publication or memory callbacks. Evidence is local ARM64 fixture only; owner/MMU integration, physical x86, and performance remain open.

### S3-20w RSP-base no-index SIB with signed disp8

Run `cargo test -p od-jit --lib ir_rsp_base_no_index_sib_disp8_is_exact_and_signed`, `cargo test -p od-host v2_address32_sib_mov_matches_direct_ir_and_interpreter`, and `cargo test -p od-host bounded_dispatch_runs_address32_sib_disp32_nop_and_pair_continuously`. The exact singleton profile is `48 8B/89 /r sib disp8`, ModRM `mod=01,r/m=100`, SIB `0x24`, RSP base, no index/scale 1, signed disp8, data RAX..RBX, qword and 64-bit EA, length five. The existing no-displacement `[RSP]` form stays separate. Direct V2/shared IR/interpreter tests cover both operations and -128/-1/0/127 with complete state/RAM, callback address/count, retirement and unchanged RSP/RBP/flags. Fault tests retain #SS for RSP and #GP controls, no memory callback/commit, distinct AddressOverflow/backend errors, entry STOP/budget-zero guards, and committed store+STOP. Dispatcher checks budgets 0..4, STOP-RIP before the NOP bridge, continuation from the singleton's actual next RIP without replay, pair execution, partial windows 1..4 and invalid index/disp32/prefix variants without callbacks. Evidence is local ARM64 fixture only; owner/MMU integration, physical x86 and performance remain open.

### S3-20x RSP-base no-index SIB with signed disp32

Run `cargo test -p od-jit --lib ir_rsp_base_no_index_sib_disp32_is_exact_and_keeps_all_source_bytes`, `cargo test -p od-host v2_address32_sib_mov_matches_direct_ir_and_interpreter`, and `cargo test -p od-host bounded_dispatch_runs_address32_sib_disp32_nop_and_pair_continuously`. The exact singleton is `48 8B/89 /r sib disp32`, ModRM `mod=10,r/m=100`, SIB exactly `0x24`, RSP base/no index/scale 1, disp32 at source offsets 4..7, data RAX..RBX, qword/64-bit EA, length eight. Descriptor checks both operations, all data registers, signed MIN/-1/0/MAX and exact all-byte source identity; two sources differing only at byte 7 decode to distinct offsets. Host direct V2/shared IR/interpreter checks both operations at signed boundaries with one contiguous callback across `0xffff_ffff`, plus a positive address sum wrapping to canonical zero. Fixed-RSP sources differing only in byte 7 yield the explicit different callback addresses `0x0000_7000_0100_5020` and `0x0000_7000_0200_5020`. RSP #SS/RBX #GP start/end controls, AddressOverflow and backend classification, no commit, entry STOP/budget-zero and successful store+STOP are checked. Dispatcher composes singleton→NOP→pair at budgets 0..4, STOP-RIP and continuation without replay; windows 1..7 and invalid no-index/profile/prefix shapes have no data effects. Existing `[RSP]` and disp8 tests remain. Evidence is local ARM64 fixture only; owner/MMU integration, physical x86 and performance remain open.

### S3-20y RBP-base signed disp32 qword singleton

Run `cargo test -p od-jit --lib ir_checked_rbp_base_disp32_is_distinct_from_rip_relative_disp32`, `cargo test -p od-host v2_address32_sib_mov_matches_direct_ir_and_interpreter`, and `cargo test -p od-host bounded_dispatch_runs_address32_sib_disp32_nop_and_pair_continuously`. The exact source is `48 8B/89 /r disp32`, ModRM `mod=10/rm=101`, RBP base, data RAX..RBX, qword/64-bit EA and length seven. Descriptor/direct V2/shared IR/interpreter tests cover both operations and disp32 MIN/-1/0/127/128/MAX, full state/RAM parity, callback address/count and retirement. The same displacement under `mod=00/rm=101` is explicitly checked to use instruction-local NextRip and yield a different expected target. RBP noncanonical start/end gives exact #SS payload without callback while RBX controls give #GP; AddressOverflow and backend failures remain distinct host errors with no commit. STOP/budget-zero suppress callbacks; successful store+STOP remains committed. Dispatcher composes RBP disp32→bare NOP→pair across budgets 0..4, STOP-RIP and continuation from actual RIP without replay; partial windows 1..6 have no effects. Evidence is local ARM64 fixture only; owner/MMU integration, physical x86 and performance remain open.


### S3-20z guest RSP as qword MOV data register

Run `cargo test -p od-jit --lib rsp_is_admitted_only_as_qword_memory_data_for_exact_direct_forms` and `cargo test -p od-host rsp_is_an_exact_qword_memory_data_register_and_commits_before_next_use`. Only exact three-byte `48 8B/89 /r` with ModRM `mod=00`, data RSP and base RAX..RBX is admitted; generic GPR read/write validation remains limited to RAX..RBX. Direct V2/shared IR/interpreter parity covers both load and store, full-width RSP values, state/RAM, callback target/count and retirement. A loaded noncanonical value commits to guest RSP only after successful read; store parity proves all 64 bits of `0x0000_8000_0000_0000` reach RAM, since canonicality applies to the effective address. The IR frame stages GPRs at `SP+56..95`, flags at `SP+96`, RIP at `SP+104`, and preserves X19/X20 and X21/X22 at `SP+112` and `SP+128`; the required reserved prefix is 144 bytes. A native x19 canary covers successful return, budget guard and architectural fault. Entry STOP and budget zero perform no callback; a budget-one split resumes at the actual RIP without repeating the load. Negative encoding variants remain unsupported. The sequential dispatcher case commits `MOV RSP,[RAX]`, then faults with #SS on `MOV RCX,[RSP]` at the second RIP, preserving the first effect and not repeating its read on callback STOP/resume. Evidence is local ARM64 fixture only; owner/MMU integration, physical x86 and performance remain open.


### S3-20aa guest RBP as qword MOV data register

Run `cargo test -p od-jit --lib rbp_is_admitted_only_as_qword_memory_data_for_exact_direct_forms`, `cargo test -p od-jit --lib rbp_memory_data_spill_stays_after_six_gpr_runtime_prefix`, and `cargo test -p od-host rsp_is_an_exact_qword_memory_data_register_and_commits_before_next_use`. Admit only exact three-byte `48 8B/89 /r`, ModRM `mod=00`, data RBP and base RAX..RBX. Generic GPR validation stays restricted, and the existing pair-profile compiler rejects RBP-data pair sequences. Direct V2/shared IR/interpreter tests compare full CPU/RAM state, callback address/count and retirement for load/store; a noncanonical RBP payload is stored as all eight bytes. Failed accesses preserve old RBP. The six-slot frame places RAX..RBP at SP+56..103, flags +104, RIP +112, X19/X20 at +128 and X21/X22 at +144, with 160-byte prefix; forced spill and emitted PC-map regression covers allocator storage after the prefix. x19 canary covers repeated success, budget guard and fault returns. Dispatcher checks entry STOP/budget-zero, budget-one and callback-STOP split/resume, then `MOV RBP,[RAX]; MOV RCX,[RBP+0]` faults #SS on the second instruction while preserving the first commit and avoiding repeated reads. Evidence is local ARM64 fixture only; owner/MMU integration, physical x86 and performance remain open.

### S3-20ab guest RBP as both qword MOV data and disp8 base

Run `cargo test -p od-jit --lib ir_rbp_rbp_disp8_has_exact_dual_register_descriptor`, `cargo test -p od-jit --lib ir_rbp_disp8_has_exact_qword_descriptor_and_rejects_other_rbp_forms`, and `cargo test -p od-host rbp_data_disp8_and_disp32_use_old_rbp_for_address_and_dispatch_continuation`. Only exact four-byte `48 8B/89 6D ib` is admitted: ModRM `mod=01/reg=5/rm=5`, both operand data register and decoded address base RBP, signed disp8, qword data and 64-bit EA. Direct V2/shared IR/interpreter compare both operations at -128/-1/0/127, complete CPU/RAM state, callback target/count and retirement. The load callback target is calculated from the old RBP even though the instruction commits a different value into RBP; stores use old RBP both as address base and full 64-bit payload. A current noncanonical RBP access returns #SS before the backend callback and preserves state/RAM/budget/retirement. The dispatcher budget split and callback STOP both commit the first load, resume at its actual next RIP, fault on the subsequent RBP-relative access, and never repeat the first read. Existing RBP-data-only, RBP-base-only and RSP-data tests remain covered. Evidence is local ARM64 fixture only; production owner/MMU integration, physical x86, Mini and performance remain open.

### S3-20ac guest RSP as both qword MOV data and exact SIB base

Run `cargo test -p od-jit --lib ir_rsp_data_and_base_is_only_exact_0x24_sib_form`, `cargo test -p od-jit --lib ir_rsp_base_no_index_sib_is_exactly_the_0x24_form`, and `cargo test -p od-host rsp_is_an_exact_qword_memory_data_register_and_commits_before_next_use`. Only exact four-byte `48 8B/89 24 24` is admitted: ModRM `mod=00/rm=100`, reg/data RSP and SIB `0x24` (RSP base, no index, scale 1, no displacement). Direct V2/shared IR/interpreter compare full CPU/RAM, callback target/count and retirement for load/store. Both the effective address and store payload use old RSP; load commits a distinct new RSP after successful access. Current noncanonical `[RSP]` load/store returns #SS without a backend callback or state/RAM/budget/retirement change. Exact-profile x19 canary covers repeated native success plus budget guard and fault. Dispatcher budget-one and callback-STOP runs commit the first `[RSP]` load, resume at its actual next RIP and fault on a second `[RSP]` without repeating the first read. Partial SIB and invalid no-base/scale/index/displacement/prefix variants stop before publication and helper calls. The prior direct RSP-data-only and RSP-base SIB forms remain covered. Evidence is local ARM64 fixture only; production owner/MMU integration, physical x86, Mini and performance remain open.

### S3-20ad guest RSP as both qword MOV data and signed-disp8 SIB base

Run `cargo test -p od-jit --lib ir_rsp_data_and_base_disp8_is_exact_signed_0x64_sib_form` and `cargo test -p od-host --lib rsp_is_an_exact_qword_memory_data_register_and_commits_before_next_use`. Admit only exact five-byte `48 8B/89 64 24 ib`: ModRM `mod=01/reg=4/rm=4`, SIB exactly `0x24`, signed disp8, qword data and 64-bit EA. The descriptor test checks both opcodes, signed disp8 -128/-1/0/127, and rejects truncated input, altered ModRM/SIB, disp32, prefix and trailing-byte variants. Host parity compares direct V2/shared IR/interpreter CPU and RAM state, callback address/count and retirement for both operations; effective address and store payload use entry RSP, while load commits a different RSP only after successful access. Architectural #SS occurs before the backend callback and preserves the instruction state. Address-overflow and backend errors remain distinct host errors. Budget zero and entry STOP suppress data access. Dispatcher budget-one and callback-STOP runs execute `MOV RSP,[RSP-1]`, resume at `ENTRY+5`, then fault #SS on `[RSP]` while retaining the first commit and leaving read count at one. A separate `48 89 64 24 FF` dispatch writes all eight bytes of old RSP at address 32; the successful write callback raises STOP, and the dispatcher returns at `ENTRY+5` with one native retirement and the write preserved. The exact profile is included in the x19 canary path. Existing RSP data-only, RSP base and no-displacement RSP data/base forms remain covered. Evidence is local ARM64 fixture only; owner/MMU integration, physical x86, Mini and performance remain open.

### S3-20ae guest RSP as both qword MOV data and signed-disp32 SIB base

Run `cargo test -p od-jit --lib ir_rsp_data_and_base_disp32_is_exact_full_source_0xa4_sib_form` and `cargo test -p od-host --lib rsp_is_an_exact_qword_memory_data_register_and_commits_before_next_use`. Admit only exact eight-byte `48 8B/89 A4 24 id`: ModRM `mod=10/reg=4/rm=4`, SIB exactly `0x24`, signed disp32 bytes 4..7, qword data and 64-bit EA. Tests compare direct V2/shared IR/interpreter full CPU/RAM state, callback address/count and retirement at disp32 MIN/-1/0/MAX; effective address is explicitly entry RSP plus the decoded signed displacement, and a load commits a different RSP. Sources differing only at byte 7 produce exact expected callback addresses `0x01000040` and `0x02000040`. Current noncanonical accesses produce #SS before data callback; qword range overflow and backend failures remain host errors without architectural commit. The exact source is covered by the x19 canary at success, zero-budget and fault exits. Dispatcher checks partial windows 1..7 and invalid SIB/prefix forms; it also verifies `MOV RSP,[RSP+0]` commits a noncanonical loaded value at RIP+8 before the following `[RSP]` faults #SS without another callback, across both budget split and callback STOP. The mixed singleton→NOP→pair test proves exact native/interpreter counters and no replay. A store callback STOP after `48 89 A4 24 FF FF FF FF` commits old RSP to address 32 and returns at `ENTRY+8`. Existing RSP base/no-displacement and signed-disp8 profiles remain covered. Evidence is local ARM64 fixture only; owner/MMU integration, physical x86, Mini and performance remain open.

### S3-20af guest RBP as both qword MOV data and signed-disp32 base

Run `cargo test -p od-jit --lib ir_rbp_data_and_base_disp32_is_exact_ad_form_and_distinct_from_rip_relative` and `cargo test -p od-host rbp_data_disp8_and_disp32_use_old_rbp_for_address_and_dispatch_continuation`. The only added encoding is the seven-byte `48 8B/89 AD id` singleton: ModRM `mod=10/reg=5/rm=5`, disp32 bytes 3..6. Direct V2/shared IR/interpreter parity covers both operations at disp32 MIN/-1/0/MAX, full CPU/RAM state, callback target and retirement; address and store value use entry RBP. Descriptor checks pass actual RBP and NextRip operands to `access_address_48`, compare both results with independent formulas and assert byte-six variants reach exact callback targets `0x01000040`/`0x02000040`. Partial windows1..6 and malformed SIB/prefix forms reject. Exact-profile tests cover #SS without callbacks, AddressOverflow and unmapped backend-error classification, and the macOS/ARM64 x19 canary. Dispatcher budget split commits a noncanonical loaded RBP at `ENTRY+7`; continuation faults #SS without repeating the first read. A mixed AD singleton→NOP→existing pair run checks native/interpreter/native retirements 1/1/2 across actual-RIP continuations. Existing exact data/base-only and RIP-relative forms remain covered. Local ARM64 fixture only; production owner/MMU integration, physical x86 and performance remain open.

### S3-20ag RBP data with the exact RSP-base SIB

Run `cargo test -p od-jit --lib ir_rbp_data_with_rsp_sib_base_is_only_exact_0x2c_24_form` and `cargo test -p od-host rsp_is_an_exact_qword_memory_data_register_and_commits_before_next_use`. Admit only exact four-byte `48 8B/89 2C 24`: ModRM `mod=00/reg=5/rm=4`, SIB `0x24`, RSP base, no index, scale 1, no displacement, qword/64-bit EA. Direct V2/shared IR/interpreter parity checks complete state/RAM, callbacks and retirement for both operations with distinct RSP and RBP. The effective address uses old RSP; store payload uses all 64 bits of old RBP, and load updates only RBP after a successful read. Noncanonical RSP returns #SS before a callback; noncanonical RBP remains valid store data. Exact store callback STOP preserves the write. Dispatcher resumes after `MOV RBP,[RSP]` at actual RIP and uses the newly loaded RBP for the next access without repeating the first read. Truncated/changed SIB, scale/index, data-register, displacement and prefix variants reject. Existing RSP/RBP profiles and canary/lifecycle coverage remain green.

### S3-20ah RBP data with signed displacement from an RSP-base SIB

Run `cargo test -p od-jit --lib ir_rbp_data_with_rsp_sib_base_disp8_and_disp32_is_exact` and `cargo test -p od-host rsp_is_an_exact_qword_memory_data_register_and_commits_before_next_use`. Only exact singleton `48 8B/89 6C 24 ib` (length 5, disp8 at byte 4) and `48 8B/89 AC 24 id` (length 8, disp32 at bytes 4..7) are admitted. ModRM selects `mod=01/10, reg=5, r/m=4`; SIB is exactly `0x24`, meaning RSP base, no index, scale 1. Direct V2/shared IR/interpreter parity covers both opcodes, signed boundaries and complete CPU/RAM/callback/retirement state. The target is entry RSP plus signed displacement; store data is full entry RBP, and load commits only RBP after a successful read. Noncanonical RSP yields exact #SS before callback; noncanonical RBP store data is permitted. AddressOverflow and backend errors remain host errors. Store callback STOP keeps the write. Dispatcher checks singleton→NOP→pair and displaced RSP-base load followed by an RBP-relative access using the loaded RBP, with actual-RIP continuation and no repeated access. Partial windows and malformed SIB/ModRM/prefix shapes have no data effects. Evidence is local ARM64 fixture only; owner/MMU integration, physical x86 and performance remain open.

### S3-20ai RSP data with signed displacement from an RBP base

Run `cargo test -p od-jit --lib ir_rsp_data_with_rbp_base_disp8_and_disp32_is_exact` and `cargo test -p od-host rsp_data_with_rbp_base_displacements_preserves_old_rbp_and_continues_from_rip`. Admit only exact `48 8B/89 65 ib` (disp8, length 4) and `48 8B/89 A5 id` (disp32, length 7), with ModRM `mod=01/10, reg=4, rm=5`. The decoded `MemoryOperand` supplies RBP base plus signed displacement; data is qword RSP. Direct V2/shared IR/interpreter parity compares complete CPU/RAM, exact callback target/count and retirement for both operations at disp8 -128/-1/0/127 and disp32 MIN/-1/0/MAX/128. Noncanonical RSP remains valid data; noncanonical RBP EA yields exact #SS without callback or commit. Store callback STOP commits all old RSP bits. Disp32 sources differing only in byte 6 reach independently asserted addresses `0x01000000` and `0x02000000` with fixed RBP. Dispatcher tests entry STOP and budget-zero guards, budget/STOP actual-RIP split and following `[RSP]` use of the loaded RSP without replay; loading noncanonical RSP commits first, then `[RSP]` faults #SS without another callback. Partial windows and forbidden SIB/prefix/ModRM forms stop before publication or memory effects. An explicit `mod=00/rm=5` control remains RIP-relative and targets instruction NextRip; existing RBP-data `mod=10/rm=5` tests remain green. Exact-profile x19 canary covers both lengths on macOS/ARM64. Evidence is local ARM64 fixture only; owner/MMU integration, physical x86 and performance remain open.

### S3-20aj RSP/RBP data with signed displacement from RAX..RBX

Run `cargo test -p od-jit --lib ir_rsp_rbp_data_with_low_direct_base_displacements_is_exact` and `cargo test -p od-host rsp_data_with_rbp_base_displacements_preserves_old_rbp_and_continues_from_rip`. Admit exact singleton `48 8B/89 /r disp8` (length 4) or `48 8B/89 /r disp32` (length 7), ModRM `mod=01/10`, data field RSP/RBP, base field RAX..RBX. The decoded `MemoryOperand` supplies signed displacement and effective-address semantics. No SIB, prefixes, other base/data registers, pairs or V3 are added.

The host matrix compares direct V2, shared IR and interpreter for all four bases, both data registers, both widths, loads/stores and signed boundaries. It checks full CPU/RAM state, exact callback targets/counts and retirement. Noncanonical RSP/RBP payloads remain valid data, while noncanonical low-GPR effective addresses raise #GP before callback. Store callback STOP keeps the write. Dispatcher coverage composes the singleton with NOP fallback and the existing pair; budget and callback-STOP continuations resume at actual RIP without replay. A noncanonical loaded RSP/RBP commits first, then use as the next address raises #SS without another callback. Partial windows and invalid SIB/register/prefix near matches have no effects. Existing RSP-data/RBP-base disp32, RBP-data/RBP-base, RSP-base SIB and RIP-relative profiles remain covered. See the [S3-20aj report](../reports/STAGE_3_S3-20aj_2026-10-27.md).

### S3-20ak RIP-relative MOV with RSP/RBP data

Run `cargo test -p od-jit --lib ir_rip_relative_rsp_rbp_data_keeps_exact_next_rip_profile`, `cargo test -p od-host --lib v2_rip_relative_mov_success_targets_follow_guest_rip`, and `cargo test -p od-host --lib bounded_dispatch_composes_rip_relative_nop_and_pair_with_actual_rip_continuation`. Admit only exact seven-byte `48 8B/89 25 disp32` (RSP data) and `48 8B/89 2D disp32` (RBP data), with ModRM `mod=00/rm=101`; SIB, prefixes, other data registers, pairs and V3 remain unsupported. Direct V2/shared IR/interpreter parity checks both operations at an instruction entry inside CodeView, exact state/RAM/callback counts and retirement, with the target independently computed from this instruction's `NextRip + signed disp32`. Noncanonical NextRip targets yield #GP before callbacks for both stack data registers. Loading a noncanonical RSP/RBP payload is valid; dispatching the next `[RSP]` / `[RBP+0]` access yields #SS and preserves the first retirement without a second memory callback. Exact-profile store callbacks that request STOP retain the write and report the following RIP. NOP→singleton→two following singleton memory operations compare budgets 0..4 and a split continuation from actual RIP without repeating the initial access. Partial byte windows 1..6 and invalid encoding shapes reject before effects. See the [S3-20ak report](../reports/STAGE_3_S3-20ak_2026-10-27.md).


### S3-20al RSP/RBP data through indexed 64-bit SIB

Run `cargo test -p od-jit --lib ir_rsp_rbp_data_with_indexed_sib_keeps_exact_qword_profile`, `cargo test -p od-host --lib v2_sib_mov_all_register_scale_combinations_match_shared_ir_and_interpreter`, and `cargo test -p od-host --lib bounded_dispatch_composes_rip_relative_nop_and_pair_with_actual_rip_continuation`. The added data-register encodings are only RSP/RBP in the existing indexed SIB subset: ModRM `r/m=100`, base/index RAX..RBX, scale 1/2/4/8, qword and 64-bit EA. No-displacement/disp8/disp32 lengths are 4/5/8; the decoded `MemoryOperand` remains authoritative for address semantics.

The descriptor and host matrices cover load/store, all scales, base=index aliases, signed displacement edges, full state/RAM and callback parity against the interpreter. Noncanonical effective addresses from low GPRs fault as #GP before callbacks; noncanonical RSP/RBP values remain valid payloads and fault as #SS only if subsequently used as stack bases. Successful memory access commits before STOP, and budget/STOP continuation resumes at actual RIP without repeating the access. The bounded dispatcher test also checks partial instruction windows without data callbacks. A macOS/ARM64 x19 canary covers this exact SIB stack-data path across success, budget guard and backend fault. Existing exact RSP-base SIB and prior RSP/RBP profiles remain covered. This is borrowed-fixture execution evidence; production owner/MMU integration, physical x86 and performance remain open. See the [S3-20al report](../reports/STAGE_3_S3-20al_2026-10-27.md).


### S3-20am RSP/RBP data with address32 direct-base qword MOV

Run `cargo test -p od-jit --lib address32_direct_rsp_rbp_data_is_exact_and_keeps_qword_width` and `cargo test -p od-host --lib address32_direct_rsp_rbp_data_keeps_qword_values_and_wraps_contiguously`. Admit only `67 48 8B/89 /r` direct singleton forms with ModRM `mod=00/01/10`, data register RSP or RBP, base RAX..RBX, Dword EA and Qword data; lengths are 4/5/8. SIB, RIP-relative and additional/reordered prefixes remain unsupported. The decoder `MemoryOperand` supplies the address semantics.

The descriptor and host tests cover both operations, all four bases, no displacement and signed disp8/disp32 edges. They compare direct V2, shared IR and interpreter state/RAM/retirement and exact callback target/count. Base registers have nonzero upper halves, and each case independently targets `0xffff_fffc`; the qword is one contiguous callback ending at `0x1_0000_0003`. Full 64-bit stack-data payloads are retained. A callback failure preserves guest state and retirement; a successful store callback STOP commits once. Dispatcher budget split loads a noncanonical RSP as valid qword data, then the next 64-bit `[RSP]` access raises #SS while retaining the first retirement and avoiding a duplicate callback. Entry STOP/budget-zero, partial windows and invalid source shapes stop before helper access. This is borrowed fixture evidence; production owner/MMU integration, physical x86, Mini and performance remain open. See the [S3-20am report](../reports/STAGE_3_S3-20am_2026-10-27.md).

### S3-20an RSP/RBP data through address32 indexed SIB

Run `cargo test -p od-jit --lib address32_indexed_sib_rsp_rbp_data_has_exact_qword_profile`, `cargo test -p od-host --lib v2_address32_sib_mov_matches_direct_ir_and_interpreter`, and `cargo test -p od-host --lib bounded_dispatch_runs_address32_sib_disp32_nop_and_pair_continuously`. Admit only `67 48 8B/89 /r sib [disp8|disp32]`, ModRM `mod=00/01/10,r/m=100`, data RSP/RBP, SIB base/index RAX..RBX and scale 1/2/4/8. Address size is Dword, data size Qword; exact instruction lengths are 5/6/9 bytes. No-index/no-base special encodings, RSP/RBP base/index, additional prefixes, pairs and V3 are excluded.

Descriptor and host cases cover load/store, all scales, base=index aliases, signed disp8/disp32 bounds, upper-half truncation, modulo-2^32 addresses and direct V2/shared IR/interpreter state/RAM/retirement parity. Callback records verify one contiguous qword access across `0xffff_ffff`; sources whose ninth byte differs assert callback targets `0x20` and `0x0100_0020`. Exact-profile architectural and backend failures leave architectural state uncommitted; a successful store callback STOP retains all eight bytes of a noncanonical RSP data payload. Bounded dispatch runs the stack-data disp32 singleton → NOP → existing pair and separately loads noncanonical RSP/RBP values before `[RSP]` / `[RBP+0]` faults #SS, preserving the first retirement and issuing no second callback. Partial windows and forbidden SIB fields reject without access. This is borrowed-fixture evidence; owner/MMU integration, physical x86, Mini and performance remain open. See the [S3-20an report](../reports/STAGE_3_S3-20an_2026-10-27.md).

### S3-20ao RSP/RBP data through address32 no-index SIB

Run `cargo test -p od-jit --lib address32_rsp_sib_base_has_exact_qword_profile_for_all_data_registers` and `cargo test -p od-host --lib address32_rsp_sib_base_data_matrix_and_dispatch_continuation`. Admit only exact singleton `67 48 8B/89 /r 24 [disp8|disp32]`: ModRM `mod=00/01/10,r/m=100`, SIB `0x24`, old guest RSP base, no index, scale one, data RAX..RBP. Address size is Dword, transfer size Qword, and lengths are 5/6/9 bytes. Decoder `MemoryOperand` and `access_address_48` supply all effective-address semantics; no other SIB/prefix/pair/V3 shape is added.

The host matrix compares direct V2, shared IR and interpreter for both operations, all six data registers, and signed displacement boundaries. It checks full CPU/RAM state, exact callback address/count, RSP upper-half truncation for EA, and full-width qword payload. A qword read at `0xffff_fffc` crosses 4 GiB in one contiguous callback. Exact load/store backend failures preserve guest state and RAM. Loading a noncanonical full RSP value commits; a following 64-bit `[RSP]` access raises #SS while retaining the first retirement and avoiding a second callback. Dispatcher coverage resumes from the singleton's actual RIP through the explicit NOP and existing pair without repeating the first access. Budget-zero and pre-entry STOP prevent access; store callback STOP retains the full old RSP payload; byte windows 1..8 expose a boundary without publication or memory effects. Other SIB forms remain rejected. This is borrowed-fixture evidence; production owner/MMU integration, physical x86, Mini and performance remain open. See the [S3-20ao report](../reports/STAGE_3_S3-20ao_2026-10-27.md).

### S3-20ap RSP/RBP data through address32 direct EBP base

Run `cargo test -p od-jit --lib address32_rbp_direct_base_has_exact_qword_profile --offline` and `cargo test -p od-host address32_direct_rsp_rbp_data_keeps_qword_values_and_wraps_contiguously --offline`. Admit only exact singleton `67 48 8B/89 /r disp8|disp32` with ModRM `mod=01/10,r/m=101`, data RAX..RBP, Dword EA, Qword data and lengths 5/8. The decoded `MemoryOperand` supplies displacement and effective-address semantics; `mod=00/rm=101` remains excluded and no SIB/prefix/pair/V3 form is added.

The host differential matrix compares direct V2, shared IR and interpreter for both operations, all six data registers, and signed disp8/disp32 edges. It checks old RBP low-half addressing, high-half truncation, modulo-2^32 wrap, qword crossing at `0xffff_fffc`, complete state/RAM parity and exact one-callback retirement. RBP destination/base alias loads use old RBP for the address before committing the full qword; alias stores use old RBP as the complete payload. Backend faults preserve state/RAM. Dispatcher continuation first commits a noncanonical RBP load through EBP, then raises #SS on the subsequent 64-bit `[RBP+0]` access without a second callback. Budget-zero and entry STOP prevent access; successful EBP-base store callback STOP preserves the write; partial windows and `mod=00/rm=101` have no effects. See the [S3-20ap report](../reports/STAGE_3_S3-20ap_2026-10-27.md).

### S3-20aq RSP-base indexed SIB with RSP/RBP data

Run `cargo test -p od-jit --lib ir_rsp_rbp_data_with_indexed_sib_keeps_exact_qword_profile --offline`, `cargo test -p od-host --lib v2_sib_mov_all_register_scale_combinations_match_shared_ir_and_interpreter --offline`, and `cargo test -p od-host --lib rsp_base_indexed_sib_displacements_match_all_execution_paths --offline`. Admit only exact singleton `48 8B/89 /r sib [disp8|disp32]`: ModRM `mod=00/01/10,r/m=100`, SIB base RSP, index RAX..RBX, scale 1/2/4/8, data RAX..RBP, lengths 4/5/8. No-index `SIB=0x24` remains separately admitted; 67, other base/index, pair and V3 forms remain rejected.

The descriptor and host matrices cover all six data registers, all scales, signed disp8/disp32 edges, and direct V2/shared-IR/interpreter state, RAM, callback and retirement parity. They check data=RSP/index pre-state aliasing, exact callback target and full qword values. Noncanonical RSP at the first or last qword byte yields #SS before callback; backend errors remain host errors with no commit. A successful RSP-data store commits before callback STOP. Dispatcher STOP before entry performs no access; a budget split resumes at the actual RIP after the SIB load and continues through NOP and the existing pair without repeating that read. Partial windows and an unsupported SIB index stop before effects. Existing no-index RSP SIB remains covered. See the [S3-20aq report](../reports/STAGE_3_S3-20aq_2026-10-27.md).

### S3-20ar RBP-base indexed SIB with RSP/RBP data

Run `cargo test -p od-jit --lib ir_rsp_rbp_data_with_indexed_sib_keeps_exact_qword_profile --offline` and `cargo test -p od-host --lib rsp_base_indexed_sib_displacements_match_all_execution_paths --offline`. Admit only singleton `48 8B/89 /r sib disp8|disp32`, ModRM `mod=01/10,r/m=100`, SIB base RBP, index RAX..RBX, scale 1/2/4/8, data RAX..RBP and Qword/64-bit EA; exact lengths are 5/8. `mod=00,SIB.base=101` is no-base and remains unsupported. Indexed RSP-base and no-index `SIB=0x24` profiles stay distinct.

The shared descriptor and host matrices compare direct V2, shared IR and interpreter state/RAM/callback/retirement for both operations, all data/index registers, scales and signed disp8/disp32 boundaries. They cover data=RBP/index pre-state aliases, 64-bit address wrap, #SS from noncanonical RBP across data/index encodings, #GP through RBX, qword AddressOverflow and backend host errors without commit. A store through `[RBP+RCX]` commits old RBP before callback STOP. Dispatcher runs the disp32 singleton and resumes the disp8 store from actual RIP through NOP and the existing pair after both budget and callback-STOP exits, without repeating the store. The bounded-window test rejects partial disp32 and complete mod00 no-base sources with zero publication/effects. The exact profile's cfg-gated x19 canary passed on this Darwin ARM64 host; flags and all guest state are compared in the matrix. See the [S3-20ar report](../reports/STAGE_3_S3-20ar_2026-10-27.md).

### S3-20as address32 indexed SIB with RSP base

Run `cargo test -p od-jit --lib address32_indexed_sib_rsp_rbp_data_has_exact_qword_profile --offline`, `cargo test -p od-host --lib v2_address32_sib_mov_matches_direct_ir_and_interpreter --offline`, and `cargo test -p od-host --lib bounded_dispatch_runs_address32_sib_disp32_nop_and_pair_continuously --offline`. Admit only singleton `67 48 8B/89 /r sib [disp8|disp32]`, ModRM `mod=00/01/10,r/m=100`, SIB base RSP, index RAX..RBX, scale 1/2/4/8, data RAX..RBP, lengths 5/6/9. Address calculation is Dword; data transfer remains Qword. The exact no-index SIB `0x24` stays separately admitted; no-base, RSP/RBP index, extra prefixes, pair and V3 shapes remain rejected.

The JIT descriptor matrix covers RSP/RBP data, both operations, every scale, and signed disp8/disp32 boundaries. The host direct V2/shared IR/interpreter matrix covers all six data registers and compares exact callback address/count, complete CPU/RAM state, alias pre-state behavior, and an independent modulo-2^32 target formula. A noncanonical high RSP is truncated for EA; a qword starting at `0xffff_fffc` is one contiguous callback crossing 4 GiB. Architectural/backend failures preserve the required guest/RAM/budget/retirement state, while successful RSP-data store+STOP commits one complete qword before returning. Dispatcher continuation runs the RSP-base disp32 singleton, NOP bridge and existing pair with budget/stop-RIP continuation and no repeated access; partial byte windows 1..8 and forbidden SIB fields have no effects. No-index SIB `0x24` regression remains covered. This is borrowed-fixture evidence; production owner/MMU integration, physical x86, Mini and performance remain open. See the [S3-20as report](../reports/STAGE_3_S3-20as_2026-10-27.md).

### S3-20at RBP-base indexed SIB with address32 EA

Run `cargo test -p od-jit --lib address32_indexed_sib_rbp_base_disp_has_exact_qword_profile --offline`, `cargo test -p od-host --lib v2_address32_sib_mov_matches_direct_ir_and_interpreter --offline`, and `cargo test -p od-host --lib bounded_dispatch_runs_address32_sib_disp32_nop_and_pair_continuously --offline`. Admit only singleton `67 48 8B/89 /r sib disp8|disp32`, ModRM `mod=01/10,r/m=100`, SIB base RBP, index RAX..RBX, scale 1/2/4/8, and data RAX..RBP. Address calculation is Dword and data transfer Qword; exact instruction lengths are 6/9 bytes. Decoder-owned `MemoryOperand`/`access_address_48` supplies signed displacement and effective-address semantics. The `mod=00,SIB.base=101` no-base form, no-index forms, extra prefixes, pairs, and V3 remain rejected.

The JIT descriptor matrix covers both operations, all six data registers, all low index registers and scales, and signed disp8/disp32 edges. Host direct V2/shared IR/interpreter checks complete CPU/RAM state, exact callback address/count and retirement, including data=RBP/base and data=index aliases from pre-state. A high noncanonical RBP is truncated for the Dword EA, and a qword at `0xffff_fffc` is one contiguous callback across 4 GiB. Fault and backend cases preserve no-commit behavior and error classification; successful RBP-data store+STOP retains the full old payload. Sources differing only in the ninth byte produce and execute distinct expected addresses. Dispatcher runs RBP-base disp32 → NOP → existing pair, compares budgets 0..4, resumes from actual RIP without repeating the first access, and rejects partial windows 1..8 and the complete no-base neighbor with zero effects. See the [S3-20at report](../reports/STAGE_3_S3-20at_2026-10-27.md).

### S3-20au absolute qword MOV through no-base/no-index SIB

Run `cargo test -p od-jit --lib absolute_sib_disp32_has_exact_qword_profile_for_all_data_registers --offline` and `cargo test -p od-host --lib v2_absolute_sib_disp32_matches_ir_interpreter_and_dispatch --offline`. Admit only singleton `48 8B/89 /r 25 disp32`, ModRM `mod=00/rm=100`, exact SIB `0x25`, data RAX..RBP, length 8. The disp32 is sign-extended in 64-bit address mode; the existing decoder `MemoryOperand`/`access_address_48` owns EA semantics. Other SIB, `67`, moffs, pairs, and V3 remain unsupported.

The descriptor and host matrix covers both operations and all six data registers, direct V2/shared IR/interpreter CPU/RAM/callback/retirement parity, and an independent absolute-address formula despite changed GPR/RIP. A qword at `-8` is one contiguous callback through `u64::MAX`; `-1` is rejected as AddressOverflow before callback. Architectural and backend failures do not commit, and successful store+STOP commits once. Two immutable decoder-built artifacts that differ in source byte eight execute distinct expected targets; the runtime matcher validates the fixed opcode/ModRM/SIB profile and consumes displacement metadata retained by decoding rather than re-decoding raw displacement bytes. The dispatcher checks budgets and actual-RIP continuation; partial windows 1..7 return `BoundaryReached` without effects, while a complete address32 neighbor is Unsupported. See the [S3-20au report](../reports/STAGE_3_S3-20au_2026-10-27.md).

### S3-20av address32 absolute qword MOV through no-base/no-index SIB

Run `cargo test -p od-jit --lib address32_absolute_sib_disp32_has_exact_qword_profile_for_all_data_registers --offline` and `cargo test -p od-host --lib v2_address32_absolute_sib_disp32_matches_ir_interpreter_and_dispatch --offline`. Admit only singleton `67 48 8B/89 /r 25 disp32`: ModRM `mod=00/rm=100`, exact SIB `0x25`, data RAX..RBP, Dword EA/Qword data, length 9. The decoder's `MemoryOperand` and `access_address_48` own address calculation. Runtime matching validates opcode/ModRM/SIB and decoded metadata from immutable artifact provenance; it does not re-decode raw displacement.

Both operations and all six data registers are compared across direct V2, shared IR, and interpreter for full state/RAM, callback address/count, and retirement. Signed disp32 targets use the low 32 bits independently of GPR/RIP; `-1` targets `0xffff_ffff`, and a qword at `0xffff_fffc` uses one callback crossing 4 GiB. Fault/backend errors do not commit, store+STOP commits once, and two immutable artifacts differing only in byte nine execute targets `0` and `0x0100_0000`. Dispatcher budgets 0..4 and split continuation resume from actual RIP without replay. Partial windows 1..8 return `BoundaryReached` without effects; indexed no-base returns Unsupported. The 64-bit absolute 20au profile remains covered. See the [S3-20av report](../reports/STAGE_3_S3-20av_2026-10-27.md).

### S3-20aw indexed no-base SIB disp32 qword MOV

Run `cargo test -p od-jit --lib indexed_no_base_sib_disp32_has_exact_qword_profile_for_all_registers --offline` and `cargo test -p od-host --lib v2_address32_absolute_sib_disp32_matches_ir_interpreter_and_dispatch --offline`. The host test name is shared with the preceding 20av report and now includes both absolute-SIB profiles. Admit only `48 8B/89 /r sib disp32`, ModRM `mod=00/rm=100`, SIB base field `101`, index RAX..RBX, scale 1/2/4/8, data RAX..RBP, exact length 8. The decoded operand/access helper supplies full64 `index*scale + signed disp32` modulo arithmetic and canonical/range checks; no-base/no-index SIB `0x25` remains separate.

Descriptor and host parity span both directions, all data and index registers, scales, and signed disp32 edges. Host checks direct V2/shared IR/interpreter CPU/RAM/callback/retirement, source/data index aliasing, independent target formulas, u64 wrap, the eighth-byte displacement effect, #GP with RSP/RBP data and zero callback, AddressOverflow, architectural/backend no-commit, and store+STOP. Dispatcher executes the singleton before NOP and the existing pair, matches budgets 0..4, resumes from actual RIP without replay, and rejects partial windows 1..7 and invalid neighbors with zero blocks/effects. The exact `SIB=0x25` 20au profile is explicitly retained. See the [S3-20aw report](../reports/STAGE_3_S3-20aw_2026-10-27.md).

### S3-20ax address32 indexed no-base SIB disp32 qword MOV

Run `cargo test -p od-jit --lib address32_indexed_no_base_sib_disp32_has_exact_qword_profile_for_all_registers --offline` and `cargo test -p od-host --lib v2_address32_absolute_sib_disp32_matches_ir_interpreter_and_dispatch --offline`. Admit only `67 48 8B/89 /r sib disp32`: ModRM `mod=00/rm=100`, SIB base field `101` (no base), index RAX..RBX, scale 1/2/4/8, data RAX..RBP; exact length 9, Dword EA and Qword transfer. The decoded operand/helper computes `(low32(index) * scale + signed disp32) mod 2^32`, zero-extends once, then performs one contiguous qword access. No-index SIB `0x25` and 64-bit indexed no-base S3-20aw remain separate.

Descriptor/host matrices compare both directions and all data/index/scale combinations at signed disp32 boundaries. Host parity covers direct V2/shared IR/interpreter CPU/RAM/callback/retirement, independent low32 targets with high index halves, data=index pre-state, ninth-byte identity, qword access crossing 4 GiB, fault/backend atomicity, and successful store+STOP. Dispatcher runs the 9-byte singleton → NOP fallback → existing pair; budgets 0..4, split continuation, and partial windows 1..8 verify no duplicate effects. The complete unsupported RBP-index SIB neighbor returns Unsupported without publication. See the [S3-20ax report](../reports/STAGE_3_S3-20ax_2026-10-27.md).

### S3-20ay qword SIB with RBP index

Run `cargo test -p od-jit --lib indexed_rbp_sib_without_displacement_is_exact_for_all_data_and_base_registers --offline` and `cargo test -p od-host --lib v2_address32_absolute_sib_disp32_matches_ir_interpreter_and_dispatch --offline`. Admit only `48 8B/89 /r sib`, ModRM `mod=00/rm=100`, base RAX..RBX, index RBP, scale 1/2/4/8, data RAX..RBP, exact length 4. This is a scoped exact exception; generic index RAX..RBX stays unchanged, and SIB index field `100` remains no-index when `REX.X=0`.

The no-displacement descriptor matrix covers both operations, six data registers, four bases, and four scales; it rejects partial windows 1..3 and unsupported RSP/no-base bases with RBP index, RSI/RDI index, and prefix neighbors, and asserts the old no-index SIB `0x24` metadata. Displacement forms are covered separately by S3-20az below. Host tests compare direct V2/shared IR/interpreter state, RAM, callbacks, and retirement against `base + RBP*scale`, including high bits and `data=RBP`/`data=base` pre-state aliases. Fault cases verify #GP start/end, AddressOverflow, backend classification and callback identity; store+STOP remains committed. Dispatcher resumes after budget=1 at actual RIP and completes the pair without repeating the first access. See the [S3-20ay report](../reports/STAGE_3_S3-20ay_2026-10-27.md).

### S3-20az qword SIB with RBP index and displacement

Run `cargo test -p od-jit --lib indexed_rbp_sib_displacements_are_exact_for_all_data_bases_and_scales --offline` and `cargo test -p od-host --lib rsp_base_indexed_sib_displacements_match_all_execution_paths --offline`. Admit only singleton `48 8B/89 /r sib disp8|disp32`: ModRM `mod=01/10,r/m=100`, base RAX..RBX, index RBP, scale 1/2/4/8, data RAX..RBP, lengths 5/8. The generic index whitelist remains unchanged, and the no-displacement 20ay profile stays separately tested.

The JIT matrix checks both operations, all data/base/scale combinations, and signed displacement edges; partial windows and invalid RSP/RBP/no-base base, RSI/RDI index, prefix, and form variants are rejected. Host direct V2/shared IR/interpreter comparisons check full CPU/RAM state, callback address/count, retirement, independent full-width `base + RBP*scale + displacement`, high RBP bits and `data=RBP`/`data=base` aliases. Exact-profile cases cover #GP at noncanonical start/end before callback, qword endpoint AddressOverflow, backend failure classification with no commit, final-disp32-byte target identity, and store+STOP commit. Dispatcher splits after the disp32 singleton, resumes from actual RIP through NOP and the existing pair without repeating the first access, and rejects byte windows 1..7 without effects. See the [S3-20az report](../reports/STAGE_3_S3-20az_2026-10-27.md).

### S3-20ba address32 SIB with RBP index

Run `cargo test -p od-jit --lib address32_sib_rbp_index_has_exact_qword_profile --offline`, `cargo test -p od-host --lib v2_address32_sib_mov_matches_direct_ir_and_interpreter --offline`, and `cargo test -p od-host --lib address32_rsp_sib_base_data_matrix_and_dispatch_continuation --offline`. Admit only singleton `67 48 8B/89 /r sib [disp8|disp32]`, ModRM `mod=00/01/10,r/m=100`, base RAX..RBX, index RBP, scale 1/2/4/8, data RAX..RBP; lengths 5/6/9 with Dword EA and Qword data. No stack/no-base base, no other index, extra prefix, pair or V3 shape is added; prior 20ay/20az profiles remain separately admitted.

Descriptor and host tests cover both directions, all data/base/scale combinations, and signed displacement boundaries. Host parity compares direct V2/shared IR/interpreter complete state and RAM, exact callback address/count, and retirement. Targets are checked against low32 base + low32 RBP × scale + signed displacement modulo 2^32, including ignored high halves, wrap, and one contiguous qword access across 4 GiB. Two immutable disp32 source views differing only at byte nine hit distinct expected callback addresses. Architectural and backend callback errors leave state/RAM uncommitted with exact error class and callback identity; RBP-data store+STOP commits the old full qword once. Dispatcher continuation resumes at actual RIP after budget=1 through NOP and the existing two-instruction pair without replay. Partial windows 1..8 return fixture `BoundaryReached` with zero retirement/access; exact no-base, stack-base/index, and invalid disp/shape neighbors are rejected. See the [S3-20ba report](../reports/STAGE_3_S3-20ba_2026-10-27.md).


### S3-20bb address64 SIB with RSP base and RBP index

Run `cargo test -p od-jit --lib rsp_base_rbp_index_sib_has_exact_qword_profile --offline` and `cargo test -p od-host --lib rsp_base_indexed_sib_displacements_match_all_execution_paths --offline`. Admit only singleton `48 8B/89 /r sib [disp8|disp32]`, ModRM `mod=00/01/10,r/m=100`, SIB base RSP/index RBP, scale 1/2/4/8, data RAX..RBP, lengths 4/5/8. Keep the generic index whitelist RAX..RBX; this exact branch preserves separately supported RSP-base low-index and low-base RBP-index forms.

Descriptor and host matrices compare both operations and all data registers across scales and signed displacement widths. Host parity checks direct V2/shared IR/interpreter CPU/RAM/callback/retirement and the independent wrapping u64 target formula, including data=RSP/RBP pre-state aliases. Fault cases check RSP-base #SS at noncanonical qword start/end with no callback, low-GPR-base #GP, AddressOverflow separately, and backend failures with exact callback identity/no commit. Store+STOP commits once. The dispatcher test splits after this singleton, resumes at actual RIP through NOP and the existing pair without replay, and checks partial windows 1..7 for `BoundaryReached` with no effects. See the [S3-20bb report](../reports/STAGE_3_S3-20bb_2026-10-27.md).

### S3-20bc address64 SIB with RBP base and RBP index

Run `cargo test -p od-jit --lib rbp_base_rbp_index_sib_displacements_have_exact_qword_profile --offline` and `cargo test -p od-host --lib rsp_base_indexed_sib_displacements_match_all_execution_paths --offline`. Admit only singleton `48 8B/89 /r sib disp8|disp32`, ModRM `mod=01/10,r/m=100`, SIB base=index=RBP, scale 1/2/4/8, data RAX..RBP, lengths 5/8. `mod=00` with SIB base `101` is no-base and remains rejected; no `67`, other base/index combinations, pairs, or V3 are added. The decoded `MemoryOperand::access_address_48` owns effective-address semantics and signed displacement.

Descriptor and host matrices cover both directions, all six data registers, scales, and disp8/disp32 signed edges. Direct V2/shared IR/interpreter parity checks full CPU/RAM/callback/retirement state against `RBP + RBP*scale + signed displacement`, including pre-state alias `data=base=index=RBP`. Noncanonical qword start/end produce #SS with no callback; low-base control produces #GP; endpoint overflow is AddressOverflow; backend failures preserve host classification and exact attempted callback without commit. Store+STOP commits once. Bounded dispatch splits after the singleton and resumes at actual RIP through NOP and the frozen pair without replay. Dispatcher byte windows 1..4 and 1..7, plus unsupported neighbors, stop before publication or memory effects. See the [S3-20bc report](../reports/STAGE_3_S3-20bc_2026-10-27.md).

### S3-20bd address32 SIB with RSP base and RBP index

Run `cargo test -p od-jit --lib address32_sib_rsp_base_rbp_index_has_exact_qword_profile --offline` and `cargo test -p od-host --lib address32_rsp_sib_base_data_matrix_and_dispatch_continuation --offline`. Admit only singleton `67 48 8B/89 /r sib [disp8|disp32]`, ModRM `mod=00/01/10,r/m=100`, SIB base RSP/index RBP, scale 1/2/4/8, data RAX..RBP, Dword EA/Qword data, lengths 5/6/9. This is a scoped exact exception; the generic index whitelist remains unchanged, and existing SIB no-index/RSP-base low-index/low-base RBP-index forms remain separately covered.

The JIT descriptor matrix and host direct V2/shared IR/interpreter matrix cover both memory directions, all six data registers, four scales, and no-disp/disp8/disp32 signed boundaries. The expected target is independently computed from low32 RSP, low32 RBP, scale, and signed displacement modulo 2^32; tests check aliases, high-half truncation, full state/RAM/callback/retirement parity, and a contiguous qword crossing 4 GiB. Two immutable source artifacts differing only in the ninth byte execute distinct expected addresses. Backend errors preserve state with the exact attempted callback; successful store+STOP commits once. Bounded dispatch splits after the singleton and resumes at actual RIP through NOP and the existing pair without replay. Partial byte windows 1..8 and unsupported neighbors stop before effects. See the [S3-20bd report](../reports/STAGE_3_S3-20bd_2026-10-27.md).

### S3-20be address32 SIB with RBP base and RBP index

Run `cargo test -p od-jit --lib address32_sib_rbp_base_rbp_index_has_exact_qword_profile --offline` and `cargo test -p od-host --lib address32_rsp_sib_base_data_matrix_and_dispatch_continuation --offline`. Admit only singleton `67 48 8B/89 /r sib disp8|disp32`, ModRM `mod=01/10,r/m=100`, SIB base=index=RBP, scale 1/2/4/8, data RAX..RBP, Dword EA/Qword transfer, lengths 6/9. ModRM `mod=00` with SIB base `101` remains no-base and rejected; the generic index whitelist stays unchanged, with no other bases/indices, prefixes, pairs, or V3 admitted.

Descriptor and host matrices cover load/store, six data registers, four scales, and signed disp8/disp32 bounds. Direct V2/shared IR/interpreter parity checks complete state, RAM, callback address/count and retirement against `(low32(RBP) + low32(RBP)*scale + signed displacement) mod 2^32`. This includes `data=base=index=RBP` using pre-state and preserving full-qword data, a single contiguous callback across 4 GiB, and high RBP halves ignored by Dword EA. Guest callback faults preserve the exact payload without commit; backend errors remain host errors without commit. The exact-profile store+STOP commits once. Dispatcher budget=1 resumes from actual RIP through NOP and the existing pair without replay; entry guards, partial windows 1..5/1..8, and a complete no-base neighbor stop before effects. Two immutable disp32 artifacts differing only in source byte nine produce targets `0x500` and `0x0100_0500`. See the [S3-20be report](../reports/STAGE_3_S3-20be_2026-10-27.md).

### S3-20bf address64 indexed no-base SIB with RBP index

Run `cargo test -p od-jit --lib indexed_no_base_sib_disp32_has_exact_qword_profile_for_all_registers --offline` and `cargo test -p od-host --lib rsp_base_indexed_sib_displacements_match_all_execution_paths --offline`. Admit only singleton `48 8B/89 /r sib disp32`: ModRM `mod=00/rm=100`, SIB base field `101` (no base), index RBP, scale 1/2/4/8, data RAX..RBP, exact length 8, Qword EA/data. The decoded operand computes `RBP*scale + signed disp32` modulo u64; it has no base or RIP contribution. Descriptor/host matrices compare load/store across all data registers, scales and signed disp32 edges; direct V2/shared IR/interpreter parity checks full CPU/RAM, callback address/count and retirement, including `data=RBP` pre-state aliasing. Noncanonical range faults are #GP with no callback; AddressOverflow and backend failures remain distinct; successful store+STOP commits once. Byte-8 displacement changes the target. Dispatcher executes the valid singleton and partial windows 1..7 stop before effects. The address32 sibling is a separately bounded S3-20bg profile. See the [S3-20bf report](../reports/STAGE_3_S3-20bf_2026-10-27.md).

### S3-20bg address32 indexed no-base SIB with RBP index

Run `cargo test -p od-jit --lib address32_indexed_no_base_sib_disp32_has_exact_qword_profile_for_all_registers --offline` and `cargo test -p od-host --lib v2_address32_absolute_sib_disp32_matches_ir_interpreter_and_dispatch --offline`. Exact form: singleton `67 48 8B/89 /r sib disp32`, ModRM `mod=00/rm=100`, SIB base field `101` (no base), index RBP, scale 1/2/4/8, data RAX..RBP, Dword EA/Qword transfer, length 9. The decoded operand supplies `(low32(RBP)*scale + signed disp32) mod 2^32`, then zero-extension; no base or RIP contributes. The JIT descriptor matrix and host direct V2/shared IR/interpreter matrix cover both directions, all six data registers, scales and signed disp32 edges, including `data=RBP` pre-state alias and full qword payload. They verify high-half truncation, independent callback targets, one contiguous qword callback across 4 GiB, byte-nine identity, architectural/backend no-commit, store+STOP, actual-RIP budget continuation without replay, and partial windows 1..8 with no effects. Unsupported RSI-index and non-exact no-index neighbors remain rejected. See [S3-20bg report](../reports/STAGE_3_S3-20bg_2026-10-28.md).

### S3-20bh address64 direct-base qword MOV with RSI/RDI base

Run `cargo test -p od-jit --lib rsi_rdi_are_only_admitted_as_exact_qword_direct_bases --offline` and `cargo test -p od-host --lib v2_rsi_rdi_direct_base_mov_matches_direct_ir_and_interpreter --offline`. The S3-20bh form is singleton `48 8B/89 /r`, ModRM `mod=00`, base `rm=110` (RSI) or `rm=111` (RDI), data RAX..RBP, length 3; RSI/RDI are not admitted as data or index. Direct V2/shared IR/interpreter covers load/store × two bases × six data registers and asserts state/RAM parity plus one qword callback at the old full RSI/RDI value. Fault checks include noncanonical start and qword end #GP without callback, distinct AddressOverflow, backend error without commit, and RSP/RBP as data. Store+STOP commits once; entry STOP, stop-RIP, budget zero and partial windows 1..2 avoid memory access. Bounded dispatch with budget 1 resumes at the second singleton’s actual RIP and does not repeat the first read. S3-20bh remains length 3; disp8/disp32 variants are admitted only by S3-20bi. The fixture rejects SIB, address32 and data-RSI/RDI neighbor encodings. See [S3-20bh report](../reports/STAGE_3_S3-20bh_2026-10-28.md).

### S3-20bi address64 direct-base qword MOV with RSI/RDI and displacement

Run `cargo test -p od-jit --lib rsi_rdi_are_only_admitted_as_exact_qword_direct_bases --offline` and `cargo test -p od-host --lib v2_rsi_rdi_direct_base_mov_matches_direct_ir_and_interpreter --offline`. Admit only `48 8B/89 /r disp8|disp32`, ModRM `mod=01/10`, `rm=110/111`, data RAX..RBP, lengths 4/7. The descriptor matrix and host triple path cover both directions, both bases, all six data registers and signed disp8/disp32 edges. Each target is checked independently as old full RSI/RDI plus signed displacement; disp32's last byte is proven to change the native callback target. The host checks CPU/RAM/callback/retirement parity, base preservation, full qword stores, #GP at noncanonical start/end before callback, distinct AddressOverflow, architectural/backend no-commit, successful store+STOP once, entry guards, and budget continuation from actual RIP without replay. Partial windows 1..3/1..6 plus SIB, address32, extra-prefix, and RSI/RDI-as-data neighbors are rejected before effects. See [S3-20bi report](../reports/STAGE_3_S3-20bi_2026-10-28.md).

### S3-20bj address32 direct-base qword MOV with RSI/RDI

Run `cargo test -p od-jit --lib address32_rsi_rdi_direct_qword_bases_have_exact_displacement_profiles --offline` and `cargo test -p od-host --lib v2_address32_rsi_rdi_direct_base_mov_matches_direct_ir_and_interpreter --offline`. Admit only `67 48 8B/89 /r [disp8|disp32]`, ModRM `mod=00/01/10`, `rm=110/111`, data RAX..RBP, lengths 4/5/8. EA is `(low32(old RSI/RDI) + signed displacement) mod 2^32`, while data and transfer remain qword; the complete base register is unchanged. Descriptor and host cover load/store, both bases, six data registers and signed displacement edges. Direct V2/shared IR/interpreter parity checks full CPU/RAM state, exact callback/retirement, and target formula independently. The host verifies qword access at `0xffff_fffc` remains one contiguous callback across 4 GiB, backend no-commit, successful store+STOP, entry guards, partial windows, unsupported neighbors, and budget continuation at actual RIP without replay. See [S3-20bj report](../reports/STAGE_3_S3-20bj_2026-10-28.md).

### S3-20bk address64 indexed SIB with RSI/RDI base

Run `cargo test -p od-jit --lib rsi_rdi_indexed_sib_no_displacement_has_exact_qword_profile --offline` and `cargo test -p od-host --lib address32_rsp_sib_base_data_matrix_and_dispatch_continuation --offline`. Admit only four-byte singleton `48 8B/89 /r sib`, ModRM `mod=00/rm=100`, SIB base RSI/RDI, index RAX..RBX, scale 1/2/4/8 and data RAX..RBP. No displacement, 67, no-index/no-base, RBP/RSI/RDI index, extra prefix, pair or V3 form is added. Effective address is decoded by `access_address_48`: old full base plus old full index times scale with u64 wrapping. The JIT descriptor and host direct V2/shared IR/interpreter matrices cover both operations, bases, all four indices/scales and six data registers, including data=index pre-state aliases and base preservation (RSI/RDI bases cannot alias RAX..RBP data). Host checks include noncanonical start/end #GP before callbacks, AddressOverflow, entry STOP and zero budget without access, a successful store+STOP commit, and budget continuation through SIB→NOP fallback→pair from actual RIP without replay. Unsupported neighbors and partial source windows have no effects. See the [S3-20bk report](../reports/STAGE_3_S3-20bk_2026-10-28.md).

### S3-20bl address64 indexed SIB with RSI/RDI base and displacement

Run `cargo test -p od-jit --lib rsi_rdi_indexed_sib_signed_displacements_have_exact_qword_profile --offline` and `cargo test -p od-host --lib address32_rsp_sib_base_data_matrix_and_dispatch_continuation --offline`. Admit only singleton `48 8B/89 /r sib disp8|disp32`, ModRM `mod=01/10,r/m=100`, SIB base RSI/RDI, index RAX..RBX, scale 1/2/4/8, data RAX..RBP, lengths 5/8. `MemoryOperand::access_address_48` computes old full base + old full index × scale + signed displacement with wrapping u64. JIT descriptor and host direct V2/shared IR/interpreter exercise both operations and all registers/scales with signed displacement boundaries; the host checks independent target/callback address and count, full state/RAM parity and retirement, high-index wrapping, and two disp32 artifacts that differ only in the final encoded displacement byte. Exact dispatcher cases cover budget-split disp8 and disp32 singleton continuations through SIB→NOP→existing pair without replay, entry STOP/budget0, partial byte windows without helper access, noncanonical start/end #GP without callback, AddressOverflow, backend no-commit, and successful store+STOP. No address32, other base/index, no-index/no-base, extra prefix, pair profile, or V3 admission is added. See the [S3-20bl report](../reports/STAGE_3_S3-20bl_2026-10-28.md).

### S3-20bm address32 indexed SIB with RSI/RDI base

Run `cargo test -p od-jit --lib address32_indexed_sib_rsi_rdi_base_has_exact_qword_profile --offline` and `cargo test -p od-host --lib address32_rsp_sib_base_data_matrix_and_dispatch_continuation --offline`. Admit only `67 48 8B/89 /r sib [disp8|disp32]`: ModRM `mod=00/01/10,r/m=100`, SIB base RSI/RDI, index RAX..RBX, scale 1/2/4/8, data RAX..RBP, lengths 5/6/9. EA comes from `MemoryOperand::access_address_48`: `(low32(old base)+low32(old index)*scale+signed displacement) mod 2^32`; transfer remains qword and base registers remain full-width. Descriptor and host direct V2/shared IR/interpreter matrices cover both directions, bases, data/index/scale combinations including mod00 per combination, and signed disp8/disp32 values, with independent targets, full CPU/RAM parity, callback counts and retirement. Host coverage includes data=index pre-state, qword crossing 4 GiB, distinct targets from the final disp32 byte, load/store architectural and backend no-commit with RAM snapshots, store+STOP, entry STOP/stop-RIP/zero budget, partial windows 1..8, unsupported neighbors, and budget-split actual-RIP continuation across lengths 5/6/9 followed by NOP and the existing pair without replay. No extra prefix, other base/index, pair or V3 admission is added. See [S3-20bm report](../reports/STAGE_3_S3-20bm_2026-10-28.md).

### S3-20bn address64 indexed SIB with RSI/RDI base and RBP index

Run `cargo test -p od-jit --lib rsi_rdi_base_rbp_index_sib_has_exact_qword_profile --offline` and `cargo test -p od-host --lib address32_rsp_sib_base_data_matrix_and_dispatch_continuation --offline`. Admit only `48 8B/89 /r sib [disp8|disp32]`: ModRM `mod=00/01/10,r/m=100`, SIB base RSI/RDI, index exactly RBP, scale 1/2/4/8, data RAX..RBP, lengths 4/5/8. EA is the decoder's full-width old-base + old-RBP×scale + signed-displacement result, wrapping u64. Descriptor and host matrices cover both operations, base registers, scales, all data registers including `data=RBP` pre-state use, mod00, and signed disp8/disp32 boundaries; host direct V2/shared IR/interpreter checks full state/RAM parity, independent target, callback address/count, and retirement. Additional host cases prove u64 address wrap and distinct target when only disp32 byte 7 changes. Dispatcher checks actual-RIP budget continuation across 4/5/8-byte forms without replay, entry STOP/budget0, successful store+STOP, partial windows, invalid neighbors, noncanonical start/end #GP without callbacks, AddressOverflow, and backend no-commit. No address32, other index/base, prefix, pair or V3 admission is added. See [S3-20bn report](../reports/STAGE_3_S3-20bn_2026-10-28.md).

### S3-20bo address32 indexed SIB with RSI/RDI base and RBP index

Run `cargo test -p od-jit --lib address32_rsi_rdi_base_rbp_index_sib_has_exact_qword_profile --offline` and `cargo test -p od-host --lib address32_rsp_sib_base_data_matrix_and_dispatch_continuation --offline`. Admit only singleton `67 48 8B/89 /r sib [disp8|disp32]`: ModRM `mod=00/01/10,r/m=100`, SIB base RSI/RDI and index exactly RBP, scale 1/2/4/8, data RAX..RBP, lengths 5/6/9. Effective address comes from decoded `MemoryOperand::access_address_48`: `(low32(old base)+low32(old RBP)*scale+signed displacement) mod 2^32`, then zero-extension; qword transfer and data value remain full-width. Direct V2/shared IR/interpreter host parity covers both operations, bases, data registers, scales, mod00 and signed disp8/disp32 edges, checking full CPU/RAM state, callback address/count and retirement. The host proves scale-product wrap with `low32(RBP)=0x80000001`, scale 8 and disp8 -1 targeting `0x1007`, with RBP as load destination (address uses pre-state RBP, commit follows success). Additional checks cover high-half truncation, qword crossing 4 GiB with one callback, final disp32 byte target sensitivity, architectural/backend faults without commit, store+STOP, entry STOP/budget0, partial windows 1..8, and actual-RIP dispatcher continuation through 5/6/9-byte forms without replay. The exact profile does not widen the global register whitelist. See the [S3-20bo report](../reports/STAGE_3_S3-20bo_2026-10-28.md).

### S3-20bp address64 no-displacement SIB with RSI/RDI index

Run `cargo test -p od-jit --lib low_base_rsi_rdi_index_sib_has_exact_qword_profile --offline` and `cargo test -p od-host --lib address32_rsp_sib_base_data_matrix_and_dispatch_continuation --offline`. Admit only singleton `48 8B/89 /r sib`: ModRM `mod=00/rm=100`, SIB base RAX..RBX, index exactly RSI or RDI, scale 1/2/4/8, data RAX..RBP, length 4. EA is decoder-owned through `MemoryOperand::access_address_48`, using old full-width base + old full-width index×scale with wrapping u64. Direct V2/shared IR/interpreter parity covers load/store × four low bases × two index registers × four scales × six data registers, complete CPU/RAM state, independent target, callback count/address and retirement; data=base uses the pre-state value for EA/store payload and commits a load only after success; RSI/RDI index registers remain unchanged. Host checks cover canonical-start/noncanonical-end #GP before access, separate AddressOverflow, architectural/backend faults without commit, successful store+STOP, entry STOP/zero budget, actual-RIP singleton→NOP→pair continuation without replay, and invalid neighbors/partial windows. No address32, displacement, no-index, no-base, extra prefix, other index/base, pair or V3 shape is admitted. See the [S3-20bp report](../reports/STAGE_3_S3-20bp_2026-10-28.md).

### S3-20bq address64 indexed SIB with low base, RSI/RDI index, and displacement

Run `cargo test -p od-jit --lib low_base_rsi_rdi_index_sib_displacements_have_exact_qword_profile --offline` and `cargo test -p od-host --lib address32_rsp_sib_base_data_matrix_and_dispatch_continuation --offline`. Admit only `48 8B/89 /r sib disp8|disp32`: ModRM `mod=01/10,r/m=100`, SIB base RAX..RBX, index RSI/RDI, scale 1/2/4/8, data RAX..RBP, lengths 5/8. Effective addresses come only from decoded `MemoryOperand::access_address_48` and use old full-width base + old full-width index×scale + signed displacement with u64 wrapping. Descriptor and host direct V2/shared IR/interpreter cover load/store × four bases × two indices × four scales × six data registers and signed disp8/disp32 boundaries, checking complete CPU/RAM, exact callback address/count, retirement, pre-state data=base behavior, preserved RSI/RDI, full-width high-index bits and u64 wrap. Host cases prove final disp32 byte target identity, noncanonical start/end #GP before callback, separate AddressOverflow, architectural/backend no-commit, successful store+STOP, entry STOP/stop-RIP/zero-budget, partial windows, and dispatcher continuation from actual RIP across disp8 and disp32 singleton→NOP→pair without replay. Address32, stack/no-base, extra prefixes, other base/index, pair and V3 shapes remain unsupported. See the [S3-20bq report](../reports/STAGE_3_S3-20bq_2026-10-28.md).

### S3-20br address32 indexed SIB with low base and RSI/RDI index

Run `cargo test -p od-jit --lib address32_low_base_rsi_rdi_index_sib_has_exact_qword_profile --offline` and `cargo test -p od-host --lib address32_low_base_rsi_rdi_index_sib_matches_all_execution_paths --offline`. Admit only `67 48 8B/89 /r sib [disp8|disp32]`: ModRM `mod=00/01/10,r/m=100`, SIB base RAX..RBX, index RSI/RDI, scale 1/2/4/8, data RAX..RBP, lengths 5/6/9. Address is decoder-owned: `(low32(old base)+low32(old index)*scale+signed displacement) mod 2^32`, then zero-extension; qword transfer/source data remain full-width. The JIT descriptor and host direct V2/shared IR/interpreter cover both operations and all bases, indices, scales and data registers over no-displacement, signed disp8 and disp32 forms. Host evidence checks complete CPU/RAM parity, exact callback address/length/count, retirement, data=base pre-state behavior, preserved full RSI/RDI, high-half truncation and modulo-32 wrap. Additional cases prove contiguous qword crossing 4 GiB, ninth-byte disp32 target identity, architectural/unmapped/backend no-commit for loads and stores, store+STOP, entry STOP/stop-RIP/zero-budget, partial windows 1..8, invalid neighbors, and actual-RIP budget continuation over 5/6/9-byte forms without replay. The generic register whitelist is unchanged. See the [S3-20br report](../reports/STAGE_3_S3-20br_2026-10-28.md).

### S3-20bs address64 indexed SIB with RSP base and RSI/RDI index

Run `cargo test -p od-jit --lib rsp_base_rsi_rdi_index_sib_has_exact_qword_profile --offline`, `cargo test -p od-host --lib rsp_base_indexed_sib_displacements_match_all_execution_paths --offline`, and `cargo test -p od-host --lib bounded_dispatch_continues_rsp_base_rsi_rdi_index_forms_by_actual_rip --offline`. Admit only `48 8B/89 /r sib [disp8|disp32]`: ModRM `mod=00/01/10,r/m=100`, SIB base RSP, index RSI/RDI, scale 1/2/4/8, data RAX..RBP, lengths 4/5/8. The decoder's `MemoryOperand::access_address_48` is the sole EA implementation: old full RSP + old full RSI/RDI×scale + signed displacement with u64 wrapping. The JIT descriptor and host direct V2/shared IR/interpreter matrix cover loads and stores across six data registers, both indices, all scales and no-displacement/disp8/disp32 signed boundaries. Evidence checks complete CPU/RAM parity, independent target formulas, exact callback address/length/count, retirement, data=RSP pre-state behavior and preserved RSI/RDI. Exact profile cases cover RSP-base #SS on noncanonical start/end before callback, low-base #GP, separate AddressOverflow, architectural/backend no-commit for both directions, successful store+STOP, dispatcher entry stop-RIP/STOP/zero-budget, final disp32 byte target identity and continuation across 4/5/8-byte forms without replay. Partial windows and invalid neighbors have no effects. See the [S3-20bs report](../reports/STAGE_3_S3-20bs_2026-10-28.md). No Stage 2/ISA, owner/MMU, Mini or performance work is included.

### S3-20bt address64 indexed SIB with RBP base, RSI/RDI index, and displacement

Run `cargo test -p od-jit --lib rbp_base_rsi_rdi_index_sib_displacements_have_exact_qword_profile --offline`, `cargo test -p od-host --lib rsp_base_indexed_sib_displacements_match_all_execution_paths --offline`, and `cargo test -p od-host --lib bounded_dispatch_continues_rsp_base_rsi_rdi_index_forms_by_actual_rip --offline`. Admit only singleton `48 8B/89 /r sib disp8|disp32`: ModRM `mod=01/10,r/m=100`, SIB base RBP, index RSI/RDI, scale 1/2/4/8, data RAX..RBP, lengths 5/8. ModRM `mod=00` with SIB base field 5 is no-base and remains rejected. Effective addresses come solely from the decoder's `MemoryOperand::access_address_48`: old full RBP + old full RSI/RDI×scale + signed displacement with u64 wrapping. Descriptor and host direct V2/shared IR/interpreter tests cover loads/stores × six data registers × two indices × four scales × signed displacement edges, checking full CPU/RAM parity, independent targets, exact callback address/count and retirement. Host checks also cover noncanonical RBP start/end #SS without callbacks, low-base #GP, separate AddressOverflow, architectural/backend no-commit, committed store+STOP, entry stop-RIP/STOP/budget0, partial windows 1..7 and budget-split dispatcher continuation from actual RIP across 5/8-byte forms without replay. No extra prefix, no-base, other index/base, pair or V3 form is admitted. See [S3-20bt report](../reports/STAGE_3_S3-20bt_2026-10-28.md).

### S3-20bu address32 indexed SIB with RSP base and RSI/RDI index

Run `cargo test -p od-jit --lib address32_rsp_base_rsi_rdi_index_sib_has_exact_qword_profile --offline` and `cargo test -p od-host --lib address32_low_base_rsi_rdi_index_sib_matches_all_execution_paths --offline`. Admit only singleton `67 48 8B/89 /r sib [disp8|disp32]`: ModRM `mod=00/01/10,r/m=100`, SIB base RSP, index RSI/RDI, scale 1/2/4/8, data RAX..RBP, exact lengths 5/6/9. The sole EA implementation is decoder-owned `MemoryOperand::access_address_48`: `(low32(old RSP)+low32(old index)*scale+signed displacement) mod 2^32`, then zero-extension for the qword transfer. Data=RSP uses the pre-state value for the address and store payload; a load commits RSP only after a successful read. RSI/RDI remain index-only and unchanged. The descriptor and host direct V2/shared IR/interpreter matrix covers both directions, six data registers, both indices, all scales and no-displacement/disp8/disp32 forms with full CPU/RAM parity, independent target, callback address/length/count and retirement checks. Host cases also verify high-half truncation and modulo-32 wrap, contiguous qword crossing 4 GiB, ninth-byte disp32 target identity, architectural/backend no-commit, store+STOP, entry STOP/stop-RIP/zero-budget, partial windows 1..8, unsupported neighbors and actual-RIP budget continuation across 5/6/9-byte singleton forms without replay. No additional prefix, base/index, pair or V3 form is admitted. See [S3-20bu report](../reports/STAGE_3_S3-20bu_2026-10-28.md). Production owner/MMU, Stage 2/ISA, Mini and performance remain outside this slice.

### S3-20bv address32 indexed SIB with RBP base and RSI/RDI index

Run `cargo test -p od-jit --lib address32_rbp_base_rsi_rdi_index_sib_has_exact_qword_profile --offline` and `cargo test -p od-host --lib address32_rbp_base_rsi_rdi_index_sib_matches_execution_paths_and_dispatch --offline`. Admit only singleton `67 48 8B/89 /r sib disp8|disp32`: ModRM `mod=01/10,r/m=100`, SIB base RBP, index RSI/RDI, scale 1/2/4/8, data RAX..RBP, lengths 6/9. ModRM `mod=00` with SIB base field 5 is no-base and remains outside this profile. Effective address is decoder-owned `MemoryOperand::access_address_48`: `(low32(old RBP)+low32(old index)*scale+signed displacement) mod 2^32`, zero-extended for the qword transfer. Data=RBP uses its pre-state value for EA/store payload; load commits only after success, and RSI/RDI remain unchanged. JIT descriptor and host direct V2/shared IR/interpreter matrices check both operations, all data registers, both indices/scales and signed disp8/disp32 boundaries, full CPU/RAM parity, independent target, callback address/width/count and retirement. Host cases cover upper-half truncation, wrap, contiguous qword crossing 4 GiB, final disp32 byte target identity, architectural/backend no-commit for both directions, store+STOP, entry guards, partial windows 1..5/1..8, unsupported no-base neighbors, and actual-RIP dispatcher continuation from 6 to 9 bytes without replay. See [S3-20bv report](../reports/STAGE_3_S3-20bv_2026-10-28.md). Stage 2/ISA, production owner/MMU, Mini and performance are outside this slice.

### S3-20bw address64 indexed no-base SIB with RSI/RDI index and disp32

Run `cargo test -p od-jit --lib indexed_no_base_rsi_rdi_sib_disp32_has_exact_qword_profile --offline`, `cargo test -p od-host --lib rsp_base_indexed_sib_displacements_match_all_execution_paths --offline`, and `cargo test -p od-host --lib bounded_dispatch_continues_rsp_base_rsi_rdi_index_forms_by_actual_rip --offline`. Admit only singleton `48 8B/89 /r sib disp32`: ModRM `mod=00/rm=100`, SIB base field `101` (no base), index RSI/RDI, scale 1/2/4/8, data RAX..RBP, length 8. The decoder-owned EA is `old_index*scale + signed disp32` modulo u64; no base or RIP contributes. Descriptor and host direct V2/shared IR/interpreter matrices cover both directions, six data registers, both indices, all scales and signed disp32 edges, with full CPU/RAM parity, independent EA, callback address/width/count and retirement. Additional cases verify high index bits and wrapping, canonical-end #GP and end-of-u64 overflow before callbacks, architectural/backend no-commit, committed store+STOP, exact entry guards, partial windows 1..7, final-byte target identity, and dispatcher continuation over actual RIP without replay. No `67`, other prefix/base/index form, pair or V3 is added. The SIB `0x25` absolute form remains separate. See the [S3-20bw report](../reports/STAGE_3_S3-20bw_2026-10-28.md).

### S3-20bx address32 indexed no-base SIB with RSI/RDI index and disp32

Run `cargo test -p od-jit --lib address32_indexed_no_base_rsi_rdi_sib_disp32_has_exact_qword_profile --offline` and `cargo test -p od-host --lib address32_low_base_rsi_rdi_index_sib_matches_all_execution_paths --offline`. Admit only singleton `67 48 8B/89 /r sib disp32`: ModRM `mod=00/rm=100`, SIB base=101 (no base), index RSI/RDI, scale 1/2/4/8, data RAX..RBP, exact length 9. The decoder-owned EA is `(low32(index)*scale + signed disp32) mod 2^32`, zero-extended for qword transfer; no base or RIP contributes. The host matrix checks direct V2/shared IR/interpreter parity for both directions, all data registers/indices/scales and signed disp32 edges, independent EA, full CPU/RAM, callback address/width/count, and retirement. Additional host cases cover high-half truncation/product wrap, contiguous qword crossing 4 GiB, final disp32 byte target identity, architectural/backend no-commit, committed store+STOP payload, entry guards, partial windows 1..8, unsupported neighbors and actual-RIP dispatcher continuation without replay. Other prefixes, index/data registers, pair and V3 profiles remain unchanged. See the [S3-20bx report](../reports/STAGE_3_S3-20bx_2026-10-28.md). Production owner/MMU, Stage 2/ISA, Mini and performance work remain outside this slice.

### S3-20by: address64 indexed SIB with RSI/RDI base and RSI/RDI index

Run `cargo test -p od-jit --lib rsi_rdi_base_rsi_rdi_index_sib_has_exact_qword_profile --offline` and `cargo test -p od-host --lib rsp_base_indexed_sib_displacements_match_all_execution_paths --offline`. Admit only singleton `48 8B/89 /r sib`: ModRM `mod=00/01/10,r/m=100`, base and index each RSI/RDI, scale 1/2/4/8, data RAX..RBP, lengths 4/5/8. The decoder-owned EA is `old_base + old_index*scale + signed displacement` modulo u64. When base=index, the same pre-state register contributes to both terms. Descriptor/host evidence covers both operations, all four base/index combinations, scales and data registers; direct V2/shared IR/interpreter parity; signed d8/d32 boundaries; callback address/count and retirement; base=index u64 wrapping; disp32 final-byte target identity; #GP start/end, separate overflow, architectural/backend atomicity and full-payload store+STOP. Dispatcher checks partial windows, entry guards, and actual-RIP continuation through this singleton, NOP and the frozen pair without replay. See the [S3-20by report](../reports/STAGE_3_S3-20by_2026-10-28.md). No address32, no-index/no-base, extra-prefix, pair-profile or V3 admission is added.


### S3-20bz address32 indexed SIB with RSI/RDI base and RSI/RDI index

Run `cargo test -p od-jit --lib address32_rsi_rdi_base_rsi_rdi_index_sib_has_exact_qword_profile --offline` and `cargo test -p od-host --lib address32_low_base_rsi_rdi_index_sib_matches_all_execution_paths --offline`. Admit only singleton `67 48 8B/89 /r sib [disp8|disp32]`, ModRM `mod=00/01/10,r/m=100`, SIB base and index each RSI/RDI, scale 1/2/4/8, data RAX..RBP, lengths 5/6/9. `MemoryOperand::access_address_48` is the sole EA implementation: `(low32(old_base)+low32(old_index)*scale+signed displacement) mod 2^32`, zero-extended for qword transfer; when base=index, both terms use the same old value. Tests compare direct V2/shared IR/interpreter for both directions over all base/index pairs, scales, data registers, forms and signed displacement edges, including full state/RAM, independent targets, callback address/width/count and retirement. Host coverage includes truncation/wrap, contiguous qword crossing 4 GiB, final disp32 byte target identity, arch/backend no-commit, store+STOP, entry guards, partial windows and actual-RIP continuation over 5/6/9-byte singleton→NOP→frozen pair without replay. Invalid neighbors remain unsupported. See [S3-20bz report](../reports/STAGE_3_S3-20bz_2026-10-28.md).

### S3-20ca: address64 low-base SIB with no index

Run `cargo test -p od-jit --lib low_base_no_index_sib_has_exact_qword_profile --offline` and `cargo test -p od-host --lib low_base_no_index_sib_matches_direct_ir_and_interpreter --offline`. Admit only `48 8B/89 /r sib [disp8|disp32]` with ModRM `mod=00/01/10,r/m=100`, SIB exactly `0x20..=0x23`, data RAX..RBP, Qword/64-bit EA and lengths 4/5/8. SIB index field 100 with no extension is the absent-index sentinel, not RSP; scale is one and base is RAX..RBX. The decoder `MemoryOperand::access_address_48` supplies EA semantics. The JIT descriptor checks both opcodes, all four bases and all six data registers across all three lengths with signed displacement edges, exact metadata/source, truncation and rejected neighboring forms. Host direct V2/shared IR/interpreter comparisons check full CPU/RAM, independent base+signed-displacement target, callback address/width/count, and retirement; the same work is compared against equivalent direct-base interpreter encodings. Faulting addresses preserve state and make no callback; backend failures preserve state/RAM with exact callback evidence. Store+STOP remains committed once, entry STOP/budget-zero suppress access, and dispatcher continuation across 4/5/8-byte forms resumes at actual RIP without replay. Final disp32-byte identity and partial windows are checked. See the [S3-20ca report](../reports/STAGE_3_S3-20ca_2026-10-28.md). Production owner/MMU, Stage 2/ISA, Mini and performance are outside this slice.

### S3-20cb: address32 low-base SIB with no index

Run `cargo test -p od-jit --lib address32_low_base_no_index_sib_has_exact_qword_profile --offline` and `cargo test -p od-host --lib low_base_no_index_sib_matches_direct_ir_and_interpreter --offline`. Admit only `67 48 8B/89 /r sib [disp8|disp32]`, ModRM `mod=00/01/10,r/m=100`, SIB `0x20..=0x23`, base RAX..RBX, data RAX..RBP, Dword EA/Qword data and lengths 5/6/9. The index field is the absent-index sentinel and scale is one. EA is decoder-owned: `(low32(old base)+signed displacement) mod 2^32`, zero-extended. The base GPR is preserved unless it is the load destination; data=base loads use the old base for EA and write the full qword only after a successful read. The descriptor matrix covers both directions, every low base/data register, all forms and signed edges, partial windows and exact rejected neighbors. Host direct V2/shared IR/interpreter compare full CPU/RAM, independent address, callback address/width/count and retirement across 480 combinations; equivalent address32 direct-base interpreter cases are compared with RIP normalized for their shorter encoding. Additional cases cover qword crossing 4 GiB as one callback, final disp32 byte target identity, backend no-commit, committed store+STOP, entry guards, and dispatcher actual-RIP continuation from 5/6/9-byte SIB singleton to the frozen pair without replay. See [S3-20cb report](../reports/STAGE_3_S3-20cb_2026-10-28.md). Production owner/MMU, Stage 2/ISA, Mini and performance remain outside this slice.

### S3-20cc address64 RBP-base SIB with no index and displacement

Run `cargo test -p od-jit --lib rbp_base_no_index_sib_disp_has_exact_qword_profile --offline` and `cargo test -p od-host --lib rbp_base_no_index_sib_disp_matches_direct_ir_interpreter_and_dispatch --offline`. Admit only `48 8B/89 /r sib disp8|disp32`, ModRM `mod=01/10,r/m=100`, SIB exactly `0x25`, data RAX..RBP, Qword/64-bit EA, lengths 5/8. This is RBP base with no index; `mod=00` + SIB `0x25` remains the distinct absolute/no-base profile. Decoder-owned `MemoryOperand::access_address_48` computes old RBP + signed displacement with u64 wrapping.

The JIT descriptor matrix validates both opcodes, six data registers, signed disp8/disp32 boundaries, direct ModRM RBP-base semantic comparison, source/decoded metadata, partial windows and invalid neighbors. The addr32 `67` mod01/mod10 sibling is covered separately by S3-20cd; mod00 remains the absolute/no-base shape. The host matrix compares direct V2/shared IR/interpreter full CPU/RAM state, callback target/width/count and retirement. It covers data=RBP old-value semantics, source disp32 final-byte target identity, #SS noncanonical start/end with no callback, separate AddressOverflow, architectural/backend atomicity, committed store+STOP, entry guards and dispatcher continuation from actual RIP through both 5/8-byte forms without replay. See [S3-20cc report](../reports/STAGE_3_S3-20cc_2026-10-28.md). Production owner/MMU, Stage 2/ISA, Mini and performance remain outside this slice.

### S3-20cd address32 RBP-base SIB with no index and displacement

Run `cargo test -p od-jit --lib address32_rbp_base_no_index_sib_disp_has_exact_qword_profile --offline` and `cargo test -p od-host --lib address32_rbp_base_no_index_sib_disp_matches_all_paths_and_dispatch --offline`. Admit only singleton `67 48 8B/89 /r sib disp8|disp32`, ModRM `mod=01/10,r/m=100`, SIB exactly `0x25`, data RAX..RBP, Dword EA/Qword data, lengths 6/9. Decoder-owned `access_address_48` computes `(low32(old RBP)+signed displacement) mod 2^32`; `mod=00` with the same SIB remains the separate S3-20av absolute/no-base encoding.

JIT tests cover load/store × six data registers × signed disp8/disp32 edges, direct-base metadata equivalence, and partial/invalid neighbors. Host direct V2/shared IR/interpreter checks full CPU/RAM, callback target/width/count and retirement, including data=RBP pre-state, contiguous qword at 4 GiB, architectural/backend no-commit, entry STOP/budget zero, store+STOP, final disp32 byte identity, and dispatcher actual-RIP continuation through both lengths without replay. See [S3-20cd report](../reports/STAGE_3_S3-20cd_2026-10-28.md).

### S3-20ce address64 RSI/RDI-base SIB with no index

Run `cargo test -p od-jit --lib low_and_rsi_rdi_base_no_index_sib_have_exact_qword_profiles --offline` and `cargo test -p od-host --lib low_and_rsi_rdi_base_no_index_sib_match_direct_ir_and_interpreter --offline`. Admit only singleton `48 8B/89 /r sib [disp8|disp32]`, ModRM `mod=00/01/10,r/m=100`, SIB exactly `0x26/0x27`, data RAX..RBP, Qword address/data, and lengths 4/5/8. Scale is one; index is absent and RSP is not read for EA. Decoder-owned `MemoryOperand::access_address_48` computes full old RSI/RDI plus signed displacement with wrapping u64.

The descriptor test covers both directions, both bases, all six data registers, each length form and signed disp8/disp32 boundaries, exact source and decoded metadata, truncated windows and rejected scale/prefix neighbors. Host direct V2/shared IR/interpreter matrix compares complete CPU/RAM and equivalent direct-base interpreter effects with RIP normalized for shorter direct encoding; callbacks carry the exact qword target and width and retire once. Host cases exercise RSP independence, RSP/RBP data with noncanonical-base #GP start/end and zero callbacks, separate AddressOverflow, backend/architectural no-commit, committed store+STOP payload, entry guards, changed final disp32 byte, partial windows, invalid neighbors and bounded dispatcher continuation from actual RIP through 4/5/8-byte SIB form to the frozen pair without replay. See [S3-20ce report](../reports/STAGE_3_S3-20ce_2026-10-28.md). Mini and performance checks are deferred until the end of Stage 3; owner/MMU integration is outside this slice.

### S3-20ch direct-base qword load into RSI/RDI

Run the focused `rsi_rdi_load_destinations_are_limited_to_eight_direct_low_base_encodings` JIT matrix and `v2_rsi_rdi_direct_base_mov_matches_direct_ir_and_interpreter` host matrix. Admit only the eight 3-byte `48 8B /r` forms with ModRM `mod=00`, destination RSI/RDI and base RAX..RBX. Compare direct V2/shared IR with the interpreter for complete CPU state, RAM, high-bit qword result, one qword read and one retirement. Fault and budget-zero paths must leave full state unchanged; budget zero performs no callback. Reject addr32, displacement, SIB, non-`0x48` REX, R8 destination, narrow widths, arbitrary RSI/RDI `ReadGpr`/`WriteGpr`, and non-adjacent or mismatched checked-load writes. Verify staging offsets `[56,120)`, flags 120, RIP 128, saved X19/X20 at 144 and X21/X22 at 160, prefix 176, 16-byte frame alignment, and spill homes after the reserved prefix. Keep the 4096-byte block cap and rerun existing arithmetic, store and two-memory regressions. Linux checks may run; Mini and performance are deferred until Stage 3 ends. See [S3-20ch report](../reports/STAGE_3_S3-20ch_2026-10-28.md).

### S3-20cj address32 direct low-base MOV with RSI/RDI data

Run `cargo test -p od-jit --lib address32_rsi_rdi_data_has_exact_48_direct_low_base_forms --offline` and `cargo test -p od-host --lib v2_rsi_rdi_direct_base_mov_matches_direct_ir_and_interpreter --offline`. Accept only `67 48 8B/89 /r [disp8|disp32]`, ModRM `mod=00/01/10`, RSI/RDI data and RAX..RBX base, Dword EA/Qword transfer, lengths 4/5/8. Verify all 48 opcode/data/base/form combinations and direct V2/shared IR/interpreter full-state and RAM parity, callback address/width/count, RIP and retirement. Cover signed displacement boundaries, high-half truncation and wrap, continuous qword access across 4 GiB, atomic faults, store+STOP, entry guards, actual-RIP continuation without replay, changed last disp32 byte, partial windows 1..3/1..4/1..7, forged descriptors and forbidden neighbors. Ensure RSI/RDI `WriteGpr` remains tied to the immediately preceding matching checked load, with generic reads/arithmetic closed. Keep frame 176, code cap 4096, and 10×MOV size 3868. Mini/performance are deferred to Stage 3 end; owner/MMU/Stage 2 are outside this slice. See [S3-20cj report](../reports/STAGE_3_S3-20cj_2026-10-28.md).

### S3-20ci address64 direct low-base MOV with RSI/RDI data and displacement

Run `cargo test -p od-jit --lib rsi_rdi_data_direct_displacement_profile_is_exactly_32_encodings --offline` and `cargo test -p od-host --lib v2_rsi_rdi_direct_base_mov_matches_direct_ir_and_interpreter --offline`. Admit only `48 8B/89 /r disp8|disp32` with ModRM `mod=01/10`, RSI/RDI data and RAX..RBX base, qword transfer, lengths 4/7. Compare direct V2/shared IR/interpreter complete CPU and RAM, callback target/width/count, retirement and actual next RIP. Descriptor and host cases cover all 32 combinations and signed displacements; assert full-width wrap, #GP start/end without callback, endpoint overflow, backend/architectural atomicity, budget-zero guard, one qword store followed by STOP, final disp32-byte identity, partial windows 1..3/1..6, and rejection of addr32, SIB, extra prefix, other data/base registers and narrow widths. RSI/RDI `WriteGpr` must remain immediately after the corresponding checked load; store sources are accepted only for this exact decoded operation, while generic reads/arithmetic remain closed. Keep the frame at 176 bytes, emitted block cap at 4096, and 10×MOV code size at 3868. Mini/performance and owner/MMU/Stage 2 remain deferred/outside this slice. See [S3-20ci report](../reports/STAGE_3_S3-20ci_2026-10-28.md).

### S3-20ck RIP-relative qword MOV with RSI/RDI data

Run `cargo test -p od-jit --lib admits_only_exact_rip_relative_mod00_disp32_sites --offline`, `cargo test -p od-jit --lib rip_relative_rsi_rdi_sites_reject_forged_descriptors_and_partial_windows --offline`, `cargo test -p od-host --lib v2_rip_relative_mov_success_targets_follow_guest_rip --offline`, and `cargo test -p od-host --lib v2_rip_relative_final_disp32_byte_changes_executed_target --offline`. Admit exactly `48 8B/89 /r disp32`, ModRM `mod=00,r/m=101`, data RSI/RDI, Qword EA/data and length 7. Decoder `AddressBase::NextRip` plus signed disp32 is the sole EA source. Verify all four encodings in direct V2/shared IR/interpreter, full state/RAM/callback/retirement parity, signed disp32 boundaries, same source at two guest RIP targets, final disp32-byte identity through executed distinct targets, and one committed qword store+STOP. Reject bad descriptors (displacement/base/index/scale/address/data size/length), mismatched source bytes, partial windows 1..6, address override, prefix, SIB and invalid data/base shapes; verify noncanonical #GP, fault atomicity, architectural/backend errors and budget-zero guards on all four forms, plus actual-RIP+7 continuation without replay. Preserve frame 176, block cap 4096 and 10×MOV size 3868. Mini and performance checks wait until Stage 3 ends; owner/MMU and Stage 2 remain outside scope. See [S3-20ck report](../reports/STAGE_3_S3-20ck_2026-10-28.md).

### S3-20cl Address64 RSP-base SIB with RSI/RDI data

Run `cargo test -p od-jit --offline rsi_rdi_rsp_sib_qword_mov_has_exact_twelve_form_profile`, `cargo test -p od-jit --offline rsp_sib_rsi_rdi_dispatch_respects_partial_windows_and_actual_rip`, and `cargo test -p od-host --offline s3_20cl_rsp_sib_rsi_rdi_mov_matches_native_ir_and_interpreter`. Accept only 12 forms: `48 8B/89`, ModRM `mod=00/01/10,r/m=100`, SIB `0x24`, RSI/RDI data, qword EA/data, lengths 4/5/8. Check signed displacement edges, full CPU/RAM and callback address/width/count against the interpreter, RSP start/end #SS, RBX #GP control, committed store+STOP payload, partial windows, and actual-RIP continuation without replay. Verify invalid SIB/address-size/prefix/REX/length neighbors remain unsupported. Keep frame 176, emitted-code cap 4096, and 10-MOV regression 3868. Mini/performance checks wait until Stage 3 ends; owner/MMU and Stage 2 are outside scope. See [S3-20cl report](../reports/STAGE_3_S3-20cl_2026-10-28.md).

### S3-20cm: Address64 direct RBP-base qword MOV with RSI/RDI data

Run `cargo test -p od-jit --lib rsi_rdi_data_direct_displacement_profile_is_exactly_40_encodings --offline`, `cargo test -p od-host --lib v2_rsi_rdi_direct_base_mov_matches_direct_ir_and_interpreter --offline`, and `cargo test -p od-host --lib rbp_direct_rsi_rdi_dispatch_handles_partial_stop_and_actual_rip --offline`. Accept only eight singleton direct forms `48 8B/89 /r disp8|disp32`, ModRM `mod=01/10`, RSI/RDI data and RBP base, lengths 4/7. `[RBP]` is disp8 zero; `mod=00,r/m=5` remains RIP-relative. Check direct V2/shared IR/interpreter full CPU/RAM/callback/retirement parity, both data registers, signed displacement edges, unaligned and old-RBP alias accesses, source-byte binding, dispatcher STOP on both callback types, exact one retirement and no replay at actual RIP +4/+7, partial windows 1..3/1..6, changed final disp32 byte, entry guards, architectural/backend atomicity, #SS at noncanonical start/end, and RBX #GP control. Keep RSI/RDI `WriteGpr` tied to the immediately matching `CheckedLoad` and generic reads/arithmetic closed. Preserve frame 176, emitted-code cap 4096, and 10-MOV size 3868. Mini/performance defer to Stage 3 end. S3-20cm is an optional slice, not a Stage 3 completion gate under S3-33. See [S3-20cm report](../reports/STAGE_3_S3-20cm_2026-10-29.md).

### S3-29e bounded arithmetic/checked-load coordinator

Run `cargo test -p od-host --lib s3_29e_bounded_arithmetic_load_coordinator --offline` and `cargo test -p od-jit --offline`. Exercise the public `od_jit::memory_dispatch::run_bounded_arithmetic_load` call with eager and uncached demand-flags strategies over one/two exact ADD/SUB/CMP full writers, `48 8B 03`, JZ/JNZ and the existing ADD fallthrough. Compare complete state, RAM, flags and retirement with the interpreter for both branch outcomes. Verify `#PF` and `#GP` preserve the prefix and load RIP with no load retirement/RAX commit/suffix, then resume at that RIP without replay; backend failures must remain host errors. Check shared budget and stop-RIP/STOP/event priority at each phase boundary and after publication before native entry. Inject malformed native and memory reports, invalid plans/entries, unsupported compilation, zero progress and invalid phase RIPs; require typed results/errors without panics or masked faults. Confirm register mappings are released before memory publication and memory mappings before suffix publication through host adapters. No owner/MMU, cache, ISA-form, Mini or performance claims are in this slice. Full Stage 3 local harness and Linux workspace checks are recorded in the [S3-29e report](../reports/STAGE_3_S3-29e_2026-10-29.md).

### S3-29f single-block eager/demand-flags measurement path

`od-bench flags-compare` uses a fixed seven-case profile of accepted S0-09
W64 forms (`48 03 C0`, `48 03 C3`, `48 3B C3`, short JZ/JNZ): replaced
arithmetic writers, a budget exit between writers, aliased ADD inputs, and
taken/not-taken JZ and JNZ. Both compilers must emit the complete single
block. The command compiles and interpreter-checks all cases, then validates
both eager and candidate execution for the entire profile before timing any
case. It compares complete CPU state, branch continuation, exit accounting,
retirement and remaining budget with the interpreter. The report fingerprints
benchmark, JIT, host execution and interpreter source files, as well as guest
and emitted code. Three M3 Max release runs completed the full profile; all
seven cases had cross-run median paired candidate/eager ratios above 1.0, and
the aliased ADD case was near parity at 1.003352. Candidate code was 40–47%
larger. The
measured interval sums repeated calls to one published artifact after state
reset; compilation, publication, reset and validation remain separate.
Variant order alternates across 20 pairs, each with at least 200 ms per
variant. One MAP_JIT mapping is live at a time and is dropped before the paired
variant is published. An earlier preflight attempt
used unsupported ADD/SUB forms and produced no timing or output artifact.
That rejected attempt is historical method-report evidence only; its exact
command, revision and profile hash were not preserved, and new run JSON does
not claim it as a per-run event.
Timed calls include the `execute_block` safe wrapper, state
materialization/snapshot and arithmetic-flags helper work. It does not measure
dispatcher/cache lookup, interpreter fallback, checked-memory helpers or
guest-memory workloads, and does not select the production default. Mini and
physical-x86 checks remain deferred until the end of Stage 3. See the
[S3-29f report](../reports/STAGE_3_S3-29f_2026-10-30.md).

### S3-29g cached NativeRegisterV1 dispatcher comparison

`od-bench cached-flags-compare` exercises the full cached dispatcher for a
fixed seven-case register profile, with an explicit interpreter/cache
preflight before timing. Three sequential M3 Max release runs found cold
candidate/eager median ratios of 1.0354–1.1682 and resident ratios of
0.9993–1.0090. Candidate code was 28.2–57.5% larger for the four emitted
shapes. This does not select a global default or cover memory helpers, Mini,
physical x86, or the complete Stage 3 performance gate. See the
[S3-29g report](../reports/STAGE_3_S3-29g_2026-10-31.md) and its raw
[preflight](../reports/data/stage3/s3-29g/preflight.json),
[process 1](../reports/data/stage3/s3-29g/cached-flags-compare-1.json),
[process 2](../reports/data/stage3/s3-29g/cached-flags-compare-2.json), and
[process 3](../reports/data/stage3/s3-29g/cached-flags-compare-3.json).

### S3-35a cold first full-corpus benchmark path

`od-bench cold-first-run` compares interpreter execution and the default
FIFO(16) block dispatcher on the exact frozen S3-17 register-only corpus.
Correctness preflight and oracle construction run before timing. Every timed
dispatcher sweep creates a fresh runtime/cache/arena with an empty code cache;
timing includes initialization plus all dispatch calls through first full
corpus completion, but excludes input preparation, state cloning/reset,
validation, and teardown. Samples alternate order and report raw counts,
median, range, IQR, and paired per-sweep ratios. Three local release runs
measured the cold FIFO dispatcher at 6.64–6.69 ms per corpus sweep and
interpreter at 0.164–0.166 ms. The raw results and exclusions are in the
[S3-35a report](../reports/STAGE_3_S3-35a_2026-10-30.md). This is one bounded
cold workload; S3-35 remains partial and warm/performance acceptance remain
open. Mini/physical and Stage 2 checks are outside this slice.

### S3-36a resident warm register-only benchmark path

`od-bench warm-first-run` selects a deterministic prefix of the frozen S3-17
cases whose summed per-case unique-entry budget fits the FIFO(16) cache. This
budget is conservative and is not the count of globally distinct cache keys.
It warms one runtime
before calibration and keeps it alive through alternating interpreter and
dispatcher samples. Every measured dispatcher sample checks for hits and zero
misses, compiles, publications, and evictions; state reset and result validation
stay outside timed calls. The full 1,010-case corpus churns FIFO16 and cannot
provide the resident warm evidence this path measures. Three release runs
measured the warm dispatcher at 12.129–12.343 µs and
interpreter at 2.019–2.053 µs per selected corpus sweep. The interpreter was
about 6.0× faster on this register-only slice; S3-36 remains partial. Details
and raw results are in the
[S3-36a report](../reports/STAGE_3_S3-36a_2026-10-30.md).

### S3-36b bounded S3-14 branch/helper warm profile

`od-bench warm-branch-helper` measures four accepted conditional-branch
boundary fixtures and one live-through-helper fixture using a frozen case list
and trace. Three release runs measured the warm dispatcher at 3.736–3.959 µs
and interpreter at 0.726–0.751 µs per five-case sweep. The interpreter was
about 5.3× faster on these fixtures; memory workloads and the full S3-36
matrix remain open. See the [S3-36b report](../reports/STAGE_3_S3-36b_2026-10-30.md).

### S3-37a bounded FIFO16 corpus churn path

`od-bench cache-churn` runs 128 full sweeps of the frozen S3-17 corpus on one
retained production FIFO16 runtime and records per-sweep/cumulative cache
counters and resident code/metadata and arena high-water. Its unoccupied-slot
value is derived as capacity minus resident-entry high-water, not read from an
allocator counter. Three release runs each observed 168,960
misses/compiles/publications, 168,944 evictions, 256 hits, and the same
resident high-water: 16 entries, 21,640 code bytes, and 50,124 logical
metadata bytes. This is a bounded capacity-churn observation. The path does
not measure process RSS, teardown, independent runtime instances, SMC
invalidation, or longer leak-free stress; S3-37 remains open. See the
[S3-37a report](../reports/STAGE_3_S3-37a_2026-10-30.md).

### S3-37b process-wide RSS snapshots

The same 128-sweep workload now records process-wide RSS before runtime
creation, before runtime drop, and after explicit drop. Three release runs
observed 5,024–5,056 KiB before creation, 5,808–6,000 KiB before drop, and
5,744–5,936 KiB after drop. Each post-drop sample was 64 KiB below its
pre-drop sample and remained above its pre-runtime baseline. RSS includes all
benchmark-process allocations and cannot attribute changes to the cache;
runtime lifecycle, SMC, broader workloads, and leak-free behavior remain
unverified. See the [S3-37b report](../reports/STAGE_3_S3-37b_2026-10-30.md).

### S3-37c deterministic cache and arena lifecycle path

`od-bench cache-lifecycle` exercises the existing production FIFO16 cache with
17 admitted one-entry fixtures: fill, controlled stop-RIP at the current guest
boundary requiring `StopAddress` (distinct from async `StopRequested`), repeated hit, FIFO replacement, targeted between-call backing
invalidation, changed-code miss/recompile and hit, execution-context generation
advance, explicit clear, runtime drop, and fresh runtime reopen. The runner
compares full interpreter-visible state, exit,
retirement, and budget results on each dispatch. It snapshots current cache
accounting and actual arena free slots independently from high-water counters.
This is deterministic supported-API lifecycle evidence, not guest SMC or
leak-free stress evidence; release measurements and review outcome remain in
the S3-37c report. The controlled STOP step verifies no lookup/dispatch/cache
clear, with explicit clear measured as a separate operation.

### S3-37d sustained mixed FIFO16 lifecycle stress

`od-bench cache-lifecycle-stress-calibrate` runs 512 warm-up epochs followed
by eight measured 65,536-epoch batches on one retained runtime, then sizes the
scored count from the fastest batch rate. The profile targets at least 11
minutes and reserves one minute below the 30-minute process cap. Calibration
keeps 128 bounded RSS/report/write checkpoints across 524,800 epochs, about
one every 4,100 epochs; its per-batch body timing excludes checkpoint work.
Warm-up is excluded from scored loop timing. The integrity digest,
source/build/host fingerprint, corpus, schedule and timing arithmetic are
checked before scoring. A scored report explicitly records whether actual
elapsed time met 660 seconds; an under-target report is persisted with an
error status and the command fails. One overall deadline covers input
preparation through final report persistence. Each retained-runtime epoch
validates hit, unique pressure miss and one FIFO eviction, between-call
backing invalidation, changed admitted
code at a new backing version, and the subsequent hit. Full interpreter
parity is checked on each dispatch. Every 128 epochs, clear must return all
current resident usage to zero and make all 16 slots available; every 512
epochs the sole runtime and arena are dropped and reopened before execution
continues. The report uses bounded checkpoints and separates current occupancy
from high-water values. RSS is process-wide auxiliary evidence only. This does
not claim guest SMC, owner notification, fault cleanup, concurrent arenas,
SMP, or leak-free behavior. Three sequential v5 release processes completed
in Stage2-quiet windows. Runs 1–3 completed 45,436,794 / 46,049,027 /
46,049,027 epochs in
671.161073750 / 679.267265042 / 688.093795250 seconds, respectively; each
met the 660-second minimum and exact counters reconciled. Source, calibration,
and all three scored artifacts passed independent review. V4 run1 completed
29,682,315 epochs in 439.482 seconds, below target, and remains separate
lifecycle/counter diagnostic evidence. Raw JSON, limitations and review status
are in the [S3-37d report](../reports/STAGE_3_S3-37d_2026-10-31.md). Full
S3-37 remains open. Mini stays deferred until Stage 3 ends.
