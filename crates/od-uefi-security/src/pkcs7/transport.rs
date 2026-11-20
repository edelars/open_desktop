//! Wire format for a caller-authorized PKCS#7 request through the protected NVRAM staging area.

use crate::protected::IMAGE_CAPACITY;

use super::{MAX_DATABASE_BYTES, MAX_MESSAGE_SIGNATURE_BYTES};

pub const HANDOFF_MINOR: u16 = 16;
pub const VERIFY_COMMAND: u32 = 8;
pub const REPLY_READY: u32 = 7;
pub const HEADER_BYTES: usize = 24;
pub const REPLY_BYTES: usize = 12;
pub const REQUEST_LIMIT: usize = IMAGE_CAPACITY;

const VERSION: u8 = 1;
const DIGEST_BYTES: usize = 32;
const PART_COUNT: usize = 5;

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mode {
    Embedded = 0,
    Detached = 1,
    Digest = 2,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidRequest;

pub struct Request<'a> {
    mode: Mode,
    signed_data: &'a [u8],
    input: &'a [u8],
    allowed_db: &'a [u8],
    revoked_db: &'a [u8],
    timestamp_db: &'a [u8],
}

impl<'a> Request<'a> {
    pub fn decode(encoded: &'a [u8]) -> Result<Self, InvalidRequest> {
        let header: &[u8; HEADER_BYTES] = encoded.first_chunk().ok_or(InvalidRequest)?;
        let mode = match header[..4] {
            [VERSION, 0, 0, 0] => Mode::Embedded,
            [VERSION, 1, 0, 0] => Mode::Detached,
            [VERSION, 2, 0, 0] => Mode::Digest,
            _ => return Err(InvalidRequest),
        };
        let mut lengths = [0; PART_COUNT];
        for (length, bytes) in lengths.iter_mut().zip(header[4..].as_chunks::<4>().0) {
            *length = u32::from_le_bytes(*bytes) as usize;
        }
        let total = checked_size(mode, lengths)?;
        if total != encoded.len() {
            return Err(InvalidRequest);
        }
        let mut parts = [&[][..]; PART_COUNT];
        let mut data = &encoded[HEADER_BYTES..];
        for (part, length) in parts.iter_mut().zip(lengths) {
            (*part, data) = data.split_at(length);
        }
        let [signed_data, input, allowed_db, revoked_db, timestamp_db] = parts;
        Ok(Self {
            mode,
            signed_data,
            input,
            allowed_db,
            revoked_db,
            timestamp_db,
        })
    }

    #[cfg(feature = "verify")]
    pub fn verify(&self) -> Result<Reply, InvalidRequest> {
        use crate::verify::{
            MessageSource, MessageVerificationError, verify_message_with_timestamps,
        };
        use sha2::{Digest, Sha256};

        let source = match self.mode {
            Mode::Embedded => MessageSource::Embedded,
            Mode::Detached => MessageSource::Detached(Sha256::digest(self.input).into()),
            Mode::Digest => {
                MessageSource::Digest(self.input.try_into().map_err(|_| InvalidRequest)?)
            }
        };
        let result = verify_message_with_timestamps(
            self.signed_data,
            source,
            self.allowed_db,
            self.revoked_db,
            self.timestamp_db,
        );
        let mut reply = Reply {
            outcome: Outcome::Success,
            content_offset: 0,
            content_length: 0,
        };
        match result {
            Ok(Some(content)) => {
                let offset = (content.as_ptr() as usize)
                    .checked_sub(self.signed_data.as_ptr() as usize)
                    .ok_or(InvalidRequest)?;
                let end = offset.checked_add(content.len()).ok_or(InvalidRequest)?;
                if end > self.signed_data.len() {
                    return Err(InvalidRequest);
                }
                reply.content_offset = u32::try_from(offset).map_err(|_| InvalidRequest)?;
                reply.content_length = u32::try_from(content.len()).map_err(|_| InvalidRequest)?;
            }
            Ok(None) => {}
            Err(error) => {
                use MessageVerificationError::*;
                reply.outcome = match error {
                    UnsupportedSignature | ConflictingContent => Outcome::Unsupported,
                    MissingContent => Outcome::NotFound,
                    CompromisedData => Outcome::CompromisedData,
                    InvalidDatabase => Outcome::Aborted,
                    Unauthorized => Outcome::SecurityViolation,
                };
            }
        }
        Ok(reply)
    }
}

pub fn header(
    mode: Mode,
    lengths: [usize; PART_COUNT],
) -> Result<([u8; HEADER_BYTES], usize), InvalidRequest> {
    let size = checked_size(mode, lengths)?;
    let mut encoded = [0; HEADER_BYTES];
    encoded[0] = VERSION;
    encoded[1] = mode as u8;
    for (bytes, length) in encoded[4..].as_chunks_mut::<4>().0.iter_mut().zip(lengths) {
        *bytes = (length as u32).to_le_bytes();
    }
    Ok((encoded, size))
}

fn checked_size(mode: Mode, lengths: [usize; PART_COUNT]) -> Result<usize, InvalidRequest> {
    if !(1..=MAX_MESSAGE_SIGNATURE_BYTES).contains(&lengths[0])
        || lengths[2..].iter().any(|&bytes| bytes > MAX_DATABASE_BYTES)
        || match mode {
            Mode::Embedded => lengths[1] != 0,
            Mode::Detached => false,
            Mode::Digest => lengths[1] != DIGEST_BYTES,
        }
    {
        return Err(InvalidRequest);
    }
    lengths
        .into_iter()
        .try_fold(HEADER_BYTES, usize::checked_add)
        .filter(|&bytes| bytes <= REQUEST_LIMIT)
        .ok_or(InvalidRequest)
}

/// Values after the successful transport completion. Errors use UEFI status numbers without
/// the architecture's error bit.
#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    Success = 0,
    Unsupported = 3,
    NotFound = 14,
    Aborted = 21,
    SecurityViolation = 26,
    CompromisedData = 33,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Reply {
    pub outcome: Outcome,
    pub content_offset: u32,
    pub content_length: u32,
}

impl Reply {
    pub fn encode(self) -> [u8; REPLY_BYTES] {
        let mut encoded = [0; REPLY_BYTES];
        let words = [
            self.outcome as u32,
            self.content_offset,
            self.content_length,
        ];
        for (bytes, word) in encoded.as_chunks_mut::<4>().0.iter_mut().zip(words) {
            *bytes = word.to_le_bytes();
        }
        encoded
    }

    pub fn decode(encoded: &[u8; REPLY_BYTES]) -> Result<Self, InvalidRequest> {
        let [status, offset, length] = encoded.as_chunks::<4>().0 else {
            return Err(InvalidRequest);
        };
        let outcome = match u32::from_le_bytes(*status) {
            0 => Outcome::Success,
            3 => Outcome::Unsupported,
            14 => Outcome::NotFound,
            21 => Outcome::Aborted,
            26 => Outcome::SecurityViolation,
            33 => Outcome::CompromisedData,
            _ => return Err(InvalidRequest),
        };
        let content_offset = u32::from_le_bytes(*offset);
        let content_length = u32::from_le_bytes(*length);
        if outcome != Outcome::Success && (content_offset != 0 || content_length != 0) {
            return Err(InvalidRequest);
        }
        Ok(Self {
            outcome,
            content_offset,
            content_length,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_mode_distinguishes_empty_detached_content_from_embedded_content() {
        for mode in [Mode::Embedded, Mode::Detached] {
            let (header, total) = header(mode, [2, 0, 1, 2, 1]).unwrap();
            let request = [header.as_slice(), b"abxyz!"].concat();
            assert_eq!(total, request.len());
            let parsed = Request::decode(&request).unwrap();
            assert_eq!(parsed.mode, mode);
            assert_eq!(parsed.signed_data, b"ab");
            assert_eq!(parsed.input, b"");
            assert_eq!(parsed.allowed_db, b"x");
            assert_eq!(parsed.revoked_db, b"yz");
            assert_eq!(parsed.timestamp_db, b"!");
            assert!(Request::decode(&request[..request.len() - 1]).is_err());
            assert!(Request::decode(&[request, vec![0]].concat()).is_err());
        }
        assert!(header(Mode::Embedded, [1, 1, 0, 0, 0]).is_err());
        assert!(header(Mode::Digest, [1, 31, 0, 0, 0]).is_err());
        assert!(header(Mode::Digest, [1, 32, 0, 0, 0]).is_ok());
        assert!(header(Mode::Detached, [1, usize::MAX, 0, 0, 0]).is_err());
        assert!(header(Mode::Detached, [1, REQUEST_LIMIT, 0, 0, 0]).is_err());
        assert!(header(Mode::Embedded, [1, 0, MAX_DATABASE_BYTES + 1, 0, 0]).is_err());
    }

    #[test]
    fn reply_errors_cannot_claim_any_output_bytes() {
        let reply = Reply {
            outcome: Outcome::CompromisedData,
            content_offset: 0,
            content_length: 0,
        };
        assert_eq!(Reply::decode(&reply.encode()), Ok(reply));
        let invalid = Reply {
            content_length: 32,
            ..reply
        };
        assert_eq!(Reply::decode(&invalid.encode()), Err(InvalidRequest));
    }
}
