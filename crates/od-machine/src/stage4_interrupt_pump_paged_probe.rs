//! S4-27i4b: loader for the paged `Stage4InterruptPumpPagedProbeV1` guest.
//!
//! The image is the S4-27h/S4-27i4a layout (`Stage4PagedIrqProbeV1`: page tables,
//! main, handler, IDT, GDT, stack and the Local APIC page; 4 KiB pages, CR0/CR4 =
//! BASE, EFER = 0xD00, vector 0x41) with the main code `STI; HLT; HLT; HLT`
//! (`FB F4 F4 F4`): three halts, so the queue scenario of S4-27i3 (a due event and
//! a future one, two wakes) fits without a jump (no new ISA step). The handler is
//! the S4-27h `MOV [RBX],EAX; IRETQ` (the EOI store). The pump drives it through
//! [`PagedBackend`]; the guest never programs a device, its interrupts come from
//! the pump's IRQ event queue scheduled by the caller (as in S4-27i3).
//!
//! The vector stays 0x41 (the one IDT gate of the paged layout), not the flat
//! queue vector 0x62; the Local APIC page is software-enabled by the loader and
//! has no timer.

use crate::stage4_paged_irq_probe::{build_bus, expected_image, handoff_state, validate};
use crate::{
    AccessWidth, DeviceError, GuestPhysicalAddress, InterruptPump, LOCAL_APIC_MMIO_BASE,
    PagedBackend, STAGE4_FLAT_PROBE_CR3, STAGE4_FLAT_PROBE_EFER, STAGE4_FLAT_PROBE_RAM_BYTES,
    STAGE4_PAGED_IRQ_IMAGE_BYTES, STAGE4_PAGED_IRQ_VECTOR, Stage4FlatProbeLoadError, VirtualClock,
};

/// `STI; HLT; HLT; HLT`.
pub const STAGE4_INTERRUPT_PUMP_PAGED_MAIN_CODE: [u8; 4] = [0xfb, 0xf4, 0xf4, 0xf4];
pub const STAGE4_INTERRUPT_PUMP_PAGED_IMAGE_BYTES: usize = STAGE4_PAGED_IRQ_IMAGE_BYTES;
pub const STAGE4_INTERRUPT_PUMP_PAGED_VECTOR: u8 = STAGE4_PAGED_IRQ_VECTOR;
/// Addresses of `STI` and the three `HLT`s.
pub const STAGE4_INTERRUPT_PUMP_PAGED_STI_RIP: u64 = 0x0040_1000;
pub const STAGE4_INTERRUPT_PUMP_PAGED_HLT_RIPS: [u64; 3] = [0x0040_1001, 0x0040_1002, 0x0040_1003];
/// Start of the handler (`MOV [RBX],EAX`).
pub const STAGE4_INTERRUPT_PUMP_PAGED_HANDLER_RIP: u64 = 0x0040_2000;
/// Frequency of the virtual TSC (1 GHz: one tick per nanosecond), as in S4-27i3.
pub const STAGE4_INTERRUPT_PUMP_PAGED_TSC_HZ: u64 = 1_000_000_000;
/// Guest linear base of the stack page and its backing guest physical address.
pub const STAGE4_INTERRUPT_PUMP_PAGED_STACK_GVA: u64 = 0x0070_0000;
pub const STAGE4_INTERRUPT_PUMP_PAGED_STACK_GPA: u64 = 0xa000;

/// Initial interrupt flag of the guest.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stage4InterruptPumpPagedStart {
    /// IF=0: `STI` sets IF and the interrupt shadow; `HLT` retires in the shadow.
    InterruptsOff,
    /// IF=1: `STI` leaves no shadow; the boundary before the first `HLT` is open.
    InterruptsOn,
}

/// The one image this profile accepts.
pub fn expected_stage4_interrupt_pump_paged_image() -> Vec<u8> {
    expected_image(&STAGE4_INTERRUPT_PUMP_PAGED_MAIN_CODE)
}

/// Validate a `Stage4InterruptPumpPagedProbeV1` raw image and return the pump at
/// the entry state (Long64/CPL0, `rflags` per `start`, RSP 0x700ff8, virtual
/// clock at 0 ns, Local APIC software-enabled) watching vector 0x41.
pub fn load_stage4_interrupt_pump_paged_probe(
    image: &[u8],
    start: Stage4InterruptPumpPagedStart,
) -> Result<InterruptPump<PagedBackend>, Stage4FlatProbeLoadError> {
    if image.len() != STAGE4_INTERRUPT_PUMP_PAGED_IMAGE_BYTES {
        return Err(Stage4FlatProbeLoadError::ImageSize {
            expected: STAGE4_INTERRUPT_PUMP_PAGED_IMAGE_BYTES,
            actual: image.len(),
        });
    }
    validate(image, &STAGE4_INTERRUPT_PUMP_PAGED_MAIN_CODE)?;
    let (bus, apic) = build_bus(image)?;
    let state = handoff_state(match start {
        Stage4InterruptPumpPagedStart::InterruptsOff => 0x002,
        Stage4InterruptPumpPagedStart::InterruptsOn => 0x202,
    });
    let clock = VirtualClock::new(STAGE4_INTERRUPT_PUMP_PAGED_TSC_HZ)
        .expect("1 GHz is a valid virtual TSC frequency");
    let backend = PagedBackend::new(
        state,
        bus,
        Some(clock),
        STAGE4_FLAT_PROBE_CR3,
        STAGE4_FLAT_PROBE_EFER,
        STAGE4_FLAT_PROBE_RAM_BYTES,
    );
    Ok(InterruptPump::with_backend(backend, apic)
        .with_watched_vectors(&[STAGE4_INTERRUPT_PUMP_PAGED_VECTOR]))
}

/// Owner handoff: software-enable the Local APIC (SVR bit 8, spurious vector 0xFF),
/// exactly the write of the loader. A global disable resets the Local APIC state
/// (S4-27i3: the only way to make `request_fixed_edge` fail); the flat guest repeats
/// its SVR write itself, the paged guest has no APIC code, so the owner does it.
pub fn enable_stage4_interrupt_pump_paged_apic(
    pump: &mut InterruptPump<PagedBackend>,
) -> Result<(), DeviceError> {
    pump.backend_mut()
        .bus_mut()
        .write_physical(
            GuestPhysicalAddress::new(LOCAL_APIC_MMIO_BASE + 0xf0),
            AccessWidth::Dword,
            0x1ff,
            |_| {},
        )
        .map_err(|_| DeviceError::UnsupportedConfiguration)
}
