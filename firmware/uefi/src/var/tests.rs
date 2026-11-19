use super::*;
use crate::var::{
    codec::{DecodeError, Decoder, MAX_NAME_UNITS},
    services::{
        AUTHENTICATED_WRITE_ACCESS, BOOT_RUNTIME, ENHANCED_AUTHENTICATED_ACCESS, GLOBAL_VARIABLE,
        HARDWARE_ERROR_RECORD, TIME_BASED_AUTHENTICATED_WRITE_ACCESS, seed_defaults,
    },
};

const VENDOR: Guid = Guid {
    data1: 0x0123_4567,
    data2: 0x89ab,
    data3: 0xcdef,
    data4: [0x89; 8],
};
const OTHER: Guid = Guid {
    data1: 0xfeca_dede,
    ..VENDOR
};

fn a() -> Name<'static> {
    Name::new(b"a\0").unwrap()
}
fn b() -> Name<'static> {
    Name::new(b"b\0").unwrap()
}

#[test]
fn validates_ucs2_name_bounds_and_code_units() {
    assert!(Name::new(b"").is_none());
    assert!(Name::new(b"x").is_none());
    assert!(Name::new(b"x\0\0\0").is_none());
    assert!(Name::new(&[0, 0xd8]).is_none());
    assert!(Name::new(&[0xa9, 0x03]).is_some());
    assert!(Name::new(&[0x41; 2 * MAX_NAME_UNITS]).is_some());
    assert!(Name::new(&[0x41; 2 * (MAX_NAME_UNITS + 1)]).is_none());
}

#[test]
fn keys_include_the_guid_and_names_are_case_sensitive() {
    let mut store = VariableStore::new();
    let upper = Name::new(b"A\0").unwrap();
    assert_eq!(
        store.set(VENDOR, a(), BOOTSERVICE_ACCESS, b"first"),
        Ok(false)
    );
    assert_eq!(
        store.set(OTHER, a(), BOOTSERVICE_ACCESS, b"other"),
        Ok(false)
    );
    assert_eq!(
        store.set(VENDOR, upper, BOOTSERVICE_ACCESS, b"upper"),
        Ok(false)
    );
    assert_eq!(store.get(VENDOR, a()).unwrap().value, b"first");
    assert_eq!(store.get(OTHER, a()).unwrap().value, b"other");
    assert_eq!(store.get(VENDOR, upper).unwrap().value, b"upper");
    assert_eq!(store.get(OTHER, upper), None);
}

#[test]
fn denies_changes_to_the_persistent_attributes_of_an_existing_key() {
    let mut store = VariableStore::new();
    store.set(VENDOR, a(), NON_VOLATILE_RUNTIME, b"a").unwrap();
    for data in [&b""[..], &b"new"[..]] {
        assert_eq!(
            store.set(VENDOR, a(), BOOT_RUNTIME, data),
            Err(Status::INVALID_PARAMETER)
        );
    }
    assert_eq!(store.get(VENDOR, a()).unwrap().value, b"a");
}

#[test]
fn appends_without_storing_the_append_flag_or_moving_other_keys() {
    let mut store = VariableStore::new();
    assert_eq!(
        store.set(VENDOR, a(), NON_VOLATILE_RUNTIME | APPEND_WRITE, b"a"),
        Ok(true)
    );
    store.set(VENDOR, b(), BOOTSERVICE_ACCESS, b"b").unwrap();
    assert_eq!(
        store.set(VENDOR, a(), NON_VOLATILE_RUNTIME | APPEND_WRITE, b"cdf"),
        Ok(true)
    );
    let item = store.get(VENDOR, a()).unwrap();
    assert_eq!(item.value, b"acdf");
    assert_eq!(item.attributes, NON_VOLATILE_RUNTIME);
    assert_eq!(store.next(None).unwrap().name, a());
    assert_eq!(store.next(Some((VENDOR, a()))).unwrap().value, b"b");
}

#[test]
fn zero_length_append_is_a_noop_for_present_or_absent_variables() {
    let mut store = VariableStore::new();
    let attrs = NON_VOLATILE_RUNTIME | APPEND_WRITE;
    assert_eq!(store.set(VENDOR, a(), attrs, b""), Ok(false));
    assert!(store.get(VENDOR, a()).is_none());
    store.set(VENDOR, a(), attrs, b"a").unwrap();
    let before = store.query(attrs).unwrap();
    assert_eq!(store.set(VENDOR, a(), attrs, b""), Ok(false));
    assert_eq!(store.get(VENDOR, a()).unwrap().value, b"a");
    assert_eq!(store.query(attrs), Ok(before));
}

#[test]
fn deletes_with_no_access_attributes_and_reclaims_the_record() {
    let mut store = VariableStore::new();
    let before = store.query(BOOT_RUNTIME).unwrap();
    store
        .set(VENDOR, a(), NON_VOLATILE_RUNTIME, b"abc")
        .unwrap();
    assert_eq!(store.set(VENDOR, a(), 0, b"ignored"), Ok(true));
    assert_eq!(store.query(BOOT_RUNTIME), Ok(before));
    assert_eq!(store.set(VENDOR, a(), 0, b""), Err(Status::NOT_FOUND));
}

#[test]
fn deletes_zero_size_without_append_and_compacts_the_tail() {
    let mut store = VariableStore::new();
    store.set(VENDOR, a(), BOOT_RUNTIME, b"abc").unwrap();
    store.set(VENDOR, b(), BOOT_RUNTIME, b"tail").unwrap();
    assert_eq!(store.set(VENDOR, a(), BOOT_RUNTIME, b""), Ok(false));
    assert_eq!(store.next(None).unwrap().value, b"tail");
    assert_eq!(
        store.next(Some((VENDOR, a()))),
        Err(Status::INVALID_PARAMETER)
    );
    assert_eq!(store.next(Some((VENDOR, b()))), Err(Status::NOT_FOUND));
}

#[test]
fn shortening_a_record_does_not_move_its_enumeration_position() {
    let mut store = VariableStore::new();
    store
        .set(VENDOR, a(), BOOTSERVICE_ACCESS, b"compression")
        .unwrap();
    store.set(VENDOR, b(), BOOTSERVICE_ACCESS, b"tail").unwrap();
    let before = store.query(BOOTSERVICE_ACCESS).unwrap().remaining;
    store.set(VENDOR, a(), BOOTSERVICE_ACCESS, b"c").unwrap();
    assert_eq!(
        store.query(BOOTSERVICE_ACCESS).unwrap().remaining,
        before + 10
    );
    assert_eq!(store.next(None).unwrap().value, b"c");
    assert_eq!(store.next(Some((VENDOR, a()))).unwrap().value, b"tail");
}

#[test]
fn refuses_invalid_and_unsupported_attribute_combinations() {
    let mut store = VariableStore::new();
    for flags in [RUNTIME_ACCESS, NON_VOLATILE | RUNTIME_ACCESS, 1 << 8] {
        assert_eq!(
            store.set(VENDOR, a(), flags, b"x"),
            Err(Status::INVALID_PARAMETER)
        );
        assert_eq!(store.query(flags), Err(Status::INVALID_PARAMETER));
    }
    for flags in [
        AUTHENTICATED_WRITE_ACCESS,
        TIME_BASED_AUTHENTICATED_WRITE_ACCESS,
        ENHANCED_AUTHENTICATED_ACCESS,
        HARDWARE_ERROR_RECORD,
    ] {
        assert_eq!(
            store.set(VENDOR, a(), flags | BOOT_RUNTIME, b"x"),
            Err(Status::UNSUPPORTED)
        );
        assert_eq!(
            store.query(flags | RUNTIME_ACCESS),
            Err(Status::UNSUPPORTED)
        );
    }
    assert!(store.get(VENDOR, a()).is_none());
}

#[test]
fn query_ignores_append_and_reports_shared_capacity() {
    let store = VariableStore::new();
    let info = VariableInfo {
        capacity: CAPACITY as u64,
        remaining: CAPACITY as u64,
        maximum_value: MAX_VALUE_BYTES as u64,
    };
    assert_eq!(store.query(NON_VOLATILE_RUNTIME | APPEND_WRITE), Ok(info));
    assert_eq!(store.query(BOOTSERVICE_ACCESS), Ok(info));
    assert_eq!(store.query(0), Err(Status::INVALID_PARAMETER));
}

#[test]
fn full_store_preserves_an_existing_record_on_failed_growth() {
    let mut store = VariableStore::new();
    let big = [0xa5; MAX_VALUE_BYTES];
    for index in 1..=15 {
        let name = (0x30u16 + index).to_le_bytes();
        store
            .set(VENDOR, Name::new(&name).unwrap(), BOOT_RUNTIME, &big)
            .unwrap();
    }
    store.set(VENDOR, a(), BOOT_RUNTIME, b"old").unwrap();
    let remaining = store.query(BOOT_RUNTIME).unwrap().remaining as usize;
    let last = remaining - codec::record_size(b(), 0);
    store.set(VENDOR, b(), BOOT_RUNTIME, &big[..last]).unwrap();
    assert_eq!(store.query(BOOT_RUNTIME).unwrap().remaining, 0);
    assert_eq!(
        store.set(VENDOR, a(), BOOT_RUNTIME | APPEND_WRITE, b"x"),
        Err(Status::OUT_OF_RESOURCES)
    );
    assert_eq!(store.get(VENDOR, a()).unwrap().value, b"old");
    store.set(VENDOR, b(), 0, b"").unwrap();
    assert!(
        store
            .set(VENDOR, a(), BOOT_RUNTIME | APPEND_WRITE, b"x")
            .is_ok()
    );
}

#[test]
fn individual_size_limit_keeps_the_record_unchanged() {
    let mut store = VariableStore::new();
    let value = [3; MAX_VALUE_BYTES + 1];
    assert_eq!(
        store.set(VENDOR, a(), BOOT_RUNTIME, &value),
        Err(Status::INVALID_PARAMETER)
    );
    store
        .set(VENDOR, a(), BOOT_RUNTIME, &value[..MAX_VALUE_BYTES])
        .unwrap();
    assert_eq!(
        store.set(VENDOR, a(), BOOT_RUNTIME | APPEND_WRITE, &[4]),
        Err(Status::INVALID_PARAMETER)
    );
    assert_eq!(
        store.get(VENDOR, a()).unwrap().value,
        &value[..MAX_VALUE_BYTES]
    );
}

#[test]
fn runtime_hides_boot_only_records_and_freezes_volatile_values() {
    let mut store = VariableStore::new();
    store.set(VENDOR, a(), BOOTSERVICE_ACCESS, b"old").unwrap();
    store.set(VENDOR, b(), BOOT_RUNTIME, b"visible").unwrap();
    store.enter_runtime();
    assert!(store.get(VENDOR, a()).is_none());
    assert_eq!(store.next(None).unwrap().name, b());
    assert_eq!(
        store.query(BOOTSERVICE_ACCESS),
        Err(Status::INVALID_PARAMETER)
    );
    assert_eq!(store.set(VENDOR, b(), 0, &[]), Err(Status::WRITE_PROTECTED));
    assert_eq!(store.set(OTHER, b(), NON_VOLATILE_RUNTIME, b"nv"), Ok(true));
    assert_eq!(store.set(OTHER, b(), 0, &[]), Ok(true));
}

#[test]
fn platform_seeds_default_values_and_protects_policy_globals() {
    let mut store = VariableStore::new();
    seed_defaults(&mut store, &crate::console::COM1_DEVICE_PATH).unwrap();
    let name = |bytes| Name::new(bytes).unwrap();
    let setup = name(b"S\0e\0t\0u\0p\0M\0o\0d\0e\0");
    let secure = name(b"S\0e\0c\0u\0r\0e\0B\0o\0o\0t\0");
    let lang = name(b"P\0l\0a\0t\0f\0o\0r\0m\0L\0a\0n\0g\0");
    assert_eq!(store.records().count(), 9);
    assert_eq!(store.get(GLOBAL_VARIABLE, setup).unwrap().value, &[1]);
    assert_eq!(store.get(GLOBAL_VARIABLE, secure).unwrap().value, &[0]);
    assert_eq!(
        store.set(GLOBAL_VARIABLE, setup, 0, &[]),
        Err(Status::WRITE_PROTECTED)
    );
    assert_eq!(
        store.set(GLOBAL_VARIABLE, secure, BOOT_RUNTIME | APPEND_WRITE, &[1]),
        Err(Status::WRITE_PROTECTED)
    );
    assert_eq!(
        store.set(GLOBAL_VARIABLE, lang, NON_VOLATILE_RUNTIME, b"en-GB\0"),
        Err(Status::INVALID_PARAMETER)
    );
    assert_eq!(store.get(GLOBAL_VARIABLE, lang).unwrap().value, b"en-US\0");
}

fn encoded_pair() -> ([u8; 160], usize) {
    let mut store = VariableStore::new();
    store
        .set(VENDOR, a(), NON_VOLATILE_RUNTIME, b"NV-a")
        .unwrap();
    store.set(VENDOR, b(), BOOT_RUNTIME, b"volatile").unwrap();
    store
        .set(OTHER, b(), NON_VOLATILE_RUNTIME, b"NV-b")
        .unwrap();
    let mut buffer = [0x55; 160];
    let used = store.encode_nv(&mut buffer).unwrap();
    let mut reference = [0; 160];
    assert_eq!(store.encode_nv(&mut reference), Ok(used));
    assert_eq!(buffer[..used], reference[..used]);
    (buffer, used)
}

#[test]
fn codec_roundtrips_only_live_nv_records_in_deterministic_order() {
    let (encoded, used) = encoded_pair();
    assert_eq!(&encoded[..8], b"ODVARSTR");
    assert_eq!(&encoded[8..12], &[1, 0, 24, 0]);
    let items: Vec<_> = Decoder::new(&encoded[..used])
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(
        (items[0].guid, items[0].name, items[0].value),
        (VENDOR, a(), &b"NV-a"[..])
    );
    assert_eq!(
        (items[1].guid, items[1].name, items[1].value),
        (OTHER, b(), &b"NV-b"[..])
    );
}

#[test]
fn codec_checks_capacity_before_touching_the_destination() {
    let mut store = VariableStore::new();
    store
        .set(VENDOR, a(), NON_VOLATILE_RUNTIME, b"persist")
        .unwrap();
    let mut too_small = [0x55; 40];
    assert_eq!(
        store.encode_nv(&mut too_small),
        Err(Status::BUFFER_TOO_SMALL)
    );
    assert_eq!(too_small, [0x55; 40]);
}

#[test]
fn codec_skips_a_record_with_bad_crc_and_reads_the_next_in_frame() {
    let (mut encoded, used) = encoded_pair();
    encoded[HEADER_BYTES + codec::RECORD_HEADER_BYTES + 2] ^= 0x80;
    let mut decoder = Decoder::new(&encoded[..used]).unwrap();
    assert_eq!(
        decoder.next(),
        Some(Err(DecodeError::CorruptRecord {
            offset: HEADER_BYTES
        }))
    );
    assert_eq!(decoder.next().unwrap().unwrap().value, b"NV-b");
    assert!(decoder.next().is_none());
}

#[test]
fn codec_reports_a_truncated_tail_after_recovering_complete_records() {
    let (encoded, used) = encoded_pair();
    let mut decoder = Decoder::new(&encoded[..used - 1]).unwrap();
    assert_eq!(decoder.next().unwrap().unwrap().value, b"NV-a");
    let offset = HEADER_BYTES + codec::record_size(a(), 4);
    assert_eq!(
        decoder.next(),
        Some(Err(DecodeError::TruncatedRecord { offset }))
    );
    assert!(decoder.next().is_none());
}

#[test]
fn codec_rejects_corrupt_header_and_does_not_guess_a_broken_record_length() {
    let (mut encoded, used) = encoded_pair();
    encoded[16] ^= 1;
    assert!(matches!(
        Decoder::new(&encoded[..used]),
        Err(DecodeError::Header)
    ));
    encoded[16] ^= 1;
    encoded[HEADER_BYTES + 4] ^= 1;
    let mut decoder = Decoder::new(&encoded[..used]).unwrap();
    assert_eq!(
        decoder.next(),
        Some(Err(DecodeError::TruncatedRecord {
            offset: HEADER_BYTES
        }))
    );
    assert!(decoder.next().is_none());
}
