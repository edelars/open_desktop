//! Permission changes to existing identity mappings in the active firmware tables.

use super::{ENTRIES, NO_EXECUTE, PAGE_BYTES, PRESENT, WRITABLE};
use core::{ops::Range, ptr};

const ADDRESS: u64 = 0x000f_ffff_ffff_f000;
const LARGE_PAGE: u64 = 1 << 7;

#[derive(Clone, Copy)]
pub(crate) struct ActivePageTables {
    root: u64,
    end: u64,
}

impl ActivePageTables {
    /// The arena is private to the BSP and the active CR3 points to its first page.
    pub(crate) fn new(root: u64, bytes: u64) -> Self {
        Self {
            root,
            end: root + bytes,
        }
    }

    /// # Safety
    /// The arena must remain identity-mapped and reserved to firmware.
    pub(crate) unsafe fn verify(&self, span: Range<u64>, staging: bool) -> bool {
        if span.start >= span.end || (span.start | span.end) & (PAGE_BYTES - 1) != 0 {
            return false;
        }
        for page in span.step_by(PAGE_BYTES as usize) {
            let Some(pte) = (unsafe { self.leaf(page) }) else {
                return false;
            };
            if staging
                && unsafe { pte.read_volatile() } & (WRITABLE | NO_EXECUTE) != WRITABLE | NO_EXECUTE
            {
                return false;
            }
        }
        true
    }

    /// # Safety
    /// `span` passed `verify` under the same BSP guard. Finish mutations and
    /// flush before publishing permissions or handing pages back to EFI code.
    pub(crate) unsafe fn set(&self, span: Range<u64>, writable: bool, executable: bool) {
        let permissions =
            (if writable { WRITABLE } else { 0 }) | (if executable { 0 } else { NO_EXECUTE });
        for page in span.step_by(PAGE_BYTES as usize) {
            let pte = unsafe { self.leaf(page) }.expect("preflighted image leaf");
            let current = unsafe { pte.read_volatile() };
            unsafe { pte.write_volatile((current & !(WRITABLE | NO_EXECUTE)) | permissions) };
        }
    }

    /// # Safety
    /// The caller owns this CR3 on the BSP. The tables map the current stack and code.
    pub(crate) unsafe fn flush(&self) {
        #[cfg(target_os = "none")]
        unsafe {
            core::arch::asm!("mov cr3, rax", in("rax") self.root, options(nostack, preserves_flags));
        }
    }

    unsafe fn leaf(&self, page: u64) -> Option<*mut u64> {
        let mut table = self.root;
        for shift in [39, 30, 21] {
            let slot = slot_at(table, page, shift);
            let entry = unsafe { slot.read_volatile() };
            if entry & (PRESENT | WRITABLE | NO_EXECUTE | LARGE_PAGE) != PRESENT | WRITABLE {
                return None;
            }
            table = entry & ADDRESS;
            if !(self.root..self.end).contains(&table) {
                return None;
            }
        }
        let slot = slot_at(table, page, 12);
        let entry = unsafe { slot.read_volatile() };
        (entry & (PRESENT | ADDRESS) == PRESENT | page).then_some(slot)
    }
}

fn slot_at(table: u64, page: u64, shift: u32) -> *mut u64 {
    let index = (page >> shift) as usize & (ENTRIES - 1);
    ptr::with_exposed_provenance_mut(table as usize + index * size_of::<u64>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_only_permissions_on_existing_identity_leaves() {
        #[repr(align(4096))]
        struct Arena([u64; ENTRIES * 4]);
        let mut arena = Arena([0; ENTRIES * 4]);
        let root = arena.0.as_mut_ptr().expose_provenance() as u64;
        let tables = ActivePageTables::new(root, 4 * PAGE_BYTES);
        let page = 0x5000;
        for table in 0..3 {
            arena.0[table * ENTRIES] = root + (table as u64 + 1) * PAGE_BYTES | PRESENT | WRITABLE;
        }
        let leaf = ENTRIES * 3 + 5;
        let accessed_dirty = (1 << 5) | (1 << 6);
        arena.0[leaf] = page | PRESENT | WRITABLE | NO_EXECUTE | accessed_dirty;
        assert!(unsafe { tables.verify(page..page + PAGE_BYTES, true) });
        assert!(!unsafe { tables.verify(page..page + 2 * PAGE_BYTES, true) });
        unsafe { tables.set(page..page + PAGE_BYTES, false, true) };
        assert_eq!(arena.0[leaf], page | PRESENT | accessed_dirty);
        assert!(!unsafe { tables.verify(page..page + PAGE_BYTES, true) });
        assert!(unsafe { tables.verify(page..page + PAGE_BYTES, false) });
        unsafe { tables.set(page..page + PAGE_BYTES, true, false) };
        assert_eq!(
            arena.0[leaf],
            page | PRESENT | WRITABLE | NO_EXECUTE | accessed_dirty
        );

        arena.0[2 * ENTRIES] |= LARGE_PAGE;
        assert!(!unsafe { tables.verify(page..page + PAGE_BYTES, true) });
        unsafe {
            ptr::addr_of_mut!(arena.0[2 * ENTRIES])
                .write_volatile((root + 4 * PAGE_BYTES) | PRESENT | WRITABLE);
        }
        assert!(!unsafe { tables.verify(page..page + PAGE_BYTES, true) });
    }
}
