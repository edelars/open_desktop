//! Device paths for image provenance and the firmware volume's Load File protocol.

use crate::bs::protocol::ProtocolRegistry;
use crate::device_path::{
    DEVICE_PATH_PROTOCOL_GUID, DevicePathNode, END_PATH_BYTES, HEADER_SIZE, path_instances,
};
#[cfg(any(target_os = "none", test))]
use crate::fv::MAX_NAME_SIZE;
use crate::fv::valid_name;
use crate::uefi::{guid::Guid, status::Status, types::Handle};
use core::{ffi::c_void, ptr};

#[cfg(target_os = "none")]
use super::filesystem;
#[cfg(target_os = "none")]
use crate::fs::simple_fs::{SIMPLE_FILE_SYSTEM_PROTOCOL_GUID, SimpleFileSystem};

pub const LOAD_FILE_PROTOCOL_GUID: Guid = Guid {
    data1: 0x56ec_3091,
    data2: 0x954c,
    data3: 0x11d2,
    data4: [0x8e, 0x3f, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b],
};

#[repr(C)]
pub struct LoadFileProtocol {
    pub load_file: unsafe extern "efiapi" fn(
        *mut Self,
        *const DevicePathNode,
        u8,
        *mut usize,
        *mut c_void,
    ) -> Status,
}

const MEDIA: u8 = 4;
const MEDIA_VENDOR: u8 = 3;
const MEDIA_FILE_PATH: u8 = 4;
const ROOT_BYTES: usize = HEADER_SIZE + 16;

// Media Vendor node with GUID 369ec96f-b0dd-4a8e-bbcb-a60a59915839,
// followed by End Entire. This identifies the Open Desktop built-in volume.
static VOLUME_PATH: [u8; ROOT_BYTES + HEADER_SIZE] = [
    MEDIA,
    MEDIA_VENDOR,
    ROOT_BYTES as u8,
    0,
    0x6f,
    0xc9,
    0x9e,
    0x36,
    0xdd,
    0xb0,
    0x8e,
    0x4a,
    0xbb,
    0xcb,
    0xa6,
    0x0a,
    0x59,
    0x91,
    0x58,
    0x39,
    0x7f,
    0xff,
    4,
    0,
];

/// Encode the complete firmware-volume path to an archive member.
pub fn firmware_file_device_path<'a>(name: &str, output: &'a mut [u8]) -> Option<&'a [u8]> {
    if !valid_name(name.as_bytes()) {
        return None;
    }
    let file_length = HEADER_SIZE + (name.len() + 2) * 2;
    let end = ROOT_BYTES + file_length;
    let output = output.get_mut(..end + HEADER_SIZE)?;
    output[..ROOT_BYTES].copy_from_slice(&VOLUME_PATH[..ROOT_BYTES]);
    let file = &mut output[ROOT_BYTES..end];
    file[..HEADER_SIZE].copy_from_slice(&[MEDIA, MEDIA_FILE_PATH, file_length as u8, 0]);
    file[HEADER_SIZE..].fill(0);
    file[HEADER_SIZE] = b'\\';
    for (index, &byte) in name.as_bytes().iter().enumerate() {
        file[HEADER_SIZE + 2 + index * 2] = byte;
    }
    output[end..].copy_from_slice(&END_PATH_BYTES);
    Some(output)
}

#[derive(Clone, Copy)]
pub(super) struct ImagePath<'a> {
    pub full: &'a [u8],
    pub device: Handle,
    // The matched instance's suffix excluding its terminator.
    pub relative: Option<&'a [u8]>,
}

impl<'a> ImagePath<'a> {
    pub(super) unsafe fn for_buffer(protocols: &ProtocolRegistry, full: &'a [u8]) -> Self {
        let found =
            unsafe { matching_instances(protocols, full, DEVICE_PATH_PROTOCOL_GUID) }.next();
        let (device, relative) = match found {
            Some((device, relative)) => (device, Some(relative)),
            None => (ptr::null_mut(), None),
        };
        Self {
            full,
            device,
            relative,
        }
    }

    pub(super) fn storage_len(&self) -> usize {
        self.full.len() + self.relative.map_or(0, |path| path.len() + HEADER_SIZE)
    }
}

/// The whole input passed validation. An installed candidate must back the
/// Device Path interface it registered, like LocateDevicePath.
unsafe fn matching_instances<'path, 'db>(
    protocols: &'db ProtocolRegistry,
    full: &'path [u8],
    protocol: Guid,
) -> impl Iterator<Item = (Handle, &'path [u8])> + use<'path, 'db> {
    unsafe { path_instances(full) }.filter_map(move |instance| {
        let (device, prefix) = unsafe { protocols.closest_device_path(protocol, instance) }?;
        Some((device, &instance[prefix..]))
    })
}

#[cfg(target_os = "none")]
static VOLUME_LOAD_FILE: LoadFileProtocol = LoadFileProtocol { load_file };

#[cfg(target_os = "none")]
/// Publish the firmware volume and its exact-name load provider.
///
/// # Safety
/// Call once on the BSP after initializing Boot Services.
pub unsafe fn install_firmware_volume() -> Status {
    use crate::bs::{protocol, protocol::multiple::ProtocolPair, state};

    let pairs = [
        ProtocolPair {
            protocol: DEVICE_PATH_PROTOCOL_GUID,
            interface: VOLUME_PATH.as_ptr().cast_mut().cast(),
        },
        ProtocolPair {
            protocol: LOAD_FILE_PROTOCOL_GUID,
            interface: ptr::from_ref(&VOLUME_LOAD_FILE).cast_mut().cast(),
        },
    ];
    let status = unsafe {
        state::with(|state| {
            if let Err(status) = state.protocols.install_bindings(ptr::null_mut(), &pairs) {
                return status;
            }
            state.protocols.signal_installations(state.events, |guid| {
                guid == DEVICE_PATH_PROTOCOL_GUID || guid == LOAD_FILE_PROTOCOL_GUID
            });
            Status::SUCCESS
        })
    };
    unsafe { protocol::dispatch_after_install(status) }
}

#[cfg(target_os = "none")]
pub(super) enum PathSource {
    FirmwareVolume(&'static [u8]),
    File {
        buffer: filesystem::FileBuffer,
        provider: ptr::NonNull<SimpleFileSystem>,
    },
}

#[cfg(target_os = "none")]
impl PathSource {
    pub(super) fn as_bytes(&self) -> &[u8] {
        match self {
            Self::FirmwareVolume(bytes) => bytes,
            Self::File { buffer, .. } => buffer.as_bytes(),
        }
    }

    pub(super) fn still_bound(&self, protocols: &ProtocolRegistry, device: Handle) -> bool {
        let (guid, expected) = match self {
            Self::FirmwareVolume(_) => (
                LOAD_FILE_PROTOCOL_GUID,
                ptr::from_ref(&VOLUME_LOAD_FILE).cast_mut().cast(),
            ),
            Self::File { provider, .. } => {
                (SIMPLE_FILE_SYSTEM_PROTOCOL_GUID, provider.as_ptr().cast())
            }
        };
        protocols.handle_protocol(device, guid) == Ok(expected)
    }
}

#[cfg(target_os = "none")]
enum Provider {
    FileSystem(ptr::NonNull<SimpleFileSystem>),
    FirmwareVolume,
}

#[cfg(target_os = "none")]
unsafe fn provider(
    protocols: &ProtocolRegistry,
    instance: &[u8],
) -> Result<Option<(Handle, usize, Provider)>, Status> {
    if let Some((device, prefix)) =
        unsafe { protocols.closest_device_path(SIMPLE_FILE_SYSTEM_PROTOCOL_GUID, instance) }
    {
        let interface = protocols.handle_protocol(device, SIMPLE_FILE_SYSTEM_PROTOCOL_GUID)?;
        let interface = ptr::NonNull::new(interface.cast()).ok_or(Status::DEVICE_ERROR)?;
        return Ok(Some((device, prefix, Provider::FileSystem(interface))));
    }
    let Some((device, prefix)) =
        (unsafe { protocols.closest_device_path(LOAD_FILE_PROTOCOL_GUID, instance) })
    else {
        return Ok(None);
    };
    let built_in = protocols.handle_protocol(device, LOAD_FILE_PROTOCOL_GUID)
        == Ok(ptr::from_ref(&VOLUME_LOAD_FILE).cast_mut().cast());
    Ok(built_in.then_some((device, prefix, Provider::FirmwareVolume)))
}

/// Fetch the path source without holding the Boot Services registry or allocator
/// guard across any file protocol call. Preserve the order of path instances.
#[cfg(target_os = "none")]
pub(super) unsafe fn path_source(full: &[u8]) -> Result<(PathSource, ImagePath<'_>), Status> {
    use crate::bs::state;

    for instance in unsafe { path_instances(full) } {
        let selected = unsafe { state::try_with(|state| provider(state.protocols, instance)) }
            .ok_or(Status::NOT_READY)??;
        let Some((device, prefix, selected)) = selected else {
            continue;
        };
        let relative = &instance[prefix..];
        let result = match selected {
            Provider::FirmwareVolume => archive_file(relative).map(PathSource::FirmwareVolume),
            Provider::FileSystem(provider) => unsafe { filesystem::read_file(provider, relative) }
                .map(|buffer| PathSource::File { buffer, provider }),
        };
        match result {
            Ok(source) => {
                return Ok((
                    source,
                    ImagePath {
                        full,
                        device,
                        relative: Some(relative),
                    },
                ));
            }
            Err(Status::NOT_FOUND | Status::NO_MEDIA | Status::MEDIA_CHANGED) => continue,
            Err(status) => return Err(status),
        }
    }
    Err(Status::NOT_FOUND)
}

#[cfg(target_os = "none")]
unsafe extern "efiapi" fn load_file(
    this: *mut LoadFileProtocol,
    relative: *const DevicePathNode,
    _boot_policy: u8,
    buffer_size: *mut usize,
    buffer: *mut c_void,
) -> Status {
    if !ptr::eq(this, &VOLUME_LOAD_FILE) || buffer_size.is_null() {
        return Status::INVALID_PARAMETER;
    }
    let Some(path) = (unsafe { crate::device_path::path_bytes(relative) }) else {
        return Status::INVALID_PARAMETER;
    };
    let leaf = &path[..path.len() - HEADER_SIZE];
    let bytes = match archive_file(leaf) {
        Ok(bytes) => bytes,
        Err(status) => return status,
    };
    unsafe {
        if buffer.is_null() || buffer_size.read() < bytes.len() {
            buffer_size.write(bytes.len());
            return Status::BUFFER_TOO_SMALL;
        }
        crate::bs::misc::copy_mem(buffer, bytes.as_ptr().cast(), bytes.len());
        buffer_size.write(bytes.len());
    }
    Status::SUCCESS
}

#[cfg(target_os = "none")]
fn archive_file(path: &[u8]) -> Result<&'static [u8], Status> {
    let mut name = [0; MAX_NAME_SIZE];
    let name = file_name(path, &mut name)?;
    let archive = crate::fv::embedded().map_err(|_| Status::DEVICE_ERROR)?;
    archive.get_bytes(name).ok_or(Status::NOT_FOUND)
}

#[cfg(any(target_os = "none", test))]
fn file_name<'a>(path: &[u8], buffer: &'a mut [u8; MAX_NAME_SIZE]) -> Result<&'a [u8], Status> {
    let Some(header) = path.first_chunk::<HEADER_SIZE>() else {
        return Err(Status::NOT_FOUND);
    };
    if header[..2] != [MEDIA, MEDIA_FILE_PATH] {
        return Err(Status::NOT_FOUND);
    }
    if usize::from(u16::from_le_bytes([header[2], header[3]])) != path.len() {
        return Err(Status::NOT_FOUND);
    }
    let name = path[HEADER_SIZE..]
        .strip_prefix(&[b'\\', 0])
        .and_then(|name| name.strip_suffix(&[0, 0]))
        .ok_or(Status::INVALID_PARAMETER)?;
    if name.len() % 2 != 0 {
        return Err(Status::INVALID_PARAMETER);
    }
    let decoded = buffer.get_mut(..name.len() / 2).ok_or(Status::NOT_FOUND)?;
    for (unit, byte) in name.chunks_exact(2).zip(decoded.iter_mut()) {
        if unit[0] == 0 && unit[1] == 0 {
            return Err(Status::INVALID_PARAMETER);
        }
        if unit[1] != 0 {
            return Err(Status::NOT_FOUND);
        }
        *byte = unit[0];
    }
    if !valid_name(decoded) {
        return Err(Status::NOT_FOUND);
    }
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_path_names_are_exact_and_fit_in_one_file_path_node() {
        let mut bytes = [0; ROOT_BYTES + 140];
        let mut name = [0; MAX_NAME_SIZE];
        let path = firmware_file_device_path("hello", &mut bytes).unwrap();
        assert_eq!(&path[..ROOT_BYTES], &VOLUME_PATH[..ROOT_BYTES]);
        assert_eq!(path.len(), 42);
        let leaf = &path[ROOT_BYTES..path.len() - HEADER_SIZE];
        assert_eq!(file_name(leaf, &mut name), Ok(b"hello".as_slice()));
        let long = "x".repeat(MAX_NAME_SIZE);
        let path = firmware_file_device_path(&long, &mut bytes).unwrap();
        let leaf = &path[ROOT_BYTES..path.len() - HEADER_SIZE];
        assert_eq!(file_name(leaf, &mut name), Ok(long.as_bytes()));
        assert!(firmware_file_device_path("..", &mut bytes).is_none());
        assert!(firmware_file_device_path("x/y", &mut bytes).is_none());
        assert!(firmware_file_device_path("HELLO", &mut bytes).is_none());
        assert!(firmware_file_device_path("hello", &mut bytes[..41]).is_none());
        assert_eq!(
            file_name(&[4, 4, 10, 0, b'\\', 0, b'x', 0, 0, 1], &mut name),
            Err(Status::INVALID_PARAMETER)
        );
    }
}
