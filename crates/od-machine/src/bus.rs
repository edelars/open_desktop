use crate::lock_wait_metrics::MeasuredMutex;
use crate::{
    GuestLinearAddress, GuestPhysicalAddress, GuestPhysicalRange, Long64ProbeConfig,
    MachineMemoryMap, MemoryAccessError, MemoryRegionKind, MemoryRegionSpec, PaeAccessRequest,
    PaeProbeConfig, PhysicalMemory, ProbeReport, RamAccessOperation, RamLimits, TrackedAccessError,
    TrackedAccessOutcome, TrackedAccessReport, TrackedAccessRequest,
};
use od_isa::{
    PortIo,
    memory::{AccessContext, AccessKind, AccessOrigin},
};
use std::cell::{Cell, RefCell};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, MutexGuard, PoisonError};

// Must match the bounded 36-bit physical-address format accepted by
// page_walk.rs. Higher PTE bits can contain software metadata or NX.
const TRACKED_CODE_FRAME_MASK: u64 = ((1u64 << 36) - 1) & !0xfff;

mod device_error_log;

pub(crate) use device_error_log::DeviceErrorContext;
pub use device_error_log::{
    DEVICE_ERROR_LOG_GROUP_LIMIT, DeviceErrorDirection, DeviceErrorGroup, DeviceErrorKey,
    DeviceErrorKind, DeviceErrorLog, DeviceErrorLogReport, DeviceErrorLogStop,
};
use device_error_log::{DeviceErrorAccess, absorbable};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PortAddress(u16);

impl PortAddress {
    pub const fn new(value: u16) -> Self {
        Self(value)
    }
    pub const fn get(self) -> u16 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PortRange {
    start: PortAddress,
    len: u32,
}

impl PortRange {
    pub fn new(start: PortAddress, len: u32) -> Result<Self, BusBuildError> {
        let end = u64::from(start.0) + u64::from(len);
        if len == 0 || end > 65_536 {
            return Err(BusBuildError::InvalidPortRange);
        }
        Ok(Self { start, len })
    }
    pub const fn start(self) -> PortAddress {
        self.start
    }
    pub const fn len(self) -> u32 {
        self.len
    }
    fn end(self) -> u32 {
        u32::from(self.start.0) + self.len
    }
    fn contains(self, start: PortAddress, len: u32) -> bool {
        let end = u32::from(start.0) + len;
        u32::from(start.0) >= u32::from(self.start.0) && end <= self.end()
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AccessWidth {
    Byte,
    Word,
    Dword,
    Qword,
}

impl AccessWidth {
    pub const fn bytes(self) -> u8 {
        match self {
            Self::Byte => 1,
            Self::Word => 2,
            Self::Dword => 4,
            Self::Qword => 8,
        }
    }
    const fn mask(self) -> u64 {
        match self {
            Self::Byte => 0xff,
            Self::Word => 0xffff,
            Self::Dword => 0xffff_ffff,
            Self::Qword => u64::MAX,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum IoSpace {
    Mmio,
    Pio,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeviceAccess {
    pub space: IoSpace,
    pub offset: u64,
    pub width: AccessWidth,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum DeviceError {
    UnknownOffset,
    UnsupportedWidth,
    UnsupportedSpace,
    UnsupportedConfiguration,
    Busy,
    Misaligned,
    ReadOnly,
    WriteOnly,
    CrossesRegister,
    ClockOverflow,
}

impl fmt::Display for DeviceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "device access error: {self:?}")
    }
}
impl std::error::Error for DeviceError {}

/// A device on the bus. `Send` (S6-33): the device table sits behind one lock and is used from the
/// thread of the vCPU that holds it (MACHINE_CONTRACT.md, lock order); a device does not call back
/// into the bus.
pub trait BusDevice: Send {
    /// An error must be returned before any guest-visible state or external
    /// effect is committed. Implementations validate the complete operation
    /// before applying its synchronous effect.
    fn read(&mut self, access: DeviceAccess) -> Result<u64, DeviceError>;
    /// An error must be returned before any guest-visible state or external
    /// effect is committed. Implementations validate the complete operation
    /// before applying its synchronous effect.
    fn write(&mut self, access: DeviceAccess, value: u64) -> Result<(), DeviceError>;

    /// Sample the currently executing instruction's board time on demand.
    /// Devices with no time-dependent I/O use their ordinary access method.
    fn read_at_time(
        &mut self,
        access: DeviceAccess,
        _sample_time: &dyn Fn() -> Result<u64, DeviceError>,
    ) -> Result<u64, DeviceError> {
        self.read(access)
    }

    /// The timed write counterpart of `read_at_time`.
    fn write_at_time(
        &mut self,
        access: DeviceAccess,
        value: u64,
        _sample_time: &dyn Fn() -> Result<u64, DeviceError>,
    ) -> Result<(), DeviceError> {
        self.write(access, value)
    }

    /// The access of vCPU `cpu`. Only a device with one register window per processor (the
    /// Local APIC, S6-11) tells the processors apart; every other device serves all of them with
    /// `read`.
    fn read_from_cpu(&mut self, _cpu: usize, access: DeviceAccess) -> Result<u64, DeviceError> {
        self.read(access)
    }
    /// The access of vCPU `cpu`; see [`BusDevice::read_from_cpu`].
    fn write_from_cpu(
        &mut self,
        _cpu: usize,
        access: DeviceAccess,
        value: u64,
    ) -> Result<(), DeviceError> {
        self.write(access, value)
    }

    /// A display device reports the framebuffer window its registers
    /// advertise. `MachineBus::new` checks it against the memory map; every
    /// other device keeps the default `None`.
    fn framebuffer_binding(&self) -> Option<FramebufferBinding> {
        None
    }

    /// Framebuffer window of the second screen (DISPLAY_DEVICE.md 16.3). Only a
    /// display device in the two-screen configuration reports one.
    fn secondary_framebuffer_binding(&self) -> Option<FramebufferBinding> {
        None
    }
}

/// Framebuffer GPA window advertised by a display device (`FB_BASE`/`FB_SIZE`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FramebufferBinding {
    pub base: GuestPhysicalAddress,
    pub size: u64,
}

/// Length of the register window an MMIO-bound display device requires.
pub(crate) const DISPLAY_REGISTER_WINDOW_BYTES: u64 = 0x1000;

/// Why a display device and the `Framebuffer` region of the map disagree.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisplayBindingError {
    /// The map has a `Framebuffer` region but no device advertises one.
    MissingDevice,
    /// A `Framebuffer` region of the map is not advertised by the device
    /// (its base and length match no `FB_BASE`/`FB_SIZE` of any screen).
    UnboundFramebufferRegion {
        region_start: GuestPhysicalAddress,
        region_len: u64,
    },
    /// A display device exists but the map has no `Framebuffer` region.
    MissingFramebufferRegion,
    /// Version 1 has exactly one screen.
    MultipleDevices,
    /// The region start or length differs from the device's `FB_BASE`/`FB_SIZE`.
    FramebufferMismatch {
        region_start: GuestPhysicalAddress,
        region_len: u64,
        device_base: GuestPhysicalAddress,
        device_size: u64,
    },
    /// The display device has no MMIO binding.
    RegisterWindowUnbound,
    /// The display device is bound to more than one MMIO region.
    MultipleRegisterWindows,
    /// The MMIO region bound to the display device is not 0x1000 bytes long.
    RegisterWindowLength { actual: u64 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MmioBinding {
    pub region_start: GuestPhysicalAddress,
    pub device_index: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PortRegionBinding {
    pub range: PortRange,
    pub device_index: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BusBuildError {
    InvalidPortRange,
    OverlappingPortRanges(PortRange, PortRange),
    InvalidDeviceIndex(usize),
    InvalidMmioBinding(GuestPhysicalAddress),
    MissingMmioBinding(GuestPhysicalAddress),
    DuplicateMmioBinding(GuestPhysicalAddress),
    Display(DisplayBindingError),
    Memory(MemoryAccessError),
}

impl fmt::Display for BusBuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "machine bus build error: {self:?}")
    }
}
impl std::error::Error for BusBuildError {}

#[derive(Debug, Eq, PartialEq)]
pub enum BusAccessError {
    UnmappedPhysical(GuestPhysicalAddress),
    UnmappedPort(PortAddress),
    AddressOverflow,
    CrossesRegion,
    UnsupportedWidth,
    Memory(MemoryAccessError),
    Device {
        device_index: usize,
        source: DeviceError,
    },
}

impl fmt::Display for BusAccessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "machine bus access error: {self:?}")
    }
}
impl std::error::Error for BusAccessError {}

pub struct MachineBus {
    memory: PhysicalMemory,
    /// The device table, behind the one device lock of S6-33 (MACHINE_CONTRACT.md, lock order).
    /// RAM is not behind it: `memory` is a separate field, reached through a shared reference
    /// without the lock. Code that owns the bus exclusively (every interleaved path) reaches the
    /// table with [`exclusive_devices`], which takes no lock.
    devices: MeasuredMutex<Vec<Box<dyn BusDevice>>>,
    mmio: Vec<MmioBinding>,
    /// Byte length of the region of each `mmio` binding (same order), for the set-step MMIO
    /// dispatch, which must not look at `memory` while the page walk holds it (S4-38L9s).
    mmio_lens: Vec<u64>,
    /// Selected once by the board to restrict set-step MMIO to its opt-in devices.
    set_step_windows: &'static [crate::page_walk::MmioWindow],
    /// S7-31c: the windows the set loop serves when the board runs the firmware ROM in it:
    /// `set_step_windows` and the firmware device apertures. `None` for every other board.
    firmware_set_windows: Option<Vec<crate::page_walk::MmioWindow>>,
    ports: Vec<PortRegionBinding>,
    /// PERF-04: how many times a guest instruction reached a device through the set-step
    /// dispatch (a port in or out, an MMIO read or write). It lives in a shared cell, like
    /// `rdtsc_tick`, so the `stop` predicate of a run loop reads it without borrowing the bus. A
    /// device flag that is raised only by an access of the guest's own instruction can change
    /// only when this count changes.
    device_accesses: Arc<AtomicU64>,
    /// S7-32 diagnostic (`--device-errors log`): the log of refused guest accesses the general
    /// step of the set loop absorbs; `None` (the default) keeps every refusal a stop.
    device_errors: Option<Arc<DeviceErrorLog>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrackedCodeSnapshot {
    pub linear_base: u64,
    pub physical_base: GuestPhysicalAddress,
    pub bytes: Vec<u8>,
    pub backing_generation: u64,
}

/// Executable bytes of one range spanning at most two guest 4 KiB pages
/// (S4-16g). `first` starts at `linear_base`; `second` starts at the next
/// 4 KiB boundary and exists only when the range reaches it. Each part has its
/// own physical page and generation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrackedCrossPageCodeSnapshot {
    pub linear_base: u64,
    pub first: TrackedCodeSnapshot,
    pub second: Option<TrackedCodeSnapshot>,
}

impl TrackedCrossPageCodeSnapshot {
    /// All captured bytes in linear order.
    pub fn bytes(&self) -> Vec<u8> {
        let mut bytes = self.first.bytes.clone();
        if let Some(second) = &self.second {
            bytes.extend_from_slice(&second.bytes);
        }
        bytes
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TrackedCodeSnapshotError {
    InvalidLength,
    InvalidAccessContext,
    Access(TrackedAccessError),
    NotRam(GuestPhysicalAddress),
    MissingTranslation,
    Physical(MemoryAccessError),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TrackedCodePatchError {
    InvalidAccessContext,
    Access(TrackedAccessError),
    PhysicalPageMismatch {
        expected: GuestPhysicalAddress,
        actual: GuestPhysicalAddress,
    },
}

impl MachineBus {
    /// PERF-04: the shared cell of the guest device-access count (see the field).
    pub(crate) fn device_access_counter(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.device_accesses)
    }

    /// Diagnostic-only read of one guest virtual range. The page walk uses a no-op A/D writer,
    /// and the final physical access is admitted only when its entire range is backed by RAM.
    pub(crate) fn read_virtual_ram_diagnostic(
        &self,
        paging: &Long64ProbeConfig,
        rip: u64,
        linear: GuestLinearAddress,
        context: AccessContext,
        output: &mut [u8],
    ) -> Result<(), String> {
        let mut offset = 0usize;
        while offset < output.len() {
            let linear_address = linear
                .get()
                .checked_add(u64::try_from(offset).map_err(|_| "virtual offset overflow")?)
                .ok_or("virtual address overflow")?;
            let bytes_to_page_end = 4096 - (linear_address as usize & 0xfff);
            let fragment_len = bytes_to_page_end.min(output.len() - offset);
            let physical = crate::page_walk::translate_long64_2m_ram_read_only(
                &self.memory,
                paging,
                rip,
                GuestLinearAddress::new(linear_address),
                context,
                AccessKind::Read,
            )
            .map_err(|error| format!("read-only translation failed: {error:?}"))?;
            let end = u128::from(physical.get()) + fragment_len as u128;
            let in_ram = self.memory.region_specs().any(|region| {
                region.kind == MemoryRegionKind::Ram
                    && physical.get() >= region.range.start().get()
                    && end <= region.range.end_exclusive()
            });
            if !in_ram {
                return Err("translated range is not wholly backed by RAM".into());
            }
            self.memory
                .load_bytes(physical, &mut output[offset..offset + fragment_len])
                .map_err(|error| format!("RAM read failed: {error:?}"))?;
            offset += fragment_len;
        }
        Ok(())
    }

    pub fn code_page_generation(
        &self,
        address: GuestPhysicalAddress,
    ) -> Result<u64, MemoryAccessError> {
        self.memory.code_page_generation(address)
    }

    /// Capture one executable byte range contained in a single guest 4 KiB
    /// page. The first byte is translated as an instruction fetch; the page
    /// walker enforces execute permissions and commits its A-bit effects.
    pub fn tracked_code_snapshot(
        &self,
        paging: &Long64ProbeConfig,
        linear_start: u64,
        length: usize,
        context: AccessContext,
    ) -> Result<TrackedCodeSnapshot, TrackedCodeSnapshotError> {
        self.tracked_code_snapshot_with_callback(paging, linear_start, length, context, |_| {})
    }

    /// Capture one executable byte range and report each committed page-walk
    /// write in the order it occurs.
    pub fn tracked_code_snapshot_with_callback(
        &self,
        paging: &Long64ProbeConfig,
        linear_start: u64,
        length: usize,
        context: AccessContext,
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> Result<TrackedCodeSnapshot, TrackedCodeSnapshotError> {
        if context.origin != AccessOrigin::Explicit {
            return Err(TrackedCodeSnapshotError::InvalidAccessContext);
        }
        let offset = (linear_start & 0xfff) as usize;
        if length == 0 || length > 0x1000 - offset {
            return Err(TrackedCodeSnapshotError::InvalidLength);
        }
        self.capture_code_page(
            paging,
            linear_start,
            linear_start,
            length,
            context,
            committed_write,
        )
    }

    /// Capture one executable byte range that starts in the 4 KiB page of
    /// `linear_start` and may continue into the next page, at most two pages
    /// in total (S4-16g). The first page is walked as an instruction fetch at
    /// `linear_start`; the second page, when the range reaches it, gets its own
    /// fetch walk at the page boundary with the original `linear_start` as the
    /// instruction RIP, so a second-page fault carries the same
    /// `PageFaultRecord` (saved RIP, CR2 = boundary address) as a byte-wise
    /// interpreter fetch. Each page keeps its own physical base and
    /// generation; the physical pages need not be adjacent. A range that does
    /// not fit two pages is `InvalidLength` before any walk. A second-page
    /// failure keeps the first-page walk effects (reported through the
    /// callback) and returns no snapshot.
    pub fn tracked_code_snapshot_two_pages_with_callback(
        &self,
        paging: &Long64ProbeConfig,
        linear_start: u64,
        length: usize,
        context: AccessContext,
        mut committed_write: impl FnMut(GuestPhysicalRange),
    ) -> Result<TrackedCrossPageCodeSnapshot, TrackedCodeSnapshotError> {
        if context.origin != AccessOrigin::Explicit {
            return Err(TrackedCodeSnapshotError::InvalidAccessContext);
        }
        let offset = (linear_start & 0xfff) as usize;
        let first_length_max = 0x1000 - offset;
        if length == 0 || length > first_length_max + 0x1000 {
            return Err(TrackedCodeSnapshotError::InvalidLength);
        }
        let first_length = length.min(first_length_max);
        let mut first = self.capture_code_page(
            paging,
            linear_start,
            linear_start,
            first_length,
            context,
            &mut committed_write,
        )?;
        if length == first_length {
            return Ok(TrackedCrossPageCodeSnapshot {
                linear_base: linear_start,
                first,
                second: None,
            });
        }
        let second_linear = (linear_start & !0xfff)
            .checked_add(0x1000)
            .ok_or(TrackedCodeSnapshotError::InvalidLength)?;
        let second = self.capture_code_page(
            paging,
            linear_start,
            second_linear,
            length - first_length,
            context,
            &mut committed_write,
        )?;
        // The second walk can commit A/D bits into a page-table page that is
        // also the first code page. Re-read the first page so both snapshots
        // describe memory as it is when the call returns.
        let first_generation = self
            .memory
            .code_page_generation(first.physical_base)
            .map_err(TrackedCodeSnapshotError::Physical)?;
        if first_generation != first.backing_generation {
            let byte_address = GuestPhysicalAddress::new(first.physical_base.get() + offset as u64);
            self.memory
                .load_bytes(byte_address, &mut first.bytes)
                .map_err(TrackedCodeSnapshotError::Physical)?;
            first.backing_generation = first_generation;
        }
        Ok(TrackedCrossPageCodeSnapshot {
            linear_base: linear_start,
            first,
            second: Some(second),
        })
    }

    /// Walk `fetch_linear` as an instruction fetch of the instruction at
    /// `instruction_rip` and read `length` bytes from its page. The caller
    /// guarantees that the range lies within the page of `fetch_linear`.
    fn capture_code_page(
        &self,
        paging: &Long64ProbeConfig,
        instruction_rip: u64,
        linear_start: u64,
        length: usize,
        context: AccessContext,
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> Result<TrackedCodeSnapshot, TrackedCodeSnapshotError> {
        let offset = (linear_start & 0xfff) as usize;
        let report = self.access_tracked_ram(
            paging,
            TrackedAccessRequest {
                instruction_rip,
                address: crate::GuestLinearAddress::new(linear_start),
                context,
                operation: RamAccessOperation::Fetch,
            },
            committed_write,
        );
        let physical = match report.outcome {
            Ok(TrackedAccessOutcome::Fetched(_)) => report
                .trace
                .entries()
                .iter()
                .flatten()
                .last()
                .map(|entry| entry.entry_value & TRACKED_CODE_FRAME_MASK)
                .ok_or(TrackedCodeSnapshotError::MissingTranslation)?,
            Ok(_) => return Err(TrackedCodeSnapshotError::MissingTranslation),
            Err(error) => return Err(TrackedCodeSnapshotError::Access(error)),
        };
        let physical_base = GuestPhysicalAddress::new(physical);
        let region = self
            .memory
            .locate_region_kind(physical_base, 0x1000)
            .map_err(TrackedCodeSnapshotError::Physical)?;
        if region != MemoryRegionKind::Ram {
            return Err(TrackedCodeSnapshotError::NotRam(physical_base));
        }
        let byte_address = GuestPhysicalAddress::new(physical + offset as u64);
        // S6-30b: the generation is read BEFORE the bytes (it is an `Acquire` load). A write that
        // lands between the two leaves the snapshot older than the page, so the next generation
        // check refreshes it; the reverse order could pair old bytes with a new generation.
        let backing_generation = self
            .memory
            .code_page_generation(physical_base)
            .map_err(TrackedCodeSnapshotError::Physical)?;
        let mut bytes = vec![0; length];
        self.memory
            .load_bytes(byte_address, &mut bytes)
            .map_err(TrackedCodeSnapshotError::Physical)?;
        Ok(TrackedCodeSnapshot {
            linear_base: linear_start,
            physical_base,
            bytes,
            backing_generation,
        })
    }

    /// Store one loader patch byte through a writable guest virtual alias of
    /// the captured physical page. The read preflight verifies the alias
    /// before the tracked write is allowed to commit. Either walk may commit
    /// Accessed effects and the write walk may commit Dirty effects before a
    /// later failure.
    pub fn patch_tracked_code_byte(
        &mut self,
        paging: &Long64ProbeConfig,
        linear_address: u64,
        expected_physical_page: GuestPhysicalAddress,
        value: u8,
        context: AccessContext,
    ) -> Result<GuestPhysicalAddress, TrackedCodePatchError> {
        self.patch_tracked_code_byte_with_callback(
            paging,
            linear_address,
            expected_physical_page,
            value,
            context,
            |_| {},
        )
    }

    /// Store one code byte and report committed A/D and operand writes in
    /// operation order. Failed operations report only writes that committed
    /// before the failure.
    pub fn patch_tracked_code_byte_with_callback(
        &mut self,
        paging: &Long64ProbeConfig,
        linear_address: u64,
        expected_physical_page: GuestPhysicalAddress,
        value: u8,
        context: AccessContext,
        mut committed_write: impl FnMut(GuestPhysicalRange),
    ) -> Result<GuestPhysicalAddress, TrackedCodePatchError> {
        if context.origin != AccessOrigin::Explicit {
            return Err(TrackedCodePatchError::InvalidAccessContext);
        }
        let request = |operation| TrackedAccessRequest {
            instruction_rip: linear_address,
            address: crate::GuestLinearAddress::new(linear_address),
            context,
            operation,
        };
        let preflight = self.access_tracked_ram(
            paging,
            request(RamAccessOperation::Read {
                width: AccessWidth::Byte,
            }),
            &mut committed_write,
        );
        if let Err(error) = preflight.outcome {
            return Err(TrackedCodePatchError::Access(error));
        }
        let actual = preflight
            .trace
            .entries()
            .iter()
            .flatten()
            .last()
            .map(|entry| {
                GuestPhysicalAddress::new(
                    (entry.entry_value & TRACKED_CODE_FRAME_MASK) | (linear_address & 0xfff),
                )
            })
            .ok_or_else(|| {
                TrackedCodePatchError::Access(TrackedAccessError::DeviceUnavailable {
                    address: expected_physical_page,
                })
            })?;
        if actual.get() & TRACKED_CODE_FRAME_MASK != expected_physical_page.get() {
            return Err(TrackedCodePatchError::PhysicalPageMismatch {
                expected: expected_physical_page,
                actual,
            });
        }
        let write = self.access_tracked_ram(
            paging,
            request(RamAccessOperation::Write {
                width: AccessWidth::Byte,
                value: u64::from(value),
            }),
            &mut committed_write,
        );
        match write.outcome {
            Ok(TrackedAccessOutcome::Written) => Ok(actual),
            Ok(_) => Err(TrackedCodePatchError::Access(
                TrackedAccessError::DeviceUnavailable { address: actual },
            )),
            Err(error) => Err(TrackedCodePatchError::Access(error)),
        }
    }

    /// Loader write used by the inactive-code session. PhysicalMemory
    /// validates the whole access and advances its conservative code version
    /// only after the RAM bytes commit.
    pub fn patch_physical_ram(
        &mut self,
        address: GuestPhysicalAddress,
        bytes: &[u8],
    ) -> Result<(), MemoryAccessError> {
        self.patch_physical_ram_with_callback(address, bytes, |_| {})
    }

    /// Write physical RAM and report the exact range after the bytes commit.
    /// Validation and generation advancement remain owned by PhysicalMemory.
    pub fn patch_physical_ram_with_callback(
        &mut self,
        address: GuestPhysicalAddress,
        bytes: &[u8],
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> Result<(), MemoryAccessError> {
        self.memory.store_bytes(address, bytes, committed_write)
    }
}

/// Disjoint PIO view used while the CPU fetch adapter borrows physical RAM.
pub(crate) struct MachineBusPortIo<'a> {
    devices: &'a mut [Box<dyn BusDevice>],
    ports: &'a [PortRegionBinding],
    quantum_start_ns: Option<u64>,
    retired_before: u64,
}

impl MachineBusPortIo<'_> {
    pub(crate) fn set_firmware_time(&mut self, quantum_start_ns: Option<u64>, retired_before: u64) {
        self.quantum_start_ns = quantum_start_ns;
        self.retired_before = retired_before;
    }

    fn rtc_time(&self, port: u16) -> Option<impl Fn() -> Result<u64, DeviceError> + use<>> {
        if port != crate::rtc_cmos::CMOS_DATA_PORT {
            return None;
        }
        let start = self.quantum_start_ns?;
        let retired = self.retired_before;
        Some(move || {
            use crate::firmware_board::FIRMWARE_EXECUTION_CYCLE_NS;
            retired
                .checked_mul(FIRMWARE_EXECUTION_CYCLE_NS)
                .and_then(|delta| start.checked_add(delta))
                .ok_or(DeviceError::ClockOverflow)
        })
    }
}

/// The device and port halves of one set-loop instruction (S4-38L9s). The CPU core takes the
/// memory (which routes MMIO data accesses) and the port view in the same call, and both reach the
/// same device table (the PCI device is bound to ECAM MMIO and to the conf1 ports), so the table
/// sits in a `RefCell`. One instruction makes its accesses one after another, so the cell is never
/// borrowed twice; if it ever is, that is a typed `Busy` error and not a panic.
pub(crate) struct MachineBusSetStepIo<'a> {
    mmio_written: Cell<bool>,
    devices: DeviceTable<'a>,
    /// The gate of the split `LOCK` of the vCPU thread (S6-34 step 8); `None` outside the
    /// parallel runner.
    gate: Option<&'a dyn SplitLockGate>,
    ports: &'a [PortRegionBinding],
    mmio: &'a [MmioBinding],
    mmio_lens: &'a [u64],
    windows: &'a [crate::page_walk::MmioWindow],
    /// Index of the vCPU whose set loop this is (S6-16a): its MMIO accesses reach the
    /// per-processor devices (the Local APIC window) as that vCPU's. Every other device ignores it.
    vcpu: usize,
    /// The count of [`MachineBus::device_access_counter`]; every device access of this view adds
    /// one before the device is called.
    device_accesses: &'a AtomicU64,
    /// S7-32 diagnostic: the log of the bus, if it has one. A refusal is absorbed only when the
    /// view also has the instruction (`device_error_context`): the general step of the set loop
    /// gives it; the views of the turbo engine and of every other loop do not, so their refusals
    /// stay what they were. Both are read on the error path only.
    device_errors: Option<&'a DeviceErrorLog>,
    device_error_context: Option<DeviceErrorContext>,
    /// S7-32 diagnostic: the guest-physical range (start, bytes) of the last MMIO read of this
    /// instruction the log absorbed. A write of the same instruction that overlaps it (the write
    /// half of a read-modify-write at CPL0) is not sent to the device: its value derives from
    /// the all-ones the guest was given. Set only on the error path, so `None` without a log.
    absorbed_read: Cell<Option<(u64, u64)>>,
}

/// How the set-step view of one instruction reaches the device table (S6-34). `Exclusive`: the
/// caller owns the bus (`&mut MachineBus`, every interleaved path); the table is borrowed through
/// a cell, no lock is taken. `Shared`: the caller holds only `&MachineBus` (a vCPU thread); every
/// access takes the one device lock of the bus for the duration of that access and releases it
/// before returning (MACHINE_CONTRACT.md, lock order, rule 3: never held across an instruction, a
/// `HLT` or a wait).
enum DeviceTable<'a> {
    Exclusive(RefCell<&'a mut [Box<dyn BusDevice>]>),
    Shared(&'a MeasuredMutex<Vec<Box<dyn BusDevice>>>),
}

impl DeviceTable<'_> {
    /// Run `f` on the table for one access; `Err` when the exclusive cell is already borrowed.
    fn with<R>(
        &self,
        f: impl FnOnce(&mut [Box<dyn BusDevice>]) -> R,
    ) -> Result<R, DeviceTableBusy> {
        match self {
            Self::Exclusive(cell) => {
                let mut devices = cell.try_borrow_mut().map_err(|_| DeviceTableBusy)?;
                Ok(f(&mut devices))
            }
            Self::Shared(lock) => {
                let mut devices = lock.lock().unwrap_or_else(PoisonError::into_inner);
                Ok(f(&mut devices))
            }
        }
    }
}

struct DeviceTableBusy;

/// The port view of [`MachineBusSetStepIo`].
pub(crate) struct SetStepPortIo<'a, 'b>(&'b MachineBusSetStepIo<'a>);

/// MMIO dispatch of one tracked data access, for the page walker of the set loop.
pub(crate) trait SetStepMmio {
    /// Every physical range this dispatch serves: the ECAM window, and with the APIC profile
    /// (S4-38L8d) the Local APIC and IOAPIC windows. Every other MMIO operand of the set loop
    /// stays the typed error it was before S4-38L9s.
    fn windows(&self) -> &[crate::page_walk::MmioWindow];

    fn dispatch(
        &self,
        address: GuestPhysicalAddress,
        width: u8,
        operation: RamAccessOperation,
    ) -> Result<u64, crate::page_walk::MmioDispatchError>;

    /// The gate of the split `LOCK` of the vCPU thread, if the view belongs to one (S6-34 step 8).
    fn split_gate(&self) -> Option<&dyn SplitLockGate> {
        None
    }
}

impl<'a> MachineBusSetStepIo<'a> {
    pub(crate) fn port_io(&self) -> SetStepPortIo<'a, '_> {
        SetStepPortIo(self)
    }

    pub(crate) fn mmio_written(&self) -> bool {
        self.mmio_written.get()
    }

    /// The one place where this view reaches a device (a port or an MMIO access): it counts the
    /// access (PERF-04) and runs `f` on the device table.
    fn with_counted_devices<R>(
        &self,
        f: impl FnOnce(&mut [Box<dyn BusDevice>]) -> R,
    ) -> Result<R, DeviceTableBusy> {
        self.device_accesses.fetch_add(1, AtomicOrdering::Relaxed);
        self.devices.with(f)
    }

    /// The same view with the gate of the split `LOCK` of the vCPU thread (S6-34 step 8).
    pub(crate) fn with_split_gate(mut self, gate: Option<&'a dyn SplitLockGate>) -> Self {
        self.gate = gate;
        self
    }

    /// S7-32 diagnostic: the same view for the instruction `context` of the general step. With a
    /// log on the bus a refused access of this view is absorbed (see [`DeviceErrorLog`]); without
    /// one nothing changes.
    pub(crate) fn with_device_error_context(mut self, context: DeviceErrorContext) -> Self {
        self.device_error_context = Some(context);
        self
    }

    /// The error path of a port access: with the log and the instruction, a refusal of a device
    /// (or a port no device is bound to) is recorded and completed as an open bus (a read gives
    /// all ones of the width, a write is dropped); otherwise, or once a limit is reached, the
    /// error is returned as it was.
    #[cold]
    #[inline(never)]
    fn absorb_port(
        &self,
        error: BusAccessError,
        port: u16,
        width: AccessWidth,
        value: Option<u32>,
    ) -> Result<u32, BusAccessError> {
        let (Some(log), Some(context)) = (self.device_errors, self.device_error_context) else {
            return Err(error);
        };
        let (device_index, kind) = match &error {
            BusAccessError::Device {
                device_index,
                source,
            } if *device_index != usize::MAX && absorbable(source) => {
                (Some(*device_index), DeviceErrorKind::Device(source.clone()))
            }
            BusAccessError::UnmappedPort(_) => (None, DeviceErrorKind::UnmappedPort),
            BusAccessError::CrossesRegion => (None, DeviceErrorKind::CrossesRegion),
            _ => return Err(error),
        };
        let len = u32::from(width.bytes());
        let region_base = device_index.and_then(|_| {
            self.ports
                .iter()
                .find(|binding| binding.range.contains(PortAddress::new(port), len))
                .map(|binding| u64::from(binding.range.start.0))
        });
        let access = DeviceErrorAccess {
            key: DeviceErrorKey {
                space: IoSpace::Pio,
                device_index,
                address: u64::from(port),
                direction: if value.is_some() {
                    DeviceErrorDirection::Write
                } else {
                    DeviceErrorDirection::Read
                },
                width,
                kind,
            },
            region_base,
            value: value.map(u64::from),
        };
        if log.absorb(access, context) {
            Ok(width.mask() as u32)
        } else {
            Err(error)
        }
    }

    /// The error path of an MMIO access to a device: as [`Self::absorb_port`] for the refusal
    /// `error` of device `device_index` (bound at `region_start`).
    #[cold]
    #[inline(never)]
    #[allow(clippy::too_many_arguments)]
    fn absorb_mmio(
        &self,
        error: DeviceError,
        address: GuestPhysicalAddress,
        region_start: GuestPhysicalAddress,
        device_index: usize,
        width: AccessWidth,
        value: Option<u64>,
    ) -> Result<u64, DeviceError> {
        let (Some(log), Some(context)) = (self.device_errors, self.device_error_context) else {
            return Err(error);
        };
        if !absorbable(&error) {
            return Err(error);
        }
        let access = DeviceErrorAccess {
            key: DeviceErrorKey {
                space: IoSpace::Mmio,
                device_index: Some(device_index),
                address: address.get(),
                direction: if value.is_some() {
                    DeviceErrorDirection::Write
                } else {
                    DeviceErrorDirection::Read
                },
                width,
                kind: DeviceErrorKind::Device(error.clone()),
            },
            region_base: Some(region_start.get()),
            value,
        };
        if !log.absorb(access, context) {
            return Err(error);
        }
        if value.is_none() {
            self.absorbed_read
                .set(Some((address.get(), u64::from(width.bytes()))));
        }
        Ok(width.mask())
    }

    /// Whether an MMIO write of `len` bytes at `address` overlaps the read of this instruction
    /// the log absorbed (always `false` without a log: nothing is absorbed then).
    fn follows_an_absorbed_read(&self, address: GuestPhysicalAddress, len: u64) -> bool {
        self.absorbed_read.get().is_some_and(|(start, bytes)| {
            u128::from(address.get()) < u128::from(start) + u128::from(bytes)
                && u128::from(start) < u128::from(address.get()) + u128::from(len)
        })
    }

    /// The write that follows an absorbed read of the same instruction (see `absorbed_read`):
    /// dropped without reaching the device and recorded as its own kind of event; once the log
    /// is full the run stops on it.
    #[cold]
    #[inline(never)]
    fn suppress_write_after_absorbed_read(
        &self,
        address: GuestPhysicalAddress,
        region_start: GuestPhysicalAddress,
        device_index: usize,
        width: AccessWidth,
        value: u64,
    ) -> Result<u64, crate::page_walk::MmioDispatchError> {
        let recorded = match (self.device_errors, self.device_error_context) {
            (Some(log), Some(context)) => log.absorb(
                DeviceErrorAccess {
                    key: DeviceErrorKey {
                        space: IoSpace::Mmio,
                        device_index: Some(device_index),
                        address: address.get(),
                        direction: DeviceErrorDirection::Write,
                        width,
                        kind: DeviceErrorKind::SuppressedAfterAbsorbedRead,
                    },
                    region_base: Some(region_start.get()),
                    value: Some(value),
                },
                context,
            ),
            _ => false,
        };
        if recorded {
            Ok(0)
        } else {
            Err(crate::page_walk::MmioDispatchError::Backend(format!(
                "MMIO device {device_index} write after an absorbed read of the same \
                 instruction: not sent, the device error log is full"
            )))
        }
    }
}

/// The stop of every other vCPU around a split `LOCK` (S6-34 step 8, MEMORY_MODEL.md D8). The
/// vCPU that executes a locked operand which is not naturally aligned (so that no host atomic
/// covers it) calls [`Self::acquire`] before ANY visible effect of the instruction (before its
/// first walk, so before an accessed or dirty bit is set) and [`Self::release`] when the
/// operation is done. `acquire` returns when every other vCPU is quiet (stands on an instruction
/// boundary, is asleep, or waits in its own `acquire` before any effect); it fails (`Err`, no
/// effect, nothing to release) when that does not happen in the time allowed. When two vCPUs ask
/// at once, one gets it and the other waits inside `acquire`, quiet, until the first has released,
/// and then asks again: the instruction of the loser has had no effect, so it starts over.
pub(crate) trait SplitLockGate {
    fn acquire(&self) -> Result<(), String>;

    /// A descriptor transaction needs the same quiescence as a split operand.
    fn acquire_for_descriptor(&self) -> Result<(), String> {
        self.acquire()
    }

    fn release(&self);
}

/// Excludes other vCPUs and synchronous DMA for a descriptor transaction.
pub(crate) struct DescriptorGate<'a>(&'a dyn SplitLockGate);

impl<'a> DescriptorGate<'a> {
    pub(crate) fn acquire(gate: &'a dyn SplitLockGate) -> Result<Self, String> {
        gate.acquire_for_descriptor()?;
        Ok(Self(gate))
    }
}

impl Drop for DescriptorGate<'_> {
    fn drop(&mut self) {
        self.0.release();
    }
}

/// The ECAM window: the PCI configuration device is the one MMIO device the set loop's own profile
/// needs (S4-38L9s). Without the APIC profile it is the only MMIO the set loop serves; the other
/// MMIO (Local APIC, IOAPIC, display, NVMe) is not admitted.
const ECAM_WINDOW: crate::page_walk::MmioWindow = (
    crate::PCI_ECAM_DESCRIPTOR.base,
    crate::PCI_ECAM_DESCRIPTOR.size,
);

const SET_STEP_WINDOWS: [crate::page_walk::MmioWindow; 1] = [ECAM_WINDOW];
/// S4-38L8d: with the APIC profile the set loop also reaches the Local APIC and the IOAPIC (one 4 KiB
/// window each, MADT addresses), and still nothing else.
const SET_STEP_APIC_WINDOWS: [crate::page_walk::MmioWindow; 3] = [
    ECAM_WINDOW,
    (crate::LOCAL_APIC_MMIO_BASE, crate::LOCAL_APIC_MMIO_SIZE),
    (crate::IO_APIC_MMIO_BASE, crate::IO_APIC_MMIO_SIZE),
];

const NVME_PCI_WINDOW: crate::page_walk::MmioWindow = (
    crate::nvme::NVME_PCI_BAR_BASE,
    crate::nvme::NVME_PCI_BAR_BYTES,
);
const SET_STEP_NVME_WINDOWS: [crate::page_walk::MmioWindow; 2] = [ECAM_WINDOW, NVME_PCI_WINDOW];
const SET_STEP_APIC_NVME_WINDOWS: [crate::page_walk::MmioWindow; 4] = [
    SET_STEP_APIC_WINDOWS[0],
    SET_STEP_APIC_WINDOWS[1],
    SET_STEP_APIC_WINDOWS[2],
    NVME_PCI_WINDOW,
];

impl SetStepMmio for MachineBusSetStepIo<'_> {
    fn windows(&self) -> &[crate::page_walk::MmioWindow] {
        self.windows
    }

    fn split_gate(&self) -> Option<&dyn SplitLockGate> {
        self.gate
    }

    fn dispatch(
        &self,
        address: GuestPhysicalAddress,
        width: u8,
        operation: RamAccessOperation,
    ) -> Result<u64, crate::page_walk::MmioDispatchError> {
        use crate::page_walk::MmioDispatchError::Backend;
        let width = match width {
            8 => AccessWidth::Qword,
            4 => AccessWidth::Dword,
            2 => AccessWidth::Word,
            1 => AccessWidth::Byte,
            _ => return Err(Backend(format!("unsupported MMIO width {width}"))),
        };
        let len = u64::from(width.bytes());
        let served = self.windows.iter().any(|&(base, size)| {
            address.get() >= base
                && u128::from(address.get()) + u128::from(len)
                    <= u128::from(base) + u128::from(size)
        });
        if !served {
            return Err(Backend(format!(
                "MMIO address {address:?} is outside the windows the set loop serves"
            )));
        }
        let (binding, _) = self
            .mmio
            .iter()
            .zip(self.mmio_lens)
            .find(|(binding, region_len)| {
                binding.region_start <= address
                    && u128::from(address.get()) + u128::from(len)
                        <= u128::from(binding.region_start.get()) + u128::from(**region_len)
            })
            .ok_or_else(|| {
                Backend("MMIO range is not wholly inside one device region".to_owned())
            })?;
        let access = DeviceAccess {
            space: IoSpace::Mmio,
            offset: address.get() - binding.region_start.get(),
            width,
        };
        let vcpu = self.vcpu;
        let device_index = binding.device_index;
        match operation {
            RamAccessOperation::Read { .. } => self
                .with_counted_devices(|devices| devices[device_index].read_from_cpu(vcpu, access))
                .map_err(|_| Backend("the device table is already borrowed".to_owned()))?
                .map(|value| value & width.mask())
                .or_else(|error| {
                    self.absorb_mmio(
                        error,
                        address,
                        binding.region_start,
                        device_index,
                        width,
                        None,
                    )
                })
                .map_err(|error| {
                    Backend(format!("MMIO device {device_index} read failed: {error:?}"))
                }),
            // S7-32 diagnostic: the write half of a read-modify-write whose read the log absorbed
            // does not reach the device (one `Cell` load: without a log it is always `None`).
            RamAccessOperation::Write { value, .. }
                if self.follows_an_absorbed_read(address, len) =>
            {
                self.suppress_write_after_absorbed_read(
                    address,
                    binding.region_start,
                    device_index,
                    width,
                    value & width.mask(),
                )
            }
            RamAccessOperation::Write { value, .. } => self
                .with_counted_devices(|devices| {
                    devices[device_index].write_from_cpu(vcpu, access, value & width.mask())
                })
                .map_err(|_| Backend("the device table is already borrowed".to_owned()))?
                .map(|()| {
                    self.mmio_written.set(true);
                    0
                })
                // An absorbed write is dropped: nothing reached the device, `mmio_written` stays.
                .or_else(|error| {
                    self.absorb_mmio(
                        error,
                        address,
                        binding.region_start,
                        device_index,
                        width,
                        Some(value & width.mask()),
                    )
                    .map(|_| 0)
                })
                .map_err(|error| {
                    Backend(format!(
                        "MMIO device {device_index} write failed: {error:?}"
                    ))
                }),
            RamAccessOperation::Fetch => Err(Backend(
                "instruction fetch from MMIO is unsupported".to_owned(),
            )),
            RamAccessOperation::ReadPseudoDescriptor => {
                Err(Backend("pseudo-descriptor reads are RAM-only".to_owned()))
            }
            RamAccessOperation::ReadForWrite { .. } => {
                Err(Backend("write-intent reads are RAM-only".to_owned()))
            }
            // S6-31: a locked operand is never served by a device; the walker refuses an MMIO
            // operand before it reaches this dispatch.
            RamAccessOperation::AtomicTarget { .. } => {
                Err(Backend("locked operations are RAM-only".to_owned()))
            }
        }
    }
}

impl SetStepPortIo<'_, '_> {
    fn with_devices<R>(
        &self,
        f: impl FnOnce(&mut [Box<dyn BusDevice>]) -> Result<R, BusAccessError>,
    ) -> Result<R, BusAccessError> {
        self.0
            .with_counted_devices(f)
            .map_err(|_| BusAccessError::Device {
                device_index: usize::MAX,
                source: DeviceError::Busy,
            })?
    }
}

impl PortIo for SetStepPortIo<'_, '_> {
    type Error = BusAccessError;

    fn read_u8(&mut self, port: u16) -> Result<u8, Self::Error> {
        let ports = self.0.ports;
        self.with_devices(|devices| {
            read_port_from(devices, ports, PortAddress::new(port), AccessWidth::Byte)
        })
        .or_else(|error| self.0.absorb_port(error, port, AccessWidth::Byte, None))
        .map(|value| value as u8)
    }

    fn write_u8(&mut self, port: u16, value: u8) -> Result<(), Self::Error> {
        let ports = self.0.ports;
        self.with_devices(|devices| {
            write_port_to(
                devices,
                ports,
                PortAddress::new(port),
                AccessWidth::Byte,
                u32::from(value),
            )
        })
        .or_else(|error| {
            self.0
                .absorb_port(error, port, AccessWidth::Byte, Some(u32::from(value)))
                .map(|_| ())
        })
    }

    fn read_u16(&mut self, port: u16) -> Result<u16, Self::Error> {
        let ports = self.0.ports;
        self.with_devices(|devices| {
            read_port_from(devices, ports, PortAddress::new(port), AccessWidth::Word)
        })
        .or_else(|error| self.0.absorb_port(error, port, AccessWidth::Word, None))
        .map(|value| value as u16)
    }

    fn write_u16(&mut self, port: u16, value: u16) -> Result<(), Self::Error> {
        let ports = self.0.ports;
        self.with_devices(|devices| {
            write_port_to(
                devices,
                ports,
                PortAddress::new(port),
                AccessWidth::Word,
                u32::from(value),
            )
        })
        .or_else(|error| {
            self.0
                .absorb_port(error, port, AccessWidth::Word, Some(u32::from(value)))
                .map(|_| ())
        })
    }

    fn read_u32(&mut self, port: u16) -> Result<u32, Self::Error> {
        let ports = self.0.ports;
        self.with_devices(|devices| {
            read_port_from(devices, ports, PortAddress::new(port), AccessWidth::Dword)
        })
        .or_else(|error| self.0.absorb_port(error, port, AccessWidth::Dword, None))
    }

    fn write_u32(&mut self, port: u16, value: u32) -> Result<(), Self::Error> {
        let ports = self.0.ports;
        self.with_devices(|devices| {
            write_port_to(
                devices,
                ports,
                PortAddress::new(port),
                AccessWidth::Dword,
                value,
            )
        })
        .or_else(|error| {
            self.0
                .absorb_port(error, port, AccessWidth::Dword, Some(value))
                .map(|_| ())
        })
    }
}

impl MachineBus {
    /// Memory, ports and MMIO of one set-loop instruction (S4-38L9s); the cell inside the
    /// returned value lets the port view and the MMIO dispatch share the device table.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn split_for_set_step(&mut self) -> (&PhysicalMemory, MachineBusSetStepIo<'_>) {
        self.split_for_set_step_of_vcpu(crate::vcpu::BOOT_VCPU_INDEX)
    }

    /// [`MachineBus::split_for_set_step`] for the set loop of vCPU `vcpu` (S6-16a): the MMIO
    /// dispatch of the returned value serves a Local APIC access as that processor's.
    pub(crate) fn split_for_set_step_of_vcpu(
        &mut self,
        vcpu: usize,
    ) -> (&PhysicalMemory, MachineBusSetStepIo<'_>) {
        let Self {
            memory,
            devices,
            mmio,
            mmio_lens,
            set_step_windows,
            firmware_set_windows,
            ports,
            device_accesses,
            device_errors,
        } = self;
        (
            memory,
            MachineBusSetStepIo {
                mmio_written: Cell::new(false),
                devices: DeviceTable::Exclusive(RefCell::new(
                    exclusive_devices(devices).as_mut_slice(),
                )),
                gate: None,
                ports,
                mmio,
                mmio_lens,
                windows: firmware_set_windows.as_deref().unwrap_or(*set_step_windows),
                vcpu,
                device_accesses,
                device_errors: device_errors.as_deref(),
                device_error_context: None,
                absorbed_read: Cell::new(None),
            },
        )
    }

    /// [`Self::split_for_set_step_of_vcpu`] for a vCPU thread that holds only `&MachineBus`
    /// (S6-34): the memory is the shared reference, and the MMIO dispatch and the port view of the
    /// returned value take the one device lock for each access and release it before they
    /// return. Nothing else of the bus is mutated.
    pub(crate) fn split_for_set_step_shared(
        &self,
        vcpu: usize,
    ) -> (&PhysicalMemory, MachineBusSetStepIo<'_>) {
        (
            &self.memory,
            MachineBusSetStepIo {
                mmio_written: Cell::new(false),
                devices: DeviceTable::Shared(&self.devices),
                gate: None,
                ports: &self.ports,
                mmio: &self.mmio,
                mmio_lens: &self.mmio_lens,
                windows: self
                    .firmware_set_windows
                    .as_deref()
                    .unwrap_or(self.set_step_windows),
                vcpu,
                device_accesses: &self.device_accesses,
                device_errors: self.device_errors.as_deref(),
                device_error_context: None,
                absorbed_read: Cell::new(None),
            },
        )
    }

    /// S4-38L8d: let the set loop reach the Local APIC and IOAPIC windows (off by default). Only the
    /// APIC profile of the Linux board turns it on.
    pub(crate) fn enable_apic_mmio_windows(&mut self) {
        self.set_step_windows = if self.set_step_windows.contains(&NVME_PCI_WINDOW) {
            &SET_STEP_APIC_NVME_WINDOWS
        } else {
            &SET_STEP_APIC_WINDOWS
        };
    }

    /// S7-31c (ADR-0013 variant (b)): let the set loop reach the firmware device apertures (NVRAM,
    /// display registers, the firmware NVMe window, TPM CRB). The devices are bound on the bus
    /// like every other MMIO device of the board; the set loop dispatches an access to them
    /// through those bindings, once the window is served. Call it after the `enable_*` window
    /// calls: the windows are taken from the bus as it is now.
    pub(crate) fn serve_firmware_set_windows(
        &mut self,
        apertures: &[crate::page_walk::MmioWindow],
    ) {
        let mut windows = self.set_step_windows.to_vec();
        for aperture in apertures {
            if !windows.contains(aperture) {
                windows.push(*aperture);
            }
        }
        self.firmware_set_windows = Some(windows);
    }

    /// S7-32 diagnostic (`--device-errors log`): installs the log of refused guest accesses (see
    /// [`DeviceErrorLog`]). From now on the general step of the set loop absorbs a refusal of a
    /// device, or an access to a port no device is bound to, until `event_limit` accesses or
    /// `group_limit` groups; nothing else of the bus changes. A diagnostic, never a compatibility
    /// mode. One vCPU only: the log has one retired base, which only the single-vCPU firmware
    /// board loop (`run_firmware_unified_guest`) sets; on any other loop the retired counts of the
    /// groups are counted per call from the last base the board set (0 if none).
    pub fn enable_device_error_log(&mut self, event_limit: u64, group_limit: usize) {
        self.device_errors = Some(Arc::new(DeviceErrorLog::new(event_limit, group_limit)));
    }

    /// The log of [`Self::enable_device_error_log`], if installed.
    pub fn device_error_log(&self) -> Option<&Arc<DeviceErrorLog>> {
        self.device_errors.as_ref()
    }

    pub(crate) fn enable_nvme_pci_mmio_window(&mut self) {
        self.set_step_windows = if self.set_step_windows.contains(&SET_STEP_APIC_WINDOWS[1]) {
            &SET_STEP_APIC_NVME_WINDOWS
        } else {
            &SET_STEP_NVME_WINDOWS
        };
    }

    pub(crate) fn split_for_pio_step(&mut self) -> (&PhysicalMemory, MachineBusPortIo<'_>) {
        let Self {
            memory,
            devices,
            ports,
            ..
        } = self;
        (
            memory,
            MachineBusPortIo {
                devices: exclusive_devices(devices),
                ports,
                quantum_start_ns: None,
                retired_before: 0,
            },
        )
    }

    /// A tracked access by vCPU `vcpu`: an MMIO operand reaches the device as that vCPU's access.
    pub(crate) fn access_tracked(
        &mut self,
        vcpu: usize,
        paging: &Long64ProbeConfig,
        request: TrackedAccessRequest,
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> TrackedAccessReport {
        let Self {
            memory,
            devices,
            mmio,
            ..
        } = self;
        let devices = exclusive_devices(devices);
        let regions = memory.region_specs().collect::<Vec<_>>();
        crate::page_walk::access_long64_4k_with_mmio(
            memory,
            paging,
            request,
            committed_write,
            None,
            |address, width, operation| {
                dispatch_tracked_mmio(&regions, mmio, devices, vcpu, address, width, operation)
            },
        )
    }

    /// S2-25k: a tracked operand access of PAE paging by vCPU `vcpu`: the walk of
    /// [`crate::page_walk::pae`], then the physical address decides RAM, device or refusal; an MMIO
    /// operand reaches the device as that vCPU's access (the same dispatch as [`Self::access_tracked`]).
    pub(crate) fn access_tracked_pae(
        &mut self,
        vcpu: usize,
        paging: &PaeProbeConfig,
        request: PaeAccessRequest,
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> TrackedAccessReport {
        let Self {
            memory,
            devices,
            mmio,
            ..
        } = self;
        let devices = exclusive_devices(devices);
        let regions = memory.region_specs().collect::<Vec<_>>();
        crate::page_walk::pae::access_pae_with_mmio(
            memory,
            paging,
            request,
            committed_write,
            |address, width, operation| {
                dispatch_tracked_mmio(&regions, mmio, devices, vcpu, address, width, operation)
            },
        )
    }

    /// S2-25 remainder: a tracked operand access of 32-bit paging by vCPU `vcpu`: the walk of
    /// [`crate::page_walk::pg32`], then the physical address decides RAM, device or refusal, as for
    /// [`Self::access_tracked_pae`].
    pub(crate) fn access_tracked_pg32(
        &mut self,
        vcpu: usize,
        paging: &crate::Pg32ProbeConfig,
        request: crate::Pg32AccessRequest,
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> TrackedAccessReport {
        let Self {
            memory,
            devices,
            mmio,
            ..
        } = self;
        let devices = exclusive_devices(devices);
        let regions = memory.region_specs().collect::<Vec<_>>();
        crate::page_walk::pg32::access_pg32_with_mmio(
            memory,
            paging,
            request,
            committed_write,
            |address, width, operation| {
                dispatch_tracked_mmio(&regions, mmio, devices, vcpu, address, width, operation)
            },
        )
    }

    pub(crate) fn access_tracked_ram(
        &self,
        paging: &Long64ProbeConfig,
        request: TrackedAccessRequest,
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> TrackedAccessReport {
        crate::page_walk::access_long64_4k_ram(&self.memory, paging, request, committed_write)
    }

    pub(crate) fn access_tracked_2m_ram(
        &self,
        paging: &Long64ProbeConfig,
        request: TrackedAccessRequest,
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> TrackedAccessReport {
        crate::page_walk::access_long64_2m_ram(&self.memory, paging, request, committed_write)
    }

    pub(crate) fn probe_tracked_ram_access(
        &self,
        paging: &Long64ProbeConfig,
        address: GuestLinearAddress,
        access: AccessKind,
        context: AccessContext,
    ) -> ProbeReport {
        crate::page_walk::probe_long64_4k(&self.memory, paging, address, access, context)
    }

    /// S6-32: sets the execution mode of the machine memory (the one switch; see
    /// [`crate::ExecutionMode`]). `&mut self`: only while nothing shares the memory.
    pub fn set_execution_mode(&mut self, mode: crate::ExecutionMode) {
        self.memory.set_execution_mode(mode);
    }

    /// S6-32: the execution mode of the machine memory.
    pub fn execution_mode(&self) -> crate::ExecutionMode {
        self.memory.execution_mode()
    }

    /// The physical memory the locked operations of an adapter run on (S6-31): a shared reference,
    /// like the `Ram` forms of the adapter hold.
    pub(crate) fn physical_memory(&self) -> &PhysicalMemory {
        &self.memory
    }

    /// S6-41d: the machine memory as the shared reference the inline atomic helper of `od-host`
    /// works on (`locate_paged_atomic_operand` translates and checks with the walks of the
    /// interpreter's locked operation, hands out the host pointer of a RAM operand, and the
    /// generated host atomic operation runs on it). It is the same memory the locked operations
    /// of the adapters use ([`Self::physical_memory`]); the shared reference does not let the
    /// caller change the memory map or the execution mode (`set_execution_mode` needs `&mut`).
    pub fn atomic_inline_memory(&self) -> &PhysicalMemory {
        &self.memory
    }

    pub(crate) fn tracked_physical_range_is_ram(&self, range: GuestPhysicalRange) -> bool {
        self.memory
            .locate_region_kind(range.start(), range.len())
            .is_ok_and(|kind| kind == MemoryRegionKind::Ram)
    }

    pub(crate) fn read_tracked_bounded_ram_read(
        &self,
        paging: &Long64ProbeConfig,
        request: TrackedAccessRequest,
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> Result<u64, crate::TrackedAccessError> {
        crate::page_walk::read_long64_4k_bounded_ram_read(
            &self.memory,
            paging,
            request,
            committed_write,
        )
    }

    pub(crate) fn read_tracked_bounded_ram_read_with_fault_trace(
        &self,
        paging: &Long64ProbeConfig,
        request: TrackedAccessRequest,
        committed_write: impl FnMut(GuestPhysicalRange),
        fault_trace: &mut Option<crate::WalkTrace>,
    ) -> Result<u64, crate::TrackedAccessError> {
        crate::page_walk::read_long64_4k_bounded_ram_read_with_fault_trace(
            &self.memory,
            paging,
            request,
            committed_write,
            fault_trace,
        )
    }

    pub(crate) fn memory_kind(
        &self,
        address: GuestPhysicalAddress,
        len: u64,
    ) -> Result<MemoryRegionKind, BusAccessError> {
        self.memory
            .locate_region_kind(address, len)
            .map_err(map_memory_error)
    }

    /// Cross-checks the display device's advertised `FB_BASE`/`FB_SIZE` and
    /// register window against the map (see `DISPLAY_DEVICE.md`, 7.3).
    fn check_display_binding(
        map: &MachineMemoryMap,
        devices: &[Box<dyn BusDevice>],
        mmio: &[MmioBinding],
    ) -> Result<(), BusBuildError> {
        let mut displays = devices
            .iter()
            .enumerate()
            .filter_map(|(index, device)| device.framebuffer_binding().map(|b| (index, b)));
        let display = displays.next();
        if displays.next().is_some() {
            return Err(BusBuildError::Display(DisplayBindingError::MultipleDevices));
        }
        let secondary =
            display.and_then(|(index, _)| devices[index].secondary_framebuffer_binding());
        let regions: Vec<&MemoryRegionSpec> = map
            .regions()
            .iter()
            .filter(|region| region.kind == MemoryRegionKind::Framebuffer)
            .collect();
        let (device_index, primary) = match (display, regions.is_empty()) {
            (None, true) => return Ok(()),
            (None, false) => {
                return Err(BusBuildError::Display(DisplayBindingError::MissingDevice));
            }
            (Some(_), true) => {
                return Err(BusBuildError::Display(
                    DisplayBindingError::MissingFramebufferRegion,
                ));
            }
            (Some((index, binding)), false) => (index, binding),
        };
        // Every advertised window must match exactly one region of the map, and
        // every region must be advertised (DISPLAY_DEVICE.md 16.3).
        let advertised: Vec<FramebufferBinding> =
            std::iter::once(primary).chain(secondary).collect();
        if advertised.len() != regions.len() {
            let unmatched = regions
                .iter()
                .find(|region| {
                    !advertised
                        .iter()
                        .any(|b| region.range.start() == b.base && region.range.len() == b.size)
                })
                .copied();
            if let Some(region) = unmatched {
                return Err(BusBuildError::Display(
                    DisplayBindingError::UnboundFramebufferRegion {
                        region_start: region.range.start(),
                        region_len: region.range.len(),
                    },
                ));
            }
        }
        for binding in &advertised {
            let region = regions.iter().find(|region| {
                region.range.start() == binding.base && region.range.len() == binding.size
            });
            if region.is_none() {
                let nearest = regions[0];
                return Err(BusBuildError::Display(
                    DisplayBindingError::FramebufferMismatch {
                        region_start: nearest.range.start(),
                        region_len: nearest.range.len(),
                        device_base: binding.base,
                        device_size: binding.size,
                    },
                ));
            }
        }
        let mut windows = mmio
            .iter()
            .filter(|binding| binding.device_index == device_index);
        let window = windows.next().ok_or(BusBuildError::Display(
            DisplayBindingError::RegisterWindowUnbound,
        ))?;
        if windows.next().is_some() {
            return Err(BusBuildError::Display(
                DisplayBindingError::MultipleRegisterWindows,
            ));
        }
        // Every MMIO binding was already validated to name an `Mmio` region.
        let window_len = map
            .regions()
            .iter()
            .find(|region| region.range.start() == window.region_start)
            .map(|region| region.range.len())
            .unwrap_or(0);
        if window_len != DISPLAY_REGISTER_WINDOW_BYTES {
            return Err(BusBuildError::Display(
                DisplayBindingError::RegisterWindowLength { actual: window_len },
            ));
        }
        Ok(())
    }

    /// Diagnostic read of RAM, ROM or framebuffer bytes (S8-10): never touches an MMIO device, so
    /// it changes no device state. An MMIO or reserved range is an error.
    pub(crate) fn read_physical_bytes_readonly(
        &self,
        address: GuestPhysicalAddress,
        output: &mut [u8],
    ) -> Result<(), MemoryAccessError> {
        self.memory.load_bytes(address, output)
    }

    /// Copies frame bytes out of the `Framebuffer` region at `base`. Used only
    /// by the display handle between guest quanta.
    pub(crate) fn read_framebuffer_bytes(
        &self,
        base: GuestPhysicalAddress,
        output: &mut [u8],
    ) -> Result<(), MemoryAccessError> {
        self.memory.read_framebuffer_bytes(base, output)
    }

    pub fn new(
        map: MachineMemoryMap,
        ram_limits: RamLimits,
        devices: Vec<Box<dyn BusDevice>>,
        mut mmio: Vec<MmioBinding>,
        mut ports: Vec<PortRegionBinding>,
    ) -> Result<Self, BusBuildError> {
        for binding in &mmio {
            if binding.device_index >= devices.len() {
                return Err(BusBuildError::InvalidDeviceIndex(binding.device_index));
            }
        }
        for binding in &ports {
            if binding.device_index >= devices.len() {
                return Err(BusBuildError::InvalidDeviceIndex(binding.device_index));
            }
        }
        mmio.sort_by_key(|binding| binding.region_start);
        for pair in mmio.windows(2) {
            if pair[0].region_start == pair[1].region_start {
                return Err(BusBuildError::DuplicateMmioBinding(pair[0].region_start));
            }
        }
        for binding in &mmio {
            if !map.regions().iter().any(|region| {
                region.kind == MemoryRegionKind::Mmio
                    && region.range.start() == binding.region_start
            }) {
                return Err(BusBuildError::InvalidMmioBinding(binding.region_start));
            }
        }
        for region in map
            .regions()
            .iter()
            .filter(|region| region.kind == MemoryRegionKind::Mmio)
        {
            if !mmio
                .iter()
                .any(|binding| binding.region_start == region.range.start())
            {
                return Err(BusBuildError::MissingMmioBinding(region.range.start()));
            }
        }
        Self::check_display_binding(&map, &devices, &mmio)?;
        ports.sort_by_key(|binding| binding.range.start);
        for pair in ports.windows(2) {
            if pair[0].range.end() > u32::from(pair[1].range.start.0) {
                return Err(BusBuildError::OverlappingPortRanges(
                    pair[0].range,
                    pair[1].range,
                ));
            }
        }
        let mmio_lens = mmio
            .iter()
            .map(|binding| {
                map.regions()
                    .iter()
                    .find(|region| {
                        region.kind == MemoryRegionKind::Mmio
                            && region.range.start() == binding.region_start
                    })
                    .map_or(0, |region| region.range.len())
            })
            .collect();
        let memory = PhysicalMemory::new(&map, ram_limits).map_err(BusBuildError::Memory)?;
        Ok(Self {
            memory,
            devices: MeasuredMutex::new(devices),
            mmio,
            mmio_lens,
            set_step_windows: &SET_STEP_WINDOWS,
            firmware_set_windows: None,
            ports,
            device_accesses: Arc::new(AtomicU64::new(0)),
            device_errors: None,
        })
    }

    /// The device and the device-relative access of an MMIO operand: the region that wholly holds
    /// `width` bytes at `address`, and the device bound to it.
    fn mmio_target(
        &self,
        address: GuestPhysicalAddress,
        width: AccessWidth,
    ) -> Result<(usize, DeviceAccess), BusAccessError> {
        let len = u64::from(width.bytes());
        let region = self
            .memory
            .region_specs()
            .find(|region| {
                region.kind == MemoryRegionKind::Mmio
                    && region.range.start() <= address
                    && u128::from(address.get()) + u128::from(len) <= region.range.end_exclusive()
            })
            .ok_or(BusAccessError::CrossesRegion)?;
        let device_index = self
            .mmio
            .iter()
            .find(|binding| binding.region_start == region.range.start())
            .ok_or(BusAccessError::UnmappedPhysical(address))?
            .device_index;
        Ok((
            device_index,
            DeviceAccess {
                space: IoSpace::Mmio,
                offset: address.get() - region.range.start().get(),
                width,
            },
        ))
    }

    /// S6-33: the one device lock. Every access of a thread that holds only `&MachineBus` takes
    /// it, so device accesses of different vCPU threads are serialized; RAM is not behind it
    /// ([`MachineBus::physical_memory`] needs no lock). The device lock is the first lock of the
    /// order in MACHINE_CONTRACT.md: a device reached through it takes its own lock (a Local APIC
    /// the APIC lock) and never calls back into the bus. A panic in a device does not poison it
    /// for good: the table is used as the panicking thread left it, as for every device lock.
    pub(crate) fn lock_devices(&self) -> MutexGuard<'_, Vec<Box<dyn BusDevice>>> {
        self.devices.lock().unwrap_or_else(PoisonError::into_inner)
    }

    #[cfg(feature = "lock-wait-metrics")]
    pub(crate) fn device_lock_metrics(&self) -> crate::MutexAcquisitionMetrics {
        self.devices.metrics()
    }

    #[cfg(feature = "lock-wait-metrics")]
    pub(crate) fn reset_device_lock_metrics(&mut self) {
        self.devices.reset_metrics();
    }

    /// Port read by a thread that holds only `&MachineBus` (takes the device lock).
    pub fn read_port_shared(
        &self,
        port: PortAddress,
        width: AccessWidth,
    ) -> Result<u32, BusAccessError> {
        read_port_from(&mut self.lock_devices(), &self.ports, port, width)
    }

    /// Port write by a thread that holds only `&MachineBus` (takes the device lock).
    pub fn write_port_shared(
        &self,
        port: PortAddress,
        width: AccessWidth,
        value: u32,
    ) -> Result<(), BusAccessError> {
        write_port_to(&mut self.lock_devices(), &self.ports, port, width, value)
    }

    /// MMIO read of vCPU `vcpu` by a thread that holds only `&MachineBus` (takes the device
    /// lock); a Local APIC access is that processor's.
    ///
    /// Only addresses inside an MMIO region are served: an address of RAM or outside every MMIO
    /// region gives `CrossesRegion` here, where `read_physical` would return the RAM bytes or
    /// `UnmappedPhysical`. This shared path does not consult `apic_mmio_windows` (S6-34 must route
    /// the Local APIC window of the executing vCPU itself).
    pub fn read_mmio_shared(
        &self,
        vcpu: usize,
        address: GuestPhysicalAddress,
        width: AccessWidth,
    ) -> Result<u64, BusAccessError> {
        let (device_index, access) = self.mmio_target(address, width)?;
        self.lock_devices()[device_index]
            .read_from_cpu(vcpu, access)
            .map(|v| v & width.mask())
            .map_err(|source| BusAccessError::Device {
                device_index,
                source,
            })
    }

    /// MMIO write of vCPU `vcpu` by a thread that holds only `&MachineBus`; see
    /// [`MachineBus::read_mmio_shared`] (same `CrossesRegion` result for RAM or non-MMIO
    /// addresses, same absence of `apic_mmio_windows` routing).
    pub fn write_mmio_shared(
        &self,
        vcpu: usize,
        address: GuestPhysicalAddress,
        width: AccessWidth,
        value: u64,
    ) -> Result<(), BusAccessError> {
        let (device_index, access) = self.mmio_target(address, width)?;
        self.lock_devices()[device_index]
            .write_from_cpu(vcpu, access, value & width.mask())
            .map_err(|source| BusAccessError::Device {
                device_index,
                source,
            })
    }

    pub fn read_physical(
        &mut self,
        address: GuestPhysicalAddress,
        width: AccessWidth,
    ) -> Result<u64, BusAccessError> {
        let len = u64::from(width.bytes());
        match self
            .memory
            .locate_region_kind(address, len)
            .map_err(map_memory_error)?
        {
            MemoryRegionKind::Ram => {
                read_ram(&self.memory, address, width).map_err(BusAccessError::Memory)
            }
            MemoryRegionKind::Mmio => {
                let (device_index, access) = self.mmio_target(address, width)?;
                exclusive_devices(&mut self.devices)[device_index]
                    .read(access)
                    .map(|v| v & width.mask())
                    .map_err(|source| BusAccessError::Device {
                        device_index,
                        source,
                    })
            }
            // Pixel memory is read as data like RAM; ROM bytes are read-only data.
            MemoryRegionKind::Framebuffer | MemoryRegionKind::Rom => {
                read_ram(&self.memory, address, width).map_err(BusAccessError::Memory)
            }
            MemoryRegionKind::Reserved => {
                Err(BusAccessError::Memory(MemoryAccessError::UnreadableRegion(
                    self.memory.locate_region_kind(address, len).unwrap(),
                )))
            }
        }
    }

    pub fn write_physical(
        &mut self,
        address: GuestPhysicalAddress,
        width: AccessWidth,
        value: u64,
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> Result<(), BusAccessError> {
        let len = u64::from(width.bytes());
        match self
            .memory
            .locate_region_kind(address, len)
            .map_err(map_memory_error)?
        {
            MemoryRegionKind::Ram => {
                write_ram(&mut self.memory, address, width, value, committed_write)
                    .map_err(BusAccessError::Memory)
            }
            MemoryRegionKind::Mmio => {
                let (device_index, access) = self.mmio_target(address, width)?;
                exclusive_devices(&mut self.devices)[device_index]
                    .write(access, value & width.mask())
                    .map_err(|source| BusAccessError::Device {
                        device_index,
                        source,
                    })
            }
            // Pixel writes never reach the committed-write callback: nothing
            // depends on framebuffer bytes (no fetch, no page tables).
            MemoryRegionKind::Framebuffer => {
                write_ram(&mut self.memory, address, width, value, |_| {})
                    .map_err(BusAccessError::Memory)
            }
            MemoryRegionKind::Rom | MemoryRegionKind::Reserved => {
                Err(BusAccessError::Memory(MemoryAccessError::ReadOnlyRegion(
                    self.memory.locate_region_kind(address, len).unwrap(),
                )))
            }
        }
    }

    pub fn read_port(
        &mut self,
        port: PortAddress,
        width: AccessWidth,
    ) -> Result<u32, BusAccessError> {
        read_port_from(
            exclusive_devices(&mut self.devices),
            &self.ports,
            port,
            width,
        )
    }

    pub fn write_port(
        &mut self,
        port: PortAddress,
        width: AccessWidth,
        value: u32,
    ) -> Result<(), BusAccessError> {
        write_port_to(
            exclusive_devices(&mut self.devices),
            &self.ports,
            port,
            width,
            value,
        )
    }

    pub(crate) fn read_flat_bytes(
        &mut self,
        address: GuestPhysicalAddress,
        output: &mut [u8],
    ) -> Result<(), BusAccessError> {
        let len = u64::try_from(output.len()).map_err(|_| BusAccessError::AddressOverflow)?;
        let kind = self
            .memory
            .locate_region_kind(address, len)
            .map_err(map_memory_error)?;
        match kind {
            MemoryRegionKind::Ram | MemoryRegionKind::Framebuffer | MemoryRegionKind::Rom => self
                .memory
                .load_bytes(address, output)
                .map_err(BusAccessError::Memory),
            MemoryRegionKind::Mmio => {
                let width = access_width(output.len()).ok_or(BusAccessError::UnsupportedWidth)?;
                let value = self.read_physical(address, width)?;
                output.copy_from_slice(&value.to_le_bytes()[..output.len()]);
                Ok(())
            }
            kind @ MemoryRegionKind::Reserved => Err(BusAccessError::Memory(
                MemoryAccessError::UnreadableRegion(kind),
            )),
        }
    }

    /// Machine-side one-time fill of the ROM region (firmware image at power
    /// on). Guest-visible writes keep returning `ReadOnlyRegion(Rom)`.
    pub(crate) fn load_rom_bytes(
        &mut self,
        address: GuestPhysicalAddress,
        input: &[u8],
    ) -> Result<(), BusAccessError> {
        self.memory
            .load_rom_bytes(address, input)
            .map_err(BusAccessError::Memory)
    }

    pub(crate) fn write_flat_bytes(
        &mut self,
        address: GuestPhysicalAddress,
        input: &[u8],
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> Result<(), BusAccessError> {
        let len = u64::try_from(input.len()).map_err(|_| BusAccessError::AddressOverflow)?;
        let kind = self
            .memory
            .locate_region_kind(address, len)
            .map_err(map_memory_error)?;
        match kind {
            MemoryRegionKind::Ram => self
                .memory
                .store_bytes(address, input, committed_write)
                .map_err(BusAccessError::Memory),
            // `PhysicalMemory::store_bytes` skips the callback for Framebuffer.
            MemoryRegionKind::Framebuffer => self
                .memory
                .store_bytes(address, input, committed_write)
                .map_err(BusAccessError::Memory),
            MemoryRegionKind::Mmio => {
                let width = access_width(input.len()).ok_or(BusAccessError::UnsupportedWidth)?;
                let mut bytes = [0; 8];
                bytes[..input.len()].copy_from_slice(input);
                self.write_physical(address, width, u64::from_le_bytes(bytes), committed_write)
            }
            kind @ (MemoryRegionKind::Rom | MemoryRegionKind::Reserved) => Err(
                BusAccessError::Memory(MemoryAccessError::ReadOnlyRegion(kind)),
            ),
        }
    }
}

impl PortIo for MachineBusPortIo<'_> {
    type Error = BusAccessError;

    fn read_u8(&mut self, port: u16) -> Result<u8, Self::Error> {
        let sample_time = self.rtc_time(port);
        read_port_from_with(
            self.devices,
            self.ports,
            PortAddress::new(port),
            AccessWidth::Byte,
            |device, access| match sample_time {
                Some(sample) => device.read_at_time(access, &sample),
                None => device.read(access),
            },
        )
        .map(|value| value as u8)
    }

    fn write_u8(&mut self, port: u16, value: u8) -> Result<(), Self::Error> {
        let sample_time = self.rtc_time(port);
        write_port_to_with(
            self.devices,
            self.ports,
            PortAddress::new(port),
            AccessWidth::Byte,
            u32::from(value),
            |device, access, value| match sample_time {
                Some(sample) => device.write_at_time(access, value, &sample),
                None => device.write(access, value),
            },
        )
    }

    fn read_u16(&mut self, port: u16) -> Result<u16, Self::Error> {
        read_port_from(
            self.devices,
            self.ports,
            PortAddress::new(port),
            AccessWidth::Word,
        )
        .map(|value| value as u16)
    }

    fn write_u16(&mut self, port: u16, value: u16) -> Result<(), Self::Error> {
        write_port_to(
            self.devices,
            self.ports,
            PortAddress::new(port),
            AccessWidth::Word,
            u32::from(value),
        )
    }

    fn read_u32(&mut self, port: u16) -> Result<u32, Self::Error> {
        read_port_from(
            self.devices,
            self.ports,
            PortAddress::new(port),
            AccessWidth::Dword,
        )
    }

    fn write_u32(&mut self, port: u16, value: u32) -> Result<(), Self::Error> {
        write_port_to(
            self.devices,
            self.ports,
            PortAddress::new(port),
            AccessWidth::Dword,
            value,
        )
    }
}

/// The device dispatch of a tracked MMIO operand access by vCPU `vcpu` (shared by the 4-level and the
/// PAE paths of [`MachineBus`]): the operand must lie wholly inside one `Mmio` region that has a
/// device binding; it reaches the device as that vCPU's access of the operand width.
fn dispatch_tracked_mmio(
    regions: &[MemoryRegionSpec],
    mmio: &[MmioBinding],
    devices: &mut [Box<dyn BusDevice>],
    vcpu: usize,
    address: GuestPhysicalAddress,
    width: u8,
    operation: RamAccessOperation,
) -> Result<u64, crate::page_walk::MmioDispatchError> {
    let width = match width {
        8 => AccessWidth::Qword,
        4 => AccessWidth::Dword,
        2 => AccessWidth::Word,
        1 => AccessWidth::Byte,
        _ => {
            return Err(crate::page_walk::MmioDispatchError::Backend(format!(
                "unsupported MMIO width {width}"
            )));
        }
    };
    let len = u64::from(width.bytes());
    let region = regions
        .iter()
        .find(|region| {
            region.kind == MemoryRegionKind::Mmio
                && region.range.start() <= address
                && u128::from(address.get()) + u128::from(len) <= region.range.end_exclusive()
        })
        .ok_or_else(|| {
            crate::page_walk::MmioDispatchError::Backend(
                "MMIO range is not wholly inside one device region".to_owned(),
            )
        })?;
    let binding = mmio
        .iter()
        .find(|binding| binding.region_start == region.range.start())
        .ok_or_else(|| {
            crate::page_walk::MmioDispatchError::Backend(format!(
                "unmapped MMIO address {address:?}"
            ))
        })?;
    let access = DeviceAccess {
        space: IoSpace::Mmio,
        offset: address.get() - region.range.start().get(),
        width,
    };
    match operation {
        RamAccessOperation::Read { .. } => devices[binding.device_index]
            .read_from_cpu(vcpu, access)
            .map(|value| value & width.mask())
            .map_err(|error| {
                crate::page_walk::MmioDispatchError::Backend(format!(
                    "MMIO device {} read failed: {error:?}",
                    binding.device_index
                ))
            }),
        RamAccessOperation::Write { value, .. } => devices[binding.device_index]
            .write_from_cpu(vcpu, access, value & width.mask())
            .map(|()| 0)
            .map_err(|error| {
                crate::page_walk::MmioDispatchError::Backend(format!(
                    "MMIO device {} write failed: {error:?}",
                    binding.device_index
                ))
            }),
        RamAccessOperation::Fetch => Err(crate::page_walk::MmioDispatchError::Backend(
            "instruction fetch from MMIO is unsupported".to_owned(),
        )),
        RamAccessOperation::ReadPseudoDescriptor => {
            Err(crate::page_walk::MmioDispatchError::Backend(
                "pseudo-descriptor reads are RAM-only".to_owned(),
            ))
        }
        RamAccessOperation::ReadForWrite { .. } => {
            Err(crate::page_walk::MmioDispatchError::Backend(
                "write-intent reads are RAM-only".to_owned(),
            ))
        }
        // S6-31: refused by the walker before the dispatch.
        RamAccessOperation::AtomicTarget { .. } => {
            Err(crate::page_walk::MmioDispatchError::Backend(
                "locked operations are RAM-only".to_owned(),
            ))
        }
    }
}

/// The device table of a bus owned exclusively: `&mut` proves no other thread holds the lock, so
/// no lock is taken (the interleaved paths pay nothing for the device lock of S6-33).
fn exclusive_devices(
    devices: &mut MeasuredMutex<Vec<Box<dyn BusDevice>>>,
) -> &mut Vec<Box<dyn BusDevice>> {
    devices.get_mut().unwrap_or_else(PoisonError::into_inner)
}

fn read_port_from(
    devices: &mut [Box<dyn BusDevice>],
    ports: &[PortRegionBinding],
    port: PortAddress,
    width: AccessWidth,
) -> Result<u32, BusAccessError> {
    read_port_from_with(devices, ports, port, width, |device, access| {
        device.read(access)
    })
}

fn read_port_from_with(
    devices: &mut [Box<dyn BusDevice>],
    ports: &[PortRegionBinding],
    port: PortAddress,
    width: AccessWidth,
    read: impl FnOnce(&mut dyn BusDevice, DeviceAccess) -> Result<u64, DeviceError>,
) -> Result<u32, BusAccessError> {
    if width == AccessWidth::Qword {
        return Err(BusAccessError::UnsupportedWidth);
    }
    let len = u32::from(width.bytes());
    let binding = ports
        .iter()
        .find(|binding| binding.range.contains(port, len))
        .ok_or_else(|| {
            if ports.iter().any(|b| {
                u32::from(port.0) >= u32::from(b.range.start.0) && u32::from(port.0) < b.range.end()
            }) {
                BusAccessError::CrossesRegion
            } else {
                BusAccessError::UnmappedPort(port)
            }
        })?;
    let access = DeviceAccess {
        space: IoSpace::Pio,
        offset: u64::from(port.0 - binding.range.start.0),
        width,
    };
    read(devices[binding.device_index].as_mut(), access)
        .map(|v| (v & width.mask()) as u32)
        .map_err(|source| BusAccessError::Device {
            device_index: binding.device_index,
            source,
        })
}

fn write_port_to(
    devices: &mut [Box<dyn BusDevice>],
    ports: &[PortRegionBinding],
    port: PortAddress,
    width: AccessWidth,
    value: u32,
) -> Result<(), BusAccessError> {
    write_port_to_with(
        devices,
        ports,
        port,
        width,
        value,
        |device, access, value| device.write(access, value),
    )
}

fn write_port_to_with(
    devices: &mut [Box<dyn BusDevice>],
    ports: &[PortRegionBinding],
    port: PortAddress,
    width: AccessWidth,
    value: u32,
    write: impl FnOnce(&mut dyn BusDevice, DeviceAccess, u64) -> Result<(), DeviceError>,
) -> Result<(), BusAccessError> {
    if width == AccessWidth::Qword {
        return Err(BusAccessError::UnsupportedWidth);
    }
    let len = u32::from(width.bytes());
    let binding = ports
        .iter()
        .find(|binding| binding.range.contains(port, len))
        .ok_or_else(|| {
            if ports.iter().any(|b| {
                u32::from(port.0) >= u32::from(b.range.start.0) && u32::from(port.0) < b.range.end()
            }) {
                BusAccessError::CrossesRegion
            } else {
                BusAccessError::UnmappedPort(port)
            }
        })?;
    let access = DeviceAccess {
        space: IoSpace::Pio,
        offset: u64::from(port.0 - binding.range.start.0),
        width,
    };
    write(
        devices[binding.device_index].as_mut(),
        access,
        u64::from(value) & width.mask(),
    )
    .map_err(|source| BusAccessError::Device {
        device_index: binding.device_index,
        source,
    })
}

fn access_width(len: usize) -> Option<AccessWidth> {
    match len {
        1 => Some(AccessWidth::Byte),
        2 => Some(AccessWidth::Word),
        4 => Some(AccessWidth::Dword),
        8 => Some(AccessWidth::Qword),
        _ => None,
    }
}

fn map_memory_error(error: MemoryAccessError) -> BusAccessError {
    match error {
        MemoryAccessError::Unmapped(address) => BusAccessError::UnmappedPhysical(address),
        MemoryAccessError::AccessCrossesRegion(_) => BusAccessError::CrossesRegion,
        MemoryAccessError::AddressOverflow => BusAccessError::AddressOverflow,
        error => BusAccessError::Memory(error),
    }
}

fn read_ram(
    memory: &PhysicalMemory,
    address: GuestPhysicalAddress,
    width: AccessWidth,
) -> Result<u64, MemoryAccessError> {
    Ok(match width {
        AccessWidth::Byte => u64::from(memory.load_u8(address)?),
        AccessWidth::Word => u64::from(memory.load_u16(address)?),
        AccessWidth::Dword => u64::from(memory.load_u32(address)?),
        AccessWidth::Qword => memory.load_u64(address)?,
    })
}
fn write_ram(
    memory: &PhysicalMemory,
    address: GuestPhysicalAddress,
    width: AccessWidth,
    value: u64,
    callback: impl FnMut(GuestPhysicalRange),
) -> Result<(), MemoryAccessError> {
    match width {
        AccessWidth::Byte => memory.store_u8(address, value as u8, callback),
        AccessWidth::Word => memory.store_u16(address, value as u16, callback),
        AccessWidth::Dword => memory.store_u32(address, value as u32, callback),
        AccessWidth::Qword => memory.store_u64(address, value, callback),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct DeviceState {
        calls: usize,
        value: u64,
    }

    struct CountingDevice(Arc<Mutex<DeviceState>>);

    impl BusDevice for CountingDevice {
        fn read(&mut self, access: DeviceAccess) -> Result<u64, DeviceError> {
            let mut state = self.0.lock().unwrap();
            state.calls += 1;
            if access.offset >= 16 {
                return Err(DeviceError::UnknownOffset);
            }
            if access.offset % u64::from(access.width.bytes()) != 0 {
                return Err(DeviceError::Misaligned);
            }
            Ok(state.value)
        }

        fn write(&mut self, access: DeviceAccess, value: u64) -> Result<(), DeviceError> {
            let mut state = self.0.lock().unwrap();
            state.calls += 1;
            if access.offset >= 16 {
                return Err(DeviceError::UnknownOffset);
            }
            if access.offset % u64::from(access.width.bytes()) != 0 {
                return Err(DeviceError::Misaligned);
            }
            if access.offset == 4 {
                return Err(DeviceError::ReadOnly);
            }
            state.value = value;
            Ok(())
        }
    }

    fn range(start: u64, len: u64) -> GuestPhysicalRange {
        GuestPhysicalRange::new(GuestPhysicalAddress::new(start), len).unwrap()
    }

    fn setup(with_adjacent_ram: bool) -> (MachineBus, Arc<Mutex<DeviceState>>) {
        let state = Arc::new(Mutex::new(DeviceState::default()));
        let mut regions = vec![
            crate::MemoryRegionSpec::new(range(0x1000, 0x100), MemoryRegionKind::Ram),
            crate::MemoryRegionSpec::new(range(0x2000, 0x100), MemoryRegionKind::Mmio),
        ];
        if with_adjacent_ram {
            regions.push(crate::MemoryRegionSpec::new(
                range(0x2100, 0x10),
                MemoryRegionKind::Ram,
            ));
        }
        let map = MachineMemoryMap::new(36, regions).unwrap();
        let bus = MachineBus::new(
            map,
            RamLimits::new(0x200),
            vec![Box::new(CountingDevice(Arc::clone(&state)))],
            vec![MmioBinding {
                region_start: GuestPhysicalAddress::new(0x2000),
                device_index: 0,
            }],
            vec![PortRegionBinding {
                range: PortRange::new(PortAddress::new(0x20), 16).unwrap(),
                device_index: 0,
            }],
        )
        .unwrap();
        (bus, state)
    }

    #[test]
    fn shared_and_exclusive_set_views_keep_apic_and_nvme_in_either_order() {
        for apic_first in [false, true] {
            let (mut bus, _) = setup(false);
            if apic_first {
                bus.enable_apic_mmio_windows();
                bus.enable_nvme_pci_mmio_window();
            } else {
                bus.enable_nvme_pci_mmio_window();
                bus.enable_apic_mmio_windows();
            }
            let shared_windows = bus.split_for_set_step_shared(0).1.windows().to_vec();
            let exclusive_windows = bus.split_for_set_step_of_vcpu(0).1.windows().to_vec();
            assert_eq!(shared_windows, SET_STEP_APIC_NVME_WINDOWS);
            assert_eq!(exclusive_windows, shared_windows);
        }
    }

    #[test]
    fn firmware_set_windows_add_to_the_set_windows_in_both_views_and_only_once() {
        let (mut bus, _) = setup(false);
        // Without the call a board serves exactly what it served before (S7-31c).
        assert_eq!(
            bus.split_for_set_step_of_vcpu(0).1.windows(),
            SET_STEP_WINDOWS
        );
        let nvram = (0xff00_0000, 0x4_1000);
        let tpm = (0xfed4_0000, 0x5000);
        bus.enable_apic_mmio_windows();
        bus.serve_firmware_set_windows(&[nvram, tpm, nvram]);
        let mut expected = SET_STEP_APIC_WINDOWS.to_vec();
        expected.extend([nvram, tpm]);
        assert_eq!(bus.split_for_set_step_shared(0).1.windows(), expected);
        assert_eq!(bus.split_for_set_step_of_vcpu(0).1.windows(), expected);
    }

    fn diagnostic_page_bus(map_second_page: bool) -> (MachineBus, Long64ProbeConfig) {
        let map = MachineMemoryMap::new(
            36,
            vec![crate::MemoryRegionSpec::new(
                range(0, 0x10_000),
                MemoryRegionKind::Ram,
            )],
        )
        .unwrap();
        let mut bus =
            MachineBus::new(map, RamLimits::new(0x10_000), vec![], vec![], vec![]).unwrap();
        for (address, value) in [
            (0x1000, 0x2007),
            (0x2000, 0x3007),
            (0x3000, 0x4007),
            (0x4020, 0x7007),
            (0x4028, if map_second_page { 0x9007 } else { 0 }),
        ] {
            bus.write_physical(
                GuestPhysicalAddress::new(address),
                AccessWidth::Qword,
                value,
                |_| {},
            )
            .unwrap();
        }
        bus.patch_physical_ram(GuestPhysicalAddress::new(0x7ff8), &[0x11; 8])
            .unwrap();
        bus.patch_physical_ram(GuestPhysicalAddress::new(0x8000), &[0x22; 8])
            .unwrap();
        bus.patch_physical_ram(GuestPhysicalAddress::new(0x9000), &[0x33; 8])
            .unwrap();
        let paging = Long64ProbeConfig::new(
            0x8001_0033,
            0x1000,
            1 << 5,
            (1 << 8) | (1 << 10) | (1 << 11),
        )
        .unwrap();
        (bus, paging)
    }

    #[test]
    fn diagnostic_ram_read_translates_each_virtual_page_fragment() {
        let (mut bus, paging) = diagnostic_page_bus(true);
        let pte_before = bus
            .read_physical(GuestPhysicalAddress::new(0x4020), AccessWidth::Qword)
            .unwrap();
        let next_pte_before = bus
            .read_physical(GuestPhysicalAddress::new(0x4028), AccessWidth::Qword)
            .unwrap();
        let mut output = [0; 16];
        bus.read_virtual_ram_diagnostic(
            &paging,
            0x1234,
            GuestLinearAddress::new(0x4ff8),
            AccessContext::default(),
            &mut output,
        )
        .unwrap();
        assert_eq!(
            output,
            [0x11; 8].into_iter().chain([0x33; 8]).collect::<Vec<_>>()[..]
        );
        assert_ne!(&output[8..], &[0x22; 8]);
        assert_eq!(
            bus.read_physical(GuestPhysicalAddress::new(0x4020), AccessWidth::Qword)
                .unwrap(),
            pte_before
        );
        assert_eq!(
            bus.read_physical(GuestPhysicalAddress::new(0x4028), AccessWidth::Qword)
                .unwrap(),
            next_pte_before
        );
    }

    #[test]
    fn diagnostic_ram_read_refuses_unmapped_second_virtual_page() {
        let (mut bus, paging) = diagnostic_page_bus(false);
        let mut output = [0xaa; 16];
        let error = bus
            .read_virtual_ram_diagnostic(
                &paging,
                0x1234,
                GuestLinearAddress::new(0x4ff8),
                AccessContext::default(),
                &mut output,
            )
            .unwrap_err();
        assert!(error.contains("read-only translation failed"));
        assert_eq!(&output[8..], &[0xaa; 8]);
    }

    #[test]
    fn tracked_code_snapshot_two_pages_captures_each_page_with_its_own_generation() {
        let ram = range(0, 0x9000);
        let map = MachineMemoryMap::new(
            36,
            vec![crate::MemoryRegionSpec::new(ram, MemoryRegionKind::Ram)],
        )
        .unwrap();
        let mut bus = MachineBus::new(map, RamLimits::new(0x9000), vec![], vec![], vec![]).unwrap();
        // Linear page 0x4000 -> physical 0x7000, linear 0x5000 -> physical
        // 0x5000 (not adjacent), linear 0x6000 -> not present.
        for (address, value) in [
            (0x1000, 0x2007),
            (0x2000, 0x3007),
            (0x3000, 0x4007),
            (0x4020, 0x7007),
            (0x4028, 0x5007),
            (0x4030, 0x6006),
        ] {
            bus.write_physical(
                GuestPhysicalAddress::new(address),
                AccessWidth::Qword,
                value,
                |_| {},
            )
            .unwrap();
        }
        let code = [0x48, 0xb8, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
        bus.patch_physical_ram(GuestPhysicalAddress::new(0x7ffd), &code[..3])
            .unwrap();
        bus.patch_physical_ram(GuestPhysicalAddress::new(0x5000), &code[3..])
            .unwrap();
        let paging = Long64ProbeConfig::new(
            0x8001_0033,
            0x1000,
            1 << 5,
            (1 << 8) | (1 << 10) | (1 << 11),
        )
        .unwrap();
        let context = AccessContext::default();
        let mut writes = Vec::new();
        let snapshot = bus
            .tracked_code_snapshot_two_pages_with_callback(&paging, 0x4ffd, 10, context, |range| {
                writes.push(range)
            })
            .unwrap();
        assert_eq!(snapshot.linear_base, 0x4ffd);
        assert_eq!(snapshot.bytes(), code);
        assert_eq!(snapshot.first.linear_base, 0x4ffd);
        assert_eq!(
            snapshot.first.physical_base,
            GuestPhysicalAddress::new(0x7000)
        );
        assert_eq!(snapshot.first.bytes, code[..3]);
        let second = snapshot.second.as_ref().unwrap();
        assert_eq!(second.linear_base, 0x5000);
        assert_eq!(second.physical_base, GuestPhysicalAddress::new(0x5000));
        assert_eq!(second.bytes, code[3..]);
        // A bits of PML4/PDPT/PD/PT[4] for the first walk, PT[5] for the second.
        assert_eq!(writes.len(), 5);
        assert_eq!(writes[4].start().get(), 0x4028);
        let first_generation = snapshot.first.backing_generation;
        let second_generation = second.backing_generation;

        // Generations are per 4 KiB page: a write to the second page leaves the
        // first generation unchanged and vice versa.
        bus.patch_physical_ram(GuestPhysicalAddress::new(0x5001), &[0xff])
            .unwrap();
        let after_second = bus
            .tracked_code_snapshot_two_pages_with_callback(&paging, 0x4ffd, 10, context, |_| {})
            .unwrap();
        assert_eq!(after_second.first.backing_generation, first_generation);
        assert!(after_second.second.as_ref().unwrap().backing_generation > second_generation);
        assert_eq!(after_second.bytes()[4], 0xff);
        bus.patch_physical_ram(GuestPhysicalAddress::new(0x7ffe), &[0xee])
            .unwrap();
        let after_first = bus
            .tracked_code_snapshot_two_pages_with_callback(&paging, 0x4ffd, 10, context, |_| {})
            .unwrap();
        assert!(after_first.first.backing_generation > first_generation);
        assert_eq!(
            after_first.second.as_ref().unwrap().backing_generation,
            after_second.second.as_ref().unwrap().backing_generation
        );

        // A range inside the first page has no second part and matches the
        // single-page capture.
        let single = bus
            .tracked_code_snapshot_two_pages_with_callback(&paging, 0x4ffd, 3, context, |_| {})
            .unwrap();
        assert!(single.second.is_none());
        assert_eq!(
            single.first,
            bus.tracked_code_snapshot(&paging, 0x4ffd, 3, context)
                .unwrap()
        );

        // At most two pages: 3 + 4096 bytes fit, one more does not; neither
        // does an empty range. The single-page API keeps its one-page limit.
        assert!(
            bus.tracked_code_snapshot_two_pages_with_callback(
                &paging,
                0x4ffd,
                0x1003,
                context,
                |_| {}
            )
            .is_ok()
        );
        for length in [0x1004, 0] {
            assert_eq!(
                bus.tracked_code_snapshot_two_pages_with_callback(
                    &paging,
                    0x4ffd,
                    length,
                    context,
                    |_| panic!("no walk")
                ),
                Err(TrackedCodeSnapshotError::InvalidLength)
            );
        }
        assert_eq!(
            bus.tracked_code_snapshot(&paging, 0x4ffd, 10, context),
            Err(TrackedCodeSnapshotError::InvalidLength)
        );
        let descriptor_origin = AccessContext {
            origin: AccessOrigin::DescriptorTable,
            ..context
        };
        assert_eq!(
            bus.tracked_code_snapshot_two_pages_with_callback(
                &paging,
                0x4ffd,
                10,
                descriptor_origin,
                |_| {}
            ),
            Err(TrackedCodeSnapshotError::InvalidAccessContext)
        );

        // Second page not present: the fault is the second page's walk (CR2 =
        // the boundary address, saved RIP = the instruction start) and the
        // first walk's effects are not rolled back (here the A bits were already set).
        let mut fault_writes = Vec::new();
        let error = bus
            .tracked_code_snapshot_two_pages_with_callback(&paging, 0x5ffd, 10, context, |range| {
                fault_writes.push(range)
            })
            .unwrap_err();
        // The Accessed bits were already set by the earlier walks.
        assert!(fault_writes.is_empty());
        match error {
            TrackedCodeSnapshotError::Access(TrackedAccessError::PageFault { record, .. }) => {
                assert_eq!(record.saved_rip(), 0x5ffd);
                assert_eq!(record.cr2_value(), 0x6000);
            }
            other => panic!("expected a second-page fault, got {other:?}"),
        }
    }

    #[test]
    fn tracked_code_snapshot_reads_one_ram_page_through_two_aliases() {
        let ram = range(0, 0x9000);
        let map = MachineMemoryMap::new(
            36,
            vec![crate::MemoryRegionSpec::new(ram, MemoryRegionKind::Ram)],
        )
        .unwrap();
        let mut bus = MachineBus::new(map, RamLimits::new(0x9000), vec![], vec![], vec![]).unwrap();
        for (address, value) in [
            (0x1000, 0x2007),
            (0x2000, 0x3007),
            (0x3000, 0x4007),
            (0x4020, 0x7007),
            (0x4028, 0x7007),
        ] {
            bus.write_physical(
                GuestPhysicalAddress::new(address),
                AccessWidth::Qword,
                value,
                |_| {},
            )
            .unwrap();
        }
        // Bits 52–62 are accepted software metadata; they must not leak into
        // the translated physical frame extracted from the leaf PTE.
        for address in [0x4020, 0x4028] {
            let pte = bus
                .read_physical(GuestPhysicalAddress::new(address), AccessWidth::Qword)
                .unwrap();
            bus.write_physical(
                GuestPhysicalAddress::new(address),
                AccessWidth::Qword,
                pte | (1 << 52) | (1 << 58),
                |_| {},
            )
            .unwrap();
        }
        let code = [0x48, 0xb8, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
        bus.patch_physical_ram(GuestPhysicalAddress::new(0x7000), &code)
            .unwrap();
        let paging = Long64ProbeConfig::new(
            0x8001_0033,
            0x1000,
            1 << 5,
            (1 << 8) | (1 << 10) | (1 << 11),
        )
        .unwrap();

        let first = bus
            .tracked_code_snapshot(&paging, 0x4000, code.len(), AccessContext::default())
            .unwrap();
        let second = bus
            .tracked_code_snapshot(&paging, 0x5000, code.len(), AccessContext::default())
            .unwrap();
        assert_eq!(first.physical_base, GuestPhysicalAddress::new(0x7000));
        assert_eq!(second.physical_base, first.physical_base);
        assert_eq!(first.bytes, code);
        assert_eq!(second.bytes, code);
        assert_eq!(first.backing_generation, second.backing_generation);
        assert_eq!(
            bus.patch_tracked_code_byte(
                &paging,
                0x4002,
                GuestPhysicalAddress::new(0x7000),
                0xaa,
                AccessContext::default(),
            )
            .unwrap(),
            GuestPhysicalAddress::new(0x7002)
        );
        assert_eq!(
            bus.read_physical(GuestPhysicalAddress::new(0x7002), AccessWidth::Byte)
                .unwrap(),
            0xaa
        );
        assert_ne!(
            bus.read_physical(GuestPhysicalAddress::new(0x1000), AccessWidth::Qword)
                .unwrap()
                & (1 << 5),
            0
        );
        assert_ne!(
            bus.read_physical(GuestPhysicalAddress::new(0x2000), AccessWidth::Qword)
                .unwrap()
                & (1 << 5),
            0
        );
        assert_ne!(
            bus.read_physical(GuestPhysicalAddress::new(0x3000), AccessWidth::Qword)
                .unwrap()
                & (1 << 5),
            0
        );
        assert_ne!(
            bus.read_physical(GuestPhysicalAddress::new(0x4020), AccessWidth::Qword)
                .unwrap()
                & (1 << 5),
            0
        );

        let pte = bus
            .read_physical(GuestPhysicalAddress::new(0x4020), AccessWidth::Qword)
            .unwrap();
        bus.write_physical(
            GuestPhysicalAddress::new(0x4020),
            AccessWidth::Qword,
            pte | (1 << 63),
            |_| {},
        )
        .unwrap();
        assert!(matches!(
            bus.tracked_code_snapshot(&paging, 0x4000, code.len(), AccessContext::default()),
            Err(TrackedCodeSnapshotError::Access(
                TrackedAccessError::PageFault { .. }
            ))
        ));
        bus.write_physical(
            GuestPhysicalAddress::new(0x4020),
            AccessWidth::Qword,
            (pte | (1 << 5)) & !(1 << 63),
            |_| {},
        )
        .unwrap();

        // The loader API models an explicit fetch/write at the supplied CPL.
        // Descriptor-table origin has architecturally privileged page checks,
        // so it is rejected instead of being accepted as a CPL3 code access.
        let supervisor_only = pte & !(1 << 2);
        bus.write_physical(
            GuestPhysicalAddress::new(0x4020),
            AccessWidth::Qword,
            supervisor_only | (1 << 5),
            |_| {},
        )
        .unwrap();
        let cpl3 = AccessContext {
            privilege: od_isa::memory::PrivilegeLevel::Ring3,
            ..AccessContext::default()
        };
        assert!(matches!(
            bus.tracked_code_snapshot(&paging, 0x4000, code.len(), cpl3),
            Err(TrackedCodeSnapshotError::Access(
                TrackedAccessError::PageFault { .. }
            ))
        ));
        assert!(matches!(
            bus.patch_tracked_code_byte(
                &paging,
                0x4002,
                GuestPhysicalAddress::new(0x7000),
                0xbb,
                cpl3,
            ),
            Err(TrackedCodePatchError::Access(
                TrackedAccessError::PageFault { .. }
            ))
        ));
        let descriptor_origin = AccessContext {
            origin: od_isa::memory::AccessOrigin::DescriptorTable,
            ..cpl3
        };
        assert_eq!(
            bus.tracked_code_snapshot(&paging, 0x4000, code.len(), descriptor_origin),
            Err(TrackedCodeSnapshotError::InvalidAccessContext)
        );
        assert_eq!(
            bus.patch_tracked_code_byte(
                &paging,
                0x4002,
                GuestPhysicalAddress::new(0x7000),
                0xbb,
                descriptor_origin,
            ),
            Err(TrackedCodePatchError::InvalidAccessContext)
        );
        bus.write_physical(
            GuestPhysicalAddress::new(0x4020),
            AccessWidth::Qword,
            pte | (1 << 5),
            |_| {},
        )
        .unwrap();

        bus.patch_physical_ram(GuestPhysicalAddress::new(0x7002), &[0xaa])
            .unwrap();
        let patched = bus
            .tracked_code_snapshot(&paging, 0x5000, code.len(), AccessContext::default())
            .unwrap();
        assert_eq!(patched.bytes[2], 0xaa);
        assert!(patched.backing_generation > first.backing_generation);

        // Reset walker A/D bits and require the callback API to report the
        // read preflight's A updates, then the write walk's D update and the
        // committed operand byte, in architectural operation order.
        for address in [0x1000, 0x2000, 0x3000, 0x4020] {
            let entry = bus
                .read_physical(GuestPhysicalAddress::new(address), AccessWidth::Qword)
                .unwrap();
            bus.write_physical(
                GuestPhysicalAddress::new(address),
                AccessWidth::Qword,
                entry & !((1 << 5) | (1 << 6)),
                |_| {},
            )
            .unwrap();
        }
        let mut committed = Vec::new();
        bus.patch_tracked_code_byte_with_callback(
            &paging,
            0x4002,
            GuestPhysicalAddress::new(0x7000),
            0xbb,
            AccessContext::default(),
            |range| committed.push(range),
        )
        .unwrap();
        assert_eq!(
            committed,
            [0x1000, 0x2000, 0x3000, 0x4020, 0x4020, 0x7002].map(|address| {
                GuestPhysicalRange::new(
                    GuestPhysicalAddress::new(address),
                    if address == 0x7002 { 1 } else { 8 },
                )
                .unwrap()
            })
        );
    }

    #[test]
    fn tracked_code_callbacks_preserve_commits_before_snapshot_and_patch_errors() {
        let make_bus = || {
            let ram = range(0, 0x9000);
            let map = MachineMemoryMap::new(
                36,
                vec![crate::MemoryRegionSpec::new(ram, MemoryRegionKind::Ram)],
            )
            .unwrap();
            let mut bus =
                MachineBus::new(map, RamLimits::new(0x9000), vec![], vec![], vec![]).unwrap();
            for (address, value) in [
                (0x7000, 0x2007), // PML4[0] resides in the code backing page.
                (0x7400, 0x2007), // PML4[128] is used by the failing fetch.
                (0x2000, 0x3007),
                (0x3010, 0x4007),
                (0x4008, 0x7007),
            ] {
                bus.write_physical(
                    GuestPhysicalAddress::new(address),
                    AccessWidth::Qword,
                    value,
                    |_| {},
                )
                .unwrap();
            }
            let paging = Long64ProbeConfig::new(
                0x8001_0033,
                0x7000,
                1 << 5,
                (1 << 8) | (1 << 10) | (1 << 11),
            )
            .unwrap();
            (bus, paging)
        };
        let ring3 = AccessContext {
            privilege: od_isa::memory::PrivilegeLevel::Ring3,
            ..AccessContext::default()
        };

        // The fetch sets PML4 and PDPT Accessed bits, then faults on the
        // absent PD entry. Only those two committed writes are reported.
        let (mut snapshot_bus, paging) = make_bus();
        let fault_linear = 0x4000_0000_0000;
        let mut snapshot_writes = Vec::new();
        let snapshot_error = snapshot_bus
            .tracked_code_snapshot_with_callback(&paging, fault_linear, 1, ring3, |range| {
                snapshot_writes.push(range)
            })
            .unwrap_err();
        assert!(matches!(
            snapshot_error,
            TrackedCodeSnapshotError::Access(TrackedAccessError::PageFault { ref record, .. })
                if record.cr2_value() == fault_linear
                    && record.fault().error_code == Some(0x14)
        ));
        assert_eq!(
            snapshot_writes,
            [0x7400, 0x2000].map(|address| {
                GuestPhysicalRange::new(GuestPhysicalAddress::new(address), 8).unwrap()
            })
        );

        // The patch read preflight commits A updates through the root, PDPT,
        // PDE and read-only PTE, then the write walk faults with #PF(P=1,W=1,U=1).
        let (mut patch_bus, paging) = make_bus();
        patch_bus
            .write_physical(
                GuestPhysicalAddress::new(0x4008),
                AccessWidth::Qword,
                0x7005,
                |_| {},
            )
            .unwrap();
        let mut patch_writes = Vec::new();
        let patch_error = patch_bus
            .patch_tracked_code_byte_with_callback(
                &paging,
                0x0040_1202,
                GuestPhysicalAddress::new(0x7000),
                0xaa,
                ring3,
                |range| patch_writes.push(range),
            )
            .unwrap_err();
        assert!(matches!(
            patch_error,
            TrackedCodePatchError::Access(TrackedAccessError::PageFault { ref record, .. })
                if record.cr2_value() == 0x0040_1202
                    && record.fault().error_code == Some(0x7)
        ));
        assert_eq!(
            patch_writes,
            [0x7000, 0x2000, 0x3010, 0x4008].map(|address| {
                GuestPhysicalRange::new(GuestPhysicalAddress::new(address), 8).unwrap()
            })
        );
        assert_eq!(
            patch_bus
                .read_physical(GuestPhysicalAddress::new(0x7000), AccessWidth::Qword)
                .unwrap(),
            0x2027
        );

        // A mismatched expected frame fails after the read preflight. Its A
        // updates remain observable and no write-walk/operand callback occurs.
        let (mut mismatch_bus, paging) = make_bus();
        let mut mismatch_writes = Vec::new();
        let mismatch = mismatch_bus
            .patch_tracked_code_byte_with_callback(
                &paging,
                0x0040_1202,
                GuestPhysicalAddress::new(0x8000),
                0xaa,
                ring3,
                |range| mismatch_writes.push(range),
            )
            .unwrap_err();
        assert_eq!(
            mismatch,
            TrackedCodePatchError::PhysicalPageMismatch {
                expected: GuestPhysicalAddress::new(0x8000),
                actual: GuestPhysicalAddress::new(0x7202),
            }
        );
        assert_eq!(
            mismatch_writes,
            [0x7000, 0x2000, 0x3010, 0x4008].map(|address| {
                GuestPhysicalRange::new(GuestPhysicalAddress::new(address), 8).unwrap()
            })
        );
    }

    #[test]
    fn dispatches_all_supported_widths_and_rejects_pio_qword() {
        let (mut bus, state) = setup(false);
        let mut commits = 0;
        for width in [
            AccessWidth::Byte,
            AccessWidth::Word,
            AccessWidth::Dword,
            AccessWidth::Qword,
        ] {
            bus.write_physical(
                GuestPhysicalAddress::new(0x2000),
                width,
                0x8877_6655_4433_2211,
                |_| commits += 1,
            )
            .unwrap();
            assert_eq!(
                bus.read_physical(GuestPhysicalAddress::new(0x2000), width)
                    .unwrap(),
                0x8877_6655_4433_2211 & width.mask()
            );
        }
        for width in [AccessWidth::Byte, AccessWidth::Word, AccessWidth::Dword] {
            bus.write_port(PortAddress::new(0x20), width, 0x8877_6655)
                .unwrap();
            assert_eq!(
                bus.read_port(PortAddress::new(0x20), width).unwrap(),
                (0x8877_6655u64 & width.mask()) as u32
            );
        }
        let before = state.lock().unwrap().calls;
        assert_eq!(
            bus.read_port(PortAddress::new(0x20), AccessWidth::Qword),
            Err(BusAccessError::UnsupportedWidth)
        );
        assert_eq!(
            bus.write_port(PortAddress::new(0x20), AccessWidth::Qword, 1),
            Err(BusAccessError::UnsupportedWidth)
        );
        assert_eq!(state.lock().unwrap().calls, before);
        assert_eq!(commits, 0);
    }

    #[test]
    fn device_errors_are_distinct_and_dispatch_once() {
        let (mut bus, state) = setup(false);
        assert!(matches!(
            bus.read_physical(GuestPhysicalAddress::new(0x2010), AccessWidth::Byte),
            Err(BusAccessError::Device {
                source: DeviceError::UnknownOffset,
                ..
            })
        ));
        assert!(matches!(
            bus.read_physical(GuestPhysicalAddress::new(0x2001), AccessWidth::Word),
            Err(BusAccessError::Device {
                source: DeviceError::Misaligned,
                ..
            })
        ));
        assert!(matches!(
            bus.write_physical(
                GuestPhysicalAddress::new(0x2004),
                AccessWidth::Byte,
                1,
                |_| {}
            ),
            Err(BusAccessError::Device {
                source: DeviceError::ReadOnly,
                ..
            })
        ));
        assert_eq!(state.lock().unwrap().calls, 3);
        assert_eq!(state.lock().unwrap().value, 0);
    }

    #[test]
    fn pre_dispatch_rejections_do_not_call_device_or_change_ram() {
        let (mut bus, state) = setup(true);
        let mut commits = 0;
        bus.write_physical(
            GuestPhysicalAddress::new(0x1000),
            AccessWidth::Dword,
            0x1234_5678,
            |_| commits += 1,
        )
        .unwrap();
        assert_eq!(commits, 1);
        let mut rejected_commits = 0;
        assert_eq!(
            bus.write_physical(
                GuestPhysicalAddress::new(0x20fe),
                AccessWidth::Dword,
                7,
                |_| rejected_commits += 1
            ),
            Err(BusAccessError::CrossesRegion)
        );
        assert_eq!(
            bus.read_physical(GuestPhysicalAddress::new(0x1000), AccessWidth::Dword)
                .unwrap(),
            0x1234_5678
        );
        assert_eq!(rejected_commits, 0);
        assert_eq!(state.lock().unwrap().calls, 0);
    }

    #[test]
    fn validates_port_overlap_and_mmio_bindings() {
        let overlap = PortRange::new(PortAddress::new(0x20), 4).unwrap();
        let overlap2 = PortRange::new(PortAddress::new(0x23), 2).unwrap();
        let map = MachineMemoryMap::new(
            36,
            vec![crate::MemoryRegionSpec::new(
                range(0x2000, 16),
                MemoryRegionKind::Mmio,
            )],
        )
        .unwrap();
        let device: Vec<Box<dyn BusDevice>> = vec![Box::new(CountingDevice(Arc::new(Mutex::new(
            DeviceState::default(),
        ))))];
        assert!(matches!(
            MachineBus::new(
                map.clone(),
                RamLimits::new(0),
                vec![Box::new(CountingDevice(Arc::new(Mutex::new(
                    DeviceState::default()
                ))))],
                vec![],
                vec![]
            ),
            Err(BusBuildError::MissingMmioBinding(_))
        ));
        assert!(matches!(
            MachineBus::new(
                map.clone(),
                RamLimits::new(0),
                device,
                vec![MmioBinding {
                    region_start: GuestPhysicalAddress::new(0x2000),
                    device_index: 0
                }],
                vec![
                    PortRegionBinding {
                        range: overlap,
                        device_index: 0
                    },
                    PortRegionBinding {
                        range: overlap2,
                        device_index: 0
                    }
                ]
            ),
            Err(BusBuildError::OverlappingPortRanges(_, _))
        ));
        assert_eq!(
            PortRange::new(PortAddress::new(u16::MAX), 2),
            Err(BusBuildError::InvalidPortRange)
        );
        assert_eq!(
            PortRange::new(PortAddress::new(0), u32::MAX),
            Err(BusBuildError::InvalidPortRange)
        );
        assert_eq!(
            PortRange::new(PortAddress::new(1), u32::MAX),
            Err(BusBuildError::InvalidPortRange)
        );
        let final_port = PortRange::new(PortAddress::new(u16::MAX), 1).unwrap();
        assert_eq!(final_port.end(), 65_536);
    }

    const STEP: DeviceErrorContext = DeviceErrorContext {
        rip: 0x7000,
        cpl: 0,
        retired_in_call: 9,
    };

    /// S7-32 diagnostic: without a log, and with a log but a view without the instruction (the
    /// turbo engine's, every other loop's), a refusal of the set-step view is the error it was;
    /// with both a refused port write is dropped (the device keeps its value), a refused or
    /// unmapped read gives all ones of its width, and the accesses are grouped with the
    /// instruction of the first one and the retired count from the board's base.
    #[test]
    fn the_device_error_log_completes_refused_port_accesses_only_for_the_general_step() {
        let refused = || BusAccessError::Device {
            device_index: 0,
            source: DeviceError::ReadOnly,
        };
        let (mut bus, state) = setup(false);
        state.lock().unwrap().value = 0x5a;
        {
            let (_, io) = bus.split_for_set_step();
            let io = io.with_device_error_context(STEP);
            assert_eq!(io.port_io().write_u8(0x24, 1), Err(refused()));
            assert_eq!(
                io.port_io().read_u8(0x40),
                Err(BusAccessError::UnmappedPort(PortAddress::new(0x40)))
            );
        }
        bus.enable_device_error_log(10, 8);
        bus.device_error_log().unwrap().set_retired_base(100);
        {
            let (_, io) = bus.split_for_set_step();
            assert_eq!(io.port_io().write_u8(0x24, 1), Err(refused()));
        }
        assert_eq!(bus.device_error_log().unwrap().report().events, 0);
        {
            let (_, io) = bus.split_for_set_step();
            let io = io.with_device_error_context(STEP);
            let mut port = io.port_io();
            assert_eq!(port.write_u8(0x24, 0x11), Ok(()));
            assert_eq!(port.write_u8(0x24, 0x22), Ok(()));
            // Word at an odd port: the device refuses it (Misaligned); a read gives all ones.
            assert_eq!(port.read_u16(0x21), Ok(0xffff));
            assert_eq!(port.read_u32(0x40), Ok(0xffff_ffff));
            assert_eq!(port.write_u16(0x40, 0x1234), Ok(()));
            // An access the device serves is untouched by the log.
            assert_eq!(port.read_u8(0x20), Ok(0x5a));
        }
        assert_eq!(
            state.lock().unwrap().value,
            0x5a,
            "the refused write is dropped"
        );
        let report = bus.device_error_log().unwrap().report();
        assert_eq!((report.events, report.stopped), (5, None));
        let groups: Vec<_> = report
            .groups
            .iter()
            .map(|group| {
                (
                    group.key.device_index,
                    group.key.address,
                    group.key.direction,
                    group.key.width,
                    group.key.kind.clone(),
                    group.count,
                    group.first_value,
                    group.region_base,
                )
            })
            .collect();
        use DeviceErrorDirection::{Read, Write};
        assert_eq!(
            groups,
            [
                (
                    Some(0),
                    0x24,
                    Write,
                    AccessWidth::Byte,
                    DeviceErrorKind::Device(DeviceError::ReadOnly),
                    2,
                    Some(0x11),
                    Some(0x20)
                ),
                (
                    Some(0),
                    0x21,
                    Read,
                    AccessWidth::Word,
                    DeviceErrorKind::Device(DeviceError::Misaligned),
                    1,
                    None,
                    Some(0x20)
                ),
                (
                    None,
                    0x40,
                    Read,
                    AccessWidth::Dword,
                    DeviceErrorKind::UnmappedPort,
                    1,
                    None,
                    None
                ),
                (
                    None,
                    0x40,
                    Write,
                    AccessWidth::Word,
                    DeviceErrorKind::UnmappedPort,
                    1,
                    Some(0x1234),
                    None
                ),
            ]
        );
        let first = &report.groups[0];
        assert_eq!(
            (first.first_rip, first.first_cpl, first.first_retired),
            (0x7000, 0, 109)
        );
        assert!(
            report
                .groups
                .iter()
                .all(|group| group.key.space == IoSpace::Pio)
        );
    }

    /// S7-32 diagnostic: an MMIO refusal of the set-step view keeps its text without the
    /// instruction; with it a refused write is dropped (`mmio_written` stays clear) and a refused
    /// read gives all ones of its width. Once the event limit is reached the next refusal is the
    /// error it was, and the log says why it stopped.
    #[test]
    fn the_device_error_log_completes_refused_mmio_accesses_and_stops_at_its_limit() {
        let (mut bus, state) = setup(false);
        state.lock().unwrap().value = 0x77;
        bus.serve_firmware_set_windows(&[(0x2000, 0x100)]);
        bus.enable_device_error_log(2, 8);
        let write = RamAccessOperation::Write {
            width: AccessWidth::Dword,
            value: 9,
        };
        let read = RamAccessOperation::Read {
            width: AccessWidth::Qword,
        };
        let at = |address| GuestPhysicalAddress::new(address);
        {
            let (_, io) = bus.split_for_set_step();
            assert_eq!(
                io.dispatch(at(0x2004), 4, write),
                Err(crate::page_walk::MmioDispatchError::Backend(
                    "MMIO device 0 write failed: ReadOnly".to_owned()
                ))
            );
        }
        {
            let (_, io) = bus.split_for_set_step();
            let io = io.with_device_error_context(STEP);
            assert_eq!(io.dispatch(at(0x2004), 4, write), Ok(0));
            assert!(!io.mmio_written(), "a dropped write reached no device");
            assert_eq!(io.dispatch(at(0x2010), 8, read), Ok(u64::MAX));
            // An access the device serves.
            assert_eq!(io.dispatch(at(0x2000), 8, read), Ok(0x77));
            // The third refusal: the limit (2) is reached, the error is what it was.
            assert_eq!(
                io.dispatch(at(0x2004), 4, write),
                Err(crate::page_walk::MmioDispatchError::Backend(
                    "MMIO device 0 write failed: ReadOnly".to_owned()
                ))
            );
        }
        assert_eq!(state.lock().unwrap().value, 0x77);
        let report = bus.device_error_log().unwrap().report();
        assert_eq!(report.events, 2);
        assert_eq!(report.stopped, Some(DeviceErrorLogStop::EventLimit));
        let write_group = &report.groups[0];
        assert_eq!(write_group.key.space, IoSpace::Mmio);
        assert_eq!(write_group.key.address, 0x2004);
        assert_eq!(write_group.region_base, Some(0x2000));
        assert_eq!(write_group.first_value, Some(9));
        assert_eq!(
            report.groups[1].key.kind,
            DeviceErrorKind::Device(DeviceError::UnknownOffset)
        );
        assert_eq!(report.groups[1].key.width, AccessWidth::Qword);
    }

    /// A device that refuses every access with one error, and counts the accesses that reach it.
    struct RefusingDevice(DeviceError, Arc<Mutex<DeviceState>>);

    impl BusDevice for RefusingDevice {
        fn read(&mut self, _: DeviceAccess) -> Result<u64, DeviceError> {
            self.1.lock().unwrap().calls += 1;
            Err(self.0.clone())
        }

        fn write(&mut self, _: DeviceAccess, value: u64) -> Result<(), DeviceError> {
            let mut state = self.1.lock().unwrap();
            state.calls += 1;
            state.value = value;
            Err(self.0.clone())
        }
    }

    /// [`setup`] with a [`RefusingDevice`] in place of the counting one.
    fn refusing_bus(error: DeviceError) -> (MachineBus, Arc<Mutex<DeviceState>>) {
        let state = Arc::new(Mutex::new(DeviceState::default()));
        let map = MachineMemoryMap::new(
            36,
            vec![
                crate::MemoryRegionSpec::new(range(0x1000, 0x100), MemoryRegionKind::Ram),
                crate::MemoryRegionSpec::new(range(0x2000, 0x100), MemoryRegionKind::Mmio),
            ],
        )
        .unwrap();
        let mut bus = MachineBus::new(
            map,
            RamLimits::new(0x200),
            vec![Box::new(RefusingDevice(error, Arc::clone(&state)))],
            vec![MmioBinding {
                region_start: GuestPhysicalAddress::new(0x2000),
                device_index: 0,
            }],
            vec![PortRegionBinding {
                range: PortRange::new(PortAddress::new(0x20), 16).unwrap(),
                device_index: 0,
            }],
        )
        .unwrap();
        bus.serve_firmware_set_windows(&[(0x2000, 0x100)]);
        bus.enable_device_error_log(10, 8);
        (bus, state)
    }

    /// S7-32 diagnostic: what the log never absorbs, even with the instruction: a clock overflow
    /// of a device (port and MMIO: the end of virtual time, not a gap of the model) and the busy
    /// device table (`device_index` `usize::MAX`). An access that straddles the end of a port
    /// binding is absorbed, with no device.
    #[test]
    fn the_device_error_log_keeps_a_clock_overflow_and_a_busy_table_and_absorbs_a_crossing() {
        let (mut bus, state) = refusing_bus(DeviceError::ClockOverflow);
        let overflow = || BusAccessError::Device {
            device_index: 0,
            source: DeviceError::ClockOverflow,
        };
        let read = RamAccessOperation::Read {
            width: AccessWidth::Dword,
        };
        {
            let (_, io) = bus.split_for_set_step();
            let io = io.with_device_error_context(STEP);
            assert_eq!(io.port_io().read_u8(0x20), Err(overflow()));
            assert_eq!(io.port_io().write_u8(0x20, 1), Err(overflow()));
            assert_eq!(
                io.dispatch(GuestPhysicalAddress::new(0x2000), 4, read),
                Err(crate::page_walk::MmioDispatchError::Backend(
                    "MMIO device 0 read failed: ClockOverflow".to_owned()
                ))
            );
            let busy = || BusAccessError::Device {
                device_index: usize::MAX,
                source: DeviceError::Busy,
            };
            assert_eq!(
                io.absorb_port(busy(), 0x20, AccessWidth::Byte, None),
                Err(busy())
            );
            // A dword at 0x2E covers 0x2E..0x32 and the binding ends at 0x30.
            assert_eq!(io.port_io().read_u32(0x2e), Ok(0xffff_ffff));
        }
        assert_eq!(
            state.lock().unwrap().calls,
            3,
            "the crossing reached no device"
        );
        let report = bus.device_error_log().unwrap().report();
        assert_eq!(report.events, 1);
        let crossing = &report.groups[0];
        assert_eq!(crossing.key.kind, DeviceErrorKind::CrossesRegion);
        assert_eq!(
            (
                crossing.key.device_index,
                crossing.key.address,
                crossing.key.width
            ),
            (None, 0x2e, AccessWidth::Dword)
        );
    }

    /// S7-32 diagnostic: the write half of a read-modify-write (CPL0 reads the operand with a
    /// plain read) after an absorbed read of the same instruction is not sent to the device and
    /// is logged as its own kind; a write elsewhere still reaches the device, and the next
    /// instruction (a new view) starts with no absorbed read.
    #[test]
    fn a_write_after_an_absorbed_read_of_the_same_instruction_is_not_sent_to_the_device() {
        let (mut bus, state) = refusing_bus(DeviceError::WriteOnly);
        let at = GuestPhysicalAddress::new;
        let read = RamAccessOperation::Read {
            width: AccessWidth::Dword,
        };
        let write = |value| RamAccessOperation::Write {
            width: AccessWidth::Dword,
            value,
        };
        {
            let (_, io) = bus.split_for_set_step();
            let io = io.with_device_error_context(STEP);
            assert_eq!(io.dispatch(at(0x20b0), 4, read), Ok(0xffff_ffff));
            assert_eq!(io.dispatch(at(0x20b0), 4, write(0xffff_ffff)), Ok(0));
            // A byte write inside the dword is suppressed as well.
            assert_eq!(io.dispatch(at(0x20b2), 1, write(0x12)), Ok(0));
            assert!(!io.mmio_written());
        }
        assert_eq!(
            state.lock().unwrap().calls,
            1,
            "only the read reached the device"
        );
        {
            let (_, io) = bus.split_for_set_step();
            let io = io.with_device_error_context(STEP);
            // A new instruction: the write goes to the device (which refuses it: absorbed).
            assert_eq!(io.dispatch(at(0x20b0), 4, write(7)), Ok(0));
        }
        {
            let mut state = state.lock().unwrap();
            assert_eq!((state.calls, state.value), (2, 7));
            state.calls = 0;
        }
        let report = bus.device_error_log().unwrap().report();
        let groups: Vec<_> = report
            .groups
            .iter()
            .map(|group| {
                (
                    group.key.address,
                    group.key.direction,
                    group.key.kind.clone(),
                    group.count,
                    group.first_value,
                )
            })
            .collect();
        use DeviceErrorDirection::{Read, Write};
        use DeviceErrorKind::{Device, SuppressedAfterAbsorbedRead as Suppressed};
        assert_eq!(
            groups,
            [
                (0x20b0, Read, Device(DeviceError::WriteOnly), 1, None),
                (0x20b0, Write, Suppressed, 1, Some(0xffff_ffff)),
                (0x20b2, Write, Suppressed, 1, Some(0x12)),
                (0x20b0, Write, Device(DeviceError::WriteOnly), 1, Some(7)),
            ]
        );
        // Without a log nothing is absorbed, so a write always reaches the device.
        let (mut plain, plain_state) = refusing_bus(DeviceError::WriteOnly);
        plain.device_errors = None;
        let (_, io) = plain.split_for_set_step();
        let io = io.with_device_error_context(STEP);
        assert!(io.dispatch(at(0x20b0), 4, read).is_err());
        assert!(io.dispatch(at(0x20b0), 4, write(1)).is_err());
        assert_eq!(plain_state.lock().unwrap().calls, 2);
    }
}

#[cfg(test)]
mod shared_tests;
