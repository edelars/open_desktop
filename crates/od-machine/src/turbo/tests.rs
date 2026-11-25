//! Differential tests of the turbo engine (experiment, branch `exp/turbo-jit`).
//!
//! Every case builds the same machine twice (RAM, four-level tables with 4 KiB and 2 MiB pages,
//! code, data, stack, a long-mode CPL0 state) and runs it
//!
//! - through the general interpreter step only: the set loop without a turbo hook (the reference
//!   semantics), see [`run_reference`], and
//! - through the turbo engine: the set loop with a turbo hook, as the board's turbo quantum runs
//!   it, see [`run_turbo`];
//!
//! then compares everything guest-visible: the whole `LongModeState` (GPRs, RIP, selectors, CR3,
//! ...) with RFLAGS compared under the definedness mask both runs report (the masks must be equal
//! too), the whole RAM byte for byte (page tables with their accessed/dirty bits included), the
//! state of the MMIO device, the RDTSC reads, the interrupt acknowledgements (when the case has a
//! source), the exit of the loop, the retired count and the other report fields.
//!
//! The programs are hand-encoded x86-64 templates (every operation form `jit::plan` admits, plus
//! a few the general path keeps), randomized by a small xorshift generator, and random
//! straight-line sequences of templates in which flag consumers only read flags the producers
//! before them define. A test checks that every template variant declared for generated code is
//! admitted by `jit::plan`, and the template tests that each such variant ran in generated code.
//!
//! Known differences have ignored tests of their own (`#[ignore = "turbo mismatch: ..."]`); their
//! templates are kept out of the other tests (`KNOWN_MISMATCHES`).

#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use super::TurboEngine;
use crate::paged_memory_adapter::{PagedCpl0SetExit, PagedCpl0SetReport};
use crate::{
    AccessWidth, BusDevice, DeviceAccess, DeviceError, GuestPhysicalAddress, GuestPhysicalRange,
    MachineBus, MachineMemoryMap, MemoryRegionKind, MemoryRegionSpec, MmioBinding, RamLimits,
};
use od_isa::execute::{
    ControlRegisterPolicy, Cpl0ProfileSet, MovCr3Long64V1, REP_ITERATION_WORK_LIMIT, RunExit,
};
use od_isa::memory::PrivilegeLevel;
use od_isa::memory::{AccessContext, AccessOrigin, GuestMemory, InstructionMode, MemoryError};
use od_isa::state::{CodeSegmentDescriptorCache, StackSegmentDescriptorCache};
use od_isa::{
    ARITHMETIC_FLAGS, CliProfile, CpuActivity, HaltProfile, INTEGER_FLAGS, LongModeState, Register,
    StiProfile, StiShadow,
};
use std::sync::{Arc, Mutex};

// ---- machine layout ------------------------------------------------------------------------------

const RAM_LEN: u64 = 0x10_0000;
const MMIO_PHYS: u64 = 0x0100_0000;

const PML4_A: u64 = 0x1000;
const PDPT_A: u64 = 0x2000;
const PD_A: u64 = 0x3000;
const PT_A: u64 = 0x4000;
const PML4_B: u64 = 0x6000;
const PDPT_B: u64 = 0x7000;
const PD_B: u64 = 0x8000;
const PT_B: u64 = 0x9000;

/// Code: 8 pages, linear = physical.
const CODE: u64 = 0x10000;
const CODE_PAGES: u64 = 8;
/// Data: 8 pages, linear = physical in space A; in space B the same linear pages map `ALT`.
const DATA: u64 = 0x20000;
const DATA_PAGES: u64 = 8;
/// The data pointer of the programs (RBX): data pages on both sides of it.
const DATA_PTR: u64 = DATA + 0x2000;
/// The GS base: inside the data pages.
const GS_BASE: u64 = DATA + 0x5000;
/// Stack: 4 pages, RSP starts in the middle.
const STACK: u64 = 0x30000;
const STACK_TOP: u64 = STACK + 0x2000;
const IDT: u64 = 0x38000;
const HANDLER: u64 = 0x39000;
const GDT: u64 = 0x3A000;
/// Alternative frames: space B's data, remap targets.
const ALT: u64 = 0x40000;
/// The linear page that maps the MMIO page.
const MMIO_LINEAR: u64 = 0x50000;
/// A linear page whose PTE is not present.
const UNMAPPED: u64 = 0x60000;
/// A read-only linear page (ALT + 0x9000), followed by a writable one (ALT + 0xA000).
const READ_ONLY: u64 = 0x61000;
const AFTER_READ_ONLY: u64 = 0x62000;
/// Linear windows onto the leaf table and the directory of space A.
const PT_WINDOW: u64 = 0x70000;
const PD_WINDOW: u64 = 0x71000;
/// PD_A[1]: a 2 MiB page over physical frame 0 (the RAM is its first MiB); the second pointer of
/// the programs, RBP, is in it, over physical `BIG_DATA`.
const BIG: u64 = 0x20_0000;
const BIG_DATA: u64 = 0x9_0000;
const BIG_PTR: u64 = BIG + BIG_DATA;
/// PML4_A[511]: a high alias of the whole low 512 GiB (the same PDPT).
const HIGH: u64 = 0xFFFF_FF80_0000_0000;

const P: u64 = 1;
const RW: u64 = 2;
const PS: u64 = 1 << 7;
const NX: u64 = 1 << 63;

const CR0: u64 = 0x8001_0033;
const CR4: u64 = 0x20;
const EFER: u64 = (1 << 8) | (1 << 10) | (1 << 11);

const CF: u64 = 1;
const PF: u64 = 1 << 2;
const AF: u64 = 1 << 4;
const ZF: u64 = 1 << 6;
const SF: u64 = 1 << 7;
const OF: u64 = 1 << 11;
const IF: u64 = 1 << 9;
const DF: u64 = 1 << 10;

fn at(address: u64) -> GuestPhysicalAddress {
    GuestPhysicalAddress::new(address)
}

fn context() -> AccessContext {
    AccessContext {
        mode: InstructionMode::Long64,
        privilege: PrivilegeLevel::Ring0,
        origin: AccessOrigin::Explicit,
    }
}

// ---- the MMIO device -----------------------------------------------------------------------------

/// A page of device registers that behave like memory, with a log of every access.
#[derive(Clone, Debug, PartialEq, Eq)]
struct MmioState {
    bytes: Vec<u8>,
    log: Vec<(bool, u64, u64, u64)>,
}

struct MmioDevice(Arc<Mutex<MmioState>>);

fn width_bytes(width: AccessWidth) -> usize {
    match width {
        AccessWidth::Byte => 1,
        AccessWidth::Word => 2,
        AccessWidth::Dword => 4,
        AccessWidth::Qword => 8,
    }
}

impl BusDevice for MmioDevice {
    fn read(&mut self, access: DeviceAccess) -> Result<u64, DeviceError> {
        let mut state = self.0.lock().unwrap();
        let len = width_bytes(access.width);
        let offset = access.offset as usize;
        let mut value = [0u8; 8];
        value[..len].copy_from_slice(&state.bytes[offset..offset + len]);
        let value = u64::from_le_bytes(value);
        state.log.push((false, access.offset, len as u64, value));
        Ok(value)
    }

    fn write(&mut self, access: DeviceAccess, value: u64) -> Result<(), DeviceError> {
        let mut state = self.0.lock().unwrap();
        let len = width_bytes(access.width);
        let offset = access.offset as usize;
        state.bytes[offset..offset + len].copy_from_slice(&value.to_le_bytes()[..len]);
        state.log.push((true, access.offset, len as u64, value));
        Ok(())
    }
}

// ---- the deterministic generator -----------------------------------------------------------------

#[derive(Clone)]
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[self.below(items.len() as u64) as usize]
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }

    /// A register value: often a boundary value, otherwise random bits of a random width.
    fn value(&mut self) -> u64 {
        const SPECIAL: [u64; 14] = [
            0,
            1,
            2,
            u64::MAX,
            u64::MAX - 1,
            0x7F,
            0x80,
            0xFF,
            0x7FFF,
            0x8000,
            0x7FFF_FFFF,
            0x8000_0000,
            0x7FFF_FFFF_FFFF_FFFF,
            0x8000_0000_0000_0000,
        ];
        match self.below(4) {
            0 => self.pick(&SPECIAL),
            1 => self.next() & 0xFFFF_FFFF,
            2 => self.next() & 0xFF,
            _ => self.next(),
        }
    }
}

// ---- the machine ---------------------------------------------------------------------------------

struct Machine {
    bus: MachineBus,
    state: LongModeState,
    cr3: u64,
    efer: u64,
    defined: u64,
    mmio: Arc<Mutex<MmioState>>,
    rdtsc: u64,
    /// The interrupt source of the board's hook (a source without a vector unless the case
    /// arms one; `None`: no hook).
    irq: Option<TestIrq>,
    /// The device-access count of the bus (the board hands it to its interrupt hook).
    device_accesses: Arc<std::sync::atomic::AtomicU64>,
}

/// The interrupt source of a case and the counters of its hook.
#[derive(Debug, PartialEq, Eq)]
struct TestIrq {
    source: TestSource,
    counters: crate::irq_source::IrqCounters,
}

/// One vector pending until it is acknowledged (the loop asks at every boundary it polls; both
/// runs see the same answers).
#[derive(Debug, PartialEq, Eq)]
struct TestSource {
    pending: Option<u8>,
    acknowledged: Vec<u8>,
}

impl crate::irq_source::BoardIrqSource for TestSource {
    fn pending_vector(&self) -> Option<u8> {
        self.pending
    }

    fn acknowledge(&mut self, vector: u8) {
        self.acknowledged.push(vector);
        self.pending = None;
    }

    fn next_deadline_ns(&self) -> Option<u64> {
        None
    }
}

fn put(bus: &MachineBus, address: u64, bytes: &[u8]) {
    bus.physical_memory()
        .store_bytes(at(address), bytes, |_| {})
        .unwrap();
}

fn put_qword(bus: &MachineBus, address: u64, value: u64) {
    put(bus, address, &value.to_le_bytes());
}

fn get_qword(bus: &MachineBus, address: u64) -> u64 {
    let mut bytes = [0u8; 8];
    bus.physical_memory()
        .load_bytes(at(address), &mut bytes)
        .unwrap();
    u64::from_le_bytes(bytes)
}

/// The leaf entries of space A and B: the same pages except data, which B maps to `ALT`.
fn leaf_entries(space_b: bool) -> Vec<(u64, u64)> {
    let mut entries = Vec::new();
    for page in 0..CODE_PAGES {
        entries.push((CODE + page * 0x1000, CODE + page * 0x1000 | P | RW));
    }
    for page in 0..DATA_PAGES {
        let frame = if space_b { ALT } else { DATA } + page * 0x1000;
        entries.push((DATA + page * 0x1000, frame | P | RW | NX));
    }
    for page in 0..4 {
        entries.push((STACK + page * 0x1000, STACK + page * 0x1000 | P | RW | NX));
    }
    for page in [IDT, HANDLER, GDT] {
        entries.push((page, page | P | RW));
    }
    entries.push((MMIO_LINEAR, MMIO_PHYS | P | RW | NX));
    entries.push((READ_ONLY, (ALT + 0x9000) | P | NX));
    entries.push((AFTER_READ_ONLY, (ALT + 0xA000) | P | RW | NX));
    entries.push((PT_WINDOW, PT_A | P | RW | NX));
    entries.push((PD_WINDOW, PD_A | P | RW | NX));
    // Linear ALT pages map ALT itself (a remap target the program can also read directly).
    for page in 0..8 {
        entries.push((ALT + page * 0x1000, ALT + page * 0x1000 | P | RW | NX));
    }
    entries
}

impl Machine {
    /// The base machine: tables of both spaces, GDT, data filled from `seed`, the state at `CODE`.
    fn new(seed: u64) -> Self {
        let mmio = Arc::new(Mutex::new(MmioState {
            bytes: (0..0x1000u64)
                .map(|i| (i.wrapping_mul(0x3B) ^ (i >> 3)) as u8)
                .collect(),
            log: Vec::new(),
        }));
        let regions = vec![
            MemoryRegionSpec::new(
                GuestPhysicalRange::new(at(0), RAM_LEN).unwrap(),
                MemoryRegionKind::Ram,
            ),
            MemoryRegionSpec::new(
                GuestPhysicalRange::new(at(MMIO_PHYS), 0x1000).unwrap(),
                MemoryRegionKind::Mmio,
            ),
        ];
        let bus = MachineBus::new(
            MachineMemoryMap::new(36, regions).unwrap(),
            RamLimits::new(RAM_LEN),
            vec![Box::new(MmioDevice(Arc::clone(&mmio)))],
            vec![MmioBinding {
                region_start: at(MMIO_PHYS),
                device_index: 0,
            }],
            vec![],
        )
        .unwrap();
        // Tables: every entry starts with A = D = 0.
        put_qword(&bus, PML4_A, PDPT_A | P | RW);
        put_qword(&bus, PML4_A + 511 * 8, PDPT_A | P | RW);
        put_qword(&bus, PDPT_A, PD_A | P | RW);
        put_qword(&bus, PD_A, PT_A | P | RW);
        put_qword(&bus, PD_A + 8, P | RW | PS);
        put_qword(&bus, PML4_B, PDPT_B | P | RW);
        put_qword(&bus, PML4_B + 511 * 8, PDPT_B | P | RW);
        put_qword(&bus, PDPT_B, PD_B | P | RW);
        put_qword(&bus, PD_B, PT_B | P | RW);
        put_qword(&bus, PD_B + 8, P | RW | PS);
        for (linear, entry) in leaf_entries(false) {
            put_qword(&bus, PT_A + (linear >> 12) * 8, entry);
        }
        for (linear, entry) in leaf_entries(true) {
            put_qword(&bus, PT_B + (linear >> 12) * 8, entry);
        }
        // GDT: null, 64-bit ring 0 code (0x08), ring 0 data (0x10).
        put_qword(&bus, GDT + 8, 0x00AF_9B00_0000_FFFF);
        put_qword(&bus, GDT + 16, 0x00CF_9300_0000_FFFF);
        // Data: deterministic bytes in the data, ALT and big-page areas (written through the host
        // pages: byte stores of this size are slow in a debug build).
        let mut rng = Rng::new(seed ^ 0xDA7A);
        for (start, len) in [
            (DATA, DATA_PAGES * 0x1000),
            (ALT, 0x8000),
            (BIG_DATA - 0x1_0000, 0x2_0000),
        ] {
            for page in (start..start + len).step_by(0x1000) {
                let host = bus
                    .physical_memory()
                    .ram_page_host_ptr(at(page))
                    .expect("a RAM page");
                // SAFETY: the host address of a 4 KiB page of plain RAM; nothing else uses the
                // memory yet.
                let bytes = unsafe { std::slice::from_raw_parts_mut(host.cast::<u64>(), 0x200) };
                for word in bytes {
                    *word = rng.next();
                }
            }
        }
        let mut state = LongModeState {
            rip: CODE,
            rflags: 2,
            cr0: Some(CR0),
            cr4: Some(CR4),
            cr3: Some(PML4_A),
            cs_selector: Some(0x08),
            cs_cache: Some(CodeSegmentDescriptorCache {
                base: 0,
                limit: u32::MAX,
                access: 0x9B,
                flags: 2,
            }),
            ss_selector: Some(0x10),
            ss_cache: Some(StackSegmentDescriptorCache {
                base: 0,
                limit: u32::MAX,
                access: 0x93,
                flags: 0,
            }),
            ds_selector: 0x10,
            es_selector: 0x10,
            gs_base: GS_BASE,
            gdtr: Some(od_isa::state::Gdtr {
                base: GDT,
                limit: 0x17,
            }),
            sti_shadow: Some(StiShadow::Clear),
            activity: Some(CpuActivity::Running),
            ..LongModeState::default()
        };
        state.gpr[Register::Rsp as usize] = STACK_TOP;
        state.gpr[Register::Rbx as usize] = DATA_PTR;
        state.gpr[Register::Rbp as usize] = BIG_PTR;
        let device_accesses = bus.device_access_counter();
        Self {
            bus,
            state,
            cr3: PML4_A,
            efer: EFER,
            defined: INTEGER_FLAGS,
            mmio,
            rdtsc: 0x1234_5678_9000,
            irq: Some(TestIrq {
                source: TestSource {
                    pending: None,
                    acknowledged: Vec::new(),
                },
                counters: Default::default(),
            }),
            device_accesses,
        }
    }

    fn code(&mut self, address: u64, bytes: &[u8]) {
        put(&self.bus, address, bytes);
    }

    /// An IDT whose gates 0..64 lead to `HANDLER`, which is a `HLT`.
    fn idt(&mut self) {
        for vector in 0..64u64 {
            let mut gate = [0u8; 16];
            gate[0..2].copy_from_slice(&(HANDLER as u16).to_le_bytes());
            gate[2..4].copy_from_slice(&0x08u16.to_le_bytes());
            gate[5] = 0x8E;
            gate[6..8].copy_from_slice(&((HANDLER >> 16) as u16).to_le_bytes());
            put(&self.bus, IDT + vector * 16, &gate);
        }
        put(&self.bus, HANDLER, &[0xF4]);
        self.state.idtr = Some(od_isa::state::Idtr {
            base: IDT,
            limit: 64 * 16 - 1,
        });
    }

    /// The whole RAM (read through the host pages: `load_bytes` of all of it is slow in a debug
    /// build).
    fn ram(&self) -> Vec<u8> {
        let memory = self.bus.physical_memory();
        let mut image = Vec::with_capacity(RAM_LEN as usize);
        for page in 0..RAM_LEN >> 12 {
            let host = memory
                .ram_page_host_ptr(at(page << 12))
                .expect("a RAM page");
            // SAFETY: the host address of a 4 KiB page of plain RAM, alive with `memory`; nothing
            // writes the RAM while the test reads it (the loop has returned).
            image.extend_from_slice(unsafe { std::slice::from_raw_parts(host, 0x1000) });
        }
        image
    }
}

/// The CPL0 set of the tests: the members the board's unified loop has for these programs.
fn with_set<R>(f: impl FnOnce(&Cpl0ProfileSet<'_>) -> R) -> R {
    let (cli, sti, halt) = (
        CliProfile::NonPviLong64V1,
        StiProfile::NonPviLong64V1,
        HaltProfile::Long64V1,
    );
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
        .with_cli(&cli)
        .with_sti(&sti)
        .with_halt(&halt)
        .with_cpu_owned_cr3(MovCr3Long64V1)
        .with_rdtsc()
        .with_popf_cpl0();
    f(&set)
}

// ---- the two loop entries (keep every call of them here) ---------------------------------------

/// The interrupt hook of a machine with a source, in the board's form (the device-access count of
/// the bus, nothing polled yet).
fn irq_hook<'a>(
    irq: &'a mut Option<TestIrq>,
    device_accesses: &'a std::sync::atomic::AtomicU64,
) -> Option<crate::irq_source::IrqBoundaryHook<'a>> {
    irq.as_mut().map(|irq| crate::irq_source::IrqBoundaryHook {
        source: &mut irq.source,
        counters: &mut irq.counters,
        delivered_mirror: None,
        skip_first_boundary: false,
        device_accesses: Some(device_accesses),
        polled: None,
    })
}

/// One call of the set loop without a turbo hook: the general step only (the reference).
fn run_reference(machine: &mut Machine, budget: u64) -> PagedCpl0SetReport {
    let mut tsc = machine.rdtsc;
    let report = with_set(|set| {
        let mut source = || {
            tsc += 1000;
            tsc
        };
        crate::paged_memory_adapter::run_cpl0_set_paged_2m_ram_with_cpuid_boundary(
            &mut machine.state,
            &mut machine.bus,
            machine.cr3,
            machine.efer,
            context(),
            budget,
            machine.defined,
            set,
            Some(&mut source),
            || false,
            |_| {},
            false,
            false,
            irq_hook(&mut machine.irq, &machine.device_accesses),
        )
    });
    machine.rdtsc = tsc;
    report
}

/// One call of the set loop with a turbo hook (the board's turbo quantum without the fast
/// transition between RDTSC quanta).
fn run_turbo(machine: &mut Machine, engine: &mut TurboEngine, budget: u64) -> PagedCpl0SetReport {
    let mut tsc = machine.rdtsc;
    let report = with_set(|set| {
        let mut source = || {
            tsc += 1000;
            tsc
        };
        crate::paged_memory_adapter::run_cpl0_set_paged_turbo(
            &mut machine.state,
            &mut machine.bus,
            machine.cr3,
            machine.efer,
            context(),
            budget,
            machine.defined,
            set,
            Some(&mut source),
            || false,
            |_| {},
            irq_hook(&mut machine.irq, &machine.device_accesses),
            super::TurboHook {
                engine,
                tick: None,
                fast_quantum: None,
            },
        )
    });
    machine.rdtsc = tsc;
    report
}

// ---- engines -------------------------------------------------------------------------------------

/// Engines are reused (one code arena each, never unmapped): a test takes one, resets it for its
/// memory and gives it back.
static POOL: Mutex<Vec<Box<TurboEngine>>> = Mutex::new(Vec::new());

struct Engine(Option<Box<TurboEngine>>);

impl Engine {
    fn take(machine: &Machine) -> Self {
        let engine = POOL.lock().unwrap().pop();
        let mut engine = engine.unwrap_or_else(|| TurboEngine::new(super::test_arena().expect("test arena")).expect("turbo engine"));
        engine.reset(machine.bus.physical_memory());
        engine.stats = super::TurboStats::default();
        Self(Some(engine))
    }

    /// A new engine (it goes to the pool afterwards).
    fn fresh() -> Self {
        Self(Some(TurboEngine::new(super::test_arena().expect("test arena")).expect("turbo engine")))
    }

    fn get(&mut self) -> &mut TurboEngine {
        self.0.as_deref_mut().unwrap()
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        if let Some(engine) = self.0.take() {
            if let Ok(mut pool) = POOL.lock() {
                pool.push(engine);
            }
        }
    }
}

// ---- comparison ----------------------------------------------------------------------------------

/// The report fields both runs must agree on (the walk counters and cache statistics are
/// measurements of the path, not guest-visible).
#[derive(Debug, PartialEq, Eq)]
struct ReportView {
    exit: String,
    instructions_executed: u64,
    rflags_defined: u64,
    page_fault: String,
    memory_error: String,
    translation_context_changes: String,
    cr3: u64,
    efer: u64,
    delivered: (u64, u64, u64),
    pit_written: bool,
    port_accessed: bool,
    mmio_written: bool,
    last_instruction_rip: Option<u64>,
}

fn view(report: &PagedCpl0SetReport) -> ReportView {
    ReportView {
        exit: format!("{:?}", report.exit),
        instructions_executed: report.instructions_executed,
        rflags_defined: report.rflags_defined,
        page_fault: format!("{:?}", report.page_fault),
        memory_error: format!("{:?}", report.memory_error),
        translation_context_changes: format!("{:?}", report.translation_context_changes),
        cr3: report.cr3,
        efer: report.efer,
        delivered: (
            report.delivered_exceptions,
            report.delivered_page_faults,
            report.delivered_general_protection_faults,
        ),
        pit_written: report.pit_written,
        port_accessed: report.port_accessed,
        mmio_written: report.mmio_written,
        last_instruction_rip: report.last_instruction_rip,
    }
}

/// After a call: the next call starts from the CR3 and EFER this one left and, as the board's
/// quanta do, with every integer flag counted as defined.
fn carry(machine: &mut Machine, report: &PagedCpl0SetReport) {
    machine.cr3 = report.cr3;
    machine.efer = report.efer;
    machine.defined = INTEGER_FLAGS;
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The state with the arithmetic flags outside `defined` cleared (undefined values are no
/// promise of either side).
fn masked_state(state: &LongModeState, defined: u64) -> LongModeState {
    let mut state = state.clone();
    state.rflags &= !ARITHMETIC_FLAGS | defined;
    state
}

/// The first difference of the two machines after a call, or `None`.
fn difference(
    reference: &Machine,
    turbo: &Machine,
    left: &ReportView,
    right: &ReportView,
) -> Option<String> {
    if left != right {
        return Some(format!(
            "report:\n  reference {left:x?} (rip {:#x})\n  turbo     {right:x?} (rip {:#x})",
            reference.state.rip, turbo.state.rip
        ));
    }
    let defined = left.rflags_defined & ARITHMETIC_FLAGS;
    let (a, b) = (
        masked_state(&reference.state, defined),
        masked_state(&turbo.state, defined),
    );
    if a != b {
        let mut lines = Vec::new();
        for index in 0..16 {
            if a.gpr[index] != b.gpr[index] {
                lines.push(format!(
                    "  gpr[{index}]: reference {:#x} turbo {:#x}",
                    a.gpr[index], b.gpr[index]
                ));
            }
        }
        if a.rip != b.rip {
            lines.push(format!("  rip: reference {:#x} turbo {:#x}", a.rip, b.rip));
        }
        if a.rflags != b.rflags {
            lines.push(format!(
                "  rflags (defined {defined:#x}): reference {:#x} turbo {:#x}",
                a.rflags, b.rflags
            ));
        }
        if lines.is_empty() {
            lines.push(format!("  reference {a:x?}\n  turbo     {b:x?}"));
        }
        return Some(format!("state:\n{}", lines.join("\n")));
    }
    let (ram_a, ram_b) = (reference.ram(), turbo.ram());
    if ram_a != ram_b {
        let diffs: Vec<String> = ram_a
            .iter()
            .zip(&ram_b)
            .enumerate()
            .filter(|(_, (x, y))| x != y)
            .take(16)
            .map(|(i, (x, y))| format!("  [{i:#x}] reference {x:02x} turbo {y:02x}"))
            .collect();
        return Some(format!("ram:\n{}", diffs.join("\n")));
    }
    let (mmio_a, mmio_b) = (
        reference.mmio.lock().unwrap().clone(),
        turbo.mmio.lock().unwrap().clone(),
    );
    if mmio_a != mmio_b {
        return Some(format!(
            "mmio log:\n  reference {:x?}\n  turbo     {:x?}",
            mmio_a.log, mmio_b.log
        ));
    }
    // The source's acknowledgements and the delivered count (the other hook counters are
    // accounting of the path, not guest-visible).
    let irq = |machine: &Machine| {
        machine
            .irq
            .as_ref()
            .map(|irq| (irq.source.acknowledged.clone(), irq.counters.irqs_delivered))
    };
    if irq(reference) != irq(turbo) {
        return Some(format!(
            "interrupts (acknowledged, delivered): reference {:?} turbo {:?}",
            irq(reference),
            irq(turbo)
        ));
    }
    if reference.rdtsc != turbo.rdtsc {
        return Some(format!(
            "rdtsc reads: reference {:#x} turbo {:#x}",
            reference.rdtsc, turbo.rdtsc
        ));
    }
    None
}

/// What a differential case gives back for extra checks.
struct Outcome {
    /// The report of the last call.
    report: ReportView,
    /// The instructions retired over all calls.
    retired: u64,
    reference_rip: u64,
    state: LongModeState,
    stats: super::TurboStats,
    /// The vectors the interrupt source saw acknowledged.
    acknowledged: Vec<u8>,
}

/// A case: two machines prepared alike, run call after call (a call that ends at the REP
/// iteration limit is followed by another, as the board does) until `calls` calls or an exit
/// other than that limit; compared after every call.
struct Case<'a> {
    name: String,
    seed: u64,
    prepare: &'a dyn Fn(&mut Machine),
    /// The program bytes (for the message of a difference).
    program: Vec<u8>,
    budget: u64,
    calls: usize,
    /// Between two calls (the same on both machines).
    between: Option<&'a dyn Fn(&mut Machine, usize)>,
    /// A new engine instead of a reset one from the pool.
    fresh: bool,
}

impl<'a> Case<'a> {
    fn new(name: impl Into<String>, seed: u64, prepare: &'a dyn Fn(&mut Machine)) -> Self {
        Self {
            name: name.into(),
            seed,
            prepare,
            program: Vec::new(),
            budget: 100_000,
            calls: 8,
            between: None,
            fresh: false,
        }
    }

    fn fresh(mut self) -> Self {
        self.fresh = true;
        self
    }

    fn program(mut self, program: &[u8]) -> Self {
        self.program = program.to_vec();
        self
    }

    fn budget(mut self, budget: u64) -> Self {
        self.budget = budget;
        self
    }

    fn calls(mut self, calls: usize) -> Self {
        self.calls = calls;
        self
    }

    fn between(mut self, between: &'a dyn Fn(&mut Machine, usize)) -> Self {
        self.between = Some(between);
        self
    }

    /// Run the case; `Err` describes the first difference (and, for a case without work between
    /// calls, the first instruction after which the machines differ).
    fn run(&self) -> Result<Outcome, String> {
        self.run_once().map_err(|message| {
            if self.between.is_none() {
                format!("{message}\n  {}", self.locate())
            } else {
                message
            }
        })
    }

    /// The smallest budget with a difference, and the instruction it ended at.
    fn locate(&self) -> String {
        // The program as prepared (before anything ran): its bytes at the instruction found.
        let mut initial = Machine::new(self.seed);
        (self.prepare)(&mut initial);
        let mut before = initial.state.rip;
        for budget in 1..=self.budget.min(5_000) {
            let case = Case {
                name: self.name.clone(),
                seed: self.seed,
                prepare: self.prepare,
                program: Vec::new(),
                budget,
                calls: self.calls,
                between: None,
                fresh: self.fresh,
            };
            match case.run_once() {
                Ok(outcome) => {
                    if outcome.report.exit != "Run(InstructionBudgetExhausted)" {
                        return format!("no difference with any budget up to {budget}");
                    }
                    before = outcome.reference_rip;
                }
                Err(message) => {
                    let physical = if before >= HIGH {
                        before - HIGH
                    } else {
                        before
                    };
                    let mut bytes = [0u8; 15];
                    initial
                        .bus
                        .physical_memory()
                        .load_bytes(at(physical), &mut bytes)
                        .ok();
                    let decoded = od_isa::long_decode::decode(
                        &mut Bytes {
                            at: before,
                            bytes: &bytes,
                        },
                        before,
                        context(),
                    )
                    .map(|instruction| {
                        format!(
                            "{} = {:?}",
                            hex(&bytes[..instruction.length as usize]),
                            instruction.operation
                        )
                    })
                    .unwrap_or_else(|error| format!("{} ({error:?})", hex(&bytes)));
                    let detail = message.split("\n  program").next().unwrap_or("").to_owned();
                    return format!(
                        "first difference with budget {budget}: the instruction at {before:#x}: {decoded}\n  {detail}"
                    );
                }
            }
        }
        "no smaller budget found".to_owned()
    }

    fn run_once(&self) -> Result<Outcome, String> {
        let mut reference = Machine::new(self.seed);
        let mut turbo = Machine::new(self.seed);
        (self.prepare)(&mut reference);
        (self.prepare)(&mut turbo);
        let initial = reference.state.clone();
        let mut engine = if self.fresh {
            Engine::fresh()
        } else {
            Engine::take(&turbo)
        };
        let mut last = None;
        let mut retired = 0;
        for call in 0..self.calls {
            if call > 0
                && let Some(between) = self.between
            {
                between(&mut reference, call);
                between(&mut turbo, call);
            }
            let left = run_reference(&mut reference, self.budget);
            let right = run_turbo(&mut turbo, engine.get(), self.budget);
            let (left_view, right_view) = (view(&left), view(&right));
            if let Some(difference) = difference(&reference, &turbo, &left_view, &right_view) {
                return Err(format!(
                    "{} (seed {:#x}), call {call}: {difference}\n  program {}\n  initial state {initial:x?}",
                    self.name,
                    self.seed,
                    hex(&self.program)
                ));
            }
            carry(&mut reference, &left);
            carry(&mut turbo, &right);
            let partial = left.exit == PagedCpl0SetExit::Run(RunExit::RepIterationBudgetExhausted);
            retired += left.instructions_executed;
            last = Some(left_view);
            if !partial && self.between.is_none() {
                break;
            }
        }
        Ok(Outcome {
            report: last.unwrap(),
            retired,
            reference_rip: reference.state.rip,
            state: turbo.state.clone(),
            stats: engine.get().stats,
            acknowledged: turbo
                .irq
                .as_ref()
                .map(|irq| irq.source.acknowledged.clone())
                .unwrap_or_default(),
        })
    }

    #[track_caller]
    fn check(&self) -> Outcome {
        match self.run() {
            Ok(outcome) => outcome,
            Err(message) => panic!("turbo mismatch: {message}"),
        }
    }
}

// ---- the encoder ---------------------------------------------------------------------------------

const RAX: u8 = 0;
const RCX: u8 = 1;
const RDX: u8 = 2;
const RBX: u8 = 3;
const RSP: u8 = 4;
const RBP: u8 = 5;
const RSI: u8 = 6;
const RDI: u8 = 7;
const R8: u8 = 8;
const R10: u8 = 10;
const R11: u8 = 11;
const R12: u8 = 12;
const R13: u8 = 13;
const R14: u8 = 14;
const R15: u8 = 15;

/// Registers the programs may change freely (RBX and RBP are the data pointers, RSP the stack).
const FREE: [u8; 13] = [RAX, RCX, RDX, RSI, RDI, 8, 9, R10, R11, R12, R13, R14, R15];

/// A ModRM operand.
#[derive(Clone, Copy, Debug)]
enum Rm {
    R(u8),
    /// `[base + disp]`.
    M(u8, i32),
    /// `[base + index * (1 << scale) + disp]`.
    Sib(u8, u8, u8, i32),
    /// `[disp32]` (SIB without base and index).
    Abs(i32),
    /// `[rip + disp32]` that reaches this linear address.
    Rip(u64),
}

struct Asm {
    at: u64,
    bytes: Vec<u8>,
}

fn disp_mode(disp: i32, base: u8) -> (u8, Vec<u8>) {
    if disp == 0 && base & 7 != 5 {
        (0, Vec::new())
    } else if (-128..128).contains(&disp) {
        (1, vec![disp as i8 as u8])
    } else {
        (2, disp.to_le_bytes().to_vec())
    }
}

/// The immediate of an operation of `size` bytes (imm32 for 64-bit operations).
fn imm(size: u8, value: u64) -> Vec<u8> {
    match size {
        1 => vec![value as u8],
        2 => (value as u16).to_le_bytes().to_vec(),
        _ => (value as u32).to_le_bytes().to_vec(),
    }
}

/// The byte (`size` 1) or wide form of an opcode whose low bit selects the width.
fn wide(opcode: u8, size: u8) -> u8 {
    if size == 1 { opcode } else { opcode + 1 }
}

impl Asm {
    fn new(at: u64) -> Self {
        Self {
            at,
            bytes: Vec::new(),
        }
    }

    fn pc(&self) -> u64 {
        self.at + self.bytes.len() as u64
    }

    fn raw(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }

    /// One instruction: legacy prefixes, REX (W from `w`, forced for byte registers), opcode,
    /// ModRM (`reg`, `rm`), immediate.
    #[allow(clippy::too_many_arguments)]
    fn enc(
        &mut self,
        prefixes: &[u8],
        w: bool,
        force_rex: bool,
        opcode: &[u8],
        reg: u8,
        rm: Rm,
        imm: &[u8],
    ) {
        let (b, x) = match rm {
            Rm::R(r) | Rm::M(r, _) => (r >> 3, 0),
            Rm::Sib(base, index, _, _) => (base >> 3, index >> 3),
            Rm::Abs(_) | Rm::Rip(_) => (0, 0),
        };
        let rex = 0x40 | (u8::from(w) << 3) | ((reg >> 3) & 1) << 2 | (x & 1) << 1 | (b & 1);
        let mut out = prefixes.to_vec();
        if rex != 0x40 || force_rex {
            out.push(rex);
        }
        out.extend_from_slice(opcode);
        let reg3 = (reg & 7) << 3;
        match rm {
            Rm::R(r) => out.push(0xC0 | reg3 | (r & 7)),
            Rm::M(base, disp) => {
                let (mode, bytes) = disp_mode(disp, base);
                out.push(mode << 6 | reg3 | if base & 7 == 4 { 4 } else { base & 7 });
                if base & 7 == 4 {
                    out.push(0x24);
                }
                out.extend(bytes);
            }
            Rm::Sib(base, index, scale, disp) => {
                assert_ne!(index, RSP, "RSP is no index");
                let (mode, bytes) = disp_mode(disp, base);
                out.push(mode << 6 | reg3 | 4);
                out.push(scale << 6 | (index & 7) << 3 | (base & 7));
                out.extend(bytes);
            }
            Rm::Abs(disp) => {
                out.push(reg3 | 4);
                out.push(0x25);
                out.extend(disp.to_le_bytes());
            }
            Rm::Rip(target) => {
                out.push(reg3 | 5);
                let end = self.pc() + out.len() as u64 + 4 + imm.len() as u64;
                out.extend((target.wrapping_sub(end) as i32).to_le_bytes());
            }
        }
        out.extend_from_slice(imm);
        self.bytes.extend(out);
    }

    /// An operation of `size` bytes: `extra` prefixes, then 66 for a word; REX.W for a qword.
    fn op(&mut self, extra: &[u8], size: u8, opcode: &[u8], reg: u8, rm: Rm, imm: &[u8]) {
        let mut prefixes = extra.to_vec();
        if size == 2 {
            prefixes.push(0x66);
        }
        self.enc(&prefixes, size == 8, size == 1, opcode, reg, rm, imm);
    }

    /// `MOV r64, imm64`.
    fn mov_imm(&mut self, reg: u8, value: u64) {
        self.raw(&[0x48 | (reg >> 3), 0xB8 + (reg & 7)]);
        self.raw(&value.to_le_bytes());
    }

    /// `MOV r32, imm32` (zero-extending).
    fn mov_imm32(&mut self, reg: u8, value: u32) {
        if reg >= 8 {
            self.raw(&[0x41]);
        }
        self.raw(&[0xB8 + (reg & 7)]);
        self.raw(&value.to_le_bytes());
    }

    fn hlt(&mut self) {
        self.raw(&[0xF4]);
    }

    /// `LEA reg, [reg + 1]` (changes no flag): the filler of the branch templates.
    fn bump(&mut self, reg: u8) {
        self.enc(&[], true, false, &[0x8D], reg, Rm::M(reg, 1), &[]);
    }
}

/// The bytes of `LEA reg, [reg + 1]` (4 or 5 bytes).
fn filler(reg: u8) -> Vec<u8> {
    let mut asm = Asm::new(0);
    asm.bump(reg);
    asm.bytes
}

fn free(rng: &mut Rng) -> u8 {
    rng.pick(&FREE)
}

/// Two different free registers.
fn two(rng: &mut Rng) -> (u8, u8) {
    let a = free(rng);
    loop {
        let b = free(rng);
        if b != a {
            return (a, b);
        }
    }
}

/// A data operand of `size` bytes in mapped RAM: around RBX (4 KiB pages) or RBP (the 2 MiB
/// page), sometimes misaligned, sometimes crossing a page.
fn mem(rng: &mut Rng, size: u8) -> Rm {
    let size = i32::from(size);
    let disp = match rng.below(6) {
        0 => (rng.below(0x200) as i32) * size,
        1 => -(rng.below(0x100) as i32 + 1) * size,
        2 => 0x1000 - size + 1 + rng.below(size.max(2) as u64 - 1) as i32,
        3 => rng.below(0x5000) as i32 - 0x1000,
        _ => (rng.below(0x80) as i32) * 8,
    };
    match rng.below(4) {
        0 => Rm::M(RBP, disp),
        _ => Rm::M(RBX, disp),
    }
}

// ---- instruction decoding of the templates -------------------------------------------------------

struct Bytes<'a> {
    at: u64,
    bytes: &'a [u8],
}

impl GuestMemory for Bytes<'_> {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        address
            .checked_sub(self.at)
            .and_then(|offset| self.bytes.get(offset as usize))
            .copied()
            .ok_or_else(|| MemoryError::Backend("past the template".into()))
    }
    fn read(&mut self, _: u64, _: AccessContext, _: &mut [u8]) -> Result<(), MemoryError> {
        Err(MemoryError::Backend("no data".into()))
    }
    fn write(&mut self, _: u64, _: AccessContext, _: &[u8]) -> Result<(), MemoryError> {
        Err(MemoryError::Backend("no data".into()))
    }
    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        Ok(())
    }
}

/// The instructions of straight-line bytes at linear `at`.
fn decode_all(at: u64, bytes: &[u8]) -> Result<Vec<od_isa::long_decode::Instruction>, String> {
    let mut memory = Bytes { at, bytes };
    let mut rip = at;
    let mut out = Vec::new();
    while rip < at + bytes.len() as u64 {
        let instruction = od_isa::long_decode::decode(&mut memory, rip, context())
            .map_err(|error| format!("decode at +{:#x}: {error:?}", rip - at))?;
        rip += u64::from(instruction.length);
        out.push(instruction);
    }
    Ok(out)
}

/// The definedness of the arithmetic flags after the instructions (a register-count shift
/// counts as leaving every flag undefined: the generator does not track CL).
fn defined_after(instructions: &[od_isa::long_decode::Instruction], mut defined: u64) -> u64 {
    use od_isa::long_decode::{Operand, Operation};
    for instruction in instructions {
        defined = match instruction.operation {
            Operation::Shift {
                count: Operand::Immediate(count),
                ..
            } => instruction
                .flag_effects_for_count(count as u8)
                .apply(defined),
            Operation::Shift { .. } => 0,
            _ => instruction.flag_effects().apply(defined),
        } & ARITHMETIC_FLAGS;
    }
    defined
}

/// The arithmetic flags condition `cc` (the low four bits of a Jcc/SETcc/CMOVcc opcode) reads.
fn cc_reads(cc: u8) -> u64 {
    match cc & 0xF {
        0 | 1 => OF,
        2 | 3 => CF,
        4 | 5 => ZF,
        6 | 7 => CF | ZF,
        8 | 9 => SF,
        0xA | 0xB => PF,
        0xC | 0xD => SF | OF,
        _ => ZF | SF | OF,
    }
}

/// The arithmetic flags an instruction reads.
fn reads_of(instruction: &od_isa::long_decode::Instruction) -> u64 {
    use od_isa::long_decode::{ArithmeticKind, Operation, ShiftKind};
    match instruction.operation {
        Operation::Branch { condition, .. }
        | Operation::SetCondition { condition, .. }
        | Operation::ConditionalMove { condition, .. } => cc_reads(condition as u8),
        Operation::Arithmetic {
            kind: ArithmeticKind::Adc | ArithmeticKind::Sbb,
            ..
        } => CF,
        Operation::Shift {
            kind: ShiftKind::Rcl | ShiftKind::Rcr,
            ..
        } => CF,
        Operation::PushFlags => ARITHMETIC_FLAGS,
        _ => 0,
    }
}

/// Whether every flag the instructions read is defined when they read it.
fn reads_defined(instructions: &[od_isa::long_decode::Instruction], mut defined: u64) -> bool {
    for instruction in instructions {
        if reads_of(instruction) & !defined != 0 {
            return false;
        }
        defined = defined_after(std::slice::from_ref(instruction), defined);
    }
    true
}

// ---- templates -----------------------------------------------------------------------------------

const SIZES: [u8; 4] = [1, 2, 4, 8];

/// One family of hand-encoded instruction forms. `variant` selects the form (every variant is
/// run by the template tests); the generator randomizes registers, immediates and addresses.
/// A template is stack-neutral, keeps RBX/RBP/RSP and falls through to its end.
struct Template {
    name: &'static str,
    variants: u64,
    /// Whether every instruction of `variant` is one `jit::plan` admits (checked by a test).
    jit: fn(u64) -> bool,
    emit: fn(&mut Asm, &mut Rng, u64),
}

fn yes(_: u64) -> bool {
    true
}

fn no(_: u64) -> bool {
    false
}

/// ADC/SBB of 8 or 16 bits stay on the general path.
fn alu_jit(variant: u64) -> bool {
    let kind = variant % 8;
    let size = SIZES[(variant / 8 % 4) as usize];
    !(matches!(kind, 2 | 3) && size <= 2)
}

fn alu_rr(asm: &mut Asm, rng: &mut Rng, v: u64) {
    let kind = (v % 8) as u8;
    let size = SIZES[(v / 8 % 4) as usize];
    let direction = (v / 32 % 2) as u8;
    let (a, b) = two(rng);
    let b = if rng.chance(10) { a } else { b };
    asm.op(
        &[],
        size,
        &[wide(kind * 8 + direction * 2, size)],
        b,
        Rm::R(a),
        &[],
    );
}

fn alu_ri(asm: &mut Asm, rng: &mut Rng, v: u64) {
    let kind = (v % 8) as u8;
    let size = SIZES[(v / 8 % 4) as usize];
    let value = rng.value();
    match v / 32 % 3 {
        0 => asm.op(
            &[],
            size,
            &[wide(0x80, size)],
            kind,
            Rm::R(free(rng)),
            &imm(size, value),
        ),
        1 if size > 1 => asm.op(&[], size, &[0x83], kind, Rm::R(free(rng)), &[value as u8]),
        1 => asm.op(&[], 1, &[0x80], kind, Rm::R(free(rng)), &[value as u8]),
        // The accumulator form.
        _ => {
            let mut prefixes = Vec::new();
            if size == 2 {
                prefixes.push(0x66);
            }
            if size == 8 {
                prefixes.push(0x48);
            }
            prefixes.push(wide(kind * 8 + 4, size));
            asm.raw(&prefixes);
            asm.raw(&imm(size, value));
        }
    }
}

fn alu_rm(asm: &mut Asm, rng: &mut Rng, v: u64) {
    let kind = (v % 8) as u8;
    let size = SIZES[(v / 8 % 4) as usize];
    let operand = mem(rng, size);
    asm.op(
        &[],
        size,
        &[wide(kind * 8 + 2, size)],
        free(rng),
        operand,
        &[],
    );
}

fn alu_mr(asm: &mut Asm, rng: &mut Rng, v: u64) {
    let kind = (v % 8) as u8;
    let size = SIZES[(v / 8 % 4) as usize];
    let operand = mem(rng, size);
    asm.op(&[], size, &[wide(kind * 8, size)], free(rng), operand, &[]);
}

fn alu_mi(asm: &mut Asm, rng: &mut Rng, v: u64) {
    let kind = (v % 8) as u8;
    let size = SIZES[(v / 8 % 4) as usize];
    let operand = mem(rng, size);
    let value = rng.value();
    if v / 32 % 2 == 1 && size > 1 {
        asm.op(&[], size, &[0x83], kind, operand, &[value as u8]);
    } else {
        asm.op(
            &[],
            size,
            &[wide(0x80, size)],
            kind,
            operand,
            &imm(size, value),
        );
    }
}

/// INC/DEC, NEG/NOT of a register or memory.
fn unary(asm: &mut Asm, rng: &mut Rng, v: u64) {
    // 0 INC, 1 DEC (FE/FF), 2 NOT, 3 NEG (F6/F7).
    let kind = v % 4;
    let size = SIZES[(v / 4 % 4) as usize];
    let operand = if v / 16 % 2 == 0 {
        Rm::R(free(rng))
    } else {
        mem(rng, size)
    };
    let (opcode, reg) = match kind {
        0 => (0xFE, 0),
        1 => (0xFE, 1),
        2 => (0xF6, 2),
        _ => (0xF6, 3),
    };
    asm.op(&[], size, &[wide(opcode, size)], reg, operand, &[]);
}

fn test_op(asm: &mut Asm, rng: &mut Rng, v: u64) {
    let size = SIZES[(v % 4) as usize];
    let value = rng.value();
    match v / 4 % 5 {
        0 => {
            let (a, b) = two(rng);
            asm.op(&[], size, &[wide(0x84, size)], b, Rm::R(a), &[]);
        }
        1 => {
            let mut bytes = Vec::new();
            if size == 2 {
                bytes.push(0x66);
            }
            if size == 8 {
                bytes.push(0x48);
            }
            bytes.push(wide(0xA8, size));
            bytes.extend(imm(size, value));
            asm.raw(&bytes);
        }
        2 => asm.op(
            &[],
            size,
            &[wide(0xF6, size)],
            0,
            Rm::R(free(rng)),
            &imm(size, value),
        ),
        3 => {
            let operand = mem(rng, size);
            asm.op(&[], size, &[wide(0x84, size)], free(rng), operand, &[]);
        }
        _ => {
            let operand = mem(rng, size);
            asm.op(
                &[],
                size,
                &[wide(0xF6, size)],
                0,
                operand,
                &imm(size, value),
            );
        }
    }
}

const SHIFT_KINDS: [u8; 7] = [0, 1, 2, 3, 4, 5, 7];

fn shift_count(rng: &mut Rng, width: u64, index: u64) -> u8 {
    match index {
        0 => 0,
        1 => 1,
        2 => (width - 1) as u8,
        3 => width as u8,
        4 => (width + 1) as u8,
        5 => 31,
        6 => 63,
        _ => rng.below(256) as u8,
    }
}

fn shift_imm_jit(v: u64) -> bool {
    let kind = SHIFT_KINDS[(v % 7) as usize];
    let size = SIZES[(v / 7 % 4) as usize];
    let width = u32::from(size) * 8;
    if matches!(kind, 2 | 3) {
        return false;
    }
    if width >= 32 {
        return true;
    }
    // 8/16 bits: SHL/SHR/SAR by an immediate below the width; the random count is decided in
    // the generator (kept below the width there).
    let index = v / 28 % 8;
    let count = match index {
        0 => 0,
        1 => 1,
        2 => width - 1,
        3 => width,
        4 => width + 1,
        5 => 31,
        6 => 63,
        _ => 0,
    } & 0x1F;
    matches!(kind, 4 | 5 | 7) && count < width
}

fn shift_imm(asm: &mut Asm, rng: &mut Rng, v: u64) {
    let kind = SHIFT_KINDS[(v % 7) as usize];
    let size = SIZES[(v / 7 % 4) as usize];
    let width = u64::from(size) * 8;
    let index = v / 28 % 8;
    let mut count = shift_count(rng, width, index);
    if index == 7 && width < 32 {
        count = rng.below(width) as u8;
    }
    let dst = Rm::R(free(rng));
    if count == 1 && rng.chance(50) {
        asm.op(&[], size, &[wide(0xD0, size)], kind, dst, &[]);
    } else {
        asm.op(&[], size, &[wide(0xC0, size)], kind, dst, &[count]);
    }
}

/// SHL/SHR/SAR/ROL/ROR/RCL by CL (CL set first).
const CL_KINDS: [u8; 6] = [4, 5, 7, 0, 1, 2];

fn shift_cl_jit(v: u64) -> bool {
    let kind = CL_KINDS[(v % 6) as usize];
    let size = SIZES[(v / 6 % 4) as usize];
    size >= 4 && matches!(kind, 4 | 5 | 7)
}

fn shift_cl(asm: &mut Asm, rng: &mut Rng, v: u64) {
    let kind = CL_KINDS[(v % 6) as usize];
    let size = SIZES[(v / 6 % 4) as usize];
    let width = u64::from(size) * 8;
    let count = shift_count(rng, width, [0, 1, 2, 3, 4, 7][(v / 24 % 6) as usize]);
    // MOV ECX, count with random upper garbage in the low byte's neighbours.
    let garbage = (rng.next() as u32) & 0xFFFF_FF00;
    asm.mov_imm32(RCX, garbage | u32::from(count));
    asm.op(&[], size, &[wide(0xD2, size)], kind, Rm::R(free(rng)), &[]);
}

fn shift_mem(asm: &mut Asm, rng: &mut Rng, v: u64) {
    let size = SIZES[(v % 4) as usize];
    let operand = mem(rng, size);
    asm.op(
        &[],
        size,
        &[wide(0xC0, size)],
        4 + (v / 4 % 2) as u8,
        operand,
        &[rng.below(8) as u8],
    );
}

fn shld(asm: &mut Asm, rng: &mut Rng, v: u64) {
    let opcode = if v % 2 == 0 { 0xA4 } else { 0xAC };
    let size = [4u8, 8][(v / 2 % 2) as usize];
    let width = u64::from(size) * 8;
    let count = shift_count(rng, width, [0, 1, 2, 3, 4, 7][(v / 4 % 6) as usize]);
    let (a, b) = two(rng);
    asm.op(&[], size, &[0x0F, opcode], b, Rm::R(a), &[count]);
}

/// SHLD/SHRD by CL, and the 16-bit forms: the general path.
fn shld_slow(asm: &mut Asm, rng: &mut Rng, v: u64) {
    let size = [2u8, 4, 8][(v % 3) as usize];
    let (a, b) = two(rng);
    let (a, b) = (
        if a == RCX { R14 } else { a },
        if b == RCX { R15 } else { b },
    );
    if v / 3 % 2 == 0 {
        asm.mov_imm32(RCX, rng.below(u64::from(size) * 8) as u32);
        asm.op(
            &[],
            size,
            &[0x0F, if v / 6 % 2 == 0 { 0xA5 } else { 0xAD }],
            b,
            Rm::R(a),
            &[],
        );
    } else {
        let count = rng.below(16) as u8;
        asm.op(
            &[],
            2,
            &[0x0F, if v / 6 % 2 == 0 { 0xA4 } else { 0xAC }],
            b,
            Rm::R(a),
            &[count],
        );
    }
}

/// BT/BTS/BTR/BTC: register and memory destinations, register and immediate offsets (memory
/// register offsets negative and beyond the operand, with garbage above the operand size).
fn bit(asm: &mut Asm, rng: &mut Rng, v: u64) {
    let kind = (v % 4) as u8;
    let size = [2u8, 4, 8][(v / 4 % 3) as usize];
    let register_opcode = [0xA3, 0xAB, 0xB3, 0xBB][kind as usize];
    match v / 12 % 4 {
        0 => {
            let (a, b) = two(rng);
            asm.op(&[], size, &[0x0F, register_opcode], b, Rm::R(a), &[]);
        }
        1 => asm.op(
            &[],
            size,
            &[0x0F, 0xBA],
            4 + kind,
            Rm::R(free(rng)),
            &[rng.below(256) as u8],
        ),
        2 => {
            let offset_register = rng.pick(&[RAX, RCX, RDX, RSI, RDI, R8, R13]);
            let bits: i64 = match rng.below(3) {
                0 => rng.below(64) as i64,
                1 => -(rng.below(0x8000) as i64) - 1,
                _ => rng.below(0x10000) as i64,
            };
            let value = match size {
                2 => (bits as u16 as u64) | (rng.next() << 16),
                4 => (bits as u32 as u64) | (rng.next() << 32),
                _ => bits as u64,
            };
            asm.mov_imm(offset_register, value);
            let base = if rng.chance(50) { RBX } else { RBP };
            let disp = rng.below(0x100) as i32;
            asm.op(
                &[],
                size,
                &[0x0F, register_opcode],
                offset_register,
                Rm::M(base, disp),
                &[],
            );
        }
        _ => {
            let operand = mem(rng, size);
            asm.op(
                &[],
                size,
                &[0x0F, 0xBA],
                4 + kind,
                operand,
                &[rng.below(256) as u8],
            );
        }
    }
}

fn bitscan(asm: &mut Asm, rng: &mut Rng, v: u64) {
    let (prefixes, opcode): (&[u8], u8) = match v % 3 {
        0 => (&[], 0xBC),
        1 => (&[], 0xBD),
        _ => (&[0xF3], 0xBC),
    };
    let size = [4u8, 8][(v / 3 % 2) as usize];
    let (dst, src) = two(rng);
    if v / 6 % 2 == 0 {
        if rng.chance(25) {
            asm.op(&[], 8, &[0x31], src, Rm::R(src), &[]);
        }
        asm.op(prefixes, size, &[0x0F, opcode], dst, Rm::R(src), &[]);
    } else {
        let operand = mem(rng, size);
        asm.op(prefixes, size, &[0x0F, opcode], dst, operand, &[]);
    }
}

/// MUL, one-operand IMUL, IMUL r, r/m, IMUL r, r/m, imm.
fn mul(asm: &mut Asm, rng: &mut Rng, v: u64) {
    let size = [4u8, 8][(v / 4 % 2) as usize];
    let operand = if v / 8 % 2 == 0 {
        Rm::R(free(rng))
    } else {
        mem(rng, size)
    };
    match v % 4 {
        0 => asm.op(&[], size, &[0xF7], 4, operand, &[]),
        1 => asm.op(&[], size, &[0xF7], 5, operand, &[]),
        2 => asm.op(&[], size, &[0x0F, 0xAF], free(rng), operand, &[]),
        _ => {
            if rng.chance(50) {
                asm.op(&[], size, &[0x6B], free(rng), operand, &[rng.next() as u8]);
            } else {
                asm.op(&[], size, &[0x69], free(rng), operand, &imm(4, rng.value()));
            }
        }
    }
}

fn mul_narrow(asm: &mut Asm, rng: &mut Rng, v: u64) {
    let size = [1u8, 2][(v % 2) as usize];
    let operand = if v / 2 % 2 == 0 {
        Rm::R(free(rng))
    } else {
        mem(rng, size)
    };
    asm.op(
        &[],
        size,
        &[wide(0xF6, size)],
        4 + (v / 4 % 2) as u8,
        operand,
        &[],
    );
}

/// DIV/IDIV that do not fault: RDX below the divisor (DIV) or the sign of RAX (IDIV).
fn divide(asm: &mut Asm, rng: &mut Rng, v: u64) {
    let signed = v % 2 == 1;
    let size = [4u8, 8][(v / 2 % 2) as usize];
    let divisor = rng.pick(&[RCX, RSI, RDI, R8, R13, R15]);
    let top = if size == 8 { 1u64 << 63 } else { 1u64 << 31 };
    let mask = if size == 8 { u64::MAX } else { 0xFFFF_FFFF };
    if signed {
        let mut value = rng.value() & mask;
        if value == 0 || value == mask {
            value = 7;
        }
        if rng.chance(30) {
            value = 1;
        }
        asm.mov_imm(divisor, value | (rng.next() & !mask));
        // CDQ / CQO.
        asm.raw(if size == 8 { &[0x48, 0x99] } else { &[0x99] });
    } else {
        let value = (rng.value() | top) & mask;
        asm.mov_imm(divisor, value | (rng.next() & !mask));
        asm.mov_imm(RDX, rng.below(value) | (rng.next() & !mask));
    }
    if v / 4 % 2 == 0 {
        asm.op(
            &[],
            size,
            &[0xF7],
            if signed { 7 } else { 6 },
            Rm::R(divisor),
            &[],
        );
    } else {
        let operand = Rm::M(RBX, (rng.below(0x200) * 8) as i32);
        asm.op(&[], size, &[0x89], divisor, operand, &[]);
        asm.op(&[], size, &[0xF7], if signed { 7 } else { 6 }, operand, &[]);
    }
}

fn cmov(asm: &mut Asm, rng: &mut Rng, v: u64) {
    let cc = (v % 16) as u8;
    let size = [2u8, 4, 8][(v / 16 % 3) as usize];
    let operand = if v / 48 % 2 == 0 {
        Rm::R(free(rng))
    } else {
        mem(rng, size)
    };
    asm.op(&[], size, &[0x0F, 0x40 + cc], free(rng), operand, &[]);
}

fn setcc(asm: &mut Asm, rng: &mut Rng, v: u64) {
    let cc = (v % 16) as u8;
    match v / 16 % 3 {
        0 => asm.op(&[], 1, &[0x0F, 0x90 + cc], 0, Rm::R(free(rng)), &[]),
        // AH, CH or DH (no REX).
        1 => asm.enc(
            &[],
            false,
            false,
            &[0x0F, 0x90 + cc],
            0,
            Rm::R(4 + rng.below(3) as u8),
            &[],
        ),
        _ => {
            let operand = mem(rng, 1);
            asm.op(&[], 1, &[0x0F, 0x90 + cc], 0, operand, &[]);
        }
    }
}

/// Jcc over a filler that changes no flag.
fn jcc(asm: &mut Asm, rng: &mut Rng, v: u64) {
    let cc = (v % 16) as u8;
    let body = filler(free(rng));
    if v / 16 % 2 == 0 {
        asm.raw(&[0x70 + cc, body.len() as u8]);
    } else {
        asm.raw(&[0x0F, 0x80 + cc]);
        asm.raw(&(body.len() as i32).to_le_bytes());
    }
    asm.raw(&body);
}

/// JMP short/near over a filler.
fn jmp(asm: &mut Asm, rng: &mut Rng, v: u64) {
    let body = filler(free(rng));
    if v % 2 == 0 {
        asm.raw(&[0xEB, body.len() as u8]);
    } else {
        asm.raw(&[0xE9]);
        asm.raw(&(body.len() as i32).to_le_bytes());
    }
    asm.raw(&body);
}

fn push_reg(asm: &mut Asm, reg: u8, word: bool) {
    if word {
        asm.raw(&[0x66]);
    }
    if reg >= 8 {
        asm.raw(&[0x41]);
    }
    asm.raw(&[0x50 + (reg & 7)]);
}

fn pop_reg(asm: &mut Asm, reg: u8, word: bool) {
    if word {
        asm.raw(&[0x66]);
    }
    if reg >= 8 {
        asm.raw(&[0x41]);
    }
    asm.raw(&[0x58 + (reg & 7)]);
}

/// `LEA reg, [rip + target]`.
fn lea_rip(asm: &mut Asm, reg: u8, target: u64) {
    asm.enc(&[], true, false, &[0x8D], reg, Rm::Rip(target), &[]);
}

/// Stack forms: PUSH/POP, PUSH imm, PUSHF, CALL/RET (also RET imm16), indirect CALL/JMP through
/// a register and memory, LEAVE. Every form leaves RSP as it found it.
fn stack(asm: &mut Asm, rng: &mut Rng, v: u64) {
    let (a, b) = two(rng);
    match v {
        0 => {
            push_reg(asm, a, false);
            pop_reg(asm, b, false);
        }
        1 => {
            push_reg(asm, a, true);
            pop_reg(asm, b, true);
        }
        2 => {
            asm.raw(&[0x6A, rng.next() as u8]);
            pop_reg(asm, b, false);
        }
        3 => {
            asm.raw(&[0x68]);
            asm.raw(&(rng.next() as u32).to_le_bytes());
            pop_reg(asm, b, false);
        }
        4 => {
            asm.raw(&[0x9C]);
            pop_reg(asm, b, false);
        }
        5 | 6 => {
            // [push a]; call sub; jmp end; sub: bump b; ret [8]; end:
            if v == 6 {
                push_reg(asm, a, false);
            }
            let body = filler(b);
            let ret: &[u8] = if v == 6 { &[0xC2, 8, 0] } else { &[0xC3] };
            asm.raw(&[0xE8]);
            asm.raw(&2i32.to_le_bytes());
            asm.raw(&[0xEB, (body.len() + ret.len()) as u8]);
            asm.raw(&body);
            asm.raw(ret);
        }
        7 | 8 | 12 => {
            // lea r, [rip + sub]; [mov [rbx + d], r]; call r / call [rbx + d]; jmp end;
            // sub: bump b; ret; end:
            let reg = if v == 12 { RAX } else { R11 };
            let b = if b == reg { R14 } else { b };
            let disp = (rng.below(0x100) * 8) as i32;
            let body = filler(b);
            let call = |asm: &mut Asm, sub: u64| {
                lea_rip(asm, reg, sub);
                if v == 8 {
                    asm.op(&[], 8, &[0x89], reg, Rm::M(RBX, disp), &[]);
                    asm.raw(&[0xFF, 0x93]);
                    asm.raw(&disp.to_le_bytes());
                } else if reg >= 8 {
                    asm.raw(&[0x41, 0xFF, 0xD0 + (reg & 7)]);
                } else {
                    asm.raw(&[0xFF, 0xD0 + reg]);
                }
                asm.raw(&[0xEB, body.len() as u8 + 1]);
            };
            let mut probe = Asm::new(asm.pc());
            call(&mut probe, 0);
            let sub = probe.pc();
            call(asm, sub);
            asm.raw(&body);
            asm.raw(&[0xC3]);
        }
        9 | 10 => {
            // lea r11, [rip + target]; [mov [rbx + d], r11]; jmp r11 / jmp [rbx + d]; bump; target:
            let disp = (rng.below(0x100) * 8) as i32;
            let tail = |asm: &mut Asm| {
                if v == 10 {
                    asm.op(&[], 8, &[0x89], R11, Rm::M(RBX, disp), &[]);
                    asm.raw(&[0xFF, 0xA3]);
                    asm.raw(&disp.to_le_bytes());
                } else {
                    asm.raw(&[0x41, 0xFF, 0xE3]);
                }
            };
            let body = filler(if b == R11 { R14 } else { b });
            let mut probe = Asm::new(asm.pc());
            lea_rip(&mut probe, R11, 0);
            tail(&mut probe);
            let target = probe.pc() + body.len() as u64;
            lea_rip(asm, R11, target);
            tail(asm);
            asm.raw(&body);
        }
        11 => {
            // push rbp; mov rbp, rsp; sub rsp, n; mov [rbp - 8], a; leave
            push_reg(asm, RBP, false);
            asm.op(&[], 8, &[0x89], RSP, Rm::R(RBP), &[]);
            asm.op(&[], 8, &[0x83], 5, Rm::R(RSP), &[0x20]);
            asm.op(&[], 8, &[0x89], a, Rm::M(RBP, -8), &[]);
            asm.raw(&[0xC9]);
        }
        _ => {
            push_reg(asm, a, false);
            push_reg(asm, b, false);
            pop_reg(asm, a, false);
            pop_reg(asm, b, false);
        }
    }
}

/// PUSH/POP of memory, POPF: the general path.
fn stack_slow(asm: &mut Asm, rng: &mut Rng, v: u64) {
    let operand = Rm::M(RBX, (rng.below(0x200) * 8) as i32);
    match v {
        0 => {
            asm.enc(&[], false, false, &[0xFF], 6, operand, &[]);
            asm.enc(&[], false, false, &[0x8F], 0, operand, &[]);
        }
        _ => {
            // PUSH imm32 of random arithmetic flags (DF clear); POPF.
            let flags = (rng.next() & ARITHMETIC_FLAGS) | 2;
            asm.raw(&[0x68]);
            asm.raw(&(flags as u32).to_le_bytes());
            asm.raw(&[0x9D]);
        }
    }
}

/// LEA with base, base + index * scale, RIP-relative, 32-bit destination, 32-bit address size.
fn lea(asm: &mut Asm, rng: &mut Rng, v: u64) {
    let dst = free(rng);
    let base = rng.below(16) as u8;
    let index = loop {
        let index = rng.below(16) as u8;
        if index != RSP {
            break index;
        }
    };
    let disp = rng.next() as i32 >> rng.below(32);
    let scale = rng.below(4) as u8;
    match v {
        0 => asm.op(&[], 8, &[0x8D], dst, Rm::M(base, disp), &[]),
        1 => asm.op(&[], 8, &[0x8D], dst, Rm::Sib(base, index, scale, disp), &[]),
        2 => {
            let target = asm.pc().wrapping_add(disp as i64 as u64);
            asm.op(&[], 8, &[0x8D], dst, Rm::Rip(target), &[]);
        }
        3 => asm.op(&[], 4, &[0x8D], dst, Rm::Sib(base, index, scale, disp), &[]),
        4 => asm.op(
            &[0x67],
            8,
            &[0x8D],
            dst,
            Rm::Sib(base, index, scale, disp),
            &[],
        ),
        _ => asm.op(&[], 2, &[0x8D], dst, Rm::M(base, disp), &[]),
    }
}

/// A data address for a RIP-relative operand (in the same half as the code).
fn data_target(asm: &Asm, rng: &mut Rng, size: u8) -> u64 {
    (asm.pc() & HIGH) + DATA + rng.below(0x8000 - u64::from(size))
}

fn mov(asm: &mut Asm, rng: &mut Rng, v: u64) {
    let size = SIZES[(v % 4) as usize];
    let (a, b) = two(rng);
    let value = rng.value();
    match v / 4 % 12 {
        0 => asm.op(&[], size, &[wide(0x88, size)], b, Rm::R(a), &[]),
        1 => asm.op(&[], size, &[wide(0x8A, size)], b, Rm::R(a), &[]),
        2 => match size {
            8 => asm.mov_imm(a, value),
            1 => asm.raw(&[0x40 | (a >> 3), 0xB0 + (a & 7), value as u8]),
            _ => {
                let mut bytes = Vec::new();
                if size == 2 {
                    bytes.push(0x66);
                }
                if a >= 8 {
                    bytes.push(0x41);
                }
                bytes.push(0xB8 + (a & 7));
                bytes.extend(imm(size, value));
                asm.raw(&bytes);
            }
        },
        3 => asm.op(
            &[],
            size,
            &[wide(0xC6, size)],
            0,
            Rm::R(a),
            &imm(size, value),
        ),
        4 => {
            let operand = mem(rng, size);
            asm.op(&[], size, &[wide(0x8A, size)], b, operand, &[]);
        }
        5 => {
            let operand = mem(rng, size);
            asm.op(&[], size, &[wide(0x88, size)], b, operand, &[]);
        }
        6 => {
            let operand = mem(rng, size);
            asm.op(
                &[],
                size,
                &[wide(0xC6, size)],
                0,
                operand,
                &imm(size, value),
            );
        }
        7 | 8 => {
            let index = rng.pick(&[RSI, RDI, R8, R13]);
            let b = if b == index { R14 } else { b };
            let scale = rng.below(4) as u8;
            asm.mov_imm32(index, rng.below(0x400) as u32);
            let operand = Rm::Sib(RBX, index, scale, rng.below(0x100) as i32 - 0x80);
            let opcode = if v / 4 % 12 == 7 { 0x8A } else { 0x88 };
            asm.op(&[], size, &[wide(opcode, size)], b, operand, &[]);
        }
        9 => {
            let address = (DATA + rng.below(0x8000 - u64::from(size))) as i32;
            asm.op(&[], size, &[wide(0x8A, size)], b, Rm::Abs(address), &[]);
        }
        10 => {
            let target = data_target(asm, rng, size);
            asm.op(&[], size, &[wide(0x8A, size)], b, Rm::Rip(target), &[]);
        }
        _ => {
            let target = data_target(asm, rng, size);
            asm.op(&[], size, &[wide(0x88, size)], b, Rm::Rip(target), &[]);
        }
    }
}

/// MOVZX/MOVSX of a byte or word, MOVSXD; register and memory sources.
fn movx(asm: &mut Asm, rng: &mut Rng, v: u64) {
    let kind = v % 5;
    // The decoder has no MOVZX/MOVSX r16, r/m16.
    let size = if kind == 4 {
        8
    } else if matches!(kind, 1 | 3) {
        [4u8, 4, 8][(v / 5 % 3) as usize]
    } else {
        [2u8, 4, 8][(v / 5 % 3) as usize]
    };
    let source_size = match kind {
        0 | 2 => 1,
        1 | 3 => 2,
        _ => 4,
    };
    let (dst, src) = two(rng);
    let operand = if v / 15 % 2 == 0 {
        Rm::R(src)
    } else {
        mem(rng, source_size)
    };
    let opcode: &[u8] = match kind {
        0 => &[0x0F, 0xB6],
        1 => &[0x0F, 0xB7],
        2 => &[0x0F, 0xBE],
        3 => &[0x0F, 0xBF],
        _ => &[0x63],
    };
    // A byte source register needs REX for SIL/DIL and R8B-R15B (the low byte of every register).
    let mut prefixes = Vec::new();
    if size == 2 {
        prefixes.push(0x66);
    }
    asm.enc(
        &prefixes,
        size == 8,
        source_size == 1,
        opcode,
        dst,
        operand,
        &[],
    );
}

/// XCHG: the general path.
fn xchg(asm: &mut Asm, rng: &mut Rng, v: u64) {
    let (a, b) = two(rng);
    let size = [4u8, 8][(v / 2 % 2) as usize];
    if v % 2 == 0 {
        asm.op(&[], size, &[0x87], b, Rm::R(a), &[]);
    } else {
        let operand = Rm::M(RBX, (rng.below(0x200) * 8) as i32);
        asm.op(&[], size, &[0x87], b, operand, &[]);
    }
}

/// CMPXCHG m32/m64 with LOCK, without a prefix and with a DS prefix, the accumulator equal to the
/// memory (loaded first) or not.
/// Equal accumulator: variants 0..6 of `cmpxchg_any`.
fn cmpxchg(asm: &mut Asm, rng: &mut Rng, v: u64) {
    cmpxchg_any(asm, rng, v % 6);
}

/// Unequal accumulator: variants 6..12 of `cmpxchg_any` (a known mismatch, see
/// `cmpxchg_with_an_unequal_accumulator_loads_the_old_memory_value`).
fn cmpxchg_unequal(asm: &mut Asm, rng: &mut Rng, v: u64) {
    cmpxchg_any(asm, rng, 6 + v % 6);
}

fn cmpxchg_any(asm: &mut Asm, rng: &mut Rng, v: u64) {
    let prefix: &[u8] = [&[0xF0u8][..], &[], &[0x3E]][(v % 3) as usize];
    let size = [4u8, 8][(v / 3 % 2) as usize];
    let operand = Rm::M(RBX, (rng.below(0x400) * u64::from(size)) as i32 - 0x400);
    if v / 6 % 2 == 0 {
        asm.op(&[], size, &[0x8B], RAX, operand, &[]);
    }
    let source = loop {
        let source = free(rng);
        if source != RAX {
            break source;
        }
    };
    asm.op(prefix, size, &[0x0F, 0xB1], source, operand, &[]);
}

fn xadd(asm: &mut Asm, rng: &mut Rng, v: u64) {
    let prefix: &[u8] = [&[0xF0u8][..], &[], &[0x3E]][(v % 3) as usize];
    let size = [4u8, 8][(v / 3 % 2) as usize];
    let operand = Rm::M(RBX, (rng.below(0x400) * u64::from(size)) as i32 - 0x400);
    asm.op(prefix, size, &[0x0F, 0xC1], free(rng), operand, &[]);
}

/// CWDE, CDQE, CDQ, CQO; then CBW, CWD (the general path).
fn signext(asm: &mut Asm, _: &mut Rng, v: u64) {
    asm.raw(match v {
        0 => &[0x98],
        1 => &[0x48, 0x98],
        2 => &[0x99],
        3 => &[0x48, 0x99],
        4 => &[0x66, 0x98],
        _ => &[0x66, 0x99],
    });
}

fn signext_jit(v: u64) -> bool {
    v < 4
}

fn bswap(asm: &mut Asm, rng: &mut Rng, v: u64) {
    let reg = free(rng);
    let rex = 0x40 | if v == 1 { 8 } else { 0 } | (reg >> 3);
    if rex != 0x40 {
        asm.raw(&[rex]);
    }
    asm.raw(&[0x0F, 0xC8 + (reg & 7)]);
}

/// CLD, STD + CLD, NOP, NOP r/m, PAUSE, ENDBR64, LFENCE, INVLPG, CLI, STI, CLI + STI.
fn misc(asm: &mut Asm, rng: &mut Rng, v: u64) {
    match v {
        0 => asm.raw(&[0xFC]),
        1 => asm.raw(&[0xFD, 0xFC]),
        2 => asm.raw(&[0x90]),
        3 => asm.raw(if rng.chance(50) {
            &[0x0F, 0x1F, 0x40, 0x00]
        } else {
            &[0x0F, 0x1F, 0x44, 0x00, 0x00]
        }),
        4 => asm.raw(&[0xF3, 0x90]),
        5 => asm.raw(&[0xF3, 0x0F, 0x1E, 0xFA]),
        6 => asm.raw(&[0x0F, 0xAE, 0xE8]),
        7 => {
            let operand = mem(rng, 1);
            asm.enc(&[], false, false, &[0x0F, 0x01], 7, operand, &[]);
        }
        8 => asm.raw(&[0xFA]),
        // STI, then NOP in its shadow (the general path refuses an STI in the shadow of STI, see
        // `an_sti_in_the_shadow_of_sti_is_refused_by_the_general_step`).
        9 => asm.raw(&[0xFB, 0x90]),
        _ => asm.raw(&[0xFA, 0xFB, 0x90]),
    }
}

/// REP MOVS/STOS of every size: counts 0, 1, small, crossing pages, above the iteration limit,
/// an overlapping forward copy (MOVS) and DF = 1 (STOS, the general path).
fn rep(asm: &mut Asm, rng: &mut Rng, v: u64) {
    let movs = v % 2 == 0;
    let size = SIZES[(v / 2 % 4) as usize];
    let case = v / 8 % 6;
    let count = match case {
        0 => 0,
        1 => 1,
        2 => 2 + rng.below(30),
        3 => 0x1000 / u64::from(size) + rng.below(50),
        4 => REP_ITERATION_WORK_LIMIT + 1 + rng.below(200),
        _ => 3 + rng.below(100),
    };
    // Aligned elements (the helper's case) or not (a crossing element sends REP to the general
    // path).
    let align = if rng.chance(50) {
        !(u64::from(size) - 1)
    } else {
        u64::MAX
    };
    let rdi = (DATA + 0x20 + rng.below(0x800)) & align;
    let rsi = if movs && case == 5 {
        // Forward copy onto itself a few bytes ahead.
        rdi - 1 - rng.below(16)
    } else {
        (BIG_PTR - 0x8000 + rng.below(0x8000)) & align
    };
    asm.mov_imm(RCX, count);
    asm.mov_imm(RDI, rdi);
    asm.mov_imm(RSI, rsi);
    asm.mov_imm(RAX, rng.next());
    let std = !movs && case == 5;
    if std {
        asm.raw(&[0xFD]);
    }
    let mut bytes = vec![0xF3];
    if size == 2 {
        bytes.push(0x66);
    }
    if size == 8 {
        bytes.push(0x48);
    }
    bytes.push(wide(if movs { 0xA4 } else { 0xAA }, size));
    asm.raw(&bytes);
    if std {
        asm.raw(&[0xFC]);
    }
}

/// The GS-relative forms `jit::plan` admits.
fn gs(asm: &mut Asm, rng: &mut Rng, v: u64) {
    let wide64 = rng.chance(50);
    let size = if wide64 { 8 } else { 4 };
    // `gs:[disp32]` (per-CPU style) or `gs:[rbx + disp]`: inside the data or ALT pages.
    let operand = if rng.chance(60) {
        Rm::Abs(rng.below(0x2FF0) as i32 & !(i32::from(size) - 1))
    } else {
        Rm::M(RBX, (rng.below(0x7F0)) as i32 & !(i32::from(size) - 1))
    };
    let reg = free(rng);
    let value = rng.value();
    let g: &[u8] = &[0x65];
    match v {
        0 => asm.op(g, 8, &[0x89], reg, operand, &[]),
        1 => asm.op(g, 4, &[0x89], reg, operand, &[]),
        2 => asm.op(g, 2, &[0x89], reg, operand, &[]),
        3 => asm.op(g, 8, &[0x8B], reg, operand, &[]),
        4 => asm.op(g, 4, &[0x8B], reg, operand, &[]),
        5 => asm.enc(g, wide64, false, &[0x0F, 0xB7], reg, operand, &[]),
        6 => asm.enc(g, wide64, false, &[0x0F, 0xB6], reg, operand, &[]),
        7 => asm.enc(g, wide64, false, &[0x0F, 0xBE], reg, operand, &[]),
        8 => asm.op(g, 8, &[0x63], reg, operand, &[]),
        9 => asm.enc(g, false, false, &[0xC6], 0, operand, &[value as u8]),
        10 => asm.op(g, size, &[0xC7], 0, operand, &imm(4, value)),
        11 => asm.enc(g, false, false, &[0xF6], 0, operand, &[value as u8]),
        12 => asm.op(g, 2, &[0xF7], 0, operand, &imm(2, value)),
        13 => asm.op(g, size, &[0xF7], 0, operand, &imm(4, value)),
        14 => asm.op(g, size, &[0xFF], rng.below(2) as u8, operand, &[]),
        15 => asm.op(g, 8, &[0x2B], reg, operand, &[]),
        16 => asm.op(g, size, &[0x03], reg, operand, &[]),
        17 => asm.op(g, size, &[0x3B], reg, operand, &[]),
        18 => asm.op(g, size, &[rng.pick(&[0x0B, 0x23, 0x33])], reg, operand, &[]),
        19 => asm.op(g, size, &[0x39], reg, operand, &[]),
        20 => asm.op(g, size, &[0x01], reg, operand, &[]),
        21 => asm.op(g, size, &[0x83], 7, operand, &[value as u8]),
        22 => asm.op(g, size, &[0x81], 0, operand, &imm(4, value)),
        23 => asm.op(g, size, &[0x81], rng.pick(&[4, 6]), operand, &imm(4, value)),
        _ => asm.op(g, size, &[0x81], 5, operand, &imm(4, value)),
    }
}

/// The LOCK forms `jit::plan` admits but `LOCK SUB m32, imm8` (a known mismatch, see
/// `lock_sub_dword_imm8_is_refused_by_the_general_step`).
fn lock(asm: &mut Asm, rng: &mut Rng, v: u64) {
    lock_any(asm, rng, if v >= 10 { v + 1 } else { v });
}

fn lock_sub_dword_imm8(asm: &mut Asm, rng: &mut Rng, _: u64) {
    lock_any(asm, rng, 10);
}

/// The LOCK forms `jit::plan` admits (aligned operands).
fn lock_any(asm: &mut Asm, rng: &mut Rng, v: u64) {
    let size = if rng.chance(50) { 8 } else { 4 };
    let base = if rng.chance(30) { RBP } else { RBX };
    let operand = Rm::M(base, (rng.below(0x400) * 8) as i32 - 0x800);
    let reg = free(rng);
    let value = rng.value();
    let l: &[u8] = &[0xF0];
    match v {
        0 => asm.op(l, size, &[0xFF], 0, operand, &[]),
        1 => asm.op(l, size, &[0xFF], 1, operand, &[]),
        2 => asm.op(l, 1, &[0x80], 1, operand, &[value as u8]),
        3 => asm.op(l, size, &[0x81], 1, operand, &imm(4, value)),
        4 => asm.op(l, size, &[0x83], 1, operand, &[value as u8]),
        5 => asm.op(l, 1, &[0x80], 0, operand, &[value as u8]),
        6 => asm.op(l, size, &[0x81], 0, operand, &imm(4, value)),
        7 => asm.op(l, size, &[0x83], 0, operand, &[value as u8]),
        8 => asm.op(l, size, &[0x81], 5, operand, &imm(4, value)),
        9 => asm.op(l, 8, &[0x83], 5, operand, &[value as u8]),
        10 => asm.op(l, 4, &[0x83], 5, operand, &[value as u8]),
        11 => asm.op(l, size, &[0x01], reg, operand, &[]),
        12 => asm.op(l, size, &[0x29], reg, operand, &[]),
        13 => asm.op(l, 1, &[0x80], 4, operand, &[value as u8]),
        14 => asm.op(l, 8, &[0x83], 4, operand, &[value as u8]),
        15 => asm.op(l, 1, &[0x80], 6, operand, &[value as u8]),
        16 => asm.op(l, 8, &[0x21], reg, operand, &[]),
        // LOCK XOR m8, r8 decodes without REX only (AL, CL, DL).
        _ => asm.enc(l, false, false, &[0x30], rng.below(3) as u8, operand, &[]),
    }
}

/// MOV r, Sreg (ES, CS, SS, DS, FS, GS) into a 32- and 64-bit register (the decoder has no 16-bit
/// register form; `MOV RAX, FS/GS` is a separate operation of the general path).
fn sreg(asm: &mut Asm, rng: &mut Rng, v: u64) {
    let segment = (v % 6) as u8;
    let size = [4u8, 8, 4][(v / 6 % 3) as usize];
    let mut dst = free(rng);
    if dst == RAX && size == 8 && segment >= 4 {
        dst = R13;
    }
    asm.op(&[], size, &[0x8C], segment, Rm::R(dst), &[]);
}

fn rdtsc(asm: &mut Asm, _: &mut Rng, _: u64) {
    asm.raw(&[0x0F, 0x31]);
}

const TEMPLATES: &[Template] = &[
    Template {
        name: "alu_rr",
        variants: 64,
        jit: alu_jit,
        emit: alu_rr,
    },
    Template {
        name: "alu_ri",
        variants: 96,
        jit: alu_jit,
        emit: alu_ri,
    },
    Template {
        name: "alu_rm",
        variants: 32,
        jit: alu_jit,
        emit: alu_rm,
    },
    Template {
        name: "alu_mr",
        variants: 32,
        jit: alu_jit,
        emit: alu_mr,
    },
    Template {
        name: "alu_mi",
        variants: 64,
        jit: alu_jit,
        emit: alu_mi,
    },
    Template {
        name: "unary",
        variants: 32,
        jit: yes,
        emit: unary,
    },
    Template {
        name: "test",
        variants: 20,
        jit: yes,
        emit: test_op,
    },
    Template {
        name: "shift_imm",
        variants: 224,
        jit: shift_imm_jit,
        emit: shift_imm,
    },
    Template {
        name: "shift_cl",
        variants: 144,
        jit: shift_cl_jit,
        emit: shift_cl,
    },
    Template {
        name: "shift_mem",
        variants: 8,
        jit: no,
        emit: shift_mem,
    },
    Template {
        name: "shld",
        variants: 24,
        jit: yes,
        emit: shld,
    },
    Template {
        name: "shld_slow",
        variants: 12,
        jit: no,
        emit: shld_slow,
    },
    Template {
        name: "bit",
        variants: 48,
        jit: yes,
        emit: bit,
    },
    Template {
        name: "bitscan",
        variants: 12,
        jit: yes,
        emit: bitscan,
    },
    Template {
        name: "mul",
        variants: 16,
        jit: yes,
        emit: mul,
    },
    Template {
        name: "mul_narrow",
        variants: 8,
        jit: no,
        emit: mul_narrow,
    },
    Template {
        name: "divide",
        variants: 8,
        jit: yes,
        emit: divide,
    },
    Template {
        name: "cmov",
        variants: 96,
        jit: yes,
        emit: cmov,
    },
    Template {
        name: "setcc",
        variants: 48,
        jit: yes,
        emit: setcc,
    },
    Template {
        name: "jcc",
        variants: 32,
        jit: yes,
        emit: jcc,
    },
    Template {
        name: "jmp",
        variants: 2,
        jit: yes,
        emit: jmp,
    },
    Template {
        name: "stack",
        variants: 14,
        jit: yes,
        emit: stack,
    },
    Template {
        name: "stack_slow",
        variants: 2,
        jit: no,
        emit: stack_slow,
    },
    Template {
        name: "lea",
        variants: 6,
        jit: yes,
        emit: lea,
    },
    Template {
        name: "mov",
        variants: 48,
        jit: yes,
        emit: mov,
    },
    Template {
        name: "movx",
        variants: 30,
        jit: yes,
        emit: movx,
    },
    Template {
        name: "xchg",
        variants: 4,
        jit: no,
        emit: xchg,
    },
    Template {
        name: "cmpxchg",
        variants: 6,
        jit: yes,
        emit: cmpxchg,
    },
    Template {
        name: "cmpxchg_unequal",
        variants: 6,
        jit: yes,
        emit: cmpxchg_unequal,
    },
    Template {
        name: "xadd",
        variants: 6,
        jit: yes,
        emit: xadd,
    },
    Template {
        name: "signext",
        variants: 6,
        jit: signext_jit,
        emit: signext,
    },
    Template {
        name: "bswap",
        variants: 2,
        jit: yes,
        emit: bswap,
    },
    Template {
        name: "misc",
        variants: 11,
        jit: yes,
        emit: misc,
    },
    Template {
        name: "rep",
        variants: 48,
        jit: yes,
        emit: rep,
    },
    Template {
        name: "gs",
        variants: 25,
        jit: yes,
        emit: gs,
    },
    Template {
        name: "lock",
        variants: 17,
        jit: yes,
        emit: lock,
    },
    Template {
        name: "lock_sub_dword_imm8",
        variants: 1,
        jit: yes,
        emit: lock_sub_dword_imm8,
    },
    Template {
        name: "sreg",
        variants: 18,
        jit: yes,
        emit: sreg,
    },
    Template {
        name: "rdtsc",
        variants: 1,
        jit: yes,
        emit: rdtsc,
    },
];

/// Templates with a known difference (each has an ignored test of its own): kept out of the
/// other tests so that those find new differences.
const KNOWN_MISMATCHES: &[&str] = &["cmpxchg_unequal", "lock_sub_dword_imm8"];

fn template(name: &str) -> &'static Template {
    TEMPLATES
        .iter()
        .find(|template| template.name == name)
        .expect("a template of that name")
}

// ---- programs ------------------------------------------------------------------------------------

/// A producer of all six arithmetic flags (ADD/SUB/CMP of registers or an immediate).
fn producer(asm: &mut Asm, rng: &mut Rng) {
    let (a, b) = two(rng);
    let size = [4u8, 8][rng.below(2) as usize];
    match rng.below(3) {
        0 => asm.op(&[], size, &[0x01], b, Rm::R(a), &[]),
        1 => asm.op(&[], size, &[0x81], 5, Rm::R(a), &imm(4, rng.value())),
        _ => asm.op(&[], size, &[0x39], b, Rm::R(a), &[]),
    }
}

/// Emit `template`'s `variant` at the end of `asm`, after a producer when it would read a flag
/// that is not defined; the definedness after it.
fn emit(asm: &mut Asm, rng: &mut Rng, template: &Template, variant: u64, defined: u64) -> u64 {
    let saved = rng.clone();
    let mut scratch = Asm::new(asm.pc());
    (template.emit)(&mut scratch, rng, variant);
    let mut instructions = decode_all(scratch.at, &scratch.bytes).unwrap_or_else(|error| {
        panic!(
            "{} {variant}: {error}: {}",
            template.name,
            hex(&scratch.bytes)
        )
    });
    let mut defined = defined;
    if !reads_defined(&instructions, defined) {
        producer(asm, rng);
        defined = ARITHMETIC_FLAGS;
        *rng = saved;
        scratch = Asm::new(asm.pc());
        (template.emit)(&mut scratch, rng, variant);
        instructions = decode_all(scratch.at, &scratch.bytes).unwrap();
        assert!(
            reads_defined(&instructions, defined),
            "{} {variant} reads a flag it leaves undefined",
            template.name
        );
    }
    asm.raw(&scratch.bytes);
    defined_after(&instructions, defined)
}

/// Consumers of the flags `defined` holds, so that the flags of the code before them show in
/// registers and memory: SETcc, ADC, PUSHF.
fn consumers(asm: &mut Asm, rng: &mut Rng, defined: u64) {
    let usable: Vec<u8> = (0..16u8)
        .filter(|&cc| cc_reads(cc) & !defined == 0)
        .collect();
    if !usable.is_empty() {
        let cc = rng.pick(&usable);
        asm.op(&[], 1, &[0x0F, 0x90 + cc], 0, Rm::R(free(rng)), &[]);
    }
    if defined & CF != 0 && rng.chance(50) {
        let (a, b) = two(rng);
        asm.op(&[], 8, &[0x11], b, Rm::R(a), &[]);
    }
    if defined == ARITHMETIC_FLAGS && rng.chance(50) {
        asm.raw(&[0x9C]);
        pop_reg(asm, free(rng), false);
    }
}

/// The end of a sequence: HLT, reached directly, by a JMP, by a Jcc (both ways), by a RET to a
/// pushed address or by a CALL.
fn terminator(asm: &mut Asm, rng: &mut Rng, defined: u64) {
    match rng.below(5) {
        0 => {}
        1 => asm.raw(&[0xEB, 1, 0xCC]),
        2 => {
            let usable: Vec<u8> = (0..16u8)
                .filter(|&cc| cc_reads(cc) & !defined == 0)
                .collect();
            if let Some(&cc) = usable.first() {
                let cc = if usable.len() > 1 {
                    rng.pick(&usable)
                } else {
                    cc
                };
                asm.raw(&[0x70 + cc, 1, 0xF4]);
            }
        }
        3 => {
            let mut probe = Asm::new(asm.pc());
            lea_rip(&mut probe, R11, 0);
            let target = probe.pc() + 3;
            lea_rip(asm, R11, target);
            asm.raw(&[0x41, 0x53, 0xC3]);
        }
        _ => {
            asm.raw(&[0xE8]);
            asm.raw(&0i32.to_le_bytes());
        }
    }
    asm.hlt();
}

/// Random register values (RBX, RBP, RSP keep the layout) and arithmetic flags, IF.
fn randomize(machine: &mut Machine, rng: &mut Rng) {
    for &reg in &FREE {
        machine.state.gpr[reg as usize] = rng.value();
    }
    machine.state.rflags =
        2 | (rng.next() & ARITHMETIC_FLAGS) | if rng.chance(50) { IF } else { 0 };
}

/// One template at `CODE` (plus consumers of what it defines), then HLT.
fn template_program(template: &Template, variant: u64, seed: u64) -> Vec<u8> {
    let mut rng = Rng::new(seed);
    let mut asm = Asm::new(CODE);
    let defined = emit(&mut asm, &mut rng, template, variant, ARITHMETIC_FLAGS);
    if rng.chance(70) {
        consumers(&mut asm, &mut rng, defined);
    }
    asm.hlt();
    asm.bytes
}

/// A random straight-line sequence of 20 to 60 templates at `at`, ending in a terminator.
fn sequence(at: u64, seed: u64, pool: &[&Template]) -> Vec<u8> {
    let mut rng = Rng::new(seed);
    let (mut asm, defined) = sequence_body(at, &mut rng, pool);
    terminator(&mut asm, &mut rng, defined);
    asm.bytes
}

/// The templates of [`sequence`] without the terminator; the definedness at their end.
fn sequence_body(at: u64, rng: &mut Rng, pool: &[&Template]) -> (Asm, u64) {
    let mut asm = Asm::new(at);
    let mut defined = ARITHMETIC_FLAGS;
    let count = 20 + rng.below(41);
    for _ in 0..count {
        let template = rng.pick(pool);
        let variant = rng.below(template.variants);
        defined = emit(&mut asm, rng, template, variant, defined);
        if rng.chance(20) {
            consumers(&mut asm, rng, defined);
        }
    }
    (asm, defined)
}

/// Run `program` at linear `rip` (physical `CODE + (rip & 0x7FFF)`) on randomized registers.
fn run_program(
    name: &str,
    seed: u64,
    rip: u64,
    program: &[u8],
    budget: u64,
) -> Result<Outcome, String> {
    let prepare = |machine: &mut Machine| {
        machine.code(CODE + (rip & 0x7FFF), program);
        machine.state.rip = rip;
        randomize(machine, &mut Rng::new(seed ^ 0x5EED));
    };
    Case::new(name, seed, &prepare)
        .program(program)
        .budget(budget)
        .run()
}

// ---- template tests ------------------------------------------------------------------------------

/// Every instruction of a `jit` template variant is one `jit::plan` admits, and a variant declared
/// for the general path really has an instruction the plan refuses: the templates exercise what
/// they claim to.
#[test]
fn every_template_variant_is_admitted_as_declared() {
    let mut wrong = Vec::new();
    for template in TEMPLATES {
        for variant in 0..template.variants {
            for seed in 0..6 {
                let mut rng = Rng::new(seed * 7919 + variant);
                let mut asm = Asm::new(CODE);
                (template.emit)(&mut asm, &mut rng, variant);
                let instructions = match decode_all(CODE, &asm.bytes) {
                    Ok(instructions) => instructions,
                    Err(error) => {
                        wrong.push(format!(
                            "{} {variant}: {error}: {}",
                            template.name,
                            hex(&asm.bytes)
                        ));
                        continue;
                    }
                };
                let refused: Vec<String> = instructions
                    .iter()
                    .filter(|instruction| !super::jit::admitted(instruction))
                    .map(|instruction| format!("{:?}", instruction.operation))
                    .collect();
                let expected = (template.jit)(variant);
                if expected != refused.is_empty() {
                    wrong.push(format!(
                        "{} {variant} (declared jit: {expected}): {} refused {refused:?}",
                        template.name,
                        hex(&asm.bytes)
                    ));
                }
            }
        }
    }
    wrong.dedup();
    assert!(
        wrong.is_empty(),
        "{} wrong declarations:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

/// Run every variant of the named templates with `seeds` seeds each (registers, immediates,
/// addresses and initial flags random), then consumers of the flags they define. Every
/// difference is collected; a `jit` variant must also run in generated code at least once (only
/// the final HLT on the general path).
fn check_templates(names: &[&str], seeds: u64) {
    check_templates_except(names, seeds, &[]);
}

/// [`check_templates`] where the variants in `slow` ("name variant") always take the general path
/// at run time (for example REP STOS with DF = 1).
fn check_templates_except(names: &[&str], seeds: u64, slow: &[&str]) {
    let mut failures = Vec::new();
    let mut never_generated = Vec::new();
    for name in names {
        let template = template(name);
        for variant in 0..template.variants {
            let mut generated = false;
            for seed in 0..seeds {
                let seed = seed * 0x1_0000 + variant * 0x100 + name.len() as u64;
                let program = template_program(template, variant, seed);
                let case = format!("{name} {variant}");
                match run_program(&case, seed, CODE, &program, 100_000) {
                    Ok(outcome) => {
                        // To the final HLT with nothing but it on the general path (an RDTSC
                        // through the engine's helper, not the loop's own path).
                        generated |= outcome.report.exit == "Run(Halted)"
                            && outcome.stats.general_steps <= 1
                            && outcome.stats.retired > 0
                            && (*name != "rdtsc" || outcome.stats.rdtsc_calls > 0);
                    }
                    Err(message) => {
                        failures.push(message);
                        break;
                    }
                }
            }
            let key = format!("{name} {variant}");
            if (template.jit)(variant) && !generated && !slow.contains(&key.as_str()) {
                never_generated.push(format!("{name} {variant}"));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "turbo mismatch in {} variants:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
    assert!(
        never_generated.is_empty(),
        "variants never run in generated code: {never_generated:?}"
    );
}

#[test]
fn alu_register_and_immediate_forms_match_the_general_step() {
    check_templates(&["alu_rr", "alu_ri", "unary", "test"], 4);
}

#[test]
fn alu_memory_forms_match_the_general_step() {
    check_templates(&["alu_rm", "alu_mr", "alu_mi"], 4);
}

#[test]
fn shifts_and_rotates_match_the_general_step() {
    check_templates(&["shift_imm", "shift_cl", "shld"], 4);
}

#[test]
fn bit_tests_scans_multiplies_and_divides_match_the_general_step() {
    check_templates(&["bit", "bitscan", "mul", "divide"], 4);
}

#[test]
fn condition_consumers_match_the_general_step() {
    check_templates(&["cmov", "setcc", "jcc", "jmp"], 4);
}

#[test]
fn stack_and_control_transfers_match_the_general_step() {
    check_templates(&["stack", "lea"], 6);
}

#[test]
fn moves_and_extensions_match_the_general_step() {
    check_templates(&["mov", "movx", "signext", "bswap", "sreg"], 4);
}

#[test]
fn atomic_forms_match_the_general_step() {
    check_templates(&["cmpxchg", "xadd", "lock"], 4);
}

#[test]
fn rep_string_forms_match_the_general_step() {
    // Case 5 of STOS sets DF: REP then takes the general path.
    check_templates_except(&["rep"], 6, &["rep 41", "rep 43", "rep 45", "rep 47"]);
}

#[test]
fn gs_relative_forms_match_the_general_step() {
    check_templates(&["gs"], 6);
}

#[test]
fn flag_and_system_forms_match_the_general_step() {
    check_templates(&["misc", "rdtsc"], 6);
}

#[test]
fn general_path_forms_match_inside_generated_code() {
    check_templates(
        &["shift_mem", "shld_slow", "mul_narrow", "stack_slow", "xchg"],
        4,
    );
}

// ---- random sequences ----------------------------------------------------------------------------

/// Random straight-line sequences of 20 to 60 templates at a random offset of the first code page
/// (they cross into the next pages), every fourth through the high alias; run to the end and with
/// a budget that ends in the middle.
fn check_sequences(pool: &[&Template], seeds: std::ops::Range<u64>) {
    let mut failures = Vec::new();
    for seed in seeds {
        let mut rng = Rng::new(seed ^ 0xC0DE);
        let offset = rng.below(0x1000);
        let rip = if seed % 4 == 3 { HIGH } else { 0 } + CODE + offset;
        let program = sequence(rip, seed, pool);
        let name = format!("sequence {seed}");
        match run_program(&name, seed, rip, &program, 100_000) {
            Ok(outcome) => {
                let total = outcome.retired;
                if total > 2 {
                    let budget = 1 + rng.below(total - 1);
                    if let Err(message) = run_program(
                        &format!("{name} budget {budget}"),
                        seed,
                        rip,
                        &program,
                        budget,
                    ) {
                        failures.push(message);
                    }
                }
            }
            Err(message) => failures.push(message),
        }
        if failures.len() >= 5 {
            break;
        }
    }
    assert!(
        failures.is_empty(),
        "turbo mismatch:\n{}",
        failures.join("\n\n")
    );
}

#[test]
fn random_sequences_of_translated_templates_match_the_general_step() {
    let pool: Vec<&Template> = TEMPLATES
        .iter()
        .filter(|t| t.name != "rep" && !KNOWN_MISMATCHES.contains(&t.name))
        .collect();
    check_sequences(&pool, 0..300);
}

#[test]
fn random_sequences_with_strings_and_general_path_forms_match_the_general_step() {
    let pool: Vec<&Template> = TEMPLATES
        .iter()
        .filter(|t| !KNOWN_MISMATCHES.contains(&t.name))
        .collect();
    check_sequences(&pool, 1000..1200);
}

// ---- scenarios -----------------------------------------------------------------------------------

/// The turbo run executed generated code (a scenario that passes as interpreter against
/// interpreter proves nothing).
#[track_caller]
fn assert_generated(outcome: &Outcome, name: &str) {
    assert!(
        outcome.stats.retired > 0,
        "{name}: no instruction ran in generated code ({:?})",
        outcome.stats
    );
}

/// A scenario: `program` at `CODE` on the base machine, `prepare` for the rest; checked.
fn scenario(name: &str, program: &[u8], prepare: &dyn Fn(&mut Machine)) -> Outcome {
    let setup = |machine: &mut Machine| {
        machine.code(CODE, program);
        prepare(machine);
    };
    Case::new(name, 1, &setup).program(program).check()
}

/// `MOV r, imm64` patched by a store before it runs, in the same block and in the next one; the
/// patched code ran once before (it is translated), then the patch, then it runs again.
#[test]
fn a_store_into_translated_code_of_the_same_and_the_next_block_is_seen() {
    for through_alias in [false, true] {
        for next_block in [false, true] {
            // r15 = 2; again: [jmp next]; next: mov rax, imm64 (the patched instruction);
            // add r14, rax; mov byte [target], r15b (patch the imm64's low byte);
            // dec r15; jnz again; hlt
            let mut asm = Asm::new(CODE);
            asm.mov_imm(R15, 2);
            asm.mov_imm(R14, 0);
            let again = asm.pc();
            if next_block {
                asm.raw(&[0xEB, 0]);
            }
            let patched = asm.pc() + 2;
            asm.mov_imm(RAX, 0x1111_1111_1111_1111);
            asm.op(&[], 8, &[0x01], RAX, Rm::R(R14), &[]);
            let base = if through_alias { HIGH } else { 0 };
            asm.mov_imm(RDI, base + patched);
            asm.op(&[], 1, &[0x88], R15, Rm::M(RDI, 0), &[]);
            asm.op(&[], 8, &[0xFF], 1, Rm::R(R15), &[]);
            let back = again.wrapping_sub(asm.pc() + 6) as i32;
            asm.raw(&[0x0F, 0x85]);
            asm.raw(&back.to_le_bytes());
            asm.hlt();
            let program = asm.bytes.clone();
            let outcome = scenario(
                &format!("smc alias {through_alias} next block {next_block}"),
                &program,
                &|_| {},
            );
            assert_eq!(
                outcome.state.gpr[R14 as usize],
                0x1111_1111_1111_1111u64.wrapping_add(0x1111_1111_1111_1102),
                "the second pass ran the patched immediate"
            );
            assert_generated(&outcome, "smc");
            assert!(outcome.stats.code_invalidations > 0, "{:?}", outcome.stats);
        }
    }
}

/// A REP STOSB over translated code: the second pass runs the NOPs it wrote.
#[test]
fn a_rep_stos_over_translated_code_is_seen() {
    // r15 = 2; again: call region; mov al, 0x90; lea rdi, [rip + region]; mov ecx, 16; rep stosb;
    // dec r15; jnz again; hlt; region: 8 x INC ESI (FF C6); RET
    let mut asm = Asm::new(CODE);
    asm.mov_imm(R15, 2);
    asm.mov_imm(RSI, 0);
    let again = asm.pc();
    let call_at = asm.bytes.len();
    asm.raw(&[0xE8, 0, 0, 0, 0]);
    asm.raw(&[0xB0, 0x90]);
    let lea_at = asm.bytes.len();
    lea_rip(&mut asm, RDI, 0);
    asm.mov_imm32(RCX, 16);
    asm.raw(&[0xF3, 0xAA]);
    asm.op(&[], 8, &[0xFF], 1, Rm::R(R15), &[]);
    let back = again.wrapping_sub(asm.pc() + 6) as i32;
    asm.raw(&[0x0F, 0x85]);
    asm.raw(&back.to_le_bytes());
    asm.hlt();
    let region = asm.pc();
    for _ in 0..8 {
        asm.raw(&[0xFF, 0xC6]);
    }
    asm.raw(&[0xC3]);
    // Patch the CALL and the LEA now that the region's address is known.
    let call_end = CODE + call_at as u64 + 5;
    asm.bytes[call_at + 1..call_at + 5]
        .copy_from_slice(&((region.wrapping_sub(call_end)) as i32).to_le_bytes());
    let lea_end = CODE + lea_at as u64 + 7;
    asm.bytes[lea_at + 3..lea_at + 7]
        .copy_from_slice(&((region.wrapping_sub(lea_end)) as i32).to_le_bytes());
    let outcome = scenario("rep stos over code", &asm.bytes, &|_| {});
    assert_eq!(
        outcome.state.gpr[RSI as usize], 8,
        "only the first pass incremented"
    );
    assert_generated(&outcome, "rep stos over code");
    assert!(
        outcome.stats.rep_calls > 0 && outcome.stats.code_invalidations > 0,
        "{:?}",
        outcome.stats
    );
}

/// A PTE written through the table window, then the remapped data page read and written (it was
/// read before, so a stale translation would show), then the same through a directory entry
/// (the 2 MiB page moved, a new leaf table attached).
#[test]
fn a_page_table_write_is_followed_by_accesses_through_the_new_entry() {
    for invlpg in [false, true] {
        let mut asm = Asm::new(CODE);
        let page = DATA + 0x7000;
        // mov rax, [page]; mov [page + 8], rax (fills the read and write TLBs)
        asm.mov_imm(RDI, page);
        asm.op(&[], 8, &[0x8B], RAX, Rm::M(RDI, 0), &[]);
        asm.op(&[], 8, &[0x89], RAX, Rm::M(RDI, 8), &[]);
        // PT_A entry of `page` := ALT + 0x3000 | P | RW
        asm.mov_imm(RSI, PT_WINDOW + (page >> 12) * 8);
        asm.mov_imm(RDX, (ALT + 0x3000) | P | RW);
        asm.op(&[], 8, &[0x89], RDX, Rm::M(RSI, 0), &[]);
        if invlpg {
            asm.enc(&[], false, false, &[0x0F, 0x01], 7, Rm::M(RDI, 0), &[]);
        }
        asm.op(&[], 8, &[0x8B], RCX, Rm::M(RDI, 0), &[]);
        asm.op(&[], 8, &[0x89], RCX, Rm::M(RDI, 0x10), &[]);
        // The 2 MiB page: read [BIG + DATA] (physical DATA), then PD_A[1] := PT_B (a leaf table:
        // the same linear address now reaches ALT), read and write again.
        asm.mov_imm(R8, BIG + DATA + 0x100);
        asm.op(&[], 8, &[0x8B], R10, Rm::M(R8, 0), &[]);
        asm.mov_imm(RSI, PD_WINDOW + 8);
        asm.mov_imm(RDX, PT_B | P | RW);
        asm.op(&[], 8, &[0x89], RDX, Rm::M(RSI, 0), &[]);
        asm.op(&[], 8, &[0x8B], R11, Rm::M(R8, 0), &[]);
        asm.op(&[], 8, &[0x89], R11, Rm::M(R8, 8), &[]);
        // PD_A[2] := PT_B: linear 0x40_0000 + DATA reaches ALT through space B's leaf table.
        asm.mov_imm(RDX, PT_B | P | RW);
        asm.op(&[], 8, &[0x89], RDX, Rm::M(RSI, 8), &[]);
        asm.mov_imm(R8, 0x40_0000 + DATA + 0x20);
        asm.op(&[], 8, &[0x8B], R13, Rm::M(R8, 0), &[]);
        asm.op(&[], 8, &[0x89], R13, Rm::M(R8, 8), &[]);
        asm.hlt();
        let outcome = scenario(&format!("pte write invlpg {invlpg}"), &asm.bytes, &|_| {});
        assert_eq!(outcome.report.exit, "Run(Halted)");
        assert_generated(&outcome, "pte write");
        assert!(
            outcome.stats.leaf_drops + outcome.stats.flushes > 0,
            "{:?}",
            outcome.stats
        );
    }
}

/// MOV CR3 from space A to B and back, twice (the translation slots of both spaces are reused):
/// the data page reads and writes the frame of the active space.
#[test]
fn mov_cr3_between_two_spaces_and_back_uses_the_frames_of_each() {
    let mut asm = Asm::new(CODE);
    asm.mov_imm(R15, 2);
    let again = asm.pc();
    for (root, register) in [(PML4_B, R8), (PML4_A, R10), (PML4_B, R11)] {
        asm.mov_imm(RAX, root);
        asm.raw(&[0x0F, 0x22, 0xD8]);
        asm.op(&[], 8, &[0x03], register, Rm::M(RBX, 0x40), &[]);
        asm.op(&[], 8, &[0x89], register, Rm::M(RBX, 0x48), &[]);
    }
    asm.mov_imm(RAX, PML4_A);
    asm.raw(&[0x0F, 0x22, 0xD8]);
    asm.op(&[], 8, &[0xFF], 1, Rm::R(R15), &[]);
    let back = again.wrapping_sub(asm.pc() + 6) as i32;
    asm.raw(&[0x0F, 0x85]);
    asm.raw(&back.to_le_bytes());
    asm.hlt();
    let outcome = scenario("mov cr3 A B A", &asm.bytes, &|_| {});
    assert_eq!(outcome.report.exit, "Run(Halted)");
    assert_generated(&outcome, "mov cr3");
    assert!(outcome.stats.slot_switches >= 2, "{:?}", outcome.stats);
    assert_eq!(outcome.state.cr3, Some(PML4_A));
}

/// Between two runs a device writes the code page through `PhysicalMemory` (DMA): the second run
/// executes the new bytes; a data write of the device is read too.
#[test]
fn a_device_write_into_a_code_page_between_two_runs_is_seen() {
    let first = {
        let mut asm = Asm::new(CODE);
        asm.mov_imm(RAX, 0x1111);
        asm.op(&[], 8, &[0x03], RAX, Rm::M(RBX, 0x10), &[]);
        asm.op(&[], 8, &[0x01], RAX, Rm::R(R14), &[]);
        asm.hlt();
        asm.bytes
    };
    let second = {
        let mut asm = Asm::new(CODE);
        asm.mov_imm(RAX, 0x2222);
        asm.op(&[], 8, &[0x03], RAX, Rm::M(RBX, 0x10), &[]);
        asm.op(&[], 8, &[0x29], RAX, Rm::R(R14), &[]);
        asm.hlt();
        asm.bytes
    };
    assert_eq!(first.len(), second.len());
    for partial in [false, true] {
        let (first, second) = (&first, &second);
        let prepare = move |machine: &mut Machine| {
            machine.code(CODE, first);
            machine.state.gpr[R14 as usize] = 0x1_0000;
        };
        let between = move |machine: &mut Machine, _call: usize| {
            if partial {
                // One byte of the immediate only.
                put(&machine.bus, CODE + 2, &[0x33]);
            } else {
                put(&machine.bus, CODE, second);
            }
            put_qword(&machine.bus, DATA_PTR + 0x10, 0x5000);
            machine.state.rip = CODE;
            machine.state.activity = Some(CpuActivity::Running);
        };
        let outcome = Case::new(format!("dma partial {partial}"), 3, &prepare)
            .program(first)
            .between(&between)
            .calls(3)
            .check();
        assert_eq!(outcome.report.exit, "Run(Halted)");
        assert_generated(&outcome, "dma");
        assert!(outcome.stats.code_invalidations > 0, "{:?}", outcome.stats);
    }
}

/// Data accesses that cross a page (into a mapped page, into an unmapped one, a store from a
/// read-only page into a writable one) and instructions that cross a code page (into the next code
/// page, into an unmapped page).
#[test]
fn accesses_and_instructions_crossing_a_page_match_the_general_step() {
    // Crossing data accesses.
    let crossings: [(u64, u8, bool); 8] = [
        (DATA + 0x0FFD, 8, false),
        (DATA + 0x1FFE, 4, true),
        (DATA + 0x7FFC, 8, false),
        (DATA + 0x7FFF, 2, true),
        (READ_ONLY + 0xFFD, 8, true),
        (READ_ONLY + 0xFFD, 8, false),
        (BIG + 0x1FFF, 8, true),
        (BIG + 0x20_0000 - 3, 4, false),
    ];
    for (index, &(address, size, store)) in crossings.iter().enumerate() {
        for with_idt in [false, true] {
            let mut asm = Asm::new(CODE);
            asm.mov_imm(RDI, address);
            asm.op(&[], 8, &[0x8B], RDX, Rm::M(RBX, 0), &[]);
            let opcode = if store { 0x89 } else { 0x8B };
            asm.op(&[], size, &[opcode], RAX, Rm::M(RDI, 0), &[]);
            asm.op(&[], 8, &[0x01], RAX, Rm::R(RDX), &[]);
            asm.hlt();
            let outcome = scenario(
                &format!("crossing data {index} idt {with_idt}"),
                &asm.bytes,
                &|machine| {
                    if with_idt {
                        machine.idt();
                    }
                },
            );
            assert_generated(&outcome, "crossing data");
        }
    }
    // A MOV r64, imm64 that crosses from the first code page into the second, inside a block.
    for split in 1..10u64 {
        let start = CODE + 0x1000 - 3 - split;
        let mut asm = Asm::new(start);
        asm.op(&[], 8, &[0x01], RAX, Rm::R(RDX), &[]);
        asm.mov_imm(RCX, 0x0102_0304_0506_0708);
        asm.op(&[], 8, &[0x01], RCX, Rm::R(RDX), &[]);
        asm.hlt();
        let program = asm.bytes.clone();
        let prepare = |machine: &mut Machine| {
            machine.code(start, &program);
            machine.state.rip = start;
        };
        let outcome = Case::new(format!("crossing code {split}"), 5, &prepare)
            .program(&program)
            .check();
        assert_eq!(outcome.report.exit, "Run(Halted)");
        assert_generated(&outcome, "crossing code");
    }
    // An instruction that runs off the last code page into an unmapped one.
    for with_idt in [false, true] {
        let start = CODE + CODE_PAGES * 0x1000 - 7;
        let mut asm = Asm::new(start);
        asm.op(&[], 8, &[0x01], RAX, Rm::R(RDX), &[]);
        asm.mov_imm(RCX, 0x0102_0304_0506_0708);
        let program = asm.bytes.clone();
        let prepare = |machine: &mut Machine| {
            machine.code(start, &program);
            machine.state.rip = start;
            if with_idt {
                machine.idt();
            }
        };
        let outcome = Case::new(
            format!("crossing into unmapped code idt {with_idt}"),
            6,
            &prepare,
        )
        .program(&program)
        .check();
        assert!(
            outcome.report.exit != "Run(Halted)" || outcome.report.delivered.1 == 1,
            "the fetch faults: {:?}",
            outcome.report
        );
    }
}

/// Loads, stores and read-modify-writes of the MMIO page: the general path serves them, the
/// device sees the same accesses in the same order.
#[test]
fn accesses_to_a_non_ram_region_match_the_general_step() {
    let mut asm = Asm::new(CODE);
    asm.mov_imm(RDI, MMIO_LINEAR);
    asm.op(&[], 4, &[0x8B], RAX, Rm::M(RDI, 0x10), &[]);
    asm.op(&[], 8, &[0x89], RAX, Rm::M(RDI, 0x20), &[]);
    asm.op(&[], 4, &[0x01], RAX, Rm::M(RDI, 0x30), &[]);
    asm.op(&[], 1, &[0x88], RCX, Rm::M(RDI, 0x41), &[]);
    asm.op(&[], 8, &[0x8B], RDX, Rm::M(RDI, 0x20), &[]);
    asm.op(&[], 8, &[0x01], RDX, Rm::R(RAX), &[]);
    asm.op(&[0xF0], 4, &[0xFF], 0, Rm::M(RDI, 0x50), &[]);
    asm.op(&[], 8, &[0x8B], RSI, Rm::M(RBX, 0x10), &[]);
    asm.hlt();
    let outcome = scenario("mmio", &asm.bytes, &|machine| {
        randomize(machine, &mut Rng::new(9));
    });
    assert!(outcome.report.mmio_written || outcome.report.exit != "Run(Halted)");
    assert_generated(&outcome, "mmio");
}

/// The faults: a read of an unmapped page, a write to a read-only page, a non-canonical address
/// (#GP), DIV by zero and IDIV overflow (#DE), in registers and memory; without an IDT (the loop
/// stops with the fault) and with one (#PF and #GP are delivered, the handler halts).
#[test]
fn faults_match_the_general_step() {
    let mut programs: Vec<(String, Vec<u8>)> = Vec::new();
    let mut add = |name: &str, build: &dyn Fn(&mut Asm)| {
        let mut asm = Asm::new(CODE);
        asm.op(&[], 8, &[0x01], R14, Rm::R(R13), &[]);
        build(&mut asm);
        asm.op(&[], 8, &[0x01], R14, Rm::R(R13), &[]);
        asm.hlt();
        programs.push((name.to_owned(), asm.bytes));
    };
    add("read unmapped", &|asm| {
        asm.mov_imm(RDI, UNMAPPED + 0x18);
        asm.op(&[], 8, &[0x8B], RAX, Rm::M(RDI, 0), &[]);
    });
    add("write unmapped", &|asm| {
        asm.mov_imm(RDI, UNMAPPED + 0x18);
        asm.op(&[], 4, &[0x89], RAX, Rm::M(RDI, 0), &[]);
    });
    add("write read-only", &|asm| {
        asm.mov_imm(RDI, READ_ONLY + 0x20);
        asm.op(&[], 8, &[0x89], RAX, Rm::M(RDI, 0), &[]);
    });
    add("rmw read-only", &|asm| {
        asm.mov_imm(RDI, READ_ONLY + 0x20);
        asm.op(&[], 8, &[0x01], RAX, Rm::M(RDI, 0), &[]);
    });
    add("lock read-only", &|asm| {
        asm.mov_imm(RDI, READ_ONLY + 0x20);
        asm.op(&[0xF0], 8, &[0xFF], 0, Rm::M(RDI, 0), &[]);
    });
    add("push unmapped stack", &|asm| {
        asm.mov_imm(RSP, UNMAPPED + 0x100);
        asm.raw(&[0x50]);
    });
    add("non-canonical", &|asm| {
        asm.mov_imm(RDI, 0x8000_0000_0000_0000);
        asm.op(&[], 8, &[0x8B], RAX, Rm::M(RDI, 0), &[]);
    });
    add("jump to unmapped", &|asm| {
        asm.mov_imm(R11, UNMAPPED);
        asm.raw(&[0x41, 0xFF, 0xE3]);
    });
    for (size, signed, memory) in [
        (4u8, false, false),
        (8, false, false),
        (4, false, true),
        (8, true, false),
        (8, true, true),
    ] {
        add(
            &format!("divide by zero {size} signed {signed} memory {memory}"),
            &|asm| {
                asm.mov_imm(RCX, 0);
                asm.mov_imm(RDX, 0);
                let operand = if memory {
                    asm.op(&[], 8, &[0x89], RCX, Rm::M(RBX, 0x30), &[]);
                    Rm::M(RBX, 0x30)
                } else {
                    Rm::R(RCX)
                };
                asm.op(&[], size, &[0xF7], if signed { 7 } else { 6 }, operand, &[]);
            },
        );
    }
    for size in [4u8, 8] {
        add(&format!("idiv overflow {size}"), &|asm| {
            asm.mov_imm(RAX, if size == 8 { 1 << 63 } else { 0x8000_0000 });
            asm.raw(if size == 8 { &[0x48, 0x99] } else { &[0x99] });
            asm.mov_imm(RCX, u64::MAX);
            asm.op(&[], size, &[0xF7], 7, Rm::R(RCX), &[]);
        });
        add(&format!("div overflow {size}"), &|asm| {
            asm.mov_imm(RDX, 5);
            asm.mov_imm(RCX, 3);
            asm.op(&[], size, &[0xF7], 6, Rm::R(RCX), &[]);
        });
    }
    let mut failures = Vec::new();
    for (name, program) in &programs {
        for with_idt in [false, true] {
            let prepare = |machine: &mut Machine| {
                machine.code(CODE, program);
                randomize(machine, &mut Rng::new(name.len() as u64));
                if with_idt {
                    machine.idt();
                }
            };
            match Case::new(format!("{name} idt {with_idt}"), 7, &prepare)
                .program(program)
                .run()
            {
                // The fault happened: the loop stopped before the HLT, or the handler (a HLT)
                // ran after a delivery.
                Ok(outcome) => assert!(
                    outcome.report.exit != "Run(Halted)" || outcome.report.delivered.0 > 0,
                    "{name}: no fault ({:?})",
                    outcome.report
                ),
                Err(message) => failures.push(message),
            }
        }
    }
    assert!(
        failures.is_empty(),
        "turbo mismatch:\n{}",
        failures.join("\n\n")
    );
}

/// A budget that ends inside a translated block, at every instruction of a block of ALU, memory
/// and stack forms.
#[test]
fn every_budget_inside_a_block_matches_the_general_step() {
    let pool: Vec<&Template> = ["alu_rr", "alu_mr", "stack", "mov", "setcc", "lock", "gs"]
        .iter()
        .map(|name| template(name))
        .collect();
    let program = sequence(CODE, 77, &pool);
    let total = run_program("budget probe", 77, CODE, &program, 100_000)
        .unwrap()
        .retired;
    let mut failures = Vec::new();
    for budget in 1..=total {
        if let Err(message) = run_program(&format!("budget {budget}"), 77, CODE, &program, budget) {
            failures.push(message);
            break;
        }
    }
    assert!(
        failures.is_empty(),
        "turbo mismatch:\n{}",
        failures.join("\n\n")
    );
}

/// STI and the shadow of the next instruction: STI; HLT, STI; ADD; HLT, CLI; STI; NOP; STI; ADD;
/// HLT, an RDTSC in the shadow.
#[test]
fn sti_and_its_shadow_match_the_general_step() {
    // (STI in the shadow of STI: `an_sti_in_the_shadow_of_sti_is_refused_by_the_general_step`.)
    let programs: [&[u8]; 4] = [
        &[0xFB, 0xF4],
        &[0xFB, 0x48, 0x01, 0xD0, 0x48, 0x01, 0xD0, 0xF4],
        &[0xFA, 0xFB, 0x90, 0xFB, 0x48, 0x01, 0xD0, 0xF4],
        &[0x48, 0x01, 0xD0, 0xFB, 0x0F, 0x31, 0x48, 0x01, 0xD0, 0xF4],
    ];
    for (index, program) in programs.iter().enumerate() {
        for interrupts in [false, true] {
            let outcome = scenario(
                &format!("sti {index} if {interrupts}"),
                program,
                &|machine| {
                    if interrupts {
                        machine.state.rflags |= IF;
                    }
                },
            );
            assert_eq!(outcome.report.exit, "Run(Halted)");
            assert_generated(&outcome, "sti");
        }
    }
}

/// DF = 1 from the start (REP takes the general path; other forms ignore it).
#[test]
fn a_set_direction_flag_matches_the_general_step() {
    for seed in 0..20 {
        let pool: Vec<&Template> = ["rep", "alu_rr", "mov", "stack"]
            .iter()
            .map(|name| template(name))
            .collect();
        let program = sequence(CODE, 500 + seed, &pool);
        let prepare = |machine: &mut Machine| {
            machine.code(CODE, &program);
            randomize(machine, &mut Rng::new(seed));
            machine.state.rflags |= DF;
        };
        Case::new(format!("df sequence {seed}"), seed, &prepare)
            .program(&program)
            .check();
    }
}

/// External interrupts through the board's hook: pending from the start with IF = 1 (delivered
/// at the first boundary), and pending while IF = 0 through translated code until STI (delivered
/// after the instruction in the STI shadow).
#[test]
fn an_external_interrupt_is_delivered_at_the_same_boundary() {
    let pool: Vec<&Template> = [
        "alu_rr",
        "alu_mr",
        "mov",
        "stack",
        "setcc",
        "shift_imm",
        "gs",
    ]
    .iter()
    .map(|name| template(name))
    .collect();
    for seed in 0..12u64 {
        // Templates, then STI, two instructions, HLT.
        let (body, _) = sequence_body(CODE, &mut Rng::new(900 + seed), &pool);
        let mut program = body.bytes;
        program.extend([0xFB, 0x48, 0x01, 0xD0, 0x48, 0x01, 0xD0, 0xF4]);
        let early = seed % 3 == 0;
        let prepare = |machine: &mut Machine| {
            machine.code(CODE, &program);
            randomize(machine, &mut Rng::new(seed));
            machine.idt();
            machine.state.rflags &= !IF;
            if early {
                machine.state.rflags |= IF;
            }
            machine.irq = Some(TestIrq {
                source: TestSource {
                    pending: Some(0x30),
                    acknowledged: Vec::new(),
                },
                counters: Default::default(),
            });
        };
        let outcome = Case::new(format!("irq {seed} early {early}"), seed, &prepare)
            .program(&program)
            .check();
        assert_eq!(outcome.report.exit, "Run(Halted)");
        assert_eq!(outcome.acknowledged, [0x30], "the interrupt was delivered");
        if !early {
            assert_generated(&outcome, "irq");
        }
    }
}

/// A random sequence on a new engine and on one reset from the pool: both agree with the general
/// step, and the engine did the same work (a gap in `TurboEngine::reset` would show).
#[test]
fn a_new_engine_and_a_reset_one_run_alike() {
    let pool: Vec<&Template> = TEMPLATES
        .iter()
        .filter(|t| !KNOWN_MISMATCHES.contains(&t.name))
        .collect();
    for seed in 0..6u64 {
        let program = sequence(CODE, 3000 + seed, &pool);
        let prepare = |machine: &mut Machine| {
            machine.code(CODE, &program);
            randomize(machine, &mut Rng::new(seed));
        };
        // Leave state of another program in the pooled engine first.
        let other = sequence(CODE, 4000 + seed, &pool);
        let prepare_other = |machine: &mut Machine| {
            machine.code(CODE, &other);
            randomize(machine, &mut Rng::new(seed + 1));
        };
        Case::new("warm-up", seed, &prepare_other)
            .program(&other)
            .check();
        let fresh = Case::new(format!("fresh {seed}"), seed, &prepare)
            .program(&program)
            .fresh()
            .check();
        let reused = Case::new(format!("reused {seed}"), seed, &prepare)
            .program(&program)
            .check();
        assert_eq!(fresh.report, reused.report);
        assert_eq!(
            format!("{:?}", fresh.stats),
            format!("{:?}", reused.stats),
            "the same work on a new and on a reset engine"
        );
    }
}

/// Random sequences without an interrupt hook (the loop then knows the source's answer from the
/// first boundary and runs whole blocks at once).
#[test]
fn random_sequences_without_an_interrupt_hook_match_the_general_step() {
    let pool: Vec<&Template> = TEMPLATES
        .iter()
        .filter(|t| !KNOWN_MISMATCHES.contains(&t.name))
        .collect();
    for seed in 0..60u64 {
        let program = sequence(CODE, 5000 + seed, &pool);
        let prepare = |machine: &mut Machine| {
            machine.code(CODE, &program);
            randomize(machine, &mut Rng::new(seed));
            machine.irq = None;
        };
        let outcome = Case::new(format!("no hook {seed}"), seed, &prepare)
            .program(&program)
            .check();
        assert_generated(&outcome, "no hook");
    }
}

/// The comparison itself: a difference in a register, a defined flag, RAM, the MMIO device or the
/// report is found; a difference in an undefined flag is not.
#[test]
fn the_comparison_finds_a_difference_in_every_compared_part() {
    let report = |machine: &mut Machine| view(&run_reference(machine, 10));
    let pair = || {
        let mut a = Machine::new(1);
        let mut b = Machine::new(1);
        a.code(CODE, &[0xF4]);
        b.code(CODE, &[0xF4]);
        (a, b)
    };
    let (mut a, mut b) = pair();
    let (left, right) = (report(&mut a), report(&mut b));
    assert_eq!(difference(&a, &b, &left, &right), None);
    b.state.gpr[R13 as usize] ^= 1;
    assert!(difference(&a, &b, &left, &right).is_some(), "a register");
    let (mut a, mut b) = pair();
    let (left, right) = (report(&mut a), report(&mut b));
    b.state.rflags ^= CF;
    assert!(
        difference(&a, &b, &left, &right).is_some(),
        "a defined flag"
    );
    let (mut a, mut b) = pair();
    let (mut left, mut right) = (report(&mut a), report(&mut b));
    left.rflags_defined &= !CF;
    right.rflags_defined &= !CF;
    b.state.rflags ^= CF;
    assert_eq!(
        difference(&a, &b, &left, &right),
        None,
        "an undefined flag is no difference"
    );
    let (mut a, mut b) = pair();
    let (left, right) = (report(&mut a), report(&mut b));
    put(&b.bus, PT_A + 0x20 * 8, &[0x67]);
    assert!(
        difference(&a, &b, &left, &right).is_some(),
        "an accessed/dirty bit"
    );
    let (mut a, mut b) = pair();
    let (left, right) = (report(&mut a), report(&mut b));
    b.mmio.lock().unwrap().log.push((true, 0, 1, 0));
    assert!(
        difference(&a, &b, &left, &right).is_some(),
        "the MMIO device"
    );
    let (mut a, mut b) = pair();
    let left = report(&mut a);
    let right = ReportView {
        instructions_executed: left.instructions_executed + 1,
        ..report(&mut b)
    };
    assert!(
        difference(&a, &b, &left, &right).is_some(),
        "the retired count"
    );
    let (mut a, mut b) = pair();
    let left = report(&mut a);
    let right = ReportView {
        rflags_defined: left.rflags_defined & !OF,
        ..report(&mut b)
    };
    assert!(
        difference(&a, &b, &left, &right).is_some(),
        "the definedness masks"
    );
    let (mut a, mut b) = pair();
    let (left, right) = (report(&mut a), report(&mut b));
    b.state.rflags ^= IF;
    assert!(difference(&a, &b, &left, &right).is_some(), "IF");
    let (mut a, mut b) = pair();
    let (left, right) = (report(&mut a), report(&mut b));
    b.rdtsc += 1000;
    assert!(
        difference(&a, &b, &left, &right).is_some(),
        "the RDTSC reads"
    );
    let (mut a, mut b) = pair();
    let (left, right) = (report(&mut a), report(&mut b));
    if let Some(irq) = b.irq.as_mut() {
        irq.source.acknowledged.push(0x30);
    }
    assert!(
        difference(&a, &b, &left, &right).is_some(),
        "the interrupt acknowledgements"
    );
}

// ---- known mismatches ----------------------------------------------------------------------------

/// CMPXCHG whose accumulator differs from memory must load the old memory value into RAX/EAX
/// (ZF = 0). The turbo engine stores RAX - [mem] instead (in every form: LOCK, none, DS; 32 and
/// 64 bits).
#[test]
#[ignore = "turbo mismatch: CMPXCHG with RAX != [mem] loads RAX - [mem] into RAX"]
fn cmpxchg_with_an_unequal_accumulator_loads_the_old_memory_value() {
    // cmpxchg [rbx + 0x40], rcx; hlt
    let program = [0x48, 0x0F, 0xB1, 0x4B, 0x40, 0xF4];
    let outcome = scenario("cmpxchg unequal", &program, &|machine| {
        machine.state.gpr[RAX as usize] = 0x1111;
        machine.state.gpr[RCX as usize] = 0x7777;
        put_qword(&machine.bus, DATA_PTR + 0x40, 0x3333);
    });
    assert_eq!(outcome.state.gpr[RAX as usize], 0x3333);
    check_templates(&["cmpxchg_unequal"], 4);
}

/// `LOCK SUB m32, imm8` (`F0 83 /5 ib` without REX.W, the decoder's `LockSubDwordImm8`, a CPL3
/// form): `jit::plan` admits it, the general CPL0 step refuses it (`Unsupported { opcode: 0x83 }`)
/// and the loop stops there; the turbo engine executes it and goes on.
#[test]
#[ignore = "turbo mismatch: LOCK SUB m32, imm8 runs in turbo, the general CPL0 step refuses it"]
fn lock_sub_dword_imm8_is_refused_by_the_general_step() {
    // lock sub dword [rbx + 0x40], 5; hlt
    let program = [0xF0, 0x83, 0x6B, 0x40, 0x05, 0xF4];
    scenario("lock sub m32, imm8", &program, &|_| {});
}

/// STI in the shadow of STI: the general step refuses the second STI (`Unsupported { opcode:
/// 0xFB }`), the turbo engine executes it (one instruction in the shadow) and goes on.
#[test]
#[ignore = "turbo mismatch: STI in the shadow of STI runs in turbo, the general step refuses it"]
fn an_sti_in_the_shadow_of_sti_is_refused_by_the_general_step() {
    scenario(
        "sti sti",
        &[0xFA, 0xFB, 0xFB, 0x48, 0x01, 0xD0, 0xF4],
        &|_| {},
    );
}
