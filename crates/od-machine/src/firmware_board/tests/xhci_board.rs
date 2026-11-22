//! The xHCI controller on the firmware board (S8-42 wiring, S8-43 devices): configuration, memory
//! map, handoff, PCI function, registers, the two HID devices behind the ports, board reset.

use super::*;
use crate::xhci::{XHCI_PCI_BAR_BASE, XHCI_PCI_BAR_BYTES, XHCI_PCI_SLOT};
use crate::{BusAccessError, XhciConfig};

fn xhci_config() -> FirmwareBoardConfig {
    FirmwareBoardConfig {
        pci_ecam: true,
        xhci: Some(XhciConfig::default()),
        ..reference_config()
    }
}

fn board_without_xhci_config() -> FirmwareBoardConfig {
    FirmwareBoardConfig {
        xhci: None,
        ..xhci_config()
    }
}

fn loaded(config: &FirmwareBoardConfig) -> LoadedFirmwareBoard {
    load_firmware_image(&test_image(&[0xf4], &[]), config).unwrap()
}

fn cfg(register: u64) -> GuestPhysicalAddress {
    GuestPhysicalAddress::new(
        PCI_ECAM_DESCRIPTOR.base + (u64::from(XHCI_PCI_SLOT) << 15) + register,
    )
}

fn bar(offset: u64) -> GuestPhysicalAddress {
    GuestPhysicalAddress::new(XHCI_PCI_BAR_BASE + offset)
}

fn read(board: &mut LoadedFirmwareBoard, address: GuestPhysicalAddress) -> u64 {
    board
        .bus_mut()
        .read_physical(address, AccessWidth::Dword)
        .unwrap()
}

fn write(board: &mut LoadedFirmwareBoard, address: GuestPhysicalAddress, value: u64) {
    board
        .bus_mut()
        .write_physical(address, AccessWidth::Dword, value, |_| {})
        .unwrap();
}

fn write_qword(board: &mut LoadedFirmwareBoard, address: GuestPhysicalAddress, value: u64) {
    board
        .bus_mut()
        .write_physical(address, AccessWidth::Qword, value, |_| {})
        .unwrap();
}

fn put(board: &mut LoadedFirmwareBoard, address: u64, bytes: &[u8]) {
    board
        .bus_mut()
        .write_flat_bytes(GuestPhysicalAddress::new(address), bytes, |_| {})
        .unwrap();
}

fn get(board: &mut LoadedFirmwareBoard, address: u64, len: usize) -> Vec<u8> {
    let mut bytes = vec![0u8; len];
    board
        .bus_mut()
        .read_flat_bytes(GuestPhysicalAddress::new(address), &mut bytes)
        .unwrap();
    bytes
}

fn service(board: &mut LoadedFirmwareBoard) {
    let xhci = board.xhci().unwrap();
    xhci.service(board.bus_mut(), |_| {});
}

fn trb(dw: [u32; 4]) -> Vec<u8> {
    dw.iter().flat_map(|dword| dword.to_le_bytes()).collect()
}

// ---------------------------------------------------------------- configuration, map, handoff

#[test]
fn xhci_needs_the_pci_configuration_device() {
    let config = FirmwareBoardConfig {
        pci_ecam: false,
        ..xhci_config()
    };
    assert_eq!(
        firmware_handoff_block(&config).unwrap_err(),
        FirmwareBoardError::Config("xhci needs pci_ecam")
    );
}

#[test]
fn xhci_is_refused_with_the_linux_profile_until_its_loops_serve_it() {
    let config = FirmwareBoardConfig {
        linux_profile: true,
        acpi_pm: true,
        acpi_tables: true,
        ..xhci_config()
    };
    assert_eq!(
        firmware_handoff_block(&config).unwrap_err(),
        FirmwareBoardError::Config(
            "xhci is served by the firmware profile only (no linux_profile)"
        )
    );
}

#[test]
fn without_xhci_the_board_has_no_controller_window_function_or_handles() {
    let config = board_without_xhci_config();
    let layout = FirmwareLayout::for_config(&config, HANDOFF_MINOR_SECURE_BOOT).unwrap();
    assert!(
        layout
            .map
            .regions()
            .iter()
            .all(|region| region.range.start().get() != XHCI_PCI_BAR_BASE)
    );
    let mut board = loaded(&config);
    assert!(board.xhci().is_none());
    assert!(board.hid_devices().is_none());
    // The function does not answer, and the window is not mapped.
    assert_eq!(read(&mut board, cfg(0)), 0xFFFF_FFFF);
    assert!(matches!(
        board.bus_mut().read_physical(bar(0), AccessWidth::Dword),
        Err(BusAccessError::UnmappedPhysical(_))
    ));
}

#[test]
fn the_controller_changes_only_the_region_table_of_the_handoff_and_not_the_acpi_manifest() {
    let without = board_without_xhci_config();
    let with = xhci_config();
    let handoff = |config: &FirmwareBoardConfig| firmware_handoff_block(config).unwrap();
    let (block_without, block_with) = (handoff(&without), handoff(&with));
    // The device table (and so the ACPI manifest hash) is the same: no new device code.
    let count = configured_board_devices(&without).len();
    assert_eq!(configured_board_devices(&with).len(), count);
    for index in 0..count {
        assert_eq!(
            device_entry(&block_with, index),
            device_entry(&block_without, index),
            "device entry {index}"
        );
    }
    assert_eq!(
        with.acpi_manifest_hash().unwrap(),
        without.acpi_manifest_hash().unwrap()
    );
    assert_eq!(
        le_u16(&block_with, 0x0a),
        le_u16(&block_without, 0x0a),
        "version_minor"
    );
    // The region table has exactly one more region: the BAR window, MMIO, no owner.
    let (regions_without, regions_with) = (regions_of(&block_without), regions_of(&block_with));
    assert_eq!(regions_with.len(), regions_without.len() + 1);
    let window: Vec<_> = regions_with
        .iter()
        .filter(|region| le_u64(region, 8).unwrap() == XHCI_PCI_BAR_BASE)
        .collect();
    assert_eq!(window.len(), 1);
    assert_eq!(le_u32(window[0], 0).unwrap(), REGION_MMIO);
    assert_eq!(le_u64(window[0], 16).unwrap(), XHCI_PCI_BAR_BYTES);
    assert_eq!(le_u32(window[0], 24).unwrap(), NO_OWNER);
}

#[test]
fn the_bar_window_is_mmio_and_disjoint_from_every_other_region_and_keeps_the_boot_tables() {
    let with = FirmwareLayout::for_config(&xhci_config(), HANDOFF_MINOR_SECURE_BOOT).unwrap();
    let without =
        FirmwareLayout::for_config(&board_without_xhci_config(), HANDOFF_MINOR_SECURE_BOOT)
            .unwrap();
    let window = with
        .map
        .regions()
        .iter()
        .find(|region| region.range.start().get() == XHCI_PCI_BAR_BASE)
        .expect("the BAR window is in the map");
    assert_eq!(window.kind, MemoryRegionKind::Mmio);
    assert_eq!(window.range.len(), XHCI_PCI_BAR_BYTES);
    let end = XHCI_PCI_BAR_BASE + XHCI_PCI_BAR_BYTES;
    for region in with.map.regions() {
        if region.range.start().get() == XHCI_PCI_BAR_BASE {
            continue;
        }
        let (start, len) = (region.range.start().get(), region.range.len());
        assert!(
            start + len <= XHCI_PCI_BAR_BASE || start >= end,
            "{start:#x}+{len:#x} overlaps the xHCI window"
        );
    }
    // The window is in the 2 MiB block of the display registers and ECAM: no new page table.
    assert_eq!(with.table_pages, without.table_pages);
    assert_eq!(XHCI_PCI_BAR_BASE, 0xFC00_8000);
    assert_eq!(XHCI_PCI_BAR_BYTES, 0x4000);
}

// ---------------------------------------------------------------- PCI function and registers

#[test]
fn the_function_is_read_through_ecam_with_the_contract_vendor_class_and_bar() {
    let mut board = loaded(&xhci_config());
    assert_eq!(
        read(&mut board, cfg(0)),
        0x0010_D1A6,
        "vendor D1A6, device 0010"
    );
    assert_eq!(
        read(&mut board, cfg(8)) >> 8,
        0x0C_03_30,
        "class: serial bus, USB, xHCI"
    );
    assert_eq!(
        read(&mut board, cfg(0x10)),
        XHCI_PCI_BAR_BASE | 4,
        "64-bit memory BAR0"
    );
    assert_eq!(read(&mut board, cfg(0x3C)) & 0xFF00, 0x0100, "INTA");
    assert_eq!(read(&mut board, cfg(0x34)) & 0xFF, 0, "no capability list");
    // Other slots stay empty; the NVMe slot 4 is not the xHCI.
    assert_eq!(
        read(
            &mut board,
            GuestPhysicalAddress::new(PCI_ECAM_DESCRIPTOR.base + (6 << 15))
        ),
        0xFFFF_FFFF
    );
}

#[test]
fn conf1_reaches_the_same_function() {
    let mut board = loaded(&xhci_config());
    let address = (1u64 << 31) | (u64::from(XHCI_PCI_SLOT) << 11);
    board
        .bus_mut()
        .write_port(
            PortAddress::new(PCI_CONF1_BASE_PORT),
            AccessWidth::Dword,
            address as u32,
        )
        .unwrap();
    let id = board
        .bus_mut()
        .read_port(
            PortAddress::new(PCI_CONF1_BASE_PORT + 4),
            AccessWidth::Dword,
        )
        .unwrap();
    assert_eq!(id, 0x0010_D1A6);
}

#[test]
fn the_registers_decode_after_the_driver_sets_mse() {
    let mut board = loaded(&xhci_config());
    assert_eq!(
        read(&mut board, bar(0)),
        0xFFFF_FFFF,
        "MSE = 0 reads all ones"
    );
    write(&mut board, cfg(4), 0x6);
    assert_eq!(
        read(&mut board, bar(0)),
        0x0100_0020,
        "CAPLENGTH 0x20, HCIVERSION 0x0100"
    );
    assert_eq!(
        read(&mut board, bar(4)),
        0x0200_0108,
        "8 slots, 1 interrupter, 2 ports"
    );
    assert_eq!(read(&mut board, bar(0x20 + 4)), 1, "USBSTS.HCH");
    // Both ports have a device attached since the board was created.
    for port in 0..2u64 {
        let portsc = read(&mut board, bar(0x420 + 0x10 * port));
        assert_eq!(portsc & 1, 1, "CCS of port {}", port + 1);
        assert_eq!((portsc >> 10) & 0xF, 1, "Full-speed");
    }
}

// ---------------------------------------------------------------- the two devices

const CMD_RING: u64 = 0x0200_0000;
const EVENT_RING: u64 = 0x0200_1000;
const ERST: u64 = 0x0200_2000;
const DCBAA: u64 = 0x0200_3000;
const OUTPUT: u64 = 0x0200_4000;
const INPUT: u64 = 0x0200_8000;
const EP0_RING: u64 = 0x0200_9000;
const BUF: u64 = 0x0200_A000;

/// A minimal driver on the board bus: rings, one slot, the device descriptor of a port.
struct Driver {
    event: u64,
    event_cycle: bool,
    command: u64,
}

impl Driver {
    fn start(board: &mut LoadedFirmwareBoard) -> Self {
        write(board, cfg(4), 0x6);
        // A driver hands the controller zeroed rings.
        for (address, len) in [(CMD_RING, 0x1000), (EVENT_RING, 0x1000), (EP0_RING, 0x1000)] {
            put(board, address, &vec![0u8; len]);
        }
        put(board, ERST, &{
            let mut entry = [0u8; 16];
            entry[0..8].copy_from_slice(&EVENT_RING.to_le_bytes());
            entry[8..12].copy_from_slice(&16u32.to_le_bytes());
            entry
        });
        write(board, bar(0x1028), 1);
        write_qword(board, bar(0x1030), ERST);
        write_qword(board, bar(0x1038), EVENT_RING);
        write_qword(board, bar(0x38), CMD_RING | 1);
        write_qword(board, bar(0x50), DCBAA);
        write(board, bar(0x58), 8);
        write(board, bar(0x20), 1); // R/S
        service(board);
        let mut driver = Self {
            event: 0,
            event_cycle: true,
            command: 0,
        };
        // Two Port Status Change Events are waiting.
        driver.next_event(board);
        driver.next_event(board);
        driver
    }

    fn next_event(&mut self, board: &mut LoadedFirmwareBoard) -> [u32; 4] {
        let raw = get(board, EVENT_RING + 16 * self.event, 16);
        let dw: [u32; 4] =
            std::array::from_fn(|i| u32::from_le_bytes(raw[i * 4..i * 4 + 4].try_into().unwrap()));
        assert_eq!(dw[3] & 1 != 0, self.event_cycle, "an event is waiting");
        self.event += 1;
        write_qword(board, bar(0x1038), (EVENT_RING + 16 * self.event) | 8);
        dw
    }

    fn command(&mut self, board: &mut LoadedFirmwareBoard, mut dw: [u32; 4]) -> [u32; 4] {
        dw[3] |= 1;
        put(board, CMD_RING + 16 * self.command, &trb(dw));
        self.command += 1;
        write(board, bar(0x800), 0);
        service(board);
        self.next_event(board)
    }

    /// Resets the port, enables a slot, addresses the device and reads its device descriptor.
    fn device_descriptor(&mut self, board: &mut LoadedFirmwareBoard, port: u64) -> Vec<u8> {
        let portsc = 0x420 + 0x10 * (port - 1);
        write(board, bar(portsc), 1 << 4);
        board.advance_time_ns(50_000_000).unwrap();
        assert_eq!(read(board, bar(portsc)) & 0x12, 0x2, "PED, reset done");
        // The connect event of start() was never acknowledged: the reset adds no new event.
        write(board, bar(portsc), (1 << 17) | (1 << 21));
        let slot = self.command(board, [0, 0, 0, 9 << 10])[3] >> 24;
        let output = OUTPUT + 0x1000 * u64::from(slot);
        put(board, output, &vec![0u8; 0x1000]);
        put(board, DCBAA + 8 * u64::from(slot), &output.to_le_bytes());
        let ctx = |dw: [u32; 8]| -> Vec<u8> { dw.iter().flat_map(|d| d.to_le_bytes()).collect() };
        put(board, INPUT, &ctx([0, 0b11, 0, 0, 0, 0, 0, 0]));
        put(
            board,
            INPUT + 32,
            &ctx([1 << 27, (port as u32) << 16, 0, 0, 0, 0, 0, 0]),
        );
        let ep0 = [
            0,
            (4 << 3) | (3 << 1) | (64 << 16),
            EP0_RING as u32 | 1,
            0,
            8,
            0,
            0,
            0,
        ];
        put(board, INPUT + 64, &ctx(ep0));
        let address = self.command(board, [INPUT as u32, 0, 0, (slot << 24) | (11 << 10)]);
        assert_eq!(
            (address[2] >> 24, address[3] >> 24),
            (1, slot),
            "Address Device: Success"
        );
        let mut ring = Vec::new();
        ring.extend(trb([
            0x0100_0680,
            18 << 16,
            8,
            (2 << 10) | (1 << 6) | (3 << 16) | 1,
        ]));
        ring.extend(trb([
            BUF as u32,
            0,
            18,
            (3 << 10) | (1 << 16) | (1 << 5) | 1,
        ]));
        ring.extend(trb([0, 0, 0, (4 << 10) | (1 << 5) | 1]));
        put(board, EP0_RING, &ring);
        write(board, bar(0x800 + 4 * u64::from(slot)), 1);
        service(board);
        let data = self.next_event(board);
        let status = self.next_event(board);
        assert_eq!(
            (data[2] >> 24, status[2] >> 24),
            (1, 1),
            "both stages succeed"
        );
        get(board, BUF, 18)
    }
}

#[test]
fn port_1_is_a_keyboard_and_port_2_an_absolute_pointer() {
    for (port, product) in [(1u64, 0x0101u16), (2, 0x0102)] {
        let mut board = loaded(&xhci_config());
        let mut driver = Driver::start(&mut board);
        let descriptor = driver.device_descriptor(&mut board, port);
        assert_eq!(&descriptor[..2], &[18, 1], "a device descriptor");
        assert_eq!(
            u16::from_le_bytes([descriptor[8], descriptor[9]]),
            0xD1A6,
            "idVendor"
        );
        assert_eq!(
            u16::from_le_bytes([descriptor[10], descriptor[11]]),
            product,
            "idProduct of port {port}"
        );
    }
}

#[test]
fn the_host_handles_reach_the_devices_behind_the_ports() {
    let mut board = loaded(&xhci_config());
    let devices = board.hid_devices().unwrap();
    // An event outside the Usage ID range is dropped and counted by the keyboard only.
    let xhci = board.xhci().unwrap();
    xhci.push_input(crate::InputEvent::Key {
        usage: 0,
        pressed: true,
    })
    .unwrap();
    service(&mut board);
    assert_eq!(devices.keyboard.dropped_events(), 1);
    assert_eq!(devices.pointer.dropped_events(), 0);
}

// ---------------------------------------------------------------- run loop and reset

#[test]
fn a_guest_register_write_wakes_the_loop_until_the_next_service() {
    let mut board = loaded(&xhci_config());
    let xhci = board.xhci().unwrap();
    assert!(!xhci.wakeup_pending());
    write(&mut board, cfg(4), 0x6);
    write(&mut board, bar(0x800), 0);
    assert!(xhci.wakeup_pending(), "a doorbell ends the quantum");
    service(&mut board);
    assert!(!xhci.wakeup_pending());
}

#[test]
fn board_reset_returns_the_controller_and_its_pci_function_to_the_power_on_state() {
    let mut board = loaded(&xhci_config());
    let mut driver = Driver::start(&mut board);
    driver.device_descriptor(&mut board, 1);
    write(&mut board, bar(0x34), 0x1234); // DNCTRL
    assert_eq!(read(&mut board, cfg(4)) & 0x6, 0x6);
    board.platform_reset(PlatformResetKind::Warm).unwrap();
    // PCI configuration as at board creation: decoding off, BAR0 programmed.
    assert_eq!(read(&mut board, cfg(4)) & 0xFFFF, 0);
    assert_eq!(read(&mut board, cfg(0x10)), XHCI_PCI_BAR_BASE | 4);
    assert_eq!(read(&mut board, bar(0)), 0xFFFF_FFFF);
    write(&mut board, cfg(4), 0x6);
    assert_eq!(read(&mut board, bar(0x20)), 0, "USBCMD");
    assert_eq!(read(&mut board, bar(0x24)), 1, "USBSTS: halted");
    assert_eq!(read(&mut board, bar(0x34)), 0, "DNCTRL");
    assert_eq!(read(&mut board, bar(0x58)), 0, "CONFIG");
    assert_eq!(read(&mut board, bar(0x50)), 0, "DCBAAP");
    assert_eq!(
        read(&mut board, bar(0x420)),
        0x0002_06E1,
        "port 1: attached, not reset"
    );
    assert!(!board.xhci().unwrap().wakeup_pending());
    // The same controller works again from the reset state: a new driver enumerates a device.
    let mut driver = Driver::start(&mut board);
    assert_eq!(driver.device_descriptor(&mut board, 2)[10], 0x02);
}

#[test]
fn the_firmware_run_loop_executes_a_command_the_guest_rings_with_its_doorbell() {
    // The guest only rings doorbell 0 (mov edi, BAR+0x800; mov dword [rdi], 0; hlt): everything
    // else is set up by the host, as a firmware driver would do before.
    let doorbell = (XHCI_PCI_BAR_BASE + 0x800) as u32;
    let mut code = vec![0xBF];
    code.extend(doorbell.to_le_bytes());
    code.extend([0xC7, 0x07, 0, 0, 0, 0, 0xF4]);
    let mut board = load_firmware_image(&test_image(&code, &[]), &xhci_config()).unwrap();
    write(&mut board, cfg(4), 0x6);
    for (address, len) in [(CMD_RING, 0x1000), (EVENT_RING, 0x1000)] {
        put(&mut board, address, &vec![0u8; len]);
    }
    let mut entry = [0u8; 16];
    entry[0..8].copy_from_slice(&EVENT_RING.to_le_bytes());
    entry[8..12].copy_from_slice(&16u32.to_le_bytes());
    put(&mut board, ERST, &entry);
    write(&mut board, bar(0x1028), 1);
    write_qword(&mut board, bar(0x1030), ERST);
    write_qword(&mut board, bar(0x1038), EVENT_RING);
    write_qword(&mut board, bar(0x38), CMD_RING | 1);
    write(&mut board, bar(0x20), 1);
    put(&mut board, CMD_RING, &trb([0, 0, 0, (23 << 10) | 1]));
    let report = board.run_boot(1000).unwrap();
    assert!(
        matches!(report.exit, PagedProbeExit::Halted),
        "{:?}",
        report.exit
    );
    // Two Port Status Change Events (the first service), then the No Op completion.
    let completion = get(&mut board, EVENT_RING + 32, 16);
    let dw3 = u32::from_le_bytes(completion[12..16].try_into().unwrap());
    assert_eq!((dw3 >> 10) & 0x3F, 33, "Command Completion Event");
    assert_eq!(dw3 & 1, 1);
    assert_eq!(completion[11], 1, "Success");
    assert_eq!(
        u64::from_le_bytes(completion[0..8].try_into().unwrap()),
        CMD_RING,
        "the address of the No Op TRB"
    );
}

#[test]
fn firmware_code_enumerates_the_function_through_the_ecam_window() {
    // mov edi, ECAM + (5 << 15); mov eax, [rdi] (vendor and device); mov [0x0300_0000], eax;
    // mov eax, [rdi + 8] (revision and class); mov [0x0300_0004], eax; hlt.
    let function = (PCI_ECAM_DESCRIPTOR.base + (u64::from(XHCI_PCI_SLOT) << 15)) as u32;
    let mut code = vec![0xBF];
    code.extend(function.to_le_bytes());
    code.extend([0x8B, 0x07, 0x89, 0x04, 0x25, 0, 0, 0, 0x03]);
    code.extend([0x8B, 0x47, 0x08, 0x89, 0x04, 0x25, 4, 0, 0, 0x03, 0xF4]);
    let mut board = load_firmware_image(&test_image(&code, &[]), &xhci_config()).unwrap();
    let report = board.run_boot(1000).unwrap();
    assert!(
        matches!(report.exit, PagedProbeExit::Halted),
        "{:?}",
        report.exit
    );
    assert_eq!(
        get(&mut board, 0x0300_0000, 4),
        0x0010_D1A6u32.to_le_bytes()
    );
    assert_eq!(
        get(&mut board, 0x0300_0004, 4),
        0x0C03_3001u32.to_le_bytes(),
        "class 0C0330, revision 1"
    );
}

#[test]
fn firmware_code_programs_command_through_ecam_and_then_reads_the_bar() {
    // The firmware driver sequence: Command = MSE | BME through ECAM, then a BAR register.
    // mov edi, ECAM+(5<<15); mov dword [rdi+4], 6; mov edi, BAR; mov eax, [rdi]; store; hlt.
    let function = (PCI_ECAM_DESCRIPTOR.base + (u64::from(XHCI_PCI_SLOT) << 15)) as u32;
    let mut code = vec![0xBF];
    code.extend(function.to_le_bytes());
    code.extend([0xC7, 0x47, 0x04, 6, 0, 0, 0, 0xBF]);
    code.extend((XHCI_PCI_BAR_BASE as u32).to_le_bytes());
    code.extend([0x8B, 0x07, 0x89, 0x04, 0x25, 0, 0, 0, 0x03, 0xF4]);
    let mut board = load_firmware_image(&test_image(&code, &[]), &xhci_config()).unwrap();
    let report = board.run_boot(1000).unwrap();
    assert!(
        matches!(report.exit, PagedProbeExit::Halted),
        "{:?}",
        report.exit
    );
    assert_eq!(
        get(&mut board, 0x0300_0000, 4),
        0x0100_0020u32.to_le_bytes()
    );
}

#[test]
fn firmware_code_reaches_the_bar_registers_through_the_exact_window() {
    // mov edi, BAR; mov eax, [rdi] (CAPLENGTH and HCIVERSION); mov [0x0300_0000], eax; hlt.
    // Command.MSE is set by the host, as the firmware driver would do through PCI first.
    let mut code = vec![0xBF];
    code.extend((XHCI_PCI_BAR_BASE as u32).to_le_bytes());
    code.extend([0x8B, 0x07, 0x89, 0x04, 0x25, 0, 0, 0, 0x03, 0xF4]);
    let mut board = load_firmware_image(&test_image(&code, &[]), &xhci_config()).unwrap();
    write(&mut board, cfg(4), 0x6);
    let report = board.run_boot(1000).unwrap();
    assert!(
        matches!(report.exit, PagedProbeExit::Halted),
        "{:?}",
        report.exit
    );
    assert_eq!(
        get(&mut board, 0x0300_0000, 4),
        0x0100_0020u32.to_le_bytes()
    );
}
