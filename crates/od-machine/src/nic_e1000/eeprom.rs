//! The configuration EEPROM (NIC_DEVICE.md section 5): a 64-word Microwire part holding the MAC
//! address, a few identity words and the checksum, reachable through EERD and through the
//! EECD bit-bang interface that the Linux driver uses for this chip.

use super::regs::{EECD_CS, EECD_DI, EECD_SK};

pub(super) const WORDS: usize = 64;
/// The words 0..=0x3F sum to this value.
const CHECKSUM_TARGET: u16 = 0xBABA;
const CHECKSUM_WORD: usize = 0x3F;
/// Opcode of READ with its start bit: 1 10.
const READ_OPCODE: u16 = 0b110;
const ADDRESS_BITS: u8 = 6;
const COMMAND_BITS: u8 = 3 + ADDRESS_BITS;

/// The image for `mac`: words 0..=2 the address (little-endian pairs), 0x0B..=0x0E the
/// subsystem and device identifiers of the 82540EM, 0x3F the checksum.
pub(super) fn image(mac: [u8; 6]) -> [u16; WORDS] {
    let mut words = [0u16; WORDS];
    for (index, pair) in mac.chunks(2).enumerate() {
        words[index] = u16::from(pair[0]) | (u16::from(pair[1]) << 8);
    }
    words[0x0B] = 0x001E;
    words[0x0C] = 0x8086;
    words[0x0D] = 0x100E;
    words[0x0E] = 0x8086;
    let sum = words[..CHECKSUM_WORD]
        .iter()
        .fold(0u16, |sum, word| sum.wrapping_add(*word));
    words[CHECKSUM_WORD] = CHECKSUM_TARGET.wrapping_sub(sum);
    words
}

/// The Microwire protocol on SK edges: nine bits of start, opcode and address, then a dummy zero
/// and sixteen data bits on DO, most significant bit first.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    /// Chip select low, or a command that is not READ: nothing is driven.
    Idle,
    Command {
        bits: u8,
        value: u16,
    },
    Data {
        sent: u8,
        word: u16,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Microwire {
    phase: Phase,
    sk: bool,
    data_out: bool,
}

impl Microwire {
    pub(super) const fn new() -> Self {
        Self {
            phase: Phase::Idle,
            sk: false,
            data_out: false,
        }
    }

    pub(super) fn data_out(&self) -> bool {
        self.data_out
    }

    /// The guest wrote EECD: `value` carries the new SK, CS and DI levels.
    pub(super) fn update(&mut self, value: u32, eeprom: &[u16; WORDS]) {
        let sk = value & EECD_SK != 0;
        let rising = sk && !self.sk;
        self.sk = sk;
        if value & EECD_CS == 0 {
            self.phase = Phase::Idle;
            self.data_out = false;
            return;
        }
        if !rising {
            return;
        }
        let di = value & EECD_DI != 0;
        self.phase = match self.phase {
            Phase::Idle => {
                // Leading zeros before the start bit are ignored.
                if di {
                    Phase::Command { bits: 1, value: 1 }
                } else {
                    Phase::Idle
                }
            }
            Phase::Command { bits, value } => {
                let value = (value << 1) | u16::from(di);
                let bits = bits + 1;
                if bits < COMMAND_BITS {
                    Phase::Command { bits, value }
                } else if value >> ADDRESS_BITS == READ_OPCODE {
                    // The dummy zero appears after the last address bit.
                    self.data_out = false;
                    let address = usize::from(value & ((1 << ADDRESS_BITS) - 1));
                    Phase::Data {
                        sent: 0,
                        word: eeprom[address],
                    }
                } else {
                    Phase::Idle
                }
            }
            Phase::Data { sent, word } if sent < 16 => {
                self.data_out = word >> (15 - sent) & 1 != 0;
                Phase::Data {
                    sent: sent + 1,
                    word,
                }
            }
            Phase::Data { .. } => {
                self.data_out = false;
                Phase::Idle
            }
        };
    }
}
