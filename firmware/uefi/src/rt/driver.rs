//! The private runtime driver entry and its retained service implementations.

use core::{arch::asm, ffi::c_void, ptr, slice};

use crate::{
    bs::event::{EVT_SIGNAL_VIRTUAL_ADDRESS_CHANGE, Event, TPL_NOTIFY},
    mem::map::MemoryDescriptor,
};

use crate::uefi::{
    runtime::{RT_PROPERTIES_TABLE_GUID, ResetType, RuntimeProperties, Time, TimeCapabilities},
    status::Status,
    system_table::{RuntimeServices, RuntimeServicesStorage, SystemTable},
    types::Handle,
};

use super::{
    reset::{self, ResetCommand},
    time::{Cmos, Rtc},
    variable::RuntimeVariables,
    virt::{MAX_IMAGES, OPTIONAL_POINTER, VirtualMap},
};

// UEFI serializes these services. This slot and the code that accesses it
// belong to the runtime image; the value points into RuntimeServicesData.
static mut ROOT: *mut RuntimeServicesStorage = ptr::null_mut();
static mut ACTIVE_MAP: *const VirtualMap<'static> = ptr::null();

static TABLE: RuntimeServices = RuntimeServices {
    get_time,
    set_time,
    set_virtual_address_map,
    convert_pointer,
    get_next_high_monotonic_count,
    ..RuntimeServices::diagnostic(reset_system)
};

fn root() -> *mut RuntimeServicesStorage {
    // SAFETY: access is serialized by firmware initialization and the UEFI
    // runtime call contract. The slot itself is in the executing image.
    unsafe { ptr::addr_of!(ROOT).read_volatile() }
}

/// # Safety
/// The firmware passes its live SystemTable and initialized private storage.
/// This is the single BSP start of the built-in driver.
pub unsafe fn entry(image: Handle, system: *mut SystemTable) -> Status {
    if image.is_null() || system.is_null() || !root().is_null() {
        return Status::INVALID_PARAMETER;
    }
    unsafe {
        let storage = (*system).runtime_services.cast::<RuntimeServicesStorage>();
        if storage.is_null() || (*system).boot_services.is_null() || (*storage).system != system {
            return Status::INVALID_PARAMETER;
        }
        if (*storage).driver_loaded {
            return Status::ALREADY_STARTED;
        }
        // Reserve this shared instance before any Boot Service can notify
        // a caller that starts a second copy of the PE.
        (*storage).driver_loaded = true;
        let boot = (*system).boot_services;
        let create = (*boot).create_event;
        let mut event = ptr::null_mut();
        let status = create(
            EVT_SIGNAL_VIRTUAL_ADDRESS_CHANGE,
            TPL_NOTIFY,
            Some(virtualize_private_pointers),
            storage.cast(),
            &mut event,
        );
        if status != Status::SUCCESS {
            (*storage).driver_loaded = false;
            return status;
        }
        let present = (*storage)
            .variables
            .as_ref()
            .is_some_and(|vars| vars.nvram_base() != 0);
        ptr::addr_of_mut!((*storage).properties).write(RuntimeProperties::new(
            (*storage).rtc.is_some(),
            present,
            (*storage).fixed_hardware.is_some(),
        ));
        let status = ((*boot).install_configuration_table)(
            &RT_PROPERTIES_TABLE_GUID,
            ptr::addr_of_mut!((*storage).properties).cast(),
        );
        if status != Status::SUCCESS {
            let _ = ((*boot).close_event)(event);
            (*storage).driver_loaded = false;
            return status;
        }
        if let Some(variables) = (*storage).variables.as_mut() {
            (*storage).monotonic = variables
                .advance_monotonic_high()
                .ok()
                .map(|high| (high, 0));
        }
        ptr::addr_of_mut!(ROOT).write_volatile(storage);
        // A volatile copy keeps the template and its PE base relocations.
        ptr::addr_of_mut!((*storage).table).write(ptr::addr_of!(TABLE).read_volatile());
        (*storage).table.update_crc32();
    }
    Status::SUCCESS
}

unsafe extern "efiapi" fn set_virtual_address_map(
    size: usize,
    stride: usize,
    version: u32,
    descriptors: *const MemoryDescriptor,
) -> Status {
    let storage = root();
    if storage.is_null()
        || unsafe { !(*storage).at_runtime || (*storage).virtual_mode }
        || !unsafe { ptr::addr_of!(ACTIVE_MAP).read_volatile() }.is_null()
    {
        return Status::UNSUPPORTED;
    }
    match unsafe { apply_virtual_address_map(storage, size, stride, version, descriptors) } {
        Ok(()) => Status::SUCCESS,
        Err(status) => status,
    }
}

unsafe fn apply_virtual_address_map(
    storage: *mut RuntimeServicesStorage,
    size: usize,
    stride: usize,
    version: u32,
    descriptors: *const MemoryDescriptor,
) -> Result<(), Status> {
    unsafe {
        let map = VirtualMap::new(
            descriptors,
            size,
            stride,
            version,
            (*storage).final_runtime_map.descriptors(),
        )?;
        let system = (*storage).system;
        let event_table = (*storage).event_table;
        let variables = (*storage).variables;
        let runtime = (*storage).table.relocated(&map)?;
        let virtual_system = (*system).relocated(&map)?;
        // The built-in notification converts these dynamic pointers using
        // the EFI ABI. Verify them now, before any callback can mutate state.
        for address in [
            storage.addr(),
            system.addr(),
            event_table.addr(),
            variables.addr(),
        ] {
            map.convert_pointer(0, address)?;
        }
        map.convert_pointer(OPTIONAL_POINTER, (*variables).nvram_base())?;
        map.convert_pointer(OPTIONAL_POINTER, (*variables).protected_scratch_base())?;
        (*event_table).validate_virtual_notifications(&map)?;

        let mut deltas = [0u64; MAX_IMAGES];
        for (image, delta) in (*storage).images.0.iter().zip(&mut deltas) {
            if image.base != 0 {
                *delta = map
                    .translate_range(image.base, image.size)?
                    .wrapping_sub(image.base);
            }
        }

        // Physical callbacks see the old tables and use the temporary map.
        // Every operation that can return an error completed above.
        ptr::addr_of_mut!(ACTIVE_MAP).write_volatile(ptr::addr_of!(map).cast());
        (*event_table).notify_virtual_address_change();
        ptr::addr_of_mut!(ACTIVE_MAP).write_volatile(ptr::null());

        // The caller owns these page tables. Use the existing physical
        // aliases without altering their permissions or the caller's CR3.
        let (cr0, flags) = suspend_write_protection();
        for (image, delta) in (*storage).images.0.iter().zip(deltas) {
            if image.fixup_count != 0 && delta != 0 {
                for &fixup in slice::from_raw_parts(image.fixups, image.fixup_count) {
                    fixup.apply(image.base as usize, delta);
                }
            }
        }
        resume_write_protection(cr0, flags);
        ptr::addr_of_mut!((*storage).table).write(runtime);
        system.write(virtual_system);
        ptr::addr_of_mut!((*storage).virtual_mode).write(true);
    }
    Ok(())
}

unsafe extern "efiapi" fn convert_pointer(disposition: usize, address: *mut *mut c_void) -> Status {
    let map = unsafe { ptr::addr_of!(ACTIVE_MAP).read_volatile() };
    if map.is_null() {
        return Status::UNSUPPORTED;
    }
    if address.is_null() {
        return Status::INVALID_PARAMETER;
    }
    unsafe {
        match (*map).convert_pointer(disposition, address.read_unaligned().addr()) {
            Ok(virtual_address) => {
                address.write_unaligned(ptr::with_exposed_provenance_mut(virtual_address));
                Status::SUCCESS
            }
            Err(status) => status,
        }
    }
}

unsafe extern "efiapi" fn virtualize_private_pointers(_: Event, context: *mut c_void) {
    // The physical context, the virtual map and every address below were
    // checked before any notification was allowed to run.
    let storage = context.cast::<RuntimeServicesStorage>();
    unsafe {
        let convert = (*storage).table.convert_pointer;
        let convert_address = |address: *mut c_void, disposition| {
            let mut address = address;
            assert_eq!(convert(disposition, &mut address), Status::SUCCESS);
            address
        };
        let variables = (*storage).variables;
        let nvram = ptr::with_exposed_provenance_mut((*variables).nvram_base());
        (*variables).set_nvram_base(convert_address(nvram, OPTIONAL_POINTER).addr());
        let scratch = ptr::with_exposed_provenance_mut((*variables).protected_scratch_base());
        (*variables).set_protected_scratch_base(convert_address(scratch, OPTIONAL_POINTER).addr());
        (*storage).variables = convert_address(variables.cast(), 0).cast();
        (*storage).system = convert_address((*storage).system.cast(), 0).cast();
        (*storage).event_table = convert_address((*storage).event_table.cast(), 0).cast();
        ptr::addr_of_mut!(ROOT).write_volatile(convert_address(context, 0).cast());
    }
}

unsafe fn suspend_write_protection() -> (u64, u64) {
    let cr0: u64;
    let flags: u64;
    unsafe {
        asm!(
            "pushfq",
            "pop {flags}",
            "cli",
            "mov rax, cr0",
            "mov {cr0}, rax",
            "and rax, -65537",
            "mov cr0, rax",
            cr0 = out(reg) cr0,
            flags = out(reg) flags,
            out("rax") _,
        );
    }
    (cr0, flags)
}

unsafe fn resume_write_protection(cr0: u64, flags: u64) {
    unsafe {
        asm!("mov cr0, rax", in("rax") cr0, options(nostack));
        if flags & (1 << 9) != 0 {
            asm!("sti", options(nostack));
        }
    }
}

pub(super) unsafe fn with_variables(
    operation: impl FnOnce(&mut RuntimeVariables) -> Status,
) -> Status {
    let storage = root();
    if storage.is_null() {
        return Status::NOT_READY;
    }
    unsafe {
        let Some(variables) = (*storage).variables.as_mut() else {
            return Status::NOT_READY;
        };
        operation(variables)
    }
}

unsafe extern "efiapi" fn get_next_high_monotonic_count(count: *mut u32) -> Status {
    if count.is_null() {
        return Status::INVALID_PARAMETER;
    }
    let storage = root();
    if storage.is_null() || unsafe { !(*storage).at_runtime } {
        return Status::UNSUPPORTED;
    }
    unsafe {
        with_variables(|variables| {
            if variables.nvram_base() == 0 {
                return Status::UNSUPPORTED;
            }
            match variables.advance_monotonic_high() {
                Ok(high) => {
                    count.write_unaligned(high);
                    Status::SUCCESS
                }
                Err(status) => status,
            }
        })
    }
}

unsafe extern "efiapi" fn get_time(time: *mut Time, capabilities: *mut TimeCapabilities) -> Status {
    if time.is_null() {
        return Status::INVALID_PARAMETER;
    }
    let storage = root();
    if storage.is_null() {
        return Status::UNSUPPORTED;
    }
    unsafe {
        let Some(rtc) = (*storage).rtc.as_ref() else {
            return Status::UNSUPPORTED;
        };
        let mut cmos = IndexedCmos::new();
        let value = match rtc.get_time(&mut cmos) {
            Ok(value) => value,
            Err(status) => return status,
        };
        time.write_unaligned(value);
        if !capabilities.is_null() {
            capabilities.write_unaligned(Rtc::capabilities());
        }
    }
    Status::SUCCESS
}

unsafe extern "efiapi" fn set_time(time: *const Time) -> Status {
    if time.is_null() {
        return Status::INVALID_PARAMETER;
    }
    let storage = root();
    if storage.is_null() {
        return Status::UNSUPPORTED;
    }
    unsafe {
        let Some(rtc) = (*storage).rtc.as_mut() else {
            return Status::UNSUPPORTED;
        };
        rtc.set_time(&mut IndexedCmos::new(), &time.read_unaligned())
    }
}

const CMOS_INDEX_PORT: u16 = 0x70;
const CMOS_DATA_PORT: u16 = 0x71;

struct IndexedCmos(u8);

impl IndexedCmos {
    /// The caller synchronizes access to the validated board RTC.
    unsafe fn new() -> Self {
        Self(unsafe { in_byte(CMOS_INDEX_PORT) })
    }
}

impl Drop for IndexedCmos {
    fn drop(&mut self) {
        unsafe { out_byte(CMOS_INDEX_PORT, self.0) };
    }
}

impl Cmos for IndexedCmos {
    fn read(&mut self, register: u8) -> u8 {
        unsafe {
            out_byte(CMOS_INDEX_PORT, register | 0x80);
            in_byte(CMOS_DATA_PORT)
        }
    }

    fn write(&mut self, register: u8, value: u8) {
        unsafe {
            out_byte(CMOS_INDEX_PORT, register | 0x80);
            out_byte(CMOS_DATA_PORT, value);
        }
    }
}

unsafe extern "efiapi" fn reset_system(
    reset_type: ResetType,
    _: Status,
    _: usize,
    _: *const c_void,
) {
    let storage = root();
    if !storage.is_null()
        && let Some(hardware) = unsafe { (*storage).fixed_hardware }
    {
        match reset::command(hardware, reset_type) {
            ResetCommand::Reset { port, value } => unsafe { out_byte(port, value) },
            ResetCommand::Shutdown { port, sleep_bits } => unsafe {
                let previous: u16;
                asm!("in ax, dx", in("dx") port, out("ax") previous, options(nostack, preserves_flags));
                let value = (previous & !(0b111 << 10)) | sleep_bits;
                asm!("out dx, ax", in("dx") port, in("ax") value, options(nostack, preserves_flags));
            },
        }
    }
    // ResetSystem is non-returning. On boards with the advertised hardware
    // the stop is latched by the I/O bus before execution reaches this loop.
    unsafe { asm!("cli", "2:", "hlt", "jmp 2b", options(noreturn, nostack)) };
}

unsafe fn in_byte(port: u16) -> u8 {
    let value: u8;
    unsafe { asm!("in al, dx", in("dx") port, out("al") value, options(nostack, preserves_flags)) };
    value
}

unsafe fn out_byte(port: u16, value: u8) {
    unsafe { asm!("out dx, al", in("dx") port, in("al") value, options(nostack, preserves_flags)) };
}
