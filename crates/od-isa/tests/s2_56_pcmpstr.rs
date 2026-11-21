//! S2-56 slice B: PCMPESTRM, PCMPESTRI, PCMPISTRM and PCMPISTRI (SSE4.2 string compares).
//!
//! Every expectation comes from the Intel SDM text read for this task (combined SDM 325462-093US,
//! cdrdv2.intel.com/v1/dl/getContent/671200, Vol. 2A/2B, text through pdftotext):
//!
//! * Section 4.1 "Imm8 Control Byte Operation for PCMPESTRI / PCMPESTRM / PCMPISTRI / PCMPISTRM" (Vol. 2B
//!   pp. 4-1..4-4): Table 4-1 source data format (imm8[1:0]: unsigned bytes, unsigned words, signed bytes,
//!   signed words), Table 4-2 and 4-3 aggregation (imm8[3:2]: equal any, ranges, equal each, equal ordered, with
//!   the pseudocode of IntRes1), Table 4-4 polarity (imm8[5:4]: positive, negative, masked positive, masked
//!   negative), Tables 4-5 and 4-6 output selection (imm8[6]: least or most significant index; bit mask or
//!   byte/word mask), Table 4-7 valid/invalid override of BoolRes[i, j], Table 4-8 summary (imm8[7] has no
//!   effect).
//! * PCMPESTRI "66 0F 3A 61 /r imm8" (pp. 4-254..4-255), PCMPESTRM "66 0F 3A 60 /r imm8" (pp. 4-256..4-257),
//!   PCMPISTRI "66 0F 3A 63 /r imm8" (pp. 4-267..4-268), PCMPISTRM "66 0F 3A 62 /r imm8" (pp. 4-269..4-270),
//!   operands ModRM:reg (xmm1) and ModRM:r/m (xmm2/m128). The explicit forms take the lengths from EAX (xmm1)
//!   and EDX (xmm2/m128), from RAX and RDX with REX.W ("Effective Operand Size"), as the absolute value
//!   saturated to 16 (bytes) or 8 (words). The implicit forms treat an element as valid only if its index is
//!   lower than the least significant null byte/word. The index goes to ECX (16, or 8 for words, if IntRes2 is
//!   zero), the mask to XMM0. Flags: CF = IntRes2 != 0, ZF = (xmm2/m128 length < 16 (8): "absolute-value of
//!   EDX < 16 (8)" or "any byte/word of xmm2/mem128 is null"), SF = the same for xmm1, OF = IntRes2[0], AF
//!   and PF reset.
//! * "Other Exceptions: See Table 2-21, Type 4 Class Exception Conditions, additionally, this instruction does
//!   not cause #GP if the memory operand is not aligned to 16 Byte boundary" (Vol. 2A Table 2-21 note 1):
//!   #UD for CR0.EM = 1, CR4.OSFXSR = 0 or a LOCK prefix, #NM for CR0.TS = 1, #SS(0)/#GP(0) for a
//!   non-canonical address, #PF; no #AC in Type 4 and no SIMD floating-point exception.
//!
//! The expected values of the sweeps come from `sdm_reference`, a literal transcription of Tables 4-3, 4-4,
//! 4-5/4-6 and 4-7 on arrays of booleans, which shares no code with the model. `GOLDEN` holds 224 vectors with
//! explicit XMM data that were run on the physical oracle (Intel Core i5-3210M, work/stage8/s2_56b_oracle:
//! 28460 random and boundary cases, 28068 matched; the differences are forms the model refuses: prefix
//! combinations, FS/GS, the 2^64 wrap of an operand address, and the oracle program's own r11).
//!
//! Not asserted here because the SDM text does not define it or the model does not carry it: the CPUID gates
//! (SSE4.2: S2-57 owns CPUID), the CPL3 refusal of a profile set (`cpl3_profile_set.rs`), the VEX forms.

use od_isa::execute::{self, Cr4Profile, RunConfig, RunExit, StepOutcome, run_with_rflags_defined};
use od_isa::long_decode::{self, Operation, Sse41Kind, Sse41Operand};
use od_isa::state::{SseState, X87State};
use od_isa::{
    AccessContext, AccessKind, ArchitecturalFault, CpuActivity, GuestMemory, LongModeState,
    MemoryError, PrivilegeLevel, Register, StiShadow,
};

const CODE: u64 = 0x2000;
const RAM: usize = 0x10000;
const DATA: u64 = 0x8000;
const SENTINEL: u8 = 0xcc;
const CF: u64 = 1;
const PF: u64 = 1 << 2;
const AF: u64 = 1 << 4;
const ZF: u64 = 1 << 6;
const SF: u64 = 1 << 7;
const OF: u64 = 1 << 11;
const DF: u64 = 1 << 10;
const IF: u64 = 1 << 9;
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

    fn put(&mut self, address: u64, data: &[u8]) {
        self.bytes[address as usize..address as usize + data.len()].copy_from_slice(data);
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

    fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound
    }
}

fn xmm_pattern(index: usize) -> [u8; 16] {
    core::array::from_fn(|lane| {
        (index as u8).wrapping_mul(0x11) ^ (lane as u8).wrapping_mul(7) ^ 0x35
    })
}

/// Distinct GPRs, distinct XMM registers, a non-default MXCSR and a flag word with many bits set: any unwanted
/// write is visible in a whole-state comparison.
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

fn exec(
    mut cpu: LongModeState,
    ram: &mut Ram,
) -> (Result<StepOutcome, MemoryError>, LongModeState) {
    let outcome = execute::step(&mut cpu, ram, ctx(PrivilegeLevel::Ring0));
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

// ---- the four instructions ----

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    Estrm,
    Estri,
    Istrm,
    Istri,
}

const KINDS: [Kind; 4] = [Kind::Estrm, Kind::Estri, Kind::Istrm, Kind::Istri];

impl Kind {
    fn opcode(self) -> u8 {
        match self {
            Kind::Estrm => 0x60,
            Kind::Estri => 0x61,
            Kind::Istrm => 0x62,
            Kind::Istri => 0x63,
        }
    }

    fn explicit(self) -> bool {
        matches!(self, Kind::Estrm | Kind::Estri)
    }

    fn index_form(self) -> bool {
        matches!(self, Kind::Estri | Kind::Istri)
    }
}

/// `66 [REX] 0F 3A op /r imm8` with the register form (xmm1 = ModRM.reg, xmm2 = ModRM.r/m).
fn reg_code(kind: Kind, rex_w: bool, a: u8, b: u8, imm: u8) -> Vec<u8> {
    let rex = u8::from(rex_w) << 3 | (a >> 3) << 2 | (b >> 3);
    let mut code = vec![0x66];
    if rex != 0 {
        code.push(0x40 | rex);
    }
    code.extend([
        0x0f,
        0x3a,
        kind.opcode(),
        0xc0 | (a & 7) << 3 | (b & 7),
        imm,
    ]);
    code
}

/// One memory addressing form: the prefix and REX.B bits, the ModRM byte with reg = 0, the SIB and
/// displacement bytes, and how the registers are set so that the effective address is `ea`.
struct Form {
    name: &'static str,
    legacy: &'static [u8],
    rex_b: u8,
    modrm: u8,
    tail: &'static [u8],
    setup: fn(&mut LongModeState, u64),
}

const FORMS: &[Form] = &[
    Form {
        name: "[rbx]",
        legacy: &[],
        rex_b: 0,
        modrm: 0x03,
        tail: &[],
        setup: |cpu, ea| cpu.gpr[Register::Rbx as usize] = ea,
    },
    Form {
        name: "[rbx+0x10]",
        legacy: &[],
        rex_b: 0,
        modrm: 0x43,
        tail: &[0x10],
        setup: |cpu, ea| cpu.gpr[Register::Rbx as usize] = ea - 0x10,
    },
    Form {
        name: "[rsp]",
        legacy: &[],
        rex_b: 0,
        modrm: 0x04,
        tail: &[0x24],
        setup: |cpu, ea| cpu.gpr[Register::Rsp as usize] = ea,
    },
    Form {
        name: "[rbp-0x20]",
        legacy: &[],
        rex_b: 0,
        modrm: 0x45,
        tail: &[0xe0],
        setup: |cpu, ea| cpu.gpr[Register::Rbp as usize] = ea + 0x20,
    },
    Form {
        name: "[rbx+rsi*4+0x100]",
        legacy: &[],
        rex_b: 0,
        modrm: 0x84,
        tail: &[0xb3, 0x00, 0x01, 0x00, 0x00],
        setup: |cpu, ea| {
            cpu.gpr[Register::Rsi as usize] = 3;
            cpu.gpr[Register::Rbx as usize] = ea - 0x100 - 12;
        },
    },
    Form {
        name: "[r8]",
        legacy: &[],
        rex_b: 1,
        modrm: 0x00,
        tail: &[],
        setup: |cpu, ea| cpu.gpr[Register::R8 as usize] = ea,
    },
    Form {
        name: "[r13+0]",
        legacy: &[],
        rex_b: 1,
        modrm: 0x45,
        tail: &[0x00],
        setup: |cpu, ea| cpu.gpr[Register::R13 as usize] = ea,
    },
    Form {
        name: "[r12]",
        legacy: &[],
        rex_b: 1,
        modrm: 0x04,
        tail: &[0x24],
        setup: |cpu, ea| cpu.gpr[Register::R12 as usize] = ea,
    },
    Form {
        name: "[ebx] (67)",
        legacy: &[0x67],
        rex_b: 0,
        modrm: 0x03,
        tail: &[],
        setup: |cpu, ea| cpu.gpr[Register::Rbx as usize] = 0xffff_ffff_0000_0000 | ea,
    },
    Form {
        name: "[ds:rbx]",
        legacy: &[0x3e],
        rex_b: 0,
        modrm: 0x03,
        tail: &[],
        setup: |cpu, ea| cpu.gpr[Register::Rbx as usize] = ea,
    },
];

/// `66 [67] [REX] 0F 3A op modrm[+sib+disp] imm8` with the memory form; the RIP-relative form is built apart.
fn mem_code(kind: Kind, rex_w: bool, a: u8, form: &Form, imm: u8) -> Vec<u8> {
    let rex = u8::from(rex_w) << 3 | (a >> 3) << 2 | form.rex_b;
    let mut code = vec![0x66];
    code.extend_from_slice(form.legacy);
    if rex != 0 {
        code.push(0x40 | rex);
    }
    code.extend([0x0f, 0x3a, kind.opcode(), form.modrm | (a & 7) << 3]);
    code.extend_from_slice(form.tail);
    code.push(imm);
    code
}

/// `66 0F 3A op /r imm8` with `[rip+disp32]` addressing `ea`.
fn rip_code(kind: Kind, rex_w: bool, a: u8, ea: u64, imm: u8) -> Vec<u8> {
    let rex = u8::from(rex_w) << 3 | (a >> 3) << 2;
    let mut code = vec![0x66];
    if rex != 0 {
        code.push(0x40 | rex);
    }
    code.extend([0x0f, 0x3a, kind.opcode(), 0x05 | (a & 7) << 3]);
    let end = CODE + code.len() as u64 + 4 + 1;
    code.extend(((ea.wrapping_sub(end)) as u32).to_le_bytes());
    code.push(imm);
    code
}

// ---- the reference: a transcription of SDM Vol. 2B Section 4.1 ----

struct Expected {
    index: u32,
    mask: [u8; 16],
    flags: u64,
}

/// `lengths` is `Some((EAX or RAX, EDX or RDX))` (sign-extended) for the explicit forms.
fn sdm_reference(a: [u8; 16], b: [u8; 16], imm: u8, lengths: Option<(i64, i64)>) -> Expected {
    let word = imm & 1 != 0;
    let signed = imm & 2 != 0;
    let upper_bound: usize = if word { 7 } else { 15 };
    let count = upper_bound + 1;
    let unsigned_element = |x: &[u8; 16], i: usize| -> i64 {
        if word {
            i64::from(u16::from_le_bytes([x[2 * i], x[2 * i + 1]]))
        } else {
            i64::from(x[i])
        }
    };
    let element = |x: &[u8; 16], i: usize| -> i64 {
        let value = unsigned_element(x, i);
        match (word, signed) {
            (true, true) => i64::from(value as u16 as i16),
            (false, true) => i64::from(value as u8 as i8),
            _ => value,
        }
    };
    // Valid elements: a length register (absolute value, saturated), or the elements before the first null.
    let (len_a, len_b) = match lengths {
        Some((la, lb)) => {
            let saturate = |v: i64| (i128::from(v).abs().min(count as i128)) as usize;
            (saturate(la), saturate(lb))
        }
        None => {
            let before_null = |x: &[u8; 16]| {
                (0..count)
                    .find(|&i| unsigned_element(x, i) == 0)
                    .unwrap_or(count)
            };
            (before_null(&a), before_null(&b))
        }
    };
    let valid_a = |i: usize| i < len_a;
    let valid_b = |j: usize| j < len_b;
    let mode = (imm >> 2) & 3;
    // BoolRes[j, i]: j indexes xmm2/m128, i indexes xmm1; Table 4-7 overrides it for invalid elements.
    let mut bool_res = [[false; 16]; 16];
    for j in 0..count {
        for i in 0..count {
            let compared = if mode == 1 {
                if i % 2 == 0 {
                    element(&b, j) >= element(&a, i)
                } else {
                    element(&b, j) <= element(&a, i)
                }
            } else {
                element(&b, j) == element(&a, i)
            };
            bool_res[j][i] = match (valid_a(i), valid_b(j)) {
                (true, true) => compared,
                (true, false) => false,
                (false, true) => mode == 3,
                (false, false) => mode == 2 || mode == 3,
            };
        }
    }
    let mut int_res1 = [false; 16];
    match mode {
        0 => {
            for j in 0..=upper_bound {
                for i in 0..=upper_bound {
                    int_res1[j] |= bool_res[j][i];
                }
            }
        }
        1 => {
            for j in 0..=upper_bound {
                let mut i = 0;
                while i <= upper_bound {
                    int_res1[j] |= bool_res[j][i] && bool_res[j][i + 1];
                    i += 2;
                }
            }
        }
        2 => {
            for i in 0..=upper_bound {
                int_res1[i] = bool_res[i][i];
            }
        }
        _ => {
            for j in 0..=upper_bound {
                int_res1[j] = true;
                let (mut i, mut k) = (0, j);
                while i <= upper_bound - j {
                    int_res1[j] &= bool_res[k][i];
                    i += 1;
                    k += 1;
                }
            }
        }
    }
    let mut int_res2 = [false; 16];
    for i in 0..=upper_bound {
        int_res2[i] = match (imm >> 4) & 3 {
            0 | 2 => int_res1[i],
            1 => !int_res1[i],
            _ => {
                if valid_b(i) {
                    !int_res1[i]
                } else {
                    int_res1[i]
                }
            }
        };
    }
    let set_bits: Vec<usize> = (0..=upper_bound).filter(|&i| int_res2[i]).collect();
    let index = if imm & 0x40 == 0 {
        set_bits.first().copied()
    } else {
        set_bits.last().copied()
    }
    .unwrap_or(count) as u32;
    let mut mask = [0u8; 16];
    if imm & 0x40 == 0 {
        for &i in &set_bits {
            mask[i / 8] |= 1 << (i % 8);
        }
    } else {
        for &i in &set_bits {
            if word {
                mask[2 * i] = 0xff;
                mask[2 * i + 1] = 0xff;
            } else {
                mask[i] = 0xff;
            }
        }
    }
    let mut flags = 0;
    if !set_bits.is_empty() {
        flags |= CF;
    }
    if len_b < count {
        flags |= ZF;
    }
    if len_a < count {
        flags |= SF;
    }
    if int_res2[0] {
        flags |= OF;
    }
    Expected { index, mask, flags }
}

/// The state after the instruction retires, from the reference.
fn expected_state(
    start: &LongModeState,
    kind: Kind,
    rex_w: bool,
    (a, b): ([u8; 16], [u8; 16]),
    imm: u8,
    length: usize,
) -> LongModeState {
    let lengths = kind.explicit().then(|| {
        let (rax, rdx) = (
            start.gpr[Register::Rax as usize],
            start.gpr[Register::Rdx as usize],
        );
        if rex_w {
            (rax as i64, rdx as i64)
        } else {
            (i64::from(rax as u32 as i32), i64::from(rdx as u32 as i32))
        }
    });
    let expected = sdm_reference(a, b, imm, lengths);
    let mut state = start.clone();
    if kind.index_form() {
        state.gpr[Register::Rcx as usize] = u64::from(expected.index);
    } else {
        state.sse.as_mut().unwrap().xmm[0] = expected.mask;
    }
    state.rflags = (start.rflags & !STATUS) | expected.flags;
    state.rip = CODE + length as u64;
    state
}

/// A string of up to 16 bytes (8 words): a few valid elements from a small alphabet, a null, then garbage.
fn string_data(rng: &mut Rng, word: bool) -> [u8; 16] {
    const BYTES: [u8; 8] = [0x41, 0x42, 0x43, 0x80, 0xff, 0x7f, 0x01, 0xfe];
    const WORDS: [u16; 8] = [0x41, 0x42, 0x4142, 0x8000, 0xffff, 0x7fff, 0x100, 0x1];
    let count = if word { 8 } else { 16 };
    let valid = rng.below(count as u64 + 1) as usize;
    let mut data = [0u8; 16];
    for i in 0..count {
        if word {
            let value = if i < valid {
                WORDS[rng.below(8) as usize]
            } else if rng.below(2) == 0 {
                0
            } else {
                rng.next() as u16
            };
            data[2 * i..2 * i + 2].copy_from_slice(&value.to_le_bytes());
        } else {
            data[i] = if i < valid {
                BYTES[rng.below(8) as usize]
            } else if rng.below(2) == 0 {
                0
            } else {
                rng.next() as u8
            };
        }
    }
    data
}

const LENGTHS: [u64; 40] = [
    0,
    1,
    2,
    3,
    4,
    5,
    6,
    7,
    8,
    9,
    10,
    15,
    16,
    17,
    31,
    32,
    0xffff_ffff_ffff_ffff,
    0xffff_ffff_ffff_fffe,
    0xffff_ffff_ffff_fff9,
    0xffff_ffff_ffff_fff8,
    0xffff_ffff_ffff_fff7,
    0xffff_ffff_ffff_fff1,
    0xffff_ffff_ffff_fff0,
    0xffff_ffff_ffff_ffef,
    0x7fff_ffff,
    0x8000_0000,
    0xffff_ffff,
    0xffff_ffff_8000_0000,
    0x1_0000_0000,
    0x1_0000_0003,
    0xffff_ffff_0000_0005,
    0xdead_beef_0000_0008,
    0x8000_0000_0000_0000,
    0x7fff_ffff_ffff_ffff,
    0xffff_fff0,
    0xffff_fff8,
    0xffff_fffd,
    0x8000_0000_0000_0001,
    255,
    256,
];

#[test]
fn known_answers_from_the_sdm_description() {
    fn text(s: &str) -> [u8; 16] {
        let mut data = [0u8; 16];
        data[..s.len()].copy_from_slice(s.as_bytes());
        data
    }
    // (kind, imm8, xmm1, xmm2, RAX, RDX, ECX if an index form, XMM0 if a mask form, flags)
    let rows: Vec<(Kind, u8, [u8; 16], [u8; 16], u64, u64, u32, [u8; 16], u64)> = vec![
        // Equal any, the set "aeiou" in the text "xyzebcd": the only match is the 'e' at index 3.
        (
            Kind::Istri,
            0x00,
            text("aeiou"),
            text("xyzebcd"),
            0,
            0,
            3,
            [0; 16],
            CF | ZF | SF,
        ),
        // No match: the index is 16; CF is reset, both strings end inside the register.
        (
            Kind::Istri,
            0x00,
            text("xyz"),
            text("hello world"),
            0,
            0,
            16,
            [0; 16],
            ZF | SF,
        ),
        // strcmp: equal each with negative polarity of two equal strings gives no difference, index 16.
        (
            Kind::Istri,
            0x18,
            text("hello"),
            text("hello"),
            0,
            0,
            16,
            [0; 16],
            ZF | SF,
        ),
        // ... and the first difference is at index 4 (OF = IntRes2[0] = 0).
        (
            Kind::Istri,
            0x18,
            text("hello"),
            text("hellp"),
            0,
            0,
            4,
            [0; 16],
            CF | ZF | SF,
        ),
        // Equal ordered: the substring "ab" is at 2 and 6; imm8[6] selects the most significant index.
        (
            Kind::Istri,
            0x0c,
            text("ab"),
            text("xxabxxab"),
            0,
            0,
            2,
            [0; 16],
            CF | ZF | SF,
        ),
        (
            Kind::Istri,
            0x4c,
            text("ab"),
            text("xxabxxab"),
            0,
            0,
            6,
            [0; 16],
            CF | ZF | SF,
        ),
        // Ranges 'a'..'z' over "Hello World": a 16-bit mask 0b0111_1001_1110 in XMM0.
        (
            Kind::Istrm,
            0x04,
            text("az"),
            text("Hello World"),
            0,
            0,
            0,
            mask16(0x079e),
            CF | ZF | SF,
        ),
        // The same result as a byte mask (imm8[6]).
        (
            Kind::Istrm,
            0x44,
            text("az"),
            text("Hello World"),
            0,
            0,
            0,
            byte_mask(0x079e),
            CF | ZF | SF,
        ),
        // Explicit lengths: the absolute value saturates at 16, so |17| and |-100| mean "all 16 valid".
        (
            Kind::Estri,
            0x00,
            text("abcdefghijklmnop"),
            text("ponmlkjihgfedcba"),
            17,
            (-100i64) as u64,
            0,
            [0; 16],
            CF | OF,
        ),
        // Word strings: the length saturates at 8; a length of 9 leaves ZF and SF clear.
        (
            Kind::Estri,
            0x01,
            words(&[1, 2, 3, 4, 5, 6, 7, 8]),
            words(&[8, 7, 6, 5, 4, 3, 2, 1]),
            9,
            9,
            0,
            [0; 16],
            CF | OF,
        ),
    ];
    for (n, (kind, imm, a, b, rax, rdx, ecx, xmm0, flags)) in rows.into_iter().enumerate() {
        let mut start = cpu();
        start.gpr[Register::Rax as usize] = rax;
        start.gpr[Register::Rdx as usize] = rdx;
        start.sse.as_mut().unwrap().xmm[1] = a;
        start.sse.as_mut().unwrap().xmm[2] = b;
        let code = reg_code(kind, false, 1, 2, imm);
        let mut ram = Ram::new(&code);
        let (outcome, end) = exec(start.clone(), &mut ram);
        assert_eq!(outcome, Ok(StepOutcome::Advanced), "row {n}");
        if kind.index_form() {
            assert_eq!(
                end.gpr[Register::Rcx as usize],
                u64::from(ecx),
                "row {n} ECX"
            );
        } else {
            assert_eq!(end.sse.as_ref().unwrap().xmm[0], xmm0, "row {n} XMM0");
        }
        assert_eq!(end.rflags & STATUS, flags, "row {n} flags");
        assert_eq!(end.rflags & !STATUS, start.rflags & !STATUS, "row {n}");
    }
}

fn mask16(bits: u16) -> [u8; 16] {
    let mut data = [0u8; 16];
    data[..2].copy_from_slice(&bits.to_le_bytes());
    data
}

fn byte_mask(bits: u16) -> [u8; 16] {
    core::array::from_fn(|i| if bits >> i & 1 != 0 { 0xff } else { 0 })
}

fn words(values: &[u16]) -> [u8; 16] {
    let mut data = [0u8; 16];
    for (i, value) in values.iter().enumerate() {
        data[2 * i..2 * i + 2].copy_from_slice(&value.to_le_bytes());
    }
    data
}

#[test]
fn every_imm8_of_every_instruction_matches_the_sdm_reference_in_register_forms() {
    let mut rng = Rng(0x56b0_0001);
    let mut cases = 0u32;
    for kind in KINDS {
        for rex_w in [false, true] {
            for imm in 0..=255u8 {
                for round in 0..6 {
                    let word = imm & 1 != 0;
                    let (a_reg, b_reg) = (rng.below(16) as u8, rng.below(16) as u8);
                    let mut start = cpu();
                    let a = string_data(&mut rng, word);
                    let b = if round == 5 {
                        a
                    } else {
                        string_data(&mut rng, word)
                    };
                    let sse = start.sse.as_mut().unwrap();
                    sse.xmm[usize::from(a_reg)] = a;
                    sse.xmm[usize::from(b_reg)] = b;
                    // Two registers that are the same: the later write wins and both strings are equal.
                    let (a, b) = (sse.xmm[usize::from(a_reg)], sse.xmm[usize::from(b_reg)]);
                    if kind.explicit() {
                        let pick = |rng: &mut Rng| {
                            if round % 3 == 2 {
                                LENGTHS[rng.below(LENGTHS.len() as u64) as usize]
                            } else {
                                rng.below(19)
                                    .wrapping_sub(if round & 1 == 0 { 0 } else { 9 })
                            }
                        };
                        start.gpr[Register::Rax as usize] = pick(&mut rng);
                        start.gpr[Register::Rdx as usize] = pick(&mut rng);
                    }
                    let code = reg_code(kind, rex_w, a_reg, b_reg, imm);
                    let mut ram = Ram::new(&code);
                    let (outcome, end) = exec(start.clone(), &mut ram);
                    assert_eq!(
                        outcome,
                        Ok(StepOutcome::Advanced),
                        "{kind:?} {imm:#04x} {code:02x?}"
                    );
                    let expected = expected_state(&start, kind, rex_w, (a, b), imm, code.len());
                    assert_eq!(
                        end, expected,
                        "{kind:?} rex.w={rex_w} imm={imm:#04x} round {round} {code:02x?}"
                    );
                    assert!(ram.reads.is_empty() && ram.writes.is_empty());
                    cases += 1;
                }
            }
        }
    }
    assert_eq!(cases, 4 * 2 * 256 * 6);
}

#[test]
fn explicit_lengths_use_the_absolute_value_saturated_to_the_element_count_and_rex_w_selects_64_bits()
 {
    // Equal any with all-match: IntRes1 is the valid prefix of xmm2/m128 (every element of b is in a).
    for imm in [0x00u8, 0x01] {
        let count: u64 = if imm & 1 != 0 { 8 } else { 16 };
        let (a, b) = if imm & 1 != 0 {
            (
                words(&[1, 2, 3, 4, 5, 6, 7, 8]),
                words(&[8, 7, 6, 5, 4, 3, 2, 1]),
            )
        } else {
            (*b"abcdefghijklmnop", *b"ponmlkjihgfedcba")
        };
        for (rax, rdx, rex_w, valid_a, valid_b) in [
            (0u64, 0u64, false, 0u64, 0u64),
            (3, 5, false, 3, 5),
            ((-3i64) as u64, (-5i64) as u64, false, 3, 5),
            (16, 16, false, 16.min(count), 16.min(count)),
            (17, 17, false, count, count),
            (
                (-17i64) as u64,
                (-16i64) as u64,
                false,
                count,
                16.min(count),
            ),
            (0x8000_0000, 0x8000_0000, false, count, count), // INT_MIN saturates
            (0x7fff_ffff, 0x7fff_ffff, false, count, count),
            (0x1_0000_0003, 0x1_0000_0005, false, 3, 5), // the high halves are ignored without REX.W
            (0x1_0000_0003, 0x1_0000_0005, true, count, count), // and count as large positive values with it
            (0xffff_ffff_0000_0003, 0xffff_ffff_0000_0005, false, 3, 5),
            (
                0xffff_ffff_0000_0003,
                0xffff_ffff_0000_0005,
                true,
                count,
                count,
            ),
            (
                0x8000_0000_0000_0000,
                0x8000_0000_0000_0001,
                true,
                count,
                count,
            ),
            ((-3i64) as u64, (-5i64) as u64, true, 3, 5),
        ] {
            let mut start = cpu();
            start.gpr[Register::Rax as usize] = rax;
            start.gpr[Register::Rdx as usize] = rdx;
            start.sse.as_mut().unwrap().xmm[1] = a;
            start.sse.as_mut().unwrap().xmm[2] = b;
            let code = reg_code(Kind::Estrm, rex_w, 1, 2, imm);
            let mut ram = Ram::new(&code);
            let (outcome, end) = exec(start.clone(), &mut ram);
            assert_eq!(outcome, Ok(StepOutcome::Advanced));
            // ZF = |EDX| < count, SF = |EAX| < count, and the bit mask has the valid-b prefix set when a is full.
            let mut flags = 0;
            if valid_b < count {
                flags |= ZF;
            }
            if valid_a < count {
                flags |= SF;
            }
            if valid_a == count && valid_b != 0 {
                flags |= CF | OF;
            }
            assert_eq!(
                end.rflags & STATUS,
                flags,
                "{rax:#x} {rdx:#x} rex.w={rex_w} imm={imm}"
            );
            if valid_a == count {
                assert_eq!(
                    end.sse.as_ref().unwrap().xmm[0],
                    mask16(((1u32 << valid_b) - 1) as u16)
                );
            }
        }
    }
}

#[test]
fn implicit_forms_ignore_eax_and_edx_and_a_null_ends_the_string() {
    let mut rng = Rng(0x56b0_0002);
    for imm in [0x00u8, 0x0c, 0x18, 0x44, 0x01, 0x0d] {
        let word = imm & 1 != 0;
        for _ in 0..40 {
            let (a, b) = (string_data(&mut rng, word), string_data(&mut rng, word));
            let mut results = Vec::new();
            for lengths in [
                (0u64, 0u64),
                (16, 16),
                (u64::MAX, 3),
                (0x8000_0000, 0x1_0000_0000),
            ] {
                let mut start = cpu();
                start.gpr[Register::Rax as usize] = lengths.0;
                start.gpr[Register::Rdx as usize] = lengths.1;
                start.sse.as_mut().unwrap().xmm[1] = a;
                start.sse.as_mut().unwrap().xmm[2] = b;
                let code = reg_code(Kind::Istri, false, 1, 2, imm);
                let mut ram = Ram::new(&code);
                let (outcome, end) = exec(start, &mut ram);
                assert_eq!(outcome, Ok(StepOutcome::Advanced));
                results.push((end.gpr[Register::Rcx as usize], end.rflags));
            }
            assert!(
                results.windows(2).all(|pair| pair[0] == pair[1]),
                "{imm:#x} {results:?}"
            );
        }
    }
}

#[test]
fn xmm0_as_a_source_is_read_before_the_mask_overwrites_it_and_ecx_zero_extends() {
    let mut start = cpu();
    start.gpr[Register::Rcx as usize] = 0xffff_ffff_ffff_ffff;
    let a = words(&[0x61, 0x62]);
    let b = words(&[0x62, 0x61, 0x63]);
    start.sse.as_mut().unwrap().xmm[0] = a;
    start.sse.as_mut().unwrap().xmm[3] = b;
    // PCMPISTRM xmm0, xmm3, 0x41 (equal any, words, byte/word mask)
    let code = reg_code(Kind::Istrm, false, 0, 3, 0x41);
    let mut ram = Ram::new(&code);
    let (outcome, end) = exec(start.clone(), &mut ram);
    assert_eq!(outcome, Ok(StepOutcome::Advanced));
    assert_eq!(
        end,
        expected_state(&start, Kind::Istrm, false, (a, b), 0x41, code.len())
    );
    assert_eq!(end.sse.as_ref().unwrap().xmm[0], words(&[0xffff, 0xffff]));
    assert_eq!(
        end.gpr[Register::Rcx as usize],
        0xffff_ffff_ffff_ffff,
        "the mask forms leave RCX alone"
    );
    // PCMPISTRI writes ECX and zero-extends it to RCX; the other registers are unchanged.
    let code = reg_code(Kind::Istri, false, 0, 3, 0x41);
    let mut ram = Ram::new(&code);
    let (_, end) = exec(start.clone(), &mut ram);
    // IntRes2 = 0b011 (b[0] and b[1] are in the set, b[2] is not), imm8[6] selects the most significant bit.
    assert_eq!(end.gpr[Register::Rcx as usize], 1);
    assert_eq!(
        end.sse.as_ref().unwrap().xmm[0],
        a,
        "the index forms leave XMM0 alone"
    );
}

#[test]
fn memory_forms_read_sixteen_bytes_once_at_any_alignment_and_match_the_register_forms() {
    let mut rng = Rng(0x56b0_0003);
    for kind in KINDS {
        for (fi, form) in FORMS.iter().enumerate() {
            for delta in [0u64, 1, 3, 7, 8, 15] {
                let ea = DATA + 0x40 + delta;
                for round in 0..4 {
                    let imm = rng.below(256) as u8;
                    let rex_w = round & 1 == 1;
                    let word = imm & 1 != 0;
                    let a_reg = ((fi + round) % 16) as u8;
                    let mut start = cpu();
                    let (a, b) = (string_data(&mut rng, word), string_data(&mut rng, word));
                    start.sse.as_mut().unwrap().xmm[usize::from(a_reg)] = a;
                    if kind.explicit() {
                        start.gpr[Register::Rax as usize] = rng.below(20);
                        start.gpr[Register::Rdx as usize] = rng.below(20);
                    }
                    (form.setup)(&mut start, ea);
                    let code = mem_code(kind, rex_w, a_reg, form, imm);
                    let mut ram = Ram::new(&code);
                    ram.put(ea, &b);
                    let (outcome, end) = exec(start.clone(), &mut ram);
                    assert_eq!(
                        outcome,
                        Ok(StepOutcome::Advanced),
                        "{kind:?} {} {delta}",
                        form.name
                    );
                    assert_eq!(ram.reads, vec![(ea, 16)], "{kind:?} {} {delta}", form.name);
                    assert!(ram.writes.is_empty());
                    let expected = expected_state(&start, kind, rex_w, (a, b), imm, code.len());
                    assert_eq!(
                        end, expected,
                        "{kind:?} {} delta {delta} imm {imm:#x}",
                        form.name
                    );
                }
            }
        }
        // RIP-relative: the operand is addressed from the end of the whole instruction (imm8 included).
        let ea = DATA + 0x91;
        let mut start = cpu();
        let (a, b) = (string_data(&mut rng, false), string_data(&mut rng, false));
        start.sse.as_mut().unwrap().xmm[9] = a;
        let code = rip_code(kind, false, 9, ea, 0x0c);
        let mut ram = Ram::new(&code);
        ram.put(ea, &b);
        let (outcome, end) = exec(start.clone(), &mut ram);
        assert_eq!(outcome, Ok(StepOutcome::Advanced), "{kind:?} rip");
        assert_eq!(ram.reads, vec![(ea, 16)]);
        assert_eq!(
            end,
            expected_state(&start, kind, false, (a, b), 0x0c, code.len()),
            "{kind:?} rip"
        );
    }
}

#[test]
fn memory_faults_are_raised_before_any_state_change() {
    for kind in KINDS {
        let code = mem_code(kind, false, 1, &FORMS[0], 0x0c);
        // Non-canonical addresses: #GP(0), or #SS(0) when the base is RSP or RBP; no memory access.
        for (base, tail, expected) in [
            (Register::Rbx, vec![0x03u8], gp0()),
            (Register::Rbp, vec![0x4d, 0x00], ss0()),
            (Register::Rsp, vec![0x0c, 0x24], ss0()),
        ] {
            for address in [
                0x0000_8000_0000_0000u64,
                0x0000_8000_0000_0001,
                0xffff_7fff_ffff_ffff,
                0x7fff_ffff_ffff_ffff,
            ] {
                let mut code = vec![0x66, 0x0f, 0x3a, kind.opcode()];
                code.extend_from_slice(&tail);
                code.push(0x00);
                let mut start = cpu();
                start.gpr[base as usize] = address;
                let mut ram = Ram::new(&code);
                let (outcome, end) = exec(start.clone(), &mut ram);
                assert_eq!(
                    outcome,
                    Ok(expected.clone()),
                    "{kind:?} {base:?} {address:#x}"
                );
                assert_eq!(end, start);
                assert!(ram.reads.is_empty() && ram.writes.is_empty());
            }
        }
        // A fault of the memory backend (#PF) is its own result and leaves the state untouched, also when the
        // 16 bytes straddle into the faulting page.
        for address in [0x9000u64, 0x8ff8, 0x8fff, 0x9ff1] {
            let mut start = cpu();
            start.gpr[Register::Rbx as usize] = address;
            let mut ram = Ram::new(&code);
            ram.fault_page = Some(0x9000);
            let (outcome, end) = exec(start.clone(), &mut ram);
            let touches_page = address + 16 > 0x9000 && address < 0xa000;
            if touches_page {
                assert_eq!(
                    outcome,
                    Ok(StepOutcome::Fault(ArchitecturalFault {
                        vector: 14,
                        error_code: Some(0),
                        linear_address: Some(address.max(0x9000)),
                    })),
                    "{kind:?} {address:#x}"
                );
                assert_eq!(end, start);
            } else {
                assert_eq!(outcome, Ok(StepOutcome::Advanced));
            }
        }
        // An address range that wraps past 2^64 is a backend range error, not a guess.
        let mut start = cpu();
        start.gpr[Register::Rbx as usize] = u64::MAX - 3;
        let mut ram = Ram::new(&code);
        let (outcome, end) = exec(start.clone(), &mut ram);
        assert!(
            outcome.is_err() || matches!(outcome, Ok(StepOutcome::Fault(_))),
            "{kind:?} {outcome:?}"
        );
        assert_eq!(end, start);
    }
}

#[test]
fn lock_is_invalid_opcode_in_every_prefix_order() {
    for kind in KINDS {
        let op = kind.opcode();
        for code in [
            vec![0xf0, 0x66, 0x0f, 0x3a, op, 0xca, 0x00],
            vec![0x66, 0xf0, 0x0f, 0x3a, op, 0xca, 0x00],
            vec![0xf0, 0x66, 0x0f, 0x3a, op, 0x03, 0x00],
            vec![0x66, 0xf0, 0x48, 0x0f, 0x3a, op, 0x03, 0x00],
            vec![0x66, 0x44, 0xf0, 0x0f, 0x3a, op, 0xca, 0x00],
            vec![0xf0, 0x66, 0x4d, 0x0f, 0x3a, op, 0xca, 0x00],
        ] {
            let start = cpu();
            let mut ram = Ram::new(&code);
            let (outcome, end) = exec(start.clone(), &mut ram);
            assert_eq!(outcome, Ok(ud()), "{code:02x?}");
            assert_eq!(end, start);
            assert!(ram.reads.is_empty() && ram.writes.is_empty());
        }
    }
}

#[test]
fn the_cr0_cr4_gate_of_the_sse_forms_applies_under_the_fp_init_profile() {
    const CR0_BASE: u64 = 0x8001_0033; // PG | WP | NE | ET | MP | PE: EM = 0, TS = 0
    const CR4_PAE: u64 = 0x20;
    const CR4_OSFXSR: u64 = 0x200;
    for kind in KINDS {
        for (cr0, cr4, vector) in [
            (CR0_BASE, CR4_PAE | CR4_OSFXSR, None),
            (CR0_BASE | 4, CR4_PAE | CR4_OSFXSR, Some(6)),
            (CR0_BASE | 8, CR4_PAE | CR4_OSFXSR, Some(7)),
            (CR0_BASE | 4 | 8, CR4_PAE | CR4_OSFXSR, Some(6)),
            (CR0_BASE, CR4_PAE, Some(6)),
            (CR0_BASE | 8, CR4_PAE, Some(6)),
        ] {
            for code in [
                reg_code(kind, false, 1, 2, 0x0c),
                mem_code(kind, false, 1, &FORMS[0], 0x0c),
            ] {
                let mut start = cpu();
                start.activity = Some(CpuActivity::Running);
                start.sti_shadow = Some(StiShadow::Clear);
                start.cr0 = Some(cr0);
                start.cr4 = Some(cr4);
                start.gpr[Register::Rbx as usize] = DATA;
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
                            "{kind:?} {cr0:#x} {cr4:#x}"
                        );
                        assert_eq!(after, start);
                    }
                    None => assert_eq!(
                        result.exit,
                        RunExit::InstructionBudgetExhausted,
                        "{kind:?} {cr0:#x} {cr4:#x}"
                    ),
                }
            }
        }
    }
}

#[test]
fn without_sse_state_the_forms_stop_as_unsupported_and_need_no_x87_state() {
    for kind in KINDS {
        let code = reg_code(kind, false, 1, 2, 0x00);
        let mut start = cpu();
        start.sse = None;
        let mut ram = Ram::new(&code);
        let (outcome, end) = exec(start.clone(), &mut ram);
        assert_eq!(
            outcome,
            Ok(StepOutcome::Unsupported {
                opcode: kind.opcode()
            })
        );
        assert_eq!(end, start);
        let mut start = cpu();
        start.x87 = None;
        let mut ram = Ram::new(&code);
        assert_eq!(exec(start, &mut ram).0, Ok(StepOutcome::Advanced));
    }
}

#[test]
fn neighbouring_encodings_stay_unsupported_without_change_and_harmless_prefixes_are_ignored() {
    for kind in KINDS {
        let op = kind.opcode();
        for (code, what) in [
            (vec![0x0f, 0x3a, op, 0xca, 0x00], "no 66, register"),
            (vec![0x0f, 0x3a, op, 0x03, 0x00], "no 66, memory"),
            (vec![0xf2, 0x66, 0x0f, 0x3a, op, 0xca, 0x00], "F2 66"),
            (vec![0x66, 0xf3, 0x0f, 0x3a, op, 0xca, 0x00], "66 F3"),
            (vec![0xf3, 0x0f, 0x3a, op, 0xca, 0x00], "F3 only"),
            (vec![0x66, 0x66, 0x0f, 0x3a, op, 0xca, 0x00], "duplicate 66"),
            (vec![0x64, 0x66, 0x0f, 0x3a, op, 0x03, 0x00], "FS"),
            (vec![0x66, 0x65, 0x0f, 0x3a, op, 0x03, 0x00], "GS"),
            (
                vec![0x66, 0x40, 0x40, 0x0f, 0x3a, op, 0xca, 0x00],
                "duplicate REX",
            ),
            (
                vec![0x66, 0x0f, 0x3a, 0x64, 0xca, 0x00],
                "reserved neighbour 64",
            ),
            (
                vec![0x66, 0x0f, 0x3a, 0x5f, 0xca, 0x00],
                "reserved neighbour 5F",
            ),
            (vec![0x66, 0x0f, 0x38, op, 0xca], "0F 38 map"),
        ] {
            let start = cpu();
            let mut ram = Ram::new(&code);
            let (outcome, end) = exec(start.clone(), &mut ram);
            assert!(
                matches!(
                    outcome,
                    Ok(StepOutcome::Unsupported { .. }) | Ok(StepOutcome::Fault(_))
                ),
                "{kind:?} {what}: {outcome:?}"
            );
            assert_eq!(end, start, "{kind:?} {what}");
            assert!(
                ram.reads.is_empty() && ram.writes.is_empty(),
                "{kind:?} {what}"
            );
        }
        // The null segment overrides, the address-size prefix and a REX that selects no register are ignored by
        // the register form; REX.W is ignored by the implicit forms.
        for code in [
            vec![0x2e, 0x66, 0x0f, 0x3a, op, 0xca, 0x00],
            vec![0x67, 0x66, 0x0f, 0x3a, op, 0xca, 0x00],
            vec![0x66, 0x40, 0x0f, 0x3a, op, 0xca, 0x00],
            vec![0x48, 0x66, 0x0f, 0x3a, op, 0xca, 0x00], // a REX before 66 is not the REX prefix
        ] {
            let start = cpu();
            let mut ram = Ram::new(&code);
            assert_eq!(
                exec(start, &mut ram).0,
                Ok(StepOutcome::Advanced),
                "{kind:?} {code:02x?}"
            );
        }
    }
    // The 0F 3A 64.. neighbours decode as nothing (the profile decoder owns them), never as a string compare.
    let mut ram = Ram::new(&[0x66, 0x0f, 0x3a, 0x64, 0xca, 0x00]);
    assert!(!matches!(
        long_decode::decode(&mut ram, CODE, AccessContext::default()).map(|i| i.operation),
        Ok(Operation::Sse41 { .. })
    ));
}

#[test]
fn decoding_selects_the_kind_the_registers_the_memory_operand_and_the_immediate() {
    let decode = |code: &[u8]| {
        long_decode::decode(&mut Ram::new(code), CODE, AccessContext::default()).unwrap()
    };
    // PCMPESTRI xmm9, xmm12, 0xa5 (REX.R, REX.B)
    let instruction = decode(&[0x66, 0x45, 0x0f, 0x3a, 0x61, 0xcc, 0xa5]);
    assert_eq!(instruction.length, 7);
    assert_eq!(
        instruction.operation,
        Operation::Sse41 {
            kind: Sse41Kind::Pcmpestri,
            xmm: 9,
            operand: Sse41Operand::Xmm(12),
            immediate: 0xa5
        }
    );
    for (op, kind) in [
        (0x60u8, Sse41Kind::Pcmpestrm),
        (0x61, Sse41Kind::Pcmpestri),
        (0x62, Sse41Kind::Pcmpistrm),
        (0x63, Sse41Kind::Pcmpistri),
    ] {
        // with REX.W (the length registers are 64-bit) and a memory operand with disp8
        let instruction = decode(&[0x66, 0x48, 0x0f, 0x3a, op, 0x43, 0x10, 0x7f]);
        assert_eq!(instruction.length, 8);
        let Operation::Sse41 {
            kind: decoded,
            xmm,
            operand,
            immediate,
        } = instruction.operation
        else {
            panic!("{:?}", instruction.operation);
        };
        assert_eq!((decoded, xmm, immediate), (kind, 0, 0x7f));
        assert!(matches!(operand, Sse41Operand::Memory(_)));
        assert!(
            kind.is_string_compare() && !kind.requires_alignment() && kind.memory_bytes() == 16
        );
        assert_eq!(kind.has_explicit_lengths(), op < 0x62);
    }
}

#[test]
fn flag_definedness_defines_all_six_arithmetic_flags() {
    let config = RunConfig {
        instruction_budget: 1,
        stop_rip: None,
        context: ctx(PrivilegeLevel::Ring0),
    };
    for kind in KINDS {
        let mut start = cpu();
        start.rflags = 0x2;
        let code = reg_code(kind, false, 1, 2, 0x0c);
        for (initial, defined) in [
            (0x02u64, 0x02 | STATUS),
            (0x02 | OF, 0x02 | STATUS),
            (0x02 | STATUS | DF, 0x02 | STATUS | DF),
        ] {
            let mut ram = Ram::new(&code);
            let mut after = start.clone();
            let result =
                run_with_rflags_defined(&mut after, &mut ram, config, initial, || false).unwrap();
            assert_eq!(result.exit, RunExit::InstructionBudgetExhausted);
            assert_eq!(result.rflags_defined, defined, "{kind:?} {initial:#x}");
        }
    }
}

#[test]
fn only_the_six_arithmetic_flags_change_and_the_other_flag_bits_survive() {
    let mut rng = Rng(0x56b0_0004);
    for kind in KINDS {
        for initial in [
            0x2u64,
            0x2 | STATUS,
            0x2 | DF | IF,
            FLAGS,
            0x2 | (1 << 18) | STATUS,
            0x2 | (1 << 16) | (1 << 14),
        ] {
            for imm in [0x00u8, 0x0c, 0x18, 0x44] {
                let mut start = cpu();
                start.rflags = initial;
                let (a, b) = (string_data(&mut rng, false), string_data(&mut rng, false));
                start.sse.as_mut().unwrap().xmm[1] = a;
                start.sse.as_mut().unwrap().xmm[2] = b;
                let code = reg_code(kind, false, 1, 2, imm);
                let mut ram = Ram::new(&code);
                let (_, end) = exec(start.clone(), &mut ram);
                assert_eq!(end.rflags & !STATUS, initial & !STATUS, "{kind:?}");
                assert_eq!(end.rflags & (AF | PF), 0, "AF and PF are reset");
                assert_eq!(
                    end,
                    expected_state(&start, kind, false, (a, b), imm, code.len())
                );
            }
        }
    }
}

/// (name, code, RAX, RDX, xmm1 bytes, xmm2/m128 bytes, memory form, RCX after, XMM0 after, status flags after):
/// run on the physical oracle with RCX = 0x1122334455667788, XMM0 = AA 55 ..., RFLAGS = 2 before. Strings are
/// in memory order (byte 0 first). A memory form has ModRM 0x0B (xmm1, [rbx]).
type Golden = (
    &'static str,
    &'static str,
    u64,
    u64,
    &'static str,
    &'static str,
    bool,
    u64,
    &'static str,
    u64,
);

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len() / 2)
        .map(|i| u8::from_str_radix(&text[2 * i..2 * i + 2], 16).unwrap())
        .collect()
}

fn block(text: &str) -> [u8; 16] {
    unhex(text).try_into().unwrap()
}

#[test]
fn golden_vectors_of_the_physical_oracle_are_reproduced_by_the_model_and_by_the_reference() {
    assert_eq!(GOLDEN.len(), 224);
    for &(name, code_hex, rax, rdx, a, b, memory, rcx, xmm0, flags) in GOLDEN {
        let code = unhex(code_hex);
        let (a, b, xmm0) = (block(a), block(b), block(xmm0));
        let mut start = cpu();
        start.rflags = 0x2;
        start.gpr[Register::Rax as usize] = rax;
        start.gpr[Register::Rcx as usize] = 0x1122_3344_5566_7788;
        start.gpr[Register::Rdx as usize] = rdx;
        start.gpr[Register::Rbx as usize] = DATA + 0x100;
        let sse = start.sse.as_mut().unwrap();
        sse.xmm[0] = block("aa55aa55aa55aa55aa55aa55aa55aa55");
        sse.xmm[1] = a;
        sse.xmm[2] = b;
        let mut ram = Ram::new(&code);
        if memory {
            ram.put(DATA + 0x100, &b);
        }
        let (outcome, end) = exec(start.clone(), &mut ram);
        assert_eq!(outcome, Ok(StepOutcome::Advanced), "{name}");
        let mut expected = start.clone();
        expected.gpr[Register::Rcx as usize] = rcx;
        expected.sse.as_mut().unwrap().xmm[0] = xmm0;
        expected.rflags = 0x2 | flags;
        expected.rip = CODE + code.len() as u64;
        assert_eq!(end, expected, "{name}");
        // The reference agrees with the hardware too (it is the oracle of the sweeps).
        let rex_w = code[1] == 0x48;
        let kind = match code[if rex_w { 4 } else { 3 }] {
            0x60 => Kind::Estrm,
            0x61 => Kind::Estri,
            0x62 => Kind::Istrm,
            _ => Kind::Istri,
        };
        let imm = *code.last().unwrap();
        let reference = expected_state(&start, kind, rex_w, (a, b), imm, code.len());
        assert_eq!(
            reference.gpr[Register::Rcx as usize] & 0xffff_ffff,
            rcx & 0xffff_ffff,
            "{name} reference"
        );
        assert_eq!(
            reference.sse.as_ref().unwrap().xmm[0],
            xmm0,
            "{name} reference"
        );
        assert_eq!(reference.rflags & STATUS, flags, "{name} reference");
    }
}

const GOLDEN: &[Golden] = &[
    (
        "any_vowel_lsb",
        "660f3a63ca00",
        0x0,
        0x0,
        "6165696f750000000000000000000000",
        "78797a65626364000000000000000000",
        false,
        0x3,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc1,
    ),
    (
        "any_vowel_neg",
        "660f3a63ca10",
        0x0,
        0x0,
        "6165696f750000000000000000000000",
        "78797a65626364000000000000000000",
        false,
        0x0,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x8c1,
    ),
    (
        "any_vowel_masked_neg",
        "660f3a63ca30",
        0x0,
        0x0,
        "6165696f750000000000000000000000",
        "78797a65626364000000000000000000",
        false,
        0x0,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x8c1,
    ),
    (
        "any_msb",
        "660f3a63ca40",
        0x0,
        0x0,
        "6c6f0000000000000000000000000000",
        "68656c6c6f20776f726c640000000000",
        false,
        0x9,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc1,
    ),
    (
        "any_notfound",
        "660f3a63ca00",
        0x0,
        0x0,
        "78797a00000000000000000000000000",
        "68656c6c6f20776f726c640000000000",
        false,
        0x10,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc0,
    ),
    (
        "any_mask_bits",
        "660f3a62ca00",
        0x0,
        0x0,
        "6165696f750000000000000000000000",
        "656475636174696f6e00000000000000",
        false,
        0x1122334455667788,
        "d5000000000000000000000000000000",
        0x8c1,
    ),
    (
        "any_mask_bytes",
        "660f3a62ca40",
        0x0,
        0x0,
        "6165696f750000000000000000000000",
        "656475636174696f6e00000000000000",
        false,
        0x1122334455667788,
        "ff00ff00ff00ffff0000000000000000",
        0x8c1,
    ),
    (
        "any_mask_bytes_neg",
        "660f3a62ca50",
        0x0,
        0x0,
        "6165696f750000000000000000000000",
        "656475636174696f6e00000000000000",
        false,
        0x1122334455667788,
        "00ff00ff00ff0000ffffffffffffffff",
        0xc1,
    ),
    (
        "any_full_strings",
        "660f3a63ca00",
        0x0,
        0x0,
        "6162636465666768696a6b6c6d6e6f70",
        "706f6e6d6c6b6a696867666564636261",
        false,
        0x0,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x801,
    ),
    (
        "ranges_lower_digits",
        "660f3a63ca04",
        0x0,
        0x0,
        "617a415a303900000000000000000000",
        "202048656c6c6f203432000000000000",
        false,
        0x2,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc1,
    ),
    (
        "ranges_negative",
        "660f3a63ca14",
        0x0,
        0x0,
        "617a0000000000000000000000000000",
        "48656c6c6f20576f726c640000000000",
        false,
        0x0,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x8c1,
    ),
    (
        "ranges_msb_mask",
        "660f3a62ca44",
        0x0,
        0x0,
        "617a0000000000000000000000000000",
        "48656c6c6f20576f726c640000000000",
        false,
        0x1122334455667788,
        "00ffffffff0000ffffffff0000000000",
        0xc1,
    ),
    (
        "ranges_odd_set",
        "660f3a63ca04",
        0x0,
        0x0,
        "617a6200000000000000000000000000",
        "30313233206200000000000000000000",
        false,
        0x5,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc1,
    ),
    (
        "ranges_signed",
        "660f3a63ca06",
        0x0,
        0x0,
        "80fe0000000000000000000000000000",
        "017fff80fdfe00000000000000000000",
        false,
        0x3,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc1,
    ),
    (
        "ranges_unsigned",
        "660f3a63ca04",
        0x0,
        0x0,
        "80fe0000000000000000000000000000",
        "017fff80fdfe00000000000000000000",
        false,
        0x3,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc1,
    ),
    (
        "each_equal",
        "660f3a63ca08",
        0x0,
        0x0,
        "68656c6c6f0000000000000000000000",
        "68656c6c6f0000000000000000000000",
        false,
        0x0,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x8c1,
    ),
    (
        "each_equal_neg",
        "660f3a63ca18",
        0x0,
        0x0,
        "68656c6c6f0000000000000000000000",
        "68656c6c6f0000000000000000000000",
        false,
        0x10,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc0,
    ),
    (
        "each_differs_neg",
        "660f3a63ca18",
        0x0,
        0x0,
        "68656c6c6f0000000000000000000000",
        "68656c6c700000000000000000000000",
        false,
        0x4,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc1,
    ),
    (
        "each_differs_neg_msb",
        "660f3a63ca58",
        0x0,
        0x0,
        "68656c6c6f0000000000000000000000",
        "68786c6c700000000000000000000000",
        false,
        0x4,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc1,
    ),
    (
        "each_prefix_neg",
        "660f3a63ca18",
        0x0,
        0x0,
        "68656c00000000000000000000000000",
        "68656c6c6f0000000000000000000000",
        false,
        0x3,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc1,
    ),
    (
        "each_prefix_masked",
        "660f3a63ca38",
        0x0,
        0x0,
        "68656c00000000000000000000000000",
        "68656c6c6f0000000000000000000000",
        false,
        0x3,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc1,
    ),
    (
        "each_mask",
        "660f3a62ca08",
        0x0,
        0x0,
        "61626378656667000000000000000000",
        "61626364656667000000000000000000",
        false,
        0x1122334455667788,
        "f7ff0000000000000000000000000000",
        0x8c1,
    ),
    (
        "each_empty",
        "660f3a63ca08",
        0x0,
        0x0,
        "00000000000000000000000000000000",
        "00000000000000000000000000000000",
        false,
        0x0,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x8c1,
    ),
    (
        "each_one_empty",
        "660f3a63ca08",
        0x0,
        0x0,
        "00000000000000000000000000000000",
        "61000000000000000000000000000000",
        false,
        0x1,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc1,
    ),
    (
        "ordered_found_lsb",
        "660f3a63ca0c",
        0x0,
        0x0,
        "61620000000000000000000000000000",
        "78786162787861620000000000000000",
        false,
        0x2,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc1,
    ),
    (
        "ordered_found_msb",
        "660f3a63ca4c",
        0x0,
        0x0,
        "61620000000000000000000000000000",
        "78786162787861620000000000000000",
        false,
        0x6,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc1,
    ),
    (
        "ordered_partial_tail",
        "660f3a63ca0c",
        0x0,
        0x0,
        "61626300000000000000000000000000",
        "78787878616200000000000000000000",
        false,
        0x10,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc0,
    ),
    (
        "ordered_empty_needle",
        "660f3a63ca0c",
        0x0,
        0x0,
        "00000000000000000000000000000000",
        "61626300000000000000000000000000",
        false,
        0x0,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x8c1,
    ),
    (
        "ordered_empty_both",
        "660f3a63ca0c",
        0x0,
        0x0,
        "00000000000000000000000000000000",
        "00000000000000000000000000000000",
        false,
        0x0,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x8c1,
    ),
    (
        "ordered_needle_longer",
        "660f3a63ca0c",
        0x0,
        0x0,
        "61626364656600000000000000000000",
        "61626300000000000000000000000000",
        false,
        0x10,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc0,
    ),
    (
        "ordered_mask",
        "660f3a62ca0c",
        0x0,
        0x0,
        "61610000000000000000000000000000",
        "61616178616100000000000000000000",
        false,
        0x1122334455667788,
        "13000000000000000000000000000000",
        0x8c1,
    ),
    (
        "ordered_full",
        "660f3a63ca0c",
        0x0,
        0x0,
        "6162636465666768696a6b6c6d6e6f70",
        "6162636465666768696a6b6c6d6e6f70",
        false,
        0x0,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x801,
    ),
    (
        "e_any_len_3_5",
        "660f3a61ca00",
        0x3,
        0x5,
        "61626364656667000000000000000000",
        "78786378787878000000000000000000",
        false,
        0x2,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc1,
    ),
    (
        "e_any_len_neg",
        "660f3a61ca00",
        0xfffffffffffffffd,
        0xfffffffffffffffb,
        "61626364656667000000000000000000",
        "78786378787878000000000000000000",
        false,
        0x2,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc1,
    ),
    (
        "e_any_len_0_0",
        "660f3a61ca00",
        0x0,
        0x0,
        "61626364656667000000000000000000",
        "78786378787878000000000000000000",
        false,
        0x10,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc0,
    ),
    (
        "e_any_len_16",
        "660f3a61ca00",
        0x10,
        0x10,
        "6162636465666768696a6b6c6d6e6f70",
        "70707070707070707070707070707070",
        false,
        0x0,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x801,
    ),
    (
        "e_any_len_17",
        "660f3a61ca00",
        0x11,
        0x11,
        "6162636465666768696a6b6c6d6e6f70",
        "70707070707070707070707070707070",
        false,
        0x0,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x801,
    ),
    (
        "e_any_len_minus17",
        "660f3a61ca00",
        0xffffffffffffffef,
        0xffffffffffffffef,
        "6162636465666768696a6b6c6d6e6f70",
        "70707070707070707070707070707070",
        false,
        0x0,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x801,
    ),
    (
        "e_any_len_int_min",
        "660f3a61ca00",
        0x80000000,
        0x80000000,
        "6162636465666768696a6b6c6d6e6f70",
        "70707070707070707070707070707070",
        false,
        0x0,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x801,
    ),
    (
        "e_any_nulls_inside",
        "660f3a61ca00",
        0x4,
        0x5,
        "01000200000000000000000000000000",
        "00000200010000000000000000000000",
        false,
        0x0,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x8c1,
    ),
    (
        "e_each_len",
        "660f3a61ca18",
        0x8,
        0x8,
        "61626364656667680000000000000000",
        "61626364586667680000000000000000",
        false,
        0x4,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc1,
    ),
    (
        "e_each_len_short",
        "660f3a61ca18",
        0x8,
        0x4,
        "61626364656667680000000000000000",
        "61626364656667680000000000000000",
        false,
        0x4,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc1,
    ),
    (
        "e_ordered_len",
        "660f3a61ca0c",
        0x2,
        0xa,
        "61626162000000000000000000000000",
        "61626162616261626162000000000000",
        false,
        0x0,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x8c1,
    ),
    (
        "e_mask_len",
        "660f3a60ca40",
        0x3,
        0x9,
        "61626300000000000000000000000000",
        "63626163626163626100000000000000",
        false,
        0x1122334455667788,
        "ffffffffffffffffff00000000000000",
        0x8c1,
    ),
    (
        "e_rexw_high_ignored",
        "660f3a61ca00",
        0x100000003,
        0x100000003,
        "6162636465666768696a6b6c6d6e6f70",
        "70707070707070707070707070707070",
        false,
        0x10,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc0,
    ),
    (
        "e_rexw_high_counts",
        "66480f3a61ca00",
        0x100000003,
        0x100000003,
        "6162636465666768696a6b6c6d6e6f70",
        "70707070707070707070707070707070",
        false,
        0x0,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x801,
    ),
    (
        "e_rexw_neg64",
        "66480f3a61ca00",
        0xfffffffffffffffd,
        0xfffffffffffffffb,
        "6162636465666768696a6b6c6d6e6f70",
        "70707070707070707070707070707070",
        false,
        0x10,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc0,
    ),
    (
        "e_rexw_min64",
        "66480f3a61ca00",
        0x8000000000000000,
        0x8000000000000001,
        "6162636465666768696a6b6c6d6e6f70",
        "70707070707070707070707070707070",
        false,
        0x0,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x801,
    ),
    (
        "e_rexw_mask",
        "66480f3a60ca00",
        0x2,
        0xffffffff00000010,
        "6162636465666768696a6b6c6d6e6f70",
        "70707070707070707070707070707070",
        false,
        0x1122334455667788,
        "00000000000000000000000000000000",
        0x80,
    ),
    (
        "w_any",
        "660f3a63ca01",
        0x0,
        0x0,
        "41000041341200000000000000000000",
        "01000200004103000000000000000000",
        false,
        0x2,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc1,
    ),
    (
        "w_ranges_signed",
        "660f3a63ca07",
        0x0,
        0x0,
        "0080f0ff000000000000000000000000",
        "0100ff7fffff0080f1ff000000000000",
        false,
        0x3,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc1,
    ),
    (
        "w_ranges_unsigned",
        "660f3a63ca05",
        0x0,
        0x0,
        "0080f0ff000000000000000000000000",
        "0100ff7fffff0080f1ff000000000000",
        false,
        0x3,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc1,
    ),
    (
        "w_each_neg",
        "660f3a63ca19",
        0x0,
        0x0,
        "01000200030004000000000000000000",
        "01000200090004000000000000000000",
        false,
        0x2,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc1,
    ),
    (
        "w_ordered",
        "660f3a63ca0d",
        0x0,
        0x0,
        "02000300000000000000000000000000",
        "01000200030002000300000000000000",
        false,
        0x1,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc1,
    ),
    (
        "w_ordered_msb_mask",
        "660f3a62ca4d",
        0x0,
        0x0,
        "02000300000000000000000000000000",
        "01000200030002000300000000000000",
        false,
        0x1122334455667788,
        "0000ffff0000ffff0000000000000000",
        0xc1,
    ),
    (
        "w_null_in_low_byte_only",
        "660f3a63ca01",
        0x0,
        0x0,
        "00010002000000000000000000000000",
        "00010100000000000000000000000000",
        false,
        0x0,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x8c1,
    ),
    (
        "w_e_len_8",
        "660f3a61ca01",
        0x8,
        0x8,
        "01000200030004000500060007000800",
        "08000800080008000800080008000800",
        false,
        0x0,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x801,
    ),
    (
        "w_e_len_9",
        "660f3a61ca01",
        0x9,
        0x9,
        "01000200030004000500060007000800",
        "08000800080008000800080008000800",
        false,
        0x0,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x801,
    ),
    (
        "w_e_len_16",
        "660f3a60ca41",
        0xfffffffffffffff0,
        0xfffffffffffffff0,
        "01000200030004000500060007000800",
        "08000800080008000800080008000800",
        false,
        0x1122334455667788,
        "ffffffffffffffffffffffffffffffff",
        0x801,
    ),
    (
        "w_e_len_7_mask",
        "660f3a60ca01",
        0x7,
        0x7,
        "01000200030004000500060007000800",
        "08000700060005000400030002000100",
        false,
        0x1122334455667788,
        "7e000000000000000000000000000000",
        0xc1,
    ),
    (
        "m_any",
        "660f3a630b00",
        0x0,
        0x0,
        "6165696f750000000000000000000000",
        "78797a65626364000000000000000000",
        true,
        0x3,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc1,
    ),
    (
        "m_each_neg",
        "660f3a630b18",
        0x0,
        0x0,
        "68656c6c6f0000000000000000000000",
        "68656c6c700000000000000000000000",
        true,
        0x4,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc1,
    ),
    (
        "m_e_ranges",
        "660f3a600b44",
        0x2,
        0xb,
        "617a0000000000000000000000000000",
        "48656c6c6f20576f726c640000000000",
        true,
        0x1122334455667788,
        "00ffffffff0000ffffffff0000000000",
        0xc1,
    ),
    (
        "m_e_rexw",
        "66480f3a610b0c",
        0x2,
        0x8,
        "61620000000000000000000000000000",
        "78786162787861620000000000000000",
        true,
        0x2,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc1,
    ),
    (
        "g000_istri_i00",
        "660f3a630b00",
        0x0,
        0x0,
        "39414230612000000000000000000000",
        "20396120204220ff4180ff8000000000",
        true,
        0x0,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x8c1,
    ),
    (
        "g001_istrm_i03",
        "660f3a62ca03",
        0x0,
        0x0,
        "0080ff7f008001006100ffff00000000",
        "ff7f00410001ff7f0000000000000000",
        false,
        0x1122334455667788,
        "09000000000000000000000000000000",
        0x8c1,
    ),
    (
        "g002_estri_i04",
        "660f3a61ca04",
        0xfffffffffffffffc,
        0x9,
        "41206241000000000000000000000000",
        "417f61808042307f7f00000000000000",
        false,
        0x10,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc0,
    ),
    (
        "g003_estrm_i07",
        "660f3a60ca07",
        0x4,
        0x0,
        "0100ff7f620000000000000000000000",
        "00000000000000000000000000000000",
        false,
        0x1122334455667788,
        "00000000000000000000000000000000",
        0xc0,
    ),
    (
        "g004_istri_i08",
        "660f3a63ca08",
        0x0,
        0x0,
        "7f7f203062622030ff30000000000000",
        "20ff61806261626280ff000000000000",
        false,
        0x4,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc1,
    ),
    (
        "g005_istrm_i0b",
        "660f3a620b0b",
        0x0,
        0x0,
        "00800100610062000000000000000000",
        "00800080ffffffff0000000000000000",
        true,
        0x1122334455667788,
        "f1000000000000000000000000000000",
        0x8c1,
    ),
    (
        "g006_estri_i0c",
        "660f3a61ca0c",
        0xfffffffffffffff0,
        0x9,
        "626162ff80416120393042303939ff41",
        "617f617f300000000000000000000000",
        false,
        0x10,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x40,
    ),
    (
        "g007_estrm_i0f",
        "66480f3a60ca0f",
        0x2,
        0x11,
        "ff7f0001000000000000000000000000",
        "62006100000161000001ffffff7f0001",
        false,
        0x1122334455667788,
        "40000000000000000000000000000000",
        0x81,
    ),
    (
        "g008_istri_i10",
        "660f3a63ca10",
        0x0,
        0x0,
        "30390000000000000000000000000000",
        "4230ff42422041000000000000000000",
        false,
        0x0,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x8c1,
    ),
    (
        "g009_istrm_i13",
        "660f3a62ca13",
        0x0,
        0x0,
        "004162000080ff7f0041010000410001",
        "62000000000000000000000000000000",
        false,
        0x1122334455667788,
        "fe000000000000000000000000000000",
        0x41,
    ),
    (
        "g010_estri_i14",
        "660f3a610b14",
        0x0,
        0x1,
        "41203939416162306280808039304200",
        "00000000000000000000000000000000",
        true,
        0x0,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x8c1,
    ),
    (
        "g011_estrm_i17",
        "660f3a60ca17",
        0x11,
        0x11,
        "0001ff7fffff00410041000000000000",
        "000100800100ffff0001000100000000",
        false,
        0x1122334455667788,
        "02000000000000000000000000000000",
        0x1,
    ),
    (
        "g012_istri_i18",
        "660f3a63ca18",
        0x0,
        0x0,
        "62000000000000000000000000000000",
        "61423900000000000000000000000000",
        false,
        0x0,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x8c1,
    ),
    (
        "g013_istrm_i1b",
        "660f3a62ca1b",
        0x0,
        0x0,
        "0001ffff620000416200008000000000",
        "01000080010001000000000000000000",
        false,
        0x1122334455667788,
        "3f000000000000000000000000000000",
        0x8c1,
    ),
    (
        "g014_estri_i1c",
        "66480f3a61ca1c",
        0x11,
        0xf,
        "30396100000000000000000000000000",
        "39806239396261803039802039616100",
        false,
        0x0,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x841,
    ),
    (
        "g015_estrm_i1f",
        "660f3a600b1f",
        0x9,
        0x0,
        "00800001008000800000000000000000",
        "0080ffff000100010000000000000000",
        true,
        0x1122334455667788,
        "ff000000000000000000000000000000",
        0x841,
    ),
    (
        "g016_istri_i20",
        "660f3a63ca20",
        0x0,
        0x0,
        "42627f00000000000000000000000000",
        "612062413941ff204200000000000000",
        false,
        0x2,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc1,
    ),
    (
        "g017_istrm_i23",
        "660f3a62ca23",
        0x0,
        0x0,
        "01006200ff7f00410000000000000000",
        "ff7fffff000000000000000000000000",
        false,
        0x1122334455667788,
        "01000000000000000000000000000000",
        0x8c1,
    ),
    (
        "g018_estri_i24",
        "660f3a61ca24",
        0x9,
        0x10,
        "80426261394130610000000000000000",
        "3962398062626180627f613030802000",
        false,
        0x0,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x881,
    ),
    (
        "g019_estrm_i27",
        "660f3a60ca27",
        0x0,
        0x9,
        "62006200620000016200010000800001",
        "0100004161000041ff7fffff00010080",
        false,
        0x1122334455667788,
        "00000000000000000000000000000000",
        0x80,
    ),
    (
        "g020_istri_i28",
        "660f3a630b28",
        0x0,
        0x0,
        "393942426261ffff2039ff6261418061",
        "30806241616141000000000000000000",
        true,
        0x5,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x41,
    ),
    (
        "g021_istrm_i2b",
        "660f3a62ca2b",
        0x0,
        0x0,
        "6100610061006200008062000041ff7f",
        "0001008062006200ff7f000100000000",
        false,
        0x1122334455667788,
        "08000000000000000000000000000000",
        0x41,
    ),
    (
        "g022_estri_i2c",
        "660f3a61ca2c",
        0x1,
        0xd,
        "00000000000000000000000000000000",
        "412042ff7f3042203939413062000000",
        false,
        0x10,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc0,
    ),
    (
        "g023_estrm_i2f",
        "660f3a60ca2f",
        0x9,
        0x11,
        "62000041ff7f00000000000000000000",
        "61000041620000000000000000000000",
        false,
        0x1122334455667788,
        "00000000000000000000000000000000",
        0x0,
    ),
    (
        "g024_istri_i30",
        "660f3a63ca30",
        0x0,
        0x0,
        "42804261000000000000000000000000",
        "ff8061ffff2080610000000000000000",
        false,
        0x0,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x8c1,
    ),
    (
        "g025_istrm_i33",
        "660f3a620b33",
        0x0,
        0x0,
        "ffff6100010000010041ffffff7f0000",
        "0041ffff000000000000000000000000",
        true,
        0x1122334455667788,
        "00000000000000000000000000000000",
        0xc0,
    ),
    (
        "g026_estri_i34",
        "660f3a61ca34",
        0x9,
        0x1,
        "6261307f62ff41000000000000000000",
        "80000000000000000000000000000000",
        false,
        0x10,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc0,
    ),
    (
        "g027_estrm_i37",
        "660f3a60ca37",
        0x11,
        0x5,
        "00806100010000016100000101000000",
        "ff7f0041ff7fff7f0000000000000000",
        false,
        0x1122334455667788,
        "0f000000000000000000000000000000",
        0x841,
    ),
    (
        "g028_istri_i38",
        "660f3a63ca38",
        0x0,
        0x0,
        "417f8000000000000000000000000000",
        "4141394130617f412061ff80ff000000",
        false,
        0x1,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc1,
    ),
    (
        "g029_istrm_i3b",
        "660f3a62ca3b",
        0x0,
        0x0,
        "ffff0000000000000000000000000000",
        "00416100010000000000000000000000",
        false,
        0x1122334455667788,
        "ff000000000000000000000000000000",
        0x8c1,
    ),
    (
        "g030_estri_i3c",
        "660f3a610b3c",
        0xfffffffffffffffa,
        0xb,
        "61806280396200000000000000000000",
        "4220627f393080204162000000000000",
        true,
        0x0,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x8c1,
    ),
    (
        "g031_estrm_i3f",
        "660f3a60ca3f",
        0x1,
        0x1,
        "00000000000000000000000000000000",
        "ffff0000000000000000000000000000",
        false,
        0x1122334455667788,
        "01000000000000000000000000000000",
        0x8c1,
    ),
    (
        "g032_istri_i40",
        "660f3a63ca40",
        0x0,
        0x0,
        "ff7f807f000000000000000000000000",
        "80806180000000000000000000000000",
        false,
        0x3,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x8c1,
    ),
    (
        "g033_istrm_i43",
        "660f3a62ca43",
        0x0,
        0x0,
        "00410100000000000000000000000000",
        "61000000000000000000000000000000",
        false,
        0x1122334455667788,
        "00000000000000000000000000000000",
        0xc0,
    ),
    (
        "g034_estri_i44",
        "660f3a61ca44",
        0x11,
        0x11,
        "30417f623920ff41424242ff30000000",
        "80203039306200000000000000000000",
        false,
        0xf,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x801,
    ),
    (
        "g035_estrm_i47",
        "66480f3a600b47",
        0x11,
        0x11,
        "0001ffff010000016100620062000100",
        "0100ffffffff0100ffff620000000000",
        true,
        0x1122334455667788,
        "ffff00000000ffff0000ffff00000000",
        0x801,
    ),
    (
        "g036_istri_i48",
        "660f3a63ca48",
        0x0,
        0x0,
        "7f808080ffff62ffff8080422039ff42",
        "41307fff41ff6262627f000000000000",
        false,
        0x6,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x41,
    ),
    (
        "g037_istrm_i4b",
        "660f3a62ca4b",
        0x0,
        0x0,
        "0100008000800001ffffffff0041ffff",
        "61006200620000000000000000000000",
        false,
        0x1122334455667788,
        "00000000000000000000000000000000",
        0x40,
    ),
    (
        "g038_estri_i4c",
        "660f3a61ca4c",
        0x0,
        0xf,
        "4262627f2030618062397f4142000000",
        "627f2020ff62ff803042302061300000",
        false,
        0xf,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x8c1,
    ),
    (
        "g039_estrm_i4f",
        "660f3a60ca4f",
        0x11,
        0x9,
        "00010000000000000000000000000000",
        "01006200ffff00010000000000000000",
        false,
        0x1122334455667788,
        "000000000000ffff0000000000000000",
        0x1,
    ),
    (
        "g040_istri_i50",
        "660f3a630b50",
        0x0,
        0x0,
        "41ffff20390000000000000000000000",
        "00000000000000000000000000000000",
        true,
        0xf,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x8c1,
    ),
    (
        "g041_istrm_i53",
        "660f3a62ca53",
        0x0,
        0x0,
        "62000041620000000000000000000000",
        "008062000001ff7f6200000000000000",
        false,
        0x1122334455667788,
        "ffff0000ffffffff0000ffffffffffff",
        0x8c1,
    ),
    (
        "g042_estri_i54",
        "66480f3a61ca54",
        0x11,
        0xfffffffffffffff5,
        "62393920414180306242000000000000",
        "41ff417f392020423020420000000000",
        false,
        0xf,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x41,
    ),
    (
        "g043_estrm_i57",
        "660f3a60ca57",
        0xfffffffffffffffa,
        0xfffffffffffffffb,
        "00010080ff7f00010100008000000000",
        "00800080620061000080000000000000",
        false,
        0x1122334455667788,
        "ffffffffffffffffffffffffffffffff",
        0x8c1,
    ),
    (
        "g044_istri_i58",
        "660f3a63ca58",
        0x0,
        0x0,
        "80ff3080000000000000000000000000",
        "ff418080416141ff807f802020000000",
        false,
        0xc,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x8c1,
    ),
    (
        "g045_istrm_i5b",
        "660f3a620b5b",
        0x0,
        0x0,
        "01000000000000000000000000000000",
        "62000100620061000000000000000000",
        true,
        0x1122334455667788,
        "ffffffffffffffff0000000000000000",
        0x8c1,
    ),
    (
        "g046_estri_i5c",
        "660f3a61ca5c",
        0xfffffffffffffff2,
        0x1,
        "8039ff20392062413961ff6220610000",
        "00000000000000000000000000000000",
        false,
        0xf,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x8c1,
    ),
    (
        "g047_estrm_i5f",
        "660f3a60ca5f",
        0x9,
        0x9,
        "62000080ffff62000001ffff01000080",
        "00010041ff7f00000000000000000000",
        false,
        0x1122334455667788,
        "ffffffffffffffffffffffffffffffff",
        0x801,
    ),
    (
        "g048_istri_i60",
        "660f3a63ca60",
        0x0,
        0x0,
        "42ff2080000000000000000000000000",
        "614230617f4242627f42627f20ff6180",
        false,
        0xf,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x81,
    ),
    (
        "g049_istrm_i63",
        "660f3a62ca63",
        0x0,
        0x0,
        "620000010100ffff0000000000000000",
        "00010001ffff00800001ffff0080ff7f",
        false,
        0x1122334455667788,
        "ffffffffffff0000ffffffff00000000",
        0x881,
    ),
    (
        "g050_estri_i64",
        "660f3a610b64",
        0x9,
        0x0,
        "207f4180000000000000000000000000",
        "42410000000000000000000000000000",
        true,
        0x10,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc0,
    ),
    (
        "g051_estrm_i67",
        "660f3a60ca67",
        0x4,
        0x0,
        "01006200ff7f01000000000000000000",
        "ffffffff0041ffff0000000000000000",
        false,
        0x1122334455667788,
        "00000000000000000000000000000000",
        0xc0,
    ),
    (
        "g052_istri_i68",
        "660f3a63ca68",
        0x0,
        0x0,
        "80206239623961807fffff20807fff7f",
        "61616262000000000000000000000000",
        false,
        0x2,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x41,
    ),
    (
        "g053_istrm_i6b",
        "660f3a62ca6b",
        0x0,
        0x0,
        "00410000000000000000000000000000",
        "6200ff7f004100010100010000000000",
        false,
        0x1122334455667788,
        "000000000000000000000000ffffffff",
        0xc1,
    ),
    (
        "g054_estri_i6c",
        "660f3a61ca6c",
        0x9,
        0x1,
        "7f616261ff6161ff3020413000000000",
        "41000000000000000000000000000000",
        false,
        0x10,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc0,
    ),
    (
        "g055_estrm_i6f",
        "660f3a600b6f",
        0x0,
        0x0,
        "01000041000162000001ffff0100ffff",
        "00800001610061000041000000000000",
        true,
        0x1122334455667788,
        "ffffffffffffffffffffffffffffffff",
        0x8c1,
    ),
    (
        "g056_istri_i70",
        "660f3a63ca70",
        0x0,
        0x0,
        "207f3030427f61ffff39306220000000",
        "39204239208041803942000000000000",
        false,
        0x7,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc1,
    ),
    (
        "g057_istrm_i73",
        "660f3a62ca73",
        0x0,
        0x0,
        "00000000000000000000000000000000",
        "ff7f0000000000000000000000000000",
        false,
        0x1122334455667788,
        "ffff0000000000000000000000000000",
        0x8c1,
    ),
    (
        "g058_estri_i74",
        "660f3a61ca74",
        0x8,
        0xc,
        "803061ff426130620000000000000000",
        "80ff3942807fff41ff30394100000000",
        false,
        0x10,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc0,
    ),
    (
        "g059_estrm_i77",
        "660f3a60ca77",
        0xfffffffffffffff8,
        0x0,
        "6100010062000100ff7f004162006200",
        "00010080000101000041ffffffff0100",
        false,
        0x1122334455667788,
        "00000000000000000000000000000000",
        0x40,
    ),
    (
        "g060_istri_i78",
        "660f3a630b78",
        0x0,
        0x0,
        "00000000000000000000000000000000",
        "39612041426239800000000000000000",
        true,
        0xf,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x8c1,
    ),
    (
        "g061_istrm_i7b",
        "660f3a62ca7b",
        0x0,
        0x0,
        "00000000000000000000000000000000",
        "ffff62000001ff7f6100000000000000",
        false,
        0x1122334455667788,
        "ffffffffffffffffffffffffffffffff",
        0x8c1,
    ),
    (
        "g062_estri_i7c",
        "660f3a61ca7c",
        0x9,
        0xe,
        "39426100000000000000000000000000",
        "ff8062ff628020393062ff8041000000",
        false,
        0xd,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x8c1,
    ),
    (
        "g063_estrm_i7f",
        "66480f3a60ca7f",
        0x9,
        0x11,
        "008061006100ff7f0080000000000000",
        "0001004161006200ffff000000000000",
        false,
        0x1122334455667788,
        "ffffffffffffffffffffffffffffffff",
        0x801,
    ),
    (
        "g064_istri_i80",
        "660f3a63ca80",
        0x0,
        0x0,
        "20416130ff422030307f6239804230ff",
        "30300000000000000000000000000000",
        false,
        0x0,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x841,
    ),
    (
        "g065_istrm_i83",
        "660f3a620b83",
        0x0,
        0x0,
        "00800000000000000000000000000000",
        "ff7f6200610000000000000000000000",
        true,
        0x1122334455667788,
        "00000000000000000000000000000000",
        0xc0,
    ),
    (
        "g066_estri_i84",
        "660f3a61ca84",
        0x9,
        0xfffffffffffffff8,
        "42ff0000000000000000000000000000",
        "3039ff7f628061420000000000000000",
        false,
        0x2,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc1,
    ),
    (
        "g067_estrm_i87",
        "660f3a60ca87",
        0x6,
        0x0,
        "ffff62000100ff7f0041000000000000",
        "ff7f6100610000000000000000000000",
        false,
        0x1122334455667788,
        "00000000000000000000000000000000",
        0xc0,
    ),
    (
        "g068_istri_i88",
        "660f3a63ca88",
        0x0,
        0x0,
        "3939412041ff42420000000000000000",
        "42620000000000000000000000000000",
        false,
        0x8,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc1,
    ),
    (
        "g069_istrm_i8b",
        "660f3a62ca8b",
        0x0,
        0x0,
        "01006200610000416200008061000000",
        "0100ffff000100410041008000010001",
        false,
        0x1122334455667788,
        "29000000000000000000000000000000",
        0x881,
    ),
    (
        "g070_estri_i8c",
        "66480f3a610b8c",
        0x0,
        0x0,
        "00000000000000000000000000000000",
        "7f423030000000000000000000000000",
        true,
        0x0,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x8c1,
    ),
    (
        "g071_estrm_i8f",
        "660f3a60ca8f",
        0xffffffffffffffff,
        0x7,
        "61000000000000000000000000000000",
        "ffffff7f00416100ffff008062000000",
        false,
        0x1122334455667788,
        "08000000000000000000000000000000",
        0xc1,
    ),
    (
        "g072_istri_i90",
        "660f3a63ca90",
        0x0,
        0x0,
        "7f612000000000000000000000000000",
        "62627f20420000000000000000000000",
        false,
        0x0,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x8c1,
    ),
    (
        "g073_istrm_i93",
        "660f3a62ca93",
        0x0,
        0x0,
        "ffff00010100ff7f6200004161000000",
        "00000000000000000000000000000000",
        false,
        0x1122334455667788,
        "ff000000000000000000000000000000",
        0x8c1,
    ),
    (
        "g074_estri_i94",
        "660f3a61ca94",
        0x11,
        0x9,
        "20ff20ff800000000000000000000000",
        "206141413041306139618080ff613062",
        false,
        0x9,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x41,
    ),
    (
        "g075_estrm_i97",
        "660f3a600b97",
        0x11,
        0xfffffffffffffffd,
        "01000041610061000041000162006100",
        "61006200010000000000000000000000",
        true,
        0x1122334455667788,
        "f8000000000000000000000000000000",
        0x41,
    ),
    (
        "g076_istri_i98",
        "660f3a63ca98",
        0x0,
        0x0,
        "42610000000000000000000000000000",
        "8030ff7fff80617f7f3041807f426130",
        false,
        0x0,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x881,
    ),
    (
        "g077_istrm_i9b",
        "660f3a62ca9b",
        0x0,
        0x0,
        "00410100ffff0080008062000100ffff",
        "00000000000000000000000000000000",
        false,
        0x1122334455667788,
        "ff000000000000000000000000000000",
        0x841,
    ),
    (
        "g078_estri_i9c",
        "660f3a61ca9c",
        0xf,
        0x6,
        "62302042622041802042204220623000",
        "62394180390000000000000000000000",
        false,
        0x0,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x8c1,
    ),
    (
        "g079_estrm_i9f",
        "660f3a60ca9f",
        0x1,
        0xfffffffffffffffe,
        "00000000000000000000000000000000",
        "ffff0100000000000000000000000000",
        false,
        0x1122334455667788,
        "ff000000000000000000000000000000",
        0x8c1,
    ),
    (
        "g080_istri_ia0",
        "660f3a630ba0",
        0x0,
        0x0,
        "62623980ff3041ff7f397f7f7f800000",
        "4262ff62397f62000000000000000000",
        true,
        0x1,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc1,
    ),
    (
        "g081_istrm_ia3",
        "660f3a62caa3",
        0x0,
        0x0,
        "ffff0000000000000000000000000000",
        "0041ff7f00016100ffff000000000000",
        false,
        0x1122334455667788,
        "10000000000000000000000000000000",
        0xc1,
    ),
    (
        "g082_estri_ia4",
        "660f3a61caa4",
        0x0,
        0x0,
        "00000000000000000000000000000000",
        "7f202062390000000000000000000000",
        false,
        0x10,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc0,
    ),
    (
        "g083_estrm_ia7",
        "660f3a60caa7",
        0x9,
        0x1,
        "61000001ff7f00000000000000000000",
        "01000000000000000000000000000000",
        false,
        0x1122334455667788,
        "00000000000000000000000000000000",
        0x40,
    ),
    (
        "g084_istri_ia8",
        "660f3a63caa8",
        0x0,
        0x0,
        "39392061204142000000000000000000",
        "61427f41304100000000000000000000",
        false,
        0x5,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc1,
    ),
    (
        "g085_istrm_iab",
        "660f3a620bab",
        0x0,
        0x0,
        "00800100004100416100000100010000",
        "01000041ff7f61000080610000000000",
        true,
        0x1122334455667788,
        "80000000000000000000000000000000",
        0xc1,
    ),
    (
        "g086_estri_iac",
        "660f3a61caac",
        0x7,
        0x2,
        "208030427f3930000000000000000000",
        "61000000000000000000000000000000",
        false,
        0x10,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc0,
    ),
    (
        "g087_estrm_iaf",
        "660f3a60caaf",
        0x11,
        0x0,
        "00410080004100010001610000000000",
        "01006100610000000000000000000000",
        false,
        0x1122334455667788,
        "00000000000000000000000000000000",
        0x40,
    ),
    (
        "g088_istri_ib0",
        "660f3a63cab0",
        0x0,
        0x0,
        "ff397f00000000000000000000000000",
        "397f42423039306261804262806120ff",
        false,
        0x2,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x81,
    ),
    (
        "g089_istrm_ib3",
        "660f3a62cab3",
        0x0,
        0x0,
        "620000410041610000416200ffffffff",
        "00010000000000000000000000000000",
        false,
        0x1122334455667788,
        "01000000000000000000000000000000",
        0x841,
    ),
    (
        "g090_estri_ib4",
        "660f3a610bb4",
        0x0,
        0x0,
        "80613900000000000000000000000000",
        "2042ff2020ff39623080624241000000",
        true,
        0x10,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc0,
    ),
    (
        "g091_estrm_ib7",
        "66480f3a60cab7",
        0x9,
        0x9,
        "ff7fff7f000000000000000000000000",
        "0001ffffff7f00010000000000000000",
        false,
        0x1122334455667788,
        "0b000000000000000000000000000000",
        0x801,
    ),
    (
        "g092_istri_ib8",
        "660f3a63cab8",
        0x0,
        0x0,
        "7f7f4220617f306142627f6120306142",
        "7fff626239623930417f392042626200",
        false,
        0x1,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x41,
    ),
    (
        "g093_istrm_ibb",
        "660f3a62cabb",
        0x0,
        0x0,
        "0041004161000041ff7f000000000000",
        "61000080ffff61000000000000000000",
        false,
        0x1122334455667788,
        "ef000000000000000000000000000000",
        0x8c1,
    ),
    (
        "g094_estri_ibc",
        "660f3a61cabc",
        0xa,
        0x7,
        "20ff7fff61ff7f396241000000000000",
        "7f3042ff207f00000000000000000000",
        false,
        0x0,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x8c1,
    ),
    (
        "g095_estrm_ibf",
        "660f3a600bbf",
        0x8,
        0xfffffffffffffff8,
        "0100000100410080ff7f620061006100",
        "0001004101000041ff7f004100416100",
        true,
        0x1122334455667788,
        "ff000000000000000000000000000000",
        0x801,
    ),
    (
        "g096_istri_ic0",
        "660f3a63cac0",
        0x0,
        0x0,
        "20ff4241414239800000000000000000",
        "ff42ffff8042ff617f426261304120ff",
        false,
        0xf,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x881,
    ),
    (
        "g097_istrm_ic3",
        "660f3a62cac3",
        0x0,
        0x0,
        "01000000000000000000000000000000",
        "61000041ffff61000001000000000000",
        false,
        0x1122334455667788,
        "00000000000000000000000000000000",
        0xc0,
    ),
    (
        "g098_estri_ic4",
        "66480f3a61cac4",
        0x5,
        0x9,
        "80612039200000000000000000000000",
        "61616162000000000000000000000000",
        false,
        0x10,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc0,
    ),
    (
        "g099_estrm_ic7",
        "660f3a60cac7",
        0x8,
        0x3,
        "004100410001ffff0080010000800041",
        "0041ffff000000000000000000000000",
        false,
        0x1122334455667788,
        "ffffffffffff00000000000000000000",
        0x841,
    ),
    (
        "g100_istri_ic8",
        "660f3a630bc8",
        0x0,
        0x0,
        "30000000000000000000000000000000",
        "41614239000000000000000000000000",
        true,
        0xf,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc1,
    ),
    (
        "g101_istrm_icb",
        "660f3a62cacb",
        0x0,
        0x0,
        "ff7f0080620000416200000100000000",
        "00010080620001000080000000000000",
        false,
        0x1122334455667788,
        "0000ffffffff000000000000ffffffff",
        0xc1,
    ),
    (
        "g102_estri_icc",
        "660f3a61cacc",
        0x0,
        0x9,
        "397f426239ff426261806280ffff0000",
        "61396230803920304239394220390000",
        false,
        0xf,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x8c1,
    ),
    (
        "g103_estrm_icf",
        "660f3a60cacf",
        0x9,
        0x9,
        "00010000000000000000000000000000",
        "01000000000000000000000000000000",
        false,
        0x1122334455667788,
        "00000000000000000000000000000000",
        0x0,
    ),
    (
        "g104_istri_id0",
        "660f3a63cad0",
        0x0,
        0x0,
        "61420000000000000000000000000000",
        "42ff0000000000000000000000000000",
        false,
        0xf,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc1,
    ),
    (
        "g105_istrm_id3",
        "660f3a620bd3",
        0x0,
        0x0,
        "00800080ffffffff0000000000000000",
        "00000000000000000000000000000000",
        true,
        0x1122334455667788,
        "ffffffffffffffffffffffffffffffff",
        0x8c1,
    ),
    (
        "g106_estri_id4",
        "660f3a61cad4",
        0x0,
        0x11,
        "417f0000000000000000000000000000",
        "6180417f7f7f39618030204130428030",
        false,
        0xf,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x881,
    ),
    (
        "g107_estrm_id7",
        "660f3a60cad7",
        0x11,
        0x11,
        "00800001ff7f00016100ff7f6200ff7f",
        "00010001ff7f00806200004100010000",
        false,
        0x1122334455667788,
        "00000000000000000000000000000000",
        0x0,
    ),
    (
        "g108_istri_id8",
        "660f3a63cad8",
        0x0,
        0x0,
        "616139807f0000000000000000000000",
        "ffffff7f8042806142392061ff000000",
        false,
        0xc,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x8c1,
    ),
    (
        "g109_istrm_idb",
        "660f3a62cadb",
        0x0,
        0x0,
        "00410000000000000000000000000000",
        "6200ff7fffff62000080ffff00000000",
        false,
        0x1122334455667788,
        "ffffffffffffffffffffffff00000000",
        0x8c1,
    ),
    (
        "g110_estri_idc",
        "660f3a610bdc",
        0x0,
        0x3,
        "00000000000000000000000000000000",
        "207f0000000000000000000000000000",
        true,
        0x10,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc0,
    ),
    (
        "g111_estrm_idf",
        "660f3a60cadf",
        0x1,
        0x0,
        "00000000000000000000000000000000",
        "61000001610062000080000000000000",
        false,
        0x1122334455667788,
        "ffffffffffffffffffffffffffffffff",
        0x8c1,
    ),
    (
        "g112_istri_ie0",
        "660f3a63cae0",
        0x0,
        0x0,
        "41000000000000000000000000000000",
        "00000000000000000000000000000000",
        false,
        0x10,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc0,
    ),
    (
        "g113_istrm_ie3",
        "660f3a62cae3",
        0x0,
        0x0,
        "00410000000000000000000000000000",
        "000100010041008000410100ff7f0000",
        false,
        0x1122334455667788,
        "00000000ffff0000ffff000000000000",
        0xc1,
    ),
    (
        "g114_estri_ie4",
        "660f3a61cae4",
        0x0,
        0x9,
        "62204230393930000000000000000000",
        "6120423041ff00000000000000000000",
        false,
        0x10,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc0,
    ),
    (
        "g115_estrm_ie7",
        "660f3a600be7",
        0x6,
        0x8,
        "00806200010000416200000000000000",
        "61000001ff7f01000001ff7f61006200",
        true,
        0x1122334455667788,
        "ffffffff0000ffffffff0000ffffffff",
        0x881,
    ),
    (
        "g116_istri_ie8",
        "660f3a63cae8",
        0x0,
        0x0,
        "3041394162307f4241ff4162ff610000",
        "ff612061ff7f61ff4261207f7f616142",
        false,
        0xd,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x81,
    ),
    (
        "g117_istrm_ieb",
        "660f3a62caeb",
        0x0,
        0x0,
        "6100ff7f620000410000000000000000",
        "00416100620061000100ffffffff0000",
        false,
        0x1122334455667788,
        "00000000ffff0000000000000000ffff",
        0xc1,
    ),
    (
        "g118_estri_iec",
        "660f3a61caec",
        0xfffffffffffffff4,
        0xfffffffffffffffd,
        "423930427f62ff8042ffff7f00000000",
        "80613900000000000000000000000000",
        false,
        0x10,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc0,
    ),
    (
        "g119_estrm_ief",
        "66480f3a60caef",
        0xfffffffffffffffd,
        0x11,
        "61000100ff7f00000000000000000000",
        "0100ff7fffff00800100000000000000",
        false,
        0x1122334455667788,
        "00000000000000000000000000000000",
        0x80,
    ),
    (
        "g120_istri_if0",
        "660f3a630bf0",
        0x0,
        0x0,
        "00000000000000000000000000000000",
        "7f62627f20302030ff4139ff7f303900",
        true,
        0xe,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x8c1,
    ),
    (
        "g121_istrm_if3",
        "660f3a62caf3",
        0x0,
        0x0,
        "01000000000000000000000000000000",
        "0041010001000080ffff010000000000",
        false,
        0x1122334455667788,
        "ffff00000000ffffffff000000000000",
        0x8c1,
    ),
    (
        "g122_estri_if4",
        "660f3a61caf4",
        0xf,
        0x1,
        "3939807fffffff7f7f30423041418000",
        "20000000000000000000000000000000",
        false,
        0x0,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x8c1,
    ),
    (
        "g123_estrm_if7",
        "660f3a60caf7",
        0x0,
        0x9,
        "010001000080ffff6200008000000000",
        "0041ffffff7f00806200610000000000",
        false,
        0x1122334455667788,
        "ffffffffffffffffffffffffffffffff",
        0x881,
    ),
    (
        "g124_istri_if8",
        "660f3a63caf8",
        0x0,
        0x0,
        "417f0000000000000000000000000000",
        "62627f30423030804142416139413000",
        false,
        0xf,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x8c1,
    ),
    (
        "g125_istrm_ifb",
        "660f3a620bfb",
        0x0,
        0x0,
        "0041ff7fffff00000000000000000000",
        "62000041000000000000000000000000",
        true,
        0x1122334455667788,
        "ffffffff0000ffffffffffffffffffff",
        0x8c1,
    ),
    (
        "g126_estri_ifc",
        "66480f3a61cafc",
        0xa,
        0xfffffffffffffffa,
        "61ff62422041ffff207f000000000000",
        "62204180428000000000000000000000",
        false,
        0x5,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x8c1,
    ),
    (
        "g127_estrm_iff",
        "660f3a60caff",
        0xffffffffffffffff,
        0x7,
        "00800000000000000000000000000000",
        "01000001008061000001008000010000",
        false,
        0x1122334455667788,
        "ffffffff0000ffffffff0000ffff0000",
        0x8c1,
    ),
    (
        "g128_istri_ie7",
        "660f3a63cae7",
        0x0,
        0x0,
        "0080ff7f00800100ff7f000000000000",
        "62006200ff7f61000000000000000000",
        false,
        0x3,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x8c1,
    ),
    (
        "g129_istrm_if9",
        "660f3a62caf9",
        0x0,
        0x0,
        "62000001610062000000000000000000",
        "ff7f0041620000000000000000000000",
        false,
        0x1122334455667788,
        "ffffffffffff0000ffffffffffffffff",
        0x8c1,
    ),
    (
        "g130_estri_iaa",
        "660f3a610baa",
        0x0,
        0xc,
        "628030ff7f42303980ff41ff30000000",
        "41627f4130423962427f620000000000",
        true,
        0xc,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc1,
    ),
    (
        "g131_estrm_i7f",
        "660f3a60ca7f",
        0xfffffffffffffff8,
        0x3,
        "0080610000416100ff7f004100800041",
        "ff7f0080000100000000000000000000",
        false,
        0x1122334455667788,
        "ffffffffffff00000000000000000000",
        0x841,
    ),
    (
        "g132_istri_ifc",
        "660f3a63cafc",
        0x0,
        0x0,
        "20423041200000000000000000000000",
        "30393900000000000000000000000000",
        false,
        0x2,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x8c1,
    ),
    (
        "g133_istrm_i64",
        "660f3a62ca64",
        0x0,
        0x0,
        "62ff2080306141000000000000000000",
        "80624230ff3941300000000000000000",
        false,
        0x1122334455667788,
        "ffffffffffffffff0000000000000000",
        0x8c1,
    ),
    (
        "g134_estri_ib6",
        "660f3a61cab6",
        0x11,
        0x9,
        "807f4230303080626130804242ff0000",
        "ff6162428039ff306262426180623000",
        false,
        0x10,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x40,
    ),
    (
        "g135_estrm_ied",
        "660f3a600bed",
        0x11,
        0x6,
        "008061000041004100010080ff7fffff",
        "01000041ff7f00806200004100000000",
        true,
        0x1122334455667788,
        "00000000000000000000000000000000",
        0x40,
    ),
    (
        "g136_istri_ic5",
        "660f3a63cac5",
        0x0,
        0x0,
        "0100ffff010000010001ffff01000001",
        "61000001000000000000000000000000",
        false,
        0x1,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x841,
    ),
    (
        "g137_istrm_i62",
        "660f3a62ca62",
        0x0,
        0x0,
        "61416280392080ff3000000000000000",
        "39306142807f41396141000000000000",
        false,
        0x1122334455667788,
        "ffffff00ff00ffffffff000000000000",
        0x8c1,
    ),
    (
        "g138_estri_i35",
        "660f3a61ca35",
        0x0,
        0xfffffffffffffffd,
        "62000080ff7f01000100000000000000",
        "01000001620000000000000000000000",
        false,
        0x0,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x8c1,
    ),
    (
        "g139_estrm_i61",
        "660f3a60ca61",
        0x9,
        0x0,
        "620061000041ff7fff7f000100000000",
        "00410100620001000000000000000000",
        false,
        0x1122334455667788,
        "00000000000000000000000000000000",
        0x40,
    ),
    (
        "g140_istri_i51",
        "660f3a630b51",
        0x0,
        0x0,
        "00410100000100800080610000806100",
        "00416100004100000000000000000000",
        true,
        0x7,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x41,
    ),
    (
        "g141_istrm_if4",
        "660f3a62caf4",
        0x0,
        0x0,
        "61ff8062616130423000000000000000",
        "00000000000000000000000000000000",
        false,
        0x1122334455667788,
        "00000000000000000000000000000000",
        0xc0,
    ),
    (
        "g142_estri_i7b",
        "660f3a61ca7b",
        0x11,
        0x9,
        "ffffff7f6100ffff0041008061000000",
        "00000000000000000000000000000000",
        false,
        0x6,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x801,
    ),
    (
        "g143_estrm_idf",
        "660f3a60cadf",
        0x0,
        0x4,
        "0100ff7f610061000000000000000000",
        "61000100010000000000000000000000",
        false,
        0x1122334455667788,
        "00000000000000000000000000000000",
        0xc0,
    ),
    (
        "g144_istri_i84",
        "660f3a63ca84",
        0x0,
        0x0,
        "41626142000000000000000000000000",
        "41307fff80207f6220807f8039000000",
        false,
        0x0,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x8c1,
    ),
    (
        "g145_istrm_id2",
        "660f3a620bd2",
        0x0,
        0x0,
        "42417f427f627f000000000000000000",
        "ff393061ff3062616180000000000000",
        true,
        0x1122334455667788,
        "ffffffffffff00ffffffffffffffffff",
        0x8c1,
    ),
    (
        "g146_estri_ia5",
        "660f3a61caa5",
        0x11,
        0x0,
        "6100010000800100ffffffff00000000",
        "0100ffffffffffff0000000000000000",
        false,
        0x8,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x40,
    ),
    (
        "g147_estrm_i2f",
        "66480f3a60ca2f",
        0xfffffffffffffff9,
        0x9,
        "610062000041004101000041ff7f0000",
        "0001ffff000000000000000000000000",
        false,
        0x1122334455667788,
        "00000000000000000000000000000000",
        0x80,
    ),
    (
        "g148_istri_i79",
        "660f3a63ca79",
        0x0,
        0x0,
        "6100ffff6200ffff0000000000000000",
        "6200ffff0041ffff0100ffff00010001",
        false,
        0x7,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x881,
    ),
    (
        "g149_istrm_if7",
        "660f3a62caf7",
        0x0,
        0x0,
        "0080ff7f620000800100000000000000",
        "6200ff7f008000010080004100800000",
        false,
        0x1122334455667788,
        "00000000000000000000000000000000",
        0xc0,
    ),
    (
        "g150_estri_i32",
        "660f3a610b32",
        0x0,
        0x9,
        "ffffff80393061ff6180413000000000",
        "61207f7f808062802062ff4280803061",
        true,
        0x0,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x8c1,
    ),
    (
        "g151_estrm_i05",
        "660f3a60ca05",
        0x9,
        0x6,
        "ffff6100010000010080000000000000",
        "00016200620061000001620000000000",
        false,
        0x1122334455667788,
        "3f000000000000000000000000000000",
        0x841,
    ),
    (
        "g152_istri_id5",
        "660f3a63cad5",
        0x0,
        0x0,
        "01000000000000000000000000000000",
        "00410041004101000041ffffff7f0000",
        false,
        0x7,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x8c1,
    ),
    (
        "g153_istrm_ibe",
        "660f3a62cabe",
        0x0,
        0x0,
        "61ff2080000000000000000000000000",
        "39396100000000000000000000000000",
        false,
        0x1122334455667788,
        "07000000000000000000000000000000",
        0x8c1,
    ),
    (
        "g154_estri_iec",
        "66480f3a61caec",
        0x9,
        0x0,
        "30410000000000000000000000000000",
        "ff422042304241426280394142000000",
        false,
        0x10,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc0,
    ),
    (
        "g155_estrm_i07",
        "660f3a600b07",
        0x1,
        0x2,
        "00000000000000000000000000000000",
        "00410080000000000000000000000000",
        true,
        0x1122334455667788,
        "00000000000000000000000000000000",
        0xc0,
    ),
    (
        "g156_istri_i95",
        "660f3a63ca95",
        0x0,
        0x0,
        "61000001000000000000000000000000",
        "00806100ffff00410000000000000000",
        false,
        0x0,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0x8c1,
    ),
    (
        "g157_istrm_i07",
        "660f3a62ca07",
        0x0,
        0x0,
        "61006100ffffff7f0080008000000000",
        "00016200000101006200000000000000",
        false,
        0x1122334455667788,
        "1f000000000000000000000000000000",
        0x8c1,
    ),
    (
        "g158_estri_i87",
        "660f3a61ca87",
        0x0,
        0x3,
        "0080ff7f010061000001000000000000",
        "ffff0080000000000000000000000000",
        false,
        0x8,
        "aa55aa55aa55aa55aa55aa55aa55aa55",
        0xc0,
    ),
    (
        "g159_estrm_i9a",
        "660f3a60ca9a",
        0x2,
        0x9,
        "61000000000000000000000000000000",
        "39398030307f61ffff80200000000000",
        false,
        0x1122334455667788,
        "ff010000000000000000000000000000",
        0x8c1,
    ),
];
