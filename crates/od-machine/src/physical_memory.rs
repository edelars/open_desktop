use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::ram_storage::RamStorage;

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
    /// A map may contain at most one `Framebuffer` region.
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

/// Handle to one plain-RAM 4 KiB frame of a [`PhysicalMemory`] (see `PhysicalMemory::ram_frame`),
/// or, with `large` set, to one 2 MiB-aligned plain-RAM span (`PhysicalMemory::ram_leaf_2m`, S4-13d).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RamFrame {
    region: usize,
    offset: usize,
    large: bool,
}

impl RamFrame {
    /// A 2 MiB span from `PhysicalMemory::ram_leaf_2m` (not a 4 KiB frame).
    pub(crate) fn is_large(self) -> bool {
        self.large
    }
}

/// The sole owner of production RAM backing in this crate.
pub struct PhysicalMemory {
    regions: Vec<Region>,
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
        Ok(Self { regions })
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

    pub fn read_bytes(
        &self,
        address: GuestPhysicalAddress,
        output: &mut [u8],
    ) -> Result<(), MemoryAccessError> {
        let len = checked_access_len(output.len())?;
        let (region, offset) = self.locate(address, len)?;
        let backing = region
            .backing
            .as_ref()
            .ok_or(MemoryAccessError::UnreadableRegion(region.spec.kind))?;
        backing.copy_out(offset, output);
        Ok(())
    }

    /// All scalar and loader writes go through this method. For RAM the
    /// callback runs exactly once, after the complete write has committed.
    /// Writes to a `Framebuffer` region commit without invoking the callback.
    pub fn write_bytes(
        &mut self,
        address: GuestPhysicalAddress,
        input: &[u8],
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> Result<(), MemoryAccessError> {
        let len = checked_access_len(input.len())?;
        self.commit_write(
            address,
            len,
            |storage, offset| storage.copy_in(offset, input),
            committed_write,
        )
    }

    /// The one commit transaction under every RAM write, old (`write_bytes`) and new (`store_*`):
    /// locate and validate (errors and their order are those of `write_bytes`), check that no
    /// page generation would overflow, run `write` on the storage, raise the generation of every
    /// touched page (S6-30b: one `fetch_add(1, Release)` per page, after the data), and only then
    /// call `committed_write` once with the written range. A rejected write changes nothing and
    /// notifies nothing; a `Framebuffer` write has no generations and no callback.
    ///
    /// The overflow check and the raise are separate atomic steps: concurrent writers to one page
    /// cannot make them inexact in practice (a `u64` counter would need 2^64 writes to that one
    /// page), and the serial paths cannot interleave.
    fn commit_write(
        &self,
        address: GuestPhysicalAddress,
        len: u64,
        write: impl FnOnce(&RamStorage, usize),
        mut committed_write: impl FnMut(GuestPhysicalRange),
    ) -> Result<(), MemoryAccessError> {
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
            write(backing, offset);
            return Ok(());
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
        write(backing, offset);
        // Publish: a reader that `Acquire`-loads the raised value sees the data written above.
        for generation in generations {
            let previous = generation.fetch_add(1, Ordering::Release);
            debug_assert_ne!(previous, u64::MAX, "page generation wrapped to 0");
        }
        let committed = GuestPhysicalRange {
            start: address,
            len,
        };
        committed_write(committed);
        Ok(())
    }

    /// Machine-side one-time fill of a ROM region (the firmware image at power
    /// on). This is the only ROM writer; `write_bytes` keeps rejecting ROM.
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
        backing.copy_in(offset, input);
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
        self.read_bytes(address, output)
    }

    pub fn read_u8(&self, address: GuestPhysicalAddress) -> Result<u8, MemoryAccessError> {
        let mut bytes = [0; 1];
        self.read_bytes(address, &mut bytes)?;
        Ok(bytes[0])
    }

    pub fn read_u16(&self, address: GuestPhysicalAddress) -> Result<u16, MemoryAccessError> {
        let mut bytes = [0; 2];
        self.read_bytes(address, &mut bytes)?;
        Ok(u16::from_le_bytes(bytes))
    }

    pub fn read_u32(&self, address: GuestPhysicalAddress) -> Result<u32, MemoryAccessError> {
        let mut bytes = [0; 4];
        self.read_bytes(address, &mut bytes)?;
        Ok(u32::from_le_bytes(bytes))
    }

    pub fn read_u64(&self, address: GuestPhysicalAddress) -> Result<u64, MemoryAccessError> {
        let mut bytes = [0; 8];
        self.read_bytes(address, &mut bytes)?;
        Ok(u64::from_le_bytes(bytes))
    }

    pub fn write_u8(
        &mut self,
        address: GuestPhysicalAddress,
        value: u8,
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> Result<(), MemoryAccessError> {
        self.write_bytes(address, &[value], committed_write)
    }

    pub fn write_u16(
        &mut self,
        address: GuestPhysicalAddress,
        value: u16,
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> Result<(), MemoryAccessError> {
        self.write_bytes(address, &value.to_le_bytes(), committed_write)
    }

    pub fn write_u32(
        &mut self,
        address: GuestPhysicalAddress,
        value: u32,
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> Result<(), MemoryAccessError> {
        self.write_bytes(address, &value.to_le_bytes(), committed_write)
    }

    pub fn write_u64(
        &mut self,
        address: GuestPhysicalAddress,
        value: u64,
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> Result<(), MemoryAccessError> {
        self.write_bytes(address, &value.to_le_bytes(), committed_write)
    }

    /// S6-25: compare-and-replace of one 8-byte value, the single backend
    /// operation under every page-table accessed/dirty update (and the place S6-31 replaces
    /// with a host atomic compare-exchange). It reads the 8 bytes; when they equal `expected`
    /// it writes `replacement` through [`Self::write_u64`] (the callback runs once, after the
    /// commit) and returns `Ok(Ok(()))`; otherwise it writes nothing, runs no callback and
    /// returns `Ok(Err(observed))`. Region errors are those of the read and of the write, and
    /// memory is unchanged on every error. Interleaving mode runs one whole instruction at a
    /// time, so the read and the write cannot be separated by another vCPU; a backend shared
    /// between host threads must make them one operation. Precondition: `address` is aligned
    /// to 8 for the future host compare-exchange (S6-31); callers pass aligned addresses (a 4K
    /// table base plus index * 8), checked by a debug assertion.
    pub fn compare_replace_u64(
        &mut self,
        address: GuestPhysicalAddress,
        expected: u64,
        replacement: u64,
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> Result<Result<(), u64>, MemoryAccessError> {
        debug_assert!(
            address.0 % 8 == 0,
            "compare_replace_u64 needs an 8-aligned address"
        );
        let observed = self.read_u64(address)?;
        if observed != expected {
            return Ok(Err(observed));
        }
        self.write_u64(address, replacement, committed_write)?;
        Ok(Ok(()))
    }

    /// S6-30a: the region storage a read of `len` bytes at `address` goes to, with the offset in
    /// it. Same checks and errors, in the same order, as [`Self::read_bytes`].
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
        region.backing.as_ref()?.try_load_u8(at)
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

/// S6-30a: shared-reference access functions next to the old `&self`/`&mut self` API.
///
/// Loads and stores of 1, 2, 4 and 8 bytes at a naturally aligned address are one host atomic
/// access of that width (`SeqCst`, the choice of S6-06; the order itself is revisited in S6-32);
/// a misaligned access is split into ascending one-byte accesses (MEMORY_MODEL.md, "Атомарность
/// и разделённые доступы"). `load_bytes`/`store_bytes` are bulk copies: not atomic as a whole,
/// not ordered, and another thread can see any prefix of one (relaxed accesses of up to 8
/// bytes, ascending; see `ram_storage`). The locating rules, region kinds and errors are those
/// of `read_bytes`/`write_bytes`; the results are bit-identical to them.
///
/// S6-30b: the stores take the same `committed_write` callback as `write_*` and go through the
/// same commit transaction (`commit_write`): same validation and errors in the same order, the
/// data is stored first, then every touched page generation is raised atomically (`Release`),
/// then the callback runs once; ROM and a rejected write change and notify nothing, a
/// `Framebuffer` write has no generation and no callback. Nothing in production calls these
/// functions until S6-30c switches the adapters.
#[allow(dead_code)]
impl PhysicalMemory {
    pub(crate) fn load_bytes(
        &self,
        address: GuestPhysicalAddress,
        output: &mut [u8],
    ) -> Result<(), MemoryAccessError> {
        let (storage, offset) = self.readable_storage(address, output.len())?;
        storage.copy_out(offset, output);
        Ok(())
    }

    pub(crate) fn store_bytes(
        &self,
        address: GuestPhysicalAddress,
        input: &[u8],
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> Result<(), MemoryAccessError> {
        let len = checked_access_len(input.len())?;
        self.commit_write(
            address,
            len,
            |storage, offset| storage.copy_in(offset, input),
            committed_write,
        )
    }

    pub(crate) fn load_u8(&self, address: GuestPhysicalAddress) -> Result<u8, MemoryAccessError> {
        let (storage, offset) = self.readable_storage(address, 1)?;
        Ok(storage.load_u8(offset, Ordering::SeqCst))
    }

    pub(crate) fn load_u16(&self, address: GuestPhysicalAddress) -> Result<u16, MemoryAccessError> {
        let (storage, offset) = self.readable_storage(address, 2)?;
        Ok(storage.load_u16(offset, Ordering::SeqCst))
    }

    pub(crate) fn load_u32(&self, address: GuestPhysicalAddress) -> Result<u32, MemoryAccessError> {
        let (storage, offset) = self.readable_storage(address, 4)?;
        Ok(storage.load_u32(offset, Ordering::SeqCst))
    }

    pub(crate) fn load_u64(&self, address: GuestPhysicalAddress) -> Result<u64, MemoryAccessError> {
        let (storage, offset) = self.readable_storage(address, 8)?;
        Ok(storage.load_u64(offset, Ordering::SeqCst))
    }

    pub(crate) fn store_u8(
        &self,
        address: GuestPhysicalAddress,
        value: u8,
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> Result<(), MemoryAccessError> {
        self.commit_write(
            address,
            1,
            |storage, offset| storage.store_u8(offset, value, Ordering::SeqCst),
            committed_write,
        )
    }

    pub(crate) fn store_u16(
        &self,
        address: GuestPhysicalAddress,
        value: u16,
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> Result<(), MemoryAccessError> {
        self.commit_write(
            address,
            2,
            |storage, offset| storage.store_u16(offset, value, Ordering::SeqCst),
            committed_write,
        )
    }

    pub(crate) fn store_u32(
        &self,
        address: GuestPhysicalAddress,
        value: u32,
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> Result<(), MemoryAccessError> {
        self.commit_write(
            address,
            4,
            |storage, offset| storage.store_u32(offset, value, Ordering::SeqCst),
            committed_write,
        )
    }

    pub(crate) fn store_u64(
        &self,
        address: GuestPhysicalAddress,
        value: u64,
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> Result<(), MemoryAccessError> {
        self.commit_write(
            address,
            8,
            |storage, offset| storage.store_u64(offset, value, Ordering::SeqCst),
            committed_write,
        )
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
        let mut top_byte = PhysicalMemory::new(&top_byte_map, RamLimits::new(1)).unwrap();
        top_byte
            .write_u8(GuestPhysicalAddress::new(u64::MAX), 0x5a, |_| {})
            .unwrap();
        assert_eq!(
            top_byte.read_u8(GuestPhysicalAddress::new(u64::MAX)),
            Ok(0x5a)
        );
    }

    /// S6-25: compare-and-replace writes (and notifies, and bumps the generation) only when the
    /// 8 bytes equal `expected`; otherwise it returns the observed value and changes nothing.
    #[test]
    fn compare_replace_u64_replaces_only_a_matching_value() {
        let mut ram = memory(&[
            (0, 0x2000, MemoryRegionKind::Ram),
            (0x2000, 0x1000, MemoryRegionKind::Rom),
        ]);
        let at = GuestPhysicalAddress::new(0x100);
        ram.write_u64(at, 7, |_| {}).unwrap();
        let generation = ram.code_page_generation(at).unwrap();
        let mut log = Vec::new();
        assert_eq!(
            ram.compare_replace_u64(at, 8, 9, |r| log.push(r)),
            Ok(Err(7))
        );
        assert!(log.is_empty());
        assert_eq!(ram.read_u64(at), Ok(7));
        assert_eq!(ram.code_page_generation(at), Ok(generation));
        assert_eq!(
            ram.compare_replace_u64(at, 7, 9, |r| log.push(r)),
            Ok(Ok(()))
        );
        assert_eq!(log, [range(0x100, 8)]);
        assert_eq!(ram.read_u64(at), Ok(9));
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
        let mut memory = memory(&[
            (0x1000, 0x1000, MemoryRegionKind::Ram),
            (0x3000, 0x1000, MemoryRegionKind::Rom),
            (0x5000, 0x1000, MemoryRegionKind::Reserved),
        ]);
        memory
            .write_u32(GuestPhysicalAddress::new(0x1ffc), 0x1234_5678, |_| {})
            .unwrap();
        assert_eq!(
            memory.read_u32(GuestPhysicalAddress::new(0x1ffc)).unwrap(),
            0x1234_5678
        );
        assert!(matches!(
            memory.read_u8(GuestPhysicalAddress::new(0x2000)),
            Err(MemoryAccessError::Unmapped(_))
        ));
        // S7-03: ROM has read backing (zeroed until the loader fills it).
        assert_eq!(
            memory.read_u8(GuestPhysicalAddress::new(0x3000)).unwrap(),
            0
        );
        assert!(matches!(
            memory.write_u8(GuestPhysicalAddress::new(0x3000), 1, |_| {}),
            Err(MemoryAccessError::ReadOnlyRegion(MemoryRegionKind::Rom))
        ));
        assert!(matches!(
            memory.write_u8(GuestPhysicalAddress::new(0x5000), 1, |_| {}),
            Err(MemoryAccessError::ReadOnlyRegion(
                MemoryRegionKind::Reserved
            ))
        ));
        assert!(matches!(
            memory.read_u16(GuestPhysicalAddress::new(u64::MAX)),
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
            memory.read_u32(GuestPhysicalAddress::new(0x3010)).unwrap(),
            0x0403_0201
        );
        // Guest-visible writes never reach ROM and never invoke the callback.
        let mut commits = 0;
        assert_eq!(
            memory.write_u8(GuestPhysicalAddress::new(0x3010), 9, |_| commits += 1),
            Err(MemoryAccessError::ReadOnlyRegion(MemoryRegionKind::Rom))
        );
        assert_eq!(commits, 0);
        assert_eq!(
            memory.read_u8(GuestPhysicalAddress::new(0x3010)).unwrap(),
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
        let mut memory = memory(&[(0x1000, 0x1000, MemoryRegionKind::Ram)]);
        let mut commits = Vec::new();
        memory
            .write_u16(GuestPhysicalAddress::new(0x1002), 0xa1b2, |range| {
                commits.push(range)
            })
            .unwrap();
        assert_eq!(commits, vec![range(0x1002, 2)]);
        assert!(
            memory
                .write_u8(GuestPhysicalAddress::new(0x3000), 1, |range| commits
                    .push(range))
                .is_err()
        );
        assert_eq!(commits, vec![range(0x1002, 2)]);
        assert!(
            memory
                .read_bytes(GuestPhysicalAddress::new(0x1002), &mut [])
                .is_err()
        );
    }

    #[test]
    fn committed_writes_advance_only_the_physical_pages_they_touch() {
        let mut memory = memory(&[(0x1000, 0x3000, MemoryRegionKind::Ram)]);
        let first = GuestPhysicalAddress::new(0x1000);
        let second = GuestPhysicalAddress::new(0x2000);
        let third = GuestPhysicalAddress::new(0x3000);
        assert_eq!(memory.code_page_generation(first), Ok(1));
        assert_eq!(memory.code_page_generation(second), Ok(1));
        assert_eq!(memory.code_page_generation(third), Ok(1));

        memory
            .write_u8(GuestPhysicalAddress::new(0x1010), 0x90, |_| {})
            .unwrap();
        assert_eq!(memory.code_page_generation(first), Ok(2));
        assert_eq!(memory.code_page_generation(second), Ok(1));
        assert_eq!(memory.code_page_generation(third), Ok(1));

        assert!(
            memory
                .write_bytes(GuestPhysicalAddress::new(0x3fff), &[1, 2], |_| {})
                .is_err()
        );
        assert_eq!(memory.code_page_generation(third), Ok(1));

        memory
            .write_bytes(GuestPhysicalAddress::new(0x1fff), &[1, 2], |_| {})
            .unwrap();
        assert_eq!(memory.code_page_generation(first), Ok(3));
        assert_eq!(memory.code_page_generation(second), Ok(2));
        assert_eq!(memory.code_page_generation(third), Ok(1));
    }

    #[test]
    fn write_crossing_adjacent_ram_regions_is_atomic_and_unnotified() {
        let mut memory = memory(&[
            (0x1000, 2, MemoryRegionKind::Ram),
            (0x1002, 2, MemoryRegionKind::Ram),
        ]);
        let mut commits = Vec::new();
        assert!(matches!(
            memory.write_bytes(GuestPhysicalAddress::new(0x1000), &[1, 2, 3, 4], |range| {
                commits.push(range)
            }),
            Err(MemoryAccessError::AccessCrossesRegion(_))
        ));
        assert!(commits.is_empty());
        let mut first = [0xff; 2];
        let mut second = [0xff; 2];
        memory
            .read_bytes(GuestPhysicalAddress::new(0x1000), &mut first)
            .unwrap();
        memory
            .read_bytes(GuestPhysicalAddress::new(0x1002), &mut second)
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
        let mut memory = PhysicalMemory::new(&map, RamLimits::new(0x1000)).unwrap();
        let mut commits = Vec::new();
        memory
            .write_bytes(
                GuestPhysicalAddress::new(FB_START + 0x3ffc),
                &[1, 2, 3, 4],
                |r| commits.push(r),
            )
            .unwrap();
        assert!(commits.is_empty());
        assert_eq!(
            memory.read_u32(GuestPhysicalAddress::new(FB_START + 0x3ffc)),
            Ok(0x0403_0201)
        );
        // Zero-filled at power-on.
        assert_eq!(memory.read_u64(GuestPhysicalAddress::new(FB_START)), Ok(0));
        // RAM in the same memory still notifies exactly once.
        memory
            .write_u8(GuestPhysicalAddress::new(0x1000), 7, |r| commits.push(r))
            .unwrap();
        assert_eq!(commits, vec![range(0x1000, 1)]);
        // Crossing the end of the region is rejected without a partial write.
        assert!(matches!(
            memory.write_bytes(
                GuestPhysicalAddress::new(FB_START + 0x3ffe),
                &[9; 4],
                |_| {}
            ),
            Err(MemoryAccessError::AccessCrossesRegion(_))
        ));
        assert_eq!(
            memory.read_u16(GuestPhysicalAddress::new(FB_START + 0x3ffe)),
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
        let mut memory = PhysicalMemory::new(&map, RamLimits::new(0x2000)).unwrap();
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
                .write_bytes(GuestPhysicalAddress::new(FB_START + offset), bytes, |r| {
                    commits.push(r)
                })
                .unwrap();
        }
        assert!(commits.is_empty());
        assert_eq!(
            memory.read_u32(GuestPhysicalAddress::new(FB_START + 0x3ffc)),
            Ok(0x0908_0706)
        );
        assert_eq!(memory.code_page_generation(ram0), Ok(1));
        assert_eq!(memory.code_page_generation(ram1), Ok(1));

        // RAM writes still bump exactly the touched generations and notify once.
        memory
            .write_bytes(GuestPhysicalAddress::new(0x1ffe), &[1, 2, 3, 4], |r| {
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
        let mut memory = memory(&[(0, 0x9000, MemoryRegionKind::Ram)]);
        let bytes: Vec<u8> = (0..0x30).map(|value| value as u8).collect();
        let start = GuestPhysicalAddress::new(0x3ff0);
        let mut commits = Vec::new();
        memory
            .write_bytes(start, &bytes, |range| commits.push(range))
            .unwrap();
        let mut read_back = vec![0; bytes.len()];
        memory.read_bytes(start, &mut read_back).unwrap();
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
            .map(|at| memory.read_u8(GuestPhysicalAddress::new(at)))
            .collect()
    }

    /// The generation (or the error) of every page of the probe window.
    fn generations(memory: &PhysicalMemory) -> Vec<Result<u64, MemoryAccessError>> {
        (PROBE_START >> 12..=PROBE_END >> 12)
            .map(|page| memory.code_page_generation(GuestPhysicalAddress::new(page << 12)))
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

    /// Old API on one memory and the new functions on another give the same results, the same
    /// errors and, in the end, the same bytes, over random operations that hit edges, holes, ROM,
    /// Reserved, Framebuffer, adjacent regions and every width and alignment. A third copy of the
    /// bytes is a plain `Vec<u8>` model, so equality does not rest on the two paths sharing one
    /// storage: each success of the new API is applied to the model, each successful load is
    /// compared with it, and the model equals the final dump.
    #[test]
    fn new_access_functions_match_the_old_api_byte_for_byte() {
        for seed in [
            0x9e37_79b9_7f4a_7c15u64,
            0x1234_5678_9abc_def1,
            0xdead_beef_cafe_f00d,
        ] {
            let mut rng = XorShift(seed);
            let mut old = storage_test_memory();
            let new = storage_test_memory();
            let mut model = ByteModel::new();
            let mut new_reads = 0u32;
            for step in 0..20_000u32 {
                let at = random_address(&mut rng);
                let value = rng.next();
                let mut old_commits = Vec::new();
                let mut new_commits = Vec::new();
                match rng.next() % 12 {
                    0 => {
                        let result = new.store_u8(at, value as u8, |r| new_commits.push(r));
                        assert_eq!(
                            old.write_u8(at, value as u8, |r| old_commits.push(r)),
                            result,
                            "step {step} store_u8 {at:?}"
                        );
                        if result.is_ok() {
                            model.store(at, &(value as u8).to_le_bytes());
                        }
                    }
                    1 => {
                        let result = new.store_u16(at, value as u16, |r| new_commits.push(r));
                        assert_eq!(
                            old.write_u16(at, value as u16, |r| old_commits.push(r)),
                            result,
                            "step {step} store_u16 {at:?}"
                        );
                        if result.is_ok() {
                            model.store(at, &(value as u16).to_le_bytes());
                        }
                    }
                    2 => {
                        let result = new.store_u32(at, value as u32, |r| new_commits.push(r));
                        assert_eq!(
                            old.write_u32(at, value as u32, |r| old_commits.push(r)),
                            result,
                            "step {step} store_u32 {at:?}"
                        );
                        if result.is_ok() {
                            model.store(at, &(value as u32).to_le_bytes());
                        }
                    }
                    3 => {
                        let result = new.store_u64(at, value, |r| new_commits.push(r));
                        assert_eq!(
                            old.write_u64(at, value, |r| old_commits.push(r)),
                            result,
                            "step {step} store_u64 {at:?}"
                        );
                        if result.is_ok() {
                            model.store(at, &value.to_le_bytes());
                        }
                    }
                    4 => {
                        let len = (rng.next() % 26) as usize; // 0 is the empty-range error
                        let data: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
                        let result = new.store_bytes(at, &data, |r| new_commits.push(r));
                        assert_eq!(
                            old.write_bytes(at, &data, |r| old_commits.push(r)),
                            result,
                            "step {step} store_bytes {at:?} len {len}"
                        );
                        if result.is_ok() {
                            model.store(at, &data);
                        }
                    }
                    5 => {
                        let result = new.load_u8(at);
                        assert_eq!(old.read_u8(at), result, "step {step} load_u8 {at:?}");
                        if let Ok(read) = result {
                            assert_eq!(&[read], model.bytes(at, 1), "step {step} model u8");
                        }
                    }
                    6 => {
                        let result = new.load_u16(at);
                        assert_eq!(old.read_u16(at), result, "step {step} load_u16 {at:?}");
                        if let Ok(read) = result {
                            assert_eq!(
                                read.to_le_bytes(),
                                model.bytes(at, 2),
                                "step {step} model u16"
                            );
                        }
                    }
                    7 => {
                        let result = new.load_u32(at);
                        assert_eq!(old.read_u32(at), result, "step {step} load_u32 {at:?}");
                        if let Ok(read) = result {
                            assert_eq!(
                                read.to_le_bytes(),
                                model.bytes(at, 4),
                                "step {step} model u32"
                            );
                        }
                    }
                    8 => {
                        let result = new.load_u64(at);
                        assert_eq!(old.read_u64(at), result, "step {step} load_u64 {at:?}");
                        if let Ok(read) = result {
                            assert_eq!(
                                read.to_le_bytes(),
                                model.bytes(at, 8),
                                "step {step} model u64"
                            );
                        }
                    }
                    _ => {
                        let len = (rng.next() % 26) as usize;
                        let mut from_old = vec![0xeeu8; len];
                        let mut from_new = vec![0xeeu8; len];
                        let result = new.load_bytes(at, &mut from_new);
                        assert_eq!(
                            old.read_bytes(at, &mut from_old),
                            result,
                            "step {step} load_bytes {at:?} len {len}"
                        );
                        assert_eq!(from_old, from_new, "step {step} bytes {at:?} len {len}");
                        if result.is_ok() {
                            assert_eq!(from_new, model.bytes(at, len), "step {step} model bytes");
                        }
                        new_reads += 1;
                    }
                }
                // S6-30b: the same notifications (none on a rejection or a Framebuffer write,
                // otherwise exactly one range of the written bytes) ...
                assert_eq!(old_commits, new_commits, "step {step} notifications");
                assert!(new_commits.len() <= 1, "step {step}");
                // ... and the same generation of every RAM page after every step.
                if step % 64 == 0 {
                    assert_eq!(
                        generations(&old),
                        generations(&new),
                        "step {step} generations"
                    );
                }
            }
            assert!(new_reads > 1000);
            assert_eq!(dump(&old), dump(&new), "seed {seed:#x}");
            // Every readable byte of the final dump equals the model; unreadable ones are errors.
            for (index, byte) in dump(&new).into_iter().enumerate() {
                if let Ok(byte) = byte {
                    assert_eq!(byte, model.0[index], "seed {seed:#x} byte {index:#x}");
                }
            }
            // S6-30b: the stores raise generations like the old path does.
            assert_eq!(generations(&old), generations(&new), "seed {seed:#x}");
            let ram = GuestPhysicalAddress::new(0x2000);
            assert!(new.code_page_generation(ram).unwrap() > 1);
        }
    }

    /// Both orders of mixing: the same random writes through old and new paths into one
    /// memory each, read back through the other API, see the same bytes.
    #[test]
    fn old_and_new_paths_share_one_storage() {
        let mut mixed = storage_test_memory();
        let mut reference = storage_test_memory();
        let mut rng = XorShift(0x0123_4567_89ab_cdef);
        for _ in 0..5_000 {
            let at = random_address(&mut rng);
            let value = rng.next();
            let width = [1usize, 2, 4, 8][(rng.next() % 4) as usize];
            let bytes = &value.to_le_bytes()[..width];
            let expected = reference.write_bytes(at, bytes, |_| {});
            let through_new = rng.next() % 2 == 0;
            let got = if through_new {
                mixed.store_bytes(at, bytes, |_| {})
            } else {
                mixed.write_bytes(at, bytes, |_| {})
            };
            assert_eq!(got, expected);
            let mut a = vec![0u8; width];
            let mut b = vec![0u8; width];
            let ra = if through_new {
                mixed.read_bytes(at, &mut a)
            } else {
                mixed.load_bytes(at, &mut a)
            };
            assert_eq!(ra, reference.read_bytes(at, &mut b));
            assert_eq!(a, b);
        }
        assert_eq!(dump(&mixed), dump(&reference));
    }

    #[test]
    fn new_functions_keep_the_boundary_errors_of_the_old_ones() {
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
            u64::MAX,
            u64::MAX - 3,
        ] {
            let at = GuestPhysicalAddress::new(at);
            let mut one = [0u8; 1];
            let mut eight = [0u8; 8];
            assert_eq!(memory.read_u8(at), memory.load_u8(at));
            assert_eq!(memory.read_u16(at), memory.load_u16(at));
            assert_eq!(memory.read_u32(at), memory.load_u32(at));
            assert_eq!(memory.read_u64(at), memory.load_u64(at));
            assert_eq!(
                memory.read_bytes(at, &mut one),
                memory.load_bytes(at, &mut one)
            );
            assert_eq!(
                memory.read_bytes(at, &mut eight),
                memory.load_bytes(at, &mut eight)
            );
            assert_eq!(
                memory.read_bytes(at, &mut []),
                memory.load_bytes(at, &mut [])
            );
        }
        let mut old = storage_test_memory();
        for at in [
            0x2fff_u64,
            0x3804,
            0x40ff,
            0x4000,
            0x50ff,
            0x6ff9,
            0x7000,
            u64::MAX,
        ] {
            let at = GuestPhysicalAddress::new(at);
            assert_eq!(old.write_u8(at, 1, |_| {}), memory.store_u8(at, 1, |_| {}));
            assert_eq!(
                old.write_u16(at, 1, |_| {}),
                memory.store_u16(at, 1, |_| {})
            );
            assert_eq!(
                old.write_u32(at, 1, |_| {}),
                memory.store_u32(at, 1, |_| {})
            );
            assert_eq!(
                old.write_u64(at, 1, |_| {}),
                memory.store_u64(at, 1, |_| {})
            );
            assert_eq!(
                old.write_bytes(at, &[], |_| {}),
                memory.store_bytes(at, &[], |_| {})
            );
        }
    }

    #[test]
    fn rejected_new_stores_leave_memory_unchanged() {
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
    fn framebuffer_is_writable_by_new_stores_without_generations() {
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

    /// An aligned 1/2/4/8-byte access is a single host access of that width, on the old API
    /// (through `read_bytes`/`write_bytes`) and the new one, also in a region whose start is
    /// not 8-aligned; a misaligned access is split into ascending bytes.
    #[test]
    fn aligned_accesses_are_single_host_accesses_on_both_paths() {
        use crate::ram_storage::traced;
        let mut memory = storage_test_memory();
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
            let ((), old_write) = traced(|| match width {
                1 => memory.write_u8(at, 1, |_| {}).unwrap(),
                2 => memory.write_u16(at, 1, |_| {}).unwrap(),
                4 => memory.write_u32(at, 1, |_| {}).unwrap(),
                _ => memory.write_u64(at, 1, |_| {}).unwrap(),
            });
            let ((), old_read) = traced(|| match width {
                1 => drop(memory.read_u8(at).unwrap()),
                2 => drop(memory.read_u16(at).unwrap()),
                4 => drop(memory.read_u32(at).unwrap()),
                _ => drop(memory.read_u64(at).unwrap()),
            });
            let ((), new_write) = traced(|| match width {
                1 => memory.store_u8(at, 2, |_| {}).unwrap(),
                2 => memory.store_u16(at, 2, |_| {}).unwrap(),
                4 => memory.store_u32(at, 2, |_| {}).unwrap(),
                _ => memory.store_u64(at, 2, |_| {}).unwrap(),
            });
            let ((), new_read) = traced(|| match width {
                1 => drop(memory.load_u8(at).unwrap()),
                2 => drop(memory.load_u16(at).unwrap()),
                4 => drop(memory.load_u32(at).unwrap()),
                _ => drop(memory.load_u64(at).unwrap()),
            });
            assert_eq!(old_write, expect, "old write width {width} at {address:#x}");
            assert_eq!(old_read, expect, "old read width {width} at {address:#x}");
            assert_eq!(new_write, expect, "new store width {width} at {address:#x}");
            assert_eq!(new_read, expect, "new load width {width} at {address:#x}");
        }
        // Misaligned dword: ascending bytes.
        let at = GuestPhysicalAddress::new(0x1011);
        let (_, log) = traced(|| memory.store_u32(at, 9, |_| {}).unwrap());
        assert_eq!(
            log,
            (0..4).map(|i| (0x1011 - 0x1003 + i, 1)).collect::<Vec<_>>()
        );
    }

    /// S6-25: the compare-and-replace stays correct on the new storage and is made of single
    /// aligned 8-byte accesses (a read and, on a match, a write), also in a region with
    /// an odd start.
    #[test]
    fn compare_replace_u64_is_two_single_accesses_on_the_new_storage() {
        use crate::ram_storage::traced;
        let mut memory = storage_test_memory();
        let at = GuestPhysicalAddress::new(0x1008); // aligned guest address, region start 0x1003
        memory.store_u64(at, 5, |_| {}).unwrap();
        let (result, log) = traced(|| memory.compare_replace_u64(at, 5, 6, |_| {}));
        assert_eq!(result, Ok(Ok(())));
        assert_eq!(log, vec![(5, 8), (5, 8)]);
        let (result, log) = traced(|| memory.compare_replace_u64(at, 5, 7, |_| {}));
        assert_eq!(result, Ok(Err(6)));
        assert_eq!(log, vec![(5, 8)]);
        assert_eq!(memory.load_u64(at), Ok(6));
    }

    #[test]
    fn physical_memory_can_be_shared_between_threads_by_reference() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<PhysicalMemory>();
        let memory = storage_test_memory();
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

    /// A page generation at the top of `u64` rejects the write before any byte is stored, on both
    /// paths, and a write that does not touch that page is unaffected.
    #[test]
    fn an_exhausted_generation_rejects_old_and_new_writes_alike() {
        let mut old = storage_test_memory();
        let new = storage_test_memory();
        for memory in [&old, &new] {
            memory.regions[0].page_generations[0].store(u64::MAX, Ordering::Relaxed);
        }
        let at = GuestPhysicalAddress::new(0x1010);
        assert_eq!(
            old.write_u32(at, 1, |_| panic!("no callback")),
            Err(MemoryAccessError::CodeGenerationExhausted)
        );
        assert_eq!(
            new.store_u32(at, 1, |_| panic!("no callback")),
            Err(MemoryAccessError::CodeGenerationExhausted)
        );
        assert_eq!(new.load_u32(at), Ok(0));
        assert_eq!(old.read_u32(at), Ok(0));
        assert_eq!(
            new.store_u32(GuestPhysicalAddress::new(0x2010), 1, |_| {}),
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
        let memory = memory(&[(0x1000, 0x5000, MemoryRegionKind::Ram)]);
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
}
