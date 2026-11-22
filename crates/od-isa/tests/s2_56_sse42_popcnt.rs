//! S2-56 slice A: PCMPGTQ (SSE4.2), POPCNT and CRC32 (CRC-32C), the integer forms without the string
//! compare core.
//!
//! Every expectation comes from the Intel SDM text read for this task (combined SDM 325462-093US,
//! cdrdv2.intel.com/v1/dl/getContent/671200, Vol. 2A/2B, text through pdftotext):
//!
//! * PCMPGTQ (Vol. 2B pp. 4-264..4-266): "66 0F 38 37 /r"; per quadword, DEST = all ones if DEST > SRC as
//!   signed integers, else zero; Flags Affected: None; "Other Exceptions: Non-EVEX-encoded instruction, see
//!   Table 2-21, Type 4 Class Exception Conditions" (Vol. 2A): #UD for CR0.EM = 1, CR4.OSFXSR = 0 or a LOCK
//!   prefix, #NM for CR0.TS = 1, #SS(0)/#GP(0) for a non-canonical address, #GP(0) for a 128-bit memory
//!   operand that is not 16-byte aligned.
//! * POPCNT (Vol. 2B pp. 4-405..4-406): "F3 0F B8 /r" (r16, r32; "F3 REX.W" r64); the destination is the
//!   count of set bits of the source; "OF, SF, ZF, AF, CF, PF are all cleared. ZF is set if SRC = 0,
//!   otherwise ZF is cleared"; 64-bit mode exceptions: #GP(0)/#SS(0) non-canonical address, #PF, #AC(0) at
//!   CPL3 with alignment checking, #UD if CPUID.01H:ECX.POPCNT[23] = 0 or a LOCK prefix. No CR0/CR4 gate.
//! * CRC32 (Vol. 2A pp. 3-205..3-207): "F2 0F 38 F0 /r" r32, r/m8; "F2 0F 38 F1 /r" r32, r/m16 (with 66) and
//!   r32, r/m32; "F2 REX.W 0F 38 F0 /r" r64, r/m8; "F2 REX.W 0F 38 F1 /r" r64, r/m64; the destination low
//!   dword is the initial CRC, the result is the CRC-32C accumulation (polynomial 11EDC6F41H, data and
//!   result in reflected bit order) stored in the low dword with the high dword zeroed; Flags Affected:
//!   None; exceptions as for POPCNT with CPUID.01H:ECX.SSE4_2[20]. The expected values of the matrix
//!   tests come from `sdm_crc32`, a literal transcription of the SDM "Operation" (BIT_REFLECT, the
//!   shifts and the MOD2 division by 11EDC6F41H) on 128-bit integers, which shares no code with the model.
//!   Known values: RFC 3720 (iSCSI) section 12.1 (generator 0x11edc6f41, initial value all ones, final
//!   complement) and Appendix B.4 (CRC examples: 32 zero bytes aa 36 91 8a, 32 bytes 0xff 43 ab a8 62,
//!   00..1f 4e 79 dd 46, 1f..00 5c db 3f 11, Read (10) PDU 56 3a 96 d9, the bytes in transmission order, so
//!   the 32-bit values below are the little-endian words), and the CRC catalogue check value 0xE3069283 of
//!   "123456789"; every one of them was also computed by the CRC32 instruction of the physical oracle
//!   (Intel Core i5-3210M, work/stage8/s2_56a_oracle, 10023 cases, the table `HARDWARE_STEPS` below).
//!
//! Not asserted here because the SDM text does not define it or the model does not carry it: the CPUID
//! gates (POPCNT, SSE4.2: S2-57 owns CPUID), `#AC` at CPL3 (an operand not aligned to its size at CPL3 with
//! RFLAGS.AC = 1 is refused as `Unsupported` instead of guessed; the physical oracle raises SIGBUS there),
//! and the string forms PCMPESTRI/PCMPESTRM/PCMPISTRI/PCMPISTRM (a later slice).

use od_isa::execute::{
    self, ControlRegisterPolicy, Cpl0ProfileSet, Cpl0ProfileSetExit, Cr0Profile, Cr4Profile,
    RunConfig, RunExit, StepOutcome, run_with_cpl0_profile_set, run_with_rflags_defined,
};
use od_isa::long_decode::{self, Operand, Operation, Sse41Kind, Sse41Operand};
use od_isa::state::{SseState, X87State};
use od_isa::{
    AccessContext, AccessKind, ArchitecturalFault, CpuActivity, GuestMemory, LongModeState,
    MemoryError, OperandSize, PrivilegeLevel, Register, StiShadow,
};

const CODE: u64 = 0x2000;
const RAM: usize = 0x10000;
const SENTINEL: u8 = 0xcc;
const CF: u64 = 1;
const PF: u64 = 1 << 2;
const AF: u64 = 1 << 4;
const ZF: u64 = 1 << 6;
const SF: u64 = 1 << 7;
const OF: u64 = 1 << 11;
const DF: u64 = 1 << 10;
const IF: u64 = 1 << 9;
const AC: u64 = 1 << 18;
const STATUS: u64 = CF | PF | AF | ZF | SF | OF;
const FLAGS: u64 = 0x2 | STATUS | DF | IF;
const MXCSR: u32 = 0x1fa5;

struct Ram {
    bytes: Vec<u8>,
    reads: Vec<(u64, usize)>,
    writes: Vec<(u64, usize)>,
    /// Any data access that touches this page raises the #PF below.
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
        usize::try_from(address)
            .ok()
            .filter(|start| start.checked_add(length).is_some_and(|end| end <= RAM))
            .ok_or(MemoryError::Unmapped { address, access })
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

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}

/// Values at the ends of every width: zero, one, the sign bits, all ones, single bytes.
const BOUNDARY: [u64; 16] = [
    0,
    1,
    0x7f,
    0x80,
    0xff,
    0x7fff,
    0x8000,
    0xffff,
    0x7fff_ffff,
    0x8000_0000,
    0xffff_ffff,
    0x0001_0000_0000,
    0x7fff_ffff_ffff_ffff,
    0x8000_0000_0000_0000,
    0x8000_0000_0000_0001,
    0xffff_ffff_ffff_ffff,
];

fn xmm_pattern(index: usize) -> [u8; 16] {
    core::array::from_fn(|lane| {
        (index as u8).wrapping_mul(0x11) ^ (lane as u8).wrapping_mul(7) ^ 0x35
    })
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

fn ctx(privilege: PrivilegeLevel) -> AccessContext {
    AccessContext {
        privilege,
        ..AccessContext::default()
    }
}

fn step(
    cpu: &mut LongModeState,
    ram: &mut Ram,
    privilege: PrivilegeLevel,
) -> Result<StepOutcome, MemoryError> {
    execute::step(cpu, ram, ctx(privilege))
}

/// Runs the instruction at CODE (already in `ram`) at Ring0 on `cpu` and returns the outcome and the new state.
fn exec(
    mut cpu: LongModeState,
    ram: &mut Ram,
) -> (Result<StepOutcome, MemoryError>, LongModeState) {
    let outcome = step(&mut cpu, ram, PrivilegeLevel::Ring0);
    (outcome, cpu)
}

fn fault(vector: u8, error_code: Option<u32>) -> StepOutcome {
    StepOutcome::Fault(ArchitecturalFault {
        vector,
        error_code,
        linear_address: None,
    })
}

fn gp0() -> StepOutcome {
    fault(13, Some(0))
}

fn ss0() -> StepOutcome {
    fault(12, Some(0))
}

fn ud() -> StepOutcome {
    fault(6, None)
}

// ---- encodings ----

enum Disp {
    None,
    D8(i8),
    D32(i32),
    /// RIP-relative: the absolute target; disp32 is computed from the end of the instruction.
    Rip(u64),
}

/// One memory addressing form: the registers that make the effective address and the effective address
/// that the SDM rules (Vol. 1 3.7.5, Vol. 2A 2.1.5 and 2.2.1.6) give for them.
struct Form {
    name: &'static str,
    /// Legacy prefixes in front of everything else (67, null segment overrides).
    extra: &'static [u8],
    /// REX bits X/B (W and R come from the form under test); 0 means no REX byte unless needed.
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
        regs: &[(Register::Rax, 0x8400), (Register::Rcx, 4)],
        address: 0x8420,
    },
    Form {
        name: "[r8]",
        extra: &[],
        rex: 1,
        mode: 0,
        rm: 0,
        sib: None,
        disp: Disp::None,
        regs: &[(Register::R8, 0x8540)],
        address: 0x8540,
    },
    Form {
        name: "[r13+0x10]",
        extra: &[],
        rex: 1,
        mode: 1,
        rm: 5,
        sib: None,
        disp: Disp::D8(0x10),
        regs: &[(Register::R13, 0x8550)],
        address: 0x8560,
    },
    Form {
        name: "[r12] (SIB)",
        extra: &[],
        rex: 1,
        mode: 0,
        rm: 4,
        sib: Some(0x24),
        disp: Disp::None,
        regs: &[(Register::R12, 0x8570)],
        address: 0x8570,
    },
    Form {
        name: "[rbx+r9*8+0x20]",
        extra: &[],
        rex: 2,
        mode: 1,
        rm: 4,
        sib: Some(0xcb),
        disp: Disp::D8(0x20),
        regs: &[(Register::Rbx, 0x8600), (Register::R9, 2)],
        address: 0x8630,
    },
    Form {
        name: "[disp32] (SIB, no base)",
        extra: &[],
        rex: 0,
        mode: 0,
        rm: 4,
        sib: Some(0x25),
        disp: Disp::D32(0x8700),
        regs: &[],
        address: 0x8700,
    },
    Form {
        name: "[r9*4+disp32]",
        extra: &[],
        rex: 2,
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
        disp: Disp::Rip(0x8800),
        regs: &[],
        address: 0x8800,
    },
    Form {
        name: "[rip+disp32] backward",
        extra: &[],
        rex: 0,
        mode: 0,
        rm: 5,
        sib: None,
        disp: Disp::Rip(0x1800),
        regs: &[],
        address: 0x1800,
    },
    Form {
        name: "[eax] 67",
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
        name: "[rax] cs override",
        extra: &[0x2e],
        rex: 0,
        mode: 0,
        rm: 0,
        sib: None,
        disp: Disp::None,
        regs: &[(Register::Rax, 0x8b30)],
        address: 0x8b30,
    },
    Form {
        name: "[rax] 67 + ds override",
        extra: &[0x67, 0x3e],
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

/// `extra` + `prefix` + REX + `opcode` + ModRM(`reg`, memory form) + SIB + displacement.
fn mem_code(prefix: &[u8], rex_w: bool, opcode: &[u8], reg: u8, form: &Form) -> Vec<u8> {
    let rex = form.rex | if rex_w { 8 } else { 0 } | if reg >= 8 { 4 } else { 0 };
    let mut code = form.extra.to_vec();
    code.extend_from_slice(prefix);
    if rex != 0 {
        code.push(0x40 | rex);
    }
    code.extend_from_slice(opcode);
    code.push(form.mode << 6 | (reg & 7) << 3 | form.rm);
    if let Some(sib) = form.sib {
        code.push(sib);
    }
    match form.disp {
        Disp::None => {}
        Disp::D8(value) => code.push(value as u8),
        Disp::D32(value) => code.extend_from_slice(&value.to_le_bytes()),
        Disp::Rip(target) => {
            let end = CODE + code.len() as u64 + 4;
            code.extend_from_slice(&(target.wrapping_sub(end) as u32).to_le_bytes());
        }
    }
    code
}

/// Register form: prefix bytes, REX (W, R from `reg`, B from `rm`, or `force_rex`), opcode, ModRM.
fn reg_code(
    prefix: &[u8],
    rex_w: bool,
    force_rex: bool,
    opcode: &[u8],
    reg: u8,
    rm: u8,
) -> Vec<u8> {
    let rex =
        if rex_w { 8 } else { 0 } | if reg >= 8 { 4 } else { 0 } | if rm >= 8 { 1 } else { 0 };
    let mut code = prefix.to_vec();
    if rex != 0 || force_rex {
        code.push(0x40 | rex);
    }
    code.extend_from_slice(opcode);
    code.push(0xc0 | (reg & 7) << 3 | (rm & 7));
    code
}

/// A distinct, non-trivial value for the memory operand of form number `seed`.
fn memory_value(seed: u64) -> u64 {
    Rng(seed.wrapping_mul(0x1234_5678_9abc_def1) ^ 0xa5a5).next()
}

fn put_memory(ram: &mut Ram, address: u64, value: u64) {
    let start = address as usize;
    ram.bytes[start..start + 8].copy_from_slice(&value.to_le_bytes());
}

fn mask(bytes: usize) -> u64 {
    if bytes == 8 {
        u64::MAX
    } else {
        (1u64 << (8 * bytes)) - 1
    }
}

// =====================================================================================================
// PCMPGTQ
// =====================================================================================================

const PCMPGTQ: [u8; 3] = [0x0f, 0x38, 0x37];

fn pcmpgtq_reg(dest: u8, src: u8) -> Vec<u8> {
    reg_code(&[0x66], false, false, &PCMPGTQ, dest, src)
}

fn qwords(values: [u64; 2]) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[..8].copy_from_slice(&values[0].to_le_bytes());
    out[8..].copy_from_slice(&values[1].to_le_bytes());
    out
}

/// The SDM operation: per quadword, all ones when the first operand is greater as a signed integer.
fn pcmpgtq_reference(dest: [u8; 16], src: [u8; 16]) -> [u8; 16] {
    let lane = |bytes: &[u8; 16], index: usize| {
        i64::from_le_bytes(bytes[8 * index..8 * index + 8].try_into().unwrap())
    };
    qwords(core::array::from_fn(|index| {
        if lane(&dest, index) > lane(&src, index) {
            u64::MAX
        } else {
            0
        }
    }))
}

/// PCMPGTQ xmm1, xmm2 on the given register values; returns the new xmm1 and the whole state.
fn run_pcmpgtq(dest: [u8; 16], src: [u8; 16]) -> ([u8; 16], LongModeState, LongModeState) {
    let code = pcmpgtq_reg(1, 2);
    let mut start = cpu();
    start.sse.as_mut().unwrap().xmm[1] = dest;
    start.sse.as_mut().unwrap().xmm[2] = src;
    let mut ram = Ram::new(&code);
    let (outcome, end) = exec(start.clone(), &mut ram);
    assert_eq!(outcome, Ok(StepOutcome::Advanced), "{dest:02x?} {src:02x?}");
    (end.sse.as_ref().unwrap().xmm[1], start, end)
}

#[test]
fn pcmpgtq_compares_signed_quadwords_at_the_range_ends() {
    const MIN: u64 = i64::MIN as u64;
    const MAX: u64 = i64::MAX as u64;
    // (destination lane, source lane, expected lane): signed order, so 0x8000.. is the smallest and
    // 0x7fff.. the largest value, -1 (all ones) is below 0 and above the minimum.
    let lanes: [(u64, u64, u64); 20] = [
        (MAX, MIN, u64::MAX),
        (MIN, MAX, 0),
        (MAX, MAX, 0),
        (MIN, MIN, 0),
        (0, u64::MAX, u64::MAX),
        (u64::MAX, 0, 0),
        (u64::MAX, MIN, u64::MAX),
        (MIN, u64::MAX, 0),
        (MIN + 1, MIN, u64::MAX),
        (MIN, MIN + 1, 0),
        (MAX, MAX - 1, u64::MAX),
        (MAX - 1, MAX, 0),
        (1, 0, u64::MAX),
        (0, 1, 0),
        (0, 0, 0),
        (u64::MAX, u64::MAX, 0),
        // the value that is large as an unsigned integer but negative as a signed one
        (0x8000_0000_0000_0001, 1, 0),
        (1, 0x8000_0000_0000_0001, u64::MAX),
        // only the low dword differs
        (0x0000_0001_0000_0000, 0x0000_0000_ffff_ffff, u64::MAX),
        (0x0000_0000_ffff_ffff, 0x0000_0001_0000_0000, 0),
    ];
    for (dest, src, expected) in lanes {
        let (low, _, _) = run_pcmpgtq(qwords([dest, 0]), qwords([src, 0]));
        assert_eq!(
            &low[..8],
            &expected.to_le_bytes(),
            "lane 0: {dest:#x} > {src:#x}"
        );
        assert_eq!(&low[8..], &[0u8; 8], "lane 1 equal operands");
        let (high, _, _) = run_pcmpgtq(qwords([0, dest]), qwords([0, src]));
        assert_eq!(
            &high[8..],
            &expected.to_le_bytes(),
            "lane 1: {dest:#x} > {src:#x}"
        );
        assert_eq!(&high[..8], &[0u8; 8], "lane 0 equal operands");
    }
}

#[test]
fn pcmpgtq_matches_the_reference_for_boundary_and_generated_data_and_changes_only_the_destination()
{
    let mut rng = Rng(0x5600_0001);
    let mut cases = 0;
    let mut check = |dest: [u64; 2], src: [u64; 2]| {
        let (dest, src) = (qwords(dest), qwords(src));
        let (result, start, end) = run_pcmpgtq(dest, src);
        assert_eq!(
            result,
            pcmpgtq_reference(dest, src),
            "{dest:02x?} {src:02x?}"
        );
        let mut expected = start.clone();
        expected.sse.as_mut().unwrap().xmm[1] = result;
        expected.rip = CODE + 5;
        assert_eq!(
            end, expected,
            "only xmm1, RIP change; flags and MXCSR untouched"
        );
        cases += 1;
    };
    for a in BOUNDARY {
        for b in BOUNDARY {
            check([a, b], [b, a]);
            check([a, a], [b, b]);
        }
    }
    for _ in 0..2000 {
        // random lanes, with the high halves forced equal often so that the low halves decide
        let (a, b) = (rng.next(), rng.next());
        let (c, d) = if rng.next() & 1 == 0 {
            (a, b)
        } else {
            (rng.next(), rng.next())
        };
        let tweak = rng.next() & 3;
        check([a, b], [c ^ (tweak << 62), d ^ (tweak & 1)]);
    }
    assert!(cases > 2500, "{cases}");
}

#[test]
fn pcmpgtq_register_pairs_including_the_same_register_use_the_right_registers() {
    for dest in 0..16u8 {
        for src in 0..16u8 {
            let code = pcmpgtq_reg(dest, src);
            let mut start = cpu();
            let (a, b) = (qwords([1 << 63, 7]), qwords([u64::MAX, 3]));
            start.sse.as_mut().unwrap().xmm[usize::from(dest)] = a;
            if src != dest {
                start.sse.as_mut().unwrap().xmm[usize::from(src)] = b;
            }
            let mut ram = Ram::new(&code);
            let (outcome, end) = exec(start.clone(), &mut ram);
            assert_eq!(outcome, Ok(StepOutcome::Advanced), "xmm{dest}, xmm{src}");
            let source = start.sse.as_ref().unwrap().xmm[usize::from(src)];
            let mut expected = start.clone();
            expected.sse.as_mut().unwrap().xmm[usize::from(dest)] = pcmpgtq_reference(a, source);
            expected.rip = CODE + code.len() as u64;
            assert_eq!(end, expected, "xmm{dest}, xmm{src}");
            if src == dest {
                assert_eq!(end.sse.as_ref().unwrap().xmm[usize::from(dest)], [0; 16]);
            }
            assert!(ram.reads.is_empty() && ram.writes.is_empty());
        }
    }
}

#[test]
fn pcmpgtq_every_addressing_form_reads_sixteen_aligned_bytes_once() {
    for (index, form) in FORMS.iter().enumerate() {
        for xmm in [0u8, 7, 12] {
            let code = mem_code(&[0x66], false, &PCMPGTQ, xmm, form);
            let mut start = setup(form);
            let mut ram = Ram::new(&code);
            let address = form.address as usize;
            let source = qwords([memory_value(index as u64), 0x8000_0000_0000_0000]);
            ram.bytes[address..address + 16].copy_from_slice(&source);
            let dest = qwords([u64::MAX, 0x7fff_ffff_ffff_ffff]);
            start.sse.as_mut().unwrap().xmm[usize::from(xmm)] = dest;
            let mut expected = start.clone();
            expected.sse.as_mut().unwrap().xmm[usize::from(xmm)] = pcmpgtq_reference(dest, source);
            expected.rip = CODE + code.len() as u64;
            let ram_before = ram.bytes.clone();
            let decoded =
                long_decode::decode(&mut Ram::new(&code), CODE, AccessContext::default()).unwrap();
            assert_eq!(decoded.length as usize, code.len(), "{}", form.name);
            assert!(
                matches!(
                    decoded.operation,
                    Operation::Sse41 {
                        kind: Sse41Kind::Pcmpgtq,
                        operand: Sse41Operand::Memory(_),
                        ..
                    }
                ),
                "{}: {:?}",
                form.name,
                decoded.operation
            );
            let (outcome, end) = exec(start, &mut ram);
            assert_eq!(outcome, Ok(StepOutcome::Advanced), "{}", form.name);
            assert_eq!(end, expected, "{}", form.name);
            assert_eq!(ram.bytes, ram_before, "{}", form.name);
            assert_eq!(ram.reads, [(form.address, 16)], "{}", form.name);
            assert!(ram.writes.is_empty(), "{}", form.name);
        }
    }
}

#[test]
fn pcmpgtq_type_4_faults_are_raised_before_any_state_change() {
    let code = mem_code(&[0x66], false, &PCMPGTQ, 1, &FORMS[0]);
    // A 128-bit operand that is not 16-byte aligned is #GP(0) (Table 2-21 note 1 exempts only LDDQU, MOVUPD,
    // MOVUPS and the PCMP*STR* forms).
    for address in [0x8001u64, 0x8002, 0x8004, 0x8008, 0x800f, 0x8018] {
        let mut start = cpu();
        start.gpr[Register::Rax as usize] = address;
        let mut ram = Ram::new(&code);
        let ram_before = ram.bytes.clone();
        let (outcome, end) = exec(start.clone(), &mut ram);
        assert_eq!(outcome, Ok(gp0()), "{address:#x}");
        assert_eq!(end, start);
        assert_eq!(ram.bytes, ram_before);
        assert!(ram.reads.is_empty() && ram.writes.is_empty());
    }
    // Non-canonical addresses: #GP(0), or #SS(0) when the base is RSP or RBP; before alignment and memory.
    for (base, tail, expected) in [
        (Register::Rax, vec![0x08u8], gp0()),
        (Register::Rbp, vec![0x4d, 0x00], ss0()),
        (Register::Rsp, vec![0x0c, 0x24], ss0()),
    ] {
        for address in [
            0x0000_8000_0000_0000u64,
            0x0000_8000_0000_0001,
            0xffff_7fff_ffff_ffff,
            0x7fff_ffff_ffff_ffff,
        ] {
            let mut code = vec![0x66, 0x0f, 0x38, 0x37];
            code.extend_from_slice(&tail);
            let mut start = cpu();
            start.gpr[base as usize] = address;
            let mut ram = Ram::new(&code);
            let (outcome, end) = exec(start.clone(), &mut ram);
            assert_eq!(outcome, Ok(expected.clone()), "{base:?} {address:#x}");
            assert_eq!(end, start);
            assert!(ram.reads.is_empty() && ram.writes.is_empty());
        }
    }
    // A fault of the memory backend (for example #PF) is its own result and leaves the state untouched.
    let mut start = cpu();
    start.gpr[Register::Rax as usize] = 0x9000;
    let mut ram = Ram::new(&code);
    ram.fault_page = Some(0x9000);
    let (outcome, end) = exec(start.clone(), &mut ram);
    assert_eq!(
        outcome,
        Ok(StepOutcome::Fault(ArchitecturalFault {
            vector: 14,
            error_code: Some(0),
            linear_address: Some(0x9000)
        }))
    );
    assert_eq!(end, start);
}

#[test]
fn pcmpgtq_with_lock_is_invalid_opcode_in_every_prefix_order() {
    for code in [
        vec![0xf0, 0x66, 0x0f, 0x38, 0x37, 0x00],
        vec![0x66, 0xf0, 0x0f, 0x38, 0x37, 0x00],
        vec![0xf0, 0x66, 0x48, 0x0f, 0x38, 0x37, 0x10],
        vec![0xf0, 0x66, 0x0f, 0x38, 0x37, 0xc1],
        vec![0x66, 0xf0, 0x0f, 0x38, 0x37, 0xc1],
        vec![0x66, 0x44, 0xf0, 0x0f, 0x38, 0x37, 0xc1],
    ] {
        let start = cpu();
        let mut ram = Ram::new(&code);
        let (outcome, end) = exec(start.clone(), &mut ram);
        assert_eq!(outcome, Ok(ud()), "{code:02x?}");
        assert_eq!(end, start);
        assert!(ram.reads.is_empty() && ram.writes.is_empty());
    }
}

#[test]
fn pcmpgtq_follows_the_cr0_cr4_gate_of_the_sse_forms_under_the_fp_init_profile() {
    const CR0_BASE: u64 = 0x8001_0033; // PG | WP | NE | ET | MP | PE: EM = 0, TS = 0
    const CR4_PAE: u64 = 0x20;
    const CR4_OSFXSR: u64 = 0x200;
    for (cr0, cr4, vector) in [
        (CR0_BASE, CR4_PAE | CR4_OSFXSR, None),
        (CR0_BASE | 4, CR4_PAE | CR4_OSFXSR, Some(6)),
        (CR0_BASE | 8, CR4_PAE | CR4_OSFXSR, Some(7)),
        (CR0_BASE | 4 | 8, CR4_PAE | CR4_OSFXSR, Some(6)),
        (CR0_BASE, CR4_PAE, Some(6)),
        (CR0_BASE | 8, CR4_PAE, Some(6)),
    ] {
        for code in [
            pcmpgtq_reg(1, 2),
            mem_code(&[0x66], false, &PCMPGTQ, 1, &FORMS[0]),
        ] {
            let mut start = cpu();
            start.activity = Some(CpuActivity::Running);
            start.sti_shadow = Some(StiShadow::Clear);
            start.cr0 = Some(cr0);
            start.cr4 = Some(cr4);
            start.gpr[Register::Rax as usize] = 0x8000;
            let mut ram = Ram::new(&code);
            let mut after = start.clone();
            let result = execute::run_with_cr4_profile(
                &mut after,
                &mut ram,
                RunConfig {
                    instruction_budget: 1,
                    stop_rip: None,
                    context: ctx(PrivilegeLevel::Ring0),
                },
                Cr4Profile::FpInitLong64V1,
                || false,
            );
            match vector {
                Some(vector) => {
                    assert_eq!(
                        result.exit,
                        RunExit::Fault(ArchitecturalFault {
                            vector,
                            error_code: None,
                            linear_address: None
                        }),
                        "{cr0:#x} {cr4:#x}"
                    );
                    assert_eq!(after, start);
                }
                None => assert_eq!(
                    result.exit,
                    RunExit::InstructionBudgetExhausted,
                    "{cr0:#x} {cr4:#x}"
                ),
            }
        }
    }
}

#[test]
fn pcmpgtq_without_sse_state_stops_as_unsupported_and_needs_no_x87_state() {
    let code = pcmpgtq_reg(1, 2);
    let mut start = cpu();
    start.sse = None;
    let mut ram = Ram::new(&code);
    let (outcome, end) = exec(start.clone(), &mut ram);
    assert_eq!(outcome, Ok(StepOutcome::Unsupported { opcode: 0x37 }));
    assert_eq!(end, start);
    let mut start = cpu();
    start.x87 = None;
    let mut ram = Ram::new(&code);
    assert_eq!(exec(start, &mut ram).0, Ok(StepOutcome::Advanced));
}

#[test]
fn pcmpgtq_neighbouring_encodings_stay_unsupported_without_change() {
    for (code, what) in [
        (vec![0x0f, 0x38, 0x37, 0x00], "no 66, memory"),
        (vec![0x0f, 0x38, 0x37, 0xc1], "no 66, register"),
        (vec![0xf2, 0x66, 0x0f, 0x38, 0x37, 0xc1], "F2 66"),
        (vec![0x66, 0xf3, 0x0f, 0x38, 0x37, 0xc1], "66 F3"),
        (vec![0xf2, 0xf3, 0x66, 0x0f, 0x38, 0x37, 0xc1], "F2 F3 66"),
        (vec![0x66, 0x66, 0x0f, 0x38, 0x37, 0xc1], "duplicate 66"),
        (vec![0x64, 0x66, 0x0f, 0x38, 0x37, 0x00], "FS"),
        (vec![0x66, 0x65, 0x0f, 0x38, 0x37, 0x00], "GS"),
        (
            vec![0x66, 0x40, 0x40, 0x0f, 0x38, 0x37, 0xc1],
            "duplicate REX",
        ),
    ] {
        let start = cpu();
        let mut ram = Ram::new(&code);
        let (outcome, end) = exec(start.clone(), &mut ram);
        assert!(
            matches!(outcome, Ok(StepOutcome::Unsupported { .. })),
            "{what}: {outcome:?}"
        );
        assert_eq!(end, start, "{what}");
        assert!(ram.reads.is_empty() && ram.writes.is_empty(), "{what}");
    }
    // REX.W and the null segment overrides are ignored by the register form.
    for code in [
        vec![0x66, 0x48, 0x0f, 0x38, 0x37, 0xca],
        vec![0x2e, 0x66, 0x0f, 0x38, 0x37, 0xca],
        vec![0x67, 0x66, 0x0f, 0x38, 0x37, 0xca],
    ] {
        let start = cpu();
        let mut ram = Ram::new(&code);
        assert_eq!(
            exec(start, &mut ram).0,
            Ok(StepOutcome::Advanced),
            "{code:02x?}"
        );
    }
}

// =====================================================================================================
// POPCNT
// =====================================================================================================

/// `F3 [REX.W] 0F B8 /r` with the register forms; `bytes` is the operand width.
fn popcnt_prefix(bytes: usize) -> (&'static [u8], bool) {
    match bytes {
        2 => (&[0x66, 0xf3], false),
        4 => (&[0xf3], false),
        _ => (&[0xf3], true),
    }
}

fn popcnt_reg(bytes: usize, dest: u8, src: u8) -> Vec<u8> {
    let (prefix, wide) = popcnt_prefix(bytes);
    reg_code(prefix, wide, false, &[0x0f, 0xb8], dest, src)
}

/// What POPCNT does to the destination register: 16 bits keep bits 63:16, 32 bits zero-extend, 64 bits replace.
fn popcnt_destination(old: u64, bytes: usize, count: u64) -> u64 {
    match bytes {
        2 => (old & !0xffff) | count,
        _ => count,
    }
}

#[test]
fn popcnt_counts_the_set_bits_of_the_operand_width_and_sets_zf_only_for_a_zero_source() {
    // (operand bytes, full source register value, count)
    let vectors: [(usize, u64, u64); 24] = [
        (2, 0, 0),
        (2, 0xffff_ffff_ffff_0000, 0),
        (2, 0x0001, 1),
        (2, 0x8000, 1),
        (2, 0xffff, 16),
        (2, 0xaaaa, 8),
        (2, 0xffff_ffff_ffff_ffff, 16),
        (2, 0x1234_5678_9abc_00ff, 8),
        (4, 0, 0),
        (4, 0xffff_ffff_0000_0000, 0),
        (4, 0x8000_0000, 1),
        (4, 0xffff_ffff, 32),
        (4, 0x5555_5555, 16),
        (4, 0xffff_ffff_ffff_ffff, 32),
        (4, 0xdead_beef_0000_0001, 1),
        (8, 0, 0),
        (8, 1, 1),
        (8, 1 << 63, 1),
        (8, u64::MAX, 64),
        (8, 0x8000_0000_0000_0001, 2),
        (8, 0x0123_4567_89ab_cdef, 32),
        (8, 0xffff_ffff_0000_0000, 32),
        (8, 0x0000_0001_0000_0000, 1),
        (8, 0xaaaa_aaaa_aaaa_aaaa, 32),
    ];
    for (bytes, source, count) in vectors {
        for flags in [0x2, FLAGS, 0x2 | STATUS, 0x2 | DF | IF, 0x2 | CF | OF] {
            let code = popcnt_reg(bytes, 1, 2);
            let mut start = cpu();
            start.rflags = flags;
            start.gpr[Register::Rcx as usize] = 0xdead_beef_cafe_f00d;
            start.gpr[Register::Rdx as usize] = source;
            let mut ram = Ram::new(&code);
            let (outcome, end) = exec(start.clone(), &mut ram);
            assert_eq!(outcome, Ok(StepOutcome::Advanced), "{bytes} {source:#x}");
            let mut expected = start.clone();
            expected.gpr[Register::Rcx as usize] =
                popcnt_destination(0xdead_beef_cafe_f00d, bytes, count);
            // OF, SF, ZF, AF, CF, PF are all cleared; ZF is set if SRC = 0. DF, IF and the reserved bit stay.
            expected.rflags = (flags & !STATUS) | if count == 0 { ZF } else { 0 };
            expected.rip = CODE + code.len() as u64;
            assert_eq!(
                end, expected,
                "{bytes} bytes, source {source:#x}, flags {flags:#x}"
            );
            assert!(ram.reads.is_empty() && ram.writes.is_empty());
        }
    }
}

#[test]
fn popcnt_matches_count_ones_for_every_register_pair_and_size_and_boundary_data() {
    let mut rng = Rng(0x5600_0002);
    let mut cases = 0;
    for bytes in [2usize, 4, 8] {
        for dest in 0..16u8 {
            for src in 0..16u8 {
                let code = popcnt_reg(bytes, dest, src);
                let mut start = cpu();
                // a different source value per pair, with the boundary values rotating through
                let value = if (usize::from(dest) + usize::from(src)) % 3 == 0 {
                    BOUNDARY[(usize::from(dest) * 16 + usize::from(src)) % 16]
                } else {
                    rng.next()
                };
                start.gpr[usize::from(src)] = value;
                let mut ram = Ram::new(&code);
                let (outcome, end) = exec(start.clone(), &mut ram);
                assert_eq!(outcome, Ok(StepOutcome::Advanced));
                let count = u64::from((value & mask(bytes)).count_ones());
                // when the destination is also the source, the old value was counted and is then replaced
                let mut expected = start.clone();
                expected.gpr[usize::from(dest)] =
                    popcnt_destination(start.gpr[usize::from(dest)], bytes, count);
                expected.rflags = (start.rflags & !STATUS) | if count == 0 { ZF } else { 0 };
                expected.rip = CODE + code.len() as u64;
                assert_eq!(end, expected, "{bytes} bytes r{dest}, r{src}, {value:#x}");
                cases += 1;
            }
        }
    }
    assert_eq!(cases, 3 * 256);
}

#[test]
fn popcnt_every_addressing_form_reads_exactly_the_operand_width_once() {
    for bytes in [2usize, 4, 8] {
        let (prefix, wide) = popcnt_prefix(bytes);
        for (index, form) in FORMS.iter().enumerate() {
            for dest in [0u8, 5, 9, 15] {
                let code = mem_code(prefix, wide, &[0x0f, 0xb8], dest, form);
                let start = setup(form);
                let value = memory_value(index as u64 * 7 + u64::from(dest));
                let mut ram = Ram::new(&code);
                put_memory(&mut ram, form.address, value);
                let ram_before = ram.bytes.clone();
                let old = start.gpr[usize::from(dest)];
                // the destination can also be an address register of the form: the address is made first
                let count = u64::from((value & mask(bytes)).count_ones());
                let mut expected = start.clone();
                expected.gpr[usize::from(dest)] = popcnt_destination(old, bytes, count);
                expected.rflags = (start.rflags & !STATUS) | if count == 0 { ZF } else { 0 };
                expected.rip = CODE + code.len() as u64;
                let decoded =
                    long_decode::decode(&mut Ram::new(&code), CODE, AccessContext::default())
                        .unwrap();
                assert_eq!(decoded.length as usize, code.len(), "{}", form.name);
                let Operation::Popcnt {
                    dst,
                    src: Operand::Memory(memory),
                } = decoded.operation
                else {
                    panic!("{}: {:?}", form.name, decoded.operation);
                };
                assert_eq!(
                    (dst.register() as u8, dst.size().bytes()),
                    (dest, bytes),
                    "{}",
                    form.name
                );
                assert_eq!(memory.size.bytes(), bytes, "{}", form.name);
                let (outcome, end) = exec(start, &mut ram);
                assert_eq!(outcome, Ok(StepOutcome::Advanced), "{} {bytes}", form.name);
                assert_eq!(end, expected, "{} {bytes} r{dest}", form.name);
                assert_eq!(ram.bytes, ram_before, "{}", form.name);
                assert_eq!(ram.reads, [(form.address, bytes)], "{}", form.name);
                assert!(ram.writes.is_empty(), "{}", form.name);
            }
        }
    }
}

#[test]
fn popcnt_and_crc32_memory_operands_have_no_alignment_requirement_at_cpl0() {
    for (code_prefix, opcode, wide, bytes, name) in [
        (
            &[0x66u8, 0xf3][..],
            [0x0fu8, 0xb8],
            false,
            2usize,
            "popcnt16",
        ),
        (&[0xf3], [0x0f, 0xb8], false, 4, "popcnt32"),
        (&[0xf3], [0x0f, 0xb8], true, 8, "popcnt64"),
        (&[0xf2], [0x0f, 0x38], false, 1, "crc32 m8"),
    ] {
        let mut code = code_prefix.to_vec();
        if wide {
            code.push(0x48);
        }
        code.extend_from_slice(&opcode);
        if opcode[1] == 0x38 {
            code.push(0xf0);
        }
        code.extend_from_slice(&[0x08]); // reg = ecx, [rax]
        for address in [0x8001u64, 0x8002, 0x8003, 0x8005, 0x800f, 0x8ffd] {
            let mut start = cpu();
            start.gpr[Register::Rax as usize] = address;
            let mut ram = Ram::new(&code);
            let (outcome, _) = exec(start, &mut ram);
            assert_eq!(outcome, Ok(StepOutcome::Advanced), "{name} {address:#x}");
            assert_eq!(ram.reads, [(address, bytes)], "{name}");
        }
    }
}

#[test]
fn popcnt_and_crc32_fault_before_any_state_change() {
    let popcnt_code = [0xf3u8, 0x48, 0x0f, 0xb8, 0x08]; // popcnt rcx, [rax]
    let crc_code = [0xf2u8, 0x48, 0x0f, 0x38, 0xf1, 0x08]; // crc32 rcx, qword [rax]
    for code in [&popcnt_code[..], &crc_code[..]] {
        // Non-canonical start: #GP(0); the last byte of an 8-byte operand non-canonical: #GP(0).
        for address in [
            0x0000_8000_0000_0000u64,
            0xffff_7fff_ffff_ffff,
            0x7fff_ffff_ffff_ffff,
            0x0000_7fff_ffff_fffc,
        ] {
            let mut start = cpu();
            start.gpr[Register::Rax as usize] = address;
            let mut ram = Ram::new(code);
            let (outcome, end) = exec(start.clone(), &mut ram);
            assert_eq!(outcome, Ok(gp0()), "{code:02x?} {address:#x}");
            assert_eq!(end, start);
            assert!(ram.reads.is_empty() && ram.writes.is_empty());
        }
        // RSP / RBP based: #SS(0).
        for (base, tail) in [(Register::Rbp, [0x4d, 0x00]), (Register::Rsp, [0x0c, 0x24])] {
            let mut based = code[..code.len() - 1].to_vec();
            based.extend_from_slice(&tail[..]);
            let mut start = cpu();
            start.gpr[base as usize] = 0x0000_8000_0000_0000;
            let mut ram = Ram::new(&based);
            let (outcome, end) = exec(start.clone(), &mut ram);
            assert_eq!(outcome, Ok(ss0()), "{based:02x?} {base:?}");
            assert_eq!(end, start);
        }
        // A fault of the memory backend leaves registers and flags untouched.
        let mut start = cpu();
        start.gpr[Register::Rax as usize] = 0x9000;
        let mut ram = Ram::new(code);
        ram.fault_page = Some(0x9000);
        let (outcome, end) = exec(start.clone(), &mut ram);
        assert!(
            matches!(
                outcome,
                Ok(StepOutcome::Fault(ArchitecturalFault { vector: 14, .. }))
            ),
            "{outcome:?}"
        );
        assert_eq!(end, start);
        // The page boundary: an 8-byte read that straddles the faulting page faults as a whole.
        let mut start = cpu();
        start.gpr[Register::Rax as usize] = 0x8ffc;
        let mut ram = Ram::new(code);
        ram.fault_page = Some(0x9000);
        let (outcome, end) = exec(start.clone(), &mut ram);
        assert!(matches!(
            outcome,
            Ok(StepOutcome::Fault(ArchitecturalFault { vector: 14, .. }))
        ));
        assert_eq!(end, start);
    }
}

#[test]
fn popcnt_and_crc32_alignment_check_at_cpl3_is_refused_not_guessed() {
    for (code, bytes, opcode) in [
        (vec![0xf3u8, 0x0f, 0xb8, 0x08], 4usize, 0xb8u8),
        (vec![0xf3, 0x48, 0x0f, 0xb8, 0x08], 8, 0xb8),
        (vec![0x66, 0xf3, 0x0f, 0xb8, 0x08], 2, 0xb8),
        (vec![0xf2, 0x0f, 0x38, 0xf1, 0x08], 4, 0xf1),
        (vec![0xf2, 0x48, 0x0f, 0x38, 0xf1, 0x08], 8, 0xf1),
        (vec![0x66, 0xf2, 0x0f, 0x38, 0xf1, 0x08], 2, 0xf1),
    ] {
        for (privilege, ac, aligned_only) in [
            (PrivilegeLevel::Ring3, true, true),
            (PrivilegeLevel::Ring3, false, false),
            (PrivilegeLevel::Ring0, true, false), // AC is a CPL3-only check
        ] {
            for address in [0x8000u64, 0x8001, 0x8002, 0x8004, 0x8008, 0x800c] {
                let mut start = cpu();
                start.gpr[Register::Rax as usize] = address;
                start.rflags |= if ac { AC } else { 0 };
                let mut ram = Ram::new(&code);
                let mut after = start.clone();
                let outcome = step(&mut after, &mut ram, privilege);
                let misaligned = address % bytes as u64 != 0;
                if aligned_only && misaligned {
                    assert_eq!(
                        outcome,
                        Ok(StepOutcome::Unsupported { opcode }),
                        "{code:02x?} {address:#x}"
                    );
                    assert_eq!(after, start);
                    assert!(ram.reads.is_empty() && ram.writes.is_empty());
                } else {
                    assert_eq!(
                        outcome,
                        Ok(StepOutcome::Advanced),
                        "{code:02x?} {address:#x} {privilege:?} ac={ac}"
                    );
                }
            }
        }
    }
}

#[test]
fn popcnt_with_66_selects_16_bits_and_rex_w_wins_over_66_and_other_prefixes_are_ignored() {
    // (code, operand bytes)
    for (code, bytes) in [
        (vec![0xf3u8, 0x0f, 0xb8, 0xca], 4usize),
        (vec![0x66, 0xf3, 0x0f, 0xb8, 0xca], 2),
        (vec![0xf3, 0x66, 0x0f, 0xb8, 0xca], 2),
        (vec![0xf3, 0x48, 0x0f, 0xb8, 0xca], 8),
        (vec![0x66, 0xf3, 0x48, 0x0f, 0xb8, 0xca], 8),
        (vec![0xf3, 0x66, 0x48, 0x0f, 0xb8, 0xca], 8),
        (vec![0xf3, 0x40, 0x0f, 0xb8, 0xca], 4),
        (vec![0x2e, 0xf3, 0x0f, 0xb8, 0xca], 4),
        (vec![0x67, 0xf3, 0x0f, 0xb8, 0xca], 4),
        (vec![0x3e, 0x66, 0xf3, 0x0f, 0xb8, 0xca], 2),
    ] {
        let mut start = cpu();
        start.gpr[Register::Rcx as usize] = 0xdead_beef_cafe_f00d;
        start.gpr[Register::Rdx as usize] = 0xf0f0_f0f0_0f0f_0f0f;
        let mut ram = Ram::new(&code);
        let (outcome, end) = exec(start, &mut ram);
        assert_eq!(outcome, Ok(StepOutcome::Advanced), "{code:02x?}");
        let count = u64::from((0xf0f0_f0f0_0f0f_0f0fu64 & mask(bytes)).count_ones());
        assert_eq!(
            end.gpr[Register::Rcx as usize],
            popcnt_destination(0xdead_beef_cafe_f00d, bytes, count),
            "{code:02x?}"
        );
    }
    // REX.R and REX.B reach r8..r15.
    let code = [0xf3u8, 0x4d, 0x0f, 0xb8, 0xf9]; // popcnt r15, r9
    let mut start = cpu();
    start.gpr[Register::R9 as usize] = 0x0f0f;
    let mut ram = Ram::new(&code);
    let (outcome, end) = exec(start.clone(), &mut ram);
    assert_eq!(outcome, Ok(StepOutcome::Advanced));
    assert_eq!(end.gpr[Register::R15 as usize], 8);
    assert_eq!(end.gpr[Register::R9 as usize], 0x0f0f);
}

#[test]
fn popcnt_and_crc32_with_lock_are_invalid_opcode() {
    for code in [
        vec![0xf0, 0xf3, 0x0f, 0xb8, 0x00],
        vec![0xf3, 0xf0, 0x0f, 0xb8, 0x00],
        vec![0xf0, 0x66, 0xf3, 0x0f, 0xb8, 0x00],
        vec![0xf0, 0xf3, 0x48, 0x0f, 0xb8, 0x10],
        vec![0xf0, 0xf3, 0x0f, 0xb8, 0xc1],
        vec![0xf0, 0xf2, 0x0f, 0x38, 0xf0, 0x00],
        vec![0xf2, 0xf0, 0x0f, 0x38, 0xf1, 0x00],
        vec![0xf0, 0xf2, 0x48, 0x0f, 0x38, 0xf1, 0x10],
        vec![0xf0, 0x66, 0xf2, 0x0f, 0x38, 0xf1, 0xc1],
        vec![0xf0, 0xf2, 0x0f, 0x38, 0xf0, 0xc1],
    ] {
        let start = cpu();
        let mut ram = Ram::new(&code);
        let (outcome, end) = exec(start.clone(), &mut ram);
        assert_eq!(outcome, Ok(ud()), "{code:02x?}");
        assert_eq!(end, start);
        assert!(ram.reads.is_empty() && ram.writes.is_empty());
        assert!(matches!(
            long_decode::decode(&mut Ram::new(&code), CODE, AccessContext::default()),
            Err(long_decode::DecodeError::Invalid { .. })
        ));
    }
}

// =====================================================================================================
// CRC32
// =====================================================================================================

/// The SDM "Operation" of CRC32, transcribed literally: BIT_REFLECT of source and destination, the shifts
/// by 32 bits and by the source width, the XOR and the MOD2 division by 11EDC6F41H, then BIT_REFLECT of the
/// 32-bit remainder. `bits` is 8, 16, 32 or 64.
fn sdm_crc32(dest: u32, src: u64, bits: u32) -> u32 {
    fn reflect(value: u128, bits: u32) -> u128 {
        (0..bits).fold(0, |acc, index| {
            acc | (((value >> index) & 1) << (bits - 1 - index))
        })
    }
    fn mod2(mut value: u128) -> u32 {
        const POLYNOMIAL: u128 = 0x1_1EDC_6F41;
        for bit in (32..128).rev() {
            if value >> bit & 1 == 1 {
                value ^= POLYNOMIAL << (bit - 32);
            }
        }
        value as u32
    }
    let source = u128::from(src) & ((1u128 << bits) - 1);
    let temp1 = reflect(source, bits);
    let temp2 = reflect(u128::from(dest), 32);
    let temp3 = temp1 << 32;
    let temp4 = temp2 << bits;
    reflect(u128::from(mod2(temp3 ^ temp4)), 32) as u32
}

/// (name, legacy prefixes, REX.W, opcode, source bytes)
const CRC_FORMS: [(&str, &[u8], bool, u8, usize); 5] = [
    ("crc32 r32, r/m8", &[0xf2], false, 0xf0, 1),
    ("crc32 r64, r/m8", &[0xf2], true, 0xf0, 1),
    ("crc32 r32, r/m16", &[0x66, 0xf2], false, 0xf1, 2),
    ("crc32 r32, r/m32", &[0xf2], false, 0xf1, 4),
    ("crc32 r64, r/m64", &[0xf2], true, 0xf1, 8),
];

fn crc_reg(form: usize, dest: u8, src: u8, force_rex: bool) -> Vec<u8> {
    let (_, prefix, wide, opcode, _) = CRC_FORMS[form];
    reg_code(prefix, wide, force_rex, &[0x0f, 0x38, opcode], dest, src)
}

/// The low byte (or word, dword, qword) of general register `encoding` as the source operand of a CRC32
/// register form: without a REX prefix the byte encodings 4..7 are AH, CH, DH, BH.
fn crc_source(cpu: &LongModeState, encoding: u8, bytes: usize, rex: bool) -> u64 {
    if bytes == 1 && !rex && (4..8).contains(&encoding) {
        (cpu.gpr[usize::from(encoding - 4)] >> 8) & 0xff
    } else {
        cpu.gpr[usize::from(encoding)] & mask(bytes)
    }
}

/// Runs one CRC32 register form and returns the new destination register.
fn run_crc(form: usize, crc: u64, data: u64) -> u64 {
    let code = crc_reg(form, 1, 2, false); // crc32 {e,r}cx, {dl,dx,edx,rdx}
    let mut start = cpu();
    start.gpr[Register::Rcx as usize] = crc;
    start.gpr[Register::Rdx as usize] = data;
    let mut ram = Ram::new(&code);
    let (outcome, end) = exec(start.clone(), &mut ram);
    assert_eq!(
        outcome,
        Ok(StepOutcome::Advanced),
        "{} {crc:#x} {data:#x}",
        CRC_FORMS[form].0
    );
    // No flag changes (Flags Affected: None), only the destination register and RIP.
    let mut expected = start;
    expected.gpr[Register::Rcx as usize] = end.gpr[Register::Rcx as usize];
    expected.rip = CODE + code.len() as u64;
    assert_eq!(end, expected, "{}", CRC_FORMS[form].0);
    end.gpr[Register::Rcx as usize]
}

/// The CRC-32C of `data` (initial value all ones, final complement) computed with the form number `form`
/// of CRC32 over successive chunks of that form's width.
fn crc32c_of(data: &[u8], form: usize) -> u32 {
    let width = CRC_FORMS[form].4;
    assert_eq!(data.len() % width, 0);
    let mut crc = 0xffff_ffffu64;
    for chunk in data.chunks(width) {
        let mut raw = [0u8; 8];
        raw[..width].copy_from_slice(chunk);
        crc = run_crc(form, crc, u64::from_le_bytes(raw));
    }
    !(crc as u32)
}

#[test]
fn crc32_reproduces_the_rfc_3720_crc32c_examples_with_every_source_width() {
    let incrementing: Vec<u8> = (0..32).collect();
    let decrementing: Vec<u8> = (0..32).rev().collect();
    let read_pdu: [u8; 48] = [
        0x01, 0xc0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x14, 0, 0, 0, 0, 0, 0x04, 0, 0, 0,
        0, 0x14, 0, 0, 0, 0x18, 0x28, 0, 0, 0, 0, 0, 0, 0, 0x02, 0, 0, 0, 0, 0, 0, 0,
    ];
    // RFC 3720 Appendix B.4; the printed CRC bytes in transmission order, as little-endian words.
    let examples: [(&str, Vec<u8>, u32); 5] = [
        (
            "32 bytes of zeroes",
            vec![0; 32],
            u32::from_le_bytes([0xaa, 0x36, 0x91, 0x8a]),
        ),
        (
            "32 bytes of ones",
            vec![0xff; 32],
            u32::from_le_bytes([0x43, 0xab, 0xa8, 0x62]),
        ),
        (
            "incrementing 00..1f",
            incrementing,
            u32::from_le_bytes([0x4e, 0x79, 0xdd, 0x46]),
        ),
        (
            "decrementing 1f..00",
            decrementing,
            u32::from_le_bytes([0x5c, 0xdb, 0x3f, 0x11]),
        ),
        (
            "SCSI Read (10) PDU",
            read_pdu.to_vec(),
            u32::from_le_bytes([0x56, 0x3a, 0x96, 0xd9]),
        ),
    ];
    assert_eq!(examples[0].2, 0x8a91_36aa);
    for (name, data, expected) in &examples {
        for form in 0..CRC_FORMS.len() {
            if data.len() % CRC_FORMS[form].4 != 0 {
                continue;
            }
            assert_eq!(
                crc32c_of(data, form),
                *expected,
                "{name}, {}",
                CRC_FORMS[form].0
            );
        }
    }
    // The CRC catalogue check value of CRC-32/ISCSI (CRC-32C) for "123456789" (nine bytes: byte steps only).
    assert_eq!(crc32c_of(b"123456789", 0), 0xe306_9283);
    assert_eq!(crc32c_of(b"123456789", 1), 0xe306_9283);
}

/// `(initial CRC, data, [result of the byte, word, dword, qword form])` measured with the CRC32 instruction
/// of the physical oracle (Intel Core i5-3210M under Linux x86-64), data truncated to the form's width.
const HARDWARE_STEPS: [(u64, u64, [u64; 4]); 55] = [
    (
        0x00000000,
        0x0000000000000000,
        [0x00000000, 0x00000000, 0x00000000, 0x00000000],
    ),
    (
        0x00000000,
        0x0000000000000001,
        [0xf26b8303, 0x13a29877, 0xdd45aab8, 0x493c7d27],
    ),
    (
        0x00000000,
        0x0000000000000080,
        [0x82f63b78, 0xfbc3faf9, 0x52a0c93f, 0x34019664],
    ),
    (
        0x00000000,
        0x00000000000000ff,
        [0xad7d5351, 0xa3e32483, 0x2803e842, 0x1f1530a5],
    ),
    (
        0x00000000,
        0x0000000000008000,
        [0x00000000, 0x82f63b78, 0x8b277743, 0xf56e0ef4],
    ),
    (
        0x00000000,
        0x000000000000ffff,
        [0xad7d5351, 0x0e9e77d2, 0xb906c3ea, 0x62040dea],
    ),
    (
        0x00000000,
        0x0000000080000000,
        [0x00000000, 0x00000000, 0x82f63b78, 0x6ea2d55c],
    ),
    (
        0x00000000,
        0x00000000ffffffff,
        [0xad7d5351, 0x0e9e77d2, 0xb798b438, 0x73d74d75],
    ),
    (
        0x00000000,
        0x0123456789abcdef,
        [0xbd23943e, 0xe083bf15, 0x5914342a, 0xe9986aa9],
    ),
    (
        0x00000000,
        0x8000000000000000,
        [0x00000000, 0x00000000, 0x00000000, 0x82f63b78],
    ),
    (
        0x00000000,
        0xffffffffffffffff,
        [0xad7d5351, 0x0e9e77d2, 0xb798b438, 0xc44ff94d],
    ),
    (
        0x00000001,
        0x0000000000000000,
        [0xf26b8303, 0x13a29877, 0xdd45aab8, 0x493c7d27],
    ),
    (
        0x00000001,
        0x0000000000000001,
        [0x00000000, 0x00000000, 0x00000000, 0x00000000],
    ),
    (
        0x00000001,
        0x0000000000000080,
        [0x709db87b, 0xe861628e, 0x8fe56387, 0x7d3deb43],
    ),
    (
        0x00000001,
        0x00000000000000ff,
        [0x5f16d052, 0xb041bcf4, 0xf54642fa, 0x56294d82],
    ),
    (
        0x00000001,
        0x0000000000008000,
        [0xf26b8303, 0x9154a30f, 0x5662ddfb, 0xbc5273d3],
    ),
    (
        0x00000001,
        0x000000000000ffff,
        [0x5f16d052, 0x1d3cefa5, 0x64436952, 0x2b3870cd],
    ),
    (
        0x00000001,
        0x0000000080000000,
        [0xf26b8303, 0x13a29877, 0x5fb391c0, 0x279ea87b],
    ),
    (
        0x00000001,
        0x00000000ffffffff,
        [0x5f16d052, 0x1d3cefa5, 0x6add1e80, 0x3aeb3052],
    ),
    (
        0x00000001,
        0x0123456789abcdef,
        [0x4f48173d, 0xf3212762, 0x84519e92, 0xa0a4178e],
    ),
    (
        0x00000001,
        0x8000000000000000,
        [0xf26b8303, 0x13a29877, 0xdd45aab8, 0xcbca465f],
    ),
    (
        0x00000001,
        0xffffffffffffffff,
        [0x5f16d052, 0x1d3cefa5, 0x6add1e80, 0x8d73846a],
    ),
    (
        0xffffffff,
        0x0000000000000000,
        [0xad82acae, 0x0e9e882d, 0xb798b438, 0x73d74d75],
    ),
    (
        0xffffffff,
        0x0000000000000001,
        [0x5fe92fad, 0x1d3c105a, 0x6add1e80, 0x3aeb3052],
    ),
    (
        0xffffffff,
        0x0000000000000080,
        [0x2f7497d6, 0xf55d72d4, 0xe5387d07, 0x47d6db11],
    ),
    (
        0xffffffff,
        0x00000000000000ff,
        [0x00ffffff, 0xad7dacae, 0x9f9b5c7a, 0x6cc27dd0],
    ),
    (
        0xffffffff,
        0x0000000000008000,
        [0xad82acae, 0x8c68b355, 0x3cbfc37b, 0x86b94381],
    ),
    (
        0xffffffff,
        0x000000000000ffff,
        [0x00ffffff, 0x0000ffff, 0x0e9e77d2, 0x11d3409f],
    ),
    (
        0xffffffff,
        0x0000000080000000,
        [0xad82acae, 0x0e9e882d, 0x356e8f40, 0x1d759829],
    ),
    (
        0xffffffff,
        0x00000000ffffffff,
        [0x00ffffff, 0x0000ffff, 0x00000000, 0x00000000],
    ),
    (
        0xffffffff,
        0x0123456789abcdef,
        [0x10a13890, 0xee1d3738, 0xee8c8012, 0x9a4f27dc],
    ),
    (
        0xffffffff,
        0x8000000000000000,
        [0xad82acae, 0x0e9e882d, 0xb798b438, 0xf121760d],
    ),
    (
        0xffffffff,
        0xffffffffffffffff,
        [0x00ffffff, 0x0000ffff, 0x00000000, 0xb798b438],
    ),
    (
        0x12345678,
        0x0000000000000000,
        [0xfb533894, 0x55c93830, 0xfa745634, 0x4a4a6d17],
    ),
    (
        0x12345678,
        0x0000000000000001,
        [0x0938bb97, 0x466ba047, 0x2731fc8c, 0x03761030],
    ),
    (
        0x12345678,
        0x0000000000000080,
        [0x79a503ec, 0xae0ac2c9, 0xa8d49f0b, 0x7e4bfb73],
    ),
    (
        0x12345678,
        0x00000000000000ff,
        [0x562e6bc5, 0xf62a1cb3, 0xd277be76, 0x555f5db2],
    ),
    (
        0x12345678,
        0x0000000000008000,
        [0xfb533894, 0xd73f0348, 0x71532177, 0xbf2463e3],
    ),
    (
        0x12345678,
        0x000000000000ffff,
        [0x562e6bc5, 0x5b574fe2, 0x437295de, 0x284e60fd],
    ),
    (
        0x12345678,
        0x0000000080000000,
        [0xfb533894, 0x55c93830, 0x78826d4c, 0x24e8b84b],
    ),
    (
        0x12345678,
        0x00000000ffffffff,
        [0x562e6bc5, 0x5b574fe2, 0x4dece20c, 0x399d2062],
    ),
    (
        0x12345678,
        0x0123456789abcdef,
        [0x4670acaa, 0xb54a8725, 0xa360621e, 0xa3d207be],
    ),
    (
        0x12345678,
        0x8000000000000000,
        [0xfb533894, 0x55c93830, 0xfa745634, 0xc8bc566f],
    ),
    (
        0x12345678,
        0xffffffffffffffff,
        [0x562e6bc5, 0x5b574fe2, 0x4dece20c, 0x8e05945a],
    ),
    (
        0x82f63b78,
        0x0000000000000000,
        [0xfbc3faf9, 0x8b277743, 0x6ea2d55c, 0xa66805eb],
    ),
    (
        0x82f63b78,
        0x0000000000000001,
        [0x09a879fa, 0x9885ef34, 0xb3e77fe4, 0xef5478cc],
    ),
    (
        0x82f63b78,
        0x0000000000000080,
        [0x7935c181, 0x70e48dba, 0x3c021c63, 0x9269938f],
    ),
    (
        0x82f63b78,
        0x00000000000000ff,
        [0x56bea9a8, 0x28c453c0, 0x46a13d1e, 0xb97d354e],
    ),
    (
        0x82f63b78,
        0x0000000000008000,
        [0xfbc3faf9, 0x09d14c3b, 0xe585a21f, 0x53060b1f],
    ),
    (
        0x82f63b78,
        0x000000000000ffff,
        [0x56bea9a8, 0x85b90091, 0xd7a416b6, 0xc46c0801],
    ),
    (
        0x82f63b78,
        0x0000000080000000,
        [0xfbc3faf9, 0x8b277743, 0xec54ee24, 0xc8cad0b7],
    ),
    (
        0x82f63b78,
        0x00000000ffffffff,
        [0x56bea9a8, 0x85b90091, 0xd93a6164, 0xd5bf489e],
    ),
    (
        0x82f63b78,
        0x0123456789abcdef,
        [0x46e06ec7, 0x6ba4c856, 0x37b6e176, 0x4ff06f42],
    ),
    (
        0x82f63b78,
        0x8000000000000000,
        [0xfbc3faf9, 0x8b277743, 0x6ea2d55c, 0x249e3e93],
    ),
    (
        0x82f63b78,
        0xffffffffffffffff,
        [0x56bea9a8, 0x85b90091, 0xd93a6164, 0x6227fca6],
    ),
];

#[test]
fn crc32_single_steps_match_the_values_measured_on_physical_hardware() {
    for (crc, data, results) in HARDWARE_STEPS {
        // form 0 is the byte form, 2 the word, 3 the dword, 4 the qword form (1 is the REX.W byte form).
        for (form, expected) in [
            (0usize, results[0]),
            (2, results[1]),
            (3, results[2]),
            (4, results[3]),
        ] {
            assert_eq!(
                run_crc(form, crc, data),
                expected,
                "{} crc {crc:#x} data {data:#x}",
                CRC_FORMS[form].0
            );
        }
        assert_eq!(
            run_crc(1, crc, data),
            results[0],
            "REX.W byte form crc {crc:#x} data {data:#x}"
        );
    }
}

#[test]
fn crc32_matches_the_literal_sdm_operation_for_boundary_and_generated_data() {
    let mut rng = Rng(0x5600_0003);
    let mut cases = 0;
    let mut check = |crc: u64, data: u64| {
        for (form, (name, _, _, _, bytes)) in CRC_FORMS.iter().enumerate() {
            let expected = sdm_crc32(crc as u32, data, 8 * *bytes as u32);
            // The result occupies the low dword; the high dword is zeroed, whatever the old destination held.
            assert_eq!(
                run_crc(form, crc, data),
                u64::from(expected),
                "{name} crc {crc:#x} data {data:#x}"
            );
            cases += 1;
        }
    };
    for crc in BOUNDARY {
        for data in BOUNDARY {
            check(crc, data);
        }
    }
    for _ in 0..1500 {
        let (crc, data) = (rng.next(), rng.next());
        check(crc, data);
    }
    assert!(cases > 8_000, "{cases}");
}

#[test]
fn crc32_ignores_the_high_dword_of_the_destination_and_zeroes_it_in_the_result() {
    for form in 0..CRC_FORMS.len() {
        let low = run_crc(form, 0x1234_5678, 0xa5a5_a5a5_a5a5_a5a5);
        for high in [0u64, 1, 0xffff_ffff, 0xdead_beef] {
            let result = run_crc(form, (high << 32) | 0x1234_5678, 0xa5a5_a5a5_a5a5_a5a5);
            assert_eq!(result, low, "{}", CRC_FORMS[form].0);
            assert_eq!(result >> 32, 0, "{}", CRC_FORMS[form].0);
        }
    }
    // The 32-bit destination forms zero bits 63:32 of the register (the 64-bit mode rule for a dword write).
    assert_eq!(run_crc(0, 0xffff_ffff_0000_0000, 0), 0);
}

#[test]
fn crc32_byte_source_register_encodings_follow_the_rex_rule() {
    let mut rng = Rng(0x5600_0004);
    for src in 0..16u8 {
        for dest in [0u8, 1, 7, 8, 15] {
            for (form, force_rex) in [(0usize, false), (0, true), (1, false), (1, true)] {
                let code = crc_reg(form, dest, src, force_rex);
                let rex = force_rex || CRC_FORMS[form].2 || dest >= 8 || src >= 8;
                let mut start = cpu();
                for slot in start.gpr.iter_mut() {
                    *slot = rng.next();
                }
                let mut ram = Ram::new(&code);
                let (outcome, end) = exec(start.clone(), &mut ram);
                assert_eq!(outcome, Ok(StepOutcome::Advanced), "{code:02x?}");
                let data = crc_source(&start, src, 1, rex);
                let initial = start.gpr[usize::from(dest)] as u32;
                let mut expected = start.clone();
                expected.gpr[usize::from(dest)] = u64::from(sdm_crc32(initial, data, 8));
                expected.rip = CODE + code.len() as u64;
                assert_eq!(end, expected, "{code:02x?} src {src} dest {dest}");
            }
        }
    }
    // The decoder reports AH for encoding 4 without REX and SPL with it.
    let decode = |code: &[u8]| {
        long_decode::decode(&mut Ram::new(code), CODE, AccessContext::default()).unwrap()
    };
    let Operation::Crc32 {
        src: Operand::Register(no_rex),
        ..
    } = decode(&[0xf2, 0x0f, 0x38, 0xf0, 0xcc]).operation
    else {
        panic!();
    };
    let Operation::Crc32 {
        src: Operand::Register(with_rex),
        ..
    } = decode(&[0xf2, 0x40, 0x0f, 0x38, 0xf0, 0xcc]).operation
    else {
        panic!();
    };
    assert_ne!(no_rex, with_rex);
    assert_eq!(with_rex.register(), Register::Rsp);
    assert_eq!(no_rex.register(), Register::Rax);
}

#[test]
fn crc32_every_register_pair_and_addressing_form_works() {
    let mut rng = Rng(0x5600_0005);
    for (form_index, (name, _, _, _, bytes)) in CRC_FORMS.iter().enumerate() {
        for dest in 0..16u8 {
            for src in 0..16u8 {
                let code = crc_reg(form_index, dest, src, false);
                let rex = CRC_FORMS[form_index].2 || dest >= 8 || src >= 8;
                let mut start = cpu();
                for slot in start.gpr.iter_mut() {
                    *slot = rng.next();
                }
                let mut ram = Ram::new(&code);
                let (outcome, end) = exec(start.clone(), &mut ram);
                assert_eq!(outcome, Ok(StepOutcome::Advanced), "{name} {code:02x?}");
                let data = crc_source(&start, src, *bytes, rex);
                let mut expected = start.clone();
                expected.gpr[usize::from(dest)] = u64::from(sdm_crc32(
                    start.gpr[usize::from(dest)] as u32,
                    data,
                    8 * *bytes as u32,
                ));
                expected.rip = CODE + code.len() as u64;
                assert_eq!(end, expected, "{name} {code:02x?}");
            }
        }
        let (_, prefix, wide, opcode, _) = CRC_FORMS[form_index];
        for (index, form) in FORMS.iter().enumerate() {
            for dest in [0u8, 6, 9] {
                let code = mem_code(prefix, wide, &[0x0f, 0x38, opcode], dest, form);
                let start = setup(form);
                let value = memory_value(1000 + index as u64);
                let mut ram = Ram::new(&code);
                put_memory(&mut ram, form.address, value);
                let ram_before = ram.bytes.clone();
                let initial = start.gpr[usize::from(dest)] as u32;
                let mut expected = start.clone();
                expected.gpr[usize::from(dest)] =
                    u64::from(sdm_crc32(initial, value, 8 * *bytes as u32));
                expected.rip = CODE + code.len() as u64;
                let decoded =
                    long_decode::decode(&mut Ram::new(&code), CODE, AccessContext::default())
                        .unwrap();
                assert_eq!(decoded.length as usize, code.len(), "{name} {}", form.name);
                assert!(matches!(
                    decoded.operation,
                    Operation::Crc32 {
                        src: Operand::Memory(_),
                        ..
                    }
                ));
                let (outcome, end) = exec(start, &mut ram);
                assert_eq!(outcome, Ok(StepOutcome::Advanced), "{name} {}", form.name);
                assert_eq!(end, expected, "{name} {} r{dest}", form.name);
                assert_eq!(ram.bytes, ram_before);
                assert_eq!(ram.reads, [(form.address, *bytes)], "{name} {}", form.name);
                assert!(ram.writes.is_empty());
            }
        }
    }
}

#[test]
fn crc32_with_66_is_ignored_by_the_byte_form_and_rex_w_wins_over_66() {
    // SDM Vol. 2D, Table A-4 (prefix row `66 & F2`) defines `F0` as CRC32 Gd, Eb and `F1` as CRC32 Gd, Ew, so
    // 66 does not change the byte form and REX.W wins over it; the physical oracle agrees (2968 cases of the
    // oracle corpus with 66 next to F2/F3, 2864 equal, the rest refused by the model).
    for (code, bytes, dest64) in [
        (vec![0x66u8, 0xf2, 0x0f, 0x38, 0xf0, 0xca], 1usize, false),
        (vec![0xf2, 0x66, 0x0f, 0x38, 0xf0, 0xca], 1, false),
        (vec![0x66, 0xf2, 0x48, 0x0f, 0x38, 0xf0, 0xca], 1, true),
        (vec![0x66, 0xf2, 0x48, 0x0f, 0x38, 0xf1, 0xca], 8, true),
        (vec![0xf2, 0x66, 0x48, 0x0f, 0x38, 0xf1, 0xca], 8, true),
    ] {
        let mut start = cpu();
        start.gpr[Register::Rcx as usize] = 0xffff_ffff_8765_4321;
        start.gpr[Register::Rdx as usize] = 0x0123_4567_89ab_cdef;
        let mut ram = Ram::new(&code);
        let (outcome, end) = exec(start, &mut ram);
        assert_eq!(outcome, Ok(StepOutcome::Advanced), "{code:02x?}");
        let expected = sdm_crc32(0x8765_4321, 0x0123_4567_89ab_cdef, 8 * bytes as u32);
        assert_eq!(
            end.gpr[Register::Rcx as usize],
            u64::from(expected),
            "{code:02x?} dest64={dest64}"
        );
    }
}

// =====================================================================================================
// Refusals, profiles, decoder shape
// =====================================================================================================

#[test]
fn neighbouring_encodings_stay_unsupported_without_change() {
    let refused: Vec<(Vec<u8>, &str)> = vec![
        // POPCNT without F3 is not POPCNT (0F B8 is JMPE on other CPUs and #UD on this one); F2 is not the prefix.
        (vec![0x0f, 0xb8, 0xca], "0F B8 without prefix"),
        (vec![0x66, 0x0f, 0xb8, 0xca], "66 0F B8"),
        (vec![0xf2, 0x0f, 0xb8, 0xca], "F2 0F B8"),
        (
            vec![0xf2, 0xf3, 0x0f, 0xb8, 0xca],
            "F2 F3 0F B8 (repeated group)",
        ),
        (vec![0xf3, 0xf3, 0x0f, 0xb8, 0xca], "F3 F3"),
        (vec![0xf3, 0x66, 0x66, 0x0f, 0xb8, 0xca], "duplicate 66"),
        (
            vec![0x64, 0xf3, 0x0f, 0xb8, 0x00],
            "FS segment (no segment base in the operand model)",
        ),
        (vec![0xf3, 0x65, 0x0f, 0xb8, 0x00], "GS segment"),
        (vec![0xf3, 0x40, 0x40, 0x0f, 0xb8, 0xca], "duplicate REX"),
        (vec![0xf0, 0x0f, 0xb8, 0x00], "LOCK without F3"),
        // CRC32 without F2: 0F 38 F0/F1 is MOVBE, owned elsewhere.
        (
            vec![0x0f, 0x38, 0xf0, 0x00],
            "0F 38 F0 without prefix (MOVBE)",
        ),
        (
            vec![0x0f, 0x38, 0xf1, 0x00],
            "0F 38 F1 without prefix (MOVBE)",
        ),
        (
            vec![0x66, 0x0f, 0x38, 0xf1, 0x00],
            "66 0F 38 F1 (MOVBE r/m16)",
        ),
        (vec![0xf3, 0x0f, 0x38, 0xf0, 0xca], "F3 0F 38 F0"),
        (vec![0xf3, 0x0f, 0x38, 0xf1, 0xca], "F3 0F 38 F1"),
        (vec![0xf2, 0xf3, 0x0f, 0x38, 0xf1, 0xca], "F2 F3"),
        (vec![0xf2, 0xf2, 0x0f, 0x38, 0xf1, 0xca], "F2 F2"),
        (vec![0x64, 0xf2, 0x0f, 0x38, 0xf1, 0x00], "CRC32 with FS"),
        (vec![0xf2, 0x65, 0x0f, 0x38, 0xf0, 0x00], "CRC32 with GS"),
        (
            vec![0xf2, 0x0f, 0x38, 0xf2, 0xca],
            "F2 0F 38 F2 (not CRC32)",
        ),
        (vec![0xf2, 0x0f, 0x38, 0xef, 0xca], "F2 0F 38 EF"),
    ];
    for (code, what) in &refused {
        let start = cpu();
        let mut ram = Ram::new(code);
        let (outcome, end) = exec(start.clone(), &mut ram);
        assert!(
            matches!(outcome, Ok(StepOutcome::Unsupported { .. })),
            "{what}: {outcome:?}"
        );
        assert_eq!(end, start, "{what}");
        assert!(ram.reads.is_empty() && ram.writes.is_empty(), "{what}");
    }
}

#[test]
fn the_decoder_reports_operation_registers_operand_width_and_length() {
    let decode = |code: &[u8]| {
        long_decode::decode(&mut Ram::new(code), CODE, AccessContext::default()).unwrap()
    };
    // POPCNT r15w, [r9+0x10]
    let instruction = decode(&[0x66, 0xf3, 0x45, 0x0f, 0xb8, 0x79, 0x10]);
    assert_eq!(instruction.length, 7);
    let Operation::Popcnt {
        dst,
        src: Operand::Memory(memory),
    } = instruction.operation
    else {
        panic!("{:?}", instruction.operation);
    };
    assert_eq!(
        (dst.register(), dst.size()),
        (Register::R15, OperandSize::Word)
    );
    assert_eq!(memory.size, OperandSize::Word);
    assert_eq!(instruction.operand_size, OperandSize::Word);
    // CRC32 r9, r/m8 with REX.W and REX.R: destination qword, source byte.
    let instruction = decode(&[0xf2, 0x4c, 0x0f, 0x38, 0xf0, 0xca]);
    assert_eq!(instruction.length, 6);
    let Operation::Crc32 {
        dst,
        src: Operand::Register(src),
    } = instruction.operation
    else {
        panic!("{:?}", instruction.operation);
    };
    assert_eq!(
        (dst.register(), dst.size()),
        (Register::R9, OperandSize::Qword)
    );
    assert_eq!(
        (src.register(), src.size()),
        (Register::Rdx, OperandSize::Byte)
    );
    // CRC32 r32, r/m16 (66) from memory, and CRC32 r32, r/m32.
    let instruction = decode(&[0x66, 0xf2, 0x0f, 0x38, 0xf1, 0x00]);
    let Operation::Crc32 {
        dst,
        src: Operand::Memory(memory),
    } = instruction.operation
    else {
        panic!("{:?}", instruction.operation);
    };
    assert_eq!(
        (dst.register(), dst.size(), memory.size),
        (Register::Rax, OperandSize::Dword, OperandSize::Word)
    );
    let instruction = decode(&[0xf2, 0x0f, 0x38, 0xf1, 0xc1]);
    let Operation::Crc32 {
        dst,
        src: Operand::Register(src),
    } = instruction.operation
    else {
        panic!("{:?}", instruction.operation);
    };
    assert_eq!(
        (dst.size(), src.size()),
        (OperandSize::Dword, OperandSize::Dword)
    );
    // PCMPGTQ xmm9, xmm12
    let instruction = decode(&[0x66, 0x45, 0x0f, 0x38, 0x37, 0xcc]);
    assert_eq!(instruction.length, 6);
    assert_eq!(
        instruction.operation,
        Operation::Sse41 {
            kind: Sse41Kind::Pcmpgtq,
            xmm: 9,
            operand: Sse41Operand::Xmm(12),
            immediate: 0
        }
    );
}

#[test]
fn flag_definedness_popcnt_defines_all_six_arithmetic_flags_and_crc32_changes_none() {
    let config = RunConfig {
        instruction_budget: 1,
        stop_rip: None,
        context: ctx(PrivilegeLevel::Ring0),
    };
    let mut start = cpu();
    start.rflags = 0x2;
    let code = popcnt_reg(8, 1, 2);
    let mut ram = Ram::new(&code);
    let mut after = start.clone();
    let result = run_with_rflags_defined(&mut after, &mut ram, config, 0x02, || false).unwrap();
    assert_eq!(result.exit, RunExit::InstructionBudgetExhausted);
    assert_eq!(result.rflags_defined, 0x02 | STATUS);
    for initial in [0x02, 0x02 | OF, 0x02 | STATUS | DF] {
        let code = crc_reg(3, 1, 2, false);
        let mut ram = Ram::new(&code);
        let mut after = start.clone();
        let result =
            run_with_rflags_defined(&mut after, &mut ram, config, initial, || false).unwrap();
        assert_eq!(result.rflags_defined, initial);
    }
}

fn cpl0_state() -> LongModeState {
    LongModeState {
        cr0: Some(Cr0Profile::BASE),
        cr4: Some(Cr4Profile::BASE),
        sti_shadow: Some(StiShadow::Clear),
        activity: Some(CpuActivity::Running),
        rip: CODE,
        rflags: 0x2 | STATUS,
        ..LongModeState::default()
    }
}

#[test]
fn the_cpl0_profile_set_executes_popcnt_and_crc32_at_ring0() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let config = RunConfig {
        instruction_budget: 1,
        stop_rip: None,
        context: ctx(PrivilegeLevel::Ring0),
    };
    for (code, rcx_before, rdx, rcx_after, flags_after) in [
        (
            vec![0xf3u8, 0x48, 0x0f, 0xb8, 0xca],
            0u64,
            0xf0f0u64,
            8u64,
            0x2u64,
        ),
        (vec![0xf3, 0x48, 0x0f, 0xb8, 0xca], 7, 0, 0, 0x2 | ZF),
        (
            vec![0xf2, 0x48, 0x0f, 0x38, 0xf1, 0xca],
            0xffff_ffff,
            0,
            u64::from(sdm_crc32(0xffff_ffff, 0, 64)),
            0x2 | STATUS,
        ),
        (
            vec![0xf2, 0x0f, 0x38, 0xf0, 0xca],
            0xffff_ffff,
            0x31,
            u64::from(sdm_crc32(0xffff_ffff, 0x31, 8)),
            0x2 | STATUS,
        ),
    ] {
        let mut cpu = cpl0_state();
        cpu.gpr[Register::Rcx as usize] = rcx_before;
        cpu.gpr[Register::Rdx as usize] = rdx;
        let mut ram = Ram::new(&code);
        let result = run_with_cpl0_profile_set(
            &mut cpu,
            &mut ram,
            config,
            od_isa::INTEGER_FLAGS,
            &set,
            None,
            || false,
        )
        .unwrap();
        assert_eq!(
            result.exit,
            Cpl0ProfileSetExit::Run(RunExit::InstructionBudgetExhausted),
            "{code:02x?}"
        );
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(cpu.gpr[Register::Rcx as usize], rcx_after, "{code:02x?}");
        assert_eq!(cpu.rflags, flags_after, "{code:02x?}");
        assert_eq!(cpu.rip, CODE + code.len() as u64);
    }
}

#[test]
fn the_fp_init_profile_pair_refuses_popcnt_and_crc32_without_change() {
    // The FP-init CR0/CR4 profile admits only its own SSE forms; the general-register forms are not in it.
    for (code, opcode) in [
        (vec![0xf3u8, 0x0f, 0xb8, 0xca], 0xb8u8),
        (vec![0xf2, 0x0f, 0x38, 0xf0, 0xca], 0xf0),
        (vec![0xf2, 0x0f, 0x38, 0xf1, 0xca], 0xf1),
    ] {
        let mut start = cpu();
        start.activity = Some(CpuActivity::Running);
        start.sti_shadow = Some(StiShadow::Clear);
        start.cr0 = Some(0x8001_0033);
        start.cr4 = Some(0x20 | 0x200);
        let mut ram = Ram::new(&code);
        let mut after = start.clone();
        let result = execute::run_with_cr4_profile(
            &mut after,
            &mut ram,
            RunConfig {
                instruction_budget: 1,
                stop_rip: None,
                context: ctx(PrivilegeLevel::Ring0),
            },
            Cr4Profile::FpInitLong64V1,
            || false,
        );
        assert_eq!(result.exit, RunExit::Unsupported { opcode }, "{code:02x?}");
        assert_eq!(after, start);
    }
}

#[test]
fn popcnt_and_crc32_need_neither_sse_nor_x87_state() {
    for code in [
        popcnt_reg(8, 1, 2),
        crc_reg(4, 1, 2, false),
        mem_code(&[0xf3], true, &[0x0f, 0xb8], 1, &FORMS[0]),
    ] {
        let mut start = cpu();
        start.sse = None;
        start.x87 = None;
        start.gpr[Register::Rax as usize] = 0x8000;
        let mut ram = Ram::new(&code);
        assert_eq!(
            exec(start, &mut ram).0,
            Ok(StepOutcome::Advanced),
            "{code:02x?}"
        );
    }
}
