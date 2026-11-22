//! The NIC on the firmware board (S11-32 wiring): configuration, memory map, handoff, PCI
//! function, registers, the run loop that services the rings, board reset.

use super::*;
use crate::nic_e1000::{NIC_PCI_BAR_BASE, NIC_PCI_BAR_BYTES, NIC_PCI_SLOT};
use crate::{BusAccessError, Dropped, NetBackend, NetCounters, NicConfig};
use std::sync::{Arc, Mutex};

const GUEST_MAC: [u8; 6] = [0x02, 0x4F, 0x44, 0x00, 0x00, 0x10];
const TX_RING: u64 = 0x0200_0000;
const TX_BUFFER: u64 = 0x0200_2000;
const RX_RING: u64 = 0x0200_4000;
const RX_BUFFER: u64 = 0x0200_6000;

fn nic_config() -> FirmwareBoardConfig {
    FirmwareBoardConfig {
        pci_ecam: true,
        nic: Some(NicConfig { mac: GUEST_MAC }),
        ..reference_config()
    }
}

fn board_without_nic_config() -> FirmwareBoardConfig {
    FirmwareBoardConfig {
        nic: None,
        ..nic_config()
    }
}

fn loaded(config: &FirmwareBoardConfig) -> LoadedFirmwareBoard {
    load_firmware_image(&test_image(&[0xf4], &[]), config).unwrap()
}

fn cfg(register: u64) -> GuestPhysicalAddress {
    GuestPhysicalAddress::new(PCI_ECAM_DESCRIPTOR.base + (u64::from(NIC_PCI_SLOT) << 15) + register)
}

fn bar(offset: u64) -> GuestPhysicalAddress {
    GuestPhysicalAddress::new(NIC_PCI_BAR_BASE + offset)
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
    let nic = board.nic().unwrap();
    nic.service(board.bus_mut(), |_| {});
}

#[derive(Default)]
struct Wire {
    sent: Vec<Vec<u8>>,
    to_guest: Vec<Vec<u8>>,
    resets: u32,
}

#[derive(Clone, Default)]
struct WireBackend(Arc<Mutex<Wire>>);

impl NetBackend for WireBackend {
    fn transmit(&mut self, frame: &[u8]) -> Result<(), Dropped> {
        self.0.lock().unwrap().sent.push(frame.to_vec());
        Ok(())
    }

    fn poll_receive(&mut self, buf: &mut [u8]) -> Option<usize> {
        let mut wire = self.0.lock().unwrap();
        if wire.to_guest.is_empty() {
            return None;
        }
        let frame = wire.to_guest.remove(0);
        buf[..frame.len()].copy_from_slice(&frame);
        Some(frame.len())
    }

    fn counters(&self) -> NetCounters {
        NetCounters::default()
    }

    fn reset(&mut self) {
        self.0.lock().unwrap().resets += 1;
    }
}

/// A frame of 60 bytes from the gateway MAC to `destination`.
fn frame_to(destination: [u8; 6]) -> Vec<u8> {
    let mut frame = vec![0xA5u8; 60];
    frame[0..6].copy_from_slice(&destination);
    frame[6..12].copy_from_slice(&crate::NIC_GATEWAY_MAC);
    frame
}

/// Memory decoding and bus mastering on, the transmit ring (8 descriptors) and the receive ring
/// (8 descriptors, 2 KiB buffers, the first at [`RX_BUFFER`]) programmed, as a driver would.
fn program_rings(board: &mut LoadedFirmwareBoard) {
    write(board, cfg(4), 0x6);
    write(board, bar(0x3800), TX_RING);
    write(board, bar(0x3804), 0);
    write(board, bar(0x3808), 8 * 16);
    write(board, bar(0x3810), 0);
    write(board, bar(0x3818), 0);
    write(board, bar(0x0400), 1 << 1);
    for index in 0..8u64 {
        let mut descriptor = [0u8; 16];
        descriptor[0..8].copy_from_slice(&(RX_BUFFER + index * 0x1000).to_le_bytes());
        put(board, RX_RING + index * 16, &descriptor);
    }
    write(board, bar(0x2800), RX_RING);
    write(board, bar(0x2804), 0);
    write(board, bar(0x2808), 8 * 16);
    write(board, bar(0x2810), 0);
    write(board, bar(0x2818), 7);
    // EN, BAM, SECRC.
    write(board, bar(0x0100), (1 << 1) | (1 << 15) | (1 << 26));
}

/// One transmit descriptor at index 0 for the frame at [`TX_BUFFER`]: EOP, RS.
fn queue_frame(board: &mut LoadedFirmwareBoard, frame: &[u8]) {
    put(board, TX_BUFFER, frame);
    let mut descriptor = [0u8; 16];
    descriptor[0..8].copy_from_slice(&TX_BUFFER.to_le_bytes());
    descriptor[8..10].copy_from_slice(&(frame.len() as u16).to_le_bytes());
    descriptor[11] = 0b1001;
    put(board, TX_RING, &descriptor);
}

// ---------------------------------------------------------------- configuration, map, handoff

#[test]
fn the_default_board_has_no_nic() {
    assert_eq!(FirmwareBoardConfig::default().nic, None);
}

#[test]
fn nic_needs_the_pci_configuration_device() {
    let config = FirmwareBoardConfig {
        pci_ecam: false,
        ..nic_config()
    };
    assert_eq!(
        firmware_handoff_block(&config).unwrap_err(),
        FirmwareBoardError::Config("nic needs pci_ecam")
    );
}

#[test]
fn nic_is_refused_with_the_linux_profile_until_its_loops_serve_it() {
    let config = FirmwareBoardConfig {
        linux_profile: true,
        acpi_pm: true,
        acpi_tables: true,
        ..nic_config()
    };
    assert_eq!(
        firmware_handoff_block(&config).unwrap_err(),
        FirmwareBoardError::Config("nic is served by the firmware profile only (no linux_profile)")
    );
}

#[test]
fn a_gateway_group_or_zero_mac_refuses_the_board() {
    for mac in [crate::NIC_GATEWAY_MAC, [0x01, 0, 0x5E, 0, 0, 1], [0; 6]] {
        let config = FirmwareBoardConfig {
            nic: Some(NicConfig { mac }),
            ..nic_config()
        };
        assert!(
            matches!(
                firmware_handoff_block(&config),
                Err(FirmwareBoardError::Config(_))
            ),
            "{mac:02x?}"
        );
    }
}

#[test]
fn without_nic_the_board_has_no_window_function_or_handle() {
    let config = board_without_nic_config();
    let layout = FirmwareLayout::for_config(&config, HANDOFF_MINOR_SECURE_BOOT).unwrap();
    assert!(
        layout
            .map
            .regions()
            .iter()
            .all(|region| region.range.start().get() != NIC_PCI_BAR_BASE)
    );
    let mut board = loaded(&config);
    assert!(board.nic().is_none());
    assert_eq!(read(&mut board, cfg(0)), 0xFFFF_FFFF);
    assert!(matches!(
        board.bus_mut().read_physical(bar(0), AccessWidth::Dword),
        Err(BusAccessError::UnmappedPhysical(_))
    ));
}

#[test]
fn the_nic_changes_only_the_region_table_of_the_handoff_and_not_the_acpi_manifest() {
    let without = board_without_nic_config();
    let with = nic_config();
    let handoff = |config: &FirmwareBoardConfig| firmware_handoff_block(config).unwrap();
    let (block_without, block_with) = (handoff(&without), handoff(&with));
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
    let (regions_without, regions_with) = (regions_of(&block_without), regions_of(&block_with));
    assert_eq!(regions_with.len(), regions_without.len() + 1);
    let window: Vec<_> = regions_with
        .iter()
        .filter(|region| le_u64(region, 8).unwrap() == NIC_PCI_BAR_BASE)
        .collect();
    assert_eq!(window.len(), 1);
    assert_eq!(le_u32(window[0], 0).unwrap(), REGION_MMIO);
    assert_eq!(le_u64(window[0], 16).unwrap(), NIC_PCI_BAR_BYTES);
    assert_eq!(le_u32(window[0], 24).unwrap(), NO_OWNER);
}

#[test]
fn the_nic_and_the_xhci_can_share_a_board_with_disjoint_windows() {
    let config = FirmwareBoardConfig {
        xhci: Some(crate::XhciConfig::default()),
        ..nic_config()
    };
    let layout = FirmwareLayout::for_config(&config, HANDOFF_MINOR_SECURE_BOOT).unwrap();
    let without =
        FirmwareLayout::for_config(&board_without_nic_config(), HANDOFF_MINOR_SECURE_BOOT).unwrap();
    let window = layout
        .map
        .regions()
        .iter()
        .find(|region| region.range.start().get() == NIC_PCI_BAR_BASE)
        .expect("the BAR window is in the map");
    assert_eq!(window.kind, MemoryRegionKind::Mmio);
    assert_eq!(window.range.len(), NIC_PCI_BAR_BYTES);
    let end = NIC_PCI_BAR_BASE + NIC_PCI_BAR_BYTES;
    for region in layout.map.regions() {
        if region.range.start().get() == NIC_PCI_BAR_BASE {
            continue;
        }
        let (start, len) = (region.range.start().get(), region.range.len());
        assert!(
            start + len <= NIC_PCI_BAR_BASE || start >= end,
            "{start:#x}+{len:#x} overlaps the NIC window"
        );
    }
    // The window lies in the 2 MiB block of the display registers and ECAM: no new page table.
    assert_eq!(layout.table_pages, without.table_pages);
    let mut board = loaded(&config);
    assert!(board.nic().is_some() && board.xhci().is_some());
    assert_eq!(read(&mut board, cfg(0)), 0x100E_8086);
}

// ---------------------------------------------------------------- PCI function and registers

#[test]
fn the_function_is_read_through_ecam_with_the_contract_identity_class_and_bar() {
    let mut board = loaded(&nic_config());
    assert_eq!(read(&mut board, cfg(0)), 0x100E_8086, "8086:100E");
    assert_eq!(read(&mut board, cfg(8)) >> 8, 0x02_00_00, "class: Ethernet");
    assert_eq!(
        read(&mut board, cfg(0x10)),
        NIC_PCI_BAR_BASE,
        "32-bit memory BAR0"
    );
    assert_eq!(read(&mut board, cfg(0x3C)) & 0xFF00, 0x0100, "INTA");
    // The slots around it stay empty.
    for slot in [3u64, 7] {
        assert_eq!(
            read(
                &mut board,
                GuestPhysicalAddress::new(PCI_ECAM_DESCRIPTOR.base + (slot << 15))
            ),
            0xFFFF_FFFF
        );
    }
}

#[test]
fn the_registers_decode_after_the_driver_sets_mse() {
    let mut board = loaded(&nic_config());
    assert_eq!(read(&mut board, bar(0x8)), 0xFFFF_FFFF, "decode is off");
    write(&mut board, cfg(4), 0x6);
    let status = read(&mut board, bar(0x8));
    assert_eq!(status & 3, 3, "full duplex, link up");
    // The first receive address pair holds the MAC from the configuration.
    assert_eq!(read(&mut board, bar(0x5400)), 0x0044_4F02);
    assert_eq!(read(&mut board, bar(0x5404)), 0x8000_1000);
}

#[test]
fn a_guest_register_write_wakes_the_loop_until_the_next_service() {
    let mut board = loaded(&nic_config());
    let nic = board.nic().unwrap();
    assert!(!nic.wakeup_pending());
    write(&mut board, cfg(4), 0x6);
    write(&mut board, bar(0x3818), 0);
    assert!(nic.wakeup_pending(), "a tail write ends the quantum");
    service(&mut board);
    assert!(!nic.wakeup_pending());
}

// ---------------------------------------------------------------- run loop and reset

#[test]
fn service_moves_frames_both_ways_between_the_rings_and_the_backend() {
    let mut board = loaded(&nic_config());
    let wire = WireBackend::default();
    board
        .nic()
        .unwrap()
        .attach_backend(Box::new(wire.clone()))
        .unwrap();
    program_rings(&mut board);
    let outbound = frame_to(crate::NIC_GATEWAY_MAC);
    queue_frame(&mut board, &outbound);
    write(&mut board, bar(0x3818), 1);
    let inbound = frame_to(GUEST_MAC);
    wire.0.lock().unwrap().to_guest.push(inbound.clone());
    service(&mut board);
    assert_eq!(wire.0.lock().unwrap().sent, vec![outbound]);
    assert_eq!(get(&mut board, TX_RING + 12, 1), [1], "TX descriptor done");
    assert_eq!(get(&mut board, RX_BUFFER, 60), inbound);
    assert_eq!(get(&mut board, RX_RING + 12, 1), [0b111], "DD, EOP, IXSM");
    assert_eq!(read(&mut board, bar(0x2810)), 1, "RDH");
    // ICR: TXDW, TXQE and RXT0 are pending; the line is up only once unmasked.
    let nic = board.nic().unwrap();
    assert!(!nic.sample_pci_interrupts());
    write(&mut board, bar(0xD0), 1 << 7);
    assert!(nic.sample_pci_interrupts());
}

#[test]
fn the_firmware_run_loop_transmits_what_the_guest_rings_with_its_tail_write() {
    // The guest only advances TDT (mov edi, BAR+0x3818; mov dword [rdi], 1; hlt): the rings are
    // programmed by the host, as a firmware driver would do before.
    let tail = (NIC_PCI_BAR_BASE + 0x3818) as u32;
    let mut code = vec![0xBF];
    code.extend(tail.to_le_bytes());
    code.extend([0xC7, 0x07, 1, 0, 0, 0, 0xF4]);
    let mut board = load_firmware_image(&test_image(&code, &[]), &nic_config()).unwrap();
    let wire = WireBackend::default();
    board
        .nic()
        .unwrap()
        .attach_backend(Box::new(wire.clone()))
        .unwrap();
    program_rings(&mut board);
    let outbound = frame_to(crate::NIC_GATEWAY_MAC);
    queue_frame(&mut board, &outbound);
    let report = board.run_boot(1000).unwrap();
    assert!(
        matches!(report.exit, PagedProbeExit::Halted),
        "{:?}",
        report.exit
    );
    assert_eq!(wire.0.lock().unwrap().sent, vec![outbound]);
    assert_eq!(get(&mut board, TX_RING + 12, 1), [1]);
}

#[test]
fn board_reset_returns_the_nic_and_its_pci_function_to_the_power_on_state() {
    let mut board = loaded(&nic_config());
    let wire = WireBackend::default();
    board
        .nic()
        .unwrap()
        .attach_backend(Box::new(wire.clone()))
        .unwrap();
    program_rings(&mut board);
    write(&mut board, bar(0xD0), 1);
    board.platform_reset(PlatformResetKind::Warm).unwrap();
    assert_eq!(read(&mut board, cfg(4)) & 0xFFFF, 0);
    assert_eq!(read(&mut board, cfg(0x10)), NIC_PCI_BAR_BASE);
    assert_eq!(read(&mut board, bar(0)), 0xFFFF_FFFF);
    write(&mut board, cfg(4), 0x6);
    for (register, name) in [
        (0xD0, "IMS"),
        (0x100, "RCTL"),
        (0x400, "TCTL"),
        (0x3808, "TDLEN"),
    ] {
        assert_eq!(read(&mut board, bar(register)), 0, "{name}");
    }
    assert_eq!(
        wire.0.lock().unwrap().resets,
        1,
        "the backend forgets its flows"
    );
    assert!(!board.nic().unwrap().wakeup_pending());
}

#[test]
fn firmware_code_enumerates_the_function_through_the_ecam_window() {
    // mov edi, ECAM + (6 << 15); mov eax, [rdi] (vendor and device); mov [0x0300_0000], eax;
    // mov eax, [rdi + 8] (revision and class); mov [0x0300_0004], eax; hlt.
    let function = (PCI_ECAM_DESCRIPTOR.base + (u64::from(NIC_PCI_SLOT) << 15)) as u32;
    let mut code = vec![0xBF];
    code.extend(function.to_le_bytes());
    code.extend([0x8B, 0x07, 0x89, 0x04, 0x25, 0, 0, 0, 0x03]);
    code.extend([0x8B, 0x47, 0x08, 0x89, 0x04, 0x25, 4, 0, 0, 0x03, 0xF4]);
    let mut board = load_firmware_image(&test_image(&code, &[]), &nic_config()).unwrap();
    let report = board.run_boot(1000).unwrap();
    assert!(
        matches!(report.exit, PagedProbeExit::Halted),
        "{:?}",
        report.exit
    );
    assert_eq!(
        get(&mut board, 0x0300_0000, 4),
        0x100E_8086u32.to_le_bytes()
    );
    assert_eq!(
        get(&mut board, 0x0300_0004, 4),
        0x0200_0002u32.to_le_bytes(),
        "class 020000, revision 2"
    );
}

#[test]
fn firmware_code_reaches_the_bar_registers_through_the_exact_window() {
    // mov edi, BAR; mov eax, [rdi + 8] (STATUS); mov [0x0300_0000], eax; hlt. Command.MSE is set
    // by the host, as the firmware driver would do through PCI first.
    let mut code = vec![0xBF];
    code.extend((NIC_PCI_BAR_BASE as u32).to_le_bytes());
    code.extend([0x8B, 0x47, 0x08, 0x89, 0x04, 0x25, 0, 0, 0, 0x03, 0xF4]);
    let mut board = load_firmware_image(&test_image(&code, &[]), &nic_config()).unwrap();
    write(&mut board, cfg(4), 0x6);
    let report = board.run_boot(1000).unwrap();
    assert!(
        matches!(report.exit, PagedProbeExit::Halted),
        "{:?}",
        report.exit
    );
    let status = u32::from_le_bytes(get(&mut board, 0x0300_0000, 4).try_into().unwrap());
    assert_eq!(status & 3, 3);
}
