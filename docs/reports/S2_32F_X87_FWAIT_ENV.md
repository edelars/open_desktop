# S2-32f — FSTENV/FSAVE with FWAIT

Slice of S2-32 (the task checkbox stays open). Branch `s2-32-fwait` (from `stage8`); commits: `21c4652` (code), `f2d1e7a` (tests), then the documents.

## What runs

* `9B D9 30` (FSTENV) and `9B DD 30` (FSAVE), image at `[RAX]`, no prefix: one instruction of three bytes with the operation of the non-waiting form
  (`FnstenvRax`/`FnsaveRax`, opcode `9B`, length 3), in the bounded x87 profile and at CPL0 of the set (`step_cpl0_bounded_x87`). Spec: `docs/specs/CPU_CONTRACT.md`, section Local S2-32f.
* The FWAIT part: in the bounded profile a no-op (the admission of the environment forms leaves no pending unmasked exception); a state that would give `#MF` (незамаскированное FCW, ES, B) is refused `Unsupported` with opcode `9B` and no trace (the `#MF` is carried over by ADR-0015). In the set: the FWAIT checks of `step_cpl0_fp` first (`#NM` for MP and TS,
  `#MF` for a pending exception with NE set), then the gates of the x87 forms.
* The decoder: only the exact bytes `D9 30`/`DD 30` after a bare `9B`; any other byte, a prefix between `9B` and the escape byte, a truncated fetch or a fetch error is the bare FWAIT of one byte
  (length restored). A prefix before `9B` is refused. FLDENV, FRSTOR and the non-waiting forms are unchanged.
* SDM 325462-093US: Vol. 2A FSTENV/FNSTENV p. 3-390, FSAVE/FNSAVE p. 3-375, WAIT/FWAIT (Vol. 2D); Vol. 1 section 8.7.

## Saved RIP of a fault, look-ahead (review fixes)
* Intel SDM (FSTENV/FSAVE): the assembler issues two instructions, FWAIT and FNSTENV/FNSAVE; "if an exception is generated for either of these instructions, the save EIP points to the instruction that
  caused the exception". Rule: an error of the FWAIT part (#NM for CR0.MP with CR0.TS, #MF) leaves RIP at `9B`; an error after the FWAIT part retired leaves RIP at `9B` + 1: the #NM of CR0.EM (EM, or TS with
  MP clear, is no condition of FWAIT, it gates the non-waiting form), and every architectural fault of the store (non-canonical last byte, #GP, #PF); only RIP changes on a fault. Model errors of the store (unmapped memory, wrapping address past 2^64, backend failure) are not x86 exceptions: as in FLDENV/FNSTENV they return `Err` with the state untouched and RIP at the first byte of the instruction (`9B`); the SDM rule about the saved EIP does not apply to them. CR0.EM with a pending
  unmasked exception is #MF at RIP `9B` (the FWAIT part is checked first); a test fails when the check order is swapped (checked by a mutation).
* The look-ahead after `9B` stays on the 4 KiB page of `9B`: when `9B` or the next byte is the last byte of its page nothing after `9B` is fetched (bare FWAIT). Test of the decoder (fetch log) and of the
  paged adapter in `od-machine` (no page fault, memory error or access error record, the accessed bit of the next page stays clear). Limitation: a spelling across a page boundary is two instructions
  (the bounded profile refuses the bare FWAIT; in the CPL0 set the split spelling runs as two instructions, FWAIT and then the non-waiting form, with the same effect).

## Physical oracle

Mac mini, Core i5-3210M: `crates/od-isa/tests/data/x87_fwait_oracle.c` runs `FNINIT; FRSTOR in; FSTENV; FSAVE; FSTENV` over the 324 images of S2-32c (the assembler emits `9B D9 30`/`9B DD 30`, checked with
objdump); answers in `x87_fwait_oracle_i5_3210m.txt`. The file is byte-identical to `x87_env_oracle_i5_3210m.txt` (the non-waiting forms). The model reproduces all 324 rows (28 + 108 + 28 bytes) from
two starting selector states (648 runs of the three-instruction sequence).

## Validation

`tests/s2_32f_x87_fwait_env.rs` (14 tests; two added: model error keeps RIP `9B`, architectural #GP is `9B` + 1), `od-machine` `a_bare_fwait_on_the_last_byte_of_a_page_leaves_no_record_of_the_next_page`, `s2_32c_x87_env` (8; decoder test updated), `s2_32d_x87_fldenv` (8), `s2_32e_cpl0_x87` (13; the `9B D9 30` pair is one instruction), `s2_57_cpuid_windows` (19),
`cpl0_profile_set` (1 626), `s2_37_cvt_int` (36), `--lib long_decode` (431), `--lib x87` (103), `--lib fwait` (4), `s2_32_x87_transcend` (6), `s2_32b_x87_bcd` (3), `cargo fmt --all -- --check`, `git diff --check`.

## Not done (rest of S2-32)

`#MF` delivery for pending exceptions and 16-bit images (carried over); FWAIT before other forms; images with unmasked exceptions, ES/B or the X bit; #AC; FCS/FDS of executed sequences equal to the
i5-3210M's; the earlier S2-32a/S2-32b remainder; JIT and native (closed). Not checked: the NE-clear `Unsupported` of the set (not reachable under the CR0 policy), a fused form through other adapters
that use `long_decode` over paged memory beyond the one covered by the `od-machine` test (the decoder reads up to two bytes after a `9B`, on the page of `9B` only).
