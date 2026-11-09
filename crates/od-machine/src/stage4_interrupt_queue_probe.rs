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

use crate::{
    BusBuildError, DiagnosticVcpuSession, GuestPhysicalAddress, GuestPhysicalRange, InterruptPump,
    LOCAL_APIC_MMIO_BASE, LOCAL_APIC_MMIO_SIZE, LocalApic, MachineBus, MachineMemoryMap,
    MemoryAccessError, MemoryRegionKind, MemoryRegionSpec, MmioBinding, RamLimits,
    VirtualClockHandle,
};
use od_isa::execute::RdtscControls;
use od_isa::memory::{AccessContext, AccessOrigin, InstructionMode, PrivilegeLevel};
use od_isa::state::{Gdtr, Idtr};
use od_isa::{
    CodeSegmentDescriptorCache, CpuActivity, INTEGER_FLAGS, LongModeState, Register,
    StackSegmentDescriptorCache, StiShadow,
};

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
const GDT_GPA: u64 = 0x2000;
const IDT_GPA: u64 = 0x3000;
const CODE_GPA: u64 = 0x4000;
const GDT: [u64; 3] = [0, 0x00af_9b00_0000_ffff, 0x00cf_9300_0000_ffff];

#[derive(Debug, Eq, PartialEq)]
pub enum Stage4InterruptQueueProbeLoadError {
    ImageSize { expected: usize, actual: usize },
    Layout(&'static str),
    Memory(MemoryAccessError),
    BusBuild(BusBuildError),
    Session(String),
}

fn layout(reason: &'static str) -> Stage4InterruptQueueProbeLoadError {
    Stage4InterruptQueueProbeLoadError::Layout(reason)
}

fn validate_image(image: &[u8]) -> Result<(), Stage4InterruptQueueProbeLoadError> {
    if image.len() != STAGE4_INTERRUPT_QUEUE_PROBE_IMAGE_BYTES {
        return Err(Stage4InterruptQueueProbeLoadError::ImageSize {
            expected: STAGE4_INTERRUPT_QUEUE_PROBE_IMAGE_BYTES,
            actual: image.len(),
        });
    }
    let at = |gpa: u64, len: usize| {
        let offset = (gpa - STAGE4_INTERRUPT_QUEUE_PROBE_IMAGE_GPA) as usize;
        &image[offset..offset + len]
    };
    for (index, descriptor) in GDT.iter().enumerate() {
        if at(GDT_GPA + 8 * index as u64, 8) != descriptor.to_le_bytes() {
            return Err(layout("GDT descriptors do not match the fixed profile"));
        }
    }
    let vectors = [usize::from(STAGE4_INTERRUPT_QUEUE_PROBE_VECTOR)];
    let idt = at(IDT_GPA, 0x1000);
    for (slot, gate) in idt.chunks(16).enumerate() {
        if vectors.contains(&slot) {
            // Present, DPL0, 64-bit interrupt gate, selector 8, IST 0, handler
            // inside the code page.
            let offset = u64::from(u16::from_le_bytes([gate[0], gate[1]]));
            let selector = u16::from_le_bytes([gate[2], gate[3]]);
            if selector != 8
                || gate[4] != 0
                || gate[5] != 0x8e
                || gate[6..16] != [0; 10]
                || !(CODE_GPA..CODE_GPA + 0x1000).contains(&offset)
            {
                return Err(layout("IDT gate is outside the profile"));
            }
        } else if gate.iter().any(|byte| *byte != 0) {
            return Err(layout(
                "IDT must contain no gate other than the queue vector",
            ));
        }
    }
    let permitted = [(GDT_GPA, 0x18_u64), (IDT_GPA, 0x1000), (CODE_GPA, 0x1000)];
    for (offset, byte) in image.iter().enumerate() {
        let gpa = STAGE4_INTERRUPT_QUEUE_PROBE_IMAGE_GPA + offset as u64;
        if *byte != 0
            && !permitted
                .iter()
                .any(|(start, len)| (*start..*start + *len).contains(&gpa))
        {
            return Err(layout("non-zero byte outside the fixed layout"));
        }
    }
    Ok(())
}

/// Validate a `Stage4InterruptQueueProbeV1` raw image, place it at GPA `0x1000`
/// and return the pump at the fixed Long64/CPL0 entry state (IF=0, shadow Clear,
/// RSP at the stack top, IDTR installed, Local APIC software-disabled, session
/// virtual clock at 0 ns).
pub fn load_stage4_interrupt_queue_probe(
    image: &[u8],
) -> Result<InterruptPump, Stage4InterruptQueueProbeLoadError> {
    validate_image(image)?;
    let region = |base: u64, len: u64, kind| {
        GuestPhysicalRange::new(GuestPhysicalAddress::new(base), len)
            .map(|range| MemoryRegionSpec::new(range, kind))
            .map_err(Stage4InterruptQueueProbeLoadError::Memory)
    };
    let map = MachineMemoryMap::new(
        36,
        vec![
            region(
                0,
                STAGE4_INTERRUPT_QUEUE_PROBE_RAM_BYTES,
                MemoryRegionKind::Ram,
            )?,
            region(
                LOCAL_APIC_MMIO_BASE,
                LOCAL_APIC_MMIO_SIZE,
                MemoryRegionKind::Mmio,
            )?,
        ],
    )
    .map_err(Stage4InterruptQueueProbeLoadError::Memory)?;
    let clock = VirtualClockHandle::new(STAGE4_INTERRUPT_QUEUE_PROBE_TSC_HZ)
        .map_err(|error| Stage4InterruptQueueProbeLoadError::Session(format!("{error:?}")))?;
    let local = LocalApic::new();
    let apic = local.handle();
    let mut bus = MachineBus::new(
        map,
        RamLimits::new(STAGE4_INTERRUPT_QUEUE_PROBE_RAM_BYTES),
        vec![Box::new(local)],
        vec![MmioBinding {
            region_start: GuestPhysicalAddress::new(LOCAL_APIC_MMIO_BASE),
            device_index: 0,
        }],
        vec![],
    )
    .map_err(Stage4InterruptQueueProbeLoadError::BusBuild)?;
    bus.patch_physical_ram(
        GuestPhysicalAddress::new(STAGE4_INTERRUPT_QUEUE_PROBE_IMAGE_GPA),
        image,
    )
    .map_err(Stage4InterruptQueueProbeLoadError::Memory)?;

    let mut state = LongModeState {
        rip: STAGE4_INTERRUPT_QUEUE_PROBE_ENTRY,
        rflags: 2,
        cs_selector: Some(8),
        cs_cache: Some(CodeSegmentDescriptorCache {
            base: 0,
            limit: u32::MAX,
            access: 0x9b,
            flags: 2,
        }),
        ss_selector: Some(0x10),
        ss_cache: Some(StackSegmentDescriptorCache {
            base: 0,
            limit: u32::MAX,
            access: 0x93,
            flags: 0,
        }),
        gdtr: Some(Gdtr {
            base: GDT_GPA,
            limit: 0x17,
        }),
        idtr: Some(Idtr {
            base: IDT_GPA,
            limit: 0x0fff,
        }),
        activity: Some(CpuActivity::Running),
        sti_shadow: Some(StiShadow::Clear),
        ..LongModeState::default()
    };
    state.gpr[Register::Rsp as usize] = STAGE4_INTERRUPT_QUEUE_PROBE_STACK_TOP;
    let context = AccessContext {
        mode: InstructionMode::Long64,
        privilege: PrivilegeLevel::Ring0,
        origin: AccessOrigin::Explicit,
    };
    let session = DiagnosticVcpuSession::new_with_clock_handle(
        state,
        bus,
        context,
        INTEGER_FLAGS,
        clock,
        RdtscControls { cr4_tsd: false },
    )
    .map_err(|error| Stage4InterruptQueueProbeLoadError::Session(format!("{error:?}")))?;
    Ok(InterruptPump::new(session, apic)
        .with_watched_vectors(&[STAGE4_INTERRUPT_QUEUE_PROBE_VECTOR]))
}
