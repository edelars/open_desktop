//! UEFI 2.11 §32.4 image execution information records.

use crate::{device_path::END_PATH_BYTES, uefi::status::Status};
use core::mem::MaybeUninit;
use od_uefi_security::{
    image_execution::{RejectedImage, RejectedImageAction},
    siglist::SignatureType,
};

const ENTRY_HEADER: usize = 8;
const SHA256_LIST_BYTES: usize = 28 + 16 + 32;

/// Serializes a record using the application name from the last File Path node.
fn write_entry(
    output: &mut [MaybeUninit<u8>],
    rejected: RejectedImage,
    device_path: Option<&[u8]>,
) -> Result<usize, Status> {
    let path = device_path.unwrap_or(&END_PATH_BYTES);
    let name = file_name(path);
    let digest = match rejected.action {
        RejectedImageAction::Untested | RejectedImageAction::PolicyFailed => None,
        _ => rejected.digest,
    };
    let signature_bytes = digest.map_or(0, |_| SHA256_LIST_BYTES);
    let size = ENTRY_HEADER + name.len() + 2 + path.len() + signature_bytes;
    let out = output.get_mut(..size).ok_or(Status::OUT_OF_RESOURCES)?;
    out[..4].write_copy_of_slice(&(rejected.action as u32).to_le_bytes());
    out[4..8].write_copy_of_slice(&(size as u32).to_le_bytes());
    let name_end = ENTRY_HEADER + name.len();
    out[ENTRY_HEADER..name_end].write_copy_of_slice(name);
    out[name_end..name_end + 2].write_copy_of_slice(&[0; 2]);
    let path_end = name_end + 2 + path.len();
    out[name_end + 2..path_end].write_copy_of_slice(path);
    if let Some(hash) = digest {
        let signature = &mut out[path_end..];
        signature[..16].write_copy_of_slice(&SignatureType::Sha256.guid_bytes());
        signature[16..20].write_copy_of_slice(&(SHA256_LIST_BYTES as u32).to_le_bytes());
        signature[20..24].write_copy_of_slice(&[0; 4]);
        signature[24..28].write_copy_of_slice(&48u32.to_le_bytes());
        signature[28..44].write_copy_of_slice(&[0; 16]);
        signature[44..].write_copy_of_slice(&hash);
    }
    Ok(size)
}

fn file_name(mut path: &[u8]) -> &[u8] {
    let mut found = &[][..];
    while let Some(header) = path.first_chunk::<4>() {
        let length = usize::from(u16::from_le_bytes([header[2], header[3]]));
        if !(4..=path.len()).contains(&length) {
            return &[];
        }
        let (node, next) = path.split_at(length);
        path = next;
        if header[..2] == [0x7f, 1] {
            return &[]; // The resolved instance cannot be inferred from the full path.
        }
        if header[0] == 4 && header[1] == 4 {
            let units = node[4..].chunks_exact(2);
            if !units.remainder().is_empty() {
                return &[];
            }
            let Some(end) = units.clone().position(|pair| pair == [0, 0]) else {
                return &[];
            };
            let start = units.take(end).enumerate().fold(0, |start, (index, pair)| {
                if pair == [b'\\', 0] { index + 1 } else { start }
            });
            found = &node[4 + 2 * start..4 + 2 * end];
        }
    }
    found
}

#[cfg(target_os = "none")]
mod runtime {
    use super::*;
    use crate::{
        bs::{config_table::install_configuration_table, memory},
        mem::map::MemoryType,
    };
    use core::{ptr, ptr::NonNull, slice};
    use od_uefi_security::protected::IMAGE_SECURITY_DATABASE;

    const TABLE_BYTES: usize = 256 * 1024;
    const TABLE_HEADER: usize = size_of::<usize>();

    struct Table {
        base: NonNull<u8>,
        used: usize,
        count: usize,
    }

    // The BSP owns the allocation and is the sole writer. The stable entry
    // pointer stays published through ExitBootServices.
    static mut TABLE: Option<Table> = None;

    /// # Safety
    /// Call once on the BSP with live Boot Services, before any guest image.
    pub unsafe fn install() -> Status {
        let mut allocation = ptr::null_mut();
        let status = unsafe {
            memory::allocate_pool(
                MemoryType::RUNTIME_SERVICES_DATA,
                TABLE_BYTES,
                &mut allocation,
            )
        };
        if status != Status::SUCCESS {
            return status;
        }
        let base = NonNull::new(allocation.cast::<u8>()).unwrap();
        unsafe { base.as_ptr().cast::<usize>().write(0) };
        let status = unsafe { install_configuration_table(&IMAGE_SECURITY_DATABASE, allocation) };
        if status != Status::SUCCESS {
            unsafe { memory::free_pool(allocation) };
            return status;
        }
        unsafe {
            ptr::addr_of_mut!(TABLE).write(Some(Table {
                base,
                used: TABLE_HEADER,
                count: 0,
            }))
        };
        Status::SUCCESS
    }

    /// # Safety
    /// Call on the BSP. The source path, if present, belongs to the image
    /// allocation and is disjoint from the table's private append area.
    pub(in super::super) unsafe fn append(
        rejected: RejectedImage,
        path: Option<&[u8]>,
    ) -> Result<(), Status> {
        let table = unsafe { &mut *ptr::addr_of_mut!(TABLE) }
            .as_mut()
            .ok_or(Status::NOT_READY)?;
        let out = unsafe {
            slice::from_raw_parts_mut(
                table
                    .base
                    .as_ptr()
                    .add(table.used)
                    .cast::<MaybeUninit<u8>>(),
                TABLE_BYTES - table.used,
            )
        };
        table.used += write_entry(out, rejected, path)?;
        table.count += 1;
        unsafe { table.base.as_ptr().cast::<usize>().write(table.count) };
        Ok(())
    }
}

#[cfg(target_os = "none")]
pub(super) use runtime::append;
#[cfg(target_os = "none")]
pub use runtime::install;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_report_exact_length_name_path_and_signature() {
        let mut storage = [MaybeUninit::new(0xa5); 256];
        let unsigned = RejectedImage {
            action: RejectedImageAction::Untested,
            digest: Some([0x11; 32]),
        };
        assert_eq!(write_entry(&mut storage, unsigned, None), Ok(14));
        let out = unsafe { storage.assume_init_ref() };
        assert_eq!(out[..14], [0, 0, 0, 0, 14, 0, 0, 0, 0, 0, 0x7f, 0xff, 4, 0]);
        assert_eq!(out[14], 0xa5);

        let path = [
            4, 4, 16, 0, b'\\', 0, b'D', 0, b'\\', 0, b'X', 0, b'Y', 0, 0, 0, 0x7f, 0xff, 4, 0,
        ];
        let denied = RejectedImage {
            action: RejectedImageAction::SignatureFound,
            digest: Some([0x22; 32]),
        };
        assert_eq!(write_entry(&mut storage, denied, Some(&path)), Ok(110));
        let out = unsafe { storage.assume_init_ref() };
        assert_eq!(
            out[..14],
            [4, 0, 0, 0, 110, 0, 0, 0, b'X', 0, b'Y', 0, 0, 0]
        );
        assert_eq!(out[14..34], path);
        assert_eq!(out[34..50], SignatureType::Sha256.guid_bytes());
        assert_eq!(out[50..62], [76, 0, 0, 0, 0, 0, 0, 0, 48, 0, 0, 0]);
        assert_eq!(out[62..78], [0; 16]);
        assert_eq!(out[78..110], [0x22; 32]);
        assert_eq!(out[110], 0xa5);

        let mut short = [MaybeUninit::new(0xbc); 109];
        assert_eq!(
            write_entry(&mut short, denied, Some(&path)),
            Err(Status::OUT_OF_RESOURCES)
        );
        assert_eq!(unsafe { short.assume_init_ref() }, &[0xbc; 109]);
    }
}
