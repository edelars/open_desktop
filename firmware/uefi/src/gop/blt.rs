//! Raster operations on byte-strided GOP framebuffers and caller-owned buffers.

use core::ptr;

use crate::uefi::status::Status;

use super::device::{Geometry, PIXEL_BYTES};

// Keep each compiler-rt string operation within the firmware probe's REP bound.
const COPY_BYTES: usize = 1024;

pub(super) const VIDEO_FILL: u32 = 0;
pub(super) const VIDEO_TO_BUFFER: u32 = 1;
pub(super) const BUFFER_TO_VIDEO: u32 = 2;
pub(super) const VIDEO_TO_VIDEO: u32 = 3;

#[derive(Clone, Copy)]
pub(super) struct Request {
    pub(super) buffer: *mut u8,
    pub(super) operation: u32,
    pub(super) source_x: usize,
    pub(super) source_y: usize,
    pub(super) destination_x: usize,
    pub(super) destination_y: usize,
    pub(super) width: usize,
    pub(super) height: usize,
    pub(super) delta: usize,
}

#[derive(Clone, Copy)]
struct Rows {
    start: *mut u8,
    width_bytes: usize,
    stride: usize,
}

impl Rows {
    fn new(
        base: *mut u8,
        x: usize,
        y: usize,
        width: usize,
        height: usize,
        stride: usize,
    ) -> Option<Self> {
        if base.is_null() {
            return None;
        }
        let x_bytes = x.checked_mul(PIXEL_BYTES)?;
        let width_bytes = width.checked_mul(PIXEL_BYTES)?;
        let row_end = x_bytes.checked_add(width_bytes)?;
        if row_end > stride {
            return None;
        }
        let start = y.checked_mul(stride)?.checked_add(x_bytes)?;
        let end = (height - 1)
            .checked_mul(stride)?
            .checked_add(start)?
            .checked_add(width_bytes)?;
        base.addr().checked_add(end)?;
        Some(Self {
            start: base.wrapping_add(start),
            width_bytes,
            stride,
        })
    }

    fn row(self, index: usize) -> *mut u8 {
        self.start.wrapping_add(index * self.stride)
    }
}

impl Request {
    fn video(self, base: *mut u8, mode: Geometry, x: usize, y: usize) -> Option<Rows> {
        if x.checked_add(self.width)? > mode.width as usize
            || y.checked_add(self.height)? > mode.height as usize
        {
            return None;
        }
        Rows::new(base, x, y, self.width, self.height, mode.stride as usize)
    }

    fn buffer(self, x: usize, y: usize) -> Option<Rows> {
        let stride = if self.delta == 0 {
            if x != 0 || y != 0 {
                return None;
            }
            self.width.checked_mul(PIXEL_BYTES)?
        } else {
            self.delta
        };
        Rows::new(self.buffer, x, y, self.width, self.height, stride)
    }

    /// Validate every row before the first write. Returns whether the framebuffer changed.
    ///
    /// # Safety
    /// `base` is the mapped framebuffer for `mode`, and any caller buffer backs the complete
    /// implied rectangle. The BSP owns the operation for its duration.
    pub(super) unsafe fn execute(self, base: *mut u8, mode: Geometry) -> Result<bool, Status> {
        if self.width == 0 || self.height == 0 {
            return Err(Status::INVALID_PARAMETER);
        }
        let source = || self.video(base, mode, self.source_x, self.source_y);
        let destination = || self.video(base, mode, self.destination_x, self.destination_y);
        let invalid = Status::INVALID_PARAMETER;
        match self.operation {
            VIDEO_FILL => {
                let destination = destination().ok_or(invalid)?;
                if self.buffer.is_null() || self.buffer.addr().checked_add(PIXEL_BYTES).is_none() {
                    return Err(invalid);
                }
                // SAFETY: the caller supplies one complete BLT pixel. The destination rows fit.
                let pixel = unsafe { self.buffer.cast::<[u8; PIXEL_BYTES]>().read() };
                for y in 0..self.height {
                    let row = destination.row(y);
                    for x in 0..self.width {
                        unsafe {
                            row.add(x * PIXEL_BYTES)
                                .cast::<[u8; PIXEL_BYTES]>()
                                .write_volatile(pixel)
                        };
                    }
                }
            }
            VIDEO_TO_BUFFER => unsafe {
                copy_rows(
                    source().ok_or(invalid)?,
                    self.buffer(self.destination_x, self.destination_y)
                        .ok_or(invalid)?,
                    self.height,
                    false,
                );
                return Ok(false);
            },
            BUFFER_TO_VIDEO => unsafe {
                copy_rows(
                    self.buffer(self.source_x, self.source_y).ok_or(invalid)?,
                    destination().ok_or(invalid)?,
                    self.height,
                    false,
                );
            },
            VIDEO_TO_VIDEO => unsafe {
                copy_rows(
                    source().ok_or(invalid)?,
                    destination().ok_or(invalid)?,
                    self.height,
                    self.destination_y > self.source_y,
                );
            },
            _ => return Err(invalid),
        }
        Ok(true)
    }
}

/// # Safety
/// `base` points to at least `bytes` writable framebuffer bytes.
pub(super) unsafe fn clear(base: *mut u8, bytes: usize) {
    let mut offset = 0;
    while offset < bytes {
        let count = (bytes - offset).min(COPY_BYTES);
        unsafe { base.add(offset).write_bytes(0, count) };
        offset += count;
    }
}

unsafe fn copy_rows(source: Rows, destination: Rows, height: usize, reverse: bool) {
    for index in 0..height {
        let row = if reverse { height - 1 - index } else { index };
        // SAFETY: both sets of rows were checked before entering the copy.
        unsafe { copy_row(source.row(row), destination.row(row), source.width_bytes) };
    }
}

unsafe fn copy_row(source: *const u8, destination: *mut u8, bytes: usize) {
    let backward = destination.addr() > source.addr() && destination.addr() - source.addr() < bytes;
    let mut copied = 0;
    while copied < bytes {
        let count = (bytes - copied).min(COPY_BYTES);
        let offset = if backward {
            bytes - copied - count
        } else {
            copied
        };
        // SAFETY: every chunk is within its validated row. copy() permits overlap.
        unsafe { ptr::copy(source.add(offset), destination.add(offset), count) };
        copied += count;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_moves_overlap_across_rows_and_copy_chunks() {
        const STRIDE: usize = 1280;
        let mode = Geometry {
            width: 320,
            height: 3,
            stride: STRIDE as u32,
            bytes: STRIDE * 3,
        };
        let original = core::array::from_fn::<_, { STRIDE * 3 }, _>(|i| (i % 251) as u8);
        let width = 300;
        for (source_x, source_y, destination_x, destination_y) in
            [(0, 0, 1, 1), (1, 1, 0, 0), (0, 0, 1, 0), (1, 0, 0, 0)]
        {
            let mut actual = original;
            let mut expected = original;
            for row in 0..2 {
                let source = (source_y + row) * STRIDE + source_x * PIXEL_BYTES;
                let destination = (destination_y + row) * STRIDE + destination_x * PIXEL_BYTES;
                let count = width * PIXEL_BYTES;
                expected[destination..destination + count]
                    .copy_from_slice(&original[source..source + count]);
            }
            let request = Request {
                buffer: ptr::null_mut(),
                operation: VIDEO_TO_VIDEO,
                source_x,
                source_y,
                destination_x,
                destination_y,
                width,
                height: 2,
                delta: 0,
            };
            assert_eq!(
                unsafe { request.execute(actual.as_mut_ptr(), mode) },
                Ok(true)
            );
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn invalid_buffer_rectangle_preserves_the_frame() {
        let mode = Geometry {
            width: 4,
            height: 4,
            stride: 16,
            bytes: 64,
        };
        let mut frame = [0x5a; 64];
        let mut source = [0xff; 64];
        let request = Request {
            buffer: source.as_mut_ptr(),
            operation: BUFFER_TO_VIDEO,
            source_x: 1,
            source_y: 1,
            destination_x: 0,
            destination_y: 0,
            width: 2,
            height: 2,
            delta: 16,
        };
        for invalid in [
            Request {
                delta: 0,
                ..request
            },
            Request {
                delta: 11,
                ..request
            },
            Request {
                source_y: usize::MAX,
                ..request
            },
            Request {
                destination_y: 3,
                ..request
            },
        ] {
            assert_eq!(
                unsafe { invalid.execute(frame.as_mut_ptr(), mode) },
                Err(Status::INVALID_PARAMETER)
            );
            assert_eq!(frame, [0x5a; 64]);
        }
    }
}
