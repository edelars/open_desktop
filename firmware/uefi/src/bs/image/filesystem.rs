//! Resolve an image's Media File Path nodes through Simple File System.

use crate::uefi::status::Status;
use core::mem::MaybeUninit;

const FILE_PATH: [u8; 2] = [4, 4];
const SEPARATOR: u16 = b'\\' as u16;

fn filename(nodes: &[u8], output: &mut [MaybeUninit<u16>]) -> Result<(), Status> {
    let mut remaining = nodes;
    let mut written = 0;
    let mut last = 0;
    while !remaining.is_empty() {
        let header = remaining.get(..4).ok_or(Status::INVALID_PARAMETER)?;
        if header[..2] != FILE_PATH {
            return Err(Status::UNSUPPORTED);
        }
        let length = usize::from(u16::from_le_bytes([header[2], header[3]]));
        if length < 8 || length % 2 != 0 {
            return Err(Status::INVALID_PARAMETER);
        }
        let (node, rest) = remaining
            .split_at_checked(length)
            .ok_or(Status::INVALID_PARAMETER)?;
        if node[length - 2..] != [0, 0] {
            return Err(Status::INVALID_PARAMETER);
        }
        let first = u16::from_le_bytes([node[4], node[5]]);
        if written > 0 && last != SEPARATOR && first != SEPARATOR {
            push_unit(output, &mut written, SEPARATOR)?;
        }
        for pair in node[4..length - 2].chunks_exact(2) {
            let unit = u16::from_le_bytes([pair[0], pair[1]]);
            if unit == 0 || (0xd800..=0xdfff).contains(&unit) {
                return Err(Status::INVALID_PARAMETER);
            }
            if unit != SEPARATOR || last != SEPARATOR {
                push_unit(output, &mut written, unit)?;
            }
            last = unit;
        }
        remaining = rest;
    }
    if written == 0 {
        return Err(Status::NOT_FOUND);
    }
    push_unit(output, &mut written, 0)
}

fn push_unit(
    output: &mut [MaybeUninit<u16>],
    written: &mut usize,
    unit: u16,
) -> Result<(), Status> {
    output
        .get_mut(*written)
        .ok_or(Status::BAD_BUFFER_SIZE)?
        .write(unit);
    *written += 1;
    Ok(())
}

#[cfg(target_os = "none")]
mod source {
    use super::*;
    use crate::bs::memory::BootPool;
    use crate::fs::simple_fs::{FILE_INFO_GUID, FileInfo, FileProtocol, SimpleFileSystem};
    use core::{ptr, ptr::NonNull, slice};

    /// Bytes owned by Boot Services until the PE loader has copied the image.
    pub(in crate::bs::image) struct FileBuffer(BootPool);

    impl FileBuffer {
        pub(in crate::bs::image) fn as_bytes(&self) -> &[u8] {
            // Only a complete and successful File.Read publishes this source.
            unsafe { slice::from_raw_parts(self.0.as_ptr(), self.0.len()) }
        }
    }

    struct OpenFile(NonNull<FileProtocol>);

    impl Drop for OpenFile {
        fn drop(&mut self) {
            let file = self.0.as_ptr();
            unsafe { ((*file).close)(file) };
        }
    }

    fn check(status: Status) -> Result<(), Status> {
        if status == Status::SUCCESS {
            Ok(())
        } else {
            Err(status)
        }
    }

    /// The provider and path must remain valid across the protocol calls.
    pub(in crate::bs::image) unsafe fn read_file(
        provider: NonNull<SimpleFileSystem>,
        nodes: &[u8],
    ) -> Result<FileBuffer, Status> {
        // Every node includes more framing bytes than the maximum separator it adds.
        let path_buffer = unsafe { BootPool::allocate(nodes.len() + 2) }?;
        let units = unsafe {
            slice::from_raw_parts_mut(
                path_buffer.as_ptr().cast::<MaybeUninit<u16>>(),
                path_buffer.len() / 2,
            )
        };
        filename(nodes, units)?;

        let provider = provider.as_ptr();
        let mut root = ptr::null_mut();
        check(unsafe { ((*provider).open_volume)(provider, &mut root) })?;
        let root_handle = OpenFile(NonNull::new(root).ok_or(Status::DEVICE_ERROR)?);
        let mut file = ptr::null_mut();
        // The decoder wrote every unit through the terminating NUL.
        check(unsafe { ((*root).open)(root, &mut file, units.as_ptr().cast(), 1, 0) })?;
        let _file_handle = OpenFile(NonNull::new(file).ok_or(Status::DEVICE_ERROR)?);
        drop(root_handle);
        drop(path_buffer);

        // The FAT driver exposes a maximum of 255 filename units plus the NUL.
        let mut info = [0u8; size_of::<FileInfo>() + 256 * size_of::<u16>()];
        let mut info_size = info.len();
        check(unsafe {
            ((*file).get_info)(
                file,
                &FILE_INFO_GUID,
                &mut info_size,
                info.as_mut_ptr().cast(),
            )
        })?;
        if !(size_of::<FileInfo>()..=info.len()).contains(&info_size) {
            return Err(Status::DEVICE_ERROR);
        }
        let info = unsafe { info.as_ptr().cast::<FileInfo>().read_unaligned() };
        if info.attribute & 0x10 != 0 {
            return Err(Status::LOAD_ERROR);
        }
        let size = usize::try_from(info.file_size).map_err(|_| Status::BAD_BUFFER_SIZE)?;
        if size == 0 || size > isize::MAX as usize {
            return Err(Status::LOAD_ERROR);
        }
        let buffer = unsafe { BootPool::allocate(size) }?;
        let mut actual = size;
        check(unsafe { ((*file).read)(file, &mut actual, buffer.as_ptr().cast()) })?;
        if actual != size {
            return Err(Status::DEVICE_ERROR);
        }
        Ok(FileBuffer(buffer))
    }
}

#[cfg(target_os = "none")]
pub(super) use source::{FileBuffer, read_file};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_joins_file_nodes_at_one_separator() {
        let mut out = [MaybeUninit::new(0xa5); 12];
        for nodes in [
            &[4, 4, 8, 0, b'A', 0, 0, 0, 4, 4, 8, 0, b'B', 0, 0, 0][..],
            &[
                4, 4, 10, 0, b'A', 0, b'\\', 0, 0, 0, 4, 4, 10, 0, b'\\', 0, b'B', 0, 0, 0,
            ][..],
        ] {
            assert_eq!(filename(nodes, &mut out), Ok(()));
            let prefix = out.map(|unit| unsafe { unit.assume_init() });
            assert_eq!(prefix[..5], [b'A' as u16, SEPARATOR, b'B' as u16, 0, 0xa5]);
        }
        assert_eq!(
            filename(&[4, 4, 8, 0, b'X', 0, 0, 0], &mut out[..1]),
            Err(Status::BAD_BUFFER_SIZE)
        );
    }

    #[test]
    fn filename_rejects_unterminated_or_non_file_nodes() {
        let mut out = [MaybeUninit::uninit(); 12];
        for bytes in [
            &[4, 4, 8, 0, 1, 0, 0][..],
            &[4, 4, 9, 0, 1, 0, 0, 0, 0][..],
            &[4, 4, 8, 0, 1, 0, 2, 0][..],
            &[4, 4, 10, 0, 0, 0, 2, 0, 0, 0][..],
            &[4, 4, 8, 0, 0, 0xd8, 0, 0][..],
        ] {
            assert_eq!(filename(bytes, &mut out), Err(Status::INVALID_PARAMETER));
        }
        assert_eq!(filename(&[4, 1, 4, 0], &mut out), Err(Status::UNSUPPORTED));
        assert_eq!(filename(&[], &mut out), Err(Status::NOT_FOUND));
    }
}
