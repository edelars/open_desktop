//! Deterministic MC146818-compatible RTC/CMOS device (S7-14).
//!
//! The calendar starts at the explicit UTC Unix timestamp in handoff and advances only with
//! the board's shared virtual clock. No host wall clock is consulted. Periodic/alarm/update
//! interrupts are not routed in this slice; register C therefore never reports an interrupt.

use crate::{AccessWidth, BusDevice, DeviceAccess, DeviceError, IoSpace, VirtualClockHandle};

pub const CMOS_INDEX_PORT: u16 = 0x70;
pub const CMOS_DATA_PORT: u16 = 0x71;
pub const CMOS_PORT_BYTES: u32 = 2;
pub const CMOS_RTC_NOT_PRESENT_BIT: u32 = 5;
pub const RTC_BASE_UNIX_SECONDS_MAX: u64 = 253_402_300_799;

const NANOS_PER_SECOND: u64 = 1_000_000_000;
// Status A resets to 0x26 (32.768 kHz). Table 6 gives a 1.984 ms update cycle; UIP is
// asserted 244 μs before each virtual-second boundary, then remains high for that update cycle.
// Alternate divider/time-base programming is not modeled.
const UIP_ASSERT_LEAD_NS: u64 = 244_000;
const UPDATE_CYCLE_NS: u64 = 1_984_000;
const REG_SECONDS: u8 = 0x00;
const REG_MINUTES: u8 = 0x02;
const REG_HOURS: u8 = 0x04;
const REG_WEEKDAY: u8 = 0x06;
const REG_DAY: u8 = 0x07;
const REG_MONTH: u8 = 0x08;
const REG_YEAR: u8 = 0x09;
const REG_STATUS_A: u8 = 0x0A;
const REG_STATUS_B: u8 = 0x0B;
const REG_STATUS_C: u8 = 0x0C;
const REG_STATUS_D: u8 = 0x0D;
pub const RTC_CENTURY_REGISTER: u8 = 0x32;

const STATUS_A_DEFAULT: u8 = 0x26;
const STATUS_B_24_HOUR: u8 = 1 << 1;
const STATUS_B_BINARY: u8 = 1 << 2;
const STATUS_B_SET: u8 = 1 << 7;
const STATUS_B_UIE: u8 = 1 << 4;
const STATUS_D_VALID_RAM: u8 = 1 << 7;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RtcCmosError {
    BaseTimeOutOfRange(u64),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Calendar {
    year: u16,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: u8,
}

/// One CMOS/RTC pair at ports `0x70/0x71`.
#[derive(Debug)]
pub struct RtcCmos {
    clock: VirtualClockHandle,
    anchor_unix_seconds: u64,
    anchor_clock_ns: u64,
    selected_register: u8,
    nmi_disabled: bool,
    status_a: u8,
    status_b: u8,
    weekday_offset: i8,
    frozen: Option<Calendar>,
    cmos_ram: [u8; 128],
}

impl RtcCmos {
    pub fn new(clock: VirtualClockHandle, base_unix_seconds: u64) -> Result<Self, RtcCmosError> {
        if base_unix_seconds > RTC_BASE_UNIX_SECONDS_MAX {
            return Err(RtcCmosError::BaseTimeOutOfRange(base_unix_seconds));
        }
        let anchor_clock_ns = clock.snapshot().now_ns();
        Ok(Self {
            clock,
            anchor_unix_seconds: base_unix_seconds,
            anchor_clock_ns,
            selected_register: 0,
            nmi_disabled: false,
            status_a: STATUS_A_DEFAULT,
            // BCD and 24-hour mode are the MC146818-compatible reset defaults.
            status_b: STATUS_B_24_HOUR,
            weekday_offset: 0,
            frozen: None,
            cmos_ram: [0; 128],
        })
    }

    fn now_ns(&self) -> u64 {
        self.clock.snapshot().now_ns()
    }

    fn current_unix_seconds_at(&self, now_ns: u64) -> u64 {
        let elapsed_seconds = now_ns.saturating_sub(self.anchor_clock_ns) / NANOS_PER_SECOND;
        self.anchor_unix_seconds
            .saturating_add(elapsed_seconds)
            .min(RTC_BASE_UNIX_SECONDS_MAX)
    }

    fn current_calendar(&self) -> Calendar {
        if self.status_b & STATUS_B_SET != 0 {
            if let Some(frozen) = self.frozen {
                return frozen;
            }
        }
        unix_seconds_to_calendar(self.current_unix_seconds_at(self.now_ns()))
    }

    fn write_index(&mut self, value: u8) {
        self.nmi_disabled = value & 0x80 != 0;
        self.selected_register = value & 0x7F;
    }

    fn read_selected(&mut self) -> u8 {
        let now_ns = self.now_ns();
        match self.selected_register {
            REG_SECONDS | REG_MINUTES | REG_HOURS | REG_WEEKDAY | REG_DAY | REG_MONTH
            | REG_YEAR | RTC_CENTURY_REGISTER => {
                let calendar = if self.status_b & STATUS_B_SET != 0 {
                    self.frozen.unwrap_or_else(|| {
                        unix_seconds_to_calendar(self.current_unix_seconds_at(now_ns))
                    })
                } else {
                    unix_seconds_to_calendar(self.current_unix_seconds_at(now_ns))
                };
                self.encode_calendar_register(self.selected_register, calendar)
            }
            REG_STATUS_A => {
                let elapsed = now_ns.saturating_sub(self.anchor_clock_ns);
                let phase = elapsed % NANOS_PER_SECOND;
                let before_boundary = phase >= NANOS_PER_SECOND - UIP_ASSERT_LEAD_NS;
                let after_boundary = elapsed >= NANOS_PER_SECOND && phase < UPDATE_CYCLE_NS;
                (self.status_a & 0x7F)
                    | if self.status_b & STATUS_B_SET == 0 && (before_boundary || after_boundary) {
                        0x80
                    } else {
                        0
                    }
            }
            REG_STATUS_B => self.status_b,
            REG_STATUS_C => 0,
            REG_STATUS_D => STATUS_D_VALID_RAM,
            register => self.cmos_ram[usize::from(register)],
        }
    }

    fn encode_calendar_register(&self, register: u8, calendar: Calendar) -> u8 {
        let binary = self.status_b & STATUS_B_BINARY != 0;
        let value = match register {
            REG_SECONDS => calendar.second,
            REG_MINUTES => calendar.minute,
            REG_HOURS if self.status_b & STATUS_B_24_HOUR == 0 => {
                let pm = calendar.hour >= 12;
                let hour = match calendar.hour % 12 {
                    0 => 12,
                    hour => hour,
                };
                encode_number(hour, binary) | if pm { 0x80 } else { 0 }
            }
            REG_HOURS => calendar.hour,
            REG_WEEKDAY => shifted_weekday(weekday(calendar), self.weekday_offset),
            REG_DAY => calendar.day,
            REG_MONTH => calendar.month,
            REG_YEAR => (calendar.year % 100) as u8,
            RTC_CENTURY_REGISTER => (calendar.year / 100) as u8,
            _ => return 0,
        };
        if register == REG_HOURS && self.status_b & STATUS_B_24_HOUR == 0 {
            value
        } else {
            encode_number(value, binary)
        }
    }

    fn write_selected(&mut self, value: u8) {
        let register = self.selected_register;
        match register {
            REG_STATUS_A => self.status_a = value & 0x7F,
            REG_STATUS_B => self.write_status_b(value),
            REG_STATUS_C | REG_STATUS_D => {}
            REG_SECONDS | REG_MINUTES | REG_HOURS | REG_WEEKDAY | REG_DAY | REG_MONTH
            | REG_YEAR | RTC_CENTURY_REGISTER => self.write_calendar_register(register, value),
            register => self.cmos_ram[usize::from(register)] = value,
        }
    }

    fn write_status_b(&mut self, value: u8) {
        let was_set = self.status_b & STATUS_B_SET != 0;
        let becomes_set = value & STATUS_B_SET != 0;
        if !was_set && becomes_set {
            self.frozen = Some(self.current_calendar());
        }
        if was_set && !becomes_set {
            if let Some(calendar) = self.frozen.take() {
                self.rebase(calendar, self.now_ns());
            }
        }
        self.status_b = if becomes_set {
            value & !STATUS_B_UIE
        } else {
            value
        };
    }

    fn write_calendar_register(&mut self, register: u8, value: u8) {
        let binary = self.status_b & STATUS_B_BINARY != 0;
        let decoded = if register == REG_HOURS && self.status_b & STATUS_B_24_HOUR == 0 {
            decode_number(value & 0x7F, binary)
        } else {
            decode_number(value, binary)
        };
        let Some(decoded) = decoded else { return };
        let mut calendar = self.current_calendar();
        match register {
            REG_SECONDS if decoded < 60 => calendar.second = decoded,
            REG_MINUTES if decoded < 60 => calendar.minute = decoded,
            REG_HOURS if self.status_b & STATUS_B_24_HOUR == 0 => {
                if (1..=12).contains(&decoded) {
                    let pm = value & 0x80 != 0;
                    calendar.hour = (decoded % 12) + if pm { 12 } else { 0 };
                } else {
                    return;
                }
            }
            REG_HOURS if decoded < 24 => calendar.hour = decoded,
            REG_WEEKDAY if (1..=7).contains(&decoded) => {
                let natural_weekday = weekday(calendar);
                self.weekday_offset = decoded as i8 - natural_weekday as i8;
            }
            REG_WEEKDAY => return,
            REG_DAY if (1..=days_in_month(calendar.year, calendar.month)).contains(&decoded) => {
                calendar.day = decoded;
            }
            REG_MONTH if (1..=12).contains(&decoded) => {
                calendar.month = decoded;
                if calendar.day > days_in_month(calendar.year, calendar.month) {
                    return;
                }
            }
            REG_YEAR if decoded < 100 => {
                calendar.year = (calendar.year / 100) * 100 + u16::from(decoded);
            }
            RTC_CENTURY_REGISTER if decoded <= 99 => {
                calendar.year = u16::from(decoded) * 100 + calendar.year % 100;
            }
            _ => return,
        }
        if !(1970..=9999).contains(&calendar.year) {
            return;
        }
        if self.status_b & STATUS_B_SET != 0 {
            self.frozen = Some(calendar);
        } else {
            self.rebase(calendar, self.now_ns());
        }
    }

    fn rebase(&mut self, calendar: Calendar, now_ns: u64) {
        if let Some(unix_seconds) = calendar_to_unix_seconds(calendar) {
            self.anchor_unix_seconds = unix_seconds;
            self.anchor_clock_ns = now_ns;
        }
    }
}

impl BusDevice for RtcCmos {
    fn read(&mut self, access: DeviceAccess) -> Result<u64, DeviceError> {
        check_access(access)?;
        Ok(u64::from(if access.offset == 0 {
            self.selected_register | if self.nmi_disabled { 0x80 } else { 0 }
        } else {
            self.read_selected()
        }))
    }

    fn write(&mut self, access: DeviceAccess, value: u64) -> Result<(), DeviceError> {
        check_access(access)?;
        let value = value as u8;
        if access.offset == 0 {
            self.write_index(value);
        } else {
            self.write_selected(value);
        }
        Ok(())
    }
}

fn check_access(access: DeviceAccess) -> Result<(), DeviceError> {
    if access.space != IoSpace::Pio {
        return Err(DeviceError::UnsupportedSpace);
    }
    if access.width != AccessWidth::Byte {
        return Err(DeviceError::UnsupportedWidth);
    }
    if access.offset >= u64::from(CMOS_PORT_BYTES) {
        return Err(DeviceError::UnknownOffset);
    }
    Ok(())
}

fn encode_number(value: u8, binary: bool) -> u8 {
    if binary {
        value
    } else {
        (value / 10) << 4 | (value % 10)
    }
}

fn decode_number(value: u8, binary: bool) -> Option<u8> {
    if binary {
        Some(value)
    } else {
        let high = value >> 4;
        let low = value & 0x0F;
        (high <= 9 && low <= 9).then_some(high * 10 + low)
    }
}

fn is_leap_year(year: u16) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn days_in_month(year: u16, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

fn days_before_year(year: u16) -> u64 {
    let y = u64::from(year);
    let leap_count_before = |year: u64| year / 4 - year / 100 + year / 400;
    365 * (y - 1970) + leap_count_before(y - 1) - leap_count_before(1969)
}

fn calendar_to_unix_seconds(calendar: Calendar) -> Option<u64> {
    if !(1970..=9999).contains(&calendar.year)
        || !(1..=12).contains(&calendar.month)
        || !(1..=days_in_month(calendar.year, calendar.month)).contains(&calendar.day)
        || calendar.hour > 23
        || calendar.minute > 59
        || calendar.second > 59
    {
        return None;
    }
    let mut days = days_before_year(calendar.year);
    for month in 1..calendar.month {
        days += u64::from(days_in_month(calendar.year, month));
    }
    days += u64::from(calendar.day - 1);
    let seconds = days * 86_400
        + u64::from(calendar.hour) * 3_600
        + u64::from(calendar.minute) * 60
        + u64::from(calendar.second);
    (seconds <= RTC_BASE_UNIX_SECONDS_MAX).then_some(seconds)
}

fn unix_seconds_to_calendar(unix_seconds: u64) -> Calendar {
    let unix_seconds = unix_seconds.min(RTC_BASE_UNIX_SECONDS_MAX);
    let mut days = unix_seconds / 86_400;
    let seconds_of_day = unix_seconds % 86_400;
    let mut low = 1970u16;
    let mut high = 10_000u16;
    while low + 1 < high {
        let mid = low + (high - low) / 2;
        if days_before_year(mid) <= days {
            low = mid;
        } else {
            high = mid;
        }
    }
    let year = low;
    days -= days_before_year(year);
    let mut month = 1u8;
    while days >= u64::from(days_in_month(year, month)) {
        days -= u64::from(days_in_month(year, month));
        month += 1;
    }
    Calendar {
        year,
        month,
        day: days as u8 + 1,
        hour: (seconds_of_day / 3_600) as u8,
        minute: ((seconds_of_day / 60) % 60) as u8,
        second: (seconds_of_day % 60) as u8,
    }
}

fn weekday(calendar: Calendar) -> u8 {
    let days = calendar_to_unix_seconds(Calendar {
        hour: 0,
        minute: 0,
        second: 0,
        ..calendar
    })
    .unwrap_or(0)
        / 86_400;
    ((days + 4) % 7 + 1) as u8
}

fn shifted_weekday(natural_weekday: u8, offset: i8) -> u8 {
    (i16::from(natural_weekday - 1) + i16::from(offset)).rem_euclid(7) as u8 + 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AccessWidth, DeviceAccess, IoSpace, VirtualClockHandle};

    const BYTE: AccessWidth = AccessWidth::Byte;
    const UNIX_2024_02_28_23_59_30: u64 = 1_709_164_770;

    fn access(offset: u64) -> DeviceAccess {
        DeviceAccess {
            space: IoSpace::Pio,
            offset,
            width: BYTE,
        }
    }

    fn select(rtc: &mut RtcCmos, index: u8) {
        rtc.write(access(0), u64::from(index)).unwrap();
    }

    fn read_reg(rtc: &mut RtcCmos, index: u8) -> u8 {
        select(rtc, index);
        rtc.read(access(1)).unwrap() as u8
    }

    fn write_reg(rtc: &mut RtcCmos, index: u8, value: u8) {
        select(rtc, index);
        rtc.write(access(1), u64::from(value)).unwrap();
    }

    #[test]
    fn default_registers_and_bcd_24_hour_calendar_are_visible() {
        let clock = VirtualClockHandle::new(1_000_000_000).unwrap();
        let mut rtc = RtcCmos::new(clock, 1_704_067_200).unwrap(); // 2024-01-01 00:00:00 UTC
        assert_eq!(read_reg(&mut rtc, REG_SECONDS), 0x00);
        assert_eq!(read_reg(&mut rtc, REG_MINUTES), 0x00);
        assert_eq!(read_reg(&mut rtc, REG_HOURS), 0x00);
        assert_eq!(read_reg(&mut rtc, REG_WEEKDAY), 0x02); // Monday, Sunday=1
        assert_eq!(read_reg(&mut rtc, REG_DAY), 0x01);
        assert_eq!(read_reg(&mut rtc, REG_MONTH), 0x01);
        assert_eq!(read_reg(&mut rtc, REG_YEAR), 0x24);
        assert_eq!(read_reg(&mut rtc, RTC_CENTURY_REGISTER), 0x20);
        assert_eq!(read_reg(&mut rtc, REG_STATUS_A), 0x26);
        assert_eq!(read_reg(&mut rtc, REG_STATUS_B), STATUS_B_24_HOUR);
        assert_eq!(read_reg(&mut rtc, REG_STATUS_C), 0);
        assert_eq!(read_reg(&mut rtc, REG_STATUS_D), 0x80);
    }

    #[test]
    fn clock_follows_virtual_time_with_exact_sixty_one_second_delta() {
        let clock = VirtualClockHandle::new(1_000_000_000).unwrap();
        let mut rtc = RtcCmos::new(clock.clone(), 1_704_067_200).unwrap();
        clock.advance_standalone(61 * NANOS_PER_SECOND).unwrap();
        assert_eq!(read_reg(&mut rtc, REG_MINUTES), 0x01);
        assert_eq!(read_reg(&mut rtc, REG_SECONDS), 0x01);
    }

    #[test]
    fn update_in_progress_brackets_boundary_with_datasheet_lead_and_cycle_times() {
        let clock = VirtualClockHandle::new(1_000_000_000).unwrap();
        let mut rtc = RtcCmos::new(clock.clone(), 1_704_067_200).unwrap();
        clock
            .advance_standalone(NANOS_PER_SECOND - UIP_ASSERT_LEAD_NS - 1)
            .unwrap();
        assert_eq!(read_reg(&mut rtc, REG_STATUS_A) & 0x80, 0);
        clock.advance_standalone(1).unwrap();
        assert_eq!(read_reg(&mut rtc, REG_STATUS_A) & 0x80, 0x80);
        clock.advance_standalone(UIP_ASSERT_LEAD_NS).unwrap();
        assert_eq!(read_reg(&mut rtc, REG_STATUS_A) & 0x80, 0x80);
        clock.advance_standalone(UPDATE_CYCLE_NS - 1).unwrap();
        assert_eq!(read_reg(&mut rtc, REG_STATUS_A) & 0x80, 0x80);
        clock.advance_standalone(1).unwrap();
        assert_eq!(read_reg(&mut rtc, REG_STATUS_A) & 0x80, 0);
    }

    #[test]
    fn leap_day_and_year_rollover_are_gregorian() {
        let clock = VirtualClockHandle::new(1_000_000_000).unwrap();
        let mut rtc = RtcCmos::new(clock.clone(), UNIX_2024_02_28_23_59_30).unwrap();
        clock.advance_standalone(30 * NANOS_PER_SECOND).unwrap();
        assert_eq!(read_reg(&mut rtc, REG_DAY), 0x29);
        assert_eq!(read_reg(&mut rtc, REG_MONTH), 0x02);
        clock.advance_standalone(86_400 * NANOS_PER_SECOND).unwrap();
        assert_eq!(read_reg(&mut rtc, REG_DAY), 0x01);
        assert_eq!(read_reg(&mut rtc, REG_MONTH), 0x03);
        let year_clock = VirtualClockHandle::new(1_000_000_000).unwrap();
        year_clock
            .advance_standalone(30 * NANOS_PER_SECOND)
            .unwrap();
        let mut year_end = RtcCmos::new(year_clock, 1_735_689_570).unwrap(); // 2024-12-31 23:59:30
        // This starts a fresh virtual-time domain; advance it across midnight.
        year_end
            .clock
            .advance_standalone(30 * NANOS_PER_SECOND)
            .unwrap();
        assert_eq!(read_reg(&mut year_end, REG_YEAR), 0x25);
    }

    #[test]
    fn binary_and_twelve_hour_modes_and_set_freeze_work() {
        let clock = VirtualClockHandle::new(1_000_000_000).unwrap();
        let mut rtc = RtcCmos::new(clock.clone(), 1_704_110_400).unwrap(); // 2024-01-01 12:00 UTC
        write_reg(&mut rtc, REG_STATUS_B, STATUS_B_24_HOUR | STATUS_B_BINARY);
        assert_eq!(read_reg(&mut rtc, REG_HOURS), 12);
        write_reg(&mut rtc, REG_STATUS_B, 0); // BCD, 12-hour mode
        assert_eq!(read_reg(&mut rtc, REG_HOURS), 0x92); // 12 PM
        write_reg(
            &mut rtc,
            REG_STATUS_B,
            STATUS_B_SET | STATUS_B_24_HOUR | STATUS_B_BINARY,
        );
        let frozen = read_reg(&mut rtc, REG_SECONDS);
        clock.advance_standalone(61 * NANOS_PER_SECOND).unwrap();
        assert_eq!(read_reg(&mut rtc, REG_SECONDS), frozen);
        write_reg(&mut rtc, REG_STATUS_B, STATUS_B_24_HOUR | STATUS_B_BINARY);
        assert_eq!(read_reg(&mut rtc, REG_SECONDS), frozen);
    }

    #[test]
    fn weekday_is_writable_and_advances_across_midnight() {
        let clock = VirtualClockHandle::new(1_000_000_000).unwrap();
        let mut rtc = RtcCmos::new(clock.clone(), 1_704_067_200).unwrap(); // Monday
        write_reg(&mut rtc, REG_WEEKDAY, 0x06); // Saturday, BCD
        assert_eq!(read_reg(&mut rtc, REG_WEEKDAY), 0x06);
        write_reg(&mut rtc, REG_WEEKDAY, 0x08); // out of range is ignored
        assert_eq!(read_reg(&mut rtc, REG_WEEKDAY), 0x06);
        clock.advance_standalone(86_400 * NANOS_PER_SECOND).unwrap();
        assert_eq!(read_reg(&mut rtc, REG_WEEKDAY), 0x07);
    }

    #[test]
    fn alarm_registers_and_cmos_ram_round_trip_through_indexed_data_port() {
        let clock = VirtualClockHandle::new(1_000_000_000).unwrap();
        let mut rtc = RtcCmos::new(clock, 1_704_067_200).unwrap();
        for (register, value) in [(0x01, 0x35), (0x03, 0x47), (0x05, 0x12)] {
            write_reg(&mut rtc, register, value);
            assert_eq!(read_reg(&mut rtc, register), value);
        }
        for (register, value) in [(0x0E, 0xA5), (0x20, 0x5A), (0x3F, 0xC3)] {
            write_reg(&mut rtc, register, value);
            assert_eq!(read_reg(&mut rtc, register), value);
        }
    }

    #[test]
    fn status_a_masks_uip_and_status_c_d_are_read_only() {
        let clock = VirtualClockHandle::new(1_000_000_000).unwrap();
        let mut rtc = RtcCmos::new(clock, 1_704_067_200).unwrap();
        write_reg(&mut rtc, REG_STATUS_A, 0xFF);
        assert_eq!(read_reg(&mut rtc, REG_STATUS_A), 0x7F);
        write_reg(&mut rtc, REG_STATUS_C, 0xFF);
        write_reg(&mut rtc, REG_STATUS_D, 0);
        assert_eq!(read_reg(&mut rtc, REG_STATUS_C), 0);
        assert_eq!(read_reg(&mut rtc, REG_STATUS_D), STATUS_D_VALID_RAM);
    }

    #[test]
    fn all_calendar_registers_are_writable_and_invalid_values_are_ignored() {
        let clock = VirtualClockHandle::new(1_000_000_000).unwrap();
        let mut rtc = RtcCmos::new(clock, 1_704_067_200).unwrap();
        write_reg(&mut rtc, REG_STATUS_B, STATUS_B_SET | STATUS_B_24_HOUR);
        write_reg(&mut rtc, REG_SECONDS, 0x42);
        write_reg(&mut rtc, REG_MINUTES, 0x45);
        write_reg(&mut rtc, REG_HOURS, 0x13);
        write_reg(&mut rtc, REG_DAY, 0x15);
        write_reg(&mut rtc, REG_MONTH, 0x02);
        write_reg(&mut rtc, REG_YEAR, 0x25);
        write_reg(&mut rtc, RTC_CENTURY_REGISTER, 0x21);
        assert_eq!(read_reg(&mut rtc, REG_SECONDS), 0x42);
        assert_eq!(read_reg(&mut rtc, REG_MINUTES), 0x45);
        assert_eq!(read_reg(&mut rtc, REG_HOURS), 0x13);
        assert_eq!(read_reg(&mut rtc, REG_DAY), 0x15);
        assert_eq!(read_reg(&mut rtc, REG_MONTH), 0x02);
        assert_eq!(read_reg(&mut rtc, REG_YEAR), 0x25);
        assert_eq!(read_reg(&mut rtc, RTC_CENTURY_REGISTER), 0x21);

        for (register, invalid) in [
            (REG_SECONDS, 0x60),
            (REG_MINUTES, 0x60),
            (REG_HOURS, 0x24),
            (REG_DAY, 0x00),
            (REG_MONTH, 0x13),
            (REG_YEAR, 0xFA),
            (RTC_CENTURY_REGISTER, 0xFA),
        ] {
            let before = read_reg(&mut rtc, register);
            write_reg(&mut rtc, register, invalid);
            assert_eq!(
                read_reg(&mut rtc, register),
                before,
                "register {register:#04x}"
            );
        }
    }

    #[test]
    fn setting_calendar_freeze_clears_update_ended_interrupt_enable() {
        let clock = VirtualClockHandle::new(1_000_000_000).unwrap();
        let mut rtc = RtcCmos::new(clock, 1_704_067_200).unwrap();
        write_reg(
            &mut rtc,
            REG_STATUS_B,
            STATUS_B_24_HOUR | STATUS_B_UIE | STATUS_B_SET,
        );
        assert_eq!(
            read_reg(&mut rtc, REG_STATUS_B),
            STATUS_B_24_HOUR | STATUS_B_SET
        );
    }

    #[test]
    fn index_preserves_nmi_disable_and_data_register_writes_rebase_time() {
        let clock = VirtualClockHandle::new(1_000_000_000).unwrap();
        let mut rtc = RtcCmos::new(clock.clone(), 1_704_067_200).unwrap();
        rtc.write(access(0), 0x8A).unwrap();
        assert_eq!(rtc.read(access(0)).unwrap(), 0x8A);
        assert_eq!(rtc.read(access(1)).unwrap(), 0x26);
        write_reg(&mut rtc, REG_SECONDS, 0x30);
        assert_eq!(read_reg(&mut rtc, REG_SECONDS), 0x30);
        clock.advance_standalone(NANOS_PER_SECOND).unwrap();
        assert_eq!(read_reg(&mut rtc, REG_SECONDS), 0x31);
    }

    #[test]
    fn rejects_wrong_width_space_offset_and_out_of_range_value() {
        let clock = VirtualClockHandle::new(1_000_000_000).unwrap();
        let mut rtc = RtcCmos::new(clock, 0).unwrap();
        assert_eq!(
            rtc.read(DeviceAccess {
                width: AccessWidth::Word,
                ..access(0)
            }),
            Err(DeviceError::UnsupportedWidth)
        );
        assert_eq!(
            rtc.read(DeviceAccess {
                space: IoSpace::Mmio,
                ..access(0)
            }),
            Err(DeviceError::UnsupportedSpace)
        );
        assert_eq!(rtc.read(access(2)), Err(DeviceError::UnknownOffset));
        assert_eq!(rtc.write(access(0), 0x100), Ok(()));
        assert!(matches!(
            RtcCmos::new(
                VirtualClockHandle::new(1).unwrap(),
                RTC_BASE_UNIX_SECONDS_MAX + 1
            ),
            Err(RtcCmosError::BaseTimeOutOfRange(_))
        ));
    }
}
