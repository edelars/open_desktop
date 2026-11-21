//! Register values and offsets of the xHCI model (`docs/specs/XHCI_DEVICE.md`
//! sections 4 to 7). Offsets are from BAR0.

/// Slots and ports of the controller (HCSPARAMS1).
pub(super) const MAX_SLOTS: usize = 8;
pub(super) const MAX_PORTS: usize = 2;

/// CAPLENGTH (byte at 0x00), HCIVERSION (word at 0x02): xHCI interface 1.0.
pub(super) const CAPLENGTH: u32 = 0x20;
pub(super) const HCIVERSION: u32 = 0x0100;
/// MaxSlots 8, MaxIntrs 1, MaxPorts 2.
pub(super) const HCSPARAMS1: u32 = 0x0200_0108;
/// AC64, CSZ = 0, xECP = 0x800 dwords (offset 0x2000).
pub(super) const HCCPARAMS1: u32 = 0x0800_0001;
pub(super) const DBOFF: u32 = 0x800;
pub(super) const RTSOFF: u32 = 0x1000;
/// Capability registers occupy `0x00..0x20` and are the only ones readable in bytes and words.
pub(super) const CAPABILITY_END: u64 = 0x20;

pub(super) const USBCMD: u64 = 0x20;
pub(super) const USBSTS: u64 = 0x24;
pub(super) const PAGESIZE: u64 = 0x28;
pub(super) const DNCTRL: u64 = 0x34;
pub(super) const CRCR: u64 = 0x38;
pub(super) const DCBAAP: u64 = 0x50;
pub(super) const CONFIG: u64 = 0x58;
/// PORTSC of port 1; ports are 0x10 apart.
pub(super) const PORT_BASE: u64 = 0x420;
pub(super) const PORT_STRIDE: u64 = 0x10;
pub(super) const DOORBELLS: u64 = 0x800;
pub(super) const MFINDEX: u64 = 0x1000;
pub(super) const IMAN: u64 = 0x1020;
pub(super) const IMOD: u64 = 0x1024;
pub(super) const ERSTSZ: u64 = 0x1028;
pub(super) const ERSTBA: u64 = 0x1030;
pub(super) const ERDP: u64 = 0x1038;
/// Supported Protocol Capability (the only extended capability).
pub(super) const XECP: u64 = 0x2000;

pub(super) const USBCMD_RS: u32 = 1;
pub(super) const USBCMD_HCRST: u32 = 1 << 1;
pub(super) const USBCMD_INTE: u32 = 1 << 2;
pub(super) const USBCMD_HSEE: u32 = 1 << 3;

pub(super) const USBSTS_HCH: u32 = 1;
pub(super) const USBSTS_HSE: u32 = 1 << 2;
pub(super) const USBSTS_EINT: u32 = 1 << 3;
pub(super) const USBSTS_PCD: u32 = 1 << 4;

pub(super) const CRCR_RCS: u32 = 1;
pub(super) const CRCR_CRR: u32 = 1 << 3;

/// IMOD reset value: IMODI = 4000 (250 microseconds).
pub(super) const IMOD_RESET: u32 = 0x0FA0;

pub(super) const IMAN_IP: u32 = 1;
pub(super) const IMAN_IE: u32 = 1 << 1;

pub(super) const ERDP_EHB: u32 = 1 << 3;

/// Registers of 64 bits (readable and writable as two dwords, low first, or as one qword).
pub(super) const fn is_qword_register(offset: u64) -> bool {
    matches!(offset, CRCR | DCBAAP | ERSTBA | ERDP)
}

/// MFINDEX: microframes (125 microseconds) of virtual time, 14 bits.
pub(super) const fn mfindex(now_ns: u64) -> u32 {
    ((now_ns / 125_000) & 0x3FFF) as u32
}

/// Supported Protocol Capability dwords at `XECP`: USB 2.0, "USB ", ports 1..2, slot type 0.
pub(super) const fn extended_capability(offset: u64) -> u32 {
    match offset - XECP {
        0x0 => 0x0200_0002,
        0x4 => 0x2042_5355,
        0x8 => 0x0000_0201,
        _ => 0,
    }
}
