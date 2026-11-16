//! Execution of the first two instruction groups of the AP trampoline in real mode
//! (S6-14d, S6-14e).
//!
//! Scope is the closed set of forms from the S6-14a profile (CPU_CONTRACT.md, sections
//! "S6-14d" and "S6-14e"), block 1 of the trampoline at blob offsets `0x1000..0x101e`:
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
//!
//! Everything else (`CALL verify_cpu`, `HLT`, the padding `NOP`, any other prefix byte or
//! prefix order, any other ModRM form, `BT`/`BTR`/`BTC`, unlocked or 16-bit `BTS`) is an
//! explicit [`RealModeRefusal::Unsupported`] with the address and the bytes, and the state is
//! not changed. Those belong to the following slices (S6-14f and later).
//!
//! State: [`RealModeCpu`] wraps the S6-14c [`RealModeState`] (segments, EIP, EFLAGS, CR0)
//! and adds only the eight 32-bit general-purpose registers; `LongModeState` is not used
//! and not changed. Operand and address size are 16 bits except the one `66`-prefixed
//! `LOCK BTS` form (32-bit operand, 16-bit address). Instruction fetch goes through CS
//! (`CS.base + IP`, offset limit-checked), the one data operand (`LOCK BTS`) through DS
//! (`DS.base + disp16`, the whole 4-byte operand limit-checked, no wrap), with no paging and
//! no A20 or wrap-around modelling. There are no caches: `WBINVD` changes nothing visible,
//! and `LOCK` adds no atomicity (a single processor is modelled).
//!
//! Sources: Intel SDM Vol. 2 (`CLI`, `JMP`, `Jcc`, `MOV` to/from segment registers, `BTS`,
//! `PAUSE`, `WBINVD`) and Vol. 3A section 12.1.4 (real-mode segment base = selector << 4),
//! all from memory. That loading a segment register in real mode keeps the limit is also
//! from memory; its section number is not checked.

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

/// Decoded instruction of the first two groups.
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
}

/// Memory for instruction fetch and the one data operand: physical (= linear, no paging)
/// address.
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
    /// The target offset of a far jump or of a taken short jump is greater than the CS limit.
    TargetBeyondLimit { offset: u32, limit: u32 },
    /// The 4-byte data operand of `LOCK BTS` does not fit under the DS limit: `offset` is the
    /// offset of the first byte beyond it (real mode does not wrap the operand).
    DataBeyondLimit { cs: u16, ip: u32, offset: u32 },
    /// A data byte at `linear` is not readable in memory.
    DataUnreadable { cs: u16, ip: u32, linear: u32 },
    /// A data byte at `linear` is not writable in memory.
    DataUnwritable { cs: u16, ip: u32, linear: u32 },
    /// CR0.PE is set: this module executes real mode only.
    NotRealMode { cr0: u32 },
}

/// Result of [`RealModeCpu::run`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RealRun {
    /// Number of instructions executed.
    pub steps: u64,
    /// `None` when `max_steps` was reached, otherwise the refusal that stopped the run.
    pub stop: Option<RealModeRefusal>,
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
                _ => Err(self.unsupported(mem)),
            },
            // The only prefixed form is `66 F0 0F BA /5 ib` with ModRM `2E` (mod 00, rm 110:
            // disp16 only, reg 5 = BTS), in exactly this prefix order. Operand bytes are free.
            0x66 => {
                let modrm_ok = self.fetch(mem, 1)? == 0xF0
                    && self.fetch(mem, 2)? == 0x0F
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
        next_eip.wrapping_add_signed(i32::from(rel)) & 0xFFFF
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
            RealInstruction::Jae { rel } if self.mode.eflags & EFLAGS_CF == 0 => {
                Some(Self::short_target(next_eip, rel))
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
            (&[0xE8, 0x59, 0x00], "CALL"),
            (&[0x66, 0xBC, 0x20, 0x60, 0x00, 0x00], "MOV ESP,imm32"),
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
            (&[0x72, 0x04], "JB"),
            (&[0x74, 0x04], "JE"),
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
            assert_eq!(u64::from(cpu.gpr[i]), get(&["gpr", name]), "{name}");
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
}
