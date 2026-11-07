//! Structured AML construction: Scope, Device, Name, Method, Package, Buffer and the EISA ID
//! helper, on top of [`Encoder`]. Every named object is registered by absolute path, so a
//! duplicate in one scope is rejected while the AML is built.
//!
//! Spec: ACPI 20.2.5.1 (DefName, DefScope), 20.2.5.2 (DefMethod, DefDevice), 20.2.3 (data
//! objects), 19.3.4 (EISAID algorithm). Bodies are closures so that PkgLength can be patched
//! when the package closes.

use super::AmlError;
use super::encoder::{Encoder, PkgMark};
use super::names::{AbsPath, AmlName, Registry};
use super::opcodes::{
    BUFFER_OP, DEVICE_OP_SUFFIX, EXT_OP_PREFIX, METHOD_OP, NAME_OP, PACKAGE_OP, RETURN_OP, SCOPE_OP,
};
use super::resource::ResourceTemplate;

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

impl<'a> AmlBuilder<'a> {
    pub fn new(buf: &'a mut [u8]) -> Self {
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
    pub fn bytes(&self) -> &[u8] {
        self.enc.bytes()
    }

    pub fn len(&self) -> usize {
        self.enc.len()
    }

    pub fn is_empty(&self) -> bool {
        self.enc.is_empty()
    }

    /// `Scope(name) { body }`: ScopeOp PkgLength NameString TermList. Does not register a name.
    pub fn scope(
        &mut self,
        name: &str,
        body: impl FnOnce(&mut Self) -> Result<(), AmlError>,
    ) -> Result<(), AmlError> {
        let name = AmlName::parse(name)?;
        let path = self.scope.resolve(&name)?;
        self.enc.put(SCOPE_OP)?;
        self.container(&name, path, body)
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
        let path = self.declare(name)?;
        self.enc.put(EXT_OP_PREFIX)?;
        self.enc.put(DEVICE_OP_SUFFIX)?;
        self.container(name, path, body)
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
        if arg_count > 7 {
            return Err(AmlError::BadMethodFlags);
        }
        let name = AmlName::parse(name)?;
        self.declare(&name)?;
        self.enc.put(METHOD_OP)?;
        let mark = self.enc.begin_package()?;
        self.enc.name(&name)?;
        self.enc.put(arg_count | (u8::from(serialized) << 3))?;
        self.nested(body)?;
        self.enc.end_package(mark)
    }

    /// `Return(integer)`: ReturnOp followed by the integer constant (ReturnOp is NOT VERIFIED,
    /// see `opcodes::RETURN_OP`).
    pub fn return_integer(&mut self, value: u64) -> Result<(), AmlError> {
        self.enc.put(RETURN_OP)?;
        self.enc.integer(value)
    }

    /// `Name(name, <value>)`: NameOp NameString DataRefObject; `value` writes the object.
    pub fn name(
        &mut self,
        name: &str,
        value: impl FnOnce(&mut Self) -> Result<(), AmlError>,
    ) -> Result<(), AmlError> {
        let name = AmlName::parse(name)?;
        self.declare(&name)?;
        self.enc.put(NAME_OP)?;
        self.enc.name(&name)?;
        value(self)
    }

    pub fn name_integer(&mut self, name: &str, value: u64) -> Result<(), AmlError> {
        self.name(name, |b| b.integer(value))
    }

    pub fn name_string(&mut self, name: &str, text: &str) -> Result<(), AmlError> {
        self.name(name, |b| b.string(text))
    }

    /// `Name(name, EISAID("..."))`, e.g. `_HID` / `_CID`.
    pub fn name_eisa_id(&mut self, name: &str, id: &str) -> Result<(), AmlError> {
        let value = eisa_id(id)?;
        self.name(name, |b| b.integer(u64::from(value)))
    }

    /// `Name(name, ResourceTemplate() {...})`: a Buffer holding the descriptors.
    pub fn name_resource_template(
        &mut self,
        name: &str,
        template: &ResourceTemplate,
    ) -> Result<(), AmlError> {
        self.name(name, |b| b.buffer(template.bytes()))
    }

    /// Integer constant as a data object (counts as one element inside a Package).
    pub fn integer(&mut self, value: u64) -> Result<(), AmlError> {
        self.count_element()?;
        self.enc.integer(value)
    }

    /// String constant as a data object.
    pub fn string(&mut self, text: &str) -> Result<(), AmlError> {
        self.count_element()?;
        self.enc.string(text)
    }

    /// `Buffer(len) {bytes}`: BufferOp PkgLength BufferSize(integer) ByteList.
    pub fn buffer(&mut self, bytes: &[u8]) -> Result<(), AmlError> {
        self.count_element()?;
        self.enc.put(BUFFER_OP)?;
        let mark = self.enc.begin_package()?;
        self.enc.integer(bytes.len() as u64)?;
        self.enc.put_all(bytes)?;
        self.enc.end_package(mark)
    }

    /// Reference to an already defined object by absolute name (a Package element, e.g. the
    /// `Source` field of a `_PRT` entry). Relative references are not supported.
    pub fn name_reference(&mut self, name: &str) -> Result<(), AmlError> {
        let name = AmlName::parse(name)?;
        if !name.absolute {
            return Err(AmlError::RelativeReference);
        }
        let path = AbsPath::ROOT.resolve(&name)?;
        if !self.registry.contains(&path) {
            return Err(AmlError::UnknownName);
        }
        self.count_element()?;
        self.enc.name(&name)
    }

    /// `Package() { elements }`: PackageOp PkgLength NumElements elements. `NumElements` is the
    /// number of elements `body` emits (ByteData, at most 255).
    pub fn package(
        &mut self,
        body: impl FnOnce(&mut Self) -> Result<(), AmlError>,
    ) -> Result<(), AmlError> {
        self.count_element()?;
        if self.pkg_depth == MAX_NEST {
            return Err(AmlError::TooDeep);
        }
        self.enc.put(PACKAGE_OP)?;
        let mark = self.enc.begin_package()?;
        let count_at = self.enc.len();
        self.enc.put(0)?;
        self.pkg_counts[self.pkg_depth] = 0;
        self.pkg_depth += 1;
        let result = body(self);
        self.pkg_depth -= 1;
        result?;
        let count =
            u8::try_from(self.pkg_counts[self.pkg_depth]).map_err(|_| AmlError::PackageTooLarge)?;
        self.enc.patch(count_at, count);
        self.enc.end_package(mark)
    }

    /// `Name(name, Package() { elements })`.
    pub fn name_package(
        &mut self,
        name: &str,
        body: impl FnOnce(&mut Self) -> Result<(), AmlError>,
    ) -> Result<(), AmlError> {
        self.name(name, |b| b.package(body))
    }

    fn count_element(&mut self) -> Result<(), AmlError> {
        if self.pkg_depth > 0 {
            let counter = &mut self.pkg_counts[self.pkg_depth - 1];
            *counter = counter.checked_add(1).ok_or(AmlError::PackageTooLarge)?;
        }
        Ok(())
    }

    /// Registers a declared object and returns its absolute path.
    fn declare(&mut self, name: &AmlName) -> Result<AbsPath, AmlError> {
        if name.segs().is_empty() {
            return Err(AmlError::EmptyName);
        }
        let path = self.scope.resolve(name)?;
        self.registry.insert(path)?;
        Ok(path)
    }

    /// Shared tail of Scope and Device: PkgLength, NameString, TermList in the new scope.
    fn container(
        &mut self,
        name: &AmlName,
        path: AbsPath,
        body: impl FnOnce(&mut Self) -> Result<(), AmlError>,
    ) -> Result<(), AmlError> {
        let mark: PkgMark = self.enc.begin_package()?;
        self.enc.name(name)?;
        let outer = self.scope;
        self.scope = path;
        let result = self.nested(body);
        self.scope = outer;
        result?;
        self.enc.end_package(mark)
    }

    fn nested(
        &mut self,
        body: impl FnOnce(&mut Self) -> Result<(), AmlError>,
    ) -> Result<(), AmlError> {
        if self.depth == MAX_NEST {
            return Err(AmlError::TooDeep);
        }
        self.depth += 1;
        let result = body(self);
        self.depth -= 1;
        result
    }
}

/// EISAID algorithm (ACPI 19.3.4): a seven-character `UUUNNNN` string becomes a DWORD. The
/// three letters give 5 bits each (`char - 0x40`) packed into bytes 0..1 (big-endian bit order:
/// byte 0 bit 6..2 = first letter, bit 1..0 + byte 1 bit 7..5 = second, byte 1 bit 4..0 =
/// third); the four hex digits fill bytes 2 and 3 high nibble first.
pub fn eisa_id(text: &str) -> Result<u32, AmlError> {
    let raw = text.as_bytes();
    if raw.len() != 7 {
        return Err(AmlError::BadString);
    }
    let mut letters = [0u8; 3];
    for (slot, &c) in letters.iter_mut().zip(&raw[..3]) {
        if !c.is_ascii_uppercase() {
            return Err(AmlError::BadString);
        }
        *slot = c - 0x40;
    }
    let mut digits = [0u8; 4];
    for (slot, &c) in digits.iter_mut().zip(&raw[3..]) {
        *slot = match c {
            b'0'..=b'9' => c - b'0',
            b'A'..=b'F' => c - b'A' + 10,
            _ => return Err(AmlError::BadString),
        };
    }
    let byte0 = (letters[0] << 2) | (letters[1] >> 3);
    let byte1 = ((letters[1] & 0x07) << 5) | letters[2];
    let byte2 = (digits[0] << 4) | digits[1];
    let byte3 = (digits[2] << 4) | digits[3];
    Ok(u32::from_le_bytes([byte0, byte1, byte2, byte3]))
}
