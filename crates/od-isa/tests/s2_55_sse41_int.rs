//! S2-55 slice A: the integer SSE4.1 forms on XMM registers: PTEST, PMOVSXBW/BD/BQ/WD/WQ/DQ,
//! PMOVZXBW/BD/BQ/WD/WQ/DQ, PMULLD, PMULDQ, PMINSB/PMINSD/PMINUW/PMINUD, PMAXSB/PMAXSD/PMAXUW/PMAXUD,
//! PCMPEQQ, PACKUSDW, PBLENDW, PBLENDVB, PINSRB/PINSRD/PINSRQ, PEXTRB/PEXTRW/PEXTRD/PEXTRQ, MOVNTDQA,
//! PHMINPOSUW and MPSADBW.
//!
//! Every expectation comes from the Intel SDM text read for this task (combined SDM 325462-093US,
//! cdrdv2.intel.com/v1/dl/getContent/671200, Vol. 2B, text through pdftotext): PTEST pp. 4-498..4-499,
//! PMOVSX pp. 4-352..4-361, PMOVZX pp. 4-362..4-371, PMULDQ pp. 4-372..4-374, PMULLD pp. 4-387..4-390,
//! PMINSB/PMINSD/PMINUW/PMINUD pp. 4-332..4-349, PMAXSB/PMAXSD/PMAXUW/PMAXUD pp. 4-316..4-331, PCMPEQQ
//! pp. 4-251..4-253, PACKUSDW pp. 4-191..4-195, PBLENDW pp. 4-235..4-237, PBLENDVB pp. 4-231..4-234,
//! PINSRB/PINSRD/PINSRQ pp. 4-305..4-307, PEXTRB/PEXTRD/PEXTRQ pp. 4-286..4-288, PEXTRW pp. 4-289..4-291,
//! MOVNTDQA pp. 4-90..4-91, PHMINPOSUW pp. 4-298..4-299, MPSADBW pp. 4-132..4-139, and Vol. 2A Tables 2-18
//! (Type 1: MOVNTDQA), 2-21 (Type 4: the register-or-m128 forms) and 2-22 (Type 5: PMOVSX/PMOVZX, PINSR*,
//! PEXTR*): #UD for CR0.EM = 1, CR4.OSFXSR = 0 or a LOCK prefix, #NM for CR0.TS = 1, #SS(0)/#GP(0) for a
//! non-canonical address, #GP(0) for a 128-bit memory operand that is not 16-byte aligned (Types 1 and 4
//! only), no SIMD floating-point exception, no MXCSR access. The vector tests carry values worked out by
//! hand from the SDM pseudocode; the matrix tests compare against `reference`, a second, literal
//! transcription of that pseudocode on 128-bit integers that shares no code with the model.
//!
//! Not asserted here because the SDM text does not define it or the model does not carry it: the CPUID.SSE4_1
//! gate (S2-57), `#AC` of the Type 5 forms (an operand not aligned to its size at CPL3 with RFLAGS.AC = 1 is
//! refused as `Unsupported` instead of guessed; the physical oracle raises SIGBUS there), and the
//! floating-point SSE4.1 forms (ROUNDPS/PD/SS/SD, DPPS/DPPD, INSERTPS/EXTRACTPS, BLENDPS/PD,
//! BLENDVPS/PD), which stay refused until the FP core of S2-37/S2-38.

use od_isa::execute::{self, Cr4Profile, RunConfig, RunExit, StepOutcome};
use od_isa::long_decode::{self, Operation, Sse41Kind, Sse41Operand};
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

// ---- the SSE4.1 forms under test ----

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    /// xmm, xmm/m128 (Type 4: a memory operand must be 16-byte aligned).
    Vector,
    /// xmm, m128 only (Type 1).
    Movntdqa,
    /// xmm, xmm/mN with N < 16 (Type 5: PMOVSX/PMOVZX).
    Widen,
    /// xmm, r/m (Type 5: PINSR*).
    Insert,
    /// r/m, xmm (Type 5: PEXTR*).
    Extract,
}

struct Kind {
    name: &'static str,
    kind: Sse41Kind,
    /// Opcode bytes after the mandatory 66 (and REX): 0F 38 xx or 0F 3A xx.
    opcode: [u8; 3],
    rex_w: bool,
    shape: Shape,
    /// Bytes of the memory operand.
    bytes: usize,
    immediate: bool,
}

const fn kind(
    name: &'static str,
    kind: Sse41Kind,
    opcode: [u8; 3],
    rex_w: bool,
    shape: Shape,
    bytes: usize,
    immediate: bool,
) -> Kind {
    Kind {
        name,
        kind,
        opcode,
        rex_w,
        shape,
        bytes,
        immediate,
    }
}

#[rustfmt::skip]
const KINDS: [Kind; 37] = [
    kind(
        "ptest",
        Sse41Kind::Ptest,
        [0x0f, 0x38, 0x17],
        false,
        Shape::Vector,
        16,
        false,
    ),
    kind(
        "pmuldq",
        Sse41Kind::Pmuldq,
        [0x0f, 0x38, 0x28],
        false,
        Shape::Vector,
        16,
        false,
    ),
    kind(
        "pcmpeqq",
        Sse41Kind::Pcmpeqq,
        [0x0f, 0x38, 0x29],
        false,
        Shape::Vector,
        16,
        false,
    ),
    kind(
        "packusdw",
        Sse41Kind::Packusdw,
        [0x0f, 0x38, 0x2b],
        false,
        Shape::Vector,
        16,
        false,
    ),
    kind(
        "pminsb",
        Sse41Kind::Pminsb,
        [0x0f, 0x38, 0x38],
        false,
        Shape::Vector,
        16,
        false,
    ),
    kind(
        "pminsd",
        Sse41Kind::Pminsd,
        [0x0f, 0x38, 0x39],
        false,
        Shape::Vector,
        16,
        false,
    ),
    kind(
        "pminuw",
        Sse41Kind::Pminuw,
        [0x0f, 0x38, 0x3a],
        false,
        Shape::Vector,
        16,
        false,
    ),
    kind(
        "pminud",
        Sse41Kind::Pminud,
        [0x0f, 0x38, 0x3b],
        false,
        Shape::Vector,
        16,
        false,
    ),
    kind(
        "pmaxsb",
        Sse41Kind::Pmaxsb,
        [0x0f, 0x38, 0x3c],
        false,
        Shape::Vector,
        16,
        false,
    ),
    kind(
        "pmaxsd",
        Sse41Kind::Pmaxsd,
        [0x0f, 0x38, 0x3d],
        false,
        Shape::Vector,
        16,
        false,
    ),
    kind(
        "pmaxuw",
        Sse41Kind::Pmaxuw,
        [0x0f, 0x38, 0x3e],
        false,
        Shape::Vector,
        16,
        false,
    ),
    kind(
        "pmaxud",
        Sse41Kind::Pmaxud,
        [0x0f, 0x38, 0x3f],
        false,
        Shape::Vector,
        16,
        false,
    ),
    kind(
        "pmulld",
        Sse41Kind::Pmulld,
        [0x0f, 0x38, 0x40],
        false,
        Shape::Vector,
        16,
        false,
    ),
    kind(
        "phminposuw",
        Sse41Kind::Phminposuw,
        [0x0f, 0x38, 0x41],
        false,
        Shape::Vector,
        16,
        false,
    ),
    kind(
        "pblendvb",
        Sse41Kind::Pblendvb,
        [0x0f, 0x38, 0x10],
        false,
        Shape::Vector,
        16,
        false,
    ),
    kind(
        "pblendw",
        Sse41Kind::Pblendw,
        [0x0f, 0x3a, 0x0e],
        false,
        Shape::Vector,
        16,
        true,
    ),
    kind(
        "mpsadbw",
        Sse41Kind::Mpsadbw,
        [0x0f, 0x3a, 0x42],
        false,
        Shape::Vector,
        16,
        true,
    ),
    kind(
        "movntdqa",
        Sse41Kind::Movntdqa,
        [0x0f, 0x38, 0x2a],
        false,
        Shape::Movntdqa,
        16,
        false,
    ),
    kind(
        "pmovsxbw",
        Sse41Kind::Pmovsxbw,
        [0x0f, 0x38, 0x20],
        false,
        Shape::Widen,
        8,
        false,
    ),
    kind(
        "pmovsxbd",
        Sse41Kind::Pmovsxbd,
        [0x0f, 0x38, 0x21],
        false,
        Shape::Widen,
        4,
        false,
    ),
    kind(
        "pmovsxbq",
        Sse41Kind::Pmovsxbq,
        [0x0f, 0x38, 0x22],
        false,
        Shape::Widen,
        2,
        false,
    ),
    kind(
        "pmovsxwd",
        Sse41Kind::Pmovsxwd,
        [0x0f, 0x38, 0x23],
        false,
        Shape::Widen,
        8,
        false,
    ),
    kind(
        "pmovsxwq",
        Sse41Kind::Pmovsxwq,
        [0x0f, 0x38, 0x24],
        false,
        Shape::Widen,
        4,
        false,
    ),
    kind(
        "pmovsxdq",
        Sse41Kind::Pmovsxdq,
        [0x0f, 0x38, 0x25],
        false,
        Shape::Widen,
        8,
        false,
    ),
    kind(
        "pmovzxbw",
        Sse41Kind::Pmovzxbw,
        [0x0f, 0x38, 0x30],
        false,
        Shape::Widen,
        8,
        false,
    ),
    kind(
        "pmovzxbd",
        Sse41Kind::Pmovzxbd,
        [0x0f, 0x38, 0x31],
        false,
        Shape::Widen,
        4,
        false,
    ),
    kind(
        "pmovzxbq",
        Sse41Kind::Pmovzxbq,
        [0x0f, 0x38, 0x32],
        false,
        Shape::Widen,
        2,
        false,
    ),
    kind(
        "pmovzxwd",
        Sse41Kind::Pmovzxwd,
        [0x0f, 0x38, 0x33],
        false,
        Shape::Widen,
        8,
        false,
    ),
    kind(
        "pmovzxwq",
        Sse41Kind::Pmovzxwq,
        [0x0f, 0x38, 0x34],
        false,
        Shape::Widen,
        4,
        false,
    ),
    kind(
        "pmovzxdq",
        Sse41Kind::Pmovzxdq,
        [0x0f, 0x38, 0x35],
        false,
        Shape::Widen,
        8,
        false,
    ),
    kind(
        "pinsrb",
        Sse41Kind::Pinsrb,
        [0x0f, 0x3a, 0x20],
        false,
        Shape::Insert,
        1,
        true,
    ),
    kind(
        "pinsrd",
        Sse41Kind::Pinsrd,
        [0x0f, 0x3a, 0x22],
        false,
        Shape::Insert,
        4,
        true,
    ),
    kind(
        "pinsrq",
        Sse41Kind::Pinsrq,
        [0x0f, 0x3a, 0x22],
        true,
        Shape::Insert,
        8,
        true,
    ),
    kind(
        "pextrb",
        Sse41Kind::Pextrb,
        [0x0f, 0x3a, 0x14],
        false,
        Shape::Extract,
        1,
        true,
    ),
    kind(
        "pextrw",
        Sse41Kind::Pextrw,
        [0x0f, 0x3a, 0x15],
        false,
        Shape::Extract,
        2,
        true,
    ),
    kind(
        "pextrd",
        Sse41Kind::Pextrd,
        [0x0f, 0x3a, 0x16],
        false,
        Shape::Extract,
        4,
        true,
    ),
    kind(
        "pextrq",
        Sse41Kind::Pextrq,
        [0x0f, 0x3a, 0x16],
        true,
        Shape::Extract,
        8,
        true,
    ),
];

impl Kind {
    /// A 128-bit memory operand: #GP(0) when not 16-byte aligned.
    fn aligned16(&self) -> bool {
        self.bytes == 16
    }

    /// REX.W selects the quadword form of PINSRD/PINSRQ and PEXTRD/PEXTRQ.
    fn rex_w_selects(&self) -> bool {
        matches!(
            self.kind,
            Sse41Kind::Pinsrd | Sse41Kind::Pinsrq | Sse41Kind::Pextrd | Sse41Kind::Pextrq
        )
    }

    /// The register form exists (MOVNTDQA is memory only).
    fn has_register_form(&self) -> bool {
        self.shape != Shape::Movntdqa
    }
}

fn kind_named(name: &str) -> &'static Kind {
    KINDS.iter().find(|kind| kind.name == name).unwrap()
}

/// 66 [REX] 0F 38 xx /r (or 0F 3A xx /r ib) with a register r/m (mod = 3): 'reg' is the ModRM.reg
/// register (an XMM register) and 'rm' the ModRM.r/m register (a general register for PINSR*/PEXTR*).
fn reg_code_w(kind: &Kind, reg: u8, rm: u8, immediate: u8, force_w: bool) -> Vec<u8> {
    let mut code = vec![0x66];
    let rex = (if kind.rex_w || force_w { 8 } else { 0 })
        | (if reg >= 8 { 4 } else { 0 })
        | (if rm >= 8 { 1 } else { 0 });
    if rex != 0 {
        code.push(0x40 | rex);
    }
    code.extend_from_slice(&kind.opcode);
    code.push(0xc0 | ((reg & 7) << 3) | (rm & 7));
    if kind.immediate {
        code.push(immediate);
    }
    code
}

fn reg_code(kind: &Kind, reg: u8, rm: u8, immediate: u8) -> Vec<u8> {
    reg_code_w(kind, reg, rm, immediate, false)
}

/// 66 [prefixes] [REX] 0F 38 xx /r [SIB] [disp] [ib] for one addressing form.
fn mem_code(kind: &Kind, form: &Form, reg: u8, immediate: u8) -> Vec<u8> {
    let mut code = form.extra.to_vec();
    code.push(0x66);
    let rex = form.rex | if kind.rex_w { 8 } else { 0 } | if reg >= 8 { 0x04 } else { 0 };
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
            let end = CODE + code.len() as u64 + 4 + u64::from(kind.immediate);
            code.extend_from_slice(&((target.wrapping_sub(end)) as i32).to_le_bytes());
        }
    }
    if kind.immediate {
        code.push(immediate);
    }
    code
}

// ---- the reference model: the SDM pseudocode, transcribed literally on u128 / i128 ----

fn big(value: &[u8; 16]) -> u128 {
    u128::from_le_bytes(*value)
}

/// Lane 'index' of 'bits' bits, zero-extended.
fn field(value: u128, bits: u32, index: u32) -> u128 {
    (value >> (bits * index)) & ((1u128 << bits) - 1)
}

fn signed(value: u128, bits: u32) -> i128 {
    let shift = 128 - bits;
    ((value << shift) as i128) >> shift
}

fn put(out: &mut u128, bits: u32, index: u32, value: u128) {
    let mask = ((1u128 << bits) - 1) << (bits * index);
    *out = (*out & !mask) | ((value << (bits * index)) & mask);
}

/// Result register of the register-writing vector forms.
fn reference(
    kind: Sse41Kind,
    dest: &[u8; 16],
    src: &[u8; 16],
    xmm0: &[u8; 16],
    imm8: u8,
) -> [u8; 16] {
    let (d, s) = (big(dest), big(src));
    let mut out = 0u128;
    // (source bits, destination bits, sign-extend) of PMOVSX / PMOVZX.
    let widen = |from: u32, to: u32, sign: bool| {
        let mut out = 0u128;
        for index in 0..128 / to {
            let lane = field(s, from, index);
            put(
                &mut out,
                to,
                index,
                if sign {
                    signed(lane, from) as u128
                } else {
                    lane
                },
            );
        }
        out
    };
    // (lane bits, signed compare, maximum) of PMIN* / PMAX*.
    let extremum = |bits: u32, sign: bool, maximum: bool| {
        let mut out = 0u128;
        for index in 0..128 / bits {
            let (a, b) = (field(d, bits, index), field(s, bits, index));
            let (ka, kb) = if sign {
                (signed(a, bits), signed(b, bits))
            } else {
                (a as i128, b as i128)
            };
            let pick = if (ka > kb) == maximum { a } else { b };
            put(&mut out, bits, index, pick);
        }
        out
    };
    match kind {
        Sse41Kind::Pmovsxbw => out = widen(8, 16, true),
        Sse41Kind::Pmovsxbd => out = widen(8, 32, true),
        Sse41Kind::Pmovsxbq => out = widen(8, 64, true),
        Sse41Kind::Pmovsxwd => out = widen(16, 32, true),
        Sse41Kind::Pmovsxwq => out = widen(16, 64, true),
        Sse41Kind::Pmovsxdq => out = widen(32, 64, true),
        Sse41Kind::Pmovzxbw => out = widen(8, 16, false),
        Sse41Kind::Pmovzxbd => out = widen(8, 32, false),
        Sse41Kind::Pmovzxbq => out = widen(8, 64, false),
        Sse41Kind::Pmovzxwd => out = widen(16, 32, false),
        Sse41Kind::Pmovzxwq => out = widen(16, 64, false),
        Sse41Kind::Pmovzxdq => out = widen(32, 64, false),
        Sse41Kind::Pminsb => out = extremum(8, true, false),
        Sse41Kind::Pminsd => out = extremum(32, true, false),
        Sse41Kind::Pminuw => out = extremum(16, false, false),
        Sse41Kind::Pminud => out = extremum(32, false, false),
        Sse41Kind::Pmaxsb => out = extremum(8, true, true),
        Sse41Kind::Pmaxsd => out = extremum(32, true, true),
        Sse41Kind::Pmaxuw => out = extremum(16, false, true),
        Sse41Kind::Pmaxud => out = extremum(32, false, true),
        Sse41Kind::Pmulld => {
            for index in 0..4 {
                let product = signed(field(d, 32, index), 32) * signed(field(s, 32, index), 32);
                put(&mut out, 32, index, product as u128);
            }
        }
        Sse41Kind::Pmuldq => {
            for index in 0..2 {
                let product =
                    signed(field(d, 32, 2 * index), 32) * signed(field(s, 32, 2 * index), 32);
                put(&mut out, 64, index, product as u128);
            }
        }
        Sse41Kind::Pcmpeqq => {
            for index in 0..2 {
                if field(d, 64, index) == field(s, 64, index) {
                    put(&mut out, 64, index, u128::from(u64::MAX));
                }
            }
        }
        Sse41Kind::Packusdw => {
            for index in 0..4 {
                for (half, value) in [(0, d), (4, s)] {
                    let lane = signed(field(value, 32, index), 32).clamp(0, 0xffff);
                    put(&mut out, 16, half + index, lane as u128);
                }
            }
        }
        Sse41Kind::Pblendw => {
            for index in 0..8u8 {
                let lane = if imm8 >> index & 1 == 1 {
                    field(s, 16, u32::from(index))
                } else {
                    field(d, 16, u32::from(index))
                };
                put(&mut out, 16, u32::from(index), lane);
            }
        }
        Sse41Kind::Pblendvb => {
            for index in 0..16 {
                let lane = if field(big(xmm0), 8, index) >> 7 == 1 {
                    field(s, 8, index)
                } else {
                    field(d, 8, index)
                };
                put(&mut out, 8, index, lane);
            }
        }
        Sse41Kind::Phminposuw => {
            let mut minimum = field(s, 16, 0);
            let mut position = 0u32;
            for index in 1..8 {
                if field(s, 16, index) < minimum {
                    minimum = field(s, 16, index);
                    position = index;
                }
            }
            out = minimum | (u128::from(position) << 16);
        }
        Sse41Kind::Mpsadbw => {
            let block2 = u32::from(imm8 & 3) * 4;
            let block1 = u32::from(imm8 >> 2 & 1) * 4;
            for index in 0..8 {
                let mut sum = 0i128;
                for byte in 0..4 {
                    let a = field(d, 8, block1 + index + byte) as i128;
                    let b = field(s, 8, block2 + byte) as i128;
                    sum += (a - b).abs();
                }
                put(&mut out, 16, index, sum as u128);
            }
        }
        Sse41Kind::Movntdqa => out = s,
        Sse41Kind::Ptest => out = d,
        other => panic!("{other:?} is not a register-writing vector form"),
    }
    out.to_le_bytes()
}

/// PTEST: (ZF, CF).
fn ptest_flags(dest: &[u8; 16], src: &[u8; 16]) -> (bool, bool) {
    let (d, s) = (big(dest), big(src));
    (s & d == 0, s & !d == 0)
}

const STATUS: u64 = 0x1 | 0x4 | 0x10 | 0x40 | 0x80 | 0x800;
const ZF: u64 = 0x40;
const CF: u64 = 0x1;

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

fn qwords(values: [u64; 2]) -> [u8; 16] {
    let mut out = [0u8; 16];
    for (index, value) in values.into_iter().enumerate() {
        out[8 * index..8 * index + 8].copy_from_slice(&value.to_le_bytes());
    }
    out
}

fn counting(start: u8) -> [u8; 16] {
    core::array::from_fn(|index| start + index as u8)
}

/// What one run of a register-writing vector form produced.
struct Run {
    xmm: [u8; 16],
    rflags: u64,
}

/// Runs a Vector, Widen or Movntdqa form with the destination in xmm2, XMM0 holding 'mask', once with
/// the source in xmm1 (register form, not for MOVNTDQA) and once in memory at 0x8010. Checks that nothing
/// but xmm2 (and RFLAGS for PTEST) and RIP changed, and that the memory form read exactly the operand.
fn run_xmm(kind: &Kind, dest: [u8; 16], src: [u8; 16], mask: [u8; 16], immediate: u8) -> Vec<Run> {
    let mut results = vec![];
    for variant in 0..2 {
        if variant == 0 && !kind.has_register_form() {
            continue;
        }
        let code = if variant == 0 {
            reg_code(kind, 2, 1, immediate)
        } else {
            mem_code(kind, &FORMS[0], 2, immediate)
        };
        let mut cpu = cpu();
        cpu.sse.as_mut().unwrap().xmm[0] = mask;
        cpu.sse.as_mut().unwrap().xmm[2] = dest;
        let mut ram = Ram::new(&code);
        let mut visible = src;
        if variant == 0 {
            cpu.sse.as_mut().unwrap().xmm[1] = src;
        } else {
            cpu.gpr[Register::Rax as usize] = 0x8010;
            ram.bytes[0x8010..0x8010 + kind.bytes].copy_from_slice(&src[..kind.bytes]);
            visible[kind.bytes..].fill(0);
        }
        let ram_before = ram.bytes.clone();
        let mut expected = cpu.clone();
        expected.rip = CODE + code.len() as u64;
        let label = format!("{} variant {variant} imm {immediate:#x}", kind.name);
        let outcome = step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring0);
        assert_eq!(outcome, Ok(StepOutcome::Advanced), "{label}");
        let xmm = cpu.sse.as_ref().unwrap().xmm[2];
        if kind.kind == Sse41Kind::Ptest {
            let (zf, cf) = ptest_flags(&dest, &visible);
            expected.rflags =
                (expected.rflags & !STATUS) | if zf { ZF } else { 0 } | if cf { CF } else { 0 };
        } else {
            expected.sse.as_mut().unwrap().xmm[2] = xmm;
        }
        assert_eq!(
            cpu, expected,
            "{label}: only xmm2 (RFLAGS for PTEST) and RIP change"
        );
        assert_eq!(ram.bytes, ram_before, "{label}");
        assert_eq!(ram.writes, [], "{label}");
        let expected_reads: Vec<(u64, usize)> = if variant == 0 {
            vec![]
        } else {
            vec![(0x8010, kind.bytes)]
        };
        assert_eq!(ram.reads, expected_reads, "{label}");
        results.push(Run {
            xmm,
            rflags: cpu.rflags,
        });
    }
    results
}

fn assert_vector(name: &str, dest: [u8; 16], src: [u8; 16], immediate: u8, expected: [u8; 16]) {
    assert_masked_vector(name, dest, src, xmm_pattern(0), immediate, expected);
}

fn assert_masked_vector(
    name: &str,
    dest: [u8; 16],
    src: [u8; 16],
    mask: [u8; 16],
    immediate: u8,
    expected: [u8; 16],
) {
    let kind = kind_named(name);
    for (variant, run) in run_xmm(kind, dest, src, mask, immediate).iter().enumerate() {
        assert_eq!(
            run.xmm, expected,
            "{name} variant {variant}, imm {immediate:#x}"
        );
    }
    // The independent transcription of the pseudocode gives the same bytes.
    assert_eq!(
        reference(kind.kind, &dest, &src, &mask, immediate),
        expected,
        "{name} reference"
    );
}

/// PINSR*: destination xmm2; the source is general register rcx (register form) or [rax] = 0x8010
/// (memory form). 'value' carries garbage above the operand width in the register form.
fn run_insert(kind: &Kind, dest: [u8; 16], value: u64, immediate: u8) -> [[u8; 16]; 2] {
    let mut results = [[0u8; 16]; 2];
    for (variant, result) in results.iter_mut().enumerate() {
        let code = if variant == 0 {
            reg_code(kind, 2, 1, immediate)
        } else {
            mem_code(kind, &FORMS[0], 2, immediate)
        };
        let mut cpu = cpu();
        cpu.sse.as_mut().unwrap().xmm[2] = dest;
        let mut ram = Ram::new(&code);
        if variant == 0 {
            cpu.gpr[Register::Rcx as usize] = value;
        } else {
            cpu.gpr[Register::Rax as usize] = 0x8010;
            ram.bytes[0x8010..0x8010 + kind.bytes]
                .copy_from_slice(&value.to_le_bytes()[..kind.bytes]);
        }
        let mut expected = cpu.clone();
        expected.rip = CODE + code.len() as u64;
        let ram_before = ram.bytes.clone();
        let label = format!("{} variant {variant} imm {immediate:#x}", kind.name);
        assert_eq!(
            step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring0),
            Ok(StepOutcome::Advanced),
            "{label}"
        );
        // SDM: the destination element is imm8 modulo the element count; the other elements keep their value.
        let bits = 8 * kind.bytes as u32;
        let mut want = big(&dest);
        let lane = u32::from(immediate) % (128 / bits);
        put(
            &mut want,
            bits,
            lane,
            u128::from(value) & ((1u128 << bits) - 1),
        );
        *result = cpu.sse.as_ref().unwrap().xmm[2];
        expected.sse.as_mut().unwrap().xmm[2] = want.to_le_bytes();
        assert_eq!(cpu, expected, "{label}");
        assert_eq!(ram.bytes, ram_before, "{label}");
        assert_eq!(ram.writes, [], "{label}");
        assert_eq!(ram.reads.len(), variant, "{label}");
    }
    results
}

/// PEXTR*: source xmm2; the destination is general register rcx (pre-set to all ones) or [rax] = 0x8010
/// (memory, surrounded by sentinel bytes). Returns the value of the register form.
fn run_extract(kind: &Kind, source: [u8; 16], immediate: u8) -> u64 {
    let bits = 8 * kind.bytes as u32;
    let lane = u32::from(immediate) % (128 / bits);
    let value = field(big(&source), bits, lane) as u64;
    // Register form: the destination is zero-extended to 64 bits (PEXTRB/PEXTRW/PEXTRD alike).
    let code = reg_code(kind, 2, 1, immediate);
    let mut cpu = cpu();
    cpu.sse.as_mut().unwrap().xmm[2] = source;
    cpu.gpr[Register::Rcx as usize] = u64::MAX;
    let mut ram = Ram::new(&code);
    let ram_before = ram.bytes.clone();
    let mut expected = cpu.clone();
    expected.rip = CODE + code.len() as u64;
    expected.gpr[Register::Rcx as usize] = value;
    let label = format!("{} register imm {immediate:#x}", kind.name);
    assert_eq!(
        step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring0),
        Ok(StepOutcome::Advanced),
        "{label}"
    );
    assert_eq!(cpu, expected, "{label}");
    assert_eq!(ram.bytes, ram_before, "{label}");
    assert!(ram.reads.is_empty() && ram.writes.is_empty(), "{label}");
    // Memory form: exactly the operand bytes are written, in one access, and nothing is read.
    let code = mem_code(kind, &FORMS[0], 2, immediate);
    let mut cpu = self::cpu();
    cpu.sse.as_mut().unwrap().xmm[2] = source;
    cpu.gpr[Register::Rax as usize] = 0x8010;
    let mut ram = Ram::new(&code);
    let mut expected_ram = ram.bytes.clone();
    expected_ram[0x8010..0x8010 + kind.bytes].copy_from_slice(&value.to_le_bytes()[..kind.bytes]);
    let mut expected = cpu.clone();
    expected.rip = CODE + code.len() as u64;
    let label = format!("{} memory imm {immediate:#x}", kind.name);
    assert_eq!(
        step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring0),
        Ok(StepOutcome::Advanced),
        "{label}"
    );
    assert_eq!(cpu, expected, "{label}");
    assert_eq!(ram.bytes, expected_ram, "{label}");
    assert_eq!(ram.writes, [(0x8010, kind.bytes)], "{label}");
    assert!(ram.reads.is_empty(), "{label}");
    value
}

// ---- vectors worked out by hand from the SDM pseudocode ----

#[test]
fn pmovsx_extends_by_the_sign_and_pmovzx_by_zero_for_all_twelve_forms() {
    let byte_source = bytes([
        0x80, 0x7f, 0xff, 0x01, 0x00, 0xfe, 0x81, 0x02, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x11,
        0x22,
    ]);
    let word_source = words([
        0x8000, 0x7fff, 0xffff, 0x0001, 0x1234, 0xfffe, 0x8001, 0x0002,
    ]);
    let dword_source = dwords([0x8000_0000, 0x7fff_ffff, 0xffff_ffff, 0x0000_0001]);
    let junk = counting(0xa0);
    for (name, src, expected) in [
        (
            "pmovsxbw",
            byte_source,
            words([
                0xff80, 0x007f, 0xffff, 0x0001, 0x0000, 0xfffe, 0xff81, 0x0002,
            ]),
        ),
        (
            "pmovzxbw",
            byte_source,
            words([
                0x0080, 0x007f, 0x00ff, 0x0001, 0x0000, 0x00fe, 0x0081, 0x0002,
            ]),
        ),
        (
            "pmovsxbd",
            byte_source,
            dwords([0xffff_ff80, 0x0000_007f, 0xffff_ffff, 0x0000_0001]),
        ),
        (
            "pmovzxbd",
            byte_source,
            dwords([0x0000_0080, 0x0000_007f, 0x0000_00ff, 0x0000_0001]),
        ),
        (
            "pmovsxbq",
            byte_source,
            qwords([0xffff_ffff_ffff_ff80, 0x0000_0000_0000_007f]),
        ),
        ("pmovzxbq", byte_source, qwords([0x80, 0x7f])),
        (
            "pmovsxwd",
            word_source,
            dwords([0xffff_8000, 0x0000_7fff, 0xffff_ffff, 0x0000_0001]),
        ),
        (
            "pmovzxwd",
            word_source,
            dwords([0x0000_8000, 0x0000_7fff, 0x0000_ffff, 0x0000_0001]),
        ),
        (
            "pmovsxwq",
            word_source,
            qwords([0xffff_ffff_ffff_8000, 0x0000_0000_0000_7fff]),
        ),
        ("pmovzxwq", word_source, qwords([0x8000, 0x7fff])),
        (
            "pmovsxdq",
            dword_source,
            qwords([0xffff_ffff_8000_0000, 0x0000_0000_7fff_ffff]),
        ),
        ("pmovzxdq", dword_source, qwords([0x8000_0000, 0x7fff_ffff])),
    ] {
        // The destination register's previous value is fully overwritten.
        assert_vector(name, junk, src, 0, expected);
    }
}

#[test]
fn pmulld_keeps_the_low_32_bits_and_pmuldq_is_the_signed_product_of_dwords_0_and_2() {
    assert_vector(
        "pmulld",
        dwords([0x7fff_ffff, 0x8000_0000, 0xffff_ffff, 0x0001_0001]),
        dwords([2, 0x8000_0000, 0xffff_ffff, 0x0001_0001]),
        0,
        // 0x7fffffff*2 = 0xfffffffe; 2^31*2^31 = 2^62 -> low 32 bits 0; (-1)*(-1) = 1; 0x10001^2 = 0x100020001.
        dwords([0xffff_fffe, 0, 1, 0x0002_0001]),
    );
    assert_vector(
        "pmuldq",
        dwords([0x8000_0000, 0x1111_1111, 0x7fff_ffff, 0x2222_2222]),
        dwords([0x8000_0000, 0x3333_3333, 0x8000_0000, 0x4444_4444]),
        0,
        // (-2^31)*(-2^31) = 2^62; 0x7fffffff * (-2^31) = -0x3fffffff80000000. Lanes 1 and 3 are ignored.
        qwords([0x4000_0000_0000_0000, 0xc000_0000_8000_0000]),
    );
    assert_vector(
        "pmuldq",
        dwords([0xffff_ffff, 0, 0xffff_ffff, 0]),
        dwords([0xffff_ffff, 0, 1, 0]),
        0,
        // (-1)*(-1) = 1; (-1)*1 = -1 (sign-extended to 64 bits).
        qwords([1, 0xffff_ffff_ffff_ffff]),
    );
}

#[test]
fn pmin_and_pmax_compare_with_the_sign_or_without_it_at_the_range_ends() {
    let a = bytes([
        0x80, 0x7f, 0x00, 0xff, 0x01, 0x81, 0x7e, 0x00, 0x80, 0x7f, 0x00, 0xff, 1, 2, 3, 4,
    ]);
    let b = bytes([
        0x7f, 0x80, 0xff, 0x00, 0x01, 0x80, 0x7f, 0x01, 0x80, 0x7f, 0xff, 0x00, 4, 3, 2, 1,
    ]);
    assert_vector(
        "pminsb",
        a,
        b,
        0,
        bytes([
            0x80, 0x80, 0xff, 0xff, 0x01, 0x80, 0x7e, 0x00, 0x80, 0x7f, 0xff, 0xff, 1, 2, 2, 1,
        ]),
    );
    assert_vector(
        "pmaxsb",
        a,
        b,
        0,
        bytes([
            0x7f, 0x7f, 0x00, 0x00, 0x01, 0x81, 0x7f, 0x01, 0x80, 0x7f, 0x00, 0x00, 4, 3, 3, 4,
        ]),
    );
    let (wa, wb) = (
        words([
            0x8000, 0xffff, 0x0000, 0x0001, 0x7fff, 0x8001, 0x1234, 0xfffe,
        ]),
        words([
            0x7fff, 0xfffe, 0xffff, 0x0001, 0x8000, 0x8000, 0x1234, 0xffff,
        ]),
    );
    assert_vector(
        "pminuw",
        wa,
        wb,
        0,
        words([
            0x7fff, 0xfffe, 0x0000, 0x0001, 0x7fff, 0x8000, 0x1234, 0xfffe,
        ]),
    );
    assert_vector(
        "pmaxuw",
        wa,
        wb,
        0,
        words([
            0x8000, 0xffff, 0xffff, 0x0001, 0x8000, 0x8001, 0x1234, 0xffff,
        ]),
    );
    let (da, db) = (
        dwords([0x8000_0000, 0x7fff_ffff, 0xffff_ffff, 0]),
        dwords([0x7fff_ffff, 0x8000_0000, 0, 0xffff_ffff]),
    );
    assert_vector(
        "pminsd",
        da,
        db,
        0,
        dwords([0x8000_0000, 0x8000_0000, 0xffff_ffff, 0xffff_ffff]),
    );
    assert_vector(
        "pmaxsd",
        da,
        db,
        0,
        dwords([0x7fff_ffff, 0x7fff_ffff, 0, 0]),
    );
    assert_vector(
        "pminud",
        da,
        db,
        0,
        dwords([0x7fff_ffff, 0x7fff_ffff, 0, 0]),
    );
    assert_vector(
        "pmaxud",
        da,
        db,
        0,
        dwords([0x8000_0000, 0x8000_0000, 0xffff_ffff, 0xffff_ffff]),
    );
}

#[test]
fn pcmpeqq_compares_whole_qwords() {
    assert_vector(
        "pcmpeqq",
        qwords([0x1122_3344_5566_7788, 0x1122_3344_5566_7788]),
        // The first qword is equal, the second differs only in the top bit.
        qwords([0x1122_3344_5566_7788, 0x9122_3344_5566_7788]),
        0,
        qwords([u64::MAX, 0]),
    );
    assert_vector(
        "pcmpeqq",
        qwords([0, u64::MAX]),
        qwords([1, u64::MAX]),
        0,
        qwords([0, u64::MAX]),
    );
}

#[test]
fn packusdw_clamps_signed_dwords_to_the_unsigned_word_range_destination_low_source_high() {
    assert_vector(
        "packusdw",
        dwords([0xffff_ffff, 0x0001_0000, 0x0000_ffff, 0x8000_0000]),
        dwords([0x7fff_ffff, 0x0000_0001, 0xffff_0000, 0x0000_8000]),
        0,
        // dest: -1 -> 0, 65536 -> 65535, 65535, INT_MIN -> 0; src: INT_MAX -> 65535, 1, negative -> 0, 32768.
        words([0, 0xffff, 0xffff, 0, 0xffff, 1, 0, 0x8000]),
    );
}

#[test]
fn pblendw_selects_words_by_the_immediate_and_pblendvb_bytes_by_the_top_bit_of_xmm0() {
    let (d, s) = (
        words([
            0x1111, 0x2222, 0x3333, 0x4444, 0x5555, 0x6666, 0x7777, 0x8888,
        ]),
        words([
            0xaaaa, 0xbbbb, 0xcccc, 0xdddd, 0xeeee, 0xffff, 0x0123, 0x4567,
        ]),
    );
    assert_vector("pblendw", d, s, 0x00, d);
    assert_vector("pblendw", d, s, 0xff, s);
    assert_vector(
        "pblendw",
        d,
        s,
        0xa5,
        words([
            0xaaaa, 0x2222, 0xcccc, 0x4444, 0x5555, 0xffff, 0x7777, 0x4567,
        ]),
    );
    let mut mask = [0u8; 16];
    // Only bit 7 of each mask byte counts: 0x7f and 0x01 select the destination, 0x80 and 0xff the source.
    for (index, value) in [0x7f, 0x80, 0x01, 0xff, 0x00, 0x81, 0xfe, 0x7e]
        .into_iter()
        .enumerate()
    {
        mask[index] = value;
        mask[8 + index] = value ^ 0x80;
    }
    let expected: [u8; 16] = core::array::from_fn(|index| {
        if mask[index] & 0x80 != 0 {
            s[index]
        } else {
            d[index]
        }
    });
    assert_eq!(expected[..4], [d[0], s[1], d[2], s[3]]);
    assert_masked_vector("pblendvb", d, s, mask, 0, expected);
}

#[test]
fn pblendvb_with_the_destination_xmm0_uses_the_old_mask_and_the_implicit_register_is_xmm0() {
    let code = reg_code(kind_named("pblendvb"), 0, 1, 0);
    let mut cpu = cpu();
    let mask = bytes([
        0x80, 0, 0x80, 0, 0x80, 0, 0x80, 0, 0x80, 0, 0x80, 0, 0x80, 0, 0x80, 0,
    ]);
    cpu.sse.as_mut().unwrap().xmm[0] = mask;
    cpu.sse.as_mut().unwrap().xmm[1] = [0xee; 16];
    let mut ram = Ram::new(&code);
    assert_eq!(
        step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring0),
        Ok(StepOutcome::Advanced)
    );
    let expected: [u8; 16] = core::array::from_fn(|index| {
        if mask[index] == 0x80 {
            0xee
        } else {
            mask[index]
        }
    });
    assert_eq!(cpu.sse.as_ref().unwrap().xmm[0], expected);
    // The same instruction with another destination takes the mask from xmm0, not from the destination.
    let code = reg_code(kind_named("pblendvb"), 3, 1, 0);
    let mut cpu = self::cpu();
    cpu.sse.as_mut().unwrap().xmm[0] = [0xff; 16];
    cpu.sse.as_mut().unwrap().xmm[3] = [0x00; 16];
    cpu.sse.as_mut().unwrap().xmm[1] = [0x5a; 16];
    let mut ram = Ram::new(&code);
    assert_eq!(
        step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring0),
        Ok(StepOutcome::Advanced)
    );
    assert_eq!(cpu.sse.as_ref().unwrap().xmm[3], [0x5a; 16]);
}

#[test]
fn ptest_sets_zf_and_cf_clears_the_other_arithmetic_flags_and_leaves_the_registers() {
    let zero = [0u8; 16];
    let ones = [0xffu8; 16];
    let low = qwords([u64::MAX, 0]);
    let high = qwords([0, u64::MAX]);
    let single = bytes([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x80]);
    // (destination xmm2, source, ZF, CF): ZF = (src AND dest) == 0, CF = (src AND NOT dest) == 0.
    for (dest, src, zf, cf) in [
        (zero, zero, true, true),
        (ones, zero, true, true),
        (zero, ones, true, false),
        (ones, ones, false, true),
        (low, high, true, false),
        (high, low, true, false),
        (low, low, false, true),
        (ones, low, false, true),
        (low, ones, false, false),
        (single, single, false, true),
        (single, ones, false, false),
        (zero, single, true, false),
    ] {
        for run in run_xmm(kind_named("ptest"), dest, src, xmm_pattern(0), 0) {
            assert_eq!(
                run.rflags & STATUS,
                (if zf { ZF } else { 0 }) | (if cf { CF } else { 0 }),
                "{dest:02x?} {src:02x?}"
            );
            // The direction flag and the reserved bit 1 keep the value (run_xmm compares the whole state).
            assert_eq!(run.rflags & !STATUS, FLAGS & !STATUS);
            assert_eq!(run.xmm, dest);
        }
        assert_eq!(ptest_flags(&dest, &src), (zf, cf));
    }
}

#[test]
fn flag_definedness_ptest_defines_the_arithmetic_flags_and_other_forms_change_none() {
    let config = RunConfig {
        instruction_budget: 1,
        stop_rip: None,
        context: ctx(PrivilegeLevel::Ring0),
    };
    // PTEST xmm2, xmm1 sets ZF and CF from the operands and clears AF, OF, PF and SF (SDM), so all six
    // arithmetic flags are defined afterwards, whatever the tracker held before; DF is not touched.
    for initial in [0x02, 0x02 | 0x800, 0x02 | STATUS | (1 << 10)] {
        let code = reg_code(kind_named("ptest"), 2, 1, 0);
        let mut ram = Ram::new(&code);
        let mut after = cpu();
        let result =
            execute::run_with_rflags_defined(&mut after, &mut ram, config, initial, || false)
                .unwrap();
        assert_eq!(
            result.exit,
            RunExit::InstructionBudgetExhausted,
            "{initial:#x}"
        );
        assert_eq!(result.rflags_defined, initial | STATUS, "{initial:#x}");
    }
    // A form without flag effects keeps the definedness unchanged.
    for initial in [0x02, 0x02 | 0x800, 0x02 | STATUS] {
        let code = reg_code(kind_named("pmovsxbw"), 2, 1, 0);
        let mut ram = Ram::new(&code);
        let mut after = cpu();
        let result =
            execute::run_with_rflags_defined(&mut after, &mut ram, config, initial, || false)
                .unwrap();
        assert_eq!(
            result.exit,
            RunExit::InstructionBudgetExhausted,
            "{initial:#x}"
        );
        assert_eq!(result.rflags_defined, initial, "{initial:#x}");
    }
}

#[test]
fn phminposuw_returns_the_smallest_word_its_first_index_and_zeroes_the_rest() {
    let junk = counting(0x40);
    // Minimum 3 first occurs at index 1.
    assert_vector(
        "phminposuw",
        junk,
        words([5, 3, 9, 3, 7, 0xffff, 3, 8]),
        0,
        words([3, 1, 0, 0, 0, 0, 0, 0]),
    );
    // Minimum at the last index: the index 7 sits in bits 18:16.
    assert_vector(
        "phminposuw",
        junk,
        words([9, 8, 7, 6, 5, 4, 3, 2]),
        0,
        dwords([0x0007_0002, 0, 0, 0]),
    );
    assert_vector(
        "phminposuw",
        junk,
        words([0xffff; 8]),
        0,
        dwords([0x0000_ffff, 0, 0, 0]),
    );
    assert_vector(
        "phminposuw",
        junk,
        words([4, 4, 4, 4, 0, 0, 4, 4]),
        0,
        dwords([0x0004_0000, 0, 0, 0]),
    );
}

#[test]
fn mpsadbw_sums_four_absolute_differences_at_the_offsets_in_imm8_bits_0_to_2() {
    // dest bytes 2*i, source bytes j: word i of imm 0 is sum over j of |2(i+j) - j| = 8i + 6.
    let dest: [u8; 16] = core::array::from_fn(|index| 2 * index as u8);
    let src = counting(0);
    assert_vector(
        "mpsadbw",
        dest,
        src,
        0,
        words([6, 14, 22, 30, 38, 46, 54, 62]),
    );
    // imm 1: source block bytes 4..8, destination block 0: |2(i+j) - (4+j)| summed over j.
    assert_vector(
        "mpsadbw",
        dest,
        src,
        1,
        words([10, 4, 6, 14, 22, 30, 38, 46]),
    );
    // imm 4: destination block bytes 4.., source block 0: 38 + 8i.
    assert_vector(
        "mpsadbw",
        dest,
        src,
        4,
        words([38, 46, 54, 62, 70, 78, 86, 94]),
    );
    // Bits 7:3 are ignored: 0xf8 is 0, 0xfd is 5.
    assert_vector(
        "mpsadbw",
        dest,
        src,
        0xf8,
        words([6, 14, 22, 30, 38, 46, 54, 62]),
    );
    // imm 5: source block bytes 4..8 against the destination from byte 4: 22 + 8i.
    assert_vector(
        "mpsadbw",
        dest,
        src,
        0xfd,
        words([22, 30, 38, 46, 54, 62, 70, 78]),
    );
    assert_vector(
        "mpsadbw",
        dest,
        src,
        5,
        words([22, 30, 38, 46, 54, 62, 70, 78]),
    );
    // The largest sums: four differences of 255 are 1020 = 0x03fc.
    assert_vector("mpsadbw", [0xff; 16], [0; 16], 0, words([0x03fc; 8]));
    assert_vector("mpsadbw", [0; 16], [0xff; 16], 7, words([0x03fc; 8]));
}

#[test]
fn pinsr_replaces_the_element_selected_by_imm8_modulo_the_element_count() {
    let dest = counting(0x10);
    let garbage = 0xfedc_ba98_7654_3210u64;
    for (name, immediate, lane_bytes, lane) in [
        ("pinsrb", 0x13u8, 1usize, 3usize),
        ("pinsrb", 0x0f, 1, 15),
        ("pinsrb", 0xf0, 1, 0),
        ("pinsrd", 0x05, 4, 1),
        ("pinsrd", 0xfe, 4, 2),
        ("pinsrd", 0x04, 4, 0),
        ("pinsrq", 0x03, 8, 1),
        ("pinsrq", 0x02, 8, 0),
        ("pinsrq", 0xff, 8, 1),
    ] {
        let kind = kind_named(name);
        let mut expected = dest;
        expected[lane * lane_bytes..(lane + 1) * lane_bytes]
            .copy_from_slice(&garbage.to_le_bytes()[..lane_bytes]);
        for (variant, result) in run_insert(kind, dest, garbage, immediate)
            .into_iter()
            .enumerate()
        {
            assert_eq!(
                result, expected,
                "{name} imm {immediate:#x} variant {variant}"
            );
        }
    }
}

#[test]
fn pextr_extracts_the_element_selected_by_imm8_modulo_the_element_count_zero_extended() {
    let source = counting(0x81);
    let value = |bytes: &[u8]| {
        let mut raw = [0u8; 8];
        raw[..bytes.len()].copy_from_slice(bytes);
        u64::from_le_bytes(raw)
    };
    assert_eq!(run_extract(kind_named("pextrb"), source, 0x1f), 0x90);
    assert_eq!(run_extract(kind_named("pextrb"), source, 0x03), 0x84);
    assert_eq!(
        run_extract(kind_named("pextrw"), source, 0x09),
        value(&source[2..4])
    );
    assert_eq!(
        run_extract(kind_named("pextrw"), source, 0x07),
        value(&source[14..16])
    );
    assert_eq!(
        run_extract(kind_named("pextrd"), source, 0x07),
        value(&source[12..16])
    );
    assert_eq!(
        run_extract(kind_named("pextrd"), source, 0x04),
        value(&source[0..4])
    );
    assert_eq!(
        run_extract(kind_named("pextrq"), source, 0x02),
        value(&source[0..8])
    );
    assert_eq!(
        run_extract(kind_named("pextrq"), source, 0x03),
        value(&source[8..16])
    );
    // Values with the top bit set stay zero-extended (never sign-extended) in the 64-bit register.
    assert_eq!(
        run_extract(kind_named("pextrb"), bytes([0xff; 16]), 0),
        0xff
    );
    assert_eq!(
        run_extract(kind_named("pextrw"), bytes([0xff; 16]), 0),
        0xffff
    );
    assert_eq!(
        run_extract(kind_named("pextrd"), bytes([0xff; 16]), 0),
        0xffff_ffff
    );
    assert_eq!(
        run_extract(kind_named("pextrq"), bytes([0xff; 16]), 0),
        u64::MAX
    );
}

#[test]
fn movntdqa_loads_sixteen_aligned_bytes_and_has_no_register_form() {
    let kind = kind_named("movntdqa");
    let src = counting(0x21);
    for run in run_xmm(kind, counting(0xa0), src, xmm_pattern(0), 0) {
        assert_eq!(run.xmm, src);
    }
    for code in [
        vec![0x66, 0x0f, 0x38, 0x2a, 0xc1],
        vec![0x66, 0x44, 0x0f, 0x38, 0x2a, 0xcb],
        vec![0x66, 0x41, 0x0f, 0x38, 0x2a, 0xc1],
        vec![0x66, 0x48, 0x0f, 0x38, 0x2a, 0xc1],
    ] {
        let mut cpu = cpu();
        let mut ram = Ram::new(&code);
        let before = cpu.clone();
        assert_eq!(
            step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring0),
            Ok(ud()),
            "{code:02x?}"
        );
        assert_eq!(cpu, before);
        assert!(matches!(
            long_decode::decode(&mut Ram::new(&code), CODE, AccessContext::default()),
            Err(long_decode::DecodeError::Invalid { .. })
        ));
    }
}

// ---- generated data, register pairs and addressing forms ----

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// Mode 0 random bytes; 1 extreme bytes; 2 extreme words; 3 extreme dwords; 4 byte values below 0x90.
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
fn generated_and_boundary_data_agree_with_the_reference_for_every_vector_widen_and_movntdqa_form() {
    let mut rng = Rng(0x55a0_0000_0000_0001);
    let mut cases = 0;
    for kind in &KINDS {
        if matches!(kind.shape, Shape::Insert | Shape::Extract) {
            continue;
        }
        for iteration in 0..600usize {
            let (dest, src) = (rng.vector(iteration % 5), rng.vector((iteration / 5) % 5));
            let mask = rng.vector((iteration / 25) % 5);
            let immediate = (rng.next() % 256) as u8;
            let label = format!(
                "{} {dest:02x?} {src:02x?} {mask:02x?} {immediate:#x}",
                kind.name
            );
            let runs = run_xmm(kind, dest, src, mask, immediate);
            // The memory form sees only the operand bytes; the reference is fed the same view.
            let mut visible = src;
            visible[kind.bytes..].fill(0);
            for (variant, run) in runs.iter().enumerate() {
                let operand = if variant + usize::from(!kind.has_register_form()) == 0 {
                    &src
                } else {
                    &visible
                };
                if kind.kind != Sse41Kind::Ptest {
                    assert_eq!(
                        run.xmm,
                        reference(kind.kind, &dest, operand, &mask, immediate),
                        "{label}"
                    );
                } else {
                    let (zf, cf) = ptest_flags(&dest, operand);
                    assert_eq!(
                        run.rflags & STATUS,
                        (if zf { ZF } else { 0 }) | (if cf { CF } else { 0 }),
                        "{label}"
                    );
                }
                cases += 1;
            }
        }
    }
    assert_eq!(cases, 600 * (17 * 2 + 12 * 2 + 1));
}

#[test]
fn generated_data_agrees_with_the_reference_for_pinsr_and_pextr() {
    let mut rng = Rng(0x55a0_0000_0000_0002);
    for kind in KINDS
        .iter()
        .filter(|kind| matches!(kind.shape, Shape::Insert | Shape::Extract))
    {
        for iteration in 0..400usize {
            let source = rng.vector(iteration % 5);
            let (value, immediate) = (rng.next(), (rng.next() % 256) as u8);
            if kind.shape == Shape::Insert {
                let bits = 8 * kind.bytes as u32;
                let mut want = big(&source);
                put(
                    &mut want,
                    bits,
                    u32::from(immediate) % (128 / bits),
                    u128::from(value) & ((1u128 << bits) - 1),
                );
                for result in run_insert(kind, source, value, immediate) {
                    assert_eq!(result, want.to_le_bytes(), "{} {immediate:#x}", kind.name);
                }
            } else {
                run_extract(kind, source, immediate);
            }
        }
    }
}

#[test]
fn every_register_pair_of_the_register_forms_changes_only_the_destination() {
    let mut cases = 0;
    for (kind_index, kind) in KINDS.iter().enumerate() {
        if !kind.has_register_form() {
            continue;
        }
        for reg in 0..16u8 {
            // For PINSR*/PEXTR* the r/m operand is a general register: rsp (4) and the registers
            // rax..r15 are all legal, so every number is exercised.
            for rm in 0..16u8 {
                let immediate =
                    ((kind_index * 16 + usize::from(reg) * 3 + usize::from(rm)) % 256) as u8;
                let code = reg_code(kind, reg, rm, immediate);
                let mut cpu = cpu();
                let mut ram = Ram::new(&code);
                let (before, ram_before) = (cpu.clone(), ram.bytes.clone());
                let label = format!("{} reg {reg} rm {rm}", kind.name);
                let decoded =
                    long_decode::decode(&mut Ram::new(&code), CODE, AccessContext::default())
                        .unwrap();
                assert_eq!(decoded.length as usize, code.len(), "{label}");
                let Operation::Sse41 {
                    kind: decoded_kind,
                    xmm,
                    operand,
                    immediate: decoded_immediate,
                } = decoded.operation
                else {
                    panic!("{label}: {:?}", decoded.operation);
                };
                let want_operand = if kind.shape == Shape::Insert || kind.shape == Shape::Extract {
                    Sse41Operand::Gpr(rm)
                } else {
                    Sse41Operand::Xmm(rm)
                };
                assert_eq!(
                    (decoded_kind, xmm, operand, decoded_immediate),
                    (
                        kind.kind,
                        reg,
                        want_operand,
                        if kind.immediate { immediate } else { 0 }
                    ),
                    "{label}"
                );
                assert_eq!(
                    step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring0),
                    Ok(StepOutcome::Advanced),
                    "{label}"
                );
                let mut expected = before.clone();
                expected.rip = CODE + code.len() as u64;
                let xmm_of = |state: &LongModeState, index: u8| {
                    state.sse.as_ref().unwrap().xmm[usize::from(index)]
                };
                let (d, s) = (xmm_of(&before, reg), xmm_of(&before, rm));
                let mask = xmm_of(&before, 0);
                match kind.shape {
                    Shape::Vector | Shape::Widen if kind.kind == Sse41Kind::Ptest => {
                        let (zf, cf) = ptest_flags(&d, &s);
                        expected.rflags = (expected.rflags & !STATUS)
                            | if zf { ZF } else { 0 }
                            | if cf { CF } else { 0 };
                    }
                    Shape::Vector | Shape::Widen => {
                        expected.sse.as_mut().unwrap().xmm[usize::from(reg)] =
                            reference(kind.kind, &d, &s, &mask, immediate);
                    }
                    Shape::Insert => {
                        let bits = 8 * kind.bytes as u32;
                        let mut want = big(&d);
                        let value = before.gpr[usize::from(rm)];
                        put(
                            &mut want,
                            bits,
                            u32::from(immediate) % (128 / bits),
                            u128::from(value) & ((1u128 << bits) - 1),
                        );
                        expected.sse.as_mut().unwrap().xmm[usize::from(reg)] = want.to_le_bytes();
                    }
                    Shape::Extract => {
                        let bits = 8 * kind.bytes as u32;
                        expected.gpr[usize::from(rm)] =
                            field(big(&d), bits, u32::from(immediate) % (128 / bits)) as u64;
                    }
                    Shape::Movntdqa => unreachable!(),
                }
                assert_eq!(cpu, expected, "{label}");
                assert_eq!(ram.bytes, ram_before, "{label}");
                assert!(ram.reads.is_empty() && ram.writes.is_empty(), "{label}");
                cases += 1;
            }
        }
    }
    assert_eq!(cases, 36 * 16 * 16);
}

#[test]
fn every_addressing_form_works_for_all_memory_forms() {
    let mut cases = 0;
    for (kind_index, kind) in KINDS.iter().enumerate() {
        for (form_index, form) in FORMS.iter().enumerate() {
            if kind.rex_w_selects() && !kind.rex_w && form.rex & 8 != 0 {
                continue; // REX.W would turn PINSRD/PEXTRD into the quadword form
            }
            let xmm = ((kind_index * 5 + form_index * 3) % 16) as u8;
            let immediate = (kind_index * 7 + form_index) as u8;
            let code = mem_code(kind, form, xmm, immediate);
            let source = memory_pattern((form_index * 40 + kind_index) as u8);
            let mut cpu = setup(form);
            let mut ram = Ram::new(&code);
            let address = form.address as usize;
            ram.bytes[address..address + 16].copy_from_slice(&source);
            let (cpu_before, ram_before) = (cpu.clone(), ram.bytes.clone());
            let label = format!("{} {} xmm{xmm}", kind.name, form.name);
            let decoded =
                long_decode::decode(&mut Ram::new(&code), CODE, AccessContext::default()).unwrap();
            assert_eq!(decoded.length as usize, code.len(), "{label}");
            let Operation::Sse41 {
                kind: decoded_kind,
                xmm: decoded_xmm,
                operand: Sse41Operand::Memory(_),
                immediate: decoded_immediate,
            } = decoded.operation
            else {
                panic!("{label}: {:?}", decoded.operation);
            };
            assert_eq!(
                (decoded_kind, decoded_xmm, decoded_immediate),
                (kind.kind, xmm, if kind.immediate { immediate } else { 0 }),
                "{label}"
            );
            assert_eq!(
                step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring0),
                Ok(StepOutcome::Advanced),
                "{label}"
            );
            let mut expected_cpu = cpu_before.clone();
            expected_cpu.rip = CODE + code.len() as u64;
            let d = cpu_before.sse.as_ref().unwrap().xmm[usize::from(xmm)];
            let mask = cpu_before.sse.as_ref().unwrap().xmm[0];
            let mut visible = source;
            visible[kind.bytes..].fill(0);
            match kind.shape {
                Shape::Extract => {
                    let bits = 8 * kind.bytes as u32;
                    let value = field(big(&d), bits, u32::from(immediate) % (128 / bits)) as u64;
                    let mut expected_ram = ram_before.clone();
                    expected_ram[address..address + kind.bytes]
                        .copy_from_slice(&value.to_le_bytes()[..kind.bytes]);
                    assert_eq!(ram.bytes, expected_ram, "{label}");
                    assert_eq!(ram.writes, [(form.address, kind.bytes)], "{label}");
                    assert_eq!(ram.reads, [], "{label}");
                }
                _ => {
                    if kind.kind == Sse41Kind::Ptest {
                        let (zf, cf) = ptest_flags(&d, &visible);
                        expected_cpu.rflags = (expected_cpu.rflags & !STATUS)
                            | if zf { ZF } else { 0 }
                            | if cf { CF } else { 0 };
                    } else if kind.shape == Shape::Insert {
                        let bits = 8 * kind.bytes as u32;
                        let mut want = big(&d);
                        put(
                            &mut want,
                            bits,
                            u32::from(immediate) % (128 / bits),
                            big(&visible) & ((1u128 << bits) - 1),
                        );
                        expected_cpu.sse.as_mut().unwrap().xmm[usize::from(xmm)] =
                            want.to_le_bytes();
                    } else {
                        expected_cpu.sse.as_mut().unwrap().xmm[usize::from(xmm)] =
                            reference(kind.kind, &d, &visible, &mask, immediate);
                    }
                    assert_eq!(ram.bytes, ram_before, "{label}");
                    assert_eq!(ram.reads, [(form.address, kind.bytes)], "{label}");
                    assert_eq!(ram.writes, [], "{label}");
                }
            }
            // Registers, flags (except PTEST), MXCSR, x87 and the other XMM registers are untouched.
            if kind.shape == Shape::Extract {
                assert_eq!(cpu, expected_cpu, "{label}");
            } else {
                assert_eq!(cpu, expected_cpu, "{label}");
            }
            cases += 1;
        }
    }
    assert!(cases > 37 * 20, "{cases}");
}

#[test]
fn register_forms_ignore_rex_w_null_segment_and_address_overrides_except_where_rex_w_selects() {
    for kind in &KINDS {
        if !kind.has_register_form() {
            continue;
        }
        let base = reg_code(kind, 1, 2, 3);
        let mut variants: Vec<Vec<u8>> = vec![];
        for prefix in [
            &[0x67u8][..],
            &[0x2e],
            &[0x3e],
            &[0x26],
            &[0x36],
            &[0x67, 0x2e],
        ] {
            variants.push([prefix, &base[..]].concat());
        }
        if !kind.rex_w_selects() {
            // 66 REX.W: the width bit is ignored (SDM: only PEXTRD/PEXTRQ and PINSRD/PINSRQ use it).
            variants.push(reg_code_w(kind, 1, 2, 3, true));
        }
        let run = |code: &[u8]| {
            let mut cpu = cpu();
            let mut ram = Ram::new(code);
            let outcome = step(code, &mut cpu, &mut ram, PrivilegeLevel::Ring0);
            (
                outcome,
                cpu.sse.clone(),
                cpu.gpr,
                cpu.rflags,
                cpu.rip - CODE,
            )
        };
        let reference_run = run(&base);
        assert_eq!(reference_run.0, Ok(StepOutcome::Advanced), "{}", kind.name);
        for code in variants {
            let (outcome, xmm, gpr, rflags, advance) = run(&code);
            assert_eq!(
                outcome,
                Ok(StepOutcome::Advanced),
                "{} {code:02x?}",
                kind.name
            );
            assert_eq!(
                (xmm, gpr, rflags),
                (reference_run.1.clone(), reference_run.2, reference_run.3),
                "{} {code:02x?}",
                kind.name
            );
            assert_eq!(advance, code.len() as u64);
        }
    }
}

#[test]
fn rex_w_selects_the_quadword_form_of_pinsr_and_pextr_and_nothing_else() {
    // 66 REX.W 0F 3A 22 /r ib is PINSRQ, without REX.W it is PINSRD; likewise 0F 3A 16 PEXTRQ / PEXTRD.
    for (code, expected) in [
        (
            vec![0x66, 0x48, 0x0f, 0x3a, 0x22, 0xd1, 0x00],
            Sse41Kind::Pinsrq,
        ),
        (vec![0x66, 0x0f, 0x3a, 0x22, 0xd1, 0x00], Sse41Kind::Pinsrd),
        (
            vec![0x66, 0x48, 0x0f, 0x3a, 0x16, 0xd1, 0x00],
            Sse41Kind::Pextrq,
        ),
        (vec![0x66, 0x0f, 0x3a, 0x16, 0xd1, 0x00], Sse41Kind::Pextrd),
        (
            vec![0x66, 0x48, 0x0f, 0x3a, 0x20, 0xd1, 0x00],
            Sse41Kind::Pinsrb,
        ),
        (
            vec![0x66, 0x48, 0x0f, 0x3a, 0x14, 0xd1, 0x00],
            Sse41Kind::Pextrb,
        ),
        (
            vec![0x66, 0x48, 0x0f, 0x3a, 0x15, 0xd1, 0x00],
            Sse41Kind::Pextrw,
        ),
    ] {
        let decoded =
            long_decode::decode(&mut Ram::new(&code), CODE, AccessContext::default()).unwrap();
        assert!(
            matches!(decoded.operation, Operation::Sse41 { kind, .. } if kind == expected),
            "{code:02x?}: {:?}",
            decoded.operation
        );
    }
}

#[test]
fn a_16_byte_memory_operand_that_is_not_16_byte_aligned_is_gp0_for_types_1_and_4_only() {
    for kind in KINDS.iter().filter(|kind| kind.aligned16()) {
        let code = mem_code(kind, &FORMS[0], 1, 3);
        for address in [0x8001u64, 0x8002, 0x8004, 0x8008, 0x800f, 0x8018] {
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
            assert_eq!(cpu, before);
            assert_eq!(ram.bytes, ram_before);
            assert!(ram.reads.is_empty() && ram.writes.is_empty());
        }
    }
    // Type 5 forms accept every alignment at CPL0: one access of the operand width.
    for kind in KINDS.iter().filter(|kind| !kind.aligned16()) {
        let code = mem_code(kind, &FORMS[0], 1, 3);
        for address in [0x8001u64, 0x8002, 0x8003, 0x8005, 0x800f, 0x8ffe] {
            let mut cpu = cpu();
            cpu.gpr[Register::Rax as usize] = address;
            let mut ram = Ram::new(&code);
            assert_eq!(
                step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring0),
                Ok(StepOutcome::Advanced),
                "{} {address:#x}",
                kind.name
            );
            let accesses = if kind.shape == Shape::Extract {
                &ram.writes
            } else {
                &ram.reads
            };
            assert_eq!(*accesses, [(address, kind.bytes)], "{}", kind.name);
        }
    }
}

#[test]
fn at_cpl3_with_the_alignment_check_armed_type_4_is_gp0_and_type_5_unaligned_is_refused_not_guessed()
 {
    const AC: u64 = 1 << 18;
    const CR0_BASE: u64 = 0x8001_0033; // PG | WP | NE | ET | MP | PE: AM = 0
    for kind in &KINDS {
        let code = mem_code(kind, &FORMS[0], 1, 3);
        let aligned16 = [0x8000u64, 0x8010];
        for (address, expected) in [
            (0x8000u64, Ok(StepOutcome::Advanced)),
            (
                0x8001,
                if kind.aligned16() {
                    Ok(gp0())
                } else if kind.bytes == 1 {
                    Ok(StepOutcome::Advanced)
                } else {
                    Ok(StepOutcome::Unsupported {
                        opcode: kind.opcode[2],
                    })
                },
            ),
            (
                0x8008,
                if kind.aligned16() {
                    Ok(gp0())
                } else if kind.bytes <= 8 {
                    Ok(StepOutcome::Advanced)
                } else {
                    unreachable!()
                },
            ),
            (
                0x8002,
                if kind.aligned16() {
                    Ok(gp0())
                } else if kind.bytes <= 2 {
                    Ok(StepOutcome::Advanced)
                } else {
                    Ok(StepOutcome::Unsupported {
                        opcode: kind.opcode[2],
                    })
                },
            ),
        ] {
            assert!(aligned16.contains(&0x8000));
            let mut cpu = cpu();
            cpu.rflags |= AC;
            cpu.gpr[Register::Rax as usize] = address;
            let mut ram = Ram::new(&code);
            let before = cpu.clone();
            let outcome = step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring3);
            assert_eq!(outcome, expected, "{} {address:#x}", kind.name);
            if outcome != Ok(StepOutcome::Advanced) {
                assert_eq!(cpu, before, "{} {address:#x}", kind.name);
                assert!(ram.reads.is_empty() && ram.writes.is_empty());
            }
            // Without the AC flag, or with CR0.AM = 0, a Type 5 operand may be unaligned at CPL3.
            if !kind.aligned16() {
                let mut cpu = self::cpu();
                cpu.gpr[Register::Rax as usize] = address;
                let mut ram = Ram::new(&code);
                assert_eq!(
                    step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring3),
                    Ok(StepOutcome::Advanced),
                    "{} {address:#x} without AC",
                    kind.name
                );
                // CR0.AM = 0 (CR0 carried by the FP-init profile pair): alignment checking is off.
                let mut cpu = fp_init_cpu(CR0_BASE, 0x220);
                cpu.rflags |= AC;
                cpu.gpr[Register::Rax as usize] = address;
                let mut ram = Ram::new(&code);
                let (result, _) = run_fp_init(cpu, &mut ram, PrivilegeLevel::Ring3);
                assert_eq!(
                    result.exit,
                    RunExit::InstructionBudgetExhausted,
                    "{} {address:#x} AM = 0",
                    kind.name
                );
            }
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
                if kind.rex_w {
                    code.push(0x48);
                }
                code.extend_from_slice(&kind.opcode);
                code.extend_from_slice(modrm_tail);
                if kind.immediate {
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
                assert!(ram.reads.is_empty() && ram.writes.is_empty());
            }
        }
    }
}

#[test]
fn an_operand_whose_last_byte_is_non_canonical_faults_before_memory() {
    // The 8-byte PMOVSXBW operand at 0x00007ffffffffffc ends in a non-canonical byte.
    for name in ["pmovsxbw", "pinsrq", "pextrq", "pextrd", "pmovzxwq"] {
        let kind = kind_named(name);
        let code = mem_code(kind, &FORMS[0], 1, 3);
        for address in [
            0x0000_7fff_ffff_fffcu64,
            0x0000_7fff_ffff_fffd,
            0x0000_7fff_ffff_ffff,
        ] {
            if address + kind.bytes as u64 - 1 < 0x0000_8000_0000_0000 {
                continue;
            }
            let mut cpu = cpu();
            cpu.gpr[Register::Rax as usize] = address;
            let mut ram = Ram::new(&code);
            let before = cpu.clone();
            assert_eq!(
                step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring0),
                Ok(gp0()),
                "{name} {address:#x}"
            );
            assert_eq!(cpu, before);
            assert!(ram.reads.is_empty() && ram.writes.is_empty());
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
        // A faulting store is a write fault (error code bit 1), a faulting load a read fault.
        let error_code = if kind.shape == Shape::Extract { 2 } else { 0 };
        assert_eq!(
            step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring0),
            Ok(StepOutcome::Fault(ArchitecturalFault {
                vector: 14,
                error_code: Some(error_code),
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
        let tail = if kind.immediate { vec![0x00] } else { vec![] };
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
fn cr0_em_ts_and_cr4_osfxsr_gate_the_sse41_forms_before_any_address_or_memory_check() {
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
        let mut codes = vec![(mem_code(kind, &FORMS[0], 1, 3), false)];
        if kind.has_register_form() {
            codes.push((reg_code(kind, 1, 2, 3), true));
        }
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
                        None if register_form
                            || address == 0x8000
                            || (address == 0x8001 && !kind.aligned16()) =>
                        {
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
        let mut codes = vec![mem_code(kind, &FORMS[0], 1, 3)];
        if kind.has_register_form() {
            codes.push(reg_code(kind, 1, 2, 3));
        }
        for code in codes {
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
        let tail: &[u8] = if kind.immediate { &[0x04] } else { &[] };
        let name = kind.name;
        // Without the mandatory 66 prefix these opcodes are not SSE4.1 (and not MMX forms): refused.
        refused.push((
            [&kind.opcode[..], &[0x00], tail].concat(),
            format!("{name} no 66 memory"),
        ));
        refused.push((
            [&kind.opcode[..], &[0xc1], tail].concat(),
            format!("{name} no 66 register"),
        ));
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
        // LOCK without the 66 prefix is not claimed either.
        refused.push((
            [&[0xf0][..], &kind.opcode, &[0x00], tail].concat(),
            format!("{name} LOCK no 66"),
        ));
    }
    // (66 0F 38 37 is PCMPGTQ since S2-56 and is covered by s2_56_sse42_popcnt.rs.)
    // Neighbouring opcodes of the same maps that are not integer SSE4.1. The floating-point SSE4.1 forms
    // (BLENDVPS/PD 38/14 and 38/15, ROUNDPS/PD/SS/SD 3A/08..0B, BLENDPS/PD 3A/0C and 3A/0D, EXTRACTPS 3A/17,
    // INSERTPS 3A/21, DPPS 3A/40, DPPD 3A/41) wait for the FP core of S2-37/S2-38 and must stay refused.
    for opcode in [
        0x11u8, 0x12, 0x13, 0x14, 0x15, 0x16, 0x18, 0x19, 0x26, 0x27, 0x2c, 0x2d, 0x36, 0x42,
    ] {
        refused.push((
            vec![0x66, 0x0f, 0x38, opcode, 0x00],
            format!("66 0F 38 {opcode:02x}"),
        ));
    }
    for opcode in [
        0x08u8, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x17, 0x21, 0x40, 0x41, 0x43, 0x44,
    ] {
        refused.push((
            vec![0x66, 0x0f, 0x3a, opcode, 0x00, 0x01],
            format!("66 0F 3A {opcode:02x}"),
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
fn the_decoder_reports_kind_registers_operand_immediate_and_length() {
    let decode = |code: &[u8]| {
        long_decode::decode(&mut Ram::new(code), CODE, AccessContext::default()).unwrap()
    };
    // MPSADBW xmm9, xmm12, 0x1f: REX.R and REX.B extend the registers, the immediate is the last byte.
    let instruction = decode(&[0x66, 0x45, 0x0f, 0x3a, 0x42, 0xcc, 0x1f]);
    assert_eq!(instruction.length, 7);
    assert_eq!(
        instruction.operation,
        Operation::Sse41 {
            kind: Sse41Kind::Mpsadbw,
            xmm: 9,
            operand: Sse41Operand::Xmm(12),
            immediate: 0x1f,
        }
    );
    // PMULLD xmm0, [rax+rcx*4+0x10]: no immediate.
    let instruction = decode(&[0x66, 0x0f, 0x38, 0x40, 0x44, 0x88, 0x10]);
    assert_eq!(instruction.length, 7);
    assert!(matches!(
        instruction.operation,
        Operation::Sse41 {
            kind: Sse41Kind::Pmulld,
            xmm: 0,
            operand: Sse41Operand::Memory(_),
            immediate: 0
        }
    ));
    // PEXTRQ r10, xmm3, 1: REX.W.B, the destination is a general register (REX.B), the source xmm3.
    let instruction = decode(&[0x66, 0x49, 0x0f, 0x3a, 0x16, 0xda, 0x01]);
    assert_eq!(instruction.length, 7);
    assert_eq!(
        instruction.operation,
        Operation::Sse41 {
            kind: Sse41Kind::Pextrq,
            xmm: 3,
            operand: Sse41Operand::Gpr(10),
            immediate: 1,
        }
    );
    // PINSRB xmm15, [rip+0], 2: 66, REX.R, three opcode bytes, ModRM, disp32 and imm8 make 11 bytes.
    let instruction = decode(&[0x66, 0x44, 0x0f, 0x3a, 0x20, 0x3d, 0, 0, 0, 0, 0x02]);
    assert_eq!(instruction.length, 11);
    assert!(matches!(
        instruction.operation,
        Operation::Sse41 {
            kind: Sse41Kind::Pinsrb,
            xmm: 15,
            immediate: 2,
            ..
        }
    ));
    // The opcode map and byte are those of the instruction.
    assert_eq!(instruction.opcode.byte, 0x20);
}

#[test]
fn the_sse41_forms_do_not_disturb_the_neighbouring_ssse3_forms() {
    // PSHUFB (0F 38 00) and PALIGNR (0F 3A 0F) still decode to the SSSE3 operation.
    for code in [
        &[0x66, 0x0f, 0x38, 0x00, 0xc1][..],
        &[0x66, 0x0f, 0x3a, 0x0f, 0xc1, 0x04],
    ] {
        let decoded =
            long_decode::decode(&mut Ram::new(code), CODE, AccessContext::default()).unwrap();
        assert!(
            matches!(decoded.operation, Operation::Ssse3Int { .. }),
            "{code:02x?}"
        );
    }
}
