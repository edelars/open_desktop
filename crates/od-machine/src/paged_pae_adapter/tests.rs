//! Tests of [`PagedPaeMemoryAdapter`] (S2-25l). There is no hardware oracle for PAE paging (the Oracle
//! machine's guests run in long mode): the adapter is checked against the `od-isa` model
//! (`ProtectedCpu`, which walks data through `translate_pae`) on identical tables, against literal
//! page-fault error codes of the SDM (Vol. 3A section 5.7), and by the order of the effects of the
//! 4-level tracked access of this crate.

use super::*;
use crate::ap_legacy_exec::BoardRam;
use crate::page_walk::{MmioDispatchError, MmioWindow};
use crate::{
    BusDevice, DeviceAccess, DeviceError, GuestPhysicalAddress, Long64ProbeConfig,
    MachineMemoryMap, MemoryAccessError, MemoryRegionKind, MemoryRegionSpec, MmioBinding,
    PageFaultReason, PagedProbeExit, PagedProbeRejection, ProbeConfigError, RamAccessOperation,
    RamLimits,
};
use od_isa::LongModeState;
use od_isa::legacy_mode::ProtectedSegment;
use od_isa::memory::{InstructionMode, PrivilegeLevel};
use od_isa::protected_exec::{ProtectedCpu, ProtectedRefusal};
use od_isa::real_mode_exec::TableRegister;
use std::cell::{Cell, RefCell};
use std::sync::{Arc, Mutex};

const P: u64 = 1;
const RW: u64 = 1 << 1;
const US: u64 = 1 << 2;
const ACCESSED: u64 = 1 << 5;
const DIRTY: u64 = 1 << 6;
const PS: u64 = 1 << 7;
const XD: u64 = 1 << 63;
const EFER_NXE: u64 = 1 << 11;
/// `CR0` of a protected-mode PAE vCPU: PG, AM, WP, NE, ET, MP, PE.
const CR0_WP_SET: u32 = 0x8005_0033;
const CR0_WP_CLEAR: u32 = 0x8004_0033;

const PDPT: u64 = 0x1000;
const PD: u64 = 0x2000;
const PT: u64 = 0x3000;
/// The code page: linear = physical.
const CODE: u32 = 0x8000;
/// The physical pages behind linear pages 5, 6 and 7 (not contiguous: a crossing operand shows it).
const DATA_A: u64 = 0x1_0000;
const DATA_B: u64 = 0x1_8000;
const DATA_X: u64 = 0x1_1000;
const RAM_END: u64 = 0x2_0000;
const ROM_AT: u64 = 0x2_0000;
const MMIO_AT: u64 = 0x3_0000;
/// The framebuffer region (one page), behind PT[12].
const FB_AT: u64 = 0x5_0000;
/// An address no region covers.
const HOLE: u64 = 0x4_0000;
const EAX_VALUE: u32 = 0xA1B2_C3D4;

fn at(address: u64) -> GuestPhysicalAddress {
    GuestPhysicalAddress::new(address)
}

fn range(start: u64, len: u64) -> GuestPhysicalRange {
    GuestPhysicalRange::new(at(start), len).unwrap()
}

fn map() -> MachineMemoryMap {
    MachineMemoryMap::new(
        36,
        vec![
            MemoryRegionSpec::new(range(0, RAM_END), MemoryRegionKind::Ram),
            MemoryRegionSpec::new(range(ROM_AT, 0x1000), MemoryRegionKind::Rom),
            MemoryRegionSpec::new(range(MMIO_AT, 0x1000), MemoryRegionKind::Mmio),
        ],
    )
    .unwrap()
}

/// The map of `map()` plus a framebuffer page at `FB_AT` (a `MachineBus` needs a display device for
/// such a region, so only the tests that use `PhysicalMemory` directly have it).
fn memory_with_framebuffer() -> PhysicalMemory {
    let mut regions = vec![
        MemoryRegionSpec::new(range(0, RAM_END), MemoryRegionKind::Ram),
        MemoryRegionSpec::new(range(ROM_AT, 0x1000), MemoryRegionKind::Rom),
        MemoryRegionSpec::new(range(MMIO_AT, 0x1000), MemoryRegionKind::Mmio),
    ];
    regions.push(MemoryRegionSpec::new(
        range(FB_AT, 0x1000),
        MemoryRegionKind::Framebuffer,
    ));
    PhysicalMemory::new(
        &MachineMemoryMap::new(36, regions).unwrap(),
        RamLimits::new(1 << 20),
    )
    .unwrap()
}

fn memory() -> PhysicalMemory {
    PhysicalMemory::new(&map(), RamLimits::new(1 << 20)).unwrap()
}

fn put(memory: &PhysicalMemory, address: u64, value: u64) {
    memory
        .store_bytes(at(address), &value.to_le_bytes(), |_| {})
        .unwrap();
}

fn get(memory: &PhysicalMemory, address: u64) -> u64 {
    let mut bytes = [0u8; 8];
    memory.load_bytes(at(address), &mut bytes).unwrap();
    u64::from_le_bytes(bytes)
}

/// The first differing byte of two images (offset, left, right): a readable failure instead of a dump.
fn first_difference(left: &[u8], right: &[u8]) -> Option<(usize, u8, u8)> {
    assert_eq!(left.len(), right.len());
    left.iter()
        .zip(right)
        .position(|(a, b)| a != b)
        .map(|index| (index, left[index], right[index]))
}

fn dump(memory: &PhysicalMemory) -> Vec<u8> {
    let mut image = vec![0u8; RAM_END as usize];
    memory.load_bytes(at(0), &mut image).unwrap();
    image
}

/// One case of the differential: an operand access at `disp` on tables whose entries for linear pages 5
/// and 6 and the 2 MiB PDE are the given ones.
#[derive(Clone)]
struct Scenario {
    name: &'static str,
    store: bool,
    disp: u32,
    pte5: u64,
    pte6: u64,
    pd1: u64,
    wp: bool,
    expect: Expect,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Expect {
    Ok,
    /// A `#PF` with the error code of the SDM and the CR2 value.
    Fault(u32, u32),
    /// A refusal that is no guest fault (memory that is not there, a device the access cannot reach).
    Refused,
}

impl Scenario {
    fn new(name: &'static str, store: bool, disp: u32, expect: Expect) -> Self {
        Self {
            name,
            store,
            disp,
            pte5: DATA_A | P | RW | US,
            pte6: DATA_B | P | RW | US,
            pd1: P | RW | US | PS,
            wp: true,
            expect,
        }
    }
    fn pte5(mut self, entry: u64) -> Self {
        self.pte5 = entry;
        self
    }
    fn pte6(mut self, entry: u64) -> Self {
        self.pte6 = entry;
        self
    }
    fn pd1(mut self, entry: u64) -> Self {
        self.pd1 = entry;
        self
    }
    fn wp_clear(mut self) -> Self {
        self.wp = false;
        self
    }
}

/// PDPTE0 and PDPTE1 -> PD (PDPTE2 and 3 absent); PD[0] -> PT; PD[1] = `pd1` (a 2 MiB page at 0);
/// PT[5], PT[6] as given, PT[7] an XD page over `DATA_X`, PT[8] the code page, PT[9] an MMIO page,
/// PT[10] a ROM page, PT[11] a page of the hole, PT[12] a framebuffer page.
fn fixture(memory: &PhysicalMemory, s: &Scenario) {
    for base in [DATA_A, DATA_B, DATA_X] {
        let pattern: Vec<u8> = (0..0x1000u32)
            .map(|i| (i.wrapping_mul(7).wrapping_add(base as u32 >> 12)) as u8)
            .collect();
        memory.store_bytes(at(base), &pattern, |_| {}).unwrap();
    }
    put(memory, PDPT, PD | P);
    put(memory, PDPT + 8, PD | P);
    put(memory, PD, PT | P | RW | US);
    put(memory, PD + 8, s.pd1);
    let entries = [
        (5, s.pte5),
        (6, s.pte6),
        (7, DATA_X | P | RW | US | XD),
        (8, u64::from(CODE) | P | RW | US),
        (9, MMIO_AT | P | RW | US),
        (10, ROM_AT | P | RW | US),
        (11, HOLE | P | RW | US),
        (12, FB_AT | P | RW | US),
    ];
    for (page, entry) in entries {
        put(memory, PT + 8 * page, entry);
    }
}

fn segment(selector: u16, access: u8) -> ProtectedSegment {
    ProtectedSegment {
        selector,
        base: 0,
        limit: 0xFFFF_FFFF,
        access,
        flags: 0x0C,
    }
}

/// A protected-mode vCPU on PAE paging (the PDPTE registers as the fixture's tables give them) at
/// the code page.
fn cpu(wp: bool) -> ProtectedCpu {
    ProtectedCpu {
        gpr: [0; 8],
        eip: CODE,
        rip_high: 0,
        target_entered: false,
        eflags: 2,
        cr0: if wp { CR0_WP_SET } else { CR0_WP_CLEAR },
        cr3: PDPT as u32,
        cr4: 0x20,
        efer: EFER_NXE,
        pdptes: [PD | P, PD | P, 0, 0],
        pse36: false,
        cs: segment(0x08, 0x9B),
        ds: segment(0x10, 0x93),
        es: segment(0x10, 0x93),
        ss: segment(0x10, 0x93),
        fs: segment(0x10, 0x93),
        gs: segment(0x10, 0x93),
        gdtr: TableRegister { base: 0, limit: 0 },
        idtr: TableRegister { base: 0, limit: 0 },
        mov_ss_shadow: false,
    }
}

fn ctx(privilege: PrivilegeLevel) -> AccessContext {
    AccessContext {
        mode: InstructionMode::Long64,
        privilege,
        origin: AccessOrigin::Explicit,
    }
}

fn ring0() -> AccessContext {
    ctx(PrivilegeLevel::Ring0)
}

#[derive(Debug, PartialEq)]
struct Observed {
    result: Expect,
    /// The loaded value (a load that succeeded).
    value: Option<u32>,
    ram: Vec<u8>,
}

/// The instruction at the code page: `MOV EAX, DS:disp32` (`a1`) or `MOV DS:disp32, EAX` (`a3`).
fn install_code(memory: &PhysicalMemory, s: &Scenario) {
    let mut code = vec![if s.store { 0xA3 } else { 0xA1 }];
    code.extend(s.disp.to_le_bytes());
    memory
        .store_bytes(at(u64::from(CODE)), &code, |_| {})
        .unwrap();
}

/// `MOV EAX, DS:disp32` (`a1`) or `MOV DS:disp32, EAX` (`a3`) on the `od-isa` model.
fn run_model(s: &Scenario) -> Observed {
    let memory = memory();
    fixture(&memory, s);
    install_code(&memory, s);
    let mut model = cpu(s.wp);
    model.gpr[0] = EAX_VALUE;
    let mut ram = BoardRam::new(&memory);
    let result = match model.step(&mut ram) {
        Ok(_) => Expect::Ok,
        Err(refusal @ ProtectedRefusal::PageFault { linear, .. }) => {
            match model.page_fault_error_code(&refusal) {
                Some(code) => Expect::Fault(code, linear),
                None => Expect::Refused,
            }
        }
        Err(_) => Expect::Refused,
    };
    Observed {
        result,
        value: (!s.store && result == Expect::Ok).then_some(model.gpr[0]),
        ram: dump(&memory),
    }
}

/// The same operand through [`PagedPaeMemoryAdapter`]: the five instruction bytes are fetched first
/// (the model walks them too), then the load or the store.
fn run_adapter(s: &Scenario) -> (Observed, Vec<(u64, u64)>) {
    let memory = memory();
    fixture(&memory, s);
    install_code(&memory, s);
    let paging = PaeProbeConfig::from_protected_cpu(&cpu(s.wp)).unwrap();
    let mut writes = Vec::new();
    let mut sink = |range: GuestPhysicalRange| writes.push((range.start().get(), range.len()));
    let mut adapter =
        PagedPaeMemoryAdapter::over_memory(&memory, paging, u64::from(CODE), &mut sink);
    for offset in 0..5 {
        adapter.fetch(u64::from(CODE) + offset, ring0()).unwrap();
    }
    let mut buffer = [0u8; 4];
    let result = if s.store {
        adapter.write(u64::from(s.disp), ring0(), &EAX_VALUE.to_le_bytes())
    } else {
        adapter.read(u64::from(s.disp), ring0(), &mut buffer)
    };
    let expect = match result {
        Ok(()) => Expect::Ok,
        Err(MemoryError::Architectural(fault)) => {
            assert_eq!(fault.vector, 14, "{}", s.name);
            let record = adapter.page_fault().expect("a recorded page fault");
            assert_eq!(record.saved_rip(), u64::from(CODE), "{}", s.name);
            assert_eq!(
                record.cr2_value(),
                fault.linear_address.unwrap(),
                "{}",
                s.name
            );
            Expect::Fault(
                fault.error_code.unwrap(),
                fault.linear_address.unwrap() as u32,
            )
        }
        Err(MemoryError::Backend(_)) => {
            assert!(adapter.memory_error().is_some(), "{}", s.name);
            Expect::Refused
        }
        Err(other) => panic!("{}: unexpected {other:?}", s.name),
    };
    drop(adapter);
    (
        Observed {
            result: expect,
            value: (!s.store && expect == Expect::Ok).then(|| u32::from_le_bytes(buffer)),
            ram: dump(&memory),
        },
        writes,
    )
}

fn scenarios() -> Vec<Scenario> {
    use Expect::{Fault, Ok as Pass, Refused};
    let not_present = DATA_A | RW | US;
    let read_only = DATA_A | P | US;
    let reserved = DATA_A | P | RW | US | 1 << 36;
    vec![
        Scenario::new("load a read-write page", false, 0x5010, Pass),
        Scenario::new("store to a read-write page", true, 0x5010, Pass),
        Scenario::new("load, entry with A and D preset", false, 0x5010, Pass)
            .pte5(DATA_A | P | RW | US | ACCESSED | DIRTY),
        Scenario::new("store, entry with A and D preset", true, 0x5010, Pass)
            .pte5(DATA_A | P | RW | US | ACCESSED | DIRTY),
        Scenario::new("load, page not present", false, 0x5010, Fault(0x0, 0x5010))
            .pte5(not_present),
        Scenario::new("store, page not present", true, 0x5010, Fault(0x2, 0x5010))
            .pte5(not_present),
        Scenario::new(
            "store to a read-only page (WP = 1)",
            true,
            0x5010,
            Fault(0x3, 0x5010),
        )
        .pte5(read_only),
        Scenario::new("load from a read-only page", false, 0x5010, Pass).pte5(read_only),
        Scenario::new("store to a read-only page (WP = 0)", true, 0x5010, Pass)
            .pte5(read_only)
            .wp_clear(),
        Scenario::new("load, reserved bit 36", false, 0x5010, Fault(0x9, 0x5010)).pte5(reserved),
        Scenario::new("store, reserved bit 36", true, 0x5010, Fault(0xB, 0x5010)).pte5(reserved),
        Scenario::new(
            "load from an XD page (data is not fetched)",
            false,
            0x7010,
            Pass,
        ),
        Scenario::new("store to an XD page", true, 0x7010, Pass),
        Scenario::new("store through a 2 MiB page", true, 0x20_5010, Pass),
        Scenario::new("load through a 2 MiB page", false, 0x20_5010, Pass),
        Scenario::new(
            "store to a read-only 2 MiB page",
            true,
            0x20_5010,
            Fault(0x3, 0x20_5010),
        )
        .pd1(P | US | PS),
        Scenario::new("load through PDPTE 1", false, 0x4000_5010, Pass),
        Scenario::new("store through PDPTE 1", true, 0x4000_5010, Pass),
        Scenario::new(
            "load through an absent PDPTE 2",
            false,
            0x8000_0010,
            Fault(0x0, 0x8000_0010),
        ),
        Scenario::new(
            "store through an absent PDPTE 3",
            true,
            0xC000_0010,
            Fault(0x2, 0xC000_0010),
        ),
        Scenario::new("crossing load, both pages present", false, 0x5FFE, Pass),
        Scenario::new("crossing store, both pages present", true, 0x5FFE, Pass),
        Scenario::new(
            "crossing load, second page absent",
            false,
            0x5FFE,
            Fault(0x0, 0x6000),
        )
        .pte6(DATA_B | RW | US),
        Scenario::new(
            "crossing store, second page absent",
            true,
            0x5FFE,
            Fault(0x2, 0x6000),
        )
        .pte6(DATA_B | RW | US),
        Scenario::new(
            "crossing store, second page read-only",
            true,
            0x5FFE,
            Fault(0x3, 0x6000),
        )
        .pte6(DATA_B | P | US),
        Scenario::new(
            "crossing load, first page absent",
            false,
            0x5FFE,
            Fault(0x0, 0x5FFE),
        )
        .pte5(not_present),
        Scenario::new("load from a ROM page", false, 0xA010, Refused),
        Scenario::new("store to a ROM page", true, 0xA010, Refused),
        Scenario::new("load from a hole", false, 0xB010, Refused),
        Scenario::new(
            "store to an MMIO page without a dispatch",
            true,
            0x9010,
            Refused,
        ),
        Scenario::new(
            "load from an MMIO page without a dispatch",
            false,
            0x9010,
            Refused,
        ),
    ]
}

#[test]
fn the_adapter_equals_the_od_isa_protected_model_on_identical_pae_tables() {
    let pristine = {
        let memory = memory();
        fixture(&memory, &Scenario::new("", false, 0, Expect::Ok));
        dump(&memory)
    };
    let mut compared = 0;
    for scenario in scenarios() {
        let model = run_model(&scenario);
        let (adapter, _) = run_adapter(&scenario);
        // The literal expectation (the SDM error codes) holds for both: a defect shared by both would
        // otherwise pass the comparison.
        assert_eq!(model.result, scenario.expect, "{}: model", scenario.name);
        assert_eq!(
            adapter.result, scenario.expect,
            "{}: adapter",
            scenario.name
        );
        assert_eq!(
            adapter.value, model.value,
            "{}: loaded value",
            scenario.name
        );
        if scenario.expect == Expect::Ok {
            // The data bytes and every accessed and dirty bit of every entry the access used.
            assert_eq!(
                first_difference(&adapter.ram, &model.ram),
                None,
                "{}: memory image (offset, adapter, model)",
                scenario.name
            );
            compared += 1;
        } else {
            // A refused access changes no data byte (the walks may have set accessed bits: the
            // walk sets them as it reaches an entry, the model after a whole walk).
            for (image, who) in [(&adapter.ram, "adapter"), (&model.ram, "model")] {
                for base in [DATA_A, DATA_B, DATA_X] {
                    let range = base as usize..base as usize + 0x1000;
                    assert_eq!(
                        image[range.clone()],
                        pristine[range],
                        "{}: {who} changed data",
                        scenario.name
                    );
                }
            }
        }
    }
    assert_eq!(compared, 14, "the scenarios that succeed");
}

#[test]
fn a_crossing_operand_reads_the_bytes_of_both_pages_and_a_store_lands_in_both() {
    let scenario = Scenario::new("", false, 0x5FFE, Expect::Ok);
    let memory = memory();
    fixture(&memory, &scenario);
    let paging = PaeProbeConfig::from_protected_cpu(&cpu(true)).unwrap();
    let mut writes = Vec::new();
    let mut sink = |range: GuestPhysicalRange| writes.push((range.start().get(), range.len()));
    let mut adapter = PagedPaeMemoryAdapter::over_memory(&memory, paging, 0x77, &mut sink);
    let mut bytes = [0u8; 8];
    adapter.read(0x5FFC, ring0(), &mut bytes).unwrap();
    let mut expected = [0u8; 8];
    memory
        .load_bytes(at(DATA_A + 0xFFC), &mut expected[..4])
        .unwrap();
    memory.load_bytes(at(DATA_B), &mut expected[4..]).unwrap();
    assert_eq!(
        bytes, expected,
        "qword at 0x5FFC: 4 bytes of the first frame, 4 of the second"
    );
    adapter
        .write(0x5FFE, ring0(), &0x0403_0201u32.to_le_bytes())
        .unwrap();
    drop(adapter);
    assert_eq!(get(&memory, DATA_A + 0xFF8) >> 48, 0x0201);
    assert_eq!(get(&memory, DATA_B) & 0xFFFF, 0x0403);
    // Both leaves have A and D now.
    for page in [5, 6] {
        assert_eq!(
            get(&memory, PT + 8 * page) & (ACCESSED | DIRTY),
            ACCESSED | DIRTY
        );
    }
}

#[test]
fn walk_effects_come_in_the_order_of_the_four_level_access() {
    let s = Scenario::new("", false, 0, Expect::Ok);
    let memory = memory();
    fixture(&memory, &s);
    let paging = PaeProbeConfig::from_protected_cpu(&cpu(true)).unwrap();
    let mut writes = Vec::new();
    let mut sink = |range: GuestPhysicalRange| writes.push((range.start().get(), range.len()));
    let mut adapter = PagedPaeMemoryAdapter::over_memory(&memory, paging, 0x77, &mut sink);
    let mut dword = [0u8; 4];
    adapter.read(0x5010, ring0(), &mut dword).unwrap();
    assert_eq!(
        get(&memory, PT + 40) & (ACCESSED | DIRTY),
        ACCESSED,
        "a read: A, never D"
    );
    assert_eq!(get(&memory, PD) & ACCESSED, ACCESSED);
    assert_eq!(get(&memory, PDPT), PD | P, "a PDPTE has no accessed bit");
    adapter.write(0x5010, ring0(), &[1, 2, 3, 4]).unwrap();
    drop(adapter);
    assert_eq!(get(&memory, PT + 40) & (ACCESSED | DIRTY), ACCESSED | DIRTY);
    // The PDE A, the PTE A (the read), then the PTE D, then the operand bytes (the write).
    assert_eq!(
        writes,
        [(PD, 8), (PT + 40, 8), (PT + 40, 8), (DATA_A + 0x10, 4)]
    );
}

/// The device behind the set-loop dispatch of the tests: it records every call and, at a write,
/// whether the dirty bit of the PTE of linear page 9 was already set.
struct FakeMmio<'m> {
    memory: &'m PhysicalMemory,
    windows: Vec<MmioWindow>,
    calls: RefCell<Vec<(GuestPhysicalAddress, u8, RamAccessOperation)>>,
    dirty_at_call: Cell<Option<bool>>,
    fail: bool,
}

impl<'m> FakeMmio<'m> {
    fn new(memory: &'m PhysicalMemory, windows: Vec<MmioWindow>) -> Self {
        Self {
            memory,
            windows,
            calls: RefCell::new(Vec::new()),
            dirty_at_call: Cell::new(None),
            fail: false,
        }
    }
}

impl crate::bus::SetStepMmio for FakeMmio<'_> {
    fn windows(&self) -> &[MmioWindow] {
        &self.windows
    }

    fn dispatch(
        &self,
        address: GuestPhysicalAddress,
        width: u8,
        operation: RamAccessOperation,
    ) -> Result<u64, MmioDispatchError> {
        self.dirty_at_call
            .set(Some(get(self.memory, PT + 72) & DIRTY != 0));
        self.calls.borrow_mut().push((address, width, operation));
        if self.fail {
            return Err(MmioDispatchError::Backend("device refused".to_owned()));
        }
        Ok(0xCAFE_F00D)
    }
}

fn mmio_setup() -> (PhysicalMemory, PaeProbeConfig) {
    let memory = memory();
    fixture(&memory, &Scenario::new("", false, 0, Expect::Ok));
    let paging = PaeProbeConfig::from_protected_cpu(&cpu(true)).unwrap();
    (memory, paging)
}

#[test]
fn an_mmio_operand_reaches_the_set_dispatch_through_the_pae_walk() {
    let (memory, paging) = mmio_setup();
    let device = FakeMmio::new(&memory, vec![(MMIO_AT, 0x1000)]);
    let mut writes = Vec::new();
    let mut sink = |range: GuestPhysicalRange| writes.push((range.start().get(), range.len()));
    let mut adapter =
        PagedPaeMemoryAdapter::over_memory_and_set_mmio(&memory, &device, paging, 0x77, &mut sink);
    // A dword read at linear 0x9010: PAE walk to MMIO_AT + 0x10, served by the device.
    let mut dword = [0u8; 4];
    adapter.read(0x9010, ring0(), &mut dword).unwrap();
    assert_eq!(u32::from_le_bytes(dword), 0xCAFE_F00D);
    assert_eq!(
        device.calls.borrow().as_slice(),
        [(
            at(MMIO_AT + 0x10),
            4,
            RamAccessOperation::Read {
                width: AccessWidth::Dword
            }
        )]
    );
    assert_eq!(
        get(&memory, PT + 72) & (ACCESSED | DIRTY),
        ACCESSED,
        "a read does not dirty"
    );
    // A word write at 0x9008: the dirty bit is set before the device sees the access.
    adapter.write(0x9008, ring0(), &[0x34, 0x12]).unwrap();
    assert_eq!(device.dirty_at_call.get(), Some(true));
    assert_eq!(
        device.calls.borrow()[1],
        (
            at(MMIO_AT + 8),
            2,
            RamAccessOperation::Write {
                width: AccessWidth::Word,
                value: 0x1234
            }
        )
    );
    // The device's bytes are not RAM: nothing was stored behind the region.
    drop(adapter);
    assert_eq!(get(&memory, PT + 72) & DIRTY, DIRTY);
}

#[test]
fn an_mmio_operand_outside_the_windows_is_a_typed_refusal_before_any_device_or_dirty_bit() {
    let (memory, paging) = mmio_setup();
    let device = FakeMmio::new(&memory, vec![(MMIO_AT + 0x800, 0x100)]);
    let mut sink = |_: GuestPhysicalRange| {};
    let mut adapter =
        PagedPaeMemoryAdapter::over_memory_and_set_mmio(&memory, &device, paging, 0x77, &mut sink);
    let mut dword = [0xEE; 4];
    assert!(matches!(
        adapter.read(0x9010, ring0(), &mut dword),
        Err(MemoryError::Backend(_))
    ));
    assert_eq!(dword, [0xEE; 4], "a failed read leaves the destination");
    assert!(matches!(
        adapter.memory_error(),
        Some(TrackedAccessError::OperandPhysical { .. })
    ));
    assert!(matches!(
        adapter.write(0x9010, ring0(), &[1, 2, 3, 4]),
        Err(MemoryError::Backend(_))
    ));
    assert!(matches!(
        adapter.memory_error(),
        Some(TrackedAccessError::DeviceUnavailable { .. })
    ));
    // A write inside the window at 0x9800 is served (the window starts at 0x800 of the page).
    adapter.write(0x9800, ring0(), &[7]).unwrap();
    drop(adapter);
    assert_eq!(device.calls.borrow().len(), 1);
    assert_eq!(
        get(&memory, PT + 72) & DIRTY,
        DIRTY,
        "only the served write dirtied the page"
    );
}

#[test]
fn a_device_error_is_a_backend_error_and_a_read_never_dirties() {
    let (memory, paging) = mmio_setup();
    let mut device = FakeMmio::new(&memory, vec![(MMIO_AT, 0x1000)]);
    device.fail = true;
    let mut sink = |_: GuestPhysicalRange| {};
    let mut adapter =
        PagedPaeMemoryAdapter::over_memory_and_set_mmio(&memory, &device, paging, 0x77, &mut sink);
    let mut dword = [0u8; 4];
    assert!(matches!(
        adapter.read(0x9000, ring0(), &mut dword),
        Err(MemoryError::Backend(_))
    ));
    assert_eq!(
        adapter.memory_error(),
        Some(&TrackedAccessError::MmioBackend {
            address: at(MMIO_AT),
            message: "device refused".to_owned()
        })
    );
    drop(adapter);
    assert_eq!(get(&memory, PT + 72) & DIRTY, 0);
}

#[test]
fn a_page_crossing_operand_never_reaches_a_device_and_a_crossing_store_writes_nothing() {
    let (memory, paging) = mmio_setup();
    let device = FakeMmio::new(&memory, vec![(MMIO_AT, 0x1000)]);
    let mut sink = |_: GuestPhysicalRange| {};
    let mut adapter =
        PagedPaeMemoryAdapter::over_memory_and_set_mmio(&memory, &device, paging, 0x77, &mut sink);
    // 0x8FFE..0x9002: two RAM bytes of the code page, two of the MMIO page.
    let before = get(&memory, u64::from(CODE) + 0xFF8);
    let mut dword = [0xEE; 4];
    assert!(adapter.read(0x8FFE, ring0(), &mut dword).is_err());
    assert_eq!(dword, [0xEE; 4]);
    assert!(adapter.write(0x8FFE, ring0(), &[1, 2, 3, 4]).is_err());
    assert_eq!(
        get(&memory, u64::from(CODE) + 0xFF8),
        before,
        "no byte of the RAM half stored"
    );
    assert!(matches!(
        adapter.memory_error(),
        Some(TrackedAccessError::DeviceUnavailable { .. })
    ));
    drop(adapter);
    assert!(device.calls.borrow().is_empty());
    assert_eq!(
        get(&memory, PT + 72) & DIRTY,
        0,
        "the probe of a crossing store sets no dirty bit"
    );
}

#[test]
fn a_write_intent_read_and_an_instruction_fetch_never_reach_a_device() {
    let (memory, paging) = mmio_setup();
    let device = FakeMmio::new(&memory, vec![(MMIO_AT, 0x1000)]);
    let mut sink = |_: GuestPhysicalRange| {};
    let mut adapter =
        PagedPaeMemoryAdapter::over_memory_and_set_mmio(&memory, &device, paging, 0x77, &mut sink);
    assert!(matches!(
        adapter.read_integer_for_write(0x9010, ring0(), OperandSize::Dword),
        Err(MemoryError::Backend(_))
    ));
    assert!(matches!(
        adapter.fetch(0x9010, ring0()),
        Err(MemoryError::Backend(_))
    ));
    assert!(matches!(
        adapter.memory_error(),
        Some(TrackedAccessError::OperandPhysical { .. })
    ));
    // On RAM both are served: the destination is walked as a write (no dirty bit), the fetch as a fetch.
    assert!(
        adapter
            .read_integer_for_write(0x5010, ring0(), OperandSize::Dword)
            .is_ok()
    );
    assert!(adapter.fetch(u64::from(CODE), ring0()).is_ok());
    drop(adapter);
    assert!(device.calls.borrow().is_empty());
    assert_eq!(get(&memory, PT + 40) & (ACCESSED | DIRTY), ACCESSED);
}

#[test]
fn an_mmio_operand_ending_exactly_at_the_end_of_its_window_is_served_and_one_byte_more_is_not() {
    let (memory, paging) = mmio_setup();
    // The window is the first 0x10 bytes of the MMIO page.
    let device = FakeMmio::new(&memory, vec![(MMIO_AT, 0x10)]);
    let mut sink = |_: GuestPhysicalRange| {};
    let mut adapter =
        PagedPaeMemoryAdapter::over_memory_and_set_mmio(&memory, &device, paging, 0x77, &mut sink);
    let mut dword = [0u8; 4];
    // 0x900C..0x9010 is MMIO_AT + 0xC..0x10: the last four bytes of the window.
    adapter.read(0x900C, ring0(), &mut dword).unwrap();
    assert_eq!(u32::from_le_bytes(dword), 0xCAFE_F00D);
    adapter.write(0x900F, ring0(), &[9]).unwrap();
    assert_eq!(device.calls.borrow().len(), 2);
    // One byte further the operand leaves the window: refused before any device is reached.
    assert!(matches!(
        adapter.read(0x900D, ring0(), &mut dword),
        Err(MemoryError::Backend(_))
    ));
    assert!(matches!(
        adapter.write(0x9010, ring0(), &[9]),
        Err(MemoryError::Backend(_))
    ));
    assert_eq!(device.calls.borrow().len(), 2);
}

#[test]
fn a_framebuffer_page_serves_data_like_ram_but_an_instruction_fetch_from_it_is_a_backend_error() {
    let memory = memory_with_framebuffer();
    fixture(&memory, &Scenario::new("", false, 0, Expect::Ok));
    memory
        .store_bytes(at(FB_AT + 0x10), &[0x11, 0x22, 0x33, 0x44], |_| {
            panic!("framebuffer writes never notify")
        })
        .unwrap();
    let paging = PaeProbeConfig::from_protected_cpu(&cpu(true)).unwrap();
    let mut notified = Vec::new();
    let mut sink = |range: GuestPhysicalRange| notified.push(range.start().get());
    let mut adapter = PagedPaeMemoryAdapter::over_memory(&memory, paging, 0x77, &mut sink);
    // Data: a read and a write behave like RAM (linear 0xC000 is PT[12]).
    let mut dword = [0u8; 4];
    adapter.read(0xC010, ring0(), &mut dword).unwrap();
    assert_eq!(dword, [0x11, 0x22, 0x33, 0x44]);
    adapter.write(0xC010, ring0(), &[0xAA, 0xBB]).unwrap();
    assert_eq!(adapter.memory_error(), None);
    // An instruction fetch is refused as in the 4-level access: Framebuffer is not executable memory.
    assert!(matches!(
        adapter.fetch(0xC010, ring0()),
        Err(MemoryError::Backend(_))
    ));
    assert_eq!(
        adapter.memory_error(),
        Some(&TrackedAccessError::OperandPhysical {
            address: at(FB_AT + 0x10),
            source: MemoryAccessError::UnreadableRegion(MemoryRegionKind::Framebuffer),
        })
    );
    drop(adapter);
    // The callback saw the page-table writes (PDE A, PTE A, PTE D: RAM) and no framebuffer byte.
    assert_eq!(notified, [PD, PT + 96, PT + 96]);
    assert_eq!(get(&memory, FB_AT + 0x10) & 0xFFFF_FFFF, 0x4433_BBAA);
    assert_eq!(
        get(&memory, PT + 96) & (ACCESSED | DIRTY),
        ACCESSED | DIRTY,
        "the walks reached the leaf; the write dirtied it"
    );
}

/// A device that counts its accesses and stores one value.
struct CountingDevice(Arc<Mutex<(usize, u64)>>);

impl BusDevice for CountingDevice {
    fn read(&mut self, _access: DeviceAccess) -> Result<u64, DeviceError> {
        let mut state = self.0.lock().unwrap();
        state.0 += 1;
        Ok(state.1)
    }

    fn write(&mut self, _access: DeviceAccess, value: u64) -> Result<(), DeviceError> {
        let mut state = self.0.lock().unwrap();
        state.0 += 1;
        state.1 = value;
        Ok(())
    }
}

#[test]
fn an_mmio_operand_reaches_the_bound_device_of_the_machine_bus() {
    let state = Arc::new(Mutex::new((0usize, 0x1122_3344u64)));
    let mut bus = MachineBus::new(
        map(),
        RamLimits::new(1 << 20),
        vec![Box::new(CountingDevice(Arc::clone(&state)))],
        vec![MmioBinding {
            region_start: at(MMIO_AT),
            device_index: 0,
        }],
        vec![],
    )
    .unwrap();
    fixture(
        bus.physical_memory(),
        &Scenario::new("", false, 0, Expect::Ok),
    );
    let paging = PaeProbeConfig::from_protected_cpu(&cpu(true)).unwrap();
    let mut sink = |_: GuestPhysicalRange| {};
    let mut adapter = PagedPaeMemoryAdapter::over_machine(&mut bus, 0, paging, 0x77, &mut sink);
    let mut dword = [0u8; 4];
    adapter.read(0x9004, ring0(), &mut dword).unwrap();
    assert_eq!(u32::from_le_bytes(dword), 0x1122_3344);
    adapter
        .write(0x9004, ring0(), &0xAABB_CCDDu32.to_le_bytes())
        .unwrap();
    drop(adapter);
    assert_eq!(*state.lock().unwrap(), (2, 0xAABB_CCDD));
    assert_eq!(
        get(bus.physical_memory(), PT + 72) & (ACCESSED | DIRTY),
        ACCESSED | DIRTY
    );
}

#[test]
fn locked_operations_and_cmpxchg16b_are_refused_on_pae_paging_before_any_walk() {
    let (memory, paging) = mmio_setup();
    let image = dump(&memory);
    let mut writes = 0;
    let mut sink = |_: GuestPhysicalRange| writes += 1;
    let mut adapter = PagedPaeMemoryAdapter::over_memory(&memory, paging, 0x77, &mut sink);
    fn refused<T>(result: Result<T, MemoryError>) -> bool {
        match result {
            Err(MemoryError::Backend(message)) => message.contains("refused on PAE paging"),
            _ => false,
        }
    }
    let size = OperandSize::Dword;
    assert!(refused(adapter.atomic_exchange_integer(
        0x5010,
        ring0(),
        size,
        1
    )));
    assert!(refused(adapter.atomic_fetch_add_integer(
        0x5010,
        ring0(),
        size,
        1
    )));
    assert!(refused(adapter.atomic_compare_exchange_integer(
        0x5010,
        ring0(),
        size,
        1,
        2
    )));
    assert!(refused(adapter.atomic_rmw_integer(
        0x5010,
        ring0(),
        size,
        &mut |old| old + 1
    )));
    assert!(refused(adapter.atomic_rmw_u128(
        0x5010,
        ring0(),
        &mut |old| old
    )));
    // Through the provided methods of the trait that end in the same override.
    assert!(refused(adapter.atomic_rmw_integer(
        0x5010,
        ring0(),
        OperandSize::Qword,
        &mut |old| old
    )));
    assert!(adapter.mfence(ring0()).is_err());
    drop(adapter);
    assert_eq!(writes, 0, "nothing was committed");
    assert_eq!(
        dump(&memory),
        image,
        "no accessed or dirty bit, no data byte"
    );
}

#[test]
fn widths_addresses_and_origins_the_adapter_does_not_serve_are_typed_refusals() {
    let (memory, paging) = mmio_setup();
    let image = dump(&memory);
    let mut sink = |_: GuestPhysicalRange| {};
    let mut adapter = PagedPaeMemoryAdapter::over_memory(&memory, paging, 0x77, &mut sink);
    let mut sixteen = [0u8; 16];
    assert!(matches!(
        adapter.read(0x5010, ring0(), &mut sixteen),
        Err(MemoryError::Backend(_))
    ));
    assert!(matches!(
        adapter.write(0x5010, ring0(), &sixteen),
        Err(MemoryError::Backend(_))
    ));
    let mut three = [0u8; 3];
    assert!(matches!(
        adapter.read(0x5010, ring0(), &mut three),
        Err(MemoryError::Backend(_))
    ));
    // A linear address of 32 bits: above 4 GiB, or an operand that wraps past it, is refused.
    let mut dword = [0u8; 4];
    assert!(matches!(
        adapter.read(1 << 32, ring0(), &mut dword),
        Err(MemoryError::Backend(_))
    ));
    assert!(matches!(
        adapter.read(0xFFFF_FFFE, ring0(), &mut dword),
        Err(MemoryError::Backend(_))
    ));
    assert!(matches!(
        adapter.fetch(u64::MAX, ring0()),
        Err(MemoryError::AddressOverflow { .. }) | Err(MemoryError::Backend(_))
    ));
    // A descriptor-table access belongs to the long-mode adapter's gate.
    let descriptor = AccessContext {
        origin: AccessOrigin::DescriptorTable,
        ..ring0()
    };
    assert!(matches!(
        adapter.read(0x5010, descriptor, &mut dword),
        Err(MemoryError::Backend(_))
    ));
    assert!(matches!(
        adapter.write(0x5010, descriptor, &dword),
        Err(MemoryError::Backend(_))
    ));
    // A page-crossing write-intent destination is the typed `CrossesPage`.
    assert!(
        adapter
            .read_integer_for_write(0x5FFE, ring0(), OperandSize::Dword)
            .is_err()
    );
    assert!(matches!(
        adapter.memory_error(),
        Some(TrackedAccessError::CrossesPage { .. })
    ));
    drop(adapter);
    assert_eq!(dump(&memory), image, "no walk ran for a refused shape");
}

#[test]
fn a_user_access_is_checked_against_the_user_bit_and_the_context_is_normalized() {
    let s = Scenario::new("", false, 0, Expect::Ok).pte5(DATA_A | P | RW);
    let memory = memory();
    fixture(&memory, &s);
    let paging = PaeProbeConfig::from_protected_cpu(&cpu(true)).unwrap();
    let mut sink = |_: GuestPhysicalRange| {};
    let mut adapter = PagedPaeMemoryAdapter::over_memory(&memory, paging, 0x77, &mut sink);
    let mut dword = [0u8; 4];
    // CPL 3 on a supervisor page: P = 1, U/S = 1 (error code 0x5).
    let Err(MemoryError::Architectural(fault)) =
        adapter.read(0x5010, ctx(PrivilegeLevel::Ring3), &mut dword)
    else {
        panic!("expected a page fault");
    };
    assert_eq!(
        (fault.error_code, fault.linear_address),
        (Some(0x5), Some(0x5010))
    );
    // The same address at CPL 0 reads. An implicit stack access takes the CPL of the instruction.
    adapter.read(0x5010, ring0(), &mut dword).unwrap();
    let stack = AccessContext {
        origin: AccessOrigin::ImplicitStack,
        ..ctx(PrivilegeLevel::Ring3)
    };
    assert!(adapter.read(0x5010, stack, &mut dword).is_err());
    assert_eq!(adapter.paging().cr3(), PDPT);
}

#[test]
fn a_page_fault_is_recorded_with_the_saved_eip_and_the_reason() {
    let s = Scenario::new("", false, 0, Expect::Ok).pte5(DATA_A | RW | US);
    let memory = memory();
    fixture(&memory, &s);
    let paging = PaeProbeConfig::from_protected_cpu(&cpu(true)).unwrap();
    let mut sink = |_: GuestPhysicalRange| {};
    let mut adapter = PagedPaeMemoryAdapter::over_memory(&memory, paging, 0x1111, &mut sink);
    let mut dword = [0u8; 4];
    assert!(adapter.read(0x5010, ring0(), &mut dword).is_err());
    assert_eq!(adapter.page_fault().unwrap().saved_rip(), 0x1111);
    assert!(matches!(
        adapter.access_error(),
        Some(TrackedAccessError::PageFault {
            reason: PageFaultReason::NotPresent { .. },
            ..
        })
    ));
    adapter.set_instruction_rip(0x2222);
    assert!(adapter.read(0x5010, ring0(), &mut dword).is_err());
    assert_eq!(adapter.page_fault().unwrap().saved_rip(), 0x2222);
    assert!(
        adapter.memory_error().is_none(),
        "a page fault is not a model refusal"
    );
}

#[test]
fn an_instruction_fetch_walks_as_a_fetch_with_the_xd_check_and_serves_ram_only() {
    let (memory, paging) = mmio_setup();
    let mut sink = |_: GuestPhysicalRange| {};
    let mut adapter = PagedPaeMemoryAdapter::over_memory(&memory, paging, 0x77, &mut sink);
    // The code page is executable: the byte, A on the PDE and the PTE, never D.
    memory
        .store_bytes(at(u64::from(CODE)), &[0xC3], |_| {})
        .unwrap();
    assert_eq!(adapter.fetch(u64::from(CODE), ring0()), Ok(0xC3));
    assert_eq!(get(&memory, PT + 64) & (ACCESSED | DIRTY), ACCESSED);
    // PT[7] is an XD page and EFER.NXE is set: a fetch is #PF with P = 1 and I/D = 1 (0x11).
    let Err(MemoryError::Architectural(fault)) = adapter.fetch(0x7000, ring0()) else {
        panic!("expected a page fault");
    };
    assert_eq!(
        (fault.error_code, fault.linear_address),
        (Some(0x11), Some(0x7000))
    );
    assert_eq!(
        get(&memory, PT + 56) & ACCESSED,
        ACCESSED,
        "the walk reached the leaf"
    );
    // A ROM byte, a hole and an MMIO byte are not fetched (the walk of a fetch is the walk of a read; the region decides).
    for linear in [0xA000, 0xB000, 0x9000] {
        assert!(matches!(
            adapter.fetch(linear, ring0()),
            Err(MemoryError::Backend(_))
        ));
        assert!(matches!(
            adapter.memory_error(),
            Some(TrackedAccessError::OperandPhysical { .. })
        ));
    }
}

// ---- the long-mode path is not selected by a PAE state, and the PAE path not by a long-mode one -----

const EFER_LME_LMA: u64 = (1 << 8) | (1 << 10);

#[test]
fn the_two_paging_snapshots_exclude_each_other_by_construction() {
    let pdptes = [PD | P, 0, 0, 0];
    let cr0 = u64::from(CR0_WP_SET);
    // PAE registers are not a 4-level snapshot: LME and LMA are required there, and the state is
    // named as PAE paging of a protected-mode vCPU (S2-25n).
    assert_eq!(
        Long64ProbeConfig::new(cr0, PDPT, 0x20, EFER_NXE),
        Err(ProbeConfigError::PaeProtectedMode {
            cr0,
            cr4: 0x20,
            efer: EFER_NXE
        })
    );
    // LME alone (the transitional state) is still a missing bit, not PAE paging.
    assert_eq!(
        Long64ProbeConfig::new(cr0, PDPT, 0x20, 1 << 8),
        Err(ProbeConfigError::RequiredBits {
            register: "EFER",
            bits: EFER_LME_LMA
        })
    );
    // IA-32e registers are not a PAE snapshot.
    assert!(PaeProbeConfig::new(cr0, PDPT, 0x20, EFER_LME_LMA | EFER_NXE, pdptes).is_err());
    assert!(PaeProbeConfig::new(cr0, PDPT, 0x20, EFER_NXE, pdptes).is_ok());
}

/// A vCPU state on PAE paging as the 4-level interpreter paths would receive it (CR0.PG, CR4.PAE,
/// no LME) with a NOP at the RIP: executing it as long mode would be wrong, so every entry refuses.
fn pae_long_state() -> LongModeState {
    LongModeState {
        rip: u64::from(CODE),
        cr0: Some(u64::from(CR0_WP_SET)),
        cr3: None,
        cr4: Some(0x20),
        rflags: 0x202,
        activity: Some(od_isa::state::CpuActivity::Running),
        sti_shadow: Some(od_isa::state::StiShadow::Clear),
        ..Default::default()
    }
}

fn pae_bus() -> MachineBus {
    let device = CountingDevice(Arc::new(Mutex::new((0, 0))));
    let bus = MachineBus::new(
        map(),
        RamLimits::new(1 << 20),
        vec![Box::new(device)],
        vec![MmioBinding {
            region_start: at(MMIO_AT),
            device_index: 0,
        }],
        vec![],
    )
    .unwrap();
    fixture(
        bus.physical_memory(),
        &Scenario::new("", false, 0, Expect::Ok),
    );
    bus.physical_memory()
        .store_bytes(at(u64::from(CODE)), &[0x90, 0x90, 0x90, 0x90], |_| {})
        .unwrap();
    bus
}

#[test]
fn the_four_level_probe_entries_refuse_a_pae_state_with_a_typed_configuration_error() {
    let rejection = PagedProbeExit::Rejected(PagedProbeRejection::Configuration(
        ProbeConfigError::PaeProtectedMode {
            cr0: u64::from(CR0_WP_SET),
            cr4: 0x20,
            efer: EFER_NXE,
        },
    ));
    // Exclusive bus.
    let mut bus = pae_bus();
    let image = dump(bus.physical_memory());
    let mut state = pae_long_state();
    let mut writes = 0;
    let report = crate::run_paged_probe(
        &mut state,
        &mut bus,
        PDPT,
        EFER_NXE,
        ring0(),
        4,
        od_isa::INTEGER_FLAGS,
        || false,
        |_| writes += 1,
    );
    assert_eq!(report.exit, rejection);
    assert_eq!(report.instructions_executed, 0);
    assert_eq!(state, pae_long_state(), "the state is untouched");
    assert_eq!(writes, 0);
    assert_eq!(
        first_difference(&dump(bus.physical_memory()), &image),
        None,
        "no fetch walk, no accessed bit"
    );
    // Shared bus (the vCPU thread's form).
    let mut state = pae_long_state();
    let report = crate::run_paged_probe_shared(
        &mut state,
        &bus,
        0,
        PDPT,
        EFER_NXE,
        ring0(),
        4,
        od_isa::INTEGER_FLAGS,
        || false,
        |_| writes += 1,
    );
    assert_eq!(report.exit, rejection);
    assert_eq!(report.instructions_executed, 0);
    assert_eq!(first_difference(&dump(bus.physical_memory()), &image), None);
}

#[test]
fn the_cpl0_set_loop_refuses_a_pae_state_with_the_same_configuration_error() {
    let mut bus = pae_bus();
    let image = dump(bus.physical_memory());
    let mut state = pae_long_state();
    let set = od_isa::execute::Cpl0ProfileSet::new(od_isa::ControlRegisterPolicy::Cpl0BaseLong64V1);
    let report = crate::paged_memory_adapter::run_cpl0_set_paged_2m_ram(
        &mut state,
        &mut bus,
        PDPT,
        EFER_NXE,
        ring0(),
        4,
        od_isa::INTEGER_FLAGS,
        &set,
        None,
        || false,
        |_| {},
    );
    assert_eq!(report.instructions_executed, 0);
    assert_eq!(
        report.exit,
        crate::paged_memory_adapter::PagedCpl0SetExit::Configuration(
            ProbeConfigError::PaeProtectedMode {
                cr0: u64::from(CR0_WP_SET),
                cr4: 0x20,
                efer: EFER_NXE,
            }
        )
    );
    assert_eq!(first_difference(&dump(bus.physical_memory()), &image), None);
}
