use std::fmt;

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
        let mut framebuffers = regions
            .iter()
            .filter(|region| region.kind == MemoryRegionKind::Framebuffer);
        if let Some(first) = framebuffers.next() {
            if framebuffers.next().is_some() {
                return Err(MemoryAccessError::MultipleFramebufferRegions);
            }
            if first.range.start.0 % FRAMEBUFFER_ALIGNMENT != 0
                || first.range.len % FRAMEBUFFER_ALIGNMENT != 0
                || first.range.len > MAX_FRAMEBUFFER_BYTES
            {
                return Err(MemoryAccessError::InvalidFramebufferRegion(first.range));
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
    backing: Option<Vec<u8>>,
    page_generations: Vec<u64>,
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
            // Page generations exist only for RAM: Framebuffer is never code.
            let (backing, page_generations) = if matches!(
                spec.kind,
                MemoryRegionKind::Ram | MemoryRegionKind::Framebuffer
            ) {
                let len = usize::try_from(spec.range.len).map_err(|_| {
                    MemoryAccessError::AllocationFailed {
                        bytes: spec.range.len,
                    }
                })?;
                let mut bytes = Vec::new();
                bytes
                    .try_reserve_exact(len)
                    .map_err(|_| MemoryAccessError::AllocationFailed {
                        bytes: spec.range.len,
                    })?;
                bytes.resize(len, 0);
                if spec.kind == MemoryRegionKind::Framebuffer {
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
                    generations.resize(page_count, 1);
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
            .copied()
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
        output.copy_from_slice(&backing[offset..offset + output.len()]);
        Ok(())
    }

    /// All scalar and loader writes go through this method. For RAM the
    /// callback runs exactly once, after the complete write has committed.
    /// Writes to a `Framebuffer` region commit without invoking the callback.
    pub fn write_bytes(
        &mut self,
        address: GuestPhysicalAddress,
        input: &[u8],
        mut committed_write: impl FnMut(GuestPhysicalRange),
    ) -> Result<(), MemoryAccessError> {
        let len = checked_access_len(input.len())?;
        let (region_index, offset) = self.locate_index(address, len)?;
        let region = &mut self.regions[region_index];
        if region.backing.is_none() {
            return Err(MemoryAccessError::ReadOnlyRegion(region.spec.kind));
        }
        if region.spec.kind == MemoryRegionKind::Framebuffer {
            // Device-owned pixel memory: no page generations, no callback.
            let backing = region
                .backing
                .as_mut()
                .ok_or(MemoryAccessError::ReadOnlyRegion(region.spec.kind))?;
            backing[offset..offset + input.len()].copy_from_slice(input);
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
            .get_mut(first_generation..end_generation)
            .ok_or(MemoryAccessError::AddressOverflow)?;
        if generations
            .iter()
            .any(|generation| generation.checked_add(1).is_none())
        {
            return Err(MemoryAccessError::CodeGenerationExhausted);
        }
        let backing = region
            .backing
            .as_mut()
            .ok_or(MemoryAccessError::ReadOnlyRegion(region.spec.kind))?;
        backing[offset..offset + input.len()].copy_from_slice(input);
        for generation in generations {
            *generation += 1;
        }
        let committed = GuestPhysicalRange {
            start: address,
            len,
        };
        committed_write(committed);
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

    fn locate(
        &self,
        address: GuestPhysicalAddress,
        len: u64,
    ) -> Result<(&Region, usize), MemoryAccessError> {
        let (index, offset) = self.locate_index(address, len)?;
        Ok((&self.regions[index], offset))
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
        assert!(matches!(
            memory.read_u8(GuestPhysicalAddress::new(0x3000)),
            Err(MemoryAccessError::UnreadableRegion(MemoryRegionKind::Rom))
        ));
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
}
