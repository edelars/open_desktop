//! S2-38 (logic and permute remainder): ANDPS/ANDPD, ANDNPS/ANDNPD, ORPS/ORPD, XORPS/XORPD, SHUFPS/SHUFPD,
//! UNPCKLPS/UNPCKHPS/UNPCKLPD/UNPCKHPD and MOVMSKPS/MOVMSKPD on the local SSE state.
//!
//! Every expectation comes from the Intel SDM text (combined SDM 325462-093US, cdrdv2.intel.com/v1/dl/getContent/671200):
//!
//! * ANDNPD/ANDNPS Vol. 2A pp. 3-63..3-68, ANDPD/ANDPS pp. 3-69..3-74, ORPD/ORPS Vol. 2B pp. 4-165..4-170, XORPD/XORPS,
//!   SHUFPD pp. 4-646..4-650, SHUFPS pp. 4-651..4-654, UNPCKHPD pp. 4-744..4-747, UNPCKHPS pp. 4-748..4-751, UNPCKLPD
//!   pp. 4-752..4-755, UNPCKLPS pp. 4-756..4-759 and Vol. 2D XORPD pp. 6-42..6-44, XORPS pp. 6-45..6-47: the legacy-SSE operations are bitwise or lane copies of DEST and SRC (ANDN: NOT DEST AND SRC; SHUFPS: DEST[31:0] and
//!   DEST[63:32] select a dword of DEST by imm8[1:0] and imm8[3:2], DEST[95:64] and DEST[127:96] a dword of SRC by imm8[5:4] and
//!   imm8[7:6]; SHUFPD: DEST[63:0] := imm8[0] ? DEST[127:64] : DEST[63:0], DEST[127:64] := imm8[1] ? SRC[127:64] : SRC[63:0];
//!   UNPCKLPS: DEST[0], SRC[0], DEST[1], SRC[1] by dword; UNPCKHPS: DEST[2], SRC[2], DEST[3], SRC[3]; UNPCKLPD: DEST[63:0],
//!   SRC[63:0]; UNPCKHPD: DEST[127:64], SRC[127:64]); "SIMD Floating-Point Exceptions: None"; the upper bits are unmodified.
//! * Exceptions Type 4 (Vol. 2A Table 2-21 p. 2-29): #UD for CR0.EM = 1, CR4.OSFXSR = 0 or a LOCK prefix, #NM for CR0.TS = 1,
//!   #SS(0) or #GP(0) for a non-canonical address, #GP(0) for a memory operand that is not 16-byte aligned (note 1 exempts only
//!   LDDQU, MOVUPD, MOVUPS and the PCMP*STR* forms), #PF. No #AC row exists, so an unaligned operand is #GP(0) whatever RFLAGS.AC is.
//! * MOVMSKPS (Vol. 2B pp. 4-86..4-87) and MOVMSKPD (pp. 4-84..4-85): the sign bits of the elements form a 4-bit (2-bit) mask in
//!   the low bits of the destination general register, the rest is zero; ModRM.MOD = 11B is required (a memory operand is #UD
//!   on hardware); Exceptions Type 7 (Table 2-24 p. 2-32): #UD for CR0.EM = 1, CR4.OSFXSR = 0 or LOCK, #NM for CR0.TS = 1.
//!
//! Not asserted here because the SDM text does not define it or the model does not carry it: the CPUID feature gate (S2-57), the
//! F2/F3 prefixes with these opcodes, duplicate prefixes and FS/GS overrides (all refused by the decoder as Unsupported).

use od_isa::execute::{self, Cr4Profile, RunConfig, RunExit, StepOutcome};
use od_isa::long_decode::{self, Operation, Sse3Source, SseLogicPermuteKind};
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
const DATA: u64 = 0x8000;

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

/// Distinct bytes per register and lane (a splitmix-style hash): every dword and qword differs, so a wrong lane is visible.
fn xmm_pattern(index: usize) -> [u8; 16] {
    core::array::from_fn(|lane| {
        let mut z = (index as u64 * 16 + lane as u64 + 1).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        (z >> 24) as u8
    })
}

fn memory_pattern(seed: u8) -> [u8; 16] {
    core::array::from_fn(|lane| {
        let mut z =
            (u64::from(seed) * 64 + lane as u64 + 0x1000).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        z = (z ^ (z >> 29)).wrapping_mul(0x94d0_49bb_1331_11eb);
        (z >> 20) as u8
    })
}

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

/// The state that the FP-init CR0/CR4 profile pair admits plus the given control registers.
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
    cpu: &mut LongModeState,
    ram: &mut Ram,
    privilege: PrivilegeLevel,
) -> Result<StepOutcome, MemoryError> {
    execute::step(cpu, ram, ctx(privilege))
}

fn fault(vector: u8, error_code: Option<u32>) -> StepOutcome {
    StepOutcome::Fault(ArchitecturalFault {
        vector,
        error_code,
        linear_address: None,
    })
}

fn operation_of(code: &[u8]) -> Result<Operation, long_decode::DecodeError> {
    long_decode::decode(&mut Ram::new(code), CODE, AccessContext::default())
        .map(|instruction| instruction.operation)
}

struct Spec {
    name: &'static str,
    prefix: &'static [u8],
    opcode: u8,
    immediate: bool,
}

const SPECS: [Spec; 14] = [
    Spec {
        name: "andps",
        prefix: &[],
        opcode: 0x54,
        immediate: false,
    },
    Spec {
        name: "andpd",
        prefix: &[0x66],
        opcode: 0x54,
        immediate: false,
    },
    Spec {
        name: "andnps",
        prefix: &[],
        opcode: 0x55,
        immediate: false,
    },
    Spec {
        name: "andnpd",
        prefix: &[0x66],
        opcode: 0x55,
        immediate: false,
    },
    Spec {
        name: "orps",
        prefix: &[],
        opcode: 0x56,
        immediate: false,
    },
    Spec {
        name: "orpd",
        prefix: &[0x66],
        opcode: 0x56,
        immediate: false,
    },
    Spec {
        name: "xorps",
        prefix: &[],
        opcode: 0x57,
        immediate: false,
    },
    Spec {
        name: "xorpd",
        prefix: &[0x66],
        opcode: 0x57,
        immediate: false,
    },
    Spec {
        name: "unpcklps",
        prefix: &[],
        opcode: 0x14,
        immediate: false,
    },
    Spec {
        name: "unpcklpd",
        prefix: &[0x66],
        opcode: 0x14,
        immediate: false,
    },
    Spec {
        name: "unpckhps",
        prefix: &[],
        opcode: 0x15,
        immediate: false,
    },
    Spec {
        name: "unpckhpd",
        prefix: &[0x66],
        opcode: 0x15,
        immediate: false,
    },
    Spec {
        name: "shufps",
        prefix: &[],
        opcode: 0xc6,
        immediate: true,
    },
    Spec {
        name: "shufpd",
        prefix: &[0x66],
        opcode: 0xc6,
        immediate: true,
    },
];

/// The SDM pseudo-code of each form on dword/qword lane arrays (independent of the byte-array implementation).
fn model(spec: &Spec, dest: [u8; 16], source: [u8; 16], imm: u8) -> [u8; 16] {
    let d = u128::from_le_bytes(dest);
    let s = u128::from_le_bytes(source);
    let dw = |v: [u8; 16]| -> [u32; 4] {
        core::array::from_fn(|i| u32::from_le_bytes(v[i * 4..i * 4 + 4].try_into().unwrap()))
    };
    let qw = |v: [u8; 16]| -> [u64; 2] {
        core::array::from_fn(|i| u64::from_le_bytes(v[i * 8..i * 8 + 8].try_into().unwrap()))
    };
    let from_dw = |l: [u32; 4]| -> [u8; 16] {
        let mut out = [0u8; 16];
        for (i, lane) in l.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&lane.to_le_bytes());
        }
        out
    };
    let from_qw = |l: [u64; 2]| -> [u8; 16] {
        let mut out = [0u8; 16];
        for (i, lane) in l.iter().enumerate() {
            out[i * 8..i * 8 + 8].copy_from_slice(&lane.to_le_bytes());
        }
        out
    };
    let (dd, sd, dq, sq) = (dw(dest), dw(source), qw(dest), qw(source));
    match spec.name {
        "andps" | "andpd" => (d & s).to_le_bytes(),
        "andnps" | "andnpd" => (!d & s).to_le_bytes(),
        "orps" | "orpd" => (d | s).to_le_bytes(),
        "xorps" | "xorpd" => (d ^ s).to_le_bytes(),
        "shufps" => from_dw([
            dd[usize::from(imm & 3)],
            dd[usize::from((imm >> 2) & 3)],
            sd[usize::from((imm >> 4) & 3)],
            sd[usize::from((imm >> 6) & 3)],
        ]),
        "shufpd" => from_qw([dq[usize::from(imm & 1)], sq[usize::from((imm >> 1) & 1)]]),
        "unpcklps" => from_dw([dd[0], sd[0], dd[1], sd[1]]),
        "unpckhps" => from_dw([dd[2], sd[2], dd[3], sd[3]]),
        "unpcklpd" => from_qw([dq[0], sq[0]]),
        "unpckhpd" => from_qw([dq[1], sq[1]]),
        other => panic!("{other}"),
    }
}

fn rex_for(reg: u8, rm: u8, extra: u8) -> Option<u8> {
    let rex = extra | if reg >= 8 { 4 } else { 0 } | if rm >= 8 { 1 } else { 0 };
    (rex != 0).then_some(0x40 | rex)
}

fn register_code(spec: &Spec, destination: u8, source: u8, imm: u8, extra_rex: u8) -> Vec<u8> {
    let mut code = spec.prefix.to_vec();
    if let Some(rex) = rex_for(destination, source, extra_rex) {
        code.push(rex);
    }
    code.extend_from_slice(&[
        0x0f,
        spec.opcode,
        0xc0 | ((destination & 7) << 3) | (source & 7),
    ]);
    if spec.immediate {
        code.push(imm);
    }
    code
}

#[test]
fn register_forms_follow_the_sdm_lane_rules_and_touch_nothing_else() {
    for spec in &SPECS {
        for destination in 0..16u8 {
            for source in 0..16u8 {
                let imm = (destination * 16 + source).wrapping_mul(37) ^ 0x5a;
                for extra_rex in [0u8, 8] {
                    if extra_rex != 0 && (destination + source) % 5 != 0 {
                        continue;
                    }
                    let code = register_code(spec, destination, source, imm, extra_rex);
                    let mut cpu = cpu();
                    let mut ram = Ram::new(&code);
                    let mut expected = cpu.clone();
                    let sse = expected.sse.as_mut().unwrap();
                    sse.xmm[usize::from(destination)] = model(
                        spec,
                        xmm_pattern(usize::from(destination)),
                        xmm_pattern(usize::from(source)),
                        imm,
                    );
                    expected.rip = CODE + code.len() as u64;
                    let label = format!("{} {code:02x?}", spec.name);
                    assert_eq!(
                        step(&mut cpu, &mut ram, PrivilegeLevel::Ring0),
                        Ok(StepOutcome::Advanced),
                        "{label}"
                    );
                    assert_eq!(cpu, expected, "{label}");
                    assert!(ram.reads.is_empty() && ram.writes.is_empty(), "{label}");
                }
            }
        }
    }
}

#[test]
fn every_shuffle_immediate_selects_the_lanes_the_sdm_gives() {
    for spec in SPECS.iter().filter(|spec| spec.immediate) {
        for (destination, source) in [(0u8, 1u8), (11, 4), (3, 3), (15, 8)] {
            for imm in 0..=255u8 {
                let code = register_code(spec, destination, source, imm, 0);
                let mut cpu = cpu();
                let mut ram = Ram::new(&code);
                assert_eq!(
                    step(&mut cpu, &mut ram, PrivilegeLevel::Ring3),
                    Ok(StepOutcome::Advanced)
                );
                let dest = xmm_pattern(usize::from(destination));
                let src = xmm_pattern(usize::from(source));
                assert_eq!(
                    cpu.sse.as_ref().unwrap().xmm[usize::from(destination)],
                    model(spec, dest, src, imm),
                    "{} {destination} {source} {imm:#04x}",
                    spec.name
                );
            }
        }
    }
}

#[test]
fn literal_vectors_for_every_form() {
    // DEST dwords 00112233 44556677 8899aabb ccddeeff, SRC dwords 0f1e2d3c 4b5a6978 8796a5b4 c3d2e1f0.
    let dest: [u8; 16] = [
        0x33, 0x22, 0x11, 0x00, 0x77, 0x66, 0x55, 0x44, 0xbb, 0xaa, 0x99, 0x88, 0xff, 0xee, 0xdd,
        0xcc,
    ];
    let source: [u8; 16] = [
        0x3c, 0x2d, 0x1e, 0x0f, 0x78, 0x69, 0x5a, 0x4b, 0xb4, 0xa5, 0x96, 0x87, 0xf0, 0xe1, 0xd2,
        0xc3,
    ];
    let dw = |l: [u32; 4]| -> [u8; 16] {
        let mut out = [0u8; 16];
        for (i, lane) in l.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&lane.to_le_bytes());
        }
        out
    };
    // (name, imm, expected dwords)
    let cases: [(&str, u8, [u32; 4]); 11] = [
        ("andps", 0, [0x00102030, 0x40506070, 0x8090a0b0, 0xc0d0e0f0]),
        (
            "andnps",
            0,
            [0x0f0e0d0c, 0x0b0a0908, 0x07060504, 0x03020100],
        ),
        ("orps", 0, [0x0f1f2f3f, 0x4f5f6f7f, 0x8f9fafbf, 0xcfdfefff]),
        ("xorps", 0, [0x0f0f0f0f, 0x0f0f0f0f, 0x0f0f0f0f, 0x0f0f0f0f]),
        (
            "unpcklps",
            0,
            [0x00112233, 0x0f1e2d3c, 0x44556677, 0x4b5a6978],
        ),
        (
            "unpckhps",
            0,
            [0x8899aabb, 0x8796a5b4, 0xccddeeff, 0xc3d2e1f0],
        ),
        (
            "shufps",
            0x1b,
            [0xccddeeff, 0x8899aabb, 0x4b5a6978, 0x0f1e2d3c],
        ),
        (
            "shufps",
            0xe4,
            [0x00112233, 0x44556677, 0x8796a5b4, 0xc3d2e1f0],
        ),
        (
            "shufps",
            0x00,
            [0x00112233, 0x00112233, 0x0f1e2d3c, 0x0f1e2d3c],
        ),
        (
            "shufps",
            0xff,
            [0xccddeeff, 0xccddeeff, 0xc3d2e1f0, 0xc3d2e1f0],
        ),
        (
            "shufps",
            0x4e,
            [0x8899aabb, 0xccddeeff, 0x0f1e2d3c, 0x4b5a6978],
        ),
    ];
    for (name, imm, lanes) in cases {
        let spec = SPECS.iter().find(|spec| spec.name == name).unwrap();
        assert_eq!(model(spec, dest, source, imm), dw(lanes), "{name} {imm:#x}");
        let code = register_code(spec, 2, 3, imm, 0);
        let mut cpu = cpu();
        cpu.sse.as_mut().unwrap().xmm[2] = dest;
        cpu.sse.as_mut().unwrap().xmm[3] = source;
        let mut ram = Ram::new(&code);
        assert_eq!(
            step(&mut cpu, &mut ram, PrivilegeLevel::Ring0),
            Ok(StepOutcome::Advanced)
        );
        assert_eq!(
            cpu.sse.as_ref().unwrap().xmm[2],
            dw(lanes),
            "{name} {imm:#x}"
        );
    }
    // Packed-double forms: DEST qwords 4455667700112233 ccddeeff8899aabb, SRC qwords 4b5a69780f1e2d3c c3d2e1f08796a5b4.
    let qw = |l: [u64; 2]| -> [u8; 16] {
        let mut out = [0u8; 16];
        out[..8].copy_from_slice(&l[0].to_le_bytes());
        out[8..].copy_from_slice(&l[1].to_le_bytes());
        out
    };
    let (d0, d1) = (0x4455_6677_0011_2233u64, 0xccdd_eeff_8899_aabbu64);
    let (s0, s1) = (0x4b5a_6978_0f1e_2d3cu64, 0xc3d2_e1f0_8796_a5b4u64);
    let cases: [(&str, u8, [u64; 2]); 8] = [
        ("unpcklpd", 0, [d0, s0]),
        ("unpckhpd", 0, [d1, s1]),
        ("shufpd", 0b00, [d0, s0]),
        ("shufpd", 0b01, [d1, s0]),
        ("shufpd", 0b10, [d0, s1]),
        ("shufpd", 0b11, [d1, s1]),
        // imm8[7:2] are ignored by the legacy encoding.
        ("shufpd", 0xfc, [d0, s0]),
        ("shufpd", 0xfd, [d1, s0]),
    ];
    let mut dest_q = [0u8; 16];
    dest_q[..8].copy_from_slice(&d0.to_le_bytes());
    dest_q[8..].copy_from_slice(&d1.to_le_bytes());
    let mut source_q = [0u8; 16];
    source_q[..8].copy_from_slice(&s0.to_le_bytes());
    source_q[8..].copy_from_slice(&s1.to_le_bytes());
    for (name, imm, lanes) in cases {
        let spec = SPECS.iter().find(|spec| spec.name == name).unwrap();
        let code = register_code(spec, 4, 6, imm, 0);
        let mut cpu = cpu();
        cpu.sse.as_mut().unwrap().xmm[4] = dest_q;
        cpu.sse.as_mut().unwrap().xmm[6] = source_q;
        let mut ram = Ram::new(&code);
        assert_eq!(
            step(&mut cpu, &mut ram, PrivilegeLevel::Ring0),
            Ok(StepOutcome::Advanced)
        );
        assert_eq!(
            cpu.sse.as_ref().unwrap().xmm[4],
            qw(lanes),
            "{name} {imm:#x}"
        );
    }
}

// ---- memory forms ----

struct Mem {
    label: &'static str,
    /// Legacy prefixes between the group prefix and the opcode (67).
    extra: &'static [u8],
    /// REX.B / REX.X bits of the addressing form.
    rex: u8,
    /// ModRM.mod and ModRM.r/m.
    mode: u8,
    rm: u8,
    sib: Option<u8>,
    disp: Disp,
    regs: &'static [(Register, u64)],
    /// The effective address that SDM Vol. 1 3.7.5 gives for the registers.
    address: u64,
}

enum Disp {
    None,
    D8(i8),
    D32(i32),
    /// RIP-relative: the absolute target; disp32 is computed from the end of the instruction.
    Rip(u64),
}

const MEMS: &[Mem] = &[
    Mem {
        label: "[rax]",
        extra: &[],
        rex: 0,
        mode: 0,
        rm: 0,
        sib: None,
        disp: Disp::None,
        regs: &[(Register::Rax, 0x8010)],
        address: 0x8010,
    },
    Mem {
        label: "[rbx+0x10]",
        extra: &[],
        rex: 0,
        mode: 1,
        rm: 3,
        sib: None,
        disp: Disp::D8(0x10),
        regs: &[(Register::Rbx, 0x8100)],
        address: 0x8110,
    },
    Mem {
        label: "[rsp]",
        extra: &[],
        rex: 0,
        mode: 0,
        rm: 4,
        sib: Some(0x24),
        disp: Disp::None,
        regs: &[(Register::Rsp, 0x8210)],
        address: 0x8210,
    },
    Mem {
        label: "[rbp-0x20]",
        extra: &[],
        rex: 0,
        mode: 1,
        rm: 5,
        sib: None,
        disp: Disp::D8(-0x20),
        regs: &[(Register::Rbp, 0x8320)],
        address: 0x8300,
    },
    Mem {
        label: "[rax+rcx*4+0x10]",
        extra: &[],
        rex: 0,
        mode: 1,
        rm: 4,
        sib: Some(0x88),
        disp: Disp::D8(0x10),
        regs: &[(Register::Rax, 0x8400), (Register::Rcx, 4)],
        address: 0x8420,
    },
    Mem {
        label: "[rbx+disp32]",
        extra: &[],
        rex: 0,
        mode: 2,
        rm: 3,
        sib: None,
        disp: Disp::D32(0x120),
        regs: &[(Register::Rbx, 0x8500)],
        address: 0x8620,
    },
    Mem {
        label: "[r8]",
        extra: &[],
        rex: 1,
        mode: 0,
        rm: 0,
        sib: None,
        disp: Disp::None,
        regs: &[(Register::R8, 0x8740)],
        address: 0x8740,
    },
    Mem {
        label: "[r13+0x10]",
        extra: &[],
        rex: 1,
        mode: 1,
        rm: 5,
        sib: None,
        disp: Disp::D8(0x10),
        regs: &[(Register::R13, 0x8750)],
        address: 0x8760,
    },
    Mem {
        label: "[rbx+r9*8+0x20]",
        extra: &[],
        rex: 2,
        mode: 1,
        rm: 4,
        sib: Some(0xcb),
        disp: Disp::D8(0x20),
        regs: &[(Register::Rbx, 0x8800), (Register::R9, 2)],
        address: 0x8830,
    },
    Mem {
        label: "[disp32 abs]",
        extra: &[],
        rex: 0,
        mode: 0,
        rm: 4,
        sib: Some(0x25),
        disp: Disp::D32(0x8900),
        regs: &[],
        address: 0x8900,
    },
    Mem {
        label: "[rip+disp32] forward",
        extra: &[],
        rex: 0,
        mode: 0,
        rm: 5,
        sib: None,
        disp: Disp::Rip(0x8a00),
        regs: &[],
        address: 0x8a00,
    },
    Mem {
        label: "[rip+disp32] backward",
        extra: &[],
        rex: 0,
        mode: 0,
        rm: 5,
        sib: None,
        disp: Disp::Rip(0x1000),
        regs: &[],
        address: 0x1000,
    },
    Mem {
        label: "[rax] REX.W",
        extra: &[],
        rex: 8,
        mode: 0,
        rm: 0,
        sib: None,
        disp: Disp::None,
        regs: &[(Register::Rax, 0x8a20)],
        address: 0x8a20,
    },
    Mem {
        label: "[eax] 67",
        extra: &[0x67],
        rex: 0,
        mode: 0,
        rm: 0,
        sib: None,
        disp: Disp::None,
        regs: &[(Register::Rax, 0xdead_beef_0000_8a40)],
        address: 0x8a40,
    },
    Mem {
        label: "[eax+ecx*4+0x10] 67",
        extra: &[0x67],
        rex: 0,
        mode: 1,
        rm: 4,
        sib: Some(0x88),
        disp: Disp::D8(0x10),
        regs: &[
            (Register::Rax, 0xffff_ffff_0000_8b00),
            (Register::Rcx, 0x1_0000_0004),
        ],
        address: 0x8b20,
    },
    Mem {
        label: "[rax] CS",
        extra: &[0x2e],
        rex: 0,
        mode: 0,
        rm: 0,
        sib: None,
        disp: Disp::None,
        regs: &[(Register::Rax, 0x8c00)],
        address: 0x8c00,
    },
    Mem {
        label: "[rax] SS",
        extra: &[0x36],
        rex: 0,
        mode: 0,
        rm: 0,
        sib: None,
        disp: Disp::None,
        regs: &[(Register::Rax, 0x8c10)],
        address: 0x8c10,
    },
    Mem {
        label: "[rax] DS",
        extra: &[0x3e],
        rex: 0,
        mode: 0,
        rm: 0,
        sib: None,
        disp: Disp::None,
        regs: &[(Register::Rax, 0x8c20)],
        address: 0x8c20,
    },
    Mem {
        label: "[rax] ES",
        extra: &[0x26],
        rex: 0,
        mode: 0,
        rm: 0,
        sib: None,
        disp: Disp::None,
        regs: &[(Register::Rax, 0x8c30)],
        address: 0x8c30,
    },
];

fn memory_code(spec: &Spec, destination: u8, mem: &Mem, imm: u8) -> Vec<u8> {
    let mut code = mem.extra.to_vec();
    code.extend_from_slice(spec.prefix);
    if let Some(rex) = rex_for(destination, 0, mem.rex) {
        code.push(rex);
    }
    code.extend_from_slice(&[
        0x0f,
        spec.opcode,
        (mem.mode << 6) | ((destination & 7) << 3) | mem.rm,
    ]);
    if let Some(sib) = mem.sib {
        code.push(sib);
    }
    match mem.disp {
        Disp::None => {}
        Disp::D8(value) => code.push(value as u8),
        Disp::D32(value) => code.extend_from_slice(&value.to_le_bytes()),
        Disp::Rip(target) => {
            let end = CODE + code.len() as u64 + 4 + u64::from(spec.immediate);
            code.extend_from_slice(&((target.wrapping_sub(end)) as i32).to_le_bytes());
        }
    }
    if spec.immediate {
        code.push(imm);
    }
    code
}

fn with_memory(mem: &Mem, delta: u64, code: &[u8]) -> (LongModeState, Ram) {
    let mut cpu = cpu();
    for (register, value) in mem.regs {
        cpu.gpr[*register as usize] = value + delta;
    }
    let mut ram = Ram::new(code);
    let address = (mem.address + delta) as usize;
    ram.bytes[address..address + 16].copy_from_slice(&memory_pattern(7));
    (cpu, ram)
}

#[test]
fn memory_sources_read_sixteen_bytes_in_every_addressing_form() {
    for spec in &SPECS {
        for mem in MEMS {
            for privilege in [PrivilegeLevel::Ring0, PrivilegeLevel::Ring3] {
                let destination = (mem.address % 13) as u8 + 1;
                let imm = 0xb1u8.wrapping_add(destination);
                let code = memory_code(spec, destination, mem, imm);
                let (mut cpu, mut ram) = with_memory(mem, 0, &code);
                let mut expected = cpu.clone();
                expected.sse.as_mut().unwrap().xmm[usize::from(destination)] = model(
                    spec,
                    xmm_pattern(usize::from(destination)),
                    memory_pattern(7),
                    imm,
                );
                expected.rip = CODE + code.len() as u64;
                let ram_before = ram.bytes.clone();
                let label = format!("{} {} {code:02x?} {privilege:?}", spec.name, mem.label);
                assert_eq!(
                    step(&mut cpu, &mut ram, privilege),
                    Ok(StepOutcome::Advanced),
                    "{label}"
                );
                assert_eq!(cpu, expected, "{label}");
                assert_eq!(ram.reads, vec![(mem.address, 16)], "{label}");
                assert!(ram.writes.is_empty(), "{label}");
                assert_eq!(ram.bytes, ram_before, "{label}");
            }
        }
    }
}

#[test]
fn an_operand_that_is_not_sixteen_byte_aligned_is_general_protection_even_with_the_alignment_check_armed()
 {
    for spec in &SPECS {
        for delta in [1u64, 2, 4, 7, 8, 15] {
            for (privilege, flags) in [
                (PrivilegeLevel::Ring0, FLAGS),
                (PrivilegeLevel::Ring3, FLAGS),
                (PrivilegeLevel::Ring3, FLAGS | (1 << 18)),
            ] {
                let mem = &MEMS[0];
                let code = memory_code(spec, 3, mem, 0x1b);
                let (mut cpu, mut ram) = with_memory(mem, delta, &code);
                cpu.rflags = flags;
                let before = cpu.clone();
                let label = format!("{} +{delta} {privilege:?} {flags:#x}", spec.name);
                assert_eq!(
                    step(&mut cpu, &mut ram, privilege),
                    Ok(fault(13, Some(0))),
                    "{label}"
                );
                assert_eq!(cpu, before, "{label}");
                assert!(ram.reads.is_empty() && ram.writes.is_empty(), "{label}");
            }
        }
    }
}

#[test]
fn addresses_outside_the_canonical_range_fault_before_any_access() {
    for spec in &SPECS {
        for (label, base, stack) in [
            ("non-canonical rax", 0x0000_8000_0000_0000u64, false),
            ("non-canonical high rax", 0xffff_7fff_ffff_fff0, false),
            ("garbage rax", 0x1234_5678_1234_5670, false),
            ("non-canonical rsp", 0x0000_8000_0000_0000, true),
            ("non-canonical rbp", 0x0100_0000_0000_0000, true),
        ] {
            let mut code = spec.prefix.to_vec();
            code.extend_from_slice(&[0x0f, spec.opcode]);
            // [rax], [rsp] (SIB) or [rbp+0].
            let (cpu_setup, vector): (Box<dyn Fn(&mut LongModeState)>, u8) =
                if label.contains("rsp") {
                    code.extend_from_slice(&[0x04, 0x24]);
                    (
                        Box::new(move |cpu| cpu.gpr[Register::Rsp as usize] = base),
                        12,
                    )
                } else if label.contains("rbp") {
                    code.extend_from_slice(&[0x45, 0x00]);
                    (
                        Box::new(move |cpu| cpu.gpr[Register::Rbp as usize] = base),
                        12,
                    )
                } else {
                    code.push(0x00);
                    (
                        Box::new(move |cpu| cpu.gpr[Register::Rax as usize] = base),
                        13,
                    )
                };
            assert!(stack || vector == 13);
            if spec.immediate {
                code.push(0x44);
            }
            let mut cpu = cpu();
            cpu_setup(&mut cpu);
            let mut ram = Ram::new(&code);
            let before = cpu.clone();
            assert_eq!(
                step(&mut cpu, &mut ram, PrivilegeLevel::Ring0),
                Ok(fault(vector, Some(0))),
                "{} {label}",
                spec.name
            );
            assert_eq!(cpu, before, "{} {label}", spec.name);
            assert!(ram.reads.is_empty() && ram.writes.is_empty());
        }
    }
}

#[test]
fn a_backend_page_fault_and_an_unmapped_range_leave_the_cpu_state_untouched() {
    for spec in &SPECS {
        let mem = &MEMS[0];
        let code = memory_code(spec, 5, mem, 0x2d);
        // The faulting page holds the operand.
        let (mut cpu, mut ram) = with_memory(mem, 0, &code);
        ram.fault_page = Some(0x8000);
        let before = cpu.clone();
        let outcome = step(&mut cpu, &mut ram, PrivilegeLevel::Ring0);
        assert_eq!(
            outcome,
            Ok(StepOutcome::Fault(ArchitecturalFault {
                vector: 14,
                error_code: Some(0),
                linear_address: Some(0x8010),
            })),
            "{}",
            spec.name
        );
        assert_eq!(cpu, before, "{}", spec.name);
        // The operand lies (partly) outside the backend's range: the access error is the backend's result.
        let (mut cpu, mut ram) = with_memory(mem, 0, &code);
        cpu.gpr[Register::Rax as usize] = RAM as u64;
        let before = cpu.clone();
        assert!(matches!(
            step(&mut cpu, &mut ram, PrivilegeLevel::Ring0),
            Err(MemoryError::Unmapped { .. }) | Ok(StepOutcome::Fault(_))
        ));
        assert_eq!(cpu, before, "{}", spec.name);
    }
}

// ---- control registers, LOCK, refused encodings ----

const CR0_BASE: u64 = 0x8001_0033; // PG | WP | NE | ET | MP | PE: EM = 0, TS = 0
const CR4_PAE: u64 = 0x20;
const CR4_OSFXSR: u64 = 0x200;

fn gate_cases() -> [(u64, u64, Option<u8>); 8] {
    [
        (CR0_BASE, CR4_PAE | CR4_OSFXSR, None),
        (CR0_BASE | 4, CR4_PAE | CR4_OSFXSR, Some(6)),
        (CR0_BASE | 8, CR4_PAE | CR4_OSFXSR, Some(7)),
        (CR0_BASE | 4 | 8, CR4_PAE | CR4_OSFXSR, Some(6)),
        (CR0_BASE, CR4_PAE, Some(6)),
        (CR0_BASE | 4, CR4_PAE, Some(6)),
        (CR0_BASE | 8, CR4_PAE, Some(6)),
        (CR0_BASE | 4 | 8, CR4_PAE, Some(6)),
    ]
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
fn cr0_em_ts_and_cr4_osfxsr_gate_every_form_before_any_address_check() {
    for spec in &SPECS {
        // (code, memory form, the address register is RBP: a non-canonical address is then #SS(0))
        let mut codes = vec![
            (register_code(spec, 1, 2, 0x4e, 0), false, false),
            (
                [
                    spec.prefix,
                    &[0x0f, spec.opcode, 0x00],
                    if spec.immediate { &[0x4e][..] } else { &[] },
                ]
                .concat(),
                true,
                false,
            ),
        ];
        codes.push((memory_code(spec, 1, &MEMS[3], 0x4e), true, true));
        for (code, memory_form, stack) in codes {
            for (cr0, cr4, vector) in gate_cases() {
                for address in [DATA, 0x0000_8000_0000_0000, DATA + 1] {
                    let mut cpu = fp_init_cpu(cr0, cr4);
                    cpu.gpr[Register::Rax as usize] = address;
                    cpu.gpr[Register::Rbp as usize] = address + 0x20;
                    let mut ram = Ram::new(&code);
                    let before = cpu.clone();
                    let ram_before = ram.bytes.clone();
                    let label = format!(
                        "{} {code:02x?} cr0={cr0:#x} cr4={cr4:#x} {address:#x}",
                        spec.name
                    );
                    let (result, cpu) = run_fp_init(cpu, &mut ram, PrivilegeLevel::Ring0);
                    let expected_exit = match vector {
                        Some(vector) => RunExit::Fault(ArchitecturalFault {
                            vector,
                            error_code: None,
                            linear_address: None,
                        }),
                        None if !memory_form || address == DATA => {
                            RunExit::InstructionBudgetExhausted
                        }
                        // A non-canonical address (#SS(0) for an RBP base), and an operand that is not 16-byte aligned.
                        None => RunExit::Fault(ArchitecturalFault {
                            vector: if stack && address != DATA + 1 { 12 } else { 13 },
                            error_code: Some(0),
                            linear_address: None,
                        }),
                    };
                    assert_eq!(result.exit, expected_exit, "{label}");
                    if vector.is_some() {
                        // The gate precedes every address check: nothing moves, nothing is read.
                        assert_eq!(result.instructions_executed, 0, "{label}");
                        assert_eq!(cpu, before, "{label}");
                        assert_eq!(ram.bytes, ram_before, "{label}");
                        assert!(ram.reads.is_empty() && ram.writes.is_empty(), "{label}");
                    } else if expected_exit == RunExit::InstructionBudgetExhausted {
                        assert_eq!(result.instructions_executed, 1, "{label}");
                    }
                }
            }
        }
    }
}

#[test]
fn the_fp_init_profile_runs_the_forms_at_every_privilege_level() {
    for spec in &SPECS {
        for privilege in [PrivilegeLevel::Ring0, PrivilegeLevel::Ring3] {
            let code = register_code(spec, 6, 7, 0x93, 0);
            let cpu = fp_init_cpu(CR0_BASE, CR4_PAE | CR4_OSFXSR);
            let before = cpu.clone();
            let (result, after) = run_fp_init(cpu, &mut Ram::new(&code), privilege);
            assert_eq!(
                result.exit,
                RunExit::InstructionBudgetExhausted,
                "{}",
                spec.name
            );
            let mut expected = before.clone();
            expected.rip = CODE + code.len() as u64;
            expected.sse.as_mut().unwrap().xmm[6] =
                model(spec, xmm_pattern(6), xmm_pattern(7), 0x93);
            // The FP-init run keeps the control registers and the CPU activity as they are.
            assert_eq!(after.sse, expected.sse, "{}", spec.name);
            assert_eq!(after.rip, expected.rip, "{}", spec.name);
            assert_eq!(after.gpr, before.gpr, "{}", spec.name);
        }
    }
}

#[test]
fn lock_is_invalid_opcode_for_every_form_before_any_state_change() {
    for spec in &SPECS {
        let mut codes = vec![
            [
                &[0xf0][..],
                spec.prefix,
                &[0x0f, spec.opcode, 0xc1],
                if spec.immediate { &[0x01][..] } else { &[] },
            ]
            .concat(),
            [
                spec.prefix,
                &[0xf0, 0x0f, spec.opcode, 0xc1],
                if spec.immediate { &[0x01][..] } else { &[] },
            ]
            .concat(),
            [
                &[0xf0][..],
                spec.prefix,
                &[0x0f, spec.opcode, 0x00],
                if spec.immediate { &[0x01][..] } else { &[] },
            ]
            .concat(),
            [
                &[0xf0][..],
                spec.prefix,
                &[0x48, 0x0f, spec.opcode, 0x00],
                if spec.immediate { &[0x01][..] } else { &[] },
            ]
            .concat(),
        ];
        codes.push(
            [
                &[0xf0][..],
                spec.prefix,
                &[0x0f, spec.opcode, 0x05, 0x00, 0x00, 0x00, 0x00],
                if spec.immediate { &[0x01][..] } else { &[] },
            ]
            .concat(),
        );
        for code in codes {
            for privilege in [PrivilegeLevel::Ring0, PrivilegeLevel::Ring3] {
                let mut cpu = cpu();
                let mut ram = Ram::new(&code);
                let before = cpu.clone();
                assert_eq!(
                    step(&mut cpu, &mut ram, privilege),
                    Ok(fault(6, None)),
                    "{} {code:02x?}",
                    spec.name
                );
                assert_eq!(cpu, before);
                assert!(ram.reads.is_empty() && ram.writes.is_empty());
            }
        }
    }
}

#[test]
fn encodings_outside_the_slice_are_refused_without_a_state_change() {
    let refused: [(&str, Vec<u8>); 12] = [
        ("F3 ANDPS", vec![0xf3, 0x0f, 0x54, 0xc1]),
        ("F2 ANDPS", vec![0xf2, 0x0f, 0x54, 0x00]),
        ("F3 66 ANDPS", vec![0xf3, 0x66, 0x0f, 0x54, 0xc1]),
        ("F2 UNPCKLPS", vec![0xf2, 0x0f, 0x14, 0xc1]),
        ("F3 SHUFPS", vec![0xf3, 0x0f, 0xc6, 0xc1, 0x01]),
        ("duplicate 66", vec![0x66, 0x66, 0x0f, 0x57, 0xc1]),
        ("FS ANDPS", vec![0x64, 0x0f, 0x54, 0x00]),
        ("GS ORPD", vec![0x65, 0x66, 0x0f, 0x56, 0x00]),
        ("two REX", vec![0x40, 0x44, 0x0f, 0x57, 0xc1]),
        ("F3 MOVMSKPS", vec![0xf3, 0x0f, 0x50, 0xc1]),
        ("F2 MOVMSKPD", vec![0xf2, 0x66, 0x0f, 0x50, 0xc1]),
        ("FS MOVMSKPS", vec![0x64, 0x0f, 0x50, 0xc1]),
    ];
    for (label, code) in refused {
        let mut cpu = cpu();
        let mut ram = Ram::new(&code);
        let before = cpu.clone();
        let outcome = step(&mut cpu, &mut ram, PrivilegeLevel::Ring0);
        assert!(
            matches!(outcome, Ok(StepOutcome::Unsupported { .. })),
            "{label}: {outcome:?}"
        );
        assert_eq!(cpu, before, "{label}");
        assert!(ram.reads.is_empty() && ram.writes.is_empty(), "{label}");
    }
}

#[test]
fn no_mxcsr_bit_or_flag_is_read_or_written_and_signaling_nans_are_copied() {
    // Reserved MXCSR bits, every exception unmasked, every flag set, DAZ/FTZ on: the result and MXCSR do not change.
    let snan32 = 0x7fa0_0001u32.to_le_bytes();
    let snan64 = 0x7ff4_0000_0000_0001u64.to_le_bytes();
    for mxcsr in [
        0xffff_ffffu32,
        0x0000_ffff & !0x1f80 | 0x3f,
        0x8040,
        0x1f80 | 0x3f,
    ] {
        for spec in &SPECS {
            let code = register_code(spec, 0, 1, 0xd8, 0);
            let mut cpu = cpu();
            cpu.rflags = 0x2 | 0x8d5 | (1 << 10);
            let sse = cpu.sse.as_mut().unwrap();
            sse.mxcsr = mxcsr;
            for chunk in 0..4 {
                sse.xmm[0][chunk * 4..chunk * 4 + 4].copy_from_slice(&snan32);
            }
            sse.xmm[0][8..].copy_from_slice(&snan64);
            sse.xmm[1] = [0xff; 16];
            let mut expected = cpu.clone();
            let dest = cpu.sse.as_ref().unwrap().xmm[0];
            expected.sse.as_mut().unwrap().xmm[0] = model(spec, dest, [0xff; 16], 0xd8);
            expected.rip = CODE + code.len() as u64;
            let mut ram = Ram::new(&code);
            assert_eq!(
                step(&mut cpu, &mut ram, PrivilegeLevel::Ring0),
                Ok(StepOutcome::Advanced),
                "{} {mxcsr:#x}",
                spec.name
            );
            assert_eq!(cpu, expected, "{} {mxcsr:#x}", spec.name);
        }
    }
}

// ---- MOVMSKPS / MOVMSKPD ----

fn movmsk_code(double: bool, destination: u8, source: u8, extra_rex: u8) -> Vec<u8> {
    let mut code = if double { vec![0x66] } else { vec![] };
    if let Some(rex) = rex_for(destination, source, extra_rex) {
        code.push(rex);
    }
    code.extend_from_slice(&[0x0f, 0x50, 0xc0 | ((destination & 7) << 3) | (source & 7)]);
    code
}

/// An XMM value whose element sign bits are the given pattern (bit i is the sign of element i); the other bits are ones
/// and zeros chosen so that an implementation reading a wrong bit position gets a different mask.
fn sign_pattern(signs: u8, double: bool) -> [u8; 16] {
    let mut raw = [0x7f_u8; 16];
    for index in 0..16 {
        // Bytes that are not the top byte of an element hold 0xff or 0x00 alternating (their top bits are ones).
        raw[index] = if index % 2 == 0 { 0x7f } else { 0xff };
    }
    let (width, count) = if double { (8, 2) } else { (4, 4) };
    for element in 0..count {
        let top = element * width + width - 1;
        raw[top] = if signs >> element & 1 != 0 {
            0x80
        } else {
            0x7f
        };
    }
    raw
}

#[test]
fn movmsk_builds_the_sign_mask_and_zero_extends_into_the_whole_register() {
    for double in [false, true] {
        let count = if double { 4u8 } else { 16 };
        for signs in 0..count {
            for destination in 0..16u8 {
                if destination == 4 {
                    // RSP: keep the stack pointer out of the comparison below by also covering it explicitly.
                }
                for (source, extra_rex) in [(1u8, 0u8), (12, 0), (3, 8), (9, 8)] {
                    let code = movmsk_code(double, destination, source, extra_rex);
                    let mut cpu = cpu();
                    cpu.sse.as_mut().unwrap().xmm[usize::from(source)] =
                        sign_pattern(signs, double);
                    cpu.gpr[Register::from_encoding(destination).unwrap() as usize] = u64::MAX;
                    let mut expected = cpu.clone();
                    expected.gpr[Register::from_encoding(destination).unwrap() as usize] =
                        u64::from(signs);
                    expected.rip = CODE + code.len() as u64;
                    let mut ram = Ram::new(&code);
                    let label = format!(
                        "double={double} signs={signs:#x} g{destination} x{source} {code:02x?}"
                    );
                    assert_eq!(
                        step(&mut cpu, &mut ram, PrivilegeLevel::Ring3),
                        Ok(StepOutcome::Advanced),
                        "{label}"
                    );
                    assert_eq!(cpu, expected, "{label}");
                    assert!(ram.reads.is_empty() && ram.writes.is_empty(), "{label}");
                }
            }
        }
    }
}

#[test]
fn movmsk_takes_the_sign_of_each_element_not_of_neighbouring_bits() {
    // MOVMSKPS: DEST[i] := SRC[32*i+31]; MOVMSKPD: DEST[i] := SRC[64*i+63]. Dword signs 1,0,1,1 -> 0b1101; qword signs 0,1 -> 0b10.
    let mut cpu = cpu();
    let mut value = [0u8; 16];
    value[3] = 0x80;
    value[11] = 0x80;
    value[15] = 0xc0;
    value[7] = 0x7f;
    cpu.sse.as_mut().unwrap().xmm[5] = value;
    let mut ram = Ram::new(&movmsk_code(false, 0, 5, 0));
    assert_eq!(
        step(&mut cpu, &mut ram, PrivilegeLevel::Ring0),
        Ok(StepOutcome::Advanced)
    );
    assert_eq!(cpu.gpr[Register::Rax as usize], 0b1101);
    let mut ram = Ram::new(&movmsk_code(true, 1, 5, 0));
    cpu.rip = CODE;
    assert_eq!(
        step(&mut cpu, &mut ram, PrivilegeLevel::Ring0),
        Ok(StepOutcome::Advanced)
    );
    assert_eq!(cpu.gpr[Register::Rcx as usize], 0b10);
}

#[test]
fn movmsk_gates_lock_memory_form_and_refusals() {
    for double in [false, true] {
        for (cr0, cr4, vector) in gate_cases() {
            let code = movmsk_code(double, 2, 3, 0);
            let (result, after) = run_fp_init(
                fp_init_cpu(cr0, cr4),
                &mut Ram::new(&code),
                PrivilegeLevel::Ring0,
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
                        "{double} {cr0:#x} {cr4:#x}"
                    );
                    assert_eq!(after, fp_init_cpu(cr0, cr4));
                }
                None => {
                    assert_eq!(result.exit, RunExit::InstructionBudgetExhausted);
                    assert_eq!(after.gpr[Register::Rdx as usize], 0);
                }
            }
        }
        // LOCK, and a memory operand (SDM: ModRM.MOD = 11B required) are #UD.
        let pf: &[u8] = if double { &[0x66] } else { &[] };
        for code in [
            [&[0xf0][..], pf, &[0x0f, 0x50, 0xc1]].concat(),
            [pf, &[0xf0, 0x0f, 0x50, 0xc1]].concat(),
            [pf, &[0x0f, 0x50, 0x00]].concat(),
            [pf, &[0x0f, 0x50, 0x45, 0x10]].concat(),
            [pf, &[0x0f, 0x50, 0x05, 0, 0, 0, 0]].concat(),
            [pf, &[0x48, 0x0f, 0x50, 0x80, 0, 0, 0, 0]].concat(),
        ] {
            let mut cpu = cpu();
            let before = cpu.clone();
            let mut ram = Ram::new(&code);
            assert_eq!(
                step(&mut cpu, &mut ram, PrivilegeLevel::Ring0),
                Ok(fault(6, None)),
                "{code:02x?}"
            );
            assert_eq!(cpu, before);
            assert!(ram.reads.is_empty());
        }
    }
}

// ---- decoder ----

#[test]
fn the_decoder_reports_kind_registers_source_immediate_and_length() {
    let spec = |name: &str| SPECS.iter().find(|spec| spec.name == name).unwrap();
    // A form that the narrow register profiles do not claim: REX.W.
    let code = register_code(spec("shufps"), 9, 3, 0x6c, 0x08);
    let instruction =
        long_decode::decode(&mut Ram::new(&code), CODE, AccessContext::default()).unwrap();
    assert_eq!(usize::from(instruction.length), code.len());
    assert!(matches!(
        instruction.operation,
        Operation::SseLogicPermute {
            kind: SseLogicPermuteKind::Shufps,
            destination: 9,
            source: Sse3Source::Register(3),
            immediate: 0x6c,
        }
    ));
    // Memory forms of the whole group (the narrow decoders claim none of them).
    for spec in &SPECS {
        let code = memory_code(spec, 10, &MEMS[10], 0x39);
        let instruction =
            long_decode::decode(&mut Ram::new(&code), CODE, AccessContext::default()).unwrap();
        assert_eq!(usize::from(instruction.length), code.len(), "{}", spec.name);
        let Operation::SseLogicPermute {
            kind,
            destination,
            source,
            immediate,
        } = instruction.operation
        else {
            panic!("{}: {:?}", spec.name, instruction.operation);
        };
        assert_eq!(destination, 10, "{}", spec.name);
        assert!(matches!(source, Sse3Source::Memory(_)), "{}", spec.name);
        assert_eq!(
            immediate,
            if spec.immediate { 0x39 } else { 0 },
            "{}",
            spec.name
        );
        assert_eq!(kind.has_immediate(), spec.immediate);
        let double = !spec.prefix.is_empty();
        let expected = match spec.opcode {
            0x54 => SseLogicPermuteKind::And,
            0x55 => SseLogicPermuteKind::Andn,
            0x56 => SseLogicPermuteKind::Or,
            0x57 => SseLogicPermuteKind::Xor,
            0xc6 if double => SseLogicPermuteKind::Shufpd,
            0xc6 => SseLogicPermuteKind::Shufps,
            0x14 if double => SseLogicPermuteKind::Unpcklpd,
            0x14 => SseLogicPermuteKind::Unpcklps,
            0x15 if double => SseLogicPermuteKind::Unpckhpd,
            _ => SseLogicPermuteKind::Unpckhps,
        };
        assert_eq!(kind, expected, "{}", spec.name);
    }
    for (double, code) in [
        (false, vec![0x41, 0x0f, 0x50, 0xc2]),
        (true, vec![0x66, 0x44, 0x0f, 0x50, 0xfb]),
        (true, vec![0x66, 0x48, 0x0f, 0x50, 0xc1]),
    ] {
        let instruction =
            long_decode::decode(&mut Ram::new(&code), CODE, AccessContext::default()).unwrap();
        assert_eq!(usize::from(instruction.length), code.len());
        let Operation::SseMovmsk { double: found, .. } = instruction.operation else {
            panic!("{code:02x?}: {:?}", instruction.operation);
        };
        assert_eq!(found, double);
    }
    assert!(matches!(
        operation_of(&[0x66, 0x44, 0x0f, 0x50, 0xfb]),
        Ok(Operation::SseMovmsk {
            double: true,
            destination: 15,
            source: 3
        })
    ));
    assert!(matches!(
        operation_of(&[0x41, 0x0f, 0x50, 0xc2]),
        Ok(Operation::SseMovmsk {
            double: false,
            destination: 0,
            source: 10
        })
    ));
}

#[test]
fn a_truncated_instruction_is_not_executed() {
    for spec in &SPECS {
        let code = memory_code(spec, 1, &MEMS[1], 0x11);
        for cut in 1..code.len() {
            let mut cpu = cpu();
            let mut ram = Ram::new(&code[..cut]);
            ram.bytes.truncate(CODE as usize + cut);
            let before = cpu.clone();
            let outcome = step(&mut cpu, &mut ram, PrivilegeLevel::Ring0);
            assert!(
                !matches!(outcome, Ok(StepOutcome::Advanced)),
                "{} cut {cut}: {outcome:?}",
                spec.name
            );
            assert_eq!(cpu, before, "{} cut {cut}", spec.name);
        }
    }
}
