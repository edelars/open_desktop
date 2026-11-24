//! PERF-11 (ADR-0016, prototype): the data translation cache of the set loop's adapter against
//! the same adapter without it. Every test runs the same operations twice on fresh, identical
//! machines and compares, operation by operation, the outcome (bytes or error), the page fault
//! record and the access error, then the sequence of committed writes (the accessed bits of the
//! walks, the guest stores) and the whole RAM.

use super::*;
use crate::data_translation_cache::{
    DATA_CACHE_COUNTED, DataCacheCounters, DataTranslationCache, data_cache_counters,
    reset_data_cache_counters,
};
use crate::{MachineMemoryMap, MemoryRegionSpec, PhysicalMemory, RamLimits};

const CR0: u64 = 0x8001_0033;
const CR0_WP: u64 = 1 << 16;
const CR4: u64 = 1 << 5;
const EFER: u64 = (1 << 8) | (1 << 10) | (1 << 11);
const ROOT: u64 = 0x1000;
/// A second root (`MOV CR3`): the same tables, except that `DATA` maps the frame 0x8000.
const OTHER_ROOT: u64 = 0xa000;
const RAM_LEN: u64 = 0x40_0000;
/// PT 0x5000: [0] 0x7000, [1] 0x8000, [2] 0x9000 supervisor, [3] not present, [4] reserved bit,
/// [5] 0x8000 read-only.
const DATA: u64 = 0x0080_0000;
/// PT 0x4000: [0] the frame of the PT of `DATA` (0x5000), [1] the PD (0x3000).
const TABLE_VIEW: u64 = 0x0040_0000;
/// PD[6]: a 2 MiB leaf at 0x20_0000.
const LARGE: u64 = 0x00c0_0000;
/// PD[7] of the second variant: a 4 KiB PT at 0x6000 whose [0] maps a frame of another region.
const OTHER_REGION: u64 = 0x00e0_0000;

fn context(privilege: PrivilegeLevel) -> AccessContext {
    AccessContext {
        mode: InstructionMode::Long64,
        privilege,
        origin: AccessOrigin::Explicit,
    }
}

fn ring0() -> AccessContext {
    context(PrivilegeLevel::Ring0)
}

fn ring3() -> AccessContext {
    context(PrivilegeLevel::Ring3)
}

fn paging() -> Long64ProbeConfig {
    Long64ProbeConfig::new(CR0, ROOT, CR4, EFER).unwrap()
}

fn put(memory: &PhysicalMemory, address: u64, value: u64) {
    memory
        .store_u64(GuestPhysicalAddress::new(address), value, |_| {})
        .unwrap();
}

/// The tables of the tests (no accessed bit set anywhere: the first walk sets them) over plain
/// RAM, and `extra` regions after the RAM. An MMIO region is served by the `SetStepMmio` of the
/// test, not by a device of a bus.
fn memory_with(extra: Vec<MemoryRegionSpec>) -> PhysicalMemory {
    let mut regions = vec![MemoryRegionSpec::new(
        GuestPhysicalRange::new(GuestPhysicalAddress::new(0), RAM_LEN).unwrap(),
        MemoryRegionKind::Ram,
    )];
    regions.extend(extra);
    let map = MachineMemoryMap::new(36, regions).unwrap();
    let memory = PhysicalMemory::new(&map, RamLimits::new(RAM_LEN + 0x10_0000)).unwrap();
    let bus = &memory;
    for root in [ROOT, OTHER_ROOT] {
        put(bus, root, 0x2007);
    }
    put(bus, 0x2000, 0x3007);
    put(bus, 0x3010, 0x4007);
    put(bus, 0x3020, 0x5007);
    put(bus, 0x3030, 0x20_0000 | 0x87);
    put(bus, 0x4000, 0x5007);
    put(bus, 0x4008, 0x3007);
    put(bus, 0x5000, 0x7007);
    put(bus, 0x5008, 0x8007);
    put(bus, 0x5010, 0x9003);
    put(bus, 0x5018, 0);
    put(bus, 0x5020, 0x7007 | (1 << 40));
    put(bus, 0x5028, 0x8005);
    // The second root: PDPT 0xb000, PD 0xc000 whose [4] is the PT 0xd000 ([0] = 0x8000).
    put(bus, OTHER_ROOT, 0xb007);
    put(bus, 0xb000, 0xc007);
    put(bus, 0xc020, 0xd007);
    put(bus, 0xd000, 0x8007);
    for (frame, salt) in [
        (0x7000u64, 0x11u64),
        (0x8000, 0x22),
        (0x9000, 0x33),
        (0x20_0000, 0x44),
    ] {
        for index in 0..0x200u64 {
            put(bus, frame + index * 8, (index << 8) ^ salt ^ (frame << 20));
        }
    }
    memory
}

fn bus() -> PhysicalMemory {
    memory_with(Vec::new())
}

#[derive(Clone)]
enum Kind {
    Read,
    Write(Vec<u8>),
}

#[derive(Clone)]
struct Op {
    address: u64,
    context: AccessContext,
    paging: Long64ProbeConfig,
    kind: Kind,
}

fn read(address: u64) -> Op {
    Op {
        address,
        context: ring0(),
        paging: paging(),
        kind: Kind::Read,
    }
}

fn write(address: u64, bytes: &[u8]) -> Op {
    Op {
        kind: Kind::Write(bytes.to_vec()),
        ..read(address)
    }
}

struct Recorded {
    /// Per operation: the outcome (the bytes read or the error), the page fault record and the
    /// access error the adapter kept.
    steps: Vec<String>,
    writes: Vec<(u64, u64)>,
    ram: Vec<u8>,
}

/// Runs `ops` on `bus`, one adapter per operation (as the loop makes one per instruction), all over
/// one data translation cache when `cached`. Committed writes reach the cache as in the loop's
/// `watch`.
fn run_on(
    ram: &PhysicalMemory,
    mmio: Option<&dyn crate::bus::SetStepMmio>,
    ops: &[Op],
    cached: bool,
) -> (Recorded, DataCacheCounters) {
    reset_data_cache_counters();
    let cache = DataTranslationCache::new();
    let mut writes = Vec::new();
    let mut watch = |range: GuestPhysicalRange| {
        if cached {
            cache.note_write(range);
        }
        writes.push((range.start().get(), range.len()));
    };
    let mut steps = Vec::new();
    for op in ops {
        let mut adapter = set_loop_adapter(
            ram,
            mmio.map(|mmio| (mmio, 0)),
            op.paging,
            0x0040_1000,
            &mut watch,
            None,
            None,
        );
        if cached {
            adapter.data_cache = Some(&cache);
        }
        let outcome = match &op.kind {
            Kind::Read => {
                let mut bytes = [0u8; 8];
                format!(
                    "{:?}",
                    adapter
                        .read(op.address, op.context, &mut bytes)
                        .map(|()| bytes)
                )
            }
            Kind::Write(bytes) => format!("{:?}", adapter.write(op.address, op.context, bytes)),
        };
        steps.push(format!(
            "{outcome} | {:?} | {:?}",
            adapter.page_fault, adapter.access_error
        ));
    }
    let mut bytes = vec![0u8; RAM_LEN as usize];
    ram.load_bytes(GuestPhysicalAddress::new(0), &mut bytes)
        .unwrap();
    (
        Recorded {
            steps,
            writes,
            ram: bytes,
        },
        data_cache_counters(),
    )
}

/// Both runs on fresh machines from `make`; asserts they are equal and returns the counters of
/// the cached one.
fn equivalent(make: impl Fn() -> PhysicalMemory, ops: &[Op]) -> DataCacheCounters {
    let (plain, plain_counters) = run_on(&make(), None, ops, false);
    let (cached, counters) = run_on(&make(), None, ops, true);
    assert_eq!(plain_counters, DataCacheCounters::default());
    assert_same(&plain, &cached);
    counters
}

fn assert_same(plain: &Recorded, cached: &Recorded) {
    assert_eq!(plain.steps.len(), cached.steps.len());
    for (index, (a, b)) in plain.steps.iter().zip(&cached.steps).enumerate() {
        assert_eq!(a, b, "operation {index}");
    }
    assert_eq!(plain.writes, cached.writes, "committed writes");
    assert!(plain.ram == cached.ram, "RAM differs");
}

/// (a) Reads of one page and of a 2 MiB leaf, aligned, misaligned and at the last qword of the
/// page, faulting reads (not present, reserved bit, U/S) repeated, reads between data stores and
/// the descriptor-table and crossing reads the cache leaves alone: the same outcome, records,
/// committed writes and RAM, and the cache really serves (debug builds count the hits).
#[test]
fn qword_reads_through_the_cache_equal_the_walks() {
    let user = |op: Op| Op {
        context: ring3(),
        ..op
    };
    let stack = |op: Op| Op {
        context: AccessContext {
            origin: AccessOrigin::ImplicitStack,
            ..ring0()
        },
        ..op
    };
    let descriptor = |op: Op| Op {
        context: ring0().descriptor_table(),
        ..op
    };
    let ops = vec![
        read(DATA),
        read(DATA),
        read(DATA + 0x10),
        read(DATA + 0x14),
        read(DATA + 0xff8),
        stack(read(DATA + 0x100)),
        user(read(DATA + 0x20)),
        user(read(DATA + 0x20)),
        read(DATA + 0x1000),
        read(DATA + 0x1ff8),
        write(DATA + 0x18, &0x1234_5678_9abc_def0u64.to_le_bytes()),
        read(DATA + 0x18),
        write(DATA + 0x1c, &[0x55]),
        read(DATA + 0x18),
        read(DATA + 0x2000),
        read(DATA + 0x2000),
        user(read(DATA + 0x2000)),
        user(read(DATA + 0x2000)),
        read(DATA + 0x3000),
        read(DATA + 0x3000),
        read(DATA + 0x4000),
        read(DATA + 0x4000),
        user(read(DATA + 0x5008)),
        user(read(DATA + 0x5008)),
        read(LARGE + 0x1238),
        read(LARGE + 0x1238),
        read(LARGE + 0x1_f000),
        // Not served by the cache: a qword across the page end, a descriptor-table read.
        read(DATA + 0xffc),
        descriptor(read(DATA + 0x40)),
        read(DATA + 0x40),
        read(0x0000_8000_0000_0000),
        read(0x0000_8000_0000_0000),
    ];
    let counters = equivalent(bus, &ops);
    if DATA_CACHE_COUNTED {
        assert!(counters.hits >= 10, "{counters:?}");
        assert!(counters.inserts >= 6, "{counters:?}");
    }
}

/// (a) A hit makes exactly the host accesses of the operand read of the walk (an aligned qword is
/// one 8-byte access, a misaligned one the chunks of the copy) and none of the table reads.
#[test]
fn a_hit_reads_the_operand_with_the_host_accesses_of_the_walk() {
    for (offset, operand) in [
        (0x10u64, vec![(0x7010usize, 8usize)]),
        (0x14, vec![(0x7014, 4), (0x7018, 4)]),
        (
            0x13,
            vec![(0x7013, 1), (0x7014, 4), (0x7018, 2), (0x701a, 1)],
        ),
    ] {
        let memory = bus();
        let ram = &memory;
        let cache = DataTranslationCache::new();
        let mut watch = |range: GuestPhysicalRange| cache.note_write(range);
        let mut adapter = set_loop_adapter(ram, None, paging(), 0, &mut watch, None, None);
        adapter.data_cache = Some(&cache);
        let mut bytes = [0u8; 8];
        // The first walk sets the accessed bits; then a miss on a walk that writes nothing.
        adapter.read(DATA + offset, ring0(), &mut bytes).unwrap();
        cache.flush();
        let (miss, walked) =
            crate::ram_storage::traced(|| adapter.read(DATA + offset, ring0(), &mut bytes));
        miss.unwrap();
        let expected = bytes;
        bytes = [0; 8];
        let (hit, served) =
            crate::ram_storage::traced(|| adapter.read(DATA + offset, ring0(), &mut bytes));
        hit.unwrap();
        assert_eq!(bytes, expected);
        assert_eq!(served, operand, "offset {offset:#x}");
        assert_eq!(walked[walked.len() - served.len()..], served[..]);
        assert_eq!(
            walked.len(),
            served.len() + 4,
            "four table reads on the walk"
        );
    }
}

/// (b) A guest store into the PT of a cached page (through its own mapping) changes the
/// translation: the next read sees the new frame, as without the cache. A store into another
/// entry of the same PT frame and an A/D commit of another walk into it drop the entry too (frame
/// granularity), and every outcome stays the one of the walks.
#[test]
fn a_guest_store_into_a_table_frame_drops_the_entries_that_watch_it() {
    let pte_to_8000 = 0x8027u64.to_le_bytes();
    let ops = vec![
        read(DATA),
        read(DATA),
        // PTE[0] of `DATA` -> frame 0x8000.
        write(TABLE_VIEW, &pte_to_8000),
        read(DATA),
        read(DATA),
        // A sibling entry of the same PT frame (PTE[6], unused).
        write(TABLE_VIEW + 0x30, &[0]),
        read(DATA),
        // One byte of the PDE of `DATA` through the view of the PD: NX off (bit 63 = 0), the
        // same mapping, but a write into the PD frame.
        write(TABLE_VIEW + 0x1000 + 0x27, &[0]),
        read(DATA + 8),
        // The first walk of `DATA + 0x1000` commits the accessed bit of PTE[1] in the same frame.
        read(DATA + 0x1000),
        read(DATA + 0x10),
        // PTE[0] not present: the cached page must fault now.
        write(TABLE_VIEW, &0u64.to_le_bytes()),
        read(DATA),
        read(DATA),
    ];
    let counters = equivalent(bus, &ops);
    let (cached, _) = run_on(&bus(), None, &ops, true);
    let after_store = &cached.steps[3];
    let expected = (0x8000u64 << 20) ^ 0x22;
    assert!(
        after_store.starts_with(&format!("Ok({:?})", expected.to_le_bytes())),
        "{after_store}"
    );
    assert!(
        cached.steps[12].contains("PageFault"),
        "{}",
        cached.steps[12]
    );
    if DATA_CACHE_COUNTED {
        assert!(counters.write_flushes >= 4, "{counters:?}");
        assert!(counters.hits >= 2, "{counters:?}");
    }
}

/// (c) A change of CR3, of CR0.WP, of EFER.NXE or of the privilege never hits an entry of another
/// context: each read is the walk of its own context.
#[test]
fn another_root_write_protection_nxe_or_privilege_is_never_a_false_hit() {
    let other_root = Long64ProbeConfig::new(CR0, OTHER_ROOT, CR4, EFER).unwrap();
    let no_wp = Long64ProbeConfig::new(CR0 & !CR0_WP, ROOT, CR4, EFER).unwrap();
    let no_nxe = Long64ProbeConfig::new(CR0, ROOT, CR4, EFER & !(1 << 11)).unwrap();
    let with = |paging: Long64ProbeConfig, op: Op| Op { paging, ..op };
    let user = |op: Op| Op {
        context: ring3(),
        ..op
    };
    let ops = vec![
        read(DATA),
        read(DATA),
        // The same linear page through the second root maps 0x8000.
        with(other_root, read(DATA)),
        with(other_root, read(DATA)),
        read(DATA),
        with(no_wp, read(DATA)),
        with(no_wp, read(DATA)),
        with(no_nxe, read(DATA)),
        // The supervisor page 0x9000: cached for CPL0, a #PF for CPL3 every time.
        read(DATA + 0x2000),
        read(DATA + 0x2000),
        user(read(DATA + 0x2000)),
        read(DATA + 0x2000),
        user(read(DATA + 0x2000)),
        // A user page cached for CPL3 also walks for CPL0.
        user(read(DATA + 0x1000)),
        user(read(DATA + 0x1000)),
        read(DATA + 0x1000),
    ];
    let counters = equivalent(bus, &ops);
    let (cached, _) = run_on(&bus(), None, &ops, true);
    let frame_8000 = (0x8000u64 << 20) ^ 0x22;
    assert!(
        cached.steps[2].starts_with(&format!("Ok({:?})", frame_8000.to_le_bytes())),
        "{}",
        cached.steps[2]
    );
    for index in [10, 12] {
        assert!(
            cached.steps[index].contains("PageFault"),
            "{}",
            cached.steps[index]
        );
    }
    if DATA_CACHE_COUNTED {
        assert!(counters.hits >= 4, "{counters:?}");
        // Every first read of a context is a miss and makes its entry.
        assert!(counters.inserts >= 7, "{counters:?}");
    }
}

/// (c) EFER.NXE with a PTE whose bit 63 (XD) is set: with NXE = 1 a data read is allowed (XD only
/// forbids execution), the first read inserts and the second hits; with NXE = 0 bit 63 is
/// reserved, so every read is a #PF (reserved bit), never a hit of the NXE = 1 entry, and makes no
/// entry; back under NXE = 1 the entry is still there.
#[test]
fn an_xd_pte_is_read_under_nxe_and_a_reserved_bit_fault_without_it() {
    const XD_PAGE: u64 = DATA + 0x7000;
    let make = || {
        let bus = bus();
        // PTE[7] of the PT of `DATA`: the frame 0x7000 with XD.
        put(&bus, 0x5038, 0x7007 | (1 << 63));
        bus
    };
    let no_nxe = Long64ProbeConfig::new(CR0, ROOT, CR4, EFER & !(1 << 11)).unwrap();
    let without_nxe = |op: Op| Op {
        paging: no_nxe,
        ..op
    };
    let ops = vec![
        read(XD_PAGE),
        read(XD_PAGE),
        without_nxe(read(XD_PAGE)),
        without_nxe(read(XD_PAGE)),
        read(XD_PAGE),
    ];
    let counters = equivalent(make, &ops);
    let (cached, _) = run_on(&make(), None, &ops, true);
    let frame_7000 = (0x7000u64 << 20) ^ 0x11;
    for index in [0, 1, 4] {
        assert!(
            cached.steps[index].starts_with(&format!("Ok({:?})", frame_7000.to_le_bytes())),
            "{}",
            cached.steps[index]
        );
    }
    for index in [2, 3] {
        assert!(
            cached.steps[index].contains("PageFault"),
            "{}",
            cached.steps[index]
        );
    }
    if DATA_CACHE_COUNTED {
        // One entry (the first read); hits: the second read and the last one; misses: the first
        // read and the two faulting reads.
        assert_eq!(
            (counters.inserts, counters.hits, counters.misses),
            (1, 2, 3),
            "{counters:?}"
        );
    }
}

/// (b) The view of the tables warmed up first: reads through `TABLE_VIEW` (the PT of `DATA`
/// seen as data) are cached like any page, then a store through that same view into the PTE of
/// `DATA` still drops the entry of `DATA` (the next read walks and sees the new frame), and the
/// reads of the view see the stored PTE.
#[test]
fn a_store_through_a_warm_table_view_still_drops_the_entries_of_its_frame() {
    let ops = vec![
        read(TABLE_VIEW),
        read(TABLE_VIEW),
        read(DATA),
        read(DATA),
        // PTE[0] of `DATA` -> frame 0x8000, through the cached view.
        write(TABLE_VIEW, &0x8027u64.to_le_bytes()),
        read(TABLE_VIEW),
        read(TABLE_VIEW),
        read(DATA),
        read(DATA),
    ];
    let counters = equivalent(bus, &ops);
    let (cached, _) = run_on(&bus(), None, &ops, true);
    let frame_8000 = (0x8000u64 << 20) ^ 0x22;
    for index in [7, 8] {
        assert!(
            cached.steps[index].starts_with(&format!("Ok({:?})", frame_8000.to_le_bytes())),
            "{}",
            cached.steps[index]
        );
    }
    assert!(
        cached.steps[5].starts_with(&format!("Ok({:?})", 0x8027u64.to_le_bytes())),
        "{}",
        cached.steps[5]
    );
    if DATA_CACHE_COUNTED {
        // Inserts: the view and `DATA`, before and after the store; hits: the second read of each
        // pair. The store drops both entries: the dirty bit it commits into PTE[0] of the view's
        // own PT (frame 0x4000) drops the view, the stored bytes in frame 0x5000 drop `DATA`.
        assert_eq!(
            (
                counters.inserts,
                counters.hits,
                counters.misses,
                counters.write_flushes
            ),
            (4, 4, 4, 2),
            "{counters:?}"
        );
    }
}

/// (d) A page in ROM, in a framebuffer, in MMIO, or whose PT lies in a framebuffer is never
/// remembered: every read walks again (and the device is called each time).
#[test]
fn rom_framebuffer_mmio_and_tables_outside_ram_are_never_cached() {
    let base = RAM_LEN;
    let regions = || {
        vec![
            MemoryRegionSpec::new(
                GuestPhysicalRange::new(GuestPhysicalAddress::new(base), 0x1000).unwrap(),
                MemoryRegionKind::Rom,
            ),
            MemoryRegionSpec::new(
                GuestPhysicalRange::new(GuestPhysicalAddress::new(base + 0x1000), 0x2000).unwrap(),
                MemoryRegionKind::Framebuffer,
            ),
        ]
    };
    // PD[7] -> PT 0x6000: [0] the ROM frame, [1] the first framebuffer frame. PD[8] -> a PT in the
    // second framebuffer frame, whose [0] maps the RAM frame 0x7000.
    let make = || {
        let bus = memory_with(regions());
        put(&bus, 0x3038, 0x6007);
        put(&bus, 0x6000, base | 7);
        put(&bus, 0x6008, (base + 0x1000) | 7);
        put(&bus, 0x3040, (base + 0x2000) | 7);
        put(&bus, base + 0x2000, 0x7027);
        bus
    };
    let table_in_framebuffer = 0x0100_0000;
    let ops = vec![
        read(OTHER_REGION),
        read(OTHER_REGION),
        read(OTHER_REGION + 0x1000),
        read(OTHER_REGION + 0x1000),
        read(table_in_framebuffer),
        read(table_in_framebuffer),
        read(DATA),
        read(DATA),
    ];
    let counters = equivalent(make, &ops);
    if DATA_CACHE_COUNTED {
        // Only `DATA` is remembered (one insert, one hit).
        assert_eq!(counters.inserts, 1, "{counters:?}");
        assert_eq!(counters.hits, 1, "{counters:?}");
    }

    struct CountingMmio(std::cell::Cell<usize>);
    impl crate::bus::SetStepMmio for CountingMmio {
        fn windows(&self) -> &[crate::page_walk::MmioWindow] {
            &[(crate::LOCAL_APIC_MMIO_BASE, crate::LOCAL_APIC_MMIO_SIZE)]
        }
        fn dispatch(
            &self,
            _address: GuestPhysicalAddress,
            _width: u8,
            _operation: RamAccessOperation,
        ) -> Result<u64, crate::page_walk::MmioDispatchError> {
            self.0.set(self.0.get() + 1);
            Ok(0x1122_3344_5566_7788)
        }
    }
    let make = || {
        let bus = memory_with(vec![MemoryRegionSpec::new(
            GuestPhysicalRange::new(
                GuestPhysicalAddress::new(crate::LOCAL_APIC_MMIO_BASE),
                crate::LOCAL_APIC_MMIO_SIZE,
            )
            .unwrap(),
            MemoryRegionKind::Mmio,
        )]);
        put(&bus, 0x5030, crate::LOCAL_APIC_MMIO_BASE | 7);
        bus
    };
    let ops = vec![
        read(DATA + 0x6000),
        read(DATA + 0x6000),
        read(DATA + 0x6000),
    ];
    let plain_device = CountingMmio(std::cell::Cell::new(0));
    let (plain, _) = run_on(&make(), Some(&plain_device), &ops, false);
    let cached_device = CountingMmio(std::cell::Cell::new(0));
    let (cached, counters) = run_on(&make(), Some(&cached_device), &ops, true);
    assert_same(&plain, &cached);
    assert!(plain.steps[0].starts_with("Ok("), "{}", plain.steps[0]);
    assert_eq!(plain_device.0.get(), 3);
    assert_eq!(cached_device.0.get(), 3);
    if DATA_CACHE_COUNTED {
        assert_eq!(counters.inserts, 0, "{counters:?}");
        assert_eq!(counters.hits, 0, "{counters:?}");
    }
}

/// The capture modes and a descriptor scope keep the walk: an adapter that captures effects or
/// page fault traces does not consult the cache at all.
#[test]
fn the_capture_modes_do_not_use_the_cache() {
    let memory = bus();
    let ram = &memory;
    let cache = DataTranslationCache::new();
    let mut watch = |range: GuestPhysicalRange| cache.note_write(range);
    let mut adapter = set_loop_adapter(ram, None, paging(), 0, &mut watch, None, None);
    adapter.data_cache = Some(&cache);
    adapter.capture_page_fault_trace = true;
    reset_data_cache_counters();
    let mut bytes = [0u8; 8];
    adapter.read(DATA, ring0(), &mut bytes).unwrap();
    adapter.read(DATA, ring0(), &mut bytes).unwrap();
    assert_eq!(cache.len(), 0);
    assert_eq!(data_cache_counters(), DataCacheCounters::default());
}
