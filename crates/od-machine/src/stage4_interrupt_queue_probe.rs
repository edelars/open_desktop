//! S4-27i3: loader for the flat `Stage4InterruptQueueProbeV1` guest.
//!
//! The image is a flat identity-mapped Long64/CPL0 guest (GDT, an IDT with the
//! queue vector below, one code page, a counter dword at 0x5000). The loader
//! installs IDTR in the initial state (the pump does not execute LIDT), maps
//! RAM and the Local APIC MMIO window, gives the session a virtual clock (the
//! session owns it; the Local APIC has no timer) and returns an
//! [`InterruptPump`] watching the queue vector. The guest never programs a
//! device: its interrupts come from the pump's IRQ event queue, scheduled by
//! the caller.
//!
//! Loading is shared with the other flat interrupt probes
//! (`stage4_interrupt_probe_common`, S4-27i5); this module keeps the probe's
//! constants, error enum and entry point.

use crate::BusBuildError;
use crate::stage4_interrupt_probe_common::{
    FlatProbeClock, FlatProbeError, FlatProbeSpec, build_flat_pump,
};
use crate::{InterruptPump, MemoryAccessError};

pub const STAGE4_INTERRUPT_QUEUE_PROBE_IMAGE_GPA: u64 = 0x1000;
pub const STAGE4_INTERRUPT_QUEUE_PROBE_IMAGE_BYTES: usize = 0x4000;
pub const STAGE4_INTERRUPT_QUEUE_PROBE_RAM_BYTES: u64 = 0x7000;
pub const STAGE4_INTERRUPT_QUEUE_PROBE_ENTRY: u64 = 0x4000;
pub const STAGE4_INTERRUPT_QUEUE_PROBE_STACK_TOP: u64 = 0x6ff0;
/// Fixed vector of the queued IRQ events (priority class 6); distinct from the
/// S4-27i1 vectors 0x41 and 0x51 and the S4-27i2 timer vector 0x61.
pub const STAGE4_INTERRUPT_QUEUE_PROBE_VECTOR: u8 = 0x62;
/// Guest RAM address of the handler's wake-up counter dword.
pub const STAGE4_INTERRUPT_QUEUE_PROBE_COUNTER_GPA: u64 = 0x5000;
/// Frequency of the virtual TSC (1 GHz: one tick per nanosecond).
pub const STAGE4_INTERRUPT_QUEUE_PROBE_TSC_HZ: u64 = 1_000_000_000;

#[derive(Debug, Eq, PartialEq)]
pub enum Stage4InterruptQueueProbeLoadError {
    ImageSize { expected: usize, actual: usize },
    Layout(&'static str),
    Memory(MemoryAccessError),
    BusBuild(BusBuildError),
    Session(String),
}

impl From<FlatProbeError> for Stage4InterruptQueueProbeLoadError {
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
    image_gpa: STAGE4_INTERRUPT_QUEUE_PROBE_IMAGE_GPA,
    image_bytes: STAGE4_INTERRUPT_QUEUE_PROBE_IMAGE_BYTES,
    ram_bytes: STAGE4_INTERRUPT_QUEUE_PROBE_RAM_BYTES,
    entry: STAGE4_INTERRUPT_QUEUE_PROBE_ENTRY,
    stack_top: STAGE4_INTERRUPT_QUEUE_PROBE_STACK_TOP,
    vectors: &[STAGE4_INTERRUPT_QUEUE_PROBE_VECTOR],
    extra_gate_message: "IDT must contain no gate other than the queue vector",
    clock: Some(FlatProbeClock {
        tsc_hz: STAGE4_INTERRUPT_QUEUE_PROBE_TSC_HZ,
        timer_hz: None,
    }),
};

/// Validate a `Stage4InterruptQueueProbeV1` raw image, place it at GPA `0x1000`
/// and return the pump at the fixed Long64/CPL0 entry state (IF=0, shadow Clear,
/// RSP at the stack top, IDTR installed, Local APIC software-disabled, session
/// virtual clock at 0 ns).
pub fn load_stage4_interrupt_queue_probe(
    image: &[u8],
) -> Result<InterruptPump, Stage4InterruptQueueProbeLoadError> {
    Ok(build_flat_pump(image, &SPEC)?)
}
