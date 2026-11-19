//! EFIAPI argument handling and retained storage for the variable slots.

use crate::uefi::{guid::Guid, status::Status, types::Char16};
use crate::var::{
    codec::{MAX_NAME_UNITS, MAX_STORE_BYTES, MAX_VALUE_BYTES, Name, valid_unit},
    services::seed_defaults,
    store::VariableStore,
};
use core::{ffi::c_void, slice};

#[repr(C)]
pub struct RuntimeVariables {
    store: VariableStore,
    nv_length: usize,
    nv_image: [u8; MAX_STORE_BYTES],
}

impl RuntimeVariables {
    /// Initialize in place so the arenas never use the boot stack.
    ///
    /// # Safety
    /// `storage` points at aligned, exclusively owned `RuntimeServicesData` for Self.
    pub unsafe fn initialize(storage: *mut Self, console_path: &[u8]) -> Result<(), Status> {
        // All fields are integer, bool, or arrays of bytes, so zero is valid.
        unsafe { crate::bs::misc::set_mem(storage.cast(), core::mem::size_of::<Self>(), 0) };
        let state = unsafe { &mut *storage };
        seed_defaults(&mut state.store, console_path)?;
        state.refresh_image();
        Ok(())
    }

    pub(crate) fn enter_runtime(&mut self) {
        self.store.enter_runtime();
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
        if self.store.set(guid, name, attributes, value)? {
            self.refresh_image();
        }
        Ok(())
    }
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
        crate::bs::with_runtime_variables(|state| {
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
            let Some(record) = state.store.get(guid.read_unaligned(), name) else {
                return Status::NOT_FOUND;
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
        crate::bs::with_runtime_variables(|state| {
            if size.is_null() || name.is_null() || guid.is_null() {
                return Status::INVALID_PARAMETER;
            }
            let capacity = size.read_unaligned();
            let mut name_bytes = [0; MAX_NAME_UNITS * 2];
            let Ok(bytes) = read_name(name, capacity / 2, &mut name_bytes) else {
                return Status::INVALID_PARAMETER;
            };
            let previous = Name::new(bytes).map(|name| (guid.read_unaligned(), name));
            let record = match state.store.next(previous) {
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
        crate::bs::with_runtime_variables(|state| {
            if name.is_null() || guid.is_null() || (size != 0 && data.is_null()) {
                return Status::INVALID_PARAMETER;
            }
            if size > MAX_VALUE_BYTES {
                return Status::INVALID_PARAMETER;
            }
            let mut name_bytes = [0; MAX_NAME_UNITS * 2];
            let Ok(bytes) = read_name(name, MAX_NAME_UNITS + 1, &mut name_bytes) else {
                return Status::INVALID_PARAMETER;
            };
            let Some(name) = Name::new(bytes) else {
                return Status::INVALID_PARAMETER;
            };
            let value = if size == 0 {
                &[]
            } else {
                slice::from_raw_parts(data.cast(), size)
            };
            match state.set(guid.read_unaligned(), name, attributes, value) {
                Ok(()) => Status::SUCCESS,
                Err(status) => status,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::var::{codec::Decoder, services::NON_VOLATILE_RUNTIME};

    #[test]
    fn nv_deletion_without_attributes_updates_the_retained_image() {
        let mut storage = Box::<RuntimeVariables>::new_uninit();
        // SAFETY: the allocation is uniquely owned, and initialize writes all fields.
        unsafe {
            RuntimeVariables::initialize(storage.as_mut_ptr(), &crate::console::COM1_DEVICE_PATH)
        }
        .unwrap();
        let mut state = unsafe { storage.assume_init() };
        let before = state.nv_image().to_vec();
        let guid = Guid {
            data1: 1,
            data2: 0,
            data3: 0,
            data4: [0; 8],
        };
        let name = Name::new(b"N\0V\0").unwrap();
        state.set(guid, name, NON_VOLATILE_RUNTIME, b"new").unwrap();
        assert!(Decoder::new(state.nv_image()).unwrap().any(|record| {
            record.is_ok_and(|record| record.guid == guid && record.value == b"new")
        }));
        state.set(guid, name, 0, &[]).unwrap();
        state.enter_runtime();
        assert_eq!(state.nv_image(), before);
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
        crate::bs::with_runtime_variables(|state| {
            if storage.is_null() || remaining.is_null() || maximum.is_null() {
                return Status::INVALID_PARAMETER;
            }
            let info = match state.store.query(attributes) {
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
