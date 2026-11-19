//! BootNext, BootOrder, and fixed-disk ESP recovery for the firmware board.

use super::{
    char16, loader,
    options::{
        ACTIVE, LoadOption, encode_new, encoded_bytes, is_short_media_path, short_media_suffix,
    },
};
use crate::block::block_io::BLOCK_IO_PROTOCOL_GUID;
use crate::bs::memory::{BootPool, free_pool};
use crate::device_path::{DEVICE_PATH_PROTOCOL_GUID, END_PATH_BYTES, path_bytes, path_instances};
use crate::fs::simple_fs::SIMPLE_FILE_SYSTEM_PROTOCOL_GUID;
use crate::uefi::{
    status::Status,
    system_table::{BootServices, RuntimeServices, SystemTable},
    types::Handle,
};
use crate::var::{
    codec::MAX_VALUE_BYTES,
    services::{GLOBAL_VARIABLE, NON_VOLATILE_RUNTIME},
};
use core::{ptr, slice};

const BOOT_NEXT: [u16; 9] = char16(b"BootNext\0");
const BOOT_ORDER: [u16; 10] = char16(b"BootOrder\0");
const DESCRIPTION: [u16; 8] = char16(b"NVMe ESP");
const DEFAULT_NAME: &[u8] = b"\\EFI\\BOOT\\BOOTX64.EFI\0";
const DEFAULT_NODE_BYTES: usize = 4 + 2 * DEFAULT_NAME.len();
pub(super) const DEFAULT_NODE: [u8; DEFAULT_NODE_BYTES] = {
    let mut bytes = [0; DEFAULT_NODE_BYTES];
    bytes[0] = 4;
    bytes[1] = 4;
    bytes[2] = DEFAULT_NODE_BYTES as u8;
    let mut index = 0;
    while index < DEFAULT_NAME.len() {
        bytes[4 + 2 * index] = DEFAULT_NAME[index];
        index += 1;
    }
    bytes
};

/// # Safety
/// Call on the boot BSP with a live system table.
pub unsafe fn storage_present(system: *mut SystemTable) -> bool {
    let boot = unsafe { &*(*system).boot_services };
    let mut interface = ptr::null_mut();
    let status =
        unsafe { (boot.locate_protocol)(&BLOCK_IO_PROTOCOL_GUID, ptr::null_mut(), &mut interface) };
    status == Status::SUCCESS
}

/// Run normal headless boot policy. If all applications return, try remaining
/// candidates and report exhaustion rather than entering a boot menu.
///
/// # Safety
/// Call on the boot BSP with live boot and runtime service tables.
pub unsafe fn run(system: *mut SystemTable) -> Result<(), Status> {
    let boot = unsafe { &*(*system).boot_services };
    let runtime = unsafe { &*(*system).runtime_services };
    let mut order = unsafe { Order::read(runtime) }?;
    let mut scratch = unsafe { BootPool::allocate(MAX_VALUE_BYTES) }?;
    let handles = unsafe { EspHandles::locate(boot) }?;
    let devices = handles.as_slice();
    let mut attempted = unsafe { BootPool::allocate(devices.len().max(1)) }?;
    let attempted = attempted
        .zeroed_prefix(devices.len())
        .ok_or(Status::BAD_BUFFER_SIZE)?;

    let next = unsafe { take_boot_next(runtime) };
    for number in next.into_iter().chain(order.numbers()) {
        let Some(option) = (unsafe { read_option(runtime, number, &mut scratch) }) else {
            continue;
        };
        if option.is_active_boot()
            && let Some(loader::Started::Default(device)) =
                (unsafe { launch_configured(boot, runtime, devices, number, &option) })
            && let Some(index) = devices.iter().position(|&handle| handle == device)
        {
            attempted[index] = 1;
        }
    }

    for (&handle, tried) in devices.iter().zip(attempted.iter()) {
        if *tried != 0 {
            continue;
        }
        let Some(prefix) = (unsafe { device_prefix(boot, handle) }) else {
            continue;
        };
        let joined = unsafe { join_path(prefix, &DEFAULT_NODE) }?;
        let path = unsafe { slice::from_raw_parts(joined.as_ptr(), joined.len()) };
        // The execution order is a snapshot. Preserve any newer order written for
        // the next reset by an application that just returned.
        unsafe { order.refresh(runtime) }?;
        let Some(number) = (unsafe { allocate_number(runtime, &order, next) }) else {
            continue;
        };
        let name = boot_name(number);
        let Some(bytes) = encoded_bytes(DESCRIPTION.len(), path.len(), 0) else {
            continue;
        };
        let Some(output) = scratch.zeroed_prefix(bytes) else {
            continue;
        };
        let Some(size) = encode_new(ACTIVE, &DESCRIPTION, path, &[], output) else {
            continue;
        };
        let saved = unsafe { set_variable(runtime, &name, NON_VOLATILE_RUNTIME, &output[..size]) };
        if saved != Status::SUCCESS {
            continue;
        }
        if !unsafe { order.append(runtime, number) } {
            // Roll back before transferring control. A boot image need not return.
            let rolled_back = unsafe { set_variable(runtime, &name, 0, &[]) };
            if rolled_back != Status::SUCCESS {
                return Err(rolled_back);
            }
            continue;
        }
        unsafe { loader::launch(boot, runtime, number, path, &[]) };
    }
    Ok(())
}

unsafe fn launch_configured(
    boot: &BootServices,
    runtime: &RuntimeServices,
    devices: &[Handle],
    number: u16,
    option: &LoadOption<'_>,
) -> Option<loader::Started> {
    if !unsafe { path_instances(option.boot_path) }.any(is_short_media_path) {
        return unsafe {
            loader::launch(
                boot,
                runtime,
                number,
                option.boot_path,
                option.optional_data,
            )
        };
    }
    // Short forms are transient expansions. Stored Boot#### paths stay as written.
    for instance in unsafe { path_instances(option.boot_path) } {
        if !is_short_media_path(instance) {
            if let Some(started) = unsafe {
                launch_expanded(boot, runtime, number, &[], instance, option.optional_data)
            } {
                return Some(started);
            }
            continue;
        }
        for &device in devices {
            let Some(prefix) = (unsafe { device_prefix(boot, device) }) else {
                continue;
            };
            let Some(suffix) = short_media_suffix(instance, prefix) else {
                continue;
            };
            let suffix = if suffix.is_empty() {
                &DEFAULT_NODE
            } else {
                suffix
            };
            if let Some(started) = unsafe {
                launch_expanded(boot, runtime, number, prefix, suffix, option.optional_data)
            } {
                return Some(started);
            }
        }
    }
    None
}

unsafe fn launch_expanded(
    boot: &BootServices,
    runtime: &RuntimeServices,
    number: u16,
    prefix: &[u8],
    suffix: &[u8],
    options: &[u8],
) -> Option<loader::Started> {
    let path = unsafe { join_path(prefix, suffix) }.ok()?;
    let bytes = unsafe { slice::from_raw_parts(path.as_ptr(), path.len()) };
    unsafe { loader::launch(boot, runtime, number, bytes, options) }
}

fn boot_name(number: u16) -> [u16; 9] {
    let mut name = char16(b"Boot0000\0");
    let hex = b"0123456789ABCDEF";
    for (offset, unit) in name[4..8].iter_mut().enumerate() {
        let digit = usize::from((number >> ((3 - offset) * 4)) & 15);
        *unit = u16::from(hex[digit]);
    }
    name
}

pub(super) unsafe fn set_variable(
    runtime: &RuntimeServices,
    name: &[u16],
    attributes: u32,
    data: &[u8],
) -> Status {
    unsafe {
        (runtime.set_variable)(
            name.as_ptr(),
            &GLOBAL_VARIABLE,
            attributes,
            data.len(),
            data.as_ptr().cast(),
        )
    }
}

unsafe fn read_variable<'a>(
    runtime: &RuntimeServices,
    name: &[u16],
    buffer: &'a mut BootPool,
) -> Result<&'a [u8], Status> {
    let mut size = buffer.len();
    let status = unsafe {
        (runtime.get_variable)(
            name.as_ptr(),
            &GLOBAL_VARIABLE,
            ptr::null_mut(),
            &mut size,
            buffer.as_ptr().cast(),
        )
    };
    if status != Status::SUCCESS {
        return Err(status);
    }
    if size > buffer.len() {
        return Err(Status::DEVICE_ERROR);
    }
    Ok(unsafe { slice::from_raw_parts(buffer.as_ptr(), size) })
}

unsafe fn take_boot_next(runtime: &RuntimeServices) -> Option<u16> {
    let mut value = [0; 2];
    let mut size = value.len();
    let status = unsafe {
        (runtime.get_variable)(
            BOOT_NEXT.as_ptr(),
            &GLOBAL_VARIABLE,
            ptr::null_mut(),
            &mut size,
            value.as_mut_ptr().cast(),
        )
    };
    if !matches!(status, Status::SUCCESS | Status::BUFFER_TOO_SMALL) {
        return None;
    }
    let deleted = unsafe { set_variable(runtime, &BOOT_NEXT, 0, &[]) };
    (status == Status::SUCCESS && size == 2 && deleted == Status::SUCCESS)
        .then_some(u16::from_le_bytes(value))
}

unsafe fn read_option<'a>(
    runtime: &RuntimeServices,
    number: u16,
    buffer: &'a mut BootPool,
) -> Option<LoadOption<'a>> {
    let value = unsafe { read_variable(runtime, &boot_name(number), buffer) }.ok()?;
    LoadOption::parse(value)
}

unsafe fn allocate_number(
    runtime: &RuntimeServices,
    order: &Order,
    next: Option<u16>,
) -> Option<u16> {
    for number in 0..=u16::MAX {
        let mut size = 0;
        let status = unsafe {
            (runtime.get_variable)(
                boot_name(number).as_ptr(),
                &GLOBAL_VARIABLE,
                ptr::null_mut(),
                &mut size,
                ptr::null_mut(),
            )
        };
        match status {
            Status::NOT_FOUND => {
                if next != Some(number) && !order.numbers().any(|existing| existing == number) {
                    return Some(number);
                }
            }
            Status::SUCCESS | Status::BUFFER_TOO_SMALL => {}
            _ => return None,
        }
    }
    None
}

struct Order {
    data: BootPool,
    used: usize,
}

impl Order {
    unsafe fn read(runtime: &RuntimeServices) -> Result<Self, Status> {
        let data = unsafe { BootPool::allocate(MAX_VALUE_BYTES) }?;
        let mut order = Self { data, used: 0 };
        unsafe { order.refresh(runtime) }?;
        Ok(order)
    }

    unsafe fn refresh(&mut self, runtime: &RuntimeServices) -> Result<(), Status> {
        self.used = match unsafe { read_variable(runtime, &BOOT_ORDER, &mut self.data) } {
            Ok(bytes) if bytes.len().is_multiple_of(2) => bytes.len(),
            Ok(_) | Err(Status::NOT_FOUND | Status::BUFFER_TOO_SMALL) => 0,
            Err(error) => return Err(error),
        };
        Ok(())
    }

    fn numbers(&self) -> impl Iterator<Item = u16> + '_ {
        let bytes = unsafe { slice::from_raw_parts(self.data.as_ptr(), self.used) };
        bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
    }

    unsafe fn append(&mut self, runtime: &RuntimeServices, number: u16) -> bool {
        let end = self.used + 2;
        if end > self.data.len() {
            return false;
        }
        let bytes = number.to_le_bytes();
        let value = unsafe {
            ptr::copy_nonoverlapping(bytes.as_ptr(), self.data.as_ptr().add(self.used), 2);
            slice::from_raw_parts(self.data.as_ptr(), end)
        };
        if unsafe { set_variable(runtime, &BOOT_ORDER, NON_VOLATILE_RUNTIME, value) }
            != Status::SUCCESS
        {
            return false;
        }
        self.used = end;
        true
    }
}

struct EspHandles {
    pointer: *mut Handle,
    count: usize,
}

impl EspHandles {
    unsafe fn locate(boot: &BootServices) -> Result<Self, Status> {
        let mut result = Self {
            pointer: ptr::null_mut(),
            count: 0,
        };
        let status = unsafe {
            (boot.locate_handle_buffer)(
                2,
                &SIMPLE_FILE_SYSTEM_PROTOCOL_GUID,
                ptr::null_mut(),
                &mut result.count,
                &mut result.pointer,
            )
        };
        match status {
            Status::SUCCESS if result.count != 0 && !result.pointer.is_null() => Ok(result),
            Status::NOT_FOUND => Ok(result),
            Status::SUCCESS => Err(Status::DEVICE_ERROR),
            status => Err(status),
        }
    }

    fn as_slice(&self) -> &[Handle] {
        if self.count == 0 {
            &[]
        } else {
            unsafe { slice::from_raw_parts(self.pointer, self.count) }
        }
    }
}

impl Drop for EspHandles {
    fn drop(&mut self) {
        if !self.pointer.is_null() {
            unsafe { free_pool(self.pointer.cast()) };
        }
    }
}

unsafe fn device_prefix(boot: &BootServices, handle: Handle) -> Option<&[u8]> {
    let mut path = ptr::null_mut();
    let status = unsafe { (boot.handle_protocol)(handle, &DEVICE_PATH_PROTOCOL_GUID, &mut path) };
    if status != Status::SUCCESS {
        return None;
    }
    unsafe { path_bytes(path.cast()) }?.strip_suffix(&END_PATH_BYTES)
}

unsafe fn join_path(head: &[u8], suffix: &[u8]) -> Result<BootPool, Status> {
    let end = head
        .len()
        .checked_add(suffix.len())
        .ok_or(Status::BAD_BUFFER_SIZE)?;
    let size = end
        .checked_add(END_PATH_BYTES.len())
        .ok_or(Status::BAD_BUFFER_SIZE)?;
    let output = unsafe { BootPool::allocate(size) }?;
    unsafe {
        ptr::copy_nonoverlapping(head.as_ptr(), output.as_ptr(), head.len());
        ptr::copy_nonoverlapping(
            suffix.as_ptr(),
            output.as_ptr().add(head.len()),
            suffix.len(),
        );
        ptr::copy_nonoverlapping(
            END_PATH_BYTES.as_ptr(),
            output.as_ptr().add(end),
            END_PATH_BYTES.len(),
        );
    }
    Ok(output)
}
