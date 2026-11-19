//! Processing for updates to the authoritative Secure Boot policy.

use std::collections::HashSet;

use od_uefi_security::{
    Guid, Name,
    authvar::Descriptor,
    pkcs7::{DetachedSignedData, MAX_SIGNED_DATA_BYTES},
    protected::{
        AUTHENTICATED_ATTRIBUTES, HEADER_BYTES, IMAGE_CAPACITY, Image, PROTECTED_VALUE_LIMIT,
        PolicyVariable, Record,
    },
    siglist::{self, SignatureType},
    verify::{self, TrustAnchor},
};

const REQUEST_HEADER_BYTES: usize = 24;
const APPEND_WRITE: u32 = 0x40;
// EFI_TIME plus the fixed WIN_CERTIFICATE_UEFI_GUID fields.
const DESCRIPTOR_HEADER_BYTES: usize = 40;
const SIGNATURE_LIST_HEADER_BYTES: usize = 28;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Error {
    InvalidRequest,
    PolicyRejected,
    NotFound,
    OutOfResources,
}

struct Request<'a> {
    guid: Guid,
    name: Name<'a>,
    policy: PolicyVariable,
    append: bool,
    attributes: u32,
    descriptor: Descriptor<'a>,
}

impl<'a> Request<'a> {
    fn decode(bytes: &'a [u8]) -> Result<Self, Error> {
        let header: &[u8; REQUEST_HEADER_BYTES] =
            bytes.first_chunk().ok_or(Error::InvalidRequest)?;
        let attributes = u32::from_le_bytes(header[..4].try_into().unwrap());
        if attributes & !APPEND_WRITE != AUTHENTICATED_ATTRIBUTES || header[6..8] != [0; 2] {
            return Err(Error::InvalidRequest);
        }
        let units = usize::from(u16::from_le_bytes(header[4..6].try_into().unwrap()));
        let name_end = REQUEST_HEADER_BYTES + 2 * units;
        let name = bytes
            .get(REQUEST_HEADER_BYTES..name_end)
            .and_then(Name::new)
            .ok_or(Error::InvalidRequest)?;
        let guid = Guid {
            data1: u32::from_le_bytes(header[8..12].try_into().unwrap()),
            data2: u16::from_le_bytes(header[12..14].try_into().unwrap()),
            data3: u16::from_le_bytes(header[14..16].try_into().unwrap()),
            data4: header[16..24].try_into().unwrap(),
        };
        let policy = PolicyVariable::from_key(guid, name).ok_or(Error::InvalidRequest)?;
        let append = attributes & APPEND_WRITE != 0;
        let data = &bytes[name_end..];
        if data.len() > DESCRIPTOR_HEADER_BYTES + MAX_SIGNED_DATA_BYTES + PROTECTED_VALUE_LIMIT {
            return Err(Error::OutOfResources);
        }
        let descriptor = Descriptor::decode(data, append).map_err(|_| Error::PolicyRejected)?;
        if descriptor.value.len() > PROTECTED_VALUE_LIMIT
            || descriptor.signed_data.len() > MAX_SIGNED_DATA_BYTES
        {
            return Err(Error::OutOfResources);
        }
        Ok(Self {
            guid,
            name,
            policy,
            append,
            attributes,
            descriptor,
        })
    }
}

/// Checks an update against the current bank and returns the full proposed store image.
pub(super) fn prepare(current: &[u8], wire: &[u8]) -> Result<Option<Vec<u8>>, Error> {
    let request = Request::decode(wire)?;
    let image = Image::new(current).map_err(|_| Error::PolicyRejected)?;
    let records = image
        .records()
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| Error::PolicyRejected)?;
    let old = find(&records, request.policy);
    let old_time = old.and_then(|record| record.watermark);
    let (stage_time, incoming) = (request.descriptor.timestamp, request.descriptor.value);
    if !request.append && stage_time <= old_time {
        return Err(Error::PolicyRejected);
    }
    let setup_mode = authorize(&request, &records)?;
    validate_payload(request.policy, incoming)?;

    let existing_value = old.map_or(&[][..], |record| record.value);
    let appended;
    let value = if request.append {
        appended = append_unique(existing_value, incoming)?;
        &appended[..]
    } else {
        if incoming.is_empty() && existing_value.is_empty() {
            return Err(Error::NotFound);
        }
        incoming
    };
    if request.append && request.policy == PolicyVariable::Pk {
        validate_payload(request.policy, value)?;
    }
    let watermark = old_time.max(stage_time);
    if value == existing_value && watermark == old_time {
        return Ok(None);
    }
    if value.is_empty() && old.is_none() {
        // Empty append acknowledges the authenticated request without creating a visible variable.
        return Ok(None);
    }

    let replacement = Record {
        guid: request.guid,
        name: request.name,
        attributes: AUTHENTICATED_ATTRIBUTES,
        value,
        watermark,
    };
    let mut output = vec![0; IMAGE_CAPACITY];
    let mut position = HEADER_BYTES;
    for (record, encoded) in &records {
        if record.policy_variable() == Some(request.policy) {
            position += replacement
                .write(&mut output[position..])
                .map_err(|_| Error::OutOfResources)?;
        } else {
            let end = position + encoded.len();
            output
                .get_mut(position..end)
                .ok_or(Error::OutOfResources)?
                .copy_from_slice(encoded);
            position = end;
        }
    }
    if old.is_none() {
        position += replacement
            .write(&mut output[position..])
            .map_err(|_| Error::OutOfResources)?;
    }
    output.truncate(position);
    let mut flags = image.flags;
    flags.vendor_keys_modified |= setup_mode && value != existing_value;
    Image::write_header(&mut output, flags).map_err(|_| Error::OutOfResources)?;
    Image::new(&output)
        .and_then(|image| image.policy_bytes())
        .map_err(|_| Error::OutOfResources)?;
    Ok(Some(output))
}

fn find<'a>(records: &[(Record<'a>, &'a [u8])], kind: PolicyVariable) -> Option<Record<'a>> {
    records
        .iter()
        .find_map(|(record, _)| (record.policy_variable() == Some(kind)).then_some(*record))
}

fn authorize(request: &Request<'_>, records: &[(Record<'_>, &[u8])]) -> Result<bool, Error> {
    let signed = if request.descriptor.signed_data.is_empty() {
        None
    } else {
        Some(
            DetachedSignedData::decode(request.descriptor.signed_data)
                .map_err(|_| Error::PolicyRejected)?,
        )
    };
    let Some(pk) = find(records, PolicyVariable::Pk).filter(|record| !record.value.is_empty())
    else {
        // SetupMode skips signature and signer checks. The Auth2 layout and time still apply.
        return Ok(true);
    };
    let signed = signed.ok_or(Error::PolicyRejected)?;
    let digest = request
        .descriptor
        .sha256(request.name, request.guid, request.attributes);
    let mut anchors = Vec::new();
    for list in siglist::signature_lists(pk.value) {
        let list = list.map_err(|_| Error::PolicyRejected)?;
        if list.kind != SignatureType::X509 {
            return Err(Error::PolicyRejected);
        }
        anchors.extend(
            list.signatures()
                .map(|signature| TrustAnchor::PlatformKey(signature.data())),
        );
    }
    if !matches!(anchors.as_slice(), [TrustAnchor::PlatformKey(_)]) {
        return Err(Error::PolicyRejected);
    }
    if !matches!(request.policy, PolicyVariable::Pk | PolicyVariable::Kek) {
        if let Some(kek) = find(records, PolicyVariable::Kek) {
            for list in siglist::signature_lists(kek.value) {
                let list = list.map_err(|_| Error::PolicyRejected)?;
                if list.kind != SignatureType::X509 {
                    return Err(Error::PolicyRejected);
                }
                anchors.extend(
                    list.signatures()
                        .map(|signature| TrustAnchor::ExchangeKey(signature.data())),
                );
            }
        }
    }
    verify::detached(&signed, &digest, &anchors).map_err(|_| Error::PolicyRejected)?;
    Ok(false)
}

fn validate_payload(policy: PolicyVariable, value: &[u8]) -> Result<(), Error> {
    if value.is_empty() {
        return Ok(());
    }
    let mut count = 0;
    for list in siglist::signature_lists(value) {
        let list = list.map_err(|_| Error::InvalidRequest)?;
        let allowed = match policy {
            PolicyVariable::Pk | PolicyVariable::Kek | PolicyVariable::Dbt => {
                list.kind == SignatureType::X509
            }
            PolicyVariable::Db | PolicyVariable::Dbr => {
                matches!(list.kind, SignatureType::X509 | SignatureType::Sha256)
            }
            PolicyVariable::Dbx => true,
        };
        if !allowed {
            return Err(Error::InvalidRequest);
        }
        for signature in list.signatures() {
            count += 1;
            if list.kind == SignatureType::X509 {
                let certificate = signature.data();
                let result = match policy {
                    PolicyVariable::Pk => {
                        verify::validate_anchor(TrustAnchor::PlatformKey(certificate))
                    }
                    PolicyVariable::Kek => {
                        verify::validate_anchor(TrustAnchor::ExchangeKey(certificate))
                    }
                    _ => verify::validate_certificate(certificate),
                };
                result.map_err(|_| Error::InvalidRequest)?;
            }
        }
    }
    if policy == PolicyVariable::Pk && count != 1 {
        return Err(Error::InvalidRequest);
    }
    Ok(())
}

fn append_unique(original: &[u8], extra: &[u8]) -> Result<Vec<u8>, Error> {
    let mut seen = HashSet::new();
    for list in siglist::signature_lists(original) {
        let list = list.map_err(|_| Error::PolicyRejected)?;
        seen.extend(
            list.signatures()
                .map(|signature| (list.kind, signature.bytes())),
        );
    }
    let mut output = original.to_vec();
    for list in siglist::signature_lists(extra) {
        let list = list.map_err(|_| Error::InvalidRequest)?;
        let start = output.len();
        for signature in list.signatures() {
            let bytes = signature.bytes();
            if !seen.insert((list.kind, bytes)) {
                continue;
            }
            if output.len() == start {
                output.extend(list.kind.guid_bytes());
                output.extend([0; 8]);
                output.extend((bytes.len() as u32).to_le_bytes());
            }
            output.extend(bytes);
            if output.len() > PROTECTED_VALUE_LIMIT {
                return Err(Error::OutOfResources);
            }
        }
        if output.len() != start {
            let list_size = (output.len() - start) as u32;
            output[start + 16..start + 20].copy_from_slice(&list_size.to_le_bytes());
            debug_assert!(list_size as usize > SIGNATURE_LIST_HEADER_BYTES);
        }
    }
    Ok(output)
}
