//! UEFI 2.11 §7.4 image services and the BSP's loaded-image records.

use super::{
    misc::{MEMORY_CHUNK, copy_mem},
    protocol,
    protocol::multiple::ProtocolPair,
    state,
};
use crate::device_path::{DevicePathNode, END_PATH_BYTES, path_bytes};
use crate::mem::{map::MemoryType, pages::ImageMemoryRange};
use crate::paging::PAGE_BYTES;
use crate::pe::{self, MappedImage, PageLayout, PeImage, Subsystem};
use crate::rt::virt::{MAX_IMAGES, RuntimeImage};
use crate::uefi::{
    loaded_image::{
        LOADED_IMAGE_DEVICE_PATH_PROTOCOL_GUID, LOADED_IMAGE_PROTOCOL_GUID,
        LOADED_IMAGE_PROTOCOL_REVISION, LoadedImageProtocol,
    },
    status::Status,
    types::{Char16, Handle},
};
use core::{cell::UnsafeCell, ffi::c_void, mem::MaybeUninit, ptr, slice};

#[cfg(any(target_os = "none", test))]
mod filesystem;
#[cfg(target_os = "none")]
mod invoke;
mod path;

#[cfg(target_os = "none")]
pub use path::install_firmware_volume;
pub use path::{LOAD_FILE_PROTOCOL_GUID, LoadFileProtocol, firmware_file_device_path};

struct ImageRecord {
    handle: Handle,
    mapped: MappedImage,
    allocation_pages: u64,
    #[cfg(target_os = "none")]
    parent: Handle,
    #[cfg(target_os = "none")]
    phase: invoke::Phase,
    #[cfg(target_os = "none")]
    stack_base: Option<u64>,
    #[cfg(target_os = "none")]
    external_source: Option<ExternalSource>,
}

#[cfg(target_os = "none")]
struct ExternalSource {
    base: u64,
    length: usize,
    denied: bool,
}

#[cfg(target_os = "none")]
impl ExternalSource {
    /// The BSP owns the record and still maps this read-only tail of the image allocation.
    unsafe fn reauthorize(&self) -> Result<(), Status> {
        if self.denied {
            return Err(Status::SECURITY_VIOLATION);
        }
        let mmio = state::image_policy_mmio().ok_or(Status::DEVICE_ERROR)?;
        let bytes = unsafe {
            slice::from_raw_parts(
                ptr::with_exposed_provenance(self.base as usize),
                self.length,
            )
        };
        let nvram = unsafe { crate::var::nvram::Nvram::at(mmio as usize) };
        crate::tpm::secure_boot::authorize(nvram, bytes)
    }
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
/// bytes. A non-null path must back its described bytes through End Entire,
/// within the profile's scan cap. The output must be aligned, writable and
/// outside private state and the input spans.
pub unsafe extern "efiapi" fn load_image(
    _boot_policy: u8,
    parent: Handle,
    device_path: *const DevicePathNode,
    source: *const c_void,
    source_size: usize,
    image: *mut Handle,
) -> Status {
    unsafe { load(parent, device_path, source, source_size, image, false) }
}

/// Use the same LoadImage operation with direct firmware boot attribution.
#[cfg(target_os = "none")]
pub(crate) unsafe fn load_boot_option(
    device_path: *const DevicePathNode,
    image: *mut Handle,
) -> Status {
    unsafe { load(ptr::null_mut(), device_path, ptr::null(), 0, image, true) }
}

unsafe fn load(
    parent: Handle,
    device_path: *const DevicePathNode,
    source: *const c_void,
    source_size: usize,
    image: *mut Handle,
    firmware_parent: bool,
) -> Status {
    if (!firmware_parent && parent.is_null()) || image.is_null() {
        return Status::INVALID_PARAMETER;
    }
    if source.is_null() && device_path.is_null() {
        return Status::NOT_FOUND;
    }
    let Some(out_end) = image.addr().checked_add(size_of::<Handle>()) else {
        return Status::INVALID_PARAMETER;
    };
    let buffer = if source.is_null() {
        None
    } else {
        let Some(source_end) = source.addr().checked_add(source_size) else {
            return Status::INVALID_PARAMETER;
        };
        if source_size == 0
            || source_size > isize::MAX as usize
            || (source.addr() < out_end && image.addr() < source_end)
        {
            return Status::INVALID_PARAMETER;
        }
        Some(unsafe { slice::from_raw_parts(source.cast::<u8>(), source_size) })
    };
    let full_path = if device_path.is_null() {
        None
    } else {
        let Some(bytes) = (unsafe { path_bytes(device_path) }) else {
            return Status::INVALID_PARAMETER;
        };
        if overlaps(bytes, image.addr(), out_end) {
            return Status::INVALID_PARAMETER;
        }
        Some(bytes)
    };

    #[cfg(target_os = "none")]
    let file_source = if let (None, Some(full)) = (buffer, full_path) {
        if !firmware_parent {
            match unsafe { state::try_with(|state| state.protocols.known_handle(parent)) } {
                Some(true) => {}
                Some(false) => return Status::INVALID_PARAMETER,
                None => return Status::NOT_READY,
            }
        }
        match unsafe { path::path_source(full) } {
            Ok(source) => Some(source),
            Err(status) => return status,
        }
    } else {
        None
    };

    // A caller's SourceBuffer never supplies firmware-volume provenance. Take an owned
    // copy before parsing or loading it when the board provides image policy.
    #[cfg(target_os = "none")]
    let policy = state::image_policy_mmio();
    #[cfg(target_os = "none")]
    let caller_copy = if let (Some(_), Some(bytes)) = (policy, buffer) {
        if bytes.len() > crate::var::nvram::MAX_IMAGE_BYTES {
            unsafe { image.write(ptr::null_mut()) };
            return Status::ACCESS_DENIED;
        }
        let copy = match unsafe { super::memory::BootPool::allocate(bytes.len()) } {
            Ok(copy) => copy,
            Err(status) => return status,
        };
        unsafe { copy_mem(copy.as_ptr().cast(), bytes.as_ptr().cast(), bytes.len()) };
        Some(copy)
    } else {
        None
    };
    #[cfg(target_os = "none")]
    let buffer = caller_copy
        .as_ref()
        .map(|copy| unsafe { slice::from_raw_parts(copy.as_ptr(), copy.len()) })
        .or(buffer);

    unsafe {
        let status = state::with_map_mutation(|state| {
            if !firmware_parent && !state.protocols.known_handle(parent) {
                return Status::INVALID_PARAMETER;
            }
            let (bytes, location) = if let Some(buffer) = buffer {
                let location =
                    full_path.map(|full| path::ImagePath::for_buffer(state.protocols, full));
                (buffer, location)
            } else {
                #[cfg(target_os = "none")]
                {
                    let Some((source, location)) = file_source.as_ref() else {
                        return Status::NOT_FOUND;
                    };
                    if !source.still_bound(state.protocols, location.device) {
                        return Status::NO_MEDIA;
                    }
                    (source.as_bytes(), Some(*location))
                }
                #[cfg(not(target_os = "none"))]
                return Status::UNSUPPORTED;
            };
            #[cfg(target_os = "none")]
            let embedded = matches!(
                file_source.as_ref(),
                Some((path::PathSource::FirmwareVolume(_), _))
            );
            #[cfg(not(target_os = "none"))]
            let embedded = false;
            #[cfg(target_os = "none")]
            let active_policy = policy.filter(|_| !embedded);
            #[cfg(target_os = "none")]
            if active_policy.is_some() && bytes.len() > crate::var::nvram::MAX_IMAGE_BYTES {
                return Status::ACCESS_DENIED;
            }
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
            let runtime = if pe.subsystem() == Subsystem::RuntimeDriver {
                let Some(storage) = state.runtime_services else {
                    return Status::NOT_READY;
                };
                Some(storage)
            } else {
                None
            };
            let fixup_count = if runtime.is_some() && pe.base_relocations().is_some() {
                match pe::runtime_fixup_count(&pe) {
                    Ok(count) => count,
                    Err(_) => return Status::LOAD_ERROR,
                }
            } else {
                0
            };
            let fixup_pages =
                (fixup_count as u64 * size_of::<pe::RuntimeFixup>() as u64).div_ceil(PAGE_BYTES);
            let default = ImageMemoryRange {
                pages: 0,
                memory_type: data,
            };
            let mut regions = [default; pe::MAX_REGIONS + 3];
            let run_count = layout.runs().len();
            for (region, run) in regions[..run_count].iter_mut().zip(layout.runs()) {
                *region = ImageMemoryRange {
                    pages: u64::from(run.end - run.first),
                    memory_type: if runtime.is_some() || run.permissions.executable() {
                        code
                    } else {
                        data
                    },
                };
            }
            let metadata_pages = location
                .as_ref()
                .map_or(0, |path| (path.storage_len() as u64).div_ceil(PAGE_BYTES));
            #[cfg(target_os = "none")]
            let source_pages =
                active_policy.map_or(0, |_| (bytes.len() as u64).div_ceil(PAGE_BYTES));
            #[cfg(not(target_os = "none"))]
            let source_pages = 0;
            let mut region_count = run_count;
            for pages in [fixup_pages, metadata_pages] {
                if pages != 0 {
                    regions[region_count].pages = pages;
                    region_count += 1;
                }
            }
            if source_pages != 0 {
                regions[region_count] = ImageMemoryRange {
                    pages: source_pages,
                    memory_type: MemoryType::BOOT_SERVICES_DATA,
                };
                region_count += 1;
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
                &regions[..region_count],
            ) else {
                return Status::OUT_OF_RESOURCES;
            };
            let span = plan.span.clone();
            let start = span.start as usize;
            let end = span.end as usize;
            if overlaps(bytes, start, end)
                || full_path.is_some_and(|path| overlaps(path, start, end))
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
            let raw_base = span.end - source_pages * PAGE_BYTES;
            let metadata_base = raw_base - metadata_pages * PAGE_BYTES;
            let fixup_base = metadata_base - fixup_pages * PAGE_BYTES;
            if fixup_count != 0 {
                let (bytes, tail) = destination.split_at_mut((fixup_base - span.start) as usize);
                // The zeroed, aligned tail is exclusive and fits this record array.
                let records = slice::from_raw_parts_mut(
                    tail.as_mut_ptr().cast::<pe::RuntimeFixup>(),
                    fixup_count,
                );
                if pe::capture_runtime_fixups(&pe, &bytes[..mapped.size as usize], records).is_err()
                {
                    return Status::LOAD_ERROR;
                }
            }
            let (device, file_path, image_path) = match location {
                Some(path) => {
                    let target = destination
                        .as_mut_ptr()
                        .add((metadata_base - span.start) as usize);
                    copy_mem(target.cast(), path.full.as_ptr().cast(), path.full.len());
                    let relative = path.relative.map_or(ptr::null_mut(), |relative| {
                        let target = target.add(path.full.len());
                        copy_mem(target.cast(), relative.as_ptr().cast(), relative.len());
                        copy_mem(
                            target.add(relative.len()).cast(),
                            END_PATH_BYTES.as_ptr().cast(),
                            END_PATH_BYTES.len(),
                        );
                        target.cast()
                    });
                    (path.device, relative, target.cast())
                }
                None => (ptr::null_mut(), ptr::null_mut(), ptr::null_mut()),
            };
            #[cfg(target_os = "none")]
            let external_source = if let Some(mmio) = active_policy {
                let nvram = crate::var::nvram::Nvram::at(mmio as usize);
                let denied = match crate::tpm::secure_boot::authorize(nvram, bytes) {
                    Ok(()) => false,
                    Err(Status::SECURITY_VIOLATION) => true,
                    Err(status) => return status,
                };
                copy_mem(
                    destination
                        .as_mut_ptr()
                        .add((raw_base - span.start) as usize)
                        .cast(),
                    bytes.as_ptr().cast(),
                    bytes.len(),
                );
                Some(ExternalSource {
                    base: raw_base,
                    length: bytes.len(),
                    denied,
                })
            } else {
                None
            };
            #[cfg(target_os = "none")]
            let denied = external_source.as_ref().is_some_and(|source| source.denied);
            #[cfg(not(target_os = "none"))]
            let denied = false;
            if let Err(status) = crate::tpm::measure::image(
                &pe,
                mapped.base,
                u64::from(mapped.size),
                location.map_or(&[], |path| path.full),
                embedded,
            ) {
                return status;
            }
            let protocol = IMAGE_PROTOCOLS.0[index].get().cast::<LoadedImageProtocol>();
            protocol.write(LoadedImageProtocol {
                revision: LOADED_IMAGE_PROTOCOL_REVISION,
                parent_handle: parent,
                system_table: system.as_ptr(),
                device_handle: device,
                file_path,
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
                    interface: image_path,
                },
            ];
            let handle = match state.protocols.install_bindings(ptr::null_mut(), &pairs) {
                Ok(handle) => handle,
                Err(status) => return status,
            };
            state.images.records[index] = Some(ImageRecord {
                handle,
                mapped,
                allocation_pages: (span.end - span.start) / PAGE_BYTES,
                #[cfg(target_os = "none")]
                parent,
                #[cfg(target_os = "none")]
                phase: invoke::Phase::Loaded,
                #[cfg(target_os = "none")]
                stack_base: None,
                #[cfg(target_os = "none")]
                external_source,
            });

            if let Some(runtime) = runtime.filter(|_| !denied) {
                (*ptr::addr_of_mut!((*runtime.as_ptr()).images)).0[index] = RuntimeImage {
                    base: mapped.base,
                    size: u64::from(mapped.size),
                    fixups: ptr::with_exposed_provenance(fixup_base as usize),
                    fixup_count,
                };
            }
            state.allocator.commit_image(plan);
            for run in layout.runs() {
                let start = mapped.base + u64::from(run.first) * PAGE_BYTES;
                let end = mapped.base + u64::from(run.end) * PAGE_BYTES;
                tables.set(
                    start..end,
                    run.permissions.writable(),
                    run.permissions.executable() && !denied,
                );
            }
            if fixup_base != raw_base {
                tables.set(fixup_base..raw_base, false, false);
            }
            if raw_base != span.end {
                tables.set(raw_base..span.end, false, false);
            }
            tables.flush();

            image.write(handle);
            state.protocols.signal_installations(state.events, |guid| {
                guid == LOADED_IMAGE_PROTOCOL_GUID || guid == LOADED_IMAGE_DEVICE_PATH_PROTOCOL_GUID
            });
            if denied {
                Status::SECURITY_VIOLATION
            } else {
                Status::SUCCESS
            }
        });
        #[cfg(target_os = "none")]
        {
            drop(file_source);
            drop(caller_copy);
        }
        if status == Status::SECURITY_VIOLATION {
            super::event::dispatch_pending();
            status
        } else {
            if status == Status::ACCESS_DENIED {
                image.write(ptr::null_mut());
            }
            protocol::dispatch_after_install(status)
        }
    }
}

fn overlaps(input: &[u8], start: usize, end: usize) -> bool {
    let address = input.as_ptr().addr();
    address < end && start < address + input.len()
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
    #[cfg(target_os = "none")]
    return unsafe { invoke::unload(image) };

    #[cfg(not(target_os = "none"))]
    unsafe {
        state::with_map_mutation(|mut state| {
            let Some(index) = state.images.find(image) else {
                return Status::INVALID_PARAMETER;
            };
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
    let pages = record.allocation_pages;
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
    if mapped.subsystem == Subsystem::RuntimeDriver
        && let Some(runtime) = state.runtime_services
    {
        unsafe { (*ptr::addr_of_mut!((*runtime.as_ptr()).images)).0[index] = RuntimeImage::EMPTY };
    }
    Status::SUCCESS
}
