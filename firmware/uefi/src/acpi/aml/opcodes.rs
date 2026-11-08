//! AML opcode, prefix and resource-descriptor byte values.
//!
//! Source for every value below: ACPI Specification, chapter 20 "ACPI Machine Language (AML)
//! Specification" (grammar 20.2.x and Table 20.2 "AML Byte Stream Byte Values") and chapter 6.4
//! "Resource Data Types for ACPI". The text was read from the ACPI **6.4** HTML edition served
//! through Context7 (`/websites/uefi_htmlspecs_acpi_spec_6_4_html`): the 6.6 pages on uefi.org
//! answered HTTP 403 on 2026-11-07. AML encodings are unchanged between the two editions as far
//! as the cited tables go, but that equality is **not verified** against the 6.6 text.
//!
//! Values that could not be read from the specification text carry `NOT VERIFIED` in their
//! comment; see `docs/specs/ACPI_SMBIOS_TABLES.md`, section "AML and DSDT".

// --- Data objects, 20.2.3 and Table 20.2 -------------------------------------------------------
pub const ZERO_OP: u8 = 0x00; // ZeroOp, 20.2.3
pub const ONE_OP: u8 = 0x01; // OneOp, 20.2.3
pub const ONES_OP: u8 = 0xFF; // OnesOp, 20.2.3
pub const BYTE_PREFIX: u8 = 0x0A; // BytePrefix, 20.2.3
pub const WORD_PREFIX: u8 = 0x0B; // WordPrefix, 20.2.3
pub const DWORD_PREFIX: u8 = 0x0C; // DWordPrefix, 20.2.3
pub const STRING_PREFIX: u8 = 0x0D; // StringPrefix, 20.2.3
pub const QWORD_PREFIX: u8 = 0x0E; // QWordPrefix, 20.2.3

// --- Name objects, 20.2.2 ----------------------------------------------------------------------
pub const ROOT_CHAR: u8 = 0x5C; // RootChar '\', 20.2.2
pub const PARENT_PREFIX_CHAR: u8 = 0x5E; // ParentPrefixChar '^', 20.2.2
pub const DUAL_NAME_PREFIX: u8 = 0x2E; // DualNamePrefix, 20.2.2
pub const MULTI_NAME_PREFIX: u8 = 0x2F; // MultiNamePrefix, 20.2.2
pub const NULL_NAME: u8 = 0x00; // NullName, 20.2.2

// --- Term objects, 20.2.5 and Table 20.2 -------------------------------------------------------
pub const NAME_OP: u8 = 0x08; // DefName := NameOp NameString DataRefObject, 20.2.5.1
pub const SCOPE_OP: u8 = 0x10; // DefScope := ScopeOp PkgLength NameString TermList, 20.2.5.1
pub const BUFFER_OP: u8 = 0x11; // BufferOp, Table 20.2 (fixed arg TermArg, then ByteList)
pub const PACKAGE_OP: u8 = 0x12; // PackageOp, Table 20.2 (fixed arg ByteData, then elements)
pub const VAR_PACKAGE_OP: u8 = 0x13; // VarPackageOp, Table 20.2
pub const METHOD_OP: u8 = 0x14; // DefMethod := MethodOp PkgLength NameString MethodFlags TermList, 20.2.5.2
pub const EXT_OP_PREFIX: u8 = 0x5B; // ExtOpPrefix, 20.2.3
pub const DEVICE_OP_SUFFIX: u8 = 0x82; // DeviceOp := ExtOpPrefix 0x82, 20.2.5.2
/// ReturnOp. NOT VERIFIED: the part of Table 20.2 above 0x74 was not returned by the source.
pub const RETURN_OP: u8 = 0xA4;

// --- PkgLength, 20.2.4 -------------------------------------------------------------------------
/// Largest value a one-byte PkgLength encodes (bits 5..0), 20.2.4 and 5.4.1.
pub const PKG_LEN_MAX_1: usize = 0x3F;
/// Largest value of a two-byte PkgLength (4 + 8 bits), 5.4.1.
pub const PKG_LEN_MAX_2: usize = 0x0FFF;
/// Largest value of a three-byte PkgLength (4 + 16 bits), 5.4.1.
pub const PKG_LEN_MAX_3: usize = 0x000F_FFFF;
/// Largest value of a four-byte PkgLength (4 + 24 bits), 20.2.4 ("2^28" range).
pub const PKG_LEN_MAX_4: usize = 0x0FFF_FFFF;

// --- Resource descriptors, 6.4.2 (small items) and 6.4.3 (large items) --------------------------
/// IRQ descriptor: type 0, small item name 0x4, length 2 or 3 -> 0x22 / 0x23 (Table 6.28).
pub const RES_IRQ_NO_INFO: u8 = 0x22;
pub const RES_IRQ_WITH_INFO: u8 = 0x23;
/// I/O Port descriptor: small item name 0x8, length 7 -> 0x47 (Table 6.33).
pub const RES_IO_PORT: u8 = 0x47;
/// End Tag: small item name 0xF, length 1 -> 0x79 (Table 6.37).
pub const RES_END_TAG: u8 = 0x79;
/// 32-bit Fixed Memory Range: large item name 0x06 -> 0x86, length 9 (Table 6.43).
pub const RES_MEMORY32_FIXED: u8 = 0x86;
/// DWord Address Space: large item name 0x07 -> 0x87, length 23 (Table 6.46).
pub const RES_DWORD_SPACE: u8 = 0x87;
/// Word Address Space: large item name 0x08 -> 0x88, length 13 (Table 6.47).
pub const RES_WORD_SPACE: u8 = 0x88;
/// Extended Interrupt: large item name 0x09 -> 0x89, length 2 + 4 * n for n interrupts (Table 6.52).
pub const RES_EXTENDED_INTERRUPT: u8 = 0x89;
/// QWord Address Space: large item name 0x0A -> 0x8A, length 43. NOT VERIFIED directly: the
/// descriptor table (Table 6.45) was not returned by the source; 43 = 3 + 5 * 8 is derived by
/// analogy with the verified Word (13 = 3 + 5 * 2) and DWord (23 = 3 + 5 * 4) layouts.
pub const RES_QWORD_SPACE: u8 = 0x8A;

/// Address space descriptor resource types, Tables 6.46/6.47 "Resource Type".
pub const SPACE_TYPE_MEMORY: u8 = 0;
pub const SPACE_TYPE_IO: u8 = 1;
pub const SPACE_TYPE_BUS_NUMBER: u8 = 2;
