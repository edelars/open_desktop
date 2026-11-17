//! Execution of block 6 of the AP trampoline in 32-bit protected mode up to, but excluding, the
//! `MOV CR0, EAX` that enables paging (S6-14k, S6-14l).
//!
//! Scope is the closed set of forms of the S6-14a profile row 6 (CPU_CONTRACT.md, sections
//! "S6-14a", "S6-14k" and "S6-14l"), blob offsets `0x2000..0x2062`, entered from the block 5 far
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
//! | `0f 22 c0`                    | `MOV CR0, EAX`: always refused (below)   |
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
//! written value is ignored and `LMA` keeps its value, which is 0 here because long mode is never
//! entered (observed in the oracle: KVM drops it without a fault). No other MSR exists.
//!
//! Paging is not modelled. `MOV CR0, EAX` with `PG = 1` is [`ProtectedRefusal::PagingNotModelled`]
//! (the state is not changed); any other CR0 write is [`ProtectedRefusal::Cr0WriteUnmodelled`].
//! `MOV CR3, EAX` stores the value (no translation, no PDPTR load, no TLB); `MOV CR4, EAX`
//! stores a value whose bits above bit 10 are clear and refuses any other as
//! [`ProtectedRefusal::Cr4WriteUnmodelled`] (a model refusal, not a hardware effect: per the SDM,
//! from memory, a reserved or unsupported CR4 bit is `#GP(0)`, bits 11.. name features
//! (`UMIP`, `LA57`, `VMXE`, `PCIDE`, `OSXSAVE`, ...) that the profile does not enable, and the
//! model does not gate bits 0..10 by CPUID).
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
//! physical = linear (no paging) through [`RealModeMemory`].
//!
//! Sources: Intel SDM Vol. 2 (`MOV` to/from control and segment registers, `BT`, `ADD`, `CMP`, `Jcc`,
//! `RDMSR`, `WRMSR`), Vol. 3A (IA32_EFER),
//! chapters 2 (control registers) and 5 (protection), all from memory; the observed behaviour of
//! each form is the KVM oracle of `docs/reports/data/stage6/s6-14k/` and `s6-14l/` (native
//! silicon under VMX, except where the S6-14i note on `CR0.ET` applies; `RDMSR`/`WRMSR` of EFER are
//! emulated by KVM, so the oracle shows KVM's guest-visible behaviour).

use crate::legacy_mode::ProtectedSegment;
use crate::real_mode_exec::{
    CR0_PE, EAX, ECX, EDX, ESP, RealModeCpu, RealModeMemory, TableRegister, UNSUPPORTED_BYTES_MAX,
    logic_flags, sub_flags,
};

/// CR0.PG.
pub const CR0_PG: u32 = 1 << 31;
/// CR4 bits 0..10 (`VME` .. `OSXMMEXCPT`): the only bits `MOV CR4, EAX` may set in this slice.
pub const CR4_MODELLED_MASK: u32 = 0x7FF;
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

/// Decoded instruction of block 6 (before the `MOV CR0` that enables paging).
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
    /// `0f 30`: `WRMSR`; only `ECX = 0xC0000080` (EFER) is modelled.
    Wrmsr,
    /// `0f 22 e0`.
    MovCr4Eax,
    /// `0f 22 d8`.
    MovCr3Eax,
    /// `0f 22 c0`: decoded, but always refused when executed (see the module documentation).
    MovCr0Eax,
    /// `b8 imm32`.
    MovEaxImm32(u32),
    /// `b9 imm32`.
    MovEcxImm32(u32),
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

/// Why [`ProtectedCpu::step`] did not execute. The state and memory are unchanged in every case.
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
    /// `WRMSR` of EFER with a bit outside [`EFER_WRITABLE_MASK`] in `EDX:EAX` (`value`): the
    /// hardware raises `#GP(0)`.
    EferWriteReservedBits { eip: u32, value: u64 },
    /// `MOV CR4, EAX` with a bit above bit 10 set (`value` is EAX).
    Cr4WriteUnmodelled { eip: u32, value: u32 },
    /// `MOV CR0, EAX` with `PG = 1`: paging is not modelled (`value` is EAX).
    PagingNotModelled { eip: u32, value: u32 },
    /// `MOV CR0, EAX` with `PG = 0`: CR0 writes in protected mode are not modelled in this slice.
    Cr0WriteUnmodelled { eip: u32, value: u32 },
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

/// Processor state for block 6: 32-bit protected mode, paging off.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProtectedCpu {
    /// EAX, ECX, EDX, EBX, ESP, EBP, ESI, EDI (index constants of `real_mode_exec`).
    pub gpr: [u32; 8],
    pub eip: u32,
    pub eflags: u32,
    pub cr0: u32,
    /// Stored by `MOV CR3, EAX`; never used for a translation.
    pub cr3: u32,
    /// Stored by `MOV CR4, EAX` (bits 0..10 only).
    pub cr4: u32,
    /// `IA32_EFER`, the only MSR: 0 at entry; `WRMSR` stores `SCE`, `LME`, `NXE`. `LMA` is never
    /// changed by `WRMSR` (0 in this slice).
    pub efer: u64,
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
            eflags: real.mode.eflags,
            cr0: real.mode.cr0,
            cr3: 0,
            cr4: 0,
            efer: 0,
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

    fn fetch<M: RealModeMemory>(&self, mem: &M, i: u32) -> Result<u8, ProtectedRefusal> {
        let offset = u64::from(self.eip) + u64::from(i);
        if offset > u64::from(self.cs.limit) {
            return Err(ProtectedRefusal::FetchBeyondLimit {
                eip: self.eip,
                offset,
            });
        }
        let linear = self.cs.base.wrapping_add(offset as u32);
        mem.read_u8(linear)
            .ok_or(ProtectedRefusal::FetchUnreadable {
                eip: self.eip,
                linear,
            })
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
            linear: self.cs.base.wrapping_add(self.eip),
            bytes,
        }
    }

    /// Decodes the instruction at `CS:EIP`; returns it with its length.
    pub fn decode<M: RealModeMemory>(
        &self,
        mem: &M,
    ) -> Result<(ProtectedInstruction, u32), ProtectedRefusal> {
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
                0x30 => Ok((ProtectedInstruction::Wrmsr, 2)),
                0x22 => match self.fetch(mem, 2)? {
                    0xE0 => Ok((ProtectedInstruction::MovCr4Eax, 3)),
                    0xD8 => Ok((ProtectedInstruction::MovCr3Eax, 3)),
                    0xC0 => Ok((ProtectedInstruction::MovCr0Eax, 3)),
                    _ => Err(self.unsupported(mem)),
                },
                _ => Err(self.unsupported(mem)),
            },
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
    fn read_ds_u32<M: RealModeMemory>(&self, mem: &M, disp: u32) -> Result<u32, ProtectedRefusal> {
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
            *slot = mem
                .read_u8(linear)
                .ok_or(ProtectedRefusal::DataUnreadable {
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

    /// Checks a `MOV Sreg` load of `selector` and returns the new cache and, if the descriptor's
    /// accessed bit is clear, the address of the access byte to write. Nothing is changed.
    fn check_segment_load<M: RealModeMemory>(
        &self,
        mem: &M,
        reg: ProtectedSegReg,
        selector: u16,
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
        for (i, byte) in descriptor.iter_mut().enumerate() {
            let at = u64::from(self.gdtr.base) + u64::from(index) + i as u64;
            match u32::try_from(at).ok().and_then(|a| mem.read_u8(a)) {
                Some(v) => *byte = v,
                None => {
                    return Err(refuse(SegmentLoadReason::DescriptorUnreadable {
                        linear: at as u32,
                    }));
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
        let access_at = self.gdtr.base.wrapping_add(index).wrapping_add(5);
        let write_at = if access & 1 == 0 {
            if !mem.can_write(access_at) {
                return Err(refuse(SegmentLoadReason::DescriptorUnwritable {
                    linear: access_at,
                }));
            }
            Some(access_at)
        } else {
            None
        };
        let segment = ProtectedSegment::from_descriptor(selector, descriptor);
        Ok((
            ProtectedSegment {
                access: access | 1,
                ..segment
            },
            write_at,
        ))
    }

    /// Executes one instruction. On a refusal the state and memory are unchanged.
    pub fn step<M: RealModeMemory>(
        &mut self,
        mem: &mut M,
    ) -> Result<ProtectedInstruction, ProtectedRefusal> {
        let (insn, len) = self.decode(mem)?;
        let next_eip = self.eip.wrapping_add(len);
        let shadow_before = self.mov_ss_shadow;
        let mut eip = next_eip;
        match insn {
            ProtectedInstruction::MovSegFromEdx(reg) => {
                let selector = self.gpr[EDX] as u16;
                let (segment, write_at) = self.check_segment_load(mem, reg, selector)?;
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
                let value = self.read_ds_u32(mem, disp)?;
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
                self.gpr[EAX] = self.read_ds_u32(mem, disp)?;
            }
            ProtectedInstruction::MovEdxFromDs(disp) => {
                self.gpr[EDX] = self.read_ds_u32(mem, disp)?;
            }
            ProtectedInstruction::CmpEaxWithDs(disp) => {
                let rhs = self.read_ds_u32(mem, disp)?;
                self.eflags = (self.eflags & !EFLAGS_ARITH) | sub_flags(self.gpr[EAX], rhs);
            }
            ProtectedInstruction::CmpEdxWithDs(disp) => {
                let rhs = self.read_ds_u32(mem, disp)?;
                self.eflags = (self.eflags & !EFLAGS_ARITH) | sub_flags(self.gpr[EDX], rhs);
            }
            ProtectedInstruction::Rdmsr => {
                if self.gpr[ECX] != MSR_EFER {
                    return Err(self.unsupported(mem));
                }
                self.gpr[EAX] = self.efer as u32;
                self.gpr[EDX] = (self.efer >> 32) as u32;
            }
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
                // LMA is set by the processor only: the written bit is ignored.
                self.efer = (value & !EFER_LMA) | (self.efer & EFER_LMA);
            }
            ProtectedInstruction::MovCr4Eax => {
                let value = self.gpr[EAX];
                if value & !CR4_MODELLED_MASK != 0 {
                    return Err(ProtectedRefusal::Cr4WriteUnmodelled {
                        eip: self.eip,
                        value,
                    });
                }
                self.cr4 = value;
            }
            ProtectedInstruction::MovCr3Eax => self.cr3 = self.gpr[EAX],
            ProtectedInstruction::MovCr0Eax => {
                let value = self.gpr[EAX];
                return Err(if value & CR0_PG != 0 {
                    ProtectedRefusal::PagingNotModelled {
                        eip: self.eip,
                        value,
                    }
                } else {
                    ProtectedRefusal::Cr0WriteUnmodelled {
                        eip: self.eip,
                        value,
                    }
                });
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
    use crate::real_mode_exec::{EBP, EBX, EDI, ESI, ImageMemory, ImageMemoryMut, RealModeRefusal};

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
            eflags: 0x2,
            cr0: 0x0005_0033,
            cr3: 0,
            cr4: 0,
            efer: 0,
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
    fn mov_cr0_with_paging_is_refused_without_a_state_change() {
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
            assert_eq!(
                run.stop,
                Some(ProtectedRefusal::PagingNotModelled {
                    eip: 0x2062,
                    value: 0x8005_0033
                })
            );
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
            assert!(matches!(
                run.stop,
                Some(ProtectedRefusal::PagingNotModelled { eip: 0x2062, .. })
            ));
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
            &[0xEA, 0, 0, 0, 0, 0x10, 0],             // far JMP
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
        let mut real = RealModeCpu::at_cs_ip(0, 0, 0x6000_0010, oracle_signature(json));
        let mut ram = vec![0u8; RAM];
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
        assert_eq!(u64::from(cpu.eip) + rip_delta, fs(&["rip"]), "RIP");
        assert_eq!(
            u64::from(cpu.eflags),
            fs(&["rflags"]) & !rflags_ignore,
            "EFLAGS"
        );
        for (name, value) in [("cr0", cpu.cr0), ("cr3", cpu.cr3), ("cr4", cpu.cr4)] {
            assert_eq!(u64::from(value), fs(&["control", name]), "{name}");
        }
        assert_eq!(cpu.efer, fs(&["control", "efer"]), "EFER");
        assert_eq!(fs(&["control", "cr2"]), 0);
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
}
