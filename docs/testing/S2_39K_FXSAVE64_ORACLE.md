# S2-39k — bounded CPL3 FXSAVE64 oracle implementation

**Status:** Linux x86_64 harness and independent evidence comparator are
implemented. Execution on this Darwin arm64 host is unavailable; all hardware
observations remain `not_run`. Do not treat cross-target compilation or static
disassembly as physical evidence.

## Scope and run

The runner exposes `od-x86-runner --fxsave64-oracle`. The only guest instruction
is exact `48 0F AE 40 40` (`FXSAVE64 [RAX+0x40]`) at CPL3. It performs A/B/A/B
with fresh shared mappings, explicit XMM0–15 byte tables, MXCSR `0x1f80` and
`0x7f80`, a 16-byte-aligned writable 512-byte image, and 32-byte canaries on
both sides. Raw JSON is written under ignored
`work/stage2/s2-39k-fxsave64/`. An unsupported host reports
`unavailable/not_run` and does not emit empty-success evidence.

The code is isolated from the fixture/native_v2 admission path. `od-x86-runner`
owns CPUID, CPU affinity, shared mappings, tracee lifecycle, ptrace, signal
source, addresses, raw capture and environment. `od-testkit::fxsave64` owns only
the strict evidence shape and independent literal byte comparator. It does not
call the production decoder, executor, FXSAVE serializer or CPU-state adapter.

## Audited assembly and completion marker

One global-assembly function saves caller XMM0–15 with MOVDQU and MXCSR with
STMXCSR, loads the literal test XMM/MXCSR inputs, captures independent pre
observations, emits the exact five FXSAVE64 bytes, and places INT3 immediately
at E+5. The tracee stops there. The parent requires SIGTRAP, reads PTRACE_GETREGS
and PTRACE_GETSIGINFO, requires raw RIP E+6, and allows only the reviewed Linux
INT3 signal codes 0, TRAP_BRKPT (1), or SI_KERNEL (0x80), alongside the exact
instruction/marker bytes and phase. It then resumes the tracee so MOVDQU/STMXCSR
capture post-state and restore caller XMM/MXCSR before normal exit. No FXRSTOR
is used. The measured completion boundary is normalized to E+5 by subtracting
the known one-byte INT3; it is not a retirement-counter claim.

Linux ptrace stops require sequence-aware interpretation and GETSIGINFO; the
runner uses a child declared with PTRACE_TRACEME, checks the exact marker and
reads general-purpose registers while the child is stopped. The accepted
signal-code set follows the [Linux ptrace(2) manual](https://man7.org/linux/man-pages/man2/ptrace.2.html).
The architectural instruction/setup encoding continues to use the S2-39j
references to Intel SDM Rev.093 and the S2-39d contract. This delivery does not
claim an additional Intel PDF review or physical execution.

## Evidence and comparison

The evidence keeps source/executable/table/byte/comparator/protocol SHA-256,
source revision, CPUID identity and raw leaf 1, kernel/OS, CPU affinity,
toolchain/build flags, virtualization indicator, actual code/input/observation/
image/guard addresses, mapping permissions, RAX, all raw image and guard bytes,
pre/post MOVDQU/STMXCSR bytes, raw MXCSR_MASK, exit status, marker signal code,
raw trapped RIP and adjusted E+5 boundary. Comparison ranges are carried in
the evidence and checked against the protocol:

- Compare MXCSR `[24,28)`, XMM `[160,416)`, and software tail `[464,512)`.
- Compare both pre/post observations and unchanged guards.
- Record MXCSR_MASK `[28,32)` raw without normalizing or asserting the synthetic
  target mask.
- Retain but exclude x87, reserved and unused ranges, including `[0,24)`,
  `[32,160)`, `[416,464)`, and byte padding.

The testkit validator rejects malformed byte lengths, hashes, case ordering,
missing repetitions, `not_run` evidence reported as passing, missing or
contradictory FXSR/SSE CPUID facts, noncanonical or overlapping code/input/
observation/image/guard spans, unexpected signals/exits, changed compared
bytes, guard corruption, invalid image/RAX or guard addresses, bad mapping
permissions, wrong INT3/RIP adjustment, MXCSR_MASK bytes that disagree with the
raw image, and altered comparison ranges.

Input addresses cover the four-byte MXCSR literal followed by the 256-byte XMM
literal table. Evidence gives separate addresses for the 256-byte pre/post XMM
observation arrays and the four-byte pre/post STMXCSR stores; the validator
checks each complete span for canonicality and pairwise overlap with code,
inputs, image, and guards.

## Verification status and limits

This host is Darwin arm64, so Linux runtime/ptrace execution and physical or
virtual hardware evidence remain `not_run`. The Linux x86_64 target `cargo
check` passes for all targets with
`cargo check -p od-x86-runner --all-targets --target x86_64-unknown-linux-gnu
--offline`. This check type-checks the runner and tests but does not link the
final runner binary. The former blocker was two `native_v2.rs` schema-v2
literals missing 142 optional fields each; they now explicitly use `None`.
`serde_json` 1.0.151 was already available and compiled for this target. A
current compiler raw-pointer lint in the S2-39k setup was satisfied with an
explicit mutable reference to the existing XMM input array field. The
independent `global_asm!` strings were extracted and assembled by Apple clang
for ELF x86_64; disassembly shows `48 0f ae 40 40` at E and `cc` at E+5. Final
Linux binary inspection, ptrace runtime, and hardware behavior remain
unverified.

No interpreter integration, native profile widening, FXRSTOR, privileged
gates/faults, JIT, or machine integration is added. Parent S2-39 remains open.
