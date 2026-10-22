# S1-06 manually encoded boundary fixtures

These byte sequences are hand-authored from the project's [CPU contract](../../../docs/specs/CPU_CONTRACT.md), using the Intel SDM 093 instruction forms documented there. They extend the original S0-09 cases with signed overflow, subtraction borrow, signed CMP, taken/not-taken JZ, and initial flag preservation. The `.case.json` program SHA-256 values cover the exact bytes below.

| Case | Bytes | Check |
|---|---|---|
| `s1_06_add_signed_overflow` | `4803c1` | `0x7fff…ffff + 1` sets SF/OF without carry |
| `s1_06_sub_borrow` | `482bc1` | `0 - 1` wraps to all ones and sets borrow |
| `s1_06_cmp_signed_boundary` | `483bc1` | `INT64_MIN` compared with `INT64_MAX` |
| `s1_06_jz_taken` | `48b8010000000000000048b90100000000000000483bc1740a48ba0000000000000000` | Equal operands take JZ and preserve initial RDX |
| `s1_06_jz_not_taken` | `48b8010000000000000048b90200000000000000483bc1740a48ba0000000000000000` | Unequal operands do not take JZ and set RDX to zero |
| `s1_06_mov_preserves_flags` | `48bb8877665544332211` | MOV preserves all modeled arithmetic flags supplied at entry |
