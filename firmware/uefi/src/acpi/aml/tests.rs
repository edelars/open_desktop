//! Reference constructs for the AML encoder (S7-12).
//!
//! Every expected byte string below is computed by hand from the ACPI specification (chapter 20
//! AML grammar, chapter 6.4 resource data types, 19.3.4 EISAID) and written out as hex with the
//! derivation in a comment; none of it is captured from another compiler. Spec text read from
//! the ACPI 6.4 edition (6.6 returned HTTP 403). `ReturnOp` (0xA4) is NOT VERIFIED.

use super::AmlError;
use crate::acpi::aml::builder::{AmlBuilder, eisa_id};
use crate::acpi::aml::encoder::{Encoder, decode_pkg_length, pkg_length_for_body};
use crate::acpi::aml::names::AmlName;
use crate::acpi::aml::resource::{
    AddressRange, InterruptFlags, MemoryFlags, ResourceTemplate, SpaceFlags,
};
use crate::acpi::aml::validate::validate_definition_block;

fn hex(text: &str) -> Vec<u8> {
    text.split_whitespace()
        .map(|b| u8::from_str_radix(b, 16).unwrap())
        .collect()
}

/// Encodes with the closure and returns the produced bytes.
fn build(f: impl FnOnce(&mut AmlBuilder<'_>) -> Result<(), AmlError>) -> Vec<u8> {
    let mut storage = [0u8; 512];
    let mut b = AmlBuilder::new(&mut storage);
    f(&mut b).unwrap();
    b.bytes().to_vec()
}

fn encode(f: impl FnOnce(&mut Encoder<'_>) -> Result<(), AmlError>) -> Vec<u8> {
    let mut storage = [0u8; 64];
    let mut e = Encoder::new(&mut storage);
    f(&mut e).unwrap();
    e.bytes().to_vec()
}

fn pkg(body_len: usize) -> Vec<u8> {
    let (bytes, n) = pkg_length_for_body(body_len).unwrap();
    bytes[..n].to_vec()
}

// --- PkgLength (ACPI 20.2.4) -------------------------------------------------------------------

#[test]
fn pkg_length_one_byte() {
    // По спецификации, вычислено вручную: value = body + 1 byte; value <= 0x3F -> one byte.
    assert_eq!(pkg(0), hex("01"));
    assert_eq!(pkg(62), hex("3F")); // 62 + 1 = 63 = 0x3F, the largest one-byte value
}

#[test]
fn pkg_length_two_bytes() {
    // По спецификации, вычислено вручную: body 63 -> 63 + 1 = 64 > 0x3F, so two bytes:
    // value = 63 + 2 = 65 = 0x41; lead = (1 << 6) | (0x41 & 0xF) = 0x41; next = 0x41 >> 4 = 0x04.
    assert_eq!(pkg(63), hex("41 04"));
    // body 0xFFD -> value 0xFFF (largest two-byte value): lead 0x40 | 0xF, next 0xFF.
    assert_eq!(pkg(0x0FFD), hex("4F FF"));
}

#[test]
fn pkg_length_three_bytes() {
    // По спецификации, вычислено вручную: body 0xFFE -> two bytes would give 0x1000 > 0xFFF, so
    // three: value = 0xFFE + 3 = 0x1001; lead = (2 << 6) | 1 = 0x81; next = 0x1001 >> 4 & 0xFF =
    // 0x00; last = 0x1001 >> 12 = 0x01.
    assert_eq!(pkg(0x0FFE), hex("81 00 01"));
    // body 0xFFFFC -> value 0xFFFFF (largest three-byte value).
    assert_eq!(pkg(0x000F_FFFC), hex("8F FF FF"));
}

#[test]
fn pkg_length_four_bytes() {
    // По спецификации, вычислено вручную: body 0xFFFFD -> value 0x100001 needs four bytes:
    // lead = (3 << 6) | 1 = 0xC1; then 0x00, 0x00, 0x01.
    assert_eq!(pkg(0x000F_FFFD), hex("C1 00 00 01"));
    // body 0x0FFF_FFFB -> value 0x0FFF_FFFF, the largest encodable PkgLength.
    assert_eq!(pkg(0x0FFF_FFFB), hex("CF FF FF FF"));
}

#[test]
fn pkg_length_round_trips_through_decoder() {
    for body in [
        0usize,
        1,
        61,
        62,
        63,
        64,
        0xFFD,
        0xFFE,
        0xFFFFC,
        0xFFFFD,
        0x0FFF_FFFB,
    ] {
        let (bytes, n) = pkg_length_for_body(body).unwrap();
        let (value, used) = decode_pkg_length(&bytes[..n]).unwrap();
        assert_eq!((value, used), (body + n, n), "body {body:#x}");
    }
}

// --- NameString (ACPI 20.2.2) ------------------------------------------------------------------

fn name_bytes(text: &str) -> Vec<u8> {
    let name = AmlName::parse(text).unwrap();
    encode(|e| e.name(&name))
}

#[test]
fn name_single_segment_is_padded_with_underscores() {
    // "_SB" -> NameSeg `_SB_`; with RootChar: 5C 5F 53 42 5F.
    assert_eq!(name_bytes("\\_SB"), hex("5C 5F 53 42 5F"));
    assert_eq!(name_bytes("_HID"), hex("5F 48 49 44"));
}

#[test]
fn name_dual_path() {
    // `\_SB.PCI0`: RootChar, DualNamePrefix 0x2E, two NameSegs.
    assert_eq!(
        name_bytes("\\_SB.PCI0"),
        hex("5C 2E 5F 53 42 5F 50 43 49 30")
    );
}

#[test]
fn name_multi_path() {
    // `\_SB.PCI0.COM1`: RootChar, MultiNamePrefix 0x2F, SegCount 3, three NameSegs.
    assert_eq!(
        name_bytes("\\_SB.PCI0.COM1"),
        hex("5C 2F 03 5F 53 42 5F 50 43 49 30 43 4F 4D 31")
    );
}

#[test]
fn name_parent_prefixes_and_root_alone() {
    // `^^FOO`: two ParentPrefixChars then `FOO_`. `\` alone: RootChar then NullName 0x00.
    assert_eq!(name_bytes("^^FOO"), hex("5E 5E 46 4F 4F 5F"));
    assert_eq!(name_bytes("\\"), hex("5C 00"));
}

// --- Data objects (ACPI 20.2.3) ----------------------------------------------------------------

fn int_bytes(value: u64) -> Vec<u8> {
    encode(|e| e.integer(value))
}

#[test]
fn integer_constants_use_the_smallest_form() {
    // По спецификации, вычислено вручную: ZeroOp 00, OneOp 01, BytePrefix 0A, WordPrefix 0B,
    // DWordPrefix 0C, QWordPrefix 0E, little-endian data.
    assert_eq!(int_bytes(0), hex("00"));
    assert_eq!(int_bytes(1), hex("01"));
    assert_eq!(int_bytes(0x2A), hex("0A 2A"));
    assert_eq!(int_bytes(0xFF), hex("0A FF"));
    assert_eq!(int_bytes(0x100), hex("0B 00 01"));
    assert_eq!(int_bytes(0x1234), hex("0B 34 12"));
    assert_eq!(int_bytes(0x1_0000), hex("0C 00 00 01 00"));
    assert_eq!(int_bytes(0x1234_5678), hex("0C 78 56 34 12"));
    assert_eq!(int_bytes(0xFFFF_FFFF), hex("0C FF FF FF FF"));
    assert_eq!(int_bytes(0x1_0000_0000), hex("0E 00 00 00 00 01 00 00 00"));
}

#[test]
fn string_is_prefixed_and_nul_terminated() {
    // StringPrefix 0D, "ABC" = 41 42 43, NullChar 00.
    assert_eq!(encode(|e| e.string("ABC")), hex("0D 41 42 43 00"));
}

#[test]
fn eisa_ids_follow_the_compression_algorithm() {
    // ACPI 19.3.4. "PNP0A08": P=0x10 N=0x0E P=0x10 -> byte0 = 0x10 << 2 | 0x0E >> 3 = 0x41,
    // byte1 = (0x0E & 7) << 5 | 0x10 = 0xD0, digits 0A -> 0x0A, 08 -> 0x08: 41 D0 0A 08.
    assert_eq!(
        eisa_id("PNP0A08").unwrap().to_le_bytes(),
        [0x41, 0xD0, 0x0A, 0x08]
    );
    assert_eq!(
        eisa_id("PNP0501").unwrap().to_le_bytes(),
        [0x41, 0xD0, 0x05, 0x01]
    );
    assert_eq!(
        eisa_id("PNP0103").unwrap().to_le_bytes(),
        [0x41, 0xD0, 0x01, 0x03]
    );
}

// --- Named objects and containers (ACPI 20.2.5) ------------------------------------------------

#[test]
fn name_with_integer() {
    // DefName: NameOp 08, NameString `_UID`, ZeroOp.
    let got = build(|b| b.name_integer("_UID", 0));
    assert_eq!(got, hex("08 5F 55 49 44 00"));
}

#[test]
fn name_with_eisa_id() {
    // `Name(_HID, EISAID("PNP0A08"))`: NameOp, `_HID`, DWordPrefix 0C, 41 D0 0A 08.
    let got = build(|b| b.name_eisa_id("_HID", "PNP0A08"));
    assert_eq!(got, hex("08 5F 48 49 44 0C 41 D0 0A 08"));
}

#[test]
fn name_with_string() {
    // `Name(_HID, "ACPI0003")`: NameOp, `_HID`, StringPrefix, 8 characters, NUL.
    let got = build(|b| b.name_string("_HID", "ACPI0003"));
    assert_eq!(got, hex("08 5F 48 49 44 0D 41 43 50 49 30 30 30 33 00"));
}

#[test]
fn buffer_with_bytes() {
    // BufferOp 11, PkgLength, BufferSize = ByteConst 03 (0A 03), bytes 01 02 03.
    // Body = 2 + 3 = 5 -> PkgLength value 6.
    let got = build(|b| b.buffer(&[1, 2, 3]));
    assert_eq!(got, hex("11 06 0A 03 01 02 03"));
}

#[test]
fn empty_package() {
    // PackageOp 12, PkgLength (NumElements byte only: body 1 -> value 2), NumElements 00.
    let got = build(|b| b.package(|_| Ok(())));
    assert_eq!(got, hex("12 02 00"));
}

#[test]
fn package_of_four_integers_like_a_prt_entry() {
    // Package{0x0004FFFF, 0, 0, 5}: NumElements 4; elements 0C FF FF 04 00 | 00 | 00 | 0A 05
    // (9 bytes). Body = 1 + 9 = 10 -> PkgLength value 11 = 0x0B.
    let got = build(|b| {
        b.package(|b| {
            b.integer(0x0004_FFFF)?;
            b.integer(0)?;
            b.integer(0)?;
            b.integer(5)
        })
    });
    assert_eq!(got, hex("12 0B 04 0C FF FF 04 00 00 00 0A 05"));
}

#[test]
fn nested_package_counts_one_element() {
    // Package{ Package{1} }: inner = 12 03 01 01 (body: NumElements + OneOp = 2 -> value 3);
    // outer body = 1 + 4 = 5 -> value 6, NumElements 1.
    let got = build(|b| b.package(|b| b.package(|b| b.integer(1))));
    assert_eq!(got, hex("12 06 01 12 03 01 01"));
}

#[test]
fn empty_scope() {
    // ScopeOp 10, PkgLength (name 5 bytes -> value 6), `\_SB_`.
    let got = build(|b| b.scope("\\_SB", |_| Ok(())));
    assert_eq!(got, hex("10 06 5C 5F 53 42 5F"));
}

#[test]
fn device_inside_scope() {
    // Device PCI0 = 5B 82, PkgLength (body 4 -> value 5), `PCI0` -> 5B 82 05 50 43 49 30 (7
    // bytes). Scope body = 5 (name) + 7 = 12 -> value 13 = 0x0D.
    let got = build(|b| b.scope("\\_SB", |b| b.device("PCI0", |_| Ok(()))));
    assert_eq!(got, hex("10 0D 5C 5F 53 42 5F 5B 82 05 50 43 49 30"));
}

#[test]
fn method_returning_an_integer() {
    // `Method(_STA, 0, NotSerialized) { Return(0x0F) }`: MethodOp 14, PkgLength, `_STA`, flags
    // 00, ReturnOp A4 (NOT VERIFIED), ByteConst 0A 0F. Body = 4 + 1 + 3 = 8 -> value 9.
    let got = build(|b| b.method("_STA", 0, false, |b| b.return_integer(0x0F)));
    assert_eq!(got, hex("14 09 5F 53 54 41 00 A4 0A 0F"));
}

#[test]
fn method_flags_carry_arg_count_and_serialize_bit() {
    // MethodFlags: bits 2..0 = ArgCount (2), bit 3 = Serialized -> 0x0A. Body = 4 + 1 -> value 6.
    let got = build(|b| b.method("MTH0", 2, true, |_| Ok(())));
    assert_eq!(got, hex("14 06 4D 54 48 30 0A"));
}

#[test]
fn package_element_can_reference_a_defined_device() {
    // Scope(\_SB) { Device(LNKA){}  Name(PRT0, Package{ Package{0x0004FFFF, 0, \_SB.LNKA, 0} }) }
    // Device LNKA: 5B 82 05 4C 4E 4B 41. Inner package: 0C FF FF 04 00 | 00 |
    // 5C 2E 5F 53 42 5F 4C 4E 4B 41 | 00 = 17 bytes, NumElements 4, body 18 -> value 19 = 0x13.
    // Outer package: NumElements 1 + inner (1 + 1 + 18 = 20) = 21 -> value 22 = 0x16.
    // Name = 08 `PRT0` + outer = 5 + 23 = 28 bytes. Scope body = 5 + 7 + 28 = 40 -> value 41.
    let got = build(|b| {
        b.scope("\\_SB", |b| {
            b.device("LNKA", |_| Ok(()))?;
            b.name_package("PRT0", |b| {
                b.package(|b| {
                    b.integer(0x0004_FFFF)?;
                    b.integer(0)?;
                    b.name_reference("\\_SB.LNKA")?;
                    b.integer(0)
                })
            })
        })
    });
    assert_eq!(
        got,
        hex(
            "10 29 5C 5F 53 42 5F 5B 82 05 4C 4E 4B 41 08 50 52 54 30 12 16 01 12 13 04 \
             0C FF FF 04 00 00 5C 2E 5F 53 42 5F 4C 4E 4B 41 00"
        )
    );
}

// --- Resource descriptors (ACPI 6.4.2, 6.4.3) --------------------------------------------------

fn template(f: impl FnOnce(&mut ResourceTemplate) -> Result<(), AmlError>) -> Vec<u8> {
    ResourceTemplate::build(f).unwrap().bytes().to_vec()
}

#[test]
fn resource_template_end_tag_only() {
    // End Tag: 0x79 (small item 0xF, length 1) and checksum 0 ("checksum operation succeeded").
    assert_eq!(template(|_| Ok(())), hex("79 00"));
}

#[test]
fn resource_memory32_fixed() {
    // Memory32Fixed(ReadWrite, 0xFED40000, 0x5000): 86, length 9 (09 00), info 01, base
    // 00 00 D4 FE, length 00 50 00 00 (Table 6.43), then the End Tag.
    let got = template(|rt| rt.memory32_fixed(true, 0xFED4_0000, 0x5000));
    assert_eq!(got, hex("86 09 00 01 00 00 D4 FE 00 50 00 00 79 00"));
}

#[test]
fn resource_io_port() {
    // IO(Decode16, 0x3F8, 0x3F8, 1, 8): 47, info 01 (_DEC), min F8 03, max F8 03, align 01,
    // length 08 (Table 6.33).
    let got = template(|rt| rt.io(true, 0x3F8, 0x3F8, 1, 8));
    assert_eq!(got, hex("47 01 F8 03 F8 03 01 08 79 00"));
}

#[test]
fn resource_irq_short_and_long_forms() {
    // IRQ {4}: 22 (length 2), mask 0x0010 -> 10 00. With information byte (length 3 -> 23):
    // edge/active-high/exclusive -> bit 0 only = 01; level/active-low/shared -> bit 3 | bit 4 =
    // 0x18 (Table 6.28).
    let short = template(|rt| rt.irq(&[4], None));
    assert_eq!(short, hex("22 10 00 79 00"));
    let edge = InterruptFlags {
        edge: true,
        active_low: false,
        shared: false,
        wake: false,
    };
    assert_eq!(
        template(|rt| rt.irq(&[4], Some(edge))),
        hex("23 10 00 01 79 00")
    );
    let level = InterruptFlags {
        edge: false,
        active_low: true,
        shared: true,
        wake: false,
    };
    assert_eq!(
        template(|rt| rt.irq(&[4], Some(level))),
        hex("23 10 00 18 79 00")
    );
}

#[test]
fn resource_extended_interrupt_single() {
    // Interrupt(ResourceConsumer, Edge, ActiveHigh, Exclusive) {4}: 89, length 6 (06 00), flags =
    // consumer (bit 0) | edge (bit 1) = 03, table length 01, number 04 00 00 00 (Table 6.52).
    let flags = InterruptFlags {
        edge: true,
        active_low: false,
        shared: false,
        wake: false,
    };
    let got = template(|rt| rt.extended_interrupt(true, flags, &[4]));
    assert_eq!(got, hex("89 06 00 03 01 04 00 00 00 79 00"));
}

#[test]
fn resource_extended_interrupt_two_numbers() {
    // Interrupt(ResourceConsumer, Level, ActiveLow, Shared) {10, 11}: length 2 + 8 = 10 (0A 00),
    // flags = 1 | (active-low) 4 | (shared) 8 = 0D, count 02, 0A 00 00 00, 0B 00 00 00.
    let flags = InterruptFlags {
        edge: false,
        active_low: true,
        shared: true,
        wake: false,
    };
    let got = template(|rt| rt.extended_interrupt(true, flags, &[10, 11]));
    assert_eq!(got, hex("89 0A 00 0D 02 0A 00 00 00 0B 00 00 00 79 00"));
}

#[test]
fn resource_word_bus_number() {
    // WordBusNumber(ResourceProducer, MinFixed, MaxFixed, PosDecode, 0, 0, 0xFF, 0, 0x100):
    // 88, length 13 (0D 00), type 02 (bus number), general flags MAF|MIF = 0C, type flags 00,
    // _GRA 00 00, _MIN 00 00, _MAX FF 00, _TRA 00 00, _LEN 00 01 (Table 6.47).
    let range = AddressRange {
        granularity: 0,
        min: 0,
        max: 0xFF,
        translation: 0,
        length: 0x100,
    };
    let got = template(|rt| rt.word_bus_number(SpaceFlags::FIXED, range));
    assert_eq!(
        got,
        hex("88 0D 00 02 0C 00 00 00 00 00 FF 00 00 00 00 01 79 00")
    );
}

#[test]
fn resource_dword_memory() {
    // DWordMemory(..., MinFixed, MaxFixed, Cacheable, ReadWrite, 0, 0xC0000000, 0xDFFFFFFF, 0,
    // 0x20000000): 87, length 23 (17 00), type 00 (memory), flags 0C, type-specific = _MEM 1 << 1
    // | _RW 1 = 03, then five little-endian DWORDs (Tables 6.46, 6.49).
    let mem = MemoryFlags {
        cacheability: 1,
        read_write: true,
    };
    let range = AddressRange {
        granularity: 0,
        min: 0xC000_0000,
        max: 0xDFFF_FFFF,
        translation: 0,
        length: 0x2000_0000,
    };
    let got = template(|rt| rt.dword_memory(SpaceFlags::FIXED, mem, range));
    assert_eq!(
        got,
        hex("87 17 00 00 0C 03 00 00 00 00 00 00 00 C0 FF FF FF DF 00 00 00 00 00 00 00 20 79 00")
    );
}

#[test]
fn resource_qword_memory() {
    // QWordMemory(..., NonCacheable, ReadWrite, 0, 1<<32, 0x1FFFFFFFF, 0, 1<<32): 8A, length 43
    // (2B 00), type 00, flags 0C, type-specific 01, five little-endian QWORDs. NOT VERIFIED
    // against Table 6.45 directly; layout by analogy with the DWord descriptor.
    let mem = MemoryFlags {
        cacheability: 0,
        read_write: true,
    };
    let range = AddressRange {
        granularity: 0,
        min: 0x1_0000_0000,
        max: 0x1_FFFF_FFFF,
        translation: 0,
        length: 0x1_0000_0000,
    };
    let got = template(|rt| rt.qword_memory(SpaceFlags::FIXED, mem, range));
    assert_eq!(
        got,
        hex("8A 2B 00 00 0C 01 \
             00 00 00 00 00 00 00 00 \
             00 00 00 00 01 00 00 00 \
             FF FF FF FF 01 00 00 00 \
             00 00 00 00 00 00 00 00 \
             00 00 00 00 01 00 00 00 79 00")
    );
}

#[test]
fn name_crs_wraps_the_template_in_a_buffer() {
    // Name(_CRS, ResourceTemplate(){IO(...)}): NameOp, `_CRS`, BufferOp 11, PkgLength, BufferSize
    // = ByteConst 0A 0A (10 bytes = 8 + End Tag 2), bytes. Body = 2 + 10 = 12 -> value 13 = 0D.
    let crs = ResourceTemplate::build(|rt| rt.io(true, 0x3F8, 0x3F8, 1, 8)).unwrap();
    let got = build(|b| b.name_resource_template("_CRS", &crs));
    assert_eq!(
        got,
        hex("08 5F 43 52 53 11 0D 0A 0A 47 01 F8 03 F8 03 01 08 79 00")
    );
}

// --- Negative cases ----------------------------------------------------------------------------

#[test]
fn pkg_length_overflow_is_rejected() {
    // body 0x0FFF_FFFC would need value 0x1000_0000 > 2^28 - 1.
    assert_eq!(
        pkg_length_for_body(0x0FFF_FFFC),
        Err(AmlError::PkgLengthOverflow)
    );
    assert_eq!(
        pkg_length_for_body(usize::MAX),
        Err(AmlError::PkgLengthOverflow)
    );
}

#[test]
fn invalid_names_are_rejected() {
    for bad in ["lower", "TOOLONG", "1ABC", "A-B", "A..B", "\\_SB.", ""] {
        let parsed = AmlName::parse(bad);
        let rejected = match parsed {
            Err(AmlError::BadNameSeg) => true,
            // The empty string is a legal NullName for the parser; named objects reject it.
            Ok(name) if bad.is_empty() => name.segs().is_empty(),
            _ => false,
        };
        assert!(rejected, "{bad:?} -> {parsed:?}");
    }
    let mut storage = [0u8; 32];
    let mut b = AmlBuilder::new(&mut storage);
    assert_eq!(b.name_integer("", 0), Err(AmlError::EmptyName));
}

#[test]
fn reference_to_an_unknown_name_is_rejected() {
    let mut storage = [0u8; 64];
    let mut b = AmlBuilder::new(&mut storage);
    let result = b.package(|b| b.name_reference("\\_SB.NOPE"));
    assert_eq!(result, Err(AmlError::UnknownName));
    let mut b = AmlBuilder::new(&mut storage);
    let result = b.package(|b| b.name_reference("LNKA"));
    assert_eq!(result, Err(AmlError::RelativeReference));
}

#[test]
fn duplicate_name_in_one_scope_is_rejected() {
    let mut storage = [0u8; 64];
    let mut b = AmlBuilder::new(&mut storage);
    let result = b.scope("\\_SB", |b| {
        b.name_integer("FOO0", 1)?;
        b.name_integer("FOO0", 2)
    });
    assert_eq!(result, Err(AmlError::DuplicateName));
    let mut b = AmlBuilder::new(&mut storage);
    let result = b.scope("\\_SB", |b| {
        b.device("DEV0", |_| Ok(()))?;
        b.method("DEV0", 0, false, |_| Ok(()))
    });
    assert_eq!(result, Err(AmlError::DuplicateName));
}

#[test]
fn same_name_in_different_scopes_is_allowed() {
    let got = build(|b| {
        b.scope("\\_SB", |b| {
            b.device("DEV0", |b| b.name_integer("_UID", 0))?;
            b.device("DEV1", |b| b.name_integer("_UID", 1))
        })
    });
    validate_definition_block(&got).unwrap();
}

#[test]
fn full_output_buffer_is_reported() {
    let mut storage = [0u8; 4];
    let mut b = AmlBuilder::new(&mut storage);
    assert_eq!(b.name_integer("_UID", 0), Err(AmlError::BufferFull));
}

#[test]
fn method_with_too_many_arguments_is_rejected() {
    let mut storage = [0u8; 32];
    let mut b = AmlBuilder::new(&mut storage);
    assert_eq!(
        b.method("MTH0", 8, false, |_| Ok(())),
        Err(AmlError::BadMethodFlags)
    );
}

#[test]
fn resource_fields_are_range_checked() {
    assert_eq!(
        ResourceTemplate::build(|rt| rt.io(true, 0x400, 0x3F8, 1, 8)).err(),
        Some(AmlError::BadResource)
    );
    let flags = InterruptFlags {
        edge: true,
        active_low: false,
        shared: false,
        wake: false,
    };
    assert_eq!(
        ResourceTemplate::build(|rt| rt.extended_interrupt(true, flags, &[])).err(),
        Some(AmlError::BadResource)
    );
    assert_eq!(
        ResourceTemplate::build(|rt| rt.irq(&[16], None)).err(),
        Some(AmlError::BadResource)
    );
}

// --- Validator on raw bytes --------------------------------------------------------------------

#[test]
fn validator_accepts_builder_output() {
    let got = build(|b| {
        b.scope("\\_SB", |b| {
            b.device("DEV0", |b| {
                b.name_integer("_UID", 0)?;
                b.name_string("_STR", "x")?;
                b.name_package("_PRT", |_| Ok(()))
            })
        })
    });
    validate_definition_block(&got).unwrap();
}

#[test]
fn validator_rejects_duplicate_names_in_raw_aml() {
    // Scope(\_SB_) { Name(FOO_, 0) Name(FOO_, 0) }: body 5 + 6 + 6 = 17 -> PkgLength 18 = 0x12.
    let aml = hex("10 12 5C 5F 53 42 5F 08 46 4F 4F 5F 00 08 46 4F 4F 5F 00");
    assert_eq!(
        validate_definition_block(&aml),
        Err(AmlError::DuplicateName)
    );
}

#[test]
fn validator_rejects_a_package_that_overruns_its_parent() {
    // Scope claims 0x20 bytes but only 7 are present.
    let aml = hex("10 20 5C 5F 53 42 5F");
    assert_eq!(validate_definition_block(&aml), Err(AmlError::BadPkgLength));
}

#[test]
fn validator_rejects_unsupported_opcodes() {
    assert_eq!(
        validate_definition_block(&hex("A4 00")),
        Err(AmlError::UnsupportedOpcode(0xA4))
    );
}
