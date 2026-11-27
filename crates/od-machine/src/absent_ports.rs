//! Explicitly absent legacy ports: the `outb_p` delay sink `0xED`, and CMOS/RTC
//! `0x70/0x71` only when the board has no explicit RTC base time (S4-29b/S7-14).
//!
//! A port without a binding stops the machine (`BusAccessError::UnmappedPort`),
//! not a read of `0xFF`. The three ports below are touched by a stock Linux
//! guest (`CMOS_READ`/`CMOS_WRITE`, `outb_p`/`io_delay`). The `0xED` sink is
//! definitely absent: reads return `0xFF` and writes are ignored. The CMOS pair
//! uses this absent model only for a board without `rtc_base_unix_seconds`; with
//! an explicit handoff base, `RtcCmos` owns the same two ports and the FADT bit
//! is cleared. The absent board's FADT bit is pinned by a board-level test.
//!
//! Width policy: byte accesses only. Linux uses only `inb`/`outb` on these
//! ports, and the other port devices of the model (8259, UART, conf1) refuse an
//! unsupported width with [`DeviceError::UnsupportedWidth`] instead of
//! fabricating a value. A word or dword access therefore stops the machine
//! rather than returning `0xFFFF`/`0xFFFF_FFFF`: it means the guest is doing
//! something this model has not been audited for. Accesses that straddle the end
//! of a binding never reach the device (`BusAccessError::CrossesRegion`).
//!
//! Not part of this module: the implemented RTC clock, CMOS RAM and NMI-disable
//! bit 7 of port `0x70` (`rtc_cmos.rs`), and the 8042 (`0x60/0x64`). The 8042 stays **unbound** on
//! purpose: that decision is still open (see the S4-29 decisions in the stage
//! plan), so a guest touching it is a trap. The POST port `0x80` has the same
//! absent-device model ([`AbsentPortRegion::Post`], S4-37a1) but the board binds
//! it only under `FirmwareBoardConfig::linux_profile`; without the flag it stays
//! a trap.

use crate::{AccessWidth, BusDevice, DeviceAccess, DeviceError, IoSpace};

pub use crate::rtc_cmos::{CMOS_DATA_PORT, CMOS_INDEX_PORT, CMOS_PORT_BYTES};
/// The `outb_p`/`io_delay` sink port.
pub const IO_DELAY_PORT: u16 = 0xED;
/// Ports of the `0xED` binding.
pub const IO_DELAY_PORT_BYTES: u32 = 1;
/// The POST/delay port (`0x80`), bound only under the Linux profile (S4-37a1).
pub const POST_PORT: u16 = 0x80;
/// Ports of the `0x80` binding.
pub const POST_PORT_BYTES: u32 = 1;
/// Base ports of the three legacy UART positions the board does not declare (COM2 `0x2F8`, COM3
/// `0x3E8`, COM4 `0x2E8`), bound only under the Linux profile (S7-31j): the kernel's 8250 driver
/// probes all four legacy positions, and an absent UART reads `0xFF` there.
pub const UART2_PORT: u16 = 0x2F8;
pub const UART3_PORT: u16 = 0x3E8;
pub const UART4_PORT: u16 = 0x2E8;
/// Ports of each of those bindings (the eight 8250 registers).
pub const ABSENT_UART_PORT_BYTES: u32 = 8;
/// What a read of an absent port returns.
pub const ABSENT_PORT_READ_VALUE: u8 = 0xFF;
/// The ports of the VMware backdoor (S7-32 slice T, ISSUE-714): `0x5658` (the command port, read
/// with `in eax, dx` and EAX = `VMXh`) and `0x5659` (the high-bandwidth port), bound as eight ports so
/// that a word or dword access at either does not cross the end of the binding.
pub const VMWARE_BACKDOOR_PORT: u16 = 0x5658;
/// Ports of the VMware backdoor binding.
pub const VMWARE_BACKDOOR_PORT_BYTES: u32 = 8;

/// Which group of absent ports a device stands for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AbsentPortRegion {
    /// `0x70/0x71`.
    Cmos,
    /// `0xED`.
    IoDelay,
    /// `0x80` (S4-37a1, Linux profile only).
    Post,
    /// `0x2F8..0x300`: COM2, not declared by the board (S7-31j, Linux profile only).
    Uart2,
    /// `0x3E8..0x3F0`: COM3, not declared by the board (S7-31j, Linux profile only).
    Uart3,
    /// `0x2E8..0x2F0`: COM4, not declared by the board (S7-31j, Linux profile only).
    Uart4,
    /// `0x5658..0x5660`: the VMware backdoor, not present on the board (S7-32 slice T, Linux profile
    /// only). `pvscsii.sys` of the Windows setup media probes it with `in eax, dx`; on a PC without
    /// VMware nothing decodes the port and the read gives all ones. Unlike the other regions it
    /// takes word and dword accesses (the probe is a dword read).
    VmwareBackdoor,
}

impl AbsentPortRegion {
    /// First port of the binding.
    pub const fn base_port(self) -> u16 {
        match self {
            Self::Cmos => CMOS_INDEX_PORT,
            Self::IoDelay => IO_DELAY_PORT,
            Self::Post => POST_PORT,
            Self::Uart2 => UART2_PORT,
            Self::Uart3 => UART3_PORT,
            Self::Uart4 => UART4_PORT,
            Self::VmwareBackdoor => VMWARE_BACKDOOR_PORT,
        }
    }

    /// Length of the `PortRegionBinding`.
    pub const fn port_bytes(self) -> u32 {
        match self {
            Self::Cmos => CMOS_PORT_BYTES,
            Self::IoDelay => IO_DELAY_PORT_BYTES,
            Self::Post => POST_PORT_BYTES,
            Self::Uart2 | Self::Uart3 | Self::Uart4 => ABSENT_UART_PORT_BYTES,
            Self::VmwareBackdoor => VMWARE_BACKDOOR_PORT_BYTES,
        }
    }
}

/// One stateless absent-port device. Bind it to
/// `[region.base_port(), region.base_port() + region.port_bytes())`; the device
/// sees offsets relative to its binding.
#[derive(Debug)]
pub struct AbsentPorts {
    region: AbsentPortRegion,
}

impl AbsentPorts {
    pub const fn new(region: AbsentPortRegion) -> Self {
        Self { region }
    }

    pub const fn region(&self) -> AbsentPortRegion {
        self.region
    }

    /// Space, width and offset checks, in the order of the other PIO devices.
    fn check(&self, access: DeviceAccess) -> Result<(), DeviceError> {
        if access.space != IoSpace::Pio {
            return Err(DeviceError::UnsupportedSpace);
        }
        // Byte only, except the VMware backdoor, which also takes words and dwords (S7-32 slice T).
        let width_ok = match access.width {
            AccessWidth::Byte => true,
            AccessWidth::Word | AccessWidth::Dword => {
                self.region == AbsentPortRegion::VmwareBackdoor
            }
            _ => false,
        };
        if !width_ok {
            return Err(DeviceError::UnsupportedWidth);
        }
        if access.offset >= u64::from(self.region.port_bytes()) {
            return Err(DeviceError::UnknownOffset);
        }
        Ok(())
    }
}

impl BusDevice for AbsentPorts {
    fn read(&mut self, access: DeviceAccess) -> Result<u64, DeviceError> {
        self.check(access)?;
        // All ones in every byte of the access (an absent port on the PC/AT bus).
        Ok(match access.width {
            AccessWidth::Word => 0xFFFF,
            AccessWidth::Dword => 0xFFFF_FFFF,
            _ => u64::from(ABSENT_PORT_READ_VALUE),
        })
    }

    fn write(&mut self, access: DeviceAccess, _value: u64) -> Result<(), DeviceError> {
        self.check(access)?;
        // Ignored: there is nothing to store and no side effect.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ACPI_PM_BASE_PORT, ACPI_PM_PORT_BYTES, BusAccessError, FIRMWARE_COM1_PORT,
        GuestPhysicalAddress, GuestPhysicalRange, MachineBus, MachineMemoryMap, MemoryRegionKind,
        MemoryRegionSpec, PIC_MASTER_BASE_PORT, PIC_PORT_BYTES, PIC_SLAVE_BASE_PORT, PortAddress,
        PortRange, PortRegionBinding, RamLimits,
    };

    const BYTE: AccessWidth = AccessWidth::Byte;
    /// Every region, including `Post`, which the board binds only under the
    /// Linux profile; these tests use the devices directly or on a private bus.
    const ALL_REGIONS: [AbsentPortRegion; 7] = [
        AbsentPortRegion::Cmos,
        AbsentPortRegion::IoDelay,
        AbsentPortRegion::Post,
        AbsentPortRegion::Uart2,
        AbsentPortRegion::Uart3,
        AbsentPortRegion::Uart4,
        AbsentPortRegion::VmwareBackdoor,
    ];

    fn acc(offset: u64, width: AccessWidth) -> DeviceAccess {
        DeviceAccess {
            space: IoSpace::Pio,
            offset,
            width,
        }
    }

    fn p(port: u16) -> PortAddress {
        PortAddress::new(port)
    }

    /// Both devices behind real `PortRegionBinding`s on a `MachineBus`.
    fn bus() -> MachineBus {
        let map = MachineMemoryMap::new(
            36,
            vec![MemoryRegionSpec::new(
                GuestPhysicalRange::new(GuestPhysicalAddress::new(0), 1 << 20).unwrap(),
                MemoryRegionKind::Ram,
            )],
        )
        .unwrap();
        let ports = [AbsentPortRegion::Cmos, AbsentPortRegion::IoDelay]
            .iter()
            .enumerate()
            .map(|(device_index, region)| PortRegionBinding {
                range: PortRange::new(PortAddress::new(region.base_port()), region.port_bytes())
                    .unwrap(),
                device_index,
            })
            .collect();
        MachineBus::new(
            map,
            RamLimits::new(1 << 20),
            vec![
                Box::new(AbsentPorts::new(AbsentPortRegion::Cmos)),
                Box::new(AbsentPorts::new(AbsentPortRegion::IoDelay)),
            ],
            Vec::new(),
            ports,
        )
        .unwrap()
    }

    #[test]
    fn constants_match_the_legacy_ports() {
        assert_eq!((CMOS_INDEX_PORT, CMOS_DATA_PORT), (0x70, 0x71));
        assert_eq!(IO_DELAY_PORT, 0xED);
        assert_eq!(AbsentPortRegion::Cmos.base_port(), 0x70);
        assert_eq!(AbsentPortRegion::Cmos.port_bytes(), 2);
        assert_eq!(AbsentPortRegion::IoDelay.base_port(), 0xED);
        assert_eq!(AbsentPortRegion::IoDelay.port_bytes(), 1);
        assert_eq!(ABSENT_PORT_READ_VALUE, 0xFF);
        assert_eq!(POST_PORT, 0x80);
        assert_eq!(AbsentPortRegion::Post.base_port(), 0x80);
        assert_eq!(AbsentPortRegion::Post.port_bytes(), POST_PORT_BYTES);
        assert_eq!(POST_PORT_BYTES, 1);
    }

    #[test]
    fn reads_of_the_absent_ports_give_0xff() {
        for region in ALL_REGIONS {
            let mut dev = AbsentPorts::new(region);
            for offset in 0..u64::from(region.port_bytes()) {
                assert_eq!(dev.read(acc(offset, BYTE)), Ok(0xFF), "{region:?} {offset}");
            }
        }
    }

    #[test]
    fn writes_are_ignored_and_nothing_can_be_read_back() {
        // The device is stateless; the guest-visible "snapshot" is the read-back
        // of every port. Every byte value is written to every port and the
        // read-back is compared with the one taken before.
        for region in ALL_REGIONS {
            let mut dev = AbsentPorts::new(region);
            let ports = u64::from(region.port_bytes());
            let snapshot = |dev: &mut AbsentPorts| -> Vec<u64> {
                (0..ports)
                    .map(|offset| dev.read(acc(offset, BYTE)).unwrap())
                    .collect()
            };
            let before = snapshot(&mut dev);
            assert_eq!(before, vec![0xFF; ports as usize]);
            for offset in 0..ports {
                for value in 0..=0xFFu64 {
                    assert_eq!(dev.write(acc(offset, BYTE), value), Ok(()));
                    assert_eq!(snapshot(&mut dev), before, "{region:?} {offset} {value:#x}");
                }
            }
        }
    }

    #[test]
    fn cmos_index_then_data_is_still_absent() {
        // The Linux `CMOS_READ`: `outb(addr, 0x70); inb(0x71)`, with and without
        // the NMI-disable bit, and `CMOS_WRITE`.
        let mut bus = bus();
        for index in [0x00u32, 0x0A, 0x0B, 0x32, 0x7F, 0x80, 0x8F, 0xFF] {
            bus.write_port(p(0x70), BYTE, index).unwrap();
            assert_eq!(bus.read_port(p(0x71), BYTE), Ok(0xFF), "index {index:#x}");
            assert_eq!(bus.read_port(p(0x70), BYTE), Ok(0xFF), "index {index:#x}");
            bus.write_port(p(0x71), BYTE, 0x55).unwrap();
            assert_eq!(bus.read_port(p(0x71), BYTE), Ok(0xFF));
        }
    }

    #[test]
    fn io_delay_sink_through_the_bus() {
        let mut bus = bus();
        assert_eq!(bus.read_port(p(0xED), BYTE), Ok(0xFF));
        // `outb_p`/`io_delay` writes a junk byte (Linux uses 0).
        bus.write_port(p(0xED), BYTE, 0).unwrap();
        bus.write_port(p(0xED), BYTE, 0xAB).unwrap();
        assert_eq!(bus.read_port(p(0xED), BYTE), Ok(0xFF));
    }

    #[test]
    fn word_dword_and_qword_are_refused_by_the_device() {
        // Policy: byte only (see the module comment). Refused before any effect. The VMware
        // backdoor takes words and dwords (its own test); a qword is refused there too.
        for region in ALL_REGIONS {
            let mut dev = AbsentPorts::new(region);
            let widths: &[AccessWidth] = if region == AbsentPortRegion::VmwareBackdoor {
                &[AccessWidth::Qword]
            } else {
                &[AccessWidth::Word, AccessWidth::Dword, AccessWidth::Qword]
            };
            for &width in widths {
                assert_eq!(dev.read(acc(0, width)), Err(DeviceError::UnsupportedWidth));
                assert_eq!(
                    dev.write(acc(0, width), 0x1234),
                    Err(DeviceError::UnsupportedWidth)
                );
            }
            assert_eq!(dev.read(acc(0, BYTE)), Ok(0xFF));
        }
    }

    /// S7-32 slice T (ISSUE-714): the VMware backdoor reads all ones at every width 1, 2, 4 and
    /// every offset of its eight ports and ignores writes; through the bus a word or dword that
    /// would leave the binding (`in ax/eax` at `0x565f`/`0x565d`) crosses its end, and the ports
    /// around it stay unbound.
    #[test]
    fn the_vmware_backdoor_reads_all_ones_at_every_width_and_ignores_writes() {
        let mut dev = AbsentPorts::new(AbsentPortRegion::VmwareBackdoor);
        for offset in 0..u64::from(VMWARE_BACKDOOR_PORT_BYTES) {
            for (width, ones) in [
                (BYTE, 0xFF),
                (AccessWidth::Word, 0xFFFF),
                (AccessWidth::Dword, 0xFFFF_FFFF),
            ] {
                assert_eq!(dev.read(acc(offset, width)), Ok(ones), "{offset} {width:?}");
                assert_eq!(dev.write(acc(offset, width), 0x5658_4d56), Ok(()));
                assert_eq!(dev.read(acc(offset, width)), Ok(ones), "{offset} {width:?}");
            }
        }
        let region = AbsentPortRegion::VmwareBackdoor;
        let map = MachineMemoryMap::new(
            36,
            vec![MemoryRegionSpec::new(
                GuestPhysicalRange::new(GuestPhysicalAddress::new(0), 1 << 20).unwrap(),
                MemoryRegionKind::Ram,
            )],
        )
        .unwrap();
        let mut bus = MachineBus::new(
            map,
            RamLimits::new(1 << 20),
            vec![Box::new(AbsentPorts::new(region))],
            Vec::new(),
            vec![PortRegionBinding {
                range: PortRange::new(p(region.base_port()), region.port_bytes()).unwrap(),
                device_index: 0,
            }],
        )
        .unwrap();
        assert_eq!(
            bus.read_port(p(0x5658), AccessWidth::Dword),
            Ok(0xFFFF_FFFF)
        );
        assert_eq!(
            bus.read_port(p(0x5659), AccessWidth::Dword),
            Ok(0xFFFF_FFFF)
        );
        assert_eq!(
            bus.read_port(p(0x565c), AccessWidth::Dword),
            Ok(0xFFFF_FFFF)
        );
        assert_eq!(bus.read_port(p(0x565e), AccessWidth::Word), Ok(0xFFFF));
        assert_eq!(
            bus.read_port(p(0x565f), AccessWidth::Word),
            Err(BusAccessError::CrossesRegion)
        );
        assert_eq!(
            bus.read_port(p(0x565d), AccessWidth::Dword),
            Err(BusAccessError::CrossesRegion)
        );
        assert_eq!(
            bus.read_port(p(0x5657), BYTE),
            Err(BusAccessError::UnmappedPort(p(0x5657)))
        );
        assert_eq!(
            bus.read_port(p(0x5660), BYTE),
            Err(BusAccessError::UnmappedPort(p(0x5660)))
        );
    }

    #[test]
    fn invalid_space_and_offset_are_refused() {
        let mmio = DeviceAccess {
            space: IoSpace::Mmio,
            offset: 0,
            width: BYTE,
        };
        for region in ALL_REGIONS {
            let mut dev = AbsentPorts::new(region);
            assert_eq!(dev.read(mmio), Err(DeviceError::UnsupportedSpace));
            assert_eq!(dev.write(mmio, 0), Err(DeviceError::UnsupportedSpace));
            let past = u64::from(region.port_bytes());
            assert_eq!(dev.read(acc(past, BYTE)), Err(DeviceError::UnknownOffset));
            assert_eq!(
                dev.write(acc(past, BYTE), 0),
                Err(DeviceError::UnknownOffset)
            );
        }
    }

    #[test]
    fn wide_accesses_through_the_bus() {
        let mut bus = bus();
        // A word at 0x70 is the only wide access that fits a binding (0x70..0x72):
        // the device refuses the width.
        assert!(matches!(
            bus.read_port(p(0x70), AccessWidth::Word),
            Err(BusAccessError::Device {
                source: DeviceError::UnsupportedWidth,
                ..
            })
        ));
        assert!(matches!(
            bus.write_port(p(0x70), AccessWidth::Word, 0xFFFF),
            Err(BusAccessError::Device {
                source: DeviceError::UnsupportedWidth,
                ..
            })
        ));
        // Everything else straddles the end of a binding: the bus refuses it
        // before the device (a dword at 0x70, a word at 0x71, any wide access at 0xED).
        for (port, width) in [
            (0x70u16, AccessWidth::Dword),
            (0x71, AccessWidth::Word),
            (0x71, AccessWidth::Dword),
            (0xED, AccessWidth::Word),
            (0xED, AccessWidth::Dword),
        ] {
            assert_eq!(
                bus.read_port(p(port), width),
                Err(BusAccessError::CrossesRegion),
                "read {width:?} @{port:#x}"
            );
            assert_eq!(
                bus.write_port(p(port), width, 0),
                Err(BusAccessError::CrossesRegion),
                "write {width:?} @{port:#x}"
            );
        }
        assert_eq!(
            bus.read_port(p(0x70), AccessWidth::Qword),
            Err(BusAccessError::UnsupportedWidth)
        );
        // Nothing changed.
        assert_eq!(bus.read_port(p(0x71), BYTE), Ok(0xFF));
    }

    /// The decision about the 8042 (`0x60/0x64`) is still open (S4-29
    /// decisions): it is deliberately **not** bound, so a guest touching it stops
    /// the machine with `UnmappedPort` instead of getting a made-up answer. The
    /// POST port `0x80` is a sink only under the board's `linux_profile`
    /// (S4-37a1); this test checks the private bus of this file, which has no
    /// profile, so `0x80` is unbound here. This test is a trap: when the 8042
    /// decision is made and a device is added, it must be changed on purpose.
    /// Ports next to the absent ones are unbound too (no accidental widening of
    /// the bindings).
    #[test]
    fn open_decision_ports_stay_unmapped() {
        let mut bus = bus();
        for port in [0x60u16, 0x64, 0x80, 0x6F, 0x72, 0x73, 0xEC, 0xEE] {
            assert_eq!(
                bus.read_port(p(port), BYTE),
                Err(BusAccessError::UnmappedPort(p(port))),
                "read {port:#x}"
            );
            assert_eq!(
                bus.write_port(p(port), BYTE, 0),
                Err(BusAccessError::UnmappedPort(p(port))),
                "write {port:#x}"
            );
        }
    }

    #[test]
    fn ports_do_not_overlap_other_devices() {
        // PIC pair, COM1, PM block and conf1 as `(start, len)`; the PIT ports
        // `0x40..=0x43` and `0x61` are listed for S4-29c, `0x60/0x64/0x80`
        // are the open-decision ports (they must stay outside the bindings).
        use crate::pci_ecam::{PCI_CONF1_BASE_PORT, PCI_CONF1_PORT_BYTES};
        let others = [
            ("PIC master", PIC_MASTER_BASE_PORT, PIC_PORT_BYTES),
            ("PIC slave", PIC_SLAVE_BASE_PORT, PIC_PORT_BYTES),
            ("COM1", FIRMWARE_COM1_PORT, 8),
            ("PM block", ACPI_PM_BASE_PORT, ACPI_PM_PORT_BYTES),
            ("conf1", PCI_CONF1_BASE_PORT, PCI_CONF1_PORT_BYTES),
            ("PIT", 0x40, 4),
            ("port 0x61", 0x61, 1),
            ("8042 data", 0x60, 1),
            ("8042 status", 0x64, 1),
            ("POST", 0x80, 1),
        ];
        for region in ALL_REGIONS {
            let a =
                u32::from(region.base_port())..u32::from(region.base_port()) + region.port_bytes();
            for &(name, start, len) in &others {
                // The `Post` region is the "POST" entry itself.
                if region == AbsentPortRegion::Post && name == "POST" {
                    continue;
                }
                let b = u32::from(start)..u32::from(start) + len;
                assert!(a.end <= b.start || b.end <= a.start, "{region:?} vs {name}");
            }
        }
        // One bus that holds the two absent devices, the PIC pair and the PM block
        // accepts all bindings (`MachineBus::new` rejects overlapping ports).
        let pair = crate::Pic8259Pair::new();
        let clock = crate::VirtualClockHandle::new(1_000_000_000).unwrap();
        let (pm, _) = crate::AcpiPm::new_with_handle(clock);
        let range = |start: u16, len: u32| PortRange::new(PortAddress::new(start), len).unwrap();
        let devices: Vec<Box<dyn BusDevice>> = vec![
            Box::new(AbsentPorts::new(AbsentPortRegion::Cmos)),
            Box::new(AbsentPorts::new(AbsentPortRegion::IoDelay)),
            Box::new(pair.master),
            Box::new(pair.slave),
            Box::new(pm),
            Box::new(AbsentPorts::new(AbsentPortRegion::VmwareBackdoor)),
        ];
        let ports = vec![
            PortRegionBinding {
                range: range(VMWARE_BACKDOOR_PORT, VMWARE_BACKDOOR_PORT_BYTES),
                device_index: 5,
            },
            PortRegionBinding {
                range: range(CMOS_INDEX_PORT, CMOS_PORT_BYTES),
                device_index: 0,
            },
            PortRegionBinding {
                range: range(IO_DELAY_PORT, IO_DELAY_PORT_BYTES),
                device_index: 1,
            },
            PortRegionBinding {
                range: range(PIC_MASTER_BASE_PORT, PIC_PORT_BYTES),
                device_index: 2,
            },
            PortRegionBinding {
                range: range(PIC_SLAVE_BASE_PORT, PIC_PORT_BYTES),
                device_index: 3,
            },
            PortRegionBinding {
                range: range(ACPI_PM_BASE_PORT, ACPI_PM_PORT_BYTES),
                device_index: 4,
            },
        ];
        let map = MachineMemoryMap::new(
            36,
            vec![MemoryRegionSpec::new(
                GuestPhysicalRange::new(GuestPhysicalAddress::new(0), 1 << 20).unwrap(),
                MemoryRegionKind::Ram,
            )],
        )
        .unwrap();
        MachineBus::new(map, RamLimits::new(1 << 20), devices, Vec::new(), ports).unwrap();
    }

    /// A board without explicit RTC configuration keeps absent ports and the
    /// matching FADT `CMOS_RTC_NOT_PRESENT` bit.
    #[test]
    fn model_agrees_with_the_fadt_iapc_boot_arch() {
        use crate::{FirmwareBoardConfig, validate_acpi_blob};
        const BLOB: &[u8] =
            include_bytes!("../../../tests/fixtures/guest/stage4/acpi_blob/stage4_acpi_blob.bin");
        let board = FirmwareBoardConfig {
            pci_ecam: true,
            acpi_pm: true,
            acpi_tables: true,
            ..FirmwareBoardConfig::default()
        };
        let info = validate_acpi_blob(BLOB, &board.acpi_manifest_hash().unwrap()).unwrap();
        let table = info
            .tables
            .iter()
            .find(|t| &t.signature == b"FACP")
            .expect("FADT in the blob");
        let fadt = info.table_bytes(BLOB, table);
        let iapc = u16::from_le_bytes(fadt[109..111].try_into().unwrap());
        let not_present = iapc >> crate::CMOS_RTC_NOT_PRESENT_BIT & 1 == 1;
        assert!(not_present, "default board has no configured RTC");
        assert_eq!(iapc & 0b11, 0, "LEGACY_DEVICES and 8042 stay 0");

        // The model side: the ports of the CMOS pair really are bound and absent.
        let mut bus = bus();
        assert_eq!(bus.read_port(p(CMOS_INDEX_PORT), BYTE), Ok(0xFF));
        assert_eq!(bus.read_port(p(CMOS_DATA_PORT), BYTE), Ok(0xFF));
    }
}
