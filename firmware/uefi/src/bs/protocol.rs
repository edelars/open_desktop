//! The BSP's bounded handle database and UEFI 2.11 §7.3 protocol interface services.

use crate::uefi::{guid::Guid, status::Status, types::Handle};
use core::ffi::c_void;
use core::ptr;

const MAX_PROTOCOL_BINDINGS: usize = 128;
const NATIVE_INTERFACE: u32 = 0;

#[derive(Clone, Copy)]
struct Binding {
    handle: Handle,
    protocol: Guid,
    interface: *mut c_void,
}

/// A handle remains valid exactly while this database contains a binding for it.
pub(crate) struct ProtocolRegistry {
    bindings: [Option<Binding>; MAX_PROTOCOL_BINDINGS],
    last_handle: usize,
}

impl ProtocolRegistry {
    pub(crate) const fn new() -> Self {
        Self {
            bindings: [None; MAX_PROTOCOL_BINDINGS],
            last_handle: 0,
        }
    }

    fn known_handle(&self, handle: Handle) -> bool {
        self.bindings
            .iter()
            .flatten()
            .any(|binding| binding.handle == handle)
    }

    fn binding_index(&self, handle: Handle, protocol: Guid) -> Option<usize> {
        self.bindings.iter().position(|slot| {
            slot.is_some_and(|binding| binding.handle == handle && binding.protocol == protocol)
        })
    }

    fn install(
        &mut self,
        requested: Handle,
        protocol: Guid,
        interface: *mut c_void,
    ) -> Result<Handle, Status> {
        if !requested.is_null()
            && (!self.known_handle(requested) || self.binding_index(requested, protocol).is_some())
        {
            return Err(Status::INVALID_PARAMETER);
        }
        let slot = self
            .bindings
            .iter_mut()
            .find(|binding| binding.is_none())
            .ok_or(Status::OUT_OF_RESOURCES)?;
        let handle = if requested.is_null() {
            let id = self
                .last_handle
                .checked_add(1)
                .ok_or(Status::OUT_OF_RESOURCES)?;
            self.last_handle = id;
            // EFI handles are opaque. This token is never dereferenced or reissued.
            ptr::without_provenance_mut(id)
        } else {
            requested
        };
        *slot = Some(Binding {
            handle,
            protocol,
            interface,
        });
        Ok(handle)
    }

    fn existing_interface(
        &self,
        handle: Handle,
        protocol: Guid,
        interface: *mut c_void,
    ) -> Result<usize, Status> {
        if handle.is_null() || !self.known_handle(handle) {
            return Err(Status::INVALID_PARAMETER);
        }
        self.binding_index(handle, protocol)
            .filter(|&index| {
                self.bindings[index].is_some_and(|binding| binding.interface == interface)
            })
            .ok_or(Status::NOT_FOUND)
    }

    fn reinstall(
        &mut self,
        handle: Handle,
        protocol: Guid,
        old_interface: *mut c_void,
        new_interface: *mut c_void,
    ) -> Result<(), Status> {
        let index = self.existing_interface(handle, protocol, old_interface)?;
        self.bindings[index] = Some(Binding {
            handle,
            protocol,
            interface: new_interface,
        });
        Ok(())
    }

    fn uninstall(
        &mut self,
        handle: Handle,
        protocol: Guid,
        interface: *mut c_void,
    ) -> Result<(), Status> {
        let index = self.existing_interface(handle, protocol, interface)?;
        self.bindings[index] = None;
        Ok(())
    }

    fn handle_protocol(&self, handle: Handle, protocol: Guid) -> Result<*mut c_void, Status> {
        if handle.is_null() || !self.known_handle(handle) {
            return Err(Status::INVALID_PARAMETER);
        }
        self.bindings
            .iter()
            .flatten()
            .find(|binding| binding.handle == handle && binding.protocol == protocol)
            .map(|binding| binding.interface)
            .ok_or(Status::UNSUPPORTED)
    }

    fn locate_protocol(&self, protocol: Guid) -> Option<*mut c_void> {
        self.bindings
            .iter()
            .flatten()
            .find(|binding| binding.protocol == protocol)
            .map(|binding| binding.interface)
    }
}

/// # Safety
/// Call on the BSP. Non-null arguments must point to aligned storage. The GUID
/// and incoming handle are readable; the handle output is writable and outside
/// firmware-private state. The interface is stored as an opaque pointer.
pub unsafe extern "efiapi" fn install_protocol_interface(
    handle: *mut Handle,
    protocol: *const Guid,
    interface_type: u32,
    interface: *mut c_void,
) -> Status {
    if handle.is_null() || protocol.is_null() || interface_type != NATIVE_INTERFACE {
        return Status::INVALID_PARAMETER;
    }
    unsafe {
        let requested = handle.read();
        let protocol = protocol.read();
        super::state::with(
            |state| match state.protocols.install(requested, protocol, interface) {
                Ok(installed) => {
                    handle.write(installed);
                    Status::SUCCESS
                }
                Err(status) => status,
            },
        )
    }
}

/// # Safety
/// Call on the BSP. A non-null GUID pointer must be readable and aligned.
pub unsafe extern "efiapi" fn reinstall_protocol_interface(
    handle: Handle,
    protocol: *const Guid,
    old_interface: *mut c_void,
    new_interface: *mut c_void,
) -> Status {
    if handle.is_null() || protocol.is_null() {
        return Status::INVALID_PARAMETER;
    }
    unsafe {
        let protocol = protocol.read();
        super::state::with(|state| {
            state
                .protocols
                .reinstall(handle, protocol, old_interface, new_interface)
                .err()
                .unwrap_or(Status::SUCCESS)
        })
    }
}

/// # Safety
/// Call on the BSP. A non-null GUID pointer must be readable and aligned.
pub unsafe extern "efiapi" fn uninstall_protocol_interface(
    handle: Handle,
    protocol: *const Guid,
    interface: *mut c_void,
) -> Status {
    if handle.is_null() || protocol.is_null() {
        return Status::INVALID_PARAMETER;
    }
    unsafe {
        let protocol = protocol.read();
        super::state::with(|state| {
            state
                .protocols
                .uninstall(handle, protocol, interface)
                .err()
                .unwrap_or(Status::SUCCESS)
        })
    }
}

/// # Safety
/// Call on the BSP. Non-null arguments point to an aligned readable GUID and
/// writable pointer storage outside firmware-private state.
pub unsafe extern "efiapi" fn handle_protocol(
    handle: Handle,
    protocol: *const Guid,
    interface: *mut *mut c_void,
) -> Status {
    if handle.is_null() || protocol.is_null() || interface.is_null() {
        return Status::INVALID_PARAMETER;
    }
    unsafe {
        let protocol = protocol.read();
        super::state::with(
            |state| match state.protocols.handle_protocol(handle, protocol) {
                Ok(found) => {
                    interface.write(found);
                    Status::SUCCESS
                }
                Err(Status::UNSUPPORTED) => {
                    interface.write(ptr::null_mut());
                    Status::UNSUPPORTED
                }
                Err(status) => status,
            },
        )
    }
}

/// # Safety
/// Call on the BSP. Non-null arguments point to an aligned readable GUID and
/// writable pointer storage outside firmware-private state.
pub unsafe extern "efiapi" fn locate_protocol(
    protocol: *const Guid,
    registration: *mut c_void,
    interface: *mut *mut c_void,
) -> Status {
    if protocol.is_null() || !registration.is_null() || interface.is_null() {
        return Status::INVALID_PARAMETER;
    }
    unsafe {
        let protocol = protocol.read();
        super::state::with(|state| match state.protocols.locate_protocol(protocol) {
            Some(found) => {
                interface.write(found);
                Status::SUCCESS
            }
            None => {
                interface.write(ptr::null_mut());
                Status::NOT_FOUND
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guid(kind: u32) -> Guid {
        Guid {
            data1: kind,
            data2: 0x12b6,
            data3: 0x478b,
            data4: [0xa3, 0x63, 0x07, 0x3c, 0, 0x41, 0x25, 0x61],
        }
    }

    #[test]
    fn registry_keeps_handle_identity_and_distinguishes_markers_from_missing_bindings() {
        let mut registry = ProtocolRegistry::new();
        let mut payload = 0xa5u8;
        let interface = ptr::addr_of_mut!(payload).cast();
        let marker = ptr::null_mut();
        let handle = registry
            .install(ptr::null_mut(), guid(1), interface)
            .unwrap();
        assert_eq!(registry.handle_protocol(handle, guid(1)), Ok(interface));
        assert_eq!(registry.locate_protocol(guid(1)), Some(interface));
        assert_eq!(
            registry.handle_protocol(handle, guid(2)),
            Err(Status::UNSUPPORTED)
        );
        assert_eq!(
            registry.install(handle, guid(1), marker),
            Err(Status::INVALID_PARAMETER)
        );
        assert_eq!(
            registry.reinstall(handle, guid(1), marker, interface),
            Err(Status::NOT_FOUND)
        );
        assert_eq!(registry.locate_protocol(guid(1)), Some(interface));

        assert_eq!(
            registry.reinstall(handle, guid(1), interface, marker),
            Ok(())
        );
        assert_eq!(registry.handle_protocol(handle, guid(1)), Ok(marker));
        assert_eq!(registry.locate_protocol(guid(1)), Some(marker));
        assert_eq!(registry.install(handle, guid(2), interface), Ok(handle));
        assert_eq!(registry.uninstall(handle, guid(1), marker), Ok(()));
        assert_eq!(registry.locate_protocol(guid(1)), None);
        assert_eq!(registry.handle_protocol(handle, guid(2)), Ok(interface));
        assert_eq!(registry.uninstall(handle, guid(2), interface), Ok(()));
        assert_eq!(
            registry.handle_protocol(handle, guid(2)),
            Err(Status::INVALID_PARAMETER)
        );
        assert_eq!(
            registry.install(handle, guid(3), interface),
            Err(Status::INVALID_PARAMETER)
        );
        let later = registry.install(ptr::null_mut(), guid(3), marker).unwrap();
        assert_ne!(later, handle);
        assert_eq!(payload, 0xa5);
    }

    #[test]
    fn exhausting_bindings_does_not_publish_or_consume_a_handle() {
        let mut registry = ProtocolRegistry::new();
        let marker = ptr::null_mut();
        let handle = registry.install(ptr::null_mut(), guid(0), marker).unwrap();
        for kind in 1..MAX_PROTOCOL_BINDINGS {
            assert_eq!(
                registry.install(handle, guid(kind as u32), marker),
                Ok(handle)
            );
        }
        assert_eq!(
            registry.install(ptr::null_mut(), guid(42), marker),
            Err(Status::OUT_OF_RESOURCES)
        );
        assert_eq!(registry.last_handle, handle.addr());
        assert_eq!(registry.uninstall(handle, guid(1), marker), Ok(()));
        let later = registry.install(ptr::null_mut(), guid(42), marker).unwrap();
        assert_eq!(later.addr(), handle.addr() + 1);
    }
}
