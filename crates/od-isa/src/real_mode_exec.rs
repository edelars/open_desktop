//! Execution of the first four instruction groups of the AP trampoline in real mode
//! (S6-14d, S6-14e, S6-14f, S6-14g).
//!
//! Scope is the closed set of forms from the S6-14a profile (CPU_CONTRACT.md, sections
//! "S6-14d", "S6-14e", "S6-14f" and "S6-14g"), block 1 of the trampoline at blob offsets
//! `0x1000..0x1027` (through `MOV ESP, imm32` at `0x101e` and `CALL verify_cpu` at `0x1024`),
//! plus the forms of block 2 (`verify_cpu`, `0x1080..0x11a8`) and block 3 (`0x1027..0x102b`)
//! without the vendor-specific (Intel/AMD, `RDMSR`/`WRMSR`) branches:
//!
//! | bytes                    | instruction                  | slice  |
//! |--------------------------|------------------------------|--------|
//! | `fa`                     | `CLI`                        | S6-14d |
//! | `ea o o s s`             | `JMP ptr16:16` (far)         | S6-14d |
//! | `8c c8`                  | `MOV AX, CS`                 | S6-14d |
//! | `8e d8` / `8e c0` / `8e d0` | `MOV DS`/`ES`/`SS`, `AX`  | S6-14d |
//! | `0f 09`                  | `WBINVD` (no-op here)        | S6-14e |
//! | `66 f0 0f ba 2e d d i`   | `LOCK BTS m32, imm8` (DS:d16) | S6-14e |
//! | `73 cb`                  | `JAE rel8`                   | S6-14e |
//! | `f3 90`                  | `PAUSE`                      | S6-14e |
//! | `eb cb`                  | `JMP rel8`                   | S6-14e |
//! | `66 bc i i i i`          | `MOV ESP, imm32`             | S6-14f |
//! | `66 b8 i i i i`          | `MOV EAX, imm32`             | S6-14f |
//! | `e8 lo hi`               | `CALL rel16` (near)          | S6-14f |
//! | `c3`                     | `RET` (near)                 | S6-14f |
//! | `9c` / `9d`              | `PUSHF` / `POPF` (16-bit)    | S6-14f |
//! | `6a cb`                  | `PUSH imm8` (16-bit word)    | S6-14f |
//! | `66 31 c0`               | `XOR EAX, EAX`               | S6-14f |
//! | `66 85 c0`               | `TEST EAX, EAX`              | S6-14g |
//! | `75`/`74`/`72 cb`        | `JNE`/`JE`/`JB rel8`         | S6-14g |
//! | `0f 84 lo hi` / `0f 82 lo hi` | `JE`/`JB rel16`         | S6-14g |
//! | `66 9c` / `66 9d`        | `PUSHFD` / `POPFD`           | S6-14g |
//! | `66 50` / `66 58`        | `PUSH EAX` / `POP EAX`       | S6-14g |
//! | `66 89 c3`               | `MOV EBX, EAX`               | S6-14g |
//! | `66 39 c3`               | `CMP EBX, EAX`               | S6-14g |
//! | `66 35` / `66 3d imm32`  | `XOR` / `CMP EAX, imm32`     | S6-14g |
//! | `66 83 f8 ib`            | `CMP EAX, imm8` (sign-extended) | S6-14g |
//! | `66 81 fb` / `fa imm32`  | `CMP EBX` / `EDX, imm32`     | S6-14g |
//! | `66 81 e2` / `f2 imm32`  | `AND` / `XOR EDX, imm32`     | S6-14g |
//! | `31 ff`                  | `XOR DI, DI`                 | S6-14g |
//! | `0f a2`                  | `CPUID` (leaves 0, 1, `0x80000000`, `0x80000001`) | S6-14g |
//!
//! Everything else (`HLT`, the padding `NOP`, `RDMSR`/`WRMSR`, `BTR`, `TEST DI, DI`, any
//! other prefix byte or prefix order, any other ModRM form, `BT`/`BTR`/`BTC`, unlocked or
//! 16-bit `BTS`) is an explicit [`RealModeRefusal::Unsupported`] with the address and the
//! bytes, and the state is not changed. The Intel/AMD branches of `verify_cpu`
//! (`0x10bb..0x1126`, `0x1184..0x1199`) and block 4 belong to later slices.
//!
//! S6-14g `CPUID` is executed only for EAX in {0, 1, `0x80000000`, `0x80000001`}, from the
//! [`CpuidSource`] stored in the state (a profile of `cpuid.rs` by default); any other leaf is
//! [`RealModeRefusal::CpuidLeafUnmodelled`]. The values are never taken from the KVM oracle.
//! 32-bit stack words (`PUSHFD`, `POPFD`, `PUSH EAX`, `POP EAX`) use SP only, like the 16-bit
//! ones; the whole 4-byte word must lie under the SS limit. `POPFD` clears `RF` (SDM, from
//! memory) and refuses a word with `TF`, `IF`, `VM`, `VIF` or `VIP`.
//!
//! S6-14f stack: real mode, 16-bit stack size (the oracle reports `SS.B = 0`), so implicit
//! stack accesses use `SS.base + SP` and change SP only (the upper half of ESP is kept); a
//! word is never split across the SS limit and the offset does not wrap inside a word (a push
//! with `SP = 1` or a pop at `SP = 0xFFFF` is refused as [`RealModeRefusal::DataBeyondLimit`]).
//!
//! State: [`RealModeCpu`] wraps the S6-14c [`RealModeState`] (segments, EIP, EFLAGS, CR0)
//! and adds only the eight 32-bit general-purpose registers; `LongModeState` is not used
//! and not changed. Operand and address size are 16 bits except the `66`-prefixed forms:
//! `MOV ESP, imm32`, `MOV EAX, imm32`, `XOR EAX, EAX`, `TEST EAX, EAX`, `MOV EBX, EAX`,
//! `CMP`/`XOR`/`AND` with a 32-bit register operand (S6-14g), `PUSHFD`/`POPFD`, `PUSH EAX`/
//! `POP EAX` (32-bit stack word) and `CPUID` (register results), and `LOCK BTS` (32-bit
//! operand, 16-bit address). Instruction fetch goes through CS
//! (`CS.base + IP`, offset limit-checked). There is more than one kind of data operand: the
//! `LOCK BTS` operand goes through DS (`DS.base + disp16`, the whole 4-byte operand
//! limit-checked, no wrap), and the stack word of `CALL`/`RET`/`PUSH`/`POP`/`PUSHF`/`POPF`
//! goes through `SS.base + SP` (the whole word limit-checked against SS, no wrap), with no
//! paging and no A20 or wrap-around modelling. There are no caches: `WBINVD` changes nothing visible,
//! and `LOCK` adds no atomicity (a single processor is modelled).
//!
//! Sources: Intel SDM Vol. 2 (`CLI`, `JMP`, `Jcc`, `MOV` to/from segment registers, `BTS`,
//! `PAUSE`, `WBINVD`, `CPUID`, `TEST`, `CMP`, `PUSHFD`/`POPFD`; the `CPUID` leaf values come from
//! `CPU_CONTRACT.md`, the instruction semantics from the SDM) and Vol. 3A section 12.1.4 (real-mode segment base = selector << 4),
//! all from memory. That loading a segment register in real mode keeps the limit is also
//! from memory; its section number is not checked.

use crate::cpuid::{CpuidProfile, CpuidResult};
use crate::legacy_mode::{RealModeState, RealSegment};

/// Index of a 32-bit general-purpose register in [`RealModeCpu::gpr`] (x86 encoding order).
pub const EAX: usize = 0;
pub const ECX: usize = 1;
pub const EDX: usize = 2;
pub const EBX: usize = 3;
pub const ESP: usize = 4;
pub const EBP: usize = 5;
pub const ESI: usize = 6;
pub const EDI: usize = 7;

/// CR0.PE: real mode requires it to be clear.
pub const CR0_PE: u32 = 1;
/// EFLAGS.CF.
pub const EFLAGS_CF: u32 = 1;
/// EFLAGS.IF.
pub const EFLAGS_IF: u32 = 1 << 9;
/// EFLAGS.TF.
pub const EFLAGS_TF: u32 = 1 << 8;
/// EFLAGS.PF and ZF (the flags `XOR EAX, EAX` sets).
const EFLAGS_PF: u32 = 1 << 2;
const EFLAGS_AF: u32 = 1 << 4;
const EFLAGS_ZF: u32 = 1 << 6;
const EFLAGS_SF: u32 = 1 << 7;
const EFLAGS_OF: u32 = 1 << 11;
/// EFLAGS.RF: `POPFD` at CPL 0 clears it (SDM Vol. 2, from memory), whatever the popped word says.
const EFLAGS_RF: u32 = 1 << 16;
/// EFLAGS.VM, VIF, VIP: a `POPFD` word carrying one of them is refused.
const EFLAGS_VM_VIF_VIP: u32 = (1 << 17) | (1 << 19) | (1 << 20);
/// `PUSHFD` stores `EFLAGS AND 0x00FCFFFF` (VM and RF cleared in the image; SDM Vol. 2).
const EFLAGS_PUSHFD_MASK: u32 = 0x00FC_FFFF;
/// CF, PF, AF, ZF, SF, OF: the arithmetic flags rewritten by `XOR`.
const EFLAGS_ARITH: u32 = 0x8D5;
/// Bits a 16-bit `POPF` writes in real mode: CF, PF, AF, ZF, SF, TF, IF, DF, OF, IOPL, NT.
/// Bit 1 stays 1, bits 3, 5, 15 stay 0, the upper half (RF, VM, AC, ID) is untouched.
const EFLAGS_POPF_MASK: u32 = 0x7FD5;
/// Bits a 32-bit `POPFD` writes in real mode: the 16-bit set plus AC (bit 18) and ID (bit 21).
/// The upper reserved bits, VM, VIF and VIP are not written (see
/// [`RealModeRefusal::PopfdUnmodelledFlags`]); RF is cleared, not written.
const EFLAGS_POPFD_MASK: u32 = EFLAGS_POPF_MASK | (1 << 18) | (1 << 21);

/// Number of bytes copied into [`RealModeRefusal::Unsupported::bytes`] at most.
pub const UNSUPPORTED_BYTES_MAX: usize = 8;

/// Segment register loaded by `MOV Sreg, AX` in the profile (CS is not loadable this way:
/// `8E C8` is refused as `Unsupported`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SegReg {
    Ds,
    Es,
    Ss,
}

/// Condition of the S6-14g conditional jumps (`JNE`, `JE`, `JB`); `JAE` is the separate S6-14e
/// form.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Cond {
    /// `JNE`: `ZF = 0`.
    Ne,
    /// `JE`: `ZF = 1`.
    E,
    /// `JB`: `CF = 1`.
    B,
}

impl Cond {
    fn holds(self, eflags: u32) -> bool {
        match self {
            Self::Ne => eflags & EFLAGS_ZF == 0,
            Self::E => eflags & EFLAGS_ZF != 0,
            Self::B => eflags & EFLAGS_CF != 0,
        }
    }
}

/// Operation of the S6-14g `reg32, imm` forms.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Alu32Op {
    /// `XOR`: logical flags, writes the destination.
    Xor,
    /// `AND`: logical flags, writes the destination.
    And,
    /// `CMP`: subtraction flags, no destination write.
    Cmp,
}

/// Where `CPUID` takes its values from: a profile of `cpuid.rs`, or an explicit table of
/// leaves (every leaf not in the table reads as four zero registers). The KVM oracle is never
/// a source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CpuidSource {
    Profile(CpuidProfile),
    Leaves(&'static [(u32, CpuidResult)]),
}

impl CpuidSource {
    fn query(self, leaf: u32) -> CpuidResult {
        match self {
            Self::Profile(profile) => profile.query(leaf, 0),
            Self::Leaves(table) => table
                .iter()
                .find(|(l, _)| *l == leaf)
                .map(|(_, r)| *r)
                .unwrap_or_default(),
        }
    }
}

/// Leaves `CPUID` executes in this slice (the leaves `verify_cpu` reads on the profile path).
const CPUID_LEAVES: [u32; 4] = [0, 1, 0x8000_0000, 0x8000_0001];

/// Decoded instruction of the first four groups (S6-14d, S6-14e, S6-14f, S6-14g).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RealInstruction {
    /// `FA`.
    Cli,
    /// `EA` offset16 selector16: `JMP ptr16:16`.
    JmpFar { selector: u16, offset: u16 },
    /// `8C C8`: `MOV AX, CS` (only AX is written).
    MovAxFromCs,
    /// `8E D8` / `8E C0` / `8E D0`: `MOV DS|ES|SS, AX`.
    MovSegFromAx(SegReg),
    /// `0F 09` (S6-14e): `WBINVD`. No visible effect in a model without caches.
    Wbinvd,
    /// `66 F0 0F BA 2E d_lo d_hi imm8` (S6-14e): `LOCK BTS DWORD PTR ds:disp16, imm8`. The bit
    /// index is `imm8 & 31`; `imm8` is kept as encoded.
    LockBtsImm8 { disp: u16, imm8: u8 },
    /// `73 cb` (S6-14e): `JAE rel8`, jump if `CF = 0`.
    Jae { rel: i8 },
    /// `F3 90` (S6-14e): `PAUSE`.
    Pause,
    /// `EB cb` (S6-14e): `JMP rel8`.
    JmpShort { rel: i8 },
    /// `66 B8 imm32` / `66 BC imm32` (S6-14f): `MOV EAX|ESP, imm32` (`reg` is [`EAX`] or
    /// [`ESP`]). No flags change.
    MovR32Imm32 { reg: usize, imm: u32 },
    /// `E8 lo hi` (S6-14f): near `CALL rel16`.
    CallNear { rel: i16 },
    /// `C3` (S6-14f): near `RET`.
    RetNear,
    /// `9C` (S6-14f): 16-bit `PUSHF`.
    Pushf,
    /// `9D` (S6-14f): 16-bit `POPF`.
    Popf,
    /// `6A cb` (S6-14f): `PUSH imm8`, sign-extended to a 16-bit word.
    PushImm8 { imm: i8 },
    /// `66 31 C0` (S6-14f): `XOR EAX, EAX`.
    XorEaxEax,
    /// `66 85 C0` (S6-14g): `TEST EAX, EAX`. Flags only.
    TestEaxEax,
    /// `75 cb` / `74 cb` / `72 cb` (S6-14g): `JNE` / `JE` / `JB rel8`.
    JccShort { cond: Cond, rel: i8 },
    /// `0F 84 lo hi` / `0F 82 lo hi` (S6-14g): `JE` / `JB rel16`.
    JccNear { cond: Cond, rel: i16 },
    /// `66 9C` (S6-14g): `PUSHFD`.
    Pushfd,
    /// `66 9D` (S6-14g): `POPFD`.
    Popfd,
    /// `66 50` (S6-14g): `PUSH EAX`.
    PushEax,
    /// `66 58` (S6-14g): `POP EAX`.
    PopEax,
    /// `66 89 C3` (S6-14g): `MOV EBX, EAX`.
    MovEbxEax,
    /// `66 39 C3` (S6-14g): `CMP EBX, EAX` (flags of `EBX - EAX`).
    CmpEbxEax,
    /// `66 35`, `66 3D`, `66 81 /4 /6 /7`, `66 83 /7` (S6-14g): `XOR`/`AND`/`CMP reg32, imm`
    /// for the listed (operation, register) pairs only. `imm` is the sign-extended immediate.
    Alu32Imm { op: Alu32Op, reg: usize, imm: u32 },
    /// `31 FF` (S6-14g): `XOR DI, DI` (only DI is written).
    XorDiDi,
    /// `0F A2` (S6-14g): `CPUID`.
    Cpuid,
}

/// Memory for instruction fetch and the data operands (the `LOCK BTS` operand and the stack
/// word of `CALL`/`RET`/`PUSH`/`POP`/`PUSHF`/`POPF`): physical (= linear, no paging) address.
pub trait RealModeMemory {
    /// One byte at `linear`, or `None` if the address is not readable.
    fn read_u8(&self, linear: u32) -> Option<u8>;
    /// Whether [`Self::write_u8`] would succeed at `linear`. A multi-byte write checks every
    /// byte first, so a refused write leaves memory unchanged.
    fn can_write(&self, linear: u32) -> bool;
    /// Writes one byte. Returns `false` (and writes nothing) when `linear` is not writable.
    fn write_u8(&mut self, linear: u32, value: u8) -> bool;
}

/// Read-only memory made of one byte image placed at `base`; everything else is unreadable,
/// and nothing is writable.
#[derive(Clone, Copy, Debug)]
pub struct ImageMemory<'a> {
    pub base: u32,
    pub bytes: &'a [u8],
}

impl RealModeMemory for ImageMemory<'_> {
    fn read_u8(&self, linear: u32) -> Option<u8> {
        let index = linear.checked_sub(self.base)? as usize;
        self.bytes.get(index).copied()
    }

    fn can_write(&self, _linear: u32) -> bool {
        false
    }

    fn write_u8(&mut self, _linear: u32, _value: u8) -> bool {
        false
    }
}

/// Readable and writable memory made of one byte image placed at `base`.
#[derive(Debug)]
pub struct ImageMemoryMut<'a> {
    pub base: u32,
    pub bytes: &'a mut [u8],
}

impl ImageMemoryMut<'_> {
    fn index(&self, linear: u32) -> Option<usize> {
        let index = linear.checked_sub(self.base)? as usize;
        (index < self.bytes.len()).then_some(index)
    }
}

impl RealModeMemory for ImageMemoryMut<'_> {
    fn read_u8(&self, linear: u32) -> Option<u8> {
        self.index(linear).map(|i| self.bytes[i])
    }

    fn can_write(&self, linear: u32) -> bool {
        self.index(linear).is_some()
    }

    fn write_u8(&mut self, linear: u32, value: u8) -> bool {
        match self.index(linear) {
            Some(i) => {
                self.bytes[i] = value;
                true
            }
            None => false,
        }
    }
}

/// Why a step did not execute. The processor state is unchanged in every case.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RealModeRefusal {
    /// The instruction is outside the admitted forms. `bytes` are the (up to
    /// [`UNSUPPORTED_BYTES_MAX`]) readable bytes at the address.
    Unsupported {
        cs: u16,
        ip: u32,
        linear: u32,
        bytes: Vec<u8>,
    },
    /// An instruction byte lies beyond the CS limit.
    FetchBeyondLimit { cs: u16, ip: u32, offset: u32 },
    /// An instruction byte is not readable in memory: `cs:ip` is the start of the
    /// instruction, `linear` the address of the unreadable byte.
    FetchUnreadable { cs: u16, ip: u32, linear: u32 },
    /// The target offset of a far jump, a taken short jump, a `CALL` or a `RET` is greater than
    /// the CS limit.
    TargetBeyondLimit { offset: u32, limit: u32 },
    /// The 4-byte data operand of `LOCK BTS` does not fit under the DS limit, or the stack word
    /// of `PUSH`/`POP`/`CALL`/`RET`/`PUSHF`/`POPF` does not fit under the SS limit: `offset` is
    /// the offset of the first byte beyond it (real mode does not wrap the operand).
    DataBeyondLimit { cs: u16, ip: u32, offset: u32 },
    /// A data byte at `linear` is not readable in memory.
    DataUnreadable { cs: u16, ip: u32, linear: u32 },
    /// A data byte at `linear` is not writable in memory.
    DataUnwritable { cs: u16, ip: u32, linear: u32 },
    /// CR0.PE is set: this module executes real mode only.
    NotRealMode { cr0: u32 },
    /// `POPF` would set `EFLAGS.TF` or `EFLAGS.IF` (`flags` is the 16-bit word on the stack).
    /// Trap and interrupt enabling are not modelled in this slice, so the pop is refused.
    PopfUnmodelledFlags { cs: u16, ip: u32, flags: u16 },
    /// `POPFD` would set `TF` or `IF`, or the word carries `VM`, `VIF` or `VIP`
    /// (`flags` is the 32-bit word on the stack). None of these is modelled in this slice.
    /// A word carrying `RF` is not refused: `POPFD` clears `RF`.
    PopfdUnmodelledFlags { cs: u16, ip: u32, flags: u32 },
    /// `CPUID` with an EAX outside {0, 1, `0x80000000`, `0x80000001`}.
    CpuidLeafUnmodelled { cs: u16, ip: u32, leaf: u32 },
}

/// Result of [`RealModeCpu::run`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RealRun {
    /// Number of instructions executed.
    pub steps: u64,
    /// `None` when `max_steps` was reached, otherwise the refusal that stopped the run.
    pub stop: Option<RealModeRefusal>,
}

/// Flags of a logical result (`AND`, `XOR`, `TEST`): `CF = OF = 0`; `SF`, `ZF`, `PF` follow the
/// result; `AF` is undefined by the SDM and the model clears it. Only the six arithmetic bits
/// are returned.
fn logic_flags(result: u32) -> u32 {
    let mut flags = 0;
    if result == 0 {
        flags |= EFLAGS_ZF;
    }
    if result >> 31 != 0 {
        flags |= EFLAGS_SF;
    }
    if (result as u8).count_ones().is_multiple_of(2) {
        flags |= EFLAGS_PF;
    }
    flags
}

/// Flags of the 32-bit subtraction `lhs - rhs` (`CMP`): `CF` = borrow, `OF` = signed overflow,
/// `AF` = borrow out of bit 3, `SF`/`ZF`/`PF` from the result (SDM Vol. 1 appendix A, by
/// memory). Only the six arithmetic bits are returned.
fn sub_flags(lhs: u32, rhs: u32) -> u32 {
    let result = lhs.wrapping_sub(rhs);
    let mut flags = logic_flags(result);
    if lhs < rhs {
        flags |= EFLAGS_CF;
    }
    if (lhs ^ rhs ^ result) & 0x10 != 0 {
        flags |= EFLAGS_AF;
    }
    if ((lhs ^ rhs) & (lhs ^ result)) >> 31 != 0 {
        flags |= EFLAGS_OF;
    }
    flags
}

/// Real-mode processor state for this slice: S6-14c [`RealModeState`] plus the
/// general-purpose registers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RealModeCpu {
    pub mode: RealModeState,
    /// EAX, ECX, EDX, EBX, ESP, EBP, ESI, EDI (see the index constants).
    pub gpr: [u32; 8],
    /// Set by `MOV SS, AX`: interrupts (and NMI/debug traps) are inhibited until after the
    /// next instruction (SDM Vol. 3A, "MOV to SS"). It is cleared after the next retired
    /// instruction. Nothing in this slice delivers an interrupt, so it only records state
    /// for a later scheduler; it is not compared with the KVM oracle.
    pub mov_ss_shadow: bool,
    /// Source of the `CPUID` values (S6-14g); [`CpuidProfile::DiagnosticV1`] unless the caller
    /// replaces it. The profiles of `cpuid.rs` do not implement the full target table of
    /// `CPU_CONTRACT.md` (see the S6-14g contract section).
    pub cpuid: CpuidSource,
}

impl RealModeCpu {
    /// State after INIT and a SIPI with `vector`, with all registers zero except `EDX`,
    /// which holds the processor signature (`edx_signature`) that INIT preserves.
    pub const fn after_sipi(vector: u8, old_cr0: u32, edx_signature: u32) -> Self {
        let mut gpr = [0u32; 8];
        gpr[EDX] = edx_signature;
        Self {
            mode: RealModeState::after_init(old_cr0).after_sipi(vector),
            gpr,
            mov_ss_shadow: false,
            cpuid: CpuidSource::Profile(CpuidProfile::DiagnosticV1),
        }
    }

    /// State with an explicit CS:IP and the reset state elsewhere (the entry the KVM oracle
    /// `--real-mode CS:IP` creates): CS base `selector << 4`, DS/ES/FS/GS/SS zero.
    pub const fn at_cs_ip(cs: u16, ip: u16, old_cr0: u32, edx_signature: u32) -> Self {
        let mut cpu = Self::after_sipi(0, old_cr0, edx_signature);
        cpu.mode.cs = RealSegment {
            selector: cs,
            base: (cs as u32) << 4,
            limit: cpu.mode.cs.limit,
        };
        cpu.mode.eip = ip as u32;
        cpu
    }

    /// Reads up to `n` bytes at the current `CS:IP` for a refusal, stopping at the first
    /// byte that is not readable or beyond the CS limit.
    fn refusal_bytes<M: RealModeMemory>(&self, mem: &M) -> Vec<u8> {
        let mut out = Vec::new();
        for i in 0..UNSUPPORTED_BYTES_MAX as u32 {
            let offset = self.mode.eip.wrapping_add(i);
            if offset > self.mode.cs.limit {
                break;
            }
            match mem.read_u8(self.mode.cs.base.wrapping_add(offset)) {
                Some(b) => out.push(b),
                None => break,
            }
        }
        out
    }

    fn unsupported<M: RealModeMemory>(&self, mem: &M) -> RealModeRefusal {
        RealModeRefusal::Unsupported {
            cs: self.mode.cs.selector,
            ip: self.mode.eip,
            linear: self.mode.fetch_linear_address(),
            bytes: self.refusal_bytes(mem),
        }
    }

    fn fetch<M: RealModeMemory>(&self, mem: &M, i: u32) -> Result<u8, RealModeRefusal> {
        let offset = self.mode.eip.wrapping_add(i);
        if offset > self.mode.cs.limit {
            return Err(RealModeRefusal::FetchBeyondLimit {
                cs: self.mode.cs.selector,
                ip: self.mode.eip,
                offset,
            });
        }
        let linear = self.mode.cs.base.wrapping_add(offset);
        mem.read_u8(linear).ok_or(RealModeRefusal::FetchUnreadable {
            cs: self.mode.cs.selector,
            ip: self.mode.eip,
            linear,
        })
    }

    /// Little-endian 32-bit value at instruction offsets `i..i + 4`.
    fn fetch_u32<M: RealModeMemory>(&self, mem: &M, i: u32) -> Result<u32, RealModeRefusal> {
        Ok(u32::from_le_bytes([
            self.fetch(mem, i)?,
            self.fetch(mem, i + 1)?,
            self.fetch(mem, i + 2)?,
            self.fetch(mem, i + 3)?,
        ]))
    }

    /// Decodes the instruction at `CS:IP`. Returns it with its length. A byte that begins
    /// no admitted form (including any prefix) is [`RealModeRefusal::Unsupported`].
    pub fn decode<M: RealModeMemory>(
        &self,
        mem: &M,
    ) -> Result<(RealInstruction, u32), RealModeRefusal> {
        match self.fetch(mem, 0)? {
            0xFA => Ok((RealInstruction::Cli, 1)),
            0xEA => {
                let lo = self.fetch(mem, 1)?;
                let hi = self.fetch(mem, 2)?;
                let slo = self.fetch(mem, 3)?;
                let shi = self.fetch(mem, 4)?;
                Ok((
                    RealInstruction::JmpFar {
                        selector: u16::from_le_bytes([slo, shi]),
                        offset: u16::from_le_bytes([lo, hi]),
                    },
                    5,
                ))
            }
            0x8C => match self.fetch(mem, 1)? {
                0xC8 => Ok((RealInstruction::MovAxFromCs, 2)),
                _ => Err(self.unsupported(mem)),
            },
            0x8E => match self.fetch(mem, 1)? {
                0xD8 => Ok((RealInstruction::MovSegFromAx(SegReg::Ds), 2)),
                0xC0 => Ok((RealInstruction::MovSegFromAx(SegReg::Es), 2)),
                0xD0 => Ok((RealInstruction::MovSegFromAx(SegReg::Ss), 2)),
                _ => Err(self.unsupported(mem)),
            },
            0x0F => match self.fetch(mem, 1)? {
                0x09 => Ok((RealInstruction::Wbinvd, 2)),
                0xA2 => Ok((RealInstruction::Cpuid, 2)),
                op @ (0x82 | 0x84) => {
                    let lo = self.fetch(mem, 2)?;
                    let hi = self.fetch(mem, 3)?;
                    let cond = if op == 0x82 { Cond::B } else { Cond::E };
                    Ok((
                        RealInstruction::JccNear {
                            cond,
                            rel: i16::from_le_bytes([lo, hi]),
                        },
                        4,
                    ))
                }
                _ => Err(self.unsupported(mem)),
            },
            // `31 FF`: XOR DI, DI (ModRM FF = mod 11, reg DI, rm DI).
            0x31 => match self.fetch(mem, 1)? {
                0xFF => Ok((RealInstruction::XorDiDi, 2)),
                _ => Err(self.unsupported(mem)),
            },
            op @ (0x72 | 0x74 | 0x75) => {
                let cond = match op {
                    0x72 => Cond::B,
                    0x74 => Cond::E,
                    _ => Cond::Ne,
                };
                Ok((
                    RealInstruction::JccShort {
                        cond,
                        rel: self.fetch(mem, 1)? as i8,
                    },
                    2,
                ))
            }
            0xE8 => {
                let lo = self.fetch(mem, 1)?;
                let hi = self.fetch(mem, 2)?;
                Ok((
                    RealInstruction::CallNear {
                        rel: i16::from_le_bytes([lo, hi]),
                    },
                    3,
                ))
            }
            0xC3 => Ok((RealInstruction::RetNear, 1)),
            0x9C => Ok((RealInstruction::Pushf, 1)),
            0x9D => Ok((RealInstruction::Popf, 1)),
            0x6A => Ok((
                RealInstruction::PushImm8 {
                    imm: self.fetch(mem, 1)? as i8,
                },
                2,
            )),
            // Prefixed forms: `66 F0 0F BA /5 ib` (S6-14e) and the S6-14f `66`-prefixed
            // register forms. Any other byte after `66` is refused.
            0x66 => match self.fetch(mem, 1)? {
                // `66 F0 0F BA /5 ib` with ModRM `2E` (mod 00, rm 110: disp16 only, reg 5 =
                // BTS), in exactly this prefix order. Operand bytes are free.
                0xF0 => {
                    let modrm_ok = self.fetch(mem, 2)? == 0x0F
                        && self.fetch(mem, 3)? == 0xBA
                        && self.fetch(mem, 4)? == 0x2E;
                    if !modrm_ok {
                        return Err(self.unsupported(mem));
                    }
                    let lo = self.fetch(mem, 5)?;
                    let hi = self.fetch(mem, 6)?;
                    let imm8 = self.fetch(mem, 7)?;
                    Ok((
                        RealInstruction::LockBtsImm8 {
                            disp: u16::from_le_bytes([lo, hi]),
                            imm8,
                        },
                        8,
                    ))
                }
                // `66 B8 imm32` (EAX) and `66 BC imm32` (ESP): the only register targets in
                // the profile.
                op @ (0xB8 | 0xBC) => {
                    let imm = u32::from_le_bytes([
                        self.fetch(mem, 2)?,
                        self.fetch(mem, 3)?,
                        self.fetch(mem, 4)?,
                        self.fetch(mem, 5)?,
                    ]);
                    let reg = if op == 0xB8 { EAX } else { ESP };
                    Ok((RealInstruction::MovR32Imm32 { reg, imm }, 6))
                }
                // `66 31 C0`: XOR EAX, EAX (ModRM C0 = mod 11, reg EAX, rm EAX).
                0x31 => match self.fetch(mem, 2)? {
                    0xC0 => Ok((RealInstruction::XorEaxEax, 3)),
                    _ => Err(self.unsupported(mem)),
                },
                0x9C => Ok((RealInstruction::Pushfd, 2)),
                0x9D => Ok((RealInstruction::Popfd, 2)),
                0x50 => Ok((RealInstruction::PushEax, 2)),
                0x58 => Ok((RealInstruction::PopEax, 2)),
                // `66 85 C0`: TEST EAX, EAX.
                0x85 => match self.fetch(mem, 2)? {
                    0xC0 => Ok((RealInstruction::TestEaxEax, 3)),
                    _ => Err(self.unsupported(mem)),
                },
                // `66 89 C3`: MOV EBX, EAX (89 /r: r/m32 = EBX, reg = EAX).
                0x89 => match self.fetch(mem, 2)? {
                    0xC3 => Ok((RealInstruction::MovEbxEax, 3)),
                    _ => Err(self.unsupported(mem)),
                },
                // `66 39 C3`: CMP EBX, EAX (39 /r: r/m32 = EBX, reg = EAX).
                0x39 => match self.fetch(mem, 2)? {
                    0xC3 => Ok((RealInstruction::CmpEbxEax, 3)),
                    _ => Err(self.unsupported(mem)),
                },
                // `66 35 imm32` XOR EAX and `66 3D imm32` CMP EAX.
                op @ (0x35 | 0x3D) => {
                    let imm = self.fetch_u32(mem, 2)?;
                    let op = if op == 0x35 {
                        Alu32Op::Xor
                    } else {
                        Alu32Op::Cmp
                    };
                    Ok((RealInstruction::Alu32Imm { op, reg: EAX, imm }, 6))
                }
                // `66 83 F8 ib`: CMP EAX, imm8 (83 /7, sign-extended).
                0x83 => match self.fetch(mem, 2)? {
                    0xF8 => {
                        let imm = self.fetch(mem, 3)? as i8;
                        Ok((
                            RealInstruction::Alu32Imm {
                                op: Alu32Op::Cmp,
                                reg: EAX,
                                imm: i32::from(imm) as u32,
                            },
                            4,
                        ))
                    }
                    _ => Err(self.unsupported(mem)),
                },
                // `66 81 /r imm32`: only CMP EBX (FB), CMP EDX (FA), AND EDX (E2), XOR EDX (F2).
                0x81 => {
                    let (op, reg) = match self.fetch(mem, 2)? {
                        0xFB => (Alu32Op::Cmp, EBX),
                        0xFA => (Alu32Op::Cmp, EDX),
                        0xE2 => (Alu32Op::And, EDX),
                        0xF2 => (Alu32Op::Xor, EDX),
                        _ => return Err(self.unsupported(mem)),
                    };
                    let imm = self.fetch_u32(mem, 3)?;
                    Ok((RealInstruction::Alu32Imm { op, reg, imm }, 7))
                }
                _ => Err(self.unsupported(mem)),
            },
            0x73 => Ok((
                RealInstruction::Jae {
                    rel: self.fetch(mem, 1)? as i8,
                },
                2,
            )),
            0xEB => Ok((
                RealInstruction::JmpShort {
                    rel: self.fetch(mem, 1)? as i8,
                },
                2,
            )),
            0xF3 => match self.fetch(mem, 1)? {
                0x90 => Ok((RealInstruction::Pause, 2)),
                _ => Err(self.unsupported(mem)),
            },
            _ => Err(self.unsupported(mem)),
        }
    }

    /// Target offset of a short jump whose next instruction is at `next_eip`: 16-bit operand
    /// size, so the sum is truncated to 16 bits (SDM Vol. 2, `Jcc`/`JMP`, by memory). An
    /// unwrapped `next_eip` of `0x10000` (see the contract) is therefore taken as 0.
    fn short_target(next_eip: u32, rel: i8) -> u32 {
        Self::near_target(next_eip, i32::from(rel))
    }

    /// Same for a 16-bit relative displacement (`CALL rel16`).
    fn near_target(next_eip: u32, rel: i32) -> u32 {
        next_eip.wrapping_add_signed(rel) & 0xFFFF
    }

    /// Decodes and executes one instruction. On refusal the state and the memory are not
    /// changed.
    pub fn step<M: RealModeMemory>(
        &mut self,
        mem: &mut M,
    ) -> Result<RealInstruction, RealModeRefusal> {
        if self.mode.cr0 & CR0_PE != 0 {
            return Err(RealModeRefusal::NotRealMode { cr0: self.mode.cr0 });
        }
        let (insn, len) = self.decode(mem)?;
        let shadow_before = self.mov_ss_shadow;
        let next_eip = self.mode.eip.wrapping_add(len);
        // Everything that can refuse is checked before any state or memory change.
        let branch_target = match insn {
            RealInstruction::JmpFar { offset, .. } => Some(u32::from(offset)),
            RealInstruction::JmpShort { rel } => Some(Self::short_target(next_eip, rel)),
            RealInstruction::CallNear { rel } => Some(Self::near_target(next_eip, i32::from(rel))),
            RealInstruction::Jae { rel } if self.mode.eflags & EFLAGS_CF == 0 => {
                Some(Self::short_target(next_eip, rel))
            }
            RealInstruction::JccShort { cond, rel } if cond.holds(self.mode.eflags) => {
                Some(Self::short_target(next_eip, rel))
            }
            RealInstruction::JccNear { cond, rel } if cond.holds(self.mode.eflags) => {
                Some(Self::near_target(next_eip, i32::from(rel)))
            }
            _ => None,
        };
        if let Some(offset) = branch_target
            && offset > self.mode.cs.limit
        {
            return Err(RealModeRefusal::TargetBeyondLimit {
                offset,
                limit: self.mode.cs.limit,
            });
        }
        // Stack accesses (S6-14f): checked in full before anything is written.
        let push_at = match insn {
            RealInstruction::CallNear { .. }
            | RealInstruction::Pushf
            | RealInstruction::PushImm8 { .. } => Some(self.check_stack_push(mem, 2)?),
            RealInstruction::Pushfd | RealInstruction::PushEax => {
                Some(self.check_stack_push(mem, 4)?)
            }
            _ => None,
        };
        let popped = match insn {
            RealInstruction::RetNear | RealInstruction::Popf => Some(self.check_stack_pop(mem, 2)?),
            RealInstruction::Popfd | RealInstruction::PopEax => Some(self.check_stack_pop(mem, 4)?),
            _ => None,
        };
        if matches!(insn, RealInstruction::RetNear) {
            let offset = popped.expect("RET pops a word");
            if offset > self.mode.cs.limit {
                return Err(RealModeRefusal::TargetBeyondLimit {
                    offset,
                    limit: self.mode.cs.limit,
                });
            }
        }
        if matches!(insn, RealInstruction::Popf) {
            let flags = popped.expect("POPF pops a word") as u16;
            if u32::from(flags) & (EFLAGS_TF | EFLAGS_IF) != 0 {
                return Err(RealModeRefusal::PopfUnmodelledFlags {
                    cs: self.mode.cs.selector,
                    ip: self.mode.eip,
                    flags,
                });
            }
        }
        if matches!(insn, RealInstruction::Popfd) {
            let flags = popped.expect("POPFD pops a dword");
            if flags & (EFLAGS_TF | EFLAGS_IF | EFLAGS_VM_VIF_VIP) != 0 {
                return Err(RealModeRefusal::PopfdUnmodelledFlags {
                    cs: self.mode.cs.selector,
                    ip: self.mode.eip,
                    flags,
                });
            }
        }
        if matches!(insn, RealInstruction::Cpuid) && !CPUID_LEAVES.contains(&self.gpr[EAX]) {
            return Err(RealModeRefusal::CpuidLeafUnmodelled {
                cs: self.mode.cs.selector,
                ip: self.mode.eip,
                leaf: self.gpr[EAX],
            });
        }
        let bts_operand = match insn {
            RealInstruction::LockBtsImm8 { disp, .. } => Some(self.check_dword_operand(mem, disp)?),
            _ => None,
        };
        match insn {
            RealInstruction::Cli => {
                // Real mode (CR0.PE = 0): IF is cleared unconditionally.
                self.mode.eflags &= !EFLAGS_IF;
                self.mode.eip = next_eip;
            }
            RealInstruction::JmpFar { selector, offset } => {
                // Real mode: base = selector << 4, limit and attributes unchanged.
                self.mode.cs = RealSegment {
                    selector,
                    base: u32::from(selector) << 4,
                    limit: self.mode.cs.limit,
                };
                self.mode.eip = u32::from(offset);
            }
            RealInstruction::Wbinvd | RealInstruction::Pause => {
                // `WBINVD` writes back and invalidates caches; no cache is modelled, so memory
                // and registers are unaffected. `PAUSE` is a hint (a NOP-like delay).
                self.mode.eip = next_eip;
            }
            RealInstruction::LockBtsImm8 { imm8, .. } => {
                // Read-modify-write of a 32-bit word (little-endian). The bit index is
                // `imm8 mod 32` (the immediate form never reaches beyond the operand). CF =
                // the old bit; OF, SF, AF, PF are undefined by the SDM and ZF is unaffected:
                // the model leaves all of them as they were. The write happens even when the
                // bit was already set. `LOCK` adds nothing on a single processor.
                let (linear, old) = bts_operand.expect("operand checked for LOCK BTS");
                let mask = 1u32 << (imm8 & 31);
                for (i, byte) in (old | mask).to_le_bytes().into_iter().enumerate() {
                    let ok = mem.write_u8(linear.wrapping_add(i as u32), byte);
                    debug_assert!(ok, "writability was checked before the update");
                }
                let cf = u32::from(old & mask != 0);
                self.mode.eflags = (self.mode.eflags & !EFLAGS_CF) | cf;
                self.mode.eip = next_eip;
            }
            RealInstruction::Jae { .. } | RealInstruction::JmpShort { .. } => {
                // Taken (`branch_target` set) or falling through: JAE jumps when CF = 0.
                self.mode.eip = branch_target.unwrap_or(next_eip);
            }
            RealInstruction::MovR32Imm32 { reg, imm } => {
                self.gpr[reg] = imm;
                self.mode.eip = next_eip;
            }
            RealInstruction::XorEaxEax => {
                // Result zero: CF = OF = SF = 0, ZF = PF = 1. AF is undefined by the SDM; the
                // model clears it (the oracle host did the same, see the contract).
                self.gpr[EAX] = 0;
                self.mode.eflags = (self.mode.eflags & !EFLAGS_ARITH) | EFLAGS_ZF | EFLAGS_PF;
                self.mode.eip = next_eip;
            }
            RealInstruction::CallNear { .. }
            | RealInstruction::Pushf
            | RealInstruction::PushImm8 { .. } => {
                let word = match insn {
                    // The return address is the 16-bit IP of the next instruction.
                    RealInstruction::CallNear { .. } => (next_eip & 0xFFFF) as u16,
                    RealInstruction::Pushf => self.mode.eflags as u16,
                    RealInstruction::PushImm8 { imm } => i16::from(imm) as u16,
                    _ => unreachable!(),
                };
                let (new_sp, linear) = push_at.expect("stack slot checked");
                for (i, byte) in word.to_le_bytes().into_iter().enumerate() {
                    let ok = mem.write_u8(linear.wrapping_add(i as u32), byte);
                    debug_assert!(ok, "writability was checked before the push");
                }
                self.set_sp(new_sp);
                self.mode.eip = branch_target.unwrap_or(next_eip);
            }
            RealInstruction::RetNear => {
                let ip = popped.expect("RET pops a word");
                self.set_sp((self.gpr[ESP] as u16).wrapping_add(2));
                self.mode.eip = ip;
            }
            RealInstruction::Popf => {
                let flags = popped.expect("POPF pops a word");
                self.set_sp((self.gpr[ESP] as u16).wrapping_add(2));
                self.mode.eflags =
                    (self.mode.eflags & !EFLAGS_POPF_MASK) | (flags & EFLAGS_POPF_MASK);
                self.mode.eip = next_eip;
            }
            RealInstruction::Pushfd | RealInstruction::PushEax => {
                // The 32-bit word is `EFLAGS AND 0x00FCFFFF` (VM and RF cleared in the image)
                // or EAX; SP changes by 4, the upper half of ESP is kept.
                let word = match insn {
                    RealInstruction::Pushfd => self.mode.eflags & EFLAGS_PUSHFD_MASK,
                    _ => self.gpr[EAX],
                };
                let (new_sp, linear) = push_at.expect("stack slot checked");
                for (i, byte) in word.to_le_bytes().into_iter().enumerate() {
                    let ok = mem.write_u8(linear.wrapping_add(i as u32), byte);
                    debug_assert!(ok, "writability was checked before the push");
                }
                self.set_sp(new_sp);
                self.mode.eip = next_eip;
            }
            RealInstruction::Popfd => {
                let flags = popped.expect("POPFD pops a dword");
                self.set_sp((self.gpr[ESP] as u16).wrapping_add(4));
                // RF is cleared whatever it was and whatever the word says; VM, VIF, VIP and the
                // reserved bits outside the mask keep their values.
                self.mode.eflags = ((self.mode.eflags & !EFLAGS_POPFD_MASK)
                    | (flags & EFLAGS_POPFD_MASK))
                    & !EFLAGS_RF;
                self.mode.eip = next_eip;
            }
            RealInstruction::PopEax => {
                self.gpr[EAX] = popped.expect("POP EAX pops a dword");
                self.set_sp((self.gpr[ESP] as u16).wrapping_add(4));
                self.mode.eip = next_eip;
            }
            RealInstruction::TestEaxEax => {
                // Logical flags of EAX & EAX; AF is undefined by the SDM and cleared.
                self.set_arith_flags(logic_flags(self.gpr[EAX]));
                self.mode.eip = next_eip;
            }
            RealInstruction::JccShort { .. } | RealInstruction::JccNear { .. } => {
                self.mode.eip = branch_target.unwrap_or(next_eip);
            }
            RealInstruction::MovEbxEax => {
                self.gpr[EBX] = self.gpr[EAX];
                self.mode.eip = next_eip;
            }
            RealInstruction::CmpEbxEax => {
                self.set_arith_flags(sub_flags(self.gpr[EBX], self.gpr[EAX]));
                self.mode.eip = next_eip;
            }
            RealInstruction::Alu32Imm { op, reg, imm } => {
                let lhs = self.gpr[reg];
                match op {
                    Alu32Op::Xor | Alu32Op::And => {
                        let result = if op == Alu32Op::Xor {
                            lhs ^ imm
                        } else {
                            lhs & imm
                        };
                        self.gpr[reg] = result;
                        self.set_arith_flags(logic_flags(result));
                    }
                    Alu32Op::Cmp => self.set_arith_flags(sub_flags(lhs, imm)),
                }
                self.mode.eip = next_eip;
            }
            RealInstruction::XorDiDi => {
                // 16-bit result zero: only DI is written (EDI[31:16] kept).
                self.gpr[EDI] &= 0xFFFF_0000;
                self.set_arith_flags(logic_flags(0));
                self.mode.eip = next_eip;
            }
            RealInstruction::Cpuid => {
                let r = self.cpuid.query(self.gpr[EAX]);
                self.gpr[EAX] = r.eax;
                self.gpr[EBX] = r.ebx;
                self.gpr[ECX] = r.ecx;
                self.gpr[EDX] = r.edx;
                self.mode.eip = next_eip;
            }
            RealInstruction::MovAxFromCs => {
                // 16-bit operand: only AX is written, the upper half of EAX is kept. This is
                // from memory, not confirmed by the oracle (its input EAX is 0); the SDM note
                // on upper bits for a 16-bit operand is ambiguous. An oracle run with a
                // non-zero input EAX is a future option.
                self.gpr[EAX] = (self.gpr[EAX] & 0xFFFF_0000) | u32::from(self.mode.cs.selector);
                self.mode.eip = next_eip;
            }
            RealInstruction::MovSegFromAx(seg) => {
                let selector = self.gpr[EAX] as u16;
                let slot = match seg {
                    SegReg::Ds => &mut self.mode.ds,
                    SegReg::Es => &mut self.mode.es,
                    SegReg::Ss => &mut self.mode.ss,
                };
                *slot = RealSegment {
                    selector,
                    base: u32::from(selector) << 4,
                    limit: slot.limit,
                };
                self.mode.eip = next_eip;
                if seg == SegReg::Ss {
                    self.mov_ss_shadow = true;
                }
            }
        }
        // The shadow armed by an earlier `MOV SS` ends after this (the next) instruction;
        // one armed by this instruction survives it. So consecutive `MOV SS` extend the
        // shadow: a conservative choice. By the SDM (Vol. 3A section 6.8.3, from memory)
        // only the first is guaranteed to inhibit interrupts, later ones "may or may not".
        if shadow_before {
            self.mov_ss_shadow = matches!(insn, RealInstruction::MovSegFromAx(SegReg::Ss));
        }
        Ok(insn)
    }

    /// Checks the 4-byte DS operand at `disp` (limit, readable, writable) and returns its
    /// linear address and value. Nothing is changed.
    fn check_dword_operand<M: RealModeMemory>(
        &self,
        mem: &M,
        disp: u16,
    ) -> Result<(u32, u32), RealModeRefusal> {
        let (cs, ip) = (self.mode.cs.selector, self.mode.eip);
        let offset = u32::from(disp);
        // The operand occupies offsets `disp..=disp + 3`: the last one must be under the limit.
        if offset + 3 > self.mode.ds.limit {
            return Err(RealModeRefusal::DataBeyondLimit {
                cs,
                ip,
                offset: offset.max(self.mode.ds.limit + 1),
            });
        }
        let linear = self.mode.ds.base.wrapping_add(offset);
        let mut value = 0u32;
        for i in 0..4u32 {
            let byte_linear = linear.wrapping_add(i);
            let byte = mem
                .read_u8(byte_linear)
                .ok_or(RealModeRefusal::DataUnreadable {
                    cs,
                    ip,
                    linear: byte_linear,
                })?;
            if !mem.can_write(byte_linear) {
                return Err(RealModeRefusal::DataUnwritable {
                    cs,
                    ip,
                    linear: byte_linear,
                });
            }
            value |= u32::from(byte) << (8 * i);
        }
        Ok((linear, value))
    }

    /// Replaces SP and keeps the upper half of ESP (16-bit stack size).
    fn set_sp(&mut self, sp: u16) {
        self.gpr[ESP] = (self.gpr[ESP] & 0xFFFF_0000) | u32::from(sp);
    }

    /// Replaces the six arithmetic flags (CF, PF, AF, ZF, SF, OF) and keeps every other bit.
    fn set_arith_flags(&mut self, flags: u32) {
        self.mode.eflags = (self.mode.eflags & !EFLAGS_ARITH) | flags;
    }

    /// Checks the `size`-byte word (2 or 4) at `SS:(SP - size)` for a push (offset limit,
    /// writability of every byte). Returns the new SP and the linear address of the low byte.
    /// Nothing is changed.
    fn check_stack_push<M: RealModeMemory>(
        &self,
        mem: &M,
        size: u32,
    ) -> Result<(u16, u32), RealModeRefusal> {
        let (cs, ip) = (self.mode.cs.selector, self.mode.eip);
        let new_sp = (self.gpr[ESP] as u16).wrapping_sub(size as u16);
        let offset = u32::from(new_sp);
        if offset + (size - 1) > self.mode.ss.limit {
            return Err(RealModeRefusal::DataBeyondLimit {
                cs,
                ip,
                offset: offset.max(self.mode.ss.limit + 1),
            });
        }
        let linear = self.mode.ss.base.wrapping_add(offset);
        for i in 0..size {
            let byte_linear = linear.wrapping_add(i);
            if !mem.can_write(byte_linear) {
                return Err(RealModeRefusal::DataUnwritable {
                    cs,
                    ip,
                    linear: byte_linear,
                });
            }
        }
        Ok((new_sp, linear))
    }

    /// Checks and reads the `size`-byte word (2 or 4) at `SS:SP` for a pop (offset limit,
    /// readability of every byte). Nothing is changed.
    fn check_stack_pop<M: RealModeMemory>(
        &self,
        mem: &M,
        size: u32,
    ) -> Result<u32, RealModeRefusal> {
        let (cs, ip) = (self.mode.cs.selector, self.mode.eip);
        let offset = u32::from(self.gpr[ESP] as u16);
        if offset + (size - 1) > self.mode.ss.limit {
            return Err(RealModeRefusal::DataBeyondLimit {
                cs,
                ip,
                offset: offset.max(self.mode.ss.limit + 1),
            });
        }
        let linear = self.mode.ss.base.wrapping_add(offset);
        let mut bytes = [0u8; 4];
        for (i, slot) in bytes.iter_mut().enumerate().take(size as usize) {
            let byte_linear = linear.wrapping_add(i as u32);
            *slot = mem
                .read_u8(byte_linear)
                .ok_or(RealModeRefusal::DataUnreadable {
                    cs,
                    ip,
                    linear: byte_linear,
                })?;
        }
        Ok(u32::from_le_bytes(bytes))
    }

    /// Executes up to `max_steps` instructions, stopping at the first refusal.
    pub fn run<M: RealModeMemory>(&mut self, mem: &mut M, max_steps: u64) -> RealRun {
        let mut steps = 0;
        while steps < max_steps {
            if let Err(refusal) = self.step(mem) {
                return RealRun {
                    steps,
                    stop: Some(refusal),
                };
            }
            steps += 1;
        }
        RealRun { steps, stop: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem(base: u32, bytes: &[u8]) -> ImageMemory<'_> {
        ImageMemory { base, bytes }
    }

    /// Entry `0000:1000`, the trampoline's first block as linked (without `WBINVD`):
    /// `cli; jmp 0:1008 (padding); mov ax,cs; mov ds,ax; mov es,ax; mov ss,ax; hlt`.
    const BLOCK1_LINKED: [u8; 19] = [
        0xFA, // 1000 cli
        0xEA, 0x08, 0x10, 0x00, 0x00, // 1001 jmp 0x0:0x1008
        0x90, 0x90, // 1006, 1007: skipped bytes
        0x8C, 0xC8, // 1008 mov ax,cs
        0x8E, 0xD8, // 100a mov ds,ax
        0x8E, 0xC0, // 100c mov es,ax
        0x8E, 0xD0, // 100e mov ss,ax
        0xF4, // 1010 hlt (not in this slice: stops the run)
        0x00, 0x00,
    ];

    #[test]
    fn cli_clears_if_and_advances_ip() {
        let mut cpu = RealModeCpu::at_cs_ip(0x0100, 0x0010, 0, 0);
        cpu.mode.eflags = 0x0000_0202; // IF = 1
        let image = [0xFA];
        let mut m = mem(0x1010, &image);
        assert_eq!(cpu.step(&mut m), Ok(RealInstruction::Cli));
        assert_eq!(cpu.mode.eflags, 0x0000_0002);
        assert_eq!(cpu.mode.eip, 0x0011);
        // CLI with IF already clear leaves the other flags alone.
        cpu.mode.eip = 0x10;
        cpu.mode.eflags = 0x0000_0003; // CF = 1
        assert_eq!(cpu.step(&mut m), Ok(RealInstruction::Cli));
        assert_eq!(cpu.mode.eflags, 0x0000_0003);
    }

    #[test]
    fn far_jump_loads_selector_base_and_offset() {
        // ea 34 12 78 56: jmp 0x5678:0x1234. By hand: base = 0x5678 << 4 = 0x56780,
        // EIP = 0x1234, limit kept.
        let mut cpu = RealModeCpu::at_cs_ip(0x0200, 0x0000, 0, 0);
        let image = [0xEA, 0x34, 0x12, 0x78, 0x56];
        let mut m = mem(0x2000, &image);
        let before = cpu;
        assert_eq!(
            cpu.step(&mut m),
            Ok(RealInstruction::JmpFar {
                selector: 0x5678,
                offset: 0x1234
            })
        );
        assert_eq!(
            cpu.mode.cs,
            RealSegment {
                selector: 0x5678,
                base: 0x56780,
                limit: 0xFFFF
            }
        );
        assert_eq!(cpu.mode.eip, 0x1234);
        // Nothing else changed.
        assert_eq!(cpu.gpr, before.gpr);
        assert_eq!(cpu.mode.ds, before.mode.ds);
        assert_eq!(cpu.mode.eflags, before.mode.eflags);
        assert_eq!(cpu.mode.cr0, before.mode.cr0);
    }

    #[test]
    fn far_jump_to_trampoline_form_zero_segment() {
        // ea 08 10 00 00: jmp 0x0:0x1008 -> CS = 0, base 0, EIP = 0x1008.
        let mut cpu = RealModeCpu::at_cs_ip(0x0000, 0x1001, 0, 0);
        let image = [0xEA, 0x08, 0x10, 0x00, 0x00];
        let mut m = mem(0x1001, &image);
        cpu.step(&mut m).unwrap();
        assert_eq!(cpu.mode.cs.selector, 0);
        assert_eq!(cpu.mode.cs.base, 0);
        assert_eq!(cpu.mode.eip, 0x1008);
    }

    #[test]
    fn far_jump_beyond_cs_limit_is_refused_without_state_change() {
        let mut cpu = RealModeCpu::at_cs_ip(0x0100, 0x0000, 0, 0);
        cpu.mode.cs.limit = 0x0FFF;
        let image = [0xEA, 0x00, 0x10, 0x00, 0x03]; // jmp 0x0300:0x1000
        let mut m = mem(0x1000, &image);
        let before = cpu;
        assert_eq!(
            cpu.step(&mut m),
            Err(RealModeRefusal::TargetBeyondLimit {
                offset: 0x1000,
                limit: 0x0FFF
            })
        );
        assert_eq!(cpu, before);
    }

    #[test]
    fn mov_ax_cs_writes_only_ax() {
        // CS = 0x9F00; EAX = 0xAABB_CCDD -> EAX = 0xAABB_9F00 (upper half kept).
        let mut cpu = RealModeCpu::at_cs_ip(0x9F00, 0, 0, 0);
        cpu.gpr[EAX] = 0xAABB_CCDD;
        let image = [0x8C, 0xC8];
        let mut m = mem(0x9F000, &image);
        assert_eq!(cpu.step(&mut m), Ok(RealInstruction::MovAxFromCs));
        assert_eq!(cpu.gpr[EAX], 0xAABB_9F00);
        assert_eq!(cpu.mode.eip, 2);
        // Other registers are untouched.
        assert_eq!(cpu.gpr[ECX], 0);
        assert_eq!(cpu.gpr[EBX], 0);
    }

    #[test]
    fn mov_seg_from_ax_sets_selector_and_base_keeping_limit() {
        // AX = 0x1234: DS = 0x1234, base 0x12340. EAX upper bits do not matter.
        for (bytes, seg) in [
            ([0x8E, 0xD8], SegReg::Ds),
            ([0x8E, 0xC0], SegReg::Es),
            ([0x8E, 0xD0], SegReg::Ss),
        ] {
            let mut cpu = RealModeCpu::at_cs_ip(0x0100, 0, 0, 0);
            cpu.gpr[EAX] = 0xFFFF_1234;
            // A non-default limit must survive the load.
            cpu.mode.ds.limit = 0x00FF;
            cpu.mode.es.limit = 0x01FF;
            cpu.mode.ss.limit = 0x02FF;
            let before = cpu;
            let mut m = mem(0x1000, &bytes);
            assert_eq!(cpu.step(&mut m), Ok(RealInstruction::MovSegFromAx(seg)));
            let loaded = RealSegment {
                selector: 0x1234,
                base: 0x12340,
                limit: match seg {
                    SegReg::Ds => 0x00FF,
                    SegReg::Es => 0x01FF,
                    _ => 0x02FF,
                },
            };
            assert_eq!(
                cpu.mode.ds,
                if seg == SegReg::Ds {
                    loaded
                } else {
                    before.mode.ds
                }
            );
            assert_eq!(
                cpu.mode.es,
                if seg == SegReg::Es {
                    loaded
                } else {
                    before.mode.es
                }
            );
            assert_eq!(
                cpu.mode.ss,
                if seg == SegReg::Ss {
                    loaded
                } else {
                    before.mode.ss
                }
            );
            assert_eq!(cpu.mode.cs, before.mode.cs);
            assert_eq!(cpu.mode.fs, before.mode.fs);
            assert_eq!(cpu.mode.gs, before.mode.gs);
            assert_eq!(cpu.gpr, before.gpr);
            assert_eq!(cpu.mode.eip, 2);
        }
    }

    #[test]
    fn mov_ss_arms_shadow_for_one_instruction() {
        let mut cpu = RealModeCpu::at_cs_ip(0x0100, 0, 0, 0);
        let image = [0x8E, 0xD0, 0xFA, 0xFA];
        let mut m = mem(0x1000, &image);
        cpu.step(&mut m).unwrap();
        assert!(cpu.mov_ss_shadow, "armed by MOV SS");
        cpu.step(&mut m).unwrap();
        assert!(!cpu.mov_ss_shadow, "ended after the next instruction");
        // MOV DS does not arm it.
        let mut cpu = RealModeCpu::at_cs_ip(0x0100, 0, 0, 0);
        let mut m2 = mem(0x1000, &[0x8E, 0xD8]);
        cpu.step(&mut m2).unwrap();
        assert!(!cpu.mov_ss_shadow);
        // MOV SS directly after MOV SS keeps it armed (its own shadow).
        let mut cpu = RealModeCpu::at_cs_ip(0x0100, 0, 0, 0);
        let mut m3 = mem(0x1000, &[0x8E, 0xD0, 0x8E, 0xD0]);
        cpu.step(&mut m3).unwrap();
        cpu.step(&mut m3).unwrap();
        assert!(cpu.mov_ss_shadow);
    }

    #[test]
    fn whole_block_linked_bytes_by_hand() {
        // Entry 0000:1000 as linked; the block begins at 0x1000 (the image here starts at
        // 0x1000, WBINVD omitted: that layout is the S6-14d one; S6-14e covers 0f 09).
        // Hand calculation: cli -> IF clear; jmp 0:0x1008 -> CS 0, EIP 0x1008;
        // mov ax,cs -> AX = 0; ds/es/ss = 0, base 0. Stops at hlt (0x1010).
        let mut cpu = RealModeCpu::at_cs_ip(0, 0x1000, 0, 0x0003_06A9);
        // The jump to 0:0x1008 skips the two padding bytes; the first image byte is the CLI
        // then the 5-byte jump (offset 1..5), padding at 6..7.
        let mut m = mem(0x1000, &BLOCK1_LINKED);
        let run = cpu.run(&mut m, 100);
        assert_eq!(run.steps, 6);
        assert_eq!(
            run.stop,
            Some(RealModeRefusal::Unsupported {
                cs: 0,
                ip: 0x1010,
                linear: 0x1010,
                bytes: vec![0xF4, 0x00, 0x00],
            })
        );
        assert_eq!(cpu.mode.eip, 0x1010);
        assert_eq!(cpu.gpr[EAX], 0);
        assert_eq!(cpu.gpr[EDX], 0x0003_06A9);
        assert_eq!(cpu.mode.eflags & EFLAGS_IF, 0);
    }

    #[test]
    fn unsupported_forms_name_address_and_bytes_and_keep_state() {
        let cases: &[(&[u8], &str)] = &[
            (&[0x90], "NOP"),
            (&[0xF4], "HLT"),
            // Near-misses of the S6-14f forms are in `s6_14f_near_misses_are_refused`.
            (&[0x0F, 0x22, 0xC0], "MOV CR0,EAX"),
            (&[0x66, 0x0F, 0x01, 0x1E, 0x60, 0x00], "LIDT"),
            // Near-miss forms of admitted opcodes: other ModRM, prefixes.
            (&[0x8C, 0xD8], "MOV AX,DS"),
            (&[0x8C, 0xC0], "MOV AX,ES"),
            (&[0x8C, 0xD0], "MOV AX,SS"),
            (&[0x8C, 0xCB], "MOV BX,CS"),
            (&[0x8E, 0xDB], "MOV DS,BX"),
            (&[0x8E, 0xE0], "MOV FS,AX"),
            (&[0x8E, 0xC8], "MOV CS,AX"),
            (&[0x8E, 0x1E, 0x00, 0x10], "MOV DS,[mem]"),
            (&[0x66, 0xFA], "prefixed CLI"),
            (&[0x66, 0x8C, 0xC8], "MOV EAX,CS"),
            (&[0x2E, 0xFA], "CS-prefixed CLI"),
            (&[0x67, 0x8E, 0xD8], "address-size prefix"),
            (&[0x66, 0xEA, 0, 0, 0, 0, 0, 0], "32-bit far jump"),
            // Near-misses of the S6-14e forms (opcode, prefix order, ModRM, operand size).
            (&[0x66, 0xF0, 0x0F, 0xBA, 0x26, 0x18, 0x50, 0x00], "LOCK BT"),
            (
                &[0x66, 0xF0, 0x0F, 0xBA, 0x36, 0x18, 0x50, 0x00],
                "LOCK BTR",
            ),
            (
                &[0x66, 0xF0, 0x0F, 0xBA, 0x3E, 0x18, 0x50, 0x00],
                "LOCK BTC",
            ),
            (
                &[0x66, 0xF0, 0x0F, 0xBA, 0x2D, 0x00, 0x00, 0x00],
                "LOCK BTS [DI]",
            ),
            (
                &[0x66, 0xF0, 0x0F, 0xBA, 0x6E, 0x00, 0x00, 0x00],
                "LOCK BTS [bp+d8]",
            ),
            (
                &[0x66, 0xF0, 0x0F, 0xBA, 0xEE, 0x00, 0x00, 0x00],
                "LOCK BTS reg",
            ),
            (
                &[0xF0, 0x66, 0x0F, 0xBA, 0x2E, 0x18, 0x50, 0x00],
                "F0 before 66",
            ),
            (
                &[0x66, 0x0F, 0xBA, 0x2E, 0x18, 0x50, 0x00, 0x00],
                "unlocked BTS",
            ),
            (
                &[0xF0, 0x0F, 0xBA, 0x2E, 0x18, 0x50, 0x00, 0x00],
                "16-bit LOCK BTS",
            ),
            (
                &[0x66, 0xF0, 0x0F, 0xAB, 0x2E, 0x18, 0x50, 0x00],
                "LOCK BTS m32,r32",
            ),
            (
                &[0x67, 0x66, 0xF0, 0x0F, 0xBA, 0x2E, 0x18, 0x50],
                "67-prefixed BTS",
            ),
            (&[0xF0, 0xFA], "LOCK CLI"),
            (&[0xF3, 0xA4], "REP MOVSB"),
            (&[0xF3, 0x0F, 0x09], "F3 WBINVD"),
            (&[0xF2, 0x90], "F2 90"),
            (&[0x66, 0x90], "66 NOP"),
            (&[0x0F, 0x0B], "UD2"),
            (&[0x0F, 0x08], "INVD"),
            (&[0x0F, 0x83, 0x04, 0x00], "JAE rel16"),
            // `JB rel8` and `JE rel8` became S6-14g forms; `JB`/`JE` rel16 are `0f 82`/`0f 84`.
            (&[0xE9, 0x00, 0x00], "JMP rel16"),
        ];
        for (bytes, name) in cases {
            let mut cpu = RealModeCpu::at_cs_ip(0x0123, 0x0040, 0, 0x1234);
            cpu.gpr[EAX] = 0x1111_2222;
            let before = cpu;
            let mut m = mem(0x1270, bytes);
            let r = cpu.step(&mut m);
            let mut expected_bytes = bytes.to_vec();
            expected_bytes.truncate(UNSUPPORTED_BYTES_MAX);
            assert_eq!(
                r,
                Err(RealModeRefusal::Unsupported {
                    cs: 0x0123,
                    ip: 0x0040,
                    linear: 0x1270,
                    bytes: expected_bytes,
                }),
                "{name}"
            );
            assert_eq!(cpu, before, "{name}: state unchanged");
        }
    }

    #[test]
    fn protected_mode_is_refused() {
        let mut cpu = RealModeCpu::at_cs_ip(0x0100, 0, 0x0000_0001, 0);
        cpu.mode.cr0 = 0x11;
        let mut m = mem(0x1000, &[0xFA]);
        let before = cpu;
        assert_eq!(
            cpu.step(&mut m),
            Err(RealModeRefusal::NotRealMode { cr0: 0x11 })
        );
        assert_eq!(cpu, before);
    }

    #[test]
    fn fetch_beyond_cs_limit_is_refused() {
        // MOV AX,CS opcode at the last offset 0xFFFF; its ModRM byte would be at 0x10000.
        let mut cpu = RealModeCpu::at_cs_ip(0x0000, 0xFFFF, 0, 0);
        let mut image = vec![0; 0x10000];
        image[0xFFFF] = 0x8C;
        let mut m = mem(0, &image);
        let before = cpu;
        assert_eq!(
            cpu.step(&mut m),
            Err(RealModeRefusal::FetchBeyondLimit {
                cs: 0,
                ip: 0xFFFF,
                offset: 0x10000
            })
        );
        assert_eq!(cpu, before);
    }

    #[test]
    fn instruction_ending_at_ffff_leaves_ip_10000_then_fetch_refuses() {
        // MOV AX,CS (8c c8) at 0000:FFFE occupies offsets 0xFFFE and 0xFFFF, the last
        // bytes under the limit 0xFFFF. By hand: next EIP = 0xFFFE + 2 = 0x10000 (not
        // wrapped), AX = CS = 0; the next fetch is at offset 0x10000 > limit.
        let mut cpu = RealModeCpu::at_cs_ip(0x0000, 0xFFFE, 0, 0);
        cpu.gpr[EAX] = 0x1234_5678;
        let mut image = vec![0; 0x10000];
        image[0xFFFE] = 0x8C;
        image[0xFFFF] = 0xC8;
        let mut m = mem(0, &image);
        assert_eq!(cpu.step(&mut m), Ok(RealInstruction::MovAxFromCs));
        assert_eq!(cpu.mode.eip, 0x10000);
        assert_eq!(cpu.gpr[EAX], 0x1234_0000);
        let after = cpu;
        assert_eq!(
            cpu.step(&mut m),
            Err(RealModeRefusal::FetchBeyondLimit {
                cs: 0,
                ip: 0x10000,
                offset: 0x10000
            })
        );
        assert_eq!(cpu, after);
    }

    #[test]
    fn unreadable_instruction_byte_is_refused() {
        let mut cpu = RealModeCpu::at_cs_ip(0x0100, 0, 0, 0);
        let mut m = mem(0x5000, &[0xFA]); // the instruction at 0x1000 is not in memory
        let before = cpu;
        assert_eq!(
            cpu.step(&mut m),
            Err(RealModeRefusal::FetchUnreadable {
                cs: 0x0100,
                ip: 0,
                linear: 0x1000
            })
        );
        assert_eq!(cpu, before);
    }

    #[test]
    fn after_sipi_entry_matches_smp_contract() {
        // SIPI vector 0x9F: CS = 0x9F00, base 0x9F000, IP = 0; EDX keeps the signature.
        let cpu = RealModeCpu::after_sipi(0x9F, 0x6000_0031, 0x306A9);
        assert_eq!(cpu.mode.cs.selector, 0x9F00);
        assert_eq!(cpu.mode.cs.base, 0x9F000);
        assert_eq!(cpu.mode.eip, 0);
        assert_eq!(cpu.mode.cr0, 0x6000_0010);
        assert_eq!(cpu.gpr, [0, 0, 0x306A9, 0, 0, 0, 0, 0]);
    }

    // ---- S6-14e forms: expectations computed by hand from the SDM (by memory) ------------

    /// `LOCK BTS DWORD PTR ds:0x0010, imm8` for a given immediate.
    fn bts_0010(imm8: u8) -> [u8; 8] {
        [0x66, 0xF0, 0x0F, 0xBA, 0x2E, 0x10, 0x00, imm8]
    }

    /// RAM from linear 0: code at 0x1000, a data dword at 0x2010 (DS base 0x2000 + 0x10).
    /// CS = 0x0100 (base 0x1000), IP = 0; DS = 0x0200 (base 0x2000).
    fn bts_setup(code: &[u8], dword: u32) -> (RealModeCpu, Vec<u8>) {
        let mut cpu = RealModeCpu::at_cs_ip(0x0100, 0, 0, 0);
        cpu.mode.ds = RealSegment {
            selector: 0x0200,
            base: 0x2000,
            limit: 0xFFFF,
        };
        let mut ram = vec![0u8; 0x3000];
        ram[0x1000..0x1000 + code.len()].copy_from_slice(code);
        ram[0x2010..0x2014].copy_from_slice(&dword.to_le_bytes());
        (cpu, ram)
    }

    fn dword_at(ram: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(ram[at..at + 4].try_into().unwrap())
    }

    #[test]
    fn wbinvd_changes_only_ip_and_memory_is_untouched() {
        let mut cpu = RealModeCpu::at_cs_ip(0x0100, 0x0020, 0, 0x1234);
        cpu.gpr[EAX] = 0xDEAD_BEEF;
        cpu.mode.eflags = 0x0000_0A97;
        let before = cpu;
        let mut ram = vec![0xA5u8; 0x1100];
        ram[0x1020] = 0x0F;
        ram[0x1021] = 0x09;
        let snapshot = ram.clone();
        let mut m = ImageMemoryMut {
            base: 0,
            bytes: &mut ram,
        };
        assert_eq!(cpu.step(&mut m), Ok(RealInstruction::Wbinvd));
        // By hand: IP 0x20 + 2 = 0x22; every other field is as before.
        assert_eq!(cpu.mode.eip, 0x0022);
        let mut expected = before;
        expected.mode.eip = 0x0022;
        assert_eq!(cpu, expected);
        assert_eq!(ram, snapshot);
    }

    #[test]
    fn pause_is_f3_90_only_and_changes_only_ip() {
        let mut cpu = RealModeCpu::at_cs_ip(0x0100, 0x0020, 0, 0x1234);
        cpu.gpr[ECX] = 7;
        cpu.mode.eflags = 0x0000_0247;
        let before = cpu;
        let image = [0xF3, 0x90];
        let mut m = mem(0x1020, &image);
        assert_eq!(cpu.step(&mut m), Ok(RealInstruction::Pause));
        let mut expected = before;
        expected.mode.eip = 0x0022;
        assert_eq!(cpu, expected);
        // The bare NOP (90) is not PAUSE and stays outside the profile.
        let mut cpu = before;
        let nop = [0x90];
        let mut m = mem(0x1020, &nop);
        assert!(matches!(
            cpu.step(&mut m),
            Err(RealModeRefusal::Unsupported { .. })
        ));
        assert_eq!(cpu, before);
    }

    #[test]
    fn wbinvd_and_pause_end_a_mov_ss_shadow() {
        let mut cpu = RealModeCpu::at_cs_ip(0x0100, 0, 0, 0);
        let image = [0x8E, 0xD0, 0x0F, 0x09, 0xF3, 0x90];
        let mut m = mem(0x1000, &image);
        cpu.step(&mut m).unwrap();
        assert!(cpu.mov_ss_shadow);
        cpu.step(&mut m).unwrap(); // WBINVD
        assert!(!cpu.mov_ss_shadow);
        cpu.step(&mut m).unwrap(); // PAUSE
        assert!(!cpu.mov_ss_shadow);
    }

    #[test]
    fn lock_bts_sets_bit_and_cf_is_the_old_bit() {
        // Hand calculation: DWORD 0x0000_0000, bit 3. Old bit 0 -> CF = 0, DWORD = 0x8.
        // EFLAGS starts as 0x2 (reserved bit only) and stays 0x2. Second run on the same
        // word: old bit 1 -> CF = 1, DWORD stays 0x8, EFLAGS = 0x3.
        let code = bts_0010(3);
        let (mut cpu, mut ram) = bts_setup(&code, 0);
        cpu.mode.eflags = 0x2;
        let mut m = ImageMemoryMut {
            base: 0,
            bytes: &mut ram,
        };
        assert_eq!(
            cpu.step(&mut m),
            Ok(RealInstruction::LockBtsImm8 {
                disp: 0x0010,
                imm8: 3
            })
        );
        assert_eq!(cpu.mode.eip, 8);
        assert_eq!(cpu.mode.eflags, 0x2);
        assert_eq!(dword_at(&ram, 0x2010), 0x0000_0008);
        cpu.mode.eip = 0;
        let mut m = ImageMemoryMut {
            base: 0,
            bytes: &mut ram,
        };
        cpu.step(&mut m).unwrap();
        assert_eq!(cpu.mode.eflags, 0x3);
        assert_eq!(dword_at(&ram, 0x2010), 0x0000_0008);
        // The code bytes and the byte before the operand are untouched.
        assert_eq!(&ram[0x1000..0x1008], &code);
        assert_eq!(ram[0x200F], 0);
        assert_eq!(ram[0x2014], 0);
    }

    #[test]
    fn lock_bts_bit_index_is_imm8_mod_32_little_endian() {
        // (imm8, initial dword, expected dword, expected CF), all by hand:
        // 0x00 -> bit 0 of byte 0; 0x08 -> bit 8 = byte 1 bit 0; 0x1F -> bit 31 = byte 3 bit
        // 7; 0x20 -> bit 0 again (mod 32); 0x25 -> bit 5; 0xFF -> bit 31.
        let cases: [(u8, u32, u32, u32); 7] = [
            (0x00, 0, 0x0000_0001, 0),
            (0x08, 0, 0x0000_0100, 0),
            (0x1F, 0, 0x8000_0000, 0),
            (0x20, 0x0000_0001, 0x0000_0001, 1),
            (0x25, 0x0000_0020, 0x0000_0020, 1),
            (0xFF, 0x7FFF_FFFF, 0xFFFF_FFFF, 0),
            (0xFF, 0x8000_0000, 0x8000_0000, 1),
        ];
        for (imm8, initial, expected, cf) in cases {
            let (mut cpu, mut ram) = bts_setup(&bts_0010(imm8), initial);
            let mut m = ImageMemoryMut {
                base: 0,
                bytes: &mut ram,
            };
            cpu.step(&mut m).unwrap();
            assert_eq!(dword_at(&ram, 0x2010), expected, "imm8 {imm8:#x}");
            assert_eq!(cpu.mode.eflags & EFLAGS_CF, cf, "imm8 {imm8:#x} CF");
        }
    }

    #[test]
    fn lock_bts_writes_cf_only_and_clears_it_for_a_clear_bit() {
        // EFLAGS 0x0000_0243 = CF | ZF | IF | reserved. Bit 4 of a zero dword is clear ->
        // CF = 0, ZF and IF kept (the other flags are undefined by the SDM; this model keeps
        // them): EFLAGS = 0x0000_0242. With the bit set: CF = 1 -> 0x0000_0243.
        let (mut cpu, mut ram) = bts_setup(&bts_0010(4), 0);
        cpu.mode.eflags = 0x0000_0243;
        let mut m = ImageMemoryMut {
            base: 0,
            bytes: &mut ram,
        };
        cpu.step(&mut m).unwrap();
        assert_eq!(cpu.mode.eflags, 0x0000_0242);
        cpu.mode.eip = 0;
        let mut m = ImageMemoryMut {
            base: 0,
            bytes: &mut ram,
        };
        cpu.step(&mut m).unwrap();
        assert_eq!(cpu.mode.eflags, 0x0000_0243);
        // Registers other than EFLAGS/EIP are untouched.
        assert_eq!(cpu.gpr, [0, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn lock_bts_operand_must_fit_under_the_ds_limit() {
        // DS base 0, limit 0xFFFF: the dword at 0xFFFC..0xFFFF fits; at 0xFFFD it would end at
        // 0x10000, beyond the limit (real mode does not wrap the operand): the first byte
        // beyond the limit is offset 0x10000. State and memory are unchanged.
        let mut cpu = RealModeCpu::at_cs_ip(0x0100, 0, 0, 0);
        let mut ram = vec![0u8; 0x10000];
        let fit = [0x66, 0xF0, 0x0F, 0xBA, 0x2E, 0xFC, 0xFF, 0x00];
        ram[0x1000..0x1008].copy_from_slice(&fit);
        let mut m = ImageMemoryMut {
            base: 0,
            bytes: &mut ram,
        };
        cpu.step(&mut m).unwrap();
        assert_eq!(dword_at(&ram, 0xFFFC), 1);

        let over = [0x66, 0xF0, 0x0F, 0xBA, 0x2E, 0xFD, 0xFF, 0x00];
        ram[0x1000..0x1008].copy_from_slice(&over);
        cpu.mode.eip = 0;
        let (before, snapshot) = (cpu, ram.clone());
        let mut m = ImageMemoryMut {
            base: 0,
            bytes: &mut ram,
        };
        assert_eq!(
            cpu.step(&mut m),
            Err(RealModeRefusal::DataBeyondLimit {
                cs: 0x0100,
                ip: 0,
                offset: 0x10000
            })
        );
        assert_eq!(cpu, before);
        assert_eq!(ram, snapshot);

        // A smaller DS limit: operand wholly beyond it names its own first offset.
        let (mut cpu, mut ram) = bts_setup(&[0x66, 0xF0, 0x0F, 0xBA, 0x2E, 0x00, 0x02, 0x00], 0);
        cpu.mode.ds.limit = 0x00FF;
        let before = cpu;
        let mut m = ImageMemoryMut {
            base: 0,
            bytes: &mut ram,
        };
        assert_eq!(
            cpu.step(&mut m),
            Err(RealModeRefusal::DataBeyondLimit {
                cs: 0x0100,
                ip: 0,
                offset: 0x0200
            })
        );
        assert_eq!(cpu, before);
    }

    /// Memory that is writable only below `read_only_from`.
    struct PartlyReadOnly<'a> {
        inner: ImageMemoryMut<'a>,
        read_only_from: u32,
    }

    impl RealModeMemory for PartlyReadOnly<'_> {
        fn read_u8(&self, linear: u32) -> Option<u8> {
            self.inner.read_u8(linear)
        }
        fn can_write(&self, linear: u32) -> bool {
            linear < self.read_only_from && self.inner.can_write(linear)
        }
        fn write_u8(&mut self, linear: u32, value: u8) -> bool {
            self.can_write(linear) && self.inner.write_u8(linear, value)
        }
    }

    #[test]
    fn lock_bts_unreadable_or_unwritable_operand_is_refused_unchanged() {
        // Unreadable: memory ends at 0x2012, so the third operand byte (0x2012) is missing.
        let (mut cpu, ram) = bts_setup(&bts_0010(0), 0);
        let mut short = ram[..0x2012].to_vec();
        let before = cpu;
        let mut m = ImageMemoryMut {
            base: 0,
            bytes: &mut short,
        };
        assert_eq!(
            cpu.step(&mut m),
            Err(RealModeRefusal::DataUnreadable {
                cs: 0x0100,
                ip: 0,
                linear: 0x2012
            })
        );
        assert_eq!(cpu, before);
        assert_eq!(short, ram[..0x2012]);

        // Unwritable tail: bytes 0x2012 and 0x2013 are read-only, 0x2010 and 0x2011 are
        // writable. The refusal names 0x2012 and no byte of the operand is written.
        let (mut cpu, mut ram) = bts_setup(&bts_0010(0), 0);
        let before = cpu;
        let snapshot = ram.clone();
        let mut m = PartlyReadOnly {
            inner: ImageMemoryMut {
                base: 0,
                bytes: &mut ram,
            },
            read_only_from: 0x2012,
        };
        assert_eq!(
            cpu.step(&mut m),
            Err(RealModeRefusal::DataUnwritable {
                cs: 0x0100,
                ip: 0,
                linear: 0x2012
            })
        );
        assert_eq!(cpu, before);
        assert_eq!(ram, snapshot);

        // Fully read-only image memory.
        let (mut cpu, ram) = bts_setup(&bts_0010(0), 0);
        let before = cpu;
        let mut m = mem(0, &ram);
        assert_eq!(
            cpu.step(&mut m),
            Err(RealModeRefusal::DataUnwritable {
                cs: 0x0100,
                ip: 0,
                linear: 0x2010
            })
        );
        assert_eq!(cpu, before);
    }

    #[test]
    fn truncated_lock_bts_is_a_fetch_refusal() {
        // Only 6 of the 8 bytes are present: the displacement high byte (offset 6) is missing.
        let mut cpu = RealModeCpu::at_cs_ip(0x0000, 0x1000, 0, 0);
        let image = [0x66, 0xF0, 0x0F, 0xBA, 0x2E, 0x18];
        let mut m = mem(0x1000, &image);
        let before = cpu;
        assert_eq!(
            cpu.step(&mut m),
            Err(RealModeRefusal::FetchUnreadable {
                cs: 0,
                ip: 0x1000,
                linear: 0x1006
            })
        );
        assert_eq!(cpu, before);
    }

    #[test]
    fn jae_follows_cf_only() {
        // `73 04` at 0x1018: next IP 0x101A, taken target 0x101A + 4 = 0x101E (as in the
        // trampoline). Other flags set (OF SF ZF PF = 0x8C4) with CF = 0: taken.
        let image = [0x73, 0x04];
        let mut cpu = RealModeCpu::at_cs_ip(0x0000, 0x1018, 0, 0);
        cpu.mode.eflags = 0x0000_08C6;
        let before = cpu;
        let mut m = mem(0x1018, &image);
        assert_eq!(cpu.step(&mut m), Ok(RealInstruction::Jae { rel: 4 }));
        assert_eq!(cpu.mode.eip, 0x101E);
        assert_eq!(cpu.mode.eflags, before.mode.eflags);
        // CF = 1: not taken, IP = 0x101A.
        let mut cpu = RealModeCpu::at_cs_ip(0x0000, 0x1018, 0, 0);
        cpu.mode.eflags = 0x0000_0003;
        cpu.step(&mut m).unwrap();
        assert_eq!(cpu.mode.eip, 0x101A);
        assert_eq!(cpu.mode.eflags, 3);
        // Backward: `73 fa` (rel = -6) at 0x1018: 0x101A - 6 = 0x1014.
        let back = [0x73, 0xFA];
        let mut cpu = RealModeCpu::at_cs_ip(0x0000, 0x1018, 0, 0);
        let mut m = mem(0x1018, &back);
        assert_eq!(cpu.step(&mut m), Ok(RealInstruction::Jae { rel: -6 }));
        assert_eq!(cpu.mode.eip, 0x1014);
    }

    #[test]
    fn jmp_short_forward_backward_and_wrapping() {
        // `eb f2` at 0x101C: next 0x101E, rel -14 -> 0x1010 (the trampoline back-edge).
        let mut cpu = RealModeCpu::at_cs_ip(0x0000, 0x101C, 0, 0);
        let image = [0xEB, 0xF2];
        let mut m = mem(0x101C, &image);
        assert_eq!(cpu.step(&mut m), Ok(RealInstruction::JmpShort { rel: -14 }));
        assert_eq!(cpu.mode.eip, 0x1010);
        // `eb fe` at 0: next 2, rel -2 -> 0 (jump to itself).
        let mut cpu = RealModeCpu::at_cs_ip(0x0000, 0, 0, 0);
        let image = [0xEB, 0xFE];
        let mut m = mem(0, &image);
        cpu.step(&mut m).unwrap();
        assert_eq!(cpu.mode.eip, 0);
        // 16-bit operand size truncates the target: `eb 80` at 2: next 4, rel -128 ->
        // -124 = 0xFF84 mod 0x10000.
        let mut cpu = RealModeCpu::at_cs_ip(0x0000, 2, 0, 0);
        let image = [0x00, 0x00, 0xEB, 0x80];
        let mut m = mem(0, &image);
        cpu.step(&mut m).unwrap();
        assert_eq!(cpu.mode.eip, 0xFF84);
        // `eb 10` at 0xFFFE: next 0x10000 (unwrapped), +16 = 0x10010 -> 0x0010.
        let mut cpu = RealModeCpu::at_cs_ip(0x0000, 0xFFFE, 0, 0);
        let mut ram = vec![0u8; 0x10000];
        ram[0xFFFE] = 0xEB;
        ram[0xFFFF] = 0x10;
        let mut m = ImageMemoryMut {
            base: 0,
            bytes: &mut ram,
        };
        cpu.step(&mut m).unwrap();
        assert_eq!(cpu.mode.eip, 0x0010);
    }

    #[test]
    fn taken_short_jump_beyond_cs_limit_is_refused_unchanged() {
        // CS limit 0x0FFF; `eb 20` at 0x0FF0: next 0x0FF2 + 0x20 = 0x1012 > limit.
        let mut cpu = RealModeCpu::at_cs_ip(0x0000, 0x0FF0, 0, 0);
        cpu.mode.cs.limit = 0x0FFF;
        let image = [0xEB, 0x20];
        let mut m = mem(0x0FF0, &image);
        let before = cpu;
        assert_eq!(
            cpu.step(&mut m),
            Err(RealModeRefusal::TargetBeyondLimit {
                offset: 0x1012,
                limit: 0x0FFF
            })
        );
        assert_eq!(cpu, before);
        // The same distance with JAE: refused when CF = 0, a plain fall-through when CF = 1.
        let image = [0x73, 0x20];
        let mut m = mem(0x0FF0, &image);
        assert!(matches!(
            cpu.step(&mut m),
            Err(RealModeRefusal::TargetBeyondLimit { offset: 0x1012, .. })
        ));
        assert_eq!(cpu, before);
        cpu.mode.eflags |= EFLAGS_CF;
        cpu.step(&mut m).unwrap();
        assert_eq!(cpu.mode.eip, 0x0FF2);
    }

    /// The block-1 lock loop at blob offsets 0x1010..0x101E, followed by HLT at 0x101E.
    const LOCK_LOOP: [u8; 17] = [
        0x66, 0xF0, 0x0F, 0xBA, 0x2E, 0x18, 0x50, 0x00, // 1010 lock bts [0x5018], 0
        0x73, 0x04, // 1018 jae 0x101e
        0xF3, 0x90, // 101a pause
        0xEB, 0xF2, // 101c jmp 0x1010
        0xF4, 0x00, 0x00, // 101e hlt (stops the run)
    ];

    #[test]
    fn lock_loop_spins_while_held_and_exits_when_released() {
        // CS = DS = 0, EFLAGS = 2, lock word at 0x5018 = 1 (held).
        // Hand calculation, one lap: bts -> CF = 1 (EFLAGS 3), word stays 1, IP 0x1018;
        // jae not taken -> 0x101A; pause -> 0x101C; jmp -> 0x1010. 4 steps per lap, state
        // identical after each lap.
        let mut cpu = RealModeCpu::at_cs_ip(0, 0x1010, 0, 0);
        cpu.mode.eflags = 2;
        let mut ram = vec![0u8; 0x6000];
        ram[0x1010..0x1010 + LOCK_LOOP.len()].copy_from_slice(&LOCK_LOOP);
        ram[0x5018] = 1;
        let mut m = ImageMemoryMut {
            base: 0,
            bytes: &mut ram,
        };
        let run = cpu.run(&mut m, 4);
        assert_eq!((run.steps, run.stop), (4, None));
        assert_eq!(cpu.mode.eip, 0x1010);
        assert_eq!(cpu.mode.eflags, 3);
        let run = cpu.run(&mut m, 8);
        assert_eq!((run.steps, run.stop), (8, None));
        assert_eq!(cpu.mode.eip, 0x1010);
        assert_eq!(cpu.mode.eflags, 3);
        assert_eq!(dword_at(&ram, 0x5018), 1);
        // Another processor releases the lock (word = 0): bts -> CF = 0, word = 1 (taken by
        // us), jae taken -> 0x101E; the run then stops at HLT.
        ram[0x5018] = 0;
        let mut m = ImageMemoryMut {
            base: 0,
            bytes: &mut ram,
        };
        let run = cpu.run(&mut m, 100);
        assert_eq!(run.steps, 2);
        assert_eq!(
            run.stop,
            Some(RealModeRefusal::Unsupported {
                cs: 0,
                ip: 0x101E,
                linear: 0x101E,
                bytes: vec![0xF4, 0, 0, 0, 0, 0, 0, 0], // zeroed RAM after the HLT
            })
        );
        assert_eq!(cpu.mode.eip, 0x101E);
        assert_eq!(cpu.mode.eflags, 2);
        assert_eq!(dword_at(&ram, 0x5018), 1);
    }

    #[test]
    fn block1_with_wbinvd_and_lock_by_hand() {
        // The linked layout of the blob (0x1000..0x101E), entry 0000:1000, lock word 0.
        // Hand calculation: cli (IF already 0), wbinvd (nothing), jmp 0:0x1008, mov ax,cs ->
        // AX = 0, mov ds/es/ss,ax -> 0; lock bts [ds:0x5018],0 -> CF = 0, word = 1; jae
        // 0x101e taken. 9 steps, then HLT at 0x101E stops the run.
        let code: [u8; 31] = [
            0xFA, 0x0F, 0x09, 0xEA, 0x08, 0x10, 0x00, 0x00, 0x8C, 0xC8, 0x8E, 0xD8, 0x8E, 0xC0,
            0x8E, 0xD0, 0x66, 0xF0, 0x0F, 0xBA, 0x2E, 0x18, 0x50, 0x00, 0x73, 0x04, 0xF3, 0x90,
            0xEB, 0xF2, 0xF4,
        ];
        let mut cpu = RealModeCpu::at_cs_ip(0, 0x1000, 0, 0x0003_06A9);
        let mut ram = vec![0u8; 0x6000];
        ram[0x1000..0x1000 + code.len()].copy_from_slice(&code);
        let mut m = ImageMemoryMut {
            base: 0,
            bytes: &mut ram,
        };
        let run = cpu.run(&mut m, 100);
        assert_eq!(run.steps, 9);
        assert!(matches!(
            run.stop,
            Some(RealModeRefusal::Unsupported {
                ip: 0x101E,
                linear: 0x101E,
                ..
            })
        ));
        assert_eq!(cpu.mode.eip, 0x101E);
        assert_eq!(cpu.mode.eflags, 2);
        assert_eq!(cpu.gpr[EAX], 0);
        assert_eq!(cpu.gpr[EDX], 0x0003_06A9);
        assert_eq!(dword_at(&ram, 0x5018), 1);
    }

    // ---- Comparison with the KVM oracle (S6-14d) ---------------------------------------
    //
    // Images, entry points and oracle output are the files in
    // docs/reports/data/stage6/s6-14d/ (produced by run-oracle.sh on the Intel Mini). The
    // image ends with HLT, which this slice does not execute: the run stops there with
    // `Unsupported`, and the oracle's RIP is one byte past the HLT.

    /// Number after the nested keys `path` (searched in order) in the oracle JSON.
    fn oracle_num(json: &str, path: &[&str]) -> u64 {
        let mut pos = 0;
        for key in path {
            let needle = format!("\"{key}\":");
            pos += json[pos..]
                .find(&needle)
                .unwrap_or_else(|| panic!("key {key} not found in oracle JSON"))
                + needle.len();
        }
        let digits: String = json[pos..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        digits.parse().expect("number in oracle JSON")
    }

    /// CPUID leaf 1 EAX of the oracle guest model: the processor signature that INIT leaves
    /// in EDX.
    fn oracle_signature(json: &str) -> u32 {
        let at = json.find("\"function\":1,").expect("leaf 1");
        oracle_num(&json[at..], &["eax"]) as u32
    }

    fn compare_with_oracle(
        image: &[u8],
        json: &str,
        mut cpu: RealModeCpu,
        entry_cs: u16,
        entry_ip: u16,
        expected_hlt_ip: u32,
    ) {
        assert!(json.contains("\"stop_reason\":\"guest_hlt\""));
        let base = (u32::from(entry_cs) << 4) + u32::from(entry_ip);
        let mut m = mem(base, image);
        let run = cpu.run(&mut m, 100);
        assert_eq!(run.steps, 6, "cli, jmp, mov ax,cs, mov ds/es/ss");
        match run.stop {
            Some(RealModeRefusal::Unsupported {
                bytes, linear, ip, ..
            }) => {
                assert_eq!(bytes, vec![0xF4], "stops at HLT");
                assert_eq!(ip, expected_hlt_ip);
                assert_eq!(linear, cpu.mode.cs.base + expected_hlt_ip);
                // HLT is the last image byte.
                assert_eq!(linear, base + image.len() as u32 - 1);
            }
            other => panic!("expected stop at HLT, got {other:?}"),
        }
        assert_final_state_matches(json, &cpu);
    }

    /// Every GPR, segment (selector, base, limit), EIP (the oracle's RIP is one byte past the
    /// HLT), EFLAGS and CR0 of `cpu` against the oracle's `final_state`.
    fn assert_final_state_matches(json: &str, cpu: &RealModeCpu) {
        assert_final_state_matches_except(json, cpu, &[]);
    }

    /// The same, skipping the general-purpose registers listed in `skip` (indices of [`EAX`]...):
    /// the registers whose value comes from the oracle host's CPUID.
    fn assert_final_state_matches_except(json: &str, cpu: &RealModeCpu, skip: &[usize]) {
        let get = |tail: &[&str]| {
            let mut path = vec!["final_state"];
            path.extend_from_slice(tail);
            oracle_num(json, &path)
        };
        // IP: the oracle's RIP is after the HLT.
        assert_eq!(u64::from(cpu.mode.eip) + 1, get(&["rip"]));
        for (i, name) in ["rax", "rcx", "rdx", "rbx", "rsp", "rbp", "rsi", "rdi"]
            .iter()
            .enumerate()
        {
            if !skip.contains(&i) {
                assert_eq!(u64::from(cpu.gpr[i]), get(&["gpr", name]), "{name}");
            }
        }
        for (name, seg) in [
            ("cs", cpu.mode.cs),
            ("ds", cpu.mode.ds),
            ("es", cpu.mode.es),
            ("ss", cpu.mode.ss),
            ("fs", cpu.mode.fs),
            ("gs", cpu.mode.gs),
        ] {
            assert_eq!(
                u64::from(seg.selector),
                get(&["segments", name, "selector"]),
                "{name} selector"
            );
            assert_eq!(
                u64::from(seg.base),
                get(&["segments", name, "base"]),
                "{name} base"
            );
            assert_eq!(
                u64::from(seg.limit),
                get(&["segments", name, "limit"]),
                "{name} limit"
            );
        }
        assert_eq!(u64::from(cpu.mode.eflags), get(&["rflags"]));
        assert_eq!(u64::from(cpu.mode.cr0), get(&["control", "cr0"]));
    }

    macro_rules! oracle_case {
        ($name:ident, $stem:literal, $build:expr, $cs:expr, $ip:expr, $hlt_ip:expr) => {
            #[test]
            fn $name() {
                let image: &[u8] = include_bytes!(concat!(
                    "../../../docs/reports/data/stage6/s6-14d/",
                    $stem,
                    ".bin"
                ));
                let json: &str = include_str!(concat!(
                    "../../../docs/reports/data/stage6/s6-14d/oracle-",
                    $stem,
                    ".json"
                ));
                // KVM's CR0 before the oracle applies the INIT rule has CD and NW set.
                let cpu: RealModeCpu = ($build)(oracle_signature(json));
                compare_with_oracle(image, json, cpu, $cs, $ip, $hlt_ip);
            }
        };
    }

    oracle_case!(
        oracle_a_linked_entry_0000_1000,
        "a-linked-0000-1000",
        |sig| RealModeCpu::at_cs_ip(0x0000, 0x1000, 0x6000_0010, sig),
        0x0000,
        0x1000,
        0x1010
    );
    oracle_case!(
        oracle_b_relocated_segment_0100_1000,
        "b-reloc-0100-1000",
        |sig| RealModeCpu::at_cs_ip(0x0100, 0x1000, 0x6000_0010, sig),
        0x0100,
        0x1000,
        0x1010
    );
    oracle_case!(
        oracle_c_far_jump_changes_cs,
        "c-farjmp-0100-1000",
        |sig| RealModeCpu::at_cs_ip(0x0100, 0x1000, 0x6000_0010, sig),
        0x0100,
        0x1000,
        0x000E
    );
    oracle_case!(
        oracle_d_sipi_vector_9f,
        "d-sipi9f-9f00-0000",
        |sig| RealModeCpu::after_sipi(0x9F, 0x6000_0010, sig),
        0x9F00,
        0x0000,
        0x000E
    );

    // ---- Comparison with the KVM oracle (S6-14e) ---------------------------------------
    //
    // Files in docs/reports/data/stage6/s6-14e/ (run-oracle.sh). Besides the registers, the
    // 4-byte word captured with `--result-block` (the lock word or the data word) is
    // compared with the model's memory.

    /// Bytes of the oracle's `result_block_hex` and its guest address.
    fn oracle_result_block(json: &str) -> (u32, Vec<u8>) {
        let gpa = oracle_num(json, &["result_block_gpa"]) as u32;
        let needle = "\"result_block_hex\":\"";
        let at = json.find(needle).expect("result_block_hex") + needle.len();
        let hex: String = json[at..].chars().take_while(|c| *c != '"').collect();
        let bytes = (0..hex.len() / 2)
            .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap())
            .collect();
        (gpa, bytes)
    }

    /// Runs the image through `od-isa` from the same entry as the oracle (RAM = the image at
    /// `entry_cs << 4 + entry_ip` in zeroed memory), then compares registers, segments,
    /// EFLAGS, CR0 and the captured memory word with the oracle's JSON.
    fn compare_with_oracle_e(
        image: &[u8],
        json: &str,
        mut cpu: RealModeCpu,
        (entry_cs, entry_ip): (u16, u16),
        expected_steps: u64,
        expected_hlt_ip: u32,
    ) {
        assert!(json.contains("\"stop_reason\":\"guest_hlt\""));
        let base = u32::from(entry_cs) << 4;
        let mut ram = vec![0u8; 0x6000];
        let at = usize::from(entry_ip);
        ram[at..at + image.len()].copy_from_slice(image);
        let mut m = ImageMemoryMut {
            base,
            bytes: &mut ram,
        };
        let run = cpu.run(&mut m, 100);
        assert_eq!(run.steps, expected_steps);
        match run.stop {
            Some(RealModeRefusal::Unsupported { bytes, ip, .. }) => {
                assert_eq!(bytes[0], 0xF4, "stops at HLT");
                assert_eq!(ip, expected_hlt_ip);
            }
            other => panic!("expected stop at HLT, got {other:?}"),
        }
        assert_final_state_matches(json, &cpu);
        let (gpa, oracle_bytes) = oracle_result_block(json);
        let start = (gpa - base) as usize;
        assert_eq!(oracle_bytes.len(), 4);
        assert_eq!(
            &ram[start..start + 4],
            &oracle_bytes[..],
            "memory word at {gpa:#x}"
        );
    }

    macro_rules! oracle_case_e {
        ($name:ident, $stem:literal, $cs:expr, $ip:expr, $steps:expr, $hlt_ip:expr) => {
            #[test]
            fn $name() {
                let image: &[u8] = include_bytes!(concat!(
                    "../../../docs/reports/data/stage6/s6-14e/",
                    $stem,
                    ".bin"
                ));
                let json: &str = include_str!(concat!(
                    "../../../docs/reports/data/stage6/s6-14e/oracle-",
                    $stem,
                    ".json"
                ));
                let cpu = RealModeCpu::at_cs_ip($cs, $ip, 0x6000_0010, oracle_signature(json));
                compare_with_oracle_e(image, json, cpu, ($cs, $ip), $steps, $hlt_ip);
            }
        };
    }

    // a: the blob layout of block 1 (cli, wbinvd, far jmp, mov x4, lock bts, jae taken):
    // 9 steps, stop at the HLT at 0x101E, lock word 0 -> 1.
    oracle_case_e!(
        oracle_e_a_linked_block1_lock_free,
        "a-linked-0000-1000",
        0x0000,
        0x1000,
        9,
        0x101E
    );
    // b: the same relocated to CS = 0x9F00 (the lock word at 0x9F000 + 0x5018).
    oracle_case_e!(
        oracle_e_b_relocated_block1_lock_free,
        "b-reloc-9f00-1000",
        0x9F00,
        0x1000,
        9,
        0x101E
    );
    // c: wbinvd, bts, bts (CF = 1), jae not taken, pause, short jmp forward: 6 steps.
    oracle_case_e!(
        oracle_e_c_bts_twice_cf1_jae_not_taken,
        "c-bts-twice-0000-1000",
        0x0000,
        0x1000,
        6,
        0x101A
    );
    // d: bts bit 31 of a data word initialised to 0x0100_0001: 1 step.
    oracle_case_e!(
        oracle_e_d_bts_bit31_of_initialised_word,
        "d-bts-bit31-0000-1000",
        0x0000,
        0x1000,
        1,
        0x1008
    );

    // ---- S6-14f: MOV r32,imm32, CALL/RET, PUSHF/POPF, PUSH imm8, XOR EAX,EAX -----------
    //
    // Expected values below are worked out by hand from the SDM (Vol. 2, from memory), not
    // read back from the implementation.

    /// CS = 0 (base 0), SS = DS = 0, SP = 0x6020 (`ESP` = `esp`), RAM 0..0x10000 with `code`
    /// at `ip`.
    fn stack_setup(ip: u16, code: &[u8], esp: u32) -> (RealModeCpu, Vec<u8>) {
        let mut cpu = RealModeCpu::at_cs_ip(0, ip, 0, 0x1234);
        cpu.gpr[ESP] = esp;
        let mut ram = vec![0u8; 0x1_0000];
        ram[usize::from(ip)..usize::from(ip) + code.len()].copy_from_slice(code);
        (cpu, ram)
    }

    fn word_at(ram: &[u8], at: usize) -> u16 {
        u16::from_le_bytes([ram[at], ram[at + 1]])
    }

    #[test]
    fn mov_r32_imm32_writes_eax_and_esp_only_and_no_flags() {
        // 66 bc 20 60 00 00 at 0x101e (the blob's `mov esp, 0x6020`): ESP = 0x6020, EIP =
        // 0x101e + 6 = 0x1024, everything else unchanged.
        let (mut cpu, mut ram) = stack_setup(0x101E, &[0x66, 0xBC, 0x20, 0x60, 0, 0], 0xFFFF_FFFF);
        cpu.gpr[EAX] = 0x1111_2222;
        cpu.mode.eflags = 0x0000_08D7;
        let mut m = ImageMemoryMut {
            base: 0,
            bytes: &mut ram,
        };
        assert_eq!(
            cpu.step(&mut m),
            Ok(RealInstruction::MovR32Imm32 {
                reg: ESP,
                imm: 0x6020
            })
        );
        assert_eq!(cpu.gpr[ESP], 0x6020, "the whole ESP is replaced");
        assert_eq!(cpu.gpr[EAX], 0x1111_2222);
        assert_eq!(cpu.mode.eip, 0x1024);
        assert_eq!(cpu.mode.eflags, 0x0000_08D7);
        // 66 b8 imm32 = EAX; the full 32-bit value, little-endian.
        let (mut cpu, mut ram) = stack_setup(0x1000, &[0x66, 0xB8, 0xEF, 0xBE, 0xAD, 0xDE], 5);
        let mut m = ImageMemoryMut {
            base: 0,
            bytes: &mut ram,
        };
        assert_eq!(
            cpu.step(&mut m),
            Ok(RealInstruction::MovR32Imm32 {
                reg: EAX,
                imm: 0xDEAD_BEEF
            })
        );
        assert_eq!(cpu.gpr[EAX], 0xDEAD_BEEF);
        assert_eq!(cpu.gpr[ESP], 5);
        assert_eq!(cpu.mode.eip, 0x1006);
    }

    #[test]
    fn call_near_pushes_next_ip_and_jumps() {
        // e8 59 00 at 0x1024: next = 0x1027, target = 0x1027 + 0x59 = 0x1080. SP 0x6020 ->
        // 0x601e, word 0x1027 stored little-endian at 0x601e.
        let (mut cpu, mut ram) = stack_setup(0x1024, &[0xE8, 0x59, 0x00], 0x6020);
        cpu.mode.eflags = 0x0000_0A97;
        let mut m = ImageMemoryMut {
            base: 0,
            bytes: &mut ram,
        };
        assert_eq!(
            cpu.step(&mut m),
            Ok(RealInstruction::CallNear { rel: 0x59 })
        );
        assert_eq!(cpu.mode.eip, 0x1080);
        assert_eq!(cpu.gpr[ESP], 0x601E);
        assert_eq!(&ram[0x601E..0x6020], &[0x27, 0x10]);
        assert_eq!(cpu.mode.eflags, 0x0000_0A97, "CALL changes no flag");
        // Backward call: e8 fd ff at 0x1100: next 0x1103, -3 -> 0x1100 (a call to itself).
        let (mut cpu, mut ram) = stack_setup(0x1100, &[0xE8, 0xFD, 0xFF], 0x6020);
        let mut m = ImageMemoryMut {
            base: 0,
            bytes: &mut ram,
        };
        cpu.step(&mut m).unwrap();
        assert_eq!(cpu.mode.eip, 0x1100);
        assert_eq!(word_at(&ram, 0x601E), 0x1103);
        // 16-bit wrap of the target: at 0xFF00, next 0xFF03, rel +0x0200 -> 0x10103 & 0xFFFF
        // = 0x0103.
        let (mut cpu, mut ram) = stack_setup(0xFF00, &[0xE8, 0x00, 0x02], 0x6020);
        let mut m = ImageMemoryMut {
            base: 0,
            bytes: &mut ram,
        };
        cpu.step(&mut m).unwrap();
        assert_eq!(cpu.mode.eip, 0x0103);
        assert_eq!(word_at(&ram, 0x601E), 0xFF03);
        // A call ending exactly at 0x10000 pushes the 16-bit IP 0 and, with rel 0, goes to 0.
        let (mut cpu, mut ram) = stack_setup(0xFFFD, &[0xE8, 0x00, 0x00], 0x6020);
        let mut m = ImageMemoryMut {
            base: 0,
            bytes: &mut ram,
        };
        cpu.step(&mut m).unwrap();
        assert_eq!(cpu.mode.eip, 0);
        assert_eq!(word_at(&ram, 0x601E), 0);
    }

    #[test]
    fn ret_near_pops_ip_and_keeps_flags() {
        // Word 0x1027 at 0x601e, SP = 0x601e: RET -> IP = 0x1027, SP = 0x6020.
        let (mut cpu, mut ram) = stack_setup(0x11A8, &[0xC3], 0x6_601E);
        ram[0x601E..0x6020].copy_from_slice(&[0x27, 0x10]);
        cpu.mode.eflags = 0x0000_0A97;
        let before_ram = ram.clone();
        let mut m = ImageMemoryMut {
            base: 0,
            bytes: &mut ram,
        };
        assert_eq!(cpu.step(&mut m), Ok(RealInstruction::RetNear));
        assert_eq!(cpu.mode.eip, 0x1027);
        assert_eq!(
            cpu.gpr[ESP], 0x6_6020,
            "SP + 2, upper half of ESP kept: 0x0006_601E -> 0x0006_6020"
        );
        assert_eq!(cpu.mode.eflags, 0x0000_0A97);
        assert_eq!(ram, before_ram, "RET writes no memory");
    }

    #[test]
    fn call_then_ret_round_trip() {
        // 0x1024: call 0x1080 (e8 59 00); 0x1080: ret. Hand: after the call SP = 0x601e and
        // IP = 0x1080, after the ret IP = 0x1027 and SP = 0x6020 again.
        let (mut cpu, mut ram) = stack_setup(0x1024, &[0xE8, 0x59, 0x00], 0x6020);
        ram[0x1080] = 0xC3;
        let mut m = ImageMemoryMut {
            base: 0,
            bytes: &mut ram,
        };
        cpu.step(&mut m).unwrap();
        assert_eq!((cpu.mode.eip, cpu.gpr[ESP]), (0x1080, 0x601E));
        cpu.step(&mut m).unwrap();
        assert_eq!((cpu.mode.eip, cpu.gpr[ESP]), (0x1027, 0x6020));
    }

    #[test]
    fn esp_upper_half_is_kept_and_sp_wraps_below_zero() {
        // ESP = 0xABCD_0000 (SP = 0): CALL stores at SS:0xFFFE, SP = 0xFFFE, ESP = 0xABCD_FFFE
        // (a 16-bit stack touches SP only). Then RET returns SP to 0, ESP = 0xABCD_0000.
        let (mut cpu, mut ram) = stack_setup(0x1000, &[0xE8, 0x0D, 0x00], 0xABCD_0000);
        ram[0x1010] = 0xC3;
        let mut m = ImageMemoryMut {
            base: 0,
            bytes: &mut ram,
        };
        cpu.step(&mut m).unwrap();
        assert_eq!(cpu.gpr[ESP], 0xABCD_FFFE);
        assert_eq!(cpu.mode.eip, 0x1010);
        cpu.step(&mut m).unwrap();
        assert_eq!(cpu.gpr[ESP], 0xABCD_0000);
        assert_eq!(cpu.mode.eip, 0x1003);
        assert_eq!(word_at(&ram, 0xFFFE), 0x1003);
    }

    #[test]
    fn stack_uses_the_ss_base_not_the_ds_or_cs_base() {
        // SS = 0x0800 (base 0x8000), SP = 0x0100: push imm8 writes at 0x8000 + 0xFE.
        let (mut cpu, mut ram) = stack_setup(0x1000, &[0x6A, 0x55], 0x0100);
        cpu.mode.ss = RealSegment {
            selector: 0x0800,
            base: 0x8000,
            limit: 0xFFFF,
        };
        cpu.mode.ds.base = 0x4000;
        ram.resize(0x2_0000, 0);
        let mut m = ImageMemoryMut {
            base: 0,
            bytes: &mut ram,
        };
        cpu.step(&mut m).unwrap();
        assert_eq!(word_at(&ram, 0x80FE), 0x0055);
        assert_eq!(cpu.gpr[ESP], 0x00FE);
    }

    #[test]
    fn pushf_pushes_the_low_word_and_popf_restores_the_modifiable_bits() {
        // EFLAGS = 0x0021_08D7 (ID, OF, SF, ZF, AF, PF, bit 1, CF). PUSHF stores the low word
        // 0x08D7. Then POPF of 0x0000 (push imm8 0) gives 0x0021_0002: the writable bits go
        // to 0, bit 1 stays set, the upper half (ID) is not touched by the 16-bit POPF.
        let (mut cpu, mut ram) = stack_setup(0x1000, &[0x9C, 0x6A, 0x00, 0x9D, 0x9D], 0x6020);
        cpu.mode.eflags = 0x0021_08D7;
        let mut m = ImageMemoryMut {
            base: 0,
            bytes: &mut ram,
        };
        assert_eq!(cpu.step(&mut m), Ok(RealInstruction::Pushf));
        assert_eq!(cpu.gpr[ESP], 0x601E);
        assert_eq!(cpu.mode.eflags, 0x0021_08D7);
        assert_eq!(cpu.step(&mut m), Ok(RealInstruction::PushImm8 { imm: 0 }));
        assert_eq!(cpu.step(&mut m), Ok(RealInstruction::Popf));
        assert_eq!(cpu.mode.eflags, 0x0021_0002);
        assert_eq!(cpu.gpr[ESP], 0x601E);
        // The second POPF restores the saved 0x08D7 (the verify_cpu pattern).
        assert_eq!(cpu.step(&mut m), Ok(RealInstruction::Popf));
        assert_eq!(cpu.mode.eflags, 0x0021_08D7);
        assert_eq!(cpu.gpr[ESP], 0x6020);
        assert_eq!(cpu.mode.eip, 0x1005);
    }

    #[test]
    fn popf_writes_only_its_mask() {
        // Pop 0xFCFF (every bit but TF and IF): the written bits are 0xFCFF & 0x7FD5 =
        // 0x7CD5; bit 15 (reserved) and bits 3 and 5 are not written; bit 1 stays 1.
        // 0x0002 | 0x7CD5 = 0x7CD7.
        let (mut cpu, mut ram) = stack_setup(0x1000, &[0x9D], 0x6000);
        ram[0x6000..0x6002].copy_from_slice(&0xFCFFu16.to_le_bytes());
        cpu.mode.eflags = 0x0002;
        let mut m = ImageMemoryMut {
            base: 0,
            bytes: &mut ram,
        };
        cpu.step(&mut m).unwrap();
        assert_eq!(cpu.mode.eflags, 0x7CD7);
        // A pop of 0 clears every bit of the mask (IF, IOPL, NT, OF, SF, ZF, AF, PF, CF) except
        // the ones outside it.
        let (mut cpu, mut ram) = stack_setup(0x1000, &[0x9D], 0x6000);
        cpu.mode.eflags = 0x0002 | 0x7000 | 0x0A00 | 0x8D5 | 0x0008 | 0x0020;
        let mut m = ImageMemoryMut {
            base: 0,
            bytes: &mut ram,
        };
        cpu.step(&mut m).unwrap();
        // Bits 3 and 5 (0x28) are not in the mask: they are not touched by the pop.
        assert_eq!(cpu.mode.eflags, 0x0002 | 0x0028);
    }

    #[test]
    fn popf_with_tf_or_if_is_refused_unchanged() {
        for flags in [0x0100u16, 0x0200, 0x0302, 0xFFFF] {
            let (mut cpu, mut ram) = stack_setup(0x1000, &[0x9D], 0x6000);
            ram[0x6000..0x6002].copy_from_slice(&flags.to_le_bytes());
            cpu.mode.eflags = 0x0000_0046;
            let before = cpu;
            let snapshot = ram.clone();
            let mut m = ImageMemoryMut {
                base: 0,
                bytes: &mut ram,
            };
            assert_eq!(
                cpu.step(&mut m),
                Err(RealModeRefusal::PopfUnmodelledFlags {
                    cs: 0,
                    ip: 0x1000,
                    flags
                })
            );
            assert_eq!(cpu, before);
            assert_eq!(ram, snapshot);
        }
    }

    #[test]
    fn push_imm8_sign_extends_to_a_word() {
        // 0x00 -> 0x0000, 0x7f -> 0x007f, 0x80 -> 0xff80, 0xff -> 0xffff.
        for (imm, word) in [
            (0x00u8, 0x0000u16),
            (0x7F, 0x007F),
            (0x80, 0xFF80),
            (0xFF, 0xFFFF),
        ] {
            let (mut cpu, mut ram) = stack_setup(0x1000, &[0x6A, imm], 0x6020);
            cpu.mode.eflags = 0x0000_08D7;
            let mut m = ImageMemoryMut {
                base: 0,
                bytes: &mut ram,
            };
            assert_eq!(
                cpu.step(&mut m),
                Ok(RealInstruction::PushImm8 { imm: imm as i8 })
            );
            assert_eq!(word_at(&ram, 0x601E), word, "imm8 {imm:#x}");
            assert_eq!(cpu.gpr[ESP], 0x601E);
            assert_eq!(cpu.mode.eip, 0x1002);
            assert_eq!(cpu.mode.eflags, 0x0000_08D7);
        }
    }

    #[test]
    fn xor_eax_eax_sets_zero_result_flags() {
        // EAX = 0xDEADBEEF, EFLAGS = 0x0A97 (IF, OF, SF, ZF, AF, PF, CF). XOR clears CF, OF,
        // SF, AF and sets ZF, PF: bits 0x8D5 are cleared (leaving IF 0x200 and bit 1) and 0x44
        // is set: 0x202 | 0x44 = 0x246. Upper EFLAGS bits (ID 0x200000) are kept.
        let (mut cpu, mut ram) = stack_setup(0x11A5, &[0x66, 0x31, 0xC0], 0x6020);
        cpu.gpr[EAX] = 0xDEAD_BEEF;
        cpu.gpr[ECX] = 7;
        cpu.mode.eflags = 0x0020_0A97;
        let mut m = ImageMemoryMut {
            base: 0,
            bytes: &mut ram,
        };
        assert_eq!(cpu.step(&mut m), Ok(RealInstruction::XorEaxEax));
        assert_eq!(cpu.gpr[EAX], 0);
        assert_eq!(cpu.gpr[ECX], 7);
        assert_eq!(cpu.mode.eflags, 0x0020_0246);
        assert_eq!(cpu.mode.eip, 0x11A8);
    }

    #[test]
    fn stack_push_must_fit_under_the_ss_limit() {
        // SS limit 0x00FF. SP = 0x0100: the word goes to 0xFE..=0xFF, fine. SP = 0x0101: the
        // word would occupy 0xFF..=0x100: refused, first offset beyond the limit is 0x100.
        // SP = 0x0300: new SP = 0x2FE is entirely beyond: offset 0x2FE. SP = 0: new SP =
        // 0xFFFE, offset 0xFFFE. SP = 1: new SP = 0xFFFF, the word straddles 0xFFFF..0x10000.
        let pushes: [(&[u8], &str); 3] = [
            (&[0x9C], "PUSHF"),
            (&[0x6A, 0x01], "PUSH imm8"),
            (&[0xE8, 0x00, 0x00], "CALL"),
        ];
        for (code, name) in pushes {
            for (esp, limit, expected) in [
                (0x0100u32, 0x00FFu32, None),
                (0x0101, 0x00FF, Some(0x0100)),
                (0x0300, 0x00FF, Some(0x02FE)),
                (0x0000, 0x00FF, Some(0xFFFE)),
                (0x0001, 0xFFFF, Some(0x1_0000)),
            ] {
                let (mut cpu, mut ram) = stack_setup(0x1000, code, esp);
                cpu.mode.ss.limit = limit;
                let before = cpu;
                let snapshot = ram.clone();
                let mut m = ImageMemoryMut {
                    base: 0,
                    bytes: &mut ram,
                };
                let r = cpu.step(&mut m);
                match expected {
                    None => {
                        assert!(r.is_ok(), "{name} esp {esp:#x}");
                        assert_eq!(cpu.gpr[ESP], 0x00FE);
                    }
                    Some(offset) => {
                        assert_eq!(
                            r,
                            Err(RealModeRefusal::DataBeyondLimit {
                                cs: 0,
                                ip: 0x1000,
                                offset
                            }),
                            "{name} esp {esp:#x}"
                        );
                        assert_eq!(cpu, before, "{name}: state unchanged");
                        assert_eq!(ram, snapshot, "{name}: memory unchanged");
                    }
                }
            }
        }
    }

    #[test]
    fn stack_pop_must_fit_under_the_ss_limit() {
        // SS limit 0x00FF. SP = 0xFE: bytes 0xFE and 0xFF, fine. SP = 0xFF: bytes 0xFF and
        // 0x100: refused (offset 0x100). SP = 0xFFFF with limit 0xFFFF: 0xFFFF and 0x10000:
        // refused (offset 0x10000), no wrap to offset 0.
        for code in [&[0xC3u8][..], &[0x9D]] {
            for (esp, limit, expected) in [
                (0x00FEu32, 0x00FFu32, None),
                (0x00FF, 0x00FF, Some(0x0100u32)),
                (0xFFFF, 0xFFFF, Some(0x1_0000)),
            ] {
                let (mut cpu, mut ram) = stack_setup(0x1000, code, esp);
                cpu.mode.ss.limit = limit;
                let before = cpu;
                let mut m = ImageMemoryMut {
                    base: 0,
                    bytes: &mut ram,
                };
                let r = cpu.step(&mut m);
                match expected {
                    None => assert!(r.is_ok()),
                    Some(offset) => {
                        assert_eq!(
                            r,
                            Err(RealModeRefusal::DataBeyondLimit {
                                cs: 0,
                                ip: 0x1000,
                                offset
                            })
                        );
                        assert_eq!(cpu, before);
                    }
                }
            }
        }
    }

    #[test]
    fn stack_unwritable_or_unreadable_is_refused_unchanged() {
        // Push: the second byte of the word (0x601f) is read-only. The first one is writable
        // but must not be written.
        for code in [&[0x9Cu8][..], &[0x6A, 0x01], &[0xE8, 0x00, 0x00]] {
            let (mut cpu, mut ram) = stack_setup(0x1000, code, 0x6020);
            let before = cpu;
            let snapshot = ram.clone();
            let mut m = PartlyReadOnly {
                inner: ImageMemoryMut {
                    base: 0,
                    bytes: &mut ram,
                },
                read_only_from: 0x601F,
            };
            assert_eq!(
                cpu.step(&mut m),
                Err(RealModeRefusal::DataUnwritable {
                    cs: 0,
                    ip: 0x1000,
                    linear: 0x601F
                })
            );
            assert_eq!(cpu, before);
            assert_eq!(ram, snapshot);
        }
        // Read-only image: the first byte 0x601e is the one named.
        let (mut cpu, ram) = stack_setup(0x1000, &[0x9C], 0x6020);
        let before = cpu;
        let mut m = mem(0, &ram);
        assert_eq!(
            cpu.step(&mut m),
            Err(RealModeRefusal::DataUnwritable {
                cs: 0,
                ip: 0x1000,
                linear: 0x601E
            })
        );
        assert_eq!(cpu, before);
        // Pop: memory ends at 0x6001, so the second byte (0x6001) is not readable.
        for code in [&[0xC3u8][..], &[0x9D]] {
            let (mut cpu, ram) = stack_setup(0x1000, code, 0x6000);
            let before = cpu;
            let mut short = ram[..0x6001].to_vec();
            let mut m = ImageMemoryMut {
                base: 0,
                bytes: &mut short,
            };
            assert_eq!(
                cpu.step(&mut m),
                Err(RealModeRefusal::DataUnreadable {
                    cs: 0,
                    ip: 0x1000,
                    linear: 0x6001
                })
            );
            assert_eq!(cpu, before);
        }
    }

    #[test]
    fn call_and_ret_targets_must_lie_under_the_cs_limit() {
        // CS limit 0x2000. CALL to 0x1000 + 3 + 0x2000 = 0x3003 is refused without pushing.
        let (mut cpu, mut ram) = stack_setup(0x1000, &[0xE8, 0x00, 0x20], 0x6020);
        cpu.mode.cs.limit = 0x2000;
        let before = cpu;
        let snapshot = ram.clone();
        let mut m = ImageMemoryMut {
            base: 0,
            bytes: &mut ram,
        };
        assert_eq!(
            cpu.step(&mut m),
            Err(RealModeRefusal::TargetBeyondLimit {
                offset: 0x3003,
                limit: 0x2000
            })
        );
        assert_eq!(cpu, before);
        assert_eq!(ram, snapshot);
        // RET popping 0x3000 (limit 0x2000): refused, SP unchanged.
        let (mut cpu, mut ram) = stack_setup(0x1000, &[0xC3], 0x6000);
        ram[0x6000..0x6002].copy_from_slice(&0x3000u16.to_le_bytes());
        cpu.mode.cs.limit = 0x2000;
        let before = cpu;
        let mut m = ImageMemoryMut {
            base: 0,
            bytes: &mut ram,
        };
        assert_eq!(
            cpu.step(&mut m),
            Err(RealModeRefusal::TargetBeyondLimit {
                offset: 0x3000,
                limit: 0x2000
            })
        );
        assert_eq!(cpu, before);
        // A target exactly at the limit is allowed.
        let (mut cpu, mut ram) = stack_setup(0x1000, &[0xC3], 0x6000);
        ram[0x6000..0x6002].copy_from_slice(&0x2000u16.to_le_bytes());
        cpu.mode.cs.limit = 0x2000;
        let mut m = ImageMemoryMut {
            base: 0,
            bytes: &mut ram,
        };
        cpu.step(&mut m).unwrap();
        assert_eq!(cpu.mode.eip, 0x2000);
    }

    #[test]
    fn stack_instructions_end_a_mov_ss_shadow() {
        // `MOV SS, AX` then `PUSH imm8` (the next instruction): the shadow ends after it.
        let (mut cpu, mut ram) = stack_setup(0x1000, &[0x8E, 0xD0, 0x6A, 0x01, 0x9C], 0x6020);
        let mut m = ImageMemoryMut {
            base: 0,
            bytes: &mut ram,
        };
        cpu.step(&mut m).unwrap();
        assert!(cpu.mov_ss_shadow);
        cpu.step(&mut m).unwrap();
        assert!(!cpu.mov_ss_shadow);
    }

    #[test]
    fn s6_14f_near_misses_are_refused() {
        let cases: &[(&[u8], &str)] = &[
            (&[0x66, 0xB9, 0, 0, 0, 0], "MOV ECX,imm32"),
            (&[0x66, 0xBD, 0, 0, 0, 0], "MOV EBP,imm32"),
            (&[0xB8, 0x01, 0x00], "MOV AX,imm16"),
            (&[0xBC, 0x20, 0x60], "MOV SP,imm16"),
            (&[0x58], "POP AX"),
            (&[0x50], "PUSH AX"),
            (&[0x66, 0x6A, 0x00], "PUSH imm8 (32-bit)"),
            (&[0x68, 0x00, 0x00], "PUSH imm16"),
            (&[0x66, 0xE8, 0x00, 0x00, 0x00, 0x00], "CALL rel32"),
            (&[0x66, 0xC3], "RET (32-bit)"),
            (&[0xC2, 0x04, 0x00], "RET imm16"),
            (&[0xCB], "RETF"),
            (&[0x9A, 0, 0, 0, 0], "CALL far"),
            (&[0xFF, 0xD0], "CALL AX"),
            (&[0xF3, 0xC3], "REP RET"),
            (&[0x67, 0xE8, 0x00, 0x00], "67 CALL"),
            (&[0x2E, 0xC3], "CS RET"),
            (&[0xF0, 0x9C], "LOCK PUSHF"),
            (&[0x66, 0x31, 0xC9], "XOR ECX,ECX"),
            (&[0x66, 0x31, 0xD8], "XOR EAX,EBX"),
            (&[0x66, 0x33, 0xC0], "XOR r32,r/m32 (33)"),
            (&[0x31, 0xC0], "XOR AX,AX"),
            (&[0x66, 0x0F, 0xA2], "66 CPUID"),
            (&[0x9B], "FWAIT"),
        ];
        for (bytes, name) in cases {
            let (mut cpu, mut ram) = stack_setup(0x1000, &[0; 0], 0x6020);
            ram[0x1000..0x1000 + bytes.len()].copy_from_slice(bytes);
            let before = cpu;
            let snapshot = ram.clone();
            let mut m = ImageMemoryMut {
                base: 0,
                bytes: &mut ram,
            };
            let mut expected_bytes = ram_bytes(&m, 0x1000, 8);
            expected_bytes.truncate(UNSUPPORTED_BYTES_MAX);
            assert_eq!(
                cpu.step(&mut m),
                Err(RealModeRefusal::Unsupported {
                    cs: 0,
                    ip: 0x1000,
                    linear: 0x1000,
                    bytes: expected_bytes,
                }),
                "{name}"
            );
            assert_eq!(cpu, before, "{name}: state unchanged");
            assert_eq!(ram, snapshot, "{name}: memory unchanged");
        }
    }

    fn ram_bytes(m: &ImageMemoryMut<'_>, at: u32, n: u32) -> Vec<u8> {
        (0..n).map(|i| m.read_u8(at + i).unwrap()).collect()
    }

    #[test]
    fn truncated_s6_14f_forms_are_fetch_refusals() {
        // `66 b8` with only two immediate bytes: the third immediate byte (offset 4) is missing.
        let mut cpu = RealModeCpu::at_cs_ip(0, 0x1000, 0, 0);
        let image = [0x66, 0xB8, 0x01, 0x00];
        let mut m = mem(0x1000, &image);
        assert_eq!(
            cpu.step(&mut m),
            Err(RealModeRefusal::FetchUnreadable {
                cs: 0,
                ip: 0x1000,
                linear: 0x1004
            })
        );
        // `e8 59` without the high byte; `6a` without the immediate; `66 31` without ModRM.
        for (image, linear) in [
            (&[0xE8u8, 0x59][..], 0x1002u32),
            (&[0x6A], 0x1001),
            (&[0x66, 0x31], 0x1002),
        ] {
            let mut cpu = RealModeCpu::at_cs_ip(0, 0x1000, 0, 0);
            let mut m = mem(0x1000, image);
            assert_eq!(
                cpu.step(&mut m),
                Err(RealModeRefusal::FetchUnreadable {
                    cs: 0,
                    ip: 0x1000,
                    linear
                })
            );
        }
    }

    #[test]
    fn verify_cpu_shaped_success_and_failure_tails_by_hand() {
        // Blob bytes 0x101e..0x1027 then a callee shaped like verify_cpu's start and tails:
        //   0x101e mov esp,0x6020 ; 0x1024 call 0x1080 ; 0x1027 hlt (stands for TEST EAX,EAX)
        //   0x1080 pushf; push 0; popf ; 0x1084 popf; xor eax,eax; ret   (0x11a4 shape)
        // Hand: ESP = 0x6020; call: [0x601e] = 0x1027, SP = 0x601e; pushf: [0x601c] = 0x0002
        // (EFLAGS = 2), SP = 0x601c; push 0: [0x601a] = 0, SP = 0x601a; popf: flags = 2,
        // SP = 0x601c; popf: flags = 0x0002, SP = 0x601e; xor eax,eax: EAX = 0, flags = 0x46;
        // ret: IP = 0x1027, SP = 0x6020. 8 steps in all: the mov, the call, then 6 callee
        // instructions (pushf, push 0, popf, popf, xor, ret) -> stop at 0x1027 (HLT).
        let mut code = vec![0u8; 0x90];
        code[0x1E..0x1E + 6].copy_from_slice(&[0x66, 0xBC, 0x20, 0x60, 0, 0]);
        code[0x24..0x27].copy_from_slice(&[0xE8, 0x59, 0x00]);
        code[0x27] = 0xF4;
        code[0x80..0x84].copy_from_slice(&[0x9C, 0x6A, 0x00, 0x9D]);
        code[0x84..0x89].copy_from_slice(&[0x9D, 0x66, 0x31, 0xC0, 0xC3]);
        let (mut cpu, mut ram) = stack_setup(0x101E - 0x1E, &[], 0);
        ram[0x1000..0x1000 + code.len()].copy_from_slice(&code);
        cpu.mode.eip = 0x101E;
        cpu.gpr[EAX] = 0x5555_5555;
        let mut m = ImageMemoryMut {
            base: 0,
            bytes: &mut ram,
        };
        let run = cpu.run(&mut m, 100);
        assert_eq!(run.steps, 8);
        assert!(matches!(
            run.stop,
            Some(RealModeRefusal::Unsupported { ip: 0x1027, .. })
        ));
        assert_eq!(cpu.gpr[EAX], 0);
        assert_eq!(cpu.gpr[ESP], 0x6020);
        assert_eq!(cpu.mode.eflags, 0x0000_0046);
        assert_eq!(word_at(&ram, 0x601E), 0x1027);
        assert_eq!(word_at(&ram, 0x601C), 0x0002);
        assert_eq!(word_at(&ram, 0x601A), 0x0000);

        // Failure tail (0x119b shape): `popf; mov eax,1; ret` with CF = 1 saved by pushf.
        // Hand: entry flags 0x0003 (CF). pushf [..] = 0x0003, push 0, popf -> flags 2, popf
        // -> flags 3, mov eax,1, ret.
        let mut code = vec![0u8; 0x40];
        code[0..6].copy_from_slice(&[0x66, 0xBC, 0x20, 0x60, 0, 0]);
        code[6..9].copy_from_slice(&[0xE8, 0x11, 0x00]); // next 0x1009 + 0x11 = 0x101a
        code[9] = 0xF4;
        code[0x1A..0x1E].copy_from_slice(&[0x9C, 0x6A, 0x00, 0x9D]);
        code[0x1E..0x26].copy_from_slice(&[0x9D, 0x66, 0xB8, 1, 0, 0, 0, 0xC3]);
        let (mut cpu, mut ram) = stack_setup(0x1000, &[], 0);
        ram[0x1000..0x1000 + code.len()].copy_from_slice(&code);
        cpu.mode.eflags = 0x0003;
        let mut m = ImageMemoryMut {
            base: 0,
            bytes: &mut ram,
        };
        let run = cpu.run(&mut m, 100);
        assert_eq!(run.steps, 8);
        assert_eq!(cpu.gpr[EAX], 1);
        assert_eq!(cpu.mode.eflags, 0x0003);
        assert_eq!(cpu.gpr[ESP], 0x6020);
        assert_eq!(cpu.mode.eip, 0x1009);
    }

    // ---- Comparison with the KVM oracle (S6-14f) ---------------------------------------
    //
    // Files in docs/reports/data/stage6/s6-14f/ (run-oracle.sh). The model runs the same image
    // from the same entry; the registers, segments, EFLAGS, CR0 and the captured stack words
    // are compared with the oracle's JSON, not with test expectations.

    fn compare_with_oracle_f(
        image: &[u8],
        json: &str,
        cpu: RealModeCpu,
        entry: (u16, u16),
        expected_steps: u64,
        expected_hlt_ip: u32,
    ) {
        compare_with_oracle_skipping(
            image,
            json,
            cpu,
            entry,
            expected_steps,
            expected_hlt_ip,
            &[],
        );
    }

    /// As [`compare_with_oracle_f`], skipping the registers in `skip` (S6-14g: CPUID outputs).
    fn compare_with_oracle_skipping(
        image: &[u8],
        json: &str,
        mut cpu: RealModeCpu,
        (entry_cs, entry_ip): (u16, u16),
        expected_steps: u64,
        expected_hlt_ip: u32,
        skip: &[usize],
    ) {
        assert!(json.contains("\"stop_reason\":\"guest_hlt\""));
        // The stack is 16-bit in the oracle (SS.B = 0): the model's SP-only rule applies.
        assert_eq!(
            oracle_num(json, &["final_state", "segments", "ss", "db"]),
            0
        );
        let base = u32::from(entry_cs) << 4;
        let mut ram = vec![0u8; 0x1_0000];
        let at = usize::from(entry_ip);
        ram[at..at + image.len()].copy_from_slice(image);
        let mut m = ImageMemoryMut {
            base,
            bytes: &mut ram,
        };
        let run = cpu.run(&mut m, 1000);
        assert_eq!(run.steps, expected_steps);
        match run.stop {
            Some(RealModeRefusal::Unsupported { bytes, ip, .. }) => {
                assert_eq!(bytes[0], 0xF4, "stops at HLT");
                assert_eq!(ip, expected_hlt_ip);
            }
            other => panic!("expected stop at HLT, got {other:?}"),
        }
        assert_final_state_matches_except(json, &cpu, skip);
        // The oracle's SS base is the model's: the captured stack words are physical memory.
        let (gpa, oracle_bytes) = oracle_result_block(json);
        let start = (gpa - base) as usize;
        assert_eq!(
            &ram[start..start + oracle_bytes.len()],
            &oracle_bytes[..],
            "stack words at {gpa:#x}"
        );
    }

    macro_rules! oracle_case_f {
        ($name:ident, $stem:literal, $cs:expr, $ip:expr, $steps:expr, $hlt_ip:expr) => {
            #[test]
            fn $name() {
                let image: &[u8] = include_bytes!(concat!(
                    "../../../docs/reports/data/stage6/s6-14f/",
                    $stem,
                    ".bin"
                ));
                let json: &str = include_str!(concat!(
                    "../../../docs/reports/data/stage6/s6-14f/oracle-",
                    $stem,
                    ".json"
                ));
                let cpu = RealModeCpu::at_cs_ip($cs, $ip, 0x6000_0010, oracle_signature(json));
                compare_with_oracle_f(image, json, cpu, ($cs, $ip), $steps, $hlt_ip);
            }
        };
    }

    // a: the blob layout 0x1000..0x1027 (block 1, mov esp, call) and a verify_cpu-shaped
    // callee at 0x1080 (pushf; push 0; popf; popf; xor eax,eax; ret): 17 steps, HLT at 0x1027.
    oracle_case_f!(
        oracle_f_a_call_success_tail,
        "a-call-success-tail-0000-1000",
        0x0000,
        0x1000,
        17,
        0x1027
    );
    // b: relocated to CS = 0x9f00 (SS = 0x9f00), CF = 1 from the second BTS, the failure-tail
    // shape (popf; mov eax,1; ret): 13 steps, HLT at 0x101f.
    oracle_case_f!(
        oracle_f_b_call_failure_tail_cf1_relocated,
        "b-call-failure-tail-cf1-9f00-1000",
        0x9F00,
        0x1000,
        13,
        0x101F
    );
    // c: nested call, PUSH imm8 -1/0x7f/0x57, POPF of 0x57, RET: 8 steps, HLT at 0x102a.
    oracle_case_f!(
        oracle_f_c_nested_call_push_imm8_popf,
        "c-nested-call-push-imm8-popf-0000-1000",
        0x0000,
        0x1000,
        8,
        0x102A
    );
    // d: ESP = 0x10000, CALL stores at SS:0xFFFE, RET: 3 steps, HLT at 0x1009.
    oracle_case_f!(
        oracle_f_d_sp_wrap_call_ret,
        "d-sp-wrap-call-ret-0000-1000",
        0x0000,
        0x1000,
        3,
        0x1009
    );
    // e: the same, stopping inside the callee: ESP = 0x1fffe (upper half kept): 2 steps.
    oracle_case_f!(
        oracle_f_e_sp_wrap_stop_in_callee,
        "e-sp-wrap-stop-in-callee-0000-1000",
        0x0000,
        0x1000,
        2,
        0x1010
    );

    // ---- S6-14g: forms of `verify_cpu` (blocks 2 and 3) without vendor-specific branches ----
    //
    // Every expected value below was computed by hand from the SDM (by memory) and cross-checked
    // with an independent script, never read back from the implementation.

    /// Runs one instruction `code` at `0000:1000` with `ESP = 0x6020` after `prep`; returns the
    /// state before, the state after, the RAM after, and the result.
    fn g_step(
        code: &[u8],
        prep: impl FnOnce(&mut RealModeCpu, &mut Vec<u8>),
    ) -> (
        RealModeCpu,
        RealModeCpu,
        Vec<u8>,
        Result<RealInstruction, RealModeRefusal>,
    ) {
        let (mut cpu, mut ram) = stack_setup(0x1000, code, 0x6020);
        prep(&mut cpu, &mut ram);
        let before = cpu;
        let mut m = ImageMemoryMut {
            base: 0,
            bytes: &mut ram,
        };
        let r = cpu.step(&mut m);
        (before, cpu, ram, r)
    }

    /// All six arithmetic flags set (and bit 1): the input that shows which flags a form rewrites.
    const ALL_ARITH: u32 = 0x0000_08D7;

    #[test]
    fn s6_14g_test_eax_eax_sets_flags_from_eax_and_keeps_eax() {
        // 66 85 c0. By hand: result = EAX. CF = OF = 0, AF cleared by the model; ZF, SF, PF from
        // the result (PF: even number of set bits in the low byte).
        for (eax, expected) in [
            (0u32, 0x0000_0046),        // ZF, PF; bit 1
            (0x8000_0001, 0x0000_0082), // SF; low byte 01 is odd: PF = 0
            (3, 0x0000_0006),           // low byte 11b is even: PF = 1
        ] {
            let (_, cpu, _, r) = g_step(&[0x66, 0x85, 0xC0], |c, _| {
                c.gpr[EAX] = eax;
                c.mode.eflags = ALL_ARITH;
            });
            assert_eq!(r, Ok(RealInstruction::TestEaxEax));
            assert_eq!(cpu.mode.eflags, expected, "EAX = {eax:#x}");
            assert_eq!(cpu.gpr[EAX], eax);
            assert_eq!(cpu.mode.eip, 0x1003);
        }
        // Flags outside the six (DF, ID) are kept.
        let (_, cpu, _, _) = g_step(&[0x66, 0x85, 0xC0], |c, _| {
            c.mode.eflags = 0x0020_0402;
        });
        assert_eq!(cpu.mode.eflags, 0x0020_0446);
    }

    #[test]
    fn s6_14g_short_conditional_jumps_follow_zf_and_cf() {
        // Profile forms: 75 20 (JNE at 0x102a -> 0x104c), 74 20 (JE at 0x1182 -> 0x11a4),
        // 72 47 (JB at 0x1152 -> 0x119b). Target = IP_next + rel8.
        let cases: &[(u16, [u8; 2], u32, bool, u16)] = &[
            // (ip, bytes, eflags, taken, target)
            (0x102A, [0x75, 0x20], 0x02, true, 0x104C), // ZF = 0
            (0x102A, [0x75, 0x20], 0x42, false, 0x102C), // ZF = 1
            (0x1182, [0x74, 0x20], 0x42, true, 0x11A4),
            (0x1182, [0x74, 0x20], 0x02, false, 0x1184),
            (0x1152, [0x72, 0x47], 0x03, true, 0x119B), // CF = 1
            (0x1152, [0x72, 0x47], 0x02, false, 0x1154),
            // CF = 1 alone does not affect JNE/JE; ZF = 1 alone does not affect JB.
            (0x1000, [0x75, 0x04], 0x03, true, 0x1006),
            (0x1000, [0x74, 0x04], 0x03, false, 0x1002),
            (0x1000, [0x72, 0x04], 0x42, false, 0x1002),
            // Backward: 0x1002 + (-6) = 0x0FFC.
            (0x1000, [0x75, 0xFA], 0x02, true, 0x0FFC),
        ];
        for &(ip, bytes, eflags, taken, target) in cases {
            let (mut cpu, mut ram) = stack_setup(ip, &bytes, 0x6020);
            cpu.mode.eflags = eflags;
            let mut m = ImageMemoryMut {
                base: 0,
                bytes: &mut ram,
            };
            let r = cpu.step(&mut m).unwrap();
            assert!(matches!(r, RealInstruction::JccShort { .. }));
            assert_eq!(
                cpu.mode.eip,
                u32::from(target),
                "{ip:#x} {bytes:x?} {eflags:#x}"
            );
            assert_eq!(cpu.mode.eflags, eflags, "flags are only read");
            assert_eq!(
                taken,
                cpu.mode.eip != u32::from(ip) + 2,
                "taken = IP is not next"
            );
        }
    }

    #[test]
    fn s6_14g_near_conditional_jumps_use_rel16_and_wrap_at_16_bits() {
        // 0f 84 fb 00 at 0x109c: 0x10a0 + 0xfb = 0x119b (JE taken when ZF = 1);
        // 0f 82 eb 00 at 0x10ac: 0x10b0 + 0xeb = 0x119b (JB taken when CF = 1).
        for (ip, bytes, eflags, expected) in [
            (0x109C_u16, [0x0F, 0x84, 0xFB, 0x00], 0x42_u32, 0x119B_u32),
            (0x109C, [0x0F, 0x84, 0xFB, 0x00], 0x02, 0x10A0),
            (0x10AC, [0x0F, 0x82, 0xEB, 0x00], 0x03, 0x119B),
            (0x10AC, [0x0F, 0x82, 0xEB, 0x00], 0x02, 0x10B0),
            // Negative rel16: 0x2004 - 0x0010 = 0x1FF4.
            (0x2000, [0x0F, 0x84, 0xF0, 0xFF], 0x42, 0x1FF4),
            // 16-bit wrap: next = 0xFFF4, + 0x0020 = 0x10014 -> 0x0014.
            (0xFFF0, [0x0F, 0x82, 0x20, 0x00], 0x03, 0x0014),
        ] {
            let (mut cpu, mut ram) = stack_setup(ip, &bytes, 0x6020);
            cpu.mode.eflags = eflags;
            let mut m = ImageMemoryMut {
                base: 0,
                bytes: &mut ram,
            };
            let r = cpu.step(&mut m).unwrap();
            assert!(matches!(r, RealInstruction::JccNear { .. }));
            assert_eq!(cpu.mode.eip, expected, "{ip:#x} {bytes:x?}");
            assert_eq!(cpu.mode.eflags, eflags);
        }
    }

    #[test]
    fn s6_14g_taken_jump_beyond_cs_limit_is_refused_not_taken_is_not() {
        // (bytes, EFLAGS that take the jump, EFLAGS that do not)
        for (bytes, taken, not_taken) in [
            (&[0x75u8, 0x20][..], 0x02u32, 0x42u32),
            (&[0x74, 0x20][..], 0x42, 0x02),
            (&[0x72, 0x20][..], 0x03, 0x02),
            (&[0x0F, 0x84, 0x20, 0x00][..], 0x42, 0x02),
            (&[0x0F, 0x82, 0x20, 0x00][..], 0x03, 0x02),
        ] {
            // Taken: the target is beyond the CS limit 0x1010.
            let (before, cpu, ram, r) = g_step(bytes, |c, _| {
                c.mode.cs.limit = 0x1010;
                c.mode.eflags = taken;
            });
            assert!(
                matches!(
                    r,
                    Err(RealModeRefusal::TargetBeyondLimit { limit: 0x1010, .. })
                ),
                "{bytes:x?}: {r:?}"
            );
            assert_eq!(cpu, before);
            assert_eq!(ram[0x1000..0x1000 + bytes.len()], *bytes);
            // Not taken: the same target is never checked.
            let (_, cpu, _, r) = g_step(bytes, |c, _| {
                c.mode.cs.limit = 0x1010;
                c.mode.eflags = not_taken;
            });
            assert!(r.is_ok(), "{bytes:x?}: {r:?}");
            assert_eq!(cpu.mode.eip, 0x1000 + bytes.len() as u32);
        }
    }

    #[test]
    fn s6_14g_pushfd_stores_eflags_without_vm_and_rf() {
        // 66 9c. Input EFLAGS = 0x0127_0013: bits 24, 21 (ID), 18 (AC), 17 (VM), 16 (RF), 4, 1,
        // 0. By hand: the image is EFLAGS AND 0x00FCFFFF = 0x0024_0013. SP -= 4.
        let (before, cpu, ram, r) = g_step(&[0x66, 0x9C], |c, _| {
            c.mode.eflags = 0x0127_0013;
            c.gpr[ESP] = 0x0001_6020;
        });
        assert_eq!(r, Ok(RealInstruction::Pushfd));
        assert_eq!(cpu.gpr[ESP], 0x0001_601C, "upper half of ESP kept");
        assert_eq!(dword_at(&ram, 0x601C), 0x0024_0013);
        assert_eq!(cpu.mode.eflags, before.mode.eflags, "flags unchanged");
        assert_eq!(cpu.mode.eip, 0x1002);
        // The slot above (the old top) is untouched.
        assert_eq!(dword_at(&ram, 0x6020), 0);
    }

    #[test]
    fn s6_14g_popfd_writes_only_its_mask() {
        // 66 9d. Word 0x8024_CCFD: bit 31 and the reserved bits 3, 5, 15 are junk; ID (21) and AC
        // (18) are set; 0xCCFD & 0x7FD5 = 0x4CD5 (NT, OF, DF, SF, ZF, AF, PF, CF; no TF/IF).
        // Mask 0x7FD5 | AC | ID = 0x0024_7FD5. Input EFLAGS 0x0001_0002 (RF = 1). Result:
        // RF cleared (SDM, from memory) | bit 1 | 0x0024_4CD5 = 0x0024_4CD7. SP += 4.
        let (_, cpu, ram, r) = g_step(&[0x66, 0x9D], |c, ram| {
            c.mode.eflags = 0x0001_0002;
            c.gpr[ESP] = 0x0002_6020;
            ram[0x6020..0x6024].copy_from_slice(&0x8024_CCFDu32.to_le_bytes());
        });
        assert_eq!(r, Ok(RealInstruction::Popfd));
        assert_eq!(cpu.mode.eflags, 0x0024_4CD7);
        assert_eq!(cpu.gpr[ESP], 0x0002_6024);
        assert_eq!(cpu.mode.eip, 0x1002);
        assert_eq!(dword_at(&ram, 0x6020), 0x8024_CCFD, "memory untouched");
        // Popping zero clears every modifiable bit (ID, AC and the arithmetic flags included)
        // and leaves bit 1; the input RF = 1 is cleared as well.
        let (_, cpu, _, r) = g_step(&[0x66, 0x9D], |c, _| {
            c.mode.eflags = 0x0025_4CD7 | 0x0001_0000;
        });
        assert!(r.is_ok());
        assert_eq!(cpu.mode.eflags, 0x0000_0002);
        // A popped word with RF = 1 is not refused: RF ends 0 (input RF = 1 or 0), CF from the
        // word is written.
        for input in [0x0001_0002u32, 0x0000_0002] {
            let (_, cpu, _, r) = g_step(&[0x66, 0x9D], |c, ram| {
                ram[0x6020..0x6024].copy_from_slice(&0x0001_0001u32.to_le_bytes());
                c.mode.eflags = input;
            });
            assert_eq!(r, Ok(RealInstruction::Popfd));
            assert_eq!(cpu.mode.eflags, 0x0000_0003, "{input:#x}");
        }
        // VM, VIF and VIP in the input flags are kept (the popped word has none of them).
        let (_, cpu, _, r) = g_step(&[0x66, 0x9D], |c, _| {
            c.mode.eflags = 0x0000_0002 | (1 << 17) | (1 << 19) | (1 << 20) | (1 << 16);
        });
        assert!(r.is_ok());
        assert_eq!(
            cpu.mode.eflags,
            0x0000_0002 | (1 << 17) | (1 << 19) | (1 << 20)
        );
    }

    #[test]
    fn s6_14g_popfd_with_unmodelled_flags_is_refused_unchanged() {
        // TF (8), IF (9), VM (17), VIF (19), VIP (20). RF (16) is not here: it is cleared.
        for word in [0x100u32, 0x200, 0x2_0000, 0x8_0000, 0x10_0000] {
            let (before, cpu, ram, r) = g_step(&[0x66, 0x9D], |c, ram| {
                ram[0x6020..0x6024].copy_from_slice(&word.to_le_bytes());
                c.mode.eflags = 0x0000_0002;
            });
            assert_eq!(
                r,
                Err(RealModeRefusal::PopfdUnmodelledFlags {
                    cs: 0,
                    ip: 0x1000,
                    flags: word
                }),
                "{word:#x}"
            );
            assert_eq!(cpu, before);
            assert_eq!(dword_at(&ram, 0x6020), word);
        }
    }

    #[test]
    fn s6_14g_push_eax_and_pop_eax_move_one_dword() {
        // 66 50: [SP-4..SP) = EAX little-endian, SP = 0x601c. Flags unchanged.
        let (before, cpu, ram, r) = g_step(&[0x66, 0x50], |c, _| {
            c.gpr[EAX] = 0xDEAD_BEEF;
            c.mode.eflags = ALL_ARITH;
        });
        assert_eq!(r, Ok(RealInstruction::PushEax));
        assert_eq!(&ram[0x601C..0x6020], &[0xEF, 0xBE, 0xAD, 0xDE]);
        assert_eq!(cpu.gpr[ESP], 0x601C);
        assert_eq!(cpu.mode.eflags, before.mode.eflags);
        assert_eq!(cpu.mode.eip, 0x1002);
        // 66 58: EAX = the dword at SS:SP, SP = 0x6024, flags unchanged.
        let (before, cpu, _, r) = g_step(&[0x66, 0x58], |c, ram| {
            c.mode.eflags = ALL_ARITH;
            ram[0x6020..0x6024].copy_from_slice(&[0x78, 0x56, 0x34, 0x12]);
        });
        assert_eq!(r, Ok(RealInstruction::PopEax));
        assert_eq!(cpu.gpr[EAX], 0x1234_5678);
        assert_eq!(cpu.gpr[ESP], 0x6024);
        assert_eq!(cpu.mode.eflags, before.mode.eflags);
        // Round trip through the stack (push, clobber, pop) restores EAX and SP.
        let (mut cpu, mut ram) = stack_setup(0x1000, &[0x66, 0x50, 0x66, 0x58], 0x6020);
        cpu.gpr[EAX] = 0xCAFE_F00D;
        let mut m = ImageMemoryMut {
            base: 0,
            bytes: &mut ram,
        };
        cpu.step(&mut m).unwrap();
        cpu.gpr[EAX] = 0;
        cpu.step(&mut m).unwrap();
        assert_eq!((cpu.gpr[EAX], cpu.gpr[ESP]), (0xCAFE_F00D, 0x6020));
    }

    #[test]
    fn s6_14g_dword_stack_must_fit_under_the_ss_limit_and_does_not_wrap() {
        // (push form, the dword it stores for EAX = 0x01020304 and EFLAGS = 2)
        for (push, stored) in [([0x66u8, 0x50], 0x0102_0304u32), ([0x66, 0x9C], 2)] {
            // Push at SP = 0x6020: bytes 0x601c..=0x601f. Limit 0x601f: fits; 0x601e: refused,
            // first offset beyond = 0x601f.
            let (_, cpu, _, r) = g_step(&push, |c, _| {
                c.gpr[EAX] = 0x0102_0304;
                c.mode.ss.limit = 0x601F;
            });
            assert!(r.is_ok(), "{push:x?}");
            assert_eq!(cpu.gpr[ESP], 0x601C);
            let (before, cpu, _, r) = g_step(&push, |c, _| c.mode.ss.limit = 0x601E);
            assert_eq!(
                r,
                Err(RealModeRefusal::DataBeyondLimit {
                    cs: 0,
                    ip: 0x1000,
                    offset: 0x601F
                })
            );
            assert_eq!(cpu, before);
            // SP = 0: the word is at 0xfffc..=0xffff and fits (ESP upper half kept).
            let (_, cpu, ram, r) = g_step(&push, |c, _| {
                c.gpr[EAX] = 0x0102_0304;
                c.gpr[ESP] = 0x0003_0000;
            });
            assert!(r.is_ok());
            assert_eq!(cpu.gpr[ESP], 0x0003_FFFC);
            assert_eq!(dword_at(&ram, 0xFFFC), stored);
            // SP = 2: new SP = 0xfffe, bytes 0xfffe..=0x10001 do not fit; SP = 1, 3 as well.
            for sp in [1u32, 2, 3] {
                let (before, cpu, _, r) = g_step(&push, |c, _| c.gpr[ESP] = sp);
                assert_eq!(
                    r,
                    Err(RealModeRefusal::DataBeyondLimit {
                        cs: 0,
                        ip: 0x1000,
                        offset: 0x1_0000
                    }),
                    "SP = {sp}"
                );
                assert_eq!(cpu, before);
            }
        }
        // Pop at SP = 0x6020 with limit 0x6023 fits; 0x6022 does not (first beyond = 0x6023).
        let (_, cpu, _, r) = g_step(&[0x66, 0x58], |c, _| c.mode.ss.limit = 0x6023);
        assert!(r.is_ok());
        assert_eq!(cpu.gpr[ESP], 0x6024);
        let (before, cpu, _, r) = g_step(&[0x66, 0x58], |c, _| c.mode.ss.limit = 0x6022);
        assert_eq!(
            r,
            Err(RealModeRefusal::DataBeyondLimit {
                cs: 0,
                ip: 0x1000,
                offset: 0x6023
            })
        );
        assert_eq!(cpu, before);
        // SP = 0xfffc: fits, SP wraps to 0; SP = 0xfffd..0xffff: refused (no wrap in the word).
        let (_, cpu, _, r) = g_step(&[0x66, 0x58], |c, _| c.gpr[ESP] = 0x0004_FFFC);
        assert!(r.is_ok());
        assert_eq!(cpu.gpr[ESP], 0x0004_0000);
        for sp in [0xFFFDu32, 0xFFFE, 0xFFFF] {
            for pop in [[0x66u8, 0x58], [0x66, 0x9D]] {
                let (before, cpu, _, r) = g_step(&pop, |c, _| c.gpr[ESP] = sp);
                assert_eq!(
                    r,
                    Err(RealModeRefusal::DataBeyondLimit {
                        cs: 0,
                        ip: 0x1000,
                        offset: 0x1_0000
                    }),
                    "SP = {sp:#x} {pop:x?}"
                );
                assert_eq!(cpu, before);
            }
        }
    }

    #[test]
    fn s6_14g_dword_stack_unreadable_or_unwritable_is_refused_unchanged() {
        // Read-only memory: a push is DataUnwritable at the lowest byte (0x601c).
        let code = [0x66u8, 0x50];
        let image: Vec<u8> = {
            let mut v = vec![0u8; 0x6100];
            v[0x1000..0x1002].copy_from_slice(&code);
            v
        };
        let mut cpu = RealModeCpu::at_cs_ip(0, 0x1000, 0, 0);
        cpu.gpr[ESP] = 0x6020;
        let before = cpu;
        let mut ro = ImageMemory {
            base: 0,
            bytes: &image,
        };
        assert_eq!(
            cpu.step(&mut ro),
            Err(RealModeRefusal::DataUnwritable {
                cs: 0,
                ip: 0x1000,
                linear: 0x601C
            })
        );
        assert_eq!(cpu, before);
        // Writable only below 0x601e: the third byte of the dword is the first unwritable one.
        let mut ram = image.clone();
        let mut part = PartlyReadOnly {
            inner: ImageMemoryMut {
                base: 0,
                bytes: &mut ram,
            },
            read_only_from: 0x601E,
        };
        assert_eq!(
            cpu.step(&mut part),
            Err(RealModeRefusal::DataUnwritable {
                cs: 0,
                ip: 0x1000,
                linear: 0x601E
            })
        );
        assert_eq!(cpu, before);
        assert_eq!(&ram[0x601C..0x6020], &[0; 4], "no partial write");
        // Pop from an image that ends inside the dword: DataUnreadable at the first missing byte.
        for (pop, name) in [([0x66u8, 0x58], "POP EAX"), ([0x66, 0x9D], "POPFD")] {
            let mut short = vec![0u8; 0x6022];
            short[0x1000..0x1002].copy_from_slice(&pop);
            let mut cpu = RealModeCpu::at_cs_ip(0, 0x1000, 0, 0);
            cpu.gpr[ESP] = 0x6020;
            let before = cpu;
            let mut m = ImageMemory {
                base: 0,
                bytes: &short,
            };
            assert_eq!(
                cpu.step(&mut m),
                Err(RealModeRefusal::DataUnreadable {
                    cs: 0,
                    ip: 0x1000,
                    linear: 0x6022
                }),
                "{name}"
            );
            assert_eq!(cpu, before);
        }
    }

    #[test]
    fn s6_14g_mov_ebx_eax_copies_the_register_and_keeps_flags() {
        let (before, cpu, _, r) = g_step(&[0x66, 0x89, 0xC3], |c, _| {
            c.gpr[EAX] = 0x1234_5678;
            c.gpr[EBX] = 0xFFFF_FFFF;
            c.mode.eflags = ALL_ARITH;
        });
        assert_eq!(r, Ok(RealInstruction::MovEbxEax));
        assert_eq!(cpu.gpr[EBX], 0x1234_5678);
        assert_eq!(cpu.gpr[EAX], 0x1234_5678);
        assert_eq!(cpu.mode.eflags, before.mode.eflags);
        assert_eq!(cpu.mode.eip, 0x1003);
        let mut expected = before.gpr;
        expected[EBX] = 0x1234_5678;
        assert_eq!(cpu.gpr, expected);
    }

    #[test]
    fn s6_14g_cmp_ebx_eax_sets_subtraction_flags_and_writes_nothing() {
        // 66 39 c3: flags of EBX - EAX. By hand (SDM): CF = borrow, OF = signed overflow, AF =
        // borrow out of bit 3, SF/ZF/PF from the result. Input flags 0x8d7 are fully rewritten.
        for (ebx, eax, expected) in [
            (5u32, 5u32, 0x46u32),             // 0: ZF, PF
            (1, 2, 0x97),                      // 0xffffffff: CF, PF (8 ones), AF, SF
            (0x8000_0000, 1, 0x816),           // 0x7fffffff: PF, AF, OF (neg - pos = pos)
            (0, 0x8000_0000, 0x887),           // 0x80000000: CF, PF, SF, OF
            (0x6E65_704F, 0x6874_7541, 0x002), // 0x05f0fb0e: no flag
        ] {
            let (_, cpu, _, r) = g_step(&[0x66, 0x39, 0xC3], |c, _| {
                c.gpr[EBX] = ebx;
                c.gpr[EAX] = eax;
                c.mode.eflags = ALL_ARITH;
            });
            assert_eq!(r, Ok(RealInstruction::CmpEbxEax));
            assert_eq!(cpu.mode.eflags, expected, "{ebx:#x} - {eax:#x}");
            assert_eq!((cpu.gpr[EBX], cpu.gpr[EAX]), (ebx, eax));
            assert_eq!(cpu.mode.eip, 0x1003);
        }
        // The profile's ID probe: 2 vs 0x200002 differ, so ZF = 0 (JE not taken).
        let (_, cpu, _, _) = g_step(&[0x66, 0x39, 0xC3], |c, _| {
            c.gpr[EBX] = 2;
            c.gpr[EAX] = 0x0020_0002;
            c.mode.eflags = 0x0000_0002;
        });
        assert_eq!(cpu.mode.eflags & EFLAGS_ZF, 0);
        // Flags outside the six are kept (DF, ID).
        let (_, cpu, _, _) = g_step(&[0x66, 0x39, 0xC3], |c, _| {
            c.mode.eflags = 0x0020_0402;
        });
        assert_eq!(cpu.mode.eflags, 0x0020_0446);
    }

    #[test]
    fn s6_14g_cmp_eax_imm8_sign_extends_the_immediate() {
        // 66 83 f8 ib. EAX = 1, imm 1: equal -> 0x46. EAX = 0, imm8 0xff = -1 = 0xffffffff:
        // 0 - 0xffffffff = 1: CF, AF, no SF (a zero-extended 0xff would give 0xffffff01, SF = 1).
        let (_, cpu, _, r) = g_step(&[0x66, 0x83, 0xF8, 0x01], |c, _| {
            c.gpr[EAX] = 1;
            c.mode.eflags = ALL_ARITH;
        });
        assert_eq!(
            r,
            Ok(RealInstruction::Alu32Imm {
                op: Alu32Op::Cmp,
                reg: EAX,
                imm: 1
            })
        );
        assert_eq!(cpu.mode.eflags, 0x46);
        assert_eq!(cpu.mode.eip, 0x1004);
        let (_, cpu, _, r) = g_step(&[0x66, 0x83, 0xF8, 0xFF], |c, _| {
            c.gpr[EAX] = 0;
            c.mode.eflags = 0x2;
        });
        assert_eq!(
            r,
            Ok(RealInstruction::Alu32Imm {
                op: Alu32Op::Cmp,
                reg: EAX,
                imm: 0xFFFF_FFFF
            })
        );
        assert_eq!(cpu.mode.eflags, 0x13);
        assert_eq!(cpu.gpr[EAX], 0, "CMP writes nothing");
        // verify_cpu's `cmp eax,1` with the leaf-0 maximum 0: 0 - 1 -> CF (JB taken).
        let (_, cpu, _, _) = g_step(&[0x66, 0x83, 0xF8, 0x01], |c, _| c.gpr[EAX] = 0);
        assert_eq!(cpu.mode.eflags & EFLAGS_CF, EFLAGS_CF);
    }

    #[test]
    fn s6_14g_alu32_imm_forms_write_and_flag_by_the_sdm() {
        // (bytes, register, input, expected register, expected EFLAGS from ALL_ARITH).
        let cases: &[(&[u8], usize, u32, u32, u32)] = &[
            // 66 35: XOR EAX, 0x200000: 1 ^ 0x200000 = 0x200001; low byte odd: PF = 0 -> 0x2.
            (
                &[0x66, 0x35, 0x00, 0x00, 0x20, 0x00],
                EAX,
                1,
                0x0020_0001,
                0x2,
            ),
            // XOR with a zero immediate keeps 0x80000000: SF, PF (low byte 0).
            (
                &[0x66, 0x35, 0, 0, 0, 0],
                EAX,
                0x8000_0000,
                0x8000_0000,
                0x86,
            ),
            // 66 81 e2: AND EDX, 0x7008161 with 0x781a379 -> 0x7008161: low byte 0x61 has 3
            // ones, PF = 0, no SF/ZF -> 0x2.
            (
                &[0x66, 0x81, 0xE2, 0x61, 0x81, 0x00, 0x07],
                EDX,
                0x0781_A379,
                0x0700_8161,
                0x2,
            ),
            // 66 81 f2: XOR EDX, 0x7008161 with 0x7008161 -> 0: ZF, PF.
            (
                &[0x66, 0x81, 0xF2, 0x61, 0x81, 0x00, 0x07],
                EDX,
                0x0700_8161,
                0,
                0x46,
            ),
            // 66 81 fb: CMP EBX, "Auth" (0x68747541) with EBX = "Open" (0x6e65704f):
            // 0x05f0fb0e, no flag (low byte 0x0e has 3 ones).
            (
                &[0x66, 0x81, 0xFB, 0x41, 0x75, 0x74, 0x68],
                EBX,
                0x6E65_704F,
                0x6E65_704F,
                0x2,
            ),
            // 66 81 fa: CMP EDX, 0x6000000 with 0x6000000: equal.
            (
                &[0x66, 0x81, 0xFA, 0x00, 0x00, 0x00, 0x06],
                EDX,
                0x0600_0000,
                0x0600_0000,
                0x46,
            ),
            // 66 3d: CMP EAX, 0x80000001 with 0x80000008: 7 -> PF only? 7 = 111b: 3 ones, odd:
            // PF = 0, no CF/AF? AF: (0x80000008 ^ 0x80000001 ^ 7) & 0x10 = 0 -> 0x2.
            (
                &[0x66, 0x3D, 0x01, 0x00, 0x00, 0x80],
                EAX,
                0x8000_0008,
                0x8000_0008,
                0x2,
            ),
            // CMP EAX, 0x80000001 with 0x7fffffff: 0xfffffffe: CF, SF, PF (low byte 0xfe has
            // 7 ones: odd -> PF = 0), AF: (0x7fffffff ^ 0x80000001 ^ 0xfffffffe) = 0 -> 0;
            // OF: (0xfffffffe & (0x7fffffff ^ 0xfffffffe = 0x80000001)) bit 31 = 1.
            (
                &[0x66, 0x3D, 0x01, 0x00, 0x00, 0x80],
                EAX,
                0x7FFF_FFFF,
                0x7FFF_FFFF,
                0x0883,
            ),
        ];
        for &(code, reg, input, expected_reg, expected_flags) in cases {
            let (before, cpu, _, r) = g_step(code, |c, _| {
                c.gpr[reg] = input;
                c.mode.eflags = ALL_ARITH;
            });
            assert!(
                matches!(r, Ok(RealInstruction::Alu32Imm { .. })),
                "{code:x?}"
            );
            assert_eq!(cpu.gpr[reg], expected_reg, "{code:x?}");
            assert_eq!(cpu.mode.eflags, expected_flags, "{code:x?}");
            assert_eq!(cpu.mode.eip, 0x1000 + code.len() as u32);
            let mut expected = before.gpr;
            expected[reg] = expected_reg;
            assert_eq!(cpu.gpr, expected, "only the destination changes: {code:x?}");
        }
    }

    #[test]
    fn s6_14g_xor_di_di_clears_di_only() {
        // 31 ff: DI = 0, EDI[31:16] kept; result 0: ZF, PF set, CF/OF/SF/AF clear.
        let (before, cpu, _, r) = g_step(&[0x31, 0xFF], |c, _| {
            c.gpr[EDI] = 0xABCD_1234;
            c.mode.eflags = ALL_ARITH;
        });
        assert_eq!(r, Ok(RealInstruction::XorDiDi));
        assert_eq!(cpu.gpr[EDI], 0xABCD_0000);
        assert_eq!(cpu.mode.eflags, 0x46);
        assert_eq!(cpu.mode.eip, 0x1002);
        let mut expected = before.gpr;
        expected[EDI] = 0xABCD_0000;
        assert_eq!(cpu.gpr, expected);
    }

    #[test]
    fn s6_14g_cpuid_returns_the_profile_for_the_four_leaves_and_refuses_others() {
        let stage4a = CpuidSource::Profile(CpuidProfile::X8664V1Stage4A);
        // (leaf, expected EAX, EBX, ECX, EDX) from the cpuid.rs profile (read from the file).
        let open = u32::from_le_bytes(*b"Open");
        let top = u32::from_le_bytes(*b"top ");
        let desk = u32::from_le_bytes(*b"Desk");
        for (leaf, regs) in [
            (0u32, [1, open, top, desk]),
            (1, [0x600, 0x100, 0, 0x0100_0011]),
            (0x8000_0000, [0x8000_0008, 0, 0, 0]),
            (0x8000_0001, [0, 0, 0, 0]),
        ] {
            let (before, cpu, _, r) = g_step(&[0x0F, 0xA2], |c, _| {
                c.cpuid = stage4a;
                c.gpr[EAX] = leaf;
                c.gpr[ECX] = 0x1234_5678; // the subleaf is ignored
                c.gpr[EDI] = 0x5555_5555;
                c.mode.eflags = ALL_ARITH;
            });
            assert_eq!(r, Ok(RealInstruction::Cpuid));
            assert_eq!(
                [cpu.gpr[EAX], cpu.gpr[EBX], cpu.gpr[ECX], cpu.gpr[EDX]],
                regs,
                "leaf {leaf:#x}"
            );
            assert_eq!(cpu.gpr[EDI], 0x5555_5555);
            assert_eq!(cpu.mode.eflags, before.mode.eflags, "CPUID changes no flag");
            assert_eq!(cpu.mode.eip, 0x1002);
        }
        // A table replaces the profile; a leaf missing from it reads as zeros.
        static TABLE: [(u32, CpuidResult); 1] = [(
            1,
            CpuidResult {
                eax: 7,
                ebx: 8,
                ecx: 9,
                edx: 10,
            },
        )];
        let (_, cpu, _, _) = g_step(&[0x0F, 0xA2], |c, _| {
            c.cpuid = CpuidSource::Leaves(&TABLE);
            c.gpr[EAX] = 1;
        });
        assert_eq!(
            [cpu.gpr[EAX], cpu.gpr[EBX], cpu.gpr[ECX], cpu.gpr[EDX]],
            [7, 8, 9, 10]
        );
        let (_, cpu, _, _) = g_step(&[0x0F, 0xA2], |c, _| {
            c.cpuid = CpuidSource::Leaves(&TABLE);
            c.gpr[EAX] = 0;
            c.gpr[EBX] = 99;
        });
        assert_eq!(
            [cpu.gpr[EAX], cpu.gpr[EBX], cpu.gpr[ECX], cpu.gpr[EDX]],
            [0; 4]
        );
        // Leaves outside {0, 1, 0x80000000, 0x80000001} are refused and nothing changes.
        for leaf in [
            2u32,
            7,
            0xB,
            0x8000_0002,
            0x8000_0008,
            0x4000_0000,
            u32::MAX,
        ] {
            let (before, cpu, _, r) = g_step(&[0x0F, 0xA2], |c, _| c.gpr[EAX] = leaf);
            assert_eq!(
                r,
                Err(RealModeRefusal::CpuidLeafUnmodelled {
                    cs: 0,
                    ip: 0x1000,
                    leaf
                })
            );
            assert_eq!(cpu, before);
        }
    }

    #[test]
    fn s6_14g_new_forms_end_a_mov_ss_shadow() {
        let (mut cpu, mut ram) = stack_setup(0x1000, &[0x66, 0x85, 0xC0, 0x66, 0x85, 0xC0], 0x6020);
        cpu.mov_ss_shadow = true;
        let mut m = ImageMemoryMut {
            base: 0,
            bytes: &mut ram,
        };
        cpu.step(&mut m).unwrap();
        assert!(!cpu.mov_ss_shadow);
    }

    #[test]
    fn s6_14g_near_misses_are_refused() {
        let cases: &[(&[u8], &str)] = &[
            (&[0x85, 0xC0], "TEST AX,AX"),
            (&[0x66, 0x85, 0xC9], "TEST ECX,ECX"),
            (&[0x66, 0x85, 0xD8], "TEST EAX,EBX"),
            (&[0x85, 0xFF], "TEST DI,DI (Intel/AMD branch)"),
            (&[0x66, 0xA9, 0, 0, 0, 0], "TEST EAX,imm32"),
            (&[0x89, 0xC3], "MOV BX,AX"),
            (&[0x66, 0x89, 0xD8], "MOV EAX,EBX"),
            (&[0x66, 0x89, 0xC1], "MOV ECX,EAX (Intel branch)"),
            (&[0x66, 0x39, 0xD8], "CMP EAX,EBX"),
            (&[0x66, 0x3B, 0xC3], "CMP EAX,EBX (3b)"),
            (&[0x39, 0xC3], "CMP BX,AX"),
            (&[0x66, 0x83, 0xF9, 0x06], "CMP ECX,imm8"),
            (&[0x66, 0x83, 0xFB, 0x01], "CMP EBX,imm8"),
            (&[0x66, 0x83, 0xC0, 0x01], "ADD EAX,imm8"),
            (&[0x83, 0xF8, 0x01], "CMP AX,imm8"),
            (
                &[0x66, 0x81, 0xF9, 0, 0, 0, 0],
                "CMP ECX,imm32 (Intel/AMD branch)",
            ),
            (&[0x66, 0x81, 0xE1, 0, 0, 0, 0], "AND ECX,imm32"),
            (&[0x66, 0x81, 0xC2, 0, 0, 0, 0], "ADD EDX,imm32"),
            (&[0x66, 0x81, 0xFC, 0, 0, 0, 0], "CMP ESP,imm32"),
            (&[0x81, 0xFB, 0, 0], "CMP BX,imm16"),
            (&[0x66, 0x25, 0, 0, 0, 0], "AND EAX,imm32 (Intel branch)"),
            (&[0x66, 0x05, 0, 0, 0, 0], "ADD EAX,imm32"),
            (&[0x66, 0xC1, 0xE8, 0x08], "SHR EAX,8 (Intel branch)"),
            (&[0x66, 0x0F, 0xBA, 0xF2, 0x02], "BTR EDX,2 (Intel branch)"),
            (&[0x0F, 0x32], "RDMSR"),
            (&[0x0F, 0x30], "WRMSR"),
            (&[0x0F, 0x83, 0, 0], "JAE rel16"),
            (&[0x0F, 0x85, 0, 0], "JNE rel16"),
            (&[0x66, 0x0F, 0x84, 0, 0], "66 JE rel16"),
            (&[0x66, 0x74, 0x00], "66 JE rel8"),
            (&[0x77, 0x13], "JA (Intel branch)"),
            (&[0x66, 0x0F, 0xA2], "66 CPUID"),
            (&[0xF3, 0x0F, 0xA2], "F3 CPUID"),
            (&[0x31, 0xC0], "XOR AX,AX"),
            (&[0x31, 0xF6], "XOR SI,SI"),
            (&[0x66, 0x31, 0xFF], "XOR EDI,EDI"),
            (&[0x33, 0xFF], "XOR DI,DI (33)"),
            (&[0x66, 0x9B], "66 FWAIT"),
            (&[0x66, 0x53], "PUSH EBX"),
            (&[0x66, 0x5B], "POP EBX"),
            (&[0x66, 0x51], "PUSH ECX"),
            (&[0x50], "PUSH AX"),
            (&[0x58], "POP AX"),
            (&[0x67, 0x66, 0x50], "67 PUSH EAX"),
            (&[0xF0, 0x66, 0x9C], "LOCK PUSHFD"),
            (&[0x2E, 0x66, 0x9D], "CS POPFD"),
            (&[0xBF, 0x01, 0x00], "MOV DI,1 (AMD branch)"),
            (
                &[0x66, 0xB9, 0x15, 0x00, 0x01, 0xC0],
                "MOV ECX,imm32 (MSR branch)",
            ),
        ];
        for (bytes, name) in cases {
            let (before, cpu, ram, r) = g_step(bytes, |_, _| {});
            let mut expected_bytes = bytes.to_vec();
            expected_bytes.resize(UNSUPPORTED_BYTES_MAX, 0); // RAM is zero past the form
            assert_eq!(
                r,
                Err(RealModeRefusal::Unsupported {
                    cs: 0,
                    ip: 0x1000,
                    linear: 0x1000,
                    bytes: expected_bytes,
                }),
                "{name}"
            );
            assert_eq!(cpu, before, "{name}: state unchanged");
            assert_eq!(ram[0x6000..0x6040], [0u8; 0x40], "{name}: stack unchanged");
        }
    }

    #[test]
    fn s6_14g_truncated_forms_are_fetch_refusals() {
        // (bytes, linear address of the first missing byte)
        for (image, linear) in [
            (&[0x66u8, 0x85][..], 0x1002u32),
            (&[0x66, 0x89], 0x1002),
            (&[0x66, 0x39], 0x1002),
            (&[0x66, 0x83, 0xF8], 0x1003),
            (&[0x66, 0x81, 0xFB, 0x41, 0x75, 0x74], 0x1006),
            (&[0x66, 0x35, 0x00, 0x00, 0x20], 0x1005),
            (&[0x66, 0x3D, 0x01], 0x1003),
            (&[0x66, 0x81], 0x1002),
            (&[0x66, 0x9C][..1], 0x1001),
            (&[0x75], 0x1001),
            (&[0x0F, 0x84, 0xFB], 0x1003),
            (&[0x0F], 0x1001),
            (&[0x31], 0x1001),
        ] {
            let mut cpu = RealModeCpu::at_cs_ip(0, 0x1000, 0, 0);
            let mut m = mem(0x1000, image);
            assert_eq!(
                cpu.step(&mut m),
                Err(RealModeRefusal::FetchUnreadable {
                    cs: 0,
                    ip: 0x1000,
                    linear
                }),
                "{image:x?}"
            );
        }
    }

    // ---- S6-14g: the AP startup path through verify_cpu, driven by the real bytes ---------
    //
    // The blob is parsed from `16bit-objdump.txt` (the S6-02 decode of the linked trampoline), so
    // the addresses, bytes and branch targets are the ones of the contract, not retyped.

    fn blob_from_objdump() -> Vec<u8> {
        let text = include_str!("../../../docs/reports/data/stage6/s6-02-x86/16bit-objdump.txt");
        let mut ram = vec![0u8; 0x1_0000];
        for line in text.lines() {
            let mut cols = line.split('\t');
            let (Some(addr), Some(bytes)) = (cols.next(), cols.next()) else {
                continue;
            };
            let Ok(addr) = usize::from_str_radix(addr.trim().trim_end_matches(':'), 16) else {
                continue;
            };
            for (i, token) in bytes.split_whitespace().enumerate() {
                ram[addr + i] = u8::from_str_radix(token, 16).expect("hex byte");
            }
        }
        ram
    }

    /// The `OpenDesktop` target table of `CPU_CONTRACT.md` ("Ubuntu Generic amd64" section),
    /// restricted to the four leaves `verify_cpu` reads. `cpuid.rs` does not implement it.
    fn contract_leaves(
        leaf1_edx: u32,
        ext_max: u32,
        ext1_edx: u32,
        max_leaf: u32,
    ) -> Vec<(u32, CpuidResult)> {
        let r = |eax, ebx, ecx, edx| CpuidResult { eax, ebx, ecx, edx };
        vec![
            (
                0,
                r(
                    max_leaf,
                    u32::from_le_bytes(*b"Open"),
                    u32::from_le_bytes(*b"top "),
                    u32::from_le_bytes(*b"Desk"),
                ),
            ),
            (1, r(0x600, 0x100, 0, leaf1_edx)),
            (0x8000_0000, r(ext_max, 0, 0, 0)),
            (0x8000_0001, r(0, 0, 0, ext1_edx)),
        ]
    }

    /// Runs the trampoline from `0000:1000` until the first refusal, with 1000 steps at most.
    /// Returns the executed instruction addresses, the final state and the stop.
    fn run_blob(cpuid: CpuidSource) -> (Vec<u32>, RealModeCpu, Vec<u8>, RealModeRefusal) {
        let mut ram = blob_from_objdump();
        let mut cpu = RealModeCpu::at_cs_ip(0, 0x1000, 0x6000_0010, 0x306A9);
        cpu.cpuid = cpuid;
        let mut trace = Vec::new();
        let mut m = ImageMemoryMut {
            base: 0,
            bytes: &mut ram,
        };
        for _ in 0..1000 {
            let ip = cpu.mode.eip;
            match cpu.step(&mut m) {
                Ok(_) => trace.push(ip),
                Err(stop) => {
                    let ram = ram.clone();
                    return (trace, cpu, ram, stop);
                }
            }
        }
        panic!("the trampoline did not stop");
    }

    /// Instruction addresses of block 1 (through `CALL verify_cpu`), with the `LOCK BTS` free
    /// (zero RAM: `JAE` taken).
    const BLOCK1_TRACE: [u32; 11] = [
        0x1000, 0x1001, 0x1003, 0x1008, 0x100A, 0x100C, 0x100E, 0x1010, 0x1018, 0x101E, 0x1024,
    ];

    /// `verify_cpu` up to the leaf-1 mask test at `0x1142` (the `OpenDesktop` vendor path: the
    /// `JNE`s at `0x10b9` and `0x10d9` are taken, so the Intel/AMD branches are skipped).
    const VERIFY_TO_1142: [u32; 27] = [
        0x1080, 0x1081, 0x1083, 0x1084, 0x1086, 0x1088, 0x108B, 0x1091, 0x1093, 0x1095, 0x1097,
        0x1099, 0x109C, 0x10A0, 0x10A6, 0x10A8, 0x10AC, 0x10B0, 0x10B2, 0x10B9, 0x10D2, 0x10D9,
        0x112C, 0x1132, 0x1134, 0x113B, 0x1142,
    ];

    #[test]
    fn s6_14g_verify_cpu_success_path_on_the_contract_target_table() {
        // The contract's target table (NOT cpuid.rs): leaf 0 EAX 1, leaf 1 EDX 0x0781A379,
        // 0x80000000 EAX 0x80000008, 0x80000001 EDX 0x20100800. By hand: 0x781a379 & 0x7008161 ==
        // 0x7008161 (xor 0: JNE at 0x1142 not taken); 0x20100800 & 0x20000000 = 0x20000000 (xor 0:
        // JNE at 0x116a not taken); 0x781a379 & 0x6000000 == 0x6000000: JE at 0x1182 taken.
        // `CpuidSource::Leaves` needs a `'static` slice: the table is leaked for the test.
        let leaves: &'static [(u32, CpuidResult)] =
            Vec::leak(contract_leaves(0x0781_A379, 0x8000_0008, 0x2010_0800, 1));
        let (trace, cpu, _, stop) = run_blob(CpuidSource::Leaves(leaves));
        let mut expected: Vec<u32> = BLOCK1_TRACE.to_vec();
        expected.extend_from_slice(&VERIFY_TO_1142); // through the JNE at 0x1142 (not taken)
        expected.extend_from_slice(&[
            0x1144, 0x114A, 0x114C, 0x1152, 0x1154, 0x115A, 0x115C, 0x1163, 0x116A, 0x116C, 0x1172,
            0x1174, 0x117B, 0x1182, // JE taken -> 0x11a4
            0x11A4, 0x11A5, 0x11A8, // popf; xor eax,eax; ret -> 0x1027
            0x1027, 0x102A, // test eax,eax; jne not taken -> 0x102c
        ]);
        assert_eq!(trace, expected);
        // Stops at the LIDT of block 4 (not in this slice): `66 0f 01 1e 60 00`.
        match stop {
            RealModeRefusal::Unsupported { ip, bytes, .. } => {
                assert_eq!(ip, 0x102C);
                assert_eq!(&bytes[..4], &[0x66, 0x0F, 0x01, 0x1E]);
            }
            other => panic!("{other:?}"),
        }
        // Final state, by hand: EAX = 0 (xor), EBX = leaf 1 EBX 0x100 (last CPUID), ECX = 0, EDX
        // = 0x781a379 & 0x6000000 = 0x6000000 (AND, then CMP only), DI = 0, ESP = 0x6020 (balanced
        // call/ret and push/pop), flags from `test eax,eax` = ZF, PF; the ID bit (21) written by
        // the POPFD at 0x1093 stays set (the 16-bit POPF at 0x11a4 does not touch it).
        assert_eq!(cpu.gpr[EAX], 0);
        assert_eq!(cpu.gpr[EBX], 0x100);
        assert_eq!(cpu.gpr[ECX], 0);
        assert_eq!(cpu.gpr[EDX], 0x0600_0000);
        assert_eq!(cpu.gpr[ESP], 0x6020);
        assert_eq!(cpu.gpr[EDI], 0);
        assert_eq!(cpu.mode.eflags, 0x0020_0046);
        assert_eq!(cpu.mode.eip, 0x102C);
    }

    #[test]
    fn s6_14h_verify_cpu_success_path_on_the_neutral_profile() {
        // The same expectations as the table-from-test run above, now with the cpuid.rs profile
        // `OdX8664V1Neutral` (the contract's full target table): 0x781a379 & 0x7008161 ==
        // 0x7008161 (xor 0: JNE at 0x1142 not taken); 0x20100800 & 0x20000000 = 0x20000000 (xor
        // 0: JNE at 0x116a not taken); 0x781a379 & 0x6000000 == 0x6000000: JE at 0x1182 taken to
        // 0x11a4; verify_cpu returns 0, JNE at 0x102a not taken.
        let (trace, cpu, _, stop) = run_blob(CpuidSource::Profile(CpuidProfile::OdX8664V1Neutral));
        let mut expected: Vec<u32> = BLOCK1_TRACE.to_vec();
        expected.extend_from_slice(&VERIFY_TO_1142);
        expected.extend_from_slice(&[
            0x1144, 0x114A, 0x114C, 0x1152, 0x1154, 0x115A, 0x115C, 0x1163, 0x116A, 0x116C, 0x1172,
            0x1174, 0x117B, 0x1182, 0x11A4, 0x11A5, 0x11A8, 0x1027, 0x102A,
        ]);
        assert_eq!(trace, expected);
        assert!(!trace.contains(&0x119B), "the failure tail is not taken");
        assert!(matches!(
            stop,
            RealModeRefusal::Unsupported { ip: 0x102C, .. }
        ));
        assert_eq!(cpu.gpr[EAX], 0);
        assert_eq!(cpu.gpr[EBX], 0x100);
        assert_eq!(cpu.gpr[ECX], 0);
        assert_eq!(cpu.gpr[EDX], 0x0600_0000);
        assert_eq!(cpu.gpr[ESP], 0x6020);
        assert_eq!(cpu.mode.eflags, 0x0020_0046);
        assert_eq!(cpu.mode.eip, 0x102C);
    }

    #[test]
    fn s6_14g_verify_cpu_on_the_cpuid_rs_profiles_takes_the_failure_path() {
        // cpuid.rs declares leaf 1 EDX = FPU | TSC | FXSR (| APIC) only: no MSR, PAE, CX8, CMOV,
        // SSE or SSE2, and 0x80000001 is all zero. By hand: EDX & 0x7008161 = 0x1000001 (bits 0
        // and 24 only), xor 0x7008161 = 0x6008160 != 0, so the JNE at 0x1142 is TAKEN to 0x119b:
        // popf; mov eax,1; ret; then `test eax,eax` is nonzero and the JNE at 0x102a is taken
        // to 0x104c (HLT, not in this slice). verify_cpu fails for every profile of cpuid.rs.
        for (profile, leaf1_edx) in [
            (CpuidProfile::DiagnosticV1, 0u32),
            (CpuidProfile::X8664V1Stage4A, 0x0100_0011),
            (CpuidProfile::X8664V1Stage4AApic, 0x0100_0211),
        ] {
            let (trace, cpu, _, stop) = run_blob(CpuidSource::Profile(profile));
            let mut expected: Vec<u32> = BLOCK1_TRACE.to_vec();
            expected.extend_from_slice(&VERIFY_TO_1142); // ... 0x113B, 0x1142 (JNE taken)
            expected.extend_from_slice(&[
                0x119B, 0x119C, 0x11A2, // popf; mov eax,1; ret
                0x1027, 0x102A, // test eax,eax; jne taken
            ]);
            assert_eq!(trace, expected, "{profile:?}");
            match stop {
                RealModeRefusal::Unsupported { ip, bytes, .. } => {
                    assert_eq!(ip, 0x104C, "{profile:?}");
                    assert_eq!(bytes[0], 0xF4, "HLT");
                }
                other => panic!("{other:?}"),
            }
            // EAX = 1; EBX = leaf 1 EBX 0x100; EDX = (leaf1_edx & 0x7008161) ^ 0x7008161.
            assert_eq!(cpu.gpr[EAX], 1);
            assert_eq!(cpu.gpr[EBX], 0x100);
            assert_eq!(cpu.gpr[EDX], (leaf1_edx & 0x0700_8161) ^ 0x0700_8161);
            assert_ne!(cpu.gpr[EDX], 0);
            assert_eq!(cpu.gpr[ESP], 0x6020);
            // `test eax,eax` with EAX = 1: ZF = 0, PF = 0 (one set bit), SF = 0; ID (21) stays set.
            assert_eq!(cpu.mode.eflags, 0x0020_0002);
        }
    }

    #[test]
    fn s6_14g_verify_cpu_early_failures_by_table_leaves() {
        // (leaf 1 EDX, ext max, ext1 EDX, max leaf, the branch that sends control to 0x119b).
        let good_edx = 0x0781_A379;
        let cases: &[(u32, u32, u32, u32, u32)] = &[
            // CPUID(0).EAX = 0: `cmp eax,1` borrows (CF = 1): JB near at 0x10ac taken.
            (good_edx, 0x8000_0008, 0x2010_0800, 0, 0x10AC),
            // 0x80000000 EAX = 0x7fffffff: cmp eax,0x80000001 borrows: JB rel8 at 0x1152 taken.
            (good_edx, 0x7FFF_FFFF, 0x2010_0800, 1, 0x1152),
            // 0x80000001 EDX without LM: and = 0, xor 0x20000000 != 0: JNE at 0x116a taken.
            (good_edx, 0x8000_0008, 0, 1, 0x116A),
        ];
        for &(edx, ext, ext1, max_leaf, branch) in cases {
            let leaves: &'static [(u32, CpuidResult)] =
                Vec::leak(contract_leaves(edx, ext, ext1, max_leaf));
            let (trace, cpu, _, stop) = run_blob(CpuidSource::Leaves(leaves));
            let at = trace
                .iter()
                .position(|&a| a == branch)
                .expect("branch executed");
            assert_eq!(
                trace[at + 1],
                0x119B,
                "branch {branch:#x} jumps to the failure tail"
            );
            assert_eq!(cpu.gpr[EAX], 1);
            assert!(matches!(
                stop,
                RealModeRefusal::Unsupported { ip: 0x104C, .. }
            ));
        }
    }

    #[test]
    fn s6_14g_vendor_branches_stop_at_the_first_unsupported_form() {
        let good_edx = 0x0781_A379;
        // "GenuineIntel": 0x10d9 falls through, 0x10db and 0x10e2 pass, and 0x10e4 `cmp ecx,imm32`
        // (66 81 f9) is not in this slice. "AuthenticAMD" stops at the same form at 0x10c4.
        let word = |s: &[u8; 4]| u32::from_le_bytes(*s);
        for (vendor, stop_ip) in [
            ([word(b"Genu"), word(b"ntel"), word(b"ineI")], 0x10E4u32),
            ([word(b"Auth"), word(b"cAMD"), word(b"enti")], 0x10C4),
        ] {
            let mut leaves = contract_leaves(good_edx, 0x8000_0008, 0x2010_0800, 1);
            leaves[0].1.ebx = vendor[0];
            leaves[0].1.ecx = vendor[1];
            leaves[0].1.edx = vendor[2];
            let leaves: &'static [(u32, CpuidResult)] = Vec::leak(leaves);
            let (_, _, _, stop) = run_blob(CpuidSource::Leaves(leaves));
            assert!(
                matches!(stop, RealModeRefusal::Unsupported { ip, .. } if ip == stop_ip),
                "{stop:?}"
            );
        }
    }

    // ---- Comparison with the KVM oracle (S6-14g) ---------------------------------------
    //
    // Files in docs/reports/data/stage6/s6-14g/ (run-oracle.sh). Only fragments where CPUID
    // does not decide a branch: the host's CPUID values are never compared. Registers,
    // segments, EFLAGS, CR0, EIP and the captured stack words come from the oracle's JSON.

    macro_rules! oracle_case_g {
        ($name:ident, $stem:literal, $ip:expr, $steps:expr, $hlt_ip:expr, $skip:expr) => {
            #[test]
            fn $name() {
                let image: &[u8] = include_bytes!(concat!(
                    "../../../docs/reports/data/stage6/s6-14g/",
                    $stem,
                    ".bin"
                ));
                let json: &str = include_str!(concat!(
                    "../../../docs/reports/data/stage6/s6-14g/oracle-",
                    $stem,
                    ".json"
                ));
                let cpu = RealModeCpu::at_cs_ip(0, $ip, 0x6000_0010, oracle_signature(json));
                compare_with_oracle_skipping(image, json, cpu, (0, $ip), $steps, $hlt_ip, $skip);
            }
        };
    }

    // a: the real start of verify_cpu at its own addresses (pushf; push 0; popf; the ID-bit probe
    // with PUSHFD/POP EAX/MOV EBX,EAX/XOR EAX,imm32/PUSH EAX/POPFD/CMP EBX,EAX; JE rel16 not
    // taken because ID toggles), then `mov eax,0` and HLT at 0x10a6 where CPUID would be: 16 steps.
    oracle_case_g!(
        oracle_g_a_verify_cpu_id_probe,
        "a-verify-cpu-id-probe-0000-1000",
        0x1000,
        16,
        0x10A6,
        &[]
    );
    // b: 24 flag cases (XOR/TEST/CMP/AND forms, input flags all set or minimal) each followed by
    // PUSHFD; 144 steps.
    oracle_case_g!(
        oracle_g_b_alu_flags_matrix,
        "b-alu-flags-matrix-0000-1000",
        0x1000,
        144,
        0x121E,
        &[]
    );
    // c: JNE/JE/JB rel8, JE/JB rel16 in the three flag states and two backward jumps: 101 steps.
    oracle_case_g!(
        oracle_g_c_jcc_forms,
        "c-jcc-forms-0000-1000",
        0x1000,
        101,
        0x1170,
        &[]
    );
    // d: ESP = 0x10000, 32-bit push/pop at the 64 KiB wrap: 12 steps. The image proves the
    // 16-bit SP only; that the ESP upper half is kept for dword stack words is covered by unit
    // tests only, not by this image.
    oracle_case_g!(
        oracle_g_d_dword_stack_sp_wrap,
        "d-dword-stack-sp-wrap-0000-1000",
        0x1000,
        12,
        0x1028,
        &[]
    );
    // e: CPUID leaf 1 executed; EAX, ECX, EDX, EBX come from the host and are not compared.
    oracle_case_g!(
        oracle_g_e_cpuid_flags_and_ip_only,
        "e-cpuid-flags-and-ip-only-0000-1000",
        0x1000,
        7,
        0x101A,
        &[EAX, EBX, ECX, EDX]
    );
    // f/g: block 3 verbatim (TEST EAX,EAX; JNE 0x104c): EAX = 1 takes it, EAX = 0 does not.
    oracle_case_g!(
        oracle_g_f_test_jne_taken,
        "f-test-jne-eax1-0000-1021",
        0x1021,
        3,
        0x104C,
        &[]
    );
    oracle_case_g!(
        oracle_g_g_test_jne_not_taken,
        "g-test-jne-eax0-0000-1021",
        0x1021,
        3,
        0x102C,
        &[]
    );
}
