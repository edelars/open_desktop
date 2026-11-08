//! Structural validation of a finished definition block: every package stays inside its parent
//! and no object name repeats within one scope. It parses only the subset the builder emits
//! (Scope, Device, Name, Method at term level; data objects inside Name) and rejects any other
//! opcode; it is not an AML interpreter. It checks only package bounds and repeated names. It does
//! NOT parse the contents of `Buffer` / `ResourceTemplate`, the `NumElements` of `Package`, or the
//! body of `Method` (that is the scope of plan item 3 of S7-12).

use super::AmlError;
use super::encoder::decode_pkg_length;
use super::names::{AbsPath, AmlName, MAX_SEGS, Registry};
use super::opcodes::{
    BUFFER_OP, BYTE_PREFIX, DEVICE_OP_SUFFIX, DWORD_PREFIX, EXT_OP_PREFIX, METHOD_OP, NAME_OP,
    ONE_OP, ONES_OP, PACKAGE_OP, QWORD_PREFIX, SCOPE_OP, STRING_PREFIX, VAR_PACKAGE_OP,
    WORD_PREFIX, ZERO_OP,
};

const MAX_DEPTH: usize = MAX_SEGS;

/// Checks the AML of a definition block (the bytes after the 36-byte table header).
pub fn validate_definition_block(aml: &[u8]) -> Result<(), AmlError> {
    let mut registry = Registry::new();
    walk(aml, &AbsPath::ROOT, &mut registry, 0)
}

/// Reads `PkgLength` at `at`; returns `(content_start, end)` where `end` is the exclusive end
/// of the package, checked against `limit`.
fn package_bounds(buf: &[u8], at: usize) -> Result<(usize, usize), AmlError> {
    let (value, used) = decode_pkg_length(buf.get(at..).ok_or(AmlError::Truncated)?)?;
    if value < used {
        return Err(AmlError::BadPkgLength);
    }
    let end = at.checked_add(value).ok_or(AmlError::BadPkgLength)?;
    if end > buf.len() {
        return Err(AmlError::BadPkgLength);
    }
    Ok((at + used, end))
}

fn walk(
    buf: &[u8],
    scope: &AbsPath,
    registry: &mut Registry,
    depth: usize,
) -> Result<(), AmlError> {
    if depth > MAX_DEPTH {
        return Err(AmlError::TooDeep);
    }
    let mut at = 0;
    while at < buf.len() {
        match buf[at] {
            SCOPE_OP => {
                let (start, end) = package_bounds(buf, at + 1)?;
                let (name, used) = AmlName::decode(&buf[start..end])?;
                let inner = scope.resolve(&name)?;
                walk(&buf[start + used..end], &inner, registry, depth + 1)?;
                at = end;
            }
            EXT_OP_PREFIX if buf.get(at + 1) == Some(&DEVICE_OP_SUFFIX) => {
                let (start, end) = package_bounds(buf, at + 2)?;
                let (name, used) = AmlName::decode(&buf[start..end])?;
                let inner = register(scope, &name, registry)?;
                walk(&buf[start + used..end], &inner, registry, depth + 1)?;
                at = end;
            }
            METHOD_OP => {
                let (start, end) = package_bounds(buf, at + 1)?;
                let (name, used) = AmlName::decode(&buf[start..end])?;
                register(scope, &name, registry)?;
                // MethodFlags byte must be present; the body is opaque.
                if start + used >= end {
                    return Err(AmlError::Truncated);
                }
                at = end;
            }
            NAME_OP => {
                let (name, used) = AmlName::decode(&buf[at + 1..])?;
                register(scope, &name, registry)?;
                at = skip_data_object(buf, at + 1 + used)?;
            }
            other => return Err(AmlError::UnsupportedOpcode(other)),
        }
    }
    Ok(())
}

fn register(scope: &AbsPath, name: &AmlName, registry: &mut Registry) -> Result<AbsPath, AmlError> {
    if name.segs().is_empty() {
        return Err(AmlError::EmptyName);
    }
    let path = scope.resolve(name)?;
    registry.insert(path)?;
    Ok(path)
}

/// Returns the offset just past the data object that starts at `at`.
fn skip_data_object(buf: &[u8], at: usize) -> Result<usize, AmlError> {
    let fixed = |len: usize| -> Result<usize, AmlError> {
        let end = at + len;
        if end > buf.len() {
            Err(AmlError::Truncated)
        } else {
            Ok(end)
        }
    };
    match *buf.get(at).ok_or(AmlError::Truncated)? {
        ZERO_OP | ONE_OP | ONES_OP => fixed(1),
        BYTE_PREFIX => fixed(2),
        WORD_PREFIX => fixed(3),
        DWORD_PREFIX => fixed(5),
        QWORD_PREFIX => fixed(9),
        STRING_PREFIX => {
            let nul = buf[at + 1..]
                .iter()
                .position(|&c| c == 0)
                .ok_or(AmlError::Truncated)?;
            Ok(at + 1 + nul + 1)
        }
        BUFFER_OP | PACKAGE_OP | VAR_PACKAGE_OP => package_bounds(buf, at + 1).map(|(_, end)| end),
        other => Err(AmlError::UnsupportedOpcode(other)),
    }
}
