//! NameSeg, NameString and the name registry.
//!
//! Grammar: ACPI 20.2.2 "Name Objects Encoding" (LeadNameChar, NameChar, NameSeg, NameString,
//! PrefixPath, NamePath, DualNamePath, MultiNamePath, NullName).

use super::AmlError;
use super::opcodes::{
    DUAL_NAME_PREFIX, MULTI_NAME_PREFIX, NULL_NAME, PARENT_PREFIX_CHAR, ROOT_CHAR,
};
use crate::acpi::bytes::tri;

/// Most segments in one path (declared name or absolute path).
pub const MAX_SEGS: usize = 8;
/// Most named objects one registry tracks.
pub const MAX_NAMES: usize = 64;

/// Four-character name segment, padded with `_` (ACPI 20.2.2: "NameSegs shorter than 4
/// characters are filled with trailing underscores").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NameSeg(pub [u8; 4]);

impl NameSeg {
    pub const PAD: NameSeg = NameSeg([b'_'; 4]);

    /// Validates four raw bytes: lead `A-Z` or `_`, the rest `A-Z`, `0-9` or `_`.
    pub const fn from_bytes(bytes: [u8; 4]) -> Result<Self, AmlError> {
        let lead_ok = bytes[0].is_ascii_uppercase() || bytes[0] == b'_';
        let mut rest_ok = true;
        let mut i = 1;
        while i < 4 {
            let c = bytes[i];
            if !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_') {
                rest_ok = false;
            }
            i += 1;
        }
        if lead_ok && rest_ok {
            Ok(NameSeg(bytes))
        } else {
            Err(AmlError::BadNameSeg)
        }
    }

    /// Parses 1 to 4 characters and pads with `_`.
    pub const fn parse(text: &str) -> Result<Self, AmlError> {
        let raw = text.as_bytes();
        Self::parse_range(raw, 0, raw.len())
    }

    /// Parses `raw[from..to]` (1 to 4 characters) and pads with `_`.
    const fn parse_range(raw: &[u8], from: usize, to: usize) -> Result<Self, AmlError> {
        if to <= from || to - from > 4 {
            return Err(AmlError::BadNameSeg);
        }
        let mut bytes = [b'_'; 4];
        let mut i = from;
        while i < to {
            bytes[i - from] = raw[i];
            i += 1;
        }
        Self::from_bytes(bytes)
    }
}

/// A parsed NameString: optional `\` or `^` prefixes plus 0 to `MAX_SEGS` segments.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AmlName {
    pub absolute: bool,
    /// Number of `^` prefixes; always 0 when `absolute`.
    pub parents: u8,
    segs: [NameSeg; MAX_SEGS],
    count: u8,
}

impl AmlName {
    const fn empty() -> Self {
        AmlName {
            absolute: false,
            parents: 0,
            segs: [NameSeg::PAD; MAX_SEGS],
            count: 0,
        }
    }

    /// A relative name made of one segment.
    pub const fn from_seg(seg: NameSeg) -> Self {
        let mut name = Self::empty();
        name.segs[0] = seg;
        name.count = 1;
        name
    }

    pub const fn segs(&self) -> &[NameSeg] {
        self.segs.split_at(self.count as usize).0
    }

    /// Parses ASL-style text: `\_SB.PCI0`, `^FOO`, `ABCD`, `\` (root alone).
    pub const fn parse(text: &str) -> Result<Self, AmlError> {
        let raw = text.as_bytes();
        let mut name = Self::empty();
        let mut at = 0;
        if !raw.is_empty() && raw[0] == b'\\' {
            name.absolute = true;
            at = 1;
        } else {
            while at < raw.len() && raw[at] == b'^' {
                name.parents = match name.parents.checked_add(1) {
                    Some(parents) => parents,
                    None => return Err(AmlError::NameTooLong),
                };
                at += 1;
            }
        }
        if at == raw.len() {
            return Ok(name);
        }
        // Split the rest at '.'; an empty part (leading, trailing or doubled dot) is a bad seg.
        let mut start = at;
        loop {
            let mut end = start;
            while end < raw.len() && raw[end] != b'.' {
                end += 1;
            }
            tri!(name.push(tri!(NameSeg::parse_range(raw, start, end))));
            if end == raw.len() {
                break;
            }
            start = end + 1;
        }
        Ok(name)
    }

    const fn push(&mut self, seg: NameSeg) -> Result<(), AmlError> {
        let index = self.count as usize;
        if index == MAX_SEGS {
            return Err(AmlError::NameTooLong);
        }
        self.segs[index] = seg;
        self.count += 1;
        Ok(())
    }

    /// Decodes a NameString from AML bytes; returns the name and the bytes consumed.
    pub const fn decode(bytes: &[u8]) -> Result<(Self, usize), AmlError> {
        let mut name = Self::empty();
        let mut at = 0;
        if bytes.is_empty() {
            return Err(AmlError::Truncated);
        }
        match bytes[0] {
            ROOT_CHAR => {
                name.absolute = true;
                at = 1;
            }
            PARENT_PREFIX_CHAR => {
                while at < bytes.len() && bytes[at] == PARENT_PREFIX_CHAR {
                    name.parents = match name.parents.checked_add(1) {
                        Some(parents) => parents,
                        None => return Err(AmlError::NameTooLong),
                    };
                    at += 1;
                }
            }
            _ => {}
        }
        if at >= bytes.len() {
            return Err(AmlError::Truncated);
        }
        match bytes[at] {
            NULL_NAME => at += 1,
            DUAL_NAME_PREFIX => {
                tri!(name.push(tri!(seg_at(bytes, at + 1))));
                tri!(name.push(tri!(seg_at(bytes, at + 5))));
                at += 9;
            }
            MULTI_NAME_PREFIX => {
                if at + 1 >= bytes.len() {
                    return Err(AmlError::Truncated);
                }
                let n = bytes[at + 1] as usize;
                if n == 0 {
                    return Err(AmlError::BadNameSeg);
                }
                let mut i = 0;
                while i < n {
                    tri!(name.push(tri!(seg_at(bytes, at + 2 + 4 * i))));
                    i += 1;
                }
                at += 2 + 4 * n;
            }
            _ => {
                tri!(name.push(tri!(seg_at(bytes, at))));
                at += 4;
            }
        }
        Ok((name, at))
    }
}

/// The NameSeg at `bytes[at..at + 4]`.
const fn seg_at(bytes: &[u8], at: usize) -> Result<NameSeg, AmlError> {
    if at + 4 > bytes.len() {
        return Err(AmlError::Truncated);
    }
    NameSeg::from_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

/// Absolute path of a namespace object or scope; the empty path is the root `\`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AbsPath {
    segs: [NameSeg; MAX_SEGS],
    len: u8,
}

impl AbsPath {
    pub const ROOT: AbsPath = AbsPath {
        segs: [NameSeg::PAD; MAX_SEGS],
        len: 0,
    };

    pub const fn depth(&self) -> usize {
        self.len as usize
    }

    /// Resolves a declared name against this scope: absolute names ignore it, `^` climbs.
    pub const fn resolve(&self, name: &AmlName) -> Result<AbsPath, AmlError> {
        let mut path = if name.absolute {
            AbsPath::ROOT
        } else {
            let keep = match self.depth().checked_sub(name.parents as usize) {
                Some(keep) => keep,
                None => return Err(AmlError::BadParentPrefix),
            };
            let mut base = *self;
            base.len = keep as u8;
            base
        };
        let segs = name.segs();
        let mut i = 0;
        while i < segs.len() {
            let index = path.depth();
            if index == MAX_SEGS {
                return Err(AmlError::NameTooLong);
            }
            path.segs[index] = segs[i];
            path.len += 1;
            i += 1;
        }
        Ok(path)
    }

    const fn same(&self, other: &AbsPath) -> bool {
        if self.depth() != other.depth() {
            return false;
        }
        let mut i = 0;
        while i < self.depth() {
            let (a, b) = (self.segs[i].0, other.segs[i].0);
            if a[0] != b[0] || a[1] != b[1] || a[2] != b[2] || a[3] != b[3] {
                return false;
            }
            i += 1;
        }
        true
    }
}

/// Every named object (Name, Device, Method) of one definition block, by absolute path.
/// A second object with the same path is `DuplicateName`; `Scope` does not register a name.
pub struct Registry {
    paths: [AbsPath; MAX_NAMES],
    count: usize,
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

impl Registry {
    pub const fn new() -> Self {
        Registry {
            paths: [AbsPath::ROOT; MAX_NAMES],
            count: 0,
        }
    }

    pub const fn contains(&self, path: &AbsPath) -> bool {
        let mut i = 0;
        while i < self.count {
            if self.paths[i].same(path) {
                return true;
            }
            i += 1;
        }
        false
    }

    pub const fn insert(&mut self, path: AbsPath) -> Result<(), AmlError> {
        if self.contains(&path) {
            return Err(AmlError::DuplicateName);
        }
        if self.count == MAX_NAMES {
            return Err(AmlError::TooManyNames);
        }
        self.paths[self.count] = path;
        self.count += 1;
        Ok(())
    }
}
