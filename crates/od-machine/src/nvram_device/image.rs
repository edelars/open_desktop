//! Volatile transfer and Secure Boot authorization of one complete PE file.

use super::{
    IMAGE_BEGIN, IMAGE_CHUNK, IMAGE_VERIFY, Image, NVRAM_STORE_BYTES, State, Status, Transfer,
};
use crate::bus::DeviceError;
use od_uefi_security::{protected::PolicyVariable, verify::authorize_image};

const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;

pub(super) struct Upload {
    bytes: Vec<u8>,
    total: usize,
    chunk_start: usize,
    chunk_end: usize,
}

impl Upload {
    pub(super) fn write(&mut self, offset: usize, value: &[u8]) -> Result<(), DeviceError> {
        if offset != self.bytes.len() - self.chunk_start
            || self.bytes.len() + value.len() > self.chunk_end
        {
            return Err(DeviceError::UnknownOffset);
        }
        self.bytes.extend_from_slice(value);
        Ok(())
    }
}

impl State {
    pub(super) fn image_command(&mut self, code: u32, previous: Transfer) -> Status {
        let address = self.address as usize;
        let length = self.length as usize;
        if code == IMAGE_BEGIN {
            // Release an old upload before allocating for the next one.
            drop(previous);
            if address != 0 || !(1..=MAX_IMAGE_BYTES).contains(&length) {
                return Status::InvalidRange;
            }
            let mut bytes = Vec::new();
            if bytes.try_reserve_exact(length).is_err() {
                return Status::OutOfResources;
            }
            self.transfer = Transfer::ImageUpload(Upload {
                bytes,
                total: length,
                chunk_start: 0,
                chunk_end: 0,
            });
            return Status::ImageCollecting;
        }

        let Transfer::ImageUpload(mut upload) = previous else {
            return Status::Incomplete;
        };
        if upload.bytes.len() != upload.chunk_end {
            return Status::Incomplete;
        }
        match code {
            IMAGE_CHUNK
                if (1..=NVRAM_STORE_BYTES).contains(&length)
                    && address == upload.chunk_end
                    && length <= upload.total - address =>
            {
                upload.chunk_start = address;
                upload.chunk_end = address + length;
                self.transfer = Transfer::ImageUpload(upload);
                Status::WriteReady
            }
            IMAGE_VERIFY if address == 0 && length == 0 => {
                if upload.bytes.len() == upload.total {
                    self.verify_image(&upload.bytes)
                } else {
                    Status::Incomplete
                }
            }
            _ => Status::InvalidRange,
        }
    }

    fn verify_image(&mut self, bytes: &[u8]) -> Status {
        let Ok(image) = Image::new(self.banks.current().data()) else {
            return Status::PolicyRejected;
        };
        let (mut pk, mut db, mut dbx) = (false, &[][..], &[][..]);
        for decoded in image.records() {
            let Ok((record, _)) = decoded else {
                return Status::PolicyRejected;
            };
            match record.policy_variable() {
                Some(PolicyVariable::Pk) => pk = !record.value.is_empty(),
                Some(PolicyVariable::Db) => db = record.value,
                Some(PolicyVariable::Dbx) => dbx = record.value,
                _ => {}
            }
        }
        if image.flags.secure_boot_disabled || !pk {
            return Status::ImagePolicyDisabled;
        }
        let Ok(authority) = authorize_image(bytes, db, dbx) else {
            return Status::PolicyRejected;
        };
        let signature = authority.signature.bytes();
        let mut result = Vec::new();
        if result.try_reserve_exact(signature.len()).is_err() {
            return Status::OutOfResources;
        }
        result.extend_from_slice(signature);
        self.length = result.len() as u32;
        self.transfer = Transfer::ImageAuthority(result);
        Status::ImageAuthorized
    }
}
