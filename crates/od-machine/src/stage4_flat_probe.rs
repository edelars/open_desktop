//! Fixed raw-image loader and direct-entry handoff for the S4-32a probe.

use crate::{
    AccessWidth, BusAccessError, BusBuildError, GuestPhysicalAddress, GuestPhysicalRange,
    MachineBus, MachineMemoryMap, MemoryAccessError, MemoryRegionKind, MemoryRegionSpec,
    PagedProbeQuantumReport, PortAddress, PortRange, PortRegionBinding, RamLimits, Uart16550Handle,
    Uart16550PollingTx, run_paged_probe,
};
use od_isa::LongModeState;
use od_isa::memory::{AccessContext, InstructionMode, PrivilegeLevel};
use od_isa::{CpuActivity, INTEGER_FLAGS, StiShadow};
use std::fmt;

/// Raw image size accepted by [`load_stage4_flat_probe`].
pub const STAGE4_FLAT_PROBE_IMAGE_BYTES: usize = 0x9000;
/// Guest physical address where the raw image is placed verbatim.
pub const STAGE4_FLAT_PROBE_IMAGE_GPA: u64 = 0x1000;
/// Guest RAM size in the fixed Stage 4 board profile.
pub const STAGE4_FLAT_PROBE_RAM_BYTES: u64 = 0x20_000;
pub const STAGE4_FLAT_PROBE_CR3: u64 = 0x1000;
pub const STAGE4_FLAT_PROBE_EFER: u64 = 0x0d00;
pub const STAGE4_FLAT_PROBE_ENTRY_GVA: u64 = 0x0040_0000;
pub const STAGE4_FLAT_PROBE_DATA_GVA: u64 = 0x0080_0000;
pub const STAGE4_FLAT_PROBE_STACK_TOP_GVA: u64 = 0x00a0_1000;
pub const STAGE4_FLAT_PROBE_UART_BASE: u16 = 0x03f8;

const CR0: u64 = 0x8001_0033;
const CR4: u64 = 0x20;
const IMAGE_END_GPA: u64 = STAGE4_FLAT_PROBE_IMAGE_GPA + STAGE4_FLAT_PROBE_IMAGE_BYTES as u64;
const PAGE_BYTES: u64 = 0x1000;
const STACK_GPA: u64 = 0xa000;
const FLAGS_DEFINED: u64 = INTEGER_FLAGS;
const CODE: [u8; 5] = [0x48, 0x8b, 0x03, 0xee, 0xf4];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stage4FlatProbeLayoutError {
    PageTableEntry {
        gpa: u64,
        expected: u64,
        actual: u64,
    },
    CodeBytes {
        gpa: u64,
    },
}

#[derive(Debug, Eq, PartialEq)]
pub enum Stage4FlatProbeLoadError {
    ImageSize { expected: usize, actual: usize },
    AddressOverflow,
    RamCoverage { required_end: u64, ram_bytes: u64 },
    Layout(Stage4FlatProbeLayoutError),
    Memory(MemoryAccessError),
    BusBuild(BusBuildError),
    UartConfiguration(BusAccessError),
}

impl fmt::Display for Stage4FlatProbeLoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Stage4FlatProbeV1 load failed: {self:?}")
    }
}

impl std::error::Error for Stage4FlatProbeLoadError {}

/// A successfully validated image and its direct-entry machine state.
///
/// The image's page tables, code and data are supplied by the caller. This
/// object owns the resulting machine, the UART control handle, and the exact
/// CPU/CR3/EFER handoff accepted by `run_paged_probe`.
pub struct LoadedStage4FlatProbe {
    bus: MachineBus,
    state: LongModeState,
    uart: Uart16550Handle,
    rflags_defined: u64,
}

impl LoadedStage4FlatProbe {
    pub fn state(&self) -> &LongModeState {
        &self.state
    }

    pub fn state_mut(&mut self) -> &mut LongModeState {
        &mut self.state
    }

    pub fn bus_mut(&mut self) -> &mut MachineBus {
        &mut self.bus
    }

    pub const fn cr3(&self) -> u64 {
        STAGE4_FLAT_PROBE_CR3
    }

    pub const fn efer(&self) -> u64 {
        STAGE4_FLAT_PROBE_EFER
    }

    pub const fn rflags_defined(&self) -> u64 {
        self.rflags_defined
    }

    pub fn uart(&self) -> Uart16550Handle {
        self.uart.clone()
    }

    /// Execute the existing bounded paged interpreter quantum with this
    /// image's fixed CR3/EFER, Long64/CPL0 context and definedness snapshot.
    pub fn run(
        &mut self,
        instruction_budget: u64,
        stop: impl FnMut() -> bool,
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> PagedProbeQuantumReport {
        let cr3 = self.cr3();
        let efer = self.efer();
        let rflags_defined = self.rflags_defined;
        run_paged_probe(
            &mut self.state,
            &mut self.bus,
            cr3,
            efer,
            AccessContext {
                mode: InstructionMode::Long64,
                privilege: PrivilegeLevel::Ring0,
                origin: od_isa::memory::AccessOrigin::Explicit,
            },
            instruction_budget,
            rflags_defined,
            stop,
            committed_write,
        )
    }
}

/// Validate a `Stage4FlatProbeV1` image, copy its bytes to GPA `0x1000`, and
/// return the fixed CPU, paging and polling-UART handoff. No guest bytes or
/// page-table entries are synthesized by the loader.
pub fn load_stage4_flat_probe(
    image: &[u8],
) -> Result<LoadedStage4FlatProbe, Stage4FlatProbeLoadError> {
    if image.len() != STAGE4_FLAT_PROBE_IMAGE_BYTES {
        return Err(Stage4FlatProbeLoadError::ImageSize {
            expected: STAGE4_FLAT_PROBE_IMAGE_BYTES,
            actual: image.len(),
        });
    }
    let image_end = STAGE4_FLAT_PROBE_IMAGE_GPA
        .checked_add(
            u64::try_from(image.len()).map_err(|_| Stage4FlatProbeLoadError::AddressOverflow)?,
        )
        .ok_or(Stage4FlatProbeLoadError::AddressOverflow)?;
    let stack_end = STACK_GPA
        .checked_add(PAGE_BYTES)
        .ok_or(Stage4FlatProbeLoadError::AddressOverflow)?;
    if image_end != IMAGE_END_GPA
        || image_end > STAGE4_FLAT_PROBE_RAM_BYTES
        || stack_end > STAGE4_FLAT_PROBE_RAM_BYTES
        || STACK_GPA < image_end
    {
        return Err(Stage4FlatProbeLoadError::RamCoverage {
            required_end: image_end.max(stack_end),
            ram_bytes: STAGE4_FLAT_PROBE_RAM_BYTES,
        });
    }
    validate_image(image)?;

    let ram_range =
        GuestPhysicalRange::new(GuestPhysicalAddress::new(0), STAGE4_FLAT_PROBE_RAM_BYTES)
            .map_err(Stage4FlatProbeLoadError::Memory)?;
    let map = MachineMemoryMap::new(
        36,
        vec![MemoryRegionSpec::new(ram_range, MemoryRegionKind::Ram)],
    )
    .map_err(Stage4FlatProbeLoadError::Memory)?;
    let (uart_device, uart) = Uart16550PollingTx::new_with_handle();
    let uart_range = PortRange::new(PortAddress::new(STAGE4_FLAT_PROBE_UART_BASE), 8)
        .map_err(|_| Stage4FlatProbeLoadError::BusBuild(BusBuildError::InvalidPortRange))?;
    let mut bus = MachineBus::new(
        map,
        RamLimits::new(STAGE4_FLAT_PROBE_RAM_BYTES),
        vec![Box::new(uart_device)],
        vec![],
        vec![PortRegionBinding {
            range: uart_range,
            device_index: 0,
        }],
    )
    .map_err(Stage4FlatProbeLoadError::BusBuild)?;

    // One contiguous write preserves the raw image byte-for-byte and produces
    // a single committed-range notification for the placement operation.
    bus.write_flat_bytes(
        GuestPhysicalAddress::new(STAGE4_FLAT_PROBE_IMAGE_GPA),
        image,
        |_| {},
    )
    .map_err(|error| match error {
        BusAccessError::Memory(error) => Stage4FlatProbeLoadError::Memory(error),
        _ => Stage4FlatProbeLoadError::RamCoverage {
            required_end: image_end,
            ram_bytes: STAGE4_FLAT_PROBE_RAM_BYTES,
        },
    })?;
    configure_polling_uart(&mut bus)?;

    let mut state = LongModeState {
        rip: STAGE4_FLAT_PROBE_ENTRY_GVA,
        rflags: 2,
        cr0: Some(CR0),
        cr4: Some(CR4),
        sti_shadow: Some(StiShadow::Clear),
        activity: Some(CpuActivity::Running),
        ..LongModeState::default()
    };
    state.gpr[od_isa::Register::Rbx as usize] = STAGE4_FLAT_PROBE_DATA_GVA;
    state.gpr[od_isa::Register::Rsp as usize] = STAGE4_FLAT_PROBE_STACK_TOP_GVA;
    state.gpr[od_isa::Register::Rdx as usize] = u64::from(STAGE4_FLAT_PROBE_UART_BASE);

    Ok(LoadedStage4FlatProbe {
        bus,
        state,
        uart,
        rflags_defined: FLAGS_DEFINED,
    })
}

fn validate_image(image: &[u8]) -> Result<(), Stage4FlatProbeLoadError> {
    let expected_entries = [
        (0x1000, 0x2003),
        (0x2000, 0x3003),
        (0x3010, 0x4003),                    // code at 4 MiB
        (0x3020, 0x5003),                    // data at 8 MiB
        (0x3028, 0x6003),                    // stack at 10 MiB
        (0x4000, 0x7001),                    // supervisor read/execute code
        (0x5000, 0x9003 | (1 << 63)),        // supervisor read/write, NX data
        (0x6000, STACK_GPA | 3 | (1 << 63)), // supervisor read/write, NX stack
    ];
    for (gpa, expected) in expected_entries {
        let actual = image_entry(image, gpa);
        if actual != expected {
            return Err(Stage4FlatProbeLoadError::Layout(
                Stage4FlatProbeLayoutError::PageTableEntry {
                    gpa,
                    expected,
                    actual,
                },
            ));
        }
    }
    // The fixed image owns exactly six table pages. Reject additional present
    // entries, aliases, and unexpected mappings instead of silently accepting
    // a different guest memory layout.
    for table_gpa in [0x1000_u64, 0x2000, 0x3000, 0x4000, 0x5000, 0x6000] {
        for index in 0..512_u64 {
            let gpa = table_gpa + index * 8;
            let expected = expected_entries
                .iter()
                .find_map(|(entry_gpa, value)| (*entry_gpa == gpa).then_some(*value))
                .unwrap_or(0);
            let actual = image_entry(image, gpa);
            if actual != expected {
                return Err(Stage4FlatProbeLoadError::Layout(
                    Stage4FlatProbeLayoutError::PageTableEntry {
                        gpa,
                        expected,
                        actual,
                    },
                ));
            }
        }
    }
    let code_offset =
        usize::try_from(0x7000 - STAGE4_FLAT_PROBE_IMAGE_GPA).expect("fixed code image offset");
    if image.get(code_offset..code_offset + CODE.len()) != Some(CODE.as_slice()) {
        return Err(Stage4FlatProbeLoadError::Layout(
            Stage4FlatProbeLayoutError::CodeBytes { gpa: 0x7000 },
        ));
    }
    Ok(())
}

fn image_entry(image: &[u8], gpa: u64) -> u64 {
    let offset = usize::try_from(gpa - STAGE4_FLAT_PROBE_IMAGE_GPA).expect("fixed image offset");
    u64::from_le_bytes(
        image[offset..offset + 8]
            .try_into()
            .expect("qword within image"),
    )
}

fn configure_polling_uart(bus: &mut MachineBus) -> Result<(), Stage4FlatProbeLoadError> {
    for (offset, value) in [(3, 0x83), (0, 0x01), (1, 0x00), (3, 0x03)] {
        bus.write_port(
            PortAddress::new(STAGE4_FLAT_PROBE_UART_BASE + offset),
            AccessWidth::Byte,
            value,
        )
        .map_err(Stage4FlatProbeLoadError::UartConfiguration)?;
    }
    Ok(())
}
