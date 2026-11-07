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

/// Encodes the PkgLength of a package whose content (everything after the PkgLength bytes) is
/// `body_len` bytes long. The PkgLength value counts its own bytes (ACPI 5.4.1: "the inclusive
/// length"), so the smallest encoding `n` with `body_len + n` in range is chosen.
///
/// Layout (ACPI 20.2.4): lead byte bits 7..6 = number of following bytes, bits 5..4 only used
/// for one-byte forms (value <= 0x3F), bits 3..0 = least significant nibble; each following byte
/// carries the next 8 bits.
pub fn pkg_length_for_body(body_len: usize) -> Result<([u8; 4], usize), AmlError> {
    const LIMITS: [usize; 4] = [PKG_LEN_MAX_1, PKG_LEN_MAX_2, PKG_LEN_MAX_3, PKG_LEN_MAX_4];
    for (index, limit) in LIMITS.iter().enumerate() {
        let n = index + 1;
        let value = body_len.checked_add(n).ok_or(AmlError::PkgLengthOverflow)?;
        if value > *limit {
            continue;
        }
        let mut bytes = [0u8; 4];
        if n == 1 {
            bytes[0] = value as u8;
        } else {
            bytes[0] = (((n - 1) as u8) << 6) | (value & 0x0F) as u8;
            for (k, byte) in bytes.iter_mut().enumerate().take(n).skip(1) {
                *byte = ((value >> (4 + 8 * (k - 1))) & 0xFF) as u8;
            }
        }
        return Ok((bytes, n));
    }
    Err(AmlError::PkgLengthOverflow)
}

/// Decodes a PkgLength at the start of `bytes`: `(value, bytes consumed)`. The value includes
/// the PkgLength bytes themselves.
pub fn decode_pkg_length(bytes: &[u8]) -> Result<(usize, usize), AmlError> {
    let lead = *bytes.first().ok_or(AmlError::Truncated)?;
    let follow = usize::from(lead >> 6);
    if follow == 0 {
        return Ok((usize::from(lead & 0x3F), 1));
    }
    if lead & 0x30 != 0 {
        // Bits 5..4 are reserved and must be zero in multi-byte forms (ACPI 20.2.4).
        return Err(AmlError::BadPkgLength);
    }
    let mut value = usize::from(lead & 0x0F);
    for k in 0..follow {
        let byte = *bytes.get(1 + k).ok_or(AmlError::Truncated)?;
        value |= usize::from(byte) << (4 + 8 * k);
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
    pub fn new(buf: &'a mut [u8]) -> Self {
        Encoder { buf, len: 0 }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }

    pub fn put(&mut self, byte: u8) -> Result<(), AmlError> {
        let slot = self.buf.get_mut(self.len).ok_or(AmlError::BufferFull)?;
        *slot = byte;
        self.len += 1;
        Ok(())
    }

    pub fn put_all(&mut self, bytes: &[u8]) -> Result<(), AmlError> {
        let end = self
            .len
            .checked_add(bytes.len())
            .ok_or(AmlError::BufferFull)?;
        let dst = self
            .buf
            .get_mut(self.len..end)
            .ok_or(AmlError::BufferFull)?;
        dst.copy_from_slice(bytes);
        self.len = end;
        Ok(())
    }

    /// NameString (ACPI 20.2.2): `RootChar` or `^` prefixes, then NullName, a NameSeg, a
    /// DualNamePath (`0x2E`, two segments) or a MultiNamePath (`0x2F`, count, segments).
    pub fn name(&mut self, name: &AmlName) -> Result<(), AmlError> {
        if name.absolute {
            self.put(ROOT_CHAR)?;
        }
        for _ in 0..name.parents {
            self.put(PARENT_PREFIX_CHAR)?;
        }
        let segs = name.segs();
        match segs.len() {
            0 => self.put(NULL_NAME),
            1 => self.put_all(&segs[0].0),
            2 => {
                self.put(DUAL_NAME_PREFIX)?;
                self.put_all(&segs[0].0)?;
                self.put_all(&segs[1].0)
            }
            n => {
                self.put(MULTI_NAME_PREFIX)?;
                self.put(n as u8)?;
                for seg in segs {
                    self.put_all(&seg.0)?;
                }
                Ok(())
            }
        }
    }

    /// Integer constant in the smallest form: ZeroOp, OneOp, ByteConst, WordConst, DWordConst
    /// or QWordConst (ACPI 20.2.3).
    pub fn integer(&mut self, value: u64) -> Result<(), AmlError> {
        match value {
            0 => self.put(ZERO_OP),
            1 => self.put(ONE_OP),
            2..=0xFF => {
                self.put(BYTE_PREFIX)?;
                self.put(value as u8)
            }
            0x100..=0xFFFF => {
                self.put(WORD_PREFIX)?;
                self.put_all(&(value as u16).to_le_bytes())
            }
            0x1_0000..=0xFFFF_FFFF => {
                self.put(DWORD_PREFIX)?;
                self.put_all(&(value as u32).to_le_bytes())
            }
            _ => {
                self.put(QWORD_PREFIX)?;
                self.put_all(&value.to_le_bytes())
            }
        }
    }

    /// String: StringPrefix, AsciiCharList (`0x01..=0x7F`), NullChar (ACPI 20.2.3).
    pub fn string(&mut self, text: &str) -> Result<(), AmlError> {
        if text.bytes().any(|c| c == 0 || c > 0x7F) {
            return Err(AmlError::BadString);
        }
        self.put(STRING_PREFIX)?;
        self.put_all(text.as_bytes())?;
        self.put(0)
    }

    /// Starts an explicit-length encoding: the opcode bytes are written by the caller before
    /// this call; four bytes are reserved for the PkgLength and closed by [`end_package`].
    ///
    /// [`end_package`]: Encoder::end_package
    pub fn begin_package(&mut self) -> Result<PkgMark, AmlError> {
        let at = self.len;
        self.put_all(&[0; 4])?;
        Ok(PkgMark { at })
    }

    /// Writes the real PkgLength and closes the gap left by the reservation.
    pub fn end_package(&mut self, mark: PkgMark) -> Result<(), AmlError> {
        let body_start = mark.at + 4;
        let body_len = self.len - body_start;
        let (bytes, n) = pkg_length_for_body(body_len)?;
        self.buf.copy_within(body_start..self.len, mark.at + n);
        self.buf[mark.at..mark.at + n].copy_from_slice(&bytes[..n]);
        self.len = mark.at + n + body_len;
        Ok(())
    }

    /// Overwrites one byte already written (used to patch `NumElements`).
    pub fn patch(&mut self, at: usize, byte: u8) {
        self.buf[at] = byte;
    }
}
