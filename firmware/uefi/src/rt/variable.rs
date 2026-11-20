//! EFIAPI argument handling and retained storage for the variable slots.

use crate::uefi::{guid::Guid, status::Status, types::Char16};
use crate::var::{
    codec::{Decoder, MAX_NAME_UNITS, MAX_STORE_BYTES, Name, valid_unit},
    nvram::Nvram,
    services::{BootDefaults, NON_VOLATILE_RUNTIME, seed_defaults},
    store::VariableStore,
};
use core::{ffi::c_void, slice};

#[cfg(target_os = "uefi")]
use super::driver::with_variables;
#[cfg(not(target_os = "uefi"))]
use crate::bs::with_runtime_variables as with_variables;

#[path = "protected_variables.rs"]
mod protected;

const MONOTONIC_GUID: Guid = Guid {
    data1: 0xcf84_fede,
    data2: 0xa501,
    data3: 0x46c2,
    data4: [0x8a, 0x8d, 0x12, 0xba, 0x32, 0x24, 0x19, 0x2f],
};
const MONOTONIC_NAME: &[u8] = b"O\0D\0M\0o\0n\0o\0t\0o\0n\0i\0c\0H\0i\0g\0h\0";

#[repr(C)]
pub struct RuntimeVariables {
    store: VariableStore,
    nvram_base: usize,
    nv_length: usize,
    protected_scratch: *mut [u8; MAX_STORE_BYTES],
    policy_changed: Option<unsafe extern "efiapi" fn()>,
    enforces_secure_boot: bool,
    nv_image: [u8; MAX_STORE_BYTES],
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NvramRestoreReport {
    pub invalid_header: bool,
    pub missing_committed_bank: bool,
    pub discarded_records: u32,
    pub missing_defaults: u32,
}

impl RuntimeVariables {
    /// Initialize in place so the arenas never use the boot stack.
    ///
    /// # Safety
    /// `storage` points at aligned, exclusively owned `RuntimeServicesData` for Self.
    /// A present `nvram_base` identifies the validated and mapped NVRAM aperture.
    pub unsafe fn initialize(
        storage: *mut Self,
        console_path: &[u8],
        nvram_base: Option<u64>,
    ) -> Result<Option<NvramRestoreReport>, Status> {
        if console_path.is_empty() {
            return Err(Status::INVALID_PARAMETER);
        }
        // All fields are integer, bool, or arrays of bytes, so zero is valid.
        unsafe { crate::bs::misc::set_mem(storage.cast(), core::mem::size_of::<Self>(), 0) };
        let state = unsafe { &mut *storage };
        let report = if let Some(base) = nvram_base {
            state.nvram_base = base as usize;
            // Firmware-owned defaults always have room in the fresh arena.
            if seed_defaults(&mut state.store, console_path, BootDefaults::Volatile) != 0 {
                return Err(Status::OUT_OF_RESOURCES);
            }
            let nvram = unsafe { Nvram::at(state.nvram_base) };
            let length = nvram.read(&mut state.nv_image)?;
            let mut report = if length == 0 {
                NvramRestoreReport {
                    missing_committed_bank: true,
                    ..NvramRestoreReport::default()
                }
            } else {
                restore_nv(&mut state.store, &state.nv_image[..length])
            };
            report.missing_defaults =
                seed_defaults(&mut state.store, console_path, BootDefaults::NonVolatile);
            Some(report)
        } else {
            if seed_defaults(&mut state.store, console_path, BootDefaults::All) != 0 {
                return Err(Status::OUT_OF_RESOURCES);
            }
            None
        };
        state.refresh_image();
        if report.is_some_and(|report| report.missing_committed_bank) {
            // Commit a fresh store so the recovered snapshot can be reopened by strict callers.
            unsafe { Nvram::at(state.nvram_base) }.write(state.nv_image())?;
        }
        Ok(report)
    }

    pub(crate) fn enter_runtime(&mut self) {
        self.policy_changed = None;
        self.store.enter_runtime();
    }

    pub(crate) fn advance_monotonic_high(&mut self) -> Result<u32, Status> {
        if self.nvram_base == 0 {
            return Err(Status::DEVICE_ERROR);
        }
        self.sync_variables()?;
        let name = Name::new(MONOTONIC_NAME).unwrap();
        let high = match self.store.get(MONOTONIC_GUID, name) {
            None => 0,
            Some(record) => {
                if record.attributes != NON_VOLATILE_RUNTIME {
                    return Err(Status::DEVICE_ERROR);
                }
                let bytes = record.value.try_into().map_err(|_| Status::DEVICE_ERROR)?;
                u32::from_le_bytes(bytes)
            }
        };
        let next = high.checked_add(1).ok_or(Status::DEVICE_ERROR)?;
        self.set(
            MONOTONIC_GUID,
            name,
            NON_VOLATILE_RUNTIME,
            &next.to_le_bytes(),
        )
        .map_err(|_| Status::DEVICE_ERROR)?;
        Ok(next)
    }

    #[cfg(target_os = "uefi")]
    pub(super) fn nvram_base(&self) -> usize {
        self.nvram_base
    }

    #[cfg(target_os = "uefi")]
    pub(super) fn set_nvram_base(&mut self, address: usize) {
        self.nvram_base = address;
    }

    pub fn nv_image(&self) -> &[u8] {
        &self.nv_image[..self.nv_length]
    }

    fn refresh_image(&mut self) {
        // The shared arena uses the snapshot record format and reserves room for its header.
        self.nv_length = self.store.encode_nv(&mut self.nv_image).unwrap();
    }

    fn set(
        &mut self,
        guid: Guid,
        name: Name<'_>,
        attributes: u32,
        value: &[u8],
    ) -> Result<(), Status> {
        if self.is_protected() {
            return self.set_v2(guid, name, attributes, value);
        }
        let base = self.nvram_base;
        self.set_with_persistence(guid, name, attributes, value, |image| {
            if base == 0 {
                return Ok(());
            }
            // SAFETY: initialize saved an already validated aperture.
            unsafe { Nvram::at(base) }.write(image)
        })
    }

    fn set_with_persistence(
        &mut self,
        guid: Guid,
        name: Name<'_>,
        attributes: u32,
        value: &[u8],
        persist: impl FnOnce(&[u8]) -> Result<(), Status>,
    ) -> Result<(), Status> {
        let Some(mutation) = self.store.prepare_set(guid, name, attributes, value)? else {
            return Ok(());
        };
        if mutation.changes_nv() {
            let length = self.store.preview_nv(&mut self.nv_image, &mutation)?;
            if let Err(status) = persist(&self.nv_image[..length]) {
                self.refresh_image();
                return Err(status);
            }
            self.nv_length = length;
        }
        self.store.apply(mutation);
        Ok(())
    }
}

fn restore_nv(store: &mut VariableStore, bytes: &[u8]) -> NvramRestoreReport {
    let Ok(records) = Decoder::new(bytes) else {
        return NvramRestoreReport {
            invalid_header: true,
            ..NvramRestoreReport::default()
        };
    };
    let mut report = NvramRestoreReport::default();
    for record in records {
        if !record.is_ok_and(|record| {
            store
                .set(record.guid, record.name, record.attributes, record.value)
                .is_ok()
        }) {
            report.discarded_records += 1;
        }
    }
    report
}

unsafe fn read_name<'a>(
    name: *const Char16,
    limit: usize,
    bytes: &'a mut [u8; MAX_NAME_UNITS * 2],
) -> Result<&'a [u8], Status> {
    for index in 0..limit.min(MAX_NAME_UNITS + 1) {
        // SAFETY: the EFI caller provides readable CHAR16 through its terminator.
        let unit = unsafe { name.add(index).read_unaligned() };
        if unit == 0 {
            return Ok(&bytes[..index * 2]);
        }
        if index == MAX_NAME_UNITS || !valid_unit(unit) {
            break;
        }
        bytes[2 * index..2 * index + 2].copy_from_slice(&unit.to_le_bytes());
    }
    Err(Status::INVALID_PARAMETER)
}

/// # Safety
/// EFI callers provide valid name, GUID, size, and optional output storage.
pub(crate) unsafe extern "efiapi" fn get_variable(
    name: *const Char16,
    guid: *const Guid,
    attributes: *mut u32,
    size: *mut usize,
    data: *mut c_void,
) -> Status {
    unsafe {
        with_variables(|state| {
            if name.is_null() || guid.is_null() || size.is_null() {
                return Status::INVALID_PARAMETER;
            }
            let mut name_bytes = [0; MAX_NAME_UNITS * 2];
            let Ok(bytes) = read_name(name, MAX_NAME_UNITS + 1, &mut name_bytes) else {
                return Status::INVALID_PARAMETER;
            };
            let Some(name) = Name::new(bytes) else {
                return Status::INVALID_PARAMETER;
            };
            if let Err(status) = state.sync_variables() {
                return status;
            }
            let record = match state.get_current(guid.read_unaligned(), name) {
                Ok(Some(record)) => record,
                Ok(None) => return Status::NOT_FOUND,
                Err(status) => return status,
            };
            let short = size.read_unaligned() < record.value.len();
            if !short && data.is_null() {
                return Status::INVALID_PARAMETER;
            }
            size.write_unaligned(record.value.len());
            if !attributes.is_null() {
                attributes.write_unaligned(record.attributes);
            }
            if short {
                return Status::BUFFER_TOO_SMALL;
            }
            crate::bs::misc::copy_mem(data, record.value.as_ptr().cast(), record.value.len());
            Status::SUCCESS
        })
    }
}

/// # Safety
/// `size`, `guid` and the `size` bytes of `name` meet the UEFI in/out contract.
pub(crate) unsafe extern "efiapi" fn get_next_variable_name(
    size: *mut usize,
    name: *mut Char16,
    guid: *mut Guid,
) -> Status {
    unsafe {
        with_variables(|state| {
            if size.is_null() || name.is_null() || guid.is_null() {
                return Status::INVALID_PARAMETER;
            }
            let capacity = size.read_unaligned();
            let mut name_bytes = [0; MAX_NAME_UNITS * 2];
            let Ok(bytes) = read_name(name, capacity / 2, &mut name_bytes) else {
                return Status::INVALID_PARAMETER;
            };
            let previous = Name::new(bytes).map(|name| (guid.read_unaligned(), name));
            if let Err(status) = state.sync_variables() {
                return status;
            }
            let record = match state.next_current(previous) {
                Ok(record) => record,
                Err(status) => return status,
            };
            let required = record.name.as_bytes().len() + 2;
            size.write_unaligned(required);
            if capacity < required {
                return Status::BUFFER_TOO_SMALL;
            }
            for (offset, unit) in record.name.units().chain(core::iter::once(0)).enumerate() {
                name.add(offset).write_unaligned(unit);
            }
            guid.write_unaligned(record.guid);
            Status::SUCCESS
        })
    }
}

/// # Safety
/// EFI callers provide the full name, GUID and `size` bytes of data when present.
pub(crate) unsafe extern "efiapi" fn set_variable(
    name: *const Char16,
    guid: *const Guid,
    attributes: u32,
    size: usize,
    data: *const c_void,
) -> Status {
    unsafe {
        with_variables(|state| {
            if name.is_null() || guid.is_null() || (size != 0 && data.is_null()) {
                return Status::INVALID_PARAMETER;
            }
            let mut name_bytes = [0; MAX_NAME_UNITS * 2];
            let Ok(bytes) = read_name(name, MAX_NAME_UNITS + 1, &mut name_bytes) else {
                return Status::INVALID_PARAMETER;
            };
            let Some(name) = Name::new(bytes) else {
                return Status::INVALID_PARAMETER;
            };
            let guid = guid.read_unaligned();
            if size > state.maximum_request_size(guid, name) {
                return Status::INVALID_PARAMETER;
            }
            let value = if size == 0 {
                &[]
            } else {
                slice::from_raw_parts(data.cast(), size)
            };
            if guid == MONOTONIC_GUID && name.as_bytes() == MONOTONIC_NAME {
                return Status::WRITE_PROTECTED;
            }
            match state.set(guid, name, attributes, value) {
                Ok(()) => Status::SUCCESS,
                Err(status) => status,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::var::{
        codec::Decoder,
        services::{APPEND_WRITE, BOOTSERVICE_ACCESS, GLOBAL_VARIABLE, NON_VOLATILE_RUNTIME},
    };

    const VENDOR: Guid = Guid {
        data1: 1,
        data2: 0,
        data3: 0,
        data4: [0; 8],
    };

    fn runtime_state() -> Box<RuntimeVariables> {
        let mut storage = Box::<RuntimeVariables>::new_uninit();
        // SAFETY: the allocation is uniquely owned, and initialize writes all fields.
        unsafe {
            RuntimeVariables::initialize(
                storage.as_mut_ptr(),
                &crate::console::COM1_DEVICE_PATH,
                None,
            )
        }
        .unwrap();
        unsafe { storage.assume_init() }
    }

    #[test]
    fn nv_deletion_without_attributes_updates_the_retained_image() {
        let mut state = runtime_state();
        let before = state.nv_image().to_vec();
        let name = Name::new(b"N\0V\0").unwrap();
        state
            .set(VENDOR, name, NON_VOLATILE_RUNTIME, b"new")
            .unwrap();
        assert!(Decoder::new(state.nv_image()).unwrap().any(|record| {
            record.is_ok_and(|record| record.guid == VENDOR && record.value == b"new")
        }));
        state.set(VENDOR, name, 0, &[]).unwrap();
        state.enter_runtime();
        assert_eq!(state.nv_image(), before);
    }

    #[test]
    fn failed_nv_flush_preserves_table_capacity_order_and_snapshot() {
        let mut state = runtime_state();
        let name = Name::new(b"N\0V\0").unwrap();
        state
            .set(VENDOR, name, NON_VOLATILE_RUNTIME, b"old")
            .unwrap();
        let before = state.nv_image().to_vec();
        let capacity = state.store.query(NON_VOLATILE_RUNTIME).unwrap();
        for (attributes, value, expected) in [
            (
                NON_VOLATILE_RUNTIME | APPEND_WRITE,
                &b"more"[..],
                Some(&b"oldmore"[..]),
            ),
            (
                NON_VOLATILE_RUNTIME,
                &b"replacement"[..],
                Some(&b"replacement"[..]),
            ),
            (0, &b""[..], None),
        ] {
            let result = state.set_with_persistence(VENDOR, name, attributes, value, |candidate| {
                let mut records = Decoder::new(candidate).unwrap().map(Result::unwrap);
                assert_eq!(
                    records
                        .find(|record| record.guid == VENDOR)
                        .map(|record| record.value),
                    expected
                );
                Err(Status::DEVICE_ERROR)
            });
            assert_eq!(result, Err(Status::DEVICE_ERROR));
            assert_eq!(state.store.get(VENDOR, name).unwrap().value, b"old");
            assert_eq!(state.store.query(NON_VOLATILE_RUNTIME), Ok(capacity));
            assert_eq!(state.nv_image(), before);
        }
        let mut committed = Vec::new();
        state
            .set_with_persistence(
                VENDOR,
                name,
                NON_VOLATILE_RUNTIME | APPEND_WRITE,
                b"more",
                |bytes| {
                    committed.extend_from_slice(bytes);
                    Ok(())
                },
            )
            .unwrap();
        let mut fresh = vec![0; MAX_STORE_BYTES];
        let size = state.store.encode_nv(&mut fresh).unwrap();
        assert_eq!(state.nv_image(), committed);
        assert_eq!(&fresh[..size], committed);
        assert_eq!(state.store.get(VENDOR, name).unwrap().value, b"oldmore");
    }

    #[test]
    fn volatile_and_empty_append_skip_the_nv_backend() {
        let mut state = runtime_state();
        let name = Name::new(b"V\0").unwrap();
        let before = state.nv_image().to_vec();
        for (attributes, value) in [
            (NON_VOLATILE_RUNTIME | APPEND_WRITE, &b""[..]),
            (BOOTSERVICE_ACCESS, &b"volatile"[..]),
        ] {
            state
                .set_with_persistence(VENDOR, name, attributes, value, |_| {
                    panic!("unexpected write to NVRAM")
                })
                .unwrap();
        }
        assert_eq!(state.nv_image(), before);
    }

    #[test]
    fn imported_writable_default_survives_initialization_policy() {
        let mut original = runtime_state();
        let timeout = Name::new(b"T\0i\0m\0e\0o\0u\0t\0").unwrap();
        original
            .set(
                GLOBAL_VARIABLE,
                timeout,
                NON_VOLATILE_RUNTIME,
                &17u16.to_le_bytes(),
            )
            .unwrap();
        let mut reopened = runtime_state();
        let report = restore_nv(&mut reopened.store, original.nv_image());
        assert_eq!(report, NvramRestoreReport::default());
        assert_eq!(
            seed_defaults(
                &mut reopened.store,
                &crate::console::COM1_DEVICE_PATH,
                BootDefaults::NonVolatile
            ),
            0
        );
        assert_eq!(
            reopened.store.get(GLOBAL_VARIABLE, timeout).unwrap().value,
            &17u16.to_le_bytes()
        );
    }
}

/// # Safety
/// EFI callers provide all three output fields as writable u64 values.
pub(crate) unsafe extern "efiapi" fn query_variable_info(
    attributes: u32,
    storage: *mut u64,
    remaining: *mut u64,
    maximum: *mut u64,
) -> Status {
    unsafe {
        with_variables(|state| {
            if storage.is_null() || remaining.is_null() || maximum.is_null() {
                return Status::INVALID_PARAMETER;
            }
            if let Err(status) = state.sync_variables() {
                return status;
            }
            let info = match state.query_current(attributes) {
                Ok(info) => info,
                Err(status) => return status,
            };
            storage.write_unaligned(info.capacity);
            remaining.write_unaligned(info.remaining);
            maximum.write_unaligned(info.maximum_value);
            Status::SUCCESS
        })
    }
}
