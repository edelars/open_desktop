//! S2-21: CMPXCHG8B (`[F0] [REX without W] 0F C7 /1`, m64).
//!
//! Every expected value is written down from the Intel SDM text of CMPXCHG8B/CMPXCHG16B (Vol. 2A,
//! pp. 3-196..3-198; Intel SDM revision 093, combined volumes 325462-093US, Vol. 2A 253666-093US):
//!
//! * 0F C7 /1 CMPXCHG8B m64: compare EDX:EAX with m64; if equal, set ZF and load ECX:EBX into m64;
//!   else clear ZF and load m64 into EDX:EAX. EDX and ECX hold the high 32 bits.
//! * LOCK is allowed ("can be used with a LOCK prefix"); the destination receives a write cycle
//!   without regard to the result of the comparison.
//! * CF, PF, AF, SF and OF are unaffected.
//! * 64-bit mode exceptions: #SS(0) for a non-canonical address referencing SS; #GP(0) for a
//!   non-canonical address; #UD if the destination is not a memory operand; #PF on a page fault.
//!   There is no alignment requirement for the m64 form (unlike m128).
//!
//! The 32-bit-register behaviour in 64-bit mode (an unequal compare writes EAX/EDX through dword
//! writes, which zero the upper halves; an equal compare leaves RAX/RDX untouched; the upper halves
//! of RAX/RDX/RBX/RCX are not inputs) and the fault classes of unaligned, page-crossing and
//! protected operands are not in the SDM text and were taken from the hardware oracle of S2-21
//! (work/stage8/s2_21_cx8_oracle, byte for byte). The interpreter does not gate on CPUID.CX8
//! (OdX8664V1Neutral declares the bit, S6-14h; the Windows profile does not declare it until the
//! S2-57 acceptance). The SDM text of this instruction states no CX8 = 0 exception (unlike
//! CMPXCHG16B, whose CPUID flag is listed under #GP(0) in 64-bit mode), so no such case is
//! modelled or tested.

use od_isa::execute::{
    self, ControlRegisterPolicy, Cpl0ProfileSet, Cpl0ProfileSetExit, Cpl0ProfileSetInputError,
    Cr0Profile, Cr4Profile, RunConfig, RunExit, StepOutcome, run_with_cpl0_profile_set,
};
use od_isa::{
    AccessContext, AccessKind, ArchitecturalFault, CpuActivity, GuestMemory, LongModeState,
    MemoryError, OperandSize, PrivilegeLevel, Register, StiShadow,
};
use std::ops::Range;

const CF: u64 = 1;
const PF: u64 = 1 << 2;
const AF: u64 = 1 << 4;
const ZF: u64 = 1 << 6;
const SF: u64 = 1 << 7;
const OF: u64 = 1 << 11;
/// The flags CMPXCHG8B must leave alone.
const UNAFFECTED: u64 = CF | PF | AF | SF | OF;

const CODE: u64 = 0x3000;
const CELL: u64 = 0x1000;
const RAM_SIZE: usize = 0x4000;

/// `cmpxchg8b [rdi]` (RBX is the low half of the replacement, so it cannot be the address).
const CMPXCHG8B_RDI: [u8; 3] = [0x0f, 0xc7, 0x0f];

fn ring0() -> AccessContext {
    AccessContext {
        privilege: PrivilegeLevel::Ring0,
        ..AccessContext::default()
    }
}

/// Flat little-endian RAM with fault windows. An access that overlaps `absent` is
/// `Unmapped`; a read overlapping `not_present` is a #PF without the P bit; a write overlapping
/// `read_only` is a #PF with P and W set (error code 3). Every check covers the whole access, as
/// a page-crossing operand needs.
struct Ram {
    bytes: Vec<u8>,
    absent: Option<Range<u64>>,
    not_present: Option<Range<u64>>,
    read_only: Option<Range<u64>>,
    reads: Vec<(u64, usize)>,
    writes: Vec<(u64, Vec<u8>)>,
}

fn overlaps(window: &Option<Range<u64>>, address: u64, len: usize) -> bool {
    window
        .as_ref()
        .is_some_and(|w| address < w.end && address + len as u64 > w.start)
}

impl Ram {
    fn with_code(code: &[u8]) -> Self {
        let mut bytes = vec![0; RAM_SIZE];
        bytes[CODE as usize..CODE as usize + code.len()].copy_from_slice(code);
        Self {
            bytes,
            absent: None,
            not_present: None,
            read_only: None,
            reads: Vec::new(),
            writes: Vec::new(),
        }
    }

    fn set64(&mut self, address: u64, value: u64) {
        let at = address as usize;
        self.bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
    }

    fn get64(&self, address: u64) -> u64 {
        let at = address as usize;
        u64::from_le_bytes(self.bytes[at..at + 8].try_into().unwrap())
    }

    /// The outcome of a step whose memory access raised this #PF (the executor turns an
    /// architectural memory error into a delivered fault).
    fn page_fault(address: u64, error_code: u32) -> Result<StepOutcome, MemoryError> {
        Ok(StepOutcome::Fault(Self::fault(address, error_code)))
    }

    fn fault(address: u64, error_code: u32) -> ArchitecturalFault {
        ArchitecturalFault {
            vector: 14,
            error_code: Some(error_code),
            linear_address: Some(address),
        }
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
        if overlaps(&self.absent, address, out.len()) {
            return Err(MemoryError::Unmapped {
                address,
                access: AccessKind::Read,
            });
        }
        if overlaps(&self.not_present, address, out.len()) {
            return Err(MemoryError::Architectural(Self::fault(address, 0)));
        }
        let at = address as usize;
        let end = at
            .checked_add(out.len())
            .filter(|end| *end <= self.bytes.len());
        let Some(end) = end else {
            return Err(MemoryError::Unmapped {
                address,
                access: AccessKind::Read,
            });
        };
        out.copy_from_slice(&self.bytes[at..end]);
        Ok(())
    }

    fn write(&mut self, address: u64, _: AccessContext, source: &[u8]) -> Result<(), MemoryError> {
        if overlaps(&self.absent, address, source.len()) {
            return Err(MemoryError::Unmapped {
                address,
                access: AccessKind::Write,
            });
        }
        if overlaps(&self.not_present, address, source.len()) {
            return Err(MemoryError::Architectural(Self::fault(address, 2)));
        }
        if overlaps(&self.read_only, address, source.len()) {
            return Err(MemoryError::Architectural(Self::fault(address, 3)));
        }
        let at = address as usize;
        let end = at
            .checked_add(source.len())
            .filter(|end| *end <= self.bytes.len());
        let Some(end) = end else {
            return Err(MemoryError::Unmapped {
                address,
                access: AccessKind::Write,
            });
        };
        self.writes.push((address, source.to_vec()));
        self.bytes[at..end].copy_from_slice(source);
        Ok(())
    }

    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        Ok(())
    }
}

/// Wraps [`Ram`]: records the `atomic_compare_exchange_integer` calls and delegates them to the
/// inner default, so `inner.reads`/`inner.writes` hold only what that call did. Direct reads and
/// writes by the CPU land in `direct_reads`/`direct_writes`.
struct Counting {
    inner: Ram,
    cas_calls: Vec<(u64, OperandSize, u64, u64)>,
    direct_reads: usize,
    direct_writes: usize,
}

impl Counting {
    fn new(inner: Ram) -> Self {
        Self {
            inner,
            cas_calls: Vec::new(),
            direct_reads: 0,
            direct_writes: 0,
        }
    }
}

impl GuestMemory for Counting {
    fn fetch(&mut self, address: u64, context: AccessContext) -> Result<u8, MemoryError> {
        self.inner.fetch(address, context)
    }

    fn read(&mut self, a: u64, c: AccessContext, out: &mut [u8]) -> Result<(), MemoryError> {
        self.direct_reads += 1;
        self.inner.read(a, c, out)
    }

    fn write(&mut self, a: u64, c: AccessContext, source: &[u8]) -> Result<(), MemoryError> {
        self.direct_writes += 1;
        self.inner.write(a, c, source)
    }

    fn atomic_compare_exchange_integer(
        &mut self,
        address: u64,
        context: AccessContext,
        size: OperandSize,
        expected: u64,
        replacement: u64,
    ) -> Result<u64, MemoryError> {
        self.cas_calls.push((address, size, expected, replacement));
        self.inner
            .atomic_compare_exchange_integer(address, context, size, expected, replacement)
    }

    fn mfence(&mut self, context: AccessContext) -> Result<(), MemoryError> {
        self.inner.mfence(context)
    }
}

fn state() -> LongModeState {
    let mut state = LongModeState {
        rip: CODE,
        rflags: 0x2,
        ..LongModeState::default()
    };
    state.gpr[Register::Rdi as usize] = CELL;
    state
}

/// EDX:EAX and ECX:EBX are set from the 64-bit values; the upper halves of the four registers get
/// `garbage` (they are not inputs of the instruction).
fn set_pairs(state: &mut LongModeState, edx_eax: u64, ecx_ebx: u64, garbage: u64) {
    state.gpr[Register::Rax as usize] = (garbage << 32) | (edx_eax & 0xffff_ffff);
    state.gpr[Register::Rdx as usize] = (garbage << 32) | (edx_eax >> 32);
    state.gpr[Register::Rbx as usize] = (garbage << 32) | (ecx_ebx & 0xffff_ffff);
    state.gpr[Register::Rcx as usize] = (garbage << 32) | (ecx_ebx >> 32);
}

fn edx_eax(state: &LongModeState) -> u64 {
    ((state.gpr[Register::Rdx as usize] & 0xffff_ffff) << 32)
        | (state.gpr[Register::Rax as usize] & 0xffff_ffff)
}

fn step(
    state: &mut LongModeState,
    memory: &mut impl GuestMemory,
) -> Result<StepOutcome, MemoryError> {
    execute::step(state, memory, ring0())
}

fn with_lock(code: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0xf0];
    bytes.extend_from_slice(code);
    bytes
}

const MEMORY_VALUE: u64 = 0x1122_3344_5566_7788;
const REPLACEMENT: u64 = 0xf0e1_d2c3_b4a5_9687;
const GARBAGE: u64 = 0xdead_beef;

// ---- the two outcomes (SDM Operation: ZF := 1; DEST := ECX:EBX / ZF := 0; EDX:EAX := DEST) ----

#[test]
fn equal_operand_is_replaced_and_sets_zf_without_touching_rax_rdx_and_other_flags() {
    for code in [CMPXCHG8B_RDI.to_vec(), with_lock(&CMPXCHG8B_RDI)] {
        for initial_flags in [0x2, 0x2 | UNAFFECTED, 0x2 | ZF, 0x2 | ZF | UNAFFECTED] {
            let mut cpu = state();
            cpu.rflags = initial_flags;
            set_pairs(&mut cpu, MEMORY_VALUE, REPLACEMENT, GARBAGE);
            let (rax, rdx) = (
                cpu.gpr[Register::Rax as usize],
                cpu.gpr[Register::Rdx as usize],
            );
            let mut ram = Ram::with_code(&code);
            ram.set64(CELL, MEMORY_VALUE);
            let mut memory = Counting::new(ram);
            assert_eq!(step(&mut cpu, &mut memory), Ok(StepOutcome::Advanced));
            // DEST := ECX:EBX; ZF := 1; RAX/RDX untouched (upper halves included).
            assert_eq!(memory.inner.get64(CELL), REPLACEMENT);
            assert_eq!(cpu.gpr[Register::Rax as usize], rax);
            assert_eq!(cpu.gpr[Register::Rdx as usize], rdx);
            assert_eq!(cpu.rflags, initial_flags | ZF);
            assert_eq!(cpu.rflags & UNAFFECTED, initial_flags & UNAFFECTED);
            assert_eq!(
                cpu.gpr[Register::Rbx as usize] & 0xffff_ffff,
                REPLACEMENT & 0xffff_ffff
            );
            assert_eq!(
                cpu.gpr[Register::Rcx as usize] & 0xffff_ffff,
                REPLACEMENT >> 32
            );
            assert_eq!(cpu.rip, CODE + code.len() as u64);
        }
    }
}

#[test]
fn unequal_operand_is_loaded_into_edx_eax_clears_zf_and_is_written_back_unchanged() {
    // The comparison is over all 64 bits: a difference in only one dword must already fail.
    let low_differs = MEMORY_VALUE ^ 1;
    let high_differs = MEMORY_VALUE ^ (1u64 << 63);
    let both_differ = !MEMORY_VALUE;
    for code in [CMPXCHG8B_RDI.to_vec(), with_lock(&CMPXCHG8B_RDI)] {
        for expected in [low_differs, high_differs, both_differ] {
            for initial_flags in [0x2, 0x2 | ZF, 0x2 | UNAFFECTED, 0x2 | ZF | UNAFFECTED] {
                let mut cpu = state();
                cpu.rflags = initial_flags;
                set_pairs(&mut cpu, expected, REPLACEMENT, GARBAGE);
                let (rbx, rcx) = (
                    cpu.gpr[Register::Rbx as usize],
                    cpu.gpr[Register::Rcx as usize],
                );
                let mut ram = Ram::with_code(&code);
                ram.set64(CELL, MEMORY_VALUE);
                let mut memory = Counting::new(ram);
                assert_eq!(step(&mut cpu, &mut memory), Ok(StepOutcome::Advanced));
                // ZF := 0; EDX:EAX := DEST through dword writes (RAX/RDX upper halves become 0);
                // DEST := DEST (a write cycle happens anyway).
                assert_eq!(cpu.gpr[Register::Rax as usize], MEMORY_VALUE & 0xffff_ffff);
                assert_eq!(cpu.gpr[Register::Rdx as usize], MEMORY_VALUE >> 32);
                assert_eq!(cpu.rflags, initial_flags & !ZF);
                assert_eq!(memory.inner.get64(CELL), MEMORY_VALUE);
                assert_eq!(
                    memory.inner.writes,
                    vec![(CELL, MEMORY_VALUE.to_le_bytes().to_vec())]
                );
                // ECX:EBX is the source and is never modified.
                assert_eq!(cpu.gpr[Register::Rbx as usize], rbx);
                assert_eq!(cpu.gpr[Register::Rcx as usize], rcx);
                assert_eq!(cpu.rip, CODE + code.len() as u64);
            }
        }
    }
}

#[test]
fn the_upper_halves_of_rax_rdx_rbx_rcx_are_not_inputs() {
    // Equal in the low dwords, different garbage above: still equal; the stored value uses only
    // the low dwords of RBX and RCX.
    let mut cpu = state();
    set_pairs(&mut cpu, MEMORY_VALUE, REPLACEMENT, 0xffff_ffff);
    cpu.gpr[Register::Rdx as usize] ^= 0x1234_5678 << 32;
    cpu.gpr[Register::Rcx as usize] ^= 0x8765_4321 << 32;
    let mut ram = Ram::with_code(&CMPXCHG8B_RDI);
    ram.set64(CELL, MEMORY_VALUE);
    let mut memory = Counting::new(ram);
    assert_eq!(step(&mut cpu, &mut memory), Ok(StepOutcome::Advanced));
    assert_eq!(memory.inner.get64(CELL), REPLACEMENT);
    assert_ne!(cpu.rflags & ZF, 0);
    // Upper halves equal to the memory dwords but low halves different: not equal.
    let mut cpu = state();
    cpu.gpr[Register::Rax as usize] = (MEMORY_VALUE & 0xffff_ffff) << 32;
    cpu.gpr[Register::Rdx as usize] = MEMORY_VALUE & 0xffff_ffff_0000_0000;
    let mut ram = Ram::with_code(&CMPXCHG8B_RDI);
    ram.set64(CELL, MEMORY_VALUE);
    let mut memory = Counting::new(ram);
    assert_eq!(step(&mut cpu, &mut memory), Ok(StepOutcome::Advanced));
    assert_eq!(cpu.rflags & ZF, 0);
    assert_eq!(memory.inner.get64(CELL), MEMORY_VALUE);
}

#[test]
fn it_is_one_eight_byte_compare_exchange_call_and_never_direct_accesses() {
    for equal in [true, false] {
        let mut cpu = state();
        let expected = if equal { MEMORY_VALUE } else { 0 };
        set_pairs(&mut cpu, expected, REPLACEMENT, 0);
        let mut ram = Ram::with_code(&CMPXCHG8B_RDI);
        ram.set64(CELL, MEMORY_VALUE);
        let mut memory = Counting::new(ram);
        assert_eq!(step(&mut cpu, &mut memory), Ok(StepOutcome::Advanced));
        assert_eq!(
            memory.cas_calls,
            vec![(CELL, OperandSize::Qword, expected, REPLACEMENT)]
        );
        assert_eq!((memory.direct_reads, memory.direct_writes), (0, 0));
        // The default implementation: exactly one 8-byte read and one 8-byte write (the code
        // fetch goes through `fetch`, not `read`).
        assert_eq!(memory.inner.reads, vec![(CELL, 8)]);
        assert_eq!(memory.inner.writes.len(), 1);
        assert_eq!(memory.inner.writes[0].1.len(), 8);
    }
}

#[test]
fn the_operand_address_follows_the_modrm_forms() {
    type Setup = fn(&mut LongModeState);
    // [rip+disp32] is relative to the end of the whole instruction.
    let rip_relative_disp = (CELL as i64 - (CODE as i64 + 7)) as i32;
    let mut rip_code = vec![0x0f, 0xc7, 0x0d];
    rip_code.extend_from_slice(&rip_relative_disp.to_le_bytes());
    let cases: Vec<(&str, Vec<u8>, Setup)> = vec![
        ("[rdi]", CMPXCHG8B_RDI.to_vec(), |_| {}),
        ("[rbx]", vec![0x0f, 0xc7, 0x0b], |cpu| {
            cpu.gpr[Register::Rbx as usize] = CELL;
        }),
        ("[rsp]", vec![0x0f, 0xc7, 0x0c, 0x24], |cpu| {
            cpu.gpr[Register::Rsp as usize] = CELL;
        }),
        ("[r8] (REX.B)", vec![0x41, 0x0f, 0xc7, 0x08], |cpu| {
            cpu.gpr[Register::R8 as usize] = CELL;
        }),
        ("[rdi] (bare REX)", vec![0x40, 0x0f, 0xc7, 0x0f], |_| {}),
        ("[rbx+0x10]", vec![0x0f, 0xc7, 0x4b, 0x10], |cpu| {
            cpu.gpr[Register::Rbx as usize] = CELL - 0x10;
        }),
        ("[rbx+rsi*8] (SIB)", vec![0x0f, 0xc7, 0x0c, 0xf3], |cpu| {
            cpu.gpr[Register::Rbx as usize] = CELL - 0x20;
            cpu.gpr[Register::Rsi as usize] = 4;
        }),
        (
            "[r9+r10*4] (REX.X+B)",
            vec![0x43, 0x0f, 0xc7, 0x0c, 0x91],
            |cpu| {
                cpu.gpr[Register::R9 as usize] = CELL - 0x20;
                cpu.gpr[Register::R10 as usize] = 8;
            },
        ),
        ("[rip+disp32]", rip_code, |_| {}),
    ];
    for (name, code, setup) in cases {
        let mut cpu = state();
        setup(&mut cpu);
        // RBX may be an address register above, so the replacement low dword is whatever RBX
        // holds (the SDM source is ECX:EBX).
        cpu.gpr[Register::Rax as usize] = MEMORY_VALUE & 0xffff_ffff;
        cpu.gpr[Register::Rdx as usize] = MEMORY_VALUE >> 32;
        cpu.gpr[Register::Rcx as usize] = REPLACEMENT >> 32;
        let rbx = cpu.gpr[Register::Rbx as usize] & 0xffff_ffff;
        let mut ram = Ram::with_code(&code);
        ram.set64(CELL, MEMORY_VALUE);
        let mut memory = Counting::new(ram);
        assert_eq!(
            step(&mut cpu, &mut memory),
            Ok(StepOutcome::Advanced),
            "{name}"
        );
        assert_eq!(memory.cas_calls.len(), 1, "{name}");
        assert_eq!(memory.cas_calls[0].0, CELL, "{name}");
        assert_eq!(
            memory.inner.get64(CELL),
            ((REPLACEMENT >> 32) << 32) | rbx,
            "{name}"
        );
        assert_ne!(cpu.rflags & ZF, 0, "{name}");
        assert_eq!(cpu.rip, CODE + code.len() as u64, "{name}");
    }
}

// ---- no alignment requirement (unlike CMPXCHG16B) ----

#[test]
fn a_misaligned_or_page_crossing_operand_is_not_a_fault() {
    // Offsets from CELL: 1, 3, 4, 7 (misaligned inside a line), 0x3c/0x3d (a 64-byte line
    // crossing) and 0xffc/0xfff (the model's 0x2000 boundary; the flat RAM has no page edge).
    for offset in [1u64, 3, 4, 7, 0x3c, 0x3d, 0xffc, 0xfff] {
        for code in [CMPXCHG8B_RDI.to_vec(), with_lock(&CMPXCHG8B_RDI)] {
            for equal in [true, false] {
                let mut cpu = state();
                cpu.gpr[Register::Rdi as usize] = CELL + offset;
                set_pairs(
                    &mut cpu,
                    if equal { MEMORY_VALUE } else { 0 },
                    REPLACEMENT,
                    0,
                );
                let mut ram = Ram::with_code(&code);
                ram.bytes[0x0f00..0x2100].fill(0xcc);
                ram.set64(CELL + offset, MEMORY_VALUE);
                let mut memory = Counting::new(ram);
                assert_eq!(
                    step(&mut cpu, &mut memory),
                    Ok(StepOutcome::Advanced),
                    "+{offset:#x}"
                );
                assert_eq!(
                    memory.inner.get64(CELL + offset),
                    if equal { REPLACEMENT } else { MEMORY_VALUE },
                    "+{offset:#x}"
                );
                assert_eq!(cpu.rflags & ZF != 0, equal, "+{offset:#x}");
                // The neighbouring bytes are untouched.
                assert_eq!(memory.inner.bytes[(CELL + offset - 1) as usize], 0xcc);
                assert_eq!(memory.inner.bytes[(CELL + offset + 8) as usize], 0xcc);
            }
        }
    }
}

// ---- faults: no memory write, registers and flags unchanged ----

#[test]
fn non_canonical_address_is_gp0_and_ss0_for_an_rsp_rbp_based_operand() {
    let non_canonical = 0x0001_0000_0000_0000u64;
    // [rdi] -> #GP(0), also when only the last byte of the operand is non-canonical.
    for address in [non_canonical, 0x0000_7fff_ffff_fffc] {
        let mut cpu = state();
        cpu.gpr[Register::Rdi as usize] = address;
        let before = cpu.clone();
        let mut memory = Counting::new(Ram::with_code(&CMPXCHG8B_RDI));
        assert_eq!(
            step(&mut cpu, &mut memory),
            Ok(StepOutcome::Fault(ArchitecturalFault {
                vector: 13,
                error_code: Some(0),
                linear_address: None,
            })),
            "{address:#x}"
        );
        assert_eq!(cpu, before, "{address:#x}");
        assert!(memory.cas_calls.is_empty());
    }
    // [rbp+0] and [rsp] reference SS -> #SS(0).
    for code in [vec![0x0f, 0xc7, 0x4d, 0x00], vec![0x0f, 0xc7, 0x0c, 0x24]] {
        let mut cpu = state();
        cpu.gpr[Register::Rbp as usize] = non_canonical;
        cpu.gpr[Register::Rsp as usize] = non_canonical;
        let before = cpu.clone();
        let mut memory = Counting::new(Ram::with_code(&code));
        assert_eq!(
            step(&mut cpu, &mut memory),
            Ok(StepOutcome::Fault(ArchitecturalFault {
                vector: 12,
                error_code: Some(0),
                linear_address: None,
            }))
        );
        assert_eq!(cpu, before);
        assert!(memory.cas_calls.is_empty());
    }
}

#[test]
fn register_form_is_ud_with_and_without_lock_and_changes_nothing() {
    let ud = || {
        StepOutcome::Fault(ArchitecturalFault {
            vector: 6,
            error_code: None,
            linear_address: None,
        })
    };
    // mod = 3, reg = 1: `0F C7 /1` with several rm; plus a REX prefix without W and LOCK.
    for code in [
        vec![0x0f, 0xc7, 0xcb],
        vec![0xf0, 0x0f, 0xc7, 0xcb],
        vec![0x0f, 0xc7, 0xc9],
        vec![0x41, 0x0f, 0xc7, 0xcb],
        vec![0xf0, 0x41, 0x0f, 0xc7, 0xc9],
    ] {
        let mut cpu = state();
        set_pairs(&mut cpu, MEMORY_VALUE, REPLACEMENT, GARBAGE);
        let before = cpu.clone();
        let mut ram = Ram::with_code(&code);
        ram.set64(CELL, MEMORY_VALUE);
        let snapshot = ram.bytes.clone();
        let mut memory = Counting::new(ram);
        assert_eq!(step(&mut cpu, &mut memory), Ok(ud()), "{code:02x?}");
        assert_eq!(cpu, before, "{code:02x?}");
        assert_eq!(memory.inner.bytes, snapshot, "{code:02x?}");
        assert!(memory.cas_calls.is_empty(), "{code:02x?}");
    }
}

#[test]
fn unmapped_operand_is_the_memory_error_and_nothing_changes() {
    for code in [CMPXCHG8B_RDI.to_vec(), with_lock(&CMPXCHG8B_RDI)] {
        let mut cpu = state();
        set_pairs(&mut cpu, MEMORY_VALUE, REPLACEMENT, GARBAGE);
        let before = cpu.clone();
        let mut ram = Ram::with_code(&code);
        ram.set64(CELL, MEMORY_VALUE);
        ram.absent = Some(CELL..CELL + 0x100);
        let snapshot = ram.bytes.clone();
        let mut memory = Counting::new(ram);
        assert_eq!(
            step(&mut cpu, &mut memory),
            Err(MemoryError::Unmapped {
                address: CELL,
                access: AccessKind::Read,
            })
        );
        assert_eq!(cpu, before);
        assert_eq!(memory.inner.bytes, snapshot);
    }
}

#[test]
fn not_present_page_is_a_page_fault_and_nothing_changes() {
    let mut cpu = state();
    set_pairs(&mut cpu, MEMORY_VALUE, REPLACEMENT, GARBAGE);
    let before = cpu.clone();
    let mut ram = Ram::with_code(&CMPXCHG8B_RDI);
    ram.set64(CELL, MEMORY_VALUE);
    ram.not_present = Some(CELL..CELL + 0x100);
    let snapshot = ram.bytes.clone();
    let mut memory = Counting::new(ram);
    assert_eq!(step(&mut cpu, &mut memory), Ram::page_fault(CELL, 0));
    assert_eq!(cpu, before);
    assert_eq!(memory.inner.bytes, snapshot);
}

#[test]
fn read_only_page_faults_on_both_outcomes_because_the_destination_is_always_written() {
    for code in [CMPXCHG8B_RDI.to_vec(), with_lock(&CMPXCHG8B_RDI)] {
        for equal in [true, false] {
            let mut cpu = state();
            set_pairs(
                &mut cpu,
                if equal { MEMORY_VALUE } else { 0 },
                REPLACEMENT,
                GARBAGE,
            );
            let before = cpu.clone();
            let mut ram = Ram::with_code(&code);
            ram.set64(CELL, MEMORY_VALUE);
            ram.read_only = Some(CELL..CELL + 0x100);
            let snapshot = ram.bytes.clone();
            let mut memory = Counting::new(ram);
            assert_eq!(
                step(&mut cpu, &mut memory),
                Ram::page_fault(CELL, 3),
                "equal={equal}"
            );
            // Not even ZF or EDX:EAX is committed.
            assert_eq!(cpu, before, "equal={equal}");
            assert_eq!(memory.inner.bytes, snapshot, "equal={equal}");
        }
    }
}

#[test]
fn a_page_crossing_operand_with_a_faulting_second_page_writes_nothing() {
    // The "page" boundary of the model is 0x2000 here; the operand starts at 0x1ffc.
    const START: u64 = 0x1ffc;
    for equal in [true, false] {
        for second_page_read_only in [true, false] {
            let mut cpu = state();
            cpu.gpr[Register::Rdi as usize] = START;
            set_pairs(
                &mut cpu,
                if equal { MEMORY_VALUE } else { 0 },
                REPLACEMENT,
                GARBAGE,
            );
            let before = cpu.clone();
            let mut ram = Ram::with_code(&with_lock(&CMPXCHG8B_RDI));
            ram.set64(START, MEMORY_VALUE);
            if second_page_read_only {
                ram.read_only = Some(0x2000..0x3000);
            } else {
                ram.not_present = Some(0x2000..0x3000);
            }
            let snapshot = ram.bytes.clone();
            let mut memory = Counting::new(ram);
            let result = step(&mut cpu, &mut memory);
            let expected_fault = if second_page_read_only {
                // The read of the operand succeeds; the write fails on the read-only part.
                Ram::page_fault(START, 3)
            } else {
                Ram::page_fault(START, 0)
            };
            assert_eq!(result, expected_fault);
            assert_eq!(cpu, before);
            assert_eq!(memory.inner.bytes, snapshot);
        }
    }
}

// ---- neighbouring forms keep their previous behaviour ----

#[test]
fn neighbouring_forms_stay_unsupported_without_change() {
    let forms: &[(&str, &[u8])] = &[
        ("operand-size 66", &[0x66, 0x0f, 0xc7, 0x0f]),
        ("address-size 67", &[0x67, 0x0f, 0xc7, 0x0f]),
        ("fs segment", &[0x64, 0x0f, 0xc7, 0x0f]),
        ("rep", &[0xf3, 0x0f, 0xc7, 0x0f]),
        ("repne", &[0xf2, 0x0f, 0xc7, 0x0f]),
        ("lock + rep", &[0xf0, 0xf3, 0x0f, 0xc7, 0x0f]),
        ("REX before another prefix", &[0x40, 0x66, 0x0f, 0xc7, 0x0f]),
        ("REX before lock", &[0x40, 0xf0, 0x0f, 0xc7, 0x0f]),
        ("0F C7 /0 (no REX)", &[0x0f, 0xc7, 0x07]),
        ("0F C7 /2 (no REX)", &[0x0f, 0xc7, 0x17]),
        ("0F C7 /6 (no REX)", &[0x0f, 0xc7, 0x37]),
        ("0F C7 /7 (no REX)", &[0x0f, 0xc7, 0x3f]),
    ];
    for (name, code) in forms {
        let mut cpu = state();
        set_pairs(&mut cpu, MEMORY_VALUE, REPLACEMENT, GARBAGE);
        let before = cpu.clone();
        let mut ram = Ram::with_code(code);
        ram.set64(CELL, MEMORY_VALUE);
        let snapshot = ram.bytes.clone();
        let mut memory = Counting::new(ram);
        assert_eq!(
            step(&mut cpu, &mut memory),
            Ok(StepOutcome::Unsupported { opcode: 0xc7 }),
            "{name}"
        );
        assert_eq!(cpu, before, "{name}");
        assert_eq!(memory.inner.bytes, snapshot, "{name}");
        assert!(memory.cas_calls.is_empty(), "{name}");
    }
}

/// REX.R does not extend the ModRM.reg field of an opcode-extension group (`0F C7 /1`): `44 0F C7 0F`
/// is still CMPXCHG8B [rdi]. Checked on the oracle (Mac mini, x86-64 Linux): the equal case
/// stored ECX:EBX and set ZF, no #UD.
#[test]
fn rex_r_does_not_change_the_opcode_extension_and_the_form_is_cmpxchg8b() {
    for code in [
        vec![0x44, 0x0f, 0xc7, 0x0f],
        vec![0xf0, 0x44, 0x0f, 0xc7, 0x0f],
    ] {
        for equal in [true, false] {
            let mut cpu = state();
            cpu.rflags = 0x2 | CF;
            set_pairs(
                &mut cpu,
                if equal { MEMORY_VALUE } else { !MEMORY_VALUE },
                REPLACEMENT,
                GARBAGE,
            );
            let mut ram = Ram::with_code(&code);
            ram.set64(CELL, MEMORY_VALUE);
            let mut memory = Counting::new(ram);
            assert_eq!(step(&mut cpu, &mut memory), Ok(StepOutcome::Advanced));
            assert_eq!(memory.cas_calls.len(), 1, "{code:02x?}");
            assert_eq!((memory.direct_reads, memory.direct_writes), (0, 0));
            if equal {
                assert_eq!(memory.inner.get64(CELL), REPLACEMENT, "{code:02x?}");
                assert_eq!(cpu.rflags, 0x2 | CF | ZF, "{code:02x?}");
            } else {
                assert_eq!(memory.inner.get64(CELL), MEMORY_VALUE, "{code:02x?}");
                assert_eq!(edx_eax(&cpu), MEMORY_VALUE, "{code:02x?}");
                assert_eq!(cpu.rflags, 0x2 | CF, "{code:02x?}");
            }
            assert_eq!(cpu.rip, CODE + code.len() as u64);
        }
    }
}

#[test]
fn rex_w_still_selects_cmpxchg16b_and_is_not_cmpxchg8b() {
    // `48 0F C7 0F` is CMPXCHG16B: a misaligned operand is #GP(0) (CMPXCHG8B has no such rule),
    // and the eight-byte compare-exchange is never called.
    let mut cpu = state();
    cpu.gpr[Register::Rdi as usize] = CELL + 8;
    let before = cpu.clone();
    let mut memory = Counting::new(Ram::with_code(&[0x48, 0x0f, 0xc7, 0x0f]));
    assert_eq!(
        step(&mut cpu, &mut memory),
        Ok(StepOutcome::Fault(ArchitecturalFault {
            vector: 13,
            error_code: Some(0),
            linear_address: None,
        }))
    );
    assert_eq!(cpu, before);
    assert!(memory.cas_calls.is_empty());
}

// ---- the CPL0 profile set (Ring0) executes the form; the same result as the bare run ----

fn cpl0_state() -> LongModeState {
    let mut cpu = LongModeState {
        cr0: Some(Cr0Profile::BASE),
        cr4: Some(Cr4Profile::BASE),
        sti_shadow: Some(StiShadow::Clear),
        activity: Some(CpuActivity::Running),
        rip: CODE,
        rflags: 0x2,
        ..LongModeState::default()
    };
    cpu.gpr[Register::Rdi as usize] = CELL;
    cpu
}

#[test]
fn the_cpl0_profile_set_executes_both_outcomes_at_ring0() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let config = RunConfig {
        instruction_budget: 1,
        stop_rip: None,
        context: ring0(),
    };
    for code in [CMPXCHG8B_RDI.to_vec(), with_lock(&CMPXCHG8B_RDI)] {
        for equal in [true, false] {
            let mut cpu = cpl0_state();
            cpu.rflags = 0x2 | CF | SF;
            set_pairs(
                &mut cpu,
                if equal { MEMORY_VALUE } else { !MEMORY_VALUE },
                REPLACEMENT,
                0,
            );
            let mut ram = Ram::with_code(&code);
            ram.set64(CELL, MEMORY_VALUE);
            let mut memory = Counting::new(ram);
            let result = run_with_cpl0_profile_set(
                &mut cpu,
                &mut memory,
                config,
                od_isa::INTEGER_FLAGS,
                &set,
                None,
                || false,
            )
            .unwrap();
            assert_eq!(
                result.exit,
                Cpl0ProfileSetExit::Run(RunExit::InstructionBudgetExhausted)
            );
            assert_eq!(result.instructions_executed, 1);
            assert_eq!(memory.cas_calls.len(), 1);
            assert_eq!((memory.direct_reads, memory.direct_writes), (0, 0));
            if equal {
                assert_eq!(memory.inner.get64(CELL), REPLACEMENT);
                assert_eq!(edx_eax(&cpu), MEMORY_VALUE);
                assert_eq!(cpu.rflags, 0x2 | CF | SF | ZF);
            } else {
                assert_eq!(memory.inner.get64(CELL), MEMORY_VALUE);
                assert_eq!(edx_eax(&cpu), MEMORY_VALUE);
                assert_eq!(cpu.rflags, 0x2 | CF | SF);
            }
            assert_eq!(cpu.rip, CODE + code.len() as u64);
        }
    }
}

/// The CPL0 set is a Ring0-only set: its admission refuses a Ring1, Ring2 or Ring3 context with
/// `WrongPrivilege` before anything is fetched, so the form is never reached below CPL0 (the
/// per-instruction `context.privilege == Ring0` test of the admission arm is the second layer of
/// the same rule, as for CMPXCHG16B).
#[test]
fn the_cpl0_profile_set_refuses_the_form_outside_ring0() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    for privilege in [
        PrivilegeLevel::Ring1,
        PrivilegeLevel::Ring2,
        PrivilegeLevel::Ring3,
    ] {
        for code in [CMPXCHG8B_RDI.to_vec(), with_lock(&CMPXCHG8B_RDI)] {
            let mut cpu = cpl0_state();
            set_pairs(&mut cpu, MEMORY_VALUE, REPLACEMENT, GARBAGE);
            let before = cpu.clone();
            let mut ram = Ram::with_code(&code);
            ram.set64(CELL, MEMORY_VALUE);
            let snapshot = ram.bytes.clone();
            let mut memory = Counting::new(ram);
            let config = RunConfig {
                instruction_budget: 1,
                stop_rip: None,
                context: AccessContext {
                    privilege,
                    ..AccessContext::default()
                },
            };
            let result = run_with_cpl0_profile_set(
                &mut cpu,
                &mut memory,
                config,
                od_isa::INTEGER_FLAGS,
                &set,
                None,
                || false,
            );
            assert_eq!(
                result.map(|r| r.exit),
                Err(Cpl0ProfileSetInputError::WrongPrivilege),
                "{privilege:?} {code:02x?}"
            );
            assert_eq!(cpu, before, "{privilege:?}");
            assert_eq!(memory.inner.bytes, snapshot, "{privilege:?}");
            assert!(memory.cas_calls.is_empty(), "{privilege:?}");
        }
    }
}
