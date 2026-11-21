//! ACPI fixed hardware for a full FADT (S7-14a): PM1a event and control blocks,
//! the PM timer and a one-byte reset register, all on one PIO block.
//!
//! The block is `0x0600..0x0610` (see [`ACPI_PM_DESCRIPTOR`]). The board registers
//! it behind an opt-in flag (`FirmwareBoardConfig::acpi_pm`); the handoff block
//! does not describe it, the descriptor is the single source of the future FADT
//! (S4-31). `HW_REDUCED_ACPI`, GPE blocks, PM1b, an SCI route, RTC/CMOS, HPET and
//! the legacy PIC/PIT/8042 are not part of this device.
//!
//! Stopping: a write of the S5 sleep command or of [`ACPI_RESET_VALUE`] latches a
//! [`MachineStopReason`] on the device. The run loop polls
//! [`AcpiPmHandle::stop_request`] after every instruction and stops the machine
//! with that reason. The device only reports the request; the reset of the machine is
//! `LoadedFirmwareBoard::platform_reset` (S7-26), which also calls
//! [`AcpiPmHandle::platform_reset`] to take the PM block back to its power-on state.
//!
//! Port tick (S4-37a2, ISSUE-067): every successful read of the PM timer register
//! queues `ACPI_PM_TMR_READ_TICK_NS` (1 us) of virtual time; the value returned
//! is sampled before the tick. The device never moves the clock: the clock owner
//! (the board's `run_boot`) takes the queued time with
//! `AcpiPmHandle::take_timer_tick_ns`.

use crate::{AccessWidth, BusDevice, DeviceAccess, DeviceError, IoSpace, VirtualClockHandle};
use std::sync::{Arc, Mutex};

/// First port of the PM block.
pub const ACPI_PM_BASE_PORT: u16 = 0x0600;
/// Length of the PM block; ports `0x0600..0x0610` belong to the device.
pub const ACPI_PM_PORT_BYTES: u32 = 0x10;

/// `PM1a_EVT_BLK`: `PM1_STS` (16 bit) followed by `PM1_EN` (16 bit).
pub const ACPI_PM1A_EVT_BLK: u16 = 0x0600;
pub const ACPI_PM1_EVT_LEN: u8 = 4;
/// `PM1a_CNT_BLK`: `PM1_CNT` (16 bit).
pub const ACPI_PM1A_CNT_BLK: u16 = 0x0604;
pub const ACPI_PM1_CNT_LEN: u8 = 2;
/// `PM_TMR_BLK`: the 24-bit PM timer (`TMR_VAL_EXT = 0`), read as 32 bits.
pub const ACPI_PM_TMR_BLK: u16 = 0x0608;
pub const ACPI_PM_TMR_LEN: u8 = 4;
/// `RESET_REG` (generic address: SystemIO, 8 bit, offset 0).
pub const ACPI_RESET_REG: u16 = 0x060C;
pub const ACPI_RESET_REG_WIDTH_BITS: u8 = 8;
/// `RESET_VALUE` of the FADT.
pub const ACPI_RESET_VALUE: u8 = 0x06;
/// `SMI_CMD`: zero, there is no SMI handler and no ACPI enable/disable command.
pub const ACPI_SMI_CMD: u16 = 0;
/// PM timer frequency fixed by the ACPI specification.
pub const ACPI_PM_TIMER_HZ: u64 = 3_579_545;
/// Virtual time queued by one guest read of the PM timer (the port tick), in
/// nanoseconds (ISSUE-067).
pub(crate) const ACPI_PM_TMR_READ_TICK_NS: u64 = 1_000;
/// `SLP_TYP` the board's `\_S5` package will carry (S4-31 DSDT).
pub const ACPI_SLP_TYP_S5: u8 = 5;

/// `PM1_CNT` bits.
pub const ACPI_PM1_CNT_SCI_EN: u16 = 1 << 0;
pub const ACPI_PM1_CNT_BM_RLD: u16 = 1 << 1;
pub const ACPI_PM1_CNT_GBL_RLS: u16 = 1 << 2;
pub const ACPI_PM1_CNT_SLP_TYP_SHIFT: u16 = 10;
pub const ACPI_PM1_CNT_SLP_TYP_MASK: u16 = 0x7 << ACPI_PM1_CNT_SLP_TYP_SHIFT;
pub const ACPI_PM1_CNT_SLP_EN: u16 = 1 << 13;

/// `PM1_EN` bits the device stores: `TMR_EN`, `GBL_EN`, `PWRBTN_EN`, `SLPBTN_EN`,
/// `RTC_EN`. No event source exists, so no enabled event ever fires.
const PM1_EN_MASK: u16 = 0x0721;
/// `PM1_CNT` bits stored besides the fixed `SCI_EN`: `BM_RLD` and `SLP_TYP`.
const PM1_CNT_STORED: u16 = ACPI_PM1_CNT_BM_RLD | ACPI_PM1_CNT_SLP_TYP_MASK;
const PM_TIMER_MASK: u32 = 0x00FF_FFFF;
const NANOS_PER_SECOND: u128 = 1_000_000_000;

/// Typed board descriptor of the PM block: everything the FADT needs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AcpiPmDescriptor {
    pub smi_cmd: u16,
    pub pm1a_evt_blk: u16,
    pub pm1_evt_len: u8,
    pub pm1a_cnt_blk: u16,
    pub pm1_cnt_len: u8,
    pub pm_tmr_blk: u16,
    pub pm_tmr_len: u8,
    /// `TMR_VAL_EXT` flag: `false` means a 24-bit timer.
    pub tmr_val_ext: bool,
    pub reset_reg: u16,
    pub reset_reg_width_bits: u8,
    pub reset_value: u8,
    pub timer_hz: u64,
    pub slp_typ_s5: u8,
    /// First port and length of the whole block bound on the bus.
    pub base_port: u16,
    pub port_bytes: u32,
}

/// SCI route of the board (S4-31c decision): GSI 9, level-triggered, active-low,
/// delivered through an interrupt source override (the MADT entry is S4-31f).
/// The PM block raises no interrupt yet; this is a manifest/handoff fact only.
pub const ACPI_SCI_GSI: u32 = 9;
/// `irq_flags` of the SCI in a handoff device entry: bit 0 level, bit 1 active-low.
pub const ACPI_SCI_IRQ_FLAGS: u32 = 0b11;

impl AcpiPmDescriptor {
    /// Handoff `param0` of device code 9: the four register ports as `u16` lanes,
    /// `PM1a_EVT_BLK` (bits 0..16), `PM1a_CNT_BLK` (16..32), `PM_TMR_BLK` (32..48)
    /// and `RESET_REG` (48..64). Lengths and widths are the fixed constants of
    /// this descriptor (4, 2, 4 bytes and an 8-bit reset register).
    pub const fn handoff_param0(&self) -> u64 {
        self.pm1a_evt_blk as u64
            | (self.pm1a_cnt_blk as u64) << 16
            | (self.pm_tmr_blk as u64) << 32
            | (self.reset_reg as u64) << 48
    }

    /// Handoff `param1` of device code 9: `RESET_VALUE` (bits 0..8) and the S5
    /// `SLP_TYP` (bits 8..16); the rest is zero.
    pub const fn handoff_param1(&self) -> u64 {
        self.reset_value as u64 | (self.slp_typ_s5 as u64) << 8
    }
}

pub const ACPI_PM_DESCRIPTOR: AcpiPmDescriptor = AcpiPmDescriptor {
    smi_cmd: ACPI_SMI_CMD,
    pm1a_evt_blk: ACPI_PM1A_EVT_BLK,
    pm1_evt_len: ACPI_PM1_EVT_LEN,
    pm1a_cnt_blk: ACPI_PM1A_CNT_BLK,
    pm1_cnt_len: ACPI_PM1_CNT_LEN,
    pm_tmr_blk: ACPI_PM_TMR_BLK,
    pm_tmr_len: ACPI_PM_TMR_LEN,
    tmr_val_ext: false,
    reset_reg: ACPI_RESET_REG,
    reset_reg_width_bits: ACPI_RESET_REG_WIDTH_BITS,
    reset_value: ACPI_RESET_VALUE,
    timer_hz: ACPI_PM_TIMER_HZ,
    slp_typ_s5: ACPI_SLP_TYP_S5,
    base_port: ACPI_PM_BASE_PORT,
    port_bytes: ACPI_PM_PORT_BYTES,
};

// Every register lies inside the bound block, in order and without overlap.
const _: () = {
    let d = ACPI_PM_DESCRIPTOR;
    let end = d.base_port as u32 + d.port_bytes;
    assert!(d.pm1a_evt_blk == d.base_port);
    assert!(d.pm1a_evt_blk as u32 + d.pm1_evt_len as u32 <= d.pm1a_cnt_blk as u32);
    assert!(d.pm1a_cnt_blk as u32 + d.pm1_cnt_len as u32 <= d.pm_tmr_blk as u32);
    assert!(d.pm_tmr_blk as u32 + d.pm_tmr_len as u32 <= d.reset_reg as u32);
    assert!((d.reset_reg as u32) < end);
    // Stay out of the conf1 block 0xCF8..=0xCFF.
    assert!(end <= 0xCF8);
};

/// Why the machine stopped on the request of this device.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MachineStopReason {
    /// The guest wrote [`ACPI_RESET_VALUE`] to the reset register. The run loop stops
    /// with this reason; the owner then calls `LoadedFirmwareBoard::platform_reset` (S7-26).
    Reset,
    /// The guest entered S5 (`SLP_TYP = S5` with `SLP_EN`).
    Shutdown,
}

#[derive(Debug, Default)]
struct PmState {
    sts: u16,
    en: u16,
    /// Stored `PM1_CNT` bits (`BM_RLD`, `SLP_TYP`); `SCI_EN` is constant.
    cnt: u16,
    stop: Option<MachineStopReason>,
    /// Guest reads of the PM timer register seen by the device (any width).
    timer_reads: u64,
    /// Virtual time queued by those reads, not yet taken by the clock owner.
    timer_tick_pending_ns: u64,
}

/// Cloneable observer of the PM device, held by the board's run loop.
#[derive(Clone, Debug)]
pub struct AcpiPmHandle {
    state: Arc<Mutex<PmState>>,
}

impl AcpiPmHandle {
    /// Successful reads of the PM timer register (any width) since construction.
    pub fn timer_reads(&self) -> u64 {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .timer_reads
    }

    /// Whether a PM timer read has queued time that nobody has taken yet.
    pub(crate) fn timer_tick_pending(&self) -> bool {
        self.quantum_boundary_pending(false)
    }
    /// Observe the timer tick and, on paths that can publish it, the machine stop in one lock.
    pub(crate) fn quantum_boundary_pending(&self, check_stop: bool) -> bool {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.timer_tick_pending_ns != 0 || (check_stop && state.stop.is_some())
    }

    /// Takes the virtual time queued by PM timer reads (the port tick,
    /// ISSUE-067), leaving zero. The clock owner advances its clock by the
    /// returned amount; the device never moves the clock itself.
    pub(crate) fn take_timer_tick_ns(&self) -> u64 {
        std::mem::take(
            &mut self
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .timer_tick_pending_ns,
        )
    }

    /// The first stop the guest asked for, if any. It stays latched: only
    /// [`Self::platform_reset`] (S7-26) clears it.
    pub fn stop_request(&self) -> Option<MachineStopReason> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).stop
    }

    /// The PM block after a platform reset (S7-26): `PM1_STS`, `PM1_EN` and `PM1_CNT` go back
    /// to zero (`SCI_EN` is constant), the latched stop request and the PM timer time not yet
    /// taken by the clock owner are dropped. The PM timer itself is not reset: it reads the
    /// board clock, which keeps running across a reset, and `timer_reads` stays a count since
    /// construction.
    pub(crate) fn platform_reset(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        *state = PmState {
            timer_reads: state.timer_reads,
            ..PmState::default()
        };
    }
}

/// The PM block device. Offsets are relative to [`ACPI_PM_BASE_PORT`].
pub struct AcpiPm {
    state: Arc<Mutex<PmState>>,
    clock: VirtualClockHandle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Register {
    Sts,
    En,
    Cnt,
    Timer,
    Reset,
}

/// Registers as `(register, offset in the block, length)`.
const REGISTERS: [(Register, u64, u64); 5] = [
    (Register::Sts, 0x0, 2),
    (Register::En, 0x2, 2),
    (Register::Cnt, 0x4, 2),
    (Register::Timer, 0x8, 4),
    (Register::Reset, 0xC, 1),
];

impl AcpiPm {
    pub fn new_with_handle(clock: VirtualClockHandle) -> (Self, AcpiPmHandle) {
        let state = Arc::new(Mutex::new(PmState::default()));
        (
            Self {
                state: Arc::clone(&state),
                clock,
            },
            AcpiPmHandle { state },
        )
    }

    /// The PM timer counter: `floor(now_ns * 3.579545 MHz)`, 24 bits wide.
    fn timer_value(&self) -> u32 {
        let now_ns = u128::from(self.clock.snapshot().now_ns());
        let ticks = now_ns * u128::from(ACPI_PM_TIMER_HZ) / NANOS_PER_SECOND;
        (ticks as u32) & PM_TIMER_MASK
    }

    /// Checks space, width, alignment and register bounds without touching state.
    fn locate(access: DeviceAccess) -> Result<(Register, u64, u64), DeviceError> {
        if access.space != IoSpace::Pio {
            return Err(DeviceError::UnsupportedSpace);
        }
        let len = match access.width {
            AccessWidth::Qword => return Err(DeviceError::UnsupportedWidth),
            width => u64::from(width.bytes()),
        };
        if !access.offset.is_multiple_of(len) {
            return Err(DeviceError::Misaligned);
        }
        let (register, start, size) = REGISTERS
            .iter()
            .copied()
            .find(|&(_, start, size)| (start..start + size).contains(&access.offset))
            .ok_or(DeviceError::UnknownOffset)?;
        if access.offset + len > start + size {
            return Err(DeviceError::CrossesRegister);
        }
        Ok((register, access.offset - start, len))
    }
}

/// Bytes `[shift, shift + len)` of a register image as a value of the access width.
fn extract(image: u32, shift: u64, len: u64) -> u64 {
    let mask = if len >= 4 {
        u32::MAX
    } else {
        (1u32 << (len * 8)) - 1
    };
    u64::from((image >> (shift * 8)) & mask)
}

impl BusDevice for AcpiPm {
    fn read(&mut self, access: DeviceAccess) -> Result<u64, DeviceError> {
        let (register, shift, len) = Self::locate(access)?;
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let image = match register {
            Register::Sts => u32::from(state.sts),
            Register::En => u32::from(state.en),
            // `GBL_RLS` and `SLP_EN` are write-only and read as zero.
            Register::Cnt => u32::from(ACPI_PM1_CNT_SCI_EN | state.cnt),
            Register::Timer => {
                // The value is sampled before the tick; the tick is only queued.
                let mut state = state;
                state.timer_reads = state.timer_reads.saturating_add(1);
                state.timer_tick_pending_ns = state
                    .timer_tick_pending_ns
                    .saturating_add(ACPI_PM_TMR_READ_TICK_NS);
                drop(state);
                self.timer_value()
            }
            Register::Reset => return Err(DeviceError::WriteOnly),
        };
        Ok(extract(image, shift, len))
    }

    fn write(&mut self, access: DeviceAccess, value: u64) -> Result<(), DeviceError> {
        let (register, shift, len) = Self::locate(access)?;
        if register == Register::Timer {
            return Err(DeviceError::ReadOnly);
        }
        // Validation is complete. Every writable register is at most 16 bits:
        // the written bytes become `value` under `mask` in the register image.
        let byte_mask = (1u32 << (len * 8)) - 1;
        let mask = (byte_mask << (shift * 8)) as u16;
        let value = ((value as u32 & byte_mask) << (shift * 8)) as u16;
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        match register {
            // Write-one-to-clear; no event source ever sets a bit.
            Register::Sts => state.sts &= !(value & mask),
            Register::En => state.en = (state.en & !mask) | (value & mask & PM1_EN_MASK),
            Register::Cnt => {
                state.cnt = (state.cnt & !mask) | (value & mask & PM1_CNT_STORED);
                // `SCI_EN` is constant (no ACPI enable/disable through `SMI_CMD`)
                // and `GBL_RLS` has no BIOS to notify: both are ignored.
                if value & mask & ACPI_PM1_CNT_SLP_EN != 0 {
                    let slp_typ =
                        (state.cnt & ACPI_PM1_CNT_SLP_TYP_MASK) >> ACPI_PM1_CNT_SLP_TYP_SHIFT;
                    // Only S5 exists. Another type does not put the machine to
                    // sleep: the command is dropped and `SLP_EN` reads as zero.
                    if slp_typ == u16::from(ACPI_SLP_TYP_S5) && state.stop.is_none() {
                        state.stop = Some(MachineStopReason::Shutdown);
                    }
                }
            }
            // Rejected as read-only above.
            Register::Timer => {}
            Register::Reset => {
                // Any other value is not the reset command and is ignored.
                if value & mask == u16::from(ACPI_RESET_VALUE) && state.stop.is_none() {
                    state.stop = Some(MachineStopReason::Reset);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        BusAccessError, GuestPhysicalAddress, GuestPhysicalRange, MachineBus, MachineMemoryMap,
        MemoryRegionKind, MemoryRegionSpec, PortAddress, PortRange, PortRegionBinding, RamLimits,
    };

    const BYTE: AccessWidth = AccessWidth::Byte;
    const WORD: AccessWidth = AccessWidth::Word;
    const DWORD: AccessWidth = AccessWidth::Dword;

    fn device() -> (AcpiPm, AcpiPmHandle, VirtualClockHandle) {
        let clock = VirtualClockHandle::new(1_000_000_000).unwrap();
        let (device, handle) = AcpiPm::new_with_handle(clock.clone());
        (device, handle, clock)
    }

    fn acc(offset: u64, width: AccessWidth) -> DeviceAccess {
        DeviceAccess {
            space: IoSpace::Pio,
            offset,
            width,
        }
    }

    /// The device behind a real `PortRegionBinding` on a `MachineBus`.
    fn bus() -> (MachineBus, AcpiPmHandle, VirtualClockHandle) {
        let (device, handle, clock) = device();
        let map = MachineMemoryMap::new(
            36,
            vec![MemoryRegionSpec::new(
                GuestPhysicalRange::new(GuestPhysicalAddress::new(0), 1 << 20).unwrap(),
                MemoryRegionKind::Ram,
            )],
        )
        .unwrap();
        let bus = MachineBus::new(
            map,
            RamLimits::new(1 << 20),
            vec![Box::new(device)],
            Vec::new(),
            vec![PortRegionBinding {
                range: PortRange::new(PortAddress::new(ACPI_PM_BASE_PORT), ACPI_PM_PORT_BYTES)
                    .unwrap(),
                device_index: 0,
            }],
        )
        .unwrap();
        (bus, handle, clock)
    }

    fn advance(clock: &VirtualClockHandle, ns: u64) {
        clock.advance_standalone(ns).unwrap();
    }

    #[test]
    fn sci_en_is_set_from_reset_and_cannot_be_cleared() {
        let (mut pm, _, _) = device();
        assert_eq!(pm.read(acc(4, WORD)), Ok(0x0001));
        assert_eq!(pm.read(acc(4, BYTE)), Ok(0x01));
        assert_eq!(pm.read(acc(5, BYTE)), Ok(0x00));
        pm.write(acc(4, WORD), 0).unwrap();
        assert_eq!(
            pm.read(acc(4, WORD)),
            Ok(0x0001),
            "SCI_EN has no off switch"
        );
    }

    #[test]
    fn pm1_cnt_stores_slp_typ_and_bm_rld_but_gbl_rls_and_slp_en_read_zero() {
        let (mut pm, handle, _) = device();
        // SLP_TYP = 3 without SLP_EN, BM_RLD, GBL_RLS.
        pm.write(acc(4, WORD), (3u64 << 10) | 0b110).unwrap();
        assert_eq!(pm.read(acc(4, WORD)), Ok((3u64 << 10) | 0b011));
        assert_eq!(handle.stop_request(), None);
    }

    #[test]
    fn s5_with_slp_en_stops_with_shutdown() {
        let (mut pm, handle, _) = device();
        assert_eq!(handle.stop_request(), None);
        let command = u64::from(ACPI_PM1_CNT_SLP_EN | (5 << 10));
        pm.write(acc(4, WORD), command).unwrap();
        assert_eq!(handle.stop_request(), Some(MachineStopReason::Shutdown));
        // SLP_EN is write-only; SLP_TYP stays.
        assert_eq!(pm.read(acc(4, WORD)), Ok(1u64 | (5 << 10)));
    }

    #[test]
    fn s5_through_the_high_byte_alone_works_and_slp_typ_alone_does_nothing() {
        let (mut pm, handle, _) = device();
        pm.write(acc(5, BYTE), 5u64 << 2).unwrap();
        assert_eq!(handle.stop_request(), None, "SLP_TYP without SLP_EN");
        pm.write(acc(5, BYTE), (1u64 << 5) | (5 << 2)).unwrap();
        assert_eq!(handle.stop_request(), Some(MachineStopReason::Shutdown));
    }

    #[test]
    fn slp_en_with_another_type_does_not_stop_the_machine() {
        let (mut pm, handle, _) = device();
        for slp_typ in [0u16, 1, 2, 3, 4, 6, 7] {
            let command = u64::from(ACPI_PM1_CNT_SLP_EN | (slp_typ << 10));
            pm.write(acc(4, WORD), command).unwrap();
            assert_eq!(handle.stop_request(), None, "SLP_TYP {slp_typ}");
        }
    }

    #[test]
    fn reset_value_stops_with_reset_and_other_values_do_not() {
        let (mut pm, handle, _) = device();
        for value in [0u64, 1, 0x05, 0x07, 0xFF] {
            pm.write(acc(0xC, BYTE), value).unwrap();
            assert_eq!(handle.stop_request(), None, "value {value:#x}");
        }
        pm.write(acc(0xC, BYTE), u64::from(ACPI_RESET_VALUE))
            .unwrap();
        assert_eq!(handle.stop_request(), Some(MachineStopReason::Reset));
    }

    #[test]
    fn reset_register_is_write_only_and_one_byte_wide() {
        let (mut pm, handle, _) = device();
        assert_eq!(pm.read(acc(0xC, BYTE)), Err(DeviceError::WriteOnly));
        // A wider write would cross the register (or be misaligned): refused,
        // and the reset is not triggered.
        assert_eq!(
            pm.write(acc(0xC, WORD), u64::from(ACPI_RESET_VALUE)),
            Err(DeviceError::CrossesRegister)
        );
        assert_eq!(
            pm.write(acc(0xC, DWORD), u64::from(ACPI_RESET_VALUE)),
            Err(DeviceError::CrossesRegister)
        );
        assert_eq!(handle.stop_request(), None);
    }

    #[test]
    fn s5_and_reset_give_different_reasons_and_the_first_one_stays() {
        let (mut a, ha, _) = device();
        a.write(acc(4, WORD), u64::from(ACPI_PM1_CNT_SLP_EN | (5 << 10)))
            .unwrap();
        let (mut b, hb, _) = device();
        b.write(acc(0xC, BYTE), u64::from(ACPI_RESET_VALUE))
            .unwrap();
        assert_ne!(ha.stop_request(), hb.stop_request());
        // Once stopped, a later command of the other kind does not overwrite it.
        a.write(acc(0xC, BYTE), u64::from(ACPI_RESET_VALUE))
            .unwrap();
        assert_eq!(ha.stop_request(), Some(MachineStopReason::Shutdown));
        b.write(acc(4, WORD), u64::from(ACPI_PM1_CNT_SLP_EN | (5 << 10)))
            .unwrap();
        assert_eq!(hb.stop_request(), Some(MachineStopReason::Reset));
    }

    #[test]
    fn pm1_status_and_enable_registers() {
        let (mut pm, _, _) = device();
        assert_eq!(pm.read(acc(0, WORD)), Ok(0));
        assert_eq!(pm.read(acc(2, WORD)), Ok(0));
        // Enable stores only the defined bits.
        pm.write(acc(2, WORD), 0xFFFF).unwrap();
        assert_eq!(pm.read(acc(2, WORD)), Ok(u64::from(PM1_EN_MASK)));
        pm.write(acc(2, BYTE), 0).unwrap();
        assert_eq!(pm.read(acc(2, WORD)), Ok(u64::from(PM1_EN_MASK & 0xFF00)));
        // Status is write-one-to-clear and nothing sets it.
        pm.write(acc(0, WORD), 0xFFFF).unwrap();
        assert_eq!(pm.read(acc(0, WORD)), Ok(0));
    }

    #[test]
    fn pm1_status_clears_only_the_written_ones() {
        let (mut pm, _, _) = device();
        pm.state.lock().unwrap().sts = 0x8721;
        pm.write(acc(0, WORD), 0x0021).unwrap();
        assert_eq!(pm.read(acc(0, WORD)), Ok(0x8700));
        pm.write(acc(1, BYTE), 0x07).unwrap();
        assert_eq!(pm.read(acc(0, WORD)), Ok(0x8000));
    }

    #[test]
    fn timer_runs_at_3_579_545_hz_from_the_virtual_clock() {
        let (mut pm, _, clock) = device();
        assert_eq!(pm.read(acc(8, DWORD)), Ok(0));
        advance(&clock, 1_000_000_000);
        assert_eq!(pm.read(acc(8, DWORD)), Ok(3_579_545));
        // Monotonic, and two reads differ by the elapsed ticks.
        let first = pm.read(acc(8, DWORD)).unwrap();
        advance(&clock, 1_000_000);
        let second = pm.read(acc(8, DWORD)).unwrap();
        assert_eq!(
            second - first,
            3_579,
            "1 ms is 3579.545 ticks, floored from the total"
        );
        // Reading does not move time.
        assert_eq!(pm.read(acc(8, DWORD)), Ok(second));
    }

    #[test]
    fn timer_is_24_bits_wide_and_wraps() {
        let (mut pm, _, clock) = device();
        // 2^24 ticks take 4.687 s; 5 s is 17_897_725 ticks, which is above 2^24.
        advance(&clock, 5_000_000_000);
        assert_eq!(pm.read(acc(8, DWORD)), Ok(17_897_725 - (1 << 24)));
        assert!(pm.read(acc(8, DWORD)).unwrap() <= u64::from(PM_TIMER_MASK));
    }

    #[test]
    fn a_timer_read_only_queues_a_tick_and_never_moves_the_clock() {
        let (mut pm, handle, clock) = device();
        advance(&clock, 1_000_000_000);
        let before = clock.snapshot().now_ns();
        assert!(!handle.timer_tick_pending());
        // Other registers, failed accesses and writes queue nothing.
        pm.read(acc(4, WORD)).unwrap();
        assert!(pm.read(acc(8, AccessWidth::Qword)).is_err());
        assert!(pm.write(acc(8, DWORD), 1).is_err());
        assert!(!handle.timer_tick_pending());
        // Reads of any width queue 1 us each, and the value is sampled before the tick.
        assert_eq!(pm.read(acc(8, DWORD)), Ok(3_579_545));
        pm.read(acc(9, BYTE)).unwrap();
        assert_eq!(handle.timer_reads(), 2);
        assert!(handle.timer_tick_pending());
        assert_eq!(
            clock.snapshot().now_ns(),
            before,
            "the device never advances"
        );
        assert_eq!(handle.take_timer_tick_ns(), 2 * ACPI_PM_TMR_READ_TICK_NS);
        assert!(!handle.timer_tick_pending());
        assert_eq!(handle.take_timer_tick_ns(), 0);
        assert_eq!(handle.timer_reads(), 2, "the read count is kept");
    }

    #[test]
    fn timer_is_read_only() {
        let (mut pm, _, _) = device();
        assert_eq!(pm.write(acc(8, DWORD), 1), Err(DeviceError::ReadOnly));
    }

    #[test]
    fn invalid_accesses_are_rejected_without_a_side_effect() {
        let (mut pm, handle, _) = device();
        let mmio = DeviceAccess {
            space: IoSpace::Mmio,
            offset: 4,
            width: WORD,
        };
        assert_eq!(pm.read(mmio), Err(DeviceError::UnsupportedSpace));
        assert_eq!(pm.write(mmio, 0), Err(DeviceError::UnsupportedSpace));
        assert_eq!(
            pm.read(acc(0, AccessWidth::Qword)),
            Err(DeviceError::UnsupportedWidth)
        );
        assert_eq!(pm.read(acc(1, WORD)), Err(DeviceError::Misaligned));
        // PM1_STS + PM1_EN is two registers: no 32-bit access at offset 0.
        assert_eq!(pm.read(acc(0, DWORD)), Err(DeviceError::CrossesRegister));
        assert_eq!(
            pm.write(acc(0, DWORD), 0),
            Err(DeviceError::CrossesRegister)
        );
        // Gaps of the block and its tail.
        for offset in [6u64, 7, 0xD, 0xE, 0xF, 0x10] {
            assert_eq!(
                pm.read(acc(offset, BYTE)),
                Err(DeviceError::UnknownOffset),
                "{offset:#x}"
            );
            assert_eq!(
                pm.write(acc(offset, BYTE), 0xFF),
                Err(DeviceError::UnknownOffset),
                "{offset:#x}"
            );
        }
        assert_eq!(handle.stop_request(), None);
    }

    #[test]
    fn registers_are_visible_through_the_machine_bus_ports() {
        let (mut bus, handle, clock) = bus();
        let port = |offset: u16| PortAddress::new(ACPI_PM_BASE_PORT + offset);
        assert_eq!(
            bus.read_port(PortAddress::new(ACPI_PM1A_CNT_BLK), WORD),
            Ok(1)
        );
        assert_eq!(bus.read_port(port(0), WORD), Ok(0));
        advance(&clock, 1_000_000_000);
        assert_eq!(
            bus.read_port(PortAddress::new(ACPI_PM_TMR_BLK), DWORD),
            Ok(3_579_545)
        );
        // A byte write of the reset register reaches the device through the bus.
        bus.write_port(
            PortAddress::new(ACPI_RESET_REG),
            BYTE,
            u32::from(ACPI_RESET_VALUE),
        )
        .unwrap();
        assert_eq!(handle.stop_request(), Some(MachineStopReason::Reset));
        // Device errors surface as bus errors.
        assert!(matches!(
            bus.read_port(PortAddress::new(ACPI_RESET_REG), BYTE),
            Err(BusAccessError::Device {
                source: DeviceError::WriteOnly,
                ..
            })
        ));
        assert!(matches!(
            bus.read_port(port(6), BYTE),
            Err(BusAccessError::Device {
                source: DeviceError::UnknownOffset,
                ..
            })
        ));
        // Past the block the bus knows nothing.
        assert_eq!(
            bus.read_port(port(0x10), BYTE),
            Err(BusAccessError::UnmappedPort(port(0x10)))
        );
    }

    #[test]
    fn pm1_registers_take_one_word_access_through_the_cpu_port_view() {
        // S4-38L9r: `66 ED` / `66 EF` reach the device as one access of width 2.
        use od_isa::PortIo as _;
        let (mut bus, _, _) = bus();
        let (_, mut port) = bus.split_for_pio_step();
        // PM1_EN (port 0x602) resets to zero and stores only the defined bits.
        assert_eq!(port.read_u16(0x602), Ok(0));
        port.write_u16(0x602, 0xFFFF).unwrap();
        assert_eq!(port.read_u16(0x602), Ok(PM1_EN_MASK));
        // The byte view returns the same data.
        assert_eq!(port.read_u8(0x602), Ok(PM1_EN_MASK as u8));
        assert_eq!(port.read_u8(0x603), Ok((PM1_EN_MASK >> 8) as u8));
        // PM1_STS is write-one-to-clear through the word write too.
        port.write_u16(0x600, 0xFFFF).unwrap();
        assert_eq!(port.read_u16(0x600), Ok(0));
        // PM1_CNT keeps SCI_EN set from reset.
        assert_eq!(port.read_u16(ACPI_PM1A_CNT_BLK), Ok(1));
        // Errors are typed bus errors and leave the stored value alone.
        assert!(matches!(
            port.write_u16(ACPI_PM_TMR_BLK, 0),
            Err(BusAccessError::Device {
                source: DeviceError::ReadOnly,
                ..
            })
        ));
        assert!(matches!(
            port.read_u16(ACPI_RESET_REG),
            Err(BusAccessError::Device {
                source: DeviceError::CrossesRegister | DeviceError::UnknownOffset,
                ..
            })
        ));
        assert_eq!(
            port.read_u16(ACPI_PM_BASE_PORT + 0x10),
            Err(BusAccessError::UnmappedPort(PortAddress::new(
                ACPI_PM_BASE_PORT + 0x10
            )))
        );
        assert_eq!(port.read_u16(0x602), Ok(PM1_EN_MASK));
    }

    #[test]
    fn pm_timer_takes_one_dword_access_through_both_cpu_port_views() {
        // S4-38L9u: `ED` (`IN EAX, DX`) at port 0x608 reaches the timer as one access of width 4.
        use od_isa::PortIo as _;
        let (mut bus, handle, clock) = bus();
        advance(&clock, 1_000_000_000);
        {
            let (_, mut port) = bus.split_for_pio_step();
            assert_eq!(port.read_u32(ACPI_PM_TMR_BLK), Ok(3_579_545));
            // The byte view returns the same data, little-endian.
            assert_eq!(port.read_u8(ACPI_PM_TMR_BLK), Ok(3_579_545u32 as u8));
            assert_eq!(
                port.read_u8(ACPI_PM_TMR_BLK + 2),
                Ok((3_579_545u32 >> 16) as u8)
            );
            // The timer is read-only; the typed bus error is kept and nothing is stored.
            assert!(matches!(
                port.write_u32(ACPI_PM_TMR_BLK, 0),
                Err(BusAccessError::Device {
                    source: DeviceError::ReadOnly,
                    ..
                })
            ));
            // Two registers in one dword (PM1_STS + PM1_EN) and the unmapped tail stay errors.
            assert!(matches!(
                port.read_u32(ACPI_PM_BASE_PORT),
                Err(BusAccessError::Device {
                    source: DeviceError::CrossesRegister,
                    ..
                })
            ));
            assert_eq!(
                port.read_u32(ACPI_PM_BASE_PORT + 0x10),
                Err(BusAccessError::UnmappedPort(PortAddress::new(
                    ACPI_PM_BASE_PORT + 0x10
                )))
            );
            assert_eq!(port.read_u32(ACPI_PM_TMR_BLK), Ok(3_579_545));
        }
        // The same read through the set-loop port view (one timer read, like the bus view).
        let reads = handle.timer_reads();
        let (_, io) = bus.split_for_set_step();
        let mut port = io.port_io();
        assert_eq!(port.read_u32(ACPI_PM_TMR_BLK), Ok(3_579_545));
        assert!(matches!(
            port.write_u32(ACPI_PM_TMR_BLK, 0),
            Err(BusAccessError::Device {
                source: DeviceError::ReadOnly,
                ..
            })
        ));
        assert_eq!(handle.timer_reads(), reads + 1);
    }

    #[test]
    fn s5_through_the_machine_bus_uses_the_cnt_port() {
        let (mut bus, handle, _) = bus();
        bus.write_port(
            PortAddress::new(ACPI_PM1A_CNT_BLK),
            WORD,
            u32::from(ACPI_PM1_CNT_SLP_EN | (u16::from(ACPI_SLP_TYP_S5) << 10)),
        )
        .unwrap();
        assert_eq!(handle.stop_request(), Some(MachineStopReason::Shutdown));
    }

    #[test]
    fn descriptor_blocks_are_ordered_inside_the_bound_range_and_avoid_conf1() {
        let d = ACPI_PM_DESCRIPTOR;
        let blocks = [
            (d.pm1a_evt_blk, u16::from(d.pm1_evt_len)),
            (d.pm1a_cnt_blk, u16::from(d.pm1_cnt_len)),
            (d.pm_tmr_blk, u16::from(d.pm_tmr_len)),
            (d.reset_reg, 1),
        ];
        let range = d.base_port..d.base_port + d.port_bytes as u16;
        for (index, &(start, len)) in blocks.iter().enumerate() {
            assert!(range.contains(&start) && range.contains(&(start + len - 1)));
            for &(other, other_len) in &blocks[index + 1..] {
                assert!(start + len <= other || other + other_len <= start);
            }
        }
        assert_eq!(d.smi_cmd, 0);
        assert!(!d.tmr_val_ext);
        assert_eq!(d.timer_hz, 3_579_545);
        // The registers of the device match the descriptor.
        let table: Vec<(u64, u64)> = REGISTERS.iter().map(|&(_, s, l)| (s, l)).collect();
        assert_eq!(
            table,
            vec![(0, 2), (2, 2), (4, 2), (8, 4), (0xC, 1)],
            "REGISTERS follow the descriptor offsets"
        );
        assert_eq!(u64::from(d.pm1a_cnt_blk - d.base_port), 4);
        assert_eq!(u64::from(d.pm_tmr_blk - d.base_port), 8);
        assert_eq!(u64::from(d.reset_reg - d.base_port), 0xC);
    }
}
