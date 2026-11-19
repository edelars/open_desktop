//! Registration cursors for protocol installation notifications (UEFI 2.11 §7.3).
use super::{Binding, ProtocolRegistry};
use crate::bs::{
    event::{Event, EventRegistry},
    state,
};
use crate::uefi::{guid::Guid, status::Status};
use core::ffi::c_void;
use core::ptr;

pub(super) const MAX_REGISTRATIONS: usize = 64;
pub(super) type RegistrationKey = *mut c_void;

#[derive(Clone, Copy)]
pub(super) struct Registration {
    key: RegistrationKey,
    protocol: Guid,
    event: Event,
    after_revision: usize,
}

pub(super) struct PendingBinding<'a> {
    registration: &'a mut Registration,
    pub(super) binding: Binding,
}

impl PendingBinding<'_> {
    pub(super) fn consume(self) {
        self.registration.after_revision = self.binding.revision;
    }
}

impl ProtocolRegistry {
    fn register(&mut self, protocol: Guid, event: Event) -> Result<RegistrationKey, Status> {
        let slot = self
            .registrations
            .iter_mut()
            .find(|slot| slot.is_none())
            .ok_or(Status::OUT_OF_RESOURCES)?;
        let id = self
            .last_registration
            .checked_add(1)
            .ok_or(Status::OUT_OF_RESOURCES)?;
        let key = ptr::without_provenance_mut(id);
        *slot = Some(Registration {
            key,
            protocol,
            event,
            after_revision: 0,
        });
        self.last_registration = id;
        Ok(key)
    }

    pub(in crate::bs) fn unregister_event(&mut self, event: Event) {
        for slot in &mut self.registrations {
            if slot.is_some_and(|registration| registration.event == event) {
                *slot = None;
            }
        }
    }

    pub(super) fn signal_installations(
        &self,
        events: &mut EventRegistry,
        mut installed: impl FnMut(Guid) -> bool,
    ) {
        for registration in self.registrations.iter().flatten() {
            if installed(registration.protocol) {
                events.signal(registration.event);
            }
        }
    }

    pub(super) fn pending_binding(
        &mut self,
        key: RegistrationKey,
        expected_protocol: Option<Guid>,
    ) -> Result<Option<PendingBinding<'_>>, Status> {
        let registration = self
            .registrations
            .iter_mut()
            .flatten()
            .find(|registration| registration.key == key)
            .ok_or(Status::INVALID_PARAMETER)?;
        if expected_protocol.is_some_and(|protocol| protocol != registration.protocol) {
            return Ok(None);
        }
        let binding = self
            .bindings
            .iter()
            .flatten()
            .filter(|binding| {
                binding.protocol == registration.protocol
                    && binding.revision > registration.after_revision
            })
            .min_by_key(|binding| binding.revision)
            .copied();
        Ok(binding.map(|binding| PendingBinding {
            registration,
            binding,
        }))
    }
}

/// # Safety
/// Call on the BSP. The GUID is readable and aligned. The registration output
/// is writable, aligned and separate from the GUID and firmware-private state.
pub unsafe extern "efiapi" fn register_protocol_notify(
    protocol: *const Guid,
    event: Event,
    registration: *mut *mut c_void,
) -> Status {
    if protocol.is_null() || event.is_null() || registration.is_null() {
        return Status::INVALID_PARAMETER;
    }
    unsafe {
        let protocol = protocol.read();
        state::with(|state| {
            if !state.events.contains(event) {
                return Status::INVALID_PARAMETER;
            }
            match state.protocols.register(protocol, event) {
                Ok(key) => {
                    registration.write(key);
                    Status::SUCCESS
                }
                Err(status) => status,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guid(value: u32) -> Guid {
        Guid {
            data1: value,
            data2: 0,
            data3: 0,
            data4: [0; 8],
        }
    }

    #[test]
    fn cursors_are_independent_and_only_advance_after_success() {
        let mut registry = ProtocolRegistry::new();
        let event = ptr::without_provenance_mut(1);
        let first = registry
            .install(ptr::null_mut(), guid(1), ptr::null_mut())
            .unwrap();
        let key = registry.register(guid(1), event).unwrap();
        let other = registry.register(guid(1), event).unwrap();
        let second = registry
            .install(ptr::null_mut(), guid(1), ptr::null_mut())
            .unwrap();
        assert!(
            registry
                .pending_binding(key, Some(guid(2)))
                .unwrap()
                .is_none()
        );
        for _ in 0..2 {
            // A dropped pending result models a short buffer or pool allocation failure.
            assert_eq!(
                registry
                    .pending_binding(key, None)
                    .unwrap()
                    .unwrap()
                    .binding
                    .handle,
                first
            );
        }
        let pending = registry.pending_binding(key, None).unwrap().unwrap();
        assert_eq!(pending.binding.handle, first);
        pending.consume();
        let pending = registry.pending_binding(key, None).unwrap().unwrap();
        assert_eq!(pending.binding.handle, second);
        pending.consume();
        assert!(registry.pending_binding(key, None).unwrap().is_none());
        let replacement = ptr::without_provenance_mut(3);
        registry
            .reinstall(first, guid(1), ptr::null_mut(), replacement)
            .unwrap();
        let pending = registry
            .pending_binding(key, Some(guid(1)))
            .unwrap()
            .unwrap();
        assert_eq!(pending.binding.handle, first);
        assert_eq!(pending.binding.interface, replacement);
        pending.consume();
        assert!(registry.pending_binding(key, None).unwrap().is_none());
        let pending = registry.pending_binding(other, None).unwrap().unwrap();
        assert_eq!(pending.binding.handle, second);
        pending.consume();
        registry.uninstall(first, guid(1), replacement).unwrap();
        assert!(registry.pending_binding(other, None).unwrap().is_none());
    }

    #[test]
    fn closing_an_event_invalidates_every_key_and_frees_capacity() {
        let mut registry = ProtocolRegistry::new();
        let event = ptr::without_provenance_mut(1);
        let other_event = ptr::without_provenance_mut(2);
        let keep = registry.register(guid(3), other_event).unwrap();
        let keys: [RegistrationKey; MAX_REGISTRATIONS - 1] =
            core::array::from_fn(|_| registry.register(guid(1), event).unwrap());
        assert_eq!(
            registry.register(guid(2), event),
            Err(Status::OUT_OF_RESOURCES)
        );
        registry.unregister_event(event);
        let replacement = registry.register(guid(2), other_event).unwrap();
        assert!(!keys.contains(&replacement));
        for key in keys {
            assert!(matches!(
                registry.pending_binding(key, None),
                Err(Status::INVALID_PARAMETER),
            ));
        }
        assert!(registry.pending_binding(keep, None).unwrap().is_none());
    }
}
