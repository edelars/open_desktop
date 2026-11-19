//! The owned EFI invocation stack and the Rust side of the assembly boundary.

use super::{release_record, state, unload_image};
use crate::bs::{event::TPL_APPLICATION, memory::IdentityPoolMemory};
use crate::mem::{map::MemoryType, pages::ImageMemoryRange};
use crate::paging::PAGE_BYTES;
use crate::pe::Subsystem;
use crate::uefi::{
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

struct StartPlan {
    stack_top: u64,
    index: usize,
    entry: u64,
    system: *mut SystemTable,
    previous: Option<ActiveImage>,
    previous_exit_handle: Handle,
}

type Entry = unsafe extern "efiapi" fn(Handle, *mut SystemTable) -> Status;

unsafe extern "sysv64" {
    fn od_fw_invoke_image(
        context: *mut SavedContext,
        stack_top: u64,
        entry: Entry,
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
        context: SavedContext {
            saved_stack: 0,
            saved_flags: 0,
            reserved: [0; 0x30],
            fx_state: [0; 512],
        },
        exit_data: UnsafeCell::new(ExitData::EMPTY),
        exit_data_output: exit_data,
    };
    let frame_pointer = ptr::addr_of_mut!(frame);
    let plan = match unsafe { prepare(image, frame_pointer) } {
        Ok(plan) => plan,
        Err(error) => return error,
    };

    // The state guard and every borrowed service object are gone. The frame
    // stays on the suspended caller stack and is addressed only by raw pointer.
    let status = unsafe {
        od_fw_invoke_image(
            ptr::addr_of_mut!((*frame_pointer).context),
            plan.stack_top,
            mem::transmute::<usize, Entry>(plan.entry as usize),
            image,
            plan.system,
        )
    };

    let completion = unsafe { finish(plan, status) };
    let data = unsafe { frame.exit_data.get().read() };
    if !frame.exit_data_output.is_null() {
        unsafe {
            frame.exit_data_output.write(data.data);
            exit_data_size.write(data.size);
        }
    }
    completion
}

unsafe fn prepare(image: Handle, frame: *mut Frame) -> Result<StartPlan, Status> {
    unsafe {
        state::try_with(|state| {
            let Some(index) = state.images.find(image) else {
                return Err(Status::INVALID_PARAMETER);
            };
            let record = state.images.records[index].as_mut().unwrap();
            if record.phase != Phase::Loaded
                || !state.exit.can_start_image()
                || state.events.current_tpl() != TPL_APPLICATION
            {
                return Err(Status::INVALID_PARAMETER);
            }
            let (Some(system), Some(tables)) = (state.system_table, state.active_tables) else {
                return Err(Status::NOT_READY);
            };
            // The stack allocation and the two ordinary completion edits
            // must fit even when the image makes no memory-map calls itself.
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
            if !tables.verify(span.clone(), true) {
                return Err(Status::DEVICE_ERROR);
            }

            state.allocator.commit_image(stack);
            tables.set_guard(span.start, true);
            tables.flush();
            record.phase = Phase::Running;
            record.stack_base = Some(span.start);
            let active = ActiveImage {
                handle: image,
                frame,
            };
            let previous = state.images.current.replace(active);
            let previous_exit_handle = state.exit.replace_image(image);
            Ok(StartPlan {
                stack_top: span.end,
                index,
                entry: record.mapped.entry_address,
                system: system.as_ptr(),
                previous,
                previous_exit_handle,
            })
        })
    }
    .unwrap_or(Err(Status::NOT_READY))
}

unsafe fn finish(plan: StartPlan, status: Status) -> Status {
    let result = unsafe {
        state::try_with(|mut state| {
            state.images.current = plan.previous;
            state.exit.replace_image(plan.previous_exit_handle);
            state.events.restore_tpl(TPL_APPLICATION);
            let record = state.images.records[plan.index].as_mut().unwrap();
            record.phase = if record.mapped.subsystem == Subsystem::Application || status.is_error()
            {
                Phase::Finished
            } else {
                Phase::Resident
            };
            let phase = record.phase;
            if let Err(status) = release_stack(&mut state, plan.index) {
                return status;
            }
            if phase == Phase::Finished {
                let release = release_record(&mut state, plan.index);
                if release != Status::SUCCESS {
                    return release;
                }
            }
            status
        })
    };
    match result {
        Some(status) => status,
        // A successful ExitBootServices terminates this firmware invocation.
        // Its image cannot return control to the boot-services caller.
        None => unsafe {
            core::arch::asm!("cli", "2: hlt", "jmp 2b", options(noreturn, nostack));
        },
    }
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
        Ok(ExitAction::Unload) => unsafe { unload_image(image) },
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
                Phase::Resident | Phase::Finished => return Err(Status::INVALID_PARAMETER),
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
        record.parent == parent && matches!(record.phase, Phase::Loaded | Phase::Running)
    })
}
