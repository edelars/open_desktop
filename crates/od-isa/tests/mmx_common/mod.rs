//! Shared fixtures of the S2-33 / S2-54 MMX tests: a byte RAM that records accesses, a CPU whose x87 registers
//! carry recognisable payloads (so an unwanted exponent or tag write is visible), the table of the sixteen
//! SSSE3 integer forms and the code builders of their MMX encodings (no mandatory prefix).

#![allow(dead_code)]

use od_isa::execute::{self, Cr4Profile, RunConfig, StepOutcome};
use od_isa::long_decode::Ssse3IntKind;
use od_isa::state::{SseState, X87State};
use od_isa::{
    AccessContext, AccessKind, ArchitecturalFault, CpuActivity, GuestMemory, LongModeState,
    MemoryError, PrivilegeLevel, StiShadow,
};

pub const CODE: u64 = 0x2000;
pub const RAM: usize = 0x10000;
pub const SENTINEL: u8 = 0xcc;
pub const FLAGS: u64 = 0x2 | 0xcd5 | (1 << 10);
pub const MXCSR: u32 = 0x1fa5;
/// The data address that the `[rax]` forms use (8-byte aligned).
pub const DATA: u64 = 0x8000;
/// PG | WP | NE | ET | MP | PE: EM = 0, TS = 0.
pub const CR0_BASE: u64 = 0x8001_0033;
pub const CR4_PAE: u64 = 0x20;
pub const CR4_OSFXSR: u64 = 0x200;

pub struct Ram {
    pub bytes: Vec<u8>,
    pub reads: Vec<(u64, usize)>,
    pub writes: Vec<(u64, usize)>,
    /// Any data access that touches this page raises the #PF below.
    pub fault_page: Option<u64>,
}

impl Ram {
    pub fn new(code: &[u8]) -> Self {
        let mut bytes = vec![SENTINEL; RAM];
        bytes[CODE as usize..CODE as usize + code.len()].copy_from_slice(code);
        Self {
            bytes,
            reads: vec![],
            writes: vec![],
            fault_page: None,
        }
    }

    pub fn put(&mut self, address: u64, data: &[u8]) {
        self.bytes[address as usize..address as usize + data.len()].copy_from_slice(data);
    }

    pub fn get(&self, address: u64, length: usize) -> &[u8] {
        &self.bytes[address as usize..address as usize + length]
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

/// The MMX data of physical x87 register `index` in the seed state: eight distinguishable bytes.
pub fn mmx_pattern(index: usize) -> [u8; 8] {
    core::array::from_fn(|lane| {
        (index as u8).wrapping_mul(0x21) ^ (lane as u8).wrapping_mul(0x0b) ^ 0x5a
    })
}

/// An x87 state of an x87 program in the middle of work: TOP = 5, mixed tags, a non-default control word,
/// flags and condition codes in the status word, instruction and data pointers, and in every data
/// register a recognisable value with a normal exponent 0x3ff0 + n (so the all-ones exponent of an MMX
/// write is visible).
pub fn x87_seed() -> X87State {
    let mut x87 = X87State::fixture_seed();
    for (index, register) in x87.registers.iter_mut().enumerate() {
        register[..8].copy_from_slice(&mmx_pattern(index));
        register[8..].copy_from_slice(&(0x3ff0 + index as u16).to_le_bytes());
    }
    x87.control_word = 0x027f;
    x87.status_word = 0x2d41;
    x87.tag_word = 0xa5a5;
    x87.instruction_pointer = 0x0123_4567_89ab_cdef;
    x87.data_pointer = 0xfedc_ba98_7654_3210;
    x87.last_opcode = 0x05aa;
    x87
}

pub fn xmm_pattern(index: usize) -> [u8; 16] {
    core::array::from_fn(|lane| {
        (index as u8).wrapping_mul(0x11) ^ (lane as u8).wrapping_mul(7) ^ 0x35
    })
}

/// Distinct GPRs, distinct XMM registers, a non-default MXCSR, a flag word with many bits set and the
/// x87 seed: any unwanted write is visible in a whole-state comparison.
pub fn cpu() -> LongModeState {
    let mut cpu = LongModeState {
        rip: CODE,
        rflags: FLAGS,
        x87: Some(x87_seed()),
        sse: Some(SseState {
            xmm: core::array::from_fn(xmm_pattern),
            mxcsr: MXCSR,
        }),
        ..LongModeState::default()
    };
    for (index, slot) in cpu.gpr.iter_mut().enumerate() {
        *slot = 0x0101_0101_0101_0101 * (index as u64 + 0x11);
    }
    cpu.gpr[od_isa::Register::Rax as usize] = DATA;
    cpu
}

/// The state that the FP-init CR0/CR4 profile pair admits plus the given control registers.
pub fn fp_init_cpu(cr0: u64, cr4: u64) -> LongModeState {
    let mut cpu = cpu();
    cpu.activity = Some(CpuActivity::Running);
    cpu.sti_shadow = Some(StiShadow::Clear);
    cpu.cr0 = Some(cr0);
    cpu.cr4 = Some(cr4);
    cpu
}

pub fn ctx(privilege: PrivilegeLevel) -> AccessContext {
    AccessContext {
        privilege,
        ..AccessContext::default()
    }
}

pub fn step(
    cpu: &mut LongModeState,
    ram: &mut Ram,
    privilege: PrivilegeLevel,
) -> Result<StepOutcome, MemoryError> {
    execute::step(cpu, ram, ctx(privilege))
}

/// One instruction under the FP-init CR0/CR4 profile pair (the control registers gate the instruction).
pub fn run_fp_init(mut cpu: LongModeState, ram: &mut Ram) -> (execute::RunResult, LongModeState) {
    let result = execute::run_with_cr4_profile(
        &mut cpu,
        ram,
        RunConfig {
            instruction_budget: 1,
            stop_rip: None,
            context: ctx(PrivilegeLevel::Ring0),
        },
        Cr4Profile::FpInitLong64V1,
        || false,
    );
    (result, cpu)
}

pub fn fault(vector: u8, error_code: Option<u32>) -> StepOutcome {
    StepOutcome::Fault(ArchitecturalFault {
        vector,
        error_code,
        linear_address: None,
    })
}

pub fn ud() -> StepOutcome {
    fault(6, None)
}
pub fn nm() -> StepOutcome {
    fault(7, None)
}
pub fn mf() -> StepOutcome {
    fault(16, None)
}
pub fn gp0() -> StepOutcome {
    fault(13, Some(0))
}
pub fn ss0() -> StepOutcome {
    fault(12, Some(0))
}

pub struct Kind {
    pub name: &'static str,
    pub kind: Ssse3IntKind,
    /// `0F 38 xx` or `0F 3A 0F`; in the MMX form nothing precedes it.
    pub opcode: [u8; 3],
}

impl Kind {
    pub fn immediate(&self) -> bool {
        self.kind == Ssse3IntKind::Palignr
    }
}

macro_rules! kind {
    ($name:literal, $kind:ident, $map:literal, $byte:literal) => {
        Kind {
            name: $name,
            kind: Ssse3IntKind::$kind,
            opcode: [0x0f, $map, $byte],
        }
    };
}

pub const KINDS: [Kind; 16] = [
    kind!("pshufb", Pshufb, 0x38, 0x00),
    kind!("phaddw", Phaddw, 0x38, 0x01),
    kind!("phaddd", Phaddd, 0x38, 0x02),
    kind!("phaddsw", Phaddsw, 0x38, 0x03),
    kind!("pmaddubsw", Pmaddubsw, 0x38, 0x04),
    kind!("phsubw", Phsubw, 0x38, 0x05),
    kind!("phsubd", Phsubd, 0x38, 0x06),
    kind!("phsubsw", Phsubsw, 0x38, 0x07),
    kind!("psignb", Psignb, 0x38, 0x08),
    kind!("psignw", Psignw, 0x38, 0x09),
    kind!("psignd", Psignd, 0x38, 0x0a),
    kind!("pmulhrsw", Pmulhrsw, 0x38, 0x0b),
    kind!("pabsb", Pabsb, 0x38, 0x1c),
    kind!("pabsw", Pabsw, 0x38, 0x1d),
    kind!("pabsd", Pabsd, 0x38, 0x1e),
    kind!("palignr", Palignr, 0x3a, 0x0f),
];

pub fn kind_named(name: &str) -> &'static Kind {
    KINDS.iter().find(|kind| kind.name == name).unwrap()
}

/// `0F 38 xx /r` (or `0F 3A 0F /r ib`) with a register source (mod = 3): MMX `destination`, `source`.
pub fn reg_code(kind: &Kind, destination: u8, source: u8, immediate: u8) -> Vec<u8> {
    let mut code = kind.opcode.to_vec();
    code.push(0xc0 | (destination << 3) | source);
    if kind.immediate() {
        code.push(immediate);
    }
    code
}

/// `0F 38 xx /r` with the memory source `[rax]`.
pub fn mem_code(kind: &Kind, destination: u8, immediate: u8) -> Vec<u8> {
    let mut code = kind.opcode.to_vec();
    code.push(destination << 3);
    if kind.immediate() {
        code.push(immediate);
    }
    code
}

/// The low 64 bits of physical register `index` (MMn).
pub fn mm(cpu: &LongModeState, index: usize) -> u64 {
    let raw = &cpu.x87.as_ref().unwrap().registers[index];
    u64::from_le_bytes(raw[..8].try_into().unwrap())
}

/// Bits 79:64 of physical register `index`.
pub fn exponent(cpu: &LongModeState, index: usize) -> u16 {
    let raw = &cpu.x87.as_ref().unwrap().registers[index];
    u16::from_le_bytes([raw[8], raw[9]])
}

pub fn set_mm(cpu: &mut LongModeState, index: usize, value: u64) {
    cpu.x87.as_mut().unwrap().registers[index][..8].copy_from_slice(&value.to_le_bytes());
}

/// SplitMix64: deterministic data for the generated matrices.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A 64-bit value of one of five patterns: random, extreme bytes, extreme words, extreme dwords, and
    /// PSHUFB control bytes (bits 7 and 2:0 only).
    pub fn vector(&mut self, mode: usize) -> u64 {
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
        let raw = self.next();
        match mode % 5 {
            1 => u64::from_le_bytes(raw.to_le_bytes().map(|b| T1[usize::from(b) % 10])),
            2 => (0..4).fold(0, |acc, i| {
                acc | u64::from(T2[((raw >> (16 * i)) as usize & 0xffff) % 12]) << (16 * i)
            }),
            3 => (0..2).fold(0, |acc, i| {
                acc | u64::from(T3[((raw >> (32 * i)) as usize & 0xffff_ffff) % 8]) << (32 * i)
            }),
            4 => raw & 0x8f8f_8f8f_8f8f_8f8f,
            _ => raw,
        }
    }
}
