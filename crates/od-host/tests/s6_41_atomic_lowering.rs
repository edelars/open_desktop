//! S6-41: native execution of the ARM64 sequences that `od_jit::arm64::memory_order` lowers the
//! three memory-effect classes (ordinary access, atomic read-modify-write, fence) to.
//!
//! 1. Differential: every `LOCK`/`XCHG` memory form the interpreter runs through
//!    `GuestMemory::atomic_rmw_integer` (S6-20) is run once via
//!    `od_isa::execute::run_with_cpl0_profile_set` and once as the generated host sequence, in both modes, on the same initial memory; the final memory
//!    bytes (operand and a guard region around it) and the value the instruction returns to the
//!    guest (`XADD`/`XCHG` source register, `CMPXCHG` ZF and accumulator) must be equal (S6-41,
//!    `s6_41_lock_forms_generated_code_equals_the_interpreter_in_both_modes`; the flags there are
//!    not compared, which is what the next item closes).
//! 1b. S6-41b differential (`s6_41b_*`): the instruction bytes are decoded, lowered by
//!    `od_jit::atomic_adapter` to IR, and emitted by `od_jit::arm64::atomic_sequence`
//!    (the atomic instruction, the six arithmetic flags computed inline, the register writes); the
//!    final memory, ALL guest GPRs and the whole RFLAGS must equal the interpreter's, from random
//!    initial flags, in both modes, for the S6-41 forms and for `LOCK OR/AND/BTS/BTR`.
//! 2. Atomicity: several host threads run the generated parallel-mode sequences on one cell.
//! 3. Ordering: message passing (MP) and store buffering (SB) on generated code, with the parallel
//!    lowering of the accesses and with plain accesses plus the parallel lowering of the fences.
//!    The uncontrolled variants are printed, never asserted: ARM allows their weak outcomes.
//!    The opt-in S6-41e probe requires a weak SB outcome with NOP in place of the compiled MFENCE.
//!
//! Generated code is called through `PublishedCode`; the host has one `MAP_JIT` region, so every
//! test takes `JIT_LOCK` and publishes one buffer at a time.
#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Barrier, Mutex};

use od_host::PublishedCode;
use od_isa::execute::{
    ControlRegisterPolicy, Cpl0ProfileSet, Cpl0ProfileSetExit, RunConfig, RunExit,
};
use od_isa::long_decode::{self, AddressBase as GuestBase, MemoryOperand};
use od_isa::{
    AccessContext, AccessKind, AccessOrigin, CpuActivity, GuestMemory, InstructionMode,
    LongModeState, MemoryError, OperandSize, PrivilegeLevel, StiShadow,
};
use od_jit::arm64::atomic_sequence::{SequenceEnv, emit_atomic_program};
use od_jit::arm64::memory_order::{HostOperands, MemoryOrderMode, lower_memory_effect};
use od_jit::arm64::{AddressBase, Encoder, WReg, XReg, XRegOrZero};
use od_jit::atomic_adapter::lower_atomic_instruction;
use od_jit::atomic_block::compile_atomic_block;
use od_jit::ir::{
    AtomicFlagsKind, AtomicRmwKind, FenceKind, GuestGpr, IntWidth, Operation, Program, TempId,
};
use od_jit::runtime::LONG_MODE_GPR_OFFSET;

static JIT_LOCK: Mutex<()> = Mutex::new(());

const CODE: u64 = 0x2000;
const CELL: u64 = 0x1000;
/// Guard bytes compared on each side of the operand.
const GUARD_BEFORE: usize = 8;
const GUARD_AFTER: usize = 16;

fn x(number: u8) -> XReg {
    XReg::new(number).unwrap()
}

fn ring0() -> AccessContext {
    AccessContext {
        mode: InstructionMode::Long64,
        privilege: PrivilegeLevel::Ring0,
        origin: AccessOrigin::Explicit,
    }
}

// ---- interpreter side ----

struct Ram {
    bytes: Vec<u8>,
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
        let at = address as usize;
        out.copy_from_slice(&self.bytes[at..at + out.len()]);
        Ok(())
    }

    fn write(&mut self, address: u64, _: AccessContext, source: &[u8]) -> Result<(), MemoryError> {
        let at = address as usize;
        self.bytes[at..at + source.len()].copy_from_slice(source);
        Ok(())
    }

    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        Ok(())
    }
}

// ---- forms ----

#[derive(Clone, Copy, Debug)]
enum Returns {
    /// Nothing but memory (`LOCK ADD/INC/DEC`).
    Memory,
    /// The old value lands in RBX (`XADD`, `XCHG`).
    Rbx,
    /// `CMPXCHG`: ZF, and the old value in RAX on a mismatch.
    CmpxchgRax,
}

#[derive(Clone, Copy, Debug)]
struct Form {
    name: &'static str,
    /// Instruction bytes (absolute `[0x1000]` through SIB).
    code: &'static [u8],
    kind: AtomicRmwKind,
    size: OperandSize,
    returns: Returns,
    /// For forms with a fixed addend (`INC`, `DEC`, `ADD m8, imm8`): the value passed as `value`.
    fixed_value: Option<u64>,
}

const FORMS: &[Form] = &[
    Form {
        name: "lock xadd m32, r32",
        code: &[0xf0, 0x0f, 0xc1, 0x1c, 0x25, 0, 0x10, 0, 0],
        kind: AtomicRmwKind::Add,
        size: OperandSize::Dword,
        returns: Returns::Rbx,
        fixed_value: None,
    },
    Form {
        name: "lock xadd m64, r64",
        code: &[0xf0, 0x48, 0x0f, 0xc1, 0x1c, 0x25, 0, 0x10, 0, 0],
        kind: AtomicRmwKind::Add,
        size: OperandSize::Qword,
        returns: Returns::Rbx,
        fixed_value: None,
    },
    Form {
        name: "lock add m32, r32",
        code: &[0xf0, 0x01, 0x1c, 0x25, 0, 0x10, 0, 0],
        kind: AtomicRmwKind::Add,
        size: OperandSize::Dword,
        returns: Returns::Memory,
        fixed_value: None,
    },
    Form {
        name: "lock add m64, r64",
        code: &[0xf0, 0x48, 0x01, 0x1c, 0x25, 0, 0x10, 0, 0],
        kind: AtomicRmwKind::Add,
        size: OperandSize::Qword,
        returns: Returns::Memory,
        fixed_value: None,
    },
    Form {
        name: "lock add m8, imm8 (0xf5)",
        code: &[0xf0, 0x80, 0x04, 0x25, 0, 0x10, 0, 0, 0xf5],
        kind: AtomicRmwKind::Add,
        size: OperandSize::Byte,
        returns: Returns::Memory,
        fixed_value: Some(0xf5),
    },
    Form {
        name: "lock inc m32",
        code: &[0xf0, 0xff, 0x04, 0x25, 0, 0x10, 0, 0],
        kind: AtomicRmwKind::Add,
        size: OperandSize::Dword,
        returns: Returns::Memory,
        fixed_value: Some(1),
    },
    Form {
        name: "lock dec m64",
        code: &[0xf0, 0x48, 0xff, 0x0c, 0x25, 0, 0x10, 0, 0],
        kind: AtomicRmwKind::Add,
        size: OperandSize::Qword,
        returns: Returns::Memory,
        fixed_value: Some(u64::MAX),
    },
    Form {
        name: "lock cmpxchg m32, r32",
        code: &[0xf0, 0x0f, 0xb1, 0x1c, 0x25, 0, 0x10, 0, 0],
        kind: AtomicRmwKind::CompareExchange,
        size: OperandSize::Dword,
        returns: Returns::CmpxchgRax,
        fixed_value: None,
    },
    Form {
        name: "lock cmpxchg m64, r64",
        code: &[0xf0, 0x48, 0x0f, 0xb1, 0x1c, 0x25, 0, 0x10, 0, 0],
        kind: AtomicRmwKind::CompareExchange,
        size: OperandSize::Qword,
        returns: Returns::CmpxchgRax,
        fixed_value: None,
    },
    Form {
        name: "xchg m32, r32",
        code: &[0x87, 0x1c, 0x25, 0, 0x10, 0, 0],
        kind: AtomicRmwKind::Exchange,
        size: OperandSize::Dword,
        returns: Returns::Rbx,
        fixed_value: None,
    },
    Form {
        name: "xchg m64, r64",
        code: &[0x48, 0x87, 0x1c, 0x25, 0, 0x10, 0, 0],
        kind: AtomicRmwKind::Exchange,
        size: OperandSize::Qword,
        returns: Returns::Rbx,
        fixed_value: None,
    },
];

fn int_width(size: OperandSize) -> IntWidth {
    match size {
        OperandSize::Byte => IntWidth::W8,
        OperandSize::Word => IntWidth::W16,
        OperandSize::Dword => IntWidth::W32,
        OperandSize::Qword => IntWidth::W64,
    }
}

fn operand(size: OperandSize) -> MemoryOperand {
    MemoryOperand {
        segment: od_isa::long_decode::MemorySegment::Flat,
        base: GuestBase::None,
        index: None,
        scale: 1,
        displacement: CELL as i64,
        address_size: OperandSize::Qword,
        size,
    }
}

fn atomic_effect(kind: AtomicRmwKind, size: OperandSize) -> Operation {
    Operation::AtomicRmw {
        kind,
        width: int_width(size),
        operand: operand(size),
        value: TempId(1),
        expected: (kind == AtomicRmwKind::CompareExchange).then_some(TempId(2)),
        dst: TempId(3),
    }
}

// ---- generated code ----

/// One published buffer holding many small functions.
struct Image {
    encoder: Encoder,
    entries: Vec<(String, usize)>,
}

impl Image {
    fn new() -> Self {
        Self {
            encoder: Encoder::new(),
            entries: Vec::new(),
        }
    }

    fn begin(&mut self, name: &str) {
        self.entries
            .push((name.to_string(), self.encoder.position() * 4));
    }

    /// `fn(address: x0, value: x1, expected: x2) -> old: x0`.
    fn rmw(&mut self, name: &str, mode: MemoryOrderMode, kind: AtomicRmwKind, size: OperandSize) {
        self.begin(name);
        let host = HostOperands {
            address: AddressBase::Reg(x(0)),
            dst: Some(x(3)),
            value: Some(XRegOrZero::Reg(x(1))),
            expected: Some(x(2)),
        };
        lower_memory_effect(&mut self.encoder, mode, &atomic_effect(kind, size), &host).unwrap();
        self.encoder.mov_x(x(0), XRegOrZero::Reg(x(3)));
        self.encoder.ret(x(30));
    }

    fn finish(self) -> (Vec<u8>, Vec<(String, usize)>) {
        let bytes = self.encoder.finish().unwrap().to_le_bytes().unwrap();
        (bytes, self.entries)
    }
}

fn entry(code: &PublishedCode, entries: &[(String, usize)], name: &str) -> usize {
    let offset = entries
        .iter()
        .find(|(entry, _)| entry == name)
        .unwrap_or_else(|| panic!("no entry {name}"))
        .1;
    code.as_ptr() as usize + offset
}

type RmwFn = unsafe extern "C" fn(*mut u8, u64, u64) -> u64;

fn rmw_fn(address: usize) -> RmwFn {
    // SAFETY: `address` is an entry of a live `PublishedCode` built by `Image::rmw`, which
    // follows the AAPCS64 signature above.
    unsafe { std::mem::transmute::<usize, RmwFn>(address) }
}

fn mask(size: OperandSize) -> u64 {
    match size.bytes() {
        8 => u64::MAX,
        n => (1u64 << (8 * n)) - 1,
    }
}

fn mode_name(mode: MemoryOrderMode) -> &'static str {
    match mode {
        MemoryOrderMode::Interleaved => "interleaved",
        MemoryOrderMode::Parallel => "parallel",
    }
}

const MODES: [MemoryOrderMode; 2] = [MemoryOrderMode::Interleaved, MemoryOrderMode::Parallel];

fn fn_name(form: &Form, mode: MemoryOrderMode) -> String {
    format!("{}/{}", form.name, mode_name(mode))
}

#[repr(align(16))]
struct Buffer([u8; 32]);

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

const EDGES: [u64; 10] = [
    0,
    1,
    2,
    0x7f,
    0x80,
    0xff,
    0x7fff_ffff,
    0x8000_0000,
    0xffff_ffff,
    u64::MAX,
];

#[test]
fn s6_41_lock_forms_generated_code_equals_the_interpreter_in_both_modes() {
    let _guard = JIT_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut image = Image::new();
    for form in FORMS {
        for mode in MODES {
            image.rmw(&fn_name(form, mode), mode, form.kind, form.size);
        }
    }
    let (bytes, entries) = image.finish();
    let code = PublishedCode::publish(&bytes).unwrap();

    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut cases = 0usize;
    for form in FORMS {
        let width_mask = mask(form.size);
        let bytes_in_operand = form.size.bytes();
        for round in 0..96 {
            let pick = |rng: &mut Rng| {
                if rng.next() % 3 == 0 {
                    EDGES[(rng.next() % EDGES.len() as u64) as usize]
                } else {
                    rng.next()
                }
            };
            let initial = pick(&mut rng);
            let source = form.fixed_value.unwrap_or_else(|| pick(&mut rng));
            // Half of the CMPXCHG cases compare equal (success path).
            let expected = if round % 2 == 0 {
                initial
            } else {
                pick(&mut rng)
            };

            // Interpreter.
            let mut ram = Ram {
                bytes: vec![0; 0x4000],
            };
            ram.bytes[CODE as usize..CODE as usize + form.code.len()].copy_from_slice(form.code);
            for offset in 0..GUARD_BEFORE + 8 + GUARD_AFTER {
                ram.bytes[CELL as usize - GUARD_BEFORE + offset] = 0xa5 ^ (offset as u8);
            }
            ram.bytes[CELL as usize..CELL as usize + bytes_in_operand]
                .copy_from_slice(&initial.to_le_bytes()[..bytes_in_operand]);
            // The LOCK forms are admitted by the CPL0 profile set only (S6-20), not by a bare
            // `step`; state as the board entry of that set.
            let mut state = LongModeState {
                rip: CODE,
                rflags: 2 | (1 << 9),
                cr0: Some(0x8001_0033),
                cr4: Some(0x20),
                activity: Some(CpuActivity::Running),
                sti_shadow: Some(StiShadow::Clear),
                ds_selector: 0x18,
                es_selector: 0x18,
                ..LongModeState::default()
            };
            state.gpr[0] = expected; // RAX
            state.gpr[3] = source; // RBX
            let result = od_isa::execute::run_with_cpl0_profile_set(
                &mut state,
                &mut ram,
                RunConfig {
                    instruction_budget: 1,
                    stop_rip: None,
                    context: ring0(),
                },
                od_isa::INTEGER_FLAGS,
                &Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1),
                None,
                || false,
            )
            .unwrap();
            assert_eq!(result.instructions_executed, 1, "{}", form.name);
            assert!(
                matches!(
                    result.exit,
                    Cpl0ProfileSetExit::Run(RunExit::InstructionBudgetExhausted)
                ),
                "{}: {:?}",
                form.name,
                result.exit
            );
            assert_eq!(state.rip, CODE + form.code.len() as u64, "{}", form.name);
            let reference =
                &ram.bytes[CELL as usize - GUARD_BEFORE..CELL as usize + 8 + GUARD_AFTER];

            for mode in MODES {
                let mut buffer = Buffer([0; 32]);
                for (offset, byte) in buffer
                    .0
                    .iter_mut()
                    .enumerate()
                    .take(GUARD_BEFORE + 8 + GUARD_AFTER)
                {
                    *byte = 0xa5 ^ (offset as u8);
                }
                buffer.0[GUARD_BEFORE..GUARD_BEFORE + bytes_in_operand]
                    .copy_from_slice(&initial.to_le_bytes()[..bytes_in_operand]);
                // SAFETY: the entry is the generated function for this (form, mode); the cell
                // is naturally aligned (offset 8 of a 16-aligned buffer) and lives through
                // the call.
                let old = unsafe {
                    rmw_fn(entry(&code, &entries, &fn_name(form, mode)))(
                        buffer.0.as_mut_ptr().add(GUARD_BEFORE),
                        source,
                        expected,
                    )
                };
                let label = format!(
                    "{} [{}] initial={initial:#x} source={source:#x} expected={expected:#x}",
                    form.name,
                    mode_name(mode)
                );
                assert_eq!(
                    &buffer.0[..GUARD_BEFORE + 8 + GUARD_AFTER],
                    reference,
                    "memory: {label}"
                );
                // The old value is zero-extended to the operand width.
                assert_eq!(old, initial & width_mask, "old value: {label}");
                match form.returns {
                    Returns::Memory => {}
                    Returns::Rbx => {
                        assert_eq!(
                            old,
                            state.gpr[3] & width_mask,
                            "returned old value: {label}"
                        );
                    }
                    Returns::CmpxchgRax => {
                        // This ZF is the interpreter's flag, checked against the expected
                        // value derived from the old memory value; this bare `rmw` sequence
                        // does not produce flags, so this checks the old value and RAX, not
                        // a JIT-computed flag. The JIT-computed ZF and the other five flags
                        // are compared in `s6_41b_adapted_atomic_sequences_equal_...`.
                        let zf = state.rflags & (1 << 6) != 0;
                        assert_eq!(zf, old == expected & width_mask, "ZF: {label}");
                        if !zf {
                            assert_eq!(old, state.gpr[0] & width_mask, "RAX: {label}");
                        }
                    }
                }
                cases += 1;
            }
        }
    }
    assert_eq!(cases, FORMS.len() * 96 * 2);
    eprintln!("s6_41 differential: {cases} cases (forms x rounds x modes), all equal");
}

#[test]
fn s6_41_parallel_atomic_forms_lose_no_updates_across_threads() {
    let _guard = JIT_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut image = Image::new();
    image.rmw(
        "add32",
        MemoryOrderMode::Parallel,
        AtomicRmwKind::Add,
        OperandSize::Dword,
    );
    image.rmw(
        "add64",
        MemoryOrderMode::Parallel,
        AtomicRmwKind::Add,
        OperandSize::Qword,
    );
    image.rmw(
        "cas32",
        MemoryOrderMode::Parallel,
        AtomicRmwKind::CompareExchange,
        OperandSize::Dword,
    );
    image.rmw(
        "swap64",
        MemoryOrderMode::Parallel,
        AtomicRmwKind::Exchange,
        OperandSize::Qword,
    );
    let (bytes, entries) = image.finish();
    let code = PublishedCode::publish(&bytes).unwrap();
    let add32 = entry(&code, &entries, "add32");
    let add64 = entry(&code, &entries, "add64");
    let cas32 = entry(&code, &entries, "cas32");
    let swap64 = entry(&code, &entries, "swap64");

    const THREADS: u64 = 4;
    const ITERATIONS: u64 = 100_000;
    #[repr(align(16))]
    struct Cells([AtomicU64; 4]);
    let cells = Cells([
        AtomicU64::new(0),
        AtomicU64::new(0),
        AtomicU64::new(0),
        AtomicU64::new(0),
    ]);
    let swaps = AtomicU64::new(0);
    std::thread::scope(|scope| {
        for thread in 0..THREADS {
            let cells = &cells;
            let swaps = &swaps;
            scope.spawn(move || {
                let add32 = rmw_fn(add32);
                let add64 = rmw_fn(add64);
                let cas32 = rmw_fn(cas32);
                let swap64 = rmw_fn(swap64);
                for _ in 0..ITERATIONS {
                    // SAFETY: the cells are 8-aligned and outlive the scope; the generated code
                    // only touches the operand width at each address.
                    unsafe {
                        add32(cells.0[0].as_ptr().cast(), 1, 0);
                        add64(cells.0[1].as_ptr().cast(), 3, 0);
                        // CMPXCHG increment loop on the low dword of cell 2.
                        loop {
                            let seen = u64::from(
                                (*(cells.0[2].as_ptr().cast::<AtomicU32>()))
                                    .load(Ordering::Relaxed),
                            );
                            let old = cas32(cells.0[2].as_ptr().cast(), seen + 1, seen);
                            if old == seen {
                                break;
                            }
                        }
                        // Exchange: every thread swaps in its own tag and counts how many times
                        // it received a non-initial tag; each swap returns exactly one old value.
                        let old = swap64(cells.0[3].as_ptr().cast(), thread + 1, 0);
                        if old != 0 {
                            swaps.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            });
        }
    });
    assert_eq!(
        cells.0[0].load(Ordering::SeqCst) & 0xffff_ffff,
        THREADS * ITERATIONS
    );
    assert_eq!(cells.0[1].load(Ordering::SeqCst), 3 * THREADS * ITERATIONS);
    assert_eq!(
        cells.0[2].load(Ordering::SeqCst) & 0xffff_ffff,
        THREADS * ITERATIONS
    );
    // The first swap of the whole run sees 0; every other swap sees a tag, so exactly one of the
    // THREADS * ITERATIONS exchanges returned the initial zero.
    assert_eq!(swaps.load(Ordering::SeqCst), THREADS * ITERATIONS - 1);
    let last = cells.0[3].load(Ordering::SeqCst);
    assert!((1..=THREADS).contains(&last));
}

// ---- ordering litmus on generated code ----

#[derive(Clone, Copy)]
struct Shape {
    accesses: MemoryOrderMode,
    fences: MemoryOrderMode,
}

fn dword() -> MemoryOperand {
    operand(OperandSize::Dword)
}

fn host_with(address: u8, dst: Option<u8>, value: Option<XRegOrZero>) -> HostOperands {
    HostOperands {
        address: AddressBase::Reg(x(address)),
        dst: dst.map(x),
        value,
        expected: None,
    }
}

fn store(image: &mut Image, mode: MemoryOrderMode, address: u8) {
    let op = Operation::CheckedStore {
        src: GuestGpr(0),
        operand: dword(),
    };
    let host = host_with(address, None, Some(XRegOrZero::Reg(x(9))));
    lower_memory_effect(&mut image.encoder, mode, &op, &host).unwrap();
}

fn load(image: &mut Image, mode: MemoryOrderMode, address: u8, dst: u8) {
    let op = Operation::CheckedLoad {
        dst: TempId(0),
        operand: dword(),
    };
    lower_memory_effect(
        &mut image.encoder,
        mode,
        &op,
        &host_with(address, Some(dst), None),
    )
    .unwrap();
}

fn fence(image: &mut Image, mode: MemoryOrderMode, kind: FenceKind) {
    let op = Operation::Fence { kind };
    lower_memory_effect(&mut image.encoder, mode, &op, &host_with(0, None, None)).unwrap();
}

fn one(image: &mut Image) {
    image.encoder.movz_x(x(9), 1, 0).unwrap();
}

fn ret_with(image: &mut Image, result: u8) {
    image.encoder.mov_x(x(0), XRegOrZero::Reg(x(result)));
    image.encoder.ret(x(30));
}

/// MP writer `fn(data: x0, flag: x1)`: data = 1; [fence Store]; flag = 1.
fn mp_writer(image: &mut Image, name: &str, shape: Shape) {
    image.begin(name);
    one(image);
    store(image, shape.accesses, 0);
    fence(image, shape.fences, FenceKind::Store);
    store(image, shape.accesses, 1);
    ret_with(image, 9);
}

/// MP reader `fn(data: x0, flag: x1) -> data`: spin until flag != 0; [fence Load]; load data.
fn mp_reader(image: &mut Image, name: &str, shape: Shape) {
    image.begin(name);
    let wait = image.encoder.new_label().unwrap();
    image.encoder.bind(wait).unwrap();
    load(image, shape.accesses, 1, 3);
    image.encoder.cbz_w(WReg::new(3).unwrap(), wait).unwrap();
    fence(image, shape.fences, FenceKind::Load);
    load(image, shape.accesses, 0, 4);
    ret_with(image, 4);
}

/// SB thread `fn(mine: x0, other: x1) -> other value`: mine = 1; [fence Full]; load other.
/// Returns the byte offset where the fence is emitted.
fn sb_thread(image: &mut Image, name: &str, shape: Shape) -> usize {
    image.begin(name);
    one(image);
    store(image, shape.accesses, 0);
    let fence_offset = image.encoder.position() * 4;
    fence(image, shape.fences, FenceKind::Full);
    load(image, shape.accesses, 1, 3);
    ret_with(image, 3);
    fence_offset
}

#[repr(align(128))]
struct Cell(AtomicU32);

type PairFn = unsafe extern "C" fn(*mut u32, *mut u32) -> u64;

fn pair_fn(address: usize) -> PairFn {
    // SAFETY: an entry of a live `PublishedCode` emitted by the helpers above, two pointer
    // arguments in x0/x1, one result in x0.
    unsafe { std::mem::transmute::<usize, PairFn>(address) }
}

/// Runs `rounds` rounds: `main_fn(main_args)` on this thread races `worker_fn(worker_args)` on a
/// second one. Returns the `(main result, worker result)` of each round through `record`.
fn race(
    rounds: u64,
    main_fn: PairFn,
    main_args: (*mut u32, *mut u32),
    worker_fn: PairFn,
    worker_args: (*mut u32, *mut u32),
    cells: [&Cell; 2],
    mut record: impl FnMut(u64, u64),
) {
    let start = AtomicU64::new(0);
    let done = AtomicU64::new(0);
    let ready = AtomicU64::new(0);
    let ready_main = AtomicU64::new(0);
    let worker_result = AtomicU64::new(0);
    let main_args = (main_args.0 as usize, main_args.1 as usize);
    let worker_args = (worker_args.0 as usize, worker_args.1 as usize);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let mut round = 1;
            loop {
                let mut seen;
                loop {
                    seen = start.load(Ordering::Acquire);
                    if seen >= round {
                        break;
                    }
                    std::hint::spin_loop();
                }
                if seen == u64::MAX {
                    return;
                }
                // Two-sided rendezvous: announce, wait for the other side, then both fire at once.
                ready.store(round, Ordering::Release);
                while ready_main.load(Ordering::Acquire) != round {
                    std::hint::spin_loop();
                }
                // SAFETY: pointers are into `Cell`s that outlive the scope; the function was
                // generated for exactly these two-pointer arguments.
                let value =
                    unsafe { worker_fn(worker_args.0 as *mut u32, worker_args.1 as *mut u32) };
                worker_result.store(value, Ordering::Relaxed);
                done.store(round, Ordering::Release);
                round += 1;
            }
        });
        for round in 1..=rounds {
            for cell in cells {
                cell.0.store(0, Ordering::Relaxed);
            }
            start.store(round, Ordering::Release);
            ready_main.store(round, Ordering::Release);
            while ready.load(Ordering::Acquire) != round {
                std::hint::spin_loop();
            }
            // SAFETY: as above.
            let value = unsafe { main_fn(main_args.0 as *mut u32, main_args.1 as *mut u32) };
            while done.load(Ordering::Acquire) != round {
                std::hint::spin_loop();
            }
            record(value, worker_result.load(Ordering::Relaxed));
        }
        start.store(u64::MAX, Ordering::Release);
    });
}

#[test]
#[ignore = "hardware sensitivity run: requires observing relaxed ARM64 store buffering"]
fn s6_41e_compiled_mfence_prevents_store_buffering() {
    let _guard = JIT_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    const ROUNDS: u64 = 2_000_000;
    const DMB_ISH: u32 = 0xd503_3bbf;
    const NOP: u32 = 0xd503_201f;

    let block = compile_atomic_block(&adapted(&[0x0f, 0xae, 0xf0]), MemoryOrderMode::Parallel)
        .expect("compile the guest MFENCE through the V4 path");
    let [instruction] = block.layout().instructions.as_slice() else {
        panic!("expected one guest instruction in the block");
    };
    assert!(instruction.helper_call_word.is_none());
    let fence_word = block.words()[instruction.effects_start];
    // Only this position-independent instruction can be run outside the V4 ABI.
    assert_eq!(fence_word, DMB_ISH);

    let mut image = Image::new();
    let offset = sb_thread(
        &mut image,
        "sb",
        Shape {
            accesses: MemoryOrderMode::Interleaved,
            fences: MemoryOrderMode::Parallel,
        },
    );
    let (mut bytes, entries) = image.finish();
    let fence_bytes = offset..offset + 4;
    assert_eq!(bytes[fence_bytes.clone()], DMB_ISH.to_le_bytes());
    let mine = Cell(AtomicU32::new(0));
    let other = Cell(AtomicU32::new(0));
    let mine_ptr = mine.0.as_ptr();
    let other_ptr = other.0.as_ptr();
    for (variant, word) in [("MFENCE", fence_word), ("NOP", NOP)] {
        bytes[fence_bytes.clone()].copy_from_slice(&word.to_le_bytes());
        let code = PublishedCode::publish(&bytes).unwrap();
        let sb = pair_fn(entry(&code, &entries, "sb"));
        let mut histogram = [0u64; 4];
        race(
            ROUNDS,
            sb,
            (mine_ptr, other_ptr),
            sb,
            (other_ptr, mine_ptr),
            [&mine, &other],
            |main, worker| histogram[(main * 2 + worker) as usize] += 1,
        );
        eprintln!("s6_41e compiled MFENCE SB [{variant}]: rounds={ROUNDS} outcomes={histogram:?}");
        if word == NOP {
            assert!(
                histogram[0] > 0,
                "inconclusive: the NOP control must observe (0,0)"
            );
        } else {
            assert_eq!(histogram[0], 0, "compiled MFENCE admitted (0,0)");
        }
    }
}

struct NativeSbThread {
    store: MemoryOperand,
    load: MemoryOperand,
    value: u16,
    result: od_isa::Register,
    fence: Option<u32>,
    bytes: Vec<u8>,
    instructions: u64,
}

fn decode_native_sb_thread(program: &[od_machine::LitmusInstruction]) -> NativeSbThread {
    use long_decode::{Operand, Operation as GuestOp};
    let (store, fence, load) = match program {
        [store, load] => (store, None, load),
        [store, fence, load] => (store, Some(fence), load),
        _ => panic!("expected the SB guest thread"),
    };
    let decode = |bytes: &[u8]| {
        let instruction = long_decode::decode(&mut CodeBytes(bytes.to_vec()), 0, ring0()).unwrap();
        assert_eq!(usize::from(instruction.length), bytes.len());
        instruction.operation
    };
    let GuestOp::MovImmediate {
        dst: Operand::Memory(store_operand),
        value,
    } = decode(&store.bytes)
    else {
        panic!("expected the guest immediate store");
    };
    let GuestOp::Mov {
        dst: Operand::Register(result),
        src: Operand::Memory(load_operand),
    } = decode(&load.bytes)
    else {
        panic!("expected the guest load");
    };
    for operand in [store_operand, load_operand] {
        assert_eq!(operand.size, OperandSize::Dword);
        assert_eq!(operand.address_size, OperandSize::Qword);
        assert_eq!(operand.base, GuestBase::None);
        assert!(operand.index.is_none());
        assert!(matches!(operand.displacement, 0x1000 | 0x1004));
    }
    assert_ne!(store_operand.displacement, load_operand.displacement);
    assert_eq!(result.size(), OperandSize::Dword);
    assert!(matches!(
        result.register(),
        od_isa::Register::Rax | od_isa::Register::Rbx
    ));
    assert_eq!(value, 1);
    let fence = fence.map(|instruction| {
        assert_eq!(instruction.bytes, [0x0f, 0xae, 0xf0]);
        let block =
            compile_atomic_block(&adapted(&instruction.bytes), MemoryOrderMode::Parallel).unwrap();
        let [instruction] = block.layout().instructions.as_slice() else {
            panic!("expected one compiled MFENCE");
        };
        assert!(instruction.helper_call_word.is_none());
        let word = block.words()[instruction.effects_start];
        assert_eq!(word, 0xd503_3bbf);
        word
    });
    NativeSbThread {
        store: store_operand,
        load: load_operand,
        value: value as u16,
        result: result.register(),
        fence,
        bytes: program
            .iter()
            .flat_map(|instruction| instruction.bytes.iter().copied())
            .collect(),
        instructions: program.len() as u64,
    }
}

struct NativeSbHook {
    threads: Vec<NativeSbThread>,
    entries: Vec<PairFn>,
}

impl od_machine::LitmusStepHook for NativeSbHook {
    fn step(
        &self,
        step: od_machine::LitmusStep<'_>,
    ) -> Result<od_machine::LitmusStepOutcome, String> {
        use od_machine::{GuestLinearAddress, Long64ProbeConfig, ProbeOutcome, probe_long64_4k};

        let thread = &self.threads[step.thread];
        assert_eq!(step.index, 0);
        assert!(step.budget >= thread.instructions);
        let memory = step.bus.atomic_inline_memory();
        let config = Long64ProbeConfig::new(
            step.state.cr0.unwrap(),
            step.cr3,
            step.state.cr4.unwrap(),
            step.efer,
        )
        .unwrap();
        let locate = |linear, kind| {
            let probe = probe_long64_4k(
                memory,
                &config,
                GuestLinearAddress::new(linear),
                kind,
                ring0(),
            );
            let ProbeOutcome::Translated(translation) = probe.outcome.unwrap() else {
                panic!("the SB cell or code must be mapped");
            };
            assert_eq!(translation.physical.get(), linear);
            translation.physical
        };
        let rip = step.state.rip;
        assert!(matches!(rip, 0x2000 | 0x3000));
        let next_rip = rip + thread.bytes.len() as u64;
        let mut fetched = [0; 21];
        let fetched = &mut fetched[..thread.bytes.len()];
        memory
            .load_bytes(locate(rip, AccessKind::Fetch), fetched)
            .unwrap();
        assert_eq!(fetched, thread.bytes);
        let store_at = thread
            .store
            .access_address_48(step.state, next_rip)
            .unwrap();
        let load_at = thread.load.access_address_48(step.state, next_rip).unwrap();
        let store = memory
            .atomic_operand_window(locate(store_at, AccessKind::Write), 4)
            .unwrap();
        let load = memory
            .atomic_operand_window(locate(load_at, AccessKind::Read), 4)
            .unwrap();
        // SAFETY: the live PublishedCode keeps this entry mapped through the joined run.
        // The two validated operands are naturally aligned dwords in RAM, separate from all
        // code and page tables. The exact leaf body uses two pointer arguments and no helper.
        let value =
            unsafe { (self.entries[step.thread])(store.host_ptr().cast(), load.host_ptr().cast()) };
        step.state.gpr[thread.result as usize] = value;
        step.state.rip = next_rip;
        // These fixed data cells have no code observers inside the test block. Publish the
        // completed store after its paired load so the generation RMW cannot order the pair.
        store.generation().fetch_add(1, Ordering::Release);
        Ok(od_machine::LitmusStepOutcome::Executed {
            retired: thread.instructions,
        })
    }

    fn thread_exit(&self, _: usize) {}
}

fn native_guest_sb(
    fixture: &od_machine::LitmusCase,
    vcpus: usize,
    rounds: u64,
    mutate: bool,
) -> od_machine::LitmusParallelReport {
    let threads: Vec<_> = fixture
        .program()
        .iter()
        .map(|p| decode_native_sb_thread(p))
        .collect();
    let mut image = Image::new();
    let mut fence_words = Vec::new();
    for (thread, program) in threads.iter().enumerate() {
        image.begin(&thread.to_string());
        image.encoder.movz_x(x(9), program.value, 0).unwrap();
        // Use weak ordinary accesses on the real parallel board in all sensitivity controls.
        store(&mut image, MemoryOrderMode::Interleaved, 0);
        if let Some(word) = program.fence {
            let offset = image.encoder.position() * 4;
            fence(&mut image, MemoryOrderMode::Parallel, FenceKind::Full);
            fence_words.push((offset, word));
        }
        load(&mut image, MemoryOrderMode::Interleaved, 1, 3);
        ret_with(&mut image, 3);
    }
    let (mut bytes, addresses) = image.finish();
    for (offset, word) in fence_words {
        let place = &mut bytes[offset..offset + 4];
        assert_eq!(place, word.to_le_bytes());
        place.copy_from_slice(&if mutate { 0xd503_201f } else { word }.to_le_bytes());
    }
    let published = PublishedCode::publish(&bytes).unwrap();
    let entries = (0..threads.len())
        .map(|thread| pair_fn(entry(&published, &addresses, &thread.to_string())))
        .collect();
    let expected = threads
        .iter()
        .map(|thread| thread.instructions)
        .sum::<u64>()
        * rounds;
    let hook = NativeSbHook { threads, entries };
    let report =
        od_machine::run_litmus_parallel_ram_probe(fixture, vcpus, rounds, Some(&hook)).unwrap();
    assert_eq!(report.rounds, rounds);
    assert_eq!(report.late_barrier_rounds, 0);
    assert_eq!(report.interpreted_retired, 0);
    assert_eq!(report.hooked_retired, expected);
    drop(published);
    report
}

#[test]
#[ignore = "hardware sensitivity probe using two/three decoded guest instructions"]
fn s6_41e_native_guest_sb_detects_the_emitted_mfence_mutation() {
    let _guard = JIT_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/guest/stage6/litmus");
    let sb = od_machine::LitmusCase::from_fixture_file(&root.join("sb.json")).unwrap();
    let mfence = od_machine::LitmusCase::from_fixture_file(&root.join("sb-mfence.json")).unwrap();
    let rounds: u64 = std::env::var("OD_S6_SB_RAM_ROUNDS")
        .map_or(200_000, |value| value.parse().expect("round count"));
    // Resetting two dwords and executing two stores can raise the same generation each round.
    // Leave room for those writes and for the fixture setup on a fresh machine.
    assert!(rounds > 0 && rounds.checked_mul(16).is_some());
    let mut sensitive = true;
    for vcpus in [2, 4] {
        for (label, case, mutate) in [
            ("weak SB", &sb, false),
            ("DMB ISH", &mfence, false),
            ("NOP", &mfence, true),
        ] {
            let report = native_guest_sb(case, vcpus, rounds, mutate);
            let counts: Vec<_> = report
                .histogram
                .iter()
                .map(|entry| (&entry.values, entry.rounds))
                .collect();
            eprintln!(
                "{vcpus} vCPU {label}: {counts:?}, native guest retired {}, fence slots {}",
                report.hooked_retired,
                report.hooked_retired - 4 * rounds,
            );
            let zero = report
                .histogram
                .iter()
                .find(|entry| entry.values == [0, 0, 1, 1])
                .map_or(0, |entry| entry.rounds);
            if label == "DMB ISH" {
                assert_eq!(report.verdict, od_machine::LitmusParallelVerdict::Allowed);
                assert_eq!(zero, 0);
            } else {
                sensitive &= zero != 0;
            }
        }
    }
    assert!(
        sensitive,
        "inconclusive: both weak guest controls must show (0,0)"
    );
}

#[test]
fn s6_41_ordering_litmus_on_generated_code() {
    let _guard = JIT_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    const ROUNDS: u64 = 2_000_000;
    use MemoryOrderMode::{Interleaved, Parallel};
    // (name, shape): the ordered shapes and their uncontrolled counterparts.
    let shapes = [
        (
            "acquire-release accesses",
            Shape {
                accesses: Parallel,
                fences: Interleaved,
            },
            true,
        ),
        (
            "plain accesses, parallel fences",
            Shape {
                accesses: Interleaved,
                fences: Parallel,
            },
            true,
        ),
        (
            "plain accesses, no fences (control)",
            Shape {
                accesses: Interleaved,
                fences: Interleaved,
            },
            false,
        ),
    ];
    let mut image = Image::new();
    for (index, (_, shape, _)) in shapes.iter().enumerate() {
        mp_writer(&mut image, &format!("mp_writer{index}"), *shape);
        mp_reader(&mut image, &format!("mp_reader{index}"), *shape);
        sb_thread(&mut image, &format!("sb{index}"), *shape);
    }
    let (bytes, entries) = image.finish();
    let code = PublishedCode::publish(&bytes).unwrap();

    let data = Cell(AtomicU32::new(0));
    let flag = Cell(AtomicU32::new(0));
    for (index, (name, _, forbidden_must_be_zero)) in shapes.iter().enumerate() {
        // MP: the reader waits for the flag and then reads the data; 0 is the forbidden outcome.
        let mut forbidden = 0u64;
        let data_ptr = data.0.as_ptr();
        let flag_ptr = flag.0.as_ptr();
        race(
            ROUNDS,
            pair_fn(entry(&code, &entries, &format!("mp_reader{index}"))),
            (data_ptr, flag_ptr),
            pair_fn(entry(&code, &entries, &format!("mp_writer{index}"))),
            (data_ptr, flag_ptr),
            [&data, &flag],
            |reader, _| {
                if reader == 0 {
                    forbidden += 1;
                }
            },
        );
        eprintln!("s6_41 litmus MP [{name}]: rounds={ROUNDS} forbidden(flag=1,data=0)={forbidden}");
        if *forbidden_must_be_zero {
            assert_eq!(forbidden, 0, "MP forbidden outcome with {name}");
        }

        // SB: forbidden = both loads read 0.
        let mut forbidden = 0u64;
        let mut ones = 0u64;
        race(
            ROUNDS,
            pair_fn(entry(&code, &entries, &format!("sb{index}"))),
            (data_ptr, flag_ptr),
            pair_fn(entry(&code, &entries, &format!("sb{index}"))),
            (flag_ptr, data_ptr),
            [&data, &flag],
            |main, worker| {
                if main == 0 && worker == 0 {
                    forbidden += 1;
                }
                ones += main + worker;
            },
        );
        eprintln!(
            "s6_41 litmus SB [{name}]: rounds={ROUNDS} loads_that_saw_1={ones} forbidden(0,0)={forbidden}"
        );
        if *forbidden_must_be_zero {
            assert_eq!(forbidden, 0, "SB forbidden outcome with {name}");
        }
    }
}

// ---- S6-41b: adapter -> IR -> atomic sequence, against the interpreter, with all flags ----

/// Absolute `[0x1000]` operand through SIB: `prefix opcode modrm(reg) sib disp32 suffix`.
fn abs_form(prefix: &[u8], opcode: &[u8], reg: u8, suffix: &[u8]) -> Vec<u8> {
    let mut code = prefix.to_vec();
    code.extend_from_slice(opcode);
    code.extend_from_slice(&[(reg << 3) | 0x04, 0x25, 0, 0x10, 0, 0]);
    code.extend_from_slice(suffix);
    code
}

const LOCK: &[u8] = &[0xf0];
const LOCK_W: &[u8] = &[0xf0, 0x48];
const REG_RCX: u8 = 1;
const REG_RBX: u8 = 3;

fn s6_41b_forms() -> Vec<(String, Vec<u8>)> {
    let mut forms: Vec<(String, Vec<u8>)> = Vec::new();
    let mut add = |name: &str, code: Vec<u8>| forms.push((name.to_string(), code));
    // The S6-41 forms (flags are compared here for the first time).
    add(
        "lock xadd m32, ebx",
        abs_form(LOCK, &[0x0f, 0xc1], REG_RBX, &[]),
    );
    add(
        "lock xadd m64, rcx",
        abs_form(LOCK_W, &[0x0f, 0xc1], REG_RCX, &[]),
    );
    add("lock add m32, ebx", abs_form(LOCK, &[0x01], REG_RBX, &[]));
    add("lock add m64, rbx", abs_form(LOCK_W, &[0x01], REG_RBX, &[]));
    add("lock inc m32", abs_form(LOCK, &[0xff], 0, &[]));
    add("lock inc m64", abs_form(LOCK_W, &[0xff], 0, &[]));
    add("lock dec m32", abs_form(LOCK, &[0xff], 1, &[]));
    add("lock dec m64", abs_form(LOCK_W, &[0xff], 1, &[]));
    add(
        "lock cmpxchg m32, ebx",
        abs_form(LOCK, &[0x0f, 0xb1], REG_RBX, &[]),
    );
    add(
        "lock cmpxchg m64, rcx",
        abs_form(LOCK_W, &[0x0f, 0xb1], REG_RCX, &[]),
    );
    add("xchg m32, ebx", abs_form(&[], &[0x87], REG_RBX, &[]));
    add("xchg m64, rcx", abs_form(&[0x48], &[0x87], REG_RCX, &[]));
    for imm in [0x01u8, 0x7f, 0x80, 0xf5, 0xff] {
        add(
            &format!("lock add m8, {imm:#x}"),
            abs_form(LOCK, &[0x80], 0, &[imm]),
        );
    }
    add(
        "lock add m32, imm32 0x7fffffff",
        abs_form(LOCK, &[0x81], 0, &0x7fff_ffffu32.to_le_bytes()),
    );
    add(
        "lock add m64, imm8 sx 0xf0",
        abs_form(LOCK_W, &[0x83], 0, &[0xf0]),
    );
    // S6-41b (д): OR, AND, BTS, BTR.
    for imm in [0x0fu8, 0x80, 0xff, 0x00] {
        add(
            &format!("lock or m8, {imm:#x}"),
            abs_form(LOCK, &[0x80], 1, &[imm]),
        );
    }
    add(
        "lock or m32, imm32 0x80000000",
        abs_form(LOCK, &[0x81], 1, &0x8000_0000u32.to_le_bytes()),
    );
    add(
        "lock or m32, imm8 sx 0xf0",
        abs_form(LOCK, &[0x83], 1, &[0xf0]),
    );
    add(
        "lock or m64, imm8 sx 0xff",
        abs_form(LOCK_W, &[0x83], 1, &[0xff]),
    );
    for imm in [0x0fu8, 0xf0, 0x00, 0xff, 0x7f] {
        add(
            &format!("lock and m8, {imm:#x}"),
            abs_form(LOCK, &[0x80], 4, &[imm]),
        );
    }
    for bit in [0u8, 5, 31, 33, 63, 200] {
        add(
            &format!("lock bts m32, {bit}"),
            abs_form(LOCK, &[0x0f, 0xba], 5, &[bit]),
        );
        add(
            &format!("lock btr m32, {bit}"),
            abs_form(LOCK, &[0x0f, 0xba], 6, &[bit]),
        );
        add(
            &format!("lock bts m64, {bit}"),
            abs_form(LOCK_W, &[0x0f, 0xba], 5, &[bit]),
        );
        add(
            &format!("lock btr m64, {bit}"),
            abs_form(LOCK_W, &[0x0f, 0xba], 6, &[bit]),
        );
    }
    // Width 16 is not here: the decoder (S4-38) produces no LOCK ADD/OR/INC/DEC m16 with 0x66
    // (checked: `66 f0 ff /0`, `f0 66 ff /0`, `f0 66 81 /0` are refused), so the adapter's W16
    // acceptance is unreachable from decoded code and is not differentially verified (S6-41c/d).
    // Fences.
    add("mfence", vec![0x0f, 0xae, 0xf0]);
    add("lfence", vec![0x0f, 0xae, 0xe8]);
    add("sfence", vec![0x0f, 0xae, 0xf8]);
    forms
}

struct CodeBytes(Vec<u8>);

impl GuestMemory for CodeBytes {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        self.0
            .get(address as usize)
            .copied()
            .ok_or(MemoryError::Unmapped {
                address,
                access: AccessKind::Fetch,
            })
    }
    fn read(&mut self, address: u64, _: AccessContext, _: &mut [u8]) -> Result<(), MemoryError> {
        Err(MemoryError::Unmapped {
            address,
            access: AccessKind::Read,
        })
    }
    fn write(&mut self, address: u64, _: AccessContext, _: &[u8]) -> Result<(), MemoryError> {
        Err(MemoryError::Unmapped {
            address,
            access: AccessKind::Write,
        })
    }
    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        Ok(())
    }
}

fn adapted(code: &[u8]) -> Program {
    let mut memory = CodeBytes(code.to_vec());
    let decoded = long_decode::decode(&mut memory, 0, AccessContext::default())
        .unwrap_or_else(|error| panic!("{code:02x?} does not decode: {error:?}"));
    assert_eq!(decoded.length as usize, code.len(), "{code:02x?}");
    lower_atomic_instruction(&decoded, CODE)
        .unwrap_or_else(|error| panic!("{code:02x?}: {error:?}"))
}

/// Operand width in bytes (0 for a fence) and the flags kind of an adapted program.
fn program_shape(program: &Program) -> (usize, Option<AtomicFlagsKind>) {
    let mut bytes = 0;
    let mut flags = None;
    for operation in &program.operations {
        match operation {
            Operation::AtomicRmw { operand, .. } => bytes = operand.size.bytes(),
            Operation::AtomicFlags { kind, .. } => flags = Some(*kind),
            _ => {}
        }
    }
    (bytes, flags)
}

type SequenceFn = unsafe extern "C" fn(*mut LongModeState, *mut u8);

fn sequence_fn(address: usize) -> SequenceFn {
    // SAFETY: an entry of a live `PublishedCode` built from `emit_atomic_program` followed by
    // `ret`, signature `fn(state: x0, address: x1)`.
    unsafe { std::mem::transmute::<usize, SequenceFn>(address) }
}

fn emit_sequence(image: &mut Image, name: &str, mode: MemoryOrderMode, program: &Program) {
    image.begin(name);
    let env = SequenceEnv {
        mode,
        state: x(0),
        address: x(1),
    };
    emit_atomic_program(&mut image.encoder, &env, program)
        .unwrap_or_else(|error| panic!("{name}: {error:?}"));
    image.encoder.ret(x(30));
}

const DEFINED_FLAGS: u64 = 0x8d5;

#[test]
fn s6_41b_adapted_atomic_sequences_equal_the_interpreter_with_all_flags_in_both_modes() {
    let _guard = JIT_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let forms = s6_41b_forms();
    let programs: Vec<Program> = forms.iter().map(|(_, code)| adapted(code)).collect();
    let mut image = Image::new();
    for ((name, _), program) in forms.iter().zip(&programs) {
        for mode in MODES {
            emit_sequence(
                &mut image,
                &format!("{name}/{}", mode_name(mode)),
                mode,
                program,
            );
        }
    }
    let (bytes, entries) = image.finish();
    let code = PublishedCode::publish(&bytes).unwrap();

    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let mut cases = 0usize;
    // For the arithmetic kinds: which (flag, value) pairs the interpreter produced.
    let mut seen_add = [[false; 2]; 6];
    let mut seen_sub = [[false; 2]; 6];
    // Logical forms (OR/AND): PF, ZF, SF; bit test (BTS/BTR): CF. CF of INC/DEC is preserved, so
    // it is excluded from the arithmetic teeth (it would take both values from the random input).
    let mut seen_logic = [[false; 2]; 3];
    let mut seen_bit = [false; 2];
    const LOGIC_BITS: [u64; 3] = [1 << 2, 1 << 6, 1 << 7];
    const FLAG_BITS: [u64; 6] = [1, 1 << 2, 1 << 4, 1 << 6, 1 << 7, 1 << 11];
    for ((name, code_bytes), program) in forms.iter().zip(&programs) {
        let (operand_bytes, flags_kind) = program_shape(program);
        for round in 0..400usize {
            let pick = |rng: &mut Rng| {
                if rng.next() % 3 == 0 {
                    EDGES[(rng.next() % EDGES.len() as u64) as usize]
                } else {
                    rng.next()
                }
            };
            // The first 100 rounds sweep the edge cross product, the rest are random.
            let (initial, source) = if round < EDGES.len() * EDGES.len() {
                (EDGES[round / EDGES.len()], EDGES[round % EDGES.len()])
            } else {
                (pick(&mut rng), pick(&mut rng))
            };
            let expected = if round % 2 == 0 {
                initial
            } else {
                pick(&mut rng)
            };
            let flags_in = DEFINED_FLAGS & rng.next();

            let mut ram = Ram {
                bytes: vec![0; 0x4000],
            };
            ram.bytes[CODE as usize..CODE as usize + code_bytes.len()].copy_from_slice(code_bytes);
            for offset in 0..GUARD_BEFORE + 8 + GUARD_AFTER {
                ram.bytes[CELL as usize - GUARD_BEFORE + offset] = 0xa5 ^ (offset as u8);
            }
            ram.bytes[CELL as usize..CELL as usize + operand_bytes]
                .copy_from_slice(&initial.to_le_bytes()[..operand_bytes]);
            let mut state = LongModeState {
                rip: CODE,
                rflags: 2 | (1 << 9) | flags_in,
                cr0: Some(0x8001_0033),
                cr4: Some(0x20),
                activity: Some(CpuActivity::Running),
                sti_shadow: Some(StiShadow::Clear),
                ds_selector: 0x18,
                es_selector: 0x18,
                ..LongModeState::default()
            };
            state.gpr[0] = expected; // RAX
            state.gpr[1] = source ^ 0x0123_4567_89ab_cdef; // RCX
            state.gpr[2] = rng.next(); // RDX (untouched by every form)
            state.gpr[3] = source; // RBX
            let before = state.clone();
            let result = od_isa::execute::run_with_cpl0_profile_set(
                &mut state,
                &mut ram,
                RunConfig {
                    instruction_budget: 1,
                    stop_rip: None,
                    context: ring0(),
                },
                od_isa::INTEGER_FLAGS,
                &Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1),
                None,
                || false,
            )
            .unwrap();
            assert_eq!(result.instructions_executed, 1, "{name}");
            assert!(
                matches!(
                    result.exit,
                    Cpl0ProfileSetExit::Run(RunExit::InstructionBudgetExhausted)
                ),
                "{name}: {:?}",
                result.exit
            );
            let reference =
                &ram.bytes[CELL as usize - GUARD_BEFORE..CELL as usize + 8 + GUARD_AFTER];

            // Only the flags the form writes count towards the teeth (INC/DEC keep CF).
            let keeps_carry = matches!(
                flags_kind,
                Some(AtomicFlagsKind::AddKeepCarry | AtomicFlagsKind::SubtractKeepCarry)
            );
            match flags_kind {
                Some(AtomicFlagsKind::Add | AtomicFlagsKind::AddKeepCarry) => {
                    for (index, bit) in FLAG_BITS.iter().enumerate() {
                        if !(keeps_carry && *bit == 1) {
                            seen_add[index][usize::from(state.rflags & bit != 0)] = true;
                        }
                    }
                }
                Some(AtomicFlagsKind::Subtract | AtomicFlagsKind::SubtractKeepCarry) => {
                    for (index, bit) in FLAG_BITS.iter().enumerate() {
                        if !(keeps_carry && *bit == 1) {
                            seen_sub[index][usize::from(state.rflags & bit != 0)] = true;
                        }
                    }
                }
                Some(AtomicFlagsKind::Or | AtomicFlagsKind::And) => {
                    for (index, bit) in LOGIC_BITS.iter().enumerate() {
                        seen_logic[index][usize::from(state.rflags & bit != 0)] = true;
                    }
                }
                Some(AtomicFlagsKind::BitTest { .. }) => {
                    seen_bit[usize::from(state.rflags & 1 != 0)] = true;
                }
                _ => {}
            }

            for mode in MODES {
                let mut buffer = Buffer([0; 32]);
                for (offset, byte) in buffer
                    .0
                    .iter_mut()
                    .enumerate()
                    .take(GUARD_BEFORE + 8 + GUARD_AFTER)
                {
                    *byte = 0xa5 ^ (offset as u8);
                }
                buffer.0[GUARD_BEFORE..GUARD_BEFORE + operand_bytes]
                    .copy_from_slice(&initial.to_le_bytes()[..operand_bytes]);
                let mut jit = before.clone();
                // SAFETY: the entry is the sequence for this (form, mode) followed by `ret`;
                // `jit` is a live `LongModeState` of the layout the offsets come from; the cell
                // is naturally aligned (offset 8 of a 16-aligned buffer).
                unsafe {
                    sequence_fn(entry(
                        &code,
                        &entries,
                        &format!("{name}/{}", mode_name(mode)),
                    ))(&mut jit, buffer.0.as_mut_ptr().add(GUARD_BEFORE));
                }
                let label = format!(
                    "{name} [{}] round={round} initial={initial:#x} source={source:#x} \
                     expected={expected:#x} flags_in={flags_in:#x}",
                    mode_name(mode)
                );
                assert_eq!(
                    &buffer.0[..GUARD_BEFORE + 8 + GUARD_AFTER],
                    reference,
                    "memory: {label}"
                );
                assert_eq!(jit.gpr, state.gpr, "GPRs: {label}");
                assert_eq!(
                    jit.rflags, state.rflags,
                    "RFLAGS: {label} (jit {:#x} interpreter {:#x})",
                    jit.rflags, state.rflags
                );
                cases += 1;
            }
        }
    }
    assert_eq!(cases, forms.len() * 400 * 2);
    // The differential must have teeth: every flag a form writes took both values: the six
    // arithmetic flags for ADD-like and SUB-like (CMPXCHG, DEC) forms (CF of INC/DEC is kept and
    // not counted), PF/ZF/SF for OR/AND, CF for BTS/BTR; a wrong flag formula cannot hide
    // behind a constant.
    for (index, pairs) in seen_add.iter().enumerate() {
        assert_eq!(
            *pairs,
            [true, true],
            "add-like flag bit {:#x}",
            FLAG_BITS[index]
        );
    }
    for (index, pairs) in seen_sub.iter().enumerate() {
        assert_eq!(
            *pairs,
            [true, true],
            "sub-like flag bit {:#x}",
            FLAG_BITS[index]
        );
    }
    for (index, pairs) in seen_logic.iter().enumerate() {
        assert_eq!(
            *pairs,
            [true, true],
            "logical flag bit {:#x}",
            LOGIC_BITS[index]
        );
    }
    assert_eq!(seen_bit, [true, true], "bit-test CF");
    eprintln!(
        "s6_41b differential: {} forms, {cases} cases (forms x rounds x modes); memory, all GPRs and \
         RFLAGS equal the interpreter",
        forms.len()
    );
}

#[test]
fn s6_41b_or_and_clear_bits_claim_each_bit_exactly_once_across_threads() {
    let _guard = JIT_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut image = Image::new();
    image.rmw(
        "set64",
        MemoryOrderMode::Parallel,
        AtomicRmwKind::Or,
        OperandSize::Qword,
    );
    image.rmw(
        "clear64",
        MemoryOrderMode::Parallel,
        AtomicRmwKind::ClearBits,
        OperandSize::Qword,
    );
    let (bytes, entries) = image.finish();
    let code = PublishedCode::publish(&bytes).unwrap();
    let set = entry(&code, &entries, "set64");
    let clear = entry(&code, &entries, "clear64");

    // A pool of threads that lives for the whole test: every round starts at a `Barrier`, so
    // the threads begin the 64 operations of a round together instead of being spawned one
    // after another (spawning costs tens of microseconds, the 64 operations about one). The
    // overlap is by the barrier and is not guaranteed on every run: on a loaded host a thread
    // can still be late. A round is a full cycle on one cell: every bit is set by LDSET (the
    // cell goes 0 -> all ones, each bit claimed once), a second barrier, then every bit is
    // cleared by LDCLR (all ones -> 0, each bit claimed once).
    const THREADS: usize = 4;
    const ROUNDS: usize = 5000;
    #[repr(align(16))]
    struct Cell64(AtomicU64);
    let cells: Vec<Cell64> = (0..ROUNDS).map(|_| Cell64(AtomicU64::new(0))).collect();
    // winners[phase][round][bit]: how many threads saw the bit in its pre-change state.
    let winners: Vec<Vec<AtomicU64>> = (0..2)
        .map(|_| (0..ROUNDS * 64).map(|_| AtomicU64::new(0)).collect())
        .collect();
    let barrier = Barrier::new(THREADS);
    std::thread::scope(|scope| {
        for thread in 0..THREADS {
            let (cells, winners, barrier) = (&cells, &winners, &barrier);
            scope.spawn(move || {
                let phases = [(set, 0u64), (clear, 1u64)];
                for round in 0..ROUNDS {
                    for (phase, (function, claimed_old_bit)) in phases.iter().enumerate() {
                        let function = rmw_fn(*function);
                        barrier.wait();
                        // Each thread walks the bits from a different start so they collide.
                        for step in 0..64 {
                            let bit = (step + thread * 16 + round) % 64;
                            // SAFETY: the cell is 16-aligned and outlives the scope.
                            let old =
                                unsafe { function(cells[round].0.as_ptr().cast(), 1u64 << bit, 0) };
                            if (old >> bit) & 1 == *claimed_old_bit {
                                winners[phase][round * 64 + bit].fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                }
            });
        }
    });
    for (phase, name) in ["LDSET", "LDCLR"].iter().enumerate() {
        for round in 0..ROUNDS {
            for bit in 0..64 {
                let count = winners[phase][round * 64 + bit].load(Ordering::Relaxed);
                assert_eq!(
                    count, 1,
                    "{name} round {round}: bit {bit} was claimed {count} times"
                );
            }
        }
    }
    for (round, cell) in cells.iter().enumerate() {
        assert_eq!(
            cell.0.load(Ordering::SeqCst),
            0,
            "round {round}: set then clear leaves the cell empty"
        );
    }
}

/// SB with `XCHG`: each thread exchanges 1 into its own variable (adapter -> IR -> sequence,
/// parallel mode), then loads the other with `LDAR`. On x86 an `XCHG` is a full barrier, so
/// (0, 0) is forbidden.
#[test]
fn s6_41b_store_buffering_with_xchg_in_parallel_mode_has_no_forbidden_outcome() {
    let _guard = JIT_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    const ROUNDS: u64 = 1_000_000;
    // `xchg [mem], ebx` with RBX = 1: the instruction bytes use the absolute form, but the
    // sequence takes the host address from x0, so only the operand width matters here.
    let program = adapted(&abs_form(&[], &[0x87], REG_RBX, &[]));
    let mut states = [
        Box::new(LongModeState::default()),
        Box::new(LongModeState::default()),
    ];
    let mut image = Image::new();
    for (index, state) in states.iter_mut().enumerate() {
        image.begin(&format!("sb_xchg{index}"));
        // x17 = this thread's state; RBX = 1 for every round.
        let pointer = (&mut **state as *mut LongModeState) as u64;
        image
            .encoder
            .movz_x(x(17), (pointer & 0xffff) as u16, 0)
            .unwrap();
        image
            .encoder
            .movk_x(x(17), ((pointer >> 16) & 0xffff) as u16, 1)
            .unwrap();
        image
            .encoder
            .movk_x(x(17), ((pointer >> 32) & 0xffff) as u16, 2)
            .unwrap();
        image
            .encoder
            .movk_x(x(17), ((pointer >> 48) & 0xffff) as u16, 3)
            .unwrap();
        image.encoder.movz_x(x(2), 1, 0).unwrap();
        image
            .encoder
            .str_x(
                XRegOrZero::Reg(x(2)),
                AddressBase::Reg(x(17)),
                (LONG_MODE_GPR_OFFSET + 8 * usize::from(REG_RBX)) as u32,
            )
            .unwrap();
        // The address register is x0 (mine); x1 (other) is not clobbered by the sequence.
        let env = SequenceEnv {
            mode: MemoryOrderMode::Parallel,
            state: x(17),
            address: x(0),
        };
        emit_atomic_program(&mut image.encoder, &env, &program).unwrap();
        load(&mut image, MemoryOrderMode::Parallel, 1, 3);
        ret_with(&mut image, 3);
    }
    let (bytes, entries) = image.finish();
    let code = PublishedCode::publish(&bytes).unwrap();

    let data = Cell(AtomicU32::new(0));
    let flag = Cell(AtomicU32::new(0));
    let data_ptr = data.0.as_ptr();
    let flag_ptr = flag.0.as_ptr();
    let mut forbidden = 0u64;
    let mut ones = 0u64;
    race(
        ROUNDS,
        pair_fn(entry(&code, &entries, "sb_xchg0")),
        (data_ptr, flag_ptr),
        pair_fn(entry(&code, &entries, "sb_xchg1")),
        (flag_ptr, data_ptr),
        [&data, &flag],
        |main, worker| {
            if main == 0 && worker == 0 {
                forbidden += 1;
            }
            ones += main + worker;
        },
    );
    eprintln!(
        "s6_41b litmus SB with XCHG [parallel]: rounds={ROUNDS} loads_that_saw_1={ones} forbidden(0,0)={forbidden}"
    );
    assert_eq!(forbidden, 0, "SB forbidden outcome with XCHG");
    drop(states);
}
