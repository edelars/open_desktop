//! S7-32 slice P (ISSUE-704): `REPE SCAS` (`F3 AE`, `F3 [66|REX.W] AF`, with and without `67`)
//! through the CPL0 profile set from any state of the arithmetic flags, checked against an
//! independent model written from the Intel SDM (325462-093US), Vol. 2B:
//!
//! - "REPE/REPZ—Repeat String Operation While Zero (Prefix)" (p. 4-565): the instruction repeats
//!   until the count register (RCX, or ECX with `67`) is 0 or ZF is clear; "The ZF flag does not
//!   require initialization because both the CMPS and SCAS instructions affect the ZF flag
//!   according to the results of the comparisons they make"; with a zero count nothing runs.
//! - "SCAS/SCASB/SCASW/SCASD—Scan String" (p. 4-616..4-618): `temp := AL/AX/EAX/RAX − SRC;
//!   SetStatusFlags(temp)` with SRC at ES:(E/R)DI, then (E/R)DI ± operand size by DF.
//! - The status flags of the subtraction as `CMP` sets them (CF borrow, PF parity of the low
//!   byte, AF borrow out of bit 3, ZF, SF, OF signed overflow).
//!
//! With `67` the model writes back zero-extended EDI and ECX, as the existing address-size-32
//! contracts of the interpreter (`CPU_CONTRACT.md`, S2-19m and S2-19o) say.

use od_isa::execute::{
    ControlRegisterPolicy, Cpl0ProfileSet, Cpl0ProfileSetExit, RunConfig, RunExit,
    run_with_cpl0_profile_set,
};
use od_isa::{
    AccessContext, AccessKind, AccessOrigin, ArchitecturalFault, CpuActivity, GuestMemory,
    InstructionMode, LongModeState, MemoryError, OperandSize, PrivilegeLevel, Register, StiShadow,
};

const CODE: u64 = 0x1000;
const CR0_BASE: u64 = 0x8001_0033;
const CR4_BASE: u64 = 0x20;
const CF: u64 = 1;
const PF: u64 = 1 << 2;
const AF: u64 = 1 << 4;
const ZF: u64 = 1 << 6;
const SF: u64 = 1 << 7;
const OF: u64 = 1 << 11;
const DF: u64 = 1 << 10;
const ARITHMETIC: u64 = CF | PF | AF | ZF | SF | OF;
const FLAG_BITS: [u64; 6] = [CF, PF, AF, ZF, SF, OF];
/// The iterations one step of a REP string instruction runs before it returns a partial exit.
const WORK_LIMIT: u64 = od_isa::execute::REP_ITERATION_WORK_LIMIT;

/// Code at `CODE`; data in separate regions; every other address is unmapped. Counts reads.
struct Memory {
    code: Vec<u8>,
    regions: Vec<(u64, Vec<u8>)>,
    /// Addresses `[start, end)` whose reads raise an architectural `#PF` (not present, read).
    faulting: Option<(u64, u64)>,
    reads: usize,
    writes: usize,
}

impl Memory {
    fn new(code: &[u8]) -> Self {
        Self {
            code: code.to_vec(),
            regions: Vec::new(),
            faulting: None,
            reads: 0,
            writes: 0,
        }
    }

    fn faulting(mut self, start: u64, end: u64) -> Self {
        self.faulting = Some((start, end));
        self
    }

    fn region(mut self, base: u64, bytes: Vec<u8>) -> Self {
        self.regions.push((base, bytes));
        self
    }

    fn byte(&self, address: u64) -> Option<u8> {
        self.regions.iter().find_map(|(base, bytes)| {
            let offset = address.wrapping_sub(*base);
            (offset < bytes.len() as u64).then(|| bytes[offset as usize])
        })
    }

    fn element(&self, address: u64, width: u64) -> u64 {
        (0..width).fold(0, |value, index| {
            value | u64::from(self.byte(address + index).expect("mapped")) << (8 * index)
        })
    }
}

impl GuestMemory for Memory {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        let offset = address.wrapping_sub(CODE) as usize;
        self.code.get(offset).copied().ok_or(MemoryError::Unmapped {
            address,
            access: AccessKind::Fetch,
        })
    }

    fn read(&mut self, address: u64, _: AccessContext, out: &mut [u8]) -> Result<(), MemoryError> {
        if let Some((start, end)) = self.faulting
            && address < end
            && address.wrapping_add(out.len() as u64) > start
        {
            return Err(MemoryError::Architectural(ArchitecturalFault {
                vector: 14,
                error_code: Some(0),
                linear_address: Some(address.max(start)),
            }));
        }
        for (index, byte) in out.iter_mut().enumerate() {
            *byte = self
                .byte(address.wrapping_add(index as u64))
                .ok_or(MemoryError::Unmapped {
                    address,
                    access: AccessKind::Read,
                })?;
        }
        self.reads += 1;
        Ok(())
    }

    fn write(&mut self, address: u64, _: AccessContext, _: &[u8]) -> Result<(), MemoryError> {
        self.writes += 1;
        Err(MemoryError::Unmapped {
            address,
            access: AccessKind::Write,
        })
    }

    fn atomic_exchange_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        unreachable!()
    }

    fn atomic_fetch_add_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        unreachable!()
    }

    fn atomic_compare_exchange_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: OperandSize,
        _: u64,
        _: u64,
    ) -> Result<u64, MemoryError> {
        unreachable!()
    }

    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        unreachable!()
    }
}

fn state(rflags: u64) -> LongModeState {
    LongModeState {
        rip: CODE,
        rflags,
        cr0: Some(CR0_BASE),
        cr4: Some(CR4_BASE),
        activity: Some(CpuActivity::Running),
        sti_shadow: Some(StiShadow::Clear),
        ..LongModeState::default()
    }
}

/// Runs the set until it leaves `RepIterationBudgetExhausted` (the partial REP exit, which a
/// board resumes) or reaches `stop_rip`. Returns the exit, the defined-flags mask, the retired
/// instructions and the number of partial REP exits.
fn run(
    state: &mut LongModeState,
    memory: &mut Memory,
    budget: u64,
    stop_rip: Option<u64>,
    defined: u64,
) -> (RunExit, u64, u64, u64) {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let (mut defined, mut retired, mut partial) = (defined, 0, 0);
    loop {
        let result = run_with_cpl0_profile_set(
            state,
            memory,
            RunConfig {
                instruction_budget: budget - retired,
                stop_rip,
                context: AccessContext {
                    mode: InstructionMode::Long64,
                    privilege: PrivilegeLevel::Ring0,
                    origin: AccessOrigin::Explicit,
                },
            },
            defined,
            &set,
            None,
            || false,
        )
        .expect("the state is inside the set");
        defined = result.rflags_defined;
        retired += result.instructions_executed;
        let Cpl0ProfileSetExit::Run(exit) = result.exit else {
            panic!("expected a Run exit");
        };
        if exit != RunExit::RepIterationBudgetExhausted {
            return (exit, defined, retired, partial);
        }
        // At a partial REP boundary the six flags are those of the last comparison: defined,
        // whatever they were at entry (an interrupt may be delivered here).
        assert_eq!(defined & ARITHMETIC, ARITHMETIC, "partial exit {partial}");
        partial += 1;
    }
}

/// The six status flags of `a - b` at `width` bytes, as `CMP` sets them (SDM Vol. 2A/2B).
fn cmp_flags(width: u64, a: u64, b: u64) -> u64 {
    let bits = 8 * width;
    let mask = if bits == 64 {
        u64::MAX
    } else {
        (1 << bits) - 1
    };
    let (a, b) = (a & mask, b & mask);
    let r = a.wrapping_sub(b) & mask;
    let sign = 1u64 << (bits - 1);
    let mut flags = 0;
    if a < b {
        flags |= CF;
    }
    if (r as u8).count_ones() % 2 == 0 {
        flags |= PF;
    }
    if (a ^ b ^ r) & 0x10 != 0 {
        flags |= AF;
    }
    if r == 0 {
        flags |= ZF;
    }
    if r & sign != 0 {
        flags |= SF;
    }
    if (a ^ b) & (a ^ r) & sign != 0 {
        flags |= OF;
    }
    flags
}

/// The result of `REPE SCAS` by the SDM pseudocode: RCX, RDI, RFLAGS and the element reads.
fn model(
    memory: &Memory,
    width: u64,
    address32: bool,
    rax: u64,
    rcx: u64,
    rdi: u64,
    rflags: u64,
) -> (u64, u64, u64, usize) {
    let (mut rcx, mut rdi, mut rflags, mut reads) = (rcx, rdi, rflags, 0);
    let mut count = if address32 { rcx & 0xFFFF_FFFF } else { rcx };
    while count != 0 {
        let address = if address32 { rdi & 0xFFFF_FFFF } else { rdi };
        let element = memory.element(address, width);
        reads += 1;
        rflags = (rflags & !ARITHMETIC) | cmp_flags(width, rax, element);
        let step = if rflags & DF == 0 {
            width
        } else {
            width.wrapping_neg()
        };
        count -= 1;
        if address32 {
            rdi = (rdi as u32).wrapping_add(step as u32) as u64;
            rcx = count;
        } else {
            rdi = rdi.wrapping_add(step);
            rcx = count;
        }
        if rflags & ZF == 0 {
            break;
        }
    }
    (rcx, rdi, rflags, reads)
}

/// Every set encoding of `REPE SCAS`: (bytes, element width, address size 32).
const ENCODINGS: [(&[u8], u64, bool); 8] = [
    (&[0xf3, 0xae], 1, false),
    (&[0xf3, 0x66, 0xaf], 2, false),
    (&[0xf3, 0xaf], 4, false),
    (&[0xf3, 0x48, 0xaf], 8, false),
    (&[0xf3, 0x67, 0xae], 1, true),
    (&[0xf3, 0x67, 0x66, 0xaf], 2, true),
    (&[0xf3, 0x67, 0xaf], 4, true),
    (&[0xf3, 0x67, 0x48, 0xaf], 8, true),
];

/// The flag word of a six-bit pattern.
fn flags_of(pattern: u8) -> u64 {
    FLAG_BITS
        .iter()
        .enumerate()
        .filter(|(bit, _)| pattern & (1 << bit) != 0)
        .fold(0, |flags, (_, flag)| flags | flag)
}

/// How the differing element relates to the accumulator (whose sign bit is set and low byte
/// 0x35), so that the final comparison gives different flags: `Borrow` — element = accumulator +
/// 0x41: CF, AF, SF; `NoBorrow` — element = accumulator − 1: result 1, no flag but PF clear (odd
/// parity); `Overflow` — element = the largest positive value: negative minus positive gives a
/// positive result, OF without CF or SF.
#[derive(Clone, Copy, Debug)]
enum Mismatch {
    Borrow,
    NoBorrow,
    Overflow,
}

impl Mismatch {
    fn element(self, accumulator: u64, width: u64) -> u64 {
        let mask = if width == 8 {
            u64::MAX
        } else {
            (1u64 << (8 * width)) - 1
        };
        match self {
            Self::Borrow => accumulator.wrapping_add(0x41) & mask,
            Self::NoBorrow => accumulator.wrapping_sub(1) & mask,
            Self::Overflow => mask >> 1,
        }
    }
}

/// One `REPE SCAS` over a buffer of eight elements, `count` of them scanned (from the first for
/// DF = 0, from the last for DF = 1); `mismatch` is the position in scan order and the kind of the
/// element that differs from the accumulator, `None` for all equal.
fn scan_case(
    encoding: (&[u8], u64, bool),
    entry_flags: u64,
    df: bool,
    count: u64,
    mismatch: Option<(usize, Mismatch)>,
    defined: u64,
) {
    let (code, width, address32) = encoding;
    const BUFFER: u64 = 0x4000;
    let mask = if width == 8 {
        u64::MAX
    } else {
        (1u64 << (8 * width)) - 1
    };
    // Accumulator with bits above the element width (they must not take part).
    let accumulator = 0x8000_0000_0000_0000u64 >> (64 - 8 * width) | 0x35;
    let rax = 0xdead_beef_0000_0000u64.wrapping_mul(u64::from(width < 8)) | (accumulator & mask);
    let mut bytes = vec![0u8; 8 * 16];
    for index in 0..8usize {
        let mut value = accumulator & mask;
        if let Some((m, kind)) = mismatch
            && (if df { 7 - m } else { m }) == index
        {
            value = kind.element(accumulator, width);
        }
        bytes[index * width as usize..(index + 1) * width as usize]
            .copy_from_slice(&value.to_le_bytes()[..width as usize]);
    }
    let start = if df { BUFFER + 7 * width } else { BUFFER };
    let mut memory = Memory::new(code).region(BUFFER, bytes);
    let rflags = 2 | entry_flags | if df { DF } else { 0 } | (1 << 9);
    let mut state = state(rflags);
    state.gpr[Register::Rax as usize] = rax;
    state.gpr[Register::Rcx as usize] = count;
    state.gpr[Register::Rdi as usize] = start;
    let before = state.clone();
    let (rcx, rdi, flags, reads) = model(&memory, width, address32, rax, count, start, rflags);
    let (exit, after_defined, retired, partial) = run(&mut state, &mut memory, 1, None, defined);
    let what = format!(
        "{code:02x?} flags {entry_flags:#x} defined {defined:#x} df {df} count {count} mismatch {mismatch:?}"
    );
    assert_eq!(exit, RunExit::InstructionBudgetExhausted, "{what}");
    assert_eq!((retired, partial), (1, 0), "{what}");
    assert_eq!(state.rip, CODE + code.len() as u64, "{what}");
    assert_eq!(state.gpr[Register::Rcx as usize], rcx, "{what}");
    assert_eq!(state.gpr[Register::Rdi as usize], rdi, "{what}");
    assert_eq!(state.rflags, flags, "{what}");
    assert_eq!(memory.reads, reads, "{what}");
    assert_eq!(memory.writes, 0, "{what}");
    // Every other register is untouched, RAX included.
    let mut expected = before.gpr;
    expected[Register::Rcx as usize] = rcx;
    expected[Register::Rdi as usize] = rdi;
    assert_eq!(state.gpr, expected, "{what}");
    // The six are defined after the instruction (by the comparisons, or for a zero count by the
    // static effects at retirement, as for `REPE CMPS` since S7-31c); DF keeps its definedness.
    // For a zero count this pins the known deviation ISSUE-707 (the SDM leaves the flags, and so
    // their undefinedness, as they were): when ISSUE-707 is fixed, this expectation changes.
    assert_eq!(after_defined & ARITHMETIC, ARITHMETIC, "{what}");
    assert_eq!(after_defined & !ARITHMETIC, defined & !ARITHMETIC, "{what}");
    if count == 0 {
        assert_eq!(state, {
            let mut same = before.clone();
            same.rip = CODE + code.len() as u64;
            same
        });
    }
}

/// ISSUE-704: from every value of the six flags (64), five definedness masks (none, all, all but
/// OF — the state after `SHR r, 2` in `RtlCompareMemoryUlong` — all but AF, and CF|ZF), DF = 0
/// and 1, every width and both address sizes: a zero count, all equal, and a mismatch at the
/// first, a middle and the last of five iterations, each with three kinds of difference (borrow;
/// no borrow and odd parity; signed overflow).
#[test]
fn repe_scas_runs_in_the_set_from_every_entry_flag_state_and_matches_the_sdm_model() {
    let masks = [
        0,
        ARITHMETIC | DF,
        (ARITHMETIC & !OF) | DF,
        ARITHMETIC & !AF,
        CF | ZF,
    ];
    let mut scenarios = vec![(0, None), (5, None)];
    for position in [0, 2, 4] {
        for kind in [Mismatch::Borrow, Mismatch::NoBorrow, Mismatch::Overflow] {
            scenarios.push((5, Some((position, kind))));
        }
    }
    let mut runs = 0;
    for encoding in ENCODINGS {
        for pattern in 0u8..64 {
            for defined in masks {
                for df in [false, true] {
                    for &(count, mismatch) in &scenarios {
                        scan_case(encoding, flags_of(pattern), df, count, mismatch, defined);
                        runs += 1;
                    }
                }
            }
        }
    }
    assert_eq!(runs, 8 * 64 * 5 * 2 * 11);
}

/// The oracle of the model, by hand, for one case each way: `REPE SCASD` (`F3 AF`) of 0x35 over
/// [0x35, 0x35, 0x76]: three comparisons, the last 0x35 − 0x76 = 0xFFFFFFBF: CF (borrow), SF,
/// PF (0xBF has seven one bits: no), AF (5 < 6: borrow out of bit 3), not zero, no overflow.
#[test]
fn the_sdm_model_matches_a_hand_computed_case() {
    assert_eq!(cmp_flags(4, 0x35, 0x76), CF | AF | SF);
    assert_eq!(cmp_flags(1, 0x80, 0x01), OF | AF);
    assert_eq!(cmp_flags(8, 0, 0), ZF | PF);
    // The three mismatch kinds on a dword accumulator 0x80000035.
    let acc = 0x8000_0035;
    assert_eq!(
        cmp_flags(4, acc, Mismatch::Borrow.element(acc, 4)),
        CF | AF | SF
    );
    assert_eq!(cmp_flags(4, acc, Mismatch::NoBorrow.element(acc, 4)), 0);
    // 0x80000035 - 0x7FFFFFFF = 0x00000036: positive (OF), no borrow, AF (5 < 0xF), PF (0x36 has
    // four one bits).
    assert_eq!(
        cmp_flags(4, acc, Mismatch::Overflow.element(acc, 4)),
        OF | AF | PF
    );
    let memory = Memory::new(&[]).region(0x4000, {
        let mut bytes = vec![0u8; 12];
        bytes[0] = 0x35;
        bytes[4] = 0x35;
        bytes[8] = 0x76;
        bytes
    });
    assert_eq!(
        model(&memory, 4, false, 0x35, 5, 0x4000, 2 | ZF),
        (2, 0x400c, 2 | CF | AF | SF, 3)
    );
}

/// With `67` the count is ECX and the address EDI: the upper halves of RCX and RDI do not take
/// part, EDI wraps at 4 GiB up (DF = 0) and down through 0 (DF = 1), both are written back
/// zero-extended, and ECX = 0 leaves the whole RCX and RDI as they were (S2-19m/S2-19o).
#[test]
fn address_size_32_uses_ecx_and_edi_and_wraps_at_four_gibibytes() {
    // (DF, start EDI, dwords at 0xFFFFFFF8, 0xFFFFFFFC, 0x0, 0x4, expected ECX, EDI, reads)
    let cases = [
        // Up: 0xFFFFFFF8, 0xFFFFFFFC, 0x0 equal (the wrap), 0x4 differs.
        (
            false,
            0xFFFF_FFF8u64,
            [0x11u8, 0x11, 0x11, 0x22],
            3u64,
            0x8u64,
            4usize,
        ),
        // Down: 0x4, 0x0 equal, 0xFFFFFFFC equal (the wrap through 0), 0xFFFFFFF8 differs.
        (true, 0x4, [0x22, 0x11, 0x11, 0x11], 3, 0xFFFF_FFF4, 4),
    ];
    for (df, start, values, ecx, edi, element_reads) in cases {
        let mut memory = Memory::new(&[0xf3, 0x67, 0xaf])
            .region(0xFFFF_FFF8, vec![values[0], 0, 0, 0, values[1], 0, 0, 0])
            .region(0, vec![values[2], 0, 0, 0, values[3], 0, 0, 0]);
        // Arbitrary entry flags (OF|CF), none of them defined.
        let mut state = state(2 | OF | CF | if df { DF } else { 0 });
        state.gpr[Register::Rax as usize] = 0x11;
        state.gpr[Register::Rcx as usize] = 0xAAAA_0000_0000_0007;
        state.gpr[Register::Rdi as usize] = 0x5555_0000_0000_0000 | start;
        let (rcx, rdi, flags, reads) = model(
            &memory,
            4,
            true,
            0x11,
            state.gpr[Register::Rcx as usize],
            state.gpr[Register::Rdi as usize],
            state.rflags,
        );
        assert_eq!((rcx, rdi, reads), (ecx, edi, element_reads), "df {df}");
        let (exit, defined, _, _) = run(&mut state, &mut memory, 1, None, 0);
        assert_eq!(exit, RunExit::InstructionBudgetExhausted);
        assert_eq!(
            (
                state.gpr[Register::Rcx as usize],
                state.gpr[Register::Rdi as usize],
                state.rflags,
                memory.reads
            ),
            (rcx, rdi, flags, reads),
            "df {df}"
        );
        assert_eq!(defined & ARITHMETIC, ARITHMETIC);
    }
    // ECX = 0 with garbage above it: no iteration, RCX and RDI keep all 64 bits, no read.
    let mut memory = Memory::new(&[0xf3, 0x67, 0xaf]).region(0, vec![0; 8]);
    let mut state = state(2 | OF | CF);
    state.gpr[Register::Rcx as usize] = 0xAAAA_0000_0000_0000;
    state.gpr[Register::Rdi as usize] = 0x5555_0000_0000_0000;
    let before = state.clone();
    let (exit, _, _, _) = run(&mut state, &mut memory, 1, None, 0);
    assert_eq!(exit, RunExit::InstructionBudgetExhausted);
    assert_eq!(state.gpr, before.gpr);
    assert_eq!(state.rflags, before.rflags);
    assert_eq!(state.rip, CODE + 3);
    assert_eq!(memory.reads, 0);
}

/// `RtlCompareMemoryUlong` of the Windows kernel (`ntkrnlmp.pdb` + `0x6b7410`, the stop of
/// ISSUE-704), without its `push rdi`/`pop rdi`/`ret`: `mov rdi, rcx; shr rdx, 2; je end;
/// mov rcx, rdx; mov eax, r8d; rep scasd; je end; inc rcx; sub rdx, rcx; end: lea rax, [rdx*4]`.
/// It returns the number of leading bytes (in whole dwords) equal to the pattern. `shr rdx, 2`
/// leaves OF undefined, so `REP SCASD` starts from flags the old entry gate refused; counts up to
/// 0x10000 go through the work limit (partial exits resumed as a board does).
const RTL_COMPARE_MEMORY_ULONG: [u8; 33] = [
    0x48, 0x8b, 0xf9, // mov rdi, rcx
    0x48, 0xc1, 0xea, 0x02, // shr rdx, 2
    0x74, 0x10, // je +0x10
    0x48, 0x8b, 0xca, // mov rcx, rdx
    0x41, 0x8b, 0xc0, // mov eax, r8d
    0xf3, 0xaf, // rep scasd
    0x74, 0x06, // je +6
    0x48, 0xff, 0xc1, // inc rcx
    0x48, 0x2b, 0xd1, // sub rdx, rcx
    0x48, 0x8d, 0x04, 0x95, 0x00, 0x00, 0x00, 0x00, // lea rax, [rdx*4]
];

#[test]
fn rtl_compare_memory_ulong_of_the_windows_kernel_runs_through_the_set() {
    const BUFFER: u64 = 0x10_0000;
    let end = CODE + RTL_COMPARE_MEMORY_ULONG.len() as u64;
    // (length in bytes, dword index of the first difference, expected RAX)
    let cases: [(u64, Option<u64>, u64); 9] = [
        (0x40000, None, 0x40000),
        (0x40000, Some(0), 0),
        (0x40000, Some(WORK_LIMIT - 1), 4 * (WORK_LIMIT - 1)),
        (0x40000, Some(WORK_LIMIT), 4 * WORK_LIMIT),
        (0x40000, Some(0x8000), 0x20000),
        (0x40000, Some(0xFFFF), 0x3FFFC),
        (3, None, 0),
        (4, None, 4),
        (4, Some(0), 0),
    ];
    for entry in [0x206u64, 0x202, 2 | ARITHMETIC] {
        for (length, mismatch, expected) in cases {
            let mut bytes = vec![0u8; length as usize + 8];
            if let Some(index) = mismatch {
                bytes[4 * index as usize + 1] = 0x80;
            }
            let mut memory = Memory::new(&RTL_COMPARE_MEMORY_ULONG).region(BUFFER, bytes);
            let mut state = state(entry);
            state.gpr[Register::Rcx as usize] = BUFFER;
            state.gpr[Register::Rdx as usize] = length;
            state.gpr[Register::R8 as usize] = 0;
            // Entry definedness without OF (arbitrary code before the call); `shr rdx, 2` itself
            // leaves OF and AF undefined (`shr_before_rep_scasd_leaves_flags_the_old_gate_refused`).
            let (exit, _, _, partial) = run(
                &mut state,
                &mut memory,
                100,
                Some(end),
                od_isa::INTEGER_FLAGS & !OF,
            );
            let what = format!("entry {entry:#x} length {length:#x} mismatch {mismatch:?}");
            assert_eq!(exit, RunExit::StopAddress, "{what}");
            assert_eq!(state.rip, end, "{what}");
            assert_eq!(state.gpr[Register::Rax as usize], expected, "{what}");
            // One element read per iteration; the scan runs to the difference or the end.
            let iterations = if length < 4 {
                0
            } else {
                mismatch.map_or(length / 4, |index| index + 1)
            };
            assert_eq!(memory.reads as u64, iterations, "{what}");
            assert_eq!(
                partial,
                iterations.saturating_sub(1) / WORK_LIMIT,
                "{what}: partial REP exits"
            );
        }
    }
}

/// REPNE SCAS/CMPS are not forms of the set (the decoder has only their `67` spellings and the
/// set refuses them; `F2 AE`/`F2 AF` without `67` do not decode): they stay `Unsupported`
/// before any access, from any flags. This slice does not add them.
#[test]
fn repne_scas_and_cmps_stay_outside_the_set() {
    for (code, opcode) in [
        (&[0xf2, 0x67, 0xae][..], 0xae),
        (&[0xf2, 0x67, 0xaf][..], 0xaf),
        (&[0xf2, 0x67, 0xa6][..], 0xa6),
        (&[0xf2, 0x67, 0xa7][..], 0xa7),
        (&[0xf2, 0xae][..], 0xae),
        (&[0xf2, 0xaf][..], 0xaf),
    ] {
        for flags in [2, 2 | PF | ZF, 2 | ARITHMETIC] {
            let mut memory = Memory::new(code).region(0x4000, vec![0; 16]);
            let mut state = state(flags);
            state.gpr[Register::Rcx as usize] = 2;
            state.gpr[Register::Rdi as usize] = 0x4000;
            state.gpr[Register::Rsi as usize] = 0x4008;
            let before = state.clone();
            let (exit, _, retired, _) =
                run(&mut state, &mut memory, 1, None, od_isa::INTEGER_FLAGS);
            assert_eq!(exit, RunExit::Unsupported { opcode }, "{code:02x?}");
            assert_eq!(retired, 0);
            assert_eq!(state, before);
            assert_eq!(memory.reads, 0);
        }
    }
}

/// ISSUE-704, the entry of `REP SCASD` in `RtlCompareMemoryUlong`: the function's own
/// `shr rdx, 2` (SDM Vol. 2B, SAL/SAR/SHL/SHR, Flags Affected: "The OF flag is affected only for
/// 1-bit shifts …; otherwise, it is undefined", "For a non-zero count, the AF flag is undefined")
/// leaves OF and AF undefined and ZF = 0 (a nonzero count), so the entry gate kept outside the set
/// ("exactly PF|ZF, all six defined") fails on both the values and the definedness; the set runs
/// the scan from there.
#[test]
fn shr_before_rep_scasd_leaves_flags_the_old_gate_refused() {
    const BUFFER: u64 = 0x10_0000;
    const REP_SCASD: u64 = CODE + 15;
    for entry in [0x206u64, 2 | ARITHMETIC, 2 | PF | ZF] {
        let mut memory = Memory::new(&RTL_COMPARE_MEMORY_ULONG).region(BUFFER, vec![0; 0x48]);
        let mut state = state(entry);
        state.gpr[Register::Rcx as usize] = BUFFER;
        state.gpr[Register::Rdx as usize] = 0x40;
        // Every flag defined at the call: whatever is undefined at the REP comes from `shr`.
        let (exit, defined, _, _) = run(
            &mut state,
            &mut memory,
            100,
            Some(REP_SCASD),
            od_isa::INTEGER_FLAGS,
        );
        assert_eq!(exit, RunExit::StopAddress, "{entry:#x}");
        assert_eq!(state.rip, REP_SCASD);
        assert_eq!(
            defined & (OF | AF),
            0,
            "{entry:#x}: shr leaves OF and AF undefined"
        );
        assert_eq!(state.rflags & ZF, 0, "{entry:#x}: 0x40 >> 2 is not zero");
        assert_ne!(state.rflags & ARITHMETIC, PF | ZF, "{entry:#x}");
        let end = CODE + RTL_COMPARE_MEMORY_ULONG.len() as u64;
        let (exit, _, _, _) = run(&mut state, &mut memory, 100, Some(end), defined);
        assert_eq!(exit, RunExit::StopAddress, "{entry:#x}");
        assert_eq!(state.gpr[Register::Rax as usize], 0x40, "{entry:#x}");
        assert_eq!(memory.reads, 0x10);
    }
}

/// ISSUE-708 (a known deviation, pinned here): an architectural `#PF` on the k-th element of
/// `REPE SCAS` / `REPE CMPS` in the set. The SDM (Vol. 2B, REPE/REPZ, p. 4-566) says "When a
/// fault occurs during the execution of a CMPS or SCAS instruction that is prefixed with REPE or
/// REPZ, the EFLAGS value is restored to the state prior to the execution of the instruction";
/// the model keeps the flags of the last completed comparison and counts the six as defined
/// (RCX, RSI, RDI keep the progress, RIP stays on the instruction, as the SDM says). With the
/// fault on the first element nothing was compared: flags and their definedness are the entry's.
/// When ISSUE-708 is fixed, the expectations for k > 1 become the entry flags.
#[test]
fn a_fault_inside_repe_scas_and_cmps_keeps_the_last_comparison_issue_708() {
    const SRC: u64 = 0x4000;
    const DST: u64 = 0x6000;
    // (code, element width, CMPS: reads RSI first)
    let forms: [(&[u8], u64, bool); 4] = [
        (&[0xf3, 0xaf], 4, false),
        (&[0xf3, 0xae], 1, false),
        (&[0xf3, 0xa7], 4, true),
        (&[0xf3, 0xa6], 1, true),
    ];
    let entry_defined = od_isa::INTEGER_FLAGS & !(OF | AF);
    for (code, width, cmps) in forms {
        for k in [1u64, 2, 3, 5] {
            // Elements before the k-th compare equal (zeros everywhere); the k-th faults: for
            // SCAS the destination element, for CMPS the source element (read first).
            let faulting = if cmps { SRC } else { DST } + (k - 1) * width;
            let mut memory = Memory::new(code)
                .region(SRC, vec![0; 64])
                .region(DST, vec![0; 64])
                .faulting(faulting, faulting + width);
            let mut state = state(0x206);
            state.gpr[Register::Rax as usize] = 0;
            state.gpr[Register::Rcx as usize] = 5;
            state.gpr[Register::Rsi as usize] = SRC;
            state.gpr[Register::Rdi as usize] = DST;
            let (exit, defined, retired, _) = run(&mut state, &mut memory, 1, None, entry_defined);
            let what = format!("{code:02x?} fault on element {k}");
            assert_eq!(
                exit,
                RunExit::Fault(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(0),
                    linear_address: Some(faulting),
                }),
                "{what}"
            );
            assert_eq!((retired, state.rip), (0, CODE), "{what}");
            let done = k - 1;
            assert_eq!(state.gpr[Register::Rcx as usize], 5 - done, "{what}");
            assert_eq!(
                state.gpr[Register::Rdi as usize],
                DST + done * width,
                "{what}"
            );
            let rsi = if cmps { SRC + done * width } else { SRC };
            assert_eq!(state.gpr[Register::Rsi as usize], rsi, "{what}");
            if done == 0 {
                assert_eq!(state.rflags, 0x206, "{what}: nothing compared");
                assert_eq!(defined, entry_defined, "{what}");
            } else {
                // The flags of the last equal comparison (0 - 0): ZF|PF, not the entry 0x206.
                assert_eq!(state.rflags, 0x200 | 2 | ZF | PF, "{what}");
                assert_eq!(defined & ARITHMETIC, ARITHMETIC, "{what}");
            }
        }
    }
}
