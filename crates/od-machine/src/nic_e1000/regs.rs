//! Register offsets and bit positions of the 82540EM-compatible NIC (NIC_DEVICE.md section 4).
//! Names follow the public Intel software developer's manual of the 8254x family.

// General.
pub(super) const CTRL: u64 = 0x0000;
pub(super) const STATUS: u64 = 0x0008;
pub(super) const EECD: u64 = 0x0010;
pub(super) const EERD: u64 = 0x0014;
pub(super) const CTRL_EXT: u64 = 0x0018;
pub(super) const MDIC: u64 = 0x0020;

// Interrupts.
pub(super) const ICR: u64 = 0x00C0;
pub(super) const ICS: u64 = 0x00C8;
pub(super) const IMS: u64 = 0x00D0;
pub(super) const IMC: u64 = 0x00D8;

// Receive.
pub(super) const RCTL: u64 = 0x0100;
pub(super) const RDBAL: u64 = 0x2800;
pub(super) const RDBAH: u64 = 0x2804;
pub(super) const RDLEN: u64 = 0x2808;
pub(super) const RDH: u64 = 0x2810;
pub(super) const RDT: u64 = 0x2818;

// Transmit.
pub(super) const TCTL: u64 = 0x0400;
pub(super) const TDBAL: u64 = 0x3800;
pub(super) const TDBAH: u64 = 0x3804;
pub(super) const TDLEN: u64 = 0x3808;
pub(super) const TDH: u64 = 0x3810;
pub(super) const TDT: u64 = 0x3818;

// Filters: the multicast table array (128 dwords) and the receive address pairs (16 pairs).
pub(super) const MTA_BASE: u64 = 0x5200;
pub(super) const MTA_ENTRIES: usize = 128;
pub(super) const RA_BASE: u64 = 0x5400;
pub(super) const RA_ENTRIES: usize = 16;

/// Registers the model stores and returns without acting on them (NIC_DEVICE.md section 4.4):
/// the drivers program them but no modelled behaviour depends on the value. Value after reset 0
/// unless the second field says otherwise.
pub(super) const LATCHED: [(u64, u32); 19] = [
    (0x0028, 0),           // FCAL
    (0x002C, 0),           // FCAH
    (0x0030, 0),           // FCT
    (0x0038, 0),           // VET
    (0x00C4, 0),           // ITR
    (0x0170, 0),           // FCTTV
    (0x0178, 0),           // TXCW
    (0x0410, 0x0060_200A), // TIPG (reset value of the manual)
    (0x1000, 0x0010_0030), // PBA
    (0x2160, 0),           // FCRTL
    (0x2168, 0),           // FCRTH
    (0x2820, 0),           // RDTR
    (0x2828, 0),           // RXDCTL
    (0x282C, 0),           // RADV
    (0x3820, 0),           // TIDV
    (0x3828, 0),           // TXDCTL
    (0x382C, 0),           // TADV
    (0x5000, 0),           // RXCSUM
    (0x0E00, 0),           // LEDCTL
];

// CTRL bits.
pub(super) const CTRL_FD: u32 = 1 << 0;
pub(super) const CTRL_SLU: u32 = 1 << 6;
pub(super) const CTRL_RST: u32 = 1 << 26;
pub(super) const CTRL_PHY_RST: u32 = 1 << 31;
/// FD and SPEED = 1000 Mb/s: a decision of the model (the manual's reset value was not
/// verified); the driver rewrites CTRL during initialisation.
pub(super) const CTRL_RESET_VALUE: u32 = CTRL_FD | (0b10 << 8);

// STATUS bits: full duplex, link up, speed 1000 Mb/s (SPEED and ASDV).
pub(super) const STATUS_VALUE: u32 = (1 << 0) | (1 << 1) | (0b10 << 6) | (0b10 << 8);

// EECD bits (Microwire bit-bang interface).
pub(super) const EECD_SK: u32 = 1 << 0;
pub(super) const EECD_CS: u32 = 1 << 1;
pub(super) const EECD_DI: u32 = 1 << 2;
pub(super) const EECD_DO: u32 = 1 << 3;
pub(super) const EECD_FWE_MASK: u32 = 0b11 << 4;
pub(super) const EECD_REQ: u32 = 1 << 6;
pub(super) const EECD_GNT: u32 = 1 << 7;
pub(super) const EECD_PRES: u32 = 1 << 8;
/// Bits the guest can store (DO, GNT and PRES are status).
pub(super) const EECD_WRITABLE: u32 = EECD_SK | EECD_CS | EECD_DI | EECD_FWE_MASK | EECD_REQ;

// EERD bits.
pub(super) const EERD_START: u32 = 1 << 0;
pub(super) const EERD_DONE: u32 = 1 << 4;

// MDIC bits.
pub(super) const MDIC_DATA_MASK: u32 = 0xFFFF;
pub(super) const MDIC_REG_SHIFT: u32 = 16;
pub(super) const MDIC_PHY_SHIFT: u32 = 21;
pub(super) const MDIC_OP_SHIFT: u32 = 26;
pub(super) const MDIC_OP_WRITE: u32 = 0b01;
pub(super) const MDIC_OP_READ: u32 = 0b10;
pub(super) const MDIC_READY: u32 = 1 << 28;
pub(super) const MDIC_INT_EN: u32 = 1 << 29;
pub(super) const MDIC_ERROR: u32 = 1 << 30;
/// The internal PHY answers at address 1 only.
pub(super) const PHY_ADDRESS: u32 = 1;

// Interrupt causes (ICR/ICS/IMS/IMC).
pub(super) const INT_TXDW: u32 = 1 << 0;
pub(super) const INT_TXQE: u32 = 1 << 1;
pub(super) const INT_LSC: u32 = 1 << 2;
pub(super) const INT_RXDMT0: u32 = 1 << 4;
pub(super) const INT_RXT0: u32 = 1 << 7;
pub(super) const INT_MDAC: u32 = 1 << 9;
/// Cause bits the model implements as register bits (the rest is reserved or unused).
pub(super) const INT_MASK: u32 = 0x0001_E6DF;

// RCTL bits.
pub(super) const RCTL_EN: u32 = 1 << 1;
pub(super) const RCTL_UPE: u32 = 1 << 3;
pub(super) const RCTL_MPE: u32 = 1 << 4;
pub(super) const RCTL_RDMTS_SHIFT: u32 = 8;
pub(super) const RCTL_MO_SHIFT: u32 = 12;
pub(super) const RCTL_BAM: u32 = 1 << 15;
pub(super) const RCTL_BSIZE_SHIFT: u32 = 16;
pub(super) const RCTL_BSEX: u32 = 1 << 25;
pub(super) const RCTL_SECRC: u32 = 1 << 26;

// TCTL bits.
pub(super) const TCTL_EN: u32 = 1 << 1;

// Legacy descriptor command byte (transmit).
pub(super) const CMD_EOP: u8 = 1 << 0;
pub(super) const CMD_RS: u8 = 1 << 3;
pub(super) const CMD_DEXT: u8 = 1 << 5;

// Descriptor status bits.
pub(super) const STA_DD: u8 = 1 << 0;
pub(super) const STA_EOP: u8 = 1 << 1;
/// Receive: "ignore the checksum indication": the model computes none.
pub(super) const STA_IXSM: u8 = 1 << 2;
