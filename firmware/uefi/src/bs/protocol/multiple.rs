//! Win64 variadic ABI and transactional protocol batches, UEFI 2.11 §7.3.17–18.

use super::{Binding, MAX_PROTOCOL_BINDINGS, ProtocolRegistry};
use crate::bs::state;
use crate::device_path::{DEVICE_PATH_PROTOCOL_GUID, DevicePathNode, complete_extent};
use crate::uefi::{guid::Guid, status::Status, types::Handle};
use core::ffi::c_void;
use core::ptr;

// Keep the original argument slots out of the fixed callee's home area.
#[cfg(all(target_arch = "x86_64", target_os = "none"))]
core::arch::global_asm!(
    r#"
    .macro protocol_pairs_shim public_name, impl_name
    .globl \public_name
    .type \public_name, @function
\public_name:
    mov qword ptr [rsp + 16], rdx
    mov qword ptr [rsp + 24], r8
    mov qword ptr [rsp + 32], r9
    lea rdx, [rsp + 16]
    sub rsp, 40
    call \impl_name
    add rsp, 40
    ret
    .size \public_name, .-\public_name
    .endm

    .text
    protocol_pairs_shim od_fw_install_multiple_protocol_interfaces, od_fw_install_multiple_args
    protocol_pairs_shim od_fw_uninstall_multiple_protocol_interfaces, od_fw_uninstall_multiple_args
"#
);

pub type InstallMultiple = unsafe extern "efiapi" fn(*mut Handle, ...) -> Status;
pub type UninstallMultiple = unsafe extern "efiapi" fn(Handle, ...) -> Status;

#[cfg(all(target_arch = "x86_64", target_os = "none"))]
unsafe extern "efiapi" {
    fn od_fw_install_multiple_protocol_interfaces(handle: *mut Handle, ...) -> Status;
    fn od_fw_uninstall_multiple_protocol_interfaces(handle: Handle, ...) -> Status;
}

pub const INSTALL_MULTIPLE: Option<InstallMultiple> = {
    #[cfg(all(target_arch = "x86_64", target_os = "none"))]
    {
        Some(od_fw_install_multiple_protocol_interfaces)
    }
    #[cfg(not(all(target_arch = "x86_64", target_os = "none")))]
    {
        None
    }
};

pub const UNINSTALL_MULTIPLE: Option<UninstallMultiple> = {
    #[cfg(all(target_arch = "x86_64", target_os = "none"))]
    {
        Some(od_fw_uninstall_multiple_protocol_interfaces)
    }
    #[cfg(not(all(target_arch = "x86_64", target_os = "none")))]
    {
        None
    }
};

impl ProtocolPair {
    const EMPTY: Self = Self {
        protocol: Guid {
            data1: 0,
            data2: 0,
            data3: 0,
            data4: [0; 8],
        },
        interface: ptr::null_mut(),
    };
}

/// The caller supplies pointer slots for the full list, including a NULL GUID
/// sentinel, on the Win64 incoming stack plus the first three variable args.
/// Each non-null GUID is readable at its original pointer.
unsafe fn decode_pairs(
    args: *const *mut c_void,
    output: &mut [ProtocolPair; MAX_PROTOCOL_BINDINGS],
) -> Result<usize, Status> {
    for (index, pair) in output.iter_mut().enumerate() {
        let guid = unsafe { args.add(index * 2).read() }.cast::<Guid>();
        if guid.is_null() {
            return Ok(index);
        }
        *pair = ProtocolPair {
            protocol: unsafe { guid.read() },
            interface: unsafe { args.add(index * 2 + 1).read() },
        };
    }
    if unsafe { args.add(output.len() * 2).read() }.is_null() {
        Ok(output.len())
    } else {
        Err(Status::OUT_OF_RESOURCES)
    }
}

#[derive(Clone, Copy)]
pub(in crate::bs) struct ProtocolPair {
    pub protocol: Guid,
    pub interface: *mut c_void,
}

impl ProtocolRegistry {
    pub(in crate::bs) fn install_bindings(
        &mut self,
        requested: Handle,
        pairs: &[ProtocolPair],
    ) -> Result<Handle, Status> {
        if !requested.is_null() && !self.known_handle(requested) {
            return Err(Status::INVALID_PARAMETER);
        }
        if pairs.is_empty() {
            return Ok(requested);
        }
        for (index, pair) in pairs.iter().enumerate() {
            if pairs[..index]
                .iter()
                .any(|earlier| earlier.protocol == pair.protocol)
                || (!requested.is_null() && self.binding_index(requested, pair.protocol).is_some())
            {
                return Err(Status::INVALID_PARAMETER);
            }
        }
        let last_revision = self.next_bindings_revision(pairs.len())?;
        let handle = if requested.is_null() {
            let id = self
                .last_handle
                .checked_add(1)
                .ok_or(Status::OUT_OF_RESOURCES)?;
            self.last_handle = id;
            ptr::without_provenance_mut(id)
        } else {
            requested
        };
        for (offset, (slot, pair)) in self
            .bindings
            .iter_mut()
            .filter(|slot| slot.is_none())
            .zip(pairs)
            .enumerate()
        {
            *slot = Some(Binding {
                handle,
                protocol: pair.protocol,
                interface: pair.interface,
                revision: self.last_revision + offset + 1,
            });
        }
        self.last_revision = last_revision;
        Ok(handle)
    }

    pub(super) fn next_bindings_revision(&self, count: usize) -> Result<usize, Status> {
        if self
            .bindings
            .iter()
            .filter(|slot| slot.is_none())
            .nth(count - 1)
            .is_none()
        {
            return Err(Status::OUT_OF_RESOURCES);
        }
        self.last_revision
            .checked_add(count)
            .ok_or(Status::OUT_OF_RESOURCES)
    }

    fn uninstall_bindings(&mut self, handle: Handle, pairs: &[ProtocolPair]) -> Status {
        if !self.known_handle(handle) || pairs.len() > MAX_PROTOCOL_BINDINGS {
            return Status::INVALID_PARAMETER;
        }
        let mut indices = [0; MAX_PROTOCOL_BINDINGS];
        for (index, pair) in pairs.iter().enumerate() {
            let Ok(binding) = self.existing_interface(handle, pair.protocol, pair.interface) else {
                return Status::INVALID_PARAMETER;
            };
            if indices[..index].contains(&binding) || !self.changes_allowed(binding) {
                return Status::INVALID_PARAMETER;
            }
            indices[index] = binding;
        }
        for &index in &indices[..pairs.len()] {
            self.forget_opens(index);
            self.bindings[index] = None;
        }
        Status::SUCCESS
    }
}

impl ProtocolRegistry {
    /// Device Path interfaces are caller-owned and must be readable until
    /// their final End Entire nodes, subject to the configured scan cap.
    unsafe fn check_device_paths(&self, pairs: &[ProtocolPair]) -> Result<(), Status> {
        for pair in pairs
            .iter()
            .filter(|pair| pair.protocol == DEVICE_PATH_PROTOCOL_GUID)
        {
            let path = pair.interface.cast::<DevicePathNode>();
            let Some(length) = (unsafe { complete_extent(path) }) else {
                return Err(Status::INVALID_PARAMETER);
            };
            let bytes = unsafe { core::slice::from_raw_parts(path.cast::<u8>(), length) };
            for binding in self
                .bindings
                .iter()
                .flatten()
                .filter(|binding| binding.protocol == DEVICE_PATH_PROTOCOL_GUID)
            {
                let installed = binding.interface.cast::<DevicePathNode>();
                let Some(installed_length) = (unsafe { complete_extent(installed) }) else {
                    return Err(Status::INVALID_PARAMETER);
                };
                if length == installed_length {
                    let installed = unsafe {
                        core::slice::from_raw_parts(installed.cast::<u8>(), installed_length)
                    };
                    if bytes == installed {
                        return Err(Status::ALREADY_STARTED);
                    }
                }
            }
        }
        Ok(())
    }
}

/// Fixed-arity entry called only by the InstallMultiple assembler adapter.
/// The caller is the BSP and supplies a readable argument list and aligned,
/// disjoint handle storage outside the firmware state.
#[unsafe(no_mangle)]
unsafe extern "efiapi" fn od_fw_install_multiple_args(
    handle: *mut Handle,
    args: *const *mut c_void,
) -> Status {
    if handle.is_null() || args.is_null() {
        return Status::INVALID_PARAMETER;
    }
    let mut pairs = [ProtocolPair::EMPTY; MAX_PROTOCOL_BINDINGS];
    unsafe {
        let count = match decode_pairs(args, &mut pairs) {
            Ok(count) => count,
            Err(status) => return status,
        };
        let pairs = &pairs[..count];
        let requested = handle.read();
        let status = state::with(|state| {
            if let Err(status) = state.protocols.check_device_paths(pairs) {
                return status;
            }
            match state.protocols.install_bindings(requested, pairs) {
                Ok(installed) => {
                    handle.write(installed);
                    state.protocols.signal_installations(state.events, |guid| {
                        pairs.iter().any(|pair| pair.protocol == guid)
                    });
                    Status::SUCCESS
                }
                Err(status) => status,
            }
        });
        super::dispatch_after_install(status)
    }
}

/// Fixed-arity entry called only by the UninstallMultiple assembler adapter.
/// The caller is the BSP and supplies a readable argument list.
#[unsafe(no_mangle)]
unsafe extern "efiapi" fn od_fw_uninstall_multiple_args(
    handle: Handle,
    args: *const *mut c_void,
) -> Status {
    if handle.is_null() || args.is_null() {
        return Status::INVALID_PARAMETER;
    }
    let mut pairs = [ProtocolPair::EMPTY; MAX_PROTOCOL_BINDINGS];
    unsafe {
        let Ok(count) = decode_pairs(args, &mut pairs) else {
            return Status::INVALID_PARAMETER;
        };
        state::with(|state| state.protocols.uninstall_bindings(handle, &pairs[..count]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bs::protocol::{OPEN_BY_DRIVER, OPEN_GET_PROTOCOL};

    fn guid(id: u32) -> Guid {
        Guid {
            data1: id,
            data2: 0x12b6,
            data3: 0x478b,
            data4: [0xaa; 8],
        }
    }

    fn marker(id: u32) -> ProtocolPair {
        ProtocolPair {
            protocol: guid(id),
            interface: ptr::null_mut(),
        }
    }

    #[test]
    fn decoding_stops_at_null_guid_and_allows_the_exact_capacity() {
        let protocol = guid(1);
        let protocol_pointer = ptr::from_ref(&protocol).cast_mut().cast::<c_void>();
        let mut decoded = [ProtocolPair::EMPTY; MAX_PROTOCOL_BINDINGS];
        let mut incoming = [ptr::null_mut(); MAX_PROTOCOL_BINDINGS * 2 + 1];
        for pair in incoming[..MAX_PROTOCOL_BINDINGS * 2].chunks_exact_mut(2) {
            pair[0] = protocol_pointer;
        }
        // SAFETY: the array covers all pointer slots, and protocol_pointer is
        // readable while the decoder copies the GUID values.
        unsafe {
            assert_eq!(
                decode_pairs(incoming.as_ptr(), &mut decoded),
                Ok(MAX_PROTOCOL_BINDINGS)
            );
            incoming[MAX_PROTOCOL_BINDINGS * 2] = protocol_pointer;
            assert_eq!(
                decode_pairs(incoming.as_ptr(), &mut decoded),
                Err(Status::OUT_OF_RESOURCES)
            );
            assert_eq!(
                decode_pairs([ptr::null_mut()].as_ptr(), &mut decoded),
                Ok(0)
            );
        }
    }

    #[test]
    fn late_batch_failure_preserves_bindings_opens_and_handle_ids() {
        let mut registry = ProtocolRegistry::new();
        let none = ptr::null_mut();
        assert_eq!(registry.install_bindings(none, &[]), Ok(none));
        assert_eq!(
            registry.install_bindings(none, &[marker(1), marker(1)]),
            Err(Status::INVALID_PARAMETER)
        );
        assert_eq!(registry.last_handle, 0);

        let target = registry
            .install_bindings(none, &[marker(1), marker(2)])
            .unwrap();
        assert_eq!(target.addr(), 1);
        assert_eq!(
            registry.install_bindings(target, &[marker(3), marker(1)]),
            Err(Status::INVALID_PARAMETER)
        );
        assert_eq!(
            registry.handle_protocol(target, guid(3)),
            Err(Status::UNSUPPORTED)
        );
        let driver = registry.install(none, guid(4), none).unwrap();
        assert_eq!(
            registry.open(target, guid(1), driver, none, OPEN_GET_PROTOCOL),
            Ok((Status::SUCCESS, none))
        );
        assert_eq!(
            registry.open(target, guid(2), driver, target, OPEN_BY_DRIVER),
            Ok((Status::SUCCESS, none))
        );

        assert_eq!(
            registry.uninstall_bindings(target, &[marker(1), marker(2)]),
            Status::INVALID_PARAMETER
        );
        assert_eq!(registry.handle_protocol(target, guid(1)), Ok(none));
        assert_eq!(registry.handle_protocol(target, guid(2)), Ok(none));
        assert_eq!(registry.opens.iter().flatten().count(), 2);
        assert_eq!(registry.close(target, guid(2), driver, target), Ok(()));
        assert_eq!(
            registry.uninstall_bindings(target, &[marker(1), marker(2)]),
            Status::SUCCESS
        );
        assert!(registry.opens.iter().all(Option::is_none));
        assert_eq!(
            registry.handle_protocol(target, guid(1)),
            Err(Status::INVALID_PARAMETER)
        );
    }

    #[test]
    fn device_path_duplicate_detection_uses_all_instances_and_not_storage_identity() {
        let mut registry = ProtocolRegistry::new();
        let a = [
            1u8, 1, 6, 0, 0, 1, 0x7f, 1, 4, 0, 1, 1, 6, 0, 0, 2, 0x7f, 0xff, 4, 0,
        ];
        let copied = a;
        let mut b = a;
        b[15] = 3;
        let installed = ProtocolPair {
            protocol: DEVICE_PATH_PROTOCOL_GUID,
            interface: a.as_ptr().cast_mut().cast(),
        };
        registry
            .install_bindings(ptr::null_mut(), &[installed])
            .unwrap();
        let duplicate = ProtocolPair {
            interface: copied.as_ptr().cast_mut().cast(),
            ..installed
        };
        let different = ProtocolPair {
            interface: b.as_ptr().cast_mut().cast(),
            ..installed
        };
        assert_ne!(installed.interface, duplicate.interface);
        // SAFETY: all path interfaces span initialized arrays through End Entire.
        unsafe {
            assert_eq!(
                registry.check_device_paths(&[duplicate]),
                Err(Status::ALREADY_STARTED)
            );
            assert_eq!(registry.check_device_paths(&[different]), Ok(()));
        }
    }
}
