//! Display device framebuffer region seen through the CPU-facing bridges
//! (S10-03): data is RAM-like without committed-write callbacks, instruction
//! fetch and page-table structures are backend errors, never `#PF`.

use od_isa::execute::{PioU8Exit, RunExit};
use od_isa::memory::{AccessContext, AccessOrigin, InstructionMode, MemoryError, PrivilegeLevel};
use od_isa::{INTEGER_FLAGS, LongModeState, Register};
use od_machine::{
    AccessWidth, DISPLAY_FRAMEBUFFER_BASE, DISPLAY_FRAMEBUFFER_BYTES, DISPLAY_REGISTERS_BASE,
    DISPLAY_REGISTERS_BYTES, DisplayDevice, DisplayHandle, DisplayReadError, GuestPhysicalAddress,
    GuestPhysicalRange, MachineBus, MachineMemoryMap, MemoryAccessError, MemoryRegionKind,
    MemoryRegionSpec, MmioBinding, RamLimits, TrackedAccessError, step_diagnostic_pio_u8,
    step_paged_mov, step_paged_pio_u8,
};

const CR0: u64 = 0x8001_0033;
const CR4: u64 = 1 << 5;
const EFER: u64 = (1 << 8) | (1 << 10) | (1 << 11);
const RAM_SIZE: u64 = 0x20_000;
const ROOT: u64 = 0x1000;
const CODE_LINEAR: u64 = 0x0040_0000;
const CODE_PHYSICAL: u64 = 0x7000;
const DATA_LINEAR: u64 = 0x0080_0000;
const FLAGS_DEFINED: u64 = INTEGER_FLAGS & !(1 << 4);

fn range(start: u64, len: u64) -> GuestPhysicalRange {
    GuestPhysicalRange::new(GuestPhysicalAddress::new(start), len).unwrap()
}

fn make_bus() -> (MachineBus, DisplayHandle) {
    let (device, handle) = DisplayDevice::new_with_handle();
    let map = MachineMemoryMap::new(
        36,
        vec![
            MemoryRegionSpec::new(range(0, RAM_SIZE), MemoryRegionKind::Ram),
            MemoryRegionSpec::new(
                range(DISPLAY_FRAMEBUFFER_BASE, DISPLAY_FRAMEBUFFER_BYTES),
                MemoryRegionKind::Framebuffer,
            ),
            MemoryRegionSpec::new(
                range(DISPLAY_REGISTERS_BASE, DISPLAY_REGISTERS_BYTES),
                MemoryRegionKind::Mmio,
            ),
        ],
    )
    .unwrap();
    let bus = MachineBus::new(
        map,
        RamLimits::new(RAM_SIZE),
        vec![Box::new(device)],
        vec![MmioBinding {
            region_start: GuestPhysicalAddress::new(DISPLAY_REGISTERS_BASE),
            device_index: 0,
        }],
        vec![],
    )
    .unwrap();
    (bus, handle)
}

fn put(bus: &mut MachineBus, address: u64, width: AccessWidth, value: u64) {
    bus.write_physical(GuestPhysicalAddress::new(address), width, value, |_| {})
        .unwrap();
}

fn put_bytes(bus: &mut MachineBus, address: u64, bytes: &[u8]) {
    for (offset, byte) in bytes.iter().copied().enumerate() {
        put(
            bus,
            address + offset as u64,
            AccessWidth::Byte,
            u64::from(byte),
        );
    }
}

/// Maps `CODE_LINEAR` to `code_physical` and `DATA_LINEAR` to `data_physical`
/// through tables at `root` (all tables in RAM unless `root` says otherwise).
fn map_pages(bus: &mut MachineBus, root: u64, code_physical: u64, data_physical: u64) {
    put(bus, root, AccessWidth::Qword, root + 0x1007);
    put(bus, root + 0x1000, AccessWidth::Qword, root + 0x2007);
    put(bus, root + 0x2010, AccessWidth::Qword, root + 0x3007);
    put(bus, root + 0x3000, AccessWidth::Qword, code_physical | 7);
    put(bus, root + 0x2020, AccessWidth::Qword, root + 0x4007);
    put(bus, root + 0x4000, AccessWidth::Qword, data_physical | 7);
}

fn context() -> AccessContext {
    AccessContext {
        mode: InstructionMode::Long64,
        privilege: PrivilegeLevel::Ring0,
        origin: AccessOrigin::DescriptorTable,
    }
}

fn cpu(rip: u64) -> LongModeState {
    LongModeState {
        rip,
        cr0: Some(CR0),
        cr4: Some(CR4),
        ..LongModeState::default()
    }
}

fn unreadable_framebuffer(error: &TrackedAccessError) -> bool {
    matches!(
        error,
        TrackedAccessError::OperandPhysical {
            source: MemoryAccessError::UnreadableRegion(MemoryRegionKind::Framebuffer),
            ..
        }
    )
}

#[test]
fn paged_mov_reads_and_writes_framebuffer_data_without_operand_callbacks() {
    let (mut bus, _display) = make_bus();
    let pixel_physical = DISPLAY_FRAMEBUFFER_BASE + 0x1000;
    map_pages(&mut bus, ROOT, CODE_PHYSICAL, pixel_physical);
    put(
        &mut bus,
        pixel_physical,
        AccessWidth::Qword,
        0x0011_2233_4455_6677,
    );

    // mov rax, [rbx]; mov [rbx], rcx
    put_bytes(
        &mut bus,
        CODE_PHYSICAL,
        &[0x48, 0x8b, 0x03, 0x48, 0x89, 0x0b],
    );
    let mut state = cpu(CODE_LINEAR);
    state.gpr[Register::Rbx as usize] = DATA_LINEAR;
    state.gpr[Register::Rcx as usize] = 0xAABB_CCDD_EEFF_0011;
    state.rflags = 0x2d7;
    let mut callbacks = Vec::new();

    let read = step_paged_mov(
        &mut state,
        &mut bus,
        ROOT,
        EFER,
        context(),
        FLAGS_DEFINED,
        |written| callbacks.push(written),
    )
    .unwrap();
    assert_eq!(read.cpu.exit, RunExit::StopAddress);
    assert_eq!(read.memory_error, None);
    assert_eq!(state.gpr[Register::Rax as usize], 0x0011_2233_4455_6677);

    let write = step_paged_mov(
        &mut state,
        &mut bus,
        ROOT,
        EFER,
        context(),
        FLAGS_DEFINED,
        |written| callbacks.push(written),
    )
    .unwrap();
    assert_eq!(write.cpu.exit, RunExit::StopAddress);
    assert_eq!(write.memory_error, None);
    assert_eq!(
        bus.read_physical(
            GuestPhysicalAddress::new(pixel_physical),
            AccessWidth::Qword
        ),
        Ok(0xAABB_CCDD_EEFF_0011)
    );
    // Only page-table accessed/dirty bits (RAM) reach the callback.
    assert!(!callbacks.is_empty());
    for written in &callbacks {
        assert!(
            written.start().get() < RAM_SIZE,
            "callback outside RAM: {written:?}"
        );
    }
}

#[test]
fn paged_instruction_fetch_from_framebuffer_is_a_backend_error_not_a_page_fault() {
    let (mut bus, _display) = make_bus();
    map_pages(
        &mut bus,
        ROOT,
        DISPLAY_FRAMEBUFFER_BASE,
        DISPLAY_FRAMEBUFFER_BASE + 0x1000,
    );
    put_bytes(&mut bus, DISPLAY_FRAMEBUFFER_BASE, &[0xec]);
    let mut state = cpu(CODE_LINEAR);
    state.rflags = 0x2d7;
    let before = state.clone();
    let report = step_paged_pio_u8(
        &mut state,
        &mut bus,
        ROOT,
        EFER,
        context(),
        FLAGS_DEFINED,
        |_| {},
    )
    .unwrap();
    assert!(matches!(
        report.cpu.exit,
        PioU8Exit::MemoryError(MemoryError::Backend(_))
    ));
    assert_eq!(report.cpu.instructions_executed, 0);
    assert_eq!(report.page_fault, None);
    assert!(unreadable_framebuffer(
        report.memory_error.as_ref().unwrap()
    ));
    assert_eq!(state, before);
}

#[test]
fn page_tables_inside_framebuffer_are_a_backend_error_and_get_no_ad_writes() {
    let (mut bus, _display) = make_bus();
    // Valid-looking tables, but located in framebuffer memory.
    let root = DISPLAY_FRAMEBUFFER_BASE;
    map_pages(&mut bus, root, CODE_PHYSICAL, 0x9000);
    let snapshot = bus
        .read_physical(GuestPhysicalAddress::new(root), AccessWidth::Qword)
        .unwrap();
    put_bytes(&mut bus, CODE_PHYSICAL, &[0xec]);
    let mut state = cpu(CODE_LINEAR);
    let mut callbacks = Vec::new();
    let report = step_paged_pio_u8(
        &mut state,
        &mut bus,
        root,
        EFER,
        context(),
        FLAGS_DEFINED,
        |w| callbacks.push(w),
    )
    .unwrap();
    assert!(matches!(
        report.cpu.exit,
        PioU8Exit::MemoryError(MemoryError::Backend(_))
    ));
    assert_eq!(report.page_fault, None);
    assert!(matches!(
        report.memory_error,
        Some(TrackedAccessError::TableRead {
            source: MemoryAccessError::UnreadableRegion(MemoryRegionKind::Framebuffer),
            ..
        })
    ));
    assert!(callbacks.is_empty());
    assert_eq!(
        bus.read_physical(GuestPhysicalAddress::new(root), AccessWidth::Qword),
        Ok(snapshot)
    );
}

#[test]
fn diagnostic_pio_fetch_from_framebuffer_is_rejected() {
    let (mut bus, _display) = make_bus();
    put_bytes(&mut bus, DISPLAY_FRAMEBUFFER_BASE, &[0xec]);
    let mut state = LongModeState {
        rip: DISPLAY_FRAMEBUFFER_BASE,
        ..LongModeState::default()
    };
    state.rflags = 0x202;
    let before = state.clone();
    let result = step_diagnostic_pio_u8(&mut state, &mut bus, context(), FLAGS_DEFINED);
    assert!(matches!(
        result.exit,
        PioU8Exit::MemoryError(MemoryError::Backend(_))
    ));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
}

#[test]
fn guest_register_flow_and_host_frame_read_through_public_api() {
    let (mut bus, display) = make_bus();
    let reg = |offset: u64| GuestPhysicalAddress::new(DISPLAY_REGISTERS_BASE + offset);
    for (offset, value) in [(0x40, 2u64), (0x44, 2), (0x48, 1), (0x4C, 1)] {
        bus.write_physical(reg(offset), AccessWidth::Dword, value, |_| {})
            .unwrap();
    }
    let mut frame = vec![0u8; 16];
    assert_eq!(
        display.read_frame_into(&bus, &mut frame),
        Err(DisplayReadError::NoFrame)
    );
    for pixel in 0..4u64 {
        bus.write_physical(
            GuestPhysicalAddress::new(DISPLAY_FRAMEBUFFER_BASE + pixel * 4),
            AccessWidth::Dword,
            0xFF00_0000 | (pixel + 1) * 0x0101_01,
            |_| panic!("pixel stores must not reach the callback"),
        )
        .unwrap();
    }
    bus.write_physical(reg(0x80), AccessWidth::Dword, 1, |_| {})
        .unwrap();
    let info = display.read_frame_into(&bus, &mut frame).unwrap();
    assert_eq!((info.generation, info.mode_change_count), (1, 1));
    assert_eq!(
        (info.mode.width, info.mode.height, info.mode.stride),
        (2, 2, 8)
    );
    assert_eq!(&frame[..4], &[0x01, 0x01, 0x01, 0xFF]);
    assert_eq!(&frame[12..], &[0x04, 0x04, 0x04, 0xFF]);
}
