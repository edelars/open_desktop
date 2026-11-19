//! Binary of the firmware image. The S7-04 entry validates the boot handoff and prepares its
//! own identity page tables, and prints the initial EFI memory map after switching stacks.

#![no_std]
#![no_main]

use core::arch::asm;
use core::ffi::c_void;
use core::ptr;

use core::panic::PanicInfo;
use od_firmware::console::serial_term::{self, init as com1_init, write as com1_write};

use od_firmware::HANDOFF_GPA;
use od_firmware::bs::event::{
    EVT_NOTIFY_SIGNAL, EVT_NOTIFY_WAIT, EVT_RUNTIME, EVT_SIGNAL_EXIT_BOOT_SERVICES,
    EVT_SIGNAL_VIRTUAL_ADDRESS_CHANGE, EVT_TIMER, Event, EventNotify, TIMER_CANCEL, TIMER_PERIODIC,
    TIMER_RELATIVE, TPL_APPLICATION, TPL_CALLBACK, TPL_HIGH_LEVEL, TPL_NOTIFY,
    VIRTUAL_ADDRESS_CHANGE,
};
use od_firmware::bs::exit::{BEFORE_EXIT_BOOT_SERVICES, EXIT_BOOT_SERVICES, set_boot_image_handle};
use od_firmware::bs::memory::IdentityPoolMemory;
use od_firmware::bs::protocol::{
    LOCATE_ALL_HANDLES, LOCATE_BY_PROTOCOL, LOCATE_BY_REGISTER_NOTIFY, OPEN_BY_DRIVER,
    OPEN_GET_PROTOCOL,
};
use od_firmware::device_path::{DEVICE_PATH_PROTOCOL_GUID, DevicePathNode};
use od_firmware::handoff::{
    BOOT_PAGE_TABLES_GPA, FirmwareFvApp, HANDOFF_MINOR_MAX, HANDOFF_WINDOW_SIZE, Handoff,
};
use od_firmware::idt::Idt;
use od_firmware::mem::map::{
    DESCRIPTOR_VERSION, MAX_INITIAL_DESCRIPTORS, MemoryDescriptor, MemoryMap, MemoryMapError,
    MemoryType,
};
use od_firmware::mem::pages::{AllocateAt, PageAllocator, PageError, PageOwner, PoolError};
use od_firmware::paging::{BootPaging, PAGE_BYTES, PagingError, ShadowSections};
use od_firmware::rt::variable::RuntimeVariables;
use od_firmware::stop::{
    STOP_BOOT_SERVICES, STOP_EXCEPTION, STOP_INTERNAL, STOP_INVALID_RDI, STOP_MEMORY_MAP,
    STOP_NO_BOOT_OPTIONS, STOP_PAGE_ALLOCATOR, STOP_PAGING, STOP_POOL_ALLOCATOR, fixed_hex,
    format_stop_line,
};
use od_firmware::uefi::guid::Guid;
use od_firmware::uefi::runtime::{RESET_COLD, RESET_WARM, ResetType};
use od_firmware::uefi::status::Status;
use od_firmware::uefi::system_table::{BootServices, RuntimeServicesStorage, SystemTable};
use od_firmware::uefi::types::Handle;

unsafe extern "C" {
    static __shadow_read_only_start: u8;
    static __shadow_writable_start: u8;
    fn od_fw_install_gdt();
    static od_fw_exception_stubs: u8;
}

const VERSION_LINE: &[u8] = concat!("OD-FW version ", env!("CARGO_PKG_VERSION"), "\r\n").as_bytes();
const HANDOFF_VALIDATED_LINE: &[u8] = b"OD-FW S7-04 handoff validated\r\n";
const PAGING_PREPARED_LINE: &[u8] = b"OD-FW S7-04 page tables prepared\r\n";
const PAGING_ACTIVE_LINE: &[u8] = b"OD-FW S7-04 page tables active\r\n";
// Three live allocations can each introduce two additional descriptors.
const BOOT_DESCRIPTOR_CAPACITY: usize = MAX_INITIAL_DESCRIPTORS + 6;
static FIRMWARE_VENDOR: [u16; 13] = [
    0x4f, 0x70, 0x65, 0x6e, 0x20, 0x44, 0x65, 0x73, 0x6b, 0x74, 0x6f, 0x70, 0,
];
#[repr(C)]
struct RetainedRuntime {
    services: RuntimeServicesStorage,
    variables: RuntimeVariables,
}
static mut BOOT_DESCRIPTORS: [MemoryDescriptor; BOOT_DESCRIPTOR_CAPACITY] =
    [MemoryDescriptor::EMPTY; BOOT_DESCRIPTOR_CAPACITY];
static mut BOOT_PAGE_OWNERS: [PageOwner; BOOT_DESCRIPTOR_CAPACITY] =
    [PageOwner::Initial; BOOT_DESCRIPTOR_CAPACITY];
const DIAGNOSTIC_PROTOCOL: Guid = Guid {
    data1: 0x0a44_97ec,
    data2: 0x12b6,
    data3: 0x478b,
    data4: [0xa3, 0x63, 0x07, 0x3c, 0x00, 0x41, 0x25, 0x61],
};

fn halt_forever() -> ! {
    loop {
        unsafe { asm!("cli", "hlt", options(nomem, nostack)) };
    }
}

fn stop(code: u8, detail: u64) -> ! {
    com1_init();
    com1_write(&format_stop_line(code, detail));
    halt_forever()
}

/// Rust entry point, called by `_start` with `RDI` as the first argument.
#[unsafe(no_mangle)]
pub extern "C" fn od_fw_main(handoff_gpa: u64) -> ! {
    if handoff_gpa != HANDOFF_GPA {
        stop(STOP_INVALID_RDI, handoff_gpa);
    }

    let parsed = Handoff::parse_for_boot(boot_handoff_bytes(), HANDOFF_MINOR_MAX);
    let handoff = match &parsed {
        Ok(handoff) => handoff,
        Err(error) => {
            let (code, detail) = error.stop_code_detail();
            stop(code, detail);
        }
    };

    com1_init();
    com1_write(VERSION_LINE);
    com1_write(HANDOFF_VALIDATED_LINE);
    let (root, stack_top) = match prepare_paging(handoff) {
        Ok(layout) => layout,
        Err(error) => stop(STOP_PAGING, error.stop_detail()),
    };
    com1_write(PAGING_PREPARED_LINE);
    // SAFETY: the prepared root maps the current code and a writable
    // guard-backed stack. CALL establishes the System V stack alignment
    // after replacing RSP. The continuation halts and never returns here.
    unsafe {
        asm!(
            "mov cr3, rax",
            "mov rsp, rsi",
            "call {entry}",
            "ud2",
            in("rax") root,
            in("rdi") root,
            in("rsi") stack_top,
            entry = sym paging_active,
            options(noreturn),
        )
    }
}

extern "C" fn paging_active(active_cr3: u64) -> ! {
    com1_write(PAGING_ACTIVE_LINE);
    // SAFETY: the guest is at CPL0, its own CR3 and stack are active,
    // and the linked descriptor table is mapped in writable firmware RAM.
    unsafe { od_fw_install_gdt() };
    com1_write(b"OD-FW S7-04 GDT active\r\n");
    install_idt();
    enable_firmware_fp_controls();
    com1_write(b"OD-FW S7-04 IDT and CR4 active\r\n");
    let parsed = Handoff::parse_for_boot(boot_handoff_bytes(), HANDOFF_MINOR_MAX);
    let handoff = match &parsed {
        Ok(handoff) => handoff,
        Err(error) => {
            let (code, detail) = error.stop_code_detail();
            stop(code, detail);
        }
    };
    let paging = match BootPaging::plan(handoff) {
        Ok(paging) => paging,
        Err(error) => stop(STOP_PAGING, error.stop_detail()),
    };
    // SAFETY: initialization runs once on the BSP. From this point the allocator
    // owns both buffers, and Boot Services access them only through that allocator.
    let descriptors = unsafe { &mut *ptr::addr_of_mut!(BOOT_DESCRIPTORS) };
    let map =
        match MemoryMap::from_active_paging(&paging, active_cr3, shadow_sections(), descriptors) {
            Ok(map) => map,
            Err(MemoryMapError::TablesNotActive) => stop(STOP_MEMORY_MAP, 1),
            Err(MemoryMapError::InvalidPlacement) => stop(STOP_MEMORY_MAP, 2),
            Err(MemoryMapError::BufferTooSmall { required }) => {
                stop(STOP_MEMORY_MAP, 3 | ((required as u64) << 32))
            }
            Err(MemoryMapError::Paging(error)) => stop(STOP_PAGING, error.stop_detail()),
        };
    com1_write_memory_map(&map);
    let ownership = unsafe { &mut *ptr::addr_of_mut!(BOOT_PAGE_OWNERS) };
    let mut allocator = require_pages(PageAllocator::new(map, ownership));
    let key = verify_boot_page_allocator(&mut allocator, active_cr3);
    com1_write(b"OD-FW S7-04 page allocator verified key=0x");
    com1_write(&fixed_hex(key));
    com1_write(b"\r\n");
    let key = verify_boot_pool_allocator(&mut allocator);
    com1_write(b"OD-FW S7-04 pool allocator verified key=0x");
    com1_write(&fixed_hex(key));
    com1_write(b"\r\n");
    if let Some(byte) = serial_term::try_read() {
        com1_write(b"OD-FW S7-04 COM1 RX byte=0x");
        com1_write(&fixed_hex(u64::from(byte)));
        com1_write(b"\r\n");
    }
    // The live allocator's free pages are identity-mapped and writable.
    let runtime_address = match allocator.allocate_pool(
        &mut IdentityPoolMemory,
        MemoryType::RUNTIME_SERVICES_DATA,
        size_of::<RetainedRuntime>() as u64,
    ) {
        Ok(address) => address,
        Err(_) => stop(STOP_BOOT_SERVICES, 60),
    };
    let retained = ptr::with_exposed_provenance_mut::<RetainedRuntime>(runtime_address as usize);
    // SAFETY: calculate both field addresses in the allocation before initialization.
    let (runtime, variables) = unsafe {
        (
            ptr::addr_of_mut!((*retained).services),
            ptr::addr_of_mut!((*retained).variables),
        )
    };
    // SAFETY: this pool is exclusive, identity-mapped runtime data.
    let restore = unsafe {
        RuntimeVariables::initialize(
            variables,
            &od_firmware::console::COM1_DEVICE_PATH,
            handoff.nvram_base(),
        )
    };
    let Ok(restore) = restore else {
        stop(STOP_BOOT_SERVICES, 76)
    };
    if let Some(report) = restore {
        if report.invalid_header {
            com1_write(b"OD-FW S7-10 NVRAM ignored invalid store header\r\n");
        }
        com1_write(b"OD-FW S7-10 NVRAM restored discarded=0x");
        com1_write(&fixed_hex(u64::from(report.discarded_records)));
        com1_write(b" missing_defaults=0x");
        com1_write(&fixed_hex(u64::from(report.missing_defaults)));
        com1_write(b"\r\n");
    }
    let mut runtime_storage = RuntimeServicesStorage::diagnostic(diagnostic_reset_system);
    runtime_storage.variables = variables;
    runtime_storage.table.update_crc32();
    // SAFETY: the pool allocated a live, aligned runtime storage slot.
    unsafe { runtime.write(runtime_storage) };
    let service_map_key = allocator.map().key() as usize;
    // SAFETY: this is the one BSP initialization; the diagnostic has finished
    // borrowing the allocator. All ensuing accesses go through Boot Services.
    unsafe {
        od_firmware::bs::initialize(allocator, handoff.tsc_hz(), read_tsc);
        od_firmware::bs::attach_image_paging(&paging);
    }
    let fixed_hardware = match handoff.acpi_fixed_hardware() {
        Ok(entry) => entry,
        Err(_) => stop(STOP_BOOT_SERVICES, 58),
    };
    if let Some(pm) = fixed_hardware {
        // SAFETY: the ACPI manifest validated the reset target from the handoff.
        unsafe {
            od_firmware::bs::attach_watchdog(pm.reset_reg, pm.reset_value, reset_for_watchdog)
        };
    }
    let mut boot = BootServices::diagnostic();
    boot.update_crc32();
    let mut system = SystemTable::diagnostic(
        FIRMWARE_VENDOR.as_ptr(),
        runtime.cast(),
        ptr::addr_of_mut!(boot),
    );
    system.update_crc32();
    let system_pointer = ptr::addr_of_mut!(system);
    // SAFETY: the System Table stays on the boot stack and the Runtime
    // Services storage resides in its dedicated runtime pool.
    unsafe { od_firmware::bs::attach_system_tables(system_pointer, runtime) };
    verify_boot_services(&system);
    verify_memory_services(&system, service_map_key);
    verify_protocol_services(&system);
    verify_event_services(&system);
    verify_protocol_notify(&system);
    verify_timer_services(&system, handoff.tsc_hz());
    verify_configuration_services(system_pointer);
    verify_watchdog_services(&system, fixed_hardware.is_some());
    verify_service_boundaries(&system);
    // SAFETY: the BSP owns the initialized Boot Services registry.
    if unsafe { od_firmware::device_path::utilities::install() } != Status::SUCCESS {
        stop(STOP_BOOT_SERVICES, 67);
    }
    if unsafe { od_firmware::bs::image::install_firmware_volume() } != Status::SUCCESS {
        stop(STOP_BOOT_SERVICES, 68);
    }
    if unsafe { od_firmware::console::install() } != Status::SUCCESS {
        stop(STOP_BOOT_SERVICES, 74);
    }
    boot_fv_image(&system, handoff.fv_app());
    verify_exit_boot_services(system_pointer, handoff.tsc_hz());
    // Keep the full wire representations inspectable in RAM at the diagnostic halt.
    unsafe {
        asm!(
            "/* {system} {boot} {runtime} */",
            system = in(reg) &system,
            boot = in(reg) &boot,
            runtime = in(reg) runtime,
            options(nostack, preserves_flags),
        );
    }
    halt_forever()
}

#[inline(never)]
fn verify_boot_services(system: &SystemTable) {
    let mut buffer = [0u8; 10];
    let mut checksum = 0;
    // SAFETY: the System Table points to the live diagnostic tables on the boot
    // stack. Each service receives valid buffers and the correct x64 EFI ABI.
    unsafe {
        let boot = ptr::addr_of!(system.boot_services).read_volatile();
        let calculate_crc32 = ptr::addr_of!((*boot).calculate_crc32).read_volatile();
        let copy_mem = ptr::addr_of!((*boot).copy_mem).read_volatile();
        let set_mem = ptr::addr_of!((*boot).set_mem).read_volatile();
        set_mem(buffer.as_mut_ptr().cast(), buffer.len(), 0xa5);
        if calculate_crc32(buffer.as_ptr().cast(), buffer.len(), &mut checksum) != Status::SUCCESS
            || checksum != 0xd42a82ad
        {
            stop(STOP_BOOT_SERVICES, 1);
        }

        let data = buffer.as_mut_ptr();
        let shifted = data.add(1);
        copy_mem(data.cast(), b"123456789".as_ptr().cast(), 9);
        copy_mem(shifted.cast(), data.cast(), 9);
        if calculate_crc32(shifted.cast(), 9, &mut checksum) != Status::SUCCESS
            || checksum != 0xcbf43926
        {
            stop(STOP_BOOT_SERVICES, 2);
        }
        let runtime = ptr::addr_of!(system.runtime_services).read_volatile();
        com1_write(b"OD-FW S7-05 table CRC ST=0x");
        com1_write(&fixed_hex(u64::from(system.header.crc32)));
        com1_write(b" BS=0x");
        com1_write(&fixed_hex(u64::from((*boot).header.crc32)));
        com1_write(b" RT=0x");
        com1_write(&fixed_hex(u64::from((*runtime).header.crc32)));
        com1_write(b"\r\n");
    }
    com1_write(b"OD-FW S7-05 Boot Services utilities verified ST=0x");
    com1_write(&fixed_hex(ptr::from_ref(system).addr() as u64));
    com1_write(b"\r\n");
}

#[inline(never)]
fn verify_memory_services(system: &SystemTable, initial_key: usize) {
    let mut snapshot = [MemoryDescriptor::EMPTY; BOOT_DESCRIPTOR_CAPACITY];
    let mut size = 0;
    let mut key = usize::MAX;
    let mut descriptor_size = 0;
    let mut version = 0;
    // SAFETY: the BSP owns the initialized allocator and the private System Table.
    // Each ABI call receives live, disjoint output buffers of the stated sizes.
    unsafe {
        let boot = ptr::addr_of!(system.boot_services).read_volatile();
        let get_map = ptr::addr_of!((*boot).get_memory_map).read_volatile();
        let allocate_pages = ptr::addr_of!((*boot).allocate_pages).read_volatile();
        let free_pages = ptr::addr_of!((*boot).free_pages).read_volatile();
        let allocate_pool = ptr::addr_of!((*boot).allocate_pool).read_volatile();
        let free_pool = ptr::addr_of!((*boot).free_pool).read_volatile();
        if get_map(
            &mut size,
            ptr::null_mut(),
            &mut key,
            &mut descriptor_size,
            &mut version,
        ) != Status::BUFFER_TOO_SMALL
            || size == 0
            || key != usize::MAX
            || descriptor_size != size_of::<MemoryDescriptor>()
            || version != DESCRIPTOR_VERSION
        {
            stop(STOP_BOOT_SERVICES, 3);
        }
        if get_map(
            &mut size,
            snapshot.as_mut_ptr(),
            &mut key,
            ptr::null_mut(),
            ptr::null_mut(),
        ) != Status::SUCCESS
            || key != initial_key
        {
            stop(STOP_BOOT_SERVICES, 4);
        }

        let mut page = 0;
        if allocate_pages(0, MemoryType::CONVENTIONAL, 2, &mut page) != Status::SUCCESS
            || page & (PAGE_BYTES - 1) != 0
        {
            stop(STOP_BOOT_SERVICES, 5);
        }
        let mut already_owned = page;
        if allocate_pages(2, MemoryType::LOADER_DATA, 2, &mut already_owned) != Status::NOT_FOUND
            || already_owned != page
        {
            stop(STOP_BOOT_SERVICES, 6);
        }
        let mut pool = ptr::null_mut();
        if allocate_pool(MemoryType::CONVENTIONAL, 32, &mut pool) != Status::SUCCESS
            || pool.is_null()
            || pool.addr() & 7 != 0
            || pool.addr() + 32 > page as usize
        {
            stop(STOP_BOOT_SERVICES, 7);
        }
        size = size_of_val(&snapshot);
        if get_map(
            &mut size,
            snapshot.as_mut_ptr(),
            &mut key,
            ptr::null_mut(),
            ptr::null_mut(),
        ) != Status::SUCCESS
            || key != initial_key + 2
        {
            stop(STOP_BOOT_SERVICES, 8);
        }
        if free_pool(pool) != Status::SUCCESS
            || free_pool(pool) != Status::INVALID_PARAMETER
            || free_pages(page, 2) != Status::SUCCESS
            || free_pages(page, 2) != Status::NOT_FOUND
        {
            stop(STOP_BOOT_SERVICES, 9);
        }
        if get_map(
            &mut size,
            snapshot.as_mut_ptr(),
            &mut key,
            ptr::null_mut(),
            ptr::null_mut(),
        ) != Status::SUCCESS
            || key != initial_key + 4
        {
            stop(STOP_BOOT_SERVICES, 10);
        }
    }
    com1_write(b"OD-FW S7-05 Boot Services memory verified key=0x");
    com1_write(&fixed_hex(key as u64));
    com1_write(b"\r\n");
}

#[inline(never)]
fn verify_protocol_services(system: &SystemTable) {
    // SAFETY: the BSP owns the private tables and all the following output
    // storage. The protocol pointer is backed by a live pool allocation.
    unsafe {
        let boot = ptr::addr_of!(system.boot_services).read_volatile();
        let allocate = ptr::addr_of!((*boot).allocate_pool).read_volatile();
        let free = ptr::addr_of!((*boot).free_pool).read_volatile();
        let install = ptr::addr_of!((*boot).install_protocol_interface).read_volatile();
        let reinstall = ptr::addr_of!((*boot).reinstall_protocol_interface).read_volatile();
        let uninstall = ptr::addr_of!((*boot).uninstall_protocol_interface).read_volatile();
        let handle_protocol = ptr::addr_of!((*boot).handle_protocol).read_volatile();
        let locate_protocol = ptr::addr_of!((*boot).locate_protocol).read_volatile();

        let mut contents = ptr::null_mut();
        if allocate(MemoryType::LOADER_DATA, 1, &mut contents) != Status::SUCCESS
            || contents.is_null()
        {
            stop(STOP_BOOT_SERVICES, 11);
        }
        contents.cast::<u8>().write(0xa5);
        let mut handle = ptr::null_mut();
        if install(&mut handle, &DIAGNOSTIC_PROTOCOL, 0, contents) != Status::SUCCESS
            || handle.is_null()
            || install(&mut handle, &DIAGNOSTIC_PROTOCOL, 0, contents) != Status::INVALID_PARAMETER
        {
            stop(STOP_BOOT_SERVICES, 12);
        }

        let mut found = ptr::null_mut();
        if handle_protocol(handle, &DIAGNOSTIC_PROTOCOL, &mut found) != Status::SUCCESS
            || found != contents
            || locate_protocol(&DIAGNOSTIC_PROTOCOL, ptr::null_mut(), &mut found) != Status::SUCCESS
            || found != contents
            || found.cast::<u8>().read() != 0xa5
        {
            stop(STOP_BOOT_SERVICES, 13);
        }
        verify_tracked_open(boot, handle, contents);
        if reinstall(handle, &DIAGNOSTIC_PROTOCOL, contents, ptr::null_mut()) != Status::SUCCESS
            || handle_protocol(handle, &DIAGNOSTIC_PROTOCOL, &mut found) != Status::SUCCESS
            || !found.is_null()
        {
            stop(STOP_BOOT_SERVICES, 14);
        }
        if uninstall(handle, &DIAGNOSTIC_PROTOCOL, ptr::null_mut()) != Status::SUCCESS
            || locate_protocol(&DIAGNOSTIC_PROTOCOL, ptr::null_mut(), &mut found)
                != Status::NOT_FOUND
            || handle_protocol(handle, &DIAGNOSTIC_PROTOCOL, &mut found)
                != Status::INVALID_PARAMETER
        {
            stop(STOP_BOOT_SERVICES, 15);
        }
        if contents.cast::<u8>().read() != 0xa5 || free(contents) != Status::SUCCESS {
            stop(STOP_BOOT_SERVICES, 16);
        }
        verify_device_path(boot);
        verify_protocol_batches(boot);
    }
    com1_write(b"OD-FW S7-05 Boot Services protocol registry verified\r\n");
}

/// The BSP owns the live diagnostic table and the installed interface.
#[inline(never)]
unsafe fn verify_tracked_open(boot: *const BootServices, handle: Handle, contents: *mut c_void) {
    let agent_guid = Guid {
        data1: DIAGNOSTIC_PROTOCOL.data1 + 1,
        ..DIAGNOSTIC_PROTOCOL
    };
    let mut owner = ptr::null_mut();
    let mut other = ptr::null_mut();
    let mut found = ptr::null_mut();
    unsafe {
        let open = ptr::addr_of!((*boot).open_protocol).read_volatile();
        let close = ptr::addr_of!((*boot).close_protocol).read_volatile();
        let disconnect = ptr::addr_of!((*boot).disconnect_controller).read_volatile();
        let install = ptr::addr_of!((*boot).install_protocol_interface).read_volatile();
        let reinstall = ptr::addr_of!((*boot).reinstall_protocol_interface).read_volatile();
        let uninstall = ptr::addr_of!((*boot).uninstall_protocol_interface).read_volatile();
        if install(&mut owner, &agent_guid, 0, ptr::null_mut()) != Status::SUCCESS
            || install(&mut other, &agent_guid, 0, ptr::null_mut()) != Status::SUCCESS
        {
            stop(STOP_BOOT_SERVICES, 17);
        }
        if open(
            handle,
            &DIAGNOSTIC_PROTOCOL,
            &mut found,
            owner,
            handle,
            OPEN_BY_DRIVER,
        ) != Status::SUCCESS
            || found != contents
        {
            stop(STOP_BOOT_SERVICES, 18);
        }
        found = ptr::null_mut();
        if open(
            handle,
            &DIAGNOSTIC_PROTOCOL,
            &mut found,
            owner,
            handle,
            OPEN_BY_DRIVER,
        ) != Status::ALREADY_STARTED
            || found != contents
        {
            stop(STOP_BOOT_SERVICES, 19);
        }
        found = ptr::null_mut();
        if open(
            handle,
            &DIAGNOSTIC_PROTOCOL,
            &mut found,
            other,
            handle,
            OPEN_BY_DRIVER,
        ) != Status::ACCESS_DENIED
            || !found.is_null()
            || disconnect(handle, ptr::null_mut(), ptr::null_mut()) != Status::DEVICE_ERROR
            || uninstall(handle, &DIAGNOSTIC_PROTOCOL, contents) != Status::ACCESS_DENIED
            || reinstall(handle, &DIAGNOSTIC_PROTOCOL, contents, ptr::null_mut())
                != Status::ACCESS_DENIED
        {
            stop(STOP_BOOT_SERVICES, 20);
        }
        if open(
            handle,
            &DIAGNOSTIC_PROTOCOL,
            &mut found,
            other,
            ptr::null_mut(),
            OPEN_GET_PROTOCOL,
        ) != Status::SUCCESS
            || found != contents
        {
            stop(STOP_BOOT_SERVICES, 21);
        }
        verify_protocol_enumeration(boot, handle, owner, other, &agent_guid);
        if close(handle, &DIAGNOSTIC_PROTOCOL, owner, handle) != Status::SUCCESS
            || close(handle, &DIAGNOSTIC_PROTOCOL, owner, handle) != Status::NOT_FOUND
        {
            stop(STOP_BOOT_SERVICES, 21);
        }
        if uninstall(owner, &agent_guid, ptr::null_mut()) != Status::SUCCESS
            || uninstall(other, &agent_guid, ptr::null_mut()) != Status::SUCCESS
        {
            stop(STOP_BOOT_SERVICES, 22);
        }
    }
}

/// The BSP owns the live diagnostic table, agents and tested protocol.
#[inline(never)]
unsafe fn verify_protocol_enumeration(
    boot: *const BootServices,
    handle: Handle,
    owner: Handle,
    other: Handle,
    agent_guid: &Guid,
) {
    unsafe {
        let locate = ptr::addr_of!((*boot).locate_handle).read_volatile();
        let locate_buffer = ptr::addr_of!((*boot).locate_handle_buffer).read_volatile();
        let protocols = ptr::addr_of!((*boot).protocols_per_handle).read_volatile();
        let information = ptr::addr_of!((*boot).open_protocol_information).read_volatile();
        let free = ptr::addr_of!((*boot).free_pool).read_volatile();

        let mut handles = [ptr::null_mut(); 3];
        let mut bytes = 0;
        if locate(
            LOCATE_ALL_HANDLES,
            ptr::null(),
            ptr::null_mut(),
            &mut bytes,
            ptr::null_mut(),
        ) != Status::BUFFER_TOO_SMALL
            || bytes != size_of_val(&handles)
            || locate(
                LOCATE_ALL_HANDLES,
                ptr::null(),
                ptr::null_mut(),
                &mut bytes,
                handles.as_mut_ptr(),
            ) != Status::SUCCESS
            || handles != [handle, owner, other]
        {
            stop(STOP_BOOT_SERVICES, 23);
        }

        let mut count = 0;
        let mut pool_handles = ptr::null_mut();
        if locate_buffer(
            LOCATE_BY_PROTOCOL,
            agent_guid,
            ptr::null_mut(),
            &mut count,
            &mut pool_handles,
        ) != Status::SUCCESS
            || pool_handles.is_null()
            || count != 2
            || pool_handles.read() != owner
            || pool_handles.add(1).read() != other
            || free(pool_handles.cast()) != Status::SUCCESS
        {
            stop(STOP_BOOT_SERVICES, 24);
        }

        let mut guids = ptr::null_mut();
        if protocols(handle, &mut guids, &mut count) != Status::SUCCESS
            || guids.is_null()
            || count != 1
        {
            stop(STOP_BOOT_SERVICES, 25);
        }
        let guid = guids.read();
        if guid.is_null()
            || guid.read() != DIAGNOSTIC_PROTOCOL
            || free(guids.cast()) != Status::SUCCESS
        {
            stop(STOP_BOOT_SERVICES, 25);
        }
        com1_write(b"OD-FW S7-05 diagnostic handle protocols=0x");
        com1_write(&fixed_hex(count as u64));
        com1_write(b"\r\n");

        let mut opens = ptr::null_mut();
        if information(handle, &DIAGNOSTIC_PROTOCOL, &mut opens, &mut count) != Status::SUCCESS
            || opens.is_null()
            || count != 2
        {
            stop(STOP_BOOT_SERVICES, 26);
        }
        let driver = opens.read();
        let reader = opens.add(1).read();
        if driver.agent_handle != owner
            || driver.controller_handle != handle
            || driver.attributes != OPEN_BY_DRIVER
            || driver.open_count != 1
            || reader.agent_handle != other
            || !reader.controller_handle.is_null()
            || reader.attributes != OPEN_GET_PROTOCOL
            || reader.open_count != 1
            || free(opens.cast()) != Status::SUCCESS
        {
            stop(STOP_BOOT_SERVICES, 26);
        }
    }
}

/// The BSP owns the live diagnostic table.
#[inline(never)]
unsafe fn verify_device_path(boot: *const BootServices) {
    static PARENT: [u8; 10] = [1, 1, 6, 0, 0, 1, 0x7f, 0xff, 4, 0];
    static CHILD: [u8; 16] = [1, 1, 6, 0, 0, 1, 1, 1, 6, 0, 0, 2, 0x7f, 0xff, 4, 0];
    static INPUT: [u8; 34] = [
        1, 1, 6, 0, 0, 1, 1, 1, 6, 0, 0, 2, 4, 4, 8, 0, b'x', 0, 0, 0, 0x7f, 1, 4, 0, 1, 1, 6, 0,
        0, 3, 0x7f, 0xff, 4, 0,
    ];
    static BAD_LENGTH: [u8; 4] = [1, 1, 2, 0];
    static SECOND_ONLY: [u8; 26] = [
        1, 1, 6, 0, 0, 3, 0x7f, 1, 4, 0, 1, 1, 6, 0, 0, 1, 1, 1, 6, 0, 0, 2, 0x7f, 0xff, 4, 0,
    ];
    let parent_path = PARENT.as_ptr().cast_mut().cast::<DevicePathNode>();
    let child_path = CHILD.as_ptr().cast_mut().cast::<DevicePathNode>();
    let input_path = INPUT.as_ptr().cast_mut().cast::<DevicePathNode>();
    let mut parent = ptr::null_mut();
    let mut child = ptr::null_mut();
    unsafe {
        let install = ptr::addr_of!((*boot).install_protocol_interface).read_volatile();
        let uninstall = ptr::addr_of!((*boot).uninstall_protocol_interface).read_volatile();
        let locate = ptr::addr_of!((*boot).locate_device_path).read_volatile();
        if install(
            &mut parent,
            &DEVICE_PATH_PROTOCOL_GUID,
            0,
            parent_path.cast(),
        ) != Status::SUCCESS
            || install(&mut parent, &DIAGNOSTIC_PROTOCOL, 0, ptr::null_mut()) != Status::SUCCESS
            || install(&mut child, &DEVICE_PATH_PROTOCOL_GUID, 0, child_path.cast())
                != Status::SUCCESS
            || install(&mut child, &DIAGNOSTIC_PROTOCOL, 0, ptr::null_mut()) != Status::SUCCESS
        {
            stop(STOP_BOOT_SERVICES, 27);
        }

        let mut remaining = input_path;
        let mut found = ptr::null_mut();
        if locate(&DIAGNOSTIC_PROTOCOL, &mut remaining, &mut found) != Status::SUCCESS
            || found != child
            || remaining != input_path.byte_add(12)
        {
            stop(STOP_BOOT_SERVICES, 28);
        }
        remaining = child_path;
        if locate(&DIAGNOSTIC_PROTOCOL, &mut remaining, &mut found) != Status::SUCCESS
            || found != child
            || remaining != child_path.byte_add(12)
            || uninstall(child, &DIAGNOSTIC_PROTOCOL, ptr::null_mut()) != Status::SUCCESS
        {
            stop(STOP_BOOT_SERVICES, 29);
        }
        remaining = input_path;
        if locate(&DIAGNOSTIC_PROTOCOL, &mut remaining, &mut found) != Status::SUCCESS
            || found != parent
            || remaining != input_path.byte_add(6)
        {
            stop(STOP_BOOT_SERVICES, 29);
        }

        let no_match = SECOND_ONLY.as_ptr().cast_mut().cast();
        remaining = no_match;
        if locate(&DIAGNOSTIC_PROTOCOL, &mut remaining, ptr::null_mut()) != Status::NOT_FOUND
            || remaining != no_match
        {
            stop(STOP_BOOT_SERVICES, 31);
        }
        remaining = input_path;
        if locate(&DIAGNOSTIC_PROTOCOL, &mut remaining, ptr::null_mut())
            != Status::INVALID_PARAMETER
            || remaining != input_path
        {
            stop(STOP_BOOT_SERVICES, 31);
        }

        let malformed = BAD_LENGTH.as_ptr().cast_mut().cast();
        remaining = malformed;
        if locate(&DIAGNOSTIC_PROTOCOL, &mut remaining, &mut found) != Status::INVALID_PARAMETER
            || remaining != malformed
            || found != parent
            || uninstall(child, &DEVICE_PATH_PROTOCOL_GUID, child_path.cast()) != Status::SUCCESS
            || uninstall(parent, &DIAGNOSTIC_PROTOCOL, ptr::null_mut()) != Status::SUCCESS
            || uninstall(parent, &DEVICE_PATH_PROTOCOL_GUID, parent_path.cast()) != Status::SUCCESS
        {
            stop(STOP_BOOT_SERVICES, 30);
        }
    }
}

/// The BSP owns the live diagnostic table.
#[inline(never)]
unsafe fn verify_protocol_batches(boot: *const BootServices) {
    static FIRST_PATH: [u8; 20] = [
        1, 1, 6, 0, 0, 1, 0x7f, 1, 4, 0, 1, 1, 6, 0, 0, 2, 0x7f, 0xff, 4, 0,
    ];
    static OTHER_PATH: [u8; 20] = [
        1, 1, 6, 0, 0, 1, 0x7f, 1, 4, 0, 1, 1, 6, 0, 0, 3, 0x7f, 0xff, 4, 0,
    ];
    let duplicate_path = FIRST_PATH;
    let proposed = Guid {
        data1: DIAGNOSTIC_PROTOCOL.data1 + 2,
        ..DIAGNOSTIC_PROTOCOL
    };
    let first_guid = ptr::from_ref(&DIAGNOSTIC_PROTOCOL);
    let proposed_guid = ptr::from_ref(&proposed);
    let path_guid = ptr::from_ref(&DEVICE_PATH_PROTOCOL_GUID);
    let first_path = FIRST_PATH.as_ptr().cast::<c_void>();
    let other_path = OTHER_PATH.as_ptr().cast::<c_void>();
    let marker = ptr::null::<c_void>();
    let end = ptr::null::<Guid>();
    unsafe {
        let Some(install) =
            ptr::addr_of!((*boot).install_multiple_protocol_interfaces).read_volatile()
        else {
            stop(STOP_BOOT_SERVICES, 32);
        };
        let Some(uninstall) =
            ptr::addr_of!((*boot).uninstall_multiple_protocol_interfaces).read_volatile()
        else {
            stop(STOP_BOOT_SERVICES, 32);
        };
        let handle_protocol = ptr::addr_of!((*boot).handle_protocol).read_volatile();
        let mut handle = ptr::null_mut();
        if install(&mut handle, end) != Status::SUCCESS || !handle.is_null() {
            stop(STOP_BOOT_SERVICES, 32);
        }
        if install(&mut handle, first_guid, marker, path_guid, first_path, end) != Status::SUCCESS
            || handle.is_null()
        {
            stop(STOP_BOOT_SERVICES, 33);
        }
        let mut found = ptr::null_mut();
        if handle_protocol(handle, first_guid, &mut found) != Status::SUCCESS
            || !found.is_null()
            || handle_protocol(handle, path_guid, &mut found) != Status::SUCCESS
            || found.cast_const() != first_path
        {
            stop(STOP_BOOT_SERVICES, 33);
        }

        let installed = handle;
        if install(&mut handle, proposed_guid, marker, first_guid, marker, end)
            != Status::INVALID_PARAMETER
            || handle != installed
            || handle_protocol(handle, proposed_guid, &mut found) != Status::UNSUPPORTED
        {
            stop(STOP_BOOT_SERVICES, 34);
        }

        let mut other = ptr::null_mut();
        if install(
            &mut other,
            proposed_guid,
            marker,
            path_guid,
            duplicate_path.as_ptr().cast::<c_void>(),
            end,
        ) != Status::ALREADY_STARTED
            || !other.is_null()
            || install(&mut other, path_guid, other_path, end) != Status::SUCCESS
            || other.is_null()
            || other == handle
        {
            stop(STOP_BOOT_SERVICES, 35);
        }
        if uninstall(handle, first_guid, marker, proposed_guid, marker, end)
            != Status::INVALID_PARAMETER
            || handle_protocol(handle, first_guid, &mut found) != Status::SUCCESS
            || uninstall(handle, first_guid, marker, path_guid, first_path, end) != Status::SUCCESS
            || handle_protocol(handle, first_guid, &mut found) != Status::INVALID_PARAMETER
            || uninstall(other, path_guid, other_path, end) != Status::SUCCESS
        {
            stop(STOP_BOOT_SERVICES, 36);
        }
    }
}

struct EventProbe {
    boot: *const BootServices,
    target: Event,
    count: usize,
    trace: [u8; 8],
}

unsafe fn record_event(probe: *mut EventProbe, byte: u8) {
    unsafe {
        let count = ptr::addr_of!((*probe).count).read();
        if count == 8 {
            stop(STOP_BOOT_SERVICES, 38);
        }
        ptr::addr_of_mut!((*probe).trace)
            .cast::<u8>()
            .add(count)
            .write(byte);
        ptr::addr_of_mut!((*probe).count).write(count + 1);
    }
}

unsafe extern "efiapi" fn notify_high(_: Event, context: *mut c_void) {
    unsafe { record_event(context.cast(), b'H') };
}

unsafe extern "efiapi" fn notify_low(_: Event, context: *mut c_void) {
    let probe = context.cast::<EventProbe>();
    unsafe {
        record_event(probe, b'L');
        let boot = ptr::addr_of!((*probe).boot).read();
        let target = ptr::addr_of!((*probe).target).read();
        let signal = ptr::addr_of!((*boot).signal_event).read_volatile();
        if signal(target) != Status::SUCCESS {
            stop(STOP_BOOT_SERVICES, 38);
        }
        record_event(probe, b'l');
    }
}

unsafe extern "efiapi" fn notify_wait(event: Event, context: *mut c_void) {
    let probe = context.cast::<EventProbe>();
    unsafe {
        let boot = ptr::addr_of!((*probe).boot).read();
        let signal = ptr::addr_of!((*boot).signal_event).read_volatile();
        if signal(event) != Status::SUCCESS {
            stop(STOP_BOOT_SERVICES, 39);
        }
        record_event(probe, b'W');
    }
}

unsafe extern "efiapi" fn notify_group(event: Event, context: *mut c_void) {
    let probe = context.cast::<EventProbe>();
    unsafe {
        let boot = ptr::addr_of!((*probe).boot).read();
        let target = ptr::addr_of!((*probe).target).read();
        let check = ptr::addr_of!((*boot).check_event).read_volatile();
        let close = ptr::addr_of!((*boot).close_event).read_volatile();
        if check(target) != Status::SUCCESS || close(event) != Status::SUCCESS {
            stop(STOP_BOOT_SERVICES, 40);
        }
        record_event(probe, b'G');
    }
}

#[inline(never)]
fn verify_event_services(system: &SystemTable) {
    // SAFETY: the BSP owns the table and the context stays live through all
    // callbacks and closes. Nested callbacks only use raw context fields.
    unsafe {
        let boot = ptr::addr_of!(system.boot_services).read_volatile();
        let create = ptr::addr_of!((*boot).create_event).read_volatile();
        let create_ex = ptr::addr_of!((*boot).create_event_ex).read_volatile();
        let signal = ptr::addr_of!((*boot).signal_event).read_volatile();
        let check = ptr::addr_of!((*boot).check_event).read_volatile();
        let close = ptr::addr_of!((*boot).close_event).read_volatile();
        let raise = ptr::addr_of!((*boot).raise_tpl).read_volatile();
        let restore = ptr::addr_of!((*boot).restore_tpl).read_volatile();
        let mut probe = EventProbe {
            boot,
            target: ptr::null_mut(),
            count: 0,
            trace: [0; 8],
        };
        let context = ptr::addr_of_mut!(probe).cast::<c_void>();
        let mut low = ptr::null_mut();
        let mut high = ptr::null_mut();
        let mut wait = ptr::null_mut();
        if create(
            EVT_NOTIFY_SIGNAL,
            TPL_CALLBACK,
            Some(notify_low),
            context,
            &mut low,
        ) != Status::SUCCESS
            || create(
                EVT_NOTIFY_SIGNAL,
                TPL_NOTIFY,
                Some(notify_high),
                context,
                &mut high,
            ) != Status::SUCCESS
            || create(
                EVT_NOTIFY_WAIT,
                TPL_CALLBACK,
                Some(notify_wait),
                context,
                &mut wait,
            ) != Status::SUCCESS
            || check(high) != Status::INVALID_PARAMETER
        {
            stop(STOP_BOOT_SERVICES, 37);
        }
        ptr::addr_of_mut!(probe.target).write(high);
        let old = raise(TPL_NOTIFY);
        if old != TPL_APPLICATION
            || signal(low) != Status::SUCCESS
            || signal(low) != Status::SUCCESS
            || ptr::addr_of!(probe.count).read() != 0
        {
            stop(STOP_BOOT_SERVICES, 38);
        }
        restore(old);
        if ptr::addr_of!(probe.count).read() != 3 {
            stop(STOP_BOOT_SERVICES, 38);
        }
        if check(wait) != Status::SUCCESS || ptr::addr_of!(probe.count).read() != 4 {
            stop(STOP_BOOT_SERVICES, 39);
        }

        let mut plain = ptr::null_mut();
        let mut grouped = ptr::null_mut();
        let group = Guid {
            data1: DIAGNOSTIC_PROTOCOL.data1 + 3,
            ..DIAGNOSTIC_PROTOCOL
        };
        if create_ex(0, 0, None, ptr::null(), &group, &mut plain) != Status::SUCCESS
            || create_ex(
                EVT_NOTIFY_SIGNAL,
                TPL_CALLBACK,
                Some(notify_group),
                context,
                &group,
                &mut grouped,
            ) != Status::SUCCESS
        {
            stop(STOP_BOOT_SERVICES, 40);
        }
        ptr::addr_of_mut!(probe.target).write(plain);
        if signal(plain) != Status::SUCCESS
            || ptr::addr_of!(probe.count).read() != 5
            || ptr::addr_of!(probe.trace).read() != *b"LHlWG\0\0\0"
            || close(grouped) != Status::INVALID_PARAMETER
            || close(plain) != Status::SUCCESS
            || close(wait) != Status::SUCCESS
            || close(high) != Status::SUCCESS
            || close(low) != Status::SUCCESS
        {
            stop(STOP_BOOT_SERVICES, 40);
        }
    }
    com1_write(b"OD-FW S7-05 event dispatch verified\r\n");
}

struct ProtocolNotifyProbe {
    boot: *const BootServices,
    registration: *mut c_void,
    handle: Handle,
    expected_interface: *mut c_void,
    batch_marker: *const Guid,
    calls: usize,
}

unsafe extern "efiapi" fn notify_protocol(_: Event, context: *mut c_void) {
    let probe = context.cast::<ProtocolNotifyProbe>();
    unsafe {
        let boot = ptr::addr_of!((*probe).boot).read();
        let registration = ptr::addr_of!((*probe).registration).read();
        let handle = ptr::addr_of!((*probe).handle).read();
        let expected = ptr::addr_of!((*probe).expected_interface).read();
        let marker = ptr::addr_of!((*probe).batch_marker).read();
        let locate = ptr::addr_of!((*boot).locate_protocol).read_volatile();
        let handle_protocol = ptr::addr_of!((*boot).handle_protocol).read_volatile();
        let mut found = ptr::null_mut();
        if locate(&DIAGNOSTIC_PROTOCOL, registration, &mut found) != Status::SUCCESS
            || found != expected
            || handle_protocol(handle, &DIAGNOSTIC_PROTOCOL, &mut found) != Status::SUCCESS
            || found != expected
            || (!marker.is_null()
                && (handle_protocol(handle, marker, &mut found) != Status::SUCCESS
                    || !found.is_null()))
        {
            stop(STOP_BOOT_SERVICES, 43);
        }
        let calls = ptr::addr_of!((*probe).calls).read();
        ptr::addr_of_mut!((*probe).calls).write(calls + 1);
    }
}

#[inline(never)]
fn verify_protocol_notify(system: &SystemTable) {
    // SAFETY: all input and output storage lives through the callbacks.
    // The callback only accesses the probe through raw pointers.
    unsafe {
        let boot = ptr::addr_of!(system.boot_services).read_volatile();
        let register = ptr::addr_of!((*boot).register_protocol_notify).read_volatile();
        let install = ptr::addr_of!((*boot).install_protocol_interface).read_volatile();
        let reinstall = ptr::addr_of!((*boot).reinstall_protocol_interface).read_volatile();
        let uninstall = ptr::addr_of!((*boot).uninstall_protocol_interface).read_volatile();
        let Some(install_multiple) =
            ptr::addr_of!((*boot).install_multiple_protocol_interfaces).read_volatile()
        else {
            stop(STOP_BOOT_SERVICES, 41);
        };
        let locate_handle = ptr::addr_of!((*boot).locate_handle).read_volatile();
        let locate_buffer = ptr::addr_of!((*boot).locate_handle_buffer).read_volatile();
        let locate = ptr::addr_of!((*boot).locate_protocol).read_volatile();
        let free = ptr::addr_of!((*boot).free_pool).read_volatile();
        let create = ptr::addr_of!((*boot).create_event).read_volatile();
        let close = ptr::addr_of!((*boot).close_event).read_volatile();

        let mut probe = ProtocolNotifyProbe {
            boot,
            registration: ptr::null_mut(),
            handle: ptr::null_mut(),
            expected_interface: ptr::null_mut(),
            batch_marker: ptr::null(),
            calls: 0,
        };
        let mut existing = ptr::null_mut();
        let mut event = ptr::null_mut();
        let mut second_key = ptr::null_mut();
        let context = ptr::addr_of_mut!(probe).cast();
        if install(&mut existing, &DIAGNOSTIC_PROTOCOL, 0, ptr::null_mut()) != Status::SUCCESS
            || create(
                EVT_NOTIFY_SIGNAL,
                TPL_CALLBACK,
                Some(notify_protocol),
                context,
                &mut event,
            ) != Status::SUCCESS
            || register(
                &DIAGNOSTIC_PROTOCOL,
                event,
                ptr::addr_of_mut!(probe.registration),
            ) != Status::SUCCESS
            || register(&DIAGNOSTIC_PROTOCOL, event, &mut second_key) != Status::SUCCESS
            || ptr::addr_of!(probe.calls).read() != 0
        {
            stop(STOP_BOOT_SERVICES, 41);
        }

        let key = ptr::addr_of!(probe.registration).read();
        let mut bytes = 0;
        let mut found = ptr::null_mut();
        let mut count = 0;
        let mut buffer = ptr::null_mut();
        if locate_handle(
            LOCATE_BY_REGISTER_NOTIFY,
            ptr::null(),
            key,
            &mut bytes,
            ptr::null_mut(),
        ) != Status::BUFFER_TOO_SMALL
            || bytes != size_of::<Handle>()
            || locate_handle(
                LOCATE_BY_REGISTER_NOTIFY,
                ptr::null(),
                key,
                &mut bytes,
                &mut found,
            ) != Status::SUCCESS
            || found != existing
            || locate_buffer(
                LOCATE_BY_REGISTER_NOTIFY,
                ptr::null(),
                second_key,
                &mut count,
                &mut buffer,
            ) != Status::SUCCESS
            || count != 1
            || buffer.is_null()
            || buffer.read() != existing
            || free(buffer.cast()) != Status::SUCCESS
        {
            stop(STOP_BOOT_SERVICES, 42);
        }

        let mut interface = 17_u32;
        let mut replacement = 42_u32;
        let original = ptr::addr_of_mut!(interface).cast();
        let replacement = ptr::addr_of_mut!(replacement).cast();
        let handle = ptr::addr_of_mut!(probe.handle);
        ptr::addr_of_mut!(probe.expected_interface).write(original);
        if install(handle, &DIAGNOSTIC_PROTOCOL, 0, original) != Status::SUCCESS
            || ptr::addr_of!(probe.calls).read() != 1
        {
            stop(STOP_BOOT_SERVICES, 43);
        }
        ptr::addr_of_mut!(probe.expected_interface).write(replacement);
        let installed = handle.read();
        if reinstall(installed, &DIAGNOSTIC_PROTOCOL, original, replacement) != Status::SUCCESS
            || ptr::addr_of!(probe.calls).read() != 2
        {
            stop(STOP_BOOT_SERVICES, 43);
        }

        let batch_marker = Guid {
            data1: DIAGNOSTIC_PROTOCOL.data1 + 4,
            ..DIAGNOSTIC_PROTOCOL
        };
        let guid = &DIAGNOSTIC_PROTOCOL as *const Guid;
        let marker = &batch_marker as *const Guid;
        handle.write(ptr::null_mut());
        ptr::addr_of_mut!(probe.expected_interface).write(original);
        ptr::addr_of_mut!(probe.batch_marker).write(marker);
        if install_multiple(
            handle,
            guid,
            original,
            guid,
            replacement,
            ptr::null::<Guid>(),
        ) != Status::INVALID_PARAMETER
            || !handle.read().is_null()
            || ptr::addr_of!(probe.calls).read() != 2
            || install_multiple(
                handle,
                guid,
                original,
                marker,
                ptr::null_mut::<c_void>(),
                ptr::null::<Guid>(),
            ) != Status::SUCCESS
            || ptr::addr_of!(probe.calls).read() != 3
        {
            stop(STOP_BOOT_SERVICES, 44);
        }
        let batch = handle.read();
        let mut unused = replacement;
        if locate(guid, key, &mut unused) != Status::NOT_FOUND
            || !unused.is_null()
            || close(event) != Status::SUCCESS
            || locate(guid, key, &mut unused) != Status::INVALID_PARAMETER
            || locate(guid, second_key, &mut unused) != Status::INVALID_PARAMETER
            || uninstall(batch, marker, ptr::null_mut()) != Status::SUCCESS
            || uninstall(batch, guid, original) != Status::SUCCESS
            || uninstall(installed, guid, replacement) != Status::SUCCESS
            || uninstall(existing, guid, ptr::null_mut()) != Status::SUCCESS
        {
            stop(STOP_BOOT_SERVICES, 44);
        }
    }
    com1_write(b"OD-FW S7-05 protocol notifications verified\r\n");
}

fn read_tsc() -> u64 {
    let low: u32;
    let high: u32;
    // SAFETY: the BSP runs in CPL0. The instruction touches only EDX:EAX.
    unsafe {
        asm!("rdtsc", out("eax") low, out("edx") high, options(nomem, nostack, preserves_flags));
    }
    (u64::from(high) << 32) | u64::from(low)
}

fn request_reset(port: u16, value: u8) -> ! {
    // SAFETY: this port and value were decoded from validated fixed hardware.
    unsafe {
        asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags))
    };
    halt_forever()
}

fn reset_for_watchdog(port: u16, value: u8, code: u64) -> ! {
    com1_write(b"OD-FW S7-05 watchdog expired code=0x");
    com1_write(&fixed_hex(code));
    com1_write(b"\r\n");
    request_reset(port, value)
}

unsafe extern "efiapi" fn diagnostic_reset_system(
    reset_type: ResetType,
    status: Status,
    _data_size: usize,
    _reset_data: *const c_void,
) {
    let digit = if reset_type <= 3 {
        b'0' + reset_type as u8
    } else {
        b'?'
    };
    com1_write(b"OD-FW RESET type=");
    com1_write(&[digit]);
    com1_write(b" status=0x");
    com1_write(&fixed_hex(status.raw() as u64));
    com1_write(b"\r\n");

    if matches!(reset_type, RESET_COLD | RESET_WARM)
        && let Ok(handoff) = Handoff::parse_for_boot(boot_handoff_bytes(), HANDOFF_MINOR_MAX)
        && let Ok(Some(pm)) = handoff.acpi_fixed_hardware()
    {
        request_reset(pm.reset_reg, pm.reset_value);
    }
    halt_forever()
}

#[inline(never)]
fn verify_service_boundaries(system: &SystemTable) {
    // SAFETY: the private tables are live before exit. All provided buffers
    // and output variables meet their service signatures.
    unsafe {
        let boot = ptr::addr_of!(system.boot_services).read_volatile();
        let runtime = ptr::addr_of!(system.runtime_services).read_volatile();
        let install = ptr::addr_of!((*boot).install_protocol_interface).read_volatile();
        let uninstall = ptr::addr_of!((*boot).uninstall_protocol_interface).read_volatile();
        let load = ptr::addr_of!((*boot).load_image).read_volatile();
        let start = ptr::addr_of!((*boot).start_image).read_volatile();
        let exit = ptr::addr_of!((*boot).exit).read_volatile();
        let unload = ptr::addr_of!((*boot).unload_image).read_volatile();
        let count = ptr::addr_of!((*boot).get_next_monotonic_count).read_volatile();
        let connect = ptr::addr_of!((*boot).connect_controller).read_volatile();
        let disconnect = ptr::addr_of!((*boot).disconnect_controller).read_volatile();

        let mut handle = ptr::null_mut();
        if install(&mut handle, &DIAGNOSTIC_PROTOCOL, 0, ptr::null_mut()) != Status::SUCCESS {
            stop(STOP_BOOT_SERVICES, 64);
        }
        let mut image = handle;
        let mut exit_size = 43;
        let mut exit_data = ptr::null_mut();
        let mut monotonic = 43;
        if load(0, ptr::null_mut(), ptr::null(), ptr::null(), 0, &mut image)
            != Status::INVALID_PARAMETER
            || load(0, handle, ptr::null(), ptr::null(), 0, &mut image) != Status::NOT_FOUND
            || load(0, handle, ptr::null(), b"MZ".as_ptr().cast(), 2, &mut image)
                != Status::LOAD_ERROR
            || image != handle
            || start(handle, &mut exit_size, &mut exit_data) != Status::INVALID_PARAMETER
            || exit_size != 43
            || !exit_data.is_null()
            || exit(handle, Status::SUCCESS, 0, ptr::null()) != Status::INVALID_PARAMETER
            || unload(handle) != Status::INVALID_PARAMETER
            || count(ptr::null_mut()) != Status::INVALID_PARAMETER
            || count(&mut monotonic) != Status::DEVICE_ERROR
            || monotonic != 43
            || connect(handle, ptr::null(), ptr::null(), 0) != Status::NOT_FOUND
            || disconnect(handle, ptr::null_mut(), ptr::null_mut()) != Status::SUCCESS
            || disconnect(handle, handle, ptr::null_mut()) != Status::INVALID_PARAMETER
            || uninstall(handle, &DIAGNOSTIC_PROTOCOL, ptr::null_mut()) != Status::SUCCESS
        {
            stop(STOP_BOOT_SERVICES, 65);
        }

        let get_variable = ptr::addr_of!((*runtime).get_variable).read_volatile();
        let name = [0x41u16, 0];
        let mut attributes = 43;
        let mut size = 1;
        let mut data = 0xa5u8;
        if get_variable(
            name.as_ptr(),
            &DIAGNOSTIC_PROTOCOL,
            &mut attributes,
            &mut size,
            ptr::addr_of_mut!(data).cast(),
        ) != Status::NOT_FOUND
            || attributes != 43
            || size != 1
            || data != 0xa5
        {
            stop(STOP_BOOT_SERVICES, 66);
        }
    }
    com1_write(b"OD-FW S7-05 service ABI boundaries verified\r\n");
}

#[inline(never)]
fn boot_fv_image(system: &SystemTable, requested: Option<FirmwareFvApp>) {
    // Console initialization and failed applications can leave an open COM1 line.
    fn fail(detail: u64) -> ! {
        com1_write(b"\r\n");
        stop(STOP_BOOT_SERVICES, detail);
    }

    let Ok(archive) = od_firmware::fv::embedded() else {
        fail(69);
    };
    if requested.is_none() && archive.get("hello").is_none() {
        return;
    }
    let name = match requested {
        Some(FirmwareFvApp::Ebs) => "ebs",
        Some(FirmwareFvApp::Vars) => "vars",
        _ => "hello",
    };
    let mut path_buffer = [0; 64];
    let Some(path) = od_firmware::bs::image::firmware_file_device_path(name, &mut path_buffer)
    else {
        stop(STOP_INTERNAL, 70);
    };
    // SAFETY: the table is active and the encoded path and scalar buffers
    // remain live through their Boot Services calls.
    unsafe {
        let boot = ptr::addr_of!(system.boot_services).read_volatile();
        let install = ptr::addr_of!((*boot).install_protocol_interface).read_volatile();
        let uninstall = ptr::addr_of!((*boot).uninstall_protocol_interface).read_volatile();
        let load = ptr::addr_of!((*boot).load_image).read_volatile();
        let start = ptr::addr_of!((*boot).start_image).read_volatile();
        let mut parent = ptr::null_mut();
        if install(&mut parent, &DIAGNOSTIC_PROTOCOL, 0, ptr::null_mut()) != Status::SUCCESS {
            fail(71);
        }
        let mut image = ptr::null_mut();
        match load(0, parent, path.as_ptr().cast(), ptr::null(), 0, &mut image) {
            Status::SUCCESS => {
                let status = start(image, ptr::null_mut(), ptr::null_mut());
                if requested == Some(FirmwareFvApp::Ebs) {
                    fail(75);
                }
                if status != Status::SUCCESS {
                    fail(72);
                }
                if requested == Some(FirmwareFvApp::Vars) {
                    com1_write(b"OD-FW image vars: EFI_SUCCESS\r\n");
                } else {
                    com1_write(b"OD-FW image hello: EFI_SUCCESS\r\n");
                }
            }
            Status::LOAD_ERROR if requested.is_none() && image.is_null() => {
                com1_write(b"OD-FW image hello: EFI_LOAD_ERROR\r\n");
            }
            status => fail(status.raw() as u64),
        }
        if uninstall(parent, &DIAGNOSTIC_PROTOCOL, ptr::null_mut()) != Status::SUCCESS {
            fail(73);
        }
    }
    if requested.is_some() {
        stop(STOP_NO_BOOT_OPTIONS, 0);
    }
}

#[inline(never)]
fn verify_watchdog_services(system: &SystemTable, has_reset: bool) {
    const CODE: u64 = 0x10042;
    // SAFETY: the BSP uses the live service table and empty optional data.
    unsafe {
        let boot = ptr::addr_of!(system.boot_services).read_volatile();
        let set = ptr::addr_of!((*boot).set_watchdog_timer).read_volatile();
        let expected = if has_reset {
            Status::SUCCESS
        } else {
            Status::UNSUPPORTED
        };
        if set(0, 0, 0, ptr::null()) != Status::SUCCESS
            || set(1, 0xffff, 0, ptr::null()) != Status::INVALID_PARAMETER
            || set(30, CODE, 0, ptr::null()) != expected
            || set(0, 0, 0, ptr::null()) != Status::SUCCESS
            || set(1, CODE, 0, ptr::null()) != expected
        {
            stop(STOP_BOOT_SERVICES, 59);
        }
        if has_reset {
            let raise = ptr::addr_of!((*boot).raise_tpl).read_volatile();
            let restore = ptr::addr_of!((*boot).restore_tpl).read_volatile();
            let stall = ptr::addr_of!((*boot).stall).read_volatile();
            let old = raise(TPL_HIGH_LEVEL);
            com1_write(b"OD-FW S7-05 watchdog armed\r\n");
            if stall(1) != Status::SUCCESS {
                stop(STOP_BOOT_SERVICES, 59);
            }
            restore(old);
        }
    }
    com1_write(b"OD-FW S7-05 watchdog service verified\r\n");
}

unsafe extern "efiapi" fn count_timer_notifications(_: Event, context: *mut c_void) {
    let count = context.cast::<usize>();
    // SAFETY: the diagnostic keeps one aligned counter alive through CloseEvent.
    unsafe { count.write(count.read() + 1) };
}

#[inline(never)]
fn verify_timer_services(system: &SystemTable, tsc_hz: u64) {
    // SAFETY: event arrays, outputs and callback context stay alive through the
    // service calls and are separate from firmware-private service state.
    unsafe {
        let boot = ptr::addr_of!(system.boot_services).read_volatile();
        let create = ptr::addr_of!((*boot).create_event).read_volatile();
        let close = ptr::addr_of!((*boot).close_event).read_volatile();
        let set_timer = ptr::addr_of!((*boot).set_timer).read_volatile();
        let wait = ptr::addr_of!((*boot).wait_for_event).read_volatile();
        let check = ptr::addr_of!((*boot).check_event).read_volatile();
        let stall = ptr::addr_of!((*boot).stall).read_volatile();
        let raise = ptr::addr_of!((*boot).raise_tpl).read_volatile();
        let restore = ptr::addr_of!((*boot).restore_tpl).read_volatile();

        let mut idle = ptr::null_mut();
        let mut relative = ptr::null_mut();
        let mut periodic = ptr::null_mut();
        let mut notifications = 0usize;
        let counter = ptr::addr_of_mut!(notifications);
        if create(0, 0, None, ptr::null_mut(), &mut idle) != Status::SUCCESS
            || create(EVT_TIMER, 0, None, ptr::null_mut(), &mut relative) != Status::SUCCESS
            || create(
                EVT_TIMER | EVT_NOTIFY_SIGNAL,
                TPL_CALLBACK,
                Some(count_timer_notifications),
                counter.cast(),
                &mut periodic,
            ) != Status::SUCCESS
            || set_timer(idle, TIMER_CANCEL, 0) != Status::INVALID_PARAMETER
        {
            stop(STOP_BOOT_SERVICES, 45);
        }

        let mut actual = usize::MAX;
        let events = [idle, relative];
        if set_timer(relative, TIMER_RELATIVE, 200) != Status::SUCCESS
            || wait(events.len(), events.as_ptr(), &mut actual) != Status::SUCCESS
            || actual != 1
            || check(relative) != Status::NOT_READY
        {
            stop(STOP_BOOT_SERVICES, 46);
        }
        let rejected = [idle, periodic];
        if wait(rejected.len(), rejected.as_ptr(), &mut actual) != Status::INVALID_PARAMETER
            || actual != 1
        {
            stop(STOP_BOOT_SERVICES, 46);
        }

        let old_tpl = raise(TPL_NOTIFY);
        if old_tpl != TPL_APPLICATION
            || wait(events.len(), events.as_ptr(), &mut actual) != Status::UNSUPPORTED
            || set_timer(periodic, TIMER_PERIODIC, 50) != Status::SUCCESS
        {
            stop(STOP_BOOT_SERVICES, 47);
        }
        let started = read_tsc();
        if stall(25) != Status::SUCCESS
            || u128::from(read_tsc() - started) < (25 * u128::from(tsc_hz)).div_ceil(1_000_000)
            || counter.read() != 0
            || set_timer(periodic, TIMER_CANCEL, u64::MAX) != Status::SUCCESS
        {
            stop(STOP_BOOT_SERVICES, 47);
        }
        restore(old_tpl);
        if counter.read() != 1
            || stall(10) != Status::SUCCESS
            || counter.read() != 1
            || close(periodic) != Status::SUCCESS
            || close(relative) != Status::SUCCESS
            || close(idle) != Status::SUCCESS
        {
            stop(STOP_BOOT_SERVICES, 48);
        }
    }
    com1_write(b"OD-FW S7-05 boot timers and waits verified\r\n");
}

const DIAGNOSTIC_CONFIGURATION: Guid = Guid {
    data1: DIAGNOSTIC_PROTOCOL.data1 + 5,
    ..DIAGNOSTIC_PROTOCOL
};
const DIAGNOSTIC_CONFIGURATION_SECOND: Guid = Guid {
    data1: DIAGNOSTIC_PROTOCOL.data1 + 6,
    ..DIAGNOSTIC_PROTOCOL
};
static DIAGNOSTIC_CONFIGURATION_PAYLOAD: u64 = 0x4f44_5f43_4f4e_4647;

struct ConfigurationProbe {
    system: *mut SystemTable,
    expected_table: *mut c_void,
    expected_count: usize,
    calls: usize,
}

unsafe extern "efiapi" fn notify_configuration_change(_: Event, context: *mut c_void) {
    let probe = context.cast::<ConfigurationProbe>();
    unsafe {
        let system = ptr::addr_of!((*probe).system).read();
        let expected = ptr::addr_of!((*probe).expected_table).read();
        let expected_count = ptr::addr_of!((*probe).expected_count).read();
        let (entries, count) = (&*system).configuration_tables();
        if entries.is_null() || count != expected_count {
            stop(STOP_BOOT_SERVICES, 50);
        }
        let first = entries.read();
        if expected.is_null() {
            if first.vendor_guid != DIAGNOSTIC_CONFIGURATION_SECOND {
                stop(STOP_BOOT_SERVICES, 50);
            }
        } else if first.vendor_guid != DIAGNOSTIC_CONFIGURATION || first.vendor_table != expected {
            stop(STOP_BOOT_SERVICES, 50);
        }
        let calls = ptr::addr_of!((*probe).calls).read();
        ptr::addr_of_mut!((*probe).calls).write(calls + 1);
    }
}

#[inline(never)]
fn verify_configuration_services(system: *mut SystemTable) {
    // SAFETY: the table lives on the boot stack. Short borrows end before
    // calling services that mutate it. The probe stays live through CloseEvent.
    unsafe {
        let boot = ptr::addr_of!((*system).boot_services).read_volatile();
        let create = ptr::addr_of!((*boot).create_event_ex).read_volatile();
        let close = ptr::addr_of!((*boot).close_event).read_volatile();
        let install = ptr::addr_of!((*boot).install_configuration_table).read_volatile();
        let get_map = ptr::addr_of!((*boot).get_memory_map).read_volatile();
        let current_crc = || ptr::addr_of!((*system).header.crc32).read();
        let published = || (&*system).configuration_tables();
        let mut snapshot = [MemoryDescriptor::EMPTY; BOOT_DESCRIPTOR_CAPACITY];
        let mut read_map = || {
            let mut bytes = size_of_val(&snapshot);
            let mut key = 0;
            if get_map(
                &mut bytes,
                snapshot.as_mut_ptr(),
                &mut key,
                ptr::null_mut(),
                ptr::null_mut(),
            ) != Status::SUCCESS
            {
                stop(STOP_BOOT_SERVICES, 49);
            }
            (key, bytes / size_of::<MemoryDescriptor>())
        };
        let (initial_key, _) = read_map();
        let initial_crc = current_crc();
        let mut first_value = 17_u64;
        let mut new_value = 42_u64;
        let first = ptr::addr_of_mut!(first_value).cast();
        let replacement = ptr::addr_of_mut!(new_value).cast();
        let persistent = ptr::addr_of!(DIAGNOSTIC_CONFIGURATION_PAYLOAD)
            .cast_mut()
            .cast();
        let mut event = ptr::null_mut();
        let mut probe = ConfigurationProbe {
            system,
            expected_table: first,
            expected_count: 1,
            calls: 0,
        };
        if install(ptr::null(), first) != Status::INVALID_PARAMETER
            || install(&DIAGNOSTIC_CONFIGURATION, ptr::null_mut()) != Status::NOT_FOUND
            || create(
                EVT_NOTIFY_SIGNAL,
                TPL_CALLBACK,
                Some(notify_configuration_change),
                ptr::addr_of_mut!(probe).cast(),
                &DIAGNOSTIC_CONFIGURATION,
                &mut event,
            ) != Status::SUCCESS
            || install(&DIAGNOSTIC_CONFIGURATION, first) != Status::SUCCESS
            || ptr::addr_of!(probe.calls).read() != 1
        {
            stop(STOP_BOOT_SERVICES, 49);
        }
        let (storage, count) = published();
        let (runtime_key, _) = read_map();
        if count != 1
            || storage.is_null()
            || initial_key.checked_add(1) != Some(runtime_key)
            || install(&DIAGNOSTIC_CONFIGURATION_SECOND, persistent) != Status::SUCCESS
            || ptr::addr_of!(probe.calls).read() != 1
        {
            stop(STOP_BOOT_SERVICES, 50);
        }
        let two_crc = current_crc();
        ptr::addr_of_mut!(probe.expected_count).write(2);
        ptr::addr_of_mut!(probe.expected_table).write(replacement);
        if install(&DIAGNOSTIC_CONFIGURATION, replacement) != Status::SUCCESS
            || ptr::addr_of!(probe.calls).read() != 2
            || published() != (storage, 2)
            || current_crc() != two_crc
        {
            stop(STOP_BOOT_SERVICES, 50);
        }

        ptr::addr_of_mut!(probe.expected_count).write(1);
        ptr::addr_of_mut!(probe.expected_table).write(ptr::null_mut());
        if install(&DIAGNOSTIC_CONFIGURATION, ptr::null_mut()) != Status::SUCCESS
            || ptr::addr_of!(probe.calls).read() != 3
            || install(&DIAGNOSTIC_CONFIGURATION_SECOND, ptr::null_mut()) != Status::SUCCESS
            || published() != (ptr::null_mut(), 0)
            || current_crc() != initial_crc
            || install(&DIAGNOSTIC_CONFIGURATION, ptr::null_mut()) != Status::NOT_FOUND
            || ptr::addr_of!(probe.calls).read() != 3
            || close(event) != Status::SUCCESS
        {
            stop(STOP_BOOT_SERVICES, 51);
        }
        if install(&DIAGNOSTIC_CONFIGURATION_SECOND, persistent) != Status::SUCCESS
            || published() != (storage, 1)
            || storage.read().vendor_table != persistent
        {
            stop(STOP_BOOT_SERVICES, 52);
        }

        let (final_key, descriptors) = read_map();
        let address = storage.addr() as u64;
        if final_key != runtime_key
            || !snapshot[..descriptors].iter().any(|descriptor| {
                descriptor.memory_type == MemoryType::RUNTIME_SERVICES_DATA.raw()
                    && descriptor.attribute & (1 << 63) != 0
                    && descriptor.physical_start <= address
                    && address < descriptor.physical_start + descriptor.number_of_pages * PAGE_BYTES
            })
        {
            stop(STOP_BOOT_SERVICES, 52);
        }
    }
    com1_write(b"OD-FW S7-05 configuration tables verified\r\n");
}

struct ExitProbe {
    system: *mut SystemTable,
    boot: *mut BootServices,
    tsc_hz: u64,
    trace: [u8; 4],
    count: usize,
}
const EXIT_VAR_NAME: [u16; 7] = [0x4f, 0x44, 0x45, 0x78, 0x69, 0x74, 0];
const EXIT_VAR_VALUE: [u8; 1] = [0x5a];

unsafe fn record_exit_notification(probe: *mut ExitProbe, value: u8) {
    unsafe {
        let system = ptr::addr_of!((*probe).system).read();
        let boot = ptr::addr_of!((*probe).boot).read();
        let count = ptr::addr_of!((*probe).count).read();
        if count >= 3 || ptr::addr_of!((*system).boot_services).read() != boot {
            stop(STOP_BOOT_SERVICES, 54);
        }
        ptr::addr_of_mut!((*probe).trace)
            .cast::<u8>()
            .add(count)
            .write(value);
        ptr::addr_of_mut!((*probe).count).write(count + 1);
    }
}

unsafe fn verify_runtime_events(boot: *mut BootServices, runtime_context: *mut c_void) {
    // SAFETY: this runs in the private BEFORE notification, with the table
    // and output arrays live and no mutable references across service calls.
    unsafe {
        let allocate = ptr::addr_of!((*boot).allocate_pages).read_volatile();
        let get_map = ptr::addr_of!((*boot).get_memory_map).read_volatile();
        let create = ptr::addr_of!((*boot).create_event).read_volatile();
        let create_ex = ptr::addr_of!((*boot).create_event_ex).read_volatile();
        let close = ptr::addr_of!((*boot).close_event).read_volatile();
        let set_timer = ptr::addr_of!((*boot).set_timer).read_volatile();
        let mut callback_page = 0;
        if allocate(0, MemoryType::RUNTIME_SERVICES_CODE, 1, &mut callback_page) != Status::SUCCESS
            || callback_page == 0
        {
            stop(STOP_BOOT_SERVICES, 61);
        }
        let callback_byte = ptr::with_exposed_provenance_mut::<u8>(callback_page as usize);
        // Install the x64 RET instruction for a no-op EFI callback. This
        // diagnostic checks retention of the registration and never calls it.
        callback_byte.write(0xc3);
        let notify = core::mem::transmute::<*mut u8, EventNotify>(callback_byte);

        let mut descriptors = [MemoryDescriptor::EMPTY; BOOT_DESCRIPTOR_CAPACITY];
        let mut read_key = || {
            let mut size = size_of_val(&descriptors);
            let mut key = 0;
            if get_map(
                &mut size,
                descriptors.as_mut_ptr(),
                &mut key,
                ptr::null_mut(),
                ptr::null_mut(),
            ) != Status::SUCCESS
            {
                stop(STOP_BOOT_SERVICES, 61);
            }
            key
        };
        let before = read_key();
        let mut legacy = ptr::null_mut();
        let mut extended = ptr::null_mut();
        let mut timer = ptr::null_mut();
        let mut transient = ptr::null_mut();
        if create_ex(
            EVT_SIGNAL_VIRTUAL_ADDRESS_CHANGE,
            TPL_NOTIFY,
            Some(notify),
            runtime_context,
            &VIRTUAL_ADDRESS_CHANGE,
            &mut legacy,
        ) != Status::INVALID_PARAMETER
            || !legacy.is_null()
            || read_key() != before
            || create(
                EVT_SIGNAL_VIRTUAL_ADDRESS_CHANGE,
                TPL_NOTIFY,
                Some(notify),
                runtime_context,
                &mut legacy,
            ) != Status::SUCCESS
            || legacy.is_null()
        {
            stop(STOP_BOOT_SERVICES, 62);
        }
        let retained = read_key();
        if before.checked_add(1) != Some(retained)
            || create_ex(
                EVT_NOTIFY_SIGNAL,
                TPL_NOTIFY,
                Some(notify),
                ptr::null(),
                &VIRTUAL_ADDRESS_CHANGE,
                &mut extended,
            ) != Status::SUCCESS
            || extended.is_null()
            || create(
                EVT_RUNTIME | EVT_TIMER,
                0,
                None,
                ptr::null_mut(),
                &mut timer,
            ) != Status::SUCCESS
            || set_timer(timer, TIMER_RELATIVE, 10_000) != Status::SUCCESS
            || create(
                EVT_RUNTIME | EVT_NOTIFY_SIGNAL,
                TPL_CALLBACK,
                Some(notify),
                ptr::null_mut(),
                &mut transient,
            ) != Status::SUCCESS
            || close(transient) != Status::SUCCESS
            || read_key() != retained
        {
            stop(STOP_BOOT_SERVICES, 63);
        }
    }
    com1_write(b"OD-FW S7-05 runtime events registered\r\n");
}

unsafe extern "efiapi" fn notify_before_exit(_: Event, context: *mut c_void) {
    let probe = context.cast::<ExitProbe>();
    unsafe {
        let boot = ptr::addr_of!((*probe).boot).read();
        let allocate = ptr::addr_of!((*boot).allocate_pool).read_volatile();
        let mut allocation = ptr::null_mut();
        // The BEFORE callback owns this LoaderData allocation. Its new
        // descriptor makes the map key used by the first attempt stale.
        if allocate(MemoryType::LOADER_DATA, 8, &mut allocation) != Status::SUCCESS
            || allocation.is_null()
        {
            stop(STOP_BOOT_SERVICES, 54);
        }
        let system = ptr::addr_of!((*probe).system).read();
        verify_runtime_events(
            boot,
            ptr::addr_of!((*system).runtime_services).read().cast(),
        );
        record_exit_notification(probe, b'B');
    }
}

unsafe extern "efiapi" fn notify_modern_exit(_: Event, context: *mut c_void) {
    let probe = context.cast::<ExitProbe>();
    unsafe {
        let boot = ptr::addr_of!((*probe).boot).read();
        let hz = u128::from(ptr::addr_of!((*probe).tsc_hz).read());
        let stall = ptr::addr_of!((*boot).stall).read_volatile();
        let started = read_tsc();
        if stall(200) != Status::SUCCESS
            || u128::from(read_tsc() - started) < (200 * hz).div_ceil(1_000_000)
        {
            stop(STOP_BOOT_SERVICES, 54);
        }
        let system = ptr::addr_of!((*probe).system).read();
        let runtime = ptr::addr_of!((*system).runtime_services).read();
        let set = ptr::addr_of!((*runtime).set_variable).read_volatile();
        if set(
            EXIT_VAR_NAME.as_ptr(),
            &DIAGNOSTIC_PROTOCOL,
            od_firmware::var::services::NON_VOLATILE_RUNTIME,
            EXIT_VAR_VALUE.len(),
            EXIT_VAR_VALUE.as_ptr().cast(),
        ) != Status::SUCCESS
        {
            stop(STOP_BOOT_SERVICES, 77);
        }
        record_exit_notification(probe, b'E');
    }
}

unsafe extern "efiapi" fn notify_legacy_exit(_: Event, context: *mut c_void) {
    unsafe { record_exit_notification(context.cast(), b'L') };
}

unsafe extern "efiapi" fn notify_unexpected_exit_timer(_: Event, _: *mut c_void) {
    stop(STOP_BOOT_SERVICES, 56);
}

#[inline(never)]
fn verify_exit_boot_services(system: *mut SystemTable, tsc_hz: u64) {
    // SAFETY: the environment is the private BSP diagnostic. The System
    // Table, context, and outputs stay live through both exit attempts.
    unsafe {
        let boot = ptr::addr_of!((*system).boot_services).read_volatile();
        let create = ptr::addr_of!((*boot).create_event).read_volatile();
        let create_ex = ptr::addr_of!((*boot).create_event_ex).read_volatile();
        let set_timer = ptr::addr_of!((*boot).set_timer).read_volatile();
        let install = ptr::addr_of!((*boot).install_protocol_interface).read_volatile();
        let get_map = ptr::addr_of!((*boot).get_memory_map).read_volatile();
        let exit = ptr::addr_of!((*boot).exit_boot_services).read_volatile();
        let mut image = ptr::null_mut();
        let mut before_event = ptr::null_mut();
        let mut exit_event = ptr::null_mut();
        let mut legacy_event = ptr::null_mut();
        let mut timer_event = ptr::null_mut();
        let mut probe = ExitProbe {
            system,
            boot,
            tsc_hz,
            trace: [0; 4],
            count: 0,
        };
        let context = ptr::addr_of_mut!(probe).cast();
        if install(&mut image, &DIAGNOSTIC_PROTOCOL, 0, ptr::null_mut()) != Status::SUCCESS
            || set_boot_image_handle(image) != Status::SUCCESS
            || create_ex(
                EVT_NOTIFY_SIGNAL,
                TPL_CALLBACK,
                Some(notify_before_exit),
                context,
                &BEFORE_EXIT_BOOT_SERVICES,
                &mut before_event,
            ) != Status::SUCCESS
            || create_ex(
                EVT_NOTIFY_SIGNAL,
                TPL_CALLBACK,
                Some(notify_modern_exit),
                context,
                &EXIT_BOOT_SERVICES,
                &mut exit_event,
            ) != Status::SUCCESS
            || create(
                EVT_SIGNAL_EXIT_BOOT_SERVICES,
                TPL_CALLBACK,
                Some(notify_legacy_exit),
                context,
                &mut legacy_event,
            ) != Status::SUCCESS
            || create(
                EVT_TIMER | EVT_NOTIFY_SIGNAL,
                TPL_CALLBACK,
                Some(notify_unexpected_exit_timer),
                ptr::null_mut(),
                &mut timer_event,
            ) != Status::SUCCESS
            || set_timer(timer_event, TIMER_RELATIVE, 1_000) != Status::SUCCESS
        {
            stop(STOP_BOOT_SERVICES, 53);
        }

        let mut descriptors = [MemoryDescriptor::EMPTY; BOOT_DESCRIPTOR_CAPACITY];
        let mut read_key = || {
            let mut size = size_of_val(&descriptors);
            let mut key = 0;
            if get_map(
                &mut size,
                descriptors.as_mut_ptr(),
                &mut key,
                ptr::null_mut(),
                ptr::null_mut(),
            ) != Status::SUCCESS
            {
                stop(STOP_BOOT_SERVICES, 55);
            }
            key
        };
        let key = read_key();
        let before_trace = ptr::addr_of!(probe.trace);
        if exit(ptr::null_mut(), key) != Status::INVALID_PARAMETER
            || ptr::addr_of!(probe.count).read() != 0
            || exit(image, key) != Status::INVALID_PARAMETER
            || before_trace.read() != *b"B\0\0\0"
            || ptr::addr_of!((*system).boot_services).read() != boot
        {
            stop(STOP_BOOT_SERVICES, 55);
        }
        let fresh_key = read_key();
        if key.checked_add(3) != Some(fresh_key) || exit(image, fresh_key) != Status::SUCCESS {
            stop(STOP_BOOT_SERVICES, 56);
        }

        // Only the diagnostic inspects raw memory and prints after the exit.
        // No Boot Services pointer is used again.
        let trace = before_trace.read();
        if !matches!(&trace, b"BEL\0" | b"BLE\0")
            || ptr::addr_of!(probe.count).read() != 3
            || !ptr::addr_of!((*system).boot_services).read().is_null()
        {
            stop(STOP_BOOT_SERVICES, 57);
        }
        verify_retained_variables(system);
        com1_write(b"OD-FW S7-05 exit boot services verified BS=0x");
        com1_write(&fixed_hex(boot.addr() as u64));
        com1_write(b"\r\n");
    }
}

#[inline(never)]
unsafe fn verify_retained_variables(system: *mut SystemTable) {
    use od_firmware::var::codec::Decoder;

    // SAFETY: the private diagnostic still owns the initial identity mapping
    // and these objects were allocated as RuntimeServicesData before exit.
    let image = unsafe {
        let storage = ptr::addr_of!((*system).runtime_services)
            .read()
            .cast::<RuntimeServicesStorage>();
        (*(*storage).variables).nv_image()
    };
    let Ok(mut records) = Decoder::new(image) else {
        stop(STOP_BOOT_SERVICES, 78);
    };
    if !records.any(|record| {
        record.is_ok_and(|record| {
            record.guid == DIAGNOSTIC_PROTOCOL
                && record.name.units().eq(EXIT_VAR_NAME[..6].iter().copied())
                && record.value == EXIT_VAR_VALUE
        })
    }) {
        stop(STOP_BOOT_SERVICES, 78);
    }
    com1_write(b"OD-FW S7-09 variable image retained\r\n");
}

static mut EXCEPTION_IDT: Idt = Idt::EMPTY;

fn install_idt() {
    #[repr(C, packed)]
    struct Idtr {
        limit: u16,
        base: u64,
    }
    let base = &raw mut EXCEPTION_IDT;
    let stubs = (&raw const od_fw_exception_stubs) as u64;
    // SAFETY: this runs once at boot with interrupts disabled. The table
    // is in the firmware shadow's writable, NX section and is not aliased.
    unsafe { (*base).fill(stubs) };
    let pointer = Idtr {
        base: base as u64,
        limit: Idt::LIMIT,
    };
    // SAFETY: both the pseudo-descriptor and the installed gates remain
    // readable under the active firmware CR3 at CPL0.
    unsafe {
        asm!("lidt [{}]", in(reg) &pointer, options(readonly, nostack, preserves_flags));
    }
}

fn enable_firmware_fp_controls() {
    const CR4_FIRMWARE: u64 = (1 << 5) | (1 << 9) | (1 << 10);
    // SAFETY: CPL0 with 4-level paging. Keep PAE and enable the FP control
    // bits required at the later EFI entry point.
    unsafe { asm!("mov cr4, rax", in("rax") CR4_FIRMWARE, options(nostack)) };
}

#[unsafe(no_mangle)]
extern "C" fn od_fw_exception_stop(vector: u64) -> ! {
    stop(STOP_EXCEPTION, vector);
}

fn require_pages<T>(result: Result<T, PageError>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => stop(STOP_PAGE_ALLOCATOR, error.stop_detail()),
    }
}

#[inline(never)]
fn verify_boot_page_allocator(allocator: &mut PageAllocator<'_, '_>, active_cr3: u64) -> u64 {
    let exact = require_pages(allocator.allocate_pages(
        AllocateAt::Address(BOOT_PAGE_TABLES_GPA),
        MemoryType::BOOT_SERVICES_DATA,
        3,
    ));
    let bounded = require_pages(allocator.allocate_pages(
        AllocateAt::MaxAddress(BOOT_PAGE_TABLES_GPA + 4 * PAGE_BYTES - 1),
        MemoryType::LOADER_DATA,
        1,
    ));
    let high =
        require_pages(allocator.allocate_pages(AllocateAt::Any, MemoryType::BOOT_SERVICES_DATA, 1));
    if exact != BOOT_PAGE_TABLES_GPA || bounded != exact + 3 * PAGE_BYTES {
        stop(STOP_PAGE_ALLOCATOR, 7);
    }
    let allocated_key = allocator.map().key();
    if allocator.free_pages(active_cr3, 1) != Err(PageError::NotFound)
        || allocator.map().key() != allocated_key
    {
        stop(STOP_PAGE_ALLOCATOR, 7);
    }

    // SAFETY: both addresses are aligned, live allocations in conventional
    // identity-mapped RAM. The validated map excludes the active tables,
    // firmware shadow and stack. The old root is no longer in use.
    unsafe {
        let old_root = exact as *mut u64;
        let high_page = high as *mut u64;
        const ROOT_PATTERN: u64 = 0x0123_4567_89ab_cdef;
        const PAGE_PATTERN: u64 = 0x1020_3040_5060_7080;
        old_root.write_volatile(ROOT_PATTERN);
        high_page.write_volatile(PAGE_PATTERN);
        if old_root.read_volatile() != ROOT_PATTERN || high_page.read_volatile() != PAGE_PATTERN {
            stop(STOP_PAGE_ALLOCATOR, 7);
        }
    }

    require_pages(allocator.free_pages(exact + 2 * PAGE_BYTES, 2));
    let partial_key = allocator.map().key();
    if allocator.free_pages(bounded, 1) != Err(PageError::NotFound)
        || allocator.map().key() != partial_key
    {
        stop(STOP_PAGE_ALLOCATOR, 7);
    }
    require_pages(allocator.free_pages(exact, 2));
    require_pages(allocator.free_pages(high, 1));
    allocator.map().key()
}

fn require_pool<T>(result: Result<T, PoolError>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => stop(STOP_POOL_ALLOCATOR, error.stop_detail()),
    }
}

#[inline(never)]
fn verify_boot_pool_allocator(allocator: &mut PageAllocator<'_, '_>) -> u64 {
    let mut memory = IdentityPoolMemory;
    let initial_key = allocator.map().key();
    let first = require_pool(allocator.allocate_pool(&mut memory, MemoryType::LOADER_DATA, 16));
    let second =
        require_pool(allocator.allocate_pool(&mut memory, MemoryType::LOADER_DATA, PAGE_BYTES));
    let first_page = first & !(PAGE_BYTES - 1);
    let allocated_key = allocator.map().key();
    if (first | second) & 7 != 0
        || second + PAGE_BYTES > first
        || allocator.free_pages(first_page, 1) != Err(PageError::NotFound)
        || allocator.free_pool(&mut memory, first + 8) != Err(PoolError::InvalidPointer)
        || allocator.map().key() != allocated_key
    {
        stop(STOP_POOL_ALLOCATOR, 7);
    }
    // SAFETY: all three positions are aligned and inside live, disjoint
    // payloads in writable RAM mapped by the current firmware page tables.
    unsafe {
        let first_value = first as *mut u64;
        let second_value = second as *mut u64;
        let second_tail = (second + PAGE_BYTES - 8) as *mut u64;
        const FIRST: u64 = 0x091a_2b3c_4d5e_6f70;
        const SECOND: u64 = 0x0123_4500_0067_89ab;
        const TAIL: u64 = 0xf0e1_d2c3_b4a5_9687;
        first_value.write_volatile(FIRST);
        second_value.write_volatile(SECOND);
        second_tail.write_volatile(TAIL);
        if first_value.read_volatile() != FIRST
            || second_value.read_volatile() != SECOND
            || second_tail.read_volatile() != TAIL
        {
            stop(STOP_POOL_ALLOCATOR, 7);
        }
    }
    require_pool(allocator.free_pool(&mut memory, first));
    let freed_key = allocator.map().key();
    if allocator.free_pool(&mut memory, first) != Err(PoolError::InvalidPointer)
        || allocator.map().key() != freed_key
    {
        stop(STOP_POOL_ALLOCATOR, 7);
    }
    require_pool(allocator.free_pool(&mut memory, second));
    let key = allocator.map().key();
    if key != initial_key + 4 {
        stop(STOP_POOL_ALLOCATOR, 7);
    }
    key
}

fn boot_handoff_bytes() -> &'static [u8] {
    // SAFETY: both the initial and activated maps include the fixed readable
    // handoff window. The firmware never writes to these bytes.
    unsafe { core::slice::from_raw_parts(HANDOFF_GPA as *const u8, HANDOFF_WINDOW_SIZE) }
}

fn shadow_sections() -> ShadowSections {
    ShadowSections {
        read_only_start: (&raw const __shadow_read_only_start) as u64,
        writable_start: (&raw const __shadow_writable_start) as u64,
    }
}

#[inline(never)]
fn com1_write_memory_map(map: &MemoryMap<'_>) {
    com1_write(b"OD-FW S7-04 memory map key=0x");
    com1_write(&fixed_hex(map.key()));
    com1_write(b" version=0x");
    com1_write(&fixed_hex(u64::from(DESCRIPTOR_VERSION)));
    com1_write(b" count=0x");
    com1_write(&fixed_hex(map.descriptors().len() as u64));
    com1_write(b"\r\n");
    for descriptor in map.descriptors() {
        com1_write(b"OD-FW MEM type=0x");
        com1_write(&fixed_hex(u64::from(descriptor.memory_type)));
        com1_write(b" base=0x");
        com1_write(&fixed_hex(descriptor.physical_start));
        com1_write(b" pages=0x");
        com1_write(&fixed_hex(descriptor.number_of_pages));
        com1_write(b" attr=0x");
        com1_write(&fixed_hex(descriptor.attribute));
        com1_write(b"\r\n");
    }
}

#[inline(never)]
fn prepare_paging(handoff: &Handoff<'_>) -> Result<(u64, u64), PagingError> {
    let paging = BootPaging::plan(handoff)?;
    // SAFETY: the accepted handoff describes this boot's identity-mapped RAM.
    // The chosen page-aligned run belongs to a single ordinary RAM extent
    // distinct from the shadow, the handoff, the active tables and the stack.
    let words =
        unsafe { core::slice::from_raw_parts_mut(paging.root() as *mut u64, paging.table_words()) };
    paging.build(words, shadow_sections())?;
    Ok((paging.root(), paging.stack_top()))
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    let line = info
        .location()
        .map_or(0, |location| u64::from(location.line()));
    stop(STOP_INTERNAL, line)
}
