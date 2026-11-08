//! Byte-level AML writer over a caller-supplied buffer: PkgLength, NameString, integers,
//! strings and length-prefixed packages. Nothing here allocates.
//!
//! Spec: ACPI 20.2.2 (names), 20.2.3 (data objects), 20.2.4 (PkgLength), 5.4.1 (explicit-length
//! encodings). See `opcodes.rs` for the edition note.

use super::AmlError;
use super::names::AmlName;
use super::opcodes::{
    BYTE_PREFIX, DUAL_NAME_PREFIX, DWORD_PREFIX, MULTI_NAME_PREFIX, NULL_NAME, ONE_OP,
    PARENT_PREFIX_CHAR, PKG_LEN_MAX_1, PKG_LEN_MAX_2, PKG_LEN_MAX_3, PKG_LEN_MAX_4, QWORD_PREFIX,
    ROOT_CHAR, STRING_PREFIX, WORD_PREFIX, ZERO_OP,
};
use crate::acpi::bytes::{copy_at, tri};

/// Encodes the PkgLength of a package whose content (everything after the PkgLength bytes) is
/// `body_len` bytes long. The PkgLength value counts its own bytes (ACPI 5.4.1: "the inclusive
/// length"), so the smallest encoding `n` with `body_len + n` in range is chosen.
///
/// Layout (ACPI 20.2.4): lead byte bits 7..6 = number of following bytes, bits 5..4 only used
/// for one-byte forms (value <= 0x3F), bits 3..0 = least significant nibble; each following byte
/// carries the next 8 bits.
pub const fn pkg_length_for_body(body_len: usize) -> Result<([u8; 4], usize), AmlError> {
    const LIMITS: [usize; 4] = [PKG_LEN_MAX_1, PKG_LEN_MAX_2, PKG_LEN_MAX_3, PKG_LEN_MAX_4];
    let mut index = 0;
    while index < LIMITS.len() {
        let n = index + 1;
        index += 1;
        let value = match body_len.checked_add(n) {
            Some(value) => value,
            None => return Err(AmlError::PkgLengthOverflow),
        };
        if value > LIMITS[n - 1] {
            continue;
        }
        let mut bytes = [0u8; 4];
        if n == 1 {
            bytes[0] = value as u8;
        } else {
            bytes[0] = (((n - 1) as u8) << 6) | (value & 0x0F) as u8;
            let mut k = 1;
            while k < n {
                bytes[k] = ((value >> (4 + 8 * (k - 1))) & 0xFF) as u8;
                k += 1;
            }
        }
        return Ok((bytes, n));
    }
    Err(AmlError::PkgLengthOverflow)
}

/// Decodes a PkgLength at the start of `bytes`: `(value, bytes consumed)`. The value includes
/// the PkgLength bytes themselves.
pub const fn decode_pkg_length(bytes: &[u8]) -> Result<(usize, usize), AmlError> {
    let lead = match bytes.first() {
        Some(&lead) => lead,
        None => return Err(AmlError::Truncated),
    };
    let follow = (lead >> 6) as usize;
    if follow == 0 {
        return Ok(((lead & 0x3F) as usize, 1));
    }
    if lead & 0x30 != 0 {
        // Bits 5..4 are reserved and must be zero in multi-byte forms (ACPI 20.2.4).
        return Err(AmlError::BadPkgLength);
    }
    let mut value = (lead & 0x0F) as usize;
    let mut k = 0;
    while k < follow {
        if 1 + k >= bytes.len() {
            return Err(AmlError::Truncated);
        }
        value |= (bytes[1 + k] as usize) << (4 + 8 * k);
        k += 1;
    }
    Ok((value, 1 + follow))
}

/// Marker returned by [`Encoder::begin_package`]: where the PkgLength goes.
#[derive(Clone, Copy, Debug)]
pub struct PkgMark {
    at: usize,
}

pub struct Encoder<'a> {
    buf: &'a mut [u8],
    len: usize,
}

impl<'a> Encoder<'a> {
    pub const fn new(buf: &'a mut [u8]) -> Self {
        Encoder { buf, len: 0 }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub const fn bytes(&self) -> &[u8] {
        self.buf.split_at(self.len).0
    }

    pub const fn put(&mut self, byte: u8) -> Result<(), AmlError> {
        if self.len >= self.buf.len() {
            return Err(AmlError::BufferFull);
        }
        self.buf[self.len] = byte;
        self.len += 1;
        Ok(())
    }

    pub const fn put_all(&mut self, bytes: &[u8]) -> Result<(), AmlError> {
        let end = match self.len.checked_add(bytes.len()) {
            Some(end) => end,
            None => return Err(AmlError::BufferFull),
        };
        if end > self.buf.len() {
            return Err(AmlError::BufferFull);
        }
        copy_at(self.buf, self.len, bytes);
        self.len = end;
        Ok(())
    }

    /// NameString (ACPI 20.2.2): `RootChar` or `^` prefixes, then NullName, a NameSeg, a
    /// DualNamePath (`0x2E`, two segments) or a MultiNamePath (`0x2F`, count, segments).
    pub const fn name(&mut self, name: &AmlName) -> Result<(), AmlError> {
        if name.absolute {
            tri!(self.put(ROOT_CHAR));
        }
        let mut prefix = 0;
        while prefix < name.parents {
            tri!(self.put(PARENT_PREFIX_CHAR));
            prefix += 1;
        }
        let segs = name.segs();
        match segs.len() {
            0 => self.put(NULL_NAME),
            1 => self.put_all(&segs[0].0),
            2 => {
                tri!(self.put(DUAL_NAME_PREFIX));
                tri!(self.put_all(&segs[0].0));
                self.put_all(&segs[1].0)
            }
            n => {
                tri!(self.put(MULTI_NAME_PREFIX));
                tri!(self.put(n as u8));
                let mut i = 0;
                while i < n {
                    tri!(self.put_all(&segs[i].0));
                    i += 1;
                }
                Ok(())
            }
        }
    }

    /// Integer constant in the smallest form: ZeroOp, OneOp, ByteConst, WordConst, DWordConst
    /// or QWordConst (ACPI 20.2.3).
    pub const fn integer(&mut self, value: u64) -> Result<(), AmlError> {
        match value {
            0 => self.put(ZERO_OP),
            1 => self.put(ONE_OP),
            2..=0xFF => {
                tri!(self.put(BYTE_PREFIX));
                self.put(value as u8)
            }
            0x100..=0xFFFF => {
                tri!(self.put(WORD_PREFIX));
                self.put_all(&(value as u16).to_le_bytes())
            }
            0x1_0000..=0xFFFF_FFFF => {
                tri!(self.put(DWORD_PREFIX));
                self.put_all(&(value as u32).to_le_bytes())
            }
            _ => {
                tri!(self.put(QWORD_PREFIX));
                self.put_all(&value.to_le_bytes())
            }
        }
    }

    /// String: StringPrefix, AsciiCharList (`0x01..=0x7F`), NullChar (ACPI 20.2.3).
    pub const fn string(&mut self, text: &str) -> Result<(), AmlError> {
        let raw = text.as_bytes();
        let mut i = 0;
        while i < raw.len() {
            if raw[i] == 0 || raw[i] > 0x7F {
                return Err(AmlError::BadString);
            }
            i += 1;
        }
        tri!(self.put(STRING_PREFIX));
        tri!(self.put_all(raw));
        self.put(0)
    }

    /// Starts an explicit-length encoding: the opcode bytes are written by the caller before
    /// this call; four bytes are reserved for the PkgLength and closed by [`end_package`].
    ///
    /// [`end_package`]: Encoder::end_package
    pub const fn begin_package(&mut self) -> Result<PkgMark, AmlError> {
        let at = self.len;
        tri!(self.put_all(&[0; 4]));
        Ok(PkgMark { at })
    }

    /// Writes the real PkgLength and closes the gap left by the reservation.
    pub const fn end_package(&mut self, mark: PkgMark) -> Result<(), AmlError> {
        let body_start = mark.at + 4;
        let body_len = self.len - body_start;
        let (bytes, n) = tri!(pkg_length_for_body(body_len));
        // Move the body left over the unused part of the reservation (overlapping ranges, the
        // destination is below the source, so a forward copy is safe).
        let mut i = 0;
        while i < body_len {
            self.buf[mark.at + n + i] = self.buf[body_start + i];
            i += 1;
        }
        let mut i = 0;
        while i < n {
            self.buf[mark.at + i] = bytes[i];
            i += 1;
        }
        self.len = mark.at + n + body_len;
        Ok(())
    }

    /// Overwrites one byte already written (used to patch `NumElements`).
    pub const fn patch(&mut self, at: usize, byte: u8) {
        self.buf[at] = byte;
    }
}
