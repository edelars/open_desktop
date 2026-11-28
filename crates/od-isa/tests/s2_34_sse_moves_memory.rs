//! S2-34 (m128 addressing): the memory forms of the six raw 128-bit SSE moves with any ModR/M, SIB,
//! displacement or RIP-relative operand.
//!
//! Every expectation comes from the Intel SDM text read for this task (combined SDM 325462-093US,
//! Vol. 2B): MOVAPD pp. 4-37..4-40 (66 0F 28 /r, 66 0F 29 /r; Exceptions Type 1.SSE2), MOVAPS pp. 4-41..4-44
//! (NP 0F 28 /r, NP 0F 29 /r; Type 1.SSE), MOVDQA pp. 4-59..4-63 (66 0F 6F /r, 66 0F 7F /r; Type 1.SSE2),
//! MOVDQU pp. 4-64..4-71 (F3 0F 6F /r, F3 0F 7F /r; Type 4), MOVUPD pp. 4-122..4-125 (66 0F 10 /r,
//! 66 0F 11 /r; Type 4) and MOVUPS pp. 4-126..4-129 (NP 0F 10 /r, NP 0F 11 /r; Type 4), plus Vol. 2A
//! Table 2-18 (Type 1, p. 2-26) and Table 2-21 (Type 4, p. 2-29):
//!
//! * the legacy-SSE operation is DEST[127:0] := SRC[127:0] (a raw copy: no MXCSR read or write, no flag
//!   change, no SIMD floating-point exception);
//! * #UD for CR0.EM = 1, CR4.OSFXSR = 0 or a LOCK prefix; #NM for CR0.TS = 1 (EM / OSFXSR take priority);
//! * #SS(0) for a non-canonical address through SS, #GP(0) for any other non-canonical address;
//! * MOVAPS / MOVAPD / MOVDQA: #GP(0) when the memory operand is not 16-byte aligned; MOVUPS / MOVUPD /
//!   MOVDQU have no alignment requirement (Table 2-21 note 1); #PF is the memory backend's.
//!
//! Not asserted here because the SDM text does not define it: #AC (the SDM only says that "treatment of
//! #AC varies" for MOVUPS/MOVUPD) and the CPUID.SSE/SSE2 gates (S2-57).

use od_isa::execute::{self, Cr4Profile, RunConfig, RunExit, StepOutcome};
use od_isa::long_decode::{self, AddressBase, MemoryOperand, Operation, Sse128MoveKind};
use od_isa::state::{SseState, X87State};
use od_isa::{
    AccessContext, AccessKind, ArchitecturalFault, CpuActivity, GuestMemory, LongModeState,
    MemoryError, OperandSize, PrivilegeLevel, Register, StiShadow,
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

struct Kind {
    name: &'static str,
    kind: Sse128MoveKind,
    /// The mandatory prefix (none, 66 or F3).
    prefix: &'static [u8],
    load: u8,
    store: u8,
    aligned: bool,
}

const KINDS: [Kind; 6] = [
    Kind {
        name: "MOVAPS",
        kind: Sse128MoveKind::Movaps,
        prefix: &[],
        load: 0x28,
        store: 0x29,
        aligned: true,
    },
    Kind {
        name: "MOVUPS",
        kind: Sse128MoveKind::Movups,
        prefix: &[],
        load: 0x10,
        store: 0x11,
        aligned: false,
    },
    Kind {
        name: "MOVAPD",
        kind: Sse128MoveKind::Movapd,
        prefix: &[0x66],
        load: 0x28,
        store: 0x29,
        aligned: true,
    },
    Kind {
        name: "MOVUPD",
        kind: Sse128MoveKind::Movupd,
        prefix: &[0x66],
        load: 0x10,
        store: 0x11,
        aligned: false,
    },
    Kind {
        name: "MOVDQA",
        kind: Sse128MoveKind::Movdqa,
        prefix: &[0x66],
        load: 0x6f,
        store: 0x7f,
        aligned: true,
    },
    Kind {
        name: "MOVDQU",
        kind: Sse128MoveKind::Movdqu,
        prefix: &[0xf3],
        load: 0x6f,
        store: 0x7f,
        aligned: false,
    },
];

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

/// Encodes one instruction. The legacy prefixes (extra, then the mandatory prefix) come first and
/// the REX byte last, immediately before 0F, as the architecture requires for it to take effect.
fn encode(form: &Form, kind: &Kind, opcode: u8, xmm: u8, forced_rex: bool) -> Vec<u8> {
    let mut code = form.extra.to_vec();
    code.extend_from_slice(kind.prefix);
    let rex = form.rex | if xmm >= 8 { 0x04 } else { 0 };
    if rex != 0 || forced_rex {
        code.push(0x40 | rex);
    }
    code.extend_from_slice(&[0x0f, opcode, (form.mode << 6) | ((xmm & 7) << 3) | form.rm]);
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

// ---- every addressing form, both directions, all six instructions ----

#[test]
fn every_addressing_form_loads_and_stores_sixteen_raw_bytes_for_all_six_moves() {
    let mut cases = 0;
    for (kind_index, kind) in KINDS.iter().enumerate() {
        for (form_index, form) in FORMS.iter().enumerate() {
            for store in [false, true] {
                // Every XMM register is exercised over the matrix (REX.R selects XMM8..15).
                let xmm = ((kind_index * 5 + form_index * 3 + usize::from(store)) % 16) as u8;
                let opcode = if store { kind.store } else { kind.load };
                let code = encode(form, kind, opcode, xmm, false);
                let seed = (form_index * 16 + kind_index) as u8;
                let mut cpu = setup(form);
                let mut ram = Ram::new(&code);
                let address = form.address as usize;
                ram.bytes[address..address + 16].copy_from_slice(&memory_pattern(seed));
                let (cpu_before, ram_before) = (cpu.clone(), ram.bytes.clone());
                let outcome = step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring0);
                let label = format!(
                    "{} {} {} xmm{xmm}",
                    kind.name,
                    form.name,
                    if store { "store" } else { "load" }
                );
                // Forms that a narrower profile already decodes keep their own operation; every
                // other form is the general one and must report the right kind, direction and register.
                let decoded =
                    long_decode::decode(&mut Ram::new(&code), CODE, AccessContext::default())
                        .unwrap();
                assert_eq!(decoded.length as usize, code.len(), "{label}");
                if let Operation::Sse128MemoryMove {
                    kind: k,
                    store: s,
                    xmm: x,
                    ..
                } = decoded.operation
                {
                    assert_eq!((k, s, x), (kind.kind, store, xmm), "{label}");
                }
                assert_eq!(outcome, Ok(StepOutcome::Advanced), "{label}");
                let mut expected_cpu = cpu_before;
                expected_cpu.rip = CODE + code.len() as u64;
                let mut expected_ram = ram_before;
                if store {
                    expected_ram[address..address + 16]
                        .copy_from_slice(&xmm_pattern(usize::from(xmm)));
                    assert_eq!(ram.reads, [], "{label}");
                    assert_eq!(ram.writes, [(form.address, 16)], "{label}");
                } else {
                    expected_cpu.sse.as_mut().unwrap().xmm[usize::from(xmm)] = memory_pattern(seed);
                    assert_eq!(ram.reads, [(form.address, 16)], "{label}");
                    assert_eq!(ram.writes, [], "{label}");
                }
                // Registers, flags, MXCSR, the other XMM registers and the neighbouring bytes are
                // untouched: whole-state equality.
                assert_eq!(cpu, expected_cpu, "{label}");
                assert_eq!(ram.bytes, expected_ram, "{label}");
                cases += 1;
            }
        }
    }
    assert_eq!(cases, 6 * FORMS.len() * 2);
}

#[test]
fn the_decoder_reports_kind_direction_register_and_operand() {
    // MOVDQU [rbp-0x20], xmm9: F3 44 0F 7F 4D E0
    let mut ram = Ram::new(&[0xf3, 0x44, 0x0f, 0x7f, 0x4d, 0xe0]);
    let instruction = long_decode::decode(&mut ram, CODE, AccessContext::default()).unwrap();
    assert_eq!(instruction.length, 6);
    assert_eq!(
        instruction.operation,
        Operation::Sse128MemoryMove {
            kind: Sse128MoveKind::Movdqu,
            store: true,
            xmm: 9,
            memory: MemoryOperand {
                segment: od_isa::long_decode::MemorySegment::Flat,
                base: AddressBase::Register(Register::Rbp),
                index: None,
                scale: 1,
                displacement: -0x20,
                address_size: OperandSize::Qword,
                size: OperandSize::Qword,
            },
        }
    );
    // MOVAPD xmm2, [rsi+rdi*2+0x1000]: 66 0F 28 94 7E 00 10 00 00
    let mut ram = Ram::new(&[0x66, 0x0f, 0x28, 0x94, 0x7e, 0x00, 0x10, 0x00, 0x00]);
    let instruction = long_decode::decode(&mut ram, CODE, AccessContext::default()).unwrap();
    assert_eq!(instruction.length, 9);
    assert_eq!(
        instruction.operation,
        Operation::Sse128MemoryMove {
            kind: Sse128MoveKind::Movapd,
            store: false,
            xmm: 2,
            memory: MemoryOperand {
                segment: od_isa::long_decode::MemorySegment::Flat,
                base: AddressBase::Register(Register::Rsi),
                index: Some(Register::Rdi),
                scale: 2,
                displacement: 0x1000,
                address_size: OperandSize::Qword,
                size: OperandSize::Qword,
            },
        }
    );
    // 67 prefix: a 32-bit effective address.
    let mut ram = Ram::new(&[0x67, 0x0f, 0x11, 0x00]);
    let instruction = long_decode::decode(&mut ram, CODE, AccessContext::default()).unwrap();
    let Operation::Sse128MemoryMove { memory, .. } = instruction.operation else {
        panic!("{:?}", instruction.operation);
    };
    assert_eq!(memory.address_size, OperandSize::Dword);
    assert_eq!(instruction.address_size, OperandSize::Dword);
}

#[test]
fn a_rex_before_a_legacy_prefix_is_dropped_like_any_other_ineffective_rex() {
    // 44 66 0F 28 00: REX.R precedes the 66 prefix, so it does not apply: destination XMM0, not XMM8.
    for (code, kind) in [
        (&[0x44, 0x66, 0x0f, 0x28, 0x00][..], "MOVAPD"),
        (&[0x44, 0xf3, 0x0f, 0x6f, 0x00][..], "MOVDQU"),
        (&[0x44, 0x2e, 0x0f, 0x10, 0x00][..], "MOVUPS"),
        (
            &[0x41, 0x66, 0x0f, 0x10, 0x00][..],
            "MOVUPD, REX.B dropped: base RAX",
        ),
    ] {
        let mut cpu = cpu();
        cpu.gpr[Register::Rax as usize] = 0x8c00;
        cpu.gpr[Register::R8 as usize] = 0x8d00;
        let mut ram = Ram::new(code);
        ram.bytes[0x8c00..0x8c10].copy_from_slice(&memory_pattern(0x21));
        let mut expected = cpu.clone();
        expected.rip = CODE + code.len() as u64;
        expected.sse.as_mut().unwrap().xmm[0] = memory_pattern(0x21);
        assert_eq!(
            step(code, &mut cpu, &mut ram, PrivilegeLevel::Ring0),
            Ok(StepOutcome::Advanced),
            "{kind}"
        );
        assert_eq!(cpu, expected, "{kind}");
    }
}

// ---- alignment ----

#[test]
fn aligned_forms_fault_on_any_misalignment_and_unaligned_forms_never_do() {
    for kind in &KINDS {
        for store in [false, true] {
            for low in 0..16u64 {
                let address = 0x8000 + low;
                let opcode = if store { kind.store } else { kind.load };
                let code = [kind.prefix, &[0x0f, opcode, 0x00]].concat(); // [rax]
                let mut cpu = cpu();
                cpu.gpr[Register::Rax as usize] = address;
                let mut ram = Ram::new(&code);
                ram.bytes[address as usize..address as usize + 16]
                    .copy_from_slice(&memory_pattern(0x44));
                let (cpu_before, ram_before) = (cpu.clone(), ram.bytes.clone());
                let outcome = step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring0);
                let label = format!(
                    "{} {} {address:#x}",
                    kind.name,
                    if store { "store" } else { "load" }
                );
                if kind.aligned && low != 0 {
                    // #GP(0) before any memory access, nothing retires.
                    assert_eq!(outcome, Ok(gp0()), "{label}");
                    assert_eq!(cpu, cpu_before, "{label}");
                    assert_eq!(ram.bytes, ram_before, "{label}");
                    assert!(ram.reads.is_empty() && ram.writes.is_empty(), "{label}");
                } else {
                    assert_eq!(outcome, Ok(StepOutcome::Advanced), "{label}");
                    if store {
                        assert_eq!(
                            &ram.bytes[address as usize..address as usize + 16],
                            &xmm_pattern(0),
                            "{label}"
                        );
                    } else {
                        assert_eq!(
                            cpu.sse.as_ref().unwrap().xmm[0],
                            memory_pattern(0x44),
                            "{label}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn an_unaligned_access_across_a_page_boundary_is_one_sixteen_byte_transfer() {
    for kind in KINDS.iter().filter(|kind| !kind.aligned) {
        for (store, address) in [(false, 0x8ff9u64), (true, 0x8ffc)] {
            let opcode = if store { kind.store } else { kind.load };
            let code = [kind.prefix, &[0x0f, opcode, 0x00]].concat();
            let mut cpu = cpu();
            cpu.gpr[Register::Rax as usize] = address;
            let mut ram = Ram::new(&code);
            ram.bytes[address as usize..address as usize + 16]
                .copy_from_slice(&memory_pattern(0x66));
            assert_eq!(
                step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring0),
                Ok(StepOutcome::Advanced)
            );
            assert_eq!(ram.reads.len() + ram.writes.len(), 1, "{}", kind.name);
        }
    }
}

// ---- addresses: canonical form, #SS versus #GP, backend faults ----

#[test]
fn non_canonical_addresses_fault_before_alignment_and_before_memory() {
    // The first four are 16-byte aligned; the last four are the same shapes made misaligned, so for
    // MOVAPS, MOVAPD and MOVDQA the non-canonical fault (#SS for an RSP or RBP base, #GP otherwise)
    // must win over the alignment #GP(0). The order comes from the SDM listing both conditions and
    // from the model; the physical oracle has no case with a misaligned non-canonical start.
    let non_canonical = [
        0x0000_8000_0000_0000u64,
        0xffff_7fff_ffff_fff0,
        0x1234_5678_1234_5670,
        0x0100_0000_0000_0000,
        0x0000_8000_0000_0001,
        0xffff_7fff_ffff_fff8,
        0x1234_5678_1234_5671,
        0x0100_0000_0000_0008,
    ];
    for kind in &KINDS {
        for store in [false, true] {
            let opcode = if store { kind.store } else { kind.load };
            for address in non_canonical {
                // [rax]: #GP(0); [rsp] and [rbp+0]: #SS(0).
                for (modrm_tail, register, expected) in [
                    (&[0x00][..], Register::Rax, gp0()),
                    (&[0x04, 0x24][..], Register::Rsp, ss0()),
                    (&[0x45, 0x00][..], Register::Rbp, ss0()),
                ] {
                    let mut code = kind.prefix.to_vec();
                    code.extend_from_slice(&[0x0f, opcode]);
                    code.extend_from_slice(modrm_tail);
                    let mut cpu = cpu();
                    cpu.gpr[register as usize] = address;
                    let mut ram = Ram::new(&code);
                    let (before, ram_before) = (cpu.clone(), ram.bytes.clone());
                    let label = format!("{} {address:#x} {register:?}", kind.name);
                    assert_eq!(
                        step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring0),
                        Ok(expected),
                        "{label}"
                    );
                    assert_eq!(cpu, before, "{label}");
                    assert_eq!(ram.bytes, ram_before, "{label}");
                    assert!(ram.reads.is_empty() && ram.writes.is_empty(), "{label}");
                }
            }
        }
    }
}

#[test]
fn a_range_that_leaves_the_canonical_space_faults_without_a_partial_access() {
    // Unaligned forms only (the aligned ones fault on the alignment first): the last byte of the
    // 16-byte range is non-canonical although the start is canonical.
    for kind in KINDS.iter().filter(|kind| !kind.aligned) {
        for store in [false, true] {
            let opcode = if store { kind.store } else { kind.load };
            let code = [kind.prefix, &[0x0f, opcode, 0x00]].concat();
            let mut cpu = cpu();
            cpu.gpr[Register::Rax as usize] = 0x0000_7fff_ffff_fff8;
            let mut ram = Ram::new(&code);
            let before = cpu.clone();
            assert_eq!(
                step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring0),
                Ok(gp0()),
                "{}",
                kind.name
            );
            assert_eq!(cpu, before);
            assert!(ram.reads.is_empty() && ram.writes.is_empty());
        }
    }
}

#[test]
fn a_range_that_leaves_the_canonical_space_through_a_stack_base_is_ss_after_the_alignment_check() {
    // Oracle cases edge_*_{rsp,rbp}_canon_top_cross: the start 0x7ffffffffff8 is canonical but
    // misaligned and the last byte of the 16-byte range is not. Aligned forms stop on the
    // alignment #GP(0) first (hardware: SIGSEGV, #GP); unaligned forms fault on the range, which
    // for an RSP or RBP base is #SS(0) (hardware: SIGBUS).
    for kind in &KINDS {
        for store in [false, true] {
            let opcode = if store { kind.store } else { kind.load };
            for (modrm_tail, register) in [
                (&[0x04, 0x24][..], Register::Rsp),
                (&[0x45, 0x00][..], Register::Rbp),
            ] {
                let mut code = kind.prefix.to_vec();
                code.extend_from_slice(&[0x0f, opcode]);
                code.extend_from_slice(modrm_tail);
                let mut cpu = cpu();
                cpu.gpr[register as usize] = 0x0000_7fff_ffff_fff8;
                let mut ram = Ram::new(&code);
                let before = cpu.clone();
                let expected = if kind.aligned { gp0() } else { ss0() };
                let label = format!("{} {register:?}", kind.name);
                assert_eq!(
                    step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring0),
                    Ok(expected),
                    "{label}"
                );
                assert_eq!(cpu, before, "{label}");
                assert!(ram.reads.is_empty() && ram.writes.is_empty(), "{label}");
            }
        }
    }
}

#[test]
fn backend_faults_leave_cpu_and_memory_untouched() {
    for kind in &KINDS {
        for store in [false, true] {
            let opcode = if store { kind.store } else { kind.load };
            let code = [kind.prefix, &[0x0f, opcode, 0x00]].concat();
            let label = format!("{} {}", kind.name, if store { "store" } else { "load" });

            // An unmapped aligned operand: the backend's error is the result (not a guest fault).
            let mut cpu1 = cpu();
            cpu1.gpr[Register::Rax as usize] = 0x1_0000;
            let mut ram = Ram::new(&code);
            let (before, ram_before) = (cpu1.clone(), ram.bytes.clone());
            let access = if store {
                AccessKind::Write
            } else {
                AccessKind::Read
            };
            assert_eq!(
                step(&code, &mut cpu1, &mut ram, PrivilegeLevel::Ring0),
                Err(MemoryError::Unmapped {
                    address: 0x1_0000,
                    access
                }),
                "{label}"
            );
            assert_eq!(cpu1, before, "{label}");
            assert_eq!(ram.bytes, ram_before, "{label}");

            // A #PF raised by the backend for the second page of a page-crossing access (unaligned
            // forms) or for the operand's own page (aligned forms): a guest fault, no commit.
            let address = if kind.aligned { 0x9000 } else { 0x8ff8 };
            let mut cpu2 = cpu();
            cpu2.gpr[Register::Rax as usize] = address;
            let mut ram = Ram::new(&code);
            ram.fault_page = Some(0x9000);
            let (before, ram_before) = (cpu2.clone(), ram.bytes.clone());
            let expected_fault = StepOutcome::Fault(ArchitecturalFault {
                vector: 14,
                error_code: Some(if store { 2 } else { 0 }),
                linear_address: Some(0x9000),
            });
            assert_eq!(
                step(&code, &mut cpu2, &mut ram, PrivilegeLevel::Ring0),
                Ok(expected_fault),
                "{label}"
            );
            assert_eq!(cpu2, before, "{label}");
            assert_eq!(ram.bytes, ram_before, "{label}");
        }
    }
}

// ---- LOCK, CR0 / CR4 gates, missing state ----

#[test]
fn lock_before_a_memory_form_is_ud() {
    for kind in KINDS.iter().filter(|kind| kind.prefix != [0xf3]) {
        for store in [false, true] {
            let opcode = if store { kind.store } else { kind.load };
            for code in [
                [&[0xf0][..], kind.prefix, &[0x0f, opcode, 0x00]].concat(),
                [kind.prefix, &[0xf0, 0x0f, opcode, 0x00]].concat(),
                [&[0xf0][..], kind.prefix, &[0x48, 0x0f, opcode, 0x10, 0x20]].concat(),
                [&[0xf0][..], kind.prefix, &[0x0f, opcode, 0x05, 0, 0, 0, 0]].concat(),
            ] {
                let mut cpu = cpu();
                let mut ram = Ram::new(&code);
                let (before, ram_before) = (cpu.clone(), ram.bytes.clone());
                assert_eq!(
                    step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring0),
                    Ok(ud()),
                    "{} {code:02x?}",
                    kind.name
                );
                assert_eq!(cpu, before);
                assert_eq!(ram.bytes, ram_before);
                assert!(ram.reads.is_empty() && ram.writes.is_empty());
            }
        }
    }
}

#[test]
fn cr0_em_ts_and_cr4_osfxsr_gate_the_moves_before_any_address_or_memory_check() {
    // CR0 and CR4 values are modeled by the FP-init profile pair (CR0 = BASE plus EM / TS bits,
    // CR4 = PAE plus OSFXSR / OSXMMEXCPT bits), run with `run_with_cr4_profile`.
    const CR0_BASE: u64 = 0x8001_0033; // PG | WP | NE | ET | MP | PE: EM = 0, TS = 0
    const CR4_PAE: u64 = 0x20;
    const CR4_OSFXSR: u64 = 0x200;
    // (CR0, CR4, vector of the expected fault; None = the move runs)
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
        for store in [false, true] {
            let opcode = if store { kind.store } else { kind.load };
            let code = [kind.prefix, &[0x0f, opcode, 0x00]].concat();
            for (cr0, cr4, vector) in cases {
                for address in [0x8000u64, 0x0000_8000_0000_0000] {
                    let mut cpu = fp_init_cpu(cr0, cr4);
                    cpu.gpr[Register::Rax as usize] = address;
                    let mut ram = Ram::new(&code);
                    let (before, ram_before) = (cpu.clone(), ram.bytes.clone());
                    let result = execute::run_with_cr4_profile(
                        &mut cpu,
                        &mut ram,
                        RunConfig {
                            instruction_budget: 1,
                            stop_rip: None,
                            context: ctx(PrivilegeLevel::Ring0),
                        },
                        Cr4Profile::FpInitLong64V1,
                        || false,
                    );
                    let label = format!("{} cr0={cr0:#x} cr4={cr4:#x} {address:#x}", kind.name);
                    let fault = |vector, error_code| {
                        RunExit::Fault(ArchitecturalFault {
                            vector,
                            error_code,
                            linear_address: None,
                        })
                    };
                    match (vector, address == 0x8000) {
                        // The EM / OSFXSR / TS gate precedes the non-canonical #GP too.
                        (Some(vector), _) => {
                            assert_eq!(result.exit, fault(vector, None), "{label}");
                            assert_eq!(result.instructions_executed, 0, "{label}");
                            assert_eq!(cpu, before, "{label}");
                            assert_eq!(ram.bytes, ram_before, "{label}");
                            assert!(ram.reads.is_empty() && ram.writes.is_empty(), "{label}");
                        }
                        (None, true) => {
                            assert_eq!(result.exit, RunExit::InstructionBudgetExhausted, "{label}");
                            assert_eq!(result.instructions_executed, 1, "{label}");
                        }
                        (None, false) => {
                            assert_eq!(result.exit, fault(13, Some(0)), "{label}");
                            assert_eq!(result.instructions_executed, 0, "{label}");
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn without_x87_and_sse_state_the_forms_stop_as_unsupported_without_change() {
    for kind in &KINDS {
        for (x87, sse) in [(true, false), (false, true), (false, false)] {
            let code = [kind.prefix, &[0x0f, kind.load, 0x00]].concat();
            let mut cpu = cpu();
            if !x87 {
                cpu.x87 = None;
            }
            if !sse {
                cpu.sse = None;
            }
            let mut ram = Ram::new(&code);
            let before = cpu.clone();
            assert_eq!(
                step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring0),
                Ok(StepOutcome::Unsupported { opcode: kind.load }),
                "{} x87={x87} sse={sse}",
                kind.name
            );
            assert_eq!(cpu, before);
            assert!(ram.reads.is_empty() && ram.writes.is_empty());
        }
    }
}

// ---- encodings that stay outside ----

#[test]
fn neighbouring_encodings_stay_unsupported_without_change() {
    let refused: &[(&[u8], &str)] = &[
        (&[0xf2, 0x0f, 0x28, 0x00], "F2 0F 28 (no such instruction)"),
        (&[0xf3, 0x0f, 0x28, 0x00], "F3 0F 28"),
        (&[0xf3, 0x0f, 0x29, 0x00], "F3 0F 29"),
        (&[0xf2, 0x0f, 0x7f, 0x00], "F2 0F 7F"),
        (&[0xf2, 0x0f, 0x6f, 0x00], "F2 0F 6F"),
        (&[0x66, 0xf3, 0x0f, 0x6f, 0x00], "66 and F3 together"),
        (&[0xf3, 0x66, 0x0f, 0x7f, 0x00], "F3 and 66 together"),
        (
            &[0x64, 0x0f, 0x28, 0x00],
            "FS override: no segment base in this operand model",
        ),
        (&[0x65, 0x66, 0x0f, 0x29, 0x00], "GS override"),
        (&[0x64, 0xf3, 0x0f, 0x7f, 0x00], "FS override MOVDQU"),
        (&[0x65, 0x0f, 0x11, 0x00], "GS override MOVUPS store"),
        (&[0x66, 0x66, 0x0f, 0x28, 0x00], "duplicate 66"),
        (&[0x67, 0x67, 0x0f, 0x28, 0x00], "duplicate 67"),
        (&[0x2e, 0x3e, 0x0f, 0x28, 0x00], "two segment overrides"),
        (&[0x48, 0x48, 0x0f, 0x28, 0x00], "duplicate REX"),
        // The register forms of MOVDQU (F3 0F 6F/7F) and the MOVDQA store (66 0F 7F) are executed since S2-39
        // (integer SSE2 remainder, `s2_39_int_sse2`).
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

// ---- #AC: none for the 16-byte unaligned forms ----

/// ADR-0023 decision 4 (slice C1): the 16-byte unaligned moves (MOVUPS, MOVUPD, MOVDQU) have no #AC
/// (Vol. 2A §2.5, exception Type 4: "#AC(0) For 2, 4, or 8 byte memory access if alignment checking
/// is enabled"); at CPL3 with RFLAGS.AC = 1 a misaligned operand executes (it was `Unsupported`
/// while #AC was not modeled).
#[test]
fn an_unaligned_unaligned_form_at_cpl3_with_the_alignment_check_armed_runs_without_ac() {
    const AC: u64 = 1 << 18;
    for kind in KINDS.iter().filter(|kind| !kind.aligned) {
        let code = [kind.prefix, &[0x0f, kind.load, 0x00]].concat();
        let run = |rflags: u64, cr0: Option<u64>, address: u64, privilege| {
            let mut cpu = cpu();
            cpu.rflags = rflags;
            cpu.cr0 = cr0;
            cpu.gpr[Register::Rax as usize] = address;
            let mut ram = Ram::new(&code);
            let before = cpu.clone();
            let outcome = step(&code, &mut cpu, &mut ram, privilege);
            (outcome, cpu == before)
        };
        // AC = 1, CR0.AM not carried (counts as 1), CPL3, unaligned: no #AC for 16 bytes.
        let (outcome, _) = run(FLAGS | AC, None, 0x8001, PrivilegeLevel::Ring3);
        assert_eq!(outcome, Ok(StepOutcome::Advanced), "{}", kind.name);
        // Aligned, AC clear, or CPL0: the access runs.
        for (rflags, address, privilege) in [
            (FLAGS | AC, 0x8000, PrivilegeLevel::Ring3),
            (FLAGS, 0x8001, PrivilegeLevel::Ring3),
            (FLAGS | AC, 0x8001, PrivilegeLevel::Ring0),
        ] {
            let (outcome, _) = run(rflags, None, address, privilege);
            assert_eq!(
                outcome,
                Ok(StepOutcome::Advanced),
                "{} {rflags:#x} {address:#x}",
                kind.name
            );
        }
        // CR0 carried by the FP-init profile never has AM set (the profile accepts only EM / TS
        // beyond BASE), so the access is not alignment-checked there and runs.
        let mut cpu = fp_init_cpu(0x8001_0033, 0x220);
        cpu.rflags = FLAGS | AC;
        cpu.gpr[Register::Rax as usize] = 0x8001;
        let mut ram = Ram::new(&code);
        let result = execute::run_with_cr4_profile(
            &mut cpu,
            &mut ram,
            RunConfig {
                instruction_budget: 1,
                stop_rip: None,
                context: ctx(PrivilegeLevel::Ring3),
            },
            Cr4Profile::FpInitLong64V1,
            || false,
        );
        assert_eq!(
            result.exit,
            RunExit::InstructionBudgetExhausted,
            "{}",
            kind.name
        );
    }
    // The aligned forms fault with #GP(0) for a misaligned operand even with AC armed.
    for kind in KINDS.iter().filter(|kind| kind.aligned) {
        let code = [kind.prefix, &[0x0f, kind.load, 0x00]].concat();
        let mut cpu = cpu();
        cpu.rflags = FLAGS | AC;
        cpu.gpr[Register::Rax as usize] = 0x8008;
        let mut ram = Ram::new(&code);
        assert_eq!(
            step(&code, &mut cpu, &mut ram, PrivilegeLevel::Ring3),
            Ok(gp0()),
            "{}",
            kind.name
        );
    }
}
