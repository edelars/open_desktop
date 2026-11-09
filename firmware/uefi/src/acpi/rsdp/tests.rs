//! S4-31g: RSDP revision 2 and XSDT.
//!
//! The reference bytes were produced by a separate Python script that shares no code with the
//! serializer (`struct.pack` of each field, then the checksums as `(-sum) & 0xFF`); the RSDP
//! checksums 0x10 and 0x4D and the XSDT checksum 0x6C come from there. Offsets are literals.

use super::*;

fn hex(text: &str) -> Vec<u8> {
    text.split_whitespace()
        .map(|b| u8::from_str_radix(b, 16).unwrap())
        .collect()
}

fn sum(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0u8, |a, &b| a.wrapping_add(b))
}

/// RSDP for an XSDT at 0xE_0180. Offsets: 0 "RSD PTR "; 8 checksum 0x10 (sum of 0..20 = 0);
/// 9 OEMID "OPNDSK"; 15 Revision 2; 16 RsdtAddress 0; 20 Length 0x24; 24 XsdtAddress 0xE_0180;
/// 32 extended checksum 0x4D (sum of 0..36 = 0); 33..36 reserved 0.
fn reference_rsdp() -> Vec<u8> {
    hex(
        "52 53 44 20 50 54 52 20 10 4f 50 4e 44 53 4b 02 00 00 00 00 24 00 00 00 \
         80 01 0e 00 00 00 00 00 4d 00 00 00",
    )
}

/// XSDT with three entries 0xE_0200, 0xE_0150, 0xE_0320: header (signature, Length 0x3C,
/// Revision 1, checksum 0x6C, OEM "OPNDSK", OEM table "ODFWXSDT", OEM rev 1, creator "ODFW",
/// creator rev 1), then three little-endian u64.
fn reference_xsdt() -> Vec<u8> {
    hex(
        "58 53 44 54 3c 00 00 00 01 6c 4f 50 4e 44 53 4b 4f 44 46 57 58 53 44 54 \
         01 00 00 00 4f 44 46 57 01 00 00 00 \
         00 02 0e 00 00 00 00 00 50 01 0e 00 00 00 00 00 20 03 0e 00 00 00 00 00",
    )
}

fn rsdp(xsdt: u64, fill: u8) -> Vec<u8> {
    let mut out = [fill; 48];
    assert_eq!(build_rsdp(xsdt, &mut out), Ok(36));
    // Nothing past the 36 bytes is touched.
    assert!(out[36..].iter().all(|&b| b == fill));
    out[..36].to_vec()
}

fn xsdt(entries: &[u64], fill: u8) -> Vec<u8> {
    let mut out = [fill; 128];
    let len = build_xsdt(entries, &mut out).unwrap();
    assert_eq!(len, 36 + 8 * entries.len());
    out[..len].to_vec()
}

#[test]
fn rsdp_equals_the_independent_reference() {
    assert_eq!(rsdp(0xE_0180, 0), reference_rsdp());
    let r = reference_rsdp();
    assert_eq!(sum(&r[..20]), 0, "first checksum");
    assert_eq!(sum(&r), 0, "extended checksum");
    assert_eq!((r[8], r[32]), (0x10, 0x4D));
}

#[test]
fn rsdp_does_not_depend_on_the_buffer_and_has_no_rsdt() {
    assert_eq!(rsdp(0xE_0180, 0xFF), rsdp(0xE_0180, 0x00));
    assert_eq!(rsdp(0xE_0180, 0xA5), reference_rsdp());
    let r = rsdp(0xE_0180, 0);
    assert_eq!(&r[..8], b"RSD PTR ");
    assert_eq!(r[15], 2);
    assert_eq!(&r[16..20], &[0, 0, 0, 0], "RsdtAddress is 0");
    assert_eq!(u32::from_le_bytes(r[20..24].try_into().unwrap()), 36);
}

#[test]
fn const_rsdp_and_xsdt_equal_the_runtime_ones() {
    const RSDP: [u8; 36] = {
        let mut out = [0u8; 36];
        match build_rsdp(0xE_0180, &mut out) {
            Ok(_) => out,
            Err(_) => panic!("rsdp"),
        }
    };
    assert_eq!(RSDP.as_slice(), reference_rsdp().as_slice());
    const XSDT: [u8; 60] = {
        let mut out = [0u8; 60];
        match build_xsdt(&[0xE_0200, 0xE_0150, 0xE_0320], &mut out) {
            Ok(_) => out,
            Err(_) => panic!("xsdt"),
        }
    };
    assert_eq!(XSDT.as_slice(), reference_xsdt().as_slice());
}

#[test]
fn rsdp_validation_accepts_the_result_and_reports_each_defect() {
    let good = reference_rsdp();
    assert_eq!(validate_rsdp(&good), Ok(()));
    assert_eq!(validate_rsdp(&good[..35]), Err(RsdpError::TooShort));

    let mut t = good.clone();
    t[0] = b'X';
    assert_eq!(validate_rsdp(&t), Err(RsdpError::SignatureMismatch));

    // A change inside the first 20 bytes breaks both sums; the first one is reported.
    let mut t = good.clone();
    t[10] ^= 1;
    assert_eq!(validate_rsdp(&t), Err(RsdpError::BadChecksum(1)));
    // A change after byte 20 leaves the first sum intact and breaks only the extended one
    // (byte 26 is 0x0E, flipping bit 2 lowers it by 4, so the sum is 0x100 - 4).
    let mut t = good.clone();
    t[26] ^= 0x04;
    assert_eq!(validate_rsdp(&t), Err(RsdpError::BadExtendedChecksum(0xFC)));
    let mut t = good.clone();
    t[32] ^= 0x80;
    assert_eq!(validate_rsdp(&t), Err(RsdpError::BadExtendedChecksum(0x80)));

    // Rules of this board, each with both checksums repaired so only the rule fails.
    let fixed = |mutate: &dyn Fn(&mut Vec<u8>)| {
        let mut t = good.clone();
        mutate(&mut t);
        t[8] = 0;
        t[8] = sum(&t[..20]).wrapping_neg();
        t[32] = 0;
        t[32] = sum(&t).wrapping_neg();
        validate_rsdp(&t)
    };
    assert_eq!(fixed(&|t| t[15] = 0), Err(RsdpError::BadRevision(0)));
    assert_eq!(fixed(&|t| t[16] = 1), Err(RsdpError::RsdtAddressNotZero(1)));
    assert_eq!(fixed(&|t| t[20] = 20), Err(RsdpError::BadLength(20)));
    assert_eq!(
        fixed(&|t| t[24..32].fill(0)),
        Err(RsdpError::BadXsdtAddress(0))
    );
    assert_eq!(
        fixed(&|t| t[24] = 0x84),
        Err(RsdpError::BadXsdtAddress(0xE_0184))
    );
    assert_eq!(fixed(&|t| t[34] = 1), Err(RsdpError::ReservedNotZero(34)));
}

#[test]
fn build_rsdp_refuses_a_short_buffer_and_a_bad_address() {
    let mut out = [0u8; 35];
    assert_eq!(build_rsdp(0xE_0180, &mut out), Err(RsdpError::TooShort));
    let mut out = [0u8; 36];
    assert_eq!(build_rsdp(0, &mut out), Err(RsdpError::BadXsdtAddress(0)));
    assert_eq!(
        build_rsdp(0xE_0184, &mut out),
        Err(RsdpError::BadXsdtAddress(0xE_0184))
    );
}

#[test]
fn xsdt_equals_the_independent_reference() {
    let entries = [0xE_0200u64, 0xE_0150, 0xE_0320];
    let t = xsdt(&entries, 0);
    assert_eq!(t, reference_xsdt());
    assert_eq!(sum(&t), 0);
    assert_eq!(t[9], 0x6C);
    // The buffer content does not matter.
    assert_eq!(xsdt(&entries, 0xFF), t);
    assert_eq!(xsdt(&entries, 0xA5), t);
    // One entry: 44 bytes.
    assert_eq!(xsdt(&[0xE_0150], 0).len(), 44);
}

#[test]
fn xsdt_validation_accepts_the_result_and_reports_each_defect() {
    let good = reference_xsdt();
    assert_eq!(validate_xsdt(&good), Ok(()));

    let mut t = good.clone();
    t[0] = b'Y';
    assert_eq!(
        validate_xsdt(&t),
        Err(RsdpError::Table(TableError::SignatureMismatch))
    );
    let mut t = good.clone();
    t[40] ^= 1;
    assert_eq!(
        validate_xsdt(&t),
        Err(RsdpError::Table(TableError::BadChecksum { sum: 1 }))
    );
    assert_eq!(
        validate_xsdt(&good[..59]),
        Err(RsdpError::Table(TableError::LengthMismatch {
            declared: 60,
            actual: 59
        }))
    );
    let fixed = |mutate: &dyn Fn(&mut Vec<u8>)| {
        let mut t = good.clone();
        mutate(&mut t);
        t[9] = 0;
        t[9] = sum(&t).wrapping_neg();
        validate_xsdt(&t)
    };
    assert_eq!(fixed(&|t| t[8] = 2), Err(RsdpError::BadXsdtRevision(2)));
    // A zero entry.
    assert_eq!(
        fixed(&|t| t[44..52].fill(0)),
        Err(RsdpError::BadXsdtEntries)
    );
    // A body that is not a whole number of entries: 36 + 4 bytes with a consistent Length.
    let mut odd = good[..40].to_vec();
    odd[4..8].copy_from_slice(&40u32.to_le_bytes());
    odd[9] = 0;
    odd[9] = sum(&odd).wrapping_neg();
    assert_eq!(validate_xsdt(&odd), Err(RsdpError::BadXsdtEntries));
    // No entries at all.
    let mut empty = good[..36].to_vec();
    empty[4..8].copy_from_slice(&36u32.to_le_bytes());
    empty[9] = 0;
    empty[9] = sum(&empty).wrapping_neg();
    assert_eq!(validate_xsdt(&empty), Err(RsdpError::BadXsdtEntries));
}

#[test]
fn build_xsdt_refuses_bad_entry_lists_and_short_buffers() {
    let mut out = [0u8; 128];
    assert_eq!(build_xsdt(&[], &mut out), Err(RsdpError::BadXsdtEntries));
    assert_eq!(
        build_xsdt(&[0xE_0150, 0], &mut out),
        Err(RsdpError::BadXsdtEntries)
    );
    assert_eq!(
        build_xsdt(&[1; 9], &mut [0u8; 128]),
        Err(RsdpError::BadXsdtEntries)
    );
    assert_eq!(
        build_xsdt(&[0xE_0150], &mut [0u8; 43]),
        Err(RsdpError::Table(TableError::TooShort))
    );
}
