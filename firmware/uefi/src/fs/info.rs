//! Variable-length EFI_FILE_INFO and EFI_FILE_SYSTEM_INFO records.

use core::{
    ffi::c_void,
    mem::{offset_of, size_of},
    ptr,
};

use crate::{
    fat::{Entry, Times},
    uefi::{guid::Guid, runtime::Time, status::Status},
};

use super::simple_fs::Mounted;

pub const FILE_INFO_GUID: Guid = Guid {
    data1: 0x0957_6e92,
    data2: 0x6d3f,
    data3: 0x11d2,
    data4: [0x8e, 0x39, 0, 0xa0, 0xc9, 0x69, 0x72, 0x3b],
};

pub const FILE_SYSTEM_INFO_GUID: Guid = Guid {
    data1: 0x0957_6e93,
    data2: 0x6d3f,
    data3: 0x11d2,
    data4: [0x8e, 0x39, 0, 0xa0, 0xc9, 0x69, 0x72, 0x3b],
};

/// Fixed prefix of EFI_FILE_INFO. A terminated UTF-16 filename follows it.
#[repr(C)]
pub struct FileInfo {
    pub size: u64,
    pub file_size: u64,
    pub physical_size: u64,
    pub create_time: Time,
    pub last_access_time: Time,
    pub modification_time: Time,
    pub attribute: u64,
}

#[repr(C)]
pub struct FileSystemInfo {
    pub size: u64,
    pub read_only: u8,
    reserved: [u8; 7],
    pub volume_size: u64,
    pub free_space: u64,
    pub block_size: u32,
    pub volume_label: [u16; 1],
}

fn required(prefix: usize, name: &[u16]) -> usize {
    prefix + (name.len() + 1) * 2
}

pub(super) fn file_info_size(entry: &Entry) -> usize {
    required(size_of::<FileInfo>(), entry.name.as_slice())
}

/// Copy the exact-size wire record to the caller's possibly unaligned buffer.
pub(super) unsafe fn file_info(entry: &Entry, physical: u64, buffer: *mut c_void) {
    let times = entry.times;
    let record = FileInfo {
        size: file_info_size(entry) as u64,
        file_size: u64::from(entry.len),
        physical_size: physical,
        create_time: creation_time(times),
        last_access_time: time(times.accessed_date, 0, 0),
        modification_time: time(times.modified_date, times.modified_time, 0),
        attribute: u64::from(entry.attributes & 0x37),
    };
    unsafe {
        buffer.cast::<FileInfo>().write_unaligned(record);
        write_name(
            buffer.cast::<u8>().add(size_of::<FileInfo>()),
            entry.name.as_slice(),
        );
    }
}

pub(super) unsafe fn file_system_info(
    mounted: &Mounted,
    buffer_size: *mut usize,
    buffer: *mut c_void,
) -> Result<(), Status> {
    const NAME_AT: usize = offset_of!(FileSystemInfo, volume_label);
    let mut disk = mounted.disk;
    let label = mounted.volume.volume_label(&mut disk)?;
    let name = label.as_slice();
    let needed = required(NAME_AT, name);
    unsafe { check_buffer(buffer_size, buffer, needed) }?;
    let free = mounted.volume.free_bytes(&mut disk)?;
    let at = buffer.cast::<FileSystemInfo>();
    unsafe {
        ptr::addr_of_mut!((*at).size).write_unaligned(needed as u64);
        ptr::addr_of_mut!((*at).read_only).write(1);
        ptr::addr_of_mut!((*at).reserved).write_unaligned([0; 7]);
        ptr::addr_of_mut!((*at).volume_size).write_unaligned(mounted.volume.bytes());
        ptr::addr_of_mut!((*at).free_space).write_unaligned(free);
        ptr::addr_of_mut!((*at).block_size).write_unaligned(mounted.volume.block_size());
        write_name(buffer.cast::<u8>().add(NAME_AT), name);
        buffer_size.write(needed);
    }
    Ok(())
}

pub(super) unsafe fn check_buffer(
    size: *mut usize,
    buffer: *mut c_void,
    needed: usize,
) -> Result<(), Status> {
    if size.is_null() {
        return Err(Status::INVALID_PARAMETER);
    }
    if unsafe { size.read() } < needed {
        unsafe { size.write(needed) };
        return Err(Status::BUFFER_TOO_SMALL);
    }
    if buffer.is_null() || buffer.addr().checked_add(needed).is_none() {
        return Err(Status::INVALID_PARAMETER);
    }
    Ok(())
}

unsafe fn write_name(out: *mut u8, name: &[u16]) {
    let out = out.cast::<u16>();
    for (index, &unit) in name.iter().enumerate() {
        unsafe { out.add(index).write_unaligned(unit) };
    }
    unsafe { out.add(name.len()).write_unaligned(0) };
}

fn creation_time(times: Times) -> Time {
    time(
        times.created_date,
        times.created_time,
        times.created_centiseconds,
    )
}

fn time(date: u16, clock: u16, centiseconds: u8) -> Time {
    let year = 1980 + (date >> 9);
    let month = ((date >> 5) & 15) as u8;
    let day = (date & 31) as u8;
    let max_day = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)) => {
            29
        }
        2 => 28,
        _ => 0,
    };
    let hour = (clock >> 11) as u8;
    let minute = ((clock >> 5) & 63) as u8;
    let fraction = centiseconds.min(199);
    let second = (clock & 31) as u8 * 2 + fraction / 100;
    if day == 0 || day > max_day || hour > 23 || minute > 59 || second > 59 {
        return Time {
            year: 0,
            month: 0,
            day: 0,
            hour: 0,
            minute: 0,
            second: 0,
            pad1: 0,
            nanosecond: 0,
            time_zone: 0,
            daylight: 0,
            pad2: 0,
        };
    }
    Time {
        year,
        month,
        day,
        hour,
        minute,
        second,
        pad1: 0,
        nanosecond: u32::from(fraction % 100) * 10_000_000,
        time_zone: 2047,
        daylight: 0,
        pad2: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::time;

    #[test]
    fn fat_timestamp_rejects_impossible_calendar_dates() {
        let february = |year: u16, day: u16| ((year - 1980) << 9) | (2 << 5) | day;
        let leap_day = time(february(2024, 29), 0, 0);
        assert_eq!((leap_day.year, leap_day.month, leap_day.day), (2024, 2, 29));
        assert_eq!(time(february(2024, 31), 0, 0).year, 0);
        assert_eq!(time(february(2023, 29), 0, 0).year, 0);
        assert_eq!(time(february(2100, 29), 0, 0).year, 0);
        assert_eq!(time(february(2000, 29), 0, 0).year, 2000);
    }
}
