//! S2-51: CMPXCHG16B (`[F0] REX.W 0F C7 /1`, m128).
//!
//! Every expected value is written down from the Intel SDM text of CMPXCHG8B/CMPXCHG16B (Vol. 2A,
//! pp. 3-196..3-198, combined SDM order numbers 253665/325383/325384/335592), which was read for
//! this task, not from memory:
//!
//! * REX.W 0F C7 /1 CMPXCHG16B m128: compare RDX:RAX with m128; if equal, set ZF and load RCX:RBX
//!   into m128; else clear ZF and load m128 into RDX:RAX. RDX and RCX hold the high 64 bits.
//! * LOCK is allowed ("can be used with a LOCK prefix"); the destination receives a write cycle
//!   without regard to the result of the comparison.
//! * CF, PF, AF, SF and OF are unaffected.
//! * 64-bit mode exceptions: #SS(0) for a non-canonical address referencing SS; #GP(0) for a
//!   non-canonical address and for an operand not aligned on a 16-byte boundary; #UD if the
//!   destination is not a memory operand; #PF on a page fault.
//!
//! The SDM also lists #GP(0) "If CPUID.01H:ECX.CMPXCHG16B[bit 13] = 0". The interpreter does not
//! gate on that bit (the bit is declared by S2-57 only), so the cases below do not cover it.

use od_isa::execute::{
    self, ControlRegisterPolicy, Cpl0ProfileSet, Cpl0ProfileSetExit, Cr0Profile, Cr4Profile,
    RunConfig, RunExit, StepOutcome, run_with_cpl0_profile_set,
};
use od_isa::{
    AccessContext, AccessKind, ArchitecturalFault, CpuActivity, GuestMemory, LongModeState,
    MemoryError, PrivilegeLevel, Register, StiShadow,
};
use std::ops::Range;

const CF: u64 = 1;
const PF: u64 = 1 << 2;
const AF: u64 = 1 << 4;
const ZF: u64 = 1 << 6;
const SF: u64 = 1 << 7;
const OF: u64 = 1 << 11;
/// The flags CMPXCHG16B must leave alone.
const UNAFFECTED: u64 = CF | PF | AF | SF | OF;

const CODE: u64 = 0x2000;
const CELL: u64 = 0x1000;
const RAM_SIZE: usize = 0x4000;

/// `cmpxchg16b [rdi]` (RBX is the low half of the replacement, so it cannot be the address).
const CMPXCHG16B_RDI: [u8; 4] = [0x48, 0x0f, 0xc7, 0x0f];

fn ring0() -> AccessContext {
    AccessContext {
        privilege: PrivilegeLevel::Ring0,
        ..AccessContext::default()
    }
}

/// Flat little-endian RAM with fault windows. `read`/`write` outside the RAM are
/// `Unmapped`; inside `not_present` they are a #PF without the P bit, inside
/// `read_only` a write is a #PF with P and W set (error code 3).
struct Ram {
    bytes: Vec<u8>,
    not_present: Option<Range<u64>>,
    read_only: Option<Range<u64>>,
    reads: Vec<(u64, usize)>,
    writes: Vec<(u64, Vec<u8>)>,
}

impl Ram {
    fn with_code(code: &[u8]) -> Self {
        let mut bytes = vec![0; RAM_SIZE];
        bytes[CODE as usize..CODE as usize + code.len()].copy_from_slice(code);
        Self {
            bytes,
            not_present: None,
            read_only: None,
            reads: Vec::new(),
            writes: Vec::new(),
        }
    }

    fn set128(&mut self, address: u64, value: u128) {
        let at = address as usize;
        self.bytes[at..at + 16].copy_from_slice(&value.to_le_bytes());
    }

    fn get128(&self, address: u64) -> u128 {
        let at = address as usize;
        u128::from_le_bytes(self.bytes[at..at + 16].try_into().unwrap())
    }

    fn page_fault(address: u64, error_code: u32) -> MemoryError {
        MemoryError::Architectural(ArchitecturalFault {
            vector: 14,
            error_code: Some(error_code),
            linear_address: Some(address),
        })
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
        if self
            .not_present
            .as_ref()
            .is_some_and(|window| window.contains(&address))
        {
            return Err(Self::page_fault(address, 0));
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
        if self
            .not_present
            .as_ref()
            .is_some_and(|window| window.contains(&address))
        {
            return Err(Self::page_fault(address, 2));
        }
        if self
            .read_only
            .as_ref()
            .is_some_and(|window| window.contains(&address))
        {
            return Err(Self::page_fault(address, 3));
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

/// Wraps [`Ram`]: records the `atomic_rmw_u128` calls and delegates them to the inner
/// default, so `inner.reads`/`inner.writes` hold only what that call did. Direct reads and writes
/// by the CPU land in `direct_reads`/`direct_writes`.
struct Counting {
    inner: Ram,
    rmw_calls: Vec<u64>,
    direct_reads: usize,
    direct_writes: usize,
}

impl Counting {
    fn new(inner: Ram) -> Self {
        Self {
            inner,
            rmw_calls: Vec::new(),
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

    fn atomic_rmw_u128(
        &mut self,
        address: u64,
        context: AccessContext,
        update: &mut dyn FnMut(u128) -> u128,
    ) -> Result<u128, MemoryError> {
        self.rmw_calls.push(address);
        self.inner.atomic_rmw_u128(address, context, update)
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

fn set_pairs(state: &mut LongModeState, rdx_rax: u128, rcx_rbx: u128) {
    state.gpr[Register::Rax as usize] = rdx_rax as u64;
    state.gpr[Register::Rdx as usize] = (rdx_rax >> 64) as u64;
    state.gpr[Register::Rbx as usize] = rcx_rbx as u64;
    state.gpr[Register::Rcx as usize] = (rcx_rbx >> 64) as u64;
}

fn rdx_rax(state: &LongModeState) -> u128 {
    (u128::from(state.gpr[Register::Rdx as usize]) << 64)
        | u128::from(state.gpr[Register::Rax as usize])
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

const MEMORY_VALUE: u128 = 0x1122_3344_5566_7788_99aa_bbcc_ddee_ff00;
const REPLACEMENT: u128 = 0xf0e1_d2c3_b4a5_9687_7869_5a4b_3c2d_1e0f;

// ---- the two outcomes (SDM Operation: ZF := 1; DEST := RCX:RBX / ZF := 0; RDX:RAX := DEST) ----

#[test]
fn equal_operand_is_replaced_and_sets_zf_without_touching_rax_rdx_and_other_flags() {
    for code in [CMPXCHG16B_RDI.to_vec(), with_lock(&CMPXCHG16B_RDI)] {
        for initial_flags in [0x2, 0x2 | UNAFFECTED, 0x2 | ZF, 0x2 | ZF | UNAFFECTED] {
            let mut cpu = state();
            cpu.rflags = initial_flags;
            set_pairs(&mut cpu, MEMORY_VALUE, REPLACEMENT);
            let mut ram = Ram::with_code(&code);
            ram.set128(CELL, MEMORY_VALUE);
            let mut memory = Counting::new(ram);
            assert_eq!(step(&mut cpu, &mut memory), Ok(StepOutcome::Advanced));
            // DEST := RCX:RBX; ZF := 1; RDX:RAX untouched; CF/PF/AF/SF/OF unaffected.
            assert_eq!(memory.inner.get128(CELL), REPLACEMENT);
            assert_eq!(rdx_rax(&cpu), MEMORY_VALUE);
            assert_eq!(cpu.rflags, (initial_flags & !ZF) | ZF);
            assert_eq!(cpu.rflags & UNAFFECTED, initial_flags & UNAFFECTED);
            assert_eq!(cpu.gpr[Register::Rbx as usize], REPLACEMENT as u64);
            assert_eq!(cpu.gpr[Register::Rcx as usize], (REPLACEMENT >> 64) as u64);
            assert_eq!(cpu.rip, CODE + code.len() as u64);
        }
    }
}

#[test]
fn unequal_operand_is_loaded_into_rdx_rax_clears_zf_and_is_written_back_unchanged() {
    // The comparison is over all 128 bits: a difference in only one qword must already fail.
    let low_differs = MEMORY_VALUE ^ 1;
    let high_differs = MEMORY_VALUE ^ (1u128 << 127);
    let both_differ = !MEMORY_VALUE;
    for code in [CMPXCHG16B_RDI.to_vec(), with_lock(&CMPXCHG16B_RDI)] {
        for expected in [low_differs, high_differs, both_differ] {
            for initial_flags in [0x2, 0x2 | ZF, 0x2 | UNAFFECTED, 0x2 | ZF | UNAFFECTED] {
                let mut cpu = state();
                cpu.rflags = initial_flags;
                set_pairs(&mut cpu, expected, REPLACEMENT);
                let mut ram = Ram::with_code(&code);
                ram.set128(CELL, MEMORY_VALUE);
                let mut memory = Counting::new(ram);
                assert_eq!(step(&mut cpu, &mut memory), Ok(StepOutcome::Advanced));
                // ZF := 0; RDX:RAX := DEST; DEST := DEST (a write cycle happens anyway).
                assert_eq!(rdx_rax(&cpu), MEMORY_VALUE);
                assert_eq!(cpu.rflags, initial_flags & !ZF);
                assert_eq!(memory.inner.get128(CELL), MEMORY_VALUE);
                assert_eq!(
                    memory.inner.writes,
                    vec![(CELL, MEMORY_VALUE.to_le_bytes().to_vec())]
                );
                // RCX:RBX is the source and is never modified.
                assert_eq!(cpu.gpr[Register::Rbx as usize], REPLACEMENT as u64);
                assert_eq!(cpu.gpr[Register::Rcx as usize], (REPLACEMENT >> 64) as u64);
                assert_eq!(cpu.rip, CODE + code.len() as u64);
            }
        }
    }
}

#[test]
fn it_is_one_u128_call_and_never_two_independent_8_byte_accesses() {
    for equal in [true, false] {
        let mut cpu = state();
        set_pairs(&mut cpu, if equal { MEMORY_VALUE } else { 0 }, REPLACEMENT);
        let mut ram = Ram::with_code(&CMPXCHG16B_RDI);
        ram.set128(CELL, MEMORY_VALUE);
        let mut memory = Counting::new(ram);
        assert_eq!(step(&mut cpu, &mut memory), Ok(StepOutcome::Advanced));
        assert_eq!(memory.rmw_calls, vec![CELL]);
        assert_eq!((memory.direct_reads, memory.direct_writes), (0, 0));
        // The default implementation: exactly one 16-byte read and one 16-byte write (the code
        // fetch goes through `fetch`, not `read`).
        assert_eq!(memory.inner.reads, vec![(CELL, 16)]);
        assert_eq!(memory.inner.writes.len(), 1);
        assert_eq!(memory.inner.writes[0].1.len(), 16);
    }
}

#[test]
fn the_operand_address_follows_the_modrm_forms() {
    type Setup = fn(&mut LongModeState);
    // [rip+disp32] is relative to the end of the whole instruction.
    let rip_relative_disp = (CELL as i64 - (CODE as i64 + 8)) as i32;
    let mut rip_code = vec![0x48, 0x0f, 0xc7, 0x0d];
    rip_code.extend_from_slice(&rip_relative_disp.to_le_bytes());
    let cases: Vec<(&str, Vec<u8>, Setup)> = vec![
        ("[rdi]", CMPXCHG16B_RDI.to_vec(), |_| {}),
        ("[rbx]", vec![0x48, 0x0f, 0xc7, 0x0b], |cpu| {
            cpu.gpr[Register::Rbx as usize] = CELL;
        }),
        ("[rsp]", vec![0x48, 0x0f, 0xc7, 0x0c, 0x24], |cpu| {
            cpu.gpr[Register::Rsp as usize] = CELL;
        }),
        ("[r8] (REX.W+B)", vec![0x49, 0x0f, 0xc7, 0x08], |cpu| {
            cpu.gpr[Register::R8 as usize] = CELL;
        }),
        ("[rbx+0x10]", vec![0x48, 0x0f, 0xc7, 0x4b, 0x10], |cpu| {
            cpu.gpr[Register::Rbx as usize] = CELL - 0x10;
        }),
        (
            "[rbx+rsi*8] (SIB)",
            vec![0x48, 0x0f, 0xc7, 0x0c, 0xf3],
            |cpu| {
                cpu.gpr[Register::Rbx as usize] = CELL - 0x20;
                cpu.gpr[Register::Rsi as usize] = 4;
            },
        ),
        ("[rip+disp32]", rip_code, |_| {}),
    ];
    for (name, code, setup) in cases {
        let mut cpu = state();
        setup(&mut cpu);
        // Compare value and replacement; RBX may be an address register above, so the replacement
        // low qword is whatever RBX holds (the SDM source is RCX:RBX).
        cpu.gpr[Register::Rax as usize] = MEMORY_VALUE as u64;
        cpu.gpr[Register::Rdx as usize] = (MEMORY_VALUE >> 64) as u64;
        cpu.gpr[Register::Rcx as usize] = (REPLACEMENT >> 64) as u64;
        let rbx = cpu.gpr[Register::Rbx as usize];
        let mut ram = Ram::with_code(&code);
        ram.set128(CELL, MEMORY_VALUE);
        let mut memory = ram;
        assert_eq!(
            step(&mut cpu, &mut memory),
            Ok(StepOutcome::Advanced),
            "{name}"
        );
        assert_eq!(cpu.rflags & ZF, ZF, "{name}");
        // Memory low qword is RBX, high qword is RCX.
        assert_eq!(
            memory.get128(CELL),
            (REPLACEMENT >> 64) << 64 | u128::from(rbx),
            "{name}"
        );
        assert_eq!(cpu.rip, CODE + code.len() as u64, "{name}");
    }
}

// ---- #GP(0) for a misaligned operand, before any memory access ----

#[test]
fn misaligned_operand_is_gp0_without_any_memory_access_or_state_change() {
    let gp0 = StepOutcome::Fault(ArchitecturalFault {
        vector: 13,
        error_code: Some(0),
        linear_address: None,
    });
    // 8-byte aligned is not enough ("aligned on a 16-byte boundary"); also an address inside the
    // RAM that would be mapped, and one beyond it (the alignment check comes first, so it is #GP
    // and not an unmapped-memory error).
    for address in [
        CELL + 1,
        CELL + 4,
        CELL + 8,
        CELL + 15,
        (RAM_SIZE as u64) + 8,
        0x0000_7fff_ffff_fff8,
    ] {
        for code in [CMPXCHG16B_RDI.to_vec(), with_lock(&CMPXCHG16B_RDI)] {
            for equal in [true, false] {
                let mut cpu = state();
                cpu.gpr[Register::Rdi as usize] = address;
                cpu.gpr[Register::Rax as usize] = if equal { 0 } else { 1 };
                cpu.rflags = 0x2 | CF | ZF;
                let before = cpu.clone();
                let mut ram = Ram::with_code(&code);
                if (address as usize) + 16 <= RAM_SIZE {
                    // All-zero RAM: an "equal" compare would succeed if it were (wrongly) done.
                    ram.set128(address, 0);
                }
                let snapshot = ram.bytes.clone();
                let mut memory = Counting::new(ram);
                assert_eq!(step(&mut cpu, &mut memory), Ok(gp0.clone()), "{address:#x}");
                assert_eq!(cpu, before, "{address:#x}");
                assert_eq!(memory.inner.bytes, snapshot, "{address:#x}");
                assert!(memory.rmw_calls.is_empty(), "{address:#x}");
                assert!(memory.inner.reads.is_empty(), "{address:#x}");
                assert!(memory.inner.writes.is_empty(), "{address:#x}");
            }
        }
    }
}

#[test]
fn non_canonical_address_is_gp0_and_ss0_for_an_rsp_rbp_based_operand() {
    let non_canonical = 0x0001_0000_0000_0000u64;
    // [rdi] -> #GP(0).
    let mut cpu = state();
    cpu.gpr[Register::Rdi as usize] = non_canonical;
    let before = cpu.clone();
    let mut memory = Counting::new(Ram::with_code(&CMPXCHG16B_RDI));
    assert_eq!(
        step(&mut cpu, &mut memory),
        Ok(StepOutcome::Fault(ArchitecturalFault {
            vector: 13,
            error_code: Some(0),
            linear_address: None,
        }))
    );
    assert_eq!(cpu, before);
    assert!(memory.rmw_calls.is_empty());
    // [rbp+0] and [rsp] reference SS -> #SS(0).
    for code in [
        vec![0x48, 0x0f, 0xc7, 0x4d, 0x00],
        vec![0x48, 0x0f, 0xc7, 0x0c, 0x24],
    ] {
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
        assert!(memory.rmw_calls.is_empty());
    }
}

// ---- #UD for the register form ----

#[test]
fn register_form_is_ud_with_and_without_lock_and_changes_nothing() {
    let ud = StepOutcome::Fault(ArchitecturalFault {
        vector: 6,
        error_code: None,
        linear_address: None,
    });
    // ModRM 0xc8..0xcf: mod = 3, reg = 1.
    for modrm in [0xc8u8, 0xcb, 0xcf] {
        for code in [
            vec![0x48, 0x0f, 0xc7, modrm],
            vec![0xf0, 0x48, 0x0f, 0xc7, modrm],
            vec![0x49, 0x0f, 0xc7, modrm],
        ] {
            let mut cpu = state();
            set_pairs(&mut cpu, MEMORY_VALUE, REPLACEMENT);
            cpu.rflags = 0x2 | ZF | CF;
            let before = cpu.clone();
            let mut ram = Ram::with_code(&code);
            ram.set128(CELL, MEMORY_VALUE);
            let snapshot = ram.bytes.clone();
            let mut memory = Counting::new(ram);
            assert_eq!(step(&mut cpu, &mut memory), Ok(ud.clone()), "{code:02x?}");
            assert_eq!(cpu, before, "{code:02x?}");
            assert_eq!(memory.inner.bytes, snapshot, "{code:02x?}");
            assert!(memory.rmw_calls.is_empty(), "{code:02x?}");
            assert!(memory.inner.reads.is_empty() && memory.inner.writes.is_empty());
        }
    }
}

// ---- memory faults: unmapped, not present, read-only; nothing changes ----

#[test]
fn unmapped_operand_is_the_memory_error_and_nothing_changes() {
    for address in [RAM_SIZE as u64, 0x10_0000] {
        for code in [CMPXCHG16B_RDI.to_vec(), with_lock(&CMPXCHG16B_RDI)] {
            let mut cpu = state();
            cpu.gpr[Register::Rdi as usize] = address;
            cpu.rflags = 0x2 | CF;
            let before = cpu.clone();
            let ram = Ram::with_code(&code);
            let snapshot = ram.bytes.clone();
            let mut memory = Counting::new(ram);
            assert_eq!(
                step(&mut cpu, &mut memory),
                Err(MemoryError::Unmapped {
                    address,
                    access: AccessKind::Read,
                })
            );
            assert_eq!(cpu, before);
            assert_eq!(memory.inner.bytes, snapshot);
            assert!(memory.inner.writes.is_empty());
        }
    }
}

#[test]
fn not_present_page_is_a_page_fault_and_nothing_changes() {
    let mut cpu = state();
    set_pairs(&mut cpu, MEMORY_VALUE, REPLACEMENT);
    cpu.rflags = 0x2 | ZF | OF;
    let before = cpu.clone();
    let mut ram = Ram::with_code(&CMPXCHG16B_RDI);
    ram.set128(CELL, MEMORY_VALUE);
    ram.not_present = Some(CELL..CELL + 16);
    let snapshot = ram.bytes.clone();
    let mut memory = Counting::new(ram);
    assert_eq!(
        step(&mut cpu, &mut memory),
        Ok(StepOutcome::Fault(ArchitecturalFault {
            vector: 14,
            error_code: Some(0),
            linear_address: Some(CELL),
        }))
    );
    assert_eq!(cpu, before);
    assert_eq!(memory.inner.bytes, snapshot);
    assert!(memory.inner.writes.is_empty());
}

#[test]
fn read_only_page_faults_on_both_outcomes_because_the_destination_is_always_written() {
    // SDM: "the destination operand receives a write cycle without regard to the result of the
    // comparison", so a failed compare faults on a read-only page exactly like a successful one.
    for equal in [true, false] {
        let mut cpu = state();
        set_pairs(&mut cpu, if equal { MEMORY_VALUE } else { 1 }, REPLACEMENT);
        cpu.rflags = 0x2 | CF;
        let before = cpu.clone();
        let mut ram = Ram::with_code(&CMPXCHG16B_RDI);
        ram.set128(CELL, MEMORY_VALUE);
        ram.read_only = Some(CELL..CELL + 16);
        let snapshot = ram.bytes.clone();
        let mut memory = Counting::new(ram);
        assert_eq!(
            step(&mut cpu, &mut memory),
            Ok(StepOutcome::Fault(ArchitecturalFault {
                vector: 14,
                error_code: Some(3),
                linear_address: Some(CELL),
            })),
            "equal = {equal}"
        );
        // No partial write, no RDX:RAX load, no ZF change, RIP not advanced.
        assert_eq!(cpu, before, "equal = {equal}");
        assert_eq!(memory.inner.bytes, snapshot, "equal = {equal}");
        assert!(memory.inner.writes.is_empty(), "equal = {equal}");
    }
}

// ---- forms outside the S2-51 slice stay refused (Unsupported, no change) ----

#[test]
fn neighbouring_forms_stay_unsupported_without_change() {
    let forms: &[(&str, &[u8])] = &[
        ("cmpxchg8b [rdi] (no REX.W)", &[0x0f, 0xc7, 0x0f]),
        ("lock cmpxchg8b [rdi]", &[0xf0, 0x0f, 0xc7, 0x0f]),
        ("REX without W", &[0x40, 0x0f, 0xc7, 0x0f]),
        ("operand-size 66", &[0x66, 0x48, 0x0f, 0xc7, 0x0f]),
        ("address-size 67", &[0x67, 0x48, 0x0f, 0xc7, 0x0f]),
        ("fs segment", &[0x64, 0x48, 0x0f, 0xc7, 0x0f]),
        ("rep", &[0xf3, 0x48, 0x0f, 0xc7, 0x0f]),
        ("repne", &[0xf2, 0x48, 0x0f, 0xc7, 0x0f]),
        ("lock + rep", &[0xf0, 0xf3, 0x48, 0x0f, 0xc7, 0x0f]),
        ("REX before another prefix", &[0x48, 0x66, 0x0f, 0xc7, 0x0f]),
        ("REX before lock", &[0x48, 0xf0, 0x0f, 0xc7, 0x0f]),
        ("0F C7 /0", &[0x48, 0x0f, 0xc7, 0x07]),
        ("0F C7 /2", &[0x48, 0x0f, 0xc7, 0x17]),
        ("0F C7 /6 (rdrand/vmptrld)", &[0x48, 0x0f, 0xc7, 0x37]),
        ("0F C7 /7", &[0x48, 0x0f, 0xc7, 0x3f]),
    ];
    for (name, code) in forms {
        let mut cpu = state();
        set_pairs(&mut cpu, MEMORY_VALUE, REPLACEMENT);
        let before = cpu.clone();
        let mut ram = Ram::with_code(code);
        ram.set128(CELL, MEMORY_VALUE);
        let snapshot = ram.bytes.clone();
        let mut memory = Counting::new(ram);
        assert_eq!(
            step(&mut cpu, &mut memory),
            Ok(StepOutcome::Unsupported { opcode: 0xc7 }),
            "{name}"
        );
        assert_eq!(cpu, before, "{name}");
        assert_eq!(memory.inner.bytes, snapshot, "{name}");
        assert!(memory.rmw_calls.is_empty(), "{name}");
    }
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
    for code in [CMPXCHG16B_RDI.to_vec(), with_lock(&CMPXCHG16B_RDI)] {
        for equal in [true, false] {
            let mut cpu = cpl0_state();
            cpu.rflags = 0x2 | CF | SF;
            set_pairs(
                &mut cpu,
                if equal { MEMORY_VALUE } else { !MEMORY_VALUE },
                REPLACEMENT,
            );
            let mut ram = Ram::with_code(&code);
            ram.set128(CELL, MEMORY_VALUE);
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
            assert_eq!(memory.rmw_calls, vec![CELL]);
            assert_eq!((memory.direct_reads, memory.direct_writes), (0, 0));
            if equal {
                assert_eq!(memory.inner.get128(CELL), REPLACEMENT);
                assert_eq!(rdx_rax(&cpu), MEMORY_VALUE);
                assert_eq!(cpu.rflags, 0x2 | CF | SF | ZF);
            } else {
                assert_eq!(memory.inner.get128(CELL), MEMORY_VALUE);
                assert_eq!(rdx_rax(&cpu), MEMORY_VALUE);
                assert_eq!(cpu.rflags, 0x2 | CF | SF);
            }
            assert_eq!(cpu.rip, CODE + code.len() as u64);
        }
    }
}
