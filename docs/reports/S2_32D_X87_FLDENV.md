# S2-32d — bounded FLDENV

Slice of S2-32 (the task checkbox stays open). Branch `s2-32d-fldenv` (from `stage8`); commits: `94103a5` (code), `bc95205` (tests), then the documents.

## What runs

* Bare `D9 20` FLDENV with the 28-byte image at `[RAX]`, no prefix, in the bounded local x87 profile (`state.x87`, interpreter only; the CPL0 set
  does not own it; JIT and native admission stay closed). One 28-byte read; FCW, FSW (TOP, C0..C3), tag word, FIP/FDP (low 32 bits, upper
  cleared), FOP (11 bits), FCS/FDS are loaded; the register contents are untouched. Spec: `docs/specs/CPU_CONTRACT.md`, section Local S2-32d.
* Shares `x87_restore_image` with FRSTOR (flag `with_registers`), the admission preflight and the exemption from the selector refusal with the
  three S2-32c forms. Tags: empty stays empty, any other slot is tagged from the register contents. Selectors: the S2-32c policy
  (`Some(v)` kept, a zero selector stays `None` when it was `None`); no CPUID bit is declared.
* SDM 325462-093US Vol. 2A: FLDENV p. 3-358; Vol. 1 section 8.1.8, 8.1.10, Figure 8-9.

## Physical oracle

Mac mini, Core i5-3210M: `crates/od-isa/tests/data/x87_fldenv_oracle.c` runs `FNINIT; FRSTOR a; FLDENV b; FNSTENV; FNSAVE; FNSTENV` for each of 324
rows from `x87_fldenv_oracle_gen.py` (a: 108-byte register image; b: independent 28-byte environment with random TOP, C bits, sticky flags, tag
words including all-empty/all-valid, FIP/FDP/FOP, zero and non-zero selectors); answers in `x87_fldenv_oracle_i5_3210m.txt`. All 324 rows
(28 + 108 + 28 bytes each) are reproduced exactly from two starting selector states (648 model runs). Row coverage: 64 zero FCS, 80 zero FDS, 43
all-empty tag words, 284 rows where FLDENV changes TOP. The first model run matched with no code change after the oracle: the tag word after FLDENV is
derived from the register contents exactly as after FRSTOR.

## Validation

`tests/s2_32d_x87_fldenv.rs` (7 tests), `s2_32c_x87_env` (8), `s2_57_cpuid_windows` (19), `--lib long_decode` (431), `--lib x87` (103),
`s2_32_x87_transcend` (6), `s2_32b_x87_bcd` (3), `cargo fmt --all -- --check`, `git diff --check`.

## Not done (rest of S2-32)

16-bit (m14byte) and real-mode images; images with unmasked exceptions, ES/B or the X bit (refused, the SDM raises #MF at the next waiting
instruction); #NM and #AC; the CPL0 set and the `FSTENV`/`FSAVE` `FWAIT` spellings; FCS/FDS of executed sequences equal to the i5-3210M's; the earlier S2-32a/S2-32b remainder.
