# S2-32a/S2-32b — bounded x87 transcendental, scaling, extraction and BCD forms

Slice of S2-32 (the task checkbox stays open). Branch `s2-x87-windows`; commits: `b393147` (S2-32a code), `662e216`
(S2-32a tests), `5bf8451` (S2-32b code), `fd5f0aa` (S2-32b tests), then the documents.

## What runs

* S2-32a: bare `D9 F0` F2XM1, `F1` FYL2X, `F2` FPTAN, `F3` FPATAN, `F4` FXTRACT, `F9` FYL2XP1, `FB` FSINCOS, `FD` FSCALE, `FE` FSIN,
  `FF` FCOS in the bounded local x87 profile (`state.x87`, interpreter only; the CPL0 set still owns only FNINIT, FXSAVE, FXRSTOR64
  and the MMX forms; JIT and native admission stay closed).
* S2-32b: `DF 20` FBLD and `DF 30` FBSTP with the operand at `[RAX]`.
* Values: `crates/od-isa/src/x87_transcend.rs`, a 256-bit software float with one rounding to the 64-bit significand under the
  control-word RC. Admission, status-word rules and refusals: `docs/specs/CPU_CONTRACT.md`, sections Local S2-32a and S2-32b.
* SDM 325462-093US Vol. 2A: F2XM1 3-307, FBLD 3-314, FBSTP 3-316, FCOS 3-330, FPATAN 3-364, FPTAN 3-370, FSCALE 3-378, FSIN 3-380,
  FSINCOS 3-382, FXTRACT 3-420, FYL2X 3-422, FYL2XP1 3-424; Vol. 1 section 8.3.8, 8.3.10, 8.1.3.2, 4.7.

## Model accuracy and the physical oracle

The oracle is the Mac mini (Ubuntu x86-64, Core i5-3210M): `crates/od-isa/tests/data/x87_oracle.c` runs one instruction between
FNINIT/FLDENV and FNSTENV/FSTP, `x87_oracle_gen.py` writes the operands, `x87_oracle_i5_3210m.txt` holds the 1,800 answers (md5 of
the answers `d78f41701853d9b3ef93539115d92617`); `x87_bcd_oracle*` the same for FBLD/FBSTP (335 answers).

Findings that shaped the model:

* PC is ignored and RC honoured by every transcendental form (12 control words checked).
* FSIN/FCOS/FSINCOS/FPTAN reduce by the 66-bit approximation of pi/2 (the first 66 significant bits of pi). With it the oracle is
  reproduced including operands next to multiples of pi/2 up to about 2^60 (168 rows); the exact pi reproduces only a fraction.
* |x| >= 2^63: C2 = 1, C1 = 0, operand unchanged, no push (FSIN, FCOS, FSINCOS, FPTAN).
* Tiny arguments: |r| < 2^-68 returns the operand (1.0 for the cosine) with PE and C1 = 0 in every RC; FPTAN below 2^-33 behaves as if
  tan r lay just below r; FPATAN below 2^-40 returns the rounded quotient with PE. F2XM1(+-1) and FYL2X(2^k) report PE although exact;
  FYL2X(1, y) = +-0 with the sign of y; FYL2XP1(+-0, y) = +-0 with the product of the signs; FXTRACT/FSCALE clear C1.

Result of the interpreter against the 1,800 rows (pinned in `tests/s2_32_x87_transcend.rs`): 96 out-of-range operands unchanged,
2 FYL2XP1 operands outside the SDM domain refused, of the other 1,702 rows 1,654 are bit-identical and 48 differ by exactly one ulp
(the microcode is not correctly rounded); C1 differs on 22 rows; TOP, C0/C2/C3, tags and PE on none. Per instruction (rows /
exact / one ulp / C1 differs): F2XM1 146/138/8/3, FCOS 236/235/1/1, FPATAN 184/178/6/1, FPTAN 236/231/5/8, FSCALE 86/86/0/0,
FSIN 236/234/2/0, FSINCOS 236/232/4/0, FXTRACT 60/60/0/0, FYL2X 152/145/7/5, FYL2XP1 130/115/15/4.
FBLD/FBSTP: 335/335 bit-identical (result bytes, TOP, C0-C3, PE, tags).

## Validation

* `cargo test -p od-isa --offline --lib`: 2,714 passed (the decoder tests that used D9 F4/F9/FB, DF 20 or DF 30 as unsupported
  examples were adjusted).
* `--test s2_32_x87_transcend`: 5 passed; `--test s2_32b_x87_bcd`: 3 passed.
* `cargo check --workspace --all-targets --offline`, `cargo test -p od-jit --offline` (JIT denial unchanged), `cargo fmt --all --
  --check`, `git diff --check`.

## Not done

FNSTENV/FNSAVE/FRSTOR (and the FWAIT forms): their images carry FCS/FDS, which the local profile does not represent (it requires both
selectors absent); needs the selector decision first. Denormal, NaN, infinite and out-of-domain operands, unmasked exceptions and stack
faults of the new forms stay `Unsupported`. CPL0 wiring of x87 arithmetic (as S2-39 did for MMX), arbitrary addressing and prefixes for
FBLD/FBSTP, JIT/native, and the CPUID FPU bit (S2-57) are outside the slice. The parent S2-32 is open.

## Review fixes (2026-11-22)

* FYL2XP1 admits only the SDM range of ST(0), |x| <= 1 - sqrt(2)/2 (exact comparison on the binary80 value; the oracle has no row in (1 - sqrt(2)/2, sqrt(2) - 1], so the pinned counters are unchanged and the boundary is pinned by explicit vectors in the tests).
* FBLD refuses a non-decimal digit after the 10-byte read and before any x87 state, RIP or register change.
* The FPU blocker of S2-57 is checked through the CPL0 set (x87 state None) and the missing FSTENV/FSAVE; SDM references corrected (condition codes: Vol. 1 section 8.1.3.2; 66-bit pi: section 8.3.8; FBSTP: Vol. 2A p. 3-316).
