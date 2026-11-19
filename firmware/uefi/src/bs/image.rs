//! UEFI 2.11 §7.4 image services and the BSP's loaded-image records.

use super::{misc::MEMORY_CHUNK, protocol, protocol::multiple::ProtocolPair, state};
use crate::device_path::DevicePathNode;
use crate::mem::{map::MemoryType, pages::ImageMemoryRange};
use crate::paging::PAGE_BYTES;
use crate::pe::{self, MappedImage, PageLayout, PeImage, Subsystem};
use crate::uefi::{
    loaded_image::{
        LOADED_IMAGE_DEVICE_PATH_PROTOCOL_GUID, LOADED_IMAGE_PROTOCOL_GUID,
        LOADED_IMAGE_PROTOCOL_REVISION, LoadedImageProtocol,
    },
    status::Status,
    types::{Char16, Handle},
};
use core::{cell::UnsafeCell, ffi::c_void, mem::MaybeUninit, ptr, slice};

#[cfg(target_os = "none")]
mod invoke;

const MAX_IMAGES: usize = 64;

struct ImageRecord {
    handle: Handle,
    mapped: MappedImage,
    #[cfg(target_os = "none")]
    parent: Handle,
    #[cfg(target_os = "none")]
    phase: invoke::Phase,
    #[cfg(target_os = "none")]
    stack_base: Option<u64>,
}

// Published interfaces can be held by an EFI caller across service calls.
// Keep their backing outside the exclusive borrow of the image registry.
struct ImageProtocols([UnsafeCell<MaybeUninit<LoadedImageProtocol>>; MAX_IMAGES]);

// SAFETY: only the BSP accesses the protocol payloads. The registry guard
// owns the decision to initialize a vacant slot, before publishing a pointer.
unsafe impl Sync for ImageProtocols {}

static IMAGE_PROTOCOLS: ImageProtocols =
    ImageProtocols([const { UnsafeCell::new(MaybeUninit::uninit()) }; MAX_IMAGES]);

pub(super) struct ImageRegistry {
    records: [Option<ImageRecord>; MAX_IMAGES],
    #[cfg(target_os = "none")]
    current: Option<invoke::ActiveImage>,
}

impl ImageRegistry {
    pub(super) const fn new() -> Self {
        Self {
            records: [const { None }; MAX_IMAGES],
            #[cfg(target_os = "none")]
            current: None,
        }
    }

    fn find(&self, handle: Handle) -> Option<usize> {
        self.records
            .iter()
            .position(|slot| slot.as_ref().is_some_and(|record| record.handle == handle))
    }
}

/// # Safety
/// Call on the BSP. A non-null source must provide stable and readable file
/// bytes. The output must be aligned, writable and outside private state.
pub unsafe extern "efiapi" fn load_image(
    _boot_policy: u8,
    parent: Handle,
    device_path: *const DevicePathNode,
    source: *const c_void,
    source_size: usize,
    image: *mut Handle,
) -> Status {
    if parent.is_null() || image.is_null() {
        return Status::INVALID_PARAMETER;
    }
    if !device_path.is_null() {
        return Status::UNSUPPORTED;
    }
    if source.is_null() {
        return Status::NOT_FOUND;
    }
    let Some(source_end) = source.addr().checked_add(source_size) else {
        return Status::INVALID_PARAMETER;
    };
    let Some(out_end) = image.addr().checked_add(size_of::<Handle>()) else {
        return Status::INVALID_PARAMETER;
    };
    if source_size == 0
        || source_size > isize::MAX as usize
        || (source.addr() < out_end && image.addr() < source_end)
    {
        return Status::INVALID_PARAMETER;
    }

    unsafe {
        let status = state::with_map_mutation(|state| {
            if !state.protocols.known_handle(parent) {
                return Status::INVALID_PARAMETER;
            }
            let bytes = slice::from_raw_parts(source.cast::<u8>(), source_size);
            let pe = match PeImage::parse(bytes) {
                Ok(pe) => pe,
                Err(
                    pe::Error::UnsupportedFormat
                    | pe::Error::UnsupportedMachine
                    | pe::Error::UnsupportedSubsystem,
                ) => {
                    return Status::UNSUPPORTED;
                }
                Err(_) => return Status::LOAD_ERROR,
            };
            let Some(layout) = PageLayout::for_image(&pe) else {
                return Status::LOAD_ERROR;
            };
            if pe.relocations_stripped() && pe.base_relocations().is_some() {
                return Status::LOAD_ERROR;
            }
            let (code, data) = image_types(pe.subsystem());
            let default = ImageMemoryRange {
                pages: 0,
                memory_type: data,
            };
            let mut regions = [default; pe::MAX_REGIONS];
            let regions = &mut regions[..layout.runs().len()];
            for (region, run) in regions.iter_mut().zip(layout.runs()) {
                *region = ImageMemoryRange {
                    pages: u64::from(run.end - run.first),
                    memory_type: if run.permissions.executable() {
                        code
                    } else {
                        data
                    },
                };
            }

            let (Some(system), Some(tables)) = (state.system_table, state.active_tables) else {
                return Status::NOT_READY;
            };
            let Some(index) = state.images.records.iter().position(Option::is_none) else {
                return Status::OUT_OF_RESOURCES;
            };
            if !state.protocols.can_install_image() {
                return Status::OUT_OF_RESOURCES;
            }
            let alignment = u64::from(pe.section_alignment()).max(PAGE_BYTES);
            let Ok(plan) = state.allocator.plan_image(
                pe.image_base(),
                pe.base_relocations().is_some(),
                alignment,
                regions,
            ) else {
                return Status::OUT_OF_RESOURCES;
            };
            let span = plan.span.clone();
            let start = span.start as usize;
            let end = span.end as usize;
            if (start < source_end && source.addr() < end)
                || (start < out_end && image.addr() < end)
            {
                return Status::INVALID_PARAMETER;
            }
            if start == 0 || !tables.verify(span.clone(), true) {
                return Status::DEVICE_ERROR;
            }

            // The span is still free RAM, reserved by the BSP guard. No callback or
            // allocator mutation can make it visible before the transaction commits.
            let destination = slice::from_raw_parts_mut(
                ptr::with_exposed_provenance_mut::<u8>(start),
                end - start,
            );
            let Ok(mapped) = pe::load_into(&pe, span.start, destination) else {
                return Status::LOAD_ERROR;
            };
            for chunk in destination[mapped.size as usize..].chunks_mut(MEMORY_CHUNK) {
                chunk.fill(0);
            }
            let protocol = IMAGE_PROTOCOLS.0[index].get().cast::<LoadedImageProtocol>();
            protocol.write(LoadedImageProtocol {
                revision: LOADED_IMAGE_PROTOCOL_REVISION,
                parent_handle: parent,
                system_table: system.as_ptr(),
                device_handle: ptr::null_mut(),
                file_path: ptr::null_mut(),
                reserved: ptr::null_mut(),
                load_options_size: 0,
                load_options: ptr::null_mut(),
                image_base: ptr::with_exposed_provenance_mut(start),
                image_size: u64::from(mapped.size),
                image_code_type: code,
                image_data_type: data,
                unload: None,
            });
            let pairs = [
                ProtocolPair {
                    protocol: LOADED_IMAGE_PROTOCOL_GUID,
                    interface: protocol.cast(),
                },
                ProtocolPair {
                    protocol: LOADED_IMAGE_DEVICE_PATH_PROTOCOL_GUID,
                    interface: ptr::null_mut(),
                },
            ];
            let handle = match state.protocols.install_bindings(ptr::null_mut(), &pairs) {
                Ok(handle) => handle,
                Err(status) => return status,
            };
            state.images.records[index] = Some(ImageRecord {
                handle,
                mapped,
                #[cfg(target_os = "none")]
                parent,
                #[cfg(target_os = "none")]
                phase: invoke::Phase::Loaded,
                #[cfg(target_os = "none")]
                stack_base: None,
            });

            state.allocator.commit_image(plan);
            for run in layout.runs() {
                let start = mapped.base + u64::from(run.first) * PAGE_BYTES;
                let end = mapped.base + u64::from(run.end) * PAGE_BYTES;
                tables.set(
                    start..end,
                    run.permissions.writable(),
                    run.permissions.executable(),
                );
            }
            tables.flush();

            image.write(handle);
            state.protocols.signal_installations(state.events, |guid| {
                guid == LOADED_IMAGE_PROTOCOL_GUID || guid == LOADED_IMAGE_DEVICE_PATH_PROTOCOL_GUID
            });
            Status::SUCCESS
        });
        protocol::dispatch_after_install(status)
    }
}

fn image_types(subsystem: Subsystem) -> (MemoryType, MemoryType) {
    match subsystem {
        Subsystem::Application => (MemoryType::LOADER_CODE, MemoryType::LOADER_DATA),
        Subsystem::BootServiceDriver => (
            MemoryType::BOOT_SERVICES_CODE,
            MemoryType::BOOT_SERVICES_DATA,
        ),
        Subsystem::RuntimeDriver => (
            MemoryType::RUNTIME_SERVICES_CODE,
            MemoryType::RUNTIME_SERVICES_DATA,
        ),
    }
}

/// # Safety
/// Call on the BSP with a loaded image handle.
pub unsafe extern "efiapi" fn start_image(
    image: Handle,
    exit_data_size: *mut usize,
    exit_data: *mut *mut Char16,
) -> Status {
    #[cfg(target_os = "none")]
    unsafe {
        invoke::start(image, exit_data_size, exit_data)
    }
    #[cfg(not(target_os = "none"))]
    unsafe {
        let _ = (exit_data_size, exit_data);
        state::with(|state| {
            if state.images.find(image).is_some() {
                Status::UNSUPPORTED
            } else {
                Status::INVALID_PARAMETER
            }
        })
    }
}

/// # Safety
/// Call on the BSP. A loaded image that has not started may exit by unloading.
pub unsafe extern "efiapi" fn exit(
    image: Handle,
    exit_status: Status,
    exit_data_size: usize,
    exit_data: *const Char16,
) -> Status {
    #[cfg(target_os = "none")]
    unsafe {
        invoke::exit(image, exit_status, exit_data_size, exit_data)
    }
    #[cfg(not(target_os = "none"))]
    unsafe {
        let _ = (exit_status, exit_data_size, exit_data);
        unload_image(image)
    }
}

/// # Safety
/// Call on the BSP with a loaded image handle.
pub unsafe extern "efiapi" fn unload_image(image: Handle) -> Status {
    unsafe {
        state::with_map_mutation(|mut state| {
            let Some(index) = state.images.find(image) else {
                return Status::INVALID_PARAMETER;
            };
            #[cfg(target_os = "none")]
            match state.images.records[index].as_ref().unwrap().phase {
                invoke::Phase::Running => return Status::ACCESS_DENIED,
                invoke::Phase::Resident => return Status::UNSUPPORTED,
                invoke::Phase::Loaded | invoke::Phase::Finished => {}
            }
            release_record(&mut state, index)
        })
    }
}

unsafe fn release_record(state: &mut state::ServiceState<'_>, index: usize) -> Status {
    let Some(tables) = state.active_tables else {
        return Status::NOT_READY;
    };
    let record = state.images.records[index].as_ref().unwrap();
    let image = record.handle;
    let mapped = record.mapped;
    #[cfg(target_os = "none")]
    if invoke::has_unfinished_child(state.images, image) {
        return Status::ACCESS_DENIED;
    }
    let pages = u64::from(mapped.size).div_ceil(PAGE_BYTES);
    let span = mapped.base..mapped.base + pages * PAGE_BYTES;
    if !state.protocols.can_remove_image(image) {
        return Status::ACCESS_DENIED;
    }
    unsafe {
        #[cfg(target_os = "none")]
        if let Err(status) = invoke::release_stack(state, index) {
            return status;
        }
        if !tables.verify(span.clone(), false) {
            return Status::DEVICE_ERROR;
        }
        if state.allocator.release_image(mapped.base, pages).is_err() {
            return Status::OUT_OF_RESOURCES;
        }
        tables.set(span, true, false);
        tables.flush();
    }
    state.protocols.remove_image(image);
    state.images.records[index] = None;
    Status::SUCCESS
}
