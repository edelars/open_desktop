//! S4-27i5: shared loader for the flat interrupt probes of S4-27i1/i2/i3.
//!
//! The pump, timer and queue probes load the same flat identity-mapped
//! Long64/CPL0 image shape (GDT, an IDT that holds only the probe's vectors,
//! one code page) and differ only in their constants, the vectors they allow,
//! and whether the session owns a virtual clock (and whether the Local APIC
//! gets a shared-clock timer). This module owns that common part; the three
//! probe modules keep their public constants, error enums and entry points as
//! thin wrappers. S4-24e (UART IRQ probe) shares only the entry state and the
//! access context.

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

pub(crate) const FLAT_GDT_GPA: u64 = 0x2000;
pub(crate) const FLAT_IDT_GPA: u64 = 0x3000;
pub(crate) const FLAT_CODE_GPA: u64 = 0x4000;
pub(crate) const FLAT_GDT: [u64; 3] = [0, 0x00af_9b00_0000_ffff, 0x00cf_9300_0000_ffff];

/// Failure of the shared flat loader; each probe maps it into its own enum.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum FlatProbeError {
    ImageSize { expected: usize, actual: usize },
    Layout(&'static str),
    Memory(MemoryAccessError),
    BusBuild(BusBuildError),
    Session(String),
}

/// Virtual clock owned by the session, optionally feeding a Local APIC timer.
#[derive(Clone, Copy)]
pub(crate) struct FlatProbeClock {
    pub tsc_hz: u64,
    /// `Some(hz)`: the Local APIC gets a shared-clock timer at this frequency.
    pub timer_hz: Option<u64>,
}

/// Everything that differs between the flat interrupt probes.
#[derive(Clone, Copy)]
pub(crate) struct FlatProbeSpec {
    pub image_gpa: u64,
    pub image_bytes: usize,
    pub ram_bytes: u64,
    pub entry: u64,
    pub stack_top: u64,
    /// The only IDT vectors that may hold a gate.
    pub vectors: &'static [u8],
    /// Layout error text for a gate outside `vectors`.
    pub extra_gate_message: &'static str,
    pub clock: Option<FlatProbeClock>,
}

fn layout(reason: &'static str) -> FlatProbeError {
    FlatProbeError::Layout(reason)
}

/// Check the fixed layout: size, GDT, one present DPL0 interrupt gate per
/// allowed vector (selector 8, IST 0, handler in the code page), no other gate,
/// and no non-zero byte outside GDT, IDT and the code page.
pub(crate) fn validate_flat_image(
    image: &[u8],
    spec: &FlatProbeSpec,
) -> Result<(), FlatProbeError> {
    if image.len() != spec.image_bytes {
        return Err(FlatProbeError::ImageSize {
            expected: spec.image_bytes,
            actual: image.len(),
        });
    }
    let at = |gpa: u64, len: usize| {
        let offset = (gpa - spec.image_gpa) as usize;
        &image[offset..offset + len]
    };
    for (index, descriptor) in FLAT_GDT.iter().enumerate() {
        if at(FLAT_GDT_GPA + 8 * index as u64, 8) != descriptor.to_le_bytes() {
            return Err(layout("GDT descriptors do not match the fixed profile"));
        }
    }
    let idt = at(FLAT_IDT_GPA, 0x1000);
    for (slot, gate) in idt.chunks(16).enumerate() {
        if spec
            .vectors
            .iter()
            .any(|vector| usize::from(*vector) == slot)
        {
            // Present, DPL0, 64-bit interrupt gate, selector 8, IST 0, handler
            // inside the code page.
            let offset = u64::from(u16::from_le_bytes([gate[0], gate[1]]));
            let selector = u16::from_le_bytes([gate[2], gate[3]]);
            if selector != 8
                || gate[4] != 0
                || gate[5] != 0x8e
                || gate[6..16] != [0; 10]
                || !(FLAT_CODE_GPA..FLAT_CODE_GPA + 0x1000).contains(&offset)
            {
                return Err(layout("IDT gate is outside the profile"));
            }
        } else if gate.iter().any(|byte| *byte != 0) {
            return Err(layout(spec.extra_gate_message));
        }
    }
    let permitted = [
        (FLAT_GDT_GPA, 0x18_u64),
        (FLAT_IDT_GPA, 0x1000),
        (FLAT_CODE_GPA, 0x1000),
    ];
    for (offset, byte) in image.iter().enumerate() {
        let gpa = spec.image_gpa + offset as u64;
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

/// Fixed Long64/CPL0 entry state: IF=0, STI shadow Clear, flat CS/SS caches,
/// GDTR at the fixed GDT, RSP at `stack_top`, and the given IDTR.
pub(crate) fn flat_entry_state(entry: u64, stack_top: u64, idtr: Idtr) -> LongModeState {
    let mut state = LongModeState {
        rip: entry,
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
            base: FLAT_GDT_GPA,
            limit: 0x17,
        }),
        idtr: Some(idtr),
        activity: Some(CpuActivity::Running),
        sti_shadow: Some(StiShadow::Clear),
        ..LongModeState::default()
    };
    state.gpr[Register::Rsp as usize] = stack_top;
    state
}

pub(crate) fn flat_access_context() -> AccessContext {
    AccessContext {
        mode: InstructionMode::Long64,
        privilege: PrivilegeLevel::Ring0,
        origin: AccessOrigin::Explicit,
    }
}

/// Validate the image, place it at `spec.image_gpa`, install IDTR in the
/// initial state (the pump does not execute LIDT) and return the pump with the
/// Local APIC software-disabled, watching `spec.vectors`.
pub(crate) fn build_flat_pump(
    image: &[u8],
    spec: &FlatProbeSpec,
) -> Result<InterruptPump, FlatProbeError> {
    validate_flat_image(image, spec)?;
    let region = |base: u64, len: u64, kind| {
        GuestPhysicalRange::new(GuestPhysicalAddress::new(base), len)
            .map(|range| MemoryRegionSpec::new(range, kind))
            .map_err(FlatProbeError::Memory)
    };
    let map = MachineMemoryMap::new(
        36,
        vec![
            region(0, spec.ram_bytes, MemoryRegionKind::Ram)?,
            region(
                LOCAL_APIC_MMIO_BASE,
                LOCAL_APIC_MMIO_SIZE,
                MemoryRegionKind::Mmio,
            )?,
        ],
    )
    .map_err(FlatProbeError::Memory)?;
    let session_error = |error: &dyn std::fmt::Debug| FlatProbeError::Session(format!("{error:?}"));
    let clock = spec
        .clock
        .map(|clock| {
            VirtualClockHandle::new(clock.tsc_hz)
                .map(|handle| (handle, clock.timer_hz))
                .map_err(|error| session_error(&error))
        })
        .transpose()?;
    let local = match &clock {
        Some((handle, Some(timer_hz))) => LocalApic::with_shared_timer(handle.clone(), *timer_hz)
            .map_err(|error| session_error(&error))?,
        _ => LocalApic::new(),
    };
    let apic = local.handle();
    let mut bus = MachineBus::new(
        map,
        RamLimits::new(spec.ram_bytes),
        vec![Box::new(local)],
        vec![MmioBinding {
            region_start: GuestPhysicalAddress::new(LOCAL_APIC_MMIO_BASE),
            device_index: 0,
        }],
        vec![],
    )
    .map_err(FlatProbeError::BusBuild)?;
    bus.patch_physical_ram(GuestPhysicalAddress::new(spec.image_gpa), image)
        .map_err(FlatProbeError::Memory)?;

    let state = flat_entry_state(
        spec.entry,
        spec.stack_top,
        Idtr {
            base: FLAT_IDT_GPA,
            limit: 0x0fff,
        },
    );
    let context = flat_access_context();
    let session = match clock {
        Some((handle, _)) => DiagnosticVcpuSession::new_with_clock_handle(
            state,
            bus,
            context,
            INTEGER_FLAGS,
            handle,
            RdtscControls { cr4_tsd: false },
        )
        .map_err(|error| session_error(&error))?,
        None => DiagnosticVcpuSession::new(state, bus, context, INTEGER_FLAGS)
            .map_err(|error| session_error(&error))?,
    };
    Ok(InterruptPump::new(session, apic).with_watched_vectors(spec.vectors))
}
