//! Bounded RFC 3161 time-stamp tokens and the signed TSA certificate identifier.

use alloc::vec::Vec;
use core::time::Duration;
use der::{
    DateTime, Reader, SliceReader, Tag, TagNumber,
    asn1::{AnyRef, ObjectIdentifier, UintRef},
};
use sha2::{Digest, Sha256};

use super::{
    Attributes, CONTEXT_ONE, CONTEXT_ZERO, Error, MAX_CERTIFICATES, MessageSignedData, SHA256,
    Signer, algorithm, raw_sequence, require_tag, sequence, signed_data_header, signed_data_tail,
};

const TST_INFO: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.16.1.4");
const TIMESTAMP_TOKEN: ObjectIdentifier =
    ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.16.2.14");
const MICROSOFT_TIMESTAMP_TOKEN: ObjectIdentifier =
    ObjectIdentifier::new_unwrap("1.3.6.1.4.1.311.3.3.1");
const SIGNING_CERTIFICATE: ObjectIdentifier =
    ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.16.2.12");
const SIGNING_CERTIFICATE_V2: ObjectIdentifier =
    ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.16.2.47");
const MAX_TOKEN_BYTES: usize = 16 * 1024;
const MAX_TOKENS: usize = 4;
const DIRECTORY_NAME: Tag = Tag::ContextSpecific {
    constructed: true,
    number: TagNumber(4),
};

pub(crate) fn tokens<'a>(message: &MessageSignedData<'a>) -> Result<Vec<&'a [u8]>, Error> {
    let mut found = Vec::new();
    let mut fields = SliceReader::new(message.unsigned_attributes.unwrap_or(&[]))?;
    while !fields.is_finished() {
        let (oid, values) = super::authenticode::attribute(raw_sequence(&mut fields)?)?;
        if [TIMESTAMP_TOKEN, MICROSOFT_TIMESTAMP_TOKEN].contains(&oid) {
            let mut values = SliceReader::new(values)?;
            while !values.is_finished() {
                if found.len() == MAX_TOKENS {
                    return Err(Error::TooLarge);
                }
                found.push(raw_sequence(&mut values)?);
            }
        }
    }
    Ok(found)
}

#[derive(Clone, Copy)]
pub(crate) struct IssuerSerial<'a> {
    pub issuer: &'a [u8],
    pub serial: &'a [u8],
}

#[derive(Clone, Copy)]
pub(crate) enum CertificateHash<'a> {
    Sha1(&'a [u8]),
    Sha256(&'a [u8]),
}

pub(crate) struct TimestampToken<'a> {
    pub signer: Signer<'a>,
    certificates: [Option<&'a [u8]>; MAX_CERTIFICATES],
    pub signature_digest: [u8; 32],
    pub certificate_hash: CertificateHash<'a>,
    pub issuer_serial: Option<IssuerSerial<'a>>,
    pub tsa_name: Option<&'a [u8]>,
    pub time: Duration,
    /// Latest time of creation represented by the TSA's declared accuracy.
    pub latest_time: Duration,
}

impl<'a> TimestampToken<'a> {
    pub fn decode(token: &'a [u8], signature: &[u8]) -> Result<Self, Error> {
        if token.len() > MAX_TOKEN_BYTES {
            return Err(Error::TooLarge);
        }
        let (mut fields, wrapped) = signed_data_header(token, 3)?;
        if !wrapped {
            return Err(Error::UnsupportedProfile);
        }
        let mut content = SliceReader::new(require_tag(&mut fields, Tag::Sequence)?)?;
        if content.decode::<ObjectIdentifier>()? != TST_INFO {
            return Err(Error::UnsupportedProfile);
        }
        let embedded = require_tag(&mut content, CONTEXT_ZERO)?;
        content.finish()?;
        let mut embedded = SliceReader::new(embedded)?;
        let tst_info = require_tag(&mut embedded, Tag::OctetString)?;
        embedded.finish()?;
        let signed = signed_data_tail(fields, Attributes::Message(TST_INFO))?;
        let attrs = signed.info.authenticated.ok_or(Error::Malformed)?;
        if attrs.content_digest != <[u8; 32]>::from(Sha256::digest(tst_info)) {
            return Err(Error::Malformed);
        }
        let (certificate_hash, issuer_serial) = certificate_identifier(attrs.entries)?;
        let (time, accuracy, tsa_name) = parse_tst_info(tst_info, signature)?;
        Ok(Self {
            signer: signed.info.signer,
            certificates: signed.certificates,
            signature_digest: attrs.signature_digest,
            certificate_hash,
            issuer_serial,
            tsa_name,
            time,
            latest_time: time.checked_add(accuracy).ok_or(Error::Malformed)?,
        })
    }

    pub fn certificates(&self) -> impl Iterator<Item = &'a [u8]> {
        self.certificates.into_iter().flatten()
    }
}

fn certificate_identifier(
    attrs: &[u8],
) -> Result<(CertificateHash<'_>, Option<IssuerSerial<'_>>), Error> {
    let mut fields = SliceReader::new(attrs)?;
    let mut identifier = None;
    while !fields.is_finished() {
        let (oid, data) = super::authenticode::attribute(raw_sequence(&mut fields)?)?;
        if ![SIGNING_CERTIFICATE, SIGNING_CERTIFICATE_V2].contains(&oid) {
            continue;
        }
        if identifier.is_some() {
            return Err(Error::Malformed);
        }
        let mut attr = sequence(data)?;
        let mut certs = SliceReader::new(require_tag(&mut attr, Tag::Sequence)?)?;
        let mut cert = SliceReader::new(require_tag(&mut certs, Tag::Sequence)?)?;
        // One ESSCertID is supported. Additional IDs restrict the acceptable chain.
        certs.finish()?;
        if !attr.is_finished() {
            return Err(Error::UnsupportedProfile);
        }
        let sha1 = oid == SIGNING_CERTIFICATE;
        if !sha1 && peek(&cert, Tag::Sequence)? && algorithm(&mut cert)? != SHA256 {
            return Err(Error::UnsupportedProfile);
        }
        let hash = require_tag(&mut cert, Tag::OctetString)?;
        let hash = match (sha1, hash.len()) {
            (true, 20) => CertificateHash::Sha1(hash),
            (false, 32) => CertificateHash::Sha256(hash),
            _ => return Err(Error::Malformed),
        };
        let issuer_serial = if cert.is_finished() {
            None
        } else {
            let mut pair = SliceReader::new(require_tag(&mut cert, Tag::Sequence)?)?;
            let mut names = SliceReader::new(require_tag(&mut pair, Tag::Sequence)?)?;
            let issuer = directory_name(&mut names)?;
            names.finish()?;
            let serial = pair.decode::<UintRef<'_>>()?.as_bytes();
            pair.finish()?;
            Some(IssuerSerial { issuer, serial })
        };
        cert.finish()?;
        identifier = Some((hash, issuer_serial));
    }
    identifier.ok_or(Error::Malformed)
}

fn parse_tst_info<'a>(
    encoded: &'a [u8],
    signature: &[u8],
) -> Result<(Duration, Duration, Option<&'a [u8]>), Error> {
    let mut fields = sequence(encoded)?;
    if fields.decode::<u8>()? != 1 {
        return Err(Error::UnsupportedProfile);
    }
    fields.decode::<ObjectIdentifier>()?; // TSA policy
    let mut imprint = SliceReader::new(require_tag(&mut fields, Tag::Sequence)?)?;
    if algorithm(&mut imprint)? != SHA256 {
        return Err(Error::UnsupportedProfile);
    }
    if require_tag(&mut imprint, Tag::OctetString)? != Sha256::digest(signature).as_slice() {
        return Err(Error::Malformed);
    }
    imprint.finish()?;
    if fields.decode::<UintRef<'_>>()?.as_bytes().len() > 20 {
        return Err(Error::TooLarge);
    }
    let time = generalized_time(require_tag(&mut fields, Tag::GeneralizedTime)?)?;
    let accuracy = if peek(&fields, Tag::Sequence)? {
        accuracy(require_tag(&mut fields, Tag::Sequence)?)?
    } else {
        Duration::ZERO
    };
    // DER omits the default ordering value.
    if peek(&fields, Tag::Boolean)? && !fields.decode::<bool>()? {
        return Err(Error::Malformed);
    }
    if peek(&fields, Tag::Integer)? {
        fields.decode::<UintRef<'_>>()?; // Request nonce, when supplied by the client
    }
    let tsa_name = if peek(&fields, CONTEXT_ZERO)? {
        let mut name = SliceReader::new(require_tag(&mut fields, CONTEXT_ZERO)?)?;
        let directory = directory_name(&mut name)?;
        name.finish()?;
        Some(directory)
    } else {
        None
    };
    if peek(&fields, CONTEXT_ONE)? {
        extensions(require_tag(&mut fields, CONTEXT_ONE)?)?;
    }
    fields.finish()?;
    Ok((time, accuracy, tsa_name))
}

fn directory_name<'a>(fields: &mut SliceReader<'a>) -> Result<&'a [u8], Error> {
    let mut encoded = SliceReader::new(require_tag(fields, DIRECTORY_NAME)?)?;
    let name = raw_sequence(&mut encoded)?;
    encoded.finish()?;
    Ok(name)
}

fn accuracy(encoded: &[u8]) -> Result<Duration, Error> {
    let mut fields = SliceReader::new(encoded)?;
    let seconds = if peek(&fields, Tag::Integer)? {
        fields.decode::<u32>()?
    } else {
        0
    };
    let mut nanos = 0;
    for (tag, multiplier) in [(TagNumber(0), 1_000_000), (TagNumber(1), 1_000)] {
        let context = Tag::ContextSpecific {
            constructed: false,
            number: tag,
        };
        if peek(&fields, context)? {
            let value = require_tag(&mut fields, context)?;
            let value = AnyRef::new(Tag::Integer, value)?.decode_as::<u16>()?;
            if !(1..=999).contains(&value) {
                return Err(Error::Malformed);
            }
            nanos += u32::from(value) * multiplier;
        }
    }
    fields.finish()?;
    Ok(Duration::new(u64::from(seconds), nanos))
}

fn extensions(encoded: &[u8]) -> Result<(), Error> {
    let mut fields = SliceReader::new(encoded)?;
    if fields.is_finished() {
        return Err(Error::Malformed);
    }
    while !fields.is_finished() {
        let extension = raw_sequence(&mut fields)?;
        let mut extension = sequence(extension)?;
        extension.decode::<ObjectIdentifier>()?;
        if peek(&extension, Tag::Boolean)? {
            return Err(Error::UnsupportedProfile); // No supported critical extensions
        }
        require_tag(&mut extension, Tag::OctetString)?;
        extension.finish()?;
    }
    Ok(())
}

fn generalized_time(bytes: &[u8]) -> Result<Duration, Error> {
    let utc = bytes.strip_suffix(b"Z").ok_or(Error::Malformed)?;
    let (seconds, fraction) = utc.split_at_checked(14).ok_or(Error::Malformed)?;
    let date = DateTime::new(
        decimal(&seconds[..4])? as u16,
        decimal(&seconds[4..6])? as u8,
        decimal(&seconds[6..8])? as u8,
        decimal(&seconds[8..10])? as u8,
        decimal(&seconds[10..12])? as u8,
        decimal(&seconds[12..14])? as u8,
    )?;
    if fraction.is_empty() {
        return Ok(date.unix_duration());
    }
    let digits = fraction.strip_prefix(b".").ok_or(Error::Malformed)?;
    if digits.is_empty() || digits.len() > 9 || digits.last() == Some(&b'0') {
        return Err(Error::Malformed);
    }
    let nanos = decimal(digits)? * 10u32.pow((9 - digits.len()) as u32);
    Ok(Duration::new(date.unix_duration().as_secs(), nanos))
}

fn decimal(bytes: &[u8]) -> Result<u32, Error> {
    bytes.iter().try_fold(0, |value, byte| {
        if byte.is_ascii_digit() {
            Ok(value * 10 + u32::from(byte - b'0'))
        } else {
            Err(Error::Malformed)
        }
    })
}

fn peek(fields: &SliceReader<'_>, tag: Tag) -> Result<bool, Error> {
    Ok(!fields.is_finished() && Tag::peek(fields)? == tag)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generalized_time_preserves_valid_fractional_seconds_and_rejects_non_der() {
        assert_eq!(generalized_time(b"19700101000000Z"), Ok(Duration::ZERO));
        assert_eq!(
            generalized_time(b"19700101000001.00000123Z"),
            Ok(Duration::new(1, 1230))
        );
        for value in [
            &b"19700101000000.0Z"[..],
            &b"19700101000000.0000000001Z"[..],
            &b"19700101000001.10Z"[..],
            &b"19700101000000+0000"[..],
            &b"19700101000000,1Z"[..],
            &b"197001010000Z"[..],
            &b"19700230000000Z"[..],
        ] {
            assert_eq!(generalized_time(value), Err(Error::Malformed));
        }
    }
}
