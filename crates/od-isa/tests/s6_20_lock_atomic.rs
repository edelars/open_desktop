//! S6-20: every `LOCK` read-modify-write form and `XCHG` with memory reaches guest memory through
//! the single `GuestMemory::atomic_rmw_integer` call, and the interpreter runs the dword forms of
//! the S6-05 litmus corpus in a bare run. Also the `WaitForSipi`/`Shutdown` refusal of the public
//! entry points outside the CPL0 profile set (SMP_CONTRACT.md 3).
//!
//! Expected register, memory and flag values are written down from the Intel SDM instruction
//! entries (INC, ADD, CMP flags; CMPXCHG, XADD, XCHG semantics), each case with its arithmetic
//! in the comment; they are "from memory of the SDM", not re-checked against the PDF text.

use od_isa::execute::{
    self, Cr0Profile, DiagnosticCpl0PioU8V1, MaskableRequestRunInputError, MovCr3Exit,
    MovCr3InputError, MovCr3Long64V1, PagedCpl0PioU8V1, PioU8AdmissionError, PioU8Exit,
    RepRunInputError, RunConfig, RunExit, StepOutcome,
};
use od_isa::{
    AccessContext, AccessKind, ArchitecturalFault, CpuActivity, GuestMemory, InstructionMode,
    LongModeState, MemoryError, OperandSize, PortIo, PrivilegeLevel, Register,
};

const CF: u64 = 1;
const PF: u64 = 1 << 2;
const AF: u64 = 1 << 4;
const ZF: u64 = 1 << 6;
const SF: u64 = 1 << 7;
const OF: u64 = 1 << 11;
const ARITH: u64 = CF | PF | AF | ZF | SF | OF;

const CODE: u64 = 0x2000;
const CELL: u64 = 0x1000;

fn ring0() -> AccessContext {
    AccessContext {
        privilege: PrivilegeLevel::Ring0,
        ..AccessContext::default()
    }
}

/// Flat little-endian RAM with an optional read-only window. Direct `read`/`write` calls are
/// recorded; the default `atomic_rmw_integer` runs on top of them, so a test that wants to know
/// whether the CPU reached memory through the RMW call wraps this in [`Counting`].
struct Ram {
    bytes: Vec<u8>,
    read_only: Option<std::ops::Range<u64>>,
    reads: Vec<(u64, usize)>,
    writes: Vec<(u64, Vec<u8>)>,
}

impl Ram {
    fn with_code(code: &[u8]) -> Self {
        let mut ram = Self {
            bytes: vec![0; 0x4000],
            read_only: None,
            reads: Vec::new(),
            writes: Vec::new(),
        };
        ram.bytes[CODE as usize..CODE as usize + code.len()].copy_from_slice(code);
        ram
    }

    fn set(&mut self, address: u64, size: OperandSize, value: u64) {
        let at = address as usize;
        self.bytes[at..at + size.bytes()].copy_from_slice(&value.to_le_bytes()[..size.bytes()]);
    }

    fn get(&self, address: u64, size: OperandSize) -> u64 {
        let mut bytes = [0u8; 8];
        let at = address as usize;
        bytes[..size.bytes()].copy_from_slice(&self.bytes[at..at + size.bytes()]);
        u64::from_le_bytes(bytes)
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
        let at = address as usize;
        out.copy_from_slice(&self.bytes[at..at + out.len()]);
        Ok(())
    }

    fn write(&mut self, address: u64, _: AccessContext, source: &[u8]) -> Result<(), MemoryError> {
        if self
            .read_only
            .as_ref()
            .is_some_and(|window| window.contains(&address))
        {
            // #PF(P=1, W=1, U/S=0): a write to a present read-only page.
            return Err(MemoryError::Architectural(ArchitecturalFault {
                vector: 14,
                error_code: Some(3),
                linear_address: Some(address),
            }));
        }
        self.writes.push((address, source.to_vec()));
        let at = address as usize;
        self.bytes[at..at + source.len()].copy_from_slice(source);
        Ok(())
    }

    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        Ok(())
    }
}

/// Wraps [`Ram`]: counts the `atomic_rmw_integer` calls and delegates them to the inner default,
/// so `inner.reads` and `inner.writes` hold only what that call did. Direct reads/writes by the
/// CPU would land in `direct_reads`/`direct_writes`.
struct Counting {
    inner: Ram,
    rmw_calls: Vec<(u64, OperandSize)>,
    direct_reads: usize,
    direct_writes: usize,
    fail_rmw: Option<MemoryError>,
}

impl Counting {
    fn new(inner: Ram) -> Self {
        Self {
            inner,
            rmw_calls: Vec::new(),
            direct_reads: 0,
            direct_writes: 0,
            fail_rmw: None,
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

    fn atomic_rmw_integer(
        &mut self,
        address: u64,
        context: AccessContext,
        size: OperandSize,
        update: &mut dyn FnMut(u64) -> u64,
    ) -> Result<u64, MemoryError> {
        self.rmw_calls.push((address, size));
        if let Some(error) = &self.fail_rmw {
            return Err(error.clone());
        }
        self.inner
            .atomic_rmw_integer(address, context, size, update)
    }

    fn mfence(&mut self, context: AccessContext) -> Result<(), MemoryError> {
        self.inner.mfence(context)
    }
}

fn state() -> LongModeState {
    LongModeState {
        rip: CODE,
        rflags: 0x2,
        ..LongModeState::default()
    }
}

fn step(state: &mut LongModeState, memory: &mut impl GuestMemory) -> StepOutcome {
    execute::step(state, memory, ring0()).unwrap()
}

// ---- the trait: one RMW definition, the three older operations are its specializations ----

#[test]
fn rmw_returns_the_old_value_writes_the_update_and_always_writes() {
    let mut ram = Ram::with_code(&[]);
    ram.set(CELL, OperandSize::Dword, 0xdead_beef);
    let old = ram
        .atomic_rmw_integer(CELL, ring0(), OperandSize::Dword, &mut |old| old ^ 0xff)
        .unwrap();
    assert_eq!(old, 0xdead_beef);
    assert_eq!(ram.get(CELL, OperandSize::Dword), 0xdead_be10);
    // An update that returns the old value is still a write (a failed CMPXCHG writes, SDM).
    ram.writes.clear();
    ram.atomic_rmw_integer(CELL, ring0(), OperandSize::Dword, &mut |old| old)
        .unwrap();
    assert_eq!(
        ram.writes,
        vec![(CELL, 0xdead_be10u32.to_le_bytes().to_vec())]
    );
    // Only the operand width is touched: the neighbouring byte stays.
    ram.set(CELL + 4, OperandSize::Dword, 0x1122_3344);
    ram.atomic_rmw_integer(CELL, ring0(), OperandSize::Dword, &mut |_| u64::MAX)
        .unwrap();
    assert_eq!(ram.get(CELL, OperandSize::Dword), 0xffff_ffff);
    assert_eq!(ram.get(CELL + 4, OperandSize::Dword), 0x1122_3344);
}

#[test]
fn rmw_on_a_read_only_cell_faults_and_leaves_memory_unchanged() {
    let mut ram = Ram::with_code(&[]);
    ram.set(CELL, OperandSize::Dword, 7);
    ram.read_only = Some(CELL..CELL + 4);
    let error = ram
        .atomic_rmw_integer(CELL, ring0(), OperandSize::Dword, &mut |old| old + 1)
        .unwrap_err();
    assert!(matches!(error, MemoryError::Architectural(fault) if fault.vector == 14));
    assert_eq!(ram.get(CELL, OperandSize::Dword), 7);
}

#[test]
fn the_older_atomic_operations_are_the_rmw_specializations() {
    let mut ram = Ram::with_code(&[]);
    // XCHG: returns the old value, stores the replacement (SDM XCHG: TEMP = DEST, DEST = SRC).
    ram.set(CELL, OperandSize::Dword, 5);
    assert_eq!(
        ram.atomic_exchange_integer(CELL, ring0(), OperandSize::Dword, 9),
        Ok(5)
    );
    assert_eq!(ram.get(CELL, OperandSize::Dword), 9);
    // XADD: returns the old value, stores old + addend wrapped to the width.
    ram.set(CELL, OperandSize::Dword, 0xffff_fffe);
    assert_eq!(
        ram.atomic_fetch_add_integer(CELL, ring0(), OperandSize::Dword, 3),
        Ok(0xffff_fffe)
    );
    assert_eq!(ram.get(CELL, OperandSize::Dword), 1);
    // CMPXCHG, success: observed == expected -> replaced.
    ram.set(CELL, OperandSize::Dword, 4);
    assert_eq!(
        ram.atomic_compare_exchange_integer(CELL, ring0(), OperandSize::Dword, 4, 8),
        Ok(4)
    );
    assert_eq!(ram.get(CELL, OperandSize::Dword), 8);
    // CMPXCHG, failure: the observed value is returned, memory keeps it, and it is still written
    // (SDM: "writes the destination operand back unchanged"), so a read-only cell faults.
    ram.writes.clear();
    assert_eq!(
        ram.atomic_compare_exchange_integer(CELL, ring0(), OperandSize::Dword, 4, 99),
        Ok(8)
    );
    assert_eq!(ram.get(CELL, OperandSize::Dword), 8);
    assert_eq!(ram.writes.len(), 1);
    ram.read_only = Some(CELL..CELL + 4);
    assert!(matches!(
        ram.atomic_compare_exchange_integer(CELL, ring0(), OperandSize::Dword, 4, 99),
        Err(MemoryError::Architectural(_))
    ));
    // The expected value is compared at the operand width.
    ram.read_only = None;
    ram.set(CELL, OperandSize::Dword, 0x10);
    assert_eq!(
        ram.atomic_compare_exchange_integer(CELL, ring0(), OperandSize::Dword, 0x1_0000_0010, 0x20),
        Ok(0x10)
    );
    assert_eq!(ram.get(CELL, OperandSize::Dword), 0x20);
}

// ---- the corpus forms in a bare run (the S6-05 bytes, absolute address through SIB) ----

/// `lock inc dword ptr [0x1000]`.
const LOCK_INC: [u8; 8] = [0xf0, 0xff, 0x04, 0x25, 0x00, 0x10, 0x00, 0x00];
/// `lock cmpxchg dword ptr [0x1000], ebx`.
const LOCK_CMPXCHG: [u8; 9] = [0xf0, 0x0f, 0xb1, 0x1c, 0x25, 0x00, 0x10, 0x00, 0x00];
/// `lock xadd dword ptr [0x1000], ebx`.
const LOCK_XADD: [u8; 9] = [0xf0, 0x0f, 0xc1, 0x1c, 0x25, 0x00, 0x10, 0x00, 0x00];
/// `xchg dword ptr [0x1000], ebx` (implicitly locked).
const XCHG: [u8; 7] = [0x87, 0x1c, 0x25, 0x00, 0x10, 0x00, 0x00];

fn counting(code: &[u8], cell: u32) -> Counting {
    let mut ram = Ram::with_code(code);
    ram.set(CELL, OperandSize::Dword, u64::from(cell));
    Counting::new(ram)
}

#[test]
fn lock_inc_dword_is_one_rmw_call_with_the_inc_flags() {
    // SDM INC: result = old + 1, OF/SF/ZF/AF/PF from the result, CF unchanged.
    // (old, flags other than CF)
    for (old, flags) in [
        (0x41u32, PF),                    // 0x42: two set bits -> PF
        (0x7fff_ffff, OF | SF | PF | AF), // INT_MAX + 1
        (0xffff_ffff, ZF | PF | AF),      // wraps to 0
        (0x0f, AF),                       // 0x10: nibble carry, one bit
    ] {
        for cf in [0, CF] {
            let mut cpu = state();
            cpu.rflags |= cf;
            let mut memory = counting(&LOCK_INC, old);
            assert_eq!(step(&mut cpu, &mut memory), StepOutcome::Advanced);
            assert_eq!(memory.rmw_calls, vec![(CELL, OperandSize::Dword)]);
            assert_eq!((memory.direct_reads, memory.direct_writes), (0, 0));
            assert_eq!(
                memory.inner.get(CELL, OperandSize::Dword),
                u64::from(old.wrapping_add(1))
            );
            assert_eq!(cpu.rflags & ARITH, cf | flags, "old {old:#x}");
            assert_eq!(cpu.rip, CODE + LOCK_INC.len() as u64);
        }
    }
}

#[test]
fn lock_cmpxchg_dword_success_and_failure_follow_the_sdm() {
    // Success: EAX (5) == [m] (5): ZF = 1, [m] = EBX, EAX and the upper half of RAX untouched;
    // flags as CMP EAX, [m] (5 - 5 = 0: ZF, PF; no CF/AF/OF/SF).
    let mut cpu = state();
    cpu.gpr[Register::Rax as usize] = 0x1111_2222_0000_0005;
    cpu.gpr[Register::Rbx as usize] = 0x3333_4444_0000_0009;
    let mut memory = counting(&LOCK_CMPXCHG, 5);
    assert_eq!(step(&mut cpu, &mut memory), StepOutcome::Advanced);
    assert_eq!(memory.rmw_calls, vec![(CELL, OperandSize::Dword)]);
    assert_eq!((memory.direct_reads, memory.direct_writes), (0, 0));
    assert_eq!(memory.inner.get(CELL, OperandSize::Dword), 9);
    assert_eq!(cpu.gpr[Register::Rax as usize], 0x1111_2222_0000_0005);
    assert_eq!(cpu.rflags & ARITH, ZF | PF);

    // Failure: EAX (5) != [m] (7): EAX = 7 (dword write zero-extends RAX), [m] keeps 7 and is
    // written back (one write), ZF = 0; flags as CMP 5, 7: 5 - 7 = 0xffff_fffe -> CF (borrow),
    // SF, AF (low nibble 5 < 7), PF = 0 (0xfe has seven set bits), ZF = 0, OF = 0.
    let mut cpu = state();
    cpu.gpr[Register::Rax as usize] = 0x1111_2222_0000_0005;
    cpu.gpr[Register::Rbx as usize] = 0x3333_4444_0000_0009;
    let mut memory = counting(&LOCK_CMPXCHG, 7);
    assert_eq!(step(&mut cpu, &mut memory), StepOutcome::Advanced);
    assert_eq!(memory.rmw_calls.len(), 1);
    assert_eq!(memory.inner.get(CELL, OperandSize::Dword), 7);
    assert_eq!(
        memory.inner.writes.len(),
        1,
        "a failed CMPXCHG still writes"
    );
    assert_eq!(cpu.gpr[Register::Rax as usize], 7);
    assert_eq!(cpu.rflags & ARITH, CF | SF | AF);
}

#[test]
fn lock_xadd_dword_returns_the_old_value_in_the_register_and_adds_in_memory() {
    // SDM XADD: TEMP = SRC + DEST, SRC = DEST, DEST = TEMP; flags as ADD.
    // 10 + 3 = 13 (0b1101: three bits -> PF = 0), no carries.
    let mut cpu = state();
    cpu.gpr[Register::Rbx as usize] = 0x5555_6666_0000_0003;
    let mut memory = counting(&LOCK_XADD, 10);
    assert_eq!(step(&mut cpu, &mut memory), StepOutcome::Advanced);
    assert_eq!(memory.rmw_calls, vec![(CELL, OperandSize::Dword)]);
    assert_eq!((memory.direct_reads, memory.direct_writes), (0, 0));
    assert_eq!(memory.inner.get(CELL, OperandSize::Dword), 13);
    assert_eq!(
        cpu.gpr[Register::Rbx as usize],
        10,
        "EBX = old, RBX high half cleared"
    );
    assert_eq!(cpu.rflags & ARITH, 0);

    // 0xffff_ffff + 2 wraps to 1: CF, AF (f + 2 carries out of the nibble), PF = 0 (one bit).
    let mut cpu = state();
    cpu.gpr[Register::Rbx as usize] = 2;
    let mut memory = counting(&LOCK_XADD, 0xffff_ffff);
    assert_eq!(step(&mut cpu, &mut memory), StepOutcome::Advanced);
    assert_eq!(memory.inner.get(CELL, OperandSize::Dword), 1);
    assert_eq!(cpu.gpr[Register::Rbx as usize], 0xffff_ffff);
    assert_eq!(cpu.rflags & ARITH, CF | AF);
}

#[test]
fn xchg_dword_memory_swaps_through_one_rmw_call_and_keeps_the_flags() {
    let mut cpu = state();
    cpu.rflags |= CF | ZF | OF;
    cpu.gpr[Register::Rbx as usize] = 0xaaaa_bbbb_0000_0001;
    let mut memory = counting(&XCHG, 0x55);
    assert_eq!(step(&mut cpu, &mut memory), StepOutcome::Advanced);
    assert_eq!(memory.rmw_calls, vec![(CELL, OperandSize::Dword)]);
    assert_eq!((memory.direct_reads, memory.direct_writes), (0, 0));
    assert_eq!(memory.inner.get(CELL, OperandSize::Dword), 1);
    assert_eq!(
        cpu.gpr[Register::Rbx as usize],
        0x55,
        "EBX = old, RBX high half cleared"
    );
    assert_eq!(
        cpu.rflags,
        0x2 | CF | ZF | OF,
        "XCHG does not touch the flags"
    );
}

#[test]
fn xchg_qword_memory_still_swaps_all_eight_bytes() {
    // `xchg qword ptr [0x1000], rbx`: the older (S2-21) form, now through the same API.
    let mut cpu = state();
    cpu.gpr[Register::Rbx as usize] = 0x0123_4567_89ab_cdef;
    let mut ram = Ram::with_code(&[0x48, 0x87, 0x1c, 0x25, 0x00, 0x10, 0x00, 0x00]);
    ram.set(CELL, OperandSize::Qword, 0xfedc_ba98_7654_3210);
    let mut memory = Counting::new(ram);
    assert_eq!(step(&mut cpu, &mut memory), StepOutcome::Advanced);
    assert_eq!(
        memory.inner.get(CELL, OperandSize::Qword),
        0x0123_4567_89ab_cdef
    );
    assert_eq!(cpu.gpr[Register::Rbx as usize], 0xfedc_ba98_7654_3210);
    assert_eq!((memory.direct_reads, memory.direct_writes), (0, 0));
}

#[test]
fn a_failing_rmw_leaves_registers_flags_rip_and_memory_unchanged_for_every_form() {
    for (code, name) in [
        (LOCK_INC.to_vec(), "lock inc"),
        (LOCK_CMPXCHG.to_vec(), "lock cmpxchg"),
        (LOCK_XADD.to_vec(), "lock xadd"),
        (XCHG.to_vec(), "xchg"),
    ] {
        let mut cpu = state();
        cpu.rflags |= CF | PF;
        cpu.gpr[Register::Rax as usize] = 5;
        cpu.gpr[Register::Rbx as usize] = 9;
        let before = cpu.clone();
        let mut memory = counting(&code, 7);
        memory.fail_rmw = Some(MemoryError::Backend("injected".into()));
        let result = execute::step(&mut cpu, &mut memory, ring0());
        assert_eq!(
            result,
            Err(MemoryError::Backend("injected".into())),
            "{name}"
        );
        assert_eq!(cpu, before, "{name}");
        assert_eq!(memory.inner.get(CELL, OperandSize::Dword), 7, "{name}");
        assert_eq!(memory.rmw_calls.len(), 1, "{name}");
    }
}

#[test]
fn a_read_only_cell_faults_every_form_including_a_failing_cmpxchg() {
    for (code, name) in [
        (LOCK_INC.to_vec(), "lock inc"),
        (LOCK_CMPXCHG.to_vec(), "lock cmpxchg (mismatch: EAX 5 != 7)"),
        (LOCK_XADD.to_vec(), "lock xadd"),
        (XCHG.to_vec(), "xchg"),
    ] {
        let mut cpu = state();
        cpu.rflags |= CF;
        cpu.gpr[Register::Rax as usize] = 5;
        cpu.gpr[Register::Rbx as usize] = 9;
        let before = cpu.clone();
        let mut ram = Ram::with_code(&code);
        ram.set(CELL, OperandSize::Dword, 7);
        ram.read_only = Some(CELL..CELL + 4);
        let mut memory = Counting::new(ram);
        let outcome = step(&mut cpu, &mut memory);
        assert!(
            matches!(&outcome, StepOutcome::Fault(fault) if fault.vector == 14),
            "{name}: {outcome:?}"
        );
        assert_eq!(cpu, before, "{name}: no register, flag or RIP change");
        assert_eq!(memory.inner.get(CELL, OperandSize::Dword), 7, "{name}");
    }
}

#[test]
fn other_lock_forms_stay_unsupported_in_a_bare_run_and_touch_no_data() {
    // LOCK DEC dword [0x1000]; LOCK OR dword [0x1000], 1; LOCK ADD dword [0x1000], ebx;
    // LOCK AND byte [0x1000], 1; LOCK BTS dword [0x1000], 1. Admitted only in the CPL0 set.
    // Also the qword INC/CMPXCHG and the DS form of CMPXCHG (see the entries).
    let forms: [(&[u8], u8); 8] = [
        (&[0xf0, 0xff, 0x0c, 0x25, 0x00, 0x10, 0x00, 0x00], 0xff),
        (
            &[0xf0, 0x83, 0x0c, 0x25, 0x00, 0x10, 0x00, 0x00, 0x01],
            0x83,
        ),
        (&[0xf0, 0x01, 0x1c, 0x25, 0x00, 0x10, 0x00, 0x00], 0x01),
        (
            &[0xf0, 0x80, 0x24, 0x25, 0x00, 0x10, 0x00, 0x00, 0x01],
            0x80,
        ),
        (
            &[0xf0, 0x0f, 0xba, 0x2c, 0x25, 0x00, 0x10, 0x00, 0x00, 0x01],
            0xba,
        ),
        // LOCK INC / LOCK CMPXCHG with REX.W (qword): not in the corpus, not admitted.
        (
            &[0xf0, 0x48, 0xff, 0x04, 0x25, 0x00, 0x10, 0x00, 0x00],
            0xff,
        ),
        (
            &[0xf0, 0x48, 0x0f, 0xb1, 0x1c, 0x25, 0x00, 0x10, 0x00, 0x00],
            0xb1,
        ),
        // CMPXCHG with the DS byte (the Linux `.smp_locks` substitute) is not a LOCK byte: refused.
        (
            &[0x3e, 0x0f, 0xb1, 0x1c, 0x25, 0x00, 0x10, 0x00, 0x00],
            0xb1,
        ),
    ];
    for (code, opcode) in forms {
        let mut cpu = state();
        let before = cpu.clone();
        let mut memory = counting(code, 7);
        let outcome = step(&mut cpu, &mut memory);
        assert_eq!(outcome, StepOutcome::Unsupported { opcode }, "{code:02x?}");
        assert_eq!(cpu, before, "{code:02x?}");
        assert!(memory.rmw_calls.is_empty(), "{code:02x?}");
        assert_eq!((memory.direct_reads, memory.direct_writes), (0, 0));
        assert_eq!(memory.inner.get(CELL, OperandSize::Dword), 7);
    }
}

#[test]
fn run_executes_the_four_corpus_forms_back_to_back() {
    // LOCK INC; LOCK XADD EBX; XCHG EBX; LOCK CMPXCHG EBX with EAX = 3: a short program through
    // `run`, the entry point the litmus harness uses.
    let mut code = Vec::new();
    code.extend_from_slice(&LOCK_INC);
    code.extend_from_slice(&LOCK_XADD);
    code.extend_from_slice(&XCHG);
    code.extend_from_slice(&LOCK_CMPXCHG);
    let mut cpu = state();
    cpu.gpr[Register::Rax as usize] = 3;
    cpu.gpr[Register::Rbx as usize] = 10;
    let mut memory = counting(&code, 1);
    let end = CODE + code.len() as u64;
    let result = execute::run(
        &mut cpu,
        &mut memory,
        RunConfig {
            instruction_budget: 10,
            stop_rip: Some(end),
            context: ring0(),
        },
        || false,
    );
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 4);
    // [m]: 1 -> inc 2 -> xadd (EBX 10): [m] = 12, EBX = 2 -> xchg: [m] = 2, EBX = 12 ->
    // cmpxchg: EAX (3) != [m] (2): EAX = 2, [m] = 2.
    assert_eq!(memory.inner.get(CELL, OperandSize::Dword), 2);
    assert_eq!(cpu.gpr[Register::Rbx as usize], 12);
    assert_eq!(cpu.gpr[Register::Rax as usize], 2);
    assert_eq!(memory.rmw_calls.len(), 4);
    assert_eq!((memory.direct_reads, memory.direct_writes), (0, 0));
}

// ---- WaitForSipi / Shutdown at the public entry points outside the CPL0 set ----

/// A memory that must never be reached: an entry that refuses has executed nothing.
struct Untouched;

impl GuestMemory for Untouched {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        panic!("fetch at {address:#x} by a vCPU that is not running")
    }
    fn read(&mut self, address: u64, _: AccessContext, _: &mut [u8]) -> Result<(), MemoryError> {
        panic!("read at {address:#x} by a vCPU that is not running")
    }
    fn write(&mut self, address: u64, _: AccessContext, _: &[u8]) -> Result<(), MemoryError> {
        panic!("write at {address:#x} by a vCPU that is not running")
    }
    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        panic!("fence by a vCPU that is not running")
    }
}

struct NoPorts;

impl PortIo for NoPorts {
    type Error = &'static str;
    fn read_u8(&mut self, port: u16) -> Result<u8, Self::Error> {
        panic!("port read {port:#x} by a vCPU that is not running")
    }
    fn write_u8(&mut self, port: u16, _: u8) -> Result<(), Self::Error> {
        panic!("port write {port:#x} by a vCPU that is not running")
    }
}

fn not_running(activity: CpuActivity) -> LongModeState {
    LongModeState {
        rip: CODE,
        rflags: 0x2,
        activity: Some(activity),
        ..LongModeState::default()
    }
}

const NOT_RUNNING: [CpuActivity; 2] = [CpuActivity::WaitForSipi, CpuActivity::Shutdown];

fn config() -> RunConfig {
    RunConfig {
        instruction_budget: 10,
        stop_rip: None,
        context: ring0(),
    }
}

#[test]
fn run_refuses_a_vcpu_that_is_not_running() {
    for activity in NOT_RUNNING {
        let mut cpu = not_running(activity);
        let before = cpu.clone();
        let result = execute::run(&mut cpu, &mut Untouched, config(), || false);
        assert_eq!(result.exit, RunExit::NotRunning, "{activity:?}");
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(cpu, before);
    }
}

#[test]
fn step_refuses_a_vcpu_that_is_not_running() {
    for activity in NOT_RUNNING {
        let mut cpu = not_running(activity);
        let before = cpu.clone();
        let outcome = execute::step(&mut cpu, &mut Untouched, ring0()).unwrap();
        assert_eq!(outcome, StepOutcome::NotRunning, "{activity:?}");
        assert_eq!(cpu, before);
    }
}

#[test]
fn run_with_maskable_request_refuses_a_vcpu_that_is_not_running_even_with_a_request() {
    for activity in NOT_RUNNING {
        let mut cpu = not_running(activity);
        cpu.rflags |= 1 << 9; // IF = 1: a request would be eligible for a running vCPU
        let before = cpu.clone();
        let result = execute::run_with_maskable_request(
            &mut cpu,
            &mut Untouched,
            config(),
            od_isa::INTEGER_FLAGS,
            Some(0x30),
            || false,
        );
        assert_eq!(
            result.unwrap_err(),
            MaskableRequestRunInputError::NotRunning,
            "{activity:?}"
        );
        assert_eq!(cpu, before);
    }
}

#[test]
fn run_configured_through_a_cr_profile_entry_refuses_a_vcpu_that_is_not_running() {
    // `run_configured` is private; `run_with_cr0_profile` is one of its public entries. The
    // refusal precedes every profile check, so the state needs no CR0.
    for activity in NOT_RUNNING {
        let mut cpu = not_running(activity);
        let before = cpu.clone();
        let result = execute::run_with_cr0_profile(
            &mut cpu,
            &mut Untouched,
            config(),
            Cr0Profile::FpInitLong64V1,
            || false,
        );
        assert_eq!(result.exit, RunExit::NotRunning, "{activity:?}");
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(cpu, before);
    }
}

#[test]
fn step_pio_u8_refuses_a_vcpu_that_is_not_running() {
    for activity in NOT_RUNNING {
        let mut cpu = not_running(activity);
        let before = cpu.clone();
        let result = execute::step_pio_u8(
            DiagnosticCpl0PioU8V1,
            &mut cpu,
            &mut Untouched,
            &mut NoPorts,
            ring0(),
            od_isa::INTEGER_FLAGS,
        );
        assert_eq!(
            result.exit,
            PioU8Exit::Rejected(PioU8AdmissionError::NotRunning),
            "{activity:?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(cpu, before);
    }
}

#[test]
fn step_paged_pio_u8_refuses_a_vcpu_that_is_not_running() {
    for activity in NOT_RUNNING {
        let mut cpu = not_running(activity);
        cpu.cr0 = Some(Cr0Profile::BASE);
        cpu.cr4 = Some(od_isa::execute::Cr4Profile::BASE);
        let before = cpu.clone();
        let result = execute::step_paged_pio_u8(
            PagedCpl0PioU8V1,
            &mut cpu,
            &mut Untouched,
            &mut NoPorts,
            AccessContext {
                mode: InstructionMode::Long64,
                privilege: PrivilegeLevel::Ring0,
                ..AccessContext::default()
            },
            od_isa::INTEGER_FLAGS,
        );
        assert_eq!(
            result.exit,
            PioU8Exit::Rejected(PioU8AdmissionError::NotRunning),
            "{activity:?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(cpu, before);
    }
}

#[test]
fn run_with_rep_continuation_refuses_a_vcpu_that_is_not_running() {
    for activity in NOT_RUNNING {
        let mut cpu = not_running(activity);
        let before = cpu.clone();
        let result = execute::run_with_rep_continuation(
            &mut cpu,
            &mut Untouched,
            config(),
            od_isa::INTEGER_FLAGS,
            &mut None,
            || false,
        );
        assert_eq!(
            result.unwrap_err(),
            RepRunInputError::NotRunning,
            "{activity:?}"
        );
        assert_eq!(cpu, before);
    }
}

#[test]
fn step_mov_cr3_refuses_wait_for_sipi_and_keeps_its_shutdown_exit() {
    // `step_mov_cr3` reaches `step_mov_cr3_with_profiles`. WaitForSipi is a new input error;
    // Shutdown keeps the exit it always had (nothing executed, the exit names the activity).
    let mut cpu = not_running(CpuActivity::WaitForSipi);
    let before = cpu.clone();
    let result = execute::step_mov_cr3(
        MovCr3Long64V1,
        &mut cpu,
        &mut Untouched,
        AccessContext {
            mode: InstructionMode::Long64,
            privilege: PrivilegeLevel::Ring0,
            ..AccessContext::default()
        },
        0xd00,
        od_isa::INTEGER_FLAGS,
    );
    assert_eq!(result.unwrap_err(), MovCr3InputError::NotRunning);
    assert_eq!(cpu, before);

    let mut cpu = not_running(CpuActivity::Shutdown);
    let before = cpu.clone();
    let result = execute::step_mov_cr3(
        MovCr3Long64V1,
        &mut cpu,
        &mut Untouched,
        ring0(),
        0xd00,
        od_isa::INTEGER_FLAGS,
    )
    .unwrap();
    assert_eq!(result.exit, MovCr3Exit::Shutdown);
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(cpu, before);
}

#[test]
fn running_and_halted_keep_their_behaviour() {
    // Running executes (a NOP then the stop address); Halted is reported as before.
    let mut cpu = state();
    cpu.activity = Some(CpuActivity::Running);
    let mut memory = Ram::with_code(&[0x90]);
    let result = execute::run(
        &mut cpu,
        &mut memory,
        RunConfig {
            instruction_budget: 5,
            stop_rip: Some(CODE + 1),
            context: ring0(),
        },
        || false,
    );
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 1);

    let mut cpu = state();
    cpu.activity = Some(CpuActivity::Halted);
    let before = cpu.clone();
    assert_eq!(
        execute::step(&mut cpu, &mut Untouched, ring0()).unwrap(),
        StepOutcome::Halted
    );
    assert_eq!(cpu, before);
    let result = execute::run(&mut cpu, &mut Untouched, config(), || false);
    assert_eq!(result.exit, RunExit::Halted);
    assert_eq!(result.instructions_executed, 0);
}
