//! S4-27i1: loader for the flat `Stage4InterruptPumpProbeV1` guest.
//!
//! The image is a flat identity-mapped Long64/CPL0 guest (GDT, an IDT with the
//! two vectors below, one code page). The loader installs IDTR in the initial
//! state (the pump does not execute LIDT), maps RAM and the Local APIC MMIO
//! window and returns an [`InterruptPump`] watching both vectors. Which scenario
//! the guest code implements is the image's business; the pump is generic.
//!
//! Loading is shared with the other flat interrupt probes
//! (`stage4_interrupt_probe_common`, S4-27i5); this module keeps the probe's
//! constants, error enum and entry point.

use crate::BusBuildError;
use crate::stage4_interrupt_probe_common::{FlatProbeError, FlatProbeSpec, build_flat_pump};
use crate::{InterruptPump, MemoryAccessError};

pub const STAGE4_INTERRUPT_PUMP_PROBE_IMAGE_GPA: u64 = 0x1000;
pub const STAGE4_INTERRUPT_PUMP_PROBE_IMAGE_BYTES: usize = 0x4000;
pub const STAGE4_INTERRUPT_PUMP_PROBE_RAM_BYTES: u64 = 0x7000;
pub const STAGE4_INTERRUPT_PUMP_PROBE_ENTRY: u64 = 0x4000;
pub const STAGE4_INTERRUPT_PUMP_PROBE_STACK_TOP: u64 = 0x6ff0;
/// Vector with the higher priority class (0x5): its handler raises `VECTOR_B`.
pub const STAGE4_INTERRUPT_PUMP_PROBE_VECTOR_A: u8 = 0x51;
/// Vector with the lower priority class (0x4).
pub const STAGE4_INTERRUPT_PUMP_PROBE_VECTOR_B: u8 = 0x41;

#[derive(Debug, Eq, PartialEq)]
pub enum Stage4InterruptPumpProbeLoadError {
    ImageSize { expected: usize, actual: usize },
    Layout(&'static str),
    Memory(MemoryAccessError),
    BusBuild(BusBuildError),
    Session(String),
}

impl From<FlatProbeError> for Stage4InterruptPumpProbeLoadError {
    fn from(error: FlatProbeError) -> Self {
        match error {
            FlatProbeError::ImageSize { expected, actual } => Self::ImageSize { expected, actual },
            FlatProbeError::Layout(reason) => Self::Layout(reason),
            FlatProbeError::Memory(error) => Self::Memory(error),
            FlatProbeError::BusBuild(error) => Self::BusBuild(error),
            FlatProbeError::Session(detail) => Self::Session(detail),
        }
    }
}

const SPEC: FlatProbeSpec = FlatProbeSpec {
    image_gpa: STAGE4_INTERRUPT_PUMP_PROBE_IMAGE_GPA,
    image_bytes: STAGE4_INTERRUPT_PUMP_PROBE_IMAGE_BYTES,
    ram_bytes: STAGE4_INTERRUPT_PUMP_PROBE_RAM_BYTES,
    entry: STAGE4_INTERRUPT_PUMP_PROBE_ENTRY,
    stack_top: STAGE4_INTERRUPT_PUMP_PROBE_STACK_TOP,
    vectors: &[
        STAGE4_INTERRUPT_PUMP_PROBE_VECTOR_B,
        STAGE4_INTERRUPT_PUMP_PROBE_VECTOR_A,
    ],
    extra_gate_message: "IDT must contain no gate other than the two vectors",
    clock: None,
};

/// Validate a `Stage4InterruptPumpProbeV1` raw image, place it at GPA `0x1000`
/// and return the pump at the fixed Long64/CPL0 entry state (IF=0, shadow Clear,
/// RSP at the stack top, IDTR installed, Local APIC software-disabled).
pub fn load_stage4_interrupt_pump_probe(
    image: &[u8],
) -> Result<InterruptPump, Stage4InterruptPumpProbeLoadError> {
    Ok(build_flat_pump(image, &SPEC)?)
}
