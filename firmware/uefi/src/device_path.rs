//! Packed device path structure from UEFI 2.11 §10.

use crate::uefi::guid::Guid;
use core::slice;

pub mod utilities;

pub const DEVICE_PATH_PROTOCOL_GUID: Guid = Guid {
    data1: 0x0957_6e91,
    data2: 0x6d3f,
    data3: 0x11d2,
    data4: [0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b],
};

pub(crate) const HEADER_SIZE: usize = 4;
const END_TYPE: u8 = 0x7f;
const END_INSTANCE: u8 = 0x01;
const END_ENTIRE: u8 = 0xff;
const MAX_PATH_BYTES: usize = 64 * 1024;
pub(crate) const END_PATH_BYTES: [u8; HEADER_SIZE] = [END_TYPE, END_ENTIRE, HEADER_SIZE as u8, 0];

pub(crate) fn memory_mapped_node(base: u64, last: u64) -> [u8; 24] {
    let mut node = [0; 24];
    node[..4].copy_from_slice(&[1, 3, 24, 0]);
    node[4..8].copy_from_slice(
        &crate::mem::map::MemoryType::MEMORY_MAPPED_IO
            .raw()
            .to_le_bytes(),
    );
    node[8..16].copy_from_slice(&base.to_le_bytes());
    node[16..].copy_from_slice(&last.to_le_bytes());
    node
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum EndAt {
    FirstInstance,
    EntirePath,
}

/// The packed header at the beginning of every device path node.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct DevicePathNode {
    pub node_type: u8,
    pub subtype: u8,
    pub length: [u8; 2],
}

/// Returns the extent before the first End node, in bytes.
///
/// # Safety
/// A non-null path must provide readable, stable backing for the nodes it
/// describes through the first End node, subject to the profile's scan cap.
pub(crate) unsafe fn first_instance_extent(path: *const DevicePathNode) -> Option<usize> {
    unsafe { raw_extent(path, EndAt::FirstInstance) }
}

/// Returns the complete length, including the final End Entire node.
///
/// # Safety
/// A non-null path must provide readable, stable backing through End Entire,
/// subject to the profile's scan cap.
pub(crate) unsafe fn complete_extent(path: *const DevicePathNode) -> Option<usize> {
    unsafe { raw_extent(path, EndAt::EntirePath) }.map(|offset| offset + HEADER_SIZE)
}

/// Borrow the complete path including End Entire.
///
/// # Safety
/// For a non-null path, all described bytes must be readable and immutable
/// for the returned borrow, subject to the profile's scan cap.
pub(crate) unsafe fn path_bytes<'a>(path: *const DevicePathNode) -> Option<&'a [u8]> {
    let length = unsafe { complete_extent(path) }?;
    Some(unsafe { slice::from_raw_parts(path.cast(), length) })
}

unsafe fn raw_extent(path: *const DevicePathNode, end_at: EndAt) -> Option<usize> {
    if path.is_null() {
        return None;
    }
    extent(MAX_PATH_BYTES, end_at, |offset| {
        path.addr().checked_add(offset + HEADER_SIZE - 1)?;
        // SAFETY: the caller supplies a readable header at every node we visit.
        Some(unsafe { path.byte_add(offset).read() })
    })
}

fn extent(
    limit: usize,
    end_at: EndAt,
    mut read_header: impl FnMut(usize) -> Option<DevicePathNode>,
) -> Option<usize> {
    let mut offset = 0;
    loop {
        let remaining = limit - offset;
        if remaining < HEADER_SIZE {
            return None;
        }
        let node = read_header(offset)?;
        let length = usize::from(u16::from_le_bytes(node.length));
        if length < HEADER_SIZE || length > remaining {
            return None;
        }
        if node.node_type == END_TYPE {
            if length != HEADER_SIZE || !matches!(node.subtype, END_INSTANCE | END_ENTIRE) {
                return None;
            }
            if node.subtype == END_ENTIRE || end_at == EndAt::FirstInstance {
                return Some(offset);
            }
        }
        offset += length;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(bytes: &[u8]) -> Option<usize> {
        extent(
            bytes.len().min(MAX_PATH_BYTES),
            EndAt::FirstInstance,
            |offset| {
                let header = bytes.get(offset..offset + HEADER_SIZE)?;
                Some(DevicePathNode {
                    node_type: header[0],
                    subtype: header[1],
                    length: [header[2], header[3]],
                })
            },
        )
    }

    #[test]
    fn finds_the_first_end_on_a_node_boundary() {
        assert_eq!(size_of::<DevicePathNode>(), 4);
        assert_eq!(align_of::<DevicePathNode>(), 1);
        assert_eq!(parse(&[0x7f, 0xff, 4, 0]), Some(0));
        assert_eq!(
            parse(&[1, 1, 6, 0, 0, 0, 0x7f, 0x01, 4, 0, 0x7f, 0xff, 4, 0]),
            Some(6)
        );
        assert_eq!(parse(&[0x22, 1, 5, 0, 7, 0x7f, 0xff, 4, 0]), Some(5));
    }

    #[test]
    fn rejects_short_bad_or_missing_terminators() {
        for malformed in [
            &[1, 1, 0, 0][..],
            &[1, 1, 5, 0][..],
            &[1, 1, 4, 0][..],
            &[0x7f, 2, 4, 0][..],
            &[0x7f, 0xff, 5, 0, 0][..],
            &[0x7f, 0xff, 4][..],
        ] {
            assert_eq!(parse(malformed), None);
        }
    }

    #[test]
    fn terminator_must_fit_within_the_scan_cap() {
        let mut bytes = std::vec![0u8; MAX_PATH_BYTES + 1];
        let extent = MAX_PATH_BYTES - HEADER_SIZE;
        bytes[0] = 1;
        bytes[1] = 1;
        bytes[2..4].copy_from_slice(&(extent as u16).to_le_bytes());
        bytes[extent..extent + HEADER_SIZE].copy_from_slice(&[0x7f, 0xff, 4, 0]);
        assert_eq!(parse(&bytes), Some(extent));

        let over = extent + 1;
        bytes[2..4].copy_from_slice(&(over as u16).to_le_bytes());
        bytes[over..over + HEADER_SIZE].copy_from_slice(&[0x7f, 0xff, 4, 0]);
        assert_eq!(parse(&bytes), None);
    }
}
