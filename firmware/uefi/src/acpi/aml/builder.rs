//! Structured AML construction: Scope, Device, Name, Method, Package, Buffer and the EISA ID
//! helper, on top of [`Encoder`]. Every named object is registered by absolute path, so a
//! duplicate in one scope is rejected while the AML is built.
//!
//! Spec: ACPI 20.2.5.1 (DefName, DefScope), 20.2.5.2 (DefMethod, DefDevice), 20.2.3 (data
//! objects), 19.3.4 (EISAID algorithm).
//!
//! Containers come in two forms over one implementation. The closure form (`scope`, `device`,
//! `method`, `package`, `name`, ...) takes a body closure and is a plain `fn`. The explicit form
//! (`begin_scope` / `begin_device` / `begin_method` / `begin_package` ... followed by `end` /
//! `end_package`) is a `const fn`, because closures cannot be called in a const context (S4-31b0).
//! The closure form is the explicit form wrapped around the body call; both write identical bytes.

use super::AmlError;
use super::encoder::{Encoder, PkgMark};
use super::names::{AbsPath, AmlName, Registry};
use super::opcodes::{
    BUFFER_OP, DEVICE_OP_SUFFIX, EXT_OP_PREFIX, METHOD_OP, NAME_OP, PACKAGE_OP, RETURN_OP, SCOPE_OP,
};
use super::resource::ResourceTemplate;
use crate::acpi::bytes::tri;

/// Deepest nesting of Scope/Device/Method/Package the builder keeps state for.
pub const MAX_NEST: usize = 8;

pub struct AmlBuilder<'a> {
    enc: Encoder<'a>,
    scope: AbsPath,
    registry: Registry,
    depth: usize,
    /// Element counters of the open packages (`NumElements`).
    pkg_counts: [u16; MAX_NEST],
    pkg_depth: usize,
}

/// An open Scope, Device or Method: returned by `begin_*`, consumed by [`AmlBuilder::end`].
///
/// Neither `Clone` nor `Copy`: the token is consumed by exactly one `end`. Containers must be
/// closed in LIFO order (the innermost open one first), as the closure form does.
#[derive(Debug)]
pub struct Open {
    mark: PkgMark,
    /// The scope to return to when the container closes.
    outer: AbsPath,
}

/// An open Package: returned by [`AmlBuilder::begin_package`], consumed by
/// [`AmlBuilder::end_package`].
///
/// Neither `Clone` nor `Copy`: the token is consumed by exactly one `end_package`. Packages must
/// be closed in LIFO order (the innermost open one first).
#[derive(Debug)]
pub struct OpenPackage {
    mark: PkgMark,
    /// Offset of the `NumElements` byte.
    count_at: usize,
}

impl<'a> AmlBuilder<'a> {
    pub const fn new(buf: &'a mut [u8]) -> Self {
        AmlBuilder {
            enc: Encoder::new(buf),
            scope: AbsPath::ROOT,
            registry: Registry::new(),
            depth: 0,
            pkg_counts: [0; MAX_NEST],
            pkg_depth: 0,
        }
    }

    /// The AML written so far.
    pub const fn bytes(&self) -> &[u8] {
        self.enc.bytes()
    }

    pub const fn len(&self) -> usize {
        self.enc.len()
    }

    pub const fn is_empty(&self) -> bool {
        self.enc.is_empty()
    }

    /// `Scope(name) { body }`: ScopeOp PkgLength NameString TermList. Does not register a name.
    pub fn scope(
        &mut self,
        name: &str,
        body: impl FnOnce(&mut Self) -> Result<(), AmlError>,
    ) -> Result<(), AmlError> {
        let open = self.begin_scope(name)?;
        let result = body(self);
        self.end_with(open, result)
    }

    /// Opens a Scope; close it with [`end`](Self::end).
    pub const fn begin_scope(&mut self, name: &str) -> Result<Open, AmlError> {
        let name = tri!(AmlName::parse(name));
        let path = tri!(self.scope.resolve(&name));
        tri!(self.enc.put(SCOPE_OP));
        self.begin_container(&name, path)
    }

    /// `Device(name) { body }`: ExtOpPrefix DeviceOp PkgLength NameString TermList.
    pub fn device(
        &mut self,
        name: &str,
        body: impl FnOnce(&mut Self) -> Result<(), AmlError>,
    ) -> Result<(), AmlError> {
        let name = AmlName::parse(name)?;
        self.device_at(&name, body)
    }

    /// Like [`device`](Self::device) with an already parsed name.
    pub fn device_at(
        &mut self,
        name: &AmlName,
        body: impl FnOnce(&mut Self) -> Result<(), AmlError>,
    ) -> Result<(), AmlError> {
        let open = self.begin_device_at(name)?;
        let result = body(self);
        self.end_with(open, result)
    }

    /// Opens a Device; close it with [`end`](Self::end).
    pub const fn begin_device(&mut self, name: &str) -> Result<Open, AmlError> {
        let name = tri!(AmlName::parse(name));
        self.begin_device_at(&name)
    }

    /// Like [`begin_device`](Self::begin_device) with an already parsed name.
    pub const fn begin_device_at(&mut self, name: &AmlName) -> Result<Open, AmlError> {
        let path = tri!(self.declare(name));
        tri!(self.enc.put(EXT_OP_PREFIX));
        tri!(self.enc.put(DEVICE_OP_SUFFIX));
        self.begin_container(name, path)
    }

    /// `Method(name, args, NotSerialized/Serialized) { body }`: MethodOp PkgLength NameString
    /// MethodFlags TermList; flags bits 2..0 = ArgCount, bit 3 = Serialize, bits 7..4 =
    /// SyncLevel (written 0).
    pub fn method(
        &mut self,
        name: &str,
        arg_count: u8,
        serialized: bool,
        body: impl FnOnce(&mut Self) -> Result<(), AmlError>,
    ) -> Result<(), AmlError> {
        let open = self.begin_method(name, arg_count, serialized)?;
        let result = body(self);
        self.end_with(open, result)
    }

    /// Opens a Method; close it with [`end`](Self::end).
    pub const fn begin_method(
        &mut self,
        name: &str,
        arg_count: u8,
        serialized: bool,
    ) -> Result<Open, AmlError> {
        if arg_count > 7 {
            return Err(AmlError::BadMethodFlags);
        }
        let name = tri!(AmlName::parse(name));
        tri!(self.declare(&name));
        tri!(self.enc.put(METHOD_OP));
        let mark = tri!(self.enc.begin_package());
        tri!(self.enc.name(&name));
        tri!(self.enc.put(arg_count | ((serialized as u8) << 3)));
        tri!(self.enter());
        Ok(Open {
            mark,
            outer: self.scope,
        })
    }

    /// Closes the Scope, Device or Method opened by the matching `begin_*`.
    pub const fn end(&mut self, open: Open) -> Result<(), AmlError> {
        self.end_with(open, Ok(()))
    }

    /// `Return(integer)`: ReturnOp followed by the integer constant (ReturnOp is NOT VERIFIED,
    /// see `opcodes::RETURN_OP`).
    pub const fn return_integer(&mut self, value: u64) -> Result<(), AmlError> {
        tri!(self.enc.put(RETURN_OP));
        self.enc.integer(value)
    }

    /// `Name(name, <value>)`: NameOp NameString DataRefObject; `value` writes the object.
    pub fn name(
        &mut self,
        name: &str,
        value: impl FnOnce(&mut Self) -> Result<(), AmlError>,
    ) -> Result<(), AmlError> {
        self.begin_name(name)?;
        value(self)
    }

    /// Writes `NameOp NameString` for a new object; the caller writes the DataRefObject next.
    pub const fn begin_name(&mut self, name: &str) -> Result<(), AmlError> {
        let name = tri!(AmlName::parse(name));
        tri!(self.declare(&name));
        tri!(self.enc.put(NAME_OP));
        self.enc.name(&name)
    }

    pub const fn name_integer(&mut self, name: &str, value: u64) -> Result<(), AmlError> {
        tri!(self.begin_name(name));
        self.integer(value)
    }

    pub const fn name_string(&mut self, name: &str, text: &str) -> Result<(), AmlError> {
        tri!(self.begin_name(name));
        self.string(text)
    }

    /// `Name(name, EISAID("..."))`, e.g. `_HID` / `_CID`.
    pub const fn name_eisa_id(&mut self, name: &str, id: &str) -> Result<(), AmlError> {
        let value = tri!(eisa_id(id));
        tri!(self.begin_name(name));
        self.integer(value as u64)
    }

    /// `Name(name, ResourceTemplate() {...})`: a Buffer holding the descriptors.
    pub const fn name_resource_template(
        &mut self,
        name: &str,
        template: &ResourceTemplate,
    ) -> Result<(), AmlError> {
        tri!(self.begin_name(name));
        self.buffer(template.bytes())
    }

    /// Integer constant as a data object (counts as one element inside a Package).
    pub const fn integer(&mut self, value: u64) -> Result<(), AmlError> {
        tri!(self.count_element());
        self.enc.integer(value)
    }

    /// String constant as a data object.
    pub const fn string(&mut self, text: &str) -> Result<(), AmlError> {
        tri!(self.count_element());
        self.enc.string(text)
    }

    /// `Buffer(len) {bytes}`: BufferOp PkgLength BufferSize(integer) ByteList.
    pub const fn buffer(&mut self, bytes: &[u8]) -> Result<(), AmlError> {
        tri!(self.count_element());
        tri!(self.enc.put(BUFFER_OP));
        let mark = tri!(self.enc.begin_package());
        tri!(self.enc.integer(bytes.len() as u64));
        tri!(self.enc.put_all(bytes));
        self.enc.end_package(mark)
    }

    /// Reference to an already defined object by absolute name (a Package element, e.g. the
    /// `Source` field of a `_PRT` entry). Relative references are not supported.
    pub const fn name_reference(&mut self, name: &str) -> Result<(), AmlError> {
        let name = tri!(AmlName::parse(name));
        if !name.absolute {
            return Err(AmlError::RelativeReference);
        }
        let path = tri!(AbsPath::ROOT.resolve(&name));
        if !self.registry.contains(&path) {
            return Err(AmlError::UnknownName);
        }
        tri!(self.count_element());
        self.enc.name(&name)
    }

    /// `Package() { elements }`: PackageOp PkgLength NumElements elements. `NumElements` is the
    /// number of elements `body` emits (ByteData, at most 255).
    pub fn package(
        &mut self,
        body: impl FnOnce(&mut Self) -> Result<(), AmlError>,
    ) -> Result<(), AmlError> {
        let open = self.begin_package()?;
        let result = body(self);
        self.end_package_with(open, result)
    }

    /// Opens a Package; the elements follow, then [`end_package`](Self::end_package) closes it.
    pub const fn begin_package(&mut self) -> Result<OpenPackage, AmlError> {
        tri!(self.count_element());
        if self.pkg_depth == MAX_NEST {
            return Err(AmlError::TooDeep);
        }
        tri!(self.enc.put(PACKAGE_OP));
        let mark = tri!(self.enc.begin_package());
        let count_at = self.enc.len();
        tri!(self.enc.put(0));
        self.pkg_counts[self.pkg_depth] = 0;
        self.pkg_depth += 1;
        Ok(OpenPackage { mark, count_at })
    }

    /// Closes the Package opened by [`begin_package`](Self::begin_package).
    pub const fn end_package(&mut self, open: OpenPackage) -> Result<(), AmlError> {
        self.end_package_with(open, Ok(()))
    }

    /// `Name(name, Package() { elements })`.
    pub fn name_package(
        &mut self,
        name: &str,
        body: impl FnOnce(&mut Self) -> Result<(), AmlError>,
    ) -> Result<(), AmlError> {
        self.name(name, |b| b.package(body))
    }

    const fn end_package_with(
        &mut self,
        open: OpenPackage,
        result: Result<(), AmlError>,
    ) -> Result<(), AmlError> {
        self.pkg_depth -= 1;
        tri!(result);
        let counted = self.pkg_counts[self.pkg_depth];
        if counted > 0xFF {
            return Err(AmlError::PackageTooLarge);
        }
        self.enc.patch(open.count_at, counted as u8);
        self.enc.end_package(open.mark)
    }

    const fn count_element(&mut self) -> Result<(), AmlError> {
        if self.pkg_depth > 0 {
            let counter = &mut self.pkg_counts[self.pkg_depth - 1];
            *counter = match counter.checked_add(1) {
                Some(count) => count,
                None => return Err(AmlError::PackageTooLarge),
            };
        }
        Ok(())
    }

    /// Registers a declared object and returns its absolute path.
    const fn declare(&mut self, name: &AmlName) -> Result<AbsPath, AmlError> {
        if name.segs().is_empty() {
            return Err(AmlError::EmptyName);
        }
        let path = tri!(self.scope.resolve(name));
        tri!(self.registry.insert(path));
        Ok(path)
    }

    /// Shared head of Scope and Device: PkgLength, NameString, then enter the new scope.
    const fn begin_container(&mut self, name: &AmlName, path: AbsPath) -> Result<Open, AmlError> {
        let mark = tri!(self.enc.begin_package());
        tri!(self.enc.name(name));
        let outer = self.scope;
        self.scope = path;
        if let Err(error) = self.enter() {
            self.scope = outer;
            return Err(error);
        }
        Ok(Open { mark, outer })
    }

    /// Counts one more level of nesting; `TooDeep` past `MAX_NEST`.
    const fn enter(&mut self) -> Result<(), AmlError> {
        if self.depth == MAX_NEST {
            return Err(AmlError::TooDeep);
        }
        self.depth += 1;
        Ok(())
    }

    /// Leaves the container (nesting and scope are restored even when the body failed), then
    /// writes its PkgLength if the body succeeded.
    const fn end_with(&mut self, open: Open, result: Result<(), AmlError>) -> Result<(), AmlError> {
        self.depth -= 1;
        self.scope = open.outer;
        tri!(result);
        self.enc.end_package(open.mark)
    }
}

/// EISAID algorithm (ACPI 19.3.4): a seven-character `UUUNNNN` string becomes a DWORD. The
/// three letters give 5 bits each (`char - 0x40`) packed into bytes 0..1 (big-endian bit order:
/// byte 0 bit 6..2 = first letter, bit 1..0 + byte 1 bit 7..5 = second, byte 1 bit 4..0 =
/// third); the four hex digits fill bytes 2 and 3 high nibble first.
pub const fn eisa_id(text: &str) -> Result<u32, AmlError> {
    let raw = text.as_bytes();
    if raw.len() != 7 {
        return Err(AmlError::BadString);
    }
    let mut letters = [0u8; 3];
    let mut i = 0;
    while i < 3 {
        if !raw[i].is_ascii_uppercase() {
            return Err(AmlError::BadString);
        }
        letters[i] = raw[i] - 0x40;
        i += 1;
    }
    let mut digits = [0u8; 4];
    let mut i = 0;
    while i < 4 {
        let c = raw[3 + i];
        digits[i] = match c {
            b'0'..=b'9' => c - b'0',
            b'A'..=b'F' => c - b'A' + 10,
            _ => return Err(AmlError::BadString),
        };
        i += 1;
    }
    let byte0 = (letters[0] << 2) | (letters[1] >> 3);
    let byte1 = ((letters[1] & 0x07) << 5) | letters[2];
    let byte2 = (digits[0] << 4) | digits[1];
    let byte3 = (digits[2] << 4) | digits[3];
    Ok(u32::from_le_bytes([byte0, byte1, byte2, byte3]))
}
