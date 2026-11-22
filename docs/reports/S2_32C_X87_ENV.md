# S2-32c — bounded FNSTENV, FNSAVE and FRSTOR

Slice of S2-32 (the task checkbox stays open). Branch `s2-32c-env` (from `stage8`); commits: `6630fdd` (code), `b19c9be` (tests), then the documents.

## What runs

* Bare `D9 30` FNSTENV, `DD 30` FNSAVE and `DD 20` FRSTOR with the image at `[RAX]`, no prefix, in the bounded local x87 profile
  (`state.x87`, interpreter only; the CPL0 set does not own them; JIT and native admission stay closed). `FSTENV`/`FSAVE` are
  `FWAIT` plus the non-waiting form, so they are not forms of their own.
* The 32-bit protected-mode image (28 bytes; 108 with ST(0)..ST(7) for FNSAVE), one write or one read. Layout, tag word, admission and
  the FCS/FDS decision: `docs/specs/CPU_CONTRACT.md`, section Local S2-32c.
* SDM 325462-093US Vol. 2A: FRSTOR p. 3-373, FSAVE/FNSAVE p. 3-375, FSTENV/FNSTENV p. 3-390; Vol. 1 section 8.1.8, 8.1.10, Figure 8-9.

## FCS/FDS policy

Saved as the represented selector (`Some(v)`) or 0000H when none is represented (`None`): for `None` that is the behavior of a processor with FCS/FDS
deprecation (CPUID.07H.00H:EBX[13]); for `Some(v)` after FRSTOR it is the Core i5-3210M (no deprecation, the selector is kept). So no x87 segment tracking is needed and no CPUID bit is declared. FRSTOR loads `Some(v)` (a zero selector
stays `None` when it was); FNSAVE clears known selectors. The three forms are exempt from the global refusal of `D8..DF` opcodes while a
selector is represented; every other bounded x87 form still refuses then.

## Physical oracle

Mac mini, Core i5-3210M: `crates/od-isa/tests/data/x87_env_oracle.c` runs `FNINIT; FRSTOR in; FNSTENV; FNSAVE; FNSTENV` for each of 324
images from `x87_env_oracle_gen.py` (random control word of the twelve all-masked ones, TOP, C bits, sticky flags, tag words, FIP/FDP/FCS/FDS/FOP
junk, register classes: normal, zero, denormal, pseudo-denormal, infinity, NaN, pseudo-NaN, unnormal, raw), answers in
`x87_env_oracle_i5_3210m.txt`. All 324 rows (28 + 108 + 28 bytes each) are reproduced exactly, from two starting selector states.
Findings: reserved fields are FFFFH (control/status/tag word upper halves, bytes 26..27), FOP keeps 11 bits, the tag word is recomputed from
the register contents with empty kept, FSAVE stores ST(0) first, FCS/FDS are saved as loaded (not deprecated on this CPU; after executed
instructions it saves CS 0033H and DS 002BH, which the model does not, hence image rows rather than instruction rows).

## Validation

`tests/s2_32c_x87_env.rs` (8 tests), `s2_57_cpuid_windows` (19, the FPU blocker check moved to the set-not-connected list),
`--lib long_decode` (431), `--lib x87` (103), `s2_32_x87_transcend` (6), `s2_32b_x87_bcd` (3), `cargo fmt --all -- --check`, `git diff --check`.

## Not done (rest of S2-32)

`FLDENV`; 16-bit and real-mode images; images with unmasked exceptions, ES/B or the X bit (refused); #NM and #AC; the CPL0 set and the
`FWAIT` spellings; FCS/FDS of executed sequences equal to the i5-3210M's; the earlier S2-32a/S2-32b remainder.
