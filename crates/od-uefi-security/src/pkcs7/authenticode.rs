//! PE Authenticode signed content and the PKCS#7 authenticated-attribute transcript.

use der::{
    Decode, Reader, SliceReader, Tag, Tagged,
    asn1::{AnyRef, ObjectIdentifier},
};
use sha2::{Digest, Sha256};

use super::{
    CONTEXT_ONE, CONTEXT_ZERO, Error, MAX_CERTIFICATES, SHA256, Signer, algorithm,
    ordered_sequence, raw_sequence, require_tag, sequence, signed_data_header, signed_data_tail,
};

const SPC_INDIRECT_DATA: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.6.1.4.1.311.2.1.4");
const SPC_PE_IMAGE_DATA: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.6.1.4.1.311.2.1.15");
const CONTENT_TYPE: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.3");
const MESSAGE_DIGEST: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.4");
const SPC_NESTED_SIGNATURE: ObjectIdentifier =
    ObjectIdentifier::new_unwrap("1.3.6.1.4.1.311.2.4.1");
const MAX_IMAGE_SIGNATURE_BYTES: usize = 64 * 1024;
const MAX_DER_DEPTH: usize = 32;
const DER_SET_TAG: u8 = 0x31;

#[derive(Debug)]
pub struct AuthenticodeSignedData<'a> {
    signer: Signer<'a>,
    image_digest: [u8; 32],
    signature_digest: [u8; 32],
    certificates: [Option<&'a [u8]>; MAX_CERTIFICATES],
    nested_signatures: Option<&'a [u8]>,
}

impl<'a> AuthenticodeSignedData<'a> {
    /// Reads the PKCS#7 data of a revision 2 WIN_CERTIFICATE.
    ///
    /// Validation here binds the decoded image digest to the signed attributes.
    /// The caller verifies the signature, certificate trust and actual PE digest.
    pub fn decode(data: &'a [u8]) -> Result<Self, Error> {
        if data.len() > MAX_IMAGE_SIGNATURE_BYTES {
            return Err(Error::TooLarge);
        }
        let mut input = SliceReader::new(data)?;
        let encoded = raw_sequence(&mut input)?;
        let padding = &data[encoded.len()..];
        if data.len() > encoded.len().next_multiple_of(8) || padding.iter().any(|&byte| byte != 0) {
            return Err(Error::Malformed);
        }

        let mut fields = signed_data_header(encoded, true)?;
        let mut content = SliceReader::new(require_tag(&mut fields, Tag::Sequence)?)?;
        if content.decode::<ObjectIdentifier>()? != SPC_INDIRECT_DATA {
            return Err(Error::UnsupportedProfile);
        }
        let indirect = require_tag(&mut content, CONTEXT_ZERO)?;
        content.finish()?;
        let (image_digest, content_digest) = indirect_data(indirect)?;
        let signed = signed_data_tail(fields, Some(&content_digest))?;
        Ok(Self {
            signer: signed.info.signer,
            image_digest,
            signature_digest: signed.info.signature_digest.ok_or(Error::Malformed)?,
            certificates: signed.certificates,
            nested_signatures: signed.info.nested_signatures,
        })
    }

    pub fn signer(&self) -> Signer<'a> {
        self.signer
    }

    /// Authenticode SHA-256 from the signed SpcIndirectDataContent.
    pub fn image_digest(&self) -> &[u8; 32] {
        &self.image_digest
    }

    /// SHA-256 of the DER SET OF signed attributes, for the RSA signature.
    pub fn signature_digest(&self) -> &[u8; 32] {
        &self.signature_digest
    }

    pub fn certificates(&self) -> impl Iterator<Item = &'a [u8]> {
        self.certificates.into_iter().flatten()
    }

    /// Exact DER ContentInfo values of nested PE signatures.
    pub fn nested_signatures(
        &self,
    ) -> Result<impl Iterator<Item = Result<&'a [u8], Error>>, Error> {
        let mut entries = SliceReader::new(self.nested_signatures.unwrap_or(&[]))?;
        Ok(core::iter::from_fn(move || {
            if entries.is_finished() {
                None
            } else {
                Some(raw_sequence(&mut entries))
            }
        }))
    }
}

fn indirect_data(encoded: &[u8]) -> Result<([u8; 32], [u8; 32]), Error> {
    let content = AnyRef::from_der(encoded)?;
    if content.tag() != Tag::Sequence {
        return Err(Error::Malformed);
    }
    let mut fields = SliceReader::new(content.value())?;
    let mut image = SliceReader::new(require_tag(&mut fields, Tag::Sequence)?)?;
    if image.decode::<ObjectIdentifier>()? != SPC_PE_IMAGE_DATA {
        return Err(Error::UnsupportedProfile);
    }
    if !image.is_finished() {
        // SpcPeImageData is signed metadata. It does not select bytes for the PE hash.
        let mut metadata: AnyRef<'_> = image.decode()?;
        if metadata.tag() == CONTEXT_ZERO {
            metadata = AnyRef::from_der(metadata.value())?;
        }
        if metadata.tag() != Tag::Sequence {
            return Err(Error::Malformed);
        }
        validate_constructed(metadata, 0)?;
    }
    image.finish()?;
    let digest = fields.sequence(|digest_info| -> Result<[u8; 32], Error> {
        if algorithm(digest_info)? != SHA256 {
            return Err(Error::UnsupportedProfile);
        }
        require_tag(digest_info, Tag::OctetString)?
            .try_into()
            .map_err(|_| Error::Malformed)
    })?;
    fields.finish()?;
    // PKCS#7 v1.5 §9.3 hashes the contents, excluding the SpcIndirectDataContent tag and length.
    Ok((digest, Sha256::digest(content.value()).into()))
}

pub(super) fn authenticated_attributes(
    fields: &mut SliceReader<'_>,
    content_digest: &[u8; 32],
) -> Result<[u8; 32], Error> {
    let encoded = fields.tlv_bytes()?;
    let attributes = AnyRef::from_der(encoded)?;
    if attributes.tag() != CONTEXT_ZERO {
        return Err(Error::UnsupportedProfile);
    }
    let mut attributes = SliceReader::new(attributes.value())?;
    let mut previous = None;
    let mut have_content_type = false;
    let mut have_digest = false;
    while !attributes.is_finished() {
        let encoded_attr = ordered_sequence(&mut attributes, &mut previous)?;
        let (oid, values) = attribute(encoded_attr)?;
        let mut values = SliceReader::new(values)?;
        match oid {
            CONTENT_TYPE if !have_content_type => {
                if values.decode::<ObjectIdentifier>()? != SPC_INDIRECT_DATA {
                    return Err(Error::Malformed);
                }
                values.finish()?;
                have_content_type = true;
            }
            MESSAGE_DIGEST if !have_digest => {
                if require_tag(&mut values, Tag::OctetString)? != content_digest {
                    return Err(Error::Malformed);
                }
                values.finish()?;
                have_digest = true;
            }
            CONTENT_TYPE | MESSAGE_DIGEST => return Err(Error::Malformed),
            _ => validate_attribute_values(&mut values)?,
        }
    }
    if !have_content_type || !have_digest {
        return Err(Error::Malformed);
    }
    // PKCS#7 uses the original DER length and content under the universal SET OF tag.
    Ok(Sha256::new()
        .chain_update([DER_SET_TAG])
        .chain_update(&encoded[1..])
        .finalize()
        .into())
}

pub(super) fn unauthenticated_attributes<'a>(
    fields: &mut SliceReader<'a>,
) -> Result<Option<&'a [u8]>, Error> {
    let mut entries = SliceReader::new(require_tag(fields, CONTEXT_ONE)?)?;
    if entries.is_finished() {
        return Err(Error::Malformed);
    }
    let mut previous = None;
    let mut nested = None;
    while !entries.is_finished() {
        let (oid, data) = attribute(ordered_sequence(&mut entries, &mut previous)?)?;
        let mut values = SliceReader::new(data)?;
        validate_attribute_values(&mut values)?;
        if oid == SPC_NESTED_SIGNATURE {
            if nested.replace(data).is_some() {
                return Err(Error::Malformed);
            }
            let mut nested_values = SliceReader::new(data)?;
            while !nested_values.is_finished() {
                raw_sequence(&mut nested_values)?;
            }
        }
    }
    Ok(nested)
}

fn attribute(encoded: &[u8]) -> Result<(ObjectIdentifier, &[u8]), Error> {
    let mut fields = sequence(encoded)?;
    let oid = fields.decode()?;
    let values = require_tag(&mut fields, Tag::Set)?;
    fields.finish()?;
    Ok((oid, values))
}

fn validate_attribute_values(values: &mut SliceReader<'_>) -> Result<(), Error> {
    if values.is_finished() {
        return Err(Error::Malformed);
    }
    let mut previous = None;
    while !values.is_finished() {
        let encoded = values.tlv_bytes()?;
        if previous
            .replace(encoded)
            .is_some_and(|before| before > encoded)
        {
            return Err(Error::Malformed);
        }
        validate_constructed(AnyRef::from_der(encoded)?, 0)?;
    }
    Ok(())
}

fn validate_constructed(value: AnyRef<'_>, depth: usize) -> Result<(), Error> {
    let tag = value.tag();
    if !tag.is_constructed() {
        return Ok(());
    }
    if depth >= MAX_DER_DEPTH {
        return Err(Error::TooLarge);
    }
    let mut children = SliceReader::new(value.value())?;
    let mut previous = None;
    while !children.is_finished() {
        let encoded = children.tlv_bytes()?;
        if tag == Tag::Set
            && previous
                .replace(encoded)
                .is_some_and(|before| before > encoded)
        {
            return Err(Error::Malformed);
        }
        validate_constructed(AnyRef::from_der(encoded)?, depth + 1)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pkcs7::{ID_SIGNED_DATA, RSA_ENCRYPTION};

    fn tlv(tag: u8, bytes: &[u8]) -> Vec<u8> {
        let mut out = vec![tag];
        match bytes.len() {
            len @ 0..=127 => out.push(len as u8),
            len @ 128..=255 => out.extend([0x81, len as u8]),
            len => {
                out.push(0x82);
                out.extend(u16::try_from(len).unwrap().to_be_bytes());
            }
        }
        out.extend(bytes);
        out
    }

    fn oid(id: ObjectIdentifier) -> Vec<u8> {
        tlv(0x06, id.as_bytes())
    }

    fn algorithm_id(id: ObjectIdentifier) -> Vec<u8> {
        tlv(0x30, &[oid(id), vec![0x05, 0]].concat())
    }

    fn indirect(image_digest: &[u8]) -> Vec<u8> {
        let metadata = tlv(0x30, &oid(SPC_PE_IMAGE_DATA));
        let digest_info = tlv(0x30, &[algorithm_id(SHA256), tlv(4, image_digest)].concat());
        [metadata, digest_info].concat()
    }

    fn encoded_attribute(id: ObjectIdentifier, value: Vec<u8>) -> Vec<u8> {
        tlv(0x30, &[oid(id), tlv(DER_SET_TAG, &value)].concat())
    }

    fn required_attributes(indirect: &[u8]) -> Vec<Vec<u8>> {
        let mut entries = vec![
            encoded_attribute(CONTENT_TYPE, oid(SPC_INDIRECT_DATA)),
            encoded_attribute(MESSAGE_DIGEST, tlv(4, &Sha256::digest(indirect))),
        ];
        entries.sort();
        entries
    }

    fn signed_data(indirect: &[u8], attributes: &[Vec<u8>], unsigned: &[u8]) -> Vec<u8> {
        let version = vec![2, 1, 1];
        let issuer_and_serial = vec![0x30, 5, 0x30, 0, 2, 1, 7];
        let signer = tlv(
            0x30,
            &[
                version.clone(),
                issuer_and_serial,
                algorithm_id(SHA256),
                tlv(0xa0, &attributes.concat()),
                algorithm_id(RSA_ENCRYPTION),
                tlv(4, &[0x78; 256]),
                unsigned.to_vec(),
            ]
            .concat(),
        );
        let content = tlv(
            0x30,
            &[oid(SPC_INDIRECT_DATA), tlv(0xa0, &tlv(0x30, indirect))].concat(),
        );
        let inner = tlv(
            0x30,
            &[
                version,
                tlv(DER_SET_TAG, &algorithm_id(SHA256)),
                content,
                vec![0xa0, 2, 0x30, 0],
                tlv(DER_SET_TAG, &signer),
            ]
            .concat(),
        );
        tlv(0x30, &[oid(ID_SIGNED_DATA), tlv(0xa0, &inner)].concat())
    }

    #[test]
    fn binds_the_signed_image_digest_and_uses_universal_set_tag_for_the_signature() {
        let indirect = indirect(&[0x5c; 32]);
        let attributes = required_attributes(&indirect);
        let unsigned = tlv(
            0xa1,
            &encoded_attribute(
                ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.6"),
                vec![0x30, 0],
            ),
        );
        let mut encoded = signed_data(&indirect, &attributes, &unsigned);
        encoded.resize(encoded.len().next_multiple_of(8), 0);
        let parsed = AuthenticodeSignedData::decode(&encoded).unwrap();
        assert_eq!(parsed.image_digest, [0x5c; 32]);
        assert_eq!(
            parsed.signature_digest,
            <[u8; 32]>::from(Sha256::digest(tlv(DER_SET_TAG, &attributes.concat())))
        );
        assert_eq!(parsed.signer.serial, &[7]);
        assert_eq!(parsed.signer.signature, &[0x78; 256]);
        assert!(parsed.certificates().eq([&b"\x30\x00"[..]]));
        encoded.push(1);
        assert_eq!(
            AuthenticodeSignedData::decode(&encoded).unwrap_err(),
            Error::Malformed
        );
        *encoded.last_mut().unwrap() = 0;
        assert_eq!(
            AuthenticodeSignedData::decode(&encoded).unwrap_err(),
            Error::Malformed
        );
    }

    #[test]
    fn rejects_inconsistent_missing_duplicate_or_unordered_signed_attributes() {
        let original = indirect(&[0x5c; 32]);
        let attributes = required_attributes(&original);
        let changed = indirect(&[0x5d; 32]);
        assert_eq!(
            AuthenticodeSignedData::decode(&signed_data(&changed, &attributes, &[])).unwrap_err(),
            Error::Malformed,
        );
        for invalid in [
            attributes[..1].to_vec(),
            vec![
                attributes[0].clone(),
                attributes[0].clone(),
                attributes[1].clone(),
            ],
            vec![attributes[1].clone(), attributes[0].clone()],
        ] {
            assert_eq!(
                AuthenticodeSignedData::decode(&signed_data(&original, &invalid, &[])).unwrap_err(),
                Error::Malformed,
            );
        }
        let short_digest = indirect(&[0x5c; 31]);
        let encoded = signed_data(&short_digest, &required_attributes(&short_digest), &[]);
        assert_eq!(
            AuthenticodeSignedData::decode(&encoded).unwrap_err(),
            Error::Malformed
        );
    }

    #[test]
    fn validates_structure_inside_opaque_metadata_and_attribute_values() {
        let mut deeply_nested = vec![0x30, 0];
        for _ in 0..MAX_DER_DEPTH {
            deeply_nested = tlv(0x30, &deeply_nested);
        }
        for (value, expected) in [
            (vec![0x30, 1, 2], Error::Malformed),
            (vec![0x30, 8, 0x31, 6, 2, 1, 2, 2, 1, 1], Error::Malformed),
            (deeply_nested, Error::TooLarge),
        ] {
            let content = indirect(&[0x5c; 32]);
            let mut attributes = required_attributes(&content);
            attributes.push(encoded_attribute(
                ObjectIdentifier::new_unwrap("1.3.6.1.4.1.311.2.1.12"),
                value.clone(),
            ));
            attributes.sort();
            let encoded = signed_data(&content, &attributes, &[]);
            assert_eq!(
                AuthenticodeSignedData::decode(&encoded).unwrap_err(),
                expected
            );

            let mut fields = SliceReader::new(&content).unwrap();
            raw_sequence(&mut fields).unwrap();
            let metadata = tlv(0x30, &[oid(SPC_PE_IMAGE_DATA), tlv(0xa0, &value)].concat());
            let content = [metadata, fields.tlv_bytes().unwrap().to_vec()].concat();
            let encoded = signed_data(&content, &required_attributes(&content), &[]);
            assert_eq!(
                AuthenticodeSignedData::decode(&encoded).unwrap_err(),
                expected
            );
        }
    }
}
