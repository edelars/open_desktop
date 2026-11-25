//! PERF-11 (ADR-0016, prototype) and E8 (writes, reads of 1/2/4 bytes, `--data-cache-verify`):
//! the data translation cache of the set loop's adapter against the same adapter without it. Every test runs the same operations twice on fresh, identical
//! machines and compares, operation by operation, the outcome (bytes or error), the page fault
//! record and the access error, then the sequence of committed writes (the accessed bits of the
//! walks, the guest stores) and the whole RAM.

use super::*;
use crate::data_translation_cache::{
    DATA_CACHE_COUNTED, DataCacheCounters, DataCacheMutations, DataTranslationCache,
    data_cache_counters, reset_data_cache_counters,
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
    /// A read of this many bytes.
    Read(usize),
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
    read_n(address, 8)
}

fn read_n(address: u64, len: usize) -> Op {
    Op {
        address,
        context: ring0(),
        paging: paging(),
        kind: Kind::Read(len),
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
    let cache = DataTranslationCache::new();
    run_with(ram, mmio, ops, cached.then_some(&cache))
}

/// [`run_on`] over the cache `cache` (`None`: no cache), which the caller may have built for the
/// check or mutated.
fn run_with(
    ram: &PhysicalMemory,
    mmio: Option<&dyn crate::bus::SetStepMmio>,
    ops: &[Op],
    cache: Option<&DataTranslationCache>,
) -> (Recorded, DataCacheCounters) {
    reset_data_cache_counters();
    let cached = cache.is_some();
    let mut writes = Vec::new();
    let mut watch = |range: GuestPhysicalRange| {
        if let Some(cache) = cache {
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
            adapter.data_cache = cache;
        }
        let outcome = match &op.kind {
            Kind::Read(len) => {
                let mut bytes = vec![0u8; *len];
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
            "{outcome} | {:?} | {:?} | {:?}",
            adapter.page_fault, adapter.access_error, adapter.memory_error
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
        // Read inserts: the view and `DATA` before the store, `DATA` after it; the store (a miss
        // of a write: the view has a Read entry) makes the Write entry of the view, which the two
        // reads of the view after it hit (E8). Read hits: the second read of each pair before the
        // store, the two reads of the view and the second read of `DATA` after it. The store drops
        // both Read entries: the dirty bit it commits into PTE[0] of the view's own PT (frame
        // 0x4000) drops the view, the stored bytes in frame 0x5000 drop `DATA`.
        assert_eq!(
            (
                counters.inserts,
                counters.hits,
                counters.misses,
                counters.write_flushes
            ),
            (3, 5, 3, 2),
            "{counters:?}"
        );
        assert_eq!(
            (counters.write_misses, counters.write_inserts),
            (1, 1),
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
/// page fault traces does not consult the cache at all, for reads of every width and for writes
/// (E8), and neither does an adapter without the large-RAM profile.
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
    // E8: the writes and the narrow reads too.
    adapter.write(DATA + 8, ring0(), &[1]).unwrap();
    adapter.write(DATA + 8, ring0(), &[2]).unwrap();
    adapter.read(DATA + 8, ring0(), &mut bytes[..2]).unwrap();
    adapter.capture_page_fault_trace = false;
    adapter.capture_effects = true;
    adapter.committed_writes = Some(Vec::new());
    adapter.committed_ad_updates = Some(Vec::new());
    adapter.read(DATA, ring0(), &mut bytes).unwrap();
    adapter.write(DATA + 8, ring0(), &[3]).unwrap();
    adapter.write(DATA + 8, ring0(), &[4]).unwrap();
    adapter.read(DATA, ring0(), &mut bytes).unwrap();
    // Without the large-RAM profile every access keeps its own path (here the refusal of the
    // RAM-only bus), with or without the cache.
    adapter.capture_effects = false;
    adapter.large_ram_profile = false;
    let refused = (
        format!("{:?}", adapter.read(DATA, ring0(), &mut bytes)),
        format!("{:?}", adapter.write(DATA + 8, ring0(), &[5])),
    );
    adapter.data_cache = None;
    assert_eq!(
        refused,
        (
            format!("{:?}", adapter.read(DATA, ring0(), &mut bytes)),
            format!("{:?}", adapter.write(DATA + 8, ring0(), &[5])),
        )
    );
    drop(adapter);
    assert_eq!(cache.len(), 0);
    assert_eq!(data_cache_counters(), DataCacheCounters::default());
}

/// E8 (step 6): reads of 1, 2 and 4 bytes of one page and of a 2 MiB leaf, at the last bytes of
/// the page, faulting ones (not present, reserved bit, U/S) repeated, user reads, and the crossing
/// ones the cache leaves alone (a word at 0xfff, a dword at 0xffe): the same outcome, records,
/// committed writes and RAM as the walks, and the cache serves them (debug builds count).
#[test]
fn narrow_reads_through_the_cache_equal_the_walks() {
    let user = |op: Op| Op {
        context: ring3(),
        ..op
    };
    let mut ops = Vec::new();
    for len in [1usize, 2, 4] {
        ops.extend([
            read_n(DATA, len),
            read_n(DATA + 0x11, len),
            read_n(DATA + 0x1000 - len as u64, len),
            user(read_n(DATA + 0x23, len)),
            user(read_n(DATA + 0x23, len)),
            read_n(DATA + 0x2000, len),
            user(read_n(DATA + 0x2000, len)),
            user(read_n(DATA + 0x2000, len)),
            read_n(DATA + 0x3000, len),
            read_n(DATA + 0x3000, len),
            read_n(DATA + 0x4000, len),
            read_n(DATA + 0x4000, len),
            read_n(LARGE + 0x1235, len),
            read_n(LARGE + 0x1235, len),
            read_n(0x0000_8000_0000_0000, len),
        ]);
    }
    // A qword read of a page then narrow reads of it hit the same entry, and the other way round.
    ops.extend([
        read(DATA + 0x1000),
        read_n(DATA + 0x1001, 1),
        read_n(DATA + 0x1100, 4),
    ]);
    ops.extend([read_n(DATA + 0x5000, 2), read(DATA + 0x5000)]);
    // Not served: crossing (split per byte), descriptor-table reads.
    ops.extend([
        read_n(DATA + 0xfff, 2),
        read_n(DATA + 0xffe, 4),
        Op {
            context: ring0().descriptor_table(),
            ..read_n(DATA + 0x40, 4)
        },
    ]);
    let counters = equivalent(bus, &ops);
    if DATA_CACHE_COUNTED {
        assert!(counters.hits >= 15, "{counters:?}");
        assert!(counters.inserts >= 10, "{counters:?}");
    }
}

/// E8 (steps 5, 10; ADR-0016 section 5, tests 3 and 4): writes of 1, 2, 4 and 8 bytes. A write
/// after a read of the page walks (a Read entry never serves a write) and sets D exactly once, as
/// without the cache; its Write entry then serves the reads and writes of the page; faulting writes
/// (read-only page under CR0.WP, a supervisor page at CPL3, not present, reserved bit) repeat their
/// fault, a write into a read-only page without CR0.WP is another key; a crossing write is split.
#[test]
fn writes_through_the_cache_equal_the_walks() {
    let user = |op: Op| Op {
        context: ring3(),
        ..op
    };
    let no_wp = Long64ProbeConfig::new(CR0 & !CR0_WP, ROOT, CR4, EFER).unwrap();
    let without_wp = |op: Op| Op {
        paging: no_wp,
        ..op
    };
    let ops = vec![
        read(DATA),
        read(DATA + 8),
        write(DATA + 0x10, &[0x11]),
        write(DATA + 0x12, &[0x22, 0x33]),
        write(DATA + 0x14, &0x4455_6677u32.to_le_bytes()),
        write(DATA + 0x18, &0x8899_aabb_ccdd_eeffu64.to_le_bytes()),
        write(DATA + 0xfff, &[0x5a]),
        write(DATA + 0xff8, &0x0102_0304_0506_0708u64.to_le_bytes()),
        read(DATA + 0x10),
        read_n(DATA + 0x12, 2),
        read(DATA + 0xff8),
        // The first access of a page is a write: one walk (A on every level, D on the leaf).
        write(DATA + 0x1008, &[1, 2, 3, 4]),
        write(DATA + 0x100c, &[5, 6, 7, 8]),
        read(DATA + 0x1008),
        // A 2 MiB leaf: D in the PDE.
        write(LARGE + 0x1_2340, &0x1234u16.to_le_bytes()),
        write(LARGE + 0x1_2342, &0x5678u16.to_le_bytes()),
        read(LARGE + 0x1_2340),
        // PTE[5]: read-only. CPL0 under CR0.WP faults (every time); without CR0.WP it writes.
        read(DATA + 0x5000),
        write(DATA + 0x5000, &[9]),
        write(DATA + 0x5000, &[9]),
        without_wp(write(DATA + 0x5000, &[9])),
        without_wp(write(DATA + 0x5001, &[10])),
        write(DATA + 0x5000, &[9]),
        // A user page written at CPL3 and then at CPL0: two keys.
        user(write(DATA + 0x1010, &[0x77])),
        user(write(DATA + 0x1011, &[0x78])),
        write(DATA + 0x1012, &[0x79]),
        // The supervisor page at CPL3: #PF each time; at CPL0 it is written.
        user(write(DATA + 0x2000, &[1])),
        write(DATA + 0x2000, &[1]),
        user(write(DATA + 0x2000, &[1])),
        write(DATA + 0x2001, &[2]),
        // Not present, reserved bit, non-canonical: the same fault each time.
        write(DATA + 0x3000, &[1]),
        write(DATA + 0x3000, &[1]),
        write(DATA + 0x4000, &[1]),
        write(DATA + 0x4000, &[1]),
        write(0x0000_8000_0000_0000, &[1]),
        // Across the page end: split, not served.
        write(DATA + 0xffe, &0x1111_2222u32.to_le_bytes()),
        read(DATA + 0xff8),
    ];
    let counters = equivalent(bus, &ops);
    let (cached, _) = run_on(&bus(), None, &ops, true);
    for index in [18, 19, 22, 26, 28, 30, 31, 32, 33] {
        assert!(
            cached.steps[index].contains("PageFault"),
            "{index}: {}",
            cached.steps[index]
        );
    }
    if DATA_CACHE_COUNTED {
        assert!(counters.write_hits >= 8, "{counters:?}");
        assert!(counters.write_inserts >= 5, "{counters:?}");
    }
}

/// E8 (ADR-0016 section 5, test 4): a Read entry never serves a write: the write after reads of
/// the page walks and commits D once (the committed writes are those of the walks); then the Write
/// entry replaces the Read one and serves both kinds.
#[test]
fn a_read_entry_never_serves_a_write_and_the_write_entry_replaces_it() {
    let memory = bus();
    let ram = &memory;
    let cache = DataTranslationCache::new();
    let writes = std::cell::RefCell::new(Vec::new());
    let mut watch = |range: GuestPhysicalRange| {
        cache.note_write(range);
        writes.borrow_mut().push(range.start().get());
    };
    let mut bytes = [0u8; 8];
    {
        let mut adapter = set_loop_adapter(ram, None, paging(), 0, &mut watch, None, None);
        adapter.data_cache = Some(&cache);
        adapter.read(DATA, ring0(), &mut bytes).unwrap();
        adapter.read(DATA, ring0(), &mut bytes).unwrap();
    }
    assert_eq!((cache.len(), cache.write_len()), (1, 0));
    // The four accessed bits of the first walk, then nothing for the hit.
    assert_eq!(*writes.borrow(), [ROOT, 0x2000, 0x3020, 0x5000]);
    writes.borrow_mut().clear();
    {
        let mut adapter = set_loop_adapter(ram, None, paging(), 0, &mut watch, None, None);
        adapter.data_cache = Some(&cache);
        adapter.write(DATA + 8, ring0(), &[0xaa]).unwrap();
    }
    // The write walked: D of PTE[0] (0x5000), then the byte.
    assert_eq!(*writes.borrow(), [0x5000, 0x7008]);
    assert_eq!((cache.len(), cache.write_len()), (1, 1));
    writes.borrow_mut().clear();
    {
        let mut adapter = set_loop_adapter(ram, None, paging(), 0, &mut watch, None, None);
        adapter.data_cache = Some(&cache);
        adapter.write(DATA + 9, ring0(), &[0xbb]).unwrap();
        adapter.read(DATA + 8, ring0(), &mut bytes).unwrap();
    }
    // A write hit: the byte only; the read hit writes nothing.
    assert_eq!(*writes.borrow(), [0x7009]);
    assert_eq!(bytes[..2], [0xaa, 0xbb]);
    let pte = ram.load_u64(GuestPhysicalAddress::new(0x5000)).unwrap();
    assert_eq!(pte & 0x60, 0x60, "A and D of the PTE: {pte:#x}");
}

/// E8 (step 10; ADR-0016 section 5, test 5): a write through a hit into a table frame of the path
/// of another live entry drops it (the next access walks and sees the new translation); a write
/// into a page whose frame is a table frame of its own path makes no Write entry (decision Д4,
/// item 3): here the write through the view of the PD clears P of the PDE of that view itself, and
/// the next write of the page must fault. The mutation that drops the rule is seen. The victim is
/// `DATA + 0x1000` (PTE[1], set 1), whose set is not the one of the view (set 0).
#[test]
fn a_write_hit_into_a_table_frame_drops_the_entries_that_watch_it() {
    let victim = DATA + 0x1000;
    let pd_view = TABLE_VIEW + 0x1000;
    let ops = vec![
        read(victim),
        write(victim + 8, &[1]),
        // The view of the PT of `DATA` warmed by a write (its first walk commits A into the PD
        // frame and drops the victim, which watches it); then the victim again: both live.
        write(TABLE_VIEW + 0x30, &[0]),
        write(victim + 9, &[2]),
        // A write hit through the view into PTE[1]: the victim's entry is dropped.
        write(TABLE_VIEW + 8, &0x9067u64.to_le_bytes()),
        write(victim + 8, &[3]),
        read(victim),
        // The view of the PD (a table of its own path): PD[2], the PDE of both views, loses P.
        write(pd_view + 0x10, &[0]),
        write(pd_view, &[0]),
        write(TABLE_VIEW, &[0]),
    ];
    let counters = equivalent(bus, &ops);
    let (cached, _) = run_on(&bus(), None, &ops, true);
    let frame_9000 = ((0x9000u64 << 20) ^ 0x33).to_le_bytes();
    assert!(
        cached.steps[6].starts_with(&format!("Ok({frame_9000:?})")),
        "{}",
        cached.steps[6]
    );
    for index in [8, 9] {
        assert!(
            cached.steps[index].contains("PageFault"),
            "{}",
            cached.steps[index]
        );
    }
    if DATA_CACHE_COUNTED {
        // Write hits: `victim + 9` is a miss (the view dropped it), the PTE store through the
        // view is the only hit; no Write entry for the page of the PD.
        assert_eq!(counters.write_hits, 1, "{counters:?}");
        assert_eq!(counters.write_inserts, 4, "{counters:?}");
    }
    let mutated = DataTranslationCache::new();
    mutated.mutate_for_test(DataCacheMutations {
        own_table_frame_allowed: true,
        ..Default::default()
    });
    let (plain, _) = run_on(&bus(), None, &ops, false);
    let (with_mutation, _) = run_with(&bus(), None, &ops, Some(&mutated));
    assert!(
        plain.steps != with_mutation.steps || plain.ram != with_mutation.ram,
        "the mutation must be seen"
    );
}

/// E8 (step 8): the accessed and dirty checks of `--data-cache-verify`: the D bit, then the A bit
/// of the leaf of a live Write entry cleared behind the loop's back (a store that bypasses
/// `watch`, the hazard of decision Д6): the next write hit, then the next read hit, is a
/// divergence named by its bit, and nothing is stored.
#[test]
fn the_check_sees_a_cleared_accessed_or_dirty_bit() {
    use crate::data_translation_cache::check;
    for (clear, write_hit, bit) in [(0x40u64, true, "dirty"), (0x20, false, "accessed")] {
        let memory = bus();
        let ram = &memory;
        let cache = DataTranslationCache::checking();
        let mut watch = |range: GuestPhysicalRange| cache.note_write(range);
        let mut adapter = set_loop_adapter(ram, None, paging(), 0, &mut watch, None, None);
        adapter.data_cache = Some(&cache);
        adapter.write(DATA + 0x1008, ring0(), &[1]).unwrap();
        let pte = ram.load_u64(GuestPhysicalAddress::new(0x5008)).unwrap();
        assert_eq!(pte & 0x60, 0x60, "{pte:#x}");
        ram.store_bytes(
            GuestPhysicalAddress::new(0x5008),
            &(pte & !clear).to_le_bytes(),
            |_| {},
        )
        .unwrap();
        check::reset_data_cache_check();
        let before = ram.load_u8(GuestPhysicalAddress::new(0x8009)).unwrap();
        let outcome = if write_hit {
            adapter.write(DATA + 0x1009, ring0(), &[0xee])
        } else {
            adapter.read(DATA + 0x1009, ring0(), &mut [0u8; 1])
        };
        assert!(
            matches!(outcome, Err(MemoryError::Backend(_))),
            "{outcome:?}"
        );
        drop(adapter);
        let divergence = check::take_data_cache_check_divergence().expect("a divergence");
        assert!(divergence.reason.contains(bit), "{divergence:?}");
        assert_eq!(divergence.write, write_hit);
        assert_eq!(check::data_cache_check_counters().divergences, 1);
        assert_eq!(
            ram.load_u8(GuestPhysicalAddress::new(0x8009)).unwrap(),
            before
        );
        assert_eq!(
            ram.load_u64(GuestPhysicalAddress::new(0x5008)).unwrap(),
            pte & !clear,
            "the check wrote nothing"
        );
    }
}

/// E8 (step 10): a read or a write the profile does not admit (here a byte read and a byte write
/// of an adapter without the narrow widths) is the same `Unsupported` refusal with the cache, and
/// makes no entry.
#[test]
fn a_width_the_profile_refuses_is_refused_the_same_with_the_cache() {
    let run = |cached: bool| {
        let memory = bus();
        let ram = &memory;
        let cache = DataTranslationCache::new();
        let mut watch = |range: GuestPhysicalRange| cache.note_write(range);
        let mut adapter = set_loop_adapter(ram, None, paging(), 0, &mut watch, None, None);
        if cached {
            adapter.data_cache = Some(&cache);
        }
        adapter.cpl0_set_byte_read = false;
        adapter.cpl0_set_word_read = false;
        adapter.cpl0_set_narrow_write = false;
        let mut byte = [0u8; 1];
        let mut word = [0u8; 2];
        let outcomes = [
            format!("{:?}", adapter.read(DATA, ring0(), &mut byte)),
            format!("{:?}", adapter.read(DATA, ring0(), &mut word)),
            format!("{:?}", adapter.write(DATA, ring0(), &[1])),
            format!("{:?}", adapter.write(DATA, ring0(), &[1, 2, 3, 4])),
            format!("{:?}", adapter.read(DATA, ring0(), &mut [0u8; 4])),
        ];
        drop(adapter);
        (outcomes, cache.len())
    };
    let (plain, _) = run(false);
    let (cached, entries) = run(true);
    assert_eq!(plain, cached);
    for outcome in &plain[..4] {
        assert!(
            outcome.starts_with("Err(Backend(\"paged MOV profile does not support"),
            "{outcome}"
        );
    }
    assert!(plain[4].starts_with("Ok"), "{}", plain[4]);
    // Only the admitted dword read made an entry.
    assert_eq!(entries, 1);
}

/// E8 (decisions Д2, Д3): a stack-frame write of a delivery (`ImplicitStack`) and a
/// descriptor-table write are never served and make no Write entry: every one walks.
#[test]
fn implicit_stack_and_descriptor_table_writes_walk() {
    let stack = |op: Op| Op {
        context: AccessContext {
            origin: AccessOrigin::ImplicitStack,
            ..ring0()
        },
        ..op
    };
    let ops = vec![
        stack(write(DATA + 0x40, &0x1122u64.to_le_bytes())),
        stack(write(DATA + 0x48, &0x3344u64.to_le_bytes())),
        stack(write(DATA + 0x40, &0x5566u64.to_le_bytes())),
        read(DATA + 0x40),
    ];
    let counters = equivalent(bus, &ops);
    if DATA_CACHE_COUNTED {
        assert_eq!(counters.write_inserts, 0, "{counters:?}");
        assert_eq!(counters.write_hits, 0, "{counters:?}");
    }
    let memory = bus();
    let ram = &memory;
    let cache = DataTranslationCache::new();
    let mut watch = |range: GuestPhysicalRange| cache.note_write(range);
    let mut adapter = set_loop_adapter(ram, None, paging(), 0, &mut watch, None, None);
    adapter.data_cache = Some(&cache);
    let descriptor = ring0().descriptor_table();
    // Without a checked read first the adapter refuses a descriptor write; with or without the
    // cache it never becomes a Write entry.
    let _ = adapter.write(DATA + 0x50, descriptor, &[0x9b]);
    let _ = adapter.write(DATA + 0x50, descriptor, &[0x9b]);
    drop(adapter);
    assert_eq!(cache.write_len(), 0);
}

/// E8 (step 10; ADR-0016 section 5, test 6): ROM, framebuffer and MMIO operands of a write are
/// never remembered: the ROM write fails each time, the framebuffer write walks each time, the
/// device is called at every write.
#[test]
fn rom_framebuffer_and_mmio_writes_are_never_cached() {
    let base = RAM_LEN;
    let make = || {
        let bus = memory_with(vec![
            MemoryRegionSpec::new(
                GuestPhysicalRange::new(GuestPhysicalAddress::new(base), 0x1000).unwrap(),
                MemoryRegionKind::Rom,
            ),
            MemoryRegionSpec::new(
                GuestPhysicalRange::new(GuestPhysicalAddress::new(base + 0x1000), 0x2000).unwrap(),
                MemoryRegionKind::Framebuffer,
            ),
        ]);
        put(&bus, 0x3038, 0x6007);
        put(&bus, 0x6000, base | 7);
        put(&bus, 0x6008, (base + 0x1000) | 7);
        bus
    };
    let ops = vec![
        write(OTHER_REGION, &[1]),
        write(OTHER_REGION, &[1]),
        write(OTHER_REGION + 0x1000, &[2, 3]),
        write(OTHER_REGION + 0x1002, &[4, 5]),
        read_n(OTHER_REGION + 0x1000, 4),
        write(DATA, &[6]),
        write(DATA, &[7]),
    ];
    let counters = equivalent(make, &ops);
    if DATA_CACHE_COUNTED {
        // Only `DATA` is remembered.
        assert_eq!(counters.write_inserts, 1, "{counters:?}");
        assert_eq!(counters.write_hits, 1, "{counters:?}");
        assert_eq!(counters.inserts, 0, "{counters:?}");
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
            Ok(0)
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
        write(DATA + 0x6000, &0x11u32.to_le_bytes()),
        write(DATA + 0x6000, &0x22u32.to_le_bytes()),
        write(DATA + 0x6000, &0x33u32.to_le_bytes()),
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
        assert_eq!(counters.write_inserts, 0, "{counters:?}");
    }
}

/// E8 (step 10; ADR-0016 section 5, test 1): a change of CR3, CR0.WP, EFER.NXE or the privilege is
/// never a false hit of a Write entry.
#[test]
fn another_root_write_protection_nxe_or_privilege_is_never_a_false_write_hit() {
    let other_root = Long64ProbeConfig::new(CR0, OTHER_ROOT, CR4, EFER).unwrap();
    let no_wp = Long64ProbeConfig::new(CR0 & !CR0_WP, ROOT, CR4, EFER).unwrap();
    let no_nxe = Long64ProbeConfig::new(CR0, ROOT, CR4, EFER & !(1 << 11)).unwrap();
    let with = |paging: Long64ProbeConfig, op: Op| Op { paging, ..op };
    let user = |op: Op| Op {
        context: ring3(),
        ..op
    };
    let ops = vec![
        write(DATA, &[1]),
        write(DATA, &[2]),
        // The second root maps `DATA` to 0x8000.
        with(other_root, write(DATA, &[3])),
        with(other_root, write(DATA, &[4])),
        write(DATA + 1, &[5]),
        with(no_wp, write(DATA + 2, &[6])),
        with(no_nxe, write(DATA + 3, &[7])),
        // The supervisor page: written at CPL0, a #PF at CPL3 after it.
        write(DATA + 0x2000, &[8]),
        user(write(DATA + 0x2000, &[9])),
        // The read-only page under no WP, then under WP: the fault comes back.
        with(no_wp, write(DATA + 0x5000, &[10])),
        write(DATA + 0x5000, &[11]),
        read(DATA),
        with(other_root, read(DATA)),
    ];
    let counters = equivalent(bus, &ops);
    let (cached, _) = run_on(&bus(), None, &ops, true);
    for index in [8, 10] {
        assert!(
            cached.steps[index].contains("PageFault"),
            "{}",
            cached.steps[index]
        );
    }
    if DATA_CACHE_COUNTED {
        assert!(counters.write_hits >= 2, "{counters:?}");
        assert!(counters.write_inserts >= 5, "{counters:?}");
    }
}

/// E8 (step 8): the check of `--data-cache-verify` changes nothing (the run is the run without
/// the cache), checks every hit of both kinds and finds no divergence.
#[test]
fn the_check_changes_nothing_and_finds_no_divergence() {
    use crate::data_translation_cache::check;
    let ops = vec![
        read(DATA),
        read(DATA),
        read_n(DATA + 3, 1),
        write(DATA + 8, &[1, 2]),
        write(DATA + 8, &[3, 4]),
        read(DATA + 8),
        write(TABLE_VIEW, &0x8067u64.to_le_bytes()),
        read(DATA),
        write(DATA, &[5]),
        write(LARGE + 0x10, &[6]),
        write(LARGE + 0x11, &[7]),
        read_n(LARGE + 0x10, 2),
    ];
    let (plain, _) = run_on(&bus(), None, &ops, false);
    check::reset_data_cache_check();
    let cache = DataTranslationCache::checking();
    let (checked, _) = run_with(&bus(), None, &ops, Some(&cache));
    assert_same(&plain, &checked);
    let counters = check::data_cache_check_counters();
    assert_eq!(counters.divergences, 0, "{counters:?}");
    assert!(
        counters.verified_reads >= 3 && counters.verified_writes >= 2,
        "{counters:?}"
    );
    assert_eq!(check::take_data_cache_check_divergence(), None);
}

/// E8 (step 10): the mutation "a Read entry serves a write" (the dirty bit never checked) is seen:
/// without the check the write leaves D clear (the RAM and the committed writes differ from the
/// walks), with the check the hit is a divergence and the write fails before any effect.
#[test]
fn the_mutation_read_entry_serves_a_write_is_seen() {
    use crate::data_translation_cache::check;
    let ops = vec![read(DATA), write(DATA + 8, &[1]), read(DATA + 8)];
    let mutations = DataCacheMutations {
        read_serves_write: true,
        ..Default::default()
    };
    let (plain, _) = run_on(&bus(), None, &ops, false);
    let cache = DataTranslationCache::new();
    cache.mutate_for_test(mutations);
    let (mutated, _) = run_with(&bus(), None, &ops, Some(&cache));
    assert!(
        plain.writes != mutated.writes && plain.ram != mutated.ram,
        "the mutation must be seen"
    );
    check::reset_data_cache_check();
    let cache = DataTranslationCache::checking();
    cache.mutate_for_test(mutations);
    let (checked, _) = run_with(&bus(), None, &ops, Some(&cache));
    assert!(
        checked.steps[1].starts_with("Err(Backend"),
        "{}",
        checked.steps[1]
    );
    let counters = check::data_cache_check_counters();
    assert_eq!(counters.divergences, 1, "{counters:?}");
    let divergence = check::take_data_cache_check_divergence().expect("a divergence");
    assert!(
        divergence.write && divergence.linear == DATA + 8,
        "{divergence:?}"
    );
    // Nothing was written by the refused write: the byte is the one before it.
    let fresh = bus();
    assert_eq!(
        checked.ram[0x7008],
        fresh.load_u8(GuestPhysicalAddress::new(0x7008)).unwrap()
    );
}

/// E8 (step 10): the mutation "a store into a watched table frame does not drop the Write entries"
/// is seen: after PTE[1] of `DATA` is changed through its view (the page `DATA + 0x1000`, whose set
/// is not the one of the view), a write of that page goes to the old frame; the check finds the
/// divergence (the walk gives another frame).
#[test]
fn the_mutation_unwatched_write_entries_is_seen() {
    use crate::data_translation_cache::check;
    let ops = vec![
        write(DATA + 0x1000, &[1]),
        write(TABLE_VIEW + 8, &0x9067u64.to_le_bytes()),
        write(DATA + 0x1001, &[2]),
        read(DATA + 0x1000),
    ];
    let mutations = DataCacheMutations {
        write_entries_unwatched: true,
        ..Default::default()
    };
    let (plain, _) = run_on(&bus(), None, &ops, false);
    let cache = DataTranslationCache::new();
    cache.mutate_for_test(mutations);
    let (mutated, _) = run_with(&bus(), None, &ops, Some(&cache));
    assert!(plain.ram != mutated.ram, "the mutation must be seen");
    check::reset_data_cache_check();
    let cache = DataTranslationCache::checking();
    cache.mutate_for_test(mutations);
    let (checked, _) = run_with(&bus(), None, &ops, Some(&cache));
    assert!(
        checked.steps[2].starts_with("Err(Backend"),
        "{}",
        checked.steps[2]
    );
    let divergence = check::take_data_cache_check_divergence().expect("a divergence");
    assert!(divergence.reason.contains("frame"), "{divergence:?}");
}
