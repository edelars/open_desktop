//! UEFI 2.11 §10.5 device path utilities and their boot-pool ownership.

use super::{
    DevicePathNode, END_ENTIRE, END_INSTANCE, END_TYPE, HEADER_SIZE, MAX_PATH_BYTES,
    complete_extent, first_instance_extent,
};
use crate::bs::{
    memory::allocate_pool,
    misc::{copy_mem, set_mem},
    protocol::install_protocol_interface,
};
use crate::mem::map::MemoryType;
use crate::uefi::{guid::Guid, status::Status};
use core::{ffi::c_void, ptr, slice};

pub const DEVICE_PATH_UTILITIES_PROTOCOL_GUID: Guid = Guid {
    data1: 0x0379_be4e,
    data2: 0xd706,
    data3: 0x437d,
    data4: [0xb0, 0x37, 0xed, 0xb8, 0x2f, 0xb7, 0x72, 0xa4],
};

#[repr(C)]
pub struct DevicePathUtilitiesProtocol {
    pub get_device_path_size: unsafe extern "efiapi" fn(*const DevicePathNode) -> usize,
    pub duplicate_device_path:
        unsafe extern "efiapi" fn(*const DevicePathNode) -> *mut DevicePathNode,
    pub append_device_path: unsafe extern "efiapi" fn(
        *const DevicePathNode,
        *const DevicePathNode,
    ) -> *mut DevicePathNode,
    pub append_device_node: unsafe extern "efiapi" fn(
        *const DevicePathNode,
        *const DevicePathNode,
    ) -> *mut DevicePathNode,
    pub append_device_path_instance: unsafe extern "efiapi" fn(
        *const DevicePathNode,
        *const DevicePathNode,
    ) -> *mut DevicePathNode,
    pub get_next_device_path_instance:
        unsafe extern "efiapi" fn(*mut *mut DevicePathNode, *mut usize) -> *mut DevicePathNode,
    pub is_device_path_multi_instance: unsafe extern "efiapi" fn(*const DevicePathNode) -> u8,
    pub create_device_node: unsafe extern "efiapi" fn(u8, u8, u16) -> *mut DevicePathNode,
}

static UTILITIES: DevicePathUtilitiesProtocol = DevicePathUtilitiesProtocol {
    get_device_path_size,
    duplicate_device_path,
    append_device_path,
    append_device_node,
    append_device_path_instance,
    get_next_device_path_instance,
    is_device_path_multi_instance,
    create_device_node,
};

const END_PATH_BYTES: [u8; HEADER_SIZE] = [END_TYPE, END_ENTIRE, HEADER_SIZE as u8, 0];
const END_INSTANCE_BYTES: [u8; HEADER_SIZE] = [END_TYPE, END_INSTANCE, HEADER_SIZE as u8, 0];

/// Publish the stateless utilities on their own handle.
///
/// # Safety
/// Call once on the BSP after the Boot Services allocator is initialized.
pub unsafe fn install() -> Status {
    let mut handle = ptr::null_mut();
    unsafe {
        install_protocol_interface(
            &mut handle,
            &DEVICE_PATH_UTILITIES_PROTOCOL_GUID,
            0,
            ptr::from_ref(&UTILITIES).cast_mut().cast(),
        )
    }
}

// For all protocol callbacks, a non-null path or node is backed by readable
// caller memory through its declared bytes. Pool results belong to the caller.
unsafe extern "efiapi" fn get_device_path_size(path: *const DevicePathNode) -> usize {
    unsafe { complete_extent(path) }.unwrap_or(0)
}

unsafe extern "efiapi" fn duplicate_device_path(
    path: *const DevicePathNode,
) -> *mut DevicePathNode {
    let Some(bytes) = (unsafe { path_bytes(path) }) else {
        return ptr::null_mut();
    };
    unsafe { make_path(&[bytes]) }
}

unsafe extern "efiapi" fn append_device_path(
    first: *const DevicePathNode,
    second: *const DevicePathNode,
) -> *mut DevicePathNode {
    let (Some(first), Some(second)) = (unsafe { path_or_end(first) }, unsafe {
        path_or_end(second)
    }) else {
        return ptr::null_mut();
    };
    unsafe { make_path(&[without_end(first), second]) }
}

unsafe extern "efiapi" fn append_device_node(
    path: *const DevicePathNode,
    node: *const DevicePathNode,
) -> *mut DevicePathNode {
    let Some(path) = (unsafe { path_or_end(path) }) else {
        return ptr::null_mut();
    };
    if node.is_null() {
        return unsafe { make_path(&[path]) };
    }
    let header = unsafe { node.read() };
    let length = usize::from(u16::from_le_bytes(header.length));
    if length < HEADER_SIZE {
        return ptr::null_mut();
    }
    if header.node_type == END_TYPE {
        if length != HEADER_SIZE {
            return ptr::null_mut();
        }
        return match header.subtype {
            END_ENTIRE => unsafe { make_path(&[path]) },
            END_INSTANCE => unsafe {
                make_path(&[without_end(path), &END_INSTANCE_BYTES, &END_PATH_BYTES])
            },
            _ => ptr::null_mut(),
        };
    }
    let bytes = unsafe { slice::from_raw_parts(node.cast(), length) };
    unsafe { make_path(&[without_end(path), bytes, &END_PATH_BYTES]) }
}

unsafe extern "efiapi" fn append_device_path_instance(
    path: *const DevicePathNode,
    instance: *const DevicePathNode,
) -> *mut DevicePathNode {
    let Some(bytes) = (unsafe { path_bytes(instance) }) else {
        return ptr::null_mut();
    };
    if unsafe { first_instance_extent(instance) } != Some(bytes.len() - HEADER_SIZE) {
        return ptr::null_mut();
    }
    if path.is_null() {
        return unsafe { make_path(&[bytes]) };
    }
    let Some(path) = (unsafe { path_bytes(path) }) else {
        return ptr::null_mut();
    };
    unsafe { make_path(&[without_end(path), &END_INSTANCE_BYTES, bytes]) }
}

unsafe extern "efiapi" fn get_next_device_path_instance(
    cursor: *mut *mut DevicePathNode,
    size: *mut usize,
) -> *mut DevicePathNode {
    let path = if cursor.is_null() {
        ptr::null_mut()
    } else {
        unsafe { cursor.read() }
    };
    if path.is_null() {
        if !size.is_null() {
            unsafe { size.write(0) };
        }
        return ptr::null_mut();
    }
    let Some(end_offset) = (unsafe { first_instance_extent(path) }) else {
        return ptr::null_mut();
    };
    let end = unsafe { path.byte_add(end_offset).read() };
    let payload = unsafe { slice::from_raw_parts(path.cast(), end_offset) };
    let result = unsafe { make_path(&[payload, &END_PATH_BYTES]) };
    if result.is_null() {
        return result;
    }
    let length = end_offset + HEADER_SIZE;
    unsafe {
        cursor.write(if end.subtype == END_INSTANCE {
            path.byte_add(length)
        } else {
            ptr::null_mut()
        });
        if !size.is_null() {
            size.write(length);
        }
    }
    result
}

unsafe extern "efiapi" fn is_device_path_multi_instance(path: *const DevicePathNode) -> u8 {
    let Some(length) = (unsafe { complete_extent(path) }) else {
        return 0;
    };
    u8::from(unsafe { first_instance_extent(path) } != Some(length - HEADER_SIZE))
}

unsafe extern "efiapi" fn create_device_node(
    node_type: u8,
    subtype: u8,
    length: u16,
) -> *mut DevicePathNode {
    let size = usize::from(length);
    if size < HEADER_SIZE {
        return ptr::null_mut();
    }
    let node = unsafe { allocate(size) };
    if !node.is_null() {
        unsafe {
            set_mem(node.cast(), size, 0);
            node.write(DevicePathNode {
                node_type,
                subtype,
                length: length.to_le_bytes(),
            });
        }
    }
    node
}

unsafe fn path_bytes<'a>(path: *const DevicePathNode) -> Option<&'a [u8]> {
    let length = unsafe { complete_extent(path) }?;
    Some(unsafe { slice::from_raw_parts(path.cast(), length) })
}

unsafe fn path_or_end<'a>(path: *const DevicePathNode) -> Option<&'a [u8]> {
    if path.is_null() {
        Some(&END_PATH_BYTES)
    } else {
        unsafe { path_bytes(path) }
    }
}

fn without_end(path: &[u8]) -> &[u8] {
    &path[..path.len() - HEADER_SIZE]
}

unsafe fn make_path(parts: &[&[u8]]) -> *mut DevicePathNode {
    let Some(size) = parts
        .iter()
        .try_fold(0usize, |size, part| size.checked_add(part.len()))
    else {
        return ptr::null_mut();
    };
    if size > MAX_PATH_BYTES {
        return ptr::null_mut();
    }
    let path = unsafe { allocate(size) };
    if !path.is_null() {
        let mut output = path.cast::<u8>();
        for part in parts {
            unsafe {
                copy_mem(output.cast(), part.as_ptr().cast(), part.len());
                output = output.add(part.len());
            }
        }
    }
    path
}

unsafe fn allocate(size: usize) -> *mut DevicePathNode {
    let mut output: *mut c_void = ptr::null_mut();
    if unsafe { allocate_pool(MemoryType::BOOT_SERVICES_DATA, size, &mut output) }
        == Status::SUCCESS
    {
        output.cast()
    } else {
        ptr::null_mut()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::{offset_of, size_of};

    #[test]
    fn utilities_protocol_matches_the_uefi_pointer_order() {
        type Protocol = DevicePathUtilitiesProtocol;
        let width = size_of::<usize>();
        assert_eq!(size_of::<Protocol>(), 8 * width);
        assert_eq!(offset_of!(Protocol, get_device_path_size), 0);
        assert_eq!(offset_of!(Protocol, duplicate_device_path), width);
        assert_eq!(offset_of!(Protocol, append_device_path), 2 * width);
        assert_eq!(offset_of!(Protocol, append_device_node), 3 * width);
        assert_eq!(offset_of!(Protocol, append_device_path_instance), 4 * width);
        assert_eq!(
            offset_of!(Protocol, get_next_device_path_instance),
            5 * width
        );
        assert_eq!(
            offset_of!(Protocol, is_device_path_multi_instance),
            6 * width
        );
        assert_eq!(offset_of!(Protocol, create_device_node), 7 * width);
    }
}
