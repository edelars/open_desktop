//! The fixture's own 4 KiB x64 page tables.

use core::{arch::asm, ptr};
use od_firmware::paging::PAGE_BYTES;

const PAGE_WORDS: usize = (PAGE_BYTES / 8) as usize;
const ADDRESS: u64 = 0x0000_000f_ffff_f000;
const PRESENT: u64 = 1;
const WRITE: u64 = 2;
const LARGE_PAGE: u64 = 1 << 7;

pub const TABLE_PAGES: usize = 32;

pub struct Tables {
    root: u64,
    used: usize,
}

impl Tables {
    /// The allocated pages are writable and still identity-mapped.
    pub unsafe fn new(root: u64) -> Self {
        let mut tables = Self { root, used: 0 };
        unsafe { tables.next_page() }.unwrap();
        tables
    }

    unsafe fn next_page(&mut self) -> Option<u64> {
        if self.used == TABLE_PAGES {
            return None;
        }
        let page = self.root + self.used as u64 * PAGE_BYTES;
        self.used += 1;
        let entries = ptr::with_exposed_provenance_mut::<u64>(page as usize);
        // SAFETY: these pages belong to the fixture and have no live entries.
        unsafe { entries.write_bytes(0, PAGE_WORDS) };
        Some(page)
    }

    /// Read the boot leaf to carry the current NX/cache policy into the fixture; the mapping is
    /// writable (see below).
    pub unsafe fn map(&mut self, old_root: u64, physical: u64, linear: u64) -> bool {
        unsafe {
            let Some(leaf) = leaf(old_root, physical) else {
                return false;
            };
            let bits = leaf.read_volatile();
            if bits & PRESENT == 0 || bits & ADDRESS != physical {
                return false;
            }

            let mut table = self.root;
            for shift in [39, 30, 21] {
                let entry = entry(table, linear, shift);
                let mut bits = entry.read_volatile();
                if bits & PRESENT == 0 {
                    let Some(next) = self.next_page() else {
                        return false;
                    };
                    bits = next | PRESENT | WRITE;
                    entry.write_volatile(bits);
                }
                table = bits & ADDRESS;
            }
            // An OS maps the runtime images writable while SetVirtualAddressMap runs (Linux and
            // EDK2 do) and applies the final permissions later; the driver writes its fixups
            // through these mappings without touching CR0.WP (S7-31k).
            entry(table, linear, 12).write_volatile(bits | WRITE);
        }
        true
    }

    /// Drop the physical alias while keeping the page-table pages accessible.
    pub unsafe fn unmap(&self, linear: u64) {
        unsafe { leaf(self.root, linear).unwrap().write_volatile(0) };
    }

    pub unsafe fn activate(&self) {
        unsafe { asm!("mov cr3, rax", in("rax") self.root, options(nostack)) };
    }
}

fn entry(table: u64, address: u64, shift: u32) -> *mut u64 {
    let index = (address >> shift) & 0x1ff;
    ptr::with_exposed_provenance_mut((table + index * 8) as usize)
}

unsafe fn leaf(root: u64, address: u64) -> Option<*mut u64> {
    let mut table = root;
    for shift in [39, 30, 21] {
        let bits = unsafe { entry(table, address, shift).read_volatile() };
        if bits & (PRESENT | LARGE_PAGE) != PRESENT {
            return None;
        }
        table = bits & ADDRESS;
    }
    Some(entry(table, address, 12))
}
