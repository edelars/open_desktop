//! RSA/SHA-256 PKCS#7 SignedData for Auth2, PE images and generic signed messages.

use der::{
    Decode, Reader, SliceReader, Tag, TagNumber, Tagged,
    asn1::{AnyRef, ObjectIdentifier, UintRef},
};

mod authenticode;
#[cfg(feature = "verify")]
pub(crate) mod timestamp;
#[cfg(feature = "protected-nvram")]
pub mod transport;
pub use authenticode::AuthenticodeSignedData;
#[cfg(feature = "verify")]
pub(crate) use authenticode::SPC_INDIRECT_DATA;

const ID_DATA: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.7.1");
const ID_SIGNED_DATA: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.7.2");
const SHA256: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.2.1");
const RSA_ENCRYPTION: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.1");
const SHA256_WITH_RSA: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.11");
const CONTEXT_ZERO: Tag = Tag::ContextSpecific {
    constructed: true,
    number: TagNumber(0),
};
const CONTEXT_ONE: Tag = Tag::ContextSpecific {
    constructed: true,
    number: TagNumber(1),
};

pub const MAX_SIGNED_DATA_BYTES: usize = 16 * 1024;
pub const MAX_MESSAGE_SIGNATURE_BYTES: usize = 64 * 1024;
pub const MAX_DATABASE_BYTES: usize = 64 * 1024;
pub(crate) const MAX_CERTIFICATES: usize = 8;
pub(crate) const RSA_2048_SIGNATURE_BYTES: usize = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Malformed,
    UnsupportedProfile,
    TooLarge,
}

impl From<der::Error> for Error {
    fn from(_: der::Error) -> Self {
        Self::Malformed
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Signer<'a> {
    pub issuer_der: &'a [u8],
    /// Unsigned big-endian certificate serial number, without DER sign padding.
    pub serial: &'a [u8],
    pub signature: &'a [u8; RSA_2048_SIGNATURE_BYTES],
}

#[derive(Debug)]
pub struct DetachedSignedData<'a> {
    pub signer: Signer<'a>,
    certificates: [Option<&'a [u8]>; MAX_CERTIFICATES],
}

impl<'a> DetachedSignedData<'a> {
    /// Parses the shape specified in UEFI 2.11 §8.2.6. Verification and trust are separate.
    pub fn decode(bytes: &'a [u8]) -> Result<Self, Error> {
        if bytes.len() > MAX_SIGNED_DATA_BYTES {
            return Err(Error::TooLarge);
        }
        let (mut fields, _) = signed_data_header(bytes, 1)?;
        let mut contents = SliceReader::new(require_tag(&mut fields, Tag::Sequence)?)?;
        if contents.decode::<ObjectIdentifier>()? != ID_DATA || !contents.is_finished() {
            return Err(Error::UnsupportedProfile);
        }
        let signed = signed_data_tail(fields, Attributes::Forbidden)?;
        Ok(Self {
            signer: signed.info.signer,
            certificates: signed.certificates,
        })
    }

    pub fn certificates(&self) -> impl Iterator<Item = &'a [u8]> {
        self.certificates.into_iter().flatten()
    }
}

/// A general PKCS#7 §9 message with an optional embedded value.
#[derive(Debug)]
pub struct MessageSignedData<'a> {
    signer: Signer<'a>,
    embedded: Option<&'a [u8]>,
    certificates: [Option<&'a [u8]>; MAX_CERTIFICATES],
    authenticated: Option<AuthenticatedAttributes<'a>>,
    content_type: ObjectIdentifier,
    #[cfg(feature = "verify")]
    unsigned_attributes: Option<&'a [u8]>,
}

impl<'a> MessageSignedData<'a> {
    pub fn decode(bytes: &'a [u8]) -> Result<Self, Error> {
        if bytes.len() > MAX_MESSAGE_SIGNATURE_BYTES {
            return Err(Error::TooLarge);
        }
        let (mut fields, wrapped) = signed_data_header(bytes, 1)?;
        let mut content = SliceReader::new(require_tag(&mut fields, Tag::Sequence)?)?;
        let content_type = content.decode::<ObjectIdentifier>()?;
        let embedded = if content.is_finished() {
            None
        } else {
            if !wrapped {
                return Err(Error::UnsupportedProfile);
            }
            let value = AnyRef::from_der(require_tag(&mut content, CONTEXT_ZERO)?)?;
            if content_type == ID_DATA && value.tag() != Tag::OctetString {
                return Err(Error::Malformed);
            }
            authenticode::validate_message_value(value)?;
            // RFC 2315 §9.3 signs the DER value octets, without the tag and length.
            Some(value.value())
        };
        content.finish()?;
        let signed = signed_data_tail(fields, Attributes::Message(content_type))?;
        Ok(Self {
            signer: signed.info.signer,
            embedded,
            certificates: signed.certificates,
            authenticated: signed.info.authenticated,
            content_type,
            #[cfg(feature = "verify")]
            unsigned_attributes: signed.info.unsigned_attributes,
        })
    }

    pub fn certificates(&self) -> impl Iterator<Item = &'a [u8]> {
        self.certificates.into_iter().flatten()
    }

    pub fn signer(&self) -> Signer<'a> {
        self.signer
    }

    pub fn embedded(&self) -> Option<&'a [u8]> {
        self.embedded
    }

    pub fn content_type(&self) -> ObjectIdentifier {
        self.content_type
    }

    /// Returns the digest used by the RSA signature if it covers the supplied message hash.
    pub fn signature_digest(&self, content_digest: [u8; 32]) -> Option<[u8; 32]> {
        match &self.authenticated {
            Some(attributes) => {
                (attributes.content_digest == content_digest).then_some(attributes.signature_digest)
            }
            None => Some(content_digest),
        }
    }
}

fn signed_data_header(bytes: &[u8], version: u8) -> Result<(SliceReader<'_>, bool), Error> {
    let mut fields = sequence(bytes)?;
    let wrapped = Tag::peek(&fields)? == Tag::ObjectIdentifier;
    if wrapped {
        let content_type: ObjectIdentifier = fields.decode()?;
        if content_type != ID_SIGNED_DATA {
            return Err(Error::UnsupportedProfile);
        }
        let signed_data = require_tag(&mut fields, CONTEXT_ZERO)?;
        fields.finish()?;
        fields = sequence(signed_data)?;
    }
    if fields.decode::<u8>()? != version {
        return Err(Error::UnsupportedProfile);
    }
    let mut digests = SliceReader::new(require_tag(&mut fields, Tag::Set)?)?;
    if algorithm(&mut digests)? != SHA256 {
        return Err(Error::UnsupportedProfile);
    }
    digests.finish()?;
    Ok((fields, wrapped))
}

struct SignedDataTail<'a> {
    info: SignerInfo<'a>,
    certificates: [Option<&'a [u8]>; MAX_CERTIFICATES],
}

struct SignerInfo<'a> {
    signer: Signer<'a>,
    authenticated: Option<AuthenticatedAttributes<'a>>,
    nested_signatures: Option<&'a [u8]>,
    #[cfg(feature = "verify")]
    unsigned_attributes: Option<&'a [u8]>,
}

#[derive(Debug)]
struct AuthenticatedAttributes<'a> {
    content_digest: [u8; 32],
    signature_digest: [u8; 32],
    #[cfg(feature = "verify")]
    entries: &'a [u8],
    #[cfg(not(feature = "verify"))]
    _entries: core::marker::PhantomData<&'a [u8]>,
}

#[derive(Clone, Copy)]
enum Attributes {
    Forbidden,
    Authenticode(ObjectIdentifier),
    Message(ObjectIdentifier),
}

fn signed_data_tail<'a>(
    mut fields: SliceReader<'a>,
    attributes: Attributes,
) -> Result<SignedDataTail<'a>, Error> {
    let mut certificates = [None; MAX_CERTIFICATES];
    let mut entries = SliceReader::new(require_tag(&mut fields, CONTEXT_ZERO)?)?;
    if entries.is_finished() {
        return Err(Error::Malformed);
    }
    let mut previous = None;
    for slot in &mut certificates {
        if entries.is_finished() {
            break;
        }
        *slot = Some(ordered_sequence(&mut entries, &mut previous)?);
    }
    if !entries.is_finished() {
        return Err(Error::TooLarge);
    }
    if Tag::peek(&fields)? == CONTEXT_ONE {
        // Optional CRLs are not authorities. UEFI image revocation uses dbx.
        let mut crls = SliceReader::new(require_tag(&mut fields, CONTEXT_ONE)?)?;
        let mut previous = None;
        while !crls.is_finished() {
            ordered_sequence(&mut crls, &mut previous)?;
        }
    }
    let mut signer_set = SliceReader::new(require_tag(&mut fields, Tag::Set)?)?;
    let info = signer_set.sequence(|info| parse_signer(info, attributes))?;
    if !signer_set.is_finished() {
        return Err(Error::UnsupportedProfile);
    }
    fields.finish()?;
    Ok(SignedDataTail { info, certificates })
}

fn parse_signer<'a>(
    fields: &mut SliceReader<'a>,
    attributes: Attributes,
) -> Result<SignerInfo<'a>, Error> {
    if fields.decode::<u8>()? != 1 {
        return Err(Error::UnsupportedProfile);
    }
    let mut id = SliceReader::new(require_tag(fields, Tag::Sequence)?)?;
    let issuer_der = raw_sequence(&mut id)?;
    let serial = id.decode::<UintRef<'a>>()?.as_bytes();
    id.finish()?;
    if algorithm(fields)? != SHA256 {
        return Err(Error::UnsupportedProfile);
    }
    let authenticated = match attributes {
        Attributes::Authenticode(content_type) => Some(authenticode::authenticated_attributes(
            fields,
            content_type,
            false,
        )?),
        Attributes::Message(content_type) if Tag::peek(fields)? == CONTEXT_ZERO => Some(
            authenticode::authenticated_attributes(fields, content_type, true)?,
        ),
        Attributes::Message(ID_DATA) => None,
        Attributes::Forbidden if Tag::peek(fields)? != CONTEXT_ZERO => None,
        _ => return Err(Error::UnsupportedProfile),
    };
    let oid = algorithm(fields)?;
    if oid != RSA_ENCRYPTION && oid != SHA256_WITH_RSA {
        return Err(Error::UnsupportedProfile);
    }
    let signature = require_tag(fields, Tag::OctetString)?
        .try_into()
        .map_err(|_| Error::UnsupportedProfile)?;
    let unsigned_attributes =
        if !fields.is_finished() && !matches!(attributes, Attributes::Forbidden) {
            Some(require_tag(fields, CONTEXT_ONE)?)
        } else {
            None
        };
    let nested_signatures = match unsigned_attributes {
        Some(entries) => authenticode::unauthenticated_attributes(
            entries,
            matches!(attributes, Attributes::Authenticode(_)),
        )?,
        None => None,
    };
    if !fields.is_finished() {
        return Err(Error::UnsupportedProfile);
    }
    Ok(SignerInfo {
        signer: Signer {
            issuer_der,
            serial,
            signature,
        },
        authenticated,
        nested_signatures,
        #[cfg(feature = "verify")]
        unsigned_attributes,
    })
}

fn algorithm(fields: &mut SliceReader<'_>) -> Result<ObjectIdentifier, Error> {
    fields.sequence(|parameters| {
        let oid = parameters.decode()?;
        if !parameters.is_finished() {
            parameters.decode::<der::asn1::Null>()?;
        }
        Ok(oid)
    })
}

pub(crate) fn sequence(bytes: &[u8]) -> Result<SliceReader<'_>, Error> {
    let top = AnyRef::from_der(bytes)?;
    if top.tag() != Tag::Sequence {
        return Err(Error::Malformed);
    }
    Ok(SliceReader::new(top.value())?)
}

fn require_tag<'a>(fields: &mut SliceReader<'a>, tag: Tag) -> Result<&'a [u8], Error> {
    let value: AnyRef<'a> = fields.decode()?;
    if value.tag() != tag {
        return Err(Error::Malformed);
    }
    Ok(value.value())
}

pub(crate) fn raw_sequence<'a>(fields: &mut SliceReader<'a>) -> Result<&'a [u8], Error> {
    if Tag::peek(fields)? != Tag::Sequence {
        return Err(Error::Malformed);
    }
    Ok(fields.tlv_bytes()?)
}

fn ordered_sequence<'a>(
    fields: &mut SliceReader<'a>,
    previous: &mut Option<&'a [u8]>,
) -> Result<&'a [u8], Error> {
    let encoded = raw_sequence(fields)?;
    if previous
        .replace(encoded)
        .is_some_and(|before| before > encoded)
    {
        return Err(Error::Malformed);
    }
    Ok(encoded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Guid, Name, authvar::Descriptor};

    const MICROSOFT_UPDATE: &[u8] = include_bytes!(
        "../../../tests/fixtures/firmware/certs/dbx-update-x64-v1.7.2-signed.efiauth2"
    );

    #[test]
    fn reads_detached_signed_data_from_a_microsoft_dbx_update() {
        let envelope = Descriptor::decode(MICROSOFT_UPDATE, true).unwrap();
        let parsed = DetachedSignedData::decode(envelope.signed_data).unwrap();
        assert_eq!(parsed.certificates().count(), 2);
        assert_eq!(parsed.signer.issuer_der[0], 0x30);
        assert_eq!(
            parsed.signer.serial,
            [
                0x33, 0, 0, 0, 0x08, 0xde, 0x1c, 0x6e, 0x41, 0x9f, 0x5b, 0x0c, 0x88, 0, 0, 0, 0, 0,
                8,
            ]
        );
        let vendor = Guid {
            data1: 0xd719_b2cb,
            data2: 0x3d3a,
            data3: 0x4596,
            data4: [0xa3, 0xbc, 0xda, 0xd0, 0x0e, 0x67, 0x65, 0x6f],
        };
        assert_eq!(
            envelope.sha256(Name::new(b"d\0b\0x\0").unwrap(), vendor, 0x67),
            [
                0x90, 0x8f, 0x44, 0x81, 0x06, 0x2a, 0xb2, 0xee, 0x42, 0x0f, 0x2d, 0xa7, 0x90, 0xb7,
                0x3e, 0xaa, 0x75, 0xfc, 0xac, 0x58, 0x96, 0x54, 0x02, 0x91, 0xc7, 0xdd, 0xc0, 0x5d,
                0x54, 0x2d, 0x66, 0x03,
            ]
        );
    }

    #[test]
    fn supports_content_info_wrapping_the_same_signed_data() {
        let envelope = Descriptor::decode(MICROSOFT_UPDATE, true).unwrap();
        let bare = DetachedSignedData::decode(envelope.signed_data).unwrap();
        let mut body = vec![
            0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x07, 0x02,
        ];
        body.extend([0xa0, 0x82]);
        body.extend((envelope.signed_data.len() as u16).to_be_bytes());
        body.extend(envelope.signed_data);
        let mut wrapped = vec![0x30, 0x82];
        wrapped.extend((body.len() as u16).to_be_bytes());
        wrapped.extend(body);
        let parsed = DetachedSignedData::decode(&wrapped).unwrap();
        assert!(parsed.certificates().eq(bare.certificates()));
        assert_eq!(parsed.signer.signature, bare.signer.signature);
        assert_eq!(parsed.signer.serial, bare.signer.serial);
    }

    #[test]
    fn rejects_truncation_noncanonical_version_content_and_excess_length() {
        let bare = Descriptor::decode(MICROSOFT_UPDATE, true)
            .unwrap()
            .signed_data;
        for length in 0..bare.len() {
            assert!(DetachedSignedData::decode(&bare[..length]).is_err());
        }
        let mut excess = bare.to_vec();
        excess.push(0);
        assert_eq!(
            DetachedSignedData::decode(&excess).unwrap_err(),
            Error::Malformed
        );
        assert_eq!(
            DetachedSignedData::decode(&[0; MAX_SIGNED_DATA_BYTES + 1]).unwrap_err(),
            Error::TooLarge
        );

        let mut wrong_version = bare.to_vec();
        assert_eq!(wrong_version[4..7], [2, 1, 1]);
        wrong_version[6] = 3;
        assert_eq!(
            DetachedSignedData::decode(&wrong_version).unwrap_err(),
            Error::UnsupportedProfile
        );

        let id = [
            0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x07, 0x01,
        ];
        let position = bare
            .windows(id.len())
            .position(|window| window == id)
            .unwrap();
        let mut wrong_content = bare.to_vec();
        wrong_content[position + id.len() - 1] = 2;
        assert_eq!(
            DetachedSignedData::decode(&wrong_content).unwrap_err(),
            Error::UnsupportedProfile
        );
    }

    #[test]
    fn rejects_noncanonical_certificate_order_in_a_der_set() {
        let bare = Descriptor::decode(MICROSOFT_UPDATE, true)
            .unwrap()
            .signed_data;
        let parsed = DetachedSignedData::decode(bare).unwrap();
        let mut certificates = parsed.certificates();
        let first = certificates.next().unwrap();
        let second = certificates.next().unwrap();
        let offset = bare
            .windows(first.len())
            .position(|input| input == first)
            .unwrap();
        let end = offset + first.len() + second.len();
        assert_eq!(&bare[offset + first.len()..end], second);
        let mut unsorted = bare.to_vec();
        unsorted[offset..offset + second.len()].copy_from_slice(second);
        unsorted[offset + second.len()..end].copy_from_slice(first);
        assert_eq!(
            DetachedSignedData::decode(&unsorted).unwrap_err(),
            Error::Malformed
        );
    }
}
