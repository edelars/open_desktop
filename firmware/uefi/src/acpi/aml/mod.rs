//! Own AML encoder (S7-12): a bounded, allocation-free writer for the AML subset the firmware
//! needs to describe the virtual board. It is an encoder only; there is no AML interpreter.
//!
//! Layers: [`opcodes`] (byte values with spec references), [`names`] (NameSeg / NameString and
//! the per-definition-block name registry), [`encoder`] (PkgLength, integers, strings, packages
//! into a fixed buffer), [`resource`] (ResourceTemplate descriptors), [`builder`] (Scope, Device,
//! Name, Method, Package, Buffer with duplicate-name tracking) and [`validate`] (structural walk
//! of finished AML for length bounds and duplicate names).

pub mod builder;
pub mod encoder;
pub mod names;
pub mod opcodes;
pub mod resource;
pub mod validate;

/// Reasons the encoder or validator rejects its input. One variant per check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AmlError {
    /// The output buffer is full.
    BufferFull,
    /// A package or buffer is larger than the 28-bit PkgLength range (ACPI 20.2.4).
    PkgLengthOverflow,
    /// A name segment is empty, longer than 4 characters or uses characters outside
    /// `[A-Z_][A-Z0-9_]*` (ACPI 20.2.2).
    BadNameSeg,
    /// A name has more segments than the encoder supports.
    NameTooLong,
    /// A `^` prefix climbs above the root.
    BadParentPrefix,
    /// An object with this absolute path already exists (ACPI 5.3.1 namespace rules).
    DuplicateName,
    /// A name reference points at an object that has not been defined.
    UnknownName,
    /// A name reference is not absolute; only absolute references are supported.
    RelativeReference,
    /// A named object was given an empty (NullName) name.
    EmptyName,
    /// Scope nesting deeper than the encoder supports.
    TooDeep,
    /// More named objects than the registry holds.
    TooManyNames,
    /// A package has more than 255 elements (NumElements is one byte, ACPI Table 20.2).
    PackageTooLarge,
    /// A string contains a byte outside `0x01..=0x7F` (ACPI 20.2.3 AsciiChar).
    BadString,
    /// A resource descriptor field is out of range or inconsistent.
    BadResource,
    /// A method argument count above 7 (MethodFlags bits 2..0, ACPI 20.2.5.2).
    BadMethodFlags,
    /// Validator: the byte stream ends inside an object.
    Truncated,
    /// Validator: a PkgLength is malformed or its package leaves the enclosing package.
    BadPkgLength,
    /// Validator: an opcode outside the supported subset.
    UnsupportedOpcode(u8),
}

#[cfg(test)]
mod tests;
