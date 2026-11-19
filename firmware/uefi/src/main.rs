//! Binary of the firmware image. The S7-04 entry validates the boot handoff and prepares its
//! own identity page tables, and prints the initial EFI memory map after switching stacks.

#![no_std]
#![no_main]

use core::arch::asm;
use core::ffi::c_void;
use core::ptr;

mod serial;
use core::panic::PanicInfo;
use serial::{init as com1_init, write as com1_write};

use od_firmware::HANDOFF_GPA;
use od_firmware::bs::memory::IdentityPoolMemory;
use od_firmware::bs::protocol::{
    LOCATE_ALL_HANDLES, LOCATE_BY_PROTOCOL, OPEN_BY_DRIVER, OPEN_GET_PROTOCOL,
};
use od_firmware::device_path::{DEVICE_PATH_PROTOCOL_GUID, DevicePathNode};
use od_firmware::handoff::{BOOT_PAGE_TABLES_GPA, HANDOFF_WINDOW_SIZE, Handoff};
use od_firmware::idt::Idt;
use od_firmware::mem::map::{
    DESCRIPTOR_VERSION, MAX_INITIAL_DESCRIPTORS, MemoryDescriptor, MemoryMap, MemoryMapError,
    MemoryType,
};
use od_firmware::mem::pages::{AllocateAt, PageAllocator, PageError, PageOwner, PoolError};
use od_firmware::paging::{BootPaging, PAGE_BYTES, PagingError, ShadowSections};
use od_firmware::stop::{
    STOP_BOOT_SERVICES, STOP_EXCEPTION, STOP_INTERNAL, STOP_INVALID_RDI, STOP_MEMORY_MAP,
    STOP_PAGE_ALLOCATOR, STOP_PAGING, STOP_POOL_ALLOCATOR, fixed_hex, format_stop_line,
};
use od_firmware::uefi::guid::Guid;
use od_firmware::uefi::status::Status;
use od_firmware::uefi::system_table::{BootServices, RuntimeServices, SystemTable};
use od_firmware::uefi::types::Handle;

unsafe extern "C" {
    static __shadow_read_only_start: u8;
    static __shadow_writable_start: u8;
    fn od_fw_install_gdt();
    static od_fw_exception_stubs: u8;
}

// Keep this equal to the image header's handoff_minor_max in build_firmware.py.
const HANDOFF_MINOR_MAX: u16 = 4;
const VERSION_LINE: &[u8] = concat!("OD-FW version ", env!("CARGO_PKG_VERSION"), "\r\n").as_bytes();
const HANDOFF_VALIDATED_LINE: &[u8] = b"OD-FW S7-04 handoff validated\r\n";
const PAGING_PREPARED_LINE: &[u8] = b"OD-FW S7-04 page tables prepared\r\n";
const PAGING_ACTIVE_LINE: &[u8] = b"OD-FW S7-04 page tables active\r\n";
// Three live allocations can each introduce two additional descriptors.
const BOOT_DESCRIPTOR_CAPACITY: usize = MAX_INITIAL_DESCRIPTORS + 6;
static FIRMWARE_VENDOR: [u16; 13] = [
    0x4f, 0x70, 0x65, 0x6e, 0x20, 0x44, 0x65, 0x73, 0x6b, 0x74, 0x6f, 0x70, 0,
];
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
    if let Some(byte) = serial::try_read() {
        com1_write(b"OD-FW S7-04 COM1 RX byte=0x");
        com1_write(&fixed_hex(u64::from(byte)));
        com1_write(b"\r\n");
    }
    // SAFETY: this is the one BSP initialization; the diagnostic has finished
    // borrowing the allocator. All ensuing accesses go through Boot Services.
    unsafe { od_firmware::bs::initialize(allocator) };
    let mut boot = BootServices::diagnostic();
    let mut runtime = RuntimeServices::diagnostic();
    boot.update_crc32();
    runtime.update_crc32();
    let mut system = SystemTable::diagnostic(
        FIRMWARE_VENDOR.as_ptr(),
        ptr::addr_of_mut!(runtime),
        ptr::addr_of_mut!(boot),
    );
    system.update_crc32();
    verify_boot_services(&system);
    verify_memory_services(&system, key as usize);
    verify_protocol_services(&system);
    // Keep the full wire representations inspectable in RAM at the diagnostic halt.
    unsafe {
        asm!(
            "/* {system} {boot} {runtime} */",
            system = in(reg) &system,
            boot = in(reg) &boot,
            runtime = in(reg) &runtime,
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
