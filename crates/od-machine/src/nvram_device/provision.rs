//! Cold host provisioning of the protected UEFI variable banks.

use od_uefi_security::{
    Guid, Name,
    protected::{
        AUTHENTICATED_ATTRIBUTES, Flags, GLOBAL_VARIABLE, HEADER_BYTES, IMAGE_CAPACITY,
        IMAGE_SECURITY_DATABASE, Image, PROTECTED_VALUE_LIMIT, PolicyVariable, Record,
    },
    siglist::{self, SignatureType},
    verify,
};

use super::{NvramContentsError, SecureBootKeys, authenticated::validate_payload};

// SignatureOwner used in Microsoft's v1.7.2 firmware signature lists.
const MICROSOFT_OWNER: [u8; 16] = [
    0xbd, 0x9a, 0xfa, 0x77, 0x59, 0x03, 0x32, 0x4d, 0xbd, 0x60, 0x28, 0xf4, 0xe7, 0x8f, 0x78, 0x4b,
];
const PLATFORM_OWNER: [u8; 16] = [
    0xe3, 0xfe, 0xe9, 0x59, 0x00, 0x61, 0x0d, 0x4a, 0xad, 0x96, 0xa6, 0xc8, 0xf6, 0x3a, 0xaf, 0x68,
];
const SIGNATURE_OVERHEAD: usize = 28 + 16;

const DB_CERTIFICATES: [&[u8]; 4] = [
    include_bytes!("../../../../tests/fixtures/firmware/certs/windows-production-pca-2011.der"),
    include_bytes!("../../../../tests/fixtures/firmware/certs/windows-uefi-ca-2023.der"),
    include_bytes!("../../../../tests/fixtures/firmware/certs/microsoft-uefi-ca-2011.der"),
    include_bytes!("../../../../tests/fixtures/firmware/certs/microsoft-uefi-ca-2023.der"),
];
const KEK_CERTIFICATES: [&[u8]; 2] = [
    include_bytes!("../../../../tests/fixtures/firmware/certs/microsoft-kek-ca-2011.der"),
    include_bytes!("../../../../tests/fixtures/firmware/certs/microsoft-kek-ca-2023.der"),
];
const DBX: &[u8] = include_bytes!("../../../../tests/fixtures/firmware/certs/dbx-x64-v1.7.2.bin");

pub(super) fn new_image(
    keys: SecureBootKeys<'_>,
    previous: Option<&[u8]>,
) -> Result<Vec<u8>, NvramContentsError> {
    if let SecureBootKeys::Defaults {
        public_platform_key,
    } = keys
    {
        if public_platform_key.len() > PROTECTED_VALUE_LIMIT - SIGNATURE_OVERHEAD {
            return Err(NvramContentsError::InvalidPlatformKey);
        }
    }
    let invalid = |_| NvramContentsError::InvalidProtectedStore;
    let mut output = vec![0; IMAGE_CAPACITY];
    let mut position = HEADER_BYTES;
    if let Some(previous) = previous {
        let image = Image::new(previous).map_err(invalid)?;
        for decoded in image.records() {
            let (record, bytes) = decoded.map_err(invalid)?;
            match record.policy_variable() {
                None => {
                    let end = position + bytes.len();
                    output[position..end].copy_from_slice(bytes);
                    position = end;
                }
                Some(PolicyVariable::Pk) if !record.value.is_empty() => {
                    if let SecureBootKeys::Defaults {
                        public_platform_key,
                    } = keys
                    {
                        reject_reused_key(record.value, public_platform_key)?;
                    }
                }
                Some(_) => {}
            }
        }
    }

    if let SecureBootKeys::Defaults {
        public_platform_key,
    } = keys
    {
        let pk = x509_list(PLATFORM_OWNER, &[public_platform_key]);
        let db = x509_list(MICROSOFT_OWNER, &DB_CERTIFICATES);
        let kek = x509_list(MICROSOFT_OWNER, &KEK_CERTIFICATES);
        let records: [(PolicyVariable, Guid, &[u8], &[u8]); 4] = [
            (PolicyVariable::Pk, GLOBAL_VARIABLE, b"P\0K\0", &pk),
            (PolicyVariable::Kek, GLOBAL_VARIABLE, b"K\0E\0K\0", &kek),
            (PolicyVariable::Db, IMAGE_SECURITY_DATABASE, b"d\0b\0", &db),
            (
                PolicyVariable::Dbx,
                IMAGE_SECURITY_DATABASE,
                b"d\0b\0x\0",
                DBX,
            ),
        ];
        for (policy, guid, name, value) in records {
            validate_payload(policy, value).map_err(|_| {
                if policy == PolicyVariable::Pk {
                    NvramContentsError::InvalidPlatformKey
                } else {
                    NvramContentsError::InvalidProtectedStore
                }
            })?;
            let record = Record {
                guid,
                name: Name::new(name).unwrap(),
                attributes: AUTHENTICATED_ATTRIBUTES,
                value,
                watermark: None,
            };
            position += record.write(&mut output[position..]).map_err(invalid)?;
        }
    }
    output.truncate(position);
    Image::write_header(
        &mut output,
        Flags {
            secure_boot_disabled: false,
            vendor_keys_modified: false,
        },
    )
    .map_err(invalid)?;
    Ok(output)
}

fn reject_reused_key(old: &[u8], new: &[u8]) -> Result<(), NvramContentsError> {
    validate_payload(PolicyVariable::Pk, old)
        .map_err(|_| NvramContentsError::InvalidProtectedStore)?;
    let old = siglist::signature_lists(old)
        .filter_map(Result::ok)
        .flat_map(|list| list.signatures())
        .next()
        .ok_or(NvramContentsError::InvalidProtectedStore)?;
    if verify::same_rsa_public_key(old.data(), new)
        .map_err(|_| NvramContentsError::InvalidPlatformKey)?
    {
        return Err(NvramContentsError::ReusedPlatformKey);
    }
    Ok(())
}

fn x509_list(owner: [u8; 16], certs: &[&[u8]]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for cert in certs {
        bytes.extend(SignatureType::X509.guid_bytes());
        bytes.extend(((SIGNATURE_OVERHEAD + cert.len()) as u32).to_le_bytes());
        bytes.extend([0; 4]);
        bytes.extend(((16 + cert.len()) as u32).to_le_bytes());
        bytes.extend(owner);
        bytes.extend(*cert);
    }
    bytes
}
