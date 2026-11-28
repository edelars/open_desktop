//! S2-39 (rest, CPL0 set): the non-temporal stores MOVNTDQ (66 0F E7), MOVNTPS (0F 2B) and MOVNTPD (66 0F 2B), MASKMOVDQU
//! (66 0F F7), MOVDQ2Q (F2 0F D6) and MOVQ2DQ (F3 0F D6) on the local SSE and x87 state of the FP member of the CPL0 set
//! and in the bare run. CR0.EM/TS and CR4.OSFXSR gate all of them (#UD, #NM) before any operand access; the stores need a
//! 16-byte aligned operand (#GP(0)), MASKMOVDQU an operand of its implicit [rdi] in any alignment, MOVDQ2Q/MOVQ2DQ the x87
//! to MMX transition and the pending x87 exception check (#MF).
//!
//! Every expected value is computed in this file from the Intel SDM definitions (Vol. 2B pp. 4-1 MASKMOVDQU, 4-58 MOVDQ2Q,
//! 4-88 MOVNTDQ, 4-94 MOVNTPD, 4-96 MOVNTPS, 4-102 MOVQ2DQ; Vol. 2A Tables 2-18/2-21 exception types; Vol. 1 sections 9.2.2
//! and 9.6.2 for the MMX register write), or from the observation of the physical oracle recorded in the S2-39 (rest)
//! report (the partial-mask fault rule, the 8-byte alignment check of MASKMOVDQU). No value is model output.

use od_isa::execute::{
    self, ControlRegisterPolicy, Cpl0ProfileSet, Cpl0ProfileSetExit, GuestFpState, RunConfig,
    RunExit, StepOutcome, run_with_cpl0_profile_set,
};
use od_isa::state::{SseState, X87State};
use od_isa::{
    AccessContext, AccessKind, AccessOrigin, ArchitecturalFault, CpuActivity, GuestMemory,
    InstructionMode, LongModeState, MemoryError, PrivilegeLevel, StiShadow, cpuid::CpuidProfile,
};
use std::cell::RefCell;

const CODE: u64 = 0x1000;
const DATA: u64 = 0x2000;
const RAM: usize = 0x4000;
const SENTINEL: u8 = 0xa5;
const CR0_BASE: u64 = 0x8001_0033;
const CR0_EM: u64 = 1 << 2;
const CR0_TS: u64 = 1 << 3;
const CR4_BASE: u64 = 0x20;
const CR4_OSFXSR: u64 = 1 << 9;
const CR4_FP: u64 = CR4_BASE | CR4_OSFXSR | (1 << 10);
const RFLAGS_AC: u64 = 1 << 18;
const NONCANONICAL: u64 = (1 << 47) + 1;

struct Ram {
    bytes: Vec<u8>,
}

impl Ram {
    fn range(&self, address: u64, length: usize, access: AccessKind) -> Result<usize, MemoryError> {
        usize::try_from(address)
            .ok()
            .filter(|start| start.checked_add(length).is_some_and(|end| end <= RAM))
            .ok_or(MemoryError::Unmapped { address, access })
    }
}

impl GuestMemory for Ram {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        let start = self.range(address, 1, AccessKind::Fetch)?;
        Ok(self.bytes[start])
    }

    fn read(&mut self, address: u64, _: AccessContext, out: &mut [u8]) -> Result<(), MemoryError> {
        let start = self.range(address, out.len(), AccessKind::Read)?;
        out.copy_from_slice(&self.bytes[start..start + out.len()]);
        Ok(())
    }

    fn write(&mut self, address: u64, _: AccessContext, input: &[u8]) -> Result<(), MemoryError> {
        let start = self.range(address, input.len(), AccessKind::Write)?;
        self.bytes[start..start + input.len()].copy_from_slice(input);
        Ok(())
    }

    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        Ok(())
    }
}

fn context(ring3: bool) -> AccessContext {
    AccessContext {
        mode: InstructionMode::Long64,
        privilege: if ring3 {
            PrivilegeLevel::Ring3
        } else {
            PrivilegeLevel::Ring0
        },
        origin: AccessOrigin::Explicit,
    }
}

/// What a case starts from: distinct bytes in every XMM register, an x87 state with TOP = 5, mixed tags, distinct register
/// payloads and the C0/C3 condition bits, the control registers, general registers and 0x40 bytes of memory at DATA.
#[derive(Clone)]
struct Setup {
    xmm: [[u8; 16]; 16],
    x87: X87State,
    cr0: u64,
    cr4: u64,
    rflags: u64,
    regs: Vec<(usize, u64)>,
    data: [u8; 0x40],
}

fn setup() -> Setup {
    let mut x87 = X87State::fixture_seed();
    x87.instruction_selector = Some(0x2b);
    x87.data_selector = Some(0x53);
    x87.control_word = 0x037f;
    x87.status_word = (5 << 11) | 0x4100;
    x87.tag_word = 0x1b6c;
    for (index, register) in x87.registers.iter_mut().enumerate() {
        for (lane, byte) in register.iter_mut().enumerate() {
            *byte = (0x30 * index + 7 * lane + 1) as u8;
        }
    }
    Setup {
        xmm: core::array::from_fn(|index| {
            core::array::from_fn(|lane| (index * 37 + lane * 11 + 5) as u8)
        }),
        x87,
        cr0: CR0_BASE,
        cr4: CR4_FP,
        rflags: 2 | (1 << 9) | 0x85,
        regs: vec![(0, DATA), (1, 4), (3, DATA), (7, DATA)],
        data: core::array::from_fn(|index| (index * 7 + 3) as u8),
    }
}

impl Setup {
    fn reg(mut self, index: usize, value: u64) -> Self {
        self.regs.retain(|(slot, _)| *slot != index);
        self.regs.push((index, value));
        self
    }

    fn controls(mut self, cr0: u64, cr4: u64) -> Self {
        self.cr0 = cr0;
        self.cr4 = cr4;
        self
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Res {
    Retired,
    Fault(ArchitecturalFault),
    Unsupported(u8),
    Memory(MemoryError),
}

#[derive(Clone, Debug, PartialEq)]
struct Out {
    result: Res,
    rip: u64,
    rflags: u64,
    gpr: [u64; 16],
    xmm: [[u8; 16]; 16],
    x87: X87State,
    ram: Vec<u8>,
}

fn initial_state(setup: &Setup) -> LongModeState {
    let mut state = LongModeState {
        rip: CODE,
        rflags: setup.rflags,
        cr0: Some(setup.cr0),
        cr4: Some(setup.cr4),
        activity: Some(CpuActivity::Running),
        sti_shadow: Some(StiShadow::Clear),
        ds_selector: 0x18,
        es_selector: 0x18,
        ..LongModeState::default()
    };
    for (index, slot) in state.gpr.iter_mut().enumerate() {
        *slot = 0x0101_0101_0101_0101 * (index as u64 + 0x11);
    }
    for (index, value) in &setup.regs {
        state.gpr[*index] = *value;
    }
    state
}

fn initial_ram(code: &[u8], setup: &Setup) -> Ram {
    let mut bytes = vec![SENTINEL; RAM];
    bytes[CODE as usize..CODE as usize + code.len()].copy_from_slice(code);
    bytes[DATA as usize..DATA as usize + setup.data.len()].copy_from_slice(&setup.data);
    Ram { bytes }
}

fn result_of(exit: RunExit) -> Res {
    match exit {
        RunExit::InstructionBudgetExhausted => Res::Retired,
        RunExit::Fault(fault) => Res::Fault(fault),
        RunExit::Unsupported { opcode } => Res::Unsupported(opcode),
        RunExit::MemoryError(error) => Res::Memory(error),
        other => panic!("unexpected exit {other:?}"),
    }
}

/// One instruction in the CPL0 set. With the FP member the XMM and x87 state live in the cell and come back from it.
fn run_set(code: &[u8], setup: &Setup, member: bool) -> Out {
    let mut state = initial_state(setup);
    let mut ram = initial_ram(code, setup);
    let cell = RefCell::new(GuestFpState {
        x87: setup.x87.clone(),
        sse: SseState {
            xmm: setup.xmm,
            mxcsr: 0x1f80,
        },
    });
    let mut set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
        .with_cpuid(&CpuidProfile::X8664V1Stage4A);
    if member {
        set = set.with_fp_state(&cell);
    }
    let result = run_with_cpl0_profile_set(
        &mut state,
        &mut ram,
        RunConfig {
            instruction_budget: 1,
            stop_rip: None,
            context: context(false),
        },
        od_isa::INTEGER_FLAGS,
        &set,
        None,
        || false,
    )
    .expect("the case is admitted by the set");
    let Cpl0ProfileSetExit::Run(exit) = result.exit else {
        panic!("unexpected set exit {:?}", result.exit)
    };
    assert!(
        state.sse.is_none() && state.x87.is_none(),
        "the set hands the state back without FP"
    );
    let after = cell.borrow();
    Out {
        result: result_of(exit),
        rip: state.rip,
        rflags: state.rflags,
        gpr: state.gpr,
        xmm: after.sse.xmm,
        x87: after.x87.clone(),
        ram: ram.bytes,
    }
}

/// The bare run: one `step` on a state that carries the SSE and x87 state and no control registers (so no gates).
fn run_bare(code: &[u8], setup: &Setup, ring3: bool) -> Out {
    let mut state = initial_state(setup);
    state.cr0 = None;
    state.cr4 = None;
    state.sse = Some(SseState {
        xmm: setup.xmm,
        mxcsr: 0x1f80,
    });
    state.x87 = Some(setup.x87.clone());
    let mut ram = initial_ram(code, setup);
    let exit = match execute::step(&mut state, &mut ram, context(ring3)) {
        Ok(StepOutcome::Advanced) => RunExit::InstructionBudgetExhausted,
        Ok(StepOutcome::Fault(fault)) => RunExit::Fault(fault),
        Ok(StepOutcome::Unsupported { opcode }) => RunExit::Unsupported { opcode },
        Ok(other) => panic!("unexpected bare outcome {other:?}"),
        Err(error) => RunExit::MemoryError(error),
    };
    Out {
        result: result_of(exit),
        rip: state.rip,
        rflags: state.rflags,
        gpr: state.gpr,
        xmm: state.sse.expect("the bare state keeps its SSE state").xmm,
        x87: state.x87.expect("the bare state keeps its x87 state"),
        ram: ram.bytes,
    }
}

/// The state after a retired instruction that changed nothing yet; the caller edits what the instruction changes.
fn retired(code: &[u8], setup: &Setup) -> Out {
    let mut out = refused(code, setup, Res::Retired);
    out.rip = CODE + code.len() as u64;
    out
}

/// The state after an instruction that did not retire: everything as it started, RIP at the instruction.
fn refused(code: &[u8], setup: &Setup, result: Res) -> Out {
    let state = initial_state(setup);
    Out {
        result,
        rip: CODE,
        rflags: setup.rflags,
        gpr: state.gpr,
        xmm: setup.xmm,
        x87: setup.x87.clone(),
        ram: initial_ram(code, setup).bytes,
    }
}

fn fault(vector: u8) -> Res {
    Res::Fault(ArchitecturalFault {
        vector,
        error_code: None,
        linear_address: None,
    })
}

fn fault_code(vector: u8, error_code: u32) -> Res {
    Res::Fault(ArchitecturalFault {
        vector,
        error_code: Some(error_code),
        linear_address: None,
    })
}

/// `[prefixes] [REX] 0F opcode modrm tail`.
fn encode(prefixes: &[u8], rex: u8, opcode: u8, modrm: u8, tail: &[u8]) -> Vec<u8> {
    let mut code = prefixes.to_vec();
    if rex != 0 {
        code.push(rex);
    }
    code.extend_from_slice(&[0x0f, opcode, modrm]);
    code.extend_from_slice(tail);
    code
}

/// Register-direct form with ModRM.reg = reg and ModRM.r/m = rm (REX.R/REX.B for the numbers above 7).
fn register_form(prefixes: &[u8], opcode: u8, reg: u8, rm: u8) -> Vec<u8> {
    let rex = 0x40 | ((reg >> 3) << 2) | (rm >> 3);
    encode(
        prefixes,
        if rex == 0x40 { 0 } else { rex },
        opcode,
        0xc0 | (reg & 7) << 3 | (rm & 7),
        &[],
    )
}

/// Both runs of a case (the set with the FP member at CPL0, and the bare run at CPL0, which has no control registers and so no
/// gates) must give the same observation; a case with other control registers than the default pair runs in the set only.
fn both(code: &[u8], setup: &Setup) -> Out {
    let in_set = run_set(code, setup, true);
    if setup.cr0 != CR0_BASE || setup.cr4 != CR4_FP {
        return in_set;
    }
    let bare = run_bare(code, setup, false);
    assert_eq!(in_set.result, bare.result, "result for {code:02x?}");
    assert_eq!(in_set.rip, bare.rip, "rip for {code:02x?}");
    assert_eq!(in_set.rflags, bare.rflags, "rflags for {code:02x?}");
    assert_eq!(in_set.gpr, bare.gpr, "gpr for {code:02x?}");
    assert_eq!(in_set.xmm, bare.xmm, "xmm for {code:02x?}");
    assert_eq!(in_set.x87, bare.x87, "x87 for {code:02x?}");
    assert!(in_set.ram == bare.ram, "memory for {code:02x?}");
    in_set
}

const NT_STORES: [(&str, &[u8], u8); 3] = [
    ("MOVNTDQ", &[0x66], 0xe7),
    ("MOVNTPS", &[], 0x2b),
    ("MOVNTPD", &[0x66], 0x2b),
];

#[test]
fn non_temporal_stores_write_the_whole_source_register_in_every_addressing_form() {
    // (name, ModRM with reg 0, bytes after the ModRM, 67 prefix, target offset from DATA)
    let forms: [(&str, u8, &[u8], bool, u64); 5] = [
        ("[rax]", 0x00, &[], false, 0),
        ("[rax+0x10]", 0x40, &[0x10], false, 0x10),
        ("[rbx+rcx*4]", 0x04, &[0x8b], false, 0x10), // SIB: scale 4, index rcx = 4, base rbx
        ("RIP-relative", 0x05, &[], false, 0x30),
        ("[eax]", 0x00, &[], true, 0x20),
    ];
    for (name, prefixes, opcode) in NT_STORES {
        for (form, modrm, tail, a32, offset) in forms {
            for source in [0u8, 1, 7, 8, 15] {
                let mut prefix = Vec::new();
                if a32 {
                    prefix.push(0x67);
                }
                prefix.extend_from_slice(prefixes);
                let rex = if source >= 8 { 0x44 } else { 0 };
                let modrm = modrm | ((source & 7) << 3);
                let mut tail = tail.to_vec();
                if form == "RIP-relative" {
                    // the displacement is relative to the end of the instruction
                    let length = (prefix.len() + usize::from(rex != 0) + 3 + 4) as u64;
                    tail = (((DATA + offset) as i64 - (CODE + length) as i64) as i32)
                        .to_le_bytes()
                        .to_vec();
                }
                let mut s = setup();
                if a32 {
                    // the upper half of RAX is ignored by the 32-bit address size
                    s = s.reg(0, (0xdead_beef << 32) | (DATA + offset));
                }
                let code = encode(&prefix, rex, opcode, modrm, &tail);
                let mut expected = retired(&code, &s);
                let address = (DATA + offset) as usize;
                expected.ram[address..address + 16].copy_from_slice(&s.xmm[usize::from(source)]);
                assert_eq!(both(&code, &s), expected, "{name} {form} xmm{source}");
            }
        }
    }
}

#[test]
fn a_misaligned_non_temporal_store_is_general_protection_and_writes_nothing() {
    for (name, prefixes, opcode) in NT_STORES {
        for offset in 1..16u64 {
            let s = setup().reg(0, DATA + offset);
            let code = encode(prefixes, 0, opcode, 0x08, &[]);
            let expected = refused(&code, &s, fault_code(13, 0));
            assert_eq!(both(&code, &s), expected, "{name} offset {offset}");
        }
    }
}

#[test]
fn the_gates_of_the_non_temporal_stores_come_before_the_alignment_and_the_access() {
    for (name, prefixes, opcode) in NT_STORES {
        let code = encode(prefixes, 0, opcode, 0x08, &[]);
        for (label, cr0, cr4, result) in [
            ("EM", CR0_BASE | CR0_EM, CR4_FP, fault(6)),
            ("OSFXSR = 0", CR0_BASE, CR4_BASE, fault(6)),
            ("TS", CR0_BASE | CR0_TS, CR4_FP, fault(7)),
            (
                "EM and TS (#UD wins)",
                CR0_BASE | CR0_EM | CR0_TS,
                CR4_FP,
                fault(6),
            ),
            ("none (alignment)", CR0_BASE, CR4_FP, fault_code(13, 0)),
        ] {
            // misaligned and non-canonical: only the alignment fault can follow, and only without a gate
            let s = setup().controls(cr0, cr4).reg(0, DATA + 1);
            let expected = refused(&code, &s, result);
            assert_eq!(both(&code, &s), expected, "{name} {label}");
        }
    }
}

#[test]
fn non_temporal_store_addresses_fault_like_the_other_sse_stores() {
    for (name, prefixes, opcode) in NT_STORES {
        let code = encode(prefixes, 0, opcode, 0x08, &[]);
        // aligned, but past the end of RAM: the backend error, nothing changed
        let s = setup().reg(0, RAM as u64 - 16 + 16);
        let expected = refused(
            &code,
            &s,
            Res::Memory(MemoryError::Unmapped {
                address: RAM as u64,
                access: AccessKind::Write,
            }),
        );
        assert_eq!(both(&code, &s), expected, "{name} past the end");
        // a non-canonical base is #GP(0); a base of RBP is #SS(0)
        let s = setup().reg(0, NONCANONICAL & !15);
        assert_eq!(
            both(&code, &s),
            refused(&code, &s, fault_code(13, 0)),
            "{name} non-canonical"
        );
        let code = encode(prefixes, 0, opcode, 0x45, &[0x00]);
        let s = setup().reg(5, NONCANONICAL & !15);
        assert_eq!(
            both(&code, &s),
            refused(&code, &s, fault_code(12, 0)),
            "{name} RBP"
        );
    }
}

#[test]
fn non_temporal_store_encodings_the_sdm_does_not_define_are_invalid_opcode() {
    let mut forms: Vec<(String, Vec<u8>)> = Vec::new();
    for (name, prefixes, opcode) in NT_STORES {
        forms.push((
            format!("{name} register form"),
            encode(prefixes, 0, opcode, 0xc1, &[]),
        ));
        forms.push((
            format!("{name} register form REX.R"),
            encode(prefixes, 0x44, opcode, 0xca, &[]),
        ));
        let mut locked = vec![0xf0];
        locked.extend(encode(prefixes, 0, opcode, 0x08, &[]));
        forms.push((format!("LOCK {name}"), locked));
    }
    for (name, code) in forms {
        // gates and a misaligned address would fault otherwise: the #UD of the encoding comes first
        let s = setup().controls(CR0_BASE | CR0_TS, CR4_FP).reg(0, DATA + 1);
        assert_eq!(both(&code, &s), refused(&code, &s, fault(6)), "{name}");
    }
}

#[test]
fn encodings_with_another_mandatory_prefix_stay_refused_without_a_change() {
    let forms: [(&str, &[u8], u8); 14] = [
        ("MOVNTDQ with FS", &[0x64, 0x66, 0x0f, 0xe7, 0x08], 0xe7),
        ("MOVNTPS with GS", &[0x65, 0x0f, 0x2b, 0x08], 0x2b),
        ("F3 0F 2B (MOVNTSS)", &[0xf3, 0x0f, 0x2b, 0x08], 0x2b),
        ("F2 0F 2B (MOVNTSD)", &[0xf2, 0x0f, 0x2b, 0x08], 0x2b),
        ("F3 0F E7", &[0xf3, 0x0f, 0xe7, 0x08], 0xe7),
        ("F2 0F E7", &[0xf2, 0x0f, 0xe7, 0x08], 0xe7),
        ("MOVNTQ (no prefix 0F E7)", &[0x0f, 0xe7, 0x08], 0xe7),
        ("66 66 0F E7", &[0x66, 0x66, 0x0f, 0xe7, 0x08], 0xe7),
        ("66 F3 0F E7", &[0x66, 0xf3, 0x0f, 0xe7, 0x08], 0xe7),
        ("MASKMOVDQU with F3", &[0xf3, 0x66, 0x0f, 0xf7, 0xca], 0xf7),
        ("MASKMOVDQU with FS", &[0x64, 0x66, 0x0f, 0xf7, 0xca], 0xf7),
        ("MASKMOVQ (no prefix 0F F7)", &[0x0f, 0xf7, 0xca], 0xf7),
        (
            "MASKMOVDQU repeated 67",
            &[0x67, 0x67, 0x66, 0x0f, 0xf7, 0xca],
            0xf7,
        ),
        ("66 F2 0F D6", &[0x66, 0xf2, 0x0f, 0xd6, 0xca], 0xd6),
    ];
    for (name, code, opcode) in forms {
        let s = setup();
        assert_eq!(
            both(code, &s),
            refused(code, &s, Res::Unsupported(opcode)),
            "{name}"
        );
    }
    let more: [(&str, &[u8], u8); 6] = [
        ("F3 66 0F D6", &[0xf3, 0x66, 0x0f, 0xd6, 0xca], 0xd6),
        ("F3 F3 0F D6", &[0xf3, 0xf3, 0x0f, 0xd6, 0xca], 0xd6),
        ("F2 F3 0F D6", &[0xf2, 0xf3, 0x0f, 0xd6, 0xca], 0xd6),
        ("F3 0F D6 with GS", &[0x65, 0xf3, 0x0f, 0xd6, 0xca], 0xd6),
        ("0F D6 without a prefix", &[0x0f, 0xd6, 0xca], 0xd6),
        (
            "F3 0F D7 (not an instruction)",
            &[0xf3, 0x0f, 0xd7, 0xca],
            0xd7,
        ),
    ];
    for (name, code, opcode) in more {
        let s = setup();
        assert_eq!(
            both(code, &s),
            refused(code, &s, Res::Unsupported(opcode)),
            "{name}"
        );
    }
}

/// The SDM operation of MASKMOVDQU: byte i of the destination memory is byte i of the source when bit 7 of mask byte i is 1.
fn reference_maskmov(memory: &mut [u8], source: &[u8; 16], mask: &[u8; 16]) {
    for lane in 0..16 {
        if mask[lane] >> 7 == 1 {
            memory[lane] = source[lane];
        }
    }
}

fn mask_patterns() -> Vec<(&'static str, [u8; 16])> {
    vec![
        ("zero", [0; 16]),
        ("all ones", [0xff; 16]),
        ("bit 7 only", [0x80; 16]),
        ("bit 7 clear, others set", [0x7f; 16]),
        (
            "alternating 80/7f",
            core::array::from_fn(|lane| if lane % 2 == 0 { 0x80 } else { 0x7f }),
        ),
        (
            "low half",
            core::array::from_fn(|lane| if lane < 8 { 0xff } else { 0 }),
        ),
        (
            "high half",
            core::array::from_fn(|lane| if lane < 8 { 0 } else { 0xff }),
        ),
        (
            "first byte",
            core::array::from_fn(|lane| if lane == 0 { 0x80 } else { 0 }),
        ),
        (
            "last byte",
            core::array::from_fn(|lane| if lane == 15 { 0x80 } else { 0 }),
        ),
        (
            "mixed",
            core::array::from_fn(|lane| (lane as u8).wrapping_mul(0x53) ^ 0x2d),
        ),
    ]
}

#[test]
fn maskmovdqu_stores_the_bytes_selected_by_the_most_significant_bit_of_the_mask() {
    for (label, mask) in mask_patterns() {
        // alignment: the destination is any address
        for offset in 0..17u64 {
            let mut s = setup().reg(7, DATA + 0x10 + offset);
            s.xmm[2] = mask;
            let code = register_form(&[0x66], 0xf7, 1, 2);
            let mut expected = retired(&code, &s);
            let address = (DATA + 0x10 + offset) as usize;
            reference_maskmov(&mut expected.ram[address..address + 16], &s.xmm[1], &mask);
            assert_eq!(both(&code, &s), expected, "{label} at +{offset}");
        }
    }
}

#[test]
fn maskmovdqu_takes_the_data_and_the_mask_from_any_pair_of_registers() {
    for (data, mask) in [(0u8, 1u8), (8, 1), (1, 9), (15, 14), (3, 3), (12, 0)] {
        let s = setup().reg(7, DATA + 0x20);
        let mut s = s;
        s.xmm[usize::from(mask)] = mask_patterns()[9].1;
        let code = register_form(&[0x66], 0xf7, data, mask);
        let mut expected = retired(&code, &s);
        let address = (DATA + 0x20) as usize;
        // when the data and the mask are the same register, the register is both
        let (source, selector) = (s.xmm[usize::from(data)], s.xmm[usize::from(mask)]);
        reference_maskmov(&mut expected.ram[address..address + 16], &source, &selector);
        assert_eq!(both(&code, &s), expected, "xmm{data}, xmm{mask}");
    }
}

#[test]
fn maskmovdqu_uses_rdi_or_edi_and_ignores_the_zero_based_segment_overrides() {
    let mask = mask_patterns()[4].1;
    let mut base = setup();
    base.xmm[2] = mask;
    let stored = |s: &Setup, code: &[u8], offset: u64| {
        let mut expected = retired(code, s);
        let address = (DATA + offset) as usize;
        reference_maskmov(&mut expected.ram[address..address + 16], &s.xmm[1], &mask);
        expected
    };
    // 67: the address is EDI, the upper half of RDI is ignored
    let s = base.clone().reg(7, (0xdead_beef << 32) | (DATA + 5));
    let code = register_form(&[0x67, 0x66], 0xf7, 1, 2);
    assert_eq!(both(&code, &s), stored(&s, &code, 5), "67");
    let code = register_form(&[0x66, 0x67], 0xf7, 1, 2);
    assert_eq!(
        both(&code, &s),
        stored(&s, &code, 5),
        "66 before 67 reversed"
    );
    // DS, ES, CS and SS overrides have a zero base in 64-bit mode (FS and GS are refused, see above)
    for segment in [0x2e, 0x3e, 0x26, 0x36] {
        let s = base.clone().reg(7, DATA + 9);
        let code = register_form(&[segment, 0x66], 0xf7, 1, 2);
        assert_eq!(
            both(&code, &s),
            stored(&s, &code, 9),
            "segment {segment:#x}"
        );
    }
    // REX.W does not change anything
    let s = base.clone().reg(7, DATA + 3);
    let code = encode(&[0x66], 0x48, 0xf7, 0xca, &[]);
    assert_eq!(both(&code, &s), stored(&s, &code, 3), "REX.W");
}

#[test]
fn maskmovdqu_gates_and_undefined_forms() {
    let code = register_form(&[0x66], 0xf7, 1, 2);
    for (label, cr0, cr4, result) in [
        ("EM", CR0_BASE | CR0_EM, CR4_FP, fault(6)),
        ("OSFXSR = 0", CR0_BASE, CR4_BASE, fault(6)),
        ("TS", CR0_BASE | CR0_TS, CR4_FP, fault(7)),
        ("EM and TS", CR0_BASE | CR0_EM | CR0_TS, CR4_FP, fault(6)),
    ] {
        // a non-canonical RDI would fault otherwise
        let s = setup().controls(cr0, cr4).reg(7, NONCANONICAL);
        assert_eq!(both(&code, &s), refused(&code, &s, result), "{label}");
    }
    for (label, code) in [
        ("memory form [rax]", encode(&[0x66], 0, 0xf7, 0x08, &[])),
        (
            "memory form [rbx+8]",
            encode(&[0x66], 0, 0xf7, 0x4b, &[0x08]),
        ),
        ("LOCK", {
            let mut code = vec![0xf0];
            code.extend(register_form(&[0x66], 0xf7, 1, 2));
            code
        }),
    ] {
        let s = setup()
            .controls(CR0_BASE | CR0_TS, CR4_FP)
            .reg(7, NONCANONICAL);
        assert_eq!(both(&code, &s), refused(&code, &s, fault(6)), "{label}");
    }
}

#[test]
fn maskmovdqu_faults_on_a_non_canonical_destination_or_a_range_that_leaves_the_canonical_space() {
    let code = register_form(&[0x66], 0xf7, 1, 2);
    for (label, rdi) in [
        ("non-canonical", NONCANONICAL),
        ("garbage", 0x1234_5678_1234_5670),
        // the first byte is canonical, the last (+15) is not
        ("last byte non-canonical", 0x0000_7fff_ffff_fff8),
    ] {
        let s = setup().reg(7, rdi);
        assert_eq!(
            both(&code, &s),
            refused(&code, &s, fault_code(13, 0)),
            "{label}"
        );
    }
}

#[test]
fn maskmovdqu_faults_for_any_of_its_16_bytes_whatever_the_mask_and_then_writes_nothing() {
    // The physical oracle faults on a split destination whose second page is not accessible even when the mask selects
    // no byte of it (and for a mask of zero), and the bytes of the first page stay as they were.
    let code = register_form(&[0x66], 0xf7, 1, 2);
    let end = RAM as u64 - 8;
    for (label, mask) in mask_patterns() {
        let mut s = setup().reg(7, end);
        s.xmm[2] = mask;
        let expected = refused(
            &code,
            &s,
            Res::Memory(MemoryError::Unmapped {
                address: end,
                access: AccessKind::Write,
            }),
        );
        assert_eq!(both(&code, &s), expected, "{label}");
    }
    // entirely outside RAM
    let mut s = setup().reg(7, RAM as u64);
    s.xmm[2] = [0xff; 16];
    let expected = refused(
        &code,
        &s,
        Res::Memory(MemoryError::Unmapped {
            address: RAM as u64,
            access: AccessKind::Write,
        }),
    );
    assert_eq!(both(&code, &s), expected, "outside");
    // a destination that ends exactly at the end of RAM is fine
    let mut s = setup().reg(7, RAM as u64 - 16);
    s.xmm[2] = [0x80; 16];
    let mut expected = retired(&code, &s);
    expected.ram[RAM - 16..].copy_from_slice(&s.xmm[1]);
    assert_eq!(both(&code, &s), expected, "last 16 bytes of RAM");
}

#[test]
fn maskmovdqu_alignment_check_is_an_eight_byte_rule() {
    // Physical oracle (Core i5-3210M, CPL3, RFLAGS.AC = 1): offsets 0, 8 and 16 retire, every other offset is SIGBUS/ADRALN,
    // whatever the mask. ADR-0023 C1 (decision 4) models #AC with this 8-byte requirement (a choice of the model by the
    // oracle): #AC(0) at CPL3 with AC set and CR0.AM = 1 (the bare run carries no CR0, which counts as AM = 1).
    let code = register_form(&[0x66], 0xf7, 1, 2);
    for offset in 0..17u64 {
        let mut s = setup().reg(7, DATA + offset);
        s.rflags |= RFLAGS_AC;
        s.xmm[2] = [0xff; 16];
        let ring3 = run_bare(&code, &s, true);
        if offset % 8 == 0 {
            let mut expected = retired(&code, &s);
            let address = (DATA + offset) as usize;
            expected.ram[address..address + 16].copy_from_slice(&s.xmm[1]);
            assert_eq!(ring3, expected, "aligned +{offset}");
        } else {
            assert_eq!(
                ring3,
                refused(
                    &code,
                    &s,
                    Res::Fault(ArchitecturalFault {
                        vector: 17,
                        error_code: Some(0),
                        linear_address: None
                    })
                ),
                "misaligned +{offset}"
            );
        }
        // CPL0 ignores the flag
        let mut expected = retired(&code, &s);
        let address = (DATA + offset) as usize;
        expected.ram[address..address + 16].copy_from_slice(&s.xmm[1]);
        assert_eq!(run_bare(&code, &s, false), expected, "CPL0 +{offset}");
    }
}

fn mm_value(x87: &X87State, index: usize) -> [u8; 8] {
    x87.registers[index][..8].try_into().unwrap()
}

/// The x87 to MMX transition (Intel SDM Vol. 2B MOVQ2DQ/MOVDQ2Q: "the x87 FPU top-of-stack pointer is set to 0 and the x87
/// FPU tag word is set to all 0s"); the other status word bits and the data registers stay.
fn transition(x87: &mut X87State) {
    x87.status_word &= !(7 << 11);
    x87.tag_word = 0;
}

#[test]
fn movq2dq_zero_extends_the_mmx_register_into_the_xmm_register_and_enters_mmx_state() {
    for mm in 0..8u8 {
        for xmm in [0u8, 1, 7, 8, 15] {
            let s = setup();
            let code = register_form(&[0xf3], 0xd6, xmm, mm);
            // REX.B would extend the XMM-side only for MOVDQ2Q; here the r/m operand is the MMX register
            let code = if mm >= 8 { unreachable!() } else { code };
            let mut expected = retired(&code, &s);
            let mut value = [0u8; 16];
            value[..8].copy_from_slice(&mm_value(&s.x87, usize::from(mm)));
            expected.xmm[usize::from(xmm)] = value;
            transition(&mut expected.x87);
            assert_eq!(both(&code, &s), expected, "MOVQ2DQ xmm{xmm}, mm{mm}");
        }
    }
}

#[test]
fn movdq2q_writes_the_mmx_register_with_all_ones_in_the_exponent_and_enters_mmx_state() {
    for mm in 0..8u8 {
        for xmm in [0u8, 1, 7, 8, 15] {
            let s = setup();
            let code = register_form(&[0xf2], 0xd6, mm, xmm);
            let mut expected = retired(&code, &s);
            let low: [u8; 8] = s.xmm[usize::from(xmm)][..8].try_into().unwrap();
            expected.x87.registers[usize::from(mm)][..8].copy_from_slice(&low);
            expected.x87.registers[usize::from(mm)][8..].copy_from_slice(&[0xff, 0xff]);
            transition(&mut expected.x87);
            assert_eq!(both(&code, &s), expected, "MOVDQ2Q mm{mm}, xmm{xmm}");
        }
    }
}

#[test]
fn the_mmx_register_number_ignores_the_rex_bits() {
    // MOVQ2DQ xmm, mm: REX.R extends the XMM register (ModRM.reg), REX.B does not extend the MMX register (ModRM.r/m).
    let s = setup();
    let code = encode(&[0xf3], 0x45, 0xd6, 0xd1, &[]); // REX.R|B, reg = 2, rm = 1
    let mut expected = retired(&code, &s);
    let mut value = [0u8; 16];
    value[..8].copy_from_slice(&mm_value(&s.x87, 1));
    expected.xmm[10] = value;
    transition(&mut expected.x87);
    assert_eq!(both(&code, &s), expected, "MOVQ2DQ REX.R|B");
    // MOVDQ2Q mm, xmm: REX.B extends the XMM register (ModRM.r/m), REX.R does not extend the MMX register (ModRM.reg).
    let code = encode(&[0xf2], 0x45, 0xd6, 0xd1, &[]); // reg = 2, rm = 1 -> xmm9, mm2
    let mut expected = retired(&code, &s);
    let low: [u8; 8] = s.xmm[9][..8].try_into().unwrap();
    expected.x87.registers[2][..8].copy_from_slice(&low);
    expected.x87.registers[2][8..].copy_from_slice(&[0xff, 0xff]);
    transition(&mut expected.x87);
    assert_eq!(both(&code, &s), expected, "MOVDQ2Q REX.R|B");
    // REX.W changes nothing
    let code = encode(&[0xf3], 0x48, 0xd6, 0xd1, &[]);
    let mut expected = retired(&code, &s);
    let mut value = [0u8; 16];
    value[..8].copy_from_slice(&mm_value(&s.x87, 1));
    expected.xmm[2] = value;
    transition(&mut expected.x87);
    assert_eq!(both(&code, &s), expected, "MOVQ2DQ REX.W");
}

#[test]
fn the_mmx_moves_leave_the_other_x87_state_and_the_condition_bits_alone() {
    for top in 0..8u16 {
        let mut s = setup();
        s.x87.status_word = (top << 11) | 0x4500 | 0x0004;
        s.x87.control_word = 0x027f;
        let code = register_form(&[0xf3], 0xd6, 3, 4);
        let out = both(&code, &s);
        assert_eq!(
            out.x87.status_word, 0x4504,
            "TOP {top}: only TOP changes in the status word"
        );
        assert_eq!(out.x87.tag_word, 0, "TOP {top}: all tags valid");
        assert_eq!(out.x87.control_word, 0x027f);
        assert_eq!(
            out.x87.registers, s.x87.registers,
            "a MOVQ2DQ does not rewrite its source"
        );
    }
}

#[test]
fn the_mmx_moves_are_gated_by_em_os_fxsr_ts_and_a_pending_x87_exception() {
    for prefix in [0xf3u8, 0xf2] {
        let code = register_form(&[prefix], 0xd6, 1, 2);
        let name = if prefix == 0xf3 { "MOVQ2DQ" } else { "MOVDQ2Q" };
        let mut pending = setup();
        pending.x87.status_word |= 0x0081; // ES and an invalid-operation flag
        pending.x87.control_word &= !1; // the invalid-operation exception is unmasked
        let masked_pending = {
            let mut masked = setup();
            masked.x87.status_word |= 0x0081; // ES with a masked flag is not a pending exception
            masked
        };
        for (label, s, result) in [
            ("EM", setup().controls(CR0_BASE | CR0_EM, CR4_FP), fault(6)),
            ("OSFXSR = 0", setup().controls(CR0_BASE, CR4_BASE), fault(6)),
            ("TS", setup().controls(CR0_BASE | CR0_TS, CR4_FP), fault(7)),
            (
                "EM and TS",
                setup().controls(CR0_BASE | CR0_EM | CR0_TS, CR4_FP),
                fault(6),
            ),
            (
                "TS before the pending exception",
                {
                    let mut s = pending.clone();
                    s.cr0 |= CR0_TS;
                    s
                },
                fault(7),
            ),
            ("pending exception", pending.clone(), fault(16)),
        ] {
            assert_eq!(
                both(&code, &s),
                refused(&code, &s, result),
                "{name} {label}"
            );
        }
        // the exception flag with ES set but the exception masked does not stop the instruction
        let out = both(&code, &masked_pending);
        assert_eq!(out.result, Res::Retired, "{name} masked ES");
    }
}

#[test]
fn the_mmx_moves_have_register_forms_only_and_lock_is_invalid() {
    for prefix in [0xf3u8, 0xf2] {
        for (label, code) in [
            ("[rax]", encode(&[prefix], 0, 0xd6, 0x08, &[])),
            ("[rbx+8]", encode(&[prefix], 0, 0xd6, 0x4b, &[0x08])),
            (
                "RIP-relative",
                encode(&[prefix], 0, 0xd6, 0x0d, &[0, 0, 0, 0]),
            ),
            ("LOCK", {
                let mut code = vec![0xf0];
                code.extend(register_form(&[prefix], 0xd6, 1, 2));
                code
            }),
        ] {
            // gates and a pending exception would fault otherwise: the #UD of the encoding comes first
            let mut s = setup().controls(CR0_BASE | CR0_TS, CR4_FP);
            s.x87.status_word |= 0x0081;
            s.x87.control_word &= !1;
            assert_eq!(
                both(&code, &s),
                refused(&code, &s, fault(6)),
                "{prefix:#x} {label}"
            );
        }
    }
}

#[test]
fn without_the_fp_member_the_set_refuses_every_form_of_the_slice() {
    let forms: [(&str, Vec<u8>, u8); 6] = [
        ("MOVNTDQ", encode(&[0x66], 0, 0xe7, 0x08, &[]), 0xe7),
        ("MOVNTPS", encode(&[], 0, 0x2b, 0x08, &[]), 0x2b),
        ("MOVNTPD", encode(&[0x66], 0, 0x2b, 0x08, &[]), 0x2b),
        ("MASKMOVDQU", register_form(&[0x66], 0xf7, 1, 2), 0xf7),
        ("MOVQ2DQ", register_form(&[0xf3], 0xd6, 1, 2), 0xd6),
        ("MOVDQ2Q", register_form(&[0xf2], 0xd6, 1, 2), 0xd6),
    ];
    for (name, code, opcode) in forms {
        let s = setup();
        let out = run_set(&code, &s, false);
        assert_eq!(out, refused(&code, &s, Res::Unsupported(opcode)), "{name}");
    }
}

#[test]
fn the_slice_is_a_cpl0_set_form_and_the_bare_run_without_sse_state_refuses_it() {
    // A state without the SSE (and x87) state in the bare run: Unsupported, nothing changes.
    let forms: [(&str, Vec<u8>, u8); 4] = [
        ("MOVNTDQ", encode(&[0x66], 0, 0xe7, 0x08, &[]), 0xe7),
        ("MASKMOVDQU", register_form(&[0x66], 0xf7, 1, 2), 0xf7),
        ("MOVQ2DQ", register_form(&[0xf3], 0xd6, 1, 2), 0xd6),
        ("MOVDQ2Q", register_form(&[0xf2], 0xd6, 1, 2), 0xd6),
    ];
    for (name, code, opcode) in forms {
        let s = setup();
        let mut state = initial_state(&s);
        let mut ram = initial_ram(&code, &s);
        let before = state.clone();
        let outcome = execute::step(&mut state, &mut ram, context(false));
        assert!(
            matches!(outcome, Ok(StepOutcome::Unsupported { .. })),
            "{name} ({opcode:#x}): {outcome:?}"
        );
        assert_eq!(state, before, "{name}");
        assert_eq!(ram.bytes, initial_ram(&code, &s).bytes, "{name}");
    }
}

#[test]
fn one_program_of_the_six_forms_runs_in_the_set() {
    // MOVNTPS [rax], xmm1; MASKMOVDQU xmm1, xmm2 (rdi = DATA + 0x20); MOVQ2DQ xmm3, mm5; MOVDQ2Q mm6, xmm3
    let program: Vec<u8> = [
        vec![0x0f, 0x2b, 0x08],
        vec![0x66, 0x0f, 0xf7, 0xca],
        vec![0xf3, 0x0f, 0xd6, 0xdd],
        vec![0xf2, 0x0f, 0xd6, 0xf3],
    ]
    .concat();
    let mut s = setup().reg(7, DATA + 0x20);
    s.xmm[2] = mask_patterns()[4].1;
    let mut state = initial_state(&s);
    let mut ram = initial_ram(&program, &s);
    let cell = RefCell::new(GuestFpState {
        x87: s.x87.clone(),
        sse: SseState {
            xmm: s.xmm,
            mxcsr: 0x1f80,
        },
    });
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
        .with_cpuid(&CpuidProfile::X8664V1Stage4A)
        .with_fp_state(&cell);
    let result = run_with_cpl0_profile_set(
        &mut state,
        &mut ram,
        RunConfig {
            instruction_budget: 4,
            stop_rip: None,
            context: context(false),
        },
        od_isa::INTEGER_FLAGS,
        &set,
        None,
        || false,
    )
    .expect("the program is admitted by the set");
    assert_eq!(result.instructions_executed, 4);
    assert_eq!(state.rip, CODE + program.len() as u64);
    let after = cell.borrow();
    // MOVNTPS stored xmm1 at DATA
    assert_eq!(&ram.bytes[DATA as usize..DATA as usize + 16], &s.xmm[1]);
    // MASKMOVDQU merged xmm1 into DATA + 0x20 under the mask of xmm2
    let mut expected = s.data;
    reference_maskmov(&mut expected[0x20..0x30], &s.xmm[1], &s.xmm[2]);
    assert_eq!(
        &ram.bytes[DATA as usize + 0x20..DATA as usize + 0x30],
        &expected[0x20..0x30]
    );
    // MOVQ2DQ xmm3, mm5 then MOVDQ2Q mm6, xmm3
    let mut xmm3 = [0u8; 16];
    xmm3[..8].copy_from_slice(&mm_value(&s.x87, 5));
    assert_eq!(after.sse.xmm[3], xmm3);
    assert_eq!(mm_value(&after.x87, 6), mm_value(&s.x87, 5));
    assert_eq!(&after.x87.registers[6][8..], &[0xff, 0xff]);
    assert_eq!(after.x87.tag_word, 0);
    assert_eq!(after.x87.status_word, 0x4100);
}

/// Memory whose reads fail with a chosen error and whose writes land in RAM: it shows how the model reports the read that
/// MASKMOVDQU makes for a partial mask (it stands for the page walk of the write, so a read failure is a write failure).
struct FailingRead {
    ram: Ram,
    error: MemoryError,
    reads: usize,
}

impl GuestMemory for FailingRead {
    fn fetch(&mut self, address: u64, context: AccessContext) -> Result<u8, MemoryError> {
        self.ram.fetch(address, context)
    }

    fn read(&mut self, _: u64, _: AccessContext, _: &mut [u8]) -> Result<(), MemoryError> {
        self.reads += 1;
        Err(self.error.clone())
    }

    fn write(
        &mut self,
        address: u64,
        context: AccessContext,
        input: &[u8],
    ) -> Result<(), MemoryError> {
        self.ram.write(address, context, input)
    }

    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        Ok(())
    }
}

/// One bare MASKMOVDQU xmm1, xmm2 with [rdi] = DATA and the given mask, over a memory whose reads fail with `error`.
/// Returns the result, the number of reads the instruction made and the bytes of RAM after it.
fn maskmovdqu_over_failing_read(error: MemoryError, mask: [u8; 16]) -> (Res, usize, Vec<u8>) {
    let code = register_form(&[0x66], 0xf7, 1, 2);
    let mut s = setup().reg(7, DATA);
    s.xmm[2] = mask;
    let mut state = initial_state(&s);
    state.cr0 = None;
    state.cr4 = None;
    state.sse = Some(SseState {
        xmm: s.xmm,
        mxcsr: 0x1f80,
    });
    state.x87 = Some(s.x87.clone());
    let mut memory = FailingRead {
        ram: initial_ram(&code, &s),
        error,
        reads: 0,
    };
    let result = match execute::step(&mut state, &mut memory, context(false)) {
        Ok(StepOutcome::Advanced) => Res::Retired,
        Ok(StepOutcome::Fault(fault)) => Res::Fault(fault),
        Ok(StepOutcome::Unsupported { opcode }) => Res::Unsupported(opcode),
        Ok(other) => panic!("unexpected bare outcome {other:?}"),
        Err(error) => Res::Memory(error),
    };
    (result, memory.reads, memory.ram.bytes)
}

#[test]
fn maskmovdqu_reports_a_failure_of_its_partial_mask_read_as_a_failure_of_the_write() {
    // A partial mask makes the model read the 16 bytes first; that read stands for the page walk of the write, so a
    // page fault gets the W/R bit of its error code and a fixture failure the access kind Write. Hardware reports the
    // fault of the store itself. A mask whose byte 0 has bit 7 clear is partial.
    let mut partial = [0xff; 16];
    partial[0] = 0x7f;
    let untouched = initial_ram(&register_form(&[0x66], 0xf7, 1, 2), &setup()).bytes;
    let page_fault = |error_code| {
        MemoryError::Architectural(ArchitecturalFault {
            vector: 14,
            error_code,
            linear_address: Some(DATA),
        })
    };
    let reported = |error_code| {
        Res::Fault(ArchitecturalFault {
            vector: 14,
            error_code,
            linear_address: Some(DATA),
        })
    };
    // (error of the read, what the instruction reports)
    let cases = [
        // not present, supervisor read: the write bit is added (0b000 -> 0b010)
        (page_fault(Some(0)), reported(Some(0b010))),
        // protection violation, user read (0b101 -> 0b111)
        (page_fault(Some(0b101)), reported(Some(0b111))),
        // the write bit already set stays set
        (page_fault(Some(0b010)), reported(Some(0b010))),
        // no error code stays without one
        (page_fault(None), reported(None)),
        (
            MemoryError::Unmapped {
                address: DATA,
                access: AccessKind::Read,
            },
            Res::Memory(MemoryError::Unmapped {
                address: DATA,
                access: AccessKind::Write,
            }),
        ),
        (
            MemoryError::PermissionDenied {
                address: DATA,
                access: AccessKind::Read,
            },
            Res::Memory(MemoryError::PermissionDenied {
                address: DATA,
                access: AccessKind::Write,
            }),
        ),
        // another architectural fault and another error are passed on as they are
        (
            MemoryError::Architectural(ArchitecturalFault {
                vector: 13,
                error_code: Some(0),
                linear_address: None,
            }),
            Res::Fault(ArchitecturalFault {
                vector: 13,
                error_code: Some(0),
                linear_address: None,
            }),
        ),
        (
            MemoryError::Backend("read".into()),
            Res::Memory(MemoryError::Backend("read".into())),
        ),
    ];
    for (index, (error, expected)) in cases.into_iter().enumerate() {
        let (result, reads, ram) = maskmovdqu_over_failing_read(error, partial);
        assert_eq!(result, expected, "case {index}");
        assert_eq!(reads, 1, "case {index}: one read for the partial mask");
        assert_eq!(ram, untouched, "case {index}: nothing is written");
    }
    // A mask of all ones needs no read, so a failing read does not matter: the store retires.
    let (result, reads, ram) =
        maskmovdqu_over_failing_read(MemoryError::Backend("read".into()), [0xff; 16]);
    assert_eq!(result, Res::Retired);
    assert_eq!(reads, 0);
    assert_ne!(ram, untouched);
}
