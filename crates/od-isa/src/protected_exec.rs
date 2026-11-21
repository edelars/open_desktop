//! Execution of block 6 of the AP trampoline in 32-bit protected mode, the `MOV CR0, EAX` that
//! enables paging, the far jump of block 7 and the 64-bit indirect jump of block 8 (S6-14k,
//! S6-14l, S6-14m, S6-14n).
//!
//! Scope is the closed set of forms of the S6-14a profile row 6 (CPU_CONTRACT.md, sections
//! "S6-14a", "S6-14k" and "S6-14l"), blob offsets `0x2000..0x2062` and the tail up to `0x206c` (`JMP 0x2065`), entered from the block 5 far
//! jump (S6-14j, [`RealModeCpu`]) with a flat 32-bit CS (`D = 1`) and no prefix byte:
//!
//! | bytes                         | instruction                              |
//! |-------------------------------|------------------------------------------|
//! | `8e d2` / `da` / `c2` / `e2` / `ea` | `MOV SS`/`DS`/`ES`/`FS`/`GS, EDX`  |
//! | `81 c4 imm32`                 | `ADD ESP, imm32`                         |
//! | `0f ba 25 disp32 imm8`        | `BT m32, imm8` (DS:disp32, absolute)     |
//! | `73 cb` / `75 cb` / `74 cb`   | `JAE` / `JNE` / `JE rel8`                |
//! | `a1 disp32` / `8b 15 disp32`  | `MOV EAX, moffs32` / `MOV EDX, m32` (DS:disp32) |
//! | `3b 05 disp32` / `3b 15 disp32` | `CMP EAX, m32` / `CMP EDX, m32` (DS:disp32) |
//! | `0f 22 e0` / `d8`             | `MOV CR4, EAX` / `MOV CR3, EAX`          |
//! | `b8 imm32` / `b9 imm32`       | `MOV EAX, imm32` / `MOV ECX, imm32`      |
//! | `0f 32` / `0f 30`             | `RDMSR` / `WRMSR`, only with `ECX = 0xC0000080` (EFER) |
//! | `0f 22 c0`                    | `MOV CR0, EAX` (S6-14m: enables paging, below) |
//! | `ea off32 sel16`              | `JMP FAR ptr16:32` (S6-14m, block 7 at `0x2065`) |
//! | `ff 25 disp32`                | `JMP QWORD PTR [rip+disp32]` (S6-14n, block 8 at `0x20f0`, 64-bit mode only) |
//!
//! Everything else is [`ProtectedRefusal::Unsupported`] with the address and bytes and no state
//! change: in particular `RDMSR`/`WRMSR` with any other `ECX` (the model has one MSR, EFER; the
//! optional SME branch reads `0xC0010010` at `0x201f`, where a run stops), other `CMP`/`MOV`
//! operand forms and every prefix byte (a `66` would select the 16-bit operand size).
//!
//! EFER (S6-14l): [`ProtectedCpu::efer`] is 0 at entry (after INIT). `RDMSR` returns it in
//! `EDX:EAX`. `WRMSR` takes `EDX:EAX`: a bit outside `SCE` (0), `LME` (8), `LMA` (10) and `NXE`
//! (11) is [`ProtectedRefusal::EferWriteReservedBits`] (the hardware `#GP(0)`; observed in the KVM
//! oracle as a shutdown without an IDT); `SCE`, `LME` and `NXE` are stored; the `LMA` bit of the
//! written value is ignored and `LMA` keeps its value (observed in the oracle: KVM drops it without
//! a fault). `WRMSR` never changes `LMA`; it becomes 1 only through `MOV CR0` with `PG = 1` while
//! `LME` and `PAE` are set (S6-14m, below). No other MSR exists.
//!
//! Paging and IA-32e (S6-14m, CPU_CONTRACT.md "S6-14m"). `MOV CR0, EAX` with `PG = 1` and `PE = 1`
//! in `EAX`, `CR0.PG = 0`, `EFER.LME = 1`, `CR4.PAE = 1` and a CS that is not a 64-bit segment
//! enters IA-32e mode: `CR0 := EAX | ET`, `EFER.LMA := 1` (the first time `LMA` changes, by the
//! processor and not by `WRMSR`), and from then on every access of the model (instruction bytes,
//! `DS` operands, GDT descriptors and their accessed-bit write) is translated by the 4-level
//! walk of [`crate::paging`] with `CR3`, `CR0.WP` and `EFER.NXE`; a walk that would fault is the
//! refusal [`ProtectedRefusal::PageFault`] (the `#PF`, never delivered). `PG = 1` without `PAE`,
//! without `PE`, with `CS.L = 1`, or with reserved `CR0` bits, `NW` without `CD` and a second `PG`
//! write are refusals (`#GP` in hardware). `PG = 1` with `LME = 0` and `PAE = 0` (32-bit paging) is
//! [`ProtectedRefusal::PagingNotModelled`]; with `PAE = 1` it is PAE paging (S2-25, below); any
//! `CR0` write with `PG = 0` is [`ProtectedRefusal::Cr0WriteUnmodelled`]. While paging is on,
//! `WRMSR` may not change `LME` and `MOV CR4` may not clear `PAE` (`#GP` in IA-32e mode,
//! [`ProtectedRefusal::Cr4PaeClearInLongMode`]; a model refusal in PAE paging). The far jump `JMP ptr16:32` (CPL 0) follows the SDM
//! Vol. 3A direct far jump rules, by memory: the descriptor must be a present code segment with
//! `DPL = 0` (conforming: `DPL <= 0`) and `RPL = 0` for a non-conforming one; in IA-32e mode a
//! descriptor with `L = 1` and `D = 1` is refused, `L = 1`, `D = 0` selects 64-bit mode (no limit
//! check, `RIP` is the zero-extended offset) and `L = 0` the compatibility mode (offset under the
//! limit); outside IA-32e mode `L = 1` is a model refusal. The model does not execute in 64-bit
//! mode beyond block 8 (S6-14n, below). `MOV CR3, EAX` stores the value (no TLB; the PDPTEs of PAE
//! paging are loaded while it is in use, see S2-25 below); `MOV CR4, EAX`
//! stores a value whose bits above bit 10 are clear and refuses any other as
//! [`ProtectedRefusal::Cr4WriteUnmodelled`] (a model refusal, not a hardware effect: per the SDM,
//! from memory, a reserved or unsupported CR4 bit is `#GP(0)`, bits 11.. name features
//! (`UMIP`, `LA57`, `VMXE`, `PCIDE`, `OSXSAVE`, ...) that the profile does not enable, and the
//! model does not gate bits 0..10 by CPUID).
//!
//! PAE paging (S2-25, model only: the SDM Vol. 3A §5.4 rules, no hardware oracle). It is in use while
//! `CR0.PG = 1`, `CR4.PAE = 1` and `EFER.LMA = 0` ([`ProtectedCpu::pae_paging`]). `MOV CR0` that sets
//! `PG` with `LME = 0` and `PAE = 1` loads the four PDPTE registers ([`ProtectedCpu::pdptes`]) from
//! `CR3 & 0xFFFF_FFE0` and stays in protected mode (`LMA` is not set); a PDPTE with `P = 1` and a
//! reserved bit (2:1, 8:5, 63:36) is `#GP(0)` ([`ProtectedRefusal::PdpteReservedBits`]) with `CR0` and
//! the registers unchanged. While PAE paging is in use `MOV CR3` loads them from the new value (the
//! same `#GP(0)`, `CR3` unchanged) and `MOV CR4` reloads them when it changes `PAE`, `PGE` or `PSE`
//! (`PAE` can only stay set: clearing it would select 32-bit paging, a refusal). Translation is
//! [`crate::paging::translate_pae`] (PDPTE register, PDE, PTE; 4 KiB and 2 MiB pages; bits 62:36 of a
//! PDE/PTE reserved; no accessed bit in a PDPTE). The enabling order of IA-32e (PAE, then CR3, then
//! `LME`, then `PG`) is the SDM Vol. 3A §12.8.5 sequence; `LME = 1` with `PG` and no `PAE` is
//! `#GP(0)`, and `LME` cannot change while `PG = 1`.
//!
//! 64-bit mode, block 8 (S6-14n, CPU_CONTRACT.md "S6-14n"). With `EFER.LMA = 1` and `CS.L = 1` the
//! closed set is the one instruction of the linked path at `0x20f0`: `ff 25 disp32`, `JMP
//! QWORD PTR [rip+disp32]` (no prefix, no REX). `RIP` is 64 bits ([`ProtectedCpu::rip`]; the low
//! half stays in `eip`, the high half in `rip_high`, nonzero only after this jump). Segmentation is
//! off: the instruction bytes and the pointer cell are at the linear address itself (the CS and DS
//! bases and limits are ignored, so a 64-bit `CS` with a nonzero base behaves like base 0); the
//! cell is `RIP of the next instruction + sign_extend(disp32)` (64-bit wrap), read as 8 bytes
//! through the paging walk of [`crate::paging`] (so an `A` bit of every entry it used is set, as for
//! any read). A cell address or a target that is not canonical (bits 63:47 not all equal, 48-bit
//! addresses; `CR4.LA57` is never set in the model) is [`ProtectedRefusal::NonCanonicalAddress`]
//! (the `#GP(0)` of hardware, by memory of the SDM Vol. 1 section 3.3.7.1); the model checks both the first
//! and the last byte of the cell. The page model covers linear addresses below 4 GiB: a cell or an
//! instruction byte above is [`ProtectedRefusal::LinearBeyond4GiB`]. After the jump `RIP` is the
//! target (`secondary_startup_64` in the linked path, `0xffffffff8139c090`) and the next step is
//! [`ProtectedRefusal::Mode64TargetEntered`]: the target is neither fetched nor executed (the
//! `CALL verify_cpu` there is only the boundary of the slice). Any other 64-bit instruction is
//! [`ProtectedRefusal::Unsupported`]; a `RIP` above 4 GiB that was not reached by this jump is
//! [`ProtectedRefusal::Mode64NotModelled`].
//!
//! Segment loads (`MOV Sreg, r32` uses the low 16 bits of the register) follow the SDM Vol. 3A
//! chapter 5 rules, from memory, with CPL = 0: a null selector, TI = 1 (the LDT is not modelled)
//! and a selector beyond the GDT limit are refused; the descriptor must have `S = 1`; `SS` needs a
//! writable data segment with `RPL = DPL = 0`; `DS`/`ES`/`FS`/`GS` need an expand-up data
//! segment with `DPL >= RPL` (readable code segments, expand-down data and null loads are model
//! refusals); `P = 0` is refused. A clear accessed bit is set in the descriptor in memory and in
//! the cache, which is checked writable first. The cache holds the selector as written (with its
//! RPL), the base, the limit after granularity, the access byte and the flags nibble. `SS.B` is
//! stored but not used: this slice has no stack access. The state changes only after every check.
//!
//! State: [`ProtectedCpu`] is built from a [`RealModeCpu`] after the block 5 jump
//! ([`ProtectedCpu::from_real_entry`]): the GPRs, EIP, EFLAGS, CR0, GDTR, IDTR and the CS cache
//! are copied; CR3 and CR4 are 0 (after INIT); DS/ES/SS/FS/GS keep their real-mode selector,
//! base and limit with the attributes the oracle reports for them (access byte `0x93`, flags
//! 0); EFER is 0. CR2, CR8 and DR7 are not part of the state; LDTR and TR are not modelled. Memory is
//! physical = linear until `CR0.PG` is set, translated by [`crate::paging`] after that (a 32-bit
//! physical address, [`RealModeMemory`]).
//!
//! Sources: Intel SDM Vol. 2 (`MOV` to/from control and segment registers, `BT`, `ADD`, `CMP`, `Jcc`,
//! `RDMSR`, `WRMSR`), Vol. 3A (IA32_EFER),
//! chapters 2 (control registers) and 5 (protection), all from memory; the observed behaviour of
//! each form is the KVM oracle of `docs/reports/data/stage6/s6-14k/` and `s6-14l/` (native
//! silicon under VMX, except where the S6-14i note on `CR0.ET` applies; `RDMSR`/`WRMSR` of EFER are
//! emulated by KVM, so the oracle shows KVM's guest-visible behaviour).

use crate::legacy_mode::{CR0_ET, ProtectedSegment};
use crate::paging::{self, AccessKind, PaeParams, PageRefusal, PagingParams, PdpteRefusal, Touch};
use crate::real_mode_exec::{
    CR0_PE, EAX, ECX, EDX, ESP, RealModeCpu, RealModeMemory, TableRegister, UNSUPPORTED_BYTES_MAX,
    logic_flags, sub_flags,
};

/// CR0.PG.
pub const CR0_PG: u32 = 1 << 31;
/// CR0.WP.
pub const CR0_WP: u32 = 1 << 16;
/// CR0 bits a `MOV CR0, EAX` may set (PE, MP, EM, TS, ET, NE, WP, AM, NW, CD, PG); the others are
/// reserved (`#GP(0)` in hardware, a refusal here).
pub const CR0_VALID_MASK: u32 = 0xE005_003F;
const CR0_NW: u32 = 0x2000_0000;
const CR0_CD: u32 = 0x4000_0000;
/// CR4.PAE.
pub const CR4_PAE: u32 = 1 << 5;
/// CR4 bits 0..10 (`VME` .. `OSXMMEXCPT`): the only bits `MOV CR4, EAX` may set in this slice.
pub const CR4_MODELLED_MASK: u32 = 0x7FF;
/// CR4.PSE.
const CR4_PSE: u32 = 1 << 4;
/// CR4.PGE.
const CR4_PGE: u32 = 1 << 7;
/// MSR index of `IA32_EFER`, the only MSR of the model.
pub const MSR_EFER: u32 = 0xC000_0080;
/// EFER.SCE (`SYSCALL` enable).
pub const EFER_SCE: u64 = 1;
/// EFER.LME (long mode enable).
pub const EFER_LME: u64 = 1 << 8;
/// EFER.LMA (long mode active): set by the processor, never by `WRMSR` (the written bit is ignored).
pub const EFER_LMA: u64 = 1 << 10;
/// EFER.NXE (no-execute enable).
pub const EFER_NXE: u64 = 1 << 11;
/// Bits a `WRMSR` of EFER may carry; any other bit of `EDX:EAX` is refused.
pub const EFER_WRITABLE_MASK: u64 = EFER_SCE | EFER_LME | EFER_LMA | EFER_NXE;
/// Access byte of a real-mode data segment register (present, DPL 0, `S = 1`, read/write,
/// accessed), as the KVM oracle reports it (`type 3`, `P 1`, `DPL 0`).
pub const REAL_MODE_DATA_ACCESS: u8 = 0x93;

const EFLAGS_CF: u32 = 1;
const EFLAGS_ZF: u32 = 1 << 6;
/// CF, PF, AF, ZF, SF, OF: rewritten by `ADD`.
const EFLAGS_ARITH: u32 = 0x8D5;
const EFLAGS_AF: u32 = 1 << 4;
const EFLAGS_OF: u32 = 1 << 11;

/// Segment register written by `MOV Sreg, EDX` in the profile (CS is not loadable this way).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtectedSegReg {
    Es,
    Ss,
    Ds,
    Fs,
    Gs,
}

/// Decoded instruction of blocks 6 and 7: the block 6 forms (including the `MOV CR0` that enables
/// paging, S6-14m) and the compatibility-mode far jump `JMP ptr16:32`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtectedInstruction {
    /// `8e /r` with `mod = 3`, `rm = EDX`: `MOV Sreg, EDX`.
    MovSegFromEdx(ProtectedSegReg),
    /// `81 c4 imm32`.
    AddEspImm32(u32),
    /// `0f ba 25 disp32 imm8`: `BT DWORD PTR ds:disp32, imm8`.
    BtDsImm8 { disp: u32, bit: u8 },
    /// `73 cb`: `JAE rel8`.
    JaeRel8(i8),
    /// `75 cb`: `JNE rel8`.
    JneRel8(i8),
    /// `74 cb`: `JE rel8`.
    JeRel8(i8),
    /// `a1 disp32`: `MOV EAX, DWORD PTR ds:disp32`.
    MovEaxFromDs(u32),
    /// `8b 15 disp32`: `MOV EDX, DWORD PTR ds:disp32`.
    MovEdxFromDs(u32),
    /// `3b 05 disp32`: `CMP EAX, DWORD PTR ds:disp32`.
    CmpEaxWithDs(u32),
    /// `3b 15 disp32`: `CMP EDX, DWORD PTR ds:disp32`.
    CmpEdxWithDs(u32),
    /// `0f 32`: `RDMSR`; only `ECX = 0xC0000080` (EFER) is modelled.
    Rdmsr,
    /// `0f 09`: no dirty cache lines in the ordered interpreter.
    Wbinvd,
    /// `0f 30`: `WRMSR`; only `ECX = 0xC0000080` (EFER) is modelled.
    Wrmsr,
    /// `0f 22 e0`.
    MovCr4Eax,
    /// `0f 22 d8`.
    MovCr3Eax,
    /// `0f 22 c0`: enables paging and IA-32e mode (see the module documentation).
    MovCr0Eax,
    /// `ea off32 sel16`: `JMP FAR ptr16:32` (32-bit operand size, no prefix).
    JmpFarPtr { selector: u16, offset: u32 },
    /// `ff 25 disp32`: `JMP QWORD PTR [rip+disp32]` in 64-bit mode (block 8, S6-14n).
    JmpIndirectRipRel { disp: i32 },
    /// `b8 imm32`.
    MovEaxImm32(u32),
    /// `b9 imm32`.
    MovEcxImm32(u32),
}

/// Why the far jump `JMP ptr16:32` was refused (`#GP`/`#NP` or a model limit).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FarJumpReason {
    /// TI = 1: the LDT is not modelled.
    LdtSelector,
    /// Null selector: `#GP(0)`.
    NullSelector,
    /// `(selector & !7) + 7` is greater than the GDT limit.
    BeyondGdtLimit { limit: u16 },
    /// A descriptor byte at `linear` is not readable in memory (or the descriptor wraps past 4 GiB).
    DescriptorUnreadable { linear: u32 },
    /// Not a code segment (`S = 0` or not executable).
    NotCodeSegment { access: u8 },
    /// `P = 0`: `#NP`.
    NotPresent,
    /// Privilege: a non-conforming code segment needs `DPL = 0` and `RPL = 0`, a conforming one
    /// `DPL = 0` (CPL = 0).
    Privilege { rpl: u8, dpl: u8 },
    /// IA-32e mode and `L = 1` with `D = 1`: `#GP(selector)`.
    LongCodeWith32BitDefault,
    /// `L = 1` while `EFER.LMA = 0`: not modelled (the SDM treats the bit as reserved there).
    LongCodeOutsideIa32e,
    /// The accessed bit is clear and the descriptor's access byte at `linear` is not writable.
    DescriptorUnwritable { linear: u32 },
}

/// Why a segment-register load was refused. `#GP`/`#NP`/`#SS` are not modelled: each reason names
/// the check that would raise one, or a model limit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SegmentLoadReason {
    /// Null selector (index 0, TI = 0): `#GP(0)` for `SS`; a legal load of a data register that the
    /// model does not represent (unusable cache).
    NullSelector,
    /// TI = 1: the LDT is not modelled.
    LdtSelector,
    /// `(selector & !7) + 7` is greater than the GDT limit.
    BeyondGdtLimit { limit: u16 },
    /// A descriptor byte at `linear` is not readable (or the descriptor address wraps past 4 GiB).
    DescriptorUnreadable { linear: u32 },
    /// `S = 0`: a system descriptor is not a segment of this kind.
    SystemDescriptor { access: u8 },
    /// `SS` target that is a code segment or read-only data.
    NotWritableData { access: u8 },
    /// `DS`/`ES`/`FS`/`GS` target that is a code segment (readable code is not modelled).
    CodeSegment { access: u8 },
    /// Expand-down data segment: its limit semantics are not modelled.
    ExpandDown { access: u8 },
    /// Privilege check with CPL = 0: `SS` needs `RPL = DPL = 0`, the others `DPL >= RPL`.
    Privilege { rpl: u8, dpl: u8 },
    /// `P = 0`.
    NotPresent,
    /// The accessed bit is clear and the descriptor byte at `linear` is not writable.
    DescriptorUnwritable { linear: u32 },
}

/// Why [`ProtectedCpu::step`] did not execute. Except for the accessed/dirty bits of the walks that
/// completed (see [`ProtectedCpu::step`]), the state and memory are unchanged on a refusal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProtectedRefusal {
    /// The instruction is outside the admitted forms; `bytes` are up to
    /// [`UNSUPPORTED_BYTES_MAX`] readable bytes at `eip`.
    Unsupported {
        eip: u32,
        linear: u32,
        bytes: Vec<u8>,
    },
    /// CS is not a 32-bit code segment (`D/B = 0`): the decode defaults of this module are 32-bit.
    CsNot32Bit { flags: u8 },
    /// An instruction byte lies beyond the CS limit.
    FetchBeyondLimit { eip: u32, offset: u64 },
    /// An instruction byte at `linear` is not readable in memory.
    FetchUnreadable { eip: u32, linear: u32 },
    /// The target of a taken `JAE`/`JNE`/`JE` is beyond the CS limit.
    TargetBeyondLimit { eip: u32, offset: u32, limit: u32 },
    /// The 4-byte DS operand at `DS:disp` does not fit under the DS limit.
    DataBeyondLimit { eip: u32, disp: u32, limit: u32 },
    /// A data byte at `linear` is not readable in memory.
    DataUnreadable { eip: u32, linear: u32 },
    /// A segment load was refused for `reason`.
    SegmentLoadRefused {
        eip: u32,
        reg: ProtectedSegReg,
        selector: u16,
        reason: SegmentLoadReason,
    },
    /// The shared-memory profile could not commit the checked descriptor image.
    DescriptorCommitRefused { eip: u32, linear: u32 },
    /// `WRMSR` of EFER with a bit outside [`EFER_WRITABLE_MASK`] in `EDX:EAX` (`value`): the
    /// hardware raises `#GP(0)`.
    EferWriteReservedBits { eip: u32, value: u64 },
    /// `MOV CR4, EAX` with a bit above bit 10 set, or one that clears `PAE` while paging is on
    /// without IA-32e mode (that would switch PAE paging to 32-bit paging, which is not modelled);
    /// `value` is EAX.
    Cr4WriteUnmodelled { eip: u32, value: u32 },
    /// `MOV CR4, EAX` that clears `PAE` while IA-32e mode is active (`LMA = 1`): `#GP(0)` (S2-25;
    /// SDM Vol. 3A §12.8.5, "IA-32e mode is active and an attempt is made to disable
    /// physical-address extensions"). `value` is EAX.
    Cr4PaeClearInLongMode { eip: u32, value: u32 },
    /// A `MOV` to `CR0`, `CR3` or `CR4` that loads the PDPTEs of PAE paging found a PDPTE with
    /// `P = 1` and a reserved bit: `#GP(0)`, the PDPTEs and the control register keep their value
    /// (S2-25; SDM Vol. 3A §5.4.1). `index` is the PDPTE number.
    PdpteReservedBits { eip: u32, index: u8, pdpte: u64 },
    /// The PDPTE at `phys` could not be read from memory (a model limit, S2-25).
    PdpteUnreadable { eip: u32, phys: u32 },
    /// `MOV CR0, EAX` with `PG = 1`, `EFER.LME = 0` and `CR4.PAE = 0`: 32-bit paging is not modelled
    /// (S2-25 models PAE paging; `value` is EAX).
    PagingNotModelled { eip: u32, value: u32 },
    /// `MOV CR0, EAX` with `PG = 0`, a second `PG` write, reserved bits or `NW` without `CD`:
    /// not modelled (the reserved-bit and `NW` cases are `#GP(0)` in hardware).
    Cr0WriteUnmodelled { eip: u32, value: u32 },
    /// `MOV CR0, EAX` with `PG = 1` and `PE = 0`: `#GP(0)`.
    PagingWithoutProtection { eip: u32, value: u32 },
    /// `MOV CR0, EAX` with `PG = 1`, `EFER.LME = 1` and `CR4.PAE = 0`: `#GP(0)`.
    LongModePagingWithoutPae { eip: u32, value: u32 },
    /// `MOV CR0, EAX` with `PG = 1`, `EFER.LME = 1` while `CS.L = 1`: `#GP(0)`.
    LongModeActivationFromLongCs { eip: u32, value: u32 },
    /// `WRMSR` of EFER that changes `LME` while paging is on: `#GP(0)`.
    EferLmeChangeWithPaging { eip: u32, value: u64 },
    /// A memory access of the instruction would raise `#PF`, or is outside the page model (the
    /// reason says which). `linear` is the address `CR2` would get; `access` the kind.
    PageFault {
        eip: u32,
        linear: u32,
        access: AccessKind,
        reason: PageRefusal,
    },
    /// The far jump `JMP ptr16:32` was refused for `reason`.
    FarJumpRefused {
        eip: u32,
        selector: u16,
        reason: FarJumpReason,
    },
    /// 64-bit mode with a `RIP` above 4 GiB that the jump of block 8 did not set: the instruction
    /// fetch is outside the page model (see [`ProtectedCpu::step`]).
    Mode64NotModelled { rip: u64 },
    /// The indirect jump of block 8 has set `RIP` to its target (`rip`); the model does not fetch
    /// or execute the target (S6-14n).
    Mode64TargetEntered { rip: u64 },
    /// A 64-bit address of the instruction at `rip` is not canonical (`#GP(0)`).
    NonCanonicalAddress {
        rip: u64,
        address: u64,
        role: AddressRole,
    },
    /// A linear address of the 64-bit instruction at `rip` is `linear` >= 4 GiB: the page model
    /// walks 32-bit linear addresses only.
    LinearBeyond4GiB { rip: u64, linear: u64 },
}

/// What a non-canonical 64-bit address of [`ProtectedRefusal::NonCanonicalAddress`] was used for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AddressRole {
    /// The memory operand of the indirect jump (its first byte or, for the last byte, the address
    /// plus 7).
    Operand,
    /// The jump target read from the cell.
    JumpTarget,
}

/// Why [`ProtectedCpu::from_real_entry`] did not build a state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtectedEntryRefusal {
    /// `CR0.PE` is clear or the block 5 far jump has not loaded a protected CS.
    NotEntered { cr0: u32, cs_loaded: bool },
}

/// Result of [`ProtectedCpu::run`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProtectedRun {
    /// Number of instructions executed.
    pub steps: u64,
    /// `None` when `max_steps` was reached, otherwise the refusal that stopped the run.
    pub stop: Option<ProtectedRefusal>,
}

/// Processor state for blocks 6 and 7: 32-bit protected mode, then (after `MOV CR0` with `PG`) IA-32e
/// compatibility mode with paging.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProtectedCpu {
    /// EAX, ECX, EDX, EBX, ESP, EBP, ESI, EDI (index constants of `real_mode_exec`).
    pub gpr: [u32; 8],
    /// Low 32 bits of `RIP` (the whole instruction pointer outside 64-bit mode).
    pub eip: u32,
    /// Bits 63:32 of `RIP`: 0 except after the indirect jump of block 8 (S6-14n).
    pub rip_high: u32,
    /// Set by the indirect jump of block 8: `RIP` is the target, which the model does not execute.
    pub target_entered: bool,
    pub eflags: u32,
    pub cr0: u32,
    /// Stored by `MOV CR3, EAX`; the PML4 base of the walk once `CR0.PG` is set.
    pub cr3: u32,
    /// Stored by `MOV CR4, EAX` (bits 0..10 only).
    pub cr4: u32,
    /// `IA32_EFER`, the only MSR: 0 at entry; `WRMSR` stores `SCE`, `LME`, `NXE`. `LMA` is never
    /// changed by `WRMSR`; it becomes 1 at `MOV CR0` with `PG = 1` while `LME` and `PAE` are set.
    pub efer: u64,
    /// The PDPTE registers of PAE paging (S2-25): loaded at `MOV CR0` (enabling paging), `MOV CR3`
    /// and `MOV CR4` (changing PAE, PGE or PSE) while `CR0.PG = 1`, `CR4.PAE = 1` and `LMA = 0`;
    /// 0 and unused otherwise.
    pub pdptes: [u64; 4],
    pub cs: ProtectedSegment,
    pub ds: ProtectedSegment,
    pub es: ProtectedSegment,
    pub ss: ProtectedSegment,
    pub fs: ProtectedSegment,
    pub gs: ProtectedSegment,
    pub gdtr: TableRegister,
    pub idtr: TableRegister,
    /// Set by `MOV SS`: interrupts are inhibited until after the next instruction (SDM Vol. 3A,
    /// "MOV to SS"). Recorded only; not compared with the oracle (as in `RealModeCpu`).
    pub mov_ss_shadow: bool,
}

fn real_data_segment(seg: crate::legacy_mode::RealSegment) -> ProtectedSegment {
    ProtectedSegment {
        selector: seg.selector,
        base: seg.base,
        limit: seg.limit,
        access: REAL_MODE_DATA_ACCESS,
        flags: 0,
    }
}

/// Flags of `lhs + rhs` (CF = carry, OF = signed overflow, AF = carry out of bit 3, SF/ZF/PF from
/// the result; SDM Vol. 1 appendix A, from memory). Only the six arithmetic bits are returned.
fn add_flags(lhs: u32, rhs: u32) -> u32 {
    let result = lhs.wrapping_add(rhs);
    let mut flags = logic_flags(result);
    if result < lhs {
        flags |= EFLAGS_CF;
    }
    if (lhs ^ rhs ^ result) & 0x10 != 0 {
        flags |= EFLAGS_AF;
    }
    if ((lhs ^ result) & (rhs ^ result)) >> 31 != 0 {
        flags |= EFLAGS_OF;
    }
    flags
}

impl ProtectedCpu {
    /// State after the block 5 far jump: `real` must have `CR0.PE = 1` and a protected CS loaded
    /// ([`RealModeCpu::cs_protected`]).
    pub fn from_real_entry(real: &RealModeCpu) -> Result<Self, ProtectedEntryRefusal> {
        let Some(cs) = real.cs_protected else {
            return Err(ProtectedEntryRefusal::NotEntered {
                cr0: real.mode.cr0,
                cs_loaded: false,
            });
        };
        if real.mode.cr0 & CR0_PE == 0 {
            return Err(ProtectedEntryRefusal::NotEntered {
                cr0: real.mode.cr0,
                cs_loaded: true,
            });
        }
        Ok(Self {
            gpr: real.gpr,
            eip: real.mode.eip,
            rip_high: 0,
            target_entered: false,
            eflags: real.mode.eflags,
            cr0: real.mode.cr0,
            cr3: 0,
            cr4: 0,
            efer: 0,
            pdptes: [0; 4],
            cs,
            ds: real_data_segment(real.mode.ds),
            es: real_data_segment(real.mode.es),
            ss: real_data_segment(real.mode.ss),
            fs: real_data_segment(real.mode.fs),
            gs: real_data_segment(real.mode.gs),
            gdtr: real.gdtr,
            idtr: real.idtr,
            mov_ss_shadow: real.mov_ss_shadow,
        })
    }

    /// The 64-bit `RIP`.
    pub fn rip(&self) -> u64 {
        (u64::from(self.rip_high) << 32) | u64::from(self.eip)
    }

    /// 64-bit mode: `EFER.LMA = 1` and `CS.L = 1` (segmentation is off, `CS.D` must be 0).
    fn is_64bit(&self) -> bool {
        self.cs.flags & 0x2 != 0 && self.efer & EFER_LMA != 0
    }

    /// Base added to `EIP` for the linear address of an instruction byte: the CS base, 0 in 64-bit
    /// mode.
    fn code_base(&self) -> u32 {
        if self.is_64bit() { 0 } else { self.cs.base }
    }

    fn fetch<M: RealModeMemory>(&self, mem: &M, i: u32) -> Result<u8, ProtectedRefusal> {
        let offset = u64::from(self.eip) + u64::from(i);
        if !self.is_64bit() && offset > u64::from(self.cs.limit) {
            return Err(ProtectedRefusal::FetchBeyondLimit {
                eip: self.eip,
                offset,
            });
        }
        let linear = if self.is_64bit() {
            u32::try_from(offset).map_err(|_| ProtectedRefusal::LinearBeyond4GiB {
                rip: self.rip(),
                linear: offset,
            })?
        } else {
            self.cs.base.wrapping_add(offset as u32)
        };
        let phys = self.physical(mem, linear, AccessKind::Fetch, &mut Vec::new())?;
        mem.read_u8(phys).ok_or(ProtectedRefusal::FetchUnreadable {
            eip: self.eip,
            linear,
        })
    }

    /// PAE paging is in use: `CR0.PG = 1`, `CR4.PAE = 1`, `EFER.LMA = 0` (SDM Vol. 3A §5.4).
    pub fn pae_paging(&self) -> bool {
        self.cr0 & CR0_PG != 0 && self.cr4 & CR4_PAE != 0 && self.efer & EFER_LMA == 0
    }

    /// The PDPTEs of the table `cr3` names (`MOV` to `CR0`/`CR3`/`CR4` of PAE paging); a refusal
    /// changes nothing.
    fn load_pdptes<M: RealModeMemory>(
        &self,
        mem: &M,
        cr3: u32,
    ) -> Result<[u64; 4], ProtectedRefusal> {
        paging::load_pdptes(mem, cr3).map_err(|refusal| match refusal {
            PdpteRefusal::Reserved { index, pdpte } => ProtectedRefusal::PdpteReservedBits {
                eip: self.eip,
                index,
                pdpte,
            },
            PdpteRefusal::Unreadable { phys } => ProtectedRefusal::PdpteUnreadable {
                eip: self.eip,
                phys,
            },
        })
    }

    /// Paging control state of the 4-level walk, `None` while `CR0.PG` is clear. PAE paging uses
    /// [`Self::pae_paging`] instead.
    fn paging_params(&self) -> Option<PagingParams> {
        (self.cr0 & CR0_PG != 0).then_some(PagingParams {
            cr3: self.cr3,
            wp: self.cr0 & CR0_WP != 0,
            nxe: self.efer & EFER_NXE != 0,
        })
    }

    /// Physical address of the byte at `linear`: the address itself while paging is off, otherwise
    /// the result of the walk, whose accessed/dirty updates are appended to `touches` (written by
    /// [`Self::step`] after the instruction succeeded). A refused walk is
    /// [`ProtectedRefusal::PageFault`]. Nothing is changed.
    fn physical<M: RealModeMemory>(
        &self,
        mem: &M,
        linear: u32,
        access: AccessKind,
        touches: &mut Vec<Touch>,
    ) -> Result<u32, ProtectedRefusal> {
        let walk = if self.pae_paging() {
            let params = PaeParams {
                pdptes: self.pdptes,
                wp: self.cr0 & CR0_WP != 0,
                nxe: self.efer & EFER_NXE != 0,
            };
            paging::translate_pae(mem, params, linear, access)
        } else if let Some(params) = self.paging_params() {
            paging::translate(mem, params, linear, access)
        } else {
            return Ok(linear);
        };
        match walk {
            Ok(t) => {
                touches.extend(t.touches);
                Ok(t.phys)
            }
            Err(reason) => Err(ProtectedRefusal::PageFault {
                eip: self.eip,
                linear,
                access,
                reason,
            }),
        }
    }

    fn fetch_u32<M: RealModeMemory>(&self, mem: &M, i: u32) -> Result<u32, ProtectedRefusal> {
        Ok(u32::from_le_bytes([
            self.fetch(mem, i)?,
            self.fetch(mem, i + 1)?,
            self.fetch(mem, i + 2)?,
            self.fetch(mem, i + 3)?,
        ]))
    }

    fn unsupported<M: RealModeMemory>(&self, mem: &M) -> ProtectedRefusal {
        let mut bytes = Vec::new();
        for i in 0..UNSUPPORTED_BYTES_MAX as u32 {
            match self.fetch(mem, i) {
                Ok(b) => bytes.push(b),
                Err(_) => break,
            }
        }
        ProtectedRefusal::Unsupported {
            eip: self.eip,
            linear: self.code_base().wrapping_add(self.eip),
            bytes,
        }
    }

    /// Decodes the closed 64-bit set (block 8, S6-14n) at `RIP`.
    fn decode64<M: RealModeMemory>(
        &self,
        mem: &M,
    ) -> Result<(ProtectedInstruction, u32), ProtectedRefusal> {
        if self.target_entered {
            return Err(ProtectedRefusal::Mode64TargetEntered { rip: self.rip() });
        }
        if self.rip_high != 0 {
            return Err(ProtectedRefusal::Mode64NotModelled { rip: self.rip() });
        }
        if self.cs.flags & 0x4 != 0 {
            // The far jump refuses L = 1 with D = 1, so this state is not reachable by the model.
            return Err(self.unsupported(mem));
        }
        // `ff /4` with `mod = 00`, `rm = 101`: the RIP-relative memory operand.
        if self.fetch(mem, 0)? != 0xFF || self.fetch(mem, 1)? != 0x25 {
            return Err(self.unsupported(mem));
        }
        Ok((
            ProtectedInstruction::JmpIndirectRipRel {
                disp: self.fetch_u32(mem, 2)? as i32,
            },
            6,
        ))
    }

    /// `Err` when `address` is not canonical (bits 63:47 not all equal).
    fn check_canonical(&self, address: u64, role: AddressRole) -> Result<(), ProtectedRefusal> {
        if ((address as i64) << 16) >> 16 == address as i64 {
            Ok(())
        } else {
            Err(ProtectedRefusal::NonCanonicalAddress {
                rip: self.rip(),
                address,
                role,
            })
        }
    }

    /// Reads the QWORD at the 64-bit linear address `cell` of the instruction at `RIP` (the first
    /// and the last byte must be canonical, every byte below 4 GiB, readable through the walk).
    fn read_linear_u64<M: RealModeMemory>(
        &self,
        mem: &M,
        cell: u64,
        touches: &mut Vec<Touch>,
    ) -> Result<u64, ProtectedRefusal> {
        self.check_canonical(cell, AddressRole::Operand)?;
        self.check_canonical(cell.wrapping_add(7), AddressRole::Operand)?;
        let mut bytes = [0u8; 8];
        for (i, slot) in bytes.iter_mut().enumerate() {
            let at = cell.wrapping_add(i as u64);
            let Ok(linear) = u32::try_from(at) else {
                return Err(ProtectedRefusal::LinearBeyond4GiB {
                    rip: self.rip(),
                    linear: at,
                });
            };
            let phys = self.physical(mem, linear, AccessKind::Read, touches)?;
            *slot = mem.read_u8(phys).ok_or(ProtectedRefusal::DataUnreadable {
                eip: self.eip,
                linear,
            })?;
        }
        Ok(u64::from_le_bytes(bytes))
    }

    /// Decodes the instruction at `CS:EIP`; returns it with its length.
    pub fn decode<M: RealModeMemory>(
        &self,
        mem: &M,
    ) -> Result<(ProtectedInstruction, u32), ProtectedRefusal> {
        if self.is_64bit() {
            return self.decode64(mem);
        }
        if self.cs.flags & 0x4 == 0 {
            return Err(ProtectedRefusal::CsNot32Bit {
                flags: self.cs.flags,
            });
        }
        match self.fetch(mem, 0)? {
            0x8E => {
                let modrm = self.fetch(mem, 1)?;
                let reg = match (modrm >> 3) & 7 {
                    0 => ProtectedSegReg::Es,
                    2 => ProtectedSegReg::Ss,
                    3 => ProtectedSegReg::Ds,
                    4 => ProtectedSegReg::Fs,
                    5 => ProtectedSegReg::Gs,
                    _ => return Err(self.unsupported(mem)),
                };
                if modrm >> 6 != 3 || usize::from(modrm & 7) != EDX {
                    return Err(self.unsupported(mem));
                }
                Ok((ProtectedInstruction::MovSegFromEdx(reg), 2))
            }
            0x81 => {
                if self.fetch(mem, 1)? != 0xC4 {
                    return Err(self.unsupported(mem));
                }
                Ok((
                    ProtectedInstruction::AddEspImm32(self.fetch_u32(mem, 2)?),
                    6,
                ))
            }
            0x0F => match self.fetch(mem, 1)? {
                0xBA => {
                    if self.fetch(mem, 2)? != 0x25 {
                        return Err(self.unsupported(mem));
                    }
                    let disp = self.fetch_u32(mem, 3)?;
                    let bit = self.fetch(mem, 7)?;
                    Ok((ProtectedInstruction::BtDsImm8 { disp, bit }, 8))
                }
                0x32 => Ok((ProtectedInstruction::Rdmsr, 2)),
                0x09 => Ok((ProtectedInstruction::Wbinvd, 2)),
                0x30 => Ok((ProtectedInstruction::Wrmsr, 2)),
                0x22 => match self.fetch(mem, 2)? {
                    0xE0 => Ok((ProtectedInstruction::MovCr4Eax, 3)),
                    0xD8 => Ok((ProtectedInstruction::MovCr3Eax, 3)),
                    0xC0 => Ok((ProtectedInstruction::MovCr0Eax, 3)),
                    _ => Err(self.unsupported(mem)),
                },
                _ => Err(self.unsupported(mem)),
            },
            0xEA => {
                let offset = self.fetch_u32(mem, 1)?;
                let selector = u16::from_le_bytes([self.fetch(mem, 5)?, self.fetch(mem, 6)?]);
                Ok((ProtectedInstruction::JmpFarPtr { selector, offset }, 7))
            }
            0x73 => Ok((ProtectedInstruction::JaeRel8(self.fetch(mem, 1)? as i8), 2)),
            0x75 => Ok((ProtectedInstruction::JneRel8(self.fetch(mem, 1)? as i8), 2)),
            0x74 => Ok((ProtectedInstruction::JeRel8(self.fetch(mem, 1)? as i8), 2)),
            0x3B => {
                let insn = match self.fetch(mem, 1)? {
                    0x05 => ProtectedInstruction::CmpEaxWithDs(self.fetch_u32(mem, 2)?),
                    0x15 => ProtectedInstruction::CmpEdxWithDs(self.fetch_u32(mem, 2)?),
                    _ => return Err(self.unsupported(mem)),
                };
                Ok((insn, 6))
            }
            0x8B => {
                if self.fetch(mem, 1)? != 0x15 {
                    return Err(self.unsupported(mem));
                }
                Ok((
                    ProtectedInstruction::MovEdxFromDs(self.fetch_u32(mem, 2)?),
                    6,
                ))
            }
            0xA1 => Ok((
                ProtectedInstruction::MovEaxFromDs(self.fetch_u32(mem, 1)?),
                5,
            )),
            0xB8 => Ok((
                ProtectedInstruction::MovEaxImm32(self.fetch_u32(mem, 1)?),
                5,
            )),
            0xB9 => Ok((
                ProtectedInstruction::MovEcxImm32(self.fetch_u32(mem, 1)?),
                5,
            )),
            _ => Err(self.unsupported(mem)),
        }
    }

    /// Reads the DWORD at `DS:disp` (the whole operand must lie under the DS limit and be
    /// readable). Nothing is changed.
    fn read_ds_u32<M: RealModeMemory>(
        &self,
        mem: &M,
        disp: u32,
        touches: &mut Vec<Touch>,
    ) -> Result<u32, ProtectedRefusal> {
        if u64::from(disp) + 3 > u64::from(self.ds.limit) {
            return Err(ProtectedRefusal::DataBeyondLimit {
                eip: self.eip,
                disp,
                limit: self.ds.limit,
            });
        }
        let mut bytes = [0u8; 4];
        for (i, slot) in bytes.iter_mut().enumerate() {
            let linear = self.ds.base.wrapping_add(disp).wrapping_add(i as u32);
            let phys = self.physical(mem, linear, AccessKind::Read, touches)?;
            *slot = mem.read_u8(phys).ok_or(ProtectedRefusal::DataUnreadable {
                eip: self.eip,
                linear,
            })?;
        }
        Ok(u32::from_le_bytes(bytes))
    }

    /// Target of a taken `Jcc rel8` whose next instruction is at `next_eip`: the 32-bit EIP wraps
    /// at 4 GiB and the target must not exceed the CS limit.
    fn jump_target(&self, next_eip: u32, rel: i8) -> Result<u32, ProtectedRefusal> {
        let target = next_eip.wrapping_add(i32::from(rel) as u32);
        if target > self.cs.limit {
            return Err(ProtectedRefusal::TargetBeyondLimit {
                eip: self.eip,
                offset: target,
                limit: self.cs.limit,
            });
        }
        Ok(target)
    }

    /// Physical address of the access byte at `access_at` (byte 5 of the descriptor that starts at
    /// `descriptor_at`) for the accessed-bit write. A refused walk reports `descriptor_at` as the
    /// faulting address: the KVM oracle (image `r`, S6-14m) loads `CR2` with the start of the
    /// descriptor, not with the byte that is written.
    fn descriptor_write_target<M: RealModeMemory>(
        &self,
        mem: &M,
        descriptor_at: u32,
        access_at: u32,
        touches: &mut Vec<Touch>,
    ) -> Result<u32, ProtectedRefusal> {
        self.physical(mem, access_at, AccessKind::Write, touches)
            .map_err(|refusal| match refusal {
                ProtectedRefusal::PageFault {
                    eip,
                    access,
                    reason,
                    ..
                } => ProtectedRefusal::PageFault {
                    eip,
                    linear: descriptor_at,
                    access,
                    reason,
                },
                other => other,
            })
    }

    /// Checks a `MOV Sreg` load of `selector` and returns the new cache and, if the descriptor's
    /// accessed bit is clear, the *physical* address of the access byte to write. The descriptor
    /// is read (and written) through the paging walk when paging is on. Nothing is changed.
    fn check_segment_load<M: RealModeMemory>(
        &self,
        mem: &M,
        reg: ProtectedSegReg,
        selector: u16,
        touches: &mut Vec<Touch>,
    ) -> Result<(ProtectedSegment, Option<u32>), ProtectedRefusal> {
        let refuse = |reason| ProtectedRefusal::SegmentLoadRefused {
            eip: self.eip,
            reg,
            selector,
            reason,
        };
        if selector & 0x4 != 0 {
            return Err(refuse(SegmentLoadReason::LdtSelector));
        }
        if selector & 0xFFF8 == 0 {
            return Err(refuse(SegmentLoadReason::NullSelector));
        }
        let index = u32::from(selector & 0xFFF8);
        if index + 7 > u32::from(self.gdtr.limit) {
            return Err(refuse(SegmentLoadReason::BeyondGdtLimit {
                limit: self.gdtr.limit,
            }));
        }
        let mut descriptor = [0u8; 8];
        let mut physical = [0; 8];
        for (i, (byte, address)) in descriptor.iter_mut().zip(&mut physical).enumerate() {
            let at = u64::from(self.gdtr.base) + u64::from(index) + i as u64;
            let Ok(linear) = u32::try_from(at) else {
                return Err(refuse(SegmentLoadReason::DescriptorUnreadable {
                    linear: at as u32,
                }));
            };
            let phys = self.physical(mem, linear, AccessKind::Read, touches)?;
            *address = phys;
            match mem.read_u8(phys) {
                Some(v) => *byte = v,
                None => {
                    return Err(refuse(SegmentLoadReason::DescriptorUnreadable { linear }));
                }
            }
        }
        let access = descriptor[5];
        if access & 0x10 == 0 {
            return Err(refuse(SegmentLoadReason::SystemDescriptor { access }));
        }
        let (rpl, dpl) = ((selector & 3) as u8, (access >> 5) & 3);
        if reg == ProtectedSegReg::Ss {
            // Code (bit 3) or read-only data (W, bit 1, clear).
            if access & 0x08 != 0 || access & 0x02 == 0 {
                return Err(refuse(SegmentLoadReason::NotWritableData { access }));
            }
            if rpl != 0 || dpl != 0 {
                return Err(refuse(SegmentLoadReason::Privilege { rpl, dpl }));
            }
        } else {
            if access & 0x08 != 0 {
                return Err(refuse(SegmentLoadReason::CodeSegment { access }));
            }
            if dpl < rpl {
                return Err(refuse(SegmentLoadReason::Privilege { rpl, dpl }));
            }
        }
        if access & 0x04 != 0 {
            return Err(refuse(SegmentLoadReason::ExpandDown { access }));
        }
        if access & 0x80 == 0 {
            return Err(refuse(SegmentLoadReason::NotPresent));
        }
        // The descriptor's own address: it was readable above, so it does not wrap.
        let descriptor_at = self.gdtr.base + index;
        let access_at = descriptor_at + 5;
        let write_at = if access & 1 == 0 {
            let phys = self.descriptor_write_target(mem, descriptor_at, access_at, touches)?;
            if !mem.can_write(phys) {
                return Err(refuse(SegmentLoadReason::DescriptorUnwritable {
                    linear: access_at,
                }));
            }
            Some(phys)
        } else {
            None
        };
        if !mem.validate_protected_descriptor(&physical, &descriptor, write_at) {
            return Err(ProtectedRefusal::DescriptorCommitRefused {
                eip: self.eip,
                linear: descriptor_at,
            });
        }
        let segment = ProtectedSegment::from_descriptor(selector, descriptor);
        Ok((
            ProtectedSegment {
                access: access | 1,
                ..segment
            },
            write_at,
        ))
    }

    /// Checks the far jump `JMP ptr16:32` to `selector:offset` (CPL = 0, see the module
    /// documentation) and returns the new `CS` cache with, if the accessed bit of the descriptor
    /// is clear, the physical address of its access byte to write. Nothing is changed.
    fn check_far_jump<M: RealModeMemory>(
        &self,
        mem: &M,
        selector: u16,
        offset: u32,
        touches: &mut Vec<Touch>,
    ) -> Result<(ProtectedSegment, Option<u32>), ProtectedRefusal> {
        let refuse = |reason| ProtectedRefusal::FarJumpRefused {
            eip: self.eip,
            selector,
            reason,
        };
        if selector & 0x4 != 0 {
            return Err(refuse(FarJumpReason::LdtSelector));
        }
        if selector & 0xFFF8 == 0 {
            return Err(refuse(FarJumpReason::NullSelector));
        }
        let index = u32::from(selector & 0xFFF8);
        if index + 7 > u32::from(self.gdtr.limit) {
            return Err(refuse(FarJumpReason::BeyondGdtLimit {
                limit: self.gdtr.limit,
            }));
        }
        let mut descriptor = [0u8; 8];
        let mut physical = [0; 8];
        for (i, (byte, address)) in descriptor.iter_mut().zip(&mut physical).enumerate() {
            let at = u64::from(self.gdtr.base) + u64::from(index) + i as u64;
            let Ok(linear) = u32::try_from(at) else {
                return Err(refuse(FarJumpReason::DescriptorUnreadable {
                    linear: at as u32,
                }));
            };
            let phys = self.physical(mem, linear, AccessKind::Read, touches)?;
            *address = phys;
            match mem.read_u8(phys) {
                Some(v) => *byte = v,
                None => return Err(refuse(FarJumpReason::DescriptorUnreadable { linear })),
            }
        }
        let access = descriptor[5];
        if access & 0x18 != 0x18 {
            return Err(refuse(FarJumpReason::NotCodeSegment { access }));
        }
        if access & 0x80 == 0 {
            return Err(refuse(FarJumpReason::NotPresent));
        }
        let (rpl, dpl) = ((selector & 3) as u8, (access >> 5) & 3);
        let conforming = access & 0x04 != 0;
        if dpl != 0 || (!conforming && rpl != 0) {
            return Err(refuse(FarJumpReason::Privilege { rpl, dpl }));
        }
        // CS.RPL becomes the CPL (0).
        let segment = ProtectedSegment::from_descriptor(selector & !3, descriptor);
        let long = segment.flags & 0x2 != 0;
        if long {
            if self.efer & EFER_LMA == 0 {
                return Err(refuse(FarJumpReason::LongCodeOutsideIa32e));
            }
            if segment.flags & 0x4 != 0 {
                return Err(refuse(FarJumpReason::LongCodeWith32BitDefault));
            }
        } else if offset > segment.limit {
            return Err(ProtectedRefusal::TargetBeyondLimit {
                eip: self.eip,
                offset,
                limit: segment.limit,
            });
        }
        let descriptor_at = self.gdtr.base + index;
        let access_at = descriptor_at + 5;
        let write_at = if access & 1 == 0 {
            let phys = self.descriptor_write_target(mem, descriptor_at, access_at, touches)?;
            if !mem.can_write(phys) {
                return Err(refuse(FarJumpReason::DescriptorUnwritable {
                    linear: access_at,
                }));
            }
            Some(phys)
        } else {
            None
        };
        if !mem.validate_protected_descriptor(&physical, &descriptor, write_at) {
            return Err(ProtectedRefusal::DescriptorCommitRefused {
                eip: self.eip,
                linear: descriptor_at,
            });
        }
        Ok((
            ProtectedSegment {
                access: access | 1,
                ..segment
            },
            write_at,
        ))
    }

    /// Executes one instruction. On a refusal the processor state is unchanged and so is memory,
    /// except for the accessed/dirty bits of the paging structures that walks which had completed
    /// before the refusal set (as hardware does for the instruction fetch and for the descriptor
    /// read of a far jump that then faults: the oracle images `e`..`i`, `l` and `r` show them).
    /// A walk that ends in a `#PF` sets `A` in none of its entries, in the model as in the oracle
    /// (images `k`, `m`..`q`, `s`; decision 4 of S6-14m). A refusal in [`decode`] (an unsupported
    /// form, a page fault of a byte of the instruction) sets none: the fetch walks of the stopping
    /// instruction are not modelled. A refusal after decode (for example `RDMSR`/`WRMSR` with
    /// `ECX` other than `EFER`, or `DescriptorUnwritable`) keeps the `A`/`D` bits of the walks
    /// completed before it, the fetch included: `DescriptorUnwritable` still commits `D` on the GDT
    /// page although the descriptor byte is not written.
    pub fn step<M: RealModeMemory>(
        &mut self,
        mem: &mut M,
    ) -> Result<ProtectedInstruction, ProtectedRefusal> {
        let mut touches: Vec<Touch> = Vec::new();
        let result = self.step_collecting(mem, &mut touches);
        // S6-31b: a touch whose entry changed its translation bits meanwhile is discarded (the
        // instruction is not repeated: its effects are already committed); the count is not used.
        let _discarded = paging::commit_touches(mem, &touches);
        result
    }

    fn step_collecting<M: RealModeMemory>(
        &mut self,
        mem: &mut M,
        touches: &mut Vec<Touch>,
    ) -> Result<ProtectedInstruction, ProtectedRefusal> {
        let (insn, len) = self.decode(mem)?;
        // Accessed/dirty updates of the paging structures walked by this instruction (the fetch
        // walks come first, as in hardware), written by `step` at the end.
        if self.cr0 & CR0_PG != 0 {
            for i in 0..len {
                let linear = self.code_base().wrapping_add(self.eip).wrapping_add(i);
                self.physical(mem, linear, AccessKind::Fetch, touches)?;
            }
        }
        let next_eip = self.eip.wrapping_add(len);
        let shadow_before = self.mov_ss_shadow;
        let mut eip = next_eip;
        match insn {
            ProtectedInstruction::MovSegFromEdx(reg) => {
                let selector = self.gpr[EDX] as u16;
                let (segment, write_at) = self.check_segment_load(mem, reg, selector, touches)?;
                if let Some(at) = write_at {
                    let access = segment.access;
                    assert!(mem.write_u8(at, access), "can_write was true");
                }
                match reg {
                    ProtectedSegReg::Es => self.es = segment,
                    ProtectedSegReg::Ss => {
                        self.ss = segment;
                        self.mov_ss_shadow = true;
                    }
                    ProtectedSegReg::Ds => self.ds = segment,
                    ProtectedSegReg::Fs => self.fs = segment,
                    ProtectedSegReg::Gs => self.gs = segment,
                }
            }
            ProtectedInstruction::AddEspImm32(imm) => {
                let lhs = self.gpr[ESP];
                self.gpr[ESP] = lhs.wrapping_add(imm);
                self.eflags = (self.eflags & !EFLAGS_ARITH) | add_flags(lhs, imm);
            }
            ProtectedInstruction::BtDsImm8 { disp, bit } => {
                let value = self.read_ds_u32(mem, disp, touches)?;
                // Bit offset of an immediate operand is taken modulo the operand size (SDM).
                if (value >> (bit & 31)) & 1 != 0 {
                    self.eflags |= EFLAGS_CF;
                } else {
                    self.eflags &= !EFLAGS_CF;
                }
            }
            ProtectedInstruction::JaeRel8(rel) => {
                if self.eflags & EFLAGS_CF == 0 {
                    eip = self.jump_target(next_eip, rel)?;
                }
            }
            ProtectedInstruction::JneRel8(rel) => {
                if self.eflags & EFLAGS_ZF == 0 {
                    eip = self.jump_target(next_eip, rel)?;
                }
            }
            ProtectedInstruction::JeRel8(rel) => {
                if self.eflags & EFLAGS_ZF != 0 {
                    eip = self.jump_target(next_eip, rel)?;
                }
            }
            ProtectedInstruction::MovEaxFromDs(disp) => {
                self.gpr[EAX] = self.read_ds_u32(mem, disp, touches)?;
            }
            ProtectedInstruction::MovEdxFromDs(disp) => {
                self.gpr[EDX] = self.read_ds_u32(mem, disp, touches)?;
            }
            ProtectedInstruction::CmpEaxWithDs(disp) => {
                let rhs = self.read_ds_u32(mem, disp, touches)?;
                self.eflags = (self.eflags & !EFLAGS_ARITH) | sub_flags(self.gpr[EAX], rhs);
            }
            ProtectedInstruction::CmpEdxWithDs(disp) => {
                let rhs = self.read_ds_u32(mem, disp, touches)?;
                self.eflags = (self.eflags & !EFLAGS_ARITH) | sub_flags(self.gpr[EDX], rhs);
            }
            ProtectedInstruction::Rdmsr => {
                if self.gpr[ECX] != MSR_EFER {
                    return Err(self.unsupported(mem));
                }
                self.gpr[EAX] = self.efer as u32;
                self.gpr[EDX] = (self.efer >> 32) as u32;
            }
            ProtectedInstruction::Wbinvd => {}
            ProtectedInstruction::Wrmsr => {
                if self.gpr[ECX] != MSR_EFER {
                    return Err(self.unsupported(mem));
                }
                let value = (u64::from(self.gpr[EDX]) << 32) | u64::from(self.gpr[EAX]);
                if value & !EFER_WRITABLE_MASK != 0 {
                    return Err(ProtectedRefusal::EferWriteReservedBits {
                        eip: self.eip,
                        value,
                    });
                }
                if self.cr0 & CR0_PG != 0 && (value ^ self.efer) & EFER_LME != 0 {
                    return Err(ProtectedRefusal::EferLmeChangeWithPaging {
                        eip: self.eip,
                        value,
                    });
                }
                // LMA is set by the processor only: the written bit is ignored.
                self.efer = (value & !EFER_LMA) | (self.efer & EFER_LMA);
            }
            ProtectedInstruction::MovCr4Eax => {
                let value = self.gpr[EAX];
                let eip = self.eip;
                if value & !CR4_MODELLED_MASK != 0 {
                    return Err(ProtectedRefusal::Cr4WriteUnmodelled { eip, value });
                }
                let paging = self.cr0 & CR0_PG != 0;
                if paging && value & CR4_PAE == 0 {
                    // IA-32e active: `#GP(0)` (SDM Vol. 3A §12.8.5). Otherwise the write would
                    // switch PAE paging to 32-bit paging, which the model does not have.
                    return Err(if self.efer & EFER_LMA != 0 {
                        ProtectedRefusal::Cr4PaeClearInLongMode { eip, value }
                    } else {
                        ProtectedRefusal::Cr4WriteUnmodelled { eip, value }
                    });
                }
                // PAE paging is in use after this write (paging on, PAE set, no IA-32e): a change
                // of PAE, PGE or PSE reloads the PDPTEs from CR3 (SDM Vol. 3A §5.4.1).
                let mut pdptes = self.pdptes;
                if paging
                    && self.efer & EFER_LMA == 0
                    && (value ^ self.cr4) & (CR4_PAE | CR4_PGE | CR4_PSE) != 0
                {
                    pdptes = self.load_pdptes(mem, self.cr3)?;
                }
                self.cr4 = value;
                self.pdptes = pdptes;
            }
            ProtectedInstruction::MovCr3Eax => {
                let value = self.gpr[EAX];
                // PAE paging: the PDPTEs are loaded from the address being loaded into CR3; a
                // reserved bit is `#GP(0)` and CR3 keeps its value.
                if self.pae_paging() {
                    self.pdptes = self.load_pdptes(mem, value)?;
                }
                self.cr3 = value;
            }
            ProtectedInstruction::MovCr0Eax => {
                let value = self.gpr[EAX];
                let eip = self.eip;
                if self.cr0 & CR0_PG != 0
                    || value & CR0_PG == 0
                    || value & !CR0_VALID_MASK != 0
                    || (value & CR0_NW != 0 && value & CR0_CD == 0)
                {
                    return Err(ProtectedRefusal::Cr0WriteUnmodelled { eip, value });
                }
                if value & CR0_PE == 0 {
                    return Err(ProtectedRefusal::PagingWithoutProtection { eip, value });
                }
                if self.efer & EFER_LME == 0 {
                    // 32-bit paging (PAE clear) is not modelled. With PAE: PAE paging, the PDPTEs
                    // are loaded from CR3 by this write (a reserved bit is `#GP(0)`); IA-32e mode
                    // is not activated, LMA stays 0 (SDM Vol. 3A §5.4.1).
                    if self.cr4 & CR4_PAE == 0 {
                        return Err(ProtectedRefusal::PagingNotModelled { eip, value });
                    }
                    let pdptes = self.load_pdptes(mem, self.cr3)?;
                    self.cr0 = value | CR0_ET;
                    self.pdptes = pdptes;
                } else {
                    if self.cr4 & CR4_PAE == 0 {
                        return Err(ProtectedRefusal::LongModePagingWithoutPae { eip, value });
                    }
                    if self.cs.flags & 0x2 != 0 {
                        return Err(ProtectedRefusal::LongModeActivationFromLongCs { eip, value });
                    }
                    // PG with LME and PAE: the processor activates IA-32e mode (LMA := 1).
                    self.cr0 = value | CR0_ET;
                    self.efer |= EFER_LMA;
                }
            }
            ProtectedInstruction::JmpFarPtr { selector, offset } => {
                let (segment, write_at) = self.check_far_jump(mem, selector, offset, touches)?;
                if let Some(at) = write_at {
                    assert!(mem.write_u8(at, segment.access), "can_write was true");
                }
                self.cs = segment;
                eip = offset;
            }
            ProtectedInstruction::JmpIndirectRipRel { disp } => {
                // The cell is relative to the next instruction (`RIP` is below 4 GiB here).
                let next = u64::from(self.eip).wrapping_add(u64::from(len));
                let cell = next.wrapping_add(i64::from(disp) as u64);
                let target = self.read_linear_u64(mem, cell, touches)?;
                self.check_canonical(target, AddressRole::JumpTarget)?;
                // Nothing after this can refuse: commit the new `RIP`.
                eip = target as u32;
                self.rip_high = (target >> 32) as u32;
                self.target_entered = true;
            }
            ProtectedInstruction::MovEaxImm32(imm) => self.gpr[EAX] = imm,
            ProtectedInstruction::MovEcxImm32(imm) => self.gpr[ECX] = imm,
        }
        self.eip = eip;
        // The shadow armed by an earlier `MOV SS` ends after this instruction; one armed by this
        // instruction survives it (as in `RealModeCpu::step`).
        if shadow_before {
            self.mov_ss_shadow = matches!(
                insn,
                ProtectedInstruction::MovSegFromEdx(ProtectedSegReg::Ss)
            );
        }
        Ok(insn)
    }

    /// Executes up to `max_steps` instructions, stopping at the first refusal.
    pub fn run<M: RealModeMemory>(&mut self, mem: &mut M, max_steps: u64) -> ProtectedRun {
        let mut steps = 0;
        while steps < max_steps {
            if let Err(refusal) = self.step(mem) {
                return ProtectedRun {
                    steps,
                    stop: Some(refusal),
                };
            }
            steps += 1;
        }
        ProtectedRun { steps, stop: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::legacy_mode::RealSegment;
    use crate::real_mode_exec::{
        EBP, EBX, EDI, ESI, ImageMemory, ImageMemoryMut, RealModeMemory, RealModeRefusal,
    };

    const RAM: usize = 0x6100;
    const FLAT_CODE: [u8; 8] = [0xFF, 0xFF, 0, 0, 0, 0x9B, 0xCF, 0];
    const FLAT_DATA_A_CLEAR: [u8; 8] = [0xFF, 0xFF, 0, 0, 0, 0x92, 0xCF, 0];
    const FLAT_DATA_A_SET: [u8; 8] = [0xFF, 0xFF, 0, 0, 0, 0x93, 0xCF, 0];

    fn flat(selector: u16, access: u8) -> ProtectedSegment {
        ProtectedSegment {
            selector,
            base: 0,
            limit: 0xFFFF_FFFF,
            access,
            flags: 0xC,
        }
    }

    /// State at 0x2000 as the block 5 jump leaves it, with the flat data selector 0x18 in EDX; the
    /// data registers are the real-mode ones. GDT at 0x38: null, code 0x08, data 0x10 (A clear),
    /// data 0x18 (A set).
    fn entry_cpu() -> ProtectedCpu {
        let mut gpr = [0u32; 8];
        gpr[EDX] = 0x0003_0018;
        let real = ProtectedSegment {
            selector: 0,
            base: 0,
            limit: 0xFFFF,
            access: REAL_MODE_DATA_ACCESS,
            flags: 0,
        };
        ProtectedCpu {
            gpr,
            eip: 0x2000,
            rip_high: 0,
            target_entered: false,
            eflags: 0x2,
            cr0: 0x0005_0033,
            cr3: 0,
            cr4: 0,
            efer: 0,
            pdptes: [0; 4],
            cs: flat(0x08, 0x9B),
            ds: real,
            es: real,
            ss: real,
            fs: real,
            gs: real,
            gdtr: TableRegister {
                base: 0x38,
                limit: 0x1F,
            },
            idtr: TableRegister { base: 0, limit: 0 },
            mov_ss_shadow: false,
        }
    }

    fn new_ram() -> Vec<u8> {
        let mut ram = vec![0u8; RAM];
        ram[0x40..0x48].copy_from_slice(&FLAT_CODE);
        ram[0x48..0x50].copy_from_slice(&FLAT_DATA_A_CLEAR);
        ram[0x50..0x58].copy_from_slice(&FLAT_DATA_A_SET);
        ram
    }

    /// The linked rows of `32bit-objdump.txt` (blob offsets 0x2000..) placed in `ram`.
    fn put_linked_block6(ram: &mut [u8]) {
        let text = include_str!("../../../docs/reports/data/stage6/s6-02-x86/32bit-objdump.txt");
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
    }

    fn put(ram: &mut [u8], at: usize, bytes: &[u8]) {
        ram[at..at + bytes.len()].copy_from_slice(bytes);
    }

    fn mem(ram: &mut [u8]) -> ImageMemoryMut<'_> {
        ImageMemoryMut {
            base: 0,
            bytes: ram,
        }
    }

    /// Runs the one instruction `code` at 0x2000 from `cpu`; asserts that a refusal changes
    /// neither the state nor the memory, and returns the result.
    fn step_one(
        cpu: &mut ProtectedCpu,
        ram: &mut Vec<u8>,
        code: &[u8],
    ) -> Result<ProtectedInstruction, ProtectedRefusal> {
        put(ram, 0x2000, code);
        let (before, ram_before) = (*cpu, ram.clone());
        let result = cpu.step(&mut mem(ram));
        if result.is_err() {
            assert_eq!(*cpu, before, "state unchanged by a refusal");
            assert_eq!(*ram, ram_before, "memory unchanged by a refusal");
        }
        result
    }

    #[test]
    fn wbinvd_advances_eip_in_protected_mode_without_data_changes() {
        let mut ram = new_ram();
        put(&mut ram, 0x2000, &[0x0f, 0x09]);
        let mut cpu = entry_cpu();
        let ram_before = ram.clone();
        let mut expected = cpu;
        expected.eip += 2;
        assert_eq!(
            cpu.step(&mut mem(&mut ram)),
            Ok(ProtectedInstruction::Wbinvd)
        );
        assert_eq!(cpu, expected);
        assert_eq!(ram, ram_before);
    }

    // ---- the linked bytes ----------------------------------------------------------------

    #[test]
    fn linked_bytes_run_13_steps_to_the_efer_rdmsr() {
        // S6-14k stopped here (the RDMSR was Unsupported); S6-14l runs on, so the bound is explicit.
        let mut ram = new_ram();
        put_linked_block6(&mut ram);
        put(&mut ram, 0x5010, &0x0000_06B0u32.to_le_bytes());
        put(&mut ram, 0x5014, &0u32.to_le_bytes());
        let mut cpu = entry_cpu();
        let run = cpu.run(&mut mem(&mut ram), 13);
        // mov ss, add, mov ds/es/fs/gs, bt, jae (taken), mov eax, mov cr4, mov eax, mov cr3, mov ecx.
        assert_eq!(
            run,
            ProtectedRun {
                steps: 13,
                stop: None
            }
        );
        assert_eq!(
            cpu.gpr[EDX], 0x0003_0018,
            "the RDMSR is not executed yet (it would load EDX = 0)"
        );
        assert_eq!(cpu.eip, 0x203E);
        for seg in [cpu.ss, cpu.ds, cpu.es, cpu.fs, cpu.gs] {
            assert_eq!(seg, flat(0x18, 0x93));
        }
        assert_eq!(cpu.gpr[EAX], 0x4000);
        assert_eq!(cpu.gpr[ECX], 0xC000_0080);
        assert_eq!((cpu.cr3, cpu.cr4), (0x4000, 0x6B0));
        assert_eq!(cpu.cr0, 0x0005_0033, "CR0 untouched");
        // `add esp, 0` on ESP = 0: ZF and PF; bt clears CF and keeps the rest; bit 1 stays.
        assert_eq!(cpu.eflags, 0x46);
        assert_eq!(cpu.cs, flat(0x08, 0x9B));
        assert_eq!(cpu.efer, 0);
    }

    #[test]
    fn sme_flag_bit_falls_through_to_the_optional_rdmsr() {
        let mut ram = new_ram();
        put_linked_block6(&mut ram);
        put(&mut ram, 0x5014, &1u32.to_le_bytes());
        let mut cpu = entry_cpu();
        let run = cpu.run(&mut mem(&mut ram), 100);
        // six loads/add, bt (CF = 1), jae not taken, mov ecx.
        assert_eq!(run.steps, 9);
        let Some(ProtectedRefusal::Unsupported { eip, bytes, .. }) = run.stop else {
            panic!("expected a stop at the SME RDMSR");
        };
        assert_eq!(eip, 0x201F);
        assert_eq!(&bytes[..2], &[0x0F, 0x32]);
        assert_eq!(cpu.gpr[ECX], 0xC001_0010);
        assert_eq!(cpu.eflags, 0x47, "bt sets CF");
    }

    #[test]
    fn only_bit_0_of_the_flag_word_selects_the_sme_branch() {
        let mut ram = new_ram();
        put_linked_block6(&mut ram);
        put(&mut ram, 0x5014, &0xFFFF_FFFEu32.to_le_bytes());
        let mut cpu = entry_cpu();
        let run = cpu.run(&mut mem(&mut ram), 13);
        assert_eq!(run.steps, 13);
        assert_eq!(cpu.eip, 0x203E);
    }

    #[test]
    fn mov_cr0_with_paging_without_lme_is_not_modelled_and_changes_nothing() {
        // S6-14l refused here for every EFER; S6-14m models PG with LME and PAE (below), so the
        // entry state (EFER = 0, LME clear: 32-bit/PAE paging) is the refusal that remains.
        // The linked tail `mov eax, 0x80050033; mov cr0, eax` (0x205d, 0x2062).
        let mut ram = new_ram();
        put_linked_block6(&mut ram);
        let mut cpu = entry_cpu();
        cpu.eip = 0x205D;
        let run = cpu.run(&mut mem(&mut ram), 10);
        assert_eq!(run.steps, 1);
        assert_eq!(
            run.stop,
            Some(ProtectedRefusal::PagingNotModelled {
                eip: 0x2062,
                value: 0x8005_0033
            })
        );
        assert_eq!(cpu.eip, 0x2062);
        assert_eq!(cpu.cr0, 0x0005_0033);
        assert_eq!(cpu.gpr[EAX], 0x8005_0033);
    }

    #[test]
    fn mov_cr0_without_paging_is_not_modelled_either() {
        let (mut cpu, mut ram) = (entry_cpu(), new_ram());
        cpu.gpr[EAX] = 0x0005_0033;
        assert_eq!(
            step_one(&mut cpu, &mut ram, &[0x0F, 0x22, 0xC0]),
            Err(ProtectedRefusal::Cr0WriteUnmodelled {
                eip: 0x2000,
                value: 0x0005_0033
            })
        );
    }

    // ---- segment loads -------------------------------------------------------------------

    #[test]
    fn every_data_register_loads_the_descriptor_and_sets_the_accessed_bit() {
        for (code, which) in [
            ([0x8E, 0xD2], ProtectedSegReg::Ss),
            ([0x8E, 0xDA], ProtectedSegReg::Ds),
            ([0x8E, 0xC2], ProtectedSegReg::Es),
            ([0x8E, 0xE2], ProtectedSegReg::Fs),
            ([0x8E, 0xEA], ProtectedSegReg::Gs),
        ] {
            let (mut cpu, mut ram) = (entry_cpu(), new_ram());
            cpu.gpr[EDX] = 0xABCD_0010; // only the low 16 bits are the selector
            assert_eq!(
                step_one(&mut cpu, &mut ram, &code),
                Ok(ProtectedInstruction::MovSegFromEdx(which))
            );
            let loaded = flat(0x10, 0x93);
            let all = [cpu.ss, cpu.ds, cpu.es, cpu.fs, cpu.gs];
            let index = [
                ProtectedSegReg::Ss,
                ProtectedSegReg::Ds,
                ProtectedSegReg::Es,
                ProtectedSegReg::Fs,
                ProtectedSegReg::Gs,
            ]
            .iter()
            .position(|r| *r == which)
            .unwrap();
            for (i, seg) in all.iter().enumerate() {
                if i == index {
                    assert_eq!(*seg, loaded);
                } else {
                    assert_eq!(seg.selector, 0, "other registers keep the real-mode cache");
                }
            }
            assert_eq!(ram[0x4D], 0x93, "A written to the GDT");
            assert_eq!(cpu.eip, 0x2002);
        }
    }

    #[test]
    fn accessed_bit_already_set_leaves_memory_alone() {
        let (mut cpu, mut ram) = (entry_cpu(), new_ram());
        let before = ram.clone();
        step_one(&mut cpu, &mut ram, &[0x8E, 0xDA]).unwrap();
        assert_eq!(cpu.ds, flat(0x18, 0x93));
        // Only the instruction bytes differ from the original memory.
        ram[0x2000] = 0;
        ram[0x2001] = 0;
        assert_eq!(ram, before);
    }

    #[test]
    fn descriptor_fields_come_from_the_descriptor() {
        // base 0x00123456 | 0x7a000000, raw limit 0x4ffff, flags 0xd (G, D/B, AVL), DPL 3 data.
        let mut ram = new_ram();
        put(
            &mut ram,
            0x58,
            &[0xFF, 0xFF, 0x56, 0x34, 0x12, 0xF3, 0xD4, 0x7A],
        );
        let mut cpu = entry_cpu();
        cpu.gdtr.limit = 0x27;
        cpu.gpr[EDX] = 0x23; // RPL 3 on a DPL 3 data segment: allowed for DS
        step_one(&mut cpu, &mut ram, &[0x8E, 0xDA]).unwrap();
        assert_eq!(
            cpu.ds,
            ProtectedSegment {
                selector: 0x23,
                base: 0x7A12_3456,
                limit: (0x4FFFF << 12) | 0xFFF,
                access: 0xF3,
                flags: 0xD,
            }
        );
    }

    #[test]
    fn segment_load_refusals_leave_state_and_memory_unchanged() {
        // GDT: 0x20 DPL-3 data, 0x28 code, 0x30 TSS (S = 0), 0x38 expand-down data,
        // 0x40 not-present data, 0x48 read-only data, 0x50 data DPL 1.
        let mut ram = new_ram();
        let at = |sel: usize| 0x38 + sel;
        put(&mut ram, at(0x20), &[0xFF, 0xFF, 0, 0, 0, 0xF2, 0xCF, 0]);
        put(&mut ram, at(0x28), &[0xFF, 0xFF, 0, 0, 0, 0x9A, 0xCF, 0]);
        put(&mut ram, at(0x30), &[0x67, 0, 0, 0, 0, 0x89, 0, 0]);
        put(&mut ram, at(0x38), &[0xFF, 0xFF, 0, 0, 0, 0x96, 0xCF, 0]);
        put(&mut ram, at(0x40), &[0xFF, 0xFF, 0, 0, 0, 0x12, 0xCF, 0]);
        put(&mut ram, at(0x48), &[0xFF, 0xFF, 0, 0, 0, 0x90, 0xCF, 0]);
        put(&mut ram, at(0x50), &[0xFF, 0xFF, 0, 0, 0, 0xB2, 0xCF, 0]);
        let ss = [0x8E, 0xD2];
        let ds = [0x8E, 0xDA];
        use SegmentLoadReason as R;
        let cases: [(&[u8; 2], u16, R); 14] = [
            (&ds, 0x0000, R::NullSelector),
            (&ds, 0x0003, R::NullSelector), // null with RPL 3 is still null
            (&ss, 0x0000, R::NullSelector),
            (&ds, 0x001C, R::LdtSelector),
            (&ds, 0x0060, R::BeyondGdtLimit { limit: 0x57 }),
            (&ss, 0x0023, R::Privilege { rpl: 3, dpl: 3 }), // SS needs RPL = DPL = 0
            (&ss, 0x0050, R::Privilege { rpl: 0, dpl: 1 }),
            (&ds, 0x0052, R::Privilege { rpl: 2, dpl: 1 }), // DPL < RPL
            (&ds, 0x0028, R::CodeSegment { access: 0x9A }),
            (&ss, 0x0028, R::NotWritableData { access: 0x9A }),
            (&ds, 0x0030, R::SystemDescriptor { access: 0x89 }),
            (&ds, 0x0038, R::ExpandDown { access: 0x96 }),
            (&ds, 0x0040, R::NotPresent),
            (&ss, 0x0048, R::NotWritableData { access: 0x90 }),
        ];
        for (code, selector, reason) in cases {
            let mut cpu = entry_cpu();
            cpu.gdtr.limit = 0x57;
            cpu.gpr[EDX] = u32::from(selector);
            let reg = if code[1] == 0xD2 {
                ProtectedSegReg::Ss
            } else {
                ProtectedSegReg::Ds
            };
            assert_eq!(
                step_one(&mut cpu, &mut ram, code),
                Err(ProtectedRefusal::SegmentLoadRefused {
                    eip: 0x2000,
                    reg,
                    selector,
                    reason
                }),
                "selector {selector:#x}"
            );
        }
    }

    #[test]
    fn unreadable_descriptor_and_unwritable_accessed_bit_are_refused() {
        // GDT base outside the image.
        let (mut cpu, mut ram) = (entry_cpu(), new_ram());
        cpu.gdtr = TableRegister {
            base: 0x7000_0000,
            limit: 0x1F,
        };
        assert_eq!(
            step_one(&mut cpu, &mut ram, &[0x8E, 0xDA]),
            Err(ProtectedRefusal::SegmentLoadRefused {
                eip: 0x2000,
                reg: ProtectedSegReg::Ds,
                selector: 0x18,
                reason: SegmentLoadReason::DescriptorUnreadable {
                    linear: 0x7000_0018
                }
            })
        );
        // A descriptor that would cross 4 GiB is unreadable, not wrapped.
        let (mut cpu, mut ram) = (entry_cpu(), new_ram());
        cpu.gdtr = TableRegister {
            base: 0xFFFF_FFFC,
            limit: 0x1F,
        };
        assert!(matches!(
            step_one(&mut cpu, &mut ram, &[0x8E, 0xDA]),
            Err(ProtectedRefusal::SegmentLoadRefused {
                reason: SegmentLoadReason::DescriptorUnreadable { .. },
                ..
            })
        ));
        // Read-only memory and a clear accessed bit (selector 0x10).
        let mut ram = new_ram();
        put(&mut ram, 0x2000, &[0x8E, 0xDA]);
        let mut cpu = entry_cpu();
        cpu.gpr[EDX] = 0x10;
        let before = cpu;
        let mut rom = ImageMemory {
            base: 0,
            bytes: &ram,
        };
        assert_eq!(
            cpu.step(&mut rom),
            Err(ProtectedRefusal::SegmentLoadRefused {
                eip: 0x2000,
                reg: ProtectedSegReg::Ds,
                selector: 0x10,
                reason: SegmentLoadReason::DescriptorUnwritable { linear: 0x4D }
            })
        );
        assert_eq!(cpu, before);
        // With the bit already set, read-only memory is enough (selector 0x18).
        cpu.gpr[EDX] = 0x18;
        assert!(cpu.step(&mut rom).is_ok());
    }

    #[test]
    fn mov_ss_shadow_follows_the_next_instruction() {
        let (mut cpu, mut ram) = (entry_cpu(), new_ram());
        put(
            &mut ram,
            0x2000,
            &[0x8E, 0xD2, 0x81, 0xC4, 0, 0, 0, 0, 0x90],
        );
        cpu.step(&mut mem(&mut ram)).unwrap();
        assert!(cpu.mov_ss_shadow);
        cpu.step(&mut mem(&mut ram)).unwrap();
        assert!(!cpu.mov_ss_shadow, "ends after the next instruction");
        // Two MOV SS in a row: the second re-arms it (conservative, as in the real-mode module).
        let (mut cpu, mut ram) = (entry_cpu(), new_ram());
        put(&mut ram, 0x2000, &[0x8E, 0xD2, 0x8E, 0xD2]);
        cpu.step(&mut mem(&mut ram)).unwrap();
        cpu.step(&mut mem(&mut ram)).unwrap();
        assert!(cpu.mov_ss_shadow);
    }

    // ---- ADD, BT, JAE, MOV ---------------------------------------------------------------

    #[test]
    fn add_esp_flags_and_result() {
        for (esp, imm, result, arith) in [
            (0u32, 0u32, 0u32, 0x44u32),          // ZF, PF
            (0xFFFF_FFFF, 1, 0, 0x55),            // CF, AF, ZF, PF
            (0x7FFF_FFFF, 1, 0x8000_0000, 0x894), // OF, SF, AF, PF
            (0x1000, 0x20, 0x1020, 0x00),         // 0x20: one bit set: PF clear
            (0x1000, 0x3, 0x1003, 0x04),          // two bits: PF
            (0x8000_0000, 0x8000_0000, 0, 0x845), // CF, OF, ZF, PF
        ] {
            let (mut cpu, mut ram) = (entry_cpu(), new_ram());
            cpu.gpr[ESP] = esp;
            cpu.eflags = 0x0000_0202 | 0x1; // IF, reserved bit 1, a stale CF
            let mut code = vec![0x81, 0xC4];
            code.extend_from_slice(&imm.to_le_bytes());
            assert_eq!(
                step_one(&mut cpu, &mut ram, &code),
                Ok(ProtectedInstruction::AddEspImm32(imm))
            );
            assert_eq!(cpu.gpr[ESP], result, "{esp:#x} + {imm:#x}");
            assert_eq!(cpu.eflags, 0x202 | arith, "{esp:#x} + {imm:#x}");
            assert_eq!(cpu.eip, 0x2006);
        }
    }

    #[test]
    fn bt_sets_cf_from_the_bit_and_keeps_other_flags() {
        for (word, imm, cf) in [
            (0x0000_0001u32, 0u8, true),
            (0x0000_0000, 0, false),
            (0x0000_0002, 1, true),
            (0x0000_0002, 0x21, true), // bit offset modulo 32
            (0x8000_0000, 31, true),
            (0x8000_0000, 0x7F, true), // 0x7f mod 32 = 31
            (0xFFFF_FFFE, 0x20, false),
        ] {
            let (mut cpu, mut ram) = (entry_cpu(), new_ram());
            put(&mut ram, 0x5014, &word.to_le_bytes());
            cpu.eflags = 0x2 | 0x80 | 0x40 | u32::from(!cf);
            let code = [0x0F, 0xBA, 0x25, 0x14, 0x50, 0, 0, imm];
            assert_eq!(
                step_one(&mut cpu, &mut ram, &code),
                Ok(ProtectedInstruction::BtDsImm8 {
                    disp: 0x5014,
                    bit: imm
                })
            );
            assert_eq!(
                cpu.eflags,
                0x2 | 0xC0 | u32::from(cf),
                "{word:#x} bit {imm}"
            );
            assert_eq!(cpu.eip, 0x2008);
            assert_eq!(
                &ram[0x5014..0x5018],
                &word.to_le_bytes(),
                "bt does not write"
            );
        }
    }

    #[test]
    fn data_operands_go_through_the_ds_base_and_limit() {
        let (mut cpu, mut ram) = (entry_cpu(), new_ram());
        cpu.ds = ProtectedSegment {
            selector: 0x18,
            base: 0x1000,
            limit: 0x5017,
            access: 0x93,
            flags: 0x4,
        };
        put(&mut ram, 0x6010, &0xDEAD_BEEFu32.to_le_bytes());
        put(&mut ram, 0x6014, &1u32.to_le_bytes());
        put(&mut ram, 0x5010, &0x1111_1111u32.to_le_bytes()); // a decoy at the unbased address
        step_one(&mut cpu, &mut ram, &[0xA1, 0x10, 0x50, 0, 0]).unwrap();
        assert_eq!(cpu.gpr[EAX], 0xDEAD_BEEF);
        assert_eq!(cpu.eip, 0x2005);
        cpu.eip = 0x2000;
        let code = [0x0F, 0xBA, 0x25, 0x14, 0x50, 0, 0, 0];
        step_one(&mut cpu, &mut ram, &code).unwrap();
        assert_eq!(cpu.eflags & 1, 1);
        // The last byte of the operand at the limit: ok; one past: refused.
        cpu.eip = 0x2000;
        step_one(&mut cpu, &mut ram, &[0xA1, 0x14, 0x50, 0, 0]).unwrap();
        cpu.eip = 0x2000;
        assert_eq!(
            step_one(&mut cpu, &mut ram, &[0xA1, 0x15, 0x50, 0, 0]),
            Err(ProtectedRefusal::DataBeyondLimit {
                eip: 0x2000,
                disp: 0x5015,
                limit: 0x5017
            })
        );
        assert_eq!(
            step_one(
                &mut cpu,
                &mut ram,
                &[0x0F, 0xBA, 0x25, 0xFD, 0xFF, 0xFF, 0xFF, 0]
            ),
            Err(ProtectedRefusal::DataBeyondLimit {
                eip: 0x2000,
                disp: 0xFFFF_FFFD,
                limit: 0x5017
            })
        );
    }

    #[test]
    fn unreadable_data_is_refused() {
        let (mut cpu, mut ram) = (entry_cpu(), new_ram());
        cpu.ds = flat(0x18, 0x93);
        // The image ends at 0x6100: the dword at 0x60fe is half outside.
        assert_eq!(
            step_one(&mut cpu, &mut ram, &[0xA1, 0xFE, 0x60, 0, 0]),
            Err(ProtectedRefusal::DataUnreadable {
                eip: 0x2000,
                linear: 0x6100
            })
        );
    }

    #[test]
    fn jae_follows_cf() {
        for (cf, rel, taken_to) in [
            (0u32, 0x0Fu8, Some(0x2011u32)),
            (1, 0x0F, None),
            (0, 0xFE, Some(0x2000)), // jae to itself
            (0, 0x80, Some(0x2002 - 128)),
        ] {
            let (mut cpu, mut ram) = (entry_cpu(), new_ram());
            cpu.eflags = 2 | cf;
            // The target must be fetched later; here only EIP is checked.
            assert_eq!(
                step_one(&mut cpu, &mut ram, &[0x73, rel]),
                Ok(ProtectedInstruction::JaeRel8(rel as i8))
            );
            assert_eq!(cpu.eip, taken_to.unwrap_or(0x2002), "cf {cf} rel {rel:#x}");
        }
    }

    #[test]
    fn taken_jae_beyond_the_cs_limit_is_refused() {
        let (mut cpu, mut ram) = (entry_cpu(), new_ram());
        cpu.cs.limit = 0x2010;
        assert_eq!(
            step_one(&mut cpu, &mut ram, &[0x73, 0x0F]),
            Err(ProtectedRefusal::TargetBeyondLimit {
                eip: 0x2000,
                offset: 0x2011,
                limit: 0x2010
            })
        );
        // Not taken: the target is not checked.
        cpu.eflags |= 1;
        assert!(step_one(&mut cpu, &mut ram, &[0x73, 0x0F]).is_ok());
    }

    #[test]
    fn mov_imm32_and_eax_loads_write_whole_registers_only() {
        let (mut cpu, mut ram) = (entry_cpu(), new_ram());
        cpu.eflags = 0x246;
        step_one(&mut cpu, &mut ram, &[0xB8, 0x78, 0x56, 0x34, 0x12]).unwrap();
        cpu.eip = 0x2000;
        step_one(&mut cpu, &mut ram, &[0xB9, 0x80, 0x00, 0x00, 0xC0]).unwrap();
        assert_eq!(cpu.gpr[EAX], 0x1234_5678);
        assert_eq!(cpu.gpr[ECX], 0xC000_0080);
        assert_eq!(cpu.eflags, 0x246, "MOV changes no flag");
        assert_eq!(cpu.gpr[EDX], 0x0003_0018);
        for r in [EBX, ESP, EBP, ESI, EDI] {
            assert_eq!(cpu.gpr[r], 0);
        }
    }

    // ---- CR4 and CR3 ---------------------------------------------------------------------

    #[test]
    fn mov_cr4_stores_bits_0_to_10_and_refuses_the_rest() {
        for value in [0u32, 0x20, 0x6B0, 0x7FF] {
            let (mut cpu, mut ram) = (entry_cpu(), new_ram());
            cpu.gpr[EAX] = value;
            assert_eq!(
                step_one(&mut cpu, &mut ram, &[0x0F, 0x22, 0xE0]),
                Ok(ProtectedInstruction::MovCr4Eax)
            );
            assert_eq!((cpu.cr4, cpu.cr3, cpu.cr0), (value, 0, 0x0005_0033));
            assert_eq!(cpu.eip, 0x2003);
        }
        for value in [
            0x800u32,
            0x1000,
            0x2000,
            0x1_0000,
            0x4_0000,
            0x20_0000,
            0x8000_0000,
            0xFFFF_FFFF,
        ] {
            let (mut cpu, mut ram) = (entry_cpu(), new_ram());
            cpu.cr4 = 0x20;
            cpu.gpr[EAX] = value;
            assert_eq!(
                step_one(&mut cpu, &mut ram, &[0x0F, 0x22, 0xE0]),
                Err(ProtectedRefusal::Cr4WriteUnmodelled { eip: 0x2000, value })
            );
            assert_eq!(cpu.cr4, 0x20);
        }
    }

    #[test]
    fn mov_cr3_stores_the_value_and_enables_nothing() {
        for value in [0u32, 0x4000, 0x0010_0018, 0xFFFF_FFFF] {
            let (mut cpu, mut ram) = (entry_cpu(), new_ram());
            cpu.cr4 = 0x20;
            cpu.gpr[EAX] = value;
            assert_eq!(
                step_one(&mut cpu, &mut ram, &[0x0F, 0x22, 0xD8]),
                Ok(ProtectedInstruction::MovCr3Eax)
            );
            assert_eq!((cpu.cr3, cpu.cr4, cpu.cr0), (value, 0x20, 0x0005_0033));
            assert_eq!(cpu.gpr[EAX], value);
        }
    }

    // ---- RDMSR / WRMSR of EFER ----------------------------------------------------------

    fn efer_cpu(efer: u64) -> ProtectedCpu {
        let mut cpu = entry_cpu();
        cpu.gpr[ECX] = MSR_EFER;
        cpu.gpr[EDX] = 0;
        cpu.efer = efer;
        cpu
    }

    #[test]
    fn rdmsr_efer_returns_the_value_in_edx_eax() {
        for (efer, eax, edx) in [
            (0u64, 0u32, 0u32),
            (0x901, 0x901, 0),
            (0x0000_0001_0000_0D01, 0xD01, 1), // a high half cannot be written, but is read as is
        ] {
            let (mut cpu, mut ram) = (efer_cpu(efer), new_ram());
            cpu.eflags = 0x246;
            cpu.gpr[EAX] = 0xAAAA_AAAA;
            cpu.gpr[EDX] = 0xBBBB_BBBB;
            assert_eq!(
                step_one(&mut cpu, &mut ram, &[0x0F, 0x32]),
                Ok(ProtectedInstruction::Rdmsr)
            );
            assert_eq!((cpu.gpr[EAX], cpu.gpr[EDX]), (eax, edx));
            assert_eq!(cpu.gpr[ECX], MSR_EFER, "ECX is not written");
            assert_eq!((cpu.eflags, cpu.eip, cpu.efer), (0x246, 0x2002, efer));
        }
    }

    #[test]
    fn rdmsr_and_wrmsr_of_any_other_msr_are_unsupported() {
        for msr in [
            0u32,
            0x1B,
            0xC001_0010, // SYSCFG: the optional SME branch
            0xC000_0081,
            0xC000_007F,
            0x4000_0080, // only the full index counts
            0xC000_0080 | 0x1_0000,
        ] {
            for code in [[0x0F, 0x32], [0x0F, 0x30]] {
                let (mut cpu, mut ram) = (efer_cpu(0x901), new_ram());
                cpu.gpr[ECX] = msr;
                cpu.gpr[EAX] = 0x100;
                match step_one(&mut cpu, &mut ram, &code) {
                    Err(ProtectedRefusal::Unsupported { eip, bytes, .. }) => {
                        assert_eq!(eip, 0x2000);
                        assert_eq!(&bytes[..2], &code);
                    }
                    other => panic!("msr {msr:#x} {code:02x?}: {other:?}"),
                }
            }
        }
    }

    #[test]
    fn wrmsr_efer_stores_sce_lme_nxe_from_edx_eax() {
        for (value, stored) in [
            (0u64, 0u64),
            (0x1, 0x1),
            (0x100, 0x100),
            (0x800, 0x800),
            (0x901, 0x901), // SCE | LME | NXE: what Linux writes for the BSP
            (0x101, 0x101),
        ] {
            let (mut cpu, mut ram) = (efer_cpu(0), new_ram());
            cpu.gpr[EAX] = value as u32;
            cpu.gpr[EDX] = (value >> 32) as u32;
            cpu.eflags = 0x246;
            assert_eq!(
                step_one(&mut cpu, &mut ram, &[0x0F, 0x30]),
                Ok(ProtectedInstruction::Wrmsr)
            );
            assert_eq!(cpu.efer, stored, "{value:#x}");
            assert_eq!((cpu.gpr[EAX], cpu.gpr[ECX]), (value as u32, MSR_EFER));
            assert_eq!((cpu.eflags, cpu.eip), (0x246, 0x2002));
        }
        // A later write replaces the whole value (it is not or-ed in).
        let (mut cpu, mut ram) = (efer_cpu(0x901), new_ram());
        cpu.gpr[EAX] = 0x100;
        step_one(&mut cpu, &mut ram, &[0x0F, 0x30]).unwrap();
        assert_eq!(cpu.efer, 0x100);
    }

    #[test]
    fn wrmsr_efer_ignores_the_lma_bit_and_keeps_lma() {
        // KVM oracle: 0x401 reads back 0x1, 0x400 reads back 0, no fault.
        for (efer_before, value, after) in [
            (0u64, 0x401u64, 0x1u64),
            (0, 0x400, 0),
            (0, 0xD01, 0x901),
            // LMA is only set by the processor (never in this slice): an LMA that is set stays set.
            (0x400, 0x101, 0x501),
            (0x400, 0, 0x400),
            (0x401, 0x400, 0x400),
        ] {
            let (mut cpu, mut ram) = (efer_cpu(efer_before), new_ram());
            cpu.gpr[EAX] = value as u32;
            cpu.gpr[EDX] = (value >> 32) as u32;
            step_one(&mut cpu, &mut ram, &[0x0F, 0x30]).unwrap();
            assert_eq!(cpu.efer, after, "{efer_before:#x} <- {value:#x}");
        }
    }

    #[test]
    fn wrmsr_efer_refuses_every_reserved_bit_and_keeps_the_state() {
        let allowed = EFER_WRITABLE_MASK;
        for bit in 0..64u32 {
            let value = 1u64 << bit;
            let (mut cpu, mut ram) = (efer_cpu(0x101), new_ram());
            cpu.gpr[EAX] = value as u32;
            cpu.gpr[EDX] = (value >> 32) as u32;
            cpu.eflags = 0x246;
            let result = step_one(&mut cpu, &mut ram, &[0x0F, 0x30]);
            if allowed & value != 0 {
                assert_eq!(result, Ok(ProtectedInstruction::Wrmsr), "bit {bit}");
            } else {
                // `step_one` has checked that the state and the memory are unchanged.
                assert_eq!(
                    result,
                    Err(ProtectedRefusal::EferWriteReservedBits { eip: 0x2000, value }),
                    "bit {bit}"
                );
                assert_eq!(cpu.efer, 0x101, "bit {bit}");
            }
        }
        // A reserved bit next to legal ones refuses the whole write.
        let (mut cpu, mut ram) = (efer_cpu(0), new_ram());
        cpu.gpr[EAX] = 0x901 | 0x2;
        assert!(matches!(
            step_one(&mut cpu, &mut ram, &[0x0F, 0x30]),
            Err(ProtectedRefusal::EferWriteReservedBits { value: 0x903, .. })
        ));
        assert_eq!(cpu.efer, 0);
    }

    // ---- CMP, JNE, JE, MOV EDX ----------------------------------------------------------

    #[test]
    fn cmp_with_ds_operand_sets_the_six_arithmetic_flags_only() {
        // (register, operand, expected CF/PF/AF/ZF/SF/OF) by SDM Vol. 1 appendix A, from memory.
        const CF: u32 = 0x1;
        const PF: u32 = 0x4;
        const AF: u32 = 0x10;
        const ZF: u32 = 0x40;
        const SF: u32 = 0x80;
        const OF: u32 = 0x800;
        for (lhs, rhs, flags) in [
            (5u32, 5u32, ZF | PF),
            (0, 0, ZF | PF),
            (0, 1, CF | AF | SF | PF),                     // 0xffffffff
            (1, 0, 0),                                     // 0x01: one bit, PF clear
            (4, 1, PF),                                    // 0x03
            (3, 1, 0),                                     // 0x02
            (0x8000_0000, 1, OF | AF | PF),                // 0x7fffffff
            (0x7FFF_FFFF, 0xFFFF_FFFF, CF | OF | SF | PF), // 0x80000000
            (0x1000, 0x1001, CF | AF | SF | PF),
            (0x901, 0xD01, CF | SF | PF), // 0xfffffc00: low byte 0 (PF)
            (0xD01, 0x901, PF),           // 0x400
        ] {
            for (code, reg) in [
                ([0x3B, 0x05, 0x08, 0x50, 0, 0], EAX),
                ([0x3B, 0x15, 0x08, 0x50, 0, 0], EDX),
            ] {
                let (mut cpu, mut ram) = (entry_cpu(), new_ram());
                cpu.ds = flat(0x18, 0x93);
                put(&mut ram, 0x5008, &rhs.to_le_bytes());
                cpu.gpr[reg] = lhs;
                // Reserved bit 1, IF, RF and TF stay; all six arithmetic bits start set.
                cpu.eflags = 0x2 | 0x200 | 0x10000 | 0x100 | 0x8D5;
                let before_gpr = cpu.gpr;
                let insn = step_one(&mut cpu, &mut ram, &code).unwrap();
                assert_eq!(
                    insn,
                    if reg == EAX {
                        ProtectedInstruction::CmpEaxWithDs(0x5008)
                    } else {
                        ProtectedInstruction::CmpEdxWithDs(0x5008)
                    }
                );
                assert_eq!(
                    cpu.eflags,
                    0x2 | 0x200 | 0x10000 | 0x100 | flags,
                    "{lhs:#x} - {rhs:#x}"
                );
                assert_eq!(cpu.gpr, before_gpr, "CMP writes no register");
                assert_eq!(
                    &ram[0x5008..0x500C],
                    &rhs.to_le_bytes(),
                    "CMP writes no memory"
                );
                assert_eq!(cpu.eip, 0x2006);
            }
        }
    }

    #[test]
    fn cmp_and_mov_edx_operands_go_through_the_ds_base_and_limit() {
        let (mut cpu, mut ram) = (entry_cpu(), new_ram());
        cpu.ds = ProtectedSegment {
            selector: 0x18,
            base: 0x1000,
            limit: 0x500F,
            access: 0x93,
            flags: 0x4,
        };
        put(&mut ram, 0x6008, &7u32.to_le_bytes());
        put(&mut ram, 0x600C, &0xCAFE_F00Du32.to_le_bytes());
        put(&mut ram, 0x5008, &9u32.to_le_bytes()); // decoy at the unbased address
        cpu.gpr[EAX] = 7;
        step_one(&mut cpu, &mut ram, &[0x3B, 0x05, 0x08, 0x50, 0, 0]).unwrap();
        assert_eq!(cpu.eflags & EFLAGS_ZF, EFLAGS_ZF, "7 - 7");
        cpu.eip = 0x2000;
        step_one(&mut cpu, &mut ram, &[0x8B, 0x15, 0x0C, 0x50, 0, 0]).unwrap();
        assert_eq!(cpu.gpr[EDX], 0xCAFE_F00D);
        assert_eq!(cpu.eip, 0x2006);
        // The last dword under the limit is fine, one byte further is refused, for all three.
        for code in [
            [0x3B, 0x05, 0x0D, 0x50, 0, 0],
            [0x3B, 0x15, 0x0D, 0x50, 0, 0],
            [0x8B, 0x15, 0x0D, 0x50, 0, 0],
        ] {
            cpu.eip = 0x2000;
            assert_eq!(
                step_one(&mut cpu, &mut ram, &code),
                Err(ProtectedRefusal::DataBeyondLimit {
                    eip: 0x2000,
                    disp: 0x500D,
                    limit: 0x500F
                })
            );
        }
        cpu.eip = 0x2000;
        cpu.ds = flat(0x18, 0x93);
        // The image ends at 0x6100: an operand half outside is unreadable.
        assert_eq!(
            step_one(&mut cpu, &mut ram, &[0x3B, 0x15, 0xFE, 0x60, 0, 0]),
            Err(ProtectedRefusal::DataUnreadable {
                eip: 0x2000,
                linear: 0x6100
            })
        );
    }

    #[test]
    fn jne_and_je_follow_zf_and_leave_the_flags() {
        for (zf, rel, jne_to, je_to) in [
            (0u32, 0x08u8, Some(0x200Au32), None),
            (1, 0x08, None, Some(0x200A)),
            (0, 0xFE, Some(0x2000), None), // to itself
            (1, 0xFE, None, Some(0x2000)),
            (0, 0x80, Some(0x2002 - 128), None),
        ] {
            for (code, taken_to, expect) in [
                (0x75u8, jne_to, ProtectedInstruction::JneRel8(rel as i8)),
                (0x74u8, je_to, ProtectedInstruction::JeRel8(rel as i8)),
            ] {
                let (mut cpu, mut ram) = (entry_cpu(), new_ram());
                cpu.eflags = 0x2 | 0x80 | 0x1 | (zf << 6);
                assert_eq!(step_one(&mut cpu, &mut ram, &[code, rel]), Ok(expect));
                assert_eq!(
                    cpu.eip,
                    taken_to.unwrap_or(0x2002),
                    "{code:#x} zf {zf} rel {rel:#x}"
                );
                assert_eq!(
                    cpu.eflags,
                    0x2 | 0x80 | 0x1 | (zf << 6),
                    "no flag is written"
                );
            }
        }
    }

    #[test]
    fn taken_jne_and_je_beyond_the_cs_limit_are_refused_and_untaken_ones_are_not_checked() {
        for (code, zf_taken) in [(0x75u8, 0u32), (0x74, 1)] {
            let (mut cpu, mut ram) = (entry_cpu(), new_ram());
            cpu.cs.limit = 0x2010;
            cpu.eflags = 2 | (zf_taken << 6);
            assert_eq!(
                step_one(&mut cpu, &mut ram, &[code, 0x0F]),
                Err(ProtectedRefusal::TargetBeyondLimit {
                    eip: 0x2000,
                    offset: 0x2011,
                    limit: 0x2010
                })
            );
            // The limit itself is allowed.
            assert!(step_one(&mut cpu, &mut ram, &[code, 0x0E]).is_ok());
            assert_eq!(cpu.eip, 0x2010);
            // Not taken: no check.
            let mut cpu = entry_cpu();
            cpu.cs.limit = 0x2010;
            cpu.eflags = 2 | ((1 - zf_taken) << 6);
            assert!(step_one(&mut cpu, &mut ram, &[code, 0x0F]).is_ok());
            assert_eq!(cpu.eip, 0x2002);
        }
        // A negative displacement that wraps below zero lands beyond the limit.
        let (mut cpu, mut ram) = (entry_cpu(), new_ram());
        cpu.cs.limit = 0xFFFF;
        cpu.eip = 0x10;
        cpu.eflags = 2;
        put(&mut ram, 0x10, &[0x75, 0x80]);
        assert_eq!(
            cpu.step(&mut mem(&mut ram)),
            Err(ProtectedRefusal::TargetBeyondLimit {
                eip: 0x10,
                offset: 0xFFFF_FF92,
                limit: 0xFFFF
            })
        );
    }

    // ---- the linked path 0x203e..0x2062 ----------------------------------------------------

    /// Runs the linked block 6 from the entry state with the header EFER `(lo, hi)` and the
    /// current `efer`, up to the refusal; returns the CPU and the run.
    fn run_linked_with_header(efer: u64, lo: u32, hi: u32) -> (ProtectedCpu, ProtectedRun) {
        let mut ram = new_ram();
        put_linked_block6(&mut ram);
        put(&mut ram, 0x5008, &lo.to_le_bytes());
        put(&mut ram, 0x500C, &hi.to_le_bytes());
        let mut cpu = entry_cpu();
        cpu.efer = efer;
        let run = cpu.run(&mut mem(&mut ram), 100);
        (cpu, run)
    }

    #[test]
    fn equal_efer_skips_the_wrmsr_and_stops_at_the_paging_write() {
        // EFER = header: rdmsr, cmp, jne (not taken), cmp, je (taken) to 0x205d, mov eax.
        for efer in [0u64, 0x901] {
            let (cpu, run) = run_linked_with_header(efer, efer as u32, 0);
            assert_eq!(run.steps, 13 + 6);
            // S6-14l: PagingNotModelled for both. S6-14m: EFER 0 has no LME (32-bit paging, still
            // not modelled); 0x901 has LME, and CR4 = 0 in this test has no PAE (`#GP`).
            let value = 0x8005_0033;
            let expected = if efer == 0 {
                ProtectedRefusal::PagingNotModelled { eip: 0x2062, value }
            } else {
                ProtectedRefusal::LongModePagingWithoutPae { eip: 0x2062, value }
            };
            assert_eq!(run.stop, Some(expected));
            assert_eq!((cpu.efer, cpu.eip), (efer, 0x2062));
            assert_eq!((cpu.gpr[EAX], cpu.gpr[EDX]), (0x8005_0033, 0));
            assert_eq!(cpu.cr0, 0x0005_0033, "CR0 untouched");
        }
    }

    #[test]
    fn different_efer_is_written_from_the_header() {
        // rdmsr, cmp, jne (taken) to 0x2050, mov eax, mov edx, wrmsr, mov eax.
        for (header, efer_after) in [(0x901u32, 0x901u64), (0x100, 0x100), (0xD01, 0x901)] {
            let (cpu, run) = run_linked_with_header(0, header, 0);
            assert_eq!(run.steps, 13 + 7, "header {header:#x}");
            // Every header has LME and CR4 = 0 (no PAE): `#GP` at the `MOV CR0` (S6-14l: PagingNotModelled).
            assert!(
                matches!(
                    run.stop,
                    Some(ProtectedRefusal::LongModePagingWithoutPae { eip: 0x2062, .. })
                ),
                "header {header:#x}"
            );
            assert_eq!(cpu.efer, efer_after, "header {header:#x}");
            assert_eq!((cpu.gpr[EAX], cpu.gpr[EDX]), (0x8005_0033, 0));
        }
        // Same low dword, different high dword: JNE falls through, CMP EDX differs, JE not
        // taken, the write goes ahead (EDX:EAX = 1:0) and is refused (bit 32): the stop is at the
        // WRMSR with the loads done and EFER unchanged.
        let (cpu, run) = run_linked_with_header(0, 0, 1);
        assert_eq!(run.steps, 13 + 7);
        assert_eq!(
            run.stop,
            Some(ProtectedRefusal::EferWriteReservedBits {
                eip: 0x205B,
                value: 1 << 32
            })
        );
        assert_eq!((cpu.eip, cpu.efer), (0x205B, 0));
        assert_eq!((cpu.gpr[EAX], cpu.gpr[EDX]), (0, 1));
        // A reserved bit in the low dword: refused at the same WRMSR.
        let (cpu, run) = run_linked_with_header(0, 0x2, 0);
        assert_eq!(run.steps, 13 + 5);
        assert!(matches!(
            run.stop,
            Some(ProtectedRefusal::EferWriteReservedBits {
                eip: 0x205B,
                value: 2
            })
        ));
        assert_eq!(cpu.efer, 0);
    }

    // ---- decode boundaries ---------------------------------------------------------------

    #[test]
    fn everything_outside_the_closed_set_is_unsupported() {
        let rejected: [&[u8]; 20] = [
            &[0x0F, 0x32],                            // RDMSR, ECX = 0 (not EFER)
            &[0x0F, 0x30],                            // WRMSR, ECX = 0 (not EFER)
            &[0xF4],                                  // HLT
            &[0x66, 0xB8, 1, 0],                      // operand-size prefix
            &[0x67, 0xA1, 0, 0, 0, 0],                // address-size prefix
            &[0xF0, 0x0F, 0xBA, 0x25, 0, 0, 0, 0, 0], // LOCK prefix
            &[0x8E, 0xC8],                            // MOV CS, EAX
            &[0x8E, 0xD1],                            // MOV SS, ECX (rm != EDX)
            &[0x8E, 0xF2],                            // reg field 6
            &[0x8E, 0x12],                            // memory operand
            &[0x8C, 0xD2],                            // MOV r/m, Sreg
            &[0x81, 0xC3, 0, 0, 0, 0],                // ADD EBX, imm32
            &[0x0F, 0xBA, 0x2D, 0, 0, 0, 0, 0],       // BT with another ModRM
            &[0x0F, 0xBA, 0x24, 0x25, 0, 0, 0, 0, 0], // BT with SIB
            &[0x0F, 0x22, 0xC8],                      // MOV CR1
            &[0x0F, 0x20, 0xC0],                      // MOV EAX, CR0
            &[0x39, 0x05, 0, 0, 0, 0],                // CMP m32, EAX
            &[0x0F, 0x85, 8, 0, 0, 0],                // JNE rel32
            &[0x8B, 0x0D, 0, 0, 0, 0],                // MOV ECX, m32
            &[0x9A, 0, 0, 0, 0, 0x10, 0],             // far CALL
        ];
        for code in rejected {
            let (mut cpu, mut ram) = (entry_cpu(), new_ram());
            match step_one(&mut cpu, &mut ram, code) {
                Err(ProtectedRefusal::Unsupported { eip, linear, bytes }) => {
                    assert_eq!((eip, linear), (0x2000, 0x2000));
                    assert_eq!(&bytes[..code.len().min(8)], &code[..code.len().min(8)]);
                }
                other => panic!("{code:02x?}: {other:?}"),
            }
        }
    }

    #[test]
    fn fetch_limits_and_a_16_bit_cs_are_refused() {
        let (mut cpu, mut ram) = (entry_cpu(), new_ram());
        cpu.cs.limit = 0x2002;
        put(&mut ram, 0x2000, &[0xB8, 1, 2, 3, 4]);
        assert_eq!(
            cpu.step(&mut mem(&mut ram)),
            Err(ProtectedRefusal::FetchBeyondLimit {
                eip: 0x2000,
                offset: 0x2003
            })
        );
        let (mut cpu, mut ram) = (entry_cpu(), new_ram());
        cpu.eip = 0x60FE;
        put(&mut ram, 0x60FE, &[0xB8]);
        assert_eq!(
            cpu.step(&mut mem(&mut ram)),
            Err(ProtectedRefusal::FetchUnreadable {
                eip: 0x60FE,
                linear: 0x6100
            })
        );
        let (mut cpu, mut ram) = (entry_cpu(), new_ram());
        cpu.cs.flags = 0x8; // G only: D/B = 0
        assert_eq!(
            step_one(&mut cpu, &mut ram, &[0xB8, 0, 0, 0, 0]),
            Err(ProtectedRefusal::CsNot32Bit { flags: 0x8 })
        );
    }

    #[test]
    fn a_refusal_in_the_middle_of_a_run_keeps_the_earlier_effects() {
        // mov ss (ok, A set), then `mov eax, ds:0x10000` beyond the real-mode DS limit.
        let (mut cpu, mut ram) = (entry_cpu(), new_ram());
        cpu.gpr[EDX] = 0x10;
        put(&mut ram, 0x2000, &[0x8E, 0xD2, 0xA1, 0, 0, 1, 0]);
        let run = cpu.run(&mut mem(&mut ram), 10);
        assert_eq!(run.steps, 1);
        assert_eq!(
            run.stop,
            Some(ProtectedRefusal::DataBeyondLimit {
                eip: 0x2002,
                disp: 0x1_0000,
                limit: 0xFFFF
            })
        );
        assert_eq!(cpu.ss.selector, 0x10);
        assert_eq!(cpu.eip, 0x2002);
        assert_eq!(ram[0x4D], 0x93);
    }

    // ---- entry from the real-mode module -------------------------------------------------

    #[test]
    fn entry_requires_the_block_5_jump() {
        let mut real = RealModeCpu::at_cs_ip(0, 0x1044, 0, 0x0003_06A9);
        real.mode.cr0 = 0x0005_0033;
        assert_eq!(
            ProtectedCpu::from_real_entry(&real),
            Err(ProtectedEntryRefusal::NotEntered {
                cr0: 0x0005_0033,
                cs_loaded: false
            })
        );
        real.cs_protected = Some(flat(0x08, 0x9B));
        real.mode.cr0 = 0x10;
        assert_eq!(
            ProtectedCpu::from_real_entry(&real),
            Err(ProtectedEntryRefusal::NotEntered {
                cr0: 0x10,
                cs_loaded: true
            })
        );
    }

    #[test]
    fn entry_copies_the_state_and_gives_real_segments_their_attributes() {
        let mut real = RealModeCpu::at_cs_ip(0, 0, 0, 0x0003_06A9);
        real.mode.cr0 = 0x0005_0033;
        real.mode.eip = 0x2000;
        real.mode.eflags = 0x46;
        real.mode.ds = RealSegment {
            selector: 0x1234,
            base: 0x12340,
            limit: 0xFFFF,
        };
        real.gpr[EAX] = 7;
        real.gdtr = TableRegister {
            base: 0x38,
            limit: 0x1F,
        };
        real.idtr = TableRegister {
            base: 0x60,
            limit: 0,
        };
        real.cs_protected = Some(flat(0x08, 0x9B));
        let cpu = ProtectedCpu::from_real_entry(&real).unwrap();
        assert_eq!(cpu.gpr, real.gpr);
        assert_eq!((cpu.eip, cpu.eflags, cpu.cr0), (0x2000, 0x46, 0x0005_0033));
        assert_eq!((cpu.cr3, cpu.cr4, cpu.efer), (0, 0, 0));
        assert_eq!(cpu.cs, flat(0x08, 0x9B));
        assert_eq!(
            cpu.ds,
            ProtectedSegment {
                selector: 0x1234,
                base: 0x12340,
                limit: 0xFFFF,
                access: 0x93,
                flags: 0
            }
        );
        assert_eq!((cpu.gdtr, cpu.idtr), (real.gdtr, real.idtr));
    }

    // ---- comparison with the KVM oracle (S6-14k) ------------------------------------------
    //
    // Files in docs/reports/data/stage6/s6-14k/ (make-images.py, run-oracle.sh). Every image is
    // entered at 0000:0000, runs the linked bytes 0x1027..0x1044 and the block 5 far jump through
    // `RealModeCpu`, and then block 6 up to a HLT that replaces the first RDMSR (0x203e, image f:
    // 0x201f). The model stops at that HLT with `Unsupported`, the oracle's RIP is one byte past
    // it. Compared: GPRs, the six segments (selector, base, limit, type, DPL, P, D/B, L, G), EIP,
    // EFLAGS, CR0, CR3, CR4, GDTR, IDTR, EFER (the model has none: the oracle's must be 0) and the
    // captured GDT block (A bits).

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

    fn oracle_signature(json: &str) -> u32 {
        let at = json.find("\"function\":1,").expect("leaf 1");
        oracle_num(&json[at..], &["eax"]) as u32
    }

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

    /// Runs the S6-14j prefix of an oracle image through `RealModeCpu` and returns the protected
    /// state at 0x2000 with the memory.
    fn enter_from_oracle_image(image: &[u8], json: &str) -> (ProtectedCpu, Vec<u8>) {
        enter_from_oracle_image_sized(image, json, RAM)
    }

    fn enter_from_oracle_image_sized(
        image: &[u8],
        json: &str,
        size: usize,
    ) -> (ProtectedCpu, Vec<u8>) {
        let mut real = RealModeCpu::at_cs_ip(0, 0, 0x6000_0010, oracle_signature(json));
        let mut ram = vec![0u8; size];
        ram[..image.len()].copy_from_slice(image);
        let run = real.run(&mut mem(&mut ram), 100);
        assert_eq!(run.steps, 9, "block 4 and the block 5 jump, as in S6-14j");
        assert_eq!(
            run.stop,
            Some(RealModeRefusal::NotRealMode { cr0: real.mode.cr0 })
        );
        let cpu = ProtectedCpu::from_real_entry(&real).expect("protected entry");
        (cpu, ram)
    }

    fn compare_with_oracle_k(image: &[u8], json: &str, expected_steps: u64, hlt_at: u32) {
        assert!(json.contains("\"stop_reason\":\"guest_hlt\""));
        assert!(json.contains("\"exits\":[{\"kind\":\"hlt\"}]"));
        let (mut cpu, mut ram) = enter_from_oracle_image(image, json);
        let run = cpu.run(&mut mem(&mut ram), 100);
        assert_eq!(run.steps, expected_steps);
        match run.stop {
            Some(ProtectedRefusal::Unsupported { eip, bytes, .. }) => {
                assert_eq!(eip, hlt_at);
                assert_eq!(bytes[0], 0xF4, "stops at the HLT that stands for the RDMSR");
            }
            other => panic!("expected a stop at the HLT, got {other:?}"),
        }
        compare_state(&cpu, &ram, json, 1, 0);
    }

    /// Compares the model state with `final_state` of the oracle JSON: GPRs, RIP (`rip_delta` past
    /// EIP: 1 after a HLT, 0 for a faulting instruction), EFLAGS (the bits of `rflags_ignore` are
    /// cleared in the oracle value first), CR0/CR3/CR4/EFER, the six segments, GDTR/IDTR and the
    /// captured memory block.
    fn compare_state(
        cpu: &ProtectedCpu,
        ram: &[u8],
        json: &str,
        rip_delta: u64,
        rflags_ignore: u64,
    ) {
        compare_state_cr2(cpu, ram, json, rip_delta, rflags_ignore, 0);
    }

    /// As [`compare_state`], with the expected `CR2` (the model keeps none: the linear address of
    /// its [`ProtectedRefusal::PageFault`], or 0).
    fn compare_state_cr2(
        cpu: &ProtectedCpu,
        ram: &[u8],
        json: &str,
        rip_delta: u64,
        rflags_ignore: u64,
        cr2: u64,
    ) {
        let fs = |path: &[&str]| {
            let mut full = vec!["final_state"];
            full.extend_from_slice(path);
            oracle_num(json, &full)
        };
        let gprs = [
            ("rax", EAX),
            ("rcx", ECX),
            ("rdx", EDX),
            ("rbx", EBX),
            ("rsp", ESP),
            ("rbp", EBP),
            ("rsi", ESI),
            ("rdi", EDI),
        ];
        for (name, index) in gprs {
            assert_eq!(u64::from(cpu.gpr[index]), fs(&["gpr", name]), "{name}");
        }
        assert_eq!(cpu.rip() + rip_delta, fs(&["rip"]), "RIP");
        assert_eq!(
            u64::from(cpu.eflags),
            fs(&["rflags"]) & !rflags_ignore,
            "EFLAGS"
        );
        for (name, value) in [("cr0", cpu.cr0), ("cr3", cpu.cr3), ("cr4", cpu.cr4)] {
            assert_eq!(u64::from(value), fs(&["control", name]), "{name}");
        }
        assert_eq!(cpu.efer, fs(&["control", "efer"]), "EFER");
        assert_eq!(fs(&["control", "cr2"]), cr2, "CR2");
        for (name, seg) in [
            ("cs", cpu.cs),
            ("ds", cpu.ds),
            ("es", cpu.es),
            ("fs", cpu.fs),
            ("gs", cpu.gs),
            ("ss", cpu.ss),
        ] {
            let get = |field: &str| fs(&["segments", name, field]);
            assert_eq!(u64::from(seg.selector), get("selector"), "{name} selector");
            assert_eq!(u64::from(seg.base), get("base"), "{name} base");
            assert_eq!(u64::from(seg.limit), get("limit"), "{name} limit");
            assert_eq!(u64::from(seg.access & 0x0F), get("type"), "{name} type");
            assert_eq!(u64::from((seg.access >> 5) & 3), get("dpl"), "{name} DPL");
            assert_eq!(u64::from(seg.access >> 7), get("present"), "{name} P");
            assert_eq!(u64::from((seg.flags >> 2) & 1), get("db"), "{name} D/B");
            assert_eq!(u64::from((seg.flags >> 1) & 1), get("l"), "{name} L");
            assert_eq!(u64::from(seg.flags >> 3), get("g"), "{name} G");
            assert_eq!(get("unusable"), 0, "{name} usable");
        }
        for (name, value) in [
            ("gdtr_base", u64::from(cpu.gdtr.base)),
            ("gdtr_limit", u64::from(cpu.gdtr.limit)),
            ("idtr_base", u64::from(cpu.idtr.base)),
            ("idtr_limit", u64::from(cpu.idtr.limit)),
        ] {
            assert_eq!(value, fs(&["tables", name]), "{name}");
        }
        let (gpa, oracle_bytes) = oracle_result_block(json);
        assert_eq!(oracle_bytes.len(), 64);
        let start = gpa as usize;
        assert_eq!(
            &ram[start..start + 64],
            &oracle_bytes[..],
            "memory at {gpa:#x}"
        );
    }

    macro_rules! oracle_case_k {
        ($name:ident, $stem:literal, $steps:expr, $hlt_at:expr) => {
            #[test]
            fn $name() {
                let image: &[u8] = include_bytes!(concat!(
                    "../../../docs/reports/data/stage6/s6-14k/",
                    $stem,
                    ".bin"
                ));
                let json: &str = include_str!(concat!(
                    "../../../docs/reports/data/stage6/s6-14k/oracle-",
                    $stem,
                    ".json"
                ));
                compare_with_oracle_k(image, json, $steps, $hlt_at);
            }
        };
    }
    // a: selector 0x18 (flat data, A set), CR4 = 0x6b0: 13 steps up to the RDMSR at 0x203e.
    oracle_case_k!(
        oracle_k_a_flat_data_accessed_set_cr4_06b0,
        "a-flat-data-accessed-set-cr4-06b0-0000-0000",
        13,
        0x203E
    );
    // b: selector 0x10 (A clear): the GDT in memory gets the A bit; CR4 = 0x20; flag word
    // 0xfffffffe: only bit 0 selects the SME branch.
    oracle_case_k!(
        oracle_k_b_flat_data_accessed_clear,
        "b-flat-data-accessed-clear-flags-fffffffe-0000-0000",
        13,
        0x203E
    );
    // c: data base 0x1000, G/D/AVL: the header is read at DS.base + 0x5010 (decoy at 0x5010).
    oracle_case_k!(
        oracle_k_c_ds_base_1000_header_through_ds,
        "c-base-1000-avl-g-d-header-at-ds-base-0000-0000",
        13,
        0x203E
    );
    // d: byte-granular data limit 0x5fff; CR4 = 0x7ff.
    oracle_case_k!(
        oracle_k_d_byte_limit_5fff,
        "d-byte-limit-5fff-0000-0000",
        13,
        0x203E
    );
    // e: D/B = 0, G = 1 data segments (16-bit stack-size SS); CR4 = 0x7ff.
    oracle_case_k!(
        oracle_k_e_db0_g1_data,
        "e-data-db0-g1-cr4-07ff-0000-0000",
        13,
        0x203E
    );
    // f: flag word bit 0 set: jae not taken, mov ecx, stop at the SME RDMSR (0x201f): 9 steps.
    oracle_case_k!(
        oracle_k_f_sme_flag_stops_at_the_optional_rdmsr,
        "f-sme-flag-stop-at-rdmsr-201f-0000-0000",
        9,
        0x201F
    );

    // ---- comparison with the KVM oracle (S6-14l) ------------------------------------------
    //
    // Files in docs/reports/data/stage6/s6-14l/ (make-images.py, run-oracle.sh). Every image runs
    // the S6-14j prefix through `RealModeCpu` and the linked block 6 bytes 0x2000..0x2062 through
    // `ProtectedCpu`; the `MOV CR0, EAX` at 0x2062 is replaced by a HLT or by an EFER read-back
    // (see make-images.py). Two outcomes: the model stops at the HLT (`Unsupported`, the oracle's
    // RIP is one past it) or the WRMSR of EFER is refused (`EferWriteReservedBits`; the oracle
    // takes `#GP` with an empty IDT, a shutdown with RIP at the WRMSR and RF set in RFLAGS, which
    // is the only bit ignored in the comparison). Compared: GPRs, EIP, EFLAGS, CR0, CR3, CR4,
    // EFER, the six segments, GDTR, IDTR and the captured GDT block.

    #[derive(Clone, Copy)]
    enum OracleOutcomeL {
        /// The run stops at the HLT at this address.
        Halt(u32),
        /// The WRMSR at 0x205b is refused for this `EDX:EAX`.
        WrmsrRefused(u64),
    }

    fn compare_with_oracle_l(
        image: &[u8],
        json: &str,
        expected_steps: u64,
        outcome: OracleOutcomeL,
    ) {
        let (mut cpu, mut ram) = enter_from_oracle_image(image, json);
        let run = cpu.run(&mut mem(&mut ram), 100);
        assert_eq!(run.steps, expected_steps);
        match outcome {
            OracleOutcomeL::Halt(hlt_at) => {
                assert!(json.contains("\"stop_reason\":\"guest_hlt\""));
                assert!(json.contains("\"exits\":[{\"kind\":\"hlt\"}]"));
                match run.stop {
                    Some(ProtectedRefusal::Unsupported { eip, bytes, .. }) => {
                        assert_eq!(eip, hlt_at);
                        assert_eq!(bytes[0], 0xF4, "stops at the HLT");
                    }
                    other => panic!("expected a stop at the HLT, got {other:?}"),
                }
                compare_state(&cpu, &ram, json, 1, 0);
            }
            OracleOutcomeL::WrmsrRefused(value) => {
                assert!(json.contains("\"stop_reason\":\"triple_fault_or_shutdown\""));
                assert!(json.contains("\"exits\":[{\"kind\":\"shutdown\"}]"));
                assert_eq!(
                    run.stop,
                    Some(ProtectedRefusal::EferWriteReservedBits { eip: 0x205B, value })
                );
                compare_state(&cpu, &ram, json, 0, 0x1_0000);
            }
        }
    }

    macro_rules! oracle_case_l {
        ($name:ident, $stem:literal, $steps:expr, $outcome:expr) => {
            #[test]
            fn $name() {
                let image: &[u8] = include_bytes!(concat!(
                    "../../../docs/reports/data/stage6/s6-14l/",
                    $stem,
                    ".bin"
                ));
                let json: &str = include_str!(concat!(
                    "../../../docs/reports/data/stage6/s6-14l/oracle-",
                    $stem,
                    ".json"
                ));
                compare_with_oracle_l(image, json, $steps, $outcome);
            }
        };
    }
    use OracleOutcomeL::{Halt, WrmsrRefused};
    // a: header EFER 0x901 (SCE|LME|NXE): rdmsr 0, cmp/jne taken, loads, wrmsr, read-back equal.
    oracle_case_l!(
        oracle_l_a_efer_901_written_and_read_back,
        "a-efer-901-written-and-read-back-0000-0000",
        25,
        Halt(0x2075)
    );
    // b: header EFER 0: both compares equal, JE skips the WRMSR, HLT in place of MOV CR0.
    oracle_case_l!(
        oracle_l_b_equal_efer_skips_wrmsr,
        "b-efer-0-equal-skips-wrmsr-0000-0000",
        19,
        Halt(0x2062)
    );
    oracle_case_l!(
        oracle_l_c_lme_only,
        "c-efer-lme-only-0000-0000",
        25,
        Halt(0x2075)
    );
    // d: low dword equal, high dword 1: JNE not taken, JE not taken, WRMSR of 1:0 refused.
    oracle_case_l!(
        oracle_l_d_high_dword_bit_32_refused,
        "d-efer-high-dword-bit32-reserved-0000-0000",
        20,
        WrmsrRefused(1 << 32)
    );
    // e: 0x401 (SCE|LMA): LMA dropped, read-back 1 differs from the header: HLT at 0x2074.
    oracle_case_l!(
        oracle_l_e_lma_dropped_with_sce,
        "e-efer-sce-lma-written-0000-0000",
        23,
        Halt(0x2074)
    );
    oracle_case_l!(
        oracle_l_f_bit_1_refused,
        "f-efer-bit1-reserved-0000-0000",
        18,
        WrmsrRefused(0x2)
    );
    oracle_case_l!(
        oracle_l_g_bit_12_refused,
        "g-efer-bit12-svme-0000-0000",
        18,
        WrmsrRefused(0x1000)
    );
    oracle_case_l!(
        oracle_l_h_nxe_only,
        "h-efer-nxe-only-0000-0000",
        25,
        Halt(0x2075)
    );
    oracle_case_l!(
        oracle_l_i_bit_9_refused,
        "i-efer-bit9-reserved-0000-0000",
        18,
        WrmsrRefused(0x200)
    );
    // j: 0x400 (LMA only): dropped, EFER stays 0.
    oracle_case_l!(
        oracle_l_j_lma_only_dropped,
        "j-efer-lma-only-0000-0000",
        23,
        Halt(0x2074)
    );
    oracle_case_l!(
        oracle_l_k_lma_dropped_from_d01,
        "k-efer-d01-lma-dropped-0000-0000",
        23,
        Halt(0x2074)
    );

    // ---- paging and the compatibility-mode jump (S6-14m) ----------------------------------

    const RAM_PAGED: usize = 0xA100;
    const PML4: usize = 0x4000;
    const PDPT: usize = 0x7000;
    const PD: usize = 0x8000;
    const PT: usize = 0x9000;
    const CODE64_A_SET: [u8; 8] = [0xFF, 0xFF, 0, 0, 0, 0x9B, 0xAF, 0];
    const CODE64_A_CLEAR: [u8; 8] = [0xFF, 0xFF, 0, 0, 0, 0x9A, 0xAF, 0];
    const CODE32_COMPAT: [u8; 8] = [0xFF, 0xFF, 0, 0, 0, 0x9B, 0xCF, 0];
    const JMP_FAR_10_20F0: [u8; 7] = [0xEA, 0xF0, 0x20, 0, 0, 0x10, 0];

    fn put_q(ram: &mut [u8], at: usize, value: u64) {
        put(ram, at, &value.to_le_bytes());
    }

    fn get_q(ram: &[u8], at: usize) -> u64 {
        u64::from_le_bytes(ram[at..at + 8].try_into().unwrap())
    }

    /// PML4[0] -> PDPT, PDPT[0] -> PD, PD[0] = 2 MiB page at 0; A and D clear.
    fn tables_2m(ram: &mut [u8]) {
        put_q(ram, PML4, PDPT as u64 | 3);
        put_q(ram, PDPT, PD as u64 | 3);
        put_q(ram, PD, 0x83);
    }

    /// As `tables_2m`, but PD[0] -> PT and PT[i] = page i (i < 10), A and D clear.
    fn tables_4k(ram: &mut [u8]) {
        tables_2m(ram);
        put_q(ram, PD, PT as u64 | 3);
        for page in 0..10u64 {
            put_q(ram, PT + 8 * page as usize, (page << 12) | 3);
        }
    }

    /// Sets A and D in every entry of the tables made by `tables_2m`/`tables_4k` (no walk then
    /// writes memory).
    fn preset_accessed_dirty(ram: &mut [u8], pt: bool) {
        for at in [PML4, PDPT, PD] {
            let e = get_q(ram, at);
            put_q(ram, at, e | 0x60);
        }
        if pt {
            for page in 0..10usize {
                let e = get_q(ram, PT + 8 * page);
                put_q(ram, PT + 8 * page, e | 0x60);
            }
        }
    }

    /// The RAM of the S6-14m images: GDT at 0x38 (selector 0x10 = `desc`, A-set data at 0x18), the
    /// `mov cr0, eax; jmp 0x10:0x20f0` at 0x2062, a HLT at 0x20f0, 2 MiB identity tables.
    fn paged_ram(desc: [u8; 8]) -> Vec<u8> {
        let mut ram = vec![0u8; RAM_PAGED];
        put(&mut ram, 0x40, &FLAT_CODE);
        put(&mut ram, 0x48, &desc);
        put(&mut ram, 0x50, &FLAT_DATA_A_SET);
        put(&mut ram, 0x2062, &[0x0F, 0x22, 0xC0]);
        put(&mut ram, 0x2065, &JMP_FAR_10_20F0);
        ram[0x20F0] = 0xF4;
        tables_2m(&mut ram);
        ram
    }

    /// Ready to execute the `MOV CR0, EAX` at 0x2062 with the values of the linked path: EAX =
    /// 0x80050033, CR3 = 0x4000, CR4 = 0x6b0, EFER = SCE|LME|NXE.
    fn ia32e_cpu() -> ProtectedCpu {
        let mut cpu = entry_cpu();
        cpu.eip = 0x2062;
        cpu.gpr[EAX] = 0x8005_0033;
        cpu.cr3 = PML4 as u32;
        cpu.cr4 = 0x6B0;
        cpu.efer = 0x901;
        cpu
    }

    /// IA-32e mode already active (compatibility CS, paging on, LMA = 1) at 0x2000.
    fn active_cpu() -> ProtectedCpu {
        let mut cpu = ia32e_cpu();
        cpu.eip = 0x2000;
        cpu.cr0 = 0x8005_0033;
        cpu.efer = 0xD01;
        cpu
    }

    #[test]
    fn mov_cr0_with_pg_pae_and_lme_enters_ia32e_and_sets_lma() {
        let mut ram = paged_ram(CODE64_A_SET);
        let before = ram.clone();
        let mut cpu = ia32e_cpu();
        assert_eq!(
            cpu.step(&mut mem(&mut ram)),
            Ok(ProtectedInstruction::MovCr0Eax)
        );
        assert_eq!(cpu.cr0, 0x8005_0033);
        // LMA (bit 10) set by the processor; SCE, LME, NXE as written by WRMSR.
        assert_eq!(cpu.efer, 0xD01);
        assert_eq!(cpu.eip, 0x2065);
        assert_eq!(
            cpu.cs,
            flat(0x08, 0x9B),
            "CS is still the compatibility segment"
        );
        assert_eq!(
            ram, before,
            "the fetch before the write was physical: no walk, no touch"
        );
        // ET reads as 1 whatever the written value says.
        let mut cpu = ia32e_cpu();
        cpu.gpr[EAX] = 0x8005_0003;
        cpu.step(&mut mem(&mut ram)).unwrap();
        assert_eq!(cpu.cr0, 0x8005_0013);
    }

    #[test]
    fn mov_cr0_paging_refusals_change_nothing() {
        let value = 0x8005_0033u32;
        let refused = |setup: &dyn Fn(&mut ProtectedCpu), eax: u32| {
            let (mut cpu, mut ram) = (ia32e_cpu(), paged_ram(CODE64_A_SET));
            // A and D preset: the walks of the second-PG-write case write nothing.
            preset_accessed_dirty(&mut ram, false);
            cpu.eip = 0x2000;
            cpu.gpr[EAX] = eax;
            setup(&mut cpu);
            step_one(&mut cpu, &mut ram, &[0x0F, 0x22, 0xC0]).unwrap_err()
        };
        let eip = 0x2000;
        // PG with LME and no PAE: #GP(0).
        assert_eq!(
            refused(&|c| c.cr4 = 0x690, value),
            ProtectedRefusal::LongModePagingWithoutPae { eip, value }
        );
        // PG without LME and without PAE (EFER 0, or NXE only): 32-bit paging, not modelled.
        // (With PAE it is PAE paging, S2-25: `mov_cr0_with_pg_and_pae_without_lme_enters_pae_paging`.)
        for efer in [0, 0x800, 0x001] {
            assert_eq!(
                refused(
                    &|c| {
                        c.efer = efer;
                        c.cr4 = 0x690;
                    },
                    value
                ),
                ProtectedRefusal::PagingNotModelled { eip, value }
            );
        }
        // PG without PE: #GP(0).
        assert_eq!(
            refused(&|_| {}, 0x8005_0032),
            ProtectedRefusal::PagingWithoutProtection {
                eip,
                value: 0x8005_0032
            }
        );
        // IA-32e activation with a 64-bit CS (L = 1): #GP(0).
        assert_eq!(
            refused(&|c| c.cs.flags |= 2, value),
            ProtectedRefusal::LongModeActivationFromLongCs { eip, value }
        );
        // Reserved bits, NW without CD, a second PG write, a write without PG.
        for (setup, eax) in [
            (
                &(|_: &mut ProtectedCpu| {}) as &dyn Fn(&mut ProtectedCpu),
                0x8005_0073u32,
            ),
            (&|_| {}, 0x9005_0033),
            (&|_| {}, 0xA005_0033),
            (
                &|c| {
                    c.cr0 = 0x8005_0033;
                    c.efer |= EFER_LMA;
                },
                0x8005_0033,
            ),
            (&|_| {}, 0x0005_0033),
        ] {
            assert_eq!(
                refused(setup, eax),
                ProtectedRefusal::Cr0WriteUnmodelled { eip, value: eax },
                "{eax:#x}"
            );
        }
    }

    #[test]
    fn paging_translates_fetches_data_reads_and_descriptor_reads() {
        // 4 KiB pages: linear page 2 (code) -> physical 0x3000, linear page 5 -> physical 0x6000.
        let mut ram = paged_ram(CODE32_COMPAT);
        tables_4k(&mut ram);
        put_q(&mut ram, PT + 16, 0x3000 | 3);
        put_q(&mut ram, PT + 40, 0x6000 | 3);
        ram[0x2065..0x206C].fill(0xF4); // the identity bytes are not what runs
        put(&mut ram, 0x3065, &JMP_FAR_10_20F0);
        put(&mut ram, 0x30F0, &[0xA1, 0x08, 0x50, 0, 0]); // mov eax, ds:0x5008
        put(&mut ram, 0x6008, &0x1234_5678u32.to_le_bytes());
        put(&mut ram, 0x5008, &0xFFFF_FFFFu32.to_le_bytes());
        let mut cpu = ia32e_cpu();
        let m = &mut mem(&mut ram);
        cpu.step(m).unwrap();
        assert_eq!(
            cpu.step(m),
            Ok(ProtectedInstruction::JmpFarPtr {
                selector: 0x10,
                offset: 0x20F0
            })
        );
        assert_eq!(
            cpu.cs,
            ProtectedSegment {
                selector: 0x10,
                base: 0,
                limit: 0xFFFF_FFFF,
                access: 0x9B,
                flags: 0xC
            }
        );
        assert_eq!(cpu.step(m), Ok(ProtectedInstruction::MovEaxFromDs(0x5008)));
        assert_eq!((cpu.gpr[EAX], cpu.eip), (0x1234_5678, 0x20F5));
        // The walks set A: PML4E, PDPTE, PDE, the PTEs of page 2 (fetch), page 0 (GDT), page 5 (data).
        assert_eq!(get_q(&ram, PML4), PDPT as u64 | 0x23);
        assert_eq!(get_q(&ram, PDPT), PD as u64 | 0x23);
        assert_eq!(get_q(&ram, PD), PT as u64 | 0x23);
        assert_eq!(get_q(&ram, PT), 0x23);
        assert_eq!(get_q(&ram, PT + 16), 0x3000 | 0x23);
        assert_eq!(get_q(&ram, PT + 40), 0x6000 | 0x23);
        assert_eq!(get_q(&ram, PT + 8), 0x1003, "page 1 was not used");
    }

    // ---- S2-25: PAE paging, CR4.PAE and CR3/PDPTE rules (model only: no hardware oracle) -------

    /// PDPT[0] -> PD (`P` only), PD[0] -> PT, PT[i] = identity of page i (i < 10), A and D clear;
    /// the PDPT is the 32-byte table at `PDPT`.
    fn pae_tables_4k(ram: &mut [u8]) {
        put_q(ram, PDPT, PD as u64 | 1);
        put_q(ram, PD, PT as u64 | 3);
        for page in 0..10u64 {
            put_q(ram, PT + 8 * page as usize, (page << 12) | 3);
        }
    }

    /// The paged RAM of the S6-14m images with PAE tables instead of the 4-level ones.
    fn pae_ram() -> Vec<u8> {
        let mut ram = paged_ram(CODE32_COMPAT);
        pae_tables_4k(&mut ram);
        ram
    }

    /// As `pae_ram` with A and D preset in the PDE and the PTEs: a walk then writes nothing, so a
    /// refused instruction leaves the memory as it was (the fetch walk completed before it).
    fn pae_ram_ad() -> Vec<u8> {
        let mut ram = pae_ram();
        put_q(&mut ram, PD, PT as u64 | 0x63);
        for page in 0..10usize {
            let e = get_q(&ram, PT + 8 * page);
            put_q(&mut ram, PT + 8 * page, e | 0x60);
        }
        ram
    }

    /// Ready to execute the `MOV CR0, EAX` at 0x2062 that enables PAE paging: PAE set, EFER = NXE
    /// (no LME), CR3 = the PDPT.
    fn pae_cpu() -> ProtectedCpu {
        let mut cpu = ia32e_cpu();
        cpu.cr3 = PDPT as u32;
        cpu.efer = EFER_NXE;
        cpu
    }

    /// PAE paging already active at 0x2000 (the PDPTEs as `pae_ram` gives them).
    fn pae_active_cpu() -> ProtectedCpu {
        let mut cpu = pae_cpu();
        cpu.eip = 0x2000;
        cpu.cr0 = 0x8005_0033;
        cpu.pdptes = [PD as u64 | 1, 0, 0, 0];
        cpu
    }

    #[test]
    fn mov_cr0_with_pg_and_pae_without_lme_enters_pae_paging() {
        let mut ram = pae_ram();
        // Page 5 -> physical 0x6000 for the data read at the end.
        put_q(&mut ram, PT + 40, 0x6000 | 3);
        put(&mut ram, 0x6008, &0x1234_5678u32.to_le_bytes());
        put(&mut ram, 0x20F0, &[0xA1, 0x08, 0x50, 0, 0]); // mov eax, ds:0x5008
        let before = ram.clone();
        let mut cpu = pae_cpu();
        let m = &mut mem(&mut ram);
        assert_eq!(cpu.step(m), Ok(ProtectedInstruction::MovCr0Eax));
        assert_eq!(cpu.cr0, 0x8005_0033);
        // PAE paging is not IA-32e mode: LMA stays clear, the PDPTE registers are loaded from CR3.
        assert_eq!(cpu.efer, EFER_NXE);
        assert!(cpu.pae_paging());
        assert_eq!(cpu.pdptes, [PD as u64 | 1, 0, 0, 0]);
        assert_eq!(cpu.eip, 0x2065);
        assert!(!cpu.is_64bit());
        assert_eq!(
            ram, before,
            "the fetch before the write was physical: no walk, no touch"
        );
        let m = &mut mem(&mut ram);
        // From here every access walks PDPTE -> PDE -> PTE.
        assert_eq!(
            cpu.step(m),
            Ok(ProtectedInstruction::JmpFarPtr {
                selector: 0x10,
                offset: 0x20F0
            })
        );
        assert_eq!(cpu.step(m), Ok(ProtectedInstruction::MovEaxFromDs(0x5008)));
        assert_eq!((cpu.gpr[EAX], cpu.eip), (0x1234_5678, 0x20F5));
        // A on the PDE and the PTEs of page 2 (fetch), page 0 (GDT) and page 5 (data); the PDPTE
        // has no accessed bit and is never written.
        assert_eq!(get_q(&ram, PDPT), PD as u64 | 1);
        assert_eq!(get_q(&ram, PD), PT as u64 | 0x23);
        assert_eq!(get_q(&ram, PT), 0x23);
        assert_eq!(get_q(&ram, PT + 16), 0x2000 | 0x23);
        assert_eq!(get_q(&ram, PT + 40), 0x6000 | 0x23);
        assert_eq!(get_q(&ram, PT + 8), 0x1003, "page 1 was not used");
    }

    #[test]
    fn pae_paging_two_mib_page_sets_a_in_the_pde_and_d_for_the_descriptor_write() {
        // PD[0] = 2 MiB page at 0 (tables_2m); the far jump loads an A-clear descriptor, so the
        // model writes its access byte through the walk: D on the PDE, none on the PDPTE.
        let mut ram = paged_ram([0xFF, 0xFF, 0, 0, 0, 0x9A, 0xCF, 0]);
        put_q(&mut ram, PDPT, PD as u64 | 1);
        let mut cpu = pae_cpu();
        let m = &mut mem(&mut ram);
        cpu.step(m).unwrap();
        cpu.step(m).unwrap();
        assert_eq!(get_q(&ram, PD), 0xE3, "A and D on the 2 MiB PDE");
        assert_eq!(get_q(&ram, PDPT), PD as u64 | 1);
        assert_eq!(ram[0x4D], 0x9B, "the accessed bit of the descriptor");
        assert_eq!(cpu.cs.selector, 0x10);
    }

    #[test]
    fn pae_paging_enable_refusals_change_nothing() {
        let eip = 0x2062;
        let value = 0x8005_0033u32;
        let run = |setup: &dyn Fn(&mut ProtectedCpu, &mut Vec<u8>)| {
            let (mut cpu, mut ram) = (pae_cpu(), pae_ram());
            setup(&mut cpu, &mut ram);
            step_one_at(&mut cpu, &mut ram)
        };
        // A PDPTE with P = 1 and a reserved bit: #GP(0), CR0 and the PDPTEs unchanged, LMA clear.
        for (index, bad) in [
            (0usize, PD as u64 | 3),
            (1, PD as u64 | 1 | 1 << 5),
            (2, PD as u64 | 1 | 1 << 36),
            (3, PD as u64 | 1 | 1 << 63),
        ] {
            assert_eq!(
                run(&|_, ram| put_q(ram, PDPT + 8 * index, bad)),
                Err(ProtectedRefusal::PdpteReservedBits {
                    eip,
                    index: index as u8,
                    pdpte: bad
                })
            );
        }
        // Reserved bits with P = 0 are not checked.
        assert_eq!(
            run(&|_, ram| put_q(ram, PDPT + 8, 0xFFFF_FFFF_FFFF_FFFE)),
            Ok(ProtectedInstruction::MovCr0Eax)
        );
        // The PDPT must be readable (a model limit).
        assert_eq!(
            run(&|cpu, _| cpu.cr3 = RAM_PAGED as u32),
            Err(ProtectedRefusal::PdpteUnreadable {
                eip,
                phys: RAM_PAGED as u32
            })
        );
        // PG without PE is #GP(0) before any PDPTE is read.
        assert_eq!(
            run(&|cpu, ram| {
                put_q(ram, PDPT, PD as u64 | 3);
                cpu.gpr[EAX] = value & !1;
            }),
            Err(ProtectedRefusal::PagingWithoutProtection {
                eip,
                value: value & !1
            })
        );
        // PG with PAE clear and LME clear is 32-bit paging: still not modelled.
        assert_eq!(
            run(&|cpu, _| cpu.cr4 = 0x690),
            Err(ProtectedRefusal::PagingNotModelled { eip, value })
        );
    }

    /// Runs the `MOV CR0, EAX` already placed at 0x2062 (the S6-14m images) from `cpu`.
    fn step_one_at(
        cpu: &mut ProtectedCpu,
        ram: &mut Vec<u8>,
    ) -> Result<ProtectedInstruction, ProtectedRefusal> {
        let (before, ram_before) = (*cpu, ram.clone());
        let result = cpu.step(&mut mem(ram));
        if result.is_err() {
            assert_eq!(*cpu, before, "state unchanged by a refusal");
            assert_eq!(*ram, ram_before, "memory unchanged by a refusal");
        }
        result
    }

    #[test]
    fn lme_with_pg_and_without_pae_is_gp_and_pae_first_then_lme_enters_ia32e() {
        // SDM Vol. 3A §12.8.5: IA-32e enabled and paging enabled before PAE is #GP(0).
        let (mut cpu, mut ram) = (ia32e_cpu(), pae_ram());
        cpu.cr4 = 0x690;
        let value = 0x8005_0033;
        assert_eq!(
            step_one_at(&mut cpu, &mut ram),
            Err(ProtectedRefusal::LongModePagingWithoutPae { eip: 0x2062, value })
        );
        assert_eq!(
            (cpu.cr0, cpu.efer, cpu.pdptes),
            (0x0005_0033, 0x901, [0; 4])
        );
        // PAE set (CR4 written first), LME written after: the same MOV CR0 now enters IA-32e mode
        // (4-level walk from CR3 = PML4), LMA set, and no PDPTE register is loaded.
        let (mut cpu, mut ram) = (ia32e_cpu(), paged_ram(CODE32_COMPAT));
        assert_eq!(
            step_one_at(&mut cpu, &mut ram),
            Ok(ProtectedInstruction::MovCr0Eax)
        );
        assert_eq!(cpu.efer & EFER_LMA, EFER_LMA);
        assert!(!cpu.pae_paging());
        assert_eq!(cpu.pdptes, [0; 4]);
    }

    #[test]
    fn pae_set_without_paging_loads_no_pdptes_and_cr3_takes_any_value() {
        // PG = 0: MOV CR4 with PAE and MOV CR3 only store; memory at CR3 is not looked at.
        let (mut cpu, mut ram) = (entry_cpu(), new_ram());
        cpu.gpr[EAX] = 0x20;
        step_one(&mut cpu, &mut ram, &[0x0F, 0x22, 0xE0]).unwrap();
        cpu.eip = 0x2000;
        cpu.gpr[EAX] = 0xFFFF_FFFF;
        step_one(&mut cpu, &mut ram, &[0x0F, 0x22, 0xD8]).unwrap();
        assert_eq!((cpu.cr4, cpu.cr3, cpu.pdptes), (0x20, 0xFFFF_FFFF, [0; 4]));
    }

    #[test]
    fn mov_cr3_in_pae_paging_loads_the_pdptes_from_the_new_address() {
        let mut ram = pae_ram();
        // A second table at 0xA020 (bits 4:0 of the CR3 value are ignored): PDPTE0 not present,
        // PDPTE1 present.
        put_q(&mut ram, 0xA028, PD as u64 | 1);
        put(
            &mut ram,
            0x2000,
            &[0xB8, 0x3F, 0xA0, 0, 0, 0x0F, 0x22, 0xD8, 0x90],
        );
        let mut cpu = pae_active_cpu();
        let m = &mut mem(&mut ram);
        cpu.step(m).unwrap();
        assert_eq!(cpu.step(m), Ok(ProtectedInstruction::MovCr3Eax));
        assert_eq!(cpu.cr3, 0xA03F);
        assert_eq!(cpu.pdptes, [0, PD as u64 | 1, 0, 0]);
        // No TLB: the next fetch walks the new PDPTE0 at once.
        assert_eq!(
            cpu.step(m),
            Err(ProtectedRefusal::PageFault {
                eip: 0x2008,
                linear: 0x2008,
                access: AccessKind::Fetch,
                reason: PageRefusal::NotPresent { level: 3, entry: 0 }
            })
        );
    }

    #[test]
    fn mov_cr3_and_mov_cr4_with_a_reserved_pdpte_are_gp_and_change_nothing() {
        for (index, bad) in [
            (0usize, PD as u64 | 3),
            (1, PD as u64 | 1 | 1 << 8),
            (2, PD as u64 | 1 | 1 << 36),
            (3, PD as u64 | 1 | 1 << 63),
        ] {
            let expected = |eip| {
                Err(ProtectedRefusal::PdpteReservedBits {
                    eip,
                    index: index as u8,
                    pdpte: bad,
                })
            };
            // MOV CR3 to a table with the bad PDPTE: CR3 and the registers keep their value.
            let (mut cpu, mut ram) = (pae_active_cpu(), pae_ram_ad());
            put_q(&mut ram, 0xA020 + 8 * index, bad);
            cpu.gpr[EAX] = 0xA020;
            assert_eq!(
                step_one(&mut cpu, &mut ram, &[0x0F, 0x22, 0xD8]),
                expected(0x2000)
            );
            // MOV CR4 that changes PGE reloads the PDPTEs from the (now bad) table at CR3.
            let (mut cpu, mut ram) = (pae_active_cpu(), pae_ram_ad());
            put_q(&mut ram, 0xA020 + 8 * index, bad);
            cpu.cr3 = 0xA020;
            cpu.cr4 = 0x6B0;
            cpu.gpr[EAX] = 0x630; // PGE cleared
            assert_eq!(
                step_one(&mut cpu, &mut ram, &[0x0F, 0x22, 0xE0]),
                expected(0x2000)
            );
        }
    }

    #[test]
    fn mov_cr4_in_pae_paging_reloads_the_pdptes_only_for_pae_pge_and_pse() {
        let (mut cpu, mut ram) = (pae_active_cpu(), pae_ram());
        cpu.cr4 = 0x6B0;
        // The table in memory changes under the registers (no TLB, but the PDPTEs are registers).
        put_q(&mut ram, PDPT, PD as u64 | 0x19); // PWT, PCD: allowed bits
        let stale = cpu.pdptes;
        // OSFXSR toggled: no reload.
        cpu.gpr[EAX] = 0x4B0;
        step_one(&mut cpu, &mut ram, &[0x0F, 0x22, 0xE0]).unwrap();
        assert_eq!((cpu.cr4, cpu.pdptes), (0x4B0, stale));
        // PGE, then PSE toggled: reload each time.
        for (value, pdpte0) in [(0x430u32, PD as u64 | 0x19), (0x420, PD as u64 | 0x19)] {
            cpu.eip = 0x2000;
            cpu.pdptes = stale;
            cpu.gpr[EAX] = value;
            step_one(&mut cpu, &mut ram, &[0x0F, 0x22, 0xE0]).unwrap();
            assert_eq!((cpu.cr4, cpu.pdptes[0]), (value, pdpte0), "{value:#x}");
        }
        // Same value written again: nothing changes, no reload.
        cpu.eip = 0x2000;
        cpu.pdptes = stale;
        cpu.gpr[EAX] = 0x420;
        step_one(&mut cpu, &mut ram, &[0x0F, 0x22, 0xE0]).unwrap();
        assert_eq!(cpu.pdptes, stale);
    }

    #[test]
    fn mov_cr4_clearing_pae_while_paging_is_gp_in_ia32e_and_unmodelled_in_pae_paging() {
        // PAE paging (LMA = 0): leaving PAE would select 32-bit paging, which is not modelled.
        let (mut cpu, mut ram) = (pae_active_cpu(), pae_ram_ad());
        cpu.cr4 = 0x6B0;
        cpu.gpr[EAX] = 0x690;
        assert_eq!(
            step_one(&mut cpu, &mut ram, &[0x0F, 0x22, 0xE0]),
            Err(ProtectedRefusal::Cr4WriteUnmodelled {
                eip: 0x2000,
                value: 0x690
            })
        );
        // IA-32e mode active (LMA = 1): #GP(0), whatever else the value holds.
        for value in [0u32, 0x690, 0x7C0] {
            let (mut cpu, mut ram) = (active_cpu(), paged_ram(CODE32_COMPAT));
            preset_accessed_dirty(&mut ram, false);
            cpu.gpr[EAX] = value;
            assert_eq!(
                step_one(&mut cpu, &mut ram, &[0x0F, 0x22, 0xE0]),
                Err(ProtectedRefusal::Cr4PaeClearInLongMode { eip: 0x2000, value })
            );
        }
        // With paging off PAE may be cleared and set freely.
        let (mut cpu, mut ram) = (entry_cpu(), new_ram());
        cpu.cr4 = 0x20;
        cpu.gpr[EAX] = 0;
        step_one(&mut cpu, &mut ram, &[0x0F, 0x22, 0xE0]).unwrap();
        assert_eq!(cpu.cr4, 0);
    }

    #[test]
    fn efer_lme_cannot_change_in_pae_paging_but_nxe_can() {
        let (mut cpu, mut ram) = (pae_active_cpu(), pae_ram_ad());
        cpu.gpr[ECX] = MSR_EFER;
        cpu.gpr[EDX] = 0;
        // LME 0 -> 1 with PG = 1 is #GP(0) (SDM Vol. 3A §12.8.5), as is PG = 1 with LME = 1 later.
        cpu.gpr[EAX] = (EFER_NXE | EFER_LME) as u32;
        assert_eq!(
            step_one(&mut cpu, &mut ram, &[0x0F, 0x30]),
            Err(ProtectedRefusal::EferLmeChangeWithPaging {
                eip: 0x2000,
                value: EFER_NXE | EFER_LME
            })
        );
        cpu.gpr[EAX] = 0;
        assert!(step_one(&mut cpu, &mut ram, &[0x0F, 0x30]).is_ok());
        assert_eq!(cpu.efer, 0, "NXE cleared, LMA still clear");
        assert!(cpu.pae_paging());
    }

    /// Memory that lets "another vCPU" change one entry right before the first replace of the
    /// accessed/dirty update of a step (S6-31b), and counts the replaces.
    struct Racy<'a> {
        inner: ImageMemoryMut<'a>,
        interfere: Option<(usize, u64)>,
        replaces: u32,
    }

    impl RealModeMemory for Racy<'_> {
        fn read_u8(&self, linear: u32) -> Option<u8> {
            self.inner.read_u8(linear)
        }
        fn can_write(&self, linear: u32) -> bool {
            self.inner.can_write(linear)
        }
        fn write_u8(&mut self, linear: u32, value: u8) -> bool {
            self.inner.write_u8(linear, value)
        }
        fn compare_replace_entry(
            &mut self,
            linear: u32,
            expected: u64,
            replacement: u64,
        ) -> Option<Result<(), u64>> {
            if let Some((at, value)) = self.interfere.take() {
                put_q(self.inner.bytes, at, value);
            }
            self.replaces += 1;
            self.inner
                .compare_replace_entry(linear, expected, replacement)
        }
    }

    /// S6-31b (b), (c) at the step: the update of the entry runs after the instruction retired and
    /// the instruction is never repeated. A change of the translation bits in between discards the
    /// touch (the stale translation was used; no A bit); a change of only ignored bits keeps them
    /// and sets A on the observed value.
    #[test]
    fn a_racing_change_of_an_entry_discards_or_absorbs_the_touch_and_never_repeats_the_step() {
        // (new PTE value, expected PTE value after the step, replaces of the step). The load of a
        // dword walks per byte: four touches of the same entry, each with the value it read. Case
        // 1: all four fail on the changed translation. Case 2: the first fails once and wins over
        // the observed value, the other three fail once and find A already set.
        let soft = 0x1A5u64 << 52 | 0b101 << 9;
        let cases = [
            (0x7000u64 | 3, 0x7000u64 | 3, 4u32),
            (0x6000 | 3 | soft, 0x6000 | 3 | soft | 0x20, 5),
        ];
        for (raced, after, replaces) in cases {
            let mut ram = paged_ram(CODE32_COMPAT);
            tables_4k(&mut ram);
            put_q(&mut ram, PT + 16, 0x3000 | 3);
            put_q(&mut ram, PT + 40, 0x6000 | 3);
            ram[0x2065..0x206C].fill(0xF4);
            put(&mut ram, 0x3065, &JMP_FAR_10_20F0);
            put(&mut ram, 0x30F0, &[0xA1, 0x08, 0x50, 0, 0]); // mov eax, ds:0x5008
            put(&mut ram, 0x6008, &0x1234_5678u32.to_le_bytes());
            put(&mut ram, 0x7008, &0x8765_4321u32.to_le_bytes());
            let mut cpu = ia32e_cpu();
            cpu.step(&mut mem(&mut ram)).unwrap();
            cpu.step(&mut mem(&mut ram)).unwrap();
            let mut racy = Racy {
                inner: mem(&mut ram),
                interfere: Some((PT + 40, raced)),
                replaces: 0,
            };
            assert_eq!(
                cpu.step(&mut racy),
                Ok(ProtectedInstruction::MovEaxFromDs(0x5008))
            );
            assert_eq!(racy.replaces, replaces, "{raced:#x}");
            // The data came through the translation the walk used, once; EIP moved once.
            assert_eq!((cpu.gpr[EAX], cpu.eip), (0x1234_5678, 0x20F5), "{raced:#x}");
            assert_eq!(get_q(&ram, PT + 40), after, "{raced:#x}");
        }
    }

    #[test]
    fn far_jump_to_a_64_bit_segment_loads_cs_and_the_model_stops_there() {
        // (descriptor, access byte after, PDE after): an A-clear descriptor is written through the
        // walk, so the 2 MiB PDE (the GDT is in it) gets D as well.
        for (desc, access_after, pde_after) in
            [(CODE64_A_SET, 0x9B, 0xA3u64), (CODE64_A_CLEAR, 0x9B, 0xE3)]
        {
            let mut ram = paged_ram(desc);
            let mut cpu = ia32e_cpu();
            cpu.step(&mut mem(&mut ram)).unwrap();
            assert_eq!(
                cpu.step(&mut mem(&mut ram)),
                Ok(ProtectedInstruction::JmpFarPtr {
                    selector: 0x10,
                    offset: 0x20F0
                })
            );
            // Base from the descriptor (0), limit after granularity, flags G=1 D=0 L=1.
            assert_eq!(
                cpu.cs,
                ProtectedSegment {
                    selector: 0x10,
                    base: 0,
                    limit: 0xFFFF_FFFF,
                    access: access_after,
                    flags: 0xA
                }
            );
            assert_eq!((cpu.eip, cpu.efer), (0x20F0, 0xD01));
            assert_eq!(ram[0x4D], access_after);
            assert_eq!(get_q(&ram, PML4), PDPT as u64 | 0x23);
            assert_eq!(get_q(&ram, PDPT), PD as u64 | 0x23);
            assert_eq!(get_q(&ram, PD), pde_after);
            // S6-14n: the 64-bit step now decodes; the HLT that stands at 0x20f0 in these RAMs is
            // outside the closed set of block 8 (the S6-14m expectation was `Mode64NotModelled`).
            let (before_cpu, before_ram) = (cpu, ram.clone());
            assert_eq!(
                cpu.step(&mut mem(&mut ram)),
                Err(ProtectedRefusal::Unsupported {
                    eip: 0x20F0,
                    linear: 0x20F0,
                    bytes: before_ram[0x20F0..0x20F0 + UNSUPPORTED_BYTES_MAX].to_vec()
                })
            );
            assert_eq!((cpu, ram), (before_cpu, before_ram));
        }
    }

    #[test]
    fn far_jump_to_a_compatibility_segment_keeps_decoding_through_paging() {
        let mut ram = paged_ram(CODE32_COMPAT);
        put(&mut ram, 0x20F0, &[0xB8, 1, 2, 3, 4]);
        let mut cpu = ia32e_cpu();
        let m = &mut mem(&mut ram);
        cpu.step(m).unwrap();
        cpu.step(m).unwrap();
        assert_eq!(cpu.cs.flags, 0xC);
        assert_eq!(
            cpu.step(m),
            Ok(ProtectedInstruction::MovEaxImm32(0x0403_0201))
        );
        assert_eq!(cpu.eip, 0x20F5);
    }

    #[test]
    fn far_jump_refusals_change_nothing() {
        let desc_data = FLAT_DATA_A_SET;
        let not_present = [0xFF, 0xFF, 0, 0, 0, 0x1B, 0xAF, 0];
        let dpl3 = [0xFF, 0xFF, 0, 0, 0, 0xFB, 0xAF, 0];
        let long_and_d = [0xFF, 0xFF, 0, 0, 0, 0x9B, 0xEF, 0];
        let short_limit = [0x20, 0, 0, 0, 0, 0x9B, 0x40, 0];
        let ldt = FarJumpReason::LdtSelector;
        // (descriptor at 0x10, selector, LMA, expected). Paging is off in this test: the checks
        // are the descriptor's, and a refusal then leaves memory alone.
        let cases: [([u8; 8], u16, bool, ProtectedRefusal); 10] = [
            (CODE64_A_SET, 0x14, true, far(0x14, ldt)),
            (
                CODE64_A_SET,
                0x03,
                true,
                far(0x03, FarJumpReason::NullSelector),
            ),
            (
                CODE64_A_SET,
                0x28,
                true,
                far(0x28, FarJumpReason::BeyondGdtLimit { limit: 0x1F }),
            ),
            (
                desc_data,
                0x10,
                true,
                far(0x10, FarJumpReason::NotCodeSegment { access: 0x93 }),
            ),
            (
                not_present,
                0x10,
                true,
                far(0x10, FarJumpReason::NotPresent),
            ),
            (
                dpl3,
                0x10,
                true,
                far(0x10, FarJumpReason::Privilege { rpl: 0, dpl: 3 }),
            ),
            (
                CODE64_A_SET,
                0x11,
                true,
                far(0x11, FarJumpReason::Privilege { rpl: 1, dpl: 0 }),
            ),
            (
                long_and_d,
                0x10,
                true,
                far(0x10, FarJumpReason::LongCodeWith32BitDefault),
            ),
            (
                CODE64_A_SET,
                0x10,
                false,
                far(0x10, FarJumpReason::LongCodeOutsideIa32e),
            ),
            (
                short_limit,
                0x10,
                true,
                ProtectedRefusal::TargetBeyondLimit {
                    eip: 0x2000,
                    offset: 0x20F0,
                    limit: 0x20,
                },
            ),
        ];
        for (desc, selector, lma, expected) in cases {
            let (mut cpu, mut ram) = (entry_cpu(), paged_ram(desc));
            cpu.efer = if lma { 0xD01 } else { 0x901 };
            let [lo, hi] = selector.to_le_bytes();
            let code = [0xEA, 0xF0, 0x20, 0, 0, lo, hi];
            assert_eq!(
                step_one(&mut cpu, &mut ram, &code),
                Err(expected),
                "selector {selector:#x}"
            );
        }
        // The descriptor table is not readable.
        let (mut cpu, mut ram) = (entry_cpu(), paged_ram(CODE64_A_SET));
        cpu.efer = 0xD01;
        cpu.gdtr.base = 0xF000_0000;
        assert_eq!(
            step_one(&mut cpu, &mut ram, &JMP_FAR_10_20F0),
            Err(far(
                0x10,
                FarJumpReason::DescriptorUnreadable {
                    linear: 0xF000_0010
                }
            ))
        );
    }

    fn far(selector: u16, reason: FarJumpReason) -> ProtectedRefusal {
        ProtectedRefusal::FarJumpRefused {
            eip: 0x2000,
            selector,
            reason,
        }
    }

    #[test]
    fn far_jump_accepts_a_conforming_segment_and_clears_the_rpl() {
        // Conforming code (type bit 2) with DPL 0: RPL is not checked; CS.RPL becomes the CPL (0).
        let conforming = [0xFF, 0xFF, 0, 0, 0, 0x9E, 0xCF, 0];
        let (mut cpu, mut ram) = (entry_cpu(), paged_ram(conforming));
        cpu.efer = 0xD01;
        let [lo, hi] = 0x11u16.to_le_bytes();
        let code = [0xEA, 0xF0, 0x20, 0, 0, lo, hi];
        assert!(step_one(&mut cpu, &mut ram, &code).is_ok());
        assert_eq!(
            (cpu.cs.selector, cpu.cs.access, cpu.eip),
            (0x10, 0x9F, 0x20F0)
        );
        assert_eq!(ram[0x4D], 0x9F, "the accessed bit is written to the GDT");
    }

    #[test]
    fn page_faults_carry_the_linear_address_and_keep_the_bits_of_completed_walks() {
        // (a) the code page is not present: the fetch walk is refused, nothing is set.
        let mut ram = paged_ram(CODE64_A_SET);
        tables_4k(&mut ram);
        put_q(&mut ram, PT + 16, 0);
        let mut cpu = ia32e_cpu();
        cpu.step(&mut mem(&mut ram)).unwrap();
        let (before_cpu, before_ram) = (cpu, ram.clone());
        assert_eq!(
            cpu.step(&mut mem(&mut ram)),
            Err(ProtectedRefusal::PageFault {
                eip: 0x2065,
                linear: 0x2065,
                access: AccessKind::Fetch,
                reason: PageRefusal::NotPresent { level: 1, entry: 0 }
            })
        );
        assert_eq!((cpu, &ram), (before_cpu, &before_ram));
        // (b) the GDT page is not present: the fetch walk completed and keeps its A bits; the
        // descriptor read at 0x48 is refused.
        let mut ram = paged_ram(CODE64_A_SET);
        tables_4k(&mut ram);
        put_q(&mut ram, PT, 0);
        let mut cpu = ia32e_cpu();
        cpu.step(&mut mem(&mut ram)).unwrap();
        let before_cpu = cpu;
        assert_eq!(
            cpu.step(&mut mem(&mut ram)),
            Err(ProtectedRefusal::PageFault {
                eip: 0x2065,
                linear: 0x48,
                access: AccessKind::Read,
                reason: PageRefusal::NotPresent { level: 1, entry: 0 }
            })
        );
        assert_eq!(cpu, before_cpu, "state unchanged");
        assert_eq!(get_q(&ram, PML4), PDPT as u64 | 0x23);
        assert_eq!(get_q(&ram, PT + 16), 0x2000 | 0x23);
        assert_eq!(get_q(&ram, PT + 8), 0x1003);
        // (c) a read-only 2 MiB page and an A-clear descriptor: the write of A is refused (WP = 1)
        // and reported at the start of the descriptor; PDE.D and the access byte are untouched.
        let mut ram = paged_ram(CODE64_A_CLEAR);
        put_q(&mut ram, PD, 0x81);
        let mut cpu = ia32e_cpu();
        cpu.step(&mut mem(&mut ram)).unwrap();
        assert_eq!(
            cpu.step(&mut mem(&mut ram)),
            Err(ProtectedRefusal::PageFault {
                eip: 0x2065,
                linear: 0x48,
                access: AccessKind::Write,
                reason: PageRefusal::WriteProtected { entry: 0x81 }
            })
        );
        assert_eq!((get_q(&ram, PD), ram[0x4D]), (0xA1, 0x9A));
        // (d) XD in the PDE: with NXE = 1 a fetch is refused, with NXE = 0 the bit is reserved.
        for (efer, reason) in [
            (
                0x901u64,
                PageRefusal::NoExecute {
                    entry: 0x83 | 1 << 63,
                },
            ),
            (
                0x101,
                PageRefusal::ReservedBits {
                    level: 2,
                    entry: 0x83 | 1 << 63,
                },
            ),
        ] {
            let mut ram = paged_ram(CODE64_A_SET);
            put_q(&mut ram, PD, 0x83 | 1 << 63);
            let mut cpu = ia32e_cpu();
            cpu.efer = efer;
            cpu.step(&mut mem(&mut ram)).unwrap();
            let before_ram = ram.clone();
            assert_eq!(
                cpu.step(&mut mem(&mut ram)),
                Err(ProtectedRefusal::PageFault {
                    eip: 0x2065,
                    linear: 0x2065,
                    access: AccessKind::Fetch,
                    reason
                })
            );
            assert_eq!(ram, before_ram);
        }
    }

    #[test]
    fn segment_loads_go_through_the_walk_when_paging_is_on() {
        let mut ram = paged_ram(FLAT_DATA_A_CLEAR);
        let mut cpu = active_cpu();
        cpu.gpr[EDX] = 0x10;
        put(&mut ram, 0x2000, &[0x8E, 0xDA]); // mov ds, edx
        cpu.step(&mut mem(&mut ram)).unwrap();
        assert_eq!(cpu.ds, flat(0x10, 0x93));
        assert_eq!(ram[0x4D], 0x93);
        // A from the fetch and the descriptor read, D from the write of the access byte.
        assert_eq!(get_q(&ram, PD), 0xE3);
    }

    #[test]
    fn cr3_takes_effect_at_once_and_cr4_pae_and_efer_lme_are_locked_while_paging() {
        let tables = |ram: &mut Vec<u8>| preset_accessed_dirty(ram, false);
        // MOV CR3: the next fetch walks the new (empty) PML4 at 0x6000.
        let mut ram = paged_ram(CODE32_COMPAT);
        tables(&mut ram);
        put(
            &mut ram,
            0x2000,
            &[0xB8, 0x00, 0x60, 0, 0, 0x0F, 0x22, 0xD8, 0x90],
        );
        let mut cpu = active_cpu();
        let m = &mut mem(&mut ram);
        cpu.step(m).unwrap();
        cpu.step(m).unwrap();
        assert_eq!(cpu.cr3, 0x6000);
        assert_eq!(
            cpu.step(m),
            Err(ProtectedRefusal::PageFault {
                eip: 0x2008,
                linear: 0x2008,
                access: AccessKind::Fetch,
                reason: PageRefusal::NotPresent { level: 4, entry: 0 }
            })
        );
        // MOV CR4 clearing PAE is #GP(0) with IA-32e active, others are stored.
        let (mut cpu, mut ram) = (active_cpu(), paged_ram(CODE32_COMPAT));
        tables(&mut ram);
        cpu.gpr[EAX] = 0x690;
        assert_eq!(
            step_one(&mut cpu, &mut ram, &[0x0F, 0x22, 0xE0]),
            Err(ProtectedRefusal::Cr4PaeClearInLongMode {
                eip: 0x2000,
                value: 0x690
            })
        );
        cpu.gpr[EAX] = 0x6A0;
        cpu.eip = 0x2000;
        assert!(step_one(&mut cpu, &mut ram, &[0x0F, 0x22, 0xE0]).is_ok());
        assert_eq!(cpu.cr4, 0x6A0);
        // WRMSR that changes LME is refused; one that keeps it is stored (LMA kept).
        cpu.eip = 0x2000;
        cpu.gpr[ECX] = MSR_EFER;
        cpu.gpr[EDX] = 0;
        cpu.gpr[EAX] = 0x801;
        assert_eq!(
            step_one(&mut cpu, &mut ram, &[0x0F, 0x30]),
            Err(ProtectedRefusal::EferLmeChangeWithPaging {
                eip: 0x2000,
                value: 0x801
            })
        );
        cpu.gpr[EAX] = 0x101;
        cpu.eip = 0x2000;
        assert!(step_one(&mut cpu, &mut ram, &[0x0F, 0x30]).is_ok());
        assert_eq!(cpu.efer, 0x501);
    }

    #[test]
    fn a_64_bit_cs_executes_only_the_block_8_jump() {
        // S6-14m expected `Mode64NotModelled` for every 64-bit step; S6-14n decodes the one form
        // of block 8 and refuses the rest as `Unsupported` (with the bytes).
        let (mut cpu, mut ram) = (active_cpu(), paged_ram(CODE64_A_SET));
        preset_accessed_dirty(&mut ram, false);
        cpu.cs = ProtectedSegment {
            selector: 0x10,
            base: 0,
            limit: 0xFFFF_FFFF,
            access: 0x9B,
            flags: 0xA,
        };
        let Err(ProtectedRefusal::Unsupported { eip, linear, bytes }) =
            step_one(&mut cpu, &mut ram, &[0xB8, 0, 0, 0, 0])
        else {
            panic!("expected Unsupported");
        };
        assert_eq!(
            (eip, linear, &bytes[..5]),
            (0x2000, 0x2000, &[0xB8, 0, 0, 0, 0][..])
        );
    }

    #[test]
    fn linked_bytes_run_22_steps_through_paging_to_the_64_bit_cs() {
        let mut ram = paged_ram(CODE64_A_SET);
        put_linked_block6(&mut ram);
        put(&mut ram, 0x5008, &0x901u32.to_le_bytes());
        put(&mut ram, 0x500C, &0u32.to_le_bytes());
        put(&mut ram, 0x5010, &0x6B0u32.to_le_bytes());
        put(&mut ram, 0x5014, &0u32.to_le_bytes());
        let mut cpu = entry_cpu();
        let run = cpu.run(&mut mem(&mut ram), 100);
        // 13 steps to the RDMSR, 7 with the WRMSR path, `mov cr0` and the far jump.
        assert_eq!(run.steps, 22);
        // S6-14n: the 64-bit CS is executed; the HLT at 0x20f0 is not a form of block 8.
        assert!(matches!(
            run.stop,
            Some(ProtectedRefusal::Unsupported { eip: 0x20F0, .. })
        ));
        assert_eq!(
            (cpu.cr0, cpu.cr3, cpu.cr4, cpu.efer),
            (0x8005_0033, 0x4000, 0x6B0, 0xD01)
        );
        assert_eq!(cpu.cs.selector, 0x10);
        assert_eq!(cpu.cs.flags & 0x6, 0x2, "L = 1, D = 0");
        assert_eq!(
            cpu.gdtr,
            TableRegister {
                base: 0x38,
                limit: 0x1F
            }
        );
        // The three entries the walks used have A; D only where something was written.
        assert_eq!(get_q(&ram, PML4), PDPT as u64 | 0x23);
        assert_eq!(get_q(&ram, PDPT), PD as u64 | 0x23);
        assert_eq!(get_q(&ram, PD), 0xA3);
    }

    // ---- comparison with the KVM oracle (S6-14m) ------------------------------------------
    //
    // Files in docs/reports/data/stage6/s6-14m/ (make-images.py, run-oracle.sh): the S6-14j prefix
    // through `RealModeCpu`, the linked block 6 bytes 0x2000..0x2062, the linked `mov cr0, eax`
    // (PG, LME and PAE) and the far jump `jmp 0x10:0x20f0`, with the descriptor of selector 0x10,
    // the page tables (PML4 0x4000, PDPT 0x7000, PD 0x8000, PT 0x9000) and EFER/CR4 of the header
    // varied per image; a HLT stands at 0x20f0. Compared: GPRs, EIP, EFLAGS, CR0, CR3, CR4, EFER
    // (LMA included), the six segments, GDTR, IDTR, `CR2` for a `#PF`, the GDT block and the first
    // 64 bytes of each of the four paging structures (A and D bits).

    #[derive(Clone, Copy)]
    enum OracleOutcomeM {
        /// 64-bit CS: the HLT at 0x20f0 is `Unsupported` for the model (S6-14n; S6-14m: stop
        /// with `Mode64NotModelled`).
        Long64,
        /// Compatibility CS: the HLT at 0x20f0 is `Unsupported` for the model.
        Compat,
        /// `#GP`/`#NP` at the far jump 0x2065.
        FarJump(u16, FarJumpReason),
        /// `#GP` at the `MOV CR0, EAX` at 0x2062 (PAE clear).
        Cr0Gp,
        /// `#PF` at 0x2065 with this `CR2`, access and reason.
        Page(u32, AccessKind, PageRefusal),
    }

    fn compare_tables(ram: &[u8], tables: &str) {
        let lines: Vec<&str> = tables.lines().collect();
        assert_eq!(lines.len(), 4);
        for (line, expected_gpa) in lines.iter().zip([PML4, PDPT, PD, PT]) {
            let (gpa, bytes) = oracle_result_block(line);
            assert_eq!(gpa as usize, expected_gpa);
            assert_eq!(
                &ram[expected_gpa..expected_gpa + 64],
                &bytes[..],
                "table {gpa:#x}"
            );
        }
    }

    fn compare_with_oracle_m(
        image: &[u8],
        json: &str,
        tables: &str,
        expected_steps: u64,
        outcome: OracleOutcomeM,
    ) {
        use OracleOutcomeM::*;
        let (mut cpu, mut ram) = enter_from_oracle_image_sized(image, json, RAM_PAGED);
        let run = cpu.run(&mut mem(&mut ram), 100);
        assert_eq!(run.steps, expected_steps);
        let (halt, rip_delta, rflags_ignore, cr2) = match outcome {
            // S6-14n: the 64-bit CS is executed, so the HLT at 0x20f0 is `Unsupported` in both
            // cases (the S6-14m expectation for `Long64` was `Mode64NotModelled`).
            Long64 | Compat => {
                let Some(ProtectedRefusal::Unsupported { eip, bytes, .. }) = run.stop else {
                    panic!("expected a stop at the HLT, got {:?}", run.stop);
                };
                assert_eq!((eip, bytes[0]), (0x20F0, 0xF4));
                (true, 1, 0, 0)
            }
            FarJump(selector, reason) => {
                assert_eq!(run.stop, Some(far_at(0x2065, selector, reason)));
                (false, 0, 0x1_0000, 0)
            }
            Cr0Gp => {
                assert_eq!(
                    run.stop,
                    Some(ProtectedRefusal::LongModePagingWithoutPae {
                        eip: 0x2062,
                        value: 0x8005_0033
                    })
                );
                (false, 0, 0x1_0000, 0)
            }
            Page(linear, access, reason) => {
                assert_eq!(
                    run.stop,
                    Some(ProtectedRefusal::PageFault {
                        eip: 0x2065,
                        linear,
                        access,
                        reason
                    })
                );
                (false, 0, 0x1_0000, u64::from(linear))
            }
        };
        if halt {
            assert!(json.contains("\"stop_reason\":\"guest_hlt\""));
            assert!(json.contains("\"exits\":[{\"kind\":\"hlt\"}]"));
        } else {
            assert!(json.contains("\"stop_reason\":\"triple_fault_or_shutdown\""));
            assert!(json.contains("\"exits\":[{\"kind\":\"shutdown\"}]"));
        }
        compare_state_cr2(&cpu, &ram, json, rip_delta, rflags_ignore, cr2);
        compare_tables(&ram, tables);
    }

    fn far_at(eip: u32, selector: u16, reason: FarJumpReason) -> ProtectedRefusal {
        ProtectedRefusal::FarJumpRefused {
            eip,
            selector,
            reason,
        }
    }

    macro_rules! oracle_case_m {
        ($name:ident, $stem:literal, $steps:expr, $outcome:expr) => {
            #[test]
            fn $name() {
                let image: &[u8] = include_bytes!(concat!(
                    "../../../docs/reports/data/stage6/s6-14m/",
                    $stem,
                    ".bin"
                ));
                let json: &str = include_str!(concat!(
                    "../../../docs/reports/data/stage6/s6-14m/oracle-",
                    $stem,
                    ".json"
                ));
                let tables: &str = include_str!(concat!(
                    "../../../docs/reports/data/stage6/s6-14m/oracle-",
                    $stem,
                    "-tables.jsonl"
                ));
                compare_with_oracle_m(image, json, tables, $steps, $outcome);
            }
        };
    }
    use OracleOutcomeM::{Compat, Cr0Gp, FarJump, Long64, Page};
    const XD_PDE: u64 = 0x83 | 1 << 63;
    // a: Linux-like: 2 MiB page, code64 with A set; A set in PML4E, PDPTE, PDE (0xa3), no D.
    oracle_case_m!(
        oracle_m_a_ia32e_2mib_code64_accessed_set,
        "a-ia32e-2mib-code64-accessed-set-0000-0000",
        22,
        Long64
    );
    // b: A-clear descriptor: the access byte and PDE.D are written through the walk (PDE 0xe3).
    oracle_case_m!(
        oracle_m_b_ia32e_2mib_code64_accessed_clear,
        "b-ia32e-2mib-code64-accessed-clear-0000-0000",
        22,
        Long64
    );
    // c: 4 KiB pages: PTE[2] A (fetch), PTE[0] A+D (the GDT write), the others untouched.
    oracle_case_m!(
        oracle_m_c_ia32e_4k_code64_accessed_clear,
        "c-ia32e-4k-code64-accessed-clear-0000-0000",
        22,
        Long64
    );
    // d: compatibility target (L = 0, D = 1): EFER.LMA = 1, CS.L = 0, the HLT runs in compat mode.
    oracle_case_m!(
        oracle_m_d_compat_target_code32,
        "d-compat-target-code32-0000-0000",
        22,
        Compat
    );
    oracle_case_m!(
        oracle_m_e_long_and_d_set_gp,
        "e-long-and-d-set-gp-0000-0000",
        21,
        FarJump(0x10, FarJumpReason::LongCodeWith32BitDefault)
    );
    oracle_case_m!(
        oracle_m_f_code64_not_present,
        "f-code64-not-present-0000-0000",
        21,
        FarJump(0x10, FarJumpReason::NotPresent)
    );
    oracle_case_m!(
        oracle_m_g_selector_beyond_gdt_limit,
        "g-selector-beyond-gdt-limit-0000-0000",
        21,
        FarJump(0x28, FarJumpReason::BeyondGdtLimit { limit: 0x1F })
    );
    oracle_case_m!(
        oracle_m_h_selector_is_data,
        "h-selector-is-data-0000-0000",
        21,
        FarJump(0x10, FarJumpReason::NotCodeSegment { access: 0x93 })
    );
    oracle_case_m!(
        oracle_m_i_code64_dpl3,
        "i-code64-dpl3-0000-0000",
        21,
        FarJump(0x10, FarJumpReason::Privilege { rpl: 0, dpl: 3 })
    );
    // j: PAE clear: `#GP` at the MOV CR0; CR0 and EFER.LMA unchanged, no table touched.
    oracle_case_m!(
        oracle_m_j_cr4_pae_clear_gp_at_mov_cr0,
        "j-cr4-pae-clear-gp-at-mov-cr0-0000-0000",
        20,
        Cr0Gp
    );
    oracle_case_m!(
        oracle_m_k_pte_code_page_not_present,
        "k-pte-code-page-not-present-0000-0000",
        21,
        Page(
            0x2065,
            AccessKind::Fetch,
            PageRefusal::NotPresent { level: 1, entry: 0 }
        )
    );
    // l: the GDT page is not present: the fetch walk keeps its A bits, CR2 = 0x48.
    oracle_case_m!(
        oracle_m_l_pte_gdt_page_not_present,
        "l-pte-gdt-page-not-present-0000-0000",
        21,
        Page(
            0x48,
            AccessKind::Read,
            PageRefusal::NotPresent { level: 1, entry: 0 }
        )
    );
    oracle_case_m!(
        oracle_m_m_pde_xd_nxe_on,
        "m-pde-xd-nxe-on-0000-0000",
        21,
        Page(
            0x2065,
            AccessKind::Fetch,
            PageRefusal::NoExecute { entry: XD_PDE }
        )
    );
    oracle_case_m!(
        oracle_m_n_pde_xd_nxe_off,
        "n-pde-xd-nxe-off-0000-0000",
        21,
        Page(
            0x2065,
            AccessKind::Fetch,
            PageRefusal::ReservedBits {
                level: 2,
                entry: XD_PDE
            }
        )
    );
    oracle_case_m!(
        oracle_m_o_pde_2mib_misaligned_bit13,
        "o-pde-2mib-misaligned-bit13-0000-0000",
        21,
        Page(
            0x2065,
            AccessKind::Fetch,
            PageRefusal::ReservedBits {
                level: 2,
                entry: 0x83 | 1 << 13
            }
        )
    );
    oracle_case_m!(
        oracle_m_p_pml4e_ps_reserved,
        "p-pml4e-ps-reserved-0000-0000",
        21,
        Page(
            0x2065,
            AccessKind::Fetch,
            PageRefusal::ReservedBits {
                level: 4,
                entry: PDPT as u64 | 0x83
            }
        )
    );
    oracle_case_m!(
        oracle_m_q_pdpte_1gib_page,
        "q-pdpte-1gib-page-0000-0000",
        21,
        Page(
            0x2065,
            AccessKind::Fetch,
            PageRefusal::GigabytePage { entry: 0x83 }
        )
    );
    // r: read-only 2 MiB page, A-clear descriptor: the write of A is `#PF` with CR2 = 0x48 (the
    // start of the descriptor, not the byte at 0x4d that is written).
    oracle_case_m!(
        oracle_m_r_pde_readonly_accessed_clear,
        "r-pde-readonly-accessed-clear-0000-0000",
        21,
        Page(
            0x48,
            AccessKind::Write,
            PageRefusal::WriteProtected { entry: 0x81 }
        )
    );
    oracle_case_m!(
        oracle_m_s_pml4e_not_present,
        "s-pml4e-not-present-0000-0000",
        21,
        Page(
            0x2065,
            AccessKind::Fetch,
            PageRefusal::NotPresent { level: 4, entry: 0 }
        )
    );
    // t: A and D preset everywhere: no memory is written.
    oracle_case_m!(
        oracle_m_t_tables_accessed_dirty_preset,
        "t-tables-accessed-dirty-preset-0000-0000",
        22,
        Long64
    );

    // ---- block 8: the 64-bit indirect jump (S6-14n) ----------------------------------------

    const JMP_RIP_2F0A: [u8; 6] = [0xFF, 0x25, 0x0A, 0x2F, 0x00, 0x00];
    const SECONDARY_STARTUP_64: u64 = 0xFFFF_FFFF_8139_C090;

    /// IA-32e mode with the 64-bit CS (selector 0x10, `L = 1`) at the block 8 jump, 0x20f0.
    fn cpu64() -> ProtectedCpu {
        let mut cpu = active_cpu();
        cpu.cs = ProtectedSegment {
            selector: 0x10,
            base: 0,
            limit: 0xFFFF_FFFF,
            access: 0x9B,
            flags: 0xA,
        };
        cpu.eip = 0x20F0;
        cpu
    }

    /// The paged RAM of the S6-14n images: `jmp [rip+0x2f0a]` at 0x20f0 and `cell` at 0x5000; the
    /// tables are 2 MiB (or 4 KiB with `pt`) identity ones with A and D preset, so that no walk
    /// writes memory.
    fn block8_ram(cell: u64, pt: bool) -> Vec<u8> {
        let mut ram = paged_ram(CODE64_A_SET);
        if pt {
            tables_4k(&mut ram);
        }
        preset_accessed_dirty(&mut ram, pt);
        put(&mut ram, 0x20F0, &JMP_RIP_2F0A);
        put_q(&mut ram, 0x5000, cell);
        ram
    }

    #[test]
    fn rip_relative_jump_reads_the_cell_and_takes_its_value_as_rip() {
        let mut ram = block8_ram(SECONDARY_STARTUP_64, false);
        let mut cpu = cpu64();
        cpu.eflags = 0x97;
        cpu.gpr[EAX] = 0x1234;
        let before = cpu;
        // 0x20f0 + 6 + 0x2f0a = 0x5000: the displacement is relative to the next instruction.
        assert_eq!(
            cpu.step(&mut mem(&mut ram)),
            Ok(ProtectedInstruction::JmpIndirectRipRel { disp: 0x2F0A })
        );
        assert_eq!(cpu.rip(), SECONDARY_STARTUP_64);
        assert_eq!((cpu.eip, cpu.rip_high), (0x8139_C090, 0xFFFF_FFFF));
        assert!(cpu.target_entered);
        // Registers, flags, segments and control state are as before.
        assert_eq!(
            ProtectedCpu {
                eip: before.eip,
                rip_high: 0,
                target_entered: false,
                ..cpu
            },
            before
        );
        // The target is neither fetched nor executed: every further step stops there.
        let (stopped, ram_stopped) = (cpu, ram.clone());
        assert_eq!(
            cpu.step(&mut mem(&mut ram)),
            Err(ProtectedRefusal::Mode64TargetEntered {
                rip: SECONDARY_STARTUP_64
            })
        );
        assert_eq!((cpu, ram), (stopped, ram_stopped));
    }

    #[test]
    fn the_displacement_is_relative_to_the_next_instruction_and_signed() {
        // Same cell at 0x5000 from another instruction address: 0x2200 + 6 + 0x2dfa.
        let mut ram = block8_ram(0x0000_0000_0000_2100, false);
        put(&mut ram, 0x2200, &[0xFF, 0x25, 0xFA, 0x2D, 0x00, 0x00]);
        let mut cpu = cpu64();
        cpu.eip = 0x2200;
        assert!(cpu.step(&mut mem(&mut ram)).is_ok());
        assert_eq!((cpu.rip(), cpu.target_entered), (0x2100, true));
        // A negative displacement is sign-extended: 0x20f0 + 6 - 0x16 = 0x20e0.
        let mut ram = block8_ram(0, false);
        put(&mut ram, 0x20F0, &[0xFF, 0x25, 0xEA, 0xFF, 0xFF, 0xFF]);
        put_q(&mut ram, 0x20E0, 0x0000_1234_5678_9ABC);
        let mut cpu = cpu64();
        assert!(cpu.step(&mut mem(&mut ram)).is_ok());
        assert_eq!(cpu.rip(), 0x0000_1234_5678_9ABC);
        assert_eq!((cpu.eip, cpu.rip_high), (0x5678_9ABC, 0x1234));
    }

    #[test]
    fn canonical_means_bits_63_to_47_are_equal() {
        // SDM Vol. 1 section 3.3.7.1 (by memory): 48-bit addresses, sign-extended from bit 47.
        let cpu = cpu64();
        for (address, canonical) in [
            (0, true),
            (0x0000_7FFF_FFFF_FFFF, true),
            (0x0000_8000_0000_0000, false),
            (0x0001_0000_0000_0000, false),
            (0x8000_0000_0000_0000, false),
            (0xFFFF_7FFF_FFFF_FFFF, false),
            (0xFFFF_8000_0000_0000, true),
            (SECONDARY_STARTUP_64, true),
            (u64::MAX, true),
        ] {
            assert_eq!(
                cpu.check_canonical(address, AddressRole::JumpTarget)
                    .is_ok(),
                canonical,
                "{address:#x}"
            );
        }
        // An operand that is canonical at its first byte and not at its last is refused too.
        let mut touches = Vec::new();
        let ram = block8_ram(0, false);
        let m = ImageMemory {
            base: 0,
            bytes: &ram,
        };
        assert_eq!(
            cpu.read_linear_u64(&m, 0x0000_7FFF_FFFF_FFFC, &mut touches),
            Err(ProtectedRefusal::NonCanonicalAddress {
                rip: 0x20F0,
                address: 0x0000_8000_0000_0003,
                role: AddressRole::Operand
            })
        );
        assert_eq!(
            cpu.read_linear_u64(&m, 0x0000_8000_0000_0000, &mut touches),
            Err(ProtectedRefusal::NonCanonicalAddress {
                rip: 0x20F0,
                address: 0x0000_8000_0000_0000,
                role: AddressRole::Operand
            })
        );
    }

    #[test]
    fn a_non_canonical_target_is_refused_and_changes_nothing() {
        for target in [
            0x0000_8000_0000_0000u64,
            0xFFFF_7FFF_FFFF_FFFF,
            0x1234_0000_0000_2100,
        ] {
            let (mut cpu, mut ram) = (cpu64(), block8_ram(target, false));
            assert_eq!(
                step_one(&mut cpu, &mut ram, &[]),
                Err(ProtectedRefusal::NonCanonicalAddress {
                    rip: 0x20F0,
                    address: target,
                    role: AddressRole::JumpTarget
                })
            );
        }
        // The boundary values are accepted (they are canonical; the model never fetches them).
        for target in [0x0000_7FFF_FFFF_FFFFu64, 0xFFFF_8000_0000_0000, 0] {
            let (mut cpu, mut ram) = (cpu64(), block8_ram(target, false));
            assert!(cpu.step(&mut mem(&mut ram)).is_ok());
            assert_eq!(cpu.rip(), target);
        }
    }

    #[test]
    fn a_cell_beyond_4_gib_is_outside_the_page_model() {
        // disp = -0x8000_0000: 0x20f6 - 0x8000_0000 sign-extends to 0xffff_ffff_8000_20f6.
        let (mut cpu, mut ram) = (cpu64(), block8_ram(0, false));
        put(&mut ram, 0x20F0, &[0xFF, 0x25, 0x00, 0x00, 0x00, 0x80]);
        assert_eq!(
            step_one(&mut cpu, &mut ram, &[]),
            Err(ProtectedRefusal::LinearBeyond4GiB {
                rip: 0x20F0,
                linear: 0xFFFF_FFFF_8000_20F6
            })
        );
        // disp = 0x7fff_ffff: 0x20f6 + 0x7fff_ffff = 0x8000_20f5, below 4 GiB, not mapped (PDPT[2]).
        put(&mut ram, 0x20F0, &[0xFF, 0x25, 0xFF, 0xFF, 0xFF, 0x7F]);
        assert_eq!(
            step_one(&mut cpu, &mut ram, &[]),
            Err(ProtectedRefusal::PageFault {
                eip: 0x20F0,
                linear: 0x8000_20F5,
                access: AccessKind::Read,
                reason: PageRefusal::NotPresent { level: 3, entry: 0 }
            })
        );
    }

    #[test]
    fn the_cell_is_read_through_the_paging_walk_and_sets_accessed_bits() {
        // 4 KiB pages: the cell's linear page 5 is physical 0x6000; the fetch uses page 2.
        let mut ram = paged_ram(CODE64_A_SET);
        tables_4k(&mut ram);
        put_q(&mut ram, PT + 40, 0x6000 | 3);
        put(&mut ram, 0x20F0, &JMP_RIP_2F0A);
        put_q(&mut ram, 0x6000, SECONDARY_STARTUP_64);
        put_q(&mut ram, 0x5000, 0xDEAD_BEEF);
        let mut cpu = cpu64();
        assert!(cpu.step(&mut mem(&mut ram)).is_ok());
        assert_eq!(cpu.rip(), SECONDARY_STARTUP_64);
        // A in PML4E, PDPTE, PDE, PTE(2) (fetch) and PTE(5) (read); no D; others untouched.
        assert_eq!(get_q(&ram, PML4), PDPT as u64 | 0x23);
        assert_eq!(get_q(&ram, PDPT), PD as u64 | 0x23);
        assert_eq!(get_q(&ram, PD), PT as u64 | 0x23);
        assert_eq!(get_q(&ram, PT + 16), 0x2000 | 0x23);
        assert_eq!(get_q(&ram, PT + 40), 0x6000 | 0x23);
        assert_eq!(get_q(&ram, PT), 0x3, "page 0 was not used");
        assert_eq!(get_q(&ram, PT + 8), 0x1003, "page 1 was not used");
        // A read-only PDE (R/W = 0) lets the read through and does not set D.
        let mut ram = block8_ram(SECONDARY_STARTUP_64, false);
        put_q(&mut ram, PD, 0x81 | 0x20);
        let mut cpu = cpu64();
        cpu.cr0 |= CR0_WP;
        assert!(cpu.step(&mut mem(&mut ram)).is_ok());
        assert_eq!(get_q(&ram, PD), 0xA1);
    }

    #[test]
    fn a_cell_across_a_page_boundary_walks_both_pages() {
        // disp = 0x5ffc - 0x20f6 = 0x3f06; the QWORD covers 0x5ffc..0x6003 (PT[5] and PT[6]).
        let mut ram = paged_ram(CODE64_A_SET);
        tables_4k(&mut ram);
        put(&mut ram, 0x20F0, &[0xFF, 0x25, 0x06, 0x3F, 0x00, 0x00]);
        put_q(&mut ram, 0x5FFC, SECONDARY_STARTUP_64);
        let mut cpu = cpu64();
        assert!(cpu.step(&mut mem(&mut ram)).is_ok());
        assert_eq!(cpu.rip(), SECONDARY_STARTUP_64);
        assert_eq!(get_q(&ram, PT + 40), 0x5000 | 0x23);
        assert_eq!(get_q(&ram, PT + 48), 0x6000 | 0x23);
        // The second page not present: the walk of byte 4 faults at 0x6000. The walk of bytes 0..3
        // (PTE(5)) is complete, so its A bit stays set (S6-14m rule: completed walks of an
        // instruction that faults later are committed; model behaviour, not confirmed by the oracle).
        let mut ram = paged_ram(CODE64_A_SET);
        tables_4k(&mut ram);
        put(&mut ram, 0x20F0, &[0xFF, 0x25, 0x06, 0x3F, 0x00, 0x00]);
        put_q(&mut ram, PT + 48, 0);
        let mut cpu = cpu64();
        let before = cpu;
        // Not `step_one`: it asserts that memory is unchanged, which is not so here (A bits).
        assert_eq!(
            cpu.step(&mut mem(&mut ram)),
            Err(ProtectedRefusal::PageFault {
                eip: 0x20F0,
                linear: 0x6000,
                access: AccessKind::Read,
                reason: PageRefusal::NotPresent { level: 1, entry: 0 }
            })
        );
        assert_eq!(cpu, before, "state unchanged by the refusal");
        assert_eq!(
            get_q(&ram, PT + 40),
            0x5000 | 0x23,
            "PTE(5): A set by the completed walk"
        );
        assert_eq!(get_q(&ram, PT + 48), 0, "PTE(6) stays not present");
    }

    #[test]
    fn a_cell_page_that_is_not_present_is_a_page_fault_at_the_cell() {
        let (mut cpu, mut ram) = (cpu64(), block8_ram(SECONDARY_STARTUP_64, true));
        put_q(&mut ram, PT + 40, 0);
        assert_eq!(
            step_one(&mut cpu, &mut ram, &[]),
            Err(ProtectedRefusal::PageFault {
                eip: 0x20F0,
                linear: 0x5000,
                access: AccessKind::Read,
                reason: PageRefusal::NotPresent { level: 1, entry: 0 }
            })
        );
        // The same with the PML4 entry not present.
        let (mut cpu, mut ram) = (cpu64(), block8_ram(SECONDARY_STARTUP_64, false));
        put_q(&mut ram, PML4, 0);
        // The fetch of the instruction faults first (decode), at 0x20f0.
        assert_eq!(
            step_one(&mut cpu, &mut ram, &[]),
            Err(ProtectedRefusal::PageFault {
                eip: 0x20F0,
                linear: 0x20F0,
                access: AccessKind::Fetch,
                reason: PageRefusal::NotPresent { level: 4, entry: 0 }
            })
        );
    }

    #[test]
    fn the_64_bit_cs_base_and_limit_are_ignored() {
        let (mut cpu, mut ram) = (cpu64(), block8_ram(SECONDARY_STARTUP_64, false));
        cpu.cs.base = 0x1000;
        cpu.cs.limit = 0xFFF;
        assert!(cpu.step(&mut mem(&mut ram)).is_ok());
        assert_eq!(cpu.rip(), SECONDARY_STARTUP_64);
        // DS has no effect on the cell either.
        let (mut cpu, mut ram) = (cpu64(), block8_ram(SECONDARY_STARTUP_64, false));
        cpu.ds.base = 0x1000;
        cpu.ds.limit = 0;
        assert!(cpu.step(&mut mem(&mut ram)).is_ok());
        assert_eq!(cpu.rip(), SECONDARY_STARTUP_64);
    }

    #[test]
    fn every_other_64_bit_instruction_is_unsupported() {
        for code in [
            &[0xF4u8][..],                                   // HLT
            &[0x48, 0xFF, 0x25, 0x0A, 0x2F, 0x00, 0x00][..], // REX.W prefix
            &[0xFF, 0x24, 0x25, 0x00, 0x50, 0x00, 0x00][..], // JMP [disp32] with a SIB byte
            &[0xFF, 0x2D, 0x0A, 0x2F, 0x00, 0x00][..],       // /5: JMP FAR m16:32
            &[0xFF, 0xE0][..],                               // JMP RAX
            &[0xFF, 0x15, 0x0A, 0x2F, 0x00, 0x00][..],       // CALL [rip+disp32]
            &[0xE8, 0x6B, 0x01, 0x00, 0x00][..],             // CALL rel32 (the target's first byte)
            &[0x66, 0xFF, 0x25, 0x0A, 0x2F, 0x00, 0x00][..], // operand-size prefix
        ] {
            let (mut cpu, mut ram) = (cpu64(), block8_ram(SECONDARY_STARTUP_64, false));
            cpu.eip = 0x2000;
            let Err(ProtectedRefusal::Unsupported { eip, linear, bytes }) =
                step_one(&mut cpu, &mut ram, code)
            else {
                panic!("{code:02x?} must be Unsupported");
            };
            assert_eq!((eip, linear), (0x2000, 0x2000));
            assert_eq!(bytes[..code.len()], code[..], "{code:02x?}");
        }
    }

    #[test]
    fn the_64_bit_decoder_needs_ia32e_and_a_default_size_clear() {
        // L = 1 without EFER.LMA stays the 32-bit decoder's refusal (D/B = 0).
        let (mut cpu, mut ram) = (cpu64(), block8_ram(SECONDARY_STARTUP_64, false));
        cpu.efer &= !EFER_LMA;
        assert_eq!(
            step_one(&mut cpu, &mut ram, &[]),
            Err(ProtectedRefusal::CsNot32Bit { flags: 0xA })
        );
        // A RIP above 4 GiB that the jump did not set is outside the fetch model.
        let (mut cpu, mut ram) = (cpu64(), block8_ram(SECONDARY_STARTUP_64, false));
        cpu.rip_high = 1;
        assert_eq!(
            step_one(&mut cpu, &mut ram, &[]),
            Err(ProtectedRefusal::Mode64NotModelled { rip: 0x1_0000_20F0 })
        );
    }

    #[test]
    fn linked_bytes_run_23_steps_to_the_64_bit_target() {
        let mut ram = paged_ram(CODE64_A_SET);
        put_linked_block6(&mut ram);
        put(&mut ram, 0x20F0, &JMP_RIP_2F0A);
        put(&mut ram, 0x5000, &SECONDARY_STARTUP_64.to_le_bytes());
        put(&mut ram, 0x5008, &0x901u32.to_le_bytes());
        put(&mut ram, 0x5010, &0x6B0u32.to_le_bytes());
        let mut cpu = entry_cpu();
        let run = cpu.run(&mut mem(&mut ram), 100);
        // 22 steps to the 64-bit CS (S6-14m) and the indirect jump of block 8.
        assert_eq!(run.steps, 23);
        assert_eq!(
            run.stop,
            Some(ProtectedRefusal::Mode64TargetEntered {
                rip: SECONDARY_STARTUP_64
            })
        );
        assert_eq!(
            (cpu.cr0, cpu.cr3, cpu.cr4, cpu.efer),
            (0x8005_0033, 0x4000, 0x6B0, 0xD01)
        );
        // The cell was only read: it keeps its value.
        assert_eq!(get_q(&ram, 0x5000), SECONDARY_STARTUP_64);
    }

    // ---- comparison with the KVM oracle (S6-14n) ------------------------------------------
    //
    // Files in docs/reports/data/stage6/s6-14n/ (make-images.py, run-oracle.sh): the S6-14m path
    // up to the 64-bit CS, then the linked `jmp QWORD PTR [rip+0x2f0a]` at 0x20f0 with the cell at
    // 0x5000 and the target of the cell varied per image (a HLT stands at the target, so the
    // oracle executes it: its RIP is one past the target). The model stops before the target
    // (`Mode64TargetEntered`). Compared: GPRs, RIP, EFLAGS, CR0, CR3, CR4, EFER, the six segments,
    // GDTR, IDTR, `CR2` for a `#PF`, the GDT block and the first 64 bytes of PML4, PDPT, PD, PT and
    // of the trampoline header (the cell). Not compared (the model does not walk the target): the
    // entries of the oracle's fetch walk to the high target, `*-high.jsonl`.

    const RAM_N: usize = 0xE000;

    #[derive(Clone, Copy)]
    enum OracleOutcomeN {
        /// The jump is taken, the oracle executes the HLT at the target (the model stops there).
        Target(u64),
        /// The jump is taken to a canonical, unmapped target: `#PF` at its fetch, `CR2` = target.
        TargetFetchFault(u64),
        /// `#GP` at the jump: the cell holds this non-canonical address.
        NonCanonical(u64),
        /// `#PF` at the read of the cell at 0x5000 (PTE of page 5 not present).
        CellPageFault,
    }

    fn compare_tables_n(ram: &[u8], tables: &str) {
        let lines: Vec<&str> = tables.lines().collect();
        assert_eq!(lines.len(), 5);
        for (line, expected_gpa) in lines.iter().zip([PML4, PDPT, PD, PT, 0x5000]) {
            let (gpa, bytes) = oracle_result_block(line);
            assert_eq!(gpa as usize, expected_gpa);
            assert_eq!(
                &ram[expected_gpa..expected_gpa + 64],
                &bytes[..],
                "block {gpa:#x}"
            );
        }
    }

    fn compare_with_oracle_n(image: &[u8], json: &str, tables: &str, outcome: OracleOutcomeN) {
        use OracleOutcomeN::*;
        let (mut cpu, mut ram) = enter_from_oracle_image_sized(image, json, RAM_N);
        let run = cpu.run(&mut mem(&mut ram), 100);
        let (steps, stop, halt, rip_delta, cr2) = match outcome {
            Target(rip) => (
                23,
                ProtectedRefusal::Mode64TargetEntered { rip },
                true,
                1,
                0,
            ),
            TargetFetchFault(rip) => (
                23,
                ProtectedRefusal::Mode64TargetEntered { rip },
                false,
                0,
                rip,
            ),
            NonCanonical(address) => (
                22,
                ProtectedRefusal::NonCanonicalAddress {
                    rip: 0x20F0,
                    address,
                    role: AddressRole::JumpTarget,
                },
                false,
                0,
                0,
            ),
            CellPageFault => (
                22,
                ProtectedRefusal::PageFault {
                    eip: 0x20F0,
                    linear: 0x5000,
                    access: AccessKind::Read,
                    reason: PageRefusal::NotPresent { level: 1, entry: 0 },
                },
                false,
                0,
                0x5000,
            ),
        };
        assert_eq!(run.steps, steps);
        assert_eq!(run.stop, Some(stop));
        if halt {
            assert!(json.contains("\"stop_reason\":\"guest_hlt\""));
            assert!(json.contains("\"exits\":[{\"kind\":\"hlt\"}]"));
        } else {
            assert!(json.contains("\"stop_reason\":\"triple_fault_or_shutdown\""));
            assert!(json.contains("\"exits\":[{\"kind\":\"shutdown\"}]"));
        }
        compare_state_cr2(
            &cpu,
            &ram,
            json,
            rip_delta,
            if halt { 0 } else { 0x1_0000 },
            cr2,
        );
        compare_tables_n(&ram, tables);
    }

    macro_rules! oracle_case_n {
        ($name:ident, $stem:literal, $outcome:expr) => {
            #[test]
            fn $name() {
                let image: &[u8] = include_bytes!(concat!(
                    "../../../docs/reports/data/stage6/s6-14n/",
                    $stem,
                    ".bin"
                ));
                let json: &str = include_str!(concat!(
                    "../../../docs/reports/data/stage6/s6-14n/oracle-",
                    $stem,
                    ".json"
                ));
                let tables: &str = include_str!(concat!(
                    "../../../docs/reports/data/stage6/s6-14n/oracle-",
                    $stem,
                    "-tables.jsonl"
                ));
                compare_with_oracle_n(image, json, tables, $outcome);
            }
        };
    }
    use OracleOutcomeN::{CellPageFault, NonCanonical, Target, TargetFetchFault};
    // a: the linked path, 2 MiB identity page, cell = 0xffffffff8139c090.
    oracle_case_n!(
        oracle_n_a_2mib_secondary_startup_64,
        "a-2mib-secondary-startup-64-0000-0000",
        Target(SECONDARY_STARTUP_64)
    );
    // b: 4 KiB pages: the cell read sets A in PTE(5).
    oracle_case_n!(
        oracle_n_b_4k_secondary_startup_64,
        "b-4k-secondary-startup-64-0000-0000",
        Target(SECONDARY_STARTUP_64)
    );
    oracle_case_n!(
        oracle_n_c_low_target_0x2100,
        "c-low-target-0x2100-0000-0000",
        Target(0x2100)
    );
    oracle_case_n!(
        oracle_n_d_noncanonical_low_gp,
        "d-noncanonical-low-gp-0000-0000",
        NonCanonical(0x0000_8000_0000_0000)
    );
    oracle_case_n!(
        oracle_n_e_noncanonical_high_gp,
        "e-noncanonical-high-gp-0000-0000",
        NonCanonical(0xFFFF_7FFF_FFFF_FFFF)
    );
    oracle_case_n!(
        oracle_n_f_canonical_low_max_unmapped_pf,
        "f-canonical-low-max-unmapped-pf-0000-0000",
        TargetFetchFault(0x0000_7FFF_FFFF_FFFF)
    );
    oracle_case_n!(
        oracle_n_g_canonical_high_min_unmapped_pf,
        "g-canonical-high-min-unmapped-pf-0000-0000",
        TargetFetchFault(0xFFFF_8000_0000_0000)
    );
    oracle_case_n!(
        oracle_n_h_pte_cell_page_not_present_pf,
        "h-pte-cell-page-not-present-pf-0000-0000",
        CellPageFault
    );
    oracle_case_n!(
        oracle_n_i_pde_readonly_cell_read,
        "i-pde-readonly-cell-read-0000-0000",
        Target(SECONDARY_STARTUP_64)
    );
    // j: `ff 25 ea ff ff ff`: the cell at 0x20e0 (negative displacement).
    oracle_case_n!(
        oracle_n_j_negative_disp_cell_at_0x20e0,
        "j-negative-disp-cell-at-0x20e0-0000-0000",
        Target(SECONDARY_STARTUP_64)
    );
    // k: the cell at 0x5ffc..0x6003 crosses a page boundary (PTE(5) and PTE(6) get A).
    oracle_case_n!(
        oracle_n_k_4k_cell_crosses_page,
        "k-4k-cell-crosses-page-0000-0000",
        Target(SECONDARY_STARTUP_64)
    );
    // l: the CS descriptor has a base of 0x1000, which 64-bit mode ignores (the oracle reports it).
    oracle_case_n!(
        oracle_n_l_code64_base_1000_ignored,
        "l-code64-base-1000-ignored-0000-0000",
        Target(SECONDARY_STARTUP_64)
    );

    // ---- connected test: SIPI entry to the first 64-bit instruction (S6-14, step 5) --------
    //
    // Files in docs/reports/data/stage6/s6-14-step5/ (make-images.py, run-oracle.sh): the
    // trampoline of the S6-16e/f probe (the builder's own record of the instructions of S6-14a,
    // not a copy of the kernel block) entered at 0800:0000 (SIPI vector 0x08, state after INIT),
    // image at GPA 0x8000, a HLT at the first 64-bit instruction (GPA 0xB000). The model runs
    // `RealModeCpu` up to `NotRealMode`, `ProtectedCpu::from_real_entry`, `ProtectedCpu::run` up
    // to `Mode64TargetEntered` and `ProtectedCpu::into_long_mode`; every compared value comes
    // from the oracle JSON.

    const RAM_STEP5: usize = 0x16000;
    const STEP5_IMAGE: &[u8] = include_bytes!(
        "../../../docs/reports/data/stage6/s6-14-step5/a-probe-v2-normal-sipi-hlt-at-0xb000-0800-0000.bin"
    );
    const STEP5_JSON: &str = include_str!(
        "../../../docs/reports/data/stage6/s6-14-step5/oracle-a-probe-v2-normal-sipi-hlt-at-0xb000-0800-0000.json"
    );
    const STEP5_BLOCKS: &str = include_str!(
        "../../../docs/reports/data/stage6/s6-14-step5/oracle-a-probe-v2-normal-sipi-hlt-at-0xb000-0800-0000-blocks.jsonl"
    );
    /// GPA of the real-mode entry (`CS = 0x0800` is the SIPI vector 0x08) and of the image.
    const STEP5_ENTRY_GPA: usize = 0x8000;

    /// Fields of an oracle segment (`final_state.segments.<name>`) as `(selector, base, limit,
    /// access-compatible type, dpl, present, db, l, g)`.
    fn oracle_segment(json: &str, name: &str) -> [u64; 9] {
        let get = |field: &str| oracle_num(json, &["final_state", "segments", name, field]);
        [
            get("selector"),
            get("base"),
            get("limit"),
            get("type"),
            get("dpl"),
            get("present"),
            get("db"),
            get("l"),
            get("g"),
        ]
    }

    /// The same nine fields of the access byte and the flag nibble of a descriptor cache.
    fn cache_fields(selector: u16, base: u64, limit: u32, access: u8, flags: u8) -> [u64; 9] {
        [
            u64::from(selector),
            base,
            u64::from(limit),
            u64::from(access & 0x0F),
            u64::from((access >> 5) & 3),
            u64::from(access >> 7),
            u64::from((flags >> 2) & 1),
            u64::from((flags >> 1) & 1),
            u64::from(flags >> 3),
        ]
    }

    /// Compares the state the long-mode interpreter continues from (S6-16b) with `final_state` of
    /// the oracle JSON (the oracle stopped on the HLT at the target: its RIP is one past it).
    fn compare_long_mode_state(
        cpu: &ProtectedCpu,
        state: &crate::state::LongModeState,
        json: &str,
    ) {
        let fs = |path: &[&str]| {
            let mut full = vec!["final_state"];
            full.extend_from_slice(path);
            oracle_num(json, &full)
        };
        let gprs = [
            "rax", "rcx", "rdx", "rbx", "rsp", "rbp", "rsi", "rdi", "r8", "r9", "r10", "r11",
            "r12", "r13", "r14", "r15",
        ];
        for (index, name) in gprs.iter().enumerate() {
            assert_eq!(state.gpr[index], fs(&["gpr", name]), "{name}");
        }
        assert_eq!(state.rip + 1, fs(&["rip"]), "RIP");
        assert_eq!(state.rflags, fs(&["rflags"]), "RFLAGS");
        assert_eq!(state.cr0, Some(fs(&["control", "cr0"])), "CR0");
        assert_eq!(state.cr2, Some(fs(&["control", "cr2"])), "CR2");
        assert_eq!(state.cr3, Some(fs(&["control", "cr3"])), "CR3");
        assert_eq!(state.cr4, Some(fs(&["control", "cr4"])), "CR4");
        // `EFER` has no field in `LongModeState`: the caller keeps the processor's value.
        assert_eq!(cpu.efer, fs(&["control", "efer"]), "EFER");
        let cs = state.cs_cache.expect("CS cache");
        assert_eq!(
            cache_fields(
                state.cs_selector.expect("CS selector"),
                cs.base,
                cs.limit,
                cs.access,
                cs.flags
            ),
            oracle_segment(json, "cs"),
            "CS"
        );
        for (name, selector, cache) in [
            ("ds", state.ds_selector, state.ds_cache.expect("DS cache")),
            ("es", state.es_selector, state.es_cache.expect("ES cache")),
            (
                "ss",
                state.ss_selector.expect("SS selector"),
                state.ss_cache.expect("SS cache"),
            ),
        ] {
            assert_eq!(
                cache_fields(
                    selector,
                    u64::from(cache.base),
                    cache.limit,
                    cache.access,
                    cache.flags
                ),
                oracle_segment(json, name),
                "{name}"
            );
        }
        for (name, selector, base, cache) in [
            (
                "fs",
                state.fs_selector,
                state.fs_base,
                state.fs_cache.expect("FS cache"),
            ),
            (
                "gs",
                state.gs_selector,
                state.gs_base,
                state.gs_cache.expect("GS cache"),
            ),
        ] {
            assert_eq!(
                cache_fields(selector, base, cache.limit, cache.access, cache.flags),
                oracle_segment(json, name),
                "{name}"
            );
        }
        let gdtr = state.gdtr.expect("GDTR");
        let idtr = state.idtr.expect("IDTR");
        for (name, value) in [
            ("gdtr_base", gdtr.base),
            ("gdtr_limit", u64::from(gdtr.limit)),
            ("idtr_base", idtr.base),
            ("idtr_limit", u64::from(idtr.limit)),
        ] {
            assert_eq!(value, fs(&["tables", name]), "{name}");
        }
    }

    #[test]
    fn connected_sipi_to_long_mode_matches_kvm_oracle() {
        use crate::state::LongModeState;

        let json = STEP5_JSON;
        // The oracle entered the image as an AP does after SIPI 0x08 and stopped at the HLT.
        assert_eq!(oracle_num(json, &["entry", "cs_selector"]), 0x0800);
        assert_eq!(oracle_num(json, &["entry", "rip"]), 0);
        assert!(json.contains("\"stop_reason\":\"guest_hlt\""));
        assert!(json.contains("\"exits\":[{\"kind\":\"hlt\"}]"));
        assert_eq!(STEP5_IMAGE.len(), 0xD000);
        let mut ram = vec![0u8; RAM_STEP5];
        ram[STEP5_ENTRY_GPA..STEP5_ENTRY_GPA + STEP5_IMAGE.len()].copy_from_slice(STEP5_IMAGE);

        // Real mode from the SIPI entry up to the `MOV CR0` with PE and the protected far jump
        // (14 instructions of the trampoline: the `JAE` over the spin loop is taken).
        let mut real = RealModeCpu::at_cs_ip(0x0800, 0, 0x6000_0010, oracle_signature(json));
        // The entry is the one a SIPI with vector 0x08 gives (CS = 0x0800, base 0x8000, IP = 0).
        let after_sipi = RealModeCpu::after_sipi(0x08, 0x6000_0010, oracle_signature(json));
        assert_eq!(real.mode.cs.selector, after_sipi.mode.cs.selector);
        assert_eq!(real.mode.cs.base, after_sipi.mode.cs.base);
        assert_eq!(real.mode.eip, after_sipi.mode.eip);
        let run = real.run(&mut mem(&mut ram), 100);
        assert_eq!(run.steps, 14, "real-mode part of the trampoline");
        assert_eq!(
            run.stop,
            Some(RealModeRefusal::NotRealMode { cr0: real.mode.cr0 })
        );

        // Protected mode, paging, IA-32e, the compatibility-mode jump through the cell: 14 + 1
        // instructions, then the model stops before the first 64-bit instruction.
        let mut cpu = ProtectedCpu::from_real_entry(&real).expect("protected entry");
        let run = cpu.run(&mut mem(&mut ram), 100);
        assert_eq!(run.steps, 15, "protected part of the trampoline");
        assert_eq!(
            run.stop,
            Some(ProtectedRefusal::Mode64TargetEntered { rip: 0xB000 })
        );
        // The oracle executed the HLT at the target: its RIP is one past it.
        compare_state_cr2(&cpu, &ram, json, 1, 0, 0);

        // The memory blocks of the other runs: the cell, GDTR and IDTR descriptors, EFER and jump
        // cells, the five page tables.
        let lines: Vec<&str> = STEP5_BLOCKS.lines().collect();
        assert_eq!(lines.len(), 7);
        for (line, (expected_gpa, size)) in lines.iter().zip([
            (0x8100usize, 128usize),
            (0x8400, 16),
            (0x10000, 64),
            (0x11000, 64),
            (0x12000, 64),
            (0x13000, 64),
            (0x14000, 64),
        ]) {
            // Every run reached the same final state (the blocks differ only in the result block).
            assert!(line.contains("\"stop_reason\":\"guest_hlt\""));
            assert_eq!(
                line[line.find("\"final_state\"").unwrap()..]
                    .split("\"result_block_gpa\"")
                    .next(),
                json[json.find("\"final_state\"").unwrap()..]
                    .split("\"result_block_gpa\"")
                    .next()
            );
            let (gpa, bytes) = oracle_result_block(line);
            assert_eq!(gpa as usize, expected_gpa);
            assert_eq!(bytes.len(), size);
            assert_eq!(
                &ram[expected_gpa..expected_gpa + size],
                &bytes[..],
                "block {gpa:#x}"
            );
        }

        // The state the long-mode interpreter continues from (S6-16b) against the same oracle.
        let state = cpu
            .into_long_mode(&LongModeState::default())
            .expect("long mode state at the target");
        compare_long_mode_state(&cpu, &state, json);
    }

    // ---- connected test: the complete S6-14a path with the CPUID of the target (S6-14o) -----
    //
    // Files in docs/reports/data/stage6/s6-14o/ (make-images.py, run-oracle.sh): an image written
    // from the instruction forms of S6-14a rows 1..8 (WBINVD, the real-mode far jump, LOCK BTS,
    // CALL verify_cpu with its CPUID tests, LIDT/LGDT, MOV CR0, the 16-bit and 32-bit far jumps,
    // BT/JAE, MOV CR4/CR3, the conditional EFER write, MOV CR0 with PG, JMP [rip+disp]) entered at
    // 0800:0000, run by kvm_runner with `--cpuid-profile od-x86-64-v1-neutral` (the guest CPUID table
    // is exactly the five leaves of `CpuidProfile::OdX8664V1Neutral`). The model runs `RealModeCpu`
    // with that profile up to `NotRealMode`, `ProtectedCpu::from_real_entry`, `ProtectedCpu::run`
    // to `Mode64TargetEntered` and `into_long_mode`; every compared value comes from the oracle.

    const RAM_S6_14O: usize = 0x16000;
    const S6_14O_IMAGE: &[u8] =
        include_bytes!("../../../docs/reports/data/stage6/s6-14o/a-full-s6-14a-path-0800-0000.bin");
    const S6_14O_JSON: &str = include_str!(
        "../../../docs/reports/data/stage6/s6-14o/oracle-a-full-s6-14a-path-0800-0000.json"
    );
    const S6_14O_BLOCKS: &str = include_str!(
        "../../../docs/reports/data/stage6/s6-14o/oracle-a-full-s6-14a-path-0800-0000-blocks.jsonl"
    );

    #[test]
    fn connected_full_s6_14a_path_matches_kvm_oracle_with_cpuid_target() {
        use crate::cpuid::CpuidProfile;
        use crate::real_mode_exec::CpuidSource;
        use crate::state::LongModeState;

        let json = S6_14O_JSON;
        assert_eq!(oracle_num(json, &["entry", "cs_selector"]), 0x0800);
        assert_eq!(oracle_num(json, &["entry", "rip"]), 0);
        assert!(json.contains("\"stop_reason\":\"guest_hlt\""));
        assert!(json.contains("\"exits\":[{\"kind\":\"hlt\"}]"));
        // The guest CPUID of the oracle is the target table: leaf 1 EAX (the EDX signature after
        // INIT) 0x600, and the five leaves only.
        assert_eq!(oracle_signature(json), 0x600);
        assert_eq!(json.matches("\"function\":").count(), 5);
        assert_eq!(S6_14O_IMAGE.len(), 0x14000 - 0x8000);
        let mut ram = vec![0u8; RAM_S6_14O];
        ram[0x8000..0x8000 + S6_14O_IMAGE.len()].copy_from_slice(S6_14O_IMAGE);

        // Rows 1..5 in real mode: 11 (row 1) + 44 (verify_cpu, by hand: 13 + 5 + 4 (the vendor
        // dispatcher: CMP EBX,'Auth'/JNE, CMP EBX,'Genu'/JNE, both JNE taken) + 5 + 4 + 5 + 5 +
        // 3) + 2 (row 3) + 5 (row 4) + 1 (the far jump of row 5, executed by the CR0.PE = 1
        // path) instructions; the next step is refused as `NotRealMode` (the protected state is
        // handed over to `ProtectedCpu`).
        let mut real = RealModeCpu::at_cs_ip(0x0800, 0, 0x6000_0010, oracle_signature(json));
        real.cpuid = CpuidSource::Profile(CpuidProfile::OdX8664V1Neutral);
        let after_sipi = RealModeCpu::after_sipi(0x08, 0x6000_0010, oracle_signature(json));
        assert_eq!(real.mode.cs.selector, after_sipi.mode.cs.selector);
        assert_eq!(real.mode.cs.base, after_sipi.mode.cs.base);
        assert_eq!(real.mode.eip, after_sipi.mode.eip);
        let mut trace = Vec::new();
        let stop = loop {
            let ip = real.mode.eip;
            match real.step(&mut mem(&mut ram)) {
                Ok(_) => trace.push(ip),
                Err(stop) => break stop,
            }
            assert!(trace.len() < 200, "runaway");
        };
        assert_eq!(stop, RealModeRefusal::NotRealMode { cr0: real.mode.cr0 });
        assert_eq!(trace.len(), 63, "rows 1..5");
        // The path: the spin loop and the verify_cpu failure tail are not taken; the call goes to
        // verify_cpu (0x80), returns to 0x27, JNE at 0x2a is not taken.
        assert!(!trace.contains(&0x1A) && !trace.contains(&0x4C));
        assert_eq!(&trace[10..12], &[0x24, 0x80]);
        assert_eq!(&trace[55..57], &[0x27, 0x2A]);
        assert_eq!(&trace[57..], &[0x2C, 0x32, 0x38, 0x3B, 0x41, 0x44]);
        // The vendor dispatcher of verify_cpu: CMP EBX,'Auth' at 0xb2 and JNE at 0xb9 (taken to
        // 0xd2), CMP EBX,'Genu' at 0xd2 and JNE at 0xd9 (taken to 0x12c); the AMD and Intel
        // branches (0xbb..0xd1, 0xdb..0x12b) are not executed.
        let at = |ip: u32| trace.iter().position(|&x| x == ip).expect("on the path");
        assert_eq!(&trace[at(0xB2)..at(0xB2) + 4], &[0xB2, 0xB9, 0xD2, 0xD9]);
        assert_eq!(trace[at(0xD9) + 1], 0x12C);
        assert!(
            trace
                .iter()
                .all(|&ip| !(0xBB..=0xD1).contains(&ip) && !(0xDB..0x12C).contains(&ip))
        );
        // verify_cpu returned zero (the profile's success path): the JNE at 0x2a was not taken
        // (EAX itself is overwritten by row 4).
        assert_eq!(real.mode.cr0, 0x0005_0033);

        // Protected mode, paging, IA-32e, the compatibility-mode jump through the cell.
        let mut cpu = ProtectedCpu::from_real_entry(&real).expect("protected entry");
        let run = cpu.run(&mut mem(&mut ram), 100);
        assert_eq!(
            run.stop,
            Some(ProtectedRefusal::Mode64TargetEntered { rip: 0xB000 })
        );
        // 6 (SS, ADD ESP, DS, ES, FS, GS) + 2 (BT, JAE) + 2 (CR4) + 2 (CR3) + 2 (ECX,
        // RDMSR) + 2 (CMP, JNE) + 3 (EAX, EDX, WRMSR) + 2 (CR0 with PG) + 1 (row 7) + 1 (row 8).
        assert_eq!(run.steps, 23, "rows 6..8");
        // The oracle executed the HLT at the target: its RIP is one past it.
        compare_state_cr2(&cpu, &ram, json, 1, 0, 0);

        // The memory blocks of the other runs: the header cells (LOCK cell with bit 0 set by the
        // LOCK BTS), the GDTR and IDTR descriptors, the stack the verify_cpu pushes went to, the
        // four page tables with their A bits.
        let lines: Vec<&str> = S6_14O_BLOCKS.lines().collect();
        assert_eq!(lines.len(), 6);
        for (line, (expected_gpa, size)) in lines.iter().zip([
            (0x8400usize, 128usize),
            (0x9F80, 128),
            (0x10000, 64),
            (0x11000, 64),
            (0x12000, 64),
            (0x13000, 128),
        ]) {
            assert!(line.contains("\"stop_reason\":\"guest_hlt\""));
            assert_eq!(
                line[line.find("\"final_state\"").unwrap()..]
                    .split("\"result_block_gpa\"")
                    .next(),
                json[json.find("\"final_state\"").unwrap()..]
                    .split("\"result_block_gpa\"")
                    .next()
            );
            let (gpa, bytes) = oracle_result_block(line);
            assert_eq!(gpa as usize, expected_gpa);
            assert_eq!(bytes.len(), size);
            let mut model = ram[expected_gpa..expected_gpa + size].to_vec();
            if expected_gpa == 0x13000 {
                // The one known difference (as in S6-14n, "the model does not walk the target"):
                // the oracle executes the HLT at 0xB000, so its fetch walk sets A in PTE(0xB)
                // (0xB003 -> 0xB023); the model stops before the fetch and leaves 0xB003. Every
                // other byte of the table is compared as it is.
                assert_eq!(model[8 * 0xB..8 * 0xB + 8], 0xB003u64.to_le_bytes());
                assert_eq!(bytes[8 * 0xB..8 * 0xB + 8], 0xB023u64.to_le_bytes());
                model[8 * 0xB] |= 0x20;
            }
            assert_eq!(model, bytes, "block {gpa:#x}");
        }

        let state = cpu
            .into_long_mode(&LongModeState::default())
            .expect("long mode state at the target");
        compare_long_mode_state(&cpu, &state, json);
    }

    #[test]
    fn s6_14o_cpuid_seen_by_the_guest_through_kvm_equals_the_neutral_profile() {
        use crate::cpuid::CpuidProfile;
        // The probe image (make-images.py, image b) stores EAX..EDX of CPUID for twelve leaves.
        let json = include_str!(
            "../../../docs/reports/data/stage6/s6-14o/oracle-b-cpuid-probe-0800-0000-profile.json"
        );
        assert!(json.contains("\"stop_reason\":\"guest_hlt\""));
        let (gpa, bytes) = oracle_result_block(json);
        assert_eq!(gpa, 0x8200);
        assert_eq!(bytes.len(), 192);
        let leaves = [
            0u32,
            1,
            2,
            4,
            7,
            0xB,
            0x4000_0000,
            0x8000_0000,
            0x8000_0001,
            0x8000_0002,
            0x8000_0008,
            0x8000_0009,
        ];
        let reg = |i: usize, r: usize| {
            u32::from_le_bytes(
                bytes[16 * i + 4 * r..16 * i + 4 * r + 4]
                    .try_into()
                    .unwrap(),
            )
        };
        // The five leaves of the table: exactly the profile, nothing adjusted by KVM.
        for leaf in [0u32, 1, 0x8000_0000, 0x8000_0001, 0x8000_0008] {
            let i = leaves.iter().position(|l| *l == leaf).unwrap();
            let want = CpuidProfile::OdX8664V1Neutral.query(leaf, 0);
            assert_eq!(
                [reg(i, 0), reg(i, 1), reg(i, 2), reg(i, 3)],
                [want.eax, want.ebx, want.ecx, want.edx],
                "leaf {leaf:#x}"
            );
        }
        // Leaves outside the table are KVM's: the data of leaf 1 for leaves above the basic
        // maximum (2, 4, 7, 0xB, 0x40000000, 0x80000009), zeros for 0x80000002. The model refuses
        // such leaves (S6-14g), the connected path never executes them.
        let leaf1 = [reg(1, 0), reg(1, 1), reg(1, 2), reg(1, 3)];
        for leaf in [2u32, 4, 7, 0xB, 0x4000_0000, 0x8000_0009] {
            let i = leaves.iter().position(|l| *l == leaf).unwrap();
            assert_eq!(
                [reg(i, 0), reg(i, 1), reg(i, 2), reg(i, 3)],
                leaf1,
                "{leaf:#x}"
            );
        }
        let i = leaves.iter().position(|l| *l == 0x8000_0002).unwrap();
        assert_eq!([reg(i, 0), reg(i, 1), reg(i, 2), reg(i, 3)], [0; 4]);
    }
}
