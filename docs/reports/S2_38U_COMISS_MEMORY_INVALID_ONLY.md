# S2-38u — COMISS memory invalid-only profile

Implemented the strict Long64 `0F 2F 00` COMISS XMM0,m32 `[RAX]` interpreter profile under schema v2. The profile has independent control/result echo fields and is selected separately from S2-37cq, so S2-37cq's masked denormal behavior remains unchanged.

## Behavior

- Fetches and admits only the exact three-byte bare RAX form. SSE/CR0.EM/CR4.OSFXSR faults precede CR0.TS; CPL0–3 are exercised. AC set refuses before the data read; unaligned access is accepted with AC clear. The four-byte source span must be canonical48 and is read once.
- Requires supported MXCSR bits, DAZ=FTZ=0, DM/ZM/OM/UM/PM=1 (MXCSR mask bits 8–12 set), and permits either IM value and prior sticky status. After a successful read, either subnormal operand returns Unsupported without changing state. Finite and zero operands use raw integer ordering. COMISS QNaN and SNaN both set IE; masked IE completes with compare flags, while unmasked IE retains sticky IE and raises #XM when OSXMMEXCPT is set or #UD otherwise, without committing flags, RIP, or retirement.
- XMM registers and unrelated state are preserved. JIT and native backend requests remain Unsupported.

## Evidence

Five literal fixtures cover finite comparison, post-read subnormal refusal, masked QNaN, unmasked QNaN/#XM, and unmasked SNaN/#UD; MXCSR cases vary IM and prior sticky status. The execution tests check both operand positions for subnormals at all four CPL values, read-error rollback, exact fetch-before-gate order, and OSXMMEXCPT exception delivery. Generated runner cases validate QNaN/SNaN in either operand across IM × OSXMMEXCPT × all 64 prior sticky-status values (1,024 cases), plus finite/infinity/zero comparisons, gate/AC/span priority, exact-form rejection, schema/result tampering, and backend-label tampering.

Checks completed:

- `cargo fmt --all -- --check`
- `cargo check --workspace --all-targets --offline`
- `cargo test -p od-isa --offline comiss_` — 9 passed
- `cargo test -p od-testkit --offline` — 455 passed
- `cargo test -p od-runner --offline` — 664 passed
- Ran all five literal fixtures with the interpreter; observed two stop-address completions, one Unsupported refusal, and two exceptions.
- Requested JIT and native execution for the finite fixture; both returned Unsupported.
- `graphify update .`

The profile is enforced as interpreter-only by result validation, including forged backend labels. The standalone fixture runner uses flat memory. Injected read-error rollback is covered in the ISA execution unit test; actual paging/device callback integration is outside this profile's runner fixture contract.
