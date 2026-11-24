use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering, fence};

use crate::AccessWidth;
use crate::ram_storage::RamStorage;

/// S6-32: the host order of every ordinary RAM load and store (scalar, split into bytes, bulk
/// copy, frame byte): parallel `SeqCst` (the choice of S6-06, MEMORY_MODEL.md "Выбор по замеру"),
/// interleaving `Relaxed` (no barrier: one host thread runs every vCPU and every writer). Locked
/// operations and generations keep their own orders in both modes. The macro runs `$body` with
/// `$order` bound to that order: one branch on the mode, a constant order in each arm. A
/// runtime-valued `Ordering` passed to an atomic access costs a second branch per access (a first
/// version did that and `page_walk_compare` showed it, see the S6-32 report); with the constant
/// the compiler emits a plain load or store.
macro_rules! by_mode {
    ($mode:expr, $order:ident => $body:expr) => {
        match $mode {
            ExecutionMode::Parallel => {
                // S6-36, `weak-host-ordering` (tests and measurements only, never in `default`):
                // the parallel mode with NO barrier, to show that the host test of SMP.md 5.1 can
                // see the weakening (or, on a host that does not reorder these, that it cannot).
                #[cfg(feature = "weak-host-ordering")]
                let $order = Ordering::Relaxed;
                #[cfg(not(feature = "weak-host-ordering"))]
                let $order = Ordering::SeqCst;
                $body
            }
            ExecutionMode::Interleaved => {
                let $order = Ordering::Relaxed;
                $body
            }
        }
    };
}

/// The execution mode of the machine (S6-32), the one switch (`PhysicalMemory::set_execution_mode`,
/// reached through the board). The interpreter (`od-isa`) does not know it. The mode orders the
/// ordinary scalar accesses and the bulk copies (each host access of a copy), and sets the host
/// fence of the guest fences.
///
/// - `Interleaved` (the default and the only mode that runs a machine today): every vCPU and every
///   RAM writer runs on one host thread and switches only on instruction/event boundaries. Ordinary
///   scalar RAM accesses are `Relaxed` (no barrier) and the guest fences place no host fence.
/// - `Parallel` (S6-34 starts vCPU threads; until then it is selectable and single-threaded
///   runs are byte-identical to the interleaving mode): ordinary scalar RAM accesses are `SeqCst`
///   (the choice of S6-06) and the guest fences place a host fence (`MFENCE` `SeqCst`, `SFENCE`
///   `Release`, `LFENCE` `Acquire`).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ExecutionMode {
    #[default]
    Interleaved,
    Parallel,
}

/// A guest physical byte address. It is deliberately distinct from host pointers.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct GuestPhysicalAddress(u64);

impl GuestPhysicalAddress {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

/// A non-empty half-open guest physical byte range.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GuestPhysicalRange {
    start: GuestPhysicalAddress,
    len: u64,
}

impl GuestPhysicalRange {
    pub fn new(start: GuestPhysicalAddress, len: u64) -> Result<Self, MemoryAccessError> {
        if len == 0 {
            return Err(MemoryAccessError::EmptyRange);
        }
        let end_exclusive = u128::from(start.0) + u128::from(len);
        if end_exclusive > (1u128 << 64) {
            return Err(MemoryAccessError::AddressOverflow);
        }
        Ok(Self { start, len })
    }

    pub const fn start(self) -> GuestPhysicalAddress {
        self.start
    }

    pub const fn len(self) -> u64 {
        self.len
    }

    /// Exclusive endpoint in an extended integer domain, allowing the final
    /// byte at `u64::MAX` to form a valid one-byte range.
    pub const fn end_exclusive(self) -> u128 {
        self.start.0 as u128 + self.len as u128
    }

    fn contains(self, addr: GuestPhysicalAddress, len: u64) -> bool {
        let end = u128::from(addr.0) + u128::from(len);
        end <= (1u128 << 64) && addr.0 >= self.start.0 && end <= self.end_exclusive()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryRegionKind {
    Ram,
    Rom,
    Reserved,
    Mmio,
    /// Device-owned pixel memory. It has its own backing outside [`RamLimits`],
    /// readable and writable as data without committed-write notification.
    /// Instruction fetch and page-table structures are rejected by consumers.
    Framebuffer,
}

/// Upper bound for the length of a `Framebuffer` region (36 MiB).
pub(crate) const MAX_FRAMEBUFFER_BYTES: u64 = 0x0240_0000;
/// Two screens at most (DISPLAY_DEVICE.md 16.3).
pub(crate) const MAX_FRAMEBUFFER_REGIONS: usize = 2;
const FRAMEBUFFER_ALIGNMENT: u64 = 0x1000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryRegionSpec {
    pub range: GuestPhysicalRange,
    pub kind: MemoryRegionKind,
}

impl MemoryRegionSpec {
    pub const fn new(range: GuestPhysicalRange, kind: MemoryRegionKind) -> Self {
        Self { range, kind }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PhysicalWidthError {
    RangeOutsideWidth(GuestPhysicalRange),
}

impl fmt::Display for PhysicalWidthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RangeOutsideWidth(range) => write!(
                f,
                "physical range {:#x}+{:#x} exceeds configured address width",
                range.start.0, range.len
            ),
        }
    }
}

impl std::error::Error for PhysicalWidthError {}

/// A validated, sorted and non-overlapping map. Guest page granularity is
/// intentionally not tied to host virtual-memory page granularity.
#[derive(Clone, Debug)]
pub struct MachineMemoryMap {
    physical_address_width: u8,
    regions: Vec<MemoryRegionSpec>,
}

impl MachineMemoryMap {
    pub fn new(
        physical_address_width: u8,
        mut regions: Vec<MemoryRegionSpec>,
    ) -> Result<Self, MemoryAccessError> {
        if !(1..=64).contains(&physical_address_width) {
            return Err(MemoryAccessError::InvalidPhysicalWidth(
                physical_address_width,
            ));
        }
        regions.sort_by_key(|region| region.range.start.0);
        for region in &regions {
            if !range_fits_width(region.range, physical_address_width) {
                return Err(MemoryAccessError::PhysicalWidth(
                    PhysicalWidthError::RangeOutsideWidth(region.range),
                ));
            }
        }
        for pair in regions.windows(2) {
            if pair[0].range.end_exclusive() > u128::from(pair[1].range.start.0) {
                return Err(MemoryAccessError::OverlappingRegions {
                    first: pair[0].range,
                    second: pair[1].range,
                });
            }
        }
        // Version 2 with two screens has two framebuffer regions (DISPLAY_DEVICE.md 16.3);
        // a third one is rejected before any allocation.
        let framebuffers: Vec<&MemoryRegionSpec> = regions
            .iter()
            .filter(|region| region.kind == MemoryRegionKind::Framebuffer)
            .collect();
        if framebuffers.len() > MAX_FRAMEBUFFER_REGIONS {
            return Err(MemoryAccessError::MultipleFramebufferRegions);
        }
        for region in framebuffers {
            if region.range.start.0 % FRAMEBUFFER_ALIGNMENT != 0
                || region.range.len % FRAMEBUFFER_ALIGNMENT != 0
                || region.range.len > MAX_FRAMEBUFFER_BYTES
            {
                return Err(MemoryAccessError::InvalidFramebufferRegion(region.range));
            }
        }
        Ok(Self {
            physical_address_width,
            regions,
        })
    }

    pub const fn physical_address_width(&self) -> u8 {
        self.physical_address_width
    }

    pub fn regions(&self) -> &[MemoryRegionSpec] {
        &self.regions
    }
}

fn range_fits_width(range: GuestPhysicalRange, width: u8) -> bool {
    if width == 64 {
        return true;
    }
    let maximum = (1u64 << width) - 1;
    range.start.0 <= maximum && range.end_exclusive() - 1 <= u128::from(maximum)
}

/// Explicit upper bound on RAM backing owned by one physical memory instance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RamLimits {
    pub max_total_bytes: u64,
}

impl RamLimits {
    pub const fn new(max_total_bytes: u64) -> Self {
        Self { max_total_bytes }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MemoryAccessError {
    EmptyRange,
    AddressOverflow,
    InvalidPhysicalWidth(u8),
    PhysicalWidth(PhysicalWidthError),
    OverlappingRegions {
        first: GuestPhysicalRange,
        second: GuestPhysicalRange,
    },
    /// A map may contain at most two `Framebuffer` regions, one per screen (DISPLAY_DEVICE.md 16.3);
    /// a third one is rejected before any allocation.
    MultipleFramebufferRegions,
    /// A `Framebuffer` region must start and end on 4 KiB boundaries and be
    /// at most 36 MiB long.
    InvalidFramebufferRegion(GuestPhysicalRange),
    AllocationFailed {
        bytes: u64,
    },
    ResourceLimit {
        requested_bytes: u64,
        max_bytes: u64,
    },
    CodeGenerationExhausted,
    Unmapped(GuestPhysicalAddress),
    AccessCrossesRegion(GuestPhysicalRange),
    UnreadableRegion(MemoryRegionKind),
    ReadOnlyRegion(MemoryRegionKind),
    /// S6-31: a locked (atomic) operation of `width` bytes at an address that is not naturally
    /// aligned to its width. It is refused and counted ([`PhysicalMemory::split_lock_refusals`]);
    /// it is never executed non-atomically (the split-lock path, including the pair of 16-byte
    /// operations for an operand inside one aligned 16-byte block, is not implemented: see
    /// `MACHINE_CONTRACT.md`, S6-31).
    MisalignedAtomic {
        address: GuestPhysicalAddress,
        width: u8,
    },
}

impl fmt::Display for MemoryAccessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "guest physical memory error: {self:?}")
    }
}

impl std::error::Error for MemoryAccessError {}

struct Region {
    spec: MemoryRegionSpec,
    backing: Option<RamStorage>,
    /// S6-30b: one atomic counter per 4 KiB page the region touches (RAM only). Raised with
    /// `Release` after the data of a write has been stored, read with `Acquire` (see
    /// [`PhysicalMemory::code_page_generation`]); every access is through `&self`.
    page_generations: Vec<AtomicU64>,
}

/// S6-41c: what generated code needs for one inline atomic operation on a RAM operand
/// ([`PhysicalMemory::atomic_operand_window`]). It borrows the memory, so it cannot outlive it;
/// the raw pointer it carries is for one synchronous call and is accessed only by host-atomic
/// instructions of the operand's width (amendment (в′) of S6-30a).
#[derive(Clone, Copy, Debug)]
pub struct AtomicOperandWindow<'m> {
    host: *mut u8,
    generation: &'m AtomicU64,
    range: GuestPhysicalRange,
}

impl<'m> AtomicOperandWindow<'m> {
    /// Host address of the operand: naturally aligned for `range().len()`.
    pub fn host_ptr(&self) -> *mut u8 {
        self.host
    }

    /// The generation counter of the page of the operand, to be raised with a `Release`
    /// read-modify-write after the data (the pointer is valid as long as the memory lives).
    pub fn generation_ptr(&self) -> *const AtomicU64 {
        self.generation
    }

    /// The generation counter, for a caller that raises it itself.
    pub fn generation(&self) -> &'m AtomicU64 {
        self.generation
    }

    /// The guest physical range of the operand (what `committed_write` is told after the write).
    pub fn range(&self) -> GuestPhysicalRange {
        self.range
    }
}

/// Handle to one plain-RAM 4 KiB frame of a [`PhysicalMemory`] (see `PhysicalMemory::ram_frame`),
/// or, with `large` set, to one 2 MiB-aligned plain-RAM span (`PhysicalMemory::ram_leaf_2m`, S4-13d).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RamFrame {
    region: usize,
    offset: usize,
    large: bool,
}

/// PERF-10: a byte of plain RAM named by (region, offset inside the region), with the generation of
/// its 4 KiB page read at that moment (see `PhysicalMemory::ram_frame_code_page`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RamCodePage {
    pub(crate) region: usize,
    pub(crate) offset: usize,
    pub(crate) generation: u64,
}

impl RamFrame {
    /// A 2 MiB span from `PhysicalMemory::ram_leaf_2m` (not a 4 KiB frame).
    pub(crate) fn is_large(self) -> bool {
        self.large
    }
}

/// The sole owner of production RAM backing in this crate.
///
/// S6-30c: every access (`load_*`, `store_*`, `compare_replace_u64`) takes `&self`; there is no
/// method that hands out `&mut` to RAM bytes, and the old `&mut self` path (`read_*`/`write_*`)
/// is gone. A vCPU adapter holds a shared reference, so any number of adapters share one memory.
/// The surface is pinned by the doctests below (a call through a shared reference compiles, the
/// removed names do not exist):
///
/// ```
/// use od_machine::{GuestPhysicalAddress, PhysicalMemory};
/// fn through_a_shared_reference(memory: &PhysicalMemory, at: GuestPhysicalAddress) {
///     let _ = memory.store_u64(at, 1, |_| {});
///     let _ = memory.compare_replace_u64(at, 1, 2, |_| {});
///     let _ = memory.load_u64(at);
/// }
/// ```
///
/// ```compile_fail,E0599
/// use od_machine::{GuestPhysicalAddress, PhysicalMemory};
/// fn removed(memory: &PhysicalMemory, at: GuestPhysicalAddress) {
///     let _ = memory.read_u8(at);
/// }
/// ```
///
/// ```compile_fail,E0599
/// use od_machine::{GuestPhysicalAddress, PhysicalMemory};
/// fn removed(memory: &mut PhysicalMemory, at: GuestPhysicalAddress) {
///     let _ = memory.write_bytes(at, &[1], |_| {});
/// }
/// ```
pub struct PhysicalMemory {
    regions: Vec<Region>,
    /// S6-32: the machine's execution mode. It chooses the host memory order of ordinary scalar
    /// accesses and of bulk copies, and the host fence of the guest fences. Set once, before the memory is shared (`&mut self`), so
    /// no access reads it through an atomic. The default is [`ExecutionMode::Interleaved`].
    mode: ExecutionMode,
    /// The counters that are written while vCPU threads run, on cache lines of their own (S6-34
    /// step 12): a write to one never invalidates the line that every access reads (`regions`,
    /// `mode`).
    counters: ColdCounters,
}

/// The counters of [`PhysicalMemory`] that are not read by an access. `repr(align(128))` puts
/// them on their own cache lines (128 bytes covers the 64-byte lines of x86_64 and the 128-byte
/// lines of Apple aarch64, where the adjacent-line prefetch pairs the 64-byte ones), so the
/// atomic increments of one thread do not bounce the line of `regions` and `mode` that every other
/// thread reads on every guest memory access. The struct is the last field.
#[repr(align(128))]
struct ColdCounters {
    /// S6-32: host fences issued by [`PhysicalMemory::fence`] in the parallel mode (the
    /// interleaving mode issues none). Measurement and tests only: it exists in the test
    /// configuration alone, so a guest `MFENCE` of a parallel run does not increment a counter the
    /// other vCPUs share (S6-34 step 12).
    #[cfg(test)]
    host_fences: AtomicU64,
    /// S6-31: locked operations refused because the operand was not naturally aligned and no
    /// stop-all gate was there (the split-lock path of S6-34 step 8 runs on the vCPU threads of
    /// the parallel runner, which carry the gate). Counted, never executed non-atomically.
    split_lock_refusals: AtomicU64,
}

impl PhysicalMemory {
    pub fn new(map: &MachineMemoryMap, limits: RamLimits) -> Result<Self, MemoryAccessError> {
        let requested_ram = map
            .regions
            .iter()
            .filter(|region| region.kind == MemoryRegionKind::Ram)
            .try_fold(0u64, |total, region| total.checked_add(region.range.len))
            .ok_or(MemoryAccessError::ResourceLimit {
                requested_bytes: u64::MAX,
                max_bytes: limits.max_total_bytes,
            })?;
        if requested_ram > limits.max_total_bytes {
            return Err(MemoryAccessError::ResourceLimit {
                requested_bytes: requested_ram,
                max_bytes: limits.max_total_bytes,
            });
        }

        let mut regions = Vec::new();
        regions.try_reserve_exact(map.regions.len()).map_err(|_| {
            MemoryAccessError::AllocationFailed {
                bytes: map
                    .regions
                    .len()
                    .checked_mul(std::mem::size_of::<Region>())
                    .and_then(|bytes| u64::try_from(bytes).ok())
                    .unwrap_or(u64::MAX),
            }
        })?;
        for &spec in &map.regions {
            // Framebuffer backing is allocated like RAM but is not counted in
            // `requested_ram`; its size is bounded by the map validation.
            // Page generations exist only for RAM: Framebuffer and ROM are never
            // tracked code (FIRMWARE_BOOT.md 3.1: ROM is read-only and the image
            // executes from its RAM shadow). ROM backing starts zeroed and is
            // filled once by the machine loader through `load_rom_bytes`.
            let (backing, page_generations) = if matches!(
                spec.kind,
                MemoryRegionKind::Ram | MemoryRegionKind::Framebuffer | MemoryRegionKind::Rom
            ) {
                let len = usize::try_from(spec.range.len).map_err(|_| {
                    MemoryAccessError::AllocationFailed {
                        bytes: spec.range.len,
                    }
                })?;
                // S6-30a: the store keeps `start % 8` spare bytes in front, so an aligned guest
                // address is an aligned host address (see `ram_storage`).
                let lead = (spec.range.start.get() & 7) as usize;
                let bytes = RamStorage::new_zeroed(len, lead).map_err(|()| {
                    MemoryAccessError::AllocationFailed {
                        bytes: spec.range.len,
                    }
                })?;
                if matches!(
                    spec.kind,
                    MemoryRegionKind::Framebuffer | MemoryRegionKind::Rom
                ) {
                    (Some(bytes), Vec::new())
                } else {
                    let covered = (spec.range.start.get() & 0xfff)
                        .checked_add(spec.range.len)
                        .and_then(|bytes| bytes.checked_add(0xfff))
                        .ok_or(MemoryAccessError::AddressOverflow)?
                        / 0x1000;
                    let page_count = usize::try_from(covered).map_err(|_| {
                        MemoryAccessError::AllocationFailed {
                            bytes: covered.saturating_mul(std::mem::size_of::<u64>() as u64),
                        }
                    })?;
                    let generation_bytes = covered
                        .checked_mul(std::mem::size_of::<u64>() as u64)
                        .ok_or(MemoryAccessError::AddressOverflow)?;
                    let mut generations = Vec::new();
                    generations.try_reserve_exact(page_count).map_err(|_| {
                        MemoryAccessError::AllocationFailed {
                            bytes: generation_bytes,
                        }
                    })?;
                    generations.resize_with(page_count, || AtomicU64::new(1));
                    (Some(bytes), generations)
                }
            } else {
                (None, Vec::new())
            };
            regions.push(Region {
                spec,
                backing,
                page_generations,
            });
        }
        Ok(Self {
            regions,
            mode: ExecutionMode::default(),
            counters: ColdCounters {
                #[cfg(test)]
                host_fences: AtomicU64::new(0),
                split_lock_refusals: AtomicU64::new(0),
            },
        })
    }

    /// S6-32: the execution mode of this memory (see [`ExecutionMode`]).
    pub fn execution_mode(&self) -> ExecutionMode {
        self.mode
    }

    /// S6-32: the one switch of the execution mode on the memory side. The machine sets it while
    /// it still owns the memory exclusively (`&mut self`), before any vCPU adapter shares it; the
    /// interpreter never sees it. Switching changes only the host memory order of ordinary
    /// scalar accesses and the host fence of the guest fences: results of a single-threaded run
    /// are identical in both modes.
    pub fn set_execution_mode(&mut self, mode: ExecutionMode) {
        self.mode = mode;
    }

    /// S6-32: the host side of a guest fence instruction. Parallel: a host fence of the kind
    /// (`MFENCE` is `SeqCst`, `SFENCE` is `Release`, `LFENCE` is `Acquire`; ordinary accesses are
    /// already `SeqCst`, so the fences add nothing the model needs, and are kept because the
    /// contract names them and no barrier is removed because a test passes). Interleaving: no
    /// operation. Changes no guest RAM.
    pub fn fence(&self, kind: od_isa::FenceKind) {
        // `weak-host-ordering` (S6-36, tests and measurements only): the guest fences place no
        // host fence either.
        if self.mode == ExecutionMode::Parallel && !cfg!(feature = "weak-host-ordering") {
            #[cfg(test)]
            self.counters.host_fences.fetch_add(1, Ordering::Relaxed);
            fence(match kind {
                od_isa::FenceKind::Full => Ordering::SeqCst,
                od_isa::FenceKind::Store => Ordering::Release,
                od_isa::FenceKind::Load => Ordering::Acquire,
            });
        }
    }

    /// S6-32: how many host fences [`Self::fence`] issued (always zero in the interleaving mode).
    /// Test builds only (S6-34 step 12): a production build keeps no shared counter on the path of
    /// a guest fence.
    #[cfg(test)]
    pub fn host_fences_issued(&self) -> u64 {
        self.counters.host_fences.load(Ordering::Relaxed)
    }

    /// The generation of the RAM page holding `address` (starts at 1; raised once per committed
    /// write that touches the page). S6-30b: an `Acquire` load. A writer stores the data first
    /// and raises the counter afterwards with `Release`, so a reader that sees a generation also
    /// sees every byte stored before the raise. A reader that caches something derived from
    /// RAM bytes must therefore read the generation **before** it reads the bytes and store that
    /// value with the cached data: a write that lands in between then leaves the cached
    /// generation older than the page, and the next check discards the entry (a spurious
    /// refresh, never a stale hit). Reading the generation after the bytes would allow the
    /// opposite, unsafe pairing (old bytes with a new generation).
    pub fn code_page_generation(
        &self,
        address: GuestPhysicalAddress,
    ) -> Result<u64, MemoryAccessError> {
        let (region, _) = self.locate(address, 1)?;
        if region.spec.kind != MemoryRegionKind::Ram {
            return Err(MemoryAccessError::UnreadableRegion(region.spec.kind));
        }
        let page_index = (address.get() >> 12) - (region.spec.range.start.get() >> 12);
        let page_index =
            usize::try_from(page_index).map_err(|_| MemoryAccessError::AddressOverflow)?;
        region
            .page_generations
            .get(page_index)
            .map(|generation| generation.load(Ordering::Acquire))
            .ok_or(MemoryAccessError::AddressOverflow)
    }

    /// The one commit transaction under every RAM write (`store_*`): locate and validate
    /// (`EmptyRange` before the region search, then the region errors), check that no
    /// page generation would overflow, run `write` on the storage, raise the generation of every
    /// touched page (S6-30b: one `fetch_add(1, Release)` per page, after the data), and only then
    /// call `committed_write` once with the written range. A rejected write changes nothing and
    /// notifies nothing; a `Framebuffer` write has no generations and no callback. The RAM
    /// bytes are reached only through `&self` and atomic accesses (S6-30c: there is no `&mut`
    /// path to them any more).
    ///
    /// The overflow check and the raise are separate atomic steps: concurrent writers to one page
    /// cannot make them inexact in practice (a `u64` counter would need 2^64 writes to that one
    /// page), and the serial paths cannot interleave.
    fn commit_write(
        &self,
        address: GuestPhysicalAddress,
        len: u64,
        write: impl FnOnce(&RamStorage, usize),
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> Result<(), MemoryAccessError> {
        self.commit_write_with(
            address,
            len,
            |storage, offset| {
                write(storage, offset);
                ((), true)
            },
            committed_write,
        )
    }

    /// [`Self::commit_write`] for an operation that reports whether it changed memory (S6-31): `op`
    /// returns its result and `true` when it wrote. Validation, the generation overflow check, the
    /// order (data, then `Release` raise of every touched page, then one callback) and the
    /// Framebuffer rule are those of `commit_write`; when `op` reports no write (a compare that
    /// did not match) the generations are not raised and the callback does not run.
    fn commit_write_with<R>(
        &self,
        address: GuestPhysicalAddress,
        len: u64,
        op: impl FnOnce(&RamStorage, usize) -> (R, bool),
        mut committed_write: impl FnMut(GuestPhysicalRange),
    ) -> Result<R, MemoryAccessError> {
        let (region_index, offset) = self.locate_index(address, len)?;
        let region = &self.regions[region_index];
        // ROM has read backing but the guest and ordinary loaders never write it.
        if region.backing.is_none() || region.spec.kind == MemoryRegionKind::Rom {
            return Err(MemoryAccessError::ReadOnlyRegion(region.spec.kind));
        }
        if region.spec.kind == MemoryRegionKind::Framebuffer {
            // Device-owned pixel memory: no page generations, no callback.
            let backing = region
                .backing
                .as_ref()
                .ok_or(MemoryAccessError::ReadOnlyRegion(region.spec.kind))?;
            return Ok(op(backing, offset).0);
        }
        let first_page = address.get() >> 12;
        let last_page = ((u128::from(address.get()) + u128::from(len) - 1) >> 12) as u64;
        let region_first_page = region.spec.range.start.get() >> 12;
        let first_generation = usize::try_from(first_page - region_first_page)
            .map_err(|_| MemoryAccessError::AddressOverflow)?;
        let generation_count = usize::try_from(last_page - first_page + 1)
            .map_err(|_| MemoryAccessError::AddressOverflow)?;
        let end_generation = first_generation
            .checked_add(generation_count)
            .ok_or(MemoryAccessError::AddressOverflow)?;
        let generations = region
            .page_generations
            .get(first_generation..end_generation)
            .ok_or(MemoryAccessError::AddressOverflow)?;
        if generations
            .iter()
            .any(|generation| generation.load(Ordering::Relaxed) == u64::MAX)
        {
            return Err(MemoryAccessError::CodeGenerationExhausted);
        }
        let backing = region
            .backing
            .as_ref()
            .ok_or(MemoryAccessError::ReadOnlyRegion(region.spec.kind))?;
        let (result, wrote) = op(backing, offset);
        if !wrote {
            return Ok(result);
        }
        // Publish: a reader that `Acquire`-loads the raised value sees the data written above.
        // `weak-host-ordering` (S6-36, tests and measurements only) removes this barrier too: a
        // `Release` read-modify-write after every store orders the stores of one vCPU in practice.
        #[cfg(feature = "weak-host-ordering")]
        let publish = Ordering::Relaxed;
        #[cfg(not(feature = "weak-host-ordering"))]
        let publish = Ordering::Release;
        for generation in generations {
            let previous = generation.fetch_add(1, publish);
            debug_assert_ne!(previous, u64::MAX, "page generation wrapped to 0");
        }
        let committed = GuestPhysicalRange {
            start: address,
            len,
        };
        committed_write(committed);
        Ok(result)
    }

    /// Machine-side one-time fill of a ROM region (the firmware image at power
    /// on). This is the only ROM writer; `store_bytes` keeps rejecting ROM. It is a loader step that
    /// runs before the memory is shared, so it keeps `&mut self`.
    pub(crate) fn load_rom_bytes(
        &mut self,
        address: GuestPhysicalAddress,
        input: &[u8],
    ) -> Result<(), MemoryAccessError> {
        let len = checked_access_len(input.len())?;
        let (region_index, offset) = self.locate_index(address, len)?;
        let region = &mut self.regions[region_index];
        if region.spec.kind != MemoryRegionKind::Rom {
            return Err(MemoryAccessError::ReadOnlyRegion(region.spec.kind));
        }
        let backing = region
            .backing
            .as_ref()
            .ok_or(MemoryAccessError::ReadOnlyRegion(region.spec.kind))?;
        backing.copy_in(offset, input, Ordering::Relaxed);
        Ok(())
    }

    /// Copies bytes out of the `Framebuffer` region containing `address` for
    /// host-side frame readers. Any other region kind is rejected.
    pub(crate) fn read_framebuffer_bytes(
        &self,
        address: GuestPhysicalAddress,
        output: &mut [u8],
    ) -> Result<(), MemoryAccessError> {
        let len = checked_access_len(output.len())?;
        let (region, _) = self.locate(address, len)?;
        if region.spec.kind != MemoryRegionKind::Framebuffer {
            return Err(MemoryAccessError::UnreadableRegion(region.spec.kind));
        }
        self.load_bytes(address, output)
    }

    /// S6-25, S6-31: compare-and-replace of one 8-byte value, the single backend operation under
    /// every page-table accessed/dirty update (the walk of `page_walk`, `compare_replace_table_entry`).
    /// S6-31: it is one host compare-exchange (`AtomicU64::compare_exchange`, `SeqCst`) on the
    /// cell, with no preceding read: when the 8 bytes equal `expected` the cell now holds
    /// `replacement`, the generation of the page is raised and the callback runs once, and the
    /// result is `Ok(Ok(()))`; otherwise nothing is written, no callback runs and the result is
    /// `Ok(Err(observed))` (the value the compare saw, so the caller can recompute or retry).
    /// Region errors are those of a write (`Unmapped`, `AccessCrossesRegion`, `ReadOnlyRegion`
    /// for ROM and holes, `CodeGenerationExhausted`) and are reported before the compare; memory
    /// is unchanged on every error. An address that is not 8-aligned is refused with
    /// `MisalignedAtomic` (counted); table entries are 8-aligned by construction (4 KiB base plus
    /// index * 8).
    pub fn compare_replace_u64(
        &self,
        address: GuestPhysicalAddress,
        expected: u64,
        replacement: u64,
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> Result<Result<(), u64>, MemoryAccessError> {
        self.locked_operation(
            address,
            AccessWidth::Qword,
            |storage, offset| match storage.atomic_compare_exchange(
                offset,
                8,
                expected,
                replacement,
            ) {
                Ok(_) => (Ok(()), true),
                Err(observed) => (Err(observed), false),
            },
            committed_write,
        )
    }

    /// S6-41c: the window for one inline atomic operation of generated code on the naturally
    /// aligned RAM operand of `width` (1, 2, 4 or 8) bytes at `address`: the host pointer of the
    /// operand (`RamStorage::operand_ptr`, amendment (в′) of S6-30a) and the page-generation
    /// counter of the one page it lies in (an aligned operand of at most 8 bytes never crosses a
    /// page). The generated code does the host-atomic data access itself, then raises the counter
    /// with a `Release` read-modify-write, then reports the write (the order of
    /// [`Self::commit_write`], S6-40 obligation 1); this function writes nothing and raises
    /// nothing.
    ///
    /// Errors, all before any effect and none counted: the region errors of a write (`Unmapped`,
    /// `AccessCrossesRegion`, `AddressOverflow`), `ReadOnlyRegion` for ROM, holes and the
    /// framebuffer (device-owned memory has no generations, the interpreter owns it),
    /// `MisalignedAtomic` for a misaligned operand (the caller sends it to the split-lock path of
    /// S6-34, it never runs inline), `CodeGenerationExhausted`.
    ///
    /// The window must be used for one synchronous call only: the pointer is valid while `self`
    /// is alive and the window borrows `self` for exactly that reason.
    pub fn atomic_operand_window(
        &self,
        address: GuestPhysicalAddress,
        width: usize,
    ) -> Result<AtomicOperandWindow<'_>, MemoryAccessError> {
        if !matches!(width, 1 | 2 | 4 | 8) {
            return Err(MemoryAccessError::InvalidPhysicalWidth(
                u8::try_from(width).unwrap_or(u8::MAX),
            ));
        }
        let bytes = width as u64;
        let (region_index, offset) = self.locate_index(address, bytes)?;
        let region = &self.regions[region_index];
        if region.spec.kind != MemoryRegionKind::Ram {
            return Err(MemoryAccessError::ReadOnlyRegion(region.spec.kind));
        }
        if address.get() % bytes != 0 {
            return Err(MemoryAccessError::MisalignedAtomic {
                address,
                width: width as u8,
            });
        }
        let backing = region
            .backing
            .as_ref()
            .ok_or(MemoryAccessError::ReadOnlyRegion(region.spec.kind))?;
        let page_index =
            usize::try_from((address.get() >> 12) - (region.spec.range.start.get() >> 12))
                .map_err(|_| MemoryAccessError::AddressOverflow)?;
        let generation = region
            .page_generations
            .get(page_index)
            .ok_or(MemoryAccessError::AddressOverflow)?;
        if generation.load(Ordering::Relaxed) == u64::MAX {
            return Err(MemoryAccessError::CodeGenerationExhausted);
        }
        let host =
            backing
                .operand_ptr(offset, width)
                .ok_or(MemoryAccessError::MisalignedAtomic {
                    address,
                    width: width as u8,
                })?;
        Ok(AtomicOperandWindow {
            host,
            generation,
            range: GuestPhysicalRange {
                start: address,
                len: bytes,
            },
        })
    }

    /// S6-31: how many locked operations were refused because the operand was not naturally
    /// aligned to its width (the split-lock case: a pair of 16-byte operations for an operand
    /// inside one aligned 16-byte block is not implemented, and stopping all vCPUs is S6-34).
    /// The count only grows.
    pub fn split_lock_refusals(&self) -> u64 {
        self.counters.split_lock_refusals.load(Ordering::Relaxed)
    }

    /// S6-31: counts one refused misaligned locked operation. An adapter that refuses the operand
    /// before it translates the address (a linear address is not a physical one) calls this
    /// directly; the physical operations call [`Self::refuse_misaligned_lock`].
    pub(crate) fn count_split_lock_refusal(&self) {
        self.counters
            .split_lock_refusals
            .fetch_add(1, Ordering::Relaxed);
    }

    /// S6-31: counts one refused misaligned locked operation and returns the matching error.
    fn refuse_misaligned_lock(
        &self,
        address: GuestPhysicalAddress,
        width: AccessWidth,
    ) -> MemoryAccessError {
        self.count_split_lock_refusal();
        MemoryAccessError::MisalignedAtomic {
            address,
            width: width.bytes(),
        }
    }

    /// The common shape of every locked operation (S6-31): refuse a misaligned operand (counted),
    /// then run `op` on the aligned host cell inside the commit transaction. `op` returns its
    /// result and whether it wrote.
    fn locked_operation<R>(
        &self,
        address: GuestPhysicalAddress,
        width: AccessWidth,
        op: impl FnOnce(&RamStorage, usize) -> (R, bool),
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> Result<R, MemoryAccessError> {
        let bytes = u64::from(width.bytes());
        if address.get() % bytes != 0 {
            return Err(self.refuse_misaligned_lock(address, width));
        }
        self.commit_write_with(address, bytes, op, committed_write)
    }

    /// S6-31: atomic compare-exchange of `width` bytes at a naturally aligned `address`: one host
    /// `compare_exchange` (`SeqCst`; `CASAL*` on aarch64 with LSE). Returns the old value,
    /// zero-extended. `expected` and `replacement` are truncated to the width. Like the x86
    /// `CMPXCHG`, the operation counts as a write even when the compare fails (the generation of
    /// the page is raised and the callback runs once): the serial path of S6-20 wrote the old
    /// value back, and the observers (code caches) see the same notifications. Errors as in
    /// [`Self::compare_replace_u64`]; memory is unchanged on every error.
    pub fn atomic_compare_exchange(
        &self,
        address: GuestPhysicalAddress,
        width: AccessWidth,
        expected: u64,
        replacement: u64,
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> Result<u64, MemoryAccessError> {
        let bytes = usize::from(width.bytes());
        self.locked_operation(
            address,
            width,
            |storage, offset| {
                #[cfg(feature = "nonatomic-lock-test")]
                {
                    let old = nonatomic::load(storage, offset, bytes);
                    if old == expected & nonatomic::mask(bytes) {
                        nonatomic::store(storage, offset, bytes, replacement);
                    }
                    return (old, true);
                }
                #[cfg(not(feature = "nonatomic-lock-test"))]
                {
                    let old =
                        match storage.atomic_compare_exchange(offset, bytes, expected, replacement)
                        {
                            Ok(old) | Err(old) => old,
                        };
                    (old, true)
                }
            },
            committed_write,
        )
    }

    /// S6-31: atomic exchange (`SWPAL*`): stores `value` (truncated to the width) and returns the
    /// old value. Alignment, notification and errors as in [`Self::atomic_compare_exchange`].
    pub fn atomic_swap(
        &self,
        address: GuestPhysicalAddress,
        width: AccessWidth,
        value: u64,
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> Result<u64, MemoryAccessError> {
        let bytes = usize::from(width.bytes());
        self.locked_operation(
            address,
            width,
            |storage, offset| {
                #[cfg(feature = "nonatomic-lock-test")]
                {
                    let old = nonatomic::load(storage, offset, bytes);
                    nonatomic::store(storage, offset, bytes, value);
                    return (old, true);
                }
                #[cfg(not(feature = "nonatomic-lock-test"))]
                (storage.atomic_swap(offset, bytes, value), true)
            },
            committed_write,
        )
    }

    /// S6-31: atomic wrapping addition (`LDADDAL*`): adds `addend` (truncated to the width) and
    /// returns the old value. Alignment, notification and errors as in
    /// [`Self::atomic_compare_exchange`].
    pub fn atomic_fetch_add(
        &self,
        address: GuestPhysicalAddress,
        width: AccessWidth,
        addend: u64,
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> Result<u64, MemoryAccessError> {
        let bytes = usize::from(width.bytes());
        self.locked_operation(
            address,
            width,
            |storage, offset| {
                #[cfg(feature = "nonatomic-lock-test")]
                {
                    let old = nonatomic::load(storage, offset, bytes);
                    nonatomic::store(storage, offset, bytes, old.wrapping_add(addend));
                    return (old, true);
                }
                #[cfg(not(feature = "nonatomic-lock-test"))]
                (storage.atomic_fetch_add(offset, bytes, addend), true)
            },
            committed_write,
        )
    }

    /// S6-31: the general locked read-modify-write behind `GuestMemory::atomic_rmw_integer` (the
    /// forms `AND`, `OR`, `BTS`, `BTR`, `XADD`, `CMPXCHG`... pass a pure function of the old
    /// value): a loop of host compare-exchanges (`SeqCst`) that stores `update(old)` (truncated
    /// to the width) only over the value `update` was computed from, and returns that old value.
    /// `update` may run more than once (once per lost compare); only the value of the successful
    /// attempt is stored. The result is always stored, even when equal to the old value (the
    /// write of a failed `CMPXCHG`); the compare-exchange of such a store still compares, so a
    /// concurrent change is never overwritten. Alignment, notification and errors as in
    /// [`Self::atomic_compare_exchange`].
    pub fn atomic_rmw(
        &self,
        address: GuestPhysicalAddress,
        width: AccessWidth,
        update: &mut dyn FnMut(u64) -> u64,
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> Result<u64, MemoryAccessError> {
        let bytes = usize::from(width.bytes());
        let mask = if bytes == 8 {
            u64::MAX
        } else {
            (1u64 << (8 * bytes)) - 1
        };
        self.locked_operation(
            address,
            width,
            |storage, offset| {
                // `nonatomic-lock-test` (S6-36, tests and measurements only): the locked
                // read-modify-write is a plain load, the update, and a plain store.
                #[cfg(feature = "nonatomic-lock-test")]
                {
                    let old = nonatomic::load(storage, offset, bytes);
                    let new = update(old) & mask;
                    nonatomic::store(storage, offset, bytes, new);
                    return (old, true);
                }
                #[cfg(not(feature = "nonatomic-lock-test"))]
                let mut old = match bytes {
                    1 => u64::from(storage.load_u8(offset, Ordering::SeqCst)),
                    2 => u64::from(storage.load_u16(offset, Ordering::SeqCst)),
                    4 => u64::from(storage.load_u32(offset, Ordering::SeqCst)),
                    _ => storage.load_u64(offset, Ordering::SeqCst),
                };
                #[cfg(not(feature = "nonatomic-lock-test"))]
                loop {
                    let new = update(old) & mask;
                    match storage.atomic_compare_exchange(offset, bytes, old, new) {
                        Ok(_) => return (old, true),
                        Err(observed) => old = observed,
                    }
                }
            },
            committed_write,
        )
    }

    /// S6-30a: the region storage a read of `len` bytes at `address` goes to, with the offset in
    /// it. Checks: `EmptyRange`/`AddressOverflow` of the length, then the region search, then
    /// the readable kind.
    fn readable_storage(
        &self,
        address: GuestPhysicalAddress,
        len: usize,
    ) -> Result<(&RamStorage, usize), MemoryAccessError> {
        let len = checked_access_len(len)?;
        let (region, offset) = self.locate(address, len)?;
        let backing = region
            .backing
            .as_ref()
            .ok_or(MemoryAccessError::UnreadableRegion(region.spec.kind))?;
        Ok((backing, offset))
    }

    fn locate(
        &self,
        address: GuestPhysicalAddress,
        len: u64,
    ) -> Result<(&Region, usize), MemoryAccessError> {
        let (index, offset) = self.locate_index(address, len)?;
        Ok((&self.regions[index], offset))
    }

    /// S4-13: a handle to the plain-RAM 4 KiB frame at `base` (4 KiB aligned), or `None` unless
    /// the whole frame lies inside one `Ram` region with backing. The region list is fixed after
    /// construction, so a handle stays valid for the life of this memory; the fetch translation
    /// cache keeps one per entry and reads instruction bytes through it without a region search.
    pub(crate) fn ram_frame(&self, base: GuestPhysicalAddress) -> Option<RamFrame> {
        if base.0 & 0xfff != 0 {
            return None;
        }
        let (index, offset) = self.locate_index(base, 0x1000).ok()?;
        let region = &self.regions[index];
        (region.spec.kind == MemoryRegionKind::Ram && region.backing.is_some()).then_some(
            RamFrame {
                region: index,
                offset,
                large: false,
            },
        )
    }

    /// S4-13d: a handle to the plain-RAM 2 MiB span at `base` (2 MiB aligned), or `None` unless
    /// the whole span lies inside one `Ram` region with backing. The fetch translation cache with
    /// the 2 MiB leaf tag keeps one per entry; the byte offset inside the span comes from the
    /// address of each read, never from the handle.
    pub(crate) fn ram_leaf_2m(&self, base: GuestPhysicalAddress) -> Option<RamFrame> {
        if base.0 & 0x1f_ffff != 0 {
            return None;
        }
        let (index, offset) = self.locate_index(base, 0x20_0000).ok()?;
        let region = &self.regions[index];
        (region.spec.kind == MemoryRegionKind::Ram && region.backing.is_some()).then_some(
            RamFrame {
                region: index,
                offset,
                large: true,
            },
        )
    }

    /// One byte of a frame from [`Self::ram_frame`] (or of a span from [`Self::ram_leaf_2m`]).
    /// `in_frame` is any address: only its low 12 bits (low 21 bits for a 2 MiB span) are used.
    /// `None` is not expected for a handle of this memory; the caller then takes the ordinary read.
    pub(crate) fn read_ram_frame_u8(&self, frame: RamFrame, in_frame: u64) -> Option<u8> {
        let region = self.regions.get(frame.region)?;
        let mask = if frame.large { 0x1f_ffff } else { 0xfff };
        let at = frame
            .offset
            .checked_add(usize::try_from(in_frame & mask).ok()?)?;
        let storage = region.backing.as_ref()?;
        by_mode!(self.mode, order => storage.try_load_u8(at, order))
    }

    /// PERF-11: the qword at `in_frame` of a 4 KiB frame from [`Self::ram_frame`] (only the low 12
    /// bits of `in_frame` are used; the qword must not cross the frame), read with the host accesses
    /// of [`Self::load_bytes`] of 8 bytes, the read of the tracked walk. `None` for a 2 MiB handle,
    /// a qword past the frame end or a handle that is not of this memory (the caller then walks).
    #[inline(always)]
    pub(crate) fn read_ram_frame_qword(&self, frame: RamFrame, in_frame: u64) -> Option<u64> {
        let in_page = (in_frame & 0xfff) as usize;
        if frame.large || in_page > 0x1000 - 8 {
            return None;
        }
        let storage = self.regions.get(frame.region)?.backing.as_ref()?;
        let at = frame.offset + in_page;
        Some(by_mode!(self.mode, order => storage.load_qword_as_copy(at, order)))
    }

    /// PERF-10: where byte `in_frame` of a frame from [`Self::ram_frame`] (or of a span from
    /// [`Self::ram_leaf_2m`]) lives: the region index, the byte offset inside the region, and the
    /// current generation of its 4 KiB page (an `Acquire` load, so the caller reads it BEFORE the
    /// bytes it caches, the reader's rule of [`Self::code_page_generation`]). `in_frame` is any
    /// address: only its low 12 bits (low 21 bits for a 2 MiB span) are used, like
    /// [`Self::read_ram_frame_u8`]. `None` for a handle that is not of this memory.
    pub(crate) fn ram_frame_code_page(
        &self,
        frame: RamFrame,
        in_frame: u64,
    ) -> Option<RamCodePage> {
        let region = self.regions.get(frame.region)?;
        let mask = if frame.large { 0x1f_ffff } else { 0xfff };
        let offset = frame
            .offset
            .checked_add(usize::try_from(in_frame & mask).ok()?)?;
        let start = region.spec.range.start.get();
        let page_index = usize::try_from(
            (start.checked_add(u64::try_from(offset).ok()?)? >> 12) - (start >> 12),
        )
        .ok()?;
        let generation = region
            .page_generations
            .get(page_index)?
            .load(Ordering::Acquire);
        Some(RamCodePage {
            region: frame.region,
            offset,
            generation,
        })
    }

    /// PERF-31: the index of the 4 KiB page of `page` among the page generations of its region
    /// (for [`Self::ram_code_page_generation_at`]); `None` for a page that is not of this memory.
    pub(crate) fn ram_code_page_index(&self, page: RamCodePage) -> Option<usize> {
        let region = self.regions.get(page.region)?;
        let start = region.spec.range.start.get();
        let index = usize::try_from(
            (start.checked_add(u64::try_from(page.offset).ok()?)? >> 12) - (start >> 12),
        )
        .ok()?;
        (index < region.page_generations.len()).then_some(index)
    }

    /// PERF-31: the current generation of page `index` of region `region` (an `Acquire` load, as
    /// [`Self::ram_frame_code_page`] reads it).
    #[inline(always)]
    pub(crate) fn ram_code_page_generation_at(&self, region: usize, index: usize) -> Option<u64> {
        Some(
            self.regions
                .get(region)?
                .page_generations
                .get(index)?
                .load(Ordering::Acquire),
        )
    }

    pub(crate) fn locate_region_kind(
        &self,
        address: GuestPhysicalAddress,
        len: u64,
    ) -> Result<MemoryRegionKind, MemoryAccessError> {
        let (region, _) = self.locate(address, len)?;
        Ok(region.spec.kind)
    }

    pub(crate) fn region_specs(&self) -> impl Iterator<Item = MemoryRegionSpec> + '_ {
        self.regions.iter().map(|region| region.spec)
    }

    fn locate_index(
        &self,
        address: GuestPhysicalAddress,
        len: u64,
    ) -> Result<(usize, usize), MemoryAccessError> {
        let end = u128::from(address.0) + u128::from(len);
        if end > (1u128 << 64) {
            return Err(MemoryAccessError::AddressOverflow);
        }
        let index = self
            .regions
            .binary_search_by_key(&address.0, |region| region.spec.range.start.0)
            .unwrap_or_else(|insertion| insertion.saturating_sub(1));
        let Some(region) = self.regions.get(index) else {
            return Err(MemoryAccessError::Unmapped(address));
        };
        if !region.spec.range.contains(address, len) {
            if address.0 < region.spec.range.start.0
                || u128::from(address.0) >= region.spec.range.end_exclusive()
            {
                return Err(MemoryAccessError::Unmapped(address));
            }
            return Err(MemoryAccessError::AccessCrossesRegion(GuestPhysicalRange {
                start: address,
                len: u64::try_from(end - u128::from(address.0))
                    .map_err(|_| MemoryAccessError::AddressOverflow)?,
            }));
        }
        let offset = usize::try_from(address.0 - region.spec.range.start.0)
            .map_err(|_| MemoryAccessError::AddressOverflow)?;
        Ok((index, offset))
    }
}

/// The access functions (S6-30a; the only RAM access path since S6-30c). All take `&self`, so a
/// vCPU adapter holds a shared reference to the memory and any number of adapters share it.
///
/// Loads and stores of 1, 2, 4 and 8 bytes at a naturally aligned address are one host atomic
/// access of that width (S6-32: `SeqCst` in the parallel mode, the choice of S6-06, and `Relaxed`
/// in the interleaving mode, see [`ExecutionMode`]; the default memory is in the latter);
/// a misaligned access is split into ascending one-byte accesses (MEMORY_MODEL.md, "Атомарность
/// и разделённые доступы"). `load_bytes`/`store_bytes` are bulk copies: the copy as a whole is
/// not atomic, and another thread can see any prefix of one. The order of each host access of a
/// bulk copy is the order of the ordinary accesses of the mode (S6-32: `SeqCst` in the parallel
/// mode, `Relaxed` in the interleaving mode); an aligned copy of 1, 2, 4 or 8 bytes is ONE host
/// access of that width (`chunk_width`, normative); otherwise ascending accesses of up to 8
/// bytes (see `ram_storage`).
///
/// S6-30b: the stores take a `committed_write` callback and go through one commit transaction
/// (`commit_write`): validation and errors, the data is stored first, then every touched page
/// generation is raised atomically (`Release`), then the callback runs once; ROM and a rejected
/// write change and notify nothing, a `Framebuffer` write has no generation and no callback.
impl PhysicalMemory {
    pub fn load_bytes(
        &self,
        address: GuestPhysicalAddress,
        output: &mut [u8],
    ) -> Result<(), MemoryAccessError> {
        let (storage, offset) = self.readable_storage(address, output.len())?;
        by_mode!(self.mode, order => storage.copy_out(offset, output, order));
        Ok(())
    }

    pub fn store_bytes(
        &self,
        address: GuestPhysicalAddress,
        input: &[u8],
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> Result<(), MemoryAccessError> {
        let len = checked_access_len(input.len())?;
        by_mode!(self.mode, order => self.commit_write(
            address,
            len,
            |storage, offset| storage.copy_in(offset, input, order),
            committed_write,
        ))
    }

    pub fn load_u8(&self, address: GuestPhysicalAddress) -> Result<u8, MemoryAccessError> {
        let (storage, offset) = self.readable_storage(address, 1)?;
        Ok(by_mode!(self.mode, order => storage.load_u8(offset, order)))
    }

    pub fn load_u16(&self, address: GuestPhysicalAddress) -> Result<u16, MemoryAccessError> {
        let (storage, offset) = self.readable_storage(address, 2)?;
        Ok(by_mode!(self.mode, order => storage.load_u16(offset, order)))
    }

    pub fn load_u32(&self, address: GuestPhysicalAddress) -> Result<u32, MemoryAccessError> {
        let (storage, offset) = self.readable_storage(address, 4)?;
        Ok(by_mode!(self.mode, order => storage.load_u32(offset, order)))
    }

    pub fn load_u64(&self, address: GuestPhysicalAddress) -> Result<u64, MemoryAccessError> {
        let (storage, offset) = self.readable_storage(address, 8)?;
        Ok(by_mode!(self.mode, order => storage.load_u64(offset, order)))
    }

    pub fn store_u8(
        &self,
        address: GuestPhysicalAddress,
        value: u8,
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> Result<(), MemoryAccessError> {
        by_mode!(self.mode, order => self.commit_write(
            address,
            1,
            |storage, offset| storage.store_u8(offset, value, order),
            committed_write,
        ))
    }

    pub fn store_u16(
        &self,
        address: GuestPhysicalAddress,
        value: u16,
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> Result<(), MemoryAccessError> {
        by_mode!(self.mode, order => self.commit_write(
            address,
            2,
            |storage, offset| storage.store_u16(offset, value, order),
            committed_write,
        ))
    }

    pub fn store_u32(
        &self,
        address: GuestPhysicalAddress,
        value: u32,
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> Result<(), MemoryAccessError> {
        by_mode!(self.mode, order => self.commit_write(
            address,
            4,
            |storage, offset| storage.store_u32(offset, value, order),
            committed_write,
        ))
    }

    pub fn store_u64(
        &self,
        address: GuestPhysicalAddress,
        value: u64,
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> Result<(), MemoryAccessError> {
        by_mode!(self.mode, order => self.commit_write(
            address,
            8,
            |storage, offset| storage.store_u64(offset, value, order),
            committed_write,
        ))
    }
}

/// `nonatomic-lock-test` (S6-36): the locked operations as a plain load and a plain store of the
/// width, so that two vCPUs can both read the old value and one update is lost. Tests and
/// measurements only: the feature is never in `default`.
#[cfg(feature = "nonatomic-lock-test")]
mod nonatomic {
    use crate::ram_storage::RamStorage;
    use std::sync::atomic::Ordering;

    pub(super) fn mask(bytes: usize) -> u64 {
        if bytes == 8 {
            u64::MAX
        } else {
            (1u64 << (8 * bytes)) - 1
        }
    }

    pub(super) fn load(storage: &RamStorage, offset: usize, bytes: usize) -> u64 {
        match bytes {
            1 => u64::from(storage.load_u8(offset, Ordering::SeqCst)),
            2 => u64::from(storage.load_u16(offset, Ordering::SeqCst)),
            4 => u64::from(storage.load_u32(offset, Ordering::SeqCst)),
            _ => storage.load_u64(offset, Ordering::SeqCst),
        }
    }

    pub(super) fn store(storage: &RamStorage, offset: usize, bytes: usize, value: u64) {
        match bytes {
            1 => storage.store_u8(offset, value as u8, Ordering::SeqCst),
            2 => storage.store_u16(offset, value as u16, Ordering::SeqCst),
            4 => storage.store_u32(offset, value as u32, Ordering::SeqCst),
            _ => storage.store_u64(offset, value, Ordering::SeqCst),
        }
    }
}

fn checked_access_len(len: usize) -> Result<u64, MemoryAccessError> {
    if len == 0 {
        return Err(MemoryAccessError::EmptyRange);
    }
    u64::try_from(len).map_err(|_| MemoryAccessError::AddressOverflow)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(start: u64, len: u64) -> GuestPhysicalRange {
        GuestPhysicalRange::new(GuestPhysicalAddress::new(start), len).unwrap()
    }

    fn memory(specs: &[(u64, u64, MemoryRegionKind)]) -> PhysicalMemory {
        let regions = specs
            .iter()
            .map(|&(start, len, kind)| MemoryRegionSpec::new(range(start, len), kind))
            .collect();
        PhysicalMemory::new(
            &MachineMemoryMap::new(36, regions).unwrap(),
            RamLimits::new(1 << 20),
        )
        .unwrap()
    }

    /// S6-34 step 12: the counters that vCPU threads write sit on cache lines of their own, away
    /// from `regions` and `mode`, which every access reads.
    #[test]
    fn the_write_counters_do_not_share_a_cache_line_with_the_fields_every_access_reads() {
        use std::mem::{align_of, offset_of, size_of};
        assert_eq!(align_of::<ColdCounters>(), 128);
        // The compiler may place the aligned field anywhere: the cache lines (128 bytes, the
        // struct itself is 128-aligned) of the counters and of each field an access reads must be
        // disjoint.
        let counters = offset_of!(PhysicalMemory, counters);
        let counter_lines = counters / 128..=(counters + size_of::<ColdCounters>() - 1) / 128;
        for (name, offset, size) in [
            (
                "regions",
                offset_of!(PhysicalMemory, regions),
                size_of::<Vec<Region>>(),
            ),
            (
                "mode",
                offset_of!(PhysicalMemory, mode),
                size_of::<ExecutionMode>(),
            ),
        ] {
            let lines = offset / 128..=(offset + size - 1) / 128;
            assert!(
                lines.end() < counter_lines.start() || lines.start() > counter_lines.end(),
                "{name} (bytes {offset}..{}) shares a 128-byte line with the counters ({counters}..)",
                offset + size
            );
        }
    }

    #[test]
    fn map_checks_overlap_width_and_range_overflow() {
        let overlap = vec![
            MemoryRegionSpec::new(range(0x1000, 0x2000), MemoryRegionKind::Ram),
            MemoryRegionSpec::new(range(0x2fff, 0x1000), MemoryRegionKind::Rom),
        ];
        assert!(matches!(
            MachineMemoryMap::new(36, overlap),
            Err(MemoryAccessError::OverlappingRegions { .. })
        ));
        assert_eq!(range(u64::MAX, 1).end_exclusive(), 1u128 << 64);
        assert_eq!(
            GuestPhysicalRange::new(GuestPhysicalAddress::new(u64::MAX), 2),
            Err(MemoryAccessError::AddressOverflow)
        );
        assert!(matches!(
            MachineMemoryMap::new(
                12,
                vec![MemoryRegionSpec::new(
                    range(0xff0, 0x20),
                    MemoryRegionKind::Ram
                )]
            ),
            Err(MemoryAccessError::PhysicalWidth(_))
        ));
        assert!(MachineMemoryMap::new(64, vec![]).is_ok());
        let top_byte_map = MachineMemoryMap::new(
            64,
            vec![MemoryRegionSpec::new(
                range(u64::MAX, 1),
                MemoryRegionKind::Ram,
            )],
        )
        .unwrap();
        let top_byte = PhysicalMemory::new(&top_byte_map, RamLimits::new(1)).unwrap();
        top_byte
            .store_u8(GuestPhysicalAddress::new(u64::MAX), 0x5a, |_| {})
            .unwrap();
        assert_eq!(
            top_byte.load_u8(GuestPhysicalAddress::new(u64::MAX)),
            Ok(0x5a)
        );
    }

    /// S6-25: compare-and-replace writes (and notifies, and bumps the generation) only when the
    /// 8 bytes equal `expected`; otherwise it returns the observed value and changes nothing.
    #[test]
    fn compare_replace_u64_replaces_only_a_matching_value() {
        let ram = memory(&[
            (0, 0x2000, MemoryRegionKind::Ram),
            (0x2000, 0x1000, MemoryRegionKind::Rom),
        ]);
        let at = GuestPhysicalAddress::new(0x100);
        ram.store_u64(at, 7, |_| {}).unwrap();
        let generation = ram.code_page_generation(at).unwrap();
        let mut log = Vec::new();
        assert_eq!(
            ram.compare_replace_u64(at, 8, 9, |r| log.push(r)),
            Ok(Err(7))
        );
        assert!(log.is_empty());
        assert_eq!(ram.load_u64(at), Ok(7));
        assert_eq!(ram.code_page_generation(at), Ok(generation));
        assert_eq!(
            ram.compare_replace_u64(at, 7, 9, |r| log.push(r)),
            Ok(Ok(()))
        );
        assert_eq!(log, [range(0x100, 8)]);
        assert_eq!(ram.load_u64(at), Ok(9));
        assert_eq!(ram.code_page_generation(at), Ok(generation + 1));
        // Errors leave memory unchanged and notify nothing.
        let rom = GuestPhysicalAddress::new(0x2000);
        assert_eq!(
            ram.compare_replace_u64(rom, 0, 1, |r| log.push(r)),
            Err(MemoryAccessError::ReadOnlyRegion(MemoryRegionKind::Rom))
        );
        assert_eq!(
            ram.compare_replace_u64(GuestPhysicalAddress::new(0x9000), 0, 1, |r| log.push(r)),
            Err(MemoryAccessError::Unmapped(GuestPhysicalAddress::new(
                0x9000
            )))
        );
        assert_eq!(log.len(), 1);
    }

    #[test]
    fn aggregate_ram_budget_rejects_before_backing_allocation() {
        let map = MachineMemoryMap::new(
            36,
            vec![
                MemoryRegionSpec::new(range(0, 8), MemoryRegionKind::Ram),
                MemoryRegionSpec::new(range(0x1000, 8), MemoryRegionKind::Ram),
            ],
        )
        .unwrap();
        assert_eq!(
            PhysicalMemory::new(&map, RamLimits::new(15)).err(),
            Some(MemoryAccessError::ResourceLimit {
                requested_bytes: 16,
                max_bytes: 15,
            })
        );
    }

    #[test]
    fn accesses_check_boundaries_holes_rom_and_reserved_regions() {
        let memory = memory(&[
            (0x1000, 0x1000, MemoryRegionKind::Ram),
            (0x3000, 0x1000, MemoryRegionKind::Rom),
            (0x5000, 0x1000, MemoryRegionKind::Reserved),
        ]);
        memory
            .store_u32(GuestPhysicalAddress::new(0x1ffc), 0x1234_5678, |_| {})
            .unwrap();
        assert_eq!(
            memory.load_u32(GuestPhysicalAddress::new(0x1ffc)).unwrap(),
            0x1234_5678
        );
        assert!(matches!(
            memory.load_u8(GuestPhysicalAddress::new(0x2000)),
            Err(MemoryAccessError::Unmapped(_))
        ));
        // S7-03: ROM has read backing (zeroed until the loader fills it).
        assert_eq!(
            memory.load_u8(GuestPhysicalAddress::new(0x3000)).unwrap(),
            0
        );
        assert!(matches!(
            memory.store_u8(GuestPhysicalAddress::new(0x3000), 1, |_| {}),
            Err(MemoryAccessError::ReadOnlyRegion(MemoryRegionKind::Rom))
        ));
        assert!(matches!(
            memory.store_u8(GuestPhysicalAddress::new(0x5000), 1, |_| {}),
            Err(MemoryAccessError::ReadOnlyRegion(
                MemoryRegionKind::Reserved
            ))
        ));
        assert!(matches!(
            memory.load_u16(GuestPhysicalAddress::new(u64::MAX)),
            Err(MemoryAccessError::AddressOverflow)
        ));
    }

    #[test]
    fn rom_is_readable_after_loader_fill_and_rejects_every_other_write() {
        let mut memory = memory(&[
            (0x1000, 0x1000, MemoryRegionKind::Ram),
            (0x3000, 0x1000, MemoryRegionKind::Rom),
        ]);
        memory
            .load_rom_bytes(GuestPhysicalAddress::new(0x3010), &[1, 2, 3, 4])
            .unwrap();
        assert_eq!(
            memory.load_u32(GuestPhysicalAddress::new(0x3010)).unwrap(),
            0x0403_0201
        );
        // Guest-visible writes never reach ROM and never invoke the callback.
        let mut commits = 0;
        assert_eq!(
            memory.store_u8(GuestPhysicalAddress::new(0x3010), 9, |_| commits += 1),
            Err(MemoryAccessError::ReadOnlyRegion(MemoryRegionKind::Rom))
        );
        assert_eq!(commits, 0);
        assert_eq!(
            memory.load_u8(GuestPhysicalAddress::new(0x3010)).unwrap(),
            1
        );
        // No page generations: ROM is never tracked code.
        assert_eq!(
            memory.code_page_generation(GuestPhysicalAddress::new(0x3000)),
            Err(MemoryAccessError::UnreadableRegion(MemoryRegionKind::Rom))
        );
        // The loader writes only ROM and only inside one region.
        assert_eq!(
            memory.load_rom_bytes(GuestPhysicalAddress::new(0x1000), &[1]),
            Err(MemoryAccessError::ReadOnlyRegion(MemoryRegionKind::Ram))
        );
        assert!(matches!(
            memory.load_rom_bytes(GuestPhysicalAddress::new(0x3ffe), &[1, 2, 3]),
            Err(MemoryAccessError::AccessCrossesRegion(_) | MemoryAccessError::Unmapped(_))
        ));
    }

    #[test]
    fn committed_write_notification_is_single_and_never_fires_on_rejection() {
        let memory = memory(&[(0x1000, 0x1000, MemoryRegionKind::Ram)]);
        let mut commits = Vec::new();
        memory
            .store_u16(GuestPhysicalAddress::new(0x1002), 0xa1b2, |range| {
                commits.push(range)
            })
            .unwrap();
        assert_eq!(commits, vec![range(0x1002, 2)]);
        assert!(
            memory
                .store_u8(GuestPhysicalAddress::new(0x3000), 1, |range| commits
                    .push(range))
                .is_err()
        );
        assert_eq!(commits, vec![range(0x1002, 2)]);
        assert!(
            memory
                .load_bytes(GuestPhysicalAddress::new(0x1002), &mut [])
                .is_err()
        );
    }

    #[test]
    fn committed_writes_advance_only_the_physical_pages_they_touch() {
        let memory = memory(&[(0x1000, 0x3000, MemoryRegionKind::Ram)]);
        let first = GuestPhysicalAddress::new(0x1000);
        let second = GuestPhysicalAddress::new(0x2000);
        let third = GuestPhysicalAddress::new(0x3000);
        assert_eq!(memory.code_page_generation(first), Ok(1));
        assert_eq!(memory.code_page_generation(second), Ok(1));
        assert_eq!(memory.code_page_generation(third), Ok(1));

        memory
            .store_u8(GuestPhysicalAddress::new(0x1010), 0x90, |_| {})
            .unwrap();
        assert_eq!(memory.code_page_generation(first), Ok(2));
        assert_eq!(memory.code_page_generation(second), Ok(1));
        assert_eq!(memory.code_page_generation(third), Ok(1));

        assert!(
            memory
                .store_bytes(GuestPhysicalAddress::new(0x3fff), &[1, 2], |_| {})
                .is_err()
        );
        assert_eq!(memory.code_page_generation(third), Ok(1));

        memory
            .store_bytes(GuestPhysicalAddress::new(0x1fff), &[1, 2], |_| {})
            .unwrap();
        assert_eq!(memory.code_page_generation(first), Ok(3));
        assert_eq!(memory.code_page_generation(second), Ok(2));
        assert_eq!(memory.code_page_generation(third), Ok(1));
    }

    #[test]
    fn write_crossing_adjacent_ram_regions_is_atomic_and_unnotified() {
        let memory = memory(&[
            (0x1000, 2, MemoryRegionKind::Ram),
            (0x1002, 2, MemoryRegionKind::Ram),
        ]);
        let mut commits = Vec::new();
        assert!(matches!(
            memory.store_bytes(GuestPhysicalAddress::new(0x1000), &[1, 2, 3, 4], |range| {
                commits.push(range)
            }),
            Err(MemoryAccessError::AccessCrossesRegion(_))
        ));
        assert!(commits.is_empty());
        let mut first = [0xff; 2];
        let mut second = [0xff; 2];
        memory
            .load_bytes(GuestPhysicalAddress::new(0x1000), &mut first)
            .unwrap();
        memory
            .load_bytes(GuestPhysicalAddress::new(0x1002), &mut second)
            .unwrap();
        assert_eq!(first, [0; 2]);
        assert_eq!(second, [0; 2]);
    }

    #[test]
    fn framebuffer_backing_is_outside_ram_limits_and_writes_never_notify() {
        const FB_START: u64 = 0x10_0000;
        let map = MachineMemoryMap::new(
            36,
            vec![
                MemoryRegionSpec::new(range(0x1000, 0x1000), MemoryRegionKind::Ram),
                MemoryRegionSpec::new(range(FB_START, 0x4000), MemoryRegionKind::Framebuffer),
            ],
        )
        .unwrap();
        // The RAM budget covers the RAM region only (0x1000), not the 0x4000 bytes of pixels.
        let memory = PhysicalMemory::new(&map, RamLimits::new(0x1000)).unwrap();
        let mut commits = Vec::new();
        memory
            .store_bytes(
                GuestPhysicalAddress::new(FB_START + 0x3ffc),
                &[1, 2, 3, 4],
                |r| commits.push(r),
            )
            .unwrap();
        assert!(commits.is_empty());
        assert_eq!(
            memory.load_u32(GuestPhysicalAddress::new(FB_START + 0x3ffc)),
            Ok(0x0403_0201)
        );
        // Zero-filled at power-on.
        assert_eq!(memory.load_u64(GuestPhysicalAddress::new(FB_START)), Ok(0));
        // RAM in the same memory still notifies exactly once.
        memory
            .store_u8(GuestPhysicalAddress::new(0x1000), 7, |r| commits.push(r))
            .unwrap();
        assert_eq!(commits, vec![range(0x1000, 1)]);
        // Crossing the end of the region is rejected without a partial write.
        assert!(matches!(
            memory.store_bytes(
                GuestPhysicalAddress::new(FB_START + 0x3ffe),
                &[9; 4],
                |_| {}
            ),
            Err(MemoryAccessError::AccessCrossesRegion(_))
        ));
        assert_eq!(
            memory.load_u16(GuestPhysicalAddress::new(FB_START + 0x3ffe)),
            Ok(0x0403)
        );
        // Frame copy-out is available only for Framebuffer regions.
        let mut out = [0u8; 4];
        memory
            .read_framebuffer_bytes(GuestPhysicalAddress::new(FB_START + 0x3ffc), &mut out)
            .unwrap();
        assert_eq!(out, [1, 2, 3, 4]);
        assert_eq!(
            memory.read_framebuffer_bytes(GuestPhysicalAddress::new(0x1000), &mut out),
            Err(MemoryAccessError::UnreadableRegion(MemoryRegionKind::Ram))
        );
    }

    #[test]
    fn framebuffer_has_no_page_generations_and_ram_writes_still_bump_them() {
        const FB_START: u64 = 0x10_0000;
        let map = MachineMemoryMap::new(
            36,
            vec![
                MemoryRegionSpec::new(range(0x1000, 0x2000), MemoryRegionKind::Ram),
                MemoryRegionSpec::new(range(FB_START, 0x4000), MemoryRegionKind::Framebuffer),
            ],
        )
        .unwrap();
        let memory = PhysicalMemory::new(&map, RamLimits::new(0x2000)).unwrap();
        let ram0 = GuestPhysicalAddress::new(0x1000);
        let ram1 = GuestPhysicalAddress::new(0x2000);
        assert_eq!(memory.code_page_generation(ram0), Ok(1));
        assert_eq!(memory.code_page_generation(ram1), Ok(1));

        // Framebuffer: the generation API rejects every page of the region,
        // including the first and the last one.
        for offset in [0, 0x1000, 0x3fff] {
            assert_eq!(
                memory.code_page_generation(GuestPhysicalAddress::new(FB_START + offset)),
                Err(MemoryAccessError::UnreadableRegion(
                    MemoryRegionKind::Framebuffer
                ))
            );
        }

        // Writes to the framebuffer (page-crossing and last page) succeed
        // without callback and without touching RAM generations.
        let mut commits = Vec::new();
        for (offset, bytes) in [
            (0u64, &[1u8][..]),
            (0xffe, &[2, 3, 4, 5][..]),
            (0x3ffc, &[6, 7, 8, 9][..]),
        ] {
            memory
                .store_bytes(GuestPhysicalAddress::new(FB_START + offset), bytes, |r| {
                    commits.push(r)
                })
                .unwrap();
        }
        assert!(commits.is_empty());
        assert_eq!(
            memory.load_u32(GuestPhysicalAddress::new(FB_START + 0x3ffc)),
            Ok(0x0908_0706)
        );
        assert_eq!(memory.code_page_generation(ram0), Ok(1));
        assert_eq!(memory.code_page_generation(ram1), Ok(1));

        // RAM writes still bump exactly the touched generations and notify once.
        memory
            .store_bytes(GuestPhysicalAddress::new(0x1ffe), &[1, 2, 3, 4], |r| {
                commits.push(r)
            })
            .unwrap();
        assert_eq!(commits, vec![range(0x1ffe, 4)]);
        assert_eq!(memory.code_page_generation(ram0), Ok(2));
        assert_eq!(memory.code_page_generation(ram1), Ok(2));
    }

    #[test]
    fn guest_pages_and_host_sized_chunks_are_ordinary_ram_bytes() {
        // This region straddles a deterministic 16 KiB boundary; no host page
        // size is queried or assumed by the implementation.
        let memory = memory(&[(0, 0x9000, MemoryRegionKind::Ram)]);
        let bytes: Vec<u8> = (0..0x30).map(|value| value as u8).collect();
        let start = GuestPhysicalAddress::new(0x3ff0);
        let mut commits = Vec::new();
        memory
            .store_bytes(start, &bytes, |range| commits.push(range))
            .unwrap();
        let mut read_back = vec![0; bytes.len()];
        memory.load_bytes(start, &mut read_back).unwrap();
        assert_eq!(read_back, bytes);
        assert_eq!(commits, vec![range(0x3ff0, 0x30)]);
    }

    // ---- S6-30a: storage and `&self` access functions ----

    /// Regions with odd starts (host lead 3 and 5), an adjacent RAM pair, ROM, Reserved,
    /// Framebuffer and holes.
    fn storage_test_memory() -> PhysicalMemory {
        memory(&[
            (0x1003, 0x1ffd, MemoryRegionKind::Ram),
            (0x3000, 0x805, MemoryRegionKind::Ram),
            (0x4000, 0x100, MemoryRegionKind::Rom),
            (0x5000, 0x100, MemoryRegionKind::Reserved),
            (0x6000, 0x1000, MemoryRegionKind::Framebuffer),
        ])
    }

    const PROBE_START: u64 = 0x0ff0;
    const PROBE_END: u64 = 0x7010;

    fn dump(memory: &PhysicalMemory) -> Vec<Result<u8, MemoryAccessError>> {
        (PROBE_START..PROBE_END)
            .map(|at| memory.load_u8(GuestPhysicalAddress::new(at)))
            .collect()
    }

    struct XorShift(u64);

    impl XorShift {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
    }

    fn random_address(rng: &mut XorShift) -> GuestPhysicalAddress {
        // Mostly near region and page edges, sometimes anywhere.
        const EDGES: [u64; 12] = [
            0x1003, 0x2000, 0x3000, 0x3805, 0x4000, 0x4100, 0x5000, 0x5100, 0x6000, 0x7000, 0x1000,
            0x0fff,
        ];
        let near = EDGES[(rng.next() % EDGES.len() as u64) as usize];
        let at = if rng.next() % 4 == 0 {
            PROBE_START + rng.next() % (PROBE_END - PROBE_START)
        } else {
            near.wrapping_add(rng.next() % 24).wrapping_sub(12)
        };
        GuestPhysicalAddress::new(at)
    }

    /// Plain `Vec<u8>` model of the whole probe window: every successful store of the new API is
    /// applied to it, every successful load of the new API must equal it.
    struct ByteModel(Vec<u8>);

    impl ByteModel {
        fn new() -> Self {
            Self(vec![0; (PROBE_END - PROBE_START) as usize])
        }

        fn at(at: GuestPhysicalAddress) -> usize {
            (at.0 - PROBE_START) as usize
        }

        fn store(&mut self, at: GuestPhysicalAddress, bytes: &[u8]) {
            let from = Self::at(at);
            self.0[from..from + bytes.len()].copy_from_slice(bytes);
        }

        fn bytes(&self, at: GuestPhysicalAddress, len: usize) -> &[u8] {
            let from = Self::at(at);
            &self.0[from..from + len]
        }
    }

    /// Independent reference of the region rules (written from the map of
    /// `storage_test_memory`, not from `PhysicalMemory`): the error, in the order of the checks, of
    /// an access of `len` bytes at `at`, or `Ok` with the kind of the region it lands in.
    fn reference_access(
        at: u64,
        len: u64,
        write: bool,
    ) -> Result<MemoryRegionKind, MemoryAccessError> {
        const MAP: [(u64, u64, MemoryRegionKind); 5] = [
            (0x1003, 0x1ffd, MemoryRegionKind::Ram),
            (0x3000, 0x805, MemoryRegionKind::Ram),
            (0x4000, 0x100, MemoryRegionKind::Rom),
            (0x5000, 0x100, MemoryRegionKind::Reserved),
            (0x6000, 0x1000, MemoryRegionKind::Framebuffer),
        ];
        if len == 0 {
            return Err(MemoryAccessError::EmptyRange);
        }
        if u128::from(at) + u128::from(len) > 1u128 << 64 {
            return Err(MemoryAccessError::AddressOverflow);
        }
        let Some(&(start, size, kind)) = MAP
            .iter()
            .find(|&&(start, size, _)| at >= start && at < start + size)
        else {
            return Err(MemoryAccessError::Unmapped(GuestPhysicalAddress::new(at)));
        };
        if at + len > start + size {
            return Err(MemoryAccessError::AccessCrossesRegion(range(at, len)));
        }
        match (kind, write) {
            (MemoryRegionKind::Reserved, false) => Err(MemoryAccessError::UnreadableRegion(kind)),
            (MemoryRegionKind::Rom | MemoryRegionKind::Reserved, true) => {
                Err(MemoryAccessError::ReadOnlyRegion(kind))
            }
            _ => Ok(kind),
        }
    }

    /// Pages of the two RAM regions of `storage_test_memory`, one probe address each.
    const GENERATION_PROBES: [u64; 3] = [0x1003, 0x2000, 0x3000];

    /// The access functions against two independent references: the region rules
    /// (`reference_access`: errors, in their order) and a plain `Vec<u8>` model of the whole probe
    /// window (every successful store is applied to it, every successful load must equal it, the
    /// final dump must equal it). Also checked on every step: the notifications (none on a
    /// rejection or a Framebuffer store, otherwise exactly one range of the written bytes) and the
    /// generation of every RAM page. Random operations hit edges, holes, ROM, Reserved,
    /// Framebuffer, adjacent regions and every width and alignment.
    #[test]
    fn access_functions_match_the_region_rules_and_a_byte_model() {
        for seed in [
            0x9e37_79b9_7f4a_7c15u64,
            0x1234_5678_9abc_def1,
            0xdead_beef_cafe_f00d,
        ] {
            let mut rng = XorShift(seed);
            let memory = storage_test_memory();
            let mut model = ByteModel::new();
            let mut generations = [1u64; 3];
            let mut loads = 0u32;
            for step in 0..20_000u32 {
                let at = random_address(&mut rng);
                let value = rng.next();
                let mut commits = Vec::new();
                // `Some(bytes)` for a store, `None` for a load.
                let (len, store): (usize, Option<Vec<u8>>) = match rng.next() % 12 {
                    0 => (1, Some((value as u8).to_le_bytes().to_vec())),
                    1 => (2, Some((value as u16).to_le_bytes().to_vec())),
                    2 => (4, Some((value as u32).to_le_bytes().to_vec())),
                    3 => (8, Some(value.to_le_bytes().to_vec())),
                    4 => {
                        let len = (rng.next() % 26) as usize; // 0 is the empty-range error
                        (len, Some((0..len).map(|_| rng.next() as u8).collect()))
                    }
                    5 => (1, None),
                    6 => (2, None),
                    7 => (4, None),
                    8 => (8, None),
                    _ => ((rng.next() % 26) as usize, None),
                };
                let expected = reference_access(at.0, len as u64, store.is_some());
                match store {
                    Some(bytes) => {
                        let result = match (len, bytes.len()) {
                            (1, 1) if step % 3 != 1 => {
                                memory.store_u8(at, bytes[0], |r| commits.push(r))
                            }
                            (2, 2) => memory.store_u16(
                                at,
                                u16::from_le_bytes([bytes[0], bytes[1]]),
                                |r| commits.push(r),
                            ),
                            (4, 4) => memory.store_u32(
                                at,
                                u32::from_le_bytes(bytes[..4].try_into().unwrap()),
                                |r| commits.push(r),
                            ),
                            (8, 8) => memory.store_u64(
                                at,
                                u64::from_le_bytes(bytes[..8].try_into().unwrap()),
                                |r| commits.push(r),
                            ),
                            _ => memory.store_bytes(at, &bytes, |r| commits.push(r)),
                        };
                        assert_eq!(
                            result,
                            expected.clone().map(|_| ()),
                            "step {step} store {at:?} len {len}"
                        );
                        if let Ok(kind) = expected {
                            model.store(at, &bytes);
                            if kind == MemoryRegionKind::Ram {
                                assert_eq!(
                                    commits,
                                    vec![range(at.0, len as u64)],
                                    "step {step} notification"
                                );
                                for page in at.0 >> 12..=(at.0 + len as u64 - 1) >> 12 {
                                    generations[page as usize - 1] += 1;
                                }
                            } else {
                                assert!(commits.is_empty(), "step {step} Framebuffer notified");
                            }
                        } else {
                            assert!(commits.is_empty(), "step {step} rejected store notified");
                        }
                    }
                    None => {
                        let mut buffer = vec![0xeeu8; len];
                        let result = match len {
                            1 if step % 3 != 1 => memory.load_u8(at).map(|v| vec![v]),
                            2 => memory.load_u16(at).map(|v| v.to_le_bytes().to_vec()),
                            4 => memory.load_u32(at).map(|v| v.to_le_bytes().to_vec()),
                            8 => memory.load_u64(at).map(|v| v.to_le_bytes().to_vec()),
                            _ => memory.load_bytes(at, &mut buffer).map(|()| buffer.clone()),
                        };
                        match (result, expected) {
                            (Ok(bytes), Ok(_)) => {
                                assert_eq!(
                                    bytes,
                                    model.bytes(at, len),
                                    "step {step} load {at:?} len {len}"
                                );
                                loads += 1;
                            }
                            (Err(error), Err(reference)) => {
                                assert_eq!(error, reference, "step {step} load {at:?} len {len}")
                            }
                            (got, want) => panic!("step {step} load {at:?}: {got:?} vs {want:?}"),
                        }
                    }
                }
                if step % 64 == 0 {
                    for (probe, expected) in GENERATION_PROBES.iter().zip(generations) {
                        assert_eq!(
                            memory.code_page_generation(GuestPhysicalAddress::new(*probe)),
                            Ok(expected),
                            "step {step} generation of {probe:#x}"
                        );
                    }
                }
            }
            assert!(loads > 1000);
            // Every byte of the final dump: an error exactly where the rules give one, otherwise
            // the model.
            for (index, byte) in dump(&memory).into_iter().enumerate() {
                let at = PROBE_START + index as u64;
                match reference_access(at, 1, false) {
                    Ok(_) => assert_eq!(byte, Ok(model.0[index]), "seed {seed:#x} byte {at:#x}"),
                    Err(error) => assert_eq!(byte, Err(error), "seed {seed:#x} byte {at:#x}"),
                }
            }
            for (probe, expected) in GENERATION_PROBES.iter().zip(generations) {
                assert_eq!(
                    memory.code_page_generation(GuestPhysicalAddress::new(*probe)),
                    Ok(expected),
                    "seed {seed:#x} final generation of {probe:#x}"
                );
            }
            assert!(generations.iter().all(|&g| g > 1));
        }
    }

    /// Two shared references to one memory see each other's stores at once (no `&mut` is needed
    /// anywhere), whichever width writes and whichever reads.
    #[test]
    fn stores_through_one_shared_reference_are_seen_through_another() {
        let memory = storage_test_memory();
        let (first, second) = (&memory, &memory);
        let mut rng = XorShift(0x0123_4567_89ab_cdef);
        for _ in 0..5_000 {
            let at = random_address(&mut rng);
            let value = rng.next();
            let width = [1usize, 2, 4, 8][(rng.next() % 4) as usize];
            let bytes = &value.to_le_bytes()[..width];
            let (writer, reader) = if rng.next() % 2 == 0 {
                (first, second)
            } else {
                (second, first)
            };
            let stored = writer.store_bytes(at, bytes, |_| {});
            assert_eq!(
                stored,
                reference_access(at.0, width as u64, true).map(|_| ())
            );
            let mut back = vec![0u8; width];
            let loaded = reader.load_bytes(at, &mut back);
            assert_eq!(
                loaded,
                reference_access(at.0, width as u64, false).map(|_| ())
            );
            if stored.is_ok() {
                assert_eq!(back, bytes);
            }
        }
    }

    #[test]
    fn boundary_errors_follow_the_region_rules() {
        let memory = storage_test_memory();
        // Region end, adjacent RAM regions, hole, region start, top of the address space.
        for at in [
            0x2fff_u64,
            0x3804,
            0x3805,
            0x0fff,
            0x1002,
            0x4fff,
            0x40ff,
            0x50ff,
            0x6ff9,
            0x7000,
            u64::MAX,
            u64::MAX - 3,
        ] {
            let address = GuestPhysicalAddress::new(at);
            let load = |len: u64| reference_access(at, len, false).map(|_| ());
            let store = |len: u64| reference_access(at, len, true).map(|_| ());
            assert_eq!(memory.load_u8(address).map(|_| ()), load(1), "{at:#x}");
            assert_eq!(memory.load_u16(address).map(|_| ()), load(2), "{at:#x}");
            assert_eq!(memory.load_u32(address).map(|_| ()), load(4), "{at:#x}");
            assert_eq!(memory.load_u64(address).map(|_| ()), load(8), "{at:#x}");
            assert_eq!(memory.load_bytes(address, &mut [0u8; 8]), load(8));
            assert_eq!(memory.load_bytes(address, &mut []), load(0));
            assert_eq!(memory.store_u8(address, 1, |_| {}), store(1), "{at:#x}");
            assert_eq!(memory.store_u16(address, 1, |_| {}), store(2), "{at:#x}");
            assert_eq!(memory.store_u32(address, 1, |_| {}), store(4), "{at:#x}");
            assert_eq!(memory.store_u64(address, 1, |_| {}), store(8), "{at:#x}");
            assert_eq!(memory.store_bytes(address, &[], |_| {}), store(0));
        }
    }

    #[test]
    fn rejected_stores_leave_memory_unchanged() {
        let memory = storage_test_memory();
        let before = dump(&memory);
        // ROM, Reserved, hole, crossing from RAM into the gap after it, empty range.
        for at in [0x4000_u64, 0x50f8, 0x0f00, 0x3804] {
            assert!(
                memory
                    .store_u64(GuestPhysicalAddress::new(at), u64::MAX, |_| {})
                    .is_err()
            );
        }
        assert_eq!(
            memory.store_bytes(GuestPhysicalAddress::new(0x2000), &[], |_| {}),
            Err(MemoryAccessError::EmptyRange)
        );
        assert_eq!(dump(&memory), before);
    }

    #[test]
    fn framebuffer_is_writable_by_stores_without_generations() {
        let memory = storage_test_memory();
        let at = GuestPhysicalAddress::new(0x6ff8);
        memory.store_u64(at, 0x0102_0304_0506_0708, |_| {}).unwrap();
        assert_eq!(memory.load_u64(at), Ok(0x0102_0304_0506_0708));
        assert_eq!(memory.read_framebuffer_bytes(at, &mut [0u8; 8]), Ok(()));
        assert_eq!(
            memory.store_u16(GuestPhysicalAddress::new(0x6fff), 1, |_| {}),
            Err(MemoryAccessError::AccessCrossesRegion(range(0x6fff, 2)))
        );
    }

    /// An aligned 1/2/4/8-byte access is a single host access of that width, also in a region
    /// whose start is not 8-aligned; a misaligned access is split into ascending bytes.
    #[test]
    fn aligned_accesses_are_single_host_accesses() {
        use crate::ram_storage::traced;
        let memory = storage_test_memory();
        // Region 0x1003: offset = address - 0x1003.
        let base = 0x1003u64;
        for (width, address) in [
            (1u64, 0x1010u64),
            (2, 0x1010),
            (4, 0x1010),
            (8, 0x1010),
            (8, 0x1fc8),
            (4, 0x1ffc),
        ] {
            let at = GuestPhysicalAddress::new(address);
            let offset = (address - base) as usize;
            let expect = vec![(offset, width as usize)];
            let ((), write) = traced(|| match width {
                1 => memory.store_u8(at, 2, |_| {}).unwrap(),
                2 => memory.store_u16(at, 2, |_| {}).unwrap(),
                4 => memory.store_u32(at, 2, |_| {}).unwrap(),
                _ => memory.store_u64(at, 2, |_| {}).unwrap(),
            });
            let ((), read) = traced(|| match width {
                1 => drop(memory.load_u8(at).unwrap()),
                2 => drop(memory.load_u16(at).unwrap()),
                4 => drop(memory.load_u32(at).unwrap()),
                _ => drop(memory.load_u64(at).unwrap()),
            });
            assert_eq!(write, expect, "store width {width} at {address:#x}");
            assert_eq!(read, expect, "load width {width} at {address:#x}");
        }
        // Misaligned dword: ascending bytes.
        let at = GuestPhysicalAddress::new(0x1011);
        let (_, log) = traced(|| memory.store_u32(at, 9, |_| {}).unwrap());
        assert_eq!(
            log,
            (0..4).map(|i| (0x1011 - 0x1003 + i, 1)).collect::<Vec<_>>()
        );
    }

    /// S6-25, S6-31: the compare-and-replace is ONE host compare-exchange of 8 bytes (S6-25: a
    /// read and a write; the change is the point of S6-31), on a match and on a mismatch alike, also
    /// in a region with an odd start.
    #[test]
    fn compare_replace_u64_is_one_host_compare_exchange_on_the_new_storage() {
        use crate::ram_storage::traced;
        let memory = storage_test_memory();
        let at = GuestPhysicalAddress::new(0x1008); // aligned guest address, region start 0x1003
        memory.store_u64(at, 5, |_| {}).unwrap();
        let (result, log) = traced(|| memory.compare_replace_u64(at, 5, 6, |_| {}));
        assert_eq!(result, Ok(Ok(())));
        assert_eq!(log, vec![(5, 8)]);
        let (result, log) = traced(|| memory.compare_replace_u64(at, 5, 7, |_| {}));
        assert_eq!(result, Ok(Err(6)));
        assert_eq!(log, vec![(5, 8)]);
        assert_eq!(memory.load_u64(at), Ok(6));
    }

    #[test]
    fn physical_memory_can_be_shared_between_threads_by_reference() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<PhysicalMemory>();
        let mut memory = storage_test_memory();
        memory.set_execution_mode(ExecutionMode::Parallel);
        let words = [0x1008u64, 0x1010, 0x1ff8];
        std::thread::scope(|scope| {
            for thread in 0..4u8 {
                let memory = &memory;
                scope.spawn(move || {
                    for round in 0..20_000u32 {
                        let fill = (round as u8).wrapping_mul(3).wrapping_add(thread) | 1;
                        let at = GuestPhysicalAddress::new(words[round as usize % words.len()]);
                        if thread % 2 == 0 {
                            memory
                                .store_u64(at, u64::from_ne_bytes([fill; 8]), |_| {})
                                .unwrap();
                        } else {
                            let bytes = memory.load_u64(at).unwrap().to_ne_bytes();
                            assert!(bytes.iter().all(|&byte| byte == bytes[0]), "torn {bytes:?}");
                        }
                    }
                });
            }
        });
    }

    // ---- S6-30b: atomic page generations and the commit notification ----

    /// The callback of a store runs once, after the data is stored and after the generation of
    /// every touched page has been raised (a store crossing a page boundary raises both).
    #[test]
    fn the_store_notifies_once_after_the_generations_are_raised() {
        let memory = storage_test_memory();
        let in_page = GuestPhysicalAddress::new(0x1010);
        let across = GuestPhysicalAddress::new(0x1ffc); // 4 bytes, then the next page (0x2000)
        let generation = |at: u64| memory.code_page_generation(GuestPhysicalAddress::new(at));
        let (g1, g2) = (generation(0x1010).unwrap(), generation(0x2000).unwrap());
        let mut seen = Vec::new();
        memory
            .store_u32(in_page, 0xdead_beef, |r| {
                seen.push((r, generation(0x1010), memory.load_u32(in_page)))
            })
            .unwrap();
        assert_eq!(seen, vec![(range(0x1010, 4), Ok(g1 + 1), Ok(0xdead_beef))]);
        let mut seen = Vec::new();
        memory
            .store_u64(across, 7, |r| {
                seen.push((r, generation(0x1010), generation(0x2000)))
            })
            .unwrap();
        assert_eq!(
            seen,
            vec![(range(0x1ffc, 8), Ok(g1 + 2), Ok(g2 + 1))],
            "both touched pages are raised before the callback, once each"
        );
        // A rejected store (hole, ROM, empty) raises nothing and does not call back.
        let mut calls = 0;
        for at in [0x0f00u64, 0x4000, 0x3804] {
            assert!(
                memory
                    .store_u16(GuestPhysicalAddress::new(at), 1, |_| calls += 1)
                    .is_err()
            );
        }
        assert!(memory.store_bytes(in_page, &[], |_| calls += 1).is_err());
        assert_eq!(calls, 0);
        assert_eq!(
            (generation(0x1010), generation(0x2000)),
            (Ok(g1 + 2), Ok(g2 + 1))
        );
    }

    /// A page generation at the top of `u64` rejects the write before any byte is stored, and a
    /// write that does not touch that page is unaffected.
    #[test]
    fn an_exhausted_generation_rejects_the_write() {
        let memory = storage_test_memory();
        memory.regions[0].page_generations[0].store(u64::MAX, Ordering::Relaxed);
        let at = GuestPhysicalAddress::new(0x1010);
        assert_eq!(
            memory.store_u32(at, 1, |_| panic!("no callback")),
            Err(MemoryAccessError::CodeGenerationExhausted)
        );
        assert_eq!(memory.load_u32(at), Ok(0));
        assert_eq!(
            memory.store_u32(GuestPhysicalAddress::new(0x2010), 1, |_| {}),
            Ok(())
        );
    }

    /// Four writer threads each own one page and all raise one shared page, through `&self`.
    /// The final generation of every page is exact (no lost increment), each owner sees its page
    /// advance by exactly one per store, and an observer thread sees every page monotone. Data and
    /// generation are tied by `Release`/`Acquire`: the writer stores the word `n`, then raises the
    /// generation to `1 + n`, so a reader that loads generation `g` and then the word never sees
    /// a word below `g - 1`.
    #[test]
    fn page_generations_are_exact_and_monotone_under_concurrent_writes() {
        use std::sync::atomic::AtomicBool;
        const WRITERS: u64 = 4;
        const ROUNDS: u64 = 5_000;
        // Pages 0x1000..=0x4000 are private, 0x5000 is shared.
        let mut memory = memory(&[(0x1000, 0x5000, MemoryRegionKind::Ram)]);
        memory.set_execution_mode(ExecutionMode::Parallel);
        let private = |index: u64| GuestPhysicalAddress::new(0x1000 + index * 0x1000 + 8);
        let shared = |index: u64| GuestPhysicalAddress::new(0x5000 + index);
        let done = AtomicBool::new(false);
        std::thread::scope(|scope| {
            let observer = scope.spawn(|| {
                let mut last = [1u64; (WRITERS + 1) as usize];
                let mut sweeps = 0u64;
                while !done.load(Ordering::Acquire) || sweeps == 0 {
                    for index in 0..WRITERS {
                        let at = private(index);
                        let seen = memory.code_page_generation(at).unwrap();
                        let word = memory.load_u64(at).unwrap();
                        assert!(seen >= last[index as usize], "page {index} went backwards");
                        assert!(
                            word + 1 >= seen,
                            "page {index}: generation {seen}, word {word}"
                        );
                        last[index as usize] = seen;
                    }
                    let seen = memory.code_page_generation(shared(0)).unwrap();
                    assert!(seen >= last[WRITERS as usize], "shared page went backwards");
                    last[WRITERS as usize] = seen;
                    sweeps += 1;
                }
            });
            let writers: Vec<_> = (0..WRITERS)
                .map(|index| {
                    let memory = &memory;
                    scope.spawn(move || {
                        for round in 1..=ROUNDS {
                            memory.store_u64(private(index), round, |_| {}).unwrap();
                            // Only the owner writes this page: one raise per store, exactly.
                            assert_eq!(memory.code_page_generation(private(index)), Ok(1 + round));
                            memory.store_u8(shared(index), round as u8, |_| {}).unwrap();
                        }
                    })
                })
                .collect();
            for writer in writers {
                writer.join().unwrap();
            }
            done.store(true, Ordering::Release);
            observer.join().unwrap();
        });
        for index in 0..WRITERS {
            assert_eq!(memory.code_page_generation(private(index)), Ok(1 + ROUNDS));
        }
        assert_eq!(
            memory.code_page_generation(shared(0)),
            Ok(1 + WRITERS * ROUNDS),
            "shared page: no lost increment"
        );
    }

    fn width_of(bytes: usize) -> AccessWidth {
        match bytes {
            1 => AccessWidth::Byte,
            2 => AccessWidth::Word,
            4 => AccessWidth::Dword,
            _ => AccessWidth::Qword,
        }
    }

    fn width_mask(bytes: usize) -> u64 {
        if bytes == 8 {
            u64::MAX
        } else {
            (1u64 << (8 * bytes)) - 1
        }
    }

    /// S6-31: the locked operations on random aligned operands of every width, in two RAM regions
    /// with different host leads, against a byte model: old value returned zero-extended, the new
    /// value wrapped at the width, neighbours untouched, ONE notification and ONE generation
    /// raise per operation (also for a compare that does not match: the CMPXCHG write), ONE host
    /// access for swap, add and compare-exchange.
    #[test]
    fn locked_operations_follow_a_byte_model_and_notify_once() {
        use crate::ram_storage::traced;
        let memory = storage_test_memory();
        let mut model: std::collections::BTreeMap<u64, u8> = std::collections::BTreeMap::new();
        let mut rng = XorShift(0x5eed_0031);
        let mut count = 0u64;
        for _ in 0..30_000 {
            let bytes = [1usize, 2, 4, 8][(rng.next() % 4) as usize];
            let (base, span) = if rng.next() % 2 == 0 {
                (0x1008u64, 0x1000u64)
            } else {
                (0x3000u64, 0x800u64)
            };
            let at = base + (rng.next() % (span / bytes as u64)) * bytes as u64;
            let address = GuestPhysicalAddress::new(at);
            let width = width_of(bytes);
            let mask = width_mask(bytes);
            let mut old = 0u64;
            for i in 0..bytes {
                old |= u64::from(*model.get(&(at + i as u64)).unwrap_or(&0)) << (8 * i);
            }
            let operand = rng.next();
            let generation = memory.code_page_generation(address).unwrap();
            let mut notes = Vec::new();
            let (kind, new, got, log) = match rng.next() % 4 {
                0 => {
                    let (got, log) =
                        traced(|| memory.atomic_swap(address, width, operand, |r| notes.push(r)));
                    ("swap", operand & mask, got, Some(log))
                }
                1 => {
                    let (got, log) = traced(|| {
                        memory.atomic_fetch_add(address, width, operand, |r| notes.push(r))
                    });
                    ("add", old.wrapping_add(operand) & mask, got, Some(log))
                }
                2 => {
                    // Half of the compares match (the expected value carries junk above the width).
                    let matching = rng.next() % 2 == 0;
                    let expected = if matching {
                        old | (operand & !mask)
                    } else {
                        old ^ 1
                    };
                    let (got, log) = traced(|| {
                        memory.atomic_compare_exchange(address, width, expected, operand, |r| {
                            notes.push(r)
                        })
                    });
                    let new = if matching { operand & mask } else { old };
                    ("cas", new, got, Some(log))
                }
                _ => {
                    let mut calls = 0;
                    let got = memory.atomic_rmw(
                        address,
                        width,
                        &mut |value| {
                            calls += 1;
                            value ^ operand
                        },
                        |r| notes.push(r),
                    );
                    assert_eq!(calls, 1, "no concurrent writer: the update runs once");
                    ("rmw", (old ^ operand) & mask, got, None)
                }
            };
            assert_eq!(got, Ok(old), "{kind} at {at:#x} width {bytes}");
            if let Some(log) = log {
                let region_start = if base == 0x1008 { 0x1003 } else { 0x3000 };
                assert_eq!(
                    log,
                    vec![((at - region_start) as usize, bytes)],
                    "{kind}: one host access"
                );
            }
            assert_eq!(
                notes,
                vec![range(at, bytes as u64)],
                "{kind}: one notification"
            );
            assert_eq!(
                memory.code_page_generation(address),
                Ok(generation + 1),
                "{kind}: one raise"
            );
            for i in 0..bytes {
                model.insert(at + i as u64, (new >> (8 * i)) as u8);
            }
            count += 1;
        }
        assert_eq!(count, 30_000);
        // The whole memory equals the model (neighbours untouched).
        for at in PROBE_START..PROBE_END {
            if let Ok(byte) = memory.load_u8(GuestPhysicalAddress::new(at)) {
                assert_eq!(byte, *model.get(&at).unwrap_or(&0), "byte {at:#x}");
            }
        }
        assert_eq!(memory.split_lock_refusals(), 0);
    }

    /// S6-31: errors of the locked operations: a misaligned operand is refused and counted (also
    /// across a page or a region), never executed as two accesses; ROM, a reserved region and a
    /// hole are the errors of a write; none of them changes memory, raises a generation or
    /// notifies; a framebuffer operand is plain memory without generations and without callback.
    #[test]
    fn locked_operations_refuse_what_they_cannot_do_atomically() {
        let memory = storage_test_memory();
        let mut notes = Vec::new();
        let mut refused = 0u64;
        for bytes in [2usize, 4, 8] {
            for delta in 1..bytes as u64 {
                for base in [0x1008u64, 0x1ff8, 0x2ffe, 0x3ff8 - 8] {
                    // `at` is aligned to `bytes` plus `delta`: misaligned by construction.
                    let at = (base & !(bytes as u64 - 1)) + delta;
                    let address = GuestPhysicalAddress::new(at);
                    let width = width_of(bytes);
                    let error = Err(MemoryAccessError::MisalignedAtomic {
                        address,
                        width: bytes as u8,
                    });
                    assert_eq!(
                        memory.atomic_swap(address, width, 1, |r| notes.push(r)),
                        error
                    );
                    assert_eq!(
                        memory.atomic_fetch_add(address, width, 1, |r| notes.push(r)),
                        error
                    );
                    assert_eq!(
                        memory.atomic_compare_exchange(address, width, 0, 1, |r| notes.push(r)),
                        error
                    );
                    assert_eq!(
                        memory.atomic_rmw(address, width, &mut |v| v + 1, |r| notes.push(r)),
                        error
                    );
                    refused += 4;
                    assert_eq!(memory.split_lock_refusals(), refused);
                }
            }
        }
        // A pair of aligned-neighbour bytes proves nothing was written by any refusal above.
        for at in PROBE_START..PROBE_END {
            if let Ok(byte) = memory.load_u8(GuestPhysicalAddress::new(at)) {
                assert_eq!(byte, 0, "refusals write nothing ({at:#x})");
            }
        }
        // Aligned operands in regions that cannot be written atomically.
        let rom = GuestPhysicalAddress::new(0x4000);
        let reserved = GuestPhysicalAddress::new(0x5000);
        let hole = GuestPhysicalAddress::new(0x9000);
        for (address, error) in [
            (
                rom,
                MemoryAccessError::ReadOnlyRegion(MemoryRegionKind::Rom),
            ),
            (
                reserved,
                MemoryAccessError::ReadOnlyRegion(MemoryRegionKind::Reserved),
            ),
            (hole, MemoryAccessError::Unmapped(hole)),
        ] {
            let width = AccessWidth::Dword;
            assert_eq!(
                memory.atomic_swap(address, width, 1, |r| notes.push(r)),
                Err(error.clone())
            );
            assert_eq!(
                memory.atomic_fetch_add(address, width, 1, |r| notes.push(r)),
                Err(error.clone())
            );
            assert_eq!(
                memory.atomic_compare_exchange(address, width, 0, 1, |r| notes.push(r)),
                Err(error.clone())
            );
            assert_eq!(
                memory.atomic_rmw(address, width, &mut |v| v, |r| notes.push(r)),
                Err(error)
            );
        }
        assert_eq!(
            memory.split_lock_refusals(),
            refused,
            "region errors are not counted"
        );
        assert!(notes.is_empty());
        // The framebuffer is plain memory: it works, has no generation and no callback.
        let frame = GuestPhysicalAddress::new(0x6010);
        assert_eq!(
            memory.atomic_fetch_add(frame, AccessWidth::Qword, 5, |r| notes.push(r)),
            Ok(0)
        );
        assert_eq!(
            memory.atomic_swap(frame, AccessWidth::Qword, 9, |r| notes.push(r)),
            Ok(5)
        );
        assert_eq!(memory.load_u64(frame), Ok(9));
        assert!(notes.is_empty());
        assert!(memory.code_page_generation(frame).is_err());
        // An exhausted generation rejects a locked write before any byte changes.
        let exhausted = memory_with_exhausted_generation();
        let at = GuestPhysicalAddress::new(0x1000);
        assert_eq!(
            exhausted.atomic_swap(at, AccessWidth::Dword, 7, |_| {}),
            Err(MemoryAccessError::CodeGenerationExhausted)
        );
        assert_eq!(exhausted.load_u32(at), Ok(0));
    }

    fn memory_with_exhausted_generation() -> PhysicalMemory {
        let memory = memory(&[(0x1000, 0x1000, MemoryRegionKind::Ram)]);
        memory.regions[0].page_generations[0].store(u64::MAX, Ordering::Relaxed);
        memory
    }

    /// S6-41c: the atomic operand window of RAM is the host cell of the operand with the counter
    /// of its page; everything it refuses is refused before any effect and without counting.
    #[test]
    fn the_atomic_operand_window_serves_aligned_ram_and_refuses_the_rest() {
        use std::sync::atomic::AtomicU32;
        let memory = memory(&[
            (0x1003, 0x2000, MemoryRegionKind::Ram),
            (0x4000, 0x1000, MemoryRegionKind::Rom),
            (0x6000, 0x1000, MemoryRegionKind::Mmio),
            (0x8000, 0x1000, MemoryRegionKind::Framebuffer),
        ]);
        let at = GuestPhysicalAddress::new;
        // An aligned 4-byte operand: the pointer reaches the bytes the safe API sees, and the
        // counter is that of its page.
        let window = memory.atomic_operand_window(at(0x1010), 4).unwrap();
        assert_eq!(window.range(), range(0x1010, 4));
        // SAFETY: the pointer is a naturally aligned `u32` cell of the live memory.
        unsafe { AtomicU32::from_ptr(window.host_ptr().cast::<u32>()) }
            .store(0x0102_0304, Ordering::SeqCst);
        assert_eq!(memory.load_u32(at(0x1010)), Ok(0x0102_0304));
        assert_eq!(window.generation().load(Ordering::Acquire), 1);
        // The write above went around `commit_write`: nothing raised the counter (the generated
        // code does), and nothing was notified.
        assert_eq!(memory.code_page_generation(at(0x1010)), Ok(1));
        window.generation().fetch_add(1, Ordering::Release);
        assert_eq!(memory.code_page_generation(at(0x1010)), Ok(2));
        // The page of the next operand is a different counter (the region starts at 0x1003).
        let second = memory.atomic_operand_window(at(0x2008), 8).unwrap();
        assert_eq!(second.generation().load(Ordering::Acquire), 1);
        assert_eq!(memory.code_page_generation(at(0x2008)), Ok(1));

        for width in [1usize, 2, 4, 8] {
            assert!(memory.atomic_operand_window(at(0x1040), width).is_ok());
        }
        assert_eq!(
            memory.atomic_operand_window(at(0x1010), 3).unwrap_err(),
            MemoryAccessError::InvalidPhysicalWidth(3)
        );
        // Misaligned: never inline, not counted (the split-lock path owns it).
        for (address, width) in [(0x1011u64, 2usize), (0x1012, 4), (0x1014, 8), (0x1005, 2)] {
            assert_eq!(
                memory
                    .atomic_operand_window(at(address), width)
                    .unwrap_err(),
                MemoryAccessError::MisalignedAtomic {
                    address: at(address),
                    width: width as u8
                },
                "{address:#x}+{width}"
            );
        }
        assert_eq!(memory.split_lock_refusals(), 0);
        // Not plain RAM, not mapped, crossing the end of the region.
        for address in [0x4000u64, 0x8000] {
            assert!(matches!(
                memory.atomic_operand_window(at(address), 4),
                Err(MemoryAccessError::ReadOnlyRegion(_))
            ));
        }
        assert!(memory.atomic_operand_window(at(0x6000), 4).is_err());
        assert!(matches!(
            memory.atomic_operand_window(at(0x9000), 4),
            Err(MemoryAccessError::Unmapped(_))
        ));
        assert!(matches!(
            memory.atomic_operand_window(at(0x3000), 8),
            Err(MemoryAccessError::AccessCrossesRegion(_) | MemoryAccessError::Unmapped(_))
        ));
        // An exhausted counter: refused (the write would not be recordable).
        assert_eq!(
            memory_with_exhausted_generation()
                .atomic_operand_window(at(0x1000), 4)
                .unwrap_err(),
            MemoryAccessError::CodeGenerationExhausted
        );
    }

    /// S6-31: four host threads on one memory: for every width the fetch-add sum is exact, a
    /// compare-exchange spin lock gives mutual exclusion to an ordinary (non-atomic) load and
    /// store of a counter, a swap-released lock works, and the closure RMW loses no update (the
    /// update runs once per attempt and only the winning attempt is stored). One width per
    /// location (S6-30a invariant d). The lock is released with `atomic_swap`.
    #[test]
    fn four_host_threads_see_every_locked_operation_as_indivisible_at_every_width() {
        const THREADS: u64 = 4;
        const ROUNDS: u64 = 4_000;
        let mut memory = memory(&[(0x1000, 0x2000, MemoryRegionKind::Ram)]);
        memory.set_execution_mode(ExecutionMode::Parallel);
        for (index, bytes) in [1usize, 2, 4, 8].into_iter().enumerate() {
            let width = width_of(bytes);
            let mask = width_mask(bytes);
            let base = 0x1000 + index as u64 * 0x200;
            let (sum, rmw, lock, data) = (
                GuestPhysicalAddress::new(base),
                GuestPhysicalAddress::new(base + 0x40),
                GuestPhysicalAddress::new(base + 0x80),
                GuestPhysicalAddress::new(base + 0xc0),
            );
            let attempts = AtomicU64::new(0);
            let start = std::sync::Barrier::new(THREADS as usize);
            std::thread::scope(|scope| {
                for _ in 0..THREADS {
                    let (memory, attempts, start) = (&memory, &attempts, &start);
                    scope.spawn(move || {
                        start.wait();
                        for _ in 0..ROUNDS {
                            memory.atomic_fetch_add(sum, width, 1, |_| {}).unwrap();
                            let mut calls = 0u64;
                            memory
                                .atomic_rmw(
                                    rmw,
                                    width,
                                    &mut |old| {
                                        calls += 1;
                                        old + 1
                                    },
                                    |_| {},
                                )
                                .unwrap();
                            attempts.fetch_add(calls, Ordering::Relaxed);
                            // Mutual exclusion: the critical section is an ordinary read and write.
                            while memory
                                .atomic_compare_exchange(lock, width, 0, 1, |_| {})
                                .unwrap()
                                != 0
                            {
                                std::hint::spin_loop();
                            }
                            let value = match bytes {
                                1 => u64::from(memory.load_u8(data).unwrap()),
                                2 => u64::from(memory.load_u16(data).unwrap()),
                                4 => u64::from(memory.load_u32(data).unwrap()),
                                _ => memory.load_u64(data).unwrap(),
                            };
                            std::hint::spin_loop();
                            let next = value.wrapping_add(1);
                            match bytes {
                                1 => memory.store_u8(data, next as u8, |_| {}).unwrap(),
                                2 => memory.store_u16(data, next as u16, |_| {}).unwrap(),
                                4 => memory.store_u32(data, next as u32, |_| {}).unwrap(),
                                _ => memory.store_u64(data, next, |_| {}).unwrap(),
                            }
                            memory.atomic_swap(lock, width, 0, |_| {}).unwrap();
                        }
                    });
                }
            });
            let total = (THREADS * ROUNDS) & mask;
            let load = |address| memory.atomic_fetch_add(address, width, 0, |_| {}).unwrap();
            assert_eq!(load(sum), total, "width {bytes}: fetch-add sum");
            assert_eq!(load(rmw), total, "width {bytes}: closure rmw counter");
            assert_eq!(load(data), total, "width {bytes}: counter under the lock");
            assert_eq!(load(lock), 0, "width {bytes}: lock released");
            assert!(
                attempts.load(Ordering::Relaxed) >= THREADS * ROUNDS,
                "width {bytes}: the update ran at least once per call"
            );
        }
    }

    /// S6-31 step 5: the page-table compare-and-replace under real threads. Four threads set
    /// disjoint bits in one entry, each retrying over the observed value (the accessed/dirty
    /// protocol of the walk): no bit is lost, a lost compare changes and notifies nothing, and
    /// the generation rises once per successful replace.
    #[test]
    fn compare_replace_u64_loses_no_bit_under_four_threads() {
        let mut memory = memory(&[(0x1000, 0x1000, MemoryRegionKind::Ram)]);
        memory.set_execution_mode(ExecutionMode::Parallel);
        let entry = GuestPhysicalAddress::new(0x1008);
        let replaced = AtomicU64::new(0);
        let notified = AtomicU64::new(0);
        std::thread::scope(|scope| {
            for thread in 0..4u32 {
                let (memory, replaced, notified) = (&memory, &replaced, &notified);
                scope.spawn(move || {
                    for bit in 0..16u32 {
                        let mask = 1u64 << (thread * 16 + bit);
                        let mut expected = memory.load_u64(entry).unwrap();
                        loop {
                            match memory.compare_replace_u64(
                                entry,
                                expected,
                                expected | mask,
                                |_| {
                                    notified.fetch_add(1, Ordering::Relaxed);
                                },
                            ) {
                                Ok(Ok(())) => {
                                    replaced.fetch_add(1, Ordering::Relaxed);
                                    break;
                                }
                                Ok(Err(observed)) => expected = observed,
                                Err(error) => panic!("{error:?}"),
                            }
                        }
                    }
                });
            }
        });
        assert_eq!(memory.load_u64(entry), Ok(u64::MAX));
        assert_eq!(replaced.load(Ordering::Relaxed), 64);
        assert_eq!(
            notified.load(Ordering::Relaxed),
            64,
            "one notification per success"
        );
        assert_eq!(memory.code_page_generation(entry), Ok(1 + 64));
        // A counter: every thread adds one, ROUNDS times, by compare-and-replace over what it
        // read; the total is exact and the generation rose once per success.
        const ROUNDS: u64 = 20_000;
        let counter = GuestPhysicalAddress::new(0x1010);
        let start = std::sync::Barrier::new(4);
        std::thread::scope(|scope| {
            for _ in 0..4 {
                let (memory, start) = (&memory, &start);
                scope.spawn(move || {
                    start.wait();
                    for _ in 0..ROUNDS {
                        let mut expected = memory.load_u64(counter).unwrap();
                        while let Ok(Err(observed)) =
                            memory.compare_replace_u64(counter, expected, expected + 1, |_| {})
                        {
                            expected = observed;
                        }
                    }
                });
            }
        });
        assert_eq!(memory.load_u64(counter), Ok(4 * ROUNDS));
        assert_eq!(
            memory.code_page_generation(counter),
            Ok(1 + 64 + 4 * ROUNDS)
        );
        // Misaligned entries are refused, not run as two accesses.
        assert_eq!(
            memory.compare_replace_u64(GuestPhysicalAddress::new(0x1004), 0, 1, |_| {}),
            Err(MemoryAccessError::MisalignedAtomic {
                address: GuestPhysicalAddress::new(0x1004),
                width: 8
            })
        );
    }
    // ---- S6-32: the execution mode ----

    /// Drives every scalar access function over random addresses and widths (aligned, misaligned,
    /// across region edges, into ROM, holes and the framebuffer) and returns what it read and the
    /// committed-write notifications, in order.
    fn drive_scalar_ops(
        memory: &PhysicalMemory,
        seed: u64,
        steps: usize,
    ) -> (Vec<String>, Vec<GuestPhysicalRange>) {
        let mut rng = XorShift(seed);
        let mut seen = Vec::new();
        let mut log = Vec::new();
        for _ in 0..steps {
            let at = random_address(&mut rng);
            let value = rng.next();
            let mut note = |range| log.push(range);
            let result = match rng.next() % 8 {
                0 => format!("{:?}", memory.load_u8(at)),
                1 => format!("{:?}", memory.load_u16(at)),
                2 => format!("{:?}", memory.load_u32(at)),
                3 => format!("{:?}", memory.load_u64(at)),
                4 => format!("{:?}", memory.store_u8(at, value as u8, &mut note)),
                5 => format!("{:?}", memory.store_u16(at, value as u16, &mut note)),
                6 => format!("{:?}", memory.store_u32(at, value as u32, &mut note)),
                _ => format!("{:?}", memory.store_u64(at, value, &mut note)),
            };
            seen.push(result);
        }
        (seen, log)
    }

    /// The generations of every page of the probed window.
    fn generations(memory: &PhysicalMemory) -> Vec<String> {
        (0..=0x7u64)
            .map(|page| {
                format!(
                    "{:?}",
                    memory.code_page_generation(GuestPhysicalAddress::new(page * 0x1000))
                )
            })
            .collect()
    }

    #[test]
    fn the_default_mode_is_interleaved_and_the_switch_reads_back() {
        let mut memory = storage_test_memory();
        assert_eq!(memory.execution_mode(), ExecutionMode::Interleaved);
        assert_eq!(ExecutionMode::default(), ExecutionMode::Interleaved);
        memory.set_execution_mode(ExecutionMode::Parallel);
        assert_eq!(memory.execution_mode(), ExecutionMode::Parallel);
        memory.set_execution_mode(ExecutionMode::Interleaved);
        assert_eq!(memory.execution_mode(), ExecutionMode::Interleaved);
    }

    #[test]
    fn ordinary_scalar_accesses_use_the_order_of_the_execution_mode() {
        use crate::ram_storage::traced_orders;
        for (mode, expected) in [
            (ExecutionMode::Interleaved, Ordering::Relaxed),
            (ExecutionMode::Parallel, Ordering::SeqCst),
        ] {
            let mut memory = storage_test_memory();
            memory.set_execution_mode(mode);
            let (_, orders) = traced_orders(|| {
                // Aligned and misaligned (byte path) of every width, inside RAM.
                for width in 0..4u32 {
                    for at in [0x2000u64, 0x2001, 0x2002, 0x2003, 0x2007] {
                        let at = GuestPhysicalAddress::new(at);
                        match width {
                            0 => {
                                memory.store_u8(at, 1, |_| {}).unwrap();
                                memory.load_u8(at).unwrap();
                            }
                            1 => {
                                memory.store_u16(at, 2, |_| {}).unwrap();
                                memory.load_u16(at).unwrap();
                            }
                            2 => {
                                memory.store_u32(at, 3, |_| {}).unwrap();
                                memory.load_u32(at).unwrap();
                            }
                            _ => {
                                memory.store_u64(at, 4, |_| {}).unwrap();
                                memory.load_u64(at).unwrap();
                            }
                        }
                    }
                }
            });
            // Bulk copies (the operand pieces of the page walk, DMA, the loader) follow the mode
            // too: every host access of a copy has the ordinary order (S6-32).
            let (_, bulk) = traced_orders(|| {
                for (at, len) in [
                    (0x2000u64, 8usize),
                    (0x2003, 8),
                    (0x2001, 5),
                    (0x2000, 13),
                    (0x2005, 1),
                ] {
                    let at = GuestPhysicalAddress::new(at);
                    memory.store_bytes(at, &vec![9u8; len], |_| {}).unwrap();
                    memory.load_bytes(at, &mut vec![0u8; len]).unwrap();
                }
            });
            assert!(bulk.len() >= 10, "{mode:?}: {} bulk accesses", bulk.len());
            assert!(
                bulk.iter().all(|order| *order == expected),
                "{mode:?}: every access of a bulk copy is {expected:?}"
            );
            assert!(orders.len() >= 40, "{mode:?}: {} accesses", orders.len());
            assert!(
                orders.iter().all(|order| *order == expected),
                "{mode:?}: every ordinary access is {expected:?}"
            );
        }
    }

    #[test]
    fn a_single_threaded_run_is_identical_in_both_modes() {
        for seed in [0x5eed_0001u64, 0x5eed_0002, 0x5eed_0003] {
            let mut interleaved = storage_test_memory();
            let mut parallel = storage_test_memory();
            interleaved.set_execution_mode(ExecutionMode::Interleaved);
            parallel.set_execution_mode(ExecutionMode::Parallel);
            let (results_i, log_i) = drive_scalar_ops(&interleaved, seed, 20_000);
            let (results_p, log_p) = drive_scalar_ops(&parallel, seed, 20_000);
            assert_eq!(results_i, results_p, "seed {seed:#x}: every result");
            assert_eq!(log_i, log_p, "seed {seed:#x}: the notification log");
            assert!(!log_i.is_empty(), "seed {seed:#x}: the log is exercised");
            assert_eq!(dump(&interleaved), dump(&parallel), "seed {seed:#x}: dump");
            // The generations (Release in both) of every page of the window and the locked
            // operations are mode independent.
            assert_eq!(
                generations(&interleaved),
                generations(&parallel),
                "seed {seed:#x}: generations"
            );
            let cell = GuestPhysicalAddress::new(0x2008);
            for memory in [&interleaved, &parallel] {
                memory.store_u64(cell, 5, |_| {}).unwrap();
            }
            assert_eq!(
                interleaved.atomic_fetch_add(cell, AccessWidth::Qword, 7, |_| {}),
                parallel.atomic_fetch_add(cell, AccessWidth::Qword, 7, |_| {})
            );
        }
    }

    #[test]
    fn a_fence_places_a_host_fence_only_in_the_parallel_mode_and_changes_no_byte() {
        use od_isa::FenceKind;
        for (mode, issued) in [
            (ExecutionMode::Interleaved, 0u64),
            (ExecutionMode::Parallel, 6),
        ] {
            let mut memory = storage_test_memory();
            memory.set_execution_mode(mode);
            drive_scalar_ops(&memory, 0x5eed_0004, 500);
            let before = dump(&memory);
            let generation = memory.code_page_generation(GuestPhysicalAddress::new(0x2000));
            assert_eq!(memory.host_fences_issued(), 0);
            for kind in [FenceKind::Full, FenceKind::Store, FenceKind::Load] {
                memory.fence(kind);
                memory.fence(kind);
            }
            assert_eq!(memory.host_fences_issued(), issued, "{mode:?}");
            assert_eq!(dump(&memory), before, "{mode:?}: no byte changed");
            assert_eq!(
                memory.code_page_generation(GuestPhysicalAddress::new(0x2000)),
                generation,
                "{mode:?}: no generation changed"
            );
        }
    }
}
