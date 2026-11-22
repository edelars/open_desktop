//! The internal copper PHY behind MDIC (NIC_DEVICE.md section 6): 32 registers of a gigabit PHY
//! with auto-negotiation done, link up, 1000 Mb/s full duplex.

const REG_CONTROL: usize = 0;
const CONTROL_RESET: u16 = 1 << 15;
const CONTROL_DEFAULT: u16 = 0x1140;
const REG_STATUS: usize = 1;
/// Link up, auto-negotiation complete and able, extended status, all 10/100 capabilities.
const STATUS_VALUE: u16 = 0x796D;
const REG_ID1: usize = 2;
const REG_ID2: usize = 3;
/// Marvell M88E1011 identifier (OUI part and model); revision 0.
const ID1_VALUE: u16 = 0x0141;
const ID2_VALUE: u16 = 0x0C20;
const REG_ADVERTISE: usize = 4;
const REG_PARTNER: usize = 5;
const REG_EXPANSION: usize = 6;
const REG_1000T_CONTROL: usize = 9;
const REG_1000T_STATUS: usize = 10;
const REG_EXT_STATUS: usize = 15;
const REG_SPECIFIC_CONTROL: usize = 0x10;
/// Vendor-specific status: speed 1000 (15:14 = 10), full duplex, resolved, link up.
const REG_SPECIFIC_STATUS: usize = 0x11;
const SPECIFIC_STATUS_VALUE: u16 = 0xAC00;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Phy {
    regs: [u16; 32],
}

impl Phy {
    pub(super) fn new() -> Self {
        let mut regs = [0u16; 32];
        regs[REG_CONTROL] = CONTROL_DEFAULT;
        regs[REG_ADVERTISE] = 0x01E1;
        regs[REG_1000T_CONTROL] = 0x0300;
        regs[REG_SPECIFIC_CONTROL] = 0x0068;
        Self { regs }
    }

    pub(super) fn read(&self, register: usize) -> u16 {
        match register {
            REG_STATUS => STATUS_VALUE,
            REG_ID1 => ID1_VALUE,
            REG_ID2 => ID2_VALUE,
            REG_PARTNER => 0xC1E1,
            REG_EXPANSION => 0x000F,
            REG_1000T_STATUS => 0x7800,
            REG_EXT_STATUS => 0x3000,
            REG_SPECIFIC_STATUS => SPECIFIC_STATUS_VALUE,
            _ => self.regs.get(register).copied().unwrap_or(0),
        }
    }

    /// Writes to the identity and status registers are ignored; the reset bit of the control
    /// register restores the defaults and reads back as zero.
    pub(super) fn write(&mut self, register: usize, value: u16) {
        match register {
            REG_CONTROL if value & CONTROL_RESET != 0 => *self = Self::new(),
            REG_CONTROL => self.regs[REG_CONTROL] = value,
            REG_STATUS | REG_ID1 | REG_ID2 | REG_PARTNER | REG_EXPANSION | REG_1000T_STATUS
            | REG_EXT_STATUS | REG_SPECIFIC_STATUS => {}
            _ => {
                if let Some(slot) = self.regs.get_mut(register) {
                    *slot = value;
                }
            }
        }
    }
}
