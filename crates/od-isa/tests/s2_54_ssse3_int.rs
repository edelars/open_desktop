//! S2-54 slice D: the integer SSSE3 forms on XMM registers: PSHUFB, PALIGNR, PABSB/PABSW/PABSD,
//! PSIGNB/PSIGNW/PSIGND, PMADDUBSW, PMULHRSW, PHADDW/PHADDD, PHSUBW/PHSUBD, PHADDSW and PHSUBSW.
//!
//! Every expectation comes from the Intel SDM text read for this task (combined SDM 325462-093US,
//! cdrdv2.intel.com/v1/dl/getContent/671200, Vol. 2B, text through pdftotext): PSHUFB pp. 4-421..4-425,
//! PALIGNR pp. 4-216..4-219, PABSB/PABSW/PABSD pp. 4-177..4-182, PSIGNB/PSIGNW/PSIGND pp. 4-437..4-440,
//! PMADDUBSW pp. 4-310..4-312, PMULHRSW pp. 4-375..4-378, PHADDW/PHADDD pp. 4-294..4-297, PHSUBW/PHSUBD
//! pp. 4-302..4-304, PHADDSW pp. 4-292..4-293, PHSUBSW pp. 4-300..4-301, and Vol. 2A Table 2-21 (Exceptions
//! Type 4, p. 2-29): #UD for CR0.EM = 1, CR4.OSFXSR = 0 or a LOCK prefix, #NM for CR0.TS = 1, #SS(0)/#GP(0)
//! for a non-canonical address, #GP(0) for a 128-bit memory operand that is not 16-byte aligned (the
//! SDM text of every one of these instructions says so; Table 2-21 note 1 exempts only LDDQU, MOVUPD,
//! MOVUPS and the PCMP*STR* forms), no SIMD floating-point exception. The vector tests carry values worked
//! out by hand from the SDM pseudocode; the matrix tests compare against `reference`, a second, literal
//! transcription of that pseudocode on 64-bit integers that shares no code with the model.
//!
//! Not asserted here because the SDM text does not define it or the model does not carry it: the MMX forms
//! (no `66` prefix; they are in `s2_54_ssse3_mmx.rs`, on the MMX register model of S2-33), the CPUID.SSSE3 gate
//! (S2-57), `#AC` (an aligned operand never faults on alignment and an unaligned one is `#GP` first).

use od_isa::execute::{self, Cr4Profile, RunConfig, RunExit, StepOutcome};
use od_isa::long_decode::{self, Operation, Sse3Source, Ssse3IntKind};
use od_isa::state::{SseState, X87State};
use od_isa::{
    AccessContext, AccessKind, ArchitecturalFault, CpuActivity, GuestMemory, LongModeState,
    MemoryError, PrivilegeLevel, Register, StiShadow,
};

const CODE: u64 = 0x2000;
const RAM: usize = 0x10000;
const SENTINEL: u8 = 0xcc;
const FLAGS: u64 = 0x2 | 0xcd5 | (1 << 10);
const MXCSR: u32 = 0x1fa5;

struct Ram {
    bytes: Vec<u8>,
    reads: Vec<(u64, usize)>,
    writes: Vec<(u64, usize)>,
    /// Any data access that touches this page raises the #PF below (an architectural fault of the
    /// backend, which the instruction must not turn into a partial transfer).
    fault_page: Option<u64>,
}

impl Ram {
    fn new(code: &[u8]) -> Self {
        let mut bytes = vec![SENTINEL; RAM];
        bytes[CODE as usize..CODE as usize + code.len()].copy_from_slice(code);
        Self {
            bytes,
            reads: vec![],
            writes: vec![],
            fault_page: None,
        }
    }

    fn check(&self, address: u64, length: usize, access: AccessKind) -> Result<usize, MemoryError> {
        if let Some(page) = self.fault_page
            && address < page + 0x1000
            && address + length as u64 > page
        {
            return Err(MemoryError::Architectural(ArchitecturalFault {
                vector: 14,
                error_code: Some(if access == AccessKind::Write { 2 } else { 0 }),
                linear_address: Some(address.max(page)),
            }));
        }
        let start = usize::try_from(address)
            .ok()
            .filter(|start| start.checked_add(length).is_some_and(|end| end <= RAM));
        start.ok_or(MemoryError::Unmapped { address, access })
    }
}

impl GuestMemory for Ram {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        self.bytes
            .get(address as usize)
            .copied()
            .ok_or(MemoryError::Unmapped {
                address,
                access: AccessKind::Fetch,
            })
    }

    fn read(&mut self, address: u64, _: AccessContext, out: &mut [u8]) -> Result<(), MemoryError> {
        self.reads.push((address, out.len()));
        let start = self.check(address, out.len(), AccessKind::Read)?;
        out.copy_from_slice(&self.bytes[start..start + out.len()]);
        Ok(())
    }

    fn write(&mut self, address: u64, _: AccessContext, data: &[u8]) -> Result<(), MemoryError> {
        self.writes.push((address, data.len()));
        let start = self.check(address, data.len(), AccessKind::Write)?;
        self.bytes[start..start + data.len()].copy_from_slice(data);
        Ok(())
    }

    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        Ok(())
    }
}

fn xmm_pattern(index: usize) -> [u8; 16] {
    core::array::from_fn(|lane| {
        (index as u8).wrapping_mul(0x11) ^ (lane as u8).wrapping_mul(7) ^ 0x35
    })
}

fn memory_pattern(seed: u8) -> [u8; 16] {
    core::array::from_fn(|lane| seed ^ (lane as u8).wrapping_mul(13).wrapping_add(0x80))
}

/// Distinct GPRs, distinct XMM registers, a non-default MXCSR and a flag word with many bits set:
/// any unwanted write is visible in a whole-state comparison.
fn cpu() -> LongModeState {
    let mut cpu = LongModeState {
        rip: CODE,
        rflags: FLAGS,
        x87: Some(X87State::fixture_seed()),
        sse: Some(SseState {
            xmm: core::array::from_fn(xmm_pattern),
            mxcsr: MXCSR,
        }),
        ..LongModeState::default()
    };
    for (index, slot) in cpu.gpr.iter_mut().enumerate() {
        *slot = 0x0101_0101_0101_0101 * (index as u64 + 0x11);
    }
    cpu
}

/// The state that the FP-init CR0/CR4 profile pair admits (CPU running, no interrupt shadow, x87
/// without selectors) plus the given control registers.
fn fp_init_cpu(cr0: u64, cr4: u64) -> LongModeState {
    let mut cpu = cpu();
    cpu.activity = Some(CpuActivity::Running);
    cpu.sti_shadow = Some(StiShadow::Clear);
    cpu.cr0 = Some(cr0);
    cpu.cr4 = Some(cr4);
    cpu
}

fn ctx(privilege: PrivilegeLevel) -> AccessContext {
    AccessContext {
        privilege,
        ..AccessContext::default()
    }
}

fn step(
    _code: &[u8],
    cpu: &mut LongModeState,
    ram: &mut Ram,
    privilege: PrivilegeLevel,
) -> Result<StepOutcome, MemoryError> {
    execute::step(cpu, ram, ctx(privilege))
}

enum Disp {
    None,
    D8(i8),
    D32(i32),
    /// RIP-relative: the absolute target; disp32 is computed from the end of the instruction.
    Rip(u64),
}

/// One memory addressing form: the bytes around the mandatory prefix and opcode, the registers that
/// make the effective address and the effective address that the SDM rules (Vol. 1 3.7.5, Vol. 2A 2.1.5
/// and 2.2.1.6) give for them.
struct Form {
    name: &'static str,
    /// Legacy prefixes before the mandatory prefix (67, null segment overrides).
    extra: &'static [u8],
    /// REX bits W/X/B (R comes from the XMM number); 0 means no REX byte unless R is needed.
    rex: u8,
    mode: u8,
    rm: u8,
    sib: Option<u8>,
    disp: Disp,
    regs: &'static [(Register, u64)],
    address: u64,
}

const FORMS: &[Form] = &[
    Form {
        name: "[rax]",
        extra: &[],
        rex: 0,
        mode: 0,
        rm: 0,
        sib: None,
        disp: Disp::None,
        regs: &[(Register::Rax, 0x8010)],
        address: 0x8010,
    },
    Form {
        name: "[rbx]",
        extra: &[],
        rex: 0,
        mode: 0,
        rm: 3,
        sib: None,
        disp: Disp::None,
        regs: &[(Register::Rbx, 0x8120)],
        address: 0x8120,
    },
    Form {
        name: "[rsp] (SIB)",
        extra: &[],
        rex: 0,
        mode: 0,
        rm: 4,
        sib: Some(0x24),
        disp: Disp::None,
        regs: &[(Register::Rsp, 0x8230)],
        address: 0x8230,
    },
    Form {
        name: "[rbp+0x10]",
        extra: &[],
        rex: 0,
        mode: 1,
        rm: 5,
        sib: None,
        disp: Disp::D8(0x10),
        regs: &[(Register::Rbp, 0x8320)],
        address: 0x8330,
    },
    Form {
        name: "[rbp-0x20]",
        extra: &[],
        rex: 0,
        mode: 1,
        rm: 5,
        sib: None,
        disp: Disp::D8(-0x20),
        regs: &[(Register::Rbp, 0x8360)],
        address: 0x8340,
    },
    Form {
        name: "[rbx+0x120] disp32",
        extra: &[],
        rex: 0,
        mode: 2,
        rm: 3,
        sib: None,
        disp: Disp::D32(0x120),
        regs: &[(Register::Rbx, 0x8000)],
        address: 0x8120,
    },
    Form {
        name: "[rax+rcx*4+0x10]",
        extra: &[],
        rex: 0,
        mode: 1,
        rm: 4,
        sib: Some(0x88),
        disp: Disp::D8(0x10),
        regs: &[(Register::Rax, 0x8000), (Register::Rcx, 4)],
        address: 0x8020,
    },
    Form {
        name: "[r8] (REX.B)",
        extra: &[],
        rex: 0x01,
        mode: 0,
        rm: 0,
        sib: None,
        disp: Disp::None,
        regs: &[(Register::R8, 0x8440)],
        address: 0x8440,
    },
    Form {
        name: "[r13+0x10] (REX.B, disp8)",
        extra: &[],
        rex: 0x01,
        mode: 1,
        rm: 5,
        sib: None,
        disp: Disp::D8(0x10),
        regs: &[(Register::R13, 0x8650)],
        address: 0x8660,
    },
    Form {
        name: "[r12] (REX.B, SIB)",
        extra: &[],
        rex: 0x01,
        mode: 0,
        rm: 4,
        sib: Some(0x24),
        disp: Disp::None,
        regs: &[(Register::R12, 0x8570)],
        address: 0x8570,
    },
    Form {
        name: "[rbx+r9*8+0x20] (REX.X)",
        extra: &[],
        rex: 0x02,
        mode: 1,
        rm: 4,
        sib: Some(0xcb),
        disp: Disp::D8(0x20),
        regs: &[(Register::Rbx, 0x8100), (Register::R9, 2)],
        address: 0x8130,
    },
    Form {
        name: "[disp32] (SIB, no base)",
        extra: &[],
        rex: 0,
        mode: 0,
        rm: 4,
        sib: Some(0x25),
        disp: Disp::D32(0x8800),
        regs: &[],
        address: 0x8800,
    },
    Form {
        name: "[r9*4+disp32] (REX.X, no base)",
        extra: &[],
        rex: 0x02,
        mode: 0,
        rm: 4,
        sib: Some(0x8d),
        disp: Disp::D32(0x8700),
        regs: &[(Register::R9, 4)],
        address: 0x8710,
    },
    Form {
        name: "[rip+disp32] forward",
        extra: &[],
        rex: 0,
        mode: 0,
        rm: 5,
        sib: None,
        disp: Disp::Rip(0x8900),
        regs: &[],
        address: 0x8900,
    },
    Form {
        name: "[rip+disp32] backward",
        extra: &[],
        rex: 0,
        mode: 0,
        rm: 5,
        sib: None,
        disp: Disp::Rip(0x1000),
        regs: &[],
        address: 0x1000,
    },
    Form {
        name: "[rip+disp32] with REX.B (ignored)",
        extra: &[],
        rex: 0x01,
        mode: 0,
        rm: 5,
        sib: None,
        disp: Disp::Rip(0x8910),
        regs: &[(Register::R13, 0x8f00)],
        address: 0x8910,
    },
    Form {
        name: "[rax] REX.W (ignored)",
        extra: &[],
        rex: 0x08,
        mode: 0,
        rm: 0,
        sib: None,
        disp: Disp::None,
        regs: &[(Register::Rax, 0x8a10)],
        address: 0x8a10,
    },
    Form {
        name: "[eax] 67 truncates to 32 bits",
        extra: &[0x67],
        rex: 0,
        mode: 0,
        rm: 0,
        sib: None,
        disp: Disp::None,
        regs: &[(Register::Rax, 0xdead_beef_0000_8a40)],
        address: 0x8a40,
    },
    Form {
        name: "[eax+ecx*4+0x10] 67 wraps in 32 bits",
        extra: &[0x67],
        rex: 0,
        mode: 1,
        rm: 4,
        sib: Some(0x88),
        disp: Disp::D8(0x10),
        regs: &[
            (Register::Rax, 0xffff_ffff_ffff_fff0),
            (Register::Rcx, 0x1_0000_0004),
        ],
        address: 0x10,
    },
    Form {
        name: "[rax] cs override (ignored)",
        extra: &[0x2e],
        rex: 0,
        mode: 0,
        rm: 0,
        sib: None,
        disp: Disp::None,
        regs: &[(Register::Rax, 0x8b00)],
        address: 0x8b00,
    },
    Form {
        name: "[rax] ss override (ignored)",
        extra: &[0x36],
        rex: 0,
        mode: 0,
        rm: 0,
        sib: None,
        disp: Disp::None,
        regs: &[(Register::Rax, 0x8b10)],
        address: 0x8b10,
    },
    Form {
        name: "[rax] ds override (ignored)",
        extra: &[0x3e],
        rex: 0,
        mode: 0,
        rm: 0,
        sib: None,
        disp: Disp::None,
        regs: &[(Register::Rax, 0x8b20)],
        address: 0x8b20,
    },
    Form {
        name: "[rax] es override (ignored)",
        extra: &[0x26],
        rex: 0,
        mode: 0,
        rm: 0,
        sib: None,
        disp: Disp::None,
        regs: &[(Register::Rax, 0x8b30)],
        address: 0x8b30,
    },
    Form {
        name: "[rax] 67 + cs override",
        extra: &[0x67, 0x2e],
        rex: 0,
        mode: 0,
        rm: 0,
        sib: None,
        disp: Disp::None,
        regs: &[(Register::Rax, 0x8b40)],
        address: 0x8b40,
    },
];

fn setup(form: &Form) -> LongModeState {
    let mut cpu = cpu();
    for (register, value) in form.regs {
        cpu.gpr[*register as usize] = *value;
    }
    cpu
}

fn gp0() -> StepOutcome {
    StepOutcome::Fault(ArchitecturalFault {
        vector: 13,
        error_code: Some(0),
        linear_address: None,
    })
}

fn ss0() -> StepOutcome {
    StepOutcome::Fault(ArchitecturalFault {
        vector: 12,
        error_code: Some(0),
        linear_address: None,
    })
}

fn ud() -> StepOutcome {
    StepOutcome::Fault(ArchitecturalFault {
        vector: 6,
        error_code: None,
        linear_address: None,
    })
}

// ---- the SSSE3 forms under test ----

struct Kind {
    name: &'static str,
    kind: Ssse3IntKind,
    /// Opcode bytes after the mandatory `66` (and REX): `0F 38 xx` or `0F 3A 0F`.
    opcode: [u8; 3],
}

impl Kind {
    fn immediate(&self) -> bool {
        self.kind == Ssse3IntKind::Palignr
    }
}

const KINDS: [Kind; 16] = [
    Kind {
        name: "pshufb",
        kind: Ssse3IntKind::Pshufb,
        opcode: [0x0f, 0x38, 0x00],
    },
    Kind {
        name: "phaddw",
        kind: Ssse3IntKind::Phaddw,
        opcode: [0x0f, 0x38, 0x01],
    },
    Kind {
        name: "phaddd",
        kind: Ssse3IntKind::Phaddd,
        opcode: [0x0f, 0x38, 0x02],
    },
    Kind {
        name: "phaddsw",
        kind: Ssse3IntKind::Phaddsw,
        opcode: [0x0f, 0x38, 0x03],
    },
    Kind {
        name: "pmaddubsw",
        kind: Ssse3IntKind::Pmaddubsw,
        opcode: [0x0f, 0x38, 0x04],
    },
    Kind {
        name: "phsubw",
        kind: Ssse3IntKind::Phsubw,
        opcode: [0x0f, 0x38, 0x05],
    },
    Kind {
        name: "phsubd",
        kind: Ssse3IntKind::Phsubd,
        opcode: [0x0f, 0x38, 0x06],
    },
    Kind {
        name: "phsubsw",
        kind: Ssse3IntKind::Phsubsw,
        opcode: [0x0f, 0x38, 0x07],
    },
    Kind {
        name: "psignb",
        kind: Ssse3IntKind::Psignb,
        opcode: [0x0f, 0x38, 0x08],
    },
    Kind {
        name: "psignw",
        kind: Ssse3IntKind::Psignw,
        opcode: [0x0f, 0x38, 0x09],
    },
    Kind {
        name: "psignd",
        kind: Ssse3IntKind::Psignd,
        opcode: [0x0f, 0x38, 0x0a],
    },
    Kind {
        name: "pmulhrsw",
        kind: Ssse3IntKind::Pmulhrsw,
        opcode: [0x0f, 0x38, 0x0b],
    },
    Kind {
        name: "pabsb",
        kind: Ssse3IntKind::Pabsb,
        opcode: [0x0f, 0x38, 0x1c],
    },
    Kind {
        name: "pabsw",
        kind: Ssse3IntKind::Pabsw,
        opcode: [0x0f, 0x38, 0x1d],
    },
    Kind {
        name: "pabsd",
        kind: Ssse3IntKind::Pabsd,
        opcode: [0x0f, 0x38, 0x1e],
    },
    Kind {
        name: "palignr",
        kind: Ssse3IntKind::Palignr,
        opcode: [0x0f, 0x3a, 0x0f],
    },
];

fn kind_named(name: &str) -> &'static Kind {
    KINDS.iter().find(|kind| kind.name == name).unwrap()
}

/// `66 [REX] 0F 38 xx /r` (or `0F 3A 0F /r ib`) with a register source (mod = 3).
fn reg_code(kind: &Kind, destination: u8, source: u8, immediate: u8) -> Vec<u8> {
    let mut code = vec![0x66];
    let rex = (if destination >= 8 { 4 } else { 0 }) | (if source >= 8 { 1 } else { 0 });
    if rex != 0 {
        code.push(0x40 | rex);
    }
    code.extend_from_slice(&kind.opcode);
    code.push(0xc0 | ((destination & 7) << 3) | (source & 7));
    if kind.immediate() {
        code.push(immediate);
    }
    code
}

/// `66 [prefixes] [REX] 0F 38 xx /r [SIB] [disp] [ib]` for one addressing form.
fn mem_code(kind: &Kind, form: &Form, reg: u8, immediate: u8) -> Vec<u8> {
    let mut code = form.extra.to_vec();
    code.push(0x66);
    let rex = form.rex | if reg >= 8 { 0x04 } else { 0 };
    if rex != 0 {
        code.push(0x40 | rex);
    }
    code.extend_from_slice(&kind.opcode);
    code.push((form.mode << 6) | ((reg & 7) << 3) | form.rm);
    if let Some(sib) = form.sib {
        code.push(sib);
    }
    match form.disp {
        Disp::None => {}
        Disp::D8(value) => code.push(value as u8),
        Disp::D32(value) => code.extend_from_slice(&value.to_le_bytes()),
        Disp::Rip(target) => {
            // RIP-relative is relative to the end of the whole instruction, immediate included.
            let end = CODE + code.len() as u64 + 4 + u64::from(kind.immediate());
            code.extend_from_slice(&((target.wrapping_sub(end)) as i32).to_le_bytes());
        }
    }
    if kind.immediate() {
        code.push(immediate);
    }
    code
}

// ---- the reference model: the SDM pseudocode, transcribed literally on i64 ----

fn lane(value: &[u8; 16], width: usize, index: usize, signed: bool) -> i64 {
    let mut raw = 0u64;
    for byte in 0..width {
        raw |= u64::from(value[index * width + byte]) << (8 * byte);
    }
    if signed {
        let shift = 64 - 8 * width as u32;
        ((raw << shift) as i64) >> shift
    } else {
        raw as i64
    }
}

fn put(out: &mut [u8; 16], width: usize, index: usize, value: i64) {
    for byte in 0..width {
        out[index * width + byte] = (value >> (8 * byte)) as u8;
    }
}

fn saturate_to_signed_word(value: i64) -> i64 {
    value.clamp(-32768, 32767)
}

fn reference(kind: Ssse3IntKind, dest: &[u8; 16], src: &[u8; 16], imm8: u8) -> [u8; 16] {
    let mut out = [0u8; 16];
    match kind {
        Ssse3IntKind::Pshufb => {
            for destpos in 0..16 {
                let shufbyte = src[destpos];
                if shufbyte & 0x80 == 0x80 {
                    out[destpos] = 0;
                } else {
                    out[destpos] = dest[usize::from(shufbyte & 0x0f)];
                }
            }
        }
        Ssse3IntKind::Palignr => {
            let (high, low) = (u128::from_le_bytes(*dest), u128::from_le_bytes(*src));
            let shift = u32::from(imm8) * 8;
            let result = if shift >= 256 {
                0
            } else if shift == 0 {
                low
            } else if shift < 128 {
                (low >> shift) | (high << (128 - shift))
            } else if shift == 128 {
                high
            } else {
                high >> (shift - 128)
            };
            out = result.to_le_bytes();
        }
        Ssse3IntKind::Pabsb | Ssse3IntKind::Pabsw | Ssse3IntKind::Pabsd => {
            let width = match kind {
                Ssse3IntKind::Pabsb => 1,
                Ssse3IntKind::Pabsw => 2,
                _ => 4,
            };
            for index in 0..16 / width {
                put(&mut out, width, index, lane(src, width, index, true).abs());
            }
        }
        Ssse3IntKind::Psignb | Ssse3IntKind::Psignw | Ssse3IntKind::Psignd => {
            let width = match kind {
                Ssse3IntKind::Psignb => 1,
                Ssse3IntKind::Psignw => 2,
                _ => 4,
            };
            for index in 0..16 / width {
                let control = lane(src, width, index, true);
                let value = lane(dest, width, index, true);
                let result = if control < 0 {
                    -value
                } else if control == 0 {
                    0
                } else {
                    value
                };
                put(&mut out, width, index, result);
            }
        }
        Ssse3IntKind::Pmaddubsw => {
            for index in 0..8 {
                let sum = lane(dest, 1, 2 * index + 1, false) * lane(src, 1, 2 * index + 1, true)
                    + lane(dest, 1, 2 * index, false) * lane(src, 1, 2 * index, true);
                put(&mut out, 2, index, saturate_to_signed_word(sum));
            }
        }
        Ssse3IntKind::Pmulhrsw => {
            for index in 0..8 {
                let temp = ((lane(dest, 2, index, true) * lane(src, 2, index, true)) >> 14) + 1;
                // DEST[15:0] := temp[16:1]
                put(&mut out, 2, index, temp >> 1);
            }
        }
        Ssse3IntKind::Phaddw
        | Ssse3IntKind::Phsubw
        | Ssse3IntKind::Phaddsw
        | Ssse3IntKind::Phsubsw => {
            let pair = |first: i64, second: i64| match kind {
                Ssse3IntKind::Phaddw => first + second,
                Ssse3IntKind::Phsubw => first - second,
                Ssse3IntKind::Phaddsw => saturate_to_signed_word(first + second),
                _ => saturate_to_signed_word(first - second),
            };
            for index in 0..4 {
                put(
                    &mut out,
                    2,
                    index,
                    pair(
                        lane(dest, 2, 2 * index, true),
                        lane(dest, 2, 2 * index + 1, true),
                    ),
                );
                put(
                    &mut out,
                    2,
                    4 + index,
                    pair(
                        lane(src, 2, 2 * index, true),
                        lane(src, 2, 2 * index + 1, true),
                    ),
                );
            }
        }
        Ssse3IntKind::Phaddd | Ssse3IntKind::Phsubd => {
            let pair = |first: i64, second: i64| {
                if kind == Ssse3IntKind::Phaddd {
                    first + second
                } else {
                    first - second
                }
            };
            for index in 0..2 {
                put(
                    &mut out,
                    4,
                    index,
                    pair(
                        lane(dest, 4, 2 * index, true),
                        lane(dest, 4, 2 * index + 1, true),
                    ),
                );
                put(
                    &mut out,
                    4,
                    2 + index,
                    pair(
                        lane(src, 4, 2 * index, true),
                        lane(src, 4, 2 * index + 1, true),
                    ),
                );
            }
        }
    }
    out
}

// ---- helpers for lane literals ----

fn bytes(values: [u8; 16]) -> [u8; 16] {
    values
}

fn words(values: [u16; 8]) -> [u8; 16] {
    let mut out = [0u8; 16];
    for (index, value) in values.into_iter().enumerate() {
        out[2 * index..2 * index + 2].copy_from_slice(&value.to_le_bytes());
    }
    out
}

fn dwords(values: [u32; 4]) -> [u8; 16] {
    let mut out = [0u8; 16];
    for (index, value) in values.into_iter().enumerate() {
        out[4 * index..4 * index + 4].copy_from_slice(&value.to_le_bytes());
    }
    out
}

fn counting(start: u8) -> [u8; 16] {
    core::array::from_fn(|index| start + index as u8)
}

/// Runs the instruction with the destination in xmm0, once with the source in xmm1 and once with the
/// source in memory at 0x8010, and checks that nothing but xmm0 and RIP changed. Returns both results.
fn run_vector(kind: &Kind, dest: [u8; 16], src: [u8; 16], immediate: u8) -> [[u8; 16]; 2] {
    let mut results = [[0u8; 16]; 2];
    for (variant, result) in results.iter_mut().enumerate() {
        let code = if variant == 0 {
            reg_code(kind, 0, 1, immediate)
        } else {
            mem_code(kind, &FORMS[0], 0, immediate)
        };
        let mut cpu = cpu();
        cpu.sse.as_mut().unwrap().xmm[0] = dest;
        if variant == 0 {
            cpu.sse.as_mut().unwrap().xmm[1] = src;
        } else {
            cpu.gpr[Register::Rax as usize] = 0x8010;
        }
        let mut ram = Ram::new(&code);
        ram.bytes[0x8010..0x8020].copy_from_slice(&src);
        let mut expected = cpu.clone();
        expected.rip = CODE + code.len() as u64;
        let outcome = step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring0);
        assert_eq!(
            outcome,
            Ok(StepOutcome::Advanced),
            "{} variant {variant}",
            kind.name
        );
        *result = cpu.sse.as_ref().unwrap().xmm[0];
        expected.sse.as_mut().unwrap().xmm[0] = *result;
        assert_eq!(
            cpu, expected,
            "{} variant {variant}: only xmm0 and RIP change",
            kind.name
        );
        assert_eq!(ram.writes, [], "{}", kind.name);
        assert_eq!(ram.reads.len(), variant, "{}", kind.name);
    }
    results
}

fn assert_vector(name: &str, dest: [u8; 16], src: [u8; 16], immediate: u8, expected: [u8; 16]) {
    let kind = kind_named(name);
    let [from_register, from_memory] = run_vector(kind, dest, src, immediate);
    assert_eq!(
        from_register, expected,
        "{name} register form, imm {immediate:#x}"
    );
    assert_eq!(
        from_memory, expected,
        "{name} memory form, imm {immediate:#x}"
    );
    // The independent transcription of the pseudocode gives the same bytes.
    assert_eq!(
        reference(kind.kind, &dest, &src, immediate),
        expected,
        "{name} reference"
    );
}

// ---- vectors worked out by hand from the SDM pseudocode ----

#[test]
fn pshufb_indexes_the_destination_by_the_low_nibble_and_zeroes_on_bit_7() {
    // DEST bytes 10..1F. Control byte c: bit 7 set -> 0, else DEST[c & 0F]. The destination is the table
    // and the source is the control (SDM: shufbyte := SRC.byte[destpos]; TEMP := DEST).
    let controls = bytes([
        0x00, 0x0f, 0x80, 0xff, 0x7f, 0x10, 0x8f, 0x01, 0x05, 0x0a, 0x81, 0x70, 0x1f, 0x0c, 0x03,
        0x90,
    ]);
    assert_vector(
        "pshufb",
        counting(0x10),
        controls,
        0,
        bytes([
            0x10, 0x1f, 0x00, 0x00, 0x1f, 0x10, 0x00, 0x11, 0x15, 0x1a, 0x00, 0x10, 0x1f, 0x1c,
            0x13, 0x00,
        ]),
    );
    // Identity, reversal and every index with bit 7 set.
    assert_vector("pshufb", counting(0x40), counting(0), 0, counting(0x40));
    let reverse: [u8; 16] = core::array::from_fn(|index| 15 - index as u8);
    assert_vector(
        "pshufb",
        counting(0x40),
        reverse,
        0,
        reverse.map(|index| 0x40 + index),
    );
    assert_vector("pshufb", counting(0x40), [0x80; 16], 0, [0; 16]);
    assert_vector("pshufb", counting(0x40), [0xff; 16], 0, [0; 16]);
    // Only the low four bits select: 0x70 and 0x7f are indexes 0 and 15, not an error.
    assert_vector("pshufb", counting(0x40), [0x70; 16], 0, [0x40; 16]);
    assert_vector("pshufb", counting(0x40), [0x7f; 16], 0, [0x4f; 16]);
}

#[test]
fn palignr_shifts_the_dest_src_concatenation_right_by_whole_bytes() {
    // SRC = 00..0F is the low half and DEST = 10..1F the high half of the 32-byte composite, so the
    // result for shift n is the bytes n..n+15 of 00..1F, with zeros beyond byte 31.
    let (dest, src) = (counting(0x10), counting(0x00));
    for shift in 0..=40u8 {
        let expected: [u8; 16] = core::array::from_fn(|index| {
            let byte = index as u8 + shift;
            if byte < 32 { byte } else { 0 }
        });
        assert_vector("palignr", dest, src, shift, expected);
    }
    assert_vector("palignr", dest, src, 0, src);
    assert_vector("palignr", dest, src, 16, dest);
    assert_vector(
        "palignr",
        dest,
        src,
        31,
        bytes([0x1f, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
    );
    // Counts above 31 (up to 255, the immediate is unsigned) give zero.
    for shift in [32u8, 33, 64, 127, 128, 129, 254, 255] {
        assert_vector("palignr", dest, src, shift, [0; 16]);
    }
}

#[test]
fn pabs_is_the_unsigned_magnitude_and_the_most_negative_value_maps_to_itself() {
    assert_vector(
        "pabsb",
        [0; 16],
        bytes([
            0x00, 0x01, 0x7f, 0x80, 0x81, 0xff, 0xfe, 0xc0, 0x02, 0x03, 0x40, 0x41, 0xdf, 0xe0,
            0xfd, 0x7e,
        ]),
        0,
        bytes([
            0x00, 0x01, 0x7f, 0x80, 0x7f, 0x01, 0x02, 0x40, 0x02, 0x03, 0x40, 0x41, 0x21, 0x20,
            0x03, 0x7e,
        ]),
    );
    assert_vector(
        "pabsw",
        [0xaa; 16],
        words([
            0x0000, 0x0001, 0x7fff, 0x8000, 0x8001, 0xffff, 0xfffe, 0xc000,
        ]),
        0,
        words([
            0x0000, 0x0001, 0x7fff, 0x8000, 0x7fff, 0x0001, 0x0002, 0x4000,
        ]),
    );
    assert_vector(
        "pabsd",
        [0x55; 16],
        dwords([0x8000_0000, 0x8000_0001, 0xffff_ffff, 0x7fff_ffff]),
        0,
        dwords([0x8000_0000, 0x7fff_ffff, 0x0000_0001, 0x7fff_ffff]),
    );
}

#[test]
fn psign_negates_zeroes_or_keeps_by_the_sign_of_the_source_including_zero_and_int_min() {
    assert_vector(
        "psignb",
        bytes([
            0x01, 0x02, 0x03, 0x80, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
            0x0f, 0x10,
        ]),
        bytes([
            0xff, 0x00, 0x01, 0xff, 0x80, 0x7f, 0x00, 0x81, 0x00, 0x00, 0x01, 0x01, 0xff, 0xff,
            0xfe, 0x7e,
        ]),
        0,
        // -1, 0, keep, -(-128) = -128, -5, keep, 0, -8, 0, 0, keep, keep, -13, -14, -15, keep.
        bytes([
            0xff, 0x00, 0x03, 0x80, 0xfb, 0x06, 0x00, 0xf8, 0x00, 0x00, 0x0b, 0x0c, 0xf3, 0xf2,
            0xf1, 0x10,
        ]),
    );
    assert_vector(
        "psignw",
        words([
            0x0001, 0x8000, 0x7fff, 0x1234, 0x0005, 0x0006, 0x0007, 0x0008,
        ]),
        words([
            0xffff, 0xffff, 0x0000, 0x8000, 0x0001, 0x7fff, 0x0000, 0x0001,
        ]),
        0,
        // -1, -(-32768) = -32768 (wraps), 0, -0x1234 = 0xEDCC, keep, keep, 0, keep.
        words([
            0xffff, 0x8000, 0x0000, 0xedcc, 0x0005, 0x0006, 0x0000, 0x0008,
        ]),
    );
    assert_vector(
        "psignd",
        dwords([0x0000_0001, 0x8000_0000, 0x1234_5678, 0x7fff_ffff]),
        dwords([0xffff_ffff, 0xffff_ffff, 0x0000_0000, 0x8000_0000]),
        0,
        dwords([0xffff_ffff, 0x8000_0000, 0x0000_0000, 0x8000_0001]),
    );
    // A zero control byte zeroes even the most negative destination value; a source of the most
    // negative value is negative.
    assert_vector("psignb", [0x80; 16], [0x00; 16], 0, [0; 16]);
    assert_vector("psignb", [0x80; 16], [0x80; 16], 0, [0x80; 16]);
    assert_vector("psignb", [0x7f; 16], [0x80; 16], 0, [0x81; 16]);
}

#[test]
fn pmaddubsw_multiplies_unsigned_destination_bytes_by_signed_source_bytes_and_saturates() {
    let dest = bytes([
        0xff, 0xff, 0x80, 0x80, 0x01, 0x02, 0x00, 0xff, 0xff, 0xff, 0xff, 0x01, 0xff, 0xff, 0x00,
        0x00,
    ]);
    let src = bytes([
        0x7f, 0x7f, 0x80, 0x80, 0xff, 0xfe, 0x00, 0x80, 0x80, 0x80, 0x7f, 0x7f, 0x7f, 0x80, 0x12,
        0x34,
    ]);
    // 255*127*2 = 64770 -> 7FFF; 128*(-128)*2 = -32768 (fits) -> 8000; 1*(-1)+2*(-2) = -5;
    // 0 + 255*(-128) = -32640 = 8080; 255*(-128)*2 = -65280 -> 8000; 255*127+1*127 = 32512 = 7F00;
    // 255*127+255*(-128) = -255 = FF01; 0.
    assert_vector(
        "pmaddubsw",
        dest,
        src,
        0,
        words([
            0x7fff, 0x8000, 0xfffb, 0x8080, 0x8000, 0x7f00, 0xff01, 0x0000,
        ]),
    );
}

#[test]
fn pmulhrsw_rounds_the_scaled_product_to_nearest_with_ties_up() {
    // (a*b >> 14) + 1 >> 1 is round-half-up of a*b / 2^15.
    assert_vector(
        "pmulhrsw",
        words([
            0x0001, 0x0001, 0x0001, 0x0001, 0xffff, 0xffff, 0x8000, 0x8000,
        ]),
        words([
            0x4000, 0x2000, 0x6000, 0x0001, 0x4000, 0x6000, 0x8000, 0x7fff,
        ]),
        0,
        // 0.5 -> 1; 0.25 -> 0; 0.75 -> 1; tiny -> 0; -0.5 -> 0 (ties go up); -0.75 -> -1;
        // 0x8000*0x8000 = 2^30 -> 0x8000 (wraps: temp = 0x10001); -32768*32767 -> -32767 = 0x8001.
        words([
            0x0001, 0x0000, 0x0001, 0x0000, 0x0000, 0xffff, 0x8000, 0x8001,
        ]),
    );
    assert_vector(
        "pmulhrsw",
        words([0x4000, 0x7fff, 0, 0, 0, 0, 0, 0]),
        words([0x4000, 0x7fff, 0, 0, 0, 0, 0, 0]),
        0,
        // 0x4000*0x4000 / 2^15 = 8192; 32767*32767 / 32768 = 32766.00003 -> 32766.
        words([0x2000, 0x7ffe, 0, 0, 0, 0, 0, 0]),
    );
}

#[test]
fn horizontal_adds_and_subtracts_pair_the_destination_low_and_the_source_high() {
    let dest = words([
        0x0001, 0x0002, 0x0003, 0x0004, 0x7fff, 0x0001, 0x8000, 0xffff,
    ]);
    let src = words([
        0x0010, 0x0020, 0x0030, 0x0040, 0xffff, 0x0001, 0x7fff, 0x7fff,
    ]);
    // Wrapping add: 7FFF+1 -> 8000, 8000+FFFF = 17FFF -> 7FFF; source pairs follow the destination pairs.
    assert_vector(
        "phaddw",
        dest,
        src,
        0,
        words([
            0x0003, 0x0007, 0x8000, 0x7fff, 0x0030, 0x0070, 0x0000, 0xfffe,
        ]),
    );
    // Saturating add: 7FFF+1 -> 7FFF, -32768 + -1 -> 8000, 7FFF+7FFF -> 7FFF.
    assert_vector(
        "phaddsw",
        dest,
        src,
        0,
        words([
            0x0003, 0x0007, 0x7fff, 0x8000, 0x0030, 0x0070, 0x0000, 0x7fff,
        ]),
    );
    // Wrapping subtract: first minus second of each pair.
    assert_vector(
        "phsubw",
        dest,
        src,
        0,
        words([
            0xffff, 0xffff, 0x7ffe, 0x8001, 0xfff0, 0xfff0, 0xfffe, 0x0000,
        ]),
    );
    let dest = words([
        0x7fff, 0xffff, 0x8000, 0x0001, 0x0005, 0x0003, 0x8000, 0x7fff,
    ]);
    let src = words([
        0x0000, 0x8000, 0x7fff, 0x8000, 0x0001, 0x0001, 0xffff, 0x8000,
    ]);
    // Saturating subtract: 7FFF-(-1) = 32768 -> 7FFF; -32768-1 -> 8000; 5-3; -32768-32767 -> 8000;
    // 0-(-32768) -> 7FFF; 32767-(-32768) -> 7FFF; 1-1; -1-(-32768) = 32767.
    assert_vector(
        "phsubsw",
        dest,
        src,
        0,
        words([
            0x7fff, 0x8000, 0x0002, 0x8000, 0x7fff, 0x7fff, 0x0000, 0x7fff,
        ]),
    );
    // The same inputs without saturation wrap.
    assert_vector(
        "phsubw",
        dest,
        src,
        0,
        words([
            0x8000, 0x7fff, 0x0002, 0x0001, 0x8000, 0xffff, 0x0000, 0x7fff,
        ]),
    );
    let dest = dwords([0x0000_0001, 0x0000_0002, 0x7fff_ffff, 0x0000_0001]);
    let src = dwords([0xffff_ffff, 0x0000_0001, 0x8000_0000, 0x8000_0000]);
    assert_vector(
        "phaddd",
        dest,
        src,
        0,
        dwords([0x0000_0003, 0x8000_0000, 0x0000_0000, 0x0000_0000]),
    );
    assert_vector(
        "phsubd",
        dest,
        src,
        0,
        dwords([0xffff_ffff, 0x7fff_fffe, 0xffff_fffe, 0x0000_0000]),
    );
}

// ---- generated data against the reference model ----

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// Mode 0 random bytes; 1 extreme bytes; 2 extreme words; 3 extreme dwords; 4 shuffle controls.
    fn vector(&mut self, mode: usize) -> [u8; 16] {
        const T1: [u8; 10] = [0x00, 0x01, 0x7f, 0x80, 0x81, 0xff, 0x02, 0xfe, 0x40, 0xc0];
        const T2: [u16; 12] = [
            0x0000, 0x0001, 0x7fff, 0x8000, 0x8001, 0xffff, 0x4000, 0xc000, 0x2000, 0x0002, 0xfffe,
            0x3fff,
        ];
        const T3: [u32; 8] = [
            0,
            1,
            0x7fff_ffff,
            0x8000_0000,
            0x8000_0001,
            0xffff_ffff,
            0x4000_0000,
            0xc000_0000,
        ];
        let mut out = [0u8; 16];
        for chunk in out.chunks_mut(8) {
            chunk.copy_from_slice(&self.next().to_le_bytes());
        }
        match mode {
            1 => out.map(|byte| T1[usize::from(byte) % 10]),
            2 => words(core::array::from_fn(|index| {
                T2[usize::from(u16::from_le_bytes([out[2 * index], out[2 * index + 1]])) % 12]
            })),
            3 => dwords(core::array::from_fn(|index| {
                let word = u32::from_le_bytes(out[4 * index..4 * index + 4].try_into().unwrap());
                T3[(word % 8) as usize]
            })),
            4 => out.map(|byte| byte & 0x8f),
            _ => out,
        }
    }
}

#[test]
fn generated_and_boundary_data_agree_with_the_reference_for_every_form() {
    let mut rng = Rng(0x54d0_0000_0000_0001);
    let mut cases = 0;
    for kind in &KINDS {
        for iteration in 0..600usize {
            let (dest, src) = (rng.vector(iteration % 5), rng.vector((iteration / 5) % 5));
            let immediate = (rng.next() % 40) as u8;
            let expected = reference(kind.kind, &dest, &src, immediate);
            let [from_register, from_memory] = run_vector(kind, dest, src, immediate);
            assert_eq!(
                from_register, expected,
                "{} {dest:02x?} {src:02x?} {immediate}",
                kind.name
            );
            assert_eq!(
                from_memory, expected,
                "{} {dest:02x?} {src:02x?} {immediate}",
                kind.name
            );
            cases += 2;
        }
    }
    assert_eq!(cases, 16 * 600 * 2);
}

#[test]
fn every_register_pair_of_the_register_forms_changes_only_the_destination() {
    let mut cases = 0;
    for kind in &KINDS {
        for destination in 0..16u8 {
            for source in 0..16u8 {
                let immediate = (destination * 3 + source) % 37;
                let code = reg_code(kind, destination, source, immediate);
                let mut cpu = cpu();
                let mut ram = Ram::new(&code);
                let mut expected = cpu.clone();
                let xmm = &cpu.sse.as_ref().unwrap().xmm;
                let result = reference(
                    kind.kind,
                    &xmm[usize::from(destination)],
                    &xmm[usize::from(source)],
                    immediate,
                );
                expected.sse.as_mut().unwrap().xmm[usize::from(destination)] = result;
                expected.rip = CODE + code.len() as u64;
                let label = format!("{} xmm{destination}, xmm{source}", kind.name);
                let decoded =
                    long_decode::decode(&mut Ram::new(&code), CODE, AccessContext::default())
                        .unwrap();
                assert_eq!(decoded.length as usize, code.len(), "{label}");
                assert_eq!(
                    decoded.operation,
                    Operation::Ssse3Int {
                        kind: kind.kind,
                        destination,
                        source: Sse3Source::Register(source),
                        immediate: if kind.immediate() { immediate } else { 0 },
                    },
                    "{label}"
                );
                assert_eq!(
                    step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring0),
                    Ok(StepOutcome::Advanced),
                    "{label}"
                );
                assert_eq!(cpu, expected, "{label}");
                assert!(ram.reads.is_empty() && ram.writes.is_empty(), "{label}");
                cases += 1;
            }
        }
    }
    assert_eq!(cases, 16 * 256);
}

#[test]
fn every_addressing_form_loads_the_source_for_all_sixteen_instructions() {
    let mut cases = 0;
    for (kind_index, kind) in KINDS.iter().enumerate() {
        for (form_index, form) in FORMS.iter().enumerate() {
            // Every XMM register is exercised over the matrix (REX.R selects XMM8..15).
            let xmm = ((kind_index * 5 + form_index * 3) % 16) as u8;
            let immediate = (kind_index * 7 + form_index) as u8 % 36;
            let code = mem_code(kind, form, xmm, immediate);
            let source = memory_pattern((form_index * 16 + kind_index) as u8);
            let mut cpu = setup(form);
            let mut ram = Ram::new(&code);
            let address = form.address as usize;
            ram.bytes[address..address + 16].copy_from_slice(&source);
            let (cpu_before, ram_before) = (cpu.clone(), ram.bytes.clone());
            let label = format!("{} {} xmm{xmm}", kind.name, form.name);
            let decoded =
                long_decode::decode(&mut Ram::new(&code), CODE, AccessContext::default()).unwrap();
            assert_eq!(decoded.length as usize, code.len(), "{label}");
            let Operation::Ssse3Int {
                kind: decoded_kind,
                destination,
                source: Sse3Source::Memory(_),
                immediate: decoded_immediate,
            } = decoded.operation
            else {
                panic!("{label}: {:?}", decoded.operation);
            };
            assert_eq!(
                (decoded_kind, destination, decoded_immediate),
                (kind.kind, xmm, if kind.immediate() { immediate } else { 0 }),
                "{label}"
            );
            assert_eq!(
                step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring0),
                Ok(StepOutcome::Advanced),
                "{label}"
            );
            let mut expected_cpu = cpu_before;
            expected_cpu.rip = CODE + code.len() as u64;
            let dest = expected_cpu.sse.as_ref().unwrap().xmm[usize::from(xmm)];
            expected_cpu.sse.as_mut().unwrap().xmm[usize::from(xmm)] =
                reference(kind.kind, &dest, &source, immediate);
            // Registers, flags, MXCSR, x87, the other XMM registers and all memory are untouched.
            assert_eq!(cpu, expected_cpu, "{label}");
            assert_eq!(ram.bytes, ram_before, "{label}");
            assert_eq!(ram.reads, [(form.address, 16)], "{label}");
            assert_eq!(ram.writes, [], "{label}");
            cases += 1;
        }
    }
    assert_eq!(cases, 16 * FORMS.len());
}

#[test]
fn register_forms_ignore_rex_w_and_null_segment_and_address_overrides() {
    for kind in &KINDS {
        let plain = reg_code(kind, 3, 2, 5);
        let mut reference_cpu = cpu();
        let mut reference_ram = Ram::new(&plain);
        assert_eq!(
            step(
                &plain,
                &mut reference_cpu,
                &mut reference_ram,
                PrivilegeLevel::Ring0
            ),
            Ok(StepOutcome::Advanced)
        );
        for extra in [
            &[0x67u8][..],
            &[0x2e],
            &[0x36],
            &[0x3e],
            &[0x26],
            &[0x67, 0x2e],
        ] {
            let code = [extra, &plain].concat();
            let mut cpu = cpu();
            let mut ram = Ram::new(&code);
            assert_eq!(
                step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring0),
                Ok(StepOutcome::Advanced),
                "{} {extra:02x?}",
                kind.name
            );
            assert_eq!(
                cpu.sse.as_ref().unwrap().xmm,
                reference_cpu.sse.as_ref().unwrap().xmm,
                "{} {extra:02x?}",
                kind.name
            );
            assert_eq!(cpu.rip, CODE + code.len() as u64);
        }
        // REX.W (the SDM operation does not use it) selects the same registers and result.
        let mut code = vec![0x66, 0x48];
        code.extend_from_slice(&plain[1..]);
        let mut cpu = cpu();
        let mut ram = Ram::new(&code);
        assert_eq!(
            step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring0),
            Ok(StepOutcome::Advanced),
            "{} REX.W",
            kind.name
        );
        assert_eq!(
            cpu.sse.as_ref().unwrap().xmm,
            reference_cpu.sse.as_ref().unwrap().xmm,
            "{} REX.W",
            kind.name
        );
    }
}

// ---- exceptions ----

#[test]
fn a_memory_source_that_is_not_16_byte_aligned_is_gp0_for_every_form() {
    for kind in &KINDS {
        for address in [0x8001u64, 0x8002, 0x8004, 0x8008, 0x800f, 0x8011, 0x7ffe] {
            let code = mem_code(kind, &FORMS[0], 2, 1);
            let mut cpu = cpu();
            cpu.gpr[Register::Rax as usize] = address;
            let mut ram = Ram::new(&code);
            let (before, ram_before) = (cpu.clone(), ram.bytes.clone());
            assert_eq!(
                step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring0),
                Ok(gp0()),
                "{} {address:#x}",
                kind.name
            );
            assert_eq!(cpu, before, "{} {address:#x}", kind.name);
            assert_eq!(ram.bytes, ram_before);
            assert!(ram.reads.is_empty() && ram.writes.is_empty());
        }
        // An aligned address is read and the instruction completes.
        let code = mem_code(kind, &FORMS[0], 2, 1);
        let mut cpu = cpu();
        cpu.gpr[Register::Rax as usize] = 0x8010;
        let mut ram = Ram::new(&code);
        assert_eq!(
            step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring0),
            Ok(StepOutcome::Advanced),
            "{}",
            kind.name
        );
    }
}

#[test]
fn at_cpl3_with_the_alignment_check_armed_an_unaligned_source_is_gp0_not_ac_and_an_aligned_one_runs()
 {
    // Physical x86 behaves the same way (oracle, S2-54 slice D): the alignment #GP precedes #AC, and an
    // aligned operand has no alignment fault, so the Type 4 class needs no #AC modeling.
    const AC: u64 = 1 << 18;
    for kind in &KINDS {
        let code = mem_code(kind, &FORMS[0], 1, 3);
        for (address, expected) in [
            (0x8000u64, Ok(StepOutcome::Advanced)),
            (0x8001, Ok(gp0())),
            (0x8008, Ok(gp0())),
        ] {
            let mut cpu = cpu();
            cpu.rflags |= AC;
            cpu.gpr[Register::Rax as usize] = address;
            let mut ram = Ram::new(&code);
            assert_eq!(
                step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring3),
                expected,
                "{} {address:#x}",
                kind.name
            );
        }
    }
}

#[test]
fn non_canonical_addresses_fault_before_alignment_and_before_memory() {
    for kind in &KINDS {
        for (base, label) in [
            (Register::Rax, "rax"),
            (Register::Rbp, "rbp"),
            (Register::Rsp, "rsp"),
        ] {
            for address in [
                0x0000_8000_0000_0000u64,
                0x0000_8000_0000_0001,
                0xffff_7fff_ffff_ffff,
                0x7fff_ffff_ffff_ffff,
            ] {
                // [base] with mod = 0 (rsp needs a SIB byte, rbp mod = 1 disp8 0).
                let modrm_tail: &[u8] = match base {
                    Register::Rax => &[0x18],
                    Register::Rbp => &[0x5d, 0x00],
                    _ => &[0x1c, 0x24],
                };
                let mut code = vec![0x66];
                code.extend_from_slice(&kind.opcode);
                code.extend_from_slice(modrm_tail);
                if kind.immediate() {
                    code.push(0);
                }
                let mut cpu = cpu();
                cpu.gpr[base as usize] = address;
                let mut ram = Ram::new(&code);
                let before = cpu.clone();
                let expected = if base == Register::Rax { gp0() } else { ss0() };
                assert_eq!(
                    step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring0),
                    Ok(expected),
                    "{} {label} {address:#x}",
                    kind.name
                );
                assert_eq!(cpu, before);
                assert!(ram.reads.is_empty());
            }
        }
    }
}

#[test]
fn backend_faults_leave_cpu_and_memory_untouched() {
    for kind in &KINDS {
        let code = mem_code(kind, &FORMS[0], 1, 3);
        let mut cpu = cpu();
        cpu.gpr[Register::Rax as usize] = 0x8010;
        let mut ram = Ram::new(&code);
        ram.fault_page = Some(0x8000);
        let (before, ram_before) = (cpu.clone(), ram.bytes.clone());
        assert_eq!(
            step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring0),
            Ok(StepOutcome::Fault(ArchitecturalFault {
                vector: 14,
                error_code: Some(0),
                linear_address: Some(0x8010),
            })),
            "{}",
            kind.name
        );
        assert_eq!(cpu, before, "{}", kind.name);
        assert_eq!(ram.bytes, ram_before, "{}", kind.name);
    }
}

#[test]
fn lock_is_ud_for_every_form_and_prefix_order() {
    for kind in &KINDS {
        let tail = if kind.immediate() { vec![0x00] } else { vec![] };
        let opcode = kind.opcode;
        let mut codes: Vec<Vec<u8>> = vec![
            // memory forms: LOCK before and after 66, with REX, with RIP-relative
            [&[0xf0, 0x66][..], &opcode, &[0x00], &tail].concat(),
            [&[0x66, 0xf0][..], &opcode, &[0x00], &tail].concat(),
            [&[0xf0, 0x66, 0x48][..], &opcode, &[0x10, 0x20], &tail].concat(),
            [&[0xf0, 0x66][..], &opcode, &[0x05, 0, 0, 0, 0], &tail].concat(),
            // register forms
            [&[0xf0, 0x66][..], &opcode, &[0xc1], &tail].concat(),
            [&[0x66, 0xf0][..], &opcode, &[0xc1], &tail].concat(),
            [&[0x66, 0x44, 0xf0][..], &opcode, &[0xc1], &tail].concat(),
        ];
        codes.push([&[0xf0, 0x2e, 0x66][..], &opcode, &[0x00], &tail].concat());
        for code in codes {
            let mut cpu = cpu();
            let mut ram = Ram::new(&code);
            let (before, ram_before) = (cpu.clone(), ram.bytes.clone());
            let outcome = step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring0);
            assert_eq!(outcome, Ok(ud()), "{} {code:02x?}", kind.name);
            assert_eq!(cpu, before);
            assert_eq!(ram.bytes, ram_before);
            assert!(ram.reads.is_empty() && ram.writes.is_empty());
            assert!(matches!(
                long_decode::decode(&mut Ram::new(&code), CODE, AccessContext::default()),
                Err(long_decode::DecodeError::Invalid { .. })
            ));
        }
    }
}

fn run_fp_init(
    mut cpu: LongModeState,
    ram: &mut Ram,
    privilege: PrivilegeLevel,
) -> (execute::RunResult, LongModeState) {
    let result = execute::run_with_cr4_profile(
        &mut cpu,
        ram,
        RunConfig {
            instruction_budget: 1,
            stop_rip: None,
            context: ctx(privilege),
        },
        Cr4Profile::FpInitLong64V1,
        || false,
    );
    (result, cpu)
}

#[test]
fn cr0_em_ts_and_cr4_osfxsr_gate_the_ssse3_forms_before_any_address_or_memory_check() {
    const CR0_BASE: u64 = 0x8001_0033; // PG | WP | NE | ET | MP | PE: EM = 0, TS = 0
    const CR4_PAE: u64 = 0x20;
    const CR4_OSFXSR: u64 = 0x200;
    let cases: [(u64, u64, Option<u8>); 8] = [
        (CR0_BASE, CR4_PAE | CR4_OSFXSR, None),
        (CR0_BASE | 4, CR4_PAE | CR4_OSFXSR, Some(6)),
        (CR0_BASE | 8, CR4_PAE | CR4_OSFXSR, Some(7)),
        (CR0_BASE | 4 | 8, CR4_PAE | CR4_OSFXSR, Some(6)),
        (CR0_BASE, CR4_PAE, Some(6)),
        (CR0_BASE | 4, CR4_PAE, Some(6)),
        (CR0_BASE | 8, CR4_PAE, Some(6)),
        (CR0_BASE | 4 | 8, CR4_PAE, Some(6)),
    ];
    for kind in &KINDS {
        let codes = [
            (reg_code(kind, 1, 2, 3), true),
            (mem_code(kind, &FORMS[0], 1, 3), false),
        ];
        for (code, register_form) in codes {
            for (cr0, cr4, vector) in cases {
                for address in [0x8000u64, 0x0000_8000_0000_0000, 0x8001] {
                    let mut cpu = fp_init_cpu(cr0, cr4);
                    cpu.gpr[Register::Rax as usize] = address;
                    let mut ram = Ram::new(&code);
                    let (before, ram_before) = (cpu.clone(), ram.bytes.clone());
                    let (result, cpu) = run_fp_init(cpu, &mut ram, PrivilegeLevel::Ring0);
                    let label = format!(
                        "{} {code:02x?} cr0={cr0:#x} cr4={cr4:#x} {address:#x}",
                        kind.name
                    );
                    let fault = |vector, error_code| {
                        RunExit::Fault(ArchitecturalFault {
                            vector,
                            error_code,
                            linear_address: None,
                        })
                    };
                    match vector {
                        // The EM / OSFXSR / TS gate precedes every address check.
                        Some(vector) => {
                            assert_eq!(result.exit, fault(vector, None), "{label}");
                            assert_eq!(result.instructions_executed, 0, "{label}");
                            assert_eq!(cpu, before, "{label}");
                            assert_eq!(ram.bytes, ram_before, "{label}");
                            assert!(ram.reads.is_empty() && ram.writes.is_empty(), "{label}");
                        }
                        None if register_form || address == 0x8000 => {
                            assert_eq!(result.exit, RunExit::InstructionBudgetExhausted, "{label}");
                            assert_eq!(result.instructions_executed, 1, "{label}");
                        }
                        None => assert_eq!(result.exit, fault(13, Some(0)), "{label}"),
                    }
                }
            }
        }
    }
}

#[test]
fn without_sse_state_the_forms_stop_as_unsupported_and_x87_state_is_not_needed() {
    for kind in &KINDS {
        for code in [reg_code(kind, 1, 2, 3), mem_code(kind, &FORMS[0], 1, 3)] {
            // No SSE state: refused without change.
            let mut cpu_a = cpu();
            cpu_a.sse = None;
            let mut ram = Ram::new(&code);
            let before = cpu_a.clone();
            assert_eq!(
                step(&code, &mut cpu_a, &mut ram, PrivilegeLevel::Ring0),
                Ok(StepOutcome::Unsupported {
                    opcode: kind.opcode[2]
                }),
                "{}",
                kind.name
            );
            assert_eq!(cpu_a, before);
            assert!(ram.reads.is_empty() && ram.writes.is_empty());
            // SSE state without x87 state: the forms do not use the x87 state and run.
            let mut cpu_b = cpu();
            cpu_b.x87 = None;
            cpu_b.gpr[Register::Rax as usize] = 0x8000;
            let mut ram = Ram::new(&code);
            assert_eq!(
                step(&code, &mut cpu_b, &mut ram, PrivilegeLevel::Ring0),
                Ok(StepOutcome::Advanced),
                "{}",
                kind.name
            );
        }
    }
}

#[test]
fn neighbouring_encodings_stay_unsupported_without_change() {
    let mut refused: Vec<(Vec<u8>, String)> = vec![];
    for kind in &KINDS {
        let tail: &[u8] = if kind.immediate() { &[0x04] } else { &[] };
        let name = kind.name;
        // F2 / F3 with 66, repeated prefixes, FS and GS (no segment base in this operand model).
        for (prefix, what) in [
            (&[0xf2u8, 0x66][..], "F2 66"),
            (&[0x66, 0xf2], "66 F2"),
            (&[0xf3, 0x66], "F3 66"),
            (&[0x66, 0xf3], "66 F3"),
            (&[0xf2, 0xf3, 0x66], "F2 F3 66"),
            (&[0x66, 0x66], "duplicate 66"),
            (&[0x67, 0x67, 0x66], "duplicate 67"),
            (&[0x2e, 0x3e, 0x66], "two segment overrides"),
            (&[0x64, 0x66], "FS"),
            (&[0x66, 0x65], "GS"),
            (&[0x66, 0x40, 0x40], "duplicate REX"),
            (&[0xf0, 0xf0, 0x66], "duplicate LOCK"),
        ] {
            for modrm in [0x00u8, 0xc1] {
                refused.push((
                    [prefix, &kind.opcode, &[modrm], tail].concat(),
                    format!("{name} {what} modrm {modrm:#x}"),
                ));
            }
        }
    }
    // Neighbouring opcodes of the same maps that are not integer SSSE3.
    for opcode in [0x0cu8, 0x0d, 0x0e, 0x0f, 0x1b, 0x1f] {
        refused.push((
            vec![0x66, 0x0f, 0x38, opcode, 0x00],
            format!("66 0F 38 {opcode:02x}"),
        ));
    }
    for (code, name) in &refused {
        let mut cpu = cpu();
        let mut ram = Ram::new(code);
        let before = cpu.clone();
        let outcome = step(code, &mut cpu, &mut ram, PrivilegeLevel::Ring0);
        assert!(
            matches!(outcome, Ok(StepOutcome::Unsupported { .. })),
            "{name}: {outcome:?}"
        );
        assert_eq!(cpu, before, "{name}");
        assert!(ram.reads.is_empty() && ram.writes.is_empty(), "{name}");
    }
}

#[test]
fn the_decoder_reports_kind_destination_source_immediate_and_length() {
    let decode = |code: &[u8]| {
        long_decode::decode(&mut Ram::new(code), CODE, AccessContext::default()).unwrap()
    };
    // PALIGNR xmm9, xmm12, 0x1f: REX.R and REX.B extend the registers, the immediate is the last byte.
    let instruction = decode(&[0x66, 0x45, 0x0f, 0x3a, 0x0f, 0xcc, 0x1f]);
    assert_eq!(instruction.length, 7);
    assert_eq!(
        instruction.operation,
        Operation::Ssse3Int {
            kind: Ssse3IntKind::Palignr,
            destination: 9,
            source: Sse3Source::Register(12),
            immediate: 0x1f,
        }
    );
    // PSHUFB xmm0, [rax+rcx*4+0x10]: no immediate.
    let instruction = decode(&[0x66, 0x0f, 0x38, 0x00, 0x44, 0x88, 0x10]);
    assert_eq!(instruction.length, 7);
    assert!(matches!(
        instruction.operation,
        Operation::Ssse3Int {
            kind: Ssse3IntKind::Pshufb,
            destination: 0,
            source: Sse3Source::Memory(_),
            immediate: 0,
        }
    ));
    // PABSD xmm15, [rip+0]: 66, REX, three opcode bytes, ModRM and disp32 make 10 bytes.
    let instruction = decode(&[0x66, 0x44, 0x0f, 0x38, 0x1e, 0x3d, 0, 0, 0, 0]);
    assert_eq!(instruction.length, 10);
    assert!(matches!(
        instruction.operation,
        Operation::Ssse3Int {
            kind: Ssse3IntKind::Pabsd,
            destination: 15,
            ..
        }
    ));
    // The opcode map and byte are those of the instruction.
    assert_eq!(instruction.opcode.byte, 0x1e);
}
