//! Packed EFI_LOAD_OPTION records from UEFI 2.11 §3.1.3.

use crate::device_path::path_prefix;

const HEADER_BYTES: usize = 6;
pub const ACTIVE: u32 = 1;
const FORCE_RECONNECT: u32 = 2;
const HIDDEN: u32 = 8;
const CATEGORY_MASK: u32 = 0x1f00;
const APPLICATION: u32 = 0x0100;

#[derive(Debug, PartialEq, Eq)]
pub struct LoadOption<'a> {
    pub attributes: u32,
    /// Description as little-endian CHAR16 units, excluding the NUL.
    pub description: &'a [u8],
    pub file_path_list: &'a [u8],
    /// First device path in the list, including End Entire.
    pub boot_path: &'a [u8],
    pub optional_data: &'a [u8],
}

impl<'a> LoadOption<'a> {
    pub fn parse(input: &'a [u8]) -> Option<Self> {
        let (header, body) = input.split_at_checked(HEADER_BYTES)?;
        let attributes = u32::from_le_bytes(header[..4].try_into().ok()?);
        let path_bytes = usize::from(u16::from_le_bytes(header[4..].try_into().ok()?));
        let mut description_end = None;
        for (index, bytes) in body.chunks_exact(2).enumerate() {
            let unit = u16::from_le_bytes([bytes[0], bytes[1]]);
            if unit == 0 {
                description_end = Some(2 * index);
                break;
            }
            if !valid_character(unit) {
                return None;
            }
        }
        let description_end = description_end?;
        let paths_and_data = body.get(description_end + 2..)?;
        let (file_path_list, optional_data) = paths_and_data.split_at_checked(path_bytes)?;
        let boot_path = validate_paths(file_path_list)?;
        Some(Self {
            attributes,
            description: &body[..description_end],
            file_path_list,
            boot_path,
            optional_data,
        })
    }

    pub fn is_active_boot(&self) -> bool {
        self.attributes & (ACTIVE | CATEGORY_MASK) == ACTIVE
    }
}

/// Encode a newly created option, with all reserved attribute bits zero.
pub fn encode_new(
    attributes: u32,
    description: &[u16],
    file_path_list: &[u8],
    optional_data: &[u8],
    output: &mut [u8],
) -> Option<usize> {
    if attributes & !(ACTIVE | FORCE_RECONNECT | HIDDEN | APPLICATION) != 0
        || !description.iter().copied().all(valid_character)
    {
        return None;
    }
    let path_size = u16::try_from(file_path_list.len()).ok()?;
    validate_paths(file_path_list)?;
    let size = encoded_bytes(description.len(), file_path_list.len(), optional_data.len())?;
    let data_start = size - optional_data.len();
    let path_start = data_start - file_path_list.len();
    let output = output.get_mut(..size)?;
    output[..4].copy_from_slice(&attributes.to_le_bytes());
    output[4..HEADER_BYTES].copy_from_slice(&path_size.to_le_bytes());
    let (characters, nul) =
        output[HEADER_BYTES..path_start].split_at_mut(path_start - HEADER_BYTES - 2);
    for (unit, bytes) in description.iter().zip(characters.chunks_exact_mut(2)) {
        bytes.copy_from_slice(&unit.to_le_bytes());
    }
    nul.fill(0);
    output[path_start..data_start].copy_from_slice(file_path_list);
    output[data_start..].copy_from_slice(optional_data);
    Some(size)
}

pub(super) fn encoded_bytes(
    description_units: usize,
    path_bytes: usize,
    data_bytes: usize,
) -> Option<usize> {
    HEADER_BYTES
        .checked_add(description_units.checked_add(1)?.checked_mul(2)?)?
        .checked_add(path_bytes)?
        .checked_add(data_bytes)
}

fn valid_character(unit: u16) -> bool {
    unit != 0 && !(0xd800..=0xdfff).contains(&unit)
}

fn validate_paths(bytes: &[u8]) -> Option<&[u8]> {
    let first = path_prefix(bytes)?;
    let mut remaining = &bytes[first.len()..];
    while !remaining.is_empty() {
        let path = path_prefix(remaining)?;
        remaining = &remaining[path.len()..];
    }
    Some(first)
}

#[cfg(any(target_os = "none", test))]
pub(super) fn is_short_media_path(instance: &[u8]) -> bool {
    instance.starts_with(&[4, 4]) || instance.starts_with(&[4, 1])
}

/// Expand file-only and GPT Hard Drive short forms on the fixed-disk profile.
/// The device prefix is the validated path installed by the ESP partition.
#[cfg(any(target_os = "none", test))]
pub(super) fn short_media_suffix<'a>(instance: &'a [u8], device: &[u8]) -> Option<&'a [u8]> {
    if instance.starts_with(&[4, 4]) {
        return Some(instance);
    }
    const HD: [u8; 4] = [4, 1, 42, 0];
    let (request, suffix) = instance.split_at_checked(42)?;
    let installed = device.get(device.len().checked_sub(42)?..)?;
    if request[..4] != HD || installed[..4] != HD || request[40..42] != [2, 2] {
        return None;
    }
    // Match GPT identity and partition number. The device may have moved LBAs.
    (installed[4..8] == request[4..8] && installed[24..42] == request[24..42]).then_some(suffix)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATH: &[u8] = &[4, 4, 8, 0, b'X', 0, 0, 0, 0x7f, 0xff, 4, 0];

    #[test]
    fn load_option_round_trips_a_packed_record_and_preserves_optional_bytes() {
        let mut buffer = [0xa5; 35];
        let size = encode_new(
            ACTIVE | HIDDEN,
            &[0x00e9],
            PATH,
            &[0xff, 0, 0x70],
            &mut buffer[1..],
        );
        assert_eq!(size, Some(25));
        let expected = [
            9, 0, 0, 0, 12, 0, 0xe9, 0, 0, 0, 4, 4, 8, 0, b'X', 0, 0, 0, 0x7f, 0xff, 4, 0, 0xff, 0,
            0x70,
        ];
        assert_eq!(&buffer[1..26], &expected);
        assert_eq!((buffer[0], buffer[26]), (0xa5, 0xa5));
        let parsed = LoadOption::parse(&buffer[1..26]).unwrap();
        assert!(parsed.is_active_boot());
        assert_eq!(parsed.description, &[0xe9, 0]);
        assert_eq!(parsed.file_path_list, PATH);
        assert_eq!(parsed.boot_path, PATH);
        assert_eq!(parsed.optional_data, &[0xff, 0, 0x70]);
        for size in 0..22 {
            assert_eq!(LoadOption::parse(&expected[..size]), None, "size {size}");
        }
    }

    #[test]
    fn load_option_validates_descendants_and_selects_the_first_complete_path() {
        let mut paths = [0; 3 * PATH.len()];
        for item in paths.chunks_exact_mut(PATH.len()) {
            item.copy_from_slice(PATH);
        }
        // The first complete path contains two instances. The last path is separate.
        paths[PATH.len() - 3] = 1;
        let mut buffer = [0; 64];
        let size = encode_new(ACTIVE | APPLICATION, &[], &paths, &[], &mut buffer).unwrap();
        let record = LoadOption::parse(&buffer[..size]).unwrap();
        assert!(!record.is_active_boot());
        assert_eq!(record.boot_path, &paths[..2 * PATH.len()]);
        assert_eq!(record.file_path_list, &paths);
        buffer[0] = 0; // Inactive.
        assert!(!LoadOption::parse(&buffer[..size]).unwrap().is_active_boot());
        buffer[0] = 0x81; // Unknown flag is preserved when decoding.
        buffer[1] = 0;
        let record = LoadOption::parse(&buffer[..size]).unwrap();
        assert_eq!(record.attributes, 0x81);
        assert!(record.is_active_boot());
        buffer[size - 2] = 3; // The second full path has an invalid terminator.
        assert_eq!(LoadOption::parse(&buffer[..size]), None);
    }

    #[test]
    fn load_option_rejects_malformed_descriptions_and_fails_before_writing() {
        let mut buffer = [0xa5; 32];
        assert_eq!(encode_new(ACTIVE, &[0xd800], PATH, &[], &mut buffer), None);
        assert_eq!(encode_new(ACTIVE, &[0], PATH, &[], &mut buffer), None);
        assert_eq!(encode_new(ACTIVE, &[], &[], &[], &mut buffer), None);
        assert_eq!(encode_new(0x80, &[], PATH, &[], &mut buffer), None);
        assert_eq!(encode_new(ACTIVE, &[], PATH, &[], &mut buffer[..19]), None);
        assert_eq!(buffer, [0xa5; 32]);
        let size = encode_new(ACTIVE, &[0x61], PATH, &[], &mut buffer).unwrap();
        buffer[7] = 0xd8;
        assert_eq!(LoadOption::parse(&buffer[..size]), None);
        assert_eq!(LoadOption::parse(&[1, 0, 0, 0, 1, 0, 24, 0]), None);
    }

    #[test]
    fn fixed_esp_short_paths_match_gpt_signature_and_partition_number() {
        let mut device = [0; 46];
        device[..4].copy_from_slice(&[1, 1, 4, 0]);
        device[4..8].copy_from_slice(&[4, 1, 42, 0]);
        device[8] = 3;
        device[28..44].fill(0x71);
        device[44..].copy_from_slice(&[2, 2]);
        let mut short = [0; 50];
        short[..42].copy_from_slice(&device[4..]);
        short[8] = 0xff;
        short[42..].copy_from_slice(&PATH[..8]);
        assert!(is_short_media_path(&short));
        assert!(!is_short_media_path(&device));
        assert_eq!(short_media_suffix(&short, &device), Some(&PATH[..8]));
        assert_eq!(short_media_suffix(&PATH[..8], &device), Some(&PATH[..8]));
        short[24] ^= 1;
        assert_eq!(short_media_suffix(&short, &device), None);
        short[24] ^= 1;
        short[4] = 4;
        assert_eq!(short_media_suffix(&short, &device), None);
    }
}
