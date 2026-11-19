use super::*;

fn seal(bytes: &mut [u8]) {
    let len = u32::try_from(bytes.len()).unwrap();
    bytes[12..16].copy_from_slice(&len.to_le_bytes());
    let crc = crc32::with_zeroed_word(bytes, CRC_OFFSET);
    bytes[20..24].copy_from_slice(&crc.to_le_bytes());
}

fn archive(files: &[(&[u8], &[u8])]) -> Vec<u8> {
    let mut bytes = b"ODFVARC\0\x01\x00\x18\x00".to_vec();
    bytes.extend(0u32.to_le_bytes());
    bytes.extend(u32::try_from(files.len()).unwrap().to_le_bytes());
    bytes.extend(0u32.to_le_bytes());
    for &(name, data) in files {
        bytes.extend(u16::try_from(name.len()).unwrap().to_le_bytes());
        bytes.extend(0u16.to_le_bytes());
        bytes.extend(u32::try_from(data.len()).unwrap().to_le_bytes());
        bytes.extend(name);
        bytes.extend(data);
    }
    seal(&mut bytes);
    bytes
}

#[test]
fn returns_borrowed_bytes_for_exact_names_only() {
    let bytes = archive(&[
        (b"a.efi", b"aa"),
        (b"abc.efi", b"bb"),
        (b"z-2_1", b"\0\xff\x60"),
    ]);
    let parsed = Archive::parse(&bytes).unwrap();
    assert_eq!(parsed.get("a.efi"), Some(&bytes[37..39]));
    assert_eq!(parsed.get("a.efi").unwrap().as_ptr(), bytes[37..].as_ptr());
    assert_eq!(parsed.get("abc.efi"), Some(&b"bb"[..]));
    assert_eq!(parsed.get("z-2_1"), Some(&b"\0\xff\x60"[..]));
    for missing in ["", "a", "abc", "abc.efi.bak", "z-2_2", "A.EFI", "é"] {
        assert_eq!(parsed.get(missing), None, "{missing}");
    }
}

#[test]
fn accepts_empty_archive_and_maximum_name() {
    let bytes = archive(&[]);
    assert_eq!(Archive::parse(&bytes).unwrap().get("missing"), None);
    let name = "a".repeat(63);
    let bytes = archive(&[(name.as_bytes(), b"1")]);
    assert_eq!(Archive::parse(&bytes).unwrap().get(&name), Some(&b"1"[..]));
}

#[test]
fn refuses_every_truncated_prefix_and_corruption() {
    let bytes = archive(&[(b"app", b"application")]);
    for size in 0..bytes.len() {
        assert!(Archive::parse(&bytes[..size]).is_err(), "prefix {size}");
    }
    for index in 0..bytes.len() {
        let mut changed = bytes.clone();
        changed[index] ^= 1;
        assert!(Archive::parse(&changed).is_err(), "byte {index}");
    }
    let mut trailing = bytes.clone();
    trailing.push(0);
    seal(&mut trailing);
    assert_eq!(Archive::parse(&trailing).unwrap_err(), ArchiveError::Count);
}

#[test]
fn rejects_bad_count_and_record_fields_with_valid_checksums() {
    let bytes = archive(&[(b"app", b"application")]);
    for (offset, encoded, expected) in [
        (16, &u32::MAX.to_le_bytes()[..], ArchiveError::Count),
        (24, &u16::MAX.to_le_bytes()[..], ArchiveError::Record),
        (26, &1u16.to_le_bytes()[..], ArchiveError::Record),
        (28, &u32::MAX.to_le_bytes()[..], ArchiveError::Record),
        (28, &0u32.to_le_bytes()[..], ArchiveError::Record),
    ] {
        let mut changed = bytes.clone();
        changed[offset..offset + encoded.len()].copy_from_slice(encoded);
        seal(&mut changed);
        assert_eq!(Archive::parse(&changed).unwrap_err(), expected);
    }
}

#[test]
fn rejects_unsafe_repeated_and_unsorted_names() {
    for &name in &[
        &b""[..],
        b".",
        b"..",
        b"A",
        b"x/y",
        b"x\\y",
        b"x y",
        b"x\0y",
        b"\xff",
        &[b'a'; 64],
    ] {
        let bytes = archive(&[(name, b"file contents")]);
        assert_eq!(Archive::parse(&bytes).unwrap_err(), ArchiveError::Name);
    }
    for files in [
        [(b"bb" as &[u8], b"1" as &[u8]), (b"aa", b"2")],
        [(b"aa" as &[u8], b"1" as &[u8]), (b"aa", b"2")],
    ] {
        let bytes = archive(&files);
        assert_eq!(Archive::parse(&bytes).unwrap_err(), ArchiveError::Order);
    }
}
