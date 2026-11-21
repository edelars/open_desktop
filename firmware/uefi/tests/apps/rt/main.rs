#![no_std]
#![no_main]

use core::{
    arch::{asm, naked_asm},
    mem::MaybeUninit,
    ptr, slice,
};
use od_firmware::{
    mem::map::{MemoryDescriptor, MemoryType, RUNTIME},
    paging::PAGE_BYTES,
    pe::for_each_dir64,
    uefi::{
        guid::Guid,
        runtime::{RESET_SHUTDOWN, RT_PROPERTIES_TABLE_GUID, Time, TimeCapabilities},
        status::Status,
        system_table::{RuntimeServices, SystemTable},
        types::{Char16, Handle},
    },
    var::services::{BOOTSERVICE_ACCESS, NON_VOLATILE_RUNTIME},
};
use od_firmware_test_apps::ascii_to_char16;

mod paging;
mod secure_vars;
use paging::{TABLE_PAGES, Tables};

const MAP_CAPACITY: usize = 96;
const STACK_PAGES: usize = 8;
const CODE: u32 = MemoryType::RUNTIME_SERVICES_CODE.raw();
const DATA: u32 = MemoryType::RUNTIME_SERVICES_DATA.raw();
const MMIO: u32 = MemoryType::MEMORY_MAPPED_IO.raw();
const LOADER_CODE: u32 = MemoryType::LOADER_CODE.raw();
const LOADER_DATA: u32 = MemoryType::LOADER_DATA.raw();
const VENDOR: Guid = Guid {
    data1: 0xc7d8_6aba,
    data2: 0xb83b,
    data3: 0x46cc,
    data4: [0x88, 0xa4, 0x25, 0xe6, 0xe3, 0x71, 0x61, 0x12],
};
static BOOT_ONLY: [Char16; 11] = ascii_to_char16(b"ODBootOnly\0");
static NV: [Char16; 9] = ascii_to_char16(b"ODRtTest\0");
static NV_POINTER: &[Char16; 9] = &NV;

struct LoaderContext {
    system: *mut SystemTable,
    tables: u64,
    count: usize,
    version: u32,
    protected_nvram: bool,
    map: [MaybeUninit<MemoryDescriptor>; MAP_CAPACITY],
}

const _: () = assert!(size_of::<LoaderContext>() <= PAGE_BYTES as usize);

#[unsafe(no_mangle)]
unsafe extern "efiapi" fn efi_main(image: Handle, system: *mut SystemTable) -> Status {
    if image.is_null() || system.is_null() {
        return Status::INVALID_PARAMETER;
    }
    unsafe {
        let boot = (*system).boot_services;
        let runtime = (*system).runtime_services;
        let name = ptr::read_volatile(&NV_POINTER);
        let value = 0x7bu8;
        if ((*runtime).set_variable)(
            BOOT_ONLY.as_ptr(),
            &VENDOR,
            BOOTSERVICE_ACCESS,
            1,
            ptr::from_ref(&value).cast(),
        ) != Status::SUCCESS
            || ((*runtime).set_variable)(
                name.as_ptr(),
                &VENDOR,
                NON_VOLATILE_RUNTIME,
                1,
                ptr::from_ref(&value).cast(),
            ) != Status::SUCCESS
        {
            return Status::DEVICE_ERROR;
        }
        let protected_nvram = match secure_vars::available(runtime) {
            Ok(value) => value,
            Err(status) => return status,
        };
        if protected_nvram {
            if !secure_vars::before_exit(boot, runtime) {
                return Status::DEVICE_ERROR;
            }
            trace(b"OD-UEFI protected Auth2 before EBS\r\n");
        }
        let mut boot_count = 0u64;
        if ((*boot).get_next_monotonic_count)(&mut boot_count) != Status::SUCCESS
            || boot_count != 0x1_0000_0001
        {
            return Status::DEVICE_ERROR;
        }

        let mut pages = 0u64;
        let total_pages = TABLE_PAGES + 1 + STACK_PAGES;
        let status = ((*boot).allocate_pages)(0, MemoryType::LOADER_DATA, total_pages, &mut pages);
        if status != Status::SUCCESS {
            return status;
        }
        let context = ptr::with_exposed_provenance_mut::<LoaderContext>(
            (pages + TABLE_PAGES as u64 * PAGE_BYTES) as usize,
        );
        let stack = pages + total_pages as u64 * PAGE_BYTES;
        for _ in 0..2 {
            let mut size = size_of::<[MemoryDescriptor; MAP_CAPACITY]>();
            let mut key = 0;
            let mut stride = 0;
            let mut version = 0;
            let status = ((*boot).get_memory_map)(
                &mut size,
                ptr::addr_of_mut!((*context).map).cast::<MemoryDescriptor>(),
                &mut key,
                &mut stride,
                &mut version,
            );
            if status != Status::SUCCESS {
                return status;
            }
            if stride != size_of::<MemoryDescriptor>() || size % stride != 0 {
                return Status::INVALID_PARAMETER;
            }
            match ((*boot).exit_boot_services)(image, key) {
                Status::SUCCESS => {
                    ptr::addr_of_mut!((*context).system).write(system);
                    ptr::addr_of_mut!((*context).tables).write(pages);
                    ptr::addr_of_mut!((*context).count).write(size / stride);
                    ptr::addr_of_mut!((*context).version).write(version);
                    ptr::addr_of_mut!((*context).protected_nvram).write(protected_nvram);
                    switch_to_loader_stack(context, stack);
                }
                Status::INVALID_PARAMETER => {}
                other => return other,
            }
        }
    }
    Status::INVALID_PARAMETER
}

#[unsafe(naked)]
unsafe extern "efiapi" fn switch_to_loader_stack(_: *mut LoaderContext, _: u64) -> ! {
    // RCX is the context and RDX is the page-aligned top of loader-owned stack.
    // Reserve the x64 EFI home area; the new stack outlives Boot Services.
    naked_asm!("mov rsp, rdx", "sub rsp, 0x20", "call {finish}", "ud2", finish = sym finish);
}

unsafe extern "efiapi" fn finish(context: *mut LoaderContext) -> ! {
    unsafe {
        let system = (*context).system;
        let version = (*context).version;
        let protected_nvram = (*context).protected_nvram;
        let map = slice::from_raw_parts_mut(
            ptr::addr_of_mut!((*context).map).cast::<MemoryDescriptor>(),
            (*context).count,
        );
        let old_root: u64;
        asm!("mov rax, cr3", out("rax") old_root, options(nostack));
        let mut tables = Tables::new((*context).tables);
        let runtime = (*system).runtime_services;
        let set_map = (*runtime).set_virtual_address_map;
        let convert = (*runtime).convert_pointer;
        for desc in map.iter_mut() {
            let retained = desc.attribute & RUNTIME != 0;
            if retained {
                let delta = match desc.memory_type {
                    CODE => 0x0080_0000_0000,
                    DATA => 0x0100_0000_0000,
                    MMIO => 0x0180_0000_0000,
                    _ => fail(b'M'),
                };
                desc.virtual_start = desc.physical_start + delta;
            }
            if retained || matches!(desc.memory_type, LOADER_CODE | LOADER_DATA) {
                for page in 0..desc.number_of_pages {
                    let offset = page * PAGE_BYTES;
                    let physical = desc.physical_start + offset;
                    if !tables.map(old_root, physical, physical)
                        || (retained
                            && !tables.map(old_root, physical, desc.virtual_start + offset))
                    {
                        fail(b'P');
                    }
                }
            }
        }
        let virtual_system = mapped_pointer(map, system);
        let virtual_runtime = mapped_pointer(map, runtime);
        let vendor = (*system).firmware_vendor();
        let (configuration, count) = (*system).configuration_tables();
        let virtual_vendor = mapped_pointer(map, vendor.cast_mut());
        let virtual_configuration = mapped_pointer(map, configuration);
        let virtual_get = mapped_pointer(map, (*runtime).get_variable as *mut u8);
        let fixup =
            find_loaded_fixup(map, (*runtime).get_time as u64).unwrap_or_else(|| fail(b'R'));
        let virtual_fixup = mapped_pointer(map, fixup.cast_mut());
        let virtual_time = mapped_pointer(map, (*runtime).get_time as *mut u8);
        asm!("cli", options(nostack));
        tables.activate();

        // The first CR3 already excludes backed Boot Services pages.
        expect_variable(runtime, 0x7b);
        let mut size = 0usize;
        if ((*runtime).get_variable)(
            BOOT_ONLY.as_ptr(),
            &VENDOR,
            ptr::null_mut(),
            &mut size,
            ptr::null_mut(),
        ) != Status::NOT_FOUND
        {
            fail(b'B');
        }
        trace(b"OD-UEFI rt physical\r\n");
        check_unsupported_slots(runtime);
        if protected_nvram {
            if !secure_vars::after_exit(runtime) {
                fail(b'X');
            }
            trace(b"OD-UEFI protected Auth2 after EBS\r\n");
            if !secure_vars::verify_authoritative_monotonic(runtime) {
                fail(b'I');
            }
            trace(b"OD-UEFI protected NVRAM direct refresh\r\n");
        }
        let stride = size_of::<MemoryDescriptor>();
        if convert(0, ptr::null_mut()) != Status::UNSUPPORTED
            || set_map(0, stride, version, map.as_ptr()) != Status::NO_MAPPING
            || (*system).runtime_services != runtime
            || set_map(size_of_val(map), stride, version, map.as_ptr()) != Status::SUCCESS
            || (*virtual_system).runtime_services != virtual_runtime
            || (*virtual_runtime).get_variable as *mut u8 != virtual_get
            || set_map(size_of_val(map), stride, version, map.as_ptr()) != Status::UNSUPPORTED
        {
            fail(b'V');
        }

        for desc in map.iter().filter(|desc| desc.attribute & RUNTIME != 0) {
            for page in 0..desc.number_of_pages {
                tables.unmap(desc.physical_start + page * PAGE_BYTES);
            }
        }
        tables.activate();
        if (*virtual_system).firmware_vendor() != virtual_vendor
            || (*virtual_system).configuration_tables() != (virtual_configuration, count)
            || virtual_vendor.read() != u16::from(b'O')
            || !slice::from_raw_parts(virtual_configuration, count)
                .iter()
                .any(|entry| entry.vendor_guid == RT_PROPERTIES_TABLE_GUID)
        {
            fail(b'K');
        }
        if virtual_fixup.read_unaligned() != virtual_time as u64 {
            fail(b'R');
        }
        let mut rt_copy = virtual_runtime.read();
        let mut system_copy = virtual_system.read();
        let (rt_crc, system_crc) = (rt_copy.header.crc32, system_copy.header.crc32);
        rt_copy.update_crc32();
        system_copy.update_crc32();
        if (rt_copy.header.crc32, system_copy.header.crc32) != (rt_crc, system_crc) {
            fail(b'K');
        }
        expect_variable(virtual_runtime, 0x7b);
        let value = 0xc1u8;
        if ((*virtual_runtime).set_variable)(
            NV.as_ptr(),
            &VENDOR,
            NON_VOLATILE_RUNTIME,
            1,
            ptr::from_ref(&value).cast(),
        ) != Status::SUCCESS
        {
            fail(b'N');
        }
        expect_variable(virtual_runtime, value);
        if protected_nvram {
            if !secure_vars::after_virtual_map(virtual_runtime) {
                fail(b'W');
            }
            trace(b"OD-UEFI protected Auth2 after VA\r\n");
        }
        let mut high = 0u32;
        if ((*virtual_runtime).get_next_high_monotonic_count)(&mut high) != Status::SUCCESS
            || high != if protected_nvram { 43 } else { 2 }
        {
            fail(b'H');
        }
        check_time(virtual_runtime);
        check_unsupported_slots(virtual_runtime);
        trace(b"OD-UEFI rt unsupported slots\r\n");
        trace(b"OD-UEFI rt virtual\r\n");
        trace(b"OD-UEFI rt shutdown\r\n");
        ((*virtual_runtime).reset_system)(RESET_SHUTDOWN, Status::SUCCESS, 0, ptr::null());
    }
    fail(b'S')
}

/// Find the actual PE template word that supplied GetTime to the RT table.
unsafe fn find_loaded_fixup(map: &[MemoryDescriptor], get_time: u64) -> Option<*const u64> {
    let code = map.iter().find(|desc| {
        desc.memory_type == CODE
            && get_time >= desc.physical_start
            && get_time - desc.physical_start < desc.number_of_pages * PAGE_BYTES
    })?;
    let bytes = unsafe {
        slice::from_raw_parts(
            ptr::with_exposed_provenance::<u8>(code.physical_start as usize),
            (code.number_of_pages * PAGE_BYTES) as usize,
        )
    };
    let read_u32 = |offset| {
        let word = bytes.get(offset..offset + 4)?;
        Some(u32::from_le_bytes(word.try_into().ok()?))
    };
    let pe_header = read_u32(0x3c)? as usize;
    // The loaded headers retain the PE32+ optional header and data directories.
    let relocation = pe_header + 24 + 112 + 5 * 8;
    let rva = read_u32(relocation)?;
    let length = read_u32(relocation + 4)?;
    let directory = bytes.get(rva as usize..rva.checked_add(length)? as usize)?;
    let mut found = None;
    for_each_dir64(rva, directory, bytes.len() as u32, |offset| {
        let candidate = unsafe { bytes.as_ptr().add(offset).cast::<u64>() };
        if unsafe { candidate.read_unaligned() } == get_time {
            found = Some(candidate);
        }
    })
    .ok()?;
    found
}

fn mapped_pointer<T>(map: &[MemoryDescriptor], pointer: *mut T) -> *mut T {
    for desc in map {
        if desc.attribute & RUNTIME != 0
            && let Some(offset) = (pointer.addr() as u64).checked_sub(desc.physical_start)
            && offset < desc.number_of_pages * PAGE_BYTES
        {
            return ptr::with_exposed_provenance_mut((desc.virtual_start + offset) as usize);
        }
    }
    fail(b'A')
}

unsafe fn expect_variable(runtime: *mut RuntimeServices, expected: u8) {
    let mut size = 1;
    let mut attributes = 0;
    let mut value = 0u8;
    if unsafe {
        ((*runtime).get_variable)(
            NV.as_ptr(),
            &VENDOR,
            &mut attributes,
            &mut size,
            ptr::from_mut(&mut value).cast(),
        )
    } != Status::SUCCESS
        || (size, attributes, value) != (1, NON_VOLATILE_RUNTIME, expected)
    {
        fail(b'G');
    }
}

unsafe fn check_time(runtime: *mut RuntimeServices) {
    let mut time = MaybeUninit::<Time>::uninit();
    let mut capabilities = MaybeUninit::<TimeCapabilities>::uninit();
    unsafe {
        let get = (*runtime).get_time;
        let set = (*runtime).set_time;
        if get(time.as_mut_ptr(), capabilities.as_mut_ptr()) != Status::SUCCESS {
            fail(b'T');
        }
        let mut now = time.assume_init();
        let capabilities = capabilities.assume_init();
        if (now.year, now.month, now.day) != (2024, 1, 1)
            || (capabilities.resolution, capabilities.sets_to_zero) != (1, 1)
        {
            fail(b'C');
        }
        now.year = 1969;
        now.month = 12;
        now.day = 31;
        now.hour = 13;
        now.minute = 24;
        now.time_zone = 180;
        now.daylight = 1;
        if set(&now) != Status::SUCCESS
            || get(&mut now, ptr::null_mut()) != Status::SUCCESS
            || (
                now.year,
                now.month,
                now.day,
                now.hour,
                now.minute,
                now.time_zone,
                now.daylight,
            ) != (1969, 12, 31, 13, 24, 180, 1)
        {
            fail(b'W');
        }
    }
}

/// The four slots the profile leaves at EFI_UNSUPPORTED (UEFI_PROFILE.md, Runtime
/// Services) must return that status and leave every caller buffer untouched.
unsafe fn check_unsupported_slots(runtime: *mut RuntimeServices) {
    const SENTINEL: u64 = 0xa5a5_a5a5_a5a5_a5a5;
    const _: () = assert!(size_of::<Time>() == 16);
    // Scalar volatile accesses: an array-compare form of this check stopped the machine with
    // Unsupported { opcode: 51 }, an instruction the firmware CPU profile does not cover.
    let mut time = MaybeUninit::<[u64; 2]>::uninit();
    let time_words = time.as_mut_ptr().cast::<u64>();
    let (mut enabled, mut pending) = (SENTINEL as u8, SENTINEL as u8);
    let (mut maximum, mut reset) = (SENTINEL, SENTINEL as u32);
    unsafe {
        ptr::write_volatile(time_words, SENTINEL);
        ptr::write_volatile(time_words.add(1), SENTINEL);
        let time_pointer = time_words.cast::<Time>();
        if ((*runtime).get_wakeup_time)(&mut enabled, &mut pending, time_pointer)
            != Status::UNSUPPORTED
            || ((*runtime).set_wakeup_time)(1, time_pointer) != Status::UNSUPPORTED
            || ((*runtime).update_capsule)(ptr::null(), 0, 0) != Status::UNSUPPORTED
            || ((*runtime).query_capsule_capabilities)(ptr::null(), 0, &mut maximum, &mut reset)
                != Status::UNSUPPORTED
            || ptr::read_volatile(&enabled) != SENTINEL as u8
            || ptr::read_volatile(&pending) != SENTINEL as u8
            || ptr::read_volatile(&maximum) != SENTINEL
            || ptr::read_volatile(&reset) != SENTINEL as u32
            || ptr::read_volatile(time_words) != SENTINEL
            || ptr::read_volatile(time_words.add(1)) != SENTINEL
        {
            fail(b'U');
        }
    }
}

fn trace(bytes: &[u8]) {
    for &byte in bytes {
        let mut line: u8;
        for _ in 0..65_536 {
            unsafe { asm!("in al, dx", in("dx") 0x3fdu16, out("al") line, options(nostack)) };
            if line & 0x20 != 0 {
                unsafe { asm!("out dx, al", in("dx") 0x3f8u16, in("al") byte, options(nostack)) };
                break;
            }
        }
    }
}

fn fail(code: u8) -> ! {
    trace(b"OD-UEFI rt failure ");
    trace(slice::from_ref(&code));
    trace(b"\r\n");
    unsafe { asm!("cli", "2:", "hlt", "jmp 2b", options(nostack, noreturn)) };
}
