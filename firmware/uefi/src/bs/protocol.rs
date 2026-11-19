//! The BSP's bounded handle database and UEFI 2.11 §7.3 protocol interface services.

use crate::uefi::{guid::Guid, status::Status, types::Handle};
use core::ffi::c_void;
use core::ptr;

mod multiple;
mod notify;
mod query;

pub use multiple::{INSTALL_MULTIPLE, InstallMultiple, UNINSTALL_MULTIPLE, UninstallMultiple};
pub use notify::register_protocol_notify;

pub use query::{
    LOCATE_ALL_HANDLES, LOCATE_BY_PROTOCOL, LOCATE_BY_REGISTER_NOTIFY,
    OpenProtocolInformationEntry, locate_device_path, locate_handle, locate_handle_buffer,
    open_protocol_information, protocols_per_handle,
};

const MAX_PROTOCOL_BINDINGS: usize = 128;
const NATIVE_INTERFACE: u32 = 0;

#[derive(Clone, Copy)]
struct Binding {
    handle: Handle,
    protocol: Guid,
    interface: *mut c_void,
    revision: usize,
}

const MAX_OPEN_RECORDS: usize = 128;
pub const OPEN_BY_HANDLE_PROTOCOL: u32 = 0x01;
pub const OPEN_GET_PROTOCOL: u32 = 0x02;
pub const OPEN_TEST_PROTOCOL: u32 = 0x04;
const OPEN_BY_CHILD_CONTROLLER: u32 = 0x08;
pub const OPEN_BY_DRIVER: u32 = 0x10;
const OPEN_EXCLUSIVE: u32 = 0x20;
const OPEN_BY_DRIVER_EXCLUSIVE: u32 = OPEN_BY_DRIVER | OPEN_EXCLUSIVE;

#[derive(Clone, Copy)]
struct OpenRecord {
    binding: usize,
    agent: Handle,
    controller: Handle,
    attributes: u32,
    count: u32,
}

/// A handle remains valid exactly while this database contains a binding for it.
pub(crate) struct ProtocolRegistry {
    bindings: [Option<Binding>; MAX_PROTOCOL_BINDINGS],
    opens: [Option<OpenRecord>; MAX_OPEN_RECORDS],
    last_handle: usize,
    registrations: [Option<notify::Registration>; notify::MAX_REGISTRATIONS],
    last_registration: usize,
    last_revision: usize,
}

impl ProtocolRegistry {
    pub(crate) const fn new() -> Self {
        Self {
            bindings: [None; MAX_PROTOCOL_BINDINGS],
            opens: [None; MAX_OPEN_RECORDS],
            last_handle: 0,
            registrations: [None; notify::MAX_REGISTRATIONS],
            last_registration: 0,
            last_revision: 0,
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
        self.install_bindings(
            requested,
            &[multiple::ProtocolPair {
                protocol,
                interface,
            }],
        )
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
        if !self.changes_allowed(index) {
            return Err(Status::ACCESS_DENIED);
        }
        let revision = self
            .last_revision
            .checked_add(1)
            .ok_or(Status::OUT_OF_RESOURCES)?;
        self.forget_opens(index);
        self.bindings[index] = Some(Binding {
            handle,
            protocol,
            interface: new_interface,
            revision,
        });
        self.last_revision = revision;
        Ok(())
    }

    fn uninstall(
        &mut self,
        handle: Handle,
        protocol: Guid,
        interface: *mut c_void,
    ) -> Result<(), Status> {
        let index = self.existing_interface(handle, protocol, interface)?;
        if !self.changes_allowed(index) {
            return Err(Status::ACCESS_DENIED);
        }
        self.forget_opens(index);
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

    fn changes_allowed(&self, binding: usize) -> bool {
        !self
            .opens
            .iter()
            .flatten()
            .any(|open| open.binding == binding && open.attributes == OPEN_BY_DRIVER)
    }

    fn forget_opens(&mut self, binding: usize) {
        for slot in &mut self.opens {
            if slot.is_some_and(|open| open.binding == binding) {
                *slot = None;
            }
        }
    }

    fn open(
        &mut self,
        handle: Handle,
        guid: Guid,
        agent: Handle,
        controller: Handle,
        attributes: u32,
    ) -> Result<(Status, *mut c_void), Status> {
        if !self.known_handle(handle) {
            return Err(Status::INVALID_PARAMETER);
        }
        match attributes {
            OPEN_BY_HANDLE_PROTOCOL | OPEN_GET_PROTOCOL | OPEN_TEST_PROTOCOL | OPEN_BY_DRIVER => {}
            OPEN_BY_CHILD_CONTROLLER | OPEN_EXCLUSIVE | OPEN_BY_DRIVER_EXCLUSIVE => {
                return Err(Status::UNSUPPORTED);
            }
            _ => return Err(Status::INVALID_PARAMETER),
        }
        if (attributes == OPEN_BY_DRIVER && (agent.is_null() || controller.is_null()))
            || (!agent.is_null() && !self.known_handle(agent))
            || (!controller.is_null() && !self.known_handle(controller))
        {
            return Err(Status::INVALID_PARAMETER);
        }

        let binding = self
            .binding_index(handle, guid)
            .ok_or(Status::UNSUPPORTED)?;
        let interface = self.bindings[binding]
            .map(|entry| entry.interface)
            .ok_or(Status::UNSUPPORTED)?;
        if agent.is_null() {
            return Ok((Status::SUCCESS, interface));
        }
        if attributes == OPEN_BY_DRIVER
            && let Some(existing) = self
                .opens
                .iter()
                .flatten()
                .find(|open| open.binding == binding && open.attributes == OPEN_BY_DRIVER)
        {
            return if existing.agent == agent {
                Ok((Status::ALREADY_STARTED, interface))
            } else {
                Err(Status::ACCESS_DENIED)
            };
        }

        if let Some(existing) = self.opens.iter_mut().flatten().find(|open| {
            open.binding == binding
                && open.agent == agent
                && open.controller == controller
                && open.attributes == attributes
        }) {
            existing.count = existing
                .count
                .checked_add(1)
                .ok_or(Status::OUT_OF_RESOURCES)?;
        } else {
            let slot = self
                .opens
                .iter_mut()
                .find(|open| open.is_none())
                .ok_or(Status::OUT_OF_RESOURCES)?;
            *slot = Some(OpenRecord {
                binding,
                agent,
                controller,
                attributes,
                count: 1,
            });
        }
        Ok((Status::SUCCESS, interface))
    }

    fn close(
        &mut self,
        handle: Handle,
        guid: Guid,
        agent: Handle,
        controller: Handle,
    ) -> Result<(), Status> {
        if handle.is_null() || agent.is_null() {
            return Err(Status::INVALID_PARAMETER);
        }
        let binding = self.binding_index(handle, guid).ok_or(Status::NOT_FOUND)?;
        let mut found = false;
        for slot in &mut self.opens {
            if slot.is_some_and(|open| {
                open.binding == binding && open.agent == agent && open.controller == controller
            }) {
                *slot = None;
                found = true;
            }
        }
        if found {
            Ok(())
        } else {
            Err(Status::NOT_FOUND)
        }
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
        let status = super::state::with(|state| {
            match state.protocols.install(requested, protocol, interface) {
                Ok(installed) => {
                    handle.write(installed);
                    state
                        .protocols
                        .signal_installations(state.events, |guid| guid == protocol);
                    Status::SUCCESS
                }
                Err(status) => status,
            }
        });
        dispatch_after_install(status)
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
        let status = super::state::with(|state| {
            if let Err(status) =
                state
                    .protocols
                    .reinstall(handle, protocol, old_interface, new_interface)
            {
                return status;
            }
            state
                .protocols
                .signal_installations(state.events, |guid| guid == protocol);
            Status::SUCCESS
        });
        dispatch_after_install(status)
    }
}

/// Call on the BSP after committing the entire installation and releasing the guard.
unsafe fn dispatch_after_install(status: Status) -> Status {
    if status == Status::SUCCESS {
        unsafe { super::event::dispatch_pending() };
    }
    status
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
    if protocol.is_null() || interface.is_null() {
        return Status::INVALID_PARAMETER;
    }
    unsafe {
        let protocol = protocol.read();
        super::state::with(|state| {
            if !registration.is_null() {
                return match state
                    .protocols
                    .pending_binding(registration, Some(protocol))
                {
                    Ok(Some(pending)) => {
                        interface.write(pending.binding.interface);
                        pending.consume();
                        Status::SUCCESS
                    }
                    Ok(None) => {
                        interface.write(ptr::null_mut());
                        Status::NOT_FOUND
                    }
                    Err(status) => status,
                };
            }
            match state.protocols.locate_protocol(protocol) {
                Some(found) => {
                    interface.write(found);
                    Status::SUCCESS
                }
                None => {
                    interface.write(ptr::null_mut());
                    Status::NOT_FOUND
                }
            }
        })
    }
}

/// # Safety
/// Call on the BSP. A non-null GUID pointer must be readable and aligned.
/// For non-TEST requests the interface output is writable, aligned and
/// outside firmware-private state.
pub unsafe extern "efiapi" fn open_protocol(
    handle: Handle,
    protocol: *const Guid,
    interface: *mut *mut c_void,
    agent: Handle,
    controller: Handle,
    attributes: u32,
) -> Status {
    let test_only = attributes == OPEN_TEST_PROTOCOL;
    if handle.is_null() || protocol.is_null() || (!test_only && interface.is_null()) {
        return Status::INVALID_PARAMETER;
    }
    unsafe {
        let protocol = protocol.read();
        super::state::with(|state| {
            match state
                .protocols
                .open(handle, protocol, agent, controller, attributes)
            {
                Ok((status, found)) => {
                    if !test_only {
                        interface.write(found);
                    }
                    status
                }
                Err(status) => {
                    if !test_only && status == Status::UNSUPPORTED {
                        interface.write(ptr::null_mut());
                    }
                    status
                }
            }
        })
    }
}

/// # Safety
/// Call on the BSP. A non-null GUID pointer must be readable and aligned.
pub unsafe extern "efiapi" fn close_protocol(
    handle: Handle,
    protocol: *const Guid,
    agent: Handle,
    controller: Handle,
) -> Status {
    if handle.is_null() || protocol.is_null() || agent.is_null() {
        return Status::INVALID_PARAMETER;
    }
    unsafe {
        let protocol = protocol.read();
        super::state::with(|state| {
            state
                .protocols
                .close(handle, protocol, agent, controller)
                .err()
                .unwrap_or(Status::SUCCESS)
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

    #[test]
    fn driver_opens_pin_bindings_until_close_and_read_only_opens_do_not() {
        let mut registry = ProtocolRegistry::new();
        let mut contents = 0xa5u8;
        let interface = ptr::addr_of_mut!(contents).cast();
        let none = ptr::null_mut();
        let driver = registry.install(none, guid(1), none).unwrap();
        let other = registry.install(none, guid(1), none).unwrap();
        let target = registry.install(none, guid(2), interface).unwrap();

        assert_eq!(
            registry.open(target, guid(2), none, target, OPEN_BY_DRIVER),
            Err(Status::INVALID_PARAMETER)
        );
        assert_eq!(
            registry.open(target, guid(9), driver, target, OPEN_BY_DRIVER),
            Err(Status::UNSUPPORTED)
        );
        assert_eq!(
            registry.open(target, guid(2), driver, target, OPEN_BY_DRIVER),
            Ok((Status::SUCCESS, interface))
        );
        assert_eq!(
            registry.open(target, guid(2), driver, target, OPEN_BY_DRIVER),
            Ok((Status::ALREADY_STARTED, interface))
        );
        assert_eq!(
            registry.open(target, guid(2), other, target, OPEN_BY_DRIVER),
            Err(Status::ACCESS_DENIED)
        );
        assert_eq!(
            registry.uninstall(target, guid(2), interface),
            Err(Status::ACCESS_DENIED)
        );
        assert_eq!(
            registry.reinstall(target, guid(2), interface, none),
            Err(Status::ACCESS_DENIED)
        );
        assert_eq!(registry.handle_protocol(target, guid(2)), Ok(interface));
        assert_eq!(
            registry.close(target, guid(2), driver, none),
            Err(Status::NOT_FOUND)
        );
        assert_eq!(registry.close(target, guid(2), driver, target), Ok(()));
        assert_eq!(
            registry.close(target, guid(2), driver, target),
            Err(Status::NOT_FOUND)
        );

        for attributes in [
            OPEN_BY_HANDLE_PROTOCOL,
            OPEN_GET_PROTOCOL,
            OPEN_TEST_PROTOCOL,
        ] {
            assert_eq!(
                registry.open(target, guid(2), other, none, attributes),
                Ok((Status::SUCCESS, interface))
            );
        }
        assert_eq!(
            registry.open(target, guid(2), other, none, OPEN_GET_PROTOCOL),
            Ok((Status::SUCCESS, interface))
        );
        assert_eq!(registry.opens.iter().flatten().count(), 3);
        assert_eq!(
            registry
                .opens
                .iter()
                .flatten()
                .find(|record| record.attributes == OPEN_GET_PROTOCOL)
                .unwrap()
                .count,
            2
        );
        let get_record = registry
            .opens
            .iter_mut()
            .flatten()
            .find(|record| record.attributes == OPEN_GET_PROTOCOL)
            .unwrap();
        get_record.count = u32::MAX;
        assert_eq!(
            registry.open(target, guid(2), other, none, OPEN_GET_PROTOCOL),
            Err(Status::OUT_OF_RESOURCES)
        );
        assert_eq!(
            registry
                .opens
                .iter()
                .flatten()
                .find(|record| record.attributes == OPEN_GET_PROTOCOL)
                .unwrap()
                .count,
            u32::MAX
        );
        assert_eq!(registry.close(target, guid(2), other, none), Ok(()));
        assert!(registry.opens.iter().all(Option::is_none));
        assert_eq!(
            registry.open(target, guid(2), other, none, OPEN_GET_PROTOCOL),
            Ok((Status::SUCCESS, interface))
        );
        assert_eq!(registry.reinstall(target, guid(2), interface, none), Ok(()));
        assert!(registry.opens.iter().all(Option::is_none));
        assert_eq!(registry.handle_protocol(target, guid(2)), Ok(none));
    }
}
