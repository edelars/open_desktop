//! NameSeg, NameString and the name registry.
//!
//! Grammar: ACPI 20.2.2 "Name Objects Encoding" (LeadNameChar, NameChar, NameSeg, NameString,
//! PrefixPath, NamePath, DualNamePath, MultiNamePath, NullName).

use super::AmlError;
use super::opcodes::{
    DUAL_NAME_PREFIX, MULTI_NAME_PREFIX, NULL_NAME, PARENT_PREFIX_CHAR, ROOT_CHAR,
};

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
    pub fn from_bytes(bytes: [u8; 4]) -> Result<Self, AmlError> {
        let lead_ok = bytes[0].is_ascii_uppercase() || bytes[0] == b'_';
        let rest_ok = bytes[1..]
            .iter()
            .all(|&c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_');
        if lead_ok && rest_ok {
            Ok(NameSeg(bytes))
        } else {
            Err(AmlError::BadNameSeg)
        }
    }

    /// Parses 1 to 4 characters and pads with `_`.
    pub fn parse(text: &str) -> Result<Self, AmlError> {
        let raw = text.as_bytes();
        if raw.is_empty() || raw.len() > 4 {
            return Err(AmlError::BadNameSeg);
        }
        let mut bytes = [b'_'; 4];
        bytes[..raw.len()].copy_from_slice(raw);
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
    fn empty() -> Self {
        AmlName {
            absolute: false,
            parents: 0,
            segs: [NameSeg::PAD; MAX_SEGS],
            count: 0,
        }
    }

    /// A relative name made of one segment.
    pub fn from_seg(seg: NameSeg) -> Self {
        let mut name = Self::empty();
        name.segs[0] = seg;
        name.count = 1;
        name
    }

    pub fn segs(&self) -> &[NameSeg] {
        &self.segs[..usize::from(self.count)]
    }

    /// Parses ASL-style text: `\_SB.PCI0`, `^FOO`, `ABCD`, `\` (root alone).
    pub fn parse(text: &str) -> Result<Self, AmlError> {
        let mut name = Self::empty();
        let mut rest = text;
        if let Some(stripped) = rest.strip_prefix('\\') {
            name.absolute = true;
            rest = stripped;
        } else {
            while let Some(stripped) = rest.strip_prefix('^') {
                name.parents = name.parents.checked_add(1).ok_or(AmlError::NameTooLong)?;
                rest = stripped;
            }
        }
        if rest.is_empty() {
            return Ok(name);
        }
        for part in rest.split('.') {
            name.push(NameSeg::parse(part)?)?;
        }
        Ok(name)
    }

    fn push(&mut self, seg: NameSeg) -> Result<(), AmlError> {
        let index = usize::from(self.count);
        if index == MAX_SEGS {
            return Err(AmlError::NameTooLong);
        }
        self.segs[index] = seg;
        self.count += 1;
        Ok(())
    }

    /// Decodes a NameString from AML bytes; returns the name and the bytes consumed.
    pub fn decode(bytes: &[u8]) -> Result<(Self, usize), AmlError> {
        let mut name = Self::empty();
        let mut at = 0;
        match bytes.first().copied() {
            None => return Err(AmlError::Truncated),
            Some(ROOT_CHAR) => {
                name.absolute = true;
                at = 1;
            }
            Some(PARENT_PREFIX_CHAR) => {
                while bytes.get(at) == Some(&PARENT_PREFIX_CHAR) {
                    name.parents = name.parents.checked_add(1).ok_or(AmlError::NameTooLong)?;
                    at += 1;
                }
            }
            Some(_) => {}
        }
        let seg_at = |at: usize| -> Result<NameSeg, AmlError> {
            let raw = bytes.get(at..at + 4).ok_or(AmlError::Truncated)?;
            NameSeg::from_bytes([raw[0], raw[1], raw[2], raw[3]])
        };
        match bytes.get(at).copied().ok_or(AmlError::Truncated)? {
            NULL_NAME => at += 1,
            DUAL_NAME_PREFIX => {
                name.push(seg_at(at + 1)?)?;
                name.push(seg_at(at + 5)?)?;
                at += 9;
            }
            MULTI_NAME_PREFIX => {
                let n = usize::from(*bytes.get(at + 1).ok_or(AmlError::Truncated)?);
                if n == 0 {
                    return Err(AmlError::BadNameSeg);
                }
                for i in 0..n {
                    name.push(seg_at(at + 2 + 4 * i)?)?;
                }
                at += 2 + 4 * n;
            }
            _ => {
                name.push(seg_at(at)?)?;
                at += 4;
            }
        }
        Ok((name, at))
    }
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

    pub fn depth(&self) -> usize {
        usize::from(self.len)
    }

    /// Resolves a declared name against this scope: absolute names ignore it, `^` climbs.
    pub fn resolve(&self, name: &AmlName) -> Result<AbsPath, AmlError> {
        let mut path = if name.absolute {
            AbsPath::ROOT
        } else {
            let keep = self
                .depth()
                .checked_sub(usize::from(name.parents))
                .ok_or(AmlError::BadParentPrefix)?;
            let mut base = *self;
            base.len = keep as u8;
            base
        };
        for seg in name.segs() {
            let index = path.depth();
            if index == MAX_SEGS {
                return Err(AmlError::NameTooLong);
            }
            path.segs[index] = *seg;
            path.len += 1;
        }
        Ok(path)
    }

    fn same(&self, other: &AbsPath) -> bool {
        self.segs[..self.depth()] == other.segs[..other.depth()]
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

    pub fn contains(&self, path: &AbsPath) -> bool {
        self.paths[..self.count].iter().any(|p| p.same(path))
    }

    pub fn insert(&mut self, path: AbsPath) -> Result<(), AmlError> {
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
