//! Minimal pair of 8259A interrupt controllers (S4-29a): master on ports
//! `0x20/0x21`, slave on `0xA0/0xA1`.
//!
//! The pair has no input lines and no `INTR` output: it delivers nothing. It
//! exists so that a Linux guest can run `probe_8259A` (write `IMR`, read it
//! back) and `init_8259A` (ICW1..ICW4) without a bus error. Everything the
//! guest can configure is either stored (`IMR`, the register select of OCW3) or
//! accepted without effect (OCW2). Configurations the model cannot honour are
//! refused with [`DeviceError`] before any state changes.
//!
//! Accepted initialisation (what Linux writes in `init_8259A`, 8086 mode, normal
//! EOI): ICW1 `0x11` (bits 7:5 and bit 2 are ignored in 8086 mode), ICW2 equal
//! to [`isa_irq_vector`] of the chip (master `0x30`, slave `0x38`), ICW3 `0x04`
//! on the master and `0x02` on the slave, ICW4 `0x01`. Single mode, level
//! trigger, special fully nested mode, buffered mode, automatic EOI and the
//! MCS-80 mode are refused. ELCR (`0x4D0/0x4D1`), ExtINT/LINT0 delivery, IRQ
//! lines and the poll command are not part of this model.
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
/// The only accepted ICW4: 8086 mode, normal EOI, not buffered, not SFNM.
pub const PIC_ICW4_ACCEPTED: u8 = 0x01;

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

    /// The ICW2 the chip accepts: `ISA_IRQ_VECTOR(0)` or `ISA_IRQ_VECTOR(8)`.
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
    /// Accepted ICW2 (vector base), once written.
    pub icw2: Option<u8>,
    /// Accepted ICW3, once written.
    pub icw3: Option<u8>,
    /// OCW3 read-register select: `true` reads ISR, `false` reads IRR.
    pub read_isr: bool,
}

impl Pic8259Snapshot {
    const RESET: Self = Self {
        phase: Pic8259InitPhase::Uninitialized,
        imr: 0xFF,
        icw2: None,
        icw3: None,
        read_isr: false,
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
}

/// One 8259A chip. Offset 0 is the command port (`A0 = 0`), offset 1 the data port.
pub struct Pic8259 {
    role: Pic8259Role,
    state: Arc<Mutex<Pic8259Snapshot>>,
}

/// Both chips of the pair with their observers.
pub struct Pic8259Pair {
    pub master: Pic8259,
    pub slave: Pic8259,
    pub master_handle: Pic8259Handle,
    pub slave_handle: Pic8259Handle,
}

impl Pic8259Pair {
    pub fn new() -> Self {
        let (master, master_handle) = Pic8259::new_with_handle(Pic8259Role::Master);
        let (slave, slave_handle) = Pic8259::new_with_handle(Pic8259Role::Slave);
        Self {
            master,
            slave,
            master_handle,
            slave_handle,
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
        // OCW2 (EOI, rotate, set priority): no interrupt is in service, no effect.
        debug_assert_eq!(value & OCW_SELECT_MASK, 0);
        Ok(())
    }

    fn write_data(&self, state: &mut Pic8259Snapshot, value: u8) -> Result<(), DeviceError> {
        match state.phase {
            Pic8259InitPhase::ExpectIcw2 => {
                if value != self.role.icw2() {
                    return Err(DeviceError::UnsupportedConfiguration);
                }
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
                if value & (ICW4_AEOI | ICW4_BUF | ICW4_SFNM | ICW4_RESERVED) != 0
                    || value & ICW4_UPM == 0
                {
                    return Err(DeviceError::UnsupportedConfiguration);
                }
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
        // IRR or ISR, selected by OCW3. No line and no service state exist.
        Ok(0)
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

    #[test]
    fn unsupported_icw2_icw3_are_refused_and_the_step_can_be_retried() {
        for role in [Pic8259Role::Master, Pic8259Role::Slave] {
            let (mut pic, handle) = chip(role);
            out(&mut pic, 0, 0x11).unwrap();
            let before = handle.snapshot();
            // Another vector base, including the legacy PC ones and a base with
            // low bits set (those bits are not part of an 8086 vector base).
            for icw2 in [0x08u8, 0x70, 0x20, 0x31, 0x00] {
                assert_eq!(
                    out(&mut pic, 1, icw2),
                    Err(DeviceError::UnsupportedConfiguration),
                    "{role:?} {icw2:#04x}"
                );
                assert_eq!(handle.snapshot(), before);
            }
            out(&mut pic, 1, role.icw2()).unwrap();
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
        // AEOI, buffered (slave and master), SFNM, MCS-80 (bit 0 = 0), reserved.
        for icw4 in [
            0x03u8, 0x09, 0x0D, 0x11, 0x00, 0x02, 0x08, 0x10, 0x21, 0x41, 0x81,
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
}
