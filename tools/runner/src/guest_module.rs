//! Finds the PE image that contains a guest-virtual address, using only PE headers read from
//! guest memory (S8-10, STAGE_8.md 4.4). No address table or module signature of any guest binary
//! is built in: the search walks back from the page of the address in 4 KiB steps until a page
//! starts with `MZ` whose `e_lfanew` points at `PE\0\0` and whose `SizeOfImage` covers the
//! address, then reads the CodeView `RSDS` record of the debug directory.

/// How far below the address the search goes (STAGE_8.md S8-10 item 2).
pub const SCAN_LIMIT_BYTES: u64 = 64 << 20;
const PAGE: u64 = 0x1000;
const DEBUG_DIRECTORY_INDEX: usize = 6;
const DEBUG_ENTRY_BYTES: usize = 28;
const MAX_DEBUG_ENTRIES: usize = 16;
const IMAGE_DEBUG_TYPE_CODEVIEW: u32 = 2;
/// A PDB path is a file name; the record is read up to this size.
const CODEVIEW_READ_LIMIT: usize = 1024;

/// Why a read of guest memory ended early.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadFault {
    /// Leading bytes of the output that are valid.
    pub read: usize,
    pub reason: String,
}

/// Read-only access to the guest-virtual space of the stopped vCPU.
pub trait GuestMemory {
    fn read(&self, linear: u64, output: &mut [u8]) -> Result<(), ReadFault>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodeView {
    pub guid: [u8; 16],
    pub age: u32,
    pub pdb_path: String,
}

impl CodeView {
    /// `XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX`: the first three fields are little-endian.
    pub fn guid_text(&self) -> String {
        let g = &self.guid;
        format!(
            "{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
            u32::from_le_bytes([g[0], g[1], g[2], g[3]]),
            u16::from_le_bytes([g[4], g[5]]),
            u16::from_le_bytes([g[6], g[7]]),
            g[8],
            g[9],
            g[10],
            g[11],
            g[12],
            g[13],
            g[14],
            g[15]
        )
    }

    /// The directory name a symbol server uses: the GUID without dashes, then the age in hex.
    pub fn symbol_key(&self) -> String {
        format!("{}{:X}", self.guid_text().replace('-', ""), self.age)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Module {
    pub base: u64,
    pub size_of_image: u32,
    pub time_date_stamp: u32,
    pub machine: u16,
    /// The CodeView record, or why it could not be read (for example, the debug data lives in a
    /// discarded section).
    pub codeview: Result<CodeView, String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Lookup {
    Found { module: Module, pages_scanned: u64 },
    NotFound { reason: String, pages_scanned: u64 },
}

/// Finds the image that contains `address`.
pub fn find_module(memory: &impl GuestMemory, address: u64) -> Lookup {
    let first_page = address & !(PAGE - 1);
    let mut pages_scanned = 0;
    for step in 0..=SCAN_LIMIT_BYTES / PAGE {
        let Some(page) = first_page.checked_sub(step * PAGE) else {
            return Lookup::NotFound {
                reason: "the search reached address 0 without a PE header".to_owned(),
                pages_scanned,
            };
        };
        let mut head = [0u8; 2];
        if let Err(fault) = memory.read(page, &mut head) {
            return Lookup::NotFound {
                reason: format!("page 0x{page:016x} is not readable: {}", fault.reason),
                pages_scanned,
            };
        }
        pages_scanned += 1;
        if head != *b"MZ" {
            continue;
        }
        if let Some(module) = parse_module(memory, page)
            && address - page < u64::from(module.size_of_image)
        {
            return Lookup::Found {
                module,
                pages_scanned,
            };
        }
    }
    Lookup::NotFound {
        reason: format!(
            "no PE header whose SizeOfImage covers the address within {} MiB below it",
            SCAN_LIMIT_BYTES >> 20
        ),
        pages_scanned,
    }
}

fn le16(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn le32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// Validates the headers of the candidate image at `base` (inside its first page) and reads
/// its debug directory. `None`: not a PE image.
fn parse_module(memory: &impl GuestMemory, base: u64) -> Option<Module> {
    let mut page = [0u8; PAGE as usize];
    memory.read(base, &mut page).ok()?;
    let pe = le32(&page, 0x3c)? as usize;
    // The signature, the COFF header and the optional header must fit in the first page.
    if pe < 0x40 || page.get(pe..pe + 4)? != b"PE\0\0" {
        return None;
    }
    let machine = le16(&page, pe + 4)?;
    let time_date_stamp = le32(&page, pe + 8)?;
    let optional_size = usize::from(le16(&page, pe + 20)?);
    let optional = pe + 24;
    if optional + optional_size > page.len() {
        return None;
    }
    // PE32+ (0x20b) and PE32 (0x10b) differ only in where the data directories start.
    let (directories, count_at) = match le16(&page, optional)? {
        0x20b => (optional + 112, optional + 108),
        0x10b => (optional + 96, optional + 92),
        _ => return None,
    };
    let size_of_image = le32(&page, optional + 56)?;
    if size_of_image == 0 || count_at + 4 > optional + optional_size {
        return None;
    }
    let directory_count = le32(&page, count_at)? as usize;
    let debug_entry = directories + DEBUG_DIRECTORY_INDEX * 8;
    let codeview = if base.checked_add(u64::from(size_of_image)).is_none() {
        // Every read below stays inside [base, base + SizeOfImage): when that range does not fit
        // the address space, no read is attempted (guest address arithmetic would wrap).
        Err(format!(
            "SizeOfImage 0x{size_of_image:x} at image base 0x{base:x} overflows the address space"
        ))
    } else if directory_count <= DEBUG_DIRECTORY_INDEX || debug_entry + 8 > optional + optional_size
    {
        Err("the image has no debug directory".to_owned())
    } else {
        let rva = le32(&page, debug_entry)?;
        let size = le32(&page, debug_entry + 4)?;
        read_codeview(memory, base, rva, size, size_of_image)
    };
    Some(Module {
        base,
        size_of_image,
        time_date_stamp,
        machine,
        codeview,
    })
}

fn read_codeview(
    memory: &impl GuestMemory,
    base: u64,
    directory_rva: u32,
    directory_size: u32,
    size_of_image: u32,
) -> Result<CodeView, String> {
    if directory_rva == 0 || directory_size == 0 {
        return Err("the debug directory is empty".to_owned());
    }
    if directory_rva >= size_of_image {
        return Err(format!(
            "the debug directory RVA 0x{directory_rva:x} is outside the image (SizeOfImage 0x{size_of_image:x})"
        ));
    }
    // The directory must lie inside the image: cut it at SizeOfImage.
    let directory_bytes = (directory_size as usize).min((size_of_image - directory_rva) as usize);
    let entries = (directory_bytes / DEBUG_ENTRY_BYTES).min(MAX_DEBUG_ENTRIES);
    if entries == 0 {
        return Err(format!(
            "the debug directory at RVA 0x{directory_rva:x} does not hold a whole entry inside the image"
        ));
    }
    let mut raw = vec![0u8; entries * DEBUG_ENTRY_BYTES];
    let directory_address = base.checked_add(u64::from(directory_rva)).ok_or_else(|| {
        format!(
            "the debug directory at RVA 0x{directory_rva:x} overflows the address space (image base 0x{base:x})"
        )
    })?;
    memory.read(directory_address, &mut raw).map_err(|fault| {
        format!(
            "the debug directory at RVA 0x{directory_rva:x} is not readable: {}",
            fault.reason
        )
    })?;
    let mut last_error = "the debug directory has no CodeView entry".to_owned();
    for entry in raw.chunks_exact(DEBUG_ENTRY_BYTES) {
        let (Some(kind), Some(size), Some(rva)) =
            (le32(entry, 12), le32(entry, 16), le32(entry, 20))
        else {
            continue;
        };
        if kind != IMAGE_DEBUG_TYPE_CODEVIEW {
            continue;
        }
        // AddressOfRawData is an RVA inside the image; 0 means the data exists only in the file.
        if rva == 0 || size < 24 || rva >= size_of_image {
            last_error = format!("the CodeView entry is not mapped (RVA 0x{rva:x}, size {size})");
            continue;
        }
        // The record must lie inside the image: cut it at SizeOfImage.
        let available = (size_of_image - rva) as usize;
        let length = (size as usize).min(CODEVIEW_READ_LIMIT).min(available);
        if length < 24 {
            last_error = format!(
                "the CodeView record at RVA 0x{rva:x} does not fit in the image (SizeOfImage 0x{size_of_image:x})"
            );
            continue;
        }
        let Some(record_address) = base.checked_add(u64::from(rva)) else {
            last_error = format!(
                "the CodeView record at RVA 0x{rva:x} overflows the address space (image base 0x{base:x})"
            );
            continue;
        };
        let mut record = vec![0u8; length];
        if let Err(fault) = memory.read(record_address, &mut record) {
            last_error = format!(
                "the CodeView record at RVA 0x{rva:x} is not readable: {}",
                fault.reason
            );
            continue;
        }
        if record.get(..4) != Some(b"RSDS") {
            last_error = "the CodeView record is not RSDS".to_owned();
            continue;
        }
        let name = &record[24..];
        let name = &name[..name
            .iter()
            .position(|&byte| byte == 0)
            .unwrap_or(name.len())];
        return Ok(CodeView {
            guid: record[4..20].try_into().expect("16 bytes"),
            age: le32(&record, 20).expect("24 bytes"),
            pdb_path: String::from_utf8_lossy(name).into_owned(),
        });
    }
    Err(last_error)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// Guest-virtual memory made of 4 KiB pages; an absent page is unreadable.
    #[derive(Default)]
    pub(crate) struct FakeMemory(pub BTreeMap<u64, [u8; 0x1000]>);

    impl FakeMemory {
        pub fn map(&mut self, base: u64, bytes: &[u8]) {
            for (index, byte) in bytes.iter().enumerate() {
                let address = base + index as u64;
                self.0.entry(address & !0xfff).or_insert([0; 0x1000])[(address & 0xfff) as usize] =
                    *byte;
            }
        }
    }

    impl GuestMemory for FakeMemory {
        fn read(&self, linear: u64, output: &mut [u8]) -> Result<(), ReadFault> {
            for (index, byte) in output.iter_mut().enumerate() {
                let address = linear + index as u64;
                let page = self.0.get(&(address & !0xfff)).ok_or_else(|| ReadFault {
                    read: index,
                    reason: format!("no page at 0x{address:x}"),
                })?;
                *byte = page[(address & 0xfff) as usize];
            }
            Ok(())
        }
    }

    pub(crate) struct TestPe {
        pub bytes: Vec<u8>,
        pub size_of_image: u32,
        pub time_date_stamp: u32,
        pub guid: [u8; 16],
        pub age: u32,
        pub pdb: &'static str,
    }

    pub(crate) const CODEVIEW_RVA: u32 = 0x1100;

    /// A minimal PE32+ (x64) whose headers fill one page, with a section at RVA 0x1000 that
    /// holds a debug directory (at RVA 0x1000) and its RSDS record (at `CODEVIEW_RVA`).
    pub(crate) fn test_pe(pdb: &'static str, size_of_image: u32) -> TestPe {
        let mut bytes = vec![0u8; 0x2000];
        let put = |bytes: &mut Vec<u8>, at: usize, data: &[u8]| {
            bytes[at..at + data.len()].copy_from_slice(data)
        };
        put(&mut bytes, 0, b"MZ");
        put(&mut bytes, 0x3c, &0x80u32.to_le_bytes());
        put(&mut bytes, 0x80, b"PE\0\0");
        put(&mut bytes, 0x84, &0x8664u16.to_le_bytes());
        put(&mut bytes, 0x86, &1u16.to_le_bytes());
        put(&mut bytes, 0x88, &0x5f3e_1c2du32.to_le_bytes());
        put(&mut bytes, 0x94, &240u16.to_le_bytes());
        let optional = 0x98;
        put(&mut bytes, optional, &0x20bu16.to_le_bytes());
        put(&mut bytes, optional + 56, &size_of_image.to_le_bytes());
        put(&mut bytes, optional + 108, &16u32.to_le_bytes());
        // Data directory 6: the debug directory, one entry.
        put(&mut bytes, optional + 112 + 6 * 8, &0x1000u32.to_le_bytes());
        put(&mut bytes, optional + 112 + 6 * 8 + 4, &28u32.to_le_bytes());
        let guid = *b"\x78\x56\x34\x12\xbc\x9a\xf0\xde\x01\x23\x45\x67\x89\xab\xcd\xef";
        let record = {
            let mut record = b"RSDS".to_vec();
            record.extend_from_slice(&guid);
            record.extend_from_slice(&7u32.to_le_bytes());
            record.extend_from_slice(pdb.as_bytes());
            record.push(0);
            record
        };
        let entry = 0x1000;
        put(&mut bytes, entry + 12, &2u32.to_le_bytes());
        put(&mut bytes, entry + 16, &(record.len() as u32).to_le_bytes());
        put(&mut bytes, entry + 20, &CODEVIEW_RVA.to_le_bytes());
        put(&mut bytes, CODEVIEW_RVA as usize, &record);
        TestPe {
            bytes,
            size_of_image,
            time_date_stamp: 0x5f3e_1c2d,
            guid,
            age: 7,
            pdb,
        }
    }

    #[test]
    fn finds_the_image_page_walking_back_and_reads_rsds() {
        let pe = test_pe("example.pdb", 0x4000);
        let base = 0xffff_f800_1234_0000;
        let mut memory = FakeMemory::default();
        memory.map(base, &pe.bytes);
        // The image is 0x4000 bytes: map the rest as zero pages, then ask about RVA 0x3f10.
        memory.map(base + 0x2000, &[0; 0x2000]);
        let Lookup::Found {
            module,
            pages_scanned,
        } = find_module(&memory, base + 0x3f10)
        else {
            panic!("module not found");
        };
        assert_eq!(
            pages_scanned, 4,
            "pages 0x3000, 0x2000, 0x1000 and the header"
        );
        assert_eq!(module.base, base);
        assert_eq!(module.size_of_image, pe.size_of_image);
        assert_eq!(module.time_date_stamp, pe.time_date_stamp);
        assert_eq!(module.machine, 0x8664);
        let codeview = module.codeview.unwrap();
        assert_eq!(codeview.pdb_path, pe.pdb);
        assert_eq!(codeview.guid, pe.guid);
        assert_eq!(codeview.age, pe.age);
        assert_eq!(codeview.guid_text(), "12345678-9ABC-DEF0-0123-456789ABCDEF");
        assert_eq!(codeview.symbol_key(), "123456789ABCDEF00123456789ABCDEF7");
    }

    #[test]
    fn reads_a_pe32_image_whose_directories_start_earlier() {
        // PE32: magic 0x10b, NumberOfRvaAndSizes at optional+92, the directories at optional+96.
        let mut pe = test_pe("x86.pdb", 0x4000);
        let optional = 0x98;
        let debug_directory = pe.bytes[optional + 112 + 48..optional + 112 + 56].to_vec();
        pe.bytes[optional + 88..optional + 240].fill(0);
        pe.bytes[optional..optional + 2].copy_from_slice(&0x10bu16.to_le_bytes());
        pe.bytes[optional + 92..optional + 96].copy_from_slice(&16u32.to_le_bytes());
        pe.bytes[optional + 96 + 48..optional + 96 + 56].copy_from_slice(&debug_directory);
        let mut memory = FakeMemory::default();
        memory.map(0x40_0000, &pe.bytes);
        let Lookup::Found { module, .. } = find_module(&memory, 0x40_1234) else {
            panic!("module not found");
        };
        assert_eq!(module.size_of_image, 0x4000);
        assert_eq!(module.codeview.unwrap().pdb_path, "x86.pdb");
    }

    #[test]
    fn rejects_headers_that_do_not_cover_the_address_and_keeps_looking() {
        // An inner "image" (SizeOfImage 0x1000) sits below the address; the outer one covers it.
        let outer = test_pe("outer.pdb", 0x10_000);
        let inner = test_pe("inner.pdb", 0x1000);
        let (outer_base, inner_base) = (0x1_0000_0000u64, 0x1_0000_5000u64);
        let mut memory = FakeMemory::default();
        memory.map(outer_base, &outer.bytes);
        memory.map(outer_base + 0x2000, &[0; 0x8000]);
        memory.map(inner_base, &inner.bytes[..0x1000]);
        let Lookup::Found { module, .. } = find_module(&memory, outer_base + 0x7000) else {
            panic!("module not found");
        };
        assert_eq!(module.base, outer_base);
        assert_eq!(module.codeview.unwrap().pdb_path, "outer.pdb");
    }

    #[test]
    fn reports_why_the_search_stopped() {
        // The page of the address is not mapped.
        let memory = FakeMemory::default();
        let Lookup::NotFound {
            reason,
            pages_scanned,
        } = find_module(&memory, 0x5000)
        else {
            panic!("found a module in empty memory");
        };
        assert_eq!((pages_scanned, reason.contains("not readable")), (0, true));

        // Mapped pages with no header, then an unmapped page below them.
        let mut memory = FakeMemory::default();
        memory.map(0x8000, &[0xcc; 0x2000]);
        let Lookup::NotFound {
            reason,
            pages_scanned,
        } = find_module(&memory, 0x9123)
        else {
            panic!("found a module without headers");
        };
        assert_eq!(pages_scanned, 2);
        assert!(reason.contains("0x0000000000007000"), "{reason}");

        // The scan limit: 64 MiB of readable pages without a header.
        struct Zeros;
        impl GuestMemory for Zeros {
            fn read(&self, _: u64, output: &mut [u8]) -> Result<(), ReadFault> {
                output.fill(0);
                Ok(())
            }
        }
        let Lookup::NotFound {
            reason,
            pages_scanned,
        } = find_module(&Zeros, 0x1000_0000)
        else {
            panic!("found a module in zero memory");
        };
        assert_eq!(pages_scanned, SCAN_LIMIT_BYTES / PAGE + 1);
        assert!(reason.contains("64 MiB"), "{reason}");
        // ...and address 0 ends the walk before the limit.
        let Lookup::NotFound {
            reason,
            pages_scanned,
        } = find_module(&Zeros, 0x3000)
        else {
            panic!("found a module in zero memory");
        };
        assert_eq!(pages_scanned, 4);
        assert!(reason.contains("address 0"), "{reason}");
    }

    #[test]
    fn not_a_pe_is_skipped_and_an_unreadable_debug_directory_is_a_reason_not_a_failure() {
        // MZ with e_lfanew pointing outside the page, and MZ without the PE signature.
        let mut bad = vec![0u8; 0x1000];
        bad[..2].copy_from_slice(b"MZ");
        bad[0x3c..0x40].copy_from_slice(&0x2000u32.to_le_bytes());
        let mut no_signature = bad.clone();
        no_signature[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        let mut memory = FakeMemory::default();
        memory.map(0x10_0000, &bad);
        memory.map(0x10_1000, &no_signature);
        let Lookup::NotFound { reason, .. } = find_module(&memory, 0x10_1800) else {
            panic!("a broken header was accepted");
        };
        assert!(reason.contains("not readable"), "{reason}");

        // Only the header page is mapped: the module is found, the debug directory is not readable.
        let pe = test_pe("example.pdb", 0x2000);
        let mut memory = FakeMemory::default();
        memory.map(0x20_0000, &pe.bytes[..0x1000]);
        let Lookup::Found { module, .. } = find_module(&memory, 0x20_0000 + 0x10) else {
            panic!("module not found");
        };
        assert_eq!(module.base, 0x20_0000);
        assert!(module.codeview.unwrap_err().contains("not readable"));
    }

    #[test]
    fn an_image_at_the_top_of_the_address_space_is_a_reason_not_a_panic() {
        let pe = test_pe("example.pdb", 0x4000);
        let base = 0xffff_ffff_ffff_f000u64;
        let mut memory = FakeMemory::default();
        memory.map(base, &pe.bytes[..0x1000]);
        let Lookup::Found { module, .. } = find_module(&memory, base + 0x10) else {
            panic!("module not found");
        };
        assert_eq!(module.base, base);
        let reason = module.codeview.unwrap_err();
        assert!(reason.contains("overflows the address space"), "{reason}");
    }

    #[test]
    fn an_image_that_runs_past_the_top_of_the_address_space_is_not_read_at_all() {
        // The debug directory (RVA 0x1000) and the record would each fit, but SizeOfImage does not.
        let pe = test_pe("example.pdb", 0x4000);
        let base = 0xffff_ffff_ffff_e000u64;
        let mut memory = FakeMemory::default();
        memory.map(base, &pe.bytes);
        let watching = Watching(&memory, std::cell::Cell::new(0));
        let Lookup::Found { module, .. } = find_module(&watching, base + 0x10) else {
            panic!("module not found");
        };
        let reason = module.codeview.unwrap_err();
        assert!(reason.contains("overflows the address space"), "{reason}");
        assert!(reason.contains("SizeOfImage 0x4000"), "{reason}");
        assert!(watching.1.get() <= base + 0x1000, "{:#x}", watching.1.get());
        // The same image lower in memory fits and reads its CodeView record.
        let base = 0xffff_ffff_ffff_a000u64;
        let mut memory = FakeMemory::default();
        memory.map(base, &pe.bytes);
        let Lookup::Found { module, .. } = find_module(&memory, base + 0x10) else {
            panic!("module not found");
        };
        assert!(module.codeview.is_ok(), "{:?}", module.codeview);
    }

    /// Records the highest guest address any read touched.
    struct Watching<'a>(&'a FakeMemory, std::cell::Cell<u64>);

    impl GuestMemory for Watching<'_> {
        fn read(&self, linear: u64, output: &mut [u8]) -> Result<(), ReadFault> {
            let end = linear + output.len() as u64;
            self.1.set(self.1.get().max(end));
            self.0.read(linear, output)
        }
    }

    #[test]
    fn debug_data_is_never_read_beyond_the_image() {
        let base = 0x30_0000u64;
        // The record starts at RVA 0x1100 but the image ends at 0x1108: 8 bytes, too short.
        let pe = test_pe("example.pdb", 0x1108);
        let mut memory = FakeMemory::default();
        memory.map(base, &pe.bytes);
        let watching = Watching(&memory, std::cell::Cell::new(0));
        let Lookup::Found { module, .. } = find_module(&watching, base + 0x10) else {
            panic!("module not found");
        };
        let reason = module.codeview.unwrap_err();
        assert!(reason.contains("does not fit in the image"), "{reason}");
        // Only the header page and the 28-byte debug directory are read; nothing past SizeOfImage.
        assert!(watching.1.get() <= base + 0x1108, "{:#x}", watching.1.get());

        // A directory whose RVA is outside the image is refused before any read of it.
        let pe = test_pe("example.pdb", 0x1000);
        let mut memory = FakeMemory::default();
        memory.map(base, &pe.bytes);
        let Lookup::Found { module, .. } = find_module(&memory, base + 0x10) else {
            panic!("module not found");
        };
        let reason = module.codeview.unwrap_err();
        assert!(reason.contains("outside the image"), "{reason}");
    }
}
