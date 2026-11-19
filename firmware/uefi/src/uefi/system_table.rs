//! x64 table layouts from UEFI 2.11 §4. The tables assembled at this stage
//! are private to firmware diagnostics.

use super::guid::Guid;
use super::status::Status;
use super::types::{Char16, Handle, TableHeader};
use crate::bs::{memory, misc, protocol};
use crate::crc32::with_zeroed_word;
use crate::mem::map::{MemoryDescriptor, MemoryType};
use core::ffi::c_void;
use core::mem::{offset_of, size_of};
use core::{ptr, slice};

pub const SYSTEM_TABLE_SIGNATURE: u64 = 0x5453_5953_2049_4249;
pub const BOOT_SERVICES_SIGNATURE: u64 = 0x5652_4553_544f_4f42;
pub const RUNTIME_SERVICES_SIGNATURE: u64 = 0x5652_4553_544e_5552;

type AllocatePages = unsafe extern "efiapi" fn(u32, MemoryType, usize, *mut u64) -> Status;
type FreePages = unsafe extern "efiapi" fn(u64, usize) -> Status;
type GetMemoryMap = unsafe extern "efiapi" fn(
    *mut usize,
    *mut MemoryDescriptor,
    *mut usize,
    *mut usize,
    *mut u32,
) -> Status;
type AllocatePool = unsafe extern "efiapi" fn(MemoryType, usize, *mut *mut c_void) -> Status;
type FreePool = unsafe extern "efiapi" fn(*mut c_void) -> Status;
type InstallProtocolInterface =
    unsafe extern "efiapi" fn(*mut Handle, *const Guid, u32, *mut c_void) -> Status;
type ReinstallProtocolInterface =
    unsafe extern "efiapi" fn(Handle, *const Guid, *mut c_void, *mut c_void) -> Status;
type UninstallProtocolInterface =
    unsafe extern "efiapi" fn(Handle, *const Guid, *mut c_void) -> Status;
type HandleProtocol = unsafe extern "efiapi" fn(Handle, *const Guid, *mut *mut c_void) -> Status;
type LocateProtocol =
    unsafe extern "efiapi" fn(*const Guid, *mut c_void, *mut *mut c_void) -> Status;
type OpenProtocol =
    unsafe extern "efiapi" fn(Handle, *const Guid, *mut *mut c_void, Handle, Handle, u32) -> Status;
type CloseProtocol = unsafe extern "efiapi" fn(Handle, *const Guid, Handle, Handle) -> Status;
type LocateHandle =
    unsafe extern "efiapi" fn(u32, *const Guid, *mut c_void, *mut usize, *mut Handle) -> Status;
type OpenProtocolInformation = unsafe extern "efiapi" fn(
    Handle,
    *const Guid,
    *mut *mut protocol::OpenProtocolInformationEntry,
    *mut usize,
) -> Status;
type ProtocolsPerHandle =
    unsafe extern "efiapi" fn(Handle, *mut *mut *mut Guid, *mut usize) -> Status;
type LocateHandleBuffer = unsafe extern "efiapi" fn(
    u32,
    *const Guid,
    *mut c_void,
    *mut usize,
    *mut *mut Handle,
) -> Status;
type CalculateCrc32 = unsafe extern "efiapi" fn(*const c_void, usize, *mut u32) -> Status;
type CopyMem = unsafe extern "efiapi" fn(*mut c_void, *const c_void, usize);
type SetMem = unsafe extern "efiapi" fn(*mut c_void, usize, u8);

#[repr(C)]
pub struct BootServices {
    pub header: TableHeader,
    raise_tpl: *const c_void,
    restore_tpl: *const c_void,
    pub allocate_pages: AllocatePages,
    pub free_pages: FreePages,
    pub get_memory_map: GetMemoryMap,
    pub allocate_pool: AllocatePool,
    pub free_pool: FreePool,
    create_event: *const c_void,
    set_timer: *const c_void,
    wait_for_event: *const c_void,
    signal_event: *const c_void,
    close_event: *const c_void,
    check_event: *const c_void,
    pub install_protocol_interface: InstallProtocolInterface,
    pub reinstall_protocol_interface: ReinstallProtocolInterface,
    pub uninstall_protocol_interface: UninstallProtocolInterface,
    pub handle_protocol: HandleProtocol,
    reserved: *const c_void,
    register_protocol_notify: *const c_void,
    pub locate_handle: LocateHandle,
    locate_device_path: *const c_void,
    install_configuration_table: *const c_void,
    load_image: *const c_void,
    start_image: *const c_void,
    exit: *const c_void,
    unload_image: *const c_void,
    exit_boot_services: *const c_void,
    get_next_monotonic_count: *const c_void,
    stall: *const c_void,
    set_watchdog_timer: *const c_void,
    connect_controller: *const c_void,
    disconnect_controller: *const c_void,
    pub open_protocol: OpenProtocol,
    pub close_protocol: CloseProtocol,
    pub open_protocol_information: OpenProtocolInformation,
    pub protocols_per_handle: ProtocolsPerHandle,
    pub locate_handle_buffer: LocateHandleBuffer,
    pub locate_protocol: LocateProtocol,
    install_multiple_protocol_interfaces: *const c_void,
    uninstall_multiple_protocol_interfaces: *const c_void,
    pub calculate_crc32: CalculateCrc32,
    pub copy_mem: CopyMem,
    pub set_mem: SetMem,
    create_event_ex: *const c_void,
}

impl BootServices {
    pub const fn diagnostic() -> Self {
        Self {
            header: TableHeader::new(BOOT_SERVICES_SIGNATURE, size_of::<Self>()),
            raise_tpl: ptr::null(),
            restore_tpl: ptr::null(),
            allocate_pages: memory::allocate_pages,
            free_pages: memory::free_pages,
            get_memory_map: memory::get_memory_map,
            allocate_pool: memory::allocate_pool,
            free_pool: memory::free_pool,
            create_event: ptr::null(),
            set_timer: ptr::null(),
            wait_for_event: ptr::null(),
            signal_event: ptr::null(),
            close_event: ptr::null(),
            check_event: ptr::null(),
            install_protocol_interface: protocol::install_protocol_interface,
            reinstall_protocol_interface: protocol::reinstall_protocol_interface,
            uninstall_protocol_interface: protocol::uninstall_protocol_interface,
            handle_protocol: protocol::handle_protocol,
            reserved: ptr::null(),
            register_protocol_notify: ptr::null(),
            locate_handle: protocol::locate_handle,
            locate_device_path: ptr::null(),
            install_configuration_table: ptr::null(),
            load_image: ptr::null(),
            start_image: ptr::null(),
            exit: ptr::null(),
            unload_image: ptr::null(),
            exit_boot_services: ptr::null(),
            get_next_monotonic_count: ptr::null(),
            stall: ptr::null(),
            set_watchdog_timer: ptr::null(),
            connect_controller: ptr::null(),
            disconnect_controller: ptr::null(),
            open_protocol: protocol::open_protocol,
            close_protocol: protocol::close_protocol,
            open_protocol_information: protocol::open_protocol_information,
            protocols_per_handle: protocol::protocols_per_handle,
            locate_handle_buffer: protocol::locate_handle_buffer,
            locate_protocol: protocol::locate_protocol,
            install_multiple_protocol_interfaces: ptr::null(),
            uninstall_multiple_protocol_interfaces: ptr::null(),
            calculate_crc32: misc::calculate_crc32,
            copy_mem: misc::copy_mem,
            set_mem: misc::set_mem,
            create_event_ex: ptr::null(),
        }
    }

    pub fn update_crc32(&mut self) {
        // SAFETY: the C layout is a fully initialized header followed by pointer-sized
        // fields with no padding on x64, and the header is at offset zero.
        self.header.crc32 = unsafe { table_crc32(ptr::from_ref(self)) };
    }
}

#[repr(C)]
pub struct RuntimeServices {
    pub header: TableHeader,
    get_time: *const c_void,
    set_time: *const c_void,
    get_wakeup_time: *const c_void,
    set_wakeup_time: *const c_void,
    set_virtual_address_map: *const c_void,
    convert_pointer: *const c_void,
    get_variable: *const c_void,
    get_next_variable_name: *const c_void,
    set_variable: *const c_void,
    get_next_high_monotonic_count: *const c_void,
    reset_system: *const c_void,
    update_capsule: *const c_void,
    query_capsule_capabilities: *const c_void,
    query_variable_info: *const c_void,
}

impl RuntimeServices {
    pub const fn diagnostic() -> Self {
        Self {
            header: TableHeader::new(RUNTIME_SERVICES_SIGNATURE, size_of::<Self>()),
            get_time: ptr::null(),
            set_time: ptr::null(),
            get_wakeup_time: ptr::null(),
            set_wakeup_time: ptr::null(),
            set_virtual_address_map: ptr::null(),
            convert_pointer: ptr::null(),
            get_variable: ptr::null(),
            get_next_variable_name: ptr::null(),
            set_variable: ptr::null(),
            get_next_high_monotonic_count: ptr::null(),
            reset_system: ptr::null(),
            update_capsule: ptr::null(),
            query_capsule_capabilities: ptr::null(),
            query_variable_info: ptr::null(),
        }
    }

    pub fn update_crc32(&mut self) {
        // SAFETY: like BootServices, the initialized header and pointer fields fill the layout.
        self.header.crc32 = unsafe { table_crc32(ptr::from_ref(self)) };
    }
}

#[repr(C)]
pub struct ConfigurationTable {
    pub vendor_guid: Guid,
    pub vendor_table: *mut c_void,
}

#[repr(C)]
pub struct SystemTable {
    pub header: TableHeader,
    firmware_vendor: *const Char16,
    firmware_revision: u32,
    padding: u32,
    console_in_handle: Handle,
    con_in: *mut c_void,
    console_out_handle: Handle,
    con_out: *mut c_void,
    standard_error_handle: Handle,
    std_err: *mut c_void,
    pub runtime_services: *mut RuntimeServices,
    pub boot_services: *mut BootServices,
    number_of_table_entries: usize,
    configuration_table: *mut ConfigurationTable,
}

impl SystemTable {
    pub const fn diagnostic(
        firmware_vendor: *const Char16,
        runtime_services: *mut RuntimeServices,
        boot_services: *mut BootServices,
    ) -> Self {
        Self {
            header: TableHeader::new(SYSTEM_TABLE_SIGNATURE, size_of::<Self>()),
            firmware_vendor,
            firmware_revision: 0x0000_0100,
            padding: 0,
            console_in_handle: ptr::null_mut(),
            con_in: ptr::null_mut(),
            console_out_handle: ptr::null_mut(),
            con_out: ptr::null_mut(),
            standard_error_handle: ptr::null_mut(),
            std_err: ptr::null_mut(),
            runtime_services,
            boot_services,
            number_of_table_entries: 0,
            configuration_table: ptr::null_mut(),
        }
    }

    pub fn update_crc32(&mut self) {
        // SAFETY: the C layout starts with a header, and the explicit padding field
        // covers the only gap between x64 fields. The entire table is initialized.
        self.header.crc32 = unsafe { table_crc32(ptr::from_ref(self)) };
    }
}

// The table types above are defined for a 64-bit firmware ABI. The size invariants
// also protect callers of table_crc32 from introducing implicit padding.
const _: () = {
    assert!(size_of::<TableHeader>() == 24);
    assert!(size_of::<BootServices>() == 24 + 44 * 8);
    assert!(size_of::<RuntimeServices>() == 24 + 14 * 8);
    assert!(size_of::<SystemTable>() == 24 + 12 * 8);
};

/// The pointed-to table must contain only initialized bytes, begin with a
/// TableHeader, and remain readable for its complete size.
unsafe fn table_crc32<T>(table: *const T) -> u32 {
    let bytes = unsafe { slice::from_raw_parts(table.cast::<u8>(), size_of::<T>()) };
    with_zeroed_word(bytes, offset_of!(TableHeader, crc32))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crc32::crc32;
    use core::mem::align_of;

    #[test]
    fn table_layout_matches_x64_uefi() {
        assert_eq!(align_of::<TableHeader>(), 8);
        assert_eq!(offset_of!(TableHeader, crc32), 16);
        assert_eq!(size_of::<Guid>(), 16);
        assert_eq!(align_of::<Guid>(), 4);
        assert_eq!(size_of::<ConfigurationTable>(), 24);
        assert_eq!(offset_of!(ConfigurationTable, vendor_table), 16);
        assert_eq!(offset_of!(SystemTable, firmware_revision), 32);
        assert_eq!(offset_of!(SystemTable, console_in_handle), 40);
        assert_eq!(offset_of!(SystemTable, runtime_services), 88);
        assert_eq!(offset_of!(SystemTable, boot_services), 96);
        assert_eq!(offset_of!(SystemTable, configuration_table), 112);
        assert_eq!(offset_of!(BootServices, allocate_pages), 40);
        assert_eq!(offset_of!(BootServices, free_pages), 48);
        assert_eq!(offset_of!(BootServices, get_memory_map), 56);
        assert_eq!(offset_of!(BootServices, allocate_pool), 64);
        assert_eq!(offset_of!(BootServices, free_pool), 72);
        assert_eq!(offset_of!(BootServices, install_protocol_interface), 128);
        assert_eq!(offset_of!(BootServices, reinstall_protocol_interface), 136);
        assert_eq!(offset_of!(BootServices, uninstall_protocol_interface), 144);
        assert_eq!(offset_of!(BootServices, handle_protocol), 152);
        assert_eq!(offset_of!(BootServices, reserved), 160);
        assert_eq!(offset_of!(BootServices, open_protocol), 280);
        assert_eq!(offset_of!(BootServices, close_protocol), 288);
        assert_eq!(offset_of!(BootServices, locate_handle), 176);
        assert_eq!(offset_of!(BootServices, open_protocol_information), 296);
        assert_eq!(offset_of!(BootServices, protocols_per_handle), 304);
        assert_eq!(offset_of!(BootServices, locate_handle_buffer), 312);
        assert_eq!(size_of::<protocol::OpenProtocolInformationEntry>(), 24);
        assert_eq!(offset_of!(BootServices, locate_protocol), 320);
        assert_eq!(offset_of!(BootServices, calculate_crc32), 344);
        assert_eq!(offset_of!(BootServices, copy_mem), 352);
        assert_eq!(offset_of!(BootServices, set_mem), 360);
        assert_eq!(offset_of!(BootServices, create_event_ex), 368);
        assert_eq!(offset_of!(RuntimeServices, get_variable), 72);
        assert_eq!(offset_of!(RuntimeServices, query_variable_info), 128);
    }

    #[test]
    fn recalculating_ignores_old_crc_and_covers_payload() {
        let mut runtime = RuntimeServices::diagnostic();
        runtime.update_crc32();
        let expected = runtime.header.crc32;
        assert_ne!(expected, 0);
        runtime.update_crc32();
        assert_eq!(runtime.header.crc32, expected);

        runtime.header.crc32 = 0;
        let bytes = unsafe {
            slice::from_raw_parts(
                ptr::from_ref(&runtime).cast::<u8>(),
                size_of::<RuntimeServices>(),
            )
        };
        assert_eq!(crc32(bytes), expected);

        let mut boot = BootServices::diagnostic();
        let mut system = SystemTable::diagnostic(ptr::null(), &mut runtime, &mut boot);
        system.update_crc32();
        let previous = system.header.crc32;
        system.boot_services = ptr::null_mut();
        system.update_crc32();
        assert_ne!(system.header.crc32, previous);
        assert!(boot.reserved.is_null());
    }
}
