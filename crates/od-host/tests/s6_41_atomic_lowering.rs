//! S6-41: native execution of the ARM64 sequences that `od_jit::arm64::memory_order` lowers the
//! three memory-effect classes (ordinary access, atomic read-modify-write, fence) to.
//!
//! 1. Differential: every `LOCK`/`XCHG` memory form the interpreter runs through
//!    `GuestMemory::atomic_rmw_integer` (S6-20) is run once via
//!    `od_isa::execute::run_with_cpl0_profile_set` and once as the generated host sequence, in both modes, on the same initial memory; the final memory
//!    bytes (operand and a guard region around it) and the value the instruction returns to the
//!    guest (`XADD`/`XCHG` source register, `CMPXCHG` ZF and accumulator) must be equal. Flags
//!    other than the `CMPXCHG` ZF are not compared: the lowering does not compute them (the IR
//!    derives them with a separate `Integer` effect), so that part is not covered here.
//! 2. Atomicity: several host threads run the generated parallel-mode sequences on one cell.
//! 3. Ordering: message passing (MP) and store buffering (SB) on generated code, with the parallel
//!    lowering of the accesses and with plain accesses plus the parallel lowering of the fences.
//!    The uncontrolled variants are printed, never asserted: ARM allows their weak outcomes.
//!
//! Generated code is called through `PublishedCode`; the host has one `MAP_JIT` region, so every
//! test takes `JIT_LOCK` and publishes one buffer at a time.
#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use od_host::PublishedCode;
use od_isa::execute::{
    ControlRegisterPolicy, Cpl0ProfileSet, Cpl0ProfileSetExit, RunConfig, RunExit,
};
use od_isa::long_decode::{AddressBase as GuestBase, MemoryOperand};
use od_isa::{
    AccessContext, AccessKind, AccessOrigin, CpuActivity, GuestMemory, InstructionMode,
    LongModeState, MemoryError, OperandSize, PrivilegeLevel, StiShadow,
};
use od_jit::arm64::memory_order::{HostOperands, MemoryOrderMode, lower_memory_effect};
use od_jit::arm64::{AddressBase, Encoder, WReg, XReg, XRegOrZero};
use od_jit::ir::{AtomicRmwKind, FenceKind, GuestGpr, IntWidth, Operation, TempId};

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
                        // value derived from the old memory value; the JIT sequence does
                        // not produce ZF for CMPXCHG, so this checks the old value and
                        // RAX, not a JIT-computed flag (full flag set: S6-41b).
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
fn sb_thread(image: &mut Image, name: &str, shape: Shape) {
    image.begin(name);
    one(image);
    store(image, shape.accesses, 0);
    fence(image, shape.fences, FenceKind::Full);
    load(image, shape.accesses, 1, 3);
    ret_with(image, 3);
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
