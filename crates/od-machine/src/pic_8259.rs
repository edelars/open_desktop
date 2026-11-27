//! Minimal pair of 8259A interrupt controllers (S4-29a): master on ports
//! `0x20/0x21`, slave on `0xA0/0xA1`.
//!
//! The pair exists so that a Linux guest can run `probe_8259A` (write `IMR`, read
//! it back) and `init_8259A` (ICW1..ICW4) without a bus error. Everything the
//! guest can configure is either stored (`IMR`, the register select of OCW3) or
//! accepted without effect (rotation, set priority). Configurations the model
//! cannot honour are refused with [`DeviceError`] before any state changes.
//!
//! S4-39g: the master chip has an IRR and an ISR. A board source raises an
//! edge on an input line ([`Pic8259Handle::raise_irq`]: IRR bit, latched even
//! when the line is masked); [`Pic8259Handle::pending_vector`] is the `INTR`
//! output (the highest-priority unmasked request that no equal or higher
//! priority line has in service, fixed priority, IR0 highest, vector =
//! `(ICW2 & 0xF8) | line`);
//! [`Pic8259Handle::acknowledge`] is the INTA cycle (IRR bit -> ISR bit, with
//! AEOI no ISR bit); OCW2 non-specific (`0x20`) and specific (`0x60 | n`) EOI
//! (and their rotate forms, with the rotation ignored) clear ISR bits; OCW3 reads
//! return IRR or ISR. The slave chip and the cascade input IR2 are not driven
//! (no source raises a slave line); priority rotation, special mask mode, level
//! trigger and the poll command are not modelled.
//!
//! Accepted initialisation (8086 mode, normal EOI): ICW1 `0x11` (bits 7:5 and
//! bit 2 are ignored in 8086 mode), any ICW2 (S7-32 slice O, ISSUE-702: in 8086
//! mode bits 7:3 are T7–T3 of the vector, the chip supplies bits 2:0 from the
//! line, and bits 2:0 of ICW2 (A10–A8) are ignored — Intel 8259A data sheet
//! 231468-003, "Initialization Command Words 1 and 2" and "Content of Interrupt
//! Vector Byte for 8086 System Mode"; Linux `init_8259A` writes [`isa_irq_vector`]
//! of the chip, master `0x30` and slave `0x38`, the Windows HAL `0xD8` to both),
//! ICW3 `0x04` on the master and `0x02` on the slave, ICW4 `0x01`. The master also
//! accepts ICW4 `0x03` (AEOI, S4-29a2; Linux `check_timer`): the flag is stored
//! and, since S4-39g, an INTA cycle then sets no ISR bit; the slave refuses it. Single mode, level
//! trigger, special fully nested mode, buffered mode and the MCS-80 mode are
//! refused. ExtINT/LINT0 delivery, the slave's input lines, the cascade and the poll command are
//! not part of this model (the master's lines are driven since S4-39g, see above). ELCR (`0x4D0/0x4D1`, S4-38L7bs) is a separate two-port device
//! ([`Pic8259Elcr`]) whose two bytes are only stored and read back under the
//! masks `0xF8` (master) and `0xDE` (slave), the standard PIIX-compatible layout
//! from memory, not checked against the primary source.
//!
//! The chips are separate [`BusDevice`]s (each bound to its own two-port
//! `PortRegionBinding`, because a device sees offsets relative to its binding);
//! [`Pic8259Pair::new`] builds both.

use crate::{AccessWidth, BusDevice, DeviceAccess, DeviceError, IoSpace};
use std::sync::{Arc, Mutex};

/// First port of the master chip (`A0 = 0`: command, `A0 = 1`: data).
pub const PIC_MASTER_BASE_PORT: u16 = 0x20;
/// First port of the slave chip.
pub const PIC_SLAVE_BASE_PORT: u16 = 0xA0;
/// Ports of each chip.
pub const PIC_PORT_BYTES: u32 = 2;

/// Interrupt vector of ISA IRQ 0 (Linux `IRQ0_VECTOR`, `FIRST_EXTERNAL_VECTOR + 16`).
/// It is the project's `ISA_IRQ_VECTOR(0)`; no such constant existed before S4-29a.
pub const ISA_IRQ_VECTOR_BASE: u8 = 0x30;

/// `ISA_IRQ_VECTOR(irq)` of Linux for `irq` in `0..16`.
pub const fn isa_irq_vector(irq: u8) -> u8 {
    assert!(irq < 16);
    ISA_IRQ_VECTOR_BASE + irq
}

/// ICW3 of the master: the slave is wired to IR2.
pub const PIC_MASTER_ICW3: u8 = 1 << 2;
/// ICW3 of the slave: its cascade identity, IR2 of the master.
pub const PIC_SLAVE_ICW3: u8 = 2;
/// The only accepted ICW1: ICW4 needed (IC4), cascade, edge, bit 4 set.
pub const PIC_ICW1_ACCEPTED: u8 = 0x11;
/// The ICW4 accepted by both chips: 8086 mode, normal EOI, not buffered, not SFNM.
pub const PIC_ICW4_ACCEPTED: u8 = 0x01;
/// ICW4 with AEOI (S4-29a2): accepted by the master only; with it an INTA cycle sets no ISR bit (S4-39g).
pub const PIC_ICW4_MASTER_AEOI: u8 = 0x03;

const ICW1_IC4: u8 = 1 << 0;
const ICW1_SNGL: u8 = 1 << 1;
const ICW1_LTIM: u8 = 1 << 3;
const ICW1_MARK: u8 = 1 << 4;
const ICW4_UPM: u8 = 1 << 0;
const ICW4_AEOI: u8 = 1 << 1;
const ICW4_BUF: u8 = 1 << 3;
const ICW4_SFNM: u8 = 1 << 4;
const ICW4_RESERVED: u8 = 0xE0;
const OCW_SELECT_MASK: u8 = 0b0001_1000;
const OCW3_SELECT: u8 = 0b0000_1000;
const OCW3_RIS: u8 = 1 << 0;
const OCW3_RR: u8 = 1 << 1;
const OCW3_POLL: u8 = 1 << 2;
const OCW3_SMM: u8 = 1 << 5;
const OCW3_ESMM: u8 = 1 << 6;
const OCW3_BIT7: u8 = 1 << 7;
const OCW2_SPECIFIC: u8 = 1 << 6;
const OCW2_EOI: u8 = 1 << 5;

/// First port of the ELCR pair: `0x4D0` master (IRQ0..7), `0x4D1` slave (IRQ8..15).
pub const PIC_ELCR_BASE_PORT: u16 = 0x4D0;
/// Ports of the ELCR pair.
pub const PIC_ELCR_PORT_BYTES: u32 = 2;
/// Master ELCR bits that are stored: bits 3..7 (IRQ3..IRQ7). Bits 0..2 (IRQ0..2)
/// are reserved (read 0, writes dropped). Standard (PIIX-compatible) layout from
/// memory, not checked against the primary source.
pub const PIC_ELCR_MASTER_MASK: u8 = 0b1111_1000;
/// Slave ELCR bits that are stored: bits 1, 2, 3, 4, 6, 7 (IRQ9..IRQ12, IRQ14,
/// IRQ15). Bits 0 (IRQ8) and 5 (IRQ13) are reserved. Standard (PIIX-compatible)
/// layout from memory, not checked against the primary source.
pub const PIC_ELCR_SLAVE_MASK: u8 = 0b1101_1110;

/// Which chip of the pair.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Pic8259Role {
    Master,
    Slave,
}

impl Pic8259Role {
    /// First port of the chip.
    pub const fn base_port(self) -> u16 {
        match self {
            Self::Master => PIC_MASTER_BASE_PORT,
            Self::Slave => PIC_SLAVE_BASE_PORT,
        }
    }

    /// The ICW2 Linux writes to the chip (`init_8259A`): `ISA_IRQ_VECTOR(0)` or
    /// `ISA_IRQ_VECTOR(8)`. The chip accepts any ICW2 (S7-32 slice O).
    pub const fn icw2(self) -> u8 {
        match self {
            Self::Master => isa_irq_vector(0),
            Self::Slave => isa_irq_vector(8),
        }
    }

    /// The ICW3 the chip accepts.
    pub const fn icw3(self) -> u8 {
        match self {
            Self::Master => PIC_MASTER_ICW3,
            Self::Slave => PIC_SLAVE_ICW3,
        }
    }
}

/// Where the ICW1..ICW4 automaton stands.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Pic8259InitPhase {
    /// Reset state: no ICW1 has been accepted yet.
    Uninitialized,
    ExpectIcw2,
    ExpectIcw3,
    ExpectIcw4,
    /// ICW1..ICW4 completed; the chip runs in 8086 mode with its vector base.
    Initialized,
}

/// Guest-visible state of one chip; equality of two snapshots is the
/// "no state change" check of the refused accesses.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Pic8259Snapshot {
    pub phase: Pic8259InitPhase,
    /// Interrupt mask register: `0xFF` from reset.
    pub imr: u8,
    /// ICW2 as written, once written: bits 7:3 are the vector base (T7–T3), bits 2:0
    /// are ignored in 8086 mode ([`vector_of`]).
    pub icw2: Option<u8>,
    /// Accepted ICW3, once written.
    pub icw3: Option<u8>,
    /// OCW3 read-register select: `true` reads ISR, `false` reads IRR.
    pub read_isr: bool,
    /// ICW4 AEOI bit (S4-29a2): accepted on the master only. S4-39g: with it
    /// an INTA cycle ([`Pic8259Handle::acknowledge`]) sets no ISR bit.
    pub aeoi: bool,
    /// Interrupt request register (S4-39g): edges latched by `raise_irq`.
    pub irr: u8,
    /// In-service register (S4-39g): set by INTA, cleared by EOI.
    pub isr: u8,
}

/// Bits of ICW2 that form the vector in 8086 mode (T7–T3); the chip supplies bits 2:0.
const ICW2_VECTOR_BASE: u8 = 0xF8;

/// The vector an 8086-mode chip with `icw2` sends for input `line` (`0..8`): T7–T3 from ICW2
/// and the line number in bits 2:0 (8259A data sheet 231468-003, "Content of Interrupt Vector
/// Byte for 8086 System Mode").
const fn vector_of(icw2: u8, line: u8) -> u8 {
    (icw2 & ICW2_VECTOR_BASE) | line
}

impl Pic8259Snapshot {
    const RESET: Self = Self {
        phase: Pic8259InitPhase::Uninitialized,
        imr: 0xFF,
        icw2: None,
        icw3: None,
        read_isr: false,
        aeoi: false,
        irr: 0,
        isr: 0,
    };
}

/// Cloneable observer of one chip.
#[derive(Clone, Debug)]
pub struct Pic8259Handle {
    state: Arc<Mutex<Pic8259Snapshot>>,
}

impl Pic8259Handle {
    pub fn snapshot(&self) -> Pic8259Snapshot {
        *self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// An edge on input line `line` (`0..8`): the IRR bit is set whether or not the line is
    /// masked (a set bit stays set: edges coalesce). S4-39g.
    pub fn raise_irq(&self, line: u8) {
        assert!(line < 8, "a chip has eight input lines");
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.irr |= 1 << line;
    }

    /// The input line of the highest-priority request that can be delivered: unmasked, in IRR,
    /// and not blocked by an in-service line of the same or a higher priority (fixed priority,
    /// IR0 highest). `None` before the chip is initialised (no vector base).
    fn deliverable_line(state: &Pic8259Snapshot) -> Option<u8> {
        if state.phase != Pic8259InitPhase::Initialized || state.icw2.is_none() {
            return None;
        }
        let requests = state.irr & !state.imr;
        (0..8u8)
            .find_map(|line| {
                let bit = 1u8 << line;
                if state.isr & bit != 0 {
                    // The lines after an in-service one have a lower priority: blocked.
                    return Some(None);
                }
                (requests & bit != 0).then_some(Some(line))
            })
            .flatten()
    }

    /// The vector of the `INTR` output now (`(ICW2 & 0xF8) | line`), without acknowledging it.
    /// S4-39g.
    pub fn pending_vector(&self) -> Option<u8> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let line = Self::deliverable_line(&state)?;
        Some(vector_of(state.icw2?, line))
    }

    /// Whether an edge on `line` raised now would be delivered at once (the chip is initialised,
    /// the line is unmasked and no line of the same or a higher priority is in service, and no
    /// higher-priority request is waiting that would be delivered first). S4-39g.
    pub fn would_deliver(&self, line: u8) -> bool {
        assert!(line < 8, "a chip has eight input lines");
        let mut state = *self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.irr |= 1 << line;
        Self::deliverable_line(&state) == Some(line)
    }

    /// The INTA cycle for `vector`: `true` only when `vector` is the one `pending_vector`
    /// gives. The IRR bit is cleared and, without AEOI, the ISR bit is set. `false` changes
    /// nothing. S4-39g.
    pub fn acknowledge(&self, vector: u8) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(line) = Self::deliverable_line(&state) else {
            return false;
        };
        if state.icw2.map(|icw2| vector_of(icw2, line)) != Some(vector) {
            return false;
        }
        let bit = 1u8 << line;
        state.irr &= !bit;
        if !state.aeoi {
            state.isr |= bit;
        }
        true
    }
}

/// One 8259A chip. Offset 0 is the command port (`A0 = 0`), offset 1 the data port.
pub struct Pic8259 {
    role: Pic8259Role,
    state: Arc<Mutex<Pic8259Snapshot>>,
}

/// Cloneable observer of the ELCR bytes (`[master, slave]`).
#[derive(Clone, Debug)]
pub struct Pic8259ElcrHandle {
    state: Arc<Mutex<[u8; 2]>>,
}

impl Pic8259ElcrHandle {
    /// `(master, slave)` ELCR bytes.
    pub fn snapshot(&self) -> (u8, u8) {
        let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        (s[0], s[1])
    }
}

/// Edge/Level Control Registers of the pair (S4-38L7bs): offset 0 (`0x4D0`) is
/// the master, offset 1 (`0x4D1`) the slave. Byte, PIO only. The bytes are
/// stored under [`PIC_ELCR_MASTER_MASK`] / [`PIC_ELCR_SLAVE_MASK`] and read
/// back; they do not change the behaviour of the chips (edge/level is not
/// modelled). ICW1 of a chip does not touch them; they are zero from
/// construction (the model has no other reset). No access is refused except by
/// space, width and offset.
pub struct Pic8259Elcr {
    state: Arc<Mutex<[u8; 2]>>,
}

impl Pic8259Elcr {
    pub fn new_with_handle() -> (Self, Pic8259ElcrHandle) {
        let state = Arc::new(Mutex::new([0; 2]));
        (
            Self {
                state: Arc::clone(&state),
            },
            Pic8259ElcrHandle { state },
        )
    }
}

impl BusDevice for Pic8259Elcr {
    fn read(&mut self, access: DeviceAccess) -> Result<u64, DeviceError> {
        let index = Pic8259::locate(access)? as usize;
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        Ok(u64::from(state[index]))
    }

    fn write(&mut self, access: DeviceAccess, value: u64) -> Result<(), DeviceError> {
        let index = Pic8259::locate(access)? as usize;
        let mask = if index == 0 {
            PIC_ELCR_MASTER_MASK
        } else {
            PIC_ELCR_SLAVE_MASK
        };
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state[index] = (value as u8) & mask;
        Ok(())
    }
}

/// Both chips of the pair with their observers, and the ELCR device.
pub struct Pic8259Pair {
    pub master: Pic8259,
    pub slave: Pic8259,
    pub master_handle: Pic8259Handle,
    pub slave_handle: Pic8259Handle,
    pub elcr: Pic8259Elcr,
    pub elcr_handle: Pic8259ElcrHandle,
}

impl Pic8259Pair {
    pub fn new() -> Self {
        let (master, master_handle) = Pic8259::new_with_handle(Pic8259Role::Master);
        let (slave, slave_handle) = Pic8259::new_with_handle(Pic8259Role::Slave);
        let (elcr, elcr_handle) = Pic8259Elcr::new_with_handle();
        Self {
            master,
            slave,
            master_handle,
            slave_handle,
            elcr,
            elcr_handle,
        }
    }
}

impl Default for Pic8259Pair {
    fn default() -> Self {
        Self::new()
    }
}

impl Pic8259 {
    pub fn new_with_handle(role: Pic8259Role) -> (Self, Pic8259Handle) {
        let state = Arc::new(Mutex::new(Pic8259Snapshot::RESET));
        (
            Self {
                role,
                state: Arc::clone(&state),
            },
            Pic8259Handle { state },
        )
    }

    pub fn role(&self) -> Pic8259Role {
        self.role
    }

    /// Space, width and offset checks; returns `A0`.
    fn locate(access: DeviceAccess) -> Result<u64, DeviceError> {
        if access.space != IoSpace::Pio {
            return Err(DeviceError::UnsupportedSpace);
        }
        if access.width != AccessWidth::Byte {
            return Err(DeviceError::UnsupportedWidth);
        }
        if access.offset >= u64::from(PIC_PORT_BYTES) {
            return Err(DeviceError::UnknownOffset);
        }
        Ok(access.offset)
    }

    fn in_init(phase: Pic8259InitPhase) -> bool {
        matches!(
            phase,
            Pic8259InitPhase::ExpectIcw2
                | Pic8259InitPhase::ExpectIcw3
                | Pic8259InitPhase::ExpectIcw4
        )
    }

    fn write_command(&self, state: &mut Pic8259Snapshot, value: u8) -> Result<(), DeviceError> {
        if value & ICW1_MARK != 0 {
            // ICW1. Only cascade, edge, 8086 (IC4 = 1) are modelled.
            if value & (ICW1_SNGL | ICW1_LTIM) != 0 || value & ICW1_IC4 == 0 {
                return Err(DeviceError::UnsupportedConfiguration);
            }
            // As on the chip, ICW1 clears IMR and selects IRR for reads.
            *state = Pic8259Snapshot {
                phase: Pic8259InitPhase::ExpectIcw2,
                imr: 0,
                icw2: None,
                icw3: None,
                read_isr: false,
                aeoi: false,
                irr: 0,
                isr: 0,
            };
            return Ok(());
        }
        // OCW2 or OCW3 in the middle of an initialisation is refused: what the
        // chip would do there is not modelled.
        if Self::in_init(state.phase) {
            return Err(DeviceError::Busy);
        }
        if value & OCW_SELECT_MASK == OCW3_SELECT {
            // OCW3. Poll, special mask mode and bit 7 are not modelled.
            if value & (OCW3_POLL | OCW3_SMM | OCW3_ESMM | OCW3_BIT7) != 0 {
                return Err(DeviceError::UnsupportedConfiguration);
            }
            if value & OCW3_RR != 0 {
                state.read_isr = value & OCW3_RIS != 0;
            }
            return Ok(());
        }
        // OCW2. Bits 7:5 are R, SL, EOI; bits 2:0 the line of a specific command.
        debug_assert_eq!(value & OCW_SELECT_MASK, 0);
        if value & OCW2_EOI != 0 {
            let line = if value & OCW2_SPECIFIC != 0 {
                // Specific EOI (also the rotate form; the rotation is ignored).
                value & 7
            } else if let Some(line) = (0..8u8).find(|line| state.isr & (1 << line) != 0) {
                // Non-specific EOI: the highest-priority line in service.
                line
            } else {
                return Ok(());
            };
            state.isr &= !(1 << line);
        }
        // Rotation, set priority and no-operation forms: no effect (priority stays fixed).
        Ok(())
    }

    fn write_data(&self, state: &mut Pic8259Snapshot, value: u8) -> Result<(), DeviceError> {
        match state.phase {
            Pic8259InitPhase::ExpectIcw2 => {
                // Every byte is a valid ICW2 in 8086 mode (S7-32 slice O): T7–T3 are the vector
                // base and A10–A8 (bits 2:0) are ignored; the vector is `vector_of`.
                state.icw2 = Some(value);
                state.phase = Pic8259InitPhase::ExpectIcw3;
            }
            Pic8259InitPhase::ExpectIcw3 => {
                if value != self.role.icw3() {
                    return Err(DeviceError::UnsupportedConfiguration);
                }
                state.icw3 = Some(value);
                state.phase = Pic8259InitPhase::ExpectIcw4;
            }
            Pic8259InitPhase::ExpectIcw4 => {
                // Bit 2 (M/S) is a don't-care while BUF = 0, so it is ignored.
                if value & (ICW4_BUF | ICW4_SFNM | ICW4_RESERVED) != 0 || value & ICW4_UPM == 0 {
                    return Err(DeviceError::UnsupportedConfiguration);
                }
                // S4-29a2: Linux `check_timer` runs `init_8259A(1)`, which writes
                // ICW4 `0x03` (AEOI) to the master and `0x01` to the slave. AEOI
                // is stored on the master (an INTA then sets no ISR bit); the slave refuses it.
                let aeoi = value & ICW4_AEOI != 0;
                if aeoi && self.role != Pic8259Role::Master {
                    return Err(DeviceError::UnsupportedConfiguration);
                }
                state.aeoi = aeoi;
                state.phase = Pic8259InitPhase::Initialized;
            }
            // OCW1: the interrupt mask register.
            Pic8259InitPhase::Uninitialized | Pic8259InitPhase::Initialized => {
                state.imr = value;
            }
        }
        Ok(())
    }
}

impl BusDevice for Pic8259 {
    fn read(&mut self, access: DeviceAccess) -> Result<u64, DeviceError> {
        let a0 = Self::locate(access)?;
        if a0 == 1 {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            return Ok(u64::from(state.imr));
        }
        // IRR or ISR, selected by OCW3.
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        Ok(u64::from(if state.read_isr {
            state.isr
        } else {
            state.irr
        }))
    }

    fn write(&mut self, access: DeviceAccess, value: u64) -> Result<(), DeviceError> {
        let a0 = Self::locate(access)?;
        let value = value as u8;
        let state = Arc::clone(&self.state);
        let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
        // Work on a copy: a refused access must leave the state untouched.
        let mut next = *state;
        if a0 == 0 {
            self.write_command(&mut next, value)?;
        } else {
            self.write_data(&mut next, value)?;
        }
        *state = next;
        Ok(())
    }
}

#[cfg(test)]
mod irq_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ACPI_PM_BASE_PORT, ACPI_PM_PORT_BYTES, BusAccessError, FIRMWARE_COM1_PORT,
        GuestPhysicalAddress, GuestPhysicalRange, IO_APIC_MMIO_BASE, LOCAL_APIC_MMIO_BASE,
        MachineBus, MachineMemoryMap, MemoryRegionKind, MemoryRegionSpec, PortAddress, PortRange,
        PortRegionBinding, RamLimits,
    };

    const BYTE: AccessWidth = AccessWidth::Byte;

    fn acc(offset: u64, width: AccessWidth) -> DeviceAccess {
        DeviceAccess {
            space: IoSpace::Pio,
            offset,
            width,
        }
    }

    fn chip(role: Pic8259Role) -> (Pic8259, Pic8259Handle) {
        Pic8259::new_with_handle(role)
    }

    /// Writes a byte to port `offset` of the chip.
    fn out(pic: &mut Pic8259, offset: u64, value: u8) -> Result<(), DeviceError> {
        pic.write(acc(offset, BYTE), u64::from(value))
    }

    fn inb(pic: &mut Pic8259, offset: u64) -> u64 {
        pic.read(acc(offset, BYTE)).unwrap()
    }

    /// The `init_8259A` sequence of Linux for one chip, with `mask` restored last.
    fn init_chip(pic: &mut Pic8259, mask: u8) {
        let role = pic.role();
        out(pic, 0, PIC_ICW1_ACCEPTED).unwrap();
        out(pic, 1, role.icw2()).unwrap();
        out(pic, 1, role.icw3()).unwrap();
        out(pic, 1, PIC_ICW4_ACCEPTED).unwrap();
        out(pic, 1, mask).unwrap();
    }

    #[test]
    fn reset_matrix() {
        for role in [Pic8259Role::Master, Pic8259Role::Slave] {
            let (mut pic, handle) = chip(role);
            assert_eq!(
                handle.snapshot(),
                Pic8259Snapshot {
                    phase: Pic8259InitPhase::Uninitialized,
                    imr: 0xFF,
                    icw2: None,
                    icw3: None,
                    read_isr: false,
                    aeoi: false,
                    irr: 0,
                    isr: 0,
                },
                "{role:?}"
            );
            assert_eq!(inb(&mut pic, 1), 0xFF, "IMR reads all masked");
            assert_eq!(inb(&mut pic, 0), 0, "IRR reads zero by default");
        }
    }

    #[test]
    fn vector_and_cascade_constants() {
        assert_eq!(ISA_IRQ_VECTOR_BASE, 0x30);
        assert_eq!(Pic8259Role::Master.icw2(), 0x30);
        assert_eq!(Pic8259Role::Slave.icw2(), 0x38);
        assert_eq!(Pic8259Role::Master.icw3(), 0x04);
        assert_eq!(Pic8259Role::Slave.icw3(), 0x02);
        assert_eq!(Pic8259Role::Master.base_port(), 0x20);
        assert_eq!(Pic8259Role::Slave.base_port(), 0xA0);
    }

    #[test]
    fn probe_8259a_writes_imr_and_reads_it_back() {
        // Linux probe_8259A: save IMR, write 0xFF to the slave and read it,
        // write 0xFB to the master and read it, restore.
        let pair = Pic8259Pair::new();
        let (mut master, mut slave) = (pair.master, pair.slave);
        out(&mut slave, 1, 0xFF).unwrap();
        out(&mut master, 1, 0xFB).unwrap();
        assert_eq!(inb(&mut master, 1), 0xFB);
        assert_eq!(inb(&mut slave, 1), 0xFF);
        out(&mut master, 1, 0x00).unwrap();
        assert_eq!(inb(&mut master, 1), 0x00);
        assert_eq!(pair.master_handle.snapshot().imr, 0x00);
        // Neither chip is initialised by a probe.
        assert_eq!(
            pair.master_handle.snapshot().phase,
            Pic8259InitPhase::Uninitialized
        );
        assert_eq!(pair.slave_handle.snapshot().imr, 0xFF);
    }

    #[test]
    fn linux_init_sequence_is_accepted_and_imr_is_restored() {
        let pair = Pic8259Pair::new();
        let (mut master, mut slave) = (pair.master, pair.slave);
        // init_8259A: mask everything, ICW1..ICW4, restore the cached mask.
        out(&mut master, 1, 0xFF).unwrap();
        out(&mut slave, 1, 0xFF).unwrap();
        init_chip(&mut master, 0xFB);
        init_chip(&mut slave, 0xFF);
        let m = pair.master_handle.snapshot();
        assert_eq!(m.phase, Pic8259InitPhase::Initialized);
        assert_eq!((m.icw2, m.icw3, m.imr), (Some(0x30), Some(0x04), 0xFB));
        let s = pair.slave_handle.snapshot();
        assert_eq!(s.phase, Pic8259InitPhase::Initialized);
        assert_eq!((s.icw2, s.icw3, s.imr), (Some(0x38), Some(0x02), 0xFF));
        assert_eq!(inb(&mut master, 1), 0xFB);
        assert_eq!(inb(&mut slave, 1), 0xFF);
        // After initialisation the data port is OCW1 again.
        out(&mut master, 1, 0x55).unwrap();
        assert_eq!(inb(&mut master, 1), 0x55);
    }

    #[test]
    fn icw1_clears_imr_and_the_data_port_walks_icw2_icw3_icw4() {
        let (mut pic, handle) = chip(Pic8259Role::Master);
        out(&mut pic, 1, 0xA5).unwrap();
        out(&mut pic, 0, 0x11).unwrap();
        let s = handle.snapshot();
        assert_eq!(s.phase, Pic8259InitPhase::ExpectIcw2);
        assert_eq!(s.imr, 0, "ICW1 clears IMR as the chip does");
        assert_eq!(inb(&mut pic, 1), 0, "reads in the sequence return IMR");
        out(&mut pic, 1, 0x30).unwrap();
        assert_eq!(handle.snapshot().phase, Pic8259InitPhase::ExpectIcw3);
        out(&mut pic, 1, 0x04).unwrap();
        assert_eq!(handle.snapshot().phase, Pic8259InitPhase::ExpectIcw4);
        out(&mut pic, 1, 0x01).unwrap();
        assert_eq!(handle.snapshot().phase, Pic8259InitPhase::Initialized);
    }

    #[test]
    fn icw1_bits_ignored_in_8086_mode_are_accepted() {
        // A7:A5 and ADI (bit 2) are only used by MCS-80.
        let (mut pic, handle) = chip(Pic8259Role::Master);
        out(&mut pic, 0, 0xF5).unwrap();
        assert_eq!(handle.snapshot().phase, Pic8259InitPhase::ExpectIcw2);
    }

    #[test]
    fn icw1_restart_mid_sequence() {
        let (mut pic, handle) = chip(Pic8259Role::Slave);
        out(&mut pic, 0, 0x11).unwrap();
        out(&mut pic, 1, 0x38).unwrap();
        out(&mut pic, 0, 0x11).unwrap();
        let s = handle.snapshot();
        assert_eq!((s.phase, s.icw2), (Pic8259InitPhase::ExpectIcw2, None));
    }

    #[test]
    fn unsupported_icw1_is_refused_without_state_change() {
        // SNGL, LTIM, IC4 = 0 (MCS-80 mode), alone and combined.
        for icw1 in [0x13u8, 0x19, 0x1B, 0x10, 0x12, 0x18, 0x1A] {
            let (mut pic, handle) = chip(Pic8259Role::Master);
            out(&mut pic, 1, 0xA5).unwrap();
            let before = handle.snapshot();
            assert_eq!(
                out(&mut pic, 0, icw1),
                Err(DeviceError::UnsupportedConfiguration),
                "{icw1:#04x}"
            );
            assert_eq!(handle.snapshot(), before, "{icw1:#04x}");
            assert_eq!(inb(&mut pic, 1), 0xA5, "IMR survives a refused ICW1");
        }
    }

    #[test]
    fn refused_icw1_in_the_middle_of_a_sequence_keeps_the_sequence() {
        let (mut pic, handle) = chip(Pic8259Role::Master);
        out(&mut pic, 0, 0x11).unwrap();
        out(&mut pic, 1, 0x30).unwrap();
        let before = handle.snapshot();
        assert_eq!(
            out(&mut pic, 0, 0x13),
            Err(DeviceError::UnsupportedConfiguration)
        );
        assert_eq!(handle.snapshot(), before);
        out(&mut pic, 1, 0x04).unwrap();
        out(&mut pic, 1, 0x01).unwrap();
        assert_eq!(handle.snapshot().phase, Pic8259InitPhase::Initialized);
    }

    /// S7-32 slice O (ISSUE-702): in 8086 mode every byte is an ICW2 (8259A data sheet
    /// 231468-003, "Initialization Command Words 1 and 2": T7–T3 are the vector, A10–A8 are
    /// ignored). It is stored as written and only moves the automaton to ICW3; nothing else
    /// of the ICW1 state changes.
    #[test]
    fn every_icw2_is_accepted_and_stored_as_written() {
        for role in [Pic8259Role::Master, Pic8259Role::Slave] {
            let (mut pic, handle) = chip(role);
            out(&mut pic, 1, 0xA5).unwrap();
            for icw2 in 0..=u8::MAX {
                out(&mut pic, 0, PIC_ICW1_ACCEPTED).unwrap();
                let after_icw1 = handle.snapshot();
                out(&mut pic, 1, icw2).unwrap();
                assert_eq!(
                    handle.snapshot(),
                    Pic8259Snapshot {
                        phase: Pic8259InitPhase::ExpectIcw3,
                        icw2: Some(icw2),
                        ..after_icw1
                    },
                    "{role:?} {icw2:#04x}"
                );
                assert_eq!(inb(&mut pic, 1), 0, "IMR stays cleared by ICW1");
            }
        }
    }

    #[test]
    fn unsupported_icw3_is_refused_and_the_step_can_be_retried() {
        for role in [Pic8259Role::Master, Pic8259Role::Slave] {
            let (mut pic, handle) = chip(role);
            out(&mut pic, 0, 0x11).unwrap();
            // The Windows HAL vector base: ICW2 is no longer the strict step.
            out(&mut pic, 1, 0xD8).unwrap();
            let before = handle.snapshot();
            for icw3 in [0x00u8, 0x01, 0x03, 0x08, 0x06, 0xFF] {
                // The accepted value of the other role is refused too.
                if icw3 == role.icw3() {
                    continue;
                }
                assert_eq!(
                    out(&mut pic, 1, icw3),
                    Err(DeviceError::UnsupportedConfiguration),
                    "{role:?} {icw3:#04x}"
                );
                assert_eq!(handle.snapshot(), before);
            }
            assert_eq!(
                out(&mut pic, 1, if role == Pic8259Role::Master { 2 } else { 4 }),
                Err(DeviceError::UnsupportedConfiguration)
            );
            out(&mut pic, 1, role.icw3()).unwrap();
        }
    }

    #[test]
    fn unsupported_icw4_is_refused_without_state_change() {
        // Buffered (slave and master), SFNM, MCS-80 (bit 0 = 0), reserved, and
        // AEOI combined with any of those (AEOI alone is accepted on the master).
        for icw4 in [
            0x09u8, 0x0B, 0x0D, 0x11, 0x13, 0x00, 0x02, 0x08, 0x10, 0x21, 0x23, 0x41, 0x81,
        ] {
            let (mut pic, handle) = chip(Pic8259Role::Master);
            out(&mut pic, 0, 0x11).unwrap();
            out(&mut pic, 1, 0x30).unwrap();
            out(&mut pic, 1, 0x04).unwrap();
            let before = handle.snapshot();
            assert_eq!(
                out(&mut pic, 1, icw4),
                Err(DeviceError::UnsupportedConfiguration),
                "{icw4:#04x}"
            );
            assert_eq!(handle.snapshot(), before, "{icw4:#04x}");
            // A legal ICW4 still completes the sequence.
            out(&mut pic, 1, 0x01).unwrap();
            assert_eq!(handle.snapshot().phase, Pic8259InitPhase::Initialized);
        }
    }

    #[test]
    fn master_accepts_aeoi_icw4_and_stores_the_flag_without_effect() {
        // Linux check_timer: init_8259A(1) writes ICW4 0x03 to the master.
        // 0x07 is the same with the (ignored) M/S bit set.
        for icw4 in [PIC_ICW4_MASTER_AEOI, 0x07] {
            let (mut pic, handle) = chip(Pic8259Role::Master);
            out(&mut pic, 0, PIC_ICW1_ACCEPTED).unwrap();
            out(&mut pic, 1, Pic8259Role::Master.icw2()).unwrap();
            out(&mut pic, 1, Pic8259Role::Master.icw3()).unwrap();
            assert!(!handle.snapshot().aeoi);
            out(&mut pic, 1, icw4).unwrap();
            let s = handle.snapshot();
            assert_eq!(s.phase, Pic8259InitPhase::Initialized, "{icw4:#04x}");
            assert!(s.aeoi, "{icw4:#04x}");
            // The rest of the state is as after a plain ICW4 0x01.
            let (mut plain, plain_handle) = chip(Pic8259Role::Master);
            init_chip(&mut plain, 0x00);
            out(&mut pic, 1, 0xFB).unwrap();
            out(&mut plain, 1, 0xFB).unwrap();
            let (with, without) = (handle.snapshot(), plain_handle.snapshot());
            assert_eq!(
                Pic8259Snapshot {
                    aeoi: false,
                    ..with
                },
                without
            );
            // IRR/ISR read zero, IMR reads back; OCW2 EOI still has no effect.
            assert_eq!(inb(&mut pic, 1), 0xFB);
            out(&mut pic, 0, 0x0B).unwrap();
            assert_eq!(inb(&mut pic, 0), 0, "ISR");
            out(&mut pic, 0, 0x0A).unwrap();
            assert_eq!(inb(&mut pic, 0), 0, "IRR");
            let before = handle.snapshot();
            for value in [0x20u8, 0x60, 0x67, 0xA0, 0xE7, 0xC0, 0xE0, 0x40, 0x00, 0x07] {
                out(&mut pic, 0, value).unwrap();
                assert_eq!(handle.snapshot(), before, "{value:#04x}");
            }
            assert!(handle.snapshot().aeoi);
        }
    }

    #[test]
    fn slave_refuses_aeoi_icw4_without_state_change() {
        // 0x03 and 0x07 (M/S bit ignored while BUF = 0).
        for icw4 in [0x03u8, 0x07] {
            let (mut pic, handle) = chip(Pic8259Role::Slave);
            out(&mut pic, 0, PIC_ICW1_ACCEPTED).unwrap();
            out(&mut pic, 1, Pic8259Role::Slave.icw2()).unwrap();
            out(&mut pic, 1, Pic8259Role::Slave.icw3()).unwrap();
            let before = handle.snapshot();
            assert_eq!(
                out(&mut pic, 1, icw4),
                Err(DeviceError::UnsupportedConfiguration),
                "{icw4:#04x}"
            );
            assert_eq!(handle.snapshot(), before, "{icw4:#04x}");
            assert!(!handle.snapshot().aeoi);
            assert_eq!(handle.snapshot().phase, Pic8259InitPhase::ExpectIcw4);
            // A legal ICW4 still completes the sequence.
            out(&mut pic, 1, PIC_ICW4_ACCEPTED).unwrap();
            assert_eq!(handle.snapshot().phase, Pic8259InitPhase::Initialized);
            assert!(!handle.snapshot().aeoi);
        }
    }

    #[test]
    fn aeoi_flag_is_clear_from_reset_and_cleared_by_a_new_icw1() {
        let (mut pic, handle) = chip(Pic8259Role::Master);
        assert!(!handle.snapshot().aeoi, "reset");
        out(&mut pic, 0, PIC_ICW1_ACCEPTED).unwrap();
        out(&mut pic, 1, Pic8259Role::Master.icw2()).unwrap();
        out(&mut pic, 1, Pic8259Role::Master.icw3()).unwrap();
        out(&mut pic, 1, PIC_ICW4_MASTER_AEOI).unwrap();
        assert!(handle.snapshot().aeoi);
        // A restart (ICW1) clears it, also in the middle of a sequence.
        out(&mut pic, 0, PIC_ICW1_ACCEPTED).unwrap();
        assert!(!handle.snapshot().aeoi);
        out(&mut pic, 1, Pic8259Role::Master.icw2()).unwrap();
        out(&mut pic, 1, Pic8259Role::Master.icw3()).unwrap();
        out(&mut pic, 1, PIC_ICW4_ACCEPTED).unwrap();
        assert!(!handle.snapshot().aeoi, "ICW4 0x01 after AEOI init");
        // Full Linux check_timer sequence on both chips of a fresh pair.
        let pair = Pic8259Pair::new();
        let (mut master, mut slave) = (pair.master, pair.slave);
        for (pic, icw4) in [(&mut master, 0x03u8), (&mut slave, 0x01u8)] {
            let role = pic.role();
            out(pic, 0, PIC_ICW1_ACCEPTED).unwrap();
            out(pic, 1, role.icw2()).unwrap();
            out(pic, 1, role.icw3()).unwrap();
            out(pic, 1, icw4).unwrap();
        }
        assert!(pair.master_handle.snapshot().aeoi);
        assert!(!pair.slave_handle.snapshot().aeoi);
    }

    #[test]
    fn icw4_ignores_ms_bit_when_not_buffered() {
        let (mut pic, handle) = chip(Pic8259Role::Slave);
        out(&mut pic, 0, 0x11).unwrap();
        out(&mut pic, 1, 0x38).unwrap();
        out(&mut pic, 1, 0x02).unwrap();
        out(&mut pic, 1, 0x05).unwrap();
        assert_eq!(handle.snapshot().phase, Pic8259InitPhase::Initialized);
    }

    #[test]
    fn ocw2_eoi_has_no_effect() {
        let (mut pic, handle) = chip(Pic8259Role::Master);
        init_chip(&mut pic, 0xFB);
        out(&mut pic, 0, 0x0B).unwrap(); // select ISR first
        let before = handle.snapshot();
        // Non-specific, specific (levels 0..7), rotate, set priority, no-op.
        for value in [0x20u8, 0x60, 0x67, 0xA0, 0xE7, 0xC0, 0xE0, 0x40, 0x00, 0x07] {
            out(&mut pic, 0, value).unwrap();
            assert_eq!(handle.snapshot(), before, "{value:#04x}");
        }
        assert_eq!(inb(&mut pic, 0), 0);
        assert_eq!(inb(&mut pic, 1), 0xFB);
    }

    #[test]
    fn ocw3_selects_irr_or_isr_and_both_read_zero() {
        let (mut pic, handle) = chip(Pic8259Role::Master);
        assert!(!handle.snapshot().read_isr);
        out(&mut pic, 0, 0x0B).unwrap();
        assert!(handle.snapshot().read_isr);
        assert_eq!(inb(&mut pic, 0), 0, "ISR");
        out(&mut pic, 0, 0x0A).unwrap();
        assert!(!handle.snapshot().read_isr);
        assert_eq!(inb(&mut pic, 0), 0, "IRR");
        // RR = 0 leaves the selection alone.
        out(&mut pic, 0, 0x0B).unwrap();
        out(&mut pic, 0, 0x08).unwrap();
        out(&mut pic, 0, 0x09).unwrap();
        assert!(handle.snapshot().read_isr);
        // ICW1 returns the read to IRR.
        out(&mut pic, 0, 0x11).unwrap();
        assert!(!handle.snapshot().read_isr);
    }

    #[test]
    fn unsupported_ocw3_is_refused_without_state_change() {
        let (mut pic, handle) = chip(Pic8259Role::Master);
        out(&mut pic, 0, 0x0B).unwrap();
        let before = handle.snapshot();
        // Poll, ESMM, ESMM+SMM, bit 7 (and poll with a register select).
        for value in [0x0Cu8, 0x0E, 0x4A, 0x6A, 0x0F, 0x28, 0x88, 0x8A] {
            assert_eq!(
                out(&mut pic, 0, value),
                Err(DeviceError::UnsupportedConfiguration),
                "{value:#04x}"
            );
            assert_eq!(handle.snapshot(), before, "{value:#04x}");
        }
    }

    #[test]
    fn ocw2_and_ocw3_in_the_middle_of_initialisation_are_busy() {
        let (mut pic, handle) = chip(Pic8259Role::Master);
        out(&mut pic, 0, 0x11).unwrap();
        let before = handle.snapshot();
        assert_eq!(out(&mut pic, 0, 0x20), Err(DeviceError::Busy));
        assert_eq!(out(&mut pic, 0, 0x0B), Err(DeviceError::Busy));
        assert_eq!(handle.snapshot(), before);
    }

    #[test]
    fn invalid_accesses_are_rejected_without_side_effect() {
        let (mut pic, handle) = chip(Pic8259Role::Master);
        out(&mut pic, 1, 0x12).unwrap();
        let before = handle.snapshot();
        let mmio = DeviceAccess {
            space: IoSpace::Mmio,
            offset: 1,
            width: BYTE,
        };
        assert_eq!(pic.read(mmio), Err(DeviceError::UnsupportedSpace));
        assert_eq!(pic.write(mmio, 0), Err(DeviceError::UnsupportedSpace));
        for width in [AccessWidth::Word, AccessWidth::Dword, AccessWidth::Qword] {
            assert_eq!(pic.read(acc(1, width)), Err(DeviceError::UnsupportedWidth));
            assert_eq!(
                pic.write(acc(1, width), 0),
                Err(DeviceError::UnsupportedWidth)
            );
        }
        for offset in [2u64, 3, 0x20] {
            assert_eq!(pic.read(acc(offset, BYTE)), Err(DeviceError::UnknownOffset));
            assert_eq!(
                pic.write(acc(offset, BYTE), 0),
                Err(DeviceError::UnknownOffset)
            );
        }
        assert_eq!(handle.snapshot(), before);
    }

    /// The pair behind two real `PortRegionBinding`s on a `MachineBus`.
    fn bus() -> (MachineBus, Pic8259Handle, Pic8259Handle) {
        let pair = Pic8259Pair::new();
        let map = MachineMemoryMap::new(
            36,
            vec![MemoryRegionSpec::new(
                GuestPhysicalRange::new(GuestPhysicalAddress::new(0), 1 << 20).unwrap(),
                MemoryRegionKind::Ram,
            )],
        )
        .unwrap();
        let ports = vec![
            PortRegionBinding {
                range: PortRange::new(PortAddress::new(PIC_MASTER_BASE_PORT), PIC_PORT_BYTES)
                    .unwrap(),
                device_index: 0,
            },
            PortRegionBinding {
                range: PortRange::new(PortAddress::new(PIC_SLAVE_BASE_PORT), PIC_PORT_BYTES)
                    .unwrap(),
                device_index: 1,
            },
        ];
        let bus = MachineBus::new(
            map,
            RamLimits::new(1 << 20),
            vec![Box::new(pair.master), Box::new(pair.slave)],
            Vec::new(),
            ports,
        )
        .unwrap();
        (bus, pair.master_handle, pair.slave_handle)
    }

    #[test]
    fn probe_and_init_through_the_machine_bus_ports() {
        let (mut bus, master, slave) = bus();
        let p = |port: u16| PortAddress::new(port);
        // probe_8259A.
        bus.write_port(p(0xA1), BYTE, 0xFF).unwrap();
        bus.write_port(p(0x21), BYTE, 0xFB).unwrap();
        assert_eq!(bus.read_port(p(0x21), BYTE), Ok(0xFB));
        assert_eq!(bus.read_port(p(0xA1), BYTE), Ok(0xFF));
        // init_8259A on both chips.
        for (cmd, data, role) in [
            (0x20u16, 0x21u16, Pic8259Role::Master),
            (0xA0, 0xA1, Pic8259Role::Slave),
        ] {
            bus.write_port(p(cmd), BYTE, 0x11).unwrap();
            bus.write_port(p(data), BYTE, u32::from(role.icw2()))
                .unwrap();
            bus.write_port(p(data), BYTE, u32::from(role.icw3()))
                .unwrap();
            bus.write_port(p(data), BYTE, 0x01).unwrap();
        }
        bus.write_port(p(0x21), BYTE, 0xFB).unwrap();
        bus.write_port(p(0xA1), BYTE, 0xFF).unwrap();
        assert_eq!(master.snapshot().phase, Pic8259InitPhase::Initialized);
        assert_eq!(slave.snapshot().phase, Pic8259InitPhase::Initialized);
        assert_eq!(bus.read_port(p(0x21), BYTE), Ok(0xFB));
        // OCW2 and OCW3 through the bus.
        bus.write_port(p(0x20), BYTE, 0x20).unwrap();
        bus.write_port(p(0xA0), BYTE, 0x20).unwrap();
        bus.write_port(p(0x20), BYTE, 0x0B).unwrap();
        assert_eq!(bus.read_port(p(0x20), BYTE), Ok(0));
        bus.write_port(p(0x20), BYTE, 0x0A).unwrap();
        assert_eq!(bus.read_port(p(0x20), BYTE), Ok(0));
        // A refused configuration surfaces as a bus device error and does not
        // change the chip.
        let before = master.snapshot();
        assert!(matches!(
            bus.write_port(p(0x20), BYTE, 0x13),
            Err(BusAccessError::Device {
                source: DeviceError::UnsupportedConfiguration,
                ..
            })
        ));
        assert_eq!(master.snapshot(), before);
        // Word access is refused by the device; ports next to the chips are not bound.
        assert!(matches!(
            bus.read_port(p(0x20), AccessWidth::Word),
            Err(BusAccessError::Device {
                source: DeviceError::UnsupportedWidth,
                ..
            })
        ));
        for port in [0x1Fu16, 0x22, 0x9F, 0xA2] {
            assert_eq!(
                bus.read_port(p(port), BYTE),
                Err(BusAccessError::UnmappedPort(p(port))),
                "{port:#x}"
            );
        }
    }

    /// ISSUE-702: the 8259 initialisation of the Windows HAL (`ntkrnlmp` + `0x56a500`, in its
    /// order, called once per chip): ICW1 `0x11` to the command port, then ICW2 `0xD8` (the same
    /// vector base for both chips), ICW3 `0x04` (master) / `0x02` (slave), ICW4 `0x01` and OCW1
    /// `0xFF` to the data port.
    #[test]
    fn windows_hal_init_sequence_is_accepted_on_both_chips() {
        let (mut bus, master, slave) = bus();
        let p = |port: u16| PortAddress::new(port);
        for (cmd, data, icw3) in [(0x20u16, 0x21u16, 0x04u32), (0xA0, 0xA1, 0x02)] {
            for (port, value) in [
                (cmd, 0x11u32),
                (data, 0xD8),
                (data, icw3),
                (data, 0x01),
                (data, 0xFF),
            ] {
                bus.write_port(p(port), BYTE, value)
                    .unwrap_or_else(|e| panic!("{port:#x} <- {value:#x}: {e:?}"));
            }
        }
        for (handle, icw3) in [(&master, 0x04u8), (&slave, 0x02)] {
            assert_eq!(
                handle.snapshot(),
                Pic8259Snapshot {
                    phase: Pic8259InitPhase::Initialized,
                    imr: 0xFF,
                    icw2: Some(0xD8),
                    icw3: Some(icw3),
                    read_isr: false,
                    aeoi: false,
                    irr: 0,
                    isr: 0,
                }
            );
        }
        assert_eq!(bus.read_port(p(0x21), BYTE), Ok(0xFF));
        assert_eq!(bus.read_port(p(0xA1), BYTE), Ok(0xFF));
        // Everything is masked: an edge is latched and nothing is pending.
        master.raise_irq(0);
        assert_eq!(master.pending_vector(), None);
    }

    #[test]
    fn ports_do_not_overlap_other_devices_and_fit_one_bus() {
        use crate::AcpiPm;
        use crate::pci_ecam::{PCI_CONF1_BASE_PORT, PCI_CONF1_PORT_BYTES};
        let chips = [
            (PIC_MASTER_BASE_PORT, PIC_PORT_BYTES),
            (PIC_SLAVE_BASE_PORT, PIC_PORT_BYTES),
        ];
        // COM1, conf1 and the PM block, as `(start, len)` of PIO ranges.
        let others = [
            ("COM1", FIRMWARE_COM1_PORT, 8u32),
            ("conf1", PCI_CONF1_BASE_PORT, PCI_CONF1_PORT_BYTES),
            ("PM block", ACPI_PM_BASE_PORT, ACPI_PM_PORT_BYTES),
        ];
        for &(start, len) in &chips {
            for &(name, other, other_len) in &others {
                let a = u32::from(start)..u32::from(start) + len;
                let b = u32::from(other)..u32::from(other) + other_len;
                assert!(a.end <= b.start || b.end <= a.start, "{name}");
            }
        }
        assert!(chips[0].0 as u32 + chips[0].1 <= chips[1].0 as u32);
        // NVMe, ECAM, LAPIC and IOAPIC are MMIO on a separate scale: no PIO bindings.
        const { assert!(LOCAL_APIC_MMIO_BASE > 0xFFFF && IO_APIC_MMIO_BASE > 0xFFFF) };
        // One bus with the pair, the PM block and stand-ins for COM1 and conf1
        // accepts all bindings: `MachineBus::new` rejects overlapping ports.
        struct Stub;
        impl BusDevice for Stub {
            fn read(&mut self, _: DeviceAccess) -> Result<u64, DeviceError> {
                Ok(0)
            }
            fn write(&mut self, _: DeviceAccess, _: u64) -> Result<(), DeviceError> {
                Ok(())
            }
        }
        let pair = Pic8259Pair::new();
        let clock = crate::VirtualClockHandle::new(1_000_000_000).unwrap();
        let (pm, _) = AcpiPm::new_with_handle(clock);
        let devices: Vec<Box<dyn BusDevice>> = vec![
            Box::new(pair.master),
            Box::new(pair.slave),
            Box::new(pm),
            Box::new(Stub),
            Box::new(Stub),
        ];
        let range = |start: u16, len: u32| PortRange::new(PortAddress::new(start), len).unwrap();
        let ports = vec![
            PortRegionBinding {
                range: range(PIC_MASTER_BASE_PORT, PIC_PORT_BYTES),
                device_index: 0,
            },
            PortRegionBinding {
                range: range(PIC_SLAVE_BASE_PORT, PIC_PORT_BYTES),
                device_index: 1,
            },
            PortRegionBinding {
                range: range(ACPI_PM_BASE_PORT, ACPI_PM_PORT_BYTES),
                device_index: 2,
            },
            PortRegionBinding {
                range: range(FIRMWARE_COM1_PORT, 8),
                device_index: 3,
            },
            PortRegionBinding {
                range: range(PCI_CONF1_BASE_PORT, PCI_CONF1_PORT_BYTES),
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

    fn elcr_out(dev: &mut Pic8259Elcr, offset: u64, value: u8) -> Result<(), DeviceError> {
        dev.write(acc(offset, BYTE), u64::from(value))
    }

    fn elcr_in(dev: &mut Pic8259Elcr, offset: u64) -> u64 {
        dev.read(acc(offset, BYTE)).unwrap()
    }

    #[test]
    fn elcr_reads_zero_from_construction() {
        let (mut elcr, handle) = Pic8259Elcr::new_with_handle();
        assert_eq!(elcr_in(&mut elcr, 0), 0);
        assert_eq!(elcr_in(&mut elcr, 1), 0);
        assert_eq!(handle.snapshot(), (0, 0));
    }

    #[test]
    fn master_elcr_keeps_bits_3_to_7_only() {
        let (mut elcr, handle) = Pic8259Elcr::new_with_handle();
        elcr_out(&mut elcr, 0, 0x08).unwrap();
        assert_eq!(elcr_in(&mut elcr, 0), 0x08);
        elcr_out(&mut elcr, 0, 0x10).unwrap();
        assert_eq!(elcr_in(&mut elcr, 0), 0x10);
        // IRQ5.
        elcr_out(&mut elcr, 0, 0x20).unwrap();
        assert_eq!(elcr_in(&mut elcr, 0), 0x20);
        elcr_out(&mut elcr, 0, 0xFF).unwrap();
        assert_eq!(elcr_in(&mut elcr, 0), 0xF8);
        // The slave register is independent.
        assert_eq!(elcr_in(&mut elcr, 1), 0);
        elcr_out(&mut elcr, 0, 0x00).unwrap();
        assert_eq!(elcr_in(&mut elcr, 0), 0);
        // Reserved bits alone (0..2) store nothing.
        elcr_out(&mut elcr, 0, 0x07).unwrap();
        assert_eq!(handle.snapshot(), (0, 0));
    }

    #[test]
    fn slave_elcr_keeps_bits_1_2_3_4_6_7_only() {
        let (mut elcr, handle) = Pic8259Elcr::new_with_handle();
        elcr_out(&mut elcr, 1, 0xFF).unwrap();
        assert_eq!(elcr_in(&mut elcr, 1), 0xDE);
        assert_eq!(elcr_in(&mut elcr, 0), 0);
        // Reserved bits alone (0 and 5) store nothing.
        elcr_out(&mut elcr, 1, 0x21).unwrap();
        assert_eq!(elcr_in(&mut elcr, 1), 0);
        // IRQ15.
        elcr_out(&mut elcr, 1, 0x80).unwrap();
        assert_eq!(elcr_in(&mut elcr, 1), 0x80);
        elcr_out(&mut elcr, 1, 0x02).unwrap();
        assert_eq!(handle.snapshot(), (0, 0x02));
    }

    #[test]
    fn elcr_accepts_every_byte_and_refuses_only_wrong_space_width_offset() {
        let (mut elcr, _) = Pic8259Elcr::new_with_handle();
        for offset in 0..2 {
            for value in 0..=255u8 {
                elcr_out(&mut elcr, offset, value).unwrap();
            }
        }
        let wide = acc(0, AccessWidth::Word);
        assert_eq!(elcr.read(wide), Err(DeviceError::UnsupportedWidth));
        assert_eq!(elcr.write(wide, 0), Err(DeviceError::UnsupportedWidth));
        assert_eq!(elcr.read(acc(2, BYTE)), Err(DeviceError::UnknownOffset));
        let mmio = DeviceAccess {
            space: IoSpace::Mmio,
            offset: 0,
            width: BYTE,
        };
        assert_eq!(elcr.read(mmio), Err(DeviceError::UnsupportedSpace));
    }

    #[test]
    fn elcr_does_not_change_pic_state_and_icw1_does_not_clear_elcr() {
        let pair = Pic8259Pair::new();
        let Pic8259Pair {
            mut master,
            mut slave,
            master_handle,
            slave_handle,
            mut elcr,
            elcr_handle,
        } = pair;
        init_chip(&mut master, 0xFB);
        init_chip(&mut slave, 0xFF);
        let (m0, s0) = (master_handle.snapshot(), slave_handle.snapshot());
        elcr_out(&mut elcr, 0, 0xFF).unwrap();
        elcr_out(&mut elcr, 1, 0xFF).unwrap();
        assert_eq!(elcr_handle.snapshot(), (0xF8, 0xDE));
        assert_eq!(master_handle.snapshot(), m0);
        assert_eq!(slave_handle.snapshot(), s0);
        assert_eq!(inb(&mut master, 1), 0xFB);
        assert_eq!(inb(&mut master, 0), 0);
        // A new ICW1 resets IMR but leaves ELCR as it was.
        out(&mut master, 0, PIC_ICW1_ACCEPTED).unwrap();
        assert_eq!(master_handle.snapshot().imr, 0);
        assert_eq!(elcr_handle.snapshot(), (0xF8, 0xDE));
    }

    #[test]
    fn elcr_is_visible_through_the_machine_bus_ports() {
        let pair = Pic8259Pair::new();
        let devices: Vec<Box<dyn BusDevice>> = vec![Box::new(pair.elcr)];
        let ports = vec![PortRegionBinding {
            range: PortRange::new(PortAddress::new(PIC_ELCR_BASE_PORT), PIC_ELCR_PORT_BYTES)
                .unwrap(),
            device_index: 0,
        }];
        let map = MachineMemoryMap::new(
            36,
            vec![MemoryRegionSpec::new(
                GuestPhysicalRange::new(GuestPhysicalAddress::new(0), 1 << 20).unwrap(),
                MemoryRegionKind::Ram,
            )],
        )
        .unwrap();
        let mut bus =
            MachineBus::new(map, RamLimits::new(1 << 20), devices, Vec::new(), ports).unwrap();
        let port = |p: u16| PortAddress::new(p);
        assert_eq!(bus.read_port(port(0x4D0), BYTE).unwrap(), 0);
        assert_eq!(bus.read_port(port(0x4D1), BYTE).unwrap(), 0);
        bus.write_port(port(0x4D0), BYTE, 0xFF).unwrap();
        bus.write_port(port(0x4D1), BYTE, 0xFF).unwrap();
        assert_eq!(bus.read_port(port(0x4D0), BYTE).unwrap(), 0xF8);
        assert_eq!(bus.read_port(port(0x4D1), BYTE).unwrap(), 0xDE);
        assert_eq!(pair.elcr_handle.snapshot(), (0xF8, 0xDE));
    }
}
