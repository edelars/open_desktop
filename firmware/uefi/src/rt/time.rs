//! EFI time services backed by the board's indexed CMOS registers.

use crate::uefi::{
    runtime::{Time, TimeCapabilities},
    status::Status,
};

const SECONDS: u8 = 0x00;
pub const RTC_CENTURY_REGISTER: u8 = 0x32;
const MINUTES: u8 = 0x02;
const HOURS: u8 = 0x04;
const DAY: u8 = 0x07;
const MONTH: u8 = 0x08;
const YEAR: u8 = 0x09;
const STATUS_A: u8 = 0x0a;
const STATUS_B: u8 = 0x0b;
const STATUS_D: u8 = 0x0d;
const UPDATE_IN_PROGRESS: u8 = 1 << 7;
const SET: u8 = 1 << 7;
const BINARY: u8 = 1 << 2;
const HOUR_24: u8 = 1 << 1;
const VALID_RAM: u8 = 1 << 7;
const PM: u8 = 1 << 7;
const UNSPECIFIED_TIME_ZONE: i16 = 0x07ff;
const READ_ATTEMPTS: usize = 4;

pub trait Cmos {
    fn read(&mut self, register: u8) -> u8;
    fn write(&mut self, register: u8, value: u8);
}

pub struct Rtc {
    century_register: u8,
    time_zone: i16,
    daylight: u8,
}

impl Rtc {
    pub const fn new(century_register: u8) -> Self {
        Self {
            century_register,
            time_zone: UNSPECIFIED_TIME_ZONE,
            daylight: 0,
        }
    }

    pub const fn capabilities() -> TimeCapabilities {
        TimeCapabilities {
            resolution: 1,
            // The virtual oscillator advances at the board clock's exact rate.
            accuracy: 0,
            sets_to_zero: 1,
            _padding: [0; 3],
        }
    }

    pub fn get_time(&self, cmos: &mut impl Cmos) -> Result<Time, Status> {
        if cmos.read(STATUS_D) & VALID_RAM == 0 {
            return Err(Status::DEVICE_ERROR);
        }
        for _ in 0..READ_ATTEMPTS {
            if cmos.read(STATUS_A) & UPDATE_IN_PROGRESS != 0 {
                continue;
            }
            let mode = cmos.read(STATUS_B);
            let second = cmos.read(SECONDS);
            let minute = cmos.read(MINUTES);
            let hour = cmos.read(HOURS);
            let day = cmos.read(DAY);
            let month = cmos.read(MONTH);
            let year = cmos.read(YEAR);
            let century = cmos.read(self.century_register);
            if cmos.read(SECONDS) != second || cmos.read(STATUS_A) & UPDATE_IN_PROGRESS != 0 {
                continue;
            }
            let decode = |raw| decode_number(raw, mode).ok_or(Status::DEVICE_ERROR);
            let year = decode(year)?;
            let century = decode(century)?;
            if year > 99 || century > 99 {
                return Err(Status::DEVICE_ERROR);
            }
            let time = Time {
                year: u16::from(century) * 100 + u16::from(year),
                month: decode(month)?,
                day: decode(day)?,
                hour: decode_hour(hour, mode).ok_or(Status::DEVICE_ERROR)?,
                minute: decode(minute)?,
                second: decode(second)?,
                pad1: 0,
                nanosecond: 0,
                time_zone: self.time_zone,
                daylight: self.daylight,
                pad2: 0,
            };
            return if valid_time(&time) {
                Ok(time)
            } else {
                Err(Status::DEVICE_ERROR)
            };
        }
        Err(Status::DEVICE_ERROR)
    }

    pub fn set_time(&mut self, cmos: &mut impl Cmos, time: &Time) -> Status {
        if !valid_time(time) {
            return Status::INVALID_PARAMETER;
        }
        let mode = cmos.read(STATUS_B);
        if mode & SET != 0 || cmos.read(STATUS_D) & VALID_RAM == 0 {
            return Status::DEVICE_ERROR;
        }
        cmos.write(STATUS_B, mode | SET);
        let encode = |value| encode_number(value, mode);
        // Use a day valid in every month while changing the other date fields.
        cmos.write(DAY, encode(1));
        cmos.write(YEAR, encode((time.year % 100) as u8));
        cmos.write(self.century_register, encode((time.year / 100) as u8));
        cmos.write(MONTH, encode(time.month));
        cmos.write(DAY, encode(time.day));
        cmos.write(HOURS, encode_hour(time.hour, mode));
        cmos.write(MINUTES, encode(time.minute));
        cmos.write(SECONDS, encode(time.second));
        cmos.write(STATUS_B, mode);
        self.time_zone = time.time_zone;
        self.daylight = time.daylight;
        Status::SUCCESS
    }
}

fn encode_number(value: u8, mode: u8) -> u8 {
    if mode & BINARY != 0 {
        value
    } else {
        ((value / 10) << 4) | (value % 10)
    }
}

fn decode_number(value: u8, mode: u8) -> Option<u8> {
    if mode & BINARY != 0 {
        return Some(value);
    }
    let tens = value >> 4;
    let ones = value & 0x0f;
    (tens <= 9 && ones <= 9).then_some(tens * 10 + ones)
}

fn encode_hour(hour: u8, mode: u8) -> u8 {
    if mode & HOUR_24 != 0 {
        return encode_number(hour, mode);
    }
    let clock_hour = ((hour + 11) % 12) + 1;
    encode_number(clock_hour, mode) | if hour >= 12 { PM } else { 0 }
}

fn decode_hour(raw: u8, mode: u8) -> Option<u8> {
    if mode & HOUR_24 != 0 {
        return decode_number(raw, mode);
    }
    let clock_hour = decode_number(raw & !PM, mode)?;
    (1..=12)
        .contains(&clock_hour)
        .then_some(clock_hour % 12 + if raw & PM != 0 { 12 } else { 0 })
}

fn valid_time(time: &Time) -> bool {
    let leap = time.year % 4 == 0 && (time.year % 100 != 0 || time.year % 400 == 0);
    let last_day = match time.month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1900..=9999).contains(&time.year)
        && (1..=last_day).contains(&time.day)
        && time.hour < 24
        && time.minute < 60
        && time.second < 60
        && time.nanosecond < 1_000_000_000
        && ((-1440..=1440).contains(&time.time_zone) || time.time_zone == UNSPECIFIED_TIME_ZONE)
        && time.daylight & !0b11 == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Registers([u8; 128]);

    impl Cmos for Registers {
        fn read(&mut self, register: u8) -> u8 {
            self.0[usize::from(register)]
        }

        fn write(&mut self, register: u8, value: u8) {
            self.0[usize::from(register)] = value;
        }
    }

    const CENTURY: u8 = 0x32;
    const LEAP_DAY: Time = Time {
        year: 2000,
        month: 2,
        day: 29,
        hour: 12,
        minute: 34,
        second: 56,
        pad1: 0,
        nanosecond: 999_999_999,
        time_zone: -180,
        daylight: 3,
        pad2: 0,
    };

    #[test]
    fn sets_and_reads_leap_day_in_all_calendar_modes() {
        for (mode, expected_hour) in [(0, 0x92), (BINARY, 0x8c), (HOUR_24, 0x12), (6, 12)] {
            let mut rtc = Rtc::new(CENTURY);
            let mut cmos = Registers([0; 128]);
            cmos.write(STATUS_B, mode);
            cmos.write(STATUS_D, VALID_RAM);
            assert_eq!(rtc.set_time(&mut cmos, &LEAP_DAY), Status::SUCCESS);
            assert_eq!(cmos.read(STATUS_B), mode);
            assert_eq!(cmos.read(HOURS), expected_hour);
            let time = rtc.get_time(&mut cmos).unwrap();
            assert_eq!((time.year, time.month, time.day), (2000, 2, 29));
            assert_eq!((time.hour, time.minute, time.second), (12, 34, 56));
            assert_eq!(
                (time.time_zone, time.daylight, time.nanosecond),
                (-180, 3, 0)
            );
        }
    }

    #[test]
    fn rejects_invalid_time_before_writing_and_unstable_or_invalid_reads() {
        let mut rtc = Rtc::new(CENTURY);
        let mut cmos = Registers([0; 128]);
        cmos.write(STATUS_D, VALID_RAM);
        let invalid = Time {
            year: 1900,
            ..LEAP_DAY
        };
        let original = cmos.0;
        assert_eq!(rtc.set_time(&mut cmos, &invalid), Status::INVALID_PARAMETER);
        assert_eq!(cmos.0, original);
        assert_eq!(rtc.set_time(&mut cmos, &LEAP_DAY), Status::SUCCESS);
        cmos.write(STATUS_A, UPDATE_IN_PROGRESS);
        assert!(matches!(rtc.get_time(&mut cmos), Err(Status::DEVICE_ERROR)));
        cmos.write(STATUS_A, 0);
        cmos.write(MINUTES, 0x4a);
        assert!(matches!(rtc.get_time(&mut cmos), Err(Status::DEVICE_ERROR)));
    }
}
