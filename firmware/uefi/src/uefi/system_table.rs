//! x64 table layouts from UEFI 2.11 §4. The tables assembled at this stage
//! are private to firmware diagnostics.

use super::guid::Guid;
use super::runtime::{self, CapsuleHeader, ResetSystem, ResetType, Time, TimeCapabilities};
use super::status::Status;
use super::types::{Char16, Handle, TableHeader};
use crate::bs::event::{self, Event, EventNotify, RuntimeEventTable, Tpl};
use crate::bs::{config_table, exit, image, memory, misc, protocol};
use crate::crc32::with_zeroed_word;
use crate::device_path::DevicePathNode;
use crate::mem::map::{MemoryDescriptor, MemoryType};
use core::ffi::c_void;
use core::mem::{offset_of, size_of};
use core::{ptr, slice};

pub const SYSTEM_TABLE_SIGNATURE: u64 = 0x5453_5953_2049_4249;
pub const BOOT_SERVICES_SIGNATURE: u64 = 0x5652_4553_544f_4f42;
pub const RUNTIME_SERVICES_SIGNATURE: u64 = 0x5652_4553_544e_5552;

type RaiseTpl = unsafe extern "efiapi" fn(Tpl) -> Tpl;
type RestoreTpl = unsafe extern "efiapi" fn(Tpl);
type CreateEvent =
    unsafe extern "efiapi" fn(u32, Tpl, Option<EventNotify>, *mut c_void, *mut Event) -> Status;
type CreateEventEx = unsafe extern "efiapi" fn(
    u32,
    Tpl,
    Option<EventNotify>,
    *const c_void,
    *const Guid,
    *mut Event,
) -> Status;
type EventOperation = unsafe extern "efiapi" fn(Event) -> Status;
type SetTimer = unsafe extern "efiapi" fn(Event, u32, u64) -> Status;
type WaitForEvent = unsafe extern "efiapi" fn(usize, *const Event, *mut usize) -> Status;
type Stall = unsafe extern "efiapi" fn(usize) -> Status;
type SetWatchdogTimer = unsafe extern "efiapi" fn(usize, u64, usize, *const Char16) -> Status;
type InstallConfigurationTable = unsafe extern "efiapi" fn(*const Guid, *mut c_void) -> Status;
type ExitBootServices = unsafe extern "efiapi" fn(Handle, usize) -> Status;
type LoadImage = unsafe extern "efiapi" fn(
    u8,
    Handle,
    *const DevicePathNode,
    *const c_void,
    usize,
    *mut Handle,
) -> Status;
type StartImage = unsafe extern "efiapi" fn(Handle, *mut usize, *mut *mut Char16) -> Status;
type Exit = unsafe extern "efiapi" fn(Handle, Status, usize, *const Char16) -> Status;
type UnloadImage = unsafe extern "efiapi" fn(Handle) -> Status;
type GetNextMonotonicCount = unsafe extern "efiapi" fn(*mut u64) -> Status;
type ConnectController =
    unsafe extern "efiapi" fn(Handle, *const Handle, *const DevicePathNode, u8) -> Status;
type DisconnectController = unsafe extern "efiapi" fn(Handle, Handle, Handle) -> Status;
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
type RegisterProtocolNotify =
    unsafe extern "efiapi" fn(*const Guid, event::Event, *mut *mut c_void) -> Status;
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
type LocateDevicePath =
    unsafe extern "efiapi" fn(*const Guid, *mut *mut DevicePathNode, *mut Handle) -> Status;
type CalculateCrc32 = unsafe extern "efiapi" fn(*const c_void, usize, *mut u32) -> Status;
type CopyMem = unsafe extern "efiapi" fn(*mut c_void, *const c_void, usize);
type SetMem = unsafe extern "efiapi" fn(*mut c_void, usize, u8);

#[repr(C)]
pub struct BootServices {
    pub header: TableHeader,
    pub raise_tpl: RaiseTpl,
    pub restore_tpl: RestoreTpl,
    pub allocate_pages: AllocatePages,
    pub free_pages: FreePages,
    pub get_memory_map: GetMemoryMap,
    pub allocate_pool: AllocatePool,
    pub free_pool: FreePool,
    pub create_event: CreateEvent,
    pub set_timer: SetTimer,
    pub wait_for_event: WaitForEvent,
    pub signal_event: EventOperation,
    pub close_event: EventOperation,
    pub check_event: EventOperation,
    pub install_protocol_interface: InstallProtocolInterface,
    pub reinstall_protocol_interface: ReinstallProtocolInterface,
    pub uninstall_protocol_interface: UninstallProtocolInterface,
    pub handle_protocol: HandleProtocol,
    reserved: *const c_void,
    pub register_protocol_notify: RegisterProtocolNotify,
    pub locate_handle: LocateHandle,
    pub locate_device_path: LocateDevicePath,
    pub install_configuration_table: InstallConfigurationTable,
    pub load_image: LoadImage,
    pub start_image: StartImage,
    pub exit: Exit,
    pub unload_image: UnloadImage,
    pub exit_boot_services: ExitBootServices,
    pub get_next_monotonic_count: GetNextMonotonicCount,
    pub stall: Stall,
    pub set_watchdog_timer: SetWatchdogTimer,
    pub connect_controller: ConnectController,
    pub disconnect_controller: DisconnectController,
    pub open_protocol: OpenProtocol,
    pub close_protocol: CloseProtocol,
    pub open_protocol_information: OpenProtocolInformation,
    pub protocols_per_handle: ProtocolsPerHandle,
    pub locate_handle_buffer: LocateHandleBuffer,
    pub locate_protocol: LocateProtocol,
    pub install_multiple_protocol_interfaces: Option<protocol::InstallMultiple>,
    pub uninstall_multiple_protocol_interfaces: Option<protocol::UninstallMultiple>,
    pub calculate_crc32: CalculateCrc32,
    pub copy_mem: CopyMem,
    pub set_mem: SetMem,
    pub create_event_ex: CreateEventEx,
}

impl BootServices {
    pub const fn diagnostic() -> Self {
        Self {
            header: TableHeader::new(BOOT_SERVICES_SIGNATURE, size_of::<Self>()),
            raise_tpl: event::raise_tpl,
            restore_tpl: event::restore_tpl,
            allocate_pages: memory::allocate_pages,
            free_pages: memory::free_pages,
            get_memory_map: memory::get_memory_map,
            allocate_pool: memory::allocate_pool,
            free_pool: memory::free_pool,
            create_event: event::create_event,
            set_timer: event::set_timer,
            wait_for_event: event::wait_for_event,
            signal_event: event::signal_event,
            close_event: event::close_event,
            check_event: event::check_event,
            install_protocol_interface: protocol::install_protocol_interface,
            reinstall_protocol_interface: protocol::reinstall_protocol_interface,
            uninstall_protocol_interface: protocol::uninstall_protocol_interface,
            handle_protocol: protocol::handle_protocol,
            reserved: ptr::null(),
            register_protocol_notify: protocol::register_protocol_notify,
            locate_handle: protocol::locate_handle,
            locate_device_path: protocol::locate_device_path,
            install_configuration_table: config_table::install_configuration_table,
            load_image: image::load_image,
            start_image: image::start_image,
            exit: image::exit,
            unload_image: image::unload_image,
            exit_boot_services: exit::exit_boot_services,
            get_next_monotonic_count: misc::get_next_monotonic_count,
            stall: event::stall,
            set_watchdog_timer: event::set_watchdog_timer,
            connect_controller: protocol::connect_controller,
            disconnect_controller: protocol::disconnect_controller,
            open_protocol: protocol::open_protocol,
            close_protocol: protocol::close_protocol,
            open_protocol_information: protocol::open_protocol_information,
            protocols_per_handle: protocol::protocols_per_handle,
            locate_handle_buffer: protocol::locate_handle_buffer,
            locate_protocol: protocol::locate_protocol,
            install_multiple_protocol_interfaces: protocol::INSTALL_MULTIPLE,
            uninstall_multiple_protocol_interfaces: protocol::UNINSTALL_MULTIPLE,
            calculate_crc32: misc::calculate_crc32,
            copy_mem: misc::copy_mem,
            set_mem: misc::set_mem,
            create_event_ex: event::create_event_ex,
        }
    }

    pub fn update_crc32(&mut self) {
        // SAFETY: the C layout is a fully initialized header followed by pointer-sized
        // fields with no padding on x64, and the header is at offset zero.
        self.header.crc32 = unsafe { table_crc32(ptr::from_ref(self)) };
    }
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct RuntimeServices {
    pub header: TableHeader,
    pub get_time: unsafe extern "efiapi" fn(*mut Time, *mut TimeCapabilities) -> Status,
    pub set_time: unsafe extern "efiapi" fn(*const Time) -> Status,
    pub get_wakeup_time: unsafe extern "efiapi" fn(*mut u8, *mut u8, *mut Time) -> Status,
    pub set_wakeup_time: unsafe extern "efiapi" fn(u8, *const Time) -> Status,
    pub set_virtual_address_map:
        unsafe extern "efiapi" fn(usize, usize, u32, *const MemoryDescriptor) -> Status,
    pub convert_pointer: unsafe extern "efiapi" fn(usize, *mut *mut c_void) -> Status,
    pub get_variable: unsafe extern "efiapi" fn(
        *const Char16,
        *const Guid,
        *mut u32,
        *mut usize,
        *mut c_void,
    ) -> Status,
    pub get_next_variable_name:
        unsafe extern "efiapi" fn(*mut usize, *mut Char16, *mut Guid) -> Status,
    pub set_variable:
        unsafe extern "efiapi" fn(*const Char16, *const Guid, u32, usize, *const c_void) -> Status,
    pub get_next_high_monotonic_count: unsafe extern "efiapi" fn(*mut u32) -> Status,
    pub reset_system: ResetSystem,
    pub update_capsule:
        unsafe extern "efiapi" fn(*const *const CapsuleHeader, usize, u64) -> Status,
    pub query_capsule_capabilities: unsafe extern "efiapi" fn(
        *const *const CapsuleHeader,
        usize,
        *mut u64,
        *mut ResetType,
    ) -> Status,
    pub query_variable_info: unsafe extern "efiapi" fn(u32, *mut u64, *mut u64, *mut u64) -> Status,
}

impl RuntimeServices {
    pub const fn diagnostic(reset_system: ResetSystem) -> Self {
        Self {
            header: TableHeader::new(RUNTIME_SERVICES_SIGNATURE, size_of::<Self>()),
            get_time: runtime::get_time,
            set_time: runtime::set_time,
            get_wakeup_time: runtime::get_wakeup_time,
            set_wakeup_time: runtime::set_wakeup_time,
            set_virtual_address_map: runtime::set_virtual_address_map,
            convert_pointer: runtime::convert_pointer,
            get_variable: crate::rt::variable::get_variable,
            get_next_variable_name: crate::rt::variable::get_next_variable_name,
            set_variable: crate::rt::variable::set_variable,
            get_next_high_monotonic_count: runtime::get_next_high_monotonic_count,
            reset_system,
            update_capsule: runtime::update_capsule,
            query_capsule_capabilities: runtime::query_capsule_capabilities,
            query_variable_info: crate::rt::variable::query_variable_info,
        }
    }

    pub fn update_crc32(&mut self) {
        // SAFETY: like BootServices, the initialized header and pointer fields fill the layout.
        self.header.crc32 = unsafe { table_crc32(ptr::from_ref(self)) };
    }

    #[cfg(target_os = "uefi")]
    pub(crate) fn relocated(&self, map: &crate::rt::virt::VirtualMap<'_>) -> Result<Self, Status> {
        let mut table = *self;
        macro_rules! convert {
            ($($slot:ident),+ $(,)?) => {$(
                let address = map.convert_pointer(0, self.$slot as usize)?;
                // SAFETY: the map translates this EFI function's current
                // entry to its alias with the identical signature and ABI.
                table.$slot = unsafe { core::mem::transmute(address) };
            )+};
        }
        convert!(
            get_time,
            set_time,
            get_wakeup_time,
            set_wakeup_time,
            get_variable,
            get_next_variable_name,
            set_variable,
            get_next_high_monotonic_count,
            reset_system,
            update_capsule,
            query_capsule_capabilities,
            query_variable_info,
        );
        table.update_crc32();
        Ok(table)
    }
}

/// Firmware-owned storage for the Runtime Services table and its retained
/// event registrations. The EFI header size covers only the table field.
#[repr(C)]
pub struct RuntimeServicesStorage {
    pub table: RuntimeServices,
    pub(crate) event_table: *mut RuntimeEventTable,
    pub variables: *mut crate::rt::variable::RuntimeVariables,
    pub rtc: Option<crate::rt::time::Rtc>,
    pub fixed_hardware: Option<crate::acpi::manifest::AcpiPmEntry>,
    pub(crate) system: *mut SystemTable,
    pub(crate) at_runtime: bool,
    pub(crate) virtual_mode: bool,
    pub(crate) monotonic: Option<(u32, u32)>,
    pub(crate) driver_loaded: bool,
    pub(crate) properties: crate::uefi::runtime::RuntimeProperties,
    pub(crate) final_runtime_map: crate::rt::virt::RuntimeRanges,
    pub(crate) images: crate::rt::virt::RuntimeImages,
}

impl RuntimeServicesStorage {
    pub const fn diagnostic(reset_system: ResetSystem) -> Self {
        Self {
            table: RuntimeServices::diagnostic(reset_system),
            event_table: ptr::null_mut(),
            variables: ptr::null_mut(),
            rtc: None,
            fixed_hardware: None,
            system: ptr::null_mut(),
            at_runtime: false,
            virtual_mode: false,
            monotonic: None,
            driver_loaded: false,
            properties: crate::uefi::runtime::RuntimeProperties::new(false, false, false),
            final_runtime_map: crate::rt::virt::RuntimeRanges::new(),
            images: crate::rt::virt::RuntimeImages::new(),
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ConfigurationTable {
    pub vendor_guid: Guid,
    pub vendor_table: *mut c_void,
}

#[derive(Clone, Copy)]
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
    pub const fn firmware_vendor(&self) -> *const Char16 {
        self.firmware_vendor
    }

    /// Handle that carries the console input protocols (null before the console is published).
    pub fn console_in_handle(&self) -> Handle {
        self.console_in_handle
    }

    pub fn console_in(&self) -> *mut c_void {
        self.con_in
    }

    pub fn console_out(&self) -> *mut c_void {
        self.con_out
    }

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

    pub(crate) fn publish_configuration(&mut self, address: *mut ConfigurationTable, count: usize) {
        self.configuration_table = address;
        self.number_of_table_entries = count;
        self.update_crc32();
    }

    pub const fn configuration_tables(&self) -> (*mut ConfigurationTable, usize) {
        (self.configuration_table, self.number_of_table_entries)
    }

    #[cfg(target_os = "none")]
    pub(crate) fn publish_console(
        &mut self,
        handle: Handle,
        input: *mut c_void,
        output: *mut c_void,
    ) {
        self.console_in_handle = handle;
        self.con_in = input;
        self.console_out_handle = handle;
        self.con_out = output;
        self.standard_error_handle = handle;
        self.std_err = output;
        self.update_crc32();
    }

    pub(crate) fn clear_boot_services(&mut self) {
        self.console_in_handle = ptr::null_mut();
        self.con_in = ptr::null_mut();
        self.console_out_handle = ptr::null_mut();
        self.con_out = ptr::null_mut();
        self.standard_error_handle = ptr::null_mut();
        self.std_err = ptr::null_mut();
        self.boot_services = ptr::null_mut();
        self.update_crc32();
    }

    pub fn update_crc32(&mut self) {
        // SAFETY: the C layout starts with a header, and the explicit padding field
        // covers the only gap between x64 fields. The entire table is initialized.
        self.header.crc32 = unsafe { table_crc32(ptr::from_ref(self)) };
    }

    #[cfg(target_os = "uefi")]
    pub(crate) fn relocated(&self, map: &crate::rt::virt::VirtualMap<'_>) -> Result<Self, Status> {
        use crate::rt::virt::OPTIONAL_POINTER;

        let mut table = *self;
        table.firmware_vendor =
            ptr::with_exposed_provenance(map.convert_pointer(0, self.firmware_vendor.addr())?);
        table.runtime_services =
            ptr::with_exposed_provenance_mut(map.convert_pointer(0, self.runtime_services.addr())?);
        table.configuration_table = ptr::with_exposed_provenance_mut(
            map.convert_pointer(OPTIONAL_POINTER, self.configuration_table.addr())?,
        );
        table.update_crc32();
        Ok(table)
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

    unsafe extern "efiapi" fn unused_reset(_: ResetType, _: Status, _: usize, _: *const c_void) {
        unreachable!();
    }

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
        assert_eq!(offset_of!(BootServices, raise_tpl), 24);
        assert_eq!(offset_of!(BootServices, restore_tpl), 32);
        assert_eq!(offset_of!(BootServices, create_event), 80);
        assert_eq!(offset_of!(BootServices, set_timer), 88);
        assert_eq!(offset_of!(BootServices, wait_for_event), 96);
        assert_eq!(offset_of!(BootServices, signal_event), 104);
        assert_eq!(offset_of!(BootServices, close_event), 112);
        assert_eq!(offset_of!(BootServices, check_event), 120);
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
        assert_eq!(offset_of!(BootServices, register_protocol_notify), 168);
        assert_eq!(offset_of!(BootServices, stall), 248);
        assert_eq!(offset_of!(BootServices, set_watchdog_timer), 256);
        assert_eq!(offset_of!(BootServices, open_protocol), 280);
        assert_eq!(offset_of!(BootServices, close_protocol), 288);
        assert_eq!(offset_of!(BootServices, locate_handle), 176);
        assert_eq!(offset_of!(BootServices, locate_device_path), 184);
        assert_eq!(offset_of!(BootServices, install_configuration_table), 192);
        assert_eq!(offset_of!(BootServices, load_image), 200);
        assert_eq!(offset_of!(BootServices, start_image), 208);
        assert_eq!(offset_of!(BootServices, exit), 216);
        assert_eq!(offset_of!(BootServices, unload_image), 224);
        assert_eq!(offset_of!(BootServices, exit_boot_services), 232);
        assert_eq!(offset_of!(BootServices, get_next_monotonic_count), 240);
        assert_eq!(offset_of!(BootServices, connect_controller), 264);
        assert_eq!(offset_of!(BootServices, disconnect_controller), 272);
        assert_eq!(offset_of!(BootServices, open_protocol_information), 296);
        assert_eq!(offset_of!(BootServices, protocols_per_handle), 304);
        assert_eq!(offset_of!(BootServices, locate_handle_buffer), 312);
        assert_eq!(size_of::<protocol::OpenProtocolInformationEntry>(), 24);
        assert_eq!(offset_of!(BootServices, locate_protocol), 320);
        assert_eq!(
            offset_of!(BootServices, install_multiple_protocol_interfaces),
            328
        );
        assert_eq!(
            offset_of!(BootServices, uninstall_multiple_protocol_interfaces),
            336
        );
        assert_eq!(offset_of!(BootServices, calculate_crc32), 344);
        assert_eq!(offset_of!(BootServices, copy_mem), 352);
        assert_eq!(offset_of!(BootServices, set_mem), 360);
        assert_eq!(offset_of!(BootServices, create_event_ex), 368);
        assert_eq!(size_of::<Time>(), 16);
        assert_eq!(offset_of!(Time, nanosecond), 8);
        assert_eq!(offset_of!(Time, time_zone), 12);
        assert_eq!(size_of::<TimeCapabilities>(), 12);
        assert_eq!(offset_of!(TimeCapabilities, sets_to_zero), 8);
        assert_eq!(size_of::<CapsuleHeader>(), 28);
        assert_eq!(offset_of!(CapsuleHeader, capsule_image_size), 24);
        assert_eq!(offset_of!(RuntimeServices, get_time), 24);
        assert_eq!(offset_of!(RuntimeServices, set_time), 32);
        assert_eq!(offset_of!(RuntimeServices, get_wakeup_time), 40);
        assert_eq!(offset_of!(RuntimeServices, set_wakeup_time), 48);
        assert_eq!(offset_of!(RuntimeServices, set_virtual_address_map), 56);
        assert_eq!(offset_of!(RuntimeServices, convert_pointer), 64);
        assert_eq!(offset_of!(RuntimeServices, get_variable), 72);
        assert_eq!(offset_of!(RuntimeServices, get_next_variable_name), 80);
        assert_eq!(offset_of!(RuntimeServices, set_variable), 88);
        assert_eq!(
            offset_of!(RuntimeServices, get_next_high_monotonic_count),
            96
        );
        assert_eq!(offset_of!(RuntimeServices, reset_system), 104);
        assert_eq!(offset_of!(RuntimeServices, update_capsule), 112);
        assert_eq!(offset_of!(RuntimeServices, query_capsule_capabilities), 120);
        assert_eq!(offset_of!(RuntimeServices, query_variable_info), 128);
        assert_eq!(offset_of!(RuntimeServicesStorage, table), 0);
        assert_eq!(offset_of!(RuntimeServicesStorage, event_table), 136);
    }

    #[test]
    fn runtime_status_slots_report_unavailable_without_accessing_arguments() {
        let table = RuntimeServices::diagnostic(unused_reset);
        // These diagnostic entry points return before following any pointer.
        let statuses = unsafe {
            [
                (table.get_time)(ptr::null_mut(), ptr::null_mut()),
                (table.set_time)(ptr::null()),
                (table.get_wakeup_time)(ptr::null_mut(), ptr::null_mut(), ptr::null_mut()),
                (table.set_wakeup_time)(0, ptr::null()),
                (table.set_virtual_address_map)(0, 0, 0, ptr::null()),
                (table.convert_pointer)(0, ptr::null_mut()),
                (table.get_variable)(
                    ptr::null(),
                    ptr::null(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                ),
                (table.get_next_variable_name)(ptr::null_mut(), ptr::null_mut(), ptr::null_mut()),
                (table.set_variable)(ptr::null(), ptr::null(), 0, 0, ptr::null()),
                (table.get_next_high_monotonic_count)(ptr::null_mut()),
                (table.update_capsule)(ptr::null(), 0, 0),
                (table.query_capsule_capabilities)(
                    ptr::null(),
                    0,
                    ptr::null_mut(),
                    ptr::null_mut(),
                ),
                (table.query_variable_info)(0, ptr::null_mut(), ptr::null_mut(), ptr::null_mut()),
            ]
        };
        assert_eq!(statuses, [Status::UNSUPPORTED; 13]);
    }

    #[test]
    fn exit_clears_boot_fields_and_preserves_runtime_fields() {
        let runtime = ptr::without_provenance_mut::<RuntimeServices>(0x1000);
        let boot = ptr::without_provenance_mut::<BootServices>(0x2000);
        let config = ptr::without_provenance_mut::<ConfigurationTable>(0x3000);
        let console = ptr::without_provenance_mut::<c_void>(0x4000);
        let mut system = SystemTable::diagnostic(ptr::null(), runtime, boot);
        system.console_in_handle = console;
        system.con_in = console;
        system.console_out_handle = console;
        system.con_out = console;
        system.standard_error_handle = console;
        system.std_err = console;
        system.publish_configuration(config, 1);
        system.clear_boot_services();
        assert!(system.console_in_handle.is_null());
        assert!(system.con_in.is_null());
        assert!(system.console_out_handle.is_null());
        assert!(system.con_out.is_null());
        assert!(system.standard_error_handle.is_null());
        assert!(system.std_err.is_null());
        assert!(system.boot_services.is_null());
        assert_eq!(system.runtime_services, runtime);
        assert_eq!(system.configuration_tables(), (config, 1));
        assert_eq!(system.header.crc32, unsafe { table_crc32(&system) });
    }

    #[test]
    fn recalculating_ignores_old_crc_and_covers_payload() {
        let mut runtime = RuntimeServices::diagnostic(unused_reset);
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
