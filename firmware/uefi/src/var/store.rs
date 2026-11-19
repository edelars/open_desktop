//! Bounded RAM storage shared by volatile and NV-tagged variables.

use super::codec::{self, HEADER_BYTES, MAX_STORE_BYTES, MAX_VALUE_BYTES, Name, Record};
use super::services::{
    APPEND_WRITE, BOOTSERVICE_ACCESS, NON_VOLATILE, NON_VOLATILE_RUNTIME, RUNTIME_ACCESS,
    global_attributes, read_only, valid_write, validate_attributes,
};
use crate::uefi::{guid::Guid, status::Status};
use core::ops::Range;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

pub const CAPACITY: usize = MAX_STORE_BYTES - HEADER_BYTES;

/// Pointer-free state. A zeroed instance is an empty boot-time store.
#[repr(C)]
pub struct VariableStore {
    used: usize,
    runtime: bool,
    bytes: [u8; CAPACITY],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VariableInfo {
    pub capacity: u64,
    pub remaining: u64,
    pub maximum_value: u64,
}

impl VariableStore {
    #[cfg(test)]
    pub(super) const fn new() -> Self {
        Self {
            used: 0,
            runtime: false,
            bytes: [0; CAPACITY],
        }
    }

    pub fn enter_runtime(&mut self) {
        self.runtime = true;
    }

    pub fn get(&self, guid: Guid, name: Name<'_>) -> Option<Record<'_>> {
        self.find(guid, name)
            .map(|(_, record)| record)
            .filter(|record| self.visible(record.attributes))
    }

    pub fn next(&self, previous: Option<(Guid, Name<'_>)>) -> Result<Record<'_>, Status> {
        let mut records = self
            .records()
            .map(|(_, record)| record)
            .filter(|record| self.visible(record.attributes));
        if let Some((guid, name)) = previous
            && !records.any(|record| record.guid == guid && record.name == name)
        {
            return Err(Status::INVALID_PARAMETER);
        }
        records.next().ok_or(Status::NOT_FOUND)
    }

    /// Returns whether the NV image must be refreshed after a successful change.
    pub fn set(
        &mut self,
        guid: Guid,
        name: Name<'_>,
        attributes: u32,
        value: &[u8],
    ) -> Result<bool, Status> {
        validate_attributes(attributes)?;
        if read_only(guid, name) {
            return Err(Status::WRITE_PROTECTED);
        }
        let append = attributes & APPEND_WRITE != 0;
        let attributes = attributes & !APPEND_WRITE;
        let no_access = attributes & BOOTSERVICE_ACCESS == 0;
        let existing = self.find(guid, name);
        if let Some((_, record)) = &existing {
            if !self.visible(record.attributes) {
                return Err(Status::NOT_FOUND);
            }
            if !no_access && record.attributes != attributes {
                return Err(Status::INVALID_PARAMETER);
            }
        }
        let effective = existing
            .as_ref()
            .map_or(attributes, |(_, record)| record.attributes);
        if self.runtime && effective & NON_VOLATILE_RUNTIME != NON_VOLATILE_RUNTIME {
            return Err(Status::WRITE_PROTECTED);
        }
        if no_access || (!append && value.is_empty()) {
            if append {
                return Err(Status::INVALID_PARAMETER);
            }
            let (range, _) = existing.ok_or(Status::NOT_FOUND)?;
            self.resize(range, 0)?;
            return Ok(effective & NON_VOLATILE != 0);
        }
        if global_attributes(guid, name).is_some_and(|expected| attributes != expected) {
            return Err(Status::INVALID_PARAMETER);
        }
        if append && value.is_empty() {
            return Ok(false);
        }
        if !valid_write(guid, name, value, append) {
            return Err(Status::INVALID_PARAMETER);
        }
        if let Some((range, record)) = existing {
            if append {
                let old_end = range.end;
                let value_size = record.value.len().saturating_add(value.len());
                if value_size > MAX_VALUE_BYTES {
                    return Err(Status::INVALID_PARAMETER);
                }
                let new = self.resize(range, codec::record_size(name, value_size))?;
                codec::copy_bytes(&mut self.bytes[old_end..new.end], value);
                codec::seal_record(&mut self.bytes[new], value_size);
                return Ok(attributes & NON_VOLATILE != 0);
            }
            self.replace(
                range,
                Record {
                    guid,
                    name,
                    attributes,
                    value,
                },
            )?;
        } else {
            self.replace(
                self.used..self.used,
                Record {
                    guid,
                    name,
                    attributes,
                    value,
                },
            )?;
        }
        Ok(attributes & NON_VOLATILE != 0)
    }

    pub(super) fn seed(&mut self, record: Record<'_>) -> Result<(), Status> {
        self.replace(self.used..self.used, record)
    }

    pub fn query(&self, attributes: u32) -> Result<VariableInfo, Status> {
        validate_attributes(attributes)?;
        if attributes & BOOTSERVICE_ACCESS == 0 || !self.visible(attributes) {
            return Err(Status::INVALID_PARAMETER);
        }
        Ok(VariableInfo {
            capacity: CAPACITY as u64,
            remaining: (CAPACITY - self.used) as u64,
            maximum_value: MAX_VALUE_BYTES as u64,
        })
    }

    /// Serialize the compact set of NV-tagged records into a store image.
    /// The caller owns the destination and retains the returned byte count.
    pub fn encode_nv(&self, out: &mut [u8]) -> Result<usize, Status> {
        let records = || {
            self.records()
                .filter(|(_, record)| record.attributes & NON_VOLATILE != 0)
        };
        let total = HEADER_BYTES + records().map(|(range, _)| range.len()).sum::<usize>();
        if out.len() < total {
            return Err(Status::BUFFER_TOO_SMALL);
        }
        let mut position = HEADER_BYTES;
        for (range, _) in records() {
            let end = position + range.len();
            codec::copy_bytes(&mut out[position..end], &self.bytes[range]);
            position = end;
        }
        codec::write_header(out, total);
        Ok(total)
    }

    fn visible(&self, attributes: u32) -> bool {
        !self.runtime || attributes & RUNTIME_ACCESS != 0
    }

    fn find(&self, guid: Guid, name: Name<'_>) -> Option<(Range<usize>, Record<'_>)> {
        self.records()
            .find(|(_, record)| record.guid == guid && record.name == name)
    }

    fn records(&self) -> impl Iterator<Item = (Range<usize>, Record<'_>)> {
        let mut position = 0;
        core::iter::from_fn(move || {
            if position == self.used {
                return None;
            }
            // Every mutation writes one complete record into the live prefix.
            let bytes = codec::framed_record(&self.bytes[position..self.used]).unwrap();
            let start = position;
            position += bytes.len();
            Some((start..position, codec::decode_record(bytes).unwrap()))
        })
    }

    fn replace(&mut self, range: Range<usize>, record: Record<'_>) -> Result<(), Status> {
        if record.value.len() > MAX_VALUE_BYTES {
            return Err(Status::INVALID_PARAMETER);
        }
        let range = self.resize(range, codec::record_size(record.name, record.value.len()))?;
        codec::write_record(&mut self.bytes[range], record);
        Ok(())
    }

    fn resize(&mut self, old: Range<usize>, size: usize) -> Result<Range<usize>, Status> {
        let used = self.used - old.len() + size;
        if used > CAPACITY {
            return Err(Status::OUT_OF_RESOURCES);
        }
        let end = old.start + size;
        let base = self.bytes.as_mut_ptr();
        // SAFETY: capacity was checked, and the allocated arena owns both spans.
        unsafe {
            crate::bs::misc::copy_mem(
                base.add(end).cast(),
                base.add(old.end).cast(),
                self.used - old.end,
            );
        }
        self.used = used;
        Ok(old.start..end)
    }
}
