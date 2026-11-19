//! Binary of the firmware image. The S7-04 entry validates the boot handoff and prepares its
//! own identity page tables, and prints the initial EFI memory map after switching stacks.

#![no_std]
#![no_main]

use core::arch::asm;
use core::panic::PanicInfo;

use od_firmware::HANDOFF_GPA;
use od_firmware::handoff::{HANDOFF_WINDOW_SIZE, Handoff};
use od_firmware::mem::map::{
    DESCRIPTOR_VERSION, MAX_INITIAL_DESCRIPTORS, MemoryDescriptor, MemoryMap, MemoryMapError,
};
use od_firmware::paging::{BootPaging, PagingError, ShadowSections};
use od_firmware::stop::{
    STOP_INTERNAL, STOP_INVALID_RDI, STOP_MEMORY_MAP, STOP_PAGING, fixed_hex, format_stop_line,
};

unsafe extern "C" {
    static __shadow_read_only_start: u8;
    static __shadow_writable_start: u8;
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
    let mut descriptors = [MemoryDescriptor::EMPTY; MAX_INITIAL_DESCRIPTORS];
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
    halt_forever()
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
