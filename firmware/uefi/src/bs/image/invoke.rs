//! The owned EFI invocation stack and the Rust side of the assembly boundary.

use super::{release_record, state};
use crate::bs::{
    event::{TPL_APPLICATION, TPL_CALLBACK, Tpl},
    memory::IdentityPoolMemory,
    protocol::{HandleSnapshot, connect_controller},
};
use crate::mem::{map::MemoryType, pages::ImageMemoryRange};
use crate::paging::PAGE_BYTES;
use crate::pe::Subsystem;
use crate::uefi::{
    loaded_image::{LOADED_IMAGE_PROTOCOL_GUID, LoadedImageProtocol},
    status::Status,
    system_table::SystemTable,
    types::{Char16, Handle},
};
use core::{cell::UnsafeCell, mem, ptr, slice};

const STACK_PAGES: u64 = 256 * 1024 / PAGE_BYTES;
const STACK_SPAN_PAGES: u64 = STACK_PAGES + 1;

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum Phase {
    Loaded,
    Running,
    Resident,
    Unloading,
    Finished,
}

// The 0x40 displacement matches the firmware's FXSAVE64/FXRSTOR64 forms.
// The FX area always lies within one hardware page.
#[repr(C, align(1024))]
struct SavedContext {
    saved_stack: u64,
    saved_flags: u64,
    reserved: [u8; 0x30],
    fx_state: [u8; 512],
}

const _: () = assert!(mem::offset_of!(SavedContext, fx_state) == 0x40);

impl SavedContext {
    const EMPTY: Self = Self {
        saved_stack: 0,
        saved_flags: 0,
        reserved: [0; 0x30],
        fx_state: [0; 512],
    };
}

struct Frame {
    context: SavedContext,
    exit_data: UnsafeCell<ExitData>,
    exit_data_output: *mut *mut Char16,
}

#[derive(Clone, Copy)]
struct ExitData {
    size: usize,
    data: *mut Char16,
}

impl ExitData {
    const EMPTY: Self = Self {
        size: 0,
        data: ptr::null_mut(),
    };
}

#[derive(Clone, Copy)]
pub(super) struct ActiveImage {
    handle: Handle,
    frame: *mut Frame,
}

struct Invocation {
    stack_top: u64,
    index: usize,
    previous: Option<ActiveImage>,
    previous_exit_handle: Handle,
    caller_tpl: Tpl,
}

struct StartPlan {
    invocation: Invocation,
    entry: u64,
    system: *mut SystemTable,
    handles: HandleSnapshot,
}

struct UnloadPlan {
    invocation: Invocation,
    callback: unsafe extern "efiapi" fn(Handle) -> Status,
}

unsafe extern "sysv64" {
    fn od_fw_invoke_image(
        context: *mut SavedContext,
        stack_top: u64,
        entry: *const (),
        handle: Handle,
        system: *mut SystemTable,
    ) -> Status;
    fn od_fw_exit_image_context(context: *mut SavedContext, status: Status) -> !;
}

pub(super) unsafe fn start(
    image: Handle,
    exit_data_size: *mut usize,
    exit_data: *mut *mut Char16,
) -> Status {
    if !exit_data.is_null()
        && (exit_data_size.is_null() || exit_data_size.addr() == exit_data.addr())
    {
        return Status::INVALID_PARAMETER;
    }

    let mut frame = Frame {
        context: SavedContext::EMPTY,
        exit_data: UnsafeCell::new(ExitData::EMPTY),
        exit_data_output: exit_data,
    };
    let frame_pointer = ptr::addr_of_mut!(frame);
    let plan = match unsafe { prepare_start(image, frame_pointer) } {
        Ok(plan) => plan,
        Err(error) => return error,
    };

    // The state guard and every borrowed service object are gone. The frame
    // stays on the suspended caller stack and is addressed only by raw pointer.
    let status = unsafe {
        od_fw_invoke_image(
            ptr::addr_of_mut!((*frame_pointer).context),
            plan.invocation.stack_top,
            ptr::with_exposed_provenance(plan.entry as usize),
            image,
            plan.system,
        )
    };

    let completion = unsafe { finish_start(plan, status) };
    let data = unsafe { frame.exit_data.get().read() };
    if !frame.exit_data_output.is_null() {
        unsafe {
            frame.exit_data_output.write(data.data);
            exit_data_size.write(data.size);
        }
    }
    completion
}

unsafe fn prepare_start(image: Handle, frame: *mut Frame) -> Result<StartPlan, Status> {
    unsafe {
        state::try_with(|mut state| {
            let Some(index) = state.images.find(image) else {
                return Err(Status::INVALID_PARAMETER);
            };
            let record = state.images.records[index].as_ref().unwrap();
            if record.phase != Phase::Loaded
                || !state.exit.can_start_image()
                || state.events.current_tpl() != TPL_APPLICATION
            {
                return Err(Status::INVALID_PARAMETER);
            }
            let Some(system) = state.system_table else {
                return Err(Status::NOT_READY);
            };
            if let Some(source) = &record.external_source {
                source.reauthorize()?;
            }
            let entry = record.mapped.entry_address;
            let stack_top = allocate_stack(&mut state, index)?;
            state.images.records[index].as_mut().unwrap().phase = Phase::Running;
            let previous = state.images.current.replace(ActiveImage {
                handle: image,
                frame,
            });
            Ok(StartPlan {
                invocation: Invocation {
                    stack_top,
                    index,
                    previous,
                    previous_exit_handle: state.exit.replace_image(image),
                    caller_tpl: TPL_APPLICATION,
                },
                entry,
                system: system.as_ptr(),
                handles: state.protocols.handle_snapshot(),
            })
        })
    }
    .unwrap_or(Err(Status::NOT_READY))
}

unsafe fn finish_start(plan: StartPlan, status: Status) -> Status {
    let index = plan.invocation.index;
    let result = unsafe {
        state::try_with(|mut state| {
            restore_service_context(&mut state, plan.invocation);
            let record = state.images.records[index].as_mut().unwrap();
            record.phase = if record.mapped.subsystem == Subsystem::Application || status.is_error()
            {
                Phase::Finished
            } else {
                Phase::Resident
            };
            let phase = record.phase;
            if let Err(status) = release_stack(&mut state, index) {
                return status;
            }
            if phase == Phase::Finished {
                let release = release_record(&mut state, index);
                if release != Status::SUCCESS {
                    return release;
                }
            }
            status
        })
    };
    match result {
        Some(status) => {
            // Call outside the state guard, since driver binding can enter
            // boot services again.
            if let Some(handles) =
                unsafe { state::try_with(|state| state.protocols.changed_handles(&plan.handles)) }
            {
                for handle in handles.into_iter().take_while(|handle| !handle.is_null()) {
                    unsafe { connect_controller(handle, ptr::null(), ptr::null(), 1) };
                }
            }
            status
        }
        // A successful ExitBootServices terminates this firmware invocation.
        // Its image cannot return control to the boot-services caller.
        None => unsafe {
            core::arch::asm!("cli", "2: hlt", "jmp 2b", options(noreturn, nostack));
        },
    }
}

pub(super) unsafe fn unload(image: Handle) -> Status {
    let plan = match unsafe { prepare_unload(image) } {
        Ok(Some(plan)) => plan,
        Ok(None) => return Status::SUCCESS,
        Err(status) => return status,
    };
    let mut context = SavedContext::EMPTY;
    // Firmware passes the image handle in RCX. An unload callback has no
    // System Table argument and ignores the value placed in RDX.
    let status = unsafe {
        od_fw_invoke_image(
            ptr::addr_of_mut!(context),
            plan.invocation.stack_top,
            plan.callback as *const (),
            image,
            ptr::null_mut(),
        )
    };
    unsafe {
        state::with(|mut state| {
            let index = plan.invocation.index;
            restore_service_context(&mut state, plan.invocation);
            let record = state.images.records[index].as_mut().unwrap();
            if status == Status::SUCCESS {
                // If release must be retried, the finished phase avoids
                // calling a callback that has already torn down the driver.
                record.phase = Phase::Finished;
                release_record(&mut state, index)
            } else {
                record.phase = Phase::Resident;
                // A failed stack release retains the recorded stack for retry.
                let _ = release_stack(&mut state, index);
                status
            }
        })
    }
}

unsafe fn prepare_unload(image: Handle) -> Result<Option<UnloadPlan>, Status> {
    unsafe {
        state::try_with(|mut state| {
            if !state.exit.can_change_map() {
                return Err(Status::ACCESS_DENIED);
            }
            let Some(index) = state.images.find(image) else {
                return Err(Status::INVALID_PARAMETER);
            };
            let caller_tpl = state.events.current_tpl();
            if caller_tpl > TPL_CALLBACK {
                return Err(Status::INVALID_PARAMETER);
            }
            match state.images.records[index].as_ref().unwrap().phase {
                Phase::Running | Phase::Unloading => return Err(Status::ACCESS_DENIED),
                Phase::Loaded | Phase::Finished => {
                    return match release_record(&mut state, index) {
                        Status::SUCCESS => Ok(None),
                        failure => Err(failure),
                    };
                }
                Phase::Resident => {}
            }
            if has_unfinished_child(state.images, image) {
                return Err(Status::ACCESS_DENIED);
            }
            let Ok(interface) = state
                .protocols
                .handle_protocol(image, LOADED_IMAGE_PROTOCOL_GUID)
            else {
                return Err(Status::UNSUPPORTED);
            };
            let interface = interface.cast::<LoadedImageProtocol>();
            if interface.is_null() {
                return Err(Status::UNSUPPORTED);
            }
            let Some(callback) = ptr::addr_of!((*interface).unload).read() else {
                return Err(Status::UNSUPPORTED);
            };
            release_stack(&mut state, index)?;
            let stack_top = allocate_stack(&mut state, index)?;
            state.images.records[index].as_mut().unwrap().phase = Phase::Unloading;
            Ok(Some(UnloadPlan {
                invocation: Invocation {
                    stack_top,
                    index,
                    previous: state.images.current.take(),
                    previous_exit_handle: state.exit.replace_image(ptr::null_mut()),
                    caller_tpl,
                },
                callback,
            }))
        })
    }
    .unwrap_or(Err(Status::NOT_READY))
}

fn restore_service_context(state: &mut state::ServiceState<'_>, invocation: Invocation) {
    state.images.current = invocation.previous;
    state.exit.replace_image(invocation.previous_exit_handle);
    state.events.restore_tpl(invocation.caller_tpl);
}

unsafe fn allocate_stack(state: &mut state::ServiceState<'_>, index: usize) -> Result<u64, Status> {
    let Some(tables) = state.active_tables else {
        return Err(Status::NOT_READY);
    };
    // Reserve keys for stack allocation, stack release and image release.
    if state.allocator.map().key().checked_add(3).is_none() {
        return Err(Status::OUT_OF_RESOURCES);
    }
    let regions = [ImageMemoryRange {
        pages: STACK_SPAN_PAGES,
        memory_type: MemoryType::BOOT_SERVICES_DATA,
    }];
    let stack = state
        .allocator
        .plan_image(u64::MAX, true, PAGE_BYTES, &regions)
        .map_err(|_| Status::OUT_OF_RESOURCES)?;
    let span = stack.span.clone();
    unsafe {
        if !tables.verify(span.clone(), true) {
            return Err(Status::DEVICE_ERROR);
        }
        state.allocator.commit_image(stack);
        tables.set_guard(span.start, true);
        tables.flush();
    }
    state.images.records[index].as_mut().unwrap().stack_base = Some(span.start);
    Ok(span.end)
}

/// Call after the original RSP has been restored. A failed release keeps
/// the stack owned and recorded for a subsequent UnloadImage attempt.
pub(super) unsafe fn release_stack(
    state: &mut state::ServiceState<'_>,
    index: usize,
) -> Result<(), Status> {
    let record = state.images.records[index].as_mut().unwrap();
    let Some(base) = record.stack_base else {
        return Ok(());
    };
    let Some(tables) = state.active_tables else {
        return Err(Status::NOT_READY);
    };
    unsafe {
        if !tables.has_guard(base) {
            return Err(Status::DEVICE_ERROR);
        }
        state
            .allocator
            .release_image(base, STACK_SPAN_PAGES)
            .map_err(|_| Status::OUT_OF_RESOURCES)?;
        tables.set_guard(base, false);
        tables.flush();
    }
    record.stack_base = None;
    Ok(())
}

enum ExitAction {
    Unload,
    Resume(*mut SavedContext),
}

pub(super) unsafe fn exit(
    image: Handle,
    status: Status,
    data_size: usize,
    data: *const Char16,
) -> Status {
    let action = unsafe { prepare_exit(image, status, data_size, data) };
    match action {
        Ok(ExitAction::Unload) => unsafe { unload(image) },
        Ok(ExitAction::Resume(context)) => unsafe { od_fw_exit_image_context(context, status) },
        Err(error) => error,
    }
}

unsafe fn prepare_exit(
    image: Handle,
    status: Status,
    data_size: usize,
    data: *const Char16,
) -> Result<ExitAction, Status> {
    unsafe {
        state::try_with(|state| {
            let Some(index) = state.images.find(image) else {
                return Err(Status::INVALID_PARAMETER);
            };
            match state.images.records[index].as_ref().unwrap().phase {
                Phase::Loaded => return Ok(ExitAction::Unload),
                Phase::Running => {}
                Phase::Resident | Phase::Unloading | Phase::Finished => {
                    return Err(Status::INVALID_PARAMETER);
                }
            }
            let Some(active) = state.images.current.filter(|active| active.handle == image) else {
                return Err(Status::INVALID_PARAMETER);
            };
            if !state.exit.can_start_image() || state.events.current_tpl() != TPL_APPLICATION {
                return Err(Status::INVALID_PARAMETER);
            }
            if has_unfinished_child(state.images, image) {
                return Err(Status::ACCESS_DENIED);
            }

            if status != Status::SUCCESS && (data_size != 0 || !data.is_null()) {
                let address = data.addr() as u64;
                let size = state
                    .allocator
                    .pool_payload_size(&mut IdentityPoolMemory, address)
                    .map_err(|_| Status::INVALID_PARAMETER)?;
                if data_size < size_of::<Char16>()
                    || data_size as u64 > size
                    || !slice::from_raw_parts(data, data_size / size_of::<Char16>()).contains(&0)
                {
                    return Err(Status::INVALID_PARAMETER);
                }
                if !ptr::addr_of!((*active.frame).exit_data_output)
                    .read()
                    .is_null()
                {
                    let destination = &*ptr::addr_of!((*active.frame).exit_data);
                    destination.get().write(ExitData {
                        size: data_size,
                        data: data.cast_mut(),
                    });
                } else if state
                    .allocator
                    .free_pool(&mut IdentityPoolMemory, address)
                    .is_err()
                {
                    return Err(Status::OUT_OF_RESOURCES);
                }
            }
            Ok(ExitAction::Resume(ptr::addr_of_mut!(
                (*active.frame).context
            )))
        })
    }
    .unwrap_or(Err(Status::NOT_READY))
}

pub(super) fn has_unfinished_child(images: &super::ImageRegistry, parent: Handle) -> bool {
    images.records.iter().flatten().any(|record| {
        record.parent == parent
            && matches!(
                record.phase,
                Phase::Loaded | Phase::Running | Phase::Unloading
            )
    })
}
