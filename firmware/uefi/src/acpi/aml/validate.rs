//! Checks namespace names and package bounds in the builder's definition blocks (S7-12).
//! The validator walks Scope and Device term lists and Name data objects, including nested
//! fixed-size Package elements. Buffer payloads, VarPackage and Method bodies stay opaque.

use super::AmlError;
use super::builder::MAX_NEST;
use super::encoder::decode_pkg_length;
use super::names::{AbsPath, AmlName, MAX_SEGS, Registry};
use super::opcodes::{
    BUFFER_OP, BYTE_PREFIX, DEVICE_OP_SUFFIX, DWORD_PREFIX, EXT_OP_PREFIX, METHOD_OP, NAME_OP,
    ONE_OP, ONES_OP, PACKAGE_OP, QWORD_PREFIX, ROOT_CHAR, SCOPE_OP, STRING_PREFIX, VAR_PACKAGE_OP,
    WORD_PREFIX, ZERO_OP,
};
use crate::acpi::bytes::{sub, tri};

const MAX_DEPTH: usize = MAX_SEGS;

/// Checks the AML of a definition block (the bytes after the 36-byte table header).
pub const fn validate_definition_block(aml: &[u8]) -> Result<(), AmlError> {
    let mut registry = Registry::new();
    walk(aml, &AbsPath::ROOT, &mut registry, 0)
}

/// Reads `PkgLength` at `at`; returns `(content_start, end)` where `end` is the exclusive end
/// of the package, checked against `limit`.
const fn package_bounds(buf: &[u8], at: usize) -> Result<(usize, usize), AmlError> {
    if at > buf.len() {
        return Err(AmlError::Truncated);
    }
    let (value, used) = tri!(decode_pkg_length(buf.split_at(at).1));
    if value < used {
        return Err(AmlError::BadPkgLength);
    }
    let end = match at.checked_add(value) {
        Some(end) => end,
        None => return Err(AmlError::BadPkgLength),
    };
    if end > buf.len() {
        return Err(AmlError::BadPkgLength);
    }
    Ok((at + used, end))
}

const fn walk(
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
                let (start, end) = tri!(package_bounds(buf, at + 1));
                let (name, used) = tri!(AmlName::decode(sub(buf, start, end)));
                let inner = tri!(scope.resolve(&name));
                tri!(walk(
                    sub(buf, start + used, end),
                    &inner,
                    registry,
                    depth + 1
                ));
                at = end;
            }
            EXT_OP_PREFIX if at + 1 < buf.len() && buf[at + 1] == DEVICE_OP_SUFFIX => {
                let (start, end) = tri!(package_bounds(buf, at + 2));
                let (name, used) = tri!(AmlName::decode(sub(buf, start, end)));
                let inner = tri!(register(scope, &name, registry));
                tri!(walk(
                    sub(buf, start + used, end),
                    &inner,
                    registry,
                    depth + 1
                ));
                at = end;
            }
            METHOD_OP => {
                let (start, end) = tri!(package_bounds(buf, at + 1));
                let (name, used) = tri!(AmlName::decode(sub(buf, start, end)));
                tri!(register(scope, &name, registry));
                // MethodFlags byte must be present; the body is opaque.
                if start + used >= end {
                    return Err(AmlError::Truncated);
                }
                at = end;
            }
            NAME_OP => {
                let (name, used) = tri!(AmlName::decode(sub(buf, at + 1, buf.len())));
                tri!(register(scope, &name, registry));
                at = tri!(skip_data_object(buf, at + 1 + used, 0));
            }
            other => return Err(AmlError::UnsupportedOpcode(other)),
        }
    }
    Ok(())
}

const fn register(
    scope: &AbsPath,
    name: &AmlName,
    registry: &mut Registry,
) -> Result<AbsPath, AmlError> {
    if name.segs().is_empty() {
        return Err(AmlError::EmptyName);
    }
    let path = tri!(scope.resolve(name));
    tri!(registry.insert(path));
    Ok(path)
}

/// Returns the offset just past the data object that starts at `at`.
const fn skip_data_object(buf: &[u8], at: usize, package_depth: usize) -> Result<usize, AmlError> {
    if at >= buf.len() {
        return Err(AmlError::Truncated);
    }
    match buf[at] {
        ZERO_OP | ONE_OP | ONES_OP => fixed(buf, at, 1),
        BYTE_PREFIX => fixed(buf, at, 2),
        WORD_PREFIX => fixed(buf, at, 3),
        DWORD_PREFIX => fixed(buf, at, 5),
        QWORD_PREFIX => fixed(buf, at, 9),
        STRING_PREFIX => {
            let mut nul = at + 1;
            while nul < buf.len() && buf[nul] != 0 {
                nul += 1;
            }
            if nul >= buf.len() {
                return Err(AmlError::Truncated);
            }
            Ok(nul + 1)
        }
        ROOT_CHAR => {
            let (name, used) = tri!(AmlName::decode(sub(buf, at, buf.len())));
            if name.segs().is_empty() {
                return Err(AmlError::EmptyName);
            }
            Ok(at + used)
        }
        PACKAGE_OP => {
            if package_depth >= MAX_NEST {
                return Err(AmlError::TooDeep);
            }
            let (start, end) = tri!(package_bounds(buf, at + 1));
            if start == end {
                return Err(AmlError::Truncated);
            }
            // Skip NumElements; the body contains the encoded initializers.
            let package = sub(buf, start + 1, end);
            let mut next = 0;
            while next < package.len() {
                next = tri!(skip_data_object(package, next, package_depth + 1));
            }
            Ok(end)
        }
        BUFFER_OP | VAR_PACKAGE_OP => match package_bounds(buf, at + 1) {
            Ok((_, end)) => Ok(end),
            Err(error) => Err(error),
        },
        other => Err(AmlError::UnsupportedOpcode(other)),
    }
}

/// The end of a fixed-size object of `len` bytes at `at`.
const fn fixed(buf: &[u8], at: usize, len: usize) -> Result<usize, AmlError> {
    let end = at + len;
    if end > buf.len() {
        Err(AmlError::Truncated)
    } else {
        Ok(end)
    }
}
