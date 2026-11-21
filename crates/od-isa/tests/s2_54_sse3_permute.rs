//! S2-54 slice C: the non-floating-point SSE3 forms MOVSHDUP, MOVSLDUP, MOVDDUP, LDDQU and the x87
//! store with truncation FISTTP.
//!
//! Every expectation comes from the Intel SDM text read for this task (combined SDM 325462-093US,
//! cdrdv2.intel.com/v1/dl/getContent/671200):
//!
//! * MOVSHDUP `F3 0F 16 /r` pp. 4-107..4-109, MOVSLDUP `F3 0F 12 /r` pp. 4-110..4-112, MOVDDUP
//!   `F2 0F 12 /r` pp. 4-47..4-49 (source xmm2 or m64, Exceptions Type 5, Table 2-22 p. 2-30) and LDDQU
//!   `F2 0F F0 /r` pp. 3-537..3-538 (source memory only), plus Vol. 2A Table 2-21 (Type 4, p. 2-29):
//!   the legacy-SSE operation copies or duplicates raw lanes (MOVSHDUP: dwords 1,1,3,3; MOVSLDUP:
//!   0,0,2,2; MOVDDUP: qword 0 twice; LDDQU: all 128 bits), touches neither MXCSR nor flags and has no
//!   SIMD floating-point exception; #UD for CR0.EM = 1, CR4.OSFXSR = 0 or LOCK, #NM for CR0.TS = 1,
//!   #SS(0)/#GP(0) for a non-canonical address, #GP(0) for a 128-bit memory operand that is not
//!   16-byte aligned for MOVSHDUP and MOVSLDUP only (Table 2-21 note 1 exempts LDDQU; MOVDDUP is Type 5).
//! * FISTTP `DF /1` m16int, `DB /1` m32int, `DD /1` m64int, Vol. 2A pp. 3-350..3-351 (Table 1-24): the
//!   destination receives ST(0) truncated toward zero and ST(0) is popped; a value too large (or an
//!   infinity or a NaN) is the invalid-operation case and stores the integer indefinite; C1 is cleared
//!   and C0, C2, C3 are undefined; numeric exceptions are Invalid, Stack Invalid and Precision; #NM for
//!   CR0.EM or CR0.TS, #UD for LOCK, #SS(0)/#GP(0) for a non-canonical address.
//!
//! Not asserted here because the SDM text does not define it or the model does not carry it: #AC (the
//! model refuses an unaligned access at CPL3 with the alignment check armed), the CPUID.SSE3 gate (S2-57),
//! the x87 last-opcode register, and unmasked x87 exceptions (a bounded refusal).

use od_isa::execute::{self, Cr4Profile, RunConfig, RunExit, StepOutcome};
use od_isa::long_decode::{self, AddressBase, Operation, Sse3PermuteKind, Sse3Source};
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

struct Perm {
    name: &'static str,
    kind: Sse3PermuteKind,
    prefix: &'static [u8],
    opcode: u8,
    /// Bytes read from a memory source.
    memory_bytes: usize,
    aligned: bool,
    has_register_form: bool,
}

const PERMS: [Perm; 4] = [
    Perm {
        name: "MOVSHDUP",
        kind: Sse3PermuteKind::Movshdup,
        prefix: &[0xf3],
        opcode: 0x16,
        memory_bytes: 16,
        aligned: true,
        has_register_form: true,
    },
    Perm {
        name: "MOVSLDUP",
        kind: Sse3PermuteKind::Movsldup,
        prefix: &[0xf3],
        opcode: 0x12,
        memory_bytes: 16,
        aligned: true,
        has_register_form: true,
    },
    Perm {
        name: "MOVDDUP",
        kind: Sse3PermuteKind::Movddup,
        prefix: &[0xf2],
        opcode: 0x12,
        memory_bytes: 8,
        aligned: false,
        has_register_form: true,
    },
    Perm {
        name: "LDDQU",
        kind: Sse3PermuteKind::Lddqu,
        prefix: &[0xf2],
        opcode: 0xf0,
        memory_bytes: 16,
        aligned: false,
        has_register_form: false,
    },
];

/// The SDM pseudo-code of the legacy-SSE forms, written over 32-bit lanes.
fn expected_permutation(perm: &Perm, source: [u8; 16]) -> [u8; 16] {
    let d: [u32; 4] = core::array::from_fn(|lane| {
        u32::from_le_bytes(source[lane * 4..lane * 4 + 4].try_into().unwrap())
    });
    let result: [u32; 4] = match perm.kind {
        Sse3PermuteKind::Movshdup => [d[1], d[1], d[3], d[3]],
        Sse3PermuteKind::Movsldup => [d[0], d[0], d[2], d[2]],
        Sse3PermuteKind::Movddup => [d[0], d[1], d[0], d[1]],
        Sse3PermuteKind::Lddqu => d,
    };
    let mut out = [0u8; 16];
    for (lane, value) in result.iter().enumerate() {
        out[lane * 4..lane * 4 + 4].copy_from_slice(&value.to_le_bytes());
    }
    out
}

/// Encodes one memory-form instruction: the legacy prefixes (extra, then the mandatory prefix) come
/// first and the REX byte last, immediately before the opcode, as the architecture requires.
fn encode(form: &Form, prefix: &[u8], opcode: &[u8], reg: u8, forced_rex: bool) -> Vec<u8> {
    let mut code = form.extra.to_vec();
    code.extend_from_slice(prefix);
    let rex = form.rex | if reg >= 8 { 0x04 } else { 0 };
    if rex != 0 || forced_rex {
        code.push(0x40 | rex);
    }
    code.extend_from_slice(opcode);
    code.push((form.mode << 6) | ((reg & 7) << 3) | form.rm);
    if let Some(sib) = form.sib {
        code.push(sib);
    }
    match form.disp {
        Disp::None => {}
        Disp::D8(value) => code.push(value as u8),
        Disp::D32(value) => code.extend_from_slice(&value.to_le_bytes()),
        Disp::Rip(target) => {
            let end = CODE + code.len() as u64 + 4;
            code.extend_from_slice(&((target.wrapping_sub(end)) as i32).to_le_bytes());
        }
    }
    code
}

/// `prefix [REX] 0F opcode /r` with a register source (mod = 3).
fn encode_register(perm: &Perm, destination: u8, source: u8) -> Vec<u8> {
    let mut code = perm.prefix.to_vec();
    let rex = (if destination >= 8 { 4 } else { 0 }) | (if source >= 8 { 1 } else { 0 });
    if rex != 0 {
        code.push(0x40 | rex);
    }
    code.extend_from_slice(&[
        0x0f,
        perm.opcode,
        0xc0 | ((destination & 7) << 3) | (source & 7),
    ]);
    code
}

fn operation_of(code: &[u8]) -> Result<Operation, long_decode::DecodeError> {
    long_decode::decode(&mut Ram::new(code), CODE, AccessContext::default())
        .map(|instruction| instruction.operation)
}

// ---- MOVSHDUP / MOVSLDUP / MOVDDUP / LDDQU ----

#[test]
fn the_sdm_lane_rules_on_a_counting_source() {
    // Source bytes 00..0F: dword lanes 03020100, 07060504, 0B0A0908, 0F0E0D0C.
    let source: [u8; 16] = core::array::from_fn(|i| i as u8);
    let cases: [(&Perm, [u8; 16]); 4] = [
        (
            &PERMS[0],
            [4, 5, 6, 7, 4, 5, 6, 7, 12, 13, 14, 15, 12, 13, 14, 15],
        ),
        (
            &PERMS[1],
            [0, 1, 2, 3, 0, 1, 2, 3, 8, 9, 10, 11, 8, 9, 10, 11],
        ),
        (&PERMS[2], [0, 1, 2, 3, 4, 5, 6, 7, 0, 1, 2, 3, 4, 5, 6, 7]),
        (&PERMS[3], source),
    ];
    for (perm, expected) in cases {
        assert_eq!(
            expected_permutation(perm, source),
            expected,
            "{}",
            perm.name
        );
        // Register form (not LDDQU): xmm3 <- xmm5 on a state where xmm5 holds the counting bytes.
        if perm.has_register_form {
            let code = encode_register(perm, 3, 5);
            let mut cpu = cpu();
            cpu.sse.as_mut().unwrap().xmm[5] = source;
            let mut ram = Ram::new(&code);
            assert_eq!(
                step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring0),
                Ok(StepOutcome::Advanced),
                "{}",
                perm.name
            );
            assert_eq!(cpu.sse.as_ref().unwrap().xmm[3], expected, "{}", perm.name);
        }
        // Memory form at [rax].
        let code = [perm.prefix, &[0x0f, perm.opcode, 0x18]].concat(); // xmm3, [rax]
        let mut cpu = cpu();
        cpu.gpr[Register::Rax as usize] = 0x8000;
        let mut ram = Ram::new(&code);
        ram.bytes[0x8000..0x8010].copy_from_slice(&source);
        assert_eq!(
            step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring0),
            Ok(StepOutcome::Advanced),
            "{}",
            perm.name
        );
        assert_eq!(cpu.sse.as_ref().unwrap().xmm[3], expected, "{}", perm.name);
    }
}

#[test]
fn every_addressing_form_loads_and_permutes_for_all_four_instructions() {
    let mut cases = 0;
    for (perm_index, perm) in PERMS.iter().enumerate() {
        for (form_index, form) in FORMS.iter().enumerate() {
            // Every XMM register is exercised over the matrix (REX.R selects XMM8..15).
            let xmm = ((perm_index * 5 + form_index * 3) % 16) as u8;
            let code = encode(form, perm.prefix, &[0x0f, perm.opcode], xmm, false);
            let seed = (form_index * 16 + perm_index) as u8;
            let mut cpu = setup(form);
            let mut ram = Ram::new(&code);
            let address = form.address as usize;
            ram.bytes[address..address + 16].copy_from_slice(&memory_pattern(seed));
            let (cpu_before, ram_before) = (cpu.clone(), ram.bytes.clone());
            let label = format!("{} {} xmm{xmm}", perm.name, form.name);
            let decoded =
                long_decode::decode(&mut Ram::new(&code), CODE, AccessContext::default()).unwrap();
            assert_eq!(decoded.length as usize, code.len(), "{label}");
            let Operation::Sse3Permute {
                kind,
                destination,
                source: Sse3Source::Memory(_),
            } = decoded.operation
            else {
                panic!("{label}: {:?}", decoded.operation);
            };
            assert_eq!((kind, destination), (perm.kind, xmm), "{label}");
            let outcome = step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring0);
            assert_eq!(outcome, Ok(StepOutcome::Advanced), "{label}");
            let mut expected_cpu = cpu_before;
            expected_cpu.rip = CODE + code.len() as u64;
            expected_cpu.sse.as_mut().unwrap().xmm[usize::from(xmm)] =
                expected_permutation(perm, memory_pattern(seed));
            // Registers, flags, MXCSR, x87, the other XMM registers and all memory are untouched.
            assert_eq!(cpu, expected_cpu, "{label}");
            assert_eq!(ram.bytes, ram_before, "{label}");
            assert_eq!(ram.reads, [(form.address, perm.memory_bytes)], "{label}");
            assert_eq!(ram.writes, [], "{label}");
            cases += 1;
        }
    }
    assert_eq!(cases, 4 * FORMS.len());
}

#[test]
fn every_register_pair_of_the_three_register_forms_permutes_only_the_destination() {
    let mut cases = 0;
    for perm in PERMS.iter().filter(|perm| perm.has_register_form) {
        for destination in 0..16u8 {
            for source in 0..16u8 {
                let code = encode_register(perm, destination, source);
                let mut cpu = cpu();
                let mut ram = Ram::new(&code);
                let before = cpu.clone();
                let ram_before = ram.bytes.clone();
                let label = format!("{} xmm{destination}, xmm{source}", perm.name);
                assert_eq!(
                    operation_of(&code),
                    Ok(Operation::Sse3Permute {
                        kind: perm.kind,
                        destination,
                        source: Sse3Source::Register(source),
                    }),
                    "{label}"
                );
                assert_eq!(
                    step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring0),
                    Ok(StepOutcome::Advanced),
                    "{label}"
                );
                let mut expected = before.clone();
                expected.rip = CODE + code.len() as u64;
                expected.sse.as_mut().unwrap().xmm[usize::from(destination)] =
                    expected_permutation(perm, xmm_pattern(usize::from(source)));
                assert_eq!(cpu, expected, "{label}");
                assert_eq!(ram.bytes, ram_before, "{label}");
                assert!(ram.reads.is_empty() && ram.writes.is_empty(), "{label}");
                cases += 1;
            }
        }
    }
    assert_eq!(cases, 3 * 256);
}

#[test]
fn register_forms_ignore_address_prefixes_and_a_rex_w_bit() {
    // MOVSHDUP xmm1, xmm2 with 67, a null segment override and REX.W in front: the extra prefixes do
    // not touch a register operand.
    for code in [
        &[0x67, 0xf3, 0x0f, 0x16, 0xca][..],
        &[0x2e, 0xf3, 0x0f, 0x16, 0xca],
        &[0xf3, 0x48, 0x0f, 0x16, 0xca],
    ] {
        let mut cpu = cpu();
        let mut ram = Ram::new(code);
        let before = cpu.clone();
        assert_eq!(
            step(code, &mut cpu, &mut ram, PrivilegeLevel::Ring0),
            Ok(StepOutcome::Advanced),
            "{code:02x?}"
        );
        let mut expected = before;
        expected.rip = CODE + code.len() as u64;
        expected.sse.as_mut().unwrap().xmm[1] = expected_permutation(&PERMS[0], xmm_pattern(2));
        assert_eq!(cpu, expected, "{code:02x?}");
    }
}

#[test]
fn the_decoder_reports_kind_destination_and_source() {
    // MOVDDUP xmm9, [rbp-0x20]: F2 44 0F 12 4D E0
    let code = [0xf2, 0x44, 0x0f, 0x12, 0x4d, 0xe0];
    let decoded =
        long_decode::decode(&mut Ram::new(&code), CODE, AccessContext::default()).unwrap();
    assert_eq!(decoded.length, 6);
    let Operation::Sse3Permute {
        kind,
        destination,
        source: Sse3Source::Memory(memory),
    } = decoded.operation
    else {
        panic!("{:?}", decoded.operation);
    };
    assert_eq!((kind, destination), (Sse3PermuteKind::Movddup, 9));
    assert_eq!(memory.base, AddressBase::Register(Register::Rbp));
    assert_eq!(memory.displacement, -0x20);
    // MOVSLDUP xmm0, xmm15: F3 41 0F 12 C7 (REX.B selects xmm15)
    assert_eq!(
        operation_of(&[0xf3, 0x41, 0x0f, 0x12, 0xc7]),
        Ok(Operation::Sse3Permute {
            kind: Sse3PermuteKind::Movsldup,
            destination: 0,
            source: Sse3Source::Register(15),
        })
    );
    // A REX before a legacy prefix is dropped: F3 REX.R 66?  Here REX.R then F3 is ineffective.
    assert_eq!(
        operation_of(&[0x44, 0xf3, 0x0f, 0x16, 0xc1]),
        Ok(Operation::Sse3Permute {
            kind: Sse3PermuteKind::Movshdup,
            destination: 0,
            source: Sse3Source::Register(1),
        })
    );
}

#[test]
fn only_the_two_movs_with_a_128_bit_operand_fault_on_misalignment() {
    for perm in &PERMS {
        for offset in 1..16u64 {
            let address = 0x8000 + offset;
            let code = [perm.prefix, &[0x0f, perm.opcode, 0x18]].concat();
            let mut cpu = cpu();
            cpu.gpr[Register::Rax as usize] = address;
            let mut ram = Ram::new(&code);
            ram.bytes[address as usize..address as usize + 16]
                .copy_from_slice(&memory_pattern(offset as u8));
            let (before, ram_before) = (cpu.clone(), ram.bytes.clone());
            let outcome = step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring0);
            if perm.aligned {
                assert_eq!(outcome, Ok(gp0()), "{} +{offset}", perm.name);
                assert_eq!(cpu, before, "{} +{offset}", perm.name);
                assert!(ram.reads.is_empty(), "{} +{offset}", perm.name);
            } else {
                assert_eq!(
                    outcome,
                    Ok(StepOutcome::Advanced),
                    "{} +{offset}",
                    perm.name
                );
                assert_eq!(ram.reads, [(address, perm.memory_bytes)]);
                let mut raw = [0u8; 16];
                raw.copy_from_slice(&ram_before[address as usize..address as usize + 16]);
                assert_eq!(
                    cpu.sse.as_ref().unwrap().xmm[3],
                    expected_permutation(perm, raw),
                    "{} +{offset}",
                    perm.name
                );
            }
            assert_eq!(ram.bytes, ram_before);
        }
    }
}

#[test]
fn the_access_width_decides_where_the_end_of_memory_and_a_page_boundary_matter() {
    // MOVDDUP reads 8 bytes: the last 8 bytes of RAM are readable; the 16-byte forms read past the end.
    for perm in &PERMS {
        let code = [perm.prefix, &[0x0f, perm.opcode, 0x18]].concat();
        let address = (RAM - 8) as u64;
        let mut cpu = cpu();
        cpu.gpr[Register::Rax as usize] = address;
        let mut ram = Ram::new(&code);
        let before = cpu.clone();
        let outcome = step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring0);
        if perm.memory_bytes == 8 {
            assert_eq!(outcome, Ok(StepOutcome::Advanced));
            assert_eq!(ram.reads, [(address, 8)]);
        } else {
            // 16-byte forms: MOVSHDUP/MOVSLDUP fail the alignment check first; LDDQU reaches memory.
            if perm.aligned {
                assert_eq!(outcome, Ok(gp0()), "{}", perm.name);
            } else {
                assert_eq!(
                    outcome,
                    Err(MemoryError::Unmapped {
                        address,
                        access: AccessKind::Read
                    }),
                    "{}",
                    perm.name
                );
            }
            assert_eq!(cpu, before, "{}", perm.name);
        }
    }
    // An unaligned LDDQU across a page boundary is one 16-byte read; a fault on the second page is
    // the backend's #PF and leaves the CPU state untouched.
    let code = [0xf2, 0x0f, 0xf0, 0x18];
    let mut cpu_a = cpu();
    cpu_a.gpr[Register::Rax as usize] = 0x8ff8;
    let mut ram = Ram::new(&code);
    ram.fault_page = Some(0x9000);
    let before = cpu_a.clone();
    assert_eq!(
        step(&code, &mut cpu_a, &mut ram, PrivilegeLevel::Ring0),
        Ok(StepOutcome::Fault(ArchitecturalFault {
            vector: 14,
            error_code: Some(0),
            linear_address: Some(0x9000),
        }))
    );
    assert_eq!(cpu_a, before);
    // MOVDDUP ending exactly at the page boundary does not touch the faulting page.
    let code = [0xf2, 0x0f, 0x12, 0x18];
    let mut cpu_b = cpu();
    cpu_b.gpr[Register::Rax as usize] = 0x8ff8;
    let mut ram = Ram::new(&code);
    ram.fault_page = Some(0x9000);
    assert_eq!(
        step(&code, &mut cpu_b, &mut ram, PrivilegeLevel::Ring0),
        Ok(StepOutcome::Advanced)
    );
    assert_eq!(ram.reads, [(0x8ff8, 8)]);
}

#[test]
fn non_canonical_addresses_fault_before_alignment_and_before_memory() {
    for perm in &PERMS {
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
                let code = [perm.prefix, &[0x0f, perm.opcode], modrm_tail].concat();
                let mut cpu = cpu();
                cpu.gpr[base as usize] = address;
                let mut ram = Ram::new(&code);
                let before = cpu.clone();
                let expected = if base == Register::Rax { gp0() } else { ss0() };
                assert_eq!(
                    step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring0),
                    Ok(expected),
                    "{} {label} {address:#x}",
                    perm.name
                );
                assert_eq!(cpu, before);
                assert!(ram.reads.is_empty());
            }
        }
    }
}

#[test]
fn a_range_that_leaves_the_canonical_space_faults_for_the_16_byte_forms_only() {
    // 0x7fff_ffff_fff8 .. +16 crosses the end of the canonical lower half; +8 bytes does not.
    for perm in PERMS.iter().filter(|perm| !perm.aligned) {
        let code = [perm.prefix, &[0x0f, perm.opcode, 0x18]].concat();
        let mut cpu = cpu();
        cpu.gpr[Register::Rax as usize] = 0x0000_7fff_ffff_fff8;
        let mut ram = Ram::new(&code);
        let before = cpu.clone();
        let outcome = step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring0);
        if perm.memory_bytes == 16 {
            assert_eq!(outcome, Ok(gp0()), "{}", perm.name);
            assert_eq!(cpu, before);
            assert!(ram.reads.is_empty());
        } else {
            // The 8-byte range is canonical, so the access goes to the backend, which has no such
            // address: not an architectural fault of the model.
            assert!(
                matches!(outcome, Err(MemoryError::Unmapped { .. })),
                "{}: {outcome:?}",
                perm.name
            );
        }
    }
}

#[test]
fn backend_faults_leave_cpu_and_memory_untouched() {
    for perm in &PERMS {
        let code = [perm.prefix, &[0x0f, perm.opcode, 0x18]].concat();
        let mut cpu = cpu();
        cpu.gpr[Register::Rax as usize] = 0x8000;
        let mut ram = Ram::new(&code);
        ram.fault_page = Some(0x8000);
        let (before, ram_before) = (cpu.clone(), ram.bytes.clone());
        assert_eq!(
            step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring0),
            Ok(StepOutcome::Fault(ArchitecturalFault {
                vector: 14,
                error_code: Some(0),
                linear_address: Some(0x8000),
            })),
            "{}",
            perm.name
        );
        assert_eq!(cpu, before, "{}", perm.name);
        assert_eq!(ram.bytes, ram_before, "{}", perm.name);
    }
}

#[test]
fn lock_is_ud_for_every_form_and_prefix_order() {
    for perm in &PERMS {
        let mut codes = vec![
            // memory forms
            [&[0xf0][..], perm.prefix, &[0x0f, perm.opcode, 0x00]].concat(),
            [perm.prefix, &[0xf0, 0x0f, perm.opcode, 0x00]].concat(),
            [
                &[0xf0][..],
                perm.prefix,
                &[0x48, 0x0f, perm.opcode, 0x10, 0x20],
            ]
            .concat(),
            [
                &[0xf0][..],
                perm.prefix,
                &[0x0f, perm.opcode, 0x05, 0, 0, 0, 0],
            ]
            .concat(),
        ];
        // register forms
        codes.push([&[0xf0][..], perm.prefix, &[0x0f, perm.opcode, 0xc1]].concat());
        codes.push([perm.prefix, &[0xf0, 0x0f, perm.opcode, 0xc1]].concat());
        for code in codes {
            let mut cpu = cpu();
            let mut ram = Ram::new(&code);
            let (before, ram_before) = (cpu.clone(), ram.bytes.clone());
            assert_eq!(
                step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring0),
                Ok(ud()),
                "{} {code:02x?}",
                perm.name
            );
            assert_eq!(cpu, before);
            assert_eq!(ram.bytes, ram_before);
            assert!(ram.reads.is_empty() && ram.writes.is_empty());
        }
    }
}

#[test]
fn the_register_form_of_lddqu_is_ud() {
    for reg in 0..16u8 {
        for rm in 0..16u8 {
            let mut code = vec![0xf2];
            let rex = (if reg >= 8 { 4 } else { 0 }) | (if rm >= 8 { 1 } else { 0 });
            if rex != 0 {
                code.push(0x40 | rex);
            }
            code.extend_from_slice(&[0x0f, 0xf0, 0xc0 | ((reg & 7) << 3) | (rm & 7)]);
            let mut cpu = cpu();
            let mut ram = Ram::new(&code);
            let before = cpu.clone();
            assert_eq!(
                step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring0),
                Ok(ud()),
                "{code:02x?}"
            );
            assert_eq!(cpu, before);
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
fn cr0_em_ts_and_cr4_osfxsr_gate_the_sse3_forms_before_any_address_or_memory_check() {
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
    for perm in &PERMS {
        let mut codes = vec![[perm.prefix, &[0x0f, perm.opcode, 0x00]].concat()];
        if perm.has_register_form {
            codes.push(encode_register(perm, 1, 2));
        }
        for code in codes {
            let register_form = code.last().is_some_and(|byte| byte >> 6 == 3);
            for (cr0, cr4, vector) in cases {
                for address in [0x8000u64, 0x0000_8000_0000_0000, 0x8001] {
                    let mut cpu = fp_init_cpu(cr0, cr4);
                    cpu.gpr[Register::Rax as usize] = address;
                    let mut ram = Ram::new(&code);
                    let (before, ram_before) = (cpu.clone(), ram.bytes.clone());
                    let (result, cpu) = run_fp_init(cpu, &mut ram, PrivilegeLevel::Ring0);
                    let label = format!(
                        "{} {code:02x?} cr0={cr0:#x} cr4={cr4:#x} {address:#x}",
                        perm.name
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
                        None if address == 0x8001 => {
                            if perm.aligned {
                                assert_eq!(result.exit, fault(13, Some(0)), "{label}");
                            } else {
                                assert_eq!(
                                    result.exit,
                                    RunExit::InstructionBudgetExhausted,
                                    "{label}"
                                );
                            }
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
    for perm in &PERMS {
        let mut codes = vec![[perm.prefix, &[0x0f, perm.opcode, 0x00]].concat()];
        if perm.has_register_form {
            codes.push(encode_register(perm, 1, 2));
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
                    opcode: perm.opcode
                }),
                "{}",
                perm.name
            );
            assert_eq!(cpu_a, before);
            assert!(ram.reads.is_empty() && ram.writes.is_empty());
            // SSE state without x87 state: the permutations do not use the x87 state and run.
            let mut cpu_b = cpu();
            cpu_b.x87 = None;
            cpu_b.gpr[Register::Rax as usize] = 0x8000;
            let mut ram = Ram::new(&code);
            assert_eq!(
                step(&code, &mut cpu_b, &mut ram, PrivilegeLevel::Ring0),
                Ok(StepOutcome::Advanced),
                "{}",
                perm.name
            );
        }
    }
}

#[test]
fn neighbouring_encodings_stay_unsupported_without_change() {
    let refused: &[(&[u8], &str)] = &[
        (
            &[0x66, 0xf3, 0x0f, 0x16, 0x00],
            "66 and F3 together (MOVSHDUP)",
        ),
        (
            &[0xf3, 0x66, 0x0f, 0x12, 0x00],
            "F3 and 66 together (MOVSLDUP)",
        ),
        (
            &[0x66, 0xf2, 0x0f, 0x12, 0x00],
            "66 and F2 together (MOVDDUP)",
        ),
        (
            &[0x66, 0xf2, 0x0f, 0xf0, 0x00],
            "66 and F2 together (LDDQU)",
        ),
        (&[0xf2, 0xf3, 0x0f, 0x12, 0x00], "F2 and F3 together"),
        (&[0xf3, 0xf3, 0x0f, 0x16, 0x00], "duplicate F3"),
        (&[0xf3, 0x0f, 0xf0, 0x00], "F3 0F F0 (no such instruction)"),
        (&[0x0f, 0xf0, 0x00], "0F F0 without prefix"),
        (&[0x66, 0x0f, 0xf0, 0x00], "66 0F F0"),
        (&[0xf2, 0x0f, 0x16, 0x00], "F2 0F 16 (no such instruction)"),
        (&[0xf2, 0x0f, 0x16, 0xc1], "F2 0F 16 register form"),
        (
            &[0x64, 0xf3, 0x0f, 0x16, 0x00],
            "FS override: no segment base in this operand model",
        ),
        (&[0xf2, 0x65, 0x0f, 0xf0, 0x00], "GS override"),
        (
            &[0xf3, 0x64, 0x0f, 0x12, 0xc1],
            "FS override on a register form",
        ),
        (&[0x67, 0x67, 0xf2, 0x0f, 0xf0, 0x00], "duplicate 67"),
        (
            &[0x2e, 0x3e, 0xf2, 0x0f, 0x12, 0x00],
            "two segment overrides",
        ),
        (&[0xf3, 0x48, 0x48, 0x0f, 0x16, 0x00], "duplicate REX"),
        (&[0xf0, 0xf0, 0xf2, 0x0f, 0xf0, 0x00], "duplicate LOCK"),
    ];
    for (code, name) in refused {
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
fn an_unaligned_source_at_cpl3_with_the_alignment_check_armed_is_refused_not_guessed() {
    const AC: u64 = 1 << 18;
    for perm in PERMS.iter().filter(|perm| !perm.aligned) {
        let code = [perm.prefix, &[0x0f, perm.opcode, 0x18]].concat();
        let run = |rflags: u64, address: u64, privilege| {
            let mut cpu = cpu();
            cpu.rflags = rflags;
            cpu.gpr[Register::Rax as usize] = address;
            let mut ram = Ram::new(&code);
            let before = cpu.clone();
            let outcome = step(&code, &mut cpu, &mut ram, privilege);
            (outcome, cpu == before)
        };
        // Misaligned to the access size, AC = 1, CPL3: refused, no state change.
        let (outcome, unchanged) = run(FLAGS | AC, 0x8001, PrivilegeLevel::Ring3);
        assert_eq!(
            outcome,
            Ok(StepOutcome::Unsupported {
                opcode: perm.opcode
            }),
            "{}",
            perm.name
        );
        assert!(unchanged);
        // Aligned, AC clear, or CPL0: runs.
        for (rflags, address, privilege) in [
            (FLAGS | AC, 0x8000, PrivilegeLevel::Ring3),
            (FLAGS, 0x8001, PrivilegeLevel::Ring3),
            (FLAGS | AC, 0x8001, PrivilegeLevel::Ring0),
        ] {
            let (outcome, _) = run(rflags, address, privilege);
            assert_eq!(
                outcome,
                Ok(StepOutcome::Advanced),
                "{} {rflags:#x} {address:#x}",
                perm.name
            );
        }
    }
    // MOVDDUP is checked against its 8-byte access: an 8-aligned, 16-misaligned address runs.
    let code = [0xf2, 0x0f, 0x12, 0x18];
    let mut cpu = cpu();
    cpu.rflags = FLAGS | AC;
    cpu.gpr[Register::Rax as usize] = 0x8008;
    let mut ram = Ram::new(&code);
    assert_eq!(
        step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring3),
        Ok(StepOutcome::Advanced)
    );
}

// ---- FISTTP ----

const IE: u16 = 0x0001;
const SF: u16 = 0x0040;
const PE: u16 = 0x0020;
/// Status word before the instruction apart from TOP: C3, C2, C1, C0 set, ZE and PE already sticky.
const SW_BEFORE: u16 = 0x4700 | 0x0004 | 0x0020;

/// x87 state with ST(0) in physical register `top`, every register tagged valid, all exceptions
/// masked and recognisable instruction and data pointers.
fn x87_with(top: u8, st0: [u8; 10], control_word: u16) -> X87State {
    let mut x87 = X87State::fixture_seed();
    x87.control_word = control_word;
    x87.status_word = (u16::from(top) << 11) | SW_BEFORE;
    x87.tag_word = 0;
    x87.registers[usize::from(top)] = st0;
    x87.instruction_pointer = 0x1111;
    x87.data_pointer = 0x2222;
    x87
}

fn raw80(sign_exponent: u16, mantissa: u64) -> [u8; 10] {
    let mut raw = [0u8; 10];
    raw[..8].copy_from_slice(&mantissa.to_le_bytes());
    raw[8..].copy_from_slice(&sign_exponent.to_le_bytes());
    raw
}

/// A finite value with the integer bit in `mantissa` bit 63 and the given unbiased exponent.
fn fp80(negative: bool, unbiased: i32, mantissa: u64) -> [u8; 10] {
    raw80(
        (u16::from(negative) << 15) | (unbiased + 16383) as u16,
        mantissa,
    )
}

/// The expected outcome for one destination width.
#[derive(Clone, Copy, Debug)]
enum Out {
    /// The truncated integer and the exception bits it raises.
    Int(i64, u16),
    /// The integer indefinite (most negative value) with IE.
    Bad,
}

const HALF: u64 = 1 << 63;

/// (name, ST(0), outcome for m16int, m32int, m64int). Every value is worked out by hand from
/// "truncate toward zero; a value outside the signed range of the destination is invalid".
fn fisttp_cases() -> Vec<(&'static str, [u8; 10], [Out; 3])> {
    use Out::{Bad, Int};
    let all = |value: i64, flags: u16| [Int(value, flags); 3];
    vec![
        ("+0", raw80(0x0000, 0), all(0, 0)),
        ("-0", raw80(0x8000, 0), all(0, 0)),
        ("0.5", fp80(false, -1, HALF), all(0, PE)),
        ("-0.5", fp80(true, -1, HALF), all(0, PE)),
        ("just below 1", fp80(false, -1, u64::MAX), all(0, PE)),
        ("1", fp80(false, 0, HALF), all(1, 0)),
        ("-1", fp80(true, 0, HALF), all(-1, 0)),
        ("1.5", fp80(false, 0, 0xc000_0000_0000_0000), all(1, PE)),
        ("-1.5", fp80(true, 0, 0xc000_0000_0000_0000), all(-1, PE)),
        ("2.5", fp80(false, 1, 0xa000_0000_0000_0000), all(2, PE)),
        ("-2.5", fp80(true, 1, 0xa000_0000_0000_0000), all(-2, PE)),
        ("3", fp80(false, 1, 0xc000_0000_0000_0000), all(3, 0)),
        ("-2.75", fp80(true, 1, 0xb000_0000_0000_0000), all(-2, PE)),
        ("32767.5", fp80(false, 14, 0xffff << 48), all(32767, PE)),
        (
            "32768",
            fp80(false, 15, HALF),
            [Bad, Int(32768, 0), Int(32768, 0)],
        ),
        ("-32768", fp80(true, 15, HALF), all(-32768, 0)),
        (
            "-32768.5",
            fp80(true, 15, HALF | (1 << 47)),
            all(-32768, PE),
        ),
        (
            "-32769",
            fp80(true, 15, 0x8001 << 48),
            [Bad, Int(-32769, 0), Int(-32769, 0)],
        ),
        (
            "2147483647.5",
            fp80(false, 30, 0xffff_ffff << 32),
            [Bad, Int(2147483647, PE), Int(2147483647, PE)],
        ),
        (
            "2147483648",
            fp80(false, 31, HALF),
            [Bad, Bad, Int(2147483648, 0)],
        ),
        (
            "-2147483648",
            fp80(true, 31, HALF),
            [Bad, Int(-2147483648, 0), Int(-2147483648, 0)],
        ),
        (
            "-2147483648.5",
            fp80(true, 31, HALF | (1 << 31)),
            [Bad, Int(-2147483648, PE), Int(-2147483648, PE)],
        ),
        (
            "-2147483649",
            fp80(true, 31, HALF | (1 << 32)),
            [Bad, Bad, Int(-2147483649, 0)],
        ),
        (
            "2^63-1",
            fp80(false, 62, 0xffff_ffff_ffff_fffe),
            [Bad, Bad, Int(i64::MAX, 0)],
        ),
        (
            "2^63-0.5",
            fp80(false, 62, u64::MAX),
            [Bad, Bad, Int(i64::MAX, PE)],
        ),
        ("2^63", fp80(false, 63, HALF), [Bad, Bad, Bad]),
        ("-2^63", fp80(true, 63, HALF), [Bad, Bad, Int(i64::MIN, 0)]),
        ("-(2^63+1)", fp80(true, 63, HALF | 1), [Bad; 3]),
        ("2^64", fp80(false, 64, HALF), [Bad; 3]),
        ("largest finite", raw80(0x7ffe, u64::MAX), [Bad; 3]),
        ("smallest normal", raw80(0x0001, HALF), all(0, PE)),
        ("denormal", raw80(0x0000, 1), all(0, PE)),
        ("negative denormal", raw80(0x8000, 1), all(0, PE)),
        ("pseudo-denormal", raw80(0x0000, HALF), all(0, PE)),
        ("+infinity", raw80(0x7fff, HALF), [Bad; 3]),
        ("-infinity", raw80(0xffff, HALF), [Bad; 3]),
        ("quiet NaN", raw80(0x7fff, 0xc000_0000_0000_0000), [Bad; 3]),
        (
            "signaling NaN",
            raw80(0x7fff, 0xa000_0000_0000_0000),
            [Bad; 3],
        ),
        (
            "negative NaN payload",
            raw80(0xffff, 0xc000_0000_0000_0001),
            [Bad; 3],
        ),
        ("pseudo-NaN", raw80(0x7fff, 0x4000_0000_0000_0000), [Bad; 3]),
        ("pseudo-infinity", raw80(0x7fff, 0), [Bad; 3]),
        ("unnormal", raw80(0x3fff, 0x4000_0000_0000_0000), [Bad; 3]),
        ("pseudo-zero", raw80(0x3fff, 0), [Bad; 3]),
    ]
}

const WIDTHS: [(u8, usize, u32); 3] = [(0xdf, 2, 16), (0xdb, 4, 32), (0xdd, 8, 64)];

fn fisttp_at_rax(opcode: u8) -> Vec<u8> {
    vec![opcode, 0x08] // FISTTP [rax]
}

#[test]
fn fisttp_truncates_every_value_class_for_all_widths_and_control_words() {
    const ADDRESS: u64 = 0x8010;
    let mut runs = 0;
    // RC = nearest, down, up, chop; PC = extended, double, single: truncation ignores both.
    for control_word in [0x037f, 0x077f, 0x0b7f, 0x0f7f, 0x027f, 0x007f] {
        for (case_index, (name, st0, outcomes)) in fisttp_cases().into_iter().enumerate() {
            for (width_index, (opcode, bytes, bits)) in WIDTHS.into_iter().enumerate() {
                let top = (case_index % 8) as u8;
                let code = fisttp_at_rax(opcode);
                let mut cpu = cpu();
                cpu.x87 = Some(x87_with(top, st0, control_word));
                cpu.gpr[Register::Rax as usize] = ADDRESS;
                let mut ram = Ram::new(&code);
                let (before, ram_before) = (cpu.clone(), ram.bytes.clone());
                let label = format!("{name} m{bits} cw={control_word:#x} top={top}");
                assert_eq!(
                    step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring0),
                    Ok(StepOutcome::Advanced),
                    "{label}"
                );
                let (stored, flags) = match outcomes[width_index] {
                    Out::Int(value, flags) => (value as u64, flags),
                    Out::Bad => (1u64 << (bits - 1), IE),
                };
                let mut expected_ram = ram_before;
                expected_ram[ADDRESS as usize..ADDRESS as usize + bytes]
                    .copy_from_slice(&stored.to_le_bytes()[..bytes]);
                assert_eq!(ram.bytes, expected_ram, "{label}");
                assert_eq!(ram.writes, [(ADDRESS, bytes)], "{label}");
                assert_eq!(ram.reads, [], "{label}");
                // ST(0) is popped: its tag is empty and TOP moves up; C1 is cleared, C0/C2/C3 and
                // the sticky bits stay, the new exception bits are ORed in; the registers and the
                // control word do not change; FIP and FDP name the instruction and the operand.
                let mut expected = before;
                expected.rip = CODE + code.len() as u64;
                let x87 = expected.x87.as_mut().unwrap();
                x87.tag_word = 3 << (u16::from(top) * 2);
                x87.status_word = (u16::from((top + 1) & 7) << 11) | 0x4500 | 0x0024 | flags;
                x87.instruction_pointer = CODE;
                x87.data_pointer = ADDRESS;
                assert_eq!(cpu, expected, "{label}");
                runs += 1;
            }
        }
    }
    assert_eq!(runs, 6 * fisttp_cases().len() * 3);
}

#[test]
fn fisttp_stack_underflow_stores_the_integer_indefinite_and_raises_ie_and_sf() {
    const ADDRESS: u64 = 0x8010;
    for (opcode, bytes, bits) in WIDTHS {
        for top in [0u8, 3, 7] {
            let code = fisttp_at_rax(opcode);
            let mut cpu = cpu();
            // A stale value in an empty register must not be converted.
            let mut x87 = x87_with(top, fp80(false, 0, HALF), 0x037f);
            x87.tag_word = 3 << (u16::from(top) * 2);
            cpu.x87 = Some(x87);
            cpu.gpr[Register::Rax as usize] = ADDRESS;
            let mut ram = Ram::new(&code);
            let (before, ram_before) = (cpu.clone(), ram.bytes.clone());
            assert_eq!(
                step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring0),
                Ok(StepOutcome::Advanced),
                "m{bits} top={top}"
            );
            let mut expected_ram = ram_before;
            expected_ram[ADDRESS as usize..ADDRESS as usize + bytes]
                .copy_from_slice(&(1u64 << (bits - 1)).to_le_bytes()[..bytes]);
            assert_eq!(ram.bytes, expected_ram, "m{bits}");
            let mut expected = before;
            expected.rip = CODE + code.len() as u64;
            let x87 = expected.x87.as_mut().unwrap();
            x87.status_word = (u16::from((top + 1) & 7) << 11) | 0x4500 | 0x0024 | IE | SF;
            x87.instruction_pointer = CODE;
            x87.data_pointer = ADDRESS;
            assert_eq!(cpu, expected, "m{bits} top={top}");
        }
    }
}

#[test]
fn every_addressing_form_stores_with_truncation_for_all_three_widths() {
    let mut cases = 0;
    for (width_index, (opcode, bytes, _)) in WIDTHS.into_iter().enumerate() {
        for (form_index, form) in FORMS.iter().enumerate() {
            // -2.75 -> -2 with PE; every form must name the same address as the loads of S2-34.
            let code = encode(form, &[], &[opcode], 1, false);
            let top = ((form_index + width_index) % 8) as u8;
            let mut cpu = setup(form);
            cpu.x87 = Some(x87_with(top, fp80(true, 1, 0xb000_0000_0000_0000), 0x037f));
            let mut ram = Ram::new(&code);
            let (before, ram_before) = (cpu.clone(), ram.bytes.clone());
            let label = format!("m{} {}", bytes * 8, form.name);
            let decoded =
                long_decode::decode(&mut Ram::new(&code), CODE, AccessContext::default()).unwrap();
            assert_eq!(decoded.length as usize, code.len(), "{label}");
            let Operation::FisttpMemory { memory } = decoded.operation else {
                panic!("{label}: {:?}", decoded.operation);
            };
            assert_eq!(memory.size.bytes(), bytes, "{label}");
            assert_eq!(
                step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring0),
                Ok(StepOutcome::Advanced),
                "{label}"
            );
            let mut expected_ram = ram_before;
            let address = form.address as usize;
            expected_ram[address..address + bytes]
                .copy_from_slice(&((-2i64) as u64).to_le_bytes()[..bytes]);
            assert_eq!(ram.bytes, expected_ram, "{label}");
            assert_eq!(ram.writes, [(form.address, bytes)], "{label}");
            let mut expected = before;
            expected.rip = CODE + code.len() as u64;
            let x87 = expected.x87.as_mut().unwrap();
            x87.tag_word = 3 << (u16::from(top) * 2);
            x87.status_word = (u16::from((top + 1) & 7) << 11) | 0x4500 | 0x0024 | PE;
            x87.instruction_pointer = CODE;
            x87.data_pointer = form.address;
            assert_eq!(cpu, expected, "{label}");
            cases += 1;
        }
    }
    assert_eq!(cases, 3 * FORMS.len());
}

#[test]
fn the_decoder_reports_the_width_and_leaves_the_fist_forms_alone() {
    for (code, bytes) in [
        (&[0xdf, 0x0b][..], 2),
        (&[0xdb, 0x4d, 0xe0], 4),
        (&[0xdd, 0x0c, 0x88], 8),
        (&[0x41, 0xdd, 0x0c, 0x24], 8),
    ] {
        match operation_of(code) {
            Ok(Operation::FisttpMemory { memory }) => {
                assert_eq!(memory.size.bytes(), bytes, "{code:02x?}")
            }
            other => panic!("{code:02x?}: {other:?}"),
        }
    }
    // The bounded FIST/FISTP [rax] forms keep their own operations.
    for (code, expected) in [
        (&[0xdb, 0x10][..], Operation::FistM32IntRax),
        (&[0xdb, 0x18], Operation::FistpM32IntRax),
        (&[0xdf, 0x10], Operation::FistM16IntRax),
        (&[0xdf, 0x18], Operation::FistpM16IntRax),
        (&[0xdf, 0x38], Operation::FistpM64IntRax),
    ] {
        assert_eq!(operation_of(code), Ok(expected), "{code:02x?}");
    }
    // The register forms (mod = 3) are not FISTTP.
    for code in [[0xdd, 0xc8], [0xdf, 0xc8], [0xdb, 0xc8]] {
        assert!(
            !matches!(operation_of(&code), Ok(Operation::FisttpMemory { .. })),
            "{code:02x?}"
        );
    }
}

#[test]
fn fisttp_masks_matter_only_for_the_exceptions_it_would_raise() {
    const ADDRESS: u64 = 0x8010;
    let exact = fp80(false, 0, HALF); // 1
    let inexact = fp80(false, 0, 0xc000_0000_0000_0000); // 1.5
    let invalid = raw80(0x7fff, HALF); // +infinity
    // (control word, ST(0), tag empty, status word bits to add, runs?)
    let cases: [(u16, [u8; 10], bool, u16, bool, &str); 11] = [
        (0x037e, invalid, false, 0, false, "IM unmasked, invalid"),
        (
            0x037e,
            exact,
            true,
            0,
            false,
            "IM unmasked, stack underflow",
        ),
        (0x035f, inexact, false, 0, false, "PM unmasked, inexact"),
        (
            0x035f,
            exact,
            false,
            0,
            false,
            "PM unmasked with the old sticky PE armed (unmasked pending exception)",
        ),
        (0x037e, inexact, false, 0, true, "IM unmasked, only inexact"),
        (0x037e, exact, false, 0, true, "IM unmasked, exact"),
        (
            0x0360,
            inexact,
            false,
            0,
            false,
            "ZM unmasked with the old sticky ZE armed (unmasked pending exception)",
        ),
        (
            0x037e,
            exact,
            false,
            IE,
            false,
            "IM unmasked with IE armed (unmasked pending exception)",
        ),
        (
            0x037f,
            exact,
            false,
            0x0080,
            false,
            "pending exception (ES)",
        ),
        (0x037f, exact, false, 0x8000, false, "pending exception (B)"),
        (
            0x037f,
            exact,
            false,
            0x0001,
            true,
            "an old sticky IE is not a pending exception",
        ),
    ];
    for (control_word, st0, empty, status_bits, runs, name) in cases {
        let code = fisttp_at_rax(0xdb);
        let mut cpu = cpu();
        let mut x87 = x87_with(2, st0, control_word);
        x87.status_word |= status_bits;
        if empty {
            x87.tag_word = 3 << 4;
        }
        cpu.x87 = Some(x87);
        cpu.gpr[Register::Rax as usize] = ADDRESS;
        let mut ram = Ram::new(&code);
        let (before, ram_before) = (cpu.clone(), ram.bytes.clone());
        let outcome = step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring0);
        if runs {
            assert_eq!(outcome, Ok(StepOutcome::Advanced), "{name}");
        } else {
            assert_eq!(
                outcome,
                Ok(StepOutcome::Unsupported { opcode: 0xdb }),
                "{name}"
            );
            assert_eq!(cpu, before, "{name}");
            assert_eq!(ram.bytes, ram_before, "{name}");
            assert!(ram.reads.is_empty() && ram.writes.is_empty(), "{name}");
        }
    }
}

#[test]
fn fisttp_runs_when_the_armed_sticky_flags_are_masked_or_cleared() {
    const ADDRESS: u64 = 0x8010;
    let exact = fp80(false, 0, HALF); // 1
    let inexact = fp80(false, 0, 0xc000_0000_0000_0000); // 1.5
    // (control word, ST(0), status word bits to clear, name)
    let cases: [(u16, [u8; 10], u16, &str); 3] = [
        (0x035f, exact, PE, "PM unmasked, sticky PE cleared"),
        (
            0x0360,
            inexact,
            0x0004 | PE,
            "DM ZM OM UM unmasked, ZE and PE cleared, PM masked",
        ),
        (
            0x037f,
            exact,
            0,
            "every exception masked, sticky flags kept",
        ),
    ];
    for (control_word, st0, clear, name) in cases {
        let code = fisttp_at_rax(0xdb);
        let mut cpu = cpu();
        let mut x87 = x87_with(2, st0, control_word);
        x87.status_word &= !clear;
        cpu.x87 = Some(x87);
        cpu.gpr[Register::Rax as usize] = ADDRESS;
        let mut ram = Ram::new(&code);
        let outcome = step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring0);
        assert_eq!(outcome, Ok(StepOutcome::Advanced), "{name}");
    }
}

#[test]
fn fisttp_address_faults_precede_the_write_and_leave_the_state_untouched() {
    for (opcode, _, bits) in WIDTHS {
        for address in [
            0x0000_8000_0000_0000u64,
            0x0000_8000_0000_0001,
            0xffff_7fff_ffff_ffff,
        ] {
            for (base, tail, expected) in [
                (Register::Rax, &[0x08][..], gp0()),
                (Register::Rbp, &[0x4d, 0x00], ss0()),
                (Register::Rsp, &[0x0c, 0x24], ss0()),
            ] {
                let code = [&[opcode][..], tail].concat();
                let mut cpu = cpu();
                cpu.x87 = Some(x87_with(1, fp80(false, 0, HALF), 0x037f));
                cpu.gpr[base as usize] = address;
                let mut ram = Ram::new(&code);
                let (before, ram_before) = (cpu.clone(), ram.bytes.clone());
                assert_eq!(
                    step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring0),
                    Ok(expected),
                    "m{bits} {base:?} {address:#x}"
                );
                assert_eq!(cpu, before);
                assert_eq!(ram.bytes, ram_before);
            }
        }
        // A store that ends exactly at the faulting page does not touch it; one that crosses into it
        // is the backend's #PF (write error code) and leaves the state and memory untouched.
        let code = fisttp_at_rax(opcode);
        let mut cpu_b = cpu();
        cpu_b.x87 = Some(x87_with(1, fp80(false, 0, HALF), 0x037f));
        cpu_b.gpr[Register::Rax as usize] = 0x8ffd;
        let mut ram = Ram::new(&code);
        ram.fault_page = Some(0x9000);
        let before_b = cpu_b.clone();
        let ram_before = ram.bytes.clone();
        let crossing = 0x8ffd + (bits / 8) as u64 > 0x9000;
        let outcome = step(&code, &mut cpu_b, &mut ram, PrivilegeLevel::Ring0);
        if crossing {
            assert_eq!(
                outcome,
                Ok(StepOutcome::Fault(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(2),
                    linear_address: Some(0x9000),
                })),
                "m{bits}"
            );
            assert_eq!(cpu_b, before_b);
            assert_eq!(ram.bytes, ram_before);
        } else {
            assert_eq!(outcome, Ok(StepOutcome::Advanced), "m{bits}");
        }
    }
}

#[test]
fn lock_before_fisttp_is_ud_and_other_prefixes_are_refused() {
    for opcode in [0xdf, 0xdb, 0xdd] {
        for code in [
            vec![0xf0, opcode, 0x08],
            vec![0xf0, 0x48, opcode, 0x08],
            vec![0xf0, opcode, 0x0d, 0, 0, 0, 0],
            vec![0x67, 0xf0, opcode, 0x08],
        ] {
            let mut cpu = cpu();
            let mut ram = Ram::new(&code);
            let (before, ram_before) = (cpu.clone(), ram.bytes.clone());
            assert_eq!(
                step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring0),
                Ok(ud()),
                "{code:02x?}"
            );
            assert_eq!(cpu, before);
            assert_eq!(ram.bytes, ram_before);
        }
        for (code, name) in [
            (vec![0x66, opcode, 0x08], "66 prefix"),
            (vec![0xf3, opcode, 0x08], "F3 prefix"),
            (vec![0xf2, opcode, 0x08], "F2 prefix"),
            (vec![0x64, opcode, 0x08], "FS override"),
            (vec![0x65, opcode, 0x08], "GS override"),
            (vec![0x2e, 0x3e, opcode, 0x08], "two segment overrides"),
            (vec![0x48, 0x48, opcode, 0x08], "duplicate REX"),
            (vec![0x67, 0x67, opcode, 0x08], "duplicate 67"),
            (vec![0xf0, 0xf0, opcode, 0x08], "duplicate LOCK"),
        ] {
            let mut cpu = cpu();
            cpu.x87 = Some(x87_with(1, fp80(false, 0, HALF), 0x037f));
            cpu.gpr[Register::Rax as usize] = 0x8010;
            let mut ram = Ram::new(&code);
            let before = cpu.clone();
            let outcome = step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring0);
            assert!(
                matches!(outcome, Ok(StepOutcome::Unsupported { .. })),
                "{name} {opcode:#x}: {outcome:?}"
            );
            assert_eq!(cpu, before, "{name}");
            assert!(ram.writes.is_empty(), "{name}");
        }
    }
}

#[test]
fn fisttp_is_gated_by_cr0_em_and_ts_not_by_osfxsr_and_needs_x87_state() {
    const CR0_BASE: u64 = 0x8001_0033;
    const CR4_PAE: u64 = 0x20;
    const CR4_OSFXSR: u64 = 0x200;
    let cases: [(u64, u64, bool); 6] = [
        (CR0_BASE, CR4_PAE | CR4_OSFXSR, true),
        (CR0_BASE | 4, CR4_PAE | CR4_OSFXSR, false),
        (CR0_BASE | 8, CR4_PAE | CR4_OSFXSR, false),
        (CR0_BASE | 4 | 8, CR4_PAE | CR4_OSFXSR, false),
        // OSFXSR concerns SSE state only: x87 stores run without it.
        (CR0_BASE, CR4_PAE, true),
        (CR0_BASE | 8, CR4_PAE, false),
    ];
    for (opcode, _, _) in WIDTHS {
        let code = fisttp_at_rax(opcode);
        for (cr0, cr4, runs) in cases {
            for address in [0x8000u64, 0x0000_8000_0000_0000] {
                let mut cpu = fp_init_cpu(cr0, cr4);
                cpu.gpr[Register::Rax as usize] = address;
                let mut ram = Ram::new(&code);
                let (before, ram_before) = (cpu.clone(), ram.bytes.clone());
                let (result, cpu) = run_fp_init(cpu, &mut ram, PrivilegeLevel::Ring0);
                let label = format!("{opcode:#x} cr0={cr0:#x} cr4={cr4:#x} {address:#x}");
                let fault = |vector, error_code| {
                    RunExit::Fault(ArchitecturalFault {
                        vector,
                        error_code,
                        linear_address: None,
                    })
                };
                if !runs {
                    // #NM precedes the non-canonical #GP.
                    assert_eq!(result.exit, fault(7, None), "{label}");
                    assert_eq!(result.instructions_executed, 0, "{label}");
                    assert_eq!(cpu, before, "{label}");
                    assert_eq!(ram.bytes, ram_before, "{label}");
                } else if address == 0x8000 {
                    assert_eq!(result.exit, RunExit::InstructionBudgetExhausted, "{label}");
                    assert_eq!(result.instructions_executed, 1, "{label}");
                } else {
                    assert_eq!(result.exit, fault(13, Some(0)), "{label}");
                }
            }
        }
        // Without x87 state the form is refused without change (SSE state is irrelevant).
        let mut cpu_a = cpu();
        cpu_a.x87 = None;
        cpu_a.gpr[Register::Rax as usize] = 0x8000;
        let mut ram = Ram::new(&code);
        let before = cpu_a.clone();
        assert_eq!(
            step(&code, &mut cpu_a, &mut ram, PrivilegeLevel::Ring0),
            Ok(StepOutcome::Unsupported { opcode }),
        );
        assert_eq!(cpu_a, before);
        let mut cpu_b = cpu();
        cpu_b.sse = None;
        cpu_b.gpr[Register::Rax as usize] = 0x8000;
        cpu_b.x87 = Some(x87_with(1, fp80(false, 0, HALF), 0x037f));
        let mut ram = Ram::new(&code);
        assert_eq!(
            step(&code, &mut cpu_b, &mut ram, PrivilegeLevel::Ring0),
            Ok(StepOutcome::Advanced)
        );
    }
}

#[test]
fn an_unaligned_fisttp_destination_at_cpl3_with_the_alignment_check_armed_is_refused() {
    const AC: u64 = 1 << 18;
    for (opcode, bytes, bits) in WIDTHS {
        let code = fisttp_at_rax(opcode);
        let run = |rflags: u64, address: u64, privilege| {
            let mut cpu = cpu();
            cpu.x87 = Some(x87_with(1, fp80(false, 0, HALF), 0x037f));
            cpu.rflags = rflags;
            cpu.gpr[Register::Rax as usize] = address;
            let mut ram = Ram::new(&code);
            let before = cpu.clone();
            let outcome = step(&code, &mut cpu, &mut ram, privilege);
            (outcome, cpu == before)
        };
        let misaligned = 0x8000 + bytes as u64 / 2;
        let (outcome, unchanged) = run(FLAGS | AC, misaligned, PrivilegeLevel::Ring3);
        assert_eq!(outcome, Ok(StepOutcome::Unsupported { opcode }), "m{bits}");
        assert!(unchanged);
        for (rflags, address, privilege) in [
            (FLAGS | AC, 0x8000, PrivilegeLevel::Ring3),
            (FLAGS, misaligned, PrivilegeLevel::Ring3),
            (FLAGS | AC, misaligned, PrivilegeLevel::Ring0),
        ] {
            let (outcome, _) = run(rflags, address, privilege);
            assert_eq!(
                outcome,
                Ok(StepOutcome::Advanced),
                "m{bits} {rflags:#x} {address:#x}"
            );
        }
    }
}
