//! Strict fixed-image handoff for the freestanding mixed-page CR3 probe.
//!
//! The image is the raw output of `tests/fixtures/guest/stage4/cr3_mixed_probe`
//! (built by `tools/build/build_stage4_cr3_mixed_probe.py`) and is placed
//! verbatim at GPA `0x1000`. Root A (`0x1000`) maps the code page through a
//! 4 KiB leaf; root B (`0x5000`) maps the same virtual region through one 2 MiB
//! leaf. The guest runs `MOV CR3,RAX` (to B), `MOV RAX,[RBX]` (loads root A from
//! the 2 MiB page) and `MOV CR3,RAX` (back to A) on the CPU-owned CR3 quantum.
//! With a budget of three the quantum ends there (`BudgetExhausted`, RIP `0x400009`);
//! a larger budget reaches the `UD2` sentinel at that address and raises a precise `#UD`.

use crate::{
    BusAccessError, BusBuildError, GuestPhysicalAddress, GuestPhysicalRange, MachineBus,
    MachineMemoryMap, MemoryAccessError, MemoryRegionKind, MemoryRegionSpec,
    PagedProbeQuantumReport, RamLimits, run_paged_cr3_probe_2m_ram,
};
use od_isa::memory::{AccessContext, AccessOrigin, InstructionMode, PrivilegeLevel};
use od_isa::{CpuActivity, INTEGER_FLAGS, LongModeState, Register, StiShadow};

/// Size of the fixed raw image placed at [`STAGE4_CR3_MIXED_PROBE_IMAGE_GPA`].
pub const STAGE4_CR3_MIXED_PROBE_IMAGE_BYTES: usize = 0x201000;
pub const STAGE4_CR3_MIXED_PROBE_IMAGE_GPA: u64 = 0x1000;
pub const STAGE4_CR3_MIXED_PROBE_ROOT_A: u64 = 0x1000;
pub const STAGE4_CR3_MIXED_PROBE_ROOT_B: u64 = 0x5000;
const RAM_BYTES: u64 = 0x40_0000;

#[derive(Debug)]
pub enum Stage4Cr3MixedProbeLoadError {
    /// The bytes are not exactly the fixed image (any size or content difference).
    InvalidImage,
    Memory(MemoryAccessError),
    Bus(BusBuildError),
    Placement(BusAccessError),
}

pub struct LoadedStage4Cr3MixedProbe {
    bus: MachineBus,
    state: LongModeState,
    rflags_defined: u64,
}

impl LoadedStage4Cr3MixedProbe {
    pub fn state(&self) -> &LongModeState {
        &self.state
    }

    pub fn bus_mut(&mut self) -> &mut MachineBus {
        &mut self.bus
    }

    pub fn rflags_defined(&self) -> u64 {
        self.rflags_defined
    }

    /// Run at most `budget` instructions of the CPU-owned CR3 quantum through
    /// mixed 4 KiB/2 MiB RAM mappings. Context changes are in the report.
    pub fn run(
        &mut self,
        budget: u64,
        stop: impl FnMut() -> bool,
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> PagedProbeQuantumReport {
        let report = run_paged_cr3_probe_2m_ram(
            &mut self.state,
            &mut self.bus,
            0xd00,
            AccessContext {
                mode: InstructionMode::Long64,
                privilege: PrivilegeLevel::Ring0,
                origin: AccessOrigin::Explicit,
            },
            budget,
            self.rflags_defined,
            stop,
            committed_write,
        );
        self.rflags_defined = report.rflags_defined;
        report
    }
}

fn expected_image() -> Vec<u8> {
    let mut image = vec![0u8; STAGE4_CR3_MIXED_PROBE_IMAGE_BYTES];
    let base = STAGE4_CR3_MIXED_PROBE_IMAGE_GPA as usize;
    for (gpa, value) in [
        (0x1000usize, 0x2003u64),
        (0x2000, 0x3003),
        (0x3010, 0x4003),
        (0x4000, 0xb001),
        (0x5000, 0x6003),
        (0x6000, 0x7003),
        (0x7010, 0x20_0083),
        (0x20_1000, 0x1000),
    ] {
        image[gpa - base..gpa - base + 8].copy_from_slice(&value.to_le_bytes());
    }
    for (gpa, bytes) in [
        // A: MOV CR3,RAX; UD2; padding; UD2 sentinel at A+9 (precise #UD when reached).
        (
            0xb000usize,
            &[0x0f, 0x22, 0xd8, 0x0f, 0x0b, 0, 0, 0, 0, 0x0f, 0x0b][..],
        ),
        // B (2 MiB leaf): padding; MOV RAX,[RBX]; MOV CR3,RAX; UD2.
        (
            0x20_0000,
            &[0, 0, 0, 0x48, 0x8b, 0x03, 0x0f, 0x22, 0xd8, 0x0f, 0x0b][..],
        ),
    ] {
        image[gpa - base..gpa - base + bytes.len()].copy_from_slice(bytes);
    }
    image
}

/// Accept only the exact fixed image, then place it verbatim at GPA `0x1000`.
/// Initial state: CPL0 Long64, CR3 = root A, RAX = root B, RBX = `0x401000`,
/// RIP = `0x400000`, CR0/CR4/EFER fixed by the CR3 profile.
pub fn load_stage4_cr3_mixed_probe(
    image: &[u8],
) -> Result<LoadedStage4Cr3MixedProbe, Stage4Cr3MixedProbeLoadError> {
    if image != expected_image().as_slice() {
        return Err(Stage4Cr3MixedProbeLoadError::InvalidImage);
    }
    let range = GuestPhysicalRange::new(GuestPhysicalAddress::new(0), RAM_BYTES)
        .map_err(Stage4Cr3MixedProbeLoadError::Memory)?;
    let map = MachineMemoryMap::new(
        36,
        vec![MemoryRegionSpec::new(range, MemoryRegionKind::Ram)],
    )
    .map_err(Stage4Cr3MixedProbeLoadError::Memory)?;
    let mut bus = MachineBus::new(map, RamLimits::new(RAM_BYTES), vec![], vec![], vec![])
        .map_err(Stage4Cr3MixedProbeLoadError::Bus)?;
    bus.write_flat_bytes(
        GuestPhysicalAddress::new(STAGE4_CR3_MIXED_PROBE_IMAGE_GPA),
        image,
        |_| {},
    )
    .map_err(Stage4Cr3MixedProbeLoadError::Placement)?;
    let mut state = LongModeState {
        rip: 0x40_0000,
        rflags: 2,
        cr0: Some(0x8001_0033),
        cr3: Some(STAGE4_CR3_MIXED_PROBE_ROOT_A),
        cr4: Some(0x20),
        sti_shadow: Some(StiShadow::Clear),
        activity: Some(CpuActivity::Running),
        ..LongModeState::default()
    };
    state.gpr[Register::Rax as usize] = STAGE4_CR3_MIXED_PROBE_ROOT_B;
    state.gpr[Register::Rbx as usize] = 0x40_1000;
    Ok(LoadedStage4Cr3MixedProbe {
        bus,
        state,
        rflags_defined: INTEGER_FLAGS,
    })
}
