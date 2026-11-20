//! Runtime variable access backed by the protected v2 NVRAM device.

use super::{Guid, Name, Nvram, RuntimeVariables, Status};
use crate::var::{
    codec::{HEADER_BYTES, MAX_STORE_BYTES, MAX_VALUE_BYTES, Record},
    services::{
        APPEND_WRITE, BOOT_RUNTIME, BootDefaults, GLOBAL_VARIABLE, NON_VOLATILE,
        NON_VOLATILE_RUNTIME, seed_ascii, seed_defaults,
    },
    store::{CAPACITY, Mutation, VariableInfo},
};
use od_uefi_security::{
    pkcs7::MAX_SIGNED_DATA_BYTES,
    protected::{
        AUTHENTICATED_ATTRIBUTES, Image, ORDINARY_AREA_LIMIT, PROTECTED_AREA_LIMIT,
        PROTECTED_VALUE_LIMIT, PolicyVariable,
    },
    siglist::SignatureType,
};

const AUTH_REQUEST_HEADER_BYTES: usize = 24;
const MAX_AUTH_DATA_BYTES: usize = 40 + MAX_SIGNED_DATA_BYTES + PROTECTED_VALUE_LIMIT;

const SETUP_MODE: &[u8] = b"S\0e\0t\0u\0p\0M\0o\0d\0e\0";
const SECURE_BOOT: &[u8] = b"S\0e\0c\0u\0r\0e\0B\0o\0o\0t\0";
const VENDOR_KEYS: &[u8] = b"V\0e\0n\0d\0o\0r\0K\0e\0y\0s\0";
const SIGNATURE_SUPPORT: [u8; 48] = {
    let kinds = [
        SignatureType::X509,
        SignatureType::Sha256,
        SignatureType::X509Sha256,
    ];
    let mut bytes = [0; 48];
    let mut index = 0;
    while index < kinds.len() {
        let guid = kinds[index].guid_bytes();
        let mut byte = 0;
        while byte < guid.len() {
            bytes[16 * index + byte] = guid[byte];
            byte += 1;
        }
        index += 1;
    }
    bytes
};

impl RuntimeVariables {
    /// # Safety
    /// Both allocations are retained writable runtime data and do not overlap. `nvram_base`
    /// is a validated protected NVRAM aperture published by the board in handoff v1.14+.
    /// `enforces_secure_boot` is set only for the image verifier capability in v1.15.
    /// A present callback must be callable on the BSP during boot and must not borrow
    /// these variables or call runtime services.
    pub unsafe fn initialize_protected(
        storage: *mut Self,
        scratch: *mut [u8; MAX_STORE_BYTES],
        console_path: &[u8],
        nvram_base: u64,
        enforces_secure_boot: bool,
        policy_changed: Option<unsafe extern "efiapi" fn()>,
    ) -> Result<(), Status> {
        if console_path.is_empty() || storage.is_null() || scratch.is_null() || nvram_base == 0 {
            return Err(Status::INVALID_PARAMETER);
        }
        unsafe {
            crate::bs::misc::set_mem(storage.cast(), core::mem::size_of::<Self>(), 0);
            crate::bs::misc::set_mem(scratch.cast(), MAX_STORE_BYTES, 0);
        }
        let state = unsafe { &mut *storage };
        state.nvram_base = nvram_base as usize;
        state.protected_scratch = scratch;
        state.enforces_secure_boot = enforces_secure_boot;
        state.policy_changed = policy_changed;
        if seed_defaults(&mut state.store, console_path, BootDefaults::Volatile) != 0 {
            return Err(Status::OUT_OF_RESOURCES);
        }
        for (name, data) in [
            (b"AuditMode".as_slice(), &[0][..]),
            (b"DeployedMode".as_slice(), &[0][..]),
            (b"VendorKeys".as_slice(), &[1][..]),
            (b"SignatureSupport".as_slice(), &SIGNATURE_SUPPORT[..]),
        ] {
            seed_ascii(&mut state.store, name, BOOT_RUNTIME, data)?;
        }
        state.refresh_protected()?;
        if seed_defaults(&mut state.store, console_path, BootDefaults::NonVolatile) != 0 {
            return Err(Status::OUT_OF_RESOURCES);
        }
        state.write_protected_image(None)
    }

    pub(super) fn is_protected(&self) -> bool {
        !self.protected_scratch.is_null()
    }

    #[cfg(target_os = "uefi")]
    pub(in crate::rt) fn protected_scratch_base(&self) -> usize {
        self.protected_scratch.addr()
    }

    #[cfg(target_os = "uefi")]
    pub(in crate::rt) fn set_protected_scratch_base(&mut self, base: usize) {
        self.protected_scratch = core::ptr::with_exposed_provenance_mut(base);
    }

    pub(super) fn sync_variables(&mut self) -> Result<(), Status> {
        if self.is_protected() {
            self.refresh_protected()?;
        }
        Ok(())
    }

    fn refresh_protected(&mut self) -> Result<(), Status> {
        // SAFETY: protected mode supplies a distinct retained buffer, converted with the RT data.
        let scratch = unsafe { &mut *self.protected_scratch };
        let nvram = unsafe { Nvram::at(self.nvram_base) };
        let result = (|| {
            let length = nvram.read(scratch)?;
            if self.nv_image[..self.nv_length] == scratch[..length] {
                return Ok(length);
            }
            let image = Image::new(&scratch[..length]).map_err(|_| Status::DEVICE_ERROR)?;
            if !self.enforces_secure_boot && !image.flags.secure_boot_disabled {
                return Err(Status::SECURITY_VIOLATION);
            }
            self.store.import_protected_nv(&image)?;
            copy_nv_bytes(&mut self.nv_image[..length], &scratch[..length]);
            Ok(length)
        })();
        self.nv_length = result.unwrap_or(0);
        result.map(|_| ())
    }

    fn protected_image(&self) -> Result<Image<'_>, Status> {
        Image::new(&self.nv_image[..self.nv_length]).map_err(|_| Status::DEVICE_ERROR)
    }

    fn write_protected_image(&mut self, change: Option<&Mutation<'_>>) -> Result<(), Status> {
        // SAFETY: initialization made scratch disjoint from the immutable current NV image.
        let scratch = unsafe { &mut *self.protected_scratch };
        let mut length = match change {
            Some(change) => self.store.preview_nv(scratch, change)?,
            None => self.store.encode_nv(scratch)?,
        };
        if length - HEADER_BYTES > ORDINARY_AREA_LIMIT {
            return Err(Status::OUT_OF_RESOURCES);
        }
        let current = self.protected_image()?;
        for encoded in current
            .policy_bytes()
            .map_err(|_| Status::DEVICE_ERROR)?
            .into_iter()
            .flatten()
        {
            let end = length + encoded.len();
            let destination = scratch
                .get_mut(length..end)
                .ok_or(Status::OUT_OF_RESOURCES)?;
            copy_nv_bytes(destination, encoded);
            length = end;
        }
        Image::write_header(&mut scratch[..length], current.flags)
            .map_err(|_| Status::DEVICE_ERROR)?;
        if self.nv_image[..self.nv_length] != scratch[..length] {
            // The host checks the current policy before committing. Only a successful
            // FLUSH permits publishing the candidate and applying the local mutation.
            unsafe { Nvram::at(self.nvram_base) }.write(&scratch[..length])?;
            copy_nv_bytes(&mut self.nv_image[..length], &scratch[..length]);
            self.nv_length = length;
        }
        Ok(())
    }

    pub(super) fn set_v2(
        &mut self,
        guid: Guid,
        name: Name<'_>,
        attributes: u32,
        value: &[u8],
    ) -> Result<(), Status> {
        self.refresh_protected()?;
        if PolicyVariable::from_key(guid, name).is_some() {
            let length = AUTH_REQUEST_HEADER_BYTES + name.as_bytes().len() + value.len();
            if value.len() > MAX_AUTH_DATA_BYTES {
                return Err(Status::INVALID_PARAMETER);
            }
            // SAFETY: protected initialization provided a retained, initialized buffer.
            let scratch = unsafe { &mut *self.protected_scratch };
            scratch[..4].copy_from_slice(&attributes.to_le_bytes());
            scratch[4..6].copy_from_slice(&((name.as_bytes().len() / 2) as u16).to_le_bytes());
            scratch[6..8].fill(0);
            scratch[8..12].copy_from_slice(&guid.data1.to_le_bytes());
            scratch[12..14].copy_from_slice(&guid.data2.to_le_bytes());
            scratch[14..16].copy_from_slice(&guid.data3.to_le_bytes());
            scratch[16..24].copy_from_slice(&guid.data4);
            let end = AUTH_REQUEST_HEADER_BYTES + name.as_bytes().len();
            scratch[AUTH_REQUEST_HEADER_BYTES..end].copy_from_slice(name.as_bytes());
            copy_nv_bytes(&mut scratch[end..length], value);
            unsafe { Nvram::at(self.nvram_base) }.authenticated_update(&scratch[..length])?;
            if let Some(notify) = self.policy_changed {
                // The update is committed. Boot measurement records the new policy or latches
                // its failure. The pointer is cleared before the runtime phase.
                unsafe { notify() };
            }
            return self.refresh_protected();
        }
        if protected_mode_variable(guid, name) {
            return Err(Status::WRITE_PROTECTED);
        }
        let Some(change) = self.store.prepare_set(guid, name, attributes, value)? else {
            return Ok(());
        };
        self.store.check_protected_volatile_growth(&change)?;
        if change.changes_nv() {
            self.write_protected_image(Some(&change))?;
        }
        self.store.apply(change);
        Ok(())
    }

    pub(super) fn maximum_request_size(&self, guid: Guid, name: Name<'_>) -> usize {
        if self.is_protected() && PolicyVariable::from_key(guid, name).is_some() {
            MAX_AUTH_DATA_BYTES
        } else {
            MAX_VALUE_BYTES
        }
    }

    pub(super) fn get_current<'a>(
        &'a self,
        guid: Guid,
        name: Name<'a>,
    ) -> Result<Option<Record<'a>>, Status> {
        if !self.is_protected() {
            return Ok(self.store.get(guid, name));
        }
        let policy = PolicyVariable::from_key(guid, name);
        let mode_value = guid == GLOBAL_VARIABLE
            && matches!(name.as_bytes(), SETUP_MODE | SECURE_BOOT | VENDOR_KEYS);
        if policy.is_none() && !mode_value {
            return Ok(self.store.get(guid, name));
        }
        let image = self.protected_image()?;
        if mode_value {
            let enabled = if name.as_bytes() == VENDOR_KEYS {
                !image.flags.vendor_keys_modified
            } else {
                let has_pk = image.records().any(|record| {
                    record.is_ok_and(|(record, _)| {
                        record.policy_variable() == Some(PolicyVariable::Pk)
                            && !record.value.is_empty()
                    })
                });
                if name.as_bytes() == SETUP_MODE {
                    !has_pk
                } else {
                    has_pk && self.enforces_secure_boot && !image.flags.secure_boot_disabled
                }
            };
            return Ok(Some(Record {
                guid,
                name,
                attributes: BOOT_RUNTIME,
                value: if enabled { &[1] } else { &[0] },
            }));
        }
        for decoded in image.records() {
            let (record, _) = decoded.map_err(|_| Status::DEVICE_ERROR)?;
            if record.policy_variable() == policy && !record.value.is_empty() {
                return Ok(Some(Record {
                    guid,
                    name: record.name,
                    attributes: record.attributes,
                    value: record.value,
                }));
            }
        }
        Ok(None)
    }

    pub(super) fn next_current(
        &self,
        previous: Option<(Guid, Name<'_>)>,
    ) -> Result<Record<'_>, Status> {
        if !self.is_protected() {
            return self.store.next(previous);
        }
        let previous_is_protected =
            previous.is_some_and(|(guid, name)| PolicyVariable::from_key(guid, name).is_some());
        if !previous_is_protected {
            match self.store.next(previous) {
                Err(Status::NOT_FOUND) => {}
                result => return result,
            }
        }
        let image = self.protected_image()?;
        let mut found = !previous_is_protected;
        for decoded in image.records() {
            let (record, _) = decoded.map_err(|_| Status::DEVICE_ERROR)?;
            if record.policy_variable().is_none() || record.value.is_empty() {
                continue;
            }
            if found {
                return Ok(Record {
                    guid: record.guid,
                    name: record.name,
                    attributes: record.attributes,
                    value: record.value,
                });
            }
            found = previous.is_some_and(|(guid, name)| guid == record.guid && name == record.name);
        }
        Err(if found {
            Status::NOT_FOUND
        } else {
            Status::INVALID_PARAMETER
        })
    }

    pub(super) fn query_current(&self, attributes: u32) -> Result<VariableInfo, Status> {
        if !self.is_protected() {
            return self.store.query(attributes);
        }
        let attributes = attributes & !APPEND_WRITE;
        let authenticate = attributes == AUTHENTICATED_ATTRIBUTES;
        if !authenticate {
            self.store.query(attributes)?;
            if attributes & NON_VOLATILE == 0 {
                let capacity = CAPACITY - ORDINARY_AREA_LIMIT;
                return Ok(VariableInfo {
                    capacity: capacity as u64,
                    remaining: (capacity - self.store.volatile_size()) as u64,
                    maximum_value: MAX_VALUE_BYTES as u64,
                });
            }
            if attributes & NON_VOLATILE_RUNTIME != attributes {
                return Err(Status::INVALID_PARAMETER);
            }
        }
        let image = self.protected_image()?;
        let capacity = if authenticate {
            PROTECTED_AREA_LIMIT
        } else {
            ORDINARY_AREA_LIMIT
        };
        let maximum_value = if authenticate {
            PROTECTED_VALUE_LIMIT
        } else {
            MAX_VALUE_BYTES
        };
        let mut used = 0;
        for decoded in image.records() {
            let (record, encoded) = decoded.map_err(|_| Status::DEVICE_ERROR)?;
            if record.policy_variable().is_some() == authenticate {
                used += encoded.len();
            }
        }
        Ok(VariableInfo {
            capacity: capacity as u64,
            remaining: (capacity - used) as u64,
            maximum_value: maximum_value as u64,
        })
    }
}

fn copy_nv_bytes(destination: &mut [u8], source: &[u8]) {
    assert_eq!(destination.len(), source.len());
    // SAFETY: both slices cover the whole copy. CopyMem bounds each string instruction.
    unsafe {
        crate::bs::misc::copy_mem(
            destination.as_mut_ptr().cast(),
            source.as_ptr().cast(),
            source.len(),
        );
    }
}

fn protected_mode_variable(guid: Guid, name: Name<'_>) -> bool {
    guid == GLOBAL_VARIABLE
        && matches!(
            name.as_bytes(),
            SETUP_MODE
                | VENDOR_KEYS
                | b"S\0e\0c\0u\0r\0e\0B\0o\0o\0t\0"
                | b"A\0u\0d\0i\0t\0M\0o\0d\0e\0"
                | b"D\0e\0p\0l\0o\0y\0e\0d\0M\0o\0d\0e\0"
                | b"S\0i\0g\0n\0a\0t\0u\0r\0e\0S\0u\0p\0p\0o\0r\0t\0"
        )
}
