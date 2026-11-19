//! Binary of the firmware image. The S7-04 entry validates the boot handoff and prepares its
//! own identity page tables, and prints the initial EFI memory map after switching stacks.

#![no_std]
#![no_main]

use core::arch::asm;
use core::panic::PanicInfo;

use od_firmware::HANDOFF_GPA;
use od_firmware::handoff::{BOOT_PAGE_TABLES_GPA, HANDOFF_WINDOW_SIZE, Handoff};
use od_firmware::idt::Idt;
use od_firmware::mem::map::{
    DESCRIPTOR_VERSION, MAX_INITIAL_DESCRIPTORS, MemoryDescriptor, MemoryMap, MemoryMapError,
    MemoryType,
};
use od_firmware::mem::pages::{
    AllocateAt, PageAllocator, PageError, PageOwner, PoolError, PoolHeader, PoolMemory,
};
use od_firmware::paging::{BootPaging, PAGE_BYTES, PagingError, ShadowSections};
use od_firmware::stop::{
    STOP_EXCEPTION, STOP_INTERNAL, STOP_INVALID_RDI, STOP_MEMORY_MAP, STOP_PAGE_ALLOCATOR,
    STOP_PAGING, STOP_POOL_ALLOCATOR, fixed_hex, format_stop_line,
};

unsafe extern "C" {
    static __shadow_read_only_start: u8;
    static __shadow_writable_start: u8;
    fn od_fw_install_gdt();
    static od_fw_exception_stubs: u8;
}

const COM1_BASE: u16 = 0x3F8;
const LSR_THRE: u8 = 0x20;
/// Polling limit before a byte is dropped (FIRMWARE_BOOT.md, section 11).
const TX_POLL_LIMIT: u32 = 65536;

// Keep this equal to the image header's handoff_minor_max in build_firmware.py.
const HANDOFF_MINOR_MAX: u16 = 4;
const HANDOFF_VALIDATED_LINE: &[u8] = b"OD-FW S7-04 handoff validated\r\n";
const PAGING_PREPARED_LINE: &[u8] = b"OD-FW S7-04 page tables prepared\r\n";
const PAGING_ACTIVE_LINE: &[u8] = b"OD-FW S7-04 page tables active\r\n";
// Three live allocations can each introduce two additional descriptors.
const BOOT_DESCRIPTOR_CAPACITY: usize = MAX_INITIAL_DESCRIPTORS + 6;

/// # Safety
/// Port I/O: the caller must be at CPL0 (or have I/O permission) and `port` must be a real device
/// register of the machine.
unsafe fn outb(port: u16, value: u8) {
    unsafe {
        asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags));
    }
}

/// # Safety
/// Same as [`outb`].
unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    unsafe {
        asm!("in al, dx", in("dx") port, out("al") value, options(nomem, nostack, preserves_flags));
    }
    value
}

fn com1_init() {
    // 8N1, divisor 1; IER/FCR/MCR are not written (FIRMWARE_BOOT.md, section 11).
    unsafe {
        outb(COM1_BASE + 3, 0x80);
        outb(COM1_BASE, 0x01);
        outb(COM1_BASE + 1, 0x00);
        outb(COM1_BASE + 3, 0x03);
    }
}

fn com1_write(bytes: &[u8]) {
    for &byte in bytes {
        for _ in 0..TX_POLL_LIMIT {
            if unsafe { inb(COM1_BASE + 5) } & LSR_THRE != 0 {
                unsafe { outb(COM1_BASE, byte) };
                break;
            }
        }
        // On timeout the byte is dropped, never a hang.
    }
}

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
    let mut descriptors = [MemoryDescriptor::EMPTY; BOOT_DESCRIPTOR_CAPACITY];
    let map = match MemoryMap::from_active_paging(
        &paging,
        active_cr3,
        shadow_sections(),
        &mut descriptors,
    ) {
        Ok(map) => map,
        Err(MemoryMapError::TablesNotActive) => stop(STOP_MEMORY_MAP, 1),
        Err(MemoryMapError::InvalidPlacement) => stop(STOP_MEMORY_MAP, 2),
        Err(MemoryMapError::BufferTooSmall { required }) => {
            stop(STOP_MEMORY_MAP, 3 | ((required as u64) << 32))
        }
        Err(MemoryMapError::Paging(error)) => stop(STOP_PAGING, error.stop_detail()),
    };
    com1_write_memory_map(&map);
    let mut ownership = [PageOwner::Initial; BOOT_DESCRIPTOR_CAPACITY];
    let mut allocator = require_pages(PageAllocator::new(map, &mut ownership));
    let key = verify_boot_page_allocator(&mut allocator, active_cr3);
    com1_write(b"OD-FW S7-04 page allocator verified key=0x");
    com1_write(&fixed_hex(key));
    com1_write(b"\r\n");
    let key = verify_boot_pool_allocator(&mut allocator);
    com1_write(b"OD-FW S7-04 pool allocator verified key=0x");
    com1_write(&fixed_hex(key));
    com1_write(b"\r\n");
    halt_forever()
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
        MemoryType::BootServicesData,
        3,
    ));
    let bounded = require_pages(allocator.allocate_pages(
        AllocateAt::MaxAddress(BOOT_PAGE_TABLES_GPA + 4 * PAGE_BYTES - 1),
        MemoryType::LoaderData,
        1,
    ));
    let high =
        require_pages(allocator.allocate_pages(AllocateAt::Any, MemoryType::BootServicesData, 1));
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

struct IdentityPoolMemory;

impl PoolMemory for IdentityPoolMemory {
    unsafe fn header_at(&mut self, address: u64) -> Option<&mut PoolHeader> {
        // SAFETY: the caller supplies an exclusive, aligned header position.
        // The active page tables identity-map this RAM and allow writes.
        Some(unsafe { &mut *(address as *mut PoolHeader) })
    }
}

#[inline(never)]
fn verify_boot_pool_allocator(allocator: &mut PageAllocator<'_, '_>) -> u64 {
    let mut memory = IdentityPoolMemory;
    let initial_key = allocator.map().key();
    let first = require_pool(allocator.allocate_pool(&mut memory, MemoryType::LoaderData, 16));
    let second =
        require_pool(allocator.allocate_pool(&mut memory, MemoryType::LoaderData, PAGE_BYTES));
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
