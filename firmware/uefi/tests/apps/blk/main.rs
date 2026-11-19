#![no_std]
#![no_main]

use core::{ffi::c_void, ptr, slice};

use od_firmware::{
    block::{
        block_io::{BLOCK_IO_PROTOCOL_GUID, BlockIo},
        disk_io::{DISK_IO_PROTOCOL_GUID, DiskIo},
        storage::{PARTITION_INFO_PROTOCOL_GUID, PartitionInfo},
    },
    bs::event::{EVT_NOTIFY_SIGNAL, EVT_TIMER, Event, TIMER_RELATIVE, TPL_CALLBACK},
    device_path::DEVICE_PATH_PROTOCOL_GUID,
    uefi::{
        guid::Guid,
        status::Status,
        system_table::{BootServices, SystemTable},
        types::Handle,
    },
};
use od_firmware_test_apps::{ascii_to_char16, output};

const ESP_GUID: Guid = Guid {
    data1: 0xc12a_7328,
    data2: 0xf81f,
    data3: 0x11d2,
    data4: [0xba, 0x4b, 0, 0xa0, 0xc9, 0x3e, 0xc9, 0x3b],
};

#[unsafe(no_mangle)]
unsafe extern "efiapi" fn efi_main(handle: Handle, system: *mut SystemTable) -> Status {
    if handle.is_null() || system.is_null() {
        return Status::INVALID_PARAMETER;
    }
    if unsafe { verify(system) }.is_some() {
        Status::SUCCESS
    } else {
        Status::LOAD_ERROR
    }
}

fn ok(status: Status) -> Option<()> {
    (status == Status::SUCCESS).then_some(())
}

unsafe fn protocol<T>(boot: &BootServices, handle: Handle, guid: &Guid) -> Option<*mut T> {
    let mut interface = ptr::null_mut::<c_void>();
    ok(unsafe { (boot.handle_protocol)(handle, guid, &mut interface) })?;
    (!interface.is_null()).then_some(interface.cast())
}

unsafe fn verify(system: *mut SystemTable) -> Option<()> {
    const NO_GPT: &[u8] = b"OD-UEFI blk raw namespace with no valid GPT\r\n\0";
    const IO_OK: &[u8] = b"OD-UEFI blk I/O, reset and device paths verified\r\n\0";
    let boot = unsafe { (*system).boot_services.as_ref() }?;
    let mut handles = ptr::null_mut::<Handle>();
    let mut count = 0;
    ok(unsafe {
        (boot.locate_handle_buffer)(
            2,
            &BLOCK_IO_PROTOCOL_GUID,
            ptr::null_mut(),
            &mut count,
            &mut handles,
        )
    })?;
    let checked = (|| {
        if !(count == 1 || count == 3) || handles.is_null() {
            return None;
        }
        let mut verified = 0;
        for &handle in unsafe { slice::from_raw_parts(handles, count) } {
            let block = unsafe { protocol::<BlockIo>(boot, handle, &BLOCK_IO_PROTOCOL_GUID) }?;
            let disk = unsafe { protocol::<DiskIo>(boot, handle, &DISK_IO_PROTOCOL_GUID) }?;
            let path = unsafe { protocol::<u8>(boot, handle, &DEVICE_PATH_PROTOCOL_GUID) }?;
            let media = unsafe { (*block).media.as_ref() }?;
            if unsafe { (*block).revision } != 0x20001
                || unsafe { (*disk).revision } != 0x10000
                || media.block_size != 512
                || media.io_align != 1
                || media.media_present != 1
            {
                return None;
            }
            let identifier = media.media_id;
            let mut sector = [0; 512];
            ok(unsafe {
                ((*block).read_blocks)(block, identifier, 0, sector.len(), sector.as_mut_ptr())
            })?;
            if media.logical_partition == 0 {
                let raw_path = unsafe { slice::from_raw_parts(path, 44) };
                if media.last_block != 255
                    || sector[510..] != [0x55, 0xaa]
                    || raw_path[..8] != [1, 3, 24, 0, 11, 0, 0, 0]
                    || raw_path[8..16] != 0xfc00_1000u64.to_le_bytes()
                    || raw_path[16..24] != 0xfc00_2fffu64.to_le_bytes()
                    || raw_path[24..]
                        != [
                            3, 23, 16, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x7f, 0xff, 4, 0,
                        ]
                {
                    return None;
                }
                unsafe { verify_timer(boot, block, identifier, &mut sector) }?;
                verified |= 1;
                continue;
            }

            let info = unsafe {
                &*protocol::<PartitionInfo>(boot, handle, &PARTITION_INFO_PROTOCOL_GUID)?
            };
            let part = &info.partition;
            let path = unsafe { slice::from_raw_parts(path, 86) };
            if info.revision != 0x10000
                || info.partition_type != 2
                || path[40..44] != [4, 1, 42, 0]
                || path[80..] != [2, 2, 0x7f, 0xff, 4, 0]
                || path[48..56] != part.start.to_le_bytes()
                || path[56..64] != (media.last_block + 1).to_le_bytes()
                || path[64..68] != part.unique_id.data1.to_le_bytes()
                || part.end - part.start != media.last_block
            {
                return None;
            }
            let (flag, marker) = match path[44..48] {
                [1, 0, 0, 0] if info.system == 1 && part.start == 64 && media.last_block == 31 => {
                    (2, &b"OD ESP\0"[..])
                }
                [3, 0, 0, 0] if info.system == 0 && part.start == 128 && media.last_block == 47 => {
                    (4, &b"OD Data\0"[..])
                }
                _ => return None,
            };
            if verified & flag != 0
                || !sector.starts_with(marker)
                || !part
                    .name
                    .iter()
                    .zip(marker)
                    .all(|(&wide, &ascii)| wide == u16::from(ascii))
            {
                return None;
            }
            if info.system != 0 {
                unsafe { verify_esp_marker(boot, handle) }?;
            }
            verified |= flag;
            unsafe { verify_io(block, disk, identifier, media.last_block, &mut sector) }?;
            let mut line = [0u16; 39];
            let len = marker.len() - 1;
            line[..len].copy_from_slice(&part.name[..len]);
            line[len] = 13;
            line[len + 1] = 10;
            ok(unsafe { output(system, line.as_ptr()) })?;
        }
        match verified {
            1 if count == 1 => {
                let line = ascii_to_char16::<{ NO_GPT.len() }>(NO_GPT);
                ok(unsafe { output(system, line.as_ptr()) })
            }
            7 if count == 3 => {
                let line = ascii_to_char16::<{ IO_OK.len() }>(IO_OK);
                ok(unsafe { output(system, line.as_ptr()) })
            }
            _ => None,
        }
    })();
    ok(unsafe { (boot.free_pool)(handles.cast()) })?;
    checked
}

unsafe fn verify_esp_marker(boot: &BootServices, esp: Handle) -> Option<()> {
    let mut count = 0;
    let mut handles = ptr::null_mut::<Handle>();
    ok(unsafe {
        (boot.locate_handle_buffer)(2, &ESP_GUID, ptr::null_mut(), &mut count, &mut handles)
    })?;
    let exact = count == 1 && !handles.is_null() && unsafe { handles.read() } == esp;
    ok(unsafe { (boot.free_pool)(handles.cast()) })?;
    exact.then_some(())
}

unsafe fn verify_timer(
    boot: &BootServices,
    block: *mut BlockIo,
    media: u32,
    sector: &mut [u8; 512],
) -> Option<()> {
    let mut fired = 0u8;
    let flag = ptr::addr_of_mut!(fired);
    let mut event = ptr::null_mut();
    ok(unsafe {
        (boot.create_event)(
            EVT_TIMER | EVT_NOTIFY_SIGNAL,
            TPL_CALLBACK,
            Some(timer_fired),
            flag.cast(),
            &mut event,
        )
    })?;
    let mut observed = false;
    if unsafe { (boot.set_timer)(event, TIMER_RELATIVE, 100) } == Status::SUCCESS {
        for _ in 0..128 {
            if unsafe { ((*block).read_blocks)(block, media, 0, sector.len(), sector.as_mut_ptr()) }
                != Status::SUCCESS
            {
                break;
            }
            if unsafe { flag.read_volatile() } != 0 {
                observed = true;
                break;
            }
        }
    }
    ok(unsafe { (boot.close_event)(event) })?;
    observed.then_some(())
}

unsafe extern "efiapi" fn timer_fired(_: Event, context: *mut c_void) {
    unsafe { context.cast::<u8>().write_volatile(1) };
}

unsafe fn verify_io(
    block: *mut BlockIo,
    disk: *mut DiskIo,
    media: u32,
    last: u64,
    sector: &mut [u8; 512],
) -> Option<()> {
    let read = unsafe { (*block).read_blocks };
    let write = unsafe { (*block).write_blocks };
    let flush = unsafe { (*block).flush_blocks };
    let reset = unsafe { (*block).reset };
    let read_disk = unsafe { (*disk).read_disk };
    let write_disk = unsafe { (*disk).write_disk };
    if unsafe { read(block, media, 0, 511, sector.as_mut_ptr()) } != Status::BAD_BUFFER_SIZE
        || unsafe { read(block, media, last + 1, 512, sector.as_mut_ptr()) }
            != Status::INVALID_PARAMETER
        || unsafe { read(block, media + 1, 0, 512, sector.as_mut_ptr()) } != Status::MEDIA_CHANGED
    {
        return None;
    }
    let bytes = [0x9d, 0xf4, 0x81, 0x13, 0x67];
    ok(unsafe { write_disk(disk, media, 510, bytes.len(), bytes.as_ptr()) })?;
    let mut across = [0xff; 7];
    ok(unsafe { read_disk(disk, media, 509, across.len(), across.as_mut_ptr()) })?;
    if across != [0, 0x9d, 0xf4, 0x81, 0x13, 0x67, 0] {
        return None;
    }
    sector[0] = 0xa6;
    sector[511] = 0x75;
    ok(unsafe { write(block, media, 2, sector.len(), sector.as_ptr()) })?;
    ok(unsafe { flush(block) })?;
    ok(unsafe { reset(block, 1) })?;
    sector[0] = 0;
    sector[511] = 0;
    ok(unsafe { read(block, media, 2, sector.len(), sector.as_mut_ptr()) })?;
    (sector[0] == 0xa6 && sector[511] == 0x75).then_some(())
}
