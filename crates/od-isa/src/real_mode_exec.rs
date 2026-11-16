//! Execution of the first instruction group of the AP trampoline in real mode (S6-14d).
//!
//! Scope is the closed set of forms from the S6-14a profile (CPU_CONTRACT.md, section
//! "S6-14d"), block 1 of the trampoline at blob offsets `0x1000..0x1024`:
//!
//! | bytes      | instruction            |
//! |------------|------------------------|
//! | `fa`       | `CLI`                  |
//! | `ea o o s s` | `JMP ptr16:16` (far)   |
//! | `8c c8`    | `MOV AX, CS`           |
//! | `8e d8`    | `MOV DS, AX`           |
//! | `8e c0`    | `MOV ES, AX`           |
//! | `8e d0`    | `MOV SS, AX`           |
//!
//! Everything else (`WBINVD`, `LOCK BTS`, `JAE`/`PAUSE`, `CALL`, `HLT`, the padding `NOP`,
//! any prefix byte, any other ModRM form) is an explicit [`RealModeRefusal::Unsupported`]
//! with the address and the bytes, and the state is not changed. Those belong to the
//! following slices (S6-14e and later).
//!
//! State: [`RealModeCpu`] wraps the S6-14c [`RealModeState`] (segments, EIP, EFLAGS, CR0)
//! and adds only the eight 32-bit general-purpose registers; `LongModeState` is not used
//! and not changed. Operand and address size are 16 bits (no override prefix is admitted).
//! Memory access here is instruction fetch only, through CS (`CS.base + IP`, offset
//! limit-checked), with no paging and no A20 or wrap-around modelling.
//!
//! Sources: Intel SDM Vol. 2 (`CLI`, `JMP`, `MOV` to/from segment registers) and Vol. 3A
//! section 12.1.4 (real-mode segment base = selector << 4, from memory). That loading a
//! segment register in real mode keeps the limit is also from memory; its section number
//! is not checked.

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

/// Decoded instruction of the first group.
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
}

/// Byte source for instruction fetch: physical (= linear, no paging) address.
pub trait RealModeMemory {
    /// One byte at `linear`, or `None` if the address is not readable.
    fn read_u8(&self, linear: u32) -> Option<u8>;
}

/// Memory made of one byte image placed at `base`; everything else is unreadable.
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
}

/// Why a step did not execute. The processor state is unchanged in every case.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RealModeRefusal {
    /// The instruction is outside the S6-14d forms. `bytes` are the (up to
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
    /// The far-jump target offset is greater than the CS limit.
    TargetBeyondLimit { offset: u32, limit: u32 },
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
            _ => Err(self.unsupported(mem)),
        }
    }

    /// Decodes and executes one instruction. On refusal the state is not changed.
    pub fn step<M: RealModeMemory>(&mut self, mem: &M) -> Result<RealInstruction, RealModeRefusal> {
        if self.mode.cr0 & CR0_PE != 0 {
            return Err(RealModeRefusal::NotRealMode { cr0: self.mode.cr0 });
        }
        let (insn, len) = self.decode(mem)?;
        // Checked before any state change (a refusal changes nothing).
        if let RealInstruction::JmpFar { offset, .. } = insn
            && u32::from(offset) > self.mode.cs.limit
        {
            return Err(RealModeRefusal::TargetBeyondLimit {
                offset: u32::from(offset),
                limit: self.mode.cs.limit,
            });
        }
        let shadow_before = self.mov_ss_shadow;
        let next_eip = self.mode.eip.wrapping_add(len);
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

    /// Executes up to `max_steps` instructions, stopping at the first refusal.
    pub fn run<M: RealModeMemory>(&mut self, mem: &M, max_steps: u64) -> RealRun {
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
        let m = mem(0x1010, &image);
        assert_eq!(cpu.step(&m), Ok(RealInstruction::Cli));
        assert_eq!(cpu.mode.eflags, 0x0000_0002);
        assert_eq!(cpu.mode.eip, 0x0011);
        // CLI with IF already clear leaves the other flags alone.
        cpu.mode.eip = 0x10;
        cpu.mode.eflags = 0x0000_0003; // CF = 1
        assert_eq!(cpu.step(&m), Ok(RealInstruction::Cli));
        assert_eq!(cpu.mode.eflags, 0x0000_0003);
    }

    #[test]
    fn far_jump_loads_selector_base_and_offset() {
        // ea 34 12 78 56: jmp 0x5678:0x1234. By hand: base = 0x5678 << 4 = 0x56780,
        // EIP = 0x1234, limit kept.
        let mut cpu = RealModeCpu::at_cs_ip(0x0200, 0x0000, 0, 0);
        let image = [0xEA, 0x34, 0x12, 0x78, 0x56];
        let m = mem(0x2000, &image);
        let before = cpu;
        assert_eq!(
            cpu.step(&m),
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
        let m = mem(0x1001, &image);
        cpu.step(&m).unwrap();
        assert_eq!(cpu.mode.cs.selector, 0);
        assert_eq!(cpu.mode.cs.base, 0);
        assert_eq!(cpu.mode.eip, 0x1008);
    }

    #[test]
    fn far_jump_beyond_cs_limit_is_refused_without_state_change() {
        let mut cpu = RealModeCpu::at_cs_ip(0x0100, 0x0000, 0, 0);
        cpu.mode.cs.limit = 0x0FFF;
        let image = [0xEA, 0x00, 0x10, 0x00, 0x03]; // jmp 0x0300:0x1000
        let m = mem(0x1000, &image);
        let before = cpu;
        assert_eq!(
            cpu.step(&m),
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
        let m = mem(0x9F000, &image);
        assert_eq!(cpu.step(&m), Ok(RealInstruction::MovAxFromCs));
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
            let m = mem(0x1000, &bytes);
            assert_eq!(cpu.step(&m), Ok(RealInstruction::MovSegFromAx(seg)));
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
        let m = mem(0x1000, &image);
        cpu.step(&m).unwrap();
        assert!(cpu.mov_ss_shadow, "armed by MOV SS");
        cpu.step(&m).unwrap();
        assert!(!cpu.mov_ss_shadow, "ended after the next instruction");
        // MOV DS does not arm it.
        let mut cpu = RealModeCpu::at_cs_ip(0x0100, 0, 0, 0);
        let m2 = mem(0x1000, &[0x8E, 0xD8]);
        cpu.step(&m2).unwrap();
        assert!(!cpu.mov_ss_shadow);
        // MOV SS directly after MOV SS keeps it armed (its own shadow).
        let mut cpu = RealModeCpu::at_cs_ip(0x0100, 0, 0, 0);
        let m3 = mem(0x1000, &[0x8E, 0xD0, 0x8E, 0xD0]);
        cpu.step(&m3).unwrap();
        cpu.step(&m3).unwrap();
        assert!(cpu.mov_ss_shadow);
    }

    #[test]
    fn whole_block_linked_bytes_by_hand() {
        // Entry 0000:1000 as linked; the block begins at 0x1000 (the image here starts at
        // 0x1000, WBINVD omitted: 0f 09 is a separate later slice).
        // Hand calculation: cli -> IF clear; jmp 0:0x1008 -> CS 0, EIP 0x1008;
        // mov ax,cs -> AX = 0; ds/es/ss = 0, base 0. Stops at hlt (0x1010).
        let mut cpu = RealModeCpu::at_cs_ip(0, 0x1000, 0, 0x0003_06A9);
        // The jump to 0:0x1008 skips the two padding bytes; the first image byte is the CLI
        // then the 5-byte jump (offset 1..5), padding at 6..7.
        let m = mem(0x1000, &BLOCK1_LINKED);
        let run = cpu.run(&m, 100);
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
            (&[0x0F, 0x09], "WBINVD"),
            (
                &[0x66, 0xF0, 0x0F, 0xBA, 0x2E, 0x18, 0x50, 0x00],
                "LOCK BTS",
            ),
            (&[0x73, 0x04], "JAE"),
            (&[0xF3, 0x90], "PAUSE"),
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
        ];
        for (bytes, name) in cases {
            let mut cpu = RealModeCpu::at_cs_ip(0x0123, 0x0040, 0, 0x1234);
            cpu.gpr[EAX] = 0x1111_2222;
            let before = cpu;
            let m = mem(0x1270, bytes);
            let r = cpu.step(&m);
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
        let m = mem(0x1000, &[0xFA]);
        let before = cpu;
        assert_eq!(
            cpu.step(&m),
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
        let m = mem(0, &image);
        let before = cpu;
        assert_eq!(
            cpu.step(&m),
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
        let m = mem(0, &image);
        assert_eq!(cpu.step(&m), Ok(RealInstruction::MovAxFromCs));
        assert_eq!(cpu.mode.eip, 0x10000);
        assert_eq!(cpu.gpr[EAX], 0x1234_0000);
        let after = cpu;
        assert_eq!(
            cpu.step(&m),
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
        let m = mem(0x5000, &[0xFA]); // the instruction at 0x1000 is not in memory
        let before = cpu;
        assert_eq!(
            cpu.step(&m),
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
        let m = mem(base, image);
        let run = cpu.run(&m, 100);
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
}
